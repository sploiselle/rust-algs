//! Times the hash join with each hashing: see [`bench_cache::time_hashers`].

use bench_cache::{
    HASHER_SIZES, HashersPart, Sentinel, benching, sentinel_ns, time_hashers, write_part,
};

fn main() {
    let start_ns = sentinel_ns();
    let sizes = if benching() {
        &HASHER_SIZES[..]
    } else {
        &HASHER_SIZES[..1]
    };
    let hashers: Vec<_> = sizes.iter().flat_map(|&n| time_hashers(n)).collect();
    for h in &hashers {
        println!(
            "{:>9} rows a side: {:>22} {:6.2} ns, {:6.1} instructions per input row",
            h.rows_a_side, h.hashing, h.ns, h.instructions
        );
    }
    let sentinel = Sentinel {
        start_ns,
        end_ns: sentinel_ns(),
    };
    println!(
        "sentinel: {:.1} µs before, {:.1} µs after",
        sentinel.start_ns / 1e3,
        sentinel.end_ns / 1e3
    );
    if benching() {
        write_part("hashers", &HashersPart { sentinel, hashers });
    }
}
