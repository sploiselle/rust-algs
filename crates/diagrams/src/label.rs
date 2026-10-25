//! How a library value appears inside a figure: compact text.
//!
//! These are presentation choices (quotes around string values, `→` between a
//! dot and its value, one line per sibling) that don't belong in the library,
//! so they live here. And since a crate can't implement a foreign trait such as
//! `Display` for foreign types, they form a trait of this crate's own.

use std::fmt::{Debug, Display};

use dist_sys::clock::{CausalContext, Causality, Dot, Timestamp, VectorClock};
use dist_sys::crdt::MvRegister;

/// Compact text for a library value, for use inside a figure. May contain `\n`
/// where a value is naturally several lines (siblings in a register); both
/// renderers honour it in every piece of text.
pub trait Label {
    fn label(&self) -> String;
}

/// `{A:2, B:3}`: entries in actor order; `{}` when empty.
impl<A: Ord + Clone + Display> Label for VectorClock<A> {
    fn label(&self) -> String {
        let entries: Vec<String> = self.iter().map(|(a, n)| format!("{a}:{n}")).collect();
        format!("{{{}}}", entries.join(", "))
    }
}

/// `A:1`.
impl<A: Display> Label for Dot<A> {
    fn label(&self) -> String {
        format!("{}:{}", self.actor, self.counter)
    }
}

/// The vector part, then any dots known past a gap, named as dots so they
/// can't be read as a vector: `{A:1} + dot A:3`, or `… + dots A:3, B:2`.
impl<A: Ord + Clone + Display> Label for CausalContext<A> {
    fn label(&self) -> String {
        let cloud: Vec<String> = self.cloud().map(|d| d.label()).collect();
        match cloud.len() {
            0 => self.vector().label(),
            1 => format!("{} + dot {}", self.vector().label(), cloud[0]),
            _ => format!("{} + dots {}", self.vector().label(), cloud.join(", ")),
        }
    }
}

/// One `dot→value` line per live entry, in dot order; `∅` when empty.
impl<A: Ord + Clone + Display, V: Clone + Debug> Label for MvRegister<A, V> {
    fn label(&self) -> String {
        let entries = sorted_entries(self);
        if entries.is_empty() {
            return "∅".to_string();
        }
        let lines: Vec<String> = entries
            .iter()
            .map(|(d, v)| format!("{}→{v:?}", d.label()))
            .collect();
        lines.join("\n")
    }
}

/// `20,1`: wall time, then the logical counter.
impl Label for Timestamp {
    fn label(&self) -> String {
        format!("{},{}", self.wall, self.logical)
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

    #[test]
    fn vector_clock() {
        let clock: VectorClock<char> = [('B', 3), ('A', 2)].into_iter().collect();
        assert_eq!(clock.label(), "{A:2, B:3}");
        assert_eq!(VectorClock::<char>::new().label(), "{}");
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
        assert_eq!(r.label(), "∅");
        r.update(&'B', |_| "b");
        let (_, empty_ctx) = MvRegister::<char, &str>::new().read(); // a write that replaces nothing
        r.write(&'A', &empty_ctx, "a");
        assert_eq!(r.label(), "A:1→\"a\"\nB:1→\"b\"");
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
    fn causality() {
        assert_eq!(Causality::Concurrent.label(), "∥");
        assert_eq!(Causality::Before.label(), "→");
    }
}
