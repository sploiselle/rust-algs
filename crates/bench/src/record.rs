//! What a bench run records, so the report can tell which results belong
//! together: a line in `target/bench/runs.jsonl` for every run, and the memory
//! profiles in `target/bench/memory.json`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::alloc::Usage;

/// A run of one bench target: when it started, and the code and machine it
/// ran on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// `joins` or `memory`.
    pub target: String,
    pub started_unix_ms: u64,
    /// `git rev-parse HEAD`.
    pub commit: String,
    /// Whether any tracked file differed from `commit`, so the code that ran
    /// isn't the code at `commit`.
    pub dirty: bool,
    /// `rustc -V`.
    pub rustc: String,
    pub cpu: String,
    /// The macOS version.
    pub os: String,
    /// The arguments the target ran with, a filter among them.
    pub args: Vec<String>,
}

impl Run {
    /// A record of a run of `target` starting now. A field whose command
    /// fails is `unknown`, and a tree git can't read counts as dirty.
    pub fn now(target: &str) -> Run {
        let unknown = || "unknown".to_owned();
        Run {
            target: target.to_owned(),
            started_unix_ms: unix_ms(SystemTime::now()),
            commit: output("git", &["rev-parse", "HEAD"]).unwrap_or_else(unknown),
            dirty: output("git", &["status", "--porcelain", "--untracked-files=no"])
                .is_none_or(|changes| !changes.is_empty()),
            rustc: output("rustc", &["-V"]).unwrap_or_else(unknown),
            cpu: output("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_else(unknown),
            os: output("sw_vers", &["-productVersion"]).unwrap_or_else(unknown),
            args: std::env::args().skip(1).collect(),
        }
    }
}

/// Whether `args` holds `flag`, alone (`--flag`) or with a value
/// (`--flag=value`).
pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|arg| arg.split_once('=').map_or(arg.as_str(), |(name, _)| name) == flag)
}

/// What `program` printed, trimmed, or `None` if it couldn't run or failed.
/// It runs in this package's directory, so git sees this repository whatever
/// the caller's working directory.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
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

/// Cargo's target directory: `CARGO_TARGET_DIR`, else the workspace's
/// `target/`, where criterion writes too.
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("crates/bench sits two levels below the workspace")
            .join("target"),
    }
}

/// Where this crate writes: `target/bench/`.
pub fn bench_dir() -> PathBuf {
    target_dir().join("bench")
}

/// The run log, one JSON [`Run`] per line.
pub fn runs_path() -> PathBuf {
    bench_dir().join("runs.jsonl")
}

/// The memory harness's output.
pub fn memory_path() -> PathBuf {
    bench_dir().join("memory.json")
}

/// Appends `run` to the run log.
///
/// # Panics
///
/// If the log can't be written: a run the report can't see would leave its
/// results looking stale.
pub fn append(run: &Run) {
    let path = runs_path();
    let write = || {
        fs::create_dir_all(bench_dir())?;
        let mut log = OpenOptions::new().create(true).append(true).open(&path)?;
        let line = serde_json::to_string(run).map_err(std::io::Error::other)?;
        writeln!(log, "{line}")
    };
    write().unwrap_or_else(|err| panic!("appending to {}: {err}", path.display()));
}

/// Every run in the log, oldest first; none if there's no log yet.
///
/// # Errors
///
/// If the log can't be read, or a line isn't a [`Run`].
pub fn read_runs() -> Result<Vec<Run>, String> {
    let path = runs_path();
    let log = match fs::read_to_string(&path) {
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

/// What the memory harness wrote: the run, the filter it ran with, and a
/// profile of every case it ran.
#[derive(Serialize, Deserialize, Debug)]
pub struct Memory {
    pub run: Run,
    pub filter: Option<String>,
    pub cases: Vec<MemoryCase>,
}

/// One case's memory profile, and its input's size.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemoryCase {
    pub group: String,
    pub algorithm: String,
    pub param: usize,
    pub input_rows: u64,
    pub input_bytes: u64,
    /// The [`Checksum`](crate::Checksum)'s sum, which every algorithm in the
    /// group agreed on.
    pub checksum: u64,
    #[serde(flatten)]
    pub usage: Usage,
}
