//! Two inputs as a grid: one input's rows down the side, the other's across
//! the top, and a cell for each pair. A cell fills in once its pair is
//! compared, more strongly if the keys matched, and the pair this step
//! compares is outlined.

use std::fmt::Write as _;

use super::tracks::{CELL_H, STYLE, cell, style};
use super::{ANN_PX, Anchor, Font, LANE_PX, OFF, Placed, Text, actor_attrs, open, tag_slot};
use crate::model::Grid;
use crate::render::steps::{At, timed};

const CELL_W: f64 = 34.0;
const PITCH_X: f64 = 38.0;
const PITCH_Y: f64 = 28.0;

const GRID_STYLE: &[&str] = &[
    ".ds-figure .ds-pair .ds-cell-box { fill: var(--ds-ink, currentColor); fill-opacity: .04; stroke: var(--ds-rule, var(--ds-ink, currentColor)); stroke-opacity: .5; stroke-width: .75; }",
    ".ds-figure .ds-pair.ds-compared .ds-cell-box { fill-opacity: .12; }",
    ".ds-figure .ds-pair.ds-matched .ds-cell-box { fill-opacity: .32; }",
    ".ds-figure .ds-pair.ds-now .ds-cell-box { stroke: var(--ds-ink, currentColor); stroke-opacity: 1; stroke-width: 1.75; }",
    ".ds-figure .ds-head { fill: var(--ds-actor, var(--ds-ink, currentColor)); }",
];

fn actors(d: &Grid) -> Vec<String> {
    vec![d.down.name.clone(), d.across.name.clone()]
}

/// `d` with the actors in its text tagged with their palette slots.
pub(crate) fn grid_with_slots(d: &Grid) -> Grid {
    let actors = actors(d);
    let mut d = d.clone();
    d.for_each_span(|s| tag_slot(s, &actors));
    d
}

pub(crate) fn grid_svg(stem: &str, d: &Grid, at: At) -> String {
    let d = &grid_with_slots(d);
    let actors = actors(d);
    let last = d.last_step();
    let timed = |mark: &dyn Fn(usize) -> Option<String>| timed(at, last, OFF, mark);
    let label_w = d
        .down
        .keys
        .iter()
        .map(|&k| Text::plain(&d.key_label(k), ANN_PX, Font::Mono, "ds-head").width())
        .fold(0.0, f64::max);
    let name_w = Text::plain(&d.down.name, LANE_PX, Font::Sans, "ds-lane-name").width();
    let left = 4.0 + name_w + 12.0 + label_w + 10.0;
    let top = 4.0 + LANE_PX + 8.0 + 14.0 + 8.0;
    let rows = d.down.keys.len();
    let cols = d.across.keys.len();
    let x_of = |c: usize| left + c as f64 * PITCH_X + CELL_W / 2.0;
    let y_of = |r: usize| top + r as f64 * PITCH_Y;
    let width = (left + cols as f64 * PITCH_X + 4.0).ceil();
    let height = (top + rows as f64 * PITCH_Y + 4.0).ceil();
    let finished = d.frame_at(last);
    let matches: Vec<String> = finished
        .compared
        .iter()
        .filter(|p| p.2)
        .map(|&(r, _, _)| d.key_label(d.down.keys[r]))
        .collect();
    let desc = format!(
        "{} down, {} across, a cell for each pair of rows; {} of {} pairs compared, matching on {}.",
        d.down.name,
        d.across.name,
        finished.compared.len(),
        rows * cols,
        if matches.is_empty() {
            "none".to_string()
        } else {
            matches.join(", ")
        }
    );

    let mut out = String::new();
    open(&mut out, stem, "grid", width, height, d.title(), &desc);
    let rules: Vec<&str> = STYLE.iter().chain(GRID_STYLE).copied().collect();
    style(&mut out, &rules);

    let head = |out: &mut String, t: &crate::model::Track, key: &str, attrs: &str| {
        emit!(
            out,
            r#"  <g class="ds-axis" data-track="{key}"{}>"#,
            actor_attrs(Some(&t.name), &actors)
        );
        out.push_str(attrs);
        emit!(out, "  </g>");
    };
    let mut down = String::new();
    Placed {
        text: Text::plain(&d.down.name, LANE_PX, Font::Sans, "ds-lane-name"),
        x: 4.0,
        y: y_of(0) + (rows as f64 * PITCH_Y) / 2.0,
        anchor: Anchor::Start,
    }
    .emit(&mut down, "    ", "");
    for (r, &k) in d.down.keys.iter().enumerate() {
        Placed {
            text: Text::plain(&d.key_label(k), ANN_PX, Font::Mono, "ds-head"),
            x: left - 10.0,
            y: y_of(r) + CELL_H / 2.0 + 4.5,
            anchor: Anchor::End,
        }
        .emit(&mut down, "    ", &format!(r#" data-row="{r}""#));
    }
    head(&mut out, &d.down, "down", &down);
    let mut across = String::new();
    Placed {
        text: Text::plain(&d.across.name, LANE_PX, Font::Sans, "ds-lane-name"),
        x: left,
        y: 4.0 + LANE_PX,
        anchor: Anchor::Start,
    }
    .emit(&mut across, "    ", "");
    for (c, &k) in d.across.keys.iter().enumerate() {
        Placed {
            text: Text::plain(&d.key_label(k), ANN_PX, Font::Mono, "ds-head"),
            x: x_of(c),
            y: top - 8.0,
            anchor: Anchor::Middle,
        }
        .emit(&mut across, "    ", &format!(r#" data-row="{c}""#));
    }
    head(&mut out, &d.across, "across", &across);

    emit!(out, r#"  <g class="ds-pairs">"#);
    for r in 0..rows {
        for c in 0..cols {
            out.push_str(&timed(&|s| {
                let f = d.frame_at(s);
                let k = f.compared.iter().position(|p| (p.0, p.1) == (r, c));
                let mut state = String::from(" ds-pair");
                let mut label = String::new();
                if let Some(k) = k {
                    state.push_str(if f.compared[k].2 {
                        " ds-matched"
                    } else {
                        " ds-compared"
                    });
                    if f.compared[k].2 {
                        label = d.key_label(d.down.keys[r]);
                    }
                    if k + 1 == f.compared.len() {
                        state.push_str(" ds-now");
                    }
                }
                let attrs = format!(r#" data-down="{r}" data-across="{c}""#);
                Some(cell(&state, &attrs, x_of(c), y_of(r), CELL_W, &label))
            }));
        }
    }
    emit!(out, "  </g>");
    emit!(out, "</svg>");
    out
}
