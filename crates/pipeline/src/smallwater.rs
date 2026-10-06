//! Small islands and lakes, worldwide (docs/plan.md §6 "Small islands and lakes"; docs/formats.md
//! "Small islands and lakes"): what the basemap leaves out zoomed out, kept on the map as the roads
//! are, faked where it's too small to draw.
//!
//! The basemap (Planetiler's OpenMapTiles profile, docs/plan.md §6 step 6) leaves a polygon out at a
//! zoom where its outline's area, measured in 256-px tile pixels, is under a minimum: an outer ring
//! and each hole alike, before any clipping or simplification. The sea's islands (the water
//! polygons' holes) go under 1 px² (z6–13); lakes and the other inland water, and their islands,
//! under 4 px² below z12 and 1 px² at z12–13; at z14, under 1/256 px². Below z6 the basemap has
//! Natural Earth's water alone. Measured on its tiles (2026-10-06, six places): 93–99 % of what the
//! rule keeps is there, against 0–3 % of what it drops.
//!
//! This step reads every island and lake from the pass's `water` set (the basemap's water areas,
//! their holes as islands, and the coastline's closed rings as the sea's islands), works out the
//! zooms the basemap lacks each at (z6–13 by the same rule, z0–5 by asking the basemap's own tiles
//! whether its water is there), and tiles them, zooms 0–12, one MVT layer `w`: at each zoom, what
//! the basemap lacks there,
//! - under 1 px²: a point, the app's dot, sized by its true area and faded with it (web/src/
//!   basemap.ts); the points of one 1-px cell and kind are one, with their summed area, at the
//!   biggest's place, in cells twice or four times as wide where a 16-px block would hold more
//!   than `BLOCK_POINTS`;
//! - from 1 px² (lakes and their islands under 4 px² at z6–11; anything Natural Earth lacks below
//!   z6): its outline, simplified as the basemap's are, drawn as the basemap would draw it.
//!
//! Properties: `k` (0 an island of the sea, 1 a lake, 2 an island of a lake or river), on points
//! `q` (the area, Web Mercator m², as round(8 log2)),
//! and in z12 tiles `o` (1: still absent at z13, where the app overzooms z12). In a tile, bigger
//! first, so what lies inside something (a pond on an island in a lake) is drawn over it.

use anyhow::{Context, Result};
use det::Det;
use names::mvt::{Feature as MvtFeature, Layer, Tile, Value};
use rayon::prelude::*;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::io::BufRead;

/// The layer (`layers/smallwater/…`, served at `/tiles/smallwater`).
pub const LAYER: &str = "smallwater";
/// What the basemap draws as water, as osmium's area tags (the export's `area_tags`): OpenMapTiles'
/// water polygons (natural=water, the reservoir, basin and salt pond land uses, docks, and
/// water=river … wastewater; bays aren't drawn). `water_kind` tells them apart.
pub const AREA_TAGS: &[&str] = &[
    "natural=water", "landuse=reservoir", "landuse=basin", "landuse=salt_pond", "waterway=dock", "water=river", "water=stream", "water=canal",
    "water=ditch", "water=drain", "water=pond", "water=basin", "water=wastewater",
];
/// The pass's `water` set's filter (pipeline::osmpass::SETS): those areas, and the coastline.
pub const SET_FILTER: &[&str] = &[
    "wr/natural=water", "wr/landuse=reservoir,basin,salt_pond", "wr/waterway=dock", "wr/water=river,stream,canal,ditch,drain,pond,basin,wastewater", "w/natural=coastline",
];
/// The kinds: an island (of the sea, or one of a lake or river: `Feat::sea` says which), a lake.
pub const ISLAND: u8 = 0;
pub const LAKE: u8 = 1;
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
/// The tiles' one layer.
pub const MVT_LAYER: &str = "w";
const EXTENT: u32 = 4096;
/// The Web Mercator world's width, metres (2πR).
pub const WORLD_M: f64 = 40_075_016.685_578_49;
/// What the basemap's Natural Earth zooms (0–5) may have: smaller is taken as missing there.
const NE_MIN_M2: f64 = 1e6;
/// Bigger islands than this (Web Mercator m², 4 px² at z0) are continents' and large islands'
/// coasts, which every zoom has.
const MAX_ISLAND_M2: f64 = 4.0 * (WORLD_M / 256.0) * (WORLD_M / 256.0);
/// Smaller than this (m²) is a mapping slip, not an island or a pond.
const MIN_M2: f64 = 1.0;
/// Outlines are simplified as the basemap's (Planetiler's default tolerance, 256-px tile pixels).
const TOLERANCE_PX: f64 = 0.1;
/// The most points a block of 16 × 16 px holds (a tile has 256): where more of its cells of 1 px
/// have some (a lake district zoomed out), they're summed in cells twice as wide, then four times,
/// until it holds no more. So a tile holds at most 16,384, and the app, which draws a circle a
/// point, ~200,000 in a view of a dozen, within an iPad's means. (By block, not by tile: a crowded
/// tile summed whole beside one that wasn't showed its edge.)
pub const BLOCK_POINTS: usize = 64;

/// One island or lake.
#[derive(Clone, Debug, PartialEq)]
pub struct Feat {
    pub kind: u8,
    /// Its outline's area, Web Mercator m² (a lake's outer ring, its islands not taken off, as the
    /// basemap's rule measures it).
    pub area: f64,
    /// Its outline's area-weighted centre, world units (Web Mercator, 0–1, y down).
    pub c: [f64; 2],
    /// A point inside it (a lake's off its islands), for asking the basemap whether it's there.
    pub inside: [f64; 2],
    /// The lake it's an island of (an index into the features; `NONE` for the sea's islands, those
    /// of rivers, and lakes).
    pub parent: u32,
    /// One of the sea's islands (from the coastline).
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

/// Whether the basemap's rule draws it at zoom `z` (6–14): the sea's islands from 1 px², the rest
/// from 4 px² below z12, 1 px² at z12–13 and 1/256 px² at z14. (Below z6: Natural Earth.)
pub fn rule_keeps(kind: u8, sea: bool, area_m2: f64, z: u8) -> bool {
    let a = px2(area_m2, z);
    match z {
        14.. => a >= 1.0 / 256.0,
        12..=13 => a >= 1.0,
        _ if sea && kind == ISLAND => a >= 1.0,
        _ => a >= 4.0,
    }
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

/// Douglas–Peucker on a closed ring (world units), keeping its first point and at least four.
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

// ---- reading the set ---------------------------------------------------------------------------

/// What the basemap draws as water (OpenMapTiles' water polygons: natural=water, the reservoir,
/// basin and salt pond land uses, docks, and water=river … wastewater), and of that, what isn't
/// a lake: water along a line (a river's, a canal's), whose islands still count.
fn water_kind(p: &serde_json::Map<String, serde_json::Value>) -> Option<bool> {
    let tag = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(tag("tunnel"), "" | "no") || tag("covered") == "yes" {
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
    Some(!matches!(water, "river" | "stream" | "canal" | "ditch" | "drain" | "rapids" | "lock" | "fish_pass"))
}

/// One exported feature: a coastline way (its points), or a water area's polygons (each its outer
/// ring then its holes) and whether it's a lake.
enum Parsed {
    Coast(Vec<[f64; 2]>),
    Water(bool, Vec<Vec<Vec<[f64; 2]>>>),
}

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
    let poly = |v: &serde_json::Value| -> Option<Vec<Vec<[f64; 2]>>> { v.as_array()?.iter().map(ring).collect() };
    let coords = g.get("coordinates")?;
    match g.get("type")?.as_str()? {
        "LineString" if p.get("natural").and_then(|v| v.as_str()) == Some("coastline") => Some(Parsed::Coast(ring(coords)?)),
        "Polygon" => Some(Parsed::Water(water_kind(p)?, vec![poly(coords)?])),
        "MultiPolygon" => Some(Parsed::Water(water_kind(p)?, coords.as_array()?.iter().map(poly).collect::<Option<_>>()?)),
        _ => None,
    }
}

/// A feature from a ring (`holes` for a lake's interior point), when it's big enough to count.
fn feat(kind: u8, outer: &[[f64; 2]], holes: &[&[[f64; 2]]], parent: u32, sea: bool) -> Option<Feat> {
    let (a, c) = ring_moments(outer);
    let area = a.abs() * WORLD_M * WORLD_M;
    if !(MIN_M2..).contains(&area) || !area.is_finite() {
        return None;
    }
    // (A ring across the antimeridian has no sensible centre: there are none small enough to matter.)
    let (x0, x1) = outer.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    if x1 - x0 > 0.5 {
        return None;
    }
    // Its outline, for the zooms it may be drawn whole at: those from 1 px² where the basemap lacks
    // it (lakes and their islands below z12, anything below z6), simplified for the finest.
    let z1 = first_px_zoom(area);
    let finest = if sea { (z1 <= 5).then_some(5) } else { (z1 <= 11).then_some(z1.clamp(5, 11)) };
    let ring = finest.map(|z| simplify_ring(outer, tolerance(z))).unwrap_or_default();
    Some(Feat { kind, area, c, inside: inside_point(outer, holes), parent, sea, absent: 0, ring })
}

/// The features of one water area: each polygon a lake (when it's one), its holes islands.
fn water_feats(lake: bool, polys: &[Vec<Vec<[f64; 2]>>], out: &mut Vec<Feat>) {
    for poly in polys {
        let Some((outer, holes)) = poly.split_first() else { continue };
        let hs: Vec<&[[f64; 2]]> = holes.iter().map(Vec::as_slice).collect();
        let parent = if lake {
            match feat(LAKE, outer, &hs, NONE, false) {
                Some(f) => {
                    out.push(f);
                    (out.len() - 1) as u32
                }
                None => NONE,
            }
        } else {
            NONE
        };
        for h in holes {
            if let Some(f) = feat(ISLAND, h, &[], parent, false) {
                out.push(f);
            }
        }
    }
}

/// What reading the set found.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Read {
    /// Exported features read.
    pub lines: u64,
    pub lakes: u64,
    /// Islands in lakes and rivers.
    pub inland_islands: u64,
    /// Coastline ways, and the islands their closed rings make.
    pub coast_ways: u64,
    pub sea_islands: u64,
    /// Coastline ways left over in chains that don't close (broken coastlines, or a continent's).
    pub open_chain_ways: u64,
}

/// Reads `osmium export`'s GeoJSON sequence of the `water` set: the lakes and their islands (in
/// the order read, each lake before its islands, with its index as their `parent`), then the
/// sea's islands, from the coastline's ways joined into rings. `said` hears the lines read so far.
pub fn read_export(r: impl BufRead, said: &dyn Fn(u64)) -> Result<(Vec<Feat>, Read)> {
    let mut feats: Vec<Feat> = Vec::new();
    let mut coast: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut st = Read::default();
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
        let parsed: Vec<Option<Parsed>> = batch.par_iter().map(|l| parse_line(l)).collect();
        // Each batch's lakes and islands made in parallel, their parents then made global.
        let made: Vec<Vec<Feat>> = parsed
            .par_iter()
            .map(|p| match p {
                Some(Parsed::Water(lake, polys)) => {
                    let mut v = Vec::new();
                    water_feats(*lake, polys, &mut v);
                    v
                }
                _ => Vec::new(),
            })
            .collect();
        for (p, fs) in parsed.into_iter().zip(made) {
            if let Some(Parsed::Coast(pts)) = p {
                coast.push(pts);
                continue;
            }
            let base = feats.len() as u32;
            for mut f in fs {
                if f.parent != NONE {
                    f.parent += base;
                }
                if f.kind == LAKE {
                    st.lakes += 1;
                } else {
                    st.inland_islands += 1;
                }
                feats.push(f);
            }
        }
        said(st.lines);
    }
    st.coast_ways = coast.len() as u64;
    let (rings, open) = join_coast(coast);
    st.open_chain_ways = open as u64;
    let islands: Vec<Feat> = rings
        .par_iter()
        .filter_map(|r| {
            // Land on the left of the coastline: an island goes round anticlockwise on the map
            // (negative here, y down). The other way round is water held in by the coastline.
            let (a, _) = ring_moments(r);
            if a >= 0.0 || -a * WORLD_M * WORLD_M > MAX_ISLAND_M2 {
                return None;
            }
            feat(ISLAND, r, &[], NONE, true)
        })
        .collect();
    st.sea_islands = islands.len() as u64;
    feats.extend(islands);
    Ok((feats, st))
}

/// The coastline's ways joined end to end into closed rings (each closed way is one), in the order
/// read; and how many ways were left in chains that don't close.
pub fn join_coast(ways: Vec<Vec<[f64; 2]>>) -> (Vec<Vec<[f64; 2]>>, usize) {
    let key = |p: [f64; 2]| (p[0].to_bits(), p[1].to_bits());
    let mut by_start: HashMap<(u64, u64), usize> = HashMap::new();
    for (i, w) in ways.iter().enumerate() {
        if w.len() >= 2 && w.first() != w.last() {
            by_start.entry(key(w[0])).or_insert(i);
        }
    }
    let mut used = vec![false; ways.len()];
    let mut rings = Vec::new();
    let mut open = 0;
    for i in 0..ways.len() {
        if used[i] || ways[i].len() < 2 {
            continue;
        }
        used[i] = true;
        if ways[i].first() == ways[i].last() {
            rings.push(ways[i].clone());
            continue;
        }
        let start = ways[i][0];
        let mut ring = ways[i].clone();
        let mut members = 1;
        let closed = loop {
            let end = *ring.last().unwrap();
            if end == start {
                break true;
            }
            match by_start.get(&key(end)) {
                Some(&j) if !used[j] => {
                    used[j] = true;
                    members += 1;
                    ring.extend_from_slice(&ways[j][1..]);
                }
                _ => break false,
            }
        };
        if closed {
            rings.push(ring);
        } else {
            open += members;
        }
    }
    (rings, open)
}

// ---- which zooms lack each ----------------------------------------------------------------------

/// Sets each feature's zooms 6–13 absent by the basemap's rule.
pub fn rule_absent(feats: &mut [Feat]) {
    feats.par_iter_mut().for_each(|f| {
        for z in 6..=13u8 {
            if !rule_keeps(f.kind, f.sea, f.area, z) {
                f.absent |= 1 << z;
            }
        }
    });
}

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

/// Sets each feature's zooms 0–5 absent by asking the basemap (Natural Earth's water there): a lake
/// is there when its inside point is in water, an island when it isn't (and an island of a lake the
/// basemap lacks is missing with it). Features under `NE_MIN_M2` are missing at all six.
pub fn ne_absent(feats: &mut [Feat], water: &(dyn Water + Sync)) -> Result<()> {
    for z in 0..=5u8 {
        let n = f64::from(1u32 << z);
        let tile_of = |p: [f64; 2]| (((p[0] * n) as u32).min((1 << z) - 1), ((p[1] * n) as u32).min((1 << z) - 1));
        let mut tiles: Vec<(u32, u32)> = feats.iter().filter(|f| f.area >= NE_MIN_M2).map(|f| tile_of(f.inside)).collect();
        tiles.sort_unstable();
        tiles.dedup();
        let decoded: HashMap<(u32, u32), Vec<Rings>> = tiles.par_iter().map(|&(x, y)| Ok(((x, y), water.water(z, x, y)?))).collect::<Result<_>>()?;
        let in_water: Vec<bool> = feats
            .par_iter()
            .map(|f| f.area >= NE_MIN_M2 && decoded.get(&tile_of(f.inside)).is_some_and(|ws| ws.iter().any(|rings| inside_rings(rings, f.inside))))
            .collect();
        // Lakes come before their islands, so a lake's bit is set before its islands read it.
        for i in 0..feats.len() {
            let f = &feats[i];
            let missing = if f.area < NE_MIN_M2 {
                true
            } else if f.kind == LAKE {
                !in_water[i]
            } else {
                in_water[i] || (f.parent != NONE && feats[f.parent as usize].absent & (1 << z) != 0)
            };
            if missing {
                feats[i].absent |= 1 << z;
            }
        }
    }
    Ok(())
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
        // Points: the features under 1 px² the basemap lacks, summed per 1-px cell and kind.
        // (At the deepest zoom, those still absent at z13 apart: the app overzooms to it.)
        let with_o = z == MAXZ;
        let mut pts: Vec<(u64, u8, bool, u32)> = feats
            .par_iter()
            .enumerate()
            .filter(|(_, f)| f.absent & (1 << z) != 0 && px2(f.area, z) < 1.0)
            .map(|(i, f)| {
                let (cx, cy) = (((f.c[0] * cells) as u64).min(cells as u64 - 1), ((f.c[1] * cells) as u64).min(cells as u64 - 1));
                ((cx << 32) | cy, tile_kind(f), with_o && f.absent & (1 << 13) != 0, i as u32)
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
        // Outlines: those of 1 px² or more the basemap lacks, in every tile their box meets.
        let polys: Vec<(usize, Vec<(u32, u32)>)> = feats
            .par_iter()
            .enumerate()
            .filter(|(_, f)| f.absent & (1 << z) != 0 && px2(f.area, z) >= 1.0 && !f.ring.is_empty())
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

    #[test]
    fn the_basemaps_rule() {
        let a = |px: f64, z: u8| px * (WORLD_M / (256.0 * f64::from(1u32 << z))).powi(2);
        // The sea's islands from 1 px² at z6–13.
        assert!(rule_keeps(ISLAND, true, a(1.0, 6), 6));
        assert!(!rule_keeps(ISLAND, true, a(0.99, 6), 6));
        assert!(!rule_keeps(ISLAND, true, a(0.99, 13), 13));
        // Lakes and their islands from 4 px² below z12, 1 px² at z12–13.
        assert!(rule_keeps(LAKE, false, a(4.0, 11), 11));
        assert!(!rule_keeps(LAKE, false, a(3.9, 11), 11));
        assert!(!rule_keeps(ISLAND, false, a(3.9, 9), 9));
        assert!(rule_keeps(LAKE, false, a(1.0, 12), 12));
        assert!(!rule_keeps(LAKE, false, a(0.9, 13), 13));
        // z14: from 1/256 px².
        assert!(rule_keeps(LAKE, false, a(0.004, 14), 14));
        assert!((px2(a(2.5, 7), 7) - 2.5).abs() < 1e-9);
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

    #[test]
    fn coastline_ways_join_into_rings() {
        let a = [0.1, 0.1];
        let b = [0.1, 0.2];
        let c = [0.2, 0.2];
        let d = [0.2, 0.1];
        let ways = vec![
            vec![c, d, a],                          // the ring's second half…
            vec![[0.5, 0.5], [0.5, 0.6], [0.6, 0.6], [0.5, 0.5]], // a closed way
            vec![a, b, c],                          // …and its first
            vec![[0.8, 0.8], [0.8, 0.9]],           // a chain that never closes
        ];
        let (rings, open) = join_coast(ways);
        assert_eq!(rings.len(), 2);
        assert_eq!(rings[0], vec![c, d, a, b, c]);
        assert_eq!(open, 1);
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
    fn reads_lakes_their_islands_and_the_seas() {
        let mut s = String::new();
        // A lake with an island; a river with one (the river no lake); a tunnel's water (nothing).
        s += &line(&format!("{{\"type\":\"MultiPolygon\",\"coordinates\":[[{},{}]]}}", ll((10.0, 50.0), 0.01, true), ll((10.0, 50.0), 0.002, false)), "{\"natural\":\"water\"}");
        s += &line(&format!("{{\"type\":\"MultiPolygon\",\"coordinates\":[[{},{}]]}}", ll((11.0, 50.0), 0.01, true), ll((11.0, 50.0), 0.003, false)), "{\"natural\":\"water\",\"water\":\"river\"}");
        s += &line(&format!("{{\"type\":\"MultiPolygon\",\"coordinates\":[[{}]]}}", ll((12.0, 50.0), 0.01, true)), "{\"natural\":\"water\",\"tunnel\":\"culvert\"}");
        // A bay isn't drawn: nothing.
        s += &line(&format!("{{\"type\":\"MultiPolygon\",\"coordinates\":[[{}]]}}", ll((13.0, 50.0), 0.01, true)), "{\"natural\":\"bay\"}");
        // The coastline: an island (land on its left: anticlockwise), and water held in (clockwise).
        s += &line(&format!("{{\"type\":\"LineString\",\"coordinates\":{}}}", ll((-5.0, 56.0), 0.005, true)), "{\"natural\":\"coastline\"}");
        s += &line(&format!("{{\"type\":\"LineString\",\"coordinates\":{}}}", ll((-6.0, 56.0), 0.005, false)), "{\"natural\":\"coastline\"}");
        let (fs, st) = read_export(std::io::Cursor::new(s), &|_| {}).unwrap();
        assert_eq!((st.lines, st.lakes, st.inland_islands, st.coast_ways, st.sea_islands), (6, 1, 2, 2, 1));
        assert_eq!(fs.len(), 4);
        assert_eq!((fs[0].kind, fs[0].parent, fs[0].sea), (LAKE, NONE, false));
        assert_eq!((fs[1].kind, fs[1].parent), (ISLAND, 0));
        assert_eq!((fs[2].kind, fs[2].parent), (ISLAND, NONE));
        assert_eq!((fs[3].kind, fs[3].sea), (ISLAND, true));
        // Areas in Web Mercator m²: a 0.02° square at 50° is 2.2 km a side there, 3.5 on the map.
        let w = WORLD_M * 0.02 / 360.0;
        assert!((fs[0].area / (w * w * (1.0 / 50f64.to_radians().cos()))).abs() > 0.9);
        let c = world(10.0, 50.0);
        assert!((fs[0].c[0] - c[0]).abs() < 1e-9 && (fs[0].c[1] - c[1]).abs() < 1e-6);
        assert!(!inside_rings(&[square(fs[1].c, 1e-9)], fs[0].inside));
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
            feat_at(ISLAND, [0.2, 0.2], big / 4.0, 2, false),  // on land, but its lake's missing
            feat_at(ISLAND, [0.2, 0.2], big / 4.0, NONE, true), // on land: there
            feat_at(ISLAND, [0.5, 0.5], 1e5, NONE, true),      // small: missing
        ];
        ne_absent(&mut fs, &Sea).unwrap();
        let low: Vec<u16> = fs.iter().map(|f| f.absent & 0x3f).collect();
        assert_eq!(low, [0, 0x3f, 0x3f, 0x3f, 0, 0x3f]);
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

    #[test]
    fn tiles_hold_what_the_basemap_lacks() {
        // Two tiny islands in one cell at z0 are one point there, with both areas; a lake 2 px² at
        // z9 is an outline at z9, a point at z8, and nothing from z10 (the basemap has it).
        let z9 = |px: f64| px * (WORLD_M / (256.0 * 512.0)).powi(2);
        let c = [0.3, 0.3];
        let mut lake = feat_at(LAKE, [0.6, 0.6], z9(2.0), NONE, false);
        lake.ring = square([0.6, 0.6], side_px(2.0, 9));
        let mut fs = vec![lake, feat_at(ISLAND, c, 100.0, NONE, true), feat_at(ISLAND, [c[0] + 1e-9, c[1]], 300.0, NONE, true), feat_at(ISLAND, c, 50.0, 0, false)];
        rule_absent(&mut fs);
        for f in fs.iter_mut() {
            f.absent |= 0x3f;
        }
        let mut got: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
        tiles(&fs, &mut |z, x, y, b| {
            got.push((z, x, y, b));
            Ok(())
        }, &|_, _, _| {})
        .unwrap();
        let feats_at = |z: u8| -> Vec<(u32, Vec<(String, u64)>)> {
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
        };
        let z0 = feats_at(0);
        // The lake, the sea's two islands as one, the lake's island apart (k 2).
        assert_eq!(z0.len(), 3, "{z0:?}");
        assert!(z0.contains(&(1, vec![("k".into(), 2), ("q".into(), u64::from(q_of(50.0)))])), "{z0:?}");
        assert!(z0.contains(&(1, vec![("k".into(), 0), ("q".into(), u64::from(q_of(400.0)))])), "{z0:?}");
        assert_eq!(feats_at(8).iter().filter(|f| f.0 == 1 && f.1[0].1 == 1).count(), 1);
        assert_eq!(feats_at(9).iter().filter(|f| f.0 == 3).count(), 1);
        assert_eq!(feats_at(10).iter().filter(|f| f.1[0].1 == 1).count(), 0);
        // The islands, under 1 px² at z12 and z13 alike, are still absent when overzoomed.
        let z12 = feats_at(12);
        assert!(!z12.is_empty() && z12.iter().all(|f| f.1.contains(&("o".into(), 1))), "{z12:?}");
    }
}
