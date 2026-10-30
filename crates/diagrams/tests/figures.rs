//! Properties every file the binary writes must have.

use std::collections::BTreeSet;

use diagrams::outputs;

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

#[test]
fn finished_figures_carry_no_steps() {
    let steps: BTreeSet<String> = diagrams::catalog::all()
        .into_iter()
        .filter(|f| f.form() == diagrams::catalog::Form::Steps)
        .map(|f| format!("{}.html", f.stem))
        .collect();
    for (name, text) in outputs() {
        if steps.contains(&name) {
            continue;
        }
        for marker in ["data-in=", "data-steps", "ds-play"] {
            assert!(!text.contains(marker), "figures/{name} has {marker}");
        }
    }
}

#[test]
fn html_figures_have_no_blank_lines() {
    for (name, text) in outputs() {
        if name.ends_with(".html") {
            assert!(
                !text.contains("\n\n"),
                "figures/{name}: a blank line would end the Markdown HTML block"
            );
        }
    }
}
