//! What the joins' results on disk were measured from, and whether the
//! machine was steady while they were: what the report sets beside what each
//! bench target is built from now.
//!
//! [`gather`] reads the results and the logs, and [`out_of_date`] says what
//! a target lacks: results, results a run the runner logged wrote, or
//! results that [stand](crate::vouch::Log::stands) for the source it's built
//! from now. The runner runs what's out of date, or keeps it, and the tables
//! refuse it.
//!
//! A result also has to come from a steady, quiet machine. Every timing
//! binary and every cache measure times two [sentinels](Sentinel), work that
//! is none of theirs, before it starts and after it ends. If a sentinel's two
//! timings differ, or both are slow beside the fastest this machine has read,
//! the machine changed speed or was busy with something else, and what the
//! target measured is some other machine's: [`Evidence::unsteady`]. Such a
//! target counts as out of date.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bench_joins::cache::{self, AbsentPart, HashersPart, InliningPart, PhasesPart, ReadsPart};
use bench_joins::harness::Sentinel;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::cases::{self, Group};
use super::profile::{Memory, memory_path};
use super::report::{Key, Timing, key, read_timings};
use crate::paths;
use crate::record::{self, Run};
use crate::subject::Results;
use crate::targets::Target;
use crate::vouch::Log;

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

/// A result file's contents, and when it was written.
#[derive(Clone, Debug)]
pub struct Written<T> {
    pub value: T,
    pub written_unix_ms: u64,
}

/// Everything on disk the report reads.
#[derive(Default)]
pub struct Evidence {
    /// The run log and the results kept: which of what's below can be vouched
    /// for.
    pub log: Log,
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
    Ok(Evidence {
        log: Log::read()?,
        checked: read_checked()?,
        cpu: record::cpu(),
        os: record::os(),
        timings: read_timings(&paths::criterion_dir())?,
        memory: read_written(&memory_path())?,
        phases: read_written(&cache::part_path("phases"))?,
        reads: read_written(&cache::part_path("reads"))?,
        hashers: read_written(&cache::part_path("hashers"))?,
        absent: read_written(&cache::part_path("absent"))?,
        inlining: read_written(&cache::part_path("inlining"))?,
    })
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

/// What the bench target called `name` measures, if it's one of the joins'.
pub fn kind_of(name: &str) -> Option<Kind> {
    if cases::GROUPS.iter().any(|group| group.name == name) {
        Some(Kind::Timing)
    } else if name == "memory" {
        Some(Kind::Memory)
    } else if cache::PARTS.contains(&name) {
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

/// What a target's sentinels read on one run of it: see
/// [`Sentinel`]. The results on disk hold only each target's
/// last run, so the sentinel log keeps every run's, and with them how fast
/// this machine reads when it's quiet.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Checked {
    pub target: String,
    /// When the run that timed them started.
    pub started_unix_ms: u64,
    pub cpu: String,
    /// The macOS version.
    pub os: String,
    /// The narrow sentinel before the target's first measure and after its
    /// last, in ns.
    pub narrow: (f64, f64),
    /// The wide sentinel, the same.
    pub wide: (f64, f64),
}

/// The sentinel log, one JSON [`Checked`] per line.
pub fn sentinels_path() -> PathBuf {
    paths::bench_dir().join("sentinels.jsonl")
}

/// Appends `checked` to the sentinel log.
///
/// # Errors
///
/// If the log can't be written.
pub fn append_checked(checked: &Checked) -> Result<(), String> {
    record::append_line(&sentinels_path(), checked)
}

/// Every line of the sentinel log, oldest first; none if there's no log yet.
///
/// # Errors
///
/// If the log can't be read, or a line isn't a [`Checked`].
pub fn read_checked() -> Result<Vec<Checked>, String> {
    record::read_lines(&sentinels_path())
}

impl Evidence {
    /// When the file a memory or cache target writes was written, if it has
    /// been.
    fn file_written(&self, target: &Target) -> Option<u64> {
        match (kind_of(&target.name)?, target.name.as_str()) {
            (Kind::Memory, _) => self.memory.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "phases") => self.phases.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "reads") => self.reads.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "hashers") => self.hashers.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "absent") => self.absent.as_ref().map(|w| w.written_unix_ms),
            (Kind::Cache, "inlining") => self.inlining.as_ref().map(|w| w.written_unix_ms),
            _ => None,
        }
    }

    /// When each of `target`'s results on disk was written: one time for a
    /// file, and one for each timing of a group.
    pub fn written(&self, target: &Target) -> Vec<u64> {
        match kind_of(&target.name) {
            Some(Kind::Timing) => timing_keys(cases::group(&target.name))
                .iter()
                .filter_map(|(_, key)| self.timings.get(key))
                .map(|timing| timing.written_unix_ms)
                .collect(),
            _ => self.file_written(target).into_iter().collect(),
        }
    }

    /// The names of `group`'s timings that are missing, that no run the
    /// runner logged wrote, or that don't [stand](Log::stands) for `target`,
    /// its bench target as built now.
    pub fn stale_timings(&self, group: &'static Group, target: &Target) -> Vec<String> {
        timing_keys(group)
            .into_iter()
            .filter(|(_, key)| {
                self.timings
                    .get(key)
                    .and_then(|timing| self.log.writer(group.name, timing.written_unix_ms))
                    .is_none_or(|run| !self.log.stands(run, target))
            })
            .map(|(name, _)| name)
            .collect()
    }
}

impl Results for Evidence {
    fn out_of_date(&self, target: &Target) -> Vec<String> {
        out_of_date(target, self)
    }

    fn stale_writers(&self, target: &Target) -> Vec<Run> {
        self.log
            .writers(target, &self.written(target))
            .into_iter()
            .filter(|run| !self.log.stands(run, target))
            .cloned()
            .collect()
    }

    fn ran(&self, target: &Target, started_unix_ms: u64) -> Result<Option<String>, String> {
        let Some(kind) = kind_of(&target.name) else {
            return Ok(None);
        };
        if let Some(checked) = self.checked(kind, &target.name, started_unix_ms) {
            append_checked(&checked)?;
        }
        Ok(self.unsteady(kind, &target.name))
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
        let part = |sentinels: cache::Sentinels| {
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
            Kind::Cache => cache::PARTS.to_vec(),
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
/// what's on disk [stands](Log::stands) for it as it's built now.
pub fn out_of_date(target: &Target, evidence: &Evidence) -> Vec<String> {
    let Some(kind) = kind_of(&target.name) else {
        return vec!["it isn't one of the joins' targets".to_owned()];
    };
    let mut why = Vec::new();
    match kind {
        Kind::Timing => {
            let group = cases::group(&target.name);
            let stale = evidence.stale_timings(group, target);
            if !stale.is_empty() {
                why.push(format!(
                    "{} of its timings are missing, weren't written by a run the runner logged, \
                     or were measured from other source than it's built from now: {}",
                    stale.len(),
                    some(&stale)
                ));
            }
        }
        Kind::Memory | Kind::Cache => match evidence.file_written(target) {
            None => why.push("it has no results".to_owned()),
            Some(written) => match evidence.log.writer(&target.name, written) {
                None => why.push("no run the runner logged wrote its results".to_owned()),
                Some(run) if !evidence.log.stands(run, target) => why.push(
                    "its results were measured from other source than it's built from now"
                        .to_owned(),
                ),
                Some(_) => {}
            },
        },
    }
    // Only of results that stand: any others are being replaced anyway.
    if why.is_empty() {
        why.extend(evidence.unsteady(kind, &target.name));
    }
    if let (Kind::Memory, Some(memory)) = (kind, &evidence.memory) {
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
