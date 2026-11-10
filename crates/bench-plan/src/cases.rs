//! The plan: every group, the parameters it runs at, and the joins run on it.
//! The memory harness and the report read [`GROUPS`], so every case that's
//! timed is also profiled, and the tables cite only cases that ran.
//!
//! Each group is defined once, in `crates/bench/groups/<name>.rs`, which two
//! crates compile: the group's own timing binary, as its `main`, and this one,
//! as a row of [`GROUPS`]. No timing binary links this crate, so the plan can
//! grow, and everything else here change, without changing a binary whose
//! group didn't. To add a group, write its file, give it a `[[bench]]` and a
//! `benches/<name>.rs` in `crates/bench`, and add it to [`GROUPS`].
//!
//! Each workload makes one tradeoff visible: its group's file says which.
//!
//! A timed case rotates through instances of its input: the same workload at
//! the same parameter from different seeds, with the same row counts, holding
//! at least [`ROTATION_ROWS`](harness::ROTATION_ROWS) rows before one repeats,
//! so the branch predictor learns little of any one. The memory harness
//! profiles each case's first instance alone.

use std::fmt;

use bench::harness::{self, Data, Prepared};
use bench::joins::{self, Join};
use bench::{Checksum, Row, Sink, workload};

use crate::alloc::Usage;

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
    /// [`bench::baseline::multiway_merge_join_by_scan`].
    Scan,
    /// `leapfrog_join`; on two inputs, on both sorted, as `Merge` takes them.
    Leapfrog,
    /// `cascade_merge_join` on the inputs smallest first, as a planner with
    /// estimates orders them.
    CascadePlanned,
    /// `cascade_merge_join` on the inputs as given.
    CascadeUnplanned,
}

impl Algorithm {
    /// Its name in benchmark ids, such as `hash_sorted`: the name its timing
    /// binary gives it.
    pub fn name(self) -> &'static str {
        match self {
            Algorithm::NestedLoop => joins::NestedLoop::NAME,
            Algorithm::Hash => joins::Hash::NAME,
            Algorithm::HashSorted => joins::HashSorted::NAME,
            Algorithm::HashSipHash => joins::HashSipHash::NAME,
            Algorithm::Merge => joins::Merge::NAME,
            Algorithm::SortMerge => joins::SortMerge::NAME,
            Algorithm::Heap => joins::Heap::NAME,
            Algorithm::Scan => joins::Scan::NAME,
            Algorithm::Leapfrog => joins::Leapfrog::NAME,
            Algorithm::CascadePlanned => joins::CascadePlanned::NAME,
            Algorithm::CascadeUnplanned => joins::CascadeUnplanned::NAME,
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

    /// Whether it runs at `param`.
    pub fn runs_at(self, param: usize) -> bool {
        match self {
            Algorithm::NestedLoop => joins::NestedLoop::runs_at(param),
            Algorithm::Hash => joins::Hash::runs_at(param),
            Algorithm::HashSorted => joins::HashSorted::runs_at(param),
            Algorithm::HashSipHash => joins::HashSipHash::runs_at(param),
            Algorithm::Merge => joins::Merge::runs_at(param),
            Algorithm::SortMerge => joins::SortMerge::runs_at(param),
            Algorithm::Heap => joins::Heap::runs_at(param),
            Algorithm::Scan => joins::Scan::runs_at(param),
            Algorithm::Leapfrog => joins::Leapfrog::runs_at(param),
            Algorithm::CascadePlanned => joins::CascadePlanned::runs_at(param),
            Algorithm::CascadeUnplanned => joins::CascadeUnplanned::runs_at(param),
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
        match self {
            Algorithm::NestedLoop => joins::NestedLoop::run(data, out),
            Algorithm::Hash => joins::Hash::run(data, out),
            Algorithm::HashSorted => joins::HashSorted::run(data, out),
            Algorithm::HashSipHash => joins::HashSipHash::run(data, out),
            Algorithm::Merge => joins::Merge::run(data, out),
            Algorithm::SortMerge => joins::SortMerge::run(data, out),
            Algorithm::Heap => joins::Heap::run(data, out),
            Algorithm::Scan => joins::Scan::run(data, out),
            Algorithm::Leapfrog => joins::Leapfrog::run(data, out),
            Algorithm::CascadePlanned => joins::CascadePlanned::run(data, out),
            Algorithm::CascadeUnplanned => joins::CascadeUnplanned::run(data, out),
        }
    }
}

/// What kind of inputs a group has, for what the captions say of them. The
/// inputs themselves come from the group's [`generate`](Group::generate).
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
}

/// A group: a workload, the parameters it is generated at, and the joins run
/// on it at each. Its timings come from a binary of its own.
#[derive(Debug)]
pub struct Group {
    pub name: &'static str,
    pub workload: Workload,
    /// Instance `instance` of the inputs at `param`.
    pub generate: fn(usize, usize) -> Data,
    pub params: &'static [usize],
    pub algorithms: &'static [Algorithm],
}

/// A group's definition, as a [`Group`]. Its timing binary reads the same
/// file with a `group!` of its own, as its `main`.
macro_rules! group {
    (
        name: $name:literal,
        workload: $kind:ident => $generate:ident,
        params: [$($param:expr),* $(,)?],
        algorithms: [$($join:ident),* $(,)?] $(,)?
    ) => {
        Group {
            name: $name,
            workload: Workload::$kind,
            generate: |param, instance| Data::from(workload::$generate(param, instance)),
            params: &[$($param),*],
            algorithms: &[$(Algorithm::$join),*],
        }
    };
}

/// Every group, in the order the runner runs them: 153 cases.
pub const GROUPS: &[Group] = &[
    include!("../../bench/groups/one_to_one.rs"),
    include!("../../bench/groups/small_side.rs"),
    include!("../../bench/groups/both_small.rs"),
    include!("../../bench/groups/hash_variants.rs"),
    include!("../../bench/groups/multiway_dense.rs"),
    include!("../../bench/groups/multiway_sparse.rs"),
    include!("../../bench/groups/multiway_selective.rs"),
];

impl Group {
    /// The instances a timed case at `param` rotates through: see
    /// [`harness::instances`].
    pub fn instances(&self, param: usize) -> Vec<Data> {
        harness::instances(self.generate, param)
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

/// Control bytes hashbrown adds past the last bucket, one per lane of its
/// group: 8 on aarch64, 16 on x86, so a claim allows up to 16.
const GROUP_WIDTH: usize = 16;

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
    /// [`harness::buckets`] of them, each a `(&key, &row)` entry and a control byte.
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
                let least = harness::BUCKET * harness::buckets(rows);
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
        (Leapfrog, Data::Binary(_)) => exact(2 * (per_input + size_of::<usize>()), 3),
        // 24 B an input.
        (Scan, Data::Multi(w)) => exact(w.inputs.len() * per_input, 2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bench::Tally;
    use std::collections::HashSet;

    #[test]
    fn every_algorithm_agrees_at_each_groups_smallest_param() {
        for group in GROUPS {
            let param = *group.params.iter().min().unwrap();
            let data = (group.generate)(param, 0);
            let prepared = data.prepare();
            let mut agreed = None;
            for case in group.cases_at(param) {
                let mut out = Checksum::default();
                case.algorithm.run(&prepared, &mut out);
                harness::verify(&case, &data, out, &mut agreed);
                // The timed sink sees the same tuples.
                let mut tally = Tally::default();
                case.algorithm.run_into(&prepared, &mut tally);
                assert_eq!(tally.tuples, out.rows, "{case}");
            }
        }
    }

    #[test]
    fn a_cycle_holds_at_least_rotation_rows() {
        for group in GROUPS {
            let param = *group.params.iter().min().unwrap();
            // Asserts that every instance has the first's row counts.
            let instances = group.instances(param);
            let cycle = instances[0].input_rows() * instances.len() as u64;
            assert!(cycle >= harness::ROTATION_ROWS, "{}", group.name);
        }
    }

    #[test]
    fn ids_are_unique() {
        let ids: Vec<String> = cases().map(|case| case.to_string()).collect();
        assert_eq!(ids.len(), 153);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    }

    #[test]
    fn every_group_has_a_timing_binary_of_its_own() {
        // The runner finds a group's timings by the bench target named for it.
        let manifest = include_str!("../../bench/Cargo.toml");
        for group in GROUPS {
            assert!(
                manifest.contains(&format!("[[bench]]\nname = \"{}\"\n", group.name)),
                "{} has no [[bench]] in crates/bench",
                group.name
            );
        }
        let targets = manifest.matches("[[bench]]").count();
        assert_eq!(
            targets,
            GROUPS.len(),
            "a bench target has no group in the plan"
        );
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
