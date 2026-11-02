//! The nested loop join: every row of one input against every row of the
//! other.

/// Joins `left` and `right` on their keys: calls `emit` with every pair whose
/// keys are equal. It compares every row of `left` with every row of `right`,
/// left-major, so it compares `left.len() × right.len()` pairs of keys however
/// few match.
pub fn nested_loop_join<K: Eq, V>(
    left: &[(K, V)],
    right: &[(K, V)],
    mut emit: impl FnMut(&(K, V), &(K, V)),
) {
    for l in left {
        for r in right {
            if l.0 == r.0 {
                emit(l, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{Counted, comparisons};

    /// Every pair `nested_loop_join` emits, in the order it emits them.
    fn pairs<K: Eq + Clone, V: Clone>(left: &[(K, V)], right: &[(K, V)]) -> Vec<((K, V), (K, V))> {
        let mut pairs = Vec::new();
        nested_loop_join(left, right, |l, r| pairs.push((l.clone(), r.clone())));
        pairs
    }

    #[test]
    fn emits_every_pair_with_equal_keys() {
        let names = [(1, "ada"), (2, "grace"), (3, "edsger")];
        let surnames = [(3, "dijkstra"), (1, "lovelace"), (4, "turing")];
        assert_eq!(
            pairs(&names, &surnames),
            [
                ((1, "ada"), (1, "lovelace")),
                ((3, "edsger"), (3, "dijkstra")),
            ]
        );
    }

    #[test]
    fn compares_every_pair_of_keys_once() {
        // Whether some pairs match or none do.
        for (left, right) in [(7, 5), (0, 5), (7, 0)] {
            for shift in [0, 100] {
                let left: Vec<(Counted, ())> = (0..left).map(|i| (Counted(i), ())).collect();
                let right: Vec<(Counted, ())> =
                    (0..right).map(|i| (Counted(i + shift), ())).collect();
                let (made, emitted) = comparisons(|| {
                    let mut emitted = 0;
                    nested_loop_join(&left, &right, |_, _| emitted += 1);
                    emitted
                });
                assert_eq!(made, (left.len() * right.len()) as u64);
                let shared = left
                    .iter()
                    .filter(|l| right.iter().any(|r| r.0 == l.0))
                    .count();
                assert_eq!(emitted, shared);
            }
        }
    }

    #[test]
    fn emits_in_left_order() {
        let joined = pairs(&[(2, 'b'), (1, 'a')], &[(1, 'x'), (2, 'y')]);
        assert_eq!(joined, [((2, 'b'), (2, 'y')), ((1, 'a'), (1, 'x'))]);
    }

    #[test]
    fn an_empty_side_emits_nothing() {
        assert!(pairs::<u8, ()>(&[], &[(1, ())]).is_empty());
        assert!(pairs::<u8, ()>(&[(1, ())], &[]).is_empty());
    }
}
