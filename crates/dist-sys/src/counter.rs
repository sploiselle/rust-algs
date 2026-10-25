//! # A monotonic counter
//!
//! The simplest source of order: a number that only goes up. Every call to
//! [`MonotonicCounter::next`] returns a value strictly greater than every value
//! returned before it, even when callers race on different threads, so no two
//! callers ever see the same value.
//!
//! That is a guarantee about one counter in one process. Two counters know
//! nothing of each other, so values from different counters can't be compared
//! for causality. A [`LamportClock`](crate::clock::LamportClock) is this counter
//! plus a rule for merging in the values other processes have seen.

use std::sync::atomic::{AtomicU64, Ordering};

/// A counter whose values strictly increase, starting at 1.
#[derive(Debug, Default)]
pub struct MonotonicCounter {
    last: AtomicU64,
}

impl MonotonicCounter {
    /// `const`, so a counter can live in a `static`.
    pub const fn new() -> Self {
        Self { last: AtomicU64::new(0) }
    }

    /// The last value handed out, or 0 before the first.
    pub fn last(&self) -> u64 {
        self.last.load(Ordering::SeqCst)
    }

    /// Hand out the next value.
    ///
    /// # Panics
    ///
    /// If the counter would pass `u64::MAX`. Wrapping would break monotonicity.
    pub fn next(&self) -> u64 {
        self.last
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .expect("counter overflowed")
            + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::thread;

    #[test]
    fn counts_up_from_one() {
        let counter = MonotonicCounter::new();
        assert_eq!(counter.last(), 0);
        assert_eq!([counter.next(), counter.next(), counter.next()], [1, 2, 3]);
        assert_eq!(counter.last(), 3);
    }

    #[test]
    fn racing_threads_never_see_the_same_value() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 1_000;
        let counter = MonotonicCounter::new();
        let runs: Vec<Vec<u64>> = thread::scope(|s| {
            let handles: Vec<_> = (0..THREADS)
                .map(|_| s.spawn(|| (0..PER_THREAD).map(|_| counter.next()).collect::<Vec<_>>()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        // Each thread sees its own values strictly increase.
        for run in &runs {
            assert!(run.windows(2).all(|w| w[0] < w[1]));
        }
        // Together the threads got every value from 1 to the total, once.
        let all: HashSet<u64> = runs.into_iter().flatten().collect();
        assert_eq!(all, (1..=(THREADS * PER_THREAD) as u64).collect());
    }

    #[test]
    #[should_panic(expected = "counter overflowed")]
    fn refuses_to_wrap() {
        let counter = MonotonicCounter { last: AtomicU64::new(u64::MAX) };
        counter.next();
    }
}
