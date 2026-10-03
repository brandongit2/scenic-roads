//! How interesting each landmark is (dem/interest.py, ported value for value for the `marks` job):
//! interest isolation, the zoom its names show from, and Python's rounding.
//!
//!   ia  the distance (km) to the nearest place of the same kind that scores higher (ties: the
//!       earlier one); within 60 km on the plane (each point's x scaled by its own latitude),
//!       farther by great circle. The best of each kind has 20000.
//!   mz  the zoom at which that distance spans one pixel on 512 px tiles.
//!
//! Every floating-point operation is in the script's order, so results are bit for bit numpy's.

use std::collections::HashMap;

/// Isolation within this on the plane, farther by great circle.
const NEAR_KM: f64 = 60.0;
/// The best of its kind.
pub const IA_BEST: f64 = 20000.0;

/// Python's `round(x, n)`: half-even on the exact binary value (Rust's formatting rounds the same).
pub fn py_round(x: f64, n: usize) -> f64 {
    format!("{x:.n$}").parse().unwrap_or(x)
}

/// The zoom at which `ia_km` spans one pixel: 78.27 km a pixel at zoom 0 on 512 px tiles (rounded
/// to 2 decimals as the script does).
pub fn min_zoom(lat: f64, ia_km: f64) -> f64 {
    py_round((78.2715 * lat.to_radians().cos() / ia_km.max(0.01)).log2(), 2)
}

/// A uniform grid of points by cell, for nearest-neighbour searches in growing rings.
struct Grid<const D: usize> {
    cell: f64,
    cells: HashMap<[i64; D], Vec<u32>>,
}

impl<const D: usize> Grid<D> {
    fn new(cell: f64) -> Self {
        Grid { cell, cells: HashMap::new() }
    }

    fn key(&self, p: &[f64; D]) -> [i64; D] {
        let mut k = [0i64; D];
        for d in 0..D {
            k[d] = (p[d] / self.cell).floor() as i64;
        }
        k
    }

    fn insert(&mut self, p: &[f64; D], i: u32) {
        self.cells.entry(self.key(p)).or_default().push(i);
    }

    /// The nearest point (by `dist`) within `max` of `p`, searching rings of cells outward until no
    /// nearer one can be farther out.
    fn nearest(&self, p: &[f64; D], max: f64, dist: &dyn Fn(u32) -> f64) -> Option<f64> {
        let c = self.key(p);
        let mut best: Option<f64> = None;
        let rings = (max / self.cell).ceil() as i64 + 1;
        for r in 0..=rings {
            // Every point within (r − 1) cells of the point's cell is at least (r − 1) × cell away
            // (in every coordinate), so a ring can stop the search once that exceeds the best.
            if let Some(b) = best {
                if (r - 1) as f64 * self.cell > b {
                    break;
                }
            }
            if (r - 1) as f64 * self.cell > max {
                break;
            }
            ring::<D>(c, r, &mut |k| {
                if let Some(v) = self.cells.get(&k) {
                    for &j in v {
                        let d = dist(j);
                        if d <= max && best.is_none_or(|b| d < b) {
                            best = Some(d);
                        }
                    }
                }
            });
        }
        best
    }
}

/// The cells at Chebyshev distance exactly `r` from `c`.
fn ring<const D: usize>(c: [i64; D], r: i64, f: &mut dyn FnMut([i64; D])) {
    let mut k = [0i64; D];
    fn rec<const D: usize>(c: &[i64; D], r: i64, d: usize, edge: bool, k: &mut [i64; D], f: &mut dyn FnMut([i64; D])) {
        if d == D {
            if edge || r == 0 {
                f(*k);
            }
            return;
        }
        for o in -r..=r {
            k[d] = c[d] + o;
            rec::<D>(c, r, d + 1, edge || o.abs() == r, k, f);
        }
    }
    rec::<D>(&c, r, 0, false, &mut k, f);
}

/// Distance (km) from each point to the nearest point with a higher score (ties: the earlier index);
/// `IA_BEST` for the best (dem/interest.py `isolation`).
pub fn isolation(lon: &[f64], lat: &[f64], score: &[f64]) -> Vec<f64> {
    let n = lon.len();
    let mut out = vec![IA_BEST; n];
    if n < 2 {
        return out;
    }
    // Best first: score descending, then index (np.lexsort((arange(n), -score))).
    // (Compared as numpy does: −0.0 equals 0.0.)
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|&a, &b| (-score[a as usize]).partial_cmp(&(-score[b as usize])).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    // The plane: x scaled by each point's own latitude.
    let xy: Vec<[f64; 2]> = (0..n).map(|i| [lon[i] * 111.32 * lat[i].to_radians().cos(), lat[i] * 110.57]).collect();
    let planar = |i: usize, j: u32| -> f64 {
        let (a, b) = (xy[i], xy[j as usize]);
        let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
        (dx * dx + dy * dy).sqrt()
    };
    let mut grid: Grid<2> = Grid::new(NEAR_KM);
    let mut far: Vec<u32> = Vec::new();
    for (r, &i) in order.iter().enumerate() {
        if r > 0 {
            match grid.nearest(&xy[i as usize], NEAR_KM, &|j| planar(i as usize, j)) {
                Some(d) => out[i as usize] = d,
                None => far.push(i),
            }
        }
        grid.insert(&xy[i as usize], i);
    }
    if far.is_empty() {
        return out;
    }
    // The nearest better one anywhere: chords between unit vectors order as great circles do.
    let u: Vec<[f64; 3]> = (0..n)
        .map(|i| {
            let (la, lo) = (lat[i].to_radians(), lon[i].to_radians());
            [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()]
        })
        .collect();
    let chord = |i: usize, j: u32| -> f64 {
        let (a, b) = (u[i], u[j as usize]);
        let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
        (dx * dx + dy * dy + dz * dz).sqrt()
    };
    let km = |c: f64| 6371.0 * 2.0 * (c / 2.0).min(1.0).asin();
    let mut rank = vec![0u32; n];
    for (r, &i) in order.iter().enumerate() {
        rank[i as usize] = r as u32;
    }
    // Far ones in rank order, each against the better ones: all of them when they're few (the best
    // of a kind have their nearest better anywhere on Earth), else a grid over the unit sphere.
    far.sort_by_key(|&i| rank[i as usize]);
    let mut sphere: Grid<3> = Grid::new(0.01);
    let mut next = 0usize;
    for &i in &far {
        let ri = rank[i as usize] as usize;
        let d = if ri <= 4096 {
            order[..ri].iter().map(|&j| chord(i as usize, j)).fold(f64::INFINITY, f64::min)
        } else {
            while next < ri {
                let j = order[next];
                sphere.insert(&u[j as usize], j);
                next += 1;
            }
            sphere.nearest(&u[i as usize], 2.0, &|j| chord(i as usize, j)).expect("a better point")
        };
        out[i as usize] = km(d);
    }
    out
}

/// A viewpoint's Wikidata item counts for its fame only when it's a landscape, a lookout or the
/// like (interest.py VIEW_ITEM: a mine or chapel tagged as a viewpoint doesn't).
pub fn view_item(description_en: &str) -> bool {
    const WORDS: [&str; 30] = [
        "viewpoint", "lookout", "observation", "belvedere", "mirador", "mountain", "hill", "peak", "summit", "cliff", "headland",
        "promontory", "point", "cape", "peninsula", "pass", "gorge", "canyon", "valley", "falls", "waterfall", "geosite", "park",
        "tower", "lighthouse", "beach", "bay", "island", "lake", "col",
    ];
    let d = description_en.to_lowercase();
    WORDS.iter().any(|w| {
        if *w != "col" {
            return d.contains(w);
        }
        // `col\b`: "col" followed by a non-word character or the end.
        d.match_indices("col").any(|(i, _)| d[i + 3..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_')))
    })
}

/// Fame from pageviews (log10 of 1 + the monthly average), else a little for Wikidata sitelinks;
/// and the pageviews when there are any (interest.py `fame`).
pub fn fame(pv: Option<f64>, sitelinks: u64) -> (f64, Option<f64>) {
    match pv {
        Some(pv) if pv != 0.0 => ((1.0 + pv).log10(), Some(pv)),
        _ => (if sitelinks != 0 { 0.3 * (1.0 + sitelinks as f64).log10() } else { 0.0 }, None),
    }
}

/// Each value's rank among the known ones, 0–1 (unknown: 0) (interest.py `percentile`:
/// searchsorted, side right).
pub fn percentile(vals: &[Option<f64>]) -> Vec<f64> {
    let mut known: Vec<f64> = vals.iter().flatten().copied().collect();
    if known.is_empty() {
        return vec![0.0; vals.len()];
    }
    known.sort_by(f64::total_cmp);
    vals.iter().map(|v| v.map_or(0.0, |v| known.partition_point(|k| *k <= v) as f64 / known.len() as f64)).collect()
}

/// A stop & sight's score before isolation (interest.py): fame, plus at most 0.01 for being named,
/// how much OpenStreetMap says about it (`rich`, of 5 tags, ÷ 3), and its size's rank in its kind.
pub fn poi_base(fame: f64, named: bool, rich: usize, size_rank: f64) -> f64 {
    let tie = 0.5 * (named as u8 as f64) + 0.2 * (rich as f64 / 3.0).min(1.0) + 0.3 * size_rank;
    fame + 0.01 * tie
}

/// A repeated name waits until the nearest more interesting place of its kind and name is 2^NAME_GAP
/// times the label spacing away on screen (dem/layers.py).
pub const NAME_GAP: f64 = 3.0;

/// layers.py `same_names`: each named place's mz at least NAME_GAP after the zoom where the nearest
/// more interesting place of its kind and name spans a pixel (between equally known ones, the more
/// isolated is the more interesting). `order`: the places as the layer has them (by fame); `key`:
/// a place's kind group and name; `fa`, `ia` rounded as the files hold them. Returns how many wait.
pub fn same_names(order: &[usize], key: &dyn Fn(usize) -> (String, String), lon: &[f64], lat: &[f64], fa: &[f64], ia: &[f64], mz: &mut [f64]) -> usize {
    let mut groups: std::collections::BTreeMap<(String, String), Vec<usize>> = Default::default();
    for &i in order {
        let (kind, name) = key(i);
        // " ".join(name.casefold().split()): lowercase, with casefold's ß → ss and ς → σ.
        let name = name.to_lowercase().replace('ß', "ss").replace('ς', "σ").split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty() && !mz[i].is_nan() {
            groups.entry((kind, name)).or_default().push(i);
        }
    }
    let mut n = 0;
    for idx in groups.values().filter(|v| v.len() >= 2) {
        let score: Vec<f64> = idx.iter().map(|&i| fa[i] + 1e-4 * ia[i] / (1.0 + ia[i])).collect();
        let iso = isolation(&idx.iter().map(|&i| lon[i]).collect::<Vec<_>>(), &idx.iter().map(|&i| lat[i]).collect::<Vec<_>>(), &score);
        for (j, &i) in idx.iter().enumerate() {
            let m = py_round(min_zoom(lat[i], iso[j]) + NAME_GAP, 2);
            if m > mz[i] {
                mz[i] = m;
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_as_python() {
        // round(x, n) in CPython.
        for (x, n, want) in [(0.25, 1, 0.2), (0.35, 1, 0.3), (1.0005, 3, 1.0), (2.675, 2, 2.67), (0.125, 2, 0.12), (-0.05, 1, -0.1), (12345.65, 1, 12345.6)] {
            assert_eq!(py_round(x, n), want, "round({x}, {n})");
        }
    }

    /// The brute force the script's answer equals: the nearest better by the plane within 60 km,
    /// else by great circle.
    fn brute(lon: &[f64], lat: &[f64], score: &[f64]) -> Vec<f64> {
        let n = lon.len();
        let better = |i: usize, j: usize| score[j] > score[i] || (score[j] == score[i] && j < i);
        (0..n)
            .map(|i| {
                let others: Vec<usize> = (0..n).filter(|&j| j != i && better(i, j)).collect();
                if others.is_empty() {
                    return IA_BEST;
                }
                let p = |k: usize| [lon[k] * 111.32 * lat[k].to_radians().cos(), lat[k] * 110.57];
                let d = others.iter().map(|&j| { let (a, b) = (p(i), p(j)); let (dx, dy) = (a[0] - b[0], a[1] - b[1]); (dx * dx + dy * dy).sqrt() }).fold(f64::INFINITY, f64::min);
                if d <= NEAR_KM {
                    return d;
                }
                let v = |k: usize| { let (la, lo) = (lat[k].to_radians(), lon[k].to_radians()); [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()] };
                let c = others.iter().map(|&j| { let (a, b) = (v(i), v(j)); let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]); (dx * dx + dy * dy + dz * dz).sqrt() }).fold(f64::INFINITY, f64::min);
                6371.0 * 2.0 * (c / 2.0).min(1.0).asin()
            })
            .collect()
    }

    #[test]
    fn isolation_is_the_nearest_better() {
        // A pseudo-random cloud: clusters within 60 km and outliers thousands of km away, ties.
        let mut s = 12345u64;
        let mut rnd = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        let (mut lon, mut lat, mut sc) = (Vec::new(), Vec::new(), Vec::new());
        for k in 0..600 {
            let (clon, clat) = if k % 50 == 0 { (rnd() * 360.0 - 180.0, rnd() * 160.0 - 80.0) } else { (6.0 + rnd() * 2.0, 45.0 + rnd() * 2.0) };
            lon.push(clon);
            lat.push(clat);
            sc.push((rnd() * 20.0).floor() / 10.0); // many ties
        }
        let got = isolation(&lon, &lat, &sc);
        let want = brute(&lon, &lat, &sc);
        for i in 0..lon.len() {
            assert_eq!(got[i].to_bits(), want[i].to_bits(), "point {i}: {} vs {}", got[i], want[i]);
        }
        assert_eq!(got.iter().filter(|&&v| v == IA_BEST).count(), 1);
    }

    #[test]
    fn min_zoom_as_the_script() {
        // min_zoom(51.5, 0.6): round(log2(78.2715 · cos(51.5°) / 0.6), 2).
        assert_eq!(min_zoom(51.5, 0.6), py_round((78.2715 * 51.5f64.to_radians().cos() / 0.6).log2(), 2));
        assert_eq!(min_zoom(0.0, 0.0), py_round((78.2715f64 / 0.01).log2(), 2));
    }
}
