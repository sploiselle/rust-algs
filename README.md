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
  examples/               programs to run against the library
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
it any more.

The `.svg` files are meant to be inlined into a page. They use `currentColor`
and a few `--ds-*` CSS custom properties (`--ds-accent`, `--ds-message`,
`--ds-event`, `--ds-annotation`, `--ds-wrong`, `--ds-lane-opacity`), so they
follow the page's light or dark theme and can be recoloured from its
stylesheet; loaded through `<img>` they render in plain black. Every class is
prefixed `ds-` and every rule scoped under `.ds-figure`, so the page's own
stylesheet leaves them alone, and they shrink to fit a narrow column. The
`.mmd` files are Mermaid sequence diagrams of the same scenarios, for places
that render Mermaid. The causal-context figure is a number line, which has no
Mermaid form.
