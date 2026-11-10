// The main case: two inputs of the same size that share half their keys, from 1K
// to 64M rows a side, far enough past L2 to show where the hash join's climb
// takes it against sort-merge. Leapfrog runs on the sorted inputs, to set it
// beside the merge join at two.
group! {
    name: "one_to_one",
    workload: OneToOne => one_to_one,
    params: [1 << 10, 1 << 12, 1 << 14, 1 << 16, 1 << 18, 1 << 20, 1 << 22, 1 << 24, 1 << 26],
    algorithms: [NestedLoop, Hash, HashSorted, Merge, SortMerge, Leapfrog],
}
