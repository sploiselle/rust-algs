//! Probes for keys the table doesn't hold, which read control bytes and
//! nothing else.

use super::*;

/// Rows a side [`time_absent_probes`] runs at: 1M, where L2 holds the control
/// bytes, to 64M, where it holds a sixth of them.
pub const ABSENT_SIZES: [usize; 6] = [1 << 20, 1 << 22, 1 << 23, 1 << 24, 1 << 25, 1 << 26];

/// What [`time_absent_probes`] adds to every key, so the table holds none of
/// them: the workloads' keys are below 2^28.
pub(super) const ABSENT: u64 = 1 << 40;

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

/// What the `absent` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AbsentPart {
    pub sentinels: Sentinels,
    pub absent: Vec<AbsentProbe>,
}

/// The hashings [`time_absent_probes`] probes with: foldhash, and foldhash
/// with instructions added and no reads.
pub const ABSENT_HASHINGS: [Hashing; 3] = [Hashing::Fold, Hashing::Padded16, Hashing::Padded32];

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
