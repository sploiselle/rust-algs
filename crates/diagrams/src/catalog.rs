//! The figures.
//!
//! Each function drives the real library API step by step, records the value
//! after every step as that event's annotation, and asserts the claims the
//! picture makes, pinned to the literal values the text relies on. [`all`]
//! runs in both the binary and the staleness test, so if the library ever
//! changes under a figure the assert fails loudly instead of a wrong picture
//! being drawn.

use std::sync::Arc;

use dist_sys::clock::hybrid::ManualClock;
use dist_sys::clock::{
    CausalContext, Causality, Dot, Hlc, LamportClock, LamportStamp, Timestamp, VectorClock,
};
use dist_sys::coordination::{Kind, LamportMutex, Message};
use dist_sys::crdt::MvRegister;

use crate::label::{Label, fields, phrase, sorted_entries, v};
use crate::model::{Claim, Diagram, DotLine, EventId, LaneId, Rich, SpaceTime, Span, rich};

/// A figure and the file stem its renderings are written under.
pub struct Figure {
    pub stem: &'static str,
    pub diagram: Diagram,
    pub form: Form,
}

/// How a figure is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Finished, as SVG, HTML and Mermaid.
    Static,
    /// As a step-through: an HTML figure a page's player steps through, one
    /// event or note at a time. Opt-in, since a step-through is written for
    /// stepping (what it says at each step, what it starts from), and most
    /// figures read best finished.
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
    vec![
        vector_clock_trace(),
        vector_clock_steps(),
        clock_comparison(),
        mv_register_siblings(),
        actor_id_reuse(),
        causal_context_gap(),
        mutex_fifo(),
        mutex_overtaking(),
    ]
}

/// Three processes, two messages: the scenario in the library test
/// `three_process_trace`. A3 is the last event on the wall clock yet
/// concurrent with B3; A1 reaches C2 only through B.
fn vector_clock_trace() -> Figure {
    Figure {
        stem: "vector-clock-trace",
        diagram: Diagram::SpaceTime(vector_trace().d),
        form: Form::Static,
    }
}

/// The vector clock trace one event at a time, each process's clock in a
/// panel from the empty clock it starts with.
fn vector_clock_steps() -> Figure {
    let mut t = vector_trace();
    for lane in t.lanes {
        t.d.start_value(lane, VectorClock::<char>::new().spans());
    }
    Figure {
        stem: "vector-clock-steps",
        diagram: Diagram::SpaceTime(t.d),
        form: Form::Steps,
    }
}

/// The vector clock trace and what both of its figures say about it, with
/// everything either one claims asserted.
struct VectorTrace {
    d: SpaceTime,
    lanes: [LaneId; 3],
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
    // The arrows are wired by hand above. The graph they draw must agree with
    // the clocks about every pair of events, or the picture would contradict
    // the values printed on it.
    assert_eq!(
        clocks.len(),
        d.events().len(),
        "every event's clock is checked"
    );
    let pasts = d.pasts();
    for (x, cx) in &clocks {
        for (y, cy) in &clocks {
            assert_eq!(
                pasts[y.0].contains(x),
                cx < cy,
                "the drawn arrows and the clocks disagree about {} and {}",
                cx.label(),
                cy.label()
            );
        }
    }
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
            "A3 ",
            phrase(a3_b3),
            " B3: ",
            v(&a3_clock),
            " and ",
            v(&b3_clock),
            " are incomparable, though A3 is later in real time"
        ],
    );
    d.callout_before(
        a1,
        c2,
        rich![
            "A1 ",
            phrase(a1_c2),
            " C2: ",
            v(&a1_clock),
            " < ",
            v(&c2_clock),
            ", carried by m1 then m2"
        ],
    );
    VectorTrace {
        d,
        lanes: [la, lb, lc],
    }
}

/// The vector clock trace again, stamped by a Lamport clock, a hybrid logical
/// clock and a vector clock at once, with B's physical clock running 25 behind
/// A's and C's. A3 and B3 are concurrent: the Lamport clock puts A3 first, the
/// HLC puts B3 first, and only the vector clocks say neither happened first.
fn clock_comparison() -> Figure {
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
            format!("B's clock runs {B_BEHIND} behind: m1 left A at "),
            v(&m1.hlc),
            " but arrives when B reads ",
            Span::value(b2_stamps.physical.to_string()),
            ", so B2 keeps A's wall time and counts on: ",
            v(&b2_stamps.hlc)
        ],
    );
    d.callout_concurrent(
        a3,
        b3,
        rich![
            "A3 ",
            phrase(a3_b3),
            " B3, yet Lamport puts A3 first (",
            Span::value(a3_stamps.lamport.to_string()),
            " < ",
            Span::value(m2.lamport.to_string()),
            ") and the HLC puts B3 first (",
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
    Figure {
        stem: "clock-comparison",
        diagram: Diagram::SpaceTime(d),
        form: Form::Static,
    }
}

/// One process carrying all three clocks, so every event is stamped by each.
struct Process {
    id: char,
    physical: Arc<ManualClock>,
    lamport: LamportClock,
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
            lamport: LamportClock::new(),
            vector: VectorClock::new(),
        }
    }

    fn local(&mut self, physical: u64) -> Stamps {
        self.physical.set(physical);
        let lamport = self.lamport.tick();
        let hlc = self.hlc.now();
        self.vector.tick(&self.id);
        self.stamps(physical, lamport, hlc)
    }

    fn send(&mut self, physical: u64) -> Stamps {
        self.physical.set(physical);
        let lamport = self.lamport.send();
        let hlc = self.hlc.now();
        self.vector.send(&self.id);
        self.stamps(physical, lamport, hlc)
    }

    fn receive(&mut self, physical: u64, message: &Stamps) -> Stamps {
        self.physical.set(physical);
        let lamport = self.lamport.receive(message.lamport);
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

/// Two replicas write concurrently, the writes become siblings, a write that
/// was shown both siblings replaces them, and a stale copy can't bring the
/// original back. The scenario in the library test
/// `concurrent_writes_become_siblings_until_a_write_that_was_shown_both`,
/// with shorter values.
fn mv_register_siblings() -> Figure {
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
    d.event(l2, "write", r2.spans());
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
    assert_eq!(live, [(merged, "merged")]);
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
            "r1 and r2 agree: ",
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
            "a copy of r2 taken right after its first sync still holds ",
            v(&draft),
            ", but r1 has already seen that write, so \"draft\" is not resurrected"
        ],
    );
    Figure {
        stem: "mv-register-siblings",
        diagram: Diagram::SpaceTime(d),
        form: Form::Static,
    }
}

/// A replica loses its state and comes back under the same actor id, so it
/// reissues a dot it already used. The scenario in the library test
/// `reusing_an_actor_id_after_state_loss_corrupts_replicas`.
fn actor_id_reuse() -> Figure {
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
        "B keeps \"x\" and A keeps \"y\": each believes it already holds the other's write, so they never converge",
    );
    Figure {
        stem: "actor-id-reuse",
        diagram: Diagram::SpaceTime(d),
        form: Form::Static,
    }
}

/// A causal context knows (A,1) and (A,3) but not (A,2). The smallest vector
/// clock containing (A,3) is {A:3}, which also claims (A,2). The scenario in
/// the library test `causal_context_tracks_gaps_a_vector_cannot`.
fn causal_context_gap() -> Figure {
    let dot = |counter| Dot {
        actor: 'A',
        counter,
    };
    let inserted = [1, 3];
    let mut cc = CausalContext::new();
    for c in inserted {
        cc.insert(dot(c));
    }
    let top = cc.next_counter(&'A') - 1; // the highest counter known
    assert_eq!(top, 3);
    let naive: VectorClock<char> = [('A', top)].into_iter().collect(); // the smallest vector containing (A,3)
    assert_eq!(naive, VectorClock::from_iter([('A', 3)]));
    assert!(cc.contains(&dot(1)) && !cc.contains(&dot(2)) && cc.contains(&dot(3)));
    assert_eq!(cc.vector(), &VectorClock::from_iter([('A', 1)]));
    assert_eq!(cc.cloud().collect::<Vec<_>>(), [dot(3)]);
    let claimed_by_vector: Vec<u64> = (1..=top).filter(|&c| c <= naive.get(&'A')).collect();
    assert_eq!(
        claimed_by_vector,
        [1, 2, 3],
        "the plain vector claims (A,2)"
    );

    let d = DotLine::new(
        "A causal context tracks a gap a vector clock cannot",
        "A",
        top,
        inserted.to_vec(),
        vec![
            Claim {
                title: "CausalContext".into(),
                text: cc.spans(),
                counters: inserted.to_vec(),
            },
            Claim {
                title: "VectorClock".into(),
                text: naive.spans(),
                counters: claimed_by_vector,
            },
        ],
    );
    Figure {
        stem: "causal-context-gap",
        diagram: Diagram::DotLine(d),
        form: Form::Static,
    }
}

/// Lamport's mutual exclusion working: the requests and the ack from
/// `mutex_overtaking`, but delivered in the order they were sent, so A's
/// request reaches B before the ack does. B waits, and enters when A releases.
/// The scenario in the library test
/// `concurrent_requests_are_granted_in_stamp_order`.
fn mutex_fifo() -> Figure {
    let mut d = SpaceTime::new("Lamport's mutex over FIFO channels: B waits its turn");
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let mut a = LamportMutex::new('A', ['A', 'B']);
    let mut b = LamportMutex::new('B', ['A', 'B']);
    let stamp = |time, process| LamportStamp { time, process };
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
    assert_eq!((ack_a.kind, ack_a.time), (Kind::Ack, 3));
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
    assert_eq!((release.kind, release.time), (Kind::Release, 5));
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
            "A's request ",
            v(&stamp(1, 'A')),
            " reaches B before anything A sent after it, and goes ahead of B's own ",
            v(&stamp(1, 'B')),
            " in B's queue"
        ],
    );
    d.callout(
        a2,
        b3,
        rich![
            "B has heard from A at ",
            Span::value("3"),
            ", later than its request, but A's request is first in its queue, so B waits"
        ],
    );
    d.callout(
        a4,
        b4,
        rich![
            "A releases: B drops ",
            v(&stamp(1, 'A')),
            " from its queue, its own request is first, and B enters"
        ],
    );
    Figure {
        stem: "mutex-fifo",
        diagram: Diagram::SpaceTime(d),
        form: Form::Static,
    }
}

/// Lamport's mutual exclusion over a channel that doesn't keep order: A's
/// acknowledgement of B's request overtakes A's own, earlier request, and both
/// processes enter. The scenario in the library test
/// `overtaking_breaks_mutual_exclusion`.
fn mutex_overtaking() -> Figure {
    let mut d = SpaceTime::new("Lamport's mutex: an overtaking message lets two processes in");
    let (la, lb) = (d.actor_lane("A", 'A'), d.actor_lane("B", 'B'));
    let mut a = LamportMutex::new('A', ['A', 'B']);
    let mut b = LamportMutex::new('B', ['A', 'B']);
    let stamp = |time, process| LamportStamp { time, process };

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
    assert_eq!((ack.kind, ack.time), (Kind::Ack, 3));
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
            "A's ack, sent after its request, arrives first: B has heard from A at ",
            Span::value("3"),
            ", later than its own request ",
            v(&stamp(1, 'B')),
            ", decides nothing earlier from A is on the way, and enters while A holds"
        ],
    );
    d.callout(
        a1,
        b3,
        rich![
            "A's request ",
            v(&stamp(1, 'A')),
            " arrives at last and goes to the head of B's queue, but B is already in"
        ],
    );
    Figure {
        stem: "mutex-overtaking",
        diagram: Diagram::SpaceTime(d),
        form: Form::Static,
    }
}

/// The clock condition on the drawn graph: whatever an arrow or a lane carries
/// into an event's past has a smaller Lamport time. `times` holds every event's.
fn assert_clock_condition(d: &SpaceTime, times: &[(EventId, u64)]) {
    assert_eq!(
        times.len(),
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
    rich![kind, Span::value(m.time.to_string())]
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
