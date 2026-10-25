//! Tracking causality: which events could have influenced which.
//!
//! - [`lamport`]: Lamport clocks, one counter per process, which never
//!   contradict happened-before but can't detect concurrency.
//! - [`vector`]: vector clocks, which decide happened-before exactly.
//! - [`dot`]: names for individual events, and sets of them that a plain
//!   vector can't represent.
//! - [`hybrid`]: hybrid logical clocks, which give up the exact answer for a
//!   fixed-size timestamp that stays close to wall-clock time.

pub mod dot;
pub mod hybrid;
pub mod lamport;
pub mod vector;

pub use dot::{CausalContext, Dot};
pub use hybrid::{Hlc, Timestamp};
pub use lamport::{LamportClock, LamportStamp};
pub use vector::{Causality, VectorClock};
