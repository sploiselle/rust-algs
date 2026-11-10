//! The timing harness, compiled into each group's binary: `benches/<group>.rs`
//! includes this file and then `groups/<group>.rs`, whose `group!` becomes the
//! binary's `main`.
//!
//! A group has a binary of its own so that its results depend on nothing but
//! its own code. The report trusts a timing for as long as the binary that
//! took it still builds byte for byte, and a binary holds only the joins and
//! the workload its group names. Adding a group adds a binary and leaves the
//! others as they were; changing a join changes the binaries that time it.
//! This file is in all of them, so a change here reruns them all.
//!
//! Each case's check runs it once on every instance of its input into a
//! [`Checksum`]. The timed calls emit into a [`Tally`] instead, so each
//! tuple's cost is the same small, fixed work for every join: a count, a read
//! of every row's payload, and an XOR the compiler can't drop or hoist. A
//! checksum in the timed calls would add three SplitMix64 rounds or more to
//! every tuple.
//!
//! The timed calls take a case's [instances](harness::instances) in turn, by
//! a step of a cycle over a slice, the same for every join, so no instance
//! repeats within [`ROTATION_ROWS`](harness::ROTATION_ROWS) input rows.
//!
//! Every case is sampled flat, the same way: see [`harness::SAMPLES`]. The
//! system allocator is untouched here: counting would add to every
//! allocation the timed code makes.
//!
//! The binary records nothing about itself. The runner that starts it logs
//! the commit and the binary's hash, so bookkeeping can change without
//! changing what's measured.

use std::hint::black_box;

use bench::harness::{self, Data, Prepared, Sentinel};
use bench::{Checksum, Tally};
use criterion::{BenchmarkId, Criterion, SamplingMode, Throughput};

/// One of a group's joins: [`bench::joins::Join`], as the functions a binary
/// calls.
pub struct Entry {
    pub name: &'static str,
    pub runs_at: fn(usize) -> bool,
    pub time: fn(&Prepared, &mut Tally),
    pub check: fn(&Prepared, &mut Checksum),
}

/// A group, as its binary runs it.
pub struct Spec {
    pub name: &'static str,
    pub generate: fn(usize, usize) -> Data,
    pub params: &'static [usize],
    pub joins: &'static [Entry],
}

/// A group's definition, as its binary's `main`. The plan crate reads the
/// same file with a `group!` of its own, as a row of the plan.
macro_rules! group {
    (
        name: $name:literal,
        workload: $kind:ident => $generate:ident,
        params: [$($param:expr),* $(,)?],
        algorithms: [$($join:ident),* $(,)?] $(,)?
    ) => {
        fn main() {
            timing::run(&timing::Spec {
                name: $name,
                generate: |param, instance| {
                    bench::harness::Data::from(bench::workload::$generate(param, instance))
                },
                params: &[$($param),*],
                joins: &[$(timing::Entry {
                    name: <bench::joins::$join as bench::joins::Join>::NAME,
                    runs_at: <bench::joins::$join as bench::joins::Join>::runs_at,
                    time: <bench::joins::$join as bench::joins::Join>::run::<bench::Tally>,
                    check: <bench::joins::$join as bench::joins::Join>::run::<bench::Checksum>,
                }),*],
            });
        }
    };
}

/// Times every case of `spec` with criterion, after checking it, between two
/// timings of each [sentinel](harness::Sentinel).
pub fn run(spec: &Spec) {
    let configured = || {
        Criterion::default()
            .warm_up_time(harness::WARM_UP)
            .measurement_time(harness::MEASUREMENT)
            .sample_size(harness::SAMPLES)
            .without_plots()
            .configure_from_args()
    };
    let mut c = configured();
    // The sentinels run whatever filter the cases run under: a rerun of one
    // case needs to know what machine it ran on as much as a whole run does.
    let mut unfiltered = configured().with_filter("");
    sentinels(&mut unfiltered, spec.name, "start");
    let mut g = c.benchmark_group(spec.name);
    g.sampling_mode(SamplingMode::Flat);
    // Parameters outermost, so each one's instances are built once and freed
    // after.
    for &param in spec.params {
        let instances = harness::instances(spec.generate, param);
        let prepared: Vec<Prepared> = instances.iter().map(Data::prepare).collect();
        // Every instance has the same row counts.
        g.throughput(Throughput::Elements(instances[0].input_rows()));
        // The joins agree on each instance's checksum.
        let mut agreed = vec![None; instances.len()];
        for join in spec.joins.iter().filter(|join| (join.runs_at)(param)) {
            // A wrong join can't post a time, on any instance.
            let case = format!("{}/{}/{param}", spec.name, join.name);
            for ((data, prepared), agreed) in instances.iter().zip(&prepared).zip(&mut agreed) {
                let mut out = Checksum::default();
                (join.check)(prepared, &mut out);
                harness::verify(&case, data, out, agreed);
            }

            let mut next = prepared.iter().cycle();
            g.bench_function(BenchmarkId::new(join.name, param), |b| {
                b.iter(|| {
                    let prepared = next.next().expect("a case has an instance");
                    let mut tally = Tally::default();
                    (join.time)(black_box(prepared), &mut tally);
                    tally.tuples
                });
            });
        }
    }
    g.finish();
    sentinels(&mut unfiltered, spec.name, "end");
    c.final_summary();
}

/// Times each [sentinel](harness::Sentinel) under
/// `<group>_<its suffix>/<edge>/<its words>`: `start` before the group's
/// first case, `end` after its last.
fn sentinels(c: &mut Criterion, group: &str, edge: &str) {
    for sentinel in Sentinel::ALL {
        let mut work = sentinel.work();
        let mut g = c.benchmark_group(format!("{group}_{}", sentinel.suffix()));
        g.sampling_mode(SamplingMode::Flat);
        g.bench_function(BenchmarkId::new(edge, sentinel.words()), |b| {
            b.iter(|| work.call());
        });
        g.finish();
    }
}
