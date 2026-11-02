# rust-algs

Algorithms in Rust, written to be read and pulled from. Each module explains
the idea it implements, and its tests check the claims the explanation makes.

## Layout

```text
crates/algs/              the library
  src/clock/              tracking causality
    lamport.rs            Lamport clocks
    vector.rs             vector clocks
    dot.rs                dots and causal contexts
    hybrid.rs             hybrid logical clocks
  src/crdt/               replicated data types
    lamport_register.rs   last-writer-wins on Lamport stamps, and what it loses
    mv_register.rs        multi-value register
  src/coordination/       agreeing on an order
    lamport_mutex.rs      Lamport's mutual exclusion
  src/state_machine/      one sequential machine, run on many processes
    user_machine.rs       the machine: commands in, responses out
    bookkeeper.rs         Lamport's reliable state machine, through failures
    airline.rs            Lamport's airline reservation example
  src/broadcast/          delivering broadcasts in causal order
    cbcast.rs             causal broadcast
  src/join/               joining inputs that hold each key once
    nested_loop.rs        compare every pair of keys
    hash.rs               build a table on one input, probe it with the other
    merge.rs              walk two sorted inputs together; sort-merge
    multiway.rs           merge k sorted inputs with a binary heap; the binary plan it replaces
    leapfrog.rs           intersect k sorted inputs by seeking
  src/merkle.rs           Merkle trees over a directory, for finding what changed
  examples/               programs to run against the library
crates/diagrams/          figures drawn from the library (SVG, HTML and Mermaid)
crates/bench/             timings (criterion) and memory profiles of the library
figures/                  the rendered figures, generated and not committed
```

## Working with it

```bash
cargo test
```

```bash
cargo doc --open
```

The rendered module docs are the write-ups. Each program in `examples/` runs
with `cargo run --example <name>`:

```bash
cargo run --release --example hlc_system_clock
```

That runs the hybrid logical clock against this machine's real clock and
reports how often it needs its logical counter.

```bash
cargo run --release --example merkle -- <dir> <index-file>
```

That prints what changed under `<dir>` since the last run (`+` added, `-`
removed, `~` modified), then saves the current tree to `<index-file>` for next
time.

## Adding an algorithm

1. Write the implementation and its tests in `crates/algs/src/<name>.rs`, or
   under `crates/algs/src/<name>/` for a family, like `clock/`.
2. Export it from `crates/algs/src/lib.rs` with `pub mod <name>;`, and add it
   to the list in that file's docs.
3. For a program to run against it, add `crates/algs/examples/<name>.rs`.

## Figures

`crates/diagrams` draws figures from the reference implementations. Each
figure drives the real API, records every step, and asserts the claims the
picture makes, so every clock and register value in a picture comes from
running the code.

```bash
cargo run -p diagrams
```

That writes every figure into `figures/` (pass a directory to write somewhere
else). The files are generated, so they aren't committed: a page that shows
figures, such as the blog, runs this to make its own. A figure with more than
one step is a step-through, which writes a single `<stem>.html` (see below). A
figure with a single step has nothing to walk through, so it is finished, and
is written three ways:

- `<stem>.html` is what a page should inline: a `<figure>` holding the SVG and
  a `<figcaption>` with the figure's title and its numbered notes as HTML, so
  the page typesets them. It has no blank lines, so Markdown passes it through.
- `<stem>.svg` stands alone: the same drawing with the notes set underneath.
- `<stem>.mmd` is a Mermaid sequence diagram of the same scenario, for places
  that render Mermaid. A number line has no Mermaid form. Mermaid draws every
  message straight across, so a message that overtakes another doesn't cross
  there; its notes still say so. Nor does a lane end at a crash there: the
  crash is a note like any other event.

`index.json` lists the figures in the order a reader should meet them, with
their kinds, forms (`static` or `steps`) and titles.

### Step-throughs

A figure with more than one step is a step-through, which a page's player
shows one event at a time. There is one version of it, under one stem: it is
written for stepping, so it can say things at each step and start from values,
and at rest, or without a player, it is the finished figure. Whether a figure
is one is decided by its steps (`Diagram::is_walkable`), so a catalog entry
doesn't choose.

A space-time step-through has a step for each event, in recorded order, with
each note right after its later event. It can give each lane a start value
(`SpaceTime::start_value`) and say what an event does (`SpaceTime::narrate`,
beside an assert of what it claims); a panel shows what each lane holds at
each step, with what its event changed marked. A number
line steps through frames (`DotLine::frames`, then `frame` after each library
call). Either way the last step is the finished figure, and the figure is laid
out once, to fit every step (a space-time step-through keeps its text clear of
the arrowheads its messages show in flight, which a finished figure never
draws), and every version of every mark is tagged with the steps it shows at:

| Hook | Means |
|---|---|
| `.ds-fig[data-steps="N"][data-steps-v="1"]` | a step-through with steps `0` to `N`; `N` is the finished figure. `data-steps-v` is the version of these hooks |
| `[data-in="k"]`, `[data-out="j"]` | shown from step `k`, up to but not including step `j` (or to the end). A version with a `data-out` also carries `display="none"` in the SVG, so without a player the page shows the finished figure |
| `[data-cont]` | the version takes over from the one before it (a message moving on, a value changing), rather than appearing afresh |
| `.ds-flight` | a message line or head in flight: between its send and its arrival it reaches as far as the latest event's column |
| `--ds-len`, `--ds-grow`; `--ds-dx`, `--ds-dy` | on each version of a message line, its length and how much longer it is than the version before; on each version of a head, how far it was from here before (for its first, from just out of the sender, where its line grows from). A page can grow the line (`stroke-dasharray`/`stroke-dashoffset`) and slide the head into place |
| `.ds-play[hidden]` | the block a player fills and shows: an empty `.ds-play-bar` for its controls and one `p.ds-beat[data-event]` / `[data-note]` per step, whose `.ds-beat-body` says what happened (`.ds-lead` is the event's name) |
| `.ds-state`, `tr.ds-state-lane[data-lane]`, `.ds-cell[data-event]`, `.ds-changed` | in a space-time step-through's block, what each lane holds: a row per lane, a version of its value per event, and the parts that event changed |

A player shows step `s` by hiding every tagged element whose steps exclude
`s`. Tests filter the tagged figure that way at every step and compare it with
the figure drawn at that step, so a step can't show anything the code didn't
draw.

### Styling

Inlined, a figure follows the page: everything is drawn in `currentColor`, and
its stylesheet sits in `@layer ds-figure`, so any unlayered rule on the page
wins without a specificity fight. A page that puts its own rules in layers
should declare `@layer ds-figure;` before them, since a figure's sheet comes
later in the document and its layer would otherwise outrank theirs. Font sizes
are not styleable: they are layout inputs, set as attributes from the
constants the layout measures with. A figure stripped of its stylesheet, or
loaded through `<img>`, still reads.

Colours, fonts and strokes come from custom properties. Each colour falls back
through a tier (`--ds-muted` for secondary text, `--ds-rule` for lane lines) to
`--ds-ink`, then `currentColor`, so setting `--ds-ink` alone recolours a
figure and a new role picks up the right tier without a mapping of its own.

| Property | Styles |
|---|---|
| `--ds-ink`, `--ds-muted`, `--ds-rule` | the tiers: everything; event names, message labels, counters, legend; lane lines |
| `--ds-font-sans`, `--ds-font-mono` | names and prose; library values |
| `--ds-font-sym` | the `→` inside register entries and delayed broadcasts, for monospace fonts that lack it |
| `--ds-lane-line`, `--ds-lane-opacity`, `--ds-lane-width`, `--ds-lane-name` | timelines (opacity defaults to .5) and their names |
| `--ds-event-dot`, `--ds-event-name`, `--ds-value` | event markers, event names, library values under events and beside number-line rows |
| `--ds-crash` | the × where an actor crashes (falls back to `--ds-event-dot`) |
| `--ds-field` | the names in a value made of named fields, such as `Lamport` in a clock reading (falls back to `--ds-muted`) |
| `--ds-message-line`, `--ds-message-width`, `--ds-message-label` | arrows and their labels |
| `--ds-accent`, `--ds-on-accent`, `--ds-ring-width`, `--ds-ring-dash` | note rings and badges; the badge numeral (defaults to `Canvas`) |
| `--ds-caption` | notes set inside a standalone SVG |
| `--ds-dot`, `--ds-gap`, `--ds-wrong` | number-line dots, the never-seen dot, the over-claim `×` |
| `--ds-actor-1` … `--ds-actor-8` | an actor's colour, by palette slot: its lane's name and its name in every clock entry and dot |

Actors are colour-coded only when a page defines `--ds-actor-<n>`; without
them everything stays in ink. Slots follow the lanes top to bottom (a lane
declared with `SpaceTime::actor_lane`, such as replica `r1` writing as actor
`A`), then any other actor in the order it first appears. Inside each slot,
`--ds-actor` holds the colour, so a page can extend it, say to tint the lane
line: `.ds-lane[data-actor-n] { --ds-lane-line: color-mix(in oklab, var(--ds-actor) 70%, white) }`.
Only the actor's name is coloured, and the count beside it stays in ink, so
the number keeps full contrast. Choose colours that pass 4.5:1 as text in each
theme, and validate them as a set for colour-blind separation.

Each figure also reports one property: `--ds-w`, its design width in px
(unitless), on the root `<svg>`. A page can use it to stop the figure shrinking
past a legible size, as the blog does with `min-width`.

Every mark is grouped and keyed. Groups are named for what they are and their
parts `ds-<group>-<part>`; custom properties inherit, so a property set on a
keyed group restyles just that part. Set properties, not `fill` or `stroke`:
the parts set their own paint, and a stroke on a group outlines its text.

| Hook | On |
|---|---|
| `[data-figure="<stem>"][data-kind]` | the `<figure>` and the `<svg>`; `data-kind` is `space-time` or `dot-line` |
| `g.ds-lane[data-lane]` | a timeline (`.ds-lane-line`, `.ds-lane-head`, `.ds-lane-name`); the key is the lane name, lowercased |
| `g.ds-event[data-event][data-lane]` | an event (`.ds-event-dot`, `.ds-event-name`, `.ds-event-value`); a crash has `.ds-event-crash`, a ×, in place of the dot, and its lane's line ends there |
| `[data-actor][data-actor-n]` | a lane showing an actor, its events, and every part of a value naming that actor; `data-actor-n` is the palette slot |
| `g.ds-message[data-message][data-from][data-to]` | an arrow (`.ds-message-line`, `.ds-message-head`, `.ds-message-label`) |
| `data-notes="1 2"` | every event, message and ring a note is about; for a happened-before note, the whole path |
| `circle.ds-ring`, `g.ds-badge[data-note][data-relation]` | the ring on a note's two events, and its numbered badge (`.ds-badge-disc`, `.ds-badge-n`) |
| `li.ds-note[data-note][data-relation]` | a note in the figcaption; `data-relation` is `before` or `concurrent` when the note claims one |
| `.ds-v` | a library value inside prose: a `<tspan>` in a message label, a `<code>` in a figcaption note |
| `.ds-entry[data-actor]` | one entry of a vector clock, in values, labels and figcaption notes, as two parts: `.ds-actor` (`A`) and `.ds-count` (`:2`) |
| `.ds-dot-id[data-dot][data-actor]`, `.ds-cloud`, `.ds-sym`, `.ds-str`, `.ds-empty` | a dot's name (also split into `.ds-actor` and `.ds-count`), a dot past a causal context's gap (also `.ds-dot-id`), a `dot→value` entry's arrow and value (a register's entries, a delayed broadcast), an empty register, queue or set of pending commands |
| `.ds-stamp[data-stamp]`, `.ds-heard[data-actor]`, `.ds-holds`, `.ds-lamport` | a Lamport stamp `(1,A)`, every part keyed `data-stamp="1,A"` (its time also `.ds-time`, its process also `.ds-actor`); the latest stamp a mutex process has heard from a peer, or when a peer sent the latest message a bookkeeper has received from it (split into `.ds-actor` and `.ds-count`); whether it holds the resource; a Lamport clock's time |
| `.ds-command[data-command]`, `.ds-voter[data-actor]`, `.ds-clock`, `.ds-known`, `.ds-booked` | a stamped command `1:reserve 221 Smith`, every part keyed `data-command="1:reserve 221 Smith"` (its time also `.ds-time`); in a bookkeeper's pending commands, just the time, keyed the same way; a bookkeeper among those that voted for a command (also `.ds-actor`); a bookkeeper's clock; the latest time up to which it knows every accepted command; a passenger its copy of the airline has booked |
| `.ds-field` | a field's name in a value made of named fields (`label::fields`), padded so the values form a column |
| `g.ds-row[data-row]`, `[data-counter][data-dot][data-actor]` | a number line's rows, and its counters and dots |

The notes drawn inside a standalone `.svg` are plain text; the tagged values
are in the HTML figcaption.

Event and message keys are positional unless the catalog pins them
(`SpaceTime::pin_event`, `pin_message`): `b-3` is lane B's third event and `m2`
the second message recorded, so they move when a scenario gains an earlier
event. Target a pinned key from a post; the positional ones are for looking
around. Note numbers follow reading order and move the same way.

For example, a page might write:

```css
.ds-fig { --ds-ink: var(--ink); --ds-muted: var(--muted); --ds-accent: var(--accent); }
.ds-fig [data-lane="b"] { --ds-event-dot: var(--accent); }          /* B's events */
.ds-fig .ds-entry[data-actor="A"] { fill: var(--accent); color: var(--accent); } /* A's entry in every clock */
.ds-fig :is([data-note="2"], .ds-ring[data-notes~="2"]) { --ds-accent: crimson; } /* note 2's marks */
.ds-fig:has(li[data-note="1"]:hover) :is(.ds-event, .ds-message, .ds-ring, .ds-badge):not([data-notes~="1"], [data-note="1"]) { opacity: .25; }
```

Restyle paint (colour, opacity, stroke), not metrics: the layout measured the
text at its set size and weight, so a bolder or larger font can collide.

## Benchmarks

`crates/bench` times the joins with criterion, profiles their memory with a
counting allocator, and turns the results into Markdown tables for a post to
quote. Every case is checked before it's measured: its row count against a
count made without any join, and a checksum every algorithm on the same input
has to agree on, so a wrong join can't post a number. The memory harness also
asserts the memory figures the post quotes, as closed forms, except the binary
plans' peaks: those depend on where their intermediate results fall against
the powers of two a Vec grows through, so they're reported, not asserted.

A join called over and over on one input runs faster than on fresh input,
because the branch predictor learns its comparisons. So a timed case rotates
through instances of its input, the same workload at the same size from other
seeds, with the same row counts, holding at least 2^20 rows before one
repeats. That still leaves the rows in cache: a join that reads every row
reads 16 MiB a cycle, which fits in the M6's 20 MiB L2, though at the
smallest binary sizes the instances hold about 32 MiB, as generated and
sorted. Leapfrog reads only about 0.5% of `multiway_selective`'s rows, 5K to
12K a cycle, so its times there are on rows in cache: at 1M rows in each
large input, reading them from memory took about 12× as long. Every instance
is checked before it's timed; the memory harness profiles each case's first
instance alone.

The tables cite the commit they ran at, so commit first. Then, on AC power
with other apps closed:

```bash
rm -rf target/criterion target/bench && cargo bench -p bench
cargo run -p bench --release
```

That takes about 10 minutes on an M6, and writes the tables into
`target/bench/tables/` (pass `--out <dir>` to write somewhere else). The report
refuses results from a dirty tree, from runs at another commit or rustc, or
from a run with other criterion settings, such as a smoke test's shorter
measurement; `--allow-dirty` and `--allow-stale` override it, and every table
written under them says so. To rerun a noisy group at the same commit, which
the report accepts:

```bash
cargo bench -p bench --bench joins -- <group>
```

Like the figures, everything the benchmarks write is generated, so it lands
under `target/` and is never committed.

criterion is slow to build, so plain `cargo test` and `cargo build` skip this
crate. `cargo test --workspace` and `cargo clippy --workspace --all-targets`
cover it.
