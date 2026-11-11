//! Times the `multiway_selective` group: see `groups/multiway_selective.rs` and `support/timing.rs`.

#[macro_use]
#[path = "support/timing.rs"]
mod timing;

include!("groups/multiway_selective.rs");
