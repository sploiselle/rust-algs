// k inputs that each hold every key: when a heap pays against a scan of every
// input's next key, set against multiway_sparse.
group! {
    name: "multiway_dense",
    workload: Dense => dense,
    params: [2, 4, 8, 16, 32, 64],
    algorithms: [Heap, Scan, Leapfrog],
}
