//! Delivering broadcasts in an order that respects causality.
//!
//! - [`cbcast`]: causal broadcast, which delivers a message only after
//!   everything that happened before it.

pub mod cbcast;

pub use cbcast::{Cbcast, Message};
