//! Times the `multiway_dense` group: see `groups/multiway_dense.rs` and `support/timing.rs`.

#[macro_use]
#[path = "support/timing.rs"]
mod timing;

include!("../groups/multiway_dense.rs");
