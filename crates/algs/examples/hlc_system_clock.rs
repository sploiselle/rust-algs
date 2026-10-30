//! Does an HLC on this machine's real clock ever need its logical component?
//!
//! Runs each scenario for a second against the operating system's wall clock:
//! raw clock reads, one thread stamping events, several threads sharing one
//! clock, and two clocks passing a message back and forth with no delay. Each
//! reports how often a stamp only stayed unique, or only stayed after the
//! message it received, because of `logical`.
//!
//! ```bash
//! cargo run --release --example hlc_system_clock
//! ```
//!
//! The numbers depend on the platform's clock resolution. On macOS the wall
//! clock moves in whole microseconds, so most stamps in a tight loop share a
//! `wall` with the one before.

use algs::clock::Timestamp;
use algs::clock::hybrid::{Hlc, PhysicalClock, SystemClock};
use std::time::{Duration, Instant};

const RUN: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Stats {
    stamps: u64,
    logical_nonzero: u64,
    max_logical: u32,
    walls: u64,
}

impl Stats {
    fn record(&mut self, prev: Option<Timestamp>, t: Timestamp) {
        self.stamps += 1;
        if t.logical > 0 {
            self.logical_nonzero += 1;
        }
        self.max_logical = self.max_logical.max(t.logical);
        if prev.is_none_or(|p| p.wall != t.wall) {
            self.walls += 1;
        }
    }

    fn print(&self, label: &str) {
        println!(
            "{label}: {} stamps, {} ({:.1}%) needed logical > 0, max logical {}, ~{:.1} stamps per distinct wall",
            self.stamps,
            self.logical_nonzero,
            100.0 * self.logical_nonzero as f64 / self.stamps as f64,
            self.max_logical,
            self.stamps as f64 / self.walls as f64,
        );
    }
}

fn raw_clock() {
    let clock = SystemClock;
    let (mut reads, mut repeats, mut backwards, mut not_whole_micros) = (0u64, 0u64, 0u64, 0u64);
    let mut min_step = u64::MAX;
    let mut prev = clock.now_nanos();
    let start = Instant::now();
    while start.elapsed() < RUN {
        let now = clock.now_nanos();
        reads += 1;
        if !now.is_multiple_of(1_000) {
            not_whole_micros += 1;
        }
        match now.cmp(&prev) {
            std::cmp::Ordering::Equal => repeats += 1,
            std::cmp::Ordering::Less => backwards += 1,
            std::cmp::Ordering::Greater => min_step = min_step.min(now - prev),
        }
        prev = now;
    }
    println!(
        "raw SystemTime: {reads} reads, {repeats} ({:.1}%) repeated the previous reading, {backwards} went backwards, \
         smallest forward step {min_step}ns, {not_whole_micros} readings not a whole microsecond",
        100.0 * repeats as f64 / reads as f64,
    );
}

fn one_thread() {
    let hlc = Hlc::system();
    let mut stats = Stats::default();
    let mut prev = None;
    let start = Instant::now();
    while start.elapsed() < RUN {
        let t = hlc.now();
        stats.record(prev, t);
        prev = Some(t);
    }
    stats.print("1 thread, now()");
}

fn many_threads(threads: usize) {
    let hlc = Hlc::system();
    let mut all: Vec<Timestamp> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let hlc = &hlc;
                s.spawn(move || {
                    let mut out = Vec::new();
                    let start = Instant::now();
                    while start.elapsed() < RUN {
                        out.push(hlc.now());
                    }
                    out
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap())
            .collect()
    });
    all.sort();
    let distinct = all.len();
    all.dedup();
    assert_eq!(distinct, all.len(), "duplicate stamps");
    let mut stats = Stats::default();
    let mut prev = None;
    for &t in &all {
        stats.record(prev, t);
        prev = Some(t);
    }
    stats.print(&format!("{threads} threads sharing one clock, now()"));
}

/// Two nodes on this machine: A stamps a send, B receives it immediately.
fn send_receive() {
    let (a, b) = (Hlc::system(), Hlc::system());
    let (mut msgs, mut same_wall, mut pt_not_ahead) = (0u64, 0u64, 0u64);
    let start = Instant::now();
    while start.elapsed() < RUN {
        let sent = a.now();
        let received = b.update(sent).unwrap();
        assert!(received > sent);
        msgs += 1;
        if received.wall == sent.wall {
            same_wall += 1; // only the logical counter put the receive after the send
        }
        if SystemClock.now_nanos() <= sent.wall {
            pt_not_ahead += 1;
        }
    }
    println!(
        "A sends, B receives: {msgs} messages, {same_wall} ({:.1}%) received in the sender's wall tick, so only \
         logical ordered receive after send; {pt_not_ahead} times the physical clock read after receiving hadn't \
         passed the send's wall",
        100.0 * same_wall as f64 / msgs as f64,
    );
}

fn main() {
    raw_clock();
    one_thread();
    for threads in [2, 4, 8] {
        many_threads(threads);
    }
    send_receive();
}
