//! Coverage (docs/plan.md §5): the union of the regions' outlines. A feature is built if it touches
//! the coverage (for a way: any vertex inside, as Geofabrik does).
//!
//! Each outline becomes a [`Shape`]: its rings, a buffer (1 km for `osm:` outlines, so piers and
//! coastal roads on a boundary that follows the coastline are in), and a cell grid over its bounding
//! box. Every cell knows whether its centre is inside and which edges cross it (or pass within the
//! buffer), so a point test reads one cell: the centre's state, flipped by each of the cell's edges
//! crossing the segment from the centre to the point, then the buffer's distance to the cell's
//! edges. Building a grid costs one pass over the edges plus a scanline per row.
//!
//! A catalog records the coverage it was built for, simplified for drawing ([`DrawnRegion`]), so
//! the map draws it from the catalog rather than from the recipes.

use det::Det;
use crate::outlines::{inside, simplify, Outlines};
use crate::agent::recipes::{parse_outline, Outline, Recipe};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Metres per E7 unit of latitude (and of longitude at the equator).
const M_PER_E7: f64 = 111_320.0 * 1e-7;

/// One outline of the coverage, indexed for point tests.
#[derive(Clone)]
pub struct Shape {
    /// The region and outline entry it came from ("borders: osm:1877178").
    pub source: String,
    pub rings: Vec<Vec<[i32; 2]>>,
    pub buffer_m: f64,
    /// Bounding box grown by the buffer (E7).
    pub bbox: [i32; 4],
    grid: Grid,
}

#[derive(Clone)]
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
        let bx = (buffer_m / (M_PER_E7 * lat.to_radians().dcos())).ceil() as i32;
        let by = (buffer_m / M_PER_E7).ceil() as i32;
        let bbox = [bb[0].saturating_sub(bx), bb[1].saturating_sub(by), bb[2].saturating_add(bx), bb[3].saturating_add(by)];
        let grid = Grid::build(&rings, bbox, bx.max(by) as f64);
        Shape { source, rings, buffer_m, bbox, grid }
    }

    /// The grid cell of `p`, which must be inside the box.
    fn cell(&self, p: [i32; 2]) -> usize {
        let g = &self.grid;
        let cx = (((p[0] as f64 - g.x0) / g.cw) as usize).min(g.nx - 1);
        let cy = (((p[1] as f64 - g.y0) / g.ch) as usize).min(g.ny - 1);
        cy * g.nx + cx
    }

    /// Whether `p` (E7) is inside the rings (even–odd), the buffer aside.
    fn inside(&self, p: [i32; 2]) -> bool {
        if p[0] < self.bbox[0] || p[0] > self.bbox[2] || p[1] < self.bbox[1] || p[1] > self.bbox[3] {
            return false;
        }
        let g = &self.grid;
        let c = self.cell(p);
        let (cx, cy) = (c % g.nx, c / g.nx);
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
        ins
    }

    /// Whether `p` (E7) is inside, or within the buffer of the boundary.
    pub fn contains(&self, p: [i32; 2]) -> bool {
        if p[0] < self.bbox[0] || p[0] > self.bbox[2] || p[1] < self.bbox[1] || p[1] > self.bbox[3] {
            return false;
        }
        if self.inside(p) {
            return true;
        }
        if self.buffer_m <= 0.0 {
            return false;
        }
        let (g, c) = (&self.grid, self.cell(p));
        let kx = M_PER_E7 * (p[1] as f64 * 1e-7).to_radians().dcos();
        let lim = self.buffer_m * self.buffer_m;
        g.edges[c].iter().any(|&(r, i)| {
            let ring = &self.rings[r as usize];
            let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
            dist2_m(p, a, b, kx) <= lim
        })
    }

    /// Whether the box w, s, e, n (E7, edges included) meets the shape or its buffer (the buffer
    /// as a box around the box: up to √2 × the buffer at its corners). Exact otherwise: an edge
    /// meeting the box, else the box wholly inside or wholly outside, which one point tells.
    pub fn meets_rect(&self, r: [i32; 4]) -> bool {
        if r[0] > self.bbox[2] || r[2] < self.bbox[0] || r[1] > self.bbox[3] || r[3] < self.bbox[1] {
            return false;
        }
        if self.edges_meeting(self.grown(r), &mut |_, _, _| true) {
            return true;
        }
        // No edge meets it: the box is all inside or all outside.
        self.contains([((r[0] as i64 + r[2] as i64) / 2) as i32, ((r[1] as i64 + r[3] as i64) / 2) as i32])
    }

    /// The box w, s, e, n (E7) grown by the buffer, at its latitude furthest from the equator.
    fn grown(&self, r: [i32; 4]) -> [i64; 4] {
        let lat = (r[1].unsigned_abs().max(r[3].unsigned_abs()) as f64 * 1e-7).min(85.0);
        let (bx, by) = if self.buffer_m > 0.0 { ((self.buffer_m / (M_PER_E7 * lat.to_radians().dcos())).ceil() as i64, (self.buffer_m / M_PER_E7).ceil() as i64) } else { (0, 0) };
        [r[0] as i64 - bx, r[1] as i64 - by, r[2] as i64 + bx, r[3] as i64 + by]
    }

    /// Calls `f` with each ring edge meeting the box `rg`, with its place (ring, index), from the
    /// grid cells the box overlaps (so an edge crossing several cells comes several times), until it
    /// returns true; whether one did.
    fn edges_meeting(&self, rg: [i64; 4], f: &mut dyn FnMut((u32, u32), [i32; 2], [i32; 2]) -> bool) -> bool {
        let g = &self.grid;
        let cell = |v: f64, v0: f64, size: f64, n: usize| (((v - v0) / size).floor().max(0.0) as usize).min(n - 1);
        let (cx0, cx1) = (cell(rg[0] as f64, g.x0, g.cw, g.nx), cell(rg[2] as f64, g.x0, g.cw, g.nx));
        let (cy0, cy1) = (cell(rg[1] as f64, g.y0, g.ch, g.ny), cell(rg[3] as f64, g.y0, g.ch, g.ny));
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                for &(ri, i) in &g.edges[cy * g.nx + cx] {
                    let ring = &self.rings[ri as usize];
                    let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
                    if segment_meets_box([a[0] as i64, a[1] as i64], [b[0] as i64, b[1] as i64], rg) && f((ri, i), a, b) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The shape as it is inside the box w, s, e, n (E7): whether a reference point of the box (its
    /// south-west corner, or the first point up its diagonal on no edge) is inside the rings, and
    /// every ring edge meeting the box grown by the buffer, each as often as the rings hold it. That
    /// decides which points of the box the shape contains: a point's inside-ness is the reference
    /// point's, flipped by the edges crossing the line between them (inside the box), and its buffer
    /// reads the edges within the buffer of it. None when the shape has nothing in the box.
    fn fingerprint(&self, r: [i32; 4]) -> Option<String> {
        let rg = self.grown(r);
        if rg[0] > self.bbox[2] as i64 || rg[2] < self.bbox[0] as i64 || rg[1] > self.bbox[3] as i64 || rg[3] < self.bbox[1] as i64 {
            return None;
        }
        // Each edge once per place in the rings (a cell grid lists an edge in every cell it crosses);
        // an edge the rings hold twice stays twice, since under even–odd the two cancel.
        let mut by_place: std::collections::BTreeMap<(u32, u32), [i32; 4]> = std::collections::BTreeMap::new();
        self.edges_meeting(rg, &mut |at, a, b| {
            // (Direction doesn't matter to which points are inside.)
            by_place.insert(at, if (a[0], a[1]) <= (b[0], b[1]) { [a[0], a[1], b[0], b[1]] } else { [b[0], b[1], a[0], a[1]] });
            false
        });
        let mut edges: Vec<[i32; 4]> = by_place.into_values().collect();
        edges.sort_unstable();
        // A reference point on no edge, so its inside-ness doesn't depend on how the grid's cells
        // fall (on an edge, either answer would be right, and the cell decides).
        let on_edge = |p: [i32; 2]| {
            edges.iter().any(|e| {
                let (a, b, p) = ([e[0] as i64, e[1] as i64], [e[2] as i64, e[3] as i64], [p[0] as i64, p[1] as i64]);
                (b[0] - a[0]) as i128 * (p[1] - a[1]) as i128 == (b[1] - a[1]) as i128 * (p[0] - a[0]) as i128 && p[0] >= a[0].min(b[0]) && p[0] <= a[0].max(b[0]) && p[1] >= a[1].min(b[1]) && p[1] <= a[1].max(b[1])
            })
        };
        let mut reference = [r[0], r[1]];
        while on_edge(reference) && reference[0] < r[2] && reference[1] < r[3] {
            reference = [reference[0] + 1, reference[1] + 1];
        }
        let inside = self.inside(reference);
        if edges.is_empty() && !inside {
            return None;
        }
        Some(format!("{}:{}:{}", self.buffer_m, inside as u8, store::naming::hash16(bytemuck::cast_slice(&edges))))
    }
}

/// Whether segment a–b meets the box w, s, e, n (edges included).
fn segment_meets_box(a: [i64; 2], b: [i64; 2], r: [i64; 4]) -> bool {
    let inside = |p: [i64; 2]| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3];
    if inside(a) || inside(b) {
        return true;
    }
    if a[0].max(b[0]) < r[0] || a[0].min(b[0]) > r[2] || a[1].max(b[1]) < r[1] || a[1].min(b[1]) > r[3] {
        return false;
    }
    // Both ends outside, boxes overlapping: it meets the box when the box's corners aren't all on
    // one side of its line.
    let side = |p: [i64; 2]| ((b[0] - a[0]) as i128 * (p[1] - a[1]) as i128 - (b[1] - a[1]) as i128 * (p[0] - a[0]) as i128).signum();
    let s: Vec<i128> = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]].iter().map(|&p| side(p)).collect();
    !(s.iter().all(|&v| v > 0) || s.iter().all(|&v| v < 0))
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

/// Whether the segment p1–p2 (from a grid cell's centre to a point) crosses the edge q1–q2, for the
/// even–odd test. An end of the edge lying on the segment's line counts as being on its left, a
/// fixed side whatever the segment's direction: a ring passing through a vertex on the line is
/// counted once, and one touching it not at all. (A point exactly on an edge is on the boundary,
/// where either answer is right.)
fn segments_cross(p1: [i64; 2], p2: [i64; 2], q1: [i64; 2], q2: [i64; 2]) -> bool {
    let o = |a: [i64; 2], b: [i64; 2], c: [i64; 2]| ((b[0] - a[0]) as i128 * (c[1] - a[1]) as i128 - (b[1] - a[1]) as i128 * (c[0] - a[0]) as i128).signum();
    let side = |q: [i64; 2]| if o(p1, p2, q) < 0 { -1 } else { 1 };
    if side(q1) == side(q2) {
        return false;
    }
    let (d1, d2) = (o(q1, q2, p1), o(q1, q2, p2));
    d1 != 0 && d2 != 0 && d1 != d2
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
#[derive(Clone, Default)]
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
    let dx = km * 1000.0 / (111_320.0 * lat.to_radians().dcos().max(0.01));
    (0..64)
        .map(|i| {
            let t = i as f64 / 64.0 * std::f64::consts::TAU;
            [((lon + dx * t.dcos()) * 1e7).round() as i32, ((lat + dy * t.dsin()).clamp(-89.9, 89.9) * 1e7).round() as i32]
        })
        .collect()
}

/// The rings of an outline entry that isn't `osm:` (those are the pass's): a Geofabrik or drawn
/// `.poly` in `outline_dir`, or a place's circle.
fn file_rings(o: &Outline, outline_dir: &Path) -> Result<Vec<Vec<[i32; 2]>>> {
    match o {
        Outline::Geofabrik(id) => {
            let p = outline_dir.join("geofabrik").join(format!("{}.poly", id.replace('/', "-")));
            read_poly(&std::fs::read_to_string(&p).with_context(|| format!("{} (fetched when the region is added)", p.display()))?)
        }
        Outline::Poly(f) => read_poly(&std::fs::read_to_string(outline_dir.join(f)).with_context(|| format!("inputs/outlines/{f}"))?),
        Outline::Place { lon, lat, km } => Ok(vec![circle(*lon, *lat, *km)]),
        Outline::Osm(id) => bail!("osm:{id} is one of the pass's outlines"),
    }
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
                    other => {
                        let rings = file_rings(&other, outline_dir).with_context(|| source.clone())?;
                        Shape::new(source, rings, 0.0)
                    }
                };
                shapes.push(shape);
            }
        }
        Ok(Coverage { shapes })
    }

    /// Each region's own coverage (its shapes, as `from_recipes` made them for it: "<id>: <entry>"),
    /// in the order its first shape comes: no outline read again.
    pub fn by_region(&self) -> Vec<(String, Coverage)> {
        let mut out: Vec<(String, Coverage)> = Vec::new();
        for s in &self.shapes {
            let id = s.source.split_once(": ").map_or(s.source.as_str(), |(id, _)| id);
            match out.iter_mut().find(|(r, _)| r == id) {
                Some((_, c)) => c.shapes.push(s.clone()),
                None => out.push((id.to_string(), Coverage { shapes: vec![s.clone()] })),
            }
        }
        out
    }

    pub fn contains(&self, p: [i32; 2]) -> bool {
        self.shapes.iter().any(|s| s.contains(p))
    }

    /// Whether any shape's (buffered) box meets the box w, s, e, n (E7): a quick filter.
    pub fn meets_box(&self, b: [i32; 4]) -> bool {
        self.shapes.iter().any(|s| s.bbox[0] <= b[2] && s.bbox[2] >= b[0] && s.bbox[1] <= b[3] && s.bbox[3] >= b[1])
    }

    /// Whether the box w, s, e, n (E7) meets the coverage itself (`Shape::meets_rect`).
    pub fn meets_rect(&self, b: [i32; 4]) -> bool {
        self.shapes.iter().any(|s| s.meets_rect(b))
    }

    /// The coverage as it is inside the box w, s, e, n (E7): its shapes' fingerprints there
    /// (`Shape::fingerprint`), each once, by geometry alone (not which region a shape came from).
    /// Equal fingerprints mean the same points of the box are covered, so a job keyed on the box it
    /// reads reruns only when the coverage changes there.
    pub fn fingerprint(&self, b: [i32; 4]) -> String {
        let mut v: Vec<String> = self.shapes.iter().filter_map(|s| s.fingerprint(b)).collect();
        v.sort();
        v.dedup();
        v.join(",")
    }

    /// Whether a way (its vertices) touches the coverage.
    pub fn touches(&self, verts: &[[i32; 2]]) -> bool {
        verts.iter().any(|&p| self.contains(p))
    }
}

// ---- the coverage a catalog records ------------------------------------------------------------

/// A region as a catalog records it (docs/formats.md, Catalog `coverage`): its recipe when the
/// catalog was made, and each outline entry's polygons simplified for drawing, as GeoJSON
/// MultiPolygon coordinates (degrees to 5 decimals, about a metre; rings closed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawnRegion {
    pub id: String,
    pub name: String,
    pub outline: Vec<String>,
    /// By outline entry; an entry that couldn't be read then has none.
    #[serde(default)]
    pub shapes: BTreeMap<String, Vec<Vec<Vec<[f64; 2]>>>>,
}

impl DrawnRegion {
    /// Whether its outlines meet the box w, s, e, n (E7, edges included): an edge meeting the box,
    /// else the box wholly inside an outline (even–odd), which its centre tells.
    pub fn meets_rect(&self, r: [i32; 4]) -> bool {
        let rb = [r[0] as i64, r[1] as i64, r[2] as i64, r[3] as i64];
        let (cx, cy) = ((rb[0] + rb[2]) as f64 / 2.0, (rb[1] + rb[3]) as f64 / 2.0);
        let e7 = |p: [f64; 2]| [(p[0] * 1e7).round() as i64, (p[1] * 1e7).round() as i64];
        self.shapes.values().any(|polygons| {
            let mut inside = false;
            for ring in polygons.iter().flatten() {
                for i in 0..ring.len() {
                    let (a, b) = (e7(ring[i]), e7(ring[(i + 1) % ring.len()]));
                    if segment_meets_box(a, b, rb) {
                        return true;
                    }
                    let (ay, by) = (a[1] as f64, b[1] as f64);
                    if (ay > cy) != (by > cy) && cx < (b[0] - a[0]) as f64 * (cy - ay) / (by - ay) + a[0] as f64 {
                        inside = !inside;
                    }
                }
            }
            inside
        })
    }
}

/// Every region's outlines for drawing: `osm:` entries as the pass simplified them for the Regions
/// panel, the others simplified here alike, by size (`draw_tolerance_m`). Each entry is read on its
/// own, so one that can't be (a relation the pass lacks, a missing `.poly`) loses only its shape,
/// with a note: the regions are still recorded. A read that fails (the NAS) fails the whole, so
/// nothing records a region without its outline.
pub fn drawn(recipes: &[Recipe], outlines: Option<&Outlines>, outline_dir: &Path) -> Result<Vec<DrawnRegion>> {
    recipes
        .iter()
        .map(|r| {
            let mut shapes = BTreeMap::new();
            for entry in &r.outline {
                match drawn_entry(entry, outlines, outline_dir) {
                    Ok(polygons) => {
                        shapes.insert(entry.clone(), polygons);
                    }
                    Err(e) if failed_read(&e) => return Err(e.context(format!("{}: {entry}", r.id))),
                    Err(e) => eprintln!("coverage: {}: {entry}: {e:#}", r.id),
                }
            }
            Ok(DrawnRegion { id: r.id.clone(), name: r.name.clone(), outline: r.outline.clone(), shapes })
        })
        .collect()
}

/// Whether `e` is a read that failed (worth trying again), not a file that isn't there.
pub fn failed_read(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() != std::io::ErrorKind::NotFound))
}

/// One outline entry's polygons for drawing, in degrees. A `.poly`'s or a circle's rings are each a
/// polygon of their own, as `/api/coverage` always gave them (the app nests holes by containment).
fn drawn_entry(entry: &str, outlines: Option<&Outlines>, outline_dir: &Path) -> Result<Vec<Vec<Vec<[f64; 2]>>>> {
    let polygons = match parse_outline(entry)? {
        Outline::Osm(id) => {
            let o = outlines.context("no outlines yet (the OSM pass makes them)")?;
            let rec = o.by_id(id).with_context(|| format!("relation {id} isn't an administrative or ISO 3166 outline of the pass"))?;
            o.simple_polygons(rec)?
        }
        other => {
            let rings = file_rings(&other, outline_dir)?;
            let tol = draw_tolerance_m(&rings);
            rings.iter().map(|r| vec![simplify(r, tol)]).collect()
        }
    };
    Ok(polygons.iter().map(|p| p.iter().map(|r| ring_degrees(r)).collect()).collect())
}

/// How far a drawn outline may stray from its file's: about a pixel when the whole outline fills a
/// screen, within the pass's own range for the panel (1 km for countries, 60 m for the smallest
/// areas).
fn draw_tolerance_m(rings: &[Vec<[i32; 2]>]) -> f64 {
    let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for p in rings.iter().flatten() {
        bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
    }
    if bb[0] > bb[2] {
        return 60.0;
    }
    let lat = ((bb[1] as f64 + bb[3] as f64) / 2.0 * 1e-7).to_radians();
    let w = (bb[2] as f64 - bb[0] as f64) * M_PER_E7 * lat.dcos();
    let h = (bb[3] as f64 - bb[1] as f64) * M_PER_E7;
    (w.max(h) / 2000.0).clamp(60.0, 1000.0)
}

/// A ring in degrees to 5 decimals, closed as GeoJSON has it.
fn ring_degrees(r: &[[i32; 2]]) -> Vec<[f64; 2]> {
    let d = |v: i32| (v as f64 / 100.0).round() / 1e5;
    let mut out: Vec<[f64; 2]> = r.iter().map(|p| [d(p[0]), d(p[1])]).collect();
    if out.len() > 1 && out.first() != out.last() {
        out.push(out[0]);
    }
    out
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
                e7(-2.0 + r * t.dcos(), 55.0 + r * t.dsin() * 0.6)
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
        let missing = vec![Recipe { id: "c".into(), name: "C".into(), outline: vec!["geofabrik:europe/gone".into()] }];
        let e = format!("{:#}", Coverage::from_recipes(&missing, None, d.path()).err().unwrap());
        assert!(e.starts_with("c: geofabrik:europe/gone: ") && e.contains("geofabrik/europe-gone.poly (fetched when the region is added)"), "{e}");
    }

    #[test]
    fn drawn_as_a_catalog_records_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("box.poly"), "b\n1\n 10 10\n 11 10\n 11 11\n 10 11\nEND\nEND\n").unwrap();
        let outline: Vec<String> = ["poly:box.poly", "place:20,20,10", "osm:1", "poly:gone.poly"].map(String::from).to_vec();
        let recipes = vec![Recipe { id: "a".into(), name: "A".into(), outline: outline.clone() }];
        let r = &drawn(&recipes, None, d.path()).unwrap()[0];
        // The recipe as it is; the entries that can't be read (no outlines yet, no file) have no shape.
        assert_eq!((r.id.as_str(), r.name.as_str(), &r.outline), ("a", "A", &outline));
        assert_eq!(r.shapes.keys().collect::<Vec<_>>(), ["place:20,20,10", "poly:box.poly"]);
        // In degrees, rings closed.
        assert_eq!(r.shapes["poly:box.poly"], vec![vec![vec![[10.0, 10.0], [11.0, 10.0], [11.0, 11.0], [10.0, 11.0], [10.0, 10.0]]]]);
        let circle = &r.shapes["place:20,20,10"][0][0];
        assert!(circle.len() > 16 && circle.first() == circle.last(), "{}", circle.len());
        // Boxes meeting it: across an edge, wholly inside, outside; the circle.
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        assert!(r.meets_rect(b(10.9, 10.4, 11.2, 10.6)));
        assert!(r.meets_rect(b(10.4, 10.4, 10.6, 10.6)), "wholly inside");
        assert!(!r.meets_rect(b(11.1, 10.4, 11.2, 10.6)));
        assert!(r.meets_rect(b(19.99, 19.99, 20.01, 20.01)) && !r.meets_rect(b(20.2, 20.2, 20.3, 20.3)));
        // Stored and read back unchanged.
        let back: DrawnRegion = serde_json::from_value(serde_json::to_value(r).unwrap()).unwrap();
        assert_eq!(&back, r);
        // Simplified by size: a country's outline by up to a kilometre, a town's by 60 m.
        assert_eq!(draw_tolerance_m(&[vec![e7(0.0, 40.0), e7(30.0, 40.0), e7(30.0, 60.0)]]), 1000.0);
        assert_eq!(draw_tolerance_m(&[vec![e7(0.0, 40.0), e7(0.01, 40.0), e7(0.01, 40.01)]]), 60.0);
    }

    #[test]
    fn each_regions_coverage_is_its_own_shapes() {
        let d = tempfile::tempdir().unwrap();
        let rs = vec![
            Recipe { id: "a".into(), name: "A".into(), outline: vec!["place:20,20,10".into(), "place:21,20,10".into()] },
            Recipe { id: "b".into(), name: "B".into(), outline: vec!["place:40,40,10".into()] },
        ];
        let all = Coverage::from_recipes(&rs, None, d.path()).unwrap();
        let each = all.by_region();
        assert_eq!(each.iter().map(|(id, c)| (id.as_str(), c.shapes.len())).collect::<Vec<_>>(), [("a", 2), ("b", 1)]);
        // The same as made from each recipe alone.
        for (r, (_, c)) in rs.iter().zip(&each) {
            let alone = Coverage::from_recipes(std::slice::from_ref(r), None, d.path()).unwrap();
            let b = [190_000_000, 190_000_000, 420_000_000, 420_000_000];
            assert_eq!(c.fingerprint(b), alone.fingerprint(b));
            assert_eq!(c.contains([200_000_000, 200_000_000]), alone.contains([200_000_000, 200_000_000]));
        }
    }

    #[test]
    fn fingerprints_see_only_their_box() {
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        let quad = |south: f64, north: f64| vec![vec![e7(0.0, south), e7(1.0, south), e7(1.0, north), e7(0.0, north)]];
        let fp = |rings: Vec<Vec<[i32; 2]>>, bx: [i32; 4]| Coverage { shapes: vec![Shape::new("r: poly:x".into(), rings, 0.0)] }.fingerprint(bx);
        // A box across the south edge.
        let across = b(0.2, 49.9, 0.4, 50.1);
        let f0 = fp(quad(50.0, 51.0), across);
        assert!(!f0.is_empty());
        assert_eq!(fp(quad(50.0, 51.3), across), f0, "the north edge moved, far from the box");
        assert_ne!(fp(quad(50.05, 51.0), across), f0, "the south edge moved through the box");
        // A box wholly inside: covered, whatever the edges far away do; wholly outside: nothing.
        let inner = b(0.4, 50.4, 0.6, 50.6);
        assert!(!fp(quad(50.0, 51.0), inner).is_empty());
        assert_eq!(fp(quad(50.0, 51.0), inner), fp(quad(49.0, 52.0), inner));
        assert_eq!(fp(quad(50.0, 51.0), b(2.0, 50.4, 2.2, 50.6)), "");
        // Same geometry from another region or entry: the same fingerprint.
        let two = Coverage { shapes: vec![Shape::new("a: poly:x".into(), quad(50.0, 51.0), 0.0), Shape::new("b: osm:1".into(), quad(50.0, 51.0), 0.0)] };
        assert_eq!(two.fingerprint(across), f0);
        // The buffer: an edge 0.5 km outside the box is within a 1 km buffer of it.
        let near = b(0.2, 49.9, 0.4, 49.995);
        let buffered = |south: f64| Coverage { shapes: vec![Shape::new("o".into(), quad(south, 51.0), 1000.0)] }.fingerprint(near);
        assert_ne!(buffered(50.0), buffered(50.001), "the edge moved within the buffer's reach");
    }

    #[test]
    fn a_line_through_a_vertex_crosses_once() {
        // The review's case: from (0, -5) up to (0, 5), through the vertex (0, 0) of a ring
        // ...(-1, 1) -> (0, 0) -> (1, 1)...: one crossing; a ring touching the line there, none.
        let (p1, p2) = ([0, -5], [0, 5]);
        let through = [segments_cross(p1, p2, [-1, 1], [0, 0]), segments_cross(p1, p2, [0, 0], [1, 1])];
        assert_eq!(through.iter().filter(|&&c| c).count(), 1);
        let touching = [segments_cross(p1, p2, [-1, 1], [0, 0]), segments_cross(p1, p2, [0, 0], [-1, -1])];
        assert_eq!(touching.iter().filter(|&&c| c).count(), 0);
        // And the same along a slanted line.
        let (p1, p2) = ([-5, -5], [5, 5]);
        let through = [segments_cross(p1, p2, [-2, 2], [0, 0]), segments_cross(p1, p2, [0, 0], [2, -2])];
        assert_eq!(through.iter().filter(|&&c| c).count(), 1);
    }

    #[test]
    fn a_ring_held_twice_isnt_the_ring() {
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        let quad = vec![e7(0.0, 50.0), e7(1.0, 50.0), e7(1.0, 51.0), e7(0.0, 51.0)];
        let once = Coverage { shapes: vec![Shape::new("a".into(), vec![quad.clone()], 0.0)] };
        let twice = Coverage { shapes: vec![Shape::new("a".into(), vec![quad.clone(), quad], 0.0)] };
        // A box across the south edge, its corner outside: under even-odd the doubled ring covers
        // nothing, so the fingerprints must differ.
        let across = b(0.2, 49.9, 0.4, 50.1);
        assert!(twice.shapes[0].contains(e7(0.3, 50.05)) != once.shapes[0].contains(e7(0.3, 50.05)));
        assert_ne!(once.fingerprint(across), twice.fingerprint(across));
    }

    #[test]
    fn boxes_meet_exactly() {
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        // Singapore-sized (0.4° × 0.2°), between the points an 8×8 sample of a z6 tile would test.
        let small = Shape::new("s".into(), vec![vec![e7(103.6, 1.2), e7(104.0, 1.2), e7(104.0, 1.4), e7(103.6, 1.4)]], 0.0);
        assert!(small.meets_rect(b(101.25, 0.0, 106.875, 5.6)));
        assert!(!small.meets_rect(b(104.1, 1.0, 105.0, 2.0)));
        // A box wholly inside a big shape, no edge near it; one wholly in its hole.
        let big = Shape::new("b".into(), shape_rings(), 0.0);
        assert!(big.meets_rect(b(-2.45, 54.98, -2.35, 55.02)));
        assert!(!big.meets_rect(b(-2.05, 54.98, -2.0, 55.02)));
        // Every box agrees with points: a box meets the shape when a point inside it is inside, and
        // a box with none of a fine lattice inside and no edge near is outside.
        for i in 0..60 {
            for j in 0..40 {
                let (x, y) = (-3.2 + i as f64 * 0.04, 54.3 + j as f64 * 0.035);
                let r = b(x, y, x + 0.03, y + 0.02);
                let any = (0..=6).any(|a| (0..=4).any(|c| big.contains(e7(x + a as f64 * 0.005, y + c as f64 * 0.005))));
                if any {
                    assert!(big.meets_rect(r), "box at {x},{y}");
                }
            }
        }
        // A thin strip crossing a box with no vertex in it and no corner inside.
        let strip = Shape::new("t".into(), vec![vec![e7(0.0, 10.0), e7(2.0, 10.0), e7(2.0, 10.001), e7(0.0, 10.001)]], 0.0);
        assert!(strip.meets_rect(b(0.9, 9.9, 1.1, 10.1)));
        assert!(!strip.meets_rect(b(0.9, 10.01, 1.1, 10.1)));
        // The buffer: 1 km around an osm: outline.
        let buffered = Shape::new("o".into(), vec![vec![e7(0.0, 50.0), e7(1.0, 50.0), e7(1.0, 51.0), e7(0.0, 51.0)]], 1000.0);
        assert!(buffered.meets_rect(b(1.008, 50.5, 1.02, 50.6)));
        assert!(!buffered.meets_rect(b(1.03, 50.5, 1.04, 50.6)));
    }
}
