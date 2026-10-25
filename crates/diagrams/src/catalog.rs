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
use dist_sys::clock::{CausalContext, Causality, Dot, Hlc, LamportClock, Timestamp, VectorClock};
use dist_sys::crdt::MvRegister;

use crate::label::{Label, sorted_entries};
use crate::model::{Claim, Diagram, DotLine, SpaceTime};

/// A figure and the file stem its renderings are written under.
pub struct Figure {
    pub stem: &'static str,
    pub diagram: Diagram,
}

/// Every figure, in the order their files are written.
pub fn all() -> Vec<Figure> {
    vec![
        vector_clock_trace(),
        clock_comparison(),
        mv_register_siblings(),
        actor_id_reuse(),
        causal_context_gap(),
    ]
}

/// Three processes, two messages: the scenario in the library test
/// `three_process_trace`. A3 is the last event on the wall clock yet
/// concurrent with B3; A1 reaches C2 only through B.
fn vector_clock_trace() -> Figure {
    let mut d = SpaceTime::new("Vector clocks: three processes, two messages");
    let (la, lb, lc) = (d.lane("A"), d.lane("B"), d.lane("C"));
    let (a, b, c) = ('A', 'B', 'C');
    let (mut va, mut vb, mut vc) = (VectorClock::new(), VectorClock::new(), VectorClock::new());

    va.tick(&a);
    let a1 = d.event(la, "A1", va.label());
    let a1_clock = va.clone();
    let m1 = va.send(&a);
    let a2 = d.event(la, "A2", va.label());
    vb.tick(&b);
    d.event(lb, "B1", vb.label());
    vb.receive(&b, &m1);
    let b2 = d.event(lb, "B2", vb.label());
    d.message(a2, b2, format!("m1 {}", m1.label()));
    vc.tick(&c);
    d.event(lc, "C1", vc.label());
    let m2 = vb.send(&b);
    let b3 = d.event(lb, "B3", vb.label());
    let b3_clock = vb.clone();
    vc.receive(&c, &m2);
    let c2 = d.event(lc, "C2", vc.label());
    let c2_clock = vc.clone();
    d.message(b3, c2, format!("m2 {}", m2.label()));
    va.tick(&a);
    let a3 = d.event(la, "A3", va.label());
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
    d.callout(
        a3,
        b3,
        format!(
            "A3 {} B3: {} and {} are incomparable, though A3 happened last",
            a3_b3.label(),
            a3_clock.label(),
            b3_clock.label()
        ),
    );
    d.callout(
        a1,
        c2,
        format!(
            "A1 {} C2: {} < {}, carried by m1 then m2",
            a1_c2.label(),
            a1_clock.label(),
            c2_clock.label()
        ),
    );
    Figure {
        stem: "vector-clock-trace",
        diagram: Diagram::SpaceTime(d),
    }
}

/// The vector clock trace again, stamped by a Lamport clock, a hybrid logical
/// clock and a vector clock at once, with B's physical clock running 25 behind
/// A's and C's. A3 and B3 are concurrent: the Lamport clock puts A3 first, the
/// HLC puts B3 first, and only the vector clocks say neither happened first.
fn clock_comparison() -> Figure {
    let mut d = SpaceTime::new("Three clocks, one trace: Lamport, hybrid logical, and vector");
    let (la, lb, lc) = (d.lane("A"), d.lane("B"), d.lane("C"));
    let (mut a, mut b, mut c) = (Process::new('A'), Process::new('B'), Process::new('C'));

    // Physical readings: A and C agree with true time, B's clock runs behind.
    const B_BEHIND: u64 = 25;
    let a1_stamps = a.local(10);
    d.event(la, "A1", a1_stamps.label());
    let m1 = a.send(20);
    let a2 = d.event(la, "A2", m1.label());
    let b1_stamps = b.local(30 - B_BEHIND);
    d.event(lb, "B1", b1_stamps.label());
    let b2_stamps = b.receive(40 - B_BEHIND, &m1);
    let b2 = d.event(lb, "B2", b2_stamps.label());
    d.message(a2, b2, "m1");
    let c1_stamps = c.local(50);
    d.event(lc, "C1", c1_stamps.label());
    let m2 = b.send(60 - B_BEHIND);
    let b3 = d.event(lb, "B3", m2.label());
    let c2_stamps = c.receive(70, &m2);
    let c2 = d.event(lc, "C2", c2_stamps.label());
    d.message(b3, c2, "m2");
    let a3_stamps = a.local(80);
    let a3 = d.event(la, "A3", a3_stamps.label());

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

    d.callout(
        a2,
        b2,
        format!(
            "B's clock runs {B_BEHIND} behind: m1 left A at {} but arrives when B reads {}, so B2 keeps A's wall time and counts on: {}",
            m1.hlc.label(),
            b2_stamps.physical,
            b2_stamps.hlc.label()
        ),
    );
    d.callout(
        a3,
        b3,
        format!(
            "A3 {} B3, yet Lamport puts A3 first ({} < {}) and the HLC puts B3 first ({} < {}); only {} and {} show neither happened first",
            a3_b3.label(),
            a3_stamps.lamport,
            m2.lamport,
            m2.hlc.label(),
            a3_stamps.hlc.label(),
            a3_stamps.vector.label(),
            m2.vector.label()
        ),
    );
    Figure {
        stem: "clock-comparison",
        diagram: Diagram::SpaceTime(d),
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

impl Label for Stamps {
    fn label(&self) -> String {
        format!(
            "clock {}\nLamport {}\nHLC {}\nVC {}",
            self.physical,
            self.lamport,
            self.hlc.label(),
            self.vector.label()
        )
    }
}

/// Two replicas write concurrently, the writes become siblings, a write that
/// was shown both siblings replaces them, and a stale copy can't bring the
/// original back. The scenario in the library test
/// `concurrent_writes_become_siblings_until_a_write_that_was_shown_both`,
/// with shorter values.
fn mv_register_siblings() -> Figure {
    let mut d = SpaceTime::new("Multi-value register: concurrent writes become siblings");
    let (l1, l2) = (d.lane("r1"), d.lane("r2"));
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
    let e1 = d.event(l1, "write", r1.label());
    r2.sync_from(&r1);
    let e2 = d.event(l2, "sync", r2.label());
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
    let e3 = d.event(l1, "write", r1.label());
    let b1 = r2.update(&'B', |shown| {
        shown_only_draft(shown);
        "edit B"
    });
    d.event(l2, "write", r2.label());
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
    let e5 = d.event(l2, "sync", r2.label());
    d.message(e3, e5, "");
    r1.sync_from(&r2);
    let e6 = d.event(l1, "sync", r1.label());
    d.message(e5, e6, "");
    assert_eq!(sorted_dots(&r1), [a2.label(), b1.label()]);
    assert_eq!(r1.label(), r2.label());

    let merged = r2.update(&'B', |shown| {
        let mut dots: Vec<&Dot<char>> = shown.iter().map(|(dot, _)| dot).collect();
        dots.sort_by_key(|dot| (dot.actor, dot.counter));
        assert_eq!(dots, [&a2, &b1]); // the writer is handed both siblings
        "merged"
    });
    let e7 = d.event(l2, "write", r2.label());
    r1.sync_from(&r2);
    let e8 = d.event(l1, "sync", r1.label());
    d.message(e7, e8, "");
    r1.sync_from(&stale);
    let e9 = d.event(l1, "sync (stale)", r1.label());
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
        format!(
            "r1 and r2 agree: {} and {} are siblings, since neither writer was shown the other's value",
            a2.label(),
            b1.label()
        ),
    );
    d.callout(
        e2,
        e9,
        format!(
            "a copy of r2 taken right after its first sync still holds {}, but r1 has already seen that write, so \"draft\" is not resurrected",
            draft.label()
        ),
    );
    Figure {
        stem: "mv-register-siblings",
        diagram: Diagram::SpaceTime(d),
    }
}

/// A replica loses its state and comes back under the same actor id, so it
/// reissues a dot it already used. The scenario in the library test
/// `reusing_an_actor_id_after_state_loss_corrupts_replicas`.
fn actor_id_reuse() -> Figure {
    let mut d = SpaceTime::new("Reusing an actor id after state loss: two writes share one name");
    let (la, lb) = (d.lane("A"), d.lane("B"));
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
    let e1 = d.event(la, "write", a.label());
    b.sync_from(&a);
    let e2 = d.event(lb, "sync", b.label());
    d.message(e1, e2, "");

    let mut a_reborn: MvRegister<char, &str> = MvRegister::new(); // disk wiped, same id
    d.event(la, "state lost", a_reborn.label());
    let second = a_reborn.update(&'A', |_| "y");
    let e4 = d.event(la, "write", a_reborn.label());
    assert_eq!(first, second);
    b.sync_from(&a_reborn);
    let e5 = d.event(lb, "sync", b.label());
    d.message(e4, e5, "");
    a_reborn.sync_from(&b);
    let e6 = d.event(la, "sync", a_reborn.label());
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
        format!(
            "both writes are dot {}: the reborn replica reissued a name it had already used",
            first.label()
        ),
    );
    d.callout(
        e5,
        e6,
        "B keeps \"x\" and A keeps \"y\": each believes it already holds the other's write, so they never converge",
    );
    Figure {
        stem: "actor-id-reuse",
        diagram: Diagram::SpaceTime(d),
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
                text: cc.label(),
                counters: inserted.to_vec(),
            },
            Claim {
                title: "VectorClock".into(),
                text: naive.label(),
                counters: claimed_by_vector,
            },
        ],
    );
    Figure {
        stem: "causal-context-gap",
        diagram: Diagram::DotLine(d),
    }
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
