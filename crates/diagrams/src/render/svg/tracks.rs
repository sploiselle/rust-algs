//! Sorted inputs as tracks on one key axis.
//!
//! Each track is a row of cells, one per key it holds, at that key's column,
//! so equal keys line up. Above a track go the hops its gallop makes, and the
//! arc of a whole seek; below it, the stride a binary search covers and that
//! search's probes. A vertical guide marks the target key, solid once every
//! input is on it, and each joined key is written under its column in the
//! `out` row. Rows a track has skipped fade; rows it joined stay filled.

use std::fmt::Write as _;

use super::{ANN_PX, Anchor, Font, LANE_PX, NAME_PX, OFF, Placed, Text, actor_attrs, num, open};
use super::{escape_attr, tag_slot};
use crate::model::{Mark, Tracks};
use crate::render::steps::{At, timed};

const PITCH: f64 = 26.0; // between neighbouring keys
const CELL_W: f64 = 22.0;
const CELL_H: f64 = 22.0;
const HOP_ROOM: f64 = 32.0; // above a track's cells, for its hops
const BELOW_ROOM: f64 = 24.0; // under them, for a stride and its probes
const BAND: f64 = HOP_ROOM + CELL_H + BELOW_ROOM;
const TOP: f64 = 4.0;
const PANEL_LH: f64 = 18.0;

const STYLE: &[&str] = &[
    ".ds-figure .ds-cell-box { fill: var(--ds-actor, var(--ds-ink, currentColor)); fill-opacity: .14; }",
    ".ds-figure .ds-cell.ds-at .ds-cell-box { fill-opacity: .42; stroke: var(--ds-actor, var(--ds-ink, currentColor)); stroke-width: 1.5; }",
    ".ds-figure .ds-cell.ds-joined .ds-cell-box { fill-opacity: .42; }",
    ".ds-figure .ds-cell.ds-skipped { opacity: .32; }",
    ".ds-figure .ds-key, .ds-figure .ds-out-key { font-family: var(--ds-font-mono, ui-monospace, Menlo, Consolas, monospace); }",
    ".ds-figure :is(.ds-hop, .ds-jump, .ds-stride) { fill: none; stroke: var(--ds-actor, var(--ds-ink, currentColor)); stroke-width: 1.25; }",
    ".ds-figure .ds-hop.ds-over { stroke-dasharray: 3 2; }",
    ".ds-figure :is(.ds-hop, .ds-probe).ds-now { stroke-width: 2.25; }",
    ".ds-figure .ds-jump-head { fill: var(--ds-actor, var(--ds-ink, currentColor)); }",
    ".ds-figure .ds-probe { fill: var(--ds-actor, var(--ds-ink, currentColor)); stroke: var(--ds-actor, var(--ds-ink, currentColor)); stroke-width: 1.25; }",
    ".ds-figure .ds-probe.ds-over { fill: none; }",
    ".ds-figure .ds-guide { stroke: var(--ds-muted, var(--ds-ink, currentColor)); stroke-width: 1; stroke-dasharray: 4 3; }",
    ".ds-figure .ds-guide.ds-met { stroke: var(--ds-ink, currentColor); stroke-width: 1.5; stroke-dasharray: none; }",
    ".ds-figure .ds-out-name, .ds-figure .ds-panel { fill: var(--ds-muted, var(--ds-ink, currentColor)); }",
];

/// `d` with the names in its text tagged with their tracks' palette slots.
pub(crate) fn tracks_with_slots(d: &Tracks) -> Tracks {
    let names = names(d);
    let mut d = d.clone();
    d.for_each_span(|s| tag_slot(s, &names));
    d
}

fn names(d: &Tracks) -> Vec<String> {
    d.tracks().iter().map(|t| t.name.clone()).collect()
}

/// The SVG for tracks: at [`At::End`] the finished figure, otherwise as `at`
/// asks, always on the finished figure's layout.
pub(crate) fn tracks_svg(stem: &str, d: &Tracks, at: At) -> String {
    let d = &tracks_with_slots(d);
    let names = names(d);
    let last = d.last_step();
    let timed = |mark: &dyn Fn(usize) -> Option<String>| timed(at, last, OFF, mark);
    let axis = d.axis();
    let (lo, hi) = (*axis.start(), *axis.end());
    let cols = axis.count() as f64;
    let left_w = names
        .iter()
        .map(|n| Text::plain(n, LANE_PX, Font::Sans, "ds-lane-name").width())
        .chain([Text::plain("out", NAME_PX, Font::Sans, "ds-out-name").width()])
        .fold(0.0, f64::max)
        + 16.0;
    let x_of = |key: u64| left_w + (key - lo) as f64 * PITCH + PITCH / 2.0;
    let cell_y = |t: usize| TOP + t as f64 * BAND + HOP_ROOM;
    let y_out = TOP + d.tracks().len() as f64 * BAND + 4.0;
    let panel_text = |r| Text::rich(r, ANN_PX, Font::Mono, "ds-panel");
    let panel_lines = d
        .all_frames()
        .iter()
        .map(|f| f.panel.len())
        .max()
        .unwrap_or(0);
    let panel_w = d
        .all_frames()
        .iter()
        .flat_map(|f| &f.panel)
        .map(|r| panel_text(r).width())
        .fold(0.0, f64::max);
    let y_panel = y_out + 46.0;
    let width = (left_w + cols * PITCH + 8.0)
        .max(left_w + panel_w + 8.0)
        .ceil();
    let height = if panel_lines == 0 {
        y_out + 26.0
    } else {
        y_panel + (panel_lines - 1) as f64 * PANEL_LH + 8.0
    };
    let finished = d.frame_at(last);
    let desc = format!(
        "{} on one key axis from {lo} to {}. {} Joined: {}.",
        names.join(", "),
        hi,
        d.tracks()
            .iter()
            .map(|t| format!("{} holds {}.", t.name, list(&t.keys)))
            .collect::<Vec<_>>()
            .join(" "),
        if finished.joined.is_empty() {
            "nothing".to_string()
        } else {
            list(&finished.joined)
        }
    );

    let mut out = String::new();
    open(&mut out, stem, "tracks", width, height, d.title(), &desc);
    emit!(out, "  <style>");
    emit!(out, "    @layer ds-figure {{");
    for rule in STYLE {
        emit!(out, "      {rule}");
    }
    emit!(out, "    }}");
    emit!(out, "  </style>");

    // The guide runs between the tracks, not over their cells, down to the
    // `out` row.
    let tops: Vec<f64> = (0..d.tracks().len())
        .map(cell_y)
        .chain([y_out + 2.0])
        .collect();
    let guide: String = tops
        .windows(2)
        .map(|w| format!("M{{x}} {}V{}", num(w[0] + CELL_H + 2.0), num(w[1] - 2.0)))
        .collect();
    out.push_str(&timed(&|s| {
        let (key, met) = d.frame_at(s).target?;
        let class = if met { "ds-guide ds-met" } else { "ds-guide" };
        let path = guide.replace("{x}", &num(x_of(key)));
        Some(format!(
            "  <path class=\"{class}\" data-key=\"{key}\" d=\"M{x} {}V{}{path}\" fill=\"none\" stroke=\"currentColor\"/>\n",
            num(cell_y(0) - 8.0),
            num(cell_y(0) - 2.0),
            x = num(x_of(key)),
        ))
    }));

    for (t, track) in d.tracks().iter().enumerate() {
        let attrs = actor_attrs(Some(&track.name), &names);
        let y = cell_y(t);
        let cx = |row: usize| x_of(track.keys[row]);
        emit!(
            out,
            r#"  <g class="ds-track" data-track="{}"{attrs}>"#,
            escape_attr(&track.name)
        );
        Placed {
            text: Text::plain(&track.name, LANE_PX, Font::Sans, "ds-lane-name"),
            x: left_w - 10.0,
            y: y + CELL_H / 2.0 + 5.0,
            anchor: Anchor::End,
        }
        .emit(&mut out, "    ", "");
        for (row, &key) in track.keys.iter().enumerate() {
            let x = x_of(key);
            out.push_str(&timed(&|s| {
                let f = d.frame_at(s);
                let cursor = f.cursors[t];
                let state = if row == cursor {
                    " ds-at"
                } else if row > cursor {
                    ""
                } else if f.joined.contains(&key) {
                    " ds-joined"
                } else {
                    " ds-skipped"
                };
                let mut cell = format!(
                    "    <g class=\"ds-cell{state}\" data-row=\"{row}\" data-key=\"{key}\">\n"
                );
                emit!(
                    cell,
                    r#"      <rect class="ds-cell-box" x="{}" y="{}" width="{}" height="{}" rx="3" fill="currentColor" fill-opacity=".14"/>"#,
                    num(x - CELL_W / 2.0),
                    num(y),
                    num(CELL_W),
                    num(CELL_H)
                );
                Placed {
                    text: Text::plain(&key.to_string(), ANN_PX, Font::Mono, "ds-key"),
                    x,
                    y: y + CELL_H / 2.0 + 4.5,
                    anchor: Anchor::Middle,
                }
                .emit(&mut cell, "      ", "");
                cell.push_str("    </g>\n");
                Some(cell)
            }));
        }
        // Each mark once, in the order it first appears, shown at every step
        // whose frame has it.
        let mut marks: Vec<Mark> = Vec::new();
        for f in d.all_frames() {
            for m in f.marks.iter().filter(|m| m.track() == t) {
                if !marks.contains(&m.settled()) {
                    marks.push(m.settled());
                }
            }
        }
        for m in marks {
            out.push_str(&timed(&|s| {
                d.frame_at(s)
                    .marks
                    .iter()
                    .find(|n| n.settled() == m)
                    .map(|n| mark(*n, y, cx))
            }));
        }
        emit!(out, "  </g>");
    }

    emit!(out, r#"  <g class="ds-out">"#);
    Placed {
        text: Text::plain("out", NAME_PX, Font::Sans, "ds-out-name"),
        x: left_w - 10.0,
        y: y_out + 15.0,
        anchor: Anchor::End,
    }
    .emit(&mut out, "    ", "");
    let mut joined: Vec<u64> = Vec::new();
    for f in d.all_frames() {
        for &k in &f.joined {
            if !joined.contains(&k) {
                joined.push(k);
            }
        }
    }
    for key in joined {
        out.push_str(&timed(&|s| {
            d.frame_at(s).joined.contains(&key).then(|| {
                let mut text = String::new();
                Placed {
                    text: Text::plain(&key.to_string(), ANN_PX, Font::Mono, "ds-out-key"),
                    x: x_of(key),
                    y: y_out + 15.0,
                    anchor: Anchor::Middle,
                }
                .emit(&mut text, "    ", &format!(r#" data-key="{key}""#));
                text
            })
        }));
    }
    emit!(out, "  </g>");

    if panel_lines > 0 {
        emit!(out, r#"  <g class="ds-state-lines">"#);
        for k in 0..panel_lines {
            out.push_str(&timed(&|s| {
                let line = d.frame_at(s).panel.get(k)?;
                let mut text = String::new();
                Placed {
                    text: panel_text(line),
                    x: left_w,
                    y: y_panel + k as f64 * PANEL_LH,
                    anchor: Anchor::Start,
                }
                .emit(&mut text, "    ", &format!(r#" data-line="{k}""#));
                Some(text)
            }));
        }
        emit!(out, "  </g>");
    }
    emit!(out, "</svg>");
    out
}

/// One mark on the track whose cells sit at `y`, `cx` placing its rows.
fn mark(m: Mark, y: f64, cx: impl Fn(usize) -> f64) -> String {
    let now = |now: bool| if now { " ds-now" } else { "" };
    match m {
        Mark::Hop {
            from,
            to,
            below,
            now: n,
            ..
        } => {
            let over = if below { "" } else { " ds-over" };
            format!(
                "    <path class=\"ds-hop{over}{}\" data-from=\"{from}\" data-to=\"{to}\" d=\"{}\" fill=\"none\" stroke=\"currentColor\"/>\n",
                now(n),
                arc(cx(from), cx(to), y - 2.0, to - from)
            )
        }
        Mark::Jump { from, to, .. } => {
            let (x0, x1, base) = (cx(from), cx(to), y - 2.0);
            let tip = (x1, base);
            let mut g =
                format!("    <g class=\"ds-jump\" data-from=\"{from}\" data-to=\"{to}\">\n");
            emit!(
                g,
                r#"      <path class="ds-jump-line" d="{}" fill="none" stroke="currentColor"/>"#,
                arc(x0, x1, base - 5.0, to - from)
            );
            emit!(
                g,
                r#"      <polygon class="ds-jump-head" points="{},{} {},{} {},{}"/>"#,
                num(tip.0 - 4.0),
                num(tip.1 - 6.0),
                num(tip.0 + 4.0),
                num(tip.1 - 6.0),
                num(tip.0),
                num(tip.1)
            );
            g.push_str("    </g>\n");
            g
        }
        Mark::Stride { start, end, .. } => {
            let x0 = cx(start) - CELL_W / 2.0 + 1.0;
            let x1 = cx(end - 1) + CELL_W / 2.0 - 1.0;
            let y0 = y + CELL_H + 3.0;
            format!(
                "    <path class=\"ds-stride\" data-start=\"{start}\" data-end=\"{end}\" d=\"M{} {}v4H{}v-4\" fill=\"none\" stroke=\"currentColor\"/>\n",
                num(x0),
                num(y0),
                num(x1)
            )
        }
        Mark::Probe {
            row, below, now: n, ..
        } => {
            let over = if below { "" } else { " ds-over" };
            format!(
                "    <circle class=\"ds-probe{over}{}\" data-row=\"{row}\" cx=\"{}\" cy=\"{}\" r=\"3\"/>\n",
                now(n),
                num(cx(row)),
                num(y + CELL_H + 14.0)
            )
        }
    }
}

/// A hop's arc over `rows` rows from `x0` to `x1`, its ends at `base`: higher
/// the further it goes, so nested hops don't cross.
fn arc(x0: f64, x1: f64, base: f64, rows: usize) -> String {
    let h = (7.0 + 6.0 * (rows.max(1) as f64).log2()).min(HOP_ROOM - 4.0);
    format!(
        "M{} {}Q{} {} {} {}",
        num(x0),
        num(base),
        num((x0 + x1) / 2.0),
        num(base - 2.0 * h),
        num(x1),
        num(base)
    )
}

fn list(keys: &[u64]) -> String {
    keys.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
