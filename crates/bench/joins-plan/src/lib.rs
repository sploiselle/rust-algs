//! The joins' plan: every case that's measured, and the memory profile of
//! each.
//!
//! [`cases`] is the plan: each group in
//! `crates/bench/joins/benches/timing/groups/`, as a row. The report reads
//! it to know which results to expect, and the `memory` target here profiles
//! every case of it with a counting allocator ([`alloc`]) and asserts every
//! [claim](cases::memory_claim) a post quotes.
//!
//! It's a package of its own so that it sits between what's timed and what
//! reports it. No timing binary links it, so the plan can grow without
//! moving a join's machine code. And its one target doesn't link the report,
//! so a change to a table leaves the memory profiles as they were.

pub mod alloc;
pub mod cases;
pub mod fmt;
pub mod paths;
pub mod profile;
