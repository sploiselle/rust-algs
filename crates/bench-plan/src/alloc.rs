//! Counting what a piece of code asks the allocator for: the most bytes it
//! has live at once, how many bytes and allocations it requests, and what it
//! leaves behind.
//!
//! The counts are per thread. A test harness runs tests on parallel threads,
//! and a global count would mix their allocations; a thread-local one sees
//! only the code [`measure`] runs. A free is charged to the thread that frees,
//! so the counts are exact only for code that frees on its own thread what it
//! allocates there; the joins and the memory harness are single-threaded.
//! These are bytes requested, not resident memory: the allocator's own
//! overhead and the pages it maps aren't counted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

use serde::{Deserialize, Serialize};

/// One thread's running totals.
#[derive(Clone, Copy, Default)]
struct Stats {
    /// Bytes allocated and not yet freed.
    live: isize,
    /// The most `live` has been since [`measure`] last reset it.
    peak: isize,
    /// Bytes requested, freed or not.
    allocated: usize,
    /// Calls to `alloc`, `alloc_zeroed` and `realloc`.
    allocations: usize,
}

impl Stats {
    fn grew(&mut self, bytes: usize) {
        self.live += bytes as isize;
        self.peak = self.peak.max(self.live);
        self.allocated += bytes;
        self.allocations += 1;
    }
}

thread_local! {
    // const-initialized and without Drop, so the allocator can touch it at any
    // point in a thread's life; `try_with` skips the thread's last moments.
    static STATS: Cell<Stats> = const {
        Cell::new(Stats { live: 0, peak: 0, allocated: 0, allocations: 0 })
    };
}

fn update(f: impl FnOnce(&mut Stats)) {
    let _ = STATS.try_with(|cell| {
        let mut stats = cell.get();
        f(&mut stats);
        cell.set(stats);
    });
}

/// The system allocator, counting what each thread asks it for. A binary
/// installs it with `#[global_allocator]`.
pub struct CountingAlloc;

// SAFETY: every call is passed straight to `System`; the counting touches
// only a thread-local `Cell`, which never allocates.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            update(|s| s.grew(layout.size()));
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            update(|s| s.grew(layout.size()));
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        update(|s| s.live -= layout.size() as isize);
    }

    /// Counted as a move: the new block live alongside the old, then the old
    /// freed. That overstates a realloc that grows in place, and never
    /// understates one that moves.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            update(|s| {
                s.grew(new_size);
                s.live -= layout.size() as isize;
            });
        }
        new
    }
}

/// What a piece of code asked the allocator for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// The most bytes it had live at once, beyond what was live before it ran.
    pub peak_bytes: usize,
    /// Bytes it requested, freed or not.
    pub allocated_bytes: usize,
    /// Its calls to `alloc`, `alloc_zeroed` and `realloc`.
    pub allocations: usize,
    /// Bytes still live when it returned, its result's included.
    pub retained_bytes: isize,
}

/// Runs `f` on this thread and reports what it asked the allocator for.
/// Allocations on other threads, even ones `f` spawns, aren't counted.
///
/// Frees are charged to the thread that frees, so the counts are exact only
/// when `f` frees nothing allocated before it ran or on another thread, and
/// nothing `f` allocates is freed on another thread. Otherwise the live count
/// can drop below where it started, so `peak_bytes` understates and
/// `retained_bytes` is off by the bytes that crossed.
pub fn measure<T>(f: impl FnOnce() -> T) -> (Usage, T) {
    let before = STATS.with(|cell| {
        let stats = cell.get();
        cell.set(Stats {
            peak: stats.live,
            ..stats
        });
        stats
    });
    let result = f();
    let after = STATS.with(Cell::get);
    // Put back the peak an enclosing `measure` is tracking.
    update(|s| s.peak = s.peak.max(before.peak));
    let usage = Usage {
        peak_bytes: (after.peak - before.live) as usize,
        allocated_bytes: after.allocated - before.allocated,
        allocations: after.allocations - before.allocations,
        retained_bytes: after.live - before.live,
    };
    (usage, result)
}

/// Whether [`CountingAlloc`] is this binary's global allocator: without it,
/// every [`measure`] would report nothing.
pub fn installed() -> bool {
    measure(|| black_box(Box::new(0u64))).0.allocations == 1
}
