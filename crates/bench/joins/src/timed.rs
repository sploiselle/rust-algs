//! Each join a group can time, as a type. A group's binary names only the
//! joins it times, so it compiles in only their code: the joins are generic,
//! and exist in a binary only where something calls them. A change to
//! leapfrog leaves a binary that doesn't time leapfrog exactly as it was.

use std::hash::{BuildHasherDefault, DefaultHasher};

use algs::join::{
    cascade_merge_join, hash_join, leapfrog_join, merge_join, multiway_merge_join,
    nested_loop_join, sort_merge_join,
};
use foldhash::fast::FixedState;

use crate::harness::Prepared;
use crate::{Row, Sink, baseline};

/// A join, and the inputs it is given.
pub trait Join {
    /// Its name in benchmark ids, such as `hash_sorted`.
    const NAME: &'static str;

    /// Whether it runs at `param`.
    fn runs_at(_param: usize) -> bool {
        true
    }

    /// Runs it on `data`, handing everything it emits to `out`.
    ///
    /// # Panics
    ///
    /// If `data` has the wrong number of inputs for it.
    fn run<S: Sink>(data: &Prepared, out: &mut S);
}

/// What a join was given that it can't run on.
fn wrong_inputs(name: &str) -> ! {
    panic!("{name} can't run on this workload")
}

/// `nested_loop_join` on equal keys.
pub struct NestedLoop;

impl Join for NestedLoop {
    const NAME: &'static str = "nested_loop";

    /// The nested loop is quadratic: at 64K rows a side it takes about 1 s a
    /// call, and past that it would take most of the run.
    fn runs_at(param: usize) -> bool {
        param <= 1 << 16
    }

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Binary(w) = data else {
            wrong_inputs(Self::NAME)
        };
        nested_loop_join(&w.left, &w.right, |l: &Row, r: &Row| out.pair(l, r));
    }
}

/// `hash_join`, building the left input, with foldhash.
pub struct Hash;

impl Join for Hash {
    const NAME: &'static str = "hash";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Binary(w) = data else {
            wrong_inputs(Self::NAME)
        };
        hash_join(&w.left, &w.right, FixedState::default(), |l, r| {
            out.pair(l, r)
        });
    }
}

/// The same on both inputs sorted by key. The table's buckets are still read
/// in hash order, but each probe's match sits further on in the build input
/// than the last one's, so the build rows the table points into are read in
/// address order. That removes the cost of the inputs being in different
/// orders, leaving the table's own.
pub struct HashSorted;

impl Join for HashSorted {
    const NAME: &'static str = "hash_sorted";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Binary(w) = data else {
            wrong_inputs(Self::NAME)
        };
        hash_join(
            &w.left_sorted,
            &w.right_sorted,
            FixedState::default(),
            |l, r| out.pair(l, r),
        );
    }
}

/// `hash_join` with std's hasher, SipHash-1-3, with fixed keys.
pub struct HashSipHash;

impl Join for HashSipHash {
    const NAME: &'static str = "hash_siphash";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Binary(w) = data else {
            wrong_inputs(Self::NAME)
        };
        let sip = BuildHasherDefault::<DefaultHasher>::default();
        hash_join(&w.left, &w.right, sip, |l, r| out.pair(l, r));
    }
}

/// `merge_join` on inputs sorted before timing, standing for an index.
pub struct Merge;

impl Join for Merge {
    const NAME: &'static str = "merge";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Binary(w) = data else {
            wrong_inputs(Self::NAME)
        };
        merge_join(&w.left_sorted, &w.right_sorted, |l, r| out.pair(l, r));
    }
}

/// `sort_merge_join`, charged for copying both inputs: an operator has to
/// materialize unsorted input before it can sort it.
pub struct SortMerge;

impl Join for SortMerge {
    const NAME: &'static str = "sort_merge";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Binary(w) = data else {
            wrong_inputs(Self::NAME)
        };
        sort_merge_join(w.left.to_vec(), w.right.to_vec(), |l, r| out.pair(l, r));
    }
}

/// `multiway_merge_join`.
pub struct Heap;

impl Join for Heap {
    const NAME: &'static str = "heap";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Multi { inputs, .. } = data else {
            wrong_inputs(Self::NAME)
        };
        multiway_merge_join(inputs, |t| out.tuple(t));
    }
}

/// [`baseline::multiway_merge_join_by_scan`].
pub struct Scan;

impl Join for Scan {
    const NAME: &'static str = "scan";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Multi { inputs, .. } = data else {
            wrong_inputs(Self::NAME)
        };
        baseline::multiway_merge_join_by_scan(inputs, |t| out.tuple(t));
    }
}

/// `leapfrog_join`; on two inputs, on both sorted, as [`Merge`] takes them.
pub struct Leapfrog;

impl Join for Leapfrog {
    const NAME: &'static str = "leapfrog";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        match data {
            Prepared::Binary(w) => {
                leapfrog_join(&[&w.left_sorted, &w.right_sorted], |t| out.tuple(t));
            }
            Prepared::Multi { inputs, .. } => leapfrog_join(inputs, |t| out.tuple(t)),
        }
    }
}

/// `cascade_merge_join` on the inputs smallest first, as a planner with
/// estimates orders them.
pub struct CascadePlanned;

impl Join for CascadePlanned {
    const NAME: &'static str = "cascade_planned";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Multi {
            planned, position, ..
        } = data
        else {
            wrong_inputs(Self::NAME)
        };
        // Its tuples come in planned order, so put each row back in its input's slot.
        cascade_merge_join(planned, |t| out.tuple_permuted(t, position));
    }
}

/// `cascade_merge_join` on the inputs as given.
pub struct CascadeUnplanned;

impl Join for CascadeUnplanned {
    const NAME: &'static str = "cascade_unplanned";

    fn run<S: Sink>(data: &Prepared, out: &mut S) {
        let Prepared::Multi { inputs, .. } = data else {
            wrong_inputs(Self::NAME)
        };
        cascade_merge_join(inputs, |t| out.tuple(t));
    }
}
