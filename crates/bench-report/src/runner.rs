//! Runs the bench targets that are out of date, and logs each run.
//!
//! The runner builds every target, hashes its executable, and compares that
//! with the binary that wrote the target's results. A target whose results
//! came from the binary it still builds to is left alone. So what reruns is
//! what a change touched: a group whose definition, workload or joins
//! changed; the memory profiles when the plan did; a cache measure whose own
//! code did. A change to the tables, or to this runner, reruns nothing.
//!
//! The bench targets hold no bookkeeping, so the runner logs each run itself,
//! before it starts: the commit, whether the tree was dirty, and the binary's
//! hash.
//!
//! It also keeps the machine awake while a target runs, and reruns a target
//! whose sentinel says the machine changed speed under it: an unattended Mac
//! can push a long job to a slower mode for minutes at a time, and timings
//! taken then are no use.

use std::process::Command;

use bench_plan::paths;

use crate::evidence::{self, Kind, Target};
use crate::record::{self, Run};

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

/// How many times a target is run before the runner gives up on a steady
/// machine.
const ATTEMPTS: usize = 3;

/// Runs `target` until the machine was steady while it ran, at most
/// [`ATTEMPTS`] times: see [`evidence::Evidence::unsteady`].
fn steadily(target: &Target, args: &[String]) -> Result<(), String> {
    for attempt in 1..=ATTEMPTS {
        start(target, args)?;
        let evidence = evidence::gather()?;
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
/// in its package's directory, with `--bench`.
fn start(target: &Target, args: &[String]) -> Result<(), String> {
    let run = Run::now(&target.name, &target.binary, args);
    if run.dirty {
        println!(
            "the tree is dirty: {}'s results will cite no commit until a clean tree runs this \
             binary",
            target.name
        );
    }
    record::append(&run)?;
    println!("running {}", target.name);
    let directory = match target.kind {
        Kind::Timing => "crates/bench",
        Kind::Memory => "crates/bench-plan",
        Kind::Cache => "crates/bench-cache",
    };
    // caffeinate holds the display and the system awake while the target
    // runs: an idle Mac dims, sleeps and slows what's still running.
    let status = Command::new("caffeinate")
        .arg("-dims")
        .arg(&target.executable)
        .arg("--bench")
        .args(args)
        .current_dir(paths::workspace().join(directory))
        // Where criterion writes, without its asking cargo.
        .env("CARGO_TARGET_DIR", paths::target_dir())
        .status()
        .map_err(|err| format!("running {}: {err}", target.executable.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} failed: {status}", target.name))
    }
}
