//! Getting processes to agree on an order, which takes hearing from them.
//!
//! - [`lamport_mutex`]: Lamport's mutual exclusion, which grants a shared
//!   resource in Lamport-stamp order.

pub mod lamport_mutex;

pub use lamport_mutex::{Kind, LamportMutex, Message};
