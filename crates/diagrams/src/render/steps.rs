//! One figure drawn at every step of a step-through.
//!
//! A step-through figure is laid out once, for its last step: laying out only
//! what has happened so far would move what is already on screen. Each mark is
//! then a function from the step to its markup, `None` while it isn't shown,
//! and [`timed`] writes each run of steps at which a mark looks the same once,
//! tagged with the steps it covers. A page shows step `s` by hiding every
//! version whose steps don't include `s`:
//!
//! - `data-in="k"`: the first step the version is shown at.
//! - `data-out="j"`: the first step it is no longer shown at, if there is one.
//!   Such a version is also hidden until a page's player takes over
//!   (`display="none"` in SVG), so without a player the figure is its last step.
//! - `data-cont=""`: the version takes over from the one before it, so a page
//!   can move the mark on rather than bring it in afresh.
//!
//! A mark with nothing to tag (shown at every step) is written as it is.

#[cfg(test)]
use std::fmt::Write as _;

/// Which moment of a figure to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum At {
    /// The last step only, untagged: the static figure.
    End,
    /// One step only, untagged: what a page's player shows at that step.
    #[cfg(test)]
    Step(usize),
    /// Every version of every mark, tagged.
    Every,
}

/// The steps a version is shown at: from `from` up to, but not including,
/// `until`, or to the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shown {
    pub from: usize,
    pub until: Option<usize>,
}

impl Shown {
    #[cfg(test)]
    pub fn contains(&self, s: usize) -> bool {
        self.from <= s && self.until.is_none_or(|u| s < u)
    }
}

/// Each maximal run of one value in `per_step` (indexed by step, `None` where
/// nothing is shown), with the steps it covers. Expanding the runs gives
/// `per_step` back; only a run that reaches the last step is open-ended.
pub(crate) fn runs<T: PartialEq + Clone>(per_step: &[Option<T>]) -> Vec<(T, Shown)> {
    let mut out: Vec<(T, Shown)> = Vec::new();
    for (s, v) in per_step.iter().enumerate() {
        let Some(v) = v else { continue };
        match out.last_mut() {
            Some((last, shown)) if shown.until == Some(s) && last == v => {
                shown.until = Some(s + 1);
            }
            _ => out.push((
                v.clone(),
                Shown {
                    from: s,
                    until: Some(s + 1),
                },
            )),
        }
    }
    for (_, shown) in &mut out {
        if shown.until == Some(per_step.len()) {
            shown.until = None;
        }
    }
    out
}

/// One mark, as `at` asks for it. `last` is the figure's last step; `off` is
/// what a version that isn't shown at the last step carries so that it stays
/// hidden without a player (` display="none"` in SVG, nothing in HTML that
/// sits in a hidden block anyway). A mark is one element, so that its tag
/// covers all of it.
pub(crate) fn timed(
    at: At,
    last: usize,
    off: &str,
    mark: impl Fn(usize) -> Option<String>,
) -> String {
    match at {
        At::End => mark(last).unwrap_or_default(),
        #[cfg(test)]
        At::Step(s) => mark(s).unwrap_or_default(),
        At::Every => {
            let per_step: Vec<Option<String>> = (0..=last).map(&mark).collect();
            let mut out = String::new();
            let mut previous: Option<Shown> = None;
            for (markup, shown) in runs(&per_step) {
                let bare = markup.trim();
                assert!(
                    top_level(&markup) == 1 && bare.starts_with('<') && bare.ends_with('>'),
                    "a mark is one element:\n{markup}"
                );
                let cont = previous.is_some_and(|p| p.until == Some(shown.from));
                previous = Some(shown);
                if shown.from == 0 && shown.until.is_none() {
                    out.push_str(&markup);
                } else {
                    out.push_str(&tag(&markup, shown, off, cont));
                }
            }
            out
        }
    }
}

/// `markup` with the steps it is shown at, written right after its first
/// `class="…"`, as `Placed::emit` writes its own attributes.
fn tag(markup: &str, shown: Shown, off: &str, cont: bool) -> String {
    let start = markup.find(r#"class=""#).expect("a timed mark has a class") + 7;
    let end = start + markup[start..].find('"').expect("the class is quoted") + 1;
    format!(
        "{}{}{}",
        &markup[..end],
        blob(shown, off, cont),
        &markup[end..]
    )
}

fn blob(shown: Shown, off: &str, cont: bool) -> String {
    let mut b = format!(r#" data-in="{}""#, shown.from);
    if let Some(until) = shown.until {
        b.push_str(&format!(r#" data-out="{until}"{off}"#));
    }
    if cont {
        b.push_str(r#" data-cont="""#);
    }
    b
}

/// One tag of some markup, as far as nesting goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tag {
    Open,
    Close,
    /// Self-closing, or an HTML void element such as `<br>`.
    Empty,
}

fn tags(markup: &str) -> impl Iterator<Item = Tag> + '_ {
    markup.match_indices('<').map(move |(i, _)| {
        let rest = &markup[i + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let inside = &rest[..end];
        let name: String = inside
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if inside.starts_with('/') {
            Tag::Close
        } else if inside.ends_with('/') || ["br", "img", "input", "hr"].contains(&name.as_str()) {
            Tag::Empty
        } else {
            Tag::Open
        }
    })
}

/// How many elements sit at the top level of `markup`.
fn top_level(markup: &str) -> usize {
    let (mut depth, mut count) = (0usize, 0usize);
    for t in tags(markup) {
        match t {
            Tag::Open => {
                if depth == 0 {
                    count += 1;
                }
                depth += 1;
            }
            Tag::Close => depth = depth.saturating_sub(1),
            Tag::Empty => {
                if depth == 0 {
                    count += 1;
                }
            }
        }
    }
    count
}

/// How much deeper a line leaves the nesting than it found it.
#[cfg(test)]
fn depth_change(line: &str) -> isize {
    tags(line)
        .map(|t| match t {
            Tag::Open => 1,
            Tag::Close => -1,
            Tag::Empty => 0,
        })
        .sum()
}

/// The steps and continuation written into `line` by [`tag`], if any.
#[cfg(test)]
fn parse_tag(line: &str) -> Option<(Shown, bool, String)> {
    let number = |key: &str| -> Option<usize> {
        let needle = format!(r#" {key}=""#);
        let i = line.find(&needle)? + needle.len();
        line[i..].split('"').next()?.parse().ok()
    };
    let from = number("data-in")?;
    let until = number("data-out");
    let off = if until.is_some() && line.contains(r#" display="none""#) {
        r#" display="none""#
    } else {
        ""
    };
    let shown = Shown { from, until };
    let cont = line.contains(r#" data-cont="""#);
    Some((shown, cont, blob(shown, off, cont)))
}

/// What a page's player shows at step `s` of `markup` written [`At::Every`]:
/// the versions shown at `s`, their tags stripped. Without `player` the page
/// has none, so the player's block and the figure's step count go too. Relies
/// on every tagged element starting a line, which `emit!` and the HTML
/// backend guarantee.
#[cfg(test)]
pub(crate) fn at_step(markup: &str, s: usize, player: bool) -> String {
    show(markup, |line| {
        if !player && line.trim_start().starts_with(r#"<div class="ds-play""#) {
            return None;
        }
        let kept = match parse_tag(line) {
            Some((shown, _, blob)) if shown.contains(s) => line.replacen(&blob, "", 1),
            Some(_) => return None,
            None => line.to_string(),
        };
        Some(if player {
            kept
        } else {
            without_step_count(&kept)
        })
    })
}

/// What a page with no player shows of `markup` written [`At::Every`]: the
/// versions not hidden, their tags stripped, and no player's block. Nothing
/// moves without a player, so what a mark needs to move goes too.
#[cfg(test)]
pub(crate) fn without_player(markup: &str) -> String {
    show(markup, |line| {
        if line.trim_start().starts_with(r#"<div class="ds-play""#) {
            return None;
        }
        match parse_tag(line) {
            Some((shown, _, _)) if shown.until.is_some() && line.contains(r#" display="none""#) => {
                None
            }
            Some((_, _, blob)) => Some(without_step_count(&line.replacen(&blob, "", 1))),
            None => Some(without_step_count(line)),
        }
    })
}

/// `markup` line by line, each line kept as `keep` rewrites it or, when
/// `keep` says `None`, dropped together with the rest of the element it opens.
#[cfg(test)]
fn show(markup: &str, keep: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut skipping: isize = 0;
    for line in markup.lines() {
        if skipping > 0 {
            skipping += depth_change(line);
            continue;
        }
        match keep(line) {
            Some(l) => writeln!(out, "{l}").expect("writing to a String cannot fail"),
            None => skipping = depth_change(line).max(0),
        }
    }
    out
}

/// `line` as a page without a player has it: no step count on the figure,
/// and nothing a mark would need to move.
#[cfg(test)]
fn without_step_count(line: &str) -> String {
    let moves = [r#" style="--ds-len:"#, r#" style="--ds-dx:"#];
    let line = &match moves.iter().find_map(|m| line.find(m)) {
        Some(i) => {
            let end = i + 8 + line[i + 8..].find('"').expect("the style is quoted") + 1;
            format!("{}{}", &line[..i], &line[end..])
        }
        None => line.to_string(),
    };
    match line.find(r#" data-steps=""#) {
        Some(i) => {
            let rest = &line[i + 13..];
            let j = rest.find('"').expect("the count is quoted") + 1;
            let mut out = format!("{}{}", &line[..i], &rest[j..]);
            out = out.replacen(r#" data-steps-v="1""#, "", 1);
            out
        }
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_give_back_every_step() {
        // Every sequence of up to six steps over {absent, a, b}.
        for len in 0..=6u32 {
            for code in 0..3usize.pow(len) {
                let per_step: Vec<Option<char>> = (0..len)
                    .map(|k| match code / 3usize.pow(k) % 3 {
                        0 => None,
                        1 => Some('a'),
                        _ => Some('b'),
                    })
                    .collect();
                let rs = runs(&per_step);
                let back: Vec<Option<char>> = (0..per_step.len())
                    .map(|s| {
                        rs.iter()
                            .find(|(_, shown)| shown.contains(s))
                            .map(|(v, _)| *v)
                    })
                    .collect();
                assert_eq!(back, per_step);
                for (k, (_, shown)) in rs.iter().enumerate() {
                    let open = shown.until.is_none();
                    assert_eq!(
                        open,
                        per_step.last().is_some_and(Option::is_some) && k == rs.len() - 1
                    );
                }
                for w in rs.windows(2) {
                    let ((a, x), (b, y)) = (&w[0], &w[1]);
                    assert!(!(x.until == Some(y.from) && a == b), "runs are maximal");
                }
            }
        }
    }

    #[test]
    fn the_tag_goes_after_the_class() {
        let shown = Shown {
            from: 2,
            until: Some(4),
        };
        assert_eq!(
            tag(
                r#"<circle class="ds-dot" r="5"/>"#,
                shown,
                r#" display="none""#,
                true
            ),
            r#"<circle class="ds-dot" data-in="2" data-out="4" display="none" data-cont="" r="5"/>"#
        );
    }

    #[test]
    #[should_panic(expected = "has a class")]
    fn a_timed_mark_needs_a_class() {
        tag(
            "<g>",
            Shown {
                from: 1,
                until: None,
            },
            "",
            false,
        );
    }

    #[test]
    #[should_panic(expected = "one element")]
    fn a_mark_is_one_element() {
        timed(At::Every, 1, "", |s| {
            (s == 1).then(|| "<b class=\"x\"></b><i class=\"y\"></i>".to_string())
        });
    }

    #[test]
    #[should_panic(expected = "one element")]
    fn a_mark_has_no_loose_text() {
        timed(At::Every, 1, "", |s| {
            (s == 1).then(|| "stray <b class=\"x\"></b>".to_string())
        });
    }

    #[test]
    fn timed_versions_tag_their_steps_and_continuations() {
        let mark = |s: usize| match s {
            0 => None,
            1 | 2 => Some("<p class=\"a\">x</p>\n".to_string()),
            _ => Some("<p class=\"a\">y</p>\n".to_string()),
        };
        let every = timed(At::Every, 3, r#" display="none""#, mark);
        assert_eq!(
            every,
            "<p class=\"a\" data-in=\"1\" data-out=\"3\" display=\"none\">x</p>\n<p class=\"a\" data-in=\"3\" data-cont=\"\">y</p>\n"
        );
        for s in 0..=3 {
            assert_eq!(
                at_step(&every, s, true),
                timed(At::Step(s), 3, "", mark),
                "step {s}"
            );
        }
        assert_eq!(without_player(&every), timed(At::End, 3, "", mark));
    }

    #[test]
    fn nesting_counts_void_and_self_closing_tags_as_empty() {
        assert_eq!(top_level("<g class=\"x\">\n  <circle/>\n</g>\n"), 1);
        assert_eq!(top_level("<span class=\"c\">a<br>b</span>"), 1);
        assert_eq!(depth_change("<g class=\"x\">"), 1);
        assert_eq!(depth_change("<td><span>a<br>b</span>"), 1);
    }
}
