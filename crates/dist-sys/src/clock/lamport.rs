//! # Lamport clocks
//!
//! A Lamport clock (Lamport, *Time, Clocks, and the Ordering of Events in a
//! Distributed System*, 1978) is one counter per process. It stamps every
//! event with a number such that
//!
//! ```text
//!     e → f  (e happened-before f)   ⇒   L(e) < L(f)
//! ```
//!
//! Happened-before is defined by what could have influenced what, not by real
//! time: an event happened before every later event at its own process, a send
//! happened before its receive, and the relation is transitive. Two events
//! where neither happened before the other are concurrent.
//!
//! Two rules keep the promise:
//! 1. **Local or send event**: `L += 1`. A send attaches `L` to the message.
//! 2. **Receive** of a message stamped `m`: `L = max(L, m) + 1`.
//!
//! Rule 1 orders each process's own events. Rule 2 puts a receive after its
//! send, and after everything the sender had seen, since the sender's counter
//! was already past all of that.
//!
//! ## A total order
//!
//! Two events at different processes can share a counter value. Breaking ties
//! by process gives a [`LamportStamp`] `(L, process)`, and these are unique and
//! totally ordered. That order never contradicts happened-before, so every
//! process can sort the same events into the same sequence without asking
//! anyone. Lamport used it for [distributed mutual
//! exclusion](crate::coordination::lamport_mutex); it's the basis of
//! total-order broadcast and replicated state machines. The order between
//! concurrent events is arbitrary, though: whichever process has the smaller
//! id wins a tie.
//!
//! A [`LamportClock`] knows its process, so the stamps it hands out are these
//! pairs. Rule 2 needs only the time: a receive takes `m`, not who sent it.
//!
//! ## What it can't do
//!
//! - **Detect concurrency.** The implication only runs one way. `L(e) < L(f)`
//!   is just as consistent with `e ∥ f`, so a Lamport clock can't tell two
//!   conflicting writes from one that overwrote the other (a
//!   [`LamportRegister`](crate::crdt::LamportRegister) builds both). Deciding
//!   happened-before exactly takes a [`VectorClock`](super::VectorClock).
//! - **Say anything about real time.** The counter means nothing outside the
//!   system, and if causality flows outside it (one user tells another to make
//!   a change), the order can contradict what people saw.
//! - **Stay close together.** A busy process races ahead, anyone who hears from
//!   it jumps forward, and nothing bounds how far apart counters drift.
//!
//! A [hybrid logical clock](super::hybrid) is a Lamport clock seeded with
//! physical time, which fixes the last two; on a physical clock stuck at 0 it
//! *is* a Lamport clock.

/// A Lamport clock: one process's counter, and the process it stamps events
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LamportClock<P> {
    time: u64,
    /// Breaks ties between this process's stamps and others' at the same time.
    process: P,
}

impl<P: Clone> LamportClock<P> {
    pub fn new(process: P) -> Self {
        Self { time: 0, process }
    }

    pub fn process(&self) -> &P {
        &self.process
    }

    /// The time of the last event, or 0 before the first.
    pub fn time(&self) -> u64 {
        self.time
    }

    /// Rule 1: record a local event and return its stamp.
    pub fn tick(&mut self) -> LamportStamp<P> {
        self.time += 1;
        LamportStamp {
            time: self.time,
            process: self.process.clone(),
        }
    }

    /// Rule 1: record a send event and return its stamp, to attach to the
    /// message.
    pub fn send(&mut self) -> LamportStamp<P> {
        self.tick()
    }

    /// Rule 2: absorb the time of an incoming message, record the receive
    /// event, and return its stamp. Only the time matters to the clock, not
    /// who sent it.
    pub fn receive(&mut self, time: u64) -> LamportStamp<P> {
        self.time = self.time.max(time);
        self.tick()
    }
}

/// A Lamport stamp with its process as the tiebreaker: ordered by `time`, then
/// by `process`.
///
/// Stamps from one execution are unique (each process's own times strictly
/// increase), and sorting them gives a total order that never contradicts
/// happened-before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LamportStamp<P> {
    pub time: u64,
    pub process: P,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::hybrid::{Hlc, ManualClock, Timestamp};
    use crate::test_util::Rng;
    use std::collections::HashSet;

    /// The trace from the vector clock tests, stamped with Lamport clocks:
    ///
    /// ```text
    /// A  --A1----A2------------------A3---->
    ///              \
    ///               \ m1
    ///                \
    /// B  --B1---------B2----B3------------->
    ///                         \
    ///                          \ m2
    ///                           \
    /// C  --C1--------------------C2-------->
    /// ```
    #[test]
    fn three_process_trace() {
        let (mut la, mut lb, mut lc) = (
            LamportClock::new('A'),
            LamportClock::new('B'),
            LamportClock::new('C'),
        );

        let a1 = la.tick();
        let m1 = la.send();
        let a2 = m1;
        let b1 = lb.tick();
        let b2 = lb.receive(m1.time);
        let c1 = lc.tick();
        let m2 = lb.send();
        let b3 = m2;
        let c2 = lc.receive(m2.time);
        let a3 = la.tick();

        assert_eq!([a1, a2, a3].map(|s| s.time), [1, 2, 3]);
        assert_eq!([b1, b2, b3].map(|s| s.time), [1, 3, 4]);
        assert_eq!([c1, c2].map(|s| s.time), [1, 5]);

        // Happened-before shows up as a smaller stamp, transitively too.
        assert!(a1 < b2 && b2 < c2 && a1 < c2);
        // But A3 and B3 are concurrent, and the clock orders them anyway.
        assert!(a3 < b3);
        // A1, B1 and C1 are concurrent and tie; the process breaks the tie.
        let mut firsts = [c1, a1, b1];
        firsts.sort();
        assert_eq!(firsts.map(|s| s.process), ['A', 'B', 'C']);
    }

    #[test]
    fn a_busy_sender_drags_the_receiver_forward() {
        let (mut busy, mut quiet) = (LamportClock::new('B'), LamportClock::new('Q'));
        for _ in 0..1_000 {
            busy.tick();
        }
        quiet.tick();
        assert_eq!(busy.send().time, 1_001);
        assert_eq!(quiet.receive(1_001).time, 1_002);
        // A message from the past changes nothing but the receive itself.
        assert_eq!(quiet.receive(1).time, 1_003);
    }

    /// On random executions with non-FIFO delivery: happened-before always
    /// implies a smaller stamp, stamps with the process attached are unique,
    /// and some concurrent pairs get ordered stamps anyway (the converse fails).
    #[test]
    fn clock_condition_holds_but_not_its_converse() {
        const PROCS: usize = 4;
        const EVENTS: usize = 80;
        let mut concurrent_but_ordered = 0;
        for seed in 1..=100u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut clocks: [_; PROCS] = std::array::from_fn(LamportClock::new);
            let mut last_event: Vec<Option<usize>> = vec![None; PROCS];
            // (destination, id of the send event, time)
            let mut in_flight: Vec<(usize, usize, u64)> = Vec::new();
            let mut stamps: Vec<LamportStamp<usize>> = Vec::new();
            let mut parents: Vec<Vec<usize>> = Vec::new();

            for _ in 0..EVENTS {
                let p = rng.below(PROCS);
                let id = stamps.len();
                let mut ps: Vec<usize> = last_event[p].into_iter().collect();
                let deliverable: Vec<usize> = (0..in_flight.len())
                    .filter(|&i| in_flight[i].0 == p)
                    .collect();
                let stamp = match rng.below(3) {
                    0 => {
                        let to = (p + 1 + rng.below(PROCS - 1)) % PROCS;
                        let stamp = clocks[p].send();
                        in_flight.push((to, id, stamp.time));
                        stamp
                    }
                    1 if !deliverable.is_empty() => {
                        let i = deliverable[rng.below(deliverable.len())];
                        let (_, send_id, time) = in_flight.swap_remove(i);
                        ps.push(send_id);
                        clocks[p].receive(time)
                    }
                    _ => clocks[p].tick(),
                };
                last_event[p] = Some(id);
                stamps.push(stamp);
                parents.push(ps);
            }

            let unique: HashSet<_> = stamps.iter().collect();
            assert_eq!(unique.len(), stamps.len(), "seed {seed}: duplicate stamps");

            // Events were recorded in a topological order, so one forward pass
            // computes each event's causal past.
            let mut ancestors: Vec<HashSet<usize>> = Vec::with_capacity(stamps.len());
            for ps in &parents {
                let mut s = HashSet::new();
                for &p in ps {
                    s.insert(p);
                    s.extend(ancestors[p].iter().copied());
                }
                ancestors.push(s);
            }
            for e in 0..stamps.len() {
                for f in 0..stamps.len() {
                    if ancestors[f].contains(&e) {
                        assert!(
                            stamps[e].time < stamps[f].time,
                            "seed {seed}: e{e} → e{f} but not L(e{e}) < L(e{f})"
                        );
                    } else if e != f
                        && !ancestors[e].contains(&f)
                        && stamps[e].time < stamps[f].time
                    {
                        concurrent_but_ordered += 1;
                    }
                }
            }
        }
        assert!(
            concurrent_but_ordered > 0,
            "expected concurrent events with ordered stamps"
        );
    }

    /// The module's closing claim: a hybrid logical clock whose physical clock
    /// never moves off 0 issues exactly the Lamport clock's stamps, in its
    /// logical component.
    #[test]
    fn an_hlc_on_a_stopped_physical_clock_is_a_lamport_clock() {
        const PROCS: usize = 4;
        let stopped = ManualClock::new(0);
        for seed in 1..=50u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut lamports: [_; PROCS] = std::array::from_fn(LamportClock::new);
            let hlcs: Vec<Hlc<&ManualClock>> = (0..PROCS).map(|_| Hlc::new(&stopped)).collect();
            // (destination, Lamport stamp, HLC stamp)
            let mut in_flight: Vec<(usize, u64, Timestamp)> = Vec::new();
            for _ in 0..200 {
                let p = rng.below(PROCS);
                let deliverable: Vec<usize> = (0..in_flight.len())
                    .filter(|&i| in_flight[i].0 == p)
                    .collect();
                let (lamport, hlc) = match rng.below(3) {
                    0 => {
                        let to = (p + 1 + rng.below(PROCS - 1)) % PROCS;
                        let stamps = (lamports[p].send().time, hlcs[p].now());
                        in_flight.push((to, stamps.0, stamps.1));
                        stamps
                    }
                    1 if !deliverable.is_empty() => {
                        let i = deliverable[rng.below(deliverable.len())];
                        let (_, l, h) = in_flight.swap_remove(i);
                        (lamports[p].receive(l).time, hlcs[p].update(h).unwrap())
                    }
                    _ => (lamports[p].tick().time, hlcs[p].now()),
                };
                assert_eq!(
                    (hlc.wall, u64::from(hlc.logical)),
                    (0, lamport),
                    "seed {seed}"
                );
            }
        }
    }
}
