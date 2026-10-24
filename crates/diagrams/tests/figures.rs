//! The committed files in `figures/` must be exactly what the catalog renders.

use std::collections::BTreeSet;
use std::fs;

use diagrams::{catalog, figures_dir, outputs};

#[test]
fn committed_figures_are_up_to_date() {
    let dir = figures_dir();
    let mut problems = Vec::new();
    let mut expected = BTreeSet::new();
    for (name, rendered) in outputs() {
        expected.insert(name.clone());
        match fs::read_to_string(dir.join(&name)) {
            Err(_) => problems.push(format!("figures/{name}: missing")),
            Ok(committed) if committed != rendered => {
                let (line, theirs, ours) = first_difference(&committed, &rendered);
                problems.push(format!(
                    "figures/{name}:{line}\n  committed: {}\n  rendered:  {}",
                    truncate(theirs),
                    truncate(ours)
                ));
            }
            Ok(_) => {}
        }
    }
    // Only generated extensions are policed, leaving room for hand-made assets.
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let generated = name.ends_with(".svg") || name.ends_with(".mmd");
            if generated && !expected.contains(&name) {
                problems.push(format!(
                    "figures/{name}: stray; delete it or add it to the catalog"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{}\n\nRegenerate with `cargo run -p diagrams` and commit the result.",
        problems.join("\n")
    );
}

/// The 1-based line where `a` and `b` first differ, with both versions of it.
fn first_difference<'a>(a: &'a str, b: &'a str) -> (usize, &'a str, &'a str) {
    let (mut la, mut lb) = (a.lines(), b.lines());
    let mut n = 1;
    loop {
        match (la.next(), lb.next()) {
            (Some(x), Some(y)) if x == y => n += 1,
            (None, None) => return (n, "<same lines; trailing whitespace differs>", ""),
            (x, y) => {
                return (
                    n,
                    x.unwrap_or("<end of file>"),
                    y.unwrap_or("<end of file>"),
                );
            }
        }
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s.to_string()
    }
}

#[test]
fn rendering_is_deterministic() {
    assert_eq!(outputs(), outputs());
}

#[test]
fn stems_are_unique() {
    let figures = catalog::all();
    let stems: BTreeSet<_> = figures.iter().map(|f| f.stem).collect();
    assert_eq!(stems.len(), figures.len());
}
