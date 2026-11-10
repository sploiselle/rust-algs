//! Times the hash join's two loops apart: see [`bench_cache::time_phases`].

use bench_cache::{
    PhasesPart, SIZES, Sentinel, benching, l2_bytes, line_bytes, sentinel_ns, time_phases,
    write_part,
};

fn main() {
    let start_ns = sentinel_ns();
    let sizes = if benching() { &SIZES[..] } else { &SIZES[..1] };
    let phases: Vec<_> = sizes.iter().map(|&n| time_phases(n)).collect();
    for p in &phases {
        println!(
            "{:>9} rows a side: build {:5.2}, probe {:5.2} ({:5.2} matches first), whole {:5.2} ns per input row; {:.1} and {:.1} instructions a row",
            p.rows_a_side,
            p.build_ns,
            p.probe_ns,
            p.grouped_probe_ns,
            p.whole_ns,
            p.build_instructions,
            p.probe_instructions
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
        write_part(
            "phases",
            &PhasesPart {
                sentinel,
                l2_bytes: l2_bytes(),
                line_bytes: line_bytes(),
                phases,
            },
        );
    }
}
