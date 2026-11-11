//! Times the hash join with the table's insert inlined and called, beside
//! sort-merge: see [`bench_joins::cache::time_inlining`].

use bench_joins::cache::{
    CALL_INSTRUCTIONS, INLINING_SIZES, InliningPart, Sentinels, benching, time_inlining, write_part,
};

fn main() {
    let started = Sentinels::start();
    let sizes = if benching() {
        &INLINING_SIZES[..]
    } else {
        &INLINING_SIZES[..1]
    };
    let inlining: Vec<_> = sizes.iter().map(|&n| time_inlining(n)).collect();
    for i in &inlining {
        println!(
            "{:>9} rows a side: sort-merge {:5.2}, hash with the insert inlined {:5.2}, called {:5.2} ns per input row; {:.1} and {:.1} instructions a build row",
            i.rows_a_side,
            i.sort_merge_ns,
            i.inlined_ns,
            i.called_ns,
            i.inlined_build_instructions,
            i.called_build_instructions
        );
        // The two joins differ only if the compiler inlined the one it was
        // free to. If it didn't, the columns' names are false: say so, and
        // write nothing.
        assert!(
            i.called_build_instructions - i.inlined_build_instructions >= CALL_INSTRUCTIONS,
            "at {} rows a side the build loop retires {:.1} instructions a row with the insert \
             left to the compiler and {:.1} with it called: the compiler didn't inline it",
            i.rows_a_side,
            i.inlined_build_instructions,
            i.called_build_instructions
        );
    }
    let sentinels = started.finish();
    if benching() {
        write_part(
            "inlining",
            &InliningPart {
                sentinels,
                inlining,
            },
        );
    }
}
