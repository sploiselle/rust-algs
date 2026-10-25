//! An HTML `<figure>` for inlining into a page: the SVG without its captions,
//! and the notes as a `<figcaption>` list the page typesets itself.
//!
//! The notes are the same generated, asserted sentences the SVG would draw;
//! only the typesetting moves to the page, so prose can wrap, stay readable on
//! a phone and reach screen readers. Library values inside a note become
//! `<code class="ds-v">`, with their parts tagged as in the SVG (`data-actor`,
//! `data-dot`), and each note carries the `data-note` number its badges show.
//!
//! A step-through ([`steps_html`]) is the same figure with every version of
//! every mark tagged by the steps it is shown at (see
//! [`steps`](super::steps)), `data-steps` on the figure, and a hidden
//! `.ds-play` block that a page's player fills with its controls and shows:
//! one sentence per step (`.ds-beat`) saying what happened, and a table of
//! what each lane holds at that step (`.ds-state`). Without a player the page
//! shows the finished figure.
//!
//! The fragment has no blank lines, so Markdown passes it through as one raw
//! HTML block.

use super::Render;
use super::steps::{At, timed};
use super::svg::{
    actor_attrs, dot_line_svg, dot_line_with_slots, escape_attr, event_sentence, lanes_sentence,
    one_line, relation_attr, role_classes, space_time_svg, subject, with_actor_slots,
};
use crate::label::split_fields;
use crate::model::{Beat, Diagram, DotLine, LaneId, Note, Rich, SpaceTime, Span, Timeline};

pub struct Html;

impl Render for Html {
    fn extension(&self) -> &'static str {
        "html"
    }

    fn space_time(&self, stem: &str, d: &SpaceTime) -> Option<String> {
        Some(space_time_html(stem, d, At::End))
    }

    fn dot_line(&self, stem: &str, d: &DotLine) -> Option<String> {
        Some(dot_line_html(stem, d, At::End))
    }
}

/// A figure as a step-through.
pub fn steps_html(stem: &str, d: &Diagram) -> String {
    match d {
        Diagram::SpaceTime(d) => space_time_html(stem, d, At::Every),
        Diagram::DotLine(d) => dot_line_html(stem, d, At::Every),
    }
}

/// A space-time figure at `at`. At [`At::End`] it is the finished figure
/// alone; otherwise it also carries its steps and their sentences.
pub(crate) fn space_time_html(stem: &str, d: &SpaceTime, at: At) -> String {
    let d = &with_actor_slots(d);
    let notes = d.notes();
    let t = d.timeline();
    let items: Vec<String> = notes
        .iter()
        .map(|n| {
            timed(at, t.last(), "", |s| {
                (s >= t.note(n.n)).then(|| note_item(&notes, n))
            })
        })
        .collect();
    let play = (at != At::End).then(|| {
        let beats = timed(at, t.last(), "", |s| Some(beat(d, &t, s)));
        (t.last(), play(&beats, state_table(d, &t, at)))
    });
    figure(
        stem,
        "space-time",
        d.title(),
        &space_time_svg(stem, d, false, at),
        &items,
        play,
    )
}

fn note_item(notes: &[Note], n: &Note) -> String {
    format!(
        r#"<li class="ds-note" data-note="{0}"{1}><span class="ds-note-n" aria-hidden="true">{0}</span> <span class="ds-note-body">{2}</span></li>"#,
        n.n,
        relation_attr(notes, n.n),
        rich_html(&n.callout.text)
    ) + "\n"
}

/// A number line at `at`: at [`At::End`] the finished figure alone,
/// otherwise its frames, each with what it says.
pub(crate) fn dot_line_html(stem: &str, d: &DotLine, at: At) -> String {
    let d = &dot_line_with_slots(d);
    let last = d.last_step();
    let play = (at != At::End).then(|| {
        let beats = timed(at, last, "", |s| {
            let body = if s == last {
                text(&one_line(d.title()))
            } else {
                rich_html(&squeezed(&d.frame_at(s).narration))
            };
            Some(
                format!(r#"<p class="ds-beat"><span class="ds-beat-body">{body}</span></p>"#)
                    + "\n",
            )
        });
        (last, play(&beats, None))
    });
    figure(
        stem,
        "dot-line",
        d.title(),
        &dot_line_svg(stem, d, at),
        &[],
        play,
    )
}

/// The block a page's player fills in: an empty bar for its controls, the
/// sentences for the steps, and, for a space-time figure, what each lane holds.
fn play(beats: &str, state: Option<String>) -> String {
    let mut out = String::from(
        "<div class=\"ds-play\" hidden>\n<div class=\"ds-play-bar\"></div>\n<div class=\"ds-beats\">\n",
    );
    out.push_str(beats);
    out.push_str("</div>\n");
    if let Some(table) = state {
        out.push_str(&table);
    }
    out.push_str("</div>\n");
    out
}

/// A row per lane holding what the lane holds at the step: the value the
/// figure shows under its latest event (see [`SpaceTime::lane_state`]), with
/// what that event changed marked `ds-changed`. Values made of named fields
/// ([`fields`](crate::label::fields)) get a column per field. Each value
/// carries the event it came from, so a step at which a lane acts without
/// changing (a sync that brings nothing new) still shows. `None` when no lane
/// ever holds anything.
fn state_table(d: &SpaceTime, t: &Timeline, at: At) -> Option<String> {
    let values: Vec<&Rich> = d
        .lanes()
        .iter()
        .map(|l| &l.start)
        .chain(d.events().iter().map(|e| &e.annotation))
        .filter(|r| !r.is_empty())
        .collect();
    let names =
        |r: &Rich| split_fields(r).map(|f| f.into_iter().map(|(n, _)| n).collect::<Vec<_>>());
    let columns = names(values.first()?)
        .filter(|first| values.iter().all(|v| names(v).as_ref() == Some(first)));
    let actors = d.actors();
    let mut out = String::from(
        "<div class=\"ds-state\">\n<table aria-label=\"What each lane holds at this step\">\n",
    );
    if let Some(columns) = &columns {
        out.push_str("<thead><tr><td></td>");
        for c in columns {
            out.push_str(&format!(r#"<th scope="col">{}</th>"#, text(c)));
        }
        out.push_str("</tr></thead>\n");
    }
    out.push_str("<tbody>\n");
    for (k, lane) in d.lanes().iter().enumerate() {
        out.push_str(&format!(
            "<tr class=\"ds-state-lane\" data-lane=\"{}\"{}>\n<th scope=\"row\">{}</th>\n",
            escape_attr(&lane.key),
            actor_attrs(lane.actor.as_deref(), &actors),
            text(&lane.name)
        ));
        for c in 0..columns.as_ref().map_or(1, Vec::len) {
            let column = columns.is_some().then_some(c);
            out.push_str("<td><span class=\"ds-cells\">\n");
            out.push_str(&timed(at, t.last(), "", |s| {
                Some(cell(d, t, LaneId(k), s, column))
            }));
            out.push_str("</span></td>\n");
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n</div>\n");
    Some(out)
}

/// One lane's value at step `s`, or one field of it.
fn cell(d: &SpaceTime, t: &Timeline, lane: LaneId, s: usize, column: Option<usize>) -> String {
    let Some(state) = d.lane_state(t, lane, s) else {
        return "<span class=\"ds-cell\"></span>\n".to_string();
    };
    let pick = |r: &Rich| match column {
        Some(c) => split_fields(r).map(|f| f[c].1.clone()).unwrap_or_default(),
        None => r.clone(),
    };
    let value = pick(state.value);
    // A start value changes nothing; a lane's first event changes all of it.
    let shown = match (state.event, state.before) {
        (None, _) => value,
        (Some(_), before) => value.mark_changes(&before.map(pick).unwrap_or_default()),
    };
    let event = state
        .event
        .map(|e| format!(r#" data-event="{}""#, escape_attr(&d.event_key(e))))
        .unwrap_or_default();
    format!(
        r#"<span class="ds-cell"{event}>{}</span>"#,
        rich_html(&shown)
    ) + "\n"
}

/// What step `s` says: the lanes; what happened at an event, including what
/// it sent; a note, with its number; the figure's title once it is finished.
fn beat(d: &SpaceTime, t: &Timeline, s: usize) -> String {
    let (attrs, lead, body) = match t.beats()[s] {
        Beat::Start => (
            String::new(),
            String::new(),
            text(&format!("{}; time runs left to right", lanes_sentence(d))),
        ),
        Beat::Event(e) => {
            let narration = &d.events()[e.0].narration;
            let said = if narration.is_empty() {
                event_sentence(d, e, true)
            } else {
                subject(d, e).then(" ").then(narration.clone())
            };
            (
                format!(r#" data-event="{}""#, escape_attr(&d.event_key(e))),
                String::new(),
                rich_html(&squeezed(&said)),
            )
        }
        Beat::Note(n) => {
            let note = d
                .notes()
                .into_iter()
                .find(|x| x.n == n)
                .expect("a note's beat");
            (
                format!(r#" data-note="{n}""#),
                format!(r#"<span class="ds-note-n" aria-hidden="true">{n}</span> "#),
                rich_html(&note.callout.text),
            )
        }
        Beat::End => (String::new(), String::new(), text(&one_line(d.title()))),
    };
    format!(r#"<p class="ds-beat"{attrs}>{lead}<span class="ds-beat-body">{body}</span></p>"#)
        + "\n"
}

/// `r` with each run of whitespace in a span, no-break spaces included,
/// squeezed to one space: prose has no columns to line up.
fn squeezed(r: &Rich) -> Rich {
    let mut r = r.clone();
    for s in r.lines.iter_mut().flatten() {
        let mut out = String::new();
        for c in s.text.chars() {
            if c.is_whitespace() {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            } else {
                out.push(c);
            }
        }
        s.text = out;
    }
    r
}

/// The `<figure>`. `play` is a step-through's step count and player block.
fn figure(
    stem: &str,
    kind: &str,
    title: &str,
    svg: &str,
    notes: &[String],
    play: Option<(usize, String)>,
) -> String {
    let steps = match &play {
        Some((n, _)) => format!(r#" data-steps="{n}" data-steps-v="1""#),
        None => String::new(),
    };
    let mut out = format!(
        "<figure class=\"ds-fig\" data-figure=\"{}\" data-kind=\"{kind}\"{steps}>\n<div class=\"ds-fig-canvas\">\n{}\n</div>\n",
        escape_attr(stem),
        svg.trim_end(),
    );
    if let Some((_, block)) = &play {
        out.push_str(block);
    }
    out.push_str(&format!(
        "<figcaption class=\"ds-fig-caption\">\n<p class=\"ds-fig-title\">{}</p>\n",
        text(&one_line(title))
    ));
    if !notes.is_empty() {
        out.push_str("<ol class=\"ds-notes\">\n");
        for n in notes {
            out.push_str(n);
        }
        out.push_str("</ol>\n");
    }
    out.push_str("</figcaption>\n</figure>\n");
    out
}

fn text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Rich text as HTML. Runs of value spans are wrapped in one `<code>`; spans
/// with a role or data become `<span>`s carrying them; lines are joined by `<br>`.
fn rich_html(r: &Rich) -> String {
    r.lines
        .iter()
        .map(|line| {
            let mut out = String::new();
            let mut in_code = false;
            for s in line {
                if s.value != in_code {
                    out.push_str(if s.value {
                        r#"<code class="ds-v">"#
                    } else {
                        "</code>"
                    });
                    in_code = s.value;
                }
                out.push_str(&span_html(s));
            }
            if in_code {
                out.push_str("</code>");
            }
            out
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn span_html(s: &Span) -> String {
    if s.roles.is_empty() && s.data.is_empty() {
        return text(&s.text);
    }
    let class = if s.roles.is_empty() {
        String::new()
    } else {
        format!(r#" class="{}""#, role_classes(&s.roles))
    };
    let data: String = s
        .data
        .iter()
        .map(|(k, v)| format!(r#" data-{k}="{}""#, escape_attr(v)))
        .collect();
    format!("<span{class}{data}>{}</span>", text(&s.text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Form;
    use crate::model::{EventId, rich};
    use crate::render::steps::{at_step, without_player};

    fn step_throughs() -> Vec<(&'static str, SpaceTime)> {
        crate::catalog::all()
            .into_iter()
            .filter(|f| f.form() == Form::Steps)
            .filter_map(|f| match f.diagram {
                Diagram::SpaceTime(d) => Some((f.stem, d)),
                Diagram::DotLine(_) => None,
            })
            .collect()
    }

    /// The tagged figure, filtered as a page's player filters it, is exactly
    /// the figure drawn at that step.
    #[test]
    fn every_step_is_the_figure_at_that_step() {
        for (stem, d) in every_step_through() {
            let every = render(stem, &d, At::Every);
            for s in 0..=last_step(&d) {
                assert_eq!(
                    at_step(&every, s, true),
                    render(stem, &d, At::Step(s)),
                    "{stem} at step {s}"
                );
            }
        }
    }

    #[test]
    fn without_a_player_a_step_through_is_the_finished_figure() {
        for (stem, d) in every_step_through() {
            let every = render(stem, &d, At::Every);
            let finished = Html.render(stem, &d).unwrap();
            assert_eq!(at_step(&every, last_step(&d), false), finished);
            assert_eq!(without_player(&every), finished, "{stem}");
        }
    }

    fn every_step_through() -> Vec<(&'static str, Diagram)> {
        crate::catalog::all()
            .into_iter()
            .filter(|f| f.form() == Form::Steps)
            .map(|f| (f.stem, f.diagram))
            .collect()
    }

    fn render(stem: &str, d: &Diagram, at: At) -> String {
        match d {
            Diagram::SpaceTime(d) => space_time_html(stem, d, at),
            Diagram::DotLine(d) => dot_line_html(stem, d, at),
        }
    }

    fn last_step(d: &Diagram) -> usize {
        match d {
            Diagram::SpaceTime(d) => d.timeline().last(),
            Diagram::DotLine(d) => d.last_step(),
        }
    }

    /// At each frame a dot is filled or dashed as the frame has it, and a row
    /// shows × exactly where it claims a dot that wasn't seen; what a frame
    /// says quotes only values on the line.
    #[test]
    fn a_number_line_shows_each_frame() {
        for (stem, d) in every_step_through() {
            let Diagram::DotLine(d) = d else { continue };
            for s in 0..=d.last_step() {
                let html = dot_line_html(stem, &d, At::Step(s));
                let f = d.frame_at(s);
                for c in 1..=d.counters() {
                    let dot = format!(r#" data-counter="{c}" data-dot="A:{c}""#);
                    let filled = format!(r#"<circle class="ds-dot"{dot}"#);
                    assert_eq!(html.contains(&filled), f.present.contains(&c), "{stem}");
                }
                for claim in &f.claims {
                    let at = html
                        .find(&format!(
                            r#"data-row="{}""#,
                            crate::model::slug(&claim.title)
                        ))
                        .unwrap();
                    let row = &html[at..at + html[at..].find("</g>").unwrap()];
                    for c in 1..=d.counters() {
                        let wrong = claim.counters.contains(&c) && !f.present.contains(&c);
                        let needle = format!(r#"class="ds-wrong" data-counter="{c}""#);
                        assert_eq!(row.contains(&needle), wrong, "{stem}: step {s}, {c}");
                    }
                }
                let on_screen: Vec<String> = f
                    .claims
                    .iter()
                    .map(|c| c.text.plain())
                    .chain((1..=d.counters()).map(|c| format!("{}:{c}", d.actor())))
                    .collect();
                for line in &f.narration.lines {
                    for run in line
                        .chunk_by(|x, y| x.value == y.value)
                        .filter(|r| r[0].value)
                    {
                        let quoted: String = run.iter().map(|x| x.text.as_str()).collect();
                        assert!(
                            on_screen.iter().any(|v| v.contains(&quoted)),
                            "{stem}: {quoted:?}"
                        );
                    }
                }
            }
        }
    }

    /// Each mark shows from the step its event happens, or its message
    /// arrives, or its note is made; a message is in flight in between.
    #[test]
    fn marks_appear_when_they_happen() {
        for (stem, d) in step_throughs() {
            let t = d.timeline();
            for s in 0..=t.last() {
                let html = space_time_html(stem, &d, At::Step(s));
                for i in 0..d.events().len() {
                    let key = d.event_key(EventId(i));
                    assert_eq!(
                        html.contains(&format!(r#"<g class="ds-event" data-event="{key}""#)),
                        s >= t.event(EventId(i)),
                        "{stem}: {key} at step {s}"
                    );
                }
                for (mi, m) in d.messages().iter().enumerate() {
                    let key = d.message_key(mi);
                    let at = html
                        .find(&format!(r#"data-message="{key}""#))
                        .expect("a message's group is always there");
                    let group = &html[at..at + html[at..].find("</g>").unwrap()];
                    let (sent, arrived) = (t.event(m.from), t.event(m.to));
                    let landed = s >= arrived;
                    assert_eq!(group.contains(r#"class="ds-message-line" "#), landed);
                    assert_eq!(group.contains(r#"class="ds-message-head" "#), landed);
                    assert_eq!(
                        group.contains("ds-message-head ds-flight"),
                        (sent..arrived).contains(&s),
                        "{stem}: {key} in flight at step {s}"
                    );
                    if !m.label.is_empty() {
                        assert_eq!(group.contains("ds-message-label"), landed);
                    }
                }
                for n in d.notes() {
                    let made = s >= t.note(n.n);
                    let li = format!(r#"<li class="ds-note" data-note="{}""#, n.n);
                    let badge = format!(r#"<g class="ds-badge" data-note="{}""#, n.n);
                    assert_eq!(html.contains(&li), made, "{stem}: note {} at {s}", n.n);
                    assert_eq!(html.contains(&badge), made);
                    assert!(t.note(n.n) > t.event(n.callout.a.max(n.callout.b)));
                }
                // A ring marks its event from the first note it ends.
                for i in 0..d.events().len() {
                    let first = d
                        .notes()
                        .iter()
                        .filter(|n| n.callout.a.0 == i || n.callout.b.0 == i)
                        .map(|n| t.note(n.n))
                        .min();
                    let key = d.event_key(EventId(i));
                    assert_eq!(
                        html.contains(&format!(r#"<circle class="ds-ring" data-event="{key}""#)),
                        first.is_some_and(|f| s >= f),
                        "{stem}: {key}'s ring at step {s}"
                    );
                }
                assert_eq!(html.matches(r#"<p class="ds-beat""#).count(), 1);
            }
        }
    }

    /// From the step each event happens, its lane's panel shows the value
    /// drawn under it, read straight off the markup.
    #[test]
    fn a_state_panel_shows_each_value_from_its_event() {
        for (stem, d) in step_throughs() {
            let d = with_actor_slots(&d);
            let t = d.timeline();
            let html = space_time_html(stem, &d, At::Every);
            for (i, e) in d.events().iter().enumerate() {
                if e.annotation.is_empty() {
                    continue;
                }
                let key = d.event_key(EventId(i));
                let row = format!(
                    r#"<tr class="ds-state-lane" data-lane="{}""#,
                    d.lane_key(e.lane)
                );
                let at = html.find(&row).expect("a row per lane");
                let row = &html[at..at + html[at..].find("</tr>").unwrap()];
                let cells: Vec<&str> = row
                    .lines()
                    .filter(|l| {
                        l.starts_with(r#"<span class="ds-cell""#)
                            && l.contains(&format!(r#" data-event="{key}""#))
                    })
                    .collect();
                assert!(!cells.is_empty(), "{stem}: no panel value from {key}");
                for cell in &cells {
                    assert!(
                        cell.contains(&format!(r#" data-in="{}""#, t.event(EventId(i)))),
                        "{stem}: {key}'s value shows from another step: {cell}"
                    );
                }
                // What the lane held before: its last value, else its start.
                let before = d.events()[..i]
                    .iter()
                    .rev()
                    .find(|x| x.lane == e.lane && !x.annotation.is_empty())
                    .map_or(&d.lanes()[e.lane.0].start, |x| &x.annotation);
                let want: Vec<String> = match split_fields(&e.annotation) {
                    Some(fields) if fields.len() == cells.len() => {
                        let was = split_fields(before);
                        fields
                            .iter()
                            .enumerate()
                            .map(|(k, (_, v))| {
                                let w = was.as_ref().map(|w| w[k].1.clone()).unwrap_or_default();
                                rich_html(&v.mark_changes(&w))
                            })
                            .collect()
                    }
                    _ => vec![rich_html(&e.annotation.mark_changes(before))],
                };
                let got: Vec<&str> = cells
                    .iter()
                    .map(|c| &c[c.find('>').unwrap() + 1..c.len() - "</span>".len()])
                    .collect();
                assert_eq!(got, want, "{stem}: {key}");
            }
        }
    }

    #[test]
    fn narration_cites_only_values_on_screen() {
        for (stem, d) in step_throughs() {
            let t = d.timeline();
            for (i, e) in d.events().iter().enumerate() {
                let s = t.event(EventId(i));
                let on_screen: Vec<String> = d
                    .events()
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| t.event(EventId(*j)) <= s)
                    .map(|(_, x)| x.annotation.plain())
                    .chain(
                        d.messages()
                            .iter()
                            .filter(|m| t.event(m.to) <= s)
                            .map(|m| m.label.plain()),
                    )
                    // A start value shows until its lane's first value.
                    .chain(
                        d.lanes()
                            .iter()
                            .enumerate()
                            .filter(|(k, _)| {
                                !d.events().iter().enumerate().any(|(j, x)| {
                                    x.lane.0 == *k
                                        && !x.annotation.is_empty()
                                        && t.event(EventId(j)) <= s
                                })
                            })
                            .map(|(_, l)| l.start.plain()),
                    )
                    .collect();
                for line in &e.narration.lines {
                    for run in line
                        .chunk_by(|x, y| x.value == y.value)
                        .filter(|r| r[0].value)
                    {
                        let quoted: String = run.iter().map(|x| x.text.as_str()).collect();
                        assert!(
                            on_screen.iter().any(|v| v.contains(&quoted)),
                            "{stem}: {} quotes {quoted:?}, which isn't on screen at its step",
                            d.event_key(EventId(i))
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_step_through_says_what_each_step_does() {
        let mut d = SpaceTime::new("Two lanes");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "x\u{a0}\u{a0}1\ny 2");
        let b1 = d.event(b, "B1", "");
        d.message(a1, b1, "m1");
        d.callout(a1, b1, "about m1");
        let html = steps_html("fig", &Diagram::SpaceTime(d));
        assert!(html.starts_with(
            r#"<figure class="ds-fig" data-figure="fig" data-kind="space-time" data-steps="4" data-steps-v="1">"#
        ));
        for needle in [
            r#"<div class="ds-play" hidden>"#,
            r#"<p class="ds-beat" data-in="0" data-out="1"><span class="ds-beat-body">Timelines for A, B; time runs left to right</span></p>"#,
            r#"<p class="ds-beat" data-in="1" data-out="2" data-cont="" data-event="a-1"><span class="ds-beat-body"><span class="ds-lead">A1</span> on A, sending m1 to B: x 1; y 2</span></p>"#,
            r#"<p class="ds-beat" data-in="3" data-out="4" data-cont="" data-note="1"><span class="ds-note-n" aria-hidden="true">1</span> <span class="ds-beat-body">about m1</span></p>"#,
            r#"<p class="ds-beat" data-in="4" data-cont=""><span class="ds-beat-body">Two lanes</span></p>"#,
            r#"<li class="ds-note" data-in="3" data-note="1">"#,
        ] {
            assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
        }
        assert!(!html.contains("\n\n"));
    }

    #[test]
    fn values_become_code_with_their_parts() {
        let r = rich![
            "saw ",
            Span::value("{"),
            Span::part("A:1", "entry").with("actor", "A"),
            Span::value("}"),
            " & more"
        ];
        assert_eq!(
            rich_html(&r),
            r#"saw <code class="ds-v">{<span class="ds-entry" data-actor="A">A:1</span>}</code> &amp; more"#
        );
    }

    #[test]
    fn a_figure_holds_the_svg_without_captions_and_lists_the_notes() {
        let mut d = SpaceTime::new("Title &\n\nmore");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "");
        let b1 = d.event(b, "B1", "");
        d.message(a1, b1, "");
        d.callout_before(a1, b1, "first");
        let html = Html.space_time("fig", &d).unwrap();
        assert!(
            html.starts_with(r#"<figure class="ds-fig" data-figure="fig" data-kind="space-time">"#)
        );
        assert!(html.contains(r#"<p class="ds-fig-title">Title &amp; more</p>"#));
        assert!(html.contains(r#"<li class="ds-note" data-note="1" data-relation="before">"#));
        assert!(!html.contains("ds-captions"));
        assert!(
            !html.contains("\n\n"),
            "a blank line would end the Markdown HTML block"
        );
    }
}
