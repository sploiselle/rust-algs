//! Times every case in [`GROUPS`] with criterion, after checking it.
//!
//! Each case's check runs it once on every instance of its input into a
//! [`Checksum`]. The timed calls emit into a [`Tally`] instead, so each
//! tuple's cost is the same small, fixed work for every join: a count, a read
//! of every row's payload, and an XOR the compiler can't drop or hoist. A
//! checksum in the timed calls would add three SplitMix64 rounds or more to
//! every tuple.
//!
//! The timed calls take a case's [instances](cases::Workload::instances) in
//! turn, by a step of a cycle over a slice, the same for every algorithm, so
//! no instance repeats within [`ROTATION_ROWS`](cases::ROTATION_ROWS) input
//! rows. It says what that takes from a timing, and what it leaves in cache.
//!
//! Every case is sampled flat, the same way: see [`cases::SAMPLES`]. A filter
//! (`cargo bench -p bench --bench joins -- one_to_one`) narrows what criterion
//! times, but every instance is still generated and every case still checked
//! on each, which takes about 12 s, most of it the nested loop's checks. The
//! system allocator is untouched here: counting would add to every
//! allocation the timed code makes.

use std::hint::black_box;

use bench::cases::{self, Data, GROUPS, Prepared};
use bench::record::{self, Run};
use bench::{Checksum, Tally};
use criterion::{BenchmarkId, Criterion, SamplingMode, Throughput};

fn main() {
    let mut c = Criterion::default()
        .warm_up_time(cases::WARM_UP)
        .measurement_time(cases::MEASUREMENT)
        .sample_size(cases::SAMPLES)
        .without_plots()
        .configure_from_args();
    if timing() {
        record::append(&Run::now("joins"));
    }
    for group in GROUPS {
        let mut g = c.benchmark_group(group.name);
        g.sampling_mode(SamplingMode::Flat);
        // Parameters outermost, so each workload's instances are built once and
        // freed after.
        for &param in group.params {
            let instances = group.workload.instances(param);
            let prepared: Vec<Prepared> = instances.iter().map(Data::prepare).collect();
            // Every instance has the same row counts.
            g.throughput(Throughput::Elements(instances[0].input_rows()));
            // The algorithms agree on each instance's checksum.
            let mut agreed = vec![None; instances.len()];
            for case in group.cases_at(param) {
                // A wrong join can't post a time, on any instance.
                for ((data, prepared), agreed) in instances.iter().zip(&prepared).zip(&mut agreed) {
                    let mut out = Checksum::default();
                    case.algorithm.run(prepared, &mut out);
                    cases::verify(&case, data, out, agreed);
                }

                let id = BenchmarkId::new(case.algorithm.name(), param);
                let mut next = prepared.iter().cycle();
                g.bench_function(id, |b| {
                    b.iter(|| {
                        let prepared = next.next().expect("a case has an instance");
                        let mut tally = Tally::default();
                        case.algorithm.run_into(black_box(prepared), &mut tally);
                        tally.tuples
                    });
                });
            }
        }
        g.finish();
    }
    c.final_summary();
}

/// Whether criterion will time anything. `cargo test --benches` runs each
/// case once (`--test`), and `--list` and `--profile-time` save no results;
/// recording those runs would make the report think every timing stale.
fn timing() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag| record::has_flag(&args, flag);
    has("--bench") && !has("--test") && !has("--list") && !has("--profile-time")
}
