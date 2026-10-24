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
```

## Working with it

```bash
cargo test
```

```bash
cargo doc --open
```

The rendered module docs are the write-ups.
