//! Times the `small_side` group: see `groups/small_side.rs` and `support/timing.rs`.

#[macro_use]
#[path = "support/timing.rs"]
mod timing;

include!("../groups/small_side.rs");
