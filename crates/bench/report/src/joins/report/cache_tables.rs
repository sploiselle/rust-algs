//! The tables of the hash join's cache measures, beside the model of its
//! reads.

use super::*;

/// The tables on why the hash join's cost per row climbs: its loops, what it
/// reads, what a read costs, how many reach DRAM, what those predict, and
/// where a read costs more than that prediction charges.
pub(super) fn cache_tables(results: &Results, cache: &Cache) -> Vec<(String, String)> {
    // Each file names the measures its numbers come from, and cites their run.
    let file = |name: &str,
                parts: &'static [&'static str],
                header: &[&str],
                rows: Vec<Vec<String>>,
                what: String| {
        let source = Source::Cache(parts);
        let run = results.cited(&source);
        let header: Vec<String> = header.iter().map(|&cell| cell.to_owned()).collect();
        let body = markdown(&header, header.len() - 1, rows);
        let caption = format!(
            "{what} {}, macOS {}, {}, rust-algs {}.",
            run.cpu,
            run.os,
            rustc(&run.rustc),
            sha(&run)
        );
        let page = page(name, results, &source, &body, &caption);
        (format!("{name}.md"), page)
    };
    let (l2, line) = (cache.l2_bytes, cache.line_bytes);
    let modelled = || cache.phases.iter().filter(|p| p.rows_a_side >= cache::FLAT);
    let label = |p: &cache::Phases| param_label(p.rows_a_side);
    let reads = |p: &cache::Phases| model::dram_reads(p.rows_a_side, l2, line);

    let phases = cache.phases.iter().map(|p| {
        let cells = [p.build_ns, p.probe_ns, p.grouped_probe_ns, p.whole_ns].map(sig3);
        [vec![label(p)], cells.to_vec()].concat()
    });
    let footprints = cache.phases.iter().map(|p| {
        let size = model::footprint(p.rows_a_side);
        let cells = [size.control, size.buckets, size.rows, l2].map(|b| bytes(b as u64));
        [vec![label(p)], cells.to_vec()].concat()
    });
    let costs = cache.reads.iter().map(|r| {
        let cells = [r.chased_ns, r.overlapped_ns, r.paged_ns].map(sig3);
        [vec![param_label(r.bytes)], cells.to_vec()].concat()
    });
    let worked = cache.worked_reads.iter().map(|w| {
        let cells = [w.l2_ns, w.dram_ns, w.dram_ns - w.l2_ns].map(sig3);
        [vec![format!("{:.0}", w.instructions)], cells.to_vec()].concat()
    });
    // A row per size the absent probes ran at, a column per hashing.
    let absent_hashings = cache::ABSENT_HASHINGS.map(cache::Hashing::label);
    let mut absent_sizes: Vec<usize> = cache.absent.iter().map(|a| a.rows_a_side).collect();
    absent_sizes.dedup();
    let absent: Vec<Vec<String>> = absent_sizes
        .iter()
        .map(|&n| {
            let cells = absent_hashings.iter().map(|&hashing| {
                cache
                    .absent
                    .iter()
                    .find(|a| a.rows_a_side == n && a.hashing == hashing)
                    .map_or(MISSING.to_owned(), |a| sig3(a.ns))
            });
            [param_label(n)].into_iter().chain(cells).collect()
        })
        .collect();
    // What each column's rows run, at the largest size: it doesn't move with size.
    let absent_instructions: Vec<String> = absent_hashings
        .iter()
        .map(|&hashing| {
            cache
                .absent
                .iter()
                .rfind(|a| a.hashing == hashing)
                .map_or(MISSING.to_owned(), |a| format!("{:.0}", a.instructions))
        })
        .collect();
    // The hash join both ways beside sort-merge, a row per size.
    let inlining: Vec<Vec<String>> = cache
        .inlining
        .iter()
        .map(|i| {
            let cells = [i.sort_merge_ns, i.inlined_ns, i.called_ns].map(sig3);
            [param_label(i.rows_a_side)]
                .into_iter()
                .chain(cells)
                .collect()
        })
        .collect();
    // The hash join on the same rows in two orders, a row per size.
    let order: Vec<Vec<String>> = cache
        .order
        .iter()
        .map(|o| {
            let cells = [
                o.random_ns,
                o.ordered_ns,
                o.random_instructions,
                o.ordered_instructions,
            ]
            .map(sig3);
            [param_label(o.rows_a_side)]
                .into_iter()
                .chain(cells)
                .collect()
        })
        .collect();
    // What the build loop runs each way, at the smallest size: there the
    // table's pages are the last call's, so the count is the loop's alone.
    let insert_instructions = cache.inlining.first().map_or(String::new(), |i| {
        format!(
            " The build loop retires {:.0} instructions for each row it builds with the insert \
             inlined, and {:.0} with it called.",
            i.inlined_build_instructions, i.called_build_instructions
        )
    });
    // The loops' instructions a row, at the largest size.
    let loop_instructions = cache.phases.last().map_or(String::new(), |p| {
        format!(
            " The build loop retires {:.0} instructions for each row it builds, and the probe \
             loop {:.0} for each row it probes with.",
            p.build_instructions, p.probe_instructions
        )
    });
    let dram = modelled().map(|p| {
        let r = reads(p);
        let cells = [r.control, r.buckets, r.rows, r.total()].map(|x| format!("{x:.2}"));
        [vec![label(p)], cells.to_vec()].concat()
    });
    let flat = modelled().next();
    let predicted = modelled().map(|p| {
        let (base, base_reads) = flat.map_or((0.0, 0.0), |f| (f.whole_ns, reads(f).total()));
        let added = reads(p).total() - base_reads;
        let across = model::footprint(p.rows_a_side).total();
        let (low, high) = cache
            .reads
            .iter()
            .find(|r| r.bytes >= across)
            .or(cache.reads.last())
            .map_or((0.0, 0.0), |r| (r.paged_ns, r.overlapped_ns));
        let cells = [base + added * low, base + added * high, p.whole_ns].map(sig3);
        [vec![label(p)], cells.to_vec()].concat()
    });

    // A row per size the hashers ran at, a column per hashing.
    let hashings = cache::HASHINGS.map(cache::Hashing::label);
    let mut sizes: Vec<usize> = cache.hashers.iter().map(|h| h.rows_a_side).collect();
    sizes.dedup();
    let by_hashing = |value: fn(&cache::HasherCost) -> f64| -> Vec<Vec<String>> {
        sizes
            .iter()
            .map(|&n| {
                let cells = hashings.iter().map(|&hashing| {
                    cache
                        .hashers
                        .iter()
                        .find(|h| h.rows_a_side == n && h.hashing == hashing)
                        .map_or(MISSING.to_owned(), |h| sig3(value(h)))
                });
                [param_label(n)].into_iter().chain(cells).collect()
            })
            .collect()
    };
    let hasher_header: Vec<&str> = ["rows a side"].into_iter().chain(hashings).collect();
    let hashed = "foldhash + 16 or 32 rounds mixes each hash that many more times, adding \
                  instructions but not reads. 16 or 256 at a time hashes that many keys before \
                  it inserts or looks up any of them.";

    let first = param_label(cache::FLAT);
    vec![
        file(
            "hash_phases",
            &["phases"],
            &[
                "rows a side",
                "build",
                "probe",
                "probe, matches first",
                "whole join",
            ],
            phases.collect(),
            format!(
                "Median of at least 30 calls, in ns per input row, counting both sides \
                 together, 2n. A clock reads between the hash join's two loops; the whole join \
                 also allocates and frees its table. Probe, matches first, is the probe loop of \
                 a second call, on the same rows with those that match moved to the \
                 front.{loop_instructions}"
            ),
        ),
        file(
            "hash_footprint",
            &["phases"],
            &[
                "rows a side",
                "control bytes",
                "buckets",
                "build rows",
                "L2 cache",
            ],
            footprints.collect(),
            "What the hash join reads at random: its table's control bytes and buckets, and the \
             build rows its buckets point to. The rows it scans are read in order. The L2 cache \
             is what this machine reports."
                .to_owned(),
        ),
        file(
            "read_cost",
            &["reads"],
            &[
                "working set",
                "one at a time",
                "overlapped",
                "overlapped, few pages",
            ],
            costs.collect(),
            format!(
                "Median of five runs, in ns per read of a {line}-byte cache line in random \
                 order. One at a time, each read's address comes from the last. Overlapped, \
                 addresses come from a list, so the core runs as many as it can at once. Few \
                 pages reads the same lines 1 MiB at a time, so hardly a read misses the TLB."
            ),
        ),
        file(
            "read_work",
            &["reads"],
            &["instructions a read", "in L2", "past L2", "added past L2"],
            worked.collect(),
            format!(
                "Median of five runs, in ns per read of a {line}-byte cache line in random \
                 order, overlapped. Beside each read the loop does 0, 4, 8, 12 or 16 rounds of \
                 arithmetic on its own counter, which read nothing and which no read waits on; \
                 instructions a read are what the kernel counts retired. In L2, the lines span \
                 4 MiB; past it, 1 GiB."
            ),
        ),
        file(
            "control_miss",
            &["absent"],
            &[
                "rows a side",
                absent_hashings[0],
                absent_hashings[1],
                absent_hashings[2],
            ],
            absent,
            format!(
                "Median of at least five calls, in ns per probe row, of the probe loop alone on \
                 keys the table doesn't hold, so a row reads control bytes and nothing else. \
                 foldhash + 16 or 32 rounds mixes each hash that many more times, adding \
                 instructions but not reads: a row retires {}, {} and {} instructions.",
                absent_instructions[0], absent_instructions[1], absent_instructions[2]
            ),
        ),
        file(
            "hash_inlining",
            &["inlining"],
            &[
                "rows a side",
                "sort-merge",
                "hash, insert inlined",
                "hash, insert called",
            ],
            inlining,
            format!(
                "Median of at least 30 calls, in ns per input row, counting both sides \
                 together, 2n. The hash join is timed twice: with the table's insert left to \
                 the compiler, which inlines it into the build loop, and with it behind a \
                 function the compiler may not inline, so each build row pays a call. Nothing \
                 else differs, and each sample calls both.{insert_instructions} Sort-merge is \
                 sampled on its own, on the same inputs."
            ),
        ),
        file(
            "hash_order",
            &["order"],
            &[
                "rows a side",
                "no order",
                "table order",
                "instructions, no order",
                "instructions, table order",
            ],
            order,
            "Median of at least 30 calls, in ns per input row, counting both sides together, \
             2n. The same join on the same rows twice: as the workload generates them, where \
             every lookup lands somewhere else in the table, and with both inputs sorted by \
             the bucket each key's lookup starts at, so the table is filled and read from one \
             end to the other. Instructions are what the kernel counts the join retiring for \
             each input row."
                .to_owned(),
        ),
        file(
            "hash_reads",
            &["phases"],
            &[
                "rows a side",
                "control bytes",
                "buckets",
                "build rows",
                "all reads",
            ],
            dram.collect(),
            format!(
                "Reads per input row that reach DRAM, modelled, not measured: uniform random \
                 reads of each structure, half the keys matching, in an LRU cache of {} and \
                 {line}-byte lines, by Che's approximation.",
                bytes(l2 as u64)
            ),
        ),
        file(
            "hash_predicted",
            &["phases", "reads"],
            &[
                "rows a side",
                "predicted, low",
                "predicted, high",
                "measured",
            ],
            predicted.collect(),
            format!(
                "In ns per input row. Predicted is the whole join measured at {first} rows a \
                 side, plus each modelled DRAM read past that point at the cost of an overlapped \
                 read across the join's own footprint: low without TLB misses, high with them. \
                 Measured is the whole join."
            ),
        ),
        file(
            "hash_hashers",
            &["hashers"],
            &hasher_header,
            by_hashing(|h| h.ns),
            format!(
                "Median of at least 30 calls, in ns per input row, counting both sides \
                 together, 2n. {hashed}"
            ),
        ),
        file(
            "hash_instructions",
            &["hashers"],
            &hasher_header,
            by_hashing(|h| h.instructions),
            format!(
                "Median instructions retired per input row over the same calls, as the \
                 kernel counts them for the process. {hashed}"
            ),
        ),
        file(
            "hash_per_instruction",
            &["hashers"],
            &hasher_header,
            by_hashing(|h| h.ns / h.instructions),
            format!(
                "Each call's median time over its median instructions, in ns per \
                 instruction. {hashed}"
            ),
        ),
    ]
}
