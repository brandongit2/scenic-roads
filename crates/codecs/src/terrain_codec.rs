//! The terrain's tiles as stored (docs/formats.md, Pack; the owner's decisions of 24 Oct,
//! .claude/notes/ONE-TERRAIN.md "Gate"): Terrarium RGB as lossless WebP (`cwebp -lossless -z 6
//! -exact`, no metadata: crate webp), at every zoom. Heights are rounded first: to 1 cm at each
//! place's native zoom and finer (`NATIVE_M`), to 10 cm on the coarser levels (`COARSER_M`); then
//! to Terrarium's 1/256 m (max error 0.39 cm and 5.1 cm). Chrome and Safari decode them natively
//! with every value exact; roadcore::grid::decode_terrain reads them back.

use anyhow::Result;

/// The encoding packs of terrain tiles name (their header's `encoding`, the catalogue's).
pub const ENCODING: &str = "terrarium-webp";
/// libwebp's lossless preset (cwebp's `-z`).
pub const LEVEL: u8 = 6;
/// A tile's heights at its place's native zoom or finer are rounded to this (m).
pub const NATIVE_M: f64 = 0.01;
/// A coarser tile's (m).
pub const COARSER_M: f64 = 0.1;

/// What a key naming the tiles' bytes names of their making: the encoding, libwebp's version and
/// level, the rounding. Made once a process (its parts are constants and the linked library's).
pub fn version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        format!("{ENCODING} libwebp {} z{LEVEL} exact; {NATIVE_M} m native, {COARSER_M} m coarser", webpenc::version())
    })
    .clone()
}

/// Heights `e` rounded to `quantum` metres, ties to even: a coarser level is the 2×2 mean of
/// stored tiles, which lands on a tie about a quarter of the time, and rounding those away from
/// zero would lift heights above zero by about a quarter quantum a level (the owner's, 4 Nov).
/// A tie is told within the f32 heights' own precision (a few of their ulps), since a mean of
/// stored heights is a tie only to within that.
pub fn round(e: &mut [f32], quantum: f64) {
    for v in e {
        let n = *v as f64 / quantum;
        let near = (v.abs() as f64 * f32::EPSILON as f64 * 4.0 / quantum).max(1e-9);
        let r = if ((n - n.floor()) - 0.5).abs() <= near { (n.floor() + 0.5).round_ties_even() } else { n.round() };
        *v = (r * quantum) as f32;
    }
}

/// A tile's heights (256 × 256, m) rounded to `quantum` (`NATIVE_M` or `COARSER_M`) and stored:
/// Terrarium RGB as lossless WebP. `e` is left rounded (what's stored, give or take 1/256 m).
pub fn encode(e: &mut [f32], quantum: f64) -> Result<Vec<u8>> {
    round(e, quantum);
    webpenc::lossless_rgb8(&roadcore::grid::terrarium_rgb(e), 256, 256, LEVEL).map_err(|m| anyhow::anyhow!(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A height half a quantum between two rounds to the even one, either side of zero.
    #[test]
    fn ties_round_to_even() {
        let mut e = vec![0.05f32, 0.15, 0.25, -0.05, -0.15, 1.35, 0.005, 0.015];
        round(&mut e[..6], COARSER_M);
        round(&mut e[6..], NATIVE_M);
        let want = [0.0f32, 0.2, 0.2, 0.0, -0.2, 1.4, 0.0, 0.02];
        for (a, w) in e.iter().zip(want) {
            assert!((a - w).abs() < 1e-6, "{e:?}");
        }
    }

    /// A tile comes back as stored, rounded: to the centimetre at its native zoom, to the
    /// decimetre coarser, each within Terrarium's 1/256 m of that (its f32 arithmetic included).
    #[test]
    fn a_tile_comes_back_rounded() {
        let e: Vec<f32> = (0..65536).map(|i| -12.0 + i as f32 * 0.0137).collect();
        for (q, worst) in [(NATIVE_M, 0.005 + 1.0 / 256.0), (COARSER_M, 0.05 + 1.0 / 256.0)] {
            let mut r = e.clone();
            let b = encode(&mut r, q).unwrap();
            assert_eq!(&b[8..16], b"WEBPVP8L");
            let back = roadcore::grid::decode_terrain(&b).unwrap();
            for ((a, s), o) in back.iter().zip(&r).zip(&e) {
                assert!((a - s).abs() <= 1.0 / 256.0, "{a} stored as {s}");
                assert!(((a - o).abs() as f64) <= worst + 1e-4, "{o} came back {a}");
            }
        }
    }

    /// Heights below 0 m come back as stored, to the trench's depth (the terrain keeps the source's
    /// at sea and below 0 m: the Dead Sea's −430 m, a polder's −6.5 m, the Mariana's −10,994 m),
    /// each still told from the voids decode marks (roadcore::grid::defined).
    #[test]
    fn heights_below_zero_come_back() {
        let lows = [-0.01f32, -6.5, -430.37, -1000.25, -4321.0, -10_994.0];
        let mut e: Vec<f32> = (0..65536).map(|i| lows[i % lows.len()]).collect();
        let b = encode(&mut e, NATIVE_M).unwrap();
        let back = roadcore::grid::decode_terrain(&b).unwrap();
        for (i, a) in back.iter().enumerate() {
            let want = lows[i % lows.len()];
            assert!((a - want).abs() <= 0.005 + 1.0 / 256.0, "{want} came back {a}");
            assert!(roadcore::grid::defined(*a), "{a}");
        }
    }
}
