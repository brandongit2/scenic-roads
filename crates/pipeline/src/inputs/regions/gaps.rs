//! Thin gaps between regions (docs/inputs.md §6.1.8, `rg-gap`, a warning): places where two shapes
//! of the coverage leave a strip narrower than `GAP_M` between them, which the build would leave out
//! though it's almost surely meant in (a GeoJSON drawn a little short of its neighbour).
//!
//! A morphological close: the coverage grown by half the width and shrunk back (`geom::dilate` of it,
//! then of what's outside it) fills every strip narrower than the width; what it fills that the
//! coverage hasn't is a gap. Counted only when it meets two shapes (a narrow inlet of one shape is a
//! coast, not a gap) and is at least `MIN_LENGTH_M` long (a close also rounds the corner where two
//! touching shapes meet a coast bending inward: a wedge under a kilometre or so). What a shape with the
//! coast tolerance takes in (everything within its tolerance) is taken out of each gap first, and
//! the rules applied to what's left: a long gap between two drawn regions that ends at an `osm:`
//! region still counts.
//!
//! Done only where it can matter, so it's fast on the real set: the edges of shapes without the
//! tolerance not shared with another shape (shared borders, the same OSM ways, are no gap) are put in
//! cells of about `GAP_M`; a cell with another such shape's edges a cell away marks its tile
//! (`TILE_E7`), and only those tiles are closed, each within a window wide enough that the close is
//! exact in the tile. The tiles' gaps are joined, so a long gap is one finding. Integer set
//! operations, fixed tiles, sorted output: the same regions give the same gaps on every machine.

use super::geom::{self, Area, Pt, Ring};
use super::stored::Geometry;
use det::Det;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

/// The width under which a strip between two shapes is a gap: 1 km. A road in a strip that narrow is
/// left out of the build though it lies between two regions, and nobody leaves out a strip under a
/// kilometre on purpose (the narrowest exclusions in use, a neighbour's whole area, are far wider),
/// while hand-drawn edges miss their neighbour by tens to hundreds of metres. It's also the coast
/// tolerance (`OSM_BUFFER_M`): shapes with it already take in anything this narrow beside them.
pub const GAP_M: f64 = 1_000.0;
/// A gap shorter than this is a close's rounding of a corner, not a strip (see above).
pub const MIN_LENGTH_M: f64 = 3.0 * GAP_M;
/// A gap thinner than this on average is two shapes' vertices not quite agreeing along a border they
/// share (a sliver a metre or less wide), which nothing in the build notices.
pub const MIN_WIDTH_M: f64 = 1.0;
/// The tiles the close is done in: a quarter degree.
const TILE_E7: i64 = 2_500_000;
/// The tiles a block is (each way) that the shapes are first clipped to: two degrees.
const BLOCK: i64 = 8;
/// Metres in a degree of latitude, near enough for cell and window sizes.
const M_PER_DEG: f64 = 111_320.0;
/// The sides a whole circle of the close's arcs is drawn with (16: within 2 % of the radius).
const SIDES: usize = 16;

/// A gap found.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    /// The regions it lies between (sorted), and the shapes ("<id>: <from>", sorted).
    pub regions: Vec<String>,
    pub shapes: Vec<String>,
    /// A point in it (E7), its mean width and its length (metres).
    pub at: Pt,
    pub width_m: f64,
    pub length_m: f64,
}

/// One stored shape of a region.
struct Unit<'a> {
    region: &'a str,
    from: &'a str,
    rings: Rings,
    bbox: [i64; 4],
    /// Without the coast tolerance: what gaps are looked for between.
    plain: bool,
    /// Its coast tolerance (metres; 0 for none).
    tol: f64,
}

fn meets(a: [i64; 4], b: [i64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn grow(b: [i64; 4], m: f64) -> [i64; 4] {
    let lat = (b[1].abs().max(b[3].abs()) as f64 * 1e-7).min(85.0).to_radians();
    let dy = (m / M_PER_DEG * 1e7).ceil() as i64;
    let dx = (m / (M_PER_DEG * lat.dcos()) * 1e7).ceil() as i64;
    [b[0] - dx, b[1] - dy, b[2] + dx, b[3] + dy]
}

/// Rings with their boxes.
#[derive(Default)]
struct Rings {
    rings: Vec<Ring>,
    boxes: Vec<[i64; 4]>,
}

impl Rings {
    fn new(rings: Vec<Ring>) -> Rings {
        let boxes = rings.iter().map(|r| geom::bbox_of(r.iter()).unwrap_or([0, 0, -1, -1])).collect();
        Rings { rings, boxes }
    }

    /// What of them is within the box `b`.
    fn clip(&self, b: [i64; 4]) -> Rings {
        Rings::new(
            self.rings
                .iter()
                .zip(&self.boxes)
                .filter(|(_, rb)| meets(**rb, b))
                .filter_map(|(r, _)| geom::clip_ring(r, b))
                .collect(),
        )
    }

    /// Their area (even–odd).
    fn area(&self) -> Area {
        if self.rings.is_empty() {
            return Area::default();
        }
        Area::from_rings(&self.rings)
    }
}

fn edge_key(p: Pt, q: Pt) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    (p.min(q), p.max(q)).hash(&mut h);
    h.finish()
}

/// The gaps between the shapes of `geoms`.
pub fn gaps(geoms: &[&Geometry]) -> Vec<Gap> {
    let mut units: Vec<Unit> = Vec::new();
    for g in geoms {
        for s in &g.shapes {
            let rings: Vec<Ring> =
                s.rings.iter().map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect()).collect();
            let Some(bbox) = geom::bbox_of(rings.iter().flatten()) else { continue };
            let rings = Rings::new(rings);
            units.push(Unit {
                region: &g.id,
                from: &s.from,
                rings,
                bbox,
                plain: (s.tolerance_m as f64) < GAP_M,
                tol: s.tolerance_m as f64,
            });
        }
    }
    if units.iter().filter(|u| u.plain).count() < 2 {
        return Vec::new();
    }
    // Edges two plain shapes share: no gap there.
    let mut owner: HashMap<u64, (u32, bool)> = HashMap::new();
    for (k, u) in units.iter().enumerate().filter(|(_, u)| u.plain) {
        for r in &u.rings.rings {
            for i in 0..r.len() {
                let e = owner.entry(edge_key(r[i], r[(i + 1) % r.len()])).or_insert((k as u32, false));
                if e.0 != k as u32 {
                    e.1 = true;
                }
            }
        }
    }
    // The cells each plain shape's other edges pass through.
    let cell = (GAP_M / M_PER_DEG * 1e7) as i64;
    let mut cells: HashMap<[i64; 2], Vec<u32>> = HashMap::new();
    for (k, u) in units.iter().enumerate().filter(|(_, u)| u.plain) {
        for r in &u.rings.rings {
            for i in 0..r.len() {
                let (p, q) = (r[i], r[(i + 1) % r.len()]);
                if owner.get(&edge_key(p, q)).is_some_and(|e| e.1) {
                    continue;
                }
                let steps = ((p[0] - q[0]).abs().max((p[1] - q[1]).abs()) * 2 / cell).max(0) + 1;
                for j in 0..=steps {
                    let x = p[0] + (q[0] - p[0]) * j / steps;
                    let y = p[1] + (q[1] - p[1]) * j / steps;
                    let c = cells.entry([x.div_euclid(cell), y.div_euclid(cell)]).or_default();
                    if !c.contains(&(k as u32)) {
                        c.push(k as u32);
                    }
                }
            }
        }
    }
    // The tiles where two plain shapes' free edges come within a cell or so of each other.
    let mut tiles: BTreeSet<[i64; 2]> = BTreeSet::new();
    for (c, here) in &cells {
        let lat = (((c[1].abs() + 2) * cell) as f64 * 1e-7).min(85.0).to_radians();
        let kx = (1.0 / lat.dcos()).ceil() as i64;
        let near = (-1..=1).any(|dy| {
            (-kx..=kx).any(|dx| {
                cells
                    .get(&[c[0] + dx, c[1] + dy])
                    .is_some_and(|there| there.iter().any(|v| here.iter().any(|u| u != v)))
            })
        });
        if near {
            // (Every tile the neighbourhood touches: a gap across a tile's edge is closed on both sides.)
            for dy in -1..=1 {
                for dx in -kx..=kx {
                    tiles.insert([((c[0] + dx) * cell).div_euclid(TILE_E7), ((c[1] + dy) * cell).div_euclid(TILE_E7)]);
                    tiles.insert([
                        ((c[0] + dx + 1) * cell).div_euclid(TILE_E7),
                        ((c[1] + dy + 1) * cell).div_euclid(TILE_E7),
                    ]);
                }
            }
        }
    }
    // Each tile closed within its window.
    let r = GAP_M / 2.0;
    let mut found: Vec<Area> = Vec::new();
    // (The shapes clipped first to a block of tiles, so a long ring is read once a block, not a tile.)
    let mut block: Option<([i64; 2], Vec<(bool, Rings)>)> = None;
    let mut order: Vec<[i64; 2]> = tiles.into_iter().collect();
    order.sort_by_key(|t| (t[0].div_euclid(BLOCK), t[1].div_euclid(BLOCK), t[0], t[1]));
    for t in &order {
        let tile = [t[0] * TILE_E7, t[1] * TILE_E7, (t[0] + 1) * TILE_E7, (t[1] + 1) * TILE_E7];
        // (A close at a point depends on what's within twice the radius: the window is that and more.)
        let window = grow(tile, 2.0 * GAP_M);
        let bk = [t[0].div_euclid(BLOCK), t[1].div_euclid(BLOCK)];
        if block.as_ref().is_none_or(|(k, _)| *k != bk) {
            let s = BLOCK * TILE_E7;
            let b = grow([bk[0] * s, bk[1] * s, (bk[0] + 1) * s, (bk[1] + 1) * s], 2.0 * GAP_M);
            let local = units.iter().filter(|u| meets(u.bbox, b)).map(|u| (u.plain, u.rings.clip(b))).collect();
            block = Some((bk, local));
        }
        let local = &block.as_ref().expect("made above").1;
        let areas: Vec<(bool, Area)> =
            local.iter().map(|(p, rs)| (*p, rs.clip(window).area())).filter(|(_, a)| !a.is_empty()).collect();
        if areas.iter().filter(|(p, _)| *p).count() < 2 {
            continue;
        }
        let all: Vec<Area> = areas.into_iter().map(|(_, a)| a).collect();
        let cover = geom::union_all(&all);
        let grown = geom::dilate(&cover, r, SIDES);
        let outside = geom::box_area(grow(window, 2.0 * GAP_M)).minus(&grown);
        let closed = grown.minus(&geom::dilate(&outside, r, SIDES));
        let gap = closed.minus(&cover).intersect(&geom::box_area(tile));
        if !gap.is_empty() {
            found.push(gap);
        }
    }
    if found.is_empty() {
        return Vec::new();
    }
    // The tiles' gaps joined; less what shapes with the coast tolerance take in; each piece measured
    // and its shapes found.
    let mut pieces: Vec<Area> = Vec::new();
    for poly in geom::union_all(&found).polys {
        let mut piece = Area { polys: vec![poly] };
        let Some(pb) = piece.bbox() else { continue };
        for u in units.iter().filter(|u| !u.plain) {
            let b = grow(pb, u.tol + GAP_M);
            if meets(u.bbox, b) {
                let a = u.rings.clip(b).area();
                if !a.is_empty() {
                    piece = piece.minus(&geom::dilate(&a, u.tol, SIDES));
                }
            }
        }
        pieces.extend(piece.polys.into_iter().map(|p| Area { polys: vec![p] }));
    }
    let mut out = Vec::new();
    for piece in pieces {
        let (area, perimeter) = measure(&piece);
        let length = perimeter / 2.0;
        if length < MIN_LENGTH_M || perimeter <= 0.0 || 2.0 * area / perimeter < MIN_WIDTH_M {
            continue;
        }
        let near = geom::dilate(&piece, 30.0, 8);
        let Some(nb) = near.bbox() else { continue };
        let met: Vec<&Unit> = units
            .iter()
            .filter(|u| u.plain && meets(u.bbox, nb) && !u.rings.clip(nb).area().intersect(&near).is_empty())
            .collect();
        if met.len() < 2 {
            continue;
        }
        let regions: BTreeSet<String> = met.iter().map(|u| u.region.to_string()).collect();
        let shapes: BTreeSet<String> = met.iter().map(|u| format!("{}: {}", u.region, u.from)).collect();
        out.push(Gap {
            regions: regions.into_iter().collect(),
            shapes: shapes.into_iter().collect(),
            at: inside_point(&piece),
            width_m: 2.0 * area / perimeter,
            length_m: length,
        });
    }
    out.sort_by(|a, b| (&a.regions, a.at).cmp(&(&b.regions, b.at)));
    out
}

/// A piece's area (m², holes out) and its outer ring's perimeter (m: what its length is half of),
/// on the ground near enough for a few kilometres.
fn measure(a: &Area) -> (f64, f64) {
    let (mut area, mut per) = (0.0, 0.0);
    for (k, r) in a.polys.iter().flat_map(|p| p.iter().enumerate()) {
        let n = r.len();
        let lat = (r[0][1] as f64 * 1e-7).to_radians();
        let (sx, sy) = (M_PER_DEG * 1e-7 * lat.dcos(), M_PER_DEG * 1e-7);
        let mut s = 0.0;
        for i in 0..n {
            let (p, q) = (r[i], r[(i + 1) % n]);
            s += ((p[0] - r[0][0]) as f64 * sx) * ((q[1] - r[0][1]) as f64 * sy)
                - ((q[0] - r[0][0]) as f64 * sx) * ((p[1] - r[0][1]) as f64 * sy);
            if k == 0 {
                per += geom::length_m(p, q);
            }
        }
        // (Outer rings counter-clockwise, holes clockwise: their signed areas sum to the piece's.)
        area += s / 2.0;
    }
    (area.max(0.0), per)
}

/// A point in the piece: the middle of its widest crossing along the parallel through its box's
/// middle latitude (or its first point, should that miss it).
fn inside_point(a: &Area) -> Pt {
    let first = a.polys[0][0][0];
    let Some(b) = a.bbox() else { return first };
    let mut best: Option<(i64, Pt)> = None;
    // (A few parallels across it: a thin strip running east–west may lie off the middle one.)
    for k in 1..8 {
        let y = b[1] + (b[3] - b[1]) * k / 8;
        let mut xs: Vec<i64> = Vec::new();
        for r in a.polys.iter().flatten() {
            for i in 0..r.len() {
                let (p, q) = (r[i], r[(i + 1) % r.len()]);
                if (p[1] <= y) != (q[1] <= y) {
                    xs.push(p[0] + ((q[0] - p[0]) as i128 * (y - p[1]) as i128 / (q[1] - p[1]) as i128) as i64);
                }
            }
        }
        xs.sort();
        for w in xs.chunks(2) {
            if let [x0, x1] = w {
                let m = [(x0 + x1) / 2, y];
                if best.is_none_or(|(d, _)| x1 - x0 > d) && a.contains(m) {
                    best = Some((x1 - x0, m));
                }
            }
        }
    }
    best.map_or(first, |(_, m)| m)
}

/// The gaps as findings' parts: the regions, a place to the 100th of a degree, and the words. The
/// place is a point inside the gap, which depends on the gap's whole shape: an edit elsewhere leaves
/// it, but one that changes the gap (or a point landing on a rounding edge) can move it and ask for
/// the gap to be accepted again.
pub fn describe(g: &Gap) -> (Vec<String>, String, String) {
    let (lon, lat) = (g.at[0] as f64 * 1e-7, g.at[1] as f64 * 1e-7);
    let place = format!("{lon:.2},{lat:.2}");
    let between = if g.regions.len() == 1 {
        format!("{}'s parts {}", g.regions[0], g.shapes.join(" and "))
    } else {
        g.regions.join(" and ")
    };
    let message = format!(
        "{between} leave a gap about {:.0} m wide and {:.1} km long at {lat:.5}, {lon:.5}: narrower than {:.0} km, likely \
         a mistake (the build leaves it out); accept if it's meant",
        (g.width_m / 10.0).round() * 10.0,
        g.length_m / 1000.0,
        GAP_M / 1000.0
    );
    (g.regions.clone(), place, message)
}

/// For the memo: the gaps of the geometry by its bytes.
pub fn key(geoms: &[&Geometry]) -> String {
    let mut m: BTreeMap<&str, String> = BTreeMap::new();
    for g in geoms {
        m.insert(&g.id, store::naming::hash16(&g.encode()));
    }
    store::naming::hash16(format!("{m:?}").as_bytes())
}
