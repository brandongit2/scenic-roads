//! How interesting each landmark is, value for value as the heritage chain's Python had it: interest isolation,
//! the zoom its names show from, and Python's rounding, for the `marks` job; and the heritage
//! chain's sites' fame and isolation (`heritage`).
//!
//!   ia  the distance (km) to the nearest place of the same kind that scores higher (ties: the
//!       earlier one); within 60 km on the plane (each point's x scaled by its own latitude),
//!       farther by great circle. The best of each kind has 20000.
//!   mz  the zoom at which that distance spans one pixel on 512 px tiles.
//!
//! Every floating-point operation is in the script's order, so results are bit for bit numpy's.

use det::Det;
use rayon::prelude::*;
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
    py_round((78.2715 * lat.to_radians().dcos() / ia_km.max(0.01)).dlog2(), 2)
}

/// The (cell, reach) km of each level of the plane search, finest first: a level answers a point
/// whose nearest better one is within its reach; the last reach is NEAR_KM.
const PLANE_LEVELS: [(f64, f64); 3] = [(2.0, 8.0), (8.0, 32.0), (20.0, NEAR_KM)];

/// A packed cell key's hash (the keys are already well spread).
#[derive(Default)]
struct CellHasher(u64);

impl std::hash::Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("cell keys are u64")
    }
    fn write_u64(&mut self, k: u64) {
        let h = k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 = h ^ (h >> 32);
    }
}

/// Points in rank order by uniform cell of the plane, each cell's points still in rank order.
struct Level {
    cell: f64,
    reach: f64,
    /// A cell's span in `items`.
    cells: HashMap<u64, (u32, u32), std::hash::BuildHasherDefault<CellHasher>>,
    /// The ranks, grouped by cell.
    items: Vec<u32>,
}

impl Level {
    fn key(&self, p: &[f64; 2]) -> [i64; 2] {
        [(p[0] / self.cell).floor() as i64, (p[1] / self.cell).floor() as i64]
    }

    fn pack(k: [i64; 2]) -> u64 {
        ((k[0] as u32 as u64) << 32) | k[1] as u32 as u64
    }

    fn new(xy: &[[f64; 2]], cell: f64, reach: f64) -> Self {
        let mut l = Level { cell, reach, cells: HashMap::default(), items: Vec::new() };
        let mut ix: Vec<(u64, u32)> = xy.iter().enumerate().map(|(r, p)| (Self::pack(l.key(p)), r as u32)).collect();
        ix.par_sort(); // by cell, then rank
        let mut s = 0;
        while s < ix.len() {
            let e = s + ix[s..].iter().take_while(|x| x.0 == ix[s].0).count();
            l.cells.insert(ix[s].0, (s as u32, e as u32));
            s = e;
        }
        l.items = ix.iter().map(|&(_, r)| r).collect();
        drop(ix);
        l
    }

    /// The nearest point of a lower rank than `r` (a better one) within `reach` of its position, searching
    /// rings of cells outward until no nearer one can be farther out.
    fn nearest(&self, xy: &[[f64; 2]], r: u32) -> Option<f64> {
        let p = xy[r as usize];
        let c = self.key(&p);
        let mut best: Option<f64> = None;
        let rings = (self.reach / self.cell).ceil() as i64 + 1;
        let scan = |dx: i64, dy: i64, best: &mut Option<f64>| {
            let Some(&(s, e)) = self.cells.get(&Self::pack([c[0] + dx, c[1] + dy])) else { return };
            for &j in &self.items[s as usize..e as usize] {
                if j >= r {
                    break;
                }
                let q = xy[j as usize];
                let (ex, ey) = (p[0] - q[0], p[1] - q[1]);
                let d = (ex * ex + ey * ey).sqrt();
                if d <= self.reach && best.is_none_or(|b| d < b) {
                    *best = Some(d);
                }
            }
        };
        for k in 0..=rings {
            // Every point within (k − 1) cells of the point's cell is at least (k − 1) × cell away
            // (in every coordinate), so a ring can stop the search once that exceeds the best.
            let lo = (k - 1) as f64 * self.cell;
            if lo > self.reach || best.is_some_and(|b| lo > b) {
                break;
            }
            if k == 0 {
                scan(0, 0, &mut best);
                continue;
            }
            for o in -k..=k {
                scan(o, -k, &mut best);
                scan(o, k, &mut best);
            }
            for o in -k + 1..k {
                scan(-k, o, &mut best);
                scan(k, o, &mut best);
            }
        }
        best
    }
}

/// A uniform grid of unit-sphere points by cell, for nearest-neighbour searches in growing rings.
struct Grid {
    cell: f64,
    cells: HashMap<[i64; 3], Vec<u32>>,
}

impl Grid {
    fn new(cell: f64) -> Self {
        Grid { cell, cells: HashMap::new() }
    }

    fn key(&self, p: &[f64; 3]) -> [i64; 3] {
        p.map(|v| (v / self.cell).floor() as i64)
    }

    fn insert(&mut self, p: &[f64; 3], i: u32) {
        self.cells.entry(self.key(p)).or_default().push(i);
    }

    /// The nearest point (by `dist`) within `max` of `p`, searching rings of cells outward until no
    /// nearer one can be farther out.
    fn nearest(&self, p: &[f64; 3], max: f64, dist: &dyn Fn(u32) -> f64) -> Option<f64> {
        let c = self.key(p);
        let mut best: Option<f64> = None;
        let rings = (max / self.cell).ceil() as i64 + 1;
        for r in 0..=rings {
            // Every point within (r − 1) cells of the point's cell is at least (r − 1) × cell away
            // (in every coordinate), so a ring can stop the search once that exceeds the best.
            let lo = (r - 1) as f64 * self.cell;
            if lo > max || best.is_some_and(|b| lo > b) {
                break;
            }
            // The cells at Chebyshev distance exactly `r` from the point's.
            for x in -r..=r {
                for y in -r..=r {
                    let step = if x.abs() == r || y.abs() == r { 1 } else { (2 * r).max(1) };
                    for z in (-r..=r).step_by(step as usize) {
                        if let Some(v) = self.cells.get(&[c[0] + x, c[1] + y, c[2] + z]) {
                            for &j in v {
                                let d = dist(j);
                                if d <= max && best.is_none_or(|b| d < b) {
                                    best = Some(d);
                                }
                            }
                        }
                    }
                }
            }
        }
        best
    }
}

/// Distance (km) from each point to the nearest point with a higher score (ties: the earlier index);
/// `IA_BEST` for the best (the Python's `isolation`: scipy's k nearest neighbours).
pub fn isolation(lon: &[f64], lat: &[f64], score: &[f64]) -> Vec<f64> {
    let n = lon.len();
    let mut out = vec![IA_BEST; n];
    if n < 2 {
        return out;
    }
    // Best first: score descending, then index (np.lexsort((arange(n), -score))).
    // (Compared as numpy does: −0.0 equals 0.0.)
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|&a, &b| {
        (-score[a as usize]).partial_cmp(&(-score[b as usize])).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
    });
    // The plane: x scaled by each point's own latitude, the points in rank order.
    let xy: Vec<[f64; 2]> = order
        .iter()
        .map(|&i| {
            let i = i as usize;
            [lon[i] * 111.32 * lat[i].to_radians().dcos(), lat[i] * 110.57]
        })
        .collect();
    let levels: Vec<Level> = PLANE_LEVELS.iter().map(|&(cell, reach)| Level::new(&xy, cell, reach)).collect();
    // Each point's nearest better one in the plane within NEAR_KM, the points at once: every
    // answer is an exact minimum over the better ones, whatever order they are found in.
    let near: Vec<Option<f64>> =
        (1..n as u32).into_par_iter().map(|r| levels.iter().find_map(|l| l.nearest(&xy, r))).collect();
    let mut far: Vec<u32> = Vec::new();
    for (r, d) in near.into_iter().enumerate() {
        match d {
            Some(d) => out[order[r + 1] as usize] = d,
            None => far.push(order[r + 1]),
        }
    }
    if far.is_empty() {
        return out;
    }
    // The nearest better one anywhere: chords between unit vectors order as great circles do.
    let u: Vec<[f64; 3]> = (0..n)
        .map(|i| {
            let (la, lo) = (lat[i].to_radians(), lon[i].to_radians());
            [la.dcos() * lo.dcos(), la.dcos() * lo.dsin(), la.dsin()]
        })
        .collect();
    let chord = |i: usize, j: u32| -> f64 {
        let (a, b) = (u[i], u[j as usize]);
        let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
        (dx * dx + dy * dy + dz * dz).sqrt()
    };
    let km = |c: f64| 6371.0 * 2.0 * (c / 2.0).min(1.0).dasin();
    let mut rank = vec![0u32; n];
    for (r, &i) in order.iter().enumerate() {
        rank[i as usize] = r as u32;
    }
    // Far ones in rank order, each against the better ones: all of them when they're few (the best
    // of a kind have their nearest better anywhere on Earth), else a grid over the unit sphere.
    far.sort_by_key(|&i| rank[i as usize]);
    let mut sphere = Grid::new(0.01);
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
/// like (VIEW_ITEM: a mine or chapel tagged as a viewpoint doesn't).
pub fn view_item(description_en: &str) -> bool {
    const WORDS: [&str; 30] = [
        "viewpoint",
        "lookout",
        "observation",
        "belvedere",
        "mirador",
        "mountain",
        "hill",
        "peak",
        "summit",
        "cliff",
        "headland",
        "promontory",
        "point",
        "cape",
        "peninsula",
        "pass",
        "gorge",
        "canyon",
        "valley",
        "falls",
        "waterfall",
        "geosite",
        "park",
        "tower",
        "lighthouse",
        "beach",
        "bay",
        "island",
        "lake",
        "col",
    ];
    let d = description_en.to_lowercase();
    WORDS.iter().any(|w| {
        if *w != "col" {
            return d.contains(w);
        }
        // `col\b`: "col" followed by a non-word character or the end.
        d.match_indices("col")
            .any(|(i, _)| d[i + 3..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_')))
    })
}

/// Fame from pageviews (log10 of 1 + the monthly average), else a little for Wikidata sitelinks;
/// and the pageviews when there are any.
pub fn fame(pv: Option<f64>, sitelinks: u64) -> (f64, Option<f64>) {
    match pv {
        Some(pv) if pv != 0.0 => ((1.0 + pv).dlog10(), Some(pv)),
        _ => (if sitelinks != 0 { 0.3 * (1.0 + sitelinks as f64).dlog10() } else { 0.0 }, None),
    }
}

/// Each value's rank among the known ones, 0–1 (unknown: 0) (numpy's searchsorted, side
/// right).
pub fn percentile(vals: &[Option<f64>]) -> Vec<f64> {
    let mut known: Vec<f64> = vals.iter().flatten().copied().collect();
    if known.is_empty() {
        return vec![0.0; vals.len()];
    }
    known.sort_by(f64::total_cmp);
    vals.iter().map(|v| v.map_or(0.0, |v| known.partition_point(|k| *k <= v) as f64 / known.len() as f64)).collect()
}

/// A stop & sight's score before isolation: fame, plus at most 0.01 for being named,
/// how much OpenStreetMap says about it (`rich`, of 5 tags, ÷ 3), and its size's rank in its kind.
pub fn poi_base(fame: f64, named: bool, rich: usize, size_rank: f64) -> f64 {
    let tie = 0.5 * (named as u8 as f64) + 0.2 * (rich as f64 / 3.0).min(1.0) + 0.3 * size_rank;
    fame + 0.01 * tie
}

/// A repeated name waits until the nearest more interesting place of its kind and name is 2^NAME_GAP
/// times the label spacing away on screen (crate::heritage_layers, crate::marksjob).
pub const NAME_GAP: f64 = 3.0;

/// Each named place's mz at least NAME_GAP after the zoom where the nearest
/// more interesting place of its kind and name spans a pixel (between equally known ones, the more
/// isolated is the more interesting). `order`: the places as the layer has them (by fame); `key`:
/// a place's kind group and name; `fa`, `ia` rounded as the files hold them. Returns how many wait.
pub fn same_names(
    order: &[usize],
    key: &dyn Fn(usize) -> (String, String),
    lon: &[f64],
    lat: &[f64],
    fa: &[f64],
    ia: &[f64],
    mz: &mut [f64],
) -> usize {
    let mut groups: std::collections::BTreeMap<(String, String), Vec<usize>> = Default::default();
    for &i in order {
        let (kind, name) = key(i);
        // " ".join(name.casefold().split()): lowercase, with casefold's ß → ss and ς → σ.
        let name =
            name.to_lowercase().replace('ß', "ss").replace('ς', "σ").split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty() && !mz[i].is_nan() {
            groups.entry((kind, name)).or_default().push(i);
        }
    }
    let mut n = 0;
    for idx in groups.values().filter(|v| v.len() >= 2) {
        let score: Vec<f64> = idx.iter().map(|&i| fa[i] + 1e-4 * ia[i] / (1.0 + ia[i])).collect();
        let iso = isolation(
            &idx.iter().map(|&i| lon[i]).collect::<Vec<_>>(),
            &idx.iter().map(|&i| lat[i]).collect::<Vec<_>>(),
            &score,
        );
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

/// The heritage chain's interest, in its stand-in root `root`:
/// each heritage site's fame (`fa`, `pv`: its Wikidata item's pageviews, a World Heritage Site's
/// best-known item's) and isolation (`ia`, `mz`: among the sites as the map shows them, a World
/// Heritage Site in several components once, at its dot; its other components get none), written
/// back into data/build/heritage.json as Python's `json` writes it.
pub fn heritage(root: &std::path::Path) -> anyhow::Result<()> {
    use crate::pyjson::{dumps, loads, Py};
    use crate::timings::{phase, Class};
    use anyhow::Context;
    let b = root.join("data/build");
    let p = phase("the layers and details read", Class::Disk);
    let pv_path = root.join("data/pageviews/items.json");
    let views: HashMap<String, Py> = if pv_path.exists() {
        match loads(&std::fs::read_to_string(&pv_path)?).context("parse items.json")? {
            Py::Dict(d) => d.into_iter().collect(),
            _ => anyhow::bail!("items.json not an object"),
        }
    } else {
        HashMap::new()
    };
    if views.is_empty() {
        eprintln!("interest: no pageviews yet (pageviews.py); fame from sitelinks only");
    }
    let mut hfc = loads(&std::fs::read_to_string(b.join("heritage.json"))?).context("parse heritage.json")?;
    let mut det: HashMap<String, Py> = HashMap::new();
    if b.join("details-heritage.jsonl").exists() {
        for line in std::fs::read_to_string(b.join("details-heritage.jsonl"))?.lines() {
            let r = loads(line).context("a line of details-heritage.jsonl")?;
            det.insert(r.at("i").key(), r);
        }
    }
    let sites = match b.join("whs-sites.json") {
        s if s.exists() => loads(&std::fs::read_to_string(s)?).context("parse whs-sites.json")?,
        _ => Py::Dict(vec![]),
    };
    drop(p);
    let p = phase("the fame scored", Class::Compute);
    let Some(Py::List(hf)) = hfc.get_mut("features") else { anyhow::bail!("heritage.json: no features") };
    let role = crate::heritage_layers::roles(hf, &sites)?;
    let views_of = |q: &str| views.get(q).filter(|v| v.truthy());
    let mut base = vec![0.0; hf.len()];
    for (i, f) in hf.iter_mut().enumerate() {
        let p = f.get_mut("properties").context("a site without properties")?;
        let empty = Py::Dict(vec![]);
        let d = det.get(&p.at("i").key()).unwrap_or(&empty);
        let qid = d.at("qid").as_str().filter(|q| !q.is_empty());
        let sl = d.get("sl").map_or(Some(0.0), Py::number).context("sl not a number")?;
        // (Python's fame: the item's pageviews, else a little for its sitelinks.)
        let pv0 = qid.and_then(views_of);
        let (mut fm, mut pv): (f64, Option<&Py>) = match pv0 {
            Some(v) => (crate::interest::fame(v.number(), 0).0, Some(v)),
            None => (fame(None, sl as u64).0, None),
        };
        if let Some(sid) = crate::heritage_layers::whs_id(p) {
            for q in sites.at(&sid).at("q").list().into_iter().flatten() {
                if let Some(v) = q.as_str().and_then(views_of) {
                    let f = (1.0 + v.number().context("pageviews not a number")?).dlog10();
                    if f > fm {
                        (fm, pv) = (f, Some(v));
                    }
                }
            }
        }
        let level = Some(p.at("level")).filter(|l| l.truthy()).map_or(Some(5.0), Py::number).context("level")?;
        let tie = 0.5 * (p.at("name").truthy() as u8 as f64)
            + 0.2 * (d.at("qid").truthy() as u8 as f64)
            + 0.3 * (5.0 - level) / 4.0;
        base[i] = fm + 0.01 * tie;
        for k in ["fa", "pv", "ia", "mz"] {
            p.pop(k);
        }
        p.set("fa", Py::Float(py_round(base[i], 3)));
        if let Some(v) = pv {
            // (Python's round(pv): an integer, half to even.)
            let n = match v {
                Py::Int(s) => s.clone(),
                v => format!("{:.0}", v.number().context("pageviews not a number")?.round_ties_even()),
            };
            p.set("pv", Py::Int(n));
        }
    }
    drop(p);
    // Isolation among the sites as the map shows them: a merged site once, at its dot.
    let p = phase("the isolation measured", Class::Compute);
    let keep: Vec<usize> = (0..hf.len()).filter(|i| !role.get(i).is_some_and(|r| !r.0)).collect();
    let mut lon = Vec::with_capacity(keep.len());
    let mut lat = Vec::with_capacity(keep.len());
    let mut n_at = 0;
    for &i in &keep {
        let at = match role.get(&i) {
            Some((true, sid)) => {
                n_at += 1;
                sites.at(sid).at("dot")
            }
            _ => hf[i].at("geometry").at("coordinates"),
        };
        let c = at.list().context("a site without coordinates")?;
        lon.push(c[0].number().context("a coordinate")?);
        lat.push(c.get(1).and_then(Py::number).context("a coordinate")?);
    }
    let ia = isolation(&lon, &lat, &keep.iter().map(|&i| base[i]).collect::<Vec<_>>());
    for (n, &i) in keep.iter().enumerate() {
        let p = hf[i].get_mut("properties").unwrap();
        p.set("ia", Py::Float(py_round(ia[n], 1)));
        p.set("mz", Py::Float(min_zoom(lat[n], ia[n])));
    }
    drop(p);
    eprintln!(
        "interest: heritage        {:7}, {} best within 20 km ({n_at} World Heritage Sites as one dot, {} of their components apart)",
        keep.len(),
        ia.iter().filter(|&&x| x >= 20.0).count(),
        hf.len() - keep.len()
    );
    let _p = phase("the layers written", Class::Disk);
    let tmp = b.join("heritage.json.tmp");
    std::fs::write(&tmp, dumps(&hfc))?;
    std::fs::rename(&tmp, b.join("heritage.json"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_as_python() {
        // round(x, n) in CPython.
        for (x, n, want) in [
            (0.25, 1, 0.2),
            (0.35, 1, 0.3),
            (1.0005, 3, 1.0),
            (2.675, 2, 2.67),
            (0.125, 2, 0.12),
            (-0.05, 1, -0.1),
            (12345.65, 1, 12345.6),
        ] {
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
                let p = |k: usize| [lon[k] * 111.32 * lat[k].to_radians().dcos(), lat[k] * 110.57];
                let d = others
                    .iter()
                    .map(|&j| {
                        let (a, b) = (p(i), p(j));
                        let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
                        (dx * dx + dy * dy).sqrt()
                    })
                    .fold(f64::INFINITY, f64::min);
                if d <= NEAR_KM {
                    return d;
                }
                let v = |k: usize| {
                    let (la, lo) = (lat[k].to_radians(), lon[k].to_radians());
                    [la.dcos() * lo.dcos(), la.dcos() * lo.dsin(), la.dsin()]
                };
                let c = others
                    .iter()
                    .map(|&j| {
                        let (a, b) = (v(i), v(j));
                        let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
                        (dx * dx + dy * dy + dz * dz).sqrt()
                    })
                    .fold(f64::INFINITY, f64::min);
                6371.0 * 2.0 * (c / 2.0).min(1.0).dasin()
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
            let (clon, clat) = if k % 50 == 0 {
                (rnd() * 360.0 - 180.0, rnd() * 160.0 - 80.0)
            } else {
                (6.0 + rnd() * 2.0, 45.0 + rnd() * 2.0)
            };
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

    /// Seeded clouds of every spacing the levels' edges care about, against the brute force bit for
    /// bit, on one thread and on the default pool.
    #[test]
    fn isolation_is_the_nearest_better_in_random_clouds() {
        for seed in 1..=6u64 {
            let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut rnd = move || {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (s >> 11) as f64 / (1u64 << 53) as f64
            };
            let (mut lon, mut lat, mut sc) = (Vec::new(), Vec::new(), Vec::new());
            // Clusters of very different densities (km → degrees at ~110 km a degree), some south
            // and west of the origin.
            for (spread_km, count) in [(0.1, 60), (20.0, 120), (60.0, 120), (800.0, 40)] {
                let (clon, clat) = (rnd() * 120.0 - 60.0, rnd() * 100.0 - 50.0);
                for _ in 0..count {
                    lon.push(clon + (rnd() - 0.5) * spread_km / 111.0);
                    lat.push(clat + (rnd() - 0.5) * spread_km / 110.0);
                }
            }
            // Pairs exactly at the levels' reaches, and points at the same spot.
            for km in [8.0, 32.0, 60.0, 59.9, 60.1] {
                let (a, b) = (rnd() * 100.0 - 50.0, rnd() * 100.0 - 50.0);
                lon.extend([b, b]);
                lat.extend([a, a + km / 110.57]);
            }
            for _ in 0..4 {
                let (a, b) = (rnd() * 100.0 - 50.0, rnd() * 100.0 - 50.0);
                lon.extend([b; 3]);
                lat.extend([a; 3]);
            }
            // Heavy ties: a handful of distinct scores.
            sc.extend((0..lon.len()).map(|_| (rnd() * 4.0).floor()));
            let want = brute(&lon, &lat, &sc);
            let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
            for got in [isolation(&lon, &lat, &sc), one.install(|| isolation(&lon, &lat, &sc))] {
                for i in 0..lon.len() {
                    assert_eq!(got[i].to_bits(), want[i].to_bits(), "seed {seed} point {i}: {} vs {}", got[i], want[i]);
                }
            }
        }
    }

    #[test]
    fn min_zoom_as_the_script() {
        // min_zoom(51.5, 0.6): round(log2(78.2715 · cos(51.5°) / 0.6), 2).
        assert_eq!(min_zoom(51.5, 0.6), py_round((78.2715 * 51.5f64.to_radians().dcos() / 0.6).dlog2(), 2));
        assert_eq!(min_zoom(0.0, 0.0), py_round((78.2715f64 / 0.01).dlog2(), 2));
    }
}

// ---- the stops & sights' filter values ------------------------------------------------------

/// The first number in a value, a comma as its decimal point ("12,5 m" →
/// 12.5); none without one.
pub fn fp_num(v: &serde_json::Value) -> Option<f64> {
    let s = match v {
        serde_json::Value::Null => return None,
        serde_json::Value::String(s) if s.is_empty() => return None,
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        other => other.to_string(),
    };
    // -?\d+(?:[.,]\d+)?
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let neg = b[i] == b'-' && i + 1 < b.len() && b[i + 1].is_ascii_digit();
        let j0 = if neg { i + 1 } else { i };
        if j0 < b.len() && b[j0].is_ascii_digit() {
            let mut j = j0;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < b.len() && (b[j] == b'.' || b[j] == b',') && b[j + 1].is_ascii_digit() {
                let mut k = j + 1;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                j = k;
            }
            return s[start..j].replace(',', ".").parse().ok();
        }
        i += 1;
    }
    None
}

/// The first run of 3–4 digits (with a minus sign before it).
pub fn fp_year(v: &serde_json::Value) -> Option<i64> {
    let s = match v {
        serde_json::Value::Null => return None,
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        other => other.to_string(),
    };
    // -?\d{3,4}: the leftmost match; greedy, so 4 digits when there are.
    let b = s.as_bytes();
    for i in 0..b.len() {
        let neg = b[i] == b'-';
        let j0 = if neg { i + 1 } else { i };
        let digits = b[j0.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits >= 3 {
            return s[i..j0 + digits.min(4)].parse().ok();
        }
    }
    None
}

fn fp_yes(v: &serde_json::Value) -> bool {
    matches!(v, serde_json::Value::Bool(true)) || matches!(v.as_str(), Some("yes" | "designated"))
}

/// Python truthiness of a JSON value.
fn truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64() != Some(0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

/// A Python `round(v, n)` of a JSON number as the files hold it: an integer stays one.
fn round_json(v: f64, n: usize, was_int: bool) -> serde_json::Value {
    if was_int {
        serde_json::json!(v as i64)
    } else {
        serde_json::json!(py_round(v, n))
    }
}

/// A stop & sight's filter values from its details record `d` (OSM tags and
/// `wd`, the Wikidata facts) and its peak computation `pk` (prominence `p`, isolation `iso`).
pub fn poi_filter_props(
    kind: &str,
    d: &serde_json::Value,
    pk: Option<&serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::{json, Value};
    let mut p = serde_json::Map::new();
    let null = Value::Null;
    let get = |k: &str| d.get(k).unwrap_or(&null);
    let wd = d.get("wd").unwrap_or(&null);
    let wget = |k: &str| wd.get(k).unwrap_or(&null);
    let is_int = |v: &Value| v.is_i64() || v.is_u64();
    match kind {
        "peak" => {
            // pr: the tag, else Wikidata's, else computed (`round` to an integer).
            let pr = fp_num(get("prominence")).map(|v| (v, false)).or_else(|| {
                if wd.get("prominence").is_some() {
                    wget("prominence").as_f64().map(|v| (v, is_int(wget("prominence"))))
                } else {
                    pk.and_then(|k| k.get("p")).and_then(Value::as_f64).map(|v| (v, false))
                }
            });
            if let Some((v, _)) = pr {
                p.insert("pr".into(), json!(py_round(v, 0) as i64));
            }
            let iso = if wd.get("isolation").is_some() {
                wget("isolation").as_f64().map(|v| v / 1000.0)
            } else {
                pk.and_then(|k| k.get("iso")).and_then(Value::as_f64)
            };
            if let Some(v) = iso {
                p.insert("is".into(), json!(py_round(v, 2)));
            }
        }
        "waterfall" => {
            let h = fp_num(get("height"))
                .map(|v| (v, false))
                .or_else(|| wget("height").as_f64().map(|v| (v, is_int(wget("height")))));
            if let Some((v, int)) = h {
                p.insert("h".into(), round_json(v, 1, int));
            }
        }
        "lighthouse" => {
            let h = fp_num(get("height"))
                .map(|v| (v, false))
                .or_else(|| wget("height").as_f64().map(|v| (v, is_int(wget("height")))));
            let fh_src = if truthy(get("seamark:light:height")) {
                get("seamark:light:height")
            } else {
                get("seamark:light:1:height")
            };
            let fh = fp_num(fh_src)
                .map(|v| (v, false))
                .or_else(|| wget("focal").as_f64().map(|v| (v, is_int(wget("focal")))));
            let rg_src = if truthy(get("seamark:light:range")) {
                get("seamark:light:range")
            } else {
                get("seamark:light:1:range")
            };
            let rg = fp_num(rg_src).map(|v| (v, false));
            let y = fp_year(if truthy(get("start_date")) { get("start_date") } else { wget("inception") });
            for (key, v) in [("h", h), ("fh", fh), ("rg", rg)] {
                if let Some((v, int)) = v {
                    p.insert(key.into(), round_json(v, 1, int));
                }
            }
            if let Some(y) = y {
                p.insert("y".into(), json!(y));
            }
        }
        "covered_bridge" => {
            if truthy(get("length_m")) {
                p.insert("len".into(), get("length_m").clone());
            }
            if let Some(y) = fp_year(if truthy(get("start_date")) { get("start_date") } else { wget("inception") }) {
                p.insert("y".into(), json!(y));
            }
        }
        "viewpoint" => {
            let dr = match get("direction") {
                Value::Null => String::new(),
                Value::String(s) => s.trim().to_uppercase(),
                other => other.to_string().trim().to_uppercase(),
            };
            // re.fullmatch(r"(\d+)\s*-\s*(\d+)", dr): a span of at least 300° (a whole turn for 0).
            let range = dr.split_once('-').and_then(|(a, b)| {
                let (a, b) = (a.trim_end(), b.trim_start());
                let digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
                (digits(a) && digits(b)).then(|| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?))).flatten()
            });
            let pan = matches!(dr.as_str(), "0-360" | "360" | "ALL")
                || range.is_some_and(|(a, b)| {
                    let span = (b - a).rem_euclid(360);
                    (if span == 0 { 360 } else { span }) >= 300
                });
            if pan {
                p.insert("pan".into(), json!(1));
            }
            if truthy(get("tower:type")) || get("man_made").as_str() == Some("tower") {
                p.insert("tw".into(), json!(1));
            }
        }
        "rest_area" | "picnic_site" | "trailhead" => {
            let fac = (if fp_yes(get("toilets")) { 1 } else { 0 })
                | (if fp_yes(get("drinking_water")) { 2 } else { 0 })
                | (if fp_yes(get("shelter")) || fp_yes(get("covered")) { 4 } else { 0 })
                | (if fp_yes(get("picnic_table")) || fp_yes(get("bench")) { 8 } else { 0 })
                | (if fp_yes(get("fireplace")) || fp_yes(get("bbq")) { 16 } else { 0 })
                | (if fp_yes(get("parking")) { 32 } else { 0 });
            if fac != 0 {
                p.insert("fac".into(), json!(fac));
            }
        }
        _ => {}
    }
    p
}
