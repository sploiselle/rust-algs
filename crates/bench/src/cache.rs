//! Why the hash join's cost per row climbs as its inputs grow.
//!
//! A hash join's random reads go to three structures: its table's control
//! bytes, its table's buckets, and, through a bucket's reference, the build
//! rows. The rows being scanned are read in order, and a prefetcher keeps up.
//! Once those structures outgrow L2, some of the reads reach DRAM, and more of
//! them the larger the structures are:
//!
//! - [`time_phases`] times the join's build and probe loops apart, and the
//!   probe loop again with its matching rows first;
//! - [`time_reads`] measures what a read costs, one at a time and overlapped;
//! - [`dram_reads`] models how many reads reach DRAM at each size.
//!
//! The report multiplies the last two, and sets that beside the first.
//!
//! [`time_hashers`] asks why a slow hasher costs more past L2 rather than
//! less. While a row waits on DRAM, the core works ahead on the rows behind
//! it, but only as many as fit in its window of instructions, so a row with
//! more instructions leaves fewer reads in flight. It times the join with
//! hashers that run more instructions, and counts them, and times it again
//! hashing a batch of keys before their lookups, which keeps the hash's
//! instructions out of the window those lookups wait in, once a batch's
//! lookups are enough to fill it.

use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hasher};
use std::hint::black_box;
use std::time::{Duration, Instant};

use algs::join::hash_join;
use foldhash::fast::{FixedState, FoldHasher};
use hashbrown::hash_table::{Entry, HashTable};
use serde::{Deserialize, Serialize};

use crate::cases::{self, BUCKET, buckets};
use crate::record::{self, Run};
use crate::workload::{self, Rng};
use crate::{Row, Sink, Tally};

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

/// What the cache harness wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Cache {
    pub run: Run,
    pub l2_bytes: usize,
    pub line_bytes: usize,
    pub phases: Vec<Phases>,
    pub reads: Vec<ReadCost>,
    pub hashers: Vec<HasherCost>,
}

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

/// [`hash_join`](algs::join::hash_join) with a clock between its two loops. A test holds it to the
/// original.
pub fn timed_hash_join<S: Sink>(
    build: &[Row],
    probe: &[Row],
    hasher: impl BuildHasher,
    out: &mut S,
) -> (Duration, Duration) {
    let start = Instant::now();
    let mut table = std::collections::HashMap::with_capacity_and_hasher(build.len(), hasher);
    for row in build {
        table.insert(&row.0, row);
    }
    let built = Instant::now();
    for row in probe {
        if let Some(found) = table.get(&row.0) {
            out.pair(found, row);
        }
    }
    (built - start, built.elapsed())
}

/// A join's two sides and how many pairs it emits.
struct Sides {
    left: Vec<Row>,
    right: Vec<Row>,
    expected: u64,
}

/// `probe`'s rows, those whose key `build` holds first, each part in its
/// original order.
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
pub fn time_phases(rows_a_side: usize) -> Phases {
    let instances = cases::rotation(2 * rows_a_side as u64);
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
        let (build, probe) = timed_hash_join(
            black_box(&input.left),
            black_box(&input.right),
            FixedState::default(),
            &mut tally,
        );
        let whole = start.elapsed();
        assert_eq!(tally.tuples, input.expected, "at {rows_a_side} rows a side");
        let mut tally = Tally::default();
        let (_, grouped_probe) = timed_hash_join(
            black_box(&input.left),
            black_box(grouped),
            FixedState::default(),
            &mut tally,
        );
        assert_eq!(
            tally.tuples, input.expected,
            "at {rows_a_side} rows a side, matches first"
        );
        [build, probe, whole, grouped_probe].map(|time| time.as_nanos() as f64)
    };
    (0..instances).for_each(|_| {
        call();
    });
    let start = Instant::now();
    let mut samples = Vec::new();
    while samples.len() < cases::SAMPLES || start.elapsed() < cases::MEASUREMENT {
        samples.push(call());
    }
    let per_row = |phase: usize| {
        let mut times: Vec<f64> = samples.iter().map(|s| s[phase]).collect();
        median(&mut times) / (2 * rows_a_side) as f64
    };
    Phases {
        rows_a_side,
        build_ns: per_row(0),
        probe_ns: per_row(1),
        grouped_probe_ns: per_row(3),
        whole_ns: per_row(2),
    }
}

/// Times and counts the instructions of each of [`HASHINGS`] at
/// `rows_a_side`, on the main case's inputs and its rotation, as
/// [`time_phases`] does.
///
/// # Panics
///
/// If a call emits the wrong number of pairs.
pub fn time_hashers(rows_a_side: usize) -> Vec<HasherCost> {
    let instances = cases::rotation(2 * rows_a_side as u64);
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
            while samples.len() < cases::SAMPLES || start.elapsed() < cases::MEASUREMENT {
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

/// `name`, from sysctl.
///
/// # Panics
///
/// If the machine doesn't report it, since a cache this harness can't size
/// can't be modelled.
fn sysctl(name: &str) -> usize {
    record::output("sysctl", &["-n", name])
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("sysctl {name} gave no number"))
}

/// Measures everything, for `run`, at `sizes`, `working_sets` and
/// `hasher_sizes`.
pub fn measure(run: Run, sizes: &[usize], working_sets: &[usize], hasher_sizes: &[usize]) -> Cache {
    let line_bytes = sysctl("hw.cachelinesize");
    let reads = working_sets
        .iter()
        .map(|&bytes| time_reads(bytes, line_bytes))
        .collect();
    Cache {
        run,
        l2_bytes: sysctl("hw.perflevel0.l2cachesize"),
        line_bytes,
        phases: sizes.iter().map(|&n| time_phases(n)).collect(),
        reads,
        hashers: hasher_sizes.iter().flat_map(|&n| time_hashers(n)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Checksum;

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
