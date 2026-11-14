//! The lakes' estimated depth (docs/steps.md §3, Lake depth; docs/inputs.md §6.10): the
//! `lake-depth` step, a z6 tile of the coverage a job, before the depth that reads it. Its sources
//! are the depth declarations of format `lake-estimate` (`FORMAT`), each made here from its inputs
//! for every z6 tile the coverage reaches, so a region added is estimated as the pool builds it.
//!
//! - **GLOBathy** (`Method::Globathy`): every HydroLAKES v1.0 lake meeting the tile with GLOBathy's
//!   maximum depth (`Dmax_use_m`), both read from the source's folder (`sources/fetched/<id>/`: the
//!   basic parameters' zip, its `(ALL_LAKES).csv`, and HydroLAKES' polygons' zip), a bowl on its
//!   outline (`bowl`) at z11, a grid a z8 tile.
//! - **The small lakes** (`Method::LandSlope`): every lake or pond of the basemap's z14 water
//!   (merged by OSM id) under 10 ha, not intermittent, not within 3 m of the sea and not a fifth or
//!   more under another source's HydroLAKES polygons (`outlines`: GLOBathy's), its maximum depth
//!   from the land's mean slope 30–100 m from its shore (the terrain's z12 tiles, the hi packs) and
//!   its largest distance to shore (`dmax_estimate`, the depth lab's fit), a bowl at z14, a grid a
//!   z10 tile.
//!
//! Each source's grids of a z6 tile are one pack (`pack`: a tar of their GeoTIFFs, its index the
//! first member), `logical(id, tile)` in the manifest, none where it has no lake. The depth reads
//! them as that source's grids (crate::depth::grids::DepthSource::estimated).

use anyhow::{bail, ensure, Context, Result};
use det::Det;
use std::collections::{BTreeMap, HashMap};

// The declarations' format and a source's method with its grids' zooms: inputs::inputs::estimate's
// (task #916 step 6d: the input check reads them), kept at these paths.
pub use inputs::inputs::estimate::{Method, FORMAT, GLOBATHY_Z, SMALL_Z};

/// Bumped with any change to the grids made from the same inputs.
pub const V: u32 = 1;
/// The packs' logical names' prefix: `work/lake-depth/<source id>/6-x-y`.
pub const PREFIX: &str = "work/lake-depth/";

/// The terrain the small lakes' slope is read from.
pub const TERRAIN_Z: u8 = 12;

/// The bowl's shape, D = Dmax · (l / L)^P (the depth lab's fit on 436 surveyed lakes, nRMSE 0.21;
/// GLOBathy's own rasters are p = 1).
pub const P: f64 = 0.75;
/// The distance to shore smoothed by a Gaussian of L · `SIGMA` (land counting as 0).
pub const SIGMA: f64 = 1.0 / 8.0;
/// A bowl wider than this many pixels is shaped at the coarsest zoom that holds it, then scaled up
/// (bilinearly, inside the outline at the grid's zoom): the Caspian at z11 would be 25,000 pixels.
pub const BOWL_SIDE: usize = 1024;

/// The small lakes' fit (the depth lab's params_depth.json, model kSLq on S30_100): ln Dmax = a +
/// c ln S + b ln L + d (ln L)², ln L held at its peak beyond it.
pub const BETA: (f64, f64, f64, f64) =
    (-3.820468659540713, 0.36259624867415013, 1.983300273194609, -0.13239733333469395);
/// The area rule where the land is flat or missing: Dmax = e^a · A^b (A in ha).
pub const AREA_BETA: (f64, f64) = (1.1112663099708497, 0.22753622975506627);
/// Under this mean slope the land is flat: the area rule.
pub const S_MIN: f64 = 0.003;
/// The ring the slope is read in (ground metres from the shore).
pub const RING: (f64, f64) = (30.0, 100.0);
/// The small lakes: under this many hectares.
pub const MAX_HA: f64 = 10.0;
/// A lake this share of whose area HydroLAKES covers is GLOBathy's.
pub const HYDROLAKES_SHARE: f64 = 0.2;
/// A lake within this many ground metres of the sea is the sea's.
pub const SEA_M: f64 = 3.0;

/// Pack `id`'s logical name for z6 tile `t`.
pub fn logical(id: &str, t: (u32, u32)) -> String {
    format!("{PREFIX}{id}/6-{}-{}", t.0, t.1)
}

/// A pack's source id and z6 tile, from its logical name.
pub fn tile_of(l: &str) -> Option<(&str, (u32, u32))> {
    let (id, t) = l.strip_prefix(PREFIX)?.split_once('/')?;
    let u = codecs::legacy::TileId::parse(t).filter(|u| u.z == 6)?;
    // (As `logical` writes it, and no other spelling of the tile.)
    store::naming::prints_as(t, format_args!("6-{}-{}", u.x, u.y)).then_some((id, (u.x, u.y)))
}

/// Z6 tile `t` and the 8 around it (columns across the date line, no rows past the poles).
pub fn ring(t: (u32, u32)) -> Vec<(u32, u32)> {
    around(t.0, t.1, 1, 1)
}

/// The z6 tiles from (x, y) spanning `n` a side and `m` more on every side (columns wrapping).
fn around(x: u32, y: u32, n: u32, m: u32) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for dy in -(m as i64)..(n + m) as i64 {
        let yy = i64::from(y) + dy;
        if !(0..64).contains(&yy) {
            continue;
        }
        for dx in -(m as i64)..(n + m) as i64 {
            v.push(((i64::from(x) + dx).rem_euclid(64) as u32, yy as u32));
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The z6 tiles whose packs a depth target of tile (z, x, y) reads: a z6 tile's (`depth`: its
/// coarse levels over it and its ring, `ring`), a z3 tile's (`depth-lo`: its z6 tiles and the ring
/// of them its blocks' margin reaches).
pub fn read_by(z: u8, x: u32, y: u32) -> Vec<(u32, u32)> {
    match z {
        6 => ring((x, y)),
        3 => around(x * 8, y * 8, 8, 1),
        _ => Vec::new(),
    }
}

/// The packs manifest `m` names, by source id and z6 tile, with their content names.
pub fn named(m: &BTreeMap<String, String>) -> impl Iterator<Item = (&str, (u32, u32), &str)> {
    m.range(PREFIX.to_string()..)
        .take_while(|(l, _)| l.starts_with(PREFIX))
        .filter_map(|(l, c)| tile_of(l).map(|(id, t)| (id, t, c.as_str())))
}

// ---- the estimates -------------------------------------------------------------------------------

/// The small lakes' maximum depth (m) from the land's mean slope `s` (m/m) and the largest distance
/// to shore `l` (m), or by the area rule from `area_ha` where the slope is missing or under
/// `S_MIN`; and whether the slope gave it.
pub fn dmax_estimate(s: Option<f64>, l: f64, area_ha: f64) -> (f64, bool) {
    if let Some(s) = s.filter(|s| s.is_finite() && *s >= S_MIN) {
        let (a, c, b, d) = BETA;
        let mut ln_l = l.max(1.0).dln();
        if d < 0.0 {
            ln_l = ln_l.min(-b / (2.0 * d));
        }
        return ((a + c * s.dln() + b * ln_l + d * ln_l * ln_l).dexp(), true);
    }
    let (a, b) = AREA_BETA;
    ((a + b * area_ha.max(1e-3).dln()).dexp(), false)
}

/// A polygon's rings (outer first, holes after; either winding) in a zoom's global pixels.
pub type Rings = Vec<Vec<[f64; 2]>>;

/// Which of the `w` × `h` pixels from (`c0`, `r0`) have their centre inside `rings` (even–odd).
pub fn fill(rings: &[Vec<[f64; 2]>], c0: i64, r0: i64, w: usize, h: usize) -> Vec<bool> {
    let mut out = vec![false; w * h];
    let mut xs: Vec<f64> = Vec::new();
    for j in 0..h {
        let yc = (r0 + j as i64) as f64 + 0.5;
        xs.clear();
        for r in rings {
            for k in 0..r.len() {
                let (a, b) = (r[k], r[(k + 1) % r.len()]);
                if (a[1] <= yc) != (b[1] <= yc) {
                    xs.push(a[0] + (yc - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        for p in xs.chunks_exact(2) {
            // (Pixels whose centre c0 + i + 0.5 lies in [p0, p1).)
            let i0 = ((p[0] - 0.5 - c0 as f64).ceil().max(0.0)) as usize;
            let i1 = ((p[1] - 0.5 - c0 as f64).ceil().max(0.0) as usize).min(w);
            for v in &mut out[j * w + i0.min(w)..j * w + i1.max(i0.min(w))] {
                *v = true;
            }
        }
    }
    out
}

/// The pixels `fill` gives and every pixel a ring's edge passes through (rasterio's `all_touched`).
fn fill_touched(rings: &[Vec<[f64; 2]>], c0: i64, r0: i64, w: usize, h: usize) -> Vec<bool> {
    let mut out = fill(rings, c0, r0, w, h);
    let mut mark = |x: f64, y: f64| {
        let (i, j) = ((x.floor() as i64) - c0, (y.floor() as i64) - r0);
        if (0..w as i64).contains(&i) && (0..h as i64).contains(&j) {
            out[j as usize * w + i as usize] = true;
        }
    };
    for r in rings {
        for k in 0..r.len() {
            let (a, b) = (r[k], r[(k + 1) % r.len()]);
            // (Sampled at a quarter pixel: every pixel the edge crosses gets a sample.)
            let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) * 4.0).ceil().max(1.0) as usize;
            for s in 0..=n {
                let t = s as f64 / n as f64;
                mark(a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]));
            }
        }
    }
    out
}

/// Ground metres a pixel of zoom `z` at global row `r`.
fn ground_px(z: u8, r: f64) -> f64 {
    let (_, lat) = super::grids::lonlat(z, 0.0, r);
    super::grids::px(z) * lat.to_radians().dcos()
}

/// `v` blurred by a Gaussian of `sigma` pixels (scipy's `gaussian_filter`, `mode="constant"`:
/// nothing past the edges, its kernel 4 σ each side).
fn gaussian(v: &[f64], w: usize, h: usize, sigma: f64) -> Vec<f64> {
    let r = (4.0 * sigma + 0.5) as i64;
    let k: Vec<f64> = (-r..=r).map(|i| (-0.5 * (i as f64 / sigma).powi(2)).dexp()).collect();
    let sum: f64 = k.iter().sum();
    let k: Vec<f64> = k.iter().map(|x| x / sum).collect();
    let pass = |src: &[f64], n: usize, m: usize, at: &dyn Fn(usize, usize) -> usize| -> Vec<f64> {
        // (`m` lines of `n` cells each, a cell's index `at(line, cell)`.)
        let mut out = vec![0f64; src.len()];
        for line in 0..m {
            for c in 0..n {
                let mut s = 0.0;
                for (q, kk) in k.iter().enumerate() {
                    let x = c as i64 + q as i64 - r;
                    if (0..n as i64).contains(&x) {
                        s += kk * src[at(line, x as usize)];
                    }
                }
                out[at(line, c)] = s;
            }
        }
        out
    };
    let rows = pass(v, w, h, &|l, c| l * w + c);
    pass(&rows, h, w, &|l, c| c * w + l)
}

/// A lake's bowl: on zoom `zoom`'s pixels `c0..c0 + w`, `r0..r0 + h`, its smoothed distance to shore
/// over its largest (`f`, 0 to 1; 0 off the lake), so the depth is −Dmax · f^P.
#[derive(Clone, Debug)]
pub struct Bowl {
    pub zoom: u8,
    pub c0: i64,
    pub r0: i64,
    pub w: usize,
    pub h: usize,
    pub f: Vec<f32>,
}

impl Bowl {
    /// The bowl of the lake `rings` (zoom `z`'s pixels): l the distance to the nearest shore
    /// (islands are shore), smoothed by a Gaussian of L · `SIGMA` with the land as 0 (so the medial
    /// axis's crease doesn't show), over L its largest; at `z`, or for a lake wider than
    /// `BOWL_SIDE` pixels the coarsest zoom holding it within them. None for a lake under every
    /// pixel's centre.
    pub fn of(rings: &[Vec<[f64; 2]>], z: u8) -> Option<Bowl> {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in rings.iter().flatten() {
            (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
        }
        if x0 > x1 {
            return None;
        }
        let mut k = 0u8;
        while k < z && ((x1 - x0).max(y1 - y0) / f64::from(1u32 << k)) as usize > BOWL_SIDE {
            k += 1;
        }
        let (zoom, s) = (z - k, 1.0 / f64::from(1u32 << k));
        let n = 256i64 << zoom;
        let c0 = ((x0 * s).floor() as i64 - 2).max(0);
        let r0 = ((y0 * s).floor() as i64 - 2).max(0);
        let (c1, r1) = (((x1 * s).ceil() as i64 + 2).min(n), ((y1 * s).ceil() as i64 + 2).min(n));
        let (w, h) = ((c1 - c0).max(0) as usize, (r1 - r0).max(0) as usize);
        let scaled: Rings = rings.iter().map(|r| r.iter().map(|p| [p[0] * s, p[1] * s]).collect()).collect();
        let inside = fill(&scaled, c0, r0, w, h);
        if !inside.iter().any(|&v| v) {
            return None;
        }
        let m = ground_px(zoom, (r0 as f64) + h as f64 / 2.0);
        let outside: Vec<bool> = inside.iter().map(|v| !v).collect();
        let (d, _) = super::edt::nearest(&outside, w, h);
        let mut l: Vec<f64> =
            (0..w * h).map(|i| if inside[i] { (f64::from(d[i]) - 0.5).max(0.25) * m } else { 0.0 }).collect();
        let big = l.iter().cloned().fold(0.0, f64::max);
        let sig = big * SIGMA / m;
        if sig > 0.05 {
            l = gaussian(&l, w, h, sig);
        }
        let top = (0..w * h).filter(|&i| inside[i]).map(|i| l[i]).fold(0.0, f64::max);
        if top <= 0.0 {
            return None;
        }
        let f = (0..w * h).map(|i| if inside[i] { (l[i] / top).clamp(0.0, 1.0) as f32 } else { 0.0 }).collect();
        Some(Bowl { zoom, c0, r0, w, h, f })
    }

    /// Its `f` at the centre of zoom-`z` pixel (`c`, `r`) (z its zoom or finer: bilinear between
    /// its own pixels' centres, the land 0).
    fn at(&self, z: u8, c: i64, r: i64) -> f64 {
        let s = 1.0 / f64::from(1u32 << (z - self.zoom));
        let (x, y) = ((c as f64 + 0.5) * s - 0.5 - self.c0 as f64, (r as f64 + 0.5) * s - 0.5 - self.r0 as f64);
        let (xf, yf) = (x.floor(), y.floor());
        let (tx, ty) = (x - xf, y - yf);
        let get = |i: i64, j: i64| -> f64 {
            if (0..self.w as i64).contains(&i) && (0..self.h as i64).contains(&j) {
                f64::from(self.f[j as usize * self.w + i as usize])
            } else {
                0.0
            }
        };
        let (i, j) = (xf as i64, yf as i64);
        get(i, j) * (1.0 - tx) * (1.0 - ty)
            + get(i + 1, j) * tx * (1.0 - ty)
            + get(i, j + 1) * (1.0 - tx) * ty
            + get(i + 1, j + 1) * tx * ty
    }
}

/// A grid being painted: zoom `zoom`'s pixels `c0..c0 + w`, `r0..r0 + h`, NaN where no lake is.
pub struct Canvas {
    pub zoom: u8,
    pub c0: i64,
    pub r0: i64,
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl Canvas {
    /// Tile (`x`, `y`) of zoom `pz` at zoom `zoom`, empty.
    pub fn tile(zoom: u8, pz: u8, x: u32, y: u32) -> Canvas {
        let span = 256usize << (zoom - pz);
        Canvas {
            zoom,
            c0: i64::from(x) * span as i64,
            r0: i64::from(y) * span as i64,
            w: span,
            h: span,
            v: vec![f32::NAN; span * span],
        }
    }

    /// Lake `rings` (its zoom's pixels) painted to maximum depth `dmax` from its bowl `b`: each
    /// pixel inside it (its centre), −dmax · f^P.
    pub fn paint(&mut self, rings: &[Vec<[f64; 2]>], b: &Bowl, dmax: f64) {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in rings.iter().flatten() {
            (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
        }
        let c0 = (x0.floor() as i64 - 1).max(self.c0);
        let r0 = (y0.floor() as i64 - 1).max(self.r0);
        let c1 = (x1.ceil() as i64 + 1).min(self.c0 + self.w as i64);
        let r1 = (y1.ceil() as i64 + 1).min(self.r0 + self.h as i64);
        if c0 >= c1 || r0 >= r1 {
            return;
        }
        let (w, h) = ((c1 - c0) as usize, (r1 - r0) as usize);
        let inside = fill(rings, c0, r0, w, h);
        for j in 0..h {
            for i in 0..w {
                if inside[j * w + i] {
                    let (c, r) = (c0 + i as i64, r0 + j as i64);
                    let f = b.at(self.zoom, c, r);
                    let o = (r - self.r0) as usize * self.w + (c - self.c0) as usize;
                    self.v[o] = (-dmax * f.clamp(0.0, 1.0).dpowf(P)) as f32;
                }
            }
        }
    }

    /// Cropped to its data, its edges on its zoom's tile edges (so its overviews' cells line up
    /// with the coarser zooms' pixels); None with none.
    pub fn cropped(self) -> Option<Canvas> {
        let (mut i0, mut j0, mut i1, mut j1) = (usize::MAX, usize::MAX, 0, 0);
        for j in 0..self.h {
            for i in 0..self.w {
                if self.v[j * self.w + i].is_finite() {
                    (i0, j0, i1, j1) = (i0.min(i), j0.min(j), i1.max(i + 1), j1.max(j + 1));
                }
            }
        }
        if i0 == usize::MAX {
            return None;
        }
        let snap = |v: usize, o: i64| -> usize { ((v as i64 + o).div_euclid(256) * 256 - o).max(0) as usize };
        let up = |v: usize, o: i64, n: usize| -> usize {
            ((v as i64 + o + 255).div_euclid(256) * 256 - o).min(n as i64) as usize
        };
        let (a0, b0) = (snap(i0, self.c0), snap(j0, self.r0));
        let (a1, b1) = (up(i1, self.c0, self.w), up(j1, self.r0, self.h));
        let (w, h) = (a1 - a0, b1 - b0);
        let mut v = Vec::with_capacity(w * h);
        for j in b0..b1 {
            v.extend_from_slice(&self.v[j * self.w + a0..j * self.w + a1]);
        }
        Some(Canvas { zoom: self.zoom, c0: self.c0 + a0 as i64, r0: self.r0 + b0 as i64, w, h, v })
    }
}

/// Lon/lat ring points as zoom `z`'s global pixels.
pub fn to_pixels(rings: &[Vec<[f64; 2]>], z: u8) -> Rings {
    let p = super::grids::px(z);
    rings
        .iter()
        .map(|r| {
            r.iter()
                .map(|q| {
                    let lat = q[1].clamp(-roadcore::geo::MERCATOR_LAT, roadcore::geo::MERCATOR_LAT);
                    let [x, y] = roadcore::geo::epsg3857(q[0], lat);
                    [(x + super::grids::HALF) / p, (super::grids::HALF - y) / p]
                })
                .collect()
        })
        .collect()
}

// ---- GLOBathy's inputs ---------------------------------------------------------------------------

/// The fields of a CSV line (quoted fields with commas and doubled quotes as RFC 4180).
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut it = line.chars().peekable();
    while let Some(ch) = it.next() {
        match (ch, quoted) {
            ('"', true) if it.peek() == Some(&'"') => {
                cur.push('"');
                it.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

/// GLOBathy's `Dmax_use_m` (m, > 0) by `Hylak_id`, of lakes `ids`, from its basic parameters' CSV
/// (`r`).
pub fn read_dmax(r: impl std::io::Read, ids: &std::collections::HashSet<u64>) -> Result<HashMap<u64, f64>> {
    use std::io::BufRead;
    let mut lines = std::io::BufReader::new(r).lines();
    let head = csv_fields(lines.next().context("GLOBathy's CSV is empty")??.trim_start_matches('\u{feff}'));
    let col = |n: &str| head.iter().position(|h| h.trim() == n).with_context(|| format!("GLOBathy's CSV has no {n}"));
    let (id, d) = (col("Hylak_id")?, col("Dmax_use_m")?);
    let mut out = HashMap::new();
    for line in lines {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let f = csv_fields(&line);
        let num = |k: usize| f.get(k).and_then(|v| v.trim().parse::<f64>().ok());
        if let (Some(h), Some(dm)) = (num(id), num(d)) {
            if ids.contains(&(h as u64)) && dm.is_finite() && dm > 0.0 {
                out.insert(h as u64, dm);
            }
        }
    }
    Ok(out)
}

/// HydroLAKES' polygons meeting lon/lat box `b`, from its shapefile (`shp`, `dbf`): (Hylak_id, its
/// rings in lon/lat).
pub fn read_hydrolakes(shp: impl std::io::Read, dbf: impl std::io::Read, b: [f64; 4]) -> Result<Vec<(u64, Rings)>> {
    use inputs::lakefill::shp::{DbfReader, ShpReader};
    let mut s = ShpReader::new(shp)?;
    let mut d = DbfReader::new(dbf)?;
    let col = d.col("Hylak_id").context("HydroLAKES' table has no Hylak_id")?;
    let mut out = Vec::new();
    while let Some(poly) = s.next_polygon_meeting(b)? {
        let row = d.next_row()?.context("HydroLAKES' table ends before its shapes")?;
        let Some(rings) = poly else { continue };
        if rings.is_empty() {
            continue;
        }
        let id =
            row.get(col).and_then(|v| v.parse::<f64>().ok()).context("a HydroLAKES polygon without its Hylak_id")?;
        out.push((id as u64, rings));
    }
    Ok(out)
}

/// The inputs a source's folder holds (`sources/fetched/<id>/`): GLOBathy's basic parameters' zip
/// (the one holding `(ALL_LAKES).csv`) and HydroLAKES v1.0's polygons' zip.
pub struct Folder {
    pub dir: std::path::PathBuf,
}

impl Folder {
    fn zips(&self) -> Result<Vec<std::path::PathBuf>> {
        let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(&self.dir)
            .with_context(|| format!("{}", self.dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")))
            .collect();
        v.sort();
        Ok(v)
    }

    /// GLOBathy's maximum depths of lakes `ids` (`read_dmax`).
    pub fn dmax(&self, ids: &std::collections::HashSet<u64>) -> Result<HashMap<u64, f64>> {
        for z in self.zips()? {
            let ms = inputs::lakefill::zip::members(&z)?;
            if let Some(m) = ms.iter().find(|m| m.name.ends_with("(ALL_LAKES).csv")) {
                return read_dmax(inputs::lakefill::zip::open(&z, m)?, ids)
                    .with_context(|| format!("{}:{}", z.display(), m.name));
            }
        }
        bail!("{}: no zip holds GLOBathy's (ALL_LAKES).csv", self.dir.display())
    }

    /// HydroLAKES' polygons meeting box `b` (`read_hydrolakes`).
    pub fn hydrolakes(&self, b: [f64; 4]) -> Result<Vec<(u64, Rings)>> {
        for z in self.zips()? {
            if let Some((shp, dbf, _)) =
                inputs::lakefill::run::shapefiles(&z, "HydroLAKES_polys_v10")?.into_iter().next()
            {
                return read_hydrolakes(shp.open()?, dbf.open()?, b).with_context(|| shp.name());
            }
        }
        bail!("{}: no zip holds HydroLAKES_polys_v10.shp", self.dir.display())
    }
}

/// GLOBathy's grids of z6 tile `t`: each HydroLAKES lake `lakes` with a maximum depth in `dmax`
/// painted as its bowl into the z8 tiles of `t` it meets (a grid each, cropped to its data), each
/// given to `sink` as it's made.
pub fn globathy_grids(
    lakes: &[(u64, Rings)],
    dmax: &HashMap<u64, f64>,
    t: (u32, u32),
    sink: &mut dyn FnMut(Canvas) -> Result<()>,
) -> Result<()> {
    let (z, pz) = Method::Globathy.zooms();
    let px: Vec<(f64, Rings, [f64; 4])> = lakes
        .iter()
        .filter_map(|(id, r)| {
            let d = *dmax.get(id)?;
            let r = to_pixels(r, z);
            let b = r.iter().flatten().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
            });
            Some((d, r, b))
        })
        .collect();
    let mut bowls: HashMap<usize, Option<Bowl>> = HashMap::new();
    let k = pz - 6;
    for ty in t.1 << k..(t.1 + 1) << k {
        for tx in t.0 << k..(t.0 + 1) << k {
            let mut c = Canvas::tile(z, pz, tx, ty);
            let (c0, r0, c1, r1) = (c.c0 as f64, c.r0 as f64, (c.c0 + c.w as i64) as f64, (c.r0 + c.h as i64) as f64);
            for (i, (d, rings, meets)) in px.iter().enumerate() {
                if meets[2] < c0 || meets[0] > c1 || meets[3] < r0 || meets[1] > r1 {
                    continue;
                }
                if let Some(b) = bowls.entry(i).or_insert_with(|| Bowl::of(rings, z)) {
                    c.paint(rings, b, *d);
                }
            }
            if let Some(c) = c.cropped() {
                sink(c)?;
            }
        }
    }
    Ok(())
}

// ---- the small lakes -----------------------------------------------------------------------------

/// The terrain's z12 tiles (metres; None where nothing is built), as the small lakes' slope reads
/// them.
pub trait Terrain {
    fn tile(&self, x: u32, y: u32) -> Result<Option<std::sync::Arc<Vec<f32>>>>;
}

/// The terrain z12 tiles of the hi packs (`layers/terrain/hi/6-x-y`, z9–12) under the project root.
pub struct HiPacks {
    root: store::nasfs::NasRoot,
    packs: BTreeMap<(u32, u32), String>,
    /// Each pack open (store::range::PlainFile, kept for the job: a read meeting its handle gone
    /// opens it again, task #1090) and its index.
    open: std::sync::Mutex<
        HashMap<(u32, u32), Option<std::sync::Arc<(store::range::PlainFile, store::pack::PackIndex)>>>,
    >,
    cache: std::sync::Mutex<HashMap<(u32, u32), Option<std::sync::Arc<Vec<f32>>>>>,
}

impl HiPacks {
    /// The hi packs manifest `m` names.
    pub fn new(root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) -> HiPacks {
        let p = "layers/terrain/hi/6-";
        let packs = m
            .range(p.to_string()..)
            .take_while(|(l, _)| l.starts_with(p))
            .filter_map(|(l, c)| {
                let u = codecs::legacy::TileId::parse(&l["layers/terrain/hi/".len()..].replace('-', "/"))?;
                Some(((u.x, u.y), c.clone()))
            })
            .collect();
        HiPacks { root: root.clone(), packs, open: Default::default(), cache: Default::default() }
    }

    fn pack(&self, t: (u32, u32)) -> Result<Option<std::sync::Arc<(store::range::PlainFile, store::pack::PackIndex)>>> {
        if let Some(p) = self.open.lock().unwrap().get(&t) {
            return Ok(p.clone());
        }
        let p = match self.packs.get(&t) {
            None => None,
            Some(c) => {
                let path = self.root.join(c);
                let f = path.open_plain().with_context(|| format!("{}", path.display()))?;
                let idx = store::pack::PackIndex::read_from(&f)
                    .with_context(|| format!("the index of {}", path.display()))?;
                Some(std::sync::Arc::new((f, idx)))
            }
        };
        self.open.lock().unwrap().insert(t, p.clone());
        Ok(p)
    }
}

impl Terrain for HiPacks {
    fn tile(&self, x: u32, y: u32) -> Result<Option<std::sync::Arc<Vec<f32>>>> {
        if let Some(t) = self.cache.lock().unwrap().get(&(x, y)) {
            return Ok(t.clone());
        }
        let k = TERRAIN_Z - 6;
        let t = match self.pack((x >> k, y >> k))? {
            None => None,
            Some(p) => match p.1.get(&p.0, TERRAIN_Z, x, y)? {
                None => None,
                Some((_, b)) => Some(std::sync::Arc::new(
                    roadcore::grid::decode_terrain(&b).with_context(|| format!("terrain {TERRAIN_Z}/{x}/{y}"))?,
                )),
            },
        };
        let mut c = self.cache.lock().unwrap();
        // (A few hundred kept: 256 KB each.)
        if c.len() >= 512 {
            c.clear();
        }
        c.insert((x, y), t.clone());
        Ok(t)
    }
}

/// A ring's signed area (the shoelace sum).
fn signed_area(r: &[[f64; 2]]) -> f64 {
    (0..r.len()).map(|k| r[k][0] * r[(k + 1) % r.len()][1] - r[(k + 1) % r.len()][0] * r[k][1]).sum::<f64>() / 2.0
}

/// A geos polygon (or multipolygon) from rings as a vector tile or a shapefile holds them: each ring
/// wound as the first an outer one, the rings wound the other way after it its holes.
fn polygon(rings: &[Vec<[f64; 2]>]) -> Result<geos::Geometry> {
    use geos::{CoordSeq, Geom, Geometry};
    let ring = |r: &[[f64; 2]]| -> Result<Geometry> {
        let mut pts: Vec<[f64; 2]> = r.to_vec();
        if pts.first() != pts.last() {
            pts.push(pts[0]);
        }
        Ok(Geometry::create_linear_ring(CoordSeq::new_from_vec(&pts)?)?)
    };
    let rs: Vec<&Vec<[f64; 2]>> = rings.iter().filter(|r| r.len() >= 3).collect();
    ensure!(!rs.is_empty(), "no ring");
    let outer = signed_area(rs[0]) >= 0.0;
    let mut parts: Vec<(Geometry, Vec<Geometry>)> = Vec::new();
    for r in rs {
        match parts.last_mut() {
            Some((_, holes)) if (signed_area(r) >= 0.0) != outer => holes.push(ring(r)?),
            _ => parts.push((ring(r)?, Vec::new())),
        }
    }
    let polys = parts.into_iter().map(|(o, h)| Geometry::create_polygon(o, h)).collect::<Result<Vec<_>, _>>()?;
    let g = if polys.len() == 1 { polys.into_iter().next().unwrap() } else { Geometry::create_multipolygon(polys)? };
    Ok(if g.is_valid()? { g } else { g.make_valid()? })
}

/// A geometry's polygons' rings.
fn rings_of(g: &geos::Geometry) -> Result<Vec<Rings>> {
    use geos::{Geom, GeometryTypes};
    let mut out = Vec::new();
    let coords = |r: &geos::ConstGeometry| -> Result<Vec<[f64; 2]>> {
        let s = r.get_coord_seq()?;
        (0..s.size()?).map(|i| Ok([s.get_x(i)?, s.get_y(i)?])).collect()
    };
    for k in 0..g.get_num_geometries()? {
        let p = g.get_geometry_n(k)?;
        match p.geometry_type()? {
            GeometryTypes::Polygon if !p.is_empty()? => {
                let mut rs = vec![coords(&p.get_exterior_ring()?)?];
                for i in 0..p.get_num_interior_rings()? {
                    rs.push(coords(&p.get_interior_ring_n(i)?)?);
                }
                out.push(rs);
            }
            GeometryTypes::MultiPolygon | GeometryTypes::GeometryCollection => {
                out.extend(rings_of(&geos::Geom::clone(&p)?)?);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// A small lake found: its polygons (zoom-`SMALL_Z` pixels) and its estimated maximum depth.
pub struct Small {
    pub parts: Vec<Rings>,
    pub dmax: f64,
}

/// What the small lakes' search counted (the run's log).
pub type Counts = BTreeMap<&'static str, u64>;

/// The land's mean slope (m/m) 30–100 m (ground) from lake `g` (zoom-`SMALL_Z` pixels) on the
/// terrain's z12 tiles, the other lakes `others` and terrain at or below 0.3 m (the sea) left out;
/// None under 3 land pixels. `gm`: ground metres a z12 pixel there.
fn land_slope(g: &geos::Geometry, others: &[&geos::Geometry], terrain: &dyn Terrain, gm: f64) -> Result<Option<f64>> {
    use geos::Geom;
    let k = f64::from(1u32 << (SMALL_Z - TERRAIN_Z));
    let margin = (RING.1 + 60.0) / gm;
    let (x0, x1, y0, y1) = (g.get_x_min()? / k, g.get_x_max()? / k, g.get_y_min()? / k, g.get_y_max()? / k);
    let (c0, r0) = ((x0 - margin).floor() as i64, (y0 - margin).floor() as i64);
    let (c1, r1) = ((x1 + margin).ceil() as i64, (y1 + margin).ceil() as i64);
    let (w, h) = ((c1 - c0) as usize, (r1 - r0) as usize);
    let n = 256i64 << TERRAIN_Z;
    let mut dem = vec![f32::NAN; w * h];
    for ty in r0.div_euclid(256)..=(r1 - 1).div_euclid(256) {
        for tx in c0.div_euclid(256)..=(c1 - 1).div_euclid(256) {
            if !(0..n / 256).contains(&tx) || !(0..n / 256).contains(&ty) {
                continue;
            }
            let Some(t) = terrain.tile(tx as u32, ty as u32)? else { continue };
            for j in 0..256i64 {
                let r = ty * 256 + j - r0;
                if !(0..h as i64).contains(&r) {
                    continue;
                }
                for i in 0..256i64 {
                    let c = tx * 256 + i - c0;
                    if (0..w as i64).contains(&c) && roadcore::grid::defined(t[(j * 256 + i) as usize]) {
                        dem[r as usize * w + c as usize] = t[(j * 256 + i) as usize];
                    }
                }
            }
        }
    }
    if !dem.iter().any(|v| v.is_finite()) {
        return Ok(None);
    }
    let scaled = |g: &geos::Geometry| -> Result<Vec<Rings>> {
        Ok(rings_of(g)?
            .into_iter()
            .map(|p| p.into_iter().map(|r| r.into_iter().map(|q| [q[0] / k, q[1] / k]).collect()).collect())
            .collect())
    };
    let mut me = vec![false; w * h];
    for p in scaled(g)? {
        for (o, v) in me.iter_mut().zip(fill_touched(&p, c0, r0, w, h)) {
            *o |= v;
        }
    }
    let mut wat = vec![false; w * h];
    for o in others {
        for p in scaled(o)? {
            for (a, v) in wat.iter_mut().zip(fill(&p, c0, r0, w, h)) {
                *a |= v;
            }
        }
    }
    // (numpy's gradient: central differences inside, one-sided at the edges.)
    let at = |i: usize, j: usize| f64::from(dem[j * w + i]);
    let grad = |i: usize, j: usize| -> f64 {
        let gx = if w < 2 {
            f64::NAN
        } else if i == 0 {
            (at(1, j) - at(0, j)) / gm
        } else if i == w - 1 {
            (at(w - 1, j) - at(w - 2, j)) / gm
        } else {
            (at(i + 1, j) - at(i - 1, j)) / (2.0 * gm)
        };
        let gy = if h < 2 {
            f64::NAN
        } else if j == 0 {
            (at(i, 1) - at(i, 0)) / gm
        } else if j == h - 1 {
            (at(i, h - 1) - at(i, h - 2)) / gm
        } else {
            (at(i, j + 1) - at(i, j - 1)) / (2.0 * gm)
        };
        gx.dhypot(gy)
    };
    let (dist, _) = super::edt::nearest(&me, w, h);
    let (mut sum, mut cnt) = (0.0, 0usize);
    for j in 0..h {
        for i in 0..w {
            let o = j * w + i;
            let d = f64::from(dist[o]) * gm;
            if me[o] || wat[o] || !(f64::from(dem[o]) > 0.3) || d <= RING.0 || d > RING.1 {
                continue;
            }
            let s = grad(i, j);
            if s.is_finite() {
                sum += s;
                cnt += 1;
            }
        }
    }
    Ok((cnt >= 3).then(|| sum / cnt as f64))
}

/// L: the radius of the largest circle inside lake `g` (ground metres, `gm` a zoom-`SMALL_Z`
/// pixel's; islands are shore), the largest of its parts'.
fn largest_distance(g: &geos::Geometry, gm: f64) -> Result<f64> {
    use geos::Geom;
    let mut best = 0f64;
    for k in 0..g.get_num_geometries()? {
        let p = geos::Geom::clone(&g.get_geometry_n(k)?)?;
        if p.geometry_type()? != geos::GeometryTypes::Polygon || p.area()? <= 0.0 {
            continue;
        }
        // (Its tolerance shapely's call gave: half a metre, or a 2000th of the side.)
        let tol = (0.5 / super::grids::px(SMALL_Z)).max(p.area()?.sqrt() / 2000.0);
        best = best.max(p.maximum_inscribed_circle(tol)?.length()?);
    }
    Ok(best * gm)
}

/// The small lakes meeting z10 tile `t` (`Method::LandSlope`'s pieces): the basemap's z14 water of
/// its tiles and a ring of one (`water`, a tile's polygons in its own pixels, 0–256), HydroLAKES'
/// polygons `hl` (zoom-`SMALL_Z` pixels), the terrain; counted in `counts`.
pub fn small_lakes(
    water: &[((u32, u32), Vec<codecs::terrain_water::Poly>)],
    hl: &[geos::Geometry],
    terrain: &dyn Terrain,
    t: (u32, u32),
    counts: &mut Counts,
) -> Result<Vec<Small>> {
    use codecs::terrain_water::Kind;
    use geos::Geom;
    let f = 1u32 << (SMALL_Z - 10);
    let (x0, y0) = (t.0 * f, t.1 * f);
    // The lakes and ponds, merged by OSM id (those of a tile's own merged where they touch).
    let mut pieces: BTreeMap<u64, Vec<geos::Geometry>> = BTreeMap::new();
    let mut own: Vec<geos::Geometry> = Vec::new();
    let mut sea: Vec<geos::Geometry> = Vec::new();
    let (mut rx0, mut ry0, mut rx1, mut ry1) = (u32::MAX, u32::MAX, 0, 0);
    for ((tx, ty), polys) in water {
        (rx0, ry0, rx1, ry1) = (rx0.min(*tx), ry0.min(*ty), rx1.max(*tx), ry1.max(*ty));
        for p in polys {
            if !matches!(p.kind, Kind::Lake | Kind::Sea) {
                continue;
            }
            let rings: Rings = p
                .rings
                .iter()
                .map(|r| r.iter().map(|q| [f64::from(*tx) * 256.0 + q[0], f64::from(*ty) * 256.0 + q[1]]).collect())
                .collect();
            let Ok(g) = polygon(&rings) else { continue };
            match p.kind {
                Kind::Sea => sea.push(g),
                _ if codecs::terrain_water::is_own(p.id) => own.push(g),
                _ => pieces.entry(p.id).or_default().push(g),
            }
        }
    }
    if rx0 > rx1 {
        return Ok(Vec::new());
    }
    let union = |gs: Vec<geos::Geometry>| -> Result<geos::Geometry> {
        let g = if gs.len() == 1 {
            gs.into_iter().next().unwrap()
        } else {
            geos::Geometry::create_geometry_collection(gs)?.unary_union()?
        };
        Ok(if g.is_valid()? { g } else { g.make_valid()? })
    };
    let mut lakes: Vec<geos::Geometry> = Vec::new();
    for (_, gs) in pieces {
        lakes.push(union(gs)?);
    }
    if !own.is_empty() {
        let u = union(own)?;
        for k in 0..u.get_num_geometries()? {
            lakes.push(geos::Geom::clone(&u.get_geometry_n(k)?)?);
        }
    }
    let read = {
        let (a, b) = (f64::from(rx0) * 256.0, f64::from(ry0) * 256.0);
        let (c, d) = (f64::from(rx1 + 1) * 256.0, f64::from(ry1 + 1) * 256.0);
        polygon(&[vec![[a, b], [c, b], [c, d], [a, d]]])?.boundary()?
    };
    let piece = {
        let (a, b) = (f64::from(x0) * 256.0, f64::from(y0) * 256.0);
        let (c, d) = (f64::from(x0 + f) * 256.0, f64::from(y0 + f) * 256.0);
        polygon(&[vec![[a, b], [c, b], [c, d], [a, d]]])?
    };
    let boxes: Vec<[f64; 4]> = lakes
        .iter()
        .map(|g| Ok([g.get_x_min()?, g.get_y_min()?, g.get_x_max()?, g.get_y_max()?]))
        .collect::<Result<_>>()?;
    let near = |b: &[f64; 4], o: &[f64; 4], d: f64| {
        o[0] <= b[2] + d && o[2] >= b[0] - d && o[1] <= b[3] + d && o[3] >= b[1] - d
    };
    let bbox = |g: &geos::Geometry| -> Result<[f64; 4]> {
        Ok([g.get_x_min()?, g.get_y_min()?, g.get_x_max()?, g.get_y_max()?])
    };
    let sea_boxes: Vec<[f64; 4]> = sea.iter().map(bbox).collect::<Result<_>>()?;
    let hl_boxes: Vec<[f64; 4]> = hl.iter().map(bbox).collect::<Result<_>>()?;
    let mut out = Vec::new();
    for (i, g) in lakes.iter().enumerate() {
        if g.is_empty()? || !g.intersects(&piece)? {
            continue;
        }
        let b = boxes[i];
        let gm = ground_px(SMALL_Z, (b[1] + b[3]) / 2.0);
        let area = g.area()?;
        let ha = area * gm * gm / 1e4;
        if ha >= MAX_HA {
            continue;
        }
        let mut count = |k: &'static str| *counts.entry(k).or_default() += 1;
        if g.distance(&read)? < 1.0 {
            count("reaching past the tiles read");
            continue;
        }
        let sea_px = SEA_M / gm;
        let mut wet = false;
        for (s, sb) in sea.iter().zip(&sea_boxes) {
            if near(&b, sb, sea_px) && g.distance(s)? <= sea_px {
                wet = true;
                break;
            }
        }
        if wet {
            count("touching the sea");
            continue;
        }
        let mut covered = 0.0;
        for (h, hb) in hl.iter().zip(&hl_boxes) {
            if near(&b, hb, 0.0) && g.intersects(h)? {
                covered += g.intersection(h)?.area()?;
            }
        }
        if covered >= HYDROLAKES_SHARE * area {
            count("in HydroLAKES");
            continue;
        }
        let reach = (RING.1 + 60.0) / gm;
        let others: Vec<&geos::Geometry> =
            lakes.iter().enumerate().filter(|(j, _)| *j != i && near(&b, &boxes[*j], reach)).map(|(_, o)| o).collect();
        let gm12 = gm * f64::from(1u32 << (SMALL_Z - TERRAIN_Z));
        let s = land_slope(g, &others, terrain, gm12)?;
        let l = largest_distance(g, gm)?;
        if l <= 0.0 {
            continue;
        }
        let (d, by_slope) = dmax_estimate(s, l, ha);
        count(if by_slope { "by the land slope" } else { "by the area rule" });
        out.push(Small { parts: rings_of(g)?, dmax: d });
    }
    Ok(out)
}

/// The small lakes' grids of z6 tile `t`, a z10 tile at a time (`small_lakes`, `small_grid`): its
/// z14 tiles and a ring of one read from `water`, HydroLAKES' lakes `hl` (lon/lat) left out; each
/// grid given to `sink` as it's made (64 MB of floats each: a z6 tile has 256), `progress` told
/// each z10 tile done.
pub fn small_grids(
    water: &dyn codecs::terrain_water::WaterSource,
    hl: &[(u64, Rings)],
    terrain: &dyn Terrain,
    t: (u32, u32),
    counts: &mut Counts,
    sink: &mut dyn FnMut(Canvas) -> Result<()>,
    progress: &dyn Fn(u64, u64),
) -> Result<()> {
    let hl: Vec<geos::Geometry> = hl.iter().filter_map(|(_, r)| polygon(&to_pixels(r, SMALL_Z)).ok()).collect();
    let (k, f) = (10 - 6, 1u32 << (SMALL_Z - 10));
    let n = 1i64 << SMALL_Z;
    let pieces: Vec<(u32, u32)> =
        (t.1 << k..(t.1 + 1) << k).flat_map(|y| (t.0 << k..(t.0 + 1) << k).map(move |x| (x, y))).collect();
    for (i, &(px, py)) in pieces.iter().enumerate() {
        let tiles: Vec<(u8, u32, u32)> = (i64::from(py * f) - 1..=i64::from((py + 1) * f))
            .filter(|y| (0..n).contains(y))
            .flat_map(|y| {
                (i64::from(px * f) - 1..=i64::from((px + 1) * f))
                    .filter(|x| (0..n).contains(x))
                    .map(move |x| (SMALL_Z, x as u32, y as u32))
            })
            .collect();
        let polys = water.depth_polys_many(&tiles)?;
        let read: Vec<((u32, u32), Vec<codecs::terrain_water::Poly>)> =
            tiles.iter().zip(polys).map(|(&(_, x, y), p)| ((x, y), p)).collect();
        let lakes = small_lakes(&read, &hl, terrain, (px, py), counts)?;
        if let Some(c) = small_grid(&lakes, (px, py)) {
            sink(c)?;
        }
        progress(i as u64 + 1, pieces.len() as u64);
    }
    Ok(())
}

/// The small lakes' grid of z10 tile `t`: each lake's bowl (each part of it its own, to the same
/// depth), cropped to its data; None without one.
pub fn small_grid(lakes: &[Small], t: (u32, u32)) -> Option<Canvas> {
    let mut c = Canvas::tile(SMALL_Z, 10, t.0, t.1);
    for l in lakes {
        for part in &l.parts {
            if let Some(b) = Bowl::of(part, SMALL_Z) {
                c.paint(part, &b, l.dmax);
            }
        }
    }
    c.cropped()
}

// ---- the packs -----------------------------------------------------------------------------------

/// A grid in a pack: its file's name, zoom, first column and row (its zoom's global pixels), size,
/// and its bytes' range in the pack.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Packed {
    pub file: String,
    pub zoom: u8,
    pub c0: i64,
    pub r0: i64,
    pub w: u32,
    pub h: u32,
    pub at: [u64; 2],
}

/// The pack's first member, its index.
pub const INDEX: &str = "index.json";

/// A grid encoded for its pack, as made (its floats not kept: `globathy_grids`' and `small_grids`'
/// sinks): its file's name, zoom, first column and row, size, and its GeoTIFF
/// (buildstore::geotiff::write_f32_mercator).
pub struct Tif {
    pub file: String,
    pub zoom: u8,
    pub c0: i64,
    pub r0: i64,
    pub w: u32,
    pub h: u32,
    pub bytes: Vec<u8>,
}

impl Tif {
    /// Depth source `id`'s grid `g`.
    pub fn of(id: &str, g: &Canvas) -> Result<Tif> {
        let (w, h) = (g.w as u32, g.h as u32);
        Ok(Tif {
            file: format!("{id}-{}-{}-{}.tif", g.zoom, g.c0, g.r0),
            zoom: g.zoom,
            c0: g.c0,
            r0: g.r0,
            w,
            h,
            bytes: buildstore::geotiff::write_f32_mercator(g.zoom, g.c0, g.r0, w, h, &g.v)?,
        })
    }
}

/// A pack of `grids`: a tar (ustar, its headers fixed: agentcore::agent::histarchive's) whose first
/// member, `INDEX`, lists them (`Packed`), each grid then its GeoTIFF, in order. (Taken, not
/// copied: a dense tile's are a few hundred MB.)
pub fn pack(grids: Vec<Tif>) -> Result<Vec<u8>> {
    // (The index's own length sets where the grids start: found by trying, as it settles.)
    let mut index_len = 0usize;
    loop {
        let mut at = 512 + index_len.div_ceil(512) * 512;
        let list: Vec<Packed> = grids
            .iter()
            .map(|g| {
                let p = Packed {
                    file: g.file.clone(),
                    zoom: g.zoom,
                    c0: g.c0,
                    r0: g.r0,
                    w: g.w,
                    h: g.h,
                    at: [at as u64 + 512, g.bytes.len() as u64],
                };
                at += 512 + g.bytes.len().div_ceil(512) * 512;
                p
            })
            .collect();
        let index = serde_json::to_vec(&list)?;
        if index.len().div_ceil(512) == index_len.div_ceil(512) && index_len > 0 {
            let mut members = vec![(INDEX.to_string(), index)];
            members.extend(grids.into_iter().map(|g| (g.file, g.bytes)));
            return codecs::tar::build(&members);
        }
        index_len = index.len();
    }
}

/// A pack's grids, from its first member (`INDEX`): `read(offset, len)` reads its bytes.
pub fn unpack_index(read: &dyn Fn(u64, usize) -> Result<Vec<u8>>) -> Result<Vec<Packed>> {
    let h = read(0, 512)?;
    let end = h[..100].iter().position(|&c| c == 0).unwrap_or(100);
    ensure!(&h[..end] == INDEX.as_bytes(), "a lake-depth pack whose first member isn't its index");
    let size = usize::from_str_radix(std::str::from_utf8(&h[124..135])?.trim_end_matches('\0').trim(), 8)
        .context("its index's size")?;
    Ok(serde_json::from_slice(&read(512, size)?)?)
}

#[cfg(test)]
mod tests;
