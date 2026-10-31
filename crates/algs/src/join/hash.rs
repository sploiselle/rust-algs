//! The hash join: a table on one input's keys, probed with the other's.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// Joins `build` and `probe` on their keys: puts each row of `build` in a
/// table under its key, then looks up each row of `probe` in it, calling
/// `emit` with every pair whose keys are equal, in probe order.
///
/// The table is one allocation for any non-empty `build`, sized up front, and
/// its memory grows with `build` alone, so build the smaller input. `hasher`
/// is the caller's: std's `RandomState` resists keys chosen to collide;
/// engines use faster ones.
///
/// A key `build` repeats keeps only its last row, which loses matches; debug
/// builds panic on one.
pub fn hash_join<K: Hash + Eq, V>(
    build: &[(K, V)],
    probe: &[(K, V)],
    hasher: impl BuildHasher,
    mut emit: impl FnMut(&(K, V), &(K, V)),
) {
    let mut table = HashMap::with_capacity_and_hasher(build.len(), hasher);
    for row in build {
        let replaced = table.insert(&row.0, row);
        debug_assert!(replaced.is_none(), "build input repeats a key");
    }
    for row in probe {
        if let Some(built) = table.get(&row.0) {
            emit(built, row);
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
    #[cfg(debug_assertions)]
    #[should_panic(expected = "build input repeats a key")]
    fn refuses_a_repeated_build_key_in_debug_builds() {
        hash_join(
            &[(1, 'a'), (1, 'b')],
            &[(1, 'x')],
            RandomState::new(),
            |_, _| {},
        );
    }
}
