//! Controls for the comparisons: joins the benchmarks measure the library's
//! against, not ones the post teaches.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// [`multiway_merge_join`](algs::join::multiway_merge_join) with the heap
/// replaced by scans of every input's next key, O(k) per step.
///
/// It differs from it only in how it finds the inputs at the smallest key:
/// one pass over every input's next row finds the key, and notices an input
/// that has run out, whose `None` sorts first; a second takes the runs at it.
/// The heap finds them at its top instead, in O(log k) each. The runs, the
/// cross product and the memory are the same, so the two time the heap. A
/// scan's constant factor depends on how its passes compile, and the first
/// is written as one pass so that it compiles to a tight loop.
pub fn multiway_merge_join_by_scan<K: Ord, V>(
    inputs: &[&[(K, V)]],
    mut emit: impl FnMut(&[&(K, V)]),
) {
    let k = inputs.len();
    let mut rest = inputs.to_vec();
    let mut runs: Vec<&[(K, V)]> = vec![&[]; k];
    let mut picked = Vec::with_capacity(k);
    // The smallest next key, until an input runs out (`None` is smallest),
    // after which no key is in all of them.
    while let Some(Some(key)) = rest
        .iter()
        .map(|&rows| rows.first().map(|row| &row.0))
        .min()
    {
        // Take the run at `key` from each input whose next key it is.
        let mut found = 0;
        for (run, rows) in runs.iter_mut().zip(rest.iter_mut()) {
            if rows[0].0 == *key {
                (*run, *rows) = rows.split_at(run_len(rows));
                found += 1;
            }
        }
        if found == k {
            cross_product(&runs, &mut picked, &mut emit);
        }
    }
}

/// A hash join whose table maps each key to a Vec of references to its build
/// rows: one allocation per distinct key, where
/// [`hash_join`](algs::join::hash_join) chains the rows through one array,
/// but a key's references sit together (the rows stay where `build` has
/// them), so a probe reads them without walking a chain.
pub fn hash_join_grouped<K: Hash + Eq, A, B>(
    build: &[(K, A)],
    probe: &[(K, B)],
    hasher: impl BuildHasher,
    mut emit: impl FnMut(&(K, A), &(K, B)),
) {
    let mut groups: HashMap<&K, Vec<&(K, A)>, _> =
        HashMap::with_capacity_and_hasher(build.len(), hasher);
    for row in build {
        groups.entry(&row.0).or_default().push(row);
    }
    for row in probe {
        for &matched in groups.get(&row.0).into_iter().flatten() {
            emit(matched, row);
        }
    }
}

// Copies of two helpers that are `pub(super)` in `algs::join`, so the scan
// above runs the same code as the heap merge apart from finding the smallest key.

/// How many leading rows share the first row's key (0 for no rows). The same
/// as `algs::join::merge::run_len`.
fn run_len<K: Eq, V>(rows: &[(K, V)]) -> usize {
    rows.iter().take_while(|row| row.0 == rows[0].0).count()
}

/// Calls `emit` with every way to pick one row from each run, after the rows
/// already `picked`, first run outermost. The same as
/// `algs::join::multiway::cross_product`.
fn cross_product<'a, K, V>(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::Rng;
    use algs::join::{hash_join, multiway_merge_join};
    use std::hash::RandomState;

    const CASES: usize = 500;

    /// A key below `keys`, and the row's index in its input, so every row is
    /// distinct.
    type Row = (u64, usize);

    /// Up to 11 rows with keys below `keys`.
    fn relation(rng: &mut Rng, keys: u64) -> Vec<Row> {
        let len = rng.below(12) as usize;
        (0..len).map(|i| (rng.below(keys), i)).collect()
    }

    /// What an n-way join emitted, in order.
    fn tuples(join: impl FnOnce(&mut dyn FnMut(&[&Row]))) -> Vec<Vec<Row>> {
        let mut out = Vec::new();
        join(&mut |tuple: &[&Row]| out.push(tuple.iter().map(|&&row| row).collect()));
        out
    }

    /// What a binary join emitted, sorted.
    fn pairs(join: impl FnOnce(&mut dyn FnMut(&Row, &Row))) -> Vec<(Row, Row)> {
        let mut out = Vec::new();
        join(&mut |l: &Row, r: &Row| out.push((*l, *r)));
        out.sort_unstable();
        out
    }

    #[test]
    fn the_scan_agrees_with_the_heap() {
        let mut rng = Rng::seeded("baseline/scan");
        for _ in 0..CASES {
            let keys = 1 + rng.below(8);
            let k = rng.below(5) as usize;
            let inputs: Vec<Vec<Row>> = (0..k)
                .map(|_| {
                    let mut rows = relation(&mut rng, keys);
                    rows.sort_by_key(|row| row.0);
                    rows
                })
                .collect();
            let inputs: Vec<&[Row]> = inputs.iter().map(Vec::as_slice).collect();
            assert_eq!(
                tuples(|emit| multiway_merge_join_by_scan(&inputs, emit)),
                tuples(|emit| multiway_merge_join(&inputs, emit)),
                "{inputs:?}"
            );
        }
    }

    #[test]
    fn the_grouped_table_agrees_with_the_chained_one() {
        let mut rng = Rng::seeded("baseline/grouped");
        for _ in 0..CASES {
            let keys = 1 + rng.below(8);
            let (build, probe) = (relation(&mut rng, keys), relation(&mut rng, keys));
            assert_eq!(
                pairs(|emit| hash_join_grouped(&build, &probe, RandomState::new(), emit)),
                pairs(|emit| hash_join(&build, &probe, RandomState::new(), emit)),
                "{build:?} ⋈ {probe:?}"
            );
        }
    }
}
