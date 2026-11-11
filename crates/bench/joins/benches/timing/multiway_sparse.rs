//! Times the `multiway_sparse` group: see `groups/multiway_sparse.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/multiway_sparse.rs");
