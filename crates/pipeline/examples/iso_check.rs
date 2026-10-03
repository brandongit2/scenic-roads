//! Compares `interest::isolation` with dem/interest.py's on the same points: a file of u64 n, then
//! n × f64 lon, lat, score, and Python's ia (written by the golden check's script).
fn main() {
    let b = std::fs::read(std::env::args().nth(1).expect("file")).unwrap();
    let n = u64::from_le_bytes(b[..8].try_into().unwrap()) as usize;
    let col = |k: usize| -> Vec<f64> { (0..n).map(|i| f64::from_le_bytes(b[8 + (k * n + i) * 8..16 + (k * n + i) * 8].try_into().unwrap())).collect() };
    let (lon, lat, sc, want) = (col(0), col(1), col(2), col(3));
    let t = std::time::Instant::now();
    let got = pipeline::interest::isolation(&lon, &lat, &sc);
    let el = t.elapsed();
    let ulp = (0..n).filter(|&i| got[i].to_bits() != want[i].to_bits()).count();
    // What the map gets: ia to 0.1 km, mz to 0.01 (from the unrounded ia).
    let r = pipeline::interest::py_round;
    let bad: Vec<usize> = (0..n)
        .filter(|&i| r(got[i], 1) != r(want[i], 1) || pipeline::interest::min_zoom(lat[i], got[i]) != pipeline::interest::min_zoom(lat[i], want[i]))
        .collect();
    println!("{n} points in {el:.1?}: {ulp} differ in the last bits, {} once rounded (ia, mz)", bad.len());
    for &i in bad.iter().take(5) {
        println!("  {i}: {} vs {} (lon {} lat {} score {})", got[i], want[i], lon[i], lat[i], sc[i]);
    }
    std::process::exit(if bad.is_empty() { 0 } else { 1 });
}
