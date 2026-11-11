//! The run log: a line in `target/bench/runs.jsonl` for every run of a bench
//! target, saying when it started, what code and machine it ran on, and the
//! hashes of the source it measured and the binary that ran. Beside it, the
//! results kept across an edit that didn't change what they measure, in
//! `target/bench/kept.jsonl`.
//!
//! The runner writes both, not the bench targets: they hold no bookkeeping,
//! so the logs' format can change without touching what's measured.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;
use serde::{Deserialize, Serialize};

/// A run of one bench target: when it started, and the code and machine it
/// ran on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The bench target: a group's name, `memory`, or a cache measure's.
    pub target: String,
    pub started_unix_ms: u64,
    /// `git rev-parse HEAD`.
    pub commit: String,
    /// Whether any tracked file differed from `commit`, so the code that ran
    /// may not be the code at `commit`.
    pub dirty: bool,
    /// `rustc -V`.
    pub rustc: String,
    pub cpu: String,
    /// The macOS version.
    pub os: String,
    /// The arguments the target ran with, a filter among them.
    pub args: Vec<String>,
    /// The SHA-256 of the executable that ran, in hex: see [`hash_file`].
    pub binary: String,
    /// What the target measured, as a hash of its source: see
    /// `evidence::sources_of`. The report trusts a result while its target is
    /// still built from this, wherever the files are. Empty for a run logged
    /// before this was recorded.
    #[serde(default)]
    pub sources: String,
}

impl Run {
    /// A record of a run of `target`, whose executable hashes to `binary`
    /// and was built from `sources`, starting now with `args`. A field whose
    /// command fails is `unknown`, and a tree git can't read counts as dirty.
    pub fn now(target: &str, binary: &str, sources: &str, args: &[String]) -> Run {
        let unknown = || "unknown".to_owned();
        Run {
            target: target.to_owned(),
            started_unix_ms: unix_ms(SystemTime::now()),
            commit: output("git", &["rev-parse", "HEAD"]).unwrap_or_else(unknown),
            dirty: output("git", &["status", "--porcelain", "--untracked-files=no"])
                .is_none_or(|changes| !changes.is_empty()),
            rustc: output("rustc", &["-V"]).unwrap_or_else(unknown),
            cpu: cpu(),
            os: os(),
            args: args.to_vec(),
            binary: binary.to_owned(),
            sources: sources.to_owned(),
        }
    }

    /// What this run measured, to set beside what a target is built from
    /// now: its [`sources`](Self::sources), or for a run logged before they
    /// were recorded, the binary that ran.
    pub fn measured(&self) -> String {
        if self.sources.is_empty() {
            format!("binary:{}", self.binary)
        } else {
            self.sources.clone()
        }
    }
}

/// The machine's CPU, as `sysctl` names it, or `unknown`.
pub fn cpu() -> String {
    output("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_else(|| "unknown".to_owned())
}

/// The macOS version, or `unknown`.
pub fn os() -> String {
    output("sw_vers", &["-productVersion"]).unwrap_or_else(|| "unknown".to_owned())
}

/// The SHA-256 of the file at `path`, in hex.
///
/// # Errors
///
/// If the file can't be read.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    Ok(hash_bytes(&fs::read(path)?))
}

/// The SHA-256 of `bytes`, in hex.
pub fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether `args` holds `flag`, alone (`--flag`) or with a value
/// (`--flag=value`).
pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|arg| arg.split_once('=').map_or(arg.as_str(), |(name, _)| name) == flag)
}

/// What `program` printed, trimmed, or `None` if it couldn't run or failed.
/// It runs in the workspace, so git sees this repository whatever the
/// caller's working directory.
pub fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(paths::workspace())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Milliseconds since the Unix epoch.
pub fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// What a target's sentinels read on one run of it: see
/// [`crate::joins::harness::Sentinel`]. The results on disk hold only each target's
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

/// A statement that a bench target's results still stand: that the source
/// it's built from now, `to`, measures the same thing as the source that
/// wrote them, `from`. An edit that touches a measured file without changing
/// what it measures (an import, a name, a comment) needs one; moving a file
/// doesn't, since the source is told by its contents. Nothing checks the
/// statement: it's whoever keeps the results saying so, on the record, with
/// the commit they said it at and why.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub target: String,
    /// What wrote the results: see [`Run::measured`].
    pub from: String,
    /// What the target was built from when they were kept: its sources.
    pub to: String,
    pub kept_unix_ms: u64,
    /// `git rev-parse HEAD`, on a clean tree: the commit that holds `to`.
    pub commit: String,
    /// Why the results still stand, in the keeper's words.
    pub because: String,
}

/// The run log, one JSON [`Run`] per line.
pub fn runs_path() -> PathBuf {
    paths::bench_dir().join("runs.jsonl")
}

/// The sentinel log, one JSON [`Checked`] per line.
pub fn sentinels_path() -> PathBuf {
    paths::bench_dir().join("sentinels.jsonl")
}

/// The log of results kept across builds, one JSON [`Kept`] per line.
pub fn kept_path() -> PathBuf {
    paths::bench_dir().join("kept.jsonl")
}

/// Appends `kept` to the log of kept results.
///
/// # Errors
///
/// If the log can't be written.
pub fn append_kept(kept: &Kept) -> Result<(), String> {
    append_line(&kept_path(), kept)
}

/// Every line of the log of kept results, oldest first; none if there's no
/// log yet.
///
/// # Errors
///
/// If the log can't be read, or a line isn't a [`Kept`].
pub fn read_kept() -> Result<Vec<Kept>, String> {
    read_lines(&kept_path())
}

/// Appends `run` to the run log.
///
/// # Errors
///
/// If the log can't be written: a run the report can't see would leave its
/// results looking stale.
pub fn append(run: &Run) -> Result<(), String> {
    append_line(&runs_path(), run)
}

/// Appends `checked` to the sentinel log.
///
/// # Errors
///
/// If the log can't be written.
pub fn append_checked(checked: &Checked) -> Result<(), String> {
    append_line(&sentinels_path(), checked)
}

fn append_line<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let write = || {
        fs::create_dir_all(paths::bench_dir())?;
        let mut log = OpenOptions::new().create(true).append(true).open(path)?;
        let line = serde_json::to_string(value).map_err(std::io::Error::other)?;
        writeln!(log, "{line}")
    };
    write().map_err(|err| format!("appending to {}: {err}", path.display()))
}

/// Every run in the log, oldest first; none if there's no log yet.
///
/// # Errors
///
/// If the log can't be read, or a line isn't a [`Run`].
pub fn read_runs() -> Result<Vec<Run>, String> {
    read_lines(&runs_path())
}

/// Every line of the sentinel log, oldest first; none if there's no log yet.
///
/// # Errors
///
/// If the log can't be read, or a line isn't a [`Checked`].
pub fn read_checked() -> Result<Vec<Checked>, String> {
    read_lines(&sentinels_path())
}

fn read_lines<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    let log = match fs::read_to_string(path) {
        Ok(log) => log,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("reading {}: {err}", path.display())),
    };
    log.lines()
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_str(line)
                .map_err(|err| format!("{} line {}: {err}", path.display(), i + 1))
        })
        .collect()
}
