//! Which results the report can vouch for: the logs, and what they say of a
//! result on disk.
//!
//! A result is some run's. It stands for a target as built now if that run
//! measured what the target is built from now: the same
//! [source](crate::targets), or the same binary, or source its results were
//! [kept](Kept) for. And a table cites the commit that measured its numbers,
//! not the one checked out.

use std::collections::HashSet;
use std::process::{Command, Stdio};

use crate::paths;
use crate::record::{self, Kept, Run};
use crate::targets::Target;

/// The run log and the log of kept results.
#[derive(Default)]
pub struct Log {
    /// Every run, oldest first.
    pub runs: Vec<Run>,
    /// The results kept across edits: see [`Kept`].
    pub kept: Vec<Kept>,
    /// The commits in the run log that aren't in the history of what's
    /// checked out: amended or rebased away, or on another branch. A table
    /// can't cite one: nobody could find it.
    pub unreachable: HashSet<String>,
}

/// Why a table can't cite a commit for a target's results.
#[derive(Debug, PartialEq, Eq)]
pub struct NoCommit;

impl Log {
    /// Reads both logs.
    ///
    /// # Errors
    ///
    /// If one can't be read.
    pub fn read() -> Result<Log, String> {
        let runs = record::read_runs()?;
        let commits: HashSet<&String> = runs.iter().map(|run| &run.commit).collect();
        let unreachable = commits
            .into_iter()
            .filter(|commit| !in_history(commit))
            .cloned()
            .collect();
        Ok(Log {
            runs,
            kept: record::read_kept()?,
            unreachable,
        })
    }

    /// The run that wrote something of `target`'s at `written_unix_ms`: the
    /// last run of it that started before then, if it hadn't ended by then.
    /// A result written after that run ended came from something the runner
    /// didn't start, such as a bare `cargo bench`, and is nobody's.
    pub fn writer(&self, target: &str, written_unix_ms: u64) -> Option<&Run> {
        self.runs
            .iter()
            .rev()
            .find(|run| run.target == target && run.started_unix_ms <= written_unix_ms)
            .filter(|run| {
                run.ended_unix_ms
                    .is_none_or(|ended| written_unix_ms <= ended)
            })
    }

    /// Whether what `run` wrote still stands for `target`: the target is
    /// built from the same source as it was then, or to the same binary, or
    /// the results were [kept](Kept) from that source for this one, in one
    /// step or several.
    pub fn stands(&self, run: &Run, target: &Target) -> bool {
        let of_binary = |binary: &str| format!("binary:{binary}");
        let wrote = [run.measured(), of_binary(&run.binary)];
        let mut reached = vec![target.sources.clone(), of_binary(&target.binary)];
        let mut next = 0;
        while let Some(at) = reached.get(next).cloned() {
            if wrote.contains(&at) {
                return true;
            }
            for kept in &self.kept {
                let was_kept_for = kept.to == at || of_binary(&kept.binary) == at;
                if kept.target == target.name && was_kept_for && !reached.contains(&kept.from) {
                    reached.push(kept.from.clone());
                }
            }
            next += 1;
        }
        false
    }

    /// The runs that wrote what's on disk for `target`, given when each of
    /// its results was written: one, unless its results came from several.
    pub fn writers(&self, target: &Target, written_unix_ms: &[u64]) -> Vec<&Run> {
        let mut writers: Vec<&Run> = written_unix_ms
            .iter()
            .filter_map(|&written| self.writer(&target.name, written))
            .collect();
        writers.sort_by_key(|run| run.started_unix_ms);
        writers.dedup_by_key(|run| run.started_unix_ms);
        writers
    }

    /// The run a table of `target`'s results cites, given when each was
    /// written: the oldest clean run, at a commit in this checkout's history,
    /// of the same code as the run that wrote them. That is the first commit
    /// here known to hold the code that was measured. Results kept across an
    /// edit cite what measured them, not what they were kept for.
    ///
    /// `Ok(None)` if nothing on disk stands for the target.
    ///
    /// # Errors
    ///
    /// [`NoCommit`] if only dirty trees have run that code, or only commits
    /// since amended away.
    pub fn cited(
        &self,
        target: &Target,
        written_unix_ms: &[u64],
    ) -> Result<Option<&Run>, NoCommit> {
        let writers = self.writers(target, written_unix_ms);
        let Some(wrote) = writers.iter().rev().find(|run| self.stands(run, target)) else {
            return Ok(None);
        };
        let same_code = |run: &&Run| {
            run.target == target.name
                && (run.binary == wrote.binary
                    || (!run.sources.is_empty() && run.sources == wrote.sources))
        };
        self.runs
            .iter()
            .filter(same_code)
            .find(|run| !run.dirty && !self.unreachable.contains(&run.commit))
            .map(Some)
            .ok_or(NoCommit)
    }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A clean run of `t` that started at `started_unix_ms`.
    fn run(started_unix_ms: u64, commit: &str, binary: &str, sources: &str) -> Run {
        Run {
            target: "t".to_owned(),
            started_unix_ms,
            commit: commit.to_owned(),
            dirty: false,
            rustc: String::new(),
            cpu: String::new(),
            os: String::new(),
            args: Vec::new(),
            binary: binary.to_owned(),
            sources: sources.to_owned(),
            ended_unix_ms: None,
        }
    }

    /// `t`, as built now.
    fn target(binary: &str, sources: &str) -> Target {
        Target {
            name: "t".to_owned(),
            directory: PathBuf::new(),
            executable: PathBuf::new(),
            binary: binary.to_owned(),
            sources: sources.to_owned(),
            files: Vec::new(),
        }
    }

    fn kept(from: &str, to: &str, binary: &str) -> Kept {
        Kept {
            target: "t".to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            binary: binary.to_owned(),
            kept_unix_ms: 0,
            commit: String::new(),
            because: String::new(),
        }
    }

    fn log(runs: Vec<Run>, kept: Vec<Kept>) -> Log {
        Log {
            runs,
            kept,
            ..Log::default()
        }
    }

    #[test]
    fn a_result_written_after_its_run_ended_is_nobodys() {
        let ended = Run {
            ended_unix_ms: Some(2000),
            ..run(1000, "a", "bin", "src")
        };
        let log = log(vec![ended], Vec::new());
        assert!(log.writer("t", 1500).is_some());
        assert!(log.writer("t", 2000).is_some());
        // A bare `cargo bench`, after the run the runner logged.
        assert!(log.writer("t", 2500).is_none());
        assert!(log.writer("t", 500).is_none());
        assert!(log.writer("another", 1500).is_none());
        // A run logged before ends were has none to be held to.
        let before = self::log(vec![run(1000, "a", "bin", "")], Vec::new());
        assert!(before.writer("t", 2500).is_some());
    }

    #[test]
    fn results_stand_for_the_same_source_or_the_same_binary() {
        let log = log(Vec::new(), Vec::new());
        let wrote = run(1000, "a", "bin", "src");
        assert!(log.stands(&wrote, &target("bin", "src")));
        // Moved, or built with something no target links changed.
        assert!(log.stands(&wrote, &target("another-bin", "src")));
        // A file it's built from changed, and the code it compiles to didn't.
        assert!(log.stands(&wrote, &target("bin", "another-src")));
        assert!(!log.stands(&wrote, &target("another-bin", "another-src")));
    }

    #[test]
    fn kept_results_stand_for_the_source_and_the_binary_they_were_kept_for() {
        let wrote = run(1000, "a", "bin-1", "src-1");
        let log = log(Vec::new(), vec![kept("src-1", "src-2", "bin-2")]);
        assert!(log.stands(&wrote, &target("bin-2", "src-2")));
        // The source changed again, and the binary is still the one kept for.
        assert!(log.stands(&wrote, &target("bin-2", "src-3")));
        assert!(log.stands(&wrote, &target("bin-3", "src-2")));
        assert!(!log.stands(&wrote, &target("bin-3", "src-3")));
        // Kept on from there.
        let twice = self::log(
            Vec::new(),
            vec![
                kept("src-1", "src-2", "bin-2"),
                kept("src-2", "src-3", "bin-3"),
            ],
        );
        assert!(twice.stands(&wrote, &target("bin-3", "src-3")));
        // A run logged before sources were is known by its binary.
        let old = run(1000, "a", "bin-0", "");
        let from_binary = self::log(Vec::new(), vec![kept("binary:bin-0", "src-2", "bin-2")]);
        assert!(from_binary.stands(&old, &target("bin-2", "src-2")));
        assert!(!from_binary.stands(&old, &target("bin-3", "src-3")));
    }

    #[test]
    fn kept_results_cite_what_measured_them_until_they_are_measured_again() {
        let mut log = log(
            vec![run(1000, "aaa", "bin-1", "src-1")],
            vec![kept("src-1", "src-2", "bin-2")],
        );
        let now = target("bin-2", "src-2");
        let cited = log.cited(&now, &[1500]).expect("a clean tree ran it");
        assert_eq!(cited.map(|run| run.commit.as_str()), Some("aaa"));
        // Rerun at the commit they were kept at: the numbers on disk are that
        // run's, and so is the commit.
        log.runs.push(run(3000, "bbb", "bin-2", "src-2"));
        let cited = log.cited(&now, &[3500]).expect("a clean tree ran it");
        assert_eq!(cited.map(|run| run.commit.as_str()), Some("bbb"));
        // Nothing on disk stands for other source.
        assert_eq!(log.cited(&target("bin-9", "src-9"), &[3500]), Ok(None));
    }

    #[test]
    fn code_only_a_dirty_tree_ran_has_no_commit_to_cite() {
        let dirty = Run {
            dirty: true,
            ..run(1000, "aaa", "bin", "src")
        };
        let log = log(vec![dirty], Vec::new());
        assert_eq!(log.cited(&target("bin", "src"), &[1500]), Err(NoCommit));
    }
}
