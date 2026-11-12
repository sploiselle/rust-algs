# Benchmarks

Everything that's benchmarked is here. So far that's the joins: how long each
takes, what memory it takes, and why the hash join's cost per row climbs. The
results become Markdown tables for a post to quote.

## Layout

```text
joins/                    package bench-joins: what's timed
  src/                    what every measure shares
    lib.rs                rows, and the sinks a join emits into
    workload.rs           the inputs
    harness.rs            instances of an input, sentinels, sampling
    timed.rs              each join as a type a group can name
    baseline.rs           what a join is set beside
    cache/                the hash join's cache measures, a file each
  benches/
    timing/               a timing binary for each group (criterion)
      mod.rs              the harness every group's binary includes
      <group>.rs          a bench target: mod.rs, then its group
      groups/<group>.rs   the group: its workload, sizes and joins
    cache/<measure>.rs    a bench target for each cache measure
joins-plan/               package bench-joins-plan: every case, and its memory
  src/
    cases.rs              the plan: every group, its sizes and its joins
    profile.rs            a memory profile
    alloc.rs              the allocator that counts
  benches/memory.rs       a bench target: every case's memory profile
report/                   package bench: runs them, and writes the tables
  src/
    main.rs               the command line
    targets.rs            the bench targets as built, and what each is built from
    record.rs             the logs: what ran, and what was kept
    vouch.rs              which results on disk the logs vouch for
    runner.rs             runs what's out of date; keeps what an edit didn't touch
    subject.rs            what the runner asks of a subject
    joins/                the joins, as a subject
      evidence.rs         their results on disk, and the machine's steadiness
      model.rs            a model of the hash join's reads
      report/             the tables and the facts
```

Each package links the one above it and none below. A bench target's machine
code depends on every crate it links, so a table that shared a crate with a
join could move the join's loops when it changed. Split this way, it can't:

- nothing that's timed links the plan or the report, so neither can move a
  join's machine code;
- the memory profile links the plan and not the report, so a change to a
  table leaves it as it was.

To benchmark something else, give it packages beside these, a module beside
`report/src/joins/` that implements `Subject` for it, and a line in
`subjects()` in `report/src/lib.rs`. The runner refuses a bench target no
subject claims.

## What's measured

Each benchmark group in `joins/benches/timing/groups/` has a timing binary of
its own, run by criterion. `joins-plan/benches/memory.rs` profiles each case's
memory with a counting allocator. `joins/benches/cache/` has a binary for each
cache measure.

Every case is checked before it's measured: its row count against a count made
without any join, and a checksum every join on the same input has to agree on,
so a wrong join can't post a number. The memory harness also asserts the
memory figures the post quotes, as closed forms, except the binary plans'
peaks: those depend on where their intermediate results fall against the
powers of two a Vec grows through, so they're reported, not asserted.

A join called over and over on one input runs faster than on fresh input,
because the branch predictor learns its comparisons. So a timed case rotates
through instances of its input, the same workload at the same size from other
seeds, with the same row counts, holding at least 2^20 rows before one
repeats. That can still leave the rows in cache: a join that reads every row
reads 16 MiB a cycle, though at the smallest binary sizes the instances hold
about 32 MiB, as generated and sorted. Leapfrog reads only about 0.5% of
`multiway_selective`'s rows, 5K to 12K a cycle, so its times there are on rows
in cache. Every instance is checked before it's timed; the memory harness
profiles each case's first instance alone.

The cache measures are seven targets: `phases` times the hash join's two loops
apart from 64K rows a side to 64M; `reads` measures what a read costs across
working sets to 2 GiB, one at a time, overlapped, and with unrelated work
beside it; `hashers` times the join with hashers that run more instructions,
and with keys hashed in batches before their lookups, and counts the
instructions each retires; `absent` probes for keys the table doesn't hold,
which reads control bytes alone; `inlining` times the hash join with the
table's insert inlined into its build loop and with it called, beside
sort-merge; `order` times it on the same rows in no order, in its table's
order, a line of control bytes at a time from the first to the last, and with
those lines shuffled, which keeps what the table's order gives a cache and not
what it gives a prefetcher; `chains` follows from 1 to 64
pointer chases at once across 2 GiB, which sets how many reads can be in flight.
The tables set those beside a model of how many of the join's reads
reach DRAM. The largest sizes need about 7 GiB free.

## Running them

On AC power, with other apps closed, and away from the machine: start it and
leave, or from a terminal rather than an app that redraws as it runs.

```bash
cargo run -p bench --release -- run
```

```bash
cargo run -p bench --release
```

The first runs every bench target that is out of date; the second writes the
tables into `target/bench/tables/` (pass `--out <dir>` to write somewhere
else). The tables cite the commit their numbers were measured at, so commit
before running: results from a dirty tree cite no commit, and the tables
refuse them.

## A quiet machine

macOS can't pin a thread to a core or keep other work off the cores that
share its L2, so the only isolation here is a machine doing nothing else. Run
it while you're away: with an editor and a chat app open, the hash join past
L2 ran a tenth to a quarter slower, six runs in a row that agreed. Two
runs on a machine left alone agreed to 0.3% at the median case and 1.4% at
the ninetieth percentile.

The runner checks for the grosser changes. Every target times two sentinels
before its first measure and after its last: random reads across 8 MiB, which
L2 holds, and across 32 MiB, which it doesn't. The narrow one moves with the
clock: a core pushed to a slower mode takes two to six times as long. The
wide one moves with heavy work on the other cores: while something compiled,
it and the hash join took about 2.5 times as long and the narrow one didn't
move. Neither sees the editor. The narrow one fits in L2 whatever else runs,
and the wide one reads anywhere from 1.27 to 1.57 ns a read from one process
to the next on a quiet machine, more than an open editor adds.

A target whose narrow sentinel moved more than 3% while it ran, or whose wide
one moved more than half, or which read that much slower than the fastest
this machine has on record, is rerun, up to three times, and the tables
refuse its results until a run passes. The record is
`target/bench/sentinels.jsonl`, a line for every run. A filtered rerun times
the sentinels like any other. If the machine itself changes speed for good,
the record keeps only lines with the same CPU and macOS version; failing
that, delete the file.

## One codegen unit

The workspace builds benchmarks with `codegen-units = 1`. With rustc's
default of 16, how a crate is split decides what gets inlined, and an edit to
one function moves the split under another. Adding a struct beside the cache
measures took the hash join's build loop, which nothing had edited, from 56
instructions a row to 71: the compiler stopped inlining the table's insert.
With one unit, both commits build the 56.

One unit doesn't settle it for every binary. The compiler inlines a function
called from one place far more readily than one called from several, so the
same join gets the insert inlined in `phases`, which holds one copy of it, and
called in `one_to_one`, which holds four. `inlining` measures what that is
worth: it times the join both ways in one binary and counts each build loop's
instructions, and it panics rather than write a result if the two counts say
the compiler inlined both or neither.

## What reruns

Only what a change touched. A result stands for as long as its target is
built from the source it was measured from, and the runner asks the build
what that is. rustc writes down every file it reads to compile a crate, and
cargo knows which crates a target links. So a target's source is, as one
hash:

- every file rustc read to compile the target and the workspace's crates it
  links, by contents and not by path;
- the other crates it links, by name, version, source and features;
- the compiler, the workspace's profiles, and what else changes how the same
  source compiles: `RUSTFLAGS` and the profile overrides in the environment,
  and cargo's configuration files.

The runner logs that hash, and the binary's, with every run. A result stands
if its target is built from the same source now, or to the same binary. So:

- a change to a table, the runner or the logs reruns nothing: no bench
  target links them;
- a change to the plan reruns the memory profiles, which take half a minute,
  and no timing;
- moving or renaming a file reruns nothing by itself: a file is told by its
  contents. If the move needs an edit to a measured file, such as the line
  that names the file moved, keep the results, as below;
- a new group, or a change to one group's file, reruns that group and the
  memory profiles;
- a change to one join reruns the targets that time it. The joins are
  generic, so a binary that doesn't name a join doesn't hold it, and builds
  to the same binary as before;
- a change to what every target shares (the harness, the workloads, the
  sinks) reruns them all;
- a change to the cache measures' code reruns the cache measures, and any
  timing group whose binary it changes: they share a library.

A result also has to have been written by a run the runner logged. It logs
when each run starts and ends, and a result written outside one, by a bare
`cargo bench` say, isn't used.

### Keeping results

The hash is of the source, not of what it means, so an edit that leaves what's
measured alone still changes it: an import, a name, a comment. Then keep the
results, and say why:

```bash
cargo run -p bench --release -- keep --only one_to_one --because "renamed a module"
```

That runs nothing. It records, in `target/bench/kept.jsonl`, that the named
targets' results stand for the source as it is now, at this commit, for the
reason given. `--all` keeps every target's. The tables go on citing the
commit the results were measured at, until they're measured again.

Nothing checks the claim, so `keep` says what is being claimed: for each
target, the commit that measured its results and the files it's built from
that have changed since. `--list` says that and keeps nothing. It won't keep
results measured on a dirty tree, which no commit holds.

The same source can still build to other machine code when what's around it
changes: see One codegen unit, above. Keeping results says the measurement is
the same, not the binary. The `inlining` measure says how far a build can
move the hash join.

To see what would run without running it:

```bash
cargo run -p bench --release -- run --list
```

To rerun a target that is up to date, or one noisy case of it (anything after
`--` goes to the target, so to criterion's filter):

```bash
cargo run -p bench --release -- run --only one_to_one -- hash/1048576
```

The tables say which results they refuse and why: a target whose source has
changed since it ran, a result no logged run wrote, a run with other
criterion settings, such as a smoke test's shorter measurement, or results
only a dirty tree has taken. `--allow-stale` and `--allow-dirty` override it,
and every table written under them says so.

The bench targets hold no bookkeeping: the runner logs the commit and the
hashes, so the logs can change without changing what's measured.

To add a group: write `joins/benches/timing/groups/<name>.rs`, copy another
group's `joins/benches/timing/<name>.rs` and its `[[bench]]` entry, and add
the file to `GROUPS` in `joins-plan/src/cases.rs`.

Like the figures, everything the benchmarks write is generated, so it lands
under `target/` and is never committed.

criterion is slow to build, so plain `cargo test` and `cargo build` skip these
packages. `cargo test --workspace` and
`cargo clippy --workspace --all-targets` cover them.
