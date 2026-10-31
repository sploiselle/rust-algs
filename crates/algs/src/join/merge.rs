//! The merge join: two inputs sorted by key, walked side by side.

use std::cmp::Ordering;

/// Joins two inputs sorted by key, calling `emit` with every pair whose keys
/// are equal, in key order. A key on both sides joins as the cross product of
/// its two runs, left-major. It makes one pass over each input and needs no
/// memory of its own.
///
/// Unsorted inputs lose matches; debug builds panic on them.
pub fn merge_join<K: Ord, A, B>(
    mut left: &[(K, A)],
    mut right: &[(K, B)],
    mut emit: impl FnMut(&(K, A), &(K, B)),
) {
    debug_assert!(
        left.is_sorted_by_key(|row| &row.0),
        "left input isn't sorted"
    );
    debug_assert!(
        right.is_sorted_by_key(|row| &row.0),
        "right input isn't sorted"
    );
    while let (Some(l), Some(r)) = (left.first(), right.first()) {
        match l.0.cmp(&r.0) {
            // The smaller key matches nothing still to come on the other side.
            Ordering::Less => left = &left[1..],
            Ordering::Greater => right = &right[1..],
            // Pair every row of the left run with every row of the right run.
            Ordering::Equal => {
                let (left_run, left_rest) = left.split_at(run_len(left));
                let (right_run, right_rest) = right.split_at(run_len(right));
                for l in left_run {
                    for r in right_run {
                        emit(l, r);
                    }
                }
                (left, right) = (left_rest, right_rest);
            }
        }
    }
}

/// How many leading rows share the first row's key (0 for no rows).
pub(super) fn run_len<K: Eq, V>(rows: &[(K, V)]) -> usize {
    rows.iter().take_while(|row| row.0 == rows[0].0).count()
}

/// Sorts both inputs by key, then merge-joins them. It takes them by value
/// because it has to reorder them: its working memory is the inputs
/// themselves, which a caller holding borrowed rows pays for with a copy.
pub fn sort_merge_join<K: Ord, A, B>(
    mut left: Vec<(K, A)>,
    mut right: Vec<(K, B)>,
    emit: impl FnMut(&(K, A), &(K, B)),
) {
    left.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    right.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    merge_join(&left, &right, emit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::nested_loop_join;
    use crate::join::testing::{Row, loop_pairs, pairs, sorted, two};
    use crate::test_util::{Counted, comparisons};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(crate::test_util::config())]

        #[test]
        fn agrees_with_a_nested_loop_join((left, right) in two()) {
            let (left, right) = (sorted(left), sorted(right));
            let joined = pairs(|emit| merge_join(&left, &right, emit));
            prop_assert_eq!(joined, loop_pairs(&left, &right));
        }

        #[test]
        fn emits_in_key_order((left, right) in two()) {
            let (left, right) = (sorted(left), sorted(right));
            let mut keys = Vec::new();
            merge_join(&left, &right, |l, _| keys.push(l.0));
            prop_assert!(keys.is_sorted());
        }

        #[test]
        fn sort_merge_agrees_with_a_nested_loop_join_on_unsorted_input((left, right) in two()) {
            let joined = pairs(|emit| sort_merge_join(left.clone(), right.clone(), emit));
            prop_assert_eq!(joined, loop_pairs(&left, &right));
        }
    }

    #[test]
    fn pairs_every_row_of_one_run_with_every_row_of_the_other() {
        let left = [(0, 'x'), (1, 'a'), (1, 'b'), (2, 'y')];
        let right = [(1, 'p'), (1, 'q'), (1, 'r'), (3, 'z')];
        let mut joined = Vec::new();
        merge_join(&left, &right, |l, r| joined.push((l.1, r.1)));
        assert_eq!(
            joined,
            [
                ('a', 'p'),
                ('a', 'q'),
                ('a', 'r'),
                ('b', 'p'),
                ('b', 'q'),
                ('b', 'r')
            ]
        );
    }

    /// The worst case for duplicates: when every row has the same key, a
    /// merge join is a nested loop join, pair for pair and in the same order.
    #[test]
    fn with_one_key_it_is_a_loop_join() {
        let left: Vec<Row> = (0..5).map(|i| (7, i)).collect();
        let right: Vec<Row> = (0..4).map(|i| (7, i)).collect();
        let mut merged = Vec::new();
        merge_join(&left, &right, |l, r| merged.push((*l, *r)));
        let mut looped = Vec::new();
        nested_loop_join(&left, &right, |_, _| true, |l, r| looped.push((*l, *r)));
        assert_eq!(merged, looped);
    }

    /// One pass over each input: at most three key comparisons per row,
    /// whether keys are unique, never match, or come eight to a key.
    #[test]
    fn compares_keys_a_linear_number_of_times() {
        const ROWS: u32 = 1_000;
        let unique: Vec<(Counted, ())> = (0..ROWS).map(|i| (Counted(i), ())).collect();
        let above: Vec<(Counted, ())> = (ROWS..2 * ROWS).map(|i| (Counted(i), ())).collect();
        let eights: Vec<(Counted, ())> = (0..ROWS).map(|i| (Counted(i / 8), ())).collect();
        for (left, right) in [(&unique, &unique), (&unique, &above), (&eights, &eights)] {
            let (made, emitted) = comparisons(|| {
                let mut emitted = 0;
                merge_join(left, right, |_, _| emitted += 1);
                emitted
            });
            // Debug builds check that both inputs are sorted first, one
            // comparison per row.
            let checks = if cfg!(debug_assertions) {
                2 * u64::from(ROWS)
            } else {
                0
            };
            assert!(
                made.saturating_sub(checks) <= 3 * 2 * u64::from(ROWS),
                "{made} comparisons for {emitted} pairs"
            );
        }
    }

    #[test]
    fn run_len_counts_the_rows_sharing_the_first_key() {
        assert_eq!(run_len::<u8, ()>(&[]), 0);
        assert_eq!(run_len(&[(1, ()), (1, ()), (2, ()), (1, ())]), 2);
        assert_eq!(run_len(&[(3, ()), (3, ()), (3, ())]), 3);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "left input isn't sorted")]
    fn refuses_an_unsorted_left_input_in_debug_builds() {
        merge_join(&[(2, ()), (1, ())], &[(1, ())], |_, _| {});
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "right input isn't sorted")]
    fn refuses_an_unsorted_right_input_in_debug_builds() {
        merge_join(&[(1, ())], &[(2, ()), (1, ())], |_, _| {});
    }
}
