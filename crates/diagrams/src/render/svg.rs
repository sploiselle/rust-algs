//! Hand-written SVG.
//!
//! No `id` attributes (arrowheads are explicit polygons, not `<marker>`s),
//! every class prefixed `ds-` and every rule scoped under `.ds-figure`, so
//! several figures can be inlined on one page and the page's own stylesheet
//! leaves them alone. Colours come from `currentColor` and a few `--ds-*`
//! custom properties with fallbacks, so an inlined figure follows the page's
//! theme and can be recoloured from its stylesheet. A figure loaded through
//! `<img>` is isolated from the page and renders in plain black.
//!
//! A space-time diagram is laid out sequentially: the `i`-th recorded event
//! sits in the `i`-th column, so every message points right and the figure
//! reads in the order its scenario was written. Text is then kept clear of the
//! arrows by an explicit collision pass. Each event's name and annotation
//! start on whichever side of the marker no arrow passes through; any text an
//! arrow still crosses is moved to the other side of its lane, and failing
//! that the events are pushed apart until the arrow clears. Message labels and
//! callout marks are placed last, in the nearest free spot. The test
//! `committed_figures_have_no_collisions` checks the whole catalog.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::Render;
use crate::model::{DotLine, EventId, Message, SpaceTime};

pub struct Svg;

const COL_W: f64 = 72.0; // minimum pitch between consecutive events
const R: f64 = 5.0; // event marker radius
const RING: f64 = 9.0; // callout ring radius
const GAP: f64 = 12.0; // clear space between neighbouring text on one lane
const NEAR: f64 = 12.0; // an event's first text block starts this far from its lane
const STACK_GAP: f64 = 4.0; // between stacked text blocks
const CLEAR: f64 = 2.0; // an arrow must miss text by this much
const PUSH: f64 = 16.0; // how far events are pushed apart per collision
const MIN_W: f64 = 420.0; // captions wrap to the figure's width, but not below this
const NAME_PX: f64 = 13.0;
const ANN_PX: f64 = 11.0;
const LABEL_PX: f64 = 11.0;
const MARK_PX: f64 = 10.0;
const CAPTION_LH: f64 = 18.0;

const STYLE: &[&str] = &[
    r#".ds-figure { max-width: 100%; height: auto; font-family: system-ui, -apple-system, "Segoe UI", sans-serif; font-size: 13px; }"#,
    ".ds-figure .ds-lane { stroke: currentColor; stroke-width: 1; opacity: var(--ds-lane-opacity, .35); }",
    ".ds-figure .ds-lane-arrow { fill: currentColor; opacity: var(--ds-lane-opacity, .35); }",
    ".ds-figure .ds-lane-name { fill: currentColor; font-weight: 600; }",
    ".ds-figure .ds-event { fill: var(--ds-event, currentColor); }",
    ".ds-figure .ds-name, .ds-figure .ds-label, .ds-figure .ds-callout { fill: currentColor; }",
    ".ds-figure .ds-label { font-size: 11px; }",
    r#".ds-figure .ds-annotation { font-family: ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace; font-size: 11px; fill: var(--ds-annotation, currentColor); }"#,
    ".ds-figure .ds-message { stroke: var(--ds-message, currentColor); stroke-width: 1.5; fill: none; }",
    ".ds-figure .ds-arrow { fill: var(--ds-message, currentColor); }",
    ".ds-figure .ds-callout-ring { fill: none; stroke: var(--ds-accent, currentColor); stroke-width: 1.5; stroke-dasharray: 2 2; }",
    ".ds-figure .ds-callout-mark { fill: var(--ds-accent, currentColor); font-size: 10px; font-weight: 700; }",
    ".ds-figure .ds-dot { fill: currentColor; }",
    ".ds-figure .ds-dot.ds-gap { fill: none; stroke: currentColor; stroke-dasharray: 3 2; }",
    ".ds-figure .ds-claim { fill: currentColor; }",
    ".ds-figure .ds-wrong { fill: var(--ds-wrong, currentColor); font-weight: 700; }",
];

/// One element per line, so test diffs point at the element that changed.
macro_rules! emit {
    ($out:expr, $($arg:tt)*) => {
        writeln!($out, $($arg)*).expect("writing to a String cannot fail")
    };
}

// ---------------------------------------------------------------------------
// Text, measured without a font engine.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Font {
    Sans,
    Mono,
}

/// Estimated width of the widest line of `s` at `px`: chars times an average
/// advance. Monospace is measured at 0.66em, which is generous. Proportional
/// text at 0.56em about matches short words and over-estimates long prose.
fn text_width(s: &str, px: f64, font: Font) -> f64 {
    let chars = s.split('\n').map(|l| l.chars().count()).max().unwrap_or(0) as f64;
    let advance = match font {
        Font::Sans => 0.56,
        Font::Mono => 0.66,
    };
    (chars * px * advance).ceil()
}

fn line_height(px: f64) -> f64 {
    px + 2.0
}

/// Greedy word wrap to an estimated width. `\n` is a hard break. A single word
/// longer than `max_w` gets a line of its own and overflows.
fn wrap(text: &str, max_w: f64, px: f64, font: Font) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let longer = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && text_width(&longer, px, font) > max_w {
                lines.push(std::mem::replace(&mut line, word.to_string()));
            } else {
                line = longer;
            }
        }
        lines.push(line);
    }
    lines
}

/// XML text escaping. C0 control characters are not allowed in XML 1.0 at
/// all, so they are dropped (tab excepted).
fn escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Coordinates to one decimal, without a trailing `.0`.
fn num(v: f64) -> String {
    if v.abs() < 0.05 {
        return "0".to_string();
    }
    let s = format!("{v:.1}");
    match s.strip_suffix(".0") {
        Some(whole) => whole.to_string(),
        None => s,
    }
}

/// A block of one or more lines in one font.
#[derive(Clone, Debug)]
struct Text {
    lines: Vec<String>,
    px: f64,
    font: Font,
    class: &'static str,
}

impl Text {
    fn new(s: &str, px: f64, font: Font, class: &'static str) -> Text {
        Text {
            lines: s.split('\n').map(str::to_string).collect(),
            px,
            font,
            class,
        }
    }

    fn width(&self) -> f64 {
        self.lines
            .iter()
            .map(|l| text_width(l, self.px, self.font))
            .fold(0.0, f64::max)
    }

    /// From the top of the first line's ascent to the bottom of the last line's descent.
    fn height(&self) -> f64 {
        (self.lines.len() as f64 - 1.0) * line_height(self.px) + self.px
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Anchor {
    Start,
    Middle,
    End,
}

/// A text block pinned to the page: `y` is the first line's baseline.
#[derive(Clone, Debug)]
struct Placed {
    text: Text,
    x: f64,
    y: f64,
    anchor: Anchor,
}

impl Placed {
    fn rect(&self) -> Rect {
        let w = self.text.width();
        let x0 = match self.anchor {
            Anchor::Start => self.x,
            Anchor::Middle => self.x - w / 2.0,
            Anchor::End => self.x - w,
        };
        let y0 = self.y - 0.75 * self.text.px;
        Rect {
            x0,
            y0,
            x1: x0 + w,
            y1: y0 + self.text.height(),
        }
    }

    fn emit(&self, out: &mut String) {
        let anchor = match self.anchor {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        };
        let line = |s: &str| {
            if s.is_empty() {
                "&#8203;".to_string()
            } else {
                escape(s)
            }
        };
        let mut body = line(&self.text.lines[0]);
        for extra in &self.text.lines[1..] {
            write!(
                body,
                r#"<tspan x="{}" dy="{}">{}</tspan>"#,
                num(self.x),
                num(line_height(self.text.px)),
                line(extra)
            )
            .expect("writing to a String cannot fail");
        }
        emit!(
            out,
            r#"  <text class="{}" x="{}" y="{}" text-anchor="{anchor}">{body}</text>"#,
            self.text.class,
            num(self.x),
            num(self.y)
        );
    }
}

// ---------------------------------------------------------------------------
// Geometry.

type Pt = (f64, f64);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn grow(self, m: f64) -> Rect {
        Rect {
            x0: self.x0 - m,
            y0: self.y0 - m,
            x1: self.x1 + m,
            y1: self.y1 + m,
        }
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    /// Does the segment `a`–`b` pass through this rectangle? (Liang–Barsky.)
    fn crossed_by(&self, a: Pt, b: Pt) -> bool {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [
            (-dx, a.0 - self.x0),
            (dx, self.x1 - a.0),
            (-dy, a.1 - self.y0),
            (dy, self.y1 - a.1),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        t0 <= t1
    }
}

fn unit(a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    (dx / len, dy / len)
}

fn is_free(r: &Rect, taken: &[Rect], segments: &[(Pt, Pt)], own: Option<usize>) -> bool {
    !taken.iter().any(|t| r.overlaps(t))
        && !segments
            .iter()
            .enumerate()
            .any(|(i, &(a, b))| Some(i) != own && r.crossed_by(a, b))
}

// ---------------------------------------------------------------------------
// Space-time layout.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Name,
    Annotation,
}

/// Where a text block sits relative to its event's marker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    /// Centred on the marker.
    Middle,
    /// Ends just right of the marker, so the text lies to its left.
    Left,
    /// Starts just left of the marker, so the text lies to its right.
    Right,
}

impl Side {
    /// Text sits above or below its marker; an arrow may pass through the left
    /// or right quadrant on that side. Put the text where no arrow is. With
    /// both quadrants busy it stays centred, and the collision pass moves it.
    fn clear_of(left_busy: bool, right_busy: bool) -> Side {
        match (left_busy, right_busy) {
            (true, false) => Side::Right,
            (false, true) => Side::Left,
            _ => Side::Middle,
        }
    }

    fn anchor(self) -> Anchor {
        match self {
            Side::Middle => Anchor::Middle,
            Side::Left => Anchor::End,
            Side::Right => Anchor::Start,
        }
    }

    /// Where the text is anchored, relative to the marker.
    fn dx(self) -> f64 {
        match self {
            Side::Middle => 0.0,
            Side::Left => 4.0,
            Side::Right => -4.0,
        }
    }

    /// Horizontal extent `(left, right)` of text `w` wide, relative to the marker.
    fn extent(self, w: f64) -> (f64, f64) {
        match self {
            Side::Middle => (-w / 2.0, w / 2.0),
            Side::Left => (4.0 - w, 4.0),
            Side::Right => (-4.0, w - 4.0),
        }
    }
}

/// The text blocks on one side of an event, from the lane outward.
struct Stack {
    blocks: Vec<(Kind, Text)>,
    side: Side,
}

impl Stack {
    fn width(&self) -> f64 {
        self.blocks
            .iter()
            .map(|(_, t)| t.width())
            .fold(0.0, f64::max)
    }

    fn extent(&self) -> (f64, f64) {
        if self.blocks.is_empty() {
            (0.0, 0.0)
        } else {
            self.side.extent(self.width())
        }
    }

    /// How far from the lane the stack reaches.
    fn depth(&self) -> f64 {
        if self.blocks.is_empty() {
            return 0.0;
        }
        NEAR + self.blocks.iter().map(|(_, t)| t.height()).sum::<f64>()
            + STACK_GAP * (self.blocks.len() as f64 - 1.0)
    }

    /// Each block's first-line baseline as an offset from the lane; negative is above.
    fn baselines(&self, above: bool) -> Vec<f64> {
        let mut cursor = NEAR;
        let mut out = Vec::with_capacity(self.blocks.len());
        for (_, t) in &self.blocks {
            let (n, px, lh) = (t.lines.len() as f64, t.px, line_height(t.px));
            if above {
                let last = -(cursor + 0.25 * px);
                out.push(last - (n - 1.0) * lh);
            } else {
                out.push(cursor + 0.75 * px);
            }
            cursor += t.height() + STACK_GAP;
        }
        out
    }
}

/// Which quadrants around each marker an arrow passes through:
/// `[upper-left, upper-right, lower-left, lower-right]`. Lanes are numbered
/// top to bottom and messages always go right, so a message to a lane above
/// leaves through the sender's upper-right and arrives through the receiver's
/// lower-left, and so on.
fn quadrants(d: &SpaceTime) -> Vec<[bool; 4]> {
    let mut q = vec![[false; 4]; d.events().len()];
    for m in d.messages() {
        if d.lane_of(m.to).0 < d.lane_of(m.from).0 {
            q[m.from.0][1] = true;
            q[m.to.0][2] = true;
        } else {
            q[m.from.0][3] = true;
            q[m.to.0][0] = true;
        }
    }
    q
}

/// What the collision pass is allowed to change.
struct Moves {
    name_below: Vec<bool>,
    annotation_above: Vec<bool>,
    extra: Vec<f64>,
}

struct Geo {
    x: Vec<f64>,
    lane_y: Vec<f64>,
    label_w: f64,
    below_h: f64,
    name: Vec<Option<Placed>>,
    annotation: Vec<Option<Placed>>,
}

fn place(d: &SpaceTime, quad: &[[bool; 4]], moves: &Moves) -> Geo {
    let events = d.events();
    let label_w = d
        .lanes()
        .iter()
        .map(|l| text_width(&l.name, NAME_PX, Font::Sans))
        .fold(0.0, f64::max)
        + 20.0;

    let (mut above, mut below) = (Vec::new(), Vec::new());
    for (i, e) in events.iter().enumerate() {
        let (mut up, mut down) = (Vec::new(), Vec::new());
        if !e.name.is_empty() {
            let t = (
                Kind::Name,
                Text::new(&e.name, NAME_PX, Font::Sans, "ds-name"),
            );
            if moves.name_below[i] {
                down.push(t)
            } else {
                up.push(t)
            }
        }
        if !e.annotation.is_empty() {
            let t = (
                Kind::Annotation,
                Text::new(&e.annotation, ANN_PX, Font::Mono, "ds-annotation"),
            );
            if moves.annotation_above[i] {
                up.push(t)
            } else {
                down.push(t)
            }
        }
        let [ul, ur, dl, dr] = quad[i];
        above.push(Stack {
            blocks: up,
            side: Side::clear_of(ul, ur),
        });
        below.push(Stack {
            blocks: down,
            side: Side::clear_of(dl, dr),
        });
    }
    let above_h = above.iter().map(Stack::depth).fold(NEAR, f64::max);
    let below_h = below.iter().map(Stack::depth).fold(NEAR, f64::max);
    let lane_h = above_h + below_h + 16.0;
    let top = above_h + 8.0;
    let lane_y: Vec<f64> = (0..d.lanes().len())
        .map(|k| top + k as f64 * lane_h)
        .collect();

    // Columns, left to right. Text must clear the lane names, consecutive
    // events keep a minimum pitch, and neighbouring stacks on one lane keep
    // GAP between them, band by band.
    let mut x: Vec<f64> = Vec::with_capacity(events.len());
    for (i, e) in events.iter().enumerate() {
        let leftmost = above[i].extent().0.min(below[i].extent().0);
        let mut xi = label_w + GAP - leftmost;
        if i > 0 {
            xi = xi.max(x[i - 1] + COL_W);
        }
        if let Some(p) = (0..i).rev().find(|&p| events[p].lane == e.lane) {
            for (band_p, band_i) in [(&above[p], &above[i]), (&below[p], &below[i])] {
                if !band_p.blocks.is_empty() && !band_i.blocks.is_empty() {
                    xi = xi.max(x[p] + band_p.extent().1 - band_i.extent().0 + GAP);
                }
            }
        }
        x.push(xi + moves.extra[i]);
    }

    let (mut name, mut annotation) = (vec![None; events.len()], vec![None; events.len()]);
    for (i, e) in events.iter().enumerate() {
        let y = lane_y[e.lane.0];
        for (stack, is_above) in [(&above[i], true), (&below[i], false)] {
            for ((kind, t), baseline) in stack.blocks.iter().zip(stack.baselines(is_above)) {
                let p = Placed {
                    text: t.clone(),
                    x: x[i] + stack.side.dx(),
                    y: y + baseline,
                    anchor: stack.side.anchor(),
                };
                match kind {
                    Kind::Name => name[i] = Some(p),
                    Kind::Annotation => annotation[i] = Some(p),
                }
            }
        }
    }
    Geo {
        x,
        lane_y,
        label_w,
        below_h,
        name,
        annotation,
    }
}

fn marker(geo: &Geo, d: &SpaceTime, e: EventId) -> Pt {
    (geo.x[e.0], geo.lane_y[d.lane_of(e).0])
}

/// The drawn part of a message: from just outside the sender's marker to the
/// arrow tip just outside the receiver's.
fn segment(geo: &Geo, d: &SpaceTime, m: &Message) -> (Pt, Pt) {
    let (a, b) = (marker(geo, d, m.from), marker(geo, d, m.to));
    let (ux, uy) = unit(a, b);
    (
        (a.0 + ux * (R + 2.0), a.1 + uy * (R + 2.0)),
        (b.0 - ux * (R + 3.0), b.1 - uy * (R + 3.0)),
    )
}

fn text_of(geo: &Geo, i: usize, kind: Kind) -> Option<&Placed> {
    match kind {
        Kind::Name => geo.name[i].as_ref(),
        Kind::Annotation => geo.annotation[i].as_ref(),
    }
}

/// The first (message, event, text) where the arrow crosses the text.
fn first_collision(d: &SpaceTime, geo: &Geo) -> Option<(usize, usize, Kind)> {
    for (mi, m) in d.messages().iter().enumerate() {
        let (a, b) = segment(geo, d, m);
        for i in 0..d.events().len() {
            for kind in [Kind::Name, Kind::Annotation] {
                if text_of(geo, i, kind).is_some_and(|p| p.rect().grow(CLEAR).crossed_by(a, b)) {
                    return Some((mi, i, kind));
                }
            }
        }
    }
    None
}

fn is_hit(d: &SpaceTime, geo: &Geo, i: usize, kind: Kind) -> bool {
    let Some(p) = text_of(geo, i, kind) else {
        return false;
    };
    let r = p.rect().grow(CLEAR);
    d.messages().iter().any(|m| {
        let (a, b) = segment(geo, d, m);
        r.crossed_by(a, b)
    })
}

struct Layout {
    x: Vec<f64>,
    lane_y: Vec<f64>,
    label_w: f64,
    width: f64,
    height: f64,
    /// Each message's drawn segment, start to arrow tip.
    segments: Vec<(Pt, Pt)>,
    /// Event names and annotations.
    texts: Vec<Placed>,
    /// Message labels, for the messages that have one.
    labels: Vec<Placed>,
    /// One per ringed event, listing its callout numbers.
    marks: Vec<Placed>,
    captions: Vec<Placed>,
}

impl Layout {
    #[cfg(test)]
    fn all_text(&self) -> impl Iterator<Item = &Placed> {
        self.texts
            .iter()
            .chain(&self.labels)
            .chain(&self.marks)
            .chain(&self.captions)
    }
}

fn layout(d: &SpaceTime) -> Layout {
    let n = d.events().len();
    let quad = quadrants(d);
    let mut moves = Moves {
        name_below: vec![false; n],
        annotation_above: vec![false; n],
        extra: vec![0.0; n],
    };
    let mut geo = place(d, &quad, &moves);
    for _ in 0..400 {
        let Some((mi, ei, kind)) = first_collision(d, &geo) else {
            break;
        };
        // First choice: the other side of the lane, if no arrow is there either.
        let moved = match kind {
            Kind::Name => &mut moves.name_below[ei],
            Kind::Annotation => &mut moves.annotation_above[ei],
        };
        if !*moved {
            *moved = true;
            let trial = place(d, &quad, &moves);
            if !is_hit(d, &trial, ei, kind) {
                geo = trial;
                continue;
            }
            match kind {
                Kind::Name => moves.name_below[ei] = false,
                Kind::Annotation => moves.annotation_above[ei] = false,
            }
        }
        // Otherwise push events apart: the receiver when the text belongs to
        // an endpoint (a shallower arrow clears it), else the event under the arrow.
        let m = &d.messages()[mi];
        let target = if ei == m.from.0 || ei == m.to.0 {
            m.to.0
        } else {
            ei
        };
        moves.extra[target] += PUSH;
        geo = place(d, &quad, &moves);
    }
    finish(d, geo)
}

fn finish(d: &SpaceTime, mut geo: Geo) -> Layout {
    let mut segments: Vec<(Pt, Pt)> = d.messages().iter().map(|m| segment(&geo, d, m)).collect();
    let mut texts: Vec<Placed> = geo
        .name
        .iter()
        .chain(&geo.annotation)
        .flatten()
        .cloned()
        .collect();
    let mut taken: Vec<Rect> = texts.iter().map(Placed::rect).collect();

    // Message labels: beside the line, pushed out along its normal, at the
    // first of several spots along it that nothing else occupies.
    let mut labels = Vec::new();
    for (mi, m) in d.messages().iter().enumerate() {
        if m.label.is_empty() {
            continue;
        }
        let (a, b) = (marker(&geo, d, m.from), marker(&geo, d, m.to));
        let (ux, uy) = unit(a, b);
        let up = (uy, -ux); // the normal that points up the page
        let text = Text::new(&m.label, LABEL_PX, Font::Sans, "ds-label");
        let candidate = |t: f64, sign: f64| Placed {
            text: text.clone(),
            x: a.0 + (b.0 - a.0) * t + up.0 * sign * 10.0,
            y: a.1 + (b.1 - a.1) * t + up.1 * sign * 10.0,
            anchor: if up.0 * sign >= 0.0 {
                Anchor::Start
            } else {
                Anchor::End
            },
        };
        let placed = [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8]
            .into_iter()
            .flat_map(|t| [(t, 1.0), (t, -1.0)])
            .map(|(t, sign)| candidate(t, sign))
            .find(|p| is_free(&p.rect().grow(CLEAR), &taken, &segments, Some(mi)))
            .unwrap_or_else(|| candidate(0.5, 1.0));
        taken.push(placed.rect());
        labels.push(placed);
    }

    // Callout marks: one per ringed event, in the first free spot around it.
    let mut marks = Vec::new();
    for i in 0..d.events().len() {
        let numbers: Vec<String> = d
            .callouts()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.a.0 == i || c.b.0 == i)
            .map(|(j, _)| (j + 1).to_string())
            .collect();
        if numbers.is_empty() {
            continue;
        }
        let text = Text::new(&numbers.join(","), MARK_PX, Font::Sans, "ds-callout-mark");
        let (x, y) = marker(&geo, d, EventId(i));
        let spots = [
            (x + RING + 3.0, y + 4.0, Anchor::Start),
            (x - RING - 3.0, y + 4.0, Anchor::End),
            (x + RING, y - RING - 1.0, Anchor::Start),
            (x + RING, y + RING + 9.0, Anchor::Start),
            (x - RING, y - RING - 1.0, Anchor::End),
            (x - RING, y + RING + 9.0, Anchor::End),
        ];
        let candidates: Vec<Placed> = spots
            .iter()
            .map(|&(px, py, anchor)| Placed {
                text: text.clone(),
                x: px,
                y: py,
                anchor,
            })
            .collect();
        let placed = candidates
            .iter()
            .find(|p| is_free(&p.rect().grow(CLEAR), &taken, &segments, None))
            .unwrap_or(&candidates[0])
            .clone();
        taken.push(placed.rect());
        marks.push(placed);
    }

    // The figure is as wide as its rightmost text; captions wrap to that. If
    // anything reaches left into the lane names, slide the whole picture right.
    let min_left = taken.iter().map(|r| r.x0).fold(f64::INFINITY, f64::min);
    let shift = (geo.label_w + GAP / 2.0 - min_left).max(0.0);
    if shift > 0.0 {
        for x in &mut geo.x {
            *x += shift;
        }
        for p in texts.iter_mut().chain(&mut labels).chain(&mut marks) {
            p.x += shift;
        }
        segments = d.messages().iter().map(|m| segment(&geo, d, m)).collect();
    }
    let width = (taken.iter().map(|r| r.x1).fold(0.0, f64::max) + shift + GAP)
        .max(MIN_W)
        .max(geo.label_w + COL_W)
        .ceil();

    let lanes_bottom = geo.lane_y.last().copied().unwrap_or(0.0) + geo.below_h + 8.0;
    let mut captions = Vec::new();
    let mut line_no = 0;
    for (j, c) in d.callouts().iter().enumerate() {
        for (k, line) in wrap(
            &format!("{}. {}", j + 1, c.text),
            width - geo.label_w - GAP,
            NAME_PX,
            Font::Sans,
        )
        .iter()
        .enumerate()
        {
            captions.push(Placed {
                text: Text::new(line, NAME_PX, Font::Sans, "ds-callout"),
                x: if k == 0 {
                    geo.label_w
                } else {
                    geo.label_w + 16.0
                }, // continuation lines hang under the text
                y: lanes_bottom + 10.0 + CAPTION_LH * line_no as f64 + 13.0,
                anchor: Anchor::Start,
            });
            line_no += 1;
        }
    }
    let captions_h = if captions.is_empty() {
        0.0
    } else {
        10.0 + CAPTION_LH * captions.len() as f64
    };
    let height = (lanes_bottom + captions_h + 12.0).ceil();

    Layout {
        x: geo.x,
        lane_y: geo.lane_y,
        label_w: geo.label_w,
        width,
        height,
        segments,
        texts,
        labels,
        marks,
        captions,
    }
}

// ---------------------------------------------------------------------------
// Emission.

fn open(out: &mut String, width: f64, height: f64, title: &str) {
    let (w, h) = (num(width), num(height));
    emit!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="ds-figure" role="img" viewBox="0 0 {w} {h}" width="{w}" height="{h}">"#
    );
    emit!(out, "  <title>{}</title>", escape(title));
    emit!(out, "  <style>");
    for rule in STYLE {
        emit!(out, "    {rule}");
    }
    emit!(out, "  </style>");
}

/// A horizontal timeline from `x0` to `x1` with an arrowhead at the right, named at the left.
fn timeline(out: &mut String, x0: f64, x1: f64, y: f64, name: &str) {
    emit!(
        out,
        r#"  <line class="ds-lane" x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
        num(x0),
        num(y),
        num(x1 - 8.0),
        num(y)
    );
    emit!(
        out,
        r#"  <polygon class="ds-lane-arrow" points="{},{} {},{} {},{}"/>"#,
        num(x1 - 8.0),
        num(y - 4.0),
        num(x1),
        num(y),
        num(x1 - 8.0),
        num(y + 4.0)
    );
    Placed {
        text: Text::new(name, NAME_PX, Font::Sans, "ds-lane-name"),
        x: x0 - 10.0,
        y: y + 4.0,
        anchor: Anchor::End,
    }
    .emit(out);
}

impl Render for Svg {
    fn extension(&self) -> &'static str {
        "svg"
    }

    fn space_time(&self, d: &SpaceTime) -> Option<String> {
        let l = layout(d);
        let mut out = String::new();
        open(&mut out, l.width, l.height, d.title());
        for (k, lane) in d.lanes().iter().enumerate() {
            timeline(&mut out, l.label_w, l.width, l.lane_y[k], &lane.name);
        }
        for (i, e) in d.events().iter().enumerate() {
            emit!(
                out,
                r#"  <circle class="ds-event" cx="{}" cy="{}" r="{}"/>"#,
                num(l.x[i]),
                num(l.lane_y[e.lane.0]),
                num(R)
            );
        }
        for t in &l.texts {
            t.emit(&mut out);
        }
        for &(a, tip) in &l.segments {
            let (ux, uy) = unit(a, tip);
            let (nx, ny) = (-uy, ux);
            let base = (tip.0 - ux * 9.0, tip.1 - uy * 9.0);
            emit!(
                out,
                r#"  <line class="ds-message" x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
                num(a.0),
                num(a.1),
                num(base.0),
                num(base.1)
            );
            emit!(
                out,
                r#"  <polygon class="ds-arrow" points="{},{} {},{} {},{}"/>"#,
                num(tip.0),
                num(tip.1),
                num(base.0 + nx * 3.5),
                num(base.1 + ny * 3.5),
                num(base.0 - nx * 3.5),
                num(base.1 - ny * 3.5)
            );
        }
        for t in &l.labels {
            t.emit(&mut out);
        }
        let ringed: BTreeSet<usize> = d.callouts().iter().flat_map(|c| [c.a.0, c.b.0]).collect();
        for i in ringed {
            let e = &d.events()[i];
            emit!(
                out,
                r#"  <circle class="ds-callout-ring" cx="{}" cy="{}" r="{}"/>"#,
                num(l.x[i]),
                num(l.lane_y[e.lane.0]),
                num(RING)
            );
        }
        for t in l.marks.iter().chain(&l.captions) {
            t.emit(&mut out);
        }
        emit!(out, "</svg>");
        Some(out)
    }

    fn dot_line(&self, d: &DotLine) -> Option<String> {
        const STEP: f64 = 56.0;
        const R_DOT: f64 = 7.0;
        let n = d.counters();
        let titles: Vec<Text> = d
            .claims()
            .iter()
            .map(|c| Text::new(&c.title, NAME_PX, Font::Sans, "ds-lane-name"))
            .collect();
        let actor = Text::new(d.actor(), NAME_PX, Font::Sans, "ds-lane-name");
        let left_w = titles
            .iter()
            .chain([&actor])
            .map(Text::width)
            .fold(0.0, f64::max)
            + 16.0;
        let x_of = |counter: u64| left_w + (counter - 1) as f64 * STEP + STEP / 2.0;
        let line_end = x_of(n) + STEP / 2.0;
        let y_line = 40.0;
        let text_x = line_end + 14.0;
        let claim_texts: Vec<Text> = d
            .claims()
            .iter()
            .map(|c| Text::new(&c.text, ANN_PX, Font::Mono, "ds-annotation"))
            .collect();
        let text_w = claim_texts.iter().map(Text::width).fold(0.0, f64::max);
        let legend = Text::new(
            "filled: dot seen · dashed: never seen · ×: claimed but never seen",
            LABEL_PX,
            Font::Sans,
            "ds-label",
        );
        let legend_y = y_line + 44.0 + 34.0 * d.claims().len() as f64;
        let width = (text_x + text_w + GAP)
            .max(left_w + legend.width() + GAP)
            .ceil();
        let height = legend_y + 12.0;

        let mut out = String::new();
        open(&mut out, width, height, d.title());
        timeline(&mut out, left_w, line_end, y_line, d.actor());
        for c in 1..=n {
            let x = x_of(c);
            Placed {
                text: Text::new(&c.to_string(), NAME_PX, Font::Sans, "ds-name"),
                x,
                y: y_line - 16.0,
                anchor: Anchor::Middle,
            }
            .emit(&mut out);
            let class = if d.present().contains(&c) {
                "ds-dot"
            } else {
                "ds-dot ds-gap"
            };
            emit!(
                out,
                r#"  <circle class="{class}" cx="{}" cy="{}" r="{}"/>"#,
                num(x),
                num(y_line),
                num(R_DOT)
            );
        }
        for (k, claim) in d.claims().iter().enumerate() {
            let y = y_line + 44.0 + 34.0 * k as f64;
            Placed {
                text: titles[k].clone(),
                x: left_w - 10.0,
                y: y + 4.0,
                anchor: Anchor::End,
            }
            .emit(&mut out);
            for &c in &claim.counters {
                let x = x_of(c);
                if d.present().contains(&c) {
                    emit!(
                        out,
                        r#"  <circle class="ds-claim" cx="{}" cy="{}" r="4"/>"#,
                        num(x),
                        num(y)
                    );
                } else {
                    Placed {
                        text: Text::new("×", NAME_PX, Font::Sans, "ds-wrong"),
                        x,
                        y: y + 5.0,
                        anchor: Anchor::Middle,
                    }
                    .emit(&mut out);
                }
            }
            Placed {
                text: claim_texts[k].clone(),
                x: text_x,
                y: y + 4.0,
                anchor: Anchor::Start,
            }
            .emit(&mut out);
        }
        Placed {
            text: legend,
            x: left_w,
            y: legend_y,
            anchor: Anchor::Start,
        }
        .emit(&mut out);
        emit!(out, "</svg>");
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Diagram;

    /// No arrow through any text, no two texts overlapping, everything inside the figure.
    fn assert_clear(stem: &str, d: &SpaceTime) {
        let l = layout(d);
        let texts: Vec<(String, Rect)> = l
            .all_text()
            .map(|p| (p.text.lines.join("\\n"), p.rect()))
            .collect();
        for (what, r) in &texts {
            for &(a, b) in &l.segments {
                assert!(
                    !r.grow(1.0).crossed_by(a, b),
                    "{stem}: an arrow crosses {what:?}"
                );
            }
            assert!(
                r.x0 >= 0.0 && r.x1 <= l.width && r.y0 >= 0.0 && r.y1 <= l.height,
                "{stem}: {what:?} is outside the figure"
            );
        }
        for (i, (a, ra)) in texts.iter().enumerate() {
            for (b, rb) in &texts[i + 1..] {
                assert!(!ra.overlaps(rb), "{stem}: {a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn committed_figures_have_no_collisions() {
        for figure in crate::catalog::all() {
            if let Diagram::SpaceTime(d) = &figure.diagram {
                assert_clear(figure.stem, d);
            }
        }
    }

    #[test]
    fn text_width_uses_the_widest_line() {
        assert_eq!(text_width("", 11.0, Font::Mono), 0.0);
        assert_eq!(text_width("ab\nabcd", 10.0, Font::Mono), 27.0); // 4 chars × 10px × 0.66 = 26.4, rounded up
    }

    #[test]
    fn wraps_on_words_and_hard_breaks() {
        assert_eq!(wrap("aa bb cc", 40.0, 10.0, Font::Sans), ["aa bb", "cc"]); // "aa bb" is 28px; "aa bb cc" would be 45
        assert_eq!(wrap("a\nb", 400.0, 10.0, Font::Sans), ["a", "b"]);
        assert_eq!(wrap("", 40.0, 10.0, Font::Sans), [""]);
    }

    #[test]
    fn escapes_xml_and_drops_control_characters() {
        assert_eq!(escape("a < b & c > d\u{1}"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn numbers_print_compactly() {
        assert_eq!(num(77.0), "77");
        assert_eq!(num(77.25), "77.2");
        assert_eq!(num(-0.01), "0");
    }

    #[test]
    fn segments_and_rectangles() {
        let r = Rect {
            x0: 10.0,
            y0: 10.0,
            x1: 20.0,
            y1: 20.0,
        };
        assert!(r.crossed_by((0.0, 0.0), (30.0, 30.0)));
        assert!(r.crossed_by((15.0, 15.0), (50.0, 15.0))); // starts inside
        assert!(!r.crossed_by((0.0, 25.0), (30.0, 25.0)));
        assert!(!r.crossed_by((0.0, 0.0), (5.0, 5.0))); // stops short
        assert!(r.overlaps(&Rect {
            x0: 19.0,
            y0: 19.0,
            x1: 30.0,
            y1: 30.0
        }));
        assert!(!r.overlaps(&Rect {
            x0: 20.0,
            y0: 10.0,
            x1: 30.0,
            y1: 20.0
        }));
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
    fn text_starts_on_the_side_no_arrow_passes_through() {
        assert_eq!(Side::clear_of(false, false), Side::Middle);
        assert_eq!(Side::clear_of(true, false), Side::Right);
        assert_eq!(Side::clear_of(false, true), Side::Left);
        assert_eq!(Side::clear_of(true, true), Side::Middle);
    }

    /// An event that receives from a neighbour and replies has arrows on both
    /// quadrants of one side. Whatever is there, even a wide two-line
    /// annotation, must end up clear of both.
    #[test]
    fn wide_text_between_two_arrows_ends_up_clear() {
        for (busy_side_is_below, name) in [(true, "annotation band"), (false, "name band")] {
            let mut d = SpaceTime::new("t");
            let (top, bottom) = (d.lane("T"), d.lane("B"));
            let (there, here) = if busy_side_is_below {
                (bottom, top)
            } else {
                (top, bottom)
            };
            let e1 = d.event(there, "e1", "{A:1}");
            let e2 = d.event(here, "e2", "A:2→\"edit from A\"\nB:1→\"edit from B\"");
            let e3 = d.event(there, "e3", "{A:1, B:1}");
            d.message(e1, e2, "");
            d.message(e2, e3, "");
            assert_clear(name, &d);
        }
    }

    /// The textbook pattern: A sends, B has local events, then B receives.
    /// The shallow arrow must not run through the local events' text.
    #[test]
    fn a_long_arrow_clears_the_events_it_passes_over() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "{A:1}");
        d.event(b, "local", "{B:1}");
        d.event(b, "local", "{B:2}");
        d.event(b, "local", "{B:3}");
        d.event(a, "A2", "{A:2}");
        let b4 = d.event(b, "B4", "{A:1, B:4}");
        d.message(a1, b4, "m {A:1}");
        assert_clear("pass-over", &d);
    }

    /// Two concurrent sends cross; their labels must not overprint.
    #[test]
    fn crossing_messages_get_separate_labels() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "{A:1}");
        let b1 = d.event(b, "B1", "{B:1}");
        let a2 = d.event(a, "A2", "{A:2, B:1}");
        let b2 = d.event(b, "B2", "{A:1, B:2}");
        d.message(a1, b2, "m1 {A:1}");
        d.message(b1, a2, "m2 {B:1}");
        assert_clear("crossing", &d);
    }

    #[test]
    fn an_event_in_two_callouts_gets_one_mark() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "{A:1}");
        let b1 = d.event(b, "B1", "{B:1}");
        let a2 = d.event(a, "A2", "{A:2}");
        d.callout(a1, b1, "first");
        d.callout(a1, a2, "second");
        let l = layout(&d);
        assert_eq!(l.marks.len(), 3);
        assert_eq!(l.marks[0].text.lines, ["1,2"]);
        assert_clear("marks", &d);
    }
}
