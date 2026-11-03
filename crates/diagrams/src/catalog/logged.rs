//! A key that writes down what a join does with it.
//!
//! The joins are generic in their keys, so a figure runs one, unchanged, on
//! [`Key`]s: each compares, hashes and clones as its plain `u64`, and logs
//! which row of which input it is, what was done with it, and what came out.
//! A figure rebuilds the join's steps from that log and checks them against
//! what the code says it does.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

/// A row: which input, and where in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct At {
    pub input: usize,
    pub row: usize,
}

/// What a join did, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Logged {
    /// `a == b`.
    Eq { a: At, b: At, equal: bool },
    /// `a < b`.
    Lt { a: At, b: At, below: bool },
    /// `a.cmp(b)`, or a comparison made through it, such as `a > b`.
    Cmp { a: At, b: At, ord: Ordering },
    /// The key was hashed.
    Hash(At),
    /// The row was copied.
    Clone(At),
    /// `emit` was called with these rows.
    Emit(Vec<At>),
}

thread_local! {
    static LOG: RefCell<Vec<Logged>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn log(entry: Logged) {
    LOG.with_borrow_mut(|l| l.push(entry));
}

/// A key that logs everything done with it. It compares and hashes as `key`.
#[derive(Debug)]
pub(super) struct Key {
    pub key: u64,
    pub at: At,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        let equal = self.key == other.key;
        log(Logged::Eq {
            a: self.at,
            b: other.at,
            equal,
        });
        equal
    }
}

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }

    fn lt(&self, other: &Self) -> bool {
        let below = self.key < other.key;
        log(Logged::Lt {
            a: self.at,
            b: other.at,
            below,
        });
        below
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        let ord = self.key.cmp(&other.key);
        log(Logged::Cmp {
            a: self.at,
            b: other.at,
            ord,
        });
        ord
    }
}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        log(Logged::Hash(self.at));
        self.key.hash(state);
    }
}

impl Clone for Key {
    fn clone(&self) -> Self {
        log(Logged::Clone(self.at));
        Key {
            key: self.key,
            at: self.at,
        }
    }
}

/// Each input's keys as rows of [`Key`]s that know where they are.
pub(super) fn rows(inputs: &[Vec<u64>]) -> Vec<Vec<(Key, ())>> {
    inputs
        .iter()
        .enumerate()
        .map(|(input, keys)| {
            keys.iter()
                .enumerate()
                .map(|(row, &key)| {
                    (
                        Key {
                            key,
                            at: At { input, row },
                        },
                        (),
                    )
                })
                .collect()
        })
        .collect()
}

/// What `run` did, logged from a clean start.
pub(super) fn record(run: impl FnOnce()) -> Vec<Logged> {
    LOG.with_borrow_mut(Vec::clear);
    run();
    LOG.take()
}

/// Logs a call to `emit` with these rows.
pub(super) fn emitted<'a>(rows: impl IntoIterator<Item = &'a (Key, ())>) {
    log(Logged::Emit(rows.into_iter().map(|row| row.0.at).collect()));
}
