//! Scenic analysis of the road network.
//!
//! usage: scenic-metrics <build_dir> prep|canopy|view|buildings [dir]|flags|all  (flags: cheap rerun after POI/heritage/area changes)
//!
//! prep    road sample points (~100 m apart) with observer eye heights; per-vertex drape
//!         heights on the Terrarium surface (what MapLibre renders in 3D)
//! canopy  Meta/WRI 1.2 m canopy heights, streamed per z9 tile:
//!           · z11 grid layers: canopy height (p75 of 5 m max-pooled cells) and cover
//!           · near-field horizons: 32 rays per sample through terrain + trees to 300 m
//!           · roadside tree height and forest cover per sample
//!         Cached (pipeline::scache): grid tiles and samples done before are copied, and only
//!         10° canopy tiles with something new are read.
//! view    far-field viewshed and landscape metrics → per-sample and per-vertex channels
//! buildings  roadside buildings per sample from Overture footprints (data/buildings, `dem/buildings.py`)
//! seed-cache  fill the canopy and view caches (data/cache/scenic) from this build's outputs, so
//!         the next run reuses them (for builds made before the caches existed)

use det::Det;
use anyhow::{bail, ensure, Context, Result};
use pipeline::count_bar;
use pipeline::scache::{self, GridChange};
use pipeline::terr::TerrainCache;
use pipeline::view::eye_height;
use rayon::prelude::*;
use roadcore::archive::Archive;
use roadcore::grid::{GridIndex, CELLS};
use roadcore::scenic::{sflag, Sample, EYE_M, NEAR_AZ, NEAR_MAX_M, RAIL_EYE_M, SAMPLE_SPACING_M};
use roadcore::{class, dist_m, flag, merc, Array, Ways, E7};
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI8, AtomicU8, Ordering::Relaxed};
use std::sync::LazyLock;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("data/build"));
    let step = args.get(2).map(String::as_str).unwrap_or("all");
    let t0 = std::time::Instant::now();
    match step {
        "prep" => prep(&dir)?,
        "canopy" => canopy(&dir)?,
        "view" => pipeline::view::run(&dir)?,
        "flags" => pipeline::view::flags(&dir)?,
        "buildings" => pipeline::buildings::run(&dir, &PathBuf::from(args.get(3).map(String::as_str).unwrap_or("data/buildings")))?,
        "seed-cache" => seed_cache(&dir)?,
        "all" => {
            prep(&dir)?;
            canopy(&dir)?;
            pipeline::view::run(&dir)?;
        }
        s => bail!("unknown step {s}"),
    }
    eprintln!("scenic {step}: done in {:.0?}", t0.elapsed());
    Ok(())
}

// ---- caches ------------------------------------------------------------------------------

fn seed_cache(dir: &Path) -> Result<()> {
    use roadcore::scenic::ch;
    let grid = GridIndex::load(dir)?;
    let samples_a = Array::<Sample>::open(&dir.join("samples.bin"))?;
    let samples = samples_a.get();
    let near_a = Array::<i8>::open(&dir.join("near.i8"))?;
    let near = near_a.get();
    let road_a = Array::<u8>::open(&dir.join("roadside.u8"))?;
    let roadside = road_a.get();
    let met_a = Array::<u8>::open(&dir.join("samples.metrics.u8"))?;
    let met = met_a.get();
    anyhow::ensure!(near.len() == samples.len() * NEAR_AZ && roadside.len() == samples.len() * 2 && met.len() == samples.len() * ch::NBASE, "build outputs out of step");
    let cdir = scache::dir(dir);
    let keys: Vec<u64> = samples.par_iter().map(scache::sample_key).collect();
    scache::Prev::save(&cdir, "canopy", &keys)?;
    GridChange::save(&cdir, "canopy", &grid.tiles)?;
    anyhow::ensure!(std::fs::metadata(dir.join("grid.canopy.u8"))?.len() as usize == grid.tiles.len() * CELLS, "grid layers out of step");
    let vkeys: Vec<u64> = samples
        .par_iter()
        .enumerate()
        .map(|(i, s)| scache::mix(scache::mix(scache::sample_key(s), bytemuck::cast_slice(&near[i * NEAR_AZ..(i + 1) * NEAR_AZ])), &roadside[i * 2..i * 2 + 2]))
        .collect();
    scache::Prev::save(&cdir, "view", &vkeys)?;
    GridChange::save(&cdir, "view", &grid.tiles)?;
    eprintln!("seed-cache: {} samples, {} grid tiles", samples.len(), grid.tiles.len());
    Ok(())
}

// ---- prep --------------------------------------------------------------------------------

fn prep(dir: &Path) -> Result<()> {
    let wv = Ways::open(dir)?;
    let ways = wv.ways();
    let verts = wv.verts();
    // A unit's folder (`SCENIC_OWN`: its tile, w,s,e,n E7): samples only for the ways it owns (its
    // output); the rest are there for context, and their results would be thrown away.
    let own: Option<[i32; 4]> = std::env::var("SCENIC_OWN").ok().and_then(|v| {
        let b: Vec<i32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
        (b.len() == 4).then(|| [b[0], b[1], b[2], b[3]])
    });
    let fin = roadcore::elev::Stored::open(dir)?;
    let fin = fin.get();
    let arc = Archive::open(&dir.join("terrain.tiles"))?;
    let pb = count_bar(ways.len() as u64, "samples + drape");
    let parts: Vec<(Vec<Sample>, Vec<i16>)> = ways
        .par_chunks(2048)
        .enumerate()
        .map(|(ci, chunk)| {
            let mut tc = TerrainCache::new(&arc);
            let mut samples = Vec::new();
            let mut drape = Vec::new();
            for (k, w) in chunk.iter().enumerate() {
                let wi = (ci * 2048 + k) as u32;
                let s = w.vstart as usize;
                let n = w.vcount as usize;
                let v = &verts[s..s + n];
                let tunnel = w.flags & flag::TUNNEL != 0;
                let bridge = w.flags & flag::BRIDGE != 0 && !tunnel;
                // Drape heights: the rendered terrain surface; tunnels keep their true (buried)
                // level. Bridges store the ground beneath: the renderer lifts the deck to
                // max(ground, elevation), and the difference is the viaduct's height.
                for (j, p) in v.iter().enumerate() {
                    let (mx, my) = merc(p[0] as f64 * E7, p[1] as f64 * E7);
                    let t = tc.at(mx, my);
                    let e = fin.m(s + j);
                    let h = if tunnel { e } else { t };
                    drape.push(h.round().clamp(-500.0, 9000.0) as i16);
                }
                if w.class == class::FERRY || own.is_some_and(|tb| !pipeline::unit::owns(tb, v[0])) {
                    pb.inc(1);
                    continue;
                }
                let mut cd = Vec::with_capacity(n);
                let mut acc = 0.0;
                cd.push(0.0);
                for j in 1..n {
                    acc += dist_m(v[j - 1][0] as f64 * E7, v[j - 1][1] as f64 * E7, v[j][0] as f64 * E7, v[j][1] as f64 * E7);
                    cd.push(acc);
                }
                let m = ((acc / SAMPLE_SPACING_M).round() as usize).max(1);
                for q in 0..m {
                    let d = (q as f64 + 0.5) * acc / m as f64;
                    let j = cd.partition_point(|&x| x <= d).clamp(1, n - 1);
                    let t = if cd[j] > cd[j - 1] { (d - cd[j - 1]) / (cd[j] - cd[j - 1]) } else { 0.0 };
                    let lon = v[j - 1][0] as f64 + (v[j][0] - v[j - 1][0]) as f64 * t;
                    let lat = v[j - 1][1] as f64 + (v[j][1] - v[j - 1][1]) as f64 * t;
                    let (e0, e1) = (fin.dm(s + j - 1), fin.dm(s + j));
                    let e = (e0 as f64 + (e1 - e0) as f64 * t) as f32 / 10.0;
                    let (mx, my) = merc(lon * E7, lat * E7);
                    let terrain = tc.at(mx, my);
                    let ground = if tunnel { e } else { terrain.max(e) };
                    samples.push(Sample {
                        way: wi,
                        dist: d as f32,
                        lon: lon.round() as i32,
                        lat: lat.round() as i32,
                        // Rail: a carriage window is higher than a car's.
                        eye: ground + if class::is_rail(w.class) { RAIL_EYE_M } else { EYE_M },
                        flags: if tunnel { sflag::TUNNEL } else if bridge { sflag::BRIDGE } else { 0 },
                        _pad: [0; 3],
                    });
                }
                pb.inc(1);
            }
            (samples, drape)
        })
        .collect();
    pb.finish_and_clear();
    let mut samples = Vec::new();
    let mut drape = Vec::with_capacity(verts.len());
    for (s, d) in parts {
        samples.extend(s);
        drape.extend(d);
    }
    std::fs::write(roadcore::tmp(dir, "samples.bin"), bytemuck::cast_slice(&samples))?;
    std::fs::write(roadcore::tmp(dir, "vterrain.i16"), bytemuck::cast_slice(&drape))?;
    roadcore::commit(dir, &["samples.bin", "vterrain.i16"])?;
    eprintln!("prep: {} samples, {} drape heights", samples.len(), drape.len());
    Ok(())
}

// ---- canopy ------------------------------------------------------------------------------
//
// Meta/WRI canopy height statistics on a 0.00025° grid (≈28 m × 20 m at 45° N), aggregated
// from their 1.2 m canopy height map: per cell the median and 95th-percentile tree height
// and the share of 1 m pixels taller than 5 m. (The 1.2 m tiles themselves total ~550 GB
// for this region.) Median height is used for occlusion, so scattered trees in a field
// don't wall off a view; p95 gives roadside tree height.

const CHM10_URL: &str = "https://dataforgood-fb-data.s3.amazonaws.com/forests/v1/alsgedi_global_v6_float_epsg4326_v3_10deg";
const C10: usize = 40_000;
const C10_RES: f64 = 0.00025;
/// A square's rows read at a time (`SCENIC_CANOPY_BAND` sets another count; any gives the same
/// bytes). A dense unit's band window, three layers, is ~85 MB (1,024 rows × ~27,000 columns);
/// reading all the rows its work reaches at the square's full width took 2.2 GB, and the files
/// themselves 1.1 GB more.
const BAND_ROWS: usize = 1024;

/// Rows `r0..r1` × columns `c0..c1` of a 10° square (nothing when either is empty).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Window {
    r0: usize,
    r1: usize,
    c0: usize,
    c1: usize,
}

impl Window {
    const NONE: Window = Window { r0: 0, r1: 0, c0: 0, c1: 0 };

    /// The pixels of rows `r` and columns `c` (each lo..=hi, floored: maybe outside the square, or
    /// not finite) inside rows `rows` and the square's columns.
    fn clip(r: (f64, f64), c: (f64, f64), rows: (usize, usize)) -> Window {
        // (A bound that's NaN is no bound: max and min give the other side.)
        let span = |(lo, hi): (f64, f64), a: usize, b: usize| {
            let (s, e) = (lo.max(a as f64), (hi + 1.0).min(b as f64));
            if s < e {
                (s as usize, e as usize)
            } else {
                (0, 0)
            }
        };
        let ((r0, r1), (c0, c1)) = (span(r, rows.0, rows.1), span(c, 0, C10));
        Window { r0, r1, c0, c1 }
    }

    fn is_empty(&self) -> bool {
        self.r0 >= self.r1 || self.c0 >= self.c1
    }

    /// The smallest window holding both.
    fn union(self, o: Window) -> Window {
        match (self.is_empty(), o.is_empty()) {
            (_, true) => self,
            (true, false) => o,
            _ => Window { r0: self.r0.min(o.r0), r1: self.r1.max(o.r1), c0: self.c0.min(o.c0), c1: self.c1.max(o.c1) },
        }
    }
}

/// A band's part of a 10° square: window `win` of its three layers. A lookup finds nothing outside
/// the square or outside rows `row0..row0 + rows`, those the unit's work there reaches (as when all
/// of them were read at once); any other lookup the band's work makes is inside its window (Bands).
struct Chm10 {
    left: f64,
    top: f64,
    row0: usize,
    rows: usize,
    win: Window,
    median: Vec<u8>, // metres
    p95: Vec<u8>,    // metres
    cover: Vec<u8>,  // share > 5 m, ×255
}

impl Chm10 {
    /// A point's index in the band's layers.
    #[inline]
    fn idx(&self, lon: f64, lat: f64) -> Option<usize> {
        Some(self.row(lat)? * (self.win.c1 - self.win.c0) + self.col(lon)?)
    }

    /// A longitude's column in the band's window, for one in the square (the window holds every
    /// column the band's work looks up: Bands).
    #[inline]
    fn col(&self, lon: f64) -> Option<usize> {
        let c = ((lon - self.left) / C10_RES).floor();
        (c >= 0.0 && c < C10 as f64).then(|| {
            let (w, c) = (&self.win, c as usize);
            assert!(c >= w.c0 && c < w.c1, "canopy: a lookup outside its band's window");
            c - w.c0
        })
    }

    /// A latitude's row in the band's window, for one among those read (from `row0`).
    #[inline]
    fn row(&self, lat: f64) -> Option<usize> {
        let r = ((self.top - lat) / C10_RES).floor();
        let (r0, r1) = (self.row0 as f64, (self.row0 + self.rows) as f64);
        (r >= r0 && r < r1).then(|| {
            let (w, r) = (&self.win, r as usize);
            assert!(r >= w.r0 && r < w.r1, "canopy: a lookup outside its band's window");
            r - w.r0
        })
    }
}

/// A canopy file, opened: the local cache's copy (`path`), else the NAS's (`store`, copied here),
/// else downloaded once, into the NAS's store first; None for an empty file, which marks one Meta
/// doesn't have. Each copy is written whole and checked whole when opened (pipeline::whole: its
/// directories, not its data): one that isn't (cut short) is deleted and taken from the next source.
/// The bands then read only the strips they need from it (Tiff), never the whole file.
fn fetch_file(agent: &ureq::Agent, url: &str, path: &Path, store: Option<&Path>) -> Result<Option<File>> {
    // A kept copy: Some(None) for Meta's "none there", None when it's missing or not whole.
    let kept = |p: &Path| -> Option<Option<File>> {
        let f = File::open(p).ok()?;
        let len = f.metadata().ok()?.len();
        if len == 0 {
            return Some(None);
        }
        if pipeline::whole::tiff_whole(&f) {
            return Some(Some(f));
        }
        eprintln!("canopy: {} isn't whole ({len} bytes): taken again", p.display());
        std::fs::remove_file(p).ok();
        None
    };
    if let Some(f) = kept(path) {
        // Used now: the build agent's room-making deletes the least recently used squares first.
        if let Ok(f) = File::options().append(true).open(path) {
            f.set_modified(std::time::SystemTime::now()).ok();
        }
        return Ok(f);
    }
    // The NAS's copy; else the right to download it there (`<file>.lock`, made with create-new: a
    // unit on one Mac and a trees job on the other may want the same square at once), or the copy
    // the holder downloads, waited for. A lock not touched for 30 minutes is a holder that died.
    let mut _lock = None;
    if let Some(st) = store {
        if let Some(d) = st.parent() {
            std::fs::create_dir_all(d)?;
        }
        let lock = st.with_file_name(format!("{}.lock", st.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        loop {
            match kept(st) {
                Some(None) => {
                    pipeline::whole::write(path, b"")?;
                    return Ok(None);
                }
                Some(Some(_)) => {
                    pipeline::whole::copy(st, path)?;
                    return kept(path).with_context(|| format!("{} isn't whole as copied", path.display()));
                }
                None => {}
            }
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(mut f) => {
                    use std::io::Write;
                    f.write_all(format!("{} {}", pipeline::agent::cond::host(), std::process::id()).as_bytes()).ok();
                    _lock = Some(DownloadLock(lock));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(&lock).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
                    if age.is_some_and(|a| a > std::time::Duration::from_secs(1800)) {
                        eprintln!("canopy: taking over {} (not touched for {:.0?})", lock.display(), age.unwrap_or_default());
                        std::fs::remove_file(&lock).ok();
                    } else {
                        std::thread::sleep(std::time::Duration::from_secs(20));
                    }
                }
                Err(e) => return Err(e).with_context(|| format!("lock {}", lock.display())),
            }
        }
    }
    let mut missing = 0;
    for attempt in 0..6 {
        match agent.get(url).call() {
            Ok(mut r) => {
                let want: Option<u64> = r.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
                // Into a temporary file beside `path`, not memory (up to 1.2 GB), flushed. (A body cut
                // short, or not a whole TIFF, is tried again, never kept.)
                let tmp = pipeline::whole::tmp_name(path);
                let got = (|| -> Result<Option<File>> {
                    let mut f = File::options().read(true).write(true).create(true).truncate(true).open(&tmp)?;
                    let Some(n) = copy_body(&mut r.body_mut().with_config().limit(3_000_000_000).reader(), &mut f)? else { return Ok(None) };
                    f.sync_all()?;
                    if want.is_some_and(|w| w != n) || !pipeline::whole::tiff_whole(&f) {
                        return Ok(None);
                    }
                    if let Some(st) = store {
                        pipeline::whole::copy(&tmp, st)?;
                    }
                    std::fs::rename(&tmp, path)?;
                    Ok(Some(f))
                })();
                if !matches!(got, Ok(Some(_))) {
                    std::fs::remove_file(&tmp).ok();
                }
                if let Some(f) = got.with_context(|| format!("write {}", path.display()))? {
                    return Ok(Some(f));
                }
            }
            // Meta has none there (404, or S3's 403): so it says twice, a moment apart, before
            // it's remembered for good.
            Err(ureq::Error::StatusCode(404 | 403)) => {
                missing += 1;
                if missing == 2 {
                    if let Some(st) = store {
                        pipeline::whole::write(st, b"")?;
                    }
                    pipeline::whole::write(path, b"")?;
                    return Ok(None);
                }
            }
            Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(if missing > 0 { 5000 } else { 1000 << attempt }));
    }
    bail!("download failed: {url}")
}

/// The right to download one canopy file into the NAS's store (fetch_file), given up when dropped.
struct DownloadLock(PathBuf);

impl Drop for DownloadLock {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

/// Copies `r` to `w`: the bytes copied once `r` ends, None when reading fails (a body cut short);
/// failing to write is an error.
fn copy_body(r: &mut impl std::io::Read, w: &mut impl std::io::Write) -> Result<Option<u64>> {
    let mut b = vec![0u8; 1 << 20];
    let mut n = 0;
    loop {
        match r.read(&mut b) {
            Ok(0) => return Ok(Some(n)),
            Ok(k) => {
                w.write_all(&b[..k])?;
                n += k as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Ok(None),
        }
    }
}

/// Minimal little-endian TIFF strip index.
struct Strips {
    width: usize,
    height: usize,
    rows_per_strip: usize,
    compression: u64,
    strips: Vec<(u64, usize)>,
}

/// A TIFF's strip index, read through `at(offset, length)` (each tag's values at once).
fn parse_tiff(at: &dyn Fn(u64, usize) -> Result<Vec<u8>>) -> Result<Strips> {
    // (Every read checked: a file cut short is an error, not a panic.)
    let le = |b: &[u8]| b.iter().rev().fold(0u64, |v, &x| v << 8 | x as u64);
    let h = at(0, 8)?;
    if &h[..2] != b"II" {
        bail!("not little-endian TIFF");
    }
    let big = match le(&h[2..4]) {
        42 => false,
        43 => true,
        v => bail!("TIFF version {v}"),
    };
    let ifd = if big { le(&at(8, 8)?) } else { le(&h[4..8]) };
    // Entries: tag, type, count, then the values or their offset (`inline` bytes).
    let (csz, esz, inline) = if big { (8, 20, 8) } else { (2, 12, 4) };
    let count = le(&at(ifd, csz)?) as usize;
    let ents = at(ifd.saturating_add(csz as u64), count.checked_mul(esz).context("TIFF cut short")?)?;
    let mut tags: HashMap<u16, Vec<u64>> = HashMap::new();
    for e in ents.chunks_exact(esz) {
        let sz = match le(&e[2..4]) {
            3 => 2,
            4 => 4,
            16 => 8,
            _ => continue,
        };
        let len = (le(&e[4..esz - inline]) as usize).checked_mul(sz).context("TIFF cut short")?;
        let vals = if len <= inline { e[esz - inline..][..len].to_vec() } else { at(le(&e[esz - inline..]), len)? };
        tags.insert(le(&e[..2]) as u16, vals.chunks_exact(sz).map(le).collect());
    }
    let get = |t: u16| tags.get(&t).and_then(|v| v.first().copied()).context(format!("TIFF tag {t}"));
    let offs = tags.get(&273).context("StripOffsets")?;
    let lens = tags.get(&279).context("StripByteCounts")?;
    Ok(Strips {
        width: get(256)? as usize,
        height: get(257)? as usize,
        rows_per_strip: get(278).unwrap_or(1) as usize,
        compression: get(259)?,
        strips: offs.iter().zip(lens).map(|(&o, &l)| (o, l as usize)).collect(),
    })
}

/// A TIFF's strips, read from its file as they're wanted (positionally: never the whole file, up to
/// 1.2 GB, which in WebAssembly would all be memory).
struct Tiff {
    file: File,
    len: u64,
    st: Strips,
}

impl Tiff {
    fn open(file: File) -> Result<Tiff> {
        let len = file.metadata()?.len();
        let st = parse_tiff(&|o, n| read_at(&file, len, o, n))?;
        Ok(Tiff { file, len, st })
    }

    /// Window `w`, each value through `f` (decode_rows).
    fn read(&self, w: Window, f: impl Fn(u16) -> u8 + Sync) -> Result<Vec<u8>> {
        decode_rows(&|o, n| read_at(&self.file, self.len, o, n), &self.st, w.r0..w.r1, w.c0..w.c1, f)
    }
}

/// `n` bytes from `off` of file `f`, `len` bytes long (past its end, an error before anything's
/// allocated: a damaged directory's count can be anything).
fn read_at(f: &File, len: u64, off: u64, n: usize) -> Result<Vec<u8>> {
    use store::sys::PosIo;
    ensure!(off.checked_add(n as u64).is_some_and(|e| e <= len), "{n} bytes at {off} run past the file's end ({len})");
    let mut b = vec![0u8; n];
    f.read_exact_at(&mut b, off)?;
    Ok(b)
}

/// Rows `rows` × columns `cols` of an LZW TIFF of any width, its strips read through `read(offset,
/// length)`, each value through `f` (no data, 65535: 0). Each strip holding wanted rows is decoded
/// whole, its wanted part straight into place; a strip that doesn't decode whole is an error (the
/// file is damaged: canopy takes it again).
fn decode_rows(read: &(dyn Fn(u64, usize) -> Result<Vec<u8>> + Sync), st: &Strips, rows: Range<usize>, cols: Range<usize>, f: impl Fn(u16) -> u8 + Sync) -> Result<Vec<u8>> {
    let (w, rps, nc) = (st.width, st.rows_per_strip.max(1), cols.len());
    ensure!(cols.end <= w, "columns {cols:?} of {w}");
    let mut out = vec![0u8; nc * rows.len()];
    if out.is_empty() {
        return Ok(out);
    }
    // Each strip's wanted rows: (strip, its first wanted row, those rows of `out`).
    let mut parts: Vec<(usize, usize, &mut [u8])> = Vec::new();
    let mut rest: &mut [u8] = &mut out;
    let mut r = rows.start;
    while r < rows.end {
        let si = r / rps;
        let end = ((si + 1) * rps).min(rows.end);
        let (head, tail) = std::mem::take(&mut rest).split_at_mut((end - r) * nc);
        parts.push((si, r, head));
        rest = tail;
        r = end;
    }
    parts.into_par_iter().try_for_each(|(si, first, dst)| -> Result<()> {
        let &(off, len) = st.strips.get(si).with_context(|| format!("strip {si} isn't listed"))?;
        let src = read(off, len).with_context(|| format!("strip {si} runs past the file's end"))?;
        let mut dec = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let raw = dec.decode(&src).map_err(|e| anyhow::anyhow!("strip {si}: {e:?}"))?;
        // (Its wanted rows whole, all columns: a strip that decodes short is damaged.)
        let skip = (first - si * rps) * w * 2;
        let vals = raw.get(skip..skip + dst.len() / nc * w * 2).with_context(|| format!("strip {si} decodes short ({} bytes)", raw.len()))?;
        for (o, v) in dst.chunks_exact_mut(nc).zip(vals.chunks_exact(w * 2)) {
            for (o, v) in o.iter_mut().zip(v[cols.start * 2..cols.end * 2].as_chunks::<2>().0) {
                let x = u16::from_le_bytes(*v);
                *o = if x == 65535 { 0 } else { f(x) };
            }
        }
        Ok(())
    })?;
    Ok(out)
}

/// One layer of a square: its file (fetch_file, a 40000² uint16 LZW TIFF), from which the bands
/// read their windows. A file that doesn't decode is damaged: this Mac's copy is deleted and it's
/// taken again (from the NAS), then the NAS's copy too (from Meta).
struct Layer<'a> {
    agent: &'a ureq::Agent,
    url: String,
    path: PathBuf,
    store: Option<PathBuf>,
    f: fn(u16) -> u8,
    tiff: Tiff,
    taken: usize,
}

impl<'a> Layer<'a> {
    /// The layer's file, opened; None when Meta has none there.
    fn open(agent: &'a ureq::Agent, url: String, path: PathBuf, store: Option<PathBuf>, f: fn(u16) -> u8) -> Result<Option<Layer<'a>>> {
        let mut taken = 0;
        loop {
            let Some(file) = fetch_file(agent, &url, &path, store.as_deref())? else { return Ok(None) };
            match canopy_tiff(file) {
                Ok(tiff) => return Ok(Some(Layer { agent, url, path, store, f, tiff, taken })),
                Err(e) => damaged(&path, store.as_deref(), &mut taken, e)?,
            }
        }
    }

    /// Window `w` of the layer.
    fn read(&mut self, w: Window) -> Result<Vec<u8>> {
        let mut got = self.tiff.read(w, self.f);
        loop {
            let e = match got {
                Ok(v) => return Ok(v),
                Err(e) => e,
            };
            damaged(&self.path, self.store.as_deref(), &mut self.taken, e)?;
            // (Bands before this one have used the square: found missing now, it can't be passed over.)
            let file = fetch_file(self.agent, &self.url, &self.path, self.store.as_deref())?.with_context(|| format!("{}: Meta has none there now", self.path.display()))?;
            got = canopy_tiff(file).and_then(|t| {
                self.tiff = t;
                self.tiff.read(w, self.f)
            });
        }
    }
}

/// A canopy file's strip index: a 40000² uint16 LZW TIFF, else it's damaged.
fn canopy_tiff(file: File) -> Result<Tiff> {
    let t = Tiff::open(file)?;
    let s = &t.st;
    if s.width != C10 || s.height != C10 || s.compression != 5 {
        bail!("unexpected TIFF {}×{} compression {}", s.width, s.height, s.compression);
    }
    Ok(t)
}

/// After error `e` reading canopy file `path`: this Mac's copy is deleted so it's taken again, the
/// second time the NAS's too; the third time, the error.
fn damaged(path: &Path, store: Option<&Path>, taken: &mut usize, e: anyhow::Error) -> Result<()> {
    if *taken == 2 {
        return Err(e.context(format!("{} (taken again twice)", path.display())));
    }
    eprintln!("canopy: {}: {e:#}; taken again", path.display());
    std::fs::remove_file(path).ok();
    if *taken == 1 {
        if let Some(st) = store {
            std::fs::remove_file(st).ok();
        }
    }
    *taken += 1;
    Ok(())
}

/// A square's work in bands of rows: each sample to do within reach of the square (in the band
/// holding its row) and each cell row of a to-do grid tile (in the band holding the first row its
/// sub-samples read) is done in exactly one band, which reads the window holding every lookup that
/// work makes inside the square's rows `row0..row1`. So each lookup finds what it found when all
/// those rows were read at once, and no more than a band's window is ever held.
struct Bands {
    /// Each band's window.
    win: Vec<Window>,
    /// Each band's samples.
    samples: Vec<Vec<u32>>,
    /// The samples to do near the square none of whose lookups land in its rows read and its
    /// columns (just outside it, within the margin it's read with, beyond their reach): in no band,
    /// their lookups all find nothing here (a tunnel's horizon is still theirs to set).
    outside: Vec<u32>,
    /// Each grid tile's cell rows' bands (u16::MAX: none here).
    cells: Vec<[u16; 256]>,
}

/// A grid tile's cell rows' bands, and the window each of those bands reads for it.
type TileBands = ([u16; 256], Vec<(usize, Window)>);

impl Bands {
    /// Bands of `size` rows over rows `row0..row1` of the square at `top`, `left`, for the samples
    /// (within reach: `touches`) and grid tiles to do.
    #[allow(clippy::too_many_arguments)]
    fn plan(top: f64, left: f64, (row0, row1): (usize, usize), size: usize, samples: &[Sample], todo_s: &[bool], touches: impl Fn(f64, f64) -> bool, tiles: &[[u32; 2]], todo_t: &[bool]) -> Bands {
        let px = |v: f64| (v / C10_RES).floor();
        let n = (row1 - row0).div_ceil(size);
        let (mut win, mut by_band, mut outside) = (vec![Window::NONE; n], vec![Vec::new(); n], Vec::new());
        // A sample's lookups (near_field) are within NEAR_MAX_M of it: its window is the rows and
        // columns that spans, a pixel spare each way for rounding.
        for (si, (s, &t)) in samples.iter().zip(todo_s).enumerate() {
            let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
            if !t || !touches(lon, lat) {
                continue;
            }
            let (m_lon, m_lat) = m_per_deg(lat);
            let (dlon, dlat) = (NEAR_MAX_M / m_lon, NEAR_MAX_M / m_lat);
            let w = Window::clip((px(top - (lat + dlat)) - 1.0, px(top - (lat - dlat)) + 1.0), (px(lon - dlon - left) - 1.0, px(lon + dlon - left) + 1.0), (row0, row1));
            // (None of its lookups here: its band's window wouldn't hold them.)
            if w.is_empty() {
                outside.push(si as u32);
                continue;
            }
            let k = (px(top - lat).clamp(row0 as f64, (row1 - 1) as f64) as usize - row0) / size;
            by_band[k].push(si as u32);
            win[k] = win[k].union(w);
        }
        // A grid cell row's sub-samples read two rows (worked out as the grid loop does), across the
        // tile's columns.
        let lon_of = |gx: f64| gx / roadcore::grid::WORLD * 360.0 - 180.0;
        let cells: Vec<TileBands> = tiles
            .par_iter()
            .zip(todo_t)
            .map(|(g, &t)| {
                let (mut cells, mut wins) = ([u16::MAX; 256], Vec::<(usize, Window)>::new());
                if !t {
                    return (cells, wins);
                }
                let gx = g[0] as f64 * 256.0;
                let cols = (px(lon_of(gx + 0.25) - left), px(lon_of(gx + 255.0 + 0.75) - left));
                for (cy, b) in cells.iter_mut().enumerate() {
                    let gy = g[1] as f64 * 256.0 + cy as f64;
                    let row = |oy: f64| px(top - (std::f64::consts::PI * (1.0 - 2.0 * (gy + oy) / roadcore::grid::WORLD)).dsinh().datan().to_degrees());
                    let (ra, rb) = (row(0.25), row(0.75));
                    let w = Window::clip((ra.min(rb), ra.max(rb)), cols, (row0, row1));
                    if w.is_empty() {
                        continue;
                    }
                    let k = (w.r0 - row0) / size;
                    *b = k as u16;
                    match wins.last_mut() {
                        Some((j, v)) if *j == k => *v = v.union(w),
                        _ => wins.push((k, w)),
                    }
                }
                (cells, wins)
            })
            .collect();
        for &(k, w) in cells.iter().flat_map(|(_, ws)| ws) {
            win[k] = win[k].union(w);
        }
        Bands { win, samples: by_band, outside, cells: cells.into_iter().map(|(c, _)| c).collect() }
    }
}

fn canopy(dir: &Path) -> Result<()> {
    let grid = GridIndex::load(dir)?;
    let terr_l = roadcore::grid::Layer::<i16>::open(&dir.join("grid.terrain.i16"))?;
    let terr = terr_l.data();
    let samples_a = Array::<Sample>::open(&dir.join("samples.bin"))?;
    let samples = samples_a.get();
    // The canopy files: data/cache/chm10 next to the build, or under SCENIC_CACHE (shared by units).
    let cache = match std::env::var_os("SCENIC_CACHE") {
        Some(c) => PathBuf::from(c).join("chm10"),
        None => dir.parent().unwrap().join("cache/chm10"),
    };
    std::fs::create_dir_all(&cache)?;
    // The NAS's store of them (`sources/canopy/`), where each is downloaded once.
    let store = std::env::var_os("SCENIC_CANOPY_STORE").map(PathBuf::from);
    let band = std::env::var("SCENIC_CANOPY_BAND").ok().and_then(|v| v.parse().ok()).filter(|&b: &usize| b > 0).unwrap_or(BAND_ROWS);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(1800)))
        .user_agent("road-elevations/0.1 (personal offline map)")
        .build()
        .into();

    // 10° tiles needed: (top latitude, left longitude).
    let mut need: BTreeSet<(i32, i32)> = BTreeSet::new();
    for t in &grid.tiles {
        for (dx, dy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = ((t[0] + dx) as f64 * 256.0, (t[1] + dy) as f64 * 256.0);
            let lon = x / roadcore::grid::WORLD * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / roadcore::grid::WORLD)).dsinh().datan().to_degrees();
            need.insert(((lat / 10.0).ceil() as i32 * 10, (lon / 10.0).floor() as i32 * 10));
        }
    }
    // Previous results: samples and grid tiles done in the last run (its outputs are still here),
    // unless new terrain is within reach.
    let cdir = scache::dir(dir);
    let change = GridChange::load(&cdir, "canopy", &grid.tiles);
    let prev = scache::Prev::load(&cdir, "canopy");
    // (The last run's outputs: each opened only when it's the size the cache says, and let go once
    // used, as in WebAssembly an open file is read whole into memory.)
    let sized = |name: &str, len: usize| std::fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == len as u64);
    let old_near = (!prev.is_empty() && sized("near.i8", prev.len() * NEAR_AZ)).then(|| Array::<i8>::open(&dir.join("near.i8")).ok()).flatten();
    let old_road = sized("roadside.u8", prev.len() * 2).then(|| Array::<u8>::open(&dir.join("roadside.u8")).ok()).flatten();
    let near: Vec<AtomicI8> = (0..samples.len() * NEAR_AZ).map(|_| AtomicI8::new(i8::MIN)).collect();
    let roadside: Vec<AtomicU8> = (0..samples.len() * 2).map(|_| AtomicU8::new(0)).collect();
    let todo_s: Vec<bool> = samples
        .par_iter()
        .enumerate()
        .map(|(si, s)| {
            let (Some(on), Some(or)) = (&old_near, &old_road) else { return true };
            if change.near(s.lon as f64 * E7, s.lat as f64 * E7, 1) {
                return true;
            }
            match prev.row(scache::sample_key(s)) {
                Some(r) => {
                    let (on, or) = (on.get(), or.get());
                    for a in 0..NEAR_AZ {
                        near[si * NEAR_AZ + a].store(on[r * NEAR_AZ + a], Relaxed);
                    }
                    roadside[si * 2].store(or[r * 2], Relaxed);
                    roadside[si * 2 + 1].store(or[r * 2 + 1], Relaxed);
                    false
                }
                None => true,
            }
        })
        .collect();
    drop((prev, old_near, old_road));
    let mut canopy_out = vec![0u8; grid.tiles.len() * CELLS];
    let mut cover_out = vec![0u8; grid.tiles.len() * CELLS];
    // Grid tiles: copied from the last run's layers where the tile was there (read tile by tile).
    let mut todo_t = vec![true; grid.tiles.len()];
    let n = change.prev_len() * CELLS;
    let old = (n > 0 && sized("grid.canopy.u8", n) && sized("grid.cover.u8", n)).then(|| (File::open(dir.join("grid.canopy.u8")), File::open(dir.join("grid.cover.u8"))));
    if let Some((Ok(oc), Ok(ov))) = old {
        use store::sys::PosIo;
        let slot = change.prev_slots();
        for (i, t) in grid.tiles.iter().enumerate() {
            if let Some(&j) = slot.get(t) {
                oc.read_exact_at(&mut canopy_out[i * CELLS..(i + 1) * CELLS], (j * CELLS) as u64)?;
                ov.read_exact_at(&mut cover_out[i * CELLS..(i + 1) * CELLS], (j * CELLS) as u64)?;
                todo_t[i] = false;
            }
        }
    }
    let n_s = todo_s.iter().filter(|&&t| t).count();
    let n_t = todo_t.iter().filter(|&&t| t).count();
    // 10° tiles with something to do (a sample or grid tile, with the near-field margin).
    let margin = NEAR_MAX_M / 111_000.0 * 2.0;
    let touches = |top: i32, left: i32, lon: f64, lat: f64| {
        lon >= left as f64 - margin && lon <= left as f64 + 10.0 + margin && lat <= top as f64 + margin && lat >= top as f64 - 10.0 - margin
    };
    need.retain(|&(top, left)| {
        samples.iter().zip(&todo_s).any(|(s, &t)| t && touches(top, left, s.lon as f64 * E7, s.lat as f64 * E7))
            || grid.tiles.iter().zip(&todo_t).any(|(g, &t)| {
                if !t {
                    return false;
                }
                let lon = (g[0] as f64 + 0.5) * 256.0 / roadcore::grid::WORLD * 360.0 - 180.0;
                let y = (g[1] as f64 + 0.5) * 256.0 / roadcore::grid::WORLD;
                let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).dsinh().datan().to_degrees();
                touches(top, left, lon, lat) || (lon - (left as f64 + 5.0)).abs() < 6.0 && (lat - (top as f64 - 5.0)).abs() < 6.0
            })
    });
    eprintln!(
        "canopy: {} samples ({} to do, {} cached), {} grid tiles ({} to do), {} ten-degree tiles to read",
        samples.len(),
        n_s,
        samples.len() - n_s,
        grid.tiles.len(),
        n_t,
        need.len()
    );
    let pb = count_bar(need.len() as u64, "canopy 10° tiles");
    for &(top, left) in &need {
        // The rows the work here reaches: its samples to do, with the near field's margin, and its
        // grid tiles to do.
        let (mut lat_lo, mut lat_hi) = (f64::MAX, f64::MIN);
        for (s, &t) in samples.iter().zip(&todo_s) {
            let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
            if t && touches(top, left, lon, lat) {
                (lat_lo, lat_hi) = (lat_lo.min(lat - margin), lat_hi.max(lat + margin));
            }
        }
        for (g, &t) in grid.tiles.iter().zip(&todo_t) {
            if !t {
                continue;
            }
            let lat_of = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y * 256.0 / roadcore::grid::WORLD)).dsinh().datan().to_degrees();
            let lon_of = |x: f64| x * 256.0 / roadcore::grid::WORLD * 360.0 - 180.0;
            let (w, e, n, so) = (lon_of(g[0] as f64), lon_of(g[0] as f64 + 1.0), lat_of(g[1] as f64), lat_of(g[1] as f64 + 1.0));
            if e >= left as f64 && w <= left as f64 + 10.0 && so <= top as f64 && n >= top as f64 - 10.0 {
                (lat_lo, lat_hi) = (lat_lo.min(so), lat_hi.max(n));
            }
        }
        let row0 = ((top as f64 - lat_hi) / C10_RES).floor().clamp(0.0, C10 as f64) as usize;
        let row1 = (((top as f64 - lat_lo) / C10_RES).ceil() + 1.0).clamp(0.0, C10 as f64) as usize;
        if row1 <= row0 {
            pb.inc(1);
            continue;
        }
        let rows = row1 - row0;
        let name = |st: &str| format!("meta_chm_lat={top}.0_lon={left}.0_{st}.tif");
        let height: fn(u16) -> u8 = |v| ((v as u32 + 50) / 100).min(254) as u8;
        let share: fn(u16) -> u8 = |v| ((v as u32).min(1000) * 255 / 1000) as u8;
        let layers: Vec<Option<Layer>> = [("median", height), ("p95", height), ("cover5m", share)]
            .par_iter()
            .map(|&(st, f)| Layer::open(&agent, format!("{CHM10_URL}/{}", name(st)), cache.join(name(st)), store.as_ref().map(|s| s.join(name(st))), f))
            .collect::<Result<_>>()?;
        let Ok([Some(median), Some(p95), Some(cover)]) = <[Option<Layer>; 3]>::try_from(layers) else {
            pb.println(format!("canopy {top},{left}: no data"));
            pb.inc(1);
            continue;
        };
        let mut layers = [median, p95, cover];
        let bands = Bands::plan(top as f64, left as f64, (row0, row1), band, samples, &todo_s, |lon, lat| touches(top, left, lon, lat), &grid.tiles, &todo_t);
        for (k, (&w, band_samples)) in bands.win.iter().zip(&bands.samples).enumerate() {
            if w.is_empty() && band_samples.is_empty() {
                continue;
            }
            let read: Vec<Vec<u8>> = if w.is_empty() { vec![Vec::new(); 3] } else { layers.par_iter_mut().map(|l| l.read(w)).collect::<Result<_>>()? };
            let Ok([median, p95, cover]) = <[Vec<u8>; 3]>::try_from(read) else { unreachable!() };
            let t = Chm10 { left: left as f64, top: top as f64, row0, rows, win: w, median, p95, cover };
            let width = w.c1 - w.c0;

            // Grid layers: cells whose centre lies in this tile (4 sub-samples per cell, at a quarter
            // and three quarters of the cell each way), for tiles to do: the band's cell rows of
            // them. A sub-sample's column in the square depends only on its longitude and its row
            // only on its latitude: each is worked out once per column and per row of the tile
            // (Chm10::idx in two halves), for tiles with a cell row in the band (whose columns its
            // window holds). (A tile a task: a band's tiles are a short run of the list, which
            // rayon's usual split would leave to one or two threads.)
            canopy_out.par_chunks_mut(CELLS).zip(cover_out.par_chunks_mut(CELLS)).zip(grid.tiles.par_iter().zip(&bands.cells)).with_max_len(1).for_each(|((can, cov), (tile, cells))| {
                if !cells.iter().any(|&b| b as usize == k) {
                    return;
                }
                let cols: Vec<[Option<usize>; 2]> = (0..256usize)
                    .map(|cx| {
                        let gx = tile[0] as f64 * 256.0 + cx as f64;
                        [0.25, 0.75].map(|ox| t.col((gx + ox) / roadcore::grid::WORLD * 360.0 - 180.0))
                    })
                    .collect();
                for cy in (0..256usize).filter(|&cy| cells[cy] as usize == k) {
                    let gy = tile[1] as f64 * 256.0 + cy as f64;
                    let rows = [0.25, 0.75].map(|oy| t.row((std::f64::consts::PI * (1.0 - 2.0 * (gy + oy) / roadcore::grid::WORLD)).dsinh().datan().to_degrees()));
                    for (cx, c) in cols.iter().enumerate() {
                        let (mut sh, mut sc, mut n) = (0u32, 0u32, 0u32);
                        for (kx, ky) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            if let (Some(c), Some(r)) = (c[kx], rows[ky]) {
                                let i = r * width + c;
                                sh += t.median[i] as u32;
                                sc += t.cover[i] as u32;
                                n += 1;
                            }
                        }
                        if n > 0 {
                            can[cy * 256 + cx] = (sh / n) as u8;
                            cov[cy * 256 + cx] = (sc / n) as u8;
                        }
                    }
                }
            });

            // Near field for the band's samples to do in (or within 300 m of) this tile.
            band_samples.par_iter().for_each(|&si| {
                let s = &samples[si as usize];
                let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
                let owned = lon >= t.left && lon < t.left + 10.0 && lat <= t.top && lat > t.top - 10.0;
                near_field(s, si as usize, owned, &t, &grid, terr, &near, &roadside);
            });
        }
        // The samples just outside it: near_field over nothing read (every lookup finds nothing, as
        // it did when the whole square was read; a tunnel's horizon set all the same).
        let none = Chm10 { left: left as f64, top: top as f64, row0, rows: 0, win: Window::NONE, median: Vec::new(), p95: Vec::new(), cover: Vec::new() };
        bands.outside.par_iter().for_each(|&si| near_field(&samples[si as usize], si as usize, false, &none, &grid, terr, &near, &roadside));
        pb.inc(1);
    }
    pb.finish_and_clear();
    let near_bytes: Vec<i8> = near.into_iter().map(|a| a.into_inner()).collect();
    let road_bytes: Vec<u8> = roadside.into_iter().map(|a| a.into_inner()).collect();
    std::fs::write(roadcore::tmp(dir, "grid.canopy.u8"), &canopy_out)?;
    std::fs::write(roadcore::tmp(dir, "grid.cover.u8"), &cover_out)?;
    std::fs::write(roadcore::tmp(dir, "near.i8"), bytemuck::cast_slice(&near_bytes))?;
    std::fs::write(roadcore::tmp(dir, "roadside.u8"), &road_bytes)?;
    roadcore::commit(dir, &["grid.canopy.u8", "grid.cover.u8", "near.i8", "roadside.u8"])?;
    // What each row is, for the next run.
    let keys: Vec<u64> = samples.par_iter().map(scache::sample_key).collect();
    scache::Prev::save(&cdir, "canopy", &keys)?;
    GridChange::save(&cdir, "canopy", &grid.tiles)?;
    Ok(())
}

/// `N` directions round the compass from north: each one's (east, north) unit step.
fn compass<const N: usize>() -> [(f64, f64); N] {
    std::array::from_fn(|a| {
        let th = a as f64 * std::f64::consts::TAU / N as f64;
        (th.dsin(), th.dcos())
    })
}

/// The near field's directions, and the roadside's.
static NEAR_DIRS: LazyLock<[(f64, f64); NEAR_AZ]> = LazyLock::new(compass);
static ROADSIDE_DIRS: LazyLock<[(f64, f64); 16]> = LazyLock::new(compass);

/// Metres per degree of longitude and of latitude at latitude `lat`: near_field's steps (and so how
/// far its lookups reach, Bands).
fn m_per_deg(lat: f64) -> (f64, f64) {
    (111_320.0 * lat.to_radians().dcos(), 111_320.0)
}

/// Near-field horizon per direction (0.5° units; merged by max across tiles) through
/// terrain + median canopy height to 300 m, plus roadside p95 tree height and forest cover
/// within 150 m for the owning tile.
#[allow(clippy::too_many_arguments)]
fn near_field(s: &Sample, si: usize, owned: bool, t: &Chm10, grid: &GridIndex, terr: &[i16], near: &[AtomicI8], roadside: &[AtomicU8]) {
    let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
    let (m_lon, m_lat) = m_per_deg(lat);
    let (gx, gy) = roadcore::grid::cell_of(lon, lat);
    let cm = roadcore::grid::cell_m(lat);
    let eye = eye_height(s, grid, terr);
    let tunnel = s.flags & sflag::TUNNEL != 0;
    for (a, &(sx, sy)) in NEAR_DIRS.iter().enumerate() {
        let mut best = f64::MIN;
        let mut d = 8.0;
        while d <= NEAR_MAX_M {
            let (plon, plat) = (lon + sx * d / m_lon, lat + sy * d / m_lat);
            if let Some(i) = t.idx(plon, plat) {
                let ground = roadcore::grid::terrain_bilinear(grid, terr, gx + sx * d / cm, gy - sy * d / cm).unwrap_or(eye - EYE_M);
                let h = ground + t.median[i] as f32;
                best = best.max((h - eye) as f64 / d);
            }
            d += 14.0;
        }
        if tunnel {
            best = best.max(10.0);
        }
        if best > f64::MIN {
            let v = (best.datan().to_degrees() * 2.0).round().clamp(-127.0, 127.0) as i8;
            near[si * NEAR_AZ + a].fetch_max(v, Relaxed);
        }
    }
    if owned {
        let (mut sum, mut n, mut cov, mut m) = (0f32, 0f32, 0f32, 0f32);
        for &(sx, sy) in ROADSIDE_DIRS.iter() {
            for d in [10.0, 20.0, 30.0] {
                if let Some(i) = t.idx(lon + sx * d / m_lon, lat + sy * d / m_lat) {
                    sum += t.p95[i] as f32;
                    n += 1.0;
                }
            }
            for k in 1..=10 {
                let d = 15.0 * k as f64;
                if let Some(i) = t.idx(lon + sx * d / m_lon, lat + sy * d / m_lat) {
                    cov += t.cover[i] as f32;
                    m += 1.0;
                }
            }
        }
        if n > 0.0 {
            roadside[si * 2].store(((sum / n) * 8.0).round().min(255.0) as u8, Relaxed);
        }
        if m > 0.0 {
            roadside[si * 2 + 1].store((cov / m).round().min(255.0) as u8, Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A width × height u16 LZW TIFF, `rps` rows a strip, value = row × 100 + column.
    fn tiff(width: usize, height: usize, rps: usize) -> Vec<u8> {
        let mut strips = Vec::new();
        for r0 in (0..height).step_by(rps) {
            let raw: Vec<u8> = (r0..(r0 + rps).min(height)).flat_map(|r| (0..width).flat_map(move |c| ((r * 100 + c) as u16).to_le_bytes())).collect();
            strips.push(weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8).encode(&raw).unwrap());
        }
        let n = strips.len();
        let tags: [(u16, Vec<u32>); 6] = [(256, vec![width as u32]), (257, vec![height as u32]), (259, vec![5]), (278, vec![rps as u32]), (273, vec![0; n]), (279, strips.iter().map(|s| s.len() as u32).collect())];
        let ifd = 8;
        let arrays = ifd + 2 + tags.len() * 12 + 4;
        let mut b = b"II\x2a\x00".to_vec();
        b.extend_from_slice(&(ifd as u32).to_le_bytes());
        b.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        // Arrays of more than one value after the directory, then the strips.
        let mut at = arrays;
        let data = arrays + 2 * 4 * n;
        let offs: Vec<u32> = strips.iter().scan(data, |o, s| { let v = *o; *o += s.len(); Some(v as u32) }).collect();
        let mut tail = Vec::new();
        for (tag, vals) in &tags {
            let vals = if *tag == 273 { &offs } else { vals };
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&4u16.to_le_bytes());
            b.extend_from_slice(&(vals.len() as u32).to_le_bytes());
            if vals.len() == 1 {
                b.extend_from_slice(&vals[0].to_le_bytes());
            } else {
                b.extend_from_slice(&(at as u32).to_le_bytes());
                at += vals.len() * 4;
                tail.extend(vals.iter().flat_map(|v| v.to_le_bytes()));
            }
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend(tail);
        assert_eq!(b.len(), data);
        for s in &strips {
            b.extend_from_slice(s);
        }
        b
    }

    /// Reads of `b`, as of a file.
    fn at(b: &[u8]) -> impl Fn(u64, usize) -> Result<Vec<u8>> + Sync + '_ {
        move |o, n| b.get(o as usize..).and_then(|b| b.get(..n)).map(<[u8]>::to_vec).context("past the end")
    }

    #[test]
    fn windows_decoded_straight_into_place() {
        let (w, h) = (5, 7);
        let b = tiff(w, h, 3);
        assert!(pipeline::whole::tiff_bytes_whole(&b));
        let st = parse_tiff(&at(&b)).unwrap();
        let f = |v: u16| (v % 251) as u8;
        // Windows starting and ending mid-strip, one row, all rows; all columns, or some.
        for (rows, cols) in [(0..7, 0..5), (2..5, 0..5), (4..5, 1..4), (6..7, 4..5), (1..6, 2..3), (0..7, 0..1)] {
            let got = decode_rows(&at(&b), &st, rows.clone(), cols.clone(), f).unwrap();
            let want: Vec<u8> = rows.clone().flat_map(|r| cols.clone().map(move |c| f((r * 100 + c) as u16))).collect();
            assert_eq!(got, want, "rows {rows:?} columns {cols:?}");
        }
        // Cut short, or a strip damaged (even outside the columns read): an error, not a panic or
        // zeros.
        assert!(parse_tiff(&at(&b[..20])).is_err());
        assert!(decode_rows(&at(&b[..b.len() - 3]), &st, 0..7, 0..5, f).is_err());
        let mut bad = b.clone();
        let (off, len) = st.strips[1];
        bad[off as usize..off as usize + len].fill(0xff);
        assert!(decode_rows(&at(&bad), &st, 3..5, 0..1, f).is_err());
        // (Rows away from the damaged strip still decode.)
        assert!(decode_rows(&at(&bad), &st, 0..3, 0..5, f).is_ok());
        // From a file, positionally.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.tif");
        std::fs::write(&p, &b).unwrap();
        let t = Tiff::open(File::open(&p).unwrap()).unwrap();
        assert_eq!(t.read(Window { r0: 2, r1: 6, c0: 1, c1: 4 }, f).unwrap(), decode_rows(&at(&b), &st, 2..6, 1..4, f).unwrap());
        assert!(canopy_tiff(File::open(&p).unwrap()).is_err(), "not 40000²");
        // A damaged directory (StripByteCounts, its sixth entry, counting 4 billion): an error.
        bad = b.clone();
        bad[10 + 5 * 12 + 4..10 + 5 * 12 + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&p, &bad).unwrap();
        assert!(Tiff::open(File::open(&p).unwrap()).is_err());
    }

    #[test]
    fn a_sample_just_outside_the_square_is_in_no_band() {
        // A square at 50° N, 90° W: a sample inside it, and one 0.005° west of it (within the margin
        // the square's read with, beyond its 300 m reach): that one in no band, its lookups none.
        let at = |lon: f64, lat: f64| Sample { way: 0, dist: 0.0, lon: (lon / E7).round() as i32, lat: (lat / E7).round() as i32, eye: 0.0, flags: 0, _pad: [0; 3] };
        let samples = [at(-89.5, 45.0), at(-90.005, 45.0)];
        let (top, left) = (50.0, -90.0);
        let px = |lat: f64| ((top - lat) / C10_RES).floor() as usize;
        let rows = (px(45.01), px(44.99));
        let b = Bands::plan(top, left, rows, 64, &samples, &[true, true], |_, _| true, &[], &[]);
        assert_eq!(b.outside, [1]);
        assert_eq!(b.samples.iter().flatten().copied().collect::<Vec<_>>(), [0]);
        // An empty window: every lookup finds nothing, and none fails.
        let none = Chm10 { left, top, row0: rows.0, rows: 0, win: Window::NONE, median: Vec::new(), p95: Vec::new(), cover: Vec::new() };
        assert_eq!(none.idx(-89.999, 45.0), None);
        assert_eq!(none.idx(-90.001, 45.0), None);
    }

    #[test]
    fn windows_clipped_and_joined() {
        assert_eq!(Window::clip((-3.0, 5.0), (39_998.0, 40_005.0), (2, 100)), Window { r0: 2, r1: 6, c0: 39_998, c1: C10 });
        assert!(Window::clip((200.0, 300.0), (0.0, 1.0), (2, 100)).is_empty());
        assert!(Window::clip((5.0, 4.0), (0.0, 1.0), (2, 100)).is_empty());
        // (A bound that's NaN or infinite is no bound.)
        assert_eq!(Window::clip((f64::NAN, f64::INFINITY), (f64::NEG_INFINITY, f64::NAN), (2, 100)), Window { r0: 2, r1: 100, c0: 0, c1: C10 });
        let w = Window { r0: 50, r1: 60, c0: 10, c1: 20 };
        assert_eq!(Window::NONE.union(w), w);
        assert_eq!(w.union(Window::NONE), w);
        assert_eq!(w.union(Window { r0: 2, r1: 6, c0: 30, c1: 40 }), Window { r0: 2, r1: 60, c0: 10, c1: 40 });
    }

    #[test]
    fn files_kept_copied_or_downloaded_whole() {
        let d = tempfile::tempdir().unwrap();
        let (cache, store) = (d.path().join("cache"), d.path().join("store"));
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).build().into();
        let b = tiff(5, 7, 3);
        let whole = |f: &File| {
            let len = f.metadata().unwrap().len();
            read_at(f, len, 0, len as usize).unwrap()
        };
        let never = "http://127.0.0.1:9/never";
        // The NAS's copy, copied here (this one, cut short, deleted first); then the copy here.
        std::fs::write(store.join("a.tif"), &b).unwrap();
        std::fs::write(cache.join("a.tif"), &b[..b.len() - 1]).unwrap();
        for _ in 0..2 {
            let f = fetch_file(&agent, never, &cache.join("a.tif"), Some(&store.join("a.tif"))).unwrap().unwrap();
            assert_eq!(whole(&f), b);
        }
        // Meta's "none there", from the NAS.
        std::fs::write(store.join("b.tif"), b"").unwrap();
        assert!(fetch_file(&agent, never, &cache.join("b.tif"), Some(&store.join("b.tif"))).unwrap().is_none());
        assert_eq!(std::fs::metadata(cache.join("b.tif")).unwrap().len(), 0);
        // Downloaded (a body cut short tried again), into the NAS's store and here.
        let srv = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/c.tif", srv.local_addr().unwrap());
        let body = b.clone();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            for cut in [true, false] {
                let (mut s, _) = srv.accept().unwrap();
                let (mut req, mut buf) = (Vec::new(), [0u8; 1024]);
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut buf).unwrap();
                    assert!(n > 0);
                    req.extend_from_slice(&buf[..n]);
                }
                s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).unwrap();
                s.write_all(&body[..if cut { body.len() / 2 } else { body.len() }]).unwrap();
            }
        });
        let f = fetch_file(&agent, &url, &cache.join("c.tif"), Some(&store.join("c.tif"))).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(whole(&f), b);
        assert_eq!(std::fs::read(cache.join("c.tif")).unwrap(), b);
        assert_eq!(std::fs::read(store.join("c.tif")).unwrap(), b);
        // No temporary file left, nor the download's lock.
        for dir in [&cache, &store] {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                assert!(!pipeline::whole::is_tmp(&p) && p.extension().is_some_and(|x| x == "tif"), "{} left", p.display());
            }
        }
    }
}
