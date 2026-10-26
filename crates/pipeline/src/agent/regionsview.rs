//! The build page's regions (docs/pool.md §10, Regions; task #512): each enabled region's outline
//! for drawing, where it sits in the regions' tree, and what downloading it weighs. Made by the
//! lead once per regions' accepted version and catalogue (`Agent::regions_view`), written for the
//! other Macs' pages beside its forecast's input, and answered at `/work/regions`.
//!
//! - **Outlines** (`fill`, `edge`): the region's drawn shapes (the regions' accepted version's
//!   `@drawn` files) in Web Mercator, the world 0–1000 each way, simplified to `SIMPLIFY` (about
//!   7 km at the equator; a ring or line smaller than `TINY` left out) at 0.1 (about 4 km): `fill`
//!   the outer rings, to fill (nonzero); `edge` the lines to stroke, the stretches two of its
//!   shapes share along a parallel or meridian (where an outline entry was cut by a tile's edge)
//!   left out.
//! - **Crumbs**: the names of the folders it's in, from the top (their `_group.toml` names), then
//!   its own (crate::inputs::regions::read::tree).
//! - **Bytes**: the catalogue's files a download of it copies (crate::areafiles::files_of over the
//!   outlines the catalogue records), summed once per catalogue; the basemap's pieces, worked out
//!   by each Mac's map server from the archives, and its terrain detail aren't counted. None for a
//!   region the catalogue doesn't have yet.

use crate::coverage::DrawnRegion;
use crate::inputs::regions::read::Folder;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// How far a simplified outline may stray (Web Mercator, the world 1 each way).
pub const SIMPLIFY: f64 = 0.000_18;
/// A ring or line spanning less than this each way is left out.
const TINY: f64 = 0.000_15;

/// What `build` made: the page's regions (None: the outlines couldn't be read), the catalogue's
/// sizes as read (its number; None: none read), and how long it took.
pub struct Made {
    pub view: Option<Value>,
    pub bytes: Option<(u64, BTreeMap<String, u64>)>,
    pub took: std::time::Duration,
}

/// The page's regions of accepted version `a` (its enabled `regions`) and served catalogue `n` in
/// `root`: its drawn files read, and the catalogue's sizes unless `had` has them (its number's).
/// Run off the agent's loop (`Agent::regions_view`); a read that fails is logged, its part left out.
pub fn build(
    root: &store::nasfs::NasRoot,
    a: &crate::inputs::Accepted,
    regions: &[crate::agent::recipes::Recipe],
    n: Option<u64>,
    had: Option<(u64, BTreeMap<String, u64>)>,
) -> Made {
    let t = std::time::Instant::now();
    let bytes = match (n, had) {
        (Some(n), Some((m, b))) if m == n => Some((n, b)),
        (Some(n), _) => match store::catalog::read_n(&root.join("catalog"), n) {
            Ok(cat) => Some((n, bytes_of(&cat))),
            Err(e) => {
                eprintln!("agent: the regions' sizes from catalogue {n}: {e:#}");
                None
            }
        },
        (None, _) => None,
    };
    let view = match crate::inputs::regions::read::drawn(root, a, regions) {
        Ok(drawn) => {
            let tree = crate::inputs::regions::read::tree(&a.index);
            let b = bytes.as_ref().map(|(_, b)| b.clone()).unwrap_or_default();
            Some(make(&a.version, n, regions, &drawn, &tree, &b))
        }
        Err(e) => {
            eprintln!("agent: the regions' outlines for the build page: {e:#}");
            None
        }
    };
    Made { view, bytes, took: t.elapsed() }
}

/// The page's regions: `regions` (the enabled ones, by id), drawn as `drawn` has them, each in the
/// regions' `tree`, its bytes from `bytes` (by id: the catalogue's). `version` and `catalog` name
/// what it was made of.
pub fn make(
    version: &str,
    catalog: Option<u64>,
    regions: &[crate::agent::recipes::Recipe],
    drawn: &[DrawnRegion],
    tree: &Folder,
    bytes: &BTreeMap<String, u64>,
) -> Value {
    let crumbs = crumbs_of(tree);
    let drawn: BTreeMap<&str, &DrawnRegion> = drawn.iter().map(|d| (d.id.as_str(), d)).collect();
    let out: Vec<Value> = regions
        .iter()
        .map(|r| {
            let (fill, edge) = drawn.get(r.id.as_str()).map(|d| outline(d)).unwrap_or_default();
            let mut c = crumbs.get(&r.id).cloned().unwrap_or_default();
            c.push(r.name.clone());
            json!({ "id": r.id, "name": r.name, "crumbs": c, "bytes": bytes.get(&r.id), "fill": fill, "edge": edge })
        })
        .collect();
    json!({ "version": version, "catalog": catalog, "regions": out })
}

/// The bytes a download of each region the catalogue records copies of its files (module doc).
pub fn bytes_of(cat: &store::catalog::Catalog) -> BTreeMap<String, u64> {
    let recorded: Vec<DrawnRegion> =
        cat.coverage.get("regions").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    recorded
        .iter()
        .map(|r| (r.id.clone(), crate::areafiles::files_of(cat, &crate::areafiles::outer_rings(r)).bytes))
        .collect()
}

/// Each region's folders' names, from the top (the root's own left out).
fn crumbs_of(tree: &Folder) -> BTreeMap<String, Vec<String>> {
    fn walk(f: &Folder, above: &[String], out: &mut BTreeMap<String, Vec<String>>) {
        let mut here = above.to_vec();
        if !f.path.is_empty() {
            // (A folder without a `_group.toml` name goes by its own: "united-states" as
            // "United States".)
            let own = f.path.rsplit('/').next().unwrap_or_default();
            here.push(if f.name == own { said(own) } else { f.name.clone() });
        }
        for id in &f.regions {
            out.insert(id.clone(), here.clone());
        }
        for sub in &f.folders {
            walk(sub, &here, out);
        }
    }
    let mut out = BTreeMap::new();
    walk(tree, &[], &mut out);
    out
}

/// A folder's own name in words: hyphens as spaces, each word capitalised but the small ones.
fn said(slug: &str) -> String {
    let words: Vec<String> = slug
        .split('-')
        .filter(|w| !w.is_empty())
        .enumerate()
        .map(|(i, w)| {
            if i > 0 && ["and", "of", "the", "da", "de"].contains(&w) {
                return w.to_string();
            }
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
        })
        .collect();
    words.join(" ")
}

/// A point in Web Mercator, the world 1 each way (latitudes past ±85° at the edge).
fn merc(p: [f64; 2]) -> [f64; 2] {
    let lat = p[1].clamp(-85.0, 85.0).to_radians();
    [(p[0] + 180.0) / 360.0, (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0]
}

/// Douglas–Peucker: the points of `pts` kept within `eps` of the line.
fn simplify(pts: &[[f64; 2]], eps: f64) -> Vec<[f64; 2]> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let ([x0, y0], [x1, y1]) = (pts[a], pts[b]);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let l = dx.hypot(dy);
        let (mut far, mut at) = (0.0, 0);
        for (i, &[x, y]) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = if l > 1e-12 { (dy * x - dx * y + x1 * y0 - y1 * x0).abs() / l } else { (x - x0).hypot(y - y0) };
            if d > far {
                (far, at) = (d, i);
            }
        }
        if far > eps {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    pts.iter().zip(&keep).filter(|(_, &k)| k).map(|(p, _)| *p).collect()
}

/// Simplified, flattened and scaled to 0–1000 at 0.1; None when it's too small to see.
fn flat(pts: &[[f64; 2]], min: usize) -> Option<Vec<f64>> {
    let (lo, hi) = pts.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| {
        ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
    });
    if hi[0] - lo[0] < TINY && hi[1] - lo[1] < TINY {
        return None;
    }
    let s = simplify(pts, SIMPLIFY);
    (s.len() >= min).then(|| s.iter().flat_map(|p| p.iter().map(|v| (v * 10_000.0).round() / 10.0)).collect())
}

type Seg = ([i64; 2], [i64; 2]);

/// A region's `fill` and `edge` (module doc).
fn outline(d: &DrawnRegion) -> (Vec<Vec<f64>>, Vec<Vec<f64>>) {
    // (Points to 1e-5 degrees, as the drawn files hold them, so shared edges match exactly.)
    let key = |p: &[f64; 2]| [(p[0] * 1e5).round() as i64, (p[1] * 1e5).round() as i64];
    let mut fill = Vec::new();
    let mut segs: Vec<Seg> = Vec::new();
    for poly in d.shapes.values().flatten() {
        let Some(ring) = poly.first().filter(|r| r.len() >= 3) else { continue };
        let mut r: Vec<[i64; 2]> = ring.iter().map(key).collect();
        if r.first() != r.last() {
            r.push(r[0]);
        }
        segs.extend(r.windows(2).filter(|w| w[0] != w[1]).map(|w| (w[0], w[1])));
        let pts: Vec<[f64; 2]> = ring.iter().map(|&p| merc(p)).collect();
        if let Some(f) = flat(&pts, 4) {
            fill.push(f);
        }
    }
    let lines = chain(&edges(segs));
    let edge = lines
        .iter()
        .filter_map(|l| flat(&l.iter().map(|p| merc([p[0] as f64 / 1e5, p[1] as f64 / 1e5])).collect::<Vec<_>>(), 2))
        .collect();
    (fill, edge)
}

/// The segments to stroke: each not along a parallel or meridian kept; of those along one, the
/// stretches only one segment covers (two covering it: a cut between two shapes, not a border).
fn edges(segs: Vec<Seg>) -> Vec<Seg> {
    // (By axis and its value: the stretches' ends, +1 where one starts and -1 where it ends.)
    let mut along: BTreeMap<(u8, i64), BTreeMap<i64, i32>> = BTreeMap::new();
    let mut keep = Vec::new();
    for (a, b) in segs {
        let (axis, v, lo, hi) = if a[0] == b[0] {
            (0, a[0], a[1].min(b[1]), a[1].max(b[1]))
        } else if a[1] == b[1] {
            (1, a[1], a[0].min(b[0]), a[0].max(b[0]))
        } else {
            keep.push((a, b));
            continue;
        };
        let ev = along.entry((axis, v)).or_default();
        *ev.entry(lo).or_default() += 1;
        *ev.entry(hi).or_default() -= 1;
    }
    for ((axis, v), ev) in along {
        let mut depth = 0;
        let pts: Vec<(i64, i32)> = ev.into_iter().collect();
        for w in pts.windows(2) {
            depth += w[0].1;
            if depth == 1 {
                let (p, q) = (w[0].0, w[1].0);
                keep.push(if axis == 0 { ([v, p], [v, q]) } else { ([p, v], [q, v]) });
            }
        }
    }
    keep
}

/// Segments joined into lines where they meet end to end.
fn chain(segs: &[Seg]) -> Vec<Vec<[i64; 2]>> {
    let mut at: BTreeMap<[i64; 2], Vec<usize>> = BTreeMap::new();
    for (i, (a, b)) in segs.iter().enumerate() {
        at.entry(*a).or_default().push(i);
        at.entry(*b).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut lines = Vec::new();
    for i in 0..segs.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut line = std::collections::VecDeque::from([segs[i].0, segs[i].1]);
        for back in [false, true] {
            loop {
                let tip = if back { line[0] } else { line[line.len() - 1] };
                let Some(&j) = at[&tip].iter().find(|&&j| !used[j]) else { break };
                used[j] = true;
                let next = if segs[j].0 == tip { segs[j].1 } else { segs[j].0 };
                if back {
                    line.push_front(next);
                } else {
                    line.push_back(next);
                }
            }
        }
        lines.push(line.into_iter().collect());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;

    fn square(w: f64, s: f64, e: f64, n: f64) -> Vec<Vec<[f64; 2]>> {
        vec![vec![[w, s], [e, s], [e, n], [w, n], [w, s]]]
    }

    fn folder(path: &str, name: &str, regions: &[&str], folders: Vec<Folder>) -> Folder {
        Folder {
            path: path.into(),
            name: name.into(),
            order: None,
            regions: regions.iter().map(|s| s.to_string()).collect(),
            folders,
        }
    }

    #[test]
    fn a_region_with_its_crumbs_outline_and_bytes() {
        // Two shapes side by side, cut along 10° E: one region's.
        let mut shapes = BTreeMap::new();
        shapes.insert("osm:1".to_string(), vec![square(0.0, 40.0, 10.0, 50.0)]);
        shapes.insert("osm:2".to_string(), vec![square(10.0, 40.0, 20.0, 50.0)]);
        let d = DrawnRegion { id: "a".into(), name: "Alpha".into(), shapes, ..Default::default() };
        let tree = folder(
            "",
            "",
            &["c"],
            vec![folder("europe", "Europe", &[], vec![folder("europe/x", "Some X", &["a"], vec![])])],
        );
        let recipe = |id: &str, name: &str| Recipe { id: id.into(), name: name.into(), outline: vec![] };
        let bytes = BTreeMap::from([("a".to_string(), 1234u64)]);
        let v = make("v1", Some(7), &[recipe("a", "Alpha"), recipe("c", "Gamma")], &[d], &tree, &bytes);
        assert_eq!((v["version"].as_str(), v["catalog"].as_u64()), (Some("v1"), Some(7)));
        let a = &v["regions"][0];
        assert_eq!(a["crumbs"], json!(["Europe", "Some X", "Alpha"]));
        // (A folder without a name of its own: its folder's, in words.)
        let bare = folder("", "", &[], vec![folder("isle-of-man", "isle-of-man", &["a"], vec![])]);
        assert_eq!(crumbs_of(&bare)["a"], ["Isle of Man"]);
        assert_eq!(a["bytes"], 1234);
        assert_eq!(a["fill"].as_array().unwrap().len(), 2);
        // (The cut along 10° E isn't stroked: the edge is the outer border alone, one line round.)
        let edge = a["edge"].as_array().unwrap();
        assert_eq!(edge.len(), 1, "{edge:?}");
        let xs: Vec<f64> = edge[0].as_array().unwrap().iter().step_by(2).map(|x| x.as_f64().unwrap()).collect();
        let mid = (190.0 / 360.0 * 10_000.0_f64).round() / 10.0;
        assert!(!xs.windows(2).any(|w| w[0] == mid && w[1] == mid), "no stretch along the cut: {xs:?}");
        // At the top level, no folder; not in the catalogue, no size; nothing drawn, no outline.
        let c = &v["regions"][1];
        assert_eq!((c["crumbs"].clone(), c["bytes"].clone()), (json!(["Gamma"]), Value::Null));
        assert_eq!((c["fill"].clone(), c["edge"].clone()), (json!([]), json!([])));
    }

    #[test]
    fn outlines_are_simplified_and_specks_dropped() {
        // A ring of 1,000 points on a straight line's sides: four corners kept.
        let mut ring: Vec<[f64; 2]> = (0..=1000).map(|i| [f64::from(i) / 100.0, 0.0]).collect();
        ring.extend([[10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]);
        let mut shapes = BTreeMap::new();
        shapes.insert("x".to_string(), vec![vec![ring], square(30.0, 0.0, 30.000_1, 0.000_1)]);
        let (fill, _) = outline(&DrawnRegion { id: "b".into(), shapes, ..Default::default() });
        assert_eq!(fill.len(), 1, "the speck left out");
        assert_eq!(fill[0].len(), 2 * 5, "{:?}", fill[0]);
    }
}
