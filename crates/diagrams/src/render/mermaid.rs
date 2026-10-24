//! Mermaid sequence diagrams.
//!
//! A space-time diagram maps onto a sequence diagram almost directly: lanes are
//! participants, messages are arrows, and each event becomes a note over its
//! participant carrying the event's name and annotation. One compromise:
//! Mermaid's arrows are horizontal and instantaneous, so a message is drawn at
//! the *receive* event. The send event's note still shows the clock at the time
//! of sending, so nothing is lost but the diagonal.
//!
//! The dot number line has no sequence-diagram shape, so this backend declines
//! it and no `.mmd` is written for that figure.

use super::Render;
use crate::model::{DotLine, SpaceTime};

pub struct Mermaid;

impl Render for Mermaid {
    fn extension(&self) -> &'static str {
        "mmd"
    }

    fn space_time(&self, d: &SpaceTime) -> Option<String> {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("title: \"{}\"\n", yaml_escape(&d.title)));
        out.push_str("---\n");
        out.push_str("sequenceDiagram\n");
        for lane in &d.lanes {
            out.push_str(&format!(
                "    participant {} as {}\n",
                participant_id(&lane.name),
                escape(&lane.name)
            ));
        }
        for (i, event) in d.events.iter().enumerate() {
            for m in d.messages.iter().filter(|m| m.to.0 == i) {
                let from = &d.lanes[d.events[m.from.0].lane.0].name;
                let to = &d.lanes[event.lane.0].name;
                // Mermaid wants at least one character after the colon, even for a bare arrow.
                let label = if m.label.is_empty() {
                    " ".to_string()
                } else {
                    format!(" {}", escape(&m.label))
                };
                out.push_str(&format!(
                    "    {}->>{}:{}\n",
                    participant_id(from),
                    participant_id(to),
                    label
                ));
            }
            let parts: Vec<String> = [&event.name, &event.annotation]
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(|s| multiline(s))
                .collect();
            if parts.is_empty() {
                continue;
            }
            let lane = &d.lanes[event.lane.0].name;
            out.push_str(&format!(
                "    Note over {}: {}\n",
                participant_id(lane),
                parts.join("<br/>")
            ));
        }
        for c in &d.callouts {
            let a = &d.lanes[d.lane_of(c.a).0].name;
            let b = &d.lanes[d.lane_of(c.b).0].name;
            let over = if a == b {
                participant_id(a)
            } else {
                format!("{},{}", participant_id(a), participant_id(b))
            };
            out.push_str(&format!("    Note over {over}: {}\n", escape(&c.text)));
        }
        Some(out)
    }

    fn dot_line(&self, _: &DotLine) -> Option<String> {
        None
    }
}

/// A lane name as a Mermaid identifier. Every lane is also declared with
/// `participant <id> as <name>`, so the display name is untouched.
fn participant_id(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
    fn participant_ids_are_identifiers() {
        assert_eq!(participant_id("stale copy"), "stale_copy");
    }

    #[test]
    fn messages_are_drawn_at_the_receive_event() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "{A:1}");
        let b1 = d.event(b, "B1", "{A:1, B:1}");
        d.message(a1, b1, "m");
        let text = Mermaid.space_time(&d).unwrap();
        let arrow = text.find("A->>B: m").unwrap();
        assert!(text.find("Note over A: A1<br/>{A:1}").unwrap() < arrow);
        assert!(arrow < text.find("Note over B: B1").unwrap());
    }
}
