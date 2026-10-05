//! Prominence and isolation for a unit's peaks (docs/phase5.md "peaks"), so that a peak's result
//! doesn't depend on which unit computes it nor on the coverage:
//!
//! - z12 is the terrain pack's tile where the manifest has it, else AWS's raw tile processed the
//!   same way (`UnitZ12`; the open sea, which AWS has no tile for, at 0 m).
//! - The summits near a peak count at their own heights, each worked out from z12 the same way
//!   whichever peak asks: its summit pixel (highest within 150 m), its claim of that pixel (several
//!   on one: the highest tagged, then the nearest to it, then the lowest OSM id), its height.
//! - The fine stage reads nothing farther than 28 km from the summit: a flood that would goes to
//!   the coarse stage; the 25 km isolation search counts and opens only what lies within 25 km.
//! - The coarse stage reads the worldwide z8 (`terrain_z8::Z8`), with the summits near the peak at
//!   their z12 heights and the others at their tagged ones where plausible (`summits::z8_height`).
//! - Distances are great-circle; the isolation searches visit tiles in order of an exact
//!   great-circle lower bound, x wrapping at the antimeridian; nothing higher within 5,000 km is a
//!   lower bound of 5,000 km.

use det::Det;
use super::{despike, round5, tile_lat, Out, Overlay, COARSE_MAX, FINE_MAX, ISO_FINE_KM, TS};
use crate::summits::Summit;
use crate::terrain_z8::Z8;
use anyhow::{Context, Result};
use rayon::prelude::*;
use roadcore::grid::decode_terrain_png;
use roadcore::{merc, EARTH_R};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
    let (lon0, lon1) = (tx as f64 / n * 360.0 - 180.0, (tx + 1) as f64 / n * 360.0 - 180.0);
    let lat_of = |t: f64| (std::f64::consts::PI * (1.0 - 2.0 * t / n)).dsinh().datan().to_degrees();
    let (lat_n, lat_s) = (lat_of(ty as f64), lat_of(ty as f64 + 1.0));
    // Longitude offset from the tile's west edge, wrapped into [0, 360).
    let off = (lon - lon0).rem_euclid(360.0);
    let inside_lon = off <= lon1 - lon0;
    if inside_lon && lat >= lat_s && lat <= lat_n {
        return 0.0;
    }
    let mut best = f64::MAX;
    // The parallels: at the longitude nearest the point's.
    let near_lon = if inside_lon { lon } else if off - (lon1 - lon0) < 360.0 - off { lon1 } else { lon0 };
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

/// A unit's z12 tiles, read before the run: each is the terrain pack's when the manifest has it,
/// else AWS's raw tile from the cache (fetched when missing; a failed fetch fails) processed the
/// same way and read back from its PNG; AWS having none is the open sea. A tile asked for that
/// wasn't read before is counted (`unexpected`), and fails the run.
pub struct UnitZ12 {
    tiles: HashMap<(u32, u32), Option<Arc<Vec<u8>>>>,
    unexpected: AtomicUsize,
    /// How many came from the packs, from AWS, as sea.
    pub from: (usize, usize, usize),
}

impl UnitZ12 {
    pub fn load(out: &crate::out::Out, raw: &crate::terrain_pack::RawTiles, want: &BTreeSet<(u32, u32)>) -> Result<UnitZ12> {
        let packs = crate::terrain_pack::ManifestTiles::new(out, "terrain");
        let got: Vec<Result<((u32, u32), Option<Arc<Vec<u8>>>, u8)>> = want
            .par_iter()
            .map(|&(x, y)| {
                if let Some(b) = packs.get(Z12, x, y)? {
                    anyhow::ensure!(decode_terrain_png(&b).is_ok(), "the terrain pack's z12 {x}/{y} doesn't decode");
                    return Ok(((x, y), Some(Arc::new(b)), 0));
                }
                match raw.get(Z12, x, y)?.0 {
                    Some(b) => {
                        // A cached tile that doesn't decode is fetched again, once.
                        let b = if decode_terrain_png(&b).is_ok() { b } else { raw.refetch(Z12, x, y)?.with_context(|| format!("AWS's z12 {x}/{y} is gone"))? };
                        anyhow::ensure!(decode_terrain_png(&b).is_ok(), "AWS's z12 {x}/{y} doesn't decode");
                        let (png, _, _) = crate::terrain_pack::process(b, Z12, x, y, &HashMap::new(), &HashMap::new());
                        Ok(((x, y), Some(Arc::new(png)), 1))
                    }
                    None => Ok(((x, y), None, 2)),
                }
            })
            .collect();
        let mut tiles = HashMap::with_capacity(want.len());
        let mut from = (0, 0, 0);
        for g in got {
            let (k, t, src) = g?;
            match src {
                0 => from.0 += 1,
                1 => from.1 += 1,
                _ => from.2 += 1,
            }
            tiles.insert(k, t);
        }
        Ok(UnitZ12 { tiles, unexpected: AtomicUsize::new(0), from })
    }

    /// For tests: tiles given (None: sea).
    pub fn from_tiles(tiles: HashMap<(u32, u32), Option<Vec<u8>>>) -> UnitZ12 {
        UnitZ12 { tiles: tiles.into_iter().map(|(k, v)| (k, v.map(Arc::new))).collect(), unexpected: AtomicUsize::new(0), from: (0, 0, 0) }
    }

    /// A tile's elevations, despiked (zeros for the sea); None for one not read before (or not
    /// decoding, which `load` rules out), counted.
    fn decoded(&self, x: u32, y: u32) -> Option<Vec<f32>> {
        match self.tiles.get(&(x, y)) {
            Some(Some(png)) => match decode_terrain_png(png) {
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

/// The z12 tiles a unit's peaks may read: those meeting a disc of `km` around each, padded.
pub fn tiles_wanted(points: &[(f64, f64)], km: f64) -> BTreeSet<(u32, u32)> {
    let n = 1u32 << Z12;
    let mut want = BTreeSet::new();
    for &(lon, lat) in points {
        let dlat = km / 110.0 + 0.01;
        let dlon = km / (111.0 * (lat.abs() + dlat).min(85.0).to_radians().dcos()) + 0.01;
        let t = |lon: f64, lat: f64| {
            let (x, y) = merc(lon, lat.clamp(-85.05, 85.05));
            ((x * n as f64).floor() as i64, (y * n as f64).floor().clamp(0.0, (n - 1) as f64) as i64)
        };
        let (x0, y0) = t(lon - dlon, lat + dlat);
        let (x1, y1) = t(lon + dlon, lat - dlat);
        for x in x0..=x1 {
            for y in y0..=y1 {
                want.insert((x.rem_euclid(n as i64) as u32, y as u32));
            }
        }
    }
    want
}

/// The overlays the unit's searches read: at z12 a peak's nearby summits; at z8 every summit's
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

/// Every summit's tagged z8 height, by z8 pixel (summit index, height), and the pixels by tile.
pub struct Z8Base {
    px: HashMap<(i64, i64), Vec<(u32, f32)>>,
    tiles: HashMap<(i64, i64), Vec<(i64, i64)>>,
}

impl Z8Base {
    pub fn new(summits: &[Summit]) -> Z8Base {
        let mut px: HashMap<(i64, i64), Vec<(u32, f32)>> = HashMap::new();
        for (i, s) in summits.iter().enumerate() {
            if let Some(h) = s.z8 {
                px.entry(crate::summits::z8_pixel(s.lon, s.lat)).or_default().push((i as u32, h));
            }
        }
        let mut tiles: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();
        for &(x, y) in px.keys() {
            tiles.entry((x.div_euclid(TS), y.div_euclid(TS))).or_default().push((x, y));
        }
        for v in tiles.values_mut() {
            v.sort_unstable_by_key(|p| (p.1, p.0));
        }
        Z8Base { px, tiles }
    }
}

/// A peak's z8 overlay: the base without the summits near it, which count at their z12 heights.
struct Ov8<'a> {
    base: &'a Z8Base,
    near: &'a HashSet<u32>,
    over: HashMap<(i64, i64), f32>,
}

impl Ov for Ov8<'_> {
    fn at(&self, p: (i64, i64)) -> Option<f32> {
        let a = self.over.get(&p).copied();
        let b = self.base.px.get(&p).and_then(|v| v.iter().filter(|(i, _)| !self.near.contains(i)).map(|x| x.1).reduce(f32::max));
        match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
    fn tile_pixels(&self, t: (i64, i64)) -> Vec<(i64, i64, f32)> {
        let mut ps: Vec<(i64, i64)> = self.base.tiles.get(&t).cloned().unwrap_or_default();
        ps.extend(self.over.keys().filter(|p| (p.0.div_euclid(TS), p.1.div_euclid(TS)) == t));
        ps.sort_unstable_by_key(|p| (p.1, p.0));
        ps.dedup();
        ps.into_iter().filter_map(|p| self.at(p).map(|v| (p.0, p.1, v))).collect()
    }
}

/// One zoom's pixels for the unit's searches: x wraps around the world; beyond the poles nothing.
struct UDem<'a> {
    z: u8,
    z12: Option<&'a UnitZ12>,
    z8: Option<&'a Z8>,
    cache: HashMap<(u32, u32), Option<Arc<Vec<f32>>>>,
    max_cache: usize,
    /// A z8 read that failed (the local artifact): fails the run.
    failed: Option<String>,
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
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).dsinh().datan().to_degrees();
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

/// Today's priority flood (peaks.rs `flood`), x wrapping, and stopping at `reach` (pixels from
/// the summit, squared) when given.
fn flood(d: &mut UDem, ov: Option<&dyn Ov>, sx: i64, sy: i64, e: f32, budget: usize, reach: Option<i64>) -> Flood {
    let key = |v: f32| (v * 100.0).round() as i32;
    let mut heap: BinaryHeap<(i32, i64, i64, Reverse<i32>, i64, i64)> = BinaryHeap::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
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
fn nearest_higher(d: &mut UDem, ov: Option<&dyn Ov>, sx: i64, sy: i64, e: f32, max_km: f64, tile_max: &dyn Fn(u32, u32) -> Option<f32>) -> Option<(f64, (i64, i64))> {
    let s = d.lonlat(sx, sy);
    let w = d.w();
    let n = 1i64 << d.z;
    let start = (sx.div_euclid(TS), sy.div_euclid(TS));
    let mut heap: BinaryHeap<Reverse<(u64, i64, i64)>> = BinaryHeap::new();
    let mut queued: HashSet<(i64, i64)> = HashSet::new();
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

/// A peak of the unit: its key (handed back), its summit's id in `summits`, position, tagged height.
#[derive(Clone, Debug)]
pub struct UnitPeak {
    pub key: String,
    pub id: String,
    pub lon: i32,
    pub lat: i32,
    pub ele: Option<f32>,
}

/// A summit as the unit's peaks see it (from z12).
#[derive(Clone, Copy, Debug)]
struct Seen {
    /// Its summit pixel after the claims, and its height there.
    sp: (i64, i64),
    e: f32,
}

/// Peaks of a unit: (key, result) in the peaks' order.
pub fn run(peaks: &[UnitPeak], summits: &[Summit], base8: &Z8Base, z12: &UnitZ12, z8: &Z8, coarse_threads: usize) -> Result<Vec<(String, Out)>> {
    let t0 = std::time::Instant::now();
    let by_id: HashMap<&str, usize> = summits.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();
    // The summits near the unit's peaks, by 0.1° cell.
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, s) in summits.iter().enumerate() {
        grid.entry((s.lon.div_euclid(1_000_000), s.lat.div_euclid(1_000_000))).or_default().push(i as u32);
    }
    let within = |lon: i32, lat: i32, km: f64| -> Vec<u32> {
        let (la, lo) = (lat as f64 * 1e-7, lon as f64 * 1e-7);
        let dlat = km / 110.0 + 0.01;
        let dlon = km / (111.0 * (la.abs() + dlat).min(89.0).to_radians().dcos()) + 0.01;
        let c = |v: f64| (v * 10.0).floor() as i32;
        let mut out = Vec::new();
        for cx in c(lo - dlon)..=c(lo + dlon) {
            for cy in c(la - dlat)..=c(la + dlat) {
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
            extra.push(Summit { id: p.id.clone(), kind: "peak".into(), lon: p.lon, lat: p.lat, ele: p.ele, z8: None });
        }
    }
    // Every summit's pixel and the highest within 150 m (today's rule), from z12.
    let mut all: Vec<(u32, &Summit)> = need.iter().map(|&i| (i, &summits[i as usize])).collect();
    let extra_base = summits.len() as u32;
    all.extend(extra.iter().enumerate().map(|(k, s)| (extra_base + k as u32, s)));
    let seen: Vec<(u32, (i64, i64), (i64, i64), f32, f32)> = all
        .par_chunks(256)
        .flat_map_iter(|chunk| {
            let mut d = UDem { z: Z12, z12: Some(z12), z8: None, cache: HashMap::new(), max_cache: 96, failed: None };
            chunk
                .iter()
                .map(|&(i, s)| {
                    let (lon, lat) = (s.lon as f64 * 1e-7, s.lat as f64 * 1e-7);
                    let (px, py) = d.px(lon, lat);
                    let r = (150.0 / (gc_m(lon, lat, lon + 0.001, lat) / 0.001 * 360.0 / ((1u64 << Z12) as f64 * 256.0))).ceil() as i64;
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
    let mut d0 = UDem { z: Z12, z12: Some(z12), z8: None, cache: HashMap::new(), max_cache: 256, failed: None };
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
                .max_by(|&&a, &&b| score(a).partial_cmp(&score(b)).unwrap().then_with(|| summit_of(seen[b].0).order().cmp(&summit_of(seen[a].0).order())))
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
            let e = if dem > f32::MIN && ele.is_finite() && ele >= dem - 30.0 && ele <= dem + 200.0 { ele.max(dem) } else { dem };
            info.insert(s.0, Seen { sp, e });
        }
    }
    drop(d0);
    eprintln!("peaks: {} summits near {} peaks, {moved} moved off a shared pixel ({:.0?})", info.len(), peaks.len(), t0.elapsed());

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
    let fine: Vec<Fine> = order
        .par_chunks(64)
        .flat_map_iter(|chunk| {
            let mut d = UDem { z: Z12, z12: Some(z12), z8: None, cache: HashMap::new(), max_cache: 96, failed: None };
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
                    let mut out = Out { i, e: e.max(0.0), p: 0.0, pl: false, c: [p.lon as f64 * 1e-7, p.lat as f64 * 1e-7], ce: 0.0, iso: 0.0, il: false, hi: None };
                    let (mut flood_done, mut iso_done) = (e <= f32::MIN, e <= f32::MIN);
                    if e > f32::MIN {
                        // The reach in pixels at the summit's latitude, a little short.
                        let px_m = gc_m(s_ll[0], s_ll[1], s_ll[0] + 360.0 / (256.0 * 4096.0), s_ll[1]);
                        let r = (FINE_REACH_KM * 1000.0 / px_m * 0.995) as i64;
                        if let Flood::Done(col, at, reached) = flood(&mut d, Some(&ov), sx, sy, e, FINE_MAX, Some(r * r)) {
                            out.p = e - col;
                            out.pl = !reached;
                            out.c = round5(d.lonlat(at.0, at.1));
                            out.ce = col;
                            flood_done = true;
                        }
                        if let Some((km, at)) = nearest_higher(&mut d, Some(&ov), sx, sy, e, ISO_FINE_KM, &|_, _| None) {
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
    anyhow::ensure!(failed_z12 == 0, "peaks: {failed_z12} z12 tiles were read that weren't loaded (the reach is wrong)");
    let need_coarse = fine.iter().filter(|f| !f.flood_done || !f.iso_done).count();
    eprintln!("peaks: fine stage done ({:.0?}), {need_coarse} need the coarse stage", t0.elapsed());

    // The coarse stage (z8), a few at a time (a flood holds up to 40M pixels).
    let pool = rayon::ThreadPoolBuilder::new().num_threads(coarse_threads.max(1)).build()?;
    let results: Vec<Result<(usize, Out)>> = pool.install(|| {
        fine.into_par_iter()
            .map(|f| {
                if f.flood_done && f.iso_done {
                    return Ok((f.i, f.out));
                }
                let mut out = f.out;
                let p = &peaks[f.i];
                let near: HashSet<u32> = f.near.iter().copied().chain(std::iter::once(peak_summit(p))).collect();
                let mut over: HashMap<(i64, i64), f32> = HashMap::new();
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
                let mut d = UDem { z: Z8_, z12: None, z8: Some(z8), cache: HashMap::new(), max_cache: 0, failed: None };
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
                if !f.iso_done {
                    match nearest_higher(&mut d, Some(&ov8), sx, sy, e, COARSE_ISO_KM, &|x, y| Some(z8.max(x, y))) {
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
                if let Some(e) = d.failed {
                    anyhow::bail!("peaks: z8 read failed: {e}");
                }
                Ok((f.i, out))
            })
            .collect()
    });
    let mut outs: Vec<(usize, Out)> = results.into_iter().collect::<Result<_>>().context("coarse stage")?;
    outs.sort_by_key(|o| o.0);
    Ok(outs
        .into_iter()
        .map(|(i, mut o)| {
            o.e = o.e.round();
            o.p = o.p.max(0.0).round();
            o.ce = o.ce.round();
            o.iso = (o.iso * 100.0).round() / 100.0;
            (peaks[i].key.clone(), o)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_bounds_exact_and_wrapping() {
        // Inside: 0.
        assert_eq!(tile_lower_bound_m(8.0, 46.0, 8, 133, 91), 0.0);
        assert!(tile_lower_bound_m(8.0, 46.0, 8, 133, 90) > 0.0);
        // A tile due north: the distance to its southern edge along the meridian.
        let n = (1u64 << 8) as f64;
        let lat_s = (std::f64::consts::PI * (1.0 - 2.0 * 90.0 / n)).dsinh().datan().to_degrees();
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
                            let plat = (std::f64::consts::PI * (1.0 - 2.0 * y)).dsinh().datan().to_degrees();
                            let d = gc_m(lon, lat, x * 360.0 - 180.0, plat);
                            assert!(lb <= d + 1e-6, "{lon},{lat} tile {tx}/{ty}: bound {lb} > {d}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn gc_agrees_with_known_distance() {
        // Paris – New York, about 5,837 km.
        let d = gc_m(2.3522, 48.8566, -74.0060, 40.7128) / 1000.0;
        assert!((d - 5837.0).abs() < 10.0, "{d}");
    }
}
