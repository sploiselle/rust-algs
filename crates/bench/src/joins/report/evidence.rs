//! What each bench target builds to now, and what the results on disk were
//! built from: the two things the report compares.
//!
//! A result is good while the target that wrote it is still built from the
//! same source: the files it measures with, the compiler and its
//! dependencies ([`sources_of`]). Moving those files, or changing a table,
//! the runner or anything else no target measures with, leaves every result
//! standing. An edit to a measured file that doesn't change what it measures
//! leaves them standing once someone says so ([kept](crate::record::Kept)).
//! [`built`] asks cargo for every target and hashes its source;
//! [`gather`] reads the results and the logs; [`out_of_date`] says what a
//! target lacks. The runner runs what's out of date, or keeps it, and the
//! tables refuse it.
//!
//! A result also has to come from a steady, quiet machine. Every timing
//! binary and every cache measure times two [sentinels](Sentinel), work that
//! is none of theirs, before it starts and after it ends. If a sentinel's two
//! timings differ, or both are slow beside the fastest this machine has read,
//! the machine changed speed or was busy with something else, and what the
//! target measured is some other machine's: [`Evidence::unsteady`]. Such a
//! target counts as out of date.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::joins::cache::{AbsentPart, HashersPart, InliningPart, PhasesPart, ReadsPart};
use crate::joins::cases::{self, Group};
use crate::joins::harness::Sentinel;
use crate::joins::profile::Memory;
use crate::paths;
use serde::de::DeserializeOwned;

use super::{Key, Timing, key, read_timings};
use crate::record::{self, Checked, Kept, Run};

/// What a bench target measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A group's timings: the target is named for the group.
    Timing,
    /// Every case's memory profile.
    Memory,
    /// One of the hash join's cache measures: the target is named for it.
    Cache,
}

/// A bench target, as cargo builds it now.
#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    pub kind: Kind,
    pub executable: PathBuf,
    /// The executable's hash: see [`record::hash_file`].
    pub binary: String,
    /// What it measures, as a hash of its source: see [`sources_of`].
    pub sources: String,
}

/// What every join's bench target is built from: the joins, and what gives
/// them their inputs and takes their output. Paths are from the workspace's
/// root; a directory stands for every file under it.
const SHARED: [&str; 6] = [
    "crates/algs/src/join",
    "crates/bench/src/joins/mod.rs",
    "crates/bench/src/joins/harness.rs",
    "crates/bench/src/joins/workload.rs",
    "crates/bench/src/joins/timed.rs",
    "crates/bench/src/joins/baseline.rs",
];

/// What no bench target's results depend on: the report, the runner, the
/// logs, and where and how things are written. A change here reruns nothing.
pub const NOT_MEASURED: [&str; 7] = [
    "crates/bench/src/lib.rs",
    "crates/bench/src/main.rs",
    "crates/bench/src/record.rs",
    "crates/bench/src/paths.rs",
    "crates/bench/src/fmt.rs",
    "crates/bench/src/joins/report",
    "crates/bench/tests",
];

/// The files the bench target called `name` measures with, beside the
/// compiler and its dependencies: a change to one of them may change its
/// results, and a change to any other file can't.
pub fn measured(name: &str, kind: Kind) -> Vec<String> {
    let own: Vec<String> = match kind {
        Kind::Timing => vec![
            "crates/bench/benches/joins/support".to_owned(),
            format!("crates/bench/benches/joins/{name}.rs"),
            format!("crates/bench/benches/joins/groups/{name}.rs"),
        ],
        Kind::Memory => vec![
            "crates/bench/src/joins/cases.rs".to_owned(),
            "crates/bench/src/joins/profile.rs".to_owned(),
            "crates/bench/src/alloc.rs".to_owned(),
            "crates/bench/benches/joins/memory.rs".to_owned(),
            "crates/bench/benches/joins/groups".to_owned(),
        ],
        Kind::Cache => vec![
            "crates/bench/src/joins/cache".to_owned(),
            format!("crates/bench/benches/joins/cache/{name}.rs"),
        ],
    };
    SHARED
        .iter()
        .map(|path| (*path).to_owned())
        .chain(own)
        .collect()
}

/// Every file at or under `path`, which is a file or a directory.
///
/// # Errors
///
/// If it can't be read.
pub fn files_under(path: &Path) -> io::Result<Vec<PathBuf>> {
    if !path.is_dir() {
        return Ok(vec![path.to_owned()]);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(path)? {
        files.extend(files_under(&entry?.path())?);
    }
    Ok(files)
}

/// The lines of `manifest`'s tables whose headers `wanted` picks: what
/// bears on a build, without the comments around it or, in a package's, the
/// list of targets, which grows when one is added.
fn tables(manifest: &str, wanted: impl Fn(&str) -> bool) -> String {
    let mut inside = false;
    let mut lines = String::new();
    for line in manifest.lines() {
        if line.starts_with('[') {
            inside = wanted(line);
        }
        if inside && !line.trim_start().starts_with('#') {
            lines.push_str(line);
            lines.push('\n');
        }
    }
    lines
}

/// What the bench target called `name` measures, as one hash: of the
/// contents of the files it [measures with](measured), the compiler, the
/// workspace's profiles and lockfile, and the package's dependencies.
///
/// It tells files by their contents and not their paths, so moving or
/// renaming one changes nothing, and it leaves out everything
/// [`NOT_MEASURED`], so a change to a table or to the runner changes
/// nothing either. A result stands while this is what it was when the result
/// was taken.
///
/// # Errors
///
/// If a file can't be read.
pub fn sources_of(name: &str, kind: Kind) -> Result<String, String> {
    let root = paths::workspace();
    let read =
        |path: &Path| fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()));
    let mut contents = Vec::new();
    for path in measured(name, kind) {
        let files =
            files_under(&root.join(&path)).map_err(|err| format!("reading {path}: {err}"))?;
        for file in files {
            contents.push(record::hash_bytes(&read(&file)?));
        }
    }
    contents.sort_unstable();
    let text = |path: &Path| Ok::<_, String>(String::from_utf8_lossy(&read(path)?).into_owned());
    let profiles = tables(&text(&root.join("Cargo.toml"))?, |header| {
        header.starts_with("[profile")
    });
    let dependencies = tables(
        &text(&root.join(PACKAGE_DIR).join("Cargo.toml"))?,
        |header| header == "[dependencies]" || header == "[dev-dependencies]",
    );
    let built_with = [
        record::output("rustc", &["-V"]).unwrap_or_else(|| "unknown".to_owned()),
        record::hash_bytes(profiles.as_bytes()),
        record::hash_bytes(&read(&root.join("Cargo.lock"))?),
        record::hash_bytes(dependencies.as_bytes()),
    ];
    let all: Vec<String> = built_with.into_iter().chain(contents).collect();
    Ok(record::hash_bytes(all.join("\n").as_bytes()))
}

/// The package every bench target is in.
const PACKAGE: &str = "bench";

/// The directory a target runs in, as `cargo bench` would run it: its
/// package's.
pub const PACKAGE_DIR: &str = "crates/bench";

/// What the bench target called `name` measures, if it's one the plan or the
/// cache measures name.
fn kind_named(name: &str) -> Option<Kind> {
    if name == "memory" {
        Some(Kind::Memory)
    } else {
        kind_of(name)
    }
}

/// Every bench target as it builds now, in the order the runner runs them:
/// the plan's groups, the memory profiles, then the cache measures. Cargo
/// builds any that's out of date, without running it, and says where each
/// executable is.
///
/// # Errors
///
/// If cargo can't build them, or the targets it built aren't the plan's
/// groups, `memory` and the cache measures.
pub fn built() -> Result<Vec<Target>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut found: Vec<Target> = Vec::new();
    let out = Command::new(&cargo)
        .args(["bench", "-p", PACKAGE, "--no-run", "--message-format=json"])
        .current_dir(paths::workspace())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|err| format!("running cargo to build the benches: {err}"))?;
    if !out.status.success() {
        return Err("cargo couldn't build the benches".to_owned());
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let is_bench = message["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bench"));
        let (Some(name), Some(executable)) = (
            message["target"]["name"].as_str(),
            message["executable"].as_str(),
        ) else {
            continue;
        };
        if message["reason"] == "compiler-artifact" && is_bench {
            let kind = kind_named(name).ok_or_else(|| {
                format!("bench target {name} is in neither the plan nor the cache measures")
            })?;
            let executable = PathBuf::from(executable);
            let binary = record::hash_file(&executable)
                .map_err(|err| format!("hashing {}: {err}", executable.display()))?;
            found.push(Target {
                name: name.to_owned(),
                kind,
                executable,
                binary,
                sources: sources_of(name, kind)?,
            });
        }
    }
    in_run_order(found)
}

/// `found` in the order the runner runs them, if they're exactly the targets
/// the plan and the cache measures name.
fn in_run_order(mut found: Vec<Target>) -> Result<Vec<Target>, String> {
    let wanted = cases::GROUPS
        .iter()
        .map(|group| (group.name, Kind::Timing))
        .chain([("memory", Kind::Memory)])
        .chain(crate::joins::cache::PARTS.map(|part| (part, Kind::Cache)));
    let mut ordered = Vec::new();
    for (name, kind) in wanted {
        let at = found
            .iter()
            .position(|target| target.name == name && target.kind == kind)
            .ok_or_else(|| format!("cargo built no bench target for {name}"))?;
        ordered.push(found.swap_remove(at));
    }
    match found.first() {
        Some(extra) => Err(format!("cargo built bench target {} twice", extra.name)),
        None => Ok(ordered),
    }
}

/// A result file's contents, and when it was written.
#[derive(Clone, Debug)]
pub struct Written<T> {
    pub value: T,
    pub written_unix_ms: u64,
}

/// Everything on disk the report reads.
#[derive(Default)]
pub struct Evidence {
    /// The run log, oldest first.
    pub runs: Vec<Run>,
    /// The results kept across builds: see [`Kept`].
    pub kept: Vec<Kept>,
    /// The commits in the log that aren't in the history of what's checked
    /// out: amended or rebased away, or on another branch. A table can't cite
    /// one: nobody could find it.
    pub unreachable: HashSet<String>,
    /// The sentinel log, oldest first.
    pub checked: Vec<Checked>,
    /// This machine's CPU and macOS version: a sentinel another machine
    /// read, or this one under another system, says nothing about this one.
    pub cpu: String,
    pub os: String,
    pub timings: HashMap<Key, Timing>,
    pub memory: Option<Written<Memory>>,
    pub phases: Option<Written<PhasesPart>>,
    pub reads: Option<Written<ReadsPart>>,
    pub hashers: Option<Written<HashersPart>>,
    pub absent: Option<Written<AbsentPart>>,
    pub inlining: Option<Written<InliningPart>>,
}

/// Reads the run log and every result.
///
/// # Errors
///
/// If one can't be read.
pub fn gather() -> Result<Evidence, String> {
    let runs = record::read_runs()?;
    let commits: HashSet<&String> = runs.iter().map(|run| &run.commit).collect();
    let unreachable = commits
        .into_iter()
        .filter(|commit| !in_history(commit))
        .cloned()
        .collect();
    Ok(Evidence {
        runs,
        kept: record::read_kept()?,
        unreachable,
        checked: record::read_checked()?,
        cpu: record::cpu(),
        os: record::os(),
        timings: read_timings(&paths::criterion_dir())?,
        memory: read_written(&paths::memory_path())?,
        phases: read_written(&crate::joins::cache::part_path("phases"))?,
        reads: read_written(&crate::joins::cache::part_path("reads"))?,
        hashers: read_written(&crate::joins::cache::part_path("hashers"))?,
        absent: read_written(&crate::joins::cache::part_path("absent"))?,
        inlining: read_written(&crate::joins::cache::part_path("inlining"))?,
    })
}

/// Whether `commit` is the commit checked out, or one of its ancestors.
fn in_history(commit: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", commit, "HEAD"])
        .current_dir(paths::workspace())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The JSON file at `path` and when it was written, if it exists and is in
/// the shape this build writes.
fn read_written<T: DeserializeOwned>(path: &Path) -> Result<Option<Written<T>>, String> {
    let json = match fs::read(path) {
        Ok(json) => json,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("reading {}: {err}", path.display())),
    };
    let written = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_err(|err| format!("reading {}: {err}", path.display()))?;
    // A file an earlier build's target wrote, in another shape, is no result
    // of this one's: its target is out of date, and will write it again.
    let Ok(value) = serde_json::from_slice(&json) else {
        eprintln!(
            "{} isn't in the shape this build writes: taking it as not run",
            path.display()
        );
        return Ok(None);
    };
    Ok(Some(Written {
        value,
        written_unix_ms: record::unix_ms(written),
    }))
}

/// The criterion ids of `sentinel` as a group's binary times it, before its
/// first case and after its last.
pub fn sentinel_keys(group: &str, sentinel: Sentinel) -> [Key; 2] {
    ["start", "end"].map(|edge| {
        (
            format!("{group}_{}", sentinel.suffix()),
            edge.to_owned(),
            sentinel.words().to_string(),
        )
    })
}

/// What kind of target `name` is, if it's one with sentinels.
fn kind_of(name: &str) -> Option<Kind> {
    if cases::GROUPS.iter().any(|group| group.name == name) {
        Some(Kind::Timing)
    } else if crate::joins::cache::PARTS.contains(&name) {
        Some(Kind::Cache)
    } else {
        None
    }
}

/// Every criterion id `group`'s binary writes: its cases', by name, and its
/// sentinels'.
pub fn timing_keys(group: &'static Group) -> Vec<(String, Key)> {
    let cases = group.cases().map(|case| (case.to_string(), key(&case)));
    let sentinels = Sentinel::ALL
        .into_iter()
        .flat_map(|sentinel| sentinel_keys(group.name, sentinel))
        .map(|key| (format!("{}/{}", key.0, key.1), key));
    cases.chain(sentinels).collect()
}

impl Evidence {
    /// The run that wrote something of `target`'s at `written_unix_ms`: the
    /// last run of it that started before then.
    pub fn writer(&self, target: &str, written_unix_ms: u64) -> Option<&Run> {
        self.runs
            .iter()
            .rev()
            .find(|run| run.target == target && run.started_unix_ms <= written_unix_ms)
    }

    /// Whether what `run` wrote still stands for `target`: the target is
    /// built from the same source as it was then, or to the same binary, or
    /// the results were [kept](Kept) from that source for this one, in one
    /// step or several.
    pub fn stands(&self, run: &Run, target: &Target) -> bool {
        if run.binary == target.binary {
            return true;
        }
        let wrote = run.measured();
        let mut reached = vec![target.sources.as_str()];
        let mut next = 0;
        while let Some(&at) = reached.get(next) {
            if at == wrote {
                return true;
            }
            for kept in &self.kept {
                if kept.target == target.name
                    && kept.to == at
                    && !reached.contains(&kept.from.as_str())
                {
                    reached.push(&kept.from);
                }
            }
            next += 1;
        }
        false
    }

    /// What wrote the results on disk for `target` that don't
    /// [stand](Self::stands) for it now, as [`Run::measured`] writes it:
    /// nothing if it's up to date or has no results, and more than one only
    /// if its timings came from several builds.
    pub fn wrote_otherwise(&self, target: &Target) -> Vec<String> {
        let mut writers: Vec<&Run> = match target.kind {
            Kind::Timing => timing_keys(cases::group(&target.name))
                .iter()
                .filter_map(|(_, key)| self.timings.get(key))
                .filter_map(|timing| self.writer(&target.name, timing.written_unix_ms))
                .collect(),
            Kind::Memory | Kind::Cache => self
                .file_written(target)
                .and_then(|written| self.writer(&target.name, written))
                .into_iter()
                .collect(),
        };
        writers.retain(|run| !self.stands(run, target));
        let mut wrote: Vec<String> = writers.iter().map(|run| run.measured()).collect();
        wrote.sort_unstable();
        wrote.dedup();
        wrote
    }

    /// When the file a memory or cache target writes was written, if it has
    /// been.
    fn file_written(&self, target: &Target) -> Option<u64> {
        match (target.kind, target.name.as_str()) {
            (Kind::Memory, _) => self.memory.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "phases") => self.phases.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "reads") => self.reads.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "hashers") => self.hashers.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "absent") => self.absent.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "inlining") => self.inlining.as_ref().map(|w| w.written_unix_ms),
            _ => None,
        }
    }

    /// The names of `group`'s timings that are missing, or that don't
    /// [stand](Self::stands) for `target`, its bench target as built now.
    pub fn stale_timings(&self, group: &'static Group, target: &Target) -> Vec<String> {
        timing_keys(group)
            .into_iter()
            .filter(|(_, key)| {
                self.timings
                    .get(key)
                    .and_then(|timing| self.writer(group.name, timing.written_unix_ms))
                    .is_none_or(|run| !self.stands(run, target))
            })
            .map(|(name, _)| name)
            .collect()
    }
}

/// How far a target's narrow sentinel may move between its start and its
/// end before the machine's speed is taken to have changed while it ran.
pub const DRIFT_LIMIT: f64 = 0.03;

/// How much slower than the fastest narrow sentinel of its kind a target's
/// may be. The timing binaries' sentinels are criterion's medians and the
/// cache measures' are timed by hand, so each kind is set beside its own.
pub const SPREAD_LIMIT: f64 = 0.10;

/// How much slower than the fastest narrow sentinel of any kind a target's
/// may be. The two kinds read within about a tenth of each other, and a core
/// pushed to a slower mode takes two to six times as long.
pub const SLOW_LIMIT: f64 = 0.50;

/// How far a target's wide sentinel may move between its start and its end,
/// and how much slower than the fastest of its kind it may be, before the
/// machine is taken to have been busy with something else. The limit is
/// loose because the reading is: on a machine left alone, 49 targets read
/// 1.27 to 1.57 ns a read, and one moved 10% between its start and its end,
/// while their own results repeated to within 0.3%. A compile on the other
/// cores takes it to 2.6 times. So this catches a machine doing heavy work,
/// not one with an editor open, which nothing here can tell from a quiet one.
pub const BUSY_LIMIT: f64 = 0.50;

impl Evidence {
    /// `sentinel` as the target of `kind` called `name` last timed it:
    /// before it measured anything and after it finished, in ns. `None` if
    /// it has none, as the memory profiles don't, or one of the two hasn't
    /// been timed.
    pub fn sentinel(&self, kind: Kind, name: &str, sentinel: Sentinel) -> Option<(f64, f64)> {
        let part = |sentinels: crate::joins::cache::Sentinels| {
            let timed = sentinels.of(sentinel);
            (timed.start_ns, timed.end_ns)
        };
        match (kind, name) {
            (Kind::Timing, group) => {
                let [start, end] = sentinel_keys(group, sentinel);
                let median = |key: &Key| Some(self.timings.get(key)?.median_ns);
                Some((median(&start)?, median(&end)?))
            }
            (Kind::Cache, "phases") => self.phases.as_ref().map(|w| part(w.value.sentinels)),
            (Kind::Cache, "reads") => self.reads.as_ref().map(|w| part(w.value.sentinels)),
            (Kind::Cache, "hashers") => self.hashers.as_ref().map(|w| part(w.value.sentinels)),
            (Kind::Cache, "absent") => self.absent.as_ref().map(|w| part(w.value.sentinels)),
            (Kind::Cache, "inlining") => self.inlining.as_ref().map(|w| part(w.value.sentinels)),
            _ => None,
        }
    }

    /// Both sentinels of the target called `name`, as a line of the sentinel
    /// log for the run that started at `started_unix_ms`; `None` if it lacks
    /// one.
    pub fn checked(&self, kind: Kind, name: &str, started_unix_ms: u64) -> Option<Checked> {
        Some(Checked {
            target: name.to_owned(),
            started_unix_ms,
            cpu: self.cpu.clone(),
            os: self.os.clone(),
            narrow: self.sentinel(kind, name, Sentinel::Narrow)?,
            wide: self.sentinel(kind, name, Sentinel::Wide)?,
        })
    }

    /// The fastest `sentinel` any target of `kind` has read on this machine:
    /// in the results on disk, or in the sentinel log. One fast reading is
    /// enough: nothing makes this work faster than a quiet machine does.
    pub fn fastest(&self, kind: Kind, sentinel: Sentinel) -> Option<f64> {
        let names: Vec<&str> = match kind {
            Kind::Timing => cases::GROUPS.iter().map(|group| group.name).collect(),
            Kind::Cache => crate::joins::cache::PARTS.to_vec(),
            Kind::Memory => Vec::new(),
        };
        let on_disk = names
            .into_iter()
            .filter_map(|name| self.sentinel(kind, name, sentinel));
        let logged = self
            .checked
            .iter()
            .filter(|checked| checked.cpu == self.cpu && checked.os == self.os)
            .filter(|checked| kind_of(&checked.target) == Some(kind))
            .map(|checked| match sentinel {
                Sentinel::Narrow => checked.narrow,
                Sentinel::Wide => checked.wide,
            });
        on_disk
            .chain(logged)
            .map(|(start, end)| start.min(end))
            .min_by(f64::total_cmp)
    }

    /// Why the machine wasn't steady and quiet while the target of `kind`
    /// called `name` ran, if it wasn't: a sentinel moved between its start
    /// and its end, or was slow beside the fastest on record. A target that
    /// ran on a slow or a busy machine has timings of some other machine.
    pub fn unsteady(&self, kind: Kind, name: &str) -> Option<String> {
        let (start, end) = self.sentinel(kind, name, Sentinel::Narrow)?;
        let drift = (end / start - 1.0).abs();
        if drift > DRIFT_LIMIT {
            return Some(format!(
                "the machine's speed moved {:.0}% while it ran",
                drift * 100.0
            ));
        }
        let slowest = start.max(end);
        let of_kind = self.fastest(kind, Sentinel::Narrow)?;
        let of_any = [Kind::Timing, Kind::Cache]
            .into_iter()
            .filter_map(|kind| self.fastest(kind, Sentinel::Narrow))
            .min_by(f64::total_cmp)?;
        let slow = |fastest: f64, limit: f64| slowest > fastest * (1.0 + limit);
        if slow(of_kind, SPREAD_LIMIT) || slow(of_any, SLOW_LIMIT) {
            let beside = if slow(of_kind, SPREAD_LIMIT) {
                of_kind
            } else {
                of_any
            };
            return Some(format!(
                "the machine was slow while it ran: its sentinel took {:.2} times the fastest \
                 on record",
                slowest / beside
            ));
        }

        let (start, end) = self.sentinel(kind, name, Sentinel::Wide)?;
        let drift = (end / start - 1.0).abs();
        if drift > BUSY_LIMIT {
            return Some(format!(
                "the machine got busier or quieter while it ran: its reads past L2 moved {:.0}%",
                drift * 100.0
            ));
        }
        let slowest = start.max(end);
        let fastest = self.fastest(kind, Sentinel::Wide)?;
        if slowest > fastest * (1.0 + BUSY_LIMIT) {
            return Some(format!(
                "the machine was busy with something else while it ran: its reads past L2 took \
                 {:.2} times the fastest on record",
                slowest / fastest
            ));
        }
        None
    }
}

/// Up to five of `ids`, and how many more there are.
pub fn some(ids: &[String]) -> String {
    let shown = ids.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    match ids.len().saturating_sub(5) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// Why `target` has to run before the report can use its results; nothing if
/// what's on disk [stands](Evidence::stands) for it as it's built now.
pub fn out_of_date(target: &Target, evidence: &Evidence) -> Vec<String> {
    let mut why = Vec::new();
    match target.kind {
        Kind::Timing => {
            let group = cases::group(&target.name);
            let stale = evidence.stale_timings(group, target);
            if !stale.is_empty() {
                why.push(format!(
                    "{} of its timings are missing, or what it measures has changed since they \
                     were taken: {}",
                    stale.len(),
                    some(&stale)
                ));
            }
        }
        Kind::Memory | Kind::Cache => match evidence.file_written(target) {
            None => why.push("it has no results".to_owned()),
            Some(written) => {
                let writer = evidence.writer(&target.name, written);
                if writer.is_none_or(|run| !evidence.stands(run, target)) {
                    why.push(
                        "what it measures has changed since its results were taken".to_owned(),
                    );
                }
            }
        },
    }
    // Only of results that stand: any others are being replaced anyway.
    if why.is_empty() {
        why.extend(evidence.unsteady(target.kind, &target.name));
    }
    if let (Kind::Memory, Some(memory)) = (target.kind, &evidence.memory) {
        if let Some(filter) = &memory.value.filter {
            why.push(format!("its profiles ran filtered to {filter:?}"));
        }
        let profiled: std::collections::HashSet<Key> = memory
            .value
            .cases
            .iter()
            .map(|case| {
                (
                    case.group.clone(),
                    case.algorithm.clone(),
                    case.param.to_string(),
                )
            })
            .collect();
        let missing: Vec<String> = cases::cases()
            .filter(|case| !profiled.contains(&key(case)))
            .map(|case| case.to_string())
            .collect();
        if !missing.is_empty() {
            why.push(format!(
                "{} cases have no memory profile: {}",
                missing.len(),
                some(&missing)
            ));
        }
    }
    why
}
