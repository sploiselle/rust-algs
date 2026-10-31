//! The hash join: a table on one input's keys, probed with the other's.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// Joins `build` and `probe` on their keys: builds a table from `build`, then
/// looks up each row of `probe` in it, calling `emit` with every pair whose
/// keys are equal, in probe order. A probe row's matches come in reverse
/// `build` order.
///
/// The table is two allocations for any non-empty `build`, however its keys
/// repeat, and its memory grows with `build` alone, so build the smaller
/// input. `hasher` is the caller's: std's `RandomState` resists keys chosen to
/// collide; engines use faster ones.
pub fn hash_join<K: Hash + Eq, A, B>(
    build: &[(K, A)],
    probe: &[(K, B)],
    hasher: impl BuildHasher,
    mut emit: impl FnMut(&(K, A), &(K, B)),
) {
    // Each key's last row in `build`, and for each row, the row before it with
    // the same key: `insert` returns the index it replaces.
    let mut last = HashMap::with_capacity_and_hasher(build.len(), hasher);
    let mut previous = Vec::with_capacity(build.len());
    for (i, (key, _)) in build.iter().enumerate() {
        previous.push(last.insert(key, i));
    }
    // Follow each probe row's key back through every build row that has it.
    for row in probe {
        let mut at = last.get(&row.0).copied();
        while let Some(i) = at {
            emit(&build[i], row);
            at = previous[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::testing::{Row, loop_pairs, pairs, two};
    use proptest::prelude::*;
    use std::hash::{BuildHasherDefault, Hasher, RandomState};

    /// A hasher that sends every key to the same bucket.
    #[derive(Default)]
    struct Collide;

    impl Hasher for Collide {
        fn finish(&self) -> u64 {
            0
        }

        fn write(&mut self, _: &[u8]) {}
    }

    proptest! {
        #![proptest_config(crate::test_util::config())]

        #[test]
        fn agrees_with_a_nested_loop_join((build, probe) in two()) {
            let joined = pairs(|emit| hash_join(&build, &probe, RandomState::new(), emit));
            prop_assert_eq!(joined, loop_pairs(&build, &probe));
        }

        /// Which side it builds changes the order of the pairs and the
        /// memory it takes, not which pairs it finds.
        #[test]
        fn agrees_whichever_side_it_builds((left, right) in two()) {
            let built_left = pairs(|emit| hash_join(&left, &right, RandomState::new(), emit));
            let built_right = pairs(|emit| {
                hash_join(&right, &left, RandomState::new(), |r, l| emit(l, r))
            });
            prop_assert_eq!(built_left, built_right);
        }

        /// A bad hash costs time, never matches: the table still compares
        /// keys for equality.
        #[test]
        fn agrees_when_every_key_hashes_alike((build, probe) in two()) {
            let collide = BuildHasherDefault::<Collide>::default();
            let joined = pairs(|emit| hash_join(&build, &probe, collide, emit));
            prop_assert_eq!(joined, loop_pairs(&build, &probe));
        }

        #[test]
        fn emits_in_probe_order((build, probe) in two()) {
            let mut probed: Vec<Row> = Vec::new();
            hash_join(&build, &probe, RandomState::new(), |_, p| probed.push(*p));
            let positions: Vec<usize> = probed
                .iter()
                .map(|p| probe.iter().position(|q| q == p).unwrap())
                .collect();
            prop_assert!(positions.is_sorted());
        }
    }

    #[test]
    fn emits_a_keys_build_rows_in_reverse_order() {
        let build = [(1, 'a'), (1, 'b'), (2, 'z'), (1, 'c')];
        let mut matched = Vec::new();
        hash_join(&build, &[(1, 'x')], RandomState::new(), |b, _| {
            matched.push(b.1)
        });
        assert_eq!(matched, ['c', 'b', 'a']);
    }
}
