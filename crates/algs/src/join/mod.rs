//! # Joins
//!
//! A join pairs the rows of two inputs whose keys are equal: each account with
//! its balance, say. Here every input holds each key at most once, so a key
//! both inputs hold joins to exactly one pair. Databases join in one of three
//! ways, and which is fastest depends on the inputs: whether they arrive
//! sorted, and how big the smaller one is.
//!
//! - A [nested loop join](nested_loop_join) compares every row of one input
//!   with every row of the other. It's the definition of a join, run
//!   literally.
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
//! the key of the input furthest ahead, so it jumps past keys some input lacks
//! in one seek instead of reading them one by one.
//!
//! ## Rows and output
//!
//! Rows are `(key, value)` pairs, the same type in every input. A key
//! every input holds joins to one pair, or to one tuple of a row from each
//! input, so a join on equal keys emits no more tuples than its smallest
//! input has rows. Every join but the cascade hands each match to `emit` as it
//! finds it, the way a push-based engine hands rows to the next operator, so
//! the memory it uses is its own and not the size of its output. A caller that
//! wants to keep the matches copies them out.
//!
//! ## Costs
//!
//! For inputs of `n` and `m` rows, or `k` inputs of `N` rows in all, the
//! fewest rows of any of them `u`:
//!
//! | join | work | working memory | needs |
//! |---|---|---|---|
//! | [`nested_loop_join`] | `n·m` key comparisons | none | keys that compare equal |
//! | [`hash_join`] | `n` inserts, `m` lookups | a table of the build side's `n` rows | keys that hash |
//! | [`merge_join`] | at most `n + m` comparisons | none | both inputs sorted |
//! | [`sort_merge_join`] | `O(n log n + m log m)` | the inputs, owned | keys that order |
//! | [`multiway_merge_join`] | `O(N log k)` | `O(k)` | every input sorted |
//! | [`cascade_merge_join`] | each binary join's, on each result | two consecutive results, the last the whole output | every input sorted, rows that clone |
//! | [`leapfrog_join`] | at most `k·u` seeks, each `O(log d)` to skip `d` rows | `O(k)` | every input sorted |
//!
//! The nested loop join emits in the left input's order and the hash join in
//! the probe side's; the rest emit in key order, each key once, so their
//! output can feed another merge join without a sort.
//!
//! ## Left out
//!
//! Real engines join inputs whose keys repeat, spill to disk when a hash table
//! or a sort outgrows memory, partition their inputs so each piece fits in
//! cache, run on many cores, and support outer, semi and anti joins. None of
//! that is here: these are in-memory inner joins of inputs that hold each key
//! once.

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
    use proptest::collection::{btree_map, vec};
    use proptest::prelude::*;

    /// A key and a value.
    pub type Row = (u8, u8);

    /// Up to 16 rows with distinct keys below `keys`, sorted by key.
    pub fn relation(keys: u8) -> impl Strategy<Value = Vec<Row>> {
        btree_map(0..keys, any::<u8>(), 0..=16).prop_map(|rows| rows.into_iter().collect())
    }

    /// Two inputs, shuffled. Drawing keys from one to sixteen values ranges
    /// them from sharing nearly every key to sharing few, and empty inputs are
    /// common.
    pub fn two() -> impl Strategy<Value = (Vec<Row>, Vec<Row>)> {
        (1u8..=16)
            .prop_flat_map(|keys| (relation(keys).prop_shuffle(), relation(keys).prop_shuffle()))
    }

    /// Up to four inputs, each sorted by key.
    pub fn many() -> impl Strategy<Value = Vec<Vec<Row>>> {
        (1u8..=16, 0usize..=4).prop_flat_map(|(keys, k)| vec(relation(keys), k))
    }

    /// `rows` sorted by key.
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
        pairs(|emit| nested_loop_join(left, right, emit))
    }

    /// The tuples a join of sorted `inputs` must emit, in the order it must
    /// emit them: for each key of the first input that every input holds, in
    /// key order, each input's row with that key.
    pub fn every_matching_tuple(inputs: &[&[Row]]) -> Vec<Vec<Row>> {
        let Some(first) = inputs.first() else {
            return Vec::new();
        };
        first
            .iter()
            .filter_map(|&(key, _)| {
                inputs
                    .iter()
                    .map(|rows| rows.iter().find(|row| row.0 == key).copied())
                    .collect()
            })
            .collect()
    }
}
