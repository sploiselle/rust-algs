//! What a random read costs: one at a time, overlapped, and with unrelated
//! work beside it.

use super::*;

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
pub(super) const WINDOW: usize = 1 << 20;

/// Samples of each read cost, and the fewest of each join.
pub(super) const READ_SAMPLES: usize = 5;

/// Rounds of unrelated work [`time_worked_reads`] gives each read. Past 16
/// the compiler stops unrolling the rounds, and the instructions a read jump.
pub const WORK_ROUNDS: [usize; 5] = [0, 4, 8, 12, 16];

/// Bytes [`time_worked_reads`] reads across: 4 MiB, which L2 holds, and
/// 1 GiB, of which it holds a fiftieth.
pub(super) const WORKED_IN_L2: usize = 1 << 22;

pub(super) const WORKED_PAST_L2: usize = 1 << 30;

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

/// What the `reads` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReadsPart {
    pub sentinels: Sentinels,
    pub line_bytes: usize,
    pub reads: Vec<ReadCost>,
    pub worked_reads: Vec<WorkedRead>,
}

/// Reads `lines` one line apart in a random cycle, each read's address coming
/// from the last, in indices rather than pointers, so a prefetcher that
/// follows pointers can't help.
#[inline]
pub(super) fn chase(cells: &mut [u32], lines: &[u32], rng: &mut Rng) -> f64 {
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
pub(super) fn sweep(cells: &[u32], order: &[u32]) -> f64 {
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
pub(super) fn work<const ROUNDS: usize>(line: u32) -> u64 {
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
pub(super) fn worked_sweep<const ROUNDS: usize>(cells: &[u32], order: &[u32]) -> (f64, f64) {
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
