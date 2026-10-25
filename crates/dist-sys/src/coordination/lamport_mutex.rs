//! # Lamport's mutual exclusion
//!
//! The algorithm from the paper that introduced the [`LamportClock`]
//! (Lamport, *Time, Clocks, and the Ordering of Events in a Distributed
//! System*, 1978). Processes share one resource, there's no coordinator, and
//! the rules are:
//!
//! 1. Grants go in order: if one request is granted before another, it was
//!    made first.
//! 2. A holder releases before anyone else is granted.
//! 3. Every request is eventually granted, as long as every holder eventually
//!    releases.
//!
//! "Made first" is the trouble: processes don't share a clock, so they can't
//! compare when requests were made. They can compare [`LamportStamp`]s, though,
//! and those never contradict happened-before, so every process sorts the same
//! requests into the same order without asking anyone.
//!
//! ## The algorithm
//!
//! Every process keeps a queue of requests, sorted by stamp.
//!
//! 1. **Request**: stamp the request, put it in your own queue, and send it to
//!    everyone else.
//! 2. **Receive a request**: put it in your queue and send an acknowledgement.
//! 3. **Release**: take your request out of your queue and send a release to
//!    everyone else.
//! 4. **Receive a release**: take the sender's request out of your queue.
//! 5. **Enter** once your request is first in your queue *and* you've heard
//!    something stamped later than it from every other process.
//!
//! The second half of rule 5 is what makes the first half mean anything. A
//! request at the head of your queue might only be there because an earlier one
//! is still in flight. But messages on a channel arrive in the order they were
//! sent, and a process's stamps only go up, so once you've heard from a process
//! past your request's stamp, every request it made before yours has already
//! arrived. Hear that from everyone and nothing earlier can still turn up.
//! Acknowledgements exist only to make sure there's something to hear.
//!
//! Each process ends up with the same queue, built by applying the same
//! requests and releases in the same order. That's the paper's larger point:
//! any shared state can be replicated this way, by running the same state
//! machine everywhere on commands sorted by Lamport stamp. The mutex is its
//! example.
//!
//! ## What it needs
//!
//! - **FIFO channels.** If a later message overtakes a request, its receiver
//!   decides nothing earlier is on the way, and two processes enter together
//!   (`overtaking_breaks_mutual_exclusion` builds that execution).
//! - **No message loss, and no crashes.** A process enters only after hearing
//!   from *every* other process, so one that crashes or goes quiet blocks
//!   everyone, forever. The algorithm tolerates no failures at all.
//! - **`3(N − 1)` messages per entry**: a request, an acknowledgement and a
//!   release for each other process. Ricart and Agrawala (1981) got it to
//!   `2(N − 1)` by holding acknowledgements back until the release. Lamport
//!   notes the acknowledgement can be skipped when the receiver has already
//!   sent something later; this implementation always sends it.
//!
//! And "made first" means first by stamp, not by the clock on the wall: of two
//! concurrent requests, the process with the smaller id wins the tie, and if
//! causality flows outside the system (one user phones another), a request
//! made later in real time can still be granted first.

use crate::clock::{LamportClock, LamportStamp};
use std::collections::{BTreeMap, BTreeSet};

/// What a [`Message`] says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The sender wants the resource; the message's stamp is its request's.
    Request,
    /// The sender has queued the receiver's request.
    Ack,
    /// The sender is done with the resource.
    Release,
}

/// A message from one process to another, stamped with the sender's Lamport
/// clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message<P> {
    pub from: P,
    pub to: P,
    pub time: u64,
    pub kind: Kind,
}

/// One process's side of Lamport's mutual exclusion, among processes named by
/// `P`.
///
/// It does no I/O: [`request`](Self::request), [`receive`](Self::receive) and
/// [`release`](Self::release) return the messages to send, and the caller
/// delivers them, on channels that preserve order.
///
/// State:
/// - `queue`: every request this process knows is outstanding, its own
///   included, sorted by stamp.
/// - `heard`: for each other process, the latest stamp received from it.
#[derive(Debug, Clone)]
pub struct LamportMutex<P> {
    me: P,
    clock: LamportClock,
    queue: BTreeSet<LamportStamp<P>>,
    heard: BTreeMap<P, u64>,
    pending: Option<LamportStamp<P>>,
}

impl<P: Ord + Clone> LamportMutex<P> {
    /// Process `me`, sharing the resource with `peers`.
    pub fn new(me: P, peers: impl IntoIterator<Item = P>) -> Self {
        // Every request is stamped at least 1, so 0 is earlier than all of them:
        // it stands for having heard nothing yet.
        let heard = peers.into_iter().filter(|p| *p != me).map(|p| (p, 0)).collect();
        Self { me, clock: LamportClock::new(), queue: BTreeSet::new(), heard, pending: None }
    }

    pub fn me(&self) -> &P {
        &self.me
    }

    /// This process's clock, for tracing an execution.
    pub fn clock(&self) -> &LamportClock {
        &self.clock
    }

    /// Every outstanding request this process knows of, earliest first.
    pub fn queue(&self) -> impl Iterator<Item = &LamportStamp<P>> {
        self.queue.iter()
    }

    /// For each other process, the latest stamp received from it, 0 if
    /// nothing yet: what the second half of rule 5 compares against.
    pub fn heard(&self) -> impl Iterator<Item = (&P, u64)> {
        self.heard.iter().map(|(p, &time)| (p, time))
    }

    /// This process's own request, from [`request`](Self::request) until
    /// [`release`](Self::release).
    pub fn pending(&self) -> Option<&LamportStamp<P>> {
        self.pending.as_ref()
    }

    /// Rule 5: whether this process holds the resource. True from the moment
    /// both conditions hold until [`release`](Self::release): over FIFO
    /// channels, nothing a process can receive undoes them.
    pub fn holds(&self) -> bool {
        let Some(mine) = &self.pending else { return false };
        self.queue.first() == Some(mine)
            && self.heard.iter().all(|(p, &time)| LamportStamp { time, process: p.clone() } > *mine)
    }

    /// Rule 1: ask for the resource. Returns the request to send to every other
    /// process; it's granted once [`holds`](Self::holds) says so.
    ///
    /// Sending the request to everyone is one event, with one stamp.
    ///
    /// # Panics
    ///
    /// If this process already has a request outstanding.
    pub fn request(&mut self) -> Vec<Message<P>> {
        assert!(self.pending.is_none(), "already requested");
        let time = self.clock.send();
        let mine = LamportStamp { time, process: self.me.clone() };
        self.queue.insert(mine.clone());
        self.pending = Some(mine);
        self.broadcast(time, Kind::Request)
    }

    /// Rule 3: give the resource up. Returns the release to send to every other
    /// process.
    ///
    /// # Panics
    ///
    /// If this process doesn't hold the resource.
    pub fn release(&mut self) -> Vec<Message<P>> {
        assert!(self.holds(), "released without holding");
        let mine = self.pending.take().expect("a holder has a request");
        self.queue.remove(&mine);
        let time = self.clock.send();
        self.broadcast(time, Kind::Release)
    }

    /// Rules 2 and 4: handle a message from another process, returning the
    /// acknowledgement to send back if it was a request.
    ///
    /// Messages from each sender must arrive in the order it sent them.
    pub fn receive(&mut self, msg: Message<P>) -> Option<Message<P>> {
        debug_assert!(msg.to == self.me, "delivered to the wrong process");
        self.clock.receive(msg.time);
        let heard = self.heard.get_mut(&msg.from).expect("message from an unknown process");
        *heard = (*heard).max(msg.time);
        match msg.kind {
            Kind::Request => {
                self.queue.insert(LamportStamp { time: msg.time, process: msg.from.clone() });
                let time = self.clock.send();
                Some(Message { from: self.me.clone(), to: msg.from, time, kind: Kind::Ack })
            }
            Kind::Ack => None,
            Kind::Release => {
                // A process has at most one request outstanding, so this is it.
                self.queue.retain(|s| s.process != msg.from);
                None
            }
        }
    }

    fn broadcast(&self, time: u64, kind: Kind) -> Vec<Message<P>> {
        self.heard.keys().map(|p| Message { from: self.me.clone(), to: p.clone(), time, kind }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Rng;
    use std::collections::VecDeque;

    /// The message in `msgs` addressed to `to`.
    fn to(msgs: &[Message<char>], to: char) -> Message<char> {
        msgs.iter().find(|m| m.to == to).cloned().expect("no message for that process")
    }

    fn stamp(time: u64, process: char) -> LamportStamp<char> {
        LamportStamp { time, process }
    }

    /// A and B ask at once, with the same counter value. Both queues sort the
    /// requests the same way, the tie goes to A, and B waits for A's release.
    #[test]
    fn concurrent_requests_are_granted_in_stamp_order() {
        let mut a = LamportMutex::new('A', ['A', 'B']);
        let mut b = LamportMutex::new('B', ['A', 'B']);
        let req_a = a.request().pop().unwrap();
        let req_b = b.request().pop().unwrap();
        assert_eq!((req_a.time, req_b.time), (1, 1));

        let ack_from_b = b.receive(req_a).unwrap();
        let ack_from_a = a.receive(req_b).unwrap();
        a.receive(ack_from_b);
        b.receive(ack_from_a);

        assert_eq!(a.queue().collect::<Vec<_>>(), b.queue().collect::<Vec<_>>());
        assert_eq!(a.queue().next(), Some(&stamp(1, 'A')));
        assert!(a.holds());
        assert!(!b.holds()); // B has heard from A, but its request isn't first

        b.receive(to(&a.release(), 'B'));
        assert!(b.holds());
        assert_eq!(b.queue().collect::<Vec<_>>(), [&stamp(1, 'B')]);
    }

    /// A request alone in its own queue isn't enough: an earlier one could be
    /// in flight. A waits until it has heard from every other process past its
    /// request's stamp.
    #[test]
    fn a_requester_waits_to_hear_from_everyone() {
        let everyone = ['A', 'B', 'C'];
        let mut a = LamportMutex::new('A', everyone);
        let mut b = LamportMutex::new('B', everyone);
        let mut c = LamportMutex::new('C', everyone);
        let reqs = a.request();
        assert_eq!(a.queue().collect::<Vec<_>>(), [&stamp(1, 'A')]);
        assert!(!a.holds());

        a.receive(b.receive(to(&reqs, 'B')).unwrap());
        assert!(!a.holds()); // C could still have an earlier request on the way

        a.receive(c.receive(to(&reqs, 'C')).unwrap());
        assert!(a.holds());
    }

    /// The execution the FIFO assumption rules out. A's request to B is slow,
    /// and A's acknowledgement of B's request overtakes it. B has now heard
    /// from A past its own request, decides nothing earlier is coming, and
    /// enters alongside A, whose request was earlier.
    #[test]
    fn overtaking_breaks_mutual_exclusion() {
        let mut a = LamportMutex::new('A', ['A', 'B']);
        let mut b = LamportMutex::new('B', ['A', 'B']);

        let req_a = a.request().pop().unwrap(); // stamp (1, A), and slow to arrive
        let req_b = b.request().pop().unwrap(); // stamp (1, B), later than A's
        let a_acks_b = a.receive(req_b).unwrap();
        assert!(a.holds()); // rightly: A's request is the earliest

        b.receive(a_acks_b); // sent by A after its request, delivered before it
        assert_eq!(b.heard().collect::<Vec<_>>(), [(&'A', 3)]);
        assert!(b.holds());
        assert_eq!(b.queue().collect::<Vec<_>>(), [&stamp(1, 'B')]); // A's request is still in flight

        // With the request delivered, B's queue is right again, but too late.
        b.receive(req_a);
        assert!(!b.holds());
    }

    /// The algorithm's claims, on random executions over FIFO channels with
    /// arbitrary interleaving and delay:
    /// - at most one process holds the resource at any moment, and a holder
    ///   keeps it until it releases;
    /// - grants happen in stamp order;
    /// - with every holder eventually releasing, every request is granted;
    /// - each entry costs `3(N − 1)` messages.
    #[test]
    fn random_executions_are_exclusive_ordered_and_live() {
        const PROCS: usize = 4;
        for seed in 1..=300u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut procs: Vec<LamportMutex<usize>> = (0..PROCS).map(|p| LamportMutex::new(p, 0..PROCS)).collect();
            // channels[from][to], each delivered in order
            let mut channels: Vec<Vec<VecDeque<Message<usize>>>> = vec![vec![VecDeque::new(); PROCS]; PROCS];
            let mut sent = 0;
            let mut requests = 0;
            let mut grants: Vec<LamportStamp<usize>> = Vec::new();
            let mut holding: Option<usize> = None;

            let mut send = |channels: &mut Vec<Vec<VecDeque<Message<usize>>>>, msgs: Vec<Message<usize>>| {
                for m in msgs {
                    sent += 1;
                    channels[m.from][m.to].push_back(m);
                }
            };

            for step in 0.. {
                let draining = step >= 200;
                let nonempty: Vec<(usize, usize)> = (0..PROCS)
                    .flat_map(|f| (0..PROCS).map(move |t| (f, t)))
                    .filter(|&(f, t)| !channels[f][t].is_empty())
                    .collect();
                let idle: Vec<usize> = (0..PROCS).filter(|&p| procs[p].pending().is_none()).collect();
                if draining && nonempty.is_empty() && holding.is_none() {
                    assert!(idle.len() == PROCS, "seed {seed}: deadlock with requests outstanding");
                    break;
                }
                match rng.below(4) {
                    0 if !draining && !idle.is_empty() => {
                        let p = idle[rng.below(idle.len())];
                        requests += 1;
                        let msgs = procs[p].request();
                        send(&mut channels, msgs);
                    }
                    1 if holding.is_some() => {
                        let p = holding.take().unwrap();
                        let msgs = procs[p].release();
                        send(&mut channels, msgs);
                    }
                    _ if !nonempty.is_empty() => {
                        let (f, t) = nonempty[rng.below(nonempty.len())];
                        let m = channels[f][t].pop_front().unwrap();
                        let reply = procs[t].receive(m);
                        send(&mut channels, reply.into_iter().collect());
                    }
                    _ => continue,
                }

                let holders: Vec<usize> = (0..PROCS).filter(|&p| procs[p].holds()).collect();
                assert!(holders.len() <= 1, "seed {seed}: {holders:?} hold at once");
                match (holding, holders.first()) {
                    (Some(h), _) => assert_eq!(holders, [h], "seed {seed}: {h} lost the resource"),
                    (None, Some(&h)) => {
                        holding = Some(h);
                        grants.push(*procs[h].pending().unwrap());
                    }
                    (None, None) => {}
                }
            }

            assert!(requests > 0);
            assert_eq!(grants.len(), requests, "seed {seed}");
            assert!(grants.is_sorted(), "seed {seed}: granted out of order: {grants:?}");
            assert_eq!(sent, 3 * (PROCS - 1) * requests, "seed {seed}");
        }
    }
}
