//! Times the `both_small` group: see `groups/both_small.rs` and `support/timing.rs`.

#[macro_use]
#[path = "support/timing.rs"]
mod timing;

include!("../groups/both_small.rs");
