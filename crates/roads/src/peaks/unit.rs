//! Prominence and isolation for a z6 tile's peaks (docs/phase5.md "peaks"), so that a peak's result
//! doesn't depend on which z6 tile computes it (the coverage decides which z12 tiles are the packs'):
//!
//! - z12 is the terrain hi pack's tile (`UnitZ12`); where the packs have none, nothing is built and
//!   there are no heights (0 m, as the open sea). It is the same whichever z6 tile reads it.
//! - The summits near a peak count at their own heights, each worked out from z12 the same way
//!   whichever peak asks: its summit pixel (highest within 150 m), its claim of that pixel (several
//!   on one: the highest tagged, then the nearest to it, then the lowest OSM id), its height.
//! - The fine stage reads nothing farther than 28 km from the summit: a flood that would goes to
//!   the coarse stage; the 25 km isolation search counts and opens only what lies within 25 km.
//! - The coarse stage reads the terrain lo packs' z8 (`z8::Z8`; 0 m where nothing is built), with the summits near the peak at
//!   their z12 heights and the others at their tagged ones where plausible (`summits::z8_height`).
//! - Distances are great-circle; the isolation searches visit tiles in order of an exact
//!   great-circle lower bound, x wrapping at the antimeridian; nothing higher within 5,000 km is a
//!   lower bound of 5,000 km.

use super::z8::Z8;
use super::{despike, round5, tile_lat, Out, Overlay, COARSE_MAX, FINE_MAX, ISO_FINE_KM, TS};
use crate::summits::Summit;
use anyhow::{Context, Result};
use det::Det;
use rayon::prelude::*;
use roadcore::geo::{merc, EARTH_R};
use roadcore::grid::decode_terrain;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// (The searches' pixel sets and the overlays' look-ups: roadcore::fx.)
use roadcore::fx::{FxMap, FxSet};

const Z12: u8 = 12;
const Z8_: u8 = 8;
/// The fine stage's reach from the summit, km.
pub const FINE_REACH_KM: f64 = 28.0;
/// The coarse isolation search's reach, km.
pub const COARSE_ISO_KM: f64 = 5000.0;

/// Great-circle distance, metres.
pub fn gc_m(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let k = std::f64::consts::PI / 180.0;
    let (p1, p2) = (lat1 * k, lat2 * k);
    let (dp, dl) = (p2 - p1, (lon2 - lon1) * k);
    let a = (dp / 2.0).dsin().powi(2) + p1.dcos() * p2.dcos() * (dl / 2.0).dsin().powi(2);
    2.0 * EARTH_R * a.sqrt().min(1.0).dasin()
}

/// The great-circle distance (m) from (lon, lat) to the nearest point of tile z/x/y.
pub fn tile_lower_bound_m(lon: f64, lat: f64, z: u8, tx: u32, ty: u32) -> f64 {
    let n = (1u64 << z) as f64;
    let (lon0, lon1) = (roadcore::geo::merc_lon(tx as f64 / n), roadcore::geo::merc_lon((tx + 1) as f64 / n));
    let lat_of = |t: f64| roadcore::geo::merc_lat(t / n);
    let (lat_n, lat_s) = (lat_of(ty as f64), lat_of(ty as f64 + 1.0));
    // Longitude offset from the tile's west edge, wrapped into [0, 360).
    let off = (lon - lon0).rem_euclid(360.0);
    let inside_lon = off <= lon1 - lon0;
    if inside_lon && lat >= lat_s && lat <= lat_n {
        return 0.0;
    }
    let mut best = f64::MAX;
    // The parallels: at the longitude nearest the point's.
    let near_lon = if inside_lon {
        lon
    } else if off - (lon1 - lon0) < 360.0 - off {
        lon1
    } else {
        lon0
    };
    for pl in [lat_s, lat_n] {
        best = best.min(gc_m(lon, lat, near_lon, pl));
    }
    // The meridians: the foot of the perpendicular, clamped to the edge.
    let k = std::f64::consts::PI / 180.0;
    for me in [lon0, lon1] {
        let (a, b) = ((lat * k).dsin(), (lat * k).dcos() * ((lon - me) * k).dcos());
        let f = a.datan2(b).clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2) / k;
        best = best.min(gc_m(lon, lat, me, f.clamp(lat_s, lat_n)));
    }
    best
}

/// A z6 tile's z12 tiles, read before the run: each is the terrain hi pack's where the manifest has
/// it; where it hasn't, nothing is built, no heights (0 m, as the open sea). A tile asked for that
/// wasn't read before is counted (`unexpected`), and fails the run.
pub struct UnitZ12 {
    tiles: HashMap<(u32, u32), Option<Arc<Vec<u8>>>>,
    unexpected: AtomicUsize,
    /// How many came from the packs, how many have none.
    pub from: (usize, usize),
}

impl UnitZ12 {
    pub fn load(out: &buildstore::out::Out, want: &BTreeSet<(u32, u32)>) -> Result<UnitZ12> {
        Self::load_from(&terrain::terrain_pack::ManifestTiles::new(out, "terrain"), want)
    }

    /// `load`, from the terrain packs `packs` reads (a job's several z6 tiles', each pack opened once).
    pub fn load_from(packs: &terrain::terrain_pack::ManifestTiles, want: &BTreeSet<(u32, u32)>) -> Result<UnitZ12> {
        // (In one phase, its tiles' bytes counted.)
        let p = jobrt::timings::phase("loading z12 terrain tiles from the packs");
        // (The packs' tiles read in spans, a pack's at once: `get_many`.)
        let want: Vec<(u32, u32)> = want.iter().copied().collect();
        let from_packs = packs.get_many(&want.iter().map(|&(x, y)| (Z12, x, y)).collect::<Vec<_>>())?;
        let mut tiles = HashMap::with_capacity(want.len());
        let mut from = (0, 0);
        for (&(x, y), b) in want.iter().zip(from_packs) {
            match b {
                Some(b) => {
                    p.count(b.len() as u64, 1);
                    anyhow::ensure!(decode_terrain(&b).is_ok(), "the terrain pack's z12 {x}/{y} doesn't decode");
                    from.0 += 1;
                    tiles.insert((x, y), Some(Arc::new(b)));
                }
                None => {
                    from.1 += 1;
                    tiles.insert((x, y), None);
                }
            }
        }
        Ok(UnitZ12 { tiles, unexpected: AtomicUsize::new(0), from })
    }

    /// For tests: tiles given (None: sea).
    pub fn from_tiles(tiles: HashMap<(u32, u32), Option<Vec<u8>>>) -> UnitZ12 {
        UnitZ12 {
            tiles: tiles.into_iter().map(|(k, v)| (k, v.map(Arc::new))).collect(),
            unexpected: AtomicUsize::new(0),
            from: (0, 0),
        }
    }

    /// A tile's elevations, despiked (zeros for the sea); None for one not read before (or not
    /// decoding, which `load` rules out), counted.
    fn decoded(&self, x: u32, y: u32) -> Option<Vec<f32>> {
        match self.tiles.get(&(x, y)) {
            Some(Some(png)) => match decode_terrain(png) {
                Ok(mut e) => {
                    despike(&mut e, Z12, tile_lat(Z12, y));
                    Some(e)
                }
                Err(_) => {
                    self.unexpected.fetch_add(1, Ordering::Relaxed);
                    None
                }
            },
            Some(None) => Some(vec![0f32; 256 * 256]),
            None => {
                self.unexpected.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn unexpected(&self) -> usize {
        self.unexpected.load(Ordering::Relaxed)
    }
}

/// Each of `items` loaded (`load`) and handed to `work` in order, the next one's load under way on a
/// thread of its own while `work` takes the one before: a job's next z6 tile's z12 tiles read from the
/// NAS while this one's peaks are measured (task #399: the reads were most of a run, at the link's
/// speed already). The first is loaded on this thread. `work` gets what `load` made either way, so
/// the results are those of loading each in turn; a failed load fails when its turn comes, after
/// the ones before it were done. Two items' loads are held at once at most.
pub fn read_ahead<I: Sync, T: Send>(
    items: &[I],
    load: impl Fn(&I) -> Result<T> + Sync,
    mut work: impl FnMut(usize, &I, T) -> Result<()>,
) -> Result<()> {
    std::thread::scope(|s| {
        let load = &load;
        let mut ahead: Option<std::thread::ScopedJoinHandle<Result<T>>> = None;
        for (k, it) in items.iter().enumerate() {
            let t = match ahead.take() {
                Some(h) => {
                    let _p = jobrt::timings::phase("waiting for the next z6 tile's z12 tiles");
                    h.join().map_err(|_| anyhow::anyhow!("the read ahead panicked"))??
                }
                None => load(it)?,
            };
            if let Some(n) = items.get(k + 1) {
                ahead = Some(s.spawn(move || load(n)));
            }
            work(k, it, t)?;
        }
        Ok(())
    })
}

/// A box grown by `PAD` degrees on every side: a disc's box (roadcore::geo::grow) taking in
/// what the tiles' and cells' edges round off.
fn pad(b: [f64; 4]) -> [f64; 4] {
    [b[0] - PAD, b[1] - PAD, b[2] + PAD, b[3] + PAD]
}
const PAD: f64 = 0.01;

/// The z12 tiles a z6 tile's peaks may read: those meeting a disc of `km` around each, padded.
pub fn tiles_wanted(points: &[(f64, f64)], km: f64) -> BTreeSet<(u32, u32)> {
    let n = 1u32 << Z12;
    let mut want = BTreeSet::new();
    for &(lon, lat) in points {
        let g = pad(roadcore::geo::grow([lon, lat, lon, lat], km));
        let t = |lon: f64, lat: f64| {
            let (x, y) = roadcore::geo::merc_clamped(lon, lat);
            ((x * n as f64).floor() as i64, (y * n as f64).floor().clamp(0.0, (n - 1) as f64) as i64)
        };
        let (x0, y0) = t(g[0], g[3]);
        let (x1, y1) = t(g[2], g[1]);
        for x in x0..=x1 {
            for y in y0..=y1 {
                want.insert((x.rem_euclid(n as i64) as u32, y as u32));
            }
        }
    }
    want
}

/// The overlays the z6 tile's searches read: at z12 a peak's nearby summits; at z8 every summit's
/// tagged height where plausible, and the nearby ones' z12 heights instead.
trait Ov: Sync {
    fn at(&self, p: (i64, i64)) -> Option<f32>;
    /// A tile's overlay pixels with their heights, in pixel order.
    fn tile_pixels(&self, t: (i64, i64)) -> Vec<(i64, i64, f32)>;
}

impl Ov for Overlay {
    fn at(&self, p: (i64, i64)) -> Option<f32> {
        self.px.get(&p).copied()
    }
    fn tile_pixels(&self, t: (i64, i64)) -> Vec<(i64, i64, f32)> {
        self.tiles.get(&t).cloned().unwrap_or_default()
    }
}

/// Every summit's tagged z8 height where plausible (crate::summits::z8_height), by z8 tile and
/// pixel (summit index, height): each tile's worked out from the z8 on its first read and kept, so
/// a run works out only the summits in the tiles it reads (those its record names with the tiles
/// behind them, `z8_behind`), from the z8 it reads (task #1556: not a worldwide summits step's,
/// which read every lo pack).
pub struct Z8Base<'a> {
    z8: &'a Z8,
    tiles: FxMap<(i64, i64), Tagged>,
    /// A z8 read that failed (the lo packs' local copies): fails the run.
    failed: std::sync::Mutex<Option<String>>,
}

/// A z8 tile's tagged summits: (summit index, tagged height) by pixel; their z8 heights by pixel,
/// once worked out (`Z8Base::heights`).
struct Tagged {
    px: Vec<((i64, i64), Vec<(u32, f32)>)>,
    heights: std::sync::OnceLock<FxMap<(i64, i64), Vec<(u32, f32)>>>,
}

impl<'a> Z8Base<'a> {
    pub fn new(summits: &[Summit], z8: &'a Z8) -> Z8Base<'a> {
        let mut px: FxMap<(i64, i64), Vec<(u32, f32)>> = FxMap::default();
        for (i, s) in summits.iter().enumerate() {
            if let Some(ele) = s.ele {
                px.entry(crate::summits::z8_pixel(s.lon, s.lat)).or_default().push((i as u32, ele));
            }
        }
        let mut tiles: FxMap<(i64, i64), Tagged> = FxMap::default();
        for (p, v) in px {
            let t = tiles
                .entry((p.0.div_euclid(TS), p.1.div_euclid(TS)))
                .or_insert_with(|| Tagged { px: Vec::new(), heights: std::sync::OnceLock::new() });
            t.px.push((p, v));
        }
        Z8Base { z8, tiles, failed: std::sync::Mutex::new(None) }
    }

    /// Tile `t`'s summits' z8 heights by pixel, those plausible; None for a tile with none tagged.
    fn heights(&self, t: (i64, i64)) -> Option<&FxMap<(i64, i64), Vec<(u32, f32)>>> {
        let tagged = self.tiles.get(&t)?;
        Some(tagged.heights.get_or_init(|| {
            let mut out: FxMap<(i64, i64), Vec<(u32, f32)>> = FxMap::default();
            for (p, v) in &tagged.px {
                let max3 = match crate::summits::z8_max_around(*p, self.z8) {
                    Ok(m) => m,
                    Err(e) => {
                        self.failed.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| format!("{e:#}"));
                        continue;
                    }
                };
                let h: Vec<(u32, f32)> =
                    v.iter().filter_map(|&(i, ele)| crate::summits::z8_height(ele, max3).map(|h| (i, h))).collect();
                if !h.is_empty() {
                    out.insert(*p, h);
                }
            }
            out
        }))
    }

    /// A z8 read that failed while working out heights.
    fn failed(&self) -> Option<String> {
        self.failed.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// A peak's z8 overlay: the base without the summits near it, which count at their z12 heights.
struct Ov8<'a> {
    base: &'a Z8Base<'a>,
    near: &'a FxSet<u32>,
    over: FxMap<(i64, i64), f32>,
}

impl Ov for Ov8<'_> {
    fn at(&self, p: (i64, i64)) -> Option<f32> {
        let a = self.over.get(&p).copied();
        let b = self
            .base
            .heights((p.0.div_euclid(TS), p.1.div_euclid(TS)))
            .and_then(|h| h.get(&p))
            .and_then(|v| v.iter().filter(|(i, _)| !self.near.contains(i)).map(|x| x.1).reduce(f32::max));
        match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
    fn tile_pixels(&self, t: (i64, i64)) -> Vec<(i64, i64, f32)> {
        let mut ps: Vec<(i64, i64)> = self.base.heights(t).map(|h| h.keys().copied().collect()).unwrap_or_default();
        ps.extend(self.over.keys().filter(|p| (p.0.div_euclid(TS), p.1.div_euclid(TS)) == t));
        ps.sort_unstable_by_key(|p| (p.1, p.0));
        ps.dedup();
        ps.into_iter().filter_map(|p| self.at(p).map(|v| (p.0, p.1, v))).collect()
    }
}

/// One zoom's pixels for the z6 tile's searches: x wraps around the world; beyond the poles nothing.
struct UDem<'a> {
    z: u8,
    z12: Option<&'a UnitZ12>,
    z8: Option<&'a Z8>,
    cache: HashMap<(u32, u32), Option<Arc<Vec<f32>>>>,
    max_cache: usize,
    /// A z8 read that failed (the lo packs' local copies): fails the run.
    failed: Option<String>,
    /// The z8 tiles read (`run`'s record).
    read: BTreeSet<(u32, u32)>,
}

impl<'a> UDem<'a> {
    fn w(&self) -> i64 {
        TS << self.z
    }
    fn tile(&mut self, tx: i64, ty: i64) -> Option<Arc<Vec<f32>>> {
        let n = 1i64 << self.z;
        if ty < 0 || ty >= n {
            return None;
        }
        let k = (tx.rem_euclid(n) as u32, ty as u32);
        if let Some(z8) = self.z8 {
            self.read.insert(k);
            return match z8.tile(k.0, k.1) {
                Ok(t) => Some(t),
                Err(e) => {
                    self.failed.get_or_insert_with(|| format!("{e:#}"));
                    None
                }
            };
        }
        if let Some(t) = self.cache.get(&k) {
            return t.clone();
        }
        if self.cache.len() >= self.max_cache {
            self.cache.clear();
        }
        let t = self.z12.and_then(|s| s.decoded(k.0, k.1)).map(Arc::new);
        self.cache.insert(k, t.clone());
        t
    }
    fn at(&mut self, ov: Option<&dyn Ov>, gx: i64, gy: i64) -> Option<f32> {
        let gx = gx.rem_euclid(self.w());
        let t = self.tile(gx.div_euclid(TS), gy.div_euclid(TS))?;
        let v = t[(gy.rem_euclid(TS) * TS + gx.rem_euclid(TS)) as usize].max(0.0);
        Some(match ov.and_then(|o| o.at((gx, gy))) {
            Some(s) => v.max(s),
            None => v,
        })
    }
    fn px(&self, lon: f64, lat: f64) -> (i64, i64) {
        let (x, y) = merc(lon, lat);
        let w = self.w() as f64;
        (((x * w).floor() as i64).rem_euclid(self.w()), (y * w).floor() as i64)
    }
    fn lonlat(&self, gx: i64, gy: i64) -> [f64; 2] {
        lonlat_at(self.w(), gx, gy)
    }
    /// Pixels between two (x wrapping), squared.
    fn d2(&self, a: (i64, i64), b: (i64, i64)) -> i64 {
        let dx = (a.0 - b.0).rem_euclid(self.w());
        let dx = dx.min(self.w() - dx);
        dx * dx + (a.1 - b.1).pow(2)
    }
}

/// A pixel's centre, `w` pixels around the world.
fn lonlat_at(w: i64, gx: i64, gy: i64) -> [f64; 2] {
    let wf = w as f64;
    let (x, y) = ((gx.rem_euclid(w) as f64 + 0.5) / wf, (gy as f64 + 0.5) / wf);
    let lat = roadcore::geo::merc_lat(y);
    [x * 360.0 - 180.0, lat]
}

enum Flood {
    /// (col height, col pixel, reached higher ground or the sea)
    Done(f32, (i64, i64), bool),
    /// The pixel budget ran out: the lowest point so far, a lower bound.
    Budget(f32, (i64, i64)),
    /// It would have read past the reach.
    Beyond,
}

/// The priority flood (peaks.rs `flood`), x wrapping, and stopping at `reach` (pixels from
/// the summit, squared) when given.
fn flood(d: &mut UDem, ov: Option<&dyn Ov>, sx: i64, sy: i64, e: f32, budget: usize, reach: Option<i64>) -> Flood {
    let key = |v: f32| (v * 100.0).round() as i32;
    let mut heap: BinaryHeap<(i32, i64, i64, Reverse<i32>, i64, i64)> = BinaryHeap::new();
    let mut seen: FxSet<(i64, i64)> = FxSet::default();
    heap.push((key(e), sx, sy, Reverse(key(e)), sx, sy));
    seen.insert((sx, sy));
    let mut low = (key(e), (sx, sy));
    let (w, n) = (d.w(), d.w());
    while let Some((k, x, y, Reverse(pm), pmx, pmy)) = heap.pop() {
        if k as f32 / 100.0 > e + 0.5 {
            return Flood::Done(pm as f32 / 100.0, (pmx, pmy), true);
        }
        if k < low.0 {
            low = (k, (x, y));
        }
        if pm <= 0 {
            return Flood::Done(0.0, (pmx, pmy), true);
        }
        if seen.len() > budget {
            return Flood::Budget(low.0 as f32 / 100.0, low.1);
        }
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (nx, ny) = ((x + dx).rem_euclid(w), y + dy);
            if ny < 0 || ny >= n {
                continue;
            }
            if !seen.insert((nx, ny)) {
                continue;
            }
            if let Some(r2) = reach {
                if d.d2((nx, ny), (sx, sy)) > r2 {
                    return Flood::Beyond;
                }
            }
            if let Some(h) = d.at(ov, nx, ny) {
                let hk = key(h);
                let (m, mx, my) = if hk < pm { (hk, nx, ny) } else { (pm, pmx, pmy) };
                heap.push((hk, nx, ny, Reverse(m), mx, my));
            }
        }
    }
    Flood::Done(low.0 as f32 / 100.0, low.1, false)
}

/// The nearest ground higher than `e` within `max_km` (great-circle, pixel centres): tiles in order
/// of their exact lower bound, x wrapping, a tile scanned only when its highest pixel (the DEM's
/// or the overlay's) is higher. `tile_max`: the DEM's highest in a tile when known without
/// decoding it. Some((km, pixel)).
fn nearest_higher(
    d: &mut UDem,
    ov: Option<&dyn Ov>,
    sx: i64,
    sy: i64,
    e: f32,
    max_km: f64,
    tile_max: &dyn Fn(u32, u32) -> Option<f32>,
) -> Option<(f64, (i64, i64))> {
    let s = d.lonlat(sx, sy);
    let w = d.w();
    let n = 1i64 << d.z;
    let start = (sx.div_euclid(TS), sy.div_euclid(TS));
    let mut heap: BinaryHeap<Reverse<(u64, i64, i64)>> = BinaryHeap::new();
    let mut queued: FxSet<(i64, i64)> = FxSet::default();
    heap.push(Reverse((0, start.0, start.1)));
    queued.insert(start);
    let mut best: Option<(f64, (i64, i64))> = None;
    while let Some(Reverse((lb_mm, tx, ty))) = heap.pop() {
        let lb_km = lb_mm as f64 / 1e6;
        if lb_km > max_km || best.is_some_and(|(b, _)| lb_km > b) {
            break;
        }
        let ovp = ov.map(|o| o.tile_pixels((tx, ty))).unwrap_or_default();
        let dem_max = match tile_max(tx as u32, ty as u32) {
            Some(m) => Some(m),
            None => d.tile(tx, ty).map(|t| t.iter().cloned().fold(0f32, f32::max)),
        };
        let mx = dem_max.unwrap_or(f32::MIN).max(ovp.iter().map(|p| p.2).fold(f32::MIN, f32::max));
        if mx > e + 0.5 {
            let consider = |x: i64, y: i64, best: &mut Option<(f64, (i64, i64))>| {
                let ll = lonlat_at(w, x, y);
                let km = gc_m(s[0], s[1], ll[0], ll[1]) / 1000.0;
                if km <= max_km && best.is_none_or(|(b, _)| km < b) {
                    *best = Some((km, (x, y)));
                }
            };
            for &(x, y, v) in &ovp {
                if v > e + 0.5 {
                    consider(x, y, &mut best);
                }
            }
            if dem_max.is_some_and(|m| m > e + 0.5) {
                if let Some(t) = d.tile(tx, ty) {
                    for j in 0..TS {
                        for i in 0..TS {
                            if t[(j * TS + i) as usize] > e + 0.5 {
                                consider(tx * TS + i, ty * TS + j, &mut best);
                            }
                        }
                    }
                }
            }
        }
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (nx, ny) = ((tx + dx).rem_euclid(n), ty + dy);
            if ny < 0 || ny >= n || !queued.insert((nx, ny)) {
                continue;
            }
            let lb = tile_lower_bound_m(s[0], s[1], d.z, nx as u32, ny as u32);
            if lb / 1000.0 <= max_km {
                heap.push(Reverse(((lb * 1000.0) as u64, nx, ny)));
            }
        }
    }
    best
}

/// A peak of the z6 tile: its key (handed back), its summit's id in `summits`, position, tagged height.
#[derive(Clone, Debug)]
pub struct UnitPeak {
    pub key: String,
    pub id: String,
    pub lon: i32,
    pub lat: i32,
    pub ele: Option<f32>,
}

/// A summit as the z6 tile's peaks see it (from z12).
#[derive(Clone, Copy, Debug)]
struct Seen {
    /// Its summit pixel after the claims, and its height there.
    sp: (i64, i64),
    e: f32,
}

/// The peaks of a z6 tile measured a piece at a time (`run`; a multiple of the fine stage's 64).
const BATCH: usize = 512;

/// A batch's results as a kept piece's payload (jobrt::resume_log::Put), with the z8 tiles its
/// coarse stage read.
fn put_outs(coarse: usize, outs: &[(usize, Out)], read: &BTreeSet<(u32, u32)>) -> Vec<u8> {
    let mut p = jobrt::resume_log::Put::default();
    let f64s = |p: &mut jobrt::resume_log::Put, v: &[f64]| v.iter().for_each(|x| p.u64(x.to_bits()));
    p.u64(coarse as u64);
    p.u64(outs.len() as u64);
    for (i, o) in outs {
        p.u64(*i as u64);
        p.u64(o.i as u64);
        p.u32(o.e.to_bits());
        p.u32(o.p.to_bits());
        p.u32(o.ce.to_bits());
        f64s(&mut p, &o.c);
        f64s(&mut p, &[o.iso]);
        p.u8(u8::from(o.pl) | u8::from(o.il) << 1 | u8::from(o.hi.is_some()) << 2);
        if let Some(h) = o.hi {
            f64s(&mut p, &h);
        }
    }
    p.u64(read.len() as u64);
    for &(x, y) in read {
        p.u32(x);
        p.u32(y);
    }
    p.0
}

/// `put_outs`'s count of peaks that needed the coarse stage, results and z8 tiles read, read back.
fn get_outs(p: &[u8]) -> Result<(usize, Vec<(usize, Out)>, BTreeSet<(u32, u32)>)> {
    let mut g = jobrt::resume_log::Get(p);
    let f64 = |g: &mut jobrt::resume_log::Get| g.u64().map(f64::from_bits);
    let coarse = usize::try_from(g.u64()?)?;
    let mut v = Vec::new();
    for _ in 0..g.count(53)? {
        let at = usize::try_from(g.u64()?)?;
        let i = usize::try_from(g.u64()?)?;
        let (e, pr, ce) = (f32::from_bits(g.u32()?), f32::from_bits(g.u32()?), f32::from_bits(g.u32()?));
        let c = [f64(&mut g)?, f64(&mut g)?];
        let iso = f64(&mut g)?;
        let f = g.u8()?;
        let hi = if f & 4 != 0 { Some([f64(&mut g)?, f64(&mut g)?]) } else { None };
        v.push((at, Out { i, e, p: pr, pl: f & 1 != 0, c, ce, iso, il: f & 2 != 0, hi }));
    }
    let mut read = BTreeSet::new();
    for _ in 0..g.count(8)? {
        read.insert((g.u32()?, g.u32()?));
    }
    g.end()?;
    Ok((coarse, v, read))
}

/// The z8 tiles behind what the coarse stage read of the summits (task #1545): its overlay reads
/// the summits whose z8 pixel is in a tile it read (`Ov8`, by pixel and by tile), at their z8
/// heights, and each of those heights reads the tiles around its pixel (crate::summits::z8_tiles_read).
fn z8_behind(summits: &[Summit], read: &BTreeSet<(u32, u32)>) -> Vec<(u32, u32)> {
    summits
        .iter()
        .filter(|s| {
            let (x, y) = crate::summits::z8_pixel(s.lon, s.lat);
            read.contains(&(x.div_euclid(TS) as u32, y.div_euclid(TS) as u32))
        })
        .flat_map(crate::summits::z8_tiles_read)
        .collect()
}

/// Peaks of a z6 tile: (key, result) in the peaks' order, and the z8 tiles its coarse stage read
/// with those the z8 heights of the summits it read there read (crate::summits::z8_tiles_read;
/// pipeline's agent::idents::peaks_lo: what its key's lo line answers); the summits as they see
/// them, then each batch of them (`BATCH`), kept in `pieces` as made.
pub fn run(
    peaks: &[UnitPeak],
    summits: &[Summit],
    base8: &Z8Base<'_>,
    z12: &UnitZ12,
    z8: &Z8,
    coarse_threads: usize,
    pieces: &mut jobrt::resume_log::Pieces,
) -> Result<(Vec<(String, Out)>, BTreeSet<(u32, u32)>)> {
    let t0 = std::time::Instant::now();
    let by_id: HashMap<&str, usize> = summits.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();
    // The summits near the z6 tile's peaks, by 0.1° cell.
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, s) in summits.iter().enumerate() {
        grid.entry((s.lon.div_euclid(1_000_000), s.lat.div_euclid(1_000_000))).or_default().push(i as u32);
    }
    let within = |lon: i32, lat: i32, km: f64| -> Vec<u32> {
        let (la, lo) = (lat as f64 * 1e-7, lon as f64 * 1e-7);
        let g = pad(roadcore::geo::grow([lo, la, lo, la], km));
        let c = |v: f64| (v * 10.0).floor() as i32;
        let mut out = Vec::new();
        for cx in c(g[0])..=c(g[2]) {
            for cy in c(g[1])..=c(g[3]) {
                let cxw = (cx + 1800).rem_euclid(3600) - 1800;
                for &i in grid.get(&(cxw, cy)).map(Vec::as_slice).unwrap_or(&[]) {
                    let s = &summits[i as usize];
                    if gc_m(lo, la, s.lon as f64 * 1e-7, s.lat as f64 * 1e-7) <= km * 1000.0 {
                        out.push(i);
                    }
                }
            }
        }
        out.sort_unstable();
        out
    };
    // A z12 pixel is at most ~38 m (the equator): the reach for the summits that can matter (their
    // 150 m search, twice) and for the claims. A summit's summit pixel is within r + 0.71 pixels
    // of it (r = ⌈150 m / pixel⌉), so two sharing one are under 300 m + 3.42 pixels apart.
    let px_km = 0.0382;
    let near_km = FINE_REACH_KM + 2.0 * (0.150 + 2.0 * px_km);
    let claim_km = near_km + 0.300 + 3.5 * px_km;
    let mut need: BTreeSet<u32> = BTreeSet::new();
    let mut extra: Vec<Summit> = Vec::new();
    for p in peaks {
        need.extend(within(p.lon, p.lat, claim_km));
        if !by_id.contains_key(p.id.as_str()) {
            extra.push(Summit { id: p.id.clone(), kind: "peak".into(), lon: p.lon, lat: p.lat, ele: p.ele });
        }
    }
    // (The summits as the peaks see them a piece kept, task #889: each its id, its summit pixel and
    // height; then each batch of peaks measured.)
    let extra_base = summits.len() as u32;
    let kept = pieces.piece((0, 0, 0), || {
        // Every summit's pixel and the highest within 150 m, from z12.
        let mut all: Vec<(u32, &Summit)> = need.iter().map(|&i| (i, &summits[i as usize])).collect();
        all.extend(extra.iter().enumerate().map(|(k, s)| (extra_base + k as u32, s)));
        let seen: Vec<(u32, (i64, i64), (i64, i64), f32, f32)> = all
            .par_chunks(256)
            .flat_map_iter(|chunk| {
                let mut d = UDem {
                    z: Z12,
                    z12: Some(z12),
                    z8: None,
                    cache: HashMap::new(),
                    max_cache: 96,
                    failed: None,
                    read: BTreeSet::new(),
                };
                chunk
                    .iter()
                    .map(|&(i, s)| {
                        let (lon, lat) = (s.lon as f64 * 1e-7, s.lat as f64 * 1e-7);
                        let (px, py) = d.px(lon, lat);
                        let r = (150.0
                            / (gc_m(lon, lat, lon + 0.001, lat) / 0.001 * 360.0 / ((1u64 << Z12) as f64 * 256.0)))
                            .ceil() as i64;
                        let (mut sx, mut sy, mut e) = (px, py, f32::MIN);
                        for dy in -r..=r {
                            for dx in -r..=r {
                                if dx * dx + dy * dy <= r * r {
                                    if let Some(v) = d.at(None, px + dx, py + dy) {
                                        if v > e {
                                            (sx, sy, e) = ((px + dx).rem_euclid(d.w()), py + dy, v);
                                        }
                                    }
                                }
                            }
                        }
                        (i, (px, py), (sx, sy), e, s.ele.unwrap_or(f32::NAN))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        // The claims: one summit per summit pixel, the highest tagged, then the nearest, then the lowest
        // OSM id; the others start from their own point.
        let summit_of = |i: u32| if i < extra_base { &summits[i as usize] } else { &extra[(i - extra_base) as usize] };
        let mut by_px: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (k, s) in seen.iter().enumerate() {
            by_px.entry(s.2).or_default().push(k);
        }
        let mut info: HashMap<u32, Seen> = HashMap::with_capacity(seen.len());
        let mut d0 = UDem {
            z: Z12,
            z12: Some(z12),
            z8: None,
            cache: HashMap::new(),
            max_cache: 256,
            failed: None,
            read: BTreeSet::new(),
        };
        let mut moved = 0;
        let mut groups: Vec<(&(i64, i64), &Vec<usize>)> = by_px.iter().collect();
        groups.sort_unstable_by_key(|g| *g.0);
        for (_, ks) in groups {
            let win = if ks.len() > 1 {
                let score = |k: usize| {
                    let s = &seen[k];
                    let dd = d0.d2(s.1, s.2) as f32;
                    (if s.4.is_finite() { s.4 } else { f32::MIN / 2.0 }, -dd)
                };
                *ks.iter()
                    .max_by(|&&a, &&b| {
                        score(a)
                            .partial_cmp(&score(b))
                            .unwrap()
                            .then_with(|| summit_of(seen[b].0).order().cmp(&summit_of(seen[a].0).order()))
                    })
                    .unwrap()
            } else {
                ks[0]
            };
            for &k in ks {
                let s = seen[k];
                let (sp, dem) = if k == win {
                    (s.2, s.3)
                } else {
                    moved += 1;
                    (s.1, d0.at(None, s.1 .0, s.1 .1).unwrap_or(f32::MIN))
                };
                let ele = s.4;
                let e = if dem > f32::MIN && ele.is_finite() && ele >= dem - 30.0 && ele <= dem + 200.0 {
                    ele.max(dem)
                } else {
                    dem
                };
                info.insert(s.0, Seen { sp, e });
            }
        }
        drop(d0);
        let mut p = jobrt::resume_log::Put::default();
        p.u64(moved);
        let mut ks: Vec<(&u32, &Seen)> = info.iter().collect();
        ks.sort_unstable_by_key(|k| *k.0);
        p.u64(ks.len() as u64);
        for (k, s) in ks {
            p.u32(*k);
            p.u64(s.sp.0 as u64);
            p.u64(s.sp.1 as u64);
            p.u32(s.e.to_bits());
        }
        Ok(p.0)
    })?;
    let mut info: HashMap<u32, Seen> = HashMap::new();
    let moved;
    {
        let mut g = jobrt::resume_log::Get(&kept);
        moved = g.u64()?;
        for _ in 0..g.count(24)? {
            let k = g.u32()?;
            let sp = (g.u64()? as i64, g.u64()? as i64);
            info.insert(k, Seen { sp, e: f32::from_bits(g.u32()?) });
        }
        g.end()?;
    }
    drop(kept);
    eprintln!(
        "peaks: {} summits near {} peaks, {moved} moved off a shared pixel ({:.0?})",
        info.len(),
        peaks.len(),
        t0.elapsed()
    );

    // The peaks in spatial order (each worker's tiles reused).
    let mut order: Vec<usize> = (0..peaks.len()).collect();
    order.sort_by_key(|&i| {
        let (x, y) = merc(peaks[i].lon as f64 * 1e-7, peaks[i].lat as f64 * 1e-7);
        (((y * 4096.0) as u64) << 32 | (x * 4096.0) as u64, i)
    });
    let peak_summit = |p: &UnitPeak| -> u32 {
        match by_id.get(p.id.as_str()) {
            Some(&i) => i as u32,
            None => extra_base + extra.iter().position(|s| s.id == p.id).unwrap() as u32,
        }
    };
    struct Fine {
        i: usize,
        out: Out,
        near: Vec<u32>,
        flood_done: bool,
        iso_done: bool,
        sum: [f64; 2],
    }
    let done = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(coarse_threads.max(1)).build()?;
    let mut need_coarse = 0;
    let mut outs: Vec<(usize, Out)> = Vec::with_capacity(peaks.len());
    let mut read: BTreeSet<(u32, u32)> = BTreeSet::new();
    // (Each batch of peaks in that order measured, fine then coarse, a piece kept, task #889: a
    // peak's result is its own, so a resumed z6 tile measures only the batches not kept.)
    for (n, batch) in order.chunks(BATCH).enumerate() {
        let kept = pieces.piece((1, n as u32, 0), || {
            let fine: Vec<Fine> = batch
                .par_chunks(64)
                .flat_map_iter(|chunk| {
                    let mut d = UDem {
                        z: Z12,
                        z12: Some(z12),
                        z8: None,
                        cache: HashMap::new(),
                        max_cache: 96,
                        failed: None,
                        read: BTreeSet::new(),
                    };
                    chunk
                        .iter()
                        .map(|&i| {
                            let p = &peaks[i];
                            let me = info[&peak_summit(p)];
                            let near = within(p.lon, p.lat, near_km);
                            let mut ov = Overlay::default();
                            for &k in near.iter().chain(std::iter::once(&peak_summit(p))) {
                                if let Some(s) = info.get(&k) {
                                    if s.e > f32::MIN {
                                        ov.add(s.sp.0, s.sp.1, s.e);
                                    }
                                }
                            }
                            ov.index();
                            let (sx, sy) = me.sp;
                            let e = me.e;
                            let s_ll = d.lonlat(sx, sy);
                            let mut out = Out {
                                i,
                                e: e.max(0.0),
                                p: 0.0,
                                pl: false,
                                c: [p.lon as f64 * 1e-7, p.lat as f64 * 1e-7],
                                ce: 0.0,
                                iso: 0.0,
                                il: false,
                                hi: None,
                            };
                            let (mut flood_done, mut iso_done) = (e <= f32::MIN, e <= f32::MIN);
                            if e > f32::MIN {
                                // The reach in pixels at the summit's latitude, a little short.
                                let px_m = gc_m(s_ll[0], s_ll[1], s_ll[0] + 360.0 / (256.0 * 4096.0), s_ll[1]);
                                let r = (FINE_REACH_KM * 1000.0 / px_m * 0.995) as i64;
                                if let Flood::Done(col, at, reached) =
                                    flood(&mut d, Some(&ov), sx, sy, e, FINE_MAX, Some(r * r))
                                {
                                    out.p = e - col;
                                    out.pl = !reached;
                                    out.c = round5(d.lonlat(at.0, at.1));
                                    out.ce = col;
                                    flood_done = true;
                                }
                                if let Some((km, at)) =
                                    nearest_higher(&mut d, Some(&ov), sx, sy, e, ISO_FINE_KM, &|_, _| None)
                                {
                                    out.iso = km;
                                    out.hi = Some(round5(d.lonlat(at.0, at.1)));
                                    iso_done = true;
                                }
                            }
                            let k = done.fetch_add(1, Ordering::Relaxed);
                            if k % 10_000 == 0 && k > 0 {
                                eprintln!("  fine {k}/{} ({:.0?})", peaks.len(), t0.elapsed());
                            }
                            Fine { i, out, near, flood_done, iso_done, sum: s_ll }
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            let failed_z12 = z12.unexpected();
            anyhow::ensure!(
                failed_z12 == 0,
                "peaks: {failed_z12} z12 tiles were read that weren't loaded (the reach is wrong)"
            );
            let coarse = fine.iter().filter(|f| !f.flood_done || !f.iso_done).count();

            // The coarse stage (z8), a few at a time (a flood holds up to 40M pixels).
            let results: Vec<Result<(usize, Out, Vec<(u32, u32)>)>> = pool.install(|| {
                fine.into_par_iter()
                    .map(|f| {
                        if f.flood_done && f.iso_done {
                            return Ok((f.i, f.out, Vec::new()));
                        }
                        let mut out = f.out;
                        let p = &peaks[f.i];
                        let near: FxSet<u32> = f.near.iter().copied().chain(std::iter::once(peak_summit(p))).collect();
                        let mut over: FxMap<(i64, i64), f32> = FxMap::default();
                        for k in &near {
                            if let Some(s) = info.get(k) {
                                if s.e > f32::MIN {
                                    let q = (s.sp.0 >> (Z12 - Z8_), s.sp.1 >> (Z12 - Z8_));
                                    let v = over.entry(q).or_insert(f32::MIN);
                                    *v = v.max(s.e);
                                }
                            }
                        }
                        let ov8 = Ov8 { base: base8, near: &near, over };
                        let mut d = UDem {
                            z: Z8_,
                            z12: None,
                            z8: Some(z8),
                            cache: HashMap::new(),
                            max_cache: 0,
                            failed: None,
                            read: BTreeSet::new(),
                        };
                        let (sx, sy) = d.px(f.sum[0], f.sum[1]);
                        // The z8 pixel is a mean: the summit keeps its fine height.
                        let e = out.e;
                        if !f.flood_done {
                            match flood(&mut d, Some(&ov8), sx, sy, e, COARSE_MAX, None) {
                                Flood::Done(col, at, reached) => {
                                    out.p = e - col;
                                    out.pl = !reached;
                                    out.c = round5(d.lonlat(at.0, at.1));
                                    out.ce = col;
                                }
                                Flood::Budget(low, at) => {
                                    out.p = e - low;
                                    out.pl = true;
                                    out.c = round5(d.lonlat(at.0, at.1));
                                    out.ce = low;
                                }
                                Flood::Beyond => unreachable!("no reach at z8"),
                            }
                        }
                        // (The tiles whose maxima the search reads, each read whole: `Z8::max`.)
                        let maxed = std::cell::RefCell::new(BTreeSet::new());
                        if !f.iso_done {
                            match nearest_higher(&mut d, Some(&ov8), sx, sy, e, COARSE_ISO_KM, &|x, y| {
                                maxed.borrow_mut().insert((x, y));
                                Some(z8.max(x, y).unwrap_or(f32::INFINITY))
                            }) {
                                Some((km, at)) => {
                                    out.iso = km.max(ISO_FINE_KM);
                                    out.hi = Some(round5(d.lonlat(at.0, at.1)));
                                }
                                None => {
                                    out.iso = COARSE_ISO_KM;
                                    out.il = true;
                                }
                            }
                        }
                        if let Some(e) = d.failed.or_else(|| base8.failed()) {
                            anyhow::bail!("peaks: z8 read failed: {e}");
                        }
                        let mut read = maxed.into_inner();
                        read.extend(d.read);
                        Ok((f.i, out, read.into_iter().collect()))
                    })
                    .collect()
            });
            let mut batch = Vec::with_capacity(results.len());
            let mut read = BTreeSet::new();
            for r in results {
                let (i, o, r) = r.context("coarse stage")?;
                batch.push((i, o));
                read.extend(r);
            }
            Ok(put_outs(coarse, &batch, &read))
        })?;
        let (coarse, kept, r) = get_outs(&kept)?;
        need_coarse += coarse;
        outs.extend(kept);
        read.extend(r);
    }
    eprintln!("peaks: measured ({:.0?}), {need_coarse} needed the coarse stage", t0.elapsed());
    let behind = z8_behind(summits, &read);
    read.extend(behind);
    outs.sort_by_key(|o| o.0);
    let outs = outs
        .into_iter()
        .map(|(i, mut o)| {
            o.e = o.e.round();
            o.p = o.p.max(0.0).round();
            o.ce = o.ce.round();
            o.iso = (o.iso * 100.0).round() / 100.0;
            (peaks[i].key.clone(), o)
        })
        .collect();
    Ok((outs, read))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn tile_bounds_exact_and_wrapping() {
        // Inside: 0.
        assert_eq!(tile_lower_bound_m(8.0, 46.0, 8, 133, 91), 0.0);
        assert!(tile_lower_bound_m(8.0, 46.0, 8, 133, 90) > 0.0);
        // A tile due north: the distance to its southern edge along the meridian.
        let n = (1u64 << 8) as f64;
        let lat_s = roadcore::geo::merc_lat(90.0 / n);
        let lb = tile_lower_bound_m(8.0, 40.0, 8, 133, 89);
        assert!((lb - gc_m(8.0, 40.0, 8.0, lat_s)).abs() < 1.0, "{lb}");
        // Across the antimeridian: the tile just east of 180° from a point at 179.9° E is close.
        let lb = tile_lower_bound_m(179.9, 0.0, 8, 0, 127);
        assert!(lb < 20_000.0, "{lb}");
        // A lower bound never exceeds the distance to any point of the tile (sampled).
        for (lon, lat) in [(-30.0, 60.0), (100.0, -40.0), (7.0, 46.5), (-179.5, 10.0)] {
            for tx in [0u32, 37, 133, 255] {
                for ty in [10u32, 90, 128, 200] {
                    let lb = tile_lower_bound_m(lon, lat, 8, tx, ty);
                    for i in 0..=8 {
                        for j in 0..=8 {
                            let x = (tx as f64 + i as f64 / 8.0) / n;
                            let y = (ty as f64 + j as f64 / 8.0) / n;
                            let plat = roadcore::geo::merc_lat(y);
                            let d = gc_m(lon, lat, x * 360.0 - 180.0, plat);
                            assert!(lb <= d + 1e-6, "{lon},{lat} tile {tx}/{ty}: bound {lb} > {d}");
                        }
                    }
                }
            }
        }
    }

    /// Task #399: two neighbouring z6 tiles' peaks measured from their z12 tiles read ahead, in spans
    /// (`read_ahead`, `UnitZ12::load_from`: `ManifestTiles::get_many`), are byte for byte those
    /// measured from the same packs read a tile at a time, each z6 tile in turn.
    #[test]
    fn read_ahead_in_spans_gives_the_same_bytes_as_tile_by_tile() {
        use terrain::terrain_pack::ManifestTiles;
        let d = tempfile::tempdir().unwrap();
        let mut out = buildstore::out::Out::unbound(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("root")),
            &d.path().join("scratch"),
        )
        .unwrap();
        // Two z6 tiles side by side (6/33/22, 6/34/22), peaks either side of their border.
        let pk = |n: u32, lon: f64, lat: f64, ele: Option<f32>| {
            let (lon, lat) = ((lon * 1e7) as i32, (lat * 1e7) as i32);
            (
                UnitPeak { key: format!("k{n}"), id: format!("n{n}"), lon, lat, ele },
                Summit { id: format!("n{n}"), kind: "peak".into(), lon, lat, ele },
            )
        };
        let units: Vec<Vec<(UnitPeak, Summit)>> = vec![
            vec![pk(1, 11.20, 47.00, Some(1500.0)), pk(2, 11.23, 47.05, None)],
            vec![pk(3, 11.30, 47.02, Some(1200.0)), pk(4, 11.27, 46.98, Some(9000.0))],
        ];
        let summits: Vec<Summit> = units.iter().flatten().map(|p| p.1.clone()).collect();
        let wants: Vec<BTreeSet<(u32, u32)>> = units
            .iter()
            .map(|u| {
                tiles_wanted(
                    &u.iter().map(|p| (p.0.lon as f64 * 1e-7, p.0.lat as f64 * 1e-7)).collect::<Vec<_>>(),
                    29.5,
                )
            })
            .collect();
        // Hills, smooth across tiles: every tile either z6 tile wants, in its z6 tile's hi pack.
        let all: BTreeSet<(u32, u32)> = wants.iter().flatten().copied().collect();
        let mut by_pack: BTreeMap<(u32, u32), Vec<(u8, u32, u32, Vec<u8>, u32)>> = BTreeMap::new();
        for &(x, y) in &all {
            let e: Vec<f32> = (0..256 * 256)
                .map(|i| {
                    let (gx, gy) = ((x * 256 + i % 256) as f64, (y * 256 + i / 256) as f64);
                    (800.0
                        + 600.0 * (gx * 0.013).dsin() * (gy * 0.011).dcos()
                        + 90.0 * (gx * 0.051 + gy * 0.037).dsin())
                    .round() as f32
                })
                .collect();
            let png = roadcore::grid::encode_terrain_png(&e, 256, 256).unwrap();
            by_pack.entry((x >> 6, y >> 6)).or_default().push((Z12, x, y, png, 1));
        }
        for ((x, y), tiles) in by_pack {
            terrain::layers::write_pack(
                &mut out,
                "terrain",
                "terrarium-png",
                false,
                "hi",
                (6, x, y),
                &mut tiles.into_iter(),
            )
            .unwrap();
        }
        // The z8: one flat tile elsewhere in a lo pack (nothing built elsewhere).
        let flat = roadcore::grid::encode_terrain_png(&vec![0f32; 256 * 256], 256, 256).unwrap();
        let z8p = terrain::layers::write_pack_local(
            &out,
            "z8t",
            "terrarium-png",
            false,
            "lo",
            (3, 0, 0),
            &mut [(8u8, 0, 0, flat, 1)].into_iter(),
        )
        .unwrap()
        .unwrap();
        let z8 = Z8::open(&[((0, 0), z8p.local.clone())], 64).unwrap();
        let base8 = Z8Base::new(&summits, &z8);
        // (Its results, then the z8 tiles it read: a resumed run's are its pieces'.)
        let bytes = |(res, read): (Vec<(String, Out)>, BTreeSet<(u32, u32)>)| -> Vec<u8> {
            let mut b = Vec::new();
            for (key, o) in res {
                let mut v = o.json();
                v.as_object_mut().unwrap().remove("i");
                v["key"] = key.into();
                b.extend(serde_json::to_vec(&v).unwrap());
                b.push(b'\n');
            }
            b.extend(format!("{read:?}").bytes());
            b
        };
        let peaks_of = |k: usize| units[k].iter().map(|p| p.0.clone()).collect::<Vec<_>>();
        // Tile by tile, each z6 tile in turn.
        let one = ManifestTiles::new(&out, "terrain");
        let in_turn: Vec<Vec<u8>> = (0..2)
            .map(|k| {
                let z12 = UnitZ12::from_tiles(
                    wants[k].iter().map(|&(x, y)| ((x, y), Some(one.get(Z12, x, y).unwrap().unwrap()))).collect(),
                );
                let r =
                    run(&peaks_of(k), &summits, &base8, &z12, &z8, 2, &mut jobrt::resume_log::Pieces::none()).unwrap();
                assert_eq!(z12.unexpected(), 0);
                bytes(r)
            })
            .collect();
        // In spans, the next z6 tile's read ahead.
        let packs = ManifestTiles::new(&out, "terrain");
        let mut ahead: Vec<Vec<u8>> = Vec::new();
        read_ahead(
            &[0usize, 1],
            |&k| UnitZ12::load_from(&packs, &wants[k]),
            |_, &k, z12| {
                assert_eq!(z12.from, (wants[k].len(), 0), "every tile from the packs");
                ahead.push(bytes(run(
                    &peaks_of(k),
                    &summits,
                    &base8,
                    &z12,
                    &z8,
                    2,
                    &mut jobrt::resume_log::Pieces::none(),
                )?));
                Ok(())
            },
        )
        .unwrap();
        assert!(in_turn.iter().all(|b| b.len() > 100), "{in_turn:?}");
        assert_eq!(ahead, in_turn);
        // Task #889: paused at every one of its safe points (the summits seen, each batch measured),
        // resumed each time: the same bytes.
        let z12 = UnitZ12::load_from(&packs, &wants[0]).unwrap();
        let scratch = d.path().join("resume");
        let mut stops = 0;
        let resumed = loop {
            jobrt::resume_log::STOP_AFTER.set(Some(1));
            let mut pieces = jobrt::resume_log::Pieces::open(&scratch, (6, 33, 22), "peaks", "a", true);
            let r = run(&peaks_of(0), &summits, &base8, &z12, &z8, 2, &mut pieces);
            jobrt::resume_log::STOP_AFTER.set(None);
            match r {
                Err(e) => {
                    assert!(jobrt::control::is_paused(&e), "{e:#}");
                    stops += 1;
                }
                Ok(r) => break bytes(r),
            }
        };
        assert_eq!(stops, 2, "the summits seen, then its one batch");
        assert_eq!(resumed, in_turn[0], "paused at each of its safe points");
    }

    /// Task #1475 review: the z8 tiles a run says it read are what its results stand on (its key's
    /// lo line, pipeline's agent::idents::peaks_lo), whether the isolation search or the flood read
    /// them. A z8 tile it didn't read changed: the same results and the same tiles read. One it
    /// read raised past its peaks: other results (nearer higher ground) and other tiles read (the
    /// search and the flood stop short).
    #[test]
    fn the_z8_tiles_a_run_read_are_what_its_results_stand_on() {
        let (lon, lat) = (11.27f64, 46.98f64);
        // A lone 2000 m dome on a 100 m plain, the highest for far more than 25 km: its isolation
        // and its flood go to the coarse stage.
        z8_read_holds(&[(lon, lat, 2000.0)], &[(11.30, 47.02), (lon, lat)], true);
        // A 1500 m dome, a higher one 18 km east: its isolation found at z12, its flood (600k
        // pixels, ~11 km) to the coarse stage alone, where it reads its own z8 tile (the sea at 0 m
        // all round).
        z8_read_holds(&[(lon, lat, 1500.0), (lon + 0.236, lat, 2000.0)], &[(lon, lat)], false);
    }

    /// `the_z8_tiles_a_run_read_are_what_its_results_stand_on` on `domes` (lon, lat, height, on a
    /// 100 m plain) with peaks at `at`, the first dome's top the one measured at z8; `wide`, its
    /// isolation searched there (read short once a tile it read is raised).
    fn z8_read_holds(domes: &[(f64, f64, f32)], at: &[(f64, f64)], wide: bool) {
        use terrain::terrain_pack::ManifestTiles;
        let d = tempfile::tempdir().unwrap();
        let mut out = buildstore::out::Out::unbound(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("root")),
            &d.path().join("scratch"),
        )
        .unwrap();
        let w = (1u64 << Z12) as f64 * 256.0;
        let px = |lon: f64, lat: f64| {
            ((lon + 180.0) / 360.0 * w, (1.0 - lat.to_radians().dtan().dasinh() / std::f64::consts::PI) / 2.0 * w)
        };
        let unit: Vec<(UnitPeak, Summit)> = at
            .iter()
            .enumerate()
            .map(|(n, &(lon, lat))| {
                let (lon, lat) = ((lon * 1e7) as i32, (lat * 1e7) as i32);
                (
                    UnitPeak { key: format!("k{n}"), id: format!("n{n}"), lon, lat, ele: None },
                    Summit { id: format!("n{n}"), kind: "peak".into(), lon, lat, ele: None },
                )
            })
            .collect();
        let summits: Vec<Summit> = unit.iter().map(|p| p.1.clone()).collect();
        let peaks: Vec<UnitPeak> = unit.iter().map(|p| p.0.clone()).collect();
        let want = tiles_wanted(at, 29.5);
        let tops: Vec<((f64, f64), f32)> = domes.iter().map(|&(lon, lat, e)| (px(lon, lat), e)).collect();
        let mut by_pack: BTreeMap<(u32, u32), Vec<(u8, u32, u32, Vec<u8>, u32)>> = BTreeMap::new();
        for &(x, y) in &want {
            let e: Vec<f32> = (0..256 * 256)
                .map(|i| {
                    let (gx, gy) = ((x * 256 + i % 256) as f64, (y * 256 + i / 256) as f64);
                    let dome = |&((cx, cy), e): &((f64, f64), f32)| {
                        (e as f64 - 100.0) * (-((gx - cx).powi(2) + (gy - cy).powi(2)) / (80.0 * 80.0)).dexp()
                    };
                    (100.0 + tops.iter().map(dome).fold(0.0, f64::max)).round() as f32
                })
                .collect();
            let png = roadcore::grid::encode_terrain_png(&e, 256, 256).unwrap();
            by_pack.entry((x >> 6, y >> 6)).or_default().push((Z12, x, y, png, 1));
        }
        for ((x, y), tiles) in by_pack {
            terrain::layers::write_pack(
                &mut out,
                "terrain",
                "terrarium-png",
                false,
                "hi",
                (6, x, y),
                &mut tiles.into_iter(),
            )
            .unwrap();
        }
        let z12 = UnitZ12::load_from(&ManifestTiles::new(&out, "terrain"), &want).unwrap();
        // The z8 as lo packs holding `tiles` (each flat at its height), a pack per z3 tile.
        let z8_of = |name: &str, tiles: &[((u32, u32), f32)]| {
            let mut packs: BTreeMap<(u32, u32), Vec<(u8, u32, u32, Vec<u8>, u32)>> = BTreeMap::new();
            for &((x, y), e) in tiles {
                let png = roadcore::grid::encode_terrain_png(&vec![e; 256 * 256], 256, 256).unwrap();
                packs.entry((x >> 5, y >> 5)).or_default().push((8, x, y, png, 1));
            }
            let local: Vec<((u32, u32), std::path::PathBuf)> = packs
                .into_iter()
                .map(|(q, ts)| {
                    let name = format!("{name}-{}-{}", q.0, q.1);
                    let p = terrain::layers::write_pack_local(
                        &out,
                        &name,
                        "terrarium-png",
                        false,
                        "lo",
                        (3, q.0, q.1),
                        &mut ts.into_iter(),
                    )
                    .unwrap()
                    .unwrap();
                    (q, p.local.clone())
                })
                .collect();
            Z8::open(&local, 64).unwrap()
        };
        let measure = |z8: &Z8| {
            let (res, read) =
                run(&peaks, &summits, &Z8Base::new(&summits, z8), &z12, z8, 2, &mut jobrt::resume_log::Pieces::none())
                    .unwrap();
            (res.iter().map(|(k, o)| format!("{k} {}", o.json())).collect::<Vec<_>>(), read)
        };
        let far = (200, 200);
        let (first, read) = measure(&z8_of("a", &[(far, 0.0)]));
        assert!(!read.is_empty(), "the coarse stage read the z8");
        assert!(!read.contains(&far), "{far:?} is past what it reads");
        // Task #1545 review: a tagged summit at the edge of a tile it read, beside one it didn't
        // (far from its peaks, its z8 height none, so the results stand): the run's record holds
        // that neighbour too, which its height reads (crate::summits::z8_tiles_read).
        let edge = read
            .iter()
            .flat_map(|&(x, y)| [(x, y, 255u32, x + 1), (x, y, 0, x.wrapping_sub(1))])
            .filter(|&(_, y, _, nx)| nx < 256 && !read.contains(&(nx, y)))
            .map(|(x, y, px, nx)| {
                let w = 256.0 * 256.0;
                let (gx, gy) = ((x * 256 + px) as f64 + 0.5, (y * 256 + 128) as f64 + 0.5);
                let (lon, lat) = (gx / w * 360.0 - 180.0, roadcore::geo::merc_lat(gy / w));
                (lon, lat, (nx, y))
            })
            .find(|&(lon, lat, _)| at.iter().all(|&(a, b)| gc_m(lon, lat, a, b) > 5000.0))
            .expect("a read tile's edge beside an unread one, away from the peaks");
        let mut with = summits.clone();
        with.push(Summit {
            id: "n99".into(),
            kind: "peak".into(),
            lon: (edge.0 * 1e7) as i32,
            lat: (edge.1 * 1e7) as i32,
            ele: Some(500.0),
        });
        let z8e = z8_of("e", &[(far, 0.0)]);
        let (res, read_with) =
            run(&peaks, &with, &Z8Base::new(&with, &z8e), &z12, &z8e, 2, &mut jobrt::resume_log::Pieces::none())
                .unwrap();
        assert_eq!(res.iter().map(|(k, o)| format!("{k} {}", o.json())).collect::<Vec<_>>(), first, "the same results");
        assert!(read_with.contains(&edge.2), "its unread neighbour {:?} recorded: {read_with:?}", edge.2);
        assert_eq!(read_with.len(), read.len() + 1, "that neighbour alone added");
        // Task #1556 review: that neighbour unreadable (a local copy gone bad): working its height
        // out fails, and so does the run, saying so.
        let bad = terrain::layers::write_pack_local(
            &out,
            "bad",
            "terrarium-png",
            false,
            "lo",
            (3, edge.2 .0 >> 5, edge.2 .1 >> 5),
            &mut [(8u8, edge.2 .0, edge.2 .1, b"not a png".to_vec(), 1)].into_iter(),
        )
        .unwrap()
        .unwrap();
        let z8b = Z8::open(&[((edge.2 .0 >> 5, edge.2 .1 >> 5), bad.local.clone())], 64).unwrap();
        let base = Z8Base::new(&with, &z8b);
        let e = run(&peaks, &with, &base, &z12, &z8b, 2, &mut jobrt::resume_log::Pieces::none()).unwrap_err();
        assert!(format!("{e:#}").contains("z8 read failed"), "{e:#}");
        assert!(base.failed().is_some());
        // A tile it didn't read raised: the same results, the same tiles read.
        assert_eq!(measure(&z8_of("b", &[(far, 9500.0)])), (first.clone(), read.clone()), "a tile it didn't read");
        // One it read raised past its peaks (the nearest to five z8 tiles east): nearer higher ground.
        let (tx, ty) = (tops[0].0 .0 as i64 / 4096, tops[0].0 .1 as i64 / 4096);
        let near = *read.iter().min_by_key(|&&(x, y)| (x as i64 - tx - 5).abs() + (y as i64 - ty).abs()).unwrap();
        let (moved, read2) = measure(&z8_of("c", &[(far, 9500.0), (near, 9500.0)]));
        assert_ne!(moved, first, "a tile it read raised changes its results");
        if wide {
            assert!(read2.len() < read.len(), "the search stops at the nearer higher ground");
        }
    }

    /// Task #1545: a run's z8 record holds the tiles behind the z8 heights of the summits its
    /// overlay read (those whose pixel is in a tile read): the tile next door for one at its edge;
    /// nothing for one in a tile it didn't read, or with no tagged height.
    #[test]
    fn the_z8_record_holds_the_tiles_behind_the_summits_read() {
        let at = |gx: f64, gy: f64, ele: Option<f32>| {
            let w = 256.0 * 256.0;
            let (lon, lat) = ((gx + 0.5) / w * 360.0 - 180.0, roadcore::geo::merc_lat((gy + 0.5) / w));
            Summit { id: "n1".into(), kind: "peak".into(), lon: (lon * 1e7) as i32, lat: (lat * 1e7) as i32, ele }
        };
        let read: BTreeSet<(u32, u32)> = [(133, 90)].into();
        let edge = at(133.0 * 256.0 + 255.0, 90.0 * 256.0 + 7.0, Some(1200.0));
        let elsewhere = at(140.0 * 256.0 + 255.0, 90.0 * 256.0 + 7.0, Some(1200.0));
        let untagged = at(133.0 * 256.0 + 255.0, 90.0 * 256.0 + 9.0, None);
        let mut got = z8_behind(&[edge, elsewhere, untagged], &read);
        got.sort_unstable();
        got.dedup();
        assert_eq!(got, [(133, 90), (134, 90)]);
    }

    /// Task #1556: the overlay's summit heights are the run's own, from the z8 it reads: a tagged
    /// height plausible over the z8 there counts, one implausible (feet over flat ground) or
    /// untagged doesn't, and a tile with no tagged summit has none.
    #[test]
    fn the_overlays_summit_heights_are_worked_out_from_the_runs_z8() {
        let d = tempfile::tempdir().unwrap();
        let out = buildstore::out::Out::unbound(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("root")),
            &d.path().join("scratch"),
        )
        .unwrap();
        let png = roadcore::grid::encode_terrain_png(&vec![900.0; 256 * 256], 256, 256).unwrap();
        let p = terrain::layers::write_pack_local(
            &out,
            "z8h",
            "terrarium-png",
            false,
            "lo",
            (3, 133 >> 5, 90 >> 5),
            &mut [(8u8, 133, 90, png, 1)].into_iter(),
        )
        .unwrap()
        .unwrap();
        let z8 = Z8::open(&[((133 >> 5, 90 >> 5), p.local.clone())], 64).unwrap();
        let at = |id: &str, gx: f64, gy: f64, ele: Option<f32>| {
            let w = 256.0 * 256.0;
            let (lon, lat) = ((gx + 0.5) / w * 360.0 - 180.0, roadcore::geo::merc_lat((gy + 0.5) / w));
            Summit { id: id.into(), kind: "peak".into(), lon: (lon * 1e7) as i32, lat: (lat * 1e7) as i32, ele }
        };
        let (gx, gy) = (133.0 * 256.0 + 100.0, 90.0 * 256.0 + 100.0);
        let summits = [
            at("n1", gx, gy, Some(1500.0)),
            at("n2", gx + 10.0, gy, Some(9000.0)),
            at("n3", gx + 20.0, gy, None),
            // (Over flat ground where nothing is built: 2,000 m is past twice 0 m plus 300.)
            at("n4", 140.0 * 256.0 + 100.0, gy, Some(2000.0)),
        ];
        let base = Z8Base::new(&summits, &z8);
        let h = base.heights((133, 90)).unwrap();
        let px = |s: &Summit| crate::summits::z8_pixel(s.lon, s.lat);
        assert_eq!(h.get(&px(&summits[0])), Some(&vec![(0, 1500.0)]));
        assert_eq!(h.len(), 1, "{h:?}");
        assert!(base.heights((140, 90)).unwrap().is_empty());
        assert!(base.heights((134, 90)).is_none(), "no tagged summit there");
        assert_eq!(base.failed(), None);
    }

    /// A load that fails fails in its turn, after the z6 tiles before it were done.
    #[test]
    fn read_ahead_fails_in_turn() {
        let mut done = Vec::new();
        let r = read_ahead(
            &[1, 2, 3],
            |&i| if i == 2 { anyhow::bail!("no candidates for {i}") } else { Ok(i * 10) },
            |_, _, t| {
                done.push(t);
                Ok(())
            },
        );
        assert_eq!(done, vec![10]);
        assert!(r.unwrap_err().to_string().contains("no candidates for 2"));
    }

    #[test]
    fn gc_agrees_with_known_distance() {
        // Paris – New York, about 5,837 km.
        let d = gc_m(2.3522, 48.8566, -74.0060, 40.7128) / 1000.0;
        assert!((d - 5837.0).abs() < 10.0, "{d}");
    }
}
