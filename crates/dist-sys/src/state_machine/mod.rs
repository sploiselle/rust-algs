//! Running a system on many processes so it survives losing some of them:
//! specify it as one sequential machine, and run a copy at each process on
//! the same commands in the same order.
//!
//! - [`user_machine`]: the machine, a [`UserMachine`], which knows nothing of
//!   processes or failures.
//! - [`bookkeeper`]: Lamport's reliable state machine, from *The
//!   Implementation of Reliable Distributed Multiprocess Systems* (1978),
//!   whose [`Bookkeeper`]s run copies of any user machine whose commands are
//!   ordered, voting on which commands to execute, and keep going while some
//!   of them have failed.
//! - [`airline`]: the paper's airline reservation system, a user machine to
//!   run on them.

pub mod airline;
pub mod bookkeeper;
pub mod user_machine;

#[cfg(test)]
mod sim;

pub use bookkeeper::{Bookkeeper, Config, Message, Stamped, Votes};
pub use user_machine::UserMachine;
