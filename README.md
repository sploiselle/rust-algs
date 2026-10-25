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
  no Mermaid form.

`index.json` lists the figures in the order a reader should meet them, with
their kinds and titles.

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
