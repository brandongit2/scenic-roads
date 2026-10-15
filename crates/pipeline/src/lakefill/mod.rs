//! Lakes filled from Canada's own data where OSM has none (docs/plan.md §6 Water, "Lakes filled
//! from Canada's own data").
//!
//! OSM's Canadian lakes came from Canada's own (CanVec, the National Hydro Network) map sheet by map
//! sheet, and some sheets were never imported: their small lakes are missing in rectangles. The
//! fill takes the source's lakes in those sheets (`nts`, found by `lakefill survey`) and adds them
//! to the water as the basemap has it, by these rules (`select`):
//! - **Whole lakes only.** A lake the source cut at a sheet's edge (pieces sharing an edge, or lying
//!   along the same grid line) is one lake: taken whole, or not at all.
//! - **None touching OSM's water.** A lake that touches or overlaps any water the basemap has (within
//!   `Rules::tol_m`, as the source's and OSM's shores differ by a few metres) is left out, with all
//!   its pieces: OSM's lake stays as it is, never doubled.
//! - **OSM's halves completed** (`Rules::complete_halves`, on): where the import cut OSM's lake at a
//!   gap sheet's edge, the source's lake touching it only across that edge is taken as its part in
//!   the gap sheets, cut there, so the two abut and the lake reads whole.
//! - **No doubles.** A lake the source has twice (two packages' copies) is taken once, and a piece
//!   wholly inside another of the same lake is dropped.
//! - Lakes meeting a gap sheet are taken whole, wherever the rest of them lies.
//!
//! **The fill** (`write`, `Fill`) is one sectioned file, content-named, made by the `lakefill` step
//! (`run`) and read by the water layer (`water::Extra`: each z14 tile's lakes, as a basemap tile of
//! their own, inland water), the map's server (the water's deeper zooms) and the terrain's
//! flattening (`terrain_water::WaterSource`, `FilledWater`: its pin names the fill, so the terrain's
//! water digests, and with them the pieces whose water the fill changes, follow it).

use anyhow::{ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use det::Det;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub mod run;

/// An MVT polygon geometry's rings (tile units).
pub fn mvt_rings(g: &[u32]) -> Vec<Vec<[f64; 2]>> {
    crate::water::rings(g)
}

/// Rings (world units, or a tile's).
pub type Rings = Vec<Vec<[f64; 2]>>;
/// A segment's ends.
type Seg = ([f64; 2], [f64; 2]);

/// World units (0–1) to the fill's integers (× 2³², 9 mm at the equator), as `watercov`'s store.
const SCALE: f64 = 4_294_967_296.0;

pub fn quant(v: f64) -> u32 {
    (v * SCALE).round().clamp(0.0, SCALE - 1.0) as u32
}

/// Longitude and latitude of a point in world units.
pub fn lonlat(p: [f64; 2]) -> (f64, f64) {
    (p[0] * 360.0 - 180.0, (std::f64::consts::PI * (1.0 - 2.0 * p[1])).dsinh().datan().to_degrees())
}

/// Metres a world unit at latitude `lat`.
pub fn metres_per_unit(lat: f64) -> f64 {
    40_075_016.686 * lat.to_radians().dcos()
}

// ---- a lake ---------------------------------------------------------------------------------------

/// A lake (or a piece of one as the source has it): rings in world units (Web Mercator, 0–1, y
/// down), outer rings clockwise on the map (`watercov::area2` > 0), holes (islands) anticlockwise.
#[derive(Clone, Debug, PartialEq)]
pub struct Lake {
    pub rings: Vec<Vec<[f64; 2]>>,
    /// Water only some of the time (the source's `perm`): drawn as water, as OSM's intermittent
    /// lakes are, but not flattened in the terrain (terrain_water leaves OSM's alone too).
    pub intermittent: bool,
}

impl Lake {
    /// A lake from rings of longitude and latitude (a shapefile's polygon: outer rings clockwise,
    /// holes anticlockwise, closed or not). Rings are put on the fill's grid (`quant`); ones with no
    /// area left out. When the largest ring isn't clockwise, the source's orientation is taken as
    /// reversed throughout. None when nothing is left.
    pub fn from_lonlat(rings: &[Vec<[f64; 2]>]) -> Option<Lake> {
        let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
        for r in rings {
            let mut q: Vec<[u32; 2]> = r
                .iter()
                .map(|p| {
                    let w = crate::watercov::world(p[0], p[1]);
                    [quant(w[0]), quant(w[1])]
                })
                .collect();
            q.dedup();
            if q.len() > 1 && q.first() == q.last() {
                q.pop();
            }
            if q.len() < 3 {
                continue;
            }
            let r: Vec<[f64; 2]> = q.iter().map(|p| [f64::from(p[0]) / SCALE, f64::from(p[1]) / SCALE]).collect();
            if crate::watercov::area2(&r) == 0.0 {
                continue;
            }
            out.push(r);
        }
        let big = out.iter().max_by(|a, b| crate::watercov::area2(a).abs().total_cmp(&crate::watercov::area2(b).abs()))?;
        if crate::watercov::area2(big) < 0.0 {
            for r in &mut out {
                r.reverse();
            }
        }
        Some(Lake { rings: out, intermittent: false })
    }

    /// West, north, east, south as min x, min y, max x, max y (world units).
    pub fn bbox(&self) -> [f64; 4] {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for p in self.rings.iter().flatten() {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        b
    }

    /// Its area (world units², outer rings less holes).
    pub fn area(&self) -> f64 {
        self.rings.iter().map(|r| crate::watercov::area2(r)).sum::<f64>() / 2.0
    }

    /// Its points on the fill's grid, sorted: the same for the same lake however its rings start.
    fn key(&self) -> u64 {
        let mut pts: Vec<u64> = self.rings.iter().flatten().map(|p| (u64::from(quant(p[0])) << 32) | u64::from(quant(p[1]))).collect();
        pts.sort_unstable();
        xxhash_rust::xxh3::xxh3_64(bytemuck::cast_slice(&pts))
    }
}

/// Nonzero winding of rings at `p` (rings in any orientation, holes against their outer ring's).
pub fn winding(rings: &[Vec<[f64; 2]>], p: [f64; 2]) -> i32 {
    let mut w = 0;
    for r in rings {
        for i in 0..r.len() {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            if a[1] <= p[1] {
                if b[1] > p[1] && cross(a, b, p) > 0.0 {
                    w += 1;
                }
            } else if b[1] <= p[1] && cross(a, b, p) < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

fn cross(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1])
}

/// The distance from `p` to segment `a`–`b`, squared.
fn seg_point2(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    qx * qx + qy * qy
}

/// The distance between two segments, squared (0 when they cross).
fn seg_seg2(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let (d1, d2, d3, d4) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return 0.0;
    }
    seg_point2(a, b, c).min(seg_point2(a, b, d)).min(seg_point2(c, d, a)).min(seg_point2(c, d, b))
}

/// Whether `lake` touches or overlaps `water` (rings, world units), within `tol` world units:
/// either's shore within `tol` of the other's, or one inside the other.
pub fn touches(lake: &Lake, water: &[Vec<[f64; 2]>], tol: f64) -> bool {
    if water.is_empty() || lake.rings.is_empty() {
        return false;
    }
    // The lake inside the water, or water inside the lake.
    if winding(water, lake.rings[0][0]) != 0 {
        return true;
    }
    if water.iter().any(|r| winding(&lake.rings, r[0]) != 0) {
        return true;
    }
    // Shores within `tol`: the water's edges filed in a grid over the lake's box.
    let b = lake.bbox();
    let b = [b[0] - tol, b[1] - tol, b[2] + tol, b[3] + tol];
    let cell = (tol * 2.0).max((b[2] - b[0]).max(b[3] - b[1]) / 256.0).max(1e-12);
    let cell_of = |v: f64, o: f64| ((v - o) / cell).floor() as i64;
    let mut grid: HashMap<(i64, i64), Vec<Seg>> = HashMap::new();
    for r in water {
        for i in 0..r.len() {
            let (p, q) = (r[i], r[(i + 1) % r.len()]);
            if p[0].max(q[0]) < b[0] || p[0].min(q[0]) > b[2] || p[1].max(q[1]) < b[1] || p[1].min(q[1]) > b[3] {
                continue;
            }
            let (x0, x1) = (cell_of(p[0].min(q[0]).max(b[0]), b[0]), cell_of(p[0].max(q[0]).min(b[2]), b[0]));
            let (y0, y1) = (cell_of(p[1].min(q[1]).max(b[1]), b[1]), cell_of(p[1].max(q[1]).min(b[3]), b[1]));
            if (x1 - x0 + 1) * (y1 - y0 + 1) > 4096 {
                // A long edge across the box: filed in the cells along it would be many; a few
                // cells' worth of samples is enough to find it.
                for k in 0..=64 {
                    let t = f64::from(k) / 64.0;
                    let s = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                    grid.entry((cell_of(s[0], b[0]), cell_of(s[1], b[1]))).or_default().push((p, q));
                }
                continue;
            }
            for y in y0..=y1 {
                for x in x0..=x1 {
                    grid.entry((x, y)).or_default().push((p, q));
                }
            }
        }
    }
    let tol2 = tol * tol;
    for r in &lake.rings {
        for i in 0..r.len() {
            let (p, q) = (r[i], r[(i + 1) % r.len()]);
            let (x0, x1) = (cell_of(p[0].min(q[0]) - tol, b[0]), cell_of(p[0].max(q[0]) + tol, b[0]));
            let (y0, y1) = (cell_of(p[1].min(q[1]) - tol, b[1]), cell_of(p[1].max(q[1]) + tol, b[1]));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if let Some(segs) = grid.get(&(x, y)) {
                        if segs.iter().any(|&(c, d)| seg_seg2(p, q, c, d) <= tol2) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

// ---- the rules ------------------------------------------------------------------------------------

/// OSM's water, as the basemap draws it, near a box.
pub trait OsmWater: Sync {
    /// Told the boxes it will be asked about, first (to read them in long reads).
    fn prepare(&self, _boxes: &[[f64; 4]]) -> Result<()> {
        Ok(())
    }
    /// The rings (world units) of the water meeting box `b` (min x, min y, max x, max y).
    fn rings_near(&self, b: [f64; 4]) -> Result<Vec<Vec<[f64; 2]>>>;
}

/// The basemap's water at z14 (`water::tile_water`: sea and inland, not in tunnels), as the water
/// layer draws it: the tiles under the boxes `prepare` is given, read in long reads (a z7 tile's at
/// a time) and kept.
pub struct BasemapOsm {
    pm: store::pmtiles::PmTiles,
    tiles: std::sync::RwLock<HashMap<(u32, u32), std::sync::Arc<Rings>>>,
}

impl BasemapOsm {
    pub fn new(pm: store::pmtiles::PmTiles) -> BasemapOsm {
        BasemapOsm { pm, tiles: std::sync::RwLock::new(HashMap::new()) }
    }

    fn range(b: [f64; 4]) -> (u32, u32, u32, u32) {
        let n = f64::from(1u32 << crate::water::BASE_Z);
        let t = |v: f64| (v.clamp(0.0, 1.0) * n).floor().min(n - 1.0) as u32;
        (t(b[0]), t(b[1]), t(b[2]), t(b[3]))
    }
}

impl OsmWater for BasemapOsm {
    fn prepare(&self, boxes: &[[f64; 4]]) -> Result<()> {
        use rayon::prelude::*;
        let z = crate::water::BASE_Z;
        let mut by7: BTreeMap<(u32, u32), HashSet<(u32, u32)>> = BTreeMap::new();
        for b in boxes {
            let (x0, y0, x1, y1) = Self::range(*b);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    by7.entry((x >> 7, y >> 7)).or_default().insert((x, y));
                }
            }
        }
        let base = |z: u8| ((1u64 << (2 * u32::from(z))) - 1) / 3;
        let data = self.pm.header().data_offset;
        let got: Vec<Vec<((u32, u32), Rings)>> = by7
            .par_iter()
            .map(|(&(x7, y7), want)| -> Result<Vec<((u32, u32), Rings)>> {
                let h7 = store::pmtiles::zxy_to_tile_id(7, x7, y7)? - base(7);
                let lo = base(z) + (h7 << 14);
                let mut es = Vec::new();
                self.pm.entries_in(lo, lo + (1 << 14), &mut |e| es.push(e))?;
                let mut tiles: Vec<((u32, u32), u64, u32)> = Vec::new();
                for e in &es {
                    for id in e.tile_id..e.tile_id + u64::from(e.run_length) {
                        let (_, x, y) = store::pmtiles::tile_id_to_zxy(id)?;
                        if want.contains(&(x, y)) {
                            tiles.push(((x, y), data + e.offset, e.length));
                        }
                    }
                }
                let mut offs: Vec<(u64, u32)> = tiles.iter().map(|t| (t.1, t.2)).collect();
                offs.sort_unstable();
                offs.dedup();
                let mut spans: Vec<(u64, u64)> = Vec::new();
                for (o, l) in offs {
                    match spans.last_mut() {
                        Some(s) if o <= s.1 + (1 << 20) && o + u64::from(l) - s.0 <= 256 << 20 => s.1 = s.1.max(o + u64::from(l)),
                        _ => spans.push((o, o + u64::from(l))),
                    }
                }
                let mut bufs: Vec<(u64, Vec<u8>)> = Vec::new();
                for (a, b) in spans {
                    bufs.push((a, self.pm.source().read_at(a, usize::try_from(b - a)?)?));
                }
                let n = f64::from(1u32 << z);
                let mut out = Vec::with_capacity(tiles.len());
                for ((x, y), o, l) in tiles {
                    let k = bufs.partition_point(|b| b.0 <= o) - 1;
                    let (a, ref buf) = bufs[k];
                    let tw = crate::water::tile_water(&buf[(o - a) as usize..(o - a) as usize + l as usize])?;
                    let rings = tw.rings.into_iter().map(|(_, r)| r.into_iter().map(|p| [(f64::from(x) + p[0] / tw.extent) / n, (f64::from(y) + p[1] / tw.extent) / n]).collect()).collect();
                    out.push(((x, y), rings));
                }
                Ok(out)
            })
            .collect::<Result<_>>()?;
        let mut t = self.tiles.write().unwrap();
        for v in got {
            for (k, r) in v {
                t.insert(k, std::sync::Arc::new(r));
            }
        }
        Ok(())
    }

    fn rings_near(&self, b: [f64; 4]) -> Result<Vec<Vec<[f64; 2]>>> {
        let (x0, y0, x1, y1) = Self::range(b);
        let t = self.tiles.read().unwrap();
        let mut out = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                if let Some(r) = t.get(&(x, y)) {
                    out.extend(r.iter().cloned());
                }
            }
        }
        Ok(out)
    }
}

/// The rules' settings.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Rules {
    /// A lake this close to OSM's water (m) touches it.
    pub tol_m: f64,
    /// OSM's halves completed: a lake touching OSM's water only across a gap sheet's edge (OSM's
    /// lake cut at the sheet's line by the import, its other half never imported) is taken as its
    /// part in the gap sheets, cut at their edges, so the two abut and the lake reads whole.
    pub complete_halves: bool,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { tol_m: 10.0, complete_halves: true }
    }
}

/// What `select` found, a sheet at a time: the source's lakes meeting it (counted in the sheet of
/// their first point in a gap sheet), how many touch OSM's water, how many are kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct SheetCheck {
    pub source: u32,
    pub touching: u32,
    pub kept: u32,
    /// Of those touching, those completing OSM's halves (`Rules::complete_halves`), kept.
    pub completed: u32,
}

/// What `select` did.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Stats {
    /// The source's pieces read, those dropped as doubles and as inside another of their lake.
    pub pieces: u64,
    pub doubles: u64,
    pub inside: u64,
    /// Lakes (pieces joined), those meeting a gap sheet, those touching OSM's water, those kept.
    pub lakes: u64,
    pub in_gaps: u64,
    pub touching: u64,
    pub kept: u64,
    /// Of those touching, those completing OSM's halves, kept (their parts in the gap sheets).
    pub completed: u64,
}

/// The lakes kept, and what was found.
pub struct Selected {
    pub lakes: Vec<Lake>,
    /// Each kept lake's gap sheet (where it's counted).
    pub sheet_of: Vec<nts::Key>,
    pub stats: Stats,
    pub sheets: BTreeMap<nts::Key, SheetCheck>,
}

struct Dsu(Vec<u32>);

impl Dsu {
    fn find(&mut self, mut i: u32) -> u32 {
        while self.0[i as usize] != i {
            let p = self.0[self.0[i as usize] as usize];
            self.0[i as usize] = p;
            i = p;
        }
        i
    }
    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            let (lo, hi) = (a.min(b), a.max(b));
            self.0[hi as usize] = lo;
        }
    }
}

/// The gap sheet a lake is counted in: that of its first point in one, else one whose middle it
/// holds (a lake round a sheet); None when it meets none.
fn gap_sheet(l: &Lake, gap: &(dyn Fn(nts::Key) -> bool + Sync)) -> Option<nts::Key> {
    for p in l.rings.iter().flatten() {
        let (lon, lat) = lonlat(*p);
        let k = nts::sheet_key(lon, lat);
        if gap(k) {
            return Some(k);
        }
    }
    let b = l.bbox();
    let (w, n) = lonlat([b[0], b[1]]);
    let (e, s) = lonlat([b[2], b[3]]);
    let (k0, k1) = (nts::sheet_key(w, s), nts::sheet_key(e, n));
    for row in k0.1..=k1.1 {
        let lat = f64::from(row) / 4.0 + 0.1;
        let (c0, c1) = (nts::sheet_key(w, lat).0, nts::sheet_key(e, lat).0);
        if c1 - c0 > 4096 {
            continue;
        }
        for col in c0..=c1 {
            let k = (col, row);
            if gap(k) {
                let (sw, ss, se, sn) = nts::bounds(k);
                if winding(&l.rings, crate::watercov::world((sw + se) / 2.0, (ss + sn) / 2.0)) != 0 {
                    return Some(k);
                }
            }
        }
    }
    None
}

/// The source's pieces (`pieces`) as whole lakes, those meeting a gap sheet (`gap`) and touching
/// none of OSM's water (`osm`) kept, by the module's rules. Deterministic: the lakes come out in
/// the order of their first piece.
pub fn select(pieces: Vec<Lake>, gap: &(dyn Fn(nts::Key) -> bool + Sync), osm: &dyn OsmWater, rules: &Rules) -> Result<Selected> {
    use rayon::prelude::*;
    let mut stats = Stats { pieces: pieces.len() as u64, ..Default::default() };
    // Doubles: the same points, however the rings start.
    let mut seen: HashSet<u64> = HashSet::new();
    let mut ps: Vec<Lake> = Vec::with_capacity(pieces.len());
    for p in pieces {
        if seen.insert(p.key()) {
            ps.push(p);
        } else {
            stats.doubles += 1;
        }
    }
    // Pieces of one lake: sharing an edge, or lying along the same straight north–south or
    // east–west line (a sheet's edge) over some length.
    let n = ps.len();
    let mut dsu = Dsu((0..n as u32).collect());
    let mut edges: HashMap<(u64, u64), u32> = HashMap::new();
    // (Per line, east–west or not and where: its pieces' spans along it, (from, to, piece).)
    type Spans = Vec<(u32, u32, u32)>;
    let mut lines: HashMap<(bool, u32), Spans> = HashMap::new();
    for (i, p) in ps.iter().enumerate() {
        for r in &p.rings {
            for k in 0..r.len() {
                let (a, b) = (r[k], r[(k + 1) % r.len()]);
                let (qa, qb) = ([quant(a[0]), quant(a[1])], [quant(b[0]), quant(b[1])]);
                let (ka, kb) = ((u64::from(qa[0]) << 32) | u64::from(qa[1]), (u64::from(qb[0]) << 32) | u64::from(qb[1]));
                match edges.entry((ka.min(kb), ka.max(kb))) {
                    std::collections::hash_map::Entry::Occupied(e) => dsu.union(*e.get(), i as u32),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(i as u32);
                    }
                }
                if qa[0] == qb[0] && qa[1] != qb[1] {
                    lines.entry((false, qa[0])).or_default().push((qa[1].min(qb[1]), qa[1].max(qb[1]), i as u32));
                } else if qa[1] == qb[1] && qa[0] != qb[0] {
                    lines.entry((true, qa[1])).or_default().push((qa[0].min(qb[0]), qa[0].max(qb[0]), i as u32));
                }
            }
        }
    }
    let mut line_keys: Vec<_> = lines.keys().copied().collect();
    line_keys.sort_unstable();
    for k in line_keys {
        let segs = lines.get_mut(&k).unwrap();
        segs.sort_unstable();
        let mut active: Option<(u32, u32)> = None;
        for &(s, e, i) in segs.iter() {
            match active {
                Some((end, j)) if s < end => {
                    dsu.union(i, j);
                    if e > end {
                        active = Some((e, i));
                    }
                }
                Some((end, _)) if e <= end => {}
                _ => active = Some((e, i)),
            }
        }
    }
    let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for i in 0..n as u32 {
        let r = dsu.find(i);
        groups.entry(r).or_default().push(i);
    }
    // A piece wholly inside another of its lake (a copy cut differently) is dropped.
    let mut dropped = vec![false; n];
    for members in groups.values() {
        if members.len() < 2 || members.len() > 64 {
            continue;
        }
        for &a in members {
            for &b in members {
                if a == b || dropped[b as usize] || dropped[a as usize] {
                    continue;
                }
                let (pa, pb) = (&ps[a as usize], &ps[b as usize]);
                let (ba, bb) = (pa.bbox(), pb.bbox());
                if ba[0] < bb[0] || ba[1] < bb[1] || ba[2] > bb[2] || ba[3] > bb[3] {
                    continue;
                }
                // (A point on B's shore, within the grid's step, counts as inside.)
                let eps2 = (2.0 / SCALE) * (2.0 / SCALE);
                let on_b = |p: [f64; 2]| pb.rings.iter().any(|r| (0..r.len()).any(|k| seg_point2(r[k], r[(k + 1) % r.len()], p) <= eps2));
                if pa.rings.iter().flatten().all(|p| winding(&pb.rings, *p) != 0 || on_b(*p)) {
                    dropped[a as usize] = true;
                }
            }
        }
    }
    stats.inside = dropped.iter().filter(|&&d| d).count() as u64;
    let lakes: Vec<Lake> = groups
        .values()
        .filter_map(|m| {
            let rings: Vec<Vec<[f64; 2]>> = m.iter().filter(|&&i| !dropped[i as usize]).flat_map(|&i| ps[i as usize].rings.clone()).collect();
            // (Intermittent only when all of it is.)
            let intermittent = m.iter().filter(|&&i| !dropped[i as usize]).all(|&i| ps[i as usize].intermittent);
            (!rings.is_empty()).then_some(Lake { rings, intermittent })
        })
        .collect();
    stats.lakes = lakes.len() as u64;
    // Each lake: the gap sheet it's counted in (its first point in one), else the sheet of a gap
    // sheet's middle inside it; then whether it touches OSM's water.
    let in_sheet: Vec<Option<nts::Key>> = lakes.par_iter().map(|l| gap_sheet(l, gap)).collect();
    let tol_of = |l: &Lake| {
        let b = l.bbox();
        let (_, lat) = lonlat([(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]);
        rules.tol_m / metres_per_unit(lat)
    };
    let grown = |l: &Lake| {
        let (b, t) = (l.bbox(), tol_of(l));
        [b[0] - t, b[1] - t, b[2] + t, b[3] + t]
    };
    let boxes: Vec<[f64; 4]> = lakes.iter().zip(&in_sheet).filter(|(_, k)| k.is_some()).map(|(l, _)| grown(l)).collect();
    osm.prepare(&boxes)?;
    // (A gap sheet's box in world units, shrunk by `by` each way.)
    let sheet_box = |k: nts::Key, by: f64| {
        let (w, s, e, n) = nts::bounds(k);
        let (a, b) = (crate::watercov::world(w, n), crate::watercov::world(e, s));
        [a[0] + by, a[1] + by, b[0] - by, b[1] - by]
    };
    // The gap sheets a lake's box meets.
    let gaps_of = |l: &Lake| -> Vec<nts::Key> {
        let b = l.bbox();
        let (w, n) = lonlat([b[0], b[1]]);
        let (e, s) = lonlat([b[2], b[3]]);
        let mut v = Vec::new();
        for row in (s * 4.0).floor() as i32..=(n * 4.0).floor() as i32 {
            let wd = nts::width(row);
            for col in (w / wd).floor() as i32..=(e / wd).floor() as i32 {
                if gap((col, row)) {
                    v.push((col, row));
                }
            }
        }
        v
    };
    // A lake's part in boxes (each ring clipped to each; the parts abut along the boxes' edges).
    let part_in = |l: &Lake, boxes: &[[f64; 4]]| -> Lake {
        let rings = boxes.iter().flat_map(|b| l.rings.iter().map(move |r| clip(r, *b))).filter(|r| r.len() >= 3 && crate::watercov::area2(r) != 0.0).collect();
        Lake { rings, intermittent: l.intermittent }
    };
    // Each lake meeting a gap sheet: whether it touches OSM's water, and when it does only across
    // a gap sheet's edge (and halves are completed), its part in the gap sheets.
    let judged: Vec<Option<(nts::Key, bool, Option<Lake>)>> = lakes
        .par_iter()
        .zip(&in_sheet)
        .map(|(l, k)| -> Result<Option<(nts::Key, bool, Option<Lake>)>> {
            let Some(k) = *k else { return Ok(None) };
            let tol = tol_of(l);
            let water = osm.rings_near(grown(l))?;
            if !touches(l, &water, tol) {
                return Ok(Some((k, false, None)));
            }
            if !rules.complete_halves {
                return Ok(Some((k, true, None)));
            }
            // Within the gap sheets (less a margin along their edges, where OSM's halves end), does
            // the lake touch any of OSM's water? If not, its part in them completes OSM's.
            let sheets = gaps_of(l);
            let inner: Vec<[f64; 4]> = sheets.iter().map(|&s| sheet_box(s, 2.0 * tol)).collect();
            let water_in: Vec<Vec<[f64; 2]>> = inner.iter().flat_map(|b| water.iter().map(move |r| clip(r, *b))).filter(|r| r.len() >= 3).collect();
            let lake_in = part_in(l, &inner);
            if lake_in.rings.is_empty() || touches(&lake_in, &water_in, tol) {
                return Ok(Some((k, true, None)));
            }
            let outer: Vec<[f64; 4]> = sheets.iter().map(|&s| sheet_box(s, 0.0)).collect();
            let part = part_in(l, &outer);
            Ok(Some((k, true, (!part.rings.is_empty()).then_some(part))))
        })
        .collect::<Result<_>>()?;
    let mut sheets: BTreeMap<nts::Key, SheetCheck> = BTreeMap::new();
    let (mut kept, mut sheet_of) = (Vec::new(), Vec::new());
    for (l, j) in lakes.into_iter().zip(judged) {
        let Some((k, touching, part)) = j else { continue };
        stats.in_gaps += 1;
        let c = sheets.entry(k).or_default();
        c.source += 1;
        if touching {
            stats.touching += 1;
            c.touching += 1;
            if let Some(p) = part {
                stats.completed += 1;
                c.completed += 1;
                kept.push(p);
                sheet_of.push(k);
            }
        } else {
            c.kept += 1;
            kept.push(l);
            sheet_of.push(k);
        }
    }
    stats.kept = kept.len() as u64;
    Ok(Selected { lakes: kept, sheet_of, stats, sheets })
}

// ---- the fill's file ------------------------------------------------------------------------------

/// The fill's logical name in the manifest and catalog: the water layer and the terrain draw it
/// when it's there (content-named, `work/lakefill.<hash>.sect`).
pub const LOGICAL: &str = "work/lakefill";

/// The fill file's format (meta `fmt`).
pub const FORMAT: &str = "lakefill-1";
/// Its grid's zoom: lakes filed by their boxes' z8 tiles.
pub const GRID_Z: u8 = 8;

/// A fill lake's id's bit saying it's intermittent.
pub const INTERMITTENT: u64 = 1 << 61;

/// A lake in the fill: its key (bit 62 set, never an OSM id; bit 61 `INTERMITTENT`), its box
/// (× 2³²), its rings.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct LakeRec {
    pub id: u64,
    pub bbox: [u32; 4],
    pub first_ring: u32,
    pub nrings: u32,
}

/// Lakes' ids (ascending) as one key, never 0.
fn ids_key(ids: &[u64]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytemuck::cast_slice(ids)) | 1
}

/// Writes the fill: `lakes` (in order), with `meta` added to the file's meta.
pub fn write(path: &Path, lakes: &[Lake], meta: serde_json::Value) -> Result<()> {
    let mut recs = Vec::with_capacity(lakes.len());
    let mut rings: Vec<[u32; 2]> = Vec::new();
    let mut points: Vec<[u32; 2]> = Vec::new();
    for l in lakes {
        let first_ring = u32::try_from(rings.len())?;
        let mut b = [u32::MAX, u32::MAX, 0, 0];
        for r in &l.rings {
            rings.push([u32::try_from(points.len())?, u32::try_from(r.len())?]);
            for p in r {
                let q = [quant(p[0]), quant(p[1])];
                b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])];
                points.push(q);
            }
        }
        let id = (1u64 << 62) | (l.key() & ((1u64 << 61) - 1)) | if l.intermittent { INTERMITTENT } else { 0 };
        recs.push(LakeRec { id, bbox: b, first_ring, nrings: u32::try_from(l.rings.len())? });
    }
    let n = 1usize << GRID_Z;
    let shift = 32 - u32::from(GRID_Z);
    let cells_of = |b: &[u32; 4]| ((b[0] >> shift) as usize..=(b[2] >> shift) as usize, (b[1] >> shift) as usize..=(b[3] >> shift) as usize);
    let mut grid = vec![0u32; n * n + 1];
    for r in &recs {
        let (xs, ys) = cells_of(&r.bbox);
        for y in ys {
            for x in xs.clone() {
                grid[y * n + x + 1] += 1;
            }
        }
    }
    for i in 1..grid.len() {
        grid[i] += grid[i - 1];
    }
    let mut at = grid.clone();
    let mut cells = vec![0u32; grid[n * n] as usize];
    for (i, r) in recs.iter().enumerate() {
        let (xs, ys) = cells_of(&r.bbox);
        for y in ys {
            for x in xs.clone() {
                cells[at[y * n + x] as usize] = i as u32;
                at[y * n + x] += 1;
            }
        }
    }
    let mut m = serde_json::json!({"fmt": FORMAT, "grid_z": GRID_Z, "lakes": recs.len(), "rings": rings.len(), "points": points.len()});
    if let (Some(o), serde_json::Value::Object(extra)) = (m.as_object_mut(), meta) {
        for (k, v) in extra {
            o.entry(k).or_insert(v);
        }
    }
    let mut w = store::sect::SectWriter::create(path, m)?;
    w.add_pod("lakes", &recs)?;
    w.add_pod("rings", &rings)?;
    w.add_pod("points", &points)?;
    w.add_pod("grid", &grid)?;
    w.add_pod("cells", &cells)?;
    w.finish()?;
    Ok(())
}

/// A fill file's bytes in place: memory-mapped from a local disk, else read whole (a NAS file is
/// never mapped: store::range).
pub trait FillBytes: store::RangeRead + store::range::Mapped {}
impl<T: store::RangeRead + store::range::Mapped> FillBytes for T {}

/// The fill, open: its sections cast in place.
pub struct Fill {
    r: store::sect::SectReader<Box<dyn FillBytes>>,
    name: String,
}

/// A tile's box (× 2³²), grown by `buf` of its side each way.
fn tile_box(z: u8, x: u32, y: u32, buf: f64) -> [f64; 4] {
    let n = f64::from(1u32 << z);
    [(f64::from(x) - buf) / n, (f64::from(y) - buf) / n, (f64::from(x) + 1.0 + buf) / n, (f64::from(y) + 1.0 + buf) / n]
}

/// A ring clipped to a box (Sutherland–Hodgman): what of it lies inside, its winding there kept.
fn clip(r: &[[f64; 2]], b: [f64; 4]) -> Vec<[f64; 2]> {
    let mut cur: Vec<[f64; 2]> = r.to_vec();
    for side in 0..4 {
        if cur.is_empty() {
            break;
        }
        let inside = |p: [f64; 2]| match side {
            0 => p[0] >= b[0],
            1 => p[1] >= b[1],
            2 => p[0] <= b[2],
            _ => p[1] <= b[3],
        };
        let cut = |p: [f64; 2], q: [f64; 2]| {
            let t = match side {
                0 => (b[0] - p[0]) / (q[0] - p[0]),
                1 => (b[1] - p[1]) / (q[1] - p[1]),
                2 => (b[2] - p[0]) / (q[0] - p[0]),
                _ => (b[3] - p[1]) / (q[1] - p[1]),
            };
            let mut s = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
            match side {
                0 => s[0] = b[0],
                1 => s[1] = b[1],
                2 => s[0] = b[2],
                _ => s[1] = b[3],
            }
            s
        };
        let mut out = Vec::with_capacity(cur.len() + 4);
        for i in 0..cur.len() {
            let (p, q) = (cur[i], cur[(i + 1) % cur.len()]);
            match (inside(p), inside(q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(cut(p, q)),
                (false, true) => {
                    out.push(cut(p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
        cur = out;
    }
    cur
}

impl Fill {
    /// The fill at `path`, pinned by `name` (its content name): mapped where the file is on this
    /// Mac's disk, read whole from the NAS.
    pub fn open(path: &Path, name: &str) -> Result<Fill> {
        let src: Box<dyn FillBytes> = match store::MmapFile::open(path) {
            Ok(m) => Box::new(m),
            Err(_) => Box::new(std::fs::read(path).with_context(|| format!("read {}", path.display()))?),
        };
        Fill::from_source(src, name).with_context(|| format!("{}", path.display()))
    }

    /// The fill from its file's bytes (mapped or in memory), pinned by `name`.
    pub fn from_source(src: Box<dyn FillBytes>, name: &str) -> Result<Fill> {
        let r = store::sect::SectReader::open(src)?;
        ensure!(r.meta().get("fmt").and_then(|v| v.as_str()) == Some(FORMAT), "not {FORMAT}");
        ensure!(r.meta().get("grid_z").and_then(serde_json::Value::as_u64) == Some(u64::from(GRID_Z)), "another grid");
        Ok(Fill { r, name: name.to_string() })
    }

    /// What pins it: its content name.
    pub fn pin(&self) -> &str {
        &self.name
    }

    pub fn meta(&self) -> &serde_json::Value {
        self.r.meta()
    }

    pub fn lakes(&self) -> &[LakeRec] {
        self.r.cast("lakes").unwrap_or(&[])
    }

    fn rings(&self) -> &[[u32; 2]] {
        self.r.cast("rings").unwrap_or(&[])
    }

    fn points(&self) -> &[[u32; 2]] {
        self.r.cast("points").unwrap_or(&[])
    }

    /// Lake `i`'s rings (world units).
    pub fn lake(&self, i: u32) -> Lake {
        let rec = self.lakes()[i as usize];
        let (rings, pts) = (self.rings(), self.points());
        let rs = &rings[rec.first_ring as usize..(rec.first_ring + rec.nrings) as usize];
        Lake { rings: rs.iter().map(|r| pts[r[0] as usize..(r[0] + r[1]) as usize].iter().map(|p| [f64::from(p[0]) / SCALE, f64::from(p[1]) / SCALE]).collect()).collect(), intermittent: rec.id & INTERMITTENT != 0 }
    }

    /// The lakes whose boxes meet box `b` (world units), ascending.
    pub fn meeting(&self, b: [f64; 4]) -> Vec<u32> {
        let q = |v: f64| (v.clamp(0.0, 1.0) * SCALE).min(SCALE - 1.0) as u64;
        let (x0, y0, x1, y1) = (q(b[0]), q(b[1]), q(b[2]), q(b[3]));
        let recs = self.lakes();
        let meets = |r: &LakeRec| u64::from(r.bbox[0]) <= x1 && u64::from(r.bbox[2]) >= x0 && u64::from(r.bbox[1]) <= y1 && u64::from(r.bbox[3]) >= y0;
        let shift = 32 - u32::from(GRID_Z);
        let n = 1usize << GRID_Z;
        let (cx0, cy0, cx1, cy1) = ((x0 >> shift) as usize, (y0 >> shift) as usize, (x1 >> shift) as usize, (y1 >> shift) as usize);
        let mut out: Vec<u32> = if (cx1 - cx0 + 1) * (cy1 - cy0 + 1) > 64 {
            (0..recs.len() as u32).filter(|&i| meets(&recs[i as usize])).collect()
        } else {
            let (grid, cells): (&[u32], &[u32]) = (self.r.cast("grid").unwrap_or(&[]), self.r.cast("cells").unwrap_or(&[]));
            let mut v = Vec::new();
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    let c = cy * n + cx;
                    v.extend(cells[grid[c] as usize..grid[c + 1] as usize].iter().copied().filter(|&i| meets(&recs[i as usize])));
                }
            }
            v
        };
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Whether any lake's box meets tile z/x/y.
    pub fn any_in(&self, z: u8, x: u32, y: u32) -> bool {
        !self.meeting(tile_box(z, x, y, 0.0)).is_empty()
    }

    /// The key of tile z/x/y's lakes (those whose boxes meet it, by their keys): 0 for none. What
    /// a piece of terrain or water under the tile keys on, for the fill.
    pub fn tile_key(&self, z: u8, x: u32, y: u32) -> u64 {
        let ids: Vec<u64> = self.meeting(tile_box(z, x, y, 0.0)).iter().map(|&i| self.lakes()[i as usize].id).collect();
        if ids.is_empty() {
            return 0;
        }
        ids_key(&ids)
    }

    /// Tile z/x/y's lakes: each lake's id and its rings in tile units (`extent` a side), clipped to
    /// the tile grown by a sixteenth each way (what's beyond adds nothing to a drawing of the tile).
    pub fn in_tile(&self, z: u8, x: u32, y: u32, extent: f64) -> Vec<(u64, Vec<Vec<[f64; 2]>>)> {
        let b = tile_box(z, x, y, 1.0 / 16.0);
        let n = f64::from(1u32 << z);
        let mut out = Vec::new();
        for i in self.meeting(tile_box(z, x, y, 0.0)) {
            let l = self.lake(i);
            let rings: Vec<Vec<[f64; 2]>> = l
                .rings
                .iter()
                .map(|r| clip(r, b))
                .filter(|r| r.len() >= 3)
                .map(|r| r.iter().map(|p| [(p[0] * n - f64::from(x)) * extent, (p[1] * n - f64::from(y)) * extent]).collect())
                .collect();
            if !rings.is_empty() {
                out.push((self.lakes()[i as usize].id, rings));
            }
        }
        out
    }

    /// Tile z/x/y's lakes as a basemap tile (MVT, a `water` layer of class `lake`, extent 4096), or
    /// None when it has none: what the water layer draws beside the basemap's own tile.
    pub fn mvt(&self, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
        let lakes = self.in_tile(z, x, y, 4096.0);
        if lakes.is_empty() {
            return None;
        }
        let mut features = Vec::new();
        for (id, rings) in lakes {
            let mut g: Vec<u32> = Vec::new();
            let (mut cx, mut cy) = (0i64, 0i64);
            let zz = |v: i64| ((v << 1) ^ (v >> 63)) as u32;
            for r in rings {
                let mut pts: Vec<[i64; 2]> = r.iter().map(|p| [p[0].round() as i64, p[1].round() as i64]).collect();
                pts.dedup();
                if pts.len() > 1 && pts.first() == pts.last() {
                    pts.pop();
                }
                if pts.len() < 3 {
                    continue;
                }
                g.push(1 | (1 << 3));
                g.push(zz(pts[0][0] - cx));
                g.push(zz(pts[0][1] - cy));
                (cx, cy) = (pts[0][0], pts[0][1]);
                g.push(2 | ((pts.len() as u32 - 1) << 3));
                for p in &pts[1..] {
                    g.push(zz(p[0] - cx));
                    g.push(zz(p[1] - cy));
                    (cx, cy) = (p[0], p[1]);
                }
                g.push(7 | (1 << 3));
            }
            if !g.is_empty() {
                features.push(names::mvt::Feature { id: Some(id), tags: vec![0, 0], geom_type: Some(3), geometry: g, unknown: Vec::new() });
            }
        }
        if features.is_empty() {
            return None;
        }
        let layer = names::mvt::Layer { name: "water".into(), version: 2, extent: 4096, keys: vec!["class".into()], values: vec![names::mvt::Value::String("lake".into())], features, unknown: Vec::new() };
        Some(names::mvt::Tile { layers: vec![layer], unknown: Vec::new() }.encode())
    }
}

impl crate::water::Extra for Fill {
    fn any(&self, z: u8, x: u32, y: u32) -> bool {
        self.any_in(z, x, y)
    }

    fn z14(&self, x: u32, y: u32) -> Option<crate::water::Stored> {
        let bytes = self.mvt(crate::water::BASE_Z, x, y)?;
        // (A key of its own: the fill's name and the tile's lakes.)
        let key = xxhash_rust::xxh3::xxh3_64(format!("{} {}", self.name, self.tile_key(crate::water::BASE_Z, x, y)).as_bytes());
        Some(crate::water::Stored { key, bytes: std::sync::Arc::new(bytes) })
    }
}

/// The terrain's water: the basemap's (`base`), with the fill's lakes added when there's a fill
/// (lakes, by their keys, flattened to their shores' level as OSM's are).
pub struct FilledWater<W> {
    pub base: W,
    pub fill: Option<Fill>,
}

impl<W: crate::terrain_water::WaterSource> crate::terrain_water::WaterSource for FilledWater<W> {
    fn polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<crate::terrain_water::Poly>> {
        let mut out = self.base.polys(z, x, y)?;
        if let Some(f) = self.fill.as_ref().filter(|_| (6..=14).contains(&z)) {
            for (id, rings) in f.in_tile(z, x, y, 256.0) {
                if id & INTERMITTENT != 0 {
                    continue;
                }
                out.push(crate::terrain_water::Poly { kind: crate::terrain_water::Kind::Lake, id, rings });
            }
        }
        Ok(out)
    }

    /// `filled_pin`'s.
    fn pin(&self) -> String {
        filled_pin(&self.base.pin(), self.fill.as_ref().map(Fill::pin))
    }
}

/// The pin of the basemap's water `base` with the fill `fill` (its content name) added: the
/// basemap's alone without one, so the terrain's keys are as they were.
pub fn filled_pin(base: &str, fill: Option<&str>) -> String {
    match fill {
        Some(f) => format!("{base} + {f}"),
        None => base.to_string(),
    }
}

// ---- the source's shapefiles ------------------------------------------------------------------------

/// ESRI shapefiles, as far as reading polygons and their attributes needs, streamed (a province's
/// layer is gigabytes: read record by record from `unzip -p`, never unpacked).
pub mod shp {
    use anyhow::{bail, ensure, Context, Result};
    use std::io::Read;

    /// A record's rings, as its points (x, y) are.
    pub type Polygon = Vec<Vec<[f64; 2]>>;

    /// A `.shp`'s records, one at a time: polygons (types 5, 15, 25) as their rings, any other
    /// shape (or a null one) empty.
    pub struct ShpReader<R: Read> {
        r: R,
        buf: Vec<u8>,
        /// The file's box: x min, y min, x max, y max.
        pub bbox: [f64; 4],
    }

    impl<R: Read> ShpReader<R> {
        pub fn new(mut r: R) -> Result<Self> {
            let mut h = [0u8; 100];
            r.read_exact(&mut h).context("a shapefile's header")?;
            ensure!(h[0..4] == [0, 0, 0x27, 0x0a], "not a shapefile");
            let f = |o: usize| f64::from_le_bytes(h[o..o + 8].try_into().unwrap());
            Ok(ShpReader { r, buf: Vec::new(), bbox: [f(36), f(44), f(52), f(60)] })
        }

        /// The next record, or None at the end.
        pub fn next_polygon(&mut self) -> Result<Option<Polygon>> {
            let mut rh = [0u8; 8];
            match self.r.read_exact(&mut rh) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            let len = 2 * u32::from_be_bytes(rh[4..8].try_into()?) as usize;
            self.buf.resize(len, 0);
            self.r.read_exact(&mut self.buf).context("a record cut short")?;
            let b = &self.buf;
            let i32le = |o: usize| -> Result<i32> { Ok(i32::from_le_bytes(b.get(o..o + 4).context("cut short")?.try_into()?)) };
            let f64le = |o: usize| -> Result<f64> { Ok(f64::from_le_bytes(b.get(o..o + 8).context("cut short")?.try_into()?)) };
            if !matches!(i32le(0)?, 5 | 15 | 25) {
                return Ok(Some(Vec::new()));
            }
            let (nparts, npoints) = (usize::try_from(i32le(36)?)?, usize::try_from(i32le(40)?)?);
            let parts: Vec<usize> = (0..nparts).map(|k| i32le(44 + 4 * k).map(|v| v as usize)).collect::<Result<_>>()?;
            let p0 = 44 + 4 * nparts;
            ensure!(p0 + 16 * npoints <= len, "a polygon's points run past its record");
            let mut rings = Vec::with_capacity(nparts);
            for (k, &s) in parts.iter().enumerate() {
                let e = parts.get(k + 1).copied().unwrap_or(npoints);
                if s > e || e > npoints {
                    bail!("a polygon's parts out of order");
                }
                rings.push((s..e).map(|j| Ok([f64le(p0 + 16 * j)?, f64le(p0 + 16 * j + 8)?])).collect::<Result<Vec<_>>>()?);
            }
            Ok(Some(rings))
        }

        /// The stream, for reading what's left (to its end, checking it).
        pub fn into_inner(self) -> R {
            self.r
        }
    }

    /// A whole `.shp`'s records.
    pub fn polygons_of(b: &[u8]) -> Result<Vec<Polygon>> {
        let mut r = ShpReader::new(b)?;
        let mut out = Vec::new();
        while let Some(p) = r.next_polygon()? {
            out.push(p);
        }
        Ok(out)
    }

    /// A `.dbf`'s records, one at a time, each value as text (trimmed).
    pub struct DbfReader<R: Read> {
        r: R,
        pub fields: Vec<String>,
        widths: Vec<usize>,
        rlen: usize,
        pub records: usize,
        read: usize,
        buf: Vec<u8>,
    }

    impl<R: Read> DbfReader<R> {
        pub fn new(mut r: R) -> Result<Self> {
            let mut h = [0u8; 32];
            r.read_exact(&mut h).context("a dBASE table's header")?;
            let records = u32::from_le_bytes(h[4..8].try_into()?) as usize;
            let (hlen, rlen) = (u16::from_le_bytes(h[8..10].try_into()?) as usize, u16::from_le_bytes(h[10..12].try_into()?) as usize);
            ensure!(hlen > 32, "not a dBASE table");
            let mut rest = vec![0u8; hlen - 32];
            r.read_exact(&mut rest)?;
            let (mut fields, mut widths) = (Vec::new(), Vec::new());
            let mut o = 0;
            while o + 32 <= rest.len() && rest[o] != 0x0d {
                fields.push(String::from_utf8_lossy(&rest[o..o + 11]).trim_end_matches('\0').trim().to_string());
                widths.push(rest[o + 16] as usize);
                o += 32;
            }
            ensure!(1 + widths.iter().sum::<usize>() <= rlen, "a dBASE table's fields wider than its records");
            Ok(DbfReader { r, fields, widths, rlen, records, read: 0, buf: vec![0; rlen] })
        }

        pub fn col(&self, name: &str) -> Option<usize> {
            self.fields.iter().position(|f| f.eq_ignore_ascii_case(name))
        }

        /// The next record's values, or None after the last.
        pub fn next_row(&mut self) -> Result<Option<Vec<String>>> {
            if self.read == self.records {
                return Ok(None);
            }
            self.r.read_exact(&mut self.buf[..self.rlen]).context("a record cut short")?;
            self.read += 1;
            let mut at = 1;
            let mut row = Vec::with_capacity(self.widths.len());
            for &w in &self.widths {
                row.push(String::from_utf8_lossy(&self.buf[at..at + w]).trim().to_string());
                at += w;
            }
            Ok(Some(row))
        }
    }

    /// A whole `.dbf`'s table.
    pub struct Table {
        pub fields: Vec<String>,
        pub rows: Vec<Vec<String>>,
    }

    impl Table {
        pub fn col(&self, name: &str) -> Option<usize> {
            self.fields.iter().position(|f| f.eq_ignore_ascii_case(name))
        }
    }

    pub fn table_of(b: &[u8]) -> Result<Table> {
        let mut r = DbfReader::new(b)?;
        let mut rows = Vec::new();
        while let Some(row) = r.next_row()? {
            rows.push(row);
        }
        Ok(Table { fields: r.fields, rows })
    }
}

/// Zip archives' members, streamed in long reads (8 MB at a time: over the NAS a reader taking a
/// few kilobytes a request, as `unzip` does, is held to a few MB/s by each request's wait), stored
/// or deflated, Zip64 too, each member's CRC checked at its end.
pub mod zip {
    use anyhow::{bail, ensure, Context, Result};
    use std::io::Read;
    use std::path::Path;
    use store::range::RangeRead;

    /// A member: its name, method, sizes, CRC and where its local header is.
    #[derive(Clone, Debug)]
    pub struct Member {
        pub name: String,
        method: u16,
        crc: u32,
        csize: u64,
        pub size: u64,
        offset: u64,
    }

    fn u16le(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    fn u32le(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }
    fn u64le(b: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
    }

    /// The members listed in `path`'s central directory.
    pub fn members(path: &Path) -> Result<Vec<Member>> {
        let f = store::range::PlainFile::open(path).with_context(|| format!("open {}", path.display()))?;
        let len = f.len()?;
        let tail_len = len.min(70_000);
        let tail = f.read_at(len - tail_len, tail_len as usize)?;
        let e = (0..tail.len().saturating_sub(21)).rev().find(|&i| u32le(&tail, i) == 0x0605_4b50).context("no end of central directory")?;
        let (mut n, mut cd_size, mut cd_off) = (u64::from(u16le(&tail, e + 10)), u64::from(u32le(&tail, e + 12)), u64::from(u32le(&tail, e + 16)));
        if n == 0xffff || cd_size == 0xffff_ffff || cd_off == 0xffff_ffff {
            // Zip64: its locator just before, pointing at its end record.
            ensure!(e >= 20 && u32le(&tail, e - 20) == 0x0706_4b50, "no Zip64 locator");
            let rec = f.read_at(u64le(&tail, e - 20 + 8), 56)?;
            ensure!(u32le(&rec, 0) == 0x0606_4b50, "no Zip64 end record");
            (n, cd_size, cd_off) = (u64le(&rec, 32), u64le(&rec, 40), u64le(&rec, 48));
        }
        let cd = f.read_at(cd_off, usize::try_from(cd_size)?)?;
        let mut out = Vec::with_capacity(n as usize);
        let mut o = 0;
        while o + 46 <= cd.len() && u32le(&cd, o) == 0x0201_4b50 {
            let (method, crc) = (u16le(&cd, o + 10), u32le(&cd, o + 16));
            let (mut csize, mut size) = (u64::from(u32le(&cd, o + 20)), u64::from(u32le(&cd, o + 24)));
            let (nl, el, cl) = (usize::from(u16le(&cd, o + 28)), usize::from(u16le(&cd, o + 30)), usize::from(u16le(&cd, o + 32)));
            let mut offset = u64::from(u32le(&cd, o + 42));
            let name = String::from_utf8_lossy(&cd[o + 46..o + 46 + nl]).into_owned();
            // Zip64's extra field: the sizes and offset that didn't fit, in that order.
            let mut x = o + 46 + nl;
            let xe = x + el;
            while x + 4 <= xe {
                let (id, l) = (u16le(&cd, x), usize::from(u16le(&cd, x + 2)));
                if id == 1 {
                    let mut k = x + 4;
                    for v in [&mut size, &mut csize, &mut offset] {
                        if *v == 0xffff_ffff && k + 8 <= x + 4 + l {
                            *v = u64le(&cd, k);
                            k += 8;
                        }
                    }
                }
                x += 4 + l;
            }
            out.push(Member { name, method, crc, csize, size, offset });
            o = xe + cl;
        }
        Ok(out)
    }

    /// The bytes of a member's data, read 8 MB at a time.
    struct Chunks {
        f: store::range::PlainFile,
        at: u64,
        end: u64,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Read for Chunks {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.pos == self.buf.len() {
                if self.at == self.end {
                    return Ok(0);
                }
                let n = (self.end - self.at).min(8 << 20) as usize;
                self.buf = self.f.read_at(self.at, n).map_err(std::io::Error::other)?;
                self.at += n as u64;
                self.pos = 0;
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// A member's bytes, checked against its CRC and size when the last is read.
    struct Checked<R: Read> {
        r: R,
        crc: crc32fast::Hasher,
        n: u64,
        want: (u32, u64),
        name: String,
        checked: bool,
    }

    impl<R: Read> Read for Checked<R> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = self.r.read(out)?;
            self.crc.update(&out[..n]);
            self.n += n as u64;
            if n == 0 && !out.is_empty() && !self.checked {
                self.checked = true;
                let crc = std::mem::take(&mut self.crc).finalize();
                if (crc, self.n) != self.want {
                    return Err(std::io::Error::other(format!("{}: CRC or size wrong ({crc:08x}, {} bytes; the zip says {:08x}, {})", self.name, self.n, self.want.0, self.want.1)));
                }
            }
            Ok(n)
        }
    }

    /// Member `m` of the zip at `path`, streamed.
    pub fn open(path: &Path, m: &Member) -> Result<Box<dyn Read + Send>> {
        let f = store::range::PlainFile::open(path)?;
        let h = f.read_at(m.offset, 30)?;
        ensure!(u32le(&h, 0) == 0x0403_4b50, "{}: no local header", m.name);
        let start = m.offset + 30 + u64::from(u16le(&h, 26)) + u64::from(u16le(&h, 28));
        let raw = Chunks { f, at: start, end: start + m.csize, buf: Vec::new(), pos: 0 };
        let r: Box<dyn Read + Send> = match m.method {
            0 => Box::new(raw),
            8 => Box::new(flate2::read::DeflateDecoder::new(raw)),
            k => bail!("{}: compression method {k}", m.name),
        };
        Ok(Box::new(std::io::BufReader::with_capacity(1 << 20, Checked { r, crc: crc32fast::Hasher::new(), n: 0, want: (m.crc, m.size), name: m.name.clone(), checked: false })))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn members_are_listed_and_read_and_checked() {
            let d = tempfile::tempdir().unwrap();
            let dir = d.path().join("in");
            std::fs::create_dir_all(&dir).unwrap();
            let big: Vec<u8> = (0..300_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
            std::fs::write(dir.join("a.bin"), &big).unwrap();
            std::fs::write(dir.join("b.txt"), b"hello").unwrap();
            let z = d.path().join("t.zip");
            let ok = std::process::Command::new("/usr/bin/zip").arg("-q").arg("-j").arg(&z).arg(dir.join("a.bin")).arg(dir.join("b.txt")).status().unwrap();
            assert!(ok.success());
            let ms = members(&z).unwrap();
            assert_eq!(ms.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["a.bin", "b.txt"]);
            let mut got = Vec::new();
            open(&z, &ms[0]).unwrap().read_to_end(&mut got).unwrap();
            assert_eq!(got, big);
            // A damaged member fails at its end.
            let mut bad = ms[0].clone();
            bad.crc ^= 1;
            let mut sink = Vec::new();
            assert!(open(&z, &bad).unwrap().read_to_end(&mut sink).is_err());
            // Read past its end again: still fine.
            let mut r = open(&z, &ms[1]).unwrap();
            let mut s = String::new();
            r.read_to_string(&mut s).unwrap();
            assert_eq!(s, "hello");
            assert_eq!(r.read(&mut [0u8; 8]).unwrap(), 0);
        }
    }
}

/// Canada's National Topographic System: its 1:50,000 map sheets.
///
/// - South of 68°N a sheet is 0.5° of longitude by 0.25° of latitude; 68–80°N, 1° by 0.25°; north
///   of 80°N, 2° by 0.25°. Sixteen make a 1:250,000 sheet (4 × 4).
/// - A series (the number) is 8° by 4°, counted west from 48°W (tens) and north from 40°N (units).
///   Its 1:250,000 sheets (2° by 1° south of 68°N, 4° by 1° to 80°N) are lettered from its south-east
///   corner, back and forth row by row (A–D east to west, E–H west to east, …), and a 1:250,000
///   sheet's 1:50,000 sheets numbered 1–16 the same way.
pub mod nts {
    /// A sheet's key: (column, row), the row its 0.25° band north of the equator, the column its
    /// band of the row's width west of Greenwich, counted east (negative in the west).
    pub type Key = (i32, i32);

    /// A row's sheet width (degrees of longitude).
    pub fn width(row: i32) -> f64 {
        let lat = f64::from(row) / 4.0;
        if lat < 68.0 {
            0.5
        } else if lat < 80.0 {
            1.0
        } else {
            2.0
        }
    }

    /// The sheet holding `lon`, `lat`.
    pub fn sheet_key(lon: f64, lat: f64) -> Key {
        let row = (lat * 4.0).floor() as i32;
        ((lon / width(row)).floor() as i32, row)
    }

    /// A sheet's west, south, east and north (degrees).
    pub fn bounds(k: Key) -> (f64, f64, f64, f64) {
        let w = width(k.1);
        (f64::from(k.0) * w, f64::from(k.1) / 4.0, f64::from(k.0 + 1) * w, f64::from(k.1 + 1) / 4.0)
    }

    /// Back and forth from the south-east: the label of cell (`col` from the east, `row` from the
    /// south) of a grid `cols` wide.
    fn snake(col: i32, row: i32, cols: i32) -> i32 {
        if row % 2 == 0 {
            row * cols + col
        } else {
            row * cols + (cols - 1 - col)
        }
    }

    /// A sheet's name, as `33H/16` (north of 80°N, where the series are wider, approximate).
    pub fn name(k: Key) -> String {
        let (w, s, e, _) = bounds(k);
        let (lonw, lat) = (-(w + e) / 2.0, s + 0.125);
        let tens = ((lonw - 48.0) / 8.0).floor() as i32;
        let units = ((lat - 40.0) / 4.0).floor() as i32;
        let series = tens * 10 + units;
        let (east, south) = (48.0 + 8.0 * f64::from(tens), 40.0 + 4.0 * f64::from(units));
        // The 1:250,000 sheet: 2° (4° from 68°N) by 1°.
        let w250 = if lat < 68.0 { 2.0 } else { 4.0 };
        let cols = (8.0 / w250) as i32;
        let (c, r) = (((lonw - east) / w250).floor() as i32, (lat - south).floor() as i32);
        let letter = (b'A' + snake(c, r, cols) as u8) as char;
        // Its 1:50,000 sheet.
        let w50 = w250 / 4.0;
        let (east250, south250) = (east + w250 * f64::from(c), south + f64::from(r));
        let (c5, r5) = (((lonw - east250) / w50).floor() as i32, ((lat - south250) * 4.0).floor() as i32);
        format!("{series:03}{letter}/{:02}", snake(c5, r5, 4) + 1).trim_start_matches('0').to_string()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sheets_are_named_as_nts_names_them() {
            // Montréal: 31H/12 (45.5°N, 73.6°W).
            assert_eq!(name(sheet_key(-73.6, 45.55)), "31H/12");
            // Ottawa: 31G/05 (45.42°N, 75.7°W).
            assert_eq!(name(sheet_key(-75.7, 45.42)), "31G/05");
            // Vancouver: 92G/06 (49.28°N, 123.12°W).
            assert_eq!(name(sheet_key(-123.12, 49.28)), "92G/06");
            // Yellowknife: 85J/08 (62.45°N, 114.37°W).
            assert_eq!(name(sheet_key(-114.37, 62.45)), "85J/08");
            // Toronto: 30M/11 (43.65°N, 79.38°W).
            assert_eq!(name(sheet_key(-79.38, 43.65)), "30M/11");
        }

        #[test]
        fn sheets_widen_northwards() {
            assert_eq!(bounds(sheet_key(-72.1, 53.84)), (-72.5, 53.75, -72.0, 54.0));
            assert_eq!(bounds(sheet_key(-105.5, 69.1)), (-106.0, 69.0, -105.0, 69.25));
            assert_eq!(bounds(sheet_key(-75.5, 81.1)), (-76.0, 81.0, -74.0, 81.25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box of longitude and latitude as a shapefile ring (clockwise: west-south, west-north, …).
    fn sq(w: f64, s: f64, e: f64, n: f64) -> Vec<[f64; 2]> {
        vec![[w, s], [w, n], [e, n], [e, s], [w, s]]
    }

    fn lake(rings: &[Vec<[f64; 2]>]) -> Lake {
        Lake::from_lonlat(rings).unwrap()
    }

    /// OSM's water as rings of longitude and latitude.
    struct Osm(Vec<Vec<[f64; 2]>>);

    impl OsmWater for Osm {
        fn rings_near(&self, _b: [f64; 4]) -> Result<Vec<Vec<[f64; 2]>>> {
            Ok(self.0.iter().map(|r| r.iter().map(|p| crate::watercov::world(p[0], p[1])).collect()).collect())
        }
    }

    /// 33H/16 (72.5–72°W, 53.75–54°N), the owner's gap.
    fn gap(k: nts::Key) -> bool {
        k == nts::sheet_key(-72.25, 53.8)
    }

    fn run(pieces: Vec<Lake>, osm: Vec<Vec<[f64; 2]>>) -> Selected {
        // (Halves not completed: the rules before it, each on its own.)
        select(pieces, &gap, &Osm(osm), &Rules { complete_halves: false, ..Rules::default() }).unwrap()
    }

    #[test]
    fn only_lakes_meeting_a_gap_sheet_are_taken() {
        let inside = lake(&[sq(-72.3, 53.80, -72.29, 53.81)]);
        let outside = lake(&[sq(-71.9, 53.80, -71.89, 53.81)]);
        let s = run(vec![inside.clone(), outside], Vec::new());
        assert_eq!(s.lakes, vec![inside]);
        assert_eq!((s.stats.lakes, s.stats.in_gaps, s.stats.kept), (2, 1, 1));
        let k = nts::sheet_key(-72.25, 53.8);
        assert_eq!(s.sheets[&k], SheetCheck { source: 1, touching: 0, kept: 1, completed: 0 });
    }

    #[test]
    fn a_lake_round_a_whole_gap_sheet_is_taken() {
        // No point of it in the sheet: its middle is inside.
        let big = lake(&[sq(-72.6, 53.7, -71.9, 54.1)]);
        let s = run(vec![big], Vec::new());
        assert_eq!(s.stats.kept, 1);
    }

    #[test]
    fn a_lake_cut_at_a_sheet_s_edge_is_one_lake_taken_whole() {
        // Two pieces sharing the cut along 72°W, the east one outside the gap.
        let west = lake(&[sq(-72.02, 53.80, -72.0, 53.81)]);
        let east = lake(&[sq(-72.0, 53.80, -71.98, 53.81)]);
        let s = run(vec![west.clone(), east.clone()], Vec::new());
        assert_eq!(s.lakes.len(), 1);
        assert_eq!(s.lakes[0].rings.len(), 2);
        assert!((s.lakes[0].area() - (west.area() + east.area())).abs() < 1e-18);
        // Cut with different points along the line (the east piece's edge split in two): still one.
        let east2 = lake(&[vec![[-72.0, 53.80], [-72.0, 53.805], [-72.0, 53.81], [-71.98, 53.81], [-71.98, 53.80]]]);
        let s = run(vec![west, east2], Vec::new());
        assert_eq!((s.stats.lakes, s.lakes.len()), (1, 1));
    }

    #[test]
    fn a_lake_touching_osm_water_is_left_out_with_all_its_pieces() {
        let west = lake(&[sq(-72.02, 53.80, -72.0, 53.81)]);
        let east = lake(&[sq(-72.0, 53.80, -71.98, 53.81)]);
        // OSM's lake beside the east piece, outside the gap sheet, 3 m off (within 10 m).
        let d = 3.0 / 111_320.0 / 53.8f64.to_radians().cos();
        let osm = sq(-71.98 + d, 53.80, -71.97, 53.81);
        let s = run(vec![west.clone(), east.clone()], vec![osm]);
        assert_eq!((s.stats.in_gaps, s.stats.touching, s.stats.kept), (1, 1, 0));
        assert!(s.lakes.is_empty());
        // 50 m off: kept.
        let d = 50.0 / 111_320.0 / 53.8f64.to_radians().cos();
        let s = run(vec![west, east], vec![sq(-71.98 + d, 53.80, -71.97, 53.81)]);
        assert_eq!(s.stats.kept, 1);
    }

    #[test]
    fn osm_s_halves_are_completed_when_asked() {
        // OSM's half of a lake east of 72°W (the gap sheet 33H/16 is west of it), cut at the line.
        let osm_half = sq(-72.0, 53.80, -71.98, 53.81);
        let whole = lake(&[sq(-72.02, 53.80, -71.98, 53.81)]);
        let s = run(vec![whole.clone()], vec![osm_half.clone()]);
        assert_eq!((s.stats.touching, s.stats.kept), (1, 0));
        let rules = Rules::default();
        assert!(rules.complete_halves);
        let s = select(vec![whole], &gap, &Osm(vec![osm_half.clone()]), &rules).unwrap();
        assert_eq!((s.stats.touching, s.stats.completed, s.stats.kept), (1, 1, 1));
        // Its part west of the line, abutting OSM's: the two together are the whole lake.
        let west = lake(&[sq(-72.02, 53.80, -72.0, 53.81)]);
        assert!((s.lakes[0].area() - west.area()).abs() / west.area() < 1e-4, "{} against {}", s.lakes[0].area(), west.area());
        assert!(s.lakes[0].bbox()[2] <= crate::watercov::world(-72.0, 53.8)[0] + 1e-12);
        // OSM's water inside the gap sheet touching it: left out still.
        let pond = sq(-72.019, 53.805, -72.018, 53.806);
        let whole = lake(&[sq(-72.02, 53.80, -71.98, 53.81)]);
        let s = select(vec![whole], &gap, &Osm(vec![osm_half, pond]), &rules).unwrap();
        assert_eq!((s.stats.completed, s.stats.kept), (0, 0));
    }

    #[test]
    fn a_lake_inside_osm_water_or_round_it_is_left_out() {
        let l = lake(&[sq(-72.3, 53.80, -72.29, 53.81)]);
        // OSM's lake holds it.
        assert_eq!(run(vec![l.clone()], vec![sq(-72.31, 53.79, -72.28, 53.82)]).stats.kept, 0);
        // It holds OSM's pond.
        assert_eq!(run(vec![l.clone()], vec![sq(-72.296, 53.804, -72.294, 53.806)]).stats.kept, 0);
        // A pond on its island isn't in it: kept.
        let with_island = lake(&[sq(-72.3, 53.80, -72.29, 53.81), vec![[-72.297, 53.803], [-72.293, 53.803], [-72.293, 53.807], [-72.297, 53.807], [-72.297, 53.803]]]);
        let s = run(vec![with_island], vec![sq(-72.2955, 53.8045, -72.2945, 53.8055)]);
        assert_eq!(s.stats.kept, 1);
    }

    #[test]
    fn a_lake_the_source_has_twice_is_taken_once() {
        let a = lake(&[sq(-72.3, 53.80, -72.29, 53.81)]);
        // The same ring from another corner, and reversed then put right.
        let mut b = a.clone();
        b.rings[0].rotate_left(2);
        let s = run(vec![a.clone(), b], Vec::new());
        assert_eq!((s.stats.doubles, s.stats.kept), (1, 1));
        assert_eq!(s.lakes, vec![a]);
    }

    #[test]
    fn a_piece_inside_another_of_its_lake_is_dropped() {
        // A whole lake, and a copy of its west half cut at 72°W sharing its west edge's line.
        let whole = lake(&[sq(-72.02, 53.80, -71.98, 53.81)]);
        let half = lake(&[sq(-72.02, 53.80, -72.0, 53.81)]);
        let s = run(vec![whole.clone(), half], Vec::new());
        assert_eq!((s.stats.inside, s.stats.kept), (1, 1));
        assert_eq!(s.lakes, vec![whole]);
    }

    #[test]
    fn rings_the_wrong_way_round_are_put_right() {
        let mut r = sq(-72.3, 53.80, -72.29, 53.81);
        r.reverse();
        let l = lake(&[r]);
        assert!(crate::watercov::area2(&l.rings[0]) > 0.0);
    }

    fn written(lakes: &[Lake]) -> (tempfile::TempDir, Fill) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("fill.sect");
        write(&p, lakes, serde_json::json!({"source": "test"})).unwrap();
        let f = Fill::open(&p, "fill-test").unwrap();
        (d, f)
    }

    fn tile_of(lon: f64, lat: f64, z: u8) -> (u32, u32) {
        let w = crate::watercov::world(lon, lat);
        let n = f64::from(1u32 << z);
        ((w[0] * n) as u32, (w[1] * n) as u32)
    }

    #[test]
    fn the_fill_file_keeps_its_lakes_and_finds_them_by_tile() {
        let a = lake(&[sq(-72.3, 53.80, -72.29, 53.81)]);
        let b = lake(&[sq(-72.1, 53.90, -72.09, 53.905), sq(-72.08, 53.90, -72.07, 53.905)]);
        let (_d, f) = written(&[a.clone(), b.clone()]);
        assert_eq!(f.meta()["source"], "test");
        assert_eq!(f.lakes().len(), 2);
        assert_eq!(f.lake(0), a);
        assert_eq!(f.lake(1), b);
        assert!(f.lakes().iter().all(|r| r.id >> 62 == 1));
        let (x, y) = tile_of(-72.295, 53.805, 14);
        assert!(f.any_in(14, x, y));
        assert_ne!(f.tile_key(14, x, y), 0);
        assert_eq!(f.tile_key(14, x + 50, y), 0);
        // The same lakes, the same keys.
        let (_d2, g) = written(&[a, b]);
        assert_eq!(g.tile_key(14, x, y), f.tile_key(14, x, y));
        // (Mapped from the disk, as the server and a local step open it, and the same in memory.)
        let bytes: Box<dyn FillBytes> = Box::new(std::fs::read(_d.path().join("fill.sect")).unwrap());
        let m = Fill::from_source(bytes, "fill-test").unwrap();
        assert_eq!(m.tile_key(14, x, y), f.tile_key(14, x, y));
        assert_eq!(m.lake(1), f.lake(1));
    }

    #[test]
    fn the_water_layer_draws_a_filled_lake_whole_and_once() {
        use crate::water::Extra;
        // A lake across a z14 tile's edge: its share over the tiles it meets is its area.
        let l = lake(&[sq(-72.3, 53.80, -72.29, 53.81)]);
        let (_d, f) = written(std::slice::from_ref(&l));
        let b = l.bbox();
        let n = f64::from(1u32 << 14);
        let (x0, y0, x1, y1) = ((b[0] * n) as u32, (b[1] * n) as u32, (b[2] * n) as u32, (b[3] * n) as u32);
        assert!(x1 > x0 || y1 > y0, "the lake crosses a tile edge");
        let blocks = crate::water::Blocks::default();
        let mut area = 0.0f64;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let get = |tx: u32, ty: u32| -> Result<Vec<crate::water::Stored>> { Ok(f.z14(tx, ty).into_iter().collect()) };
                let c = crate::water::cover(14, x, y, &get, &blocks).unwrap();
                assert!(c.sea.iter().all(|&v| v == 0.0));
                let px = 1.0 / (n * crate::water::SIZE as f64);
                area += c.inland.iter().map(|&v| f64::from(v)).sum::<f64>() * px * px;
            }
        }
        assert!((area - l.area()).abs() / l.area() < 0.01, "{area} against {}", l.area());
        assert!(f.z14(x1 + 3, y1 + 3).is_none());
    }

    #[test]
    fn the_terrain_flattens_filled_lakes_as_lakes_and_keys_on_the_fill() {
        use crate::terrain_water::{Kind, Poly, WaterSource};
        struct None_;
        impl WaterSource for None_ {
            fn polys(&self, _: u8, _: u32, _: u32) -> Result<Vec<Poly>> {
                Ok(Vec::new())
            }
            fn pin(&self) -> String {
                "basemap".into()
            }
        }
        let l = lake(&[sq(-72.3, 53.80, -72.29, 53.81)]);
        let (_d, f) = written(&[l]);
        let id = f.lakes()[0].id;
        let (x, y) = tile_of(-72.295, 53.805, 12);
        let without = FilledWater { base: None_, fill: None };
        assert_eq!(without.pin(), "basemap");
        assert!(without.polys(12, x, y).unwrap().is_empty());
        let with = FilledWater { base: None_, fill: Some(f) };
        assert_eq!(with.pin(), "basemap + fill-test");
        let p = with.polys(12, x, y).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].kind, p[0].id), (Kind::Lake, id));
        let wt = crate::terrain_water::water_tile(&p);
        assert!(wt.lake.iter().any(|&v| v > 0));
    }

    #[test]
    fn intermittent_lakes_are_water_but_not_flattened() {
        use crate::terrain_water::{Poly, WaterSource};
        struct None_;
        impl WaterSource for None_ {
            fn polys(&self, _: u8, _: u32, _: u32) -> Result<Vec<Poly>> {
                Ok(Vec::new())
            }
            fn pin(&self) -> String {
                "basemap".into()
            }
        }
        let mut l = lake(&[sq(-72.3, 53.80, -72.29, 53.81)]);
        l.intermittent = true;
        let (_d, f) = written(std::slice::from_ref(&l));
        assert_eq!(f.lake(0), l);
        let (x, y) = tile_of(-72.295, 53.805, 14);
        assert!(<Fill as crate::water::Extra>::z14(&f, x, y).is_some());
        let (x, y) = tile_of(-72.295, 53.805, 12);
        assert!(FilledWater { base: None_, fill: Some(f) }.polys(12, x, y).unwrap().is_empty());
    }

    #[test]
    fn shapefiles_polygons_and_tables_are_read() {
        // One polygon record (a square and a hole) and one null record.
        let ring = |r: &[[f64; 2]]| r.to_vec();
        let outer = ring(&[[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        let hole = ring(&[[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8], [0.2, 0.2]]);
        let mut c = Vec::new();
        c.extend(5i32.to_le_bytes());
        c.extend([0u8; 32]);
        c.extend(2i32.to_le_bytes());
        c.extend(10i32.to_le_bytes());
        c.extend(0i32.to_le_bytes());
        c.extend(5i32.to_le_bytes());
        for p in outer.iter().chain(&hole) {
            c.extend(p[0].to_le_bytes());
            c.extend(p[1].to_le_bytes());
        }
        let mut b = vec![0u8; 100];
        b[0..4].copy_from_slice(&[0, 0, 0x27, 0x0a]);
        for (n, content) in [(1u32, c), (2u32, 0i32.to_le_bytes().to_vec())] {
            b.extend(n.to_be_bytes());
            b.extend(((content.len() / 2) as u32).to_be_bytes());
            b.extend(content);
        }
        let polys = shp::polygons_of(&b).unwrap();
        assert_eq!(polys, vec![vec![outer, hole], vec![]]);
        // A table: fields `definit_en` (C 10) and `code` (N 4), two records.
        let mut t = vec![0u8; 32];
        t[0] = 3;
        t[4..8].copy_from_slice(&2u32.to_le_bytes());
        let hlen = 32 + 2 * 32 + 1;
        t[8..10].copy_from_slice(&(hlen as u16).to_le_bytes());
        t[10..12].copy_from_slice(&15u16.to_le_bytes());
        for (name, kind, w) in [("definit_en", b'C', 10u8), ("code", b'N', 4u8)] {
            let mut f = [0u8; 32];
            f[..name.len()].copy_from_slice(name.as_bytes());
            f[11] = kind;
            f[16] = w;
            t.extend(f);
        }
        t.push(0x0d);
        t.extend(b" Lake         4");
        t.extend(b" Reservoir    5");
        let tab = shp::table_of(&t).unwrap();
        assert_eq!(tab.fields, vec!["definit_en", "code"]);
        assert_eq!(tab.col("CODE"), Some(1));
        assert_eq!(tab.rows, vec![vec!["Lake".to_string(), "4".to_string()], vec!["Reservoir".to_string(), "5".to_string()]]);
    }

    #[test]
    fn clipping_keeps_what_s_inside() {
        let r = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let c = clip(&r, [1.0, 1.0, 2.0, 3.0]);
        assert!((crate::watercov::area2(&c) / 2.0 - 2.0).abs() < 1e-12);
        assert!(clip(&r, [5.0, 5.0, 6.0, 6.0]).is_empty());
    }
}
