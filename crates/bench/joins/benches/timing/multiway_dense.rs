//! Times the `multiway_dense` group: see `groups/multiway_dense.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/multiway_dense.rs");
