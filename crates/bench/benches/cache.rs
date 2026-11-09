//! Measures why the hash join's cost per row climbs (see [`bench::cache`]) and
//! writes it to `target/bench/cache.json`. It has its own binary so the timing
//! target's code, whose placement moves its times, doesn't change with it.
//!
//! cargo passes `--bench`. Run without it, as `cargo test --benches` does, it
//! measures the smallest sizes and writes nothing.

use bench::cache::{self, HASHER_SIZES, SIZES, WORKING_SETS};
use bench::record::{self, Run};

fn main() {
    let benching = std::env::args().any(|arg| arg == "--bench");
    let run = Run::now("cache");
    if !benching {
        cache::measure(run, &SIZES[..1], &WORKING_SETS[..1], &HASHER_SIZES[..1]);
        return;
    }
    record::append(&run);
    let measured = cache::measure(run, &SIZES, &WORKING_SETS, &HASHER_SIZES);
    for p in &measured.phases {
        println!(
            "{:>9} rows a side: build {:5.2}, probe {:5.2} ({:5.2} matches first), whole {:5.2} ns per input row",
            p.rows_a_side, p.build_ns, p.probe_ns, p.grouped_probe_ns, p.whole_ns
        );
    }
    for r in &measured.reads {
        println!(
            "{:>11} bytes: chased {:6.2}, overlapped {:5.2}, paged {:5.2} ns per read",
            r.bytes, r.chased_ns, r.overlapped_ns, r.paged_ns
        );
    }
    for h in &measured.hashers {
        println!(
            "{:>9} rows a side: {:>22} {:6.2} ns, {:6.1} instructions per input row",
            h.rows_a_side, h.hashing, h.ns, h.instructions
        );
    }
    let path = record::cache_path();
    let json = serde_json::to_string_pretty(&measured).expect("measures serialize");
    std::fs::write(&path, json).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
    println!("wrote {}", path.display());
}
