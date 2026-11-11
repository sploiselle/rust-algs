//! A subject: one thing that's benchmarked, such as the joins.
//!
//! The runner knows nothing of what a subject measures. It asks each for its
//! bench targets, whether what's on disk for one can be used, and its tables.
//! To benchmark something else, give it a package of bench targets beside
//! `crates/bench/joins`, a module here that implements [`Subject`], and a
//! line in [`crate::subjects`].

use std::path::PathBuf;

use crate::record::Run;
use crate::targets::Target;

/// What writing the tables may let through.
#[derive(Clone, Debug)]
pub struct Options {
    /// Where the tables are written.
    pub out: PathBuf,
    /// Write them though some results are missing, or don't stand for what's
    /// built now.
    pub allow_stale: bool,
    /// Write them though no commit holds some of the code that was measured.
    pub allow_dirty: bool,
}

pub trait Subject {
    /// What it's called.
    fn name(&self) -> &'static str;

    /// The packages its bench targets are in.
    fn packages(&self) -> &'static [&'static str];

    /// Its bench targets, in the order they run.
    fn targets(&self) -> Vec<&'static str>;

    /// Reads its results, and the logs, from disk.
    ///
    /// # Errors
    ///
    /// If one can't be read.
    fn read(&self) -> Result<Box<dyn Results>, String>;

    /// Writes its tables from the results that stand for `targets`, and says
    /// what a reader of them should know.
    ///
    /// # Errors
    ///
    /// Every problem `options` doesn't allow.
    fn write_tables(&self, targets: &[Target], options: &Options) -> Result<(), String>;
}

/// A subject's results on disk.
pub trait Results {
    /// Why `target` has to run before its results can be used; nothing if
    /// what's on disk stands for it as it's built now.
    fn out_of_date(&self, target: &Target) -> Vec<String>;

    /// The runs that wrote what's on disk for `target` and whose results
    /// don't stand for it as it's built now: what keeping them would vouch
    /// for.
    fn stale_writers(&self, target: &Target) -> Vec<Run>;

    /// Called on what a run of `target` that started at `started_unix_ms`
    /// left on disk. Notes what the subject keeps of every run, and says why
    /// the results can't be used if the machine wasn't steady while it ran.
    ///
    /// # Errors
    ///
    /// If a note can't be written.
    fn ran(&self, target: &Target, started_unix_ms: u64) -> Result<Option<String>, String>;
}
