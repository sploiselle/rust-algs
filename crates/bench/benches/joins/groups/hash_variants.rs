// What the hasher costs, on the main case.
group! {
    name: "hash_variants",
    workload: OneToOne => one_to_one,
    params: [1 << 14, 1 << 20, 1 << 22],
    algorithms: [Hash, HashSipHash],
}
