//! # Multi-value register
//!
//! A replicated cell that keeps concurrent writes side by side ("siblings")
//! instead of picking a winner, until a writer who was shown all of them
//! replaces them with one value. Each write is named by a [`Dot`], and each
//! replica tracks the writes it has heard of in a [`CausalContext`].

use crate::clock::{CausalContext, Dot};

/// Proof of what a reader was shown: exactly the dots of the values [`MvRegister::read`]
/// returned. There is no public constructor; the only way to get one is to read.
#[derive(Clone, Debug)]
pub struct ReadContext<A: Ord>(CausalContext<A>);

impl<A: Ord + Clone> ReadContext<A> {
    pub fn contains(&self, dot: &Dot<A>) -> bool {
        self.0.contains(dot)
    }
}

/// A multi-value register replicated across actors `A`, holding values `V`.
///
/// State:
/// - `entries`: the live values, each named by the [`Dot`] of the write that produced it.
/// - `seen`: every write this replica has heard of, live or overwritten.
///
/// The contract, checked by the test `mv_register_matches_its_spec`: once
/// replicas have exchanged state, the live values are exactly the writes whose
/// dot is in no write's [`ReadContext`]. A value disappears only because some
/// writer was handed it by a read and wrote with that read's context — never
/// because of sync order, timing or a clock comparison.
#[derive(Clone, Debug)]
pub struct MvRegister<A: Ord, V> {
    entries: Vec<(Dot<A>, V)>,
    seen: CausalContext<A>,
}

impl<A: Ord, V> Default for MvRegister<A, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            seen: CausalContext::default(),
        }
    }
}

impl<A: Ord + Clone, V: Clone> MvRegister<A, V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The live values with their dots: one if there's no conflict, several if there is.
    pub fn entries(&self) -> impl Iterator<Item = (&Dot<A>, &V)> {
        self.entries.iter().map(|(d, v)| (d, v))
    }

    /// Every write this replica knows about, including overwritten ones.
    pub fn seen(&self) -> &CausalContext<A> {
        &self.seen
    }

    /// The live values, plus a [`ReadContext`] naming exactly those values.
    pub fn read(&self) -> (Vec<(Dot<A>, V)>, ReadContext<A>) {
        let mut ctx = CausalContext::new();
        for (d, _) in &self.entries {
            ctx.insert(d.clone());
        }
        (self.entries.clone(), ReadContext(ctx))
    }

    /// Write `value` at `replica`, replacing exactly the values named in `ctx`.
    ///
    /// Anything not in `ctx` stays as a sibling of the new value, including
    /// values that arrived after the read. `ctx` may come from a read at another
    /// replica: its dots are recorded as seen, so if one of those values arrives
    /// here later it's recognised as already overwritten. Returns the new dot.
    pub fn write(&mut self, replica: &A, ctx: &ReadContext<A>, value: V) -> Dot<A> {
        self.entries.retain(|(d, _)| !ctx.contains(d));
        self.seen.merge(&ctx.0);
        let dot = Dot {
            actor: replica.clone(),
            counter: self.seen.next_counter(replica),
        };
        self.seen.insert(dot.clone());
        self.entries.push((dot.clone(), value));
        dot
    }

    /// Read and write in one step: `f` is handed exactly the values the new one replaces.
    pub fn update(&mut self, replica: &A, f: impl FnOnce(&[(Dot<A>, V)]) -> V) -> Dot<A> {
        let (shown, ctx) = self.read();
        let value = f(&shown);
        self.write(replica, &ctx, value)
    }

    /// Fold in another replica's state. Commutative, associative, idempotent.
    ///
    /// A dot that one side has seen but doesn't hold was overwritten there, so it
    /// must not survive on the other side; a dot one side has never seen is new.
    pub fn sync_from(&mut self, other: &Self) {
        self.entries.retain(|(d, _)| {
            !other.seen.contains(d) || other.entries.iter().any(|(od, _)| od == d)
        });
        for (d, v) in &other.entries {
            if !self.seen.contains(d) {
                self.entries.push((d.clone(), v.clone()));
            }
        }
        self.seen.merge(&other.seen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Rng;
    use std::collections::HashSet;

    fn sorted_values(r: &MvRegister<char, &'static str>) -> Vec<&'static str> {
        let mut v: Vec<_> = r.entries().map(|(_, v)| *v).collect();
        v.sort();
        v
    }

    #[test]
    fn concurrent_writes_become_siblings_until_a_write_that_was_shown_both() {
        let (mut r1, mut r2) = (MvRegister::new(), MvRegister::new());
        r1.update(&'A', |_| "draft");
        r2.sync_from(&r1);
        let stale = r2.clone();

        r1.update(&'A', |_| "edit from A"); // each writer is shown only "draft"
        r2.update(&'B', |_| "edit from B");
        r1.sync_from(&r2);
        r2.sync_from(&r1);
        assert_eq!(sorted_values(&r1), ["edit from A", "edit from B"]);
        assert_eq!(sorted_values(&r2), ["edit from A", "edit from B"]);

        let merged = r2.update(&'B', |shown| {
            assert_eq!(shown.len(), 2); // the writer is handed both siblings
            "merged"
        });
        r1.sync_from(&r2);
        r1.sync_from(&stale); // a stale peer can't resurrect "draft"
        r1.sync_from(&r2); // idempotent
        let live: Vec<_> = r1.entries().map(|(d, v)| (d.clone(), *v)).collect();
        assert_eq!(live, [(merged, "merged")]);
    }

    /// A writer working from an old read must not wipe out a value it was never shown.
    #[test]
    fn stale_read_keeps_the_value_it_never_saw() {
        let mut a = MvRegister::new();
        a.update(&'A', |_| "v1");
        let (shown, ctx) = a.read(); // a client reads v1 ...
        assert_eq!(shown.len(), 1);
        a.update(&'A', |_| "v2"); // ... meanwhile v2 replaces v1 ...
        a.write(&'A', &ctx, "v3"); // ... then the client writes from its old read
        assert_eq!(sorted_values(&a), ["v2", "v3"]); // v1 replaced; v2 survives beside v3
    }

    /// A read at one replica can be written back through another that hasn't
    /// received the value yet; when it arrives, it's recognised as overwritten.
    #[test]
    fn write_can_supersede_a_value_its_replica_has_not_received() {
        let (mut r1, mut r2) = (MvRegister::new(), MvRegister::new());
        r1.update(&'A', |_| "x");
        let (_, ctx) = r1.read();
        r2.write(&'B', &ctx, "y"); // r2 has never held "x"
        r2.sync_from(&r1);
        r1.sync_from(&r2);
        assert_eq!(sorted_values(&r1), ["y"]);
        assert_eq!(sorted_values(&r2), ["y"]);
    }

    /// The register's whole contract, on random histories with stale reads,
    /// reused reads, reads carried between replicas and arbitrary sync order:
    /// once everyone has exchanged state, the live values are exactly the writes
    /// whose dot appears in no write's ReadContext.
    #[test]
    fn mv_register_matches_its_spec() {
        const REPLICAS: usize = 3;
        for seed in 1..=300u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut regs: Vec<MvRegister<usize, u32>> = vec![MvRegister::new(); REPLICAS];
            let mut reads: Vec<ReadContext<usize>> = Vec::new();
            let mut writes: Vec<(Dot<usize>, ReadContext<usize>)> = Vec::new();

            for step in 0..60u32 {
                match rng.below(3) {
                    0 => {
                        let r = rng.below(REPLICAS);
                        reads.push(regs[r].read().1);
                    }
                    1 if !reads.is_empty() => {
                        let ctx = reads[rng.below(reads.len())].clone();
                        let w = rng.below(REPLICAS); // not necessarily where the read happened
                        let d = regs[w].write(&w, &ctx, step);
                        writes.push((d, ctx));
                    }
                    _ => {
                        let (i, j) = (rng.below(REPLICAS), rng.below(REPLICAS));
                        let src = regs[j].clone();
                        regs[i].sync_from(&src);
                    }
                }
            }
            for _ in 0..2 {
                for i in 0..REPLICAS {
                    for j in 0..REPLICAS {
                        let src = regs[j].clone();
                        regs[i].sync_from(&src);
                    }
                }
            }

            let expected: HashSet<Dot<usize>> = writes
                .iter()
                .map(|(d, _)| d.clone())
                .filter(|d| !writes.iter().any(|(_, ctx)| ctx.contains(d)))
                .collect();
            for r in &regs {
                let live: HashSet<Dot<usize>> = r.entries().map(|(d, _)| d.clone()).collect();
                assert_eq!(live, expected, "seed {seed}");
                assert_eq!(r.seen(), regs[0].seen(), "seed {seed}");
            }
        }
    }

    /// The classic operational footgun: a replica that loses its state and comes
    /// back under the same actor id reissues dots it already used. Two different
    /// writes now share one name, each side believes it already has the other's
    /// value, and the replicas never converge.
    #[test]
    fn reusing_an_actor_id_after_state_loss_corrupts_replicas() {
        let (mut a, mut b) = (MvRegister::new(), MvRegister::new());
        a.update(&'A', |_| "x"); // dot A:1
        b.sync_from(&a);

        let mut a_reborn = MvRegister::new(); // disk wiped, same id
        a_reborn.update(&'A', |_| "y"); // also dot A:1
        b.sync_from(&a_reborn);
        a_reborn.sync_from(&b);

        assert_eq!(sorted_values(&b), ["x"]);
        assert_eq!(sorted_values(&a_reborn), ["y"]);
    }
}
