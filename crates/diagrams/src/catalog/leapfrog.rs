//! Leapfrog join, drawn from the comparisons it makes.
//!
//! [`leapfrog_join`] is generic in its key, so the figures run it, unchanged,
//! on a key that writes down every comparison: which row of which input it
//! compared with which, how, and what came out, along with each tuple it
//! emits. Each turn is rebuilt from that log (who was furthest behind, the
//! target, every probe of the seek and where it landed), checked against what
//! the code says it does, and drawn a seek in two steps: its gallop, then its
//! search.

use algs::join::{leapfrog_join, multiway_merge_join};

use super::Figure;
use super::logged::{self, At, Logged};
use crate::model::{Diagram, Mark, Rich, Span, Track, Tracks, TracksFrame, actor_span, rich};

/// Runs the library's join on `inputs` and returns what it did.
fn record(inputs: &[Vec<u64>]) -> Vec<Logged> {
    let rows = logged::rows(inputs);
    let slices: Vec<&[_]> = rows.iter().map(Vec::as_slice).collect();
    logged::record(|| leapfrog_join(&slices, |tuple| logged::emitted(tuple.iter().copied())))
}

/// One turn of the loop: the input furthest behind, at row `from`, either
/// emits (it has caught up with `target`) or seeks to it.
#[derive(Clone, Debug)]
struct Turn {
    /// Where every input is when the turn starts.
    cursors: Vec<usize>,
    /// The turn's `p`: the index into `order` of the input furthest behind.
    p: usize,
    behind: usize,
    ahead: usize,
    target: u64,
    seek: Option<Seek>,
}

impl Turn {
    fn from(&self) -> usize {
        self.cursors[self.behind]
    }

    /// Where every input is when the turn ends.
    fn after(&self) -> Vec<usize> {
        let mut c = self.cursors.clone();
        c[self.behind] += match &self.seek {
            Some(s) => s.landed,
            None => 1,
        };
        c
    }
}

/// A seek's probes, as offsets from the row it starts at, as the code has
/// them: `rows[offset]` against the target.
#[derive(Clone, Debug)]
struct Seek {
    /// Each `rows[bound] < target` while galloping, and whether it was.
    gallop: Vec<(usize, bool)>,
    /// `rows[start..end]`, which the binary search covers.
    start: usize,
    end: usize,
    /// Each of the search's probes.
    search: Vec<(usize, bool)>,
    /// The first row at or past the target.
    landed: usize,
}

/// A run of the join, rebuilt from its log and checked against the code.
struct Run {
    names: Vec<&'static str>,
    inputs: Vec<Vec<u64>>,
    turns: Vec<Turn>,
    joined: Vec<u64>,
}

impl Run {
    fn key(&self, input: usize, row: usize) -> u64 {
        self.inputs[input][row]
    }

    fn name(&self, input: usize) -> Span {
        actor_span(self.names[input], self.names[input])
    }

    fn input_count(&self) -> usize {
        self.inputs.len()
    }
}

/// The selective workload's shape, at a size a figure can show: A and B hold
/// most keys and share most of them, and C holds two that both have.
pub(super) fn accounts() -> (Vec<&'static str>, Vec<Vec<u64>>) {
    let a: Vec<u64> = (0..20).filter(|k| k % 4 != 3).collect();
    let b: Vec<u64> = (0..20).filter(|k| k % 4 != 1).collect();
    (vec!["A", "B", "C"], vec![a, b, vec![6, 14]])
}

/// The join on [`accounts`], rebuilt.
fn leapfrog_run() -> Run {
    let (names, inputs) = accounts();
    let run = rebuild(names, inputs);
    assert_eq!(run.joined, [6, 14]);
    let gallops: Vec<Vec<usize>> = run
        .turns
        .iter()
        .filter_map(|t| t.seek.as_ref())
        .map(|s| s.gallop.iter().map(|g| g.0).collect())
        .collect();
    assert_eq!(
        gallops,
        [
            vec![1, 2, 4, 8],
            vec![1, 2, 4],
            vec![1, 2, 4, 8],
            vec![1, 2, 4, 8]
        ],
        "four seeks, each galloping past a stride of at least 4 rows"
    );
    run
}

/// Runs the join and rebuilds each turn from what it logged, asserting what
/// the figures draw: each turn moves the input furthest behind toward the key
/// of the input furthest ahead; a seek gallops 1, 2, 4, … rows, then searches
/// only the last stride, and lands on the first row at or past the target;
/// and the join emits what the heap merge emits.
fn rebuild(names: Vec<&'static str>, inputs: Vec<Vec<u64>>) -> Run {
    let k = inputs.len();
    let log = record(&inputs);
    let key = |at: At| inputs[at.input][at.row];
    // Everything before the first `==` is the sort into `order`.
    let first_turn = log
        .iter()
        .position(|e| matches!(e, Logged::Eq { .. }))
        .expect("the join compares");
    let mut cursors = vec![0; k];
    let mut turns: Vec<Turn> = Vec::new();
    let mut joined = Vec::new();
    let mut i = first_turn;
    while i < log.len() {
        let Logged::Eq { a, b, equal } = log[i] else {
            panic!("a turn starts with `==`, not {:?}", log[i]);
        };
        i += 1;
        let behind = a.input;
        let ahead = b.input;
        assert_eq!(a.row, cursors[behind], "a turn compares the heads");
        assert_eq!(b.row, cursors[ahead], "a turn compares the heads");
        let heads: Vec<u64> = (0..k).map(|n| inputs[n][cursors[n]]).collect();
        assert_eq!(
            heads[behind],
            *heads.iter().min().unwrap(),
            "furthest behind"
        );
        assert_eq!(heads[ahead], *heads.iter().max().unwrap(), "furthest ahead");
        let target = key(b);
        let seek = if equal {
            let Some(Logged::Emit(rows)) = log.get(i) else {
                panic!("a turn that catches up emits");
            };
            let at: Vec<At> = (0..k)
                .map(|input| At {
                    input,
                    row: cursors[input],
                })
                .collect();
            assert_eq!(rows, &at, "every input is at the target");
            joined.push(target);
            i += 1;
            None
        } else {
            let from = a.row;
            let rows = &inputs[behind][from..];
            let mut probes = Vec::new();
            while let Some(&Logged::Lt { a: x, b: y, below }) = log.get(i) {
                assert_eq!(x.input, behind, "a seek compares its own rows");
                assert_eq!(y, b, "a seek compares with the target");
                probes.push((x.row - from, below));
                i += 1;
            }
            let landed = rows.partition_point(|&r| r < target);
            Some(split(probes, rows.len(), landed))
        };
        let turn = Turn {
            cursors: cursors.clone(),
            p: turns.len() % k,
            behind,
            ahead,
            target,
            seek,
        };
        cursors = turn.after();
        turns.push(turn);
        if cursors[behind] == inputs[behind].len() {
            break;
        }
    }
    assert_eq!(i, log.len(), "the join stops once an input runs out");

    // `order` is fixed once sorted, and `p` steps around it: the inputs
    // furthest behind in the first `k` turns are `order`. A join that ends
    // sooner leaves the rest of it in first-key order.
    let mut order: Vec<usize> = turns.iter().take(k).map(|t| t.behind).collect();
    let mut rest: Vec<usize> = (0..k).filter(|i| !order.contains(i)).collect();
    rest.sort_by_key(|&i| inputs[i][0]);
    order.extend(rest);
    for t in &turns {
        assert_eq!(t.behind, order[t.p]);
        assert_eq!(t.ahead, order[(t.p + k - 1) % k]);
    }

    let rows: Vec<Vec<(u64, ())>> = inputs
        .iter()
        .map(|keys| keys.iter().map(|&key| (key, ())).collect())
        .collect();
    let slices: Vec<&[(u64, ())]> = rows.iter().map(Vec::as_slice).collect();
    let mut merged = Vec::new();
    multiway_merge_join(&slices, |tuple| merged.push(tuple[0].0));
    assert_eq!(joined, merged, "leapfrog emits what the heap merge emits");

    Run {
        names,
        inputs,
        turns,
        joined,
    }
}

/// A seek's probes, split into its gallop and its search, checked against
/// what `seek` does: the gallop probes 1, 2, 4, … rows on while each is below
/// the target, the search stays inside `rows[bound / 2..bound]`, and the seek
/// lands on the first row at or past the target.
fn split(probes: Vec<(usize, bool)>, len: usize, landed: usize) -> Seek {
    let mut bound = 1;
    let mut gallop = Vec::new();
    let mut rest = probes.into_iter().peekable();
    while bound < len {
        let (offset, below) = rest
            .next()
            .expect("a gallop probes until it passes the target");
        assert_eq!(offset, bound, "a gallop doubles");
        gallop.push((offset, below));
        if !below {
            break;
        }
        bound *= 2;
    }
    let start = bound / 2;
    let end = len.min(bound);
    let search: Vec<(usize, bool)> = rest.collect();
    assert!(
        search.iter().all(|&(o, _)| (start..end).contains(&o)),
        "the search stays inside the last stride"
    );
    let found = search
        .iter()
        .filter(|&&(_, below)| below)
        .map(|&(o, _)| o + 1)
        .chain([start])
        .max()
        .unwrap();
    assert!(
        found <= landed && landed <= end,
        "it lands where partition_point does"
    );
    Seek {
        gallop,
        start,
        end,
        search,
        landed,
    }
}

fn val(x: impl ToString) -> Span {
    Span::value(x.to_string())
}

/// `1, 2, 4 and 8`.
fn and_list(xs: &[usize]) -> String {
    match xs {
        [] => String::new(),
        [x] => x.to_string(),
        [init @ .., x] => format!(
            "{} and {x}",
            init.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn tracks(run: &Run, title: &str) -> Tracks {
    Tracks::new(
        title,
        run.names
            .iter()
            .zip(&run.inputs)
            .map(|(name, keys)| Track::input(name, keys.clone()))
            .collect(),
    )
}

/// A frame with nothing on the tracks but their cursors.
fn bare(cursors: Vec<usize>, joined: &[u64], narration: Rich) -> TracksFrame {
    TracksFrame {
        cursors: on(cursors),
        joined: joined.to_vec(),
        narration,
        ..TracksFrame::default()
    }
}

/// Every input being read, at these rows.
pub(super) fn on(cursors: Vec<usize>) -> Vec<Option<usize>> {
    cursors.into_iter().map(Some).collect()
}

fn start(run: &Run) -> Rich {
    let mut r = rich!["Each input starts at its first key: "];
    for i in 0..run.input_count() {
        let sep = match i {
            0 => "",
            i if i + 1 == run.input_count() => " and ",
            _ => ", ",
        };
        r = r
            .then(sep)
            .then(run.name(i))
            .then(" at ")
            .then(val(run.inputs[i][0]));
    }
    r.then(".")
}

/// Who is behind and ahead at the start of `t`.
fn who(run: &Run, t: &Turn) -> Rich {
    rich![
        run.name(t.behind),
        " is furthest behind, at ",
        val(run.key(t.behind, t.from())),
        ", and ",
        run.name(t.ahead),
        " furthest ahead, at ",
        val(t.target),
        ". "
    ]
}

/// What an emitting turn says: it emits, steps past, and maybe ends the join.
fn emits(run: &Run, t: &Turn) -> Rich {
    let after = t.after();
    let mut r = rich![
        "Every input is at ",
        val(t.target),
        ", so it's joined, and ",
        run.name(t.behind),
        " steps past it"
    ];
    if after[t.behind] == run.inputs[t.behind].len() {
        r = r
            .then(". ")
            .then(run.name(t.behind))
            .then(" has no rows left, so the join ends");
    }
    r.then(".")
}

/// The keys joined by the end of turn `n`.
fn joined_by(run: &Run, n: usize) -> Vec<u64> {
    let emitted = run.turns[..=n].iter().filter(|t| t.seek.is_none()).count();
    run.joined[..emitted].to_vec()
}

fn rows(n: usize) -> String {
    if n == 1 {
        "1 row".to_string()
    } else {
        format!("{n} rows")
    }
}

/// A gallop's probes as hops, each from the one before it, so the hops double.
fn hops(t: &Turn, s: &Seek) -> Vec<Mark> {
    s.gallop
        .iter()
        .enumerate()
        .map(|(j, &(offset, below))| Mark::Hop {
            track: t.behind,
            from: t.from() + if j == 0 { 0 } else { s.gallop[j - 1].0 },
            to: t.from() + offset,
            below,
        })
        .collect()
}

fn probes(t: &Turn, s: &Seek) -> Vec<Mark> {
    s.search
        .iter()
        .map(|&(offset, below)| Mark::Probe {
            track: t.behind,
            row: t.from() + offset,
            below,
        })
        .collect()
}

fn stride(t: &Turn, s: &Seek) -> Mark {
    Mark::Stride {
        track: t.behind,
        start: t.from() + s.start,
        end: t.from() + s.end,
    }
}

/// What a gallop found: the row that isn't below the target, or the end.
fn gallop_said(run: &Run, t: &Turn, s: &Seek) -> Rich {
    let offsets: Vec<usize> = s.gallop.iter().map(|g| g.0).collect();
    let r = rich![
        run.name(t.behind),
        format!(" gallops {} rows on", and_list(&offsets))
    ];
    match s.gallop.last() {
        Some(&(offset, false)) => r
            .then(", until ")
            .then(val(run.key(t.behind, t.from() + offset)))
            .then(" isn't below ")
            .then(val(t.target))
            .then("."),
        _ => r.then(", to its last row."),
    }
}

fn search_said(run: &Run, t: &Turn, s: &Seek) -> Rich {
    rich![
        "It binary-searches the last stride, ",
        format!("{} to {} rows on", s.start, s.end - 1),
        ", and lands on ",
        val(run.key(t.behind, t.from() + s.landed)),
        ", past ",
        rows(s.landed),
        "."
    ]
}

/// Each seek in two steps: its gallop's hops, then its search.
pub(super) fn leapfrog_seeks() -> Figure {
    let run = leapfrog_run();
    let mut d = tracks(
        &run,
        "Each seek gallops 1, 2, 4, … rows, then binary-searches the last stride",
    );
    d.frame(bare(vec![0; run.input_count()], &[], start(&run)));
    for (n, t) in run.turns.iter().enumerate() {
        let joined = joined_by(&run, n);
        let Some(s) = &t.seek else {
            d.frame(TracksFrame {
                cursors: on(t.after()),
                target: Some((t.target, true)),
                marks: Vec::new(),
                joined,
                heap: None,
                panel: Vec::new(),
                narration: emits(&run, t),
            });
            continue;
        };
        d.frame(TracksFrame {
            cursors: on(t.cursors.clone()),
            target: Some((t.target, false)),
            marks: hops(t, s),
            joined: joined.clone(),
            heap: None,
            panel: Vec::new(),
            narration: who(&run, t).then(gallop_said(&run, t, s)),
        });
        let mut marks = vec![stride(t, s)];
        marks.extend(probes(t, s));
        d.frame(TracksFrame {
            cursors: on(t.after()),
            target: Some((t.target, false)),
            marks,
            joined,
            heap: None,
            panel: Vec::new(),
            narration: search_said(&run, t, s),
        });
    }
    Figure {
        stem: "leapfrog-seeks",
        diagram: Diagram::Tracks(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_rebuilds_every_turn() {
        let run = leapfrog_run();
        let kinds: Vec<bool> = run.turns.iter().map(|t| t.seek.is_some()).collect();
        assert_eq!(kinds, [true, true, false, true, true, false]);
        let landed: Vec<usize> = run
            .turns
            .iter()
            .filter_map(|t| t.seek.as_ref())
            .map(|s| s.landed)
            .collect();
        assert_eq!(landed, [5, 4, 6, 6]);
    }

    /// Rebuilding holds on runs the figures don't draw: inputs that gallop
    /// off their end, and one that runs out on a seek.
    #[test]
    fn rebuilding_holds_on_other_inputs() {
        rebuild(vec!["A", "B"], vec![vec![0, 1, 2], vec![2, 9]]);
        rebuild(
            vec!["A", "B", "C"],
            vec![(0..30).collect(), vec![3, 29], (0..40).step_by(3).collect()],
        );
        rebuild(vec!["A", "B"], vec![vec![5], vec![1, 2, 3, 4]]);
    }
}
