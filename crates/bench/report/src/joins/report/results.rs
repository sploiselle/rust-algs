//! The results the tables are written from: what's on disk that the report
//! can vouch for, and what it refuses.

use super::*;

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

pub(super) fn lookup(group: &str, algorithm: Algorithm, param: usize) -> Key {
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
    pub inlining: Vec<cache::Inlining>,
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
pub(super) enum Source {
    /// These groups' timings.
    Timings(Vec<&'static str>),
    /// The memory profiles.
    Memory,
    /// These cache measures.
    Cache(&'static [&'static str]),
}

impl Source {
    /// Every group's timings.
    pub(super) fn every_group() -> Source {
        Source::Timings(cases::GROUPS.iter().map(|group| group.name).collect())
    }

    /// The groups of `columns`, each once.
    pub(super) fn of(columns: &[Column]) -> Source {
        let mut groups: Vec<&'static str> = Vec::new();
        for column in columns {
            if !groups.contains(&column.group) {
                groups.push(column.group);
            }
        }
        Source::Timings(groups)
    }

    pub(super) fn targets(&self) -> Vec<&'static str> {
        match self {
            Source::Timings(groups) => groups.clone(),
            Source::Memory => vec!["memory"],
            Source::Cache(parts) => parts.to_vec(),
        }
    }
}

/// What a file cites when none of its targets has a run to cite, which only
/// a flag lets through.
pub(super) fn unknown_run() -> Run {
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
        sources: String::new(),
        ended_unix_ms: None,
    }
}

impl Results {
    /// The run a file drawing on `source` cites: of its targets' cited runs,
    /// the one that started last, whose commit holds all of their code.
    pub(super) fn cited(&self, source: &Source) -> Run {
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
    pub(super) fn noisy(&self, q: &Quantity) -> bool {
        match q.measure {
            Measure::Median | Measure::PerRow => self
                .timing(q.group, q.algorithm, q.param)
                .is_some_and(Timing::noisy),
            Measure::Peak | Measure::Allocations | Measure::InputBytes => false,
        }
    }

    pub(super) fn timing(
        &self,
        group: &str,
        algorithm: Algorithm,
        param: usize,
    ) -> Option<&Timing> {
        self.timings.get(&lookup(group, algorithm, param))
    }

    pub(super) fn profile(
        &self,
        group: &str,
        algorithm: Algorithm,
        param: usize,
    ) -> Option<&MemoryCase> {
        self.memory.get(&lookup(group, algorithm, param))
    }

    /// The median time per input row, in ns.
    pub(super) fn per_row(&self, group: &str, algorithm: Algorithm, param: usize) -> Option<f64> {
        let timing = self.timing(group, algorithm, param)?;
        let rows = timing
            .input_rows
            .or_else(|| Some(self.profile(group, algorithm, param)?.input_rows))?;
        Some(timing.median_ns / rows as f64)
    }

    /// Whether a case's timed calls rotated through instances of its input,
    /// by the input rows criterion was given.
    pub(super) fn rotated(&self, group: &str, algorithm: Algorithm, param: usize) -> bool {
        self.timing(group, algorithm, param)
            .and_then(|timing| timing.input_rows)
            .is_some_and(|rows| harness::rotation(rows) > 1)
    }

    pub(super) fn quantity(&self, q: &Quantity) -> Option<f64> {
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
pub fn load(options: &Options, targets: &[Target]) -> Result<(Results, Vec<String>), String> {
    vouch(options, targets, evidence::gather()?)
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
        let what = match evidence::kind_of(&target.name) {
            Some(evidence::Kind::Timing) => {
                check_sampling(cases::group(&target.name), &evidence, &mut problems);
                format!("{}'s timings", target.name)
            }
            Some(evidence::Kind::Memory) => "the memory profiles".to_owned(),
            Some(evidence::Kind::Cache) => format!("the {} measures", target.name),
            None => continue,
        };
        let run = match evidence.log.cited(target, &evidence.written(target)) {
            Ok(run) => run,
            Err(NoCommit) => {
                problems.dirty(format!(
                    "{what} were measured from source no clean tree in this history has run, \
                     so no commit here holds it"
                ));
                None
            }
        };
        if let Some(run) = run {
            cited.insert(target.name.clone(), run.clone());
        }
    }
    let (warnings, allowed) = problems.into_result().map_err(|refused| {
        format!(
            "{refused}\nrun `cargo run -p bench --release -- run` to run what's out of date, or `keep` if what they measure hasn't changed"
        )
    })?;

    let cache = match (
        &evidence.phases,
        &evidence.reads,
        &evidence.hashers,
        &evidence.absent,
        &evidence.inlining,
    ) {
        (Some(phases), Some(reads), Some(hashers), Some(absent), Some(inlining)) => Some(Cache {
            l2_bytes: phases.value.l2_bytes,
            line_bytes: phases.value.line_bytes,
            phases: phases.value.phases.clone(),
            reads: reads.value.reads.clone(),
            worked_reads: reads.value.worked_reads.clone(),
            hashers: hashers.value.hashers.clone(),
            absent: absent.value.absent.clone(),
            inlining: inlining.value.inlining.clone(),
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

/// Notes the timings of `group` that criterion didn't sample as the captions
/// say: those a run with other settings wrote, and those whose confidence
/// interval isn't at 95%.
pub(super) fn check_sampling(
    group: &'static cases::Group,
    evidence: &Evidence,
    problems: &mut Problems,
) {
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
pub(super) const CONFIDENCE: f64 = 0.95;

/// The criterion flags that change how a timing is sampled or estimated. A run
/// that passed any of them took timings the captions don't describe.
pub(super) const SAMPLING_FLAGS: &[&str] = &[
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
            .and_then(|timing| evidence.log.writer(group.name, timing.written_unix_ms));
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
pub(super) struct Problems<'o> {
    pub(super) options: &'o Options,
    pub(super) allowed: Vec<String>,
    pub(super) refused: Vec<String>,
    /// The flags that allowed something.
    pub(super) used: Vec<&'static str>,
}

impl<'o> Problems<'o> {
    pub(super) fn new(options: &'o Options) -> Self {
        Self {
            options,
            allowed: Vec::new(),
            refused: Vec::new(),
            used: Vec::new(),
        }
    }

    pub(super) fn dirty(&mut self, problem: String) {
        self.note(self.options.allow_dirty, "--allow-dirty", problem);
    }

    pub(super) fn stale(&mut self, problem: String) {
        self.note(self.options.allow_stale, "--allow-stale", problem);
    }

    pub(super) fn note(&mut self, allowed: bool, flag: &'static str, problem: String) {
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
    pub(super) fn into_result(self) -> Result<(Vec<String>, Vec<&'static str>), String> {
        if self.refused.is_empty() {
            Ok((self.allowed, self.used))
        } else {
            Err(self.refused.join("\n"))
        }
    }
}

/// The memory cases by key.
pub(super) fn profiles(cases: Vec<MemoryCase>) -> HashMap<Key, MemoryCase> {
    cases
        .into_iter()
        .map(|case| (profile_key(&case), case))
        .collect()
}

/// A memory profile's [`Key`].
pub(super) fn profile_key(case: &MemoryCase) -> Key {
    (
        case.group.clone(),
        case.algorithm.clone(),
        case.param.to_string(),
    )
}

/// The parts of criterion's `new/benchmark.json` the report reads.
#[derive(Deserialize)]
pub(super) struct BenchmarkFile {
    pub(super) group_id: String,
    pub(super) function_id: Option<String>,
    pub(super) value_str: Option<String>,
    pub(super) throughput: Option<serde_json::Value>,
}

/// The parts of criterion's `new/estimates.json` the report reads. The median
/// is there whether criterion sampled linearly or flat.
#[derive(Deserialize)]
pub(super) struct EstimatesFile {
    pub(super) median: Estimate,
}

#[derive(Deserialize)]
pub(super) struct Estimate {
    pub(super) point_estimate: f64,
    pub(super) confidence_interval: Interval,
}

#[derive(Deserialize)]
pub(super) struct Interval {
    pub(super) confidence_level: f64,
    pub(super) lower_bound: f64,
    pub(super) upper_bound: f64,
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
pub(super) fn find_results(dir: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
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
pub(super) fn read_timing(benchmark: &Path) -> Result<(Key, Timing), String> {
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
pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let json = fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
    serde_json::from_slice(&json).map_err(|err| format!("parsing {}: {err}", path.display()))
}

/// How far `sentinel` moved while each group ran: |end / start − 1|, the
/// same work timed before the group's first case and after its last. A group
/// whose sentinel has no timing is left out.
pub fn drifts(results: &Results, sentinel: Sentinel) -> Vec<(&'static str, f64)> {
    cases::GROUPS
        .iter()
        .filter_map(|group| {
            let [start, end] = evidence::sentinel_keys(group.name, sentinel);
            let median = |key: &Key| Some(results.timings.get(key)?.median_ns);
            Some((group.name, (median(&end)? / median(&start)? - 1.0).abs()))
        })
        .collect()
}

/// The largest of the groups' [`drifts`].
pub fn drift(results: &Results, sentinel: Sentinel) -> Option<f64> {
    drifts(results, sentinel)
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
