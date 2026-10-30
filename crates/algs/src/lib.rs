//! Reference implementations of algorithms and distributed-systems building
//! blocks, written to be read. Each module explains the idea it implements and
//! carries tests that check the claims it makes.
//!
//! - [`clock`]: tracking causality. [`VectorClock`](clock::VectorClock), plus
//!   the [`Dot`](clock::Dot)s and [`CausalContext`](clock::CausalContext)s that
//!   name and collect individual events, the
//!   [`LamportClock`](clock::LamportClock), and the hybrid logical clock
//!   [`Hlc`](clock::Hlc).
//! - [`counter`]: the simplest source of order, a
//!   [`MonotonicCounter`](counter::MonotonicCounter).
//! - [`crdt`]: replicated data types that converge without coordination.
//!   [`LamportRegister`](crdt::LamportRegister), which shows what a total
//!   order loses, and [`MvRegister`](crdt::MvRegister), which keeps it.
//! - [`coordination`]: getting processes to agree on an order.
//!   [`LamportMutex`](coordination::LamportMutex).
//! - [`state_machine`]: running one sequential machine on many processes, so
//!   it survives losing some of them. [`Bookkeeper`](state_machine::Bookkeeper)s
//!   run Lamport's reliable state machine on any
//!   [`UserMachine`](state_machine::UserMachine) whose commands are ordered,
//!   such as Lamport's airline reservation example,
//!   [`Airline`](state_machine::airline::Airline).
//! - [`broadcast`]: delivering broadcasts in an order that respects causality.
//!   [`Cbcast`](broadcast::Cbcast).
//! - [`merkle`]: finding which paths under a directory changed between two
//!   snapshots, skipping every subtree whose hash still matches.
//!   [`MerkleTree`](merkle::MerkleTree).

pub mod broadcast;
pub mod clock;
pub mod coordination;
pub mod counter;
pub mod crdt;
pub mod merkle;
pub mod state_machine;

#[cfg(test)]
mod test_util;
