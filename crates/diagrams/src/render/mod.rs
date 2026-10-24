//! Backends: each turns a [`Diagram`] into the text of a file.

mod mermaid;
mod svg;

pub use mermaid::Mermaid;
pub use svg::Svg;

use crate::model::{Diagram, DotLine, SpaceTime};

/// A rendering backend. One method per diagram model; a backend returns `None`
/// for a model it has no sensible picture for, and that file is simply not
/// produced.
pub trait Render {
    /// File extension, without the dot.
    fn extension(&self) -> &'static str;
    fn space_time(&self, d: &SpaceTime) -> Option<String>;
    fn dot_line(&self, d: &DotLine) -> Option<String>;

    fn render(&self, d: &Diagram) -> Option<String> {
        match d {
            Diagram::SpaceTime(s) => self.space_time(s),
            Diagram::DotLine(l) => self.dot_line(l),
        }
    }
}

/// Every backend, in the order their files are written.
pub const ALL: [&dyn Render; 2] = [&Svg, &Mermaid];
