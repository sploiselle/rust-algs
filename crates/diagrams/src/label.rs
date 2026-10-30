//! How a library value appears inside a figure: compact text.
//!
//! These are presentation choices (quotes around string values, `→` between a
//! dot and its value, one line per sibling) that don't belong in the library,
//! so they live here. And since a crate can't implement a foreign trait such as
//! `Display` for foreign types, they form a trait of this crate's own.

use std::collections::BTreeSet;
use std::fmt::{Debug, Display};

use algs::broadcast::Cbcast;
use algs::clock::{
    CausalContext, Causality, Dot, LamportClock, LamportStamp, Timestamp, VectorClock,
};
use algs::coordination::LamportMutex;
use algs::crdt::{LamportRegister, MvRegister};
use algs::state_machine::airline::Command;
use algs::state_machine::{Bookkeeper, Stamped, UserMachine};

use crate::model::{Rich, Span};

/// Compact text for a library value, for use inside a figure. May contain `\n`
/// where a value is naturally several lines (siblings in a register); both
/// renderers honour it in every piece of text.
pub trait Label {
    fn label(&self) -> String;

    /// The same text split into the parts a page may want to style: a clock's
    /// entries tagged with their actor, a dot's name, a register entry's dot
    /// and value. Every span is marked as a value. Joined, the spans are
    /// exactly [`Label::label`].
    fn spans(&self) -> Rich {
        let mut r = Rich::default();
        for (i, line) in self.label().split('\n').enumerate() {
            if i > 0 {
                r.lines.push(Vec::new());
            }
            r = r.then(Span::value(line));
        }
        r
    }
}

/// A library value as rich text, to drop into a sentence: `rich!["saw ", v(&clock)]`.
pub fn v(value: &impl Label) -> Rich {
    value.spans()
}

/// One `name value` line per row, the names (role `field`) padded with no-break
/// spaces so that, in the monospace a figure sets values in, every value starts
/// in the same column:
///
/// ```text
/// clock   10
/// Lamport 1
/// HLC     10,0
/// ```
///
/// No-break spaces because SVG collapses runs of ordinary ones.
pub fn fields(rows: Vec<(&str, Rich)>) -> Rich {
    let width = rows
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = Rich::default();
    for (name, value) in rows {
        let pad = "\u{a0}".repeat(width + 1 - name.chars().count());
        let line = Rich::from(Span::plain(format!("{name}{pad}")).also("field")).then(value);
        out.lines.extend(line.lines);
    }
    out
}

/// The inverse of [`fields`]: each line's field name, its padding trimmed,
/// and its value, when every line starts with a field name; `None` for any
/// other value.
pub fn split_fields(r: &Rich) -> Option<Vec<(String, Rich)>> {
    if r.lines.is_empty() {
        return None;
    }
    r.lines
        .iter()
        .map(|line| {
            let (name, value) = line.split_first()?;
            name.roles.contains(&"field").then(|| {
                (
                    name.text.trim().to_string(),
                    Rich {
                        lines: vec![value.to_vec()],
                    },
                )
            })
        })
        .collect()
}

/// `{A:2, B:3}`: entries in actor order; `{}` when empty.
impl<A: Ord + Clone + Display> Label for VectorClock<A> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let mut line = vec![Span::value("{")];
        for (i, (a, n)) in self.iter().enumerate() {
            if i > 0 {
                line.push(Span::value(", "));
            }
            line.extend(actor_parts(&a.to_string(), n, "entry"));
        }
        line.push(Span::value("}"));
        Rich { lines: vec![line] }
    }
}

/// `A:1`.
impl<A: Display> Label for Dot<A> {
    fn label(&self) -> String {
        format!("{}:{}", self.actor, self.counter)
    }

    fn spans(&self) -> Rich {
        Rich {
            lines: vec![dot_spans(self)],
        }
    }
}

/// `A:2` as two spans, the actor's name (roles `role` and `actor`) and the rest
/// (`role` and `count`), both with `data-actor`, so a page can colour just the
/// name to match the actor's lane and leave the number in ink.
fn actor_parts(actor: &str, n: u64, role: &'static str) -> [Span; 2] {
    [
        Span::part(actor, role).also("actor").with("actor", actor),
        Span::part(format!(":{n}"), role)
            .also("count")
            .with("actor", actor),
    ]
}

/// A dot's name as [`actor_parts`], both also carrying `data-dot`.
fn dot_spans<A: Display>(d: &Dot<A>) -> Vec<Span> {
    actor_parts(&d.actor.to_string(), d.counter, "dot-id")
        .into_iter()
        .map(|s| s.with("dot", d.label()))
        .collect()
}

/// The vector part, then any dots known past a gap, named as dots so they
/// can't be read as a vector: `{A:1} + dot A:3`, or `… + dots A:3, B:2`.
impl<A: Ord + Clone + Display> Label for CausalContext<A> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let cloud: Vec<Dot<A>> = self.cloud().collect();
        let mut r = self.vector().spans();
        if cloud.is_empty() {
            return r;
        }
        r = r.then(Span::value(if cloud.len() == 1 {
            " + dot "
        } else {
            " + dots "
        }));
        for (i, d) in cloud.iter().enumerate() {
            if i > 0 {
                r = r.then(Span::value(", "));
            }
            let parts = dot_spans(d).into_iter().map(|s| s.also("cloud")).collect();
            r = r.then(Rich { lines: vec![parts] });
        }
        r
    }
}

/// One `dot→value` line per live entry, in dot order; `(empty)` when empty.
impl<A: Ord + Clone + Display, V: Clone + Debug> Label for MvRegister<A, V> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let entries = sorted_entries(self);
        if entries.is_empty() {
            return Span::part("(empty)", "empty").into();
        }
        Rich {
            lines: entries
                .iter()
                .map(|(d, v)| {
                    let mut line = dot_spans(d);
                    line.push(Span::part("→", "sym"));
                    line.push(Span::part(format!("{v:?}"), "str"));
                    line
                })
                .collect(),
        }
    }
}

/// One replica of a last-writer-wins register, as [`fields`]: its Lamport
/// clock, then the value it holds as `stamp→value`, the stamp drawn as a
/// [`LamportStamp`] and the field named `x` after the register in the
/// write-up.
///
/// ```text
/// L 3
/// x (3,B)→"eggs"
/// ```
impl<A: Ord + Clone + Display, V: Clone + Debug> Label for LamportRegister<A, V> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let entry = match self.entry() {
            Some((stamp, value)) => stamp
                .spans()
                .then(Span::part("→", "sym"))
                .then(Span::part(format!("{value:?}"), "str")),
            None => Span::part("(empty)", "empty").into(),
        };
        fields(vec![
            (
                "L",
                Span::part(self.clock().time().to_string(), "lamport").into(),
            ),
            ("x", entry),
        ])
    }
}

/// `L 3`: the clock's time (role `lamport`), named as in the other Lamport
/// values, so a trace of bare clocks reads like the mutex and register figures.
impl<P: Clone> Label for LamportClock<P> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        fields(vec![(
            "L",
            Span::part(self.time().to_string(), "lamport").into(),
        )])
    }
}

/// `20,1`: wall time, then the logical counter, each its own part (`wall`,
/// `logical`) so a page can, say, set the counter apart.
impl Label for Timestamp {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        Rich {
            lines: vec![vec![
                Span::part(self.wall.to_string(), "wall"),
                Span::value(","),
                Span::part(self.logical.to_string(), "logical"),
            ]],
        }
    }
}

/// `(1,A)`: the time (role `time`), then the process that breaks ties, as an
/// actor's name, all inside a `stamp` and keyed by it (`data-stamp="1,A"`), so a
/// page can pick out one request wherever it appears, and a step-through can
/// tell a request joining a queue from the ones already there.
impl<P: Display> Label for LamportStamp<P> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let process = self.process().to_string();
        let key = format!("{},{process}", self.time());
        Rich {
            lines: vec![
                [
                    Span::part("(", "stamp"),
                    Span::part(self.time().to_string(), "stamp").also("time"),
                    Span::part(",", "stamp"),
                    Span::part(process.clone(), "stamp")
                        .also("actor")
                        .with("actor", process),
                    Span::part(")", "stamp"),
                ]
                .into_iter()
                .map(|s| s.with("stamp", &key))
                .collect(),
            ],
        }
    }
}

/// One process's state, as [`fields`]: its clock, its queue earliest first,
/// the latest stamp it has heard from each other process, and whether it
/// holds the resource.
///
/// ```text
/// L     3
/// queue (1,A) (1,B)
/// heard B:1
/// holds yes
/// ```
impl<P: Ord + Clone + Display> Label for LamportMutex<P> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let mut queue = Rich::default();
        for (i, s) in self.queue().enumerate() {
            if i > 0 {
                queue = queue.then(Span::value(" "));
            }
            queue = queue.then(s.spans());
        }
        if queue.is_empty() {
            queue = Span::part("(empty)", "empty").into();
        }
        let mut heard = Vec::new();
        for (i, (p, time)) in self.heard().enumerate() {
            if i > 0 {
                heard.push(Span::value(" "));
            }
            heard.extend(actor_parts(&p.to_string(), time, "heard"));
        }
        let holds = if self.holds() { "yes" } else { "no" };
        fields(vec![
            (
                "L",
                Span::part(self.clock().time().to_string(), "lamport").into(),
            ),
            ("queue", queue),
            ("heard", Rich { lines: vec![heard] }),
            ("holds", Span::part(holds, "holds").into()),
        ])
    }
}

/// `reserve 221 Smith`: what the request asks, then the flight and the
/// passenger.
impl Label for Command {
    fn label(&self) -> String {
        match self {
            Command::Reserve { flight, passenger } => format!("reserve {flight} {passenger}"),
            Command::Cancel { flight, passenger } => format!("cancel {flight} {passenger}"),
        }
    }
}

/// `1:reserve 221 Smith`, the paper's `T:C`: the time (role `time`), then the
/// command, all inside a `command` and keyed by it
/// (`data-command="1:reserve 221 Smith"`), so a page can pick out one command
/// wherever it appears.
impl<C: Label> Label for Stamped<C> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let command = self.command().label();
        let key = format!("{}:{command}", self.time());
        Rich {
            lines: vec![
                [
                    Span::part(self.time().to_string(), "command").also("time"),
                    Span::part(":", "command"),
                    Span::part(command, "command"),
                ]
                .into_iter()
                .map(|s| s.with("command", &key))
                .collect(),
            ],
        }
    }
}

/// `{A,B}`: the bookkeepers known to have voted for a command, each name an
/// actor's (roles `voter` and `actor`).
pub fn voters<P: Display>(voters: &BTreeSet<P>) -> Rich {
    let mut line = vec![Span::value("{")];
    for (i, p) in voters.iter().enumerate() {
        if i > 0 {
            line.push(Span::value(","));
        }
        let p = p.to_string();
        line.push(
            Span::part(p.clone(), "voter")
                .also("actor")
                .with("actor", p),
        );
    }
    line.push(Span::value("}"));
    Rich { lines: vec![line] }
}

/// A bookkeeper whose clock reads `now`, as [`fields`]: the time, which a
/// bookkeeper is handed rather than keeps; each command stamped after `known`
/// that it has heard of, with who it knows voted for it; for each other
/// bookkeeper, when it sent the latest message this one has received from it;
/// the latest time up to which it knows every accepted command; and `copy`, a
/// field showing its copy of the user machine.
///
/// A whole command is too wide to repeat under every event, so a pending one
/// is shown by its stamp's time alone, and no two pending commands may share
/// a time (this panics if they do). That time is still keyed by the whole
/// command, as [`Stamped`]'s parts are.
///
/// ```text
/// clock   12
/// pending 1 {A,C}
/// heard   A:4 B:0
/// known   0
/// booked  (empty)
/// ```
pub fn bookkeeper<P, M>(now: u64, b: &Bookkeeper<P, M>, copy: (&str, Rich)) -> Rich
where
    P: Ord + Clone + Display,
    M: UserMachine,
    M::Command: Ord + Label,
{
    let mut pending = Rich::default();
    let mut times = BTreeSet::new();
    for (i, (command, who)) in b.pending().enumerate() {
        assert!(
            times.insert(command.time()),
            "two pending commands stamped {}",
            command.time()
        );
        if i > 0 {
            pending = pending.then(Span::value(", "));
        }
        let time = Span::part(command.time().to_string(), "command")
            .also("time")
            .with("command", command.label());
        pending = pending.then(time).then(Span::value(" ")).then(voters(who));
    }
    if pending.is_empty() {
        pending = Span::part("(empty)", "empty").into();
    }
    let mut heard = Vec::new();
    for (i, (p, sent)) in b.heard().enumerate() {
        if i > 0 {
            heard.push(Span::value(" "));
        }
        heard.extend(actor_parts(&p.to_string(), sent, "heard"));
    }
    fields(vec![
        ("clock", Span::part(now.to_string(), "clock").into()),
        ("pending", pending),
        ("heard", Rich { lines: vec![heard] }),
        ("known", Span::part(b.known().to_string(), "known").into()),
        copy,
    ])
}

/// One process's side of causal broadcast, as [`fields`]: how many of each
/// process's broadcasts it has delivered, then the messages it is delaying,
/// each as `dot→payload`, the dot naming the message as its sender's `n`th
/// broadcast.
///
/// ```text
/// delivered {}
/// delayed   B:1→"answer"
/// ```
impl<P: Ord + Clone + Display, T: Debug> Label for Cbcast<P, T> {
    fn label(&self) -> String {
        self.spans().plain()
    }

    fn spans(&self) -> Rich {
        let mut delayed = Rich::default();
        for (i, m) in self.delayed().enumerate() {
            if i > 0 {
                delayed = delayed.then(Span::value(" "));
            }
            let dot = Dot {
                actor: m.sender.clone(),
                counter: m.stamp.get(&m.sender),
            };
            delayed = delayed
                .then(dot.spans())
                .then(Span::part("→", "sym"))
                .then(Span::part(format!("{:?}", m.payload), "str"));
        }
        if delayed.is_empty() {
            delayed = Span::part("(empty)", "empty").into();
        }
        fields(vec![
            ("delivered", self.delivered().spans()),
            ("delayed", delayed),
        ])
    }
}

/// The relation symbol: `→` before, `←` after, `=` equal, `∥` concurrent.
impl Label for Causality {
    fn label(&self) -> String {
        match self {
            Causality::Before => "→",
            Causality::After => "←",
            Causality::Equal => "=",
            Causality::Concurrent => "∥",
        }
        .to_string()
    }
}

/// The relation in words, for sentences: the symbols are unreadable at caption
/// size and missing from many fonts.
pub fn phrase(c: Causality) -> &'static str {
    match c {
        Causality::Before => "happened before",
        Causality::After => "happened after",
        Causality::Equal => "is the same event as",
        Causality::Concurrent => "is concurrent with",
    }
}

/// A register's live entries sorted by dot, so that two converged replicas
/// list them identically whatever order their entries arrived in.
pub fn sorted_entries<A: Ord + Clone, V: Clone>(r: &MvRegister<A, V>) -> Vec<(&Dot<A>, &V)> {
    let mut entries: Vec<(&Dot<A>, &V)> = r.entries().collect();
    entries.sort_by(|(x, _), (y, _)| (&x.actor, x.counter).cmp(&(&y.actor, y.counter)));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use algs::state_machine::Config;
    use algs::state_machine::airline::Airline;

    #[test]
    fn split_fields_inverts_fields() {
        let r = fields(vec![
            ("phys", Rich::from("10")),
            ("HLC", Rich::from("10,0")),
        ]);
        let split = split_fields(&r).unwrap();
        assert_eq!(
            split
                .iter()
                .map(|(n, v)| (n.as_str(), v.plain()))
                .collect::<Vec<_>>(),
            [("phys", "10".to_string()), ("HLC", "10,0".to_string())]
        );
        assert_eq!(split_fields(&Rich::from("{A:1}")), None);
        assert_eq!(split_fields(&Rich::default()), None);
    }

    #[test]
    fn vector_clock() {
        let clock: VectorClock<char> = [('B', 3), ('A', 2)].into_iter().collect();
        assert_eq!(clock.label(), "{A:2, B:3}");
        assert_eq!(VectorClock::<char>::new().label(), "{}");
        let spans = clock.spans();
        let parts: Vec<_> = spans.lines[0]
            .iter()
            .filter(|s| s.roles.contains(&"entry"))
            .map(|s| (s.text.as_str(), s.roles.clone(), s.data[0].1.as_str()))
            .collect();
        assert_eq!(
            parts,
            [
                ("A", vec!["entry", "actor"], "A"),
                (":2", vec!["entry", "count"], "A"),
                ("B", vec!["entry", "actor"], "B"),
                (":3", vec!["entry", "count"], "B")
            ]
        );
    }

    #[test]
    fn dot_and_causal_context() {
        assert_eq!(
            Dot {
                actor: 'A',
                counter: 1
            }
            .label(),
            "A:1"
        );
        let mut cc = CausalContext::new();
        cc.insert(Dot {
            actor: 'A',
            counter: 1,
        });
        cc.insert(Dot {
            actor: 'A',
            counter: 3,
        });
        assert_eq!(cc.label(), "{A:1} + dot A:3");
        assert!(
            cc.spans().lines[0]
                .iter()
                .any(|s| s.roles == ["dot-id", "actor", "cloud"] && s.text == "A")
        );
        cc.insert(Dot {
            actor: 'B',
            counter: 2,
        });
        assert_eq!(cc.label(), "{A:1} + dots A:3, B:2");
        cc.insert(Dot {
            actor: 'A',
            counter: 2,
        });
        cc.insert(Dot {
            actor: 'B',
            counter: 1,
        });
        assert_eq!(cc.label(), "{A:3, B:2}");
    }

    #[test]
    fn mv_register_sorts_siblings() {
        let mut r: MvRegister<char, &str> = MvRegister::new();
        assert_eq!(r.label(), "(empty)");
        r.update(&'B', |_| "b");
        let (_, empty_ctx) = MvRegister::<char, &str>::new().read(); // a write that replaces nothing
        r.write(&'A', &empty_ctx, "a");
        assert_eq!(r.label(), "A:1→\"a\"\nB:1→\"b\"");
        let roles: Vec<_> = r.spans().lines[0].iter().map(|s| s.roles.clone()).collect();
        assert_eq!(
            roles,
            [
                vec!["dot-id", "actor"],
                vec!["dot-id", "count"],
                vec!["sym"],
                vec!["str"]
            ]
        );
    }

    #[test]
    fn spans_are_all_values() {
        let clock: VectorClock<char> = [('A', 1)].into_iter().collect();
        assert!(clock.spans().lines.iter().flatten().all(|s| s.value));
    }

    #[test]
    fn hlc_timestamp() {
        let t = Timestamp {
            wall: 20,
            logical: 1,
        };
        assert_eq!(t.label(), "20,1");
    }

    #[test]
    fn fields_line_up_their_values() {
        let r = fields(vec![("a", Rich::from("1")), ("bbb", Rich::from("2"))]);
        assert_eq!(r.plain(), "a\u{a0}\u{a0}\u{a0}1\nbbb\u{a0}2");
        assert_eq!(r.lines[0][0].roles, ["field"]);
    }

    #[test]
    fn lamport_mutex() {
        assert_eq!(LamportStamp::new(1, 'A').label(), "(1,A)");
        let mut a = LamportMutex::new('A', ['A', 'B']);
        a.request();
        assert_eq!(
            a.label(),
            "L\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}1\nqueue\u{a0}(1,A)\nheard\u{a0}B:0\nholds\u{a0}no"
        );
    }

    #[test]
    fn stamped_command() {
        let command = Stamped::new(1, Command::reserve(221, "Smith"));
        assert_eq!(command.label(), "1:reserve 221 Smith");
        assert_eq!(Command::cancel(221, "Smith").label(), "cancel 221 Smith");
        let spans = command.spans();
        assert!(
            spans.lines[0]
                .iter()
                .all(|s| s.data == [("command", "1:reserve 221 Smith".to_string())])
        );
        assert_eq!(spans.lines[0][0].roles, ["command", "time"]);
    }

    #[test]
    fn bookkeeper_shows_a_pending_command_by_its_time() {
        let config = Config::majority(['A', 'B', 'C'], 10, 40);
        let mut a = Bookkeeper::new('A', config, Airline::new([(221, 1)]));
        a.submit(4, Stamped::new(1, Command::reserve(221, "Smith")));
        let r = bookkeeper(4, &a, ("booked", Rich::from("-")));
        assert_eq!(
            r.plain(),
            "clock\u{a0}\u{a0}\u{a0}4\npending\u{a0}1 {A}\nheard\u{a0}\u{a0}\u{a0}B:0 C:0\nknown\u{a0}\u{a0}\u{a0}0\nbooked\u{a0}\u{a0}-"
        );
        let time = r.lines[1].iter().find(|s| s.text == "1").unwrap();
        assert_eq!(time.data, [("command", "1:reserve 221 Smith".to_string())]);
        assert_eq!(time.roles, ["command", "time"]);
        assert!(
            r.lines[1]
                .iter()
                .any(|s| s.roles == ["voter", "actor"] && s.text == "A")
        );
    }

    #[test]
    #[should_panic(expected = "two pending commands stamped 1")]
    fn bookkeeper_needs_a_time_per_pending_command() {
        let config = Config::majority(['A', 'B', 'C'], 10, 40);
        let mut a = Bookkeeper::new('A', config, Airline::new([(221, 1)]));
        a.submit(4, Stamped::new(1, Command::reserve(221, "Smith")));
        a.submit(5, Stamped::new(1, Command::reserve(221, "Jones")));
        bookkeeper(5, &a, ("booked", Rich::from("-")));
    }

    #[test]
    fn lamport_clock() {
        let mut c = LamportClock::new('A');
        c.receive(2);
        assert_eq!(c.label(), "L\u{a0}3");
    }

    #[test]
    fn lamport_register() {
        let mut r: LamportRegister<char, &str> = LamportRegister::new('B');
        assert_eq!(r.label(), "L\u{a0}0\nx\u{a0}(empty)");
        r.tick();
        r.write("eggs");
        assert_eq!(r.label(), "L\u{a0}2\nx\u{a0}(2,B)→\"eggs\"");
    }

    #[test]
    fn cbcast() {
        let mut a: Cbcast<char, &str> = Cbcast::new('A');
        let mut b: Cbcast<char, &str> = Cbcast::new('B');
        let mut c: Cbcast<char, &str> = Cbcast::new('C');
        assert_eq!(
            c.label(),
            "delivered\u{a0}{}\ndelayed\u{a0}\u{a0}\u{a0}(empty)"
        );
        let a1 = a.broadcast("a1");
        let a2 = a.broadcast("a2");
        b.receive(a1);
        b.receive(a2.clone());
        c.receive(a2);
        c.receive(b.broadcast("b1")); // stamped {A:2, B:1}, B's first
        assert_eq!(
            c.label(),
            "delivered\u{a0}{}\ndelayed\u{a0}\u{a0}\u{a0}A:2→\"a2\" B:1→\"b1\""
        );
    }

    #[test]
    fn causality() {
        assert_eq!(Causality::Concurrent.label(), "∥");
        assert_eq!(Causality::Before.label(), "→");
        assert_eq!(phrase(Causality::Concurrent), "is concurrent with");
    }
}
