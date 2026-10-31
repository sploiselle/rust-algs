//! The plan: every workload, the parameters it runs at, and the algorithms run
//! on it. The timing target, the memory harness and the report all read
//! [`GROUPS`], so every case that's timed is also profiled, and the tables cite
//! only cases that ran.
//!
//! Each workload makes one tradeoff visible:
//!
//! - `pk_fk` is the common case, customers and their orders. Its hash table
//!   outgrows the cache between 1M and 4M orders.
//! - `few_customers` is the one regime where a nested loop can win. Its
//!   sorted-orders column is the control for branch prediction.
//! - `hash_variants` shows the hasher, the build side and the table's layout,
//!   in time and in memory.
//! - `many_to_many` is bound by its output. Its sorted-input column removes
//!   the cost of both inputs being shuffled, without splitting it between the
//!   build and probe sides, and its Vec-per-key column changes the table's
//!   layout.
//! - `multiway_dense` against `multiway_sparse` shows when a heap pays against
//!   a scan of every input's next key.
//! - `multiway_selective` shows how much a binary plan's join order matters,
//!   and what skipping saves.
//!
//! Every case is checked with [`verify`] before it is timed or measured.

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
/// comparable across reruns. A sample runs at least one iteration, so the
/// slowest case, `hash_build_larger` at 4M (about 0.29 s a call), takes about
/// 9 s rather than [`MEASUREMENT`].
pub const SAMPLES: usize = 30;

/// A join, and the inputs it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    /// `nested_loop_join` on equal keys.
    NestedLoop,
    /// The same, with the orders sorted by key, so its branch is predictable.
    NestedLoopSorted,
    /// `hash_join`, building the customers, with foldhash.
    Hash,
    /// The same on both inputs sorted by key. Each key's probes then arrive
    /// together and reuse a cached table slot and chain, and the chains are
    /// walked in address order. That removes the cost of shuffled inputs, but
    /// doesn't say which side's order the cost belongs to.
    HashSorted,
    /// The same with std's hasher, SipHash-1-3, with fixed keys.
    HashSipHash,
    /// `hash_join` building the orders, the larger input.
    HashBuildLarger,
    /// [`baseline::hash_join_grouped`], a Vec per key.
    HashGrouped,
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
            Algorithm::NestedLoopSorted => "nested_loop_sorted",
            Algorithm::Hash => "hash",
            Algorithm::HashSorted => "hash_sorted",
            Algorithm::HashSipHash => "hash_siphash",
            Algorithm::HashBuildLarger => "hash_build_larger",
            Algorithm::HashGrouped => "hash_grouped",
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
            Algorithm::NestedLoopSorted => "nested loop (orders sorted)",
            Algorithm::Hash => "hash",
            Algorithm::HashSorted => "hash (sorted input)",
            Algorithm::HashSipHash => "hash, SipHash",
            Algorithm::HashBuildLarger => "hash, building orders",
            Algorithm::HashGrouped => "hash, Vec per key",
            Algorithm::Merge => "merge (sorted input)",
            Algorithm::SortMerge => "sort-merge",
            Algorithm::Heap => "heap",
            Algorithm::Scan => "scan",
            Algorithm::Leapfrog => "leapfrog",
            Algorithm::CascadePlanned => "binary plan (smallest first)",
            Algorithm::CascadeUnplanned => "binary plan (as given)",
        }
    }

    /// Whether it runs at `param`. The nested loop is quadratic, and past
    /// 64K orders it would take most of the run; the unplanned cascade would
    /// peak at about 1.1 GB (1.03 GiB) at 1M rows.
    pub fn runs_at(self, param: usize) -> bool {
        match self {
            Algorithm::NestedLoop | Algorithm::NestedLoopSorted => param <= 1 << 16,
            Algorithm::CascadeUnplanned => param <= 1 << 18,
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
        let equal = |l: &Row, r: &Row| l.0 == r.0;
        let mut pair = |l: &Row, r: &Row| out.pair(l, r);
        match (self, data) {
            (NestedLoop, Prepared::Binary(w)) => nested_loop_join(&w.left, &w.right, equal, pair),
            (NestedLoopSorted, Prepared::Binary(w)) => {
                nested_loop_join(&w.left, &w.right_sorted, equal, pair);
            }
            (Hash, Prepared::Binary(w)) => hash_join(&w.left, &w.right, fold, pair),
            (HashSorted, Prepared::Binary(w)) => {
                hash_join(&w.left_sorted, &w.right_sorted, fold, pair);
            }
            (HashSipHash, Prepared::Binary(w)) => hash_join(&w.left, &w.right, sip, pair),
            // Built the other way round, so flip each pair back.
            (HashBuildLarger, Prepared::Binary(w)) => {
                hash_join(&w.right, &w.left, fold, |r, l| pair(l, r));
            }
            (HashGrouped, Prepared::Binary(w)) => {
                baseline::hash_join_grouped(&w.left, &w.right, fold, pair);
            }
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
    /// [`workload::customers_orders`], at `n` orders.
    CustomersOrders,
    /// [`workload::few_customers`], at `m` customers.
    FewCustomers,
    /// [`workload::many_to_many`], at `n` rows a side.
    ManyToMany,
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
            Workload::CustomersOrders => "customers and orders together, 9/8 of the orders",
            Workload::FewCustomers => "customers and orders together",
            Workload::ManyToMany => "both sides together",
            Workload::Dense | Workload::Sparse => "every input's rows together",
            Workload::Selective => "A, B and C together, a little over 2n",
        }
    }

    /// The inputs at `param`.
    pub fn generate(self, param: usize) -> Data {
        match self {
            Workload::CustomersOrders => Data::Binary(workload::customers_orders(param)),
            Workload::FewCustomers => Data::Binary(workload::few_customers(param)),
            Workload::ManyToMany => Data::Binary(workload::many_to_many(param)),
            Workload::Dense => Data::Multi(workload::dense(param)),
            Workload::Sparse => Data::Multi(workload::sparse(param)),
            Workload::Selective => Data::Multi(workload::selective(param)),
        }
    }
}

/// What a group's times are divided by in its table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    PerInputRow,
    /// For a workload whose output outnumbers its input, so the output is
    /// the work.
    PerOutputRow,
}

impl Unit {
    /// What one is called in a caption, such as `input row`.
    pub fn row(self) -> &'static str {
        match self {
            Unit::PerInputRow => "input row",
            Unit::PerOutputRow => "output row",
        }
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
    pub unit: Unit,
}

/// The same timed case run first and last, so the report can tell whether the
/// machine sped up or slowed down in between.
const SENTINEL: usize = 1 << 18;

/// The numbers of inputs the n-way groups join.
const KS: &[usize] = &[2, 4, 8, 16, 32, 64];

/// Every group, in run order: 146 cases.
pub const GROUPS: &[Group] = {
    use Algorithm::*;
    use Workload::*;
    &[
        Group {
            name: "sentinel_start",
            workload: CustomersOrders,
            params: &[SENTINEL],
            algorithms: &[Hash],
            unit: Unit::PerInputRow,
        },
        Group {
            name: "pk_fk",
            workload: CustomersOrders,
            params: &SIZES,
            algorithms: &[NestedLoop, Hash, HashSorted, Merge, SortMerge],
            unit: Unit::PerInputRow,
        },
        Group {
            name: "few_customers",
            workload: FewCustomers,
            params: &[1, 2, 4, 16, 64],
            algorithms: &[NestedLoop, NestedLoopSorted, Hash],
            unit: Unit::PerInputRow,
        },
        Group {
            name: "hash_variants",
            workload: CustomersOrders,
            params: &[1 << 14, 1 << 20, 1 << 22],
            algorithms: &[Hash, HashSipHash, HashBuildLarger, HashGrouped],
            unit: Unit::PerInputRow,
        },
        Group {
            name: "many_to_many",
            workload: ManyToMany,
            params: SIZES.split_at(6).0,
            algorithms: &[Hash, HashSorted, HashGrouped, Merge, SortMerge],
            unit: Unit::PerOutputRow,
        },
        Group {
            name: "multiway_dense",
            workload: Dense,
            params: KS,
            algorithms: &[Heap, Scan, Leapfrog],
            unit: Unit::PerInputRow,
        },
        Group {
            name: "multiway_sparse",
            workload: Sparse,
            params: KS,
            algorithms: &[Heap, Scan, Leapfrog],
            unit: Unit::PerInputRow,
        },
        Group {
            name: "multiway_selective",
            workload: Selective,
            params: &[1 << 12, 1 << 14, 1 << 16, 1 << 18, 1 << 20],
            algorithms: &[Heap, Leapfrog, CascadePlanned, CascadeUnplanned],
            unit: Unit::PerInputRow,
        },
        Group {
            name: "sentinel_end",
            workload: CustomersOrders,
            params: &[SENTINEL],
            algorithms: &[Hash],
            unit: Unit::PerInputRow,
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

/// Bytes per build row in the hash join's chain, an `Option<usize>`.
const CHAIN_ENTRY: usize = size_of::<Option<usize>>();

/// Bytes per bucket in the hash join's table: a `(&key, row)` entry, and
/// hashbrown's control byte.
const BUCKET: usize = size_of::<(&u64, usize)>() + 1;

/// Bytes per bucket in the Vec-per-key table: a `(&key, Vec)` entry, and the
/// control byte.
const GROUPED_BUCKET: usize = size_of::<(&u64, Vec<&Row>)>() + 1;

/// Bytes of a key's Vec in the Vec-per-key table when it holds one row: its
/// first push makes room for 4 references, std's least capacity for 8-byte
/// elements.
const GROUPED_ROW: usize = 4 * size_of::<&Row>();

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
    /// A hash table with room for `rows`, `bucket` bytes a bucket, beside
    /// `per_row` bytes for each row, in `allocations` allocations. Its peak is
    /// exact but for the control bytes past the last bucket, up to
    /// 16.
    Table {
        rows: usize,
        bucket: usize,
        per_row: usize,
        allocations: usize,
    },
}

impl Claim {
    /// A table's claim; an empty build allocates nothing.
    fn table(rows: usize, bucket: usize, per_row: usize, allocations: usize) -> Self {
        match rows {
            0 => Claim::Exact {
                peak_bytes: 0,
                allocations: 0,
            },
            _ => Claim::Table {
                rows,
                bucket,
                per_row,
                allocations,
            },
        }
    }

    /// The least and most peak the claim allows.
    fn peak(self) -> (usize, usize) {
        match self {
            Claim::Exact { peak_bytes, .. } => (peak_bytes, peak_bytes),
            Claim::Table {
                rows,
                bucket,
                per_row,
                ..
            } => {
                let least = per_row * rows + bucket * buckets(rows);
                (least, least + GROUP_WIDTH)
            }
        }
    }

    fn allocations(self) -> usize {
        match self {
            Claim::Exact { allocations, .. } | Claim::Table { allocations, .. } => allocations,
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
/// [`REPORTED_ONLY`] algorithms, or for a Vec per key when keys repeat.
pub fn memory_claim(alg: Algorithm, data: &Data) -> Option<Claim> {
    use Algorithm::*;
    // An n-way join holds a slice of each input in `rest` and in `runs`, and
    // a reference to a row of each in `picked`.
    let per_input = 2 * size_of::<&[Row]>() + size_of::<&Row>();
    let exact = |peak_bytes, allocations| {
        Some(Claim::Exact {
            peak_bytes,
            allocations,
        })
    };
    let hash_join = |build| Some(Claim::table(build, BUCKET, CHAIN_ENTRY, 2));
    match (alg, data) {
        (NestedLoop | NestedLoopSorted | Merge, _) => exact(0, 0),
        (Hash | HashSorted | HashSipHash, Data::Binary(w)) => hash_join(w.left.len()),
        (HashBuildLarger, Data::Binary(w)) => hash_join(w.right.len()),
        // The table, then one Vec per key: one allocation a row when no key
        // repeats.
        (HashGrouped, Data::Binary(w)) if unique(&w.left_sorted) => {
            let build = w.left.len();
            Some(Claim::table(build, GROUPED_BUCKET, GROUPED_ROW, 1 + build))
        }
        // The two copies, sorted in place. Both inputs here are non-empty.
        (SortMerge, Data::Binary(w)) => exact(size_of::<Row>() * (w.left.len() + w.right.len()), 2),
        // Plus the heap of (next key, input): 56 B an input.
        (Heap, Data::Multi(w)) => {
            exact(w.inputs.len() * (per_input + size_of::<(&u64, usize)>()), 4)
        }
        // Plus the inputs' order in the cycle: 48 B an input. No input here is
        // empty; leapfrog allocates nothing when one is.
        (Leapfrog, Data::Multi(w)) => exact(w.inputs.len() * (per_input + size_of::<usize>()), 4),
        // 40 B an input.
        (Scan, Data::Multi(w)) => exact(w.inputs.len() * per_input, 3),
        _ => None,
    }
}

/// Whether no two of `sorted`'s rows share a key.
fn unique(sorted: &[Row]) -> bool {
    sorted.windows(2).all(|pair| pair[0].0 != pair[1].0)
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
            let data = group.workload.generate(param);
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
            let data = group(name).workload.generate(param);
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
    fn ids_are_unique() {
        let ids: Vec<String> = cases().map(|case| case.to_string()).collect();
        assert_eq!(ids.len(), 146);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    }

    #[test]
    fn the_caps_hold() {
        for case in cases() {
            match case.algorithm {
                Algorithm::NestedLoop | Algorithm::NestedLoopSorted => {
                    assert!(case.param <= 1 << 16, "{case}");
                }
                Algorithm::CascadeUnplanned => assert!(case.param <= 1 << 18, "{case}"),
                _ => {}
            }
        }
        let pk_fk = group("pk_fk")
            .cases()
            .filter(|case| case.algorithm == Algorithm::NestedLoop);
        assert_eq!(pk_fk.map(|case| case.param).max(), Some(1 << 16));
    }
}
