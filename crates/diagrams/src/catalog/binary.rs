//! The nested loop and hash joins on the question's tables, run unchanged on
//! [`logged`] keys.

use std::hash::{BuildHasherDefault, DefaultHasher};

use algs::join::{hash_join, nested_loop_join};

use super::Figure;
use super::logged::{self, At, Logged};
use super::merges::question;
use crate::model::{
    BuildProbe, BuildProbeFrame, Diagram, Grid, GridFrame, Lookup, Rich, Span, Track, actor_span,
    rich,
};

fn name(n: &'static str) -> Span {
    actor_span(n, n)
}

fn key(k: u64) -> Span {
    Span::value(format!("k{k}"))
}

/// Every pair compared, a step each: A's rows down, B's across.
pub(super) fn nested_loop() -> Figure {
    let (a, b) = question();
    let inputs = vec![a, b];
    let rows = logged::rows(&inputs);
    let log = logged::record(|| {
        nested_loop_join(&rows[0], &rows[1], |l, r| logged::emitted([l, r]));
    });
    let n = inputs[0].len();
    let m = inputs[1].len();
    let mut d = Grid::new(
        "A nested loop compares every pair of rows",
        Track::input("A", inputs[0].clone()),
        Track::input("B", inputs[1].clone()),
        "k",
    );
    d.frame(GridFrame {
        compared: Vec::new(),
        narration: rich![
            "For each row of ",
            name("A"),
            ", it compares the key with every row of ",
            name("B"),
            "."
        ],
    });
    let mut compared = Vec::new();
    let mut entries = log.iter().peekable();
    while let Some(e) = entries.next() {
        let &Logged::Eq { a: l, b: r, equal } = e else {
            panic!("the loop only compares and emits, not {e:?}");
        };
        let k = compared.len();
        // For each row of A, every row of B.
        assert_eq!(
            l,
            At {
                input: 0,
                row: k / m
            },
            "the loop's order"
        );
        assert_eq!(
            r,
            At {
                input: 1,
                row: k % m
            },
            "the loop's order"
        );
        if equal {
            assert_eq!(
                entries.next(),
                Some(&Logged::Emit(vec![l, r])),
                "a match is emitted"
            );
        }
        compared.push((l.row, r.row, equal));
        let lk = inputs[0][l.row];
        let rk = inputs[1][r.row];
        let mut narration = if equal {
            rich![
                name("A"),
                "'s ",
                key(lk),
                " matches ",
                name("B"),
                "'s ",
                key(rk),
                ", so the pair is joined."
            ]
        } else {
            rich![
                name("A"),
                "'s ",
                key(lk),
                " against ",
                name("B"),
                "'s ",
                key(rk),
                ": no match."
            ]
        };
        if compared.len() == n * m {
            let matched = compared.iter().filter(|p| p.2).count();
            narration = narration.then(format!(
                " That's every pair: {} comparisons for {matched} matches.",
                n * m
            ));
        }
        d.frame(GridFrame {
            compared: compared.clone(),
            narration,
        });
    }
    assert_eq!(compared.len(), n * m, "it compares every pair");
    Figure {
        stem: "nested-loop-join",
        diagram: Diagram::Grid(d),
    }
}

/// B, the smaller, builds the table a row per step; then A probes it a row
/// per step.
pub(super) fn hash() -> Figure {
    let (a, b) = question();
    assert!(b.len() < a.len(), "B is the smaller input, so it builds");
    let inputs = vec![a, b];
    let rows = logged::rows(&inputs);
    let log = logged::record(|| {
        hash_join(
            &rows[1],
            &rows[0],
            BuildHasherDefault::<DefaultHasher>::default(),
            |built, probe| logged::emitted([built, probe]),
        );
    });
    let build = &inputs[1];
    let probe = &inputs[0];
    // Each row is hashed once: every build row as it goes in, then every probe
    // row as it looks itself up. A probe's match is emitted before the next
    // row is hashed.
    let hashed: Vec<(usize, At)> = log
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Logged::Hash(at) => Some((i, *at)),
            _ => None,
        })
        .collect();
    let expected: Vec<At> = (0..build.len())
        .map(|row| At { input: 1, row })
        .chain((0..probe.len()).map(|row| At { input: 0, row }))
        .collect();
    assert_eq!(
        hashed.iter().map(|h| h.1).collect::<Vec<_>>(),
        expected,
        "it hashes every build row, then every probe row, once each"
    );
    let first_probe = hashed[build.len()].0;
    assert!(
        !log[..first_probe]
            .iter()
            .any(|e| matches!(e, Logged::Emit(_))),
        "nothing comes out until the table is built"
    );

    let mut d = BuildProbe::new(
        "A hash join puts one input in a table, then looks the other up in it",
        Track::input("B", build.clone()),
        Track::input("A", probe.clone()),
        "k",
    );
    d.frame(BuildProbeFrame {
        narration: rich![
            name("B"),
            ", the smaller input, builds the table, and ",
            name("A"),
            " probes it."
        ],
        ..BuildProbeFrame::default()
    });
    for (r, &k) in build.iter().enumerate() {
        let mut narration = rich![name("B"), "'s ", key(k), " goes into the table."];
        if r + 1 == build.len() {
            narration = narration
                .then(" All of ")
                .then(name("B"))
                .then(" is in, so probing can start.");
        }
        d.frame(BuildProbeFrame {
            built: r + 1,
            now: Some(Lookup::Insert(r)),
            narration,
            ..BuildProbeFrame::default()
        });
    }
    let mut joined = Vec::new();
    for (r, &k) in probe.iter().enumerate() {
        let start = hashed[build.len() + r].0;
        let end = hashed.get(build.len() + r + 1).map_or(log.len(), |h| h.0);
        let emits: Vec<&Vec<At>> = log[start..end]
            .iter()
            .filter_map(|e| match e {
                Logged::Emit(rows) => Some(rows),
                _ => None,
            })
            .collect();
        let found = match emits.as_slice() {
            [] => None,
            [rows] => {
                assert_eq!(rows[1], At { input: 0, row: r }, "the probe row is emitted");
                assert_eq!(build[rows[0].row], k, "with the build row its key found");
                Some(rows[0].row)
            }
            _ => panic!("a key the build input holds once matches once"),
        };
        assert_eq!(found.is_some(), build.contains(&k));
        if let Some(b) = found {
            joined.push((r, b));
        }
        let narration: Rich = match found {
            Some(_) => rich![
                name("A"),
                "'s ",
                key(k),
                " finds ",
                name("B"),
                "'s ",
                key(k),
                " in the table, so the pair is joined."
            ],
            None => rich![name("A"), "'s ", key(k), " isn't in the table."],
        };
        d.frame(BuildProbeFrame {
            built: build.len(),
            probed: r + 1,
            now: Some(Lookup::Probe { row: r, found }),
            joined: joined.clone(),
            narration,
        });
    }
    Figure {
        stem: "hash-join",
        diagram: Diagram::BuildProbe(d),
    }
}
