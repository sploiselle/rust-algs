//! The joins' tables: criterion's timings, the memory profiles and the cache
//! measures as Markdown a post can paste, each citing the commit and machine
//! it came from.
//!
//! - [`results`](load): what's on disk that the report can vouch for;
//! - [`TABLES`] and [`render`]: which cases each table sets beside which, and
//!   how one is written;
//! - [`FACTS`]: what a post says in a sentence, checked against the results.
//!
//! The report uses only results it can vouch for: those a bench target
//! wrote, in a run the runner logged, when that target is still built from
//! the source it was measured from. See [`crate::targets`] for what that
//! source is and [`crate::vouch`] for when a result stands for it.
//!
//! It refuses, naming what's wrong, unless a flag allows it:
//!
//! - a target whose results are missing, weren't written by a run the
//!   runner logged, or were measured from other source than it's built from
//!   now ([`evidence::out_of_date`]; `--allow-stale`);
//! - a timing written by a run with other criterion settings than the
//!   captions describe, such as a smoke run's shorter measurement
//!   ([`shortened`]), or with a confidence level other than 95%
//!   (`--allow-stale`);
//! - results measured from source no clean tree has run, so no commit is
//!   known to hold their code (`--allow-dirty`).
//!
//! Each table cites the commit of the oldest clean run of the source its
//! numbers came from: the first commit known to hold that code. Results kept
//! across an edit go on citing the commit that measured them. A table that
//! draws on several targets cites the latest of theirs. Rerunning a noisy
//! case replaces its timing without invalidating the rest, and doesn't move
//! the commit. Under `--allow-stale`, a case with no result shows as
//! `missing`. Every file written under a flag says so, in its provenance
//! comment and its caption.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bench_joins::Row;
use bench_joins::cache;
use bench_joins::harness;
use bench_joins::harness::Sentinel;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::evidence;
use super::model;
pub use crate::fmt::{bytes, count, sig3};
use crate::joins::cases::{self, Algorithm, Case, Workload};
use crate::joins::profile::MemoryCase;
use crate::record::{self, Run};
pub use crate::subject::Options;
use crate::targets::Target;
use crate::vouch::NoCommit;
use evidence::Evidence;
pub use evidence::{BUSY_LIMIT, DRIFT_LIMIT};

mod cache_tables;
mod facts;
mod format;
mod results;
mod tables;

use cache_tables::*;
pub use facts::*;
pub use format::*;
pub use results::*;
pub use tables::*;

#[cfg(test)]
mod tests;
