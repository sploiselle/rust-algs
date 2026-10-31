//! Writes the benchmark tables from the latest results:
//!
//! ```text
//! cargo run -p bench --release -- [--out DIR] [--allow-stale] [--allow-dirty]
//! ```
//!
//! `DIR` defaults to `target/bench/tables`. See [`bench::report`] for what the
//! flags allow.

use std::process::ExitCode;

use bench::record;
use bench::report::{self, DRIFT_LIMIT, Options};

const USAGE: &str =
    "usage: cargo run -p bench --release -- [--out DIR] [--allow-stale] [--allow-dirty]";

fn main() -> ExitCode {
    match options().and_then(|options| run(&options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        out: record::bench_dir().join("tables"),
        allow_stale: false,
        allow_dirty: false,
    };
    let mut args = std::env::args().skip(1);
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

fn run(options: &Options) -> Result<(), String> {
    let (results, warnings) = report::load(options)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    let written = report::write(&results, &options.out)
        .map_err(|err| format!("writing {}: {err}", options.out.display()))?;
    println!("wrote {} files to {}", written.len(), options.out.display());

    match report::drift(&results) {
        Some(drift) if drift > DRIFT_LIMIT => println!(
            "sentinel drift {}%, over {}%: the machine's speed changed during the run, so rerun it",
            report::sig3(drift * 100.0),
            DRIFT_LIMIT * 100.0
        ),
        Some(drift) => println!("sentinel drift {}%", report::sig3(drift * 100.0)),
        None => println!("sentinel drift unknown: a sentinel has no timing"),
    }
    let noisy = report::noisy(&results);
    if !noisy.is_empty() {
        println!(
            "{} timings (†) have a 95% confidence interval wider than ±5%; rerun a group with \
             `cargo bench -p bench --bench joins -- <group>`: {}",
            noisy.len(),
            noisy.join(", ")
        );
    }
    Ok(())
}
