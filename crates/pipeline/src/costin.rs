//! A target's input measures, for its done line in the job's costs file (`SCENIC_COSTS`;
//! crate::coord::Cost's `sizes`): what the step already counts as it reads, noted here as it goes
//! and taken as the target's cost is noted. No pass over data is made just to count.
//!
//! Keys (docs/formats.md, costs.jsonl): `bytes` (always: what the store's readers read for the
//! target, store::range::read_bytes, plus the inputs a step opens itself and notes with `add`),
//! `threads` (always), `cpu_ms` (the CPU time its process took over it, where it can be read), and the step's own: `ways`/`vertices` (z6 tile, pack, lo), `units` (pack, lo:
//! the base packs met), `nas_read` and `nas_secs` (pack, lo: the bytes the job read of the base packs
//! from the NAS and the seconds that took, the same on each of its targets), `buildings` (bldprep, bldtiles), `tiffs` (the GeoTIFFs opened) and `km2`
//! (the target's area: terrain, slope, trees and their assemblies, the z6 tiles' elevations), and,
//! where the job wrote to the NAS through its `Out` (crate::out), `wrote` (the bytes of the files
//! it made there) and `creates` (how many), and its NAS metadata work (`meta`: `listed`, `stats`,
//! `deletes`, task #451): with `bytes`, the target's NAS weight
//! (crate::needs::Book::nas, crate::budget::nas).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

struct State {
    /// The store's read count and the GeoTIFFs opened as the target began.
    read_at: u64,
    tiffs_at: u64,
    /// The process's CPU time as the target began (µs: crate::timings::cpu_us).
    cpu_at: Option<u64>,
    m: BTreeMap<String, u64>,
}

static STATE: Mutex<State> = Mutex::new(State { read_at: 0, tiffs_at: 0, cpu_at: None, m: BTreeMap::new() });

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
    s.cpu_at = crate::timings::cpu_us();
    s.m.clear();
}

/// Adds `n` to measure `key`.
pub fn add(key: &str, n: u64) {
    *state().m.entry(key.to_string()).or_default() += n;
}

/// Counts a file of `bytes` made on the NAS (crate::out's `put_file`, `store_bytes`): `wrote`
/// and `creates`. One already there (reused, touched) isn't counted.
pub fn wrote(bytes: u64) {
    let mut s = state();
    *s.m.entry("wrote".into()).or_default() += bytes;
    *s.m.entry("creates".into()).or_default() += 1;
}

/// Counts NAS metadata work (task #451): `listed` folder entries listed, `stats` files statted,
/// `deletes` files deleted, for its weight beside its bytes and creates (crate::budget::nas). A
/// walk that only lists, stats and removes (GC's) reads and writes next to nothing.
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
    if let (Some(a), Some(b)) = (s.cpu_at, crate::timings::cpu_us()) {
        m.insert("cpu_ms".into(), b.saturating_sub(a) / 1000);
    }
    m
}

/// What `measured` took: the bytes read, the GeoTIFFs opened and the CPU time (ms; None where
/// it can't be read).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Taken {
    pub bytes: u64,
    pub tiffs: u64,
    pub cpu_ms: Option<u64>,
}

/// Runs `f` (a z6 tile's tail steps run here after its measures were taken, before the next z6 tile
/// begins) and gives back what it read, the GeoTIFFs it opened and the CPU it took: its programs'
/// own (`child_reads`; their CPU as each is waited for) among them. (Reads and CPU on other
/// threads meanwhile are counted too.)
pub fn measured<T>(f: impl FnOnce() -> T) -> (T, Taken) {
    let (r0, t0, c0) = (store::range::read_bytes(), TIFFS.load(Ordering::Relaxed), crate::timings::cpu_us());
    let v = f();
    let cpu_ms = match (c0, crate::timings::cpu_us()) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a) / 1000),
        _ => None,
    };
    let t = Taken {
        bytes: store::range::read_bytes().saturating_sub(r0),
        tiffs: TIFFS.load(Ordering::Relaxed).saturating_sub(t0),
        cpu_ms,
    };
    (v, t)
}

/// Where a step's program notes, as it exits, what it read and the GeoTIFFs it opened (`<bytes>
/// <tiffs>` lines appended: `report_on_exit`), for the process that ran it (`child_reads`).
pub const READS_ENV: &str = "SCENIC_READS";

/// In a step's program (a z6 tile's: extract, areaflags, landcover, elev, tile, scenic-metrics): when
/// `READS_ENV` names a file, what this process read and opened is appended to it as it exits.
pub fn report_on_exit() {
    if std::env::var_os(READS_ENV).is_some() {
        // SAFETY: registers a plain function with no arguments, run once at exit.
        unsafe { libc::atexit(write_reads) };
    }
}

extern "C" fn write_reads() {
    if let Some(p) = std::env::var_os(READS_ENV) {
        append_reads(Path::new(&p), store::range::read_bytes(), TIFFS.load(Ordering::Relaxed));
    }
}

fn append_reads(p: &Path, bytes: u64, tiffs: u64) {
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{bytes} {tiffs}\n").as_bytes()));
    if let Err(e) = r {
        eprintln!("{}: noting what was read: {e}", p.display());
    }
}

/// What a program run with `READS_ENV` = `p` read and opened, counted as this process's own (so
/// the target under way, or `measured`, has it); the file removed.
pub fn child_reads(p: &Path) {
    let Ok(s) = std::fs::read_to_string(p) else { return };
    for l in s.lines() {
        let mut it = l.split(' ').map(|v| v.parse::<u64>().unwrap_or(0));
        store::range::count_read(it.next().unwrap_or(0));
        TIFFS.fetch_add(it.next().unwrap_or(0), Ordering::Relaxed);
    }
    std::fs::remove_file(p).ok();
}

/// Adds measures taken by `measured` to a target's: its CPU too, so a z6 tile's `cpu_ms` counts its
/// tail's programs run after its measures were taken (crate::needs::Book::cores reads it over the
/// target's whole time, the tail's included).
pub fn add_to(m: &mut BTreeMap<String, u64>, t: Taken) {
    *m.entry("bytes".into()).or_default() += t.bytes;
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
        store::range::count_read(100);
        tiff_opened();
        let m = take();
        assert_eq!(m["ways"], 7);
        assert_eq!(m["km2"], 9);
        assert!(m["tiffs"] >= 1);
        assert!(m["bytes"] >= 100);
        assert!(m["threads"] >= 1);
        assert!(!m.contains_key("wrote"));
        begin();
        wrote(10);
        wrote(5);
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

    /// A program's reads, noted as it exits, are counted where its runner `measured` them.
    #[test]
    fn child_reads_counted_by_their_runner() {
        let _l = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("reads");
        append_reads(&p, 1000, 3);
        append_reads(&p, 24, 1);
        let ((), t) = measured(|| child_reads(&p));
        assert!(t.bytes >= 1024 && t.tiffs >= 4);
        assert!(!p.exists());
        let mut m = BTreeMap::from([("km2".to_string(), 5)]);
        add_to(&mut m, Taken { bytes: 7, tiffs: 2, cpu_ms: None });
        assert_eq!((m["bytes"], m["tiffs"]), (7, 2));
        assert!(!m.contains_key("cpu_ms"));
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
