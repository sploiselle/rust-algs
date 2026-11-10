//! Timings of the [`algs::join`] implementations: what the timing binaries
//! run, and one binary a group to run it.
//!
//! ## Running
//!
//! ```text
//! cargo run -p bench-report --release -- run     # run whatever is out of date
//! cargo run -p bench-report --release            # the tables
//! ```
//!
//! The runner builds every bench target, sees which have no results from the
//! binary they build to now, and runs those. See `bench-report`.
//!
//! ## Why a binary a group, and four crates
//!
//! The report trusts a result for as long as the target that wrote it still
//! builds to the same binary, byte for byte. A change to code a binary links
//! can move its loops, and so its times, even when the change is to code it
//! never runs; a change to code it doesn't link can't. So each binary links
//! as little as it can:
//!
//! - each group in `groups/` has a timing binary of its own, which names only
//!   that group's joins and workload. The joins are generic and the workload
//!   generators inline, so only the named ones are compiled in;
//! - the plan of every group, the memory profiles and their claims are in
//!   `bench-plan`, the hash join's cache costs in `bench-cache`, and the
//!   tables, the runner and the run log in `bench-report`. No timing binary
//!   links any of them.
//!
//! Adding a group, or changing one, reruns that group. Changing a join reruns
//! the groups that time it. Changing a table, the runner or the run log
//! reruns nothing. Changing what every timing binary shares (the [`harness`],
//! the sinks below, `benches/support/timing.rs`) reruns them all, as it
//! should.
//!
//! ## What a timing includes
//!
//! A join hands each tuple to a [`Sink`]. The timed calls use a [`Tally`],
//! which costs the same for every tuple of every join: a count, a read of
//! each row's payload, and an XOR, kept opaque so the compiler can neither
//! skip the reads nor hoist them out of a loop. The [`Checksum`] runs only in
//! the untimed check: its three SplitMix64 rounds or more a tuple would add
//! far more to every timing than the Tally's XOR.
//!
//! The timed calls cycle through instances of each case's input, holding at
//! least [`ROTATION_ROWS`](harness::ROTATION_ROWS) rows before one repeats, so
//! the branch predictor learns little of any one. A cache that holds the
//! 16 MiB a cycle reads still serves their rows.
//!
//! Every case is checked before it's timed, by its row count against an
//! independent count and by a [`Checksum`] every join on the same input must
//! agree on, so a wrong join can't post a number.
//!
//! ## Output
//!
//! Everything lands under `target/`, so nothing generated gets committed:
//! criterion's results in `target/criterion/`, and the rest under
//! `target/bench/`.
//!
//! ## Why `cargo test` skips these crates
//!
//! criterion is slow to build, so the workspace's default members are the
//! library and the figures. `cargo test --workspace` and
//! `cargo clippy --workspace --all-targets` include them.

use std::hint::black_box;

use serde::{Deserialize, Serialize};

pub mod baseline;
pub mod harness;
pub mod joins;
pub mod workload;

/// A row: its key, and a payload. Payloads are random, never derived from the
/// key, so a checksum over them can tell apart the rows of different inputs
/// that share a key.
pub type Row = (u64, u64);

/// SplitMix64's output for state `x`: one step of the golden-ratio counter,
/// then its finalizer. It is a bijection on `u64` that spreads every input bit
/// across the output, so distinct inputs give distinct, well-mixed outputs.
pub fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Where a join's output goes: each pair a binary join emits, or each tuple
/// an n-way join emits.
pub trait Sink {
    /// Takes a pair from a binary join, left row first.
    fn pair(&mut self, left: &Row, right: &Row);

    /// Takes a tuple from an n-way join, input i's row in slot i.
    fn tuple(&mut self, rows: &[&Row]);

    /// [`tuple`](Self::tuple) for a join that took its inputs in another
    /// order: input i's row is `rows[position[i]]`.
    fn tuple_permuted(&mut self, rows: &[&Row], position: &[usize]);
}

/// What a join emitted: how many tuples, and a wrapping sum of a hash of each.
/// The sum ignores the order tuples arrive in, but each tuple's hash depends on
/// which row sits in which slot, so a mis-pairing or a swap changes it. (A XOR
/// of payloads would cancel, and couldn't see either.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksum {
    pub rows: u64,
    pub sum: u64,
}

impl Sink for Checksum {
    fn pair(&mut self, left: &Row, right: &Row) {
        self.add([left, right]);
    }

    fn tuple(&mut self, rows: &[&Row]) {
        self.add(rows.iter().copied());
    }

    fn tuple_permuted(&mut self, rows: &[&Row], position: &[usize]) {
        self.add(position.iter().map(|&slot| rows[slot]));
    }
}

impl Checksum {
    /// Hashes the tuple's key, then folds in each payload in slot order.
    fn add<'a>(&mut self, rows: impl IntoIterator<Item = &'a Row>) {
        let mut rows = rows.into_iter().peekable();
        let key = rows.peek().expect("a tuple has a row").0;
        let hash = rows.fold(mix(key), |hash, row| mix(hash ^ row.1));
        self.rows += 1;
        self.sum = self.sum.wrapping_add(hash);
    }
}

/// What the timed calls emit into: a count of the tuples, and nothing else
/// kept. It reads every row's payload and XORs them, through
/// [`black_box`], so each tuple costs the same small,
/// fixed work whichever join emits it, and none of it can be skipped, hoisted
/// out of a loop or vectorized across tuples.
#[derive(Debug, Default)]
pub struct Tally {
    pub tuples: u64,
}

impl Sink for Tally {
    fn pair(&mut self, left: &Row, right: &Row) {
        self.tuples += 1;
        black_box(left.1 ^ right.1);
    }

    fn tuple(&mut self, rows: &[&Row]) {
        self.tuples += 1;
        black_box(rows.iter().fold(0, |xor, row| xor ^ row.1));
    }

    /// The same as [`tuple`](Sink::tuple): an XOR doesn't depend on the order
    /// of the rows, so both binary plans pay the same.
    fn tuple_permuted(&mut self, rows: &[&Row], _position: &[usize]) {
        self.tuple(rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A1: Row = (1, 11);
    const A2: Row = (2, 12);
    const B1: Row = (1, 21);
    const B2: Row = (2, 22);

    fn pairs(pairs: &[(Row, Row)]) -> Checksum {
        let mut sum = Checksum::default();
        for (left, right) in pairs {
            sum.pair(left, right);
        }
        sum
    }

    #[test]
    fn checksum_ignores_emit_order() {
        assert_eq!(pairs(&[(A1, B1), (A2, B2)]), pairs(&[(A2, B2), (A1, B1)]));
    }

    #[test]
    fn checksum_sees_a_mispairing() {
        // Both emit every row once, so a sum or XOR of payloads can't tell them apart.
        assert_ne!(pairs(&[(A1, B2), (A2, B1)]), pairs(&[(A1, B1), (A2, B2)]));
    }

    #[test]
    fn checksum_sees_swapped_slots() {
        assert_ne!(pairs(&[(A1, B1)]), pairs(&[(B1, A1)]));

        let mut permuted = Checksum::default();
        permuted.tuple_permuted(&[&B1, &A1], &[1, 0]);
        let mut direct = Checksum::default();
        direct.tuple(&[&A1, &B1]);
        assert_eq!(permuted, direct);
    }
}
