//! The nested loop join: every row of one input against every row of the
//! other.

/// Joins `left` and `right` on any condition: calls `emit` with every pair for
/// which `on` holds. It compares every row of `left` with every row of
/// `right`, left-major, so it calls `on` `left.len() × right.len()` times
/// however few pairs match.
pub fn nested_loop_join<L, R>(
    left: &[L],
    right: &[R],
    on: impl Fn(&L, &R) -> bool,
    mut emit: impl FnMut(&L, &R),
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
    fn pairs<L: Clone, R: Clone>(
        left: &[L],
        right: &[R],
        on: impl Fn(&L, &R) -> bool,
    ) -> Vec<(L, R)> {
        let mut pairs = Vec::new();
        nested_loop_join(left, right, on, |l, r| pairs.push((l.clone(), r.clone())));
        pairs
    }

    #[test]
    fn emits_every_pair_the_condition_accepts() {
        let customers = [(1, "ada"), (2, "grace"), (3, "edsger")];
        let orders = [(10, 2), (11, 1), (12, 2), (13, 4)];
        let joined = pairs(&customers, &orders, |c, o| c.0 == o.1);
        assert_eq!(
            joined,
            [
                ((1, "ada"), (11, 1)),
                ((2, "grace"), (10, 2)),
                ((2, "grace"), (12, 2)),
            ]
        );
    }

    /// A band join: each event with every window it falls inside. No key
    /// makes `start <= at < end` an equality, so `hash_join` and `merge_join`,
    /// which pair only equal keys, can't run it.
    #[test]
    fn joins_on_a_condition_no_key_can_express() {
        let windows = [("morning", 6, 12), ("lunch", 11, 14), ("evening", 17, 22)];
        let events = [("a", 7), ("b", 11), ("c", 15), ("d", 21)];
        let joined = pairs(&windows, &events, |&(_, start, end), &(_, at)| {
            start <= at && at < end
        });
        let names: Vec<_> = joined.iter().map(|(w, e)| (w.0, e.0)).collect();
        assert_eq!(
            names,
            [
                ("morning", "a"),
                ("morning", "b"),
                ("lunch", "b"),
                ("evening", "d"),
            ]
        );
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
        let joined = pairs(&[1, 2], &['a', 'b', 'c'], |_, _| true);
        assert_eq!(
            joined,
            [(1, 'a'), (1, 'b'), (1, 'c'), (2, 'a'), (2, 'b'), (2, 'c')]
        );
    }

    #[test]
    fn an_empty_side_emits_nothing() {
        assert!(pairs(&[] as &[u8], &[1, 2], |_, _| true).is_empty());
        assert!(pairs(&[1, 2], &[] as &[u8], |_, _| true).is_empty());
    }
}
