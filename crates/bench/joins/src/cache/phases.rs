//! The hash join's two loops, timed apart.

use super::*;

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

/// What the `phases` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PhasesPart {
    pub sentinels: Sentinels,
    pub l2_bytes: usize,
    pub line_bytes: usize,
    pub phases: Vec<Phases>,
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
