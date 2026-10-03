//! Checks `marks::log10_js` against values from Node (`Math.log10`): stdin lines "x_bits y_bits"
//! (hex), as the golden test's companion script writes them.
use std::io::BufRead;

fn main() {
    let (mut n, mut bad) = (0u64, 0u64);
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut it = line.split_whitespace().map(|h| u64::from_str_radix(h, 16).unwrap());
        let (x, y) = (f64::from_bits(it.next().unwrap()), f64::from_bits(it.next().unwrap()));
        let got = pipeline::marks::log10_js(x);
        n += 1;
        if got.to_bits() != y.to_bits() && !(got.is_nan() && y.is_nan()) {
            bad += 1;
            if bad <= 5 {
                eprintln!("log10({x:e}): {got:e} vs V8 {y:e}");
            }
        }
    }
    println!("{n} values, {bad} differ");
    std::process::exit(if bad > 0 { 1 } else { 0 });
}
