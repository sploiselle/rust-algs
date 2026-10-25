//! Tracking causality: which events could have influenced which.
//!
//! - [`vector`]: vector clocks, which decide happened-before exactly.
//! - [`dot`]: names for individual events, and sets of them that a plain
//!   vector can't represent.
//! - [`hybrid`]: hybrid logical clocks, which give up the exact answer for a
//!   fixed-size timestamp that stays close to wall-clock time.

pub mod dot;
pub mod hybrid;
pub mod vector;

pub use dot::{CausalContext, Dot};
pub use hybrid::{Hlc, Timestamp};
pub use vector::{Causality, VectorClock};
