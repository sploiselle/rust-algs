use super::*;
use crate::joins::profile::Memory;
use crate::record::Kept;
use crate::vouch::Log;
use bench_joins_plan::alloc::Usage;
use evidence::{Checked, Kind as TargetKind, Written};

const RUSTC: &str = "rustc 1.98.1 (48a229cea 2026-09-01)";

/// A run of `target`, of the binary [`targets`] says it builds to now.
fn run(target: &str, started_unix_ms: u64, commit: &str, dirty: bool, rustc: &str) -> Run {
    Run {
        target: target.to_owned(),
        started_unix_ms,
        commit: commit.to_owned(),
        dirty,
        rustc: rustc.to_owned(),
        cpu: "Apple M6".to_owned(),
        os: "27.0".to_owned(),
        args: Vec::new(),
        binary: format!("{target}-bin"),
        // As a run logged before the source was recorded: the tests
        // that follow tell builds apart by their binaries.
        sources: String::new(),
        ended_unix_ms: None,
    }
}

/// A run of `target`, of `binary`.
fn of(target: &str, started_unix_ms: u64, binary: &str) -> Run {
    Run {
        binary: binary.to_owned(),
        ..run(target, started_unix_ms, "aaaaaaa000", false, RUSTC)
    }
}

/// Every bench target's name and kind, in run order.
fn names() -> Vec<(&'static str, TargetKind)> {
    cases::GROUPS
        .iter()
        .map(|group| (group.name, TargetKind::Timing))
        .chain([("memory", TargetKind::Memory)])
        .chain(cache::PARTS.map(|part| (part, TargetKind::Cache)))
        .collect()
}

/// Every bench target, building to the binary `run` records.
fn targets() -> Vec<Target> {
    names()
        .into_iter()
        .map(|(name, _)| Target {
            name: name.to_owned(),
            directory: PathBuf::new(),
            executable: PathBuf::new(),
            binary: format!("{name}-bin"),
            sources: format!("{name}-src"),
            files: Vec::new(),
        })
        .collect()
}

/// A run of every target at `commit`, each starting at `started_unix_ms`.
fn log(started_unix_ms: u64, commit: &str, dirty: bool) -> Vec<Run> {
    names()
        .into_iter()
        .map(|(name, _)| run(name, started_unix_ms, commit, dirty, RUSTC))
        .collect()
}

fn timing(written_unix_ms: u64) -> Timing {
    around(1000.0, written_unix_ms)
}

/// A timing of `median_ns` with a ±1% confidence interval.
fn around(median_ns: f64, written_unix_ms: u64) -> Timing {
    Timing {
        median_ns,
        lower_ns: median_ns * 0.99,
        upper_ns: median_ns * 1.01,
        confidence_level: 0.95,
        input_rows: Some(1000),
        written_unix_ms,
    }
}

/// A timing for every case and every sentinel, written at
/// `written_unix_ms`.
fn every_timing(written_unix_ms: u64) -> HashMap<Key, Timing> {
    cases::GROUPS
        .iter()
        .flat_map(evidence::timing_keys)
        .map(|(_, key)| (key, timing(written_unix_ms)))
        .collect()
}

/// A memory profile of every case.
fn memory() -> Memory {
    let cases = cases::cases()
        .map(|case| MemoryCase {
            group: case.group.name.to_owned(),
            algorithm: case.algorithm.name().to_owned(),
            param: case.param,
            input_rows: 1,
            input_bytes: 16,
            checksum: 0,
            usage: Usage::default(),
        })
        .collect();
    Memory {
        filter: None,
        cases,
    }
}

/// The cache targets' measures.
fn cache() -> Cache {
    let phases = |rows_a_side, whole_ns| cache::Phases {
        rows_a_side,
        build_ns: whole_ns / 3.0,
        probe_ns: whole_ns * 2.0 / 3.0,
        grouped_probe_ns: whole_ns / 2.0,
        whole_ns,
        build_instructions: 56.0,
        probe_instructions: 29.0,
    };
    let cost = |bytes, ns| cache::ReadCost {
        bytes,
        chased_ns: 20.0 * ns,
        overlapped_ns: ns,
        paged_ns: ns - 1.0,
    };
    Cache {
        l2_bytes: 20 << 20,
        line_bytes: 128,
        phases: vec![
            phases(1 << 16, 3.0),
            phases(1 << 18, 4.0),
            phases(1 << 26, 15.0),
        ],
        reads: vec![cost(1 << 20, 2.0), cost(1 << 31, 5.0)],
        hashers: [1 << 14, 1 << 22]
            .into_iter()
            .flat_map(|rows_a_side| {
                cache::HASHINGS.map(|hashing| cache::HasherCost {
                    rows_a_side,
                    hashing: hashing.label().to_owned(),
                    ns: 10.0,
                    instructions: 40.0,
                })
            })
            .collect(),
        worked_reads: vec![
            cache::WorkedRead {
                rounds: 0,
                instructions: 7.2,
                l2_ns: 1.0,
                dram_ns: 5.0,
            },
            cache::WorkedRead {
                rounds: 8,
                instructions: 48.4,
                l2_ns: 2.0,
                dram_ns: 9.5,
            },
        ],
        absent: [1 << 20, 1 << 26]
            .into_iter()
            .flat_map(|rows_a_side| {
                cache::ABSENT_HASHINGS.map(|hashing| cache::AbsentProbe {
                    rows_a_side,
                    hashing: hashing.label().to_owned(),
                    ns: 3.0,
                    instructions: 23.4,
                })
            })
            .collect(),
        inlining: [1 << 20, 1 << 26]
            .into_iter()
            .map(|rows_a_side| cache::Inlining {
                rows_a_side,
                sort_merge_ns: 15.0,
                inlined_ns: 14.0,
                called_ns: 16.0,
                inlined_build_instructions: 55.6,
                called_build_instructions: 69.5,
            })
            .collect(),
        order: [1 << 16, 1 << 26]
            .into_iter()
            .map(|rows_a_side| cache::Order {
                rows_a_side,
                random_ns: 15.0,
                ordered_ns: 5.0,
                lined_ns: 7.0,
                random_instructions: 44.0,
                ordered_instructions: 45.0,
                lined_instructions: 45.0,
            })
            .collect(),
        chains_bytes: 1 << 31,
        chained: [(1, 90.0), (2, 45.0), (32, 5.0)]
            .into_iter()
            .map(|(chases, ns)| cache::Chained { chases, ns })
            .collect(),
    }
}

/// A cache measure's sentinels, each reading the same before and after.
fn steady(narrow_ns: f64, wide_ns: f64) -> cache::Sentinels {
    let both = |ns| cache::Sentinel {
        start_ns: ns,
        end_ns: ns,
    };
    cache::Sentinels {
        narrow: both(narrow_ns),
        wide: both(wide_ns),
    }
}

/// The run log `runs`, `timings`, and every other result written at
/// `written_unix_ms`.
fn evidence(runs: Vec<Run>, timings: HashMap<Key, Timing>, written_unix_ms: u64) -> Evidence {
    let cache = cache();
    // As steady as the timings' sentinels, which `timing` makes 1,000 ns.
    let sentinels = steady(1000.0, 1000.0);
    Evidence {
        log: Log {
            runs,
            ..Log::default()
        },
        checked: Vec::new(),
        cpu: "a cpu".to_owned(),
        os: "an os".to_owned(),
        timings,
        memory: Some(Written {
            value: memory(),
            written_unix_ms,
        }),
        phases: Some(Written {
            value: cache::PhasesPart {
                sentinels,
                l2_bytes: cache.l2_bytes,
                line_bytes: cache.line_bytes,
                phases: cache.phases,
            },
            written_unix_ms,
        }),
        reads: Some(Written {
            value: cache::ReadsPart {
                sentinels,
                line_bytes: cache.line_bytes,
                reads: cache.reads,
                worked_reads: cache.worked_reads,
            },
            written_unix_ms,
        }),
        hashers: Some(Written {
            value: cache::HashersPart {
                sentinels,
                hashers: cache.hashers,
            },
            written_unix_ms,
        }),
        absent: Some(Written {
            value: cache::AbsentPart {
                sentinels,
                absent: cache.absent,
            },
            written_unix_ms,
        }),
        inlining: Some(Written {
            value: cache::InliningPart {
                sentinels,
                inlining: cache.inlining,
            },
            written_unix_ms,
        }),
        order: Some(Written {
            value: cache::OrderPart {
                sentinels,
                order: cache.order,
            },
            written_unix_ms,
        }),
        chains: Some(Written {
            value: cache::ChainsPart {
                sentinels,
                line_bytes: cache.line_bytes,
                bytes: cache.chains_bytes,
                chained: cache.chained,
            },
            written_unix_ms,
        }),
    }
}

/// Results with `timings`, every target cited at commit `aaaaaaa`.
fn results(timings: HashMap<Key, Timing>) -> Results {
    Results {
        timings,
        memory: HashMap::new(),
        cache: None,
        cited: log(0, "aaaaaaa000", false)
            .into_iter()
            .map(|run| (run.target.clone(), run))
            .collect(),
        allowed: Vec::new(),
    }
}

/// `results` with the cache measures, cited at `commit`.
fn with_cache(mut results: Results, commit: &str) -> Results {
    results.cache = Some(cache());
    for part in cache::PARTS {
        results
            .cited
            .insert(part.to_owned(), run(part, 0, commit, false, RUSTC));
    }
    results
}

const STRICT: Options = Options {
    out: PathBuf::new(),
    allow_stale: false,
    allow_dirty: false,
};

#[test]
fn every_quoted_memory_number_has_a_claim() {
    use std::collections::BTreeMap;
    // The algorithms each table or fact quotes a peak or allocation count
    // of, by the workload they ran on.
    let mut quoted: BTreeMap<(&str, usize), Vec<Algorithm>> = BTreeMap::new();
    let mut quote = |group: &'static str, algorithm: Algorithm, param: usize| {
        quoted.entry((group, param)).or_default().push(algorithm);
    };
    for table in TABLES {
        match &table.kind {
            Kind::Profile { cases, .. } => {
                for (c, param) in *cases {
                    quote(c.group, c.algorithm, *param);
                }
            }
            Kind::Grid {
                columns,
                metric: Metric::Peak,
                ..
            } => {
                for c in *columns {
                    for case in cases::group(c.group).cases() {
                        if case.algorithm == c.algorithm {
                            quote(c.group, c.algorithm, case.param);
                        }
                    }
                }
            }
            Kind::Grid { .. } => {}
        }
    }
    for fact in FACTS {
        let quantities = match &fact.value {
            Value::Ratio(a, b) => vec![a, b],
            Value::Drift(_) => vec![],
            Value::Single(a) => vec![a],
            Value::Crossover { .. } => vec![],
        };
        for q in quantities {
            if matches!(q.measure, Measure::Peak | Measure::Allocations) {
                quote(q.group, q.algorithm, q.param);
            }
        }
    }

    for ((name, param), algorithms) in quoted {
        let data = (cases::group(name).generate)(param, 0);
        for alg in algorithms {
            assert!(
                cases::memory_claim(alg, &data).is_some() || cases::REPORTED_ONLY.contains(&alg),
                "{name}/{}/{param} is quoted, but no claim checks it",
                alg.name()
            );
        }
    }
}

#[test]
fn a_target_is_out_of_date_until_the_binary_built_now_has_run() {
    let evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    let mut targets = targets();
    for target in &targets {
        let why = evidence::out_of_date(target, &evidence);
        assert!(why.is_empty(), "{}: {why:?}", target.name);
    }
    // One group's definition changed, and one cache measure's code.
    for name in ["small_side", "absent"] {
        let rebuilt = targets.iter_mut().find(|t| t.name == name).unwrap();
        rebuilt.binary = "rebuilt".to_owned();
    }
    let stale: Vec<&str> = targets
        .iter()
        .filter(|target| !evidence::out_of_date(target, &evidence).is_empty())
        .map(|target| target.name.as_str())
        .collect();
    assert_eq!(stale, ["small_side", "absent"]);
}

/// `target`'s results, kept from the build `from` for the build `to`.
fn kept(target: &str, from: &str, to: &str) -> Kept {
    Kept {
        target: target.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
        binary: String::new(),
        kept_unix_ms: 2000,
        commit: "ccccccc000".to_owned(),
        because: "moved, not changed".to_owned(),
    }
}

/// Every bench target, rebuilt by a change that moved its code: each
/// builds to `<name>-moved`, where the run log has `<name>-bin`.
fn moved() -> Vec<Target> {
    let mut targets = targets();
    for target in &mut targets {
        target.binary = format!("{}-moved", target.name);
    }
    targets
}

/// What wrote the results on disk for `target` that don't stand for it.
fn stale_keys(evidence: &Evidence, target: &Target) -> Vec<String> {
    use crate::subject::Results as _;
    evidence
        .stale_writers(target)
        .iter()
        .map(Run::measured)
        .collect()
}

/// The names of `targets` that are out of date.
fn stale(targets: &[Target], evidence: &Evidence) -> Vec<String> {
    targets
        .iter()
        .filter(|target| !evidence::out_of_date(target, evidence).is_empty())
        .map(|target| target.name.clone())
        .collect()
}

#[test]
fn kept_results_stand_for_the_binary_built_now() {
    let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    let targets = moved();
    assert_eq!(stale(&targets, &evidence).len(), targets.len());
    for target in &targets {
        assert_eq!(
            stale_keys(&evidence, target),
            [format!("binary:{}-bin", target.name)],
            "{}",
            target.name
        );
    }
    // A timing group, the memory profiles and a cache measure are kept.
    for name in ["one_to_one", "memory", "absent"] {
        let (from, to) = (format!("binary:{name}-bin"), format!("{name}-src"));
        evidence.log.kept.push(kept(name, &from, &to));
    }
    let still: Vec<String> = names()
        .into_iter()
        .map(|(name, _)| name.to_owned())
        .filter(|name| !["one_to_one", "memory", "absent"].contains(&name.as_str()))
        .collect();
    assert_eq!(stale(&targets, &evidence), still);
    let one_to_one = targets.iter().find(|t| t.name == "one_to_one").unwrap();
    assert!(stale_keys(&evidence, one_to_one).is_empty());
}

#[test]
fn kept_results_cite_the_commit_that_measured_them() {
    let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    let targets = moved();
    for target in &targets {
        let name = &target.name;
        // Kept twice over: across one change, and then another.
        evidence.log.kept.push(kept(
            name,
            &format!("binary:{name}-bin"),
            &format!("{name}-between"),
        ));
        evidence
            .log
            .kept
            .push(kept(name, &format!("{name}-between"), &target.sources));
    }
    let (results, warnings) =
        vouch(&STRICT, &targets, evidence).expect("every result was kept for its binary");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(results.cited.len(), targets.len());
    assert!(results.cited.values().all(|run| run.commit == "aaaaaaa000"));
}

#[test]
fn results_are_kept_for_one_target_and_one_direction() {
    let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    let targets = moved();
    // Another target's record, and one that runs from the new build to
    // the old: neither says the old results stand for the new build.
    evidence.log.kept.push(kept(
        "small_side",
        "binary:one_to_one-bin",
        "one_to_one-src",
    ));
    evidence
        .log
        .kept
        .push(kept("phases", "phases-src", "binary:phases-bin"));
    assert_eq!(stale(&targets, &evidence).len(), targets.len());
}

#[test]
fn results_stand_while_the_source_is_the_same_whatever_it_builds_to() {
    // Every run recorded its source, and every binary has since changed:
    // files moved, or something no target measures with was edited.
    let recorded = || evidence(log_from_source(), every_timing(1500), 1500);
    let mut targets = moved();
    assert!(stale(&targets, &recorded()).is_empty());
    let (results, _) =
        vouch(&STRICT, &targets, recorded()).expect("the source is what was measured");
    assert!(results.cited.values().all(|run| run.commit == "aaaaaaa000"));
    // One group's workload changed.
    targets[1].sources = "small_side-edited".to_owned();
    assert_eq!(stale(&targets, &recorded()), ["small_side"]);
}

/// A run of every target that recorded its source.
fn log_from_source() -> Vec<Run> {
    let mut runs = log(1000, "aaaaaaa000", false);
    for run in &mut runs {
        run.sources = format!("{}-src", run.target);
    }
    runs
}

/// The names of the targets that are out of date.
fn out_of_date(evidence: &Evidence) -> Vec<String> {
    targets()
        .iter()
        .filter(|target| !evidence::out_of_date(target, evidence).is_empty())
        .map(|target| target.name.clone())
        .collect()
}

/// Why the target called `name` is out of date.
fn why(evidence: &Evidence, name: &str) -> String {
    let target = targets().into_iter().find(|t| t.name == name).unwrap();
    evidence::out_of_date(&target, evidence).join("; ")
}

#[test]
fn a_target_that_ran_on_a_busy_machine_is_out_of_date() {
    let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    assert!(out_of_date(&evidence).is_empty());

    // Something else started reading memory while small_side ran: its
    // narrow sentinel, inside L2, never moved, and its wide one did.
    let [_, end] = evidence::sentinel_keys("small_side", Sentinel::Wide);
    evidence.timings.insert(end, around(1600.0, 1500));
    // both_small ran beside heavy work from start to finish.
    for key in evidence::sentinel_keys("both_small", Sentinel::Wide) {
        evidence.timings.insert(key, around(1700.0, 1500));
    }
    // A quarter is within what a quiet machine does; 2.5 times is a
    // compile.
    evidence.hashers.as_mut().unwrap().value.sentinels = steady(1000.0, 1250.0);
    evidence.absent.as_mut().unwrap().value.sentinels = steady(1000.0, 2500.0);
    assert_eq!(
        out_of_date(&evidence),
        ["small_side", "both_small", "absent"]
    );
    assert!(
        why(&evidence, "small_side").contains("reads past L2 moved 60%"),
        "{}",
        why(&evidence, "small_side")
    );
    assert!(
        why(&evidence, "both_small").contains("1.70 times the fastest on record"),
        "{}",
        why(&evidence, "both_small")
    );
    assert!(
        why(&evidence, "absent").contains("2.50 times"),
        "{}",
        why(&evidence, "absent")
    );
}

#[test]
fn a_whole_run_on_a_busy_machine_is_set_beside_the_sentinel_log() {
    // Every target's results agree with every other's, so nothing on
    // disk says the machine was busy.
    let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    assert!(out_of_date(&evidence).is_empty());

    // But an earlier run of one group read past L2 in 0.6 of the time.
    let earlier = |cpu: &str, os: &str| Checked {
        target: "one_to_one".to_owned(),
        started_unix_ms: 500,
        cpu: cpu.to_owned(),
        os: os.to_owned(),
        narrow: (1000.0, 1000.0),
        wide: (600.0, 600.0),
    };
    // On another machine, or this one under another system, it says
    // nothing about this one.
    evidence.checked = vec![
        earlier("another cpu", "an os"),
        earlier("a cpu", "another os"),
    ];
    assert!(out_of_date(&evidence).is_empty());

    // On this one, every timing group ran busy. The cache measures are
    // timed another way, and are set beside their own.
    evidence.checked.push(earlier("a cpu", "an os"));
    let groups: Vec<String> = cases::GROUPS.iter().map(|g| g.name.to_owned()).collect();
    assert_eq!(out_of_date(&evidence), groups);
    assert!(
        why(&evidence, "small_side").contains("1.67 times the fastest on record"),
        "{}",
        why(&evidence, "small_side")
    );
}

#[test]
fn a_target_s_sentinels_make_a_line_of_the_sentinel_log() {
    let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    let [start, _] = evidence::sentinel_keys("one_to_one", Sentinel::Wide);
    evidence.timings.insert(start, around(3000.0, 1500));
    assert_eq!(
        evidence.checked(evidence::Kind::Timing, "one_to_one", 1000),
        Some(Checked {
            target: "one_to_one".to_owned(),
            started_unix_ms: 1000,
            cpu: "a cpu".to_owned(),
            os: "an os".to_owned(),
            narrow: (1000.0, 1000.0),
            wide: (3000.0, 1000.0),
        })
    );
    // The memory profiles time no sentinel.
    assert_eq!(
        evidence.checked(evidence::Kind::Memory, "memory", 1000),
        None
    );
}

#[test]
fn a_target_that_ran_on_an_unsteady_machine_is_out_of_date() {
    let mut evidence = evidence(log(1000, "aaaaaaa000", false), every_timing(1500), 1500);
    let unsteady = |evidence: &Evidence| -> Vec<String> {
        targets()
            .iter()
            .filter(|target| !evidence::out_of_date(target, evidence).is_empty())
            .map(|target| target.name.clone())
            .collect()
    };
    assert!(unsteady(&evidence).is_empty());

    // The machine slowed while one_to_one ran: its sentinel ends slower.
    let [_, end] = evidence::sentinel_keys("one_to_one", Sentinel::Narrow);
    evidence.timings.insert(end, around(1900.0, 1500));
    // multiway_dense ran slow from start to finish: no drift of its own,
    // but six times the other groups' sentinels.
    for key in evidence::sentinel_keys("multiway_dense", Sentinel::Narrow) {
        evidence.timings.insert(key, around(6000.0, 1500));
    }
    // A cache measure 8% slower than the others is within what a steady
    // machine does; one 20% slower isn't.
    evidence.hashers.as_mut().unwrap().value.sentinels = steady(1080.0, 1000.0);
    evidence.absent.as_mut().unwrap().value.sentinels = steady(1200.0, 1000.0);
    assert_eq!(
        unsteady(&evidence),
        ["one_to_one", "multiway_dense", "absent"]
    );
    let why = |name: &str| {
        let target = targets().into_iter().find(|t| t.name == name).unwrap();
        evidence::out_of_date(&target, &evidence).join("; ")
    };
    assert!(
        why("one_to_one").contains("speed moved 90%"),
        "{}",
        why("one_to_one")
    );
    assert!(
        why("multiway_dense").contains("6.00 times"),
        "{}",
        why("multiway_dense")
    );
}

#[test]
fn refuses_timings_another_build_wrote() {
    let group = cases::group("one_to_one");
    let runs = vec![
        of("one_to_one", 1000, "old"),
        of("one_to_one", 2000, "now"),
        // Another group's run doesn't write this one's timings.
        of("small_side", 2200, "old"),
        of("one_to_one", 3000, "old"),
        of("one_to_one", 4000, "now"),
    ];
    let mut evidence = evidence(runs, every_timing(4500), 4500);
    let now = Target {
        binary: "now".to_owned(),
        ..targets().swap_remove(0)
    };
    assert_eq!(now.name, group.name);
    assert!(evidence.stale_timings(group, &now).is_empty());

    let ids: Vec<Case> = group.cases().collect();
    // Written by the first run of this build: still its results.
    evidence.timings.insert(key(&ids[0]), timing(2500));
    // Written by the other build, before any run, and not at all.
    evidence.timings.insert(key(&ids[1]), timing(3500));
    evidence.timings.insert(key(&ids[2]), timing(500));
    evidence.timings.remove(&key(&ids[3]));
    assert_eq!(
        evidence.stale_timings(group, &now),
        [ids[1].to_string(), ids[2].to_string(), ids[3].to_string()]
    );
}

#[test]
fn a_commit_that_leaves_a_binary_alone_leaves_its_results_standing() {
    // Everything ran at a. At b the absent probes' code changed, so that
    // target rebuilt and reran alone, and a noisy timing was rerun.
    let mut runs = log(1000, "aaaaaaa000", false);
    runs.push(Run {
        binary: "absent-rebuilt".to_owned(),
        ..run("absent", 2000, "bbbbbbb000", false, RUSTC)
    });
    runs.push(run("one_to_one", 2100, "bbbbbbb000", false, RUSTC));
    let mut timings = every_timing(1500);
    let noisy = cases::group("one_to_one").cases().next().unwrap();
    timings.insert(key(&noisy), timing(2500));
    let mut evidence = evidence(runs, timings, 1500);
    evidence.absent.as_mut().unwrap().written_unix_ms = 2500;
    let mut targets = targets();
    let absent = targets.iter_mut().find(|t| t.name == "absent").unwrap();
    absent.binary = "absent-rebuilt".to_owned();

    let (results, warnings) =
        vouch(&STRICT, &targets, evidence).expect("every binary is the one built now");
    assert!(warnings.is_empty(), "{warnings:?}");
    let files: HashMap<String, String> = render(&results).into_iter().collect();
    assert!(files["one_to_one.md"].contains("rust-algs aaaaaaa ·"));
    assert!(files["binary_memory.md"].contains("rust-algs aaaaaaa ·"));
    assert!(files["read_cost.md"].contains("rust-algs aaaaaaa ·"));
    assert!(files["control_miss.md"].contains("rust-algs bbbbbbb ·"));
    assert!(files["control_miss.md"].contains("rust-algs bbbbbbb."));
}

#[test]
fn a_table_of_two_groups_cites_the_later_of_their_commits() {
    let mut results = results(every_timing(0));
    results.cited.insert(
        "multiway_sparse".to_owned(),
        run("multiway_sparse", 86_400_000, "bbbbbbb000", false, RUSTC),
    );
    let files: HashMap<String, String> = render(&results).into_iter().collect();
    assert!(files["heap_vs_scan.md"].contains("rust-algs bbbbbbb · 1970-01-02"));
    assert!(files["selective.md"].contains("rust-algs aaaaaaa ·"));
}

#[test]
fn cites_the_oldest_clean_run_of_a_binary() {
    let mut runs = vec![run("one_to_one", 500, "9999999000", true, RUSTC)];
    runs.extend(log(1000, "bbbbbbb000", false));
    runs.push(run("one_to_one", 3000, "ccccccc000", false, RUSTC));
    let (results, _) = vouch(
        &STRICT,
        &targets(),
        evidence(runs, every_timing(3500), 3500),
    )
    .expect("a clean tree ran each binary");
    assert_eq!(results.cited["one_to_one"].commit, "bbbbbbb000");
}

#[test]
fn never_cites_a_commit_that_left_the_history() {
    // The first clean run was at a commit since amended away.
    let mut runs = log(1000, "9999999000", false);
    runs.extend(log(2000, "bbbbbbb000", false));
    let mut evidence = evidence(runs, every_timing(2500), 2500);
    evidence.log.unreachable.insert("9999999000".to_owned());
    let (results, _) =
        vouch(&STRICT, &targets(), evidence).expect("a later clean run is still here");
    assert!(results.cited.values().all(|run| run.commit == "bbbbbbb000"));
}

#[test]
fn a_binary_only_dirty_trees_ran_cites_no_commit() {
    let dirty = || evidence(log(1000, "aaaaaaa000", true), every_timing(1500), 1500);
    let Err(refused) = vouch(&STRICT, &targets(), dirty()) else {
        panic!("cited a commit for a dirty tree's binary");
    };
    assert!(
        refused.contains("no clean tree in this history"),
        "{refused}"
    );
    let allowed = Options {
        allow_dirty: true,
        ..STRICT
    };
    assert!(vouch(&allowed, &targets(), dirty()).is_ok());
}

#[test]
fn refuses_everything_when_nothing_has_run_and_says_how_to_run_it() {
    let Err(refused) = vouch(&STRICT, &targets(), Evidence::default()) else {
        panic!("vouched for nothing");
    };
    for (name, _) in names() {
        assert!(refused.contains(&format!("{name}: ")), "{refused}");
    }
    assert!(refused.contains("bench --release -- run"), "{refused}");
}

#[test]
fn refuses_a_smoke_runs_timings_until_they_are_rerun() {
    let small: Vec<Case> = cases::group("small_side").cases().take(3).collect();
    let small_side = |t, args: &[&str]| Run {
        args: args.iter().map(|arg| arg.to_string()).collect(),
        ..run("small_side", t, "aaaaaaa000", false, RUSTC)
    };
    let smoke = small_side(
        2000,
        &[
            "--warm-up-time",
            "0.1",
            "--measurement-time",
            "0.2",
            "small",
        ],
    );
    let rerun = small_side(3000, &["nested_loop"]);

    let full = log(1000, "aaaaaaa000", false);
    let mut timings = every_timing(1500);
    assert!(
        vouch(
            &STRICT,
            &targets(),
            evidence(full.clone(), timings.clone(), 1500)
        )
        .is_ok()
    );

    // The smoke run's few timings are refused, by name.
    for case in &small {
        timings.insert(key(case), timing(2500));
    }
    let runs: Vec<Run> = full.iter().cloned().chain([smoke.clone()]).collect();
    let Err(refused) = vouch(&STRICT, &targets(), evidence(runs, timings.clone(), 1500)) else {
        panic!("a smoke run's timings passed");
    };
    for case in &small {
        assert!(refused.contains(&case.to_string()), "{refused}");
    }

    // Rerunning them with the default settings clears it.
    for case in &small {
        timings.insert(key(case), timing(3500));
    }
    let runs: Vec<Run> = full.into_iter().chain([smoke, rerun]).collect();
    assert!(vouch(&STRICT, &targets(), evidence(runs, timings, 1500)).is_ok());
}

#[test]
fn refuses_a_confidence_level_other_than_95_percent() {
    let mut timings = every_timing(1500);
    let case = cases::cases().next().unwrap();
    timings.insert(
        key(&case),
        Timing {
            confidence_level: 0.99,
            ..timing(1500)
        },
    );
    let evidence = evidence(log(1000, "aaaaaaa000", false), timings, 1500);
    let Err(refused) = vouch(&STRICT, &targets(), evidence) else {
        panic!("a 99% interval passed");
    };
    assert!(refused.contains("other than 95%"), "{refused}");
    assert!(refused.contains(&case.to_string()), "{refused}");
}

#[test]
fn facts_mark_noisy_timings_and_unsettled_crossovers() {
    let mut timings = every_timing(0);
    // ±6%, past the 5% the † rule allows.
    timings.insert(
        lookup("one_to_one", Algorithm::Hash, 1 << 22),
        Timing {
            lower_ns: 940.0,
            upper_ns: 1060.0,
            ..timing(0)
        },
    );
    // The heap is clearly slower at 2 and 4 inputs, overlaps the scan at 8,
    // and is clearly faster from 16.
    let sparse = |timings: &mut HashMap<Key, Timing>, heap_at_8| {
        for (k, heap) in [
            (2, 1100.0),
            (4, 1100.0),
            (8, heap_at_8),
            (16, 900.0),
            (32, 900.0),
            (64, 900.0),
        ] {
            let at = |alg| lookup("multiway_sparse", alg, k);
            timings.insert(at(Algorithm::Heap), around(heap, 0));
            timings.insert(at(Algorithm::Scan), around(1005.0, 0));
        }
    };
    sparse(&mut timings, 1000.0);
    let page = facts(&results(timings.clone()));
    let row = |name: &str| {
        let row = page
            .lines()
            .find(|line| line.starts_with(&format!("| {name} |")));
        row.unwrap().to_owned()
    };
    assert!(row("hash_ns_4M").contains('†'), "{}", row("hash_ns_4M"));
    assert!(row("sort_merge_over_hash_4M").contains('†'));
    assert!(!row("hash_ns_1M").contains('†'));
    assert!(row("heap_scan_crossover_sparse_k").contains("| 8–16† |"));
    assert!(page.contains("heap_scan_crossover_sparse_k."), "{page}");

    // Clearly faster at 8 too settles it there.
    sparse(&mut timings, 900.0);
    let page = facts(&results(timings));
    let row = page
        .lines()
        .find(|line| line.starts_with("| heap_scan_crossover_sparse_k |"));
    assert!(row.unwrap().contains("| 8 |"));
}

#[test]
fn memory_tables_cite_the_memory_run_and_every_file_its_flags() {
    let mut results = results(every_timing(0));
    results.cited.insert(
        "memory".to_owned(),
        run("memory", 86_400_000, "bbbbbbb000", false, RUSTC),
    );
    results.allowed = vec!["--allow-stale"];
    let files: HashMap<String, String> = render(&results).into_iter().collect();

    for (file, page) in &files {
        for line in page.lines() {
            if line.starts_with("<!-- bench table") {
                assert!(line.contains(" · --allow-stale -->"), "{file}: {line}");
            }
            if line.starts_with('*') {
                assert!(
                    line.contains("Reported with --allow-stale"),
                    "{file}: {line}"
                );
            }
        }
    }
    for file in ["binary_memory.md", "selective_memory.md"] {
        let page = &files[file];
        assert!(page.contains("rust-algs bbbbbbb · 1970-01-02"), "{page}");
        assert!(page.contains("rust-algs bbbbbbb."), "{page}");
        assert!(!page.contains("aaaaaaa"), "{page}");
    }
    let (time, memory) = files["all.md"].split_once("all (memory)").unwrap();
    assert!(time.contains("aaaaaaa") && !time.contains("bbbbbbb"));
    assert!(memory.contains("bbbbbbb") && !memory.contains("aaaaaaa"));
    assert!(files["facts.md"].contains("memory profiles at rust-algs bbbbbbb"));
    assert!(!files["one_to_one.md"].contains("bbbbbbb"));
}

#[test]
fn cache_tables_cite_the_cache_run() {
    let results = with_cache(results(every_timing(0)), "ccccccc000");
    let files = render(&results);
    for name in [
        "hash_phases",
        "hash_footprint",
        "read_cost",
        "read_work",
        "read_chains",
        "control_miss",
        "hash_inlining",
        "hash_order",
        "hash_reads",
        "hash_predicted",
        "hash_hashers",
        "hash_instructions",
        "hash_per_instruction",
    ] {
        let (_, page) = files
            .iter()
            .find(|(file, _)| file == &format!("{name}.md"))
            .unwrap_or_else(|| panic!("no {name}"));
        assert!(page.contains("rust-algs ccccccc"), "{page}");
    }
}

#[test]
fn the_prediction_starts_at_the_flat_join_and_stays_in_its_band() {
    let results = with_cache(results(every_timing(0)), "ccccccc000");
    let files = render(&results);
    let (_, page) = files
        .iter()
        .find(|(file, _)| file == "hash_predicted.md")
        .expect("a prediction");
    assert!(page.contains("| 256K | 4.00 | 4.00 | 4.00 |"), "{page}");
    let last = page.lines().find(|l| l.starts_with("| 64M")).expect("64M");
    let cells: Vec<f64> = last
        .split('|')
        .filter_map(|cell| cell.trim().parse().ok())
        .collect();
    assert!(cells[0] < cells[1], "{last}");
}

#[test]
fn the_hasher_tables_have_a_column_per_hashing() {
    let results = with_cache(results(every_timing(0)), "ccccccc000");
    let files = render(&results);
    let page = |name: &str| {
        files
            .iter()
            .find(|(file, _)| file == &format!("{name}.md"))
            .map(|(_, page)| page.clone())
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let times = page("hash_hashers");
    assert!(
        times.contains(
            "| rows a side | foldhash | foldhash + 16 rounds | foldhash + 32 rounds | \
             SipHash | SipHash, 16 at a time | SipHash, 256 at a time | \
             foldhash, 256 at a time |"
        ),
        "{times}"
    );
    assert!(times.contains("| 16K | 10.0 |"), "{times}");
    assert!(times.contains("| 4M | 10.0 |"), "{times}");
    assert!(page("hash_instructions").contains("| 4M | 40.0 |"));
    assert!(page("hash_per_instruction").contains("| 4M | 0.250 |"));
}

#[test]
fn the_work_and_control_tables_say_what_their_rows_run() {
    let results = with_cache(results(every_timing(0)), "ccccccc000");
    let files = render(&results);
    let page = |name: &str| {
        files
            .iter()
            .find(|(file, _)| file == &format!("{name}.md"))
            .map(|(_, page)| page.clone())
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let work = page("read_work");
    assert!(work.contains("| 7 | 1.00 | 5.00 | 4.00 |"), "{work}");
    assert!(work.contains("| 48 | 2.00 | 9.50 | 7.50 |"), "{work}");
    let control = page("control_miss");
    assert!(
        control
            .contains("| rows a side | foldhash | foldhash + 16 rounds | foldhash + 32 rounds |"),
        "{control}"
    );
    assert!(
        control.contains("| 64M | 3.00 | 3.00 | 3.00 |"),
        "{control}"
    );
    assert!(
        control.contains("a row retires 23, 23 and 23 instructions"),
        "{control}"
    );
    let phases = page("hash_phases");
    assert!(
        phases.contains("The build loop retires 56 instructions for each row it builds"),
        "{phases}"
    );
}

#[test]
fn formats_three_significant_figures() {
    let formatted = [
        0.012_345, 1.2345, 12.345, 123.45, 12_345.6, 9.996, 99.96, 0.0,
    ]
    .map(sig3);
    assert_eq!(
        formatted,
        [
            "0.0123", "1.23", "12.3", "123", "12,346", "10.0", "100", "0"
        ]
    );

    let sizes = [168, 1023, 1500, 6_553_608, 52_428_808].map(bytes);
    assert_eq!(
        sizes,
        ["168 B", "1,023 B", "1.46 KiB", "6.25 MiB", "50.0 MiB"]
    );

    assert_eq!(
        [12.0, 1_234.0, 57e6, 1.5e9].map(duration),
        ["12.0 ns", "1.23 µs", "57.0 ms", "1.50 s"]
    );
    assert_eq!(
        [64, 1 << 10, 1 << 14, 1 << 22].map(param_label),
        ["64", "1K", "16K", "4M"]
    );
    assert_eq!(date(0), "1970-01-01");
    assert_eq!(
        moment(1_792_972_800_000 + 3_723_000),
        "2026-10-26 01:02:03 UTC"
    );
}

#[test]
fn a_timing_caption_says_whether_its_cases_rotated() {
    // The n-way workloads hold at least 2^20 rows, as criterion is told;
    // every other case here holds 1,000.
    let mut timings = every_timing(0);
    for case in cases::cases() {
        if matches!(
            case.group.workload,
            Workload::Dense | Workload::Sparse | Workload::HalfShared
        ) {
            timings.get_mut(&key(&case)).unwrap().input_rows = Some(harness::ROTATION_ROWS);
        }
    }
    let files: HashMap<String, String> = render(&results(timings)).into_iter().collect();
    let rotated = |file: &str| files[file].contains("cycled through instances");
    let same = |file: &str| files[file].contains("each timed call joined the same inputs");
    for file in ["one_to_one.md", "selective.md", "all.md", "facts.md"] {
        assert!(rotated(file) && !same(file), "{file}");
    }
    for file in ["heap_vs_scan.md", "heap_vs_plan.md", "leapfrog.md"] {
        assert!(same(file) && !rotated(file), "{file}");
    }
    let skips = |file: &str| files[file].contains(LEAPFROG_SELECTIVE);
    for file in ["leapfrog_selective.md", "all.md", "facts.md"] {
        assert!(skips(file), "{file}");
    }
    assert!(!skips("selective.md") && !skips("leapfrog.md"));
}

#[test]
fn no_table_has_more_than_five_data_columns() {
    // Every case has a timing, and nothing else, which renders every cell.
    for (file, markdown) in render(&results(every_timing(0))) {
        let lines: Vec<&str> = markdown.lines().collect();
        let headers = lines.windows(2).filter(|pair| pair[1].starts_with("|---"));
        let mut tables = 0;
        for pair in headers {
            let columns = pair[0].matches('|').count() - 1;
            assert!(columns - 1 <= 5, "{file}: {}", pair[0]);
            tables += 1;
        }
        assert!(tables > 0, "{file} has no table");
    }
}
