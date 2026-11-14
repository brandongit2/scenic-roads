//! The built set (docs/steps.md §14): the data-available layers drawn from the elevation data built
//! into the map, each place holding the one dataset the terrain took its heights from there: the
//! first in the order (rank, resolution, ground over surface, id: inputs::dem::declared::order_key)
//! whose coverage holds the place (its cells, its regions' outlines and `exclude`:
//! declared::Cover) and that has data there, as the sampler takes it. Where a dataset has data is
//! read from what the build holds, never its heights:
//!
//! - a GeoTIFF kept in the store (`[data] store`): its files' footprints, and their fill masks (a
//!   value `MASK_FILLED` or more is a void: GLO-30's) or, without masks, their own valid pixels, at
//!   the coarsest level no wider than the layer's pixel (`level_for`), read from the store;
//! - a GeoTIFF read over the network: its footprints within its presence tiles less its hollow tiles
//!   (the input check's measures, at the deep packs' zoom, z10: inputs::inputs::elevation);
//! - a pyramid read tile by tile (GSI's): a tile its store (crate::pyramids) holds with data, by the
//!   store's index of the tiles' entries, no tile read;
//! - a pyramid published at every zoom (the Terrarium tiles): everywhere in the Web Mercator plane.
//!
//! A pixel holds its centre's dataset; stored to `BUILT_MAXZ` (z10: GLO-30's voids, a fifth of a
//! mountain tile in places, make nearly every land tile mixed at z12), above it each pixel the union
//! of the four under it. Its pieces' ids are the datasets', described beside the tiles
//! (`codecs::available::datasets_logical`) for the server. The z6 tiles, the root, the polar caps
//! and the record as the best set's (super::drive), keyed by the datasets meeting each z6 tile.

use super::{combine, drive, Cell, Emit, Made, Record, Sets, Stored, Z6};
use anyhow::{Context, Result};
use buildstore::out::Out;
use codecs::available::{self as fmt, BUILT_MAXZ, PIXELS, SIDE};
use det::Det;
use inputs::dem::declared::{self, Cover, Data};
use inputs::dem::proj::Projection;
use rayon::prelude::*;
use roadcore::polar::Pole;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use store::nasfs::NasPath;

/// What the datasets' extents make of a tile: bumped with any change to the tiles' bytes for the
/// same datasets (the rules above, the levels read, the zoom stored to). Each tile's key and the
/// step's own version.
pub const KEY_V: u32 = 1;

/// The theme the elevation datasets are of.
pub const THEME: &str = "terrain";

/// The presence tiles' zoom (the deep packs', inputs::dem::zoom::PACK_Z): the built set's tiles are
/// stored to it, so a z10 tile is in a dataset's presence or not.
const _PRESENCE_Z: () = assert!(inputs::dem::zoom::PACK_Z == BUILT_MAXZ);

/// How a dataset's data is told present at a point.
enum Has {
    /// Everywhere in the Web Mercator plane (a pyramid published at every zoom).
    Everywhere,
    /// Its files in the store: each one's footprint (in `crs`) and the file read for where it has
    /// data, its fill mask (`mask`) or itself.
    Stored { crs: u16, dir: NasPath, files: Vec<(String, [f64; 4], bool)> },
    /// Within its footprints (in `crs`), in its presence tiles less its hollow tiles (None: its
    /// footprints alone, measured before presence was).
    Present { crs: u16, footprints: Vec<[f64; 4]>, tiles: Option<HashSet<(u32, u32)>> },
    /// A tile at `zoom` its store (`store`, crate::pyramids) holds with data.
    Tiles { zoom: u8, store: NasPath },
}

/// A dataset as the built set reads it.
pub struct Dataset {
    pub id: String,
    /// Its line in a key: `id keyed` (its declaration and data).
    line: String,
    measured: declared::Measured,
    cover: Cover,
    has: Has,
    /// What the server shows of it (`datasets_logical`).
    pub info: Value,
}

/// The accepted elevation datasets (`a`), in the terrain's order, each with where its data is read
/// from (`root`: the store's and the pyramids' folders).
pub fn datasets(a: &inputs::inputs::Accepted, root: &store::nasfs::NasRoot) -> Result<Vec<Dataset>> {
    let read = |n: &str| -> Result<Vec<u8>> { Ok(a.root().join(n).read()?) };
    let mut out = Vec::new();
    for (k, d) in declared::accepted(a)?.into_iter().enumerate() {
        let cover = declared::cover_read(&d, &read)?;
        let parse = |i: &str| -> Result<declared::TileIndex> {
            declared::parse_index(&a.read(i)?).map_err(|e| anyhow::anyhow!("{}: {i}: {e}", d.id))
        };
        let (has, method, grain_m) = match &d.declaration.data {
            Data::Tiles { every_zoom: true, .. } => (Has::Everywhere, "everywhere", None),
            Data::Tiles { url, zoom, .. } => {
                let store = root.join(crate::pyramids::DIR).join(crate::pyramids::folder(url));
                let z = u8::try_from(*zoom).context("a pyramid's zoom")?;
                (Has::Tiles { zoom: z, store }, "tiles", Some(roadcore::geo::merc_m(0.0) / f64::from(1u32 << z)))
            }
            Data::Geotiff { index, store, .. } => {
                let crs = d.measured.crs.with_context(|| format!("{}: its CRS not measured", d.id))?;
                let ix = match index {
                    Some(i) => Some(parse(i)?),
                    None => None,
                };
                let files: Vec<(String, [f64; 4], Option<String>)> = ix
                    .map(|ix| {
                        ix.files
                            .into_iter()
                            .zip(ix.masks)
                            .filter_map(|((w, b), m)| match w {
                                declared::Where::Url(u) => Some((u, b, m)),
                                declared::Where::File(_) => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if *store {
                    let dir = root.join("sources/fetched").join(&d.id);
                    let masked = files.iter().any(|f| f.2.is_some());
                    let files = files
                        .into_iter()
                        .map(|(u, b, m)| match m {
                            Some(m) => (m, b, true),
                            None => (u, b, false),
                        })
                        .collect();
                    let px = roadcore::geo::merc_m(0.0) / (SIDE as f64 * f64::from(1u32 << BUILT_MAXZ));
                    (Has::Stored { crs, dir, files }, if masked { "fill masks" } else { "files" }, Some(px))
                } else {
                    let tiles = d.measured.presence.as_ref().map(|p| {
                        let hollow: HashSet<[u32; 2]> = d.measured.hollow.iter().flatten().copied().collect();
                        p.iter().filter(|t| !hollow.contains(*t)).map(|t| (t[0], t[1])).collect()
                    });
                    let footprints = files.into_iter().map(|f| f.1).collect();
                    let grain = roadcore::geo::merc_m(0.0) / f64::from(1u32 << inputs::dem::zoom::PACK_Z);
                    (Has::Present { crs, footprints, tiles }, "presence", Some(grain))
                }
            }
        };
        let facts = &a.index.files.get(&d.path).with_context(|| format!("{}: not in the index", d.id))?.facts;
        let desc = facts.get("description").cloned().unwrap_or(Value::Null);
        let info = json!({
            "id": d.id,
            "order": k,
            "name": desc.get("name"),
            "label": desc.get("label").or(desc.get("name")),
            "credit": desc.get("credit"),
            "licence": desc.get("licence"),
            "licence_url": desc.get("licence_url"),
            "source": desc.get("source"),
            "resolution_m": d.measured.resolution,
            "metrics": {"surface": d.declaration.surface},
            "read": {"method": method, "grain_m": grain_m},
        });
        out.push(Dataset { id: d.id.clone(), line: format!("{} {}", d.id, d.keyed), measured: d.measured, cover, has, info });
    }
    Ok(out)
}

/// Whether dataset `d`'s files or tiles may reach box `b` (w, s, e, n, degrees), its coverage aside.
fn reaches(d: &Dataset, b: [f64; 4]) -> bool {
    match &d.has {
        Has::Everywhere | Has::Tiles { .. } => true,
        Has::Stored { crs, files, .. } => {
            let nb = native(*crs, b);
            files.iter().any(|f| meets(&f.1, &nb))
        }
        Has::Present { crs, footprints, tiles } => {
            let nb = native(*crs, b);
            footprints.iter().any(|f| meets(f, &nb))
                && tiles.as_ref().is_none_or(|t| z10_in(b).iter().any(|k| t.contains(k)))
        }
    }
}

fn meets(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Box `b` (degrees) in CRS `crs`: itself where geographic, its edges projected (grown 1 km) where
/// projected.
fn native(crs: u16, b: [f64; 4]) -> [f64; 4] {
    if inputs::dem::proj::projected(crs) {
        declared::native_box(crs, b, 1000.0).unwrap_or([f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY])
    } else {
        b
    }
}

/// The z10 tiles box `b` (degrees, within the Mercator plane) meets.
fn z10_in(b: [f64; 4]) -> Vec<(u32, u32)> {
    let z = inputs::dem::zoom::PACK_Z;
    let lat = |v: f64| v.clamp(-roadcore::geo::MERCATOR_LAT, roadcore::geo::MERCATOR_LAT);
    let (x0, y0) = roadcore::geo::tile_of(roadcore::geo::merc(b[0], lat(b[3])), z);
    let (x1, y1) = roadcore::geo::tile_of(roadcore::geo::merc(b[2], lat(b[1])), z);
    (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))).collect()
}

/// The key of a z6 tile ("6/x/y") or a pole's square from the datasets meeting box `b`: each one's
/// line, where its regions cut it there (declared::cut_note), and what it's told present by there
/// (its presence tiles less its hollow ones in the box; a pyramid's store's archives in the z6
/// tile's shard: `shard`).
fn key_of(ds: &[Dataset], b: [f64; 4], shard: Option<&str>, archives: &HashMap<String, Vec<String>>) -> Option<String> {
    let mut lines = Vec::new();
    for d in ds.iter().filter(|d| d.cover.meets(b) && reaches(d, b)) {
        let mut l = d.line.clone();
        if let Some(n) = declared::cut_note(&d.measured, &d.cover, b) {
            l = format!("{l} {n}");
        }
        match &d.has {
            Has::Present { tiles: Some(t), .. } => {
                let mut inb: Vec<String> =
                    z10_in(b).into_iter().filter(|k| t.contains(k)).map(|(x, y)| format!("{x}/{y}")).collect();
                inb.sort();
                l = format!("{l} presence {}", store::naming::hash16(inb.join(" ").as_bytes()));
            }
            Has::Tiles { .. } => {
                let names = shard.and_then(|s| archives.get(&format!("{}\n{s}", d.id))).cloned().unwrap_or_default();
                l = format!("{l} stored {}", names.join(","));
            }
            _ => {}
        }
        lines.push(l);
    }
    if lines.is_empty() {
        return None;
    }
    let mut h = blake3::Hasher::new();
    h.update(format!("available-built {KEY_V}\n").as_bytes());
    for l in &lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    Some(h.finalize().to_hex()[..16].to_string())
}

/// Each pyramid's store's archives by "<dataset>\n<shard>", as its index names them (none yet:
/// none), for the keys.
fn archives(ds: &[Dataset]) -> Result<HashMap<String, Vec<String>>> {
    let mut out = HashMap::new();
    for d in ds {
        if let Has::Tiles { store, .. } = &d.has {
            let ix = buildstore::rawpack::Index::load(store)?;
            for (s, packs) in &ix.shards {
                out.insert(format!("{}\n{s}", d.id), packs.iter().map(|p| p.name.clone()).collect());
            }
        }
    }
    Ok(out)
}

// ---- Where a dataset has data, near a place ------------------------------------------------------

/// A file's level read over a window: its transform, the window's first column and row, its size,
/// and whether each pixel has data.
struct Window {
    gt: [f64; 6],
    c0: i64,
    r0: i64,
    w: i64,
    h: i64,
    data: Vec<bool>,
}

impl Window {
    /// Whether the pixel holding native point (`x`, `y`) has data (outside the window: none).
    fn has(&self, x: f64, y: f64) -> bool {
        let c = ((x - self.gt[0]) / self.gt[1]).floor() as i64 - self.c0;
        let r = ((y - self.gt[3]) / self.gt[5]).floor() as i64 - self.r0;
        (0..self.w).contains(&c) && (0..self.h).contains(&r) && self.data[(r * self.w + c) as usize]
    }
}

/// The coarsest of a file's levels whose pixels, on the ground at latitude `lat` (a geographic
/// file's: its widest, nearest the equator), are no wider than `px_m`; its full resolution where
/// none is.
fn level_for(t: &buildstore::geotiff::Tiff, geographic: bool, lat: f64, px_m: f64) -> Result<usize> {
    let mut best = 0;
    for l in 0..t.images().len() {
        let g = t.transform(l)?;
        let (gx, gy) = if geographic {
            let m = roadcore::geo::merc_m(0.0) / 360.0;
            (g[1].abs() * m * lat.to_radians().dcos(), g[5].abs() * m)
        } else {
            (g[1].abs(), g[5].abs())
        };
        if gx.max(gy) <= px_m {
            best = l;
        }
    }
    Ok(best)
}

/// What's read of a dataset near a place: its stored files' windows (with their footprints) or its
/// pyramid's tiles with data.
enum Near<'a> {
    None,
    Everywhere,
    Stored { proj: Option<Projection>, windows: Vec<([f64; 4], Window)> },
    Present { proj: Option<Projection>, footprints: Vec<[f64; 4]>, tiles: Option<&'a HashSet<(u32, u32)>> },
    Tiles { zoom: u8, data: HashMap<u64, bool> },
}

/// What's read of `d` near box `b` (degrees; `px_m`: the layer's pixel there, on the ground): a
/// stored file's window over the box, at the coarsest level no wider (`level_for`); a pyramid's
/// tiles in z6 tile `shard` (its store's archives' entries, newest first).
fn near<'a>(d: &'a Dataset, b: [f64; 4], px_m: f64, shard: Option<(u32, u32)>) -> Result<Near<'a>> {
    if !d.cover.meets(b) || !reaches(d, b) {
        return Ok(Near::None);
    }
    Ok(match &d.has {
        Has::Everywhere => Near::Everywhere,
        Has::Present { crs, footprints, tiles } => {
            let nb = native(*crs, b);
            Near::Present {
                proj: Projection::of(*crs),
                footprints: footprints.iter().filter(|f| meets(f, &nb)).copied().collect(),
                tiles: tiles.as_ref(),
            }
        }
        Has::Stored { crs, dir, files } => {
            let geographic = !inputs::dem::proj::projected(*crs);
            let nb = native(*crs, b);
            let mut windows = Vec::new();
            for (url, fp, mask) in files.iter().filter(|f| meets(&f.1, &nb)) {
                let Some(s) = inputs::dem::generic::stored(&d.id, dir, url, None)? else { continue };
                let t = buildstore::geotiff::Tiff::open(s.read)?;
                let lat = if fp[1] > 0.0 { fp[1] } else if fp[3] < 0.0 { fp[3] } else { 0.0 };
                let level = level_for(&t, geographic, if geographic { lat } else { 0.0 }, px_m)?;
                let gt = t.transform(level)?;
                let img = t.level(level)?.clone();
                // (The box in the level's pixels, cut to the file.)
                let (ca, cb) = ((nb[0] - gt[0]) / gt[1], (nb[2] - gt[0]) / gt[1]);
                let (ra, rb) = ((nb[3] - gt[3]) / gt[5], (nb[1] - gt[3]) / gt[5]);
                let c0 = (ca.min(cb).floor().max(0.0) as u32).min(img.width);
                let c1 = (ca.max(cb).ceil().max(0.0) as u32).min(img.width);
                let r0 = (ra.min(rb).floor().max(0.0) as u32).min(img.height);
                let r1 = (ra.max(rb).ceil().max(0.0) as u32).min(img.height);
                if c0 >= c1 || r0 >= r1 {
                    continue;
                }
                let px = t.read_window(level, c0, r0, c1 - c0, r1 - r0).with_context(|| format!("{}: {url}", d.id))?;
                let nodata = t.nodata();
                let data = px
                    .iter()
                    .map(|&v| {
                        if *mask {
                            v.is_finite() && v < declared::MASK_FILLED
                        } else {
                            inputs::dem::generic::valid(v, nodata)
                        }
                    })
                    .collect();
                windows.push((
                    *fp,
                    Window { gt, c0: i64::from(c0), r0: i64::from(r0), w: i64::from(c1 - c0), h: i64::from(r1 - r0), data },
                ));
            }
            Near::Stored { proj: Projection::of(*crs), windows }
        }
        Has::Tiles { zoom, store } => {
            let mut data = HashMap::new();
            let ix = buildstore::rawpack::Index::load(store)?;
            let shards: Vec<String> = match shard {
                Some((x, y)) => vec![format!("{}-{x}-{y}", fmt::PACK_Z)],
                None => ix.shards.keys().cloned().collect(),
            };
            for s in shards {
                for p in ix.of(&s).iter().rev() {
                    let (_, entries) = buildstore::rawpack::entries_of(&store.join("packs").join(&p.name))?;
                    for e in entries {
                        data.entry(e.key).or_insert(e.len > 0);
                    }
                }
            }
            Near::Tiles { zoom: *zoom, data }
        }
    })
}

impl Near<'_> {
    /// Whether the dataset has data at `lon`, `lat` (its coverage aside).
    fn has(&self, lon: f64, lat: f64) -> bool {
        let in_fp = |f: &[f64; 4], x: f64, y: f64| f[0] <= x && x < f[2] && f[1] < y && y <= f[3];
        let at = |p: &Option<Projection>| p.as_ref().map_or((lon, lat), |p| p.project(lon, lat));
        let merc = lat.abs() <= roadcore::geo::MERCATOR_LAT;
        match self {
            Near::None => false,
            Near::Everywhere => merc,
            Near::Present { proj, footprints, tiles } => {
                let z10 = || roadcore::geo::tile_of(roadcore::geo::merc(lon, lat), inputs::dem::zoom::PACK_Z);
                if tiles.is_some_and(|t| !merc || !t.contains(&z10())) {
                    return false;
                }
                let (x, y) = at(proj);
                footprints.iter().any(|f| in_fp(f, x, y))
            }
            Near::Stored { proj, windows } => {
                let (x, y) = at(proj);
                // (The first file whose footprint holds it, as the sampler takes it.)
                windows.iter().find(|(f, _)| in_fp(f, x, y)).is_some_and(|(_, w)| w.has(x, y))
            }
            Near::Tiles { zoom, data } => {
                if !merc {
                    return false;
                }
                let (tx, ty) = roadcore::geo::tile_of(roadcore::geo::merc(lon, lat), *zoom);
                data.get(&roadcore::archive::tile_key(*zoom, tx, ty)).copied().unwrap_or(false)
            }
        }
    }
}

// ---- The tiles -----------------------------------------------------------------------------------

/// The datasets near a place, each with what's read of it, in the order.
struct Here<'a> {
    ds: &'a [Dataset],
    near: Vec<Near<'a>>,
    /// Each dataset's one-piece set, by number in `sets`.
    one: Vec<u32>,
}

impl Here<'_> {
    /// The set at `lon`, `lat`: the first dataset whose coverage holds it and that has data there
    /// (`skip`: those known not to reach the pixel's tile; `whole`: those whose coverage holds it
    /// whole, not asked again).
    fn at(&self, lon: f64, lat: f64, skip: &[bool], whole: &[bool]) -> u32 {
        for (i, n) in self.near.iter().enumerate() {
            if skip[i] {
                continue;
            }
            if (whole[i] || self.ds[i].cover.contains(lon, lat)) && n.has(lon, lat) {
                return self.one[i];
            }
        }
        0
    }
}

/// The z10 tile (x, y)'s cell: each pixel's centre's set (`Here::at`), each dataset's coverage
/// asked of the tile's box first.
fn z10_cell(here: &Here, x: u32, y: u32) -> Cell {
    let b = roadcore::geo::tile_box(BUILT_MAXZ, x, y);
    let n = here.ds.len();
    let mut skip = vec![false; n];
    let mut whole = vec![false; n];
    for i in 0..n {
        match here.ds[i].cover.contains_box(b) {
            Some(false) => skip[i] = true,
            Some(true) => whole[i] = true,
            None => {}
        }
        // (Nothing read reaches the tile: its presence tiles are this zoom's, `_PRESENCE_Z`.)
        skip[i] |= match &here.near[i] {
            Near::None => true,
            Near::Present { tiles: Some(t), .. } => !t.contains(&(x, y)),
            Near::Stored { proj: None, windows } => !windows.iter().any(|(f, _)| meets(f, &b)),
            _ => false,
        };
    }
    // (The first dataset not skipped there everywhere in the tile: uniform.)
    if let Some(i) = (0..n).find(|&i| !skip[i]) {
        if whole[i] && matches!(here.near[i], Near::Everywhere) {
            return Cell::Uniform(here.one[i]);
        }
    } else {
        return Cell::Uniform(0);
    }
    let w = f64::from(SIDE as u32) * f64::from(1u32 << BUILT_MAXZ);
    let lons: Vec<f64> = (0..SIDE).map(|i| roadcore::geo::merc_lon((f64::from(x) * SIDE as f64 + i as f64 + 0.5) / w)).collect();
    let mut px = vec![0u32; PIXELS];
    for j in 0..SIDE {
        let lat = roadcore::geo::merc_lat((f64::from(y) * SIDE as f64 + j as f64 + 0.5) / w);
        for (i, &lon) in lons.iter().enumerate() {
            px[j * SIDE + i] = here.at(lon, lat, &skip, &whole);
        }
    }
    uniform_or(px)
}

fn uniform_or(px: Vec<u32>) -> Cell {
    if px.iter().all(|&p| p == px[0]) {
        Cell::Uniform(px[0])
    } else {
        Cell::Raster(px)
    }
}

/// Tile z/x/y (z from 6 to `BUILT_MAXZ`) from its z10 cells, its stored tiles under it emitted.
fn quad(here: &Here, z: u8, x: u32, y: u32, sets: &mut Sets, emit: &mut Emit) -> Cell {
    if z == BUILT_MAXZ {
        return z10_cell(here, x, y);
    }
    let kids: [Cell; 4] = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
        let c = quad(here, z + 1, 2 * x + dx, 2 * y + dy, sets, emit);
        if matches!(c, Cell::Raster(_)) {
            emit(z + 1, 2 * x + dx, 2 * y + dy, &c, sets);
        }
        c
    });
    combine(&kids, sets)
}

/// The layer's pixel at latitude `lat`, on the ground (m).
fn pixel_m(lat: f64) -> f64 {
    roadcore::geo::merc_m(lat) / (SIDE as f64 * f64::from(1u32 << BUILT_MAXZ))
}

/// Builds the built set's layers from `ds` (the accepted datasets in their order, `datasets`), making
/// again only what their keys say moved, and the datasets' descriptions beside them; `report` hears
/// the z6 tiles done of those to make.
pub fn build(out: &mut Out, ds: &[Dataset], report: &dyn Fn(u64, u64)) -> Result<Made> {
    let ids: Vec<String> = ds.iter().map(|d| d.id.clone()).collect();
    let arch = archives(ds)?;
    let n6 = 1u32 << fmt::PACK_Z;
    let keys: BTreeMap<String, String> = (0..n6)
        .flat_map(|y| (0..n6).map(move |x| (x, y)))
        .filter_map(|(x, y)| {
            let shard = format!("{}-{x}-{y}", fmt::PACK_Z);
            let k = key_of(ds, roadcore::geo::tile_box(fmt::PACK_Z, x, y), Some(&shard), &arch)?;
            Some((format!("{}/{x}/{y}", fmt::PACK_Z), k))
        })
        .collect();
    let poles: Vec<(Pole, String, bool)> = [Pole::North, Pole::South]
        .into_iter()
        .map(|p| match key_of(ds, roadcore::polar::lat_box(p, 0, 0, 0), None, &arch) {
            Some(k) => (p, k, false),
            None => (p, format!("pole {} none", p.letter()), true),
        })
        .collect();
    let record = Record { theme: THEME, set: fmt::BUILT, v: KEY_V, ids: &ids };
    let made = drive(
        out,
        &record,
        keys,
        poles,
        &mut |stale, sink| {
            // (Some z6 tiles at once, each on a thread of its own with its own sets; put in turn.)
            for chunk in stale.chunks(rayon::current_num_threads().max(1) * 2) {
                let made: Vec<Result<Z6>> = chunk
                    .par_iter()
                    .map(|&(x, y)| {
                        let b = roadcore::geo::tile_box(fmt::PACK_Z, x, y);
                        let mid = (b[1] + b[3]) / 2.0;
                        let mut sets = Sets::default();
                        let near = ds.iter().map(|d| near(d, b, pixel_m(mid), Some((x, y)))).collect::<Result<_>>()?;
                        let one = (0..ds.len()).map(|i| sets.id(vec![i as u32])).collect();
                        let here = Here { ds, near, one };
                        let mut tiles: Vec<Stored> = Vec::new();
                        let cell = quad(&here, fmt::PACK_Z, x, y, &mut sets, &mut |z, tx, ty, c, s| {
                            let (b, raw) = super::stored(c, s, &ids);
                            tiles.push((z, tx, ty, b, raw));
                        });
                        Ok(Z6::of(x, y, &cell, &sets, &ids, tiles))
                    })
                    .collect();
                for z in made {
                    sink(z?)?;
                }
            }
            Ok(())
        },
        &mut |pole, sets, emit| {
            // (Each pixel of the pole's square at its finest zoom, by its centre; then the quadtree.)
            let b = roadcore::polar::lat_box(pole, 0, 0, 0);
            let pm = roadcore::polar::pixel_m(fmt::POLAR_MAXZ);
            let near: Vec<Near> = ds.iter().map(|d| near(d, b, pm, None)).collect::<Result<_>>()?;
            let one = (0..ds.len()).map(|i| sets.id(vec![i as u32])).collect();
            let here = Here { ds, near, one };
            let side = SIDE << fmt::POLAR_MAXZ;
            let (skip, whole) = (vec![false; ds.len()], vec![false; ds.len()]);
            let grid: Vec<u32> = (0..side * side)
                .into_par_iter()
                .map(|k| {
                    let (i, j) = (k % side, k / side);
                    let gx = -roadcore::polar::HALF + (i as f64 + 0.5) * pm;
                    let gy = roadcore::polar::HALF - (j as f64 + 0.5) * pm;
                    let (lon, lat) = roadcore::polar::inverse(pole, gx, gy);
                    here.at(lon, lat, &skip, &whole)
                })
                .collect();
            Ok(polar_quad(&grid, side, 0, 0, 0, sets, emit))
        },
        report,
    )?;
    let info: Vec<&Value> = ds.iter().map(|d| &d.info).collect();
    out.put_bytes(&fmt::datasets_logical(THEME), "json", &serde_json::to_vec_pretty(&info)?)?;
    Ok(made)
}

/// A pole's tile z/x/y from its finest zoom's pixels (`grid`, `side` a side), its stored tiles under
/// it emitted.
fn polar_quad(grid: &[u32], side: usize, z: u8, x: u32, y: u32, sets: &mut Sets, emit: &mut Emit) -> Cell {
    if z == fmt::POLAR_MAXZ {
        let mut px = vec![0u32; PIXELS];
        for j in 0..SIDE {
            let row = (y as usize * SIDE + j) * side + x as usize * SIDE;
            px[j * SIDE..(j + 1) * SIDE].copy_from_slice(&grid[row..row + SIDE]);
        }
        return uniform_or(px);
    }
    let kids: [Cell; 4] = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
        let c = polar_quad(grid, side, z + 1, 2 * x + dx, 2 * y + dy, sets, emit);
        if matches!(c, Cell::Raster(_)) {
            emit(z + 1, 2 * x + dx, 2 * y + dy, &c, sets);
        }
        c
    });
    combine(&kids, sets)
}

#[cfg(test)]
mod tests;
