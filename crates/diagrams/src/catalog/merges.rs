//! The merge joins: two sorted inputs walked side by side, and any number
//! walked at once with a heap. Each runs unchanged on [`logged`] keys and is
//! drawn on tracks.

use std::cmp::Ordering;

use algs::join::{merge_join, multiway_merge_join};

use super::Figure;
use super::leapfrog::{accounts, on};
use super::logged::{self, At, Logged};
use crate::model::{
    Diagram, HeapTree, Mark, Rich, Span, Track, Tracks, TracksFrame, actor_span, rich,
};

/// The question's tables: A and B share `k1` and `k4`.
pub(super) fn question() -> (Vec<u64>, Vec<u64>) {
    (vec![0, 1, 2, 4], vec![1, 3, 4])
}

fn name(n: &'static str) -> Span {
    actor_span(n, n)
}

/// The keys every input holds, in order.
fn common(inputs: &[Vec<u64>]) -> Vec<u64> {
    inputs[0]
        .iter()
        .copied()
        .filter(|k| inputs.iter().all(|i| i.contains(k)))
        .collect()
}

/// A comparison per step: the smaller key moves past its row, and equal keys
/// join and both move.
pub(super) fn merge_join_steps() -> Figure {
    let (a, b) = question();
    let inputs = vec![a, b];
    let rows = logged::rows(&inputs);
    let log = logged::record(|| {
        merge_join(&rows[0], &rows[1], |l, r| logged::emitted([l, r]));
    });
    let label = |k: u64| Span::value(format!("k{k}"));
    let names = ["A", "B"];
    let mut d = Tracks::new(
        "A merge join compares the two heads and moves past the smaller",
        names
            .iter()
            .zip(&inputs)
            .map(|(n, k)| Track::input(n, k.clone()))
            .collect(),
    )
    .key_prefix("k");
    d.frame(TracksFrame {
        cursors: on(vec![0, 0]),
        narration: rich![
            "Each input starts at its first key: ",
            name("A"),
            " at ",
            label(inputs[0][0]),
            " and ",
            name("B"),
            " at ",
            label(inputs[1][0]),
            "."
        ],
        ..TracksFrame::default()
    });
    let mut cursors = [0usize, 0];
    let mut joined = Vec::new();
    let mut compared = 0;
    // Debug builds check the inputs are sorted first, with `<`: only the
    // step's `cmp` and the emits are the merge.
    let mut steps = log
        .iter()
        .filter(|e| matches!(e, Logged::Cmp { .. } | Logged::Emit(_)))
        .peekable();
    while let Some(e) = steps.next() {
        let &Logged::Cmp { a: l, b: r, ord } = e else {
            panic!("a step starts with a comparison, not {e:?}");
        };
        compared += 1;
        let head = |input| At {
            input,
            row: cursors[input],
        };
        assert_eq!(l, head(0), "the step compares the heads");
        assert_eq!(r, head(1), "the step compares the heads");
        let lk = inputs[0][l.row];
        let rk = inputs[1][r.row];
        let moved: Vec<usize> = match ord {
            Ordering::Less => vec![0],
            Ordering::Greater => vec![1],
            Ordering::Equal => {
                assert_eq!(
                    steps.next(),
                    Some(&Logged::Emit(vec![l, r])),
                    "equal keys join"
                );
                joined.push(lk);
                vec![0, 1]
            }
        };
        let marks = moved
            .iter()
            .map(|&t| Mark::Jump {
                track: t,
                from: cursors[t],
                to: cursors[t] + 1,
            })
            .filter(|m| matches!(m, Mark::Jump { track, to, .. } if *to < inputs[*track].len()))
            .collect();
        for &t in &moved {
            cursors[t] += 1;
        }
        let mut narration = match ord {
            Ordering::Less => rich![
                label(lk),
                " is below ",
                label(rk),
                ", so ",
                name("A"),
                " moves past it."
            ],
            Ordering::Greater => rich![
                label(rk),
                " is below ",
                label(lk),
                ", so ",
                name("B"),
                " moves past it."
            ],
            Ordering::Equal => rich![
                label(lk),
                " is in both, so it's joined, and both move past it."
            ],
        };
        if let Some(t) = (0..2).find(|&t| cursors[t] == inputs[t].len()) {
            narration = narration
                .then(" ")
                .then(name(names[t]))
                .then(" has no rows left, so the join ends.");
        }
        d.frame(TracksFrame {
            cursors: on(cursors.to_vec()),
            target: (ord == Ordering::Equal).then_some((lk, true)),
            marks,
            joined: joined.clone(),
            narration,
            ..TracksFrame::default()
        });
    }
    assert_eq!(joined, common(&inputs));
    assert!(
        compared < inputs[0].len() + inputs[1].len(),
        "fewer comparisons than rows"
    );
    assert!(
        cursors.iter().zip(&inputs).any(|(&c, i)| c == i.len()),
        "it stops once an input runs out"
    );
    Figure {
        stem: "merge-join",
        diagram: Diagram::Tracks(d),
    }
}

/// A heap entry as the code's heap holds it: an input's next key, and the
/// input. Ties on the key go to the lower input.
type Entry = (u64, usize);

/// What each heap operation compared, as pairs of entries.
type Compared = Vec<(Entry, Entry)>;

/// `BinaryHeap::push` on a min-heap: the entry goes in last and moves up
/// past every parent larger than it.
fn push(heap: &mut Vec<Entry>, e: Entry, compared: &mut Compared) {
    heap.push(e);
    let mut hole = heap.len() - 1;
    while hole > 0 {
        let parent = (hole - 1) / 2;
        compared.push((heap[hole], heap[parent]));
        if heap[hole] >= heap[parent] {
            break;
        }
        heap.swap(hole, parent);
        hole = parent;
    }
}

/// The sift down `PeekMut` does when the top is replaced: at each level the
/// smaller child, then the entry against it. Returns where the entry ends up.
fn sift_down(heap: &mut [Entry], compared: &mut Compared) -> usize {
    let end = heap.len();
    let mut hole = 0;
    let mut child = 1;
    while child + 2 <= end {
        compared.push((heap[child], heap[child + 1]));
        if heap[child + 1] <= heap[child] {
            child += 1;
        }
        compared.push((heap[hole], heap[child]));
        if heap[hole] <= heap[child] {
            return hole;
        }
        heap.swap(hole, child);
        hole = child;
        child = 2 * hole + 1;
    }
    if child + 1 == end {
        compared.push((heap[hole], heap[child]));
        if heap[child] < heap[hole] {
            heap.swap(hole, child);
            hole = child;
        }
    }
    hole
}

/// `BinaryHeap::pop`: the last entry takes the top's place, goes down by the
/// smaller child to the bottom, then up. Returns where it ends up, if any
/// entry is left.
fn pop(heap: &mut Vec<Entry>, compared: &mut Compared) -> Option<usize> {
    let last = heap.pop().expect("the heap has a top");
    if heap.is_empty() {
        return None;
    }
    heap[0] = last;
    let end = heap.len();
    let mut hole = 0;
    let mut child = 1;
    while child + 2 <= end {
        compared.push((heap[child], heap[child + 1]));
        if heap[child + 1] <= heap[child] {
            child += 1;
        }
        heap.swap(hole, child);
        hole = child;
        child = 2 * hole + 1;
    }
    if child + 1 == end {
        heap.swap(hole, child);
        hole = child;
    }
    while hole > 0 {
        let parent = (hole - 1) / 2;
        compared.push((heap[hole], heap[parent]));
        if heap[hole] >= heap[parent] {
            break;
        }
        heap.swap(hole, parent);
        hole = parent;
    }
    Some(hole)
}

/// The pairs, each smaller first, sorted: what was compared, whichever
/// order each comparison took its operands in.
fn pairs(c: &[(Entry, Entry)]) -> Vec<(Entry, Entry)> {
    let mut p: Vec<_> = c.iter().map(|&(x, y)| (x.min(y), x.max(y))).collect();
    p.sort();
    p
}

/// A comparison as the panel shows it, `6 C > 0 B`, ordered as the heap
/// orders entries.
fn compared_line(names: &[&'static str], (x, y): (Entry, Entry)) -> Rich {
    let entry = |(k, t): Entry| {
        rich![
            Span::value(format!("{k} ")),
            Span::value(names[t]).also("actor").with("actor", names[t])
        ]
    };
    let sign = if x < y { " < " } else { " > " };
    entry(x).then(Span::value(sign)).then(entry(y))
}

/// The leapfrog inputs merged with a heap, a step per row it takes: the top
/// leaves, its input's next key goes in at the top, and sifts down. The heap's
/// tree and its comparisons are read from the log: every comparison the
/// drawn heap would make, pushing, sifting or popping, is one the code made.
pub(super) fn heap_merge() -> Figure {
    let (names, inputs) = accounts();
    let k = inputs.len();
    let rows = logged::rows(&inputs);
    let slices: Vec<&[_]> = rows.iter().map(Vec::as_slice).collect();
    let log = logged::record(|| {
        multiway_merge_join(&slices, |tuple| logged::emitted(tuple.iter().copied()));
    });
    let entry = |at: At| -> Entry { (inputs[at.input][at.row], at.input) };
    // The heap's comparisons from `i` up to the next `==` or emit, leaving
    // out any with a row in `left`, rows that have left the heap: debug
    // builds check each input's next key against the round's key.
    let heap_cmps = |i: usize, left: &[At]| -> (Compared, usize) {
        let mut c = Vec::new();
        let mut j = i;
        while let Some(Logged::Cmp { a, b, .. }) = log.get(j) {
            if !left.contains(a) && !left.contains(b) {
                c.push((entry(*a), entry(*b)));
            }
            j += 1;
        }
        (c, j)
    };

    let mut d = Tracks::new(
        "A heap merge takes the smallest next key, then sifts the input's next one in",
        names
            .iter()
            .zip(&inputs)
            .map(|(n, keys)| Track::input(n, keys.clone()))
            .collect(),
    );
    let mut heap = Vec::new();
    let mut sim = Vec::new();
    for (t, keys) in inputs.iter().enumerate() {
        push(&mut heap, (keys[0], t), &mut sim);
    }
    let (made, mut i) = heap_cmps(0, &[]);
    assert_eq!(
        pairs(&made),
        pairs(&sim),
        "the pushes compare what the drawn heap does"
    );
    let mut cursors = vec![0; k];
    d.frame(TracksFrame {
        cursors: on(cursors.clone()),
        heap: Some(HeapTree {
            entries: heap.clone(),
            new: None,
        }),
        panel: made.iter().map(|&c| compared_line(&names, c)).collect(),
        narration: rich!["The heap starts with each input's first key, smallest on top."],
        ..TracksFrame::default()
    });

    let mut joined = Vec::new();
    let mut key_row: Option<At> = None;
    let mut taken: Vec<usize> = Vec::new();
    let mut takes = 0;
    while i < log.len() {
        let Logged::Eq { a, b, equal } = log[i] else {
            panic!("a take starts with `==`, not {:?}", log[i]);
        };
        i += 1;
        let first = a == b;
        if first {
            key_row = Some(b);
            taken.clear();
        } else {
            assert_eq!(
                Some(b),
                key_row,
                "the next top is compared with the round's key"
            );
        }
        if !equal {
            // The round is over: it emits if every input had the key.
            let emitted = matches!(log.get(i), Some(Logged::Emit(_)));
            assert_eq!(
                emitted,
                taken.len() == k,
                "only a key every input has joins"
            );
            if emitted {
                i += 1;
            }
            continue;
        }
        let key = entry(b).0;
        let t = a.input;
        assert_eq!(
            entry(a),
            heap[0],
            "the top of the drawn heap is the one taken"
        );
        assert_eq!(a.row, cursors[t], "an input's next row is taken");
        taken.push(t);
        cursors[t] += 1;
        let next = inputs[t].get(cursors[t]).copied();
        let mut sim = Vec::new();
        let new = match next {
            Some(n) => {
                heap[0] = (n, t);
                Some(sift_down(&mut heap, &mut sim))
            }
            None => pop(&mut heap, &mut sim),
        };
        let (made, j) = heap_cmps(i, &[a, b]);
        assert_eq!(
            pairs(&made),
            pairs(&sim),
            "the sift compares what the drawn heap does"
        );
        i = j;
        takes += 1;

        // Whether this take ends the round: the next `==` says the new top
        // doesn't have the key.
        let last = matches!(log.get(i), Some(Logged::Eq { equal: false, .. }));
        let all = last && taken.len() == k;
        if all {
            joined.push(key);
        }
        let mut narration = if first {
            rich![
                "The top, ",
                name(names[t]),
                "'s ",
                Span::value(key.to_string()),
                ", is the smallest next key, so ",
                name(names[t]),
                " moves past it."
            ]
        } else {
            rich![
                "The new top, ",
                name(names[t]),
                "'s ",
                Span::value(key.to_string()),
                ", is the same key, so ",
                name(names[t]),
                " moves past it too."
            ]
        };
        narration = match (next, new) {
            (Some(n), Some(0)) => narration
                .then(" Its next key, ")
                .then(Span::value(n.to_string()))
                .then(", goes in at the top and stays there."),
            (Some(n), Some(_)) => {
                let (rk, rt) = heap[0];
                narration
                    .then(" Its next key, ")
                    .then(Span::value(n.to_string()))
                    .then(", goes in at the top and sifts down below ")
                    .then(name(names[rt]))
                    .then("'s ")
                    .then(Span::value(rk.to_string()))
                    .then(".")
            }
            _ => narration
                .then(" ")
                .then(name(names[t]))
                .then(" has no rows left, so it leaves the heap."),
        };
        if all {
            narration = narration
                .then(" Every input had ")
                .then(Span::value(key.to_string()))
                .then(", so it's joined.");
        }
        if heap.len() < k {
            narration = narration
                .then(" With an input gone, no later key is in every input, and the join ends.");
        }
        d.frame(TracksFrame {
            cursors: on(cursors.clone()),
            target: Some((key, all)),
            joined: joined.clone(),
            heap: Some(HeapTree {
                entries: heap.clone(),
                new,
            }),
            panel: made.iter().map(|&c| compared_line(&names, c)).collect(),
            narration,
            ..TracksFrame::default()
        });
    }
    assert_eq!(joined, common(&inputs));
    let last = *inputs[2].last().unwrap();
    assert_eq!(
        takes,
        inputs
            .iter()
            .map(|i| i.iter().filter(|&&x| x <= last).count())
            .sum::<usize>(),
        "it takes every row of every input up to C's last key"
    );
    Figure {
        stem: "heap-merge",
        diagram: Diagram::Tracks(d),
    }
}
