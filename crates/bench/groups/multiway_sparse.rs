// k inputs that share only every 1,024th key: see multiway_dense.
group! {
    name: "multiway_sparse",
    workload: Sparse => sparse,
    params: [2, 4, 8, 16, 32, 64],
    algorithms: [Heap, Scan, Leapfrog],
}
