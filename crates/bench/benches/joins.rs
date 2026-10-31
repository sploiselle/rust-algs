//! Times every case in [`GROUPS`] with criterion, after checking it.
//!
//! Each case's check runs it once into a [`Checksum`]. The timed calls emit
//! into a [`Tally`] instead, so each tuple's cost is the same small, fixed
//! work for every join: a count, a read of every row's payload, and an XOR
//! the compiler can't drop or hoist. A checksum in the timed calls would add
//! three SplitMix64 rounds or more to every tuple.
//!
//! Every case is sampled flat, the same way: see [`cases::SAMPLES`]. A filter
//! (`cargo bench -p bench --bench joins -- one_to_one`) narrows what criterion
//! times, but every workload is still generated and every case still checked,
//! which takes about 3 s. The system allocator is untouched here: counting
//! would add to every allocation the timed code makes.

use std::hint::black_box;

use bench::cases::{self, GROUPS};
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
        // Parameters outermost, so each workload is built once and freed after.
        for &param in group.params {
            let data = group.workload.generate(param);
            let prepared = data.prepare();
            g.throughput(Throughput::Elements(data.input_rows()));
            let mut agreed = None;
            for case in group.cases_at(param) {
                // A wrong join can't post a time.
                let mut out = Checksum::default();
                case.algorithm.run(&prepared, &mut out);
                cases::verify(&case, &data, out, &mut agreed);

                let id = BenchmarkId::new(case.algorithm.name(), param);
                g.bench_with_input(id, &prepared, |b, prepared| {
                    b.iter(|| {
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
