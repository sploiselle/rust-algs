//! Times the `hash_variants` group: see `groups/hash_variants.rs` and `support/timing.rs`.

#[macro_use]
#[path = "support/timing.rs"]
mod timing;

include!("groups/hash_variants.rs");
