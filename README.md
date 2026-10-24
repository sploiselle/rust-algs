# dist-sys

A learning repo: reference implementations of distributed-systems building
blocks in Rust, written to be read and pulled from. Each module explains the
idea it implements, and its tests check the claims the explanation makes.

## Layout

```text
crates/dist-sys/          the library
  src/clock/              tracking causality
    vector.rs             vector clocks
    dot.rs                dots and causal contexts
  src/crdt/               replicated data types
    mv_register.rs        multi-value register
crates/diagrams/          figures drawn from the library (SVG and Mermaid)
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

## Figures

The files in `figures/` are drawn by `crates/diagrams` from the reference
implementations. Each figure drives the real API, records every step, and
asserts the claims the picture makes, so every clock and register value in a
picture comes from running the code.

```bash
cargo run -p diagrams
```

That regenerates `figures/` (pass a directory to write somewhere else).
`cargo test` fails if the committed figures are stale.

The `.svg` files are meant to be inlined into a page. They use `currentColor`
and a few `--ds-*` CSS custom properties (`--ds-accent`, `--ds-message`,
`--ds-event`, `--ds-annotation`, `--ds-wrong`, `--ds-lane-opacity`), so they
follow the page's light or dark theme and can be recoloured from its
stylesheet; loaded through `<img>` they render in plain black. The `.mmd` files
are Mermaid sequence diagrams of the same scenarios, for places that render
Mermaid. The causal-context figure is a number line, which has no Mermaid form.
