//! # Vector clocks — a reference implementation
//!
//! A vector clock stamps every event with a map `actor -> counter`. Entry `i` of
//! event `e`'s clock is the number of events at actor `i` that lie in `e`'s
//! causal past (counting `e` itself if it happened at `i`).
//!
//! Why that's enough: each actor's own history is a totally ordered sequence, so
//! any causally closed set of events is just "some prefix of each actor's
//! history". The vector records the length of each prefix. Comparing two clocks
//! pointwise is therefore comparing two causal histories by set inclusion, which
//! gives an *iff*:
//!
//! ```text
//!     e → f  (e happened-before f)   ⇔   VC(e) < VC(f)
//!     e ∥ f  (concurrent)            ⇔   neither VC(e) ≤ VC(f) nor VC(f) ≤ VC(e)
//! ```
//!
//! A single scalar counter (a Lamport clock `L`) can't do this. It only promises
//! `e → f ⇒ L(e) < L(f)`, and `L(e) < L(f)` is just as consistent with `e` and
//! `f` being concurrent.
//!
//! Three rules drive everything:
//! 1. **Local event** at actor `i`: `V[i] += 1`.
//! 2. **Send** at `i`: apply rule 1, then attach a copy of `V` to the message.
//! 3. **Receive** at `i` of a message stamped `W`: `V = max(V, W)` pointwise, then rule 1.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

/// The causal relationship between two clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Causality {
    /// `self` happened-before `other` (`self < other`).
    Before,
    /// `other` happened-before `self` (`self > other`).
    After,
    /// Identical causal histories (for event stamps: the same event).
    Equal,
    /// Neither has seen the other.
    Concurrent,
}

/// A vector clock over actors of type `A`.
///
/// Invariant: no stored entry is ever 0 — an absent actor *means* 0. That keeps
/// the derived `PartialEq`/`Hash` consistent with the causal comparison
/// (otherwise `{a: 0}` and `{}` would be causally equal but structurally unequal).
///
/// Deliberately implements `PartialOrd` but **not** `Ord`: concurrent clocks are
/// incomparable, and pretending otherwise (e.g. `sort_by(|a, b| a.partial_cmp(b).unwrap())`)
/// either panics or quietly invents an order that doesn't exist.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VectorClock<A: Ord> {
    entries: BTreeMap<A, u64>,
}

impl<A: Ord> Default for VectorClock<A> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<A: Ord + Clone> VectorClock<A> {
    pub fn new() -> Self {
        Self::default()
    }

    /// This clock's counter for `actor` (0 if never seen).
    pub fn get(&self, actor: &A) -> u64 {
        self.entries.get(actor).copied().unwrap_or(0)
    }

    /// The entries, in actor order. An absent actor is 0 and is never yielded
    /// (the invariant: no stored zeros).
    pub fn iter(&self) -> impl Iterator<Item = (&A, u64)> {
        self.entries.iter().map(|(actor, &n)| (actor, n))
    }

    /// Rule 1: record a new event at `actor`. Returns the new counter; together
    /// with `actor` it uniquely names the event (see [`Dot`](crate::clock::Dot)).
    pub fn tick(&mut self, actor: &A) -> u64 {
        match self.entries.get_mut(actor) {
            Some(c) => {
                *c += 1;
                *c
            }
            None => {
                self.entries.insert(actor.clone(), 1);
                1
            }
        }
    }

    /// Rule 2: record a send event and return the stamp to attach to the message.
    pub fn send(&mut self, actor: &A) -> Self {
        self.tick(actor);
        self.clone()
    }

    /// Rule 3: absorb an incoming message's stamp, then record the receive event.
    pub fn receive(&mut self, actor: &A, stamp: &Self) {
        self.merge(stamp);
        self.tick(actor);
    }

    /// Pointwise max: the least upper bound (join) in the product lattice ℕ^actors.
    ///
    /// Commutative, associative and idempotent, so state can be merged in any
    /// order, any number of times, over any gossip topology.
    pub fn merge(&mut self, other: &Self) {
        for (actor, &theirs) in &other.entries {
            match self.entries.get_mut(actor) {
                Some(ours) => *ours = (*ours).max(theirs),
                None => {
                    self.entries.insert(actor.clone(), theirs);
                }
            }
        }
    }

    /// Non-mutating [`merge`](Self::merge).
    pub fn join(&self, other: &Self) -> Self {
        let mut out = self.clone();
        out.merge(other);
        out
    }

    /// Compare causal histories pointwise.
    pub fn compare(&self, other: &Self) -> Causality {
        let mut self_le = true; // ∀i: self[i] <= other[i]
        let mut other_le = true; // ∀i: other[i] <= self[i]

        for (actor, &x) in &self.entries {
            let y = other.get(actor);
            if x > y {
                self_le = false;
            }
            if x < y {
                other_le = false;
            }
            if !self_le && !other_le {
                return Causality::Concurrent;
            }
        }
        // Actors only `other` has seen: there self[i] = 0 < other[i] (no stored zeros).
        if other_le && other.entries.keys().any(|a| !self.entries.contains_key(a)) {
            other_le = false;
        }

        match (self_le, other_le) {
            (true, true) => Causality::Equal,
            (true, false) => Causality::Before,
            (false, true) => Causality::After,
            (false, false) => Causality::Concurrent,
        }
    }

    /// `self ≥ other`: everything `other` has seen, `self` has seen too.
    pub fn descends(&self, other: &Self) -> bool {
        matches!(self.compare(other), Causality::After | Causality::Equal)
    }

    pub fn concurrent_with(&self, other: &Self) -> bool {
        self.compare(other) == Causality::Concurrent
    }
}

impl<A: Ord + Clone> PartialOrd for VectorClock<A> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self.compare(other) {
            Causality::Before => Some(Ordering::Less),
            Causality::After => Some(Ordering::Greater),
            Causality::Equal => Some(Ordering::Equal),
            Causality::Concurrent => None,
        }
    }
}

/// Build a clock from `(actor, counter)` pairs; zero counters are dropped to keep the invariant.
impl<A: Ord> FromIterator<(A, u64)> for VectorClock<A> {
    fn from_iter<I: IntoIterator<Item = (A, u64)>>(iter: I) -> Self {
        let mut entries = BTreeMap::new();
        for (actor, n) in iter {
            if n > 0 {
                let e = entries.entry(actor).or_insert(0);
                *e = n.max(*e);
            }
        }
        Self { entries }
    }
}

impl<A: Ord + fmt::Debug> fmt::Debug for VectorClock<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.entries.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Rng;
    use std::collections::HashSet;

    fn vc<const N: usize>(pairs: [(char, u64); N]) -> VectorClock<char> {
        pairs.into_iter().collect()
    }

    fn random_clock(rng: &mut Rng) -> VectorClock<u8> {
        (0..4u8).map(|a| (a, rng.below(4) as u64)).collect()
    }

    /// Three processes, time flowing left to right, messages drawn as diagonals:
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
        let (a, b, c) = ('A', 'B', 'C');
        let (mut va, mut vb, mut vc_) =
            (VectorClock::new(), VectorClock::new(), VectorClock::new());

        va.tick(&a);
        let a1 = va.clone(); // A1: local
        let m1 = va.send(&a);
        let a2 = va.clone(); // A2: send m1 → B
        vb.tick(&b);
        let b1 = vb.clone(); // B1: local
        vb.receive(&b, &m1);
        let b2 = vb.clone(); // B2: receive m1
        vc_.tick(&c);
        let c1 = vc_.clone(); // C1: local
        let m2 = vb.send(&b);
        let b3 = vb.clone(); // B3: send m2 → C
        vc_.receive(&c, &m2);
        let c2 = vc_.clone(); // C2: receive m2
        va.tick(&a);
        let a3 = va.clone(); // A3: local

        assert_eq!(a1, vc([(a, 1)]));
        assert_eq!(a2, vc([(a, 2)]));
        assert_eq!(b1, vc([(b, 1)]));
        assert_eq!(b2, vc([(a, 2), (b, 2)]));
        assert_eq!(b3, vc([(a, 2), (b, 3)]));
        assert_eq!(c2, vc([(a, 2), (b, 3), (c, 2)]));
        assert_eq!(a3, vc([(a, 3)]));

        assert!(a1 < c2); // transitively, via B
        assert!(b1 < b2 && b2 < b3);
        assert_eq!(a3.compare(&b3), Causality::Concurrent);
        assert_eq!(c1.compare(&a2), Causality::Concurrent);
        assert_eq!(c1.compare(&c2), Causality::Before);
        assert!(a3.partial_cmp(&c2).is_none());
    }

    #[test]
    fn absent_means_zero() {
        let empty: VectorClock<char> = VectorClock::new();
        assert_eq!(vc([('a', 0)]), empty);
        assert!(empty < vc([('a', 1)]));
        assert_eq!(
            vc([('a', 1)]).compare(&vc([('b', 1)])),
            Causality::Concurrent
        );
        assert_eq!(
            vc([('a', 2), ('b', 1)]).compare(&vc([('a', 2)])),
            Causality::After
        );
    }

    #[test]
    fn iter_yields_entries_in_actor_order_and_skips_zeros() {
        let clock = vc([('b', 2), ('a', 1), ('c', 0)]);
        let entries: Vec<_> = clock.iter().map(|(a, n)| (*a, n)).collect();
        assert_eq!(entries, [('a', 1), ('b', 2)]);
    }

    #[test]
    fn merge_is_a_join_semilattice() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        for _ in 0..2_000 {
            let (x, y, z) = (
                random_clock(&mut rng),
                random_clock(&mut rng),
                random_clock(&mut rng),
            );
            assert_eq!(x.join(&y), y.join(&x), "commutative");
            assert_eq!(x.join(&y).join(&z), x.join(&y.join(&z)), "associative");
            assert_eq!(x.join(&x), x, "idempotent");
            let j = x.join(&y);
            assert!(j.descends(&x) && j.descends(&y), "upper bound");
            if z.descends(&x) && z.descends(&y) {
                assert!(z.descends(&j), "least upper bound");
            }
        }
    }

    /// The module's *iff*, checked empirically: on random executions with
    /// non-FIFO message delivery, clock comparison agrees *exactly* with
    /// reachability in the happens-before graph.
    #[test]
    fn clock_order_is_exactly_happens_before() {
        const PROCS: usize = 4;
        const EVENTS: usize = 80;
        for seed in 1..=100u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut clocks: Vec<VectorClock<usize>> = vec![VectorClock::new(); PROCS];
            let mut last_event: Vec<Option<usize>> = vec![None; PROCS];
            // (destination, id of the send event, stamp)
            let mut in_flight: Vec<(usize, usize, VectorClock<usize>)> = Vec::new();
            let mut stamps: Vec<VectorClock<usize>> = Vec::new();
            let mut parents: Vec<Vec<usize>> = Vec::new(); // direct happens-before edges

            for _ in 0..EVENTS {
                let p = rng.below(PROCS);
                let id = stamps.len();
                let mut ps: Vec<usize> = last_event[p].into_iter().collect();
                let deliverable: Vec<usize> = (0..in_flight.len())
                    .filter(|&i| in_flight[i].0 == p)
                    .collect();
                match rng.below(3) {
                    0 => {
                        let to = (p + 1 + rng.below(PROCS - 1)) % PROCS;
                        let stamp = clocks[p].send(&p);
                        in_flight.push((to, id, stamp));
                    }
                    1 if !deliverable.is_empty() => {
                        // Arbitrary delivery order: no FIFO assumption.
                        let i = deliverable[rng.below(deliverable.len())];
                        let (_, send_id, stamp) = in_flight.swap_remove(i);
                        clocks[p].receive(&p, &stamp);
                        ps.push(send_id);
                    }
                    _ => {
                        clocks[p].tick(&p);
                    }
                }
                last_event[p] = Some(id);
                stamps.push(clocks[p].clone());
                parents.push(ps);
            }

            // Ground truth via transitive closure. Events were recorded in a
            // topological order, so one forward pass suffices.
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
                    let expected = if e == f {
                        Causality::Equal
                    } else if ancestors[f].contains(&e) {
                        Causality::Before
                    } else if ancestors[e].contains(&f) {
                        Causality::After
                    } else {
                        Causality::Concurrent
                    };
                    assert_eq!(
                        stamps[e].compare(&stamps[f]),
                        expected,
                        "seed {seed}: e{e} vs e{f}"
                    );
                }
            }
        }
    }
}
