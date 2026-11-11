//! Times the `one_to_one` group: see `groups/one_to_one.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/one_to_one.rs");
