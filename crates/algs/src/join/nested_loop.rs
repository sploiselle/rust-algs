//! The nested loop join: every row of one input against every row of the
//! other.

/// Joins `left` and `right` on any condition: calls `emit` with every pair for
/// which `on` holds. It compares every row of `left` with every row of
/// `right`, left-major, so it calls `on` `left.len() × right.len()` times
/// however few pairs match.
pub fn nested_loop_join<T>(
    left: &[T],
    right: &[T],
    on: impl Fn(&T, &T) -> bool,
    mut emit: impl FnMut(&T, &T),
) {
    for l in left {
        for r in right {
            if on(l, r) {
                emit(l, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Every pair `nested_loop_join` emits for `on`, in the order it emits
    /// them.
    fn pairs<T: Clone>(left: &[T], right: &[T], on: impl Fn(&T, &T) -> bool) -> Vec<(T, T)> {
        let mut pairs = Vec::new();
        nested_loop_join(left, right, on, |l, r| pairs.push((l.clone(), r.clone())));
        pairs
    }

    #[test]
    fn emits_every_pair_the_condition_accepts() {
        let names = [(1, "ada"), (2, "grace"), (3, "edsger")];
        let surnames = [(3, "dijkstra"), (1, "lovelace"), (4, "turing")];
        let joined = pairs(&names, &surnames, |n, s| n.0 == s.0);
        assert_eq!(
            joined,
            [
                ((1, "ada"), (1, "lovelace")),
                ((3, "edsger"), (3, "dijkstra")),
            ]
        );
    }

    /// A band join: each of one sensor's readings with every reading of the
    /// other's taken within two seconds of it. No key makes
    /// `|l - r| <= 2` an equality, so `hash_join` and `merge_join`, which
    /// pair only equal keys, can't run it.
    #[test]
    fn joins_on_a_condition_no_key_can_express() {
        let a = [("a1", 3u32), ("a2", 10), ("a3", 20)];
        let b = [("b1", 1), ("b2", 4), ("b3", 12), ("b4", 30)];
        let joined = pairs(&a, &b, |l, r| l.1.abs_diff(r.1) <= 2);
        let names: Vec<_> = joined.iter().map(|(l, r)| (l.0, r.0)).collect();
        assert_eq!(names, [("a1", "b1"), ("a1", "b2"), ("a2", "b3")]);
    }

    #[test]
    fn calls_the_condition_once_per_pair() {
        // Whether some pairs match or none do.
        let conditions: [fn(&u32, &u32) -> bool; 2] = [|l, r| l >= r, |_, _| false];
        for on in conditions {
            for (left, right) in [(7, 5), (0, 5), (7, 0)] {
                let (left, right): (Vec<u32>, Vec<u32>) =
                    ((0..left).collect(), (0..right).collect());
                let calls = Cell::new(0);
                let mut emitted = 0;
                nested_loop_join(
                    &left,
                    &right,
                    |l, r| {
                        calls.set(calls.get() + 1);
                        on(l, r)
                    },
                    |_, _| emitted += 1,
                );
                assert_eq!(calls.get(), left.len() * right.len());
                let matching = left
                    .iter()
                    .flat_map(|l| right.iter().filter(move |r| on(l, r)));
                assert_eq!(emitted, matching.count());
            }
        }
    }

    #[test]
    fn emits_left_major() {
        let joined = pairs(&[1, 2], &[7, 8, 9], |_, _| true);
        assert_eq!(joined, [(1, 7), (1, 8), (1, 9), (2, 7), (2, 8), (2, 9)]);
    }

    #[test]
    fn an_empty_side_emits_nothing() {
        assert!(pairs(&[], &[1, 2], |_, _| true).is_empty());
        assert!(pairs(&[1, 2], &[], |_, _| true).is_empty());
    }
}
