//! The facts: what a post says in a sentence, checked against the results.

use super::*;

/// A number the prose quotes, computed here so none is worked out by hand.
pub struct Fact {
    pub name: &'static str,
    pub value: Value,
}

/// How a fact is computed.
pub enum Value {
    /// One quantity over another.
    Ratio(Quantity, Quantity),
    /// One quantity.
    Single(Quantity),
    /// The smallest parameter of `group` from which `faster` is faster than
    /// `slower` at every larger one, as far as the confidence intervals can
    /// tell: see [`Crossover`].
    Crossover {
        group: &'static str,
        faster: Algorithm,
        slower: Algorithm,
    },
    /// The largest of the groups' drifts of one sentinel: see [`drifts`].
    Drift(Sentinel),
}

/// A measure of one case.
pub struct Quantity {
    pub group: &'static str,
    pub algorithm: Algorithm,
    pub param: usize,
    pub measure: Measure,
}

/// What's measured.
#[derive(Clone, Copy)]
pub enum Measure {
    /// The median time, in ns.
    Median,
    /// The median time per input row, in ns.
    PerRow,
    Peak,
    Allocations,
    InputBytes,
}

pub(super) const fn q(
    group: &'static str,
    algorithm: Algorithm,
    param: usize,
    measure: Measure,
) -> Quantity {
    Quantity {
        group,
        algorithm,
        param,
        measure,
    }
}

impl Measure {
    /// `value`, a measure of this kind, with its unit.
    pub(super) fn show(self, value: f64) -> String {
        match self {
            Measure::Median | Measure::PerRow => duration(value),
            Measure::Peak | Measure::InputBytes => bytes(value as u64),
            Measure::Allocations => count(value as u64),
        }
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let measure = match self.measure {
            Measure::Median => "median".to_owned(),
            Measure::PerRow => "ns per input row".to_owned(),
            Measure::Peak => "peak".to_owned(),
            Measure::Allocations => "allocations".to_owned(),
            Measure::InputBytes => "input bytes".to_owned(),
        };
        write!(
            f,
            "{}/{}/{} {measure}",
            self.group,
            self.algorithm.name(),
            self.param
        )
    }
}

/// The facts the prose quotes, in the order it meets them.
pub const FACTS: &[Fact] = {
    use Algorithm::*;
    use Measure::*;
    const K1: usize = 1 << 10;
    const K16: usize = 1 << 14;
    const K64: usize = 1 << 16;
    const M1: usize = 1 << 20;
    const M4: usize = 1 << 22;
    const fn fact(name: &'static str, value: Value) -> Fact {
        Fact { name, value }
    }
    const fn ratio(group: &'static str, over: Algorithm, under: Algorithm, param: usize) -> Value {
        Value::Ratio(
            q(group, over, param, Median),
            q(group, under, param, Median),
        )
    }
    &[
        fact(
            "nested_over_hash_1K",
            ratio("one_to_one", NestedLoop, Hash, K1),
        ),
        fact(
            "nested_over_hash_64K",
            ratio("one_to_one", NestedLoop, Hash, K64),
        ),
        fact(
            "hash_ns_16K",
            Value::Single(q("one_to_one", Hash, K16, PerRow)),
        ),
        fact(
            "hash_ns_1M",
            Value::Single(q("one_to_one", Hash, M1, PerRow)),
        ),
        fact(
            "hash_ns_4M",
            Value::Single(q("one_to_one", Hash, M4, PerRow)),
        ),
        fact(
            "hash_4M_over_16K_per_row",
            Value::Ratio(
                q("one_to_one", Hash, M4, PerRow),
                q("one_to_one", Hash, K16, PerRow),
            ),
        ),
        fact(
            "hash_peak_over_input_1M",
            Value::Ratio(
                q("one_to_one", Hash, M1, Peak),
                q("one_to_one", Hash, M1, InputBytes),
            ),
        ),
        fact(
            "sort_merge_over_hash_16K",
            ratio("one_to_one", SortMerge, Hash, K16),
        ),
        fact(
            "sort_merge_over_hash_4M",
            ratio("one_to_one", SortMerge, Hash, M4),
        ),
        fact("merge_over_hash_4M", ratio("one_to_one", Merge, Hash, M4)),
        fact(
            "merge_over_hash_sorted_1M",
            ratio("one_to_one", Merge, HashSorted, M1),
        ),
        fact(
            "hash_sorted_over_hash_1M",
            ratio("one_to_one", HashSorted, Hash, M1),
        ),
        fact(
            "small_m1_nested_over_hash",
            ratio("small_side", NestedLoop, Hash, 1),
        ),
        fact(
            "hash_nested_crossover_small_m",
            Value::Crossover {
                group: "small_side",
                faster: Hash,
                slower: NestedLoop,
            },
        ),
        fact(
            "siphash_nested_crossover_small_m",
            Value::Crossover {
                group: "small_side",
                faster: HashSipHash,
                slower: NestedLoop,
            },
        ),
        fact(
            "hash_nested_crossover_both_small_n",
            Value::Crossover {
                group: "both_small",
                faster: Hash,
                slower: NestedLoop,
            },
        ),
        fact(
            "siphash_nested_crossover_both_small_n",
            Value::Crossover {
                group: "both_small",
                faster: HashSipHash,
                slower: NestedLoop,
            },
        ),
        fact(
            "siphash_over_foldhash_16K",
            ratio("hash_variants", HashSipHash, Hash, K16),
        ),
        fact(
            "siphash_over_foldhash_1M",
            ratio("hash_variants", HashSipHash, Hash, M1),
        ),
        fact(
            "siphash_over_foldhash_4M",
            ratio("hash_variants", HashSipHash, Hash, M4),
        ),
        fact(
            "unplanned_over_planned_1M",
            ratio("multiway_selective", CascadeUnplanned, CascadePlanned, M1),
        ),
        fact(
            "unplanned_peak_over_planned_1M",
            Value::Ratio(
                q("multiway_selective", CascadeUnplanned, M1, Peak),
                q("multiway_selective", CascadePlanned, M1, Peak),
            ),
        ),
        fact(
            "unplanned_peak_over_input_1M",
            Value::Ratio(
                q("multiway_selective", CascadeUnplanned, M1, Peak),
                q("multiway_selective", CascadeUnplanned, M1, InputBytes),
            ),
        ),
        fact(
            "unplanned_over_heap_1M",
            ratio("multiway_selective", CascadeUnplanned, Heap, M1),
        ),
        fact(
            "planned_over_heap_1M",
            ratio("multiway_selective", CascadePlanned, Heap, M1),
        ),
        fact(
            "heap_scan_crossover_dense_k",
            Value::Crossover {
                group: "multiway_dense",
                faster: Heap,
                slower: Scan,
            },
        ),
        fact(
            "heap_over_scan_dense_k64",
            ratio("multiway_dense", Heap, Scan, 64),
        ),
        fact(
            "heap_scan_crossover_sparse_k",
            Value::Crossover {
                group: "multiway_sparse",
                faster: Heap,
                slower: Scan,
            },
        ),
        fact(
            "scan_over_heap_sparse_k64",
            ratio("multiway_sparse", Scan, Heap, 64),
        ),
        fact(
            "heap_over_leapfrog_dense_k64",
            ratio("multiway_dense", Heap, Leapfrog, 64),
        ),
        fact(
            "heap_over_leapfrog_sparse_k64",
            ratio("multiway_sparse", Heap, Leapfrog, 64),
        ),
        fact(
            "heap_over_leapfrog_selective_1M",
            ratio("multiway_selective", Heap, Leapfrog, M1),
        ),
        fact("sentinel_drift", Value::Drift(Sentinel::Narrow)),
        fact("wide_sentinel_drift", Value::Drift(Sentinel::Wide)),
    ]
};

impl Fact {
    /// The cases whose timings it reads, as (group, algorithm, parameter).
    pub(super) fn timed(&self) -> Vec<(&'static str, Algorithm, usize)> {
        let quantities = match &self.value {
            Value::Ratio(a, b) => vec![a, b],
            Value::Drift(_) => vec![],
            Value::Single(a) => vec![a],
            Value::Crossover {
                group,
                faster,
                slower,
            } => {
                let params = cases::group(group).params.iter();
                return params
                    .flat_map(|&param| [(*group, *faster, param), (*group, *slower, param)])
                    .collect();
            }
        };
        quantities
            .into_iter()
            .filter(|q| matches!(q.measure, Measure::Median | Measure::PerRow))
            .map(|q| (q.group, q.algorithm, q.param))
            .collect()
    }

    /// Its value, its numerator and its denominator, as table cells, and
    /// whether the value is unsure: it reads a noisy timing, or it's a
    /// crossover the confidence intervals don't settle.
    pub(super) fn cells(&self, results: &Results) -> ([String; 3], bool) {
        let missing = || MISSING.to_owned();
        match &self.value {
            Value::Ratio(over, under) => {
                let ratio = results.quantity(over).zip(results.quantity(under));
                let value = ratio.map_or_else(missing, |(o, u)| format!("{}×", sig3(o / u)));
                let unsure = results.noisy(over) || results.noisy(under);
                ([value, over.to_string(), under.to_string()], unsure)
            }
            Value::Single(quantity) => {
                let value = results
                    .quantity(quantity)
                    .map_or_else(missing, |v| quantity.measure.show(v));
                let unsure = results.noisy(quantity);
                ([value, quantity.to_string(), "—".to_owned()], unsure)
            }
            Value::Crossover {
                group,
                faster,
                slower,
            } => {
                let crossover = crossover(results, group, *faster, *slower);
                let unsure = matches!(crossover, Some(Crossover::Between(..)));
                let value = crossover.map_or_else(missing, |c| c.to_string());
                let param = cases::group(group).workload.param();
                let id = |alg: &Algorithm| format!("{group}/{}/{param} median", alg.name());
                ([value, id(faster), id(slower)], unsure)
            }
            Value::Drift(sentinel) => {
                let value = drift(results, *sentinel)
                    .map_or_else(missing, |drift| format!("{}%", sig3(drift * 100.0)));
                let over = match sentinel {
                    Sentinel::Narrow => "the largest drift of any group's sentinel",
                    Sentinel::Wide => "the largest drift of any group's wide sentinel",
                }
                .to_owned();
                ([value, over, "—".to_owned()], false)
            }
        }
    }
}

/// Where one algorithm overtakes another as a group's parameter grows.
#[derive(Debug, PartialEq, Eq)]
pub enum Crossover {
    /// At this parameter: below it the first is clearly slower, and from it on
    /// clearly faster, their confidence intervals apart at every parameter.
    /// `None` if it's clearly slower at every one.
    Settled(Option<usize>),
    /// Somewhere from the first parameter where it isn't clearly slower, to
    /// the one from which it's clearly faster (`None` if it never settles):
    /// in between, the intervals overlap or a timing is noisy.
    Between(usize, Option<usize>),
}

impl fmt::Display for Crossover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Crossover::Settled(Some(at)) => write!(f, "{}", param_label(*at)),
            Crossover::Settled(None) => write!(f, "none"),
            Crossover::Between(from, Some(to)) => {
                write!(f, "{}–{}", param_label(*from), param_label(*to))
            }
            Crossover::Between(from, None) => write!(f, "≥ {}", param_label(*from)),
        }
    }
}

/// Where `faster` overtakes `slower` across `group`'s parameters, or `None` if
/// a timing is missing.
pub(super) fn crossover(
    results: &Results,
    group: &str,
    faster: Algorithm,
    slower: Algorithm,
) -> Option<Crossover> {
    let params = cases::group(group).params;
    // At each parameter, whether `faster` is clearly faster, clearly slower,
    // or neither (None).
    let mut clearly = Vec::new();
    for &param in params {
        let (f, s) = (
            results.timing(group, faster, param)?,
            results.timing(group, slower, param)?,
        );
        clearly.push(if f.noisy() || s.noisy() {
            None
        } else if f.upper_ns < s.lower_ns {
            Some(true)
        } else if f.lower_ns > s.upper_ns {
            Some(false)
        } else {
            None
        });
    }
    // The first parameter where it isn't clearly slower, and the first from
    // which it's clearly faster at every one.
    let from = clearly.iter().position(|c| *c != Some(false));
    let settled = clearly
        .iter()
        .rposition(|c| *c != Some(true))
        .map_or(0, |i| i + 1);
    let to = params.get(settled).copied();
    Some(match from {
        None => Crossover::Settled(None),
        Some(i) if Some(params[i]) == to => Crossover::Settled(to),
        Some(i) => Crossover::Between(params[i], to),
    })
}

/// The facts as a table. Its caption cites the timing run, and the memory run
/// the peaks and allocations come from.
pub(super) fn facts(results: &Results) -> String {
    let header = ["fact", "value", "numerator", "denominator"].map(str::to_owned);
    let mut unsure = Vec::new();
    let rows: Vec<Vec<String>> = FACTS
        .iter()
        .map(|fact| {
            let ([mut value, over, under], marked) = fact.cells(results);
            if marked {
                value += "†";
                unsure.push(fact.name.to_owned());
            }
            vec![fact.name.to_owned(), value, over, under]
        })
        .collect();
    let what = "Each value is the numerator's measure over the denominator's, or the \
                numerator's alone where there's no denominator. A drift is the largest \
                |end / start − 1| of any group's sentinel, the same work timed before the \
                group's first case and after its last: random reads across 8 MiB, which L2 \
                holds, and for the wide sentinel across 32 MiB, which it doesn't. A crossover is the smallest parameter from which the \
                numerator is faster than the denominator at every larger one, or the range it \
                lies in where their confidence intervals overlap. Times are from";
    let reads = reads(results, FACTS.iter().flat_map(Fact::timed));
    let source = Source::every_group();
    let mut caption = time_caption(results, &source, what, &reads, UNSURE, &unsure);
    caption += &format!(
        " Peaks, allocations and input bytes are from the memory profiles at rust-algs {}.",
        sha(&results.cited(&Source::Memory))
    );
    page(
        "facts",
        results,
        &source,
        &markdown(&header, 1, rows),
        &caption,
    )
}

/// What † marks in the facts.
pub(super) const UNSURE: &str = "a fact that reads a timing whose 95% confidence interval is wider than \
                      ±5% of its median, or a crossover the intervals don't settle";
