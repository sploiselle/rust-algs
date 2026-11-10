//! How the benches and the report write numbers: three significant figures,
//! commas between thousands, bytes in binary units.

/// `x` to three significant figures, without rounding its integer part, with
/// commas between thousands: 0.0123, 1.23, 12.3, 123, 12,346.
pub fn sig3(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return x.to_string();
    }
    let digits = x.abs().log10().floor() as i32 + 1;
    let mut decimals = (3 - digits).max(0) as usize;
    let mut text = format!("{x:.decimals$}");
    // Rounding up can carry into a new digit, as 9.996 does to 10.00.
    if decimals > 0 && text.parse::<f64>().unwrap().abs() >= 10f64.powi(digits) {
        decimals -= 1;
        text = format!("{x:.decimals$}");
    }
    match text.split_once('.') {
        Some((integer, fraction)) => format!("{}.{fraction}", thousands(integer)),
        None => thousands(&text),
    }
}

/// An integer with commas between thousands.
pub fn count(n: u64) -> String {
    thousands(&n.to_string())
}

/// `digits`, a run of digits with an optional sign, with commas between
/// thousands.
fn thousands(digits: &str) -> String {
    let (sign, digits) = digits.split_at(usize::from(digits.starts_with('-')));
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{sign}{grouped}")
}

/// A byte count as B, KiB or MiB, to three significant figures.
pub fn bytes(n: u64) -> String {
    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    match n {
        0..KIB => format!("{} B", count(n)),
        KIB..MIB => format!("{} KiB", sig3(n as f64 / KIB as f64)),
        MIB..GIB => format!("{} MiB", sig3(n as f64 / MIB as f64)),
        _ => format!("{} GiB", sig3(n as f64 / GIB as f64)),
    }
}
