//! Hand-written SVG.
//!
//! No `id` attributes (arrowheads are explicit polygons, not `<marker>`s),
//! every class prefixed `ds-` and every rule scoped under `.ds-figure`, so
//! several figures can be inlined on one page and the page's own stylesheet
//! leaves them alone.
//!
//! Styling is layered so a page can restyle as much or as little as it likes:
//!
//! - Sizes and the fallback fonts are presentation attributes. Font sizes are
//!   layout inputs, so they come from the same constants the layout measures
//!   with, and a figure stripped of its `<style>` still reads correctly.
//! - Colours, font families and stroke widths come from `--ds-*` custom
//!   properties, each falling back to `--ds-ink` and then `currentColor`, in a
//!   `<style>` wrapped in `@layer ds-figure`, so any unlayered rule on the page
//!   wins without fighting over specificity.
//! - Every mark is grouped and keyed. Groups are named for what they are
//!   (`ds-lane`, `ds-event`, `ds-message`, `ds-ring`, `ds-badge`) and their parts
//!   `ds-<group>-<part>` (`ds-event-dot`, `ds-message-label`). Keys: lanes
//!   `data-lane`, events `data-event`, messages `data-message`/`data-from`/
//!   `data-to`, notes `data-note`, plus `data-notes` on everything a note is
//!   about. Inside text, library values carry their parts: clock entries
//!   `data-actor`, dots `data-dot`. Custom properties inherit, so setting one on
//!   a keyed group restyles just that lane, event or message.
//!
//! A space-time diagram is laid out sequentially: the `i`-th recorded event
//! sits in the `i`-th column, so every message points right and the figure
//! reads in the order its scenario was written. Text is then kept clear of the
//! arrows by an explicit collision pass. Each event's name and annotation
//! start on whichever side of the marker no arrow passes through; any text an
//! arrow still crosses is moved to the other side of its lane, and failing
//! that the events are pushed apart until the arrow clears. Message labels and
//! note badges are placed last, in the nearest free spot. The test
//! `committed_figures_have_no_collisions` checks the whole catalog.

use std::fmt::Write as _;

use super::Render;
use super::steps::{At, timed};
use crate::model::{Claim, DotLine, EventId, Message, Note, Relation, Rich, SpaceTime, Span, rich};

pub struct Svg;

const COL_W: f64 = 72.0; // minimum pitch between consecutive events
const R: f64 = 5.0; // event marker radius
const RING: f64 = 9.0; // note ring radius
const GAP: f64 = 12.0; // clear space between neighbouring text on one lane
const NEAR: f64 = 12.0; // an event's first text block starts this far from its lane
const STACK_GAP: f64 = 4.0; // between stacked text blocks
const CLEAR: f64 = 2.0; // an arrow must miss text by this much
const PUSH: f64 = 16.0; // how far events are pushed apart per collision
const MIN_W: f64 = 420.0; // captions wrap to the figure's width, but not below this
const BADGE_R: f64 = 7.5; // note badge radius
const BADGE_GAP: f64 = 3.0; // between badges on one event
const LANE_PX: f64 = 14.0;
const NAME_PX: f64 = 13.0;
const ANN_PX: f64 = 13.0;
const LABEL_PX: f64 = 12.0;
const BADGE_PX: f64 = 11.0;
const CAPTION_PX: f64 = 14.0;
const CAPTION_LH: f64 = 20.0;

/// Fallback families, used only when the stylesheet is missing.
const SANS_ATTR: &str = "system-ui, sans-serif";
const MONO_ATTR: &str = "ui-monospace, Menlo, Consolas, monospace";

const STYLE: &[&str] = &[
    r#".ds-figure { max-width: 100%; height: auto; font-family: var(--ds-font-sans, system-ui, -apple-system, "Segoe UI", sans-serif); }"#,
    // Opened on its own or through <img>, there is no page colour to inherit.
    ".ds-figure:root { color-scheme: light dark; color: CanvasText; }",
    ".ds-figure text { fill: var(--ds-ink, currentColor); }",
    r#".ds-figure :is(.ds-event-value, .ds-row-value, .ds-v) { font-family: var(--ds-font-mono, ui-monospace, "SF Mono", Menlo, Consolas, monospace); }"#,
    ".ds-figure .ds-sym { font-family: var(--ds-font-sym, system-ui, sans-serif); }",
    ".ds-figure .ds-lane-line { stroke: var(--ds-lane-line, var(--ds-rule, var(--ds-ink, currentColor))); stroke-opacity: var(--ds-lane-opacity, .5); stroke-width: var(--ds-lane-width, 1); }",
    ".ds-figure .ds-lane-head { fill: var(--ds-lane-line, var(--ds-rule, var(--ds-ink, currentColor))); fill-opacity: var(--ds-lane-opacity, .5); }",
    // An actor's palette slot. With no --ds-actor-<n> on the page, --ds-actor is
    // invalid and each rule using it falls back (or, for fill, inherits), so a
    // figure stays in ink until a page opts in to colour.
    ".ds-figure [data-actor-n=\"1\"] { --ds-actor: var(--ds-actor-1); }",
    ".ds-figure [data-actor-n=\"2\"] { --ds-actor: var(--ds-actor-2); }",
    ".ds-figure [data-actor-n=\"3\"] { --ds-actor: var(--ds-actor-3); }",
    ".ds-figure [data-actor-n=\"4\"] { --ds-actor: var(--ds-actor-4); }",
    ".ds-figure [data-actor-n=\"5\"] { --ds-actor: var(--ds-actor-5); }",
    ".ds-figure [data-actor-n=\"6\"] { --ds-actor: var(--ds-actor-6); }",
    ".ds-figure [data-actor-n=\"7\"] { --ds-actor: var(--ds-actor-7); }",
    ".ds-figure [data-actor-n=\"8\"] { --ds-actor: var(--ds-actor-8); }",
    ".ds-figure .ds-actor { fill: var(--ds-actor); }",
    ".ds-figure .ds-lane-name { fill: var(--ds-lane-name, var(--ds-actor, var(--ds-ink, currentColor))); font-weight: 600; }",
    ".ds-figure .ds-row-name { fill: var(--ds-lane-name, var(--ds-ink, currentColor)); font-weight: 600; }",
    ".ds-figure .ds-event-dot { fill: var(--ds-event-dot, var(--ds-ink, currentColor)); }",
    ".ds-figure .ds-event-name { fill: var(--ds-event-name, var(--ds-muted, var(--ds-ink, currentColor))); }",
    ".ds-figure .ds-field { fill: var(--ds-field, var(--ds-muted, var(--ds-ink, currentColor))); }",
    ".ds-figure :is(.ds-event-value, .ds-row-value) { fill: var(--ds-value, var(--ds-ink, currentColor)); }",
    ".ds-figure .ds-message-line { fill: none; stroke: var(--ds-message-line, var(--ds-ink, currentColor)); stroke-width: var(--ds-message-width, 1.5); }",
    ".ds-figure .ds-message-head { fill: var(--ds-message-line, var(--ds-ink, currentColor)); }",
    ".ds-figure .ds-message-label { fill: var(--ds-message-label, var(--ds-muted, var(--ds-ink, currentColor))); }",
    ".ds-figure .ds-ring { fill: none; stroke: var(--ds-accent, var(--ds-ink, currentColor)); stroke-width: var(--ds-ring-width, 1.5); stroke-dasharray: var(--ds-ring-dash, none); }",
    ".ds-figure .ds-badge-disc { fill: var(--ds-accent, var(--ds-ink, currentColor)); }",
    ".ds-figure .ds-badge-n { fill: var(--ds-on-accent, Canvas); font-weight: 700; }",
    ".ds-figure .ds-caption { fill: var(--ds-caption, var(--ds-ink, currentColor)); }",
    ".ds-figure :is(.ds-counter, .ds-legend) { fill: var(--ds-muted, var(--ds-ink, currentColor)); }",
    ".ds-figure :is(.ds-dot, .ds-claim) { fill: var(--ds-dot, var(--ds-ink, currentColor)); }",
    ".ds-figure .ds-dot.ds-gap { fill: none; stroke: var(--ds-gap, var(--ds-ink, currentColor)); stroke-width: 1.5; stroke-dasharray: 3 2; }",
    ".ds-figure .ds-wrong { fill: var(--ds-wrong, var(--ds-accent, var(--ds-ink, currentColor))); font-weight: 700; }",
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

/// For attribute values, which are always double-quoted here.
pub(crate) fn escape_attr(s: &str) -> String {
    escape(s).replace('"', "&quot;")
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

/// ` data-key="value"` for each pair.
fn data_attrs(data: &[(&str, String)]) -> String {
    data.iter()
        .map(|(k, v)| format!(r#" data-{k}="{}""#, escape_attr(v)))
        .collect()
}

/// A space-separated list of note numbers, for `data-notes`.
fn note_list(ns: impl IntoIterator<Item = usize>) -> String {
    ns.into_iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A block of one or more lines in one font. Spans that hold library values are
/// set in monospace whatever the block's font.
#[derive(Clone, Debug)]
struct Text {
    lines: Vec<Vec<Span>>,
    px: f64,
    font: Font,
    class: &'static str,
}

impl Text {
    fn plain(s: &str, px: f64, font: Font, class: &'static str) -> Text {
        Text {
            lines: s.split('\n').map(|l| vec![Span::plain(l)]).collect(),
            px,
            font,
            class,
        }
    }

    fn rich(r: &Rich, px: f64, font: Font, class: &'static str) -> Text {
        Text {
            lines: if r.lines.is_empty() {
                vec![Vec::new()]
            } else {
                r.lines.clone()
            },
            px,
            font,
            class,
        }
    }

    fn span_font(&self, s: &Span) -> Font {
        if s.value { Font::Mono } else { self.font }
    }

    fn width(&self) -> f64 {
        self.lines
            .iter()
            .map(|l| {
                l.iter()
                    .map(|s| text_width(&s.text, self.px, self.span_font(s)))
                    .sum::<f64>()
            })
            .fold(0.0, f64::max)
    }

    /// From the top of the first line's ascent to the bottom of the last line's descent.
    fn height(&self) -> f64 {
        (self.lines.len() as f64 - 1.0) * line_height(self.px) + self.px
    }

    #[cfg(test)]
    fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\\n")
    }
}

/// `ds-<role>` for each role, space-separated.
pub(crate) fn role_classes(roles: &[&str]) -> String {
    roles
        .iter()
        .map(|r| format!("ds-{r}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A line of spans as SVG. In sans text, each run of value spans is wrapped in
/// one `<tspan class="ds-v">` set in monospace.
fn emit_spans(out: &mut String, line: &[Span], container: Font) {
    let mut in_value = false;
    for s in line {
        let wrap = s.value && container == Font::Sans;
        if wrap != in_value {
            if wrap {
                write!(out, r#"<tspan class="ds-v" font-family="{MONO_ATTR}">"#)
                    .expect("writing to a String cannot fail");
            } else {
                out.push_str("</tspan>");
            }
            in_value = wrap;
        }
        emit_span(out, s);
    }
    if in_value {
        out.push_str("</tspan>");
    }
}

/// A span as SVG: bare text, or a `<tspan>` carrying its role and data.
fn emit_span(out: &mut String, s: &Span) {
    if s.roles.is_empty() && s.data.is_empty() {
        out.push_str(&escape(&s.text));
        return;
    }
    out.push_str("<tspan");
    if !s.roles.is_empty() {
        write!(out, r#" class="{}""#, role_classes(&s.roles))
            .expect("writing to a String cannot fail");
    }
    out.push_str(&data_attrs(&s.data));
    write!(out, ">{}</tspan>", escape(&s.text)).expect("writing to a String cannot fail");
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

    fn emit_line(&self, out: &mut String, k: usize) {
        let line = &self.text.lines[k];
        if line.iter().all(|s| s.text.is_empty()) {
            out.push_str("&#8203;");
            return;
        }
        emit_spans(out, line, self.text.font);
        out.push_str(&"\u{a0}".repeat(self.pad(k)));
    }

    /// Trailing no-break spaces that bring line `k` of a monospace block up to
    /// its longest line. Every line is then the same width, so however the
    /// block is anchored, its lines share a left edge (a column, for
    /// [`fields`](crate::label::fields)) and the block still centres on the
    /// font's real widths, not this module's estimates.
    fn pad(&self, k: usize) -> usize {
        if self.text.font != Font::Mono || self.text.lines.len() < 2 {
            return 0;
        }
        let chars = |l: &[Span]| l.iter().map(|s| s.text.chars().count()).sum::<usize>();
        let longest = self.text.lines.iter().map(|l| chars(l)).max().unwrap_or(0);
        longest - chars(&self.text.lines[k])
    }

    /// `attrs` is inserted verbatim after the class (e.g. ` data-note="1"`).
    fn emit(&self, out: &mut String, indent: &str, attrs: &str) {
        let anchor = match self.anchor {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        };
        let mut body = String::new();
        self.emit_line(&mut body, 0);
        for k in 1..self.text.lines.len() {
            write!(
                body,
                r#"<tspan x="{}" dy="{}">"#,
                num(self.x),
                num(line_height(self.text.px))
            )
            .expect("writing to a String cannot fail");
            self.emit_line(&mut body, k);
            body.push_str("</tspan>");
        }
        let family = match self.text.font {
            Font::Mono => format!(r#" font-family="{MONO_ATTR}""#),
            Font::Sans => String::new(),
        };
        emit!(
            out,
            r#"{indent}<text class="{}"{attrs} x="{}" y="{}" text-anchor="{anchor}" font-size="{}"{family}>{body}</text>"#,
            self.text.class,
            num(self.x),
            num(self.y),
            num(self.text.px)
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

    fn around(c: Pt, r: f64) -> Rect {
        Rect {
            x0: c.0 - r,
            y0: c.1 - r,
            x1: c.0 + r,
            y1: c.1 + r,
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
    /// The event's marker (or its ring), which an arrow must not pass through
    /// unless the arrow starts or ends there: otherwise a message that skips a
    /// lane reads as two messages via the event it crosses.
    Marker,
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

/// The events that end a note: they get a ring, and arrows stop short of it.
fn ringed(d: &SpaceTime) -> Vec<bool> {
    let mut r = vec![false; d.events().len()];
    for c in d.callouts() {
        r[c.a.0] = true;
        r[c.b.0] = true;
    }
    r
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
    ringed: Vec<bool>,
    name: Vec<Option<Placed>>,
    annotation: Vec<Option<Placed>>,
}

fn place(d: &SpaceTime, quad: &[[bool; 4]], ringed: &[bool], moves: &Moves) -> Geo {
    let events = d.events();
    let label_w = d
        .lanes()
        .iter()
        .map(|l| text_width(&l.name, LANE_PX, Font::Sans))
        .fold(0.0, f64::max)
        + 20.0;

    let (mut above, mut below) = (Vec::new(), Vec::new());
    for (i, e) in events.iter().enumerate() {
        let (mut up, mut down) = (Vec::new(), Vec::new());
        if !e.name.is_empty() {
            let t = (
                Kind::Name,
                Text::plain(&e.name, NAME_PX, Font::Sans, "ds-event-name"),
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
                Text::rich(&e.annotation, ANN_PX, Font::Mono, "ds-event-value"),
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
        // Every earlier event on the lane, not just the last: wide text two
        // events back can still reach past a narrow one in between.
        for p in (0..i).filter(|&p| events[p].lane == e.lane) {
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
                    Kind::Marker => unreachable!("stacks hold text only"),
                }
            }
        }
    }
    Geo {
        x,
        lane_y,
        label_w,
        below_h,
        ringed: ringed.to_vec(),
        name,
        annotation,
    }
}

fn marker(geo: &Geo, d: &SpaceTime, e: EventId) -> Pt {
    (geo.x[e.0], geo.lane_y[d.lane_of(e).0])
}

/// The drawn part of a message: from just outside the sender's marker (or its
/// ring) to the arrow tip just outside the receiver's.
fn segment(geo: &Geo, d: &SpaceTime, m: &Message) -> (Pt, Pt) {
    let (a, b) = (marker(geo, d, m.from), marker(geo, d, m.to));
    let (ux, uy) = unit(a, b);
    let out = |e: EventId| if geo.ringed[e.0] { RING } else { R };
    let (start, end) = (out(m.from) + 2.0, out(m.to) + 3.0);
    (
        (a.0 + ux * start, a.1 + uy * start),
        (b.0 - ux * end, b.1 - uy * end),
    )
}

fn text_of(geo: &Geo, i: usize, kind: Kind) -> Option<&Placed> {
    match kind {
        Kind::Name => geo.name[i].as_ref(),
        Kind::Annotation => geo.annotation[i].as_ref(),
        Kind::Marker => None,
    }
}

/// The marker's box, ring included.
fn marker_rect(geo: &Geo, d: &SpaceTime, e: EventId) -> Rect {
    let r = if geo.ringed[e.0] { RING } else { R };
    Rect::around(marker(geo, d, e), r)
}

/// The first (message, event, part) where the arrow crosses the event's text,
/// or its marker when the event is not one of the arrow's own ends.
fn first_collision(d: &SpaceTime, geo: &Geo) -> Option<(usize, usize, Kind)> {
    for (mi, m) in d.messages().iter().enumerate() {
        let (a, b) = segment(geo, d, m);
        for i in 0..d.events().len() {
            for kind in [Kind::Name, Kind::Annotation] {
                if text_of(geo, i, kind).is_some_and(|p| p.rect().grow(CLEAR).crossed_by(a, b)) {
                    return Some((mi, i, kind));
                }
            }
            let own = i == m.from.0 || i == m.to.0;
            if !own && marker_rect(geo, d, EventId(i)).grow(CLEAR).crossed_by(a, b) {
                return Some((mi, i, Kind::Marker));
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

/// The numbered badges next to one event, one per note it ends.
#[derive(Clone, Debug)]
struct Badge {
    event: usize,
    notes: Vec<usize>,
    x0: f64,
    y0: f64,
}

impl Badge {
    fn width(notes: usize) -> f64 {
        let k = notes as f64;
        k * 2.0 * BADGE_R + (k - 1.0) * BADGE_GAP
    }

    fn rect(&self) -> Rect {
        Rect {
            x0: self.x0,
            y0: self.y0,
            x1: self.x0 + Badge::width(self.notes.len()),
            y1: self.y0 + 2.0 * BADGE_R,
        }
    }

    /// Centre of the `k`-th badge.
    fn centre(&self, k: usize) -> Pt {
        (
            self.x0 + BADGE_R + k as f64 * (2.0 * BADGE_R + BADGE_GAP),
            self.y0 + BADGE_R,
        )
    }
}

struct Layout {
    x: Vec<f64>,
    lane_y: Vec<f64>,
    label_w: f64,
    width: f64,
    height: f64,
    /// Each message's drawn segment, start to arrow tip.
    segments: Vec<(Pt, Pt)>,
    names: Vec<Option<Placed>>,
    annotations: Vec<Option<Placed>>,
    /// Per message, its label if it has one.
    labels: Vec<Option<Placed>>,
    ringed: Vec<bool>,
    badges: Vec<Badge>,
    /// Caption lines, each with its note's number.
    captions: Vec<(usize, Placed)>,
}

impl Layout {
    /// Every box that must stay clear of arrows, other boxes and the edges.
    #[cfg(test)]
    fn boxes(&self) -> Vec<(String, Rect)> {
        let texts = self
            .names
            .iter()
            .chain(&self.annotations)
            .chain(&self.labels)
            .flatten()
            .chain(self.captions.iter().map(|(_, p)| p))
            .map(|p| (p.text.plain_text(), p.rect()));
        let badges = self
            .badges
            .iter()
            .map(|b| (format!("badge {:?}", b.notes), b.rect()));
        texts.chain(badges).collect()
    }
}

fn layout(d: &SpaceTime, captions: bool) -> Layout {
    let n = d.events().len();
    let quad = quadrants(d);
    let ringed = ringed(d);
    let mut moves = Moves {
        name_below: vec![false; n],
        annotation_above: vec![false; n],
        extra: vec![0.0; n],
    };
    let mut geo = place(d, &quad, &ringed, &moves);
    for _ in 0..400 {
        let Some((mi, ei, kind)) = first_collision(d, &geo) else {
            break;
        };
        // First choice for text: the other side of the lane, if no arrow is
        // there either. A marker can't move sides, so it is always pushed.
        let flip = match kind {
            Kind::Name => Some(&mut moves.name_below[ei]),
            Kind::Annotation => Some(&mut moves.annotation_above[ei]),
            Kind::Marker => None,
        };
        if let Some(moved) = flip.filter(|m| !**m) {
            *moved = true;
            let trial = place(d, &quad, &ringed, &moves);
            if !is_hit(d, &trial, ei, kind) {
                geo = trial;
                continue;
            }
            match kind {
                Kind::Name => moves.name_below[ei] = false,
                Kind::Annotation => moves.annotation_above[ei] = false,
                Kind::Marker => {}
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
        geo = place(d, &quad, &ringed, &moves);
    }
    finish(d, geo, captions)
}

fn finish(d: &SpaceTime, mut geo: Geo, captions: bool) -> Layout {
    let notes = d.notes();
    let mut segments: Vec<(Pt, Pt)> = d.messages().iter().map(|m| segment(&geo, d, m)).collect();
    let mut taken: Vec<Rect> = geo
        .name
        .iter()
        .chain(&geo.annotation)
        .flatten()
        .map(Placed::rect)
        .collect();
    // Markers and rings are taken too, so labels and badges stay off them.
    for i in 0..d.events().len() {
        let r = if geo.ringed[i] { RING } else { R };
        taken.push(Rect::around(marker(&geo, d, EventId(i)), r));
    }

    // Message labels: beside the line, pushed out along its normal, at the
    // first of several spots along it that nothing else occupies, and off the
    // timelines, where a label would read as part of the lane it sits on.
    let lane_lines: Vec<(Pt, Pt)> = geo.lane_y.iter().map(|&y| ((0.0, y), (1e9, y))).collect();
    let mut labels = vec![None; d.messages().len()];
    for (mi, m) in d.messages().iter().enumerate() {
        if m.label.is_empty() {
            continue;
        }
        let (a, b) = (marker(&geo, d, m.from), marker(&geo, d, m.to));
        let (ux, uy) = unit(a, b);
        let up = (uy, -ux); // the normal that points up the page
        let text = Text::rich(&m.label, LABEL_PX, Font::Sans, "ds-message-label");
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
            .find(|p| {
                let r = p.rect().grow(CLEAR);
                is_free(&r, &taken, &segments, Some(mi)) && is_free(&r, &[], &lane_lines, None)
            })
            .unwrap_or_else(|| candidate(0.5, 1.0));
        taken.push(placed.rect());
        labels[mi] = Some(placed);
    }

    // Badges: one group per event that ends a note, in the first free spot
    // off its ring, preferring the diagonals so the badge stays off the lane.
    let mut badges = Vec::new();
    for i in 0..d.events().len() {
        let numbers: Vec<usize> = notes
            .iter()
            .filter(|n| n.callout.a.0 == i || n.callout.b.0 == i)
            .map(|n| n.n)
            .collect();
        if numbers.is_empty() {
            continue;
        }
        let (x, y) = marker(&geo, d, EventId(i));
        let (w, h) = (Badge::width(numbers.len()), 2.0 * BADGE_R);
        let off = 0.75 * RING + 1.0;
        // Beside the event's name first, where it reads as a footnote mark;
        // on the lane only as a last resort, where it could pass for an event.
        let mut spots = Vec::new();
        if let Some(name) = &geo.name[i] {
            let r = name.rect();
            let mid = (r.y0 + r.y1) / 2.0 - h / 2.0;
            spots.extend([(r.x1 + 4.0, mid), (r.x0 - 4.0 - w, mid)]);
        }
        spots.extend([
            (x + off, y - off - h),
            (x - off - w, y - off - h),
            (x + off, y + off),
            (x - off - w, y + off),
            (x + RING + 3.0, y - h / 2.0),
            (x - RING - 3.0 - w, y - h / 2.0),
        ]);
        let last_resort = spots[spots.len() - 2];
        let badge = |(x0, y0): (f64, f64)| Badge {
            event: i,
            notes: numbers.clone(),
            x0,
            y0,
        };
        let placed = spots
            .into_iter()
            .map(badge)
            .find(|b| {
                let r = b.rect();
                r.y0 >= 2.0 && is_free(&r.grow(CLEAR), &taken, &segments, None)
            })
            .unwrap_or_else(|| badge(last_resort));
        taken.push(placed.rect());
        badges.push(placed);
    }

    let mut names = geo.name.clone();
    let mut annotations = geo.annotation.clone();

    // The figure is as wide as its rightmost mark; captions wrap to that. If
    // anything reaches left into the lane names, slide the whole picture right.
    let min_left = taken.iter().map(|r| r.x0).fold(f64::INFINITY, f64::min);
    let shift = (geo.label_w + GAP / 2.0 - min_left).max(0.0);
    if shift > 0.0 {
        for x in &mut geo.x {
            *x += shift;
        }
        for p in names
            .iter_mut()
            .chain(&mut annotations)
            .chain(&mut labels)
            .flatten()
        {
            p.x += shift;
        }
        for b in &mut badges {
            b.x0 += shift;
        }
        segments = d.messages().iter().map(|m| segment(&geo, d, m)).collect();
    }
    let min_w = if captions { MIN_W } else { 0.0 };
    let width = (taken.iter().map(|r| r.x1).fold(0.0, f64::max) + shift + GAP)
        .max(min_w)
        .max(geo.label_w + COL_W)
        .ceil();

    let marks_bottom = taken.iter().map(|r| r.y1).fold(0.0, f64::max) + 4.0;
    let lanes_bottom =
        (geo.lane_y.last().copied().unwrap_or(0.0) + geo.below_h + 8.0).max(marks_bottom);
    let mut caption_lines = Vec::new();
    if captions {
        let mut line_no = 0;
        for note in &notes {
            let text = format!("{}. {}", note.n, note.callout.text.plain());
            for (k, line) in wrap(&text, width - geo.label_w - GAP, CAPTION_PX, Font::Sans)
                .iter()
                .enumerate()
            {
                caption_lines.push((
                    note.n,
                    Placed {
                        text: Text::plain(line, CAPTION_PX, Font::Sans, "ds-caption"),
                        x: if k == 0 {
                            geo.label_w
                        } else {
                            geo.label_w + 16.0
                        }, // continuation lines hang under the text
                        y: lanes_bottom + 10.0 + CAPTION_LH * line_no as f64 + CAPTION_PX,
                        anchor: Anchor::Start,
                    },
                ));
                line_no += 1;
            }
        }
    }
    let captions_h = if caption_lines.is_empty() {
        0.0
    } else {
        10.0 + CAPTION_LH * caption_lines.len() as f64
    };
    let height = (lanes_bottom + captions_h + 12.0).ceil();

    Layout {
        x: geo.x,
        lane_y: geo.lane_y,
        label_w: geo.label_w,
        width,
        height,
        segments,
        names,
        annotations,
        labels,
        ringed: geo.ringed,
        badges,
        captions: caption_lines,
    }
}

// ---------------------------------------------------------------------------
// Emission.

/// A plain-language account of a space-time figure, for `<desc>`: what screen
/// readers get in place of the picture.
fn describe(d: &SpaceTime, with_notes: bool) -> String {
    let mut parts = vec![format!("{}.", lanes_sentence(d))];
    for i in 0..d.events().len() {
        parts.push(format!("{}.", event_sentence(d, EventId(i), false).plain()));
    }
    if with_notes {
        for note in d.notes() {
            parts.push(format!(
                "Note {}: {}.",
                note.n,
                one_line(&note.callout.text.plain())
            ));
        }
    }
    parts.join(" ")
}

/// "Timelines for A, B, C": what there is before anything happens.
pub(crate) fn lanes_sentence(d: &SpaceTime) -> String {
    let names: Vec<&str> = d.lanes().iter().map(|l| l.name.as_str()).collect();
    format!("Timelines for {}", names.join(", "))
}

/// An event's name (role `lead`; its key if it has no name) and lane: "B2 on B".
pub(crate) fn subject(d: &SpaceTime, id: EventId) -> Rich {
    let e = &d.events()[id.0];
    let name = if e.name.is_empty() {
        d.event_key(id)
    } else {
        e.name.clone()
    };
    rich![
        Span::plain(name).also("lead"),
        format!(" on {}", lane_name(d, id))
    ]
}

/// What happened at an event, as one line: "B2 on B, receiving m1 {A:2} from
/// A2 on A: {A:2, B:2}", a value's lines joined by "; ". With `sends`, also
/// what it sent, which a step-through tells as it is sent and a description
/// of the finished figure tells where it arrives.
pub(crate) fn event_sentence(d: &SpaceTime, id: EventId, sends: bool) -> Rich {
    let what = |m: &Message| {
        if m.label.is_empty() {
            Rich::from("a message")
        } else {
            joined(&m.label, " ")
        }
    };
    let mut s = subject(d, id);
    for m in d.messages().iter().filter(|m| m.to == id) {
        s = s.then(", receiving ").then(what(m)).then(format!(
            " from {} on {}",
            d.events()[m.from.0].name,
            lane_name(d, m.from)
        ));
    }
    if sends {
        for m in d.messages().iter().filter(|m| m.from == id) {
            s = s
                .then(", sending ")
                .then(what(m))
                .then(format!(" to {}", lane_name(d, m.to)));
        }
    }
    let e = &d.events()[id.0];
    if !e.annotation.is_empty() {
        s = s.then(": ").then(joined(&e.annotation, "; "));
    }
    s
}

fn lane_name(d: &SpaceTime, e: EventId) -> &str {
    &d.lanes()[d.lane_of(e).0].name
}

/// `r` on one line, its lines separated by `sep`.
fn joined(r: &Rich, sep: &str) -> Rich {
    let mut line = Vec::new();
    for (k, l) in r.lines.iter().enumerate() {
        if k > 0 {
            line.push(Span::plain(sep));
        }
        line.extend(l.iter().cloned());
    }
    Rich { lines: vec![line] }
}

/// `<title>` and `<desc>` are read as one run of text, and a page may paste
/// them into Markdown, where a blank line would end an HTML block.
pub(crate) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn open(
    out: &mut String,
    stem: &str,
    kind: &str,
    width: f64,
    height: f64,
    title: &str,
    desc: &str,
) {
    let (w, h) = (num(width), num(height));
    emit!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="ds-figure" data-figure="{}" data-kind="{kind}" role="img" viewBox="0 0 {w} {h}" width="{w}" height="{h}" style="--ds-w: {w}" fill="currentColor" font-family="{SANS_ATTR}">"#,
        escape_attr(stem)
    );
    emit!(out, "  <title>{}</title>", escape(&one_line(title)));
    emit!(out, "  <desc>{}</desc>", escape(&one_line(desc)));
    emit!(out, "  <style>");
    emit!(out, "    @layer ds-figure {{");
    for rule in STYLE {
        emit!(out, "      {rule}");
    }
    emit!(out, "    }}");
    emit!(out, "  </style>");
}

/// A horizontal timeline from `x0` to `x1` with an arrowhead at the right,
/// named at the left, grouped under its key.
fn timeline(out: &mut String, x0: f64, x1: f64, y: f64, name: &str, key: &str, attrs: &str) {
    emit!(
        out,
        r#"    <g class="ds-lane" data-lane="{}"{attrs}>"#,
        escape_attr(key)
    );
    emit!(
        out,
        r#"      <line class="ds-lane-line" x1="{}" y1="{}" x2="{}" y2="{}" stroke="currentColor" stroke-opacity=".5"/>"#,
        num(x0),
        num(y),
        num(x1 - 8.0),
        num(y)
    );
    emit!(
        out,
        r#"      <polygon class="ds-lane-head" points="{},{} {},{} {},{}" fill-opacity=".5"/>"#,
        num(x1 - 8.0),
        num(y - 4.0),
        num(x1),
        num(y),
        num(x1 - 8.0),
        num(y + 4.0)
    );
    Placed {
        text: Text::plain(name, LANE_PX, Font::Sans, "ds-lane-name"),
        x: x0 - 10.0,
        y: y + 5.0,
        anchor: Anchor::End,
    }
    .emit(out, "      ", "");
    emit!(out, "    </g>");
}

/// ` data-relation="before"` or `"concurrent"` when note `n` claims one.
pub(crate) fn relation_attr(notes: &[Note], n: usize) -> &'static str {
    match notes
        .iter()
        .find(|x| x.n == n)
        .and_then(|x| x.callout.relation)
    {
        Some(Relation::Before) => r#" data-relation="before""#,
        Some(Relation::Concurrent) => r#" data-relation="concurrent""#,
        None => "",
    }
}

/// ` data-notes="1 2"` listing the notes that `has` says involve something, or nothing.
fn notes_attr(notes: &[Note], has: impl Fn(&Note) -> bool) -> String {
    let ns: Vec<usize> = notes.iter().filter(|n| has(n)).map(|n| n.n).collect();
    if ns.is_empty() {
        String::new()
    } else {
        format!(r#" data-notes="{}""#, note_list(ns))
    }
}

/// The figure with every span that names an actor also tagged `data-actor-n`,
/// that actor's palette slot (see [`SpaceTime::actors`]). Tagging twice is
/// harmless.
pub(crate) fn with_actor_slots(d: &SpaceTime) -> SpaceTime {
    let actors = d.actors();
    let mut d = d.clone();
    d.for_each_span(|s| tag_slot(s, &actors));
    d
}

/// A number line with its actor's spans tagged, as [`with_actor_slots`] does.
pub(crate) fn dot_line_with_slots(d: &DotLine) -> DotLine {
    let actors = vec![d.actor().to_string()];
    let mut d = d.clone();
    d.for_each_span(|s| tag_slot(s, &actors));
    d
}

fn tag_slot(s: &mut Span, actors: &[String]) {
    let slot = s
        .data
        .iter()
        .find(|(k, _)| *k == "actor")
        .and_then(|(_, a)| actors.iter().position(|x| x == a));
    if let Some(i) = slot {
        s.data.retain(|(k, _)| *k != "actor-n");
        s.data.push(("actor-n", (i + 1).to_string()));
    }
}

/// ` data-actor="A" data-actor-n="1"` for a lane (or one of its events) that
/// shows an actor, or nothing.
pub(crate) fn actor_attrs(actor: Option<&str>, actors: &[String]) -> String {
    match actor.and_then(|a| actors.iter().position(|x| x == a).map(|i| (a, i))) {
        Some((a, i)) => format!(
            r#" data-actor="{}" data-actor-n="{}""#,
            escape_attr(a),
            i + 1
        ),
        None => String::new(),
    }
}

/// The SVG for a space-time figure. With `captions`, the notes are set as text
/// under the drawing, for a figure used on its own; without, a page sets them
/// itself (see the HTML backend). `at` picks the step to draw, or every step
/// (see [`steps`](super::steps)), always on the finished figure's layout.
pub(crate) fn space_time_svg(stem: &str, d: &SpaceTime, captions: bool, at: At) -> String {
    let d = &with_actor_slots(d);
    let actors = d.actors();
    let l = layout(d, captions);
    let notes = d.notes();
    let t = d.timeline();
    let timed = |mark: &dyn Fn(usize) -> Option<String>| timed(at, t.last(), OFF, mark);
    let mut out = String::new();
    open(
        &mut out,
        stem,
        "space-time",
        l.width,
        l.height,
        d.title(),
        &describe(d, captions),
    );

    emit!(out, r#"  <g class="ds-lanes">"#);
    for (k, lane) in d.lanes().iter().enumerate() {
        let key = d.lane_key(crate::model::LaneId(k));
        let attrs = actor_attrs(lane.actor.as_deref(), &actors);
        timeline(
            &mut out,
            l.label_w,
            l.width,
            l.lane_y[k],
            &lane.name,
            &key,
            &attrs,
        );
    }
    emit!(out, "  </g>");

    emit!(out, r#"  <g class="ds-messages">"#);
    for (mi, m) in d.messages().iter().enumerate() {
        let seg = l.segments[mi];
        let (sent, arrives) = (t.event(m.from), t.event(m.to));
        // Between its send and its receipt, a message reaches as far as the
        // column of the latest event: the playhead.
        let flight = |s: usize| {
            (sent..arrives).contains(&s).then(|| {
                let now = t.now(s).expect("the sender has happened");
                front(seg, l.x[now.0])
            })
        };
        emit!(
            out,
            r#"    <g class="ds-message" data-message="{}" data-from="{}" data-to="{}"{}>"#,
            d.message_key(mi),
            d.event_key(m.from),
            d.event_key(m.to),
            notes_attr(&notes, |n| n.messages.contains(&mi))
        );
        // Where the arrow's head is at each step, and whether its line shows.
        let head = |s: usize| if s >= arrives { Some(seg.1) } else { flight(s) };
        let line = |s: usize| head(s).filter(|&tip| dist(seg.0, tip) - HEAD >= 0.5);
        let class = |s: usize, part: &str| {
            if s >= arrives {
                format!("ds-message-{part}")
            } else {
                format!("ds-message-{part} ds-flight")
            }
        };
        out.push_str(&timed(&|s| {
            let tip = line(s)?;
            let len = dist(seg.0, tip) - HEAD;
            let was = before(s, line).map_or(0.0, |p| dist(seg.0, p) - HEAD);
            let vars = motion(
                at,
                &format!("--ds-len: {}; --ds-grow: {}", num(len), num(len - was)),
            );
            Some(message_line(&class(s, "line"), &vars, (seg.0, tip)))
        }));
        out.push_str(&timed(&|s| {
            let tip = head(s)?;
            let was = before(s, head).unwrap_or(tip);
            let vars = motion(
                at,
                &format!(
                    "--ds-dx: {}; --ds-dy: {}",
                    num(was.0 - tip.0),
                    num(was.1 - tip.1)
                ),
            );
            Some(message_head(&class(s, "head"), &vars, (seg.0, tip)))
        }));
        if let Some(label) = &l.labels[mi] {
            let mut text = String::new();
            label.emit(&mut text, "      ", "");
            out.push_str(&timed(&|s| (s >= arrives).then(|| text.clone())));
        }
        emit!(out, "    </g>");
    }
    emit!(out, "  </g>");

    emit!(out, r#"  <g class="ds-events">"#);
    for (i, e) in d.events().iter().enumerate() {
        let id = EventId(i);
        let mut g = String::new();
        emit!(
            g,
            r#"    <g class="ds-event" data-event="{}" data-lane="{}"{}{}>"#,
            d.event_key(id),
            d.lane_key(e.lane),
            actor_attrs(d.lanes()[e.lane.0].actor.as_deref(), &actors),
            notes_attr(&notes, |n| n.events.contains(&id))
        );
        emit!(
            g,
            r#"      <circle class="ds-event-dot" cx="{}" cy="{}" r="{}"/>"#,
            num(l.x[i]),
            num(l.lane_y[e.lane.0]),
            num(R)
        );
        for p in [&l.names[i], &l.annotations[i]].into_iter().flatten() {
            p.emit(&mut g, "      ", "");
        }
        emit!(g, "    </g>");
        out.push_str(&timed(&|s| (s >= t.event(id)).then(|| g.clone())));
    }
    emit!(out, "  </g>");

    if !notes.is_empty() {
        emit!(out, r#"  <g class="ds-marks">"#);
        for (i, e) in d.events().iter().enumerate() {
            if !l.ringed[i] {
                continue;
            }
            // A ring marks the event from the first note it ends.
            let from = notes
                .iter()
                .filter(|n| n.callout.a.0 == i || n.callout.b.0 == i)
                .map(|n| t.note(n.n))
                .min()
                .expect("a ringed event ends a note");
            let mut ring = String::new();
            emit!(
                ring,
                r#"    <circle class="ds-ring" data-event="{}"{} cx="{}" cy="{}" r="{}" fill="none" stroke="currentColor" stroke-width="1.5"/>"#,
                d.event_key(EventId(i)),
                notes_attr(&notes, |n| n.callout.a.0 == i || n.callout.b.0 == i),
                num(l.x[i]),
                num(l.lane_y[e.lane.0]),
                num(RING)
            );
            out.push_str(&timed(&|s| (s >= from).then(|| ring.clone())));
        }
        for b in &l.badges {
            for (k, &n) in b.notes.iter().enumerate() {
                let (cx, cy) = b.centre(k);
                let mut g = String::new();
                emit!(
                    g,
                    r#"    <g class="ds-badge" data-note="{n}" data-event="{}"{}>"#,
                    d.event_key(EventId(b.event)),
                    relation_attr(&notes, n)
                );
                emit!(
                    g,
                    r#"      <circle class="ds-badge-disc" cx="{}" cy="{}" r="{}"/>"#,
                    num(cx),
                    num(cy),
                    num(BADGE_R)
                );
                emit!(
                    g,
                    r#"      <text class="ds-badge-n" x="{}" y="{}" text-anchor="middle" font-size="{}" fill="Canvas">{n}</text>"#,
                    num(cx),
                    num(cy + 0.36 * BADGE_PX),
                    num(BADGE_PX)
                );
                emit!(g, "    </g>");
                out.push_str(&timed(&|s| (s >= t.note(n)).then(|| g.clone())));
            }
        }
        emit!(out, "  </g>");
    }

    if !l.captions.is_empty() {
        emit!(out, r#"  <g class="ds-captions">"#);
        for (n, p) in &l.captions {
            let mut line = String::new();
            p.emit(&mut line, "    ", &format!(r#" data-note="{n}""#));
            out.push_str(&timed(&|s| (s >= t.note(*n)).then(|| line.clone())));
        }
        emit!(out, "  </g>");
    }
    emit!(out, "</svg>");
    out
}

/// What a version hidden until a page's player runs carries.
const OFF: &str = r#" display="none""#;

/// A message's arrowhead, tip to base.
const HEAD: f64 = 9.0;

/// Where a moving mark was before the version shown at step `s`: its
/// position in the last step that shows it differently, if that step shows it.
fn before(s: usize, at: impl Fn(usize) -> Option<Pt>) -> Option<Pt> {
    let now = at(s);
    (0..s).rev().map(&at).find(|p| *p != now).flatten()
}

/// ` style="…"` with what a page needs to move a mark from its last version
/// (see [`steps`](super::steps)), on a step-through; nothing on a finished
/// figure, whose marks never move.
fn motion(at: At, vars: &str) -> String {
    if at == At::End {
        String::new()
    } else {
        format!(r#" style="{vars}""#)
    }
}

/// A message's line, from its start to the base of its head at `tip`.
fn message_line(class: &str, vars: &str, (start, tip): (Pt, Pt)) -> String {
    let (ux, uy) = unit(start, tip);
    let base = (tip.0 - ux * HEAD, tip.1 - uy * HEAD);
    let mut out = String::new();
    emit!(
        out,
        r#"      <line class="{class}"{vars} x1="{}" y1="{}" x2="{}" y2="{}" stroke="currentColor" stroke-width="1.5"/>"#,
        num(start.0),
        num(start.1),
        num(base.0),
        num(base.1)
    );
    out
}

/// A message's arrowhead at `tip`, pointing away from `start`.
fn message_head(class: &str, vars: &str, (start, tip): (Pt, Pt)) -> String {
    let (ux, uy) = unit(start, tip);
    let (nx, ny) = (-uy, ux);
    let base = (tip.0 - ux * HEAD, tip.1 - uy * HEAD);
    let mut out = String::new();
    emit!(
        out,
        r#"      <polygon class="{class}"{vars} points="{},{} {},{} {},{}"/>"#,
        num(tip.0),
        num(tip.1),
        num(base.0 + nx * 3.5),
        num(base.1 + ny * 3.5),
        num(base.0 - nx * 3.5),
        num(base.1 - ny * 3.5)
    );
    out
}

/// How far a message in flight has got by the time of the column at `x`:
/// where its segment crosses that column, but at least a head's length out of
/// the sender, so that a message just sent already shows which way it goes.
/// Messages always point right, so the segment does cross each column once.
fn front((start, tip): (Pt, Pt), x: f64) -> Pt {
    let (ux, uy) = unit(start, tip);
    let along = ((x - start.0) / ux).max(HEAD).min(dist(start, tip));
    (start.0 + ux * along, start.1 + uy * along)
}

fn dist(a: Pt, b: Pt) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

/// The SVG for a dot number line at `at` (see [`space_time_svg`]). The rows
/// and counters stay put from frame to frame; each dot, claim and value is
/// drawn as the frame at that step has it, all laid out to fit every frame.
pub(crate) fn dot_line_svg(stem: &str, d: &DotLine, at: At) -> String {
    let actors = vec![d.actor().to_string()];
    let d = &dot_line_with_slots(d);
    let timed = |mark: &dyn Fn(usize) -> Option<String>| timed(at, d.last_step(), OFF, mark);
    const STEP: f64 = 56.0;
    const R_DOT: f64 = 7.0;
    let n = d.counters();
    let titles: Vec<Text> = d
        .claims()
        .iter()
        .map(|c| Text::plain(&c.title, LANE_PX, Font::Sans, "ds-row-name"))
        .collect();
    let actor = Text::plain(d.actor(), LANE_PX, Font::Sans, "ds-lane-name");
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
    let claim_text = |c: &Claim| Text::rich(&c.text, ANN_PX, Font::Mono, "ds-row-value");
    let text_w = d
        .all_frames()
        .iter()
        .flat_map(|f| &f.claims)
        .map(|c| claim_text(c).width())
        .fold(0.0, f64::max);
    let legend = Text::plain(
        "filled: dot seen · dashed: never seen · ×: claimed but never seen",
        LABEL_PX,
        Font::Sans,
        "ds-legend",
    );
    let row_h = 36.0;
    let legend_y = y_line + 46.0 + row_h * d.claims().len() as f64;
    let width = (text_x + text_w + GAP)
        .max(left_w + legend.width() + GAP)
        .ceil();
    let height = legend_y + 12.0;
    let desc = format!(
        "{}'s counters 1 to {n}; seen: {}. {}",
        d.actor(),
        d.present()
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        d.claims()
            .iter()
            .map(|c| format!("{} {} claims {}.", c.title, c.text.plain(), {
                c.counters
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            }))
            .collect::<Vec<_>>()
            .join(" ")
    );

    let mut out = String::new();
    open(&mut out, stem, "dot-line", width, height, d.title(), &desc);
    emit!(out, r#"  <g class="ds-row" data-row="seen">"#);
    let key = crate::model::slug(d.actor());
    // The number line reuses the lane mark, indented one level deeper.
    let mut lane = String::new();
    let attrs = actor_attrs(Some(d.actor()), &actors);
    timeline(&mut lane, left_w, line_end, y_line, d.actor(), &key, &attrs);
    for l in lane.lines() {
        emit!(out, "  {l}");
    }
    let dot_attrs = |c: u64| {
        let dot = format!("{}:{c}", d.actor());
        format!(
            r#" data-counter="{c}" data-dot="{}"{}"#,
            escape_attr(&dot),
            attrs
        )
    };
    for c in 1..=n {
        let x = x_of(c);
        Placed {
            text: Text::plain(&c.to_string(), NAME_PX, Font::Sans, "ds-counter"),
            x,
            y: y_line - 16.0,
            anchor: Anchor::Middle,
        }
        .emit(&mut out, "    ", &format!(r#" data-counter="{c}""#));
        let mut seen = String::new();
        emit!(
            seen,
            r#"    <circle class="ds-dot"{} cx="{}" cy="{}" r="{}"/>"#,
            dot_attrs(c),
            num(x),
            num(y_line),
            num(R_DOT)
        );
        let mut gap = String::new();
        emit!(
            gap,
            r#"    <circle class="ds-dot ds-gap"{} cx="{}" cy="{}" r="{}" fill="none" stroke="currentColor" stroke-dasharray="3 2"/>"#,
            dot_attrs(c),
            num(x),
            num(y_line),
            num(R_DOT)
        );
        out.push_str(&timed(&|s| {
            Some(if d.frame_at(s).present.contains(&c) {
                seen.clone()
            } else {
                gap.clone()
            })
        }));
    }
    emit!(out, "  </g>");
    for (k, claim) in d.claims().iter().enumerate() {
        let y = y_line + 46.0 + row_h * k as f64;
        emit!(
            out,
            r#"  <g class="ds-row" data-row="{}">"#,
            crate::model::slug(&claim.title)
        );
        Placed {
            text: titles[k].clone(),
            x: left_w - 10.0,
            y: y + 5.0,
            anchor: Anchor::End,
        }
        .emit(&mut out, "    ", "");
        for c in 1..=n {
            let x = x_of(c);
            let mut held = String::new();
            emit!(
                held,
                r#"    <circle class="ds-claim"{} cx="{}" cy="{}" r="4"/>"#,
                dot_attrs(c),
                num(x),
                num(y)
            );
            let mut wrong = String::new();
            Placed {
                text: Text::plain("×", LANE_PX, Font::Sans, "ds-wrong"),
                x,
                y: y + 5.0,
                anchor: Anchor::Middle,
            }
            .emit(&mut wrong, "    ", &dot_attrs(c));
            out.push_str(&timed(&|s| {
                let f = d.frame_at(s);
                f.claims[k].counters.contains(&c).then(|| {
                    if f.present.contains(&c) {
                        held.clone()
                    } else {
                        wrong.clone()
                    }
                })
            }));
        }
        out.push_str(&timed(&|s| {
            let mut value = String::new();
            Placed {
                text: claim_text(&d.frame_at(s).claims[k]),
                x: text_x,
                y: y + 4.0,
                anchor: Anchor::Start,
            }
            .emit(&mut value, "    ", "");
            Some(value)
        }));
        emit!(out, "  </g>");
    }
    Placed {
        text: legend,
        x: left_w,
        y: legend_y,
        anchor: Anchor::Start,
    }
    .emit(&mut out, "  ", "");
    emit!(out, "</svg>");
    out
}

impl Render for Svg {
    fn extension(&self) -> &'static str {
        "svg"
    }

    fn space_time(&self, stem: &str, d: &SpaceTime) -> Option<String> {
        Some(space_time_svg(stem, d, true, At::End))
    }

    fn dot_line(&self, stem: &str, d: &DotLine) -> Option<String> {
        Some(dot_line_svg(stem, d, At::End))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Diagram;

    /// No arrow through any text or badge, no two of them overlapping,
    /// everything inside the figure.
    fn assert_clear(stem: &str, d: &SpaceTime) {
        for captions in [true, false] {
            let l = layout(d, captions);
            let boxes = l.boxes();
            for (mi, m) in d.messages().iter().enumerate() {
                let (a, b) = l.segments[mi];
                for (i, e) in d.events().iter().enumerate() {
                    let own = i == m.from.0 || i == m.to.0;
                    let r = if l.ringed[i] { RING } else { R };
                    let rect = Rect::around((l.x[i], l.lane_y[e.lane.0]), r);
                    assert!(
                        own || !rect.crossed_by(a, b),
                        "{stem}: an arrow crosses the marker of event {i}"
                    );
                }
            }
            for (what, r) in &boxes {
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
            for (i, (a, ra)) in boxes.iter().enumerate() {
                for (b, rb) in &boxes[i + 1..] {
                    assert!(!ra.overlaps(rb), "{stem}: {a:?} overlaps {b:?}");
                }
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
    fn value_spans_are_measured_as_monospace() {
        let r = crate::model::rich!["m ", Span::value("{A:1}")];
        let t = Text::rich(&r, 10.0, Font::Sans, "ds-message-label");
        assert_eq!(
            t.width(),
            text_width("m ", 10.0, Font::Sans) + text_width("{A:1}", 10.0, Font::Mono)
        );
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
        assert_eq!(escape_attr(r#"say "hi""#), "say &quot;hi&quot;");
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
        let l = layout(&d, true);
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

    /// A sends to B and to C. The arrow to C must not run through B's marker,
    /// or the broadcast reads as a relay through B.
    #[test]
    fn an_arrow_skipping_a_lane_clears_the_event_it_passes() {
        let mut d = SpaceTime::new("t");
        let (a, b, c) = (d.lane("A"), d.lane("B"), d.lane("C"));
        let a1 = d.event(a, "A1", "{A:1}");
        let b1 = d.event(b, "B1", "{A:1, B:1}");
        let c1 = d.event(c, "C1", "{A:1, C:1}");
        d.message(a1, b1, "");
        d.message(a1, c1, "m {A:1}");
        assert_clear("broadcast", &d);
        let l = layout(&d, false);
        let label = l.labels[1].as_ref().unwrap().rect();
        assert!(
            l.lane_y.iter().all(|&y| y < label.y0 || y > label.y1),
            "the label sits on a lane line"
        );
    }

    #[test]
    fn multi_line_values_share_a_left_edge() {
        let mut d = SpaceTime::new("t");
        let a = d.lane("A");
        d.event(a, "", "ab\nabcd\nabc");
        let svg = space_time_svg("fig", &d, false, At::End);
        assert!(svg.contains("ab\u{a0}\u{a0}<tspan"), "{svg}");
        assert!(svg.contains(">abcd</tspan>"), "{svg}");
        assert!(svg.contains(">abc\u{a0}</tspan>"), "{svg}");
    }

    #[test]
    fn an_event_in_two_notes_gets_one_badge_group() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "{A:1}");
        let b1 = d.event(b, "B1", "{B:1}");
        let a2 = d.event(a, "A2", "{A:2}");
        d.callout(a1, b1, "first");
        d.callout(a1, a2, "second");
        let l = layout(&d, true);
        assert_eq!(l.badges.len(), 3);
        assert_eq!(
            (l.badges[0].event, l.badges[0].notes.clone()),
            (0, vec![1, 2])
        );
        assert_clear("badges", &d);
    }

    #[test]
    fn arrows_stop_short_of_rings() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "A1", "");
        let b1 = d.event(b, "B1", "");
        d.message(a1, b1, "");
        d.callout(a1, b1, "note");
        let l = layout(&d, false);
        let (start, _) = l.segments[0];
        let dist = ((start.0 - l.x[0]).powi(2) + (start.1 - l.lane_y[0]).powi(2)).sqrt();
        assert!(dist >= RING + 2.0 - 1e-9);
    }

    #[test]
    fn a_message_in_flight_reaches_the_playheads_column() {
        let seg = ((10.0, 0.0), (110.0, 50.0));
        let mid = front(seg, 60.0);
        assert!((mid.0 - 60.0).abs() < 1e-9 && (mid.1 - 25.0).abs() < 1e-9);
        let just_sent = front(seg, 0.0); // the sender's own column
        assert!((dist(seg.0, just_sent) - HEAD).abs() < 1e-9);
        assert_eq!(front(seg, 500.0), seg.1);
    }

    /// The line of a message in flight is part of the finished line, which
    /// the layout keeps clear; its head is wider, so check that too, against
    /// everything shown at that step, to the margin `assert_clear` allows.
    #[test]
    fn messages_in_flight_stay_clear_of_what_is_shown() {
        for figure in crate::catalog::all() {
            let Diagram::SpaceTime(d) = &figure.diagram else {
                continue;
            };
            if figure.form != crate::catalog::Form::Steps {
                continue;
            }
            let (l, t) = (layout(d, false), d.timeline());
            for s in 0..=t.last() {
                let shown = |e: EventId| t.event(e) <= s;
                let mut boxes: Vec<Rect> = (0..d.events().len())
                    .filter(|&i| shown(EventId(i)))
                    .flat_map(|i| [&l.names[i], &l.annotations[i]])
                    .flatten()
                    .map(Placed::rect)
                    .collect();
                boxes.extend(
                    d.messages()
                        .iter()
                        .zip(&l.labels)
                        .filter(|(m, _)| shown(m.to))
                        .filter_map(|(_, p)| p.as_ref().map(Placed::rect)),
                );
                boxes.extend(
                    l.badges
                        .iter()
                        .filter(|b| b.notes.iter().any(|&n| t.note(n) <= s))
                        .map(Badge::rect),
                );
                for (mi, m) in d.messages().iter().enumerate() {
                    if !(t.event(m.from)..t.event(m.to)).contains(&s) {
                        continue;
                    }
                    let seg = l.segments[mi];
                    let now = t.now(s).unwrap();
                    let tip = front(seg, l.x[now.0]);
                    let (ux, uy) = unit(seg.0, tip);
                    let base = (tip.0 - ux * HEAD, tip.1 - uy * HEAD);
                    let head = [
                        tip,
                        (base.0 - uy * 3.5, base.1 + ux * 3.5),
                        (base.0 + uy * 3.5, base.1 - ux * 3.5),
                    ];
                    let markers = (0..d.events().len())
                        .filter(|&i| shown(EventId(i)) && i != m.from.0)
                        .map(|i| {
                            let r = if l.ringed[i] { RING } else { R };
                            Rect::around((l.x[i], l.lane_y[d.lane_of(EventId(i)).0]), r)
                        });
                    for r in boxes.iter().copied().chain(markers) {
                        assert!(
                            !triangle_hits(&r.grow(1.0), head), // as in assert_clear
                            "{}: {} in flight at step {s} runs into {r:?} (head {head:?})",
                            figure.stem,
                            d.message_key(mi)
                        );
                    }
                }
            }
        }
    }

    /// At each step a message is in flight, its head is drawn at the
    /// playhead's column and its line runs to the head's base.
    #[test]
    fn a_message_in_flight_is_drawn_to_the_playhead() {
        for figure in crate::catalog::all() {
            let Diagram::SpaceTime(d) = &figure.diagram else {
                continue;
            };
            if figure.form != crate::catalog::Form::Steps {
                continue;
            }
            let (l, t) = (layout(d, false), d.timeline());
            for s in 0..=t.last() {
                let svg = space_time_svg(figure.stem, d, false, At::Step(s));
                for (mi, m) in d.messages().iter().enumerate() {
                    let at = svg
                        .find(&format!(r#"data-message="{}""#, d.message_key(mi)))
                        .unwrap();
                    let group = &svg[at..at + svg[at..].find("</g>").unwrap()];
                    if !(t.event(m.from)..t.event(m.to)).contains(&s) {
                        assert!(!group.contains("ds-flight"));
                        continue;
                    }
                    let seg = l.segments[mi];
                    let tip = front(seg, l.x[t.now(s).unwrap().0]);
                    assert!(
                        group.contains(&format!(r#"points="{},{} "#, num(tip.0), num(tip.1))),
                        "{}: {}'s head at step {s}",
                        figure.stem,
                        d.message_key(mi)
                    );
                    let (ux, uy) = unit(seg.0, tip);
                    let base = (tip.0 - ux * HEAD, tip.1 - uy * HEAD);
                    let line = format!(r#"x2="{}" y2="{}""#, num(base.0), num(base.1));
                    assert_eq!(
                        group.contains(&line),
                        dist(seg.0, tip) - HEAD >= 0.5,
                        "{}: {}'s line at step {s}",
                        figure.stem,
                        d.message_key(mi)
                    );
                    assert!(!group.contains(r#"class="ds-message-line" "#));
                }
            }
        }
    }

    /// Does the triangle overlap the rectangle?
    fn triangle_hits(r: &Rect, tri: [Pt; 3]) -> bool {
        let side = |a: Pt, b: Pt, p: Pt| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        let corner = (r.x0, r.y0);
        let signs = [
            side(tri[0], tri[1], corner),
            side(tri[1], tri[2], corner),
            side(tri[2], tri[0], corner),
        ];
        let corner_inside = signs.iter().all(|&v| v >= 0.0) || signs.iter().all(|&v| v <= 0.0);
        corner_inside
            || [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                .iter()
                .any(|&(a, b)| r.crossed_by(a, b))
    }

    #[test]
    fn marks_are_keyed_and_values_carry_their_parts() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let entry = Span::part("A:1", "entry").with("actor", "A");
        let a1 = d.event(a, "A1", crate::model::rich!["{", entry, "}"]);
        let b1 = d.event(b, "B1", "");
        d.message(a1, b1, "m1");
        d.callout(a1, b1, "note");
        let svg = space_time_svg("fig", &d, false, At::End);
        for needle in [
            r#"data-figure="fig""#,
            r#"data-kind="space-time""#,
            r#"<g class="ds-lane" data-lane="a">"#,
            r#"<g class="ds-event" data-event="a-1" data-lane="a" data-notes="1">"#,
            r#"<g class="ds-message" data-message="m1" data-from="a-1" data-to="b-1" data-notes="1">"#,
            r#"<tspan class="ds-entry" data-actor="A" data-actor-n="1">A:1</tspan>"#,
            r#"<g class="ds-badge" data-note="1" data-event="a-1">"#,
            "@layer ds-figure",
        ] {
            assert!(svg.contains(needle), "missing {needle:?} in:\n{svg}");
        }
        assert!(!svg.contains("ds-captions"));
        assert!(
            space_time_svg("fig", &d, true, At::End)
                .contains(r#"<text class="ds-caption" data-note="1""#)
        );
    }
}
