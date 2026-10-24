//! The catalogue's files an area needs (docs/macs.md, "Mirror, per Mac"): what a download of a
//! region or a view copies (the map server's downloads), and what a region's download weighs (the
//! build page's regions, crate::agent::regionsview). Areas are rings of [lon, lat] degrees, each
//! read as a filled area.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;
use store::catalog::Catalog;

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

/// Some of the catalog's files: (content name, size), sorted by name.
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

/// A region's outer rings, as the catalog records them (simplified; holes left out: a tile in a
/// hole is kept all the same).
pub fn outer_rings(r: &crate::coverage::DrawnRegion) -> Vec<Vec<[f64; 2]>> {
    r.shapes.values().flat_map(|polys| polys.iter().filter_map(|p| p.first().cloned())).collect()
}

/// The files an area (`rings`, each read as a filled area) needs of `cat`, the essentials and the
/// basemap aside (module doc).
pub fn files_of(cat: &Catalog, rings: &[Vec<[f64; 2]>]) -> Files {
    let mut out = BTreeMap::new();
    let mut add = |logical: &String| {
        if let Some(f) = cat.files.get(logical) {
            out.insert(f.file.clone(), f.size);
        }
    };
    for (x, y) in tiles_meeting(rings, NEAR_KM) {
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
    // What the map reads zoomed in from the World's part too (store::mirror's essentials), so a
    // region works away from the NAS without it: the worldwide files, those of a z6 tile
    // (`…/6-x-y.<hash>…`) only for its tiles; the Canadian lake fill only when it meets Canada.
    let near: BTreeSet<String> = tiles_meeting(rings, NEAR_KM).iter().map(|(x, y)| format!("6-{x}-{y}")).collect();
    let canada = rings.iter().flatten().any(|p| (-141.5..=-52.0).contains(&p[0]) && (41.5..=84.0).contains(&p[1]));
    for l in cat.global.values() {
        if l.starts_with("work/lakefill/") {
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
        if !per_tile || near.contains(tile) {
            add(l);
        }
    }
    Files::from_map(out)
}

/// The terrain's deep packs (its detail, z13 and finer) an area (`rings`, each read as a filled
/// area) needs, with their shading's (the slope layer's deep packs of the same z10 tiles:
/// crate::slope_pack): those whose z10 tile meets it grown by 2 km; and the share of the area the detail
/// covers (per cent, in Web Mercator: the deep packs' z10 tiles clipped to it, over its area).
pub fn detail_of(cat: &Catalog, rings: &[Vec<[f64; 2]>]) -> (Files, f64) {
    let Some(l) = cat.layers.get("terrain").filter(|l| !l.deep.is_empty()) else { return (Files::default(), 0.0) };
    let rings: Vec<&Vec<[f64; 2]>> =
        rings.iter().filter(|r| r.len() >= 3 && r.iter().all(|p| p[0].is_finite() && p[1].is_finite())).collect();
    // (Tiles of z10 per tile of z6, each way.) The rings in z10 tiles.
    let k = f64::from(1u32 << (crate::terrain_deep::PACK_Z - 6));
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
    let dlat = NEAR_KM / 111.32;
    let (mut out, mut covered) = (BTreeMap::new(), 0.0);
    for (key, logical) in &l.deep {
        let mut it = key.split('/').skip(1).map(str::parse::<u32>);
        let (Some(Ok(x)), Some(Ok(y))) = (it.next(), it.next()) else { continue };
        let (bn, bs) = (y_lat(f64::from(y) / k), y_lat(f64::from(y + 1) / k));
        let g = km_lon(NEAR_KM, bs.abs().max(bn.abs()));
        let mut meets = false;
        // (Rings may run past 180°: the tile there too.)
        for shift in [0.0, 360.0] {
            let (w, e) = (x_lon(f64::from(x) / k) + shift, x_lon(f64::from(x + 1) / k) + shift);
            let grown = [w - g, bs - dlat, e + g, bn + dlat];
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

pub fn lon_x(lon: f64) -> f64 {
    (lon + 180.0) / 360.0 * N6
}

pub fn lat_y(lat: f64) -> f64 {
    let r = lat.clamp(-85.0511, 85.0511).to_radians();
    (1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0 * N6
}

pub fn x_lon(x: f64) -> f64 {
    x / N6 * 360.0 - 180.0
}

pub fn y_lat(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y / N6)).sinh().atan().to_degrees()
}

/// Degrees of longitude a distance spans at a latitude.
fn km_lon(km: f64, lat: f64) -> f64 {
    km / (111.32 * lat.abs().min(89.0).to_radians().cos().max(0.01))
}

/// The z6 tiles (x, y) that `rings` (lon, lat degrees; each read as a filled area; longitudes
/// may run past ±180) meet once grown by `km`: each tile's box grown by `km` tested against each
/// ring (a vertex inside it, its centre inside the ring, or an edge across it).
pub fn tiles_meeting(rings: &[Vec<[f64; 2]>], km: f64) -> BTreeSet<(u32, u32)> {
    let mut out = BTreeSet::new();
    let dlat = km / 111.32;
    for ring in rings.iter().filter(|r| r.len() >= 3 && r.iter().all(|p| p[0].is_finite() && p[1].is_finite())) {
        let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in ring {
            (w, e, s, n) = (w.min(p[0]), e.max(p[0]), s.min(p[1]), n.max(p[1]));
        }
        let dl = km_lon(km, s.abs().max(n.abs()) + dlat);
        let (x0, x1) = (lon_x(w - dl).floor() as i64, lon_x(e + dl).floor() as i64);
        let (y0, y1) = (lat_y(n + dlat).floor().max(0.0) as i64, lat_y(s - dlat).floor().min(N6 - 1.0) as i64);
        for x in x0..=x1.min(x0 + 2 * N6 as i64) {
            for y in y0..=y1 {
                let t = (x.rem_euclid(N6 as i64) as u32, y as u32);
                if out.contains(&t) {
                    continue;
                }
                let (bn, bs) = (y_lat(y as f64), y_lat(y as f64 + 1.0));
                let g = km_lon(km, bs.abs().max(bn.abs()));
                if ring_meets_box(ring, [x_lon(x as f64) - g, bs - dlat, x_lon(x as f64 + 1.0) + g, bn + dlat]) {
                    out.insert(t);
                }
            }
        }
    }
    out
}

/// Whether a ring (read as a filled area) and a box [w, s, e, n] meet.
fn ring_meets_box(ring: &[[f64; 2]], b: [f64; 4]) -> bool {
    if ring.iter().any(|p| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]) {
        return true;
    }
    if inside(ring, [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]) {
        return true;
    }
    let c = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]];
    (0..ring.len()).any(|i| (0..4).any(|k| cross(ring[i], ring[(i + 1) % ring.len()], c[k], c[(k + 1) % 4])))
}

/// Point in ring, even–odd.
fn inside(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether segments ab and cd cross (properly).
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let o = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (o1, o2, o3, o4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0) && o1 != 0.0 && o2 != 0.0 && o3 != 0.0 && o4 != 0.0
}
