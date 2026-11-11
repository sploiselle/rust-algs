//! A model of how many of the hash join's reads reach DRAM at each size.
//!
//! The join's random reads go to its table's control bytes, its buckets and,
//! through a bucket's reference, the build rows. [`footprint`] says how big
//! each is, and [`dram_reads`] how many reads of each miss a cache the size
//! of L2. The tables multiply that by what a read costs, as the `reads`
//! measure times it, and set the product beside what the join takes.
//!
//! Nothing here is measured: it's arithmetic on the sizes, which is why it's
//! with the report and not with the measures.

use bench_joins::Row;
use bench_joins::harness::{BUCKET, buckets};

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

#[cfg(test)]
mod tests {
    use super::*;

    const L2: usize = 20 << 20;
    const LINE: usize = 128;

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
        let reads: Vec<f64> = bench_joins::cache::SIZES
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
}
