//! Profiles every case's memory with a counting allocator, asserts every
//! [claim](bench::cases::memory_claim) about it, and writes the profiles to
//! `target/bench/memory.json`. The binary plans' peaks have no claim: they are
//! reported, not asserted.
//!
//! Each case is profiled on its workload's first instance alone. The timed
//! calls rotate through several, but every instance has the same row counts
//! and joins to the same number of tuples, which is all the claims read.
//!
//! Each case runs twice: hashers have fixed seeds and the program is single
//! threaded, so allocation is deterministic, and the two must agree exactly.
//! The first argument not starting with `-` filters cases by id; cargo passes
//! `--bench`. Run without it, as `cargo test --benches` does, it checks
//! everything but writes nothing.

use std::collections::BTreeSet;

use bench::Checksum;
use bench::alloc::{self, CountingAlloc};
use bench::cases::{self, Algorithm, Case, Data, GROUPS};
use bench::record::{self, Memory, MemoryCase, Run};
use bench::report::{bytes, count};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn main() {
    assert!(
        alloc::installed(),
        "CountingAlloc isn't the global allocator"
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    let benching = args.iter().any(|arg| arg == "--bench");
    let filter = args.iter().find(|arg| !arg.starts_with('-')).cloned();
    let run = Run::now("memory");
    if benching {
        record::append(&run);
    }

    let mut profiles = Vec::new();
    for group in GROUPS.iter().filter(|group| !group.is_sentinel()) {
        for &param in group.params {
            let chosen: Vec<Case> = group
                .cases_at(param)
                .filter(|case| filter.as_ref().is_none_or(|f| case.to_string().contains(f)))
                .collect();
            if chosen.is_empty() {
                continue;
            }
            let data = group.workload.generate(param, 0);
            let mut agreed = None;
            for case in chosen {
                profiles.push(profile(&case, &data, &mut agreed));
            }
        }
    }
    flat_in_n(
        &profiles,
        "multiway_selective",
        &[Algorithm::Heap, Algorithm::Leapfrog],
    );

    for p in &profiles {
        println!(
            "{:<42} {:>10} in {:>9} allocations",
            format!("{}/{}/{}", p.group, p.algorithm, p.param),
            bytes(p.usage.peak_bytes as u64),
            count(p.usage.allocations as u64)
        );
    }
    println!("{} cases profiled; every claim holds", profiles.len());
    if benching {
        write(Memory {
            run,
            filter,
            cases: profiles,
        });
    }
}

/// Profiles `case` on `data` and asserts what the post claims about it.
///
/// # Panics
///
/// If the two runs allocate differently, the case keeps memory past its
/// return, its output is wrong, or a claim fails.
fn profile(case: &Case, data: &Data, agreed: &mut Option<Checksum>) -> MemoryCase {
    // Preparing is outside the measurement, as it's outside the timing.
    let prepared = data.prepare();
    let run = || {
        alloc::measure(|| {
            let mut out = Checksum::default();
            case.algorithm.run(&prepared, &mut out);
            out
        })
    };
    let (usage, out) = run();
    let (again, _) = run();
    assert_eq!(usage, again, "{case} allocated differently the second time");
    assert_eq!(
        usage.retained_bytes, 0,
        "{case} kept memory after returning"
    );
    cases::verify(case, data, out, agreed);
    if let Some(claim) = cases::memory_claim(case.algorithm, data) {
        claim
            .check(&usage)
            .unwrap_or_else(|err| panic!("{case}: {err}"));
    }
    MemoryCase {
        group: case.group.name.to_owned(),
        algorithm: case.algorithm.name().to_owned(),
        param: case.param,
        input_rows: data.input_rows(),
        input_bytes: data.input_bytes(),
        checksum: out.sum,
        usage,
    }
}

/// Asserts that each of `algorithms` has the same peak at every parameter of
/// `group`: its memory doesn't grow with the input.
fn flat_in_n(profiles: &[MemoryCase], group: &str, algorithms: &[Algorithm]) {
    for alg in algorithms {
        let peaks: BTreeSet<usize> = profiles
            .iter()
            .filter(|p| p.group == group && p.algorithm == alg.name())
            .map(|p| p.usage.peak_bytes)
            .collect();
        assert!(
            peaks.len() <= 1,
            "{group}/{}: peaks vary with n: {peaks:?}",
            alg.name()
        );
    }
}

/// Writes the profiles to `target/bench/memory.json`.
fn write(memory: Memory) {
    let path = record::memory_path();
    let json = serde_json::to_string_pretty(&memory).expect("profiles serialize");
    std::fs::create_dir_all(record::bench_dir())
        .and_then(|()| std::fs::write(&path, json))
        .unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
    println!("wrote {}", path.display());
}
