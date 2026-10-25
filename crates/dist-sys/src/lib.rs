//! Reference implementations of distributed-systems building blocks, written
//! to be read. Each module explains the idea it implements and carries tests
//! that check the claims it makes.
//!
//! - [`clock`]: tracking causality. [`VectorClock`](clock::VectorClock), plus
//!   the [`Dot`](clock::Dot)s and [`CausalContext`](clock::CausalContext)s that
//!   name and collect individual events, the
//!   [`LamportClock`](clock::LamportClock), and the hybrid logical clock
//!   [`Hlc`](clock::Hlc).
//! - [`counter`]: the simplest source of order, a
//!   [`MonotonicCounter`](counter::MonotonicCounter).
//! - [`crdt`]: replicated data types that converge without coordination.
//!   [`MvRegister`](crdt::MvRegister).
//! - [`coordination`]: getting processes to agree on an order.
//!   [`LamportMutex`](coordination::LamportMutex).

pub mod clock;
pub mod coordination;
pub mod counter;
pub mod crdt;

#[cfg(test)]
mod test_util;
