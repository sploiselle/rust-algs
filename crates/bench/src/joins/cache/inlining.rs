//! The hash join with the table's insert inlined and with it called: see
//! [`time_inlining`]. A module of its own, declared at the end of the crate,
//! so adding to it moves no line of another measure's code: a panic's line
//! number is in the binary, and a binary that changes reruns.

use std::hash::BuildHasher;
use std::hint::black_box;
use std::time::Instant;

use crate::joins::harness;
use crate::joins::workload;
use crate::joins::{Row, Sink, Tally};
use algs::join::sort_merge_join;
use foldhash::fast::FixedState;
use serde::{Deserialize, Serialize};

use super::{Sentinels, Sides, instructions, median};

/// The hash join with the table's insert inlined into its build loop and
/// with it called, beside sort-merge on the same inputs, at one size.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Inlining {
    pub rows_a_side: usize,
    /// Each join's median, in ns per input row.
    pub sort_merge_ns: f64,
    pub inlined_ns: f64,
    pub called_ns: f64,
    /// Instructions the build loop retires for each row it builds.
    pub inlined_build_instructions: f64,
    pub called_build_instructions: f64,
}

/// What the `inlining` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InliningPart {
    pub sentinels: Sentinels,
    pub inlining: Vec<Inlining>,
}

/// Rows a side [`time_inlining`] runs at: every quadrupling from where the
/// table leaves L2 to the largest inputs.
pub const INLINING_SIZES: [usize; 4] = [1 << 20, 1 << 22, 1 << 24, 1 << 26];

/// The fewest instructions a build row the called insert must add over the
/// inlined one for [`time_inlining`]'s two joins to be what their names say.
/// The call measures 15; nothing but the compiler inlining both, or neither,
/// brings them this close.
pub const CALL_INSTRUCTIONS: f64 = 8.0;

/// A hasher that hashes as `H` does under a type of its own, so the table
/// [`hash_join_inserting`] calls its insert on is one no other code inserts
/// into. The compiler inlines a function called from one place far more
/// readily than one called from two, and with one table type for both joins
/// it inlined the insert into neither.
#[derive(Default)]
pub struct Apart<H>(H);

impl<H: BuildHasher> BuildHasher for Apart<H> {
    type Hasher = H::Hasher;

    #[inline(always)]
    fn build_hasher(&self) -> H::Hasher {
        self.0.build_hasher()
    }
}

/// [`hash_join`](algs::join::hash_join), reaching the table's insert one of
/// two ways. Not `CALLED`, the insert is left to the compiler, as the join
/// itself leaves it, and here the compiler inlines it into the build loop.
/// `CALLED`, it sits behind a function the compiler may not inline, so every
/// build row pays a call: what the compiler does unasked in other binaries.
/// Nothing else differs, and a test holds both to the original. Returns the
/// instructions the build loop retired, allocating the table among them.
#[inline]
pub fn hash_join_inserting<S: Sink, const CALLED: bool>(
    build: &[Row],
    probe: &[Row],
    hasher: impl BuildHasher,
    out: &mut S,
) -> u64 {
    #[inline(never)]
    fn called<'a, H: BuildHasher>(
        table: &mut std::collections::HashMap<&'a u64, &'a Row, H>,
        row: &'a Row,
    ) {
        table.insert(&row.0, row);
    }

    let before = instructions();
    let mut table = std::collections::HashMap::with_capacity_and_hasher(build.len(), hasher);
    for row in build {
        if CALLED {
            called(&mut table, row);
        } else {
            table.insert(&row.0, row);
        }
    }
    let built = instructions();
    for row in probe {
        if let Some(found) = table.get(&row.0) {
            out.pair(found, row);
        }
    }
    built - before
}

/// Times sort-merge and the hash join both ways at `rows_a_side`, on the main
/// case's inputs and its rotation, each for the same time as a criterion case
/// and at least as many samples.
///
/// A sample of the hash join calls it both ways in turn, so whatever the
/// machine is doing, it is doing it to both, and each call's table goes where
/// the last call's was. Sort-merge is sampled on its own, before them: called
/// in turn with the hash joins at 64M rows a side, its two copies of the
/// inputs took the pages the next table had been in, and that join paid for
/// faulting 2 GiB back in, a third more instructions a row.
///
/// The hash join's clock runs over two reads of the instruction count, a few
/// microseconds in calls of milliseconds.
///
/// # Panics
///
/// If a call emits the wrong number of pairs.
#[inline]
pub fn time_inlining(rows_a_side: usize) -> Inlining {
    let instances = harness::rotation(2 * rows_a_side as u64);
    let inputs: Vec<Sides> = (0..instances)
        .map(|i| {
            let workload::Binary {
                left,
                right,
                expected,
                ..
            } = workload::one_to_one(rows_a_side, i);
            Sides {
                left,
                right,
                expected,
            }
        })
        .collect();
    /// `call`, sampled after a call on each instance: each measure's median.
    fn sampled<const N: usize>(instances: usize, mut call: impl FnMut() -> [f64; N]) -> [f64; N] {
        (0..instances).for_each(|_| {
            call();
        });
        let start = Instant::now();
        let mut samples = Vec::new();
        while samples.len() < harness::SAMPLES || start.elapsed() < harness::MEASUREMENT {
            samples.push(call());
        }
        std::array::from_fn(|measure| {
            let mut values: Vec<f64> = samples.iter().map(|s| s[measure]).collect();
            median(&mut values)
        })
    }

    let mut next = inputs.iter().cycle();
    let [sort_merge] = sampled(instances, || {
        let input = next.next().expect("a case has an instance");
        let (left, right) = (black_box(&input.left[..]), black_box(&input.right[..]));
        // Charged for copying both inputs, as the timed case is: an operator
        // has to materialize unsorted input before it can sort it.
        let mut tally = Tally::default();
        let start = Instant::now();
        sort_merge_join(left.to_vec(), right.to_vec(), |l, r| tally.pair(l, r));
        let sort_merge = start.elapsed();
        assert_eq!(tally.tuples, input.expected, "sort-merge at {rows_a_side}");
        [sort_merge.as_nanos() as f64]
    });

    let mut next = inputs.iter().cycle();
    let [inlined, called, inlined_build, called_build] = sampled(instances, || {
        let input = next.next().expect("a case has an instance");
        let (left, right) = (black_box(&input.left[..]), black_box(&input.right[..]));

        let mut tally = Tally::default();
        let start = Instant::now();
        let inlined_build =
            hash_join_inserting::<_, false>(left, right, FixedState::default(), &mut tally);
        let inlined = start.elapsed();
        assert_eq!(tally.tuples, input.expected, "inlined at {rows_a_side}");

        let mut tally = Tally::default();
        let start = Instant::now();
        let called_build =
            hash_join_inserting::<_, true>(left, right, Apart(FixedState::default()), &mut tally);
        let called = start.elapsed();
        assert_eq!(tally.tuples, input.expected, "called at {rows_a_side}");

        [
            inlined.as_nanos() as f64,
            called.as_nanos() as f64,
            inlined_build as f64,
            called_build as f64,
        ]
    });

    let input_rows = (2 * rows_a_side) as f64;
    Inlining {
        rows_a_side,
        sort_merge_ns: sort_merge / input_rows,
        inlined_ns: inlined / input_rows,
        called_ns: called / input_rows,
        inlined_build_instructions: inlined_build / rows_a_side as f64,
        called_build_instructions: called_build / rows_a_side as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::joins::Checksum;
    use algs::join::hash_join;

    #[test]
    fn the_join_matches_the_join_however_it_reaches_the_insert() {
        let input = workload::one_to_one(1 << 12, 0);
        let mut joined = Checksum::default();
        hash_join(&input.left, &input.right, FixedState::default(), |l, r| {
            joined.pair(l, r);
        });
        let (left, right) = (&input.left, &input.right);
        let mut inlined = Checksum::default();
        hash_join_inserting::<_, false>(left, right, FixedState::default(), &mut inlined);
        let mut called = Checksum::default();
        hash_join_inserting::<_, true>(left, right, FixedState::default(), &mut called);
        assert_eq!(joined, inlined);
        assert_eq!(joined, called);
    }

    #[test]
    fn the_inlining_measure_times_all_three_joins() {
        let timed = time_inlining(1 << 10);
        assert_eq!(timed.rows_a_side, 1 << 10);
        for ns in [timed.sort_merge_ns, timed.inlined_ns, timed.called_ns] {
            assert!(ns > 0.0, "{timed:?}");
        }
    }
}
