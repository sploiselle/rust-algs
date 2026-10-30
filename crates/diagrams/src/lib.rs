//! Figures for the write-ups, rendered from the reference implementations in
//! `dist-sys` so that a picture can never disagree with the code.
//!
//! - [`model`]: what a figure *is*. Plain data (lanes, events, messages) that
//!   knows nothing about clocks or registers.
//! - [`label`]: how a library value appears inside a figure, as text.
//! - [`catalog`]: the figures themselves. Each one drives the real library API,
//!   records every step, and asserts the claims the picture makes.
//! - [`render`]: backends that turn a model into the text of a file: SVG, HTML
//!   and Mermaid for a finished figure, and HTML for a step-through.
//!
//! `cargo run -p diagrams` writes every figure into the workspace's `figures/`
//! directory; the test in `tests/figures.rs` fails if those files are stale.

use std::path::PathBuf;

pub mod catalog;
pub mod label;
pub mod model;
pub mod render;

/// `<workspace>/figures`, found relative to this crate so it is the same
/// directory whatever the working directory. Cargo sets `CARGO_MANIFEST_DIR`
/// when it runs the binary or the tests, which is preferred over the path
/// baked in at compile time in case the artifacts outlive a moved checkout.
pub fn figures_dir() -> PathBuf {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    manifest_dir.join("../..").join("figures")
}

/// Every file the binary writes, as `(file name, contents)`: each finished
/// figure times each backend that renders it, each step-through's HTML, plus
/// `index.json`. The staleness test compares against exactly this list, so
/// the two can't drift apart.
pub fn outputs() -> Vec<(String, String)> {
    let figures = catalog::all();
    let mut out: Vec<(String, String)> = figures
        .iter()
        .flat_map(|figure| -> Vec<(String, String)> {
            match figure.form() {
                catalog::Form::Static => render::ALL
                    .iter()
                    .filter_map(|backend| {
                        backend
                            .render(figure.stem, &figure.diagram)
                            .map(|text| (format!("{}.{}", figure.stem, backend.extension()), text))
                    })
                    .collect(),
                catalog::Form::Steps => vec![(
                    format!("{}.html", figure.stem),
                    render::steps_html(figure.stem, &figure.diagram),
                )],
            }
        })
        .collect();
    out.push(("index.json".to_string(), index(&figures)));
    out
}

/// The figures in catalog order, which is the order a reader should meet them
/// in, with their kinds, forms and titles: what a page or a preview script
/// needs to list them without parsing the SVGs.
fn index(figures: &[catalog::Figure]) -> String {
    let rows: Vec<String> = figures
        .iter()
        .map(|f| {
            format!(
                r#"    {{ "stem": "{}", "kind": "{}", "form": "{}", "title": "{}" }}"#,
                json_escape(f.stem),
                f.diagram.kind(),
                f.form().name(),
                json_escape(f.diagram.title())
            )
        })
        .collect();
    format!("{{\n  \"figures\": [\n{}\n  ]\n}}\n", rows.join(",\n"))
}

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
