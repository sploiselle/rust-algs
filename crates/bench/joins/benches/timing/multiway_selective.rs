//! Times the `multiway_selective` group: see `groups/multiway_selective.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/multiway_selective.rs");
