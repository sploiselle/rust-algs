# dist-sys

A learning repo: reference implementations of distributed-systems building
blocks in Rust, written to be read and pulled from. Each module explains the
idea it implements, and its tests check the claims the explanation makes.

## Layout

```text
crates/dist-sys/          the library
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
  examples/               programs to run against the library
crates/diagrams/          figures drawn from the library (SVG, HTML and Mermaid)
figures/                  the rendered figures, committed
```

## Working with it

```bash
cargo test
```

```bash
cargo doc --open
```

The rendered module docs are the write-ups.

```bash
cargo run --release -p dist-sys --example hlc_system_clock
```

That runs the hybrid logical clock against this machine's real clock and
reports how often it needs its logical counter.

## Figures

The files in `figures/` are drawn by `crates/diagrams` from the reference
implementations. Each figure drives the real API, records every step, and
asserts the claims the picture makes, so every clock and register value in a
picture comes from running the code.

```bash
cargo run -p diagrams
```

That writes every figure into `figures/` (pass a directory to write somewhere
else). `cargo test` fails if a committed figure is stale or no figure produces
it any more. For each figure:

- `<stem>.html` is what a page should inline: a `<figure>` holding the SVG and
  a `<figcaption>` with the figure's title and its numbered notes as HTML, so
  the page typesets them. It has no blank lines, so Markdown passes it through.
- `<stem>.svg` stands alone: the same drawing with the notes set underneath.
- `<stem>.mmd` is a Mermaid sequence diagram of the same scenario, for places
  that render Mermaid. The causal-context figure is a number line, which has
  no Mermaid form. Mermaid draws every message straight across, so the
  overtaking message in the mutex figure doesn't cross there; its notes
  still say so.

`index.json` lists the figures in the order a reader should meet them, with
their kinds, forms (`static` or `steps`) and titles.

### Step-throughs

A figure can also be written as a step-through, which a page's player shows
one event at a time. It is opt-in: a catalog entry with `Form::Steps` writes a
single `<stem>.html` and nothing else, and it is written for stepping, so it
can say things at each step, start from values, or tell a different story
from any finished figure. Where it tells the same story, the two entries share
a scenario function, and with it the asserts (`vector_trace` in the catalog).

A space-time step-through's steps are the lanes, then each event in recorded
order with each note right after its later event, then the finished figure.
It can give each lane a start value (`SpaceTime::start_value`) and say what an
event does (`SpaceTime::narrate`, beside an assert of what it claims); a panel
shows what each lane holds at each step, with what its event changed marked.
A number line steps through frames (`DotLine::frames`, then `frame` after each
library call), then the finished figure. Either way the figure is laid out
once, to fit every step (a space-time step-through keeps its text clear of the
arrowheads its messages show in flight, which a finished figure never draws),
and every version of every mark is tagged with the steps it shows at:

| Hook | Means |
|---|---|
| `.ds-fig[data-steps="N"][data-steps-v="1"]` | a step-through with steps `0` to `N`; `N` is the finished figure. `data-steps-v` is the version of these hooks |
| `[data-in="k"]`, `[data-out="j"]` | shown from step `k`, up to but not including step `j` (or to the end). A version with a `data-out` also carries `display="none"` in the SVG, so without a player the page shows the finished figure |
| `[data-cont]` | the version takes over from the one before it (a message moving on, a value changing), rather than appearing afresh |
| `.ds-flight` | a message line or head in flight: between its send and its arrival it reaches as far as the latest event's column |
| `--ds-len`, `--ds-grow`; `--ds-dx`, `--ds-dy` | on each version of a message line, its length and how much longer it is than the version before; on each version of a head, how far it was from here before. A page can grow the line (`stroke-dasharray`/`stroke-dashoffset`) and slide the head into place |
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
| `--ds-font-sym` | the `→` inside register entries, for monospace fonts that lack it |
| `--ds-lane-line`, `--ds-lane-opacity`, `--ds-lane-width`, `--ds-lane-name` | timelines (opacity defaults to .5) and their names |
| `--ds-event-dot`, `--ds-event-name`, `--ds-value` | event markers, event names, library values under events and beside number-line rows |
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
| `g.ds-event[data-event][data-lane]` | an event (`.ds-event-dot`, `.ds-event-name`, `.ds-event-value`) |
| `[data-actor][data-actor-n]` | a lane showing an actor, its events, and every part of a value naming that actor; `data-actor-n` is the palette slot |
| `g.ds-message[data-message][data-from][data-to]` | an arrow (`.ds-message-line`, `.ds-message-head`, `.ds-message-label`) |
| `data-notes="1 2"` | every event, message and ring a note is about; for a happened-before note, the whole path |
| `circle.ds-ring`, `g.ds-badge[data-note][data-relation]` | the ring on a note's two events, and its numbered badge (`.ds-badge-disc`, `.ds-badge-n`) |
| `li.ds-note[data-note][data-relation]` | a note in the figcaption; `data-relation` is `before` or `concurrent` when the note claims one |
| `.ds-v` | a library value inside prose: a `<tspan>` in a message label, a `<code>` in a figcaption note |
| `.ds-entry[data-actor]` | one entry of a vector clock, in values, labels and figcaption notes, as two parts: `.ds-actor` (`A`) and `.ds-count` (`:2`) |
| `.ds-dot-id[data-dot][data-actor]`, `.ds-cloud`, `.ds-sym`, `.ds-str`, `.ds-empty` | a dot's name (also split into `.ds-actor` and `.ds-count`), a dot past a causal context's gap (also `.ds-dot-id`), a register entry's arrow and value, an empty register |
| `.ds-stamp[data-stamp]`, `.ds-heard[data-actor]`, `.ds-holds`, `.ds-lamport` | a Lamport stamp `(1,A)`, every part keyed `data-stamp="1,A"` (its time also `.ds-time`, its process also `.ds-actor`); the latest stamp a mutex process has heard from a peer (split into `.ds-actor` and `.ds-count`); whether it holds the resource; a Lamport clock's time |
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
