//! Mermaid sequence diagrams.
//!
//! A space-time diagram maps onto a sequence diagram almost directly: lanes are
//! participants, messages are arrows, and each event becomes a note over its
//! participant carrying the event's name and annotation. A callout becomes a
//! note spanning its two participants, right after the later of its two
//! events. One compromise: Mermaid's arrows are horizontal and instantaneous,
//! so a message is drawn at the *receive* event. The send event's note still
//! shows the clock at the time of sending, so nothing is lost but the diagonal.
//!
//! Participants are identified by lane index (`L0`, `L1`, …) and displayed
//! under the lane's name, so any name works, including Mermaid keywords and
//! names that differ only in characters an identifier can't hold.
//!
//! The dot number line has no sequence-diagram shape, so this backend declines
//! it and no `.mmd` is written for that figure.

use super::Render;
use crate::model::{DotLine, LaneId, SpaceTime};

pub struct Mermaid;

impl Render for Mermaid {
    fn extension(&self) -> &'static str {
        "mmd"
    }

    fn space_time(&self, d: &SpaceTime) -> Option<String> {
        let id = |lane: LaneId| format!("L{}", lane.0);
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("title: \"{}\"\n", yaml_escape(d.title())));
        out.push_str("---\n");
        out.push_str("sequenceDiagram\n");
        for (k, lane) in d.lanes().iter().enumerate() {
            out.push_str(&format!(
                "    participant L{k} as {}\n",
                multiline(&lane.name)
            ));
        }
        for (i, event) in d.events().iter().enumerate() {
            for m in d.messages().iter().filter(|m| m.to.0 == i) {
                // Mermaid wants at least one character after the colon, so a
                // bare arrow carries a zero-width space rather than a trailing
                // blank an editor would strip.
                let label = if m.label.is_empty() {
                    "#8203;".to_string()
                } else {
                    multiline(&m.label)
                };
                out.push_str(&format!(
                    "    {}->>{}: {label}\n",
                    id(d.lane_of(m.from)),
                    id(event.lane)
                ));
            }
            let parts: Vec<String> = [&event.name, &event.annotation]
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(|s| multiline(s))
                .collect();
            if !parts.is_empty() {
                out.push_str(&format!(
                    "    Note over {}: {}\n",
                    id(event.lane),
                    parts.join("<br/>")
                ));
            }
            for c in d.callouts().iter().filter(|c| c.a.0.max(c.b.0) == i) {
                let (a, b) = (d.lane_of(c.a), d.lane_of(c.b));
                let over = if a == b {
                    id(a)
                } else {
                    format!("{},{}", id(a), id(b))
                };
                out.push_str(&format!("    Note over {over}: {}\n", multiline(&c.text)));
            }
        }
        Some(out)
    }

    fn dot_line(&self, _: &DotLine) -> Option<String> {
        None
    }
}

/// Mermaid's entity codes for the characters that would otherwise end a
/// statement or start an entity. Done in one pass, since the replacements
/// themselves contain `#` and `;`.
fn escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '#' => "#35;".to_string(),
            ';' => "#59;".to_string(),
            '<' => "#60;".to_string(),
            '>' => "#62;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Escaped, with `\n` as a line break. Used for every piece of text, so a
/// multi-line [`Label`](crate::label::Label) works anywhere.
fn multiline(s: &str) -> String {
    escape(s).replace('\n', "<br/>")
}

fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_mermaid_specials_in_one_pass() {
        assert_eq!(escape("a; b # c <d>"), "a#59; b #35; c #60;d#62;");
    }

    #[test]
    fn messages_are_drawn_at_the_receive_event_and_callouts_after_their_later_event() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("end"), d.lane("B"));
        let a1 = d.event(a, "A1", "{A:1}");
        let b1 = d.event(b, "B1", "{A:1, B:1}");
        let a2 = d.event(a, "A2", "x\ny");
        d.message(a1, b1, "m");
        d.message(b1, a2, "");
        d.callout(a1, b1, "note");
        let text = Mermaid.space_time(&d).unwrap();
        let find = |s: &str| {
            text.find(s)
                .unwrap_or_else(|| panic!("missing {s:?} in:\n{text}"))
        };
        assert!(find("participant L0 as end") < find("Note over L0: A1<br/>{A:1}"));
        assert!(find("Note over L0: A1<br/>{A:1}") < find("L0->>L1: m"));
        assert!(find("L0->>L1: m") < find("Note over L1: B1"));
        assert!(find("Note over L1: B1") < find("Note over L0,L1: note"));
        assert!(find("Note over L0,L1: note") < find("L1->>L0: #8203;"));
        assert!(text.contains("Note over L0: A2<br/>x<br/>y"));
    }
}
