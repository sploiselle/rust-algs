//! The merge join: two inputs sorted by key, walked side by side.

use std::cmp::Ordering;

/// Joins two inputs sorted by key, calling `emit` with every pair whose keys
/// are equal, in key order. It makes one pass over each input and needs no
/// memory of its own.
///
/// Unsorted inputs, and inputs that repeat a key, lose matches; debug builds
/// panic on them.
pub fn merge_join<K: Ord, V>(
    left: &[(K, V)],
    right: &[(K, V)],
    mut emit: impl FnMut(&(K, V), &(K, V)),
) {
    debug_assert!(
        left.is_sorted_by(|a, b| a.0 < b.0),
        "left input's keys don't strictly increase"
    );
    debug_assert!(
        right.is_sorted_by(|a, b| a.0 < b.0),
        "right input's keys don't strictly increase"
    );
    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() {
        let l = &left[i];
        let r = &right[j];
        // The smaller key matches nothing still to come on the other side, so
        // move past it.
        match l.0.cmp(&r.0) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                emit(l, r);
                i += 1;
                j += 1;
            }
        }
    }
}

/// Sorts both inputs by key, then merge-joins them. It takes them by value
/// because it has to reorder them: its working memory is the inputs
/// themselves, which a caller holding borrowed rows pays for with a copy.
pub fn sort_merge_join<K: Ord, V>(
    mut left: Vec<(K, V)>,
    mut right: Vec<(K, V)>,
    emit: impl FnMut(&(K, V), &(K, V)),
) {
    left.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    right.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    merge_join(&left, &right, emit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::testing::{loop_pairs, pairs, sorted, two};
    use crate::test_util::{Counted, comparisons};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(crate::test_util::config())]

        #[test]
        fn agrees_with_a_nested_loop_join((left, right) in two()) {
            let left = sorted(left);
            let right = sorted(right);
            let joined = pairs(|emit| merge_join(&left, &right, emit));
            prop_assert_eq!(joined, loop_pairs(&left, &right));
        }

        #[test]
        fn emits_each_key_once_in_key_order((left, right) in two()) {
            let left = sorted(left);
            let right = sorted(right);
            let mut keys = Vec::new();
            merge_join(&left, &right, |l, _| keys.push(l.0));
            prop_assert!(keys.is_sorted_by(|a, b| a < b));
        }

        #[test]
        fn sort_merge_agrees_with_a_nested_loop_join_on_unsorted_input((left, right) in two()) {
            let joined = pairs(|emit| sort_merge_join(left.clone(), right.clone(), emit));
            prop_assert_eq!(joined, loop_pairs(&left, &right));
        }
    }

    /// One pass: each comparison moves past a row of one input or both, so
    /// there are fewer comparisons than rows, whether every key matches,
    /// none does, or the side that moves alternates.
    #[test]
    fn compares_keys_fewer_times_than_there_are_rows() {
        const ROWS: u32 = 1_000;
        let all: Vec<(Counted, ())> = (0..ROWS).map(|i| (Counted(i), ())).collect();
        let above: Vec<(Counted, ())> = (ROWS..2 * ROWS).map(|i| (Counted(i), ())).collect();
        let evens: Vec<(Counted, ())> = (0..ROWS).map(|i| (Counted(2 * i), ())).collect();
        let odds: Vec<(Counted, ())> = (0..ROWS).map(|i| (Counted(2 * i + 1), ())).collect();
        for (left, right) in [(&all, &all), (&all, &above), (&evens, &odds)] {
            let (made, emitted) = comparisons(|| {
                let mut emitted = 0;
                merge_join(left, right, |_, _| emitted += 1);
                emitted
            });
            let rows = (left.len() + right.len()) as u64;
            // Debug builds first check each input, one comparison per
            // adjacent pair of rows.
            let checks = if cfg!(debug_assertions) { rows - 2 } else { 0 };
            assert!(
                made - checks < rows,
                "{made} comparisons for {emitted} pairs"
            );
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "left input's keys don't strictly increase")]
    fn refuses_an_unsorted_left_input_in_debug_builds() {
        merge_join(&[(2, ()), (1, ())], &[(1, ())], |_, _| {});
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "right input's keys don't strictly increase")]
    fn refuses_an_unsorted_right_input_in_debug_builds() {
        merge_join(&[(1, ())], &[(2, ()), (1, ())], |_, _| {});
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "left input's keys don't strictly increase")]
    fn refuses_a_repeated_left_key_in_debug_builds() {
        merge_join(&[(1, ()), (1, ())], &[(1, ())], |_, _| {});
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "right input's keys don't strictly increase")]
    fn refuses_a_repeated_right_key_in_debug_builds() {
        merge_join(&[(1, ())], &[(1, ()), (1, ())], |_, _| {});
    }
}
