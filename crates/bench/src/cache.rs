//! Why the hash join's cost per row climbs as its inputs grow.
//!
//! A hash join's random reads go to three structures: its table's control
//! bytes, its table's buckets, and, through a bucket's reference, the build
//! rows. The rows being scanned are read in order, and a prefetcher keeps up.
//! Once those structures outgrow L2, some of the reads reach DRAM, and more of
//! them the larger the structures are:
//!
//! - [`time_phases`] times the join's build and probe loops apart;
//! - [`time_reads`] measures what a read costs, one at a time and overlapped;
//! - [`dram_reads`] models how many reads reach DRAM at each size.
//!
//! The report multiplies the last two, and sets that beside the first.

use std::hash::BuildHasher;
use std::hint::black_box;
use std::time::{Duration, Instant};

use foldhash::fast::FixedState;
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

/// What the cache harness wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Cache {
    pub run: Run,
    pub l2_bytes: usize,
    pub line_bytes: usize,
    pub phases: Vec<Phases>,
    pub reads: Vec<ReadCost>,
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

/// Times the hash join's loops at `rows_a_side`, on the main case's inputs and
/// its rotation, for the same time as a criterion case and at least as many
/// samples.
///
/// # Panics
///
/// If a call emits the wrong number of pairs.
pub fn time_phases(rows_a_side: usize) -> Phases {
    let instances = cases::rotation(2 * rows_a_side as u64);
    let inputs: Vec<_> = (0..instances)
        .map(|i| workload::one_to_one(rows_a_side, i))
        .collect();
    let mut next = inputs.iter().cycle();
    let mut call = || {
        let input = next.next().expect("a case has an instance");
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
        [build, probe, whole].map(|time| time.as_nanos() as f64)
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
        whole_ns: per_row(2),
    }
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

/// Measures everything, for `run`, at `sizes` and `working_sets`.
pub fn measure(run: Run, sizes: &[usize], working_sets: &[usize]) -> Cache {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Checksum;
    use algs::join::hash_join;

    const L2: usize = 20 << 20;
    const LINE: usize = 128;

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
