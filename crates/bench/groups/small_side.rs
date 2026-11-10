// A few rows on one side and 65,536 on the other. The hash join's table is built
// once and probed 65,536 times, so the nested loop wins only while its few
// comparisons a row cost less than one lookup. Both hashers run, since where
// the hash join overtakes it depends on what a hash costs.
group! {
    name: "small_side",
    workload: SmallSide => small_side,
    params: [1, 2, 4, 8, 16, 32, 64],
    algorithms: [NestedLoop, Hash, HashSipHash],
}
