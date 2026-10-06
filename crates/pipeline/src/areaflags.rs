//! A unit's area flags (`grid.areas.u8`: the PARK, HERITAGE, SPECIAL_AREA and INDIGENOUS bits of
//! roadcore::scenic::flag), rasterised onto its own z11 grid (`grid.idx`) from the heritage-sites
//! job's flagged polygons near it (`area-shapes.geojsonseq`, crate::heritage::unit_inputs), as
//! heritage.py rasterises them for a whole build (docs/phase5.md "Heritage and area flags"). Each
//! tile is the union of its polygons' burns, so units agree wherever their grids overlap.
//!
//! The bytes are dem/areaflags.py's, which burns with rasterio (GDAL 3.12):
//! - **Choice:** a polygon is read when its bounding box in degrees meets the grid's (Web Mercator
//!   is monotone on each axis, so one whose box misses the grid's misses it in Mercator too), and
//!   drawn on a tile when its Mercator envelope (of its shells, as GEOS's) meets the tile's.
//! - **Burn:** the pixels whose centres a polygon holds, even-odd over its rings, a multipolygon's
//!   parts each on their own; HERITAGE also takes every pixel its rings pass through (GDAL's
//!   ALL_TOUCHED).
//! - **Arithmetic:** GDAL's, as built for the Mac's Python (its fused multiply-adds included), on
//!   the same Mercator coordinates, so a tie at a pixel centre falls the same way.

use anyhow::{bail, Context, Result};
use det::Det;
use rayon::prelude::*;
use roadcore::grid::{CELLS, TS};
use roadcore::scenic::flag::{HERITAGE, INDIGENOUS, PARK, SPECIAL_AREA};
use std::f64::consts::PI;
use std::path::Path;

/// The bits drawn (a polygon with another bit is read but never drawn).
const BITS: [u8; 4] = [PARK, HERITAGE, SPECIAL_AREA, INDIGENOUS];
/// Web Mercator's sphere.
const R: f64 = 6378137.0;
/// Tiles per side at z11.
const N11: f64 = 2048.0;

/// A polygon in Web Mercator metres: its rings, the shell first, each closed and in the order GDAL
/// walks it (clockwise, by OGR's test).
type Poly = Vec<Vec<[f64; 2]>>;

/// A flagged area: its polygons (a multipolygon's parts), its bit, and its Mercator envelope
/// [min x, min y, max x, max y] over its shells.
pub struct Shape {
    pub bit: u8,
    polys: Vec<Poly>,
    env: [f64; 4],
}

/// Writes `dir`'s `grid.areas.u8` from the polygons in `shapes` (area-shapes.geojsonseq). A folder
/// without grid tiles gets no file.
pub fn run(dir: &Path, shapes: &Path) -> Result<()> {
    let tiles = read_grid(&dir.join("grid.idx"))?;
    if tiles.is_empty() {
        return Ok(());
    }
    let text = std::fs::read_to_string(shapes).with_context(|| format!("{}", shapes.display()))?;
    let (list, read) = read_shapes(&text, grid_box(&tiles))?;
    eprintln!("areaflags: {read} polygons over {} grid tiles", tiles.len());
    let out = rasterise(&tiles, &list);
    std::fs::write(roadcore::tmp(dir, "grid.areas.u8"), &out)?;
    roadcore::commit(dir, &["grid.areas.u8"])
}

/// grid.idx's tiles (uint32 x, y pairs), in its order.
fn read_grid(path: &Path) -> Result<Vec<[u32; 2]>> {
    let b = std::fs::read(path).with_context(|| format!("{}", path.display()))?;
    if b.len() % 8 != 0 {
        bail!("{}: {} bytes, not whole tiles", path.display(), b.len());
    }
    Ok(bytemuck::pod_collect_to_vec(&b))
}

/// The grid's box in degrees: [west, south, east, north].
fn grid_box(tiles: &[[u32; 2]]) -> [f64; 4] {
    let lon = |x: u32| x as f64 / N11 * 360.0 - 180.0;
    let lat = |y: u32| (PI * (1.0 - 2.0 * y as f64 / N11)).dsinh().datan().to_degrees();
    let (x0, x1) = (tiles.iter().map(|t| t[0]).min().unwrap(), tiles.iter().map(|t| t[0]).max().unwrap());
    let (y0, y1) = (tiles.iter().map(|t| t[1]).min().unwrap(), tiles.iter().map(|t| t[1]).max().unwrap());
    [lon(x0), lat(y1 + 1), lon(x1 + 1), lat(y0)]
}

/// Degrees to Web Mercator metres, latitudes clipped to ±85°.
fn to_merc(p: [f64; 2]) -> [f64; 2] {
    let lat = if p[1] < -85.0 {
        -85.0
    } else if p[1] > 85.0 {
        85.0
    } else {
        p[1]
    };
    [p[0] * PI / 180.0 * R, (PI / 4.0 + lat * PI / 360.0).dtan().dln() * R]
}

/// One GeoJSON ring: closed if it isn't (as shapely closes it); fewer than four points is an error
/// (shapely's too).
fn ring(v: &serde_json::Value) -> Result<Vec<[f64; 2]>> {
    let pts = v.as_array().context("a ring that isn't an array")?;
    let mut r = Vec::with_capacity(pts.len() + 1);
    for c in pts {
        let (x, y) = (c.get(0).and_then(|x| x.as_f64()), c.get(1).and_then(|y| y.as_f64()));
        r.push([x.context("a coordinate without x")?, y.context("a coordinate without y")?]);
    }
    if r.first() != r.last() {
        r.push(r[0]);
    }
    if r.len() < 4 {
        bail!("a ring of {} points", r.len());
    }
    Ok(r)
}

/// Coordinates with no coordinate in them (shapely's empty geometry).
fn empty(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Array(a) => a.iter().all(empty),
        _ => false,
    }
}

/// A GeoJSON polygon's rings in degrees; none for an empty one.
fn polygon(v: &serde_json::Value) -> Result<Vec<Vec<[f64; 2]>>> {
    if empty(v) {
        return Ok(Vec::new());
    }
    v.as_array().context("polygon coordinates that aren't an array")?.iter().map(ring).collect()
}

/// The polygons of area-shapes.geojsonseq (`text`) whose bounding box in degrees meets `b`
/// ([west, south, east, north]), in Web Mercator; and how many were read (empty ones aside).
pub fn read_shapes(text: &str, b: [f64; 4]) -> Result<(Vec<Shape>, usize)> {
    let mut out = Vec::new();
    let mut read = 0;
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let at = || format!("area-shapes line {}", i + 1);
        let v: serde_json::Value = serde_json::from_str(line).with_context(at)?;
        let g = &v["geometry"];
        if empty(&g["coordinates"]) {
            continue;
        }
        let parts: Vec<Vec<Vec<[f64; 2]>>> = match g["type"].as_str() {
            Some("Polygon") => vec![polygon(&g["coordinates"]).with_context(at)?],
            Some("MultiPolygon") => {
                let ps = g["coordinates"].as_array().with_context(at)?;
                let ps: Vec<_> = ps.iter().map(polygon).collect::<Result<_>>().with_context(at)?;
                ps.into_iter().filter(|p| !p.is_empty()).collect()
            }
            t => bail!("{}: geometry {t:?}, not a polygon", at()),
        };
        // The bounds of the shells, as shapely's.
        let mut e = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in parts.iter().flat_map(|p| &p[0]) {
            e = [e[0].min(p[0]), e[1].min(p[1]), e[2].max(p[0]), e[3].max(p[1])];
        }
        if e[2] < b[0] || e[0] > b[2] || e[3] < b[1] || e[1] > b[3] {
            continue;
        }
        let bit = &v["properties"]["bit"];
        let bit = bit.as_i64().or_else(|| bit.as_f64().map(|f| f as i64)).with_context(|| format!("{}: no bit", at()))?;
        read += 1;
        let Some(&bit) = BITS.iter().find(|&&f| f as i64 == bit) else { continue };
        let polys: Vec<Poly> = parts
            .iter()
            .map(|p| {
                p.iter()
                    .map(|r| {
                        let mut m: Vec<[f64; 2]> = r.iter().map(|&q| to_merc(q)).collect();
                        if !is_clockwise(&m) {
                            m.reverse();
                        }
                        m
                    })
                    .collect()
            })
            .collect();
        let mut env = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in polys.iter().flat_map(|p| &p[0]) {
            env = [env[0].min(p[0]), env[1].min(p[1]), env[2].max(p[0]), env[3].max(p[1])];
        }
        out.push(Shape { bit, polys, env });
    }
    Ok((out, read))
}

/// OGR's ring orientation test (OGRLineString::isClockwise, as compiled: the cross product fused,
/// and its fallback's sum vectorised in eights before a fused tail).
pub(crate) fn is_clockwise(p: &[[f64; 2]]) -> bool {
    let n = p.len();
    if n < 2 {
        return true;
    }
    // The lowest rightmost vertex; a repeat of it can't be the pivot.
    let mut fallback = false;
    let mut v = 0;
    for i in 1..n - 1 {
        if p[i][1] < p[v][1] || (p[i][1] == p[v][1] && p[i][0] > p[v][0]) {
            v = i;
            fallback = false;
        } else if p[i][1] == p[v][1] && p[i][0] == p[v][0] {
            fallback = true;
        }
    }
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
    let prev = if v == 0 { n - 2 } else { v - 1 };
    fallback |= near(p[prev], p[v]);
    let (dx0, dy0) = (p[prev][0] - p[v][0], p[prev][1] - p[v][1]);
    let next = if v + 1 >= n - 1 { 0 } else { v + 1 };
    fallback |= near(p[next], p[v]);
    let (dx1, dy1) = (p[next][0] - p[v][0], p[next][1] - p[v][1]);
    let cross = dx1.mul_add(dy0, -(dx0 * dy1));
    if !fallback {
        if cross > 0.0 {
            return false;
        } else if cross < 0.0 {
            return true;
        }
    }
    // Green's formula.
    let mut sum = p[0][0] * (p[1][1] - p[n - 1][1]);
    if n >= 3 {
        let steps = n - 2;
        let vec = if steps <= 8 { 0 } else { steps - if steps % 8 == 0 { 8 } else { steps % 8 } };
        for i in 1..n - 1 {
            let d = p[i + 1][1] - p[i - 1][1];
            sum = if i <= vec { sum + p[i][0] * d } else { p[i][0].mul_add(d, sum) };
        }
    }
    sum = p[n - 1][0].mul_add(p[0][1] - p[n - 2][1], sum);
    sum < 0.0
}

/// The grid's area flags, CELLS bytes per tile in `tiles`' order.
pub fn rasterise(tiles: &[[u32; 2]], shapes: &[Shape]) -> Vec<u8> {
    let mut out = vec![0u8; tiles.len() * CELLS];
    out.par_chunks_mut(CELLS).zip(tiles.par_iter()).for_each_init(Scratch::default, |s, (cells, t)| tile(t[0], t[1], shapes, cells, s));
    out
}

#[derive(Default)]
struct Scratch {
    /// A polygon's points in pixels, its rings one after another, and the rings' lengths.
    pts: Vec<[f64; 2]>,
    sizes: Vec<usize>,
    /// Per scanline of the polygon, the edges (point index pairs) that can cross it.
    rows: Vec<Vec<[u32; 2]>>,
    ints: Vec<i32>,
}

/// Burns the shapes meeting z11 tile (tx, ty) into its cells.
fn tile(tx: u32, ty: u32, shapes: &[Shape], cells: &mut [u8], s: &mut Scratch) {
    let world = 2.0 * PI * R;
    let x0 = tx as f64 / N11 * world - world / 2.0;
    let x1 = (tx + 1) as f64 / N11 * world - world / 2.0;
    let y1 = world / 2.0 - ty as f64 / N11 * world;
    let y0 = world / 2.0 - (ty + 1) as f64 / N11 * world;
    // The pixel transform: rasterio's from_bounds, inverted by GDAL (GDALInvGeoTransform) and
    // applied with fused multiply-adds (GDALGenImgProjTransform).
    let (sx, sy) = ((x1 - x0) / TS as f64, (y0 - y1) / TS as f64);
    let (ix0, ix1, iy0, iy1) = (-x0 / sx, 1.0 / sx, -y1 / sy, 1.0 / sy);
    for sh in shapes {
        let e = sh.env;
        if x0 > e[2] || x1 < e[0] || y0 > e[3] || y1 < e[1] {
            continue;
        }
        for p in &sh.polys {
            s.pts.clear();
            s.sizes.clear();
            for r in p {
                s.pts.extend(r.iter().map(|q| [q[0].mul_add(ix1, ix0), q[1].mul_add(iy1, iy0)]));
                s.sizes.push(r.len());
            }
            let mut burn = |y: i32, xa: i32, xb: i32| {
                let row = &mut cells[y as usize * TS..(y as usize + 1) * TS];
                for c in &mut row[xa.max(0) as usize..=xb.min(TS as i32 - 1) as usize] {
                    *c |= sh.bit;
                }
            };
            if sh.bit == HERITAGE {
                let mut at = 0;
                for &n in &s.sizes {
                    line_all_touched(&s.pts[at..at + n], &mut burn);
                    at += n;
                }
            }
            fill_polygon(s, &mut burn);
        }
    }
}

/// GDAL's scanline fill (GDALdllImageFilledPolygon) of the polygon in `s.pts`: a pixel is burnt when
/// its centre is inside by the even-odd rule; a horizontal edge on a row of centres, walked right to
/// left, burns the centres along it. `burn(y, x0, x1)` gets inclusive runs (x0 ≤ x1 not checked).
fn fill_polygon(s: &mut Scratch, burn: &mut impl FnMut(i32, i32, i32)) {
    let pts = &s.pts;
    let Some(first) = pts.first() else { return };
    let (mut dminy, mut dmaxy) = (first[1], first[1]);
    for p in &pts[1..] {
        if p[1] < dminy {
            dminy = p[1];
        } else if p[1] > dmaxy {
            dmaxy = p[1];
        }
    }
    let miny = dminy.max(0.0) as i32;
    let maxy = (if dmaxy > (TS - 1) as f64 { (TS - 1) as f64 } else { dmaxy }) as i32;
    if miny > maxy {
        return;
    }
    let maxx = TS as i32 - 1;
    // Each edge goes to the rows whose centre line it can reach (row y's is y + 0.5: none below
    // floor(min y) or above ceil(max y)), so a row looks only at its own edges, in the polygon's
    // order as GDAL does; the test below is GDAL's.
    let nrows = (maxy - miny + 1) as usize;
    if s.rows.len() < nrows {
        s.rows.resize_with(nrows, Vec::new);
    }
    for r in &mut s.rows[..nrows] {
        r.clear();
    }
    let mut start = 0;
    for &n in &s.sizes {
        for i in start..start + n {
            let (i1, i2) = if i == start { (start + n - 1, start) } else { (i - 1, i) };
            let (a, b) = (pts[i1][1], pts[i2][1]);
            let lo = a.min(b).floor().max(miny as f64) as i32;
            let hi = a.max(b).ceil().min(maxy as f64) as i32;
            for y in lo..=hi {
                s.rows[(y - miny) as usize].push([i1 as u32, i2 as u32]);
            }
        }
        start += n;
    }
    for y in miny..=maxy {
        let dy = y as f64 + 0.5;
        s.ints.clear();
        for &[i1, i2] in &s.rows[(y - miny) as usize] {
            let (p1, p2) = (pts[i1 as usize], pts[i2 as usize]);
            let (mut dy1, mut dy2) = (p1[1], p2[1]);
            if (dy1 < dy && dy2 < dy) || (dy1 > dy && dy2 > dy) {
                continue;
            }
            let (dx1, dx2);
            if dy1 < dy2 {
                (dx1, dx2) = (p1[0], p2[0]);
            } else if dy1 > dy2 {
                std::mem::swap(&mut dy1, &mut dy2);
                (dx1, dx2) = (p2[0], p1[0]);
            } else {
                // A horizontal edge: burnt when walked right to left (the polygon's bottom).
                if p1[0] > p2[0] {
                    let (h1, h2) = ((p2[0] + 0.5).floor(), (p1[0] + 0.5).floor());
                    if h1 > maxx as f64 || h2 <= 0.0 {
                        continue;
                    }
                    let xa = h1.max(0.0) as i32;
                    let xb = (if h2 > TS as f64 { TS as f64 } else { h2 }) as i32;
                    if xa <= xb - 1 {
                        burn(y, xa, xb - 1);
                    }
                }
                continue;
            }
            if dy < dy2 && dy >= dy1 {
                let x = (dy - dy1) * (dx2 - dx1) / (dy2 - dy1) + dx1;
                let x = if x < i32::MIN as f64 { i32::MIN as f64 } else if x > i32::MAX as f64 { i32::MAX as f64 } else { x };
                s.ints.push((x + 0.5).floor() as i32);
            }
        }
        s.ints.sort_unstable();
        for w in s.ints.chunks_exact(2) {
            if w[0] <= maxx && w[1] > 0 && w[0] <= w[1] - 1 {
                burn(y, w[0], w[1] - 1);
            }
        }
    }
}

/// GDAL's ALL_TOUCHED line drawing for a polygon's ring (GDALdllImageLineAllTouched with
/// bIntersectOnly: an edge along a pixel boundary burns nothing), burning one pixel at a time.
fn line_all_touched(ring: &[[f64; 2]], burn: &mut impl FnMut(i32, i32, i32)) {
    const EPS: f64 = 1e-4;
    let (n, nf) = (TS as i32, TS as f64);
    let in_int = |v: f64| v >= i32::MIN as f64 && v <= i32::MAX as f64;
    for j in 1..ring.len() {
        let ([mut x, mut y], [mut xe, mut ye]) = (ring[j - 1], ring[j]);
        if (y < 0.0 && ye < 0.0) || (y > nf && ye > nf) || (x < 0.0 && xe < 0.0) || (x > nf && xe > nf) {
            continue;
        }
        if !(in_int(x) && in_int(y) && in_int(xe) && in_int(ye)) {
            continue;
        }
        // Left to right.
        if x > xe {
            std::mem::swap(&mut x, &mut xe);
            std::mem::swap(&mut y, &mut ye);
        }
        if (x - xe).abs() < 0.01 {
            // Vertical.
            if (x - x.round()).abs() < EPS && (xe - xe.round()).abs() < EPS {
                continue;
            }
            if ye < y {
                std::mem::swap(&mut y, &mut ye);
            }
            let ix = xe.floor() as i32;
            let (mut iy, mut iye) = (y.floor() as i32, (ye - EPS).floor() as i32);
            if ix < 0 || ix >= n {
                continue;
            }
            iy = iy.max(0);
            iye = iye.min(n - 1);
            while iy <= iye {
                burn(iy, ix, ix);
                iy += 1;
            }
            continue;
        }
        if (y - ye).abs() < 0.01 {
            // Horizontal.
            if (y - y.round()).abs() < EPS && (ye - ye.round()).abs() < EPS {
                continue;
            }
            let (mut ix, iy, mut ixe) = (x.floor() as i32, y.floor() as i32, (xe - EPS).floor() as i32);
            if iy < 0 || iy >= n {
                continue;
            }
            ix = ix.max(0);
            ixe = ixe.min(n - 1);
            if ix <= ixe {
                burn(iy, ix, ixe);
            }
            continue;
        }
        // Sloped: clipped to the raster (fused, as compiled), then stepped pixel by pixel.
        let slope = (ye - y) / (xe - x);
        if xe > nf {
            ye = (-(xe - nf)).mul_add(slope, ye);
            xe = nf;
        }
        if x < 0.0 {
            y = (0.0 - x).mul_add(slope, y);
            x = 0.0;
        }
        if ye > y {
            if y < 0.0 {
                x += (0.0 - y) / slope;
                y = 0.0;
            }
            if ye >= nf {
                xe += (ye - nf) / slope;
                if xe > nf {
                    xe = nf;
                }
            }
        } else {
            if y >= nf {
                x += (nf - y) / slope;
                y = nf;
            }
            if ye < 0.0 {
                xe -= ye / slope;
            }
        }
        while x >= 0.0 && x < xe {
            let (ix, iy) = (x.floor() as i32, y.floor() as i32);
            if iy >= 0 && iy < n {
                burn(iy, ix, ix);
            }
            let sx = (x + 1.0).floor() - x;
            let sy = sx * slope;
            if (y + sy).floor() as i32 == iy {
                x += sx;
                y += sy;
            } else {
                let sy = if slope < 0.0 { (iy as f64 - y).min(-0.000000001) } else { ((iy + 1) as f64 - y).max(0.000000001) };
                x += sy / slope;
                y += sy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A polygon given in pixels of tile (tx, ty) as degrees (so the tile's own transform brings it
    /// back to those pixels, give or take rounding).
    fn deg(tx: u32, ty: u32, px: f64, py: f64) -> [f64; 2] {
        let lon = (tx as f64 + px / 256.0) / N11 * 360.0 - 180.0;
        let lat = (PI * (1.0 - 2.0 * (ty as f64 + py / 256.0) / N11)).dsinh().datan().to_degrees();
        [lon, lat]
    }

    fn feature(bit: u8, polys: &[Vec<Vec<[f64; 2]>>]) -> String {
        let c = |r: &Vec<[f64; 2]>| format!("[{}]", r.iter().map(|p| format!("[{},{}]", p[0], p[1])).collect::<Vec<_>>().join(","));
        let p = |p: &Vec<Vec<[f64; 2]>>| format!("[{}]", p.iter().map(c).collect::<Vec<_>>().join(","));
        let g = if polys.len() == 1 { format!(r#"{{"type":"Polygon","coordinates":{}}}"#, p(&polys[0])) } else { format!(r#"{{"type":"MultiPolygon","coordinates":[{}]}}"#, polys.iter().map(p).collect::<Vec<_>>().join(",")) };
        format!(r#"{{"type":"Feature","geometry":{g},"properties":{{"bit":{bit}}}}}"#)
    }

    fn rect(tx: u32, ty: u32, a: f64, b: f64, c: f64, d: f64) -> Vec<[f64; 2]> {
        vec![deg(tx, ty, a, b), deg(tx, ty, c, b), deg(tx, ty, c, d), deg(tx, ty, a, d), deg(tx, ty, a, b)]
    }

    #[test]
    fn centres_holes_parts_and_touched_pixels() {
        let (tx, ty) = (640, 704);
        let tiles = [[tx, ty], [tx + 1, ty]];
        // A park with a hole; a multipolygon whose parts overlap (each drawn on its own, so the
        // overlap stays burnt); a heritage area burning every pixel it touches.
        let park = vec![rect(tx, ty, 10.2, 10.2, 60.7, 60.7), rect(tx, ty, 20.2, 20.2, 30.7, 30.7)];
        let parts = vec![vec![rect(tx, ty, 100.2, 100.2, 140.7, 140.7)], vec![vec![deg(tx, ty, 120.2, 120.2), deg(tx, ty, 160.7, 120.2), deg(tx, ty, 160.7, 160.7), deg(tx, ty, 120.2, 120.2)]]];
        let her = vec![rect(tx, ty, 200.2, 10.2, 210.7, 20.7)];
        let text = [feature(PARK, &[park]), feature(INDIGENOUS, &parts), feature(HERITAGE, &[her]), feature(PARK, &[vec![rect(tx + 9, ty, 0.0, 0.0, 9.0, 9.0)]])].join("\n");
        let (shapes, read) = read_shapes(&text, grid_box(&tiles)).unwrap();
        assert_eq!((shapes.len(), read), (3, 3), "the polygon away from the grid isn't read");
        let out = rasterise(&tiles, &shapes);
        let at = |x: usize, y: usize| out[y * 256 + x];
        let count = |bit: u8| out[..CELLS].iter().filter(|&&v| v & bit != 0).count();
        // Centres 10.5..60.5 less 20.5..30.5.
        assert_eq!(count(PARK), 51 * 51 - 11 * 11);
        assert_eq!((at(10, 10), at(60, 60), at(25, 25), at(9, 10), at(61, 60)), (PARK, PARK, 0, 0, 0));
        // The square (41 × 41) and the triangle's half beyond it.
        assert_eq!(at(130, 130), INDIGENOUS);
        assert_eq!(at(150, 125), INDIGENOUS);
        assert_eq!(at(125, 150), 0);
        // Centres 200.5..210.5 and 10.5..20.5, and the pixels its edges cross: 200..210 × 10..20.
        assert_eq!(count(HERITAGE), 11 * 11);
        assert_eq!((at(200, 10), at(210, 20), at(211, 15)), (HERITAGE, HERITAGE, 0));
        assert!(out[CELLS..].iter().all(|&v| v == 0), "the next tile is clear");
    }

    #[test]
    fn touched_pixels_of_a_thin_heritage_area() {
        // Thinner than a pixel and off the centres: nothing for a park, its pixels for heritage.
        let (tx, ty) = (1000, 700);
        let r = rect(tx, ty, 50.1, 40.1, 80.3, 40.4);
        let text = [feature(PARK, &[vec![r.clone()]]), feature(HERITAGE, &[vec![r]])].join("\n");
        let (shapes, _) = read_shapes(&text, grid_box(&[[tx, ty]])).unwrap();
        let out = rasterise(&[[tx, ty]], &shapes);
        assert!(out.iter().all(|&v| v & PARK == 0));
        let burnt: Vec<usize> = (0..CELLS).filter(|&i| out[i] == HERITAGE).collect();
        assert_eq!(burnt, (50..=80).map(|x| 40 * 256 + x).collect::<Vec<_>>());
    }

    #[test]
    fn orientation_as_ogr() {
        let cw = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let mut ccw = cw;
        ccw.reverse();
        assert!(is_clockwise(&cw));
        assert!(!is_clockwise(&ccw));
        // A repeated lowest point: the area's sign decides.
        let rep = [[0.0, 0.0], [2.0, 0.0], [1.0, 1.0], [2.0, 0.0], [3.0, 2.0], [-1.0, 2.0], [0.0, 0.0]];
        assert!(!is_clockwise(&rep));
    }

    #[test]
    fn no_tiles_no_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("grid.idx"), b"").unwrap();
        std::fs::write(d.path().join("area-shapes.geojsonseq"), b"").unwrap();
        run(d.path(), &d.path().join("area-shapes.geojsonseq")).unwrap();
        assert!(!d.path().join("grid.areas.u8").exists());
        std::fs::write(d.path().join("grid.idx"), bytemuck::cast_slice(&[640u32, 704])).unwrap();
        run(d.path(), &d.path().join("area-shapes.geojsonseq")).unwrap();
        assert_eq!(std::fs::read(d.path().join("grid.areas.u8")).unwrap(), vec![0u8; CELLS]);
    }
}
