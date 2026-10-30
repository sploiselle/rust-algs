//! Helpers shared by the unit tests.
//!
//! Most modules check their claims on executions: processes, the network
//! between them, and a schedule of what happens next. [`check`] has proptest
//! generate schedules, and when one breaks a claim, panics with that
//! schedule, shrunk. [`shortest_failure`] is for the tests that show an
//! assumption is needed by dropping it: it tries every short schedule,
//! shortest first, and returns the first that breaks a given claim, so the
//! test can say exactly what the shortest is.

use proptest::collection::vec;
use proptest::strategy::{Just, Strategy};
use proptest::test_runner::{Config, RngAlgorithm, RngSeed, TestCaseError, TestError, TestRunner};
use std::cell::RefCell;
use std::fmt::Debug;
use std::panic::{self, AssertUnwindSafe};

/// Tiny deterministic PRNG, for the tests that draw their own random
/// executions rather than use [`check`].
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// An execution the tests drive one step at a time. It panics when a claim
/// breaks.
pub trait Execution {
    /// Something that can happen next: a process acts, or the network delivers
    /// a message.
    type Step: Clone + Debug;

    /// Take `step`, checking the claims it could break, or do nothing if it
    /// can't happen now.
    ///
    /// A failing schedule shrinks by deleting steps, which only works if the
    /// steps left go on meaning what they did. So a step that can't happen
    /// does nothing rather than turn into some other step, and a step names
    /// what it acts on by something deleting other steps doesn't shift: a
    /// process, a channel, the oldest message on it, not the kth message sent.
    fn step(&mut self, step: &Self::Step);

    /// Once the schedule is over, a step that can happen now, one
    /// [`step`](Self::step) acts on rather than ignores, or `None` once
    /// nothing more can. Draining ends only when this returns `None`.
    fn drain(&self) -> Option<Self::Step>;

    /// Check the claims that hold once nothing more can happen.
    fn finish(&self) {}
}

/// How many schedules [`check`] runs of up to each length in [`LENGTHS`].
const CASES: u32 = 256;

/// The most steps in a schedule, searched in turn: a failure found among
/// short schedules shrinks more reliably to a short one.
const LENGTHS: [usize; 4] = [10, 20, 50, 200];

/// Run [`CASES`] schedules of up to each length in [`LENGTHS`], with steps
/// drawn from `steps`, each on a fresh execution from `new`, and return the
/// executions, drained and finished, for claims about what the runs covered.
/// If a schedule breaks a claim, panic with it, shrunk, and the steps that
/// drained it.
pub fn check<E: Execution>(new: impl Fn() -> E, steps: impl Strategy<Value = E::Step>) -> Vec<E> {
    let finished = RefCell::new(Vec::new());
    for most in LENGTHS {
        // A seed of its own, so its schedules aren't the start of another
        // length's.
        let seeded = Config {
            rng_seed: RngSeed::Fixed(most as u64),
            ..config()
        };
        let result = TestRunner::new(seeded).run(&vec(&steps, 0..=most), |schedule| {
            let execution =
                run(&new, &schedule).map_err(|(failure, _)| TestCaseError::fail(failure))?;
            finished.borrow_mut().push(execution);
            Ok(())
        });
        match result {
            Ok(()) => {}
            Err(TestError::Fail(_, schedule)) => {
                let mut runner = TestRunner::new(config());
                // Every panic message contains "", so this keeps any claim.
                let schedule = shorten(&mut runner, &new, schedule, "");
                let (failure, taken) = breaks(&mut runner, &new, schedule.clone(), "")
                    .expect("a shortened schedule still fails");
                let list = |steps: &[E::Step]| -> String {
                    steps.iter().map(|step| format!("\n    {step:?}")).collect()
                };
                let drained = &taken[schedule.len().min(taken.len())..];
                let drained = match drained {
                    [] => String::new(),
                    _ => format!("\nthen draining:{}", list(drained)),
                };
                panic!(
                    "{failure}\nfailing schedule, shrunk:{}{drained}",
                    list(&schedule)
                );
            }
            Err(TestError::Abort(why)) => panic!("{why}"),
        }
    }
    finished.into_inner()
}

/// The first schedule of steps from `every` whose first broken claim has
/// `claim` in its panic message, with that message, or `None`. It tries every
/// schedule of up to `most` steps, shortest first, and those of one length in
/// the order `every` lists the steps.
///
/// Trying them all, rather than sampling them, is what lets a test say that
/// the schedule is the shortest there is, made of these steps, and the same
/// whatever proptest's seed or version. There are `every.len()`ⁿ schedules of
/// `n` steps, so it's for short ones.
pub fn shortest_failure<E: Execution>(
    new: impl Fn() -> E,
    every: &[E::Step],
    most: usize,
    claim: &str,
) -> Option<(Vec<E::Step>, String)> {
    // It generates nothing, only keeps the executions' panics quiet.
    let mut runner = TestRunner::new(config());
    for len in 0..=most {
        // Each schedule of `len` steps, as positions in `every`, counted up
        // like an odometer.
        let mut picks = vec![0; len];
        loop {
            let schedule: Vec<E::Step> = picks.iter().map(|&i| every[i].clone()).collect();
            if let Some((failure, _)) = breaks(&mut runner, &new, schedule.clone(), claim) {
                return Some((schedule, failure));
            }
            let Some(last) = picks.iter().rposition(|&i| i + 1 < every.len()) else {
                break;
            };
            picks[last] += 1;
            picks[last + 1..].fill(0);
        }
    }
    None
}

/// Delete runs of consecutive steps from `schedule`, longest first, while it
/// still breaks `claim`, until no run can go.
///
/// Proptest deletes each step once and then simplifies the ones left, which
/// can leave steps that are no longer needed, or that are needed only because
/// others are there. This also drops every step that did nothing.
fn shorten<E: Execution>(
    runner: &mut TestRunner,
    new: &impl Fn() -> E,
    mut schedule: Vec<E::Step>,
    claim: &str,
) -> Vec<E::Step> {
    'shorter: loop {
        for len in (1..=schedule.len()).rev() {
            for start in 0..=schedule.len() - len {
                let mut shorter = schedule.clone();
                shorter.drain(start..start + len);
                if breaks(runner, new, shorter.clone(), claim).is_some() {
                    schedule = shorter;
                    continue 'shorter;
                }
            }
        }
        return schedule;
    }
}

/// If the first claim `schedule` breaks is `claim`, its panic message and
/// every step taken up to it. It runs inside `runner`, whose panic hook keeps
/// the panic quiet.
fn breaks<E: Execution>(
    runner: &mut TestRunner,
    new: &impl Fn() -> E,
    schedule: Vec<E::Step>,
    claim: &str,
) -> Option<(String, Vec<E::Step>)> {
    let broke = RefCell::new(None);
    _ = runner.run_one(Just(schedule), |schedule| {
        if let Err((failure, taken)) = run(new, &schedule)
            && failure.contains(claim)
        {
            *broke.borrow_mut() = Some((failure, taken));
        }
        Ok(())
    });
    broke.into_inner()
}

/// Run `schedule` on a fresh execution from `new`, then drain and finish it.
/// Returns the execution, or the panic message of the claim it broke, with
/// every step taken up to it, the drain's included.
fn run<E: Execution>(
    new: &impl Fn() -> E,
    schedule: &[E::Step],
) -> Result<E, (String, Vec<E::Step>)> {
    let mut taken = Vec::new();
    let ran = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut execution = new();
        for step in schedule {
            taken.push(step.clone());
            execution.step(step);
        }
        while let Some(step) = execution.drain() {
            taken.push(step.clone());
            execution.step(&step);
        }
        execution.finish();
        execution
    }));
    ran.map_err(|panic| {
        // A panic's message is a `String`, or a `&str` if it was a literal.
        let failure = match panic.downcast_ref::<&str>() {
            Some(literal) => literal.to_string(),
            None => panic.downcast_ref::<String>().cloned().unwrap_or_default(),
        };
        (failure, taken)
    })
}

/// A fixed seed, and fixed values for the other settings the `PROPTEST_*`
/// environment variables could change that matter here, so that every run
/// generates and shrinks the same schedules. The rest come from
/// `Config::default()`, which reads the environment, but they're for
/// rejecting cases and sizing collections without a range, which these tests
/// don't do.
fn config() -> Config {
    Config {
        cases: CASES,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(1),
        // The default, `u32::MAX`, means four per case.
        max_shrink_iters: 100_000,
        // No time limit, which would make shrinking depend on the machine.
        max_shrink_time: 0,
        // No regression files written into the source tree.
        failure_persistence: None,
        verbose: 0,
        ..Config::default()
    }
}
