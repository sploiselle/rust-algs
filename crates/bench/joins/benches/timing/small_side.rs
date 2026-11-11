//! Times the `small_side` group: see `groups/small_side.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/small_side.rs");
