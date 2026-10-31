//! The inputs the joins run on.
//!
//! Every workload is seeded from its name, so filtering, reordering or adding
//! cases never changes another case's rows, and a sorted copy holds exactly
//! the rows of the input it was sorted from. Every payload is a fresh draw
//! from the workload's generator, so none repeats.

use crate::{Row, mix};

/// The keys [`dense`] and [`sparse`] spread across their inputs, about one row
/// each, so the input size stays put as `k` grows.
pub const MULTIWAY_ROWS: usize = 1 << 20;

/// Orders in [`few_customers`], however many customers there are.
const FEW_CUSTOMERS_ORDERS: usize = 1 << 16;

/// SplitMix64: a counter stepped by the golden ratio, scrambled by [`mix`].
/// The state never repeats within 2^64 steps and `mix` is a bijection, so no
/// two outputs of one generator are equal.
pub struct Rng(u64);

impl Rng {
    /// A generator seeded with the FNV-1a hash of `name`.
    pub fn seeded(name: &str) -> Self {
        let hash = name.bytes().fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3)
        });
        Self(hash)
    }

    /// The next number.
    pub fn next_u64(&mut self) -> u64 {
        let out = mix(self.0);
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        out
    }

    /// A number in `0..n`, by scaling rather than `%`, which needs no division.
    pub fn below(&mut self, n: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }

    /// Puts `xs` in a uniformly random order (Fisher–Yates).
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            xs.swap(i, j);
        }
    }
}

/// Two inputs, as generated and sorted by key, and how many pairs they join to.
#[derive(Debug, PartialEq, Eq)]
pub struct Binary {
    pub left: Vec<Row>,
    pub right: Vec<Row>,
    /// `left` sorted by key, made before any timing: it stands for an index.
    pub left_sorted: Vec<Row>,
    pub right_sorted: Vec<Row>,
    /// How many pairs an inner join of the two emits.
    pub expected: u64,
}

impl Binary {
    fn new(left: Vec<Row>, right: Vec<Row>) -> Self {
        let (left_sorted, right_sorted) = (sorted(left.clone()), sorted(right.clone()));
        let expected = matches(&[&left_sorted, &right_sorted]);
        Self {
            left,
            right,
            left_sorted,
            right_sorted,
            expected,
        }
    }
}

/// Inputs to an n-way join, each sorted by key, and how many tuples they join
/// to.
#[derive(Debug, PartialEq, Eq)]
pub struct Multi {
    pub inputs: Vec<Vec<Row>>,
    pub expected: u64,
}

impl Multi {
    fn new(inputs: Vec<Vec<Row>>) -> Self {
        let sorted: Vec<&[Row]> = inputs.iter().map(Vec::as_slice).collect();
        assert!(sorted.iter().all(|rows| rows.is_sorted_by_key(|row| row.0)));
        let expected = matches(&sorted);
        Self { inputs, expected }
    }
}

/// Customers and their orders: a primary key and the foreign keys that point
/// at it. `n / 8` customers with keys `0..n / 8`, once each, shuffled, and `n`
/// orders, each for a customer drawn uniformly, so every order matches one
/// customer.
pub fn customers_orders(n: usize) -> Binary {
    let mut rng = Rng::seeded(&format!("customers_orders/{n}"));
    let customers = (n / 8) as u64;
    let mut keys: Vec<u64> = (0..customers).collect();
    rng.shuffle(&mut keys);
    let left = with_payloads(&mut rng, keys);
    let keys = (0..n).map(|_| rng.below(customers)).collect();
    let right = with_payloads(&mut rng, keys);
    Binary::new(left, right)
}

/// `m` customers with keys `0..m`, and 65,536 orders among them: so few
/// customers that a nested loop compares each order only `m` times, where a
/// hash join looks each one up once.
pub fn few_customers(m: usize) -> Binary {
    let mut rng = Rng::seeded(&format!("few_customers/{m}"));
    let left = with_payloads(&mut rng, (0..m as u64).collect());
    let keys = (0..FEW_CUSTOMERS_ORDERS)
        .map(|_| rng.below(m as u64))
        .collect();
    let right = with_payloads(&mut rng, keys);
    Binary::new(left, right)
}

/// Two inputs of `n` rows with keys drawn from `n / 4`, so each key has about
/// four rows a side and joins to about sixteen pairs: about `4n` in all, more
/// output than input.
pub fn many_to_many(n: usize) -> Binary {
    let mut rng = Rng::seeded(&format!("many_to_many/{n}"));
    let keys = n as u64 / 4;
    let side = |rng: &mut Rng| {
        let keys = (0..n).map(|_| rng.below(keys)).collect();
        with_payloads(rng, keys)
    };
    let left = side(&mut rng);
    let right = side(&mut rng);
    Binary::new(left, right)
}

/// `k` inputs that each hold every key in `0..2^20 / k` once: every key is in
/// every input, so every key joins.
pub fn dense(k: usize) -> Multi {
    let mut rng = Rng::seeded(&format!("dense/{k}"));
    let keys = (MULTIWAY_ROWS / k) as u64;
    let inputs = (0..k)
        .map(|_| with_payloads(&mut rng, (0..keys).collect()))
        .collect();
    Multi::new(inputs)
}

/// `k` inputs that share only every 1,024th key in `0..2^20`. Each of the
/// rest is in one input, drawn uniformly, so the inputs interleave and only
/// 1,024 keys join.
pub fn sparse(k: usize) -> Multi {
    let mut rng = Rng::seeded(&format!("sparse/{k}"));
    let mut inputs = vec![Vec::new(); k];
    for key in 0..MULTIWAY_ROWS as u64 {
        if key % 1024 == 0 {
            for input in &mut inputs {
                input.push((key, rng.next_u64()));
            }
        } else {
            let input = rng.below(k as u64) as usize;
            inputs[input].push((key, rng.next_u64()));
        }
    }
    Multi::new(inputs)
}

/// Three inputs, in an order that's bad for a binary plan. A and B have `n`
/// rows each, with keys drawn from `n / 12`, so A⋈B is about `12n` pairs. C has
/// `n / 4096` keys from that range, each joining about 144 pairs, so all three
/// join to about `144n / 4096`, roughly `n / 28`.
///
/// Twelve rather than sixteen keeps A⋈B away from a power of two: a binary
/// plan's intermediate Vecs grow by doubling, and an intermediate that hovers
/// at a power of two would double or not on the generator's noise, halving or
/// doubling the plan's peak from one `n` to the next.
pub fn selective(n: usize) -> Multi {
    let mut rng = Rng::seeded(&format!("selective/{n}"));
    let keys = n as u64 / 12;
    let side = |rng: &mut Rng| {
        let keys = (0..n).map(|_| rng.below(keys)).collect();
        sorted(with_payloads(rng, keys))
    };
    let a = side(&mut rng);
    let b = side(&mut rng);
    let mut range: Vec<u64> = (0..keys).collect();
    rng.shuffle(&mut range);
    range.truncate(n / 4096);
    let c = sorted(with_payloads(&mut rng, range));
    Multi::new(vec![a, b, c])
}

/// Gives each key a random payload, in order.
fn with_payloads(rng: &mut Rng, keys: Vec<u64>) -> Vec<Row> {
    keys.into_iter().map(|key| (key, rng.next_u64())).collect()
}

/// `rows` sorted by key.
fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_unstable_by_key(|row| row.0);
    rows
}

/// How many tuples an inner join of `inputs`, each sorted by key, emits: for
/// each key, the product of how many rows have it in each input. It counts
/// runs rather than running any join under test.
fn matches(inputs: &[&[Row]]) -> u64 {
    let counts: Vec<Vec<(u64, u64)>> = inputs.iter().map(|rows| runs(rows)).collect();
    let Some((first, others)) = counts.split_first() else {
        return 0;
    };
    let count = |runs: &[(u64, u64)], key| {
        runs.binary_search_by_key(&key, |&(k, _)| k)
            .map_or(0, |i| runs[i].1)
    };
    first
        .iter()
        .map(|&(key, n)| n * others.iter().map(|runs| count(runs, key)).product::<u64>())
        .sum()
}

/// Each key in sorted `rows`, with how many rows have it.
fn runs(rows: &[Row]) -> Vec<(u64, u64)> {
    rows.chunk_by(|a, b| a.0 == b.0)
        .map(|run| (run[0].0, run.len() as u64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    fn keys(rows: &[Row]) -> BTreeSet<u64> {
        rows.iter().map(|row| row.0).collect()
    }

    /// The keys every input has.
    fn shared(inputs: &[Vec<Row>]) -> BTreeSet<u64> {
        let mut sets = inputs.iter().map(|rows| keys(rows));
        let first = sets.next().unwrap();
        sets.fold(first, |all, set| &all & &set)
    }

    fn within(actual: u64, target: f64, tolerance: f64) -> bool {
        (actual as f64 - target).abs() <= tolerance * target
    }

    #[test]
    fn customers_orders_matches_each_order_once() {
        let n = 1 << 12;
        let w = customers_orders(n);
        let customers = n as u64 / 8;
        let mut left: Vec<u64> = w.left.iter().map(|row| row.0).collect();
        left.sort_unstable();
        assert_eq!(left, (0..customers).collect::<Vec<_>>());
        assert!(w.right.iter().all(|row| row.0 < customers));
        assert_eq!(w.expected, n as u64);
    }

    #[test]
    fn the_same_name_gives_the_same_rows() {
        assert_eq!(customers_orders(1 << 10), customers_orders(1 << 10));
        assert_eq!(sparse(4), sparse(4));
    }

    #[test]
    fn sorted_and_unsorted_hold_the_same_rows() {
        for w in [customers_orders(1 << 12), many_to_many(1 << 12)] {
            let (mut left, mut left_sorted) = (w.left.clone(), w.left_sorted.clone());
            left.sort_unstable();
            left_sorted.sort_unstable();
            assert_eq!(left, left_sorted);
            let (mut right, mut right_sorted) = (w.right.clone(), w.right_sorted.clone());
            right.sort_unstable();
            right_sorted.sort_unstable();
            assert_eq!(right, right_sorted);
        }
    }

    #[test]
    fn payloads_never_repeat() {
        let n = 1 << 16;
        for w in [customers_orders(n), many_to_many(n)] {
            let payloads: HashSet<u64> = w.left.iter().chain(&w.right).map(|row| row.1).collect();
            assert_eq!(payloads.len(), w.left.len() + w.right.len());
        }
    }

    #[test]
    fn dense_inputs_share_every_key() {
        let w = dense(4);
        let all: BTreeSet<u64> = (0..MULTIWAY_ROWS as u64 / 4).collect();
        assert!(w.inputs.iter().all(|rows| keys(rows) == all));
        assert_eq!(w.expected, MULTIWAY_ROWS as u64 / 4);
    }

    #[test]
    fn sparse_inputs_share_only_every_1024th_key() {
        let w = sparse(4);
        let every_1024th: BTreeSet<u64> = (0..MULTIWAY_ROWS as u64).step_by(1024).collect();
        assert_eq!(shared(&w.inputs), every_1024th);
        assert_eq!(w.expected, 1024);
        // Between them, the inputs hold every key.
        let rows: usize = w.inputs.iter().map(Vec::len).sum();
        assert_eq!(rows, MULTIWAY_ROWS + 3 * 1024);
    }

    #[test]
    fn selective_joins_far_more_two_at_a_time_than_all_three() {
        let n = 1 << 16;
        let w = selective(n);
        let a_b = matches(&[&w.inputs[0], &w.inputs[1]]);
        assert!(within(a_b, 12.0 * n as f64, 0.1), "|A⋈B| = {a_b}");
        let output = 144.0 * n as f64 / 4096.0;
        assert!(within(w.expected, output, 0.1), "output {}", w.expected);
    }
}
