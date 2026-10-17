//! The elevation datasets as the owner declares them (docs/inputs.md §6.8): one `<dataset>.toml` per
//! dataset in `inputs/elevation/`, a description (§5.1) with its surface, height reference, an
//! optional rank, the source byte its heights carry (`code`) and its data: GeoTIFFs by URL, a tile
//! index beside it, files dropped in `<dataset>/`, or a tile pyramid in a declared encoding. What's
//! read from the data (each GeoTIFF's CRS and resolution, a dataset's coverage) is measured by the
//! input check (crate::inputs::elevation), never typed, and kept in the accepted index's facts.
//!
//! A unit's elevations are sampled from a `Plan` (`elevation.json` in its folder, made by the unit
//! job from the accepted version it planned with): the datasets in their order (rank where given,
//! lower first, a dataset without one ranking 0; then resolution, finest first; then ground over
//! surface; then id), each with the files near the unit. The generic sampler (crate::dem::generic)
//! reads nothing else: no source is named in code.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// The height references the app knows (docs/inputs.md §6.8). Heights aren't converted yet
/// (#143's third step): the reference is recorded, and one the app doesn't know is refused.
pub const HEIGHTS: [&str; 7] = ["EGM2008", "EGM96", "CGVD2013", "NAVD88", "JGD2011", "TWVD2001", "ellipsoid-WGS84"];

/// The tile encodings (formats, decoded by generic code).
pub const ENCODINGS: [&str; 3] = ["gsi-png", "terrarium", "terrain-rgb"];

/// The CRSs a GeoTIFF dataset may be in: geographic WGS 84 and NAD83 (read as longitude, latitude,
/// as the national datasets always were), and Canada Atlas Lambert (crate::dem::proj::Atlas).
pub const CRSS: [u16; 3] = [4326, 4269, 3979];

/// A dataset's id: its declaration's file stem, lower-case letters, digits and `-`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A declaration's own fields (beside its description's).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Declaration {
    /// `ground` (bare earth) or `surface` (with trees and buildings).
    pub surface: String,
    /// The height reference (`HEIGHTS`).
    pub heights: String,
    /// Overrides the order by resolution, lower first; none ranks 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<i64>,
    /// The source byte its heights carry in the units' outputs (`src.u8`, which the map's server
    /// labels: roadcore::DemSource), 1–255; 0 for the terrain's (`terrain`).
    pub code: u8,
    /// Read by the 3D terrain (crate::terrain_sources), never by the roads' elevations.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terrain: bool,
    pub data: Data,
}

/// Where a dataset's data is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub enum Data {
    Geotiff {
        /// Files by URL (`<zip URL>#<member>` for a member of a zip, read in place).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        urls: Vec<String>,
        /// A tile index beside the declaration (GeoJSON: a feature per file, its `url`, or its
        /// `file` in `<dataset>/`, and its footprint in the index's CRS).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<String>,
        /// Files dropped in `<dataset>/`, by glob.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        /// Read at the overview nearest this (metres) rather than the full resolution.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read_resolution: Option<f64>,
        /// Each file downloaded whole once, kept in `sources/fetched/<dataset>/` (read in place
        /// where that can't be written).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        store: bool,
    },
    Tiles {
        /// A template with `{z}`, `{x}` and `{y}`.
        url: String,
        zoom: u32,
        /// `ENCODINGS`.
        encoding: String,
    },
}

/// A declaration file's own fields parsed (its description is crate::inputs::credits'): the
/// declaration, or what's wrong with it (each a sentence).
pub fn parse(bytes: &[u8]) -> std::result::Result<Declaration, Vec<String>> {
    let text = std::str::from_utf8(bytes).map_err(|_| vec!["isn't UTF-8 text".to_string()])?;
    let v: toml::Table = toml::from_str(text).map_err(|e| vec![format!("isn't TOML: {}", e.message())])?;
    let mut why = Vec::new();
    const DESCRIPTION: [&str; 10] = ["name", "what", "credit", "licence", "licence_url", "source", "retrieved", "territory", "redistribute", "notes"];
    for k in v.keys() {
        if !DESCRIPTION.contains(&k.as_str()) && !["surface", "heights", "rank", "code", "terrain", "data"].contains(&k.as_str()) {
            why.push(format!("`{k}` isn't a field of an elevation declaration (docs/inputs.md §6.8)"));
        }
    }
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let terrain = match v.get("terrain") {
        None => false,
        Some(x) => x.as_bool().unwrap_or_else(|| {
            why.push("`terrain` isn't true or false".into());
            false
        }),
    };
    let surface = s("surface").unwrap_or_default();
    if surface != "ground" && surface != "surface" {
        why.push(format!("`surface` is \"{surface}\": ground (bare earth) or surface (with trees and buildings)"));
    }
    let heights = s("heights").unwrap_or_default();
    if heights == "mixed" && !terrain {
        why.push("`heights` is \"mixed\": allowed for the terrain alone (`terrain = true`), never for the roads' elevations".into());
    } else if heights != "mixed" && !HEIGHTS.contains(&heights.as_str()) {
        why.push(format!("`heights` is \"{heights}\", a height reference the app doesn't know ({})", HEIGHTS.join(", ")));
    }
    let rank = match v.get("rank") {
        None => None,
        Some(x) => match x.as_integer() {
            Some(r) => Some(r),
            None => {
                why.push("`rank` isn't an integer".into());
                None
            }
        },
    };
    let code = match v.get("code").and_then(|x| x.as_integer()) {
        _ if terrain => {
            if v.contains_key("code") {
                why.push("`code` is the roads' source byte: a terrain dataset has none".into());
            }
            0
        }
        Some(c) if (1..=255).contains(&c) => c as u8,
        _ => {
            why.push("`code` is required, an integer 1–255: the source byte its heights carry".into());
            0
        }
    };
    let Some(d) = v.get("data").and_then(|x| x.as_table()) else {
        why.push("`[data]` is required".into());
        return Err(why);
    };
    let ds = |k: &str| d.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let strings = |k: &str, why: &mut Vec<String>| -> Vec<String> {
        match d.get(k) {
            None => Vec::new(),
            Some(x) => match x.as_array().map(|a| a.iter().map(|e| e.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()) {
                Some(Some(v)) => v,
                _ => {
                    why.push(format!("`data.{k}` isn't a list of strings"));
                    Vec::new()
                }
            },
        }
    };
    let format = ds("format").unwrap_or_default();
    let allowed: &[&str] = match format.as_str() {
        "geotiff" => &["format", "url", "urls", "index", "files", "read_resolution", "store"],
        "tiles" => &["format", "url", "zoom", "encoding"],
        f => {
            why.push(format!("`data.format` is \"{f}\": geotiff or tiles"));
            return Err(why);
        }
    };
    for k in d.keys() {
        if !allowed.contains(&k.as_str()) {
            why.push(format!("`data.{k}` isn't a field of a {format} dataset"));
        }
    }
    let data = if format == "geotiff" {
        let mut urls = strings("urls", &mut why);
        if let Some(u) = ds("url") {
            urls.insert(0, u);
        }
        let index = ds("index");
        let files = strings("files", &mut why);
        let read_resolution = match d.get("read_resolution") {
            None => None,
            Some(x) => match x.as_float().or_else(|| x.as_integer().map(|i| i as f64)) {
                Some(r) if r > 0.0 => Some(r),
                _ => {
                    why.push("`data.read_resolution` isn't a positive number of metres".into());
                    None
                }
            },
        };
        let store = d.get("store").map(|x| x.as_bool()).unwrap_or(Some(false));
        if store.is_none() {
            why.push("`data.store` isn't true or false".into());
        }
        let ways = [!urls.is_empty(), index.is_some(), !files.is_empty()].iter().filter(|b| **b).count();
        if ways != 1 {
            why.push("a geotiff dataset names its files one way: `url`/`urls`, `index` or `files`".into());
        }
        for u in &urls {
            if !(u.starts_with("https://") || u.starts_with("http://")) {
                why.push(format!("`{u}` isn't an http(s) URL"));
            }
        }
        if let Some(i) = &index {
            if !i.ends_with(".geojson") || i.contains('/') {
                why.push(format!("`data.index` is \"{i}\": a .geojson file beside the declaration"));
            }
        }
        Data::Geotiff { urls, index, files, read_resolution, store: store.unwrap_or(false) }
    } else {
        let url = ds("url").unwrap_or_default();
        for p in ["{z}", "{x}", "{y}"] {
            if !url.contains(p) {
                why.push(format!("`data.url` (\"{url}\") hasn't the placeholder {p}"));
            }
        }
        let zoom = match d.get("zoom").and_then(|x| x.as_integer()) {
            Some(z) if (0..=22).contains(&z) => z as u32,
            _ => {
                why.push("`data.zoom` is required, an integer 0–22".into());
                0
            }
        };
        let encoding = ds("encoding").unwrap_or_default();
        if !ENCODINGS.contains(&encoding.as_str()) {
            why.push(format!("`data.encoding` is \"{encoding}\", an encoding the app doesn't know ({})", ENCODINGS.join(", ")));
        }
        Data::Tiles { url, zoom, encoding }
    };
    if !why.is_empty() {
        return Err(why);
    }
    Ok(Declaration { surface, heights, rank, code, terrain, data })
}

// ---- What's measured from the data ---------------------------------------------------------------

/// What the input check reads from a dataset's data (`facts.measured`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    /// The GeoTIFFs' CRS (EPSG); None for a tile pyramid (Web Mercator).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<u16>,
    /// Metres, as read (the overview `read_resolution` picks; a tile pyramid's at its coverage's
    /// mid-latitude).
    pub resolution: f64,
    /// Where it has data: 1° cells merged into boxes (w, s, e, n); None: the world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<Vec<[f64; 4]>>,
}

/// 1° cells (west, south corners) merged into boxes: runs along each row, then rows of the same runs
/// stacked. Sorted, so the same cells give the same boxes.
pub fn cells_to_boxes(cells: &BTreeSet<(i32, i32)>) -> Vec<[f64; 4]> {
    // Runs per row (lat, then lon).
    let mut by_row: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for &(x, y) in cells {
        by_row.entry(y).or_default().push(x);
    }
    let mut runs: BTreeMap<(i32, i32), Vec<i32>> = BTreeMap::new();
    for (y, mut xs) in by_row {
        xs.sort_unstable();
        let mut i = 0;
        while i < xs.len() {
            let mut j = i;
            while j + 1 < xs.len() && xs[j + 1] == xs[j] + 1 {
                j += 1;
            }
            runs.entry((xs[i], xs[j])).or_default().push(y);
            i = j + 1;
        }
    }
    let mut out = Vec::new();
    for ((x0, x1), ys) in runs {
        let mut i = 0;
        while i < ys.len() {
            let mut j = i;
            while j + 1 < ys.len() && ys[j + 1] == ys[j] + 1 {
                j += 1;
            }
            out.push([x0 as f64, ys[i] as f64, (x1 + 1) as f64, (ys[j] + 1) as f64]);
            i = j + 1;
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

/// Where a dataset has data, as 1° cells (the world: every cell).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cover {
    pub world: bool,
    pub cells: HashSet<(i32, i32)>,
}

impl Cover {
    pub fn of(boxes: Option<&[[f64; 4]]>) -> Cover {
        match boxes {
            None => Cover { world: true, cells: HashSet::new() },
            Some(b) => {
                let mut cells = HashSet::new();
                for b in b {
                    for x in b[0].floor() as i32..b[2].ceil() as i32 {
                        for y in b[1].floor() as i32..b[3].ceil() as i32 {
                            cells.insert((x, y));
                        }
                    }
                }
                Cover { world: false, cells }
            }
        }
    }

    /// Whether point `lon`, `lat` (degrees) is in a cell of it (a point on a cell's edge in both
    /// cells beside it).
    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        if self.world {
            return true;
        }
        let (x, y) = (lon.floor() as i32, lat.floor() as i32);
        let edge_x = lon == lon.floor();
        let edge_y = lat == lat.floor();
        self.cells.contains(&(x, y)) || (edge_x && self.cells.contains(&(x - 1, y))) || (edge_y && self.cells.contains(&(x, y - 1))) || (edge_x && edge_y && self.cells.contains(&(x - 1, y - 1)))
    }

    /// Whether box w, s, e, n (degrees) meets it.
    pub fn meets(&self, b: [f64; 4]) -> bool {
        if self.world {
            return true;
        }
        let (x0, x1, y0, y1) = (b[0].floor() as i32 - 1, b[2].floor() as i32, b[1].floor() as i32 - 1, b[3].floor() as i32);
        if ((x1 - x0 + 1) as i64) * ((y1 - y0 + 1) as i64) > self.cells.len() as i64 {
            return self.cells.iter().any(|&(x, y)| (x as f64) <= b[2] && (x + 1) as f64 >= b[0] && (y as f64) <= b[3] && (y + 1) as f64 >= b[1]);
        }
        (x0..=x1).any(|x| (y0..=y1).any(|y| self.cells.contains(&(x, y)) && (x as f64) <= b[2] && (x + 1) as f64 >= b[0] && (y as f64) <= b[3] && (y + 1) as f64 >= b[1]))
    }
}

/// The 1° cells that native boxes (`boxes`, in projected CRS `crs`) meet: every cell sampled on a
/// 5 × 5 grid, its corners included, and projected; a cell any sample of which falls in a box,
/// with its neighbours (a box's corner may cut a cell between samples).
pub fn projected_cells(crs: u16, boxes: &[[f64; 4]]) -> Result<BTreeSet<(i32, i32)>> {
    projected_cells_where(crs, &|px, py| boxes.iter().any(|b| px >= b[0] && px <= b[2] && py >= b[1] && py <= b[3]))
}

/// `projected_cells`, a point (in CRS `crs`) held where `holds` says.
pub fn projected_cells_where(crs: u16, holds: &dyn Fn(f64, f64) -> bool) -> Result<BTreeSet<(i32, i32)>> {
    let atlas = match crs {
        3979 => super::proj::Atlas::new(),
        c => bail!("EPSG:{c} isn't a projected CRS the app knows"),
    };
    let mut hit = BTreeSet::new();
    for y in -89..89 {
        for x in -180..180 {
            let mut any = false;
            'cell: for j in 0..5 {
                for i in 0..5 {
                    let (px, py) = atlas.project(x as f64 + i as f64 * 0.25, y as f64 + j as f64 * 0.25);
                    if holds(px, py) {
                        any = true;
                        break 'cell;
                    }
                }
            }
            if any {
                hit.insert((x, y));
            }
        }
    }
    let mut out = hit.clone();
    for &(x, y) in &hit {
        for dx in -1..=1 {
            for dy in -1..=1 {
                if (-90..90).contains(&(y + dy)) {
                    out.insert(((x + dx + 180i32).rem_euclid(360) - 180, y + dy));
                }
            }
        }
    }
    Ok(out)
}

/// The 1° cells geographic boxes meet (a box's edges included).
pub fn geographic_cells(boxes: &[[f64; 4]]) -> BTreeSet<(i32, i32)> {
    let mut out = BTreeSet::new();
    for b in boxes {
        for x in b[0].floor() as i32..=b[2].floor() as i32 {
            for y in b[1].floor() as i32..=b[3].floor() as i32 {
                if (-180..180).contains(&x) && (-90..90).contains(&y) {
                    out.insert((x, y));
                }
            }
        }
    }
    out
}

// ---- Tile indexes -----------------------------------------------------------------------------

/// A tile index (GeoJSON, docs/inputs.md §6.8): a feature per file, its `url` (or `file`, in
/// `<dataset>/`) and its footprint (its geometry's box) in the index's CRS (`crs`, as GDAL writes
/// it: `urn:ogc:def:crs:EPSG::3979`; WGS 84 when there's none). A feature may name its file's fill
/// mask (`mask`, a URL: which pixels the source filled from another, the terrain's to read).
#[derive(Clone, Debug, PartialEq)]
pub struct TileIndex {
    pub crs: u16,
    /// (url or file, footprint box: min x, min y, max x, max y), in the index's order.
    pub files: Vec<(Where, [f64; 4])>,
    /// Each file's fill mask's URL, in the same order.
    pub masks: Vec<Option<String>>,
}

/// Where a file is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Where {
    Url(String),
    File(String),
}

/// An index parsed: it, or what's wrong with it.
pub fn parse_index(bytes: &[u8]) -> std::result::Result<TileIndex, String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("isn't JSON: {e}"))?;
    if v["type"] != "FeatureCollection" {
        return Err("isn't a GeoJSON FeatureCollection".into());
    }
    let crs = match v["crs"]["properties"]["name"].as_str() {
        None => 4326,
        Some(n) => {
            let code = n.rsplit(':').next().and_then(|c| c.parse::<u16>().ok()).ok_or_else(|| format!("its crs \"{n}\" isn't an EPSG code"))?;
            if code == 84 || n.ends_with("CRS84") {
                4326
            } else {
                code
            }
        }
    };
    let feats = v["features"].as_array().ok_or("has no features")?;
    let mut files = Vec::with_capacity(feats.len());
    let mut masks = Vec::with_capacity(feats.len());
    let mut seen = HashSet::new();
    for (i, f) in feats.iter().enumerate() {
        let p = &f["properties"];
        let w = match (p["url"].as_str(), p["file"].as_str()) {
            (Some(u), None) if u.starts_with("https://") || u.starts_with("http://") => Where::Url(u.to_string()),
            (None, Some(f)) if !f.is_empty() && !f.starts_with('/') && !f.split('/').any(|c| c == "..") => Where::File(f.to_string()),
            _ => return Err(format!("feature {} names no file: a `url` (http or https) or a `file` in the dataset's folder", i + 1)),
        };
        if !seen.insert(w.clone()) {
            return Err(format!("feature {} names a file an earlier feature names", i + 1));
        }
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let walk = |c: &serde_json::Value, depth: u8, b: &mut [f64; 4]| -> bool {
            fn go(c: &serde_json::Value, depth: u8, b: &mut [f64; 4]) -> bool {
                match c.as_array() {
                    Some(a) if depth == 0 => {
                        let (Some(x), Some(y)) = (a.first().and_then(|x| x.as_f64()), a.get(1).and_then(|y| y.as_f64())) else { return false };
                        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                        true
                    }
                    Some(a) => !a.is_empty() && a.iter().all(|e| go(e, depth - 1, b)),
                    None => false,
                }
            }
            go(c, depth, b)
        };
        let g = &f["geometry"];
        let ok = match g["type"].as_str() {
            Some("Polygon") => walk(&g["coordinates"], 2, &mut b),
            Some("MultiPolygon") => walk(&g["coordinates"], 3, &mut b),
            _ => false,
        };
        if !ok || !(b[0] < b[2] && b[1] < b[3]) {
            return Err(format!("feature {} hasn't a polygon footprint", i + 1));
        }
        masks.push(match &p["mask"] {
            serde_json::Value::Null => None,
            m => match m.as_str() {
                Some(u) if u.starts_with("https://") || u.starts_with("http://") => Some(u.to_string()),
                _ => return Err(format!("feature {}'s `mask` isn't an http(s) URL", i + 1)),
            },
        });
        files.push((w, b));
    }
    if files.is_empty() {
        return Err("has no features".into());
    }
    Ok(TileIndex { crs, files, masks })
}

// ---- The plan a unit samples from -----------------------------------------------------------

/// What `elev` reads (`elevation.json` in the unit's folder): the datasets in their order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub fmt: u32,
    /// The accepted version it was made from (the index's content name).
    pub version: String,
    /// The digest of every dataset's `keyed` (the DEM cache's samples are kept with it: unit.rs).
    pub tag: String,
    pub datasets: Vec<Dataset>,
}

/// One dataset as a unit samples it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dataset {
    pub id: String,
    pub code: u8,
    /// Metres, as read.
    pub resolution: f64,
    /// Where it has data (None: the world).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<Vec<[f64; 4]>>,
    pub source: Source,
}

/// How a dataset's data is read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub enum Source {
    Geotiff {
        crs: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read_resolution: Option<f64>,
        /// Kept in the store (`sources/fetched/<id>/`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        store: bool,
        /// The files near the unit, in the index's order: a URL (or a path for a dropped file) and
        /// its footprint in `crs` (None: the raster's own bounds).
        files: Vec<(String, Option<[f64; 4]>)>,
    },
    Tiles {
        url: String,
        zoom: u32,
        encoding: String,
    },
}

/// The order of datasets: rank (none: 0), resolution, ground over surface, id.
pub fn order_key(rank: Option<i64>, resolution: f64, surface: &str, id: &str) -> (i64, u64, u8, String) {
    (rank.unwrap_or(0), (resolution * 1000.0).round() as u64, (surface != "ground") as u8, id.to_string())
}

/// A declaration as the accepted index records it: its id, the declaration, what was measured, and
/// its `keyed` digest.
#[derive(Clone, Debug)]
pub struct Accepted {
    pub id: String,
    pub path: String,
    pub declaration: Declaration,
    pub measured: Measured,
    pub keyed: String,
}

/// The roads' declarations of an accepted version (`<id>.toml` entries with their facts, the
/// terrain's left out), in their order.
pub fn accepted(a: &crate::inputs::Accepted) -> Result<Vec<Accepted>> {
    Ok(accepted_all(a)?.into_iter().filter(|d| !d.declaration.terrain).collect())
}

/// The terrain's declarations of an accepted version (`terrain = true`), in their order.
pub fn accepted_terrain(a: &crate::inputs::Accepted) -> Result<Vec<Accepted>> {
    Ok(accepted_all(a)?.into_iter().filter(|d| d.declaration.terrain).collect())
}

/// Every declaration of an accepted version, in their order.
fn accepted_all(a: &crate::inputs::Accepted) -> Result<Vec<Accepted>> {
    let mut out = Vec::new();
    for (p, f) in &a.index.files {
        let Some(id) = p.strip_suffix(".toml").filter(|s| !s.contains('/')) else { continue };
        let declaration: Declaration = serde_json::from_value(f.facts.get("declaration").cloned().with_context(|| format!("{p}: no declaration in the index"))?).with_context(|| format!("{p}: its declaration"))?;
        let measured: Measured = serde_json::from_value(f.facts.get("measured").cloned().with_context(|| format!("{p}: nothing measured in the index"))?).with_context(|| format!("{p}: what was measured"))?;
        out.push(Accepted { id: id.to_string(), path: p.clone(), declaration, measured, keyed: f.keyed.clone() });
    }
    out.sort_by_key(|d| order_key(d.declaration.rank, d.measured.resolution, &d.declaration.surface, &d.id));
    Ok(out)
}

/// The digest of the datasets' `keyed` digests, in their order.
pub fn tag(ds: &[Accepted]) -> String {
    let lines: Vec<String> = ds.iter().map(|d| format!("{} {}", d.id, d.keyed)).collect();
    store::naming::hash16(lines.join("\n").as_bytes())
}

/// The datasets whose coverage meets box `b` (w, s, e, n, degrees), as a key names them: `id keyed`
/// each, in their order.
pub fn meeting(ds: &[(Accepted, Cover)], b: [f64; 4]) -> Vec<String> {
    ds.iter().filter(|(_, c)| c.meets(b)).map(|(d, _)| format!("{} {}", d.id, d.keyed)).collect()
}

/// A box (w, s, e, n, degrees) in projected CRS `crs`: its edges sampled and projected, grown by
/// `grow_m`.
fn native_box(crs: u16, b: [f64; 4], grow_m: f64) -> Result<[f64; 4]> {
    let atlas = match crs {
        3979 => super::proj::Atlas::new(),
        c => bail!("EPSG:{c} isn't a projected CRS the app knows"),
    };
    let mut nb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for i in 0..=16 {
        for j in 0..=16 {
            let (x, y) = atlas.project(b[0] + (b[2] - b[0]) * i as f64 / 16.0, b[1] + (b[3] - b[1]) * j as f64 / 16.0);
            nb = [nb[0].min(x), nb[1].min(y), nb[2].max(x), nb[3].max(y)];
        }
    }
    Ok([nb[0] - grow_m, nb[1] - grow_m, nb[2] + grow_m, nb[3] + grow_m])
}

/// The plan a unit samples from: the accepted datasets in their order, each with its files whose
/// footprints meet `near` (w, s, e, n, degrees: the unit's vertices' box, grown here by a few km,
/// past any blend band), the index read from its accepted copy (`a`). A dropped file is named by its
/// accepted copy's path.
pub fn plan(a: &crate::inputs::Accepted, near: [f64; 4]) -> Result<Plan> {
    let ds = accepted(a)?;
    let tagged = tag(&ds);
    let g = 0.05;
    let near = [near[0] - g, near[1] - g, near[2] + g, near[3] + g];
    let mut out = Vec::new();
    for d in &ds {
        let cover = Cover::of(d.measured.cover.as_deref());
        if !cover.meets(near) {
            continue;
        }
        let source = match &d.declaration.data {
            Data::Tiles { url, zoom, encoding } => Source::Tiles { url: url.clone(), zoom: *zoom, encoding: encoding.clone() },
            Data::Geotiff { urls, index, files, read_resolution, store } => {
                let crs = d.measured.crs.context("a GeoTIFF dataset without its CRS measured")?;
                let nb = if crs == 3979 { native_box(crs, near, 1000.0)? } else { near };
                let mut list: Vec<(String, Option<[f64; 4]>)> = Vec::new();
                for u in urls {
                    list.push((u.clone(), None));
                }
                if let Some(i) = index {
                    let ix = parse_index(&a.read(i)?).map_err(|e| anyhow::anyhow!("{i}: {e}"))?;
                    for (w, b) in ix.files {
                        if b[0] > nb[2] || b[2] < nb[0] || b[1] > nb[3] || b[3] < nb[1] {
                            continue;
                        }
                        list.push((locate(a, &d.id, &w)?, Some(b)));
                    }
                }
                if !files.is_empty() {
                    for (p, _) in a.files() {
                        if let Some(rest) = p.strip_prefix(&format!("{}/", d.id)) {
                            if files.iter().any(|g| glob(g, rest)) {
                                list.push((locate(a, &d.id, &Where::File(rest.to_string()))?, None));
                            }
                        }
                    }
                }
                Source::Geotiff { crs, read_resolution: *read_resolution, store: *store, files: list }
            }
        };
        out.push(Dataset { id: d.id.clone(), code: d.declaration.code, resolution: d.measured.resolution, cover: d.measured.cover.clone(), source });
    }
    Ok(Plan { fmt: 1, version: a.version.clone(), tag: tagged, datasets: out })
}

/// A file's URL, or a dropped file's accepted copy's path.
fn locate(a: &crate::inputs::Accepted, id: &str, w: &Where) -> Result<String> {
    match w {
        Where::Url(u) => Ok(u.clone()),
        Where::File(f) => Ok(a.path(&format!("{id}/{f}")).with_context(|| format!("{id}/{f} isn't in the accepted version"))?.to_string_lossy().into_owned()),
    }
}

/// Whether `name` matches glob `pat` (`*` any run of characters but `/`, `?` one).
pub fn glob(pat: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], n) || (!n.is_empty() && n[0] != b'/' && go(p, &n[1..])),
            (Some(b'?'), Some(c)) if *c != b'/' => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    go(pat.as_bytes(), name.as_bytes())
}

/// Reads a unit's plan (`elevation.json`), when there's one.
pub fn read_plan(dir: &std::path::Path) -> Result<Option<Plan>> {
    let p = dir.join(PLAN_FILE);
    match std::fs::read(&p) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    }
}

/// The plan's file in a unit's folder.
pub const PLAN_FILE: &str = "elevation.json";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_parse_and_refuse() {
        let ok = br#"
name = "x"
credit = "c"
licence = "CC0-1.0"
surface = "ground"
heights = "CGVD2013"
code = 3
[data]
format = "geotiff"
url = "https://example.org/a.tif"
"#;
        let d = parse(ok).unwrap();
        assert_eq!(d.code, 3);
        assert!(matches!(&d.data, Data::Geotiff { urls, .. } if urls == &["https://example.org/a.tif"]));
        let bad = br#"
surface = "dsm"
heights = "NGVD29"
rank = "high"
colour = 1
[data]
format = "tiles"
url = "https://example.org/{z}/{x}.png"
zoom = 15
encoding = "lerc"
"#;
        let why = parse(bad).unwrap_err().join("\n");
        for w in ["`surface`", "NGVD29", "`rank`", "`colour`", "`code`", "{y}", "lerc"] {
            assert!(why.contains(w), "{w}: {why}");
        }
        let mixed = String::from_utf8(ok.to_vec()).unwrap().replace("CGVD2013", "mixed");
        assert!(parse(mixed.as_bytes()).unwrap_err()[0].contains("terrain alone"));
        // The terrain's: "mixed" heights allowed, no code (the roads' source byte).
        let terrain = br#"
name = "t"
credit = "c"
licence = "CC0-1.0"
surface = "surface"
heights = "mixed"
terrain = true
[data]
format = "tiles"
url = "https://example.org/{z}/{x}/{y}.png"
zoom = 12
encoding = "terrarium"
"#;
        let d = parse(terrain).unwrap();
        assert!(d.terrain && d.code == 0 && d.heights == "mixed");
        let coded = String::from_utf8(terrain.to_vec()).unwrap().replace("terrain = true", "terrain = true\ncode = 3");
        assert!(parse(coded.as_bytes()).unwrap_err()[0].contains("source byte"));
        // A roads' declaration's `keyed` fields are what they were (no `terrain` among them).
        assert!(!serde_json::to_string(&parse(ok).unwrap()).unwrap().contains("terrain"));
    }

    #[test]
    fn an_index_names_each_files_fill_mask() {
        let ix = br#"{"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {"url": "https://e.org/a.tif", "mask": "https://e.org/a_m.tif"}, "geometry": {"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 0]]]}},
            {"type": "Feature", "properties": {"url": "https://e.org/b.tif"}, "geometry": {"type": "Polygon", "coordinates": [[[1, 0], [2, 0], [2, 1], [1, 0]]]}}]}"#;
        let t = parse_index(ix).unwrap();
        assert_eq!(t.masks, [Some("https://e.org/a_m.tif".to_string()), None]);
        let bad = String::from_utf8(ix.to_vec()).unwrap().replace("https://e.org/a_m.tif", "a_m.tif");
        assert!(parse_index(bad.as_bytes()).unwrap_err().contains("`mask`"));
    }

    #[test]
    fn cells_merge_into_boxes_and_cover_back() {
        let cells: BTreeSet<(i32, i32)> = [(0, 0), (1, 0), (0, 1), (1, 1), (5, 0)].into();
        let b = cells_to_boxes(&cells);
        assert_eq!(b, vec![[0.0, 0.0, 2.0, 2.0], [5.0, 0.0, 6.0, 1.0]]);
        let c = Cover::of(Some(&b));
        assert!(c.contains(0.5, 1.5) && c.contains(5.0, 0.5) && !c.contains(3.5, 0.5));
        // A point on an edge: in both cells beside it.
        assert!(c.contains(2.0, 0.5) && c.contains(6.0, 0.5));
        assert!(c.meets([1.5, 1.5, 3.0, 3.0]) && !c.meets([3.0, 3.0, 4.0, 4.0]));
        assert!(Cover::of(None).meets([100.0, 10.0, 101.0, 11.0]));
    }

    #[test]
    fn indexes_parse() {
        let ix = br#"{"type":"FeatureCollection","crs":{"type":"name","properties":{"name":"urn:ogc:def:crs:EPSG::3979"}},
            "features":[{"type":"Feature","properties":{"url":"https://e.org/a.tif"},"geometry":{"type":"Polygon","coordinates":[[[0,0],[10,0],[10,5],[0,5],[0,0]]]}}]}"#;
        let t = parse_index(ix).unwrap();
        assert_eq!((t.crs, t.files[0].1), (3979, [0.0, 0.0, 10.0, 5.0]));
        assert!(parse_index(br#"{"type":"FeatureCollection","features":[{"properties":{},"geometry":null}]}"#).unwrap_err().contains("names no file"));
        assert!(glob("*.tif", "a.tif") && !glob("*.tif", "x/a.tif") && glob("x/?.tif", "x/a.tif"));
    }
}
