//! The catalogue's files an area needs (docs/macs.md, "Mirror, per Mac"): what a download of a
//! region or a view copies (the map server's downloads), and what a region's download weighs (the
//! build page's regions, crate::agent::regionsview). Areas are rings of [lon, lat] degrees, each
//! read as a filled area.

use roadcore::geo;
use std::collections::{BTreeMap, BTreeSet};
use store::catalogue::Catalogue;

/// How far around an area its tiles are kept: its outlines are simplified (60 m to 1 km), and the
/// build reaches 1 km past coasts.
pub const NEAR_KM: f64 = 2.0;
/// How far around an area its hi data are kept: the lists of a view (scenic drives and rides, rail
/// lines) read the tiles within half their window of it, 50 km at most.
pub const LISTS_KM: f64 = 50.0;
/// How far around an area the layers the viewshed reads are kept (its z11 terrain, canopy heights
/// and land cover): its radius goes to 25 km (viewshed.rs), so one from inside works offline.
pub const VIEWSHED_KM: f64 = 25.0;
pub const VIEWSHED_LAYERS: [&str; 3] = ["terrain", "grid-canopy", "grid-class"];

/// Some of the catalogue's files: (content name, size), sorted by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub names: Vec<(String, u64)>,
    pub bytes: u64,
}

impl Files {
    pub fn from_map(m: BTreeMap<String, u64>) -> Files {
        let bytes = m.values().sum();
        Files { names: m.into_iter().collect(), bytes }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.binary_search_by(|(n, _)| n.as_str().cmp(name)).is_ok()
    }
}

/// The polar blocks (roadcore::polar::BLOCK_Z tiles of `pole`'s grid) an area may meet: those meeting
/// the box of its rings' points in that hemisphere, projected onto the grid and grown by `km` (the
/// stereographic grid is conformal: a ring round the pole, as Antarctica's, boxes the pole in). A
/// superset where an area isn't box-like; never misses one it meets.
pub fn polar_blocks_meeting(rings: &[Vec<[f64; 2]>], pole: roadcore::polar::Pole, km: f64) -> Vec<(u32, u32)> {
    let north = pole == roadcore::polar::Pole::North;
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in rings.iter().flatten().filter(|p| (p[1] > 0.0) == north) {
        let (x, y) = roadcore::polar::forward(pole, p[0], p[1]);
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    if b[0] > b[2] {
        return Vec::new();
    }
    let g = km * 1000.0;
    roadcore::polar::blocks(pole)
        .into_iter()
        .filter(|&(x, y)| {
            let t = roadcore::polar::tile_box(roadcore::polar::BLOCK_Z, x, y);
            t[0] <= b[2] + g && t[2] >= b[0] - g && t[1] <= b[3] + g && t[3] >= b[1] - g
        })
        .collect()
}

/// A region's outer rings, as the catalogue records them (simplified; holes left out: a tile in a
/// hole is kept all the same).
pub fn outer_rings(r: &buildstore::coverage::DrawnRegion) -> Vec<Vec<[f64; 2]>> {
    r.shapes.values().flat_map(|polys| polys.iter().filter_map(|p| p.first().cloned())).collect()
}

/// The files an area (`rings`, each read as a filled area) needs of `cat` for the map to work over it
/// at every zoom without the NAS (its basemap aside: the map server's downloads add its pieces).
pub fn files_of(cat: &Catalogue, rings: &[Vec<[f64; 2]>]) -> Files {
    let mut out = BTreeMap::new();
    let mut add = |logical: &String| {
        if let Some(f) = cat.files.get(logical) {
            out.insert(f.file.clone(), f.size);
        }
    };
    let near = tiles_meeting(rings, NEAR_KM);
    for &(x, y) in &near {
        let key = format!("6/{x}/{y}");
        for l in cat.layers.values() {
            if let Some(h) = l.hi.get(&key) {
                add(h);
            }
        }
        for m in [&cat.base, &cat.roads] {
            if let Some(l) = m.get(&key) {
                add(l);
            }
        }
    }
    // Zoomed out over it: every layer's root (zooms 0–2) and its lo packs (3–8) of the z3 tiles
    // over its tiles; what a terrain tile without its own is made from, and the place search's
    // places that show by zoom 8. (A z3 tile reaches past the area: zoomed out, so does the map.)
    let z3: BTreeSet<(u32, u32)> = near.iter().map(|&(x, y)| (x >> 3, y >> 3)).collect();
    for l in cat.layers.values() {
        if let Some(r) = &l.root {
            add(r);
        }
        for &(x, y) in &z3 {
            if let Some(p) = l.lo.get(&format!("3/{x}/{y}")) {
                add(p);
            }
        }
    }
    // (The polar caps' blocks the area meets, its pole's layers: `polar_blocks_meeting`.)
    for (name, l) in cat.layers.iter().filter(|(_, l)| !l.polar.is_empty()) {
        let Some(pole) = name.rsplit('-').next().and_then(roadcore::polar::Pole::of_letter) else { continue };
        for (x, y) in polar_blocks_meeting(rings, pole, NEAR_KM) {
            if let Some(b) = l.polar.get(&format!("{}/{x}/{y}", roadcore::polar::BLOCK_Z)) {
                add(b);
            }
        }
    }
    for (x, y) in tiles_meeting(rings, VIEWSHED_KM) {
        let key = format!("6/{x}/{y}");
        for l in VIEWSHED_LAYERS.iter().filter_map(|l| cat.layers.get(*l)) {
            if let Some(h) = l.hi.get(&key) {
                add(h);
            }
        }
    }
    let lists = tiles_meeting(rings, LISTS_KM);
    for &(x, y) in &lists {
        let key = format!("6/{x}/{y}");
        for m in [&cat.hidata, &cat.markdata] {
            if let Some(l) = m.get(&key) {
                add(l);
            }
        }
        if let Some(l) = cat.ovdata.get(&format!("3/{}/{}", x >> 3, y >> 3)) {
            add(l);
        }
    }
    // What the map reads from the World's part too (store::mirror's essentials), so a region works
    // away from the NAS without it: the worldwide files, those of a z6 tile
    // (`…/6-x-y.<hash>…`) only for its tiles; the Canadian lake fill only when it meets Canada.
    let named: BTreeSet<String> = near.iter().map(|(x, y)| format!("6-{x}-{y}")).collect();
    let canada = rings.iter().flatten().any(|p| crate::lakefill::in_reach(p[0], p[1]));
    for l in cat.global.values() {
        if l.starts_with(crate::lakefill::PREFIX) {
            if canada {
                add(l);
            }
            continue;
        }
        if !l.starts_with("global/") {
            continue;
        }
        let last = l.rsplit('/').next().unwrap_or("");
        let tile = last.split('.').next().unwrap_or("");
        let per_tile = tile.starts_with("6-") && tile.split('-').count() == 3;
        if !per_tile || named.contains(tile) {
            add(l);
        }
    }
    Files::from_map(out)
}

/// The terrain's deep packs (its detail, z13 and finer) an area (`rings`, each read as a filled
/// area) needs, with their shading's (the slope layer's deep packs of the same z10 tiles:
/// crate::slope_pack): those whose z10 tile meets it grown by 2 km; and the share of the area the detail
/// covers (per cent, in Web Mercator: the deep packs' z10 tiles clipped to it, over its area).
pub fn detail_of(cat: &Catalogue, rings: &[Vec<[f64; 2]>]) -> (Files, f64) {
    let Some(l) = cat.layers.get("terrain").filter(|l| !l.deep.is_empty()) else { return (Files::default(), 0.0) };
    let rings: Vec<&Vec<[f64; 2]>> =
        rings.iter().filter(|r| r.len() >= 3 && r.iter().all(|p| p[0].is_finite() && p[1].is_finite())).collect();
    // (Tiles of z10 per tile of z6, each way.) The rings in z10 tiles.
    let k = f64::from(1u32 << (crate::dem::zoom::PACK_Z - 6));
    let flat: Vec<Vec<[f64; 2]>> =
        rings.iter().map(|r| r.iter().map(|p| [lon_x(p[0]) * k, lat_y(p[1]) * k]).collect()).collect();
    let area: f64 = flat.iter().map(|r| ring_area(r)).sum();
    let boxes: Vec<[f64; 4]> = rings
        .iter()
        .map(|r| {
            r.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
            })
        })
        .collect();
    let (mut out, mut covered) = (BTreeMap::new(), 0.0);
    for (key, logical) in &l.deep {
        let mut it = key.split('/').skip(1).map(str::parse::<u32>);
        let (Some(Ok(x)), Some(Ok(y))) = (it.next(), it.next()) else { continue };
        let (x0, y0, x1, y1) = (f64::from(x) / k, f64::from(y) / k, f64::from(x + 1) / k, f64::from(y + 1) / k);
        let g = geo::grow([x_lon(x0), y_lat(y1), x_lon(x1), y_lat(y0)], NEAR_KM);
        let mut meets = false;
        // (Rings may run past 180°: the tile there too.)
        for shift in [0.0, 360.0] {
            let grown = [g[0] + shift, g[1], g[2] + shift, g[3]];
            for ((r, b), f) in rings.iter().zip(&boxes).zip(&flat) {
                if grown[0] > b[2] || grown[2] < b[0] || grown[1] > b[3] || grown[3] < b[1] {
                    continue;
                }
                meets = meets || ring_meets_box(r, grown);
                let x0 = f64::from(x) + shift / 360.0 * N6 * k;
                covered += ring_area(&clip(f, [x0, f64::from(y), x0 + 1.0, f64::from(y + 1)]));
            }
        }
        if meets {
            let shading = cat.layers.get("slope").and_then(|s| s.deep.get(key));
            for f in std::iter::once(logical).chain(shading).filter_map(|l| cat.files.get(l)) {
                out.insert(f.file.clone(), f.size);
            }
        }
    }
    let cover = if area > 0.0 { (covered / area * 100.0).min(100.0) } else { 0.0 };
    (Files::from_map(out), cover)
}

/// A ring's area (shoelace; either winding).
pub fn ring_area(r: &[[f64; 2]]) -> f64 {
    let n = r.len();
    ((0..n).map(|i| r[i][0] * r[(i + 1) % n][1] - r[(i + 1) % n][0] * r[i][1]).sum::<f64>() / 2.0).abs()
}

/// A ring clipped to a box [x0, y0, x1, y1] (Sutherland–Hodgman: its area is that of the ring
/// within the box).
pub fn clip(ring: &[[f64; 2]], b: [f64; 4]) -> Vec<[f64; 2]> {
    let mut out = ring.to_vec();
    // Each edge of the box: (axis, bound, whether inside is below the bound).
    for (axis, bound, below) in [(0, b[0], false), (0, b[2], true), (1, b[1], false), (1, b[3], true)] {
        let ins = |p: &[f64; 2]| if below { p[axis] <= bound } else { p[axis] >= bound };
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (p, q) = (input[i], input[(i + 1) % input.len()]);
            if ins(&p) {
                out.push(p);
            }
            if ins(&p) != ins(&q) {
                let t = (bound - p[axis]) / (q[axis] - p[axis]);
                out.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
            }
        }
    }
    out
}

/// z6 tiles each way.
pub const N6: f64 = 64.0;

/// A longitude, latitude, as z6 tiles from the west and north (roadcore::geo's Mercator, the
/// latitude within the tiles), and back.
pub fn lon_x(lon: f64) -> f64 {
    geo::merc_x(lon) * N6
}

pub fn lat_y(lat: f64) -> f64 {
    geo::merc_clamped(0.0, lat).1 * N6
}

pub fn x_lon(x: f64) -> f64 {
    geo::merc_lon(x / N6)
}

pub fn y_lat(y: f64) -> f64 {
    geo::merc_lat(y / N6)
}

/// Tile column `x` (past 63 east of 180°) and row `y`'s box grown by `km` (roadcore::geo::grow).
fn tile_grown(x: f64, y: f64, km: f64) -> [f64; 4] {
    geo::grow([x_lon(x), y_lat(y + 1.0), x_lon(x + 1.0), y_lat(y)], km)
}

/// The z6 tiles (x, y) that `rings` (lon, lat degrees; each read as a filled area; longitudes
/// may run past ±180) meet once grown by `km`: each tile's box grown by `km` tested against each
/// ring (a vertex inside it, its centre inside the ring, or an edge across it).
pub fn tiles_meeting(rings: &[Vec<[f64; 2]>], km: f64) -> BTreeSet<(u32, u32)> {
    let mut out = BTreeSet::new();
    for ring in rings.iter().filter(|r| r.len() >= 3 && r.iter().all(|p| p[0].is_finite() && p[1].is_finite())) {
        let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in ring {
            (w, e, s, n) = (w.min(p[0]), e.max(p[0]), s.min(p[1]), n.max(p[1]));
        }
        let g = geo::grow([w, s, e, n], km);
        let (x0, x1) = (lon_x(g[0]).floor() as i64, lon_x(g[2]).floor() as i64);
        let (y0, y1) = (lat_y(g[3]).floor().max(0.0) as i64, lat_y(g[1]).floor().min(N6 - 1.0) as i64);
        for x in x0..=x1.min(x0 + 2 * N6 as i64) {
            for y in y0..=y1 {
                let t = (x.rem_euclid(N6 as i64) as u32, y as u32);
                if out.contains(&t) {
                    continue;
                }
                if ring_meets_box(ring, tile_grown(x as f64, y as f64, km)) {
                    out.insert(t);
                }
            }
        }
    }
    out
}

/// Whether a box ([w, s, e, n], degrees) grown by `km` meets an area (`rings`, each read as a filled
/// area; longitudes may run past ±180, so the box is tried a turn east too).
pub fn box_meets(rings: &[Vec<[f64; 2]>], b: [f64; 4], km: f64) -> bool {
    let g0 = geo::grow(b, km);
    [0.0, 360.0].iter().any(|shift| {
        let g = [g0[0] + shift, g0[1], g0[2] + shift, g0[3]];
        rings.iter().filter(|r| r.len() >= 3).any(|r| ring_meets_box(r, g))
    })
}

/// Whether a ring (read as a filled area) and a box [w, s, e, n] meet.
fn ring_meets_box(ring: &[[f64; 2]], b: [f64; 4]) -> bool {
    if ring.iter().any(|p| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]) {
        return true;
    }
    if geo::inside(&[ring], [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]) {
        return true;
    }
    let c = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]];
    (0..ring.len()).any(|i| (0..4).any(|k| cross(ring[i], ring[(i + 1) % ring.len()], c[k], c[(k + 1) % 4])))
}

/// Whether segments ab and cd cross (properly).
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let o = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (o1, o2, o3, o4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0) && o1 != 0.0 && o2 != 0.0 && o3 != 0.0 && o4 != 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use roadcore::polar::Pole;

    /// A ring round the pole (Antarctica's coast) meets every block; a small area by the pole (the
    /// station) the four at the pole; one in the other hemisphere none.
    #[test]
    fn polar_blocks_an_area_meets() {
        let coast: Vec<[f64; 2]> = (0..=72).map(|k| [-180.0 + 5.0 * k as f64, -70.0]).collect();
        assert_eq!(
            polar_blocks_meeting(&[coast], Pole::South, NEAR_KM).len(),
            roadcore::polar::blocks(Pole::South).len()
        );
        let station = vec![[-10.0, -89.99], [10.0, -89.99], [10.0, -89.98], [-10.0, -89.98]];
        let mut near = polar_blocks_meeting(&[station], Pole::South, NEAR_KM);
        near.sort();
        assert_eq!(near, vec![(7, 7), (7, 8), (8, 7), (8, 8)]);
        let north = vec![[0.0, 60.0], [1.0, 60.0], [1.0, 61.0]];
        assert!(polar_blocks_meeting(&[north], Pole::South, NEAR_KM).is_empty());
    }
}
