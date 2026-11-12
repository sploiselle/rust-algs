//! The hash join on rows in no order, in its table's order, and in an order
//! that keeps each line of the table's control bytes together and shuffles
//! the lines: see [`time_order`]. A module of its own, declared at the end of
//! the crate, so adding to it moves no line of another measure's code.

use std::hash::BuildHasher;
use std::hint::black_box;
use std::time::Instant;

use crate::harness;
use crate::workload;
use crate::{Row, Sink, Tally, mix};
use algs::join::hash_join;
use foldhash::fast::FixedState;
use serde::{Deserialize, Serialize};

use super::{Sentinels, Sides, instructions, median};

/// The hash join on the same rows in three orders, at one size.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Order {
    pub rows_a_side: usize,
    /// The join's median on the rows as the workload generates them, in ns
    /// per input row.
    pub random_ns: f64,
    /// And on the same rows [in the table's order](in_table_order).
    pub ordered_ns: f64,
    /// And [a line of the table at a time, the lines shuffled](in_line_order).
    pub lined_ns: f64,
    /// Instructions the join retires for each input row, each way.
    pub random_instructions: f64,
    pub ordered_instructions: f64,
    pub lined_instructions: f64,
}

/// What the `order` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OrderPart {
    pub sentinels: Sentinels,
    pub order: Vec<Order>,
}

/// Rows a side [`time_order`] runs at: every quadrupling from a table that
/// fits in L2 to the largest inputs.
pub const ORDER_SIZES: [usize; 6] = [1 << 16, 1 << 18, 1 << 20, 1 << 22, 1 << 24, 1 << 26];

/// Passes a line's rows are taken in: see [`arranged`].
const PASSES: usize = 8;

/// Sorts `rows` by the line of control bytes a table of `built` rows starts
/// looking for each one's key in, the lines in the order of what `place`
/// makes of them, and takes each line's rows in [`PASSES`] passes up the
/// line, every eighth row a pass.
///
/// The table is a control byte a bucket, so a line of `line_bytes` control
/// bytes stands over as many buckets: sixteen lines of them. A lookup starts
/// at the bucket its key's hash names.
///
/// The passes keep the build loop's inserts out of each other's way. An
/// insert loads the eight control bytes from its key's bucket on and stores
/// one of them, and that load waits when one of the three or so inserts
/// before it stored a control byte within a group of those, to either side.
/// Inserted in the order of their buckets, most rows wait on the row before,
/// and with its table in cache the build loop takes 2.4 ns an input row at
/// 64K rows a side where it takes 1.0 in no order: that's the order's doing
/// and none of the cache's. In eight passes a pass's rows are sixteen buckets
/// apart or so, and the row next up the line is a pass later, eight inserts
/// or so, and the build loop takes 1.0. Two passes or four leave part of the
/// wait, and so do thirty-two, which bring that next row two inserts later.
#[inline]
fn arranged(rows: &mut [Row], built: usize, line_bytes: usize, place: impl Fn(u64) -> u64) {
    let hasher = FixedState::default();
    let mask = harness::buckets(built) as u64 - 1;
    let buckets_a_line = line_bytes as u64;
    let bucket = |row: &Row| hasher.hash_one(row.0) & mask;
    rows.sort_unstable_by_key(|row| {
        let bucket = bucket(row);
        (place(bucket / buckets_a_line), bucket)
    });
    let mut held = Vec::new();
    for line in rows.chunk_by_mut(|a, b| bucket(a) / buckets_a_line == bucket(b) / buckets_a_line) {
        held.clear();
        held.extend_from_slice(line);
        let passes = (0..PASSES).flat_map(|pass| held.iter().skip(pass).step_by(PASSES));
        for (row, &next) in line.iter_mut().zip(passes) {
            *row = next;
        }
    }
}

/// Sorts `rows` into the order of a table of `built` rows: a line of its
/// control bytes at a time, from the table's first to its last, each line's
/// rows [in passes](arranged). Built and probed in this order, the table is
/// filled and read from one end to the other, and so are the build rows its
/// buckets point at.
///
/// Inline, as everything here is, so that its code is in the `order` binary
/// and no other: a function this library compiled would be in the object
/// every bench target links.
#[inline]
pub fn in_table_order(rows: &mut [Row], built: usize, line_bytes: usize) {
    arranged(rows, built, line_bytes, |line| line);
}

/// Sorts `rows` as [the table's order](in_table_order) does, with the lines
/// of control bytes in no order: `shuffle` picks which.
///
/// The join reads the lines the table's order reads, each as many times and
/// with the same rows together, and doesn't find a line of control bytes, or
/// the sixteen lines of buckets under it, where the last left off: what the
/// table's order gives a cache is kept, and what it gives a prefetcher from
/// one line of control bytes to the next is taken away. Give the two sides
/// different shuffles and the build rows are read the same way, a line of
/// the table's worth together and those in no order. Under one line of
/// control bytes the passes still run up the buckets, as they do in the
/// table's order.
#[inline]
pub fn in_line_order(rows: &mut [Row], built: usize, line_bytes: usize, shuffle: u64) {
    arranged(rows, built, line_bytes, |line| mix(line ^ shuffle));
}

/// The shuffles [`time_order`] gives the lines of each side.
const SHUFFLES: (u64, u64) = (0, 0x5555_5555_5555_5555);

/// Times the hash join at `rows_a_side` on the main case's inputs three
/// times: as the workload generates them, where every lookup lands somewhere
/// else in the table; with both inputs [in the table's
/// order](in_table_order); and with both [a line of the table's control bytes
/// at a time, the lines of `line_bytes` shuffled](in_line_order), each side
/// its own way. The rows, the table and the pairs are the same; only the
/// order the join meets them in differs.
///
/// Each order is sampled for the same time as a criterion case and at least
/// as many samples, one order after another: at the largest size
/// there isn't the memory to hold two at once. The clock runs over two
/// reads of the instruction count, a few microseconds in calls of
/// milliseconds.
///
/// # Panics
///
/// If a call emits the wrong number of pairs.
#[inline]
pub fn time_order(rows_a_side: usize, line_bytes: usize) -> Order {
    let instances = harness::rotation(2 * rows_a_side as u64);
    // Only the sides: at 64M rows a side the workload's sorted copies would
    // hold 2 GiB more, enough to page.
    let mut inputs: Vec<Sides> = (0..instances)
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

    /// The join's median time and instructions over `inputs`, in turn.
    fn sampled(inputs: &[Sides], rows_a_side: usize) -> (f64, f64) {
        let mut next = inputs.iter().cycle();
        let mut call = || {
            let input = next.next().expect("a case has an instance");
            let (left, right) = (black_box(&input.left[..]), black_box(&input.right[..]));
            let mut tally = Tally::default();
            let start = Instant::now();
            let before = instructions();
            hash_join(left, right, FixedState::default(), |l, r| tally.pair(l, r));
            let retired = instructions() - before;
            let took = start.elapsed();
            assert_eq!(tally.tuples, input.expected, "the join at {rows_a_side}");
            (took.as_nanos() as f64, retired as f64)
        };
        (0..inputs.len()).for_each(|_| {
            call();
        });
        let start = Instant::now();
        let mut samples = Vec::new();
        while samples.len() < harness::SAMPLES || start.elapsed() < harness::MEASUREMENT {
            samples.push(call());
        }
        let mut ns: Vec<f64> = samples.iter().map(|sample| sample.0).collect();
        let mut retired: Vec<f64> = samples.iter().map(|sample| sample.1).collect();
        (median(&mut ns), median(&mut retired))
    }

    let (random_ns, random_instructions) = sampled(&inputs, rows_a_side);
    for input in &mut inputs {
        in_table_order(&mut input.left, rows_a_side, line_bytes);
        in_table_order(&mut input.right, rows_a_side, line_bytes);
    }
    let (ordered_ns, ordered_instructions) = sampled(&inputs, rows_a_side);
    for input in &mut inputs {
        in_line_order(&mut input.left, rows_a_side, line_bytes, SHUFFLES.0);
        in_line_order(&mut input.right, rows_a_side, line_bytes, SHUFFLES.1);
    }
    let (lined_ns, lined_instructions) = sampled(&inputs, rows_a_side);

    let input_rows = (2 * rows_a_side) as f64;
    Order {
        rows_a_side,
        random_ns: random_ns / input_rows,
        ordered_ns: ordered_ns / input_rows,
        lined_ns: lined_ns / input_rows,
        random_instructions: random_instructions / input_rows,
        ordered_instructions: ordered_instructions / input_rows,
        lined_instructions: lined_instructions / input_rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Checksum;

    /// The bucket each of `rows`' lookups starts at in a table of `built`.
    fn buckets(rows: &[Row], built: usize) -> Vec<u64> {
        let hasher = FixedState::default();
        let mask = harness::buckets(built) as u64 - 1;
        rows.iter()
            .map(|row| hasher.hash_one(row.0) & mask)
            .collect()
    }

    #[test]
    fn the_tables_order_keeps_every_row_and_every_pair() {
        let workload::Binary {
            mut left,
            mut right,
            expected,
            ..
        } = workload::one_to_one(1 << 10, 0);
        let join = |left: &[Row], right: &[Row]| {
            let (mut sum, mut tally) = (Checksum::default(), Tally::default());
            hash_join(left, right, FixedState::default(), |l, r| {
                sum.pair(l, r);
                tally.pair(l, r);
            });
            (tally.tuples, sum.rows, sum.sum)
        };
        let before = join(&left, &right);
        assert_eq!(before.0, expected);
        in_table_order(&mut left, 1 << 10, 128);
        in_table_order(&mut right, 1 << 10, 128);
        assert_eq!(join(&left, &right), before);
        in_line_order(&mut left, 1 << 10, 128, SHUFFLES.0);
        in_line_order(&mut right, 1 << 10, 128, SHUFFLES.1);
        assert_eq!(join(&left, &right), before);
    }

    #[test]
    fn the_tables_order_runs_from_its_first_line_to_its_last() {
        let mut rows = workload::one_to_one(1 << 12, 0).left;
        in_table_order(&mut rows, 1 << 12, 128);
        let lines: Vec<u64> = buckets(&rows, 1 << 12).iter().map(|b| b / 128).collect();
        assert!(lines.is_sorted());
        assert_eq!((lines[0], lines[lines.len() - 1]), (0, 63));
    }

    #[test]
    fn a_lines_rows_come_in_passes_up_it_every_eighth_row_a_pass() {
        let mut rows = workload::one_to_one(1 << 12, 0).left;
        in_table_order(&mut rows, 1 << 12, 128);
        let buckets = buckets(&rows, 1 << 12);
        for line in buckets.chunk_by(|a, b| a / 128 == b / 128) {
            let mut up = line.to_vec();
            up.sort_unstable();
            let passes: Vec<u64> = (0..PASSES)
                .flat_map(|pass| up.iter().skip(pass).step_by(PASSES).copied())
                .collect();
            assert_eq!(line, passes);
        }
    }

    #[test]
    fn line_order_reads_each_line_of_control_bytes_once_and_not_in_turn() {
        let lines = |shuffle| {
            let mut rows = workload::one_to_one(1 << 12, 0).left;
            in_line_order(&mut rows, 1 << 12, 128, shuffle);
            let mut lines: Vec<u64> = buckets(&rows, 1 << 12).iter().map(|b| b / 128).collect();
            lines.dedup();
            lines
        };
        let (built, probed) = (lines(SHUFFLES.0), lines(SHUFFLES.1));
        for lines in [&built, &probed] {
            // Each run of rows under one line of control bytes is the only
            // one under that line, and fewer than one line in eight follows
            // the line before it.
            let mut distinct = lines.clone();
            distinct.sort_unstable();
            distinct.dedup();
            assert_eq!(distinct.len(), lines.len());
            let in_turn = lines.windows(2).filter(|w| w[1] == w[0] + 1).count();
            assert!(8 * in_turn < lines.len(), "{in_turn} of {}", lines.len());
        }
        // The two sides' lines come in different orders.
        let together = built.iter().zip(&probed).filter(|(a, b)| a == b).count();
        assert!(8 * together < built.len(), "{together} of {}", built.len());
    }

    #[test]
    fn line_order_takes_a_lines_rows_as_the_tables_order_does() {
        let (mut tabled, mut lined) = (
            workload::one_to_one(1 << 12, 0).left,
            workload::one_to_one(1 << 12, 0).left,
        );
        in_table_order(&mut tabled, 1 << 12, 128);
        in_line_order(&mut lined, 1 << 12, 128, SHUFFLES.1);
        let by_line = |rows: &[Row]| {
            let buckets = buckets(rows, 1 << 12);
            let mut runs: Vec<Vec<u64>> = buckets
                .chunk_by(|a, b| a / 128 == b / 128)
                .map(<[u64]>::to_vec)
                .collect();
            runs.sort_unstable();
            runs
        };
        assert_eq!(by_line(&tabled), by_line(&lined));
    }

    #[test]
    fn the_order_measure_times_the_join_each_way() {
        let order = time_order(1 << 10, 128);
        assert!(order.random_ns > 0.0 && order.ordered_ns > 0.0 && order.lined_ns > 0.0);
        #[cfg(target_os = "macos")]
        assert!(
            order.random_instructions > 0.0
                && order.ordered_instructions > 0.0
                && order.lined_instructions > 0.0
        );
    }
}
