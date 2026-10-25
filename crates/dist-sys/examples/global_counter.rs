//! A program-wide counter: Rust's closest thing to a mutable const is a
//! `static` holding a type with interior mutability. `MonotonicCounter::new` is
//! `const` and `next` takes `&self`, so no `static mut`, `unsafe` or lock.

use dist_sys::counter::MonotonicCounter;
use std::thread;

static IDS: MonotonicCounter = MonotonicCounter::new();

fn new_request_id() -> u64 {
    IDS.next()
}

fn main() {
    let handles: Vec<_> = (0..3)
        .map(|worker| thread::spawn(move || (worker, new_request_id(), new_request_id())))
        .collect();
    for h in handles {
        let (worker, a, b) = h.join().unwrap();
        println!("worker {worker} got ids {a} and {b}");
    }
    println!("last id handed out: {}", IDS.last());
}
