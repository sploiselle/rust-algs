use super::*;
use crate::Checksum;

#[test]
fn matches_first_keeps_every_row_and_puts_the_matches_first() {
    let input = workload::one_to_one(1 << 10, 0);
    let grouped = matches_first(&input.left, &input.right);
    let held: std::collections::HashSet<u64> = input.left.iter().map(|row| row.0).collect();
    let matched = grouped
        .iter()
        .take_while(|row| held.contains(&row.0))
        .count();
    assert_eq!(matched as u64, input.expected);
    assert!(grouped[matched..].iter().all(|row| !held.contains(&row.0)));
    let (mut a, mut b) = (grouped, input.right.clone());
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b);
}

#[test]
fn the_clocked_join_matches_the_join() {
    let input = workload::one_to_one(1 << 12, 0);
    let mut joined = Checksum::default();
    hash_join(&input.left, &input.right, FixedState::default(), |l, r| {
        joined.pair(l, r);
    });
    let mut clocked = Checksum::default();
    timed_hash_join(
        &input.left,
        &input.right,
        FixedState::default(),
        &mut clocked,
    );
    assert_eq!(joined, clocked);
}

// The counts a sweep reports are the whole process's, and the tests share
// one, so this checks the work itself; the run reports its instructions.
#[test]
fn no_rounds_of_work_leave_a_sweep_as_it_was() {
    assert_eq!(work::<0>(12_345), 0);
    assert_ne!(work::<4>(12_345), 0);
    assert_ne!(work::<8>(12_345), work::<16>(12_345));
    let lines: Vec<u32> = (0..1 << 10).collect();
    let cells = vec![1u32; 1 << 10];
    let (ns, instructions) = worked_sweep::<4>(&cells, &lines);
    assert!(ns > 0.0 && instructions > 0.0);
}

#[test]
fn an_absent_probe_finds_nothing_with_each_hashing() {
    let probes = time_absent_probes(1 << 10);
    let labels: Vec<&str> = probes.iter().map(|p| p.hashing.as_str()).collect();
    assert_eq!(labels, ABSENT_HASHINGS.map(Hashing::label));
    assert!(probes.iter().all(|p| p.ns > 0.0 && p.instructions > 0.0));
}

#[test]
fn every_hashing_finds_the_same_pairs() {
    // Odd, so the last batch is short.
    let input = workload::one_to_one((1 << 12) + 3, 0);
    let mut plain = Checksum::default();
    Hashing::Fold.join(&input.left, &input.right, &mut plain);
    for hashing in HASHINGS {
        let mut joined = Checksum::default();
        hashing.join(&input.left, &input.right, &mut joined);
        assert_eq!(joined, plain, "{hashing:?}");
    }
}

#[test]
fn a_batched_build_keeps_the_last_row_with_a_key() {
    let mut pairs = Vec::new();
    let mut out = Collect(&mut pairs);
    batched_hash_join::<16, _>(
        &[(1, 10), (1, 11)],
        &[(1, 20)],
        FixedState::default(),
        &mut out,
    );
    assert_eq!(pairs, [((1, 11), (1, 20))]);
}

struct Collect<'a>(&'a mut Vec<(Row, Row)>);

impl Sink for Collect<'_> {
    fn pair(&mut self, left: &Row, right: &Row) {
        self.0.push((*left, *right));
    }

    fn tuple(&mut self, _: &[&Row]) {
        unreachable!("a binary join")
    }

    fn tuple_permuted(&mut self, _: &[&Row], _: &[usize]) {
        unreachable!("a binary join")
    }
}

#[test]
fn padding_runs_more_instructions() {
    let keys: Vec<u64> = (0..1 << 16).collect();
    let count = |hash: &dyn Fn(u64) -> u64| {
        let before = instructions();
        black_box(
            keys.iter()
                .fold(0u64, |sum, &k| sum.wrapping_add(hash(black_box(k)))),
        );
        instructions() - before
    };
    let plain = count(&|k| FixedState::default().hash_one(k));
    let padded = count(&|k| Padded::<32>.hash_one(k));
    assert!(
        padded > plain + 32 * keys.len() as u64,
        "{plain} then {padded}"
    );
}

#[test]
fn a_chase_visits_every_line_once_a_cycle() {
    let mut cells = vec![0u32; 1 << 10];
    let lines: Vec<u32> = (0..16).map(|line| line * 32).collect();
    chase(&mut cells, &lines, &mut Rng::seeded("test"));
    let mut at = 0;
    let mut seen = Vec::new();
    for _ in 0..lines.len() {
        seen.push(at);
        at = cells[at as usize];
    }
    assert_eq!(at, 0);
    seen.sort_unstable();
    assert_eq!(seen, lines);
}
