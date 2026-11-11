//! Times the `multiway_sparse` group: see `groups/multiway_sparse.rs` and `support/timing.rs`.

#[macro_use]
#[path = "support/timing.rs"]
mod timing;

include!("groups/multiway_sparse.rs");
