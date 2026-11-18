//! Land and water, cell by cell (docs/inputs.md §6.1.3 "Land and water", §6.1.4 "Memory"): the
//! regions with a `water` key resolved on a grid of 2° cells, so that no operation holds more than
//! a cell's geometry (and, for a buffer or a nearest-coast split, its neighbourhood's).
//!
//! - Every part a region names is cut into the cells by clipping its rings to boxes
//!   (Sutherland–Hodgman, halving the box until each piece is one cell's: plain points, no
//!   polygon operation on the whole), then each cell's rings made an area.
//! - Each expression is evaluated cell by cell: a union, difference or intersection of the parts'
//!   pieces in the cell; a buffer of the pieces in the cells within its distance; a nearest-coast
//!   split of the cell's piece against the coasts in the cells within 450 km, the reach doubled until
//!   every point's nearest coast lies within it (the grid it decides on is
//!   anchored at 0°, 0°, so neighbouring cells' splits meet); a cut by its line.
//! - A region's land is its outline cut back to the coastline's land (the cell's), less the lakes
//!   over 100 km²; its water the `water` parts less the coastline's land and every region without
//!   water, after those its water names whole; its lakes, those in its outline that no other region's
//!   water names; the three together its area, kept by cell and stored as the cells' rings.
//! - One coastline: the outlines' coastal edges taken from the coastline (the pass's land).
//!   What lies in no region along it, land past an outline's coast or sea short of the water's
//!   parts, joined across the cells' edges, goes, where all of a stretch is within
//!   `COAST_REACH_M` of regions, to the nearest of them (split at the median line), if it has water.
//! - `hand_out`: a stretch (cells' pieces joined across the cells' edges they share) of a region's
//!   water that one member of the folder alone borders goes to it; one several border stays. A
//!   member is a region, or a subfolder holding a waters region (its regions together, the stretch
//!   going to its waters region). Only regions with water take one.
//! - With `cover = "earth"` in `_rules.toml`, the cover check, which moves nothing (`Fit`): each
//!   pair's area both have past `HAIR_KM2` is `rg-cover` (`overlap`); what no region covers, joined
//!   across the cells' edges, past it is too (`gap`), but the open sea, which the `rest` region
//!   takes (its part inside a 200-nmi zone an error, `rest-in-zone`); and a region's water (but
//!   its lakes) touching none of its land (`stray`). Each at a point inside, naming the regions round it.
//!   Regions without water are never changed.

use super::extract::Id;
use super::file::{Part, Region, Target};
use super::geom::{self, Area, Pt, Ring};
use super::progress::{Progress, Step};
use super::resolve::{Folder, Leaf, Outcome, Problem, Resolved};
use super::stored::Shape;
use crate::inputs::pace;
use det::Det;
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
}

pub fn parse_rules(text: &str) -> Result<Rules, super::file::Fault> {
    let r: Rules = toml::from_str(text)
        .map_err(|e| super::file::Fault { check: "rules", message: format!("_rules.toml: {}", e.message()) })?;
    if r.cover.as_deref().is_some_and(|c| c != "earth") {
        return Err(super::file::Fault { check: "rules", message: "_rules.toml: cover is \"earth\" or absent".into() });
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
    Problem {
        warning: false,
        check,
        about: vec![r.id.clone()],
        found: Vec::new(),
        files: vec![path.to_string()],
        message,
    }
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
    let lat_m = CELL as f64 * 1e-7 * roadcore::geo::M_PER_DEG;
    let dy = (m / lat_m).ceil() as i64;
    // (The band's latitude farthest from the equator, but no farther than 86°N or 80°S: the
    // buffers read no cells past those (no land north of 83.7°N; Antarctica's inland ice), and
    // the columns are capped, `MAX_REACH_COLS` south, `MAX_REACH_COLS_NORTH` north, enough for
    // 200 nmi at 86°N.)
    let north = c.1 + dy + 1 > (c.1 - dy).abs();
    let edge = if north { c.1 + dy + 1 } else { (c.1 - dy).abs() };
    let worst = ((edge * CELL) as f64 * 1e-7).min(if north { 86.0 } else { 80.0 });
    let cos = worst.to_radians().dcos();
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

/// The nearest-coast split's first reach: doubled while a point's nearest coast lies past it.
const NEAREST_REACH_M: f64 = geom::NEAREST_REACH_M;

/// The columns and rows of cells holding everything within `m` metres of cell `c` (no polar cap:
/// near a pole, every column).
fn nearest_reach(c: Cell, m: f64) -> (i64, i64) {
    let lat_m = CELL as f64 * 1e-7 * roadcore::geo::M_PER_DEG;
    let dy = ((m / lat_m).ceil() as i64).min(90);
    let edge = ((c.1 + 1 + dy).abs().max((c.1 - dy).abs()) * CELL) as f64 * 1e-7;
    let cos = edge.min(90.0).to_radians().dcos();
    let dx = if cos < 1e-6 {
        COLS / 2
    } else {
        ((m / (CELL as f64 * 1e-7 * roadcore::geo::M_PER_DEG * cos)).ceil() as i64).min(COLS / 2)
    };
    (dx, dy)
}

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
            // (The coasts in the cells within the reach, the reach doubled until each point's
            // nearest coast lies within it: a coast farther off is never passed over.)
            let mut m = NEAREST_REACH_M;
            loop {
                let (dx, dy) = nearest_reach(c, m);
                let gather = |ps: &[Part]| -> Result<Vec<Area>, Problem> {
                    ps.iter().map(|q| around(q, c, dx, dy, env, r, path)).collect::<Result<Vec<_>, _>>()
                };
                let (ta, tb) = (gather(to)?, gather(than)?);
                if dx >= COLS / 2 && dy >= 89 {
                    // (Every cell gathered: the whole Earth's coasts.)
                    break geom::nearest(&a, &ta, &tb, *step_m);
                }
                // (The cells gathered hold every coast within `m` of the cell.)
                if let Some(x) = geom::nearest_within(&a, &ta, &tb, *step_m, m) {
                    break x;
                }
                m *= 2.0;
            }
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

/// An area by cell, as a piece keeps it (super::kept).
fn put_tarea(p: &mut jobrt::resume_log::Put, t: &TArea) {
    p.u64(t.cells.len() as u64);
    for (c, a) in &t.cells {
        p.u64(c.0 as u64);
        p.u64(c.1 as u64);
        super::kept::area(p, a);
    }
}

fn get_tarea(g: &mut jobrt::resume_log::Get) -> anyhow::Result<TArea> {
    let mut cells = BTreeMap::new();
    for _ in 0..g.count(24)? {
        let c = (g.u64()? as i64, g.u64()? as i64);
        cells.insert(c, super::kept::get_area(g)?);
    }
    Ok(TArea { cells })
}

fn put_made(p: &mut jobrt::resume_log::Put, m: &Made) {
    put_tarea(p, &m.land);
    put_tarea(p, &m.lakes);
    put_tarea(p, &m.water);
    p.bytes(m.country.as_bytes());
}

fn get_made(g: &mut jobrt::resume_log::Get) -> anyhow::Result<Made> {
    Ok(Made {
        land: get_tarea(g)?,
        lakes: get_tarea(g)?,
        water: get_tarea(g)?,
        country: String::from_utf8(g.bytes()?)?,
    })
}

// ---- the whole --------------------------------------------------------------------------------------

/// Every region with water resolved (those without, `resolve`'s, as they were, those of them `held`
/// holding the regions with water naming them): into `out`.
pub fn resolve(folder: &Folder, ctx: &super::resolve::Context, held: &BTreeSet<String>, out: &mut Outcome) {
    // (On threads whose operations snap: `geom::snap_here`.)
    let threads = crate::inputs::pace::width();
    match rayon::ThreadPoolBuilder::new().num_threads(threads).start_handler(|_| geom::snap_here()).build() {
        Ok(pool) => pool.install(|| geom::snapping(|| resolve_in(folder, ctx, held, out))),
        Err(_) => geom::snapping(|| resolve_in(folder, ctx, held, out)),
    }
}

fn resolve_in(folder: &Folder, ctx: &super::resolve::Context, plain_held: &BTreeSet<String>, out: &mut Outcome) {
    let rules = folder.rules.clone().unwrap_or_default();
    let ids: Vec<String> =
        folder.regions.iter().filter(|(_, (_, r))| r.has_water()).map(|(id, _)| id.clone()).collect();
    if ids.is_empty() {
        return;
    }
    let Some(data) = ctx.data.filter(|d| d.has_land()) else {
        for id in &ids {
            let (p, r) = &folder.regions[id];
            out.problems.push(problem("rg-data", r, p, format!("{id}: a region with water is cut back to the coastline's land, which isn't read now (the pass's regions-land: docs/inputs.md §6.1.7)")));
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
    // (Held: the regions without water `resolve` held, and those with it as they're held here.)
    let mut held: BTreeSet<String> = plain_held.clone();
    let whole = super::resolve::whole_key(folder, ctx);
    // (What the extract lacks, as the regions without water name it: the held report's needs, so
    // the lead makes the extract; `out` is read by the waves, so added to it after.)
    let mut needs: BTreeSet<Id> = BTreeSet::new();
    trace("land");
    // 1. Land, in waves by what outlines name.
    let progress = &ctx.progress;
    progress.begin(Step::Land, ids.len());
    let mut left: Vec<String> = ids.clone();
    while !left.is_empty() {
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
        // (Each region's land kept as a piece, task #897, by the folder's key; given up once a
        // pause came.)
        let res: Vec<(String, Result<Made, Vec<Problem>>)> = wave
            .iter()
            .map(|id| {
                let _one = progress.one_on_drop();
                let make = || {
                    let mut more = BTreeSet::new();
                    let m = make_land(id, folder, ctx, data, &made, &others, out, &held, &mut more);
                    (m, more.into_iter().collect::<Vec<Id>>())
                };
                let (m, more) = match ctx.kept {
                    None => make(),
                    Some(k) => k
                        .get_or_make(
                            super::kept::LAND,
                            &format!("{whole} {id}"),
                            make,
                            |(m, more), p| {
                                super::kept::result(p, m, put_made);
                                super::kept::ids(p, more);
                            },
                            |g| Ok((super::kept::get_result(g, get_made)?, super::kept::get_ids(g)?)),
                        )
                        .unwrap_or_else(|| (Err(Vec::new()), Vec::new())),
                };
                needs.extend(more);
                (id.clone(), m)
            })
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
    progress.begin(Step::Water, left.len());
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
                let _one = progress.one_on_drop();
                trace(&format!("water of {id} begins"));
                let make = || {
                    let mut more = BTreeSet::new();
                    let w = make_water(id, folder, ctx, data, &made, &others, out, &mut more);
                    (w, more.into_iter().collect::<Vec<Id>>())
                };
                let (w, more) = match ctx.kept {
                    None => make(),
                    Some(k) => k
                        .get_or_make(
                            super::kept::WATER,
                            &format!("{whole} {id}"),
                            make,
                            |(w, more), p| {
                                super::kept::result(p, w, put_tarea);
                                super::kept::ids(p, more);
                            },
                            |g| Ok((super::kept::get_result(g, get_tarea)?, super::kept::get_ids(g)?)),
                        )
                        .unwrap_or_else(|| (Err(Vec::new()), Vec::new())),
                };
                needs.extend(more);
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
    progress.begin(Step::Lakes, lake_cells.len());
    let lake_cuts: Vec<(String, Cell, Area)> = pace::par_map(lake_cells.into_iter().collect(), |c| {
        let _one = progress.one_on_drop();
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
    trace("coast");
    trace_gaps("before the coast's fill", data, |c| {
        made.values()
            .flat_map(|m| [m.land.get(c), m.water.get(c), m.lakes.get(c)])
            .chain(others.values().map(|t| t.get(c)))
            .filter(|a| !a.is_empty())
            .collect()
    });
    // 3b. One coastline: what lies in no region between a region's coast and the coastline's.
    coast(data, folder, &mut made, &others, progress);
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
    progress.begin(Step::Areas, pairs.len());
    for (id, c, a) in pace::par_map(pairs, |(id, m, c)| {
        let _one = progress.one_on_drop();
        (id, c, m.land.get(c).union(&m.water.get(c)).union(&m.lakes.get(c)))
    }) {
        areas.get_mut(id).unwrap().set(c, a);
    }
    // (The lands and lakes kept only for the cover check's stray water.)
    let cover = rules.cover.as_deref() == Some("earth");
    let lands: BTreeMap<String, (TArea, TArea)> = if cover {
        made.iter().map(|(id, m)| (id.clone(), (m.land.clone(), m.lakes.clone()))).collect()
    } else {
        BTreeMap::new()
    };
    let countries: BTreeMap<String, String> = made.iter().map(|(id, m)| (id.clone(), m.country.clone())).collect();
    drop(made);
    trace("hand-outs");
    // 4. Hand-outs: to outer folders first (a folder's waters region with a hand_out of its own
    // hands on what it was handed); the order is otherwise the ids'. A run of them in that order
    // none of which touches a region an earlier one of the run does (its own, its members) is made
    // together, their stretches one pool of work: none reads what another writes, so making them
    // together gives what one after another would.
    let mut handing = ids.clone();
    handing.sort_by_key(|id| folder.regions[id].1.hand_out.as_ref().map_or(0, |g| g.split('/').count()));
    progress.begin(Step::HandOuts, handing.len());
    let mut next = 0;
    while next < handing.len() {
        // (The run: each handing region and its units, in order.)
        let mut batch: Vec<(&String, BTreeMap<String, Vec<String>>)> = Vec::new();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        while let Some(id) = handing.get(next) {
            let Some(g) = folder.regions[id].1.hand_out.clone().filter(|_| areas.contains_key(id)) else {
                next += 1;
                drop(progress.one_on_drop());
                continue;
            };
            // (Only regions with water take a hand-out: those without resolve as they always have.
            // The regions with water are all in `areas`, but those held.)
            let (members, old): (Vec<String>, Vec<String>) = folder
                .members(&g, id)
                .into_iter()
                .filter(|m| areas.contains_key(m) || others.contains_key(m))
                .partition(|m| folder.regions[m].1.has_water());
            if !batch.is_empty() && (touched.contains(id) || members.iter().any(|m| touched.contains(m))) {
                break;
            }
            next += 1;
            if let Some(o) = old.first() {
                let (p, r) = &folder.regions[id];
                out.problems.push(problem("rg-ref", r, p, format!("{id}: the hand-out to \"{g}\" has region {o} without water (and {} more): a hand-out gives only to regions with water", old.len() - 1)));
            }
            let units = standing(folder, &g, id, &members, &mut out.problems);
            touched.insert(id.clone());
            touched.extend(members);
            batch.push((id, units));
        }
        if batch.is_empty() {
            continue;
        }
        // (Each stretch by its polygons, its pieces made where it's measured and where it's moved:
        // the run holds no copy of its regions' water but the stretch under way on each thread,
        // and one region's moves as they're applied.)
        let comps: Vec<Vec<Stretch>> = batch.iter().map(|(id, _)| component_polys(&areas[*id])).collect();
        let moves: Vec<(usize, usize, String)> = {
            // (Counted by cell, the units' unions' and the stretches': a large stretch, or a
            // hand-out to a folder of many regions, says how far it is as it goes.)
            let joined: Vec<Vec<(&String, Vec<&TArea>)>> = batch
                .iter()
                .map(|(_, units)| {
                    units
                        .iter()
                        .filter(|(_, ms)| ms.len() > 1)
                        .map(|(to, ms)| (to, ms.iter().map(|m| &areas[m]).collect()))
                        .collect()
                })
                .collect();
            let union_cells: usize = joined
                .iter()
                .flatten()
                .map(|(_, ts)| ts.iter().flat_map(|t| t.cells.keys()).collect::<BTreeSet<_>>().len())
                .sum();
            progress.item(union_cells + comps.iter().flatten().map(Vec::len).sum::<usize>());
            let unions: Vec<BTreeMap<&String, TArea>> = joined
                .into_iter()
                .map(|j| j.into_iter().map(|(to, ts)| (to, union_of(&ts, progress))).collect())
                .collect();
            let theirs: Vec<Vec<(&String, &TArea)>> = batch
                .iter()
                .zip(&unions)
                .map(|((_, units), u)| units.keys().map(|m| (m, u.get(m).unwrap_or(&areas[m]))).collect())
                .collect();
            // (Each member's boxes by cell, made once: every stretch reads them.)
            let boxes: Vec<Vec<HashMap<Cell, [i64; 4]>>> =
                theirs.iter().map(|ts| pace::par_map(ts.iter().map(|(_, t)| *t).collect(), boxes_of)).collect();
            // Each stretch's edge with each member (the stretch's ring of 100 m made once a cell),
            // and where it goes.
            let work: Vec<(usize, usize)> =
                comps.iter().enumerate().flat_map(|(b, cs)| (0..cs.len()).map(move |k| (b, k))).collect();
            pace::par_map(work, |(b, k)| {
                let comp = piece_of(&areas[batch[b].0], &comps[b][k]);
                let mut edge: Vec<f64> = vec![0.0; theirs[b].len()];
                for (c, p) in &comp {
                    let _part = progress.part_on_drop();
                    let mut ring: Option<Area> = None;
                    for (j, (_, t)) in theirs[b].iter().enumerate() {
                        edge[j] += edge_in(p, *c, t, &boxes[b][j], &mut ring);
                    }
                }
                // (One member alone borders it: its. Several: it stays the handing region's.)
                let mut touching = edge.iter().zip(&theirs[b]).filter(|(l, _)| **l > 0.0).map(|(_, (m, _))| *m);
                match (touching.next(), touching.next()) {
                    (Some(m), None) => Some((b, k, m.clone())),
                    _ => None,
                }
            })
            .into_iter()
            .flatten()
            .collect()
        };
        // Applied region by region, cell by cell: taken from the handing region, given to each
        // member (no two of the run's regions share one: each one's as it would be alone).
        let handed: Vec<String> = batch.iter().map(|(id, _)| (*id).clone()).collect();
        drop(batch);
        for (b, id) in handed.iter().enumerate() {
            // (Each member's pieces by cell, in the moves' order; and where each cell's are.)
            let mut given: BTreeMap<&String, BTreeMap<Cell, Vec<Area>>> = BTreeMap::new();
            let mut taken: BTreeMap<Cell, Vec<(&String, usize)>> = BTreeMap::new();
            for (_, k, m) in moves.iter().filter(|x| x.0 == b) {
                for (c, p) in piece_of(&areas[id], &comps[b][*k]) {
                    let v = given.entry(m).or_default().entry(c).or_default();
                    taken.entry(c).or_default().push((m, v.len()));
                    v.push(p);
                }
            }
            let a = areas.get_mut(id).unwrap();
            for (c, at) in taken {
                let ps: Vec<&Area> = at.iter().map(|(m, i)| &given[m][&c][*i]).collect();
                let l = a.get(c).minus(&geom::union_all_of(&ps));
                a.set(c, l);
            }
            for (m, cells) in given {
                let t = areas.get_mut(m).unwrap();
                for (c, mut ps) in cells {
                    ps.push(t.get(c));
                    t.set(c, geom::union_all(&ps));
                }
            }
            drop(progress.one_on_drop());
        }
    }
    trace("the cover check");
    // 5. With cover = "earth": the check, which moves nothing; the rest.
    // (Every region's cells, moved together, not copied.)
    let mut all: BTreeMap<String, TArea> = others;
    all.extend(areas);
    if cover {
        strays(&ids, folder, &lands, &all, &mut out.problems, &mut out.fit, progress);
    }
    drop(lands);
    let rest_id = rests.first().map(|s| (*s).clone());
    let zones = if cover { zones(data, progress) } else { TArea::default() };
    let rest =
        cover_check(folder, data, &zones, &all, cover, rest_id.is_some(), &mut out.problems, &mut out.fit, progress);
    drop(zones);
    trace_gaps("at the end", data, |c| all.values().map(|t| t.get(c)).filter(|a| !a.is_empty()).collect());
    trace("stored");
    // 6. Stored: every region with water, and the rest (those without, as they resolved).
    progress.begin(Step::Stored, all.len());
    for (id, t) in &all {
        let _one = progress.one_on_drop();
        if rest_id.as_ref() == Some(id) {
            continue;
        }
        let (p, r) = &folder.regions[id];
        if !r.has_water() {
            continue;
        }
        if t.is_empty() {
            // (A waters region in a folder some other region hands out to, handed nothing: listed,
            // not built.)
            let handed = |(o, (_, x)): (&String, &(String, Region))| {
                o != id && x.hand_out.as_ref().is_some_and(|g| p.starts_with(&format!("{g}/")))
            };
            if !(r.is_waters() && folder.regions.iter().any(handed)) {
                out.problems.push(problem("rg-empty", r, p, format!("{id}: the region has neither land nor water")));
            }
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
        static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        eprintln!(
            "regions: {step} at {:.0} s, footprint {} MB; buffers {}; wide buffers {}; nearest-coast splits {}",
            T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64(),
            jobrt::sys::footprint().unwrap_or(0) / 1_000_000,
            f(&BUFFERS),
            f(&WIDE_BUFFERS),
            f(&NEARESTS)
        );
    }
}

/// With `SCENIC_REGIONS_GAPS` set to cells' south-west corners in degrees (`-128,52;-64,44`):
/// each one's land in no region (the coastline's land less every region's area there), as pieces
/// and sizes, on stderr, labelled `when`.
fn trace_gaps<'a>(when: &str, data: &super::data::Datasets, areas: impl Fn(Cell) -> Vec<Area> + Sync) {
    let Ok(v) = std::env::var("SCENIC_REGIONS_GAPS") else { return };
    let cells: Vec<Cell> = v
        .split(';')
        .filter_map(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.trim().parse::<i64>().ok()?.div_euclid(2), y.trim().parse::<i64>().ok()?.div_euclid(2)))
        })
        .collect();
    for (c, line) in pace::par_map(cells, |c| {
        let free = land_and_lakes(data, c).0.minus(&geom::union_all(&areas(c)));
        let mut k: Vec<f64> = free.polys.iter().map(|p| geom::area_km2(&Area { polys: vec![p.clone()] })).collect();
        k.sort_by(f64::total_cmp);
        let line = if k.is_empty() {
            "none".to_string()
        } else {
            format!(
                "{} pieces ({} over 100 m²), median {:.4} km², largest {:.3} km², {:.2} km² in all",
                k.len(),
                k.iter().filter(|x| **x > 1e-4).count(),
                k[k.len() / 2],
                k[k.len() - 1],
                k.iter().sum::<f64>()
            )
        };
        (c, line)
    }) {
        eprintln!("regions: gaps {when}, the cell from {}°, {}°: {line}", c.0 * 2, c.1 * 2);
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
            self.0 .2.fetch_max(jobrt::sys::footprint().unwrap_or(0) / 1_000_000, Relaxed);
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
    ctx.progress.item(cells.len());
    let got: Vec<Result<(Cell, Area, Area), Problem>> = pace::par_map(cells.into_iter().collect(), |c| {
        let _part = ctx.progress.part_on_drop();
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

/// How far a region's coast is taken out to the coastline's (metres): land in no region within this
/// of a region's land, sea within this of its water.
pub const COAST_REACH_M: f64 = 1_000.0;

/// A coast piece several regions are nearest, under this (km²), goes whole to the one nearest the
/// point inside it; a larger one is split at the median line.
pub const SPECK_KM2: f64 = 0.01;

/// One coastline for land and water: the outlines' coastal edges taken from the coastline (OSM's
/// land, the pass's). What no region takes along the coastline, on either side of it — its land where
/// an outline's coast lay inland of it, its sea where the water's parts stopped short of it — is
/// joined across the cells' edges into stretches; a stretch every point of which (its vertices and
/// a point inside each piece) lies within `COAST_REACH_M` of a region's land (a piece of land) or
/// water and lakes (a piece of sea) goes to the nearest such region, split at the median line
/// where several are nearest; a region without water keeps its outline (its share stays out).
/// So every region's coast is the coastline's.
fn coast(
    data: &super::data::Datasets,
    folder: &Folder,
    made: &mut BTreeMap<String, Made>,
    others: &BTreeMap<String, TArea>,
    progress: &Progress,
) {
    let cells: BTreeSet<Cell> = made
        .values()
        .flat_map(|m| m.land.cells.keys().chain(m.water.cells.keys()).chain(m.lakes.cells.keys()).copied())
        .collect();
    let takes = |id: &str| made.contains_key(id) && folder.regions.get(id).is_some_and(|(_, r)| r.has_water());
    // A region's side (land, or water and lakes) in a cell.
    let ids: Vec<&String> = made.keys().chain(others.keys()).collect();
    // (Which regions have any of their area in each cell.)
    let mut at: HashMap<Cell, Vec<usize>> = HashMap::new();
    for (k, id) in ids.iter().enumerate() {
        let cs: BTreeSet<Cell> = match made.get(*id) {
            Some(m) => m.land.cells.keys().chain(m.water.cells.keys()).chain(m.lakes.cells.keys()).copied().collect(),
            None => others[*id].cells.keys().copied().collect(),
        };
        for c in cs {
            at.entry(c).or_default().push(k);
        }
    }
    // A region's pieces on one side in a cell, by reference.
    let side_ref = |k: usize, sea: bool, c: Cell| -> Vec<&Area> {
        match made.get(ids[k]) {
            Some(m) if sea => [m.water.cells.get(&c), m.lakes.cells.get(&c)].into_iter().flatten().collect(),
            Some(m) => m.land.cells.get(&c).into_iter().collect(),
            None => others[ids[k]].cells.get(&c).into_iter().collect(),
        }
    };
    // (Each side's regions' edges in cell `c` and round it within 4 km of it, shifted as they lie
    // from it.)
    let index = |c: Cell, sea: bool| {
        let none = Vec::new();
        let bx = cell_box(c);
        let lat = ((c.1 * CELL) as f64 * 1e-7).abs().max(((c.1 + 1) * CELL) as f64 * 1e-7).min(89.0);
        let pad = (4.0 * COAST_REACH_M / roadcore::geo::M_PER_DEG * 1e7 / lat.to_radians().dcos()) as i64;
        let near_box = [bx[0] - pad, bx[1] - pad, bx[2] + pad, bx[3] + pad];
        let mut idx = SegIndex::default();
        for x in c.0 - 1..=c.0 + 1 {
            let (w, shift) = wrap(x);
            for y in (c.1 - 1).max(-45)..=(c.1 + 1).min(44) {
                for &k in at.get(&(w, y)).unwrap_or(&none) {
                    for a in side_ref(k, sea, (w, y)) {
                        for r in a.polys.iter().flatten() {
                            idx.add(k, r, shift, near_box);
                        }
                    }
                }
            }
        }
        idx
    };
    struct Piece {
        c: Cell,
        sea: bool,
        sides: Sides,
        area: Area,
        km2: f64,
        /// The farthest of its points from the nearest region (m), the regions nearest them, and
        /// the one nearest the point inside it.
        far: f64,
        near: BTreeSet<String>,
        inner: Option<String>,
    }
    progress.begin(Step::Coast, cells.len());
    let pieces: Vec<Piece> = pace::par_map(cells.into_iter().collect(), |c| {
        let _one = progress.one_on_drop();
        geom::set_label(format!("the coast, the cell from {}°, {}°", c.0 * 2, c.1 * 2));
        let land = land_and_lakes(data, c).0;
        if land.is_empty() {
            return Vec::new();
        }
        let none = Vec::new();
        let all: Vec<Area> = at
            .get(&c)
            .unwrap_or(&none)
            .iter()
            .flat_map(|&k| side_ref(k, false, c).into_iter().chain(side_ref(k, true, c)))
            .cloned()
            .collect();
        let free = geom::box_area(cell_box(c)).minus(&geom::union_all(&all));
        if free.is_empty() {
            return Vec::new();
        }
        let coastline = SegIndex::new(land.polys.iter().flatten().map(|r| (0, r)));
        let bx = cell_box(c);
        let mut out = Vec::new();
        for (sea, side) in [(false, free.intersect(&land)), (true, free.minus(&land))] {
            let mut idx: Option<SegIndex> = None;
            for poly in side.polys {
                // (The land's rings run along the cell's edges too: no coast there.)
                let on_coast = poly.iter().any(|r| {
                    (0..r.len()).any(|k| {
                        let (a, b) = (r[k], r[(k + 1) % r.len()]);
                        let edge = (a[0] == b[0] && (a[0] == bx[0] || a[0] == bx[2]))
                            || (a[1] == b[1] && (a[1] == bx[1] || a[1] == bx[3]));
                        !edge && coastline.on([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2]).is_some()
                    })
                });
                if !on_coast {
                    continue;
                }
                let idx = idx.get_or_insert_with(|| index(c, sea));
                let (mut far, mut near, mut inner) = (0.0f64, BTreeSet::new(), None);
                // (A point inside first, then the vertices.)
                for p in [interior(&poly)].into_iter().chain(poly.iter().flatten().copied()) {
                    // (The nearest region within reach, ties to the lesser id: looked for within 10 m,
                    // then 100 m, then the reach; whatever lies nearer than the first found is
                    // within the radius that found it.)
                    let best = [10.0, 100.0, COAST_REACH_M]
                        .into_iter()
                        .find_map(|r| idx.near(p, r).min_by(|x, y| x.1.total_cmp(&y.1).then(ids[x.0].cmp(ids[y.0]))));
                    match best {
                        Some((k, d)) => {
                            far = far.max(d);
                            near.insert(ids[k].clone());
                            inner.get_or_insert(k);
                        }
                        None => far = f64::INFINITY,
                    }
                    if far.is_infinite() {
                        break;
                    }
                }
                let sides = sides(&poly, c);
                let area = Area { polys: vec![poly] };
                let km2 = geom::area_km2(&area);
                out.push(Piece { c, sea, sides, area, km2, far, near, inner: inner.map(|k| ids[k].clone()) });
            }
        }
        out
    })
    .into_iter()
    .flatten()
    .collect();
    trace(&format!("the coast: {} pieces found", pieces.len()));
    // Each stretch within reach, to its nearest regions.
    let mut whole: Vec<(String, usize)> = Vec::new();
    let mut split: Vec<(Vec<String>, usize)> = Vec::new();
    for sea in [false, true] {
        let mine: Vec<usize> = (0..pieces.len()).filter(|&i| pieces[i].sea == sea).collect();
        let roots = joined(&mine.iter().map(|&i| (pieces[i].c, &pieces[i].sides)).collect::<Vec<_>>());
        let mut stretches: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (j, r) in roots.into_iter().enumerate() {
            stretches.entry(r).or_default().push(mine[j]);
        }
        for idx in stretches.into_values() {
            if idx.iter().any(|&i| pieces[i].far > COAST_REACH_M) {
                let k: f64 = idx.iter().map(|&i| geom::area_km2(&pieces[i].area)).sum();
                if std::env::var_os("SCENIC_REGIONS_TRACE").is_some() && k > 1.0 {
                    let b = pieces[idx[0]].area.bbox().unwrap_or_default();
                    eprintln!(
                        "regions: the coast: a stretch of {} out of reach, {k:.2} km², by {:.4}°, {:.4}°",
                        if sea { "sea" } else { "land" },
                        b[0] as f64 / 1e7,
                        b[1] as f64 / 1e7
                    );
                }
                continue;
            }
            let near: BTreeSet<String> = idx.iter().flat_map(|&i| pieces[i].near.iter().cloned()).collect();
            if near.len() == 1 {
                let id = near.into_iter().next().unwrap();
                if takes(&id) {
                    whole.extend(idx.into_iter().map(|i| (id.clone(), i)));
                }
            } else {
                // (A speck under `SPECK_KM2` whole to the region nearest the point inside it; a
                // larger piece split.)
                let v: Vec<String> = near.into_iter().collect();
                for i in idx {
                    match &pieces[i].inner {
                        Some(id) if pieces[i].km2 < SPECK_KM2 => {
                            if takes(id) {
                                whole.push((id.clone(), i));
                            }
                        }
                        _ => split.push((v.clone(), i)),
                    }
                }
            }
        }
    }
    // (A piece several regions are nearest: cut at the median line, each region's part the points
    // nearer it, decided by those regions' edges round the piece (each edge a coast's samples, as
    // geom::nearest reads them; no overlay), read from the cell's index, made once a cell and side.)
    trace(&format!("the coast: {} pieces to split", split.len()));
    let kid: HashMap<&String, usize> = ids.iter().enumerate().map(|(k, id)| (*id, k)).collect();
    let mut by_cell: BTreeMap<(Cell, bool), Vec<(Vec<String>, usize)>> = BTreeMap::new();
    for (near, i) in split {
        by_cell.entry((pieces[i].c, pieces[i].sea)).or_default().push((near, i));
    }
    progress.begin(Step::CoastSplits, by_cell.len());
    let cut: Vec<(String, usize, Area)> = pace::par_map(by_cell.into_iter().collect(), |((c, sea), v)| {
        let _one = progress.one_on_drop();
        let idx = index(c, sea);
        let mut out = Vec::new();
        for (near, i) in v {
            let p = &pieces[i];
            let Some(b) = p.area.bbox() else { continue };
            let lat = (b[1].abs().max(b[3].abs()) as f64 * 1e-7).min(89.0);
            let pad = (1.5 * COAST_REACH_M / roadcore::geo::M_PER_DEG * 1e7 / lat.to_radians().dcos()) as i64;
            let window = [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad];
            let mut edges: Vec<Vec<Ring>> = vec![Vec::new(); near.len()];
            for (k, a, z) in idx.segments_in(window) {
                if let Some(j) = near.iter().position(|id| kid[id] == k) {
                    // (Clipped to the window: a cell's edge is 2° long.)
                    if let Some(e) = clip_segment(a, z, window) {
                        edges[j].push(e);
                    }
                }
            }
            let sides: Vec<Area> = edges.into_iter().map(|e| Area { polys: vec![e] }).collect();
            let diag = geom::length_m([b[0], b[1]], [b[2], b[3]]);
            // (A staircase of about 50 steps across the piece, 10 m to 100 m: the step is the
            // edges' samples' spacing too; few pieces are split, specks going whole.)
            let step = (diag / 50.0).clamp(10.0, 100.0);
            for (k, id) in near.iter().enumerate().filter(|(_, id)| takes(id)) {
                let than: Vec<Area> =
                    sides.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, a)| a.clone()).collect();
                out.push((id.clone(), i, geom::nearest(&p.area, &sides[k..=k], &than, step)));
            }
        }
        out
    })
    .into_iter()
    .flatten()
    .collect();
    trace("the coast: pieces split");
    let mut fill: BTreeMap<(String, bool, Cell), Vec<Area>> = BTreeMap::new();
    let mut said = [(0usize, 0.0f64), (0, 0.0)];
    for (id, i, a) in whole.into_iter().map(|(id, i)| (id, i, pieces[i].area.clone())).chain(cut) {
        let p = &pieces[i];
        let s = &mut said[usize::from(p.sea)];
        s.0 += 1;
        s.1 += geom::area_km2(&a);
        fill.entry((id, p.sea, p.c)).or_default().push(a);
    }
    trace(&format!(
        "the coast: {} pieces of land, {:.1} km², and {} of sea, {:.1} km², to regions",
        said[0].0, said[0].1, said[1].0, said[1].1
    ));
    progress.begin(Step::CoastFills, fill.len());
    let filled: Vec<(String, bool, Cell, Area)> =
        pace::par_map(fill.into_iter().collect(), |((id, sea, c), mut v): ((String, bool, Cell), Vec<Area>)| {
            let _one = progress.one_on_drop();
            let m = &made[&id];
            v.push(if sea { m.water.get(c) } else { m.land.get(c) });
            (id, sea, c, geom::union_all(&v))
        });
    for (id, sea, c, a) in filled {
        let m = made.get_mut(&id).unwrap();
        if sea {
            m.water.set(c, a);
        } else {
            m.land.set(c, a);
        }
    }
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
    needs: &mut BTreeSet<Id>,
) -> Result<TArea, Vec<Problem>> {
    let (path, r) = &folder.regions[id];
    if r.water.is_empty() {
        return Ok(TArea::default());
    }
    let env = env_for(id, true, folder, ctx, made, others, out, needs)?;
    trace(&format!("water of {id}: its parts read"));
    let cells: BTreeSet<Cell> = r.water.iter().flat_map(|p| support(p, &env)).collect();
    trace(&format!("water of {id}: {} cells", cells.len()));
    ctx.progress.item(cells.len());
    let got: Vec<Result<(Cell, Area), Problem>> = pace::par_map(cells.into_iter().collect(), |c| {
        let _part = ctx.progress.part_on_drop();
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
fn components(t: &TArea) -> Vec<BTreeMap<Cell, Area>> {
    component_polys(t).iter().map(|g| piece_of(t, g)).collect()
}

/// A stretch of a tiled area by its polygons: each cell's polygons' indices there.
type Stretch = Vec<(Cell, Vec<usize>)>;

/// The stretches of a tiled area (`components`), by its polygons, nothing copied.
fn component_polys(t: &TArea) -> Vec<Stretch> {
    let polys: Vec<(Cell, usize, &Vec<Ring>)> =
        t.cells.iter().flat_map(|(c, a)| a.polys.iter().enumerate().map(move |(i, p)| (*c, i, p))).collect();
    let sides: Vec<Sides> = polys.iter().map(|(c, _, p)| sides(p, *c)).collect();
    let roots = joined(&polys.iter().zip(&sides).map(|((c, _, _), s)| (*c, s)).collect::<Vec<_>>());
    let mut groups: BTreeMap<usize, BTreeMap<Cell, Vec<usize>>> = BTreeMap::new();
    for (k, (c, i, _)) in polys.into_iter().enumerate() {
        groups.entry(roots[k]).or_default().entry(c).or_default().push(i);
    }
    groups.into_values().map(|g| g.into_iter().collect()).collect()
}

/// A stretch of `t` (`component_polys`) as its cells' pieces.
fn piece_of(t: &TArea, s: &Stretch) -> BTreeMap<Cell, Area> {
    s.iter().map(|(c, is)| (*c, Area { polys: is.iter().map(|&i| t.cells[c].polys[i].clone()).collect() })).collect()
}

/// A polygon's stretches on its cell's west, south, east and north sides (the other coordinate's
/// intervals).
type Sides = [Vec<(i64, i64)>; 4];

fn sides(poly: &[Ring], c: Cell) -> Sides {
    let b = cell_box(c);
    let side = |k: usize, v: i64| -> Vec<(i64, i64)> {
        let o = 1 - k;
        poly.iter()
            .flat_map(|ring| {
                let n = ring.len();
                (0..n).filter_map(move |i| {
                    let (p, q) = (ring[i], ring[(i + 1) % n]);
                    (p[k] == v && q[k] == v && p[o] != q[o]).then(|| (p[o].min(q[o]), p[o].max(q[o])))
                })
            })
            .collect()
    };
    [side(0, b[0]), side(1, b[1]), side(0, b[2]), side(1, b[3])]
}

/// Each polygon's group (the least index in it): polygons joined where one's east or north side
/// shares a stretch with a polygon's west or south side in the neighbouring cell (across ±180°,
/// cell 89's east meets cell −90's west).
fn joined(polys: &[(Cell, &Sides)]) -> Vec<usize> {
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut parent: Vec<usize> = (0..polys.len()).collect();
    let mut by_cell: HashMap<Cell, Vec<usize>> = HashMap::new();
    for (i, (c, _)) in polys.iter().enumerate() {
        by_cell.entry(*c).or_default().push(i);
    }
    for (i, (c, s)) in polys.iter().enumerate() {
        for (k, nc) in [(2usize, (wrap(c.0 + 1).0, c.1)), (3usize, (c.0, c.1 + 1))] {
            if s[k].is_empty() {
                continue;
            }
            let Some(js) = by_cell.get(&nc) else { continue };
            for &j in js {
                if s[k].iter().any(|a| polys[j].1[k - 2].iter().any(|b| a.0 < b.1 && b.0 < a.1)) {
                    let (ra, rb) = (find(&mut parent, i), find(&mut parent, j));
                    if ra != rb {
                        parent[ra.max(rb)] = ra.min(rb);
                    }
                }
            }
        }
    }
    (0..polys.len()).map(|i| find(&mut parent, i)).collect()
}

/// Each cell's box of a tiled area.
pub(super) fn boxes_of(t: &TArea) -> HashMap<Cell, [i64; 4]> {
    t.cells.iter().filter_map(|(c, a)| a.bbox().map(|b| (*c, b))).collect()
}

/// The edge (km) piece `p` of cell `c` shares with `t`'s pieces in the cells round it (`boxes`, its
/// cells' boxes), its ring of 100 m made once (in `ring`) and only when one comes near.
pub(super) fn edge_in(p: &Area, c: Cell, t: &TArea, boxes: &HashMap<Cell, [i64; 4]>, ring: &mut Option<Area>) -> f64 {
    let Some(pb) = p.bbox() else { return 0.0 };
    let pad = 20_000;
    let mut sum = 0.0;
    for x in c.0 - 1..=c.0 + 1 {
        let (w, shift) = wrap(x);
        for y in c.1 - 1..=c.1 + 1 {
            let Some(q) = t.cells.get(&(w, y)) else { continue };
            let Some(qb) = boxes.get(&(w, y)).map(|b| [b[0] + shift, b[1], b[2] + shift, b[3]]) else { continue };
            if !(pb[0] <= qb[2] + pad && qb[0] <= pb[2] + pad && pb[1] <= qb[3] + pad && qb[1] <= pb[3] + pad) {
                continue;
            }
            let r = ring.get_or_insert_with(|| geom::buffer_area_coarse(p, 100.0).minus(p));
            let Some(rb) = r.bbox() else { continue };
            // (Only `q`'s part round the ring, where it can meet it: `near_part` keeps every
            // segment that comes near the ring as it is.)
            let local = near_part(q, [rb[0] - shift, rb[1], rb[2] - shift, rb[3]]);
            let q = match &local {
                Some(a) if a.is_empty() => continue,
                Some(a) => a,
                None => q,
            };
            let q = if shift == 0 { q.clone() } else { q.shifted(shift) };
            sum += geom::area_km2(&r.intersect(&q)) / 0.1;
        }
    }
    sum
}

/// How far round a box `near_part` keeps an area (E7: about 1 km, longer than most of a coast's
/// segments, so few cross the cut near the box).
const NEAR_PAD: i64 = 100_000;

/// Area `a` cut down to what lies round box `r` (within `NEAR_PAD`), for an operation with
/// something inside `r`: rings whose box misses `r` left out (they neither cross nor hold any of
/// it), rings within the pad kept whole, the rest clipped to the padded box. Every segment of `a`
/// meeting `r` is kept as it is, and what's inside `r` is the same area. So with the exact engine
/// (`Precision::ABSOLUTE`) an overlay with anything inside `r` gives the same result. On a thread
/// that snaps (`geom::snap_here`, as the hand-outs run) i_overlay's
/// snapping radius and strategy depend on the whole input, so it is the same there by measurement,
/// not by construction: task #1426's Canada subset resolved to the same bytes with and without it
/// (a subset of four countries' waters handing out together, the same before and after their
/// hand-outs held less), and the tests run it snapping. None when a segment the clip would cut
/// comes near `r` (a long one, crossing both): `a` is to be read whole then. Empty when nothing of
/// `a` comes near `r`.
pub(super) fn near_part(a: &Area, r: [i64; 4]) -> Option<Area> {
    let meets = |b: [i64; 4], x: [i64; 4]| b[0] <= x[2] && x[0] <= b[2] && b[1] <= x[3] && x[1] <= b[3];
    let within = |b: [i64; 4], x: [i64; 4]| b[0] >= x[0] && b[2] <= x[2] && b[1] >= x[1] && b[3] <= x[3];
    // (`r` itself two units wider: a clipped segment's end is rounded to the unit.)
    let near = [r[0] - 2, r[1] - 2, r[2] + 2, r[3] + 2];
    let pad = [r[0] - NEAR_PAD, r[1] - NEAR_PAD, r[2] + NEAR_PAD, r[3] + NEAR_PAD];
    let mut rings: Vec<Ring> = Vec::new();
    for ring in a.polys.iter().flatten() {
        let Some(b) = geom::bbox_of(ring.iter()) else { continue };
        if !meets(b, near) {
            continue;
        }
        if within(b, pad) {
            rings.push(ring.clone());
            continue;
        }
        let n = ring.len();
        for i in 0..n {
            let (p, q) = (ring[i], ring[(i + 1) % n]);
            let sb = [p[0].min(q[0]), p[1].min(q[1]), p[0].max(q[0]), p[1].max(q[1])];
            if !within(sb, pad) && meets(sb, near) {
                return None;
            }
        }
        rings.extend(clip(ring, pad));
    }
    Some(if rings.is_empty() { Area::default() } else { Area { polys: vec![rings] } })
}

/// What the cover check measured of how the regions fit (`scenic-build regions-check`'s report):
/// it moves nothing, so this is how the regions end.
#[derive(Debug, Default)]
pub struct Fit {
    /// Each pair's area both have, summed over the cells.
    pub overlaps: BTreeMap<(String, String), OverlapFit>,
    /// Each stretch in no region (joined across the cells' edges), the rest's included.
    pub gaps: Vec<GapFit>,
    /// Each region's stretches of water touching none of its land.
    pub strays: Vec<StrayFit>,
}

/// What the cover check lets pass as a hair (km²): an overlap of two regions summed over the
/// cells, a stretch in no region, the rest's part of a zone in a stretch, or a region's stretch of
/// water touching none of its land, up to it is no finding. The outlines' own snapping leaves
/// these between regions whose definitions meet exactly (`geom::snapping`: i_overlay snaps each
/// operation's points within a few units of E7): measured on the whole input (task #1427), the
/// largest such sliver left was under a tenth of this.
pub const HAIR_KM2: f64 = 0.1;

#[derive(Debug, Default)]
pub struct OverlapFit {
    pub land_km2: f64,
    pub water_km2: f64,
    /// A point inside its largest piece, and that piece's size (km²).
    pub at: Pt,
    pub largest_km2: f64,
}

#[derive(Debug)]
pub struct GapFit {
    pub km2: f64,
    /// Its part inside any country's 200-nmi zone.
    pub zone_km2: f64,
    /// Its land (read where it comes within 100 m of land).
    pub land_km2: f64,
    /// Within 100 m of land.
    pub touches: bool,
    /// A point inside it: in its largest piece touching land, else its largest.
    pub at: Pt,
    /// The rest's (the open sea), else in no region.
    pub rest: bool,
    /// The edge it shares with each region (km), longest first.
    pub edges: Vec<(String, f64)>,
}

#[derive(Debug)]
pub struct StrayFit {
    pub id: String,
    pub km2: f64,
    /// A point inside its largest piece.
    pub at: Pt,
    /// The edge it shares with each other region (km), longest first.
    pub edges: Vec<(String, f64)>,
}

/// A point as a finding says it: degrees, to 0.0001°.
fn place(p: Pt) -> String {
    format!("{:.4}°, {:.4}°", p[0] as f64 / 1e7, p[1] as f64 / 1e7)
}

/// A place as a finding's identity: degrees, to 0.1° (a new pass moving a coast a little gives
/// the same finding).
fn place_key(p: Pt) -> String {
    format!("{:.1},{:.1}", p[0] as f64 / 1e7, p[1] as f64 / 1e7)
}

/// Regions round a place, by the edge they share with it (km), as a finding says them.
fn neighbours(edges: &[(String, f64)]) -> String {
    if edges.is_empty() {
        return "no region borders it".into();
    }
    let v: Vec<String> = edges.iter().take(6).map(|(id, l)| format!("{id} ({l:.1} km)")).collect();
    let more = if edges.len() > 6 { format!(" and {} more", edges.len() - 6) } else { String::new() };
    format!("around it {}{more}", v.join(", "))
}

/// Each region's edges by `len` (metres, by owner), longest first, in km.
fn by_edge(len: BTreeMap<&String, f64>) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = len.into_iter().map(|(id, l)| (id.clone(), l / 1000.0)).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// The edge (m) polygon `poly`'s rings share with each owner of `idx`'s segments (a segment's
/// middle on one of theirs), added to `len`.
fn edges_with(poly: &[Ring], idx: &SegIndex, len: &mut [f64]) {
    for r in poly {
        for k in 0..r.len() {
            let (a, b) = (r[k], r[(k + 1) % r.len()]);
            if let Some(owner) = idx.on([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2]) {
                len[owner] += seg_m(a, b);
            }
        }
    }
}

/// Which regions have a piece in each cell.
fn by_cell(all: &BTreeMap<String, TArea>) -> HashMap<Cell, Vec<&String>> {
    let mut index: HashMap<Cell, Vec<&String>> = HashMap::new();
    for (id, t) in all {
        for c in t.cells.keys() {
            index.entry(*c).or_default().push(id);
        }
    }
    index
}

/// The segments of the regions' pieces (but `skip`'s) in cell `c` and of those round it meeting its
/// box, shifted as they lie from it (an edge on a cell's side is shared with the cell across it),
/// each owner numbered by its place in the list returned.
fn pieces_round<'a>(
    c: Cell,
    all: &'a BTreeMap<String, TArea>,
    index: &HashMap<Cell, Vec<&'a String>>,
    skip: Option<&str>,
) -> (Vec<&'a String>, SegIndex) {
    let bx = cell_box(c);
    let mut owners: Vec<&String> = Vec::new();
    let mut idx = SegIndex::default();
    for x in c.0 - 1..=c.0 + 1 {
        let (w, shift) = wrap(x);
        for y in (c.1 - 1).max(-45)..=(c.1 + 1).min(44) {
            for id in index.get(&(w, y)).into_iter().flatten().filter(|id| Some(id.as_str()) != skip) {
                let k = owners.iter().position(|o| o == id).unwrap_or_else(|| {
                    owners.push(id);
                    owners.len() - 1
                });
                for r in all[*id].cells[&(w, y)].polys.iter().flatten() {
                    idx.add(k, r, shift, bx);
                }
            }
        }
    }
    (owners, idx)
}

/// The largest polygon of an area: its size (km²) and a point inside it.
fn largest(a: &Area) -> Option<(f64, Pt)> {
    a.polys
        .iter()
        .map(|p| (geom::area_km2(&Area { polys: vec![p.clone()] }), p))
        .max_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(k, p)| (k, interior(p)))
}

/// With `cover`: each region with land, its stretches of water (its area less its land and its
/// lakes, joined across the cells' edges) that touch none of its land past `HAIR_KM2`, an error
/// (`stray`) at a point inside, naming the regions round it. Its lakes are its outline's, wherever
/// their shores are. A region of water alone (a waters region, the rest) has no land for its water
/// to touch: its stretches are its by its definition.
fn strays(
    ids: &[String],
    folder: &Folder,
    lands: &BTreeMap<String, (TArea, TArea)>,
    all: &BTreeMap<String, TArea>,
    problems: &mut Vec<Problem>,
    fit: &mut Fit,
    progress: &Progress,
) {
    progress.begin(Step::Strays, ids.len());
    let index = by_cell(all);
    for id in ids {
        let _one = progress.one_on_drop();
        let Some((land, lakes)) = lands.get(id).filter(|(l, _)| !l.is_empty()) else { continue };
        let Some(area) = all.get(id) else { continue };
        let less = |a: &Area, b: &Area| if b.is_empty() { a.clone() } else { a.minus(b) };
        let water = area.zip(land, less).zip(lakes, less);
        let comps = components(&water);
        progress.item(comps.len());
        // (The land's segments in a cell and round it, shifted as they lie from it: a stretch's
        // edge on a cell's side meets the land across it.)
        let land_round = |c: Cell| {
            let mut idx = SegIndex::default();
            for x in c.0 - 1..=c.0 + 1 {
                let (w, shift) = wrap(x);
                for y in (c.1 - 1).max(-45)..=(c.1 + 1).min(44) {
                    if let Some(a) = land.cells.get(&(w, y)) {
                        for r in a.polys.iter().flatten() {
                            idx.add(0, r, shift, [i64::MIN / 2, -1_000_000_000, i64::MAX / 2, 1_000_000_000]);
                        }
                    }
                }
            }
            idx
        };
        let found: Vec<StrayFit> = pace::par_map(comps, |comp| {
            let _part = progress.part_on_drop();
            let km2: f64 = comp.values().map(geom::area_km2).sum();
            if km2 <= HAIR_KM2 {
                return None;
            }
            let touches = comp.iter().any(|(c, p)| {
                let idx = land_round(*c);
                p.polys.iter().flatten().any(|r| {
                    (0..r.len()).any(|k| {
                        let (a, b) = (r[k], r[(k + 1) % r.len()]);
                        idx.on([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2]).is_some()
                    })
                })
            });
            if touches {
                return None;
            }
            // (The regions round it: their segments in its cells and across their sides.)
            let mut len: BTreeMap<&String, f64> = BTreeMap::new();
            for (c, p) in &comp {
                let (theirs, idx) = pieces_round(*c, all, &index, Some(id));
                let mut l = vec![0.0; theirs.len()];
                for poly in &p.polys {
                    edges_with(poly, &idx, &mut l);
                }
                for (k, o) in theirs.iter().enumerate() {
                    if l[k] > 0.0 {
                        *len.entry(o).or_default() += l[k];
                    }
                }
            }
            let at = comp.values().filter_map(largest).max_by(|a, b| a.0.total_cmp(&b.0)).map_or([0, 0], |x| x.1);
            Some(StrayFit { id: id.clone(), km2, at, edges: by_edge(len) })
        })
        .into_iter()
        .flatten()
        .collect();
        for s in found {
            let (p, r) = &folder.regions[id];
            let mut files = vec![p.clone()];
            files.extend(s.edges.iter().filter_map(|(o, _)| folder.regions.get(o).map(|r| r.0.clone())));
            files.dedup();
            problems.push(Problem {
                warning: false,
                check: "rg-cover",
                about: vec![r.id.clone()],
                found: vec!["stray".into(), place_key(s.at)],
                files,
                message: format!(
                    "{id}: {:.2} km² of its water by {} touches none of its land ({}): water is the region's whose coast it lies off",
                    s.km2,
                    place(s.at),
                    neighbours(&s.edges)
                ),
            });
            fit.strays.push(s);
        }
    }
}

/// A piece of a cell's gap (what no region covers there).
struct Gap {
    c: Cell,
    sides: Sides,
    km2: f64,
    /// Its part inside the 200-nmi zones (any country's, `zones`), and a point inside that part's
    /// largest piece.
    zone_km2: f64,
    zone_at: Option<(f64, Pt)>,
    /// Its land, where it comes within 100 m of land, and a point inside its largest piece of it.
    land_km2: f64,
    land_at: Option<(f64, Pt)>,
    /// A point inside it.
    at: Pt,
    /// Within 100 m of land.
    touches: bool,
    /// Its edge (m) with each region, in the cell and across its sides.
    len: Vec<(String, f64)>,
    area: Area,
}

/// With `cover`, cell by cell: each pair of regions' area both have, and what no region covers,
/// joined across the cells' edges into stretches. Nothing is moved: an overlap past `HAIR_KM2`
/// (summed over the cells) is an error (`overlap`), as is a stretch in no region past it (`gap`),
/// each at a point inside it, naming the regions; but that the rest takes the open sea, a stretch
/// with no land that comes nowhere within 100 m of land, and either reaches the open sea (a cell
/// with no land in or round it: `far_from_land`) or lies `NEAR_LAND_M` or more from land (a hole
/// in the zones' water by a coast is a gap); the rest's part inside a 200-nmi zone past it is an
/// error (`rest-in-zone`: no region names that zone's water). Without a rest region every stretch
/// is a gap. Measured into `fit`. Returns the rest.
#[allow(clippy::too_many_arguments)]
fn cover_check(
    folder: &Folder,
    data: &super::data::Datasets,
    zones: &TArea,
    all: &BTreeMap<String, TArea>,
    cover: bool,
    rest: bool,
    problems: &mut Vec<Problem>,
    fit: &mut Fit,
    progress: &Progress,
) -> TArea {
    if !cover && !rest {
        return TArea::default();
    }
    let cells: Vec<Cell> = (-90..90).flat_map(|x| (-45..45).map(move |y| (x, y))).collect();
    // (Without cover: the rest is what no region covers, unchecked.)
    if !cover {
        progress.begin(Step::Cover, cells.len());
        let mut t = TArea::default();
        for (c, a) in pace::par_map(cells, |c| {
            let _one = progress.one_on_drop();
            let pieces: Vec<Area> = all.values().filter_map(|t| t.cells.get(&c).cloned()).collect();
            (c, geom::box_area(cell_box(c)).minus(&geom::union_all(&pieces)))
        }) {
            t.set(c, a);
        }
        return t;
    }
    type Pairs = Vec<((String, String), f64, f64, Option<(f64, Pt)>)>;
    let index = by_cell(all);
    progress.begin(Step::Cover, cells.len());
    let done: Vec<(Pairs, Vec<Gap>)> = pace::par_map(cells, |c| {
        let _one = progress.one_on_drop();
        geom::set_label(format!("the cover check, the cell from {}°, {}°", c.0 * 2, c.1 * 2));
        let pieces: Vec<(&String, &Area)> = all.iter().filter_map(|(id, t)| t.cells.get(&c).map(|a| (id, a))).collect();
        let land = land_and_lakes(data, c).0;
        let boxes: Vec<Option<[i64; 4]>> = pieces.iter().map(|p| p.1.bbox()).collect();
        let near = |a: &Option<[i64; 4]>, b: &Option<[i64; 4]>| match (a, b) {
            (Some(a), Some(b)) => a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3],
            _ => false,
        };
        let mut pairs: Pairs = Vec::new();
        for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                if !near(&boxes[i], &boxes[j]) {
                    continue;
                }
                let o = pieces[i].1.intersect(pieces[j].1);
                if o.is_empty() {
                    continue;
                }
                let l = geom::area_km2(&o.intersect(&land));
                let w = geom::area_km2(&o.minus(&land));
                pairs.push(((pieces[i].0.clone(), pieces[j].0.clone()), l, w, largest(&o)));
            }
        }
        let u = geom::union_all(&pieces.iter().map(|p| p.1.clone()).collect::<Vec<_>>());
        let gaps = geom::box_area(cell_box(c)).minus(&u);
        let zone = zones.get(c);
        // (Whether a gap is within 100 m of land, and the edge it shares with each region: read
        // from an index of the segments, no buffer per gap.)
        let (owners, pidx) = pieces_round(c, all, &index, None);
        let lidx = SegIndex::new(land.polys.iter().enumerate().flat_map(|(k, p)| p.iter().map(move |r| (k, r))));
        let land_boxes: Vec<Option<[i64; 4]>> = land.polys.iter().map(|p| geom::bbox_of(p.iter().flatten())).collect();
        let mut out: Vec<Gap> = Vec::new();
        for poly in gaps.polys {
            let gb = geom::bbox_of(poly.iter().flatten());
            let touches = poly.iter().flatten().any(|v| lidx.within(*v, 100.0))
                || poly.first().and_then(|r| r.first()).is_some_and(|v| {
                    land.polys.iter().zip(&land_boxes).any(|(lp, b)| near(b, &gb) && roadcore::geo::inside(lp, *v))
                })
                || land.polys.iter().zip(&land_boxes).any(|(lp, b)| {
                    near(b, &gb) && lp.first().and_then(|r| r.first()).is_some_and(|v| roadcore::geo::inside(&poly, *v))
                });
            let sides = sides(&poly, c);
            let at = interior(&poly);
            let mut len = vec![0.0f64; owners.len()];
            edges_with(&poly, &pidx, &mut len);
            let len = (0..owners.len()).filter(|&k| len[k] > 0.0).map(|k| (owners[k].clone(), len[k])).collect();
            let g = Area { polys: vec![poly] };
            let km2 = geom::area_km2(&g);
            // (Each against only the zones' and the land's part round it: a cell has tens of
            // thousands of gaps along a coast of islets, and the cell's land thousands of rings.)
            let part = |a: &Area| -> Option<Area> {
                let gb = g.bbox()?;
                match near_part(a, gb) {
                    Some(n) if n.is_empty() => None,
                    Some(n) => Some(g.intersect(&n)),
                    None => Some(g.intersect(a)),
                }
            };
            let (zone_km2, zone_at) = match part(&zone) {
                Some(z) => (geom::area_km2(&z), largest(&z)),
                None => (0.0, None),
            };
            let (land_km2, land_at) = match touches.then(|| part(&land)).flatten() {
                Some(on) => (geom::area_km2(&on), largest(&on)),
                None => (0.0, None),
            };
            out.push(Gap { c, sides, km2, zone_km2, zone_at, land_km2, land_at, at, touches, len, area: g });
        }
        (pairs, out)
    });
    let mut gaps: Vec<Gap> = Vec::new();
    for (pairs, g) in done {
        for (pair, land, water, at) in pairs {
            let e = fit.overlaps.entry(pair).or_default();
            e.land_km2 += land;
            e.water_km2 += water;
            if let Some((k, p)) = at.filter(|(k, _)| *k > e.largest_km2) {
                e.largest_km2 = k;
                e.at = p;
            }
        }
        gaps.extend(g);
    }
    for ((a, b), o) in &fit.overlaps {
        let km2 = o.land_km2 + o.water_km2;
        if km2 > HAIR_KM2 {
            problems.push(Problem {
                warning: false,
                check: "rg-cover",
                about: vec![a.clone(), b.clone()],
                found: vec!["overlap".into()],
                files: [a, b].iter().filter_map(|x| folder.regions.get(*x).map(|r| r.0.clone())).collect(),
                message: format!(
                    "{a} and {b} both cover {km2:.2} km² ({:.2} km² of land, {:.2} km² of water), the largest piece {:.2} km² by {}: a place is in one region",
                    o.land_km2,
                    o.water_km2,
                    o.largest_km2,
                    place(o.at)
                ),
            });
        }
    }
    trace("the gaps joined across the cells' edges");
    let roots = joined(&gaps.iter().map(|g| (g.c, &g.sides)).collect::<Vec<_>>());
    let mut stretches: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, r) in roots.into_iter().enumerate() {
        stretches.entry(r).or_default().push(i);
    }
    let groups: Vec<Vec<usize>> = stretches.into_values().collect();
    progress.begin(Step::Gaps, groups.len());
    let judged: Vec<(bool, GapFit)> = pace::par_map(groups.iter().collect(), |idx: &Vec<usize>| {
        let _one = progress.one_on_drop();
        let km2: f64 = idx.iter().map(|&i| gaps[i].km2).sum();
        let land_km2: f64 = idx.iter().map(|&i| gaps[i].land_km2).sum();
        let touches = idx.iter().any(|&i| gaps[i].touches);
        let mut len: BTreeMap<&String, f64> = BTreeMap::new();
        for &i in idx {
            for (id, l) in &gaps[i].len {
                *len.entry(id).or_default() += l;
            }
        }
        // (Its point: inside its largest piece of land, else its largest piece touching land, else
        // its largest.)
        let biggest = |f: &dyn Fn(&Gap) -> bool| {
            idx.iter().copied().filter(|&i| f(&gaps[i])).max_by(|&a, &b| gaps[a].km2.total_cmp(&gaps[b].km2))
        };
        let at = idx
            .iter()
            .filter_map(|&i| gaps[i].land_at)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
            .or_else(|| biggest(&|g| g.touches).map(|i| gaps[i].at))
            .or_else(|| biggest(&|_| true).map(|i| gaps[i].at))
            .unwrap_or([0, 0]);
        let open = || {
            idx.iter().any(|&i| far_from_land(data, gaps[i].c))
                || !idx.iter().any(|&i| land_within(data, gaps[i].at, NEAR_LAND_M))
        };
        let to_rest = rest && !touches && land_km2 <= HAIR_KM2 && open();
        let fit = GapFit {
            km2,
            zone_km2: idx.iter().map(|&i| gaps[i].zone_km2).sum(),
            land_km2,
            touches,
            at,
            rest: to_rest,
            edges: by_edge(len),
        };
        (km2 > HAIR_KM2, fit)
    });
    let mut rest_t: BTreeMap<Cell, Vec<Area>> = BTreeMap::new();
    for (idx, (over, g)) in groups.iter().zip(judged) {
        let files = || {
            let mut f = vec![super::resolve::RULES.to_string()];
            f.extend(g.edges.iter().take(6).filter_map(|(o, _)| folder.regions.get(o).map(|r| r.0.clone())));
            f
        };
        if g.rest {
            // (Its part inside a zone, cell by cell: the open sea's stretch is one round the
            // Earth, along every zone's edge.)
            let mut by: BTreeMap<Cell, Vec<usize>> = BTreeMap::new();
            for &i in idx {
                by.entry(gaps[i].c).or_default().push(i);
            }
            for (c, is) in by {
                let km2: f64 = is.iter().map(|&i| gaps[i].zone_km2).sum();
                if km2 > HAIR_KM2 {
                    let at = is
                        .iter()
                        .filter_map(|&i| gaps[i].zone_at)
                        .max_by(|a, b| a.0.total_cmp(&b.0))
                        .map_or(g.at, |x| x.1);
                    let mut len: BTreeMap<&String, f64> = BTreeMap::new();
                    for &i in &is {
                        for (id, l) in &gaps[i].len {
                            *len.entry(id).or_default() += l;
                        }
                    }
                    let edges = by_edge(len);
                    let mut files = vec![super::resolve::RULES.to_string()];
                    files.extend(edges.iter().take(6).filter_map(|(o, _)| folder.regions.get(o).map(|r| r.0.clone())));
                    problems.push(Problem {
                        warning: false,
                        check: "rg-cover",
                        about: vec![format!("{},{}", c.0 * 2, c.1 * 2)],
                        found: vec!["rest-in-zone".into()],
                        files,
                        message: format!(
                            "{km2:.2} km² of water inside a 200-nmi zone in the cell from {}°, {}°, the largest piece by {}, is the open sea's ({}): a zone's water is a country's region's",
                            c.0 * 2,
                            c.1 * 2,
                            place(at),
                            neighbours(&edges)
                        ),
                    });
                }
            }
            for &i in idx {
                rest_t.entry(gaps[i].c).or_default().push(std::mem::take(&mut gaps[i].area));
            }
        } else if over {
            let land = if g.land_km2 > 0.0 { format!(", {:.2} km² of it land", g.land_km2) } else { String::new() };
            problems.push(Problem {
                warning: false,
                check: "rg-cover",
                about: vec![place_key(g.at)],
                found: vec!["gap".into()],
                files: files(),
                message: format!(
                    "{:.2} km²{land} by {} is in no region ({}): every place is in one region",
                    g.km2,
                    place(g.at),
                    neighbours(&g.edges)
                ),
            });
        }
        fit.gaps.push(g);
    }
    progress.begin(Step::Rest, rest_t.len());
    let mut t = TArea::default();
    for (c, a) in pace::par_map(rest_t.into_iter().collect(), |(c, v): (Cell, Vec<Area>)| {
        let _one = progress.one_on_drop();
        (c, geom::union_all(&v))
    }) {
        t.set(c, a);
    }
    t
}

/// Whether no land lies in the cell or round it: the rows above and below, and as many columns
/// either side as make at least 111 km at the cell's poleward edge (one up to 60°, more nearer a
/// pole, where a column narrows: at 80° three, 116 km; every column at the poles' rows). So at
/// least 110 km from any coast: no lagoon or creek reaches there.
pub(super) fn far_from_land(data: &super::data::Datasets, c: Cell) -> bool {
    let Some(land) = &data.land else { return true };
    let edge = (c.1 * 2).abs().max((c.1 * 2 + 2).abs()) as f64;
    let cos = edge.to_radians().dcos();
    let kx = ((0.5 / cos.max(1e-9)).ceil() as i64).clamp(1, COLS / 2);
    (c.0 - kx..=c.0 + kx).all(|x| (c.1 - 1..=c.1 + 1).all(|y| !(-45..45).contains(&y) || !land.has((wrap(x).0, y))))
}

/// How near land a stretch touching none is still a country's (a hole in the zones' water by a
/// coast: the Indus delta's tidal flats, New Caledonia's lagoon): the open sea beyond every zone
/// lies 200 nmi (370 km) or more from its coasts.
pub const NEAR_LAND_M: f64 = 100_000.0;

/// Whether land lies within `d` metres of point `p`.
fn land_within(data: &super::data::Datasets, p: Pt, d: f64) -> bool {
    let lat = (p[1] as f64 * 1e-7).clamp(-89.0, 89.0);
    let cos = lat.to_radians().dcos();
    let dy = (1.5 * d / roadcore::geo::M_PER_DEG * 1e7) as i64;
    let dx = (dy as f64 / cos.max(0.01)) as i64;
    let land = data.land_in([p[0] - dx, p[1] - dy, p[0] + dx, p[1] + dy]);
    land.polys.iter().any(|poly| {
        roadcore::geo::inside(poly, p)
            || poly.iter().any(|r| (0..r.len()).any(|k| seg_dist_m(p, r[k], r[(k + 1) % r.len()], cos) < d))
    })
}

/// Every 200-nmi zone of the datasets with its internal waters, by cell: where water is some
/// country's.
fn zones(data: &super::data::Datasets, progress: &Progress) -> TArea {
    let shapes: Vec<&Vec<Ring>> = data.eez.values().chain(data.internal.values()).collect();
    progress.begin(Step::Zones, shapes.len());
    let parts: Vec<TArea> = pace::par_map(shapes, |rs| {
        let _one = progress.one_on_drop();
        TArea::from_rings(rs)
    });
    let mut by: BTreeMap<Cell, Vec<Area>> = BTreeMap::new();
    for t in parts {
        for (c, a) in t.cells {
            by.entry(c).or_default().push(a);
        }
    }
    progress.begin(Step::ZoneCells, by.len());
    let mut t = TArea::default();
    for (c, a) in pace::par_map(by.into_iter().collect(), |(c, v): (Cell, Vec<Area>)| {
        let _one = progress.one_on_drop();
        (c, geom::union_all(&v))
    }) {
        t.set(c, a);
    }
    t
}

/// A point inside a polygon: the middle of the widest stretch inside it along its box's middle row.
fn interior(poly: &[Ring]) -> Pt {
    let Some(b) = geom::bbox_of(poly.iter().flatten()) else { return [0, 0] };
    // (Halfway between two rows of whole numbers: no vertex on it.)
    let y = (b[1] + b[3]) as f64 / 2.0 + 0.25;
    let mut xs: Vec<f64> = Vec::new();
    for r in poly {
        let n = r.len();
        for i in 0..n {
            let (p, q) = (r[i], r[(i + 1) % n]);
            if (p[1] as f64 > y) != (q[1] as f64 > y) {
                xs.push(p[0] as f64 + (y - p[1] as f64) * (q[0] - p[0]) as f64 / (q[1] - p[1]) as f64);
            }
        }
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    xs.chunks(2)
        .filter(|w| w.len() == 2)
        .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .map(|w| [((w[0] + w[1]) / 2.0).round() as i64, y.round() as i64])
        .unwrap_or(poly[0][0])
}

/// The members a hand-out from `id` to folder `g` gives to, each with the regions it stands for:
/// each of `members` (the regions of `g` with water) for itself, but that the outermost folder
/// under `g` holding a waters region (`Region::is_waters`, in the folder itself, not deeper), and
/// not holding `id`, is one member, its waters region taking what the folder's regions border
/// alone. Two waters regions in such a folder are a finding (`rg-ref`), the first by id taking.
fn standing(
    folder: &Folder,
    g: &str,
    id: &str,
    members: &[String],
    problems: &mut Vec<Problem>,
) -> BTreeMap<String, Vec<String>> {
    let own = &folder.regions[id].0;
    let mut waters: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for m in members {
        let (p, r) = &folder.regions[m];
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
        if r.is_waters() && dir.len() > g.len() && !own.starts_with(&format!("{dir}/")) {
            waters.entry(dir).or_default().push(m);
        }
    }
    for (dir, ws) in &waters {
        if let [first, second, ..] = ws[..] {
            let (p, r) = &folder.regions[second];
            problems.push(problem("rg-ref", r, p, format!("{second}: {dir} holds two waters regions, {first} and {second}: a hand-out gives the folder's sea to one")));
        }
    }
    let mut units: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in members {
        let p = &folder.regions[m].0;
        // (Its folders under `g`, outermost first.)
        let to = p.match_indices('/').map(|(i, _)| &p[..i]).filter(|d| d.len() > g.len()).find_map(|d| waters.get(d));
        units.entry(to.map_or(m, |ws| ws[0]).clone()).or_default().push(m.clone());
    }
    units
}

/// Tiled areas' union, cell by cell, each cell a part of `progress`'s item under way.
fn union_of(ts: &[&TArea], progress: &Progress) -> TArea {
    let cells: BTreeSet<Cell> = ts.iter().flat_map(|t| t.cells.keys().copied()).collect();
    let v: Vec<(Cell, Area)> = pace::par_map(cells.into_iter().collect(), |c| {
        let _part = progress.part_on_drop();
        (c, geom::union_all(&ts.iter().filter_map(|t| t.cells.get(&c).cloned()).collect::<Vec<_>>()))
    });
    let mut u = TArea::default();
    for (c, a) in v {
        u.set(c, a);
    }
    u
}

/// The cells' pieces as one area: drawn apart, each would be outlined along the cell edges it
/// shares with its neighbours.
pub(super) fn dissolved(t: &TArea) -> Area {
    geom::union_all(&t.cells.values().cloned().collect::<Vec<_>>())
}

/// A region's stored geometry and drawing: its cells' rings, its `from` its parts; a shape of its
/// land (the coastline's, less the lakes over 100 km²) and one of its water, worked out cell by cell
/// from what it ends with (the coast's and the hand-outs' included: a region whose own parts had no
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
    let tol = buildstore::coverage::draw_tolerance_m(&rings32(t));
    let polys: Vec<Vec<Vec<[f64; 2]>>> = dissolved(t)
        .polys
        .iter()
        .map(|p| {
            p.iter()
                .map(|r| {
                    buildstore::coverage::ring_degrees(&buildstore::outlines::simplify(&super::shapes::to_i32(r), tol))
                })
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

/// The part of segment a–z inside box `b` (Liang–Barsky), as two points; None outside.
fn clip_segment(a: Pt, z: Pt, b: [i64; 4]) -> Option<Ring> {
    let (dx, dy) = ((z[0] - a[0]) as f64, (z[1] - a[1]) as f64);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, (a[0] - b[0]) as f64),
        (dx, (b[2] - a[0]) as f64),
        (-dy, (a[1] - b[1]) as f64),
        (dy, (b[3] - a[1]) as f64),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    let at = |t: f64| [a[0] + (dx * t).round() as i64, a[1] + (dy * t).round() as i64];
    (t0 <= t1).then(|| vec![at(t0), at(t1)])
}

/// Metres along a segment (a local equirectangular measure: segments here are short).
fn seg_m(a: geom::Pt, b: geom::Pt) -> f64 {
    let lat = ((a[1] + b[1]) as f64 / 2e7).to_radians();
    let dx = (b[0] - a[0]) as f64 * lat.dcos();
    let dy = (b[1] - a[1]) as f64;
    (dx * dx + dy * dy).sqrt() * (roadcore::geo::M_PER_DEG * 1e-7)
}

/// Rings' segments in a grid of about 2 km, each with its owner's number.
#[derive(Default)]
pub(super) struct SegIndex {
    grid: std::collections::HashMap<(i64, i64), Vec<(usize, geom::Pt, geom::Pt)>>,
}

const SEG_CELL: i64 = 20_000;

impl SegIndex {
    pub(super) fn new<'a>(rings: impl Iterator<Item = (usize, &'a geom::Ring)>) -> Self {
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

    /// Ring `r`'s segments, shifted by `shift` in longitude, those meeting box `b`, as owner `k`'s.
    fn add(&mut self, k: usize, r: &geom::Ring, shift: i64, b: [i64; 4]) {
        let n = r.len();
        for i in 0..n {
            let (a, c) = ([r[i][0] + shift, r[i][1]], [r[(i + 1) % n][0] + shift, r[(i + 1) % n][1]]);
            if a[0].max(c[0]) < b[0] || a[0].min(c[0]) > b[2] || a[1].max(c[1]) < b[1] || a[1].min(c[1]) > b[3] {
                continue;
            }
            for x in a[0].min(c[0]).div_euclid(SEG_CELL)..=a[0].max(c[0]).div_euclid(SEG_CELL) {
                for y in a[1].min(c[1]).div_euclid(SEG_CELL)..=a[1].max(c[1]).div_euclid(SEG_CELL) {
                    self.grid.entry((x, y)).or_default().push((k, a, c));
                }
            }
        }
    }

    /// The segments in grid squares meeting box `b` (some twice, as many squares as they cross).
    fn segments_in(&self, b: [i64; 4]) -> impl Iterator<Item = (usize, geom::Pt, geom::Pt)> + '_ {
        (b[0].div_euclid(SEG_CELL)..=b[2].div_euclid(SEG_CELL))
            .flat_map(move |x| (b[1].div_euclid(SEG_CELL)..=b[3].div_euclid(SEG_CELL)).map(move |y| (x, y)))
            .filter_map(move |c| self.grid.get(&c))
            .flatten()
            .copied()
    }

    /// The segments within `d` metres of a point, with their distance in metres.
    fn near(&self, p: geom::Pt, d: f64) -> impl Iterator<Item = (usize, f64)> + '_ {
        let cos = (p[1] as f64 / 1e7).to_radians().dcos().max(0.01);
        let (ry, rx) = (
            (d / (roadcore::geo::M_PER_DEG * 1e-7)) as i64 + 1,
            (d / (roadcore::geo::M_PER_DEG * 1e-7) / cos) as i64 + 1,
        );
        let (x0, x1) = ((p[0] - rx).div_euclid(SEG_CELL), (p[0] + rx).div_euclid(SEG_CELL));
        let (y0, y1) = ((p[1] - ry).div_euclid(SEG_CELL), (p[1] + ry).div_euclid(SEG_CELL));
        (x0..=x1)
            .flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
            .filter_map(move |c| self.grid.get(&c))
            .flatten()
            .map(move |&(k, a, b)| (k, seg_dist_m(p, a, b, cos)))
            .filter(move |(_, m)| *m <= d)
    }

    pub(super) fn within(&self, p: geom::Pt, d: f64) -> bool {
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
    (q[0] * q[0] + q[1] * q[1]).sqrt() * (roadcore::geo::M_PER_DEG * 1e-7)
}
