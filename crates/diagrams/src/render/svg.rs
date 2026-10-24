//! Hand-written SVG.
//!
//! No `id` attributes (arrowheads are explicit polygons, not `<marker>`s) and
//! every style scoped under `.ds-figure`, so several figures can be inlined on
//! one page. Colours come from `currentColor` and a few `--ds-*` custom
//! properties with fallbacks, so an inlined figure follows the page's theme and
//! can be recoloured from the page's stylesheet. A figure loaded through
//! `<img>` is isolated from the page and renders in plain black.
//!
//! Layout is sequential: the `i`-th recorded event sits in the `i`-th column.
//! Messages must go forward in time, so every arrow points right by
//! construction and the figure reads in the order its scenario was written.
//! Around each marker, the name (above) and annotation (below) are shifted to
//! whichever side the messages touching that marker don't pass through, and
//! neighbouring events on one lane are pushed apart when their text would
//! otherwise touch. What is not handled: a message spanning more than one lane
//! can cross text on the lanes in between, so order lanes to keep messages
//! between neighbours.

use std::fmt::Write as _;

use super::Render;
use crate::model::{DotLine, SpaceTime};

pub struct Svg;

const COL_W: u32 = 72; // minimum pitch between consecutive events
const R: u32 = 5; // event marker radius
const GAP: u32 = 12; // clear space between neighbouring text on one lane
const TOP: u32 = 44; // first lane's centre line; leaves room for a raised name
const NAME_PX: u32 = 13; // lane and event names, callouts
const ANN_PX: u32 = 11; // annotations and message labels
const ANN_LH: u32 = 13; // annotation line height
const NAME_DY: u32 = 14; // name baseline sits this far above the lane
const ANN_DY: u32 = 22; // first annotation baseline sits this far below the lane
const CALLOUT_LH: u32 = 18;
const MIN_W: u32 = 420; // captions wrap to the figure's width, but not below this

const STYLE: &[&str] = &[
    r#".ds-figure { font-family: system-ui, -apple-system, "Segoe UI", sans-serif; font-size: 13px; }"#,
    ".ds-figure .lane { stroke: currentColor; stroke-width: 1; opacity: var(--ds-lane-opacity, .35); }",
    ".ds-figure .lane-arrow { fill: currentColor; opacity: var(--ds-lane-opacity, .35); }",
    ".ds-figure .lane-name { fill: currentColor; font-weight: 600; }",
    ".ds-figure .event { fill: var(--ds-event, currentColor); }",
    ".ds-figure .name, .ds-figure .label, .ds-figure .callout { fill: currentColor; }",
    ".ds-figure .label { font-size: 11px; }",
    r#".ds-figure .annotation { font-family: ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace; font-size: 11px; fill: var(--ds-annotation, currentColor); }"#,
    ".ds-figure .message { stroke: var(--ds-message, currentColor); stroke-width: 1.5; fill: none; }",
    ".ds-figure .arrow { fill: var(--ds-message, currentColor); }",
    ".ds-figure .callout-ring { fill: none; stroke: var(--ds-accent, currentColor); stroke-width: 1.5; stroke-dasharray: 2 2; }",
    ".ds-figure .callout-mark { fill: var(--ds-accent, currentColor); font-size: 10px; font-weight: 700; }",
    ".ds-figure .dot { fill: currentColor; }",
    ".ds-figure .dot.gap { fill: none; stroke: currentColor; stroke-dasharray: 3 2; }",
    ".ds-figure .claim { fill: currentColor; }",
    ".ds-figure .wrong { fill: var(--ds-wrong, currentColor); font-weight: 700; }",
];

/// One element per line, so test diffs point at the element that changed.
macro_rules! emit {
    ($out:expr, $($arg:tt)*) => {
        writeln!($out, $($arg)*).expect("writing to a String cannot fail")
    };
}

/// Estimated rendered width of `s` at `px`, with no font engine to ask: the
/// longest line, counted in chars, at a generous monospace advance.
fn text_width(s: &str, px: u32) -> u32 {
    let chars = s.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    (chars as f64 * px as f64 * 0.66).ceil() as u32
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Greedy word wrap to an estimated width. A single word longer than `max_w`
/// gets a line of its own and overflows.
fn wrap(text: &str, max_w: u32, px: u32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let longer = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && text_width(&longer, px) > max_w {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = longer;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn open(out: &mut String, width: u32, height: u32, title: &str) {
    emit!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="ds-figure" role="img" viewBox="0 0 {width} {height}" width="{width}" height="{height}">"#
    );
    emit!(out, "  <title>{}</title>", escape(title));
    emit!(out, "  <style>");
    for rule in STYLE {
        emit!(out, "    {rule}");
    }
    emit!(out, "  </style>");
}

/// A horizontal timeline from `x0` to `x1` with an arrowhead at the right, named at the left.
fn timeline(out: &mut String, x0: u32, x1: u32, y: u32, name: &str) {
    emit!(
        out,
        r#"  <line class="lane" x1="{x0}" y1="{y}" x2="{}" y2="{y}"/>"#,
        x1 - 8
    );
    emit!(
        out,
        r#"  <polygon class="lane-arrow" points="{},{} {x1},{y} {},{}"/>"#,
        x1 - 8,
        y - 4,
        x1 - 8,
        y + 4
    );
    emit!(
        out,
        r#"  <text class="lane-name" x="{}" y="{}" text-anchor="end">{}</text>"#,
        x0 - 10,
        y + 4,
        escape(name)
    );
}

/// Where a piece of text sits relative to its event's marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Centred on the marker.
    Middle,
    /// Ends just right of the marker, so the text lies to its left.
    Left,
    /// Starts just left of the marker, so the text lies to its right.
    Right,
}

impl Side {
    fn anchor(self) -> &'static str {
        match self {
            Side::Middle => "middle",
            Side::Left => "end",
            Side::Right => "start",
        }
    }

    /// Where the text is anchored, relative to the marker.
    fn anchor_dx(self) -> i32 {
        match self {
            Side::Middle => 0,
            Side::Left => 4,
            Side::Right => -4,
        }
    }

    /// Horizontal extent `(left, right)` of text `w` wide, relative to the marker.
    fn extent(self, w: u32) -> (i32, i32) {
        let w = w as i32;
        match self {
            Side::Middle => (-w / 2, w / 2),
            Side::Left => (4 - w, 4),
            Side::Right => (-4, w - 4),
        }
    }
}

/// One text block next to a marker: which side, how far from the lane, how wide.
#[derive(Clone, Copy)]
struct Slot {
    side: Side,
    dy: u32,
    w: u32,
}

impl Slot {
    /// Text sits above or below its marker; a message may pass through the
    /// left or right quadrant on that side. Put the text where no message is.
    /// If both quadrants are busy, stay centred but move one line further out,
    /// where the two messages have spread apart.
    fn clear_of(left_busy: bool, right_busy: bool, dy: u32, w: u32) -> Slot {
        match (left_busy, right_busy) {
            (false, false) => Slot {
                side: Side::Middle,
                dy,
                w,
            },
            (true, false) => Slot {
                side: Side::Right,
                dy,
                w,
            },
            (false, true) => Slot {
                side: Side::Left,
                dy,
                w,
            },
            (true, true) => Slot {
                side: Side::Middle,
                dy: dy + ANN_LH,
                w,
            },
        }
    }

    fn extent(self) -> (i32, i32) {
        self.side.extent(self.w)
    }
}

struct Layout {
    x: Vec<u32>,
    lane_y: Vec<u32>,
    name: Vec<Slot>,
    annotation: Vec<Slot>,
    /// Each callout's caption, wrapped.
    captions: Vec<Vec<String>>,
    label_w: u32,
    width: u32,
    height: u32,
    lanes_bottom: u32,
}

fn layout(d: &SpaceTime) -> Layout {
    let label_w = d
        .lanes
        .iter()
        .map(|l| text_width(&l.name, NAME_PX))
        .max()
        .unwrap_or(0)
        + 20;
    let max_lines = d
        .events
        .iter()
        .map(|e| e.annotation.lines().count())
        .max()
        .unwrap_or(0)
        .max(1) as u32;
    let lane_h = 72 + ANN_LH * max_lines;

    // Which quadrants around each marker a message passes through. Lanes are
    // numbered top to bottom and messages always go right, so a message to a
    // lower lane leaves through the lower-right quadrant, one from a lower
    // lane arrives through the lower-left, and so on.
    let (mut name, mut annotation) = (Vec::new(), Vec::new());
    for (i, e) in d.events.iter().enumerate() {
        let (mut up_left, mut up_right, mut down_left, mut down_right) =
            (false, false, false, false);
        for m in &d.messages {
            let (from, to) = (d.events[m.from.0].lane.0, d.events[m.to.0].lane.0);
            if m.from.0 == i {
                if to < from {
                    up_right = true
                } else {
                    down_right = true
                }
            }
            if m.to.0 == i {
                if from < to {
                    up_left = true
                } else {
                    down_left = true
                }
            }
        }
        name.push(Slot::clear_of(
            up_left,
            up_right,
            NAME_DY,
            text_width(&e.name, NAME_PX),
        ));
        annotation.push(Slot::clear_of(
            down_left,
            down_right,
            ANN_DY,
            text_width(&e.annotation, ANN_PX),
        ));
    }

    let mut x: Vec<u32> = Vec::with_capacity(d.events.len());
    for (i, e) in d.events.iter().enumerate() {
        let leftmost = name[i].extent().0.min(annotation[i].extent().0);
        let mut xi = (label_w + GAP) as i32 - leftmost; // text must clear the lane labels
        if i > 0 {
            xi = xi.max(x[i - 1] as i32 + COL_W as i32);
        }
        // Only a neighbour on the same lane can collide, and only within the
        // same band: names with names, annotations with annotations.
        if let Some(p) = (0..i).rev().find(|&p| d.events[p].lane == e.lane) {
            let names = name[p].extent().1 - name[i].extent().0;
            let annotations = annotation[p].extent().1 - annotation[i].extent().0;
            xi = xi.max(x[p] as i32 + names.max(annotations) + GAP as i32);
        }
        x.push(xi as u32);
    }

    // The events decide the width; captions wrap to it.
    let width = (0..d.events.len())
        .map(|i| (x[i] as i32 + name[i].extent().1.max(annotation[i].extent().1)) as u32 + GAP)
        .max()
        .unwrap_or(label_w + COL_W)
        .max(MIN_W);
    let captions: Vec<Vec<String>> = d
        .callouts
        .iter()
        .enumerate()
        .map(|(j, c)| {
            wrap(
                &format!("{}. {}", j + 1, c.text),
                width - label_w - GAP,
                NAME_PX,
            )
        })
        .collect();
    let caption_lines: u32 = captions.iter().map(|c| c.len() as u32).sum();

    let lane_y: Vec<u32> = (0..d.lanes.len() as u32)
        .map(|k| TOP + k * lane_h)
        .collect();
    let lanes_bottom = lane_y.last().copied().unwrap_or(TOP) + ANN_DY + ANN_LH * (max_lines + 1);
    let callouts_h = if caption_lines == 0 {
        0
    } else {
        10 + CALLOUT_LH * caption_lines
    };
    Layout {
        x,
        lane_y,
        name,
        annotation,
        captions,
        label_w,
        width,
        height: lanes_bottom + callouts_h + 12,
        lanes_bottom,
    }
}

impl Render for Svg {
    fn extension(&self) -> &'static str {
        "svg"
    }

    fn space_time(&self, d: &SpaceTime) -> Option<String> {
        let l = layout(d);
        let mut out = String::new();
        open(&mut out, l.width, l.height, &d.title);
        for (k, lane) in d.lanes.iter().enumerate() {
            timeline(&mut out, l.label_w, l.width, l.lane_y[k], &lane.name);
        }

        for (i, e) in d.events.iter().enumerate() {
            let (x, y) = (l.x[i], l.lane_y[e.lane.0]);
            emit!(
                out,
                r#"  <circle class="event" cx="{x}" cy="{y}" r="{R}"/>"#
            );
            if !e.name.is_empty() {
                let s = l.name[i];
                emit!(
                    out,
                    r#"  <text class="name" x="{}" y="{}" text-anchor="{}">{}</text>"#,
                    x as i32 + s.side.anchor_dx(),
                    y - s.dy,
                    s.side.anchor(),
                    escape(&e.name)
                );
            }
            if !e.annotation.is_empty() {
                let s = l.annotation[i];
                let tx = x as i32 + s.side.anchor_dx();
                let mut lines = e.annotation.lines();
                let mut text = escape(lines.next().unwrap_or(""));
                for extra in lines {
                    write!(
                        text,
                        r#"<tspan x="{tx}" dy="{ANN_LH}">{}</tspan>"#,
                        escape(extra)
                    )
                    .expect("writing to a String cannot fail");
                }
                emit!(
                    out,
                    r#"  <text class="annotation" x="{tx}" y="{}" text-anchor="{}">{text}</text>"#,
                    y + s.dy,
                    s.side.anchor()
                );
            }
        }

        for m in &d.messages {
            let (x1, y1) = (
                l.x[m.from.0] as f64,
                l.lane_y[d.events[m.from.0].lane.0] as f64,
            );
            let (x2, y2) = (l.x[m.to.0] as f64, l.lane_y[d.events[m.to.0].lane.0] as f64);
            let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
            let (ux, uy) = ((x2 - x1) / len, (y2 - y1) / len); // along the message
            let (nx, ny) = (-uy, ux); // across it
            let r = R as f64;
            let (sx, sy) = (x1 + ux * (r + 2.0), y1 + uy * (r + 2.0)); // just outside the sender's marker
            let (tx, ty) = (x2 - ux * (r + 3.0), y2 - uy * (r + 3.0)); // arrow tip, just outside the receiver's
            let (bx, by) = (tx - ux * 9.0, ty - uy * 9.0); // arrow base
            emit!(
                out,
                r#"  <line class="message" x1="{sx:.1}" y1="{sy:.1}" x2="{bx:.1}" y2="{by:.1}"/>"#
            );
            emit!(
                out,
                r#"  <polygon class="arrow" points="{tx:.1},{ty:.1} {:.1},{:.1} {:.1},{:.1}"/>"#,
                bx + nx * 3.5,
                by + ny * 3.5,
                bx - nx * 3.5,
                by - ny * 3.5
            );
            if !m.label.is_empty() {
                // On the upper side of the line, pushed out along its normal so
                // the text clears the stroke whatever the slope.
                let (nx, ny) = if ny < 0.0 { (nx, ny) } else { (-nx, -ny) };
                let (mx, my) = ((x1 + x2) / 2.0 + nx * 10.0, (y1 + y2) / 2.0 + ny * 10.0);
                let anchor = if nx >= 0.0 { "start" } else { "end" };
                emit!(
                    out,
                    r#"  <text class="label" x="{mx:.1}" y="{my:.1}" text-anchor="{anchor}">{}</text>"#,
                    escape(&m.label)
                );
            }
        }

        let mut caption_line = 0;
        for (j, c) in d.callouts.iter().enumerate() {
            for e in [c.a, c.b] {
                let (x, y) = (l.x[e.0], l.lane_y[d.events[e.0].lane.0]);
                emit!(
                    out,
                    r#"  <circle class="callout-ring" cx="{x}" cy="{y}" r="{}"/>"#,
                    R + 4
                );
                emit!(
                    out,
                    r#"  <text class="callout-mark" x="{}" y="{}">{}</text>"#,
                    x + R + 7,
                    y + 4,
                    j + 1
                );
            }
            for (k, line) in l.captions[j].iter().enumerate() {
                let y = l.lanes_bottom + 10 + CALLOUT_LH * caption_line + 13;
                let x = if k == 0 { l.label_w } else { l.label_w + 16 }; // continuation lines hang under the text
                emit!(
                    out,
                    r#"  <text class="callout" x="{x}" y="{y}">{}</text>"#,
                    escape(line)
                );
                caption_line += 1;
            }
        }

        emit!(out, "</svg>");
        Some(out)
    }

    fn dot_line(&self, d: &DotLine) -> Option<String> {
        const STEP: u32 = 56;
        const R_DOT: u32 = 7;
        let n = d.counters.max(1);
        let left_w = d
            .claims
            .iter()
            .map(|c| text_width(&c.title, NAME_PX))
            .chain([text_width(&d.actor, NAME_PX)])
            .max()
            .unwrap_or(0)
            + 16;
        let x_of = |counter: u64| left_w + (counter as u32 - 1) * STEP + STEP / 2;
        let line_end = x_of(n) + STEP / 2;
        let y_line = 40;
        let text_x = line_end + 14;
        let text_w = d
            .claims
            .iter()
            .map(|c| text_width(&c.text, ANN_PX))
            .max()
            .unwrap_or(0);
        let legend = "filled: dot seen · dashed: never seen · ×: claimed but never seen";
        let legend_y = y_line + 44 + 34 * d.claims.len() as u32;
        let width = (text_x + text_w + GAP).max(left_w + text_width(legend, ANN_PX) + GAP);
        let height = legend_y + 12;

        let mut out = String::new();
        open(&mut out, width, height, &d.title);
        timeline(&mut out, left_w, line_end, y_line, &d.actor);
        for c in 1..=n {
            let x = x_of(c);
            emit!(
                out,
                r#"  <text class="name" x="{x}" y="{}" text-anchor="middle">{c}</text>"#,
                y_line - 16
            );
            let class = if d.present.contains(&c) {
                "dot"
            } else {
                "dot gap"
            };
            emit!(
                out,
                r#"  <circle class="{class}" cx="{x}" cy="{y_line}" r="{R_DOT}"/>"#
            );
        }
        for (k, claim) in d.claims.iter().enumerate() {
            let y = y_line + 44 + 34 * k as u32;
            emit!(
                out,
                r#"  <text class="lane-name" x="{}" y="{}" text-anchor="end">{}</text>"#,
                left_w - 10,
                y + 4,
                escape(&claim.title)
            );
            for &c in &claim.counters {
                let x = x_of(c);
                if d.present.contains(&c) {
                    emit!(out, r#"  <circle class="claim" cx="{x}" cy="{y}" r="4"/>"#);
                } else {
                    emit!(
                        out,
                        r#"  <text class="wrong" x="{x}" y="{}" text-anchor="middle">×</text>"#,
                        y + 5
                    );
                }
            }
            emit!(
                out,
                r#"  <text class="annotation" x="{text_x}" y="{}">{}</text>"#,
                y + 4,
                escape(&claim.text)
            );
        }
        emit!(
            out,
            r#"  <text class="label" x="{left_w}" y="{legend_y}">{legend}</text>"#
        );
        emit!(out, "</svg>");
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_width_uses_the_longest_line() {
        assert_eq!(text_width("", 11), 0);
        assert_eq!(text_width("ab\nabcd", 10), 27); // 4 chars × 10px × 0.66 = 26.4, rounded up
    }

    #[test]
    fn wraps_on_words() {
        assert_eq!(wrap("aa bb cc", 40, 10), ["aa bb", "cc"]); // "aa bb" is 33px; "aa bb cc" would be 53
        assert_eq!(wrap("", 40, 10), Vec::<String>::new());
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn same_lane_neighbours_are_pushed_apart_by_their_annotations() {
        let mut d = SpaceTime::new("t");
        let lane = d.lane("A");
        d.event(lane, "", "x".repeat(40));
        d.event(lane, "", "x".repeat(40));
        let l = layout(&d);
        assert!(l.x[1] - l.x[0] > COL_W);
    }

    #[test]
    fn text_moves_out_of_the_way_of_messages() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "{A:1}");
        let b1 = d.event(b, "B1", "{A:1, B:1}");
        let a2 = d.event(a, "A2", "{A:2, B:1}");
        d.message(a1, b1, ""); // leaves A1 through its lower right; arrives at B1 through its upper left
        d.message(b1, a2, ""); // leaves B1 through its upper right; arrives at A2 through its lower left
        let l = layout(&d);
        assert_eq!(l.annotation[0].side, Side::Left);
        assert_eq!(
            (l.name[1].side, l.name[1].dy),
            (Side::Middle, NAME_DY + ANN_LH)
        ); // both upper quadrants busy
        assert_eq!(l.annotation[2].side, Side::Right);
        assert_eq!(l.name[0].side, Side::Middle);
    }
}
