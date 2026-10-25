//! An HTML `<figure>` for inlining into a page: the SVG without its captions,
//! and the notes as a `<figcaption>` list the page typesets itself.
//!
//! The notes are the same generated, asserted sentences the SVG would draw;
//! only the typesetting moves to the page, so prose can wrap, stay readable on
//! a phone and reach screen readers. Library values inside a note become
//! `<code class="ds-v">`, with their parts tagged as in the SVG (`data-actor`,
//! `data-dot`), and each note carries the `data-note` number its badges show.
//!
//! The fragment has no blank lines, so Markdown passes it through as one raw
//! HTML block.

use super::Render;
use super::steps::At;
use super::svg::{
    dot_line_svg, escape_attr, one_line, relation_attr, role_classes, space_time_svg,
    with_actor_slots,
};
use crate::model::{DotLine, Rich, SpaceTime, Span};

pub struct Html;

impl Render for Html {
    fn extension(&self) -> &'static str {
        "html"
    }

    fn space_time(&self, stem: &str, d: &SpaceTime) -> Option<String> {
        let d = &with_actor_slots(d);
        let notes = d.notes();
        let items: Vec<String> = notes
            .iter()
            .map(|n| {
                format!(
                    r#"<li class="ds-note" data-note="{0}"{1}><span class="ds-note-n" aria-hidden="true">{0}</span> <span class="ds-note-body">{2}</span></li>"#,
                    n.n,
                    relation_attr(&notes, n.n),
                    rich_html(&n.callout.text)
                )
            })
            .collect();
        Some(figure(
            stem,
            "space-time",
            d.title(),
            &space_time_svg(stem, d, false, At::End),
            &items,
        ))
    }

    fn dot_line(&self, stem: &str, d: &DotLine) -> Option<String> {
        Some(figure(
            stem,
            "dot-line",
            d.title(),
            &dot_line_svg(stem, d),
            &[],
        ))
    }
}

fn figure(stem: &str, kind: &str, title: &str, svg: &str, notes: &[String]) -> String {
    let mut out = format!(
        "<figure class=\"ds-fig\" data-figure=\"{}\" data-kind=\"{kind}\">\n<div class=\"ds-fig-canvas\">\n{}\n</div>\n<figcaption class=\"ds-fig-caption\">\n<p class=\"ds-fig-title\">{}</p>\n",
        escape_attr(stem),
        svg.trim_end(),
        text(&one_line(title))
    );
    if !notes.is_empty() {
        out.push_str("<ol class=\"ds-notes\">\n");
        for n in notes {
            out.push_str(n);
            out.push('\n');
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
    use crate::model::rich;

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
