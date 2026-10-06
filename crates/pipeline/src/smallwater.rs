//! Small islands and lakes, worldwide (docs/plan.md §6 "Small islands and lakes"; docs/formats.md
//! "Small islands and lakes"): what the basemap leaves out zoomed out, kept on the map as the roads
//! are, faked where it's too small to draw.
//!
//! The basemap (Planetiler's OpenMapTiles profile, docs/plan.md §6 step 6) leaves a polygon out at a
//! zoom where its outline's area, in 256-px tile pixels, is under a minimum: an outer ring and each
//! hole alike, measured after Planetiler's Douglas–Peucker simplification at that zoom (0.1 px) and
//! before any clipping. The sea's islands (the pinned water polygons' holes) go under 1 px² (z6–13);
//! lakes and the other inland water, and their islands, under 4 px² below z12 and 1 px² at z12–13;
//! at z14, under 1/256 px². Below z6 the basemap has Natural Earth's water alone. Sampled on its
//! tiles (2026-10-06, z6–11): the rule so measured predicts 99.7 % of 91,537 sea islands near the
//! minimum (five z6 tiles) and 99.5 % of 159,208 lakes (north of Mont-Laurier), the area before
//! simplification 98.1 % and 99.1 %.
//!
//! This step reads every island and lake: the lakes, rivers and other water areas from the pass's
//! `water` set, their holes as islands, and the sea's islands as the holes of the basemap's own
//! water polygons (`sources/basemap/water-polygons-split-3857.zip`); it works out the zooms the
//! basemap lacks each at (z6–13 by the same rule, checked against the basemap's tiles each run;
//! z0–5 by asking its tiles whether its water is there), and tiles them, zooms 0–12, one MVT layer
//! `w`: at each zoom, what the basemap lacks there,
//! - under 1 px² (at z12, all: what the basemap lacks there is under 1 px², or a hair over and
//!   simplified under): a point, the app's dot, sized by its true area and faded with it (web/src/
//!   basemap.ts); the points of one 1-px cell and kind are one, with their summed area, at the
//!   biggest's place, in cells twice or four times as wide where a 16-px block would hold more
//!   than `BLOCK_POINTS`;
//! - from 1 px² to z11 (lakes and their islands under 4 px² at z6–11; anything Natural Earth lacks
//!   below z6): its outline, simplified as the basemap's are, drawn as the basemap would draw it.
//!
//! An island of a lake is drawn wherever its lake is (whose dot or outline would cover it). Water
//! along a line (a river's, a canal's) is no lake, so it has no dot of its own: it's read as the
//! parent of its islands, which are drawn only at the zooms the basemap draws it (else they'd be
//! land on bare land: a braided river's).
//!
//! Properties: `k` (0 an island of the sea, 1 a lake, 2 an island of a lake or river), on points
//! `q` (the area, Web Mercator m², as round(8 log2)), and in z12 tiles `o` (1: still absent at z13,
//! where the app overzooms z12). In a tile, bigger first, so what lies inside something (a pond on
//! an island in a lake) is drawn over it.

use anyhow::{bail, Context, Result};
use det::Det;
use names::mvt::{Feature as MvtFeature, Layer, Tile, Value};
use rayon::prelude::*;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::io::{BufRead, Read};

/// The layer (`layers/smallwater/…`, served at `/tiles/smallwater`).
pub const LAYER: &str = "smallwater";
/// What the basemap draws as water, as osmium's area tags (the export's `area_tags`): OpenMapTiles'
/// water polygons (natural=water, the reservoir, basin and salt pond land uses, docks, and
/// water=river … wastewater; not bays, which it reads but doesn't draw, nor swimming pools and
/// springs, which it maps but the basemap's input leaves out). `water_kind` tells them apart.
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
/// The pass's `water` set's filter (pipeline::osmpass::SETS): those areas.
pub const SET_FILTER: &[&str] = &["wr/natural=water", "wr/landuse=reservoir,basin,salt_pond", "wr/waterway=dock", "wr/water=river,stream,canal,ditch,drain,pond,basin,wastewater"];
/// The basemap's sea: Planetiler's water polygons, pinned beside its jar (docs/plan.md §6 step 6),
/// and the shapefile in it.
pub const WATER_POLYGONS: &str = "sources/basemap/water-polygons-split-3857.zip";
pub const WATER_POLYGONS_SHP: &str = "water-polygons-split-3857/water_polygons.shp";
/// The kinds: an island (of the sea, or one of a lake or river: `Feat::sea` says which), a lake,
/// and water along a line (a river's, a canal's: never drawn, the parent of its islands).
pub const ISLAND: u8 = 0;
pub const LAKE: u8 = 1;
pub const RIVER: u8 = 2;
/// The tiles' `k`: an island of the sea, a lake, an island of a lake or a river (the coastal
/// shading draws the sea's shores, and lakes' and rivers' only when asked to).
pub const K_SEA_ISLAND: u64 = 0;
pub const K_LAKE: u64 = 1;
pub const K_INLAND_ISLAND: u64 = 2;

/// A feature's `k` in the tiles.
fn tile_kind(f: &Feat) -> u8 {
    (match (f.kind, f.sea) {
        (LAKE, _) => K_LAKE,
        (_, true) => K_SEA_ISLAND,
        _ => K_INLAND_ISLAND,
    }) as u8
}
/// The deepest tiles (the app overzooms them to z13; from z14 the basemap has everything).
pub const MAXZ: u8 = 12;
/// The deepest zoom with outlines: deeper, everything the basemap lacks is under 1 px², or within a
/// hair of it (its outline simplified under the minimum), and a point.
const OUTLINE_MAXZ: u8 = 11;
/// The tiles' one layer.
pub const MVT_LAYER: &str = "w";
const EXTENT: u32 = 4096;
/// The Web Mercator world's width, metres (2πR).
pub const WORLD_M: f64 = 40_075_016.685_578_49;
/// What the basemap's Natural Earth zooms (0–5) may have: smaller is taken as missing there.
const NE_MIN_M2: f64 = 1e6;
/// Smaller than this (m²) is a mapping slip, not an island or a pond.
const MIN_M2: f64 = 1.0;
/// The basemap's simplification (Planetiler's default tolerance below its deepest zoom, 256-px tile
/// pixels), which its outlines here follow too.
const TOLERANCE_PX: f64 = 0.1;
/// The most points a block of 16 × 16 px holds (a tile has 256): where more of its cells of 1 px
/// have some (a lake district zoomed out), they're summed in cells twice as wide, then four times,
/// until it holds no more. So a tile holds at most 16,384, and the app, which draws a circle a
/// point, ~200,000 in a view of a dozen, within an iPad's means. (By block, not by tile: a crowded
/// tile summed whole beside one that wasn't showed its edge.)
pub const BLOCK_POINTS: usize = 64;

/// osmium's index of the water set's node places (a sparse array, 16 bytes a node): about 2.2
/// times the set's bytes (2026-09-28's planet, with its coastline then: 890 million nodes, 14 GB,
/// in 6.5 GB).
pub fn index_bytes(set_len: u64) -> u64 {
    set_len / 5 * 11
}

/// The memory the step needs with osmium's index in it: the index, and the step's own (its
/// features and a zoom's tiles, with osmium's buffers: 8.4 GB at most for the 6.5 GB set) at about
/// one and a half times the set's bytes and a GB (the sea's islands, the basemap's tiles read).
pub fn mem_bytes(set_len: u64) -> u64 {
    index_bytes(set_len) + set_len / 2 * 3 + (1 << 30)
}

/// Whether osmium's index goes in memory, with `free` bytes of it free: when `mem_bytes` fits. Else
/// on disk: slower, by how much the first worldwide run on disk will tell.
pub fn index_in_memory(set_len: u64, free: u64) -> bool {
    mem_bytes(set_len) <= free
}

/// The disk an index on disk leaves free at least, or the step fails (the agent made room for it
/// when it planned the job without the memory for it: `disk_bytes`).
pub const DISK_SPARE: u64 = 10 << 30;

/// The disk the step holds at most (past what it leaves free: room::RESERVE in the agent): the set
/// copied here, with osmium's index while it's read unless that's in memory (`index_in_memory`), and
/// a GB besides; later, the set gone, the tiles (844 MB worldwide) and a pack at a time.
pub fn disk_bytes(set_len: u64, index_in_memory: bool) -> u64 {
    set_len + if index_in_memory { 0 } else { index_bytes(set_len) } + (1 << 30)
}

/// The export's bytes per byte of the set (GeoJSON of its areas: 3.6 over five regions,
/// 2026-10-06), for the progress.
pub const EXPORT_PER_SET: f64 = 3.6;

/// One island, lake or river.
#[derive(Clone, Debug, PartialEq)]
pub struct Feat {
    pub kind: u8,
    /// Its outline's area, Web Mercator m² (a lake's outer ring, its islands not taken off), before
    /// any simplification.
    pub area: f64,
    /// Its outline's area-weighted centre, world units (Web Mercator, 0–1, y down).
    pub c: [f64; 2],
    /// A point inside it (a lake's off its islands), for asking the basemap whether it's there.
    pub inside: [f64; 2],
    /// The lake or river it's an island of (an index into the features; `NONE` for the sea's
    /// islands, lakes and rivers).
    pub parent: u32,
    /// One of the sea's islands (a hole of the water polygons).
    pub sea: bool,
    /// Bit z: the basemap lacks it at zoom z (0–13).
    pub absent: u16,
    /// Its outline, simplified for the finest zoom it may be drawn whole at (empty: never).
    pub ring: Vec<[f64; 2]>,
}

pub const NONE: u32 = u32::MAX;

/// A polygon's rings (the outer, then its holes), world units.
pub type Rings = Vec<Vec<[f64; 2]>>;
/// A point in a tile: its area (m²), kind, place (tile units) and whether it's still absent at z13.
type Point = (f64, u8, i32, i32, bool);
/// An outline in a tile: its area (m²), kind and ring (tile units).
type Outline = (f64, u8, Vec<(i32, i32)>);

/// Its area in 256-px tile pixels at zoom `z`.
pub fn px2(area_m2: f64, z: u8) -> f64 {
    let px = WORLD_M / (256.0 * f64::from(1u32 << z));
    area_m2 / (px * px)
}

/// The Planetiler whose rule this is (`min_px2`, `rule_absent`, `planetiler_dp`: read from its
/// source and sampled against its tiles). The pass stops on another one's jar
/// (pipeline::osmpass::check_planetiler) until the rule is checked against it, and the step checks
/// the rule against the basemap's tiles each time it runs (`check_rule`).
pub const PLANETILER_VERSION: &str = "0.10.2";

/// The basemap's minimum at zoom `z` (6–14), px² of its ring once simplified: the sea's islands 1,
/// the rest 4 below z12 and 1 at z12–13; 1/256 at z14. (Below z6: Natural Earth.)
pub fn min_px2(kind: u8, sea: bool, z: u8) -> f64 {
    match z {
        14.. => 1.0 / 256.0,
        12..=13 => 1.0,
        _ if sea && kind == ISLAND => 1.0,
        _ => 4.0,
    }
}

/// The zooms 6–13 the basemap lacks a ring at (bit z), by its rule: its area once simplified as
/// Planetiler simplifies at that zoom (`planetiler_dp`), worked out where the area before it is
/// within a factor of four of the minimum (simplifying a small ring took 1–9 % off its area in the
/// sample: far from it, the area before decides). `ring` closed, `area_m2` its area.
pub fn rule_absent(kind: u8, sea: bool, ring: &[[f64; 2]], area_m2: f64) -> u16 {
    let mut absent = 0u16;
    for z in 6..=13u8 {
        let min = min_px2(kind, sea, z);
        let raw = px2(area_m2, z);
        let kept = if raw >= 4.0 * min {
            true
        } else if raw < min / 4.0 {
            false
        } else {
            let simple = planetiler_dp(ring, tolerance(z));
            px2(ring_moments(&simple).0.abs() * WORLD_M * WORLD_M, z) >= min
        };
        if !kept {
            absent |= 1 << z;
        }
    }
    absent
}

/// The first zoom at which it's 1 px² or more (0–14; 15: never).
fn first_px_zoom(area_m2: f64) -> u8 {
    (0..=14u8).find(|&z| px2(area_m2, z) >= 1.0).unwrap_or(15)
}

// ---- geometry ----------------------------------------------------------------------------------

/// Longitude and latitude in world units (Web Mercator, 0–1, y down).
pub fn world(lon: f64, lat: f64) -> [f64; 2] {
    let s = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians().dsin();
    [lon / 360.0 + 0.5, (0.5 - 0.25 * ((1.0 + s) / (1.0 - s)).dln() / PI).clamp(0.0, 1.0)]
}

/// A ring's signed area (world units², y down: positive clockwise on the map) and its
/// area-weighted centre. A closing point repeating the first is fine.
pub fn ring_moments(r: &[[f64; 2]]) -> (f64, [f64; 2]) {
    if r.len() < 3 {
        return (0.0, r.first().copied().unwrap_or([0.0, 0.0]));
    }
    // About the first point, for precision.
    let o = r[0];
    let (mut a2, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        let (x0, y0, x1, y1) = (p[0] - o[0], p[1] - o[1], q[0] - o[0], q[1] - o[1]);
        let cross = x0 * y1 - x1 * y0;
        a2 += cross;
        cx += (x0 + x1) * cross;
        cy += (y0 + y1) * cross;
    }
    if a2 == 0.0 {
        let n = r.len() as f64;
        return (0.0, [r.iter().map(|p| p[0]).sum::<f64>() / n, r.iter().map(|p| p[1]).sum::<f64>() / n]);
    }
    (a2 / 2.0, [o[0] + cx / (3.0 * a2), o[1] + cy / (3.0 * a2)])
}

/// A point inside a polygon (its outer ring and holes): the middle of the widest stretch inside it
/// along the line across its outer ring's middle (as JTS's interior point does); the centre of a
/// polygon too thin to cross.
pub fn inside_point(outer: &[[f64; 2]], holes: &[&[[f64; 2]]]) -> [f64; 2] {
    let (mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in outer {
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    let mut y = (y0 + y1) / 2.0;
    // Off any vertex, so every edge crosses the line cleanly or not at all.
    let on_vertex = |y: f64| outer.iter().chain(holes.iter().flat_map(|h| h.iter())).any(|p| p[1] == y);
    let mut nudge = (y1 - y0) * 1e-7;
    for _ in 0..8 {
        if !on_vertex(y) {
            break;
        }
        y += nudge;
        nudge *= 3.0;
    }
    let mut xs: Vec<f64> = Vec::new();
    for r in std::iter::once(outer).chain(holes.iter().copied()) {
        for i in 0..r.len() {
            let (p, q) = (r[i], r[(i + 1) % r.len()]);
            if (p[1] < y) != (q[1] < y) {
                xs.push(p[0] + (y - p[1]) * (q[0] - p[0]) / (q[1] - p[1]));
            }
        }
    }
    xs.sort_by(f64::total_cmp);
    let best = xs.as_chunks::<2>().0.iter().max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])));
    match best {
        Some(s) if s[1] > s[0] => [(s[0] + s[1]) / 2.0, y],
        _ => ring_moments(outer).1,
    }
}

/// Whether point `p` is inside the rings (even–odd).
#[cfg(test)]
fn inside_rings(rings: &[Vec<[f64; 2]>], p: [f64; 2]) -> bool {
    let mut inside = false;
    for r in rings {
        for i in 0..r.len() {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
                inside = !inside;
            }
        }
    }
    inside
}

/// A tile's water polygons (each its rings), their edges filed in rows across the tile, for many
/// points to be asked whether they're in water: in some polygon, even–odd over its rings.
pub struct WaterIndex {
    y0: f64,
    row_h: f64,
    rows: Vec<Vec<(u32, [f64; 4])>>,
}

impl WaterIndex {
    const ROWS: usize = 256;

    /// `polys` (world units) for a tile spanning `y0`–`y1`.
    pub fn new(polys: &[Rings], y0: f64, y1: f64) -> WaterIndex {
        let row_h = (y1 - y0) / Self::ROWS as f64;
        let row = |y: f64| (((y - y0) / row_h).floor().max(0.0) as usize).min(Self::ROWS - 1);
        let mut rows = vec![Vec::new(); Self::ROWS];
        for (k, rings) in polys.iter().enumerate() {
            for r in rings {
                for i in 0..r.len() {
                    let (a, b) = (r[i], r[(i + 1) % r.len()]);
                    if a[1] == b[1] {
                        continue;
                    }
                    for row in &mut rows[row(a[1].min(b[1]))..=row(a[1].max(b[1]))] {
                        row.push((k as u32, [a[0], a[1], b[0], b[1]]));
                    }
                }
            }
        }
        WaterIndex { y0, row_h, rows }
    }

    /// Whether `p` (in the tile) is in water.
    pub fn contains(&self, p: [f64; 2]) -> bool {
        let row = (((p[1] - self.y0) / self.row_h).floor().max(0.0) as usize).min(Self::ROWS - 1);
        // The polygons crossed an odd number of times.
        let mut odd: Vec<u32> = Vec::new();
        for &(k, [ax, ay, bx, by]) in &self.rows[row] {
            if (ay > p[1]) != (by > p[1]) && p[0] < ax + (p[1] - ay) * (bx - ax) / (by - ay) {
                match odd.iter().position(|&o| o == k) {
                    Some(i) => {
                        odd.swap_remove(i);
                    }
                    None => odd.push(k),
                }
            }
        }
        !odd.is_empty()
    }
}

/// A closed ring (its last point its first) simplified as Planetiler 0.10.2 simplifies before its
/// area test (geo/DouglasPeuckerSimplifier): Douglas–Peucker with its first and last points kept
/// and, for a ring, two more forced (the farthest from the first, then the farthest within the
/// first half), so it keeps at least four; a ring of four points or fewer as it is. In world units
/// with the tolerance over 2^z: the same comparisons as Planetiler's in tile units (scaling by a
/// power of two is exact).
pub fn planetiler_dp(c: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    if c.len() <= 4 {
        return c.to_vec();
    }
    let sq = tol * tol.abs();
    let sq_seg = |p: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        let (mut x, mut y, dx, dy) = (a[0], a[1], b[0] - a[0], b[1] - a[1]);
        if dx != 0.0 || dy != 0.0 {
            let t = ((p[0] - x) * dx + (p[1] - y) * dy) / (dx * dx + dy * dy);
            if t > 1.0 {
                (x, y) = (b[0], b[1]);
            } else if t > 0.0 {
                x += dx * t;
                y += dy * t;
            }
        }
        (p[0] - x) * (p[0] - x) + (p[1] - y) * (p[1] - y)
    };
    // Its recursion as a stack, in its order: a segment's first half, its point, its second half.
    enum Step {
        Seg(usize, usize, i32),
        Keep(usize),
    }
    let mut out = vec![c[0]];
    let mut stack = vec![Step::Seg(0, c.len() - 1, 2)];
    while let Some(s) = stack.pop() {
        match s {
            Step::Keep(i) => out.push(c[i]),
            Step::Seg(first, last, forced) => {
                let force = forced > 0;
                let (mut max, mut index) = (if force { -1.0 } else { sq }, None);
                for (i, p) in c.iter().enumerate().take(last).skip(first + 1) {
                    let d = sq_seg(*p, c[first], c[last]);
                    if d > max {
                        (max, index) = (d, Some(i));
                    }
                }
                let Some(index) = index else { continue };
                if last - index > 1 {
                    stack.push(Step::Seg(index, last, forced - 2));
                }
                stack.push(Step::Keep(index));
                if index - first > 1 {
                    stack.push(Step::Seg(first, index, forced - 1));
                }
            }
        }
    }
    out.push(c[c.len() - 1]);
    out
}

/// Douglas–Peucker on a closed ring (world units), keeping its first point and at least four: the
/// outlines' (drawn, not measured).
pub fn simplify_ring(r: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    // (Without a closing point.)
    let r = if r.len() > 1 && r.first() == r.last() { &r[..r.len() - 1] } else { r };
    if r.len() <= 4 {
        return r.to_vec();
    }
    let mut keep = vec![false; r.len() + 1];
    keep[0] = true;
    keep[r.len()] = true;
    let at = |i: usize| r[i % r.len()];
    // A closed ring's chord from its first point to itself is a point: split at the farthest one.
    let far = (1..r.len()).max_by(|&a, &b| {
        let d = |i: usize| (r[i][0] - r[0][0]).powi(2) + (r[i][1] - r[0][1]).powi(2);
        d(a).total_cmp(&d(b))
    });
    let mut stack = match far {
        Some(f) => {
            keep[f] = true;
            vec![(0, f), (f, r.len())]
        }
        None => vec![(0, r.len())],
    };
    let tol2 = tol * tol;
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (at(a), at(b));
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len2 = dx * dx + dy * dy;
        let mut best = (0.0f64, 0usize);
        for i in a + 1..b {
            let p = at(i);
            let d2 = if len2 == 0.0 {
                (p[0] - pa[0]).powi(2) + (p[1] - pa[1]).powi(2)
            } else {
                let t = (((p[0] - pa[0]) * dx + (p[1] - pa[1]) * dy) / len2).clamp(0.0, 1.0);
                (p[0] - pa[0] - t * dx).powi(2) + (p[1] - pa[1] - t * dy).powi(2)
            };
            if d2 > best.0 {
                best = (d2, i);
            }
        }
        if best.0 > tol2 {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    let out: Vec<[f64; 2]> = (0..r.len()).filter(|&i| keep[i]).map(|i| r[i]).collect();
    if out.len() < 4 {
        r.to_vec()
    } else {
        out
    }
}

/// The tolerance (world units) the basemap simplifies with at zoom `z`.
fn tolerance(z: u8) -> f64 {
    TOLERANCE_PX / (256.0 * f64::from(1u32 << z))
}

/// A ring closed: its first point repeated at its end if it isn't.
fn closed(r: &[[f64; 2]]) -> std::borrow::Cow<'_, [[f64; 2]]> {
    if r.len() > 1 && r.first() != r.last() {
        let mut v = r.to_vec();
        v.push(r[0]);
        std::borrow::Cow::Owned(v)
    } else {
        std::borrow::Cow::Borrowed(r)
    }
}

// ---- reading -----------------------------------------------------------------------------------

/// What the basemap draws as water (OpenMapTiles' water polygons: natural=water, the reservoir,
/// basin and salt pond land uses, docks, and water=river … wastewater), and of that, what isn't
/// a lake: water along a line (a river's, a canal's), whose islands still count. Not water in a
/// tunnel or culvert (a tunnel tag but no, 0 or false, as OpenMapTiles reads it), which the map
/// hides where the basemap marks it, nor covered water (covered=yes), which OpenMapTiles leaves out.
fn water_kind(p: &serde_json::Map<String, serde_json::Value>) -> Option<u8> {
    let tag = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(tag("tunnel"), "" | "no" | "0" | "false") || tag("covered") == "yes" {
        return None;
    }
    let water = tag("water");
    let drawn = tag("natural") == "water"
        || matches!(tag("landuse"), "reservoir" | "basin" | "salt_pond")
        || tag("waterway") == "dock"
        || matches!(water, "river" | "stream" | "canal" | "ditch" | "drain" | "pond" | "basin" | "wastewater");
    if !drawn {
        return None;
    }
    Some(if matches!(water, "river" | "stream" | "canal" | "ditch" | "drain" | "rapids" | "lock" | "fish_pass") { RIVER } else { LAKE })
}

/// One exported water area: its kind (`LAKE` or `RIVER`) and its polygons (each its outer ring
/// then its holes).
type Parsed = (u8, Vec<Rings>);

fn parse_line(line: &str) -> Option<Parsed> {
    let line = line.trim_matches(|c: char| c == '\u{1e}' || c.is_whitespace());
    if line.is_empty() {
        return None;
    }
    let f: serde_json::Value = serde_json::from_str(line).ok()?;
    let p = f.get("properties")?.as_object()?;
    let g = f.get("geometry")?;
    let pt = |v: &serde_json::Value| -> Option<[f64; 2]> { Some(world(v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)) };
    let ring = |r: &serde_json::Value| -> Option<Vec<[f64; 2]>> { r.as_array()?.iter().map(pt).collect() };
    let poly = |v: &serde_json::Value| -> Option<Rings> { v.as_array()?.iter().map(ring).collect() };
    let coords = g.get("coordinates")?;
    match g.get("type")?.as_str()? {
        "Polygon" => Some((water_kind(p)?, vec![poly(coords)?])),
        "MultiPolygon" => Some((water_kind(p)?, coords.as_array()?.iter().map(poly).collect::<Option<_>>()?)),
        _ => None,
    }
}

/// A feature from a ring (`holes` for a lake's interior point), when it's big enough to count, with
/// the zooms 6–13 the basemap's rule leaves it out at.
fn feat(kind: u8, outer: &[[f64; 2]], holes: &[&[[f64; 2]]], parent: u32, sea: bool) -> Option<Feat> {
    let outer = closed(outer);
    let (a, c) = ring_moments(&outer);
    let area = a.abs() * WORLD_M * WORLD_M;
    if !(MIN_M2..).contains(&area) || !area.is_finite() {
        return None;
    }
    // (A ring across the antimeridian has no sensible centre: there are none small enough to matter.)
    let (x0, x1) = outer.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    if x1 - x0 > 0.5 {
        return None;
    }
    let absent = rule_absent(kind, sea, &outer, area);
    // Its outline, for the zooms it may be drawn whole at (from 1 px², where the basemap lacks it,
    // to z11: `OUTLINE_MAXZ`), simplified for the finest: the deepest such of z6–11, else z5 (the
    // Natural Earth zooms may lack anything). Rivers are never drawn.
    let finest = match kind {
        RIVER => None,
        _ => (6..=OUTLINE_MAXZ).rev().find(|&z| absent & (1 << z) != 0 && px2(area, z) >= 1.0).or((first_px_zoom(area) <= 5).then_some(5)),
    };
    let ring = finest.map(|z| simplify_ring(&outer, tolerance(z))).unwrap_or_default();
    Some(Feat { kind, area, c, inside: inside_point(&outer, holes), parent, sea, absent, ring })
}

/// The features of one water area: each polygon a lake or a river, its holes islands of it.
fn water_feats(kind: u8, polys: &[Rings], out: &mut Vec<Feat>) {
    for poly in polys {
        let Some((outer, holes)) = poly.split_first() else { continue };
        let hs: Vec<&[[f64; 2]]> = holes.iter().map(Vec::as_slice).collect();
        let parent = match feat(kind, outer, &hs, NONE, false) {
            Some(f) => {
                out.push(f);
                (out.len() - 1) as u32
            }
            None => NONE,
        };
        for h in holes {
            if let Some(f) = feat(ISLAND, h, &[], parent, false) {
                out.push(f);
            }
        }
    }
}

/// What reading found.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Counts {
    /// Exported features read.
    pub lines: u64,
    pub lakes: u64,
    /// Water along a line (rivers, canals …): their islands' parents, never drawn.
    pub rivers: u64,
    /// Islands in lakes and rivers.
    pub inland_islands: u64,
    /// The sea's islands: the water polygons' holes.
    pub sea_islands: u64,
    /// The water polygons read.
    pub water_polygons: u64,
}

/// Reads `osmium export`'s GeoJSON sequence of the `water` set: the lakes and rivers and their
/// islands, in the order read, each lake or river before its islands, with its index as their
/// `parent`. `said` hears the lines read so far.
pub fn read_export(r: impl BufRead, said: &dyn Fn(u64)) -> Result<(Vec<Feat>, Counts)> {
    let mut feats: Vec<Feat> = Vec::new();
    let mut st = Counts::default();
    let mut lines = r.lines();
    let mut batch: Vec<String> = Vec::with_capacity(16384);
    loop {
        batch.clear();
        for l in lines.by_ref().take(16384) {
            batch.push(l.context("read the export")?);
        }
        if batch.is_empty() {
            break;
        }
        st.lines += batch.len() as u64;
        // Each batch's features made in parallel, their parents then made global.
        let made: Vec<Vec<Feat>> = batch
            .par_iter()
            .map(|l| {
                let mut v = Vec::new();
                if let Some((kind, polys)) = parse_line(l) {
                    water_feats(kind, &polys, &mut v);
                }
                v
            })
            .collect();
        for fs in made {
            let base = feats.len() as u32;
            for mut f in fs {
                if f.parent != NONE {
                    f.parent += base;
                }
                match f.kind {
                    LAKE => st.lakes += 1,
                    RIVER => st.rivers += 1,
                    _ => st.inland_islands += 1,
                }
                feats.push(f);
            }
        }
        said(st.lines);
    }
    Ok((feats, st))
}

/// Reads the water polygons' shapefile (`WATER_POLYGONS_SHP`, EPSG:3857, as a stream: its records
/// in order) for the sea's islands: every hole of every polygon (its rings anticlockwise, y up),
/// each a feature. Returns them and how many polygons there were. (An island across the split
/// polygons' grid is a notch in two of them, no hole: the basemap merges them back into one hole,
/// kept whatever its size while it survives simplification.)
pub fn read_water_polygons(mut r: impl Read) -> Result<(Vec<Feat>, u64)> {
    let mut head = [0u8; 100];
    r.read_exact(&mut head).context("the shapefile's header")?;
    let word = |o: usize| i32::from_be_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]]);
    if word(0) != 9994 {
        bail!("not a shapefile");
    }
    // Its records to its length (the header's, in 16-bit words): none cut short.
    let mut left = (u64::try_from(word(24)).context("the shapefile's length")? * 2).checked_sub(100).context("the shapefile's length")?;
    let mut feats = Vec::new();
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
        let made: Vec<Vec<Feat>> = batch.par_iter().map(|b| shp_holes(b).iter().filter_map(|h| feat(ISLAND, h, &[], NONE, true)).collect()).collect();
        feats.extend(made.into_iter().flatten());
    }
    Ok((feats, polygons))
}

/// A shapefile polygon record's holes (its anticlockwise rings), in world units.
fn shp_holes(b: &[u8]) -> Vec<Vec<[f64; 2]>> {
    let i32_at = |o: usize| b.get(o..o + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()));
    let f64_at = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    // (Shape type 5: a polygon; 0, a null shape.)
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
    let mut holes = Vec::new();
    for k in 0..nparts {
        let (s, e) = (parts[k], if k + 1 < nparts { parts[k + 1] } else { npts });
        if e <= s + 2 || e > npts {
            continue;
        }
        let pts: Vec<[f64; 2]> = (s..e).map(|i| [f64_at(pts_at + 16 * i), f64_at(pts_at + 16 * i + 8)]).collect();
        // Anticlockwise (y up): a hole.
        let a2: f64 = (0..pts.len()).map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
            p[0] * q[1] - q[0] * p[1]
        }).sum();
        if a2 > 0.0 {
            holes.push(pts.iter().map(|p| [p[0] / WORLD_M + 0.5, 0.5 - p[1] / WORLD_M]).collect());
        }
    }
    holes
}

// ---- which zooms lack each ----------------------------------------------------------------------

/// The basemap's water at zooms 0–5, a tile's at a time: its water features' rings, world units.
pub trait Water {
    /// Tile z/x/y's water polygons (each its rings, world units), or none.
    fn water(&self, z: u8, x: u32, y: u32) -> Result<Vec<Rings>>;
}

/// A basemap archive's water layer.
pub struct Basemap<'a>(pub &'a store::pmtiles::PmTiles);

impl Water for Basemap<'_> {
    fn water(&self, z: u8, x: u32, y: u32) -> Result<Vec<Rings>> {
        let Some(raw) = self.0.get(z, x, y)? else { return Ok(Vec::new()) };
        let tile = Tile::decode(&names::mvt::gunzip_if_gzip(&raw)?)?;
        let Some(l) = tile.layers.iter().find(|l| l.name == "water") else { return Ok(Vec::new()) };
        let (n, e) = (f64::from(1u32 << z), f64::from(l.extent));
        let mut out = Vec::new();
        for f in &l.features {
            if f.geom_type != Some(3) {
                continue;
            }
            let tunnel = f.tags.as_chunks::<2>().0.iter().any(|kv| l.keys.get(kv[0] as usize).is_some_and(|k| k == "brunnel") && l.values.get(kv[1] as usize).and_then(Value::as_str) == Some("tunnel"));
            if tunnel {
                continue;
            }
            let rings = decode_rings(&f.geometry).into_iter().map(|r| r.into_iter().map(|(px, py)| [(f64::from(x) + px / e) / n, (f64::from(y) + py / e) / n]).collect()).collect();
            out.push(rings);
        }
        Ok(out)
    }
}

/// An MVT geometry's rings (tile units).
fn decode_rings(g: &[u32]) -> Vec<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let (mut x, mut y, mut i) = (0i64, 0i64, 0usize);
    let zz = |v: u32| (v >> 1) as i64 ^ -((v & 1) as i64);
    while i < g.len() {
        let (cmd, n) = (g[i] & 7, g[i] >> 3);
        i += 1;
        match cmd {
            1 | 2 => {
                for _ in 0..n {
                    if i + 1 >= g.len() {
                        break;
                    }
                    x += zz(g[i]);
                    y += zz(g[i + 1]);
                    i += 2;
                    if cmd == 1 && !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push((x as f64, y as f64));
                }
            }
            7 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => break,
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whether `f` is asked of Natural Earth at z0–5: an island whatever its size (its coarse shores
/// put many small islands on its land, where a dot would be land on land), a lake or river from
/// `NE_MIN_M2` (smaller, it has none: missing).
fn ne_asked(f: &Feat) -> bool {
    f.kind == ISLAND || f.area >= NE_MIN_M2
}

/// Sets each feature's zooms 0–5 absent by asking the basemap (Natural Earth's water there): a lake
/// or river is there when its inside point is in water, an island when it isn't. A lake or river
/// under `NE_MIN_M2` is missing at all six (`ne_asked`).
pub fn ne_absent(feats: &mut [Feat], water: &(dyn Water + Sync)) -> Result<()> {
    for z in 0..=5u8 {
        let n = f64::from(1u32 << z);
        let tile_of = |p: [f64; 2]| (((p[0] * n) as u32).min((1 << z) - 1), ((p[1] * n) as u32).min((1 << z) - 1));
        let mut tiles: Vec<(u32, u32)> = feats.iter().filter(|f| ne_asked(f)).map(|f| tile_of(f.inside)).collect();
        tiles.sort_unstable();
        tiles.dedup();
        let index: HashMap<(u32, u32), WaterIndex> = tiles
            .par_iter()
            .map(|&(x, y)| Ok(((x, y), WaterIndex::new(&water.water(z, x, y)?, f64::from(y) / n, f64::from(y + 1) / n))))
            .collect::<Result<_>>()?;
        feats.par_iter_mut().for_each(|f| {
            let missing = !ne_asked(f) || (f.kind == ISLAND) == index.get(&tile_of(f.inside)).is_some_and(|w| w.contains(f.inside));
            if missing {
                f.absent |= 1 << z;
            }
        });
    }
    Ok(())
}

/// The least share of the rule's calls the basemap's tiles may disagree with before the step
/// fails (`check_rule`): 99.5 % of lakes and 99.7 % of sea islands agreed on 2026-10-06.
pub const RULE_AGREES: f64 = 0.98;
/// The calls checked of each kind (lakes, sea islands), and the fewest that can fail the step.
pub const RULE_SAMPLE: usize = 2000;
const RULE_FEWEST: usize = 500;

/// How many of the rule's calls were checked against the basemap's tiles, and how many agreed.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Agreement {
    pub checked: usize,
    pub agreed: usize,
}

impl Agreement {
    pub fn share(&self) -> f64 {
        if self.checked == 0 {
            1.0
        } else {
            self.agreed as f64 / self.checked as f64
        }
    }

    /// Whether the rule holds: it agreed with `RULE_AGREES` of the tiles, or too few were checked
    /// to say (a small set's).
    pub fn holds(&self) -> bool {
        self.checked < RULE_FEWEST || self.share() >= RULE_AGREES
    }
}

/// Checks the rule (`rule_absent`) against the basemap's own tiles, so a basemap drawn otherwise
/// (another Planetiler, another profile) fails the step rather than leaving islands out or doubling
/// them: about `RULE_SAMPLE` lakes and as many sea islands, each at a zoom 6–11 where its area is
/// within a factor of four of the minimum (where simplifying decides), spread evenly over them in
/// the order read, each asked of the tile there at its inside point (a lake is there when it's in
/// water, an island when it isn't). Returns the lakes' agreement and the sea islands'.
pub fn check_rule(feats: &[Feat], water: &(dyn Water + Sync)) -> Result<[Agreement; 2]> {
    let near = |f: &Feat, z: u8| {
        let (raw, min) = (px2(f.area, z), min_px2(f.kind, f.sea, z));
        raw >= min / 4.0 && raw < 4.0 * min
    };
    let class = |f: &Feat| match (f.kind, f.sea) {
        (LAKE, _) => Some(0),
        (ISLAND, true) => Some(1),
        _ => None,
    };
    let mut total = [0usize; 2];
    for f in feats {
        if let Some(c) = class(f) {
            total[c] += (6..=11u8).filter(|&z| near(f, z)).count();
        }
    }
    let step = total.map(|t| t.div_ceil(RULE_SAMPLE).max(1));
    let mut seen = [0usize; 2];
    // The calls (a feature and its kind's class) by the tile asked, z/x/y.
    type Calls = Vec<(usize, usize)>;
    let mut by_tile: std::collections::BTreeMap<(u8, u32, u32), Calls> = std::collections::BTreeMap::new();
    for (i, f) in feats.iter().enumerate() {
        let Some(c) = class(f) else { continue };
        for z in (6..=11u8).filter(|&z| near(f, z)) {
            seen[c] += 1;
            if (seen[c] - 1) % step[c] != 0 {
                continue;
            }
            let n = f64::from(1u32 << z);
            let t = |v: f64| ((v * n) as u32).min((1 << z) - 1);
            by_tile.entry((z, t(f.inside[0]), t(f.inside[1]))).or_default().push((i, c));
        }
    }
    let tiles: Vec<((u8, u32, u32), Calls)> = by_tile.into_iter().collect();
    let each: Vec<[Agreement; 2]> = tiles
        .par_iter()
        .map(|&((z, x, y), ref calls)| {
            let n = f64::from(1u32 << z);
            let w = WaterIndex::new(&water.water(z, x, y)?, f64::from(y) / n, f64::from(y + 1) / n);
            let mut a = [Agreement::default(); 2];
            for &(i, c) in calls {
                let f = &feats[i];
                let there = w.contains(f.inside) == (f.kind == LAKE);
                a[c].checked += 1;
                a[c].agreed += usize::from((f.absent & (1 << z) == 0) == there);
            }
            Ok(a)
        })
        .collect::<Result<_>>()?;
    let mut sum = [Agreement::default(); 2];
    for a in each {
        for c in 0..2 {
            sum[c].checked += a[c].checked;
            sum[c].agreed += a[c].agreed;
        }
    }
    Ok(sum)
}

/// Whether the tiles draw `f` at zoom `z` (0–13): when the basemap lacks it there, or the lake it's
/// an island of (whose outline or dot would cover it); never a river, nor an island of a river the
/// basemap lacks there (it would be land on bare land: a braided river's).
pub fn drawn(feats: &[Feat], f: &Feat, z: u8) -> bool {
    let bit = 1u16 << z;
    if f.kind == RIVER {
        return false;
    }
    match feats.get(f.parent as usize) {
        Some(p) if p.absent & bit != 0 => p.kind == LAKE,
        _ => f.absent & bit != 0,
    }
}

// ---- tiles -------------------------------------------------------------------------------------

/// The log-scale area a point carries (`q`): round(8 log2 m²).
pub fn q_of(area_m2: f64) -> u32 {
    (8.0 * area_m2.max(1.0).dlog2()).round() as u32
}

/// One tile's features, ready to encode: points (cell, kind, area, centre) and outlines.
#[derive(Default)]
struct TileFeats {
    points: Vec<Point>,
    polys: Vec<Outline>,
}

fn encode_tile(t: &mut TileFeats, with_o: bool) -> Vec<u8> {
    // Bigger first (what lies inside something is smaller, and drawn over it), then by place.
    t.points.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then((a.2, a.3).cmp(&(b.2, b.3))));
    t.polys.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut layer = Layer { name: MVT_LAYER.into(), version: 2, extent: EXTENT, keys: vec!["k".into(), "q".into(), "o".into()], values: Vec::new(), features: Vec::new(), unknown: Vec::new() };
    let mut vals: HashMap<u64, u32> = HashMap::new();
    let mut val = |layer: &mut Layer, v: u64| -> u32 {
        *vals.entry(v).or_insert_with(|| {
            layer.values.push(Value::Uint(v));
            (layer.values.len() - 1) as u32
        })
    };
    let zz = |v: i32| ((v << 1) ^ (v >> 31)) as u32;
    for (_, kind, ring) in &t.polys {
        let mut g = Vec::with_capacity(ring.len() * 2 + 4);
        let (mut px, mut py) = (0i32, 0i32);
        for (i, &(x, y)) in ring.iter().enumerate() {
            if i == 0 {
                g.push(1 | (1 << 3));
            } else if i == 1 {
                g.push(2 | (((ring.len() - 1) as u32) << 3));
            }
            g.push(zz(x - px));
            g.push(zz(y - py));
            (px, py) = (x, y);
        }
        g.push(7 | (1 << 3));
        let k = val(&mut layer, u64::from(*kind));
        layer.features.push(MvtFeature { id: None, tags: vec![0, k], geom_type: Some(3), geometry: g, unknown: Vec::new() });
    }
    for &(area, kind, x, y, o) in &t.points {
        let k = val(&mut layer, u64::from(kind));
        let q = val(&mut layer, u64::from(q_of(area)));
        let mut tags = vec![0, k, 1, q];
        if with_o && o {
            let one = val(&mut layer, 1);
            tags.extend([2, one]);
        }
        layer.features.push(MvtFeature { id: None, tags, geom_type: Some(1), geometry: vec![1 | (1 << 3), zz(x), zz(y)], unknown: Vec::new() });
    }
    Tile { layers: vec![layer], unknown: Vec::new() }.encode()
}

/// Points summed in cells of 2, 4, 8, 16 px (32 … 256 tile units) until there are at most `cap`:
/// each cell's (by kind, and `o`) at its biggest point's place, so they lie where the islands and
/// lakes do, not on a lattice.
fn coarsen(points: &mut Vec<Point>, cap: usize) {
    let mut shift = 5;
    while points.len() > cap && shift <= 8 {
        let mut keyed: Vec<((i32, i32, u8, bool), Point)> = points.iter().map(|&p| ((p.2 >> shift, p.3 >> shift, p.1, p.4), p)).collect();
        keyed.sort_by_key(|k| k.0);
        let mut out: Vec<Point> = Vec::new();
        let mut i = 0;
        while i < keyed.len() {
            let key = keyed[i].0;
            let (mut a, mut big) = (0.0, keyed[i].1);
            while i < keyed.len() && keyed[i].0 == key {
                a += keyed[i].1 .0;
                if keyed[i].1 .0 > big.0 {
                    big = keyed[i].1;
                }
                i += 1;
            }
            out.push((a, key.2, big.2, big.3, key.3));
        }
        *points = out;
        shift += 1;
    }
}

/// A tile's points, each 16-px block's (256 units) summed in wider cells where it holds more than
/// `BLOCK_POINTS` (`coarsen`).
fn thin(points: &mut Vec<Point>) {
    if points.len() <= BLOCK_POINTS {
        return;
    }
    points.sort_by_key(|p| (p.3 >> 8, p.2 >> 8));
    let mut out = Vec::with_capacity(points.len());
    for block in points.chunk_by(|a, b| (a.3 >> 8, a.2 >> 8) == (b.3 >> 8, b.2 >> 8)) {
        let mut b = block.to_vec();
        coarsen(&mut b, BLOCK_POINTS);
        out.extend(b);
    }
    *points = out;
}

/// An outline in tile units (zoom `z`, tile `tx`/`ty`): simplified for `z`, the exterior clockwise
/// (MVT), points that round together merged; none if under three points are left.
fn tile_ring(ring: &[[f64; 2]], z: u8, tx: u32, ty: u32) -> Option<Vec<(i32, i32)>> {
    let r = simplify_ring(ring, tolerance(z));
    let n = f64::from(1u32 << z) * f64::from(EXTENT);
    let (ox, oy) = (f64::from(tx) * f64::from(EXTENT), f64::from(ty) * f64::from(EXTENT));
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(r.len());
    for p in &r {
        let q = ((p[0] * n - ox).round() as i32, (p[1] * n - oy).round() as i32);
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    if out.len() < 3 {
        return None;
    }
    let a2: i64 = (0..out.len()).map(|i| {
        let (p, q) = (out[i], out[(i + 1) % out.len()]);
        i64::from(p.0) * i64::from(q.1) - i64::from(q.0) * i64::from(p.1)
    }).sum();
    if a2 == 0 {
        return None;
    }
    if a2 < 0 {
        out.reverse();
    }
    Some(out)
}

/// The tiles of zooms 0–`MAXZ`, each to `emit` (z, x, y, the tile's MVT, not gzip'd), tiles in
/// key order within a zoom; `said` hears each zoom as it's done, with its points and outlines.
pub fn tiles(feats: &[Feat], emit: &mut dyn FnMut(u8, u32, u32, Vec<u8>) -> Result<()>, said: &dyn Fn(u8, usize, usize)) -> Result<()> {
    for z in 0..=MAXZ {
        let n = f64::from(1u32 << z);
        let cells = n * 256.0;
        let mut by_tile: std::collections::BTreeMap<(u32, u32), TileFeats> = std::collections::BTreeMap::new();
        // What's drawn whole: of 1 px² or more, with an outline for this zoom.
        let whole = |f: &Feat| z <= OUTLINE_MAXZ && px2(f.area, z) >= 1.0 && !f.ring.is_empty();
        // Points: the rest drawn, summed per 1-px cell and kind. (At the deepest zoom, those still
        // drawn at z13 apart: the app overzooms to it.)
        let with_o = z == MAXZ;
        let mut pts: Vec<(u64, u8, bool, u32)> = feats
            .par_iter()
            .enumerate()
            .filter(|(_, f)| drawn(feats, f, z) && !whole(f))
            .map(|(i, f)| {
                let (cx, cy) = (((f.c[0] * cells) as u64).min(cells as u64 - 1), ((f.c[1] * cells) as u64).min(cells as u64 - 1));
                ((cx << 32) | cy, tile_kind(f), with_o && drawn(feats, f, 13), i as u32)
            })
            .collect();
        pts.par_sort_unstable();
        let mut i = 0;
        while i < pts.len() {
            let (cell, kind, o) = (pts[i].0, pts[i].1, pts[i].2);
            // (At the biggest one's place: an area-weighted centre drifts to the cell's middle where
            // they crowd, and a crowd of cells drew a lattice.)
            let (mut a, mut big, mut at) = (0.0f64, -1.0f64, [0.0f64; 2]);
            while i < pts.len() && pts[i].0 == cell && pts[i].1 == kind && pts[i].2 == o {
                let f = &feats[pts[i].3 as usize];
                a += f.area;
                if f.area > big {
                    (big, at) = (f.area, f.c);
                }
                i += 1;
            }
            let [cx, cy] = at;
            let (tx, ty) = (((cell >> 32) >> 8) as u32, ((cell & 0xffff_ffff) >> 8) as u32);
            let px = ((cx * n - f64::from(tx)) * f64::from(EXTENT)).round().clamp(0.0, f64::from(EXTENT) - 1.0) as i32;
            let py = ((cy * n - f64::from(ty)) * f64::from(EXTENT)).round().clamp(0.0, f64::from(EXTENT) - 1.0) as i32;
            by_tile.entry((tx, ty)).or_default().points.push((a, kind, px, py, o));
        }
        // Where a block of a tile would hold too many, its points summed in wider cells.
        by_tile.par_iter_mut().for_each(|(_, t)| thin(&mut t.points));
        // Outlines, in every tile their box meets.
        let polys: Vec<(usize, Vec<(u32, u32)>)> = feats
            .par_iter()
            .enumerate()
            .filter(|(_, f)| drawn(feats, f, z) && whole(f))
            .map(|(i, f)| {
                let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
                for p in &f.ring {
                    (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
                }
                let t = |v: f64| ((v * n) as u32).min((1 << z) - 1);
                let ts = (t(x0)..=t(x1)).flat_map(|x| (t(y0)..=t(y1)).map(move |y| (x, y))).collect();
                (i, ts)
            })
            .collect();
        for (i, ts) in polys {
            let f = &feats[i];
            for (tx, ty) in ts {
                if let Some(r) = tile_ring(&f.ring, z, tx, ty) {
                    by_tile.entry((tx, ty)).or_default().polys.push((f.area, tile_kind(f), r));
                }
            }
        }
        let (np, no) = by_tile.values().fold((0, 0), |(p, o), t| (p + t.points.len(), o + t.polys.len()));
        let encoded: Vec<((u32, u32), Vec<u8>)> = by_tile.into_par_iter().map(|(k, mut t)| (k, encode_tile(&mut t, with_o))).collect();
        for ((x, y), b) in encoded {
            emit(z, x, y, b)?;
        }
        said(z, np, no);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square ring of `side` world units around `c`, anticlockwise on the map (y down: negative).
    fn square(c: [f64; 2], side: f64) -> Vec<[f64; 2]> {
        let h = side / 2.0;
        vec![[c[0] - h, c[1] - h], [c[0] - h, c[1] + h], [c[0] + h, c[1] + h], [c[0] + h, c[1] - h]]
    }

    /// A side (world units) whose square is `px` 256-px pixels² at zoom `z`.
    fn side_px(px: f64, z: u8) -> f64 {
        px.sqrt() / (256.0 * f64::from(1u32 << z))
    }

    /// Area (m²) `px` 256-px pixels² at zoom `z`.
    fn m2(px: f64, z: u8) -> f64 {
        px * (WORLD_M / (256.0 * f64::from(1u32 << z))).powi(2)
    }

    /// A square feature `px` pixels² at zoom `z`, as read.
    fn sq_feat(kind: u8, c: [f64; 2], px: f64, z: u8, parent: u32, sea: bool) -> Feat {
        feat(kind, &square(c, side_px(px, z)), &[], parent, sea).unwrap()
    }

    #[test]
    fn the_basemaps_rule() {
        let absent = |kind: u8, sea: bool, px: f64, z: u8| {
            let r = closed(&square([0.4, 0.3], side_px(px, z))).into_owned();
            rule_absent(kind, sea, &r, m2(px, z)) & (1 << z) != 0
        };
        // The sea's islands from 1 px² at z6–13 (a square loses nothing to simplifying).
        assert!(!absent(ISLAND, true, 1.01, 6));
        assert!(absent(ISLAND, true, 0.99, 6));
        assert!(absent(ISLAND, true, 0.99, 13));
        // Lakes, rivers and their islands from 4 px² below z12, 1 px² at z12–13.
        assert!(!absent(LAKE, false, 4.01, 11));
        assert!(absent(LAKE, false, 3.99, 11));
        assert!(absent(RIVER, false, 3.99, 7));
        assert!(absent(ISLAND, false, 3.99, 9));
        assert!(!absent(LAKE, false, 1.01, 12));
        assert!(absent(LAKE, false, 0.99, 13));
        assert_eq!(min_px2(LAKE, false, 14), 1.0 / 256.0);
        assert!((px2(m2(2.5, 7), 7) - 2.5).abs() < 1e-9);
        // Measured once simplified: a square of 0.98 px² at z6 with a bump along a side, 0.09 px
        // high, is 1.02 px² as mapped, and 0.98 once the bump (under 0.1 px) is simplified off.
        let (s, u, x0, y0) = (side_px(0.98, 6), 1.0 / (256.0 * 64.0), 0.4, 0.3);
        let bumped = vec![[x0, y0], [x0, y0 + s], [x0 + s, y0 + s], [x0 + s + 0.09 * u, y0 + 0.5 * s], [x0 + s, y0], [x0, y0]];
        let raw = ring_moments(&bumped).0.abs() * WORLD_M * WORLD_M;
        assert!(px2(raw, 6) > 1.02 && px2(raw, 6) < 1.03, "{}", px2(raw, 6));
        assert_eq!(planetiler_dp(&bumped, tolerance(6)).len(), 5);
        assert!(rule_absent(ISLAND, true, &bumped, raw) & (1 << 6) != 0);
        assert!(rule_absent(ISLAND, true, &bumped, raw) & (1 << 7) == 0);
    }

    #[test]
    fn the_index_goes_in_memory_when_it_fits() {
        let gb = 1u64 << 30;
        // The planet's set (~6 GB): its index 13.2 GB, and with the step's own, 23.2 GB.
        assert!(index_in_memory(6 * gb, 24 * gb));
        assert!(!index_in_memory(6 * gb, 20 * gb));
        assert_eq!(disk_bytes(6 * gb, true), 7 * gb);
        assert!(disk_bytes(6 * gb, false) > 20 * gb);
    }

    #[test]
    fn simplifies_as_planetiler_does() {
        // A wobbly ring and what Planetiler 0.10.2's Douglas–Peucker keeps of it at three
        // tolerances: com.onthegomap.planetiler.geo.DouglasPeuckerSimplifier's
        // transformCoordinates, in the jar pinned in sources/basemap/ (`planetiler_dp` follows it
        // line for line; these from a port of it whose areas matched the basemap's tiles).
        let r: Vec<[f64; 2]> = vec![
            [0.9789, 0.0], [0.9193, 0.2699], [0.8565, 0.5504], [0.6213, 0.717], [0.4172, 0.9135], [0.14, 0.9739], [-0.1348, 0.9373], [-0.4158, 0.9104],
            [-0.6185, 0.7138], [-0.8346, 0.5363], [-0.91, 0.2672], [-0.9509, 0.0], [-0.9508, -0.2792], [-0.8742, -0.5618], [-0.6253, -0.7216], [-0.4016, -0.8794],
            [-0.1445, -1.005], [0.15, -1.043], [0.4193, -0.918], [0.6467, -0.7464], [0.8893, -0.5715], [0.9073, -0.2664], [0.9789, 0.0],
        ];
        for (tol, kept) in [(0.05, vec![0, 2, 4, 5, 7, 9, 12, 13, 16, 17, 20, 22]), (0.2, vec![0, 4, 7, 9, 12, 17, 20, 22]), (2.0, vec![0, 7, 12, 22])] {
            let want: Vec<[f64; 2]> = kept.iter().map(|&i| r[i]).collect();
            assert_eq!(planetiler_dp(&r, tol), want, "{tol}");
        }
        // Four points or fewer: as they are.
        assert_eq!(planetiler_dp(&r[..4], 2.0), r[..4].to_vec());
    }

    #[test]
    fn moments_and_inside_points() {
        let r = square([0.5, 0.25], 0.01);
        let (a, c) = ring_moments(&r);
        assert!((a + 1e-4).abs() < 1e-12, "{a}");
        assert!((c[0] - 0.5).abs() < 1e-12 && (c[1] - 0.25).abs() < 1e-12);
        // A C (open to the east): its centre is in the gap, its inside point isn't.
        let cshape = vec![[0.0, 0.0], [3.0, 0.0], [3.0, 1.0], [1.0, 1.0], [1.0, 2.0], [3.0, 2.0], [3.0, 3.0], [0.0, 3.0]];
        let p = inside_point(&cshape, &[]);
        assert!(inside_rings(std::slice::from_ref(&cshape), p), "{p:?}");
        // A lake's point is off its island.
        let lake = square([0.5, 0.5], 0.1);
        let island = square([0.5, 0.5], 0.06);
        let p = inside_point(&lake, &[&island]);
        assert!(inside_rings(&[lake, island], p), "{p:?}");
    }

    fn line(geom: &str, props: &str) -> String {
        format!("\u{1e}{{\"type\":\"Feature\",\"geometry\":{geom},\"properties\":{props}}}\n")
    }

    /// A lon/lat square ring as GeoJSON coordinates, anticlockwise (RFC 7946's outer ring).
    fn ll(c: (f64, f64), h: f64, ccw: bool) -> String {
        let mut p = vec![(c.0 - h, c.1 - h), (c.0 + h, c.1 - h), (c.0 + h, c.1 + h), (c.0 - h, c.1 + h)];
        if !ccw {
            p.reverse();
        }
        p.push(p[0]);
        format!("[{}]", p.iter().map(|(x, y)| format!("[{x},{y}]")).collect::<Vec<_>>().join(","))
    }

    #[test]
    fn reads_lakes_rivers_and_their_islands() {
        let mut s = String::new();
        let mp = |rings: &[String]| format!("{{\"type\":\"MultiPolygon\",\"coordinates\":[[{}]]}}", rings.join(","));
        // A lake with an island; a river with one; a culvert's water and a covered reservoir
        // (nothing); a bay (nothing: the basemap doesn't draw it); a pond not in a tunnel.
        s += &line(&mp(&[ll((10.0, 50.0), 0.01, true), ll((10.0, 50.0), 0.002, false)]), "{\"natural\":\"water\"}");
        s += &line(&mp(&[ll((11.0, 50.0), 0.01, true), ll((11.0, 50.0), 0.003, false)]), "{\"natural\":\"water\",\"water\":\"river\"}");
        s += &line(&mp(&[ll((12.0, 50.0), 0.01, true)]), "{\"natural\":\"water\",\"tunnel\":\"culvert\"}");
        s += &line(&mp(&[ll((12.5, 50.0), 0.01, true)]), "{\"landuse\":\"reservoir\",\"covered\":\"yes\"}");
        s += &line(&mp(&[ll((13.0, 50.0), 0.01, true)]), "{\"natural\":\"bay\"}");
        s += &line(&mp(&[ll((14.0, 50.0), 0.01, true)]), "{\"water\":\"pond\",\"tunnel\":\"no\"}");
        let (fs, st) = read_export(std::io::Cursor::new(s), &|_| {}).unwrap();
        assert_eq!((st.lines, st.lakes, st.rivers, st.inland_islands), (6, 2, 1, 2));
        assert_eq!(fs.len(), 5);
        assert_eq!((fs[0].kind, fs[0].parent, fs[0].sea), (LAKE, NONE, false));
        assert_eq!((fs[1].kind, fs[1].parent), (ISLAND, 0));
        assert_eq!((fs[2].kind, fs[2].parent, fs[2].ring.len()), (RIVER, NONE, 0));
        assert_eq!((fs[3].kind, fs[3].parent, fs[3].sea), (ISLAND, 2, false));
        assert_eq!(fs[4].kind, LAKE);
        // Areas in Web Mercator m²: a 0.02° square at 50° is 2.2 km a side there, 3.5 on the map.
        let w = WORLD_M * 0.02 / 360.0;
        assert!((fs[0].area / (w * w / 50f64.to_radians().cos())) > 0.9);
        let c = world(10.0, 50.0);
        assert!((fs[0].c[0] - c[0]).abs() < 1e-9 && (fs[0].c[1] - c[1]).abs() < 1e-6);
        assert!(!inside_rings(&[square(fs[1].c, 1e-9)], fs[0].inside));
    }

    /// A shapefile (EPSG:3857) of polygon records, each its rings: the header, then the records.
    fn shapefile(records: &[Vec<Vec<[f64; 2]>>]) -> Vec<u8> {
        let mut body = Vec::new();
        for (k, rings) in records.iter().enumerate() {
            let mut rec = Vec::new();
            if rings.is_empty() {
                rec.extend(0i32.to_le_bytes());
            } else {
                let n: usize = rings.iter().map(Vec::len).sum();
                rec.extend(5i32.to_le_bytes());
                rec.extend([0u8; 32]);
                rec.extend((rings.len() as i32).to_le_bytes());
                rec.extend((n as i32).to_le_bytes());
                let mut at = 0;
                for r in rings {
                    rec.extend((at as i32).to_le_bytes());
                    at += r.len();
                }
                for p in rings.iter().flatten() {
                    rec.extend(p[0].to_le_bytes());
                    rec.extend(p[1].to_le_bytes());
                }
            }
            body.extend((k as i32 + 1).to_be_bytes());
            body.extend(((rec.len() / 2) as i32).to_be_bytes());
            body.extend(rec);
        }
        let mut head = vec![0u8; 100];
        head[0..4].copy_from_slice(&9994i32.to_be_bytes());
        head[24..28].copy_from_slice((((100 + body.len()) / 2) as i32).to_be_bytes().as_slice());
        head.extend(body);
        head
    }

    #[test]
    fn the_seas_islands_are_the_water_polygons_holes() {
        // A cell of sea 100 km square (clockwise, y up) with two islands (anticlockwise): 1 km and
        // 10 m square; a null record; a cell without any.
        let sq = |c: (f64, f64), h: f64, cw: bool| {
            let mut r = vec![[c.0 - h, c.1 - h], [c.0 + h, c.1 - h], [c.0 + h, c.1 + h], [c.0 - h, c.1 + h]];
            if cw {
                r.reverse();
            }
            r.push(r[0]);
            r
        };
        let shp = shapefile(&[
            vec![sq((1e6, 5e6), 5e4, true), sq((1e6, 5e6), 500.0, false), sq((1.02e6, 5.01e6), 5.0, false)],
            vec![],
            vec![sq((2e6, 5e6), 5e4, true)],
        ]);
        let (fs, n) = read_water_polygons(std::io::Cursor::new(shp)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(fs.len(), 2);
        assert!(fs.iter().all(|f| f.kind == ISLAND && f.sea && f.parent == NONE));
        assert!((fs[0].area - 1e6).abs() < 1.0, "{}", fs[0].area);
        assert!((fs[0].c[0] - (1e6 / WORLD_M + 0.5)).abs() < 1e-12 && (fs[0].c[1] - (0.5 - 5e6 / WORLD_M)).abs() < 1e-12);
        // 1 km² is 0.67 px² at z7 (1.22 km a side), 2.7 at z8: absent at z6–7, there from z8.
        assert_eq!(fs[0].absent & 0x3fc0, 0b11 << 6);
        assert!((fs[1].area - 100.0).abs() < 1e-3);
        assert!(read_water_polygons(std::io::Cursor::new(vec![0u8; 100])).is_err());
        // Cut short: an error, not fewer islands.
        let shp = shapefile(&[vec![sq((1e6, 5e6), 5e4, true), sq((1e6, 5e6), 500.0, false)]]);
        assert!(read_water_polygons(std::io::Cursor::new(&shp[..shp.len() - 8])).is_err());
    }

    /// Water: tiles at zoom z all hold one square of sea around (0.5, 0.5).
    struct Sea;
    impl Water for Sea {
        fn water(&self, _z: u8, _x: u32, _y: u32) -> Result<Vec<Rings>> {
            Ok(vec![vec![square([0.5, 0.5], 0.2)]])
        }
    }

    fn feat_at(kind: u8, c: [f64; 2], area: f64, parent: u32, sea: bool) -> Feat {
        Feat { kind, area, c, inside: c, parent, sea, absent: 0, ring: Vec::new() }
    }

    #[test]
    fn natural_earths_zooms_are_asked() {
        let big = 5e8;
        let mut fs = vec![
            feat_at(LAKE, [0.5, 0.5], big, NONE, false),       // in the water: there
            feat_at(ISLAND, [0.5, 0.5], big / 4.0, 0, false),  // its island, in water: missing
            feat_at(LAKE, [0.2, 0.2], big, NONE, false),       // on land: missing
            feat_at(ISLAND, [0.2, 0.2], big / 4.0, NONE, true), // on land: there
            feat_at(ISLAND, [0.5, 0.5], 1e5, NONE, true),      // small: missing
            feat_at(RIVER, [0.5, 0.5], big, NONE, false),      // in the water: there
        ];
        // Small: a lake missing unasked, though in the water; an island asked, there on land.
        fs.push(feat_at(LAKE, [0.5, 0.5], 1e5, NONE, false));
        fs.push(feat_at(ISLAND, [0.2, 0.2], 1e5, NONE, true));
        ne_absent(&mut fs, &Sea).unwrap();
        let low: Vec<u16> = fs.iter().map(|f| f.absent & 0x3f).collect();
        assert_eq!(low, [0, 0x3f, 0x3f, 0, 0x3f, 0, 0x3f, 0]);
    }

    #[test]
    fn the_water_index_answers_as_the_rings_do() {
        // Two polygons, one with a hole, overlapping a third: in water where any one holds it.
        let a = vec![square([0.3, 0.3], 0.3), square([0.3, 0.3], 0.1)];
        let b = vec![vec![[0.5, 0.1], [0.9, 0.2], [0.7, 0.9], [0.45, 0.6]]];
        let c = vec![square([0.35, 0.35], 0.1)];
        let polys = vec![a, b, c];
        let w = WaterIndex::new(&polys, 0.0, 1.0);
        let mut s = 12345u64;
        let mut rnd = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..20_000 {
            let p = [rnd(), rnd()];
            assert_eq!(w.contains(p), polys.iter().any(|rs| inside_rings(rs, p)), "{p:?}");
        }
    }

    /// Water: the same polygons at every tile.
    struct Lakes(Vec<Rings>);
    impl Water for Lakes {
        fn water(&self, _z: u8, _x: u32, _y: u32) -> Result<Vec<Rings>> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn the_rule_is_checked_against_the_tiles() {
        // 3,000 lakes 2 px² at z9 (absent there by the rule), half of them in the tiles' water.
        let mut fs: Vec<Feat> = (0..3000).map(|i| sq_feat(LAKE, [0.2 + 0.0001 * f64::from(i), 0.6], 2.0, 9, NONE, false)).collect();
        let water = Lakes(vec![vec![vec![[0.0, 0.0], [0.35, 0.0], [0.35, 1.0], [0.0, 1.0]]]]);
        let [lakes, islands] = check_rule(&fs, &water).unwrap();
        // Each is within 4× of the minimum at z9 and z10 (2 and 8 px²): a call each, 2,000 of 6,000
        // checked, half agreeing.
        assert_eq!((lakes.checked, islands.checked), (2000, 0));
        assert!((lakes.share() - 0.5).abs() < 0.02, "{lakes:?}");
        assert!(!lakes.holds() && islands.holds());
        // In the tiles' water where the rule keeps them, out where it doesn't: all agree.
        for f in fs.iter_mut() {
            f.absent = if f.inside[0] < 0.35 { 0 } else { 0x3fff };
        }
        let [lakes, _] = check_rule(&fs, &water).unwrap();
        assert_eq!(lakes.agreed, lakes.checked);
        assert!(lakes.holds());
    }

    #[test]
    fn a_crowded_tiles_points_are_summed_wider() {
        // 64 × 64 points a px apart (16 tile units): over a cap of 1,000, summed per 2 px, then
        // 4 px (256), their areas kept.
        let mut pts: Vec<Point> = (0..64).flat_map(|i| (0..64).map(move |j| (1.0, LAKE, i * 16 + 8, j * 16 + 8, false))).collect();
        coarsen(&mut pts, 1000);
        assert_eq!(pts.len(), 256);
        assert!(pts.iter().all(|p| p.0 == 16.0));
        // At a point of the cell's (all alike: the first), not its middle.
        assert_eq!((pts[0].2, pts[0].3), (8, 8));
        let mut two = vec![(1.0, LAKE, 8, 8, false), (5.0, LAKE, 24, 40, false), (1.0, LAKE, 72, 8, false)];
        coarsen(&mut two, 2);
        assert_eq!(two, vec![(6.0, LAKE, 24, 40, false), (1.0, LAKE, 72, 8, false)]);
        let mut few = vec![(1.0, ISLAND, 5, 5, false), (2.0, LAKE, 6, 6, false)];
        coarsen(&mut few, 1000);
        assert_eq!(few.len(), 2);
    }

    #[test]
    fn crowded_blocks_are_summed_alone() {
        // A block of 16 × 16 points a px apart, and a block beside it with three: the crowded one
        // summed per 2 px (64), the other as it was.
        let mut pts: Vec<Point> = (0..16).flat_map(|i| (0..16).map(move |j| (1.0, LAKE, i * 16 + 8, j * 16 + 8, false))).collect();
        pts.extend([(1.0, LAKE, 300, 8, false), (1.0, LAKE, 330, 40, false), (1.0, LAKE, 400, 200, false)]);
        thin(&mut pts);
        assert_eq!(pts.len(), 64 + 3);
        assert!(pts.iter().filter(|p| p.2 < 256).all(|p| p.0 == 4.0));
        assert_eq!(pts.iter().filter(|p| p.2 >= 256).count(), 3);
    }

    /// A zoom's features, decoded: each its geometry type and properties.
    type Decoded = Vec<(u32, Vec<(String, u64)>)>;

    /// The tiles of `fs`, decoded, a zoom at a time.
    fn tiled(fs: &[Feat]) -> impl Fn(u8) -> Decoded {
        let mut got: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
        tiles(fs, &mut |z, x, y, b| {
            got.push((z, x, y, b));
            Ok(())
        }, &|_, _, _| {})
        .unwrap();
        move |z: u8| {
            got.iter()
                .filter(|t| t.0 == z)
                .flat_map(|t| {
                    let l = Tile::decode(&t.3).unwrap().layers.remove(0);
                    l.features
                        .iter()
                        .map(|f| {
                            let props = f.tags.chunks(2).map(|kv| (l.keys[kv[0] as usize].clone(), match l.values[kv[1] as usize] { Value::Uint(v) => v, _ => 99 })).collect();
                            (f.geom_type.unwrap(), props)
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        }
    }

    #[test]
    fn tiles_hold_what_the_basemap_lacks() {
        // Two tiny islands in one cell at z0 are one point there, with both areas; a lake 2 px² at
        // z9 is an outline at z9, a point at z8, and nothing from z10 (the basemap has it).
        let c = [0.3, 0.3];
        let mut fs = vec![
            sq_feat(LAKE, [0.6, 0.6], 2.0, 9, NONE, false),
            feat(ISLAND, &square(c, 10.0 / WORLD_M), &[], NONE, true).unwrap(),
            feat(ISLAND, &square([c[0] + 1e-9, c[1]], 300f64.sqrt() / WORLD_M), &[], NONE, true).unwrap(),
            feat(ISLAND, &square([0.6, 0.6], 50f64.sqrt() / WORLD_M), &[], 0, false).unwrap(),
        ];
        for f in fs.iter_mut() {
            f.absent |= 0x3f;
        }
        let at = tiled(&fs);
        let z0 = at(0);
        // The lake, the sea's two islands as one, the lake's island apart (k 2).
        assert_eq!(z0.len(), 3, "{z0:?}");
        assert!(z0.contains(&(1, vec![("k".into(), 2), ("q".into(), u64::from(q_of(50.0)))])), "{z0:?}");
        assert!(z0.contains(&(1, vec![("k".into(), 0), ("q".into(), u64::from(q_of(400.0)))])), "{z0:?}");
        assert_eq!(at(8).iter().filter(|f| f.0 == 1 && f.1[0].1 == 1).count(), 1);
        assert_eq!(at(9).iter().filter(|f| f.0 == 3).count(), 1);
        assert_eq!(at(10).iter().filter(|f| f.1[0].1 == 1).count(), 0);
        // The islands, under 1 px² at z12 and z13 alike, are still absent when overzoomed.
        let z12 = at(12);
        assert!(!z12.is_empty() && z12.iter().all(|f| f.1.contains(&("o".into(), 1))), "{z12:?}");
    }

    #[test]
    fn a_rivers_islands_go_with_it() {
        // A river 10 px² at z9 (absent to z8) with an island 0.5 px² at z9 (absent to z10): the
        // island is drawn only where the river is there, z9 (a point) and z10 (its outline, 2 px²).
        // A lake's island is drawn with its lake, though the basemap has the island.
        let mut fs = vec![sq_feat(RIVER, [0.6, 0.6], 10.0, 9, NONE, false), sq_feat(ISLAND, [0.6, 0.6], 0.5, 9, 0, false)];
        fs.push(sq_feat(LAKE, [0.2, 0.2], 3.0, 9, NONE, false));
        let mut held = sq_feat(ISLAND, [0.2, 0.2], 5.0, 10, 2, false);
        held.absent = 0;
        fs.push(held);
        for f in fs.iter_mut() {
            f.absent |= 0x3f;
        }
        assert_eq!(fs[0].absent & 0x3fc0, 0b111 << 6);
        assert_eq!(fs[1].absent & 0x3fc0, 0b11111 << 6);
        let at = tiled(&fs);
        let kinds = |z: u8| -> Vec<(u32, u64)> {
            let mut v: Vec<(u32, u64)> = at(z).into_iter().map(|f| (f.0, f.1[0].1)).collect();
            v.sort();
            v
        };
        // z8: the lake (0.75 px²: a point) and its island; no river, nor its island. z9: the
        // river's island (a point), the lake and its island (outlines: 3 and 1.25 px²).
        assert_eq!(kinds(8), vec![(1, 1), (1, 2)]);
        assert_eq!(kinds(9), vec![(1, 2), (3, 1), (3, 2)]);
        assert_eq!(kinds(10), vec![(3, 2)]);
        assert_eq!(kinds(11), vec![]);
    }
}
