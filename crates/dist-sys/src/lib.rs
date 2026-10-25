//! Reference implementations of distributed-systems building blocks, written
//! to be read. Each module explains the idea it implements and carries tests
//! that check the claims it makes.
//!
//! - [`clock`]: tracking causality. [`VectorClock`](clock::VectorClock), plus
//!   the [`Dot`](clock::Dot)s and [`CausalContext`](clock::CausalContext)s that
//!   name and collect individual events, and the hybrid logical clock
//!   [`Hlc`](clock::Hlc).
//! - [`crdt`]: replicated data types that converge without coordination.
//!   [`MvRegister`](crdt::MvRegister).

pub mod clock;
pub mod crdt;

#[cfg(test)]
mod test_util;
