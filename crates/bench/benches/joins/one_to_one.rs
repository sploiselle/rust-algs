//! Times the `one_to_one` group: see `groups/one_to_one.rs` and `support/timing.rs`.

#[macro_use]
#[path = "support/timing.rs"]
mod timing;

include!("groups/one_to_one.rs");
