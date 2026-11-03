//! Backends: each turns a [`Diagram`] into the text of a file.

mod html;
mod mermaid;
mod steps;
mod svg;

pub use html::{Html, steps_html};
pub use mermaid::Mermaid;
pub use svg::Svg;

use crate::model::{Diagram, DotLine, SpaceTime, Tracks};

/// A rendering backend. One method per diagram model; a backend returns `None`
/// for a model it has no sensible picture for, and that file is simply not
/// produced. `stem` is the figure's file stem, which renderers also use as its
/// address on a page (`data-figure`).
pub trait Render {
    /// File extension, without the dot.
    fn extension(&self) -> &'static str;
    fn space_time(&self, stem: &str, d: &SpaceTime) -> Option<String>;
    fn dot_line(&self, stem: &str, d: &DotLine) -> Option<String>;
    fn tracks(&self, stem: &str, d: &Tracks) -> Option<String>;

    fn render(&self, stem: &str, d: &Diagram) -> Option<String> {
        match d {
            Diagram::SpaceTime(s) => self.space_time(stem, s),
            Diagram::DotLine(l) => self.dot_line(stem, l),
            Diagram::Tracks(t) => self.tracks(stem, t),
        }
    }
}

/// Every backend, in the order their files are written.
pub const ALL: [&dyn Render; 3] = [&Svg, &Html, &Mermaid];
