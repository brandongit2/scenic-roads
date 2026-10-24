//! Land and water, cell by cell (docs/inputs.md §6.1.3 "Land and water", §6.1.4 "Memory"): the
//! regions with a `water` key resolved on a grid of 2° cells, so that no operation holds more than
//! a cell's geometry (and, for a buffer or a nearest-coast split, its neighbourhood's).
//!
//! - Every part a region names is cut into the cells by clipping its rings to boxes
//!   (Sutherland–Hodgman, halving the box until each piece is one cell's: plain points, no
//!   polygon operation on the whole), then each cell's rings made an area.
//! - Each expression is evaluated cell by cell: a union, difference or intersection of the parts'
//!   pieces in the cell; a buffer of the pieces in the cells within its distance; a nearest-coast
//!   split of the cell's piece against the coasts in the cells within 4° (the grid it decides on is
//!   anchored at 0°, 0°, so neighbouring cells' splits meet); a cut by its line.
//! - A region's land is its outline cut back to the coastline's land (the cell's), less the lakes
//!   over 100 km²; its water the `water` parts less the coastline's land and every region without
//!   water, after those its water names whole; its lakes, those in its outline that no other region's
//!   water names; the three together its area, kept by cell and stored as the cells' rings.
//! - `hand_out`: a stretch (cells' pieces joined across the cells' edges they share) of a region's
//!   water that one region of the folder alone borders goes to it; one under `sliver_km2`, to the
//!   one sharing its longest edge. Only regions with water take one.
//! - With `cover = "earth"` in `_rules.toml`: per cell, land in two regions is taken from those of
//!   them with water; each piece in no region within 100 m of land, up to `sliver_km2`, joins the
//!   region with water sharing the longest edge with it, and a piece touching none stays; land in
//!   no region over `sliver_km2` is `rg-cover`, as is water in two regions, or in none without a
//!   `rest` region. The `rest` region takes what's left, cell by cell. Regions without water are
//!   never changed.

use super::extract::Id;
use super::file::{Part, Region, Target};
use super::geom::{self, Area, Pt, Ring};
use super::resolve::{Folder, Leaf, Outcome, Problem, Resolved};
use super::stored::Shape;
use crate::inputs::pace;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A cell: its column (−90…89, 2° each from −180°) and row (−45…44).
pub type Cell = (i64, i64);
/// A cell's side, E7.
pub const CELL: i64 = 20_000_000;
const COLS: i64 = 180;

pub fn cell_box(c: Cell) -> [i64; 4] {
    [c.0 * CELL, c.1 * CELL, (c.0 + 1) * CELL, (c.1 + 1) * CELL]
}

fn cells_of(b: [i64; 4]) -> impl Iterator<Item = Cell> {
    let (x0, x1) = (b[0].div_euclid(CELL), (b[2] - 1).div_euclid(CELL));
    let (y0, y1) = (b[1].div_euclid(CELL).max(-45), (b[3] - 1).div_euclid(CELL).min(44));
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

/// A cell column wrapped into −90…89, and the longitude shift (E7) that brings the wrapped cell's
/// geometry to where it was asked for.
fn wrap(x: i64) -> (i64, i64) {
    let w = (x + COLS / 2).rem_euclid(COLS) - COLS / 2;
    (w, (x - w) * CELL)
}

/// An area kept by cell: each cell's piece inside the cell's box.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TArea {
    pub cells: BTreeMap<Cell, Area>,
}

impl TArea {
    pub fn get(&self, c: Cell) -> Area {
        self.cells.get(&c).cloned().unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.values().all(Area::is_empty)
    }

    pub fn set(&mut self, c: Cell, a: Area) {
        if a.is_empty() {
            self.cells.remove(&c);
        } else {
            self.cells.insert(c, a);
        }
    }

    /// Its rings, cell by cell (even–odd: the cells' pieces don't overlap).
    pub fn rings(&self) -> Vec<Ring> {
        self.cells.values().flat_map(|a| a.rings()).collect()
    }

    pub fn km2(&self) -> f64 {
        self.cells.values().map(geom::area_km2).sum()
    }

    /// Its pieces in the cells within `dx` columns and `dy` rows of `c`, shifted across ±180° as
    /// they lie from `c`, and unioned.
    pub fn around(&self, c: Cell, dx: i64, dy: i64) -> Area {
        let mut v = Vec::new();
        for x in c.0 - dx..=c.0 + dx {
            let (w, shift) = wrap(x);
            for y in (c.1 - dy).max(-45)..=(c.1 + dy).min(44) {
                if let Some(a) = self.cells.get(&(w, y)) {
                    v.push(if shift == 0 { a.clone() } else { a.shifted(shift) });
                }
            }
        }
        geom::union_all(&v)
    }

    /// The area of rings (even–odd, plain frame), cut into cells by clipping the rings.
    pub fn from_rings(rings: &[Ring]) -> TArea {
        let mut by_cell: BTreeMap<Cell, Vec<Ring>> = BTreeMap::new();
        let Some(b) = geom::bbox_of(rings.iter().flatten()) else { return TArea::default() };
        let (x0, x1) = (b[0].div_euclid(CELL), (b[2] - 1).max(b[0]).div_euclid(CELL));
        let (y0, y1) = (b[1].div_euclid(CELL), (b[3] - 1).max(b[1]).div_euclid(CELL));
        let rs: Vec<Ring> = rings.iter().filter(|r| r.len() >= 3).cloned().collect();
        split_rings(rs, x0, x1, y0, y1, &mut by_cell);
        let cells = by_cell
            .into_iter()
            .filter(|(c, _)| (-45..=44).contains(&c.1))
            .map(|(c, rs)| {
                let (w, shift) = wrap(c.0);
                let rs: Vec<Ring> = if shift == 0 {
                    rs
                } else {
                    rs.iter().map(|r| r.iter().map(|p| [p[0] - shift, p[1]]).collect()).collect()
                };
                ((w, c.1), Area::from_rings(&rs))
            })
            .filter(|(_, a)| !a.is_empty())
            .collect::<Vec<_>>();
        let mut t = TArea::default();
        for (c, a) in cells {
            let a = match t.cells.get(&c) {
                Some(b) => b.union(&a),
                None => a,
            };
            t.cells.insert(c, a);
        }
        t
    }

    /// Cell by cell.
    pub fn zip(&self, other: &TArea, f: impl Fn(&Area, &Area) -> Area + Sync) -> TArea {
        let cs: BTreeSet<Cell> = self.cells.keys().chain(other.cells.keys()).copied().collect();
        let v: Vec<(Cell, Area)> = pace::par_map(cs.into_iter().collect(), |c| (c, f(&self.get(c), &other.get(c))));
        let mut t = TArea::default();
        for (c, a) in v {
            t.set(c, a);
        }
        t
    }
}

/// Rings clipped to the box of cells x0…x1 × y0…y1, halved (along the longer side, at a cell's
/// edge) until each part is one cell's.
fn split_rings(rings: Vec<Ring>, x0: i64, x1: i64, y0: i64, y1: i64, out: &mut BTreeMap<Cell, Vec<Ring>>) {
    if rings.is_empty() {
        return;
    }
    if x0 == x1 && y0 == y1 {
        let b = cell_box((x0, y0));
        out.entry((x0, y0)).or_default().extend(rings.iter().filter_map(|r| clip(r, b)));
        return;
    }
    let halves = if x1 - x0 >= y1 - y0 {
        let m = (x0 + x1 + 1).div_euclid(2);
        [(x0, m - 1, y0, y1), (m, x1, y0, y1)]
    } else {
        let m = (y0 + y1 + 1).div_euclid(2);
        [(x0, x1, y0, m - 1), (x0, x1, m, y1)]
    };
    // (Each ring moved to the half it lies in, or clipped to each it crosses; the parent's list
    // gone before the halves are cut further: no copies held down the levels.)
    let boxes = halves.map(|(a0, a1, b0, b1)| [a0 * CELL, b0 * CELL, (a1 + 1) * CELL, (b1 + 1) * CELL]);
    let mut parts: [Vec<Ring>; 2] = [Vec::new(), Vec::new()];
    for r in rings {
        let Some(rb) = geom::bbox_of(r.iter()) else { continue };
        let within = |bx: [i64; 4]| rb[0] >= bx[0] && rb[2] <= bx[2] && rb[1] >= bx[1] && rb[3] <= bx[3];
        let meets = |bx: [i64; 4]| !(rb[2] < bx[0] || rb[0] > bx[2] || rb[3] < bx[1] || rb[1] > bx[3]);
        if within(boxes[0]) {
            parts[0].push(r);
        } else if within(boxes[1]) {
            parts[1].push(r);
        } else {
            for k in 0..2 {
                if meets(boxes[k]) {
                    if let Some(c) = clip(&r, boxes[k]) {
                        parts[k].push(c);
                    }
                }
            }
        }
    }
    let [p0, p1] = parts;
    let (h0, h1) = (halves[0], halves[1]);
    split_rings(p0, h0.0, h0.1, h0.2, h0.3, out);
    split_rings(p1, h1.0, h1.1, h1.2, h1.3, out);
}

/// A ring clipped to a box (Sutherland–Hodgman): the part of it inside, edges along the box where
/// it ran outside; None when nothing is left.
pub fn clip(r: &[Pt], b: [i64; 4]) -> Option<Ring> {
    let mut pts: Vec<Pt> = r.to_vec();
    for side in 0..4 {
        if pts.is_empty() {
            return None;
        }
        let inside = |p: Pt| match side {
            0 => p[0] >= b[0],
            1 => p[0] <= b[2],
            2 => p[1] >= b[1],
            _ => p[1] <= b[3],
        };
        let cross = |p: Pt, q: Pt| -> Pt {
            // (Where p–q meets the side's line, rounded to the unit.)
            let (k, v) = match side {
                0 => (0, b[0]),
                1 => (0, b[2]),
                2 => (1, b[1]),
                _ => (1, b[3]),
            };
            let o = 1 - k;
            let t = (v - p[k]) as f64 / (q[k] - p[k]) as f64;
            let mut out = [0i64; 2];
            out[k] = v;
            out[o] = p[o] + ((q[o] - p[o]) as f64 * t).round() as i64;
            out
        };
        let n = pts.len();
        let mut next = Vec::with_capacity(n + 4);
        for i in 0..n {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            match (inside(p), inside(q)) {
                (true, true) => next.push(q),
                (true, false) => next.push(cross(p, q)),
                (false, true) => {
                    next.push(cross(p, q));
                    next.push(q);
                }
                (false, false) => {}
            }
        }
        pts = next;
    }
    pts.dedup();
    (pts.len() >= 3 && geom::area2(&pts) != 0).then_some(pts)
}

// ---- _rules.toml ---------------------------------------------------------------------------------

/// `_rules.toml` (at the folder's top): the rules for the whole folder.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// `"earth"`: the regions cover the Earth once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// A region's piece of water under this (km²) joins a neighbour (`hand_out`'s stretches, and
    /// with `cover`, every region's pieces of water touching none of its land).
    #[serde(default = "sliver")]
    pub sliver_km2: f64,
}

fn sliver() -> f64 {
    50.0
}

pub fn parse_rules(text: &str) -> Result<Rules, super::file::Fault> {
    let r: Rules = toml::from_str(text)
        .map_err(|e| super::file::Fault { check: "rules", message: format!("_rules.toml: {}", e.message()) })?;
    if r.cover.as_deref().is_some_and(|c| c != "earth") {
        return Err(super::file::Fault { check: "rules", message: "_rules.toml: cover is \"earth\" or absent".into() });
    }
    if !(r.sliver_km2 >= 0.0 && r.sliver_km2 <= 10_000.0) {
        return Err(super::file::Fault { check: "rules", message: "_rules.toml: sliver_km2 is 0 to 10,000".into() });
    }
    Ok(r)
}

// ---- evaluating, cell by cell -----------------------------------------------------------------------

/// What a region's parts are, by cell.
enum Src {
    T(TArea),
    Line(Vec<Pt>),
    /// Other regions (`region:`, `land:`, `group:`'s members), read cell by cell where they're
    /// kept, never copied: their land, or (`whole`) land, water and lakes.
    Refs(Vec<String>, bool),
}

struct Env<'a> {
    leaves: HashMap<String, Src>,
    /// Its first `osm:` part's country.
    country: String,
    made: &'a BTreeMap<String, Made>,
    others: &'a BTreeMap<String, TArea>,
    /// Each cell's piece of a buffered part, buffered (by the part and distance): made once,
    /// read by every cell it reaches; dropped with the region's evaluation.
    buffered: std::sync::Mutex<HashMap<(String, Cell), std::sync::Arc<Area>>>,
}

impl Env<'_> {
    /// Region `id`'s piece in cell `c` (land; or, `whole`, all of it).
    fn piece(&self, id: &str, c: Cell, whole: bool) -> Area {
        match self.made.get(id) {
            Some(m) if whole => geom::union_all(&[m.land.get(c), m.water.get(c), m.lakes.get(c)]),
            Some(m) => m.land.get(c),
            None => self.others.get(id).map(|t| t.get(c)).unwrap_or_default(),
        }
    }

    fn refs(&self, ids: &[String], c: Cell, whole: bool) -> Area {
        geom::union_all(&ids.iter().map(|id| self.piece(id, c, whole)).filter(|a| !a.is_empty()).collect::<Vec<_>>())
    }

    fn ref_cells(&self, ids: &[String], whole: bool) -> BTreeSet<Cell> {
        let mut out = BTreeSet::new();
        for id in ids {
            match self.made.get(id) {
                Some(m) => {
                    out.extend(m.land.cells.keys());
                    if whole {
                        out.extend(m.water.cells.keys());
                        out.extend(m.lakes.cells.keys());
                    }
                }
                None => out.extend(self.others.get(id).into_iter().flat_map(|t| t.cells.keys())),
            }
        }
        out
    }
}

fn problem(check: &'static str, r: &Region, path: &str, message: String) -> Problem {
    Problem { check, about: vec![r.id.clone()], found: Vec::new(), files: vec![path.to_string()], message }
}

/// The cells a part can be anywhere in.
fn support(p: &Part, env: &Env) -> BTreeSet<Cell> {
    match p {
        Part::Earth => (-90..90).flat_map(|x| (-45..45).map(move |y| (x, y))).collect(),
        Part::Box(b, _) => cells_of(*b).collect(),
        Part::Rest => BTreeSet::new(),
        Part::Union(ps) => ps.iter().flat_map(|q| support(q, env)).collect(),
        Part::Minus(ps) => support(&ps[0], env),
        Part::Intersect(ps) => {
            let mut it = ps.iter().map(|q| support(q, env));
            let first = it.next().unwrap_or_default();
            it.fold(first, |a, b| a.intersection(&b).copied().collect())
        }
        Part::Nearest { of, .. } | Part::Cut { of, .. } => support(of, env),
        Part::Buffer { of, m, .. } => {
            let base: BTreeSet<Cell> = match &**of {
                Target::Part(q) => support(q, env),
                Target::Line(l) => match env.leaves.get(&format!("line:{l}")) {
                    Some(Src::Line(pts)) => {
                        geom::bbox_of(pts.iter()).map(|b| cells_of(b).collect()).unwrap_or_default()
                    }
                    _ => BTreeSet::new(),
                },
            };
            let mut out = BTreeSet::new();
            for c in base {
                let (dx, dy) = reach(c, *m);
                for x in c.0 - dx..=c.0 + dx {
                    for y in (c.1 - dy).max(-45)..=(c.1 + dy).min(44) {
                        out.insert((wrap(x).0, y));
                    }
                }
            }
            out
        }
        _ => match env.leaves.get(&p.to_string()) {
            Some(Src::T(t)) => t.cells.keys().copied().collect(),
            Some(Src::Refs(ids, whole)) => env.ref_cells(ids, *whole),
            _ => BTreeSet::new(),
        },
    }
}

/// The columns and rows of cells within `m` metres of cell `c`.
fn reach(c: Cell, m: f64) -> (i64, i64) {
    let lat_m = CELL as f64 * 1e-7 * 111_320.0;
    let dy = (m / lat_m).ceil() as i64;
    // (The band's latitude farthest from the equator, but no farther than 86°N or 80°S: the
    // buffers read no cells past those (no land north of 83.7°N; Antarctica's inland ice), and
    // the columns are capped, `MAX_REACH_COLS` south, `MAX_REACH_COLS_NORTH` north, enough for
    // 200 nmi at 86°N.)
    let north = c.1 + dy + 1 > (c.1 - dy).abs();
    let edge = if north { c.1 + dy + 1 } else { (c.1 - dy).abs() };
    let worst = ((edge * CELL) as f64 * 1e-7).min(if north { 86.0 } else { 80.0 });
    let cos = worst.to_radians().cos();
    let dx = ((m / (lat_m * cos)).ceil() as i64).min(if north { MAX_REACH_COLS_NORTH } else { MAX_REACH_COLS });
    (dx, dy)
}

/// The most columns a cell's neighbourhood takes in each way (the polar cap), south.
const MAX_REACH_COLS: i64 = 16;

/// North: 370 km (200 nmi) at 86°N is about 48° of longitude, 24 columns.
const MAX_REACH_COLS_NORTH: i64 = 25;

fn bb_box(c: Cell) -> [i64; 4] {
    cell_box(c)
}

/// The nearest-coast split's reach: coasts farther than this from a cell don't decide it.
const NEAREST_REACH_M: f64 = geom::NEAREST_REACH_M;

fn eval(p: &Part, c: Cell, env: &Env, r: &Region, path: &str) -> Result<Area, Problem> {
    let bb = geom::box_area(cell_box(c));
    Ok(match p {
        Part::Earth => bb,
        Part::Box(b, _) => geom::box_area(*b).intersect(&bb),
        Part::Rest => Area::default(),
        Part::Union(ps) => {
            geom::union_all(&ps.iter().map(|q| eval(q, c, env, r, path)).collect::<Result<Vec<_>, _>>()?)
        }
        Part::Minus(ps) => {
            let a = eval(&ps[0], c, env, r, path)?;
            if a.is_empty() {
                return Ok(a);
            }
            let rest = ps[1..].iter().map(|q| eval(q, c, env, r, path)).collect::<Result<Vec<_>, _>>()?;
            a.minus(&geom::union_all(&rest))
        }
        Part::Intersect(ps) => {
            let mut a = eval(&ps[0], c, env, r, path)?;
            for q in &ps[1..] {
                if a.is_empty() {
                    break;
                }
                a = a.intersect(&eval(q, c, env, r, path)?);
            }
            a
        }
        Part::Buffer { of, m, .. } => {
            let _t = Timed::new(if *m > 200_000.0 { &WIDE_BUFFERS } else { &BUFFERS });
            let (dx, dy) = reach(c, *m);
            let q = match &**of {
                Target::Part(q) => q,
                Target::Line(l) => {
                    return Ok(match env.leaves.get(&format!("line:{l}")) {
                        Some(Src::Line(pts)) => geom::buffer_line(pts, *m).intersect(&bb),
                        _ => Area::default(),
                    })
                }
            };
            // (Each cell's piece of the part buffered once, kept for the cells round it: the
            // cell's buffer is those pieces' buffers that reach it, clipped to it. No union of a
            // neighbourhood.)
            let key = format!("{q} {m}");
            let mut v = Vec::new();
            for x in c.0 - dx..=c.0 + dx {
                let (w, shift) = wrap(x);
                for y in (c.1 - dy).max(-45)..=(c.1 + dy).min(44) {
                    // (Not from the cells south of 80°S: Antarctica's interior ice, which a buffer
                    // drawn in longitude and latitude wraps round the pole, and which its coasts'
                    // own buffers, nearer the equator, already reach past. The Arctic's lands,
                    // to 83.7°N, are buffered up to 86°N, where no land is.)
                    if !(-40..43).contains(&y) {
                        continue;
                    }
                    let k = (key.clone(), (w, y));
                    let got = env.buffered.lock().unwrap().get(&k).cloned();
                    let b = match got {
                        Some(b) => b,
                        None => {
                            let piece = eval(q, (w, y), env, r, path)?;
                            let b = std::sync::Arc::new(if piece.is_empty() {
                                piece
                            } else {
                                geom::buffer_area_coarse(&piece, *m)
                            });
                            env.buffered.lock().unwrap().insert(k, b.clone());
                            b
                        }
                    };
                    if b.is_empty() {
                        continue;
                    }
                    let b = if shift == 0 { (*b).clone() } else { b.shifted(shift) };
                    if b.bbox().is_some_and(|bx| {
                        bx[0] < bb_box(c)[2] && bx[2] > bb_box(c)[0] && bx[1] < bb_box(c)[3] && bx[3] > bb_box(c)[1]
                    }) {
                        v.push(b.intersect(&bb));
                    }
                }
            }
            geom::union_all(&v)
        }
        Part::Nearest { of, to, than, step_m, .. } => {
            let a = eval(of, c, env, r, path)?;
            if a.is_empty() {
                return Ok(a);
            }
            let _t = Timed::new(&NEARESTS);
            let (dx, dy) = reach(c, NEAREST_REACH_M);
            let gather = |ps: &[Part]| -> Result<Vec<Area>, Problem> {
                ps.iter().map(|q| around(q, c, dx, dy, env, r, path)).collect::<Result<Vec<_>, _>>()
            };
            let (ta, tb) = (gather(to)?, gather(than)?);
            geom::nearest(&a, &ta, &tb, *step_m)
        }
        Part::Cut { of, line, left } => {
            let a = eval(of, c, env, r, path)?;
            let Some(Src::Line(pts)) = env.leaves.get(&format!("line:{line}")) else { return Ok(Area::default()) };
            geom::cut(&a, pts, *left).map_err(|geom::CutError::EndsInside(_)| {
                problem(
                    "rg-cut",
                    r,
                    path,
                    format!("{}: line {line} ends inside a piece it cuts: draw it on past the shape's edge", r.id),
                )
            })?
        }
        _ => match env.leaves.get(&p.to_string()) {
            Some(Src::T(t)) => t.get(c),
            Some(Src::Refs(ids, whole)) => env.refs(ids, c, *whole),
            _ => Area::default(),
        },
    })
}

/// A part's pieces in the cells around `c`, as they lie from it.
fn around(p: &Part, c: Cell, dx: i64, dy: i64, env: &Env, r: &Region, path: &str) -> Result<Area, Problem> {
    match env.leaves.get(&p.to_string()) {
        Some(Src::T(t)) => return Ok(t.around(c, dx, dy)),
        Some(Src::Refs(ids, whole)) => {
            let mut v = Vec::new();
            for x in c.0 - dx..=c.0 + dx {
                let (w, shift) = wrap(x);
                for y in (c.1 - dy).max(-45)..=(c.1 + dy).min(44) {
                    let a = env.refs(ids, (w, y), *whole);
                    if !a.is_empty() {
                        v.push(if shift == 0 { a } else { a.shifted(shift) });
                    }
                }
            }
            return Ok(geom::union_all(&v));
        }
        _ => {}
    }
    let mut v = Vec::new();
    for x in c.0 - dx..=c.0 + dx {
        let (w, shift) = wrap(x);
        for y in (c.1 - dy).max(-45)..=(c.1 + dy).min(44) {
            let a = eval(p, (w, y), env, r, path)?;
            if !a.is_empty() {
                v.push(if shift == 0 { a } else { a.shifted(shift) });
            }
        }
    }
    Ok(geom::union_all(&v))
}

/// The coastline's land in cell `c`, less the big lakes; and the big lakes.
fn land_and_lakes(data: &super::data::Datasets, c: Cell) -> (Area, Area) {
    let b = cell_box(c);
    let bb = geom::box_area(b);
    let lakes = data.big_lakes_in(b).intersect(&bb);
    (data.land_in(b).intersect(&bb).minus(&lakes), lakes)
}

fn tiled_leaves(leaves: BTreeMap<String, Leaf>) -> HashMap<String, Src> {
    // (One leaf at a time: a leaf's cutting holds its rings; its cells, in parallel.)
    leaves
        .into_iter()
        .map(|(k, l)| {
            let s = match l {
                Leaf::Rings(shapes, _) => {
                    // (Each shape even–odd on its own; the shapes unioned cell by cell.)
                    let mut t = TArea::default();
                    for (rs, _) in shapes {
                        let u = TArea::from_rings(&rs);
                        t = if t.cells.is_empty() { u } else { t.zip(&u, |a, b| a.union(b)) };
                    }
                    Src::T(t)
                }
                Leaf::Line(p) => Src::Line(p),
            };
            (k, s)
        })
        .collect()
}

/// A region's land, lakes, and (with `water`) water, by cell.
struct Made {
    land: TArea,
    lakes: TArea,
    water: TArea,
    country: String,
}

// ---- the whole --------------------------------------------------------------------------------------

/// Every region with water resolved (those without, `resolve`'s, as they were): into `out`.
pub fn resolve(folder: &Folder, ctx: &super::resolve::Context, out: &mut Outcome) {
    // (On threads whose operations snap: `geom::snap_here`.)
    let threads = crate::inputs::pace::width();
    match rayon::ThreadPoolBuilder::new().num_threads(threads).start_handler(|_| geom::snap_here()).build() {
        Ok(pool) => pool.install(|| geom::snapping(|| resolve_in(folder, ctx, out))),
        Err(_) => geom::snapping(|| resolve_in(folder, ctx, out)),
    }
}

fn resolve_in(folder: &Folder, ctx: &super::resolve::Context, out: &mut Outcome) {
    let rules = folder.rules.clone().unwrap_or_default();
    let ids: Vec<String> =
        folder.regions.iter().filter(|(_, (_, r))| r.has_water()).map(|(id, _)| id.clone()).collect();
    if ids.is_empty() {
        return;
    }
    let Some(data) = ctx.data.filter(|d| d.has_land()) else {
        for id in &ids {
            let (p, r) = &folder.regions[id];
            out.problems.push(problem("rg-data", r, p, format!("{id}: a region with water is cut back to the coastline's land, which isn't read now (the regions-land source: docs/inputs.md §6.1.7)")));
        }
        return;
    };
    let rests: Vec<&String> = ids.iter().filter(|id| folder.regions[*id].1.is_rest()).collect();
    if rests.len() > 1 {
        for id in &rests {
            let (p, r) = &folder.regions[*id];
            out.problems.push(problem("rg-rest", r, p, format!("{id}: rest is in the water of more than one region")));
        }
        return;
    }
    // The regions without water, by cell: what every region's water is less, and what `region:`
    // and `group:` read of them.
    let others: BTreeMap<String, TArea> = pace::par_map(
        out.resolved.iter().filter(|(id, _)| !folder.regions.get(*id).is_some_and(|(_, r)| r.has_water())).collect(),
        |(id, res)| (id.clone(), geometry_tiled(res)),
    )
    .into_iter()
    .collect();
    // (Each region with water stored empty first: what `gather` reads of a region it names, ours
    // read from `made` instead; and what's stored at the end.)
    for id in &ids {
        let r = &folder.regions[id].1;
        out.resolved.insert(
            id.clone(),
            Resolved {
                geometry: super::stored::Geometry {
                    id: id.clone(),
                    osm: ctx.pass.unwrap_or("-").to_string(),
                    shapes: Vec::new(),
                    relations: r.relations(),
                    ways: r.ways(),
                },
                drawn: super::stored::Drawn {
                    fmt: 1,
                    id: id.clone(),
                    name: r.name.clone(),
                    outline: r.outline_text(),
                    shapes: BTreeMap::new(),
                },
                lakes: Area::default(),
            },
        );
    }
    let mut made: BTreeMap<String, Made> = BTreeMap::new();
    let mut held: BTreeSet<String> = BTreeSet::new();
    // (What the extract lacks, as the regions without water name it: the held report's needs, so
    // the lead makes the extract; `out` is read by the waves, so added to it after.)
    let mut needs: BTreeSet<Id> = BTreeSet::new();
    trace("land");
    // 1. Land, in waves by what outlines name.
    let mut left: Vec<String> = ids.clone();
    while !left.is_empty() {
        crate::agent::jobs::within((ids.len() - left.len()) as f64 / ids.len() as f64);
        let (wave, rest): (Vec<String>, Vec<String>) = left.into_iter().partition(|id| {
            folder.deps(&folder.regions[id].1).iter().all(|d| {
                // (One with water is ready once made; its placeholder in `out` isn't it.)
                let water = folder.regions.get(d).is_some_and(|(_, x)| x.has_water());
                held.contains(d) || if water { made.contains_key(d) } else { out.resolved.contains_key(d) }
            })
        });
        if wave.is_empty() {
            for id in &rest {
                let (p, r) = &folder.regions[id];
                out.problems.push(problem(
                    "rg-loop",
                    r,
                    p,
                    format!("{id}: its outline names regions that name it back"),
                ));
                held.insert(id.clone());
            }
            break;
        }
        let res: Vec<(String, Result<Made, Vec<Problem>>)> = wave
            .iter()
            .map(|id| (id.clone(), make_land(id, folder, ctx, data, &made, &others, out, &held, &mut needs)))
            .collect();
        for (id, m) in res {
            match m {
                Ok(m) => {
                    made.insert(id, m);
                }
                Err(ps) => {
                    out.problems.extend(ps);
                    held.insert(id);
                }
            }
        }
        left = rest;
    }
    trace("water");
    // 2. Water, in waves by what waters name whole.
    let wdeps = |id: &str| -> Vec<String> {
        let r = &folder.regions[id].1;
        let mut v = Vec::new();
        for p in r.parts_of(true) {
            match p {
                Part::Region(d) => v.push(d.clone()),
                Part::Group(g) => v.extend(folder.members(g, id)),
                _ => {}
            }
        }
        v.retain(|d| folder.regions.get(d).is_some_and(|(_, r)| r.has_water()) && d != id);
        v.sort();
        v.dedup();
        v
    };
    let mut done_w: BTreeSet<String> = BTreeSet::new();
    let mut left: Vec<String> =
        ids.iter().filter(|id| made.contains_key(*id) && !rests.contains(id)).cloned().collect();
    while !left.is_empty() {
        let (wave, rest): (Vec<String>, Vec<String>) = left
            .into_iter()
            .partition(|id| wdeps(id).iter().all(|d| done_w.contains(d) || held.contains(d) || !made.contains_key(d)));
        if wave.is_empty() {
            for id in &rest {
                let (p, r) = &folder.regions[id];
                out.problems.push(problem(
                    "rg-loop",
                    r,
                    p,
                    format!("{id}: its water names regions whose water names it back"),
                ));
                held.insert(id.clone());
            }
            break;
        }
        let res: Vec<(String, Result<TArea, Vec<Problem>>)> = wave
            .iter()
            .map(|id| {
                trace(&format!("water of {id} begins"));
                let w = make_water(id, folder, ctx, data, &made, &others, out, &held, &mut needs);
                trace(&format!("water of {id} ends"));
                (id.clone(), w)
            })
            .collect();
        for (id, w) in res {
            match w {
                Ok(w) => {
                    made.get_mut(&id).unwrap().water = w;
                    done_w.insert(id);
                }
                Err(ps) => {
                    out.problems.extend(ps);
                    held.insert(id);
                }
            }
        }
        left = rest;
    }
    out.needs.extend(needs);
    for id in &held {
        made.remove(id);
        out.resolved.remove(id);
    }
    trace("lakes");
    // 3. Lakes: a region's own unless another's water names them.
    let lake_cells: BTreeSet<Cell> = made.values().flat_map(|m| m.lakes.cells.keys().copied()).collect();
    let lake_cuts: Vec<(String, Cell, Area)> = pace::par_map(lake_cells.into_iter().collect(), |c| {
        let mine: Vec<(&String, Area)> =
            made.iter().map(|(id, m)| (id, m.lakes.get(c))).filter(|(_, a)| !a.is_empty()).collect();
        let waters: Vec<(&String, Area)> =
            made.iter().map(|(id, m)| (id, m.water.get(c))).filter(|(_, a)| !a.is_empty()).collect();
        mine.into_iter()
            .map(|(id, lk)| {
                let o: Vec<Area> = waters.iter().filter(|(w, _)| *w != id).map(|(_, a)| a.clone()).collect();
                (id.clone(), c, if o.is_empty() { lk } else { lk.minus(&geom::union_all(&o)) })
            })
            .collect::<Vec<_>>()
    })
    .into_iter()
    .flatten()
    .collect();
    for (id, c, a) in lake_cuts {
        made.get_mut(&id).unwrap().lakes.set(c, a);
    }
    // Each region's area by cell: land, water, lakes.
    // (Fanned out over (region, cell) pairs, not regions: one large region still uses every thread
    // allowed, its cells' unions being the work.)
    let pairs: Vec<(&String, &Made, Cell)> = made
        .iter()
        .flat_map(|(id, m)| {
            let cells: BTreeSet<Cell> =
                m.land.cells.keys().chain(m.water.cells.keys()).chain(m.lakes.cells.keys()).copied().collect();
            cells.into_iter().map(move |c| (id, m, c))
        })
        .collect();
    let mut areas: BTreeMap<String, TArea> = made.keys().map(|id| (id.clone(), TArea::default())).collect();
    for (id, c, a) in
        pace::par_map(pairs, |(id, m, c)| (id, c, m.land.get(c).union(&m.water.get(c)).union(&m.lakes.get(c))))
    {
        areas.get_mut(id).unwrap().set(c, a);
    }
    // (The lands kept only for the slivers' rule; otherwise only whether each is empty.)
    let cover = rules.cover.as_deref() == Some("earth");
    let land_empty: BTreeMap<String, bool> = made.iter().map(|(id, m)| (id.clone(), m.land.is_empty())).collect();
    let lands: BTreeMap<String, TArea> =
        if cover { made.iter().map(|(id, m)| (id.clone(), m.land.clone())).collect() } else { BTreeMap::new() };
    let countries: BTreeMap<String, String> = made.iter().map(|(id, m)| (id.clone(), m.country.clone())).collect();
    drop(made);
    trace("hand-outs");
    // 4. Hand-outs.
    for id in &ids {
        let Some(g) = folder.regions[id].1.hand_out.clone() else { continue };
        if !areas.contains_key(id) {
            continue;
        }
        // (Only regions with water take a hand-out: those without resolve as they always have.)
        let (members, old): (Vec<String>, Vec<String>) = folder
            .members(&g, id)
            .into_iter()
            .filter(|m| areas.contains_key(m) || others.contains_key(m))
            .partition(|m| folder.regions[m].1.has_water());
        if let Some(o) = old.first() {
            let (p, r) = &folder.regions[id];
            out.problems.push(problem("rg-ref", r, p, format!("{id}: hand_out \"{g}\" has region {o} without water (and {} more): a hand-out gives only to regions with water", old.len() - 1)));
        }
        let every = folder.regions[id].1.hand_out_all;
        let mine = areas[id].clone();
        let theirs: Vec<(&String, &TArea)> = members
            .iter()
            // (`hand_out_all` hands out land: only to regions with land.)
            .filter(|m| !every || !land_empty.get(*m).copied().unwrap_or(false))
            .map(|m| (m, areas.get(m).unwrap_or_else(|| &others[m])))
            .collect();
        // Each stretch's edge with each member (the stretch's ring of 100 m made once a cell), and
        // where it goes.
        let moves: Vec<(String, BTreeMap<Cell, Area>)> = pace::par_map(components(&mine), |comp| {
            let km2: f64 = comp.values().map(geom::area_km2).sum();
            let mut edge: Vec<f64> = vec![0.0; theirs.len()];
            for (c, p) in &comp {
                let mut ring: Option<Area> = None;
                for (k, (_, t)) in theirs.iter().enumerate() {
                    edge[k] += edge_in(p, *c, t, &mut ring);
                }
            }
            let mut touching: Vec<(f64, &String)> =
                edge.iter().zip(&theirs).filter(|(l, _)| **l > 0.0).map(|(l, (m, _))| (*l, *m)).collect();
            touching.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
            let to = match touching.len() {
                1 => Some(touching[0].1.clone()),
                n if n > 1 && (every || km2 < rules.sliver_km2) => Some(touching[0].1.clone()),
                0 if every => nearest_member(&comp, &theirs),
                _ => None,
            };
            to.map(|m| (m, comp))
        })
        .into_iter()
        .flatten()
        .collect();
        // Applied cell by cell: taken from this region, given to each member.
        let mut given: BTreeMap<String, BTreeMap<Cell, Vec<Area>>> = BTreeMap::new();
        let mut taken: BTreeMap<Cell, Vec<Area>> = BTreeMap::new();
        for (m, comp) in moves {
            for (c, p) in comp {
                taken.entry(c).or_default().push(p.clone());
                given.entry(m.clone()).or_default().entry(c).or_default().push(p);
            }
        }
        let a = areas.get_mut(id).unwrap();
        for (c, ps) in taken {
            let left = a.get(c).minus(&geom::union_all(&ps));
            a.set(c, left);
        }
        for (m, cells) in given {
            let mut b = areas.remove(&m).unwrap_or_else(|| others[&m].clone());
            for (c, mut ps) in cells {
                ps.push(b.get(c));
                b.set(c, geom::union_all(&ps));
            }
            areas.insert(m, b);
        }
    }
    trace("with cover");
    // 5. With cover = "earth": slivers, then the cells' fills and the check; the rest.
    // (Every region's cells, moved together, not copied.)
    let mut all: BTreeMap<String, TArea> = others;
    all.extend(areas);
    if cover {
        slivers(&ids, folder, &lands, rules.sliver_km2, &mut all);
    }
    drop(lands);
    trace("the cells' fills and the cover check");
    let rest_id = rests.first().map(|s| (*s).clone());
    let rest = cells_pass(folder, data, &mut all, cover, rest_id.is_some(), rules.sliver_km2, &mut out.problems);
    trace("stored");
    // 6. Stored: every region with water, and the rest (those without, as they resolved).
    for (id, t) in &all {
        if rest_id.as_ref() == Some(id) {
            continue;
        }
        let (p, r) = &folder.regions[id];
        if !r.has_water() {
            continue;
        }
        if r.hand_out_all {
            // (The holding region ends empty; anything left had no member to go to.)
            if !t.is_empty() {
                let v = t.cells.values().flat_map(|a| a.polys.iter()).next().map(|p| p[0][0]).unwrap_or_default();
                out.problems.push(problem(
                    "rg-hand-out",
                    r,
                    p,
                    format!(
                        "{id}: {:.1} km² by {:.4}°, {:.4}° found no region of \"{}\" with land to go to (hand_out_all)",
                        t.km2(),
                        v[0] as f64 / 1e7,
                        v[1] as f64 / 1e7,
                        r.hand_out.as_deref().unwrap_or_default()
                    ),
                ));
            }
            out.resolved.remove(id);
            continue;
        }
        if t.is_empty() {
            out.problems.push(problem("rg-empty", r, p, format!("{id}: the region has neither land nor water")));
            out.resolved.remove(id);
            continue;
        }
        if let Some(res) = out.resolved.get_mut(id) {
            if let Some(c) = countries.get(id).filter(|c| !c.is_empty()) {
                res.geometry.shapes = vec![Shape {
                    from: String::new(),
                    country: c.clone(),
                    tolerance_m: 0,
                    rings: Vec::new(),
                    water: false,
                }];
            }
            store(r, res, t, data);
        }
    }
    if let Some(id) = rest_id {
        let (p, r) = &folder.regions[&id];
        match out.resolved.get_mut(&id) {
            Some(res) if !rest.is_empty() => store(r, res, &rest, data),
            _ => {
                out.problems.push(problem(
                    "rg-empty",
                    r,
                    p,
                    format!("{id}: the rest is empty: the other regions cover the Earth"),
                ));
                out.resolved.remove(&id);
            }
        }
    }
}

/// With `SCENIC_REGIONS_TRACE` set: each step's start, the process's footprint, and the cells'
/// buffers (wide: past 200 km, Antarctica's 200 nmi) and nearest-coast splits so far (how many, their
/// time summed over threads, the most footprint seen as one ended), on stderr.
fn trace(step: &str) {
    if std::env::var_os("SCENIC_REGIONS_TRACE").is_some() {
        let f = |c: &Counter| {
            use std::sync::atomic::Ordering::Relaxed;
            format!(
                "{} in {:.0} s, up to {} MB",
                c.0.load(Relaxed),
                c.1.load(Relaxed) as f64 / 1000.0,
                c.2.load(Relaxed)
            )
        };
        eprintln!(
            "regions: {step}, footprint {} MB; buffers {}; wide buffers {}; nearest-coast splits {}",
            crate::sys::footprint().unwrap_or(0) / 1_000_000,
            f(&BUFFERS),
            f(&WIDE_BUFFERS),
            f(&NEARESTS)
        );
    }
}

/// How many, milliseconds, the most footprint (MB) seen as one ended.
type Counter = (std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64);
static BUFFERS: Counter =
    (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0));
static WIDE_BUFFERS: Counter =
    (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0));
static NEARESTS: Counter =
    (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0));

struct Timed(&'static Counter, std::time::Instant);

impl Timed {
    fn new(c: &'static Counter) -> Timed {
        Timed(c, std::time::Instant::now())
    }
}

impl Drop for Timed {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.0 .0.fetch_add(1, Relaxed);
        self.0 .1.fetch_add(self.1.elapsed().as_millis() as u64, Relaxed);
        if std::env::var_os("SCENIC_REGIONS_TRACE").is_some() {
            self.0 .2.fetch_max(crate::sys::footprint().unwrap_or(0) / 1_000_000, Relaxed);
        }
    }
}

/// A stored region's geometry by cell.
fn geometry_tiled(res: &Resolved) -> TArea {
    let mut t = TArea::default();
    for s in &res.geometry.shapes {
        let rs: Vec<Ring> = s.rings.iter().map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect()).collect();
        let u = TArea::from_rings(&rs);
        t = if t.cells.is_empty() { u } else { t.zip(&u, |a, b| a.union(b)) };
    }
    t
}

#[allow(clippy::too_many_arguments)]
fn env_for<'a>(
    id: &str,
    water: bool,
    folder: &Folder,
    ctx: &super::resolve::Context,
    made: &'a BTreeMap<String, Made>,
    others: &'a BTreeMap<String, TArea>,
    out: &Outcome,
    needs: &mut BTreeSet<Id>,
) -> Result<Env<'a>, Vec<Problem>> {
    let (path, r) = &folder.regions[id];
    // (Other regions' geometry for gather: those without water as stored; ours aren't read from
    // there, but from `made` below. What the extract lacks kept in `needs`: a region with water
    // waits for the extract too.)
    let (leaves, _, _, ps) = super::resolve::gather(r, path, folder, ctx, &out.resolved, &out.resolved, water, needs)?;
    if !ps.is_empty() {
        return Err(ps);
    }
    // Leaves of other regions and groups: by cell, from what's made (land in the outline, all of
    // it in the water) or stored.
    let country = r
        .parts_of(water)
        .into_iter()
        .find_map(|p| match (p, leaves.get(&p.to_string())) {
            (Part::Osm(_), Some(Leaf::Rings(_, c))) if !c.is_empty() => Some(c.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut env = Env { leaves: HashMap::new(), country, made, others, buffered: Default::default() };
    let (theirs, rest): (BTreeMap<String, Leaf>, BTreeMap<String, Leaf>) = leaves
        .into_iter()
        .partition(|(k, _)| k.starts_with("region:") || k.starts_with("land:") || k.starts_with("group:"));
    trace(&format!("{id}: its parts gathered"));
    env.leaves = tiled_leaves(rest);
    // (A `region:` or `land:` naming no region of the folder is an error, never read as empty.)
    let mut missing: Vec<String> = r
        .parts_of(water)
        .into_iter()
        .filter(|p| matches!(p, Part::Region(d) | Part::Land(d) if !folder.regions.contains_key(d)))
        .map(|p| p.to_string())
        .collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(vec![problem("rg-ref", r, path, format!("{id}: {} names no region", missing.join(", ")))]);
    }
    for k in theirs.keys() {
        let src = if let Some(d) = k.strip_prefix("region:") {
            Src::Refs(vec![d.to_string()], water)
        } else if let Some(d) = k.strip_prefix("land:") {
            Src::Refs(vec![d.to_string()], false)
        } else {
            Src::Refs(folder.members(&k["group:".len()..], id), water)
        };
        env.leaves.insert(k.clone(), src);
    }
    Ok(env)
}

#[allow(clippy::too_many_arguments)]
fn make_land(
    id: &str,
    folder: &Folder,
    ctx: &super::resolve::Context,
    data: &super::data::Datasets,
    made: &BTreeMap<String, Made>,
    others: &BTreeMap<String, TArea>,
    out: &Outcome,
    held: &BTreeSet<String>,
    needs: &mut BTreeSet<Id>,
) -> Result<Made, Vec<Problem>> {
    let (path, r) = &folder.regions[id];
    if let Some(d) = folder.deps(r).into_iter().find(|d| held.contains(d)) {
        return Err(vec![problem("rg-ref", r, path, format!("{id}: region:{d} is held, and so this region with it"))]);
    }
    let env = env_for(id, false, folder, ctx, made, others, out, needs)?;
    let mut cells: BTreeSet<Cell> = r.outline.iter().flat_map(|p| support(p, &env)).collect();
    if !r.intersect.is_empty() {
        let i: BTreeSet<Cell> = r.intersect.iter().flat_map(|p| support(p, &env)).collect();
        cells = cells.intersection(&i).copied().collect();
    }
    let got: Vec<Result<(Cell, Area, Area), Problem>> = pace::par_map(cells.into_iter().collect(), |c| {
        geom::set_label(format!("the land of {id}, the cell from {}°, {}°", c.0 * 2, c.1 * 2));
        let o = geom::union_all(&r.outline.iter().map(|p| eval(p, c, &env, r, path)).collect::<Result<Vec<_>, _>>()?);
        let o = if r.intersect.is_empty() || o.is_empty() {
            o
        } else {
            o.intersect(&geom::union_all(
                &r.intersect.iter().map(|p| eval(p, c, &env, r, path)).collect::<Result<Vec<_>, _>>()?,
            ))
        };
        let o = if r.minus.is_empty() || o.is_empty() {
            o
        } else {
            o.minus(&geom::union_all(
                &r.minus.iter().map(|p| eval(p, c, &env, r, path)).collect::<Result<Vec<_>, _>>()?,
            ))
        };
        if o.is_empty() {
            return Ok((c, o.clone(), o));
        }
        let (land, lakes) = land_and_lakes(data, c);
        Ok((c, o.intersect(&land), o.intersect(&lakes)))
    });
    let mut m =
        Made { land: TArea::default(), lakes: TArea::default(), water: TArea::default(), country: env.country.clone() };
    let mut ps = Vec::new();
    for g in got {
        match g {
            Ok((c, l, k)) => {
                m.land.set(c, l);
                m.lakes.set(c, k);
            }
            Err(p) => ps.push(p),
        }
    }
    if !ps.is_empty() {
        ps.truncate(1);
        return Err(ps);
    }
    Ok(m)
}

#[allow(clippy::too_many_arguments)]
fn make_water(
    id: &str,
    folder: &Folder,
    ctx: &super::resolve::Context,
    data: &super::data::Datasets,
    made: &BTreeMap<String, Made>,
    others: &BTreeMap<String, TArea>,
    out: &Outcome,
    held: &BTreeSet<String>,
    needs: &mut BTreeSet<Id>,
) -> Result<TArea, Vec<Problem>> {
    let (path, r) = &folder.regions[id];
    if r.water.is_empty() {
        return Ok(TArea::default());
    }
    let env = env_for(id, true, folder, ctx, made, others, out, needs)?;
    let _ = held;
    trace(&format!("water of {id}: its parts read"));
    let cells: BTreeSet<Cell> = r.water.iter().flat_map(|p| support(p, &env)).collect();
    trace(&format!("water of {id}: {} cells", cells.len()));
    let got: Vec<Result<(Cell, Area), Problem>> = pace::par_map(cells.into_iter().collect(), |c| {
        geom::set_label(format!("the water of {id}, the cell from {}°, {}°", c.0 * 2, c.1 * 2));
        let w = geom::union_all(&r.water.iter().map(|p| eval(p, c, &env, r, path)).collect::<Result<Vec<_>, _>>()?);
        if w.is_empty() {
            return Ok((c, w));
        }
        // (Less the land, and every region without water.)
        let mut cut = vec![land_and_lakes(data, c).0];
        cut.extend(others.values().map(|t| t.get(c)).filter(|a| !a.is_empty()));
        Ok((c, w.minus(&geom::union_all(&cut))))
    });
    let mut t = TArea::default();
    let mut ps = Vec::new();
    for g in got {
        match g {
            Ok((c, a)) => t.set(c, a),
            Err(p) => ps.push(p),
        }
    }
    if !ps.is_empty() {
        ps.truncate(1);
        return Err(ps);
    }
    Ok(t)
}

/// The stretches of a tiled area: its cells' polygons, joined where two in neighbouring cells
/// share a stretch of the cells' edge.
pub fn components(t: &TArea) -> Vec<BTreeMap<Cell, Area>> {
    // (Each polygon, by cell and index; its intervals on each side of its cell.)
    let mut polys: Vec<(Cell, Vec<Ring>)> = Vec::new();
    for (c, a) in &t.cells {
        for p in &a.polys {
            polys.push((*c, p.clone()));
        }
    }
    // Intervals on the east and north sides, matched with the neighbour's west and south.
    let side = |ring: &Ring, k: usize, v: i64| -> Vec<(i64, i64)> {
        let n = ring.len();
        let o = 1 - k;
        (0..n)
            .filter_map(|i| {
                let (p, q) = (ring[i], ring[(i + 1) % n]);
                (p[k] == v && q[k] == v && p[o] != q[o]).then(|| (p[o].min(q[o]), p[o].max(q[o])))
            })
            .collect()
    };
    let mut parent: Vec<usize> = (0..polys.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut by_cell: HashMap<Cell, Vec<usize>> = HashMap::new();
    for (i, (c, _)) in polys.iter().enumerate() {
        by_cell.entry(*c).or_default().push(i);
    }
    for (i, (c, p)) in polys.iter().enumerate() {
        let b = cell_box(*c);
        for (k, v, nc) in [(0usize, b[2], (wrap(c.0 + 1).0, c.1)), (1usize, b[3], (c.0, c.1 + 1))] {
            let mine: Vec<(i64, i64)> = p.iter().flat_map(|r| side(r, k, v)).collect();
            if mine.is_empty() {
                continue;
            }
            let Some(js) = by_cell.get(&nc) else { continue };
            // (The neighbour's west or south side; across ±180°, its west side is at −180°.)
            let nv = if k == 0 { cell_box(nc)[0] } else { cell_box(nc)[1] };
            for &j in js {
                let theirs: Vec<(i64, i64)> = polys[j].1.iter().flat_map(|r| side(r, k, nv)).collect();
                if mine.iter().any(|a| theirs.iter().any(|b| a.0 < b.1 && b.0 < a.1)) {
                    let (ra, rb) = (find(&mut parent, i), find(&mut parent, j));
                    if ra != rb {
                        parent[ra.max(rb)] = ra.min(rb);
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, BTreeMap<Cell, Vec<Vec<Ring>>>> = BTreeMap::new();
    for i in 0..polys.len() {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().entry(polys[i].0).or_default().push(polys[i].1.clone());
    }
    groups.into_values().map(|g| g.into_iter().map(|(c, ps)| (c, Area { polys: ps })).collect()).collect()
}

/// The edge two tiled areas share (km): each cell's piece of `a` against `b` there and in the
/// neighbouring cells.
pub fn shared_km(a: &TArea, b: &TArea) -> f64 {
    // (Summed in the cells' order: the same total whatever the threads' schedule.)
    let parts: Vec<f64> = pace::par_map(a.cells.iter().collect(), |(c, p)| edge_in(p, *c, b, &mut None));
    parts.iter().sum()
}

/// The edge (km) piece `p` of cell `c` shares with `t`'s pieces in the cells round it, its ring of
/// 100 m made once (in `ring`) and only when one comes near.
fn edge_in(p: &Area, c: Cell, t: &TArea, ring: &mut Option<Area>) -> f64 {
    let Some(pb) = p.bbox() else { return 0.0 };
    let pad = 20_000;
    let mut sum = 0.0;
    for x in c.0 - 1..=c.0 + 1 {
        let (w, shift) = wrap(x);
        for y in c.1 - 1..=c.1 + 1 {
            let Some(q) = t.cells.get(&(w, y)) else { continue };
            let Some(qb) = q.bbox().map(|b| [b[0] + shift, b[1], b[2] + shift, b[3]]) else { continue };
            if !(pb[0] <= qb[2] + pad && qb[0] <= pb[2] + pad && pb[1] <= qb[3] + pad && qb[1] <= pb[3] + pad) {
                continue;
            }
            let q = if shift == 0 { q.clone() } else { q.shifted(shift) };
            let r = ring.get_or_insert_with(|| geom::buffer_area_coarse(p, 100.0).minus(p));
            sum += geom::area_km2(&r.intersect(&q)) / 0.1;
        }
    }
    sum
}

/// With `cover`: a region's stretches of water under `km2` touching none of its land go to the
/// region sharing the longest edge.
fn slivers(
    ids: &[String],
    folder: &Folder,
    lands: &BTreeMap<String, TArea>,
    km2: f64,
    all: &mut BTreeMap<String, TArea>,
) {
    if km2 <= 0.0 {
        return;
    }
    // (Which regions have a piece in each cell.)
    let mut index: HashMap<Cell, Vec<String>> = HashMap::new();
    for (id, t) in all.iter() {
        for c in t.cells.keys() {
            index.entry(*c).or_default().push(id.clone());
        }
    }
    for id in ids {
        if folder.regions[id].1.is_rest() || !all.contains_key(id) {
            continue;
        }
        let land = lands.get(id).cloned().unwrap_or_default();
        // (The stretches of its water alone: its land taken out cell by cell first, so its
        // islands aren't pieces to test, each against its land.)
        let water = all[id].zip(&land, |a, l| if l.is_empty() { a.clone() } else { a.minus(l) });
        let comps = components(&water);
        // (A region all of whose area is one piece of water keeps it.)
        if comps.len() + usize::from(!land.is_empty()) < 2 {
            continue;
        }
        // (Each piece decided at once, the cheap tests first: its size, whether it's land, then
        // its edges; then moved.)
        let moves: Vec<(String, BTreeMap<Cell, Area>)> = pace::par_map(comps, |comp| {
            let piece = TArea { cells: comp };
            if piece.km2() >= km2 || shared_km(&piece, &land) > 0.0 {
                return None;
            }
            // (Only the regions with cells round the piece's: by the cells' index.)
            let mut near: BTreeSet<&String> = BTreeSet::new();
            for c in piece.cells.keys() {
                for x in c.0 - 1..=c.0 + 1 {
                    for y in c.1 - 1..=c.1 + 1 {
                        if let Some(v) = index.get(&(wrap(x).0, y)) {
                            near.extend(v.iter());
                        }
                    }
                }
            }
            near.into_iter()
                .filter(|o| *o != id && folder.regions[*o].1.has_water())
                .map(|o| (shared_km(&piece, &all[o]), o.clone()))
                .filter(|(l, _)| *l > 0.0)
                .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)))
                .map(|(_, o)| (o, piece.cells))
        })
        .into_iter()
        .flatten()
        .collect();
        for (o, cells) in moves {
            for (c, p) in &cells {
                let a = all.get_mut(id).unwrap();
                let left = a.get(*c).minus(p);
                a.set(*c, left);
                let b = all.get_mut(&o).unwrap();
                let more = b.get(*c).union(p);
                b.set(*c, more);
            }
        }
    }
}

/// Cell by cell: with `cover`, land in two regions taken from both, land and land-touching water
/// in none to the region sharing the longest edge in the cell, `rg-cover` for water in two or (with
/// no rest region) in none; the rest, what no region covers.
fn cells_pass(
    folder: &Folder,
    data: &super::data::Datasets,
    all: &mut BTreeMap<String, TArea>,
    cover: bool,
    rest: bool,
    fill_km2: f64,
    problems: &mut Vec<Problem>,
) -> TArea {
    if !cover && !rest {
        return TArea::default();
    }
    let cells: Vec<Cell> = (-90..90).flat_map(|x| (-45..45).map(move |y| (x, y))).collect();
    type One = (Cell, Vec<(String, Area)>, Area, Vec<Problem>);
    let done: Vec<One> = pace::par_map(cells, |c| {
        geom::set_label(format!("the cover check, the cell from {}°, {}°", c.0 * 2, c.1 * 2));
        {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n % 2000 == 0 {
                trace(&format!("the cover check: {n} of 16,200 cells"));
            }
        }
        let bb = geom::box_area(cell_box(c));
        let mut pieces: Vec<(String, Area)> =
            all.iter().filter_map(|(id, t)| t.cells.get(&c).map(|a| (id.clone(), a.clone()))).collect();
        let mut ps = Vec::new();
        if !cover {
            let u = geom::union_all(&pieces.iter().map(|p| p.1.clone()).collect::<Vec<_>>());
            return (c, Vec::new(), bb.minus(&u), ps);
        }
        let t0 = std::time::Instant::now();
        let land = land_and_lakes(data, c).0;
        let mut changed: BTreeSet<String> = BTreeSet::new();
        let mut twice = Vec::new();
        let boxes: Vec<Option<[i64; 4]>> = pieces.iter().map(|p| p.1.bbox()).collect();
        let near = |a: &Option<[i64; 4]>, b: &Option<[i64; 4]>| match (a, b) {
            (Some(a), Some(b)) => a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3],
            _ => false,
        };
        for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                if !near(&boxes[i], &boxes[j]) {
                    continue;
                }
                let o = pieces[i].1.intersect(&pieces[j].1);
                if o.is_empty() {
                    continue;
                }
                let sea = o.minus(&land);
                let km2 = geom::area_km2(&sea);
                if km2 > 0.1 {
                    let (a, b) = (&pieces[i].0, &pieces[j].0);
                    ps.push(Problem {
                            check: "rg-cover",
                            about: vec![a.clone(), b.clone()],
                            found: vec!["overlap".into()],
                            files: [a, b].iter().filter_map(|x| folder.regions.get(*x).map(|r| r.0.clone())).collect(),
                            message: format!("{a} and {b} both cover {km2:.1} km² of water in the cell from {}°, {}°: water is in one region", c.0 * 2, c.1 * 2),
                        });
                }
                twice.push(o.intersect(&land));
            }
        }
        let twice = geom::union_all(&twice);
        if !twice.is_empty() {
            for p in pieces.iter_mut().filter(|p| folder.regions[&p.0].1.has_water()) {
                let less = p.1.minus(&twice);
                if less != p.1 {
                    p.1 = less;
                    changed.insert(p.0.clone());
                }
            }
        }
        let t1 = t0.elapsed().as_secs_f64();
        let u = geom::union_all(&pieces.iter().map(|p| p.1.clone()).collect::<Vec<_>>());
        let gaps = bb.minus(&u);
        let t2 = t0.elapsed().as_secs_f64();
        let mut sea_left = Vec::new();
        // (Each gap goes to the region it shares the most edge with, if it's within 100 m of
        // land: read from an index of the segments, no buffer per gap.)
        let pidx = SegIndex::new(
            pieces.iter().enumerate().flat_map(|(k, (_, a))| a.polys.iter().flatten().map(move |r| (k, r))),
        );
        let lidx = SegIndex::new(land.polys.iter().enumerate().flat_map(|(k, p)| p.iter().map(move |r| (k, r))));
        let land_boxes: Vec<Option<[i64; 4]>> = land.polys.iter().map(|p| geom::bbox_of(p.iter().flatten())).collect();
        let mut won: Vec<Vec<Area>> = vec![Vec::new(); pieces.len()];
        let mut sample: Vec<(f64, String)> = Vec::new();
        let mut big: Vec<(f64, geom::Pt)> = Vec::new();
        for poly in &gaps.polys {
            let g = Area { polys: vec![poly.clone()] };
            let gb = geom::bbox_of(poly.iter().flatten());
            let touches = poly.iter().flatten().any(|v| lidx.within(*v, 100.0))
                || poly
                    .first()
                    .and_then(|r| r.first())
                    .is_some_and(|v| land.polys.iter().zip(&land_boxes).any(|(lp, b)| near(b, &gb) && inside(*v, lp)))
                || land.polys.iter().zip(&land_boxes).any(|(lp, b)| {
                    near(b, &gb) && lp.first().and_then(|r| r.first()).is_some_and(|v| inside(*v, poly))
                });
            if !touches {
                sea_left.push(g);
                continue;
            }
            let mut len = vec![0.0f64; pieces.len()];
            for r in poly {
                for k in 0..r.len() {
                    let (a, b) = (r[k], r[(k + 1) % r.len()]);
                    let m = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
                    if let Some(owner) = pidx.on(m) {
                        len[owner] += seg_m(a, b);
                    }
                }
            }
            // (A gap over `fill_km2` isn't given to a neighbour: open sea by a coast is the
            // rest's, and land in no region that large is an error.)
            let km2 = geom::area_km2(&g);
            if km2 > fill_km2 {
                let on_land = geom::area_km2(&g.intersect(&land));
                if on_land > fill_km2 {
                    big.push((on_land, poly[0][0]));
                }
                sea_left.push(g);
                continue;
            }
            let best = (0..pieces.len())
                .filter(|&k| len[k] > 0.0 && folder.regions[&pieces[k].0].1.has_water())
                .max_by(|&x, &y| len[x].total_cmp(&len[y]).then(pieces[y].0.cmp(&pieces[x].0)));
            if sample.len() < 400 {
                let km2 = geom::area_km2(&g);
                let v = poly[0][0];
                let by: Vec<String> = (0..pieces.len())
                    .filter(|&k| len[k] > 0.0)
                    .map(|k| format!("{} {:.0} m", pieces[k].0, len[k]))
                    .collect();
                sample.push((
                    km2,
                    format!(
                        "{:.5},{:.5} {:.6} km² land-near borders [{}]",
                        v[0] as f64 / 1e7,
                        v[1] as f64 / 1e7,
                        km2,
                        by.join("; ")
                    ),
                ));
            }
            match best {
                Some(k) => won[k].push(g),
                None => sea_left.push(g),
            }
        }
        for (k, gs) in won.into_iter().enumerate() {
            if gs.is_empty() {
                continue;
            }
            let mut all = gs;
            all.push(pieces[k].1.clone());
            pieces[k].1 = geom::union_all(&all);
            changed.insert(pieces[k].0.clone());
        }
        let sea = geom::union_all(&sea_left);
        let t3 = t0.elapsed().as_secs_f64();
        if t3 > 2.0 || (gaps.polys.len() > 3000 && std::env::var("SCENIC_REGIONS_GAPS").is_ok()) {
            trace(&format!(
                    "the cover check: the cell from {}°, {}° took {t3:.0} s ({} regions, land {} rings, overlaps {t1:.0} s, union {:.0} s, {} gaps {:.0} s)",
                    c.0 * 2, c.1 * 2, pieces.len(), land.polys.len(), t2 - t1, gaps.polys.len(), t3 - t2
                ));
            if std::env::var("SCENIC_REGIONS_GAPS").is_ok() {
                let mut a: Vec<f64> = sample.iter().map(|x| x.0).collect();
                a.sort_by(|x, y| x.total_cmp(y));
                let tot: f64 = a.iter().sum();
                trace(&format!(
                    "gaps sampled {} (land-near), total {tot:.4} km², median {:.7} km², max {:.4} km²",
                    a.len(),
                    a.get(a.len() / 2).copied().unwrap_or(0.0),
                    a.last().copied().unwrap_or(0.0)
                ));
                for (_, l) in sample.iter().step_by((sample.len() / 8).max(1)) {
                    trace(&format!("  gap {l}"));
                }
            }
        }
        for (km2, v) in &big {
            ps.push(Problem {
                    check: "rg-cover",
                    about: vec![format!("{},{}", c.0 * 2, c.1 * 2)],
                    found: vec!["gap".into()],
                    files: vec![super::resolve::RULES.into()],
                    message: format!("{km2:.1} km² of land by {:.4}°, {:.4}° is in no region, over sliver_km2 ({fill_km2}) for the fill to give it to a neighbour", v[0] as f64 / 1e7, v[1] as f64 / 1e7),
                });
        }
        if !rest && geom::area_km2(&sea) > 0.1 {
            ps.push(Problem {
                check: "rg-cover",
                about: vec![format!("{},{}", c.0 * 2, c.1 * 2)],
                found: vec!["gap".into()],
                files: vec![super::resolve::RULES.into()],
                message: format!(
                    "{:.1} km² of water in the cell from {}°, {}° is in no region, and no region's water is the rest",
                    geom::area_km2(&sea),
                    c.0 * 2,
                    c.1 * 2
                ),
            });
        }
        let out: Vec<(String, Area)> = pieces.into_iter().filter(|p| changed.contains(&p.0)).collect();
        (c, out, sea, ps)
    });
    let mut rest_t = TArea::default();
    for (c, changed, sea, ps) in done {
        for (id, a) in changed {
            all.get_mut(&id).unwrap().set(c, a);
        }
        rest_t.set(c, sea);
        problems.extend(ps);
    }
    rest_t
}

/// The member nearest a stretch touching none (`hand_out_all`): the least distance between the
/// stretch's points and a member's in the cells within two of the stretch's, ties to the lesser id.
fn nearest_member(comp: &BTreeMap<Cell, Area>, theirs: &[(&String, &TArea)]) -> Option<String> {
    let pts: Vec<geom::Pt> = comp.values().flat_map(|a| a.polys.iter().flatten().flatten().copied()).collect();
    let mut best: Option<(f64, &String)> = None;
    for (m, t) in theirs {
        let mut d = f64::MAX;
        let mut seen = BTreeSet::new();
        for c in comp.keys() {
            for x in c.0 - 2..=c.0 + 2 {
                // (Across ±180° the member's cell is wrapped and its points shifted to this side.)
                let (w, shift) = wrap(x);
                for y in (c.1 - 2).max(-45)..=(c.1 + 2).min(44) {
                    if !seen.insert((w, y)) {
                        continue;
                    }
                    let Some(a) = t.cells.get(&(w, y)) else { continue };
                    for q in a.polys.iter().flatten().flatten() {
                        let q = [q[0] + shift, q[1]];
                        for p in &pts {
                            d = d.min(seg_m(*p, q));
                        }
                    }
                }
            }
        }
        if d < f64::MAX && best.is_none_or(|b| d < b.0 || (d == b.0 && *m < b.1)) {
            best = Some((d, m));
        }
    }
    best.map(|b| b.1.clone())
}

/// The cells' pieces as one area: drawn apart, each would be outlined along the cell edges it
/// shares with its neighbours.
pub(super) fn dissolved(t: &TArea) -> Area {
    geom::union_all(&t.cells.values().cloned().collect::<Vec<_>>())
}

/// A region's stored geometry and drawing: its cells' rings, its `from` its parts; a shape of its
/// land (the coastline's, less the lakes over 100 km²) and one of its water, worked out cell by cell
/// from what it ends with (hand-outs, slivers and fills included: a region whose own parts had no
/// land may be given some), either left out when empty. Drawn as one.
fn store(r: &Region, res: &mut Resolved, t: &TArea, data: &super::data::Datasets) {
    let mut from = r.outline_text().join(", ");
    if !r.intersect.is_empty() {
        from += &format!(" ∩ {}", r.intersect.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
    }
    if !r.minus.is_empty() {
        from += &format!(" − {}", r.minus.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
    }
    let country = res.geometry.shapes.first().map(|s| s.country.clone()).unwrap_or_default();
    let rings32 = |t: &TArea| -> Vec<Vec<[i32; 2]>> { t.rings().iter().map(|r| super::shapes::to_i32(r)).collect() };
    let tol = crate::coverage::draw_tolerance_m(&rings32(t));
    let polys: Vec<Vec<Vec<[f64; 2]>>> = dissolved(t)
        .polys
        .iter()
        .map(|p| {
            p.iter()
                .map(|r| crate::coverage::ring_degrees(&crate::outlines::simplify(&super::shapes::to_i32(r), tol)))
                .collect()
        })
        .collect();
    let shape = |from: String, rings, water| Shape { from, country: country.clone(), tolerance_m: 0, rings, water };
    let cells: Vec<(Cell, Area)> = t.cells.iter().map(|(c, a)| (*c, a.clone())).collect();
    let (mut land, mut water) = (TArea::default(), TArea::default());
    for (c, a, l) in pace::par_map(cells, |(c, a)| {
        let l = a.intersect(&land_and_lakes(data, c).0);
        (c, a, l)
    }) {
        water.set(c, a.minus(&l));
        land.set(c, l);
    }
    res.geometry.shapes = [(from.clone(), land, false), (format!("{from} (water)"), water, true)]
        .into_iter()
        .filter(|(_, t, _)| !t.is_empty())
        .map(|(f, t, w)| shape(f, rings32(&t), w))
        .collect();
    res.drawn.shapes = BTreeMap::from([(from, polys)]);
    res.lakes = Area::default();
}

/// Metres along a segment (a local equirectangular measure: segments here are short).
fn seg_m(a: geom::Pt, b: geom::Pt) -> f64 {
    let lat = ((a[1] + b[1]) as f64 / 2e7).to_radians();
    let dx = (b[0] - a[0]) as f64 * lat.cos();
    let dy = (b[1] - a[1]) as f64;
    (dx * dx + dy * dy).sqrt() * 0.011_131_95
}

/// Whether a point is inside a polygon (its rings read even-odd, holes included).
fn inside(p: geom::Pt, poly: &[geom::Ring]) -> bool {
    let mut c = false;
    for r in poly {
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + n - 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = a[0] as f64 + (p[1] - a[1]) as f64 * (b[0] - a[0]) as f64 / (b[1] - a[1]) as f64;
                if (p[0] as f64) < x {
                    c = !c;
                }
            }
        }
    }
    c
}

/// Rings' segments in a grid of about 2 km, each with its owner's number.
struct SegIndex {
    grid: std::collections::HashMap<(i64, i64), Vec<(usize, geom::Pt, geom::Pt)>>,
}

const SEG_CELL: i64 = 20_000;

impl SegIndex {
    fn new<'a>(rings: impl Iterator<Item = (usize, &'a geom::Ring)>) -> Self {
        let mut grid: std::collections::HashMap<(i64, i64), Vec<(usize, geom::Pt, geom::Pt)>> = Default::default();
        for (k, r) in rings {
            let n = r.len();
            for i in 0..n {
                let (a, b) = (r[i], r[(i + 1) % n]);
                for x in a[0].min(b[0]).div_euclid(SEG_CELL)..=a[0].max(b[0]).div_euclid(SEG_CELL) {
                    for y in a[1].min(b[1]).div_euclid(SEG_CELL)..=a[1].max(b[1]).div_euclid(SEG_CELL) {
                        grid.entry((x, y)).or_default().push((k, a, b));
                    }
                }
            }
        }
        SegIndex { grid }
    }

    /// The segments within `d` metres of a point, with their distance in metres.
    fn near(&self, p: geom::Pt, d: f64) -> impl Iterator<Item = (usize, f64)> + '_ {
        let cos = (p[1] as f64 / 1e7).to_radians().cos().max(0.01);
        let (ry, rx) = ((d / 0.011_131_95) as i64 + 1, (d / 0.011_131_95 / cos) as i64 + 1);
        let (x0, x1) = ((p[0] - rx).div_euclid(SEG_CELL), (p[0] + rx).div_euclid(SEG_CELL));
        let (y0, y1) = ((p[1] - ry).div_euclid(SEG_CELL), (p[1] + ry).div_euclid(SEG_CELL));
        (x0..=x1)
            .flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
            .filter_map(move |c| self.grid.get(&c))
            .flatten()
            .map(move |&(k, a, b)| (k, seg_dist_m(p, a, b, cos)))
            .filter(move |(_, m)| *m <= d)
    }

    fn within(&self, p: geom::Pt, d: f64) -> bool {
        self.near(p, d).next().is_some()
    }

    /// The owner of the nearest segment the point lies on (within a metre).
    fn on(&self, p: geom::Pt) -> Option<usize> {
        self.near(p, 1.0).min_by(|x, y| x.1.total_cmp(&y.1)).map(|x| x.0)
    }
}

fn seg_dist_m(p: geom::Pt, a: geom::Pt, b: geom::Pt, cos: f64) -> f64 {
    let f = |q: geom::Pt| [(q[0] - p[0]) as f64 * cos, (q[1] - p[1]) as f64];
    let (a, b) = (f(a), f(b));
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 == 0.0 { 0.0 } else { (-(a[0] * d[0] + a[1] * d[1]) / l2).clamp(0.0, 1.0) };
    let q = [a[0] + t * d[0], a[1] + t * d[1]];
    (q[0] * q[0] + q[1] * q[1]).sqrt() * 0.011_131_95
}
