//! Tracking causality: which events could have influenced which.
//!
//! - [`vector`]: vector clocks, which decide happened-before exactly.
//! - [`dot`]: names for individual events, and sets of them that a plain
//!   vector can't represent.

pub mod dot;
pub mod vector;

pub use dot::{CausalContext, Dot};
pub use vector::{Causality, VectorClock};
