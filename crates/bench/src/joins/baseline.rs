//! A control for the comparisons: a join the benchmarks measure the library's
//! against, not one the post teaches.

/// [`multiway_merge_join`](algs::join::multiway_merge_join) with the heap
/// replaced by scans of every input's next key, O(k) per step.
///
/// It differs from it only in how it finds the inputs at the smallest key:
/// one pass over every input's next row finds the key, and notices an input
/// that has run out, whose `None` sorts first; a second takes the row at it
/// from each input whose next key it is. The heap finds them at its top
/// instead, in O(log k) each. Everything else is the same, and so is the
/// memory but for the heap's, so the two time the heap. A scan's constant
/// factor depends on how its passes compile, and the first is written as one
/// pass so that it compiles to a tight loop.
pub fn multiway_merge_join_by_scan<K: Ord, V>(
    inputs: &[&[(K, V)]],
    mut emit: impl FnMut(&[&(K, V)]),
) {
    let k = inputs.len();
    let mut rest = inputs.to_vec();
    let mut picked = Vec::with_capacity(k);
    // The smallest next key, until an input runs out (`None` is smallest),
    // after which no key is in all of them.
    while let Some(Some(key)) = rest
        .iter()
        .map(|&rows| rows.first().map(|row| &row.0))
        .min()
    {
        // Take the row at `key` from each input whose next key it is, in
        // input order.
        picked.clear();
        for slot in &mut rest {
            let rows = *slot;
            if rows[0].0 == *key {
                picked.push(&rows[0]);
                *slot = &rows[1..];
            }
        }
        // Only a key every input has joins.
        if picked.len() == k {
            emit(&picked);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::joins::workload::Rng;
    use algs::join::multiway_merge_join;

    const CASES: usize = 500;

    /// A key below `keys`, and the row's index in its input.
    type Row = (u64, usize);

    /// Each key below `keys`, at even odds, in order.
    fn relation(rng: &mut Rng, keys: u64) -> Vec<Row> {
        (0..keys)
            .filter(|_| rng.below(2) == 0)
            .enumerate()
            .map(|(i, key)| (key, i))
            .collect()
    }

    /// What an n-way join emitted, in order.
    fn tuples(join: impl FnOnce(&mut dyn FnMut(&[&Row]))) -> Vec<Vec<Row>> {
        let mut out = Vec::new();
        join(&mut |tuple: &[&Row]| out.push(tuple.iter().map(|&&row| row).collect()));
        out
    }

    #[test]
    fn the_scan_agrees_with_the_heap() {
        let mut rng = Rng::seeded("baseline/scan");
        for _ in 0..CASES {
            let keys = 1 + rng.below(16);
            let k = rng.below(5) as usize;
            let inputs: Vec<Vec<Row>> = (0..k).map(|_| relation(&mut rng, keys)).collect();
            let inputs: Vec<&[Row]> = inputs.iter().map(Vec::as_slice).collect();
            assert_eq!(
                tuples(|emit| multiway_merge_join_by_scan(&inputs, emit)),
                tuples(|emit| multiway_merge_join(&inputs, emit)),
                "{inputs:?}"
            );
        }
    }
}
