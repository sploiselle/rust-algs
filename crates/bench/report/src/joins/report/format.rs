//! How the tables write a time, a size, a commit and a date.

use super::*;

/// A time in ns, as ns, µs, ms or s, to three significant figures.
pub fn duration(ns: f64) -> String {
    let (value, unit) = match ns {
        ..1e3 => (ns, "ns"),
        ..1e6 => (ns / 1e3, "µs"),
        ..1e9 => (ns / 1e6, "ms"),
        _ => (ns / 1e9, "s"),
    };
    format!("{} {unit}", sig3(value))
}

/// A parameter as a row label: 64 for 64, 1K for 1,024, 4M for 4,194,304, 2G
/// for 2,147,483,648.
pub fn param_label(param: usize) -> String {
    const K: usize = 1 << 10;
    const M: usize = 1 << 20;
    const G: usize = 1 << 30;
    match param {
        p if p >= G && p % G == 0 => format!("{}G", p / G),
        p if p >= M && p % M == 0 => format!("{}M", p / M),
        p if p >= K && p % K == 0 => format!("{}K", p / K),
        p => p.to_string(),
    }
}

/// A run's commit, abbreviated, marked if the tree was dirty.
pub(super) fn sha(run: &Run) -> String {
    let dirty = if run.dirty { "-dirty" } else { "" };
    format!("{}{dirty}", short(&run.commit))
}

pub(super) fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

/// `rustc 1.98.1` from `rustc -V`'s `rustc 1.98.1 (48a229cea 2026-09-01)`.
pub(super) fn rustc(version: &str) -> String {
    version
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The UTC date and time of `unix_ms`, to the second.
pub(super) fn moment(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000 % 86_400;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    format!("{} {hours:02}:{minutes:02}:{seconds:02} UTC", date(unix_ms))
}

/// The UTC date of `unix_ms`, as YYYY-MM-DD (Hinnant's `civil_from_days`).
pub(super) fn date(unix_ms: u64) -> String {
    let days = (unix_ms / 86_400_000) as i64 + 719_468;
    let (era, day_of_era) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
