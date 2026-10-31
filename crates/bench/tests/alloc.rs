//! The counting allocator's claims, and the memory claims the post makes about
//! the joins, at sizes small enough for a debug build. This binary installs
//! the allocator itself; its counts are per thread, so tests run in parallel.

use std::hint::black_box;
use std::thread;

use algs::join::{
    hash_join, leapfrog_join, merge_join, multiway_merge_join, nested_loop_join, sort_merge_join,
};
use bench::Row;
use bench::alloc::{CountingAlloc, Usage, installed, measure};
use foldhash::fast::FixedState;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// A Vec of `bytes` bytes, which the optimizer can't remove.
fn block(bytes: usize) -> Vec<u8> {
    black_box(Vec::with_capacity(bytes))
}

/// `n` rows with keys `0..n / per_key`, each key on `per_key` rows in a row.
fn rows(n: u64, per_key: u64) -> Vec<Row> {
    (0..n).map(|i| (i / per_key, i)).collect()
}

/// What a join of `build` and `probe` allocates.
fn hash_usage(build: &[Row], probe: &[Row]) -> Usage {
    measure(|| {
        hash_join(build, probe, FixedState::default(), |l, r| {
            black_box((l, r));
        })
    })
    .0
}

/// An n-way join, emitting through a trait object.
type Join = fn(&[&[Row]], &mut dyn FnMut(&[&Row]));

/// What `join` on `k` copies of `rows` allocates.
fn multiway_usage(k: usize, rows: &[Row], join: Join) -> Usage {
    let inputs = vec![rows; k];
    measure(|| {
        join(&inputs, &mut |t| {
            black_box(t);
        })
    })
    .0
}

#[test]
fn peak_is_the_most_live_at_once() {
    let (usage, ()) = measure(|| {
        drop(block(1000));
        drop(block(1000));
    });
    assert_eq!(usage.peak_bytes, 1000);
    assert_eq!(usage.allocated_bytes, 2000);
    assert_eq!(usage.allocations, 2);
    assert_eq!(usage.retained_bytes, 0);
}

#[test]
fn memory_live_before_measuring_does_not_count() {
    let held = block(1 << 20);
    let (usage, ()) = measure(|| drop(block(1000)));
    assert_eq!(usage.peak_bytes, 1000);
    drop(held);
}

#[test]
fn other_threads_allocations_dont_count() {
    // The other thread frees its block too: a free is charged to the thread
    // that frees.
    let (usage, ()) =
        measure(|| thread::scope(|s| s.spawn(|| drop(block(16 << 20))).join()).unwrap());
    // Spawning a thread allocates a little on this one, but not its 16 MiB.
    assert!(usage.peak_bytes < 1 << 16, "{usage:?}");
    assert!(usage.retained_bytes.unsigned_abs() < 1 << 16, "{usage:?}");
}

#[test]
fn a_realloc_counts_both_blocks() {
    let (usage, ()) = measure(|| {
        let mut grown = block(1000);
        grown.reserve_exact(3000);
        drop(black_box(grown));
    });
    assert_eq!(usage.peak_bytes, 1000 + 3000);
    assert_eq!(usage.allocated_bytes, 1000 + 3000);
    assert_eq!(usage.allocations, 2);
}

#[test]
fn nested_loop_and_merge_joins_allocate_nothing() {
    let (left, right) = (rows(256, 1), rows(1024, 4));
    let nested = measure(|| {
        nested_loop_join(
            &left,
            &right,
            |l, r| l.0 == r.0,
            |l, r| {
                black_box((l, r));
            },
        )
    })
    .0;
    let merge = measure(|| {
        merge_join(&left, &right, |l, r| {
            black_box((l, r));
        })
    })
    .0;
    assert_eq!(nested, Usage::default());
    assert_eq!(merge, Usage::default());
}

#[test]
fn a_hash_join_allocates_twice_for_any_nonempty_build() {
    let probe = rows(4096, 1);
    let unique = hash_usage(&rows(1024, 1), &probe);
    let repeated = hash_usage(&rows(1024, 16), &probe);
    assert_eq!(unique.allocations, 2);
    assert_eq!(repeated.allocations, 2);
    // The table is sized for the build's rows, not its keys.
    assert_eq!(unique.peak_bytes, repeated.peak_bytes);
    assert_eq!(hash_usage(&[], &probe), Usage::default());
}

#[test]
fn hash_join_memory_follows_the_build_side() {
    let build = rows(1024, 1);
    let once = hash_usage(&build, &rows(1024, 1));
    let four_times = hash_usage(&build, &rows(4096, 1));
    assert_eq!(once.peak_bytes, four_times.peak_bytes);
}

#[test]
fn sort_merge_allocates_exactly_its_inputs() {
    let (left, right) = (rows(256, 1), rows(1024, 4));
    let (usage, ()) = measure(|| {
        sort_merge_join(left.to_vec(), right.to_vec(), |l, r| {
            black_box((l, r));
        })
    });
    assert_eq!(
        usage.peak_bytes,
        size_of::<Row>() * (left.len() + right.len())
    );
    assert_eq!(usage.allocations, 2);
}

#[test]
fn multiway_memory_follows_k_not_rows() {
    for n in [1 << 10, 1 << 14] {
        let rows = rows(n, 1);
        for k in [2, 3, 8] {
            let heap = multiway_usage(k, &rows, |inputs, emit| multiway_merge_join(inputs, emit));
            let leapfrog = multiway_usage(k, &rows, |inputs, emit| leapfrog_join(inputs, emit));
            assert_eq!(
                (heap.peak_bytes, heap.allocations),
                (56 * k, 4),
                "heap, k = {k}, n = {n}"
            );
            assert_eq!(
                (leapfrog.peak_bytes, leapfrog.allocations),
                (48 * k, 4),
                "leapfrog, k = {k}, n = {n}"
            );
        }
    }
}

#[test]
fn installed_detects_the_allocator() {
    assert!(installed());
}
