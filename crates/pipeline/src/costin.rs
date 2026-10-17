//! A target's input measures, for its done line in the job's costs file (`SCENIC_COSTS`;
//! crate::coord::Cost's `sizes`): what the step already counts as it reads, noted here as it goes
//! and taken as the target's cost is noted. No pass over data is made just to count.
//!
//! Keys (docs/formats.md, costs.jsonl): `bytes` (always: what the store's readers read for the
//! target, store::range::read_bytes, plus the inputs a step opens itself and notes with `add`),
//! `threads` (always), and the step's own: `ways`/`vertices` (unit, pack, lo), `units` (pack, lo:
//! the base packs met), `buildings` (bldprep, bldtiles), `tiffs` (the GeoTIFFs opened) and `km2`
//! (the target's area: terrain, slope, trees and their assemblies, the units' elevations).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

struct State {
    /// The store's read count and the GeoTIFFs opened as the target began.
    read_at: u64,
    tiffs_at: u64,
    m: BTreeMap<String, u64>,
}

static STATE: Mutex<State> = Mutex::new(State { read_at: 0, tiffs_at: 0, m: BTreeMap::new() });

/// The GeoTIFFs this process has opened (crate::geotiff::Tiff::open).
static TIFFS: AtomicU64 = AtomicU64::new(0);

/// Counts a GeoTIFF opened (crate::geotiff::Tiff::open). One a job's open-file cache keeps from an
/// earlier target isn't counted again.
pub fn tiff_opened() {
    TIFFS.fetch_add(1, Ordering::Relaxed);
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A target begins: its measures start again.
pub fn begin() {
    let mut s = state();
    s.read_at = store::range::read_bytes();
    s.tiffs_at = TIFFS.load(Ordering::Relaxed);
    s.m.clear();
}

/// Adds `n` to measure `key`.
pub fn add(key: &str, n: u64) {
    *state().m.entry(key.to_string()).or_default() += n;
}

/// Sets measure `key` to `n`.
pub fn set(key: &str, n: u64) {
    state().m.insert(key.to_string(), n);
}

/// The target's measures (since `begin`), with `bytes` (the store's reads added), `threads`, and
/// `tiffs` where any were opened or the step noted an area.
pub fn take() -> BTreeMap<String, u64> {
    let mut s = state();
    let read = store::range::read_bytes().saturating_sub(s.read_at);
    let tiffs = TIFFS.load(Ordering::Relaxed).saturating_sub(s.tiffs_at);
    let mut m = std::mem::take(&mut s.m);
    *m.entry("bytes".into()).or_default() += read;
    if tiffs > 0 || m.contains_key("km2") {
        m.insert("tiffs".into(), tiffs);
    }
    m.insert("threads".into(), threads() as u64);
    m
}

/// The threads a step's job may use: as the agent sets them (`RAYON_NUM_THREADS`), else the pool's.
pub fn threads() -> usize {
    std::env::var("RAYON_NUM_THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or_else(rayon::current_num_threads)
}

/// A tile's area in km² (a sphere of the Earth's mean radius, 6371 km).
pub fn tile_km2(z: u8, _x: u32, y: u32) -> u64 {
    let n = (1u64 << z) as f64;
    let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan();
    let r = 6371.0_f64;
    (r * r * (2.0 * std::f64::consts::PI / n) * (lat(y as f64).sin() - lat(y as f64 + 1.0).sin())).abs().round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_since_begin() {
        begin();
        add("ways", 3);
        add("ways", 4);
        set("km2", 9);
        store::range::count_read(100);
        tiff_opened();
        let m = take();
        assert_eq!(m["ways"], 7);
        assert_eq!(m["km2"], 9);
        assert!(m["tiffs"] >= 1);
        assert!(m["bytes"] >= 100);
        assert!(m["threads"] >= 1);
        begin();
        assert!(!take().contains_key("ways"));
    }

    #[test]
    fn tile_areas() {
        // The whole Earth at z0 (to ±85.05°): about 5.08e8 km² of 5.10e8.
        let all = tile_km2(0, 0, 0);
        assert!((505_000_000..=510_000_000).contains(&all), "{all}");
        // A z6 tile at the equator: about 626 km a side.
        let eq = tile_km2(6, 32, 32);
        assert!((380_000..=400_000).contains(&eq), "{eq}");
    }
}
