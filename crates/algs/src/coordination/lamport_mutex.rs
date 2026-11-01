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
//!   (`overtaking_breaks_mutual_exclusion` builds that execution, and
//!   `overtaking_needs_four_steps_to_break_mutual_exclusion` shows nothing
//!   shorter does it).
//! - **No message loss, and no crashes.** A process enters only after hearing
//!   from *every* other process, so one that crashes or goes quiet blocks
//!   everyone, forever (`one_crash_leaves_a_request_waiting_forever`). A lost
//!   request can do worse: once a later message from its sender arrives, it's
//!   as if that message had overtaken the request, and two processes can
//!   enter together (`a_lost_request_lets_two_processes_hold_at_once`). The
//!   algorithm tolerates no failures at all. Lamport's
//!   [reliable state machine](crate::state_machine::bookkeeper) replaces
//!   hearing from everyone with votes from some, and deadlines, so it
//!   survives crashes and lines that stop, for a while or for good. What it
//!   still needs is that a line never drops a message and then delivers a
//!   later one.
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

/// A message from one process to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message<P: Ord> {
    pub to: P,
    /// The stamp of the send, whose process is the sender.
    pub stamp: LamportStamp<P>,
    pub kind: Kind,
}

/// One process's side of Lamport's mutual exclusion, among processes named by
/// `P`, which are ordered because the request queue is.
#[derive(Debug, Clone)]
pub struct LamportMutex<P: Ord> {
    /// Stamps this process's requests, acks and releases, and names this
    /// process.
    clock: LamportClock<P>,
    /// Every outstanding request this process knows of, its own included,
    /// earliest first.
    queue: BTreeSet<LamportStamp<P>>,
    /// For each other process, the latest time received from it, 0 before
    /// anything arrives: what the second half of rule 5 compares against.
    heard: BTreeMap<P, u64>,
    /// This process's own request, from [`request`](Self::request) until
    /// [`release`](Self::release).
    pending: Option<LamportStamp<P>>,
}

impl<P: Ord + Clone> LamportMutex<P> {
    /// Process `me`, sharing the resource with `peers`.
    pub fn new(me: P, peers: impl IntoIterator<Item = P>) -> Self {
        // Every request is stamped at least 1, so 0 is earlier than all of them:
        // it stands for having heard nothing yet.
        let heard = peers
            .into_iter()
            .filter(|p| *p != me)
            .map(|p| (p, 0))
            .collect();
        Self {
            clock: LamportClock::new(me),
            queue: BTreeSet::new(),
            heard,
            pending: None,
        }
    }

    pub fn me(&self) -> &P {
        self.clock.process()
    }

    pub fn clock(&self) -> &LamportClock<P> {
        &self.clock
    }

    /// Every outstanding request this process knows of, earliest first.
    pub fn queue(&self) -> impl Iterator<Item = &LamportStamp<P>> {
        self.queue.iter()
    }

    /// For each other process, the latest time received from it, 0 if
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
        let Some(mine) = &self.pending else {
            return false;
        };
        self.queue.first() == Some(mine)
            && self
                .heard
                .iter()
                .all(|(p, &time)| LamportStamp::new(time, p.clone()) > *mine)
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
        let mine = self.clock.tick();
        self.queue.insert(mine.clone());
        self.pending = Some(mine.clone());
        self.broadcast(mine, Kind::Request)
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
        let stamp = self.clock.tick();
        self.broadcast(stamp, Kind::Release)
    }

    /// Rules 2 and 4: handle a message from another process, returning the
    /// acknowledgement to send back if it was a request.
    ///
    /// Messages from each sender must arrive in the order it sent them.
    pub fn receive(&mut self, msg: Message<P>) -> Option<Message<P>> {
        debug_assert!(msg.to == *self.me(), "delivered to the wrong process");
        self.clock.receive(msg.stamp.time());
        let heard = self
            .heard
            .get_mut(msg.stamp.process())
            .expect("message from an unknown process");
        *heard = (*heard).max(msg.stamp.time());
        match msg.kind {
            Kind::Request => {
                let to = msg.stamp.process().clone();
                self.queue.insert(msg.stamp);
                Some(Message {
                    to,
                    stamp: self.clock.tick(),
                    kind: Kind::Ack,
                })
            }
            Kind::Ack => None,
            Kind::Release => {
                // A process has at most one request outstanding, so this is it.
                self.queue.retain(|s| s.process() != msg.stamp.process());
                None
            }
        }
    }

    fn broadcast(&self, stamp: LamportStamp<P>, kind: Kind) -> Vec<Message<P>> {
        self.heard
            .keys()
            .map(|p| Message {
                to: p.clone(),
                stamp: stamp.clone(),
                kind,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::VectorClock;
    use crate::test_util::{Execution, check, shortest_failure};
    use proptest::strategy::Strategy;
    use std::collections::VecDeque;

    /// The message in `msgs` addressed to `to`.
    fn to(msgs: &[Message<char>], to: char) -> Message<char> {
        msgs.iter()
            .find(|m| m.to == to)
            .cloned()
            .expect("no message for that process")
    }

    fn stamp(time: u64, process: char) -> LamportStamp<char> {
        LamportStamp::new(time, process)
    }

    /// A and B ask at once, with the same counter value. Both queues sort the
    /// requests the same way, the tie goes to A, and B waits for A's release.
    #[test]
    fn concurrent_requests_are_granted_in_stamp_order() {
        let mut a = LamportMutex::new('A', ['A', 'B']);
        let mut b = LamportMutex::new('B', ['A', 'B']);
        let req_a = a.request().pop().unwrap();
        let req_b = b.request().pop().unwrap();
        assert_eq!((req_a.stamp.time(), req_b.stamp.time()), (1, 1));

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

    /// Rule 5 compares stamps, so a tie on time goes by id. Here it goes
    /// against the sender: B's second request and A's second release are both
    /// at time 10, A's id is lower, so the release isn't stamped later than the
    /// request, and B waits for A's acknowledgement, which is. Waiting isn't
    /// needed for safety, since A has no request outstanding and anything it
    /// sends later is stamped past 10, so this pins the rule as stated.
    /// `overtaking_breaks_mutual_exclusion` has the other half: A enters on
    /// B's request, made at the same time by a higher id.
    #[test]
    fn a_tie_on_time_with_a_lower_id_is_not_heard_past() {
        let mut a = LamportMutex::new('A', ['A', 'B']);
        let mut b = LamportMutex::new('B', ['A', 'B']);
        let req_a1 = a.request().pop().unwrap(); // (1, A)
        let ack_b1 = b.receive(req_a1).unwrap(); // (3, B)
        let req_b1 = b.request().pop().unwrap(); // (4, B)
        a.receive(ack_b1);
        assert!(a.holds());
        let rel_a1 = a.release().pop().unwrap(); // (5, A)
        let req_a2 = a.request().pop().unwrap(); // (6, A)
        b.receive(rel_a1);
        assert!(b.holds());
        let rel_b1 = b.release().pop().unwrap(); // (7, B)
        let ack_b2 = b.receive(req_a2).unwrap(); // (9, B)
        let req_b2 = b.request().pop().unwrap(); // (10, B)
        let ack_a1 = a.receive(req_b1).unwrap(); // (8, A)
        b.receive(ack_a1);
        a.receive(rel_b1);
        assert!(a.holds());
        let rel_a2 = a.release().pop().unwrap();
        assert_eq!(rel_a2.stamp, stamp(10, 'A'));
        assert_eq!(req_b2.stamp, stamp(10, 'B'));

        b.receive(rel_a2);
        assert_eq!(b.heard().collect::<Vec<_>>(), [(&'A', 10)]);
        assert!(!b.holds()); // (10, A) is earlier than (10, B)
        a.receive(ack_b2);
        b.receive(a.receive(req_b2).unwrap());
        assert!(b.holds());
    }

    /// What can happen next in an execution.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Step {
        /// Process `p` asks for the resource, unless it already has.
        Request(usize),
        /// Whoever holds the resource gives it up.
        Release,
        /// The oldest message on its way from `from` to `to` arrives.
        Deliver { from: usize, to: usize },
        /// If messages can overtake, the newest message on its way from `from`
        /// to `to` arrives ahead of the others.
        Overtake { from: usize, to: usize },
        /// If messages can be lost, the oldest from `from` to `to` is.
        Lose { from: usize, to: usize },
        /// If processes can crash, `p` stops for good.
        Crash(usize),
    }

    /// An assumption from "What it needs", dropped.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Fault {
        Overtaking,
        Loss,
        Crashes,
    }

    /// Steps among `n` processes over FIFO channels: a quarter are requests,
    /// an eighth releases, and the rest deliveries.
    ///
    /// They're decoded from numbers rather than chosen with `prop_oneof!`,
    /// which is several times slower in debug builds. Each number shrinks
    /// toward 0, so a step shrinks toward a request by process 0.
    fn steps(n: usize) -> impl Strategy<Value = Step> {
        (0..8, 0..n, 0..n).prop_map(|(kind, p, to)| match kind {
            0 | 1 => Step::Request(p),
            2 => Step::Release,
            _ => Step::Deliver { from: p, to },
        })
    }

    /// Every step among `n` processes with `fault` allowed, in the order the
    /// shortest failing schedules are picked in.
    fn every_step(n: usize, fault: Fault) -> Vec<Step> {
        let channels = || {
            (0..n)
                .flat_map(move |from| (0..n).map(move |to| (from, to)))
                .filter(|(from, to)| from != to)
        };
        let mut every: Vec<Step> = (0..n).map(Step::Request).collect();
        every.push(Step::Release);
        every.extend(channels().map(|(from, to)| Step::Deliver { from, to }));
        match fault {
            Fault::Overtaking => {
                every.extend(channels().map(|(from, to)| Step::Overtake { from, to }))
            }
            Fault::Loss => every.extend(channels().map(|(from, to)| Step::Lose { from, to })),
            Fault::Crashes => every.extend((0..n).map(Step::Crash)),
        }
        every
    }

    /// A message on its way, with the vector clock of its send.
    type InFlight = (Message<usize>, VectorClock<usize>);

    /// `n` processes and the channels between them, with what the checks
    /// compare the processes against, recorded as things happen rather than
    /// read off the processes.
    struct Processes {
        procs: Vec<LamportMutex<usize>>,
        /// `channels[from][to]`, oldest first.
        channels: Vec<Vec<VecDeque<InFlight>>>,
        fault: Option<Fault>,
        crashed: Vec<bool>,
        holder: Option<usize>,
        /// The last request granted.
        granted: Option<LamportStamp<usize>>,
        /// For each process, a vector clock, which tracks happened-before
        /// without relying on stamps.
        clocks: Vec<VectorClock<usize>>,
        /// The requests not granted yet, each with the vector clock of its
        /// making.
        ungranted: BTreeMap<LamportStamp<usize>, VectorClock<usize>>,
        /// For each process, the requests it knows of and hasn't received the
        /// release of: what its queue should hold.
        known: Vec<BTreeSet<LamportStamp<usize>>>,
        /// For each process, the latest release it has received from each
        /// other process.
        released: Vec<BTreeMap<usize, LamportStamp<usize>>>,
        /// For each process, the latest time it has received from each other
        /// process.
        heard: Vec<BTreeMap<usize, u64>>,
        /// For each process, the latest stamp it has sent or received.
        seen: Vec<Option<LamportStamp<usize>>>,
    }

    impl Processes {
        fn new(n: usize, fault: Option<Fault>) -> Self {
            Self {
                procs: (0..n).map(|p| LamportMutex::new(p, 0..n)).collect(),
                channels: vec![vec![VecDeque::new(); n]; n],
                fault,
                crashed: vec![false; n],
                holder: None,
                granted: None,
                clocks: vec![VectorClock::new(); n],
                ungranted: BTreeMap::new(),
                known: vec![BTreeSet::new(); n],
                released: vec![BTreeMap::new(); n],
                heard: (0..n)
                    .map(|p| (0..n).filter(|&q| q != p).map(|q| (q, 0)).collect())
                    .collect(),
                seen: vec![None; n],
            }
        }

        /// The first channel with a message on it, if any.
        fn busy_channel(&self) -> Option<(usize, usize)> {
            let n = self.procs.len();
            (0..n)
                .flat_map(|from| (0..n).map(move |to| (from, to)))
                .find(|&(from, to)| !self.channels[from][to].is_empty())
        }

        /// Check that `msgs` is one message to each other process, of `kind`
        /// and stamped `stamp`, and send them.
        fn broadcast(
            &mut self,
            from: usize,
            stamp: LamportStamp<usize>,
            kind: Kind,
            mut msgs: Vec<Message<usize>>,
        ) {
            msgs.sort_by_key(|m| m.to);
            let expected: Vec<_> = (0..self.procs.len())
                .filter(|&to| to != from)
                .map(|to| Message { to, stamp, kind })
                .collect();
            assert_eq!(msgs, expected, "{from}'s {kind:?}");
            self.send(from, msgs);
        }

        /// Check that `from` stamped `msgs`, which share one stamp, later than
        /// anything it had sent or received, so that stamps never contradict
        /// happened-before, and put them on their channels. Nothing reaches a
        /// crashed process.
        fn send(&mut self, from: usize, msgs: Vec<Message<usize>>) {
            let Some(stamp) = msgs.first().map(|m| m.stamp) else {
                return;
            };
            let seen = self.seen[from];
            assert!(
                seen.is_none_or(|seen| stamp > seen),
                "{from} stamped {stamp:?} after seeing {seen:?}"
            );
            self.seen[from] = Some(stamp);
            let clock = self.clocks[from].send(&from);
            for m in msgs {
                if !self.crashed[m.to] {
                    self.channels[from][m.to].push_back((m, clock.clone()));
                }
            }
        }

        /// Hand `msg`, sent with `clock`, from `from` to `to`, checking that a
        /// request gets one acknowledgement, sent back, and nothing else gets
        /// an answer.
        fn deliver(
            &mut self,
            from: usize,
            to: usize,
            msg: Message<usize>,
            clock: VectorClock<usize>,
        ) {
            let (stamp, kind) = (msg.stamp, msg.kind);
            self.clocks[to].receive(&to, &clock);
            let heard = self.heard[to].get_mut(&from).unwrap();
            *heard = (*heard).max(stamp.time());
            self.seen[to] = self.seen[to].max(Some(stamp));
            match kind {
                Kind::Request => {
                    // Unless its release overtook it.
                    if self.released[to].get(&from).is_none_or(|r| stamp > *r) {
                        self.known[to].insert(stamp);
                    }
                }
                Kind::Release => {
                    // It ends the request its sender made before it.
                    self.known[to].retain(|r| *r.process() != from || *r > stamp);
                    let latest = self.released[to].entry(from).or_insert(stamp);
                    *latest = (*latest).max(stamp);
                }
                Kind::Ack => {}
            }
            let reply = self.procs[to].receive(msg);
            assert_eq!(
                reply.as_ref().map(|m| (m.to, m.kind)),
                (kind == Kind::Request).then_some((from, Kind::Ack)),
                "{to}'s answer to {from}'s {kind:?}"
            );
            self.send(to, reply.into_iter().collect());
        }

        /// Check that `h`'s request can be granted now: it's stamped later
        /// than the last request granted, and no request that happened before
        /// it is still waiting.
        fn grant(&mut self, h: usize) {
            let request = *self.procs[h].pending().unwrap();
            assert!(
                self.granted.is_none_or(|g| g < request),
                "{request:?} granted after {:?}",
                self.granted
            );
            let made = self.ungranted.remove(&request).unwrap();
            let before = self.ungranted.iter().find(|(_, clock)| **clock < made);
            assert!(
                before.is_none(),
                "{request:?} granted while {:?}, which happened before it, waits",
                before.map(|(r, _)| r)
            );
            (self.holder, self.granted) = (Some(h), Some(request));
        }

        /// The claims every step must keep.
        fn check_claims(&mut self) {
            let n = self.procs.len();
            let holders: Vec<usize> = (0..n).filter(|&p| self.procs[p].holds()).collect();
            assert!(holders.len() <= 1, "{holders:?} hold at once");
            match (self.holder, holders.first()) {
                (Some(h), _) => assert_eq!(holders, [h], "{h} lost the resource"),
                (None, Some(&h)) => self.grant(h),
                (None, None) => {}
            }

            for p in 0..n {
                let queue: Vec<_> = self.procs[p].queue().collect();
                let known: Vec<_> = self.known[p].iter().collect();
                assert_eq!(queue, known, "{p}'s queue");
                let heard: BTreeMap<_, _> = self.procs[p].heard().map(|(&q, t)| (q, t)).collect();
                assert_eq!(heard, self.heard[p], "what {p} heard");
            }

            // With nothing in flight, nothing changes until the holder
            // releases, so a request can be waiting only if someone holds the
            // resource and can release it.
            let waiting: Vec<usize> = (0..n)
                .filter(|&p| !self.crashed[p] && self.procs[p].pending().is_some())
                .collect();
            let releasing = self.holder.is_some_and(|h| !self.crashed[h]);
            assert!(
                self.busy_channel().is_some() || waiting.is_empty() || releasing,
                "{waiting:?} wait forever"
            );
        }
    }

    impl Execution for Processes {
        type Step = Step;

        fn step(&mut self, step: &Step) {
            match *step {
                Step::Request(p) if self.procs[p].pending().is_none() && !self.crashed[p] => {
                    let msgs = self.procs[p].request();
                    let mine = *self.procs[p].pending().unwrap();
                    self.known[p].insert(mine);
                    self.broadcast(p, mine, Kind::Request, msgs);
                    self.ungranted.insert(mine, self.clocks[p].clone());
                }
                Step::Release if self.holder.is_some_and(|h| !self.crashed[h]) => {
                    let h = self.holder.take().unwrap();
                    let mine = *self.procs[h].pending().unwrap();
                    let msgs = self.procs[h].release();
                    self.known[h].remove(&mine);
                    // A release's stamp is new, so this checks only that the
                    // messages share it; `send` checks it's later than
                    // anything `h` has seen.
                    self.broadcast(h, msgs[0].stamp, Kind::Release, msgs);
                }
                Step::Deliver { from, to } if !self.channels[from][to].is_empty() => {
                    let (msg, clock) = self.channels[from][to].pop_front().unwrap();
                    self.deliver(from, to, msg, clock);
                }
                Step::Overtake { from, to }
                    if self.fault == Some(Fault::Overtaking)
                        && self.channels[from][to].len() > 1 =>
                {
                    let (msg, clock) = self.channels[from][to].pop_back().unwrap();
                    self.deliver(from, to, msg, clock);
                }
                Step::Lose { from, to }
                    if self.fault == Some(Fault::Loss) && !self.channels[from][to].is_empty() =>
                {
                    self.channels[from][to].pop_front();
                }
                Step::Crash(p) if self.fault == Some(Fault::Crashes) && !self.crashed[p] => {
                    self.crashed[p] = true;
                    for channel in &mut self.channels {
                        channel[p].clear();
                    }
                }
                _ => return,
            }
            self.check_claims();
        }

        /// The holder releases and messages arrive, the oldest on the first
        /// busy channel first. Once nothing more can happen, no request is
        /// waiting, or `check_claims`'s last claim would have failed.
        fn drain(&self) -> Option<Step> {
            match self.holder {
                Some(h) if !self.crashed[h] => Some(Step::Release),
                _ => self
                    .busy_channel()
                    .map(|(from, to)| Step::Deliver { from, to }),
            }
        }
    }

    /// The algorithm's claims, on generated executions among two to five
    /// processes over FIFO channels, with any interleaving and delay:
    /// - at most one process holds the resource at any moment, and a holder
    ///   keeps it until it releases;
    /// - every message is stamped later than everything its sender had sent
    ///   or received, so stamps never contradict happened-before;
    /// - grants go in stamp order, and no request is granted while one that
    ///   happened before it waits, by vector clocks kept apart from the
    ///   stamps;
    /// - whenever nothing is in flight, a waiting request means someone holds
    ///   the resource, so with every holder releasing, every request is
    ///   granted;
    /// - each process's queue is every outstanding request it knows of, and
    ///   what it has heard is the latest time it received from each other
    ///   process;
    /// - each entry costs `3(N − 1)` messages: the request and the release go
    ///   to every other process, and each of them acknowledges the request.
    #[test]
    fn random_executions_are_exclusive_ordered_and_live() {
        for n in 2..=5 {
            check(|| Processes::new(n, None), steps(n));
        }
    }

    /// The shortest schedule between processes 0 and 1, A and B in the docs
    /// below, with `fault` allowed, whose first broken claim is `claim`, and
    /// the failure. Every schedule of up to six steps is tried.
    fn shortest(fault: Fault, claim: &str) -> (Vec<Step>, String) {
        let new = || Processes::new(2, Some(fault));
        shortest_failure(new, &every_step(2, fault), 6, claim).expect("none of them broke it")
    }

    /// The execution `overtaking_breaks_mutual_exclusion` builds is the
    /// shortest way to break mutual exclusion with messages free to overtake:
    /// both request, A receives B's request and acknowledges it, and the
    /// acknowledgement reaches B ahead of A's request. Nothing in three steps
    /// does it.
    #[test]
    fn overtaking_needs_four_steps_to_break_mutual_exclusion() {
        let (schedule, failure) = shortest(Fault::Overtaking, "hold at once");
        assert_eq!(
            schedule,
            [
                Step::Request(0),
                Step::Request(1),
                Step::Deliver { from: 1, to: 0 },
                Step::Overtake { from: 0, to: 1 },
            ]
        );
        assert_eq!(failure, "[0, 1] hold at once");
    }

    /// Losing a message can break mutual exclusion too, not only liveness.
    /// The shortest way: both request, A receives B's request and
    /// acknowledges it, and A's request to B is lost. The acknowledgement
    /// reaches B, which has now heard from A past its own request, as if the
    /// acknowledgement had overtaken A's request.
    #[test]
    fn a_lost_request_lets_two_processes_hold_at_once() {
        let (schedule, failure) = shortest(Fault::Loss, "hold at once");
        assert_eq!(
            schedule,
            [
                Step::Request(0),
                Step::Request(1),
                Step::Deliver { from: 1, to: 0 },
                Step::Lose { from: 0, to: 1 },
                Step::Deliver { from: 0, to: 1 },
            ]
        );
        assert_eq!(failure, "[0, 1] hold at once");
    }

    /// The no-loss assumption is needed for liveness too. Losing one request
    /// can leave its process stuck: nothing is in flight, nobody holds the
    /// resource, and it still waits.
    #[test]
    fn one_lost_message_leaves_a_request_waiting_forever() {
        let (schedule, failure) = shortest(Fault::Loss, "wait forever");
        assert_eq!(schedule, [Step::Request(0), Step::Lose { from: 0, to: 1 }]);
        assert_eq!(failure, "[0] wait forever");
    }

    /// The no-crash assumption is needed. One crash can leave a request
    /// waiting forever: the crashed process never hears it, so never
    /// acknowledges it.
    #[test]
    fn one_crash_leaves_a_request_waiting_forever() {
        let (schedule, failure) = shortest(Fault::Crashes, "wait forever");
        assert_eq!(schedule, [Step::Request(0), Step::Crash(1)]);
        assert_eq!(failure, "[0] wait forever");
    }
}
