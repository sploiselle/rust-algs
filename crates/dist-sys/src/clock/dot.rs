//! # Dots and causal contexts
//!
//! A vector clock answers "is event `e` in event `f`'s causal past?".
//! Replicated data often needs a narrower question — "has this specific write
//! been overwritten?" — so it names each write by a [`Dot`] and tracks which
//! dots have been seen in a [`CausalContext`]: a vector clock plus the
//! exceptions a plain vector can't express.
//!
//! [`MvRegister`](crate::crdt::MvRegister) shows both in use.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::VectorClock;

/// The name of one event: the `counter`-th event at `actor`.
///
/// No `Ord`: dots from different actors aren't causally ordered, and a dot alone
/// says nothing about what its writer had seen.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Dot<A> {
    pub actor: A,
    pub counter: u64,
}

impl<A: fmt::Debug> fmt::Debug for Dot<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}:{}", self.actor, self.counter)
    }
}

/// A set of dots: "which writes do I know about?"
///
/// Stored as a vector clock (meaning every dot `(a, 1..=vv[a])`) plus a cloud of
/// dots known beyond that contiguous prefix.
///
/// Why not just a vector clock? A vector can only say "the first n events of A".
/// Knowing (A,1) and (A,3) but not (A,2), the smallest vector containing (A,3) is
/// {A:3}, which also claims (A,2). Treating a write you never saw as seen is
/// exactly how concurrent writes get silently discarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CausalContext<A: Ord> {
    vv: VectorClock<A>,
    cloud: BTreeMap<A, BTreeSet<u64>>,
}

impl<A: Ord> Default for CausalContext<A> {
    fn default() -> Self {
        Self {
            vv: VectorClock::default(),
            cloud: BTreeMap::new(),
        }
    }
}

impl<A: Ord + Clone> CausalContext<A> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, dot: &Dot<A>) -> bool {
        dot.counter <= self.vv.get(&dot.actor)
            || self
                .cloud
                .get(&dot.actor)
                .is_some_and(|s| s.contains(&dot.counter))
    }

    pub fn insert(&mut self, dot: Dot<A>) {
        if self.contains(&dot) {
            return;
        }
        let actor = dot.actor.clone();
        self.cloud.entry(dot.actor).or_default().insert(dot.counter);
        self.compact(&actor);
    }

    /// Set union.
    pub fn merge(&mut self, other: &Self) {
        self.vv.merge(&other.vv);
        for (actor, extra) in &other.cloud {
            self.cloud
                .entry(actor.clone())
                .or_default()
                .extend(extra.iter().copied());
        }
        let actors: Vec<A> = self.cloud.keys().cloned().collect();
        for actor in &actors {
            self.compact(actor);
        }
    }

    /// The contiguous part: every dot up to `vector().get(a)` for each actor.
    pub fn vector(&self) -> &VectorClock<A> {
        &self.vv
    }

    /// The dots known beyond the contiguous prefix, each one past a gap a plain
    /// vector can't express. Empty exactly when this context is just a vector
    /// clock.
    pub fn cloud(&self) -> impl Iterator<Item = Dot<A>> {
        self.cloud.iter().flat_map(|(actor, counters)| {
            counters.iter().map(move |&counter| Dot {
                actor: actor.clone(),
                counter,
            })
        })
    }

    /// One past the highest dot known for `actor`.
    pub fn next_counter(&self, actor: &A) -> u64 {
        let highest_extra = self
            .cloud
            .get(actor)
            .and_then(|s| s.last().copied())
            .unwrap_or(0);
        self.vv.get(actor).max(highest_extra) + 1
    }

    /// Fold cloud dots that extend the contiguous prefix into the vector, so the
    /// representation stays canonical (equal sets ⇒ equal structs).
    fn compact(&mut self, actor: &A) {
        let Some(extra) = self.cloud.get_mut(actor) else {
            return;
        };
        let top = self.vv.get(actor);
        extra.retain(|&c| c > top);
        while extra.remove(&(self.vv.get(actor) + 1)) {
            self.vv.tick(actor);
        }
        if extra.is_empty() {
            self.cloud.remove(actor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(actor: char, counter: u64) -> Dot<char> {
        Dot { actor, counter }
    }

    #[test]
    fn causal_context_tracks_gaps_a_vector_cannot() {
        let mut cc = CausalContext::new();
        cc.insert(dot('A', 1));
        cc.insert(dot('A', 3));
        assert!(cc.contains(&dot('A', 1)));
        assert!(!cc.contains(&dot('A', 2))); // the vector {A:3} would wrongly claim this
        assert!(cc.contains(&dot('A', 3)));
        assert_eq!(cc.vector(), &VectorClock::from_iter([('A', 1)]));
        assert_eq!(cc.cloud().collect::<Vec<_>>(), [dot('A', 3)]);

        cc.insert(dot('A', 2)); // gap filled: everything folds into the vector
        assert_eq!(cc.vector(), &VectorClock::from_iter([('A', 3)]));
        assert_eq!(cc.cloud().count(), 0);
        assert_eq!(cc.next_counter(&'A'), 4);

        let mut other = CausalContext::new();
        other.insert(dot('B', 2));
        cc.merge(&other); // merging brings B's gap along
        assert_eq!(cc.cloud().collect::<Vec<_>>(), [dot('B', 2)]);
        assert_eq!(cc.vector(), &VectorClock::from_iter([('A', 3)]));
    }
}
