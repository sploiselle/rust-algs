//! What each bench target builds to now, and what the results on disk were
//! built from: the two things the report compares.
//!
//! A result is good while the target that wrote it still builds to the same
//! binary. [`built`] asks cargo for every target's executable and hashes it;
//! [`gather`] reads the results and the run log; [`out_of_date`] says what a
//! target lacks. The runner runs what's out of date, and the tables refuse
//! it.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bench_cache::{AbsentPart, HashersPart, PhasesPart, ReadsPart};
use bench_plan::cases::{self, Group};
use bench_plan::paths;
use bench_plan::profile::Memory;
use serde::de::DeserializeOwned;

use crate::record::{self, Run};
use crate::{Key, Timing, key, read_timings};

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
    pub package: &'static str,
    pub kind: Kind,
    pub executable: PathBuf,
    /// The executable's hash: see [`record::hash_file`].
    pub binary: String,
}

/// The bench targets' packages, each built on its own, as the runner runs
/// them: built together, cargo could give a shared dependency other features,
/// and so other binaries.
const PACKAGES: [(&str, Kind); 3] = [
    ("bench", Kind::Timing),
    ("bench-plan", Kind::Memory),
    ("bench-cache", Kind::Cache),
];

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
    for (package, kind) in PACKAGES {
        let out = Command::new(&cargo)
            .args(["bench", "-p", package, "--no-run", "--message-format=json"])
            .current_dir(paths::workspace())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|err| format!("running cargo to build {package}'s benches: {err}"))?;
        if !out.status.success() {
            return Err(format!("cargo couldn't build {package}'s benches"));
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
                let executable = PathBuf::from(executable);
                let binary = record::hash_file(&executable)
                    .map_err(|err| format!("hashing {}: {err}", executable.display()))?;
                found.push(Target {
                    name: name.to_owned(),
                    package,
                    kind,
                    executable,
                    binary,
                });
            }
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
        .chain(bench_cache::PARTS.map(|part| (part, Kind::Cache)));
    let mut ordered = Vec::new();
    for (name, kind) in wanted {
        let at = found
            .iter()
            .position(|target| target.name == name && target.kind == kind)
            .ok_or_else(|| format!("cargo built no bench target for {name}"))?;
        ordered.push(found.swap_remove(at));
    }
    match found.first() {
        Some(extra) => Err(format!(
            "bench target {} of {} is in neither the plan nor the cache measures",
            extra.name, extra.package
        )),
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
    pub timings: HashMap<Key, Timing>,
    pub memory: Option<Written<Memory>>,
    pub phases: Option<Written<PhasesPart>>,
    pub reads: Option<Written<ReadsPart>>,
    pub hashers: Option<Written<HashersPart>>,
    pub absent: Option<Written<AbsentPart>>,
}

/// Reads the run log and every result.
///
/// # Errors
///
/// If one can't be read.
pub fn gather() -> Result<Evidence, String> {
    Ok(Evidence {
        runs: record::read_runs()?,
        timings: read_timings(&paths::criterion_dir())?,
        memory: read_written(&paths::memory_path())?,
        phases: read_written(&bench_cache::part_path("phases"))?,
        reads: read_written(&bench_cache::part_path("reads"))?,
        hashers: read_written(&bench_cache::part_path("hashers"))?,
        absent: read_written(&bench_cache::part_path("absent"))?,
    })
}

/// The JSON file at `path` and when it was written, if it exists.
fn read_written<T: DeserializeOwned>(path: &Path) -> Result<Option<Written<T>>, String> {
    let json = match fs::read(path) {
        Ok(json) => json,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("reading {}: {err}", path.display())),
    };
    let written = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_err(|err| format!("reading {}: {err}", path.display()))?;
    let value = serde_json::from_slice(&json)
        .map_err(|err| format!("parsing {}: {err}", path.display()))?;
    Ok(Some(Written {
        value,
        written_unix_ms: record::unix_ms(written),
    }))
}

/// The criterion ids of the sentinel a group's binary times before its first
/// case and after its last.
pub fn sentinel_keys(group: &str) -> [Key; 2] {
    ["start", "end"].map(|edge| {
        (
            format!("{group}_sentinel"),
            edge.to_owned(),
            (1usize << 20).to_string(),
        )
    })
}

/// Every criterion id `group`'s binary writes: its cases', by name, and its
/// sentinel's.
pub fn timing_keys(group: &'static Group) -> Vec<(String, Key)> {
    let cases = group.cases().map(|case| (case.to_string(), key(&case)));
    let sentinel = sentinel_keys(group.name)
        .into_iter()
        .map(|key| (format!("{}/{}", key.0, key.1), key));
    cases.chain(sentinel).collect()
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

    /// When the file a memory or cache target writes was written, if it has
    /// been.
    fn file_written(&self, target: &Target) -> Option<u64> {
        match (target.kind, target.name.as_str()) {
            (Kind::Memory, _) => self.memory.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "phases") => self.phases.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "reads") => self.reads.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "hashers") => self.hashers.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "absent") => self.absent.as_ref().map(|w| w.written_unix_ms),
            _ => None,
        }
    }

    /// The names of `group`'s timings that are missing, or that another
    /// build of its target than `binary` wrote.
    pub fn stale_timings(&self, group: &'static Group, binary: &str) -> Vec<String> {
        timing_keys(group)
            .into_iter()
            .filter(|(_, key)| {
                self.timings
                    .get(key)
                    .and_then(|timing| self.writer(group.name, timing.written_unix_ms))
                    .is_none_or(|run| run.binary != binary)
            })
            .map(|(name, _)| name)
            .collect()
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
/// what's on disk came from the binary it builds to now.
pub fn out_of_date(target: &Target, evidence: &Evidence) -> Vec<String> {
    let mut why = Vec::new();
    match target.kind {
        Kind::Timing => {
            let group = cases::group(&target.name);
            let stale = evidence.stale_timings(group, &target.binary);
            if !stale.is_empty() {
                why.push(format!(
                    "{} of its timings are missing, or came from another build of it: {}",
                    stale.len(),
                    some(&stale)
                ));
            }
        }
        Kind::Memory | Kind::Cache => match evidence.file_written(target) {
            None => why.push("it has no results".to_owned()),
            Some(written) => {
                let writer = evidence.writer(&target.name, written);
                if writer.is_none_or(|run| run.binary != target.binary) {
                    why.push("its results came from another build of it".to_owned());
                }
            }
        },
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
