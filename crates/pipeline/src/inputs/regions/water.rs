//! Land and water (docs/inputs.md §6.1.3 "Land and water", §6.1.4): what's done once every region's
//! outline is resolved, for the regions with a `water` key, and the rules `rules.toml` turns on.
//!
//! 1. Each region with water has its land (its outline cut back to the coastline's land, the lakes
//!    over 100 km² out) from `resolve`. Its water is resolved next, in an order where a region comes
//!    after those whose `region:` or `group:` it names in its water (they're then whole), and less
//!    the coastline's land and every region without water (the land dominates the sea).
//! 2. The lakes over 100 km² its outline takes in are its water too, except where another region's
//!    water names them.
//! 3. With `cover = "earth"` in `rules.toml`: a region's piece of water under `sliver_km2` (50 km²)
//!    that touches none of its land joins the region it shares the longest edge with; then, tile
//!    by tile, land in two regions is taken from both, and land in none (and sea in none touching
//!    land) joins the region it shares the longest edge with (the nearest when it touches none);
//!    sea in two regions is an error (`rg-cover`), as is sea in none when no region is the rest.
//! 4. The region whose water is `rest` takes what no other covers.
//! 5. With `cover = "earth"`, the Earth is covered once: each tile's regions' areas sum to the
//!    tile's within 0.1 km², or `rg-cover` says where and which regions.

use super::file::Region;
use super::geom::{self, Area, Frame, Pt};
use super::resolve::{Folder, Outcome, Problem, Resolved};
use super::stored::Shape;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// `rules.toml` (at the folder's top): the rules for the whole folder.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// `"earth"`: the regions cover the Earth once (step 3 and 5 above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// A region's piece of water under this (km²) that touches none of its land joins a neighbour.
    #[serde(default = "sliver")]
    pub sliver_km2: f64,
}

fn sliver() -> f64 {
    50.0
}

pub const RULES: &str = "rules.toml";

/// `rules.toml`, parsed.
pub fn parse_rules(text: &str) -> Result<Rules, super::file::Fault> {
    let r: Rules = toml::from_str(text)
        .map_err(|e| super::file::Fault { check: "rules", message: format!("rules.toml: {}", e.message()) })?;
    if r.cover.as_deref().is_some_and(|c| c != "earth") {
        return Err(super::file::Fault { check: "rules", message: "rules.toml: cover is \"earth\" or absent".into() });
    }
    if !(r.sliver_km2 >= 0.0 && r.sliver_km2 <= 10_000.0) {
        return Err(super::file::Fault { check: "rules", message: "rules.toml: sliver_km2 is 0 to 10,000".into() });
    }
    Ok(r)
}

/// `f` (a dataset's area in a plain box) for the box `b` of a region's frame: asked for each turn
/// the box meets, and shifted into the frame.
pub fn in_frame(_frame: &Frame, b: Option<[i64; 4]>, f: impl Fn([i64; 4]) -> Area) -> Area {
    let Some(b) = b else { return Area::default() };
    let k0 = (b[0] + geom::HALF).div_euclid(geom::TURN);
    let k1 = (b[2] + geom::HALF).div_euclid(geom::TURN);
    let mut parts = Vec::new();
    for k in k0..=k1 {
        let (lo, hi) = (-geom::HALF + k * geom::TURN, geom::HALF + k * geom::TURN);
        let (w, e) = (b[0].max(lo), b[2].min(hi));
        if w > e {
            continue;
        }
        let a = f([w - k * geom::TURN, b[1], e - k * geom::TURN, b[3]]);
        parts.push(if k == 0 { a } else { a.shifted(k * geom::TURN) });
    }
    geom::union_all(&parts)
}

fn plain_of(res: &Resolved) -> Area {
    // (Each shape's rings even–odd, shapes unioned.)
    let areas: Vec<Area> = res
        .geometry
        .shapes
        .iter()
        .map(|s| {
            Area::from_rings(
                &s.rings
                    .iter()
                    .map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect::<Vec<Pt>>())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    geom::union_all(&areas)
}

fn meets(a: [i64; 4], b: [i64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// A region's stored geometry and drawing from its final area (plain frame): one shape, its `from`
/// the region's parts as written.
fn store(r: &Region, res: &mut Resolved, a: &Area, land: &Area) {
    res.geometry.water_only = land.is_empty();
    let from = {
        let mut f = r.outline_text().join(", ");
        if !r.intersect.is_empty() {
            f += &format!(" ∩ {}", r.intersect.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
        }
        if !r.minus.is_empty() {
            f += &format!(" − {}", r.minus.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
        }
        f
    };
    let country = res.geometry.shapes.first().map(|s| s.country.clone()).unwrap_or_default();
    let rings32: Vec<Vec<[i32; 2]>> = a.rings().iter().map(|r| super::shapes::to_i32(r)).collect();
    let t = crate::coverage::draw_tolerance_m(&rings32);
    let polys: Vec<Vec<Vec<[f64; 2]>>> = a
        .polys
        .iter()
        .map(|p| {
            p.iter()
                .map(|r| crate::coverage::ring_degrees(&crate::outlines::simplify(&super::shapes::to_i32(r), t)))
                .collect()
        })
        .collect();
    res.geometry.shapes = if rings32.is_empty() {
        Vec::new()
    } else {
        vec![Shape { from: from.clone(), country, tolerance_m: 0, rings: rings32 }]
    };
    res.drawn.shapes = BTreeMap::from([(from, polys)]);
    res.lakes = Area::default();
}

fn problem(check: &'static str, r: &Region, path: &str, message: String) -> Problem {
    Problem { check, about: vec![r.id.clone()], found: Vec::new(), files: vec![path.to_string()], message }
}

/// Steps 1–5 above, on what `resolve` made of every region's outline.
pub fn finish(folder: &Folder, ctx: &super::resolve::Context, out: &mut Outcome) {
    let rules = folder.rules.clone().unwrap_or_default();
    let water: Vec<&String> = folder
        .regions
        .iter()
        .filter(|(id, (_, r))| r.has_water() && out.resolved.contains_key(*id))
        .map(|(id, _)| id)
        .collect();
    let rests: Vec<&String> = water.iter().copied().filter(|id| folder.regions[*id].1.is_rest()).collect();
    if water.is_empty() {
        return;
    }
    let Some(data) = ctx.data else { return };
    if rests.len() > 1 {
        for id in &rests {
            let (p, r) = &folder.regions[*id];
            out.problems.push(problem(
                "rg-rest",
                r,
                p,
                format!(
                    "{id}: rest is in the water of more than one region ({})",
                    rests.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        for id in &rests {
            out.resolved.remove(*id);
        }
        return;
    }
    // (The land of the regions with water, as `resolve` left it; every other region's is its
    // `out.resolved`, which then becomes each one's final geometry in place.)
    let lands: BTreeMap<String, Resolved> = water.iter().map(|id| ((*id).clone(), out.resolved[*id].clone())).collect();
    // Land areas (plain) of every region: a region without water whole.
    let land_of: BTreeMap<String, Area> = out.resolved.iter().map(|(id, res)| (id.clone(), plain_of(res))).collect();
    let boxes: BTreeMap<String, [i64; 4]> =
        land_of.iter().filter_map(|(id, a)| a.bbox().map(|b| (id.clone(), b))).collect();
    let no_water: Vec<&String> =
        land_of.keys().filter(|id| !folder.regions.get(*id).is_some_and(|(_, r)| r.has_water())).collect();
    // The order: each after those its water names whole (region:, group:).
    let wset: BTreeSet<&String> = water.iter().copied().collect();
    let wdeps = |id: &str| -> Vec<String> {
        let r = &folder.regions[id].1;
        let mut v = Vec::new();
        for p in &r.water {
            let mut ps = Vec::new();
            p.walk(&mut ps);
            for q in ps {
                match q {
                    super::file::Part::Region(d) => v.push(d.clone()),
                    super::file::Part::Group(g) => v.extend(folder.members(g, id)),
                    _ => {}
                }
            }
        }
        v.retain(|d| wset.contains(d));
        v.sort();
        v.dedup();
        v
    };

    let mut final_area: BTreeMap<String, Area> = BTreeMap::new();
    let mut waters: BTreeMap<String, Area> = BTreeMap::new();
    let mut left: Vec<&String> = water.iter().copied().filter(|id| !rests.contains(id)).collect();
    let mut held: BTreeSet<String> = BTreeSet::new();
    while !left.is_empty() {
        let (ready, wait): (Vec<&String>, Vec<&String>) =
            left.iter().partition(|id| wdeps(id).iter().all(|d| final_area.contains_key(d) || held.contains(d)));
        if ready.is_empty() {
            for id in &wait {
                let (p, r) = &folder.regions[*id];
                out.problems.push(problem(
                    "rg-loop",
                    r,
                    p,
                    format!("{id}: its water names regions whose water names it back ({})", wdeps(id).join(", ")),
                ));
                held.insert((*id).clone());
            }
            break;
        }
        let made: Vec<(String, Result<Area, Vec<Problem>>)> = ready
            .par_iter()
            .map(|id| {
                let (p, r) = &folder.regions[*id];
                if let Some(d) = wdeps(id).iter().find(|d| held.contains(*d)) {
                    return (
                        (*id).clone(),
                        Err(vec![problem(
                            "rg-ref",
                            r,
                            p,
                            format!("{id}: region:{d} is held, and so this region with it"),
                        )]),
                    );
                }
                let mut needs = BTreeSet::new();
                let res = super::resolve::resolve_one(r, p, folder, ctx, &out.resolved, &lands, true, &mut needs);
                let res = res.map(|w| {
                    // Each piece (one a side of ±180°) less the land and the regions without water.
                    let pieces: Vec<Area> = w
                        .geometry
                        .shapes
                        .iter()
                        .map(|s| {
                            let a = Area::from_rings(
                                &s.rings
                                    .iter()
                                    .map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect())
                                    .collect::<Vec<_>>(),
                            );
                            let mask = |b: [i64; 4]| {
                                let mut cut = vec![data.land_in(b)];
                                let bb = geom::box_area(b);
                                cut.extend(
                                    no_water
                                        .iter()
                                        .filter(|o| boxes.get(**o).is_some_and(|ob| meets(*ob, b)))
                                        .map(|o| land_of[*o].intersect(&bb)),
                                );
                                geom::union_all(&cut)
                            };
                            geom::with_mask(&a, &mask, false)
                        })
                        .collect();
                    geom::union_all(&pieces)
                });
                ((*id).clone(), res)
            })
            .collect();
        for (id, res) in made {
            match res {
                Ok(w) => {
                    let all = land_of[&id].union(&w);
                    waters.insert(id.clone(), w);
                    final_area.insert(id.clone(), all.clone());
                    let f = out.resolved.get_mut(&id).expect("resolved");
                    store(&folder.regions[&id].1, f, &all, &land_of[&id]);
                }
                Err(ps) => {
                    out.problems.extend(ps);
                    held.insert(id);
                }
            }
        }
        left = wait;
    }
    // Hand-outs: a stretch of a region's water that one region of the folder named alone borders
    // goes to it; a stretch under the sliver size, to the one it shares the longest edge with.
    // (Each region's box now, land and water: what a stretch is tested against first.)
    let hboxes: BTreeMap<String, [i64; 4]> = land_of
        .iter()
        .map(|(id, a)| (id, final_area.get(id).unwrap_or(a)))
        .filter_map(|(id, a)| a.bbox().map(|b| (id.clone(), b)))
        .collect();
    for id in water.iter().copied().filter(|id| folder.regions[*id].1.hand_out.is_some()) {
        let Some(w) = waters.get(id).cloned() else { continue };
        let g = folder.regions[id].1.hand_out.clone().unwrap();
        let members: Vec<String> = folder
            .members(&g, id)
            .into_iter()
            .filter(|m| final_area.contains_key(m) || land_of.contains_key(m))
            .collect();
        let area_of =
            |m: &String, fa: &BTreeMap<String, Area>| fa.get(m).cloned().unwrap_or_else(|| land_of[m].clone());
        let moves: Vec<(String, Area)> = w
            .polys
            .par_iter()
            .filter_map(|poly| {
                let piece = Area { polys: vec![poly.clone()] };
                let b = piece.bbox()?;
                let pad = [b[0] - 10_000, b[1] - 10_000, b[2] + 10_000, b[3] + 10_000];
                let touching: Vec<(f64, &String)> = members
                    .iter()
                    .filter(|m| hboxes.get(*m).is_some_and(|mb| meets(*mb, pad)))
                    .map(|m| (geom::shared_edge_km(&piece, &area_of(m, &final_area), 100.0), m))
                    .filter(|(l, _)| *l > 0.0)
                    .collect();
                if touching.len() == 1 {
                    return Some((touching[0].1.clone(), piece));
                }
                if geom::area_km2(&piece) < rules.sliver_km2 {
                    return touching
                        .iter()
                        .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(x.1)))
                        .map(|(_, m)| ((*m).clone(), piece.clone()));
                }
                None
            })
            .collect();
        for (m, piece) in moves {
            let left = waters[id].minus(&piece);
            waters.insert(id.clone(), left.clone());
            final_area.insert(id.clone(), land_of[id].union(&left));
            let mw = waters.get(&m).cloned().unwrap_or_default().union(&piece);
            waters.insert(m.clone(), mw);
            let mf = area_of(&m, &final_area).union(&piece);
            final_area.insert(m, mf);
        }
    }
    // Lakes: each region's own unless another's water names them.
    let wboxes: Vec<(&String, [i64; 4])> = waters.iter().filter_map(|(id, a)| a.bbox().map(|b| (id, b))).collect();
    let lake_add: Vec<(String, Area)> = final_area
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .par_iter()
        .filter_map(|id| {
            let lk = &lands[id].lakes;
            let b = lk.bbox()?;
            let others: Vec<Area> =
                wboxes.iter().filter(|(o, ob)| *o != id && meets(*ob, b)).map(|(o, _)| waters[*o].clone()).collect();
            let mine = if others.is_empty() { lk.clone() } else { lk.minus(&geom::union_all(&others)) };
            (!mine.is_empty()).then(|| (id.clone(), mine))
        })
        .collect();
    for (id, lk) in lake_add {
        let a = final_area[&id].union(&lk);
        final_area.insert(id, a);
    }
    for id in held.iter().chain(rests.iter().copied()) {
        out.resolved.remove(id);
    }
    // Every region's area now (plain): those without water as they are.
    let mut areas: BTreeMap<String, Area> = land_of.clone();
    for (id, a) in &final_area {
        areas.insert(id.clone(), a.clone());
    }
    for id in held.iter().chain(rests.iter().copied()) {
        areas.remove(id);
    }
    let cover = rules.cover.as_deref() == Some("earth");
    if cover {
        slivers(folder, &lands, &land_of, rules.sliver_km2, &mut areas);
    }
    let rest_id = rests.first().map(|s| (*s).clone());
    let (fills, rest, findings) = tiles(folder, data, &areas, cover, rest_id.is_some());
    for (id, (add, take)) in fills {
        let a = areas[&id].minus(&take).union(&add);
        areas.insert(id, a);
    }
    out.problems.extend(findings);
    // Store the regions with water (and any a fill changed).
    for (id, a) in &areas {
        let Some((_, r)) = folder.regions.get(id) else { continue };
        if !r.has_water() && land_of.get(id) == Some(a) {
            continue;
        }
        if a.is_empty() {
            let (p, _) = &folder.regions[id];
            out.problems.push(problem("rg-empty", r, p, format!("{id}: the region has neither land nor water")));
            out.resolved.remove(id);
            continue;
        }
        if let Some(f) = out.resolved.get_mut(id) {
            store(r, f, a, &land_of[id]);
        }
    }
    if let Some(id) = rest_id {
        let (p, r) = &folder.regions[&id];
        let mut f = lands[&id].clone();
        let all = land_of[&id].union(&rest);
        if all.is_empty() {
            out.problems.push(problem(
                "rg-empty",
                r,
                p,
                format!("{id}: the rest is empty: the other regions cover the Earth"),
            ));
        } else {
            store(r, &mut f, &all, &land_of[&id]);
            out.resolved.insert(id, f);
        }
    }
}

/// Step 3's slivers: a region's pieces of water under `km2` touching none of its land go to the
/// neighbour sharing the longest edge.
fn slivers(
    folder: &Folder,
    lands: &BTreeMap<String, Resolved>,
    land_of: &BTreeMap<String, Area>,
    km2: f64,
    areas: &mut BTreeMap<String, Area>,
) {
    if km2 <= 0.0 {
        return;
    }
    let ids: Vec<String> = areas
        .keys()
        .filter(|id| folder.regions.get(*id).is_some_and(|(_, r)| r.has_water() && !r.is_rest()))
        .cloned()
        .collect();
    let _ = lands;
    let mut moves: Vec<(String, String, Area)> = Vec::new();
    for id in &ids {
        let a = &areas[id];
        let land = &land_of[id];
        if a.polys.len() < 2 {
            continue;
        }
        for poly in &a.polys {
            let piece = Area { polys: vec![poly.clone()] };
            if geom::area_km2(&piece) >= km2
                || !piece.intersect(land).is_empty()
                || geom::shared_edge_km(&piece, land, 100.0) > 0.0
            {
                continue;
            }
            let Some(b) = piece.bbox() else { continue };
            let best = areas
                .iter()
                .filter(|(o, oa)| {
                    *o != id
                        && oa
                            .bbox()
                            .is_some_and(|ob| meets(ob, [b[0] - 10_000, b[1] - 10_000, b[2] + 10_000, b[3] + 10_000]))
                })
                .map(|(o, oa)| (geom::shared_edge_km(&piece, oa, 100.0), o.clone()))
                .filter(|(l, _)| *l > 0.0)
                .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)));
            if let Some((_, o)) = best {
                moves.push((id.clone(), o, piece));
            }
        }
    }
    for (from, to, piece) in moves {
        let a = areas[&from].minus(&piece);
        areas.insert(from, a);
        let b = areas[&to].union(&piece);
        areas.insert(to, b);
    }
}

/// Tile (x, y) of the 64 × 32 the Earth is halved into: its box.
fn tile_box(x: i64, y: i64) -> [i64; 4] {
    let (w, h) = (geom::TURN / 64, 1_800_000_000 / 32);
    [-geom::HALF + x * w, -900_000_000 + y * h, -geom::HALF + (x + 1) * w, -900_000_000 + (y + 1) * h]
}

/// An area's pieces in each tile it meets, cut by halving the Earth's box six times.
fn tile_pieces(a: &Area) -> Vec<((i64, i64), Area)> {
    fn rec(a: &Area, x0: i64, y0: i64, n: i64, out: &mut Vec<((i64, i64), Area)>) {
        if a.is_empty() {
            return;
        }
        if n == 1 {
            out.push(((x0, y0), a.clone()));
            return;
        }
        let h = n / 2;
        for (dx, dy) in [(0, 0), (h, 0), (0, h), (h, h)] {
            let (b0, b1) = (tile_box(x0 + dx, y0 + dy), tile_box(x0 + dx + h - 1, y0 + dy + h - 1));
            let q = [b0[0], b0[1], b1[2], b1[3]];
            if a.bbox().is_some_and(|b| meets(b, q)) {
                rec(&a.intersect(&geom::box_area(q)), x0 + dx, y0 + dy, h, out);
            }
        }
    }
    // (The Earth's 64 × 32 tiles as two squares of 32 × 32.)
    let mut out = Vec::new();
    for x0 in [0, 32] {
        let b = [tile_box(x0, 0)[0], -900_000_000, tile_box(x0 + 31, 0)[2], 900_000_000];
        if a.bbox().is_some_and(|ab| meets(ab, b)) {
            rec(&a.intersect(&geom::box_area(b)), x0, 0, 32, &mut out);
        }
    }
    out
}

/// What the tiles give: each region's area added and taken away, the rest, and the findings.
type Tiles = (BTreeMap<String, (Area, Area)>, Area, Vec<Problem>);

/// Steps 3–5 tile by tile (5.625°): with `cover`, the fills and the check; the rest when there's a
/// region for it.
fn tiles(
    folder: &Folder,
    data: &super::data::Datasets,
    areas: &BTreeMap<String, Area>,
    cover: bool,
    rest: bool,
) -> Tiles {
    if !cover && !rest {
        return (BTreeMap::new(), Area::default(), Vec::new());
    }
    let boxes: Vec<(&String, [i64; 4])> = areas.iter().filter_map(|(id, a)| a.bbox().map(|b| (id, b))).collect();
    // Each region cut into the tiles (360/64° by 180/32°) by halving: no clip of a whole region by
    // each tile it meets.
    let mut by_tile: BTreeMap<(i64, i64), Vec<(String, Area)>> = BTreeMap::new();
    let cut: Vec<(String, Vec<((i64, i64), Area)>)> =
        areas.iter().collect::<Vec<_>>().par_iter().map(|(id, a)| ((*id).clone(), tile_pieces(a))).collect();
    for (id, ps) in cut {
        for (k, a) in ps {
            by_tile.entry(k).or_default().push((id.clone(), a));
        }
    }
    let tiles: Vec<((i64, i64), [i64; 4])> =
        (0..64).flat_map(|x| (0..32).map(move |y| ((x, y), tile_box(x, y)))).collect();
    type One = (Vec<(String, Area, Area)>, Area, Vec<Problem>);
    let done: Vec<One> = tiles
        .par_iter()
        .map(|(k, t)| {
            let ta = geom::box_area(*t);
            let mut pieces: Vec<(String, Area)> = by_tile.get(k).cloned().unwrap_or_default();
            let mut changes: Vec<(String, Area, Area)> = Vec::new();
            let mut problems = Vec::new();
            if !cover {
                let u = geom::union_all(&pieces.iter().map(|p| p.1.clone()).collect::<Vec<_>>());
                return (changes, ta.minus(&u), problems);
            }
            let land = data.land_in(*t).intersect(&ta).minus(&data.big_lakes_in(*t));
            // Overlaps.
            let mut land_twice = Vec::new();
            for i in 0..pieces.len() {
                for j in i + 1..pieces.len() {
                    let (bi, bj) = (pieces[i].1.bbox(), pieces[j].1.bbox());
                    if !matches!((bi, bj), (Some(x), Some(y)) if meets(x, y)) {
                        continue;
                    }
                    let o = pieces[i].1.intersect(&pieces[j].1);
                    if o.is_empty() {
                        continue;
                    }
                    let on_land = o.intersect(&land);
                    let at_sea = o.minus(&land);
                    let km2 = geom::area_km2(&at_sea);
                    if km2 > 0.1 {
                        let (a, b) = (&pieces[i].0, &pieces[j].0);
                        let files: Vec<String> = [a, b].iter().filter_map(|x| folder.regions.get(*x).map(|r| r.0.clone())).collect();
                        problems.push(Problem {
                            check: "rg-cover",
                            about: vec![a.clone(), b.clone()],
                            found: vec!["overlap".into()],
                            files,
                            message: format!("{a} and {b} both cover {km2:.1} km² of water near {:.0}°, {:.0}° (in the tile from {:.0}°, {:.0}°): water is in one region", (at_sea.bbox().unwrap()[0] as f64) * 1e-7, (at_sea.bbox().unwrap()[1] as f64) * 1e-7, t[0] as f64 * 1e-7, t[1] as f64 * 1e-7),
                        });
                    }
                    if !on_land.is_empty() {
                        land_twice.push(on_land);
                    }
                }
            }
            let twice = geom::union_all(&land_twice);
            if !twice.is_empty() {
                for p in pieces.iter_mut() {
                    let less = p.1.minus(&twice);
                    if less != p.1 {
                        changes.push((p.0.clone(), Area::default(), p.1.intersect(&twice)));
                        p.1 = less;
                    }
                }
            }
            let u = geom::union_all(&pieces.iter().map(|p| p.1.clone()).collect::<Vec<_>>());
            let gaps = ta.minus(&u);
            let mut sea_left = Vec::new();
            for poly in &gaps.polys {
                let g = Area { polys: vec![poly.clone()] };
                let touches_land = !g.intersect(&land).is_empty() || geom::shared_edge_km(&g, &land, 100.0) > 0.0;
                if !touches_land {
                    sea_left.push(g);
                    continue;
                }
                let best = pieces
                    .iter()
                    .map(|(id, a)| (geom::shared_edge_km(&g, a, 100.0), id))
                    .filter(|(l, _)| *l > 0.0)
                    .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(x.1)))
                    .map(|(_, id)| id.clone())
                    .or_else(|| {
                        // (The nearest by the boxes' middles.)
                        let c = g.bbox()?;
                        let m = [(c[0] + c[2]) / 2, (c[1] + c[3]) / 2];
                        boxes
                            .iter()
                            .min_by_key(|(id, b)| {
                                let d = [(b[0] + b[2]) / 2 - m[0], (b[1] + b[3]) / 2 - m[1]];
                                ((d[0] as i128).pow(2) + (d[1] as i128).pow(2), (*id).clone())
                            })
                            .map(|(id, _)| (*id).clone())
                    });
                if let Some(id) = best {
                    changes.push((id.clone(), g.clone(), Area::default()));
                    if let Some(p) = pieces.iter_mut().find(|p| p.0 == id) {
                        p.1 = p.1.union(&g);
                    } else {
                        pieces.push((id, g));
                    }
                }
            }
            let sea = geom::union_all(&sea_left);
            if !rest {
                let km2 = geom::area_km2(&sea);
                if km2 > 0.1 {
                    problems.push(Problem {
                        check: "rg-cover",
                        about: vec![format!("{},{}", t[0] / 10_000_000, t[1] / 10_000_000)],
                        found: vec!["gap".into()],
                        files: vec![RULES.into()],
                        message: format!("{km2:.1} km² of water in the tile from {:.0}°, {:.0}° is in no region, and no region's water is the rest", t[0] as f64 * 1e-7, t[1] as f64 * 1e-7),
                    });
                }
            }
            (changes, sea, problems)
        })
        .collect();
    let mut fills: BTreeMap<String, (Vec<Area>, Vec<Area>)> = BTreeMap::new();
    let mut rest_parts = Vec::new();
    let mut problems = Vec::new();
    for (changes, r, ps) in done {
        for (id, add, take) in changes {
            let e = fills.entry(id).or_default();
            e.0.push(add);
            e.1.push(take);
        }
        rest_parts.push(r);
        problems.extend(ps);
    }
    let fills = fills.into_iter().map(|(id, (a, t))| (id, (geom::union_all(&a), geom::union_all(&t)))).collect();
    (fills, geom::union_all(&rest_parts), problems)
}
