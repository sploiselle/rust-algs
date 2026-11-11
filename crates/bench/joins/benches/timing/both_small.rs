//! Times the `both_small` group: see `groups/both_small.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/both_small.rs");
