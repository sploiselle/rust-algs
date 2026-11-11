//! Runs the bench targets that are out of date, keeps the results an edit
//! didn't touch, and logs both.
//!
//! [`run`] builds every subject's targets and compares what each is built
//! from with what its results were measured from: see [`crate::targets`] and
//! [`crate::vouch`]. A target whose results still stand is left alone. So
//! what reruns is what a change touched: a group whose definition changed,
//! or everything that links a join that did. Moving a file, or changing the
//! plan, a table or this runner, reruns nothing.
//!
//! An edit to a measured file that leaves what it measures alone needn't
//! rerun anything either: [`keep`] records that the results still stand.
//!
//! The bench targets hold no bookkeeping, so the runner logs each run itself:
//! when it started and ended, the commit, whether the tree was dirty, and the
//! hashes of the source and the binary. A result written outside a run it
//! logged isn't used.
//!
//! It also keeps the machine awake while a target runs, and reruns a target
//! whose subject says the machine wasn't steady while it ran.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

use crate::paths;
use crate::record::{self, Kept, Run};
use crate::subject::Subject;
use crate::targets::{self, Target};

/// What [`run`] was asked to do.
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

/// Each subject's bench targets as built now, in the order they run.
///
/// # Errors
///
/// If cargo can't build them, a subject's target wasn't built, or a target
/// was built that no subject claims: its results would be nobody's to judge.
pub fn built(subjects: &[Box<dyn Subject>]) -> Result<Vec<Vec<Target>>, String> {
    let mut packages: Vec<&str> = Vec::new();
    for subject in subjects {
        for package in subject.packages() {
            if !packages.contains(package) {
                packages.push(package);
            }
        }
    }
    let mut found = targets::built(&packages)?;
    let mut of_subjects = Vec::new();
    for subject in subjects {
        let mut ordered = Vec::new();
        for name in subject.targets() {
            let at = found
                .iter()
                .position(|target| target.name == name)
                .ok_or_else(|| {
                    format!(
                        "cargo built no bench target for {}'s {name}",
                        subject.name()
                    )
                })?;
            ordered.push(found.swap_remove(at));
        }
        of_subjects.push(ordered);
    }
    match found.first() {
        Some(extra) => Err(format!(
            "bench target {} is no subject's: add it to one's targets, in crates/bench/report",
            extra.name
        )),
        None => Ok(of_subjects),
    }
}

/// Checks that every name in `only` is a target.
fn known(only: &[String], built: &[Vec<Target>]) -> Result<(), String> {
    let names: Vec<&str> = built.iter().flatten().map(|t| t.name.as_str()).collect();
    match only.iter().find(|name| !names.contains(&name.as_str())) {
        Some(name) => Err(format!(
            "no bench target {name}: there's {}",
            names.join(", ")
        )),
        None => Ok(()),
    }
}

/// Builds every target, says which are out of date, and runs them in order.
///
/// # Errors
///
/// If the targets can't be built, a named target doesn't exist, or a target
/// fails.
pub fn run(subjects: &[Box<dyn Subject>], request: &Request) -> Result<(), String> {
    let built = built(subjects)?;
    known(&request.only, &built)?;
    let total: usize = built.iter().map(Vec::len).sum();
    let mut queue: Vec<(&dyn Subject, &Target)> = Vec::new();
    for (subject, targets) in subjects.iter().zip(&built) {
        let results = subject.read()?;
        for target in targets {
            let why = results.out_of_date(target);
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
                queue.push((subject.as_ref(), target));
            }
        }
    }
    if queue.is_empty() {
        println!("nothing to run");
        return Ok(());
    }
    if request.list {
        println!("{} of {total} targets would run", queue.len());
        return Ok(());
    }
    for (subject, target) in queue {
        steadily(subject, target, &request.args)?;
    }
    Ok(())
}

/// How many times a target is run before the runner gives up on a steady
/// machine.
const ATTEMPTS: usize = 3;

/// Runs `target` until its subject says the machine was steady while it ran,
/// at most [`ATTEMPTS`] times.
fn steadily(subject: &dyn Subject, target: &Target, args: &[String]) -> Result<(), String> {
    for attempt in 1..=ATTEMPTS {
        let started_unix_ms = start(target, args)?;
        let Some(why) = subject.read()?.ran(target, started_unix_ms)? else {
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
             source",
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
        .current_dir(&target.directory)
        // Where criterion writes, without its asking cargo.
        .env("CARGO_TARGET_DIR", paths::target_dir())
        .status()
        .map_err(|err| format!("running {}: {err}", target.executable.display()))?;
    // Whatever is written from here on isn't this run's.
    record::append(&run.ended())?;
    if status.success() {
        Ok(run.started_unix_ms)
    } else {
        Err(format!("{} failed: {status}", target.name))
    }
}

/// Which results to [`keep`], and why.
pub struct Keep {
    /// Keep these targets' results.
    pub only: Vec<String>,
    /// Keep every target's that doesn't stand for what it's built from now.
    pub all: bool,
    /// Why they still stand.
    pub because: String,
    /// Say what would be kept, and keep nothing.
    pub list: bool,
}

/// Builds every target and, for each asked for whose results were measured
/// from other source than it's built from now, records that they still
/// stand: see [`Kept`]. This is for an edit to a measured file that doesn't
/// change what it measures: an import, a name, a comment. Nothing is run,
/// and a table goes on citing the commit its results were measured at.
///
/// Nothing checks the claim, so it says what's being claimed: for each
/// target, the commit that measured its results and the files it's built
/// from that have changed since.
///
/// # Errors
///
/// If the targets can't be built, a named target doesn't exist, no target
/// was named, the tree is dirty, so that no commit holds the source being
/// vouched for, or the log can't be written.
pub fn keep(subjects: &[Box<dyn Subject>], request: &Keep) -> Result<(), String> {
    if request.only.is_empty() && !request.all && !request.list {
        return Err("say whose results to keep: --only TARGET,... or --all".to_owned());
    }
    let built = built(subjects)?;
    known(&request.only, &built)?;
    let now = Run::now("", "", "", &[]);
    if now.dirty && !request.list {
        return Err(
            "the tree is dirty: commit the change first, so the kept results name the commit \
             that holds this source"
                .to_owned(),
        );
    }
    let mut kept = 0;
    for (subject, targets) in subjects.iter().zip(&built) {
        let results = subject.read()?;
        for target in targets {
            if !request.only.is_empty() && !request.only.contains(&target.name) {
                continue;
            }
            let writers = results.stale_writers(target);
            if writers.is_empty() {
                let why = results.out_of_date(target);
                let state = if why.is_empty() {
                    "up to date".to_owned()
                } else {
                    format!("nothing to keep: {}", why.join("; "))
                };
                println!("{:<20} {state}", target.name);
                continue;
            }
            for run in writers {
                if run.dirty {
                    println!(
                        "{:<20} can't be kept: its results were measured on a dirty tree, so no \
                         commit holds what they measured",
                        target.name
                    );
                    continue;
                }
                println!(
                    "{:<20} {} its results, measured at {}; {}",
                    target.name,
                    if request.list { "would keep" } else { "keeps" },
                    &run.commit[..run.commit.len().min(7)],
                    changed_since(&run.commit, target)
                );
                if !request.list {
                    record::append_kept(&Kept {
                        target: target.name.clone(),
                        from: run.measured(),
                        to: target.sources.clone(),
                        binary: target.binary.clone(),
                        kept_unix_ms: now.started_unix_ms,
                        commit: now.commit.clone(),
                        because: request.because.clone(),
                    })?;
                }
                kept += 1;
            }
        }
    }
    if kept == 0 {
        println!("nothing to keep");
    }
    Ok(())
}

/// How many changed files [`changed_since`] names before it counts the rest.
const NAMED: usize = 6;

/// What has changed in the files `target` is built from since `commit`, in
/// words: the files whose contents that commit doesn't hold anywhere. A file
/// that only moved isn't one.
fn changed_since(commit: &str, target: &Target) -> String {
    let Some(changed) = changed_files(commit, &target.files) else {
        return "git can't say what has changed since".to_owned();
    };
    if changed.is_empty() {
        return format!(
            "none of the {} files it's built from has changed since: the compiler, a dependency \
             or a build setting has",
            target.files.len()
        );
    }
    let mut names: Vec<String> = changed
        .iter()
        .take(NAMED)
        .map(|file| file.display().to_string())
        .collect();
    if changed.len() > NAMED {
        names.push(format!("{} more", changed.len() - NAMED));
    }
    format!(
        "{} of the {} files it's built from changed since: {}",
        changed.len(),
        target.files.len(),
        names.join(", ")
    )
}

/// The `files`, from the workspace's root, whose contents aren't any file's
/// at `commit`; `None` if git can't say.
fn changed_files(commit: &str, files: &[PathBuf]) -> Option<Vec<PathBuf>> {
    let tree = record::output("git", &["ls-tree", "-r", commit])?;
    // Each line is `<mode> blob <hash>\t<path>`.
    let then: HashSet<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().nth(2))
        .collect();
    let mut args = vec!["hash-object", "--"];
    let paths: Vec<String> = files
        .iter()
        .map(|file| file.display().to_string())
        .collect();
    args.extend(paths.iter().map(String::as_str));
    let now = record::output("git", &args)?;
    Some(
        files
            .iter()
            .zip(now.lines())
            .filter(|(_, hash)| !then.contains(hash))
            .map(|(file, _)| file.clone())
            .collect(),
    )
}
