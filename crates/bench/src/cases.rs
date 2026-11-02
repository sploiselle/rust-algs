//! The plan: every workload, the parameters it runs at, and the algorithms run
//! on it. The timing target, the memory harness and the report all read
//! [`GROUPS`], so every case that's timed is also profiled, and the tables cite
//! only cases that ran.
//!
//! Each workload makes one tradeoff visible:
//!
//! - `one_to_one` is the main case: two inputs of the same size that share
//!   half their keys, from 1K to 4M rows a side.
//! - `small_side` is the one regime where a nested loop can win.
//! - `hash_variants` shows what the hasher costs.
//! - `multiway_dense` against `multiway_sparse` shows when a heap pays against
//!   a scan of every input's next key.
//! - `multiway_selective` shows how much a binary plan's join order matters,
//!   and what skipping saves.
//!
//! A timed case rotates through [`instances`](Workload::instances) of its
//! input: the same workload at the same parameter from different seeds, with
//! the same row counts, holding at least [`ROTATION_ROWS`] rows before one
//! repeats, so the branch predictor learns little of any one. A case whose
//! input holds that many rows has one instance. [`ROTATION_ROWS`] says what
//! the rotation leaves in cache, and why it does little for leapfrog on the
//! selective workload. The memory harness profiles each case's first instance
//! alone.
//!
//! Every instance is checked with [`verify`] before it is timed, and the first
//! before it is measured.

use std::fmt;
use std::hash::{BuildHasherDefault, DefaultHasher};
use std::time::Duration;

use algs::join::{
    cascade_merge_join, hash_join, leapfrog_join, merge_join, multiway_merge_join,
    nested_loop_join, sort_merge_join,
};
use foldhash::fast::FixedState;

use crate::alloc::Usage;
use crate::{Checksum, Row, Sink, baseline, workload};

/// The input sizes of the binary groups: 1K to 4M rows, by fours.
pub const SIZES: [usize; 7] = [
    1 << 10,
    1 << 12,
    1 << 14,
    1 << 16,
    1 << 18,
    1 << 20,
    1 << 22,
];

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

/// A join, and the inputs it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    /// `nested_loop_join` on equal keys.
    NestedLoop,
    /// `hash_join`, building the left input, with foldhash.
    Hash,
    /// The same on both inputs sorted by key. The table's buckets are still
    /// read in hash order, but each probe's match sits further on in the
    /// build input than the last one's, so the build rows the table points
    /// into are read in address order. That removes the cost of the inputs
    /// being in different orders, leaving the table's own.
    HashSorted,
    /// The same with std's hasher, SipHash-1-3, with fixed keys.
    HashSipHash,
    /// `merge_join` on inputs sorted before timing, standing for an index.
    Merge,
    /// `sort_merge_join`, charged for copying both inputs: an operator has to
    /// materialize unsorted input before it can sort it.
    SortMerge,
    /// `multiway_merge_join`.
    Heap,
    /// [`baseline::multiway_merge_join_by_scan`].
    Scan,
    /// `leapfrog_join`.
    Leapfrog,
    /// `cascade_merge_join` on the inputs smallest first, as a planner with
    /// estimates orders them.
    CascadePlanned,
    /// `cascade_merge_join` on the inputs as given.
    CascadeUnplanned,
}

impl Algorithm {
    /// Its name in benchmark ids, such as `hash_sorted`.
    pub fn name(self) -> &'static str {
        match self {
            Algorithm::NestedLoop => "nested_loop",
            Algorithm::Hash => "hash",
            Algorithm::HashSorted => "hash_sorted",
            Algorithm::HashSipHash => "hash_siphash",
            Algorithm::Merge => "merge",
            Algorithm::SortMerge => "sort_merge",
            Algorithm::Heap => "heap",
            Algorithm::Scan => "scan",
            Algorithm::Leapfrog => "leapfrog",
            Algorithm::CascadePlanned => "cascade_planned",
            Algorithm::CascadeUnplanned => "cascade_unplanned",
        }
    }

    /// Its name in a table, such as `hash (sorted input)`.
    pub fn label(self) -> &'static str {
        match self {
            Algorithm::NestedLoop => "nested loop",
            Algorithm::Hash => "hash",
            Algorithm::HashSorted => "hash (sorted input)",
            Algorithm::HashSipHash => "hash, SipHash",
            Algorithm::Merge => "merge (sorted input)",
            Algorithm::SortMerge => "sort-merge",
            Algorithm::Heap => "heap",
            Algorithm::Scan => "scan",
            Algorithm::Leapfrog => "leapfrog",
            Algorithm::CascadePlanned => "binary plan (smallest first)",
            Algorithm::CascadeUnplanned => "binary plan (as given)",
        }
    }

    /// Whether it runs at `param`. The nested loop is quadratic: at 64K rows
    /// a side it takes about 1 s a call, and past that it would take most of
    /// the run.
    pub fn runs_at(self, param: usize) -> bool {
        match self {
            Algorithm::NestedLoop => param <= 1 << 16,
            _ => true,
        }
    }

    /// Runs the join on `data`, adding everything it emits to `out`, a
    /// checksum.
    ///
    /// # Panics
    ///
    /// If `data` has the wrong number of inputs for it.
    pub fn run(self, data: &Prepared, out: &mut Checksum) {
        self.run_into(data, out);
    }

    /// Runs the join on `data`, handing everything it emits to `out`.
    ///
    /// # Panics
    ///
    /// If `data` has the wrong number of inputs for it.
    pub fn run_into<S: Sink>(self, data: &Prepared, out: &mut S) {
        use Algorithm::*;
        let fold = FixedState::default();
        let sip = BuildHasherDefault::<DefaultHasher>::default();
        let pair = |l: &Row, r: &Row| out.pair(l, r);
        match (self, data) {
            (NestedLoop, Prepared::Binary(w)) => nested_loop_join(&w.left, &w.right, pair),
            (Hash, Prepared::Binary(w)) => hash_join(&w.left, &w.right, fold, pair),
            (HashSorted, Prepared::Binary(w)) => {
                hash_join(&w.left_sorted, &w.right_sorted, fold, pair);
            }
            (HashSipHash, Prepared::Binary(w)) => hash_join(&w.left, &w.right, sip, pair),
            (Merge, Prepared::Binary(w)) => merge_join(&w.left_sorted, &w.right_sorted, pair),
            (SortMerge, Prepared::Binary(w)) => {
                sort_merge_join(w.left.to_vec(), w.right.to_vec(), pair);
            }
            (Heap, Prepared::Multi { inputs, .. }) => multiway_merge_join(inputs, |t| out.tuple(t)),
            (Scan, Prepared::Multi { inputs, .. }) => {
                baseline::multiway_merge_join_by_scan(inputs, |t| out.tuple(t));
            }
            (Leapfrog, Prepared::Multi { inputs, .. }) => leapfrog_join(inputs, |t| out.tuple(t)),
            (CascadeUnplanned, Prepared::Multi { inputs, .. }) => {
                cascade_merge_join(inputs, |t| out.tuple(t));
            }
            // Its tuples come in planned order, so put each row back in its input's slot.
            (
                CascadePlanned,
                Prepared::Multi {
                    planned, position, ..
                },
            ) => {
                cascade_merge_join(planned, |t| out.tuple_permuted(t, position));
            }
            (alg, _) => panic!("{} can't run on this workload", alg.name()),
        }
    }
}

/// What a group's inputs are, each generated from a parameter.
#[derive(Clone, Copy, Debug)]
pub enum Workload {
    /// [`workload::one_to_one`], at `n` rows a side.
    OneToOne,
    /// [`workload::small_side`], at `m` rows on the small side.
    SmallSide,
    /// [`workload::dense`], at `k` inputs.
    Dense,
    /// [`workload::sparse`], at `k` inputs.
    Sparse,
    /// [`workload::selective`], at `n` rows in each of the two large inputs.
    Selective,
}

impl Workload {
    /// Which rows its input rows are, for a caption: per input row, a time
    /// counts every input, not just the one a table's rows are labeled by.
    pub fn input_rows(self) -> &'static str {
        match self {
            Workload::OneToOne => "both sides together, 2n",
            Workload::SmallSide => "both sides together, m + 65,536",
            Workload::Dense | Workload::Sparse => "every input's rows together",
            Workload::Selective => "A, B and C together, a little over 2n",
        }
    }

    /// What its parameter is called: `n`, `m` or `k`.
    pub fn param(self) -> &'static str {
        match self {
            Workload::OneToOne | Workload::Selective => "n",
            Workload::SmallSide => "m",
            Workload::Dense | Workload::Sparse => "k",
        }
    }

    /// Instance `instance` of the inputs at `param`.
    pub fn generate(self, param: usize, instance: usize) -> Data {
        match self {
            Workload::OneToOne => Data::Binary(workload::one_to_one(param, instance)),
            Workload::SmallSide => Data::Binary(workload::small_side(param, instance)),
            Workload::Dense => Data::Multi(workload::dense(param, instance)),
            Workload::Sparse => Data::Multi(workload::sparse(param, instance)),
            Workload::Selective => Data::Multi(workload::selective(param, instance)),
        }
    }

    /// The instances a timed case at `param` rotates through: as many as
    /// [`rotation`] says, the first being `generate(param, 0)`.
    ///
    /// # Panics
    ///
    /// If an instance's inputs have other row counts than the first's, or
    /// join to another count: every call has to do the same work over the
    /// same rows, which the throughput and the per-row times assume.
    /// [`workload::sparse`] draws its inputs' sizes, but holds at least
    /// [`ROTATION_ROWS`] rows, so it has one instance.
    pub fn instances(self, param: usize) -> Vec<Data> {
        let first = self.generate(param, 0);
        let count = rotation(first.input_rows());
        let mut all = Vec::with_capacity(count);
        all.push(first);
        for instance in 1..count {
            let data = self.generate(param, instance);
            assert_eq!(
                data.shape(),
                all[0].shape(),
                "{self:?} at {param}: instance {instance}'s row counts differ from instance 0's"
            );
            all.push(data);
        }
        all
    }
}

/// A criterion benchmark group: a workload, the parameters it is generated
/// at, and the algorithms run on it at each.
#[derive(Debug)]
pub struct Group {
    pub name: &'static str,
    pub workload: Workload,
    pub params: &'static [usize],
    pub algorithms: &'static [Algorithm],
}

/// The same timed case run first and last, so the report can tell whether the
/// machine sped up or slowed down in between.
const SENTINEL: usize = 1 << 18;

/// The numbers of inputs the n-way groups join.
const KS: &[usize] = &[2, 4, 8, 16, 32, 64];

/// Every group, in run order: 106 cases.
pub const GROUPS: &[Group] = {
    use Algorithm::*;
    use Workload::*;
    &[
        Group {
            name: "sentinel_start",
            workload: OneToOne,
            params: &[SENTINEL],
            algorithms: &[Hash],
        },
        Group {
            name: "one_to_one",
            workload: OneToOne,
            params: &SIZES,
            algorithms: &[NestedLoop, Hash, HashSorted, Merge, SortMerge],
        },
        Group {
            name: "small_side",
            workload: SmallSide,
            params: &[1, 2, 4, 16, 64],
            algorithms: &[NestedLoop, Hash],
        },
        Group {
            name: "hash_variants",
            workload: OneToOne,
            params: &[1 << 14, 1 << 20, 1 << 22],
            algorithms: &[Hash, HashSipHash],
        },
        Group {
            name: "multiway_dense",
            workload: Dense,
            params: KS,
            algorithms: &[Heap, Scan, Leapfrog],
        },
        Group {
            name: "multiway_sparse",
            workload: Sparse,
            params: KS,
            algorithms: &[Heap, Scan, Leapfrog],
        },
        Group {
            name: "multiway_selective",
            workload: Selective,
            params: &[1 << 12, 1 << 14, 1 << 16, 1 << 18, 1 << 20],
            algorithms: &[Heap, Leapfrog, CascadePlanned, CascadeUnplanned],
        },
        Group {
            name: "sentinel_end",
            workload: OneToOne,
            params: &[SENTINEL],
            algorithms: &[Hash],
        },
    ]
};

impl Group {
    /// Whether this group only brackets the run, for measuring drift.
    pub fn is_sentinel(&self) -> bool {
        self.name.starts_with("sentinel_")
    }

    /// The cases it runs at `param`.
    pub fn cases_at(&'static self, param: usize) -> impl Iterator<Item = Case> {
        self.algorithms
            .iter()
            .filter(move |alg| alg.runs_at(param))
            .map(move |&algorithm| Case {
                group: self,
                algorithm,
                param,
            })
    }

    /// The cases it runs, parameters outermost.
    pub fn cases(&'static self) -> impl Iterator<Item = Case> {
        self.params
            .iter()
            .flat_map(move |&param| self.cases_at(param))
    }
}

/// The group called `name`.
///
/// # Panics
///
/// If there's no such group.
pub fn group(name: &str) -> &'static Group {
    GROUPS
        .iter()
        .find(|group| group.name == name)
        .unwrap_or_else(|| panic!("no group {name}"))
}

/// Every case, in run order.
pub fn cases() -> impl Iterator<Item = Case> {
    GROUPS.iter().flat_map(Group::cases)
}

/// One benchmark: an algorithm on a group's workload at one parameter.
#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub group: &'static Group,
    pub algorithm: Algorithm,
    pub param: usize,
}

/// Its criterion id, `group/algorithm/param`.
impl fmt::Display for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}",
            self.group.name,
            self.algorithm.name(),
            self.param
        )
    }
}

/// Checks what a case emitted: as many tuples as an independent count of the
/// workload says, and the checksum the group's first algorithm emitted on the
/// same inputs, which it stores in `agreed`.
///
/// # Panics
///
/// If either differs, so a wrong join can't post a number.
pub fn verify(case: &Case, data: &Data, out: Checksum, agreed: &mut Option<Checksum>) {
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
    fn shape(&self) -> (Vec<usize>, u64) {
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
const BUCKET: usize = size_of::<(&u64, &Row)>() + 1;

/// Control bytes hashbrown adds past the last bucket, one per lane of its
/// group: 8 on aarch64, 16 on x86, so a claim allows up to 16.
const GROUP_WIDTH: usize = 16;

/// How many buckets hashbrown gives a table with room for `rows`: at least 4,
/// and a power of two at most 7/8 full.
pub fn buckets(rows: usize) -> usize {
    match rows {
        0..4 => 4,
        4..8 => 8,
        _ => (rows * 8 / 7).next_power_of_two(),
    }
}

/// The algorithms whose peaks the tables show but no claim checks. A binary
/// plan's peak is set by where its intermediate results' sizes fall against
/// the powers of two its Vecs grow through, so it is reported, not asserted.
pub const REPORTED_ONLY: &[Algorithm] = &[Algorithm::CascadePlanned, Algorithm::CascadeUnplanned];

/// What the post says a join's memory is. The memory harness asserts every
/// one, and every memory number the tables quote has one, except the peaks of
/// the [`REPORTED_ONLY`] algorithms; a test holds the tables to that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// Exactly this peak, in exactly this many allocations.
    Exact {
        peak_bytes: usize,
        allocations: usize,
    },
    /// The hash join's table with room for `rows`, in one allocation:
    /// [`buckets`] of them, each a `(&key, &row)` entry and a control byte.
    /// Its peak is exact but for the control bytes past the last bucket, up
    /// to 16.
    Table { rows: usize },
}

impl Claim {
    /// A table's claim; an empty build allocates nothing.
    fn table(rows: usize) -> Self {
        match rows {
            0 => Claim::Exact {
                peak_bytes: 0,
                allocations: 0,
            },
            _ => Claim::Table { rows },
        }
    }

    /// The least and most peak the claim allows.
    fn peak(self) -> (usize, usize) {
        match self {
            Claim::Exact { peak_bytes, .. } => (peak_bytes, peak_bytes),
            Claim::Table { rows } => {
                let least = BUCKET * buckets(rows);
                (least, least + GROUP_WIDTH)
            }
        }
    }

    fn allocations(self) -> usize {
        match self {
            Claim::Exact { allocations, .. } => allocations,
            Claim::Table { .. } => 1,
        }
    }

    /// Whether `usage` bears the claim out; if not, what was claimed and what
    /// was measured.
    pub fn check(self, usage: &Usage) -> Result<(), String> {
        let (least, most) = self.peak();
        if (least..=most).contains(&usage.peak_bytes) && usage.allocations == self.allocations() {
            Ok(())
        } else {
            Err(format!(
                "claimed {self}, measured a peak of {} B in {} allocations",
                usage.peak_bytes, usage.allocations
            ))
        }
    }
}

impl fmt::Display for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.peak() {
            (least, most) if least == most => write!(f, "a peak of {least} B")?,
            (least, most) => write!(f, "a peak of {least}..={most} B")?,
        }
        write!(f, " in {} allocations", self.allocations())
    }
}

/// What the post says `alg`'s memory is on `data`: none for the
/// [`REPORTED_ONLY`] algorithms.
pub fn memory_claim(alg: Algorithm, data: &Data) -> Option<Claim> {
    use Algorithm::*;
    // An n-way join holds a slice of each input in `rest`, and a reference to
    // a row of each in `picked`. No input here is empty; leapfrog allocates
    // nothing when one is.
    let per_input = size_of::<&[Row]>() + size_of::<&Row>();
    let exact = |peak_bytes, allocations| {
        Some(Claim::Exact {
            peak_bytes,
            allocations,
        })
    };
    match (alg, data) {
        (NestedLoop | Merge, _) => exact(0, 0),
        (Hash | HashSorted | HashSipHash, Data::Binary(w)) => Some(Claim::table(w.left.len())),
        // The two copies, sorted in place. Both inputs here are non-empty.
        (SortMerge, Data::Binary(w)) => exact(size_of::<Row>() * (w.left.len() + w.right.len()), 2),
        // Plus the heap of (next key, input): 40 B an input.
        (Heap, Data::Multi(w)) => {
            exact(w.inputs.len() * (per_input + size_of::<(&u64, usize)>()), 3)
        }
        // Plus the inputs' order in the cycle: 32 B an input.
        (Leapfrog, Data::Multi(w)) => exact(w.inputs.len() * (per_input + size_of::<usize>()), 3),
        // 24 B an input.
        (Scan, Data::Multi(w)) => exact(w.inputs.len() * per_input, 2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tally;
    use std::collections::{BTreeMap, HashSet};

    #[test]
    fn every_algorithm_agrees_at_each_groups_smallest_param() {
        for group in GROUPS {
            let param = *group.params.iter().min().unwrap();
            let data = group.workload.generate(param, 0);
            let prepared = data.prepare();
            let mut agreed = None;
            for case in group.cases_at(param) {
                let mut out = Checksum::default();
                case.algorithm.run(&prepared, &mut out);
                verify(&case, &data, out, &mut agreed);
                // The timed sink sees the same tuples.
                let mut tally = Tally::default();
                case.algorithm.run_into(&prepared, &mut tally);
                assert_eq!(tally.tuples, out.rows, "{case}");
            }
        }
    }

    #[test]
    fn every_quoted_memory_number_has_a_claim() {
        use crate::report::{FACTS, Kind, Measure, Metric, TABLES, Value};
        // The algorithms each table or fact quotes a peak or allocation count
        // of, by the workload they ran on.
        let mut quoted: BTreeMap<(&str, usize), Vec<Algorithm>> = BTreeMap::new();
        let mut quote = |group: &'static str, algorithm: Algorithm, param: usize| {
            quoted.entry((group, param)).or_default().push(algorithm);
        };
        for table in TABLES {
            match &table.kind {
                Kind::Profile { cases, .. } => {
                    for (c, param) in *cases {
                        quote(c.group, c.algorithm, *param);
                    }
                }
                Kind::Grid {
                    columns,
                    metric: Metric::Peak,
                    ..
                } => {
                    for c in *columns {
                        for case in group(c.group).cases() {
                            if case.algorithm == c.algorithm {
                                quote(c.group, c.algorithm, case.param);
                            }
                        }
                    }
                }
                Kind::Grid { .. } => {}
            }
        }
        for fact in FACTS {
            let quantities = match &fact.value {
                Value::Ratio(a, b) | Value::Drift(a, b) => vec![a, b],
                Value::Single(a) => vec![a],
                Value::Crossover { .. } => vec![],
            };
            for q in quantities {
                if matches!(q.measure, Measure::Peak | Measure::Allocations) {
                    quote(q.group, q.algorithm, q.param);
                }
            }
        }

        for ((name, param), algorithms) in quoted {
            let data = group(name).workload.generate(param, 0);
            for alg in algorithms {
                assert!(
                    memory_claim(alg, &data).is_some() || REPORTED_ONLY.contains(&alg),
                    "{name}/{}/{param} is quoted, but no claim checks it",
                    alg.name()
                );
            }
        }
    }

    #[test]
    fn a_cycle_holds_at_least_rotation_rows() {
        for group in GROUPS {
            let param = *group.params.iter().min().unwrap();
            // Asserts that every instance has the first's row counts.
            let instances = group.workload.instances(param);
            let cycle = instances[0].input_rows() * instances.len() as u64;
            assert!(cycle >= ROTATION_ROWS, "{}", group.name);
        }
        // And no more instances than that takes.
        assert_eq!(rotation(2 << 10), 512);
        assert_eq!(rotation((1 << 16) + 4), 16);
        assert_eq!(rotation(1 << 20), 1);
        assert_eq!(rotation(2 << 20), 1);
    }

    #[test]
    fn ids_are_unique() {
        let ids: Vec<String> = cases().map(|case| case.to_string()).collect();
        assert_eq!(ids.len(), 106);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    }

    #[test]
    fn the_caps_hold() {
        for case in cases() {
            if case.algorithm == Algorithm::NestedLoop {
                assert!(case.param <= 1 << 16, "{case}");
            }
        }
        let nested = group("one_to_one")
            .cases()
            .filter(|case| case.algorithm == Algorithm::NestedLoop);
        assert_eq!(nested.map(|case| case.param).max(), Some(1 << 16));
        // Both binary plans run at every size.
        let selective = group("multiway_selective");
        assert_eq!(selective.cases().count(), 4 * selective.params.len());
    }
}
