//! A network of bookkeepers for the tests to run executions on.
//!
//! Time moves in steps of one. Every message goes from its sender to each
//! other bookkeeper over its own line, which delivers in order and loses
//! nothing. A working line delivers in less than `δ`; a slow one takes up to
//! `5δ`; a cut one holds everything until it's mended. A line that starts
//! working again delivers what it was holding within `δ` too, as the paper's
//! processes make sure of by resending until a message gets through. A
//! crashed bookkeeper does nothing, and whatever is sent to it waits until
//! it's restored.

use super::{Bookkeeper, Config, Message, Stamped, UserMachine};
use crate::test_util::Rng;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Debug;

/// What's on one line, oldest first, with when each message arrives.
type Line<C> = VecDeque<(u64, Message<char, C>)>;

/// A user machine that keeps every command it executes and answers with how
/// many it has executed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Log(pub Vec<u32>);

impl UserMachine for Log {
    type Command = u32;
    type Response = usize;

    fn execute(&mut self, command: &u32) -> usize {
        self.0.push(*command);
        self.0.len()
    }
}

pub struct Sim<M: UserMachine> {
    /// Real time.
    pub now: u64,
    pub books: BTreeMap<char, Bookkeeper<char, M>>,
    /// When each bookkeeper executed each command, in real time.
    pub executed_at: BTreeMap<(char, Stamped<M::Command>), u64>,
    /// How many messages each bookkeeper has sent, each to all the others.
    pub sent: BTreeMap<char, usize>,
    delta: u64,
    /// How far each bookkeeper's clock reads from real time: ahead if
    /// positive, behind if negative.
    skew: BTreeMap<char, i64>,
    crashed: BTreeSet<char>,
    cut: BTreeSet<(char, char)>,
    slow: BTreeSet<(char, char)>,
    /// What's on each line, by `(from, to)`.
    lines: BTreeMap<(char, char), Line<M::Command>>,
    /// Users' commands on their way: when each arrives, and where.
    commands: Vec<(u64, char, Stamped<M::Command>)>,
    rng: Rng,
}

impl<M: UserMachine + Clone> Sim<M>
where
    M::Command: Ord,
{
    /// Every bookkeeper of `config`, each starting its copy from `machine`,
    /// with delays drawn from `seed`.
    pub fn new(config: Config<char>, machine: M, seed: u64) -> Self {
        let delta = config.delta();
        let books = config
            .bookkeepers()
            .map(|&p| (p, Bookkeeper::new(p, config.clone(), machine.clone())))
            .collect();
        Self {
            now: 0,
            books,
            executed_at: BTreeMap::new(),
            sent: BTreeMap::new(),
            delta,
            skew: BTreeMap::new(),
            crashed: BTreeSet::new(),
            cut: BTreeSet::new(),
            slow: BTreeSet::new(),
            lines: BTreeMap::new(),
            commands: Vec::new(),
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
        }
    }
}

impl<M: UserMachine> Sim<M>
where
    M::Command: Ord + Debug,
    M::Response: Debug,
{
    /// The bookkeepers' names, in order.
    pub fn ids(&self) -> Vec<char> {
        self.books.keys().copied().collect()
    }

    pub fn book(&self, p: char) -> &Bookkeeper<char, M> {
        &self.books[&p]
    }

    /// The commands `p` has executed, in order.
    pub fn log(&self, p: char) -> Vec<Stamped<M::Command>> {
        self.books[&p]
            .executed()
            .iter()
            .map(|(c, _)| c.clone())
            .collect()
    }

    /// A user's `command` reaches bookkeeper `to` at real time `at`.
    pub fn submit(&mut self, at: u64, to: char, command: Stamped<M::Command>) {
        assert!(at > self.now, "that time has passed");
        self.commands.push((at, to, command));
    }

    /// Make `p`'s clock read `skew` from real time: ahead if positive, behind
    /// if negative, though never before 0. Changing it during a run could
    /// turn a clock back, so it's set before the run starts.
    pub fn set_skew(&mut self, p: char, skew: i64) {
        assert_eq!(self.now, 0, "clocks are set before the run starts");
        self.skew.insert(p, skew);
    }

    pub fn crash(&mut self, p: char) {
        self.crashed.insert(p);
    }

    pub fn restore(&mut self, p: char) {
        self.crashed.remove(&p);
    }

    pub fn is_crashed(&self, p: char) -> bool {
        self.crashed.contains(&p)
    }

    /// Hold everything on the line from `from` to `to` until it's mended.
    pub fn cut(&mut self, from: char, to: char) {
        self.cut.insert((from, to));
    }

    pub fn mend(&mut self, from: char, to: char) {
        self.cut.remove(&(from, to));
    }

    /// Make the line from `from` to `to` take up to `5δ`, or back to less
    /// than `δ`, for what's already on it too.
    pub fn set_slow(&mut self, from: char, to: char, slow: bool) {
        if slow {
            self.slow.insert((from, to));
        } else if self.slow.remove(&(from, to)) {
            self.hurry(from, to);
        }
    }

    /// Restore every bookkeeper, and make every line work.
    pub fn heal(&mut self) {
        self.crashed.clear();
        self.cut.clear();
        for (from, to) in std::mem::take(&mut self.slow) {
            self.hurry(from, to);
        }
    }

    /// Bring forward whatever is on a line that has stopped being slow, so it
    /// arrives within `δ`. Arrival times stay in order.
    fn hurry(&mut self, from: char, to: char) {
        let by = self.now + self.delta - 1;
        if let Some(line) = self.lines.get_mut(&(from, to)) {
            for (at, _) in line.iter_mut() {
                *at = (*at).min(by);
            }
        }
    }

    /// What's on the line from `from` to `to`, oldest first.
    pub fn in_flight(
        &self,
        from: char,
        to: char,
    ) -> impl Iterator<Item = &Message<char, M::Command>> {
        self.lines
            .get(&(from, to))
            .into_iter()
            .flatten()
            .map(|(_, msg)| msg)
    }

    pub fn run_until(&mut self, t: u64) {
        while self.now < t {
            self.step();
        }
    }

    /// One unit of time: each running bookkeeper takes what has arrived for
    /// it, then reads its clock.
    pub fn step(&mut self) {
        self.now += 1;
        let now = self.now;
        for p in self.ids() {
            if self.crashed.contains(&p) {
                continue;
            }
            let clock = now.saturating_add_signed(self.skew.get(&p).copied().unwrap_or(0));
            let book = self.books.get_mut(&p).unwrap();
            let done = book.executed().len();
            let mut out = Vec::new();
            let (due, later) = std::mem::take(&mut self.commands)
                .into_iter()
                .partition(|&(at, to, _)| at <= now && to == p);
            self.commands = later;
            for (_, _, command) in due {
                out.extend(book.submit(clock, command));
            }
            for (arc, line) in self.lines.iter_mut() {
                if arc.1 != p || self.cut.contains(arc) {
                    continue;
                }
                while line.front().is_some_and(|&(at, _)| at <= now) {
                    let (_, msg) = line.pop_front().unwrap();
                    out.extend(book.receive(clock, msg));
                }
            }
            out.extend(book.advance_to(clock));
            for (command, _) in &book.executed()[done..] {
                self.executed_at.insert((p, command.clone()), now);
            }
            // Fail on a storm of messages rather than run out of memory.
            assert!(out.len() < 1000, "{p} sent {} messages at {now}", out.len());
            *self.sent.entry(p).or_default() += out.len();
            for msg in out {
                for to in self.books.keys().copied().filter(|&q| q != p) {
                    let delay = if self.slow.contains(&(p, to)) {
                        1 + self.rng.below(5 * self.delta as usize) as u64
                    } else {
                        1 + self.rng.below(self.delta as usize - 1) as u64
                    };
                    let line = self.lines.entry((p, to)).or_default();
                    // In order: nothing arrives before what was sent ahead of it.
                    let at = line.back().map_or(0, |&(at, _)| at).max(now + delay);
                    line.push_back((at, msg.clone()));
                }
            }
        }
    }

    /// Every two bookkeepers have executed the same commands, with the same
    /// responses, up to where one stopped, in stamp order.
    pub fn assert_agree(&self, context: &str)
    where
        M::Response: PartialEq,
    {
        for (p, a) in &self.books {
            let log = a.executed();
            assert!(
                log.windows(2).all(|w| w[0].0 < w[1].0),
                "{context}: {p} executed out of order: {log:?}"
            );
            for (q, b) in self.books.range(p..).skip(1) {
                let n = log.len().min(b.executed().len());
                assert_eq!(
                    log[..n],
                    b.executed()[..n],
                    "{context}: {p} and {q} disagree"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELTA: u64 = 10;

    fn three(seed: u64) -> Sim<Log> {
        Sim::new(
            Config::majority(['A', 'B', 'C'], DELTA, 40),
            Log::default(),
            seed,
        )
    }

    /// The latest message from `from` to reach `to` in each step of a run
    /// until `until`: when it was sent, and when it arrived.
    fn arrivals(sim: &mut Sim<Log>, from: char, to: char, until: u64) -> Vec<(u64, u64)> {
        let heard = |sim: &Sim<Log>| sim.book(to).heard().find(|(p, _)| **p == from).unwrap().1;
        let mut seen = Vec::new();
        while sim.now < until {
            let before = heard(sim);
            sim.step();
            if heard(sim) != before {
                seen.push((heard(sim), sim.now));
            }
        }
        seen
    }

    #[test]
    fn a_working_line_takes_less_than_delta() {
        let mut sim = three(1);
        let seen = arrivals(&mut sim, 'A', 'B', 2000);
        assert!(seen.len() > 50);
        for (sent, at) in seen {
            assert!(
                (1..DELTA).contains(&(at - sent)),
                "sent {sent}, arrived {at}"
            );
        }
    }

    /// A slow line takes longer than `δ`. Once it's working again, what it
    /// held arrives within `δ`.
    #[test]
    fn a_slow_line_catches_up_once_it_works() {
        let mut sim = three(2);
        sim.set_slow('A', 'B', true);
        let seen = arrivals(&mut sim, 'A', 'B', 300);
        assert!(seen.iter().any(|&(sent, at)| at - sent >= DELTA));
        while sim.in_flight('A', 'B').count() == 0 {
            sim.step();
        }
        let held: Vec<u64> = sim.in_flight('A', 'B').map(|m| m.sent).collect();
        let mended = sim.now;
        sim.set_slow('A', 'B', false);
        sim.run_until(mended + DELTA - 1);
        assert!(sim.in_flight('A', 'B').all(|m| !held.contains(&m.sent)));
    }

    /// A cut line holds what's sent on it, and once mended delivers what's
    /// overdue at once and the rest within `δ`.
    #[test]
    fn a_cut_line_holds_messages_until_mended() {
        let mut sim = three(3);
        sim.cut('A', 'B');
        sim.run_until(100);
        let held: Vec<u64> = sim.in_flight('A', 'B').map(|m| m.sent).collect();
        assert!(held.len() > 1);
        let heard_a = |sim: &Sim<Log>| sim.book('B').heard().find(|(p, _)| **p == 'A').unwrap().1;
        assert_eq!(heard_a(&sim), 0);
        sim.mend('A', 'B');
        let seen = arrivals(&mut sim, 'A', 'B', 100 + DELTA - 1);
        assert_eq!(seen[0].1, 101);
        assert_eq!(heard_a(&sim), *held.last().unwrap());
    }

    /// A user's command to a crashed bookkeeper waits until it's restored, in
    /// time here for it to vote.
    #[test]
    fn a_command_for_a_crashed_bookkeeper_waits_for_it() {
        let mut sim = three(4);
        sim.crash('B');
        sim.submit(2, 'B', Stamped::new(1, 7));
        sim.run_until(5);
        sim.restore('B');
        sim.run_until(100);
        for p in sim.ids() {
            assert_eq!(sim.book(p).machine().0, [7], "{p}");
        }
    }
}
