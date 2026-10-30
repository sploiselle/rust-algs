//! # Lamport's reliable state machine
//!
//! The algorithm from Lamport, *The Implementation of Reliable Distributed
//! Multiprocess Systems* (1978): a way to run any system on a network of
//! processes so that it keeps working while some of the processes, or the
//! lines between them, have failed.
//!
//! The system is specified as a [`UserMachine`], which executes commands one
//! at a time. Several processes, the paper's *bookkeepers*, each keep a copy,
//! and the copies stay identical as long as every bookkeeper executes the
//! same commands in the same order. [Lamport's mutual
//! exclusion](crate::coordination::lamport_mutex) does that for one machine, a
//! queue of requests, and shows the catch: a process acts on a request only
//! once it has heard from every other process, so one crash stops everyone.
//! Here a bookkeeper needs to hear from only some of the others, and time
//! stands in for the rest: a bookkeeper that hasn't voted for a command by a
//! deadline has voted against it.
//!
//! ## Commands in order
//!
//! A user stamps each command with the time it issues it, which the paper
//! writes `T:C` (a [`Stamped`] command). Assume for now that clocks are
//! perfect, reading the same everywhere at once, as the paper's Section 2
//! does. Then a command issued before another has the smaller stamp, so
//! sorting by stamp keeps commands in the order they were issued. Commands
//! issued at the same time sort by command. That's the order every copy
//! executes them in.
//!
//! `T:C` is also what makes a command the same command. A user may send it to
//! several bookkeepers, and it's still executed once. By the same token, two
//! users who issue the same command at the same time issue one command, so a
//! command that must not merge with another user's has to say who issued it.
//!
//! Executing `T:C` takes knowing every command stamped before it that will
//! ever be executed, and with lines down, a bookkeeper may never hear of some
//! of them. So a command isn't executed just because it was issued: it must be
//! *accepted*, by a vote that closes a fixed time after its stamp. A command
//! that doesn't collect enough votes by then, because it reached the
//! bookkeepers late or reached only ones cut off from the rest, is
//! *rejected*, and every bookkeeper leaves it out.
//!
//! ## Voting
//!
//! A *majority graph* is a set of bookkeepers, with the lines between them,
//! chosen in advance so that any two share a member: with three bookkeepers,
//! each pair. A command is accepted once every bookkeeper in some majority
//! graph has voted to accept it. Sharing a member is what stops two groups
//! that can't hear each other from running the system separately: whatever
//! one majority graph accepts, a member of every other has voted for.
//! Messages from users to bookkeepers, and between bookkeepers, take less
//! than `δ` when the line works. With three bookkeepers the rules are:
//!
//! - **AR1(a)**: a bookkeeper that receives `T:C` from a user before `T + δ`
//!   votes to accept it, and tells the others.
//! - **AR1(b)**: a bookkeeper told of a vote for `T:C` before `T + 2δ` votes
//!   to accept it too, and tells the others. Two votes are a majority graph,
//!   so that's notice that `T:C` is accepted.
//! - **AR2**: from `T + 2δ` on, a bookkeeper never votes to accept a command
//!   stamped `T` or earlier, so it's voting to reject every such command it
//!   hasn't voted for, including any it never heard of. It says so (a) as
//!   soon as it can for any command it knows was issued, and (b) before
//!   `T + τ`, even when it has nothing else to say. `τ` is a parameter: a
//!   longer one means fewer messages while the system is idle, but a longer
//!   wait before it recovers when bookkeepers fail and come back. It must be
//!   longer than `2δ`; the paper asks for at least `3δ`, only to keep its
//!   bounds simple to state ([`Config::new`]).
//! - **AR3**: a bookkeeper that knows every accepted command stamped up to
//!   some time says so. The paper has it say so at once. Here it says so in
//!   its next message, and sends one at once only when it has just decided
//!   commands it knew of, so that an idle system doesn't trade messages
//!   forever.
//!
//! The paper's implementation (Section 2.4) sends these over lines that
//! deliver messages in the order they were sent, and so does this one. Then
//! saying it rejects (AR2) takes nothing but the time a message is sent: by
//! then the sender has sent every vote it will ever cast for the commands
//! that message closes ([`Message::sent`]). What it knows (AR3) is a time of
//! its own: it has already sent notice of every accepted command stamped up
//! to then ([`Message::known`]). Both are the paper's.
//!
//! A bookkeeper executes `T:C` once it knows every accepted command stamped up
//! to `T`. A command is accepted once its voters include a majority graph,
//! and rejected once the bookkeepers whose voting on it has closed, and who
//! didn't vote for it, meet every majority graph: no majority graph can vote
//! for it any more. That covers commands nobody mentioned, too: once the
//! bookkeepers whose voting has closed up to `T` meet every majority graph,
//! any command stamped up to `T` that none of them voted for is rejected,
//! whether or not anyone ever heard of it. With three bookkeepers, any two
//! that have closed up to `T` decide everything stamped up to `T`, except a
//! command one of them voted for and the other didn't, which waits on the
//! third's vote. A bookkeeper that can't work something out this way can
//! take the word of one that has (AR3).
//!
//! ## What it guarantees
//!
//! Suppose some majority graph `m` has worked properly since the system
//! started: its bookkeepers have kept running, and messages between them
//! have taken less than `δ`. A vote by any member of `m` then reaches the
//! others in time for them to vote too, unless it already completes a
//! majority graph, so every command one of its members hears of in time is
//! accepted. Every accepted command has a vote from `m`, since the majority
//! graph that accepted it shares a member with `m`. And the members hear each
//! other's rejections, so they decide everything else. That gives the paper's
//! correctness condition (RC):
//!
//! - a command that reaches a member of `m` before `T + δ` is accepted, and
//! - every accepted command is executed by every member of `m` before
//!   `T + 4δ` (with three bookkeepers).
//!
//! The slowest case is a command accepted by another majority graph. Say `m`
//! is {B, C}, and A and B accept the command. B's vote completes {A, B}, so B
//! asks nobody else to vote, and C hears of the command only from B's
//! notice, before `T + 3δ`. Only then does C say it rejects everything else
//! stamped up to `T`, and that reaches B before `T + 4δ`. That's the paper's
//! bound of `2δ + 2ε`, `ε` being how long a message takes to reach the rest
//! of a working majority graph once it's sent: `δ` here, since each
//! bookkeeper tells every other directly.
//!
//! Every command, accepted or not, is decided by every member of `m` before
//! `T + τ + δ`, since AR2(b) has each member say what it rejects before
//! `T + τ`.
//!
//! Nor need `m` stay the same. Suppose that ever since the system started,
//! some majority graph has been working properly, each one staying up for
//! `Ω = τ + 2ε` (`τ + 2δ` here) after the next one starts. Then RC holds for
//! every command stamped more than `Ω` after the latest one started: its
//! members catch up on everything before then from the bookkeepers they share
//! with the one before, whether or not they can hear from everyone else
//! (AR3). Once there has been a time with no majority graph working, though,
//! RC promises nothing more, even after one works again.
//!
//! None of this needs a bookkeeper to know which components are working, or
//! even whether it is.
//!
//! ## More bookkeepers
//!
//! The paper works the algorithm out in detail for three bookkeepers and
//! sketches the general case (Section 3.1). With more, a second vote needn't
//! complete a majority graph, so voting goes on in a *cascade*: a bookkeeper
//! told before `T + (k + 1)δ` that `k` bookkeepers have voted for `T:C` votes
//! too and passes on the `k + 1` names, until they include a majority graph.
//! A user's command is the cascade's first message, with no votes yet, which
//! is why its deadline is `T + δ`. The paper leaves the cascade's shape to
//! each network, and lets each line have its own `δ`. Here every bookkeeper
//! can message every other directly, with the same `δ` on every line, so
//! every vote goes to everyone, and a majority graph is just a set of
//! bookkeepers. Any family of sets in which every two meet will do
//! ([`Config`]). Voting on a command stamped `T` closes by `T + D`, where `D`
//! is `2δ` for three bookkeepers and `(s + 1)δ` in general, `s` being the
//! most bookkeepers that include no majority graph ([`Config::window`]).
//!
//! The paper proves nothing for this case, so the bounds are this
//! implementation's, worked out the same way and checked by its random tests:
//! RC's becomes `T + D + 2δ`, every command is still decided before
//! `T + τ + δ`, and `Ω` is still `τ + 2δ`.
//!
//! ## What it needs
//!
//! - **Lines that deliver in order, without gaps.** A bookkeeper infers
//!   rejections from what it hasn't been told. A line may hold messages, or
//!   stop for good (`a_bookkeeper_learns_what_it_cant_work_out_from_another`),
//!   but it mustn't skip one: a message that overtakes an earlier vote, or a
//!   vote lost while later messages get through, makes the receiver count a
//!   bookkeeper as rejecting a command it voted for
//!   (`overtaking_breaks_agreement`). To keep its implementation simple, the
//!   paper's Section 2.4 assumes lines deliver in order and every message
//!   eventually arrives, its processes resending until it does.
//! - **Clocks that never run backwards.** A bookkeeper whose clock jumps back
//!   can vote for a command it has already rejected
//!   (`a_clock_running_backwards_breaks_agreement`); the paper counts that as
//!   a malfunction (Section 3.2.4). Clocks needn't agree for
//!   the copies to agree: each bookkeeper checks every deadline, and stamps
//!   every message, on its own clock
//!   (`skewed_clocks_cost_progress_not_agreement`). They need to agree for
//!   commands to be accepted and executed in time: with real clocks, the
//!   paper replaces `δ` by a bound that covers their drift (Section 3.2.2).
//! - **Failures, not malfunctions.** A bookkeeper or a line may stop, and
//!   resume where it left off. One that does the wrong thing, such as
//!   reporting a vote nobody cast, is the paper's Goal 2 (Section 4.3). That
//!   takes more than votes that can't be forged: every vote must travel as
//!   itself, never summarized, so no [`Message::known`] and no lists of
//!   names, and users must check commands and responses redundantly. None of
//!   it is implemented here.
//! - **Enough of the network.** Once no majority graph is working, the
//!   bookkeepers may wait indefinitely, even after one works again. If one
//!   bookkeeper voted to accept a command and another let its voting close
//!   without doing so, and the third can't be reached, neither can tell
//!   whether the command was accepted, so neither can execute anything
//!   stamped later until it can (`a_split_vote_waits_for_the_third_bookkeeper`,
//!   the paper's Section 4.4). The paper's remedy is *auditors*: users that
//!   issue checkpoint commands, so that recent commands alone determine the
//!   state and the bookkeepers can go on without learning the stuck
//!   command's fate. That isn't implemented here.
//!
//! Also not here: telling a user its command was rejected, which the paper
//! leaves out too (Section 1.2.3), the
//! *distributor* that routes responses back to users (Section 1.2.2), a user
//! machine clock with actions of its own (Section 4.1), keeping the clocks
//! synchronized (Section 3.2.1), and queries answered as of a stamp (Section
//! 4.2).

use super::UserMachine;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

/// A command and the time it was issued: the paper's `T:C`.
///
/// Stamped commands sort by time, then by command, which breaks ties the same
/// way at every bookkeeper: the order a user machine executes them in. A
/// stamped command is one command however many bookkeepers it's sent to, and
/// however many users issue it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamped<C> {
    time: u64,
    command: C,
}

impl<C> Stamped<C> {
    /// # Panics
    ///
    /// If `time` is 0: the system starts then, before any command is issued.
    pub fn new(time: u64, command: C) -> Self {
        assert!(
            time > 0,
            "the system starts at 0, before any command is issued"
        );
        Self { time, command }
    }

    pub fn time(&self) -> u64 {
        self.time
    }

    pub fn command(&self) -> &C {
        &self.command
    }
}

/// What every bookkeeper agrees on before the system starts: which sets of
/// bookkeepers are majority graphs, how long messages take, and how long a
/// bookkeeper may stay quiet.
///
/// Every bookkeeper can send messages to every other directly, so a majority
/// graph is just a set of bookkeepers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config<P> {
    /// Every two share a bookkeeper.
    majority_graphs: Vec<BTreeSet<P>>,
    /// Every bookkeeper in some majority graph.
    bookkeepers: BTreeSet<P>,
    /// `δ`: when a line works, a message on it arrives in less than this.
    delta: u64,
    /// `τ`: whatever else happens, a bookkeeper says before `T + τ` that it
    /// rejects every command stamped `T` it hasn't voted for (AR2(b)).
    tau: u64,
    /// `D`: voting on a command stamped `T` closes at `T + D`.
    window: u64,
}

impl<P: Ord + Clone> Config<P> {
    /// Majority graphs of more than half the `bookkeepers`: with three, every
    /// pair. `delta` and `tau` are the paper's `δ` and `τ`.
    ///
    /// # Panics
    ///
    /// With no bookkeepers, or as [`new`](Self::new) does.
    pub fn majority(bookkeepers: impl IntoIterator<Item = P>, delta: u64, tau: u64) -> Self {
        let all: Vec<P> = bookkeepers
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(!all.is_empty(), "no bookkeepers");
        let size = all.len() / 2 + 1;
        Self::new(subsets(&all).filter(|s| s.len() == size), delta, tau)
    }

    /// Any family of majority graphs in which every two share a bookkeeper.
    /// The bookkeepers are the members of the majority graphs. `delta` and
    /// `tau` are the paper's `δ` and `τ`. Working out the voting window means
    /// trying every set of bookkeepers, so this is for a handful of them. The
    /// paper asks for `τ ≥ 2δ + ε` (`3δ` with three bookkeepers) to keep its
    /// bounds simple to state; this implementation needs only `τ` longer than
    /// the voting window, and its bounds still hold.
    ///
    /// # Panics
    ///
    /// If there are no majority graphs, if one is empty or two share no
    /// bookkeeper, if there are 32 or more bookkeepers, if `delta` is 0, if the
    /// voting window doesn't fit in a `u64`, or if `tau` isn't longer than the
    /// voting [`window`](Self::window).
    pub fn new(
        majority_graphs: impl IntoIterator<Item = impl IntoIterator<Item = P>>,
        delta: u64,
        tau: u64,
    ) -> Self {
        let majority_graphs: Vec<BTreeSet<P>> = majority_graphs
            .into_iter()
            .map(|g| g.into_iter().collect())
            .collect();
        assert!(!majority_graphs.is_empty(), "no majority graphs");
        assert!(
            majority_graphs.iter().all(|g| !g.is_empty()),
            "an empty majority graph"
        );
        for (i, g) in majority_graphs.iter().enumerate() {
            for h in &majority_graphs[i + 1..] {
                assert!(!g.is_disjoint(h), "two majority graphs share no bookkeeper");
            }
        }
        assert!(delta > 0, "δ must be positive");
        let bookkeepers: BTreeSet<P> = majority_graphs.iter().flatten().cloned().collect();
        let mut config = Self {
            majority_graphs,
            bookkeepers,
            delta,
            tau,
            window: 0,
        };
        // Voters ask for another vote only while they include no majority
        // graph, so the last ask carries at most `most` votes, and its
        // deadline is `T + (most + 1)δ`.
        let all: Vec<P> = config.bookkeepers.iter().cloned().collect();
        let most = subsets(&all)
            .filter(|s| !config.contains_majority_graph(s))
            .map(|s| s.len())
            .max()
            .unwrap_or(0);
        config.window = (most as u64 + 1)
            .checked_mul(delta)
            .expect("the voting window overflows");
        assert!(
            tau > config.window,
            "τ must be longer than the voting window, {}",
            config.window
        );
        config
    }

    pub fn bookkeepers(&self) -> impl Iterator<Item = &P> {
        self.bookkeepers.iter()
    }

    pub fn majority_graphs(&self) -> impl Iterator<Item = &BTreeSet<P>> {
        self.majority_graphs.iter()
    }

    /// `δ`: when a line works, a message on it arrives in less than this.
    pub fn delta(&self) -> u64 {
        self.delta
    }

    /// `τ`: whatever else happens, a bookkeeper says before `T + τ` that it
    /// rejects every command stamped `T` it hasn't voted for (AR2(b)).
    pub fn tau(&self) -> u64 {
        self.tau
    }

    /// `D`: no bookkeeper votes to accept a command stamped `T` at or after
    /// `T + D`. The longest cascade's last deadline: `2δ` with three
    /// bookkeepers, `(s + 1)δ` in general, `s` being the most bookkeepers
    /// that include no majority graph.
    pub fn window(&self) -> u64 {
        self.window
    }

    /// Whether every bookkeeper of some majority graph is in `set`: votes from
    /// `set` accept a command.
    pub fn contains_majority_graph(&self, set: &BTreeSet<P>) -> bool {
        self.majority_graphs.iter().any(|g| g.is_subset(set))
    }

    /// Whether every majority graph has a bookkeeper in `set`: rejections from
    /// `set` reject a command.
    pub fn meets_every_majority_graph(&self, set: &BTreeSet<P>) -> bool {
        self.majority_graphs.iter().all(|g| !g.is_disjoint(set))
    }
}

/// Every subset of `all`. There are 2ⁿ, so `all` must be small.
fn subsets<P: Ord + Clone>(all: &[P]) -> impl Iterator<Item = BTreeSet<P>> + '_ {
    assert!(all.len() < 32, "too many bookkeepers to enumerate");
    (0u32..1 << all.len()).map(move |mask| {
        all.iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, p)| p.clone())
            .collect()
    })
}

/// A message from one bookkeeper to all the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message<P, C> {
    pub from: P,
    /// The sender's clock when it sent this. The sender never votes for a
    /// command stamped `T` at or after `T + D` (the voting
    /// [`window`](Config::window)), and the receiver has had all its earlier
    /// messages, so this tells the receiver it has every vote the sender will
    /// ever cast for a command stamped `sent - D` or earlier. The sender
    /// rejects the rest, heard of or not (AR2).
    pub sent: u64,
    /// The sender knows every accepted command stamped at or before this, and
    /// has already sent notice of each one (AR3).
    pub known: u64,
    /// Votes to accept a command (AR1), if this message carries any.
    pub votes: Option<Votes<P, C>>,
}

/// Votes to accept a command (AR1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Votes<P, C> {
    pub command: Stamped<C>,
    /// Every bookkeeper the sender knows has voted to accept `command`. Once
    /// they include a majority graph, this is notice that it's accepted.
    pub voters: BTreeSet<P>,
}

/// One bookkeeper, among bookkeepers named by `P`, running a copy of the user
/// machine `M`. `M`'s commands are ordered, so that every bookkeeper breaks
/// ties between commands stamped at the same time the same way.
///
/// It's driven from outside: hand it users' commands, other bookkeepers'
/// messages and the time, and send each message it returns to every other
/// bookkeeper. `now`, in every call, is this bookkeeper's clock, which must
/// never read earlier than in a previous call
/// (`a_clock_running_backwards_breaks_agreement`).
#[derive(Debug)]
pub struct Bookkeeper<P, M: UserMachine> {
    me: P,
    config: Config<P>,
    /// This bookkeeper's copy, which has executed every accepted command
    /// stamped at or before `known`.
    machine: M,
    /// Every command `machine` has executed, in order, with its response.
    executed: Vec<(Stamped<M::Command>, M::Response)>,
    /// The latest `T` for which this bookkeeper knows the paper's `C^T`, the
    /// accepted commands stamped up to `T`.
    known: u64,
    /// Every command stamped after `known` that this bookkeeper has heard of,
    /// with the bookkeepers it knows voted to accept it.
    pending: BTreeMap<Stamped<M::Command>, BTreeSet<P>>,
    /// For each other bookkeeper, when it sent the latest message received
    /// from it, 0 before anything arrives.
    heard: BTreeMap<P, u64>,
    /// When this bookkeeper last sent a message, 0 before it has.
    last_sent: u64,
}

impl<P: Ord + Clone, M: UserMachine> Bookkeeper<P, M>
where
    M::Command: Ord,
{
    /// Bookkeeper `me`, whose copy of the user machine starts as `machine`:
    /// every bookkeeper's copy must start in the same state.
    ///
    /// # Panics
    ///
    /// If `me` isn't one of `config`'s bookkeepers.
    pub fn new(me: P, config: Config<P>, machine: M) -> Self {
        assert!(config.bookkeepers.contains(&me), "not a bookkeeper");
        let heard = config
            .bookkeepers()
            .filter(|p| **p != me)
            .map(|p| (p.clone(), 0))
            .collect();
        Self {
            me,
            config,
            machine,
            executed: Vec::new(),
            known: 0,
            pending: BTreeMap::new(),
            heard,
            last_sent: 0,
        }
    }

    pub fn me(&self) -> &P {
        &self.me
    }

    pub fn config(&self) -> &Config<P> {
        &self.config
    }

    /// This bookkeeper's copy of the user machine, which has executed every
    /// accepted command stamped at or before [`known`](Self::known). A query,
    /// a command that changes nothing, can be answered from one copy as of
    /// that time: the second of the three ways the paper's Section 4.2 gives.
    pub fn machine(&self) -> &M {
        &self.machine
    }

    /// The latest `T` for which this bookkeeper knows the paper's `C^T`, every
    /// accepted command stamped up to `T`, and has executed them all.
    pub fn known(&self) -> u64 {
        self.known
    }

    /// Every command executed, in order, with its response: what users get
    /// back.
    pub fn executed(&self) -> &[(Stamped<M::Command>, M::Response)] {
        &self.executed
    }

    /// Every command stamped after [`known`](Self::known) that this
    /// bookkeeper has heard of, earliest first, with the bookkeepers it knows
    /// voted to accept it.
    pub fn pending(&self) -> impl Iterator<Item = (&Stamped<M::Command>, &BTreeSet<P>)> {
        self.pending.iter()
    }

    /// For each other bookkeeper, when it sent the latest message received
    /// from it, 0 before anything arrives.
    pub fn heard(&self) -> impl Iterator<Item = (&P, u64)> {
        self.heard.iter().map(|(p, &sent)| (p, sent))
    }

    /// AR1(a): a user's command arrives, at `now` on this bookkeeper's clock.
    /// Returns the messages to send to every other bookkeeper.
    pub fn submit(
        &mut self,
        now: u64,
        command: Stamped<M::Command>,
    ) -> Vec<Message<P, M::Command>> {
        let mut out = Vec::new();
        self.hear(now, command, BTreeSet::new(), &mut out);
        self.settle(now, &mut out);
        out
    }

    /// A message from another bookkeeper arrives, at `now` on this
    /// bookkeeper's clock. Returns the messages to send to every other
    /// bookkeeper.
    ///
    /// Messages from each bookkeeper must arrive in the order it sent them,
    /// none skipped, though they may stop coming: a bookkeeper infers
    /// rejections from what it hasn't been told.
    pub fn receive(
        &mut self,
        now: u64,
        msg: Message<P, M::Command>,
    ) -> Vec<Message<P, M::Command>> {
        let heard = self
            .heard
            .get_mut(&msg.from)
            .expect("message from an unknown bookkeeper");
        *heard = (*heard).max(msg.sent);
        let mut out = Vec::new();
        if let Some(Votes { command, voters }) = msg.votes {
            self.hear(now, command, voters, &mut out);
        }
        self.known = self.known.max(msg.known); // AR3
        self.settle(now, &mut out);
        out
    }

    /// The clock reads `now`: execute what the passing time settles, and send
    /// any rejections that have come due (AR2). Call it every unit of time, or
    /// rejections go out late. Returns the messages to send to every other
    /// bookkeeper.
    pub fn advance_to(&mut self, now: u64) -> Vec<Message<P, M::Command>> {
        let mut out = Vec::new();
        self.settle(now, &mut out);
        out
    }

    /// AR1: hear that `voters` have voted to accept `command` (none, from a
    /// user). Vote too if the news is in time, and give notice if it means
    /// the command is accepted.
    fn hear(
        &mut self,
        now: u64,
        command: Stamped<M::Command>,
        voters: BTreeSet<P>,
        out: &mut Vec<Message<P, M::Command>>,
    ) {
        if command.time <= self.known {
            return; // decided already, and executed if it was accepted
        }
        // The cascade: `k` votes ask for another before `T + (k + 1)δ`, until
        // they include a majority graph.
        let wait = (voters.len() as u64 + 1).saturating_mul(self.config.delta);
        let asks = !self.config.contains_majority_graph(&voters)
            && now < command.time.saturating_add(wait);
        let voted = self.pending.entry(command.clone()).or_default();
        let was_accepted = self.config.contains_majority_graph(voted);
        voted.extend(voters);
        let votes_now = asks && voted.insert(self.me.clone());
        // Section 3.1: whoever learns a command is accepted tells everyone.
        let accepted_now = !was_accepted && self.config.contains_majority_graph(voted);
        if votes_now || accepted_now {
            let voters = voted.clone();
            out.push(self.send(now, Some(Votes { command, voters })));
        }
    }

    /// Learn what can be learned, execute what that allows, and send a
    /// message if one is due (AR2, AR3).
    fn settle(&mut self, now: u64, out: &mut Vec<Message<P, M::Command>>) {
        self.known = self.known.max(self.decided_through(now));
        let mut decided = false;
        while let Some(entry) = self.pending.first_entry() {
            if entry.key().time > self.known {
                break;
            }
            let (command, voters) = entry.remove_entry();
            decided = true;
            if self.config.contains_majority_graph(&voters) {
                let response = self.machine.execute(command.command());
                self.executed.push((command, response));
            }
        }
        let window = self.config.window;
        // AR2(a): voting has closed on a command this bookkeeper knows was
        // issued, and no message since has said so.
        let said = self.last_sent.saturating_sub(window);
        let closed = self
            .pending
            .keys()
            .find(|c| c.time > said)
            .is_some_and(|c| c.time.saturating_add(window) <= now);
        // AR2(b): it's been quiet long enough that rejections are due anyway.
        let quiet = now >= self.last_sent.saturating_add(self.config.tau - window);
        // AR3: it has decided commands it knew of, so tell the others.
        if closed || quiet || decided {
            out.push(self.send(now, None));
        }
    }

    /// The latest time up to which the votes heard so far decide every
    /// command, heard of or not.
    fn decided_through(&self, now: u64) -> u64 {
        // A bookkeeper's votes on commands stamped at or before its horizon
        // are final, and all here.
        let horizon = |p: &P| {
            let clock = if *p == self.me { now } else { self.heard[p] };
            clock.saturating_sub(self.config.window)
        };
        // The latest time by which the bookkeepers whose voting has closed meet
        // every majority graph. Anything stamped then or earlier that none of
        // them voted for is rejected.
        let mut latest_first: Vec<&P> = self.config.bookkeepers().collect();
        latest_first.sort_by_key(|p| Reverse(horizon(p)));
        let mut closed = BTreeSet::new();
        let mut through = 0;
        for p in latest_first {
            closed.insert(p.clone());
            if self.config.meets_every_majority_graph(&closed) {
                through = horizon(p);
                break;
            }
        }
        // Every command heard of up to then must be accepted or rejected.
        for (command, voters) in &self.pending {
            if command.time <= self.known {
                continue; // decided already, on another bookkeeper's word (AR3)
            }
            if command.time > through {
                break;
            }
            let rejecters: BTreeSet<P> = self
                .config
                .bookkeepers()
                .filter(|p| !voters.contains(*p) && horizon(p) >= command.time)
                .cloned()
                .collect();
            if !self.config.contains_majority_graph(voters)
                && !self.config.meets_every_majority_graph(&rejecters)
            {
                return command.time - 1;
            }
        }
        through
    }

    /// A message to every other bookkeeper, sent at `now`.
    fn send(&mut self, now: u64, votes: Option<Votes<P, M::Command>>) -> Message<P, M::Command> {
        self.last_sent = now;
        Message {
            from: self.me.clone(),
            sent: now,
            known: self.known,
            votes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::sim::{Log, Sim};
    use crate::test_util::Rng;

    const DELTA: u64 = 10;
    const TAU: u64 = 40;

    fn three() -> Config<char> {
        Config::majority(['A', 'B', 'C'], DELTA, TAU)
    }

    /// A and anyone, or B, C and D together: majority graphs that aren't
    /// majorities.
    fn lopsided() -> Config<char> {
        let graphs = [&['A', 'B'][..], &['A', 'C'], &['A', 'D'], &['B', 'C', 'D']];
        Config::new(graphs.map(|g| g.iter().copied()), DELTA, TAU)
    }

    fn book(me: char, config: &Config<char>) -> Bookkeeper<char, Log> {
        Bookkeeper::new(me, config.clone(), Log::default())
    }

    fn set<const N: usize>(ps: [char; N]) -> BTreeSet<char> {
        ps.into()
    }

    fn only(mut msgs: Vec<Message<char, u32>>) -> Message<char, u32> {
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        msgs.pop().unwrap()
    }

    fn voters(msg: &Message<char, u32>) -> BTreeSet<char> {
        msg.votes.as_ref().expect("no votes").voters.clone()
    }

    /// Step `sim` until `done`, failing if that takes until `limit`. Returns
    /// the time it took until.
    fn step_until(sim: &mut Sim<Log>, limit: u64, done: impl Fn(&Sim<Log>) -> bool) -> u64 {
        while !done(sim) {
            assert!(sim.now < limit, "still waiting at {limit}");
            sim.step();
        }
        sim.now
    }

    #[test]
    fn the_voting_window_is_the_longest_cascade() {
        let three = three();
        assert_eq!(three.majority_graphs().count(), 3);
        assert_eq!(three.window(), 2 * DELTA);
        assert_eq!(
            Config::majority("ABCDE".chars(), DELTA, TAU).window(),
            3 * DELTA
        );
        // Two of B, C and D are the most that include no majority graph.
        assert_eq!(lopsided().window(), 3 * DELTA);
    }

    #[test]
    #[should_panic(expected = "share no bookkeeper")]
    fn majority_graphs_must_meet() {
        Config::new([['A', 'B'], ['C', 'D']], DELTA, TAU);
    }

    #[test]
    #[should_panic(expected = "no majority graphs")]
    fn a_config_needs_majority_graphs() {
        Config::new(Vec::<Vec<char>>::new(), DELTA, TAU);
    }

    #[test]
    #[should_panic(expected = "an empty majority graph")]
    fn a_majority_graph_cant_be_empty() {
        Config::new([vec!['A'], vec![]], DELTA, TAU);
    }

    #[test]
    #[should_panic(expected = "δ must be positive")]
    fn delta_must_be_positive() {
        Config::majority(['A', 'B', 'C'], 0, TAU);
    }

    /// AR2(b) has a bookkeeper speak at least every `τ - D`, so `τ` must be
    /// longer than `D`.
    #[test]
    #[should_panic(expected = "τ must be longer than the voting window")]
    fn tau_must_be_longer_than_the_voting_window() {
        Config::majority(['A', 'B', 'C'], DELTA, 2 * DELTA);
    }

    #[test]
    #[should_panic(expected = "the voting window overflows")]
    fn the_voting_window_must_fit() {
        Config::majority(['A', 'B', 'C'], 1 << 63, u64::MAX);
    }

    #[test]
    #[should_panic(expected = "too many bookkeepers")]
    fn a_config_is_for_a_handful_of_bookkeepers() {
        Config::new([(0..32u32).collect::<Vec<_>>()], DELTA, TAU);
    }

    /// One command's life, with δ = 10. A user issues `1:7` and it reaches A
    /// at 4, before `1 + δ`. Votes spread, voting closes at `1 + 2δ = 21`,
    /// the bookkeepers say so, and each executes the command once it has
    /// heard that from another.
    #[test]
    fn three_bookkeepers_accept_and_execute_a_command() {
        let config = three();
        let (mut a, mut b, mut c) = (book('A', &config), book('B', &config), book('C', &config));
        let cmd = Stamped::new(1, 7);

        // AR1(a): A votes and tells the others.
        let vote_a = only(a.submit(4, cmd.clone()));
        assert_eq!(voters(&vote_a), set(['A']));
        // AR1(b): B hears of it before 21 and votes too. A and B are a
        // majority graph, so B's vote is notice of acceptance.
        let vote_b = only(b.receive(9, vote_a.clone()));
        assert_eq!(voters(&vote_b), set(['A', 'B']));
        let vote_c = only(c.receive(12, vote_a));
        assert_eq!(voters(&vote_c), set(['A', 'C']));
        // A learns its command is accepted, and says so.
        assert_eq!(voters(&only(a.receive(14, vote_b))), set(['A', 'B']));
        assert!(a.receive(15, vote_c).is_empty());

        // Accepted, but nobody can execute it yet: a command stamped 1 that
        // nobody has heard of could still be accepted until 21.
        for p in [&mut a, &mut b, &mut c] {
            assert!(p.advance_to(20).is_empty());
            assert_eq!(p.known(), 0);
        }
        // AR2(a): voting on 1:7 closes, and each bookkeeper says so.
        let closed_a = only(a.advance_to(21));
        let closed_b = only(b.advance_to(21));
        assert_eq!((closed_a.sent, closed_a.votes.clone()), (21, None));

        // Hearing that from one other bookkeeper is enough: A and B meet
        // every majority graph.
        b.receive(25, closed_a.clone());
        a.receive(25, closed_b);
        c.receive(26, closed_a);
        for p in [&a, &b, &c] {
            assert_eq!(p.known(), 1, "{}", p.me());
            assert_eq!(p.executed(), [(cmd.clone(), 1)]);
            assert_eq!(p.pending().count(), 0);
        }
    }

    /// The slowest case RC allows (the paper's Proposition 1, case (ii)). B
    /// and C work properly; A votes for its user's command just in time, then
    /// crashes. B's vote completes {A, B}, a majority graph, so it's notice,
    /// and asks C for no vote. C hears of the command only from it, and
    /// passes the notice on. Sent after voting on the command closed, that
    /// also says C rejects everything else stamped up to then, and B hears it
    /// after `T + 3δ`: still before `T + 4δ`.
    #[test]
    fn a_command_another_majority_graph_accepts_comes_last() {
        let config = three();
        let (mut a, mut b, mut c) = (book('A', &config), book('B', &config), book('C', &config));
        let vote_a = only(a.submit(10, Stamped::new(1, 7)));
        let vote_b = only(b.receive(19, vote_a));
        assert_eq!(voters(&vote_b), set(['A', 'B']));
        for now in 20..37 {
            b.advance_to(now);
        }
        assert_eq!(b.known(), 0); // C could have voted for a command stamped 1

        let notice_c = only(c.receive(28, vote_b));
        assert_eq!(voters(&notice_c), set(['A', 'B']));
        b.receive(37, notice_c);
        assert_eq!(b.machine().0, [7]);
    }

    /// Accepting takes a majority graph's votes; rejecting takes rejections
    /// that meet every majority graph. With four bookkeepers, where a
    /// majority graph is any three, the two differ: A's and B's votes for
    /// `1:7` meet every majority graph but contain none, so they don't accept
    /// it, and C's and D's rejections, which meet every majority graph too,
    /// reject it. D rejects it knowing of both votes, just as C does knowing
    /// only of A's.
    #[test]
    fn votes_that_meet_every_majority_graph_dont_accept() {
        let config = Config::majority("ABCD".chars(), DELTA, TAU);
        let [mut a, mut b, mut c, mut d] = ['A', 'B', 'C', 'D'].map(|p| book(p, &config));
        let vote_a = only(a.submit(2, Stamped::new(1, 7)));
        let vote_b = only(b.receive(5, vote_a.clone()));
        c.receive(25, vote_a.clone()); // too late for C and D to vote
        d.receive(25, vote_a);
        let (closed_c, closed_d) = (c.advance_to(31), d.advance_to(31));
        for m in closed_d {
            c.receive(33, m);
        }
        d.receive(35, vote_b);
        for m in closed_c {
            d.receive(36, m);
        }
        for p in [&c, &d] {
            assert!(p.known() >= 1, "{}", p.me());
            assert!(p.executed().is_empty(), "{}", p.me());
        }
    }

    /// With nothing happening, only AR2(b) makes a bookkeeper speak: once
    /// every `τ - D`, so a longer `τ` means fewer messages when the system is
    /// idle. Hearing that the others' voting has closed advances what a
    /// bookkeeper knows, but with no commands to decide, it has nothing to
    /// say about that (AR3).
    #[test]
    fn an_idle_bookkeeper_speaks_once_every_tau_minus_d() {
        let mut sim = Sim::new(three(), Log::default(), 8);
        let every = TAU - 2 * DELTA;
        while sim.now < 1000 {
            sim.step();
            for p in sim.ids() {
                let sent = sim.sent.get(&p).copied().unwrap_or(0) as u64;
                assert_eq!(sent, sim.now / every, "{p} at {}", sim.now);
            }
        }
        for p in sim.ids() {
            assert!(sim.book(p).known() >= 1000 - TAU);
        }
    }

    /// The mutex stops for good when one process crashes. Here, with C down
    /// from the start, A and B execute every command before `T + 4δ`, as RC
    /// promises.
    #[test]
    fn a_crashed_bookkeeper_doesnt_stop_the_others() {
        let mut sim = Sim::new(three(), Log::default(), 1);
        sim.crash('C');
        let commands = [(5, 'A'), (30, 'B'), (31, 'A'), (90, 'B')]
            .map(|(t, to)| (Stamped::new(t, t as u32), to));
        for (command, to) in &commands {
            sim.submit(command.time() + 3, *to, command.clone());
        }
        sim.run_until(200);
        sim.assert_agree("crash");
        let all: Vec<_> = commands.iter().map(|(c, _)| c.clone()).collect();
        for p in ['A', 'B'] {
            assert_eq!(sim.log(p), all);
            for c in &all {
                assert!(sim.executed_at[&(p, c.clone())] < c.time() + 4 * DELTA);
            }
        }
        assert!(sim.log('C').is_empty());
    }

    /// A command that reaches a bookkeeper at `T + δ` gets no vote, and every
    /// bookkeeper leaves it out, though only A ever heard of it.
    #[test]
    fn a_late_command_is_rejected_everywhere() {
        let mut sim = Sim::new(three(), Log::default(), 2);
        sim.submit(1 + DELTA, 'A', Stamped::new(1, 7));
        sim.submit(25, 'B', Stamped::new(20, 8));
        sim.run_until(200);
        sim.assert_agree("late");
        for p in sim.ids() {
            assert_eq!(sim.book(p).machine().0, [8], "{p}");
            assert!(sim.book(p).known() >= 20);
        }
    }

    /// The paper's Section 4.4. With C down and A and B cut off from each
    /// other, A votes to accept `1:7`, and B, which doesn't hear of it in
    /// time, votes to reject it. Once A and B can talk again they still can't
    /// decide it, since C's vote would, so neither executes anything stamped
    /// later, not even a command both of them voted for. When C is back, its
    /// rejection settles it.
    #[test]
    fn a_split_vote_waits_for_the_third_bookkeeper() {
        let mut sim = Sim::new(three(), Log::default(), 3);
        sim.crash('C');
        sim.cut('A', 'B');
        sim.cut('B', 'A');
        sim.submit(2, 'A', Stamped::new(1, 7));
        sim.run_until(40);
        sim.mend('A', 'B');
        sim.mend('B', 'A');
        for to in ['A', 'B'] {
            sim.submit(51, to, Stamped::new(50, 8));
        }
        sim.run_until(150);
        for p in ['A', 'B'] {
            let book = sim.book(p);
            assert_eq!(book.known(), 0, "{p}");
            assert!(book.executed().is_empty());
            let pending: Vec<_> = book
                .pending()
                .map(|(c, v)| (c.command(), v.clone()))
                .collect();
            assert_eq!(pending, [(&7, set(['A'])), (&8, set(['A', 'B']))]);
        }

        sim.restore('C');
        sim.run_until(300);
        sim.assert_agree("split");
        for p in sim.ids() {
            assert_eq!(sim.book(p).machine().0, [8], "{p}");
        }
    }

    /// AR3. C never hears from A. B voted for `1:7`; whether A did is what
    /// decides it, and only A's messages say. B hears them, and as soon as it
    /// knows everything up to then, it says so, and C takes its word for what
    /// it couldn't work out alone.
    #[test]
    fn a_bookkeeper_learns_what_it_cant_work_out_from_another() {
        let mut sim = Sim::new(three(), Log::default(), 4);
        sim.cut('A', 'C');
        sim.cut('B', 'A');
        sim.crash('C');
        sim.submit(2, 'B', Stamped::new(1, 7));
        for to in ['B', 'C'] {
            sim.submit(71, to, Stamped::new(70, 8));
        }
        sim.run_until(30);
        sim.mend('B', 'A'); // too late for A to vote
        sim.run_until(50);
        sim.restore('C'); // too late for C to vote
        let knows = |p| move |sim: &Sim<Log>| sim.book(p).known() >= 1;
        let b_knew = step_until(&mut sim, 200, knows('B'));
        let c_knew = step_until(&mut sim, 200, knows('C'));
        assert!(c_knew < b_knew + DELTA);

        sim.run_until(200);
        sim.assert_agree("ar3");
        for p in sim.ids() {
            assert_eq!(sim.book(p).machine().0, [8], "{p}");
        }
        let c = sim.book('C');
        assert_eq!(c.heard().find(|(p, _)| **p == 'A'), Some((&'A', 0)));
        assert!(c.known() >= 70);
    }

    /// Another bookkeeper's word and a bookkeeper's own votes count together.
    /// C voted for `1:7` and nobody heard; only A's rejection, which C can't
    /// hear, settles it. B voted for `22:9` and C did too, accepting it. When
    /// B's word reaches C that everything up to 20 is settled (AR3), the votes
    /// and rejections C has from itself and B settle the rest up to 22 in the
    /// same step, and C executes `22:9` at once.
    #[test]
    fn another_bookkeepers_word_and_its_own_votes_count_together() {
        let config = three();
        let (mut a, mut b, mut c) = (book('A', &config), book('B', &config), book('C', &config));
        c.submit(2, Stamped::new(1, 7));
        let quiet_b = only(b.advance_to(20));
        let vote_b = only(b.submit(23, Stamped::new(22, 9)));
        let quiet_a = [20, 40].map(|now| only(a.advance_to(now)));
        for m in quiet_a {
            b.receive(41, m);
        }
        assert_eq!(b.known(), 20); // A hasn't said anything about 22:9
        let closed_b = only(b.advance_to(42)); // AR2(a)
        assert_eq!((closed_b.sent, closed_b.known), (42, 20));

        c.receive(25, quiet_b);
        assert_eq!(voters(&only(c.receive(30, vote_b))), set(['B', 'C']));
        assert_eq!(c.known(), 0); // 1:7 waits on A
        c.receive(70, closed_b);
        assert_eq!(c.known(), 22);
        assert_eq!(c.machine().0, [9]);
    }

    /// Deadlines near the end of time saturate rather than overflow: a command
    /// stamped there gets its vote, and nothing more is sent for it.
    #[test]
    fn a_stamp_near_the_end_of_time_is_harmless() {
        let mut a = book('A', &three());
        assert_eq!(a.submit(5, Stamped::new(u64::MAX - 5, 1)).len(), 1);
        let sent: usize = (6..1006).map(|now| a.advance_to(now).len()).sum();
        assert_eq!(sent as u64, 1000 / (TAU - 2 * DELTA));
    }

    /// The working majority graph moves. A and B run the system while C, up
    /// but unheard, has its lines out cut; C's line to B is mended at 60, so
    /// B and C work from then on; A keeps working until `Ω = τ + 2δ` after
    /// that, then crashes. C never hears from A. Its own vote for `30:5` came
    /// too late for A and B, which rejected the command; C can't hear A's
    /// rejection, so it takes B's word for it (AR3).
    #[test]
    fn the_working_majority_graph_can_move() {
        let omega = TAU + 2 * DELTA;
        let mut sim = Sim::new(three(), Log::default(), 5);
        sim.cut('A', 'C');
        sim.cut('C', 'A');
        sim.cut('C', 'B');
        sim.submit(11, 'A', Stamped::new(10, 1));
        sim.submit(12, 'B', Stamped::new(10, 1));
        sim.submit(31, 'C', Stamped::new(30, 5));
        sim.submit(41, 'A', Stamped::new(40, 2));
        sim.run_until(60);
        sim.mend('C', 'B');
        sim.run_until(60 + omega);
        sim.crash('A');
        sim.submit(131, 'B', Stamped::new(130, 3));
        sim.submit(152, 'C', Stamped::new(150, 4));
        sim.run_until(300);
        sim.assert_agree("move");
        assert_eq!(sim.book('A').machine().0, [1, 2]);
        for p in ['B', 'C'] {
            assert_eq!(sim.book(p).machine().0, [1, 2, 3, 4], "{p}");
        }
        let c = sim.book('C');
        assert_eq!(c.heard().find(|(p, _)| **p == 'A'), Some((&'A', 0)));
    }

    /// With five bookkeepers a majority graph is any three, so two votes
    /// don't accept a command and the cascade runs a step longer: told of two
    /// votes, a bookkeeper may still vote until `T + 3δ`, which with three
    /// bookkeepers would be too late. Each step here takes just under `δ`.
    ///
    /// It's RC's slowest case with five, too. C, D and E work properly, and A
    /// and B crash after voting. C's vote completes {A, B, C} and asks nobody
    /// else, so D and E hear of the command only from C's notice, after
    /// voting on it has closed, and pass it on. That's when C learns their
    /// voting has closed, past `T + D + δ`, and still before `T + D + 2δ`.
    #[test]
    fn the_cascade_runs_a_step_longer_with_five() {
        let config = Config::majority("ABCDE".chars(), DELTA, TAU);
        let [mut a, mut b, mut c, mut d, mut e] =
            ['A', 'B', 'C', 'D', 'E'].map(|p| book(p, &config));
        let vote_a = only(a.submit(10, Stamped::new(1, 7)));
        let vote_b = only(b.receive(19, vote_a));
        assert_eq!(voters(&vote_b), set(['A', 'B']));
        let notice_c = only(c.receive(28, vote_b));
        assert_eq!(voters(&notice_c), set(['A', 'B', 'C']));

        let closed_c = only(c.advance_to(31)); // AR2(a)
        let notice_d = only(d.receive(37, notice_c.clone()));
        let notice_e = only(e.receive(37, notice_c));
        d.receive(40, closed_c.clone());
        e.receive(40, closed_c);
        for p in [&c, &d, &e] {
            assert!(p.executed().is_empty(), "{}", p.me());
        }
        c.receive(46, notice_d.clone());
        c.receive(46, notice_e.clone());
        d.receive(46, notice_e);
        e.receive(46, notice_d);
        let (window, at) = (config.window(), 46);
        assert!((1 + window + DELTA..1 + window + 2 * DELTA).contains(&at));
        for p in [&c, &d, &e] {
            assert_eq!(p.machine().0, [7], "{}", p.me());
        }
    }

    /// With five bookkeepers a majority graph is any three, so two crashes
    /// still leave one working, and RC's bound is `T + D + 2δ = T + 5δ`.
    #[test]
    fn five_bookkeepers_outlast_two_crashes() {
        let config = Config::majority("ABCDE".chars(), DELTA, TAU);
        let mut sim = Sim::new(config, Log::default(), 6);
        sim.crash('D');
        sim.crash('E');
        let commands = [(5, 'A'), (20, 'C')].map(|(t, to)| (Stamped::new(t, t as u32), to));
        for (command, to) in &commands {
            sim.submit(command.time() + 1, *to, command.clone());
        }
        sim.run_until(200);
        sim.assert_agree("five");
        for p in ['A', 'B', 'C'] {
            assert_eq!(sim.book(p).machine().0, [5, 20], "{p}");
            for (c, _) in &commands {
                assert!(sim.executed_at[&(p, c.clone())] < c.time() + 5 * DELTA);
            }
        }
    }

    /// Majority graphs needn't be majorities. Here A and any other bookkeeper
    /// form one, and so do B, C and D together: A and D carry on without B
    /// and C, and B, C and D carry on without A.
    #[test]
    fn a_majority_graph_needn_t_be_a_majority() {
        for (down, up) in [
            (&['B', 'C'][..], &['A', 'D'][..]),
            (&['A'], &['B', 'C', 'D']),
        ] {
            let mut sim = Sim::new(lopsided(), Log::default(), 7);
            for &p in down {
                sim.crash(p);
            }
            sim.submit(6, up[0], Stamped::new(5, 5));
            sim.submit(31, up[up.len() - 1], Stamped::new(30, 30));
            sim.run_until(200);
            sim.assert_agree("lopsided");
            for &p in up {
                assert_eq!(sim.book(p).machine().0, [5, 30], "{p} without {down:?}");
            }
        }
    }

    /// The execution the FIFO assumption rules out. B votes for A's command,
    /// making it accepted, but B's vote is slow to reach C, and B's next
    /// message, which says B's voting on it has closed, overtakes it. C has no
    /// vote from B on record, so it counts B as rejecting the command, and
    /// with its own rejection that's enough: C leaves out a command A and B
    /// execute.
    #[test]
    fn overtaking_breaks_agreement() {
        let config = three();
        let (mut a, mut b, mut c) = (book('A', &config), book('B', &config), book('C', &config));
        let vote_a = only(a.submit(2, Stamped::new(1, 7)));
        let vote_b = only(b.receive(5, vote_a.clone()));
        a.receive(6, vote_b.clone());
        let closed_a = only(a.advance_to(21));
        let closed_b = only(b.advance_to(21));
        a.receive(25, closed_b.clone());
        b.receive(25, closed_a);
        assert_eq!(a.machine().0, [7]);
        assert_eq!(b.machine().0, [7]);

        c.receive(22, vote_a); // too late for C to vote
        c.receive(23, closed_b); // overtakes vote_b
        assert_eq!(c.known(), 1);
        assert!(c.executed().is_empty());
        c.receive(24, vote_b); // too late to matter
        assert!(c.executed().is_empty());
    }

    /// A clock that runs backwards. B says at 40 that its voting on commands
    /// stamped up to 20 has closed, and C, counting B's rejection and its own,
    /// rejects `1:7`. Then B's clock jumps back to 5, A's vote reaches it
    /// "in time", and B votes for the command it had already rejected.
    #[test]
    fn a_clock_running_backwards_breaks_agreement() {
        let config = three();
        let (mut a, mut b, mut c) = (book('A', &config), book('B', &config), book('C', &config));
        let vote_a = only(a.submit(2, Stamped::new(1, 7)));
        let quiet_b = only(b.advance_to(40)); // AR2(b)
        c.receive(41, vote_a.clone()); // too late for C to vote
        c.receive(42, quiet_b.clone());
        assert_eq!(c.known(), 20);
        assert!(c.executed().is_empty());

        let vote_b = only(b.receive(5, vote_a));
        assert_eq!(voters(&vote_b), set(['A', 'B']));
        a.receive(43, quiet_b);
        a.receive(44, vote_b);
        assert_eq!(a.machine().0, [7]);
    }

    /// The algorithm's claims, on random executions with crashes, cut and
    /// slow lines, late commands, and commands issued at the same time. One
    /// majority graph, `m0`, works properly from the start until `Ω = τ + 2δ`
    /// after another, `m1`, starts working properly (the paper's `Ω`, with
    /// `ε = δ`); `m1` then works to the end. They may be the same graph.
    /// - Every two bookkeepers execute the same commands in stamp order with
    ///   the same responses, up to where one of them has got to.
    /// - RC, for a command stamped `T` and the graph `m` working from before
    ///   `T - Ω` until `T + D + 2δ`: if it reaches a member of `m` before
    ///   `T + δ` it's accepted, and if it's accepted every member of `m`
    ///   executes it before `T + D + 2δ`.
    /// - The members of a working graph know every command's fate by
    ///   `T + τ + δ`, once a new graph has worked for `Ω`.
    /// - Once everything works again, every bookkeeper executes the same
    ///   commands.
    /// - No bookkeeper sends more than four messages for each command issued
    ///   (for each one it hears of, at most a vote, a notice, its rejections,
    ///   and word that it has decided), besides AR2(b)'s one every `τ - D`.
    ///
    /// Four bookkeepers are in the mix because there votes that meet every
    /// majority graph needn't contain one; with three or five, and with
    /// `lopsided`, they always do.
    #[test]
    fn random_executions_agree_and_meet_rc() {
        for config in [
            three(),
            Config::majority("ABCD".chars(), DELTA, TAU),
            Config::majority("ABCDE".chars(), DELTA, TAU),
            lopsided(),
        ] {
            for seed in 1..=100 {
                assert_eq!(random_execution(&config, seed, false), 0);
            }
        }
    }

    /// Clocks that disagree, each running up to `3δ` ahead of real time or
    /// behind it, but never backwards. Bookkeepers ahead of the users reject
    /// commands that reached them in time by the users' clocks, but the copies
    /// still agree, and converge once everything works.
    #[test]
    fn skewed_clocks_cost_progress_not_agreement() {
        for config in [three(), Config::majority("ABCDE".chars(), DELTA, TAU)] {
            let missed: usize = (1..=100)
                .map(|seed| random_execution(&config, seed, true))
                .sum();
            assert!(missed > 0, "skew cost nothing");
        }
    }

    /// One random execution, checking what `random_executions_agree_and_meet_rc`
    /// claims, or with `skewed`, only agreement, convergence and the message
    /// budget, with room in it for clocks running ahead. Returns how many
    /// commands were rejected though RC says they'd be accepted, which with
    /// perfect clocks is none.
    fn random_execution(config: &Config<char>, seed: u64, skewed: bool) -> usize {
        const UNTIL: u64 = 300;
        let (window, delta, tau) = (config.window(), config.delta(), config.tau());
        let omega = tau + 2 * delta;
        let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
        let mut sim = Sim::new(config.clone(), Log::default(), seed);
        let ids = sim.ids();
        let graphs: Vec<&BTreeSet<char>> = config.majority_graphs().collect();
        let m0 = graphs[rng.below(graphs.len())].clone();
        let m1 = graphs[rng.below(graphs.len())].clone();
        let switch = 60 + rng.below(100) as u64;
        // Whether the bookkeepers `ps` (one, or a line's two ends) all belong
        // to a graph that must be working at `now`.
        let working = |now: u64, ps: &[char]| {
            (now <= switch + omega && ps.iter().all(|p| m0.contains(p)))
                || (now >= switch && ps.iter().all(|p| m1.contains(p)))
        };
        let lines: Vec<(char, char)> = ids
            .iter()
            .flat_map(|&f| ids.iter().map(move |&t| (f, t)))
            .filter(|&(f, t)| f != t)
            .collect();
        if skewed {
            for &p in &ids {
                let skew = rng.below(6 * delta as usize + 1) as i64 - 3 * delta as i64;
                sim.set_skew(p, skew);
            }
        }
        let (mut cut, mut slow) = (BTreeSet::new(), BTreeSet::new());
        // Every command issued, and the bookkeepers it reached in time.
        let mut issued: Vec<(Stamped<u32>, BTreeSet<char>)> = Vec::new();

        for now in 1..=UNTIL {
            if now == switch {
                for &p in &m1 {
                    sim.restore(p);
                }
                for &(f, t) in &lines {
                    if m1.contains(&f) && m1.contains(&t) {
                        cut.remove(&(f, t));
                        slow.remove(&(f, t));
                        sim.mend(f, t);
                        sim.set_slow(f, t, false);
                    }
                }
            }
            if rng.below(20) == 0 {
                let p = ids[rng.below(ids.len())];
                if !working(now, &[p]) {
                    if sim.is_crashed(p) {
                        sim.restore(p);
                    } else {
                        sim.crash(p);
                    }
                }
            }
            if rng.below(10) == 0 {
                let (f, t) = lines[rng.below(lines.len())];
                if !working(now, &[f, t]) {
                    if cut.insert((f, t)) {
                        sim.cut(f, t);
                    } else {
                        cut.remove(&(f, t));
                        sim.mend(f, t);
                    }
                }
            }
            if rng.below(10) == 0 {
                let (f, t) = lines[rng.below(lines.len())];
                if !working(now, &[f, t]) {
                    let now_slow = slow.insert((f, t)) || !slow.remove(&(f, t));
                    sim.set_slow(f, t, now_slow);
                }
            }
            let how_many = match rng.below(8) {
                0 => 1,
                1 => 2,
                _ => 0,
            };
            for _ in 0..how_many {
                let command = Stamped::new(now, rng.below(3) as u32);
                let mut to: Vec<char> = ids.iter().copied().filter(|_| rng.below(2) == 0).collect();
                if to.is_empty() {
                    to.push(ids[rng.below(ids.len())]);
                }
                let mut in_time = BTreeSet::new();
                for p in to {
                    let late = rng.below(4) == 0;
                    let after = if late {
                        delta + rng.below(2 * delta as usize) as u64
                    } else {
                        in_time.insert(p);
                        rng.below(delta as usize) as u64
                    };
                    sim.submit(now + after, p, command.clone());
                }
                issued.push((command, in_time));
            }

            sim.step();
            sim.assert_agree(&format!("seed {seed}"));
            let budget = 4 * issued.len() as u64 + (now + 3 * delta) / (tau - window) + 2;
            for (p, &sent) in &sim.sent {
                assert!(
                    sent as u64 <= budget,
                    "seed {seed}: {p} has sent {sent} messages by {now}"
                );
            }
            if skewed {
                continue;
            }
            let mut deciding: Vec<char> = Vec::new();
            if now <= switch + omega {
                deciding.extend(&m0);
            }
            if now >= switch + omega {
                deciding.extend(&m1);
            }
            for p in deciding {
                let known = sim.book(p).known();
                assert!(
                    known + tau + delta > now,
                    "seed {seed}: {p} knows only up to {known} at {now}"
                );
            }
        }

        sim.heal();
        sim.run_until(UNTIL + 2 * tau + 4 * delta);
        sim.assert_agree(&format!("seed {seed}, healed"));
        let last = issued.iter().map(|(c, _)| c.time()).max().unwrap_or(0);
        let first = sim.log(ids[0]);
        for &p in &ids {
            assert!(sim.book(p).known() >= last, "seed {seed}: {p} is stuck");
            assert_eq!(sim.log(p), first, "seed {seed}: {p}");
        }
        let mut missed = 0;
        for (command, in_time) in &issued {
            let t = command.time();
            let m = if t > switch + omega {
                &m1
            } else if t + window + 2 * delta <= switch + omega {
                &m0
            } else {
                continue; // m0 may stop working before RC's deadline
            };
            let accepted = first.contains(command);
            if !accepted && !in_time.is_disjoint(m) {
                assert!(
                    skewed,
                    "seed {seed}: {command:?} reached {m:?} in time but was rejected"
                );
                missed += 1;
            }
            if accepted && !skewed {
                for &p in m {
                    let at = sim.executed_at[&(p, command.clone())];
                    assert!(
                        at < t + window + 2 * delta,
                        "seed {seed}: {p} executed {command:?} at {at}"
                    );
                }
            }
        }
        missed
    }
}
