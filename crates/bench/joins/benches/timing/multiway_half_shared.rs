//! Times the `multiway_half_shared` group: see `groups/multiway_half_shared.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/multiway_half_shared.rs");
