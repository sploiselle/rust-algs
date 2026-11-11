// Three inputs, one selective: how much a binary plan's join order matters, and
// what skipping saves.
group! {
    name: "multiway_selective",
    workload: Selective => selective,
    params: [1 << 12, 1 << 14, 1 << 16, 1 << 18, 1 << 20],
    algorithms: [Heap, Leapfrog, CascadePlanned, CascadeUnplanned],
}
