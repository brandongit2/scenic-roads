//! The step `available` (docs/steps.md §14): the source extents' pieces (docs/inputs.md §6.11) as
//! the data-available layers' tiles, which hold at each pixel the set of pieces whose outline holds
//! its centre (codecs::available, the tiles' and packs' format). The server scores them.
//!
//! - **Rasterising.** A piece's rings are lines in longitude and latitude, densified so none strays
//!   from its image by more than a quarter of a finest pixel, then cut into each row of pixels'
//!   centres: a pixel is in a polygon when an odd number of its rings' crossings of that row lie
//!   left of its centre (even-odd: holes out), and in a piece when it's in any of the piece's
//!   polygons (overlapping polygons of one piece add up, never cancel). Each row becomes runs of
//!   one set of pieces. The antimeridian needs nothing (pieces arrive cut there; the plane runs from
//!   −180° to 180°), nor the poles: a ring closed along 90° lies past the Mercator plane's edge.
//! - **The tiles.** From a z6 tile down to z12 a quadtree: a tile all of one set at z12 (every
//!   row's run across it the same) isn't stored, nor anything under it; otherwise its children are
//!   made and it is their union, a pixel's set the union of the four under it. A z6 tile's pack holds
//!   its stored tiles of z6–12; the root pack z0–5, made from every z6 tile's own (`mids`).
//! - **Keys.** A z6 tile's content depends only on the polygons whose boxes meet it: its key is
//!   their pieces' ids and the polygons' digests (`KEY_V` first). The step records each z6 tile's
//!   key in its mids pack's meta and makes again only the tiles whose key moved (a new source, or
//!   one fetched again, makes only the tiles it meets), keeping the others' packs as they are; the
//!   root is made again from the mids whenever a z6 tile moved.
//! - **The polar caps.** Past 85.05° the same sets in each pole's grid (roadcore::polar): a root
//!   pack of z0–3 a pole (a pixel 1,024 m of the grid at z3), from the polygons reaching the
//!   pole's square, keyed alike.

use anyhow::{ensure, Context, Result};
use buildstore::out::Out;
use codecs::available::{self as fmt, Tile, PIXELS, SIDE};
use rayon::prelude::*;
use roadcore::polar::Pole;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// What a piece's polygons make of a tile: bumped with any change to the tiles' bytes for the same
/// pieces (the rasterising, the quadtree, the format). Each tile's key and the step's own version.
pub const KEY_V: u32 = 1;

/// How far a densified edge may stray from the projection of the line it stands for (pixels of
/// the finest zoom).
const STRAY_PX: f64 = 0.25;

/// A piece: its id (`<source>/<piece>`) and its polygons.
#[derive(Clone, Debug)]
pub struct Piece {
    pub id: String,
    pub polygons: Vec<Polygon>,
}

/// A polygon of a piece: its rings (the exterior first) in longitude and latitude, its box (west,
/// south, east, north) and the digest of its coordinates.
#[derive(Clone, Debug)]
pub struct Polygon {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub bbox: [f64; 4],
    pub digest: String,
}

/// The pieces of a source's `.geojson` (an accepted one: its shape is the unit's guarantee), those
/// with an extent (a piece of class `unknown` has none and is drawn nowhere).
pub fn pieces(source: &str, geojson: &[u8]) -> Result<Vec<Piece>> {
    let v: Value = serde_json::from_slice(geojson).with_context(|| format!("{source}'s pieces"))?;
    let mut out = Vec::new();
    for f in v["features"].as_array().into_iter().flatten() {
        let Some(polys) = f["geometry"]["coordinates"].as_array() else { continue };
        let id = format!("{source}/{}", f["properties"]["piece"].as_str().context("a piece without its id")?);
        let mut polygons = Vec::new();
        for p in polys {
            let digest = blake3::hash(serde_json::to_string(p)?.as_bytes()).to_hex()[..16].to_string();
            let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            let mut rings = Vec::new();
            for r in p.as_array().into_iter().flatten() {
                let ring: Vec<[f64; 2]> =
                    r.as_array().into_iter().flatten().filter_map(|q| Some([q[0].as_f64()?, q[1].as_f64()?])).collect();
                for q in &ring {
                    bbox = [bbox[0].min(q[0]), bbox[1].min(q[1]), bbox[2].max(q[0]), bbox[3].max(q[1])];
                }
                rings.push(ring);
            }
            if bbox[0].is_finite() {
                polygons.push(Polygon { rings, bbox, digest });
            }
        }
        out.push(Piece { id, polygons });
    }
    Ok(out)
}

// ---- Sets of pieces ----------------------------------------------------------------------------

/// The sets of pieces met, each once, by number (0: the empty set), and their unions: those of
/// `base` (shared, read only: a row of z6 tiles' runs, its tiles made at once), then its own.
pub struct Sets {
    base: Option<std::sync::Arc<Sets>>,
    /// Its own sets' first number (`base`'s count).
    first: u32,
    list: Vec<Vec<u32>>,
    index: HashMap<Vec<u32>, u32, Fast>,
    unions: HashMap<u64, u32, Fast>,
}

/// A hasher for the sets' numbers (a multiply and a shift: the keys are small integers, no
/// stranger's; std's SipHash was most of a run's time).
#[derive(Clone, Copy, Default)]
struct Fast;

impl std::hash::BuildHasher for Fast {
    type Hasher = FastHasher;
    fn build_hasher(&self) -> FastHasher {
        FastHasher(0)
    }
}

struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn write(&mut self, b: &[u8]) {
        for c in b.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(w));
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Sets {
    fn default() -> Self {
        let mut index = HashMap::with_hasher(Fast);
        index.insert(Vec::new(), 0);
        Sets { base: None, first: 0, list: vec![Vec::new()], index, unions: HashMap::with_hasher(Fast) }
    }
}

impl Sets {
    /// Sets over `base`'s, adding their own.
    fn over(base: std::sync::Arc<Sets>) -> Sets {
        let first = base.first + base.list.len() as u32;
        Sets {
            base: Some(base),
            first,
            list: Vec::new(),
            index: HashMap::with_hasher(Fast),
            unions: HashMap::with_hasher(Fast),
        }
    }

    /// Set `k`'s pieces.
    fn get(&self, k: u32) -> &[u32] {
        match &self.base {
            Some(b) if k < self.first => b.get(k),
            _ => &self.list[(k - self.first) as usize],
        }
    }

    /// `v`'s number, if it has one.
    fn find(&self, v: &[u32]) -> Option<u32> {
        self.base.as_ref().and_then(|b| b.find(v)).or_else(|| self.index.get(v).copied())
    }

    /// The number of sorted set `v`.
    fn id(&mut self, v: Vec<u32>) -> u32 {
        if let Some(k) = self.find(&v) {
            return k;
        }
        let k = self.first + self.list.len() as u32;
        self.list.push(v.clone());
        self.index.insert(v, k);
        k
    }

    fn union(&mut self, a: u32, b: u32) -> u32 {
        if a == b || b == 0 {
            return a;
        }
        if a == 0 {
            return b;
        }
        let key = (u64::from(a.min(b)) << 32) | u64::from(a.max(b));
        if let Some(&k) = self.unions.get(&key) {
            return k;
        }
        let mut v: Vec<u32> = self.get(a).iter().chain(self.get(b)).copied().collect();
        v.sort_unstable();
        v.dedup();
        let k = self.id(v);
        self.unions.insert(key, k);
        k
    }
}

// ---- The plane: rows of runs ---------------------------------------------------------------------

/// A polygon in a plane's pixels (x right, y down; a pixel's centre at +0.5), its piece's number.
struct PlanePoly {
    piece: u32,
    /// Its rings' edges: (x0, y0, x1, y1).
    edges: Vec<[f64; 4]>,
    ymin: f64,
    ymax: f64,
}

/// A polygon's rings densified and projected by `f` (longitude, latitude → plane pixels), each edge
/// within `STRAY_PX` of the projected line.
fn plane_poly(piece: u32, p: &Polygon, f: &dyn Fn(f64, f64) -> [f64; 2]) -> PlanePoly {
    let mut edges = Vec::new();
    let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
    for r in &p.rings {
        let mut pts: Vec<[f64; 2]> = Vec::with_capacity(r.len());
        for w in r.windows(2) {
            densify(w[0], w[1], f(w[0][0], w[0][1]), f(w[1][0], w[1][1]), f, 0, &mut pts);
        }
        if let Some(last) = r.last() {
            pts.push(f(last[0], last[1]));
        }
        for w in pts.windows(2) {
            ymin = ymin.min(w[0][1]);
            ymax = ymax.max(w[0][1]);
            if w[0][1] != w[1][1] {
                edges.push([w[0][0], w[0][1], w[1][0], w[1][1]]);
            }
        }
    }
    PlanePoly { piece, edges, ymin, ymax }
}

/// Pushes the projected points of the line from `a` to `b` (degrees; `pa`, `pb` their images),
/// all but `b`'s, so each segment between them strays from the projected line by `STRAY_PX` at
/// most (its midpoint's distance from its image, halving until it's within).
fn densify(
    a: [f64; 2],
    b: [f64; 2],
    pa: [f64; 2],
    pb: [f64; 2],
    f: &dyn Fn(f64, f64) -> [f64; 2],
    depth: u32,
    out: &mut Vec<[f64; 2]>,
) {
    let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let pm = f(m[0], m[1]);
    let (dx, dy) = (pm[0] - (pa[0] + pb[0]) / 2.0, pm[1] - (pa[1] + pb[1]) / 2.0);
    // (Forty halvings: a span of 2^40 of an edge, below a float's grain at any plane's size.)
    if depth < 40 && dx * dx + dy * dy > STRAY_PX * STRAY_PX {
        densify(a, m, pa, pm, f, depth + 1, out);
        densify(m, b, pm, pb, f, depth + 1, out);
    } else {
        out.push(pa);
    }
}

/// A row's runs: (first pixel, set), the first at 0, each to the next's first (the last to the
/// plane's width).
type Runs = Vec<(u32, u32)>;

/// The first pixel whose centre lies past crossing `c` (a pixel i is right of c when i + 0.5 > c).
fn past(c: f64, width: u32) -> u32 {
    ((c - 0.5).floor() + 1.0).clamp(0.0, f64::from(width)) as u32
}

/// The runs of rows `r0..r1` of a plane `width` pixels wide.
fn row_runs(polys: &[PlanePoly], width: u32, r0: u32, r1: u32, sets: &mut Sets) -> Vec<Runs> {
    let n = (r1 - r0) as usize;
    let mut cross: Vec<Vec<(u32, f64)>> = vec![Vec::new(); n];
    for (k, p) in polys.iter().enumerate() {
        if p.ymax < f64::from(r0) || p.ymin > f64::from(r1) {
            continue;
        }
        for e in &p.edges {
            let (ya, yb) = (e[1].min(e[3]), e[1].max(e[3]));
            // (Rows whose centre c has (y0 > c) != (y1 > c): c in [ya, yb).)
            let lo = ((ya - 0.5).ceil().max(f64::from(r0))) as i64;
            let hi = ((yb - 0.5).ceil().min(f64::from(r1))) as i64;
            for r in lo.max(i64::from(r0))..hi {
                let c = r as f64 + 0.5;
                if (e[1] > c) != (e[3] > c) {
                    let x = e[0] + (c - e[1]) * (e[2] - e[0]) / (e[3] - e[1]);
                    cross[(r - i64::from(r0)) as usize].push((k as u32, x));
                }
            }
        }
    }
    let mut out = Vec::with_capacity(n);
    let mut spans: Vec<(u32, u32, u32)> = Vec::new();
    for mut c in cross {
        c.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        spans.clear();
        for pair in c.chunks_exact(2) {
            let (a, b) = (past(pair[0].1, width), past(pair[1].1, width));
            if a < b {
                spans.push((polys[pair[0].0 as usize].piece, a, b));
            }
        }
        out.push(runs_of(&spans, width, sets));
    }
    out
}

/// A row's runs from its pieces' spans (piece, first pixel, past the last), those of one piece
/// joined.
fn runs_of(spans: &[(u32, u32, u32)], width: u32, sets: &mut Sets) -> Runs {
    if spans.is_empty() {
        return vec![(0, 0)];
    }
    // (Events: at x, a piece's count up or down.)
    let mut ev: Vec<(u32, i32, u32)> = Vec::with_capacity(spans.len() * 2);
    for &(p, a, b) in spans.iter() {
        ev.push((a, 1, p));
        ev.push((b, -1, p));
    }
    ev.sort_unstable();
    let mut count: BTreeMap<u32, i32> = BTreeMap::new();
    let mut runs: Runs = vec![(0, 0)];
    let mut i = 0;
    while i < ev.len() {
        let x = ev[i].0;
        while i < ev.len() && ev[i].0 == x {
            let c = count.entry(ev[i].2).or_insert(0);
            *c += ev[i].1;
            if *c == 0 {
                count.remove(&ev[i].2);
            }
            i += 1;
        }
        if x >= width {
            break;
        }
        let s = sets.id(count.keys().copied().collect());
        match runs.last_mut() {
            Some(last) if last.0 == x => last.1 = s,
            Some(last) if last.1 == s => {}
            _ => runs.push((x, s)),
        }
    }
    // (Runs of one set side by side, a run replaced where it started, joined.)
    runs.dedup_by(|b, a| a.1 == b.1);
    runs
}

// ---- The quadtree ------------------------------------------------------------------------------

/// A tile: all one set at the finest zoom under it, or its pixels' sets.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Uniform(u32),
    Raster(Vec<u32>),
}

/// Rows of runs of a plane from row `r0`, its tiles' finest zoom `zmax`.
struct Rows<'a> {
    runs: &'a [Runs],
    r0: u32,
    width: u32,
    zmax: u8,
}

impl Rows<'_> {
    /// The run of row `r` holding pixel `x`, and where it ends.
    fn run_at(&self, r: u32, x: u32) -> (u32, u32) {
        let rs = &self.runs[(r - self.r0) as usize];
        let k = rs.partition_point(|run| run.0 <= x) - 1;
        (rs[k].1, rs.get(k + 1).map_or(self.width, |n| n.0))
    }

    /// The set of pixels [x0, x1) × rows [ra, rb) when it's one set.
    fn uniform(&self, x0: u32, x1: u32, ra: u32, rb: u32) -> Option<u32> {
        let mut set = None;
        for r in ra..rb {
            let (s, end) = self.run_at(r, x0);
            if end < x1 || set.is_some_and(|t| t != s) {
                return None;
            }
            set = Some(s);
        }
        set
    }

    /// Tile z/x/y (z = `zmax`) as pixels.
    fn raster(&self, x0: u32, ra: u32) -> Vec<u32> {
        let mut px = vec![0u32; PIXELS];
        for j in 0..SIDE as u32 {
            let rs = &self.runs[(ra + j - self.r0) as usize];
            let mut k = rs.partition_point(|run| run.0 <= x0) - 1;
            for i in 0..SIDE as u32 {
                while rs.get(k + 1).is_some_and(|n| n.0 <= x0 + i) {
                    k += 1;
                }
                px[(j as usize) * SIDE + i as usize] = rs[k].1;
            }
        }
        px
    }

    /// Tile z/x/y, its stored descendants handed to `emit` (zoom, x, y, cell), finest first.
    fn build(&self, z: u8, x: u32, y: u32, sets: &mut Sets, emit: &mut Emit) -> Cell {
        let side = (SIDE as u32) << (self.zmax - z);
        let (x0, ra) = (x * side, y * side);
        if let Some(s) = self.uniform(x0, x0 + side, ra, ra + side) {
            return Cell::Uniform(s);
        }
        if z == self.zmax {
            return Cell::Raster(self.raster(x0, ra));
        }
        let kids: [Cell; 4] = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
            let c = self.build(z + 1, 2 * x + dx, 2 * y + dy, sets, emit);
            if matches!(c, Cell::Raster(_)) {
                emit(z + 1, 2 * x + dx, 2 * y + dy, &c, sets);
            }
            c
        });
        combine(&kids, sets)
    }
}

/// What a quadtree hands each tile it stores: its zoom, x, y and cell, and the sets its cell names.
type Emit<'a> = dyn FnMut(u8, u32, u32, &Cell, &Sets) + 'a;

/// A tile from its four children (top left, top right, bottom left, bottom right): each pixel the
/// union of the four under it; uniform where they all are, of one set.
pub fn combine(kids: &[Cell; 4], sets: &mut Sets) -> Cell {
    if let [Cell::Uniform(a), Cell::Uniform(b), Cell::Uniform(c), Cell::Uniform(d)] = kids {
        if a == b && b == c && c == d {
            return Cell::Uniform(*a);
        }
    }
    let h = SIDE / 2;
    let mut px = vec![0u32; PIXELS];
    for (q, kid) in kids.iter().enumerate() {
        let (ox, oy) = ((q % 2) * h, (q / 2) * h);
        match kid {
            Cell::Uniform(s) => {
                for j in 0..h {
                    px[(oy + j) * SIDE + ox..][..h].fill(*s);
                }
            }
            Cell::Raster(k) => {
                for j in 0..h {
                    for i in 0..h {
                        let (a, b) = (k[2 * j * SIDE + 2 * i], k[2 * j * SIDE + 2 * i + 1]);
                        let (c, d) = (k[(2 * j + 1) * SIDE + 2 * i], k[(2 * j + 1) * SIDE + 2 * i + 1]);
                        px[(oy + j) * SIDE + ox + i] = if a == b && b == c && c == d {
                            a
                        } else {
                            let (ab, cd) = (sets.union(a, b), sets.union(c, d));
                            sets.union(ab, cd)
                        };
                    }
                }
            }
        }
    }
    Cell::Raster(px)
}

/// A cell as a stored tile: its sets in the order they first appear, named by the pieces' ids.
fn tile_of(cell: &Cell, sets: &Sets, ids: &[String]) -> Tile {
    let one;
    let px: &[u32] = match cell {
        Cell::Raster(p) => p,
        Cell::Uniform(s) => {
            one = vec![*s; PIXELS];
            &one
        }
    };
    let mut local: HashMap<u32, u16, Fast> = HashMap::with_hasher(Fast);
    let mut order: Vec<u32> = Vec::new();
    // (Pixels side by side mostly hold one set: the last one's index first.)
    let mut last: Option<(u32, u16)> = None;
    let out: Vec<u16> = px
        .iter()
        .map(|s| match last {
            Some((ls, li)) if ls == *s => li,
            _ => {
                let i = *local.entry(*s).or_insert_with(|| {
                    order.push(*s);
                    (order.len() - 1) as u16
                });
                last = Some((*s, i));
                i
            }
        })
        .collect();
    let used: BTreeSet<&str> =
        order.iter().flat_map(|s| sets.get(*s).iter().map(|&p| ids[p as usize].as_str())).collect();
    let names: Vec<String> = used.iter().map(|s| s.to_string()).collect();
    let at: HashMap<&str, u32> = used.iter().enumerate().map(|(k, s)| (*s, k as u32)).collect();
    let tsets = order
        .iter()
        .map(|s| {
            let mut v: Vec<u32> = sets.get(*s).iter().map(|&p| at[ids[p as usize].as_str()]).collect();
            v.sort_unstable();
            v
        })
        .collect();
    Tile { ids: names, sets: tsets, px: out }
}

/// A stored tile as a cell of these pieces' sets (`by_id`: a piece's number by its id).
fn cell_of(t: &Tile, by_id: &HashMap<&str, u32>, sets: &mut Sets) -> Result<Cell> {
    let mut ids = Vec::with_capacity(t.sets.len());
    for s in &t.sets {
        let mut v = Vec::with_capacity(s.len());
        for &k in s {
            let id = &t.ids[k as usize];
            v.push(*by_id.get(id.as_str()).with_context(|| format!("a tile names {id}, no piece now"))?);
        }
        v.sort_unstable();
        ids.push(sets.id(v));
    }
    Ok(if ids.len() == 1 {
        Cell::Uniform(ids[0])
    } else {
        Cell::Raster(t.px.iter().map(|&p| ids[p as usize]).collect())
    })
}

// ---- The step ----------------------------------------------------------------------------------

/// Web Mercator's plane at z12: 2^20 pixels a side.
const MERC_W: u32 = (SIDE as u32) << fmt::MAXZ;
/// A z6 tile's rows at z12.
const BAND: u32 = (SIDE as u32) << (fmt::MAXZ - fmt::PACK_Z);

fn merc_px(lon: f64, lat: f64) -> [f64; 2] {
    let (x, y) = roadcore::geo::merc(lon, lat);
    [x * f64::from(MERC_W), y * f64::from(MERC_W)]
}

/// The z6 tiles a box (west, south, east, north) meets, edges included.
fn z6_range(b: [f64; 4]) -> [u32; 4] {
    let n = 1u32 << fmt::PACK_Z;
    let t = |v: f64| ((v * f64::from(n)).floor().clamp(0.0, f64::from(n - 1))) as u32;
    let (x0, y0) = roadcore::geo::merc(b[0], b[3]);
    let (x1, y1) = roadcore::geo::merc(b[2], b[1]);
    // (A box edge on a tile's edge: the tile on either side, as a pixel's centre may lie on either.)
    let e = 1e-12;
    [t(x0 - e), t(y0 - e), t(x1 + e), t(y1 + e)]
}

/// A pole's plane: the grid's square at z`POLAR_MAXZ`, a pixel `pixel_m(POLAR_MAXZ)` metres.
fn polar_w() -> u32 {
    (SIDE as u32) << fmt::POLAR_MAXZ
}

fn polar_px(pole: Pole, lon: f64, lat: f64) -> [f64; 2] {
    // (A latitude past the equator moved onto it: the square is far from it, and the plane stays
    // finite; what the polygon holds in the square doesn't change.)
    let lat = match pole {
        Pole::North => lat.max(0.0),
        Pole::South => lat.min(0.0),
    };
    let (gx, gy) = roadcore::polar::forward(pole, lon, lat);
    let p = roadcore::polar::pixel_m(fmt::POLAR_MAXZ);
    [(gx + roadcore::polar::HALF) / p, (roadcore::polar::HALF - gy) / p]
}

/// A key from its lines.
fn key_of(lines: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(format!("available {KEY_V}\n").as_bytes());
    for l in lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex()[..16].to_string()
}

/// Each z6 tile a polygon's box meets ("6/x/y"), and its key: the lines of those polygons.
pub fn tile_keys(pieces: &[Piece]) -> BTreeMap<String, String> {
    let mut lines: BTreeMap<(u32, u32), Vec<String>> = BTreeMap::new();
    for p in pieces {
        for q in &p.polygons {
            let r = z6_range(q.bbox);
            for y in r[1]..=r[3] {
                for x in r[0]..=r[2] {
                    lines.entry((x, y)).or_default().push(format!("{} {}", p.id, q.digest));
                }
            }
        }
    }
    lines
        .into_iter()
        .map(|((x, y), mut l)| {
            l.sort();
            (format!("{}/{x}/{y}", fmt::PACK_Z), key_of(&l))
        })
        .collect()
}

/// The polygons reaching pole `pole`'s square, by (piece, polygon), and its key.
fn polar_polys(pieces: &[Piece], pole: Pole) -> (Vec<(usize, usize)>, String) {
    let b = roadcore::polar::lat_box(pole, 0, 0, 0);
    let mut v = Vec::new();
    let mut lines = vec![format!("pole {}", pole.letter())];
    for (i, p) in pieces.iter().enumerate() {
        for (k, q) in p.polygons.iter().enumerate() {
            if q.bbox[3] >= b[1] && q.bbox[1] <= b[3] {
                v.push((i, k));
                lines.push(format!("{} {}", p.id, q.digest));
            }
        }
    }
    lines[1..].sort();
    (v, key_of(&lines))
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Made {
    /// The z6 tiles made again (their key moved), and those dropped (no polygon meets them now).
    pub remade: Vec<String>,
    pub dropped: Vec<String>,
    /// The z6 packs and stored tiles written.
    pub packs: usize,
    pub tiles: usize,
    /// Whether the root was made again; the poles made again.
    pub root: bool,
    pub poles: Vec<char>,
}

/// The mids pack's meta: each z6 tile's key ("6/x/y"), each pole's ("n", "s").
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct MidsMeta {
    theme: String,
    v: u32,
    keys: BTreeMap<String, String>,
    poles: BTreeMap<String, String>,
}

/// The mids pack as it was: its meta and its z6 tiles' blobs (gzip'd) by "6/x/y", and whether it
/// was there to read (none, of another version, or not readable whole: everything made again).
fn read_mids(out: &Out, theme: &str) -> Result<(MidsMeta, BTreeMap<String, Vec<u8>>, bool)> {
    let Some(c) = out.get(&fmt::mids_logical(theme)) else { return Ok((Default::default(), BTreeMap::new(), false)) };
    let b = out.path(c).read().with_context(|| format!("read {c}"))?;
    let parsed = || -> Result<(MidsMeta, BTreeMap<String, Vec<u8>>)> {
        let idx = store::pack::PackIndex::parse_bytes(&b)?;
        let meta: MidsMeta = serde_json::from_value(idx.meta.clone()).context("the mids' meta")?;
        let mut blobs = BTreeMap::new();
        for e in &idx.entries {
            let (z, x, y) = e.zxy();
            let blob =
                b.get(e.offset as usize..).and_then(|r| r.get(..e.len as usize)).context("a blob past the pack")?;
            Tile::decode(blob).with_context(|| format!("{z}/{x}/{y}"))?;
            blobs.insert(format!("{z}/{x}/{y}"), blob.to_vec());
        }
        Ok((meta, blobs))
    };
    match parsed() {
        // (Made by another version: everything made again.)
        Ok((meta, _)) if meta.v != KEY_V => Ok((Default::default(), BTreeMap::new(), false)),
        Ok((meta, blobs)) => Ok((meta, blobs, true)),
        Err(e) => {
            eprintln!("available: the record {c} isn't readable ({e:#}): every tile made again");
            Ok((Default::default(), BTreeMap::new(), false))
        }
    }
}

/// Writes a pack of `tiles` (zoom, x, y, cell; any order) as `logical` and puts it; how many tiles.
fn put_pack(out: &mut Out, layer: &str, scope: &str, root: (u8, u32, u32), mut tiles: Vec<Stored>) -> Result<usize> {
    tiles.sort_by_key(|t| roadcore::archive::tile_key(t.0, t.1, t.2));
    let n = tiles.len();
    match crate::layers::write_pack_local(out, layer, fmt::ENCODING, true, scope, root, &mut tiles.into_iter())? {
        Some(p) => {
            out.put_file(&p.logical, "pack", &p.local)?;
            Ok(n)
        }
        None => Ok(0),
    }
}

/// The z6 tile a key names ("6/x/y").
fn xy(t: &str) -> (u32, u32) {
    let mut p = t.split('/').skip(1).map(|v| v.parse().unwrap_or(0));
    (p.next().unwrap_or(0), p.next().unwrap_or(0))
}

/// The root's tile z/x/y (z under `PACK_Z`) from the z6 tiles' own (`mids`: gzip'd, by "6/x/y";
/// none, empty), its stored tiles under it pushed to `tiles` (zoom, x, y, blob, raw length).
#[allow(clippy::too_many_arguments)]
fn root_cell(
    z: u8,
    x: u32,
    y: u32,
    mids: &BTreeMap<String, Vec<u8>>,
    by_id: &HashMap<&str, u32>,
    ids: &[String],
    sets: &mut Sets,
    tiles: &mut Vec<Stored>,
) -> Result<Cell> {
    if z == fmt::PACK_Z {
        return match mids.get(&format!("{z}/{x}/{y}")) {
            Some(b) => cell_of(&Tile::decode(b)?, by_id, sets),
            None => Ok(Cell::Uniform(0)),
        };
    }
    // (A quarter of the world with no z6 tile under it: empty, not looked into.)
    let d = fmt::PACK_Z - z;
    let any = mids.keys().any(|t| {
        let (tx, ty) = xy(t);
        (tx >> d, ty >> d) == (x, y)
    });
    if !any {
        return Ok(Cell::Uniform(0));
    }
    let mut kids = Vec::with_capacity(4);
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let c = root_cell(z + 1, 2 * x + dx, 2 * y + dy, mids, by_id, ids, sets, tiles)?;
        if z + 1 < fmt::PACK_Z && matches!(c, Cell::Raster(_)) {
            let (b, raw) = stored(&c, sets, ids);
            tiles.push((z + 1, 2 * x + dx, 2 * y + dy, b, raw));
        }
        kids.push(c);
    }
    let kids: [Cell; 4] = kids.try_into().map_err(|_| anyhow::anyhow!("four children"))?;
    Ok(combine(&kids, sets))
}

/// A stored tile: its zoom, x, y, gzip'd bytes and their length unzipped.
type Stored = (u8, u32, u32, Vec<u8>, u32);

fn stored(cell: &Cell, sets: &Sets, ids: &[String]) -> (Vec<u8>, u32) {
    let t = tile_of(cell, sets, ids).encode();
    (fmt::gzip(&t), t.len() as u32)
}

/// Builds theme `theme`'s layers from `pieces` (every source's of the theme), making again only what
/// their keys say moved; `report` hears the z6 tiles done of those to make.
pub fn build(out: &mut Out, theme: &str, pieces: &[Piece], report: &dyn Fn(u64, u64)) -> Result<Made> {
    let mut seen = BTreeSet::new();
    for p in pieces {
        ensure!(seen.insert(p.id.as_str()), "piece {} twice", p.id);
    }
    let layer = fmt::layer(theme);
    let ids: Vec<String> = pieces.iter().map(|p| p.id.clone()).collect();
    let by_id: HashMap<&str, u32> = ids.iter().enumerate().map(|(k, s)| (s.as_str(), k as u32)).collect();
    let (old, mut blobs, trusted) = read_mids(out, theme)?;
    let keys = tile_keys(pieces);
    let mut made = Made::default();
    let mut sets = Sets::default();

    // The z6 tiles whose key moved, or whose pack is missing; those no polygon meets now.
    let stale: Vec<(u32, u32)> = keys
        .iter()
        .filter(|(t, k)| {
            old.keys.get(*t) != Some(*k) || {
                let (x, y) = xy(t);
                let packed = blobs.get(*t).is_some_and(|b| Tile::decode(b).is_ok_and(|t| t.sets.len() > 1));
                packed && out.get(&fmt::pack_logical(&layer, x, y)).is_none()
            }
        })
        .map(|(t, _)| xy(t))
        .collect();
    // (Those the record names, and any z6 pack in the records: one left by a record lost or of
    // another version is dropped too, never served.)
    let prefix = format!("layers/{layer}/hi/");
    let packed = out
        .manifest
        .range(prefix.clone()..)
        .take_while(|(l, _)| l.starts_with(&prefix))
        .map(|(l, _)| l[prefix.len()..].replace('-', "/"));
    let gone: BTreeSet<String> = old.keys.keys().cloned().chain(packed).filter(|t| !keys.contains_key(t)).collect();
    for t in gone {
        let (x, y) = xy(&t);
        let l = fmt::pack_logical(&layer, x, y);
        if out.get(&l).is_some() {
            out.remove(&l);
        }
        blobs.remove(&t);
        made.dropped.push(t);
    }

    // The stale z6 tiles a row of them at a time: the row's runs, then each tile's quadtree.
    let polys: Vec<PlanePoly> = pieces
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.polygons.iter().map(move |q| (i, q)))
        .map(|(i, q)| plane_poly(i as u32, q, &merc_px))
        .collect();
    let mut by_row: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(x, y) in &stale {
        by_row.entry(y).or_default().push(x);
    }
    let total = stale.len() as u64;
    let mut done = 0;
    report(0, total);
    for (y, xs) in by_row {
        let mut band = Sets::default();
        let runs = row_runs(&polys, MERC_W, y * BAND, (y + 1) * BAND, &mut band);
        let band = std::sync::Arc::new(band);
        let rows = Rows { runs: &runs, r0: y * BAND, width: MERC_W, zmax: fmt::MAXZ };
        // (Each tile's quadtree on a thread of its own, its sets over the row's; their packs put in
        // the row's order.)
        let made_row: Vec<(u32, Option<(Vec<u8>, u32)>, bool, Vec<Stored>)> = xs
            .par_iter()
            .map(|&x| {
                let mut sets = Sets::over(band.clone());
                let mut tiles = Vec::new();
                let cell = rows.build(fmt::PACK_Z, x, y, &mut sets, &mut |z, tx, ty, c, s| {
                    let (b, raw) = stored(c, s, &ids);
                    tiles.push((z, tx, ty, b, raw));
                });
                let own = (!matches!(cell, Cell::Uniform(0))).then(|| stored(&cell, &sets, &ids));
                (x, own, matches!(cell, Cell::Raster(_)), tiles)
            })
            .collect();
        for (x, own, raster, mut tiles) in made_row {
            let t = format!("{}/{x}/{y}", fmt::PACK_Z);
            made.remade.push(t.clone());
            let l = fmt::pack_logical(&layer, x, y);
            match own {
                Some((b, raw)) if raster => {
                    tiles.push((fmt::PACK_Z, x, y, b.clone(), raw));
                    made.tiles += put_pack(out, &layer, "hi", (fmt::PACK_Z, x, y), tiles)?;
                    made.packs += 1;
                    blobs.insert(t, b);
                }
                own => {
                    if out.get(&l).is_some() {
                        out.remove(&l);
                    }
                    match own {
                        Some((b, _)) => blobs.insert(t, b),
                        None => blobs.remove(&t),
                    };
                }
            }
            done += 1;
            report(done, total);
        }
    }

    // The root, from every z6 tile's own, when one moved, it's missing or the record wasn't read.
    let root_l = fmt::root_logical(&layer);
    if !trusted || !stale.is_empty() || !made.dropped.is_empty() || (out.get(&root_l).is_none() && !blobs.is_empty()) {
        made.root = true;
        let mut tiles = Vec::new();
        let cell = root_cell(0, 0, 0, &blobs, &by_id, &ids, &mut sets, &mut tiles)?;
        match cell {
            Cell::Uniform(0) => {
                if out.get(&root_l).is_some() {
                    out.remove(&root_l);
                }
            }
            c => {
                let (b, raw) = stored(&c, &sets, &ids);
                tiles.push((0, 0, 0, b, raw));
                made.tiles += put_pack(out, &layer, "root", (0, 0, 0), tiles)?;
            }
        }
    }

    // The polar caps.
    let mut poles = BTreeMap::new();
    for pole in [Pole::North, Pole::South] {
        let (which, key) = polar_polys(pieces, pole);
        let p = pole.letter().to_string();
        let pl = fmt::polar_layer(theme, pole.letter());
        let l = fmt::root_logical(&pl);
        poles.insert(p.clone(), key.clone());
        if old.poles.get(&p) == Some(&key) && (which.is_empty() || out.get(&l).is_some()) {
            continue;
        }
        made.poles.push(pole.letter());
        let polys: Vec<PlanePoly> = which
            .iter()
            .map(|&(i, k)| plane_poly(i as u32, &pieces[i].polygons[k], &|lon, lat| polar_px(pole, lon, lat)))
            .collect();
        let w = polar_w();
        let runs = row_runs(&polys, w, 0, w, &mut sets);
        let rows = Rows { runs: &runs, r0: 0, width: w, zmax: fmt::POLAR_MAXZ };
        let mut tiles = Vec::new();
        let cell = rows.build(0, 0, 0, &mut sets, &mut |z, tx, ty, c, s| {
            let (b, raw) = stored(c, s, &ids);
            tiles.push((z, tx, ty, b, raw));
        });
        match cell {
            Cell::Uniform(0) => {
                if out.get(&l).is_some() {
                    out.remove(&l);
                }
            }
            c => {
                let (b, raw) = stored(&c, &sets, &ids);
                tiles.push((0, 0, 0, b, raw));
                made.tiles += put_pack(out, &pl, "root", (0, 0, 0), tiles)?;
            }
        }
    }

    // The mids, with each tile's key.
    let meta = MidsMeta { theme: theme.to_string(), v: KEY_V, keys, poles };
    let ml = fmt::mids_logical(theme);
    let local = out.scratch_file(&format!("{ml}.pack"));
    let mut w = store::pack::PackWriter::create(&local, serde_json::to_value(&meta)?, true)?;
    for (t, b) in &blobs {
        let (x, y) = xy(t);
        let raw = Tile::decode(b)?.encode().len() as u32;
        w.add(fmt::PACK_Z, x, y, b, raw)?;
    }
    w.finish()?;
    out.put_file(&ml, "pack", &local)?;
    Ok(made)
}

#[cfg(test)]
mod tests;
