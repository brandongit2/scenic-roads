//! Coverage (docs/plan.md §5): the union of the regions' outlines. A feature is built if it touches
//! the coverage (for a way: any vertex inside, as Geofabrik does).
//!
//! Each outline becomes a [`Shape`]: its rings, a buffer (1 km for `osm:` outlines, so piers and
//! coastal roads on a boundary that follows the coastline are in), and a cell grid over its bounding
//! box. Every cell knows whether its centre is inside and which edges cross it (or pass within the
//! buffer), so a point test reads one cell: the centre's state, flipped by each of the cell's edges
//! crossing the segment from the centre to the point, then the buffer's distance to the cell's
//! edges. Building a grid costs one pass over the edges plus a scanline per row.

use crate::outlines::{inside, Outlines};
use crate::agent::recipes::{parse_outline, Outline, Recipe};
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Metres per E7 unit of latitude (and of longitude at the equator).
const M_PER_E7: f64 = 111_320.0 * 1e-7;

/// One outline of the coverage, indexed for point tests.
pub struct Shape {
    /// The region and outline entry it came from ("borders: osm:1877178").
    pub source: String,
    pub rings: Vec<Vec<[i32; 2]>>,
    pub buffer_m: f64,
    /// Bounding box grown by the buffer (E7).
    pub bbox: [i32; 4],
    grid: Grid,
}

struct Grid {
    x0: f64,
    y0: f64,
    /// Cell size (E7 units).
    cw: f64,
    ch: f64,
    nx: usize,
    ny: usize,
    /// Per cell: the centre inside?
    centre: Vec<bool>,
    /// Per cell: edges (ring, index of first point) crossing it or within the buffer of it.
    edges: Vec<Vec<(u32, u32)>>,
}

impl Shape {
    pub fn new(source: String, rings: Vec<Vec<[i32; 2]>>, buffer_m: f64) -> Shape {
        let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for r in &rings {
            for p in r {
                bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
            }
        }
        // The buffer in E7 units: latitude at the box's widest point.
        let lat = (bb[1].unsigned_abs().max(bb[3].unsigned_abs()) as f64 * 1e-7).min(85.0);
        let bx = (buffer_m / (M_PER_E7 * lat.to_radians().cos())).ceil() as i32;
        let by = (buffer_m / M_PER_E7).ceil() as i32;
        let bbox = [bb[0].saturating_sub(bx), bb[1].saturating_sub(by), bb[2].saturating_add(bx), bb[3].saturating_add(by)];
        let grid = Grid::build(&rings, bbox, bx.max(by) as f64);
        Shape { source, rings, buffer_m, bbox, grid }
    }

    /// Whether `p` (E7) is inside, or within the buffer of the boundary.
    pub fn contains(&self, p: [i32; 2]) -> bool {
        if p[0] < self.bbox[0] || p[0] > self.bbox[2] || p[1] < self.bbox[1] || p[1] > self.bbox[3] {
            return false;
        }
        let g = &self.grid;
        let cx = (((p[0] as f64 - g.x0) / g.cw) as usize).min(g.nx - 1);
        let cy = (((p[1] as f64 - g.y0) / g.ch) as usize).min(g.ny - 1);
        let c = cy * g.nx + cx;
        let centre = [(g.x0 + (cx as f64 + 0.5) * g.cw) as i64, (g.y0 + (cy as f64 + 0.5) * g.ch) as i64];
        let pp = [p[0] as i64, p[1] as i64];
        let mut ins = g.centre[c];
        for &(r, i) in &g.edges[c] {
            let ring = &self.rings[r as usize];
            let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
            if segments_cross(centre, pp, [a[0] as i64, a[1] as i64], [b[0] as i64, b[1] as i64]) {
                ins = !ins;
            }
        }
        if ins {
            return true;
        }
        if self.buffer_m <= 0.0 {
            return false;
        }
        let kx = M_PER_E7 * (p[1] as f64 * 1e-7).to_radians().cos();
        let lim = self.buffer_m * self.buffer_m;
        g.edges[c].iter().any(|&(r, i)| {
            let ring = &self.rings[r as usize];
            let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
            dist2_m(p, a, b, kx) <= lim
        })
    }
}

/// Squared distance (m²) from `p` to the segment a–b, on a local projection (`kx`: metres per E7
/// unit of longitude here).
fn dist2_m(p: [i32; 2], a: [i32; 2], b: [i32; 2], kx: f64) -> f64 {
    let (px, py) = (p[0] as f64 * kx, p[1] as f64 * M_PER_E7);
    let (ax, ay) = (a[0] as f64 * kx, a[1] as f64 * M_PER_E7);
    let (bx, by) = (b[0] as f64 * kx, b[1] as f64 * M_PER_E7);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) };
    (px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)
}

/// Whether segments p1–p2 and q1–q2 cross, counting a touch at q's lower end only (so a ray through
/// a vertex counts once, as in the even–odd test).
fn segments_cross(p1: [i64; 2], p2: [i64; 2], q1: [i64; 2], q2: [i64; 2]) -> bool {
    let o = |a: [i64; 2], b: [i64; 2], c: [i64; 2]| ((b[0] - a[0]) as i128 * (c[1] - a[1]) as i128 - (b[1] - a[1]) as i128 * (c[0] - a[0]) as i128).signum();
    let (d1, d2) = (o(q1, q2, p1), o(q1, q2, p2));
    let (d3, d4) = (o(p1, p2, q1), o(p1, p2, q2));
    // Half-open on q: an endpoint of q exactly on p1–p2 counts when it's q's lower (by y, then x) end.
    let lower = |a: [i64; 2], b: [i64; 2]| (a[1], a[0]) < (b[1], b[0]);
    let q_ok = match (d3, d4) {
        (0, 0) => false,
        (0, _) => lower(q1, q2),
        (_, 0) => lower(q2, q1),
        _ => d3 != d4,
    };
    d1 != d2 && d1 != 0 && d2 != 0 && q_ok
}

impl Grid {
    fn build(rings: &[Vec<[i32; 2]>], bbox: [i32; 4], buf_e7: f64) -> Grid {
        // About 1,024 cells along the longer side, never smaller than twice the buffer.
        let (w, h) = ((bbox[2] as f64 - bbox[0] as f64).max(1.0), (bbox[3] as f64 - bbox[1] as f64).max(1.0));
        let cell = (w.max(h) / 1024.0).max(2.0 * buf_e7).max(100.0);
        let (nx, ny) = (((w / cell).ceil() as usize).clamp(1, 4096), ((h / cell).ceil() as usize).clamp(1, 4096));
        let (cw, ch) = (w / nx as f64, h / ny as f64);
        let (x0, y0) = (bbox[0] as f64, bbox[1] as f64);
        let mut edges: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nx * ny];
        for (ri, r) in rings.iter().enumerate() {
            for i in 0..r.len() {
                let (a, b) = (r[i], r[(i + 1) % r.len()]);
                if a == b {
                    continue;
                }
                // Every cell the edge's box (grown by the buffer) touches: a superset, fine for the
                // crossing test and needed for the buffer.
                let cx0 = ((((a[0].min(b[0]) as f64 - buf_e7) - x0) / cw).floor().max(0.0) as usize).min(nx - 1);
                let cx1 = ((((a[0].max(b[0]) as f64 + buf_e7) - x0) / cw).floor().max(0.0) as usize).min(nx - 1);
                let cy0 = ((((a[1].min(b[1]) as f64 - buf_e7) - y0) / ch).floor().max(0.0) as usize).min(ny - 1);
                let cy1 = ((((a[1].max(b[1]) as f64 + buf_e7) - y0) / ch).floor().max(0.0) as usize).min(ny - 1);
                for cy in cy0..=cy1 {
                    for cx in cx0..=cx1 {
                        edges[cy * nx + cx].push((ri as u32, i as u32));
                    }
                }
            }
        }
        // Each row's centres: crossings of the row's centre line, sorted, give the parity.
        let mut centre = vec![false; nx * ny];
        let mut xs: Vec<f64> = Vec::new();
        for cy in 0..ny {
            let y = y0 + (cy as f64 + 0.5) * ch;
            xs.clear();
            for r in rings {
                for i in 0..r.len() {
                    let (a, b) = (r[i], r[(i + 1) % r.len()]);
                    let (ay, by) = (a[1] as f64, b[1] as f64);
                    if (ay > y) != (by > y) {
                        xs.push(a[0] as f64 + (y - ay) * (b[0] as f64 - a[0] as f64) / (by - ay));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            let mut k = 0;
            for cx in 0..nx {
                let x = x0 + (cx as f64 + 0.5) * cw;
                while k < xs.len() && xs[k] < x {
                    k += 1;
                }
                centre[cy * nx + cx] = k % 2 == 1;
            }
        }
        Grid { x0, y0, cw, ch, nx, ny, centre, edges }
    }
}

/// The coverage: every region's outlines.
#[derive(Default)]
pub struct Coverage {
    pub shapes: Vec<Shape>,
}

/// The buffer around `osm:` outlines (coasts), metres.
pub const OSM_BUFFER_M: f64 = 1000.0;

/// Reads a `.poly` file (Osmosis polygon format): rings, with `!`-prefixed sections as holes.
pub fn read_poly(text: &str) -> Result<Vec<Vec<[i32; 2]>>> {
    let mut rings = Vec::new();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.next().context("empty .poly")?; // its name
    while let Some(l) = lines.next() {
        if l == "END" {
            break;
        }
        let mut ring = Vec::new();
        for l in lines.by_ref() {
            if l == "END" {
                break;
            }
            let v: Vec<f64> = l.split_whitespace().map(|x| x.parse()).collect::<Result<_, _>>().with_context(|| format!("bad .poly line {l:?}"))?;
            if v.len() < 2 {
                bail!("bad .poly line {l:?}");
            }
            ring.push([(v[0] * 1e7).round() as i32, (v[1] * 1e7).round() as i32]);
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    Ok(rings)
}

/// A circle as a 64-gon.
fn circle(lon: f64, lat: f64, km: f64) -> Vec<[i32; 2]> {
    let dy = km * 1000.0 / 110_574.0;
    let dx = km * 1000.0 / (111_320.0 * lat.to_radians().cos().max(0.01));
    (0..64)
        .map(|i| {
            let t = i as f64 / 64.0 * std::f64::consts::TAU;
            [((lon + dx * t.cos()) * 1e7).round() as i32, ((lat + dy * t.sin()).clamp(-89.9, 89.9) * 1e7).round() as i32]
        })
        .collect()
}

impl Coverage {
    /// The coverage of `recipes`: `osm:` outlines from `outlines`, `poly:` and Geofabrik outlines
    /// from `outline_dir` (`inputs/outlines/`, Geofabrik's as `geofabrik/<id>.poly`), circles for
    /// places. An entry that can't be resolved is an error naming it.
    pub fn from_recipes(recipes: &[Recipe], outlines: Option<&Outlines>, outline_dir: &Path) -> Result<Coverage> {
        let mut shapes = Vec::new();
        for r in recipes {
            for entry in &r.outline {
                let source = format!("{}: {entry}", r.id);
                let shape = match parse_outline(entry)? {
                    Outline::Osm(id) => {
                        let o = outlines.context("no outlines yet (the OSM pass makes them)")?;
                        let rec = o.by_id(id).with_context(|| format!("{source}: relation {id} isn't an administrative or ISO 3166 outline of this pass"))?;
                        Shape::new(source, o.rings(rec)?, OSM_BUFFER_M)
                    }
                    Outline::Geofabrik(id) => {
                        let p = outline_dir.join("geofabrik").join(format!("{}.poly", id.replace('/', "-")));
                        let text = std::fs::read_to_string(&p).with_context(|| format!("{source}: {} (fetched when the region is added)", p.display()))?;
                        Shape::new(source, read_poly(&text)?, 0.0)
                    }
                    Outline::Poly(f) => {
                        let text = std::fs::read_to_string(outline_dir.join(&f)).with_context(|| format!("{source}: inputs/outlines/{f}"))?;
                        Shape::new(source, read_poly(&text)?, 0.0)
                    }
                    Outline::Place { lon, lat, km } => Shape::new(source, vec![circle(lon, lat, km)], 0.0),
                };
                shapes.push(shape);
            }
        }
        Ok(Coverage { shapes })
    }

    pub fn contains(&self, p: [i32; 2]) -> bool {
        self.shapes.iter().any(|s| s.contains(p))
    }

    /// Whether any shape's (buffered) box meets the box w, s, e, n (E7): a quick filter.
    pub fn meets_box(&self, b: [i32; 4]) -> bool {
        self.shapes.iter().any(|s| s.bbox[0] <= b[2] && s.bbox[2] >= b[0] && s.bbox[1] <= b[3] && s.bbox[3] >= b[1])
    }

    /// Whether a way (its vertices) touches the coverage.
    pub fn touches(&self, verts: &[[i32; 2]]) -> bool {
        verts.iter().any(|&p| self.contains(p))
    }
}

/// A plain even–odd test, for checking the grid.
pub fn inside_plain(rings: &[Vec<[i32; 2]>], p: [i32; 2]) -> bool {
    let r: Vec<&[[i32; 2]]> = rings.iter().map(Vec::as_slice).collect();
    inside(&r, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e7(x: f64, y: f64) -> [i32; 2] {
        [(x * 1e7).round() as i32, (y * 1e7).round() as i32]
    }

    /// A star-ish polygon with a hole, its points on a coarse lattice so tests hit vertices and
    /// edges exactly too.
    fn shape_rings() -> Vec<Vec<[i32; 2]>> {
        let outer: Vec<[i32; 2]> = (0..40)
            .map(|i| {
                let t = i as f64 / 40.0 * std::f64::consts::TAU;
                let r = if i % 2 == 0 { 1.0 } else { 0.6 };
                e7(-2.0 + r * t.cos(), 55.0 + r * t.sin() * 0.6)
            })
            .collect();
        let hole = vec![e7(-2.1, 54.95), e7(-1.9, 54.95), e7(-1.9, 55.05), e7(-2.1, 55.05)];
        vec![outer, hole]
    }

    #[test]
    fn grid_agrees_with_plain_test() {
        let rings = shape_rings();
        let s = Shape::new("t".into(), rings.clone(), 0.0);
        let mut n = 0;
        // Pseudo-random points over the box and a lattice through vertices.
        let mut seed = 12345u64;
        for _ in 0..20000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = -3.2 + (seed >> 33) as f64 / (1u64 << 31) as f64 * 2.4;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let y = 54.2 + (seed >> 33) as f64 / (1u64 << 31) as f64 * 1.6;
            let p = e7(x, y);
            assert_eq!(s.contains(p), inside_plain(&rings, p), "at {x},{y}");
            n += inside_plain(&rings, p) as usize;
        }
        assert!(n > 1000, "{n} inside");
        for r in &rings {
            for &p in r {
                // On the boundary either answer is fine; it must not panic.
                let _ = s.contains(p);
            }
        }
    }

    #[test]
    fn buffer_reaches_a_kilometre() {
        let rings = vec![vec![e7(0.0, 50.0), e7(1.0, 50.0), e7(1.0, 51.0), e7(0.0, 51.0)]];
        let s = Shape::new("t".into(), rings, 1000.0);
        // 0.009° of latitude ≈ 1.0 km: just inside the buffer north of the top edge; 0.011° outside.
        assert!(s.contains(e7(0.5, 51.0085)));
        assert!(!s.contains(e7(0.5, 51.011)));
        // West of the west edge at 50.5° N, a km is 0.01412° of longitude.
        assert!(s.contains(e7(-0.0135, 50.5)));
        assert!(!s.contains(e7(-0.0150, 50.5)));
        assert!(s.contains(e7(0.5, 50.5)));
    }

    #[test]
    fn poly_files() {
        let text = "test\n1\n  0.0 50.0\n  1.0 50.0\n  1.0 51.0\n  0.0 51.0\nEND\n!2\n  0.4 50.4\n  0.6 50.4\n  0.6 50.6\n  0.4 50.6\nEND\nEND\n";
        let rings = read_poly(text).unwrap();
        assert_eq!(rings.len(), 2);
        let s = Shape::new("t".into(), rings, 0.0);
        assert!(s.contains(e7(0.2, 50.2)));
        assert!(!s.contains(e7(0.5, 50.5)));
    }

    #[test]
    fn coverage_from_recipes() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("box.poly"), "b\n1\n 10 10\n 11 10\n 11 11\n 10 11\nEND\nEND\n").unwrap();
        let recipes = vec![
            Recipe { id: "a".into(), name: "A".into(), outline: vec!["poly:box.poly".into(), "place:20,20,10".into()] },
        ];
        let c = Coverage::from_recipes(&recipes, None, d.path()).unwrap();
        assert!(c.contains(e7(10.5, 10.5)));
        assert!(c.contains(e7(20.05, 20.05)));
        assert!(!c.contains(e7(20.2, 20.0)));
        assert!(c.touches(&[e7(0.0, 0.0), e7(10.1, 10.1)]));
        let bad = vec![Recipe { id: "b".into(), name: "B".into(), outline: vec!["osm:1".into()] }];
        assert!(Coverage::from_recipes(&bad, None, d.path()).is_err());
    }
}
