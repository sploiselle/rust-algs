//! Joining any number of sorted inputs on one key: all at once with a binary
//! heap, or two at a time.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::binary_heap::PeekMut;

use super::merge::merge_join;

/// Joins any number of inputs sorted by key: calls `emit` with every
/// combination of one row from each input whose keys are all equal, as a slice
/// whose `i`-th row comes from input `i`. Tuples come in key order, the first
/// input outermost. With two inputs it is [`merge_join`]; with none it emits
/// nothing.
///
/// It is a k-way merge: a min-heap of each input's next key says which input
/// is furthest behind in O(log k), and its working memory is O(k). Unsorted
/// inputs lose matches.
pub fn multiway_merge_join<K: Ord, V>(inputs: &[&[(K, V)]], mut emit: impl FnMut(&[&(K, V)])) {
    let k = inputs.len();
    let mut rest = inputs.to_vec();
    let mut runs: Vec<&[(K, V)]> = vec![&[]; k];
    let mut picked = Vec::with_capacity(k);
    // Each input's next key and the input it came from, smallest key on top.
    let mut heads = BinaryHeap::with_capacity(k);
    for (i, rows) in rest.iter().enumerate() {
        if let Some((key, _)) = rows.first() {
            heads.push(Reverse((key, i)));
        }
    }
    // Once any input runs out, no later key is in all of them.
    while k > 0 && heads.len() == k {
        let Reverse((key, _)) = *heads.peek().unwrap();
        // Take the run at `key` from each input whose next key it is.
        let mut found = 0;
        while let Some(mut top) = heads.peek_mut()
            && let Reverse((head, i)) = *top
            && head == key
        {
            let rows = rest[i];
            runs[i] = rows.chunk_by(|a, b| a.0 == b.0).next().unwrap_or_default();
            rest[i] = &rows[runs[i].len()..];
            found += 1;
            match rest[i].first() {
                // Replace the top in place: one sift down, when `top` drops.
                Some((next, _)) => *top = Reverse((next, i)),
                None => drop(PeekMut::pop(top)),
            }
        }
        // Only a key every input has joins.
        if found == k {
            cross_product(&runs, &mut picked, &mut emit);
        }
    }
}

/// Calls `emit` with every way to pick one row from each run, after the rows
/// already `picked`, first run outermost.
pub(super) fn cross_product<'a, K, V>(
    runs: &[&'a [(K, V)]],
    picked: &mut Vec<&'a (K, V)>,
    emit: &mut impl FnMut(&[&(K, V)]),
) {
    match runs.split_first() {
        None => emit(picked),
        Some((run, others)) => {
            for row in *run {
                picked.push(row);
                cross_product(others, picked, emit);
                picked.pop();
            }
        }
    }
}

/// Joins the inputs the way a left-deep plan of binary merge joins does when
/// it materializes each result, the last one too: the first two, then that
/// result with the third, and so on, copying every tuple it makes and emitting
/// only once every join is done. It emits what [`multiway_merge_join`] emits,
/// in the same order, but how big the intermediate results get depends on the
/// order of `inputs`.
pub fn cascade_merge_join<K: Ord + Clone, V: Clone>(
    inputs: &[&[(K, V)]],
    mut emit: impl FnMut(&[&(K, V)]),
) {
    let Some((first, others)) = inputs.split_first() else {
        return;
    };
    // The result so far: each tuple's key and where it starts in `tuples`.
    let mut joined: Vec<(K, usize)> = first
        .iter()
        .enumerate()
        .map(|(i, row)| (row.0.clone(), i))
        .collect();
    let mut tuples: Vec<(K, V)> = first.to_vec();
    let mut width = 1;
    for input in others {
        let (mut next_joined, mut next_tuples) = (Vec::new(), Vec::new());
        merge_join(&joined, input, |(key, at), row| {
            next_joined.push((key.clone(), next_tuples.len()));
            next_tuples.extend_from_slice(&tuples[*at..*at + width]);
            next_tuples.push(row.clone());
        });
        (joined, tuples, width) = (next_joined, next_tuples, width + 1);
    }
    let mut picked = Vec::with_capacity(width);
    for (_, at) in &joined {
        picked.clear();
        picked.extend(&tuples[*at..*at + width]);
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
            let (left, right) = (sorted(left), sorted(right));
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
        let rows = [(1, 'a'), (1, 'b'), (4, 'c')];
        let mut emitted = Vec::new();
        multiway_merge_join(&[&rows], |tuple| emitted.push(*tuple[0]));
        assert_eq!(emitted, rows);
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
}
