//! The joins, as a [`Subject`]: their plan, what their results on disk were
//! measured from, and their tables.
//!
//! What's measured is `crates/bench/joins`, and the [plan](cases) of it with
//! the memory profiles is `crates/bench/joins-plan`: packages of their own,
//! which this one links and which don't link it. So nothing here can change
//! a join's machine code, or make a result stale.

use bench_joins::cache;
use bench_joins::harness::Sentinel;

use crate::subject::{Options, Results, Subject};
use crate::targets::Target;

pub mod evidence;
pub mod model;
pub mod report;

pub use bench_joins_plan::{cases, profile};

/// The joins.
pub struct Joins;

impl Subject for Joins {
    fn name(&self) -> &'static str {
        "joins"
    }

    fn packages(&self) -> &'static [&'static str] {
        &["bench-joins", "bench-joins-plan"]
    }

    /// The plan's groups, the memory profiles, then the cache measures.
    fn targets(&self) -> Vec<&'static str> {
        cases::GROUPS
            .iter()
            .map(|group| group.name)
            .chain(["memory"])
            .chain(cache::PARTS)
            .collect()
    }

    fn read(&self) -> Result<Box<dyn Results>, String> {
        Ok(Box::new(evidence::gather()?))
    }

    fn write_tables(&self, targets: &[Target], options: &Options) -> Result<(), String> {
        let (results, warnings) = report::load(options, targets)?;
        for warning in &warnings {
            eprintln!("warning: {warning}");
        }
        let written = report::write(&results, &options.out)
            .map_err(|err| format!("writing {}: {err}", options.out.display()))?;
        println!("wrote {} files to {}", written.len(), options.out.display());

        for (sentinel, name, limit) in [
            (Sentinel::Narrow, "sentinel", evidence::DRIFT_LIMIT),
            (Sentinel::Wide, "wide sentinel", evidence::BUSY_LIMIT),
        ] {
            for (group, drift) in report::drifts(&results, sentinel) {
                if drift > limit {
                    println!(
                        "{group}: {name} drift {}%, over {}%: the machine changed while it ran, \
                         so rerun it with `run --only {group}`",
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
}
