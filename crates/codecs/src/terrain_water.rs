//! The water in the terrain (docs/steps.md §2, Terrain), from OSM's water as the pass's basemap draws
//! it (its `water` layer: the sea from the pinned water polygons,
//! `sources/basemap/water-polygons-split-3857.zip`; lakes, ponds, reservoirs and docks from the
//! pass's OSM, each with its OSM id). AWS fills water its detailed source lacks from a coarse one
//! whose cells mix the hills in (57 % of the sea off Yakutat reads above 20 m).
//!
//! - A terrain tile's water is the basemap's tile at the same zoom (z6–14: below z6 the basemap
//!   draws Natural Earth's; the terrain there is made from finer tiles), rasterized at 4 × 4 samples
//!   a pixel: each pixel's share of sea and of lake, and the lake that covers most of it (the
//!   depth's: pipeline::depth, which reads the rivers too: `WaterSource::depth_polys`,
//!   `Kind::River`, `WaterTile::river`). Intermittent water, pools and water in tunnels are left out.
//! - The sea goes to 0 m, a pixel partly sea blended by its share (`sea·0 + (1 − sea)·its own`).
//!   Lakes and rivers keep the source's heights, its artefacts too (the owner's, 5 Nov: the
//!   terrain follows its source over inland water).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// Samples a side per pixel.
const SS: usize = 4;
const NONE: u16 = u16::MAX;
/// A pixel's samples (SS × SS): a share of 1.
pub const FULL: u8 = (SS * SS) as u8;

/// What's water in a terrain tile (256 × 256): each pixel's share of sea and of lake (its samples
/// of SS × SS), and the lake covering most of it (an index into `ids`, or NONE); the depth's tiles
/// also each pixel's share of river (`WaterSource::depth_polys`; empty: none).
#[derive(Clone, Debug, Default)]
pub struct WaterTile {
    pub sea: Vec<u8>,
    pub lake: Vec<u8>,
    /// The share of river (class `river`), the sea and lakes not counted in it; empty when none.
    pub river: Vec<u8>,
    pub lake_of: Vec<u16>,
    /// The lakes' keys: their OSM ids, or (none) one of this tile's own (bit 63 set).
    pub ids: Vec<u64>,
}

impl WaterTile {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.sea.iter().all(|&s| s == 0) && self.river.iter().all(|&s| s == 0)
    }

    /// Pixel `i`'s share of river (0 when the tile has none).
    pub fn river_at(&self, i: usize) -> u8 {
        self.river.get(i).copied().unwrap_or(0)
    }
}

/// A polygon's kind as the basemap's water layer has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sea,
    Lake,
    /// Class `river`: the depth's alone (`WaterSource::depth_polys`); never `polys`.
    River,
}

/// One water polygon: its kind, its key (lakes) and its rings in tile pixels (0–256, y down).
#[derive(Clone)]
pub struct Poly {
    pub kind: Kind,
    pub id: u64,
    pub rings: Vec<Vec<[f64; 2]>>,
}

/// Where a tile's water comes from.
pub trait WaterSource: Sync {
    fn polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>>;
    /// The depth's water: `polys` and the rivers (`Kind::River`), in the tile's order. A source
    /// without rivers: `polys`.
    fn depth_polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
        self.polys(z, x, y)
    }
    /// `depth_polys` of each of `tiles`, in their order: a source that can read them together
    /// does (the basemap: its tiles in runs, `store::pmtiles::PmTiles::get_many`); the same
    /// polygons as one by one.
    fn depth_polys_many(&self, tiles: &[(u8, u32, u32)]) -> Result<Vec<Vec<Poly>>> {
        tiles.iter().map(|&(z, x, y)| self.depth_polys(z, x, y)).collect()
    }
    /// What pins it (the terrain's key): the basemap's content name.
    fn pin(&self) -> String;
}

/// How many of the basemap's runs of tiles `depth_polys_many` reads at once.
pub const WATER_READS: usize = 8;

/// The pass's basemap (Planetiler's OpenMapTiles PMTiles): its `water` layer.
pub struct BasemapWater {
    pm: store::pmtiles::PmTiles,
    name: String,
}

impl BasemapWater {
    /// The basemap at `path`, pinned by `name` (its content name in the manifest).
    pub fn open<'a>(path: impl Into<store::nasfs::Loc<'a>>, name: &str) -> Result<BasemapWater> {
        let f = path.into().open_plain()?;
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
    fn depth_polys(&self, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
        if !(6..=14).contains(&z) {
            return Ok(Vec::new());
        }
        let Some(raw) = self.pm.get(z, x, y)? else { return Ok(Vec::new()) };
        polys_of_mvt_with(&names::mvt::gunzip_if_gzip(&raw)?, z, x, y, true)
    }
    /// The tiles read in runs (`PmTiles::get_many`), then each decoded as `depth_polys` does, on
    /// the job's threads.
    fn depth_polys_many(&self, tiles: &[(u8, u32, u32)]) -> Result<Vec<Vec<Poly>>> {
        use rayon::prelude::*;
        let asked: Vec<(u8, u32, u32)> = tiles.iter().copied().filter(|t| (6..=14).contains(&t.0)).collect();
        let raws = self.pm.get_many(&asked, WATER_READS)?;
        let decoded: Vec<Result<Vec<Poly>>> = asked
            .par_iter()
            .zip(raws.into_par_iter())
            .map(|(&(z, x, y), raw)| match raw {
                Some(raw) => polys_of_mvt_with(&names::mvt::gunzip_if_gzip(&raw)?, z, x, y, true),
                None => Ok(Vec::new()),
            })
            .collect();
        let mut by = decoded.into_iter();
        tiles.iter().map(|t| if (6..=14).contains(&t.0) { by.next().unwrap() } else { Ok(Vec::new()) }).collect()
    }
    fn pin(&self) -> String {
        self.name.clone()
    }
}

/// The water polygons of an MVT tile's `water` layer, in terrain pixels (the terrain's: no rivers).
pub fn polys_of_mvt(mvt: &[u8], z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
    polys_of_mvt_with(mvt, z, x, y, false)
}

/// The water polygons of an MVT tile's `water` layer, in terrain pixels, its rivers too when
/// `rivers` (the depth's: a river's key is its tile's own, as a lake's without an id, never read).
pub fn polys_of_mvt_with(mvt: &[u8], z: u8, x: u32, y: u32, rivers: bool) -> Result<Vec<Poly>> {
    let tile = names::mvt::Tile::decode(mvt)?;
    let Some(l) = tile.layers.iter().find(|l| l.name == "water") else { return Ok(Vec::new()) };
    let s = 256.0 / f64::from(l.extent);
    let mut out = Vec::new();
    for (k, f) in l.features.iter().enumerate() {
        if f.geom_type != Some(3) {
            continue;
        }
        let prop = |name: &str| {
            f.tags
                .chunks_exact(2)
                .find(|kv| l.keys.get(kv[0] as usize).is_some_and(|k| k == name))
                .and_then(|kv| l.values.get(kv[1] as usize))
        };
        let class = prop("class").and_then(|v| v.as_str()).unwrap_or("");
        let kind = match class {
            "ocean" => Kind::Sea,
            "lake" | "pond" | "dock" => Kind::Lake,
            "river" if rivers => Kind::River,
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
            // (No id: a key of this tile's own.)
            _ => own_key(z, x, y, k)
                .with_context(|| format!("water feature {k} of {z}/{x}/{y}: past own_key's range"))?,
        };
        let rings = decode_rings(&f.geometry)
            .into_iter()
            .map(|r| r.into_iter().map(|(px, py)| [px * s, py * s]).collect())
            .collect();
        out.push(Poly { kind, id, rings });
    }
    Ok(out)
}

/// The key of a lake without an OSM id, feature `k` of tile z/x/y (z at most 14): its tile's own,
/// never another's. Features 0-15 keep the key they always had (`1<<63 | z<<56 | x<<30 | y<<4 | k`,
/// its fields apart while x, y < 2^26, so terrain made before stays as it was); from feature 16 on,
/// whose old key met the next rows' (task #464), bit 62 (never set by those: z < 64) marks a
/// layout of its own, `z<<56 | x<<42 | y<<28 | k-16`. None past z14 or 2^28 + 16 features.
pub fn own_key(z: u8, x: u32, y: u32, k: usize) -> Option<u64> {
    if z > 14 || x >> z != 0 || y >> z != 0 {
        return None;
    }
    let (z, x, y) = (u64::from(z), u64::from(x), u64::from(y));
    if k < 16 {
        return Some(OWN | z << 56 | x << 30 | y << 4 | k as u64);
    }
    let k = (k - 16) as u64;
    (k < 1 << 28).then_some(OWN | 1 << 62 | z << 56 | x << 42 | y << 28 | k)
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
    let mut wt = WaterTile {
        sea: vec![0; 65536],
        lake: vec![0; 65536],
        river: Vec::new(),
        lake_of: vec![NONE; 65536],
        ids: Vec::new(),
    };
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
            Kind::River => {
                if wt.river.is_empty() {
                    wt.river = vec![0; 65536];
                }
                for (i, c) in cells {
                    let i = i as usize;
                    wt.river[i] = (wt.river[i] + c).min(FULL);
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
    // (And a river only what they leave.)
    if !wt.river.is_empty() {
        for i in 0..65536 {
            wt.river[i] = wt.river[i].min(FULL - wt.sea[i] - wt.lake[i]);
        }
    }
    wt
}

/// Flattens tile `e`'s water: the sea to 0, a pixel partly sea blended by its share; lakes and
/// rivers keep their pixels. Returns the pixels changed.
pub fn flatten(e: &mut [f32], wt: &WaterTile) -> usize {
    let mut n = 0;
    for p in 0..e.len() {
        if wt.sea[p] == 0 {
            continue;
        }
        let v = (1.0 - wt.sea[p] as f32 / FULL as f32) * e[p];
        if v != e[p] {
            e[p] = v;
            n += 1;
        }
    }
    n
}

/// Bit 63 of a lake's key: one of its tile's own (no OSM id: `polys_of_mvt`).
const OWN: u64 = 1 << 63;

/// Whether lake key `id` is one of its tile's own (`own_key`), never another tile's.
pub fn is_own(id: u64) -> bool {
    id & OWN != 0
}

/// What a from ending so says: its target reads the depth's water (`WaterSource::depth_polys`,
/// the rivers too), so its digest is of that; any other is the terrain's (`polys`).
pub const RIVERS_FROM: &str = " +rivers";

/// A tile's water as the terrain reads it (`WaterSource::polys`), as a digest: its polygons in
/// order (their kind, their key, their rings' points), what `water_tile` makes of them; 0 for none.
/// A lake's key of its tile's own counts as such, not by its value (its feature's place in the
/// tile, which other water moves): it's never shared, so its value changes no pixel.
pub fn polys_digest(polys: &[Poly]) -> u64 {
    if polys.is_empty() {
        return 0;
    }
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    for p in polys {
        h.update(&[match p.kind {
            Kind::Sea => 1u8,
            Kind::Lake => 2,
            Kind::River => 3,
        }]);
        h.update(&if p.kind != Kind::Sea && p.id & OWN != 0 { OWN } else { p.id }.to_le_bytes());
        h.update(&(p.rings.len() as u64).to_le_bytes());
        for r in &p.rings {
            h.update(&(r.len() as u64).to_le_bytes());
            for pt in r {
                h.update(&pt[0].to_bits().to_le_bytes());
                h.update(&pt[1].to_bits().to_le_bytes());
            }
        }
    }
    h.digest() | 1
}

/// The digest of the water `src` has in the tiles `tiles` (each read in parallel as the terrain
/// reads it, `polys`, or with `rivers` as the depth does, `depth_polys`): over each tile with
/// water, its zoom, column, row and `polys_digest`; "-" when none has any. What keys a terrain or
/// depth target on the water it reads (agent::build).
pub fn tiles_digest(src: &dyn WaterSource, tiles: &[(u8, u32, u32)], rivers: bool) -> Result<String> {
    use rayon::prelude::*;
    let got: Vec<Result<u64>> = tiles
        .par_iter()
        .map(|&(z, x, y)| if rivers { src.depth_polys(z, x, y) } else { src.polys(z, x, y) }.map(|p| polys_digest(&p)))
        .collect();
    let mut lines = String::new();
    for (&(z, x, y), d) in tiles.iter().zip(got) {
        let d = d?;
        if d != 0 {
            lines.push_str(&format!("{z}/{x}/{y} {d:016x}\n"));
        }
    }
    Ok(if lines.is_empty() { "-".into() } else { store::naming::hash16(lines.as_bytes()) })
}

/// The water digests of a water source (`WaterSource::pin`: the basemap's content name) in the build
/// manifest: `work/water-idx/<hash16 of the pin>` (`WaterIdx`), one a source, kept when the source
/// is replaced.
pub fn idx_logical(pin: &str) -> String {
    format!("{IDX_PREFIX}{}", store::naming::hash16(pin.as_bytes()))
}

pub const IDX_PREFIX: &str = "work/water-idx/";

/// The water digests' format.
pub const IDX_FMT: u64 = 1;

/// A water source's digests (`tiles_digest`) of what each terrain target reads of it: JSON,
/// `{"fmt": 1, "step": "terrain-water", "pin": <its pin>, "digests": {<target>: [<from>, <digest>]}}`,
/// a target a z6 tile ("6/x/y": its z9–12 tiles near the coverage, `terrain_pack::piece_levels`; from:
/// the coverage's fingerprint that decides them) or an assembly ("3/x/y": its z6–8 tiles; from
/// "z6-8"). Made by the `terrain-water` job (`make_idx`), which keeps those made before whose from
/// is the coverage's still.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WaterIdx {
    pub fmt: u64,
    pub step: String,
    pub pin: String,
    pub digests: std::collections::BTreeMap<String, (String, String)>,
}

impl WaterIdx {
    /// Target `t`'s digest, if made from `from`.
    pub fn get(&self, t: &str, from: &str) -> Option<&str> {
        self.digests.get(t).filter(|(f, _)| f == from).map(|(_, d)| d.as_str())
    }

    pub fn from_bytes(b: &[u8]) -> Result<WaterIdx> {
        let w: WaterIdx = serde_json::from_slice(b)?;
        anyhow::ensure!(w.fmt == IDX_FMT && w.step == "terrain-water", "not terrain water digests of format {IDX_FMT}");
        Ok(w)
    }
}

/// An assembly's water reads' from (`WaterIdx`).
pub const AREA_FROM: &str = "z6-8";

/// The water tiles z3 tile `q`'s assembly reads: z6–8, the whole of it (none coarser: `polys`).
pub fn z3_tiles(q: (u32, u32)) -> Vec<(u8, u32, u32)> {
    (6..=8u8)
        .flat_map(|z| {
            let s = 1u32 << (z - 3);
            (q.0 * s..(q.0 + 1) * s).flat_map(move |x| (q.1 * s..(q.1 + 1) * s).map(move |y| (z, x, y)))
        })
        .collect()
}

/// A terrain target's water reads: the target, what decides them (`WaterIdx`'s from), its tiles.
pub type Reads = (String, String, Vec<(u8, u32, u32)>);

/// The digests `want` asks for (target, from, the tiles it reads), those `prior` (the same source's,
/// made before) has with the same from kept, the others worked out from `src`; `progress` told each
/// target done.
pub fn make_idx(
    src: &dyn WaterSource,
    prior: Option<&WaterIdx>,
    want: &[Reads],
    progress: &dyn Fn(u64, u64),
) -> Result<WaterIdx> {
    let mut out = WaterIdx { fmt: IDX_FMT, step: "terrain-water".into(), pin: src.pin(), digests: Default::default() };
    if let Some(p) = prior.filter(|p| p.pin == out.pin) {
        out.digests = p.digests.clone();
    }
    let n = want.len() as u64;
    for (k, (t, from, tiles)) in want.iter().enumerate() {
        if out.get(t, from).is_none() {
            let d = tiles_digest(src, tiles, from.ends_with(RIVERS_FROM))?;
            out.digests.insert(t.clone(), (from.clone(), d));
        }
        progress(k as u64 + 1, n);
    }
    Ok(out)
}

/// A tile's water, if its source has some there (an empty tile: None): its sea and lakes
/// (`polys`).
pub fn tile_water(src: &dyn WaterSource, z: u8, x: u32, y: u32) -> Result<Option<Arc<WaterTile>>> {
    let polys = src.polys(z, x, y)?;
    if polys.is_empty() {
        return Ok(None);
    }
    let wt = water_tile(&polys);
    Ok((!wt.is_empty()).then(|| Arc::new(wt)))
}

#[cfg(any(test, feature = "testkit"))]
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
    fn the_sea_goes_to_zero_and_a_lake_keeps_the_sources_heights() {
        // Land rising east at 1 m a pixel from 100 m; a raised, bumpy lake (a coarse source's
        // 160 m) from x 40 to 80; the sea west of x 10.5.
        let polys = vec![square(Kind::Sea, 0, 0.0, 0.0, 10.5, 256.0), square(Kind::Lake, 7, 40.0, 100.0, 80.0, 140.0)];
        let wt = water_tile(&polys);
        let mut e: Vec<f32> = (0..65536).map(|p| 100.0 + (p % 256) as f32).collect();
        for y in 100..140 {
            for x in 40..80 {
                e[y * 256 + x] = 160.0 + ((x * 7 + y * 3) % 11) as f32;
            }
        }
        let was = e.clone();
        let n = flatten(&mut e, &wt);
        assert!((0..256).all(|y| (0..10).all(|x| e[y * 256 + x] == 0.0)));
        // The pixel half sea (x 10): half 0, half its own (110 m).
        assert_eq!(e[50 * 256 + 10], 0.5 * 110.0);
        assert_eq!(n, 256 * 11, "only the sea's pixels changed");
        assert!(
            (100..140).all(|y| (40..80).all(|x| e[y * 256 + x] == was[y * 256 + x])),
            "the lake as its source has it"
        );
        assert_eq!(e[50 * 256 + 50], 150.0, "land unchanged");
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
        let f = |tags: Vec<u32>, g: Vec<u32>| Feature {
            id: None,
            tags,
            geom_type: Some(3),
            geometry: g,
            ..Default::default()
        };
        let l = Layer {
            name: "water".into(),
            version: 2,
            extent: 4096,
            keys: vec!["class".into(), "id".into(), "intermittent".into()],
            values: vec![
                Value::String("ocean".into()),
                Value::String("lake".into()),
                Value::Uint(41279034),
                Value::Int(0),
                Value::String("river".into()),
                Value::Int(1),
            ],
            features: vec![
                f(vec![0, 0], sq(0, 0, 160)),
                f(vec![0, 1, 1, 2, 2, 3], sq(1600, 1600, 160)),
                f(vec![0, 4], sq(3200, 0, 160)),
                f(vec![0, 1, 2, 5], sq(0, 3200, 160)),
            ],
            unknown: vec![],
        };
        let t = Tile { layers: vec![l], unknown: vec![] };
        let polys = polys_of_mvt(&t.encode(), 12, 1, 1).unwrap();
        assert_eq!(polys.len(), 2);
        assert_eq!((polys[0].kind, polys[1].kind, polys[1].id), (Kind::Sea, Kind::Lake, 41279034));
        let wt = water_tile(&polys);
        assert_eq!(wt.sea[5 * 256 + 5], 16);
        assert_eq!(wt.lake[105 * 256 + 105], 16);
        assert!(wt.river.is_empty(), "the terrain's water has no rivers");
        // The depth's: the river too (the intermittent lake still left), the terrain's shares as
        // they were.
        let dp = polys_of_mvt_with(&t.encode(), 12, 1, 1, true).unwrap();
        assert_eq!(dp.iter().map(|p| p.kind).collect::<Vec<_>>(), [Kind::Sea, Kind::Lake, Kind::River]);
        let dw = water_tile(&dp);
        assert_eq!((&dw.sea, &dw.lake, &dw.lake_of), (&wt.sea, &wt.lake, &wt.lake_of));
        assert_eq!((dw.river_at(5 * 256 + 205), dw.river_at(5 * 256 + 5)), (16, 0));
        assert_ne!(polys_digest(&dp), polys_digest(&polys), "a river changes the depth's digest");
    }

    #[test]
    fn a_river_takes_only_what_the_sea_and_lakes_leave() {
        let wt = water_tile(&[
            square(Kind::River, OWN, 0.0, 0.0, 100.0, 100.0),
            square(Kind::Sea, 0, 0.0, 0.0, 50.0, 100.0),
            square(Kind::Lake, 4, 50.0, 0.0, 75.0, 100.0),
        ]);
        assert_eq!((wt.river_at(10 * 256 + 10), wt.river_at(10 * 256 + 60), wt.river_at(10 * 256 + 90)), (0, 0, 16));
        assert!(!wt.is_empty());
        assert!(!water_tile(&[square(Kind::River, OWN, 0.0, 0.0, 10.0, 10.0)]).is_empty());
    }

    #[test]
    fn a_river_keeps_the_sources_heights() {
        // A noisy river: the terrain reads no rivers (`polys`), and a tile with one flattens nothing
        // but its sea.
        let wt = water_tile(&[square(Kind::River, OWN, 20.0, 100.0, 236.0, 120.0)]);
        let mut e: Vec<f32> = (0..65536).map(|p| 200.0 + ((p * 37) % 17) as f32).collect();
        let was = e.clone();
        assert_eq!(flatten(&mut e, &wt), 0);
        assert_eq!(e, was);
    }

    /// The key own_key had until task #464's fix (gap 11).
    fn old_own_key(z: u8, x: u32, y: u32, k: usize) -> u64 {
        (1u64 << 63) | (u64::from(z) << 56) ^ (u64::from(x) << 30) ^ (u64::from(y) << 4) ^ k as u64
    }

    /// Own keys (gap 11): features 0-15 keep their old key in every tile z6-14 (the old key's
    /// whole range where it is its tile's own: feature 16 on met the next rows'); and every key is
    /// one tile's feature's alone, where the old ones collided too.
    #[test]
    fn own_keys() {
        for z in 6..=14u8 {
            let n = 1u32 << z;
            for x in (0..n).step_by((n / 64).max(1) as usize).chain([n - 1]) {
                for y in 0..n {
                    for k in 0..16 {
                        assert_eq!(own_key(z, x, y, k), Some(old_own_key(z, x, y, k)), "{z}/{x}/{y} {k}");
                    }
                }
            }
        }
        // (Distinct over whole columns, a feature run past 16 in each tile, at both ends of z.)
        for (z, x) in [(6u8, 0u32), (6, 63), (14, 0), (14, 9999), (14, 16383)] {
            let mut seen = std::collections::HashMap::new();
            let ys: Vec<u32> = if z == 6 { (0..64).collect() } else { (0..64).chain(16320..16384).collect() };
            for &y in &ys {
                for k in (0..300).chain([(1 << 28) + 15]) {
                    let key = own_key(z, x, y, k).unwrap();
                    assert!(key & OWN != 0);
                    assert_eq!(seen.insert(key, (y, k)), None, "{z}/{x}/{y} {k}");
                }
            }
        }
        assert_eq!(old_own_key(12, 5, 0, 16), old_own_key(12, 5, 1, 0), "the old collision");
        assert_ne!(own_key(12, 5, 0, 16), own_key(12, 5, 1, 0));
        assert_ne!(own_key(12, 5, 0, 15), own_key(12, 6, 0, 15));
        assert_ne!(own_key(11, 5, 0, 0), own_key(12, 5, 0, 0));
        assert_ne!(own_key(14, 0, 0, 16), own_key(13, 0, 0, 16));
        assert_eq!(
            (own_key(15, 0, 0, 0), own_key(12, 1 << 12, 0, 0), own_key(12, 0, 0, (1 << 28) + 16)),
            (None, None, None)
        );
    }
}
