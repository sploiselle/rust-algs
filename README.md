# rust-algs

Random algorithms in Rust.

Each algorithm is a module of the `algs` library in `src/`. Algorithms
with a command-line front end also have a binary of the same name in
`src/bin/`.

| Algorithm | Library | Binary |
| --- | --- | --- |
| Merkle trees over a directory, for finding which files changed between snapshots or across restarts | [`merkle`](src/merkle.rs) | [`merkle`](src/bin/merkle.rs) |

## Usage

Run a binary with `cargo run --bin <name>`:

```sh
cargo run --release --bin merkle -- <dir> <index-file>
```

### `merkle`

`merkle` prints what changed under `<dir>` since the last run (`+` added, `-`
removed, `~` modified), then saves the current tree to `<index-file>` for next
time.

## Adding an algorithm

1. Write the implementation and its tests in `src/<name>.rs`.
2. Export it from `src/lib.rs` with `pub mod <name>;`.
3. For a command-line front end, add `src/bin/<name>.rs`. Cargo builds it
   automatically as the `<name>` binary.
