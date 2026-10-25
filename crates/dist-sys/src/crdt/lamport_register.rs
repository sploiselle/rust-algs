//! # A last-writer-wins register on Lamport stamps
//!
//! The obvious way to replicate a cell: stamp every write with a
//! [`LamportStamp`], and wherever two writes meet, keep the one with the larger
//! stamp. The stamps are unique and every replica orders them the same way, so
//! replicas that have exchanged state agree on one value, whatever order they
//! synced in. It converges. It also loses writes.
//!
//! ## Two histories, one set of stamps
//!
//! Replicas A and B, one register `x`.
//!
//! **B saw A's write.** A writes `"milk"`, stamped `(1,A)`, and B syncs from A,
//! which takes B's clock to 2. B then writes `"eggs"`, stamped `(3,B)`.
//!
//! **B never heard from A.** A writes `"milk"`, stamped `(1,A)`. B, partitioned
//! off, does two unrelated things (its clock goes to 1, then 2), then writes
//! `"eggs"`, stamped `(3,B)`.
//!
//! Both histories produce `(1,A)` and `(3,B)`, so once the replicas sync,
//! `"eggs"` wins both times. In the first that's right: B was shown `"milk"`
//! and chose to replace it. In the second it is silent data loss: nobody who
//! decided to discard `"milk"` ever saw it, and once it's gone nothing on either
//! replica records that it existed. Nothing in the stamps tells the two
//! histories apart.
//!
//! Vector clocks do. In the first, `"milk"` is `{A:1}` and `"eggs"` is
//! `{A:1, B:2}`: the first is below the second, so `"eggs"` safely supersedes
//! it. In the second, `"eggs"` is `{B:3}`: neither vector is below the other,
//! so the writes were concurrent, and a register that wants to keep both can
//! (an [`MvRegister`](super::MvRegister) does).
//!
//! ## Why
//!
//! A [Lamport clock](crate::clock::lamport) promises only `e → f ⇒ L(e) < L(f)`.
//! The converse fails: `L(e) < L(f)` is just as consistent with `e ∥ f`. A
//! total order on stamps isn't wrong, it's lossy: it invents an order between
//! concurrent writes and then forgets that it invented it. That's exactly what
//! Lamport's own uses want ([mutual
//! exclusion](crate::coordination::lamport_mutex), replicated state machines),
//! where any agreed order will do. It's wrong whenever the application needs to
//! *react* to concurrency rather than break ties: a shopping cart where "add"
//! must never be lost, a file synced between partitioned replicas.
//!
//! The test `keeps_one_of_the_values_an_mv_register_keeps` makes that precise:
//! run the same writes and syncs through this register and an
//! [`MvRegister`](super::MvRegister), and this register always ends on one of
//! the MV register's siblings, silently dropping the rest.

use crate::clock::{LamportClock, LamportStamp};

/// A last-writer-wins register at one replica `A`, holding values `V`.
///
/// State:
/// - `clock`: this replica's Lamport clock, which every write, sync and
///   [`tick`](Self::tick) advances.
/// - `entry`: the value with the largest stamp this replica has heard of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LamportRegister<A, V> {
    replica: A,
    clock: LamportClock,
    entry: Option<(LamportStamp<A>, V)>,
}

impl<A: Ord + Clone, V: Clone> LamportRegister<A, V> {
    pub fn new(replica: A) -> Self {
        Self {
            replica,
            clock: LamportClock::new(),
            entry: None,
        }
    }

    pub fn replica(&self) -> &A {
        &self.replica
    }

    pub fn clock(&self) -> &LamportClock {
        &self.clock
    }

    /// The current value, if anything has been written.
    pub fn get(&self) -> Option<&V> {
        self.entry.as_ref().map(|(_, v)| v)
    }

    /// The current value and the stamp of the write that produced it.
    pub fn entry(&self) -> Option<(&LamportStamp<A>, &V)> {
        self.entry.as_ref().map(|(s, v)| (s, v))
    }

    /// Record an event at this replica that isn't a write to the register,
    /// such as a write to some other key. It moves the clock all the same.
    pub fn tick(&mut self) -> u64 {
        self.clock.tick()
    }

    /// Write `value`, replacing whatever this replica holds. The new stamp is
    /// larger than every stamp this replica has heard of, so the write wins
    /// here. Returns the stamp.
    pub fn write(&mut self, value: V) -> LamportStamp<A> {
        let stamp = LamportStamp {
            time: self.clock.tick(),
            process: self.replica.clone(),
        };
        self.entry = Some((stamp.clone(), value));
        stamp
    }

    /// Receive another replica's state: a receive event for the clock, then
    /// keep whichever value has the larger stamp. Replicas that have heard of
    /// the same writes hold the same value, in whatever order they synced.
    pub fn sync_from(&mut self, other: &Self) {
        self.clock.receive(other.clock.time());
        if let Some((theirs, v)) = &other.entry
            && self.entry.as_ref().is_none_or(|(ours, _)| theirs > ours)
        {
            self.entry = Some((theirs.clone(), v.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Causality, VectorClock};
    use crate::crdt::MvRegister;
    use crate::test_util::Rng;
    use std::collections::HashSet;

    /// One history: B's register, both writes' stamps and vector clocks, and
    /// A's register after it syncs from B.
    struct History {
        a: LamportRegister<char, &'static str>,
        b: LamportRegister<char, &'static str>,
        milk: (LamportStamp<char>, VectorClock<char>),
        eggs: (LamportStamp<char>, VectorClock<char>),
    }

    /// The module's two histories. Vector clocks run beside the registers,
    /// which never look at them.
    fn history(b_saw_milk: bool) -> History {
        let (mut a, mut b) = (LamportRegister::new('A'), LamportRegister::new('B'));
        let (mut va, mut vb) = (VectorClock::new(), VectorClock::new());

        let milk = a.write("milk");
        va.tick(&'A');
        let milk = (milk, va.clone());
        if b_saw_milk {
            b.sync_from(&a);
            vb.receive(&'B', &va);
        } else {
            for _ in 0..2 {
                b.tick();
                vb.tick(&'B');
            }
        }
        assert_eq!(b.clock().time(), 2);
        let eggs = b.write("eggs");
        vb.tick(&'B');
        let eggs = (eggs, vb.clone());
        a.sync_from(&b);
        History { a, b, milk, eggs }
    }

    #[test]
    fn two_histories_get_the_same_stamps() {
        let (saw, never) = (history(true), history(false));
        let stamp = |time, process| LamportStamp { time, process };
        for h in [&saw, &never] {
            assert_eq!((&h.milk.0, &h.eggs.0), (&stamp(1, 'A'), &stamp(3, 'B')));
            assert_eq!(h.a.get(), Some(&"eggs"));
        }
        // Every replica ends identical, clock and all.
        assert_eq!((&saw.a, &saw.b), (&never.a, &never.b));

        // The vector clocks tell them apart.
        assert_eq!(saw.milk.1, VectorClock::from_iter([('A', 1)]));
        assert_eq!(saw.eggs.1, VectorClock::from_iter([('A', 1), ('B', 2)]));
        assert_eq!(saw.milk.1.compare(&saw.eggs.1), Causality::Before);
        assert_eq!(never.eggs.1, VectorClock::from_iter([('B', 3)]));
        assert_eq!(never.milk.1.compare(&never.eggs.1), Causality::Concurrent);
    }

    #[test]
    fn replicas_converge_whatever_the_sync_order() {
        let mut regs: Vec<LamportRegister<char, u32>> =
            ['A', 'B', 'C'].map(LamportRegister::new).to_vec();
        regs[0].write(1);
        regs[1].tick();
        regs[1].write(2);
        regs[2].write(3);
        let sync_all = |mut regs: Vec<LamportRegister<char, u32>>, order: [usize; 3]| {
            for i in order {
                for j in order {
                    let src = regs[j].clone();
                    regs[i].sync_from(&src);
                }
            }
            regs
        };
        // (2,B) is the largest stamp, though C wrote last.
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            for r in sync_all(regs.clone(), order) {
                assert_eq!(
                    r.entry(),
                    Some((
                        &LamportStamp {
                            time: 2,
                            process: 'B'
                        },
                        &2
                    ))
                );
            }
        }
    }

    /// The same random writes and syncs, run through this register and an
    /// MvRegister, where each write replaces exactly what its replica holds.
    /// Once everyone has synced, this register's value is always one of the MV
    /// register's siblings, which are pairwise concurrent writes: it never
    /// keeps a value that a later write knowingly replaced, and it drops every
    /// other sibling without a trace. Some histories do produce siblings, so
    /// the loss is real.
    #[test]
    fn keeps_one_of_the_values_an_mv_register_keeps() {
        const REPLICAS: usize = 3;
        let mut lost = 0;
        for seed in 1..=300u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut lww: Vec<LamportRegister<usize, u32>> =
                (0..REPLICAS).map(LamportRegister::new).collect();
            let mut mv: Vec<MvRegister<usize, u32>> = vec![MvRegister::new(); REPLICAS];

            for step in 0..40u32 {
                let r = rng.below(REPLICAS);
                match rng.below(3) {
                    0 => {
                        lww[r].write(step);
                        mv[r].update(&r, |_| step);
                    }
                    1 => {
                        lww[r].tick();
                    }
                    _ => {
                        let j = rng.below(REPLICAS);
                        let (l, m) = (lww[j].clone(), mv[j].clone());
                        lww[r].sync_from(&l);
                        mv[r].sync_from(&m);
                    }
                }
            }
            for _ in 0..2 {
                for i in 0..REPLICAS {
                    for j in 0..REPLICAS {
                        let (l, m) = (lww[j].clone(), mv[j].clone());
                        lww[i].sync_from(&l);
                        mv[i].sync_from(&m);
                    }
                }
            }

            let winner = lww[0].get().copied();
            let siblings: HashSet<u32> = mv[0].entries().map(|(_, v)| *v).collect();
            for r in &lww {
                assert_eq!(r.get().copied(), winner, "seed {seed}: replicas disagree");
            }
            match winner {
                Some(w) => assert!(
                    siblings.contains(&w),
                    "seed {seed}: kept a value that was replaced"
                ),
                None => assert!(siblings.is_empty(), "seed {seed}"),
            }
            lost += siblings.len().saturating_sub(1);
        }
        assert!(lost > 0, "expected some concurrent writes to be dropped");
    }
}
