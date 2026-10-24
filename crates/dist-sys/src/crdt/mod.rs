//! Replicated data types that converge without coordination.
//!
//! - [`mv_register`]: a register that keeps concurrent writes side by side.

pub mod mv_register;

pub use mv_register::{MvRegister, ReadContext};
