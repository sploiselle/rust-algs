//! The tables: criterion's timings and the memory profiles as Markdown a post
//! can paste, each citing the commit and machine it came from.
//!
//! The report uses only results it can vouch for. It takes the latest timing
//! run and the timing runs just before it at the same commit, dirtiness and
//! rustc (the [`window`]), and refuses, naming what's wrong, unless a flag
//! allows it:
//!
//! - a dirty tree, whose code isn't any commit's (`--allow-dirty`);
//! - a case with no timing, or one written before the window started, which
//!   ran other code ([`stale`]; `--allow-stale`);
//! - a timing written by a run with other criterion settings than the
//!   captions describe, such as a smoke run's shorter measurement
//!   ([`shortened`]), or with a confidence level other than 95%
//!   (`--allow-stale`);
//! - memory profiles that are filtered, incomplete, or from another commit or
//!   rustc (`--allow-stale`), or from a dirty tree (`--allow-dirty`);
//! - cache measures from another commit or rustc (`--allow-stale`), or from a
//!   dirty tree (`--allow-dirty`).
//!
//! Rerunning a noisy group at the same commit extends the window, so its new
//! timings replace the old without invalidating the rest. Under
//! `--allow-stale`, a case with no result shows as `missing`. Every file
//! written under a flag says so, in its provenance comment and its caption.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::Row;
use crate::cache::{self, Cache};
use crate::cases::{self, Algorithm, Case, Workload};
use crate::record::{self, Memory, MemoryCase, Run};

/// The criterion version the timings were taken with, for captions.
const CRITERION: &str = "criterion 0.8.2";

/// Sentinel drift above this suggests the machine's speed changed mid-run.
pub const DRIFT_LIMIT: f64 = 0.03;

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

/// Everything the tables are made from.
pub struct Results {
    /// The window's latest run, whose commit, rustc and machine the timing
    /// tables cite.
    pub run: Run,
    /// When the window started.
    pub since_unix_ms: u64,
    pub timings: HashMap<Key, Timing>,
    /// The run that profiled memory, which the memory tables cite.
    pub memory_run: Option<Run>,
    pub memory: HashMap<Key, MemoryCase>,
    /// The cache harness's measures, which the cache tables cite.
    pub cache: Option<Cache>,
    /// The flags that let the report use results it would otherwise refuse.
    pub allowed: Vec<&'static str>,
}

/// Which results a file's numbers come from, and so which run it cites.
#[derive(Clone, Copy)]
enum Source {
    Timings,
    Memory,
    Cache,
}

impl Results {
    /// The run `source`'s numbers came from, and when its results start.
    fn cited(&self, source: Source) -> (&Run, u64) {
        let run = match source {
            Source::Timings => None,
            Source::Memory => self.memory_run.as_ref(),
            Source::Cache => self.cache.as_ref().map(|cache| &cache.run),
        };
        run.map_or((&self.run, self.since_unix_ms), |run| {
            (run, run.started_unix_ms)
        })
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
            .is_some_and(|rows| cases::rotation(rows) > 1)
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

/// Reads every result, refusing what `options` doesn't allow. Returns the
/// results, and a warning for each problem a flag allowed.
///
/// # Errors
///
/// Every problem no flag allowed, or a file that can't be read.
pub fn load(options: &Options) -> Result<(Results, Vec<String>), String> {
    let runs = record::read_runs()?;
    let timings = read_timings(&record::target_dir().join("criterion"))?;
    vouch(options, &runs, timings, read_memory()?, read_cache()?)
}

/// Checks the results against the run log, refusing what `options` doesn't
/// allow: the part of [`load`] that reads no files.
///
/// # Errors
///
/// Every problem no flag allowed.
pub fn vouch(
    options: &Options,
    runs: &[Run],
    timings: HashMap<Key, Timing>,
    memory: Option<Memory>,
    cache: Option<Cache>,
) -> Result<(Results, Vec<String>), String> {
    let window = window(runs);
    let (Some(first), Some(latest)) = (window.first(), window.last()) else {
        return Err(format!(
            "no joins run in {}: run `cargo bench -p bench` first",
            record::runs_path().display()
        ));
    };
    let mut problems = Problems::new(options);
    if latest.dirty {
        problems.dirty(format!(
            "the timings ran on a dirty tree at {}",
            short(&latest.commit)
        ));
    }

    let stale = stale(&timings, first.started_unix_ms);
    if !stale.is_empty() {
        problems.stale(format!(
            "{} timings are missing or older than the window, which started {}: {}",
            stale.len(),
            moment(first.started_unix_ms),
            some(&stale)
        ));
    }
    for (run, ids) in shortened(&window, &timings) {
        problems.stale(format!(
            "{} timings came from the run that started {} with `{}`, not the criterion \
             settings the captions describe: {}",
            ids.len(),
            moment(run.started_unix_ms),
            run.args.join(" "),
            some(&ids)
        ));
    }
    let off_level: Vec<String> = cases::cases()
        .filter(|case| {
            timings
                .get(&key(case))
                .is_some_and(|t| (t.confidence_level - CONFIDENCE).abs() > 1e-9)
        })
        .map(|case| case.to_string())
        .collect();
    if !off_level.is_empty() {
        problems.stale(format!(
            "{} timings have a confidence interval at a level other than 95%: {}",
            off_level.len(),
            some(&off_level)
        ));
    }

    match &memory {
        Some(memory) => check_memory(memory, latest, &mut problems),
        None => problems.stale(format!(
            "no memory profiles at {}: run `cargo bench -p bench --bench memory`",
            record::memory_path().display()
        )),
    }
    match &cache {
        Some(cache) => check_cache(cache, latest, &mut problems),
        None => problems.stale(format!(
            "no cache measures at {}: run `cargo bench -p bench --bench cache`",
            record::cache_path().display()
        )),
    }
    let (warnings, allowed) = problems.into_result()?;

    let (memory_run, memory) = match memory {
        Some(memory) => (Some(memory.run), profiles(memory.cases)),
        None => (None, HashMap::new()),
    };
    let results = Results {
        run: (*latest).clone(),
        since_unix_ms: first.started_unix_ms,
        timings,
        memory_run,
        memory,
        cache,
        allowed,
    };
    Ok((results, warnings))
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

/// The timings in `window` that a run with non-default criterion settings
/// (`--warm-up-time`, `--measurement-time`, `--sample-size`, `--nresamples`,
/// `--quick` or `--confidence-level`) wrote, by run.
/// Each timing belongs to the last run that started before criterion wrote
/// it, so a run whose timings were all rewritten since owns none.
pub fn shortened<'r>(
    window: &[&'r Run],
    timings: &HashMap<Key, Timing>,
) -> Vec<(&'r Run, Vec<String>)> {
    let mut by_run: Vec<(&Run, Vec<String>)> = Vec::new();
    for case in cases::cases() {
        let Some(timing) = timings.get(&key(&case)) else {
            continue;
        };
        let writer = window
            .iter()
            .rev()
            .find(|run| run.started_unix_ms <= timing.written_unix_ms);
        let Some(&run) = writer else { continue };
        if !SAMPLING_FLAGS
            .iter()
            .any(|flag| record::has_flag(&run.args, flag))
        {
            continue;
        }
        match by_run.iter_mut().find(|(r, _)| std::ptr::eq(*r, run)) {
            Some((_, ids)) => ids.push(case.to_string()),
            None => by_run.push((run, vec![case.to_string()])),
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

/// Notes what keeps `memory` from vouching for the same code as the timing
/// run `latest`.
fn check_memory(memory: &Memory, latest: &Run, problems: &mut Problems) {
    if let Some(filter) = &memory.filter {
        problems.stale(format!("the memory profiles ran filtered to {filter:?}"));
    }
    let profiled: HashSet<Key> = memory.cases.iter().map(profile_key).collect();
    let missing: Vec<String> = cases::cases()
        .filter(|case| !case.group.is_sentinel() && !profiled.contains(&key(case)))
        .map(|case| case.to_string())
        .collect();
    if !missing.is_empty() {
        problems.stale(format!(
            "{} cases have no memory profile: {}",
            missing.len(),
            some(&missing)
        ));
    }
    if memory.run.dirty {
        problems.dirty("the memory profiles ran on a dirty tree".to_owned());
    }
    if (&memory.run.commit, &memory.run.rustc) != (&latest.commit, &latest.rustc) {
        problems.stale(format!(
            "the memory profiles ran at {} with {}, the timings at {} with {}",
            short(&memory.run.commit),
            memory.run.rustc,
            short(&latest.commit),
            latest.rustc
        ));
    }
}

fn check_cache(cache: &Cache, latest: &Run, problems: &mut Problems) {
    if cache.run.dirty {
        problems.dirty("the cache measures ran on a dirty tree".to_owned());
    }
    if (&cache.run.commit, &cache.run.rustc) != (&latest.commit, &latest.rustc) {
        problems.stale(format!(
            "the cache measures ran at {} with {}, the timings at {} with {}",
            short(&cache.run.commit),
            cache.run.rustc,
            short(&latest.commit),
            latest.rustc
        ));
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

/// The first few of `ids`, and how many more there are.
fn some(ids: &[String]) -> String {
    const SHOWN: usize = 8;
    let mut list = ids[..ids.len().min(SHOWN)].join(", ");
    if ids.len() > SHOWN {
        list += &format!(" and {} more", ids.len() - SHOWN);
    }
    list
}

/// The runs whose timings the tables may use, oldest first: the latest
/// `joins` run, and the `joins` runs before it back to the first at a
/// different commit, dirtiness or rustc. Empty if there's no `joins` run.
pub fn window(runs: &[Run]) -> Vec<&Run> {
    let joins: Vec<&Run> = runs.iter().filter(|run| run.target == "joins").collect();
    let Some(latest) = joins.last() else {
        return Vec::new();
    };
    let same = |run: &Run| {
        (&run.commit, run.dirty, &run.rustc) == (&latest.commit, latest.dirty, &latest.rustc)
    };
    let start = joins
        .iter()
        .rposition(|run| !same(run))
        .map_or(0, |i| i + 1);
    joins[start..].to_vec()
}

/// Every case with no timing, or one written before `since_unix_ms`, so by
/// other code than the window's.
pub fn stale(timings: &HashMap<Key, Timing>, since_unix_ms: u64) -> Vec<String> {
    cases::cases()
        .filter(|case| {
            timings
                .get(&key(case))
                .is_none_or(|timing| timing.written_unix_ms < since_unix_ms)
        })
        .map(|case| case.to_string())
        .collect()
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

/// The memory harness's output, if it has run.
fn read_memory() -> Result<Option<Memory>, String> {
    let path = record::memory_path();
    match fs::read(&path) {
        Ok(json) => serde_json::from_slice(&json)
            .map(Some)
            .map_err(|err| format!("parsing {}: {err}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("reading {}: {err}", path.display())),
    }
}

fn read_cache() -> Result<Option<Cache>, String> {
    let path = record::cache_path();
    match fs::read(&path) {
        Ok(json) => serde_json::from_slice(&json)
            .map(Some)
            .map_err(|err| format!("parsing {}: {err}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("reading {}: {err}", path.display())),
    }
}

/// How far the machine's speed moved during the run: |end / start − 1| for
/// the sentinels, the same case timed first and last.
pub fn drift(results: &Results) -> Option<f64> {
    let median = |group: &str| Some(results.timing(group, Algorithm::Hash, 1 << 18)?.median_ns);
    Some((median("sentinel_end")? / median("sentinel_start")? - 1.0).abs())
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
                let caption = time_caption(results, &what, &reads, NOISY, &noisy);
                page(self.name(), results, Source::Timings, &body, &caption)
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
                page(self.name(), results, Source::Memory, &body, &caption)
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
                page(self.name(), results, Source::Memory, &body, &caption)
            }
        }
    }
}

/// The tables on why the hash join's cost per row climbs: its loops, what it
/// reads, what a read costs, how many reach DRAM, what those predict, and
/// where a read costs more than that prediction charges.
fn cache_tables(results: &Results, cache: &Cache) -> Vec<(String, String)> {
    let run = &cache.run;
    let from = format!(
        "{}, macOS {}, {}, rust-algs {}.",
        run.cpu,
        run.os,
        rustc(&run.rustc),
        sha(run)
    );
    let file = |name: &str, header: &[&str], rows: Vec<Vec<String>>, what: String| {
        let header: Vec<String> = header.iter().map(|&cell| cell.to_owned()).collect();
        let body = markdown(&header, header.len() - 1, rows);
        let caption = format!("{what} {from}");
        let page = page(name, results, Source::Cache, &body, &caption);
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
            &hasher_header,
            by_hashing(|h| h.ns),
            format!(
                "Median of at least 30 calls, in ns per input row, counting both sides \
                 together, 2n. {hashed}"
            ),
        ),
        file(
            "hash_instructions",
            &hasher_header,
            by_hashing(|h| h.instructions),
            format!(
                "Median instructions retired per input row over the same calls, as the \
                 kernel counts them for the process. {hashed}"
            ),
        ),
        file(
            "hash_per_instruction",
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
    let caption = time_caption(results, ALL, &reads(results, cells), NOISY, &[]);
    let time = page(
        "all",
        results,
        Source::Timings,
        &markdown(&header, 3, times),
        &caption,
    );

    let header = ["case", "peak", "× input", "allocations", "allocated"].map(str::to_owned);
    let profiled = cases::cases()
        .filter(|case| !case.group.is_sentinel())
        .map(|case| {
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
        Source::Memory,
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
fn page(name: &str, results: &Results, source: Source, table: &str, caption: &str) -> String {
    let (run, since) = results.cited(source);
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
        "<!-- bench table: {name} · rust-algs {} · {} · cargo bench -p bench{flags} -->\n\n\
         {table}\n*{caption}*\n",
        sha(run),
        date(since)
    )
}

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
/// [`cases::ROTATION_ROWS`].
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
    let rows = param_label(cases::ROTATION_ROWS as usize);
    let mut reads = if rotated {
        format!(
            "Below {rows} input rows, the timed calls cycled through instances of the inputs, \
             generated from other seeds, that held at least {rows} rows before one repeated, \
             too many for the branch predictor to learn much of. A join that reads every row \
             reads {} MiB a cycle, so a cache that holds that much served its rows.",
            cases::ROTATION_ROWS * size_of::<Row>() as u64 / (1 << 20)
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
    what: &str,
    reads: &str,
    dagger: &str,
    marked: &[String],
) -> String {
    let run = &results.run;
    let mut caption = format!(
        "{what} {}, macOS {}, {}, {CRITERION}, rust-algs {}. {reads}",
        run.cpu,
        run.os,
        rustc(&run.rustc),
        sha(run)
    );
    if !marked.is_empty() {
        caption += &format!(" † marks {dagger}: {}.", marked.join(", "));
    }
    caption
}

/// The line under a memory table: `what` its numbers are, and where they came
/// from.
fn memory_caption(results: &Results, what: &str) -> String {
    let (run, _) = results.cited(Source::Memory);
    format!("{what} {}, rust-algs {}.", rustc(&run.rustc), sha(run))
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
    /// |end / start − 1|: how far the machine's speed moved.
    Drift(Quantity, Quantity),
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
    const K256: usize = 1 << 18;
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
        fact(
            "sentinel_drift",
            Value::Drift(
                q("sentinel_end", Hash, K256, Median),
                q("sentinel_start", Hash, K256, Median),
            ),
        ),
    ]
};

impl Fact {
    /// The cases whose timings it reads, as (group, algorithm, parameter).
    fn timed(&self) -> Vec<(&'static str, Algorithm, usize)> {
        let quantities = match &self.value {
            Value::Ratio(a, b) | Value::Drift(a, b) => vec![a, b],
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
            Value::Drift(end, start) => {
                let drift = results.quantity(end).zip(results.quantity(start));
                let value = drift.map_or_else(missing, |(e, s)| {
                    format!("{}%", sig3((e / s - 1.0).abs() * 100.0))
                });
                let unsure = results.noisy(end) || results.noisy(start);
                ([value, end.to_string(), start.to_string()], unsure)
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
                numerator's alone where there's no denominator. The drift is |numerator / \
                denominator − 1|, and a crossover the smallest parameter from which the \
                numerator is faster than the denominator at every larger one, or the range it \
                lies in where their confidence intervals overlap. Times are from";
    let reads = reads(results, FACTS.iter().flat_map(Fact::timed));
    let mut caption = time_caption(results, what, &reads, UNSURE, &unsure);
    let (memory, _) = results.cited(Source::Memory);
    caption += &format!(
        " Peaks, allocations and input bytes are from the memory profiles at rust-algs {}.",
        sha(memory)
    );
    page(
        "facts",
        results,
        Source::Timings,
        &markdown(&header, 1, rows),
        &caption,
    )
}

/// What † marks in the facts.
const UNSURE: &str = "a fact that reads a timing whose 95% confidence interval is wider than \
                      ±5% of its median, or a crossover the intervals don't settle";

/// `x` to three significant figures, without rounding its integer part, with
/// commas between thousands: 0.0123, 1.23, 12.3, 123, 12,346.
pub fn sig3(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return x.to_string();
    }
    let digits = x.abs().log10().floor() as i32 + 1;
    let mut decimals = (3 - digits).max(0) as usize;
    let mut text = format!("{x:.decimals$}");
    // Rounding up can carry into a new digit, as 9.996 does to 10.00.
    if decimals > 0 && text.parse::<f64>().unwrap().abs() >= 10f64.powi(digits) {
        decimals -= 1;
        text = format!("{x:.decimals$}");
    }
    match text.split_once('.') {
        Some((integer, fraction)) => format!("{}.{fraction}", thousands(integer)),
        None => thousands(&text),
    }
}

/// An integer with commas between thousands.
pub fn count(n: u64) -> String {
    thousands(&n.to_string())
}

/// `digits`, a run of digits with an optional sign, with commas between
/// thousands.
fn thousands(digits: &str) -> String {
    let (sign, digits) = digits.split_at(usize::from(digits.starts_with('-')));
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{sign}{grouped}")
}

/// A byte count as B, KiB or MiB, to three significant figures.
pub fn bytes(n: u64) -> String {
    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    match n {
        0..KIB => format!("{} B", count(n)),
        KIB..MIB => format!("{} KiB", sig3(n as f64 / KIB as f64)),
        MIB..GIB => format!("{} MiB", sig3(n as f64 / MIB as f64)),
        _ => format!("{} GiB", sig3(n as f64 / GIB as f64)),
    }
}

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
    use crate::alloc::Usage;

    const RUSTC: &str = "rustc 1.98.1 (48a229cea 2026-09-01)";

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
        }
    }

    fn starts(window: &[&Run]) -> Vec<u64> {
        window.iter().map(|run| run.started_unix_ms).collect()
    }

    /// A 1 µs timing with a ±1% confidence interval.
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

    /// A timing for every case, written at `written_unix_ms`.
    fn every_timing(written_unix_ms: u64) -> HashMap<Key, Timing> {
        cases::cases()
            .map(|case| (key(&case), timing(written_unix_ms)))
            .collect()
    }

    /// A memory profile of every case.
    fn memory(run: Run) -> Memory {
        let cases = cases::cases()
            .filter(|case| !case.group.is_sentinel())
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
            run,
            filter: None,
            cases,
        }
    }

    /// Timing results at commit `aaaaaaa`, and nothing else.
    fn cache(run: Run) -> Cache {
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
            run,
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

    fn results(timings: HashMap<Key, Timing>) -> Results {
        Results {
            run: run("joins", 0, "aaaaaaa000", false, RUSTC),
            since_unix_ms: 0,
            timings,
            memory_run: None,
            memory: HashMap::new(),
            cache: None,
            allowed: Vec::new(),
        }
    }

    const STRICT: Options = Options {
        out: PathBuf::new(),
        allow_stale: false,
        allow_dirty: false,
    };

    #[test]
    fn the_window_stops_at_a_different_commit_dirty_tree_or_rustc() {
        let clean = |t, commit, rustc| run("joins", t, commit, false, rustc);
        // A memory run in between neither joins nor breaks the window.
        let runs = [
            clean(1, "a", "1.98"),
            clean(2, "b", "1.98"),
            run("memory", 3, "c", true, "1.97"),
            clean(4, "b", "1.98"),
        ];
        assert_eq!(starts(&window(&runs)), [2, 4]);

        let runs = [
            clean(1, "b", "1.98"),
            run("joins", 2, "b", true, "1.98"),
            clean(3, "b", "1.98"),
        ];
        assert_eq!(starts(&window(&runs)), [3]);

        let runs = [clean(1, "b", "1.97"), clean(2, "b", "1.98")];
        assert_eq!(starts(&window(&runs)), [2]);

        assert!(window(&[run("memory", 1, "a", false, "1.98")]).is_empty());
    }

    #[test]
    fn refuses_results_older_than_the_window() {
        let since = 1_000;
        let mut timings = every_timing(since + 1);
        assert!(stale(&timings, since).is_empty());

        let ids: Vec<Case> = cases::cases().collect();
        timings.insert(key(&ids[1]), timing(since - 1));
        timings.remove(&key(&ids[2]));
        assert_eq!(
            stale(&timings, since),
            [ids[1].to_string(), ids[2].to_string()]
        );
    }

    #[test]
    fn refuses_a_smoke_runs_timings_until_they_are_rerun() {
        let joins = |t, args: &[&str]| Run {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            ..run("joins", t, "aaaaaaa000", false, RUSTC)
        };
        let full = joins(1000, &["--bench"]);
        let smoke = joins(
            2000,
            &[
                "--warm-up-time",
                "0.1",
                "--measurement-time",
                "0.2",
                "small",
                "--bench",
            ],
        );
        let rerun = joins(3000, &["small_side", "--bench"]);
        let profiles = || Some(memory(run("memory", 900, "aaaaaaa000", false, RUSTC)));
        let measures = || Some(cache(run("cache", 900, "aaaaaaa000", false, RUSTC)));
        let small: Vec<Case> = cases::group("small_side").cases().take(3).collect();

        let mut timings = every_timing(1500);
        assert!(
            vouch(
                &STRICT,
                std::slice::from_ref(&full),
                timings.clone(),
                profiles(),
                measures()
            )
            .is_ok()
        );

        // The smoke run's few timings are refused, by name.
        for case in &small {
            timings.insert(key(case), timing(2500));
        }
        let runs = [full.clone(), smoke.clone()];
        let Err(refused) = vouch(&STRICT, &runs, timings.clone(), profiles(), measures()) else {
            panic!("a smoke run's timings passed");
        };
        for case in &small {
            assert!(refused.contains(&case.to_string()), "{refused}");
        }

        // Rerunning them with the default settings clears it.
        for case in &small {
            timings.insert(key(case), timing(3500));
        }
        assert!(
            vouch(
                &STRICT,
                &[full, smoke, rerun],
                timings,
                profiles(),
                measures()
            )
            .is_ok()
        );
    }

    #[test]
    fn refuses_a_confidence_level_other_than_95_percent() {
        let full = run("joins", 1000, "aaaaaaa000", false, RUSTC);
        let profiles = Some(memory(run("memory", 900, "aaaaaaa000", false, RUSTC)));
        let measures = Some(cache(run("cache", 900, "aaaaaaa000", false, RUSTC)));
        let mut timings = every_timing(1500);
        let case = cases::cases().next().unwrap();
        timings.insert(
            key(&case),
            Timing {
                confidence_level: 0.99,
                ..timing(1500)
            },
        );
        let Err(refused) = vouch(&STRICT, &[full], timings, profiles, measures) else {
            panic!("a 99% interval passed");
        };
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
        results.memory_run = Some(run("memory", 86_400_000, "bbbbbbb000", false, RUSTC));
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
        let mut results = results(every_timing(0));
        results.cache = Some(cache(run("cache", 0, "ccccccc000", false, RUSTC)));
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
        let mut results = results(every_timing(0));
        results.cache = Some(cache(run("cache", 0, "ccccccc000", false, RUSTC)));
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
        let mut results = results(every_timing(0));
        results.cache = Some(cache(run("cache", 0, "ccccccc000", false, RUSTC)));
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
        let mut results = results(every_timing(0));
        results.cache = Some(cache(run("cache", 0, "ccccccc000", false, RUSTC)));
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
    fn refuses_cache_measures_from_another_commit() {
        let joins = run("joins", 0, "aaaaaaa000", false, RUSTC);
        let timings = every_timing(1);
        let memory = memory(run("memory", 0, "aaaaaaa000", false, RUSTC));
        let other = cache(run("cache", 0, "bbbbbbb000", false, RUSTC));
        let Err(refused) = vouch(&STRICT, &[joins], timings, Some(memory), Some(other)) else {
            panic!("accepted measures from another commit");
        };
        assert!(refused.contains("the cache measures ran at"), "{refused}");
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
                timings.get_mut(&key(&case)).unwrap().input_rows = Some(cases::ROTATION_ROWS);
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
