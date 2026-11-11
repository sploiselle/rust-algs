//! Times the hash join on rows in no order and in its table's order: see
//! [`bench_joins::cache::time_order`].

use bench_joins::cache::{ORDER_SIZES, OrderPart, Sentinels, benching, time_order, write_part};

fn main() {
    let started = Sentinels::start();
    let sizes = if benching() {
        &ORDER_SIZES[..]
    } else {
        &ORDER_SIZES[..1]
    };
    let order: Vec<_> = sizes.iter().map(|&n| time_order(n)).collect();
    for o in &order {
        println!(
            "{:>9} rows a side: {:5.2} ns per input row in no order, {:5.2} in the table's; {:.1} and {:.1} instructions",
            o.rows_a_side, o.random_ns, o.ordered_ns, o.random_instructions, o.ordered_instructions
        );
    }
    let sentinels = started.finish();
    if benching() {
        write_part("order", &OrderPart { sentinels, order });
    }
}
