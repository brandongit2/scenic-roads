//! A target's input measures, for its done line in the job's costs file (`SCENIC_COSTS`;
//! crate::coord::Cost's `sizes`): what the step already counts as it reads, noted here as it goes
//! and taken as the target's cost is noted. No pass over data is made just to count.
//!
//! Keys (docs/formats.md, costs.jsonl): `bytes` (always: what the store's readers read for the
//! target, store::nasio's `ReadBytes` and `Local`, its programs' added as each ends, plus the inputs a step opens itself and notes with `add`),
//! `local` (of `bytes`, those read from this Mac's own disk, store::nasio's `Local`: a whole
//! local copy's reads, task #551; not the NAS's, so not in its NAS weight),
//! `threads` (always), `cpu_ms` (the CPU time its process took over it, where it can be read), and the step's own: `ways`/`vertices` (z6 tile, pack, lo), `units` (pack, lo:
//! the base packs met), `nas_read` and `nas_secs` (pack, lo: the bytes the job read of the base packs
//! from the NAS and the seconds that took, the same on each of its targets), `buildings` (bldprep, bldtiles), `tiffs` (the GeoTIFFs opened) and `km2`
//! (the target's area: terrain, slope, trees and their assemblies, the z6 tiles' elevations), and,
//! where the job wrote to the NAS through its `Out` (crate::out), `wrote` (the bytes of the files
//! it made there) and `creates` (how many), and its NAS metadata work (`meta`: `listed`, `stats`,
//! `deletes`, task #451): with `bytes`, the target's NAS weight
//! (crate::needs::Book::nas, crate::budget::nas).

use std::collections::BTreeMap;
use std::sync::Mutex;
use store::nasio::Count;

struct State {
    /// This process's counts (store::nasio) as the target began.
    at: store::nasio::Counts,
    /// The process's CPU time as the target began (µs: crate::timings::cpu_us).
    cpu_at: Option<u64>,
    m: BTreeMap<String, u64>,
}

static STATE: Mutex<State> = Mutex::new(State { at: [0; store::nasio::SLOTS], cpu_at: None, m: BTreeMap::new() });

/// Counts a GeoTIFF opened (crate::geotiff::Tiff::open). One a job's open-file cache keeps from an
/// earlier target isn't counted again.
pub fn tiff_opened() {
    store::nasio::add(Count::Tiffs, 1);
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the counts went up by from `at` to `now`: the bytes read (the NAS's and the local disk's),
/// of them the local disk's, and the GeoTIFFs opened.
fn grew(at: &store::nasio::Counts, now: &store::nasio::Counts) -> (u64, u64, u64) {
    let d = |c: Count| now[c as usize].saturating_sub(at[c as usize]);
    (d(Count::ReadBytes) + d(Count::Local), d(Count::Local), d(Count::Tiffs))
}

/// A target begins: its measures start again.
pub fn begin() {
    let mut s = state();
    s.at = store::nasio::own();
    s.cpu_at = crate::timings::cpu_us();
    s.m.clear();
}

/// Adds `n` to measure `key`.
pub fn add(key: &str, n: u64) {
    *state().m.entry(key.to_string()).or_default() += n;
}

/// Counts file `path` of `bytes` made on the NAS in `ms` (crate::out's `put_file`, `store_bytes`): `wrote`
/// and `creates`, and this process's NAS count (store::nasio). One already there (reused, touched)
/// isn't counted.
pub fn wrote(path: &store::nasfs::NasPath, bytes: u64, ms: u64) {
    path.count_wrote(bytes, ms);
    let mut s = state();
    *s.m.entry("wrote".into()).or_default() += bytes;
    *s.m.entry("creates".into()).or_default() += 1;
}

/// Counts NAS metadata work (task #451): `listed` folder entries listed, `stats` files statted,
/// `deletes` files deleted, for its weight beside its bytes and creates (crate::budget::nas). (This
/// process's NAS counts, store::nasio's, are store::nasfs's: each listing, stat and remove counts
/// there.) A walk that only lists, stats and removes (GC's)
/// reads and writes next to nothing.
pub fn meta(listed: u64, stats: u64, deletes: u64) {
    let mut s = state();
    for (k, n) in [("listed", listed), ("stats", stats), ("deletes", deletes)] {
        if n > 0 {
            *s.m.entry(k.into()).or_default() += n;
        }
    }
}

/// Sets measure `key` to `n`.
pub fn set(key: &str, n: u64) {
    state().m.insert(key.to_string(), n);
}

/// The target's measures (since `begin`), with `bytes` (the store's reads added), `local` where any
/// of them were the local disk's, `threads`, and `tiffs` where any were opened or the step noted an
/// area.
pub fn take() -> BTreeMap<String, u64> {
    let mut s = state();
    let (read, local, tiffs) = grew(&s.at, &store::nasio::own());
    let mut m = std::mem::take(&mut s.m);
    *m.entry("bytes".into()).or_default() += read;
    if local > 0 {
        *m.entry("local".into()).or_default() += local;
    }
    if tiffs > 0 || m.contains_key("km2") {
        m.insert("tiffs".into(), tiffs);
    }
    m.insert("threads".into(), threads() as u64);
    if let (Some(a), Some(b)) = (s.cpu_at, crate::timings::cpu_us()) {
        m.insert("cpu_ms".into(), b.saturating_sub(a) / 1000);
    }
    m
}

/// What `measured` took: the bytes read (and of them, the local disk's), the GeoTIFFs opened and
/// the CPU time (ms; None where it can't be read).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Taken {
    pub bytes: u64,
    pub local: u64,
    pub tiffs: u64,
    pub cpu_ms: Option<u64>,
}

/// Runs `f` (a z6 tile's tail steps run here after its measures were taken, before the next z6 tile
/// begins) and gives back what it read, the GeoTIFFs it opened and the CPU it took: its programs'
/// own (their counts added to this process's as each ends: crate::unit's `run`; their CPU as each
/// is waited for) among them. (Reads and CPU on other threads meanwhile are counted too.)
pub fn measured<T>(f: impl FnOnce() -> T) -> (T, Taken) {
    let (at, c0) = (store::nasio::own(), crate::timings::cpu_us());
    let v = f();
    let cpu_ms = match (c0, crate::timings::cpu_us()) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a) / 1000),
        _ => None,
    };
    let (bytes, local, tiffs) = grew(&at, &store::nasio::own());
    (v, Taken { bytes, local, tiffs, cpu_ms })
}

/// Adds measures taken by `measured` to a target's: its CPU too, so a z6 tile's `cpu_ms` counts its
/// tail's programs run after its measures were taken (crate::needs::Book::cores reads it over the
/// target's whole time, the tail's included).
pub fn add_to(m: &mut BTreeMap<String, u64>, t: Taken) {
    *m.entry("bytes".into()).or_default() += t.bytes;
    if t.local > 0 {
        *m.entry("local".into()).or_default() += t.local;
    }
    if t.tiffs > 0 || m.contains_key("km2") {
        *m.entry("tiffs".into()).or_default() += t.tiffs;
    }
    if let Some(ms) = t.cpu_ms {
        *m.entry("cpu_ms".into()).or_default() += ms;
    }
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

/// The tests that read the measures, one at a time (they share this process's).
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_since_begin() {
        let _l = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin();
        add("ways", 3);
        add("ways", 4);
        set("km2", 9);
        store::nasio::read(100);
        tiff_opened();
        let m = take();
        assert_eq!(m["ways"], 7);
        assert_eq!(m["km2"], 9);
        assert!(m["tiffs"] >= 1);
        assert!(m["bytes"] >= 100);
        assert!(m["threads"] >= 1);
        assert!(!m.contains_key("wrote"));
        begin();
        wrote(&store::nasfs::NasPath::local_for_test(std::path::Path::new("a")), 10, 0);
        wrote(&store::nasfs::NasPath::local_for_test(std::path::Path::new("b")), 5, 0);
        let m = take();
        assert_eq!((m["wrote"], m["creates"]), (15, 2));
        begin();
        meta(10, 3, 0);
        meta(5, 0, 2);
        let m = take();
        // (At least: other tests' walks may count meanwhile, this process's measures shared.)
        assert!(m["listed"] >= 15 && m["stats"] >= 3 && m["deletes"] >= 2, "{m:?}");
        begin();
        assert!(!take().contains_key("ways"));
    }

    /// A program's counts, added to its runner's as it ends, are in what the runner `measured`.
    #[test]
    fn a_programs_counts_in_its_runners_measure() {
        let _l = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut c = [0u64; store::nasio::SLOTS];
        (c[Count::ReadBytes as usize], c[Count::Local as usize], c[Count::Tiffs as usize]) = (1000, 24, 4);
        let ((), t) = measured(|| store::nasio::add_counts(&c));
        assert!(t.bytes >= 1024 && t.local >= 24 && t.tiffs >= 4);
        let mut m = BTreeMap::from([("km2".to_string(), 5)]);
        add_to(&mut m, Taken { bytes: 7, local: 0, tiffs: 2, cpu_ms: None });
        assert_eq!((m["bytes"], m["tiffs"]), (7, 2));
        assert!(!m.contains_key("cpu_ms"));
    }

    /// The cost lines' numbers as before the counts moved to store::nasio: `bytes` the NAS's reads
    /// and the local disk's together, `local` the latter, `wrote`/`creates` once a file.
    #[test]
    fn cost_lines_unchanged() {
        let _l = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin();
        store::nasio::read(300);
        store::nasio::add(Count::Local, 200);
        wrote(&store::nasfs::NasPath::local_for_test(std::path::Path::new("a")), 50, 0);
        let m = take();
        assert!(m["bytes"] >= 500 && m["local"] >= 200, "{m:?}");
        assert_eq!((m["wrote"], m["creates"]), (50, 1));
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
