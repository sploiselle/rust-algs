//! Whether misses overlap: several pointer chases at once.
//!
//! [`time_reads`] finds a read costs about 18 times less overlapped than one
//! at a time, which says about 18 are in flight at once only if each still
//! takes as long. This measures it by setting how many may overlap: within a
//! chase every read waits on the one before, and between chases nothing does.

use super::*;

/// How many chases [`time_chains`] follows at once.
pub const CHASES: [usize; 13] = [1, 2, 3, 4, 6, 8, 12, 16, 20, 24, 32, 48, 64];

/// Bytes the chases read across: 2 GiB, the largest of [`WORKING_SETS`], of
/// which L2 holds a hundredth.
pub const CHAINS_BYTES: usize = 1 << 31;

/// Reads a sample, shared among its chases.
const CHAIN_READS: usize = 1 << 23;

/// What a read costs with `chases` chases followed at once, the median in ns
/// per read.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Chained {
    pub chases: usize,
    pub ns: f64,
}

/// What the `chains` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChainsPart {
    pub sentinels: Sentinels,
    pub line_bytes: usize,
    pub bytes: usize,
    pub chained: Vec<Chained>,
}

/// Follows `K` chases through `cells` from `starts`, a read from each in
/// turn: the ns per read. A chase's next address comes from its own last
/// read and from no other chase's, so at most `K` reads can be in flight.
#[inline]
pub(super) fn chained<const K: usize>(cells: &[u32], starts: &[u32], reads: usize) -> f64 {
    let mut at = [0u32; K];
    at.copy_from_slice(starts);
    let hops = (reads / K).max(1);
    let follow = |at: &mut [u32; K]| {
        for _ in 0..hops {
            for a in at.iter_mut() {
                *a = cells[*a as usize];
            }
        }
    };
    follow(&mut at);
    let mut samples: Vec<f64> = (0..READ_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            follow(&mut at);
            start.elapsed().as_nanos() as f64 / (hops * K) as f64
        })
        .collect();
    black_box(at);
    median(&mut samples)
}

/// Links a line of `cells` every `line_bytes` into one random cycle, each
/// holding the index of the next, and returns the lines in the cycle's order.
#[inline]
pub(super) fn link(cells: &mut [u32], line_bytes: usize, rng: &mut Rng) -> Vec<u32> {
    let stride = line_bytes / size_of::<u32>();
    let mut order: Vec<u32> = (0..cells.len() / stride)
        .map(|line| (line * stride) as u32)
        .collect();
    // Sattolo's shuffle: one cycle through every line.
    for i in (1..order.len()).rev() {
        let j = rng.below(i as u64) as usize;
        order.swap(i, j);
    }
    for (i, &line) in order.iter().enumerate() {
        cells[line as usize] = order[(i + 1) % order.len()];
    }
    order
}

/// What a read costs across `bytes`, with reads `line_bytes` apart, following
/// each of `counts` chases at once. The chases start evenly round one cycle
/// through every line, so none reads a line another read lately.
///
/// # Panics
///
/// If a count isn't one of [`CHASES`].
#[inline]
pub fn time_chains(bytes: usize, line_bytes: usize, counts: &[usize]) -> Vec<Chained> {
    let mut rng = Rng::seeded(&format!("chains/{bytes}"));
    let mut cells = vec![1u32; bytes / size_of::<u32>()];
    let order = link(&mut cells, line_bytes, &mut rng);
    counts
        .iter()
        .map(|&chases| {
            let starts: Vec<u32> = (0..chases)
                .map(|chase| order[chase * order.len() / chases])
                .collect();
            let ns = match chases {
                1 => chained::<1>(&cells, &starts, CHAIN_READS),
                2 => chained::<2>(&cells, &starts, CHAIN_READS),
                3 => chained::<3>(&cells, &starts, CHAIN_READS),
                4 => chained::<4>(&cells, &starts, CHAIN_READS),
                6 => chained::<6>(&cells, &starts, CHAIN_READS),
                8 => chained::<8>(&cells, &starts, CHAIN_READS),
                12 => chained::<12>(&cells, &starts, CHAIN_READS),
                16 => chained::<16>(&cells, &starts, CHAIN_READS),
                20 => chained::<20>(&cells, &starts, CHAIN_READS),
                24 => chained::<24>(&cells, &starts, CHAIN_READS),
                32 => chained::<32>(&cells, &starts, CHAIN_READS),
                48 => chained::<48>(&cells, &starts, CHAIN_READS),
                64 => chained::<64>(&cells, &starts, CHAIN_READS),
                other => panic!("{other} isn't one of CHASES"),
            };
            Chained { chases, ns }
        })
        .collect()
}
