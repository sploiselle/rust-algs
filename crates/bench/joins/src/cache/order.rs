//! The hash join on rows in no order, in its table's order, and in an order
//! that keeps the table's cache lines together and shuffles them: see
//! [`time_order`]. A module of its own, declared at the end of the crate, so
//! adding to it moves no line of another measure's code.

use std::hash::BuildHasher;
use std::hint::black_box;
use std::time::Instant;

use crate::harness;
use crate::workload;
use crate::{Row, Sink, Tally, mix};
use algs::join::hash_join;
use foldhash::fast::FixedState;
use serde::{Deserialize, Serialize};

use super::{Sentinels, instructions, median};

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

/// Sorts `rows` by where a table of `built` rows starts looking for each
/// one's key: the bucket its hash names. Built and probed in this order, the
/// table is filled and read from one end to the other, and so are the build
/// rows its buckets point at.
///
/// Inline, as everything here is, so that its code is in the `order` binary
/// and no other: a function this library compiled would be in the object
/// every bench target links.
#[inline]
pub fn in_table_order(rows: &mut [Row], built: usize) {
    let hasher = FixedState::default();
    let mask = harness::buckets(built) as u64 - 1;
    rows.sort_unstable_by_key(|row| hasher.hash_one(row.0) & mask);
}

/// Sorts `rows` so that the table's cache lines are each read all at once,
/// as [in its order](in_table_order), but one after another in no order.
///
/// The table is a line of control bytes over sixteen lines of buckets, a
/// control byte a bucket. Rows whose keys start in one line of buckets stay
/// together, in the table's order. The lines of buckets under one line of
/// control bytes stay together too, shuffled among themselves, and so are
/// the lines of control bytes. So the join reads the lines the table's order
/// reads, each as many times, and finds none of them where the last one left
/// off: what the table's order gives a cache is kept, and what it gives a
/// prefetcher is taken away. The build rows are the exception. They're read
/// through the table in the order they're in, which this sorts too, so they
/// still come one after the next.
#[inline]
pub fn in_line_order(rows: &mut [Row], built: usize, line_bytes: usize) {
    let hasher = FixedState::default();
    let mask = harness::buckets(built) as u64 - 1;
    // A bucket without its control byte, which is in a line of its own kind.
    let buckets_a_line = (line_bytes / (harness::BUCKET - 1)) as u64;
    let controls_a_line = line_bytes as u64;
    rows.sort_unstable_by_key(|row| {
        let bucket = hasher.hash_one(row.0) & mask;
        (
            mix(bucket / controls_a_line),
            mix(bucket / buckets_a_line),
            bucket,
        )
    });
}

/// Times the hash join at `rows_a_side` on the main case's inputs three
/// times: as the workload generates them, where every lookup lands somewhere
/// else in the table; with both inputs [in the table's
/// order](in_table_order); and with both [a line of the table at a time, the
/// lines of `line_bytes` shuffled](in_line_order). The rows, the table and
/// the pairs are the same; only the order the join meets them in differs.
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
    let mut inputs: Vec<workload::Binary> = (0..instances)
        .map(|i| workload::one_to_one(rows_a_side, i))
        .collect();

    /// The join's median time and instructions over `inputs`, in turn.
    fn sampled(inputs: &[workload::Binary], rows_a_side: usize) -> (f64, f64) {
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
        in_table_order(&mut input.left, rows_a_side);
        in_table_order(&mut input.right, rows_a_side);
    }
    let (ordered_ns, ordered_instructions) = sampled(&inputs, rows_a_side);
    for input in &mut inputs {
        in_line_order(&mut input.left, rows_a_side, line_bytes);
        in_line_order(&mut input.right, rows_a_side, line_bytes);
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
        in_table_order(&mut left, 1 << 10);
        in_table_order(&mut right, 1 << 10);
        assert_eq!(join(&left, &right), before);
        in_line_order(&mut left, 1 << 10, 128);
        in_line_order(&mut right, 1 << 10, 128);
        assert_eq!(join(&left, &right), before);
    }

    #[test]
    fn line_order_reads_each_line_of_the_table_once_and_not_in_turn() {
        let mut rows = workload::one_to_one(1 << 12, 0).left;
        in_line_order(&mut rows, 1 << 12, 128);
        let hasher = FixedState::default();
        let mask = harness::buckets(1 << 12) as u64 - 1;
        let buckets: Vec<u64> = rows
            .iter()
            .map(|row| hasher.hash_one(row.0) & mask)
            .collect();
        // Each run of rows in one line of buckets, or under one line of
        // control bytes, is the only one in that line.
        for slots in [8, 128] {
            let mut lines: Vec<u64> = buckets.iter().map(|bucket| bucket / slots).collect();
            lines.dedup();
            let runs = lines.len();
            lines.sort_unstable();
            lines.dedup();
            assert_eq!(lines.len(), runs, "{slots} buckets a line");
        }
        // Within a line of buckets the rows are in the table's order, and the
        // lines aren't: fewer than one in eight follows the line before it.
        let lines: Vec<u64> = buckets.iter().map(|bucket| bucket / 8).collect();
        assert!(
            buckets
                .chunk_by(|a, b| a / 8 == b / 8)
                .all(<[u64]>::is_sorted)
        );
        let mut distinct = lines.clone();
        distinct.dedup();
        let in_turn = distinct.windows(2).filter(|w| w[1] == w[0] + 1).count();
        assert!(
            8 * in_turn < distinct.len(),
            "{in_turn} of {}",
            distinct.len()
        );
    }

    #[test]
    fn the_tables_order_runs_from_its_first_bucket_to_its_last() {
        let mut rows = workload::one_to_one(1 << 10, 0).left;
        in_table_order(&mut rows, 1 << 10);
        let hasher = FixedState::default();
        let mask = harness::buckets(1 << 10) as u64 - 1;
        let buckets: Vec<u64> = rows
            .iter()
            .map(|row| hasher.hash_one(row.0) & mask)
            .collect();
        assert!(buckets.is_sorted());
        assert!(buckets[0] < buckets[buckets.len() - 1]);
    }

    #[test]
    fn the_order_measure_times_the_join_both_ways() {
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
