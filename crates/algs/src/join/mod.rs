//! # Joins
//!
//! A join pairs the rows of two inputs whose keys are equal: each order with
//! the customer who placed it, say. Databases do it in one of three ways, and
//! which is fastest depends on the inputs: whether they arrive sorted, how big
//! the smaller one is, and how often keys repeat.
//!
//! - A [nested loop join](nested_loop_join) compares every row of one input
//!   with every row of the other. It's the definition of a join, run
//!   literally, and the only one of the three that takes any condition, not
//!   just equal keys.
//! - A [hash join](hash_join) builds a hash table on one input's keys, then
//!   looks up each row of the other in it.
//! - A [merge join](merge_join) walks two inputs sorted by key side by side.
//!   Whichever next key is smaller can't match anything still to come on the
//!   other side, so it moves past it. [`sort_merge_join`] sorts its inputs
//!   first.
//!
//! The merge works on any number of sorted inputs that share a key.
//! [`multiway_merge_join`] keeps each input's next key in a binary heap, whose
//! top is the input furthest behind, and joins them all in one pass, with no
//! intermediate results. [`cascade_merge_join`] joins them two at a time
//! instead, the way a plan of binary joins does when it materializes each
//! result. [`leapfrog_join`] needs no heap: the input furthest behind seeks to
//! the key of the input furthest ahead, so it jumps a run of keys some input
//! lacks in one seek instead of reading it key by key.
//!
//! ## Rows and output
//!
//! Rows are `(key, value)` pairs, except in the nested loop join, which takes
//! anything its condition can compare. Every join but the cascade hands each
//! match to `emit` as it finds it, the way a push-based engine hands rows to
//! the next operator, so the memory it uses is its own and not the size of its
//! output. A caller that wants to keep the matches copies them out.
//!
//! ## Costs
//!
//! For inputs of `n` and `m` rows, or `k` inputs of `N` rows in all, the
//! fewest distinct keys of any of them `u`, and an output of `o` tuples:
//!
//! | join | work | working memory | needs |
//! |---|---|---|---|
//! | [`nested_loop_join`] | `n·m` calls of the condition | none | nothing |
//! | [`hash_join`] | `n` inserts, `m` lookups, `o` emits | a table the size of the build side | keys that hash |
//! | [`merge_join`] | `O(n + m + o)` | none | both inputs sorted |
//! | [`sort_merge_join`] | `O(n log n + m log m + o)` | the inputs, owned | keys that order |
//! | [`multiway_merge_join`] | `O(N log k + o)` | `O(k)` | every input sorted |
//! | [`cascade_merge_join`] | each binary join's, on each result | two consecutive results, the last the whole output | every input sorted, rows that clone |
//! | [`leapfrog_join`] | at most `k·u` seeks, each `O(log d)` to skip `d` rows, plus `o` | `O(k)` | every input sorted |
//!
//! The nested loop join emits in the left input's order and the hash join in
//! the probe side's; the rest emit in key order, so their output can feed
//! another merge join without a sort.
//!
//! ## Left out
//!
//! Real engines spill to disk when a hash table or a sort outgrows memory,
//! partition their inputs so each piece fits in cache, run on many cores, and
//! support outer, semi and anti joins. None of that is here: these are the
//! in-memory inner joins those are built on.

mod hash;
mod leapfrog;
mod merge;
mod multiway;
mod nested_loop;

pub use hash::hash_join;
pub use leapfrog::leapfrog_join;
pub use merge::{merge_join, sort_merge_join};
pub use multiway::{cascade_merge_join, multiway_merge_join};
pub use nested_loop::nested_loop_join;

/// Inputs and oracles shared by the joins' tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::nested_loop_join;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    /// A key, and the row's index in its input, so every row in an input is
    /// distinct and a test can tell which one it got.
    pub type Row = (u8, u8);

    /// Up to 11 rows with keys below `keys`.
    pub fn relation(keys: u8) -> impl Strategy<Value = Vec<Row>> {
        vec(0..keys, 0..12).prop_map(|keys| (0..).zip(keys).map(|(i, key)| (key, i)).collect())
    }

    /// Two inputs, unsorted. Drawing keys from one to eight values ranges
    /// them from mostly unique to all one key, and empty inputs are common.
    pub fn two() -> impl Strategy<Value = (Vec<Row>, Vec<Row>)> {
        (1u8..=8).prop_flat_map(|keys| (relation(keys), relation(keys)))
    }

    /// Up to four inputs, each sorted by key.
    pub fn many() -> impl Strategy<Value = Vec<Vec<Row>>> {
        (1u8..=8, 0usize..=4).prop_flat_map(|(keys, k)| vec(relation(keys).prop_map(sorted), k))
    }

    /// `rows` sorted by key, rows with the same key in their original order.
    pub fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
        rows.sort_by_key(|row| row.0);
        rows
    }

    /// Every pair `join` emits, sorted, so joins that emit in different
    /// orders compare equal.
    pub fn pairs(join: impl FnOnce(&mut dyn FnMut(&Row, &Row))) -> Vec<(Row, Row)> {
        let mut pairs = Vec::new();
        join(&mut |l, r| pairs.push((*l, *r)));
        pairs.sort_unstable();
        pairs
    }

    /// Every tuple `join` emits, in the order it emits them.
    pub fn tuples(join: impl FnOnce(&mut dyn FnMut(&[&Row]))) -> Vec<Vec<Row>> {
        let mut tuples = Vec::new();
        join(&mut |tuple| tuples.push(tuple.iter().map(|&&row| row).collect()));
        tuples
    }

    /// The pairs a nested loop join on equal keys finds, sorted: the oracle
    /// for the two-input joins.
    pub fn loop_pairs(left: &[Row], right: &[Row]) -> Vec<(Row, Row)> {
        pairs(|emit| nested_loop_join(left, right, |l, r| l.0 == r.0, emit))
    }

    /// The tuples a join of sorted `inputs` must emit, in the order it must
    /// emit them: for each key in every input, in key order, every way to
    /// pick one of its rows from each input, first input outermost.
    pub fn every_matching_tuple(inputs: &[&[Row]]) -> Vec<Vec<Row>> {
        if inputs.is_empty() {
            return Vec::new();
        }
        let groups: Vec<BTreeMap<u8, Vec<Row>>> = inputs
            .iter()
            .map(|rows| {
                let mut group = BTreeMap::<u8, Vec<Row>>::new();
                for &row in *rows {
                    group.entry(row.0).or_default().push(row);
                }
                group
            })
            .collect();
        let mut tuples = Vec::new();
        for key in groups[0].keys() {
            let Some(runs) = groups
                .iter()
                .map(|group| group.get(key))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let mut partial: Vec<Vec<Row>> = vec![Vec::new()];
            for run in runs {
                partial = partial
                    .into_iter()
                    .flat_map(|tuple| {
                        run.iter().map(move |&row| {
                            let mut longer = tuple.clone();
                            longer.push(row);
                            longer
                        })
                    })
                    .collect();
            }
            tuples.extend(partial);
        }
        tuples
    }
}
