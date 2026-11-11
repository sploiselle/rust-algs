//! Times several pointer chases at once: see
//! [`bench_joins::cache::time_chains`].

use bench_joins::cache::{
    CHAINS_BYTES, CHASES, ChainsPart, Sentinels, benching, line_bytes, time_chains, write_part,
};

fn main() {
    let started = Sentinels::start();
    let line_bytes = line_bytes();
    // Two gibibytes of reads, so a megabyte's and two counts in a smoke run.
    let (bytes, counts) = if benching() {
        (CHAINS_BYTES, &CHASES[..])
    } else {
        (1 << 20, &CHASES[..2])
    };
    let chained = time_chains(bytes, line_bytes, counts);
    for c in &chained {
        println!("{:>2} chases at once: {:5.2} ns per read", c.chases, c.ns);
    }
    let sentinels = started.finish();
    if benching() {
        write_part(
            "chains",
            &ChainsPart {
                sentinels,
                line_bytes,
                bytes,
                chained,
            },
        );
    }
}
