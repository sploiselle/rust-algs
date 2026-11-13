//! Joining any number of sorted inputs on one key: all at once with a binary
//! heap, or two at a time.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::binary_heap::PeekMut;

use super::merge::merge_join;

/// Joins any number of inputs sorted by key: for each key every input holds,
/// in key order, calls `emit` with a slice whose `i`-th row is input `i`'s row
/// with that key. With two inputs it is [`merge_join`]; with none it emits
/// nothing.
///
/// It is a k-way merge: a min-heap of each input's next key says which input
/// is furthest behind in O(log k), and its working memory is O(k). Unsorted
/// inputs lose matches. An input that repeats a key can also put its second
/// row at that key in another input's slot, emitting a tuple that isn't a
/// match. Debug builds panic on either, in the rows the merge reads.
///
/// The heap holds a clone of each input's next key, not a reference to it. A
/// sift compares its way down the heap, and each comparison's outcome picks
/// the next entry to read. Through a reference, every one of those steps
/// reads the entry and then the key it points to; with the key in the entry,
/// it reads the entry alone. So it suits keys that are cheap to clone.
pub fn multiway_merge_join<K: Ord + Clone, V>(
    inputs: &[&[(K, V)]],
    mut emit: impl FnMut(&[&(K, V)]),
) {
    let k = inputs.len();
    let mut rest = inputs.to_vec();
    let mut picked = Vec::with_capacity(k);
    // Each input's next key and the input it came from, smallest key on top.
    let mut heads = BinaryHeap::with_capacity(k);
    for (i, rows) in rest.iter().enumerate() {
        if let Some((key, _)) = rows.first() {
            heads.push(Reverse((key.clone(), i)));
        }
    }
    // Once any input runs out, no later key is in all of them.
    while k > 0 && heads.len() == k {
        let key = heads.peek().unwrap().0.0.clone();
        // Take the row at `key` from each input whose next key it is. Ties on
        // the key come off the heap by input, so `picked` is in input order.
        picked.clear();
        while let Some(mut top) = heads.peek_mut()
            && top.0.0 == key
        {
            let i = top.0.1;
            let rows = rest[i];
            picked.push(&rows[0]);
            rest[i] = &rows[1..];
            match rest[i].first() {
                Some((next, _)) => {
                    debug_assert!(*next > key, "input {i}'s keys don't strictly increase");
                    // Replace the top in place: one sift down, when `top` drops.
                    *top = Reverse((next.clone(), i));
                }
                None => drop(PeekMut::pop(top)),
            }
        }
        // Only a key every input has joins.
        if picked.len() == k {
            emit(&picked);
        }
    }
}

/// Joins the inputs the way a left-deep plan of binary merge joins does when
/// it materializes each result, the last one too: the first two, then that
/// result with the third, and so on, copying every row of every tuple it
/// makes and emitting only once every join is done. It emits what
/// [`multiway_merge_join`] emits, in the same order, but how big the
/// intermediate results get depends on the order of `inputs`.
///
/// # Panics
///
/// If rows are zero-sized: it finds a tuple by where its first row is.
pub fn cascade_merge_join<K: Ord + Clone, V: Clone>(
    inputs: &[&[(K, V)]],
    mut emit: impl FnMut(&[&(K, V)]),
) {
    let Some((first, others)) = inputs.split_first() else {
        return;
    };
    // The result so far, a column per input joined: tuple `t` is row `t` of
    // every column. Each column is in key order, so the first is the result's
    // keys for the next merge.
    let mut columns = vec![first.to_vec()];
    for input in others {
        let mut next = vec![Vec::new(); columns.len() + 1];
        merge_join(&columns[0], input, |l, r| {
            // `l` is a row of the first column, so where it sits is its tuple.
            let t = columns[0].element_offset(l).unwrap();
            for (column, next) in columns.iter().zip(&mut next) {
                next.push(column[t].clone());
            }
            next[columns.len()].push(r.clone());
        });
        columns = next;
    }
    let mut picked = Vec::with_capacity(columns.len());
    for t in 0..columns[0].len() {
        picked.clear();
        picked.extend(columns.iter().map(|column| &column[t]));
        emit(&picked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::testing::{Row, every_matching_tuple, many, sorted, tuples, two};
    use crate::test_util::{Counted, comparisons};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(crate::test_util::config())]

        #[test]
        fn agrees_with_every_matching_tuple(inputs in many()) {
            let inputs: Vec<&[Row]> = inputs.iter().map(Vec::as_slice).collect();
            let joined = tuples(|emit| multiway_merge_join(&inputs, emit));
            prop_assert_eq!(joined, every_matching_tuple(&inputs));
        }

        #[test]
        fn with_two_inputs_it_is_a_merge_join((left, right) in two()) {
            let left = sorted(left);
            let right = sorted(right);
            let joined = tuples(|emit| multiway_merge_join(&[&left, &right], emit));
            let merged = tuples(|emit| merge_join(&left, &right, |l, r| emit(&[l, r])));
            prop_assert_eq!(joined, merged);
        }

        /// Row `i` of every tuple comes from input `i`, whatever order the
        /// inputs' keys run out in.
        #[test]
        fn each_slot_holds_a_row_of_its_own_input(inputs in many()) {
            let labelled: Vec<Vec<Row>> = inputs
                .iter()
                .enumerate()
                .map(|(i, rows)| rows.iter().map(|&(key, _)| (key, i as u8)).collect())
                .collect();
            let labelled: Vec<&[Row]> = labelled.iter().map(Vec::as_slice).collect();
            for tuple in tuples(|emit| multiway_merge_join(&labelled, emit)) {
                for (i, row) in tuple.iter().enumerate() {
                    prop_assert_eq!(usize::from(row.1), i);
                }
            }
        }

        #[test]
        fn the_cascade_agrees_with_every_matching_tuple(inputs in many()) {
            let inputs: Vec<&[Row]> = inputs.iter().map(Vec::as_slice).collect();
            let joined = tuples(|emit| cascade_merge_join(&inputs, emit));
            prop_assert_eq!(joined, every_matching_tuple(&inputs));
        }
    }

    #[test]
    fn with_one_input_it_emits_every_row() {
        let rows = [(1, 'a'), (2, 'b'), (4, 'c')];
        let mut merged = Vec::new();
        let mut cascaded = Vec::new();
        multiway_merge_join(&[&rows], |tuple| merged.push(*tuple[0]));
        cascade_merge_join(&[&rows], |tuple| cascaded.push(*tuple[0]));
        assert_eq!(merged, rows);
        assert_eq!(cascaded, rows);
    }

    #[test]
    fn with_no_inputs_it_emits_nothing() {
        let none: [&[Row]; 0] = [];
        multiway_merge_join(&none, |_| panic!("emitted a tuple of no rows"));
        cascade_merge_join(&none, |_| panic!("emitted a tuple of no rows"));
    }

    #[test]
    fn an_empty_input_joins_to_nothing() {
        let rows: [Row; 3] = [(1, 0), (2, 1), (3, 2)];
        let inputs: [&[Row]; 3] = [&rows, &[], &rows];
        multiway_merge_join(&inputs, |_| panic!("an input had no rows"));
        cascade_merge_join(&inputs, |_| panic!("an input had no rows"));
    }

    /// It stops as soon as one input runs out: the short input's ten keys,
    /// not the long inputs' 65,536, bound the work.
    #[test]
    fn stops_once_any_input_runs_out() {
        let short: Vec<(Counted, ())> = (0..10).map(|i| (Counted(i), ())).collect();
        let long: Vec<(Counted, ())> = (0..65_536).map(|i| (Counted(i), ())).collect();
        let (made, emitted) = comparisons(|| {
            let mut emitted = 0;
            multiway_merge_join(&[&short, &long, &long], |_| emitted += 1);
            emitted
        });
        assert_eq!(emitted, 10);
        assert!(made < 200, "{made} comparisons");
    }

    /// Unchecked, input 0's second row would take input 1's slot, and the
    /// join would emit a tuple for a key input 1 doesn't hold.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "input 0's keys don't strictly increase")]
    fn refuses_a_repeated_key_in_debug_builds() {
        let inputs: [&[Row]; 2] = [&[(1, 0), (1, 1)], &[(2, 16)]];
        multiway_merge_join(&inputs, |_| {});
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "input 1's keys don't strictly increase")]
    fn refuses_an_unsorted_input_in_debug_builds() {
        let inputs: [&[Row]; 2] = [&[(1, 0), (2, 1)], &[(1, 2), (0, 3)]];
        multiway_merge_join(&inputs, |_| {});
    }
}
