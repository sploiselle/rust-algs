//! # Causal broadcast (CBCAST)
//!
//! The protocol from Birman, Schiper and Stephenson, *Lightweight Causal and
//! Atomic Group Multicast* (1991), which carried most of the ISIS toolkit's
//! traffic. Every process broadcasts to every other, and the protocol sits
//! between the network and the application: it *receives* messages in whatever
//! order the network hands them over, and *delivers* them to the application.
//! The promise is:
//!
//! ```text
//!     m → m'   ⇒   every process delivers m before m'
//! ```
//!
//! where `m → m'` (`m` happened before `m'`) means the sender of `m'` had sent
//! or delivered `m` before sending `m'`, directly or through a chain of such
//! steps. If A asks a question and B, having delivered it, answers, nobody
//! delivers the answer before the question.
//!
//! FIFO channels don't give that. The question travels from A to C and the
//! answer from B to C, on different channels, so nothing stops the answer
//! arriving first. C has to delay the answer until the question arrives.
//!
//! Concurrent messages, where neither happened before the other, get no order
//! between them. Each is delivered as soon as everything before it has been,
//! so one that has to wait can be overtaken by a concurrent one that arrived
//! after it (`a_delayed_message_can_be_overtaken_by_a_concurrent_one`), and
//! different processes can deliver them in different orders.
//!
//! ## The algorithm
//!
//! Every process keeps a vector `VT` that counts broadcasts: entry `k` is how
//! many of `k`'s broadcasts it has delivered.
//!
//! 1. **Broadcast**: add one to your own entry, stamp the message with the
//!    vector, and send it to everyone else. You deliver your own message as you
//!    send it.
//! 2. **Receive** a message from `i` stamped `V`: delay it until it's the next
//!    one from `i` (`V[i] = VT[i] + 1`) and you've delivered everything its
//!    sender had delivered when it sent it (`V[k] ≤ VT[k]` for every other
//!    `k`).
//! 3. **Deliver**: `VT = max(VT, V)` pointwise, which by rule 2 just adds one to
//!    entry `i`. Then deliver any delayed message that's now ready.
//!
//! A stamp is an exact summary of its message's causal past: `V[k]` is how many
//! of `k`'s broadcasts happened before the message, counting the message
//! itself for its sender. A process's broadcasts happen one after another, so
//! those are `k`'s first `V[k]` broadcasts. Rule 2 delivers each sender's
//! broadcasts in order, so `VT[k] ≥ V[k]` means all of them have been delivered
//! here. So rule 2 delays a message until its whole past is delivered, and no
//! longer: a message waits only if it arrived before something it depends on.
//!
//! `VT` is the paper's vector time (its Section 4.3), which counts messages
//! rather than events. A broadcast ticks it, as [`VectorClock::send`] does, and
//! a delivery merges into it without ticking, where [`VectorClock::receive`]
//! would tick. Ticking on deliveries would leave gaps between a sender's
//! stamps, and `V[i] = VT[i] + 1` couldn't tell the next message from one after
//! a message still in flight. And it merges on delivery, not receipt, because
//! it's what rule 2 checks a message against. Merging a delayed message on
//! receipt would count it as delivered: it would never be the next one from its
//! sender, and the messages that depend on it would get through ahead of it.
//!
//! ## What it needs
//!
//! - **Every broadcast reaches every process, once.** A message lost on its way
//!   to one process delays there, forever, everything that happened after it:
//!   the rest of its sender's broadcasts, and everything anyone broadcast after
//!   delivering it (`a_lost_message_holds_back_everything_after_it`). A sender
//!   that crashes partway through a broadcast, leaving some processes with it
//!   and some without, does the same. A second copy of a message would wait
//!   forever too: once the first is delivered, it's never the next one from
//!   its sender.
//! - **All communication goes through these broadcasts.** A stamp counts only
//!   what its sender had delivered. If B hears the question some other way, by
//!   a direct message or through a shared database, its answer's stamp doesn't
//!   count the question, and C can deliver the answer first
//!   (`a_question_heard_another_way_doesnt_hold_back_the_answer`). [Lamport's
//!   mutex](crate::coordination::lamport_mutex) has the same limit.
//! - **A fixed group.** Everyone broadcasts to the same processes, which never
//!   change. This module is the paper's Section 5.1; the rest of the paper lets
//!   processes join, leave and fail, by changing the group's membership in step
//!   with its messages (virtual synchrony), and lets a process belong to several
//!   groups.
//! - **A vector on every message**, with an entry for each process. This module
//!   sends the whole vector every time. The paper's implementation does better:
//!   with bursty traffic, it reports, most messages carry nothing beyond what
//!   its reliable FIFO channels need.
//!
//! It doesn't need FIFO channels, though the paper assumes them: rule 2 already
//! delivers each sender's broadcasts in the order it sent them, however they
//! arrive (`a_senders_broadcasts_are_delivered_in_order`). That's unlike
//! Lamport's mutex, which lets two processes in when one message overtakes
//! another.
//!
//! And it gives no total order, so replicas that apply operations as they're
//! delivered agree only if concurrent operations commute
//! (`concurrent_broadcasts_can_be_delivered_in_different_orders`). The paper's
//! ABCAST builds a total order on CBCAST, with a token holder that broadcasts
//! the order to deliver concurrent messages in.

use crate::clock::VectorClock;

/// A broadcast on its way: its sender sends a copy to every other process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message<P: Ord, T> {
    pub sender: P,
    /// For each process, how many of its broadcasts happened before this
    /// message, counting the message itself for its sender. It's the sender's
    /// vector when it sent.
    pub stamp: VectorClock<P>,
    pub payload: T,
}

/// One process's side of causal broadcast, among processes named by `P`,
/// broadcasting payloads `T`.
///
/// Whatever order messages arrive in, it delivers them in an order that never
/// contradicts happened-before, and delays a message only until everything that
/// happened before it has been delivered.
#[derive(Debug, Clone)]
pub struct Cbcast<P: Ord, T> {
    me: P,
    /// How many of each process's broadcasts this process has delivered, its
    /// own included: the paper's `VT`.
    delivered: VectorClock<P>,
    /// Messages received but not yet deliverable, in the order they arrived.
    /// The paper sorts its delay queue by stamp, but nothing in its Section 5.1
    /// depends on the order.
    delayed: Vec<Message<P, T>>,
}

impl<P: Ord + Clone, T> Cbcast<P, T> {
    pub fn new(me: P) -> Self {
        Self {
            me,
            delivered: VectorClock::new(),
            delayed: Vec::new(),
        }
    }

    pub fn me(&self) -> &P {
        &self.me
    }

    /// How many of each process's broadcasts this process has delivered, its
    /// own included.
    pub fn delivered(&self) -> &VectorClock<P> {
        &self.delivered
    }

    /// Messages received but not yet deliverable, in the order they arrived.
    pub fn delayed(&self) -> impl Iterator<Item = &Message<P, T>> {
        self.delayed.iter()
    }

    /// Rule 1: broadcast `payload`. Returns the message to send to every other
    /// process; this process has delivered it already.
    pub fn broadcast(&mut self, payload: T) -> Message<P, T> {
        Message {
            sender: self.me.clone(),
            stamp: self.delivered.send(&self.me),
            payload,
        }
    }

    /// Rule 2: whether `msg` can be delivered here now. It must be the next
    /// message from its sender, and everything its sender had delivered when it
    /// sent it must have been delivered here.
    pub fn deliverable(&self, msg: &Message<P, T>) -> bool {
        let from = &msg.sender;
        msg.stamp.get(from) == self.delivered.get(from) + 1
            && msg
                .stamp
                .iter()
                .all(|(p, n)| p == from || n <= self.delivered.get(p))
    }

    /// Rules 2 and 3: take a message from another process, and return every
    /// message its arrival lets this process deliver, in the order it delivers
    /// them: `msg` itself, if it's deliverable, then the delayed messages that
    /// were waiting on it, each time the earliest to arrive of those now ready.
    /// Nothing, if `msg` has to wait.
    ///
    /// Messages can arrive in any order, but each must arrive once.
    pub fn receive(&mut self, msg: Message<P, T>) -> Vec<Message<P, T>> {
        debug_assert!(msg.sender != self.me, "received its own broadcast");
        let n = msg.stamp.get(&msg.sender);
        debug_assert!(
            n > self.delivered.get(&msg.sender)
                && !self
                    .delayed
                    .iter()
                    .any(|m| m.sender == msg.sender && m.stamp.get(&m.sender) == n),
            "received twice"
        );
        self.delayed.push(msg);
        let mut out = Vec::new();
        while let Some(i) = self.delayed.iter().position(|m| self.deliverable(m)) {
            let msg = self.delayed.remove(i);
            self.delivered.merge(&msg.stamp);
            out.push(msg);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Rng;
    use std::collections::BTreeSet;

    fn vc<const N: usize>(pairs: [(char, u64); N]) -> VectorClock<char> {
        pairs.into_iter().collect()
    }

    fn payloads<T: Clone>(msgs: &[Message<char, T>]) -> Vec<T> {
        msgs.iter().map(|m| m.payload.clone()).collect()
    }

    /// The paper's Figure 2. A asks a question, and B delivers it and answers.
    /// The answer reaches C first, and C delays it until the question arrives.
    #[test]
    fn an_answer_waits_for_its_question() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let mut c = Cbcast::new('C');

        let question = a.broadcast("question");
        assert_eq!(payloads(&b.receive(question.clone())), ["question"]);
        let answer = b.broadcast("answer");
        assert_eq!(question.stamp, vc([('A', 1)]));
        assert_eq!(answer.stamp, vc([('A', 1), ('B', 1)]));
        assert_eq!(payloads(&a.receive(answer.clone())), ["answer"]);

        assert!(c.receive(answer.clone()).is_empty()); // C hasn't delivered A's first broadcast
        assert_eq!(c.delayed().collect::<Vec<_>>(), [&answer]);
        assert_eq!(c.receive(question.clone()), [question, answer]);
        assert_eq!(c.delayed().count(), 0);
        assert!(
            [a.delivered(), b.delivered()]
                .iter()
                .all(|v| *v == c.delivered())
        );
    }

    /// B's broadcast depends on A's, and waits for it at D. C's is concurrent
    /// with B's, and arrives after it, but waits for nothing, so D delivers it
    /// first.
    #[test]
    fn a_delayed_message_can_be_overtaken_by_a_concurrent_one() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let mut c = Cbcast::new('C');
        let mut d = Cbcast::new('D');
        let from_a = a.broadcast("from A");
        b.receive(from_a.clone());
        let from_b = b.broadcast("from B");
        let from_c = c.broadcast("from C");
        assert!(from_b.stamp.concurrent_with(&from_c.stamp));

        assert!(d.receive(from_b).is_empty());
        assert_eq!(payloads(&d.receive(from_c)), ["from C"]);
        assert_eq!(payloads(&d.receive(from_a)), ["from A", "from B"]);
    }

    /// The protocol sees only the causality its broadcasts carry. B hears A's
    /// question some other way and answers without having delivered it, so the
    /// answer's stamp doesn't count the question, and C delivers the answer
    /// first.
    #[test]
    fn a_question_heard_another_way_doesnt_hold_back_the_answer() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let mut c = Cbcast::new('C');
        let question = a.broadcast("question");
        let answer = b.broadcast("answer");
        assert!(question.stamp.concurrent_with(&answer.stamp));

        assert_eq!(payloads(&c.receive(answer)), ["answer"]);
        assert_eq!(payloads(&c.receive(question)), ["question"]);
    }

    /// No FIFO channels needed. A broadcasts twice, the second overtakes the
    /// first on the way to B, and B delays it: it isn't the next one from A.
    #[test]
    fn a_senders_broadcasts_are_delivered_in_order() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let first = a.broadcast(1);
        let second = a.broadcast(2);
        assert!(b.receive(second.clone()).is_empty());
        assert_eq!(b.receive(first.clone()), [first, second]);
    }

    /// Rule 2's first half: a message is deliverable as the next one from its
    /// sender, not before and not again.
    #[test]
    fn only_the_next_message_from_a_sender_is_deliverable() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let first = a.broadcast(1);
        let second = a.broadcast(2);
        assert!(!a.deliverable(&first)); // A delivered both as it sent them
        assert!(b.deliverable(&first));
        assert!(!b.deliverable(&second));

        b.receive(first.clone());
        assert!(!b.deliverable(&first));
        assert!(b.deliverable(&second));
    }

    /// Delayed messages stay in the order they arrived, and of those ready at
    /// once, the earliest to arrive is delivered first.
    #[test]
    fn delayed_messages_keep_their_arrival_order() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let mut c = Cbcast::new('C');
        let mut d = Cbcast::new('D');
        let a1 = a.broadcast("a1");
        let a2 = a.broadcast("a2");
        let c1 = c.broadcast("c1");
        let c2 = c.broadcast("c2");
        let c3 = c.broadcast("c3");
        b.receive(c1.clone());
        let b1 = b.broadcast("b1");

        for m in [&a2, &c2, &c3, &b1] {
            assert!(d.receive(m.clone()).is_empty());
        }
        assert_eq!(payloads(&d.receive(a1)), ["a1", "a2"]);
        assert_eq!(d.delayed().collect::<Vec<_>>(), [&c2, &c3, &b1]);
        // All three wait only on c1, and b1, concurrent with c2 and c3, arrived last.
        assert!(b1.stamp.concurrent_with(&c2.stamp));
        assert_eq!(payloads(&d.receive(c1)), ["c1", "c2", "c3", "b1"]);
    }

    /// A's first broadcast is lost on its way to C. C delays, forever, A's next
    /// broadcast and the one B made after delivering the lost one, while B's
    /// earlier broadcast, which doesn't depend on it, still gets through.
    #[test]
    fn a_lost_message_holds_back_everything_after_it() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let mut c = Cbcast::new('C');

        let b_before = b.broadcast("B before");
        let lost = a.broadcast("lost");
        b.receive(lost); // and never reaches C
        let b_after = b.broadcast("B after");
        let a_after = a.broadcast("A after");

        assert!(c.receive(b_after.clone()).is_empty());
        assert!(c.receive(a_after.clone()).is_empty());
        assert_eq!(c.receive(b_before.clone()), [b_before]);
        assert_eq!(c.delayed().collect::<Vec<_>>(), [&b_after, &a_after]);
        assert_eq!(c.delivered(), &vc([('B', 1)]));
    }

    /// A copy of a message that's still waiting would wait forever, so debug
    /// builds refuse it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "received twice")]
    fn a_copy_of_a_waiting_message_is_refused() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        a.broadcast(1);
        let second = a.broadcast(2);
        b.receive(second.clone());
        b.receive(second);
    }

    /// A and B each set `x` without having heard from the other. C and D get
    /// the two in opposite orders and deliver each as it comes, so a register
    /// applying them in delivery order ends at 2 on C and 1 on D.
    #[test]
    fn concurrent_broadcasts_can_be_delivered_in_different_orders() {
        let mut a = Cbcast::new('A');
        let mut b = Cbcast::new('B');
        let mut c = Cbcast::new('C');
        let mut d = Cbcast::new('D');
        let from_a = a.broadcast("x = 1");
        let from_b = b.broadcast("x = 2");
        assert!(from_a.stamp.concurrent_with(&from_b.stamp));

        let mut at_c = c.receive(from_a.clone());
        at_c.extend(c.receive(from_b.clone()));
        let mut at_d = d.receive(from_b);
        at_d.extend(d.receive(from_a));

        assert_eq!(payloads(&at_c), ["x = 1", "x = 2"]);
        assert_eq!(payloads(&at_d), ["x = 2", "x = 1"]);
        assert_eq!(c.delivered(), d.delivered()); // the same messages, in different orders
    }

    /// The module's claims, on random executions with arbitrary delay and
    /// reordering (no FIFO channels):
    /// - a stamp counts, for each process, its broadcasts that happened before
    ///   the message, the message included;
    /// - a process delivers a message only after everything that happened
    ///   before it, and delays it no longer than that;
    /// - once everything has arrived, every process has delivered every
    ///   message, once.
    #[test]
    fn random_executions_deliver_in_causal_order_without_needless_delay() {
        const PROCS: usize = 4;
        let mut delays = 0;
        for seed in 1..=300u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut procs: Vec<Cbcast<usize, usize>> = (0..PROCS).map(Cbcast::new).collect();
            // Copies on their way, with their destinations, arriving in any order.
            let mut in_flight: Vec<(usize, Message<usize, usize>)> = Vec::new();
            // Each broadcast, by payload: who sent it, and the broadcasts that
            // happened before it, worked out from deliveries rather than stamps.
            let mut sender: Vec<usize> = Vec::new();
            let mut past: Vec<BTreeSet<usize>> = Vec::new();
            // What each process has delivered, by payload.
            let mut delivered: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); PROCS];

            for step in 0.. {
                let sending = step < 200;
                if !sending && in_flight.is_empty() {
                    break;
                }
                if sending && (in_flight.is_empty() || rng.below(3) == 0) {
                    let p = rng.below(PROCS);
                    let id = past.len();
                    let before = delivered[p]
                        .iter()
                        .flat_map(|&m| past[m].iter().copied().chain([m]))
                        .collect();
                    let msg = procs[p].broadcast(id);
                    sender.push(p);
                    past.push(before);
                    for k in 0..PROCS {
                        let count = past[id]
                            .iter()
                            .chain([&id])
                            .filter(|&&m| sender[m] == k)
                            .count();
                        assert_eq!(msg.stamp.get(&k), count as u64, "seed {seed}: {id}[{k}]");
                    }
                    delivered[p].insert(id);
                    in_flight.extend((0..PROCS).filter(|&q| q != p).map(|q| (q, msg.clone())));
                } else {
                    let (q, msg) = in_flight.swap_remove(rng.below(in_flight.len()));
                    for m in procs[q].receive(msg) {
                        let m = m.payload;
                        assert!(
                            past[m].is_subset(&delivered[q]),
                            "seed {seed}: {q} delivered {m} too early"
                        );
                        assert!(
                            delivered[q].insert(m),
                            "seed {seed}: {q} delivered {m} twice"
                        );
                    }
                    for m in procs[q].delayed() {
                        assert!(
                            !past[m.payload].is_subset(&delivered[q]),
                            "seed {seed}: {q} delays {} needlessly",
                            m.payload
                        );
                        delays += 1;
                    }
                }
            }

            let sent: VectorClock<usize> = (0..PROCS)
                .map(|p| (p, sender.iter().filter(|&&s| s == p).count() as u64))
                .collect();
            for (p, proc) in procs.iter().enumerate() {
                assert_eq!(
                    delivered[p].len(),
                    past.len(),
                    "seed {seed}: {p} missed some"
                );
                assert_eq!(proc.delivered(), &sent, "seed {seed}");
                assert_eq!(proc.delayed().count(), 0, "seed {seed}");
            }
        }
        assert!(delays > 0, "no execution delayed anything");
    }
}
