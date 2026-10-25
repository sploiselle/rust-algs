//! # Hybrid logical clocks
//!
//! A hybrid logical clock (HLC; Kulkarni et al., *Logical Physical Clocks*,
//! 2014) stamps events with a pair `(wall, logical)`: `wall` is the largest
//! physical time the event has heard of, and `logical` is a counter that orders
//! events sharing a `wall`. Stamps compare lexicographically, and like a
//! Lamport clock they promise
//!
//! ```text
//!     e → f  (e happened-before f)   ⇒   HLC(e) < HLC(f)
//! ```
//!
//! but only that direction. `HLC(e) < HLC(f)` is just as consistent with `e`
//! and `f` being concurrent; deciding happened-before exactly takes a
//! [`VectorClock`](super::VectorClock).
//!
//! What the physical half buys is that a stamp means something outside the
//! system. `wall` never falls behind the local physical clock, and it runs
//! ahead of it by at most the clocks' synchronization error, so a stamp is a
//! wall-clock time to within that error. That makes it usable as a database
//! version ("read as of 10:32"), and it stays one fixed-size value however many
//! nodes there are.
//!
//! The rules, with `pt` the local physical clock and `last` the last stamp this
//! clock issued:
//! 1. **Local or send event**: if `pt > last.wall`, issue `(pt, 0)`; otherwise
//!    physical time hasn't caught up with `last`, so issue
//!    `(last.wall, last.logical + 1)`. A send attaches the stamp to the message.
//! 2. **Receive** of a message stamped `remote`: take the largest of `pt`,
//!    `last.wall` and `remote.wall` as the new `wall`. If `pt` alone is largest,
//!    `logical` is 0; otherwise it's one more than the largest `logical` among
//!    the stamps whose `wall` is the new one.
//!
//! The representation follows CockroachDB's: `wall` is nanoseconds since the
//! Unix epoch and `logical` a 32-bit counter, and the last stamp sits behind a
//! mutex so one clock can serve every thread on a node. Should more than 2³²
//! events share one `wall` (physical time would have to stand still for a long
//! while), the counter carries into `wall` by a nanosecond rather than wrapping.
//!
//! ## Bounding the skew
//!
//! An HLC trusts every timestamp it receives. A peer whose clock is an hour
//! fast drags every clock it talks to an hour ahead, and they stay there until
//! physical time catches up. [`Hlc::with_max_offset`] bounds the damage:
//! [`Hlc::update`] refuses any timestamp more than that far ahead of the local
//! physical clock, leaving the clock where it was.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// An HLC timestamp. Totally ordered (by `wall`, then `logical`), unlike a
/// vector clock: two stamps always compare, and the order agrees with
/// happened-before wherever happened-before says anything.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Physical time, in nanoseconds since the Unix epoch.
    pub wall: u64,
    /// Orders events that share a `wall`.
    pub logical: u32,
}

impl Timestamp {
    /// The next stamp after `self` at the same `wall`, carrying into `wall` if
    /// `logical` is exhausted. Stays put at the very last timestamp, which is
    /// reachable only by accepting an absurd remote stamp with no max offset.
    fn tick(self) -> Self {
        match self.logical.checked_add(1) {
            Some(logical) => Self { wall: self.wall, logical },
            None => match self.wall.checked_add(1) {
                Some(wall) => Self { wall, logical: 0 },
                None => self,
            },
        }
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}", self.wall, self.logical)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// A source of physical time, in nanoseconds since the Unix epoch.
///
/// It needn't be monotonic: the HLC never issues a smaller timestamp when the
/// physical clock steps backwards, it just counts on in `logical` until
/// physical time passes its `wall` again.
pub trait PhysicalClock {
    fn now_nanos(&self) -> u64;
}

impl<C: PhysicalClock + ?Sized> PhysicalClock for &C {
    fn now_nanos(&self) -> u64 {
        (**self).now_nanos()
    }
}

impl<C: PhysicalClock + ?Sized> PhysicalClock for Arc<C> {
    fn now_nanos(&self) -> u64 {
        (**self).now_nanos()
    }
}

/// The operating system's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl PhysicalClock for SystemClock {
    fn now_nanos(&self) -> u64 {
        // A clock set before 1970 reads as 0; the HLC stays monotonic regardless.
        // Nanoseconds since the epoch fit in a u64 until the year 2554.
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
    }
}

/// A physical clock that only moves when told to, for tests and simulations.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicU64);

impl ManualClock {
    pub fn new(nanos: u64) -> Self {
        Self(AtomicU64::new(nanos))
    }

    /// Set the time, backwards as well as forwards.
    pub fn set(&self, nanos: u64) {
        self.0.store(nanos, Ordering::Relaxed);
    }

    pub fn advance(&self, nanos: u64) {
        self.0.fetch_add(nanos, Ordering::Relaxed);
    }
}

impl PhysicalClock for ManualClock {
    fn now_nanos(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// [`Hlc::update`] refused a timestamp too far ahead of the local physical clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSkew {
    /// The timestamp that was refused.
    pub remote: Timestamp,
    /// The local physical clock when it was refused, in nanoseconds.
    pub local_wall: u64,
    /// The configured bound.
    pub max_offset: Duration,
}

impl fmt::Display for ClockSkew {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "remote timestamp {} is {:?} ahead of the local clock ({}), beyond the {:?} bound",
            self.remote,
            Duration::from_nanos(self.remote.wall - self.local_wall),
            self.local_wall,
            self.max_offset,
        )
    }
}

impl Error for ClockSkew {}

/// A hybrid logical clock reading physical time from `C`.
///
/// Every method takes `&self`, so a node shares one clock between threads
/// (behind an `Arc`, or by reference in scoped threads). Each call issues a
/// timestamp larger than every one this clock issued before it.
pub struct Hlc<C = SystemClock> {
    clock: C,
    /// The last timestamp issued.
    last: Mutex<Timestamp>,
    /// `Duration::MAX` means unbounded.
    max_offset: Duration,
}

impl Hlc<SystemClock> {
    /// A clock reading the operating system's wall clock, with no skew bound.
    pub fn system() -> Self {
        Self::new(SystemClock)
    }
}

impl<C: PhysicalClock> Hlc<C> {
    /// A clock reading physical time from `clock`, with no skew bound.
    pub fn new(clock: C) -> Self {
        Self { clock, last: Mutex::new(Timestamp::default()), max_offset: Duration::MAX }
    }

    /// Refuse, in [`update`](Self::update), any remote timestamp whose `wall` is
    /// more than `max_offset` ahead of the local physical clock.
    pub fn with_max_offset(mut self, max_offset: Duration) -> Self {
        self.max_offset = max_offset;
        self
    }

    /// Rule 1: stamp a local or send event.
    pub fn now(&self) -> Timestamp {
        let mut last = self.lock();
        let pt = self.clock.now_nanos();
        let next = if pt > last.wall {
            // Physical time has moved past everything we've heard of.
            Timestamp { wall: pt, logical: 0 }
        } else {
            // It hasn't caught up with the last stamp yet; count on from it.
            last.tick()
        };
        *last = next;
        next
    }

    /// Rule 2: absorb a received timestamp and stamp the receive event, which is
    /// larger than both `remote` and everything this clock issued before.
    ///
    /// With a [max offset](Self::with_max_offset) set, a `remote` too far ahead
    /// of the local physical clock is refused and the clock is left unchanged.
    pub fn update(&self, remote: Timestamp) -> Result<Timestamp, ClockSkew> {
        let mut last = self.lock();
        let pt = self.clock.now_nanos();
        if Duration::from_nanos(remote.wall.saturating_sub(pt)) > self.max_offset {
            return Err(ClockSkew { remote, local_wall: pt, max_offset: self.max_offset });
        }
        let next = if pt > last.wall && pt > remote.wall {
            // Physical time is ahead of both; neither counter matters.
            Timestamp { wall: pt, logical: 0 }
        } else if remote.wall > last.wall {
            // The sender has heard of a later time than we have; adopt it.
            remote.tick()
        } else if last.wall > remote.wall {
            // We're ahead of the sender; it's an ordinary next event for us.
            last.tick()
        } else {
            // Same wall time: count past whichever counter is further along.
            (*last).max(remote).tick()
        };
        *last = next;
        Ok(next)
    }

    /// The last timestamp issued, without issuing a new one.
    pub fn last(&self) -> Timestamp {
        *self.lock()
    }

    /// The guarded timestamp is replaced in one assignment, so a thread that
    /// panicked while holding the lock can't have left it half-written.
    fn lock(&self) -> MutexGuard<'_, Timestamp> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<C> fmt::Debug for Hlc<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let last = *self.last.lock().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("Hlc").field("last", &last).field("max_offset", &self.max_offset).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Rng;
    use std::collections::HashSet;

    fn ts(wall: u64, logical: u32) -> Timestamp {
        Timestamp { wall, logical }
    }

    #[test]
    fn timestamps_order_by_wall_then_logical() {
        assert!(ts(5, 3) < ts(5, 4));
        assert!(ts(5, u32::MAX) < ts(6, 0));
        assert_eq!(ts(5, u32::MAX).tick(), ts(6, 0));
        assert_eq!(format!("{:?}", ts(42, 3)), "42,3");
    }

    #[test]
    fn follows_physical_time_when_it_advances() {
        let pt = ManualClock::new(100);
        let hlc = Hlc::new(&pt);
        assert_eq!(hlc.now(), ts(100, 0));
        assert_eq!(hlc.now(), ts(100, 1));
        pt.advance(1);
        assert_eq!(hlc.now(), ts(101, 0));
        assert_eq!(hlc.last(), ts(101, 0));
    }

    #[test]
    fn stays_monotonic_when_physical_time_steps_back() {
        let pt = ManualClock::new(100);
        let hlc = Hlc::new(&pt);
        hlc.now();
        hlc.now();
        pt.set(50);
        assert_eq!(hlc.now(), ts(100, 2));
        assert_eq!(hlc.update(ts(80, 9)), Ok(ts(100, 3)));
        pt.set(101);
        assert_eq!(hlc.now(), ts(101, 0));
    }

    #[test]
    fn receive_moves_past_remote_and_local() {
        let pt = ManualClock::new(100);
        let hlc = Hlc::new(&pt);
        hlc.now();
        assert_eq!(hlc.update(ts(200, 7)), Ok(ts(200, 8)));
        // The physical clock is still at 100; the remote's wall sticks until it passes.
        assert_eq!(hlc.now(), ts(200, 9));
        assert_eq!(hlc.update(ts(150, 40)), Ok(ts(200, 10)));
        assert_eq!(hlc.update(ts(200, 40)), Ok(ts(200, 41)));
        pt.set(201);
        assert_eq!(hlc.update(ts(200, 99)), Ok(ts(201, 0)));
    }

    #[test]
    fn logical_overflow_carries_into_wall() {
        let pt = ManualClock::new(100);
        let hlc = Hlc::new(&pt);
        hlc.now();
        assert_eq!(hlc.update(ts(100, u32::MAX)), Ok(ts(101, 0)));
        assert_eq!(hlc.now(), ts(101, 1));
    }

    #[test]
    fn refuses_remote_beyond_max_offset_and_stays_put() {
        let pt = ManualClock::new(100);
        let hlc = Hlc::new(&pt).with_max_offset(Duration::from_nanos(10));
        let before = hlc.now();
        let err = hlc.update(ts(111, 0)).unwrap_err();
        assert_eq!(err, ClockSkew { remote: ts(111, 0), local_wall: 100, max_offset: Duration::from_nanos(10) });
        assert_eq!(hlc.last(), before);
        assert_eq!(hlc.update(ts(110, 5)), Ok(ts(110, 6)));
    }

    #[test]
    fn unbounded_clock_stops_at_the_last_timestamp_instead_of_wrapping() {
        let hlc = Hlc::new(ManualClock::new(100));
        let end = ts(u64::MAX, u32::MAX);
        assert_eq!(hlc.update(end), Ok(end));
        assert_eq!(hlc.now(), end);
    }

    #[test]
    fn system_clock_tracks_wall_time() {
        let hlc = Hlc::system();
        let wall = SystemClock.now_nanos();
        let t = hlc.now();
        assert!(t.wall >= wall && t.wall - wall < 1_000_000_000, "{t} vs {wall}");
    }

    #[test]
    fn concurrent_callers_get_unique_increasing_timestamps() {
        const THREADS: usize = 4;
        const CALLS: usize = 20_000;
        let pt = ManualClock::new(1_000);
        let hlc = Hlc::new(&pt);
        let runs: Vec<Vec<Timestamp>> = std::thread::scope(|s| {
            let workers: Vec<_> = (0..THREADS)
                .map(|i| {
                    let (hlc, pt) = (&hlc, &pt);
                    s.spawn(move || {
                        (0..CALLS)
                            .map(|n| {
                                if i == 0 && n % 100 == 0 {
                                    pt.advance(1);
                                }
                                hlc.now()
                            })
                            .collect()
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        let mut seen = HashSet::new();
        for run in &runs {
            assert!(run.windows(2).all(|w| w[0] < w[1]), "each caller sees increasing stamps");
            seen.extend(run.iter().copied());
        }
        assert_eq!(seen.len(), THREADS * CALLS, "no stamp issued twice");
    }

    /// Random executions, non-FIFO delivery, physical clocks that each run a
    /// fixed amount (at most `EPS`) ahead of true time, and true time that often
    /// stands still so counters climb. On every event, `wall` is at least the
    /// local physical time and at most `EPS` past it; and across all pairs,
    /// `e → f` implies `HLC(e) < HLC(f)`.
    #[test]
    fn stays_near_physical_time_and_respects_happens_before() {
        const NODES: usize = 4;
        const EVENTS: usize = 120;
        const EPS: u64 = 5;
        for seed in 1..=100u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let offsets: Vec<u64> = (0..NODES).map(|_| rng.below(EPS as usize + 1) as u64).collect();
            let clocks: Vec<ManualClock> = offsets.iter().map(|&o| ManualClock::new(1_000 + o)).collect();
            let hlcs: Vec<Hlc<&ManualClock>> = clocks.iter().map(Hlc::new).collect();
            let mut true_time = 1_000;

            let mut last_event: Vec<Option<usize>> = vec![None; NODES];
            // (destination, id of the send event, stamp)
            let mut in_flight: Vec<(usize, usize, Timestamp)> = Vec::new();
            let mut stamps: Vec<Timestamp> = Vec::new();
            let mut parents: Vec<Vec<usize>> = Vec::new();

            for _ in 0..EVENTS {
                true_time += rng.below(3).saturating_sub(1) as u64;
                for (clock, &o) in clocks.iter().zip(&offsets) {
                    clock.set(true_time + o);
                }

                let n = rng.below(NODES);
                let pt = clocks[n].now_nanos();
                let id = stamps.len();
                let mut ps: Vec<usize> = last_event[n].into_iter().collect();
                let deliverable: Vec<usize> = (0..in_flight.len()).filter(|&i| in_flight[i].0 == n).collect();
                let stamp = match rng.below(3) {
                    1 if !deliverable.is_empty() => {
                        let i = deliverable[rng.below(deliverable.len())];
                        let (_, send_id, m) = in_flight.swap_remove(i);
                        ps.push(send_id);
                        hlcs[n].update(m).unwrap()
                    }
                    choice => {
                        let stamp = hlcs[n].now();
                        if choice == 0 {
                            let to = (n + 1 + rng.below(NODES - 1)) % NODES;
                            in_flight.push((to, id, stamp));
                        }
                        stamp
                    }
                };

                assert!(stamp.wall >= pt, "seed {seed}: e{id} behind physical time");
                assert!(stamp.wall - pt <= EPS, "seed {seed}: e{id} drifted past EPS");

                last_event[n] = Some(id);
                stamps.push(stamp);
                parents.push(ps);
            }

            // Events were recorded in a topological order, so one forward pass
            // computes each event's causal past.
            let mut ancestors: Vec<HashSet<usize>> = Vec::with_capacity(stamps.len());
            for ps in &parents {
                let mut s = HashSet::new();
                for &p in ps {
                    s.insert(p);
                    s.extend(ancestors[p].iter().copied());
                }
                ancestors.push(s);
            }
            for (f, past) in ancestors.iter().enumerate() {
                for &e in past {
                    assert!(stamps[e] < stamps[f], "seed {seed}: e{e} → e{f} but {:?} ≮ {:?}", stamps[e], stamps[f]);
                }
            }
        }
    }
}
