//! Administrative and ISO 3166 outlines (docs/plan.md §5 and §6, OSM pass step 8): what regions are
//! made of (`osm:<relation>`), the areas modules apply by (ISO 3166-1 and -2), and the Regions
//! panel's "areas containing this place".
//!
//! Assembled from the pass's outline set with `osmium export`, which builds each relation's
//! (multi)polygon; the countries and territories it leaves out (their rings open in OSM) are
//! assembled here ([`assemble`]). Kept: administrative areas of levels 2–8, and anything with an
//! ISO 3166 code.
//! Stored as a sectioned file (`sources/osm/<date>/outlines`, RDSECT01):
//!
//! | section | records |
//! |---|---|
//! | `recs` | [`OutlineRec`] (64 B), sorted by relation id |
//! | `rings` | [`Ring`] (16 B): each outline's rings, outer and inner (even–odd) |
//! | `points` | `[i32; 2]` lon, lat (E7) |
//! | `srings`, `spoints` | the same, simplified for drawing (Douglas–Peucker; 1 km for countries, 250 m for levels 3–4, 60 m finer) |
//! | `strings` | names, `\n`-separated; index 0 is "" |

use det::Det;
use anyhow::{bail, ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

pub const FORMAT: &str = "outlines-1";

/// One outline.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct OutlineRec {
    /// The OSM relation id.
    pub id: u64,
    /// West, south, east, north (E7).
    pub bbox: [i32; 4],
    pub area_km2: f32,
    /// Strings: `name`, `name:en`, and the ISO 3166 code (`ISO3166-2`, else `ISO3166-1`).
    pub name: u32,
    pub name_en: u32,
    pub iso: u32,
    /// Its rings in `rings` (and simplified, in `srings`).
    pub rings: u32,
    pub nrings: u32,
    pub srings: u32,
    pub nsrings: u32,
    /// `admin_level` (0: none).
    pub level: u8,
    /// [`flag`]s.
    pub flags: u8,
    /// With [`flag::REPAIRED`]: the widest gap (m, rounded) the assembly closed with a straight
    /// line. 0 otherwise.
    pub gap_m: u16,
    /// Strings: for a country or territory ([`flag::ISO1`]), its own ISO 3166-1 code; for others,
    /// that of the smallest country or territory it lies in; "" when none is known. (Files written
    /// before 2026-10-15 have "" for a country and the containing one's `iso` for others: both
    /// read the same through [`Outlines::country_code`].)
    pub country: u32,
}

pub mod flag {
    /// Has `ISO3166-1` (a country or territory).
    pub const ISO1: u8 = 1;
    /// Has `ISO3166-2` (a subdivision).
    pub const ISO2: u8 = 2;
    /// osmium's export left it out (its rings open in OSM) and it was assembled here, its gaps
    /// closed with straight lines ([`super::OutlineRec::gap_m`]).
    pub const REPAIRED: u8 = 4;
    /// Neither the export nor the repair could assemble it from this pass: its rings are the
    /// previous pass's outline of the same relation (its names and codes this pass's).
    pub const STALE: u8 = 8;
}

/// A ring: `count` points of `points` from `start`. A polygon is an outer ring followed by its
/// inner ones (holes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct Ring {
    pub start: u64,
    pub count: u32,
    /// 0: outer (starts a polygon), 1: inner.
    pub kind: u32,
}

const _: () = assert!(std::mem::size_of::<OutlineRec>() == 64);
const _: () = assert!(std::mem::size_of::<Ring>() == 16);

/// An outline as read from the export, before storing.
struct Parsed {
    id: u64,
    level: u8,
    flags: u8,
    name: String,
    name_en: String,
    iso: String,
    /// `ISO3166-1` ("" when none).
    iso1: String,
    /// [`OutlineRec::gap_m`].
    gap_m: u16,
    /// Polygons: rings (the first outer, the rest inner), E7.
    polys: Vec<Vec<Vec<[i32; 2]>>>,
}

/// The tags that decide whether an outline is kept and how it's coded.
struct Tags<'a> {
    level: u8,
    iso1: &'a str,
    iso2: &'a str,
    admin: bool,
}

impl<'a> Tags<'a> {
    fn of(get: &impl Fn(&str) -> Option<&'a str>) -> Tags<'a> {
        let level = get("admin_level").and_then(|s| s.trim().parse::<u8>().ok()).unwrap_or(0);
        Tags {
            level,
            iso1: get("ISO3166-1").or_else(|| get("ISO3166-1:alpha2")).unwrap_or(""),
            iso2: get("ISO3166-2").unwrap_or(""),
            admin: get("boundary") == Some("administrative") && (2..=8).contains(&level),
        }
    }

    /// Kept: administrative areas of levels 2–8, and anything with an ISO 3166 code.
    fn kept(&self) -> bool {
        self.admin || !self.iso1.is_empty() || !self.iso2.is_empty()
    }
}

/// The outline from a relation's tags (`get`) and polygons, when it's one we keep.
fn outline<'a>(id: u64, get: impl Fn(&str) -> Option<&'a str>, polys: Vec<Vec<Vec<[i32; 2]>>>) -> Option<Parsed> {
    let t = Tags::of(&get);
    if !t.kept() {
        return None;
    }
    let mut flags = 0;
    if !t.iso1.is_empty() {
        flags |= flag::ISO1;
    }
    if !t.iso2.is_empty() {
        flags |= flag::ISO2;
    }
    Some(Parsed {
        id,
        level: t.level,
        flags,
        name: get("name").unwrap_or("").to_string(),
        name_en: get("name:en").unwrap_or("").to_string(),
        iso: if t.iso2.is_empty() { t.iso1 } else { t.iso2 }.to_string(),
        iso1: t.iso1.to_string(),
        gap_m: 0,
        polys,
    })
}

/// The outline in one exported feature, when it's one we keep.
fn parse(line: &str) -> Option<Parsed> {
    let line = line.trim_matches(|c: char| c == '\u{1e}' || c.is_whitespace());
    if line.is_empty() {
        return None;
    }
    let f: Value = serde_json::from_str(line).ok()?;
    let p = f.get("properties")?.as_object()?;
    if p.get("@type")?.as_str()? != "relation" {
        return None;
    }
    let get = |k: &str| p.get(k).and_then(|v| v.as_str());
    if !Tags::of(&get).kept() {
        return None;
    }
    let g = f.get("geometry")?;
    let e7 = |v: &Value| -> Option<[i32; 2]> { Some([(v.get(0)?.as_f64()? * 1e7).round() as i32, (v.get(1)?.as_f64()? * 1e7).round() as i32]) };
    let ring = |r: &Value| -> Option<Vec<[i32; 2]>> { r.as_array()?.iter().map(e7).collect() };
    let poly = |p: &Value| -> Option<Vec<Vec<[i32; 2]>>> { p.as_array()?.iter().map(ring).collect() };
    let polys: Vec<Vec<Vec<[i32; 2]>>> = match g.get("type")?.as_str()? {
        "Polygon" => vec![poly(g.get("coordinates")?)?],
        "MultiPolygon" => g.get("coordinates")?.as_array()?.iter().map(poly).collect::<Option<_>>()?,
        _ => return None,
    };
    outline(p.get("@id")?.as_u64()?, get, polys)
}

/// Signed area (m²) of a ring, on a local equirectangular projection.
fn ring_area_m2(r: &[[i32; 2]]) -> f64 {
    if r.len() < 3 {
        return 0.0;
    }
    let lat0 = r.iter().map(|p| p[1] as f64).sum::<f64>() / r.len() as f64 * 1e-7;
    let kx = 111_320.0 * lat0.to_radians().dcos() * 1e-7;
    let ky = 110_574.0 * 1e-7;
    let mut s = 0.0;
    for i in 0..r.len() {
        let (a, b) = (r[i], r[(i + 1) % r.len()]);
        s += (a[0] as f64 * kx) * (b[1] as f64 * ky) - (b[0] as f64 * kx) * (a[1] as f64 * ky);
    }
    s / 2.0
}

/// Douglas–Peucker with a tolerance in metres (on a local projection); keeps a closed ring's
/// first and last points and at least four.
pub fn simplify(r: &[[i32; 2]], tol_m: f64) -> Vec<[i32; 2]> {
    if r.len() <= 4 {
        return r.to_vec();
    }
    let lat0 = r[0][1] as f64 * 1e-7;
    let kx = 111_320.0 * lat0.to_radians().dcos() * 1e-7;
    let ky = 110_574.0 * 1e-7;
    let xy: Vec<(f64, f64)> = r.iter().map(|p| (p[0] as f64 * kx, p[1] as f64 * ky)).collect();
    let mut keep = vec![false; r.len()];
    keep[0] = true;
    keep[r.len() - 1] = true;
    // A closed ring's chord from its first point to itself is a point: split at the farthest point.
    let far = (1..r.len() - 1).max_by(|&a, &b| {
        let d = |i: usize| (xy[i].0 - xy[0].0).powi(2) + (xy[i].1 - xy[0].1).powi(2);
        d(a).total_cmp(&d(b))
    });
    let mut stack: Vec<(usize, usize)> = match far {
        Some(f) => {
            keep[f] = true;
            vec![(0, f), (f, r.len() - 1)]
        }
        None => vec![(0, r.len() - 1)],
    };
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay) = xy[a];
        let (bx, by) = xy[b];
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let mut best = (0.0f64, 0usize);
        for (i, &(px, py)) in xy.iter().enumerate().take(b).skip(a + 1) {
            let d2 = if len2 == 0.0 {
                (px - ax).powi(2) + (py - ay).powi(2)
            } else {
                let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
                (px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)
            };
            if d2 > best.0 {
                best = (d2, i);
            }
        }
        if best.0 > tol_m * tol_m {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    let out: Vec<[i32; 2]> = r.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
    if out.len() < 4 {
        r.to_vec()
    } else {
        out
    }
}

fn tolerance_m(level: u8) -> f64 {
    match level {
        0..=2 => 1000.0,
        3..=4 => 250.0,
        _ => 60.0,
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Summary {
    pub outlines: usize,
    pub points: usize,
    pub simplified_points: usize,
    pub by_level: std::collections::BTreeMap<u8, usize>,
    /// Countries and territories the export couldn't assemble, assembled here ([`repair`]):
    /// relation, its `ISO3166-1`, and the widest gap closed (m).
    pub repaired: Vec<(u64, String, f64)>,
    /// Countries and territories neither the export nor the repair could assemble, kept as the
    /// previous pass's outline ([`flag::STALE`]): relation, its `ISO3166-1`, why, and the date
    /// of the pass its rings are from.
    pub stale: Vec<(u64, String, String, String)>,
    /// Other kept relations (subdivisions, administrative areas) the export couldn't assemble:
    /// left out, by relation id.
    pub unassembled: Vec<u64>,
}

fn osmium(args: &mut Command, what: &str) -> Result<()> {
    let st = store::spawn::status(args).with_context(|| format!("run {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

/// Assembles the outlines of `set` (the pass's outline set, OSM PBF) into a sectioned file at `out`,
/// using `work` for osmium's export.
///
/// osmium's export leaves out, without naming it, a relation whose rings don't close (OSM's data
/// broken for a while: the 2026-09-28 planet's Poland and Ukraine, a way missing from their shared
/// border). So the relations kept ([`Tags::kept`]) the export didn't give are found from the set's
/// relations. Countries and territories (`boundary=administrative` and `ISO3166-1`) among them
/// are assembled here ([`repair`]: their ways joined, gaps up to [`MAX_GAP_M`] closed;
/// [`flag::REPAIRED`]). One that can't be keeps the `previous` pass's outline of the same
/// relation ([`flag::STALE`], [`Summary::stale`]); one without fails the assembly: every country
/// present in OSM has an outline, or the pass stops. Others are left out and counted
/// ([`Summary::unassembled`]): a few dozen subdivisions broken in OSM at any time would otherwise
/// stop every pass. The file's meta lists all three.
pub fn assemble(set: &Path, work: &Path, out: &Path, previous: Option<(&Path, &str)>) -> Result<Summary> {
    std::fs::create_dir_all(work)?;
    let rels_opl = work.join("outlines-relations.opl");
    osmium(Command::new("osmium").args(["cat", "-t", "relation", "-f", "opl", "--overwrite", "-o"]).arg(&rels_opl).arg(set), "osmium cat (the outline set's relations)")?;
    let rels = opl::kept_relations(BufReader::with_capacity(1 << 20, std::fs::File::open(&rels_opl)?))?;
    std::fs::remove_file(&rels_opl).ok();
    let geo = work.join("outlines.geojsonseq");
    // Node locations in a sparse index on disk (16 bytes a node): osmium's default can switch to a
    // dense array as big as the highest node id (about 100 GB for the planet's).
    let idx = work.join("outlines-nodes.idx");
    std::fs::remove_file(&idx).ok();
    let st = store::spawn::status(
        Command::new("osmium")
        .args(["export", "-f", "geojsonseq", "--geometry-types=polygon", "-a", "type,id", "--overwrite"])
        .arg(format!("--index-type=sparse_file_array,{}", idx.display()))
        .arg("-o")
        .arg(&geo)
        .arg(set),
    )
    .context("run osmium export")?;
    std::fs::remove_file(&idx).ok();
    ensure!(st.success(), "osmium export of {} failed: {st}", set.display());
    let parsed = read_export(&geo);
    std::fs::remove_file(&geo).ok();
    let mut parsed = parsed?;
    let have: std::collections::HashSet<u64> = parsed.iter().map(|p| p.id).collect();
    let missing: Vec<&opl::Relation> = rels.iter().filter(|r| !have.contains(&r.id)).collect();
    let (countries, others): (Vec<&opl::Relation>, Vec<&opl::Relation>) = missing.into_iter().partition(|r| r.country());
    let mut sum = Summary { unassembled: others.iter().map(|r| r.id).collect(), ..Default::default() };
    if !countries.is_empty() {
        // Their ways and nodes from the set.
        let ids = work.join("outlines-repair.ids");
        let members = work.join("outlines-repair.opl");
        let mut list = String::new();
        for r in &countries {
            for (w, _) in &r.ways {
                list.push_str(&format!("w{w}\n"));
            }
        }
        std::fs::write(&ids, list)?;
        osmium(Command::new("osmium").args(["getid", "-r", "-f", "opl", "--overwrite", "-i"]).arg(&ids).arg("-o").arg(&members).arg(set), "osmium getid (the unassembled countries' ways)")?;
        let geom = opl::Geometry::read(BufReader::with_capacity(1 << 20, std::fs::File::open(&members)?))?;
        std::fs::remove_file(&ids).ok();
        std::fs::remove_file(&members).ok();
        let (ok, failed) = repair_all(&countries, &geom);
        for (p, gap) in ok {
            sum.repaired.push((p.id, p.iso1.clone(), gap));
            parsed.push(p);
        }
        // (The previous pass's file read only when one is needed.)
        let prev = if failed.is_empty() { None } else { previous.and_then(|(p, d)| Previous::open(p, d)) };
        let (stale, lost) = keep_previous(failed, prev.as_ref());
        ensure!(
            lost.is_empty(),
            "outlines: countries and territories osmium's export left out, that can't be assembled here either, and that no previous pass has an outline of (fix OSM's relation or wait for a planet where it's whole): {}",
            lost.join("; ")
        );
        for (p, why, from) in stale {
            sum.stale.push((p.id, p.iso1.clone(), why, from));
            parsed.push(p);
        }
    }
    write(parsed, out, sum)
}

/// The previous pass's outlines, with its date and the relations it had itself kept from a pass
/// before (by id: that pass's date).
struct Previous {
    outlines: Outlines,
    date: String,
    stale: HashMap<u64, String>,
}

impl Previous {
    /// The file at `path`, made by the pass of `date`; None (said in the log) when it can't be
    /// read: a country it would have given is then lost, and the assembly fails naming it.
    fn open(path: &Path, date: &str) -> Option<Previous> {
        let r = Outlines::open(path).and_then(|o| Ok((o, Outlines::stale_in(path)?)));
        match r {
            Ok((outlines, stale)) => Some(Previous { outlines, date: date.to_string(), stale: stale.into_iter().collect() }),
            Err(e) => {
                eprintln!("outlines: the previous pass's ({date}) can't be read: {e:#}");
                None
            }
        }
    }
}

/// Repairs each of `rels` ([`repair`]) with the ways and nodes of `geom`: the outlines made, with
/// the widest gap each closed, and each that couldn't be with why.
fn repair_all<'a>(rels: &[&'a opl::Relation], geom: &opl::Geometry) -> (Vec<(Parsed, f64)>, Vec<(&'a opl::Relation, String)>) {
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for &r in rels {
        let res = repair(r, geom).and_then(|(polys, gap)| {
            let mut p = outline(r.id, |k| r.get(k), polys).context("not an outline kept")?;
            p.flags |= flag::REPAIRED;
            p.gap_m = gap.round().min(u16::MAX as f64) as u16;
            Ok((p, gap))
        });
        match res {
            Ok(x) => ok.push(x),
            Err(e) => failed.push((r, format!("{e:#}"))),
        }
    }
    (ok, failed)
}

/// The relations that couldn't be repaired, each as the `previous` pass's outline of it (its
/// rings; its names and codes this pass's tags), [`flag::STALE`], with why and the date of the
/// pass its rings are from (the previous pass's, or the one it had them from in turn); and a line
/// for each with no previous outline (none, or one that can't be read).
fn keep_previous(failed: Vec<(&opl::Relation, String)>, previous: Option<&Previous>) -> (Vec<(Parsed, String, String)>, Vec<String>) {
    let (mut kept, mut lost) = (Vec::new(), Vec::new());
    for (r, why) in failed {
        let got = previous.and_then(|pv| pv.outlines.by_id(r.id).map(|rec| (pv, rec))).map(|(pv, rec)| {
            let polys = pv.outlines.polygons(rec)?;
            let mut p = outline(r.id, |k| r.get(k), polys).context("not an outline kept")?;
            p.flags |= flag::STALE;
            anyhow::Ok((p, pv.stale.get(&r.id).unwrap_or(&pv.date).clone()))
        });
        match got {
            Some(Ok((p, from))) => kept.push((p, why, from)),
            Some(Err(e)) => lost.push(format!("relation {} ({}): {why}; the previous pass's outline can't be read: {e:#}", r.id, r.iso1())),
            None => lost.push(format!("relation {} ({}): {why}", r.id, r.iso1())),
        }
    }
    (kept, lost)
}

/// The widest gap between two ends of a relation's ways [`repair`] closes with a straight line.
/// Wider than a border way OSM loses for a while to a split or a redraw (the 2026-09-28
/// planet's Poland–Ukraine gap is 150 m), narrow enough not to join coasts or islands across sea.
pub const MAX_GAP_M: f64 = 1000.0;

/// Distance (m) between two E7 points, on a local equirectangular projection.
fn dist_m(a: [i32; 2], b: [i32; 2]) -> f64 {
    let lat = (a[1] as f64 + b[1] as f64) * 0.5e-7;
    let dx = (b[0] as f64 - a[0] as f64) * 1e-7 * 111_320.0 * lat.to_radians().dcos();
    let dy = (b[1] as f64 - a[1] as f64) * 1e-7 * 110_574.0;
    (dx * dx + dy * dy).sqrt()
}

/// Rings from ways (node ids and points, in the relation's member order, so the same relation gives
/// the same rings): ways joined end to end where they share an end node, then the chains left open
/// joined nearest ends first, each gap up to [`MAX_GAP_M`] and its line crossing none of `segments`
/// (all of the relation's ways', outer and inner, and the gap lines drawn so far, which this adds
/// its own to).
/// The widest gap closed with them. Refused: a node where more than two way ends meet (which way
/// a ring takes there is a guess that can make it cross itself).
pub(crate) fn rings_of(ways: &[Vec<(u64, [i32; 2])>], segments: &mut Vec<([i32; 2], [i32; 2])>) -> Result<(Vec<Vec<[i32; 2]>>, f64)> {
    let mut ends: std::collections::BTreeMap<u64, Vec<usize>> = Default::default();
    for (i, w) in ways.iter().enumerate() {
        ensure!(w.len() >= 2, "a way of fewer than two nodes");
        ends.entry(w[0].0).or_default().push(i);
        ends.entry(w[w.len() - 1].0).or_default().push(i);
    }
    if let Some((n, v)) = ends.iter().find(|(_, v)| v.len() > 2) {
        bail!("{} way ends meet at node {n}", v.len());
    }
    let mut used = vec![false; ways.len()];
    let mut chains: Vec<Vec<(u64, [i32; 2])>> = Vec::new();
    for i in 0..ways.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut c = ways[i].clone();
        // Extend at the end, then (the chain reversed) at the other.
        for _ in 0..2 {
            while c.len() < 2 || c[0].0 != c[c.len() - 1].0 {
                let end = c[c.len() - 1].0;
                let Some(&j) = ends.get(&end).and_then(|v| v.iter().find(|&&j| !used[j])) else { break };
                used[j] = true;
                let w = &ways[j];
                if w[0].0 == end {
                    c.extend_from_slice(&w[1..]);
                } else {
                    c.extend(w[..w.len() - 1].iter().rev());
                }
            }
            c.reverse();
        }
        chains.push(c);
    }
    let closed = |c: &Vec<(u64, [i32; 2])>| c.len() >= 4 && c[0].0 == c[c.len() - 1].0;
    let (mut rings, mut open): (Vec<_>, Vec<_>) = chains.into_iter().partition(closed);
    let mut widest = 0.0f64;
    while !open.is_empty() {
        // The nearest pair of ends: a chain's end to a chain's start (itself: closing it), or to
        // another's end (that one reversed).
        let mut best: Option<(f64, usize, usize, bool)> = None;
        for (a, ca) in open.iter().enumerate() {
            let e = ca[ca.len() - 1].1;
            for (b, cb) in open.iter().enumerate() {
                for rev in [false, true] {
                    if rev && a == b {
                        continue;
                    }
                    let d = dist_m(e, if rev { cb[cb.len() - 1].1 } else { cb[0].1 });
                    if best.is_none_or(|(bd, ..)| d < bd) {
                        best = Some((d, a, b, rev));
                    }
                }
            }
        }
        let (d, a, b, rev) = best.expect("an open chain");
        let from = open[a][open[a].len() - 1].1;
        ensure!(d <= MAX_GAP_M, "a gap of {d:.0} m in its rings, near {from:?} (wider than {MAX_GAP_M} m)");
        let to = if rev { open[b][open[b].len() - 1].1 } else { open[b][0].1 };
        if let Some(s) = segments.iter().find(|s| crosses((from, to), **s)) {
            bail!("the line closing a gap of {d:.0} m at {from:?} would cross the segment {:?}–{:?}", s.0, s.1);
        }
        segments.push((from, to));
        widest = widest.max(d);
        if a == b {
            let mut c = open.swap_remove(a);
            let first = c[0];
            c.push(first);
            ensure!(c.len() >= 4, "a ring of fewer than three points");
            rings.push(c);
        } else {
            let mut cb = open[b].clone();
            if rev {
                cb.reverse();
            }
            open[a].extend(cb);
            open.remove(b);
        }
    }
    Ok((rings.into_iter().map(|r| r.into_iter().map(|(_, p)| p).collect()).collect(), widest))
}

/// The segments of `ways`.
pub(crate) fn segments_of<'a>(ways: impl IntoIterator<Item = &'a Vec<(u64, [i32; 2])>>) -> Vec<([i32; 2], [i32; 2])> {
    ways.into_iter().flat_map(|w| w.windows(2).map(|s| (s[0].1, s[1].1))).collect()
}

/// Whether segments `a` and `b` cross at a point inside both (sharing an end, or touching, isn't).
fn crosses(a: ([i32; 2], [i32; 2]), b: ([i32; 2], [i32; 2])) -> bool {
    let o = |p: [i32; 2], q: [i32; 2], r: [i32; 2]| -> i128 {
        let v = (q[0] as i128 - p[0] as i128) * (r[1] as i128 - p[1] as i128) - (q[1] as i128 - p[1] as i128) * (r[0] as i128 - p[0] as i128);
        v.signum()
    };
    o(a.0, a.1, b.0) * o(a.0, a.1, b.1) < 0 && o(b.0, b.1, a.0) * o(b.0, b.1, a.1) < 0
}

/// A relation's polygons from its ways (role `outer` or none: outer; `inner`: inner), assembled
/// here ([`rings_of`]), each inner ring in the smallest outer holding its first point; with the
/// widest gap closed.
fn repair(r: &opl::Relation, geom: &opl::Geometry) -> Result<(Vec<Vec<Vec<[i32; 2]>>>, f64)> {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    for (w, role) in &r.ways {
        let nodes = geom.ways.get(w).with_context(|| format!("way {w} not in the set"))?;
        let pts = nodes.iter().map(|n| geom.nodes.get(n).map(|p| (*n, *p)).with_context(|| format!("node {n} of way {w} not in the set"))).collect::<Result<Vec<_>>>()?;
        match role.as_str() {
            "outer" | "" => outer.push(pts),
            "inner" => inner.push(pts),
            _ => {}
        }
    }
    ensure!(!outer.is_empty(), "no outer ways");
    let mut segments = segments_of(outer.iter().chain(&inner));
    let (outers, g1) = rings_of(&outer, &mut segments)?;
    let (inners, g2) = if inner.is_empty() { (Vec::new(), 0.0) } else { rings_of(&inner, &mut segments)? };
    Ok((nest(outers, inners)?, g1.max(g2)))
}

/// Polygons from outer and inner rings: each inner ring in the smallest outer one holding its
/// first point (a pond on an island in a lake goes in the island, not the mainland).
fn nest(outers: Vec<Vec<[i32; 2]>>, inners: Vec<Vec<[i32; 2]>>) -> Result<Vec<Vec<Vec<[i32; 2]>>>> {
    let areas: Vec<f64> = outers.iter().map(|r| ring_area_m2(r).abs()).collect();
    let mut polys: Vec<Vec<Vec<[i32; 2]>>> = outers.into_iter().map(|o| vec![o]).collect();
    for h in inners {
        let k = (0..polys.len()).filter(|&k| inside(&[&polys[k][0]], h[0])).min_by(|&x, &y| areas[x].total_cmp(&areas[y]).then(x.cmp(&y))).context("an inner ring in no outer one")?;
        polys[k].push(h);
    }
    Ok(polys)
}

/// Reading osmium's OPL: the set's relations and the repaired ones' ways and nodes.
mod opl {
    use super::Tags;
    use anyhow::{Context, Result};
    use std::collections::HashMap;
    use std::io::BufRead;

    /// A relation kept ([`Tags::kept`]) that's an area (`type=boundary` or `multipolygon`, with
    /// ways): its tags that matter, and, for a country, its ways and their roles.
    #[derive(Debug)]
    pub struct Relation {
        pub id: u64,
        pub tags: Vec<(String, String)>,
        pub ways: Vec<(u64, String)>,
    }

    /// The tags an outline is made with ([`super::outline`]).
    const KEYS: [&str; 7] = ["admin_level", "boundary", "ISO3166-1", "ISO3166-1:alpha2", "ISO3166-2", "name", "name:en"];

    impl Relation {
        pub fn get(&self, k: &str) -> Option<&str> {
            self.tags.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
        }
        /// A country or territory: tagged `ISO3166-1` and `boundary=administrative`. (Not the
        /// other relations some countries' codes are on too: Thailand's `boundary=land_area`.)
        pub fn country(&self) -> bool {
            !Tags::of(&|k| self.get(k)).iso1.is_empty() && self.get("boundary") == Some("administrative")
        }
        pub fn iso1(&self) -> &str {
            Tags::of(&|k| self.get(k)).iso1
        }
    }

    /// OPL's escapes: `%` hex code point `%`.
    pub fn unescape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut it = s.split('%');
        out.push_str(it.next().unwrap_or(""));
        while let Some(code) = it.next() {
            if let Some(c) = u32::from_str_radix(code, 16).ok().and_then(char::from_u32) {
                out.push(c);
            }
            out.push_str(it.next().unwrap_or(""));
        }
        out
    }

    fn field<'a>(line: &'a str, c: char) -> Option<&'a str> {
        line.split(' ').skip(1).find_map(|f| f.strip_prefix(c))
    }

    /// The relations of an OPL file kept as outlines, sorted by id.
    pub fn kept_relations(r: impl BufRead) -> Result<Vec<Relation>> {
        let mut out = Vec::new();
        for l in r.lines() {
            let l = l?;
            let Some(id) = l.split(' ').next().and_then(|f| f.strip_prefix('r')).and_then(|s| s.parse::<u64>().ok()) else { continue };
            let all: Vec<(String, String)> = field(&l, 'T')
                .unwrap_or("")
                .split(',')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (unescape(k), unescape(v)))
                .collect();
            let get = |k: &str| all.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
            if !matches!(get("type"), Some("boundary" | "multipolygon")) || !Tags::of(&get).kept() {
                continue;
            }
            let ways: Vec<(u64, String)> = field(&l, 'M')
                .unwrap_or("")
                .split(',')
                .filter_map(|m| {
                    let (r, role) = m.split_once('@')?;
                    Some((r.strip_prefix('w')?.parse().ok()?, unescape(role)))
                })
                .collect();
            if ways.is_empty() {
                continue;
            }
            let tags: Vec<(String, String)> = all.into_iter().filter(|(k, _)| KEYS.contains(&k.as_str())).collect();
            let mut rel = Relation { id, tags, ways };
            if !rel.country() {
                rel.ways = Vec::new();
            }
            out.push(rel);
        }
        out.sort_by_key(|r| r.id);
        Ok(out)
    }

    /// Ways (node ids) and nodes (E7) of an OPL file.
    #[derive(Default)]
    pub struct Geometry {
        pub ways: HashMap<u64, Vec<u64>>,
        pub nodes: HashMap<u64, [i32; 2]>,
    }

    impl Geometry {
        pub fn read(r: impl BufRead) -> Result<Geometry> {
            let mut g = Geometry::default();
            for l in r.lines() {
                let l = l?;
                let mut f = l.split(' ');
                let head = f.next().unwrap_or("");
                let Some(id) = head.get(1..).and_then(|s| s.parse::<u64>().ok()) else { continue };
                if head.starts_with('n') {
                    let c = |p: char| field(&l, p).filter(|s| !s.is_empty()).and_then(|s| s.parse::<f64>().ok());
                    let (Some(x), Some(y)) = (c('x'), c('y')) else { continue };
                    g.nodes.insert(id, [(x * 1e7).round() as i32, (y * 1e7).round() as i32]);
                } else if head.starts_with('w') {
                    let nodes = field(&l, 'N')
                        .unwrap_or("")
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.strip_prefix('n').and_then(|s| s.parse().ok()).with_context(|| format!("way {id}: node {s}")))
                        .collect::<Result<Vec<u64>>>()?;
                    g.ways.insert(id, nodes);
                }
            }
            Ok(g)
        }
    }
}

/// The outlines of an export, sorted by relation id.
fn read_export(geo: &Path) -> Result<Vec<Parsed>> {
    let f = std::fs::File::open(geo).with_context(|| format!("open {}", geo.display()))?;
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut batch: Vec<String> = Vec::with_capacity(4096);
    let mut lines = BufReader::with_capacity(1 << 20, f).lines();
    loop {
        batch.clear();
        for l in lines.by_ref().take(4096) {
            batch.push(l?);
        }
        if batch.is_empty() {
            break;
        }
        parsed.extend(batch.par_iter().filter_map(|l| parse(l)).collect::<Vec<_>>());
    }
    Ok(parsed)
}

/// The outlines of an existing export, stored at `out`. Only what the export holds: none of
/// [`assemble`]'s checks or repairs (the set isn't at hand), so a country the export left out is
/// missing here, silently. For tests and probes, not a pass.
pub fn assemble_geojsonseq(geo: &Path, out: &Path) -> Result<Summary> {
    eprintln!("outlines: {} as exported, without the repairs of countries the export left out", geo.display());
    write(read_export(geo)?, out, Summary::default())
}

/// A country's or territory's own ISO 3166-1 code: its `ISO3166-1` tag where that's an assigned
/// code (Åland's AX, though tagged as Finland's FI-01 too), else read from its ISO 3166 code
/// ([`territory_code`]: the Canaries' IC is reserved, not assigned: ES from ES-CN).
fn own_code(iso1: &str, iso: &str) -> Option<String> {
    (two_letters(iso1) && !RESERVED.contains(&iso1)).then(|| iso1.to_string()).or_else(|| territory_code(iso).map(str::to_owned))
}

/// ISO 3166-1's exceptionally reserved codes, which OSM tags some territories with (`IC` the
/// Canaries, `EA` Ceuta and Melilla, …) but name no country of their own.
const RESERVED: [&str; 13] = ["AC", "CP", "CQ", "DG", "EA", "EU", "EZ", "FX", "IC", "SU", "TA", "UK", "UN"];

/// Stores `parsed` at `out`, with `sum`'s repaired, stale and unassembled relations in its meta.
fn write(mut parsed: Vec<Parsed>, out: &Path, mut sum: Summary) -> Result<Summary> {
    // One record per relation (osmium writes each once; a repeat would be a bug upstream).
    parsed.sort_by_key(|p| p.id);
    parsed.dedup_by_key(|p| p.id);

    let mut strings: Vec<String> = vec![String::new()];
    let mut index: HashMap<String, u32> = HashMap::from([(String::new(), 0)]);
    let mut intern = |s: &str| -> u32 {
        let s = s.replace('\n', " ");
        if let Some(&i) = index.get(&s) {
            return i;
        }
        strings.push(s.clone());
        let i = (strings.len() - 1) as u32;
        index.insert(s, i);
        i
    };
    // Simplify in parallel (rings in the same order as the polygons'), then lay out.
    let simplified: Vec<Vec<Vec<[i32; 2]>>> = parsed.par_iter().map(|p| p.polys.iter().flatten().map(|r| simplify(r, tolerance_m(p.level))).collect()).collect();
    // Which country each one lies in: the smallest country or territory outline holding a point
    // of it (a town in Guadeloupe lies in GP, not FR, whose relation holds Guadeloupe too).
    let mut countries: Vec<(usize, f64, crate::coverage::Shape)> = parsed
        .par_iter()
        .enumerate()
        .filter(|(_, p)| p.flags & flag::ISO1 != 0)
        .map(|(i, p)| {
            let area: f64 = p.polys.iter().flat_map(|poly| poly.iter().enumerate().map(|(k, r)| if k == 0 { ring_area_m2(r).abs() } else { -ring_area_m2(r).abs() })).sum();
            (i, area, crate::coverage::Shape::new(String::new(), p.polys.iter().flatten().cloned().collect(), 0.0))
        })
        .collect();
    countries.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let countries: Vec<(usize, crate::coverage::Shape)> = countries.into_iter().map(|(i, _, s)| (i, s)).collect();
    let country_of: Vec<Option<usize>> = parsed
        .par_iter()
        .map(|p| {
            if p.flags & flag::ISO1 != 0 {
                return None;
            }
            // A point inside the area: its outer ring's centre, else its first point.
            let r = p.polys.first()?.first()?;
            let n = r.len().max(1) as i64;
            let c = [(r.iter().map(|q| q[0] as i64).sum::<i64>() / n) as i32, (r.iter().map(|q| q[1] as i64).sum::<i64>() / n) as i32];
            let inside = |pt: [i32; 2]| countries.iter().find(|(_, sh)| sh.contains(pt)).map(|(i, _)| *i);
            inside(c).or_else(|| inside(r[0]))
        })
        .collect();
    let mut recs = Vec::with_capacity(parsed.len());
    let (mut rings, mut points, mut srings, mut spoints): (Vec<Ring>, Vec<[i32; 2]>, Vec<Ring>, Vec<[i32; 2]>) = Default::default();
    for (pi, (p, simple)) in parsed.iter().zip(simplified).enumerate() {
        let mut bbox = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        let mut area = 0.0;
        let r0 = rings.len();
        let mut kinds: Vec<u32> = Vec::new();
        for poly in &p.polys {
            for (k, r) in poly.iter().enumerate() {
                let a = ring_area_m2(r).abs();
                area += if k == 0 { a } else { -a };
                for q in r {
                    bbox = [bbox[0].min(q[0]), bbox[1].min(q[1]), bbox[2].max(q[0]), bbox[3].max(q[1])];
                }
                let kind = (k > 0) as u32;
                kinds.push(kind);
                rings.push(Ring { start: points.len() as u64, count: r.len() as u32, kind });
                points.extend_from_slice(r);
            }
        }
        let s0 = srings.len();
        for (r, &kind) in simple.iter().zip(&kinds) {
            srings.push(Ring { start: spoints.len() as u64, count: r.len() as u32, kind });
            spoints.extend_from_slice(r);
        }
        // A country's or territory's own code; else that of the one it lies in.
        let code = |q: &Parsed| own_code(&q.iso1, &q.iso).unwrap_or_default();
        let country = if p.flags & flag::ISO1 != 0 { code(p) } else { country_of[pi].map(|c| code(&parsed[c])).unwrap_or_default() };
        if rings.len() == r0 {
            continue;
        }
        *sum.by_level.entry(p.level).or_default() += 1;
        recs.push(OutlineRec {
            id: p.id,
            bbox,
            area_km2: (area.max(0.0) / 1e6) as f32,
            name: intern(&p.name),
            name_en: intern(&p.name_en),
            iso: intern(&p.iso),
            rings: r0 as u32,
            nrings: (rings.len() - r0) as u32,
            srings: s0 as u32,
            nsrings: (srings.len() - s0) as u32,
            level: p.level,
            flags: p.flags,
            gap_m: p.gap_m,
            country: intern(&country),
        });
    }
    sum.outlines = recs.len();
    sum.points = points.len();
    sum.simplified_points = spoints.len();
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let stale: Vec<(u64, &str)> = sum.stale.iter().map(|s| (s.0, s.3.as_str())).collect();
    let meta = serde_json::json!({ "fmt": FORMAT, "outlines": recs.len(), "points": points.len(), "spoints": spoints.len(), "repaired": sum.repaired, "stale": stale, "unassembled": sum.unassembled });
    let mut w = store::sect::SectWriter::create(out, meta)?;
    w.add_pod("recs", &recs)?;
    w.add_pod("rings", &rings)?;
    w.add_pod("points", &points)?;
    w.add_pod("srings", &srings)?;
    w.add_pod("spoints", &spoints)?;
    w.add("strings", strings.join("\n").as_bytes())?;
    w.finish()?;
    Ok(sum)
}

/// Whether `p` lies inside the rings (even–odd: holes and multipolygons alike).
pub fn inside(rings: &[&[[i32; 2]]], p: [i32; 2]) -> bool {
    let (x, y) = (p[0] as f64, p[1] as f64);
    let mut c = false;
    for r in rings {
        let n = r.len();
        if n < 3 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = (r[i][0] as f64, r[i][1] as f64);
            let (xj, yj) = (r[j][0] as f64, r[j][1] as f64);
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
    }
    c
}

/// The outlines: records and names in memory, rings read from the file as asked, with plain
/// reads (the NAS's file is never mapped, nor read whole: its points are gigabytes for the
/// planet), as the server's index reads them.
pub struct Outlines {
    pub recs: Vec<OutlineRec>,
    strings: Vec<String>,
    file: store::sect::SectReader<store::range::PlainFile>,
}

impl Outlines {
    pub fn open(path: &Path) -> Result<Outlines> {
        let r = store::sect::SectReader::open(store::range::PlainFile::open(path)?)?;
        ensure!(r.meta().get("fmt").and_then(|v| v.as_str()) == Some(FORMAT), "{}: not {FORMAT}", path.display());
        Ok(Outlines { recs: r.read_pod("recs")?, strings: String::from_utf8(r.read("strings")?)?.split('\n').map(str::to_string).collect(), file: r })
    }

    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }

    pub fn by_id(&self, id: u64) -> Option<&OutlineRec> {
        self.recs.binary_search_by_key(&id, |r| r.id).ok().map(|k| &self.recs[k])
    }

    /// Rings `first..first + n` of `rings` with their points from `points`, and their kinds.
    fn read_rings(&self, rings: &str, points: &str, first: u32, n: u32) -> Result<Vec<(u32, Vec<[i32; 2]>)>> {
        let rs: Vec<Ring> = bytemuck::pod_collect_to_vec(&self.file.read_part(rings, first as u64 * std::mem::size_of::<Ring>() as u64, n as usize * std::mem::size_of::<Ring>())?);
        let (Some(a), Some(b)) = (rs.first(), rs.last()) else { return Ok(Vec::new()) };
        let (start, end) = (a.start as u64, b.start as u64 + b.count as u64);
        let pts: Vec<[i32; 2]> = bytemuck::pod_collect_to_vec(&self.file.read_part(points, start * 8, ((end - start) * 8) as usize)?);
        Ok(rs.iter().map(|r| (r.kind, pts[(r.start as u64 - start) as usize..(r.start as u64 - start) as usize + r.count as usize].to_vec())).collect())
    }

    pub fn rings(&self, o: &OutlineRec) -> Result<Vec<Vec<[i32; 2]>>> {
        Ok(self.read_rings("rings", "points", o.rings, o.nrings)?.into_iter().map(|r| r.1).collect())
    }
    /// The simplified rings grouped as polygons (an outer ring, then its holes).
    /// Its full rings as polygons, as stored: an outer ring, then its holes. (Outlines from passes
    /// before ring kinds were recorded have every ring outer: each its own polygon.)
    pub fn polygons(&self, o: &OutlineRec) -> Result<Vec<Vec<Vec<[i32; 2]>>>> {
        let mut out: Vec<Vec<Vec<[i32; 2]>>> = Vec::new();
        for (kind, pts) in self.read_rings("rings", "points", o.rings, o.nrings)? {
            match out.last_mut() {
                Some(p) if kind == 1 => p.push(pts),
                _ => out.push(vec![pts]),
            }
        }
        Ok(out)
    }

    /// The countries and territories of the file at `path` kept as the previous pass's outlines
    /// ([`flag::STALE`]): relation id and the date of the pass its rings are from, from its meta
    /// alone (none in a file made before they were recorded).
    pub fn stale_in(path: &Path) -> Result<Vec<(u64, String)>> {
        let r = store::sect::SectReader::open(store::range::PlainFile::open(path)?)?;
        Ok(r.meta().get("stale").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default())
    }

    /// The simplified rings grouped as polygons (an outer ring, then its holes).
    pub fn simple_polygons(&self, o: &OutlineRec) -> Result<Vec<Vec<Vec<[i32; 2]>>>> {
        let mut out: Vec<Vec<Vec<[i32; 2]>>> = Vec::new();
        for (kind, pts) in self.read_rings("srings", "spoints", o.srings, o.nsrings)? {
            match out.last_mut() {
                Some(p) if kind == 1 => p.push(pts),
                _ => out.push(vec![pts]),
            }
        }
        Ok(out)
    }

    pub fn simple_rings(&self, o: &OutlineRec) -> Result<Vec<Vec<[i32; 2]>>> {
        Ok(self.read_rings("srings", "spoints", o.srings, o.nsrings)?.into_iter().map(|r| r.1).collect())
    }

    /// Every outline containing `p`, smallest area first.
    pub fn containing(&self, p: [i32; 2]) -> Result<Vec<&OutlineRec>> {
        let mut v = Vec::new();
        for o in self.recs.iter().filter(|o| p[0] >= o.bbox[0] && p[0] <= o.bbox[2] && p[1] >= o.bbox[1] && p[1] <= o.bbox[3]) {
            let rings = self.rings(o)?;
            if inside(&rings.iter().map(Vec::as_slice).collect::<Vec<_>>(), p) {
                v.push(o);
            }
        }
        v.sort_by(|a, b| a.area_km2.total_cmp(&b.area_km2));
        Ok(v)
    }

    /// The ISO 3166-1 code of the country or territory an outline is or lies in: its own when it
    /// has one ([`own_code`]), else that of the country it lies in, else its subdivision
    /// code's country part; "" when none is known. Every code given is two capital letters.
    pub fn country_code(&self, o: &OutlineRec) -> String {
        let iso = self.string(o.iso);
        territory_code(self.string(o.country))
            .or_else(|| (o.flags & flag::ISO1 != 0).then(|| territory_code(iso)).flatten())
            .or_else(|| iso.split('-').next().filter(|p| two_letters(p)))
            .unwrap_or("")
            .to_string()
    }

    /// The ISO 3166-1 and 3166-2 codes at `p` ("" where none).
    pub fn iso_at(&self, p: [i32; 2]) -> Result<(String, String)> {
        let c = self.containing(p)?;
        let pick = |f: u8| c.iter().find(|o| o.flags & f != 0).map(|o| self.string(o.iso).to_string()).unwrap_or_default();
        Ok((pick(flag::ISO1), pick(flag::ISO2)))
    }
}

/// The ISO 3166 code an outline is drawn under for the languages spoken where (`names::spoken`):
/// an ISO 3166-1 territory's own code ([`own_code`]: Åland's AX, Hong Kong's HK), a
/// subdivision's ISO 3166-2 code; None for one with neither.
fn spoken_code(o: &OutlineRec, string: &impl Fn(u32) -> String) -> Option<String> {
    let iso = string(o.iso);
    if o.flags & flag::ISO1 != 0 {
        own_code_of(o.flags, &iso, &string(o.country)).map(str::to_owned)
    } else if o.flags & flag::ISO2 != 0 {
        Some(iso)
    } else {
        None
    }
}

/// The outlines the languages spoken where a thing is are drawn from (`names::spoken`): each ISO
/// 3166-1 territory under its own code, and the subdivisions with a line of their own in `table`
/// (Quebec, Wales, …), with their simplified polygons (`polygons`: an outer ring, then its holes).
pub fn spoken_areas(table: &names::Table, recs: &[OutlineRec], string: impl Fn(u32) -> String, mut polygons: impl FnMut(&OutlineRec) -> Result<Vec<Vec<Vec<[i32; 2]>>>>) -> Result<Vec<names::spoken::Area>> {
    let mut out = Vec::new();
    for o in recs {
        let Some(code) = spoken_code(o, &string).filter(|c| table.wanted(c) && table.get(c).is_some()) else { continue };
        out.push(names::spoken::Area { code, area_km2: f64::from(o.area_km2), polygons: polygons(o)? });
    }
    Ok(out)
}

/// Every code an outline of `recs` could be drawn under for the languages spoken where
/// ([`spoken_areas`]): what a line of the languages input names to have an effect.
pub fn spoken_codes(recs: &[OutlineRec], string: impl Fn(u32) -> String) -> std::collections::BTreeSet<String> {
    recs.iter().filter_map(|o| spoken_code(o, &string)).collect()
}

impl Outlines {
    /// The languages spoken where, from these outlines and `table` ([`spoken_areas`]).
    pub fn spoken(&self, table: &names::Table) -> Result<names::Spoken> {
        Ok(names::Spoken::build(table, spoken_areas(table, &self.recs, |i| self.string(i).to_owned(), |o| self.simple_polygons(o))?))
    }

    /// The codes of these outlines ([`spoken_codes`]).
    pub fn spoken_codes(&self) -> std::collections::BTreeSet<String> {
        spoken_codes(&self.recs, |i| self.string(i).to_owned())
    }
}

/// Countries whose ISO 3166-2 lists their territories by the territories' own ISO 3166-1 codes:
/// CN-HK, CN-MO, CN-TW; FR-GF, FR-GP, FR-MQ, FR-NC, FR-PF, FR-PM, FR-RE, FR-YT…; NL-AW, NL-CW,
/// NL-SX; US-AS, US-GU, US-MP, US-PR, US-VI.
const OWN_CODED: [&str; 4] = ["CN", "FR", "NL", "US"];

fn two_letters(s: &str) -> bool {
    s.len() == 2 && s.bytes().all(|b| b.is_ascii_uppercase())
}

/// A country's or territory's ISO 3166-1 code from the `iso` of its outline (one tagged
/// `ISO3166-1`): that code; or, when the outline has a subdivision code too (which `iso` then
/// holds), the code's end where it's a territory's own ("US-PR": PR), else its start ("ES-CN": ES,
/// not China). None when that isn't two capital letters.
/// A country's or territory's own ISO 3166-1 code from its record's `flags` and strings `iso` and
/// `country` (the code stored there, [`own_code`]; a file from before: read from `iso`); None for
/// an outline that isn't one ([`flag::ISO1`]).
pub fn own_code_of<'a>(flags: u8, iso: &'a str, country: &'a str) -> Option<&'a str> {
    if flags & flag::ISO1 == 0 {
        return None;
    }
    territory_code(country).or_else(|| territory_code(iso))
}

/// The two-letter country code in a record's `country` string: the code itself, or (a file from
/// before the codes were stored) the containing territory's ISO code read ([`territory_code`]).
pub fn country_of_string(country: &str) -> Option<&str> {
    territory_code(country)
}

fn territory_code(iso: &str) -> Option<&str> {
    match iso.split_once('-') {
        None => two_letters(iso).then_some(iso),
        Some((c, t)) if OWN_CODED.contains(&c) && two_letters(t) => Some(t),
        Some((c, _)) => two_letters(c).then_some(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE_WITH_HOLE: &str = r#"{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]]},"properties":{"@type":"relation","@id":7,"boundary":"administrative","admin_level":"4","name":"Carré","name:en":"Square","ISO3166-2":"XX-SQ"}}"#;

    #[test]
    fn parse_keeps_admin_and_iso_only() {
        let p = parse(SQUARE_WITH_HOLE).unwrap();
        assert_eq!((p.id, p.level, p.iso.as_str(), p.flags), (7, 4, "XX-SQ", flag::ISO2));
        assert_eq!(p.polys[0].len(), 2);
        assert!(parse(&SQUARE_WITH_HOLE.replace("\"relation\"", "\"way\"")).is_none());
        assert!(parse(&SQUARE_WITH_HOLE.replace("\"admin_level\":\"4\"", "\"admin_level\":\"10\"").replace(",\"ISO3166-2\":\"XX-SQ\"", "")).is_none());
        // An ISO code keeps it whatever its level.
        assert!(parse(&SQUARE_WITH_HOLE.replace("\"admin_level\":\"4\"", "\"admin_level\":\"10\"")).is_some());
    }

    #[test]
    fn country_codes() {
        // A territory with its own ISO 3166-1 code and a subdivision's (Puerto Rico's), and an
        // outline with neither in no country.
        let d = tempfile::tempdir().unwrap();
        let geo = d.path().join("o.geojsonseq");
        let pr = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":9").replace("\"ISO3166-2\":\"XX-SQ\"", "\"ISO3166-1\":\"PR\",\"ISO3166-2\":\"US-PR\"");
        let none = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":11").replace(",\"ISO3166-2\":\"XX-SQ\"", "").replace("[[0,0],[1,0],[1,1],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]", "[[10,10],[11,10],[11,11],[10,11],[10,10]]");
        std::fs::write(&geo, format!("\u{1e}{pr}\n\u{1e}{none}\n")).unwrap();
        let out = d.path().join("outlines.sect");
        assemble_geojsonseq(&geo, &out).unwrap();
        let o = Outlines::open(&out).unwrap();
        assert_eq!(o.country_code(o.by_id(9).unwrap()), "PR");
        assert_eq!(o.country_code(o.by_id(11).unwrap()), "");
        // The codes as read: a territory's own, a subdivision code's country, never a non-code.
        assert_eq!(territory_code("US-PR"), Some("PR"));
        assert_eq!(territory_code("FR-GF"), Some("GF"));
        assert_eq!(territory_code("ES-CN"), Some("ES"), "the Canaries: Spain's, not China's");
        assert_eq!(territory_code("FR-2A"), Some("FR"));
        assert_eq!(territory_code("JP"), Some("JP"));
        assert_eq!(territory_code(""), None);
        assert_eq!(territory_code("Japan"), None);
        assert_eq!(territory_code("jp"), None);
    }

    #[test]
    fn country_codes_in_a_territory() {
        // A town (no codes) in Puerto Rico (both codes): Puerto Rico's, not "US-PR"; a region
        // with both codes where the subdivision code isn't the territory's own: its country.
        let d = tempfile::tempdir().unwrap();
        let geo = d.path().join("o.geojsonseq");
        let pr = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":9").replace("\"admin_level\":\"4\"", "\"admin_level\":\"2\"").replace("\"ISO3166-2\":\"XX-SQ\"", "\"ISO3166-1\":\"PR\",\"ISO3166-2\":\"US-PR\"");
        let town = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":12").replace("\"admin_level\":\"4\"", "\"admin_level\":\"8\"").replace(",\"ISO3166-2\":\"XX-SQ\"", "").replace("[[0,0],[1,0],[1,1],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]", "[[0.1,0.1],[0.2,0.1],[0.2,0.2],[0.1,0.2],[0.1,0.1]]");
        let canaries = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":13").replace("\"ISO3166-2\":\"XX-SQ\"", "\"ISO3166-1\":\"IC\",\"ISO3166-2\":\"ES-CN\"").replace("[[0,0],[1,0],[1,1],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]", "[[20,20],[21,20],[21,21],[20,21],[20,20]]");
        std::fs::write(&geo, format!("\u{1e}{pr}\n\u{1e}{town}\n\u{1e}{canaries}\n")).unwrap();
        let out = d.path().join("outlines.sect");
        assemble_geojsonseq(&geo, &out).unwrap();
        let o = Outlines::open(&out).unwrap();
        assert_eq!(o.string(o.by_id(12).unwrap().country), "PR");
        assert_eq!(o.country_code(o.by_id(12).unwrap()), "PR");
        assert_eq!(o.country_code(o.by_id(13).unwrap()), "ES");
    }

    #[test]
    fn round_trip_and_lookup() {
        let d = tempfile::tempdir().unwrap();
        let geo = d.path().join("o.geojsonseq");
        let big = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":3").replace("[[0,0],[1,0],[1,1],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]", "[[-1,-1],[2,-1],[2,2],[-1,2],[-1,-1]]").replace("\"admin_level\":\"4\"", "\"admin_level\":\"2\"").replace("\"ISO3166-2\":\"XX-SQ\"", "\"ISO3166-1\":\"XX\"");
        std::fs::write(&geo, format!("\u{1e}{SQUARE_WITH_HOLE}\n\u{1e}{big}\n")).unwrap();
        let out = d.path().join("outlines.sect");
        let s = assemble_geojsonseq(&geo, &out).unwrap();
        assert_eq!(s.outlines, 2);
        let o = Outlines::open(&out).unwrap();
        let e7 = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        assert_eq!(o.containing(e7(0.2, 0.2)).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), vec![7, 3]);
        // In the hole: only the country.
        assert_eq!(o.containing(e7(0.5, 0.5)).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), vec![3]);
        assert_eq!(o.iso_at(e7(0.2, 0.2)).unwrap(), ("XX".to_string(), "XX-SQ".to_string()));
        // Countries: a subdivision's is the country it lies in; a country's its own.
        assert_eq!(o.country_code(o.by_id(7).unwrap()), "XX");
        assert_eq!(o.country_code(o.by_id(3).unwrap()), "XX");
        assert_eq!(o.string(o.by_id(7).unwrap().name_en), "Square");
        assert_eq!(o.simple_polygons(o.by_id(7).unwrap()).unwrap().iter().map(Vec::len).collect::<Vec<_>>(), vec![2], "the hole nests in its polygon");
        assert_eq!(o.string(o.by_id(7).unwrap().country), "XX", "inside the country");
        assert_eq!(o.string(o.by_id(3).unwrap().country), "XX", "a country's own");
        let a = o.by_id(7).unwrap().area_km2;
        // 1° × 1° at the equator less a 0.2° × 0.2° hole: about 12,300 − 490 km².
        assert!((a - 11_820.0).abs() < 200.0, "{a}");
    }

    #[test]
    fn simplify_keeps_shape() {
        let n = 1000;
        let ring: Vec<[i32; 2]> = (0..=n).map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            [(t.dcos() * 1e6) as i32, (t.dsin() * 1e6) as i32]
        }).collect();
        let s = simplify(&ring, 100.0);
        assert!(s.len() > 8 && s.len() < 200, "{}", s.len());
        assert_eq!(s.first(), ring.first());
        assert_eq!(s.last(), ring.last());
    }

    /// A country whose outer ring has a 55 m gap (w1's first node n5 is 0.0005° from w2's n1, as
    /// OSM's Poland and Ukraine were on 2026-09-28), with a hole; a district; a route.
    const BROKEN: &str = "\
r100 v1 dV c1 t2026-09-27T00:00:00Z i1 ua Ttype=boundary,boundary=administrative,admin_level=2,ISO3166-1=PL,name=Polska,name:en=Poland%20%(test),wikidata=Q36 Mw1@outer,w2@outer,w3@inner,n9@label,r101@subarea
r101 v1 dV c1 t2026-09-27T00:00:00Z i1 ua Ttype=boundary,boundary=administrative,admin_level=6,name=Powiat Mw4@outer
r102 v1 dV c1 t2026-09-27T00:00:00Z i1 ua Ttype=route,route=bus,ISO3166-1=PL Mw1@
r103 v1 dV c1 t2026-09-27T00:00:00Z i1 ua Ttype=boundary,boundary=administrative,admin_level=2,ISO3166-1=EU Mr100@
r104 v1 dV c1 t2026-09-27T00:00:00Z i1 ua Ttype=boundary,boundary=land_area,ISO3166-1=PL Mw1@outer
";
    const NODES: &str = "\
n1 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0 y0
n2 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0.1 y0
n3 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0.1 y0.1
n4 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0 y0.1
n5 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0.0005 y0
n6 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0.04 y0.04
n7 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0.06 y0.04
n8 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0.06 y0.06
n10 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T x0.04 y0.06
w1 v1 dV c1 t2026-09-27T00:00:00Z i1 ua Tsource=x Nn5,n2,n3
w2 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T Nn1,n4,n3
w3 v1 dV c1 t2026-09-27T00:00:00Z i1 ua T Nn6,n7,n8,n10,n6
";

    #[test]
    fn opl_reads_relations_and_geometry() {
        assert_eq!(opl::unescape("Poland%20%(test)"), "Poland (test)");
        assert_eq!(opl::unescape("a%2c%b%25%"), "a,b%");
        let rels = opl::kept_relations(BROKEN.as_bytes()).unwrap();
        // Kept: the country (with its ways and roles) and the district (without: never repaired);
        // not the route, nor a relation of no ways.
        // Kept, not a country: a land area with the country's code (never repaired, a warning).
        assert_eq!(rels.iter().map(|r| r.id).collect::<Vec<_>>(), vec![100, 101, 104]);
        assert!(rels[0].country() && !rels[1].country() && !rels[2].country());
        assert_eq!(rels[0].iso1(), "PL");
        assert_eq!(rels[0].ways, vec![(1, "outer".to_string()), (2, "outer".to_string()), (3, "inner".to_string())]);
        assert!(rels[1].ways.is_empty());
        assert!(rels[0].tags.contains(&("name:en".to_string(), "Poland (test)".to_string())));
        assert!(!rels[0].tags.iter().any(|(k, _)| k == "wikidata"), "only the tags an outline is made with");
        let g = opl::Geometry::read(NODES.as_bytes()).unwrap();
        assert_eq!(g.ways[&2], vec![1, 4, 3]);
        assert_eq!(g.nodes[&5], [5000, 0]);
        assert_eq!(g.nodes[&10], [400_000, 600_000]);
    }

    #[test]
    fn repair_closes_a_small_gap_and_nests_the_hole() {
        let rels = opl::kept_relations(BROKEN.as_bytes()).unwrap();
        let g = opl::Geometry::read(NODES.as_bytes()).unwrap();
        let (ok, failed) = repair_all(&[&rels[0]], &g);
        assert!(failed.is_empty(), "{failed:?}");
        let (p, gap) = &ok[0];
        assert!((gap - 55.7).abs() < 1.0, "{gap}");
        assert_eq!((p.id, p.level, p.iso1.as_str(), p.flags, p.gap_m, p.name_en.as_str()), (100, 2, "PL", flag::ISO1 | flag::REPAIRED, 56, "Poland (test)"));
        assert_eq!(p.polys.len(), 1);
        assert_eq!(p.polys[0].len(), 2, "the outer ring and its hole");
        let outer = &p.polys[0][0];
        assert_eq!(outer.first(), outer.last());
        assert_eq!(outer.len(), 6, "five nodes and the closing one: {outer:?}");
        // The same input, the same rings.
        let (again, _) = repair_all(&[&rels[0]], &g);
        assert_eq!(again[0].0.polys, p.polys);
        // Stored, it's Poland's outline, and it's drawn under PL.
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("o.sect");
        write(ok.into_iter().map(|(p, _)| p).collect(), &out, Summary::default()).unwrap();
        let o = Outlines::open(&out).unwrap();
        assert_eq!(o.country_code(o.by_id(100).unwrap()), "PL");
        let rec = o.by_id(100).unwrap();
        assert_eq!((rec.flags, rec.gap_m), (flag::ISO1 | flag::REPAIRED, 56), "the repair is in the record");
        assert_eq!(o.spoken_codes().into_iter().collect::<Vec<_>>(), vec!["PL"]);
        assert_eq!(o.iso_at([20_000, 20_000]).unwrap().0, "PL");
        assert_eq!(o.iso_at([500_000, 500_000]).unwrap().0, "", "in the hole");
    }

    #[test]
    fn repair_fails_on_a_wide_gap_or_a_missing_way() {
        let rels = opl::kept_relations(BROKEN.as_bytes()).unwrap();
        // n5 2.2 km from n1: no straight line drawn across that.
        let g = opl::Geometry::read(NODES.replace("x0.0005 y0", "x0.02 y0").as_bytes()).unwrap();
        let (ok, failed) = repair_all(&[&rels[0]], &g);
        assert!(ok.is_empty());
        assert!(failed[0].1.starts_with("a gap of 2226 m"), "{failed:?}");
        let g = opl::Geometry::read(NODES.replace("w2 ", "w20 ").as_bytes()).unwrap();
        let (_, failed) = repair_all(&[&rels[0]], &g);
        assert_eq!(failed[0].1, "way 2 not in the set");
    }

    #[test]
    fn an_unrepairable_country_keeps_the_previous_outline() {
        let rels = opl::kept_relations(BROKEN.as_bytes()).unwrap();
        let g = opl::Geometry::read(NODES.replace("x0.0005 y0", "x0.02 y0").as_bytes()).unwrap();
        // The previous pass had relation 100 (a square, named otherwise), not 104.
        let d = tempfile::tempdir().unwrap();
        let geo = d.path().join("prev.geojsonseq");
        let prev_square = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":100").replace("\"ISO3166-2\":\"XX-SQ\"", "\"ISO3166-1\":\"PL\"");
        std::fs::write(&geo, format!("\u{1e}{prev_square}\n")).unwrap();
        let prev_file = d.path().join("prev.sect");
        assemble_geojsonseq(&geo, &prev_file).unwrap();
        let prev = Previous::open(&prev_file, "2026-08-01").unwrap();
        let (_, failed) = repair_all(&[&rels[0]], &g);
        let (kept, lost) = keep_previous(failed, Some(&prev));
        assert!(lost.is_empty());
        let (p, why, from) = &kept[0];
        assert!(why.starts_with("a gap of 2226 m"), "{why}");
        assert_eq!(from, "2026-08-01");
        assert_eq!((p.id, p.flags, p.name_en.as_str()), (100, flag::ISO1 | flag::STALE, "Poland (test)"), "this pass's tags");
        assert_eq!(p.polys, prev.outlines.polygons(prev.outlines.by_id(100).unwrap()).unwrap(), "the previous pass's rings");
        // Stored: flagged, and listed in the meta with the pass its rings are from.
        let out = d.path().join("o.sect");
        let sum = Summary { stale: vec![(100, "PL".into(), why.clone(), from.clone())], ..Default::default() };
        write(kept.into_iter().map(|(p, ..)| p).collect(), &out, sum).unwrap();
        assert_eq!(Outlines::stale_in(&out).unwrap(), vec![(100, "2026-08-01".to_string())]);
        assert!(Outlines::stale_in(&prev_file).unwrap().is_empty());
        let o = Outlines::open(&out).unwrap();
        assert_eq!(o.by_id(100).unwrap().flags, flag::ISO1 | flag::STALE);
        // Broken again in the pass after: still the 2026-08-01 pass's rings, and said so.
        let prev2 = Previous::open(&out, "2026-09-28").unwrap();
        let (_, failed) = repair_all(&[&rels[0]], &g);
        let (kept, _) = keep_previous(failed, Some(&prev2));
        assert_eq!(kept[0].2, "2026-08-01", "carried over, not the stale pass's own date");
        assert_eq!(kept[0].0.polys, p_polys(&prev));
        // None before it (the first pass, or a country new to OSM): the pass fails, naming it.
        for prev in [None, Some(&prev2)] {
            let rels = opl::kept_relations(BROKEN.replace("r100 ", "r105 ").as_bytes()).unwrap();
            let (_, failed) = repair_all(&[&rels[rels.len() - 1]], &g);
            let (kept, lost) = keep_previous(failed, prev);
            assert!(kept.is_empty());
            assert!(lost[0].starts_with("relation 105 (PL): a gap of 2226 m"), "{lost:?}");
        }
    }

    fn p_polys(prev: &Previous) -> Vec<Vec<Vec<[i32; 2]>>> {
        prev.outlines.polygons(prev.outlines.by_id(100).unwrap()).unwrap()
    }

    #[test]
    fn a_previous_file_that_cant_be_read_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert!(Previous::open(&d.path().join("missing.sect"), "2026-08-01").is_none(), "missing");
        let junk = d.path().join("junk.sect");
        std::fs::write(&junk, b"not a sectioned file").unwrap();
        assert!(Previous::open(&junk, "2026-08-01").is_none(), "unreadable");
        // So a country that needed it is lost (the pass fails naming it), not an error of the file.
        let rels = opl::kept_relations(BROKEN.as_bytes()).unwrap();
        let g = opl::Geometry::read(NODES.replace("x0.0005 y0", "x0.02 y0").as_bytes()).unwrap();
        let (_, failed) = repair_all(&[&rels[0]], &g);
        let (kept, lost) = keep_previous(failed, Previous::open(&junk, "2026-08-01").as_ref());
        assert!(kept.is_empty() && lost.len() == 1);
    }

    #[test]
    fn a_gap_line_never_crosses_a_way() {
        // One way that turns back past its start: closing it straight from (2, 0) to (0, 0) would
        // cut through its own segment (1, 5)–(1, −1).
        let p = |i: u64, x: i32, y: i32| (i, [x * 1000, y * 1000]);
        let way = vec![p(1, 0, 0), p(2, 0, 5), p(3, 1, 5), p(4, 1, -1), p(5, 2, -1), p(6, 2, 0)];
        let ways = [way];
        let e = rings_of(&ways, &mut segments_of(&ways)).unwrap_err().to_string();
        assert!(e.starts_with("the line closing a gap of"), "{e}");
        // A way of the other role in the way (an inner way across the outer ring's gap), and a gap
        // line already drawn, block it alike.
        let open = [vec![p(1, 0, 0), p(2, 0, 5), p(3, 2, 5), p(4, 2, 0)]];
        let across = (p(9, 1, -1).1, p(10, 1, 1).1);
        assert!(rings_of(&open, &mut segments_of(&open)).is_ok());
        let mut segs = segments_of(&open);
        segs.push(across);
        assert!(rings_of(&open, &mut segs).unwrap_err().to_string().starts_with("the line closing"));
        let mut segs = segments_of(&open);
        rings_of(&open, &mut segs).unwrap();
        assert_eq!(segs.last(), Some(&(p(4, 2, 0).1, p(1, 0, 0).1)), "its gap line added");
        assert!(crosses(([0, 0], [2, 0]), ([1, -1], [1, 1])));
        assert!(!crosses(([0, 0], [2, 0]), ([2, 0], [3, 1])), "sharing an end");
        assert!(!crosses(([0, 0], [2, 0]), ([3, -1], [3, 1])));
    }

    #[test]
    fn three_ways_at_a_node_are_refused() {
        let w = |ids: &[u64]| ids.iter().map(|&i| (i, [i as i32 * 1000, (i % 3) as i32 * 1000])).collect::<Vec<_>>();
        let ways = [w(&[1, 2, 3]), w(&[3, 4, 1]), w(&[3, 5])];
        let e = rings_of(&ways, &mut segments_of(&ways)).unwrap_err().to_string();
        assert_eq!(e, "3 way ends meet at node 3");
    }

    #[test]
    fn a_hole_goes_in_the_smallest_outer_holding_it() {
        // Mainland 0–100, a lake in it 10–90 (a hole), an island in the lake 20–80, a pond on the
        // island 40–60: the pond is the island's hole, though the mainland holds it too.
        let sq = |a: i32, b: i32| vec![[a, a], [b, a], [b, b], [a, b], [a, a]];
        let polys = nest(vec![sq(0, 100), sq(20, 80)], vec![sq(10, 90), sq(40, 60)]).unwrap();
        assert_eq!(polys, vec![vec![sq(0, 100), sq(10, 90)], vec![sq(20, 80), sq(40, 60)]]);
        assert!(nest(vec![sq(0, 10)], vec![sq(20, 30)]).is_err(), "a hole in no outer ring");
    }

    #[test]
    fn rings_join_ways_in_any_direction() {
        let w = |ids: &[u64]| ids.iter().map(|&i| (i, [i as i32 * 1000, (i % 2) as i32 * 1000])).collect::<Vec<_>>();
        // A ring of three ways, the middle one reversed; and a closed way on its own.
        let ways = [w(&[1, 2, 3]), w(&[5, 4, 3]), w(&[5, 6, 1]), w(&[10, 11, 12, 10])];
        let (rings, gap) = rings_of(&ways, &mut segments_of(&ways)).unwrap();
        assert_eq!(gap, 0.0);
        assert_eq!(rings.len(), 2);
        assert_eq!(rings[0].len(), 7);
        assert_eq!(rings[1].len(), 4);
        assert!(rings_of(&[w(&[1])], &mut Vec::new()).is_err());
    }

    #[test]
    fn territories_by_their_own_codes() {
        // France holding Guadeloupe (tagged GP and FR-971) and a town in it; Åland (AX, and
        // Finland's FI-01); the Canaries (IC, reserved: ES).
        let sq = |id: u64, lvl: u8, tags: &str, x0: f64, x1: f64| {
            format!(
                r#"{{"type":"Feature","geometry":{{"type":"Polygon","coordinates":[[[{x0},{x0}],[{x1},{x0}],[{x1},{x1}],[{x0},{x1}],[{x0},{x0}]]]}},"properties":{{"@type":"relation","@id":{id},"boundary":"administrative","admin_level":"{lvl}",{tags}}}}}"#
            )
        };
        let d = tempfile::tempdir().unwrap();
        let geo = d.path().join("o.geojsonseq");
        let lines = [
            sq(1, 2, r#""ISO3166-1":"FR""#, 0.0, 10.0),
            sq(2, 3, r#""ISO3166-1":"GP","ISO3166-2":"FR-971""#, 1.0, 2.0),
            sq(3, 8, r#""name":"Town""#, 1.2, 1.4),
            sq(4, 3, r#""ISO3166-1":"AX","ISO3166-2":"FI-01""#, 20.0, 21.0),
            sq(5, 4, r#""ISO3166-1":"IC","ISO3166-2":"ES-CN""#, 30.0, 31.0),
        ];
        std::fs::write(&geo, lines.iter().map(|l| format!("\u{1e}{l}\n")).collect::<String>()).unwrap();
        let out = d.path().join("o.sect");
        assemble_geojsonseq(&geo, &out).unwrap();
        let o = Outlines::open(&out).unwrap();
        let cc = |id| o.country_code(o.by_id(id).unwrap());
        assert_eq!([cc(1), cc(2), cc(3), cc(4), cc(5)], ["FR", "GP", "GP", "AX", "ES"]);
        assert_eq!(o.spoken_codes().into_iter().collect::<Vec<_>>(), vec!["AX", "ES", "FR", "GP"]);
        assert_eq!(own_code("SJ", "NO-21").as_deref(), Some("SJ"));
        assert_eq!(own_code("", "US-PR").as_deref(), Some("PR"));
        assert_eq!(own_code("EA", "ES-CE").as_deref(), Some("ES"));
    }
}
