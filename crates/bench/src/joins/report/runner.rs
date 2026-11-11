//! Runs the bench targets that are out of date, and logs each run.
//!
//! The runner builds every target, hashes the source it measures with, and
//! compares that with the source that wrote the target's results: see
//! [`evidence::sources_of`]. A target built from the source its results came
//! from is left alone. So what reruns is what a change touched: a group whose
//! definition, workload or joins changed; the memory profiles when the plan
//! did; a cache measure when its code did. Moving files, or changing the
//! tables or this runner, reruns nothing.
//!
//! A change to a measured file that leaves what it measures alone needn't
//! rerun anything either: [`keep`] records that the results still stand.
//!
//! The bench targets hold no bookkeeping, so the runner logs each run itself,
//! before it starts: the commit, whether the tree was dirty, and the hashes
//! of the source and the binary.
//!
//! It also keeps the machine awake while a target runs, and reruns a target
//! whose sentinels say the machine changed speed under it or was busy with
//! something else: an unattended Mac can push a long job to a slower mode for
//! minutes at a time, and a Mac in use reads past L2 a tenth slower, or
//! several times slower while something compiles. Timings taken then are no
//! use. It logs what the sentinels read on every run, so a later run is set
//! beside the fastest this machine has read, not only beside what's on disk.

use std::process::Command;

use crate::paths;

use super::evidence::{self, Target};
use crate::record::{self, Kept, Run};

/// What the runner was asked to do.
#[derive(Default)]
pub struct Request {
    /// Run every target, not only those out of date.
    pub all: bool,
    /// Run these targets, whether or not they're out of date.
    pub only: Vec<String>,
    /// Say what would run, and run nothing.
    pub list: bool,
    /// Arguments passed on to every target that runs, such as a criterion
    /// filter.
    pub args: Vec<String>,
}

/// Builds every target, says which are out of date, and runs them in order.
///
/// # Errors
///
/// If the targets can't be built, a named target doesn't exist, or a target
/// fails.
pub fn run(request: &Request) -> Result<(), String> {
    let targets = evidence::built()?;
    for name in &request.only {
        if !targets.iter().any(|target| &target.name == name) {
            let known: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
            return Err(format!(
                "no bench target {name}: there's {}",
                known.join(", ")
            ));
        }
    }
    let evidence = evidence::gather()?;
    let mut queue: Vec<&Target> = Vec::new();
    for target in &targets {
        let why = evidence::out_of_date(target, &evidence);
        let asked = request.only.contains(&target.name);
        let chosen = if request.only.is_empty() {
            request.all || !why.is_empty()
        } else {
            asked
        };
        let state = match (why.is_empty(), chosen) {
            (true, false) => "up to date".to_owned(),
            (true, true) => "up to date, running anyway".to_owned(),
            (false, true) => why.join("; "),
            (false, false) => format!("not asked for, though {}", why.join("; ")),
        };
        println!("{:<20} {state}", target.name);
        if chosen {
            queue.push(target);
        }
    }
    if queue.is_empty() {
        println!("nothing to run");
        return Ok(());
    }
    if request.list {
        println!("{} of {} targets would run", queue.len(), targets.len());
        return Ok(());
    }
    for target in queue {
        steadily(target, &request.args)?;
    }
    Ok(())
}

/// Which results to keep, and why.
pub struct Keep {
    /// Keep these targets' results; if empty, every target's that were taken
    /// from other source than it's built from now.
    pub only: Vec<String>,
    /// Why they still stand.
    pub because: String,
    /// Say what would be kept, and keep nothing.
    pub list: bool,
}

/// Builds every target and, for each whose results were taken from other
/// source than it's built from now, records that they still stand: see
/// [`Kept`]. This is for an edit to a measured file that doesn't change what
/// it measures: an import, a name, a comment. Nothing is run. A table goes on
/// citing the commit its results were measured at.
///
/// # Errors
///
/// If the targets can't be built, a named target doesn't exist, the tree is
/// dirty, so that no commit holds the code being vouched for, or the log
/// can't be written.
pub fn keep(request: &Keep) -> Result<(), String> {
    let targets = evidence::built()?;
    for name in &request.only {
        if !targets.iter().any(|target| &target.name == name) {
            return Err(format!("no bench target {name}"));
        }
    }
    let now = Run::now("", "", "", &[]);
    if now.dirty && !request.list {
        return Err(
            "the tree is dirty: commit the change first, so the kept results name the commit \
             that holds this source"
                .to_owned(),
        );
    }
    let evidence = evidence::gather()?;
    let mut kept = 0;
    for target in &targets {
        if !request.only.is_empty() && !request.only.contains(&target.name) {
            continue;
        }
        let wrote = evidence.wrote_otherwise(target);
        if wrote.is_empty() {
            let why = evidence::out_of_date(target, &evidence);
            let state = if why.is_empty() {
                "up to date".to_owned()
            } else {
                format!("nothing to keep: {}", why.join("; "))
            };
            println!("{:<20} {state}", target.name);
            continue;
        }
        for from in wrote {
            println!(
                "{:<20} {} its results, taken from {}",
                target.name,
                if request.list { "would keep" } else { "keeps" },
                &from[..from.len().min(19)],
            );
            if !request.list {
                record::append_kept(&Kept {
                    target: target.name.clone(),
                    from,
                    to: target.sources.clone(),
                    kept_unix_ms: now.started_unix_ms,
                    commit: now.commit.clone(),
                    because: request.because.clone(),
                })?;
            }
            kept += 1;
        }
    }
    if kept == 0 {
        println!("nothing to keep");
    }
    Ok(())
}

/// How many times a target is run before the runner gives up on a steady
/// machine.
const ATTEMPTS: usize = 3;

/// Runs `target` until the machine was steady and quiet while it ran, at
/// most [`ATTEMPTS`] times: see [`evidence::Evidence::unsteady`].
fn steadily(target: &Target, args: &[String]) -> Result<(), String> {
    for attempt in 1..=ATTEMPTS {
        let started_unix_ms = start(target, args)?;
        let evidence = evidence::gather()?;
        if let Some(checked) = evidence.checked(target.kind, &target.name, started_unix_ms) {
            record::append_checked(&checked)?;
        }
        let Some(why) = evidence.unsteady(target.kind, &target.name) else {
            return Ok(());
        };
        let next = if attempt < ATTEMPTS {
            "running it again"
        } else {
            "giving up on it: the tables will refuse its results until it reruns"
        };
        println!("{}: {why}; {next}", target.name);
    }
    Ok(())
}

/// Logs a run of `target` and runs it with `args`, as `cargo bench` would:
/// in its package's directory, with `--bench`. Returns when the run started.
fn start(target: &Target, args: &[String]) -> Result<u64, String> {
    let run = Run::now(&target.name, &target.binary, &target.sources, args);
    if run.dirty {
        println!(
            "the tree is dirty: {}'s results will cite no commit until a clean tree runs this \
             binary",
            target.name
        );
    }
    record::append(&run)?;
    println!("running {}", target.name);
    // caffeinate holds the display and the system awake while the target
    // runs: an idle Mac dims, sleeps and slows what's still running.
    let status = Command::new("caffeinate")
        .arg("-dims")
        .arg(&target.executable)
        .arg("--bench")
        .args(args)
        .current_dir(paths::workspace().join(evidence::PACKAGE_DIR))
        // Where criterion writes, without its asking cargo.
        .env("CARGO_TARGET_DIR", paths::target_dir())
        .status()
        .map_err(|err| format!("running {}: {err}", target.executable.display()))?;
    if status.success() {
        Ok(run.started_unix_ms)
    } else {
        Err(format!("{} failed: {status}", target.name))
    }
}
