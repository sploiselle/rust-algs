//! A hash join's two phases, top to bottom: the build input's rows, the table
//! they go into, and the probe input's rows, which look themselves up in it.
//! The table's entries sit in the order they went in, each under the row it
//! came from; a probe's lookup is a line up to the entry it found, or to the
//! table's edge when it found none. Joined keys go in the `out` row under the
//! probe row that found them.

use std::fmt::Write as _;

use super::tracks::{CELL_H, STYLE, cell, style};
use super::{
    ANN_PX, Anchor, Font, LANE_PX, NAME_PX, OFF, Placed, Text, actor_attrs, num, open, tag_slot,
};
use crate::model::{BuildProbe, Lookup};
use crate::render::steps::{At, timed};

const CELL_W: f64 = 28.0;
const PITCH: f64 = 34.0;
const GAP: f64 = 34.0; // between the rows, for the lines between them

const BP_STYLE: &[&str] = &[
    ".ds-figure .ds-table { fill: none; stroke: var(--ds-rule, var(--ds-ink, currentColor)); stroke-width: 1; stroke-dasharray: 4 3; }",
    ".ds-figure :is(.ds-insert, .ds-lookup) { fill: none; stroke: var(--ds-actor, var(--ds-ink, currentColor)); stroke-width: 1.5; }",
    ".ds-figure .ds-miss { stroke: var(--ds-actor, var(--ds-ink, currentColor)); stroke-width: 1.5; }",
];

/// The inputs' actors in name order, so that A and B keep their colours
/// whichever one builds.
fn actors(d: &BuildProbe) -> Vec<String> {
    let mut a = vec![d.build.name.clone(), d.probe.name.clone()];
    a.sort();
    a
}

/// `d` with the actors in its text tagged with their palette slots.
pub(crate) fn build_probe_with_slots(d: &BuildProbe) -> BuildProbe {
    let actors = actors(d);
    let mut d = d.clone();
    d.for_each_span(|s| tag_slot(s, &actors));
    d
}

pub(crate) fn build_probe_svg(stem: &str, d: &BuildProbe, at: At) -> String {
    let d = &build_probe_with_slots(d);
    let actors = actors(d);
    let last = d.last_step();
    let timed = |mark: &dyn Fn(usize) -> Option<String>| timed(at, last, OFF, mark);
    let left = [d.build.name.as_str(), d.probe.name.as_str(), "table", "out"]
        .iter()
        .map(|n| Text::plain(n, LANE_PX, Font::Sans, "ds-lane-name").width())
        .fold(0.0, f64::max)
        + 16.0;
    let x_of = |r: usize| left + r as f64 * PITCH + PITCH / 2.0;
    let y_build = 4.0;
    let y_table = y_build + CELL_H + GAP;
    let table_h = CELL_H + 8.0;
    let y_probe = y_table + table_h + GAP;
    let y_out = y_probe + CELL_H + 6.0;
    let n = d.build.keys.len().max(d.probe.keys.len());
    let width = (left + n as f64 * PITCH + 4.0).ceil();
    let height = (y_out + 22.0).ceil();
    let finished = d.frame_at(last);
    let joined: Vec<String> = finished
        .joined
        .iter()
        .map(|&(p, _)| d.key_label(d.probe.keys[p]))
        .collect();
    let desc = format!(
        "{}'s rows go into a table, then each of {}'s rows looks its key up there. Joined: {}.",
        d.build.name,
        d.probe.name,
        if joined.is_empty() {
            "nothing".to_string()
        } else {
            joined.join(", ")
        }
    );

    let mut out = String::new();
    open(
        &mut out,
        stem,
        "build-probe",
        width,
        height,
        d.title(),
        &desc,
    );
    let rules: Vec<&str> = STYLE.iter().chain(BP_STYLE).copied().collect();
    style(&mut out, &rules);

    let name = |out: &mut String, text: &str, y: f64, class: &'static str, px: f64| {
        Placed {
            text: Text::plain(text, px, Font::Sans, class),
            x: left - 10.0,
            y: y + CELL_H / 2.0 + 5.0,
            anchor: Anchor::End,
        }
        .emit(out, "    ", "");
    };

    // The build input, the table its rows go into, and the lines between.
    let build_attrs = actor_attrs(Some(&d.build.name), &actors);
    emit!(
        out,
        r#"  <g class="ds-track" data-track="build"{build_attrs}>"#
    );
    name(&mut out, &d.build.name, y_build, "ds-lane-name", LANE_PX);
    for (r, &key) in d.build.keys.iter().enumerate() {
        out.push_str(&timed(&|s| {
            let f = d.frame_at(s);
            let state = if f.now == Some(Lookup::Insert(r)) {
                " ds-at"
            } else if r < f.built {
                " ds-passed"
            } else {
                ""
            };
            let attrs = format!(r#" data-row="{r}" data-key="{key}""#);
            Some(cell(
                state,
                &attrs,
                x_of(r),
                y_build,
                CELL_W,
                &d.key_label(key),
            ))
        }));
    }
    for r in 0..d.build.keys.len() {
        out.push_str(&timed(&|s| {
            (d.frame_at(s).now == Some(Lookup::Insert(r))).then(|| {
                format!(
                    "    <path class=\"ds-insert\" data-row=\"{r}\" d=\"M{x} {}V{}\" fill=\"none\" stroke=\"currentColor\"/>\n",
                    num(y_build + CELL_H + 3.0),
                    num(y_table + 2.0),
                    x = num(x_of(r))
                )
            })
        }));
    }
    name(&mut out, "table", y_table + 4.0, "ds-out-name", NAME_PX);
    emit!(
        out,
        r#"    <rect class="ds-table" x="{}" y="{}" width="{}" height="{}" rx="5" fill="none" stroke="currentColor"/>"#,
        num(left),
        num(y_table),
        num(d.build.keys.len() as f64 * PITCH),
        num(table_h)
    );
    for (r, &key) in d.build.keys.iter().enumerate() {
        out.push_str(&timed(&|s| {
            let f = d.frame_at(s);
            (r < f.built).then(|| {
                let found = matches!(f.now, Some(Lookup::Probe { found: Some(b), .. }) if b == r);
                let state = if found {
                    " ds-at ds-entry"
                } else {
                    " ds-entry"
                };
                let attrs = format!(r#" data-row="{r}" data-key="{key}""#);
                cell(
                    state,
                    &attrs,
                    x_of(r),
                    y_table + 4.0,
                    CELL_W,
                    &d.key_label(key),
                )
            })
        }));
    }
    emit!(out, "  </g>");

    // The probe input, its lookups, and what they joined.
    let probe_attrs = actor_attrs(Some(&d.probe.name), &actors);
    emit!(
        out,
        r#"  <g class="ds-track" data-track="probe"{probe_attrs}>"#
    );
    name(&mut out, &d.probe.name, y_probe, "ds-lane-name", LANE_PX);
    for (r, &key) in d.probe.keys.iter().enumerate() {
        out.push_str(&timed(&|s| {
            let f = d.frame_at(s);
            let now = matches!(f.now, Some(Lookup::Probe { row, .. }) if row == r);
            let state = if now {
                " ds-at"
            } else if r < f.probed && f.joined.iter().any(|&(p, _)| p == r) {
                " ds-joined"
            } else if r < f.probed {
                " ds-passed"
            } else {
                ""
            };
            let attrs = format!(r#" data-row="{r}" data-key="{key}""#);
            Some(cell(
                state,
                &attrs,
                x_of(r),
                y_probe,
                CELL_W,
                &d.key_label(key),
            ))
        }));
    }
    for r in 0..d.probe.keys.len() {
        out.push_str(&timed(&|s| {
            let Some(Lookup::Probe { row, found }) = d.frame_at(s).now else {
                return None;
            };
            (row == r).then(|| match found {
                Some(b) => format!(
                    "    <path class=\"ds-lookup\" data-row=\"{r}\" data-found=\"{b}\" d=\"M{} {}L{} {}\" fill=\"none\" stroke=\"currentColor\"/>\n",
                    num(x_of(r)),
                    num(y_probe - 3.0),
                    num(x_of(b)),
                    num(y_table + table_h - 2.0)
                ),
                None => {
                    let x = x_of(r);
                    let y = y_table + table_h + 6.0;
                    format!(
                        "    <path class=\"ds-lookup ds-miss\" data-row=\"{r}\" d=\"M{x} {}V{}M{} {}l8 8M{} {}l-8 8\" fill=\"none\" stroke=\"currentColor\"/>\n",
                        num(y_probe - 3.0),
                        num(y + 4.0),
                        num(x - 4.0),
                        num(y - 4.0),
                        num(x + 4.0),
                        num(y - 4.0),
                        x = num(x)
                    )
                }
            })
        }));
    }
    emit!(out, "  </g>");

    emit!(out, r#"  <g class="ds-out">"#);
    name(&mut out, "out", y_out - 2.0, "ds-out-name", NAME_PX);
    for (r, &key) in d.probe.keys.iter().enumerate() {
        out.push_str(&timed(&|s| {
            d.frame_at(s).joined.iter().any(|&(p, _)| p == r).then(|| {
                let mut text = String::new();
                Placed {
                    text: Text::plain(&d.key_label(key), ANN_PX, Font::Mono, "ds-out-key"),
                    x: x_of(r),
                    y: y_out + 13.0,
                    anchor: Anchor::Middle,
                }
                .emit(&mut text, "    ", &format!(r#" data-row="{r}""#));
                text
            })
        }));
    }
    emit!(out, "  </g>");
    emit!(out, "</svg>");
    out
}
