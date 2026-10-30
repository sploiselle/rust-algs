//! # User machines
//!
//! A system specified as a sequential machine (Lamport, *The Implementation
//! of Reliable Distributed Multiprocess Systems*, 1978, Section 1.2). It has
//! states, commands and responses, and a mapping `e: C × S → R × S`:
//! executing a command in some state produces a response and a new state. An
//! airline reservation system is one: its state is a set of flights, each
//! with its reservations and waiting list, and a command books or cancels a
//! seat ([`airline`](super::airline)).
//!
//! Nothing in a user machine knows about processes, messages or failures, so
//! writing one is a sequential programming problem. Those belong to whatever
//! runs it, such as the [`Bookkeeper`](super::Bookkeeper)s, which keep a copy
//! at each of several processes and execute the same commands in the same
//! order on every copy. For the copies to stay identical, a user machine must
//! be deterministic, and every copy must start from the same state.

/// A deterministic sequential machine: a system specified apart from the
/// processes that run it.
pub trait UserMachine {
    /// Cloned, so that every process running a copy can be sent it.
    type Command: Clone;
    type Response;

    /// Execute `command`, changing the state and returning the response. The
    /// same command in the same state must always give the same response and
    /// the same new state. An invalid command, such as booking a seat on a
    /// flight that doesn't exist, should produce an error response and leave
    /// the state as it was: it's executed like any other.
    fn execute(&mut self, command: &Self::Command) -> Self::Response;
}
