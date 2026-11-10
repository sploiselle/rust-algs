//! The run log: a line in `target/bench/runs.jsonl` for every run of a bench
//! target, saying when it started, what code and machine it ran on, and the
//! hash of the binary that ran.
//!
//! The runner writes it, not the bench targets: they hold no bookkeeping, so
//! the log's format, and everything else in this crate, can change without
//! changing a binary whose results the report has to trust.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bench_plan::paths;
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
    /// The SHA-256 of the executable that ran, in hex: see [`hash_file`]. The
    /// report trusts a result while its target still builds to this.
    pub binary: String,
}

impl Run {
    /// A record of a run of `target`, whose executable hashes to `binary`,
    /// starting now with `args`. A field whose command fails is `unknown`,
    /// and a tree git can't read counts as dirty.
    pub fn now(target: &str, binary: &str, args: &[String]) -> Run {
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

/// The SHA-256 of the file at `path`, in hex. Builds are reproducible, so a
/// bench target's executable hashes the same whenever the code it links, its
/// dependencies, its flags and the toolchain are the same, and differently
/// when any of them moves a byte, and so perhaps a loop.
///
/// # Errors
///
/// If the file can't be read.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(fs::read(path)?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
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
/// [`bench::harness::Sentinel`]. The results on disk hold only each target's
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

/// The run log, one JSON [`Run`] per line.
pub fn runs_path() -> PathBuf {
    paths::bench_dir().join("runs.jsonl")
}

/// The sentinel log, one JSON [`Checked`] per line.
pub fn sentinels_path() -> PathBuf {
    paths::bench_dir().join("sentinels.jsonl")
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
