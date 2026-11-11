//! Brings the benchmark results up to date, and writes the tables from them:
//!
//! ```text
//! cargo run -p bench --release -- run [--all] [--only TARGET,...] [--list] [-- ARGS]
//! cargo run -p bench --release -- keep --because WHY (--only TARGET,... | --all) [--list]
//! cargo run -p bench --release -- [--out DIR] [--allow-stale] [--allow-dirty]
//! ```
//!
//! `run` builds every bench target and runs those whose results weren't
//! measured from the source they're built from now: see [`bench::runner`].
//! `--list` says what would run. `--only` runs the named targets whether or
//! not they are out of date, and `ARGS` are passed on to them, so
//! `run --only one_to_one -- hash/1048576` reruns one noisy case.
//!
//! `keep` runs nothing. It records that the named targets' results still
//! stand, after an edit to what they're built from that left what they
//! measure alone: an import, a name, a comment. `WHY` goes on the record,
//! and `--list` says what would be kept and what has changed.
//!
//! Without either, it writes every subject's tables into `DIR`, by default
//! `target/bench/tables`.

use std::process::ExitCode;

use bench::runner::{self, Keep, Request};
use bench::subject::Options;
use bench::{paths, subjects};

const USAGE: &str = "usage: cargo run -p bench --release -- run [--all] [--only TARGET,...] \
                     [--list] [-- ARGS]\n       cargo run -p bench --release -- keep --because WHY \
                     (--only TARGET,... | --all) [--list]\n       cargo run -p bench --release \
                     -- [--out DIR] [--allow-stale] [--allow-dirty]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let subjects = subjects();
    let done = match args.peek().map(String::as_str) {
        Some("run") => request(args.skip(1)).and_then(|request| runner::run(&subjects, &request)),
        Some("keep") => keep(args.skip(1)).and_then(|keep| runner::keep(&subjects, &keep)),
        _ => options(args).and_then(|options| {
            let built = runner::built(&subjects)?;
            subjects
                .iter()
                .zip(&built)
                .try_for_each(|(subject, targets)| subject.write_tables(targets, &options))
        }),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The names after `--only`.
fn names(args: &mut impl Iterator<Item = String>) -> Result<Vec<String>, String> {
    let names = args.next().ok_or("--only needs a target")?;
    Ok(names.split(',').map(str::to_owned).collect())
}

fn help() -> ! {
    println!("{USAGE}");
    std::process::exit(0)
}

fn request(mut args: impl Iterator<Item = String>) -> Result<Request, String> {
    let mut request = Request::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--all" => request.all = true,
            "--list" => request.list = true,
            "--only" => request.only.extend(names(&mut args)?),
            "--" => request.args.extend(args.by_ref()),
            "-h" | "--help" => help(),
            other => return Err(format!("unexpected argument {other:?}\n{USAGE}")),
        }
    }
    Ok(request)
}

fn keep(mut args: impl Iterator<Item = String>) -> Result<Keep, String> {
    let mut keep = Keep {
        only: Vec::new(),
        all: false,
        because: String::new(),
        list: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--all" => keep.all = true,
            "--list" => keep.list = true,
            "--only" => keep.only.extend(names(&mut args)?),
            "--because" => keep.because = args.next().ok_or("--because needs a reason")?,
            "-h" | "--help" => help(),
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
            "-h" | "--help" => help(),
            other => return Err(format!("unexpected argument {other:?}\n{USAGE}")),
        }
    }
    Ok(options)
}
