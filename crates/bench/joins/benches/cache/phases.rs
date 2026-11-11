//! Times the hash join's two loops apart: see [`bench_joins::cache::time_phases`].

use bench_joins::cache::{
    PhasesPart, SIZES, Sentinels, benching, l2_bytes, line_bytes, time_phases, write_part,
};

fn main() {
    let started = Sentinels::start();
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
    let sentinels = started.finish();
    if benching() {
        write_part(
            "phases",
            &PhasesPart {
                sentinels,
                l2_bytes: l2_bytes(),
                line_bytes: line_bytes(),
                phases,
            },
        );
    }
}
