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

/// Rows on the large side of [`small_side`], however many the small side has.
const LARGE_SIDE: usize = 1 << 16;

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
        let expected = matches(&sorted);
        Self { inputs, expected }
    }
}

/// Two inputs of `n` rows that share half their keys: `n / 2` keys in both,
/// and `n / 2` more in each alone, from `0..3n / 2`, each input shuffled. A
/// shared key joins to one pair, so they join to `n / 2` pairs.
pub fn one_to_one(n: usize) -> Binary {
    let mut rng = Rng::seeded(&format!("one_to_one/{n}"));
    let (mut left, mut right) = two_sides(&mut rng, n, n / 2);
    rng.shuffle(&mut left);
    rng.shuffle(&mut right);
    let left = with_payloads(&mut rng, left);
    let right = with_payloads(&mut rng, right);
    Binary::new(left, right)
}

/// `m` rows against 65,536, each of the `m` keys one of the 65,536, both
/// sides shuffled: so few rows on one side that a nested loop compares each
/// row of the other only `m` times, where a hash join looks each one up once.
pub fn small_side(m: usize) -> Binary {
    let mut rng = Rng::seeded(&format!("small_side/{m}"));
    let mut keys: Vec<u64> = (0..LARGE_SIDE as u64).collect();
    rng.shuffle(&mut keys);
    let small = with_payloads(&mut rng, keys[..m].to_vec());
    // Again, so the small side's keys aren't the first the large side holds.
    rng.shuffle(&mut keys);
    let large = with_payloads(&mut rng, keys);
    Binary::new(small, large)
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
/// rows each and share `3n / 4` keys, so A⋈B is `3n / 4` pairs. C has
/// `n / 4096` of those keys, so all three join to `n / 4096` tuples, one for
/// each row of C.
///
/// Every size is exact rather than drawn, so where A⋈B falls against the
/// powers of two a binary plan's Vecs grow through, halfway from `n / 2` to
/// `n`, doesn't change from one `n` to the next.
pub fn selective(n: usize) -> Multi {
    let mut rng = Rng::seeded(&format!("selective/{n}"));
    let (a, b) = two_sides(&mut rng, n, 3 * n / 4);
    // A's keys start with the shared ones, in a random order.
    let c = sorted(with_payloads(&mut rng, a[..n / 4096].to_vec()));
    let a = sorted(with_payloads(&mut rng, a));
    let b = sorted(with_payloads(&mut rng, b));
    Multi::new(vec![a, b, c])
}

/// Keys for two inputs of `n` rows that share `shared` of them, from
/// `0..2n - shared`: the shared keys, in a random order, then each input's
/// own.
fn two_sides(rng: &mut Rng, n: usize, shared: usize) -> (Vec<u64>, Vec<u64>) {
    let mut keys: Vec<u64> = (0..(2 * n - shared) as u64).collect();
    rng.shuffle(&mut keys);
    let (both, own) = keys.split_at(shared);
    let (left, right) = own.split_at(n - shared);
    ([both, left].concat(), [both, right].concat())
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

/// How many tuples an inner join of `inputs` emits: how many keys of the
/// first every other input holds. It looks each one up rather than running
/// any join under test.
///
/// # Panics
///
/// If an input isn't sorted by key, or holds a key twice: the joins take
/// neither.
fn matches(inputs: &[&[Row]]) -> u64 {
    for rows in inputs {
        assert!(
            rows.is_sorted_by(|a, b| a.0 < b.0),
            "an input's keys don't strictly increase"
        );
    }
    let Some((first, others)) = inputs.split_first() else {
        return 0;
    };
    let held = |rows: &[Row], key| rows.binary_search_by_key(&key, |row| row.0).is_ok();
    first
        .iter()
        .filter(|row| others.iter().all(|rows| held(rows, row.0)))
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    fn keys(rows: &[Row]) -> BTreeSet<u64> {
        rows.iter().map(|row| row.0).collect()
    }

    /// Whether no two of `rows` share a key.
    fn unique(rows: &[Row]) -> bool {
        keys(rows).len() == rows.len()
    }

    /// The keys every input has.
    fn shared(inputs: &[Vec<Row>]) -> BTreeSet<u64> {
        let mut sets = inputs.iter().map(|rows| keys(rows));
        let first = sets.next().unwrap();
        sets.fold(first, |all, set| &all & &set)
    }

    #[test]
    fn one_to_one_shares_half_its_keys() {
        let n = 1 << 12;
        let w = one_to_one(n);
        assert!(unique(&w.left) && unique(&w.right));
        assert_eq!((w.left.len(), w.right.len()), (n, n));
        let both = shared(&[w.left.clone(), w.right.clone()]);
        assert_eq!(both.len(), n / 2);
        assert_eq!(w.expected, n as u64 / 2);
        // Shuffled: the shared keys aren't bunched at either end.
        let first_half = w.left[..n / 2].iter().filter(|row| both.contains(&row.0));
        let count = first_half.count();
        assert!(
            count.abs_diff(n / 4) < n / 16,
            "{count} shared keys in the first half"
        );
    }

    #[test]
    fn small_side_matches_every_key_once() {
        for m in [1, 2, 4, 16, 64] {
            let w = small_side(m);
            assert!(unique(&w.left) && unique(&w.right));
            assert_eq!((w.left.len(), w.right.len()), (m, LARGE_SIDE));
            assert!(keys(&w.left).is_subset(&keys(&w.right)));
            assert_eq!(w.expected, m as u64);
        }
    }

    #[test]
    fn the_same_name_gives_the_same_rows() {
        assert_eq!(one_to_one(1 << 10), one_to_one(1 << 10));
        assert_eq!(sparse(4), sparse(4));
    }

    #[test]
    fn sorted_and_unsorted_hold_the_same_rows() {
        for w in [one_to_one(1 << 12), small_side(64)] {
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
        for w in [one_to_one(1 << 16), small_side(64)] {
            let payloads: HashSet<u64> = w.left.iter().chain(&w.right).map(|row| row.1).collect();
            assert_eq!(payloads.len(), w.left.len() + w.right.len());
        }
    }

    #[test]
    fn dense_inputs_share_every_key() {
        let w = dense(4);
        let all: BTreeSet<u64> = (0..MULTIWAY_ROWS as u64 / 4).collect();
        assert!(
            w.inputs
                .iter()
                .all(|rows| unique(rows) && keys(rows) == all)
        );
        assert_eq!(w.expected, MULTIWAY_ROWS as u64 / 4);
    }

    #[test]
    fn sparse_inputs_share_only_every_1024th_key() {
        let w = sparse(4);
        assert!(w.inputs.iter().all(|rows| unique(rows)));
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
        assert!(w.inputs.iter().all(|rows| unique(rows)));
        let lens: Vec<usize> = w.inputs.iter().map(Vec::len).collect();
        assert_eq!(lens, [n, n, n / 4096]);
        assert_eq!(matches(&[&w.inputs[0], &w.inputs[1]]), 3 * n as u64 / 4);
        assert_eq!(w.expected, n as u64 / 4096);
    }

    #[test]
    #[should_panic(expected = "an input's keys don't strictly increase")]
    fn refuses_an_input_that_repeats_a_key() {
        matches(&[&[(1, 10), (1, 11)], &[(1, 20)]]);
    }
}
