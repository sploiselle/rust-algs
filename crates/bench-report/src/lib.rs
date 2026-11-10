//! The tables: criterion's timings, the memory profiles and the cache
//! measures as Markdown a post can paste, each citing the commit and machine
//! it came from. And the [`runner`] that brings the results up to date.
//!
//! ```text
//! cargo run -p bench-report --release -- run          # run what's out of date
//! cargo run -p bench-report --release -- run --list   # say what would run
//! cargo run -p bench-report --release                 # write the tables
//! ```
//!
//! The report uses only results it can vouch for: those a bench target's
//! binary wrote, when that target still builds to the same binary. The
//! runner logs the hash of every executable it runs ([`record`]), and
//! [`evidence::built`] hashes what cargo builds now. Builds are reproducible,
//! so equal hashes mean the same machine code, laid out the same way, and a
//! result stands for as long as they are equal, whatever commits have landed
//! since. Each bench target links as little as it can (see `bench`'s docs),
//! so a change reruns only the targets whose binaries it changes, and a
//! change to this crate reruns none.
//!
//! It refuses, naming what's wrong, unless a flag allows it:
//!
//! - a target whose results are missing, or came from another build of it
//!   than the one built now ([`evidence::out_of_date`]; `--allow-stale`);
//! - a timing written by a run with other criterion settings than the
//!   captions describe, such as a smoke run's shorter measurement
//!   ([`shortened`]), or with a confidence level other than 95%
//!   (`--allow-stale`);
//! - results whose binary no clean tree has run, so no commit is known to
//!   hold their code (`--allow-dirty`).
//!
//! Each table cites the commit of the oldest clean run of the binary its
//! numbers came from: the first commit known to hold that code. A table that
//! draws on several targets cites the latest of theirs. Rerunning a noisy
//! case replaces its timing without invalidating the rest, and doesn't move
//! the commit. Under `--allow-stale`, a case with no result shows as
//! `missing`. Every file written under a flag says so, in its provenance
//! comment and its caption.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use bench::Row;
use bench::harness;
use bench_cache as cache;
use bench_plan::cases::{self, Algorithm, Case, Workload};
pub use bench_plan::fmt::{bytes, count, sig3};
use bench_plan::profile::MemoryCase;

pub mod evidence;
pub mod record;
pub mod runner;

use evidence::{Evidence, Target};
use record::Run;

/// The criterion version the timings were taken with, for captions.
const CRITERION: &str = "criterion 0.8.2";

pub use evidence::DRIFT_LIMIT;

/// What the report CLI was asked to do.
pub struct Options {
    /// Where to write the tables.
    pub out: PathBuf,
    pub allow_stale: bool,
    pub allow_dirty: bool,
}

/// One case's timing, from criterion's files.
#[derive(Clone, Debug)]
pub struct Timing {
    /// The median of criterion's samples, in ns per iteration.
    pub median_ns: f64,
    /// The median's confidence interval, at `confidence_level`.
    pub lower_ns: f64,
    pub upper_ns: f64,
    pub confidence_level: f64,
    /// The input rows criterion was given as the case's throughput.
    pub input_rows: Option<u64>,
    /// When criterion wrote it.
    pub written_unix_ms: u64,
}

impl Timing {
    /// Whether its confidence interval's half-width is over 5% of the median,
    /// which makes it worth rerunning.
    pub fn noisy(&self) -> bool {
        (self.upper_ns - self.lower_ns) / 2.0 > 0.05 * self.median_ns
    }
}

/// How criterion identifies a result: group, function and parameter.
pub type Key = (String, String, String);

/// `case`'s [`Key`].
pub fn key(case: &Case) -> Key {
    lookup(case.group.name, case.algorithm, case.param)
}

fn lookup(group: &str, algorithm: Algorithm, param: usize) -> Key {
    (
        group.to_owned(),
        algorithm.name().to_owned(),
        param.to_string(),
    )
}

/// The cache targets' measures, together.
#[derive(Clone, Debug)]
pub struct Cache {
    pub l2_bytes: usize,
    pub line_bytes: usize,
    pub phases: Vec<cache::Phases>,
    pub reads: Vec<cache::ReadCost>,
    pub worked_reads: Vec<cache::WorkedRead>,
    pub hashers: Vec<cache::HasherCost>,
    pub absent: Vec<cache::AbsentProbe>,
}

/// Everything the tables are made from.
pub struct Results {
    pub timings: HashMap<Key, Timing>,
    pub memory: HashMap<Key, MemoryCase>,
    /// The cache targets' measures, if all four have run.
    pub cache: Option<Cache>,
    /// The run each target's results are cited by, by target: the oldest
    /// clean run of the binary that wrote them.
    pub cited: HashMap<String, Run>,
    /// The flags that let the report use results it would otherwise refuse.
    pub allowed: Vec<&'static str>,
}

/// Which targets a file's numbers come from, and so which run it cites.
#[derive(Clone, Debug)]
enum Source {
    /// These groups' timings.
    Timings(Vec<&'static str>),
    /// The memory profiles.
    Memory,
    /// These cache measures.
    Cache(&'static [&'static str]),
}

impl Source {
    /// Every group's timings.
    fn every_group() -> Source {
        Source::Timings(cases::GROUPS.iter().map(|group| group.name).collect())
    }

    /// The groups of `columns`, each once.
    fn of(columns: &[Column]) -> Source {
        let mut groups: Vec<&'static str> = Vec::new();
        for column in columns {
            if !groups.contains(&column.group) {
                groups.push(column.group);
            }
        }
        Source::Timings(groups)
    }

    fn targets(&self) -> Vec<&'static str> {
        match self {
            Source::Timings(groups) => groups.clone(),
            Source::Memory => vec!["memory"],
            Source::Cache(parts) => parts.to_vec(),
        }
    }
}

/// What a file cites when none of its targets has a run to cite, which only
/// a flag lets through.
fn unknown_run() -> Run {
    let unknown = || "unknown".to_owned();
    Run {
        target: unknown(),
        started_unix_ms: 0,
        commit: unknown(),
        dirty: false,
        rustc: unknown(),
        cpu: unknown(),
        os: unknown(),
        args: Vec::new(),
        binary: unknown(),
    }
}

impl Results {
    /// The run a file drawing on `source` cites: of its targets' cited runs,
    /// the one that started last, whose commit holds all of their code.
    fn cited(&self, source: &Source) -> Run {
        source
            .targets()
            .iter()
            .filter_map(|target| self.cited.get(*target))
            .max_by_key(|run| run.started_unix_ms)
            .cloned()
            .unwrap_or_else(unknown_run)
    }

    /// Whether `q` reads a [noisy](Timing::noisy) timing. Memory is
    /// deterministic, so only times can be.
    fn noisy(&self, q: &Quantity) -> bool {
        match q.measure {
            Measure::Median | Measure::PerRow => self
                .timing(q.group, q.algorithm, q.param)
                .is_some_and(Timing::noisy),
            Measure::Peak | Measure::Allocations | Measure::InputBytes => false,
        }
    }

    fn timing(&self, group: &str, algorithm: Algorithm, param: usize) -> Option<&Timing> {
        self.timings.get(&lookup(group, algorithm, param))
    }

    fn profile(&self, group: &str, algorithm: Algorithm, param: usize) -> Option<&MemoryCase> {
        self.memory.get(&lookup(group, algorithm, param))
    }

    /// The median time per input row, in ns.
    fn per_row(&self, group: &str, algorithm: Algorithm, param: usize) -> Option<f64> {
        let timing = self.timing(group, algorithm, param)?;
        let rows = timing
            .input_rows
            .or_else(|| Some(self.profile(group, algorithm, param)?.input_rows))?;
        Some(timing.median_ns / rows as f64)
    }

    /// Whether a case's timed calls rotated through instances of its input,
    /// by the input rows criterion was given.
    fn rotated(&self, group: &str, algorithm: Algorithm, param: usize) -> bool {
        self.timing(group, algorithm, param)
            .and_then(|timing| timing.input_rows)
            .is_some_and(|rows| harness::rotation(rows) > 1)
    }

    fn quantity(&self, q: &Quantity) -> Option<f64> {
        let profile = || self.profile(q.group, q.algorithm, q.param);
        match q.measure {
            Measure::Median => Some(self.timing(q.group, q.algorithm, q.param)?.median_ns),
            Measure::PerRow => self.per_row(q.group, q.algorithm, q.param),
            Measure::Peak => Some(profile()?.usage.peak_bytes as f64),
            Measure::Allocations => Some(profile()?.usage.allocations as f64),
            Measure::InputBytes => Some(profile()?.input_bytes as f64),
        }
    }
}

/// Builds every bench target, reads every result, and refuses what
/// `options` doesn't allow. Returns the results, and a warning for each
/// problem a flag allowed.
///
/// # Errors
///
/// Every problem no flag allowed, or a failure to build or read.
pub fn load(options: &Options) -> Result<(Results, Vec<String>), String> {
    let targets = evidence::built()?;
    vouch(options, &targets, evidence::gather()?)
}

/// Checks `evidence` against the binaries `targets` build to now, refusing
/// what `options` doesn't allow: the part of [`load`] that builds nothing and
/// reads no files.
///
/// # Errors
///
/// Every problem no flag allowed.
pub fn vouch(
    options: &Options,
    targets: &[Target],
    evidence: Evidence,
) -> Result<(Results, Vec<String>), String> {
    let mut problems = Problems::new(options);
    let mut cited = HashMap::new();
    for target in targets {
        for why in evidence::out_of_date(target, &evidence) {
            problems.stale(format!("{}: {why}", target.name));
        }
        let what = match target.kind {
            evidence::Kind::Timing => {
                check_sampling(cases::group(&target.name), &evidence, &mut problems);
                format!("{}'s timings", target.name)
            }
            evidence::Kind::Memory => "the memory profiles".to_owned(),
            evidence::Kind::Cache => format!("the {} measures", target.name),
        };
        let run = cite(
            &evidence.runs,
            &target.name,
            &target.binary,
            &what,
            &mut problems,
        );
        if let Some(run) = run {
            cited.insert(target.name.clone(), run.clone());
        }
    }
    let (warnings, allowed) = problems.into_result().map_err(|refused| {
        format!(
            "{refused}\nrun `cargo run -p bench-report --release -- run` to run what's out of date"
        )
    })?;

    let cache = match (
        &evidence.phases,
        &evidence.reads,
        &evidence.hashers,
        &evidence.absent,
    ) {
        (Some(phases), Some(reads), Some(hashers), Some(absent)) => Some(Cache {
            l2_bytes: phases.value.l2_bytes,
            line_bytes: phases.value.line_bytes,
            phases: phases.value.phases.clone(),
            reads: reads.value.reads.clone(),
            worked_reads: reads.value.worked_reads.clone(),
            hashers: hashers.value.hashers.clone(),
            absent: absent.value.absent.clone(),
        }),
        _ => None,
    };
    let memory = evidence
        .memory
        .map_or(HashMap::new(), |memory| profiles(memory.value.cases));
    let results = Results {
        timings: evidence.timings,
        memory,
        cache,
        cited,
        allowed,
    };
    Ok((results, warnings))
}

/// The run a table of `target`'s results cites: the oldest clean run of
/// `binary`, whose commit is the first known to hold the code that built it.
/// A later run of the same binary ran the same code, whatever its commit. If
/// only dirty trees have run it, no commit is known to hold that code, which
/// `what` is told.
fn cite<'r>(
    runs: &'r [Run],
    target: &str,
    binary: &str,
    what: &str,
    problems: &mut Problems,
) -> Option<&'r Run> {
    let mut of_binary = runs
        .iter()
        .filter(|run| run.target == target && run.binary == binary);
    let clean = of_binary.clone().find(|run| !run.dirty);
    if clean.is_none() && of_binary.next().is_some() {
        problems.dirty(format!(
            "{what} came from a binary only dirty trees have run, so no commit holds its code"
        ));
    }
    clean
}

/// Notes the timings of `group` that criterion didn't sample as the captions
/// say: those a run with other settings wrote, and those whose confidence
/// interval isn't at 95%.
fn check_sampling(group: &'static cases::Group, evidence: &Evidence, problems: &mut Problems) {
    for (run, ids) in shortened(group, evidence) {
        problems.stale(format!(
            "{} timings came from the run that started {} with `{}`, not the criterion \
             settings the captions describe: {}",
            ids.len(),
            moment(run.started_unix_ms),
            run.args.join(" "),
            evidence::some(&ids)
        ));
    }
    let off_level: Vec<String> = group
        .cases()
        .filter(|case| {
            evidence
                .timings
                .get(&key(case))
                .is_some_and(|t| (t.confidence_level - CONFIDENCE).abs() > 1e-9)
        })
        .map(|case| case.to_string())
        .collect();
    if !off_level.is_empty() {
        problems.stale(format!(
            "{} timings have a confidence interval at a level other than 95%: {}",
            off_level.len(),
            evidence::some(&off_level)
        ));
    }
}

/// The confidence level the captions' "95%" and the † rule assume.
const CONFIDENCE: f64 = 0.95;

/// The criterion flags that change how a timing is sampled or estimated. A run
/// that passed any of them took timings the captions don't describe.
const SAMPLING_FLAGS: &[&str] = &[
    "--warm-up-time",
    "--measurement-time",
    "--sample-size",
    "--nresamples",
    "--quick",
    "--confidence-level",
];

/// The timings of `group` that a run with non-default criterion settings
/// (`--warm-up-time`, `--measurement-time`, `--sample-size`, `--nresamples`,
/// `--quick` or `--confidence-level`) wrote, by run. Each timing belongs to
/// the last run of the group that started before criterion wrote it, so a run
/// whose timings were all rewritten since owns none.
pub fn shortened<'e>(
    group: &'static cases::Group,
    evidence: &'e Evidence,
) -> Vec<(&'e Run, Vec<String>)> {
    let mut by_run: Vec<(&Run, Vec<String>)> = Vec::new();
    for (name, key) in evidence::timing_keys(group) {
        let writer = evidence
            .timings
            .get(&key)
            .and_then(|timing| evidence.writer(group.name, timing.written_unix_ms));
        let Some(run) = writer else { continue };
        if !SAMPLING_FLAGS
            .iter()
            .any(|flag| record::has_flag(&run.args, flag))
        {
            continue;
        }
        match by_run.iter_mut().find(|(r, _)| std::ptr::eq(*r, run)) {
            Some((_, ids)) => ids.push(name),
            None => by_run.push((run, vec![name])),
        }
    }
    by_run
}

/// What's wrong with the results, sorted by whether a flag allows it.
struct Problems<'o> {
    options: &'o Options,
    allowed: Vec<String>,
    refused: Vec<String>,
    /// The flags that allowed something.
    used: Vec<&'static str>,
}

impl<'o> Problems<'o> {
    fn new(options: &'o Options) -> Self {
        Self {
            options,
            allowed: Vec::new(),
            refused: Vec::new(),
            used: Vec::new(),
        }
    }

    fn dirty(&mut self, problem: String) {
        self.note(self.options.allow_dirty, "--allow-dirty", problem);
    }

    fn stale(&mut self, problem: String) {
        self.note(self.options.allow_stale, "--allow-stale", problem);
    }

    fn note(&mut self, allowed: bool, flag: &'static str, problem: String) {
        if allowed {
            self.allowed.push(problem);
            if !self.used.contains(&flag) {
                self.used.push(flag);
            }
        } else {
            self.refused
                .push(format!("{problem} (pass {flag} to report anyway)"));
        }
    }

    /// The allowed problems and the flags that allowed them, or every refused
    /// problem.
    fn into_result(self) -> Result<(Vec<String>, Vec<&'static str>), String> {
        if self.refused.is_empty() {
            Ok((self.allowed, self.used))
        } else {
            Err(self.refused.join("\n"))
        }
    }
}

/// The memory cases by key.
fn profiles(cases: Vec<MemoryCase>) -> HashMap<Key, MemoryCase> {
    cases
        .into_iter()
        .map(|case| (profile_key(&case), case))
        .collect()
}

/// A memory profile's [`Key`].
fn profile_key(case: &MemoryCase) -> Key {
    (
        case.group.clone(),
        case.algorithm.clone(),
        case.param.to_string(),
    )
}

/// The parts of criterion's `new/benchmark.json` the report reads.
#[derive(Deserialize)]
struct BenchmarkFile {
    group_id: String,
    function_id: Option<String>,
    value_str: Option<String>,
    throughput: Option<serde_json::Value>,
}

/// The parts of criterion's `new/estimates.json` the report reads. The median
/// is there whether criterion sampled linearly or flat.
#[derive(Deserialize)]
struct EstimatesFile {
    median: Estimate,
}

#[derive(Deserialize)]
struct Estimate {
    point_estimate: f64,
    confidence_interval: Interval,
}

#[derive(Deserialize)]
struct Interval {
    confidence_level: f64,
    lower_bound: f64,
    upper_bound: f64,
}

/// Every timing criterion has written under `dir`, by the ids inside its
/// `benchmark.json` files; directory names are never parsed. None if `dir`
/// doesn't exist.
///
/// # Errors
///
/// If a result can't be read.
pub fn read_timings(dir: &Path) -> Result<HashMap<Key, Timing>, String> {
    let mut files = Vec::new();
    if dir.exists() {
        find_results(dir, &mut files).map_err(|err| format!("reading {}: {err}", dir.display()))?;
    }
    files.iter().map(|path| read_timing(path)).collect()
}

/// Every `new/benchmark.json` under `dir`: criterion keeps the latest result
/// of each benchmark in `new/`, and the one before it elsewhere.
fn find_results(dir: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            find_results(&path, found)?;
        } else if path.file_name() == Some("benchmark.json".as_ref())
            && path.parent().and_then(Path::file_name) == Some("new".as_ref())
        {
            found.push(path);
        }
    }
    Ok(())
}

/// The timing in `benchmark`'s directory.
fn read_timing(benchmark: &Path) -> Result<(Key, Timing), String> {
    let id: BenchmarkFile = read_json(benchmark)?;
    let median = read_json::<EstimatesFile>(&benchmark.with_file_name("estimates.json"))?.median;
    let written = fs::metadata(benchmark)
        .and_then(|meta| meta.modified())
        .map_err(|err| format!("reading {}: {err}", benchmark.display()))?;

    let key = (
        id.group_id,
        id.function_id.unwrap_or_default(),
        id.value_str.unwrap_or_default(),
    );
    let timing = Timing {
        median_ns: median.point_estimate,
        lower_ns: median.confidence_interval.lower_bound,
        upper_ns: median.confidence_interval.upper_bound,
        confidence_level: median.confidence_interval.confidence_level,
        input_rows: id.throughput.and_then(|t| t.get("Elements")?.as_u64()),
        written_unix_ms: record::unix_ms(written),
    };
    Ok((key, timing))
}

/// The JSON file at `path`.
fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let json = fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
    serde_json::from_slice(&json).map_err(|err| format!("parsing {}: {err}", path.display()))
}

/// How far the machine's speed moved while each group ran: |end / start − 1|
/// for its sentinel, the same work timed before the group's first case and
/// after its last. A group whose sentinel has no timing is left out.
pub fn drifts(results: &Results) -> Vec<(&'static str, f64)> {
    cases::GROUPS
        .iter()
        .filter_map(|group| {
            let [start, end] = evidence::sentinel_keys(group.name);
            let median = |key: &Key| Some(results.timings.get(key)?.median_ns);
            Some((group.name, (median(&end)? / median(&start)? - 1.0).abs()))
        })
        .collect()
}

/// The largest of the groups' [`drifts`].
pub fn drift(results: &Results) -> Option<f64> {
    drifts(results)
        .into_iter()
        .map(|(_, drift)| drift)
        .max_by(f64::total_cmp)
}

/// Every case whose timing is [noisy](Timing::noisy).
pub fn noisy(results: &Results) -> Vec<String> {
    cases::cases()
        .filter(|case| results.timings.get(&key(case)).is_some_and(Timing::noisy))
        .map(|case| case.to_string())
        .collect()
}

/// A table, and the file it's written to.
pub struct Table {
    pub file: &'static str,
    pub kind: Kind,
    /// What the caption says about this table alone, if anything.
    pub note: Option<&'static str>,
}

/// How a table is laid out.
pub enum Kind {
    /// A row per parameter, headed `rows`, and a column per case.
    Grid {
        rows: &'static str,
        columns: &'static [Column],
        metric: Metric,
    },
    /// A row per case, headed `rows`: its peak, the peak over the input's
    /// bytes, and its allocations.
    Profile {
        rows: &'static str,
        cases: &'static [(Column, usize)],
    },
}

/// What a grid's cells show.
#[derive(Clone, Copy)]
pub enum Metric {
    /// The median time per input row.
    Time,
    /// The peak bytes.
    Peak,
}

/// A group's algorithm, under a label.
pub struct Column {
    pub group: &'static str,
    pub algorithm: Algorithm,
    pub label: &'static str,
}

const fn column(group: &'static str, algorithm: Algorithm, label: &'static str) -> Column {
    Column {
        group,
        algorithm,
        label,
    }
}

/// The tables a post can paste. Each has at most 5 data columns: a post's
/// tables don't scroll sideways, so wider ones overflow a phone. That's also
/// why time and memory are separate tables.
pub const TABLES: &[Table] = {
    use Algorithm::*;
    &[
        Table {
            file: "one_to_one.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("one_to_one", NestedLoop, "nested loop"),
                    column("one_to_one", Hash, "hash"),
                    column("one_to_one", HashSorted, "hash (sorted input)"),
                    column("one_to_one", Merge, "merge (sorted input)"),
                    column("one_to_one", SortMerge, "sort-merge"),
                ],
            },
        },
        Table {
            file: "sorted_pair.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("one_to_one", Merge, "merge (sorted input)"),
                    column("one_to_one", Leapfrog, "leapfrog (sorted input)"),
                ],
            },
        },
        Table {
            file: "small_side.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows on the small side",
                metric: Metric::Time,
                columns: &[
                    column("small_side", NestedLoop, "nested loop"),
                    column("small_side", Hash, "hash"),
                    column("small_side", HashSipHash, "hash, SipHash"),
                ],
            },
        },
        Table {
            file: "both_small.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("both_small", NestedLoop, "nested loop"),
                    column("both_small", Hash, "hash"),
                    column("both_small", HashSipHash, "hash, SipHash"),
                ],
            },
        },
        Table {
            file: "hash_variants.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("hash_variants", Hash, "foldhash"),
                    column("hash_variants", HashSipHash, "SipHash"),
                ],
            },
        },
        Table {
            file: "binary_memory.md",
            note: None,
            kind: Kind::Profile {
                rows: "at 1M rows a side",
                cases: &[
                    (column("one_to_one", Hash, "hash"), 1 << 20),
                    (column("one_to_one", Merge, "merge (sorted input)"), 1 << 20),
                    (column("one_to_one", SortMerge, "sort-merge"), 1 << 20),
                    (
                        column("one_to_one", Leapfrog, "leapfrog (sorted input)"),
                        1 << 20,
                    ),
                    (
                        column("one_to_one", NestedLoop, "nested loop, at 64K rows a side"),
                        1 << 16,
                    ),
                ],
            },
        },
        Table {
            file: "selective.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows in A and B",
                metric: Metric::Time,
                columns: SELECTIVE,
            },
        },
        Table {
            file: "selective_memory.md",
            note: Some(BINARY_PLAN_PEAKS),
            kind: Kind::Grid {
                rows: "rows in A and B",
                metric: Metric::Peak,
                columns: SELECTIVE,
            },
        },
        Table {
            file: "heap_vs_scan.md",
            note: None,
            kind: Kind::Grid {
                rows: "inputs",
                metric: Metric::Time,
                columns: &[
                    column("multiway_dense", Heap, "dense heap"),
                    column("multiway_dense", Scan, "dense scan"),
                    column("multiway_sparse", Heap, "sparse heap"),
                    column("multiway_sparse", Scan, "sparse scan"),
                ],
            },
        },
        Table {
            file: "leapfrog.md",
            note: None,
            kind: Kind::Grid {
                rows: "inputs",
                metric: Metric::Time,
                columns: &[
                    column("multiway_dense", Heap, "dense heap"),
                    column("multiway_dense", Leapfrog, "dense leapfrog"),
                    column("multiway_sparse", Heap, "sparse heap"),
                    column("multiway_sparse", Leapfrog, "sparse leapfrog"),
                ],
            },
        },
        Table {
            file: "leapfrog_selective.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows in A and B",
                metric: Metric::Time,
                columns: &[
                    column("multiway_selective", Heap, "heap"),
                    column("multiway_selective", Leapfrog, "leapfrog"),
                ],
            },
        },
    ]
};

/// What the binary plans' peaks include.
const BINARY_PLAN_PEAKS: &str = "The binary plans' peaks are bytes requested: they include \
     the unfilled capacity of each intermediate Vec, which grows by doubling, and during a \
     realloc that moves one, the block it moves from.";

/// The heap against the two binary plans, in time and in memory.
const SELECTIVE: &[Column] = &[
    column("multiway_selective", Algorithm::Heap, "heap"),
    column(
        "multiway_selective",
        Algorithm::CascadePlanned,
        "binary plan (smallest first)",
    ),
    column(
        "multiway_selective",
        Algorithm::CascadeUnplanned,
        "binary plan (as given)",
    ),
];

/// Writes every table into `out`, returning the files it wrote.
///
/// # Errors
///
/// If `out` can't be written.
pub fn write(results: &Results, out: &Path) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(out)?;
    render(results)
        .into_iter()
        .map(|(file, markdown)| {
            let path = out.join(file);
            fs::write(&path, markdown)?;
            Ok(path)
        })
        .collect()
}

/// Every file's name and Markdown: the [`TABLES`], then every case in long
/// form (`all.md`), then the [`FACTS`] (`facts.md`).
pub fn render(results: &Results) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = TABLES
        .iter()
        .map(|table| (table.file.to_owned(), table.render(results)))
        .collect();
    files.extend(
        results
            .cache
            .as_ref()
            .map_or(Vec::new(), |cache| cache_tables(results, cache)),
    );
    files.push(("all.md".to_owned(), all(results)));
    files.push(("facts.md".to_owned(), facts(results)));
    files
}

impl Table {
    fn name(&self) -> &str {
        self.file.trim_end_matches(".md")
    }

    fn render(&self, results: &Results) -> String {
        let note = self.note.map_or(String::new(), |note| format!(" {note}"));
        match &self.kind {
            Kind::Grid {
                rows,
                columns,
                metric: Metric::Time,
            } => {
                let group = cases::group(columns[0].group);
                let mut noisy = Vec::new();
                let body = grid(rows, columns, |c, param| {
                    let timing = results.timing(c.group, c.algorithm, param);
                    let per_row = results.per_row(c.group, c.algorithm, param);
                    match (timing, per_row) {
                        (Some(timing), Some(ns)) if timing.noisy() => {
                            noisy.push(format!("{} at {}", c.label, param_label(param)));
                            format!("{}†", sig3(ns))
                        }
                        (_, Some(ns)) => sig3(ns),
                        _ => MISSING.to_owned(),
                    }
                });
                let what = format!(
                    "Median of criterion samples, in ns per input row, counting {}.{note}",
                    group.workload.input_rows()
                );
                let cells = columns.iter().flat_map(|c| {
                    let params = cases::group(c.group).params;
                    params.iter().map(|&param| (c.group, c.algorithm, param))
                });
                let reads = reads(results, cells);
                let source = Source::of(columns);
                let caption = time_caption(results, &source, &what, &reads, NOISY, &noisy);
                page(self.name(), results, &source, &body, &caption)
            }
            Kind::Grid {
                rows,
                columns,
                metric: Metric::Peak,
            } => {
                let body = grid(rows, columns, |c, param| {
                    let profile = results.profile(c.group, c.algorithm, param);
                    profile.map_or(MISSING.to_owned(), |p| bytes(p.usage.peak_bytes as u64))
                });
                let caption = memory_caption(results, &format!("{PEAK}{note}"));
                page(self.name(), results, &Source::Memory, &body, &caption)
            }
            Kind::Profile { rows, cases } => {
                let header = [rows, "peak", "× input", "allocations"].map(str::to_owned);
                let lines = cases.iter().map(|(c, param)| {
                    let cells = match results.profile(c.group, c.algorithm, *param) {
                        Some(p) => profile_cells(p),
                        None => vec![MISSING.to_owned(); 3],
                    };
                    [vec![c.label.to_owned()], cells].concat()
                });
                let body = markdown(&header, 3, lines);
                let caption = memory_caption(results, &format!("{PEAK} {PER_INPUT}{note}"));
                page(self.name(), results, &Source::Memory, &body, &caption)
            }
        }
    }
}

/// The tables on why the hash join's cost per row climbs: its loops, what it
/// reads, what a read costs, how many reach DRAM, what those predict, and
/// where a read costs more than that prediction charges.
fn cache_tables(results: &Results, cache: &Cache) -> Vec<(String, String)> {
    // Each file names the measures its numbers come from, and cites their run.
    let file = |name: &str,
                parts: &'static [&'static str],
                header: &[&str],
                rows: Vec<Vec<String>>,
                what: String| {
        let source = Source::Cache(parts);
        let run = results.cited(&source);
        let header: Vec<String> = header.iter().map(|&cell| cell.to_owned()).collect();
        let body = markdown(&header, header.len() - 1, rows);
        let caption = format!(
            "{what} {}, macOS {}, {}, rust-algs {}.",
            run.cpu,
            run.os,
            rustc(&run.rustc),
            sha(&run)
        );
        let page = page(name, results, &source, &body, &caption);
        (format!("{name}.md"), page)
    };
    let (l2, line) = (cache.l2_bytes, cache.line_bytes);
    let modelled = || cache.phases.iter().filter(|p| p.rows_a_side >= cache::FLAT);
    let label = |p: &cache::Phases| param_label(p.rows_a_side);
    let reads = |p: &cache::Phases| cache::dram_reads(p.rows_a_side, l2, line);

    let phases = cache.phases.iter().map(|p| {
        let cells = [p.build_ns, p.probe_ns, p.grouped_probe_ns, p.whole_ns].map(sig3);
        [vec![label(p)], cells.to_vec()].concat()
    });
    let footprints = cache.phases.iter().map(|p| {
        let size = cache::footprint(p.rows_a_side);
        let cells = [size.control, size.buckets, size.rows, l2].map(|b| bytes(b as u64));
        [vec![label(p)], cells.to_vec()].concat()
    });
    let costs = cache.reads.iter().map(|r| {
        let cells = [r.chased_ns, r.overlapped_ns, r.paged_ns].map(sig3);
        [vec![param_label(r.bytes)], cells.to_vec()].concat()
    });
    let worked = cache.worked_reads.iter().map(|w| {
        let cells = [w.l2_ns, w.dram_ns, w.dram_ns - w.l2_ns].map(sig3);
        [vec![format!("{:.0}", w.instructions)], cells.to_vec()].concat()
    });
    // A row per size the absent probes ran at, a column per hashing.
    let absent_hashings = cache::ABSENT_HASHINGS.map(cache::Hashing::label);
    let mut absent_sizes: Vec<usize> = cache.absent.iter().map(|a| a.rows_a_side).collect();
    absent_sizes.dedup();
    let absent: Vec<Vec<String>> = absent_sizes
        .iter()
        .map(|&n| {
            let cells = absent_hashings.iter().map(|&hashing| {
                cache
                    .absent
                    .iter()
                    .find(|a| a.rows_a_side == n && a.hashing == hashing)
                    .map_or(MISSING.to_owned(), |a| sig3(a.ns))
            });
            [param_label(n)].into_iter().chain(cells).collect()
        })
        .collect();
    // What each column's rows run, at the largest size: it doesn't move with size.
    let absent_instructions: Vec<String> = absent_hashings
        .iter()
        .map(|&hashing| {
            cache
                .absent
                .iter()
                .rfind(|a| a.hashing == hashing)
                .map_or(MISSING.to_owned(), |a| format!("{:.0}", a.instructions))
        })
        .collect();
    // The loops' instructions a row, at the largest size.
    let loop_instructions = cache.phases.last().map_or(String::new(), |p| {
        format!(
            " The build loop retires {:.0} instructions for each row it builds, and the probe \
             loop {:.0} for each row it probes with.",
            p.build_instructions, p.probe_instructions
        )
    });
    let dram = modelled().map(|p| {
        let r = reads(p);
        let cells = [r.control, r.buckets, r.rows, r.total()].map(|x| format!("{x:.2}"));
        [vec![label(p)], cells.to_vec()].concat()
    });
    let flat = modelled().next();
    let predicted = modelled().map(|p| {
        let (base, base_reads) = flat.map_or((0.0, 0.0), |f| (f.whole_ns, reads(f).total()));
        let added = reads(p).total() - base_reads;
        let across = cache::footprint(p.rows_a_side).total();
        let (low, high) = cache
            .reads
            .iter()
            .find(|r| r.bytes >= across)
            .or(cache.reads.last())
            .map_or((0.0, 0.0), |r| (r.paged_ns, r.overlapped_ns));
        let cells = [base + added * low, base + added * high, p.whole_ns].map(sig3);
        [vec![label(p)], cells.to_vec()].concat()
    });

    // A row per size the hashers ran at, a column per hashing.
    let hashings = cache::HASHINGS.map(cache::Hashing::label);
    let mut sizes: Vec<usize> = cache.hashers.iter().map(|h| h.rows_a_side).collect();
    sizes.dedup();
    let by_hashing = |value: fn(&cache::HasherCost) -> f64| -> Vec<Vec<String>> {
        sizes
            .iter()
            .map(|&n| {
                let cells = hashings.iter().map(|&hashing| {
                    cache
                        .hashers
                        .iter()
                        .find(|h| h.rows_a_side == n && h.hashing == hashing)
                        .map_or(MISSING.to_owned(), |h| sig3(value(h)))
                });
                [param_label(n)].into_iter().chain(cells).collect()
            })
            .collect()
    };
    let hasher_header: Vec<&str> = ["rows a side"].into_iter().chain(hashings).collect();
    let hashed = "foldhash + 16 or 32 rounds mixes each hash that many more times, adding \
                  instructions but not reads. 16 or 256 at a time hashes that many keys before \
                  it inserts or looks up any of them.";

    let first = param_label(cache::FLAT);
    vec![
        file(
            "hash_phases",
            &["phases"],
            &[
                "rows a side",
                "build",
                "probe",
                "probe, matches first",
                "whole join",
            ],
            phases.collect(),
            format!(
                "Median of at least 30 calls, in ns per input row, counting both sides \
                 together, 2n. A clock reads between the hash join's two loops; the whole join \
                 also allocates and frees its table. Probe, matches first, is the probe loop of \
                 a second call, on the same rows with those that match moved to the \
                 front.{loop_instructions}"
            ),
        ),
        file(
            "hash_footprint",
            &["phases"],
            &[
                "rows a side",
                "control bytes",
                "buckets",
                "build rows",
                "L2 cache",
            ],
            footprints.collect(),
            "What the hash join reads at random: its table's control bytes and buckets, and the \
             build rows its buckets point to. The rows it scans are read in order. The L2 cache \
             is what this machine reports."
                .to_owned(),
        ),
        file(
            "read_cost",
            &["reads"],
            &[
                "working set",
                "one at a time",
                "overlapped",
                "overlapped, few pages",
            ],
            costs.collect(),
            format!(
                "Median of five runs, in ns per read of a {line}-byte cache line in random \
                 order. One at a time, each read's address comes from the last. Overlapped, \
                 addresses come from a list, so the core runs as many as it can at once. Few \
                 pages reads the same lines 1 MiB at a time, so hardly a read misses the TLB."
            ),
        ),
        file(
            "read_work",
            &["reads"],
            &["instructions a read", "in L2", "past L2", "added past L2"],
            worked.collect(),
            format!(
                "Median of five runs, in ns per read of a {line}-byte cache line in random \
                 order, overlapped. Beside each read the loop does 0, 4, 8, 12 or 16 rounds of \
                 arithmetic on its own counter, which read nothing and which no read waits on; \
                 instructions a read are what the kernel counts retired. In L2, the lines span \
                 4 MiB; past it, 1 GiB."
            ),
        ),
        file(
            "control_miss",
            &["absent"],
            &[
                "rows a side",
                absent_hashings[0],
                absent_hashings[1],
                absent_hashings[2],
            ],
            absent,
            format!(
                "Median of at least five calls, in ns per probe row, of the probe loop alone on \
                 keys the table doesn't hold, so a row reads control bytes and nothing else. \
                 foldhash + 16 or 32 rounds mixes each hash that many more times, adding \
                 instructions but not reads: a row retires {}, {} and {} instructions.",
                absent_instructions[0], absent_instructions[1], absent_instructions[2]
            ),
        ),
        file(
            "hash_reads",
            &["phases"],
            &[
                "rows a side",
                "control bytes",
                "buckets",
                "build rows",
                "all reads",
            ],
            dram.collect(),
            format!(
                "Reads per input row that reach DRAM, modelled, not measured: uniform random \
                 reads of each structure, half the keys matching, in an LRU cache of {} and \
                 {line}-byte lines, by Che's approximation.",
                bytes(l2 as u64)
            ),
        ),
        file(
            "hash_predicted",
            &["phases", "reads"],
            &[
                "rows a side",
                "predicted, low",
                "predicted, high",
                "measured",
            ],
            predicted.collect(),
            format!(
                "In ns per input row. Predicted is the whole join measured at {first} rows a \
                 side, plus each modelled DRAM read past that point at the cost of an overlapped \
                 read across the join's own footprint: low without TLB misses, high with them. \
                 Measured is the whole join."
            ),
        ),
        file(
            "hash_hashers",
            &["hashers"],
            &hasher_header,
            by_hashing(|h| h.ns),
            format!(
                "Median of at least 30 calls, in ns per input row, counting both sides \
                 together, 2n. {hashed}"
            ),
        ),
        file(
            "hash_instructions",
            &["hashers"],
            &hasher_header,
            by_hashing(|h| h.instructions),
            format!(
                "Median instructions retired per input row over the same calls, as the \
                 kernel counts them for the process. {hashed}"
            ),
        ),
        file(
            "hash_per_instruction",
            &["hashers"],
            &hasher_header,
            by_hashing(|h| h.ns / h.instructions),
            format!(
                "Each call's median time over its median instructions, in ns per \
                 instruction. {hashed}"
            ),
        ),
    ]
}

/// What a cell shows for a case that should have a result and doesn't.
const MISSING: &str = "missing";

/// A row per parameter of the first column's group, each cell from `cell`, or
/// `—` where the case doesn't run.
fn grid(rows: &str, columns: &[Column], mut cell: impl FnMut(&Column, usize) -> String) -> String {
    let header: Vec<String> = [rows]
        .into_iter()
        .chain(columns.iter().map(|c| c.label))
        .map(str::to_owned)
        .collect();
    let params = cases::group(columns[0].group).params;
    let lines = params.iter().map(|&param| {
        let cells = columns.iter().map(|c| {
            if c.algorithm.runs_at(param) {
                cell(c, param)
            } else {
                "—".to_owned()
            }
        });
        [param_label(param)].into_iter().chain(cells).collect()
    });
    markdown(&header, columns.len(), lines.collect::<Vec<Vec<String>>>())
}

/// A profile's peak, peak over its input's bytes, and allocations.
fn profile_cells(p: &MemoryCase) -> Vec<String> {
    vec![
        bytes(p.usage.peak_bytes as u64),
        sig3(p.usage.peak_bytes as f64 / p.input_bytes as f64),
        count(p.usage.allocations as u64),
    ]
}

/// Every case in long form: one table of timings and one of memory.
fn all(results: &Results) -> String {
    let header = ["case", "median", "±CI", "ns per input row"].map(str::to_owned);
    let times = cases::cases().map(|case| {
        let (group, alg, param) = (case.group.name, case.algorithm, case.param);
        let cells = match results.timing(group, alg, param) {
            Some(t) => vec![
                duration(t.median_ns),
                format!(
                    "±{}%",
                    sig3((t.upper_ns - t.lower_ns) / 2.0 / t.median_ns * 100.0)
                ),
                results
                    .per_row(group, alg, param)
                    .map_or(MISSING.to_owned(), sig3),
            ],
            None => vec![MISSING.to_owned(); 3],
        };
        [vec![case.to_string()], cells].concat()
    });
    let cells = cases::cases().map(|case| (case.group.name, case.algorithm, case.param));
    let source = Source::every_group();
    let caption = time_caption(results, &source, ALL, &reads(results, cells), NOISY, &[]);
    let time = page(
        "all",
        results,
        &source,
        &markdown(&header, 3, times),
        &caption,
    );

    let header = ["case", "peak", "× input", "allocations", "allocated"].map(str::to_owned);
    let profiled = cases::cases().map(|case| {
        let cells = match results.memory.get(&key(&case)) {
            Some(p) => [
                profile_cells(p),
                vec![bytes(p.usage.allocated_bytes as u64)],
            ]
            .concat(),
            None => vec![MISSING.to_owned(); 4],
        };
        [vec![case.to_string()], cells].concat()
    });
    let caption = memory_caption(results, &format!("{PEAK} {PER_INPUT}"));
    let memory = page(
        "all (memory)",
        results,
        &Source::Memory,
        &markdown(&header, 4, profiled),
        &caption,
    );
    format!("{time}\n{memory}")
}

/// A Markdown table: `header`, then `rows`. The first column labels the row
/// and the `numbers` after it are right-aligned; any after those are text.
fn markdown(
    header: &[String],
    numbers: usize,
    rows: impl IntoIterator<Item = Vec<String>>,
) -> String {
    let line = |cells: &[String]| format!("| {} |\n", cells.join(" | "));
    let mut table = line(header);
    let text = header.len() - 1 - numbers;
    table += &format!("|---|{}{}\n", "--:|".repeat(numbers), "---|".repeat(text));
    for row in rows {
        table += &line(&row);
    }
    table
}

/// A file's worth: the provenance comment, the table, and its caption. Both
/// cite the run `source`'s numbers came from, and any flag that let the
/// report use results it would otherwise refuse.
fn page(name: &str, results: &Results, source: &Source, table: &str, caption: &str) -> String {
    let run = results.cited(source);
    let mut flags = String::new();
    let mut caption = caption.to_owned();
    if !results.allowed.is_empty() {
        let allowed = results.allowed.join(" and ");
        flags = format!(" · {allowed}");
        caption += &format!(
            " Reported with {allowed}, so it may include results that aren't from this \
             commit's clean run."
        );
    }
    format!(
        "<!-- bench table: {name} · rust-algs {} · {} · {COMMAND}{flags} -->\n\n\
         {table}\n*{caption}*\n",
        sha(&run),
        date(run.started_unix_ms)
    )
}

/// How the numbers were taken, for a table's provenance comment.
const COMMAND: &str = "cargo run -p bench-report -- run";

/// What the long-form timing table's numbers are.
const ALL: &str = "Medians of criterion samples, the half-width of each one's 95% confidence \
                   interval, and the median per input row, counting every input's rows.";

/// What a memory table's peaks are.
const PEAK: &str = "Peak bytes live at once during one call, as requested from the allocator \
                    (not RSS), with realloc counted as a move.";

/// What a profile's ratio is.
const PER_INPUT: &str = "× input is the peak over the inputs' bytes.";

/// What † marks in a timing table.
const NOISY: &str = "a 95% confidence interval wider than ±5% of the median";

/// What leapfrog's times on the selective workload leave out: see
/// [`harness::ROTATION_ROWS`].
const LEAPFROG_SELECTIVE: &str = "On the selective workload, leapfrog reads about 0.5% of \
     the input rows, so a timed cycle read only 5K to 12K rows, which stayed in cache.";

/// How the timed calls of a table that cites `cells`, as (group, algorithm,
/// parameter), read their inputs: whether any rotated through instances, and
/// how little leapfrog reads of the selective workload's, if it cites that.
fn reads<'a>(
    results: &Results,
    cells: impl IntoIterator<Item = (&'a str, Algorithm, usize)>,
) -> String {
    let (mut rotated, mut skips) = (false, false);
    for (group, algorithm, param) in cells {
        rotated |= results.rotated(group, algorithm, param);
        skips |= algorithm == Algorithm::Leapfrog
            && matches!(cases::group(group).workload, Workload::Selective);
    }
    let rows = param_label(harness::ROTATION_ROWS as usize);
    let mut reads = if rotated {
        format!(
            "Below {rows} input rows, the timed calls cycled through instances of the inputs, \
             generated from other seeds, that held at least {rows} rows before one repeated, \
             too many for the branch predictor to learn much of. A join that reads every row \
             reads {} MiB a cycle, so a cache that holds that much served its rows.",
            harness::ROTATION_ROWS * size_of::<Row>() as u64 / (1 << 20)
        )
    } else {
        format!(
            "Every case here held at least {rows} input rows, so each timed call joined the \
             same inputs."
        )
    };
    if skips {
        reads = format!("{reads} {LEAPFROG_SELECTIVE}");
    }
    reads
}

/// The line under a timing table: `what` its numbers are, where they came
/// from, how the timed calls `reads` their inputs, and which cells are
/// `marked` †, which means `dagger`.
fn time_caption(
    results: &Results,
    source: &Source,
    what: &str,
    reads: &str,
    dagger: &str,
    marked: &[String],
) -> String {
    let run = results.cited(source);
    let mut caption = format!(
        "{what} {}, macOS {}, {}, {CRITERION}, rust-algs {}. {reads}",
        run.cpu,
        run.os,
        rustc(&run.rustc),
        sha(&run)
    );
    if !marked.is_empty() {
        caption += &format!(" † marks {dagger}: {}.", marked.join(", "));
    }
    caption
}

/// The line under a memory table: `what` its numbers are, and where they came
/// from.
fn memory_caption(results: &Results, what: &str) -> String {
    let run = results.cited(&Source::Memory);
    format!("{what} {}, rust-algs {}.", rustc(&run.rustc), sha(&run))
}

/// A number the prose quotes, computed here so none is worked out by hand.
pub struct Fact {
    pub name: &'static str,
    pub value: Value,
}

/// How a fact is computed.
pub enum Value {
    /// One quantity over another.
    Ratio(Quantity, Quantity),
    /// One quantity.
    Single(Quantity),
    /// The smallest parameter of `group` from which `faster` is faster than
    /// `slower` at every larger one, as far as the confidence intervals can
    /// tell: see [`Crossover`].
    Crossover {
        group: &'static str,
        faster: Algorithm,
        slower: Algorithm,
    },
    /// The largest of the groups' sentinel drifts: see [`drifts`].
    Drift,
}

/// A measure of one case.
pub struct Quantity {
    pub group: &'static str,
    pub algorithm: Algorithm,
    pub param: usize,
    pub measure: Measure,
}

/// What's measured.
#[derive(Clone, Copy)]
pub enum Measure {
    /// The median time, in ns.
    Median,
    /// The median time per input row, in ns.
    PerRow,
    Peak,
    Allocations,
    InputBytes,
}

const fn q(group: &'static str, algorithm: Algorithm, param: usize, measure: Measure) -> Quantity {
    Quantity {
        group,
        algorithm,
        param,
        measure,
    }
}

impl Measure {
    /// `value`, a measure of this kind, with its unit.
    fn show(self, value: f64) -> String {
        match self {
            Measure::Median | Measure::PerRow => duration(value),
            Measure::Peak | Measure::InputBytes => bytes(value as u64),
            Measure::Allocations => count(value as u64),
        }
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let measure = match self.measure {
            Measure::Median => "median".to_owned(),
            Measure::PerRow => "ns per input row".to_owned(),
            Measure::Peak => "peak".to_owned(),
            Measure::Allocations => "allocations".to_owned(),
            Measure::InputBytes => "input bytes".to_owned(),
        };
        write!(
            f,
            "{}/{}/{} {measure}",
            self.group,
            self.algorithm.name(),
            self.param
        )
    }
}

/// The facts the prose quotes, in the order it meets them.
pub const FACTS: &[Fact] = {
    use Algorithm::*;
    use Measure::*;
    const K1: usize = 1 << 10;
    const K16: usize = 1 << 14;
    const K64: usize = 1 << 16;
    const M1: usize = 1 << 20;
    const M4: usize = 1 << 22;
    const fn fact(name: &'static str, value: Value) -> Fact {
        Fact { name, value }
    }
    const fn ratio(group: &'static str, over: Algorithm, under: Algorithm, param: usize) -> Value {
        Value::Ratio(
            q(group, over, param, Median),
            q(group, under, param, Median),
        )
    }
    &[
        fact(
            "nested_over_hash_1K",
            ratio("one_to_one", NestedLoop, Hash, K1),
        ),
        fact(
            "nested_over_hash_64K",
            ratio("one_to_one", NestedLoop, Hash, K64),
        ),
        fact(
            "hash_ns_16K",
            Value::Single(q("one_to_one", Hash, K16, PerRow)),
        ),
        fact(
            "hash_ns_1M",
            Value::Single(q("one_to_one", Hash, M1, PerRow)),
        ),
        fact(
            "hash_ns_4M",
            Value::Single(q("one_to_one", Hash, M4, PerRow)),
        ),
        fact(
            "hash_4M_over_16K_per_row",
            Value::Ratio(
                q("one_to_one", Hash, M4, PerRow),
                q("one_to_one", Hash, K16, PerRow),
            ),
        ),
        fact(
            "hash_peak_over_input_1M",
            Value::Ratio(
                q("one_to_one", Hash, M1, Peak),
                q("one_to_one", Hash, M1, InputBytes),
            ),
        ),
        fact(
            "sort_merge_over_hash_16K",
            ratio("one_to_one", SortMerge, Hash, K16),
        ),
        fact(
            "sort_merge_over_hash_4M",
            ratio("one_to_one", SortMerge, Hash, M4),
        ),
        fact("merge_over_hash_4M", ratio("one_to_one", Merge, Hash, M4)),
        fact(
            "merge_over_hash_sorted_1M",
            ratio("one_to_one", Merge, HashSorted, M1),
        ),
        fact(
            "hash_sorted_over_hash_1M",
            ratio("one_to_one", HashSorted, Hash, M1),
        ),
        fact(
            "small_m1_nested_over_hash",
            ratio("small_side", NestedLoop, Hash, 1),
        ),
        fact(
            "hash_nested_crossover_small_m",
            Value::Crossover {
                group: "small_side",
                faster: Hash,
                slower: NestedLoop,
            },
        ),
        fact(
            "siphash_nested_crossover_small_m",
            Value::Crossover {
                group: "small_side",
                faster: HashSipHash,
                slower: NestedLoop,
            },
        ),
        fact(
            "hash_nested_crossover_both_small_n",
            Value::Crossover {
                group: "both_small",
                faster: Hash,
                slower: NestedLoop,
            },
        ),
        fact(
            "siphash_nested_crossover_both_small_n",
            Value::Crossover {
                group: "both_small",
                faster: HashSipHash,
                slower: NestedLoop,
            },
        ),
        fact(
            "siphash_over_foldhash_16K",
            ratio("hash_variants", HashSipHash, Hash, K16),
        ),
        fact(
            "siphash_over_foldhash_1M",
            ratio("hash_variants", HashSipHash, Hash, M1),
        ),
        fact(
            "siphash_over_foldhash_4M",
            ratio("hash_variants", HashSipHash, Hash, M4),
        ),
        fact(
            "unplanned_over_planned_1M",
            ratio("multiway_selective", CascadeUnplanned, CascadePlanned, M1),
        ),
        fact(
            "unplanned_peak_over_planned_1M",
            Value::Ratio(
                q("multiway_selective", CascadeUnplanned, M1, Peak),
                q("multiway_selective", CascadePlanned, M1, Peak),
            ),
        ),
        fact(
            "unplanned_peak_over_input_1M",
            Value::Ratio(
                q("multiway_selective", CascadeUnplanned, M1, Peak),
                q("multiway_selective", CascadeUnplanned, M1, InputBytes),
            ),
        ),
        fact(
            "unplanned_over_heap_1M",
            ratio("multiway_selective", CascadeUnplanned, Heap, M1),
        ),
        fact(
            "planned_over_heap_1M",
            ratio("multiway_selective", CascadePlanned, Heap, M1),
        ),
        fact(
            "heap_scan_crossover_dense_k",
            Value::Crossover {
                group: "multiway_dense",
                faster: Heap,
                slower: Scan,
            },
        ),
        fact(
            "heap_over_scan_dense_k64",
            ratio("multiway_dense", Heap, Scan, 64),
        ),
        fact(
            "heap_scan_crossover_sparse_k",
            Value::Crossover {
                group: "multiway_sparse",
                faster: Heap,
                slower: Scan,
            },
        ),
        fact(
            "scan_over_heap_sparse_k64",
            ratio("multiway_sparse", Scan, Heap, 64),
        ),
        fact(
            "heap_over_leapfrog_dense_k64",
            ratio("multiway_dense", Heap, Leapfrog, 64),
        ),
        fact(
            "heap_over_leapfrog_sparse_k64",
            ratio("multiway_sparse", Heap, Leapfrog, 64),
        ),
        fact(
            "heap_over_leapfrog_selective_1M",
            ratio("multiway_selective", Heap, Leapfrog, M1),
        ),
        fact("sentinel_drift", Value::Drift),
    ]
};

impl Fact {
    /// The cases whose timings it reads, as (group, algorithm, parameter).
    fn timed(&self) -> Vec<(&'static str, Algorithm, usize)> {
        let quantities = match &self.value {
            Value::Ratio(a, b) => vec![a, b],
            Value::Drift => vec![],
            Value::Single(a) => vec![a],
            Value::Crossover {
                group,
                faster,
                slower,
            } => {
                let params = cases::group(group).params.iter();
                return params
                    .flat_map(|&param| [(*group, *faster, param), (*group, *slower, param)])
                    .collect();
            }
        };
        quantities
            .into_iter()
            .filter(|q| matches!(q.measure, Measure::Median | Measure::PerRow))
            .map(|q| (q.group, q.algorithm, q.param))
            .collect()
    }

    /// Its value, its numerator and its denominator, as table cells, and
    /// whether the value is unsure: it reads a noisy timing, or it's a
    /// crossover the confidence intervals don't settle.
    fn cells(&self, results: &Results) -> ([String; 3], bool) {
        let missing = || MISSING.to_owned();
        match &self.value {
            Value::Ratio(over, under) => {
                let ratio = results.quantity(over).zip(results.quantity(under));
                let value = ratio.map_or_else(missing, |(o, u)| format!("{}×", sig3(o / u)));
                let unsure = results.noisy(over) || results.noisy(under);
                ([value, over.to_string(), under.to_string()], unsure)
            }
            Value::Single(quantity) => {
                let value = results
                    .quantity(quantity)
                    .map_or_else(missing, |v| quantity.measure.show(v));
                let unsure = results.noisy(quantity);
                ([value, quantity.to_string(), "—".to_owned()], unsure)
            }
            Value::Crossover {
                group,
                faster,
                slower,
            } => {
                let crossover = crossover(results, group, *faster, *slower);
                let unsure = matches!(crossover, Some(Crossover::Between(..)));
                let value = crossover.map_or_else(missing, |c| c.to_string());
                let param = cases::group(group).workload.param();
                let id = |alg: &Algorithm| format!("{group}/{}/{param} median", alg.name());
                ([value, id(faster), id(slower)], unsure)
            }
            Value::Drift => {
                let value = drift(results)
                    .map_or_else(missing, |drift| format!("{}%", sig3(drift * 100.0)));
                let over = "the largest drift of any group's sentinel".to_owned();
                ([value, over, "—".to_owned()], false)
            }
        }
    }
}

/// Where one algorithm overtakes another as a group's parameter grows.
#[derive(Debug, PartialEq, Eq)]
pub enum Crossover {
    /// At this parameter: below it the first is clearly slower, and from it on
    /// clearly faster, their confidence intervals apart at every parameter.
    /// `None` if it's clearly slower at every one.
    Settled(Option<usize>),
    /// Somewhere from the first parameter where it isn't clearly slower, to
    /// the one from which it's clearly faster (`None` if it never settles):
    /// in between, the intervals overlap or a timing is noisy.
    Between(usize, Option<usize>),
}

impl fmt::Display for Crossover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Crossover::Settled(Some(at)) => write!(f, "{}", param_label(*at)),
            Crossover::Settled(None) => write!(f, "none"),
            Crossover::Between(from, Some(to)) => {
                write!(f, "{}–{}", param_label(*from), param_label(*to))
            }
            Crossover::Between(from, None) => write!(f, "≥ {}", param_label(*from)),
        }
    }
}

/// Where `faster` overtakes `slower` across `group`'s parameters, or `None` if
/// a timing is missing.
fn crossover(
    results: &Results,
    group: &str,
    faster: Algorithm,
    slower: Algorithm,
) -> Option<Crossover> {
    let params = cases::group(group).params;
    // At each parameter, whether `faster` is clearly faster, clearly slower,
    // or neither (None).
    let mut clearly = Vec::new();
    for &param in params {
        let (f, s) = (
            results.timing(group, faster, param)?,
            results.timing(group, slower, param)?,
        );
        clearly.push(if f.noisy() || s.noisy() {
            None
        } else if f.upper_ns < s.lower_ns {
            Some(true)
        } else if f.lower_ns > s.upper_ns {
            Some(false)
        } else {
            None
        });
    }
    // The first parameter where it isn't clearly slower, and the first from
    // which it's clearly faster at every one.
    let from = clearly.iter().position(|c| *c != Some(false));
    let settled = clearly
        .iter()
        .rposition(|c| *c != Some(true))
        .map_or(0, |i| i + 1);
    let to = params.get(settled).copied();
    Some(match from {
        None => Crossover::Settled(None),
        Some(i) if Some(params[i]) == to => Crossover::Settled(to),
        Some(i) => Crossover::Between(params[i], to),
    })
}

/// The facts as a table. Its caption cites the timing run, and the memory run
/// the peaks and allocations come from.
fn facts(results: &Results) -> String {
    let header = ["fact", "value", "numerator", "denominator"].map(str::to_owned);
    let mut unsure = Vec::new();
    let rows: Vec<Vec<String>> = FACTS
        .iter()
        .map(|fact| {
            let ([mut value, over, under], marked) = fact.cells(results);
            if marked {
                value += "†";
                unsure.push(fact.name.to_owned());
            }
            vec![fact.name.to_owned(), value, over, under]
        })
        .collect();
    let what = "Each value is the numerator's measure over the denominator's, or the \
                numerator's alone where there's no denominator. The drift is the largest \
                |end / start − 1| of any group's sentinel, the same work timed before the \
                group's first case and after its last, and a crossover the smallest parameter from which the \
                numerator is faster than the denominator at every larger one, or the range it \
                lies in where their confidence intervals overlap. Times are from";
    let reads = reads(results, FACTS.iter().flat_map(Fact::timed));
    let source = Source::every_group();
    let mut caption = time_caption(results, &source, what, &reads, UNSURE, &unsure);
    caption += &format!(
        " Peaks, allocations and input bytes are from the memory profiles at rust-algs {}.",
        sha(&results.cited(&Source::Memory))
    );
    page(
        "facts",
        results,
        &source,
        &markdown(&header, 1, rows),
        &caption,
    )
}

/// What † marks in the facts.
const UNSURE: &str = "a fact that reads a timing whose 95% confidence interval is wider than \
                      ±5% of its median, or a crossover the intervals don't settle";

/// A time in ns, as ns, µs, ms or s, to three significant figures.
pub fn duration(ns: f64) -> String {
    let (value, unit) = match ns {
        ..1e3 => (ns, "ns"),
        ..1e6 => (ns / 1e3, "µs"),
        ..1e9 => (ns / 1e6, "ms"),
        _ => (ns / 1e9, "s"),
    };
    format!("{} {unit}", sig3(value))
}

/// A parameter as a row label: 64 for 64, 1K for 1,024, 4M for 4,194,304, 2G
/// for 2,147,483,648.
pub fn param_label(param: usize) -> String {
    const K: usize = 1 << 10;
    const M: usize = 1 << 20;
    const G: usize = 1 << 30;
    match param {
        p if p >= G && p % G == 0 => format!("{}G", p / G),
        p if p >= M && p % M == 0 => format!("{}M", p / M),
        p if p >= K && p % K == 0 => format!("{}K", p / K),
        p => p.to_string(),
    }
}

/// A run's commit, abbreviated, marked if the tree was dirty.
fn sha(run: &Run) -> String {
    let dirty = if run.dirty { "-dirty" } else { "" };
    format!("{}{dirty}", short(&run.commit))
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

/// `rustc 1.98.1` from `rustc -V`'s `rustc 1.98.1 (48a229cea 2026-09-01)`.
fn rustc(version: &str) -> String {
    version
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The UTC date and time of `unix_ms`, to the second.
fn moment(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000 % 86_400;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    format!("{} {hours:02}:{minutes:02}:{seconds:02} UTC", date(unix_ms))
}

/// The UTC date of `unix_ms`, as YYYY-MM-DD (Hinnant's `civil_from_days`).
fn date(unix_ms: u64) -> String {
    let days = (unix_ms / 86_400_000) as i64 + 719_468;
    let (era, day_of_era) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bench_plan::alloc::Usage;
    use bench_plan::profile::Memory;
    use evidence::{Kind as TargetKind, Written};

    const RUSTC: &str = "rustc 1.98.1 (48a229cea 2026-09-01)";

    /// A run of `target`, of the binary [`targets`] says it builds to now.
    fn run(target: &str, started_unix_ms: u64, commit: &str, dirty: bool, rustc: &str) -> Run {
        Run {
            target: target.to_owned(),
            started_unix_ms,
            commit: commit.to_owned(),
            dirty,
            rustc: rustc.to_owned(),
            cpu: "Apple M6".to_owned(),
            os: "27.0".to_owned(),
            args: Vec::new(),
            binary: format!("{target}-bin"),
        }
    }

    /// A run of `target`, of `binary`.
    fn of(target: &str, started_unix_ms: u64, binary: &str) -> Run {
        Run {
            binary: binary.to_owned(),
            ..run(target, started_unix_ms, "aaaaaaa000", false, RUSTC)
        }
    }

    /// Every bench target's name and kind, in run order.
    fn names() -> Vec<(&'static str, TargetKind)> {
        cases::GROUPS
            .iter()
            .map(|group| (group.name, TargetKind::Timing))
            .chain([("memory", TargetKind::Memory)])
            .chain(cache::PARTS.map(|part| (part, TargetKind::Cache)))
            .collect()
    }

    /// Every bench target, building to the binary `run` records.
    fn targets() -> Vec<Target> {
        names()
            .into_iter()
            .map(|(name, kind)| Target {
                name: name.to_owned(),
                package: "bench",
                kind,
                executable: PathBuf::new(),
                binary: format!("{name}-bin"),
            })
            .collect()
    }

    /// A run of every target at `commit`, each starting at `started_unix_ms`.
    fn log(started_unix_ms: u64, commit: &str, dirty: bool) -> Vec<Run> {
        names()
            .into_iter()
            .map(|(name, _)| run(name, started_unix_ms, commit, dirty, RUSTC))
            .collect()
    }

    fn timing(written_unix_ms: u64) -> Timing {
        around(1000.0, written_unix_ms)
    }

    /// A timing of `median_ns` with a ±1% confidence interval.
    fn around(median_ns: f64, written_unix_ms: u64) -> Timing {
        Timing {
            median_ns,
            lower_ns: median_ns * 0.99,
            upper_ns: median_ns * 1.01,
            confidence_level: 0.95,
            input_rows: Some(1000),
            written_unix_ms,
        }
    }

    /// A timing for every case and every sentinel, written at
    /// `written_unix_ms`.
    fn every_timing(written_unix_ms: u64) -> HashMap<Key, Timing> {
        cases::GROUPS
            .iter()
            .flat_map(evidence::timing_keys)
            .map(|(_, key)| (key, timing(written_unix_ms)))
            .collect()
    }

    /// A memory profile of every case.
    fn memory() -> Memory {
        let cases = cases::cases()
            .map(|case| MemoryCase {
                group: case.group.name.to_owned(),
                algorithm: case.algorithm.name().to_owned(),
                param: case.param,
                input_rows: 1,
                input_bytes: 16,
                checksum: 0,
                usage: Usage::default(),
            })
            .collect();
        Memory {
            filter: None,
            cases,
        }
    }

    /// The cache targets' measures.
    fn cache() -> Cache {
        let phases = |rows_a_side, whole_ns| cache::Phases {
            rows_a_side,
            build_ns: whole_ns / 3.0,
            probe_ns: whole_ns * 2.0 / 3.0,
            grouped_probe_ns: whole_ns / 2.0,
            whole_ns,
            build_instructions: 56.0,
            probe_instructions: 29.0,
        };
        let cost = |bytes, ns| cache::ReadCost {
            bytes,
            chased_ns: 20.0 * ns,
            overlapped_ns: ns,
            paged_ns: ns - 1.0,
        };
        Cache {
            l2_bytes: 20 << 20,
            line_bytes: 128,
            phases: vec![
                phases(1 << 16, 3.0),
                phases(1 << 18, 4.0),
                phases(1 << 26, 15.0),
            ],
            reads: vec![cost(1 << 20, 2.0), cost(1 << 31, 5.0)],
            hashers: [1 << 14, 1 << 22]
                .into_iter()
                .flat_map(|rows_a_side| {
                    cache::HASHINGS.map(|hashing| cache::HasherCost {
                        rows_a_side,
                        hashing: hashing.label().to_owned(),
                        ns: 10.0,
                        instructions: 40.0,
                    })
                })
                .collect(),
            worked_reads: vec![
                cache::WorkedRead {
                    rounds: 0,
                    instructions: 7.2,
                    l2_ns: 1.0,
                    dram_ns: 5.0,
                },
                cache::WorkedRead {
                    rounds: 8,
                    instructions: 48.4,
                    l2_ns: 2.0,
                    dram_ns: 9.5,
                },
            ],
            absent: [1 << 20, 1 << 26]
                .into_iter()
                .flat_map(|rows_a_side| {
                    cache::ABSENT_HASHINGS.map(|hashing| cache::AbsentProbe {
                        rows_a_side,
                        hashing: hashing.label().to_owned(),
                        ns: 3.0,
                        instructions: 23.4,
                    })
                })
                .collect(),
        }
    }

    /// The run log `runs`, `timings`, and every other result written at
    /// `written_unix_ms`.
    fn evidence(runs: Vec<Run>, timings: HashMap<Key, Timing>, written_unix_ms: u64) -> Evidence {
        let cache = cache();
        // As steady as the timings' sentinels, which `timing` makes 1,000 ns.
        let sentinel = cache::Sentinel {
            start_ns: 1000.0,
            end_ns: 1000.0,
        };
        Evidence {
            runs,
            timings,
            memory: Some(Written {
                value: memory(),
                written_unix_ms,
            }),
            phases: Some(Written {
                value: cache::PhasesPart {
                    sentinel,
                    l2_bytes: cache.l2_bytes,
                    line_bytes: cache.line_bytes,
                    phases: cache.phases,
                },
                written_unix_ms,
            }),
            reads: Some(Written {
                value: cache::ReadsPart {
                    sentinel,
                    line_bytes: cache.line_bytes,
                    reads: cache.reads,
                    worked_reads: cache.worked_reads,
                },
                written_unix_ms,
            }),
            hashers: Some(Written {
                value: cache::HashersPart {
                    sentinel,
                    hashers: cache.hashers,
                },
                written_unix_ms,
            }),
            absent: Some(Written {
                value: cache::AbsentPart {
                    sentinel,
                    absent: cache.absent,
                },
                written_unix_ms,
            }),
        }
    }

    /// Results with `timings`, every target cited at commit `aaaaaaa`.
    fn results(timings: HashMap<Key, Timing>) -> Results {
        Results {
            timings,
            memory: HashMap::new(),
            cache: None,
            cited: log(0, "aaaaaaa000", false)
                .into_iter()
                .map(|run| (run.target.clone(), run))
                .collect(),
            allowed: Vec::new(),
        }
    }

    /// `results` with the cache measures, cited at `commit`.
    fn with_cache(mut results: Results, commit: &str) -> Results {
        results.cache = Some(cache());
        for part in cache::PARTS {
            results
                .cited
                .insert(part.to_owned(), run(part, 0, commit, false, RUSTC));
        }
        results
    }

    const STRICT: Options = Options {
        out: PathBuf::new(),
        allow_stale: false,
        allow_dirty: false,
    };

    #[test]
    fn every_quoted_memory_number_has_a_claim() {
        use std::collections::BTreeMap;
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
                        for case in cases::group(c.group).cases() {
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
                Value::Ratio(a, b) => vec![a, b],
                Value::Drift => vec![],
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
            let data = (cases::group(name).generate)(param, 0);
            for alg in algorithms {
                assert!(
                    cases::memory_claim(alg, &data).is_some()
                        || cases::REPORTED_ONLY.contains(&alg),
                    "{name}/{}/{param} is quoted, but no claim checks it",
                    alg.name()
                );
            }
        }
    }

    #[test]
    fn a_target_is_out_of_date_until_the_binary_built_now_has_run() {
        let evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
        let mut targets = targets();
        for target in &targets {
            let why = evidence::out_of_date(target, &evidence);
            assert!(why.is_empty(), "{}: {why:?}", target.name);
        }
        // One group's definition changed, and one cache measure's code.
        for name in ["small_side", "absent"] {
            let rebuilt = targets.iter_mut().find(|t| t.name == name).unwrap();
            rebuilt.binary = "rebuilt".to_owned();
        }
        let stale: Vec<&str> = targets
            .iter()
            .filter(|target| !evidence::out_of_date(target, &evidence).is_empty())
            .map(|target| target.name.as_str())
            .collect();
        assert_eq!(stale, ["small_side", "absent"]);
    }

    #[test]
    fn a_target_that_ran_on_an_unsteady_machine_is_out_of_date() {
        let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
        let unsteady = |evidence: &Evidence| -> Vec<String> {
            targets()
                .iter()
                .filter(|target| !evidence::out_of_date(target, evidence).is_empty())
                .map(|target| target.name.clone())
                .collect()
        };
        assert!(unsteady(&evidence).is_empty());

        // The machine slowed while one_to_one ran: its sentinel ends slower.
        let [_, end] = evidence::sentinel_keys("one_to_one");
        evidence.timings.insert(end, around(1900.0, 1500));
        // multiway_dense ran slow from start to finish: no drift of its own,
        // but six times the other groups' sentinels.
        for key in evidence::sentinel_keys("multiway_dense") {
            evidence.timings.insert(key, around(6000.0, 1500));
        }
        // A cache measure 8% slower than the others is within what a steady
        // machine does; one 20% slower isn't.
        evidence.hashers.as_mut().unwrap().value.sentinel = cache::Sentinel {
            start_ns: 1080.0,
            end_ns: 1080.0,
        };
        evidence.absent.as_mut().unwrap().value.sentinel = cache::Sentinel {
            start_ns: 1200.0,
            end_ns: 1200.0,
        };
        assert_eq!(
            unsteady(&evidence),
            ["one_to_one", "multiway_dense", "absent"]
        );
        let why = |name: &str| {
            let target = targets().into_iter().find(|t| t.name == name).unwrap();
            evidence::out_of_date(&target, &evidence).join("; ")
        };
        assert!(
            why("one_to_one").contains("speed moved 90%"),
            "{}",
            why("one_to_one")
        );
        assert!(
            why("multiway_dense").contains("6.00 times"),
            "{}",
            why("multiway_dense")
        );
    }

    #[test]
    fn refuses_timings_another_build_wrote() {
        let group = cases::group("one_to_one");
        let runs = vec![
            of("one_to_one", 1000, "old"),
            of("one_to_one", 2000, "now"),
            // Another group's run doesn't write this one's timings.
            of("small_side", 2200, "old"),
            of("one_to_one", 3000, "old"),
            of("one_to_one", 4000, "now"),
        ];
        let mut evidence = evidence(runs, every_timing(4500), 4500);
        assert!(evidence.stale_timings(group, "now").is_empty());

        let ids: Vec<Case> = group.cases().collect();
        // Written by the first run of this build: still its results.
        evidence.timings.insert(key(&ids[0]), timing(2500));
        // Written by the other build, before any run, and not at all.
        evidence.timings.insert(key(&ids[1]), timing(3500));
        evidence.timings.insert(key(&ids[2]), timing(500));
        evidence.timings.remove(&key(&ids[3]));
        assert_eq!(
            evidence.stale_timings(group, "now"),
            [ids[1].to_string(), ids[2].to_string(), ids[3].to_string()]
        );
    }

    #[test]
    fn a_commit_that_leaves_a_binary_alone_leaves_its_results_standing() {
        // Everything ran at a. At b the absent probes' code changed, so that
        // target rebuilt and reran alone, and a noisy timing was rerun.
        let mut runs = log(1000, "aaaaaaa000", false);
        runs.push(Run {
            binary: "absent-rebuilt".to_owned(),
            ..run("absent", 2000, "bbbbbbb000", false, RUSTC)
        });
        runs.push(run("one_to_one", 2100, "bbbbbbb000", false, RUSTC));
        let mut timings = every_timing(1500);
        let noisy = cases::group("one_to_one").cases().next().unwrap();
        timings.insert(key(&noisy), timing(2500));
        let mut evidence = evidence(runs, timings, 1500);
        evidence.absent.as_mut().unwrap().written_unix_ms = 2500;
        let mut targets = targets();
        let absent = targets.iter_mut().find(|t| t.name == "absent").unwrap();
        absent.binary = "absent-rebuilt".to_owned();

        let (results, warnings) =
            vouch(&STRICT, &targets, evidence).expect("every binary is the one built now");
        assert!(warnings.is_empty(), "{warnings:?}");
        let files: HashMap<String, String> = render(&results).into_iter().collect();
        assert!(files["one_to_one.md"].contains("rust-algs aaaaaaa ·"));
        assert!(files["binary_memory.md"].contains("rust-algs aaaaaaa ·"));
        assert!(files["read_cost.md"].contains("rust-algs aaaaaaa ·"));
        assert!(files["control_miss.md"].contains("rust-algs bbbbbbb ·"));
        assert!(files["control_miss.md"].contains("rust-algs bbbbbbb."));
    }

    #[test]
    fn a_table_of_two_groups_cites_the_later_of_their_commits() {
        let mut results = results(every_timing(0));
        results.cited.insert(
            "multiway_sparse".to_owned(),
            run("multiway_sparse", 86_400_000, "bbbbbbb000", false, RUSTC),
        );
        let files: HashMap<String, String> = render(&results).into_iter().collect();
        assert!(files["heap_vs_scan.md"].contains("rust-algs bbbbbbb · 1970-01-02"));
        assert!(files["selective.md"].contains("rust-algs aaaaaaa ·"));
    }

    #[test]
    fn cites_the_oldest_clean_run_of_a_binary() {
        let mut runs = vec![run("one_to_one", 500, "9999999000", true, RUSTC)];
        runs.extend(log(1000, "bbbbbbb000", false));
        runs.push(run("one_to_one", 3000, "ccccccc000", false, RUSTC));
        let (results, _) = vouch(
            &STRICT,
            &targets(),
            evidence(runs, every_timing(3500), 3500),
        )
        .expect("a clean tree ran each binary");
        assert_eq!(results.cited["one_to_one"].commit, "bbbbbbb000");
    }

    #[test]
    fn a_binary_only_dirty_trees_ran_cites_no_commit() {
        let dirty = || evidence(log(1000, "aaaaaaa000", true), every_timing(1500), 1500);
        let Err(refused) = vouch(&STRICT, &targets(), dirty()) else {
            panic!("cited a commit for a dirty tree's binary");
        };
        assert!(refused.contains("only dirty trees have run"), "{refused}");
        let allowed = Options {
            allow_dirty: true,
            ..STRICT
        };
        assert!(vouch(&allowed, &targets(), dirty()).is_ok());
    }

    #[test]
    fn refuses_everything_when_nothing_has_run_and_says_how_to_run_it() {
        let Err(refused) = vouch(&STRICT, &targets(), Evidence::default()) else {
            panic!("vouched for nothing");
        };
        for (name, _) in names() {
            assert!(refused.contains(&format!("{name}: ")), "{refused}");
        }
        assert!(
            refused.contains("bench-report --release -- run"),
            "{refused}"
        );
    }

    #[test]
    fn refuses_a_smoke_runs_timings_until_they_are_rerun() {
        let small: Vec<Case> = cases::group("small_side").cases().take(3).collect();
        let small_side = |t, args: &[&str]| Run {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            ..run("small_side", t, "aaaaaaa000", false, RUSTC)
        };
        let smoke = small_side(
            2000,
            &[
                "--warm-up-time",
                "0.1",
                "--measurement-time",
                "0.2",
                "small",
            ],
        );
        let rerun = small_side(3000, &["nested_loop"]);

        let full = log(1000, "aaaaaaa000", false);
        let mut timings = every_timing(1500);
        assert!(
            vouch(
                &STRICT,
                &targets(),
                evidence(full.clone(), timings.clone(), 1500)
            )
            .is_ok()
        );

        // The smoke run's few timings are refused, by name.
        for case in &small {
            timings.insert(key(case), timing(2500));
        }
        let runs: Vec<Run> = full.iter().cloned().chain([smoke.clone()]).collect();
        let Err(refused) = vouch(&STRICT, &targets(), evidence(runs, timings.clone(), 1500)) else {
            panic!("a smoke run's timings passed");
        };
        for case in &small {
            assert!(refused.contains(&case.to_string()), "{refused}");
        }

        // Rerunning them with the default settings clears it.
        for case in &small {
            timings.insert(key(case), timing(3500));
        }
        let runs: Vec<Run> = full.into_iter().chain([smoke, rerun]).collect();
        assert!(vouch(&STRICT, &targets(), evidence(runs, timings, 1500)).is_ok());
    }

    #[test]
    fn refuses_a_confidence_level_other_than_95_percent() {
        let mut timings = every_timing(1500);
        let case = cases::cases().next().unwrap();
        timings.insert(
            key(&case),
            Timing {
                confidence_level: 0.99,
                ..timing(1500)
            },
        );
        let evidence = evidence(log(1000, "aaaaaaa000", false), timings, 1500);
        let Err(refused) = vouch(&STRICT, &targets(), evidence) else {
            panic!("a 99% interval passed");
        };
        assert!(refused.contains("other than 95%"), "{refused}");
        assert!(refused.contains(&case.to_string()), "{refused}");
    }

    #[test]
    fn facts_mark_noisy_timings_and_unsettled_crossovers() {
        let mut timings = every_timing(0);
        // ±6%, past the 5% the † rule allows.
        timings.insert(
            lookup("one_to_one", Algorithm::Hash, 1 << 22),
            Timing {
                lower_ns: 940.0,
                upper_ns: 1060.0,
                ..timing(0)
            },
        );
        // The heap is clearly slower at 2 and 4 inputs, overlaps the scan at 8,
        // and is clearly faster from 16.
        let sparse = |timings: &mut HashMap<Key, Timing>, heap_at_8| {
            for (k, heap) in [
                (2, 1100.0),
                (4, 1100.0),
                (8, heap_at_8),
                (16, 900.0),
                (32, 900.0),
                (64, 900.0),
            ] {
                let at = |alg| lookup("multiway_sparse", alg, k);
                timings.insert(at(Algorithm::Heap), around(heap, 0));
                timings.insert(at(Algorithm::Scan), around(1005.0, 0));
            }
        };
        sparse(&mut timings, 1000.0);
        let page = facts(&results(timings.clone()));
        let row = |name: &str| {
            let row = page
                .lines()
                .find(|line| line.starts_with(&format!("| {name} |")));
            row.unwrap().to_owned()
        };
        assert!(row("hash_ns_4M").contains('†'), "{}", row("hash_ns_4M"));
        assert!(row("sort_merge_over_hash_4M").contains('†'));
        assert!(!row("hash_ns_1M").contains('†'));
        assert!(row("heap_scan_crossover_sparse_k").contains("| 8–16† |"));
        assert!(page.contains("heap_scan_crossover_sparse_k."), "{page}");

        // Clearly faster at 8 too settles it there.
        sparse(&mut timings, 900.0);
        let page = facts(&results(timings));
        let row = page
            .lines()
            .find(|line| line.starts_with("| heap_scan_crossover_sparse_k |"));
        assert!(row.unwrap().contains("| 8 |"));
    }

    #[test]
    fn memory_tables_cite_the_memory_run_and_every_file_its_flags() {
        let mut results = results(every_timing(0));
        results.cited.insert(
            "memory".to_owned(),
            run("memory", 86_400_000, "bbbbbbb000", false, RUSTC),
        );
        results.allowed = vec!["--allow-stale"];
        let files: HashMap<String, String> = render(&results).into_iter().collect();

        for (file, page) in &files {
            for line in page.lines() {
                if line.starts_with("<!-- bench table") {
                    assert!(line.contains(" · --allow-stale -->"), "{file}: {line}");
                }
                if line.starts_with('*') {
                    assert!(
                        line.contains("Reported with --allow-stale"),
                        "{file}: {line}"
                    );
                }
            }
        }
        for file in ["binary_memory.md", "selective_memory.md"] {
            let page = &files[file];
            assert!(page.contains("rust-algs bbbbbbb · 1970-01-02"), "{page}");
            assert!(page.contains("rust-algs bbbbbbb."), "{page}");
            assert!(!page.contains("aaaaaaa"), "{page}");
        }
        let (time, memory) = files["all.md"].split_once("all (memory)").unwrap();
        assert!(time.contains("aaaaaaa") && !time.contains("bbbbbbb"));
        assert!(memory.contains("bbbbbbb") && !memory.contains("aaaaaaa"));
        assert!(files["facts.md"].contains("memory profiles at rust-algs bbbbbbb"));
        assert!(!files["one_to_one.md"].contains("bbbbbbb"));
    }

    #[test]
    fn cache_tables_cite_the_cache_run() {
        let results = with_cache(results(every_timing(0)), "ccccccc000");
        let files = render(&results);
        for name in [
            "hash_phases",
            "hash_footprint",
            "read_cost",
            "read_work",
            "control_miss",
            "hash_reads",
            "hash_predicted",
            "hash_hashers",
            "hash_instructions",
            "hash_per_instruction",
        ] {
            let (_, page) = files
                .iter()
                .find(|(file, _)| file == &format!("{name}.md"))
                .unwrap_or_else(|| panic!("no {name}"));
            assert!(page.contains("rust-algs ccccccc"), "{page}");
        }
    }

    #[test]
    fn the_prediction_starts_at_the_flat_join_and_stays_in_its_band() {
        let results = with_cache(results(every_timing(0)), "ccccccc000");
        let files = render(&results);
        let (_, page) = files
            .iter()
            .find(|(file, _)| file == "hash_predicted.md")
            .expect("a prediction");
        assert!(page.contains("| 256K | 4.00 | 4.00 | 4.00 |"), "{page}");
        let last = page.lines().find(|l| l.starts_with("| 64M")).expect("64M");
        let cells: Vec<f64> = last
            .split('|')
            .filter_map(|cell| cell.trim().parse().ok())
            .collect();
        assert!(cells[0] < cells[1], "{last}");
    }

    #[test]
    fn the_hasher_tables_have_a_column_per_hashing() {
        let results = with_cache(results(every_timing(0)), "ccccccc000");
        let files = render(&results);
        let page = |name: &str| {
            files
                .iter()
                .find(|(file, _)| file == &format!("{name}.md"))
                .map(|(_, page)| page.clone())
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let times = page("hash_hashers");
        assert!(
            times.contains(
                "| rows a side | foldhash | foldhash + 16 rounds | foldhash + 32 rounds | \
                 SipHash | SipHash, 16 at a time | SipHash, 256 at a time | \
                 foldhash, 256 at a time |"
            ),
            "{times}"
        );
        assert!(times.contains("| 16K | 10.0 |"), "{times}");
        assert!(times.contains("| 4M | 10.0 |"), "{times}");
        assert!(page("hash_instructions").contains("| 4M | 40.0 |"));
        assert!(page("hash_per_instruction").contains("| 4M | 0.250 |"));
    }

    #[test]
    fn the_work_and_control_tables_say_what_their_rows_run() {
        let results = with_cache(results(every_timing(0)), "ccccccc000");
        let files = render(&results);
        let page = |name: &str| {
            files
                .iter()
                .find(|(file, _)| file == &format!("{name}.md"))
                .map(|(_, page)| page.clone())
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let work = page("read_work");
        assert!(work.contains("| 7 | 1.00 | 5.00 | 4.00 |"), "{work}");
        assert!(work.contains("| 48 | 2.00 | 9.50 | 7.50 |"), "{work}");
        let control = page("control_miss");
        assert!(
            control.contains(
                "| rows a side | foldhash | foldhash + 16 rounds | foldhash + 32 rounds |"
            ),
            "{control}"
        );
        assert!(
            control.contains("| 64M | 3.00 | 3.00 | 3.00 |"),
            "{control}"
        );
        assert!(
            control.contains("a row retires 23, 23 and 23 instructions"),
            "{control}"
        );
        let phases = page("hash_phases");
        assert!(
            phases.contains("The build loop retires 56 instructions for each row it builds"),
            "{phases}"
        );
    }

    #[test]
    fn formats_three_significant_figures() {
        let formatted = [
            0.012_345, 1.2345, 12.345, 123.45, 12_345.6, 9.996, 99.96, 0.0,
        ]
        .map(sig3);
        assert_eq!(
            formatted,
            [
                "0.0123", "1.23", "12.3", "123", "12,346", "10.0", "100", "0"
            ]
        );

        let sizes = [168, 1023, 1500, 6_553_608, 52_428_808].map(bytes);
        assert_eq!(
            sizes,
            ["168 B", "1,023 B", "1.46 KiB", "6.25 MiB", "50.0 MiB"]
        );

        assert_eq!(
            [12.0, 1_234.0, 57e6, 1.5e9].map(duration),
            ["12.0 ns", "1.23 µs", "57.0 ms", "1.50 s"]
        );
        assert_eq!(
            [64, 1 << 10, 1 << 14, 1 << 22].map(param_label),
            ["64", "1K", "16K", "4M"]
        );
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(
            moment(1_792_972_800_000 + 3_723_000),
            "2026-10-26 01:02:03 UTC"
        );
    }

    #[test]
    fn a_timing_caption_says_whether_its_cases_rotated() {
        // The n-way workloads hold at least 2^20 rows, as criterion is told;
        // every other case here holds 1,000.
        let mut timings = every_timing(0);
        for case in cases::cases() {
            if matches!(case.group.workload, Workload::Dense | Workload::Sparse) {
                timings.get_mut(&key(&case)).unwrap().input_rows = Some(harness::ROTATION_ROWS);
            }
        }
        let files: HashMap<String, String> = render(&results(timings)).into_iter().collect();
        let rotated = |file: &str| files[file].contains("cycled through instances");
        let same = |file: &str| files[file].contains("each timed call joined the same inputs");
        for file in ["one_to_one.md", "selective.md", "all.md", "facts.md"] {
            assert!(rotated(file) && !same(file), "{file}");
        }
        for file in ["heap_vs_scan.md", "leapfrog.md"] {
            assert!(same(file) && !rotated(file), "{file}");
        }
        let skips = |file: &str| files[file].contains(LEAPFROG_SELECTIVE);
        for file in ["leapfrog_selective.md", "all.md", "facts.md"] {
            assert!(skips(file), "{file}");
        }
        assert!(!skips("selective.md") && !skips("leapfrog.md"));
    }

    #[test]
    fn no_table_has_more_than_five_data_columns() {
        // Every case has a timing, and nothing else, which renders every cell.
        for (file, markdown) in render(&results(every_timing(0))) {
            let lines: Vec<&str> = markdown.lines().collect();
            let headers = lines.windows(2).filter(|pair| pair[1].starts_with("|---"));
            let mut tables = 0;
            for pair in headers {
                let columns = pair[0].matches('|').count() - 1;
                assert!(columns - 1 <= 5, "{file}: {}", pair[0]);
                tables += 1;
            }
            assert!(tables > 0, "{file} has no table");
        }
    }
}
