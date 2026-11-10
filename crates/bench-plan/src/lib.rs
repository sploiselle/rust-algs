//! Every benchmark group, and the memory side of the benchmarks: the joins'
//! memory profiles, and the claims about them a post quotes.
//!
//! [`cases`] is the plan: each group in `crates/bench/groups/`, as a row. The
//! `memory` target profiles every case of it with a counting allocator
//! ([`alloc`]) and asserts every [claim](cases::memory_claim).
//!
//! No timing binary links this crate: see `bench`'s docs for why that
//! matters. The memory target does, so it reruns when the plan changes, which
//! takes a couple of minutes: allocation is deterministic, so it profiles
//! each case twice and has nothing to sample.
//!
//! ## Why memory is a separate binary
//!
//! The memory harness counts every allocation with its own global allocator,
//! [`CountingAlloc`](alloc::CountingAlloc). In a timing binary, that would
//! add bookkeeping to every allocation the timed code makes, and count
//! criterion's own. Nor does a peak fit criterion's model: it divides each
//! sample by its iteration count, which suits a time and understates a peak,
//! and allocation is deterministic, so its statistics have nothing to estimate.

pub mod alloc;
pub mod cases;
pub mod fmt;
pub mod paths;
pub mod profile;
