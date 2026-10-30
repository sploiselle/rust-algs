//! The figures.
//!
//! Each function drives the real library API step by step, records the value
//! after every step as that event's annotation, and asserts the claims the
//! picture makes, pinned to the literal values the text relies on. [`all`]
//! runs in both the binary and the tests, so if the library ever changes
//! under a figure the assert fails loudly instead of a wrong picture being
//! drawn.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;

use algs::broadcast::{self, Cbcast};
use algs::clock::hybrid::ManualClock;
use algs::clock::{
    CausalContext, Causality, Dot, Hlc, LamportClock, LamportStamp, Timestamp, VectorClock,
};
use algs::coordination::{Kind, LamportMutex, Message};
use algs::crdt::{LamportRegister, MvRegister};
use algs::state_machine::airline::{Airline, Command, Response};
use algs::state_machine::{self, Bookkeeper, Config, Stamped};

use crate::label::{Label, bookkeeper, fields, phrase, sorted_entries, split_fields, v, voters};
use crate::model::{Claim, Diagram, DotLine, EventId, LaneId, Rich, SpaceTime, Span, rich};

/// A figure and the file stem its renderings are written under.
pub struct Figure {
    pub stem: &'static str,
    pub diagram: Diagram,
}

impl Figure {
    /// How the figure is shown, which its steps decide.
    pub fn form(&self) -> Form {
        if self.diagram.is_walkable() {
            Form::Steps
        } else {
            Form::Static
        }
    }
}

/// How a figure is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Finished, as SVG, HTML and Mermaid: a figure with a single step has
    /// nothing to walk through.
    Static,
    /// As a step-through: an HTML figure a page's player steps through, one
    /// event or note at a time. It is written for stepping (what it says at
    /// each step, what it starts from) and is finished at rest.
    Steps,
}

impl Form {
    /// As it appears in `index.json`.
    pub fn name(self) -> &'static str {
        match self {
            Form::Static => "static",
            Form::Steps => "steps",
        }
    }
}

/// Every figure, in the order their files are written.
pub fn all() -> Vec<Figure> {
    let mut figures = vec![
        concurrency_one_message(),
        concurrency_crossing_writes(),
        lamport_taking_turns(),
        lamport_no_messages(),
        lamport_clock_trace(),
        vector_clock_trace(),
        clock_comparison(),
        lamport_register_overwrite(),
        lamport_register_lost_write(),
        mv_register_siblings(),
        actor_id_reuse(),
        causal_context_gap(),
        mutex_fifo(),
        mutex_overtaking(),
        mutex_crash(),
        bookkeeper_crash(),
        cbcast_delay(),
    ];
    for f in &mut figures {
        if let (Form::Steps, Diagram::SpaceTime(d)) = (f.form(), &mut f.diagram) {
            d.step_through();
        }
    }
    figures
}

/// One message from A to B, for a reader who hasn't met a clock yet: it puts
/// A's send before B's receive, and leaves the event B has in between
/// unordered with A's. Nothing is drawn under the events. Vector clocks, kept
/// off screen, check the arrows and give the comparisons the notes quote.
fn concurrency_one_message() -> Figure {
    let mut d = SpaceTime::new("Concurrency: one message from A to B");
    let (a, b) = ('A', 'B');
    let (la, lb) = (d.actor_lane("A", a), d.actor_lane("B", b));
    let (mut va, mut vb) = (VectorClock::new(), VectorClock::new());
    let mut clocks: Vec<(EventId, VectorClock<char>)> = Vec::new();

    let m = va.send(&a);
    let a1 = d.event(la, "A1", Rich::default());
    clocks.push((a1, va.clone()));
    vb.tick(&b);
    let b1 = d.event(lb, "B1", Rich::default());
    clocks.push((b1, vb.clone()));
    vb.receive(&b, &m);
    let b2 = d.event(lb, "B2", Rich::default());
    clocks.push((b2, vb.clone()));
    let m_msg = d.message(a1, b2, "m");

    assert_arrows_match_clocks(&d, &clocks);
    let clock = |e: EventId| &clocks.iter().find(|(x, _)| *x == e).unwrap().1;
    let a1_b1 = clock(a1).partial_cmp(clock(b1));
    let a1_b2 = clock(a1).partial_cmp(clock(b2));
    assert_eq!((a1_b1, a1_b2), (None, Some(Ordering::Less)));
    assert!(
        d.recorded_before(a1, b1),
        "the note says B1 is later on the wall clock"
    );
    for (e, _) in &clocks {
        let key = d.events()[e.0].name.to_lowercase();
        d.pin_event(*e, &key);
    }
    d.pin_message(m_msg, "m");
    d.narrate(b1, "does something of its own while m is still on its way");
    d.callout_concurrent(
        a1,
        b1,
        rich![
            "{A1} and {B1} are concurrent: {B1} is later on the wall clock, but m hasn't reached {B} yet, so nothing at {B1} can depend on {A1}. Comparing them gives ",
            Span::value(format!("{a1_b1:?}"))
        ],
    );
    d.callout_before(
        a1,
        b2,
        rich![
            "{A1} comes before {B2}: {B2} is {B} receiving the message {A} sent at {A1}. Comparing them gives ",
            Span::value(format!("{a1_b2:?}"))
        ],
    );
    Figure {
        stem: "concurrency-one-message",
        diagram: Diagram::SpaceTime(d),
    }
}

/// A and B both write x and send the write to the other. B writes after A on
/// the wall clock but before A's write reaches it, so the writes are
/// concurrent, and receiving them doesn't order them. Nothing is drawn under
/// the events: a register holding x would need stamps or dots, which come
/// later, and would decide which write wins. Vector clocks, kept off screen,
/// check the arrows.
fn concurrency_crossing_writes() -> Figure {
    let mut d = SpaceTime::new("Concurrency: A and B both write x and tell each other");
    let (a, b) = ('A', 'B');
    let (la, lb) = (d.actor_lane("A", a), d.actor_lane("B", b));
    let (mut va, mut vb) = (VectorClock::new(), VectorClock::new());
    let mut clocks: Vec<(EventId, VectorClock<char>)> = Vec::new();

    let from_a = va.send(&a);
    let a1 = d.event(la, "A1", Rich::default());
    clocks.push((a1, va.clone()));
    let from_b = vb.send(&b);
    let b1 = d.event(lb, "B1", Rich::default());
    clocks.push((b1, vb.clone()));
    va.receive(&a, &from_b);
    let a2 = d.event(la, "A2", Rich::default());
    clocks.push((a2, va.clone()));
    vb.receive(&b, &from_a);
    let b2 = d.event(lb, "B2", Rich::default());
    clocks.push((b2, vb.clone()));
    let x_from_a = d.message(a1, b2, "write x");
    let x_from_b = d.message(b1, a2, "write x");

    assert_arrows_match_clocks(&d, &clocks);
    let clock = |e: EventId| &clocks.iter().find(|(x, _)| *x == e).unwrap().1;
    assert_eq!(clock(a1).partial_cmp(clock(b1)), None);
    assert!(
        d.recorded_before(a1, b1),
        "the note says B wrote later on the wall clock"
    );
    for knower in [a2, b2] {
        for write in [a1, b1] {
            assert!(
                d.happened_before(write, knower),
                "each machine ends up knowing of both writes"
            );
        }
    }
    for (e, _) in &clocks {
        let key = d.events()[e.0].name.to_lowercase();
        d.pin_event(*e, &key);
    }
    d.pin_message(x_from_a, "x-from-a");
    d.pin_message(x_from_b, "x-from-b");
    d.narrate(a1, "writes x and sends the write to {B}");
    d.narrate(
        b1,
        "writes x too, and sends it to {A}, while {A}'s write is still on its way",
    );
    d.narrate(a2, "receives {B}'s write, and now knows of both");
    d.narrate(b2, "receives {A}'s write, and now knows of both");
    d.callout_concurrent(
        a1,
        b1,
        "{A1} and {B1} are concurrent: {B} wrote x later on the wall clock, but before {A}'s write reached it, so neither write knew of the other",
    );
    d.callout(
        a2,
        b2,
        "Each machine now knows of both writes, but that doesn't order them: they are still concurrent, so nothing says which should win",
    );
    Figure {
        stem: "concurrency-crossing-writes",
        diagram: Diagram::SpaceTime(d),
    }
}

/// The arrows are wired by hand. The graph they draw must agree with the
/// clocks about every pair of events, or the picture would contradict the
/// clocks behind it.
fn assert_arrows_match_clocks(d: &SpaceTime, clocks: &[(EventId, VectorClock<char>)]) {
    assert_eq!(
        clocks.len(),
        d.events().len(),
        "every event's clock is checked"
    );
    let pasts = d.pasts();
    for (x, cx) in clocks {
        for (y, cy) in clocks {
            assert_eq!(
                pasts[y.0].contains(x),
                cx < cy,
                "the drawn arrows and the clocks disagree about {} and {}",
                cx.label(),
                cy.label()
            );
        }
    }
}

/// Three processes, two messages: the scenario in the library test
/// `three_process_trace`. A3 is the last event on the wall clock yet
/// concurrent with B3; A1 reaches C2 only through B.
///
/// Stepped through, each process's clock is in a panel from the empty clock
/// it starts with.
fn vector_clock_trace() -> Figure {
    let mut t = vector_trace();
    for lane in t.lanes {
        t.d.start_value(lane, VectorClock::<char>::new().spans());
    }
    // What a receive does, which B2 is the first to show.
    let (b2, b1_clock, m1, b2_clock) = &t.b2;
    let mut merged = b1_clock.join(m1);
    merged.tick(&'B');
    assert_eq!(&merged, b2_clock, "B2 joins m1 into its clock, then ticks");
    t.d.narrate(
        *b2,
        rich![
            "receives m1, keeps the larger of each entry, and ticks its own: ",
            v(b2_clock)
        ],
    );
    // C hears of A's events only through B.
    let (c2, c2_clock) = &t.c2;
    let [la, _, lc] = t.lanes;
    assert_eq!(c2_clock.get(&'A'), 2);
    assert!(
        t.d.messages()
            .iter()
            .all(|m| (t.d.lane_of(m.from), t.d.lane_of(m.to)) != (la, lc)),
        "A never sends to C"
    );
    t.d.narrate(
        *c2,
        rich![
            "receives m2, and so learns of {A}'s first two events though {A} never sent to {C}: ",
            v(c2_clock)
        ],
    );
    Figure {
        stem: "vector-clock-trace",
        diagram: Diagram::SpaceTime(t.d),
    }
}

/// The vector clock trace and what its figure says about it, with everything
/// the figure claims asserted.
struct VectorTrace {
    d: SpaceTime,
    lanes: [LaneId; 3],
    /// B2, B's clock just before it and the message it receives, and its clock after.
    b2: (
        EventId,
        VectorClock<char>,
        VectorClock<char>,
        VectorClock<char>,
    ),
    /// C2 and its clock.
    c2: (EventId, VectorClock<char>),
}

fn vector_trace() -> VectorTrace {
    let mut d = SpaceTime::new("Vector clocks: three processes, two messages");
    let (a, b, c) = ('A', 'B', 'C');
    let (la, lb, lc) = (
        d.actor_lane("A", a),
        d.actor_lane("B", b),
        d.actor_lane("C", c),
    );
    let (mut va, mut vb, mut vc) = (VectorClock::new(), VectorClock::new(), VectorClock::new());

    // Every event's clock, to check the drawn graph against the library.
    let mut clocks: Vec<(EventId, VectorClock<char>)> = Vec::new();

    va.tick(&a);
    let a1 = d.event(la, "A1", va.spans());
    clocks.push((a1, va.clone()));
    let a1_clock = va.clone();
    let m1 = va.send(&a);
    let a2 = d.event(la, "A2", va.spans());
    clocks.push((a2, va.clone()));
    vb.tick(&b);
    let b1 = d.event(lb, "B1", vb.spans());
    clocks.push((b1, vb.clone()));
    let b1_clock = vb.clone();
    vb.receive(&b, &m1);
    let b2 = d.event(lb, "B2", vb.spans());
    clocks.push((b2, vb.clone()));
    let m1_msg = d.message(a2, b2, rich!["m1 ", v(&m1)]);
    vc.tick(&c);
    let c1 = d.event(lc, "C1", vc.spans());
    clocks.push((c1, vc.clone()));
    let m2 = vb.send(&b);
    let b3 = d.event(lb, "B3", vb.spans());
    clocks.push((b3, vb.clone()));
    let b3_clock = vb.clone();
    vc.receive(&c, &m2);
    let c2 = d.event(lc, "C2", vc.spans());
    clocks.push((c2, vc.clone()));
    let c2_clock = vc.clone();
    let m2_msg = d.message(b3, c2, rich!["m2 ", v(&m2)]);
    va.tick(&a);
    let a3 = d.event(la, "A3", va.spans());
    clocks.push((a3, va.clone()));
    let a3_clock = va.clone();

    assert_eq!(a1_clock, VectorClock::from_iter([(a, 1)]));
    assert_eq!(m1, VectorClock::from_iter([(a, 2)]));
    assert_eq!(b3_clock, VectorClock::from_iter([(a, 2), (b, 3)]));
    assert_eq!(c2_clock, VectorClock::from_iter([(a, 2), (b, 3), (c, 2)]));
    assert_eq!(a3_clock, VectorClock::from_iter([(a, 3)]));
    let a3_b3 = a3_clock.compare(&b3_clock);
    assert_eq!(a3_b3, Causality::Concurrent);
    let a1_c2 = a1_clock.compare(&c2_clock);
    assert_eq!(a1_c2, Causality::Before);
    assert!(a1_clock < m2, "m2 carries A1 because B received m1 first");
    assert_arrows_match_clocks(&d, &clocks);
    assert!(
        d.recorded_before(b3, a3),
        "the note says A3 is later in real time"
    );
    // Pinned keys, so a post can address A1 or m2 whatever is added around them.
    for (e, _) in &clocks {
        let key = d.events()[e.0].name.to_lowercase();
        d.pin_event(*e, &key);
    }
    d.pin_message(m1_msg, "m1");
    d.pin_message(m2_msg, "m2");
    d.callout_concurrent(
        a3,
        b3,
        rich![
            "{A3} ",
            phrase(a3_b3),
            " {B3}: ",
            v(&a3_clock),
            " and ",
            v(&b3_clock),
            " are incomparable, though {A3} is later in real time"
        ],
    );
    d.callout_before(
        a1,
        c2,
        rich![
            "{A1} ",
            phrase(a1_c2),
            " {C2}: ",
            v(&a1_clock),
            " < ",
            v(&c2_clock),
            ", carried by m1 then m2"
        ],
    );
    let b2_clock = clocks.iter().find(|(e, _)| *e == b2).unwrap().1.clone();
    VectorTrace {
        d,
        lanes: [la, lb, lc],
        b2: (b2, b1_clock, m1, b2_clock),
        c2: (c2, c2_clock),
    }
}

/// Two processes taking turns, each acting only once it has heard from the
/// other: the happy path, with nothing concurrent. Every event happened
/// before the next, so the times count the events off in the order they
/// happened, as if one counter were handed back and forth in the messages.
///
/// Stepped through, each process's clock is in a panel from 0.
fn lamport_taking_turns() -> Figure {
    let mut d = SpaceTime::new("Lamport clocks: two processes taking turns");
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let (mut a, mut b) = (LamportClock::new('A'), LamportClock::new('B'));
    d.start_value(la, a.spans());
    d.start_value(lb, b.spans());
    let mut times: Vec<(EventId, u64)> = Vec::new();
    let mut record = |d: &mut SpaceTime, lane, name, clock: &LamportClock<char>| {
        let e = d.event(lane, name, clock.spans());
        times.push((e, clock.time()));
        e
    };

    a.tick();
    record(&mut d, la, "A1", &a);
    let m1 = a.tick().time();
    let a2 = record(&mut d, la, "A2", &a);
    let b0_time = b.time();
    b.receive(m1);
    let b1 = record(&mut d, lb, "B1", &b);
    let m1_msg = d.message(a2, b1, rich!["m1 ", Span::value(m1.to_string())]);
    b.tick();
    record(&mut d, lb, "B2", &b);
    let m2 = b.tick().time();
    let b3 = record(&mut d, lb, "B3", &b);
    let a2_time = a.time();
    a.receive(m2);
    let a3 = record(&mut d, la, "A3", &a);
    let m2_msg = d.message(b3, a3, rich!["m2 ", Span::value(m2.to_string())]);

    assert_eq!(
        times.iter().map(|&(_, t)| t).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6],
        "the times count the events off in the order they happened"
    );
    assert!(
        times.windows(2).all(|w| d.happened_before(w[0].0, w[1].0)),
        "the processes take turns: every event happened before the next"
    );
    assert_clock_condition(&d, &times);
    let time = |e: EventId| times.iter().find(|(x, _)| *x == e).unwrap().1;
    // B's start value is off the panel by B1's step, so B1 can't quote it.
    assert_eq!((b0_time, time(b1)), (0, m1 + 1), "B1 is B's first event");
    d.narrate(
        b1,
        rich![
            "receives a message sent at time ",
            Span::value(m1.to_string()),
            " as its first event, so its clock takes that and adds one, reaching ",
            Span::value(time(b1).to_string())
        ],
    );
    assert!(a2_time < m2, "A's clock jumps forward");
    narrate_receive(&mut d, a3, a2_time, m2, time(a3));
    for (e, _) in &times {
        let key = d.events()[e.0].name.to_lowercase();
        d.pin_event(*e, &key);
    }
    d.pin_message(m1_msg, "m1");
    d.pin_message(m2_msg, "m2");
    Figure {
        stem: "lamport-taking-turns",
        diagram: Diagram::SpaceTime(d),
    }
}

/// Two processes that never exchange a message: A ticks twice and B once,
/// last on the wall clock. C(B1) < C(A2), yet B1 ↛ A2: no arrow joins B1 to
/// anything A did, so nothing orders them.
///
/// Stepped through, each process's clock is in a panel from 0.
fn lamport_no_messages() -> Figure {
    let mut d = SpaceTime::new("Lamport clocks: two processes that never speak");
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let (mut a, mut b) = (LamportClock::new('A'), LamportClock::new('B'));
    d.start_value(la, a.spans());
    d.start_value(lb, b.spans());
    let mut times: Vec<(EventId, u64)> = Vec::new();
    let mut record = |d: &mut SpaceTime, lane, name, clock: &LamportClock<char>| {
        let e = d.event(lane, name, clock.spans());
        times.push((e, clock.time()));
        e
    };

    a.tick();
    let a1 = record(&mut d, la, "A1", &a);
    a.tick();
    let a2 = record(&mut d, la, "A2", &a);
    b.tick();
    let b1 = record(&mut d, lb, "B1", &b);

    assert_eq!(
        times.iter().map(|&(_, t)| t).collect::<Vec<_>>(),
        [1, 2, 1],
        "A counts to two and B to one"
    );
    assert_clock_condition(&d, &times);
    let time = |e: EventId| times.iter().find(|(x, _)| *x == e).unwrap().1;
    assert!(
        times
            .iter()
            .all(|&(e, _)| e == b1 || (!d.happened_before(e, b1) && !d.happened_before(b1, e))),
        "no event on A is ordered with B1"
    );
    assert!(d.recorded_before(a2, b1), "B1 is later on the wall clock");
    assert_eq!(time(a1), time(b1), "A1 and B1 share a time");
    assert!(time(b1) < time(a2), "the note says C(B1) < C(A2)");
    for (e, _) in &times {
        let key = d.events()[e.0].name.to_lowercase();
        d.pin_event(*e, &key);
    }
    d.narrate(
        b1,
        rich![
            "ticks to ",
            Span::value(time(b1).to_string()),
            " having heard nothing from {A}"
        ],
    );
    d.callout_concurrent(
        b1,
        a2,
        rich![
            "C({B1}) = ",
            Span::value(time(b1).to_string()),
            " and C({A2}) = ",
            Span::value(time(a2).to_string()),
            ", so C({B1}) < C({A2}). Yet {B1} ",
            Span::part("↛", "sym"),
            " {A2}: no message ever passed between {A} and {B}, so nothing orders them."
        ],
    );
    Figure {
        stem: "lamport-no-messages",
        diagram: Diagram::SpaceTime(d),
    }
}

/// What a Lamport receive does by rule 2: its process's time before it, the
/// time the message was sent at, and its time after.
fn narrate_receive(d: &mut SpaceTime, event: EventId, before: u64, sent: u64, after: u64) {
    assert_eq!(before.max(sent) + 1, after, "rule 2: the larger, plus one");
    d.narrate(
        event,
        rich![
            "receives a message sent at time ",
            Span::value(sent.to_string()),
            ": its clock takes the larger of its own ",
            Span::value(before.to_string()),
            " and that, then adds one, reaching ",
            Span::value(after.to_string())
        ],
    );
}

/// The vector clock trace stamped by Lamport clocks alone: the scenario in
/// the library test `three_process_trace`. A1 reaches C2 through B, and its
/// time is smaller; A3 and B3 are concurrent, and their times are ordered
/// anyway.
///
/// Stepped through, each process's clock is in a panel from 0.
fn lamport_clock_trace() -> Figure {
    let mut t = lamport_trace();
    for (lane, process) in t.lanes.into_iter().zip(['A', 'B', 'C']) {
        t.d.start_value(lane, LamportClock::new(process).spans());
    }
    for (event, before, sent, after) in [t.b2, t.c2] {
        narrate_receive(&mut t.d, event, before, sent, after);
    }
    let (a3, a3_time, b3_time) = t.a3;
    assert!(a3_time < b3_time);
    t.d.narrate(
        a3,
        rich![
            "ticks to ",
            Span::value(a3_time.to_string()),
            ", below {B3}'s ",
            Span::value(b3_time.to_string()),
            ", though nothing passed between them"
        ],
    );
    Figure {
        stem: "lamport-clock-trace",
        diagram: Diagram::SpaceTime(t.d),
    }
}

/// The Lamport trace and what its figure says about it, with everything the
/// figure claims asserted.
struct LamportTrace {
    d: SpaceTime,
    lanes: [LaneId; 3],
    /// B2 and C2, the receives: each event, its process's time before it,
    /// the time the message was sent at, and its time after.
    b2: (EventId, u64, u64, u64),
    c2: (EventId, u64, u64, u64),
    /// A3, its time, and B3's.
    a3: (EventId, u64, u64),
}

fn lamport_trace() -> LamportTrace {
    let mut d = SpaceTime::new("Lamport clocks: three processes, two messages");
    let (la, lb, lc) = (
        d.actor_lane("A", 'A'),
        d.actor_lane("B", 'B'),
        d.actor_lane("C", 'C'),
    );
    let (mut a, mut b, mut c) = (
        LamportClock::new('A'),
        LamportClock::new('B'),
        LamportClock::new('C'),
    );
    let mut times: Vec<(EventId, u64)> = Vec::new();
    let mut record = |d: &mut SpaceTime, lane, name, clock: &LamportClock<char>| {
        let e = d.event(lane, name, clock.spans());
        times.push((e, clock.time()));
        e
    };

    a.tick();
    let a1 = record(&mut d, la, "A1", &a);
    let m1 = a.tick().time();
    let a2 = record(&mut d, la, "A2", &a);
    b.tick();
    record(&mut d, lb, "B1", &b);
    let b1_time = b.time();
    b.receive(m1);
    let b2 = record(&mut d, lb, "B2", &b);
    let m1_msg = d.message(a2, b2, rich!["m1 ", Span::value(m1.to_string())]);
    c.tick();
    record(&mut d, lc, "C1", &c);
    let c1_time = c.time();
    let m2 = b.tick().time();
    let b3 = record(&mut d, lb, "B3", &b);
    c.receive(m2);
    let c2 = record(&mut d, lc, "C2", &c);
    let m2_msg = d.message(b3, c2, rich!["m2 ", Span::value(m2.to_string())]);
    a.tick();
    let a3 = record(&mut d, la, "A3", &a);

    assert_eq!(
        times.iter().map(|&(_, t)| t).collect::<Vec<_>>(),
        [1, 2, 1, 3, 1, 4, 5, 3]
    );
    assert_clock_condition(&d, &times);
    let time = |e: EventId| times.iter().find(|(x, _)| *x == e).unwrap().1;
    assert!(
        d.recorded_before(b3, a3),
        "the note says A3 is later in real time"
    );
    for (e, _) in &times {
        let key = d.events()[e.0].name.to_lowercase();
        d.pin_event(*e, &key);
    }
    d.pin_message(m1_msg, "m1");
    d.pin_message(m2_msg, "m2");
    d.callout_concurrent(
        a3,
        b3,
        rich![
            "{A3} is concurrent with {B3}, yet its time is smaller, ",
            Span::value(time(a3).to_string()),
            " < ",
            Span::value(time(b3).to_string()),
            ", though {A3} is later in real time: the clock orders events neither could have influenced"
        ],
    );
    d.callout_before(
        a1,
        c2,
        rich![
            "{A1} happened before {C2}, carried by m1 then m2, and its time is smaller: ",
            Span::value(time(a1).to_string()),
            " < ",
            Span::value(time(c2).to_string())
        ],
    );
    LamportTrace {
        d,
        lanes: [la, lb, lc],
        b2: (b2, b1_time, m1, time(b2)),
        c2: (c2, c1_time, m2, time(c2)),
        a3: (a3, time(a3), time(b3)),
    }
}

/// The vector clock trace again, stamped by a Lamport clock, a hybrid logical
/// clock and a vector clock at once, with B's physical clock running 25 behind
/// A's and C's. A3 and B3 are concurrent: the Lamport clock puts A3 first, the
/// HLC puts B3 first, and only the vector clocks say neither happened first.
///
/// Stepped through, each process's three clocks are in a panel from where
/// they start.
fn clock_comparison() -> Figure {
    let mut r = clock_comparison_run();
    for (lane, id) in r.lanes.into_iter().zip(['A', 'B', 'C']) {
        let start = Process::new(id).stamps(0, 0, Timestamp::default());
        r.d.start_value(lane, start.spans());
    }
    r.d.narrate(
        r.b2,
        rich![
            "receives m1: {B}'s own clock reads ",
            Span::value(r.b2_physical.to_string()),
            ", behind m1's wall time, so its HLC keeps ",
            Span::value(r.m1_hlc.wall.to_string()),
            " and counts on to ",
            v(&r.b2_hlc)
        ],
    );
    r.d.narrate(
        r.c2,
        rich![
            "receives m2: its vector clock takes the larger of each entry, so it now knows of {A}'s first two events: ",
            v(&r.c2_vector)
        ],
    );
    r.d.narrate(
        r.a3,
        rich![
            "the last event on the wall clock, and concurrent with {B3}: its Lamport time ",
            Span::value(r.a3_lamport.to_string()),
            " is below {B3}'s ",
            Span::value(r.b3_lamport.to_string()),
            ", but its HLC ",
            v(&r.a3_hlc),
            " is above {B3}'s ",
            v(&r.b3_hlc)
        ],
    );
    Figure {
        stem: "clock-comparison",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// The clock-comparison trace, with every claim its figure makes asserted.
fn clock_comparison_run() -> ComparisonRun {
    let mut d = SpaceTime::new("Three clocks, one trace: Lamport, hybrid logical, and vector");
    let (la, lb, lc) = (
        d.actor_lane("A", 'A'),
        d.actor_lane("B", 'B'),
        d.actor_lane("C", 'C'),
    );
    let (mut a, mut b, mut c) = (Process::new('A'), Process::new('B'), Process::new('C'));

    // Physical readings: A and C agree with true time, B's clock runs behind.
    const B_BEHIND: u64 = 25;
    let a1_stamps = a.local(10);
    let a1 = d.event(la, "A1", a1_stamps.spans());
    let m1 = a.send(20);
    let a2 = d.event(la, "A2", m1.spans());
    let b1_stamps = b.local(30 - B_BEHIND);
    let b1 = d.event(lb, "B1", b1_stamps.spans());
    let b2_stamps = b.receive(40 - B_BEHIND, &m1);
    let b2 = d.event(lb, "B2", b2_stamps.spans());
    let m1_msg = d.message(a2, b2, "m1");
    let c1_stamps = c.local(50);
    let c1 = d.event(lc, "C1", c1_stamps.spans());
    let m2 = b.send(60 - B_BEHIND);
    let b3 = d.event(lb, "B3", m2.spans());
    let c2_stamps = c.receive(70, &m2);
    let c2 = d.event(lc, "C2", c2_stamps.spans());
    let m2_msg = d.message(b3, c2, "m2");
    let a3_stamps = a.local(80);
    let a3 = d.event(la, "A3", a3_stamps.spans());

    let lamports = [
        &a1_stamps, &m1, &b1_stamps, &b2_stamps, &c1_stamps, &m2, &c2_stamps, &a3_stamps,
    ]
    .map(|s| s.lamport);
    assert_eq!(lamports, [1, 2, 1, 3, 1, 4, 5, 3]);
    let ts = |wall, logical| Timestamp { wall, logical };
    assert_eq!(m1.hlc, ts(20, 0));
    assert_eq!(
        b2_stamps.hlc,
        ts(20, 1),
        "B2 adopts m1's wall time and counts"
    );
    assert_eq!(m2.hlc, ts(35, 0));
    assert_eq!(a3_stamps.hlc, ts(80, 0));
    assert_eq!(m2.vector, VectorClock::from_iter([('A', 2), ('B', 3)]));
    assert_eq!(a3_stamps.vector, VectorClock::from_iter([('A', 3)]));

    // A1 happened before C2, and all three clocks agree.
    assert!(a1_stamps.lamport < c2_stamps.lamport && a1_stamps.hlc < c2_stamps.hlc);
    assert_eq!(
        a1_stamps.vector.compare(&c2_stamps.vector),
        Causality::Before
    );
    // A3 and B3 are concurrent, and the two total orders pick opposite sides.
    let a3_b3 = a3_stamps.vector.compare(&m2.vector);
    assert_eq!(a3_b3, Causality::Concurrent);
    assert!(a3_stamps.lamport < m2.lamport);
    assert!(a3_stamps.hlc > m2.hlc);
    // As in vector_clock_trace: the hand-wired arrows must agree with the
    // vector clocks about every pair of events.
    let stamped = [
        (a1, &a1_stamps),
        (a2, &m1),
        (b1, &b1_stamps),
        (b2, &b2_stamps),
        (c1, &c1_stamps),
        (b3, &m2),
        (c2, &c2_stamps),
        (a3, &a3_stamps),
    ];
    assert_eq!(
        stamped.len(),
        d.events().len(),
        "every event's clock is checked"
    );
    let pasts = d.pasts();
    for (x, sx) in &stamped {
        for (y, sy) in &stamped {
            assert_eq!(
                pasts[y.0].contains(x),
                sx.vector < sy.vector,
                "the drawn arrows and the clocks disagree about {} and {}",
                sx.vector.label(),
                sy.vector.label()
            );
        }
    }

    d.callout(
        a2,
        b2,
        rich![
            format!("{{B}}'s clock runs {B_BEHIND} behind: m1 left {{A}} at "),
            v(&m1.hlc),
            " but arrives when {B} reads ",
            Span::value(b2_stamps.physical.to_string()),
            ", so {B2} keeps {A}'s wall time and counts on: ",
            v(&b2_stamps.hlc)
        ],
    );
    d.callout_concurrent(
        a3,
        b3,
        rich![
            "{A3} ",
            phrase(a3_b3),
            " {B3}, yet Lamport puts {A3} first (",
            Span::value(a3_stamps.lamport.to_string()),
            " < ",
            Span::value(m2.lamport.to_string()),
            ") and the HLC puts {B3} first (",
            v(&m2.hlc),
            " < ",
            v(&a3_stamps.hlc),
            "); only ",
            v(&a3_stamps.vector),
            " and ",
            v(&m2.vector),
            " show neither happened first"
        ],
    );
    for (e, _) in &stamped {
        let key = d.events()[e.0].name.to_lowercase();
        d.pin_event(*e, &key);
    }
    d.pin_message(m1_msg, "m1");
    d.pin_message(m2_msg, "m2");
    ComparisonRun {
        d,
        lanes: [la, lb, lc],
        m1_hlc: m1.hlc,
        b2,
        b2_physical: b2_stamps.physical,
        b2_hlc: b2_stamps.hlc,
        c2,
        c2_vector: c2_stamps.vector.clone(),
        a3,
        a3_lamport: a3_stamps.lamport,
        a3_hlc: a3_stamps.hlc,
        b3_lamport: m2.lamport,
        b3_hlc: m2.hlc,
    }
}

/// The three-clock trace and what its figure says about it.
struct ComparisonRun {
    d: SpaceTime,
    lanes: [LaneId; 3],
    /// What m1 carried.
    m1_hlc: Timestamp,
    /// B2: the receive, B's physical reading, and its HLC after.
    b2: EventId,
    b2_physical: u64,
    b2_hlc: Timestamp,
    /// C2: the receive, and its vector clock after.
    c2: EventId,
    c2_vector: VectorClock<char>,
    /// A3 and B3 (which sent m2): their Lamport times and HLCs.
    a3: EventId,
    a3_lamport: u64,
    a3_hlc: Timestamp,
    b3_lamport: u64,
    b3_hlc: Timestamp,
}

/// One process carrying all three clocks, so every event is stamped by each.
struct Process {
    id: char,
    physical: Arc<ManualClock>,
    lamport: LamportClock<char>,
    hlc: Hlc<Arc<ManualClock>>,
    vector: VectorClock<char>,
}

/// What each clock said about one event, and the physical reading behind the HLC.
struct Stamps {
    physical: u64,
    lamport: u64,
    hlc: Timestamp,
    vector: VectorClock<char>,
}

impl Process {
    fn new(id: char) -> Self {
        let physical = Arc::new(ManualClock::new(0));
        Self {
            id,
            hlc: Hlc::new(Arc::clone(&physical)),
            physical,
            lamport: LamportClock::new(id),
            vector: VectorClock::new(),
        }
    }

    fn local(&mut self, physical: u64) -> Stamps {
        self.physical.set(physical);
        let lamport = self.lamport.tick().time();
        let hlc = self.hlc.now();
        self.vector.tick(&self.id);
        self.stamps(physical, lamport, hlc)
    }

    fn send(&mut self, physical: u64) -> Stamps {
        self.physical.set(physical);
        let lamport = self.lamport.tick().time();
        let hlc = self.hlc.now();
        self.vector.send(&self.id);
        self.stamps(physical, lamport, hlc)
    }

    fn receive(&mut self, physical: u64, message: &Stamps) -> Stamps {
        self.physical.set(physical);
        let lamport = self.lamport.receive(message.lamport).time();
        let hlc = self.hlc.update(message.hlc).expect("no max offset is set");
        self.vector.receive(&self.id, &message.vector);
        self.stamps(physical, lamport, hlc)
    }

    fn stamps(&self, physical: u64, lamport: u64, hlc: Timestamp) -> Stamps {
        Stamps {
            physical,
            lamport,
            hlc,
            vector: self.vector.clone(),
        }
    }
}

/// One line per clock, the values in a column (see [`fields`]): the physical
/// reading, then Lamport, hybrid logical and vector clocks. Short names keep
/// the figure narrow: `phys 10` / `L 1` / `HLC 10,0` / `VC {A:1}`.
impl Label for Stamps {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        fields(vec![
            (
                "phys",
                Span::part(self.physical.to_string(), "physical").into(),
            ),
            ("L", Span::part(self.lamport.to_string(), "lamport").into()),
            ("HLC", self.hlc.spans()),
            ("VC", self.vector.spans()),
        ])
    }
}

/// A last-writer-wins register on Lamport stamps doing its job: B syncs
/// "milk" from A, then writes "eggs", which rightly replaces it. The first
/// history in the library's `lamport_register` docs.
///
/// Stepped through, each replica's register is in a panel from empty.
fn lamport_register_overwrite() -> Figure {
    let [mut r, _] = lamport_register_histories();
    register_start_values(&mut r);
    assert_eq!(r.b_before_write.get(), Some(&"milk"));
    assert_eq!(r.b_before_write.clock().time(), 2);
    r.d.narrate(
        r.b[0],
        rich![
            "syncs from {A} and takes \"milk\", stamped ",
            v(&r.milk_stamp),
            "; receiving moves its clock past {A}'s, to ",
            Span::value("2")
        ],
    );
    r.d.narrate(
        r.eggs,
        rich![
            "writes \"eggs\", stamped ",
            v(&r.eggs_stamp),
            ", replacing \"milk\", which it was shown"
        ],
    );
    r.d.narrate(
        r.sync,
        rich![
            "syncs from {B}: ",
            v(&r.eggs_stamp),
            " beats ",
            v(&r.milk_stamp),
            ", so \"eggs\" replaces \"milk\", as {B} meant it to"
        ],
    );
    Figure {
        stem: "lamport-register-overwrite",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// The same register losing a write: B never hears of "milk", yet "eggs"
/// gets the same stamp as when it did, and replaces "milk" at A. The second
/// history in the library's `lamport_register` docs.
///
/// Stepped through, each replica's register is in a panel from empty.
fn lamport_register_lost_write() -> Figure {
    let [saw, mut r] = lamport_register_histories();
    register_start_values(&mut r);
    assert!(
        r.b_before_write.get().is_none(),
        "B has written nothing yet"
    );
    assert_eq!(
        [&r, &saw].map(|h| h.b_before_write.clock().time()),
        [2, 2],
        "B's clock reaches 2 before it writes, in both histories"
    );
    r.d.narrate(
        r.b[0],
        "does something that isn't a write to x: its clock ticks, and nothing else changes",
    );
    r.d.narrate(
        r.b[1],
        rich![
            "does something else: its clock reaches ",
            Span::value("2"),
            ", where syncing from {A} took it in the history where {B} saw milk"
        ],
    );
    r.d.narrate(
        r.eggs,
        rich![
            "writes \"eggs\", stamped ",
            v(&r.eggs_stamp),
            ": the same stamp as when it had seen milk, though it never heard from {A}"
        ],
    );
    r.d.narrate(
        r.sync,
        rich![
            "syncs from {B}: ",
            v(&r.eggs_stamp),
            " beats ",
            v(&r.milk_stamp),
            ", so \"eggs\" replaces \"milk\", which {B} never saw, and nothing records that \"milk\" was written"
        ],
    );
    Figure {
        stem: "lamport-register-lost-write",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// Each lane of a register step-through starts from an empty register.
fn register_start_values(r: &mut RegisterRun) {
    for (lane, replica) in r.lanes.into_iter().zip(['A', 'B']) {
        r.d.start_value(lane, LamportRegister::<char, &str>::new(replica).spans());
    }
}

/// One history of the Lamport register: the figure, its lanes, its events,
/// and what the figures say about it.
struct RegisterRun {
    d: SpaceTime,
    lanes: [LaneId; 2],
    /// A writes "milk".
    milk: EventId,
    /// B's events before it writes.
    b: Vec<EventId>,
    /// B writes "eggs".
    eggs: EventId,
    /// A syncs from B.
    sync: EventId,
    milk_stamp: LamportStamp<char>,
    eggs_stamp: LamportStamp<char>,
    b_before_write: LamportRegister<char, &'static str>,
    /// Both registers at the end.
    ends: [LamportRegister<char, &'static str>; 2],
}

/// Both histories, `[B saw milk, B never did]`, with the claims that compare
/// them asserted: the same stamps and the same registers at the end, though
/// in one history milk happened before eggs and in the other the two are
/// concurrent (`callout_before` and `callout_concurrent` check that against
/// the drawn arrows).
fn lamport_register_histories() -> [RegisterRun; 2] {
    let mut saw = lamport_register_run(true);
    let mut never = lamport_register_run(false);
    let stamp = LamportStamp::new;
    for r in [&saw, &never] {
        assert_eq!((r.milk_stamp, r.eggs_stamp), (stamp(1, 'A'), stamp(3, 'B')));
        assert_eq!(r.ends[0].get(), Some(&"eggs"));
    }
    assert_eq!(saw.ends, never.ends, "every register ends the same");

    saw.d.callout_before(
        saw.milk,
        saw.eggs,
        rich![
            "milk happened before eggs: {B} was shown milk and chose to replace it, so ",
            v(&saw.eggs_stamp),
            " winning over ",
            v(&saw.milk_stamp),
            " is right"
        ],
    );
    never.d.callout_concurrent(
        never.milk,
        never.eggs,
        rich![
            "milk is concurrent with eggs: {B} never heard from {A}. Yet the stamps are the same as when {B} saw milk, ",
            v(&never.milk_stamp),
            " and ",
            v(&never.eggs_stamp),
            ", so nothing in them can tell"
        ],
    );
    never.d.callout(
        never.eggs,
        never.sync,
        "{A} drops \"milk\" for \"eggs\" and ends exactly as when {B} saw milk: no one chose to discard \"milk\", and nothing on either replica records it was written",
    );
    [saw, never]
}

/// One history: A writes "milk"; B either syncs it from A or does two
/// unrelated things; B writes "eggs"; A syncs from B.
fn lamport_register_run(b_saw_milk: bool) -> RegisterRun {
    let mut d = SpaceTime::new(if b_saw_milk {
        "Last writer wins on Lamport stamps: B saw milk and replaced it"
    } else {
        "Last writer wins on Lamport stamps: B never saw milk, which is lost"
    });
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let (mut a, mut b) = (LamportRegister::new('A'), LamportRegister::new('B'));
    let mut times: Vec<(EventId, u64)> = Vec::new();
    let mut record = |d: &mut SpaceTime, lane, name, r: &LamportRegister<char, &'static str>| {
        let e = d.event(lane, name, r.spans());
        times.push((e, r.clock().time()));
        e
    };

    let milk_stamp = a.write("milk");
    let milk = record(&mut d, la, "write milk", &a);
    let mut b_events = Vec::new();
    if b_saw_milk {
        b.sync_from(&a);
        let e = record(&mut d, lb, "sync", &b);
        let m = d.message(milk, e, held(&a));
        d.pin_message(m, "milk-to-b");
        b_events.push(e);
    } else {
        for _ in 0..2 {
            b.tick();
            b_events.push(record(&mut d, lb, "local", &b));
        }
    }
    let b_before_write = b.clone();
    let eggs_stamp = b.write("eggs");
    let eggs = record(&mut d, lb, "write eggs", &b);
    a.sync_from(&b);
    let sync = record(&mut d, la, "sync", &a);
    let m = d.message(eggs, sync, held(&b));
    d.pin_message(m, "eggs-to-a");
    assert_eq!(a.entry(), Some((&eggs_stamp, &"eggs")));
    assert!(milk_stamp < eggs_stamp);
    assert_clock_condition(&d, &times);

    d.pin_event(milk, "milk");
    d.pin_event(eggs, "eggs");
    d.pin_event(sync, "a-syncs");
    RegisterRun {
        d,
        lanes: [la, lb],
        milk,
        b: b_events,
        eggs,
        sync,
        milk_stamp,
        eggs_stamp,
        b_before_write,
        ends: [a, b],
    }
}

/// What a register holds, `(1,A)→"milk"`: its `x` field, which a sync carries.
fn held(r: &LamportRegister<char, &'static str>) -> Rich {
    let rows = split_fields(&r.spans()).expect("a register is drawn as fields");
    let (_, x) = rows
        .into_iter()
        .find(|(name, _)| name == "x")
        .expect("an x field");
    x
}

/// Two replicas write concurrently, the writes become siblings, a write that
/// was shown both siblings replaces them, and a stale copy can't bring the
/// original back. The scenario in the library test
/// `concurrent_writes_become_siblings_until_a_write_that_was_shown_both`,
/// with shorter values.
///
/// Stepped through, each replica's register is in a panel from empty.
fn mv_register_siblings() -> Figure {
    let mut r = mv_register_siblings_run();
    for lane in r.lanes {
        r.d.start_value(lane, MvRegister::<char, &str>::new().spans());
    }
    let [
        write,
        sync,
        edit_a,
        edit_b,
        sync_a,
        sync_b,
        merge,
        merged,
        stale,
    ] = r.events;
    r.d.narrate(
        write,
        rich!["writes \"draft\", tagged with the dot ", v(&r.draft)],
    );
    r.d.narrate(sync, "syncs from {r1} and takes \"draft\"");
    r.d.narrate(
        edit_a,
        rich![
            "is shown only \"draft\", so \"edit A\" replaces it: dot ",
            v(&r.edit_a)
        ],
    );
    r.d.narrate(
        edit_b,
        rich![
            "is shown only \"draft\" too, so \"edit B\" replaces it: dot ",
            v(&r.edit_b),
            ". Neither write saw the other"
        ],
    );
    r.d.narrate(
        sync_a,
        "syncs from {r1} and holds both writes: neither was shown the other's, so they are siblings",
    );
    r.d.narrate(sync_b, "syncs from {r2} and holds the same two siblings");
    r.d.narrate(
        merge,
        rich![
            "is shown both siblings, so \"merged\" replaces them: dot ",
            v(&r.merge)
        ],
    );
    r.d.narrate(
        merged,
        "syncs from {r2}: \"merged\" has seen both siblings, so they give way to it",
    );
    r.d.narrate(
        stale,
        "syncs from the copy of {r2} taken after its first sync, which still holds \"draft\"; {r1} has already seen that write, so it stays gone",
    );
    Figure {
        stem: "mv-register-siblings",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// The siblings scenario, with every claim its figure makes asserted.
fn mv_register_siblings_run() -> SiblingsRun {
    let mut d = SpaceTime::new("Multi-value register: concurrent writes become siblings");
    let (l1, l2) = (d.actor_lane("r1", 'A'), d.actor_lane("r2", 'B'));
    let (mut r1, mut r2): (MvRegister<char, &str>, MvRegister<char, &str>) =
        (MvRegister::new(), MvRegister::new());

    let draft = r1.update(&'A', |_| "draft");
    assert_eq!(
        draft,
        Dot {
            actor: 'A',
            counter: 1
        }
    );
    let e1 = d.event(l1, "write", r1.spans());
    r2.sync_from(&r1);
    let e2 = d.event(l2, "sync", r2.spans());
    d.message(e1, e2, "");
    let stale = r2.clone(); // a peer that hears nothing more until the end

    // Each writer is shown only "draft", so neither write replaces the other.
    let shown_only_draft = |shown: &[(Dot<char>, &str)]| {
        assert_eq!(
            shown.iter().map(|(dot, _)| dot).collect::<Vec<_>>(),
            [&draft]
        );
    };
    let a2 = r1.update(&'A', |shown| {
        shown_only_draft(shown);
        "edit A"
    });
    let e3 = d.event(l1, "write", r1.spans());
    let b1 = r2.update(&'B', |shown| {
        shown_only_draft(shown);
        "edit B"
    });
    let e4 = d.event(l2, "write", r2.spans());
    assert_eq!(
        (&a2, &b1),
        (
            &Dot {
                actor: 'A',
                counter: 2
            },
            &Dot {
                actor: 'B',
                counter: 1
            }
        )
    );
    r2.sync_from(&r1);
    let e5 = d.event(l2, "sync", r2.spans());
    d.message(e3, e5, "");
    r1.sync_from(&r2);
    let e6 = d.event(l1, "sync", r1.spans());
    d.message(e5, e6, "");
    assert_eq!(sorted_dots(&r1), [a2.label(), b1.label()]);
    assert_eq!(r1.label(), r2.label());

    let merged = r2.update(&'B', |shown| {
        let mut dots: Vec<&Dot<char>> = shown.iter().map(|(dot, _)| dot).collect();
        dots.sort_by_key(|dot| (dot.actor, dot.counter));
        assert_eq!(dots, [&a2, &b1]); // the writer is handed both siblings
        "merged"
    });
    let e7 = d.event(l2, "write", r2.spans());
    r1.sync_from(&r2);
    let e8 = d.event(l1, "sync", r1.spans());
    d.message(e7, e8, "");
    r1.sync_from(&stale);
    let e9 = d.event(l1, "sync (stale)", r1.spans());
    d.pin_event(e9, "stale-sync");
    let live: Vec<_> = r1.entries().map(|(dot, v)| (dot.clone(), *v)).collect();
    assert_eq!(live, [(merged.clone(), "merged")]);
    assert!(
        stale.entries().all(|(dot, _)| r1.seen().contains(dot)),
        "r1 has seen every write the stale copy holds"
    );
    let before = r1.label();
    r1.sync_from(&r2);
    assert_eq!(r1.label(), before, "syncing again changes nothing");

    d.callout(
        e5,
        e6,
        rich![
            "{r1} and {r2} agree: ",
            v(&a2),
            " and ",
            v(&b1),
            " are siblings, since neither writer was shown the other's value"
        ],
    );
    d.callout(
        e2,
        e9,
        rich![
            "a copy of {r2} taken right after its first sync still holds ",
            v(&draft),
            ", but {r1} has already seen that write, so \"draft\" is not resurrected"
        ],
    );
    SiblingsRun {
        d,
        lanes: [l1, l2],
        events: [e1, e2, e3, e4, e5, e6, e7, e8, e9],
        draft,
        edit_a: a2,
        edit_b: b1,
        merge: merged,
    }
}

/// The siblings scenario and what its figure says about it.
struct SiblingsRun {
    d: SpaceTime,
    lanes: [LaneId; 2],
    /// In order: r1 writes "draft", r2 syncs, each writes, each syncs from the
    /// other, r2 writes over both, r1 syncs from it, and r1 syncs from the stale copy.
    events: [EventId; 9],
    /// The dots of "draft", of each concurrent write, and of the write over both.
    draft: Dot<char>,
    edit_a: Dot<char>,
    edit_b: Dot<char>,
    merge: Dot<char>,
}

/// A replica loses its state and comes back under the same actor id, so it
/// reissues a dot it already used. The scenario in the library test
/// `reusing_an_actor_id_after_state_loss_corrupts_replicas`.
///
/// Stepped through, each replica's register is in a panel from empty.
fn actor_id_reuse() -> Figure {
    let mut r = actor_id_reuse_run();
    for lane in r.lanes {
        r.d.start_value(lane, MvRegister::<char, &str>::new().spans());
    }
    let [write, sync, lost, rewrite, sync_b, sync_a] = r.events;
    r.d.narrate(
        write,
        rich!["writes \"x\", tagged with the dot ", v(&r.dot)],
    );
    r.d.narrate(sync, "syncs from {A} and takes \"x\"");
    r.d.narrate(
        lost,
        "loses its state: the disk is wiped, and the replica comes back as actor {A} with an empty register",
    );
    r.d.narrate(
        rewrite,
        rich![
            "writes \"y\", and from an empty register issues the dot ",
            v(&r.dot),
            " again: a name it already used"
        ],
    );
    r.d.narrate(
        sync_b,
        rich![
            "syncs from {A}: it already holds ",
            v(&r.dot),
            ", so it takes that to be the write it has and keeps \"x\""
        ],
    );
    r.d.narrate(
        sync_a,
        rich![
            "syncs from {B}, which holds ",
            v(&r.dot),
            " too: {A} believes it already has that write, and keeps \"y\""
        ],
    );
    Figure {
        stem: "actor-id-reuse",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// The reuse scenario, with every claim its figure makes asserted.
fn actor_id_reuse_run() -> ReuseRun {
    let mut d = SpaceTime::new("Reusing an actor id after state loss: two writes share one name");
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let (mut a, mut b): (MvRegister<char, &str>, MvRegister<char, &str>) =
        (MvRegister::new(), MvRegister::new());

    let first = a.update(&'A', |_| "x");
    assert_eq!(
        first,
        Dot {
            actor: 'A',
            counter: 1
        }
    );
    let e1 = d.event(la, "write", a.spans());
    b.sync_from(&a);
    let e2 = d.event(lb, "sync", b.spans());
    d.message(e1, e2, "");

    let mut a_reborn: MvRegister<char, &str> = MvRegister::new(); // disk wiped, same id
    let lost = d.event(la, "state lost", a_reborn.spans());
    d.pin_event(lost, "state-lost");
    let second = a_reborn.update(&'A', |_| "y");
    let e4 = d.event(la, "write", a_reborn.spans());
    assert_eq!(first, second);
    b.sync_from(&a_reborn);
    let e5 = d.event(lb, "sync", b.spans());
    d.message(e4, e5, "");
    a_reborn.sync_from(&b);
    let e6 = d.event(la, "sync", a_reborn.spans());
    d.message(e5, e6, "");
    assert_eq!(
        (sorted_values(&b), sorted_values(&a_reborn)),
        (vec!["x"], vec!["y"])
    );
    // Another round changes nothing: they have diverged for good.
    b.sync_from(&a_reborn);
    a_reborn.sync_from(&b);
    assert_eq!(
        (sorted_values(&b), sorted_values(&a_reborn)),
        (vec!["x"], vec!["y"])
    );

    d.callout(
        e1,
        e4,
        rich![
            "both writes are dot ",
            v(&first),
            ": the reborn replica reissued a name it had already used"
        ],
    );
    d.callout(
        e5,
        e6,
        "{B} keeps \"x\" and {A} keeps \"y\": each believes it already holds the other's write, so they never converge",
    );
    ReuseRun {
        d,
        lanes: [la, lb],
        events: [e1, e2, lost, e4, e5, e6],
        dot: first,
    }
}

/// The reuse scenario and what its figure says about it.
struct ReuseRun {
    d: SpaceTime,
    lanes: [LaneId; 2],
    /// In order: A writes, B syncs, A loses its state, writes again, B syncs
    /// from it, and A syncs from B.
    events: [EventId; 6],
    /// The dot both of A's writes got.
    dot: Dot<char>,
}

/// A causal context knows (A,1) and (A,3) but not (A,2). The smallest vector
/// clock containing (A,3) is {A:3}, which also claims (A,2). The scenario in
/// the library test `causal_context_tracks_gaps_a_vector_cannot`.
///
/// Stepped through, A:1 goes in, then A:3 past a gap at A:2. Each frame is
/// read back from the library: what the causal context contains, and the
/// smallest vector clock holding everything it has seen, whose wrong claim
/// appears as the gap opens.
fn causal_context_gap() -> Figure {
    let dot = |counter| Dot {
        actor: 'A',
        counter,
    };
    let top = 3;
    let mut d = DotLine::frames(
        "A causal context tracks a gap a vector clock cannot",
        "A",
        top,
    );
    let mut cc = CausalContext::new();
    let mut inserted: Vec<u64> = Vec::new();
    let mut wrong_per_frame = Vec::new();
    // Snapshot the line after each step. `say` gets the frame's vector clock.
    let mut frame = |d: &mut DotLine,
                     cc: &CausalContext<char>,
                     inserted: &[u64],
                     say: &dyn Fn(&VectorClock<char>) -> Rich| {
        let mut present = inserted.to_vec();
        present.sort();
        let contains: Vec<u64> = (1..=top).filter(|&c| cc.contains(&dot(c))).collect();
        assert_eq!(
            contains, present,
            "the causal context holds exactly what was inserted"
        );
        // The smallest vector clock holding every dot the context has seen.
        let naive: VectorClock<char> = [('A', cc.next_counter(&'A') - 1)].into_iter().collect();
        let claimed: Vec<u64> = (1..=top).filter(|&c| c <= naive.get(&'A')).collect();
        wrong_per_frame.push(
            claimed
                .iter()
                .copied()
                .filter(|c| !present.contains(c))
                .collect::<Vec<_>>(),
        );
        let narration = say(&naive);
        d.frame(
            present,
            vec![
                Claim {
                    title: "CausalContext".into(),
                    text: cc.spans(),
                    counters: contains,
                },
                Claim {
                    title: "VectorClock".into(),
                    text: naive.spans(),
                    counters: claimed,
                },
            ],
            narration,
        );
    };

    frame(&mut d, &cc, &inserted, &|naive| {
        rich![
            "Nothing seen yet: the causal context is ",
            v(&CausalContext::<char>::new()),
            " and the vector clock ",
            v(naive)
        ]
    });
    cc.insert(dot(1));
    inserted.push(1);
    frame(&mut d, &cc, &inserted, &|naive| {
        assert_eq!(naive, &VectorClock::from_iter([('A', 1)]));
        rich!["Insert ", v(&dot(1)), ": both claim exactly ", v(&dot(1))]
    });
    cc.insert(dot(3));
    inserted.push(3);
    assert_eq!(cc.vector(), &VectorClock::from_iter([('A', 1)]));
    assert_eq!(cc.cloud().collect::<Vec<_>>(), [dot(3)]);
    let after = cc.clone();
    frame(&mut d, &cc, &inserted, &|naive| {
        rich![
            "Insert ",
            v(&dot(3)),
            ", skipping ",
            v(&dot(2)),
            ": the causal context keeps it apart, ",
            v(&after),
            ", but the smallest vector clock holding it, ",
            v(naive),
            ", also claims ",
            v(&dot(2))
        ]
    });
    assert_eq!(
        wrong_per_frame,
        [vec![], vec![], vec![2]],
        "the vector clock overclaims exactly when the gap opens"
    );
    Figure {
        stem: "causal-context-gap",
        diagram: Diagram::DotLine(d),
    }
}

/// Lamport's mutual exclusion working: the requests and the ack from
/// `mutex_overtaking`, but delivered in the order they were sent, so A's
/// request reaches B before the ack does. B waits, and enters when A releases.
/// The scenario in the library test
/// `concurrent_requests_are_granted_in_stamp_order`.
///
/// Stepped through, each process's state is in a panel from where it starts.
fn mutex_fifo() -> Figure {
    let mut r = mutex_fifo_run();
    narrate_requests(&mut r);
    let a4 = r.a[3];
    r.d.narrate(
        a4,
        rich![
            "leaves: drops its request ",
            v(&LamportStamp::new(1, 'A')),
            " from its queue and sends a release to {B}"
        ],
    );
    Figure {
        stem: "mutex-fifo",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// A run of Lamport's mutex between processes A and B: the figure, with every
/// claim it makes asserted, its lanes, and each process's events in order.
struct MutexRun {
    d: SpaceTime,
    lanes: [LaneId; 2],
    a: Vec<EventId>,
    b: Vec<EventId>,
}

/// What the FIFO and overtaking figures say about the opening, where the two runs
/// agree: each process requests, then A gets B's request and enters. Each
/// lane starts from a process that has done nothing yet.
fn narrate_requests(r: &mut MutexRun) {
    let stamp = LamportStamp::new;
    for (lane, me) in r.lanes.into_iter().zip(['A', 'B']) {
        r.d.start_value(lane, LamportMutex::new(me, ['A', 'B']).spans());
    }
    // Both runs assert each request's stamp, and that A enters on getting B's.
    assert!(
        stamp(1, 'A') < stamp(1, 'B'),
        "a tie in time goes to the smaller id"
    );
    r.d.narrate(
        r.a[0],
        rich![
            "asks for the resource: stamps its request ",
            v(&stamp(1, 'A')),
            ", queues it, and sends it to {B}"
        ],
    );
    r.d.narrate(
        r.b[0],
        rich![
            "asks too, at the same Lamport time: ",
            v(&stamp(1, 'B')),
            " ranks after ",
            v(&stamp(1, 'A')),
            ", since {A}'s id is smaller"
        ],
    );
    r.d.narrate(
        r.a[1],
        "queues {B}'s request behind its own and acks it. Its request is first, and it has heard from {B} past it, so {A} enters",
    );
}

/// The FIFO run.
fn mutex_fifo_run() -> MutexRun {
    let mut d = SpaceTime::new("Lamport's mutex over FIFO channels: B waits its turn");
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let mut a = LamportMutex::new('A', ['A', 'B']);
    let mut b = LamportMutex::new('B', ['A', 'B']);
    let stamp = LamportStamp::new;
    let mut times: Vec<(EventId, u64)> = Vec::new();

    let req_a = only(a.request());
    let a1 = d.event(la, "request", a.spans());
    times.push((a1, a.clock().time()));
    let req_b = only(b.request());
    let b1 = d.event(lb, "request", b.spans());
    times.push((b1, b.clock().time()));
    assert_eq!(a.pending(), Some(&stamp(1, 'A')));
    assert_eq!(b.pending(), Some(&stamp(1, 'B')));

    let req_b_label = message_label(&req_b);
    let ack_a = a.receive(req_b).expect("a request is acknowledged");
    let a2 = d.event(la, "req from B", a.spans());
    times.push((a2, a.clock().time()));
    let req_b_msg = d.message(b1, a2, req_b_label);
    assert!(a.holds(), "A's request is the earliest");

    let req_a_label = message_label(&req_a);
    let ack_b = b.receive(req_a).expect("a request is acknowledged");
    let b2 = d.event(lb, "req from A", b.spans());
    times.push((b2, b.clock().time()));
    let req_a_msg = d.message(a1, b2, req_a_label);
    assert_eq!(
        b.queue().collect::<Vec<_>>(),
        [&stamp(1, 'A'), &stamp(1, 'B')]
    );
    assert!(!b.holds());

    let ack_a_label = message_label(&ack_a);
    assert_eq!((ack_a.kind, ack_a.stamp.time()), (Kind::Ack, 3));
    b.receive(ack_a);
    let b3 = d.event(lb, "ack from A", b.spans());
    times.push((b3, b.clock().time()));
    let ack_a_msg = d.message(a2, b3, ack_a_label);
    // What B knew when it entered in the overtaking run, except for the queue.
    assert_eq!(b.clock().time(), 4);
    assert_eq!(b.heard().collect::<Vec<_>>(), [(&'A', 3)]);
    assert!(
        !b.holds(),
        "B has heard from A past its request, but A's is first"
    );

    let ack_b_label = message_label(&ack_b);
    a.receive(ack_b);
    let a3 = d.event(la, "ack from B", a.spans());
    times.push((a3, a.clock().time()));
    d.message(b2, a3, ack_b_label);
    assert!(a.holds() && !b.holds());

    let release = only(a.release());
    let a4 = d.event(la, "release", a.spans());
    times.push((a4, a.clock().time()));
    let release_label = message_label(&release);
    assert_eq!((release.kind, release.stamp.time()), (Kind::Release, 5));
    b.receive(release);
    let b4 = d.event(lb, "release from A", b.spans());
    times.push((b4, b.clock().time()));
    let release_msg = d.message(a4, b4, release_label);
    assert_eq!(b.queue().collect::<Vec<_>>(), [&stamp(1, 'B')]);
    assert_eq!(b.heard().collect::<Vec<_>>(), [(&'A', 5)]);
    assert!(!a.holds() && b.holds(), "the resource passes to B");

    assert_clock_condition(&d, &times);

    d.pin_event(a2, "a-enters");
    d.pin_event(b3, "b-waits");
    d.pin_event(b4, "b-enters");
    d.pin_message(req_a_msg, "req-a");
    d.pin_message(req_b_msg, "req-b");
    d.pin_message(ack_a_msg, "ack-a");
    d.pin_message(release_msg, "release-a");
    d.callout(
        a1,
        b2,
        rich![
            "{A}'s request ",
            v(&stamp(1, 'A')),
            " reaches {B} before anything {A} sent after it, and goes ahead of {B}'s own ",
            v(&stamp(1, 'B')),
            " in {B}'s queue"
        ],
    );
    d.callout(
        a2,
        b3,
        rich![
            "{B} has heard from {A} at ",
            Span::value("3"),
            ", later than its request, but {A}'s request is first in its queue, so {B} waits"
        ],
    );
    d.callout(
        a4,
        b4,
        rich![
            "{A} releases: {B} drops ",
            v(&stamp(1, 'A')),
            " from its queue, its own request is first, and {B} enters"
        ],
    );
    MutexRun {
        d,
        lanes: [la, lb],
        a: vec![a1, a2, a3, a4],
        b: vec![b1, b2, b3, b4],
    }
}

/// Lamport's mutual exclusion over a channel that doesn't keep order: A's
/// acknowledgement of B's request overtakes A's own, earlier request, and both
/// processes enter. The scenario in the library test
/// `overtaking_breaks_mutual_exclusion`.
///
/// Stepped through, A's request is visibly still on its way when the ack A
/// sent after it reaches B.
fn mutex_overtaking() -> Figure {
    let mut r = mutex_overtaking_run();
    narrate_requests(&mut r);
    // The run asserts that both hold after B's ack, and that B's queue puts
    // A's request first once it arrives.
    r.d.narrate(
        r.b[1],
        "gets {A}'s ack before {A}'s earlier request, which is still on its way, and enters while {A} holds",
    );
    r.d.narrate(
        r.b[2],
        rich![
            "gets {A}'s request at last: ",
            v(&LamportStamp::new(1, 'A')),
            " goes ahead of its own, so {B} no longer holds by its queue, but it is already in"
        ],
    );
    Figure {
        stem: "mutex-overtaking",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// The overtaking run.
fn mutex_overtaking_run() -> MutexRun {
    let mut d = SpaceTime::new("Lamport's mutex: an overtaking message lets two processes in");
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let mut a = LamportMutex::new('A', ['A', 'B']);
    let mut b = LamportMutex::new('B', ['A', 'B']);
    let stamp = LamportStamp::new;

    let mut times: Vec<(EventId, u64)> = Vec::new();

    let req_a = only(a.request());
    let a1 = d.event(la, "request", a.spans());
    times.push((a1, a.clock().time()));
    let req_b = only(b.request());
    let b1 = d.event(lb, "request", b.spans());
    times.push((b1, b.clock().time()));
    assert_eq!(a.pending(), Some(&stamp(1, 'A')));
    assert_eq!(b.pending(), Some(&stamp(1, 'B')));
    assert!(a.pending() < b.pending(), "the tie goes to A");

    let req_b_label = message_label(&req_b);
    let ack = a.receive(req_b).expect("a request is acknowledged");
    let a2 = d.event(la, "req from B", a.spans());
    times.push((a2, a.clock().time()));
    let req_b_msg = d.message(b1, a2, req_b_label);
    assert!(a.holds(), "rightly: A's request is the earliest");

    let ack_label = message_label(&ack);
    assert_eq!((ack.kind, ack.stamp.time()), (Kind::Ack, 3));
    b.receive(ack);
    let b2 = d.event(lb, "ack from A", b.spans());
    times.push((b2, b.clock().time()));
    let ack_msg = d.message(a2, b2, ack_label);
    assert_eq!(b.heard().collect::<Vec<_>>(), [(&'A', 3)]);
    assert_eq!(b.queue().collect::<Vec<_>>(), [&stamp(1, 'B')]);
    assert!(a.holds() && b.holds(), "both processes hold the resource");

    let req_a_label = message_label(&req_a);
    let late_ack = b.receive(req_a).expect("a request is acknowledged");
    let b3 = d.event(lb, "req from A", b.spans());
    times.push((b3, b.clock().time()));
    let req_a_msg = d.message(a1, b3, req_a_label);
    assert_eq!(
        b.queue().collect::<Vec<_>>(),
        [&stamp(1, 'A'), &stamp(1, 'B')]
    );
    assert!(a.holds() && !b.holds(), "B's queue is right again");

    let late_ack_label = message_label(&late_ack);
    a.receive(late_ack);
    let a3 = d.event(la, "ack from B", a.spans());
    times.push((a3, a.clock().time()));
    d.message(b3, a3, late_ack_label);
    assert!(a.holds(), "nothing A receives takes the resource away");

    assert_clock_condition(&d, &times);
    assert!(d.recorded_before(a2, b2) && d.recorded_before(b2, b3));

    d.pin_event(a2, "a-enters");
    d.pin_event(b2, "b-enters");
    d.pin_message(req_a_msg, "req-a");
    d.pin_message(req_b_msg, "req-b");
    d.pin_message(ack_msg, "ack-a");
    d.callout(
        a2,
        b2,
        rich![
            "{A}'s ack, sent after its request, arrives first: {B} has heard from {A} at ",
            Span::value("3"),
            ", later than its own request ",
            v(&stamp(1, 'B')),
            ", decides nothing earlier from {A} is on the way, and enters while {A} holds"
        ],
    );
    d.callout(
        a1,
        b3,
        rich![
            "{A}'s request ",
            v(&stamp(1, 'A')),
            " arrives at last and goes to the head of {B}'s queue, but {B} is already in"
        ],
    );
    MutexRun {
        d,
        lanes: [la, lb],
        a: vec![a1, a2, a3],
        b: vec![b1, b2, b3],
    }
}

/// Lamport's mutual exclusion with a crash, as small as it gets. A never asks
/// for the resource; it just crashes, with B's request on its way to it. C
/// answers B, so B's request heads its queue and C has answered past it. But
/// B has to hear from every other process past its request, and A never
/// speaks again, so B never enters.
///
/// Stepped through, each process's state is in a panel from where it starts.
fn mutex_crash() -> Figure {
    let everyone = ['A', 'B', 'C'];
    let mut d = SpaceTime::new("Lamport's mutex: A crashes, and B never enters");
    let (la, lb, lc) = (
        d.actor_lane("A", 'A'),
        d.actor_lane("B", 'B'),
        d.actor_lane("C", 'C'),
    );
    for (lane, me) in [la, lb, lc].into_iter().zip(everyone) {
        d.start_value(lane, LamportMutex::new(me, everyone).spans());
    }
    let mut b = LamportMutex::new('B', everyone);
    let mut c = LamportMutex::new('C', everyone);
    let mine = LamportStamp::new(1, 'B');
    let mut times: Vec<(EventId, u64)> = Vec::new();
    let mut record = |d: &mut SpaceTime, lane, name, p: &LamportMutex<char>| {
        let e = d.event(lane, name, p.spans());
        times.push((e, p.clock().time()));
        e
    };

    let reqs = b.request();
    assert_eq!(b.pending(), Some(&mine));
    let asks = record(&mut d, lb, "request", &b);
    // B's request to A is on its way when A crashes, and is never delivered.
    let crash = d.crash(la, "crash");
    assert_eq!(reqs.len(), 2);
    to(&reqs, 'A');
    let req = to(&reqs, 'C');
    let label = message_label(&req);
    let ack = c.receive(req).expect("a request is acknowledged");
    let acks = record(&mut d, lc, "req from B", &c);
    let req_msg = d.message(asks, acks, label);
    let label = message_label(&ack);
    b.receive(ack);
    let waits = record(&mut d, lb, "ack from C", &b);
    let ack_msg = d.message(acks, waits, label);

    // Everything between live processes has arrived, so nothing else will.
    assert_eq!(b.queue().collect::<Vec<_>>(), [&mine]);
    for (q, time) in b.heard() {
        if *q == 'A' {
            assert_eq!(time, 0, "B never hears from A");
        } else {
            assert!(
                LamportStamp::new(time, *q) > mine,
                "C answers past B's request"
            );
        }
    }
    assert!(!b.holds(), "B waits on A");
    assert_clock_condition(&d, &times);

    d.pin_event(asks, "b-requests");
    d.pin_event(crash, "a-crashes");
    d.pin_event(acks, "c-acks");
    d.pin_event(waits, "b-waits");
    d.pin_message(req_msg, "req-b-c");
    d.pin_message(ack_msg, "ack-c-b");
    d.narrate(
        asks,
        rich![
            "asks for the resource: stamps its request ",
            v(&mine),
            ", queues it, and sends it to {A} and {C}"
        ],
    );
    d.narrate(
        crash,
        "crashes, with {B}'s request still on its way. Nothing sent to {A} is ever delivered",
    );
    d.narrate(acks, "queues {B}'s request and acks it");
    d.narrate(
        waits,
        rich![
            "has heard from {C} past ",
            v(&mine),
            ", and its request is first in its queue, but it has heard nothing from {A}"
        ],
    );
    d.callout(
        crash,
        waits,
        rich![
            "{B}'s request, ",
            v(&mine),
            ", heads its queue, and {C} has answered past it. But {B} must hear from {A} past it too, and {A} crashed before the request reached it: {B} never enters"
        ],
    );
    Figure {
        stem: "mutex-crash",
        diagram: Diagram::SpaceTime(d),
    }
}

/// Lamport's reliable state machine with a crash, the counterpart of
/// `mutex_crash`. A user's reservation reaches A, which votes for it and
/// crashes as its vote goes out. B and C vote too, and once voting has closed
/// and each has heard so from the other, both book the passenger: between
/// them B and C meet every majority graph, so they need nothing more from A.
/// The opening of the library test
/// `three_bookkeepers_accept_and_execute_a_command`, on the airline, with A
/// crashing once it has voted.
///
/// Stepped through, each bookkeeper's state is in a panel from where it
/// starts.
fn bookkeeper_crash() -> Figure {
    let mut r = bookkeeper_crash_run();
    for (lane, me) in r.lanes.into_iter().zip(['A', 'B', 'C']) {
        let fresh = Bookkeeper::new(me, r.config.clone(), Airline::new([(FLIGHT, 1)]));
        r.d.start_value(lane, bookkeeper_spans(0, &fresh));
    }
    // The run asserts every time, vote and decision these sentences give.
    let [a, b, c] = [&r.a, &r.b, &r.c];
    assert_eq!((r.config.delta(), FLIGHT), (10, 221));
    let set = |ps: &[char]| voters(&ps.iter().copied().collect::<BTreeSet<char>>());
    r.d.narrate(
        a[0],
        "votes to accept the user's command: reserve a seat on flight 221 for Smith. The user issued it at 1, so it's stamped 1, and it reached {A} at 4, before 1 + δ = 11 (δ is 10 here). {A} sends its vote to {B} and {C}",
    );
    r.d.narrate(
        a[1],
        "crashes as its vote goes out. Nothing sent to {A} from now on is delivered",
    );
    r.d.narrate(
        b[0],
        rich![
            "hears of the command at 9, before 1 + 2δ = 21, and votes too. ",
            set(&['A', 'B']),
            " is a majority graph, so the command is accepted, and the message {B} sends with its vote says so"
        ],
    );
    r.d.narrate(
        c[0],
        rich![
            "votes too: ",
            set(&['A', 'C']),
            " is another majority graph, so {C}'s message says the command is accepted, as {B}'s did"
        ],
    );
    r.d.narrate(
        b[1],
        "learns that {C} voted too. The message {B} sent with its vote at 9 already said the command is accepted, so {B} sends nothing",
    );
    r.d.narrate(
        c[1],
        "learns that {B} voted too, and likewise sends nothing",
    );
    r.d.narrate(
        b[2],
        rich![
            "at 21: from now on {B} votes for nothing stamped 1 or earlier, and its message says so just by when it was sent. {B} can't execute the command yet: it has heard from {A} only up to 4 and from {C} only up to 12, so for all {B} knows, ",
            set(&['A', 'C']),
            " accepted another command stamped 1"
        ],
    );
    r.d.narrate(c[2], "at 21 too, and {C}'s message says so the same way");
    r.d.narrate(
        b[3],
        "hears that {C}'s voting closed at 21 too. Every majority graph includes {B} or {C}, so no command stamped 1 or earlier that neither voted for can be accepted. {B} now knows every accepted command up to 1, says so to {C}, and executes the one there is: Smith is booked",
    );
    r.d.narrate(
        c[3],
        "hears the same, books Smith too, and says so to {B}. The figure ends with both of those messages on their way. Neither {B} nor {C} has heard from {A} since the vote it sent at 4",
    );
    Figure {
        stem: "bookkeeper-crash",
        diagram: Diagram::SpaceTime(r.d),
    }
}

/// The flight the bookkeepers' airline flies, with one seat.
const FLIGHT: u32 = 221;

/// The bookkeeper crash run.
struct BookkeeperRun {
    d: SpaceTime,
    /// The configuration the bookkeepers share.
    config: Config<char>,
    lanes: [LaneId; 3],
    /// A's events: its vote, then its crash.
    a: Vec<EventId>,
    /// B's events in order.
    b: Vec<EventId>,
    /// C's events in order.
    c: Vec<EventId>,
}

fn bookkeeper_crash_run() -> BookkeeperRun {
    const DELTA: u64 = 10;
    let config = Config::majority(['A', 'B', 'C'], DELTA, 4 * DELTA);
    assert_eq!(config.window(), 2 * DELTA, "voting closes at T + 2δ");
    let mut d = SpaceTime::new("Lamport's bookkeepers: A crashes, and B and C carry on");
    let (la, lb, lc) = (
        d.actor_lane("A", 'A'),
        d.actor_lane("B", 'B'),
        d.actor_lane("C", 'C'),
    );
    let book = |me| Bookkeeper::new(me, config.clone(), Airline::new([(FLIGHT, 1)]));
    let (mut a, mut b, mut c) = (book('A'), book('B'), book('C'));
    // What each bookkeeper's clock last read.
    let (mut ta, mut tb, mut tc) = (0, 0, 0);
    let command = Stamped::new(1, Command::reserve(FLIGHT, "Smith"));
    let set = |ps: &[char]| ps.iter().copied().collect::<BTreeSet<char>>();
    let record = |d: &mut SpaceTime, lane, name, now, p: &Bookkeeper<char, Airline>| {
        d.event(lane, name, bookkeeper_spans(now, p))
    };
    // The user issues the command at 1, and it reaches A at 4.
    run_clock(&mut a, &mut ta, 4);
    assert!(4 < command.time() + DELTA, "in time for A to vote");
    let vote_a = only_book(a.submit(4, command.clone()));
    assert_eq!(votes_of(&vote_a, &command), set(&['A']));
    let a1 = record(&mut d, la, "from user", ta, &a);
    let crash = d.crash(la, "crash");

    // A's vote reaches B and C, each in time to vote too, and each vote
    // completes a majority graph.
    assert!(
        12 < command.time() + 2 * DELTA,
        "in time for B and C to vote"
    );
    let notice_b = only_book(deliver(&mut b, &mut tb, 9, vote_a.clone()));
    assert_eq!(votes_of(&notice_b, &command), set(&['A', 'B']));
    assert!(config.contains_majority_graph(&set(&['A', 'B'])));
    let b1 = record(&mut d, lb, "vote from A", tb, &b);
    let vote_a_b = d.message(a1, b1, book_message_label(&vote_a));
    let notice_c = only_book(deliver(&mut c, &mut tc, 12, vote_a.clone()));
    assert_eq!(votes_of(&notice_c, &command), set(&['A', 'C']));
    assert!(config.contains_majority_graph(&set(&['A', 'C'])));
    let c1 = record(&mut d, lc, "vote from A", tc, &c);
    let vote_a_c = d.message(a1, c1, book_message_label(&vote_a));

    // B and C each learn the other voted, which changes nothing they say.
    assert!(deliver(&mut b, &mut tb, 14, notice_c.clone()).is_empty());
    let b2 = record(&mut d, lb, "votes from C", tb, &b);
    d.message(c1, b2, book_message_label(&notice_c));
    assert!(deliver(&mut c, &mut tc, 15, notice_b.clone()).is_empty());
    let c2 = record(&mut d, lc, "votes from B", tc, &c);
    d.message(b1, c2, book_message_label(&notice_b));
    for p in [&b, &c] {
        let pending: Vec<_> = p.pending().collect();
        assert_eq!(pending, [(&command, &set(&['A', 'B', 'C']))]);
        assert_eq!(p.known(), 0);
    }

    // Voting on the command closes at 1 + 2δ, and each says so. Neither can
    // execute it yet: until B hears from A or C in a message sent at 21 or
    // later, {A, C} could have accepted another command stamped 1, and
    // likewise {A, B} for C.
    let closes = command.time() + config.window();
    assert_eq!(closes, 21);
    run_clock(&mut b, &mut tb, closes);
    let closed_b = only_book(b.advance_to(closes));
    let b3 = record(&mut d, lb, "voting closes", tb, &b);
    run_clock(&mut c, &mut tc, closes);
    let closed_c = only_book(c.advance_to(closes));
    let c3 = record(&mut d, lc, "voting closes", tc, &c);
    for (p, closed, other) in [(&b, &closed_b, 'C'), (&c, &closed_c, 'B')] {
        assert_eq!((closed.sent, closed.votes.is_none()), (closes, true));
        assert_eq!(p.known(), 0, "{} executes nothing yet", p.me());
        let heard = p.heard().find(|(q, _)| **q == other).unwrap().1;
        assert!(heard < closes, "{} hasn't heard from {other} since", p.me());
        assert!(config.contains_majority_graph(&set(&['A', other])));
    }
    assert_eq!(b.heard().collect::<Vec<_>>(), [(&'A', 4), (&'C', 12)]);

    // Each hears the other's, and both execute the command.
    let known_b = only_book(deliver(&mut b, &mut tb, 25, closed_c.clone()));
    let b4 = record(&mut d, lb, "close from C", tb, &b);
    let closed_c_b = d.message(c3, b4, book_message_label(&closed_c));
    let known_c = only_book(deliver(&mut c, &mut tc, 26, closed_b.clone()));
    let c4 = record(&mut d, lc, "close from B", tc, &c);
    let closed_b_c = d.message(b3, c4, book_message_label(&closed_b));
    assert!(config.meets_every_majority_graph(&set(&['B', 'C'])));
    for p in [&b, &c] {
        assert_eq!(p.known(), 1, "{} knows everything up to 1", p.me());
        assert_eq!(p.executed(), [(command.clone(), Response::Booked)]);
        assert_eq!(
            p.machine().seated(FLIGHT).unwrap().collect::<Vec<_>>(),
            ["Smith"]
        );
        assert_eq!(p.pending().count(), 0);
        // A's vote is the last either heard from it.
        assert_eq!(p.heard().find(|(q, _)| **q == 'A'), Some((&'A', 4)));
    }
    assert!(a.executed().is_empty(), "A crashed before it could execute");
    // Each tells the other it knows everything up to 1, and the figure ends
    // with both messages on their way. B and C sent nothing else, and every
    // message they sent went to A too, where it was never delivered.
    for m in [&known_b, &known_c] {
        assert_eq!((m.known, m.votes.is_none()), (1, true));
    }
    assert_eq!((known_b.sent, known_c.sent), (25, 26));

    d.pin_event(a1, "a-votes");
    d.pin_event(crash, "a-crashes");
    d.pin_event(b3, "b-closes");
    d.pin_event(c3, "c-closes");
    d.pin_event(b4, "b-executes");
    d.pin_event(c4, "c-executes");
    d.pin_message(vote_a_b, "vote-a-b");
    d.pin_message(vote_a_c, "vote-a-c");
    d.pin_message(closed_c_b, "closed-c-b");
    d.pin_message(closed_b_c, "closed-b-c");
    d.callout(
        a1,
        crash,
        rich![
            "{A} votes for ",
            v(&command),
            " and crashes. Its vote still reaches {B} and {C}, but everything sent to {A} afterwards goes nowhere"
        ],
    );
    d.callout(
        b4,
        c4,
        rich![
            "{B} and {C} have each heard from the other since voting closed at 21, and every majority graph includes {B} or {C}, so without {A} they know every accepted command up to 1: both book Smith"
        ],
    );
    BookkeeperRun {
        d,
        config,
        lanes: [la, lb, lc],
        a: vec![a1, crash],
        b: vec![b1, b2, b3, b4],
        c: vec![c1, c2, c3, c4],
    }
}

/// A bookkeeper's state with its clock reading `now`, its copy shown as who
/// is booked on the flight.
fn bookkeeper_spans(now: u64, p: &Bookkeeper<char, Airline>) -> Rich {
    let mut booked = Rich::default();
    for (i, passenger) in p.machine().seated(FLIGHT).expect("the flight").enumerate() {
        if i > 0 {
            booked = booked.then(Span::value(" "));
        }
        booked = booked.then(Span::part(passenger, "booked"));
    }
    if booked.is_empty() {
        booked = Span::part("(empty)", "empty").into();
    }
    bookkeeper(now, p, ("booked", booked))
}

/// Run `p`'s clock on from `*clock` to just before `now`, one unit at a time
/// as its driver must, asserting nothing comes of it, and set `*clock` to
/// `now` for the call that follows.
fn run_clock(p: &mut Bookkeeper<char, Airline>, clock: &mut u64, now: u64) {
    assert!(now > *clock, "a clock moves forward");
    let known = p.known();
    for t in *clock + 1..now {
        assert!(p.advance_to(t).is_empty(), "{} sends at {t}", p.me());
        assert_eq!(p.known(), known, "{} learns something at {t}", p.me());
    }
    *clock = now;
}

/// Deliver `msg` to `p` at `now`, less than `δ` after it was sent, as a
/// working line does.
fn deliver(
    p: &mut Bookkeeper<char, Airline>,
    clock: &mut u64,
    now: u64,
    msg: BookMessage,
) -> Vec<BookMessage> {
    assert!(msg.sent < now && now - msg.sent < p.config().delta());
    run_clock(p, clock, now);
    p.receive(now, msg)
}

type BookMessage = state_machine::Message<char, Command>;

/// The one message a bookkeeper sends, to every other.
fn only_book(mut msgs: Vec<BookMessage>) -> BookMessage {
    assert_eq!(msgs.len(), 1, "one message, to everyone");
    msgs.pop().unwrap()
}

/// Who `msg` says voted for `command`.
fn votes_of(msg: &BookMessage, command: &Stamped<Command>) -> BTreeSet<char> {
    let votes = msg.votes.as_ref().expect("votes");
    assert_eq!(&votes.command, command);
    votes.voters.clone()
}

/// `votes {A,B}`, for a message carrying votes (the figure has one command,
/// so it leaves out which), or `sent 21` for one whose news is when it was
/// sent.
fn book_message_label(m: &BookMessage) -> Rich {
    match &m.votes {
        Some(votes) => rich!["votes ", voters(&votes.voters)],
        None => rich!["sent ", Span::value(m.sent.to_string())],
    }
}

/// Causal broadcast among three processes: the scenario in the library test
/// `an_answer_waits_for_its_question`, which is the paper's Figure 2. A
/// broadcasts a question, and B delivers it and broadcasts an answer, which
/// reaches C before the question does. C delays the answer until the question
/// arrives, then delivers both, the question first.
///
/// Stepped through, each process's state is in a panel from where it starts.
fn cbcast_delay() -> Figure {
    let mut d = SpaceTime::new("Causal broadcast: an answer waits for its question");
    let (la, lb, lc) = (
        d.actor_lane("A", 'A'),
        d.actor_lane("B", 'B'),
        d.actor_lane("C", 'C'),
    );
    let mut a: Cbcast<char, &str> = Cbcast::new('A');
    let mut b: Cbcast<char, &str> = Cbcast::new('B');
    let mut c: Cbcast<char, &str> = Cbcast::new('C');
    for (lane, p) in [(la, &a), (lb, &b), (lc, &c)] {
        d.start_value(lane, p.spans());
    }
    let payloads = |msgs: Vec<broadcast::Message<char, &'static str>>| -> Vec<&str> {
        msgs.into_iter().map(|m| m.payload).collect()
    };

    let question = a.broadcast("question");
    let a1 = d.event(la, "question", a.spans());
    assert_eq!(question.stamp, VectorClock::from_iter([('A', 1)]));
    assert_eq!(
        a.delivered(),
        &question.stamp,
        "A delivers its own as it sends it"
    );
    d.narrate(
        a1,
        rich![
            "broadcasts it, stamped ",
            v(&question.stamp),
            ": the stamp counts one broadcast from A, this one, and A delivers it as it sends it"
        ],
    );

    assert_eq!(payloads(b.receive(question.clone())), ["question"]);
    let b1 = d.event(lb, "question from A", b.spans());
    let question_b = d.message(a1, b1, broadcast_label(&question));
    d.narrate(
        b1,
        "delivers the question as it arrives: it is the first from A, and its stamp counts no other broadcast",
    );

    let answer = b.broadcast("answer");
    let b2 = d.event(lb, "answer", b.spans());
    assert_eq!(answer.stamp, VectorClock::from_iter([('A', 1), ('B', 1)]));
    assert!(question.stamp < answer.stamp);
    d.narrate(
        b2,
        rich![
            "replies, stamped ",
            v(&answer.stamp),
            ": the stamp counts the question, which B delivered before replying"
        ],
    );

    assert!(c.receive(answer.clone()).is_empty());
    let c1 = d.event(lc, "answer from B", c.spans());
    let answer_c = d.message(b2, c1, broadcast_label(&answer));
    assert_eq!(c.delayed().collect::<Vec<_>>(), [&answer]);
    assert_eq!(
        answer.stamp.get(&'B'),
        c.delivered().get(&'B') + 1,
        "the next from B"
    );
    assert!(
        answer.stamp.get(&'A') > c.delivered().get(&'A'),
        "but C hasn't delivered the question"
    );
    d.narrate(
        c1,
        "gets the answer first and delays it: it is the next from B, but C hasn't delivered the question it counts",
    );

    assert_eq!(payloads(a.receive(answer.clone())), ["answer"]);
    let a2 = d.event(la, "answer from B", a.spans());
    let answer_a = d.message(b2, a2, broadcast_label(&answer));
    d.narrate(
        a2,
        "delivers the answer as it arrives: A has delivered the question it counts",
    );

    assert_eq!(
        c.receive(question.clone()),
        [question.clone(), answer.clone()]
    );
    let c2 = d.event(lc, "question from A", c.spans());
    let question_c = d.message(a1, c2, broadcast_label(&question));
    assert_eq!(c.delayed().count(), 0);
    assert!(
        [a.delivered(), b.delivered()]
            .iter()
            .all(|v| *v == c.delivered())
    );
    d.narrate(c2, "delivers the question, and with it the delayed answer");

    assert!(d.recorded_before(c1, c2), "the answer reaches C first");
    assert_ne!(question.sender, answer.sender);
    d.pin_event(a1, "question");
    d.pin_event(b2, "answer");
    d.pin_event(c1, "c-delays");
    d.pin_event(c2, "c-delivers");
    d.pin_message(question_b, "question-to-b");
    d.pin_message(question_c, "question-to-c");
    d.pin_message(answer_a, "answer-to-a");
    d.pin_message(answer_c, "answer-to-c");
    d.callout_before(
        a1,
        b2,
        "The question happened before the answer: B delivered it, then answered. So everyone must deliver the question first",
    );
    d.callout(
        b2,
        c1,
        rich![
            "The answer ",
            v(&answer.stamp),
            " reaches C first. It comes from another sender, so FIFO channels wouldn't stop that, but it counts a broadcast from A that C hasn't delivered, so C delays it"
        ],
    );
    d.callout(
        a1,
        c2,
        "The question reaches C at last. C delivers it, and then the answer, which was waiting only on the question",
    );
    Figure {
        stem: "cbcast-delay",
        diagram: Diagram::SpaceTime(d),
    }
}

/// The clock condition on the drawn graph: whatever an arrow or a lane carries
/// into an event's past has a smaller Lamport time. `times` holds every event's
/// but a crash's, which the clock never stamps.
fn assert_clock_condition(d: &SpaceTime, times: &[(EventId, u64)]) {
    let crashes = d.lanes().iter().filter(|l| l.crash.is_some()).count();
    assert_eq!(
        times.len() + crashes,
        d.events().len(),
        "every event's time is checked"
    );
    let pasts = d.pasts();
    for (x, tx) in times {
        for (y, ty) in times {
            if pasts[y.0].contains(x) {
                assert!(tx < ty, "an arrow runs backwards in Lamport time");
            }
        }
    }
}

/// The message in a broadcast addressed to `to`.
fn to<P: PartialEq + Clone>(msgs: &[Message<P>], to: P) -> Message<P> {
    msgs.iter()
        .find(|m| m.to == to)
        .cloned()
        .expect("no message for that process")
}

/// The one message a two-process broadcast sends.
fn only<P>(mut msgs: Vec<Message<P>>) -> Message<P> {
    assert_eq!(msgs.len(), 1, "one other process, one message");
    msgs.pop().unwrap()
}

/// `req 1`, `ack 3`: what a message says and its stamp.
fn message_label<P>(m: &Message<P>) -> Rich {
    let kind = match m.kind {
        Kind::Request => "req ",
        Kind::Ack => "ack ",
        Kind::Release => "release ",
    };
    rich![kind, Span::value(m.stamp.time().to_string())]
}

/// `question {A:1}`: what a broadcast carries, and its stamp.
fn broadcast_label(m: &broadcast::Message<char, &str>) -> Rich {
    rich![m.payload, " ", v(&m.stamp)]
}

fn sorted_values(r: &MvRegister<char, &'static str>) -> Vec<&'static str> {
    let mut v: Vec<_> = r.entries().map(|(_, v)| *v).collect();
    v.sort();
    v
}

fn sorted_dots<V: Clone>(r: &MvRegister<char, V>) -> Vec<String> {
    sorted_entries(r)
        .into_iter()
        .map(|(dot, _)| dot.label())
        .collect()
}
