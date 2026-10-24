//! The committed files in `figures/` must be exactly what the catalog renders.

use std::collections::BTreeSet;
use std::fs;

use diagrams::{figures_dir, outputs};

#[test]
fn committed_figures_are_up_to_date() {
    let dir = figures_dir();
    let mut problems = Vec::new();
    let mut expected = BTreeSet::new();
    for (name, rendered) in outputs() {
        expected.insert(name.clone());
        let committed = match fs::read(dir.join(&name)) {
            Ok(bytes) => bytes,
            Err(e) => {
                problems.push(format!("figures/{name}: cannot read: {e}"));
                continue;
            }
        };
        if committed == rendered.as_bytes() {
            continue;
        }
        if committed.windows(2).any(|w| w == b"\r\n") {
            problems.push(format!(
                "figures/{name}: has CRLF line endings; the repo's .gitattributes keeps figures/ LF, so re-checkout the file"
            ));
            continue;
        }
        let (trimmed_c, trimmed_r) = (
            trim_newlines(&committed),
            rendered.trim_end_matches('\n').as_bytes(),
        );
        if trimmed_c == trimmed_r {
            problems.push(format!("figures/{name}: differs only in trailing newlines"));
            continue;
        }
        problems.push(format!(
            "figures/{name}: {}",
            describe_difference(&committed, rendered.as_bytes())
        ));
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

fn trim_newlines(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().rposition(|&b| b != b'\n').map_or(0, |i| i + 1);
    &bytes[..end]
}

/// Where two byte strings first differ, as line and column, with a window of
/// each around that point printed with `{:?}` so whitespace and odd bytes show.
fn describe_difference(a: &[u8], b: &[u8]) -> String {
    let at = a
        .iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let line = a[..at].iter().filter(|&&c| c == b'\n').count() + 1;
    let col = at
        - a[..at]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |i| i + 1)
        + 1;
    let window = |s: &[u8]| {
        let (lo, hi) = (at.saturating_sub(30), (at + 30).min(s.len()));
        format!("{:?}", String::from_utf8_lossy(&s[lo..hi]))
    };
    format!(
        "line {line}, column {col}\n  committed: …{}…\n  rendered:  …{}…",
        window(a),
        window(b)
    )
}

#[test]
fn rendering_is_deterministic() {
    assert_eq!(outputs(), outputs());
}

#[test]
fn output_file_names_are_unique() {
    let mut seen = BTreeSet::new();
    for (name, _) in outputs() {
        assert!(
            seen.insert(name.clone()),
            "two figures would both write figures/{name}"
        );
    }
}
