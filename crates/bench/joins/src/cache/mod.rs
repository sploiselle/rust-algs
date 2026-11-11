//! Why the hash join's cost per row climbs as its inputs grow.
//!
//! A hash join's random reads go to three structures: its table's control
//! bytes, its table's buckets, and, through a bucket's reference, the build
//! rows. The rows being scanned are read in order, and a prefetcher keeps up.
//! Once those structures outgrow L2, some of the reads reach DRAM, and more of
//! them the larger the structures are:
//!
//! - [`time_phases`] times the join's build and probe loops apart, and the
//!   probe loop again with its matching rows first, and counts the
//!   instructions each loop retires;
//! - [`time_reads`] measures what a read costs, one at a time and overlapped;
//! - [`time_chains`] measures it with a set number of reads able to overlap;
//! - the report models how many reads reach DRAM at each size (its
//!   `joins::model`).
//!
//! The report multiplies the last two, and sets that beside the first.
//!
//! That product prices every read alike, at what it costs in a loop that
//! does nothing but read. Two more measures say where that price is wrong:
//!
//! - [`time_worked_reads`] gives each read unrelated work, as a join's row
//!   has, and measures what the read then costs: the more instructions a read
//!   comes with, the fewer reads fit in flight;
//! - [`time_absent_probes`] probes for keys the table doesn't hold, which
//!   reads control bytes and nothing else, with hashers that add
//!   instructions: the same control-byte misses in a longer row.
//!
//! [`time_hashers`] asks why a slow hasher costs more past L2 rather than
//! less. While a row waits on DRAM, the core works ahead on the rows behind
//! it, but only as many as fit in its window of instructions, so a row with
//! more instructions leaves fewer reads in flight. It times the join with
//! hashers that run more instructions, and counts them, and times it again
//! hashing a batch of keys before their lookups, which keeps the hash's
//! instructions out of the window those lookups wait in, once a batch's
//! lookups are enough to fill it.
//!
//! ## One binary a measure
//!
//! Each of [`PARTS`] is a bench target, under `benches/cache/`, with a module
//! here named for it and a file of its own under `target/bench/cache/`. The
//! functions one target alone runs are `#[inline]`, so their code is in that
//! target's binary and no other. What they share is in this file: the
//! sentinels, the instruction counter, and where the files go.

use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hasher};

use std::hint::black_box;

use std::path::PathBuf;

use std::process::Command;

use std::time::{Duration, Instant};

use algs::join::hash_join;

use foldhash::fast::{FixedState, FoldHasher};

use hashbrown::hash_table::{Entry, HashTable};

use serde::{Deserialize, Serialize};

use crate::harness;

use crate::workload::{self, Rng};

use crate::{Row, Sink, Tally};

/// The cache measures, each a bench target and a file of its own.
pub const PARTS: [&str; 7] = [
    "phases", "reads", "hashers", "absent", "inlining", "order", "chains",
];

/// One sentinel, timed before a target measured anything and after it
/// finished, each the median in ns a call.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Sentinel {
    pub start_ns: f64,
    pub end_ns: f64,
}

/// Both [sentinels](harness::Sentinel) of a target: fixed work that is no
/// measure's, the same as the timing binaries time
/// (`benches/timing/mod.rs`), so the report can set every
/// target's beside the others'. If a target's two timings of one differ, or
/// both are slow beside other targets', the machine wasn't steady or wasn't
/// quiet while it ran, and the report refuses its measures.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Sentinels {
    pub narrow: Sentinel,
    pub wide: Sentinel,
}

impl Sentinels {
    pub fn of(self, sentinel: harness::Sentinel) -> Sentinel {
        match sentinel {
            harness::Sentinel::Narrow => self.narrow,
            harness::Sentinel::Wide => self.wide,
        }
    }
}

/// The sentinels as timed before a target's first measure.
pub struct Started {
    narrow_ns: f64,
    wide_ns: f64,
}

impl Sentinels {
    /// Times both, before a target's first measure.
    #[inline]
    pub fn start() -> Started {
        Started {
            narrow_ns: sentinel_ns(harness::Sentinel::Narrow),
            wide_ns: sentinel_ns(harness::Sentinel::Wide),
        }
    }
}

impl Started {
    /// Times both again, after a target's last measure, and prints the four.
    #[inline]
    pub fn finish(self) -> Sentinels {
        let sentinels = Sentinels {
            narrow: Sentinel {
                start_ns: self.narrow_ns,
                end_ns: sentinel_ns(harness::Sentinel::Narrow),
            },
            wide: Sentinel {
                start_ns: self.wide_ns,
                end_ns: sentinel_ns(harness::Sentinel::Wide),
            },
        };
        for (name, s) in [("narrow", sentinels.narrow), ("wide", sentinels.wide)] {
            println!(
                "{name} sentinel: {:.1} µs before, {:.1} µs after",
                s.start_ns / 1e3,
                s.end_ns / 1e3
            );
        }
        sentinels
    }
}

/// The median time of a call of `sentinel`, in ns, after as long a warm-up
/// as criterion gives a case and about as long a measurement.
#[inline]
fn sentinel_ns(sentinel: harness::Sentinel) -> f64 {
    let mut work = sentinel.work();
    let warm = Instant::now();
    while warm.elapsed() < harness::WARM_UP {
        black_box(work.call());
    }
    let start = Instant::now();
    let mut samples = Vec::new();
    while start.elapsed() < harness::MEASUREMENT {
        let call_start = Instant::now();
        black_box(work.call());
        samples.push(call_start.elapsed().as_nanos() as f64);
    }
    median(&mut samples)
}

/// Where the part called `name` is written: `target/bench/cache/<name>.json`.
pub fn part_path(name: &str) -> PathBuf {
    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("crates/bench/joins sits three levels below the workspace")
            .join("target"),
    };
    target.join("bench/cache").join(format!("{name}.json"))
}

/// Writes `part` to [`part_path`]`(name)`.
///
/// # Panics
///
/// If it can't be written.
pub fn write_part(name: &str, part: &impl Serialize) {
    let path = part_path(name);
    let json = serde_json::to_string_pretty(part).expect("measures serialize");
    let dir = path.parent().expect("a part has a directory");
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&path, json))
        .unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
    println!("wrote {}", path.display());
}

/// Whether cargo is benching: it passes `--bench`. Run without it, as
/// `cargo test --benches` does, a target measures its smallest sizes and
/// writes nothing.
pub fn benching() -> bool {
    std::env::args().any(|arg| arg == "--bench")
}

/// Instructions this process has retired, as the kernel counts them for
/// `proc_pid_rusage`, which needs no privileges.
#[cfg(target_os = "macos")]
fn instructions() -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: the buffer is a zeroed `rusage_info_v4`, the flavor asked for.
    let status = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    assert_eq!(status, 0, "proc_pid_rusage failed");
    // SAFETY: the call succeeded, so it filled the buffer, and zeroes are a
    // valid `rusage_info_v4` regardless.
    unsafe { info.assume_init() }.ri_instructions
}

#[cfg(not(target_os = "macos"))]
fn instructions() -> u64 {
    panic!("counting instructions needs macOS's proc_pid_rusage")
}

/// The middle of `samples`.
fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// A join's two sides and how many pairs it emits.
struct Sides {
    left: Vec<Row>,
    right: Vec<Row>,
    expected: u64,
}

/// `name`, from sysctl.
///
/// # Panics
///
/// If the machine doesn't report it, since a cache this harness can't size
/// can't be modelled.
pub fn sysctl(name: &str) -> usize {
    Command::new("sysctl")
        .args(["-n", name])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .unwrap_or_else(|| panic!("sysctl {name} gave no number"))
}

/// A cache line, in bytes.
pub fn line_bytes() -> usize {
    sysctl("hw.cachelinesize")
}

/// The performance cores' L2, in bytes.
pub fn l2_bytes() -> usize {
    sysctl("hw.perflevel0.l2cachesize")
}

mod absent;
mod hashers;
mod phases;
mod reads;

pub use absent::*;
pub use hashers::*;
pub use phases::*;
pub use reads::*;

#[cfg(test)]
mod tests;

mod inlining;
pub use inlining::{
    Apart, CALL_INSTRUCTIONS, INLINING_SIZES, Inlining, InliningPart, hash_join_inserting,
    time_inlining,
};

mod order;
pub use order::{ORDER_SIZES, Order, OrderPart, in_table_order, time_order};

mod chains;
pub use chains::{CHAINS_BYTES, CHASES, Chained, ChainsPart, time_chains};
