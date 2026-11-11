// A few rows on both sides. Every call allocates and fills a table for a
// handful of probes, so the nested loop wins until its quadratic comparisons
// outgrow that fixed cost.
group! {
    name: "both_small",
    workload: OneToOne => one_to_one,
    params: [2, 4, 8, 16, 32, 64, 128],
    algorithms: [NestedLoop, Hash, HashSipHash],
}
