//! Brings the benchmark results up to date, and writes the tables from them:
//!
//! ```text
//! cargo run -p bench --release -- run [--all] [--only TARGET,...] [--list] [-- ARGS]
//! cargo run -p bench --release -- keep --because WHY [--only TARGET,...] [--list]
//! cargo run -p bench --release -- [--out DIR] [--allow-stale] [--allow-dirty]
//! ```
//!
//! `run` builds every bench target and runs those whose results didn't come
//! from the binary they build to now: see [`bench::joins::report::runner`].
//! `--list` says what would run. `--only` runs the named targets whether or
//! not they are out of date, and `ARGS` are passed on to them, so
//! `run --only one_to_one -- hash/1048576` reruns one noisy case.
//!
//! `keep` runs nothing. It records that the results on disk stand for the
//! binaries the targets build to now, for a change that moved or reorganised
//! what's measured without changing it. `WHY` goes on the record.
//!
//! Without either, it writes the tables into `DIR`, by default
//! `target/bench/tables`. See [`bench::joins::report`] for what the flags
//! allow.

use std::process::ExitCode;

use bench::joins::harness::Sentinel;
use bench::joins::report::runner::{self, Keep, Request};
use bench::joins::report::{self as report, BUSY_LIMIT, DRIFT_LIMIT, Options};
use bench::paths;

const USAGE: &str = "usage: cargo run -p bench --release -- run [--all] [--only TARGET,...] \
                     [--list] [-- ARGS]\n       cargo run -p bench --release -- keep --because WHY \
                     [--only TARGET,...] [--list]\n       cargo run -p bench --release -- [--out \
                     DIR] [--allow-stale] [--allow-dirty]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let done = if args.peek().is_some_and(|arg| arg == "run") {
        request(args.skip(1)).and_then(|request| runner::run(&request))
    } else if args.peek().is_some_and(|arg| arg == "keep") {
        keep(args.skip(1)).and_then(|keep| runner::keep(&keep))
    } else {
        options(args).and_then(|options| tables(&options))
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn request(mut args: impl Iterator<Item = String>) -> Result<Request, String> {
    let mut request = Request::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--all" => request.all = true,
            "--list" => request.list = true,
            "--only" => {
                let names = args.next().ok_or("--only needs a target")?;
                request.only.extend(names.split(',').map(str::to_owned));
            }
            "--" => request.args.extend(args.by_ref()),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unexpected argument {other:?}\n{USAGE}")),
        }
    }
    Ok(request)
}

fn keep(mut args: impl Iterator<Item = String>) -> Result<Keep, String> {
    let mut keep = Keep {
        only: Vec::new(),
        because: String::new(),
        list: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => keep.list = true,
            "--only" => {
                let names = args.next().ok_or("--only needs a target")?;
                keep.only.extend(names.split(',').map(str::to_owned));
            }
            "--because" => keep.because = args.next().ok_or("--because needs a reason")?,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unexpected argument {other:?}\n{USAGE}")),
        }
    }
    if keep.because.trim().is_empty() && !keep.list {
        return Err(format!(
            "keep needs --because: say why the results still stand\n{USAGE}"
        ));
    }
    Ok(keep)
}

fn options(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        out: paths::bench_dir().join("tables"),
        allow_stale: false,
        allow_dirty: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => options.out = args.next().ok_or("--out needs a directory")?.into(),
            "--allow-stale" => options.allow_stale = true,
            "--allow-dirty" => options.allow_dirty = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unexpected argument {other:?}\n{USAGE}")),
        }
    }
    Ok(options)
}

fn tables(options: &Options) -> Result<(), String> {
    let (results, warnings) = report::load(options)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    let written = report::write(&results, &options.out)
        .map_err(|err| format!("writing {}: {err}", options.out.display()))?;
    println!("wrote {} files to {}", written.len(), options.out.display());

    for (sentinel, name, limit) in [
        (Sentinel::Narrow, "sentinel", DRIFT_LIMIT),
        (Sentinel::Wide, "wide sentinel", BUSY_LIMIT),
    ] {
        for (group, drift) in report::drifts(&results, sentinel) {
            if drift > limit {
                println!(
                    "{group}: {name} drift {}%, over {}%: the machine changed while it ran, so \
                     rerun it with `run --only {group}`",
                    report::sig3(drift * 100.0),
                    limit * 100.0
                );
            }
        }
        match report::drift(&results, sentinel) {
            Some(drift) => println!("largest {name} drift {}%", report::sig3(drift * 100.0)),
            None => println!("{name} drift unknown: no {name} has a timing"),
        }
    }
    let noisy = report::noisy(&results);
    if !noisy.is_empty() {
        println!(
            "{} timings (†) have a 95% confidence interval wider than ±5%; rerun one with \
             `run --only <group> -- <algorithm>/<parameter>`: {}",
            noisy.len(),
            noisy.join(", ")
        );
    }
    Ok(())
}
