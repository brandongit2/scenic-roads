//! The terrain finer than its base (docs/steps.md §2, Terrain, "Finer than the base"): where a
//! declared elevation dataset (docs/inputs.md §6.8, the roads' datasets) is finer than the terrain's
//! base, the map's terrain goes on past the base's finest zoom (`terrain_pack::max_zoom_at`) to the
//! zoom that dataset supports, from the dataset read at its full resolution.
//!
//! - **Which datasets:** those whose full resolution (`Measured::full`, else `resolution`) is finer
//!   than the terrain's own finest declared dataset by over a tenth (`FINER`): HRDEM, USGS 3DEP and
//!   GSI's, not MRDEM or FABDEM (30 m, as the base). None is named in code: one dropped into
//!   `inputs/elevation/` later is taken the same way.
//! - **How fine:** a dataset's zoom at a latitude is the finest whose pixels are no finer than half
//!   its cell (`zoom_for`): 2 m data to z16 in most of Canada, 3DEP's 10 m to z14 (z13 north of
//!   ~55°); a tile pyramid, its own zoom. A base tile's finest zoom (`Fine::zoom`) is the finest of the
//!   datasets meeting it there, and counts only past the base's.
//! - **What a fine tile is:** its pixels sampled from the datasets meeting its z6 tile (in their
//!   order, blended at their edges: crate::dem::generic), at their full resolution, with the stored
//!   base tile last (`BASE_URL`), so the fine data blends into the base over four base pixels where
//!   it ends; heights as each dataset gives them (no geoid conversion: the base is in mixed
//!   references itself). The base's water is kept: each pixel by its share of the base tile's sea
//!   and lakes (crate::terrain_water, at the base's zoom) takes the base's flattened height. Below
//!   zero to 0, as the base. Quantised to 0.1 m (`QUANTUM_M`), PNG at its high compression: lidar's
//!   noise below that costs 2.7× the bytes.
//! - **Only where there's data:** a base tile whose datasets have no data in a 128 × 128 grid of
//!   it (`PRESENCE`, read as their roads read them) makes none; a fine tile none of whose pixels
//!   comes from a fine dataset isn't stored (the server makes it from its ancestor, as anywhere
//!   finer than the data).
//! - **Packs:** by z10 tile (`PACK_Z`): `layers/terrain/fine/10-x-y` (docs/formats.md, Pack), each a
//!   few hundred MB at most, made by its z6 tile's terrain run after its hi pack.

use crate::dem::declared::{self, Accepted, Cover, Data, Dataset, Plan, Source};
use crate::dem::generic::{tile_resolution, Opts, Sampler};
use crate::out::Out;
use anyhow::Result;
use det::Det;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The finest zoom any dataset makes (1 m data in the far north).
pub const MAX_Z: u8 = 18;
/// The zoom of a fine pack's root tile.
pub const PACK_Z: u8 = 10;
/// A fine tile's heights are rounded to this (m).
pub const QUANTUM_M: f32 = 0.1;
/// A dataset is fine where its resolution is under this share of the terrain's finest dataset's.
pub const FINER: f64 = 0.9;
/// The presence grid's side (points) over a base tile.
pub const PRESENCE: usize = 128;
/// The stored base tiles, as the sampler reads them (a tile pyramid served from the run's memory).
pub const BASE_URL: &str = "scenic-terrain:{z}/{x}/{y}";
/// Bumped with any change to the fine tiles' bytes from the same inputs (it's in the keys of the
/// z6 tiles that make fine tiles alone).
pub const FINE_V: u32 = 1;

/// The finest zoom at latitude `lat` whose pixels are no finer than half of `res` metres.
pub fn zoom_for(res: f64, lat: f64) -> u8 {
    let mut z = 0u8;
    while z < MAX_Z && tile_resolution(z as u32 + 1, lat) >= res / 2.0 - 1e-9 {
        z += 1;
    }
    z
}

/// How a fine dataset's zoom is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    /// A tile pyramid: its own zoom.
    Pyramid(u8),
    /// A raster: its full resolution, in millimetres.
    Raster(u64),
}

impl Detail {
    pub fn zoom_at(&self, lat: f64) -> u8 {
        match self {
            Detail::Pyramid(z) => (*z).min(MAX_Z),
            Detail::Raster(mm) => zoom_for(*mm as f64 / 1000.0, lat),
        }
    }
}

/// A fine dataset as the terrain's keys and runs read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FineSet {
    pub id: String,
    /// As the keys name it (crate::dem::declared::line).
    pub line: String,
    pub cover: Cover,
    pub detail: Detail,
}

/// A dataset's full resolution (m): `full` where it's read coarser, else as read.
pub fn full_resolution(d: &Accepted) -> f64 {
    d.measured.full.unwrap_or(d.measured.resolution)
}

/// The roads' datasets `ds` finer than the terrain's datasets `terrain` (`FINER`), in their order.
pub fn sets(ds: &[Accepted], terrain: &[Accepted]) -> Vec<FineSet> {
    let base = terrain.iter().map(full_resolution).fold(f64::INFINITY, f64::min);
    ds.iter()
        .filter(|d| full_resolution(d) < base * FINER)
        .map(|d| FineSet {
            id: d.id.clone(),
            line: declared::line(d),
            cover: Cover::of(d.measured.cover.as_deref()),
            detail: match &d.declaration.data {
                Data::Tiles { zoom, .. } => Detail::Pyramid((*zoom).min(MAX_Z as u32) as u8),
                Data::Geotiff { .. } => Detail::Raster((full_resolution(d) * 1000.0).round() as u64),
            },
        })
        .collect()
}

/// Of `sets`, those meeting box `b` (w, s, e, n) and finer there than the base at latitude `lat`.
pub fn meeting(sets: &[FineSet], b: [f64; 4], lat: f64) -> Vec<&FineSet> {
    let base = crate::terrain_pack::max_zoom_at(lat);
    sets.iter().filter(|s| s.cover.meets(b) && s.detail.zoom_at(lat) > base).collect()
}

/// The poleward latitude of a tile (where its datasets are finest).
fn poleward(b: [f64; 4]) -> f64 {
    if b[1] >= 0.0 {
        b[3]
    } else if b[3] <= 0.0 {
        b[1]
    } else {
        b[3].abs().max(b[1].abs())
    }
}

/// What z6 tile `t`'s terrain key names of the fine datasets: their lines (none where none is
/// finer than the base, so such a z6 tile's key is as before).
pub fn key_lines(sets: &[FineSet], t: (u32, u32)) -> Vec<String> {
    let b = crate::stage::tile_box_grown(6, t.0, t.1, 0.0);
    meeting(sets, b, poleward(b)).into_iter().map(|s| s.line.clone()).collect()
}

/// The finest zoom the terrain has anywhere by `sets` (the catalog's terrain's maxzoom); the
/// base's (12) without them.
pub fn max_zoom(sets: &[FineSet]) -> u8 {
    let mut z = crate::terrain_pack::max_zoom_at(0.0);
    for s in sets {
        let lats: Vec<f64> = if s.cover.world {
            vec![85.0]
        } else {
            s.cover.cells.iter().map(|c| (c.1 as f64).abs().max((c.1 + 1) as f64).abs().min(85.0)).collect()
        };
        for lat in lats {
            if s.detail.zoom_at(lat) > crate::terrain_pack::max_zoom_at(lat) {
                z = z.max(s.detail.zoom_at(lat));
            }
        }
    }
    z
}

/// The fine datasets as a run reads them: the accepted version (their files, by its index) and
/// the sets.
#[derive(Clone, Debug)]
pub struct Fine {
    pub elevation: crate::inputs::Accepted,
    pub sets: Vec<FineSet>,
}

impl Fine {
    /// Of accepted version `a` (the terrain's declarations `terrain`); None when no dataset is finer
    /// than the base.
    pub fn of(a: &crate::inputs::Accepted, terrain: &[Accepted]) -> Result<Option<Fine>> {
        let sets = sets(&declared::accepted(a)?, terrain);
        Ok((!sets.is_empty()).then(|| Fine { elevation: a.clone(), sets }))
    }

    /// The plans of z6 tile `t`: the presence plan (the fine datasets meeting it, read as their
    /// roads read them) and the full one (read at full resolution, the base last).
    fn plans(&self, t: (u32, u32)) -> Result<Option<(Plan, Plan)>> {
        let b = crate::stage::tile_box_grown(6, t.0, t.1, 0.0);
        let ids: Vec<&str> = meeting(&self.sets, b, poleward(b)).iter().map(|s| s.id.as_str()).collect();
        if ids.is_empty() {
            return Ok(None);
        }
        let mut presence = declared::plan(&self.elevation, b)?;
        presence.datasets.retain(|d| ids.contains(&d.id.as_str()));
        if presence.datasets.is_empty() {
            return Ok(None);
        }
        let mut full = presence.clone();
        for d in &mut full.datasets {
            if let Source::Geotiff { read_resolution, .. } = &mut d.source {
                *read_resolution = None;
            }
        }
        // (The base, by the band of latitude its finest zoom holds: crate::terrain_pack::max_zoom_at.)
        for (zb, bands) in [
            (12u32, vec![[-180.0, -67.0, 180.0, 67.0]]),
            (11, vec![[-180.0, 67.0, 180.0, 79.0], [-180.0, -79.0, 180.0, -67.0]]),
            (10, vec![[-180.0, 79.0, 180.0, 86.0], [-180.0, -86.0, 180.0, -79.0]]),
        ] {
            full.datasets.push(Dataset {
                id: format!("terrain-z{zb}"),
                code: 0,
                resolution: tile_resolution(zb, 0.0),
                cover: Some(bands),
                source: Source::Tiles { url: BASE_URL.into(), zoom: zb, encoding: "terrarium".into() },
                convert: None,
            });
        }
        Ok(Some((presence, full)))
    }
}

/// The base tiles served to the sampler from memory (`BASE_URL`), everything else fetched.
struct BaseFetch<'a> {
    base: &'a HashMap<(u8, u32, u32), Vec<u8>>,
    inner: &'a dyn crate::fetch::Fetch,
}

impl BaseFetch<'_> {
    fn base(&self, url: &str) -> Option<Option<Vec<u8>>> {
        let rest = url.strip_prefix("scenic-terrain:")?;
        let mut p = rest.split('/').map(|s| s.parse::<u32>().ok());
        let (z, x, y) = (p.next()??, p.next()??, p.next()??);
        Some(self.base.get(&(z as u8, x, y)).cloned())
    }
}

impl crate::fetch::Fetch for BaseFetch<'_> {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
        self.inner.open(url)
    }
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.base(url) {
            Some(b) => Ok(b),
            None => self.inner.get(url),
        }
    }
    fn get_within(&self, url: &str, within: crate::fetch::Within) -> Result<Option<Vec<u8>>> {
        match self.base(url) {
            Some(b) => Ok(b),
            None => self.inner.get_within(url, within),
        }
    }
}

/// Pixel `(i, j)`'s centre of tile `z/x/y`: (lon, lat).
fn pixel_lonlat(z: u8, x: u32, y: u32, i: usize, j: usize) -> (f64, f64) {
    let n = (1u64 << z) as f64;
    let fx = (x as f64 + (i as f64 + 0.5) / 256.0) / n;
    let fy = (y as f64 + (j as f64 + 0.5) / 256.0) / n;
    (fx * 360.0 - 180.0, (std::f64::consts::PI * (1.0 - 2.0 * fy)).dsinh().datan().to_degrees())
}

/// `v` (256 × 256) at fine tile `z/x/y`'s pixel `(i, j)`, its base tile at zoom `zb` (`v` its
/// values), bilinearly, its edges clamping.
fn up(v: &[f32], zb: u8, z: u8, x: u32, y: u32, i: usize, j: usize) -> f32 {
    let k = (1u32 << (z - zb)) as f64;
    let bx = (x as f64 / k).floor();
    let by = (y as f64 / k).floor();
    let u = ((x as f64 + (i as f64 + 0.5) / 256.0) / k - bx) * 256.0 - 0.5;
    let w = ((y as f64 + (j as f64 + 0.5) / 256.0) / k - by) * 256.0 - 0.5;
    let (u, w) = (u.clamp(0.0, 255.0), w.clamp(0.0, 255.0));
    let (u0, w0) = (u.floor() as usize, w.floor() as usize);
    let (u1, w1) = ((u0 + 1).min(255), (w0 + 1).min(255));
    let (fu, fw) = ((u - u0 as f64) as f32, (w - w0 as f64) as f32);
    let a = v[w0 * 256 + u0] * (1.0 - fu) + v[w0 * 256 + u1] * fu;
    let b = v[w1 * 256 + u0] * (1.0 - fu) + v[w1 * 256 + u1] * fu;
    a * (1.0 - fw) + b * fw
}

/// A fine tile's heights finished: where the sampler has none (past the stored base), the base;
/// each pixel by its share of water (`water`, 0–1 at the base's pixels) to the base's height;
/// below zero to 0; rounded to `QUANTUM_M`.
pub fn finish(e: &mut [f32], base: &[f32], water: Option<&[f32]>, zb: u8, z: u8, x: u32, y: u32) {
    for j in 0..256 {
        for i in 0..256 {
            let k = j * 256 + i;
            let b = up(base, zb, z, x, y, i, j);
            let mut v = if e[k].is_finite() { e[k] } else { b };
            if let Some(w) = water {
                let s = up(w, zb, z, x, y, i, j).clamp(0.0, 1.0);
                v = s * b + (1.0 - s) * v;
            }
            e[k] = ((v.max(0.0) / QUANTUM_M).round() * QUANTUM_M).max(0.0);
        }
    }
}

/// Metres as a Terrarium PNG at its high compression.
pub fn encode(e: &[f32]) -> Result<Vec<u8>> {
    let mut rgb = Vec::with_capacity(e.len() * 3);
    for &v in e {
        let v = ((v + 32768.0) * 256.0).round().clamp(0.0, 16_777_215.0) as u32;
        rgb.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 256, 256);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let mut w = enc.write_header()?;
        w.write_image_data(&rgb)?;
    }
    Ok(out)
}

/// The z10 pack's logical name.
pub fn pack_logical(x: u32, y: u32) -> String {
    format!("layers/terrain/fine/{PACK_Z}-{x}-{y}")
}

/// The fine packs the manifest names under z6 tile `t`.
pub fn named_under(m: &BTreeMap<String, String>, t: (u32, u32)) -> Vec<String> {
    let p = format!("layers/terrain/fine/{PACK_Z}-");
    m.range(p.clone()..)
        .take_while(|(l, _)| l.starts_with(&p))
        .filter(|(l, _)| {
            let mut s = l[p.len()..].split('-').map(|v| v.parse::<u32>().ok());
            matches!((s.next(), s.next()), (Some(Some(x)), Some(Some(y))) if (x >> (PACK_Z - 6), y >> (PACK_Z - 6)) == t)
        })
        .map(|(l, _)| l.clone())
        .collect()
}

/// What a run of z6 tile's fine tiles did.
#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    pub base_tiles: usize,
    pub with_data: usize,
    pub tiles: usize,
    pub packs: usize,
    pub bytes: u64,
}

/// Z6 tile `t`'s fine tiles made and uploaded, from its base tiles as stored (`hi`: its hi pack's
/// tiles, the finest of each place its base), the water `water` and the fine datasets `fine`: its
/// fine packs (`pack_logical`), and those it no longer makes dropped (not saved).
pub fn make(
    out: &mut Out,
    t: (u32, u32),
    hi: &[(u8, u32, u32, Vec<u8>)],
    water: Option<&dyn crate::terrain_water::WaterSource>,
    fine: &Fine,
    fetch: &dyn crate::fetch::Fetch,
    progress: crate::rawpack::Progress,
) -> Result<Report> {
    let mut rep = Report::default();
    let before = named_under(&out.manifest, t);
    let Some((presence, full)) = fine.plans(t)? else {
        crate::terrain_pack::drop_named(out, &before);
        return Ok(rep);
    };
    // (The base tiles: each place's finest stored zoom.)
    let base: HashMap<(u8, u32, u32), Vec<u8>> = hi
        .iter()
        .filter(|(z, _, y, _)| *z == crate::terrain_pack::max_zoom_at(crate::terrain_pack::tile_lat(*z, *y)))
        .map(|(z, x, y, b)| ((*z, *x, *y), b.clone()))
        .collect();
    let mut groups: BTreeMap<(u32, u32), Vec<(u8, u32, u32)>> = BTreeMap::new();
    for &(z, x, y) in base.keys() {
        groups.entry((x >> (z - PACK_Z), y >> (z - PACK_Z))).or_default().push((z, x, y));
    }
    rep.base_tiles = base.len();
    let bf = BaseFetch { base: &base, inner: fetch };
    let store = Some(out.root().join("sources/fetched"));
    let o = Opts { fetch: &bf, store, pool: None };
    let presence = Sampler::new(&presence, &o)?;
    let full = Sampler::new(&full, &o)?;
    let fine_sets: Vec<&FineSet> = {
        let b = crate::stage::tile_box_grown(6, t.0, t.1, 0.0);
        meeting(&fine.sets, b, poleward(b))
    };
    let mut written = Vec::new();
    let n = groups.len() as u64;
    for (k, (g, mut bs)) in groups.into_iter().enumerate() {
        bs.sort();
        let made: Vec<Result<(bool, Vec<(u8, u32, u32, Vec<u8>)>)>> = bs
            .par_iter()
            .map(|&(zb, bx, by)| {
                let lat = crate::terrain_pack::tile_lat(zb, by);
                let bb = crate::stage::tile_box_grown(zb, bx, by, 0.0);
                let zf = fine_sets.iter().filter(|s| s.cover.meets(bb)).map(|s| s.detail.zoom_at(lat)).max();
                let Some(zf) = zf.filter(|&z| z > zb) else { return Ok((false, Vec::new())) };
                base_tile(&presence, &full, water, &base[&(zb, bx, by)], (zb, bx, by), zf)
            })
            .collect();
        let mut tiles = Vec::new();
        for r in made {
            let (data, ts) = r?;
            rep.with_data += data as usize;
            tiles.extend(ts);
        }
        let logical = pack_logical(g.0, g.1);
        if !tiles.is_empty() {
            tiles.sort_by_key(|t| (t.0, t.1, t.2));
            rep.tiles += tiles.len();
            rep.bytes += tiles.iter().map(|t| t.3.len() as u64).sum::<u64>();
            let mut it = tiles.into_iter().map(|(z, x, y, b)| {
                let n = b.len() as u32;
                (z, x, y, b, n)
            });
            if crate::layers::write_pack(out, "terrain", "terrarium-png", false, "fine", (PACK_Z, g.0, g.1), &mut it)?
                .is_some()
            {
                rep.packs += 1;
                written.push(logical);
            }
        }
        progress("fine packs", k as u64 + 1, n);
    }
    let gone: Vec<String> = before.into_iter().filter(|l| !written.contains(l)).collect();
    crate::terrain_pack::drop_named(out, &gone);
    Ok(rep)
}

/// Base tile `b` (its PNG `png`)'s fine tiles to zoom `zf`: whether its datasets have data in it,
/// and the tiles with some.
fn base_tile(
    presence: &Sampler,
    full: &Sampler,
    water: Option<&dyn crate::terrain_water::WaterSource>,
    png: &[u8],
    b: (u8, u32, u32),
    zf: u8,
) -> Result<(bool, Vec<(u8, u32, u32, Vec<u8>)>)> {
    let (zb, bx, by) = b;
    // (Its datasets' data in it, at all: a grid of it, as they're read for the roads.)
    let n = PRESENCE;
    let (mut lon, mut lat) = (Vec::with_capacity(n * n), Vec::with_capacity(n * n));
    for j in 0..n {
        for i in 0..n {
            let nn = (1u64 << zb) as f64;
            let fx = (bx as f64 + (i as f64 + 0.5) / n as f64) / nn;
            let fy = (by as f64 + (j as f64 + 0.5) / n as f64) / nn;
            lon.push(fx * 360.0 - 180.0);
            lat.push((std::f64::consts::PI * (1.0 - 2.0 * fy)).dsinh().datan().to_degrees());
        }
    }
    if !presence.sample(&lon, &lat, &|_| {})?.elev.iter().any(|v| v.is_finite()) {
        return Ok((false, Vec::new()));
    }
    let base = roadcore::grid::decode_terrain_png(png)?;
    let wt = match water {
        Some(w) => crate::terrain_water::tile_water(w, zb, bx, by)?,
        None => None,
    };
    let share: Option<Vec<f32>> = wt.map(|w| {
        let whole = crate::terrain_water::FULL as f32;
        w.sea.iter().zip(&w.lake).map(|(&s, &l)| (s as f32 + l as f32) / whole).collect()
    });
    let mut out = Vec::new();
    for z in zb + 1..=zf {
        let k = 1u32 << (z - zb);
        let ts: Vec<(u32, u32)> =
            (by * k..(by + 1) * k).flat_map(|y| (bx * k..(bx + 1) * k).map(move |x| (x, y))).collect();
        // (A chunk of tiles a call: 4M points at most.)
        for chunk in ts.chunks(64) {
            let (mut lon, mut lat) = (Vec::with_capacity(chunk.len() << 16), Vec::with_capacity(chunk.len() << 16));
            for &(x, y) in chunk {
                for j in 0..256 {
                    for i in 0..256 {
                        let (lo, la) = pixel_lonlat(z, x, y, i, j);
                        lon.push(lo);
                        lat.push(la);
                    }
                }
            }
            let s = full.sample(&lon, &lat, &|_| {})?;
            for (c, &(x, y)) in chunk.iter().enumerate() {
                let r = c << 16..(c + 1) << 16;
                if !s.src[r.clone()].iter().any(|&c| c != 0) {
                    continue;
                }
                let mut e = s.elev[r].to_vec();
                finish(&mut e, &base, share.as_deref(), zb, z, x, y);
                out.push((z, x, y, encode(&e)?));
            }
        }
    }
    Ok((true, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datasets_zoom_has_pixels_no_finer_than_half_its_cell() {
        // 2 m: z16 through Canada (z16's pixels 1.7 m at 45°, 1.2 m at 60°; z17's under 1 m).
        assert_eq!(zoom_for(2.0, 45.0), 16);
        assert_eq!(zoom_for(2.0, 60.0), 16);
        assert_eq!(zoom_for(2.0, 70.0), 15);
        // 3DEP's 10.3 m: z14 to ~55°, z13 north of it.
        assert_eq!(zoom_for(10.3, 40.0), 14);
        assert_eq!(zoom_for(10.3, 65.0), 13);
        // 30 m: the base's z12 at mid-latitudes.
        assert_eq!(zoom_for(30.0, 45.0), 12);
        for lat in [0.0, 30.0, 50.0, 70.0] {
            let z = zoom_for(5.0, lat);
            assert!(tile_resolution(z as u32, lat) >= 2.5 && tile_resolution(z as u32 + 1, lat) < 2.5);
        }
        assert_eq!(Detail::Pyramid(15).zoom_at(35.0), 15);
        assert_eq!(Detail::Raster(2000).zoom_at(45.0), 16);
    }

    fn set(id: &str, detail: Detail, cells: &[(i32, i32)]) -> FineSet {
        FineSet {
            id: id.into(),
            line: format!("{id} k"),
            cover: Cover { world: false, cells: cells.iter().copied().collect() },
            detail,
        }
    }

    #[test]
    fn the_keys_name_the_fine_datasets_meeting_a_z6_tile_and_finer_there() {
        let sets =
            vec![set("lidar", Detail::Raster(2000), &[(-75, 45)]), set("coarse", Detail::Raster(30000), &[(-75, 45)])];
        // 6/18/22 holds 75°W 45.5°N.
        assert_eq!(key_lines(&sets, (18, 22)), vec!["lidar k".to_string()]);
        assert!(key_lines(&sets, (10, 22)).is_empty(), "no fine dataset there: the key as before");
        assert_eq!(max_zoom(&sets), 16);
        assert_eq!(max_zoom(&[]), 12);
    }

    #[test]
    fn the_datasets_finer_than_the_terrains_are_fine() {
        let d = tempfile::tempdir().unwrap();
        let mut m = std::collections::BTreeMap::new();
        crate::inputs::elevation::accept_fixture(d.path(), &mut m);
        let a = crate::inputs::open(d.path(), &m, crate::inputs::elevation::UNIT).unwrap().unwrap();
        let terrain = declared::accepted_terrain(&a).unwrap();
        let mut ds = declared::accepted(&a).unwrap();
        assert!(sets(&ds, &terrain).is_empty(), "the fixture's roads dataset is coarser than the terrain's");
        ds[0].measured.resolution = 8.0;
        ds[0].measured.full = Some(2.0);
        let s = sets(&ds, &terrain);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].detail, Detail::Raster(2000));
    }

    #[test]
    fn a_fine_tile_takes_the_base_where_it_has_no_data_and_on_water() {
        // The base: 10 m plus its column; its lower half water.
        let base: Vec<f32> = (0..65536).map(|k| 10.0 + (k % 256) as f32).collect();
        let mut water = vec![0f32; 65536];
        for v in water.iter_mut().skip(128 * 256) {
            *v = 1.0;
        }
        let (bx, by) = (100, 200);
        // Fine tile 13/2bx/2by+1, under the base's lower half: data everywhere, but water.
        let mut e = vec![500.04f32; 65536];
        finish(&mut e, &base, Some(&water), 12, 13, bx * 2, by * 2 + 1);
        assert_eq!(e[100 * 256], 10.0, "the water's flattened height, the base's");
        // Fine tile 13/2bx/2by, dry: its data rounded to 0.1 m, the base past it.
        let mut e = vec![f32::NAN; 65536];
        e[0] = 500.04;
        finish(&mut e, &base, Some(&water), 12, 13, bx * 2, by * 2);
        assert!((e[0] - 500.0).abs() < 1e-3, "{}", e[0]);
        // (Pixel 3 lies at the base's column 1.25.)
        assert!((e[3] - 11.25).abs() < 0.06, "{}", e[3]);
        let d = roadcore::grid::decode_terrain_png(&encode(&e).unwrap()).unwrap();
        assert!(d.iter().zip(&e).all(|(a, b)| (a - b).abs() < 0.01));
    }

    #[test]
    fn the_fine_packs_under_a_z6_tile() {
        let m: BTreeMap<String, String> = [
            ("layers/terrain/fine/10-288-352", "a"),
            ("layers/terrain/fine/10-303-367", "b"),
            ("layers/terrain/fine/10-304-352", "c"),
            ("layers/terrain/hi/6-18-22", "d"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(named_under(&m, (18, 22)), vec!["layers/terrain/fine/10-288-352", "layers/terrain/fine/10-303-367"]);
    }
}
