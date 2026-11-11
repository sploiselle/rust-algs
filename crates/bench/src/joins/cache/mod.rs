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
//! - [`dram_reads`] models how many reads reach DRAM at each size.
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
//! Each of [`PARTS`] is a bench target, under `benches/joins/cache/`, with a
//! file of its own under `target/bench/cache/`. The functions one target
//! alone runs are `#[inline]`, so their code is in that target's binary and
//! no other. Every measure is built from this module, so a change here
//! reruns all five, and no group's timings.

use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hasher};
use std::hint::black_box;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use algs::join::hash_join;
use foldhash::fast::{FixedState, FoldHasher};
use hashbrown::hash_table::{Entry, HashTable};
use serde::{Deserialize, Serialize};

use crate::joins::harness::{self, BUCKET, buckets};
use crate::joins::workload::{self, Rng};
use crate::joins::{Row, Sink, Tally};

/// Rows a side [`time_phases`] runs at: 64K to 64M, by twos, finer than the
/// main case so the knee shows, and past it so the flattening does.
pub const SIZES: [usize; 11] = [
    1 << 16,
    1 << 17,
    1 << 18,
    1 << 19,
    1 << 20,
    1 << 21,
    1 << 22,
    1 << 23,
    1 << 24,
    1 << 25,
    1 << 26,
];

/// Where the report takes the join's cost with nothing reaching DRAM: the
/// largest of [`SIZES`] whose table and build rows fit in L2.
pub const FLAT: usize = 1 << 18;

/// Rows a side [`time_hashers`] runs at: one size whose table fits in L1's
/// neighbourhood, the flat rate's, and three past L2.
pub const HASHER_SIZES: [usize; 5] = [1 << 14, 1 << 18, 1 << 20, 1 << 22, 1 << 24];

/// Bytes [`time_reads`] runs at: 1 MiB to 2 GiB, by twos.
pub const WORKING_SETS: [usize; 12] = [
    1 << 20,
    1 << 21,
    1 << 22,
    1 << 23,
    1 << 24,
    1 << 25,
    1 << 26,
    1 << 27,
    1 << 28,
    1 << 29,
    1 << 30,
    1 << 31,
];

/// The window [`time_reads`] keeps its paged reads in: 1 MiB, 64 of the M6's
/// 16 KiB pages, few enough to stay in the first-level TLB.
const WINDOW: usize = 1 << 20;

/// Samples of each read cost, and the fewest of each join.
const READ_SAMPLES: usize = 5;

/// Rounds of unrelated work [`time_worked_reads`] gives each read. Past 16
/// the compiler stops unrolling the rounds, and the instructions a read jump.
pub const WORK_ROUNDS: [usize; 5] = [0, 4, 8, 12, 16];

/// Bytes [`time_worked_reads`] reads across: 4 MiB, which L2 holds, and
/// 1 GiB, of which it holds a fiftieth.
const WORKED_IN_L2: usize = 1 << 22;
const WORKED_PAST_L2: usize = 1 << 30;

/// Rows a side [`time_absent_probes`] runs at: 1M, where L2 holds the control
/// bytes, to 64M, where it holds a sixth of them.
pub const ABSENT_SIZES: [usize; 6] = [1 << 20, 1 << 22, 1 << 23, 1 << 24, 1 << 25, 1 << 26];

/// What [`time_absent_probes`] adds to every key, so the table holds none of
/// them: the workloads' keys are below 2^28.
const ABSENT: u64 = 1 << 40;

/// One size's times, each the median in ns per input row, both sides counted.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Phases {
    pub rows_a_side: usize,
    pub build_ns: f64,
    pub probe_ns: f64,
    /// The probe loop on the same rows, the ones that match first. It does the
    /// same work, but whether a row matches is no longer a coin toss, so the
    /// branch on it is predicted, where the main order misses about every
    /// other row.
    pub grouped_probe_ns: f64,
    /// The whole call, with allocating and freeing its table.
    pub whole_ns: f64,
    /// Instructions the build loop retires for each row it builds, as the
    /// kernel counts them.
    pub build_instructions: f64,
    /// Instructions the probe loop retires for each row it probes with.
    pub probe_instructions: f64,
}

/// What a read costs when each read comes with unrelated work, each the
/// median per read.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WorkedRead {
    /// Rounds of work a read: one of [`WORK_ROUNDS`].
    pub rounds: usize,
    /// Instructions retired a read, the work's among them.
    pub instructions: f64,
    /// Across [`WORKED_IN_L2`] bytes.
    pub l2_ns: f64,
    /// Across [`WORKED_PAST_L2`] bytes.
    pub dram_ns: f64,
}

/// A probe for keys the table doesn't hold, with one hashing at one size,
/// each the median per probe row.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AbsentProbe {
    pub rows_a_side: usize,
    /// The [`Hashing`]'s label.
    pub hashing: String,
    pub ns: f64,
    pub instructions: f64,
}

/// What a read costs across `bytes`, each the median in ns per read.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReadCost {
    pub bytes: usize,
    /// Each read's address comes from the last read, so none can overlap.
    pub chased_ns: f64,
    /// Addresses come from a list, so the core overlaps as many as it can.
    pub overlapped_ns: f64,
    /// The same lines overlapped, a [`WINDOW`] at a time, so few pages are
    /// live at once and hardly a read misses the TLB.
    pub paged_ns: f64,
}

/// One hashing's cost at one size, each the median per input row.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HasherCost {
    pub rows_a_side: usize,
    /// The [`Hashing`]'s label.
    pub hashing: String,
    pub ns: f64,
    /// Instructions retired, as the kernel counts them.
    pub instructions: f64,
}

/// The cache measures, each a bench target and a file of its own.
pub const PARTS: [&str; 5] = ["phases", "reads", "hashers", "absent", "inlining"];

/// One sentinel, timed before a target measured anything and after it
/// finished, each the median in ns a call.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Sentinel {
    pub start_ns: f64,
    pub end_ns: f64,
}

/// Both [sentinels](harness::Sentinel) of a target: fixed work that is no
/// measure's, the same as the timing binaries time
/// (`benches/joins/support/timing.rs`), so the report can set every
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

/// What the `phases` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PhasesPart {
    pub sentinels: Sentinels,
    pub l2_bytes: usize,
    pub line_bytes: usize,
    pub phases: Vec<Phases>,
}

/// What the `reads` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReadsPart {
    pub sentinels: Sentinels,
    pub line_bytes: usize,
    pub reads: Vec<ReadCost>,
    pub worked_reads: Vec<WorkedRead>,
}

/// What the `hashers` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HashersPart {
    pub sentinels: Sentinels,
    pub hashers: Vec<HasherCost>,
}

/// What the `absent` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AbsentPart {
    pub sentinels: Sentinels,
    pub absent: Vec<AbsentProbe>,
}

/// Where the part called `name` is written: `target/bench/cache/<name>.json`.
pub fn part_path(name: &str) -> PathBuf {
    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("crates/bench sits two levels below the workspace")
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

/// The hashings [`time_absent_probes`] probes with: foldhash, and foldhash
/// with instructions added and no reads.
pub const ABSENT_HASHINGS: [Hashing; 3] = [Hashing::Fold, Hashing::Padded16, Hashing::Padded32];

/// How [`time_hashers`] hashes the join's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hashing {
    /// [`hash_join`] with foldhash, as the main case runs it.
    Fold,
    /// [`hash_join`] with foldhash and 16 more rounds of mixing.
    Padded16,
    /// [`hash_join`] with foldhash and 32 more rounds of mixing.
    Padded32,
    /// [`hash_join`] with std's SipHash-1-3, with fixed keys, as the main
    /// case runs it.
    Sip,
    /// [`batched_hash_join`] with SipHash, 16 keys at a time: too few for
    /// their lookups to fill the window, so the next batch's hashing does.
    SipBatch16,
    /// [`batched_hash_join`] with SipHash, 256 keys at a time.
    SipBatch256,
    /// [`batched_hash_join`] with foldhash, 256 keys at a time: what batching
    /// does to a hash that's already cheap.
    FoldBatch256,
}

/// Every [`Hashing`], in the order the report's columns take.
pub const HASHINGS: [Hashing; 7] = [
    Hashing::Fold,
    Hashing::Padded16,
    Hashing::Padded32,
    Hashing::Sip,
    Hashing::SipBatch16,
    Hashing::SipBatch256,
    Hashing::FoldBatch256,
];

impl Hashing {
    pub fn label(self) -> &'static str {
        match self {
            Hashing::Fold => "foldhash",
            Hashing::Padded16 => "foldhash + 16 rounds",
            Hashing::Padded32 => "foldhash + 32 rounds",
            Hashing::Sip => "SipHash",
            Hashing::SipBatch16 => "SipHash, 16 at a time",
            Hashing::SipBatch256 => "SipHash, 256 at a time",
            Hashing::FoldBatch256 => "foldhash, 256 at a time",
        }
    }

    /// Joins `build` and `probe` this way.
    pub fn join<S: Sink>(self, build: &[Row], probe: &[Row], out: &mut S) {
        let sip = BuildHasherDefault::<DefaultHasher>::default;
        let pair = |l: &Row, r: &Row| out.pair(l, r);
        match self {
            Hashing::Fold => hash_join(build, probe, FixedState::default(), pair),
            Hashing::Padded16 => hash_join(build, probe, Padded::<16>, pair),
            Hashing::Padded32 => hash_join(build, probe, Padded::<32>, pair),
            Hashing::Sip => hash_join(build, probe, sip(), pair),
            Hashing::SipBatch16 => batched_hash_join::<16, _>(build, probe, sip(), out),
            Hashing::SipBatch256 => batched_hash_join::<256, _>(build, probe, sip(), out),
            Hashing::FoldBatch256 => {
                batched_hash_join::<256, _>(build, probe, FixedState::default(), out);
            }
        }
    }
}

/// foldhash, then `ROUNDS` rounds of a multiply and an xorshift: more
/// instructions per key, the same table and the same reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Padded<const ROUNDS: usize>;

/// [`Padded`]'s hasher.
pub struct PaddedHasher<const ROUNDS: usize>(FoldHasher<'static>);

impl<const ROUNDS: usize> BuildHasher for Padded<ROUNDS> {
    type Hasher = PaddedHasher<ROUNDS>;

    fn build_hasher(&self) -> Self::Hasher {
        static SEED: FixedState = FixedState::with_seed(0);
        PaddedHasher(SEED.build_hasher())
    }
}

impl<const ROUNDS: usize> Hasher for PaddedHasher<ROUNDS> {
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    fn write_u64(&mut self, x: u64) {
        self.0.write_u64(x);
    }

    fn finish(&self) -> u64 {
        let mut hash = self.0.finish();
        for _ in 0..ROUNDS {
            hash = (hash ^ (hash >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        }
        hash
    }
}

/// [`hash_join`], but hashing `BATCH` keys before it inserts or looks up any
/// of them, as engines do. hashbrown's `HashTable`, the table under std's
/// `HashMap`, takes a hash the caller computed. Each entry holds the same two
/// references `hash_join`'s does, and an insert looks for the key first as
/// `HashMap::insert` does, so the table and its reads are the same.
pub fn batched_hash_join<const BATCH: usize, S: Sink>(
    build: &[Row],
    probe: &[Row],
    hasher: impl BuildHasher,
    out: &mut S,
) {
    let mut table: HashTable<(&u64, &Row)> = HashTable::with_capacity(build.len());
    let rehash = |&(key, _): &(&u64, &Row)| hasher.hash_one(key);
    let mut hashes = [0; BATCH];
    for rows in build.chunks(BATCH) {
        for (hash, row) in hashes.iter_mut().zip(rows) {
            *hash = hasher.hash_one(row.0);
        }
        for (&hash, row) in hashes.iter().zip(rows) {
            match table.entry(hash, |&(key, _)| *key == row.0, rehash) {
                Entry::Occupied(mut held) => *held.get_mut() = (&row.0, row),
                Entry::Vacant(slot) => {
                    slot.insert((&row.0, row));
                }
            }
        }
    }
    for rows in probe.chunks(BATCH) {
        for (hash, row) in hashes.iter_mut().zip(rows) {
            *hash = hasher.hash_one(row.0);
        }
        for (&hash, row) in hashes.iter().zip(rows) {
            if let Some(&(_, found)) = table.find(hash, |&(key, _)| *key == row.0) {
                out.pair(found, row);
            }
        }
    }
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

/// The bytes of each structure a join at `rows_a_side` reads at random.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    pub control: usize,
    pub buckets: usize,
    pub rows: usize,
}

/// The structures of a join at `rows_a_side`.
pub fn footprint(rows_a_side: usize) -> Footprint {
    let buckets = buckets(rows_a_side);
    Footprint {
        control: buckets,
        // `BUCKET` counts each bucket's control byte too.
        buckets: (BUCKET - 1) * buckets,
        rows: rows_a_side * size_of::<Row>(),
    }
}

impl Footprint {
    pub fn total(self) -> usize {
        self.control + self.buckets + self.rows
    }
}

/// Reads per input row that reach DRAM, by what they read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reads {
    pub control: f64,
    pub buckets: f64,
    pub rows: f64,
}

impl Reads {
    pub fn total(self) -> f64 {
        self.control + self.buckets + self.rows
    }
}

/// How many of a join's reads reach DRAM, on the workload where half the keys
/// match. A build row reads a control group and writes a bucket; a probe row
/// reads a control group, and half of them then read a bucket and a build
/// row. So each input row makes 2 of these reads and writes on average: 1 of
/// control bytes, 0.75 of buckets and 0.25 of build rows, all of which reach
/// DRAM once nothing fits in L2.
///
/// Each loop is modelled apart, by [`uncached`].
pub fn dram_reads(rows_a_side: usize, l2_bytes: usize, line_bytes: usize) -> Reads {
    let size = footprint(rows_a_side);
    let lines = |bytes: usize| (bytes / line_bytes) as f64;
    let (control, slots, rows) = (lines(size.control), lines(size.buckets), lines(size.rows));
    let capacity = (l2_bytes / line_bytes) as f64;
    // The scanned rows pass through L2, a new line every so many rows.
    let scan = size_of::<Row>() as f64 / line_bytes as f64;
    let build = uncached(&[(control, 1.0), (slots, 1.0)], scan, capacity);
    let probe = uncached(&[(control, 1.0), (slots, 0.5), (rows, 0.5)], scan, capacity);
    // The two loops have one row each of the two an input row averages.
    Reads {
        control: (build[0] + probe[0]) / 2.0,
        buckets: (build[1] + probe[1]) / 2.0,
        rows: probe[2] / 2.0,
    }
}

/// Reads per row that miss an LRU cache of `capacity` lines, for each of
/// `structures`, each its lines and reads per row, read uniformly at random,
/// while a scan adds `scan` new lines per row. This is Che's approximation:
/// a line stays for a fixed time after its last read, so a line read at rate
/// r is present with probability 1 - e^(-rT), and T is what fills the cache.
fn uncached(structures: &[(f64, f64)], scan: f64, capacity: f64) -> Vec<f64> {
    let held = |t: f64| {
        scan * t
            + structures
                .iter()
                .map(|&(lines, reads)| lines * -(-reads / lines * t).exp_m1())
                .sum::<f64>()
    };
    // `held` rises with T, so bisect. T is in rows, and no line outlives the
    // rows it would take a scan alone to fill the cache.
    let (mut low, mut high) = (0.0, capacity / scan);
    for _ in 0..100 {
        let t = (low + high) / 2.0;
        if held(t) < capacity {
            low = t;
        } else {
            high = t;
        }
    }
    structures
        .iter()
        .map(|&(lines, reads)| reads * (-reads / lines * low).exp())
        .collect()
}

/// The middle of `samples`.
fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// What [`timed_hash_join`] measured of each loop.
#[derive(Clone, Copy, Debug)]
pub struct Loops {
    pub build: Duration,
    pub probe: Duration,
    /// Instructions retired while building, allocating the table among them.
    pub build_instructions: u64,
    pub probe_instructions: u64,
}

/// [`hash_join`](algs::join::hash_join) with a clock and a count of
/// instructions between its two loops. A test holds it to the original. The
/// counts are read outside both clocks.
#[inline]
pub fn timed_hash_join<S: Sink>(
    build: &[Row],
    probe: &[Row],
    hasher: impl BuildHasher,
    out: &mut S,
) -> Loops {
    let before = instructions();
    let start = Instant::now();
    let mut table = std::collections::HashMap::with_capacity_and_hasher(build.len(), hasher);
    for row in build {
        table.insert(&row.0, row);
    }
    let build_time = start.elapsed();
    let built = instructions();
    let start = Instant::now();
    for row in probe {
        if let Some(found) = table.get(&row.0) {
            out.pair(found, row);
        }
    }
    let probe_time = start.elapsed();
    Loops {
        build: build_time,
        probe: probe_time,
        build_instructions: built - before,
        probe_instructions: instructions() - built,
    }
}

/// A join's two sides and how many pairs it emits.
struct Sides {
    left: Vec<Row>,
    right: Vec<Row>,
    expected: u64,
}

/// `probe`'s rows, those whose key `build` holds first, each part in its
/// original order.
#[inline]
pub fn matches_first(build: &[Row], probe: &[Row]) -> Vec<Row> {
    let held: std::collections::HashSet<u64> = build.iter().map(|row| row.0).collect();
    let (mut first, rest): (Vec<Row>, Vec<Row>) =
        probe.iter().partition(|row| held.contains(&row.0));
    first.extend(rest);
    first
}

/// Times the hash join's loops at `rows_a_side`, on the main case's inputs and
/// its rotation, for the same time as a criterion case and at least as many
/// samples. Each sample is two calls: one on the input, and one on it with the
/// probe side's matching rows first, of which only the probe loop counts.
///
/// # Panics
///
/// If a call emits the wrong number of pairs.
#[inline]
pub fn time_phases(rows_a_side: usize) -> Phases {
    let instances = harness::rotation(2 * rows_a_side as u64);
    // Only the sides, and the probe side reordered: at 64M rows a side the
    // sorted copies would hold 2 GiB more, enough to page.
    let inputs: Vec<_> = (0..instances)
        .map(|i| {
            let workload::Binary {
                left,
                right,
                expected,
                ..
            } = workload::one_to_one(rows_a_side, i);
            let grouped = matches_first(&left, &right);
            (
                Sides {
                    left,
                    right,
                    expected,
                },
                grouped,
            )
        })
        .collect();
    let mut next = inputs.iter().cycle();
    let mut call = || {
        let (input, grouped) = next.next().expect("a case has an instance");
        let mut tally = Tally::default();
        let start = Instant::now();
        let loops = timed_hash_join(
            black_box(&input.left),
            black_box(&input.right),
            FixedState::default(),
            &mut tally,
        );
        let whole = start.elapsed();
        assert_eq!(tally.tuples, input.expected, "at {rows_a_side} rows a side");
        let mut tally = Tally::default();
        let grouped_probe = timed_hash_join(
            black_box(&input.left),
            black_box(grouped),
            FixedState::default(),
            &mut tally,
        )
        .probe;
        assert_eq!(
            tally.tuples, input.expected,
            "at {rows_a_side} rows a side, matches first"
        );
        let [build, probe, whole, grouped_probe] =
            [loops.build, loops.probe, whole, grouped_probe].map(|time| time.as_nanos() as f64);
        [
            build,
            probe,
            whole,
            grouped_probe,
            loops.build_instructions as f64,
            loops.probe_instructions as f64,
        ]
    };
    (0..instances).for_each(|_| {
        call();
    });
    let start = Instant::now();
    let mut samples = Vec::new();
    while samples.len() < harness::SAMPLES || start.elapsed() < harness::MEASUREMENT {
        samples.push(call());
    }
    let per_row = |phase: usize| {
        let mut times: Vec<f64> = samples.iter().map(|s| s[phase]).collect();
        median(&mut times) / (2 * rows_a_side) as f64
    };
    // Each loop's instructions over its own rows, not both sides'.
    let instructions = |phase: usize| 2.0 * per_row(phase);
    Phases {
        rows_a_side,
        build_ns: per_row(0),
        probe_ns: per_row(1),
        grouped_probe_ns: per_row(3),
        whole_ns: per_row(2),
        build_instructions: instructions(4),
        probe_instructions: instructions(5),
    }
}

/// Times the probe loop alone at `rows_a_side`, on the main case's first
/// input with [`ABSENT`] added to every probe key, with each of
/// [`ABSENT_HASHINGS`]. The table holds none of those keys, so a probe row
/// reads a group of control bytes and, but for the one probe in thirty whose
/// group holds a matching tag, nothing else: what the control bytes cost,
/// apart from the buckets and the build rows.
///
/// # Panics
///
/// If a probe finds a key.
#[inline]
pub fn time_absent_probes(rows_a_side: usize) -> Vec<AbsentProbe> {
    fn probe<S: BuildHasher + Default>(build: &[Row], absent: &[Row]) -> (f64, f64) {
        let mut table =
            std::collections::HashMap::with_capacity_and_hasher(build.len(), S::default());
        for row in build {
            table.insert(&row.0, row);
        }
        let call = || {
            let mut tally = Tally::default();
            let counted = instructions();
            let start = Instant::now();
            for row in black_box(absent) {
                if let Some(found) = table.get(&row.0) {
                    tally.pair(found, row);
                }
            }
            let elapsed = start.elapsed();
            let retired = instructions() - counted;
            assert_eq!(tally.tuples, 0, "the table holds an absent key");
            let rows = absent.len() as f64;
            (elapsed.as_nanos() as f64 / rows, retired as f64 / rows)
        };
        call();
        let start = Instant::now();
        let mut samples = Vec::new();
        while samples.len() < READ_SAMPLES || start.elapsed() < harness::MEASUREMENT {
            samples.push(call());
        }
        let (mut ns, mut counts): (Vec<f64>, Vec<f64>) = samples.into_iter().unzip();
        (median(&mut ns), median(&mut counts))
    }

    // Only the sides: see `time_phases`.
    let workload::Binary { left, right, .. } = workload::one_to_one(rows_a_side, 0);
    let absent: Vec<Row> = right.iter().map(|row| (row.0 + ABSENT, row.1)).collect();
    drop(right);
    ABSENT_HASHINGS
        .iter()
        .map(|&hashing| {
            let (ns, instructions) = match hashing {
                Hashing::Fold => probe::<FixedState>(&left, &absent),
                Hashing::Padded16 => probe::<Padded<16>>(&left, &absent),
                Hashing::Padded32 => probe::<Padded<32>>(&left, &absent),
                other => unreachable!("{other:?} isn't an absent hashing"),
            };
            AbsentProbe {
                rows_a_side,
                hashing: hashing.label().to_owned(),
                ns,
                instructions,
            }
        })
        .collect()
}

/// Times and counts the instructions of each of [`HASHINGS`] at
/// `rows_a_side`, on the main case's inputs and its rotation, as
/// [`time_phases`] does.
///
/// # Panics
///
/// If a call emits the wrong number of pairs.
#[inline]
pub fn time_hashers(rows_a_side: usize) -> Vec<HasherCost> {
    let instances = harness::rotation(2 * rows_a_side as u64);
    let inputs: Vec<_> = (0..instances)
        .map(|i| workload::one_to_one(rows_a_side, i))
        .collect();
    let rows = (2 * rows_a_side) as f64;
    HASHINGS
        .iter()
        .map(|&hashing| {
            let mut next = inputs.iter().cycle();
            let mut call = || {
                let input = next.next().expect("a case has an instance");
                let mut tally = Tally::default();
                let counted = instructions();
                let start = Instant::now();
                hashing.join(black_box(&input.left), black_box(&input.right), &mut tally);
                let elapsed = start.elapsed();
                let retired = instructions() - counted;
                assert_eq!(tally.tuples, input.expected, "{hashing:?} at {rows_a_side}");
                (elapsed.as_nanos() as f64 / rows, retired as f64 / rows)
            };
            (0..instances).for_each(|_| {
                call();
            });
            let start = Instant::now();
            let mut samples = Vec::new();
            while samples.len() < harness::SAMPLES || start.elapsed() < harness::MEASUREMENT {
                samples.push(call());
            }
            let (mut ns, mut counts): (Vec<f64>, Vec<f64>) = samples.into_iter().unzip();
            HasherCost {
                rows_a_side,
                hashing: hashing.label().to_owned(),
                ns: median(&mut ns),
                instructions: median(&mut counts),
            }
        })
        .collect()
}

/// Reads `lines` one line apart in a random cycle, each read's address coming
/// from the last, in indices rather than pointers, so a prefetcher that
/// follows pointers can't help.
#[inline]
fn chase(cells: &mut [u32], lines: &[u32], rng: &mut Rng) -> f64 {
    // Sattolo's shuffle: one cycle through every line.
    let mut order = lines.to_vec();
    for i in (1..order.len()).rev() {
        let j = rng.below(i as u64) as usize;
        order.swap(i, j);
    }
    for (i, &line) in order.iter().enumerate() {
        cells[line as usize] = order[(i + 1) % order.len()];
    }
    let hops = (2 * lines.len()).max(1 << 22);
    let mut at = order[0];
    for _ in 0..lines.len() {
        at = cells[at as usize];
    }
    let mut samples: Vec<f64> = (0..READ_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..hops {
                at = cells[at as usize];
            }
            start.elapsed().as_nanos() as f64 / hops as f64
        })
        .collect();
    black_box(at);
    median(&mut samples)
}

/// Reads every line of `order`, none depending on the last.
#[inline]
fn sweep(cells: &[u32], order: &[u32]) -> f64 {
    let read = || {
        order
            .iter()
            .fold(0u64, |sum, &line| sum + u64::from(cells[line as usize]))
    };
    black_box(read());
    let passes = ((1 << 22) / order.len()).max(1);
    let mut samples: Vec<f64> = (0..READ_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..passes {
                black_box(read());
            }
            start.elapsed().as_nanos() as f64 / (passes * order.len()) as f64
        })
        .collect();
    median(&mut samples)
}

/// What a read costs across `bytes`, with reads `line_bytes` apart.
#[inline]
pub fn time_reads(bytes: usize, line_bytes: usize) -> ReadCost {
    let cell = size_of::<u32>();
    let stride = line_bytes / cell;
    let mut rng = Rng::seeded(&format!("reads/{bytes}"));
    let mut cells = vec![1u32; bytes / cell];
    let lines: Vec<u32> = (0..bytes / line_bytes)
        .map(|line| (line * stride) as u32)
        .collect();
    let mut shuffled = lines.clone();
    rng.shuffle(&mut shuffled);
    let mut paged = lines.clone();
    for window in paged.chunks_mut(WINDOW / line_bytes) {
        rng.shuffle(window);
    }
    let overlapped_ns = sweep(&cells, &shuffled);
    let paged_ns = sweep(&cells, &paged);
    let chased_ns = chase(&mut cells, &lines, &mut rng);
    ReadCost {
        bytes,
        chased_ns,
        overlapped_ns,
        paged_ns,
    }
}

/// `ROUNDS` rounds of arithmetic on `line`, each a multiply, a rotate, a shift
/// and three more: instructions that read nothing, and that nothing waits on
/// but the sum they're folded into. The rounds don't feed each other, so the
/// core can run them side by side.
#[inline(always)]
fn work<const ROUNDS: usize>(line: u32) -> u64 {
    const ODD: [u64; 4] = [
        0x9E37_79B9_7F4A_7C15,
        0xBF58_476D_1CE4_E5B9,
        0x94D0_49BB_1331_11EB,
        0xD6E8_FEB8_6659_FD93,
    ];
    let x = u64::from(line) | 1;
    let mut folded = 0u64;
    for round in 0..ROUNDS {
        let mixed = x.wrapping_mul(ODD[round % 4]).rotate_left(round as u32 + 1);
        folded ^= (mixed ^ (mixed >> 29)).wrapping_add(ODD[(round + 3) % 4]);
    }
    folded
}

/// Reads every line of `order` as [`sweep`] does, with `ROUNDS` rounds of
/// [`work`] beside each read: the ns and the instructions a read.
#[inline]
fn worked_sweep<const ROUNDS: usize>(cells: &[u32], order: &[u32]) -> (f64, f64) {
    let read = || {
        order.iter().fold(0u64, |sum, &line| {
            sum.wrapping_add(u64::from(cells[line as usize])) ^ work::<ROUNDS>(line)
        })
    };
    black_box(read());
    let passes = ((1 << 23) / order.len()).max(1);
    let reads = (passes * order.len()) as f64;
    let samples = (0..READ_SAMPLES).map(|_| {
        let counted = instructions();
        let start = Instant::now();
        for _ in 0..passes {
            black_box(read());
        }
        let elapsed = start.elapsed();
        let retired = instructions() - counted;
        (elapsed.as_nanos() as f64 / reads, retired as f64 / reads)
    });
    let (mut ns, mut counts): (Vec<f64>, Vec<f64>) = samples.unzip();
    (median(&mut ns), median(&mut counts))
}

/// What an overlapped read costs with each of [`WORK_ROUNDS`] of unrelated
/// work beside it, in L2 and past it, with reads `line_bytes` apart. A join's
/// row hashes, compares and emits around its reads; this is a read with that
/// much to carry, and nothing else changed.
#[inline]
pub fn time_worked_reads(line_bytes: usize) -> Vec<WorkedRead> {
    let cell = size_of::<u32>();
    let stride = line_bytes / cell;
    let shuffled = |bytes: usize| {
        let mut rng = Rng::seeded(&format!("worked reads/{bytes}"));
        let mut order: Vec<u32> = (0..bytes / line_bytes)
            .map(|line| (line * stride) as u32)
            .collect();
        rng.shuffle(&mut order);
        (vec![1u32; bytes / cell], order)
    };
    let (small, small_order) = shuffled(WORKED_IN_L2);
    let (large, large_order) = shuffled(WORKED_PAST_L2);
    let sweep = |rounds: usize, cells: &[u32], order: &[u32]| match rounds {
        0 => worked_sweep::<0>(cells, order),
        4 => worked_sweep::<4>(cells, order),
        8 => worked_sweep::<8>(cells, order),
        12 => worked_sweep::<12>(cells, order),
        16 => worked_sweep::<16>(cells, order),
        other => unreachable!("{other} isn't one of WORK_ROUNDS"),
    };
    WORK_ROUNDS
        .iter()
        .map(|&rounds| {
            let (l2_ns, _) = sweep(rounds, &small, &small_order);
            let (dram_ns, instructions) = sweep(rounds, &large, &large_order);
            WorkedRead {
                rounds,
                instructions,
                l2_ns,
                dram_ns,
            }
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::joins::Checksum;

    const L2: usize = 20 << 20;
    const LINE: usize = 128;

    #[test]
    fn matches_first_keeps_every_row_and_puts_the_matches_first() {
        let input = workload::one_to_one(1 << 10, 0);
        let grouped = matches_first(&input.left, &input.right);
        let held: std::collections::HashSet<u64> = input.left.iter().map(|row| row.0).collect();
        let matched = grouped
            .iter()
            .take_while(|row| held.contains(&row.0))
            .count();
        assert_eq!(matched as u64, input.expected);
        assert!(grouped[matched..].iter().all(|row| !held.contains(&row.0)));
        let (mut a, mut b) = (grouped, input.right.clone());
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }

    #[test]
    fn the_clocked_join_matches_the_join() {
        let input = workload::one_to_one(1 << 12, 0);
        let mut joined = Checksum::default();
        hash_join(&input.left, &input.right, FixedState::default(), |l, r| {
            joined.pair(l, r);
        });
        let mut clocked = Checksum::default();
        timed_hash_join(
            &input.left,
            &input.right,
            FixedState::default(),
            &mut clocked,
        );
        assert_eq!(joined, clocked);
    }

    // The counts a sweep reports are the whole process's, and the tests share
    // one, so this checks the work itself; the run reports its instructions.
    #[test]
    fn no_rounds_of_work_leave_a_sweep_as_it_was() {
        assert_eq!(work::<0>(12_345), 0);
        assert_ne!(work::<4>(12_345), 0);
        assert_ne!(work::<8>(12_345), work::<16>(12_345));
        let lines: Vec<u32> = (0..1 << 10).collect();
        let cells = vec![1u32; 1 << 10];
        let (ns, instructions) = worked_sweep::<4>(&cells, &lines);
        assert!(ns > 0.0 && instructions > 0.0);
    }

    #[test]
    fn an_absent_probe_finds_nothing_with_each_hashing() {
        let probes = time_absent_probes(1 << 10);
        let labels: Vec<&str> = probes.iter().map(|p| p.hashing.as_str()).collect();
        assert_eq!(labels, ABSENT_HASHINGS.map(Hashing::label));
        assert!(probes.iter().all(|p| p.ns > 0.0 && p.instructions > 0.0));
    }

    #[test]
    fn every_hashing_finds_the_same_pairs() {
        // Odd, so the last batch is short.
        let input = workload::one_to_one((1 << 12) + 3, 0);
        let mut plain = Checksum::default();
        Hashing::Fold.join(&input.left, &input.right, &mut plain);
        for hashing in HASHINGS {
            let mut joined = Checksum::default();
            hashing.join(&input.left, &input.right, &mut joined);
            assert_eq!(joined, plain, "{hashing:?}");
        }
    }

    #[test]
    fn a_batched_build_keeps_the_last_row_with_a_key() {
        let mut pairs = Vec::new();
        let mut out = Collect(&mut pairs);
        batched_hash_join::<16, _>(
            &[(1, 10), (1, 11)],
            &[(1, 20)],
            FixedState::default(),
            &mut out,
        );
        assert_eq!(pairs, [((1, 11), (1, 20))]);
    }

    struct Collect<'a>(&'a mut Vec<(Row, Row)>);

    impl Sink for Collect<'_> {
        fn pair(&mut self, left: &Row, right: &Row) {
            self.0.push((*left, *right));
        }

        fn tuple(&mut self, _: &[&Row]) {
            unreachable!("a binary join")
        }

        fn tuple_permuted(&mut self, _: &[&Row], _: &[usize]) {
            unreachable!("a binary join")
        }
    }

    #[test]
    fn padding_runs_more_instructions() {
        let keys: Vec<u64> = (0..1 << 16).collect();
        let count = |hash: &dyn Fn(u64) -> u64| {
            let before = instructions();
            black_box(
                keys.iter()
                    .fold(0u64, |sum, &k| sum.wrapping_add(hash(black_box(k)))),
            );
            instructions() - before
        };
        let plain = count(&|k| FixedState::default().hash_one(k));
        let padded = count(&|k| Padded::<32>.hash_one(k));
        assert!(
            padded > plain + 32 * keys.len() as u64,
            "{plain} then {padded}"
        );
    }

    #[test]
    fn a_table_is_its_control_bytes_and_its_buckets() {
        let size = footprint(1 << 20);
        assert_eq!(size.control, 2 << 20);
        assert_eq!(size.buckets, 32 << 20);
        assert_eq!(size.rows, 16 << 20);
    }

    #[test]
    fn nothing_reaches_dram_while_everything_fits() {
        assert!(dram_reads(1 << 14, L2, LINE).total() < 1e-6);
    }

    #[test]
    fn at_most_every_read_reaches_dram() {
        let reads = dram_reads(1 << 40, L2, LINE);
        assert!((reads.control - 1.0).abs() < 1e-3, "{reads:?}");
        assert!((reads.buckets - 0.75).abs() < 1e-3, "{reads:?}");
        assert!((reads.rows - 0.25).abs() < 1e-3, "{reads:?}");
    }

    #[test]
    fn reads_reach_dram_more_often_at_every_size() {
        let reads: Vec<f64> = SIZES
            .iter()
            .map(|&n| dram_reads(n, L2, LINE).total())
            .collect();
        assert!(reads.is_sorted(), "{reads:?}");
    }

    #[test]
    fn a_read_more_likely_to_recur_stays_longer() {
        // At 4M, the control bytes are a sixteenth as many lines as the
        // buckets, so a far smaller share of their reads miss.
        let reads = dram_reads(1 << 22, L2, LINE);
        assert!(reads.control < reads.buckets / 2.0, "{reads:?}");
    }

    #[test]
    fn a_chase_visits_every_line_once_a_cycle() {
        let mut cells = vec![0u32; 1 << 10];
        let lines: Vec<u32> = (0..16).map(|line| line * 32).collect();
        chase(&mut cells, &lines, &mut Rng::seeded("test"));
        let mut at = 0;
        let mut seen = Vec::new();
        for _ in 0..lines.len() {
            seen.push(at);
            at = cells[at as usize];
        }
        assert_eq!(at, 0);
        seen.sort_unstable();
        assert_eq!(seen, lines);
    }
}

mod inlining;
pub use inlining::{
    Apart, CALL_INSTRUCTIONS, INLINING_SIZES, Inlining, InliningPart, hash_join_inserting,
    time_inlining,
};
