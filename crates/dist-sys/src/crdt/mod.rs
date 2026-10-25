//! Replicated data types that converge without coordination.
//!
//! - [`lamport_register`]: a last-writer-wins register on Lamport stamps,
//!   which converges by silently dropping concurrent writes.
//! - [`mv_register`]: a register that keeps concurrent writes side by side.

pub mod lamport_register;
pub mod mv_register;

pub use lamport_register::LamportRegister;
pub use mv_register::{MvRegister, ReadContext};
