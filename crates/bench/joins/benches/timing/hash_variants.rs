//! Times the `hash_variants` group: see `groups/hash_variants.rs` and `mod.rs`.

#[macro_use]
#[path = "mod.rs"]
mod timing;

include!("groups/hash_variants.rs");
