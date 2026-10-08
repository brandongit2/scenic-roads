//! Water flattened in the terrain (docs/plan.md §6, Terrain): the sea to 0 m and each lake or
//! reservoir to its shore's level, from OSM's water as the pass's basemap draws it (its `water`
//! layer: the sea from the pinned water polygons, `sources/basemap/water-polygons-split-3857.zip`;
//! lakes, ponds, reservoirs and docks from the pass's OSM, each with its OSM id). AWS fills water its
//! detailed source lacks from a coarse one whose cells mix the hills in (57 % of the sea off Yakutat
//! reads above 20 m), and above 60°N some lakes come from a sea-level source beside ellipsoidal land.
//!
//! - A terrain tile's water is the basemap's tile at the same zoom (z6–14: below z6 the basemap
//!   draws Natural Earth's; the terrain there is made from finer tiles), rasterized at 4 × 4 samples
//!   a pixel: each pixel's share of sea and of lake, and the lake that covers most of it. Rivers
//!   (class `river`: they slope), intermittent water, pools and water in tunnels are left as they are.
//! - The sea goes to 0 m. A lake goes to its level: the 10th percentile of the land pixels along its
//!   shore (those wholly dry, beside a pixel at least half that lake's). The shore's lowest pixels
//!   are where the water meets the land, the outlet's level; the very lowest are AWS's pits, voids
//!   and the next lake's water, so a low percentile, not the minimum: a tenth of the shore must be
//!   wrong to move it. Where the lake's own pixels are already flat (their spread within 1 m: a
//!   source that flattened it, as GLO-30 has) and no higher than that, they keep their level: a
//!   forest on the shore (GLO-30 is a surface model) would raise it otherwise.
//! - A pixel partly water is blended by its shares: `sea·0 + lake·level + (1 − sea − lake)·its own`.
//! - One level a lake, from all its shore in what's made together (a z6 tile's levels, then its z3
//!   pack's coarser ones, finest first: a lake keeps the level its finest tiles gave it), by its OSM
//!   id. A lake across two z6 tiles may take a level from each, a metre or two apart.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

/// Samples a side per pixel.
const SS: usize = 4;
const NONE: u16 = u16::MAX;
/// A pixel's samples (SS × SS): a share of 1.
const FULL: u8 = (SS * SS) as u8;
/// A lake's own pixels are flat when their spread (the middle half's) is within this (m).
const FLAT_M: f32 = 1.0;
/// The shore's percentile a lake's level is.
pub const SHORE_PERCENTILE: f64 = 0.10;

/// What's water in a terrain tile (256 × 256): each pixel's share of sea and of lake (its samples
/// of SS × SS), and the lake covering most of it (an index into `ids`, or NONE).
#[derive(Clone, Debug, Default)]
pub struct WaterTile {
    pub sea: Vec<u8>,
    pub lake: Vec<u8>,
    pub lake_of: Vec<u16>,
    /// The lakes' keys: their OSM ids, or (none) one of this tile's own (bit 63 set).
    pub ids: Vec<u64>,
}

impl WaterTile {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.sea.iter().all(|&s| s == 0)
    }
}

/// A polygon's kind as the basemap's water layer has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sea,
    Lake,
}

/// One water polygon: its kind, its key (lakes) and its rings in tile pixels (0–256, y down).
pub struct Poly {
    pub kind: Kind,
    pub id: u64,
    pub rings: Vec<Vec<[f64; 2]>>,
}

/// Where a tile's water comes from.
pub trait WaterSource: Sync {
    fn polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>>;
    /// What pins it (the terrain's key): the basemap's content name.
    fn pin(&self) -> String;
}

/// The pass's basemap (Planetiler's OpenMapTiles PMTiles): its `water` layer.
pub struct BasemapWater {
    pm: store::pmtiles::PmTiles,
    name: String,
}

impl BasemapWater {
    /// The basemap at `path`, pinned by `name` (its content name in the manifest).
    pub fn open(path: &std::path::Path, name: &str) -> Result<BasemapWater> {
        let f = store::range::PlainFile::open(path)?;
        Ok(BasemapWater { pm: store::pmtiles::PmTiles::with_leaf_cache(Box::new(f), 4096)?, name: name.to_string() })
    }
}

impl WaterSource for BasemapWater {
    fn polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
        if !(6..=14).contains(&z) {
            return Ok(Vec::new());
        }
        let Some(raw) = self.pm.get(z, x, y)? else { return Ok(Vec::new()) };
        polys_of_mvt(&names::mvt::gunzip_if_gzip(&raw)?, z, x, y)
    }
    fn pin(&self) -> String {
        self.name.clone()
    }
}

/// The water polygons of an MVT tile's `water` layer, in terrain pixels.
pub fn polys_of_mvt(mvt: &[u8], z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
    let tile = names::mvt::Tile::decode(mvt)?;
    let Some(l) = tile.layers.iter().find(|l| l.name == "water") else { return Ok(Vec::new()) };
    let s = 256.0 / f64::from(l.extent);
    let mut out = Vec::new();
    for (k, f) in l.features.iter().enumerate() {
        if f.geom_type != Some(3) {
            continue;
        }
        let prop = |name: &str| f.tags.chunks_exact(2).find(|kv| l.keys.get(kv[0] as usize).is_some_and(|k| k == name)).and_then(|kv| l.values.get(kv[1] as usize));
        let class = prop("class").and_then(|v| v.as_str()).unwrap_or("");
        let kind = match class {
            "ocean" => Kind::Sea,
            "lake" | "pond" | "dock" => Kind::Lake,
            _ => continue,
        };
        let num = |v: Option<&names::mvt::Value>| -> Option<i64> {
            match v? {
                names::mvt::Value::Int(i) | names::mvt::Value::Sint(i) => Some(*i),
                names::mvt::Value::Uint(u) => Some(*u as i64),
                names::mvt::Value::Double(d) => Some(*d as i64),
                names::mvt::Value::Float(d) => Some(*d as i64),
                _ => None,
            }
        };
        if num(prop("intermittent")) == Some(1) || prop("brunnel").and_then(|v| v.as_str()) == Some("tunnel") {
            continue;
        }
        let id = match num(prop("id")) {
            Some(i) if i > 0 => i as u64,
            // (No id: a key of this tile's own, never shared.)
            _ => (1u64 << 63) | (u64::from(z) << 56) ^ (u64::from(x) << 30) ^ (u64::from(y) << 4) ^ k as u64,
        };
        let rings = decode_rings(&f.geometry).into_iter().map(|r| r.into_iter().map(|(px, py)| [px * s, py * s]).collect()).collect();
        out.push(Poly { kind, id, rings });
        let _ = (z, x, y);
    }
    Ok(out)
}

/// An MVT geometry's rings (tile units).
fn decode_rings(g: &[u32]) -> Vec<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let (mut x, mut y, mut i) = (0i64, 0i64, 0usize);
    let zz = |v: u32| (v >> 1) as i64 ^ -((v & 1) as i64);
    while i < g.len() {
        let (cmd, n) = (g[i] & 7, g[i] >> 3);
        i += 1;
        match cmd {
            1 | 2 => {
                for _ in 0..n {
                    if i + 1 >= g.len() {
                        break;
                    }
                    x += zz(g[i]);
                    y += zz(g[i + 1]);
                    i += 2;
                    if cmd == 1 && !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push((x as f64, y as f64));
                }
            }
            7 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => break,
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The share of each pixel a polygon covers (even–odd over its rings), at SS × SS samples a pixel:
/// the pixels it meets and their counts (1 to SS²).
fn raster(rings: &[Vec<[f64; 2]>]) -> Vec<(u32, u8)> {
    let n = 256 * SS;
    let sc = SS as f64;
    let mut rows: Vec<Vec<f32>> = vec![Vec::new(); n];
    let (mut ylo, mut yhi) = (n, 0usize);
    for r in rings {
        if r.len() < 3 {
            continue;
        }
        for i in 0..r.len() {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            let (ay, by) = (a[1] * sc, b[1] * sc);
            if ay == by {
                continue;
            }
            let (y0, y1) = (ay.min(by), ay.max(by));
            // Sample rows whose centre (k + 0.5) lies in [y0, y1).
            let k0 = ((y0 - 0.5).ceil().max(0.0)) as usize;
            let k1 = ((y1 - 0.5).ceil().min(n as f64).max(0.0)) as usize;
            for k in k0..k1 {
                let yc = k as f64 + 0.5;
                let t = (yc - ay) / (by - ay);
                rows[k].push(((a[0] + t * (b[0] - a[0])) * sc) as f32);
                ylo = ylo.min(k);
                yhi = yhi.max(k + 1);
            }
        }
    }
    let mut counts: HashMap<u32, u8> = HashMap::new();
    let mut acc = vec![0u8; 256];
    for k in ylo..yhi.max(ylo) {
        let xs = &mut rows[k];
        if xs.is_empty() {
            continue;
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            // Sample columns whose centre (c + 0.5) lies in [x0, x1).
            let c0 = ((pair[0] as f64 - 0.5).ceil().max(0.0)) as usize;
            let c1 = ((pair[1] as f64 - 0.5).ceil().min(n as f64).max(0.0)) as usize;
            for c in c0..c1 {
                acc[c / SS] += 1;
            }
        }
        if k % SS == SS - 1 || k + 1 == yhi {
            let py = (k / SS) as u32;
            for (px, a) in acc.iter_mut().enumerate() {
                if *a > 0 {
                    *counts.entry(py * 256 + px as u32).or_default() += *a;
                    *a = 0;
                }
            }
        }
    }
    let mut v: Vec<(u32, u8)> = counts.into_iter().collect();
    v.sort_unstable();
    v
}

/// A tile's water from its polygons.
pub fn water_tile(polys: &[Poly]) -> WaterTile {
    let mut wt = WaterTile { sea: vec![0; 65536], lake: vec![0; 65536], lake_of: vec![NONE; 65536], ids: Vec::new() };
    let mut best = vec![0u8; 65536];
    let mut index: HashMap<u64, u16> = HashMap::new();
    for p in polys {
        let cells = raster(&p.rings);
        if cells.is_empty() {
            continue;
        }
        match p.kind {
            Kind::Sea => {
                for (i, c) in cells {
                    let i = i as usize;
                    wt.sea[i] = (wt.sea[i] + c).min(FULL);
                }
            }
            Kind::Lake => {
                if !index.contains_key(&p.id) && wt.ids.len() >= NONE as usize {
                    continue;
                }
                let k = *index.entry(p.id).or_insert_with(|| {
                    wt.ids.push(p.id);
                    (wt.ids.len() - 1) as u16
                });
                for (i, c) in cells {
                    let i = i as usize;
                    wt.lake[i] = (wt.lake[i] + c).min(FULL);
                    if c > best[i] {
                        best[i] = c;
                        wt.lake_of[i] = k;
                    }
                }
            }
        }
    }
    // (Sea and lake together cover a pixel at most once.)
    for i in 0..65536 {
        if wt.sea[i] + wt.lake[i] > FULL {
            wt.lake[i] = FULL - wt.sea[i];
        }
    }
    wt
}

/// What a tile says of each lake's level: its shore's land pixels, and its own pixels' spread.
#[derive(Clone, Debug, Default)]
pub struct LakeSamples {
    pub shore: Vec<f32>,
    /// Per tile: (its pixels at least half this lake's, their median, their middle half's spread).
    pub inside: Vec<(u32, f32, f32)>,
}

/// The samples of each lake in tile `e` (before flattening).
pub fn samples(e: &[f32], wt: &WaterTile) -> HashMap<u64, LakeSamples> {
    let mut out: HashMap<u64, LakeSamples> = HashMap::new();
    if wt.ids.is_empty() {
        return out;
    }
    let mut inside: Vec<Vec<f32>> = vec![Vec::new(); wt.ids.len()];
    for p in 0..65536usize {
        let k = wt.lake_of[p];
        if k != NONE && wt.lake[p] * 2 >= FULL && e[p].is_finite() {
            inside[k as usize].push(e[p]);
        }
        if wt.sea[p] + wt.lake[p] > 0 || !e[p].is_finite() {
            continue;
        }
        let (x, y) = ((p % 256) as i32, (p / 256) as i32);
        let mut seen = [NONE; 8];
        let mut ns = 0;
        for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (a, b) = (x + dx, y + dy);
            if a < 0 || b < 0 || a > 255 || b > 255 {
                continue;
            }
            let q = (b * 256 + a) as usize;
            let k = wt.lake_of[q];
            if k != NONE && wt.lake[q] * 2 >= FULL && !seen[..ns].contains(&k) {
                seen[ns] = k;
                ns += 1;
                out.entry(wt.ids[k as usize]).or_default().shore.push(e[p]);
            }
        }
    }
    for (k, mut v) in inside.into_iter().enumerate() {
        if v.is_empty() {
            continue;
        }
        v.sort_by(f32::total_cmp);
        let (med, iqr) = (v[v.len() / 2], v[v.len() * 3 / 4] - v[v.len() / 4]);
        out.entry(wt.ids[k]).or_default().inside.push((v.len() as u32, med, iqr));
    }
    out
}

/// Adds a tile's samples to those gathered.
pub fn gather(all: &mut HashMap<u64, LakeSamples>, mut one: HashMap<u64, LakeSamples>) {
    for (id, s) in one.drain() {
        let e = all.entry(id).or_default();
        e.shore.extend(s.shore);
        e.inside.extend(s.inside);
    }
}

/// A lake's level from its samples (None: no shore and no flat inside).
pub fn level(s: &LakeSamples) -> Option<f32> {
    let mut shore = s.shore.clone();
    shore.sort_by(f32::total_cmp);
    let low = (!shore.is_empty()).then(|| shore[((shore.len() - 1) as f64 * SHORE_PERCENTILE).round() as usize]);
    // Its own pixels, flat (each tile's within FLAT_M, and the tiles' medians too): their median,
    // weighted by pixels.
    let mut ins = s.inside.clone();
    ins.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let flat = !ins.is_empty() && ins.iter().all(|t| t.2 <= FLAT_M) && ins.last().unwrap().1 - ins[0].1 <= FLAT_M;
    let own = flat.then(|| {
        let total: u64 = ins.iter().map(|t| t.0 as u64).sum();
        let mut acc = 0u64;
        ins.iter().find(|t| {
            acc += t.0 as u64;
            acc * 2 >= total
        }).map(|t| t.1).unwrap()
    });
    match (low, own) {
        (Some(l), Some(o)) if o <= l + FLAT_M => Some(o),
        (Some(l), _) => Some(l),
        (None, o) => o,
    }
}

/// Levels for the lakes gathered that `known` hasn't, added to it.
pub fn add_levels(known: &mut HashMap<u64, f32>, all: &HashMap<u64, LakeSamples>) {
    let mut ids: Vec<&u64> = all.keys().collect();
    ids.sort_unstable();
    for id in ids {
        if !known.contains_key(id) {
            if let Some(l) = level(&all[id]) {
                known.insert(*id, l);
            }
        }
    }
}

/// Flattens tile `e`'s water: the sea to 0, each lake to its level (`levels`; a lake without one
/// keeps its pixels), a pixel partly water blended by its shares. Returns the pixels changed.
pub fn flatten(e: &mut [f32], wt: &WaterTile, levels: &HashMap<u64, f32>) -> usize {
    let mut n = 0;
    for p in 0..e.len() {
        if wt.sea[p] == 0 && wt.lake[p] == 0 {
            continue;
        }
        let (s, mut l) = (wt.sea[p] as f32 / FULL as f32, wt.lake[p] as f32 / FULL as f32);
        let lv = match wt.lake_of[p] {
            NONE => None,
            k => levels.get(&wt.ids[k as usize]).copied(),
        };
        if lv.is_none() {
            l = 0.0;
        }
        let v = l * lv.unwrap_or(0.0) + (1.0 - s - l) * e[p];
        if v != e[p] {
            e[p] = v;
            n += 1;
        }
    }
    n
}

/// A tile's water, if its source has some there (an empty tile: None).
pub fn tile_water(src: &dyn WaterSource, z: u8, x: u32, y: u32) -> Result<Option<Arc<WaterTile>>> {
    let polys = src.polys(z, x, y)?;
    if polys.is_empty() {
        return Ok(None);
    }
    let wt = water_tile(&polys);
    Ok((!wt.is_empty()).then(|| Arc::new(wt)))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn square(kind: Kind, id: u64, x0: f64, y0: f64, x1: f64, y1: f64) -> Poly {
        Poly { kind, id, rings: vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]] }
    }

    /// Polygons given whole, whatever the tile.
    pub struct Fixed(pub fn(u8, u32, u32) -> Vec<Poly>);
    impl WaterSource for Fixed {
        fn polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
            Ok((self.0)(z, x, y))
        }
        fn pin(&self) -> String {
            "test".into()
        }
    }

    #[test]
    fn shares_of_pixels() {
        // A square from (10.5, 20) to (30.25, 40.75): whole pixels inside, a half and a quarter at
        // its edges; a hole in it (even–odd) left dry.
        let mut p = square(Kind::Sea, 0, 10.5, 20.0, 30.25, 40.75);
        p.rings.push(vec![[15.0, 25.0], [15.0, 27.0], [17.0, 27.0], [17.0, 25.0]]);
        let wt = water_tile(&[p]);
        let at = |x: usize, y: usize| wt.sea[y * 256 + x] as f32 / 16.0;
        assert_eq!(at(12, 30), 1.0);
        assert_eq!(at(10, 30), 0.5);
        assert_eq!(at(30, 30), 0.25);
        assert_eq!(at(30, 40), 0.25 * 0.75);
        assert_eq!(at(9, 30), 0.0);
        assert_eq!(at(16, 26), 0.0, "the hole");
        assert_eq!(at(31, 30), 0.0);
        let total: f32 = wt.sea.iter().map(|&v| v as f32 / 16.0).sum();
        let want = (30.25 - 10.5) * (40.75 - 20.0) - 4.0;
        assert!((total as f64 - want).abs() < 1e-3, "{total} vs {want}");
    }

    #[test]
    fn the_sea_goes_to_zero_and_a_lake_to_its_shores_tenth_percentile() {
        // Land rising east at 1 m a pixel from 100 m; a raised lake (a coarse source's 160 m) from
        // x 40 to 80; the sea west of x 10.
        let polys = vec![square(Kind::Sea, 0, 0.0, 0.0, 10.0, 256.0), square(Kind::Lake, 7, 40.0, 100.0, 80.0, 140.0)];
        let wt = water_tile(&polys);
        let mut e: Vec<f32> = (0..65536).map(|p| 100.0 + (p % 256) as f32).collect();
        for y in 100..140 {
            for x in 40..80 {
                e[y * 256 + x] = 160.0 + ((x * 7 + y * 3) % 11) as f32;
            }
        }
        let s = samples(&e, &wt);
        let mut levels = HashMap::new();
        add_levels(&mut levels, &s);
        // Its shore: x 39 (139 m) to x 80 (180 m), rows 99 and 140 between; the 10th percentile.
        let mut shore: Vec<f32> = Vec::new();
        for y in 99..=140 {
            for x in 39..=80 {
                if x == 39 || x == 80 || y == 99 || y == 140 {
                    shore.push(100.0 + x as f32);
                }
            }
        }
        shore.sort_by(f32::total_cmp);
        let want = shore[((shore.len() - 1) as f64 * 0.1).round() as usize];
        assert_eq!(levels[&7], want);
        flatten(&mut e, &wt, &levels);
        assert!((0..256).all(|y| (0..10).all(|x| e[y * 256 + x] == 0.0)));
        assert!((100..140).all(|y| (40..80).all(|x| e[y * 256 + x] == want)));
        assert_eq!(e[50 * 256 + 50], 150.0, "land unchanged");
    }

    #[test]
    fn a_flat_lake_keeps_its_level_under_a_forested_shore_and_partial_pixels_blend() {
        // A lake flattened by its source at 200 m, its shore 6 m higher (a forest): kept at 200.
        let polys = vec![square(Kind::Lake, 9, 50.0, 50.0, 70.5, 70.0)];
        let wt = water_tile(&polys);
        let mut e = vec![206f32; 65536];
        for y in 50..70 {
            for x in 50..71 {
                e[y * 256 + x] = 200.0;
            }
        }
        let mut levels = HashMap::new();
        add_levels(&mut levels, &samples(&e, &wt));
        assert_eq!(levels[&9], 200.0);
        // A raised flat lake (Kivalliq's: 47 m over its shore) goes to the shore's level instead.
        let mut r = vec![100f32; 65536];
        for y in 50..70 {
            for x in 50..71 {
                r[y * 256 + x] = 147.0;
            }
        }
        let mut lv = HashMap::new();
        add_levels(&mut lv, &samples(&r, &wt));
        assert_eq!(lv[&9], 100.0);
        flatten(&mut r, &wt, &lv);
        assert_eq!(r[60 * 256 + 60], 100.0);
        // The pixel half lake (x 70): half the level, half its own (147 m).
        assert_eq!(r[60 * 256 + 70], 0.5 * 100.0 + 0.5 * 147.0);
    }

    #[test]
    fn one_level_for_a_lake_across_tiles() {
        // The same lake (id 3) in two tiles, its shore lower in one: one level from both.
        let wt = water_tile(&[square(Kind::Lake, 3, 100.0, 100.0, 200.0, 200.0)]);
        let a = vec![10f32; 65536];
        let b = vec![30f32; 65536];
        let mut all = HashMap::new();
        gather(&mut all, samples(&a, &wt));
        gather(&mut all, samples(&b, &wt));
        let mut lv = HashMap::new();
        add_levels(&mut lv, &all);
        assert_eq!(lv[&3], 10.0);
        // A finer zoom's level holds: add_levels keeps what's known.
        let mut known = HashMap::from([(3u64, 12.0f32)]);
        add_levels(&mut known, &all);
        assert_eq!(known[&3], 12.0);
    }

    #[test]
    fn mvt_classes_read() {
        use names::mvt::{Feature, Layer, Tile, Value};
        // A square in tile units (extent 4096): ocean, lake (id 41279034), river (left), an
        // intermittent lake (left).
        let sq = |x0: u32, y0: u32, d: u32| -> Vec<u32> {
            let zz = |v: i32| ((v << 1) ^ (v >> 31)) as u32;
            vec![9, zz(x0 as i32), zz(y0 as i32), 26, zz(d as i32), 0, 0, zz(d as i32), zz(-(d as i32)), 0, 15]
        };
        let f = |tags: Vec<u32>, g: Vec<u32>| Feature { id: None, tags, geom_type: Some(3), geometry: g, ..Default::default() };
        let l = Layer {
            name: "water".into(),
            version: 2,
            extent: 4096,
            keys: vec!["class".into(), "id".into(), "intermittent".into()],
            values: vec![Value::String("ocean".into()), Value::String("lake".into()), Value::Uint(41279034), Value::Int(0), Value::String("river".into()), Value::Int(1)],
            features: vec![f(vec![0, 0], sq(0, 0, 160)), f(vec![0, 1, 1, 2, 2, 3], sq(1600, 1600, 160)), f(vec![0, 4], sq(3200, 0, 160)), f(vec![0, 1, 2, 5], sq(0, 3200, 160))],
            unknown: vec![],
        };
        let t = Tile { layers: vec![l], unknown: vec![] };
        let polys = polys_of_mvt(&t.encode(), 12, 1, 1).unwrap();
        assert_eq!(polys.len(), 2);
        assert_eq!((polys[0].kind, polys[1].kind, polys[1].id), (Kind::Sea, Kind::Lake, 41279034));
        let wt = water_tile(&polys);
        assert_eq!(wt.sea[5 * 256 + 5], 16);
        assert_eq!(wt.lake[105 * 256 + 105], 16);
    }
}
