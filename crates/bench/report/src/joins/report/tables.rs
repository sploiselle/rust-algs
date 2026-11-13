//! The tables: which cases each sets beside which, and how one is written.

use super::*;

/// The criterion version the timings were taken with, for captions.
pub(super) const CRITERION: &str = "criterion 0.8.2";

/// A table, and the file it's written to.
pub struct Table {
    pub file: &'static str,
    pub kind: Kind,
    /// What the caption says about this table alone, if anything.
    pub note: Option<&'static str>,
}

/// How a table is laid out.
pub enum Kind {
    /// A row per parameter, headed `rows`, and a column per case.
    Grid {
        rows: &'static str,
        columns: &'static [Column],
        metric: Metric,
    },
    /// A row per case, headed `rows`: its peak, the peak over the input's
    /// bytes, and its allocations.
    Profile {
        rows: &'static str,
        cases: &'static [(Column, usize)],
    },
}

/// What a grid's cells show.
#[derive(Clone, Copy)]
pub enum Metric {
    /// The median time per input row.
    Time,
    /// The peak bytes.
    Peak,
}

/// A group's algorithm, under a label.
pub struct Column {
    pub group: &'static str,
    pub algorithm: Algorithm,
    pub label: &'static str,
}

pub(super) const fn column(
    group: &'static str,
    algorithm: Algorithm,
    label: &'static str,
) -> Column {
    Column {
        group,
        algorithm,
        label,
    }
}

/// The tables a post can paste. Each has at most 5 data columns: a post's
/// tables don't scroll sideways, so wider ones overflow a phone. That's also
/// why time and memory are separate tables.
pub const TABLES: &[Table] = {
    use Algorithm::*;
    &[
        Table {
            file: "one_to_one.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("one_to_one", NestedLoop, "nested loop"),
                    column("one_to_one", Hash, "hash"),
                    column("one_to_one", HashSorted, "hash (sorted input)"),
                    column("one_to_one", Merge, "merge (sorted input)"),
                    column("one_to_one", SortMerge, "sort-merge"),
                ],
            },
        },
        Table {
            file: "sorted_pair.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("one_to_one", Merge, "merge (sorted input)"),
                    column("one_to_one", Leapfrog, "leapfrog (sorted input)"),
                ],
            },
        },
        Table {
            file: "small_side.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows on the small side",
                metric: Metric::Time,
                columns: &[
                    column("small_side", NestedLoop, "nested loop"),
                    column("small_side", Hash, "hash"),
                    column("small_side", HashSipHash, "hash, SipHash"),
                ],
            },
        },
        Table {
            file: "both_small.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("both_small", NestedLoop, "nested loop"),
                    column("both_small", Hash, "hash"),
                    column("both_small", HashSipHash, "hash, SipHash"),
                ],
            },
        },
        Table {
            file: "hash_variants.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows a side",
                metric: Metric::Time,
                columns: &[
                    column("hash_variants", Hash, "foldhash"),
                    column("hash_variants", HashSipHash, "SipHash"),
                ],
            },
        },
        Table {
            file: "binary_memory.md",
            note: None,
            kind: Kind::Profile {
                rows: "at 1M rows a side",
                cases: &[
                    (column("one_to_one", Hash, "hash"), 1 << 20),
                    (column("one_to_one", Merge, "merge (sorted input)"), 1 << 20),
                    (column("one_to_one", SortMerge, "sort-merge"), 1 << 20),
                    (
                        column("one_to_one", Leapfrog, "leapfrog (sorted input)"),
                        1 << 20,
                    ),
                    (
                        column("one_to_one", NestedLoop, "nested loop, at 64K rows a side"),
                        1 << 16,
                    ),
                ],
            },
        },
        Table {
            file: "selective.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows in A and B",
                metric: Metric::Time,
                columns: SELECTIVE,
            },
        },
        Table {
            file: "selective_memory.md",
            note: Some(BINARY_PLAN_PEAKS),
            kind: Kind::Grid {
                rows: "rows in A and B",
                metric: Metric::Peak,
                columns: SELECTIVE,
            },
        },
        Table {
            file: "heap_vs_scan.md",
            note: None,
            kind: Kind::Grid {
                rows: "inputs",
                metric: Metric::Time,
                columns: &[
                    column("multiway_dense", Heap, "dense heap"),
                    column("multiway_dense", Scan, "dense scan"),
                    column("multiway_sparse", Heap, "sparse heap"),
                    column("multiway_sparse", Scan, "sparse scan"),
                ],
            },
        },
        Table {
            file: "heap_vs_plan.md",
            note: None,
            kind: Kind::Grid {
                rows: "inputs",
                metric: Metric::Time,
                columns: &[
                    column("multiway_dense", Heap, "dense heap"),
                    column("multiway_dense", CascadeUnplanned, "dense binary plan"),
                    column("multiway_half_shared", Heap, "half-shared heap"),
                    column(
                        "multiway_half_shared",
                        CascadeUnplanned,
                        "half-shared binary plan",
                    ),
                ],
            },
        },
        Table {
            file: "leapfrog.md",
            note: None,
            kind: Kind::Grid {
                rows: "inputs",
                metric: Metric::Time,
                columns: &[
                    column("multiway_dense", Heap, "dense heap"),
                    column("multiway_dense", Leapfrog, "dense leapfrog"),
                    column("multiway_sparse", Heap, "sparse heap"),
                    column("multiway_sparse", Leapfrog, "sparse leapfrog"),
                ],
            },
        },
        Table {
            file: "leapfrog_selective.md",
            note: None,
            kind: Kind::Grid {
                rows: "rows in A and B",
                metric: Metric::Time,
                columns: &[
                    column("multiway_selective", Heap, "heap"),
                    column("multiway_selective", Leapfrog, "leapfrog"),
                ],
            },
        },
    ]
};

/// What the binary plans' peaks include.
pub(super) const BINARY_PLAN_PEAKS: &str = "The binary plans' peaks are bytes requested: they include \
     the unfilled capacity of each intermediate Vec, which grows by doubling, and during a \
     realloc that moves one, the block it moves from.";

/// The heap against the two binary plans, in time and in memory.
pub(super) const SELECTIVE: &[Column] = &[
    column("multiway_selective", Algorithm::Heap, "heap"),
    column(
        "multiway_selective",
        Algorithm::CascadePlanned,
        "binary plan (smallest first)",
    ),
    column(
        "multiway_selective",
        Algorithm::CascadeUnplanned,
        "binary plan (as given)",
    ),
];

/// Writes every table into `out`, returning the files it wrote.
///
/// # Errors
///
/// If `out` can't be written.
pub fn write(results: &Results, out: &Path) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(out)?;
    render(results)
        .into_iter()
        .map(|(file, markdown)| {
            let path = out.join(file);
            fs::write(&path, markdown)?;
            Ok(path)
        })
        .collect()
}

/// Every file's name and Markdown: the [`TABLES`], then every case in long
/// form (`all.md`), then the [`FACTS`] (`facts.md`).
pub fn render(results: &Results) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = TABLES
        .iter()
        .map(|table| (table.file.to_owned(), table.render(results)))
        .collect();
    files.extend(
        results
            .cache
            .as_ref()
            .map_or(Vec::new(), |cache| cache_tables(results, cache)),
    );
    files.push(("all.md".to_owned(), all(results)));
    files.push(("facts.md".to_owned(), facts(results)));
    files
}

impl Table {
    pub(super) fn name(&self) -> &str {
        self.file.trim_end_matches(".md")
    }

    pub(super) fn render(&self, results: &Results) -> String {
        let note = self.note.map_or(String::new(), |note| format!(" {note}"));
        match &self.kind {
            Kind::Grid {
                rows,
                columns,
                metric: Metric::Time,
            } => {
                let group = cases::group(columns[0].group);
                let mut noisy = Vec::new();
                let body = grid(rows, columns, |c, param| {
                    let timing = results.timing(c.group, c.algorithm, param);
                    let per_row = results.per_row(c.group, c.algorithm, param);
                    match (timing, per_row) {
                        (Some(timing), Some(ns)) if timing.noisy() => {
                            noisy.push(format!("{} at {}", c.label, param_label(param)));
                            format!("{}†", sig3(ns))
                        }
                        (_, Some(ns)) => sig3(ns),
                        _ => MISSING.to_owned(),
                    }
                });
                let what = format!(
                    "Median of criterion samples, in ns per input row, counting {}.{note}",
                    group.workload.input_rows()
                );
                let cells = columns.iter().flat_map(|c| {
                    let params = cases::group(c.group).params;
                    params.iter().map(|&param| (c.group, c.algorithm, param))
                });
                let reads = reads(results, cells);
                let source = Source::of(columns);
                let caption = time_caption(results, &source, &what, &reads, NOISY, &noisy);
                page(self.name(), results, &source, &body, &caption)
            }
            Kind::Grid {
                rows,
                columns,
                metric: Metric::Peak,
            } => {
                let body = grid(rows, columns, |c, param| {
                    let profile = results.profile(c.group, c.algorithm, param);
                    profile.map_or(MISSING.to_owned(), |p| bytes(p.usage.peak_bytes as u64))
                });
                let caption = memory_caption(results, &format!("{PEAK}{note}"));
                page(self.name(), results, &Source::Memory, &body, &caption)
            }
            Kind::Profile { rows, cases } => {
                let header = [rows, "peak", "× input", "allocations"].map(str::to_owned);
                let lines = cases.iter().map(|(c, param)| {
                    let cells = match results.profile(c.group, c.algorithm, *param) {
                        Some(p) => profile_cells(p),
                        None => vec![MISSING.to_owned(); 3],
                    };
                    [vec![c.label.to_owned()], cells].concat()
                });
                let body = markdown(&header, 3, lines);
                let caption = memory_caption(results, &format!("{PEAK} {PER_INPUT}{note}"));
                page(self.name(), results, &Source::Memory, &body, &caption)
            }
        }
    }
}

/// What a cell shows for a case that should have a result and doesn't.
pub(super) const MISSING: &str = "missing";

/// A row per parameter of the first column's group, each cell from `cell`, or
/// `—` where the case doesn't run.
pub(super) fn grid(
    rows: &str,
    columns: &[Column],
    mut cell: impl FnMut(&Column, usize) -> String,
) -> String {
    let header: Vec<String> = [rows]
        .into_iter()
        .chain(columns.iter().map(|c| c.label))
        .map(str::to_owned)
        .collect();
    let params = cases::group(columns[0].group).params;
    let lines = params.iter().map(|&param| {
        let cells = columns.iter().map(|c| {
            if c.algorithm.runs_at(param) {
                cell(c, param)
            } else {
                "—".to_owned()
            }
        });
        [param_label(param)].into_iter().chain(cells).collect()
    });
    markdown(&header, columns.len(), lines.collect::<Vec<Vec<String>>>())
}

/// A profile's peak, peak over its input's bytes, and allocations.
pub(super) fn profile_cells(p: &MemoryCase) -> Vec<String> {
    vec![
        bytes(p.usage.peak_bytes as u64),
        sig3(p.usage.peak_bytes as f64 / p.input_bytes as f64),
        count(p.usage.allocations as u64),
    ]
}

/// Every case in long form: one table of timings and one of memory.
pub(super) fn all(results: &Results) -> String {
    let header = ["case", "median", "±CI", "ns per input row"].map(str::to_owned);
    let times = cases::cases().map(|case| {
        let (group, alg, param) = (case.group.name, case.algorithm, case.param);
        let cells = match results.timing(group, alg, param) {
            Some(t) => vec![
                duration(t.median_ns),
                format!(
                    "±{}%",
                    sig3((t.upper_ns - t.lower_ns) / 2.0 / t.median_ns * 100.0)
                ),
                results
                    .per_row(group, alg, param)
                    .map_or(MISSING.to_owned(), sig3),
            ],
            None => vec![MISSING.to_owned(); 3],
        };
        [vec![case.to_string()], cells].concat()
    });
    let cells = cases::cases().map(|case| (case.group.name, case.algorithm, case.param));
    let source = Source::every_group();
    let caption = time_caption(results, &source, ALL, &reads(results, cells), NOISY, &[]);
    let time = page(
        "all",
        results,
        &source,
        &markdown(&header, 3, times),
        &caption,
    );

    let header = ["case", "peak", "× input", "allocations", "allocated"].map(str::to_owned);
    let profiled = cases::cases().map(|case| {
        let cells = match results.memory.get(&key(&case)) {
            Some(p) => [
                profile_cells(p),
                vec![bytes(p.usage.allocated_bytes as u64)],
            ]
            .concat(),
            None => vec![MISSING.to_owned(); 4],
        };
        [vec![case.to_string()], cells].concat()
    });
    let caption = memory_caption(results, &format!("{PEAK} {PER_INPUT}"));
    let memory = page(
        "all (memory)",
        results,
        &Source::Memory,
        &markdown(&header, 4, profiled),
        &caption,
    );
    format!("{time}\n{memory}")
}

/// A Markdown table: `header`, then `rows`. The first column labels the row
/// and the `numbers` after it are right-aligned; any after those are text.
pub(super) fn markdown(
    header: &[String],
    numbers: usize,
    rows: impl IntoIterator<Item = Vec<String>>,
) -> String {
    let line = |cells: &[String]| format!("| {} |\n", cells.join(" | "));
    let mut table = line(header);
    let text = header.len() - 1 - numbers;
    table += &format!("|---|{}{}\n", "--:|".repeat(numbers), "---|".repeat(text));
    for row in rows {
        table += &line(&row);
    }
    table
}

/// A file's worth: the provenance comment, the table, and its caption. Both
/// cite the run `source`'s numbers came from, and any flag that let the
/// report use results it would otherwise refuse.
pub(super) fn page(
    name: &str,
    results: &Results,
    source: &Source,
    table: &str,
    caption: &str,
) -> String {
    let run = results.cited(source);
    let mut flags = String::new();
    let mut caption = caption.to_owned();
    if !results.allowed.is_empty() {
        let allowed = results.allowed.join(" and ");
        flags = format!(" · {allowed}");
        caption += &format!(
            " Reported with {allowed}, so it may include results that aren't from this \
             commit's clean run."
        );
    }
    format!(
        "<!-- bench table: {name} · rust-algs {} · {} · {COMMAND}{flags} -->\n\n\
         {table}\n*{caption}*\n",
        sha(&run),
        date(run.started_unix_ms)
    )
}

/// How the numbers were taken, for a table's provenance comment.
pub(super) const COMMAND: &str = "cargo run -p bench -- run";

/// What the long-form timing table's numbers are.
pub(super) const ALL: &str = "Medians of criterion samples, the half-width of each one's 95% confidence \
                   interval, and the median per input row, counting every input's rows.";

/// What a memory table's peaks are.
pub(super) const PEAK: &str = "Peak bytes live at once during one call, as requested from the allocator \
                    (not RSS), with realloc counted as a move.";

/// What a profile's ratio is.
pub(super) const PER_INPUT: &str = "× input is the peak over the inputs' bytes.";

/// What † marks in a timing table.
pub(super) const NOISY: &str = "a 95% confidence interval wider than ±5% of the median";

/// What leapfrog's times on the selective workload leave out: see
/// [`harness::ROTATION_ROWS`].
pub(super) const LEAPFROG_SELECTIVE: &str = "On the selective workload, leapfrog reads about 0.5% of \
     the input rows, so a timed cycle read only 5K to 12K rows, which stayed in cache.";

/// How the timed calls of a table that cites `cells`, as (group, algorithm,
/// parameter), read their inputs: whether any rotated through instances, and
/// how little leapfrog reads of the selective workload's, if it cites that.
pub(super) fn reads<'a>(
    results: &Results,
    cells: impl IntoIterator<Item = (&'a str, Algorithm, usize)>,
) -> String {
    let (mut rotated, mut skips) = (false, false);
    for (group, algorithm, param) in cells {
        rotated |= results.rotated(group, algorithm, param);
        skips |= algorithm == Algorithm::Leapfrog
            && matches!(cases::group(group).workload, Workload::Selective);
    }
    let rows = param_label(harness::ROTATION_ROWS as usize);
    let mut reads = if rotated {
        format!(
            "Below {rows} input rows, the timed calls cycled through instances of the inputs, \
             generated from other seeds, that held at least {rows} rows before one repeated, \
             too many for the branch predictor to learn much of. A join that reads every row \
             reads {} MiB a cycle, so a cache that holds that much served its rows.",
            harness::ROTATION_ROWS * size_of::<Row>() as u64 / (1 << 20)
        )
    } else {
        format!(
            "Every case here held at least {rows} input rows, so each timed call joined the \
             same inputs."
        )
    };
    if skips {
        reads = format!("{reads} {LEAPFROG_SELECTIVE}");
    }
    reads
}

/// The line under a timing table: `what` its numbers are, where they came
/// from, how the timed calls `reads` their inputs, and which cells are
/// `marked` †, which means `dagger`.
pub(super) fn time_caption(
    results: &Results,
    source: &Source,
    what: &str,
    reads: &str,
    dagger: &str,
    marked: &[String],
) -> String {
    let run = results.cited(source);
    let mut caption = format!(
        "{what} {}, macOS {}, {}, {CRITERION}, rust-algs {}. {reads}",
        run.cpu,
        run.os,
        rustc(&run.rustc),
        sha(&run)
    );
    if !marked.is_empty() {
        caption += &format!(" † marks {dagger}: {}.", marked.join(", "));
    }
    caption
}

/// The line under a memory table: `what` its numbers are, and where they came
/// from.
pub(super) fn memory_caption(results: &Results, what: &str) -> String {
    let run = results.cited(&Source::Memory);
    format!("{what} {}, rust-algs {}.", rustc(&run.rustc), sha(&run))
}
