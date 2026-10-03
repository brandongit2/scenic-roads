//! Peak prominence and isolation from terrain tiles (the `peaks` binary over a build folder's
//! pois.json and terrain.tiles; docs/phase5.md "peaks").
//!
//! Summit: the highest DEM pixel within 150 m of the OSM point. A pixel claimed by several peaks
//! (needles next to a main summit) goes to the one tagged highest; the others start from their own
//! point. The DEM (~30 m at z12) cuts sharp summits down (Snowdon: 1,040 m for 1,085 m, below the
//! broad Carneddau), so the summit height is the tagged `ele` where it is plausible (from 30 m
//! under to 200 m over the DEM), else the DEM's; every summit pixel is raised to its peak's height
//! for all floods and searches, so a neighbouring summit counts at its real height too (Garnedd
//! Ugain, 740 m from Snowdon, would otherwise see nothing higher nearby).
//!
//! Prominence: a priority flood from the summit, always expanding the highest unvisited pixel,
//! until it reaches ground higher than the summit; the lowest pixel it had to pass is the key col,
//! and prominence = summit − col. Once the flood is down to sea level the answer is the summit
//! height whatever lies beyond. It runs at z12 (terrain.tiles, finer where stored near roads)
//! for up to 600k pixels, then, for the bigger peaks, again at z8 over the whole region (preloaded;
//! ~430 m pixels, which can miss narrow cols and summits, so those values are rougher). Missing
//! tiles are walls: a flood that ends without higher ground or sea gives a lower bound.
//!
//! The terrain tiles have single-pixel spikes and pits (a 2,781 m pixel among ~700 m ones in a z8
//! tile over the Laurentides), which would be false summits and cols: a pixel more than
//! max(150 m, 1.2 × the pixel size) above or below all eight neighbours is clamped to them.
//!
//! Isolation: the distance to the nearest ground higher than the summit, searching tiles nearest
//! first (z12 within 25 km, then z8 over the region); none found = a lower bound (region edge).
//!
//! Results as peaks.json has them: {i (the peak's index), e (DEM summit m), p (prominence m), pl
//! (lower bound), c ([lon, lat] of the col), ce (col m), iso (km), il (lower bound), hi ([lon, lat]
//! of the nearest higher ground)}.

pub mod unit;

use rayon::prelude::*;
use roadcore::archive::Archive;
use roadcore::grid::decode_terrain_png;
use roadcore::{dist_m, merc};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub const FINE_Z: u8 = 12;
pub const COARSE_Z: u8 = 8;
const FINE_MAX: usize = 600_000;
const COARSE_MAX: usize = 40_000_000;
const ISO_FINE_KM: f64 = 25.0;
const TS: i64 = 256;

pub type Tile = Arc<Vec<f32>>;

/// Where a zoom's elevation tiles come from (decoded, metres; None: none there).
pub trait Tiles: Sync {
    fn get(&self, z: u8, x: u32, y: u32) -> Option<Vec<f32>>;
}

/// A build folder's archive: its tiles, else the nearest stored ancestor upsampled (today's).
pub struct ArchiveTiles<'a>(pub &'a Archive);

impl Tiles for ArchiveTiles<'_> {
    fn get(&self, z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
        roadcore::grid::tile_with_fallback(self.0, z, x, y)
    }
}

/// Summit heights stamped over the DEM: by pixel, and by tile for the tile scans.
#[derive(Default)]
struct Overlay {
    px: HashMap<(i64, i64), f32>,
    tiles: HashMap<(i64, i64), Vec<(i64, i64, f32)>>,
}

impl Overlay {
    fn add(&mut self, x: i64, y: i64, e: f32) {
        let v = self.px.entry((x, y)).or_insert(f32::MIN);
        *v = v.max(e);
    }
    fn index(&mut self) {
        for (&(x, y), &e) in &self.px {
            self.tiles.entry((x.div_euclid(TS), y.div_euclid(TS))).or_default().push((x, y, e));
        }
        // In pixel order, so that of two summits at the same distance the same one is the nearest
        // higher ground every run.
        for v in self.tiles.values_mut() {
            v.sort_unstable_by_key(|p| (p.1, p.0));
        }
    }
}

/// Clamp single-pixel spikes and pits (interior pixels) to their neighbours.
pub(crate) fn despike(t: &mut [f32], z: u8, lat: f64) {
    let px_m = 40_075_016.7 * lat.to_radians().cos() / ((1u64 << z) as f64 * TS as f64);
    let thr = (1.2 * px_m).max(150.0) as f32;
    let src = t.to_vec();
    let w = TS as usize;
    for y in 1..w - 1 {
        for x in 1..w - 1 {
            let v = src[y * w + x];
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for (dx, dy) in [(-1i32, -1i32), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let n = src[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize];
                lo = lo.min(n);
                hi = hi.max(n);
            }
            if v > hi + thr {
                t[y * w + x] = hi;
            } else if v < lo - thr {
                t[y * w + x] = lo;
            }
        }
    }
}

/// Latitude of a tile's centre.
pub(crate) fn tile_lat(z: u8, ty: u32) -> f64 {
    let y = (ty as f64 + 0.5) / (1u64 << z) as f64;
    (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()
}

/// Pixel grid at one zoom: global pixel coordinates, tiles decoded on demand (with ancestors
/// upsampled where a tile isn't stored), cached per worker.
struct Dem<'a> {
    src: &'a dyn Tiles,
    z: u8,
    cache: HashMap<(u32, u32), Option<Tile>>,
    shared: Option<&'a HashMap<(u32, u32), Tile>>,
    ov: Option<&'a Overlay>,
    max_cache: usize,
}

impl<'a> Dem<'a> {
    fn new(src: &'a dyn Tiles, z: u8, max_cache: usize) -> Self {
        Self { src, z, cache: HashMap::new(), shared: None, ov: None, max_cache }
    }
    fn tile(&mut self, tx: i64, ty: i64) -> Option<Tile> {
        let n = 1i64 << self.z;
        if tx < 0 || ty < 0 || tx >= n || ty >= n {
            return None;
        }
        let k = (tx as u32, ty as u32);
        if let Some(s) = self.shared {
            return s.get(&k).cloned();
        }
        if let Some(t) = self.cache.get(&k) {
            return t.clone();
        }
        if self.cache.len() >= self.max_cache {
            self.cache.clear();
        }
        let t = self.src.get(self.z, k.0, k.1).map(|mut t| {
            despike(&mut t, self.z, tile_lat(self.z, k.1));
            Arc::new(t)
        });
        self.cache.insert(k, t.clone());
        t
    }
    fn at(&mut self, gx: i64, gy: i64) -> Option<f32> {
        let t = self.tile(gx.div_euclid(TS), gy.div_euclid(TS))?;
        let v = t[(gy.rem_euclid(TS) * TS + gx.rem_euclid(TS)) as usize].max(0.0);
        Some(match self.ov.and_then(|o| o.px.get(&(gx, gy))) {
            Some(&s) => v.max(s),
            None => v,
        })
    }
    fn px(&self, lon: f64, lat: f64) -> (i64, i64) {
        let (x, y) = merc(lon, lat);
        let w = (1i64 << self.z) as f64 * TS as f64;
        ((x * w).floor() as i64, (y * w).floor() as i64)
    }
    fn lonlat(&self, gx: i64, gy: i64) -> [f64; 2] {
        let w = (1i64 << self.z) as f64 * TS as f64;
        let (x, y) = ((gx as f64 + 0.5) / w, (gy as f64 + 0.5) / w);
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
        [x * 360.0 - 180.0, lat]
    }
}

/// Priority flood from (sx, sy) at height e: Ok((col height, col pixel, reached higher or sea)),
/// or Err((lowest point so far, its pixel)) when the pixel budget ran out (a lower bound). Each queued pixel carries the lowest point on the path that
/// reached it, so the col is where the flood actually crossed, not just the lowest level seen.
fn flood(d: &mut Dem, sx: i64, sy: i64, e: f32, budget: usize) -> Result<(f32, (i64, i64), bool), (f32, (i64, i64))> {
    let key = |v: f32| (v * 100.0).round() as i32;
    // (height, x, y, path-min height, its x, y)
    let mut heap: BinaryHeap<(i32, i64, i64, Reverse<i32>, i64, i64)> = BinaryHeap::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    heap.push((key(e), sx, sy, Reverse(key(e)), sx, sy));
    seen.insert((sx, sy));
    let mut low = (key(e), (sx, sy));
    while let Some((k, x, y, Reverse(pm), pmx, pmy)) = heap.pop() {
        if k as f32 / 100.0 > e + 0.5 {
            return Ok((pm as f32 / 100.0, (pmx, pmy), true));
        }
        if k < low.0 {
            low = (k, (x, y));
        }
        if pm <= 0 {
            return Ok((0.0, (pmx, pmy), true));
        }
        if seen.len() > budget {
            return Err((low.0 as f32 / 100.0, low.1));
        }
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (nx, ny) = (x + dx, y + dy);
            if !seen.insert((nx, ny)) {
                continue;
            }
            if let Some(h) = d.at(nx, ny) {
                let hk = key(h);
                let (m, mx, my) = if hk < pm { (hk, nx, ny) } else { (pm, pmx, pmy) };
                heap.push((hk, nx, ny, Reverse(m), mx, my));
            }
        }
    }
    // Ran out of data (region edge or missing tiles) without higher ground or the sea.
    Ok((low.0 as f32 / 100.0, low.1, false))
}

/// Nearest pixel higher than e within max_km (tiles in order of distance, skipping tiles whose
/// maximum isn't higher). Some((km, pixel)) or None.
fn nearest_higher(d: &mut Dem, sx: i64, sy: i64, e: f32, max_km: f64, maxes: &mut HashMap<(i64, i64), f32>) -> Option<(f64, (i64, i64))> {
    let s_ll = d.lonlat(sx, sy);
    let km_of = |d: &Dem, x: i64, y: i64| {
        let ll = d.lonlat(x, y);
        dist_m(s_ll[0], s_ll[1], ll[0], ll[1]) / 1000.0
    };
    // Rough metres per pixel here (for the tile distance bound).
    let mpp = km_of(d, sx + 1, sy) * 1000.0;
    let (stx, sty) = (sx.div_euclid(TS), sy.div_euclid(TS));
    let tile_min_px = |tx: i64, ty: i64| -> f64 {
        let cx = sx.clamp(tx * TS, tx * TS + TS - 1);
        let cy = sy.clamp(ty * TS, ty * TS + TS - 1);
        (((cx - sx) as f64).powi(2) + ((cy - sy) as f64).powi(2)).sqrt()
    };
    let mut heap: BinaryHeap<Reverse<(u64, i64, i64)>> = BinaryHeap::new();
    let mut queued: HashSet<(i64, i64)> = HashSet::new();
    heap.push(Reverse((0, stx, sty)));
    queued.insert((stx, sty));
    let mut best: Option<(f64, (i64, i64))> = None;
    let max_px = max_km * 1000.0 / mpp.max(1.0) * 1.2;
    while let Some(Reverse((dk, tx, ty))) = heap.pop() {
        let dmin = dk as f64 / 16.0;
        if let Some((bkm, _)) = best {
            if dmin * mpp / 1000.0 > bkm * 1.05 {
                break;
            }
        }
        if dmin > max_px {
            break;
        }
        if let Some(t) = d.tile(tx, ty) {
            let ovp: &[(i64, i64, f32)] = d.ov.and_then(|o| o.tiles.get(&(tx, ty))).map(Vec::as_slice).unwrap_or(&[]);
            let mx = *maxes.entry((tx, ty)).or_insert_with(|| t.iter().cloned().fold(f32::MIN, f32::max).max(ovp.iter().map(|p| p.2).fold(f32::MIN, f32::max)));
            if mx > e + 0.5 {
                for &(x, y, v) in ovp {
                    if v > e + 0.5 {
                        let km = km_of(d, x, y);
                        if best.is_none_or(|(b, _)| km < b) {
                            best = Some((km, (x, y)));
                        }
                    }
                }
                for j in 0..TS {
                    for i in 0..TS {
                        let v = t[(j * TS + i) as usize];
                        if v > e + 0.5 {
                            let (x, y) = (tx * TS + i, ty * TS + j);
                            let km = km_of(d, x, y);
                            if best.is_none_or(|(b, _)| km < b) {
                                best = Some((km, (x, y)));
                            }
                        }
                    }
                }
            }
        }
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (nx, ny) = (tx + dx, ty + dy);
            if queued.insert((nx, ny)) {
                heap.push(Reverse(((tile_min_px(nx, ny) * 16.0) as u64, nx, ny)));
            }
        }
    }
    best.filter(|(km, _)| *km <= max_km)
}

/// A peak's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Out {
    pub i: usize,
    pub e: f32,
    pub p: f32,
    pub pl: bool,
    pub c: [f64; 2],
    pub ce: f32,
    pub iso: f64,
    pub il: bool,
    pub hi: Option<[f64; 2]>,
}

impl Out {
    /// As peaks.json has it.
    pub fn json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({ "i": self.i, "e": self.e, "p": self.p, "c": self.c, "ce": self.ce, "iso": self.iso });
        if self.pl {
            v["pl"] = true.into();
        }
        if self.il {
            v["il"] = true.into();
        }
        if let Some(h) = self.hi {
            v["hi"] = serde_json::json!(h);
        }
        v
    }
}

fn round5(ll: [f64; 2]) -> [f64; 2] {
    [(ll[0] * 1e5).round() / 1e5, (ll[1] * 1e5).round() / 1e5]
}


/// A peak: its index (handed back), position and tagged elevation (NaN: none).
#[derive(Clone, Copy, Debug)]
pub struct Peak {
    pub i: usize,
    pub lon: f64,
    pub lat: f64,
    pub ele: f32,
}

/// The z8 tiles of an archive, despiked, for the coarse stage.
pub fn coarse_tiles(arc: &Archive) -> HashMap<(u32, u32), Tile> {
    let mut z8: HashMap<(u32, u32), Tile> = HashMap::new();
    for e in arc.entries() {
        let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
        if z == COARSE_Z {
            if let Some(b) = arc.get(z, x, y) {
                if let Ok(mut t) = decode_terrain_png(b) {
                    despike(&mut t, z, tile_lat(z, y));
                    z8.insert((x, y), Arc::new(t));
                }
            }
        }
    }
    z8
}

/// Prominence and isolation of `peaks` (every one of them also a summit for the others): z12 from
/// `z12`, z8 from `z8`. Results in the peaks' order of index, rounded as peaks.json has them.
pub fn run(peaks: &[Peak], z12: &dyn Tiles, z8: &HashMap<(u32, u32), Tile>) -> anyhow::Result<Vec<Out>> {
    let t0 = std::time::Instant::now();
    let mut peaks: Vec<Peak> = peaks.to_vec();
    // Spatial order, so each worker's tile cache is reused.
    peaks.sort_by_key(|p| {
        let (x, y) = merc(p.lon, p.lat);
        ((y * 4096.0) as u64) << 32 | (x * 4096.0) as u64
    });
    // Stage 0 (z12): summits.
    struct Summit {
        i: usize,
        lon: f64,
        lat: f64,
        ele: f32,
        px: (i64, i64),
        s: (i64, i64),
        dem: f32,
    }
    let mut summits: Vec<Summit> = peaks
        .par_chunks(256)
        .flat_map_iter(|chunk| {
            let mut d = Dem::new(z12, FINE_Z, 96);
            chunk
                .iter()
                .map(|&Peak { i, lon, lat, ele }| {
                    let (px, py) = d.px(lon, lat);
                    let r = (150.0 / (dist_m(lon, lat, lon + 0.001, lat) / 0.001 * 360.0 / ((1u64 << FINE_Z) as f64 * 256.0))).ceil() as i64;
                    let (mut sx, mut sy, mut e) = (px, py, f32::MIN);
                    for dy in -r..=r {
                        for dx in -r..=r {
                            if dx * dx + dy * dy <= r * r {
                                if let Some(v) = d.at(px + dx, py + dy) {
                                    if v > e {
                                        (sx, sy, e) = (px + dx, py + dy, v);
                                    }
                                }
                            }
                        }
                    }
                    Summit { i, lon, lat, ele, px: (px, py), s: (sx, sy), dem: e }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    // One peak per summit pixel: the one tagged highest (then the nearest); the others start from
    // their own point.
    let mut by_px: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (k, s) in summits.iter().enumerate() {
        by_px.entry(s.s).or_default().push(k);
    }
    let mut d0 = Dem::new(z12, FINE_Z, 256);
    let mut n_moved = 0;
    for (_, ks) in by_px.into_iter().filter(|(_, v)| v.len() > 1) {
        let score = |s: &Summit| {
            let dd = ((s.px.0 - s.s.0).pow(2) + (s.px.1 - s.s.1).pow(2)) as f32;
            (if s.ele.is_finite() { s.ele } else { f32::MIN / 2.0 }, -dd)
        };
        let win = *ks.iter().max_by(|&&a, &&b| score(&summits[a]).partial_cmp(&score(&summits[b])).unwrap()).unwrap();
        for k in ks {
            if k != win {
                let s = &mut summits[k];
                s.s = s.px;
                s.dem = d0.at(s.px.0, s.px.1).unwrap_or(f32::MIN);
                n_moved += 1;
            }
        }
    }
    drop(d0);
    let e_eff = |sm: &Summit| -> f32 {
        if sm.dem > f32::MIN && sm.ele.is_finite() && sm.ele >= sm.dem - 30.0 && sm.ele <= sm.dem + 200.0 { sm.ele.max(sm.dem) } else { sm.dem }
    };
    let (mut ov12, mut ov8) = (Overlay::default(), Overlay::default());
    for sm in &summits {
        let e = e_eff(sm);
        if e > f32::MIN {
            ov12.add(sm.s.0, sm.s.1, e);
            ov8.add(sm.s.0 >> (FINE_Z - COARSE_Z), sm.s.1 >> (FINE_Z - COARSE_Z), e);
        }
    }
    ov12.index();
    ov8.index();
    eprintln!("summits: {n_moved} peaks shared a summit pixel with a higher-tagged peak ({:.0?})", t0.elapsed());
    summits.sort_by_key(|s| {
        let (x, y) = merc(s.lon, s.lat);
        ((y * 4096.0) as u64) << 32 | (x * 4096.0) as u64
    });

    // Stage 1 (z12): local flood, local isolation.
    let done = AtomicUsize::new(0);
    let n = summits.len();
    struct Fine {
        out: Out,
        sum: (f64, f64),
        flood_done: bool,
        iso_done: bool,
    }
    let fine: Vec<Fine> = summits
        .par_chunks(256)
        .flat_map_iter(|chunk| {
            let mut d = Dem::new(z12, FINE_Z, 96);
            d.ov = Some(&ov12);
            let mut maxes = HashMap::new();
            let res: Vec<Fine> = chunk
                .iter()
                .map(|sm| {
                    let (i, lon, lat) = (sm.i, sm.lon, sm.lat);
                    let (sx, sy) = sm.s;
                    let e = e_eff(sm);
                    let mut out = Out { i, e: e.max(0.0), p: 0.0, pl: false, c: [lon, lat], ce: 0.0, iso: 0.0, il: false, hi: None };
                    let mut flood_done = false;
                    if e > f32::MIN {
                        if let Ok((col, at, reached)) = flood(&mut d, sx, sy, e, FINE_MAX) {
                            out.p = e - col;
                            out.pl = !reached;
                            out.c = round5(d.lonlat(at.0, at.1));
                            out.ce = col;
                            flood_done = true;
                        }
                    } else {
                        flood_done = true;
                    }
                    let mut iso_done = false;
                    if e > f32::MIN {
                        if let Some((km, at)) = nearest_higher(&mut d, sx, sy, e, ISO_FINE_KM, &mut maxes) {
                            out.iso = km;
                            out.hi = Some(round5(d.lonlat(at.0, at.1)));
                            iso_done = true;
                        }
                    } else {
                        iso_done = true;
                    }
                    let s = d.lonlat(sx, sy);
                    let k = done.fetch_add(1, Ordering::Relaxed);
                    if k % 10_000 == 0 {
                        eprintln!("  fine {k}/{n} ({:.0?})", t0.elapsed());
                    }
                    Fine { out, sum: (s[0], s[1]), flood_done, iso_done }
                })
                .collect();
            res
        })
        .collect();
    let need_flood = fine.iter().filter(|f| !f.flood_done).count();
    let need_iso = fine.iter().filter(|f| !f.iso_done).count();
    eprintln!("fine stage done ({:.0?}): {need_flood} peaks need the coarse flood, {need_iso} the coarse isolation search", t0.elapsed());

    let done2 = AtomicUsize::new(0);
    let total2 = fine.iter().filter(|f| !f.flood_done || !f.iso_done).count();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(6).build()?;
    let outs: Vec<Out> = pool.install(|| {
        fine.into_par_iter()
            .map(|f| {
                if f.flood_done && f.iso_done {
                    return f.out;
                }
                let mut out = f.out;
                let mut d = Dem::new(z12, COARSE_Z, 0);
                d.shared = Some(z8);
                d.ov = Some(&ov8);
                let (sx, sy) = d.px(f.sum.0, f.sum.1);
                // The z8 pixel is an average: the summit keeps its fine height.
                let e = out.e;
                if !f.flood_done {
                    match flood(&mut d, sx, sy, e, COARSE_MAX) {
                        Ok((col, at, reached)) => {
                            out.p = e - col;
                            out.pl = !reached;
                            out.c = round5(d.lonlat(at.0, at.1));
                            out.ce = col;
                        }
                        // Budget spent (the flood covers a continent): at least down to here.
                        Err((low, at)) => {
                            out.p = e - low;
                            out.pl = true;
                            out.c = round5(d.lonlat(at.0, at.1));
                            out.ce = low;
                        }
                    }
                }
                if !f.iso_done {
                    let mut maxes = HashMap::new();
                    match nearest_higher(&mut d, sx, sy, e, 5000.0, &mut maxes) {
                        Some((km, at)) => {
                            out.iso = km.max(ISO_FINE_KM);
                            out.hi = Some(round5(d.lonlat(at.0, at.1)));
                        }
                        None => {
                            out.iso = ISO_FINE_KM;
                            out.il = true;
                        }
                    }
                }
                let k = done2.fetch_add(1, Ordering::Relaxed);
                if k % 1000 == 0 {
                    eprintln!("  coarse {k}/{total2} ({:.0?})", t0.elapsed());
                }
                out
            })
            .collect()
    });
    let mut outs = outs;
    outs.sort_by_key(|o| o.i);
    for o in &mut outs {
        o.e = o.e.round();
        o.p = o.p.max(0.0).round();
        o.ce = o.ce.round();
        o.iso = (o.iso * 100.0).round() / 100.0;
    }
    Ok(outs)
}
