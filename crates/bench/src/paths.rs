//! Where the benchmarks write: everything under `target/`, so nothing
//! generated gets committed.

use std::path::{Path, PathBuf};

/// Cargo's target directory: `CARGO_TARGET_DIR`, else the workspace's
/// `target/`, where criterion writes too.
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace().join("target"),
    }
}

/// The workspace's root.
pub fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/bench sits two levels below the workspace")
}

/// Where criterion writes its results.
pub fn criterion_dir() -> PathBuf {
    target_dir().join("criterion")
}

/// Where everything else lands: `target/bench/`.
pub fn bench_dir() -> PathBuf {
    target_dir().join("bench")
}

/// The memory harness's output.
pub fn memory_path() -> PathBuf {
    bench_dir().join("memory.json")
}
