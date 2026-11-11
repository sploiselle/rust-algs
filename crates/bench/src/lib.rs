//! Every benchmark in the workspace, and what runs and reports them.
//!
//! Each subject that's measured has a module here and a directory of bench
//! targets under `benches/`. So far there is one, [`joins`]: the
//! [`algs::join`] implementations.
//!
//! What any subject shares sits beside them: the run log ([`record`]), where
//! results are written ([`paths`]), a counting allocator ([`alloc`]) and how
//! numbers are written ([`fmt`]).
//!
//! ```text
//! cargo run -p bench --release -- run     # run whatever is out of date
//! cargo run -p bench --release -- keep    # keep results a change didn't touch
//! cargo run -p bench --release            # the tables
//! ```

pub mod alloc;
pub mod fmt;
pub mod joins;
pub mod paths;
pub mod record;
