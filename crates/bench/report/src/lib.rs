//! Runs the benchmarks that are out of date, and writes the tables a post
//! quotes.
//!
//! ```text
//! cargo run -p bench --release -- run     # run whatever is out of date
//! cargo run -p bench --release -- keep    # keep results an edit didn't touch
//! cargo run -p bench --release            # the tables
//! ```
//!
//! What's benchmarked is a [`Subject`]. Each has a package of bench targets
//! beside this one, and a module here: so far, the [`joins`]. The rest is
//! any subject's:
//!
//! - [`targets`]: the bench targets as cargo builds them, and what each is
//!   built from;
//! - [`record`] and [`vouch`]: the logs of what ran and what was kept, and
//!   which results on disk they vouch for;
//! - [`runner`]: runs what's out of date, and keeps what an edit didn't
//!   touch;
//! - [`paths`] and [`fmt`]: where things are written, and how numbers are.

pub mod joins;
pub mod record;
pub mod runner;
pub mod subject;
pub mod targets;
pub mod vouch;

pub use bench_joins_plan::{fmt, paths};
pub use subject::Subject;

/// Everything that's benchmarked.
pub fn subjects() -> Vec<Box<dyn Subject>> {
    vec![Box::new(joins::Joins)]
}
