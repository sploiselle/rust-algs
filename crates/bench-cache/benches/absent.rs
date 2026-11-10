//! Probes for keys the table doesn't hold: see
//! [`bench_cache::time_absent_probes`].

use bench_cache::{ABSENT_SIZES, AbsentPart, Sentinels, benching, time_absent_probes, write_part};

fn main() {
    let started = Sentinels::start();
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
    let sentinels = started.finish();
    if benching() {
        write_part("absent", &AbsentPart { sentinels, absent });
    }
}
