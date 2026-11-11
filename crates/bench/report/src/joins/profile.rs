//! What the memory harness writes: a profile of every case it ran. It says
//! nothing of the run that wrote it: the runner logs that.

use serde::{Deserialize, Serialize};

use crate::alloc::Usage;

/// What the memory harness wrote: the filter it ran with, and a profile of
/// every case it ran.
#[derive(Serialize, Deserialize, Debug)]
pub struct Memory {
    pub filter: Option<String>,
    pub cases: Vec<MemoryCase>,
}

/// One case's memory profile, and its input's size.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemoryCase {
    pub group: String,
    pub algorithm: String,
    pub param: usize,
    pub input_rows: u64,
    pub input_bytes: u64,
    /// The [`Checksum`](bench_joins::Checksum)'s sum, which every algorithm in the
    /// group agreed on.
    pub checksum: u64,
    #[serde(flatten)]
    pub usage: Usage,
}

/// Where the `memory` target writes the profiles.
pub fn memory_path() -> std::path::PathBuf {
    crate::paths::bench_dir().join("memory.json")
}
