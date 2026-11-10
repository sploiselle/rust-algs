//! Measures what a read costs: see [`bench_cache::time_reads`] and
//! [`bench_cache::time_worked_reads`].

use bench_cache::{
    ReadsPart, Sentinels, WORKING_SETS, benching, line_bytes, time_reads, time_worked_reads,
    write_part,
};

fn main() {
    let started = Sentinels::start();
    let line_bytes = line_bytes();
    let sets = if benching() {
        &WORKING_SETS[..]
    } else {
        &WORKING_SETS[..1]
    };
    let reads: Vec<_> = sets
        .iter()
        .map(|&bytes| time_reads(bytes, line_bytes))
        .collect();
    for r in &reads {
        println!(
            "{:>11} bytes: chased {:6.2}, overlapped {:5.2}, paged {:5.2} ns per read",
            r.bytes, r.chased_ns, r.overlapped_ns, r.paged_ns
        );
    }
    // A gibibyte of reads, so not in a smoke run.
    let worked_reads = if benching() {
        time_worked_reads(line_bytes)
    } else {
        Vec::new()
    };
    for w in &worked_reads {
        println!(
            "{:>2} rounds of work, {:5.1} instructions a read: {:5.2} ns in L2, {:5.2} past it",
            w.rounds, w.instructions, w.l2_ns, w.dram_ns
        );
    }
    let sentinels = started.finish();
    if benching() {
        write_part(
            "reads",
            &ReadsPart {
                sentinels,
                line_bytes,
                reads,
                worked_reads,
            },
        );
    }
}
