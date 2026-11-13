// k inputs that share half of each one's keys: one_to_one, for k inputs. Every
// intermediate result of a binary plan is half an input long, where
// multiway_dense's are a whole one.
group! {
    name: "multiway_half_shared",
    workload: HalfShared => half_shared,
    params: [2, 4, 8, 16, 32, 64],
    algorithms: [Heap, CascadeUnplanned],
}
