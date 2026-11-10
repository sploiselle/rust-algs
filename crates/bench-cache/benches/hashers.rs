//! Times the hash join with each hashing: see [`bench_cache::time_hashers`].

use bench_cache::{HASHER_SIZES, HashersPart, Sentinels, benching, time_hashers, write_part};

fn main() {
    let started = Sentinels::start();
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
    let sentinels = started.finish();
    if benching() {
        write_part("hashers", &HashersPart { sentinels, hashers });
    }
}
