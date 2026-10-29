//! Terrain targets whose key moved with no change to what they sample, told and restored (task
//! #832). A terrain-hi z6 tile's and a terrain-deep z10 tile's keys name the coverage within 20 km
//! (its fingerprint) and the elevation datasets meeting it with their regions' outlines: a regions
//! change that moves geometry anywhere near a tile re-queues it, though its run may read the very
//! same samples. A target recorded under the key it has under an older version of the inputs (a
//! generation: a manifest naming that version's regions and elevation indexes and water digests),
//! whose run would sample exactly what it samples under the current one, makes the same bytes: it
//! may be recorded under its key now, without a rebuild.
//!
//! What a run samples, and how it's compared, by the code that samples (never a key's text):
//! - the tiles it makes: a z6 tile's levels (crate::terrain_pack::piece_levels: the coverage near
//!   each tile), the deep levels it keeps (crate::terrain_deep::deeper, `short` z8 tiles meeting a
//!   fine dataset near the coverage); a z10 tile's base tiles and the zooms they reach
//!   (crate::terrain_deep::zooms), its edge ring, and each z6 tile its base is made of, as above;
//! - the water it reads (the water digest of its tiles, crate::terrain_water: the same digest is
//!   the same water at every sample);
//! - the datasets meeting it in order, each the same declaration, measurement and conversion but
//!   for its coverage; and, where a coverage differs (its cells, or its regions' outlines), every
//!   sample point the runs read inside the area (crate::terrain_base::points at z9–12,
//!   crate::terrain_deep::points and `presence_points` at z9–16, a superset of what's read), held
//!   alike (crate::dem::declared::Cover::contains). Boxes where both coverages give one answer for
//!   every point (`Cover::contains_box`, or the same cells and outlines there by their exact
//!   fingerprints, `OutlineCut::within`) are passed whole; only the rest is asked point by point.
//!
//! Read only: `scenic-build terrain-restore-check` prints what it would restore.

use super::build::{self, Keys, TerrainTargets};
use super::tiles::TerrainTiles;
use crate::coverage::Coverage;
use crate::dem::declared::{self, Cover};
use crate::legacy::TileId;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A version of the inputs the terrain keys read: a manifest's.
pub struct Gen {
    /// What it's called in the report (a term's number, "now").
    pub label: String,
    pub m: BTreeMap<String, String>,
    pub cov: Coverage,
    pub tiles: TerrainTiles,
    pub tt: TerrainTargets,
    /// The elevation datasets in their order, each with its coverage as sampled (its regions'
    /// outlines read).
    pub ds: Vec<(declared::Accepted, Cover)>,
    hi: HashMap<String, String>,
    deep: HashMap<String, String>,
}

impl Gen {
    /// The inputs manifest `m` names under `root`, its terrain targets worked out.
    pub fn read(root: &store::nasfs::NasRoot, label: &str, m: BTreeMap<String, String>) -> Result<Gen> {
        let a = crate::inputs::open(root, &m, crate::inputs::regions::UNIT)?.context("no regions version")?;
        let cov = Coverage::accepted(root, &a, None).with_context(|| format!("{label}: the coverage"))?;
        let mut tiles = TerrainTiles::new(None);
        tiles.load_inputs(root, &m);
        let e = crate::inputs::open(root, &m, crate::inputs::elevation::UNIT)?.context("no elevation version")?;
        let mut ds = declared::accepted(&e)?;
        if let Some(s) = crate::inputs::sources::open(root, &m)? {
            let _ = declared::convert(&mut ds, s.many(crate::inputs::sources::GEOID));
        }
        let read = |n: &str| -> Result<Vec<u8>> { Ok(root.join(n).read()?) };
        let ds = ds
            .into_iter()
            .map(|d| -> Result<(declared::Accepted, Cover)> {
                let outline = match &d.measured.outline {
                    Some(o) => Some(std::sync::Arc::new(declared::OutlineCut::read(o, &read)?)),
                    None => None,
                };
                let c = Cover { outline, ..Cover::of(d.measured.cover.as_deref()) };
                Ok((d, c))
            })
            .collect::<Result<_>>()?;
        let tt = build::terrain_slope_targets_made(&cov, &m, &tiles);
        let hi = tt.terrain.iter().cloned().collect();
        let deep = tt.terrain_deep.iter().cloned().collect();
        Ok(Gen { label: label.into(), m, cov, tiles, tt, ds, hi, deep })
    }

    fn fine(&self) -> Result<std::sync::Arc<Vec<crate::terrain_deep::FineSet>>, String> {
        self.tiles.fine(&self.m).ok_or_else(|| format!("{}: its fine datasets can't be read", self.label))
    }

    /// The water z6 tile or assembly `t` reads (`build::water_key`), from `from`.
    fn water(&self, t: &str, from: &str) -> Option<String> {
        let pin = crate::terrain_pack::water_source_pin(&self.m);
        let idx = pin.as_deref().and_then(|p| self.tiles.water_idx(&self.m, p));
        build::water_key(pin.as_deref(), idx, t, from)
    }
}

/// What the check says of one target.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Its record is its key now.
    Current,
    /// Recorded under its key in generation `from`, and its run samples the same there and now:
    /// its key now may be recorded.
    Restore { from: String, key: String },
    /// Recorded under its key in generation `from`, but something it samples differs (why).
    Differs { from: String, why: String },
    /// Its record is no generation's key for it: stale for another reason.
    Other,
    /// No record.
    Unrecorded,
}

/// The check of every terrain-hi and terrain-deep target of `now`: (step, target, verdict).
pub fn check(root: &store::nasfs::NasRoot, keys: &Keys, old: &[Gen], now: &Gen) -> Vec<(String, String, Verdict)> {
    let mut out = Vec::new();
    let mut pieces: HashMap<(usize, (u32, u32)), Result<(), String>> = HashMap::new();
    let mut piece = |g: usize, t: (u32, u32)| -> Result<(), String> {
        pieces.entry((g, t)).or_insert_with(|| piece_same(root, &old[g], now, t)).clone()
    };
    for (t, k) in &now.tt.terrain {
        let v = match keys.terrain_hi.get(t) {
            None => Verdict::Unrecorded,
            Some(r) if r == k => Verdict::Current,
            Some(r) => match old.iter().position(|g| g.hi.get(t) == Some(r)) {
                None => Verdict::Other,
                Some(g) => {
                    let u = TileId::parse(t).expect("a z6 tile");
                    match piece(g, (u.x, u.y)) {
                        Ok(()) if k != build::UNKNOWN => {
                            Verdict::Restore { from: old[g].label.clone(), key: k.clone() }
                        }
                        Ok(()) => Verdict::Differs { from: old[g].label.clone(), why: "its key now is unknown".into() },
                        Err(why) => Verdict::Differs { from: old[g].label.clone(), why },
                    }
                }
            },
        };
        out.push(("terrain-hi".to_string(), t.clone(), v));
    }
    for (t, k) in &now.tt.terrain_deep {
        let v = match keys.terrain_deep.get(t) {
            None => Verdict::Unrecorded,
            Some(r) if r == k => Verdict::Current,
            Some(r) => match old.iter().position(|g| g.deep.get(t) == Some(r)) {
                None => Verdict::Other,
                Some(g) => {
                    let u = TileId::parse(t).expect("a z10 tile");
                    let r = deep_same(&old[g], now, (u.x, u.y)).and_then(|bases| {
                        bases.into_iter().try_for_each(|p| {
                            piece(g, p).map_err(|w| format!("its base's z6 tile 6/{}/{}: {w}", p.0, p.1))
                        })
                    });
                    match r {
                        Ok(()) if k != build::UNKNOWN => {
                            Verdict::Restore { from: old[g].label.clone(), key: k.clone() }
                        }
                        Ok(()) => Verdict::Differs { from: old[g].label.clone(), why: "its key now is unknown".into() },
                        Err(why) => Verdict::Differs { from: old[g].label.clone(), why },
                    }
                }
            },
        };
        out.push(("terrain-deep".to_string(), t.clone(), v));
    }
    out
}

/// Whether terrain-hi z6 tile `t` samples the same in `o` and `n`: its tiles, its water, its
/// datasets and their coverage at its samples, its deep levels kept. Err: what differs.
pub fn piece_same(_root: &store::nasfs::NasRoot, o: &Gen, n: &Gen, t: (u32, u32)) -> Result<(), String> {
    let (lo, ln) = (crate::terrain_pack::piece_levels(&o.cov, t), crate::terrain_pack::piece_levels(&n.cov, t));
    if lo != ln {
        let d: Vec<String> = lo
            .iter()
            .zip(&ln)
            .filter(|(a, b)| a != b)
            .map(|(a, b)| format!("z{} {} → {} tiles", a.0, a.1.len(), b.1.len()))
            .collect();
        return Err(format!("the tiles it makes (the coverage within 20 km): {}", d.join(", ")));
    }
    let ts = format!("6/{}/{}", t.0, t.1);
    let fp = |g: &Gen| g.cov.fingerprint(build::grown_e7(6, t.0, t.1, 20.0));
    let (wo, wn) = (o.water(&ts, &build::piece_water_from(&fp(o))), n.water(&ts, &build::piece_water_from(&fp(n))));
    match (&wo, &wn) {
        (Some(a), Some(b)) if a == b => {}
        _ => {
            return Err(format!(
                "the water it reads: {} → {}",
                wo.as_deref().unwrap_or("unknown"),
                wn.as_deref().unwrap_or("unknown")
            ))
        }
    }
    // (Its deep levels kept: the z10 tiles rounded coarser, the short z8 tiles it makes deep
    // levels of.)
    let (fo, fnn) = (o.fine()?, n.fine()?);
    if crate::terrain_deep::deeper(&fo, t) != crate::terrain_deep::deeper(&fnn, t) {
        return Err("the z10 tiles it rounds as coarser than native".into());
    }
    let k = crate::terrain_deep::SUB_Z - 6;
    for x in t.0 << k..(t.0 + 1) << k {
        for y in t.1 << k..(t.1 + 1) << k {
            if !crate::terrain_deep::short((x, y)) {
                continue;
            }
            let has = |sets: &[crate::terrain_deep::FineSet], cov: &Coverage| {
                !crate::terrain_deep::key_lines(sets, crate::terrain_deep::SUB_Z, x, y).is_empty()
                    && crate::terrain_pack::near_coverage(cov, crate::terrain_deep::SUB_Z, x, y, 20.0)
            };
            if has(&fo, &o.cov) != has(&fnn, &n.cov) {
                return Err(format!("whether it makes z8 tile 8/{x}/{y}'s deep levels"));
            }
        }
    }
    let area = crate::stage::tile_box_grown(6, t.0, t.1, 0.0);
    datasets_same(o, n, area, &|z| (9..=16).contains(&z))
}

/// Whether terrain-deep z10 tile `t` samples the same in `o` and `n` but for its base: its base
/// tiles and their zooms, its ring, its datasets and their coverage at its samples. Ok: the z6
/// tiles its base is made of (each to be `piece_same`).
pub fn deep_same(o: &Gen, n: &Gen, t: (u32, u32)) -> Result<Vec<(u32, u32)>, String> {
    let (fo, fnn) = (o.fine()?, n.fine()?);
    if crate::terrain_deep::zooms(&fo, t) != crate::terrain_deep::zooms(&fnn, t) {
        return Err("its base tiles a fine dataset is finer than, or the zooms they reach".into());
    }
    let z = crate::terrain_deep::PACK_Z;
    let nn = 1i64 << z;
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = ((t.0 as i64 + dx).rem_euclid(nn) as u32, t.1 as i64 + dy);
            if (0..nn).contains(&y)
                && crate::terrain_deep::zoom_of(&fo, z, x, y as u32)
                    != crate::terrain_deep::zoom_of(&fnn, z, x, y as u32)
            {
                return Err(format!("the zoom of its neighbour {z}/{x}/{y}"));
            }
        }
    }
    // (Its area: it and its ring, what its base and its padded edge read.)
    let b0 = crate::stage::tile_box_grown(z, t.0.saturating_sub(1), (t.1 + 1).min(nn as u32 - 1), 0.0);
    let b1 = crate::stage::tile_box_grown(z, (t.0 + 1).min(nn as u32 - 1), t.1.saturating_sub(1), 0.0);
    let area = [b0[0].min(b1[0]), b0[1].min(b1[1]), b0[2].max(b1[2]), b0[3].max(b1[3])];
    datasets_same(o, n, area, &|z| (9..=16).contains(&z))?;
    let bases: BTreeSet<(u32, u32)> =
        crate::terrain_deep::base_want(t).iter().map(|&(z, x, y)| (x >> (z - 6), y >> (z - 6))).collect();
    let piece = |g: &Gen, p: &(u32, u32)| g.hi.contains_key(&format!("6/{}/{}", p.0, p.1));
    if let Some(p) = bases.iter().find(|p| piece(o, p) != piece(n, p)) {
        return Err(format!("whether its base's z6 tile 6/{}/{} is near the coverage", p.0, p.1));
    }
    Ok(bases.into_iter().filter(|p| piece(o, p)).collect())
}

/// Whether the datasets meeting `area` (as a plan takes them: grown 0.05°) are the same in `o` and
/// `n`, in order, but for their coverage, and their coverage holds alike every sample point of the
/// zooms `zs` inside `area`.
fn datasets_same(o: &Gen, n: &Gen, area: [f64; 4], zs: &dyn Fn(u8) -> bool) -> Result<(), String> {
    let g = 0.05;
    let near = [area[0] - g, area[1] - g, area[2] + g, area[3] + g];
    let meet = |gn: &'_ Gen| -> Vec<usize> {
        gn.ds
            .iter()
            .enumerate()
            .filter(|(_, (d, _))| Cover::of(d.measured.cover.as_deref()).meets(near))
            .map(|(i, _)| i)
            .collect()
    };
    let (mo, mn) = (meet(o), meet(n));
    let ids = |g: &Gen, m: &[usize]| m.iter().map(|&i| g.ds[i].0.id.clone()).collect::<Vec<_>>();
    if ids(o, &mo) != ids(n, &mn) {
        return Err(format!("the datasets meeting it: {:?} → {:?}", ids(o, &mo), ids(n, &mn)));
    }
    for (&i, &j) in mo.iter().zip(&mn) {
        let ((a, ca), (b, cb)) = (&o.ds[i], &n.ds[j]);
        if bare(a) != bare(b) {
            return Err(format!("{}: its declaration, measurement or conversion", a.id));
        }
        if a.measured.cover == b.measured.cover && ca.outline == cb.outline {
            continue;
        }
        if let Some(p) = first_apart(ca, cb, area, zs) {
            return Err(format!(
                "{}: its coverage (its regions' outlines) holds point {:.7}, {:.7} {} now",
                a.id,
                p.0,
                p.1,
                if cb.contains(p.0, p.1) { "and didn't" } else { "no longer" }
            ));
        }
    }
    Ok(())
}

/// A dataset as sampled, but for its coverage: its declaration, its measurement less its cover
/// and outline, its conversion.
fn bare(d: &declared::Accepted) -> serde_json::Value {
    let mut m = serde_json::to_value(&d.measured).unwrap_or_default();
    if let Some(o) = m.as_object_mut() {
        o.remove("cover");
        o.remove("outline");
    }
    serde_json::json!({
        "declaration": serde_json::to_value(&d.declaration).unwrap_or_default(),
        "measured": m,
        "convert": serde_json::to_value(&d.convert).unwrap_or_default(),
    })
}

/// The first sample point inside `area` of the zooms `zs` that `a` and `b` hold differently: the
/// area split until each part is one both hold alike (`alike`), the parts left under ~10 m
/// asked at every sample point inside them.
pub fn first_apart(a: &Cover, b: &Cover, area: [f64; 4], zs: &dyn Fn(u8) -> bool) -> Option<(f64, f64)> {
    let mut leaves: Vec<[f64; 4]> = Vec::new();
    let mut stack = vec![area];
    while let Some(bx) = stack.pop() {
        if alike(a, b, bx) {
            continue;
        }
        let (w, h) = (bx[2] - bx[0], bx[3] - bx[1]);
        if w.max(h) < 1e-4 {
            leaves.push(bx);
            // (Too many: a difference along a long line; the points tell soon enough.)
            if leaves.len() > 200_000 {
                break;
            }
            continue;
        }
        if w >= h {
            let m = (bx[0] + bx[2]) / 2.0;
            stack.push([bx[0], bx[1], m, bx[3]]);
            stack.push([m, bx[1], bx[2], bx[3]]);
        } else {
            let m = (bx[1] + bx[3]) / 2.0;
            stack.push([bx[0], bx[1], bx[2], m]);
            stack.push([bx[0], m, bx[2], bx[3]]);
        }
    }
    if leaves.is_empty() {
        return None;
    }
    // (Each tile any leaf meets, at each zoom and lattice, its points made once.)
    type Lattice = fn(u8, u32, u32) -> (Vec<f64>, Vec<f64>);
    let lattices: [(Lattice, &dyn Fn(u8) -> bool); 3] = [
        (|z, x, y| crate::terrain_base::points(z, &[(x, y)]), &|z| (9..=12).contains(&z)),
        (|z, x, y| crate::terrain_deep::points(z, &[(x, y)]), &|z| z >= 9),
        (|z, x, y| crate::terrain_deep::presence_points((z, x, y)), &|z| z >= 9),
    ];
    for z in 0..=crate::terrain_deep::MAX_Z {
        if !zs(z) {
            continue;
        }
        let mut by_tile: BTreeMap<(u32, u32), Vec<[f64; 4]>> = BTreeMap::new();
        for l in &leaves {
            for t in tiles_meeting(z, *l) {
                by_tile.entry(t).or_default().push(*l);
            }
        }
        for (lat, on) in &lattices {
            if !on(z) {
                continue;
            }
            for (&(x, y), ls) in &by_tile {
                let (lon, la) = lat(z, x, y);
                for (p, q) in lon.iter().zip(&la) {
                    if ls.iter().any(|l| *p >= l[0] && *p <= l[2] && *q >= l[1] && *q <= l[3])
                        && a.contains(*p, *q) != b.contains(*p, *q)
                    {
                        return Some((*p, *q));
                    }
                }
            }
        }
    }
    None
}

/// The tiles of zoom `z` meeting box `b` (w, s, e, n, degrees).
fn tiles_meeting(z: u8, b: [f64; 4]) -> Vec<(u32, u32)> {
    let n = 1u32 << z;
    let fx = |lon: f64| (((lon + 180.0) / 360.0 * n as f64).floor().max(0.0) as u32).min(n - 1);
    let fy = |lat: f64| {
        let r = lat.to_radians();
        let y = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n as f64;
        (y.floor().max(0.0) as u32).min(n - 1)
    };
    let (x0, x1, y0, y1) = (fx(b[0]), fx(b[2]), fy(b[3]), fy(b[1]));
    (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y))).collect()
}

/// Whether `a` and `b` hold every point of box `bx` alike: both one answer for all of it
/// (`contains_box`), or the same cells there and, in the edge cells, the same outlines inside
/// (their fingerprints, `OutlineCut::within`).
fn alike(a: &Cover, b: &Cover, bx: [f64; 4]) -> bool {
    if a.world && b.world {
        return true;
    }
    match (a.contains_box(bx), b.contains_box(bx)) {
        (Some(x), Some(y)) if x == y => return true,
        (Some(_), Some(_)) => return false,
        _ => {}
    }
    if a.world != b.world {
        return false;
    }
    let (x0, x1) = (bx[0].floor() as i32 - 1, bx[2].floor() as i32);
    let (y0, y1) = (bx[1].floor() as i32 - 1, bx[3].floor() as i32);
    if ((x1 - x0 + 1) as i64) * ((y1 - y0 + 1) as i64) > 64 {
        return false;
    }
    let e7 = |v: f64| (v * 1e7).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    for x in x0..=x1 {
        for y in y0..=y1 {
            let (ha, hb) = (a.cells.contains(&(x, y)), b.cells.contains(&(x, y)));
            if ha != hb {
                return false;
            }
            if !ha {
                continue;
            }
            let ea = a.outline.as_ref().is_some_and(|o| o.edges.contains(&(x, y)));
            let eb = b.outline.as_ref().is_some_and(|o| o.edges.contains(&(x, y)));
            if ea != eb {
                return false;
            }
            if !ea {
                continue;
            }
            // (The box cut to the cell, as the points in it round.)
            let r = [
                e7(bx[0].max(x as f64)),
                e7(bx[1].max(y as f64)),
                e7(bx[2].min(x as f64 + 1.0)),
                e7(bx[3].min(y as f64 + 1.0)),
            ];
            if r[0] > r[2] || r[1] > r[3] {
                continue;
            }
            let (wa, wb) = (a.outline.as_ref().unwrap().within(r), b.outline.as_ref().unwrap().within(r));
            if wa != wb {
                return false;
            }
        }
    }
    true
}

/// The restores of `step` the check `v` found, each with the key recorded now in `keys` (its
/// `from`): what `scenic redo <step> --restore` asks the lead.
pub fn restores(v: &[(String, String, Verdict)], keys: &Keys, step: &str) -> Vec<super::redo::Restore> {
    v.iter()
        .filter(|x| x.0 == step)
        .filter_map(|(s, t, v)| match v {
            Verdict::Restore { key, .. } => Some(super::redo::Restore {
                target: t.clone(),
                from: keys.recorded(s, t)?.to_string(),
                to: key.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The check's counts and samples, as `terrain-restore-check` prints them.
pub fn report(v: &[(String, String, Verdict)], sample: usize) -> Vec<String> {
    let mut out = Vec::new();
    for step in ["terrain-hi", "terrain-deep"] {
        let of: Vec<&(String, String, Verdict)> = v.iter().filter(|x| x.0 == step).collect();
        let n = |f: &dyn Fn(&Verdict) -> bool| of.iter().filter(|x| f(&x.2)).count();
        out.push(format!(
            "{step}: {} targets: {} current, {} to restore, {} differ (stay queued), {} stale otherwise, {} unrecorded",
            of.len(),
            n(&|v| *v == Verdict::Current),
            n(&|v| matches!(v, Verdict::Restore { .. })),
            n(&|v| matches!(v, Verdict::Differs { .. })),
            n(&|v| *v == Verdict::Other),
            n(&|v| *v == Verdict::Unrecorded),
        ));
        let mut by_from: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for x in &of {
            match &x.2 {
                Verdict::Restore { from, .. } => by_from.entry(from.clone()).or_default().0 += 1,
                Verdict::Differs { from, .. } => by_from.entry(from.clone()).or_default().1 += 1,
                _ => {}
            }
        }
        for (f, (r, d)) in by_from {
            out.push(format!("  recorded under {f}'s inputs: {r} to restore, {d} differ"));
        }
        let mut whys: BTreeMap<String, usize> = BTreeMap::new();
        for x in &of {
            if let Verdict::Differs { why, .. } = &x.2 {
                let kind = why.split(':').next().unwrap_or(why).to_string();
                *whys.entry(kind).or_default() += 1;
            }
        }
        for (w, c) in whys {
            out.push(format!("  differ by {w}: {c}"));
        }
        for x in of.iter().filter(|x| matches!(x.2, Verdict::Restore { .. })).take(sample) {
            out.push(format!("  restore {} {}: {:?}", x.0, x.1, x.2));
        }
        for x in of.iter().filter(|x| matches!(x.2, Verdict::Differs { .. })).take(sample) {
            out.push(format!("  stays {} {}: {:?}", x.0, x.1, x.2));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::Shape;
    use crate::dem::declared::OutlineCut;

    /// A dataset's coverage of cell 10°, 45° cut by outlines: its west half, and an islet (a square
    /// of `islet` degrees at 10.71234°, 45.31234°) when given.
    fn cover(islet: Option<f64>) -> Cover {
        let e7 = |v: f64| (v * 1e7).round() as i32;
        let sq = |w: f64, s: f64, e: f64, n: f64| {
            vec![[e7(w), e7(s)], [e7(e), e7(s)], [e7(e), e7(n)], [e7(w), e7(n)], [e7(w), e7(s)]]
        };
        let mut shapes = vec![Shape::new("half".into(), vec![sq(10.0, 45.0, 10.5, 46.0)], 0.0)];
        if let Some(d) = islet {
            shapes.push(Shape::new("islet".into(), vec![sq(10.71234, 45.31234, 10.71234 + d, 45.31234 + d)], 0.0));
        }
        let o = OutlineCut::of_shapes(&format!("{islet:?}"), [(10, 45)].into_iter().collect(), shapes);
        Cover { outline: Some(std::sync::Arc::new(o)), ..Cover::of(Some(&[[10.0, 45.0, 11.0, 46.0]])) }
    }

    /// Task #832: the samples decide, not the rings: an islet far from the area, or one no sample
    /// point of the zooms read falls in, holds every sample alike; one a sample falls in doesn't.
    #[test]
    fn an_islet_no_sample_falls_in_samples_the_same() {
        let all = |z: u8| (9..=16).contains(&z);
        // (Far from it: the cell's west, the half's edge across it.)
        assert_eq!(first_apart(&cover(Some(2e-4)), &cover(None), [10.3, 45.4, 10.6, 45.6], &all), None);
        // (A 20 m islet, under the z16 samples (~1 m apart): one of them in it.)
        let p = first_apart(&cover(Some(2e-4)), &cover(None), [10.6, 45.2, 10.8, 45.4], &all).expect("a sample in it");
        assert!((10.71234..10.71254).contains(&p.0) && (45.31234..45.31254).contains(&p.1), "{p:?}");
        // (A 10 cm islet, read at z9–12 alone (samples ~10 m apart): none in it.)
        let base = |z: u8| (9..=12).contains(&z);
        assert_eq!(first_apart(&cover(Some(1e-6)), &cover(None), [10.6, 45.2, 10.8, 45.4], &base), None);
        // (Its coverage the same: nothing asked.)
        assert_eq!(first_apart(&cover(None), &cover(None), [10.0, 45.0, 11.0, 46.0], &all), None);
    }
}
