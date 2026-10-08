//! The water at full detail, and its exact coverage of a tile's pixels.
//!
//! The water is what the basemap draws as water at its deepest zoom, before any simplification or
//! dropping: the sea from the pinned water polygons (`sources/basemap/water-polygons-split-3857.zip`,
//! what Planetiler reads for the ocean) and the inland water from the pass's `water` set (lakes,
//! reservoirs, rivers' and canals' areas, as osmium assembles them; their holes are islands).
//!
//! **The store** (a folder, `Geom`): every ring, its points as Web Mercator world units × 2³²
//! (u32, 9 mm at the equator), oriented so an outer ring winds +1 and a hole −1, and a grid at
//! `GRID_Z` filing each ring by its box. The rings aren't grouped into polygons: summed winding
//! (`coverage`) needs no grouping, and a ring wholly outside a tile adds nothing to it.
//! - `meta.json`: `{"fmt": 1, "rings", "points", "grid_z", "sea_rings"}` (the first `sea_rings`
//!   rings are the sea's, the rest inland water);
//! - `boxes.bin`: per ring `[x0, y0, x1, y1]` (u32);
//! - `offsets.bin`: per ring its first point (u64), and the count of points after the last;
//! - `points.bin`: `[x, y]` (u32), a ring's closing point left out;
//! - `grid.bin`: per grid cell (row by row) its first entry (u64), and the count after the last;
//!   `cells.bin`: the entries, ring indices (u32), each cell's ascending.
//!
//! **Coverage** is exact: each pixel's share of water, the area of the water inside its square
//! over the square's (signed-area accumulation, as font renderers do: each edge adds its area and
//! its crossing to the pixels it passes, and a sum along the row gives the winding inside each),
//! the winding clamped to 0–1. Water areas that abut (the sea's cells, a river mouth on the
//! coast) sum exactly; where two overlap the clamp takes one, and an edge pixel of the overlap may
//! count it twice.

use anyhow::{bail, Context, Result};
use det::Det;
use rayon::prelude::*;
use std::io::{BufRead, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// The grid's zoom: 128 × 128 cells (313 km at the equator).
pub const GRID_Z: u8 = 7;
const SCALE: f64 = 4_294_967_296.0;

/// Longitude and latitude in world units (Web Mercator, 0–1, y down).
pub fn world(lon: f64, lat: f64) -> [f64; 2] {
    let s = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians().dsin();
    [lon / 360.0 + 0.5, (0.5 - 0.25 * ((1.0 + s) / (1.0 - s)).dln() / std::f64::consts::PI).clamp(0.0, 1.0)]
}

fn quant(v: f64) -> u32 {
    (v * SCALE).round().clamp(0.0, SCALE - 1.0) as u32
}

/// Twice a ring's signed area (world units, y down: positive clockwise on the map). A closing point
/// repeating the first is fine.
pub fn area2(r: &[[f64; 2]]) -> f64 {
    let Some(&o) = r.first() else { return 0.0 };
    let mut a = 0.0;
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        a += (p[0] - o[0]) * (q[1] - o[1]) - (q[0] - o[0]) * (p[1] - o[1]);
    }
    a
}

// ---- the store, written --------------------------------------------------------------------------

/// Writes a store: rings as they come (`add`), then the grid (`finish`).
pub struct GeomWriter {
    dir: PathBuf,
    boxes: BufWriter<std::fs::File>,
    offsets: BufWriter<std::fs::File>,
    points: BufWriter<std::fs::File>,
    rings: u64,
    npoints: u64,
    /// Rings left out: across the antimeridian, or with no area.
    pub skipped: u64,
    /// The rings written before `end_sea` (the sea's).
    sea_rings: Option<u64>,
}

impl GeomWriter {
    pub fn create(dir: &Path) -> Result<GeomWriter> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let f = |n: &str| -> Result<BufWriter<std::fs::File>> { Ok(BufWriter::with_capacity(1 << 20, std::fs::File::create(dir.join(n)).with_context(|| format!("create {n}"))?)) };
        Ok(GeomWriter { dir: dir.to_path_buf(), boxes: f("boxes.bin")?, offsets: f("offsets.bin")?, points: f("points.bin")?, rings: 0, npoints: 0, skipped: 0, sea_rings: None })
    }

    /// A ring (world units; closed or not), oriented as an outer ring (`outer`) or a hole whatever
    /// its winding as given. Rings across the antimeridian (wider than half the world) and rings
    /// with no area are left out.
    pub fn add(&mut self, ring: &[[f64; 2]], outer: bool) -> Result<()> {
        let mut r: Vec<[u32; 2]> = ring.iter().map(|p| [quant(p[0]), quant(p[1])]).collect();
        if r.len() > 1 && r.first() == r.last() {
            r.pop();
        }
        r.dedup();
        if r.len() < 3 {
            self.skipped += 1;
            return Ok(());
        }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for p in &r {
            (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
        }
        if u64::from(x1 - x0) > 1 << 31 {
            self.skipped += 1;
            return Ok(());
        }
        // Twice its signed area about its box's corner (exact enough in f64 for any ring here).
        let mut a = 0.0f64;
        for i in 0..r.len() {
            let (p, q) = (r[i], r[(i + 1) % r.len()]);
            let (px, py, qx, qy) = (f64::from(p[0] - x0), f64::from(p[1] - y0), f64::from(q[0] - x0), f64::from(q[1] - y0));
            a += px * qy - qx * py;
        }
        if a == 0.0 {
            self.skipped += 1;
            return Ok(());
        }
        // Outer rings clockwise on the map (y down: positive), holes anticlockwise.
        if (a > 0.0) != outer {
            r.reverse();
        }
        for v in [x0, y0, x1, y1] {
            self.boxes.write_all(&v.to_le_bytes())?;
        }
        self.offsets.write_all(&self.npoints.to_le_bytes())?;
        for p in &r {
            self.points.write_all(&p[0].to_le_bytes())?;
            self.points.write_all(&p[1].to_le_bytes())?;
        }
        self.npoints += r.len() as u64;
        self.rings += 1;
        Ok(())
    }

    /// The rings so far are the sea's; the rest, inland water.
    pub fn end_sea(&mut self) {
        self.sea_rings = Some(self.rings);
    }

    /// A polygon: its outer ring, then its holes.
    pub fn add_polygon(&mut self, rings: &[Vec<[f64; 2]>]) -> Result<()> {
        for (i, r) in rings.iter().enumerate() {
            self.add(r, i == 0)?;
        }
        Ok(())
    }

    /// The grid, and the store's meta. Returns the rings and points written.
    pub fn finish(mut self) -> Result<(u64, u64)> {
        self.offsets.write_all(&self.npoints.to_le_bytes())?;
        for mut w in [self.boxes, self.offsets, self.points] {
            w.flush()?;
            w.get_mut().sync_all().ok();
        }
        let boxes: Vec<[u32; 4]> = read_vec(&self.dir.join("boxes.bin"))?;
        let n = 1usize << GRID_Z;
        let shift = 32 - u32::from(GRID_Z);
        let cells_of = |b: &[u32; 4]| ((b[0] >> shift) as usize..=(b[2] >> shift) as usize, (b[1] >> shift) as usize..=(b[3] >> shift) as usize);
        let mut counts = vec![0u64; n * n + 1];
        for b in &boxes {
            let (xs, ys) = cells_of(b);
            for y in ys {
                for x in xs.clone() {
                    counts[y * n + x + 1] += 1;
                }
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut fill = counts.clone();
        let mut cells = vec![0u32; counts[n * n] as usize];
        for (i, b) in boxes.iter().enumerate() {
            let (xs, ys) = cells_of(b);
            for y in ys {
                for x in xs.clone() {
                    let c = &mut fill[y * n + x];
                    cells[*c as usize] = i as u32;
                    *c += 1;
                }
            }
        }
        write_vec(&self.dir.join("grid.bin"), &counts)?;
        write_vec(&self.dir.join("cells.bin"), &cells)?;
        std::fs::write(self.dir.join("meta.json"), serde_json::to_vec(&serde_json::json!({"fmt": 1, "rings": self.rings, "points": self.npoints, "grid_z": GRID_Z, "skipped": self.skipped, "sea_rings": self.sea_rings.unwrap_or(0)}))?)?;
        Ok((self.rings, self.npoints))
    }
}

fn read_vec<T: bytemuck::Pod>(p: &Path) -> Result<Vec<T>> {
    let b = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    Ok(bytemuck::pod_collect_to_vec(&b))
}

fn write_vec<T: bytemuck::Pod>(p: &Path, v: &[T]) -> Result<()> {
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(p).with_context(|| format!("create {}", p.display()))?);
    w.write_all(bytemuck::cast_slice(v))?;
    w.flush()?;
    Ok(())
}

// ---- the store, read ------------------------------------------------------------------------------

/// A store, mapped.
pub struct Geom {
    boxes: memmap2::Mmap,
    offsets: memmap2::Mmap,
    points: memmap2::Mmap,
    grid: Vec<u64>,
    cells: memmap2::Mmap,
    /// The first `sea_rings` rings are the sea's.
    pub sea_rings: u32,
}

fn map(p: &Path) -> Result<memmap2::Mmap> {
    let f = std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
    // SAFETY: the store's files are written once and not changed while read.
    unsafe { memmap2::Mmap::map(&f) }.with_context(|| format!("map {}", p.display()))
}

impl Geom {
    pub fn open(dir: &Path) -> Result<Geom> {
        let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("meta.json")).with_context(|| format!("{}: no store", dir.display()))?)?;
        if meta["fmt"] != 1 || meta["grid_z"] != u64::from(GRID_Z) {
            bail!("{}: a store of another format ({meta})", dir.display());
        }
        Ok(Geom { boxes: map(&dir.join("boxes.bin"))?, offsets: map(&dir.join("offsets.bin"))?, points: map(&dir.join("points.bin"))?, grid: read_vec(&dir.join("grid.bin"))?, cells: map(&dir.join("cells.bin"))?, sea_rings: meta["sea_rings"].as_u64().unwrap_or(0) as u32 })
    }

    pub fn rings(&self) -> usize {
        self.boxes.len() / 16
    }

    fn boxes(&self) -> &[[u32; 4]] {
        bytemuck::cast_slice(&self.boxes)
    }

    /// Ring `i`'s points.
    pub fn ring(&self, i: usize) -> &[[u32; 2]] {
        let o: &[u64] = bytemuck::cast_slice(&self.offsets);
        let p: &[[u32; 2]] = bytemuck::cast_slice(&self.points);
        &p[o[i] as usize..o[i + 1] as usize]
    }

    /// The rings whose box meets the box `[x0, y0, x1, y1]` (world units × 2³², inclusive), each
    /// once, ascending.
    pub fn rings_in(&self, b: [u64; 4]) -> Vec<u32> {
        let boxes = self.boxes();
        let meets = |r: &[u32; 4]| u64::from(r[0]) <= b[2] && u64::from(r[2]) >= b[0] && u64::from(r[1]) <= b[3] && u64::from(r[3]) >= b[1];
        let shift = 32 - u32::from(GRID_Z);
        let n = 1usize << GRID_Z;
        let (cx0, cy0) = ((b[0] >> shift) as usize, (b[1] >> shift) as usize);
        let (cx1, cy1) = (((b[2].min(u64::from(u32::MAX))) >> shift) as usize, ((b[3].min(u64::from(u32::MAX))) >> shift) as usize);
        if (cx1 - cx0 + 1) * (cy1 - cy0 + 1) > 64 {
            // A box over many cells (a low zoom's tile): every ring's box asked.
            return (0..boxes.len() as u32).into_par_iter().filter(|&i| meets(&boxes[i as usize])).collect();
        }
        let cells: &[u32] = bytemuck::cast_slice(&self.cells);
        let mut out: Vec<u32> = Vec::new();
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let c = cy * n + cx;
                out.extend(cells[self.grid[c] as usize..self.grid[c + 1] as usize].iter().copied().filter(|&i| meets(&boxes[i as usize])));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

// ---- coverage -------------------------------------------------------------------------------------

/// Accumulates edges into a `w` × `h` raster's signed areas (a row `w + 2` wide: the edges clamped
/// to the raster's right side land in the two cells past it).
pub struct Raster {
    w: usize,
    h: usize,
    acc: Vec<f32>,
}

impl Raster {
    pub fn new(w: usize, h: usize) -> Raster {
        Raster { w, h, acc: vec![0.0; (w + 2) * h] }
    }

    /// An edge from `a` to `b` (pixels; anywhere: what's left of the raster counts as crossing its
    /// left side, what's right of it or above or below adds nothing).
    pub fn edge(&mut self, a: [f64; 2], b: [f64; 2]) {
        if a[1] == b[1] {
            return;
        }
        let (top, bot) = if a[1] < b[1] { (a[1], b[1]) } else { (b[1], a[1]) };
        if bot <= 0.0 || top >= self.h as f64 {
            return;
        }
        // Split where it crosses x = 0 and x = w, each piece clamped: left of the raster, a
        // vertical edge on its left side.
        let w = self.w as f64;
        let mut ts = [0.0, 1.0, 1.0, 1.0];
        let mut n = 1;
        for xc in [0.0, w] {
            if (a[0] < xc) != (b[0] < xc) {
                let t = (xc - a[0]) / (b[0] - a[0]);
                if t > 0.0 && t < 1.0 {
                    ts[n] = t;
                    n += 1;
                }
            }
        }
        let ts = &mut ts[..n];
        ts.sort_by(f64::total_cmp);
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let mut p = a;
        for q in ts[1..].iter().map(|&t| at(t)).chain(std::iter::once(b)) {
            let mid = (p[0] + q[0]) / 2.0;
            if mid < w {
                let c = |v: f64| v.clamp(0.0, w);
                self.line([c(p[0]), p[1]], [c(q[0]), q[1]]);
            }
            p = q;
        }
    }

    /// An edge within 0 ≤ x ≤ w (font-rs's accumulation).
    fn line(&mut self, p0: [f64; 2], p1: [f64; 2]) {
        if p0[1] == p1[1] {
            return;
        }
        // (Downward edges subtract: an outer ring, clockwise on the map, winds +1 inside.)
        let (dir, p0, p1) = if p0[1] < p1[1] { (-1.0, p0, p1) } else { (1.0, p1, p0) };
        let wf = self.w as f64;
        let stride = self.w + 2;
        let dxdy = (p1[0] - p0[0]) / (p1[1] - p0[1]);
        let mut x = p0[0];
        let ystart = p0[1].max(0.0);
        if p0[1] < 0.0 {
            x = (x - p0[1] * dxdy).clamp(0.0, wf);
        }
        let yend = (p1[1].ceil() as usize).min(self.h);
        for y in (ystart as usize)..yend {
            let row = y * stride;
            let dy = ((y + 1) as f64).min(p1[1]) - (y as f64).max(p0[1]);
            let xnext = (x + dxdy * dy).clamp(0.0, wf);
            let d = (dy * dir) as f32;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0floor = x0.floor();
            let x0i = x0floor as usize;
            let x1ceil = x1.ceil();
            let x1i = x1ceil as usize;
            let acc = &mut self.acc;
            if x1i <= x0i + 1 {
                let xmf = (0.5 * (x + xnext) - x0floor) as f32;
                acc[row + x0i] += d - d * xmf;
                acc[row + x0i + 1] += d * xmf;
            } else {
                let s = (1.0 / (x1 - x0)) as f32;
                let x0f = (x0 - x0floor) as f32;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = (x1 - x1ceil + 1.0) as f32;
                let am = 0.5 * s * x1f * x1f;
                acc[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    acc[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    acc[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    acc[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                acc[row + x1i] += d * am;
            }
            x = xnext;
        }
    }

    /// Each pixel's coverage, 0–1, row by row.
    pub fn coverage(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.w * self.h);
        for row in self.acc.chunks_exact(self.w + 2) {
            let mut s = 0.0f32;
            for v in &row[..self.w] {
                s += v;
                out.push(s.clamp(0.0, 1.0));
            }
        }
        out
    }
}

/// Which water.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    All,
    Sea,
    Inland,
}

/// Tile z/x/y's coverage by the water, `size` × `size` pixels, row by row (exact: `Raster`).
pub fn tile_coverage(g: &Geom, z: u8, x: u32, y: u32, size: usize) -> Vec<f32> {
    tile_coverage_of(g, z, x, y, size, Which::All)
}

/// Tile z/x/y's coverage by the sea, the inland water or both.
pub fn tile_coverage_of(g: &Geom, z: u8, x: u32, y: u32, size: usize, which: Which) -> Vec<f32> {
    // The tile's box in world units × 2³² (z ≤ 32).
    let span = 1u64 << (32 - u32::from(z));
    let b = [u64::from(x) * span, u64::from(y) * span, (u64::from(x) + 1) * span - 1, (u64::from(y) + 1) * span - 1];
    let mut rings = g.rings_in(b);
    match which {
        Which::All => {}
        Which::Sea => rings.retain(|&i| i < g.sea_rings),
        Which::Inland => rings.retain(|&i| i >= g.sea_rings),
    }
    // Pixels per world unit × 2³²; the tile's corner.
    let k = size as f64 / span as f64;
    let (ox, oy) = (f64::from(x) * size as f64, f64::from(y) * size as f64);
    // In parallel by stripes of rings, summed (the sum is the same whatever the split, to f32's
    // rounding: the stripes are fixed by count, so the bytes are too).
    const STRIPE: usize = 4096;
    let parts: Vec<Raster> = rings
        .par_chunks(STRIPE)
        .map(|ch| {
            let mut r = Raster::new(size, size);
            for &i in ch {
                let ring = g.ring(i as usize);
                let px = |p: [u32; 2]| [f64::from(p[0]) * k - ox, f64::from(p[1]) * k - oy];
                let mut prev = px(ring[ring.len() - 1]);
                for &p in ring {
                    let q = px(p);
                    r.edge(prev, q);
                    prev = q;
                }
            }
            r
        })
        .collect();
    let mut it = parts.into_iter();
    let Some(mut acc) = it.next() else { return vec![0.0; size * size] };
    for r in it {
        for (a, b) in acc.acc.iter_mut().zip(&r.acc) {
            *a += b;
        }
    }
    acc.coverage()
}

// ---- reading the sources --------------------------------------------------------------------------

/// The pass's `water` set's filter (pipeline::osmpass::SETS): the areas below.
pub const SET_FILTER: &[&str] = &["wr/natural=water", "wr/landuse=reservoir,basin,salt_pond", "wr/waterway=dock", "wr/water=river,stream,canal,ditch,drain,pond,basin,wastewater"];
/// What the basemap draws as water, as osmium's area tags (the export's `area_tags`): OpenMapTiles'
/// water polygons (natural=water, the reservoir, basin and salt pond land uses, docks, and
/// water=river … wastewater; not bays, which it reads but doesn't draw).
pub const AREA_TAGS: &[&str] = &[
    "natural=water",
    "landuse=reservoir",
    "landuse=basin",
    "landuse=salt_pond",
    "waterway=dock",
    "water=river",
    "water=stream",
    "water=canal",
    "water=ditch",
    "water=drain",
    "water=pond",
    "water=basin",
    "water=wastewater",
];

/// Whether an exported area is water the basemap draws: not water in a tunnel or culvert (a tunnel
/// tag but no, 0 or false, as OpenMapTiles reads it, which the map hides), nor covered water
/// (covered=yes, which OpenMapTiles leaves out).
pub fn drawn(p: &serde_json::Map<String, serde_json::Value>) -> bool {
    let tag = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(tag("tunnel"), "" | "no" | "0" | "false") || tag("covered") == "yes" {
        return false;
    }
    tag("natural") == "water"
        || matches!(tag("landuse"), "reservoir" | "basin" | "salt_pond")
        || tag("waterway") == "dock"
        || matches!(tag("water"), "river" | "stream" | "canal" | "ditch" | "drain" | "pond" | "basin" | "wastewater")
}

/// Polygons, each its outer ring then its holes (world units).
pub type Polygons = Vec<Vec<Vec<[f64; 2]>>>;

/// One line of `osmium export`'s GeoJSON sequence: its polygons (each its outer ring then its
/// holes, world units), if it's water the basemap draws.
pub fn parse_export_line(line: &str) -> Option<Polygons> {
    let line = line.trim_matches(|c: char| c == '\u{1e}' || c.is_whitespace());
    if line.is_empty() {
        return None;
    }
    let f: serde_json::Value = serde_json::from_str(line).ok()?;
    if !drawn(f.get("properties")?.as_object()?) {
        return None;
    }
    let g = f.get("geometry")?;
    let pt = |v: &serde_json::Value| -> Option<[f64; 2]> { Some(world(v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)) };
    let ring = |r: &serde_json::Value| -> Option<Vec<[f64; 2]>> { r.as_array()?.iter().map(pt).collect() };
    let poly = |v: &serde_json::Value| -> Option<Vec<Vec<[f64; 2]>>> { v.as_array()?.iter().map(ring).collect() };
    let coords = g.get("coordinates")?;
    match g.get("type")?.as_str()? {
        "Polygon" => Some(vec![poly(coords)?]),
        "MultiPolygon" => coords.as_array()?.iter().map(poly).collect(),
        _ => None,
    }
}

/// Reads an export into the store, in order (batches parsed in parallel). Returns the features
/// read.
pub fn read_export(r: impl BufRead, w: &mut GeomWriter, said: &dyn Fn(u64)) -> Result<u64> {
    let mut lines = r.lines();
    let mut n = 0u64;
    let mut batch: Vec<String> = Vec::with_capacity(16384);
    loop {
        batch.clear();
        for l in lines.by_ref().take(16384) {
            batch.push(l.context("read the export")?);
        }
        if batch.is_empty() {
            break;
        }
        n += batch.len() as u64;
        let polys: Vec<Option<Polygons>> = batch.par_iter().map(|l| parse_export_line(l)).collect();
        for p in polys.into_iter().flatten() {
            for poly in &p {
                w.add_polygon(poly)?;
            }
        }
        said(n);
    }
    Ok(n)
}

/// Reads the water polygons' shapefile (EPSG:3857, as a stream) into the store: every ring, outer
/// (clockwise, y up) or hole (anticlockwise) as the shapefile winds it. Returns the polygons read.
pub fn read_water_polygons(mut r: impl Read, w: &mut GeomWriter) -> Result<u64> {
    let mut head = [0u8; 100];
    r.read_exact(&mut head).context("the shapefile's header")?;
    let word = |o: usize| i32::from_be_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]]);
    if word(0) != 9994 {
        bail!("not a shapefile");
    }
    let mut left = (u64::try_from(word(24)).context("the shapefile's length")? * 2).checked_sub(100).context("the shapefile's length")?;
    let mut polygons = 0u64;
    let mut batch: Vec<Vec<u8>> = Vec::with_capacity(256);
    while left > 0 {
        batch.clear();
        while batch.len() < 256 && left > 0 {
            let mut rh = [0u8; 8];
            r.read_exact(&mut rh).context("a record's header")?;
            let len = usize::try_from(i32::from_be_bytes([rh[4], rh[5], rh[6], rh[7]])).context("a record's length")? * 2;
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).context("a record cut short")?;
            left = left.checked_sub(8 + len as u64).context("a record past the shapefile's length")?;
            batch.push(body);
        }
        polygons += batch.len() as u64;
        let made: Vec<Vec<(Vec<[f64; 2]>, bool)>> = batch.par_iter().map(|b| shp_rings(b)).collect();
        for rings in made {
            for (r, outer) in rings {
                w.add(&r, outer)?;
            }
        }
    }
    Ok(polygons)
}

/// A shapefile polygon record's rings, world units, each with whether it's an outer ring (clockwise
/// with y up, as shapefiles wind them).
fn shp_rings(b: &[u8]) -> Vec<(Vec<[f64; 2]>, bool)> {
    const WORLD_M: f64 = 40_075_016.685_578_49;
    let i32_at = |o: usize| b.get(o..o + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()));
    let f64_at = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    if i32_at(0) != Some(5) {
        return Vec::new();
    }
    let (Some(nparts), Some(npts)) = (i32_at(36), i32_at(40)) else { return Vec::new() };
    let (nparts, npts) = (nparts.max(0) as usize, npts.max(0) as usize);
    let pts_at = 44 + 4 * nparts;
    if b.len() < pts_at + 16 * npts {
        return Vec::new();
    }
    let parts: Vec<usize> = (0..nparts).map(|k| i32_at(44 + 4 * k).unwrap_or(0).max(0) as usize).collect();
    let mut out = Vec::new();
    for k in 0..nparts {
        let (s, e) = (parts[k], if k + 1 < nparts { parts[k + 1] } else { npts });
        if e <= s + 2 || e > npts {
            continue;
        }
        let r: Vec<[f64; 2]> = (s..e).map(|i| [f64_at(pts_at + 16 * i) / WORLD_M + 0.5, 0.5 - f64_at(pts_at + 16 * i + 8) / WORLD_M]).collect();
        // y flipped: clockwise with y up is positive with y down.
        let outer = area2(&r) > 0.0;
        out.push((r, outer));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(rings: &[(Vec<[f64; 2]>, bool)]) -> (tempfile::TempDir, Geom) {
        let d = tempfile::tempdir().unwrap();
        let mut w = GeomWriter::create(d.path()).unwrap();
        for (r, o) in rings {
            w.add(r, *o).unwrap();
        }
        w.finish().unwrap();
        let g = Geom::open(d.path()).unwrap();
        (d, g)
    }

    /// A square in world units.
    fn square(x: f64, y: f64, s: f64) -> Vec<[f64; 2]> {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]]
    }

    #[test]
    fn a_pixel_aligned_square_covers_its_pixels() {
        // Tile 10/500/300, 256 px: a square of pixels 10–20 × 30–50.
        let px = 1.0 / (256.0 * 1024.0);
        let (ox, oy) = (500.0 / 1024.0, 300.0 / 1024.0);
        let (_d, g) = store(&[(vec![[ox + 10.0 * px, oy + 30.0 * px], [ox + 20.0 * px, oy + 30.0 * px], [ox + 20.0 * px, oy + 50.0 * px], [ox + 10.0 * px, oy + 50.0 * px]], true)]);
        let c = tile_coverage(&g, 10, 500, 300, 256);
        for y in 0..256 {
            for x in 0..256 {
                let want = if (10..20).contains(&x) && (30..50).contains(&y) { 1.0 } else { 0.0 };
                assert!((c[y * 256 + x] - want).abs() < 1e-3, "{x},{y}: {}", c[y * 256 + x]);
            }
        }
    }

    #[test]
    fn coverage_is_the_area_inside_each_pixel() {
        // A square off the grid, its hole, either winding given: the sum over pixels is its area,
        // and a pixel cut by an edge has its share.
        let px = 1.0 / (256.0 * 1024.0);
        let (ox, oy) = (500.0 / 1024.0, 300.0 / 1024.0);
        let at = |x: f64, y: f64| [ox + x * px, oy + y * px];
        let mut outer = vec![at(10.25, 30.5), at(20.75, 30.5), at(20.75, 50.25), at(10.25, 50.25)];
        outer.reverse();
        let hole = vec![at(12.0, 32.0), at(14.5, 32.0), at(14.5, 34.0), at(12.0, 34.0)];
        let (_d, g) = store(&[(outer, true), (hole, false)]);
        let c = tile_coverage(&g, 10, 500, 300, 256);
        let sum: f64 = c.iter().map(|&v| f64::from(v)).sum();
        let want = 10.5 * 19.75 - 2.5 * 2.0;
        assert!((sum - want).abs() < 1e-2, "{sum} {want}");
        assert!((c[30 * 256 + 10] - 0.75 * 0.5).abs() < 1e-4, "{}", c[30 * 256 + 10]);
        assert!((c[40 * 256 + 20] - 0.75).abs() < 1e-4);
        assert!((c[32 * 256 + 14] - 0.5).abs() < 1e-4);
        assert!(c[32 * 256 + 12].abs() < 1e-4);
    }

    #[test]
    fn rings_past_the_tile_and_tiny_ones() {
        // A square much bigger than the tile covers all of it; one wholly left of it, nothing; a
        // pond a tenth of a pixel across, a hundredth of its pixel.
        let px = 1.0 / (256.0 * 1024.0);
        let (ox, oy) = (500.0 / 1024.0, 300.0 / 1024.0);
        let (_d, g) = store(&[(square(ox - 0.01, oy - 0.01, 0.03), true)]);
        assert!(tile_coverage(&g, 10, 500, 300, 64).iter().all(|&v| (v - 1.0).abs() < 1e-5));
        let (_d, g) = store(&[(square(ox - 0.01, oy - 0.001, 0.005), true)]);
        assert!(tile_coverage(&g, 10, 500, 300, 64).iter().all(|&v| v.abs() < 1e-5));
        let (_d, g) = store(&[(square(ox + 100.45 * px, oy + 7.45 * px, 0.1 * px), true)]);
        let c = tile_coverage(&g, 10, 500, 300, 256);
        assert!((c[7 * 256 + 100] - 0.01).abs() < 1e-5, "{}", c[7 * 256 + 100]);
        assert!((c.iter().sum::<f32>() - 0.01).abs() < 1e-5);
    }

    #[test]
    fn a_circle_s_coverage_sums_to_its_area_at_any_zoom() {
        // A circle of radius 37.3 px at z12 (256 px): every lower zoom sees its area quartered.
        let n = 2000;
        let (cx, cy) = ((1000.0 + 0.3) / 4096.0, (2000.0 + 0.6) / 4096.0);
        let r = 37.3 / (256.0 * 4096.0);
        let ring: Vec<[f64; 2]> = (0..n).map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            [cx + r * a.cos(), cy + r * a.sin()]
        }).collect();
        let area_px12 = 0.5 * area2(&ring).abs() * (256.0 * 4096.0f64).powi(2);
        let (_d, g) = store(&[(ring, true)]);
        for z in [12u8, 10, 8, 5] {
            let s = 1u32 << (12 - z);
            let c = tile_coverage(&g, z, 1000 / s, 2000 / s, 256);
            let sum: f64 = c.iter().map(|&v| f64::from(v)).sum();
            let want = area_px12 / f64::from(s * s);
            assert!((sum - want).abs() < 1e-3 * want.max(1.0), "z{z}: {sum} {want}");
        }
    }
}
