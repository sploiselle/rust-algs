//! What every timing binary shares: how a case is sampled, how its inputs
//! rotate, the inputs made ready to join, and the check a join has to pass
//! before it posts a number. A change here changes every timing binary, and
//! reruns them all.

use std::fmt;
use std::time::Duration;

use crate::{Checksum, Row, workload};

/// How long criterion warms each case up.
pub const WARM_UP: Duration = Duration::from_secs(1);
/// How long criterion measures each case.
pub const MEASUREMENT: Duration = Duration::from_secs(3);
/// Samples per case. Every case is sampled flat, each sample running the
/// same number of iterations: the report reads only the median, which flat
/// sampling gives, and one mode for every case keeps confidence intervals
/// comparable across reruns. A sample runs at least one iteration, so a case
/// whose call takes longer than [`MEASUREMENT`] over this many samples takes
/// this many calls instead.
pub const SAMPLES: usize = 30;

/// The fewest input rows a timed case's instances hold before one repeats.
/// On one input, a branchy join's comparisons come out the same way every
/// call, and the branch predictor learns them, so a join timed over and over
/// on one input runs faster than it would on fresh input.
///
/// Rotating takes away what the predictor learns, and reuse from a cache too
/// small to hold a cycle, but not from one that holds it: a join that reads
/// every row reads 16 MiB of them a cycle. The instances hold more than a
/// cycle reads: a binary workload keeps every row twice, as generated and
/// sorted, so a small binary case's hold about 32 MiB, where one instance at
/// 1K rows a side holds 64 KiB; the selective workload's hold about 16 MiB.
/// At a few rows a side an instance's four Vecs outweigh its rows, so a cycle
/// of `both_small`'s spans more still.
///
/// Leapfrog reads about 0.5% of the selective workload's rows, so a cycle
/// reads only 5K to 12K of them, on about 1 MiB of cache lines or less, which
/// stay in cache however many instances they're spread over.
pub const ROTATION_ROWS: u64 = 1 << 20;

/// How many instances a timed case rotates through when each holds `rows`
/// input rows: enough to hold [`ROTATION_ROWS`] in one cycle, and one if a
/// single instance does.
pub fn rotation(rows: u64) -> usize {
    ROTATION_ROWS.div_ceil(rows).max(1) as usize
}

/// The instances a timed case at `param` rotates through: as many as
/// [`rotation`] says, the first being `generate(param, 0)`.
///
/// # Panics
///
/// If an instance's inputs have other row counts than the first's, or join
/// to another count: every call has to do the same work over the same rows,
/// which the throughput and the per-row times assume.
/// [`workload::sparse`] draws its inputs' sizes, but holds at least
/// [`ROTATION_ROWS`] rows, so it has one instance.
pub fn instances(generate: fn(usize, usize) -> Data, param: usize) -> Vec<Data> {
    let first = generate(param, 0);
    let count = rotation(first.input_rows());
    let mut all = Vec::with_capacity(count);
    all.push(first);
    for instance in 1..count {
        let data = generate(param, instance);
        assert_eq!(
            data.shape(),
            all[0].shape(),
            "at {param}: instance {instance}'s row counts differ from instance 0's"
        );
        all.push(data);
    }
    all
}

/// Checks what a case emitted: as many tuples as an independent count of the
/// workload says, and the checksum the group's first algorithm emitted on the
/// same inputs, which it stores in `agreed`.
///
/// # Panics
///
/// If either differs, so a wrong join can't post a number.
pub fn verify(case: &dyn fmt::Display, data: &Data, out: Checksum, agreed: &mut Option<Checksum>) {
    assert_eq!(
        out.rows,
        data.expected(),
        "{case} emitted the wrong number of tuples"
    );
    let first = *agreed.get_or_insert(out);
    assert_eq!(
        out, first,
        "{case} disagrees with its group's first algorithm"
    );
}

/// A workload's inputs.
pub enum Data {
    Binary(workload::Binary),
    Multi(workload::Multi),
}

impl From<workload::Binary> for Data {
    fn from(inputs: workload::Binary) -> Self {
        Data::Binary(inputs)
    }
}

impl From<workload::Multi> for Data {
    fn from(inputs: workload::Multi) -> Self {
        Data::Multi(inputs)
    }
}

impl Data {
    /// Rows in all the inputs, not counting sorted copies.
    pub fn input_rows(&self) -> u64 {
        let rows = match self {
            Data::Binary(w) => w.left.len() + w.right.len(),
            Data::Multi(w) => w.inputs.iter().map(Vec::len).sum(),
        };
        rows as u64
    }

    /// Bytes in all the inputs, not counting sorted copies.
    pub fn input_bytes(&self) -> u64 {
        self.input_rows() * size_of::<Row>() as u64
    }

    /// How many tuples a join of the inputs emits.
    pub fn expected(&self) -> u64 {
        match self {
            Data::Binary(w) => w.expected,
            Data::Multi(w) => w.expected,
        }
    }

    /// Each input's rows, and how many tuples they join to.
    pub fn shape(&self) -> (Vec<usize>, u64) {
        let rows = match self {
            Data::Binary(w) => vec![w.left.len(), w.right.len()],
            Data::Multi(w) => w.inputs.iter().map(Vec::len).collect(),
        };
        (rows, self.expected())
    }

    /// Makes the inputs ready to join.
    pub fn prepare(&self) -> Prepared<'_> {
        match self {
            Data::Binary(w) => Prepared::Binary(w),
            Data::Multi(w) => {
                let inputs: Vec<&[Row]> = w.inputs.iter().map(Vec::as_slice).collect();
                let mut order: Vec<usize> = (0..inputs.len()).collect();
                order.sort_by_key(|&i| inputs[i].len());
                let planned = order.iter().map(|&i| inputs[i]).collect();
                let mut position = vec![0; order.len()];
                for (slot, &input) in order.iter().enumerate() {
                    position[input] = slot;
                }
                Prepared::Multi {
                    inputs,
                    planned,
                    position,
                }
            }
        }
    }
}

/// A workload made ready to join, before any timing or measuring, as an engine
/// plans before it runs.
pub enum Prepared<'w> {
    Binary(&'w workload::Binary),
    Multi {
        /// The inputs, in the workload's order.
        inputs: Vec<&'w [Row]>,
        /// The inputs smallest first, keeping the workload's order among equals.
        planned: Vec<&'w [Row]>,
        /// Where each input went in `planned`: input i is `planned[position[i]]`.
        position: Vec<usize>,
    },
}

/// Bytes per bucket in the hash join's table: a `(&key, &row)` entry, and
/// hashbrown's control byte.
pub const BUCKET: usize = size_of::<(&u64, &Row)>() + 1;

/// How many buckets hashbrown gives a table with room for `rows`: at least 4,
/// and a power of two at most 7/8 full.
pub fn buckets(rows: usize) -> usize {
    match rows {
        0..4 => 4,
        4..8 => 8,
        _ => (rows * 8 / 7).next_power_of_two(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cycle_holds_at_least_rotation_rows_and_no_more_instances_than_that_takes() {
        assert_eq!(rotation(2 << 10), 512);
        assert_eq!(rotation((1 << 16) + 4), 16);
        assert_eq!(rotation(1 << 20), 1);
        assert_eq!(rotation(2 << 20), 1);
        let all = instances(|n, i| workload::one_to_one(n, i).into(), 1 << 10);
        assert_eq!(all.len(), 512);
        assert!(all[0].input_rows() * all.len() as u64 >= ROTATION_ROWS);
    }
}
