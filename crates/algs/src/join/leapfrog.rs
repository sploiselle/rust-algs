//! Leapfrog join: intersecting any number of sorted inputs by seeking, with no
//! heap.

use super::multiway::cross_product;

/// Joins any number of sorted inputs as
/// [`multiway_merge_join`](super::multiway_merge_join) does, emitting the same
/// tuples in the same order, but jumps past runs of keys some input lacks
/// instead of reading them one at a time.
///
/// The input furthest behind seeks to the key of the input furthest ahead,
/// which makes it the furthest ahead. So the inputs stay in a cycle ordered by
/// their next keys, and finding the one furthest behind is a step around it,
/// not a heap (Veldhuizen, *Leapfrog Triejoin*, 2014). Each input is furthest
/// behind once every `k` steps, and moves past a key each time, so the input
/// with the fewest keys bounds the work.
///
/// Unsorted inputs give wrong answers: it misses matches, and since it
/// compares only the inputs furthest behind and furthest ahead, it can join
/// rows whose keys differ.
pub fn leapfrog_join<K: Ord, V>(inputs: &[&[(K, V)]], mut emit: impl FnMut(&[&(K, V)])) {
    let k = inputs.len();
    if k == 0 || inputs.iter().any(|rows| rows.is_empty()) {
        return;
    }
    let mut rest = inputs.to_vec();
    // The inputs by next key, read as a cycle from `p`: `order[p]` is furthest
    // behind, and the input before it furthest ahead.
    let mut order: Vec<usize> = (0..k).collect();
    order.sort_unstable_by_key(|&i| &rest[i][0].0);
    let mut p = 0;
    let mut runs: Vec<&[(K, V)]> = vec![&[]; k];
    let mut picked = Vec::with_capacity(k);
    loop {
        let (behind, ahead) = (order[p], order[(p + k - 1) % k]);
        let target = &rest[ahead][0].0;
        if rest[behind][0].0 == *target {
            // The furthest behind has caught up, so every input is at `target`.
            for (run, rows) in runs.iter_mut().zip(&rest) {
                *run = rows.chunk_by(|a, b| a.0 == b.0).next().unwrap_or_default();
            }
            cross_product(&runs, &mut picked, &mut emit);
            // Step only `behind` past `target`, which makes it the furthest
            // ahead, as a seek would. Stepping every input would leave the
            // cycle out of order; the others seek past `target` on their turns.
            rest[behind] = &rest[behind][runs[behind].len()..];
        } else {
            rest[behind] = seek(rest[behind], target);
        }
        if rest[behind].is_empty() {
            return;
        }
        p = (p + 1) % k;
    }
}

/// `rows` from the first row whose key is at least `target`. It gallops 1, 2,
/// 4, … rows ahead, then binary-searches the last stride, so skipping `d` rows
/// costs about 2 log₂ d comparisons.
fn seek<'a, K: Ord, V>(rows: &'a [(K, V)], target: &K) -> &'a [(K, V)] {
    // Gallop: every row before `bound / 2` is below `target`.
    let mut bound = 1;
    while bound < rows.len() && rows[bound].0 < *target {
        bound *= 2;
    }
    // So the first row at or past `target` is in `rows[bound / 2..=bound]`.
    // The gallop found `rows[bound]`, if there is one, isn't below `target`,
    // so the search leaves it out: if every row it searches is below, the
    // answer is `bound`.
    let start = bound / 2;
    let end = rows.len().min(bound);
    let first = start + rows[start..end].partition_point(|row| row.0 < *target);
    &rows[first..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::multiway_merge_join;
    use crate::join::testing::{Row, every_matching_tuple, many, tuples};
    use crate::test_util::{Counted, comparisons};
    use proptest::collection::vec;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(crate::test_util::config())]

        #[test]
        fn agrees_with_every_matching_tuple(inputs in many()) {
            let inputs: Vec<&[Row]> = inputs.iter().map(Vec::as_slice).collect();
            let joined = tuples(|emit| leapfrog_join(&inputs, emit));
            prop_assert_eq!(joined, every_matching_tuple(&inputs));
        }

        #[test]
        fn emits_exactly_what_the_heap_merge_emits(inputs in many()) {
            let inputs: Vec<&[Row]> = inputs.iter().map(Vec::as_slice).collect();
            let leapt = tuples(|emit| leapfrog_join(&inputs, emit));
            let merged = tuples(|emit| multiway_merge_join(&inputs, emit));
            prop_assert_eq!(leapt, merged);
        }

        /// Including on no rows, and for targets below and past every row.
        #[test]
        fn seek_lands_where_partition_point_does(keys in vec(any::<u8>(), 0..300), target in any::<u8>()) {
            let mut rows: Vec<(u8, ())> = keys.into_iter().map(|key| (key, ())).collect();
            rows.sort_unstable();
            let found = seek(&rows, &target);
            prop_assert_eq!(found.len(), rows.len() - rows.partition_point(|row| row.0 < target));
        }
    }

    #[test]
    fn with_one_input_it_emits_every_row() {
        let rows = [(1, 'a'), (1, 'b'), (4, 'c')];
        let mut emitted = Vec::new();
        leapfrog_join(&[&rows], |tuple| emitted.push(*tuple[0]));
        assert_eq!(emitted, rows);
    }

    /// Every distance from 0 to 4,095 rows, against a bound of
    /// 2·⌈log₂(d + 2)⌉ + 1 comparisons: two per bit of `d + 1`, plus one.
    #[test]
    fn seek_skips_d_rows_in_about_two_log_d_comparisons() {
        let rows: Vec<(Counted, ())> = (0..10_000).map(|i| (Counted(i), ())).collect();
        for d in 0..4_096 {
            let (made, found) = comparisons(|| seek(&rows, &Counted(d)).len());
            assert_eq!(found, rows.len() - d as usize);
            let bits = u64::from(u32::BITS - (d + 1).leading_zeros());
            assert!(made <= 2 * bits + 1, "{made} comparisons to skip {d} rows");
        }
    }

    /// Ten keys spread through the middle input: leapfrog seeks the other two
    /// from one to the next, while the heap merges every key of both up to
    /// the last of them. The input with the fewest keys bounds leapfrog's
    /// work, not the inputs with the most.
    #[test]
    fn skips_what_the_heap_reads() {
        let all: Vec<(Counted, ())> = (0..65_536).map(|i| (Counted(i), ())).collect();
        let ten: Vec<(Counted, ())> = (0..10).map(|i| (Counted(i * 6_553 + 17), ())).collect();
        let inputs: [&[(Counted, ())]; 3] = [&all, &ten, &all];
        let (leapt, from_leapfrog) = comparisons(|| {
            let mut emitted = 0;
            leapfrog_join(&inputs, |_| emitted += 1);
            emitted
        });
        let (merged, from_heap) = comparisons(|| {
            let mut emitted = 0;
            multiway_merge_join(&inputs, |_| emitted += 1);
            emitted
        });
        assert_eq!((from_leapfrog, from_heap), (10, 10));
        assert!(leapt < 1_000, "leapfrog made {leapt} comparisons");
        assert!(merged > 500_000, "the heap made only {merged} comparisons");
    }

    /// Unsorted, input `a` falls back to key 0 after the first match, but
    /// leapfrog compares only `c`, furthest behind, with `b`, furthest ahead,
    /// both at key 1, and joins `a`'s row anyway.
    #[test]
    fn unsorted_inputs_can_join_rows_whose_keys_differ() {
        let a = [(1, 'a'), (0, 'b')];
        let b = [(1, 'p'), (5, 'q')];
        let c = [(1, 'x'), (5, 'y')];
        let mut keys = Vec::new();
        leapfrog_join(&[&a, &b, &c], |tuple| {
            keys.push(tuple.iter().map(|row| row.0).collect::<Vec<_>>());
        });
        assert_eq!(keys, [[1, 1, 1], [0, 1, 1]]);
    }
}
