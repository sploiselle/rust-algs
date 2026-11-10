//! Probes for keys the table doesn't hold: see
//! [`bench_cache::time_absent_probes`].

use bench_cache::{
    ABSENT_SIZES, AbsentPart, Sentinel, benching, sentinel_ns, time_absent_probes, write_part,
};

fn main() {
    let start_ns = sentinel_ns();
    let sizes = if benching() {
        &ABSENT_SIZES[..]
    } else {
        &ABSENT_SIZES[..1]
    };
    let absent: Vec<_> = sizes.iter().flat_map(|&n| time_absent_probes(n)).collect();
    for a in &absent {
        println!(
            "{:>9} rows a side, absent keys: {:>22} {:6.2} ns, {:6.1} instructions per probe row",
            a.rows_a_side, a.hashing, a.ns, a.instructions
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
        write_part("absent", &AbsentPart { sentinel, absent });
    }
}
