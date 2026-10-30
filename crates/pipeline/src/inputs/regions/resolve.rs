//! Resolving the regions (docs/inputs.md §6.1.4): a folder's region files, shapes and pins, against
//! the build's current pass (its outline set and the regions' extract), into each region's stored
//! geometry and drawing, or the problems that hold it.
//!
//! A plain region (only `osm:` and `shape:` parts in `outline`, no `minus` nor `intersect`) is stored
//! as its parts unchanged, one shape each: an `osm:` part's rings as the outline set holds them with
//! the 1 km coast tolerance, a `shape:` part's as its file has them; exactly what
//! `Coverage::from_recipes` made of a recipe. Any other is computed: its parts put in the region's
//! own longitude frame (`geom::Frame`), the outline's union taken, kept inside the `intersect`'s, the
//! `minus`'s taken away; its direct `osm:` outline parts (and those of a `region:` it names there
//! kept with the tolerance) grown by a real 1 km first, so its coast is as generous; the result split
//! at the antimeridian, one shape per side it lies on.

use super::extract::{Extract, Id};
use super::file::{Line, Part, Region, Target};
use super::geom::{self, Area, Frame, Pt, Ring};
use super::shapes::Polys;
use super::stored::{Drawn, Geometry, Shape};
use crate::outlines::Outlines;
use std::collections::{BTreeMap, BTreeSet};

/// A problem that holds files (an error finding, §6.1.8): its check (`rg-<check>`), what it's about
/// (region ids, file names: the finding's identity), what was found, the files it holds, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub check: &'static str,
    pub about: Vec<String>,
    pub found: Vec<String>,
    pub files: Vec<String>,
    pub message: String,
}

/// The folder as a version holds it: its region files, shape files and pins, read and parsed.
#[derive(Clone, Debug, Default)]
pub struct Folder {
    /// By id: (the file's path, the region).
    pub regions: BTreeMap<String, (String, Region)>,
    /// By name: (the file's path, its polygons, or why it can't be read).
    pub shapes: BTreeMap<String, (String, Result<Polys, String>)>,
    /// `_pins.toml`, when there and read.
    pub pins: BTreeMap<String, String>,
    /// `_rules.toml`, when there and read.
    pub rules: Option<super::tiles::Rules>,
    /// The files that didn't parse (their own findings are the file checks').
    pub unread: BTreeSet<String>,
    /// Two region files with one id, or two shapes with one name.
    pub problems: Vec<Problem>,
    /// Each file's bytes' hash, by path (what a region's resolution is kept by: `Memo`).
    pub hashes: BTreeMap<String, String>,
}

/// The file name of an inputs folder path.
pub fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

/// Whether the regions take a file: a region file (`<id>.toml`), a shape (`<name>.geojson`) or a
/// special file (`is_special`). Anything else is ignored: neither checked nor read (§6.1.2).
pub fn takes(path: &str) -> bool {
    let n = file_name(path);
    n.ends_with(".toml") || n.ends_with(".geojson")
}

/// Whether an inputs folder path is a special file, never a region: a `.toml` whose name starts with `_`
/// (§6.1.2). Only the known ones (`special`) are taken; any other is a check fault.
pub fn is_special(path: &str) -> bool {
    let n = file_name(path);
    n.starts_with('_') && n.ends_with(".toml")
}

/// Whether the regions pass over a name in an inputs folder: `inputs::ignored`'s, but a special file's
/// (`_*.toml`) is taken, to be read or to be a fault.
pub fn passes_over(name: &str) -> bool {
    crate::inputs::ignored(name) && !is_special(name)
}

/// A folder's group file: its name in the map's Regions list, and its order (§6.1.2).
pub const GROUP: &str = "_group.toml";
/// The pins (at the folder's top): the OSM pass the folder is written against (§6.1.5).
pub const PINS: &str = "_pins.toml";
/// The rules for the whole folder (at the folder's top).
pub const RULES: &str = "_rules.toml";

/// The known special files.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Special {
    Group,
    Pins,
    Rules,
}

/// Which known special file an inputs folder path is: `_group.toml` in any folder, `_pins.toml` and
/// `_rules.toml` at the top.
pub fn special(path: &str) -> Option<Special> {
    match (file_name(path), path.contains('/')) {
        (GROUP, _) => Some(Special::Group),
        (PINS, false) => Some(Special::Pins),
        (RULES, false) => Some(Special::Rules),
        _ => None,
    }
}

/// The new name of a special file under its pre-#536 one (at the top): what the check says to rename it to.
pub fn renamed(path: &str) -> Option<&'static str> {
    match path {
        "pins.toml" => Some(PINS),
        "rules.toml" => Some(RULES),
        _ => None,
    }
}

impl Folder {
    /// The folder of the files at `paths`, `read` giving each one's bytes.
    pub fn read(
        paths: impl IntoIterator<Item = String>,
        read: &dyn Fn(&str) -> anyhow::Result<Vec<u8>>,
    ) -> anyhow::Result<Folder> {
        let mut f = Folder::default();
        let mut by_id: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let paths: Vec<String> = paths.into_iter().filter(|p| takes(p)).collect();
        for p in &paths {
            let p = p.clone();
            let kind = special(&p);
            // (Group files are the index's.)
            if kind == Some(Special::Group) {
                continue;
            }
            let bytes = read(&p)?;
            f.hashes.insert(p.clone(), store::naming::hash16(&bytes));
            let name = file_name(&p).to_string();
            if is_special(&p) && kind.is_none() {
                // (An unknown special file: its own finding is the file check's.)
                f.unread.insert(p);
            } else if kind == Some(Special::Rules) {
                match std::str::from_utf8(&bytes).ok().map(super::tiles::parse_rules) {
                    Some(Ok(r)) => f.rules = Some(r),
                    _ => {
                        f.unread.insert(p);
                    }
                }
            } else if kind == Some(Special::Pins) {
                match std::str::from_utf8(&bytes).ok().map(super::file::parse_pins) {
                    Some(Ok(pins)) => f.pins = pins,
                    _ => {
                        f.unread.insert(p);
                    }
                }
            } else if name.ends_with(".toml") {
                match std::str::from_utf8(&bytes).ok().map(|t| super::file::parse(&name, t)) {
                    Some(Ok(r)) => {
                        by_id.entry(r.id.clone()).or_default().push(p.clone());
                        f.regions.insert(r.id.clone(), (p, r));
                    }
                    _ => {
                        f.unread.insert(p);
                    }
                }
            } else {
                by_name.entry(name.clone()).or_default().push(p.clone());
                f.shapes.insert(name, (p, super::shapes::read(&bytes).map_err(|e| format!("{e:#}"))));
            }
        }
        for (id, ps) in by_id.into_iter().filter(|(_, ps)| ps.len() > 1) {
            f.problems.push(Problem {
                check: "rg-dup-id",
                about: vec![id.clone()],
                found: ps.clone(),
                files: ps.clone(),
                message: format!(
                    "region {id} is in {} files: {} (an id is unique across every folder)",
                    ps.len(),
                    ps.join(", ")
                ),
            });
            f.regions.remove(&id);
        }
        for (n, ps) in by_name.into_iter().filter(|(_, ps)| ps.len() > 1) {
            f.problems.push(Problem {
                check: "rg-dup-id",
                about: vec![n.clone()],
                found: ps.clone(),
                files: ps.clone(),
                message: format!(
                    "shape {n} is in {} files: {} (a shape's name is unique across every folder)",
                    ps.len(),
                    ps.join(", ")
                ),
            });
            f.shapes.remove(&n);
        }
        Ok(f)
    }

    /// The regions under folder `g` (its path), `but` left out.
    pub fn members(&self, g: &str, but: &str) -> Vec<String> {
        self.regions
            .iter()
            .filter(|(id, (p, _))| id.as_str() != but && p.starts_with(&format!("{g}/")))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The regions a region needs resolved first: those it names (`region:`, `land:`) and the
    /// members of the folders it names (`group:`), sorted, once each.
    pub fn deps(&self, r: &Region) -> Vec<String> {
        // (Its outline's: its water's are `tiles::resolve`'s, once every outline is resolved.)
        let (ids, groups) = r.outline_refs();
        let mut v: Vec<String> = ids.into_iter().filter(|d| *d != r.id).map(str::to_string).collect();
        for g in groups {
            v.extend(self.members(g, &r.id));
        }
        v.sort();
        v.dedup();
        v
    }

    /// The OSM pass a region is written against: its own `[pins]`, else `_pins.toml`'s.
    pub fn pin<'a>(&'a self, r: &'a Region) -> Option<&'a str> {
        r.pins.get("osm").or_else(|| self.pins.get("osm")).map(String::as_str)
    }
}

/// What the regions are resolved against: the build's current pass, its outline set and the
/// regions' extract, and the outline sets of other passes still in the records (to say what a new
/// pass changed for a region pinned to an older one).
pub struct Context<'a> {
    pub pass: Option<&'a str>,
    pub outlines: Option<&'a Outlines>,
    pub extract: Option<&'a Extract>,
    pub others: &'a BTreeMap<String, Outlines>,
    /// What's been resolved already, kept by what it was resolved from (a check resolves the
    /// candidate, the next version and the accepted one: each region once).
    pub memo: Option<&'a Memo>,
    /// The resolutions kept as pieces (task #897), when the check keeps them.
    pub kept: Option<&'a super::kept::Kept>,
    /// What the context is, for the memo's keys: the outline set's and the extract's names.
    pub names: String,
    /// The regions' datasets (sea zones, territorial seas, lakes, the coastline's land), when read.
    pub data: Option<&'a super::data::Datasets>,
}

/// Resolutions kept, by a key of everything they were made from.
pub type Memo = std::sync::Mutex<std::collections::HashMap<String, (Result<Resolved, Vec<Problem>>, Vec<Id>)>>;

/// A region resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub geometry: Geometry,
    pub drawn: Drawn,
    /// For a region with water, before `tiles::resolve`: the lakes over 100 km² its outline takes
    /// in (plain frame), which become its water unless another region's water names them.
    pub lakes: Area,
}

/// What resolving a folder gives.
#[derive(Debug, Default)]
pub struct Outcome {
    pub resolved: BTreeMap<String, Resolved>,
    pub problems: Vec<Problem>,
    /// The relations and ways the extract lacks, which the check holds files for (`rg-extract`).
    pub needs: BTreeSet<Id>,
    /// By id, a digest of everything its resolution reads: its file, the pins, the
    /// shapes and regions it names (their own keys), the context. A region with water is resolved
    /// with every other (the coast shared between them, the rest), so its key is the whole
    /// folder's (every file but the group files, which no resolution reads).
    pub keys: BTreeMap<String, String>,
}

/// Every region of `folder` resolved against `ctx`, in an order where each comes after those it
/// names (`region:`); a region naming one that's held is held too.
pub fn resolve(folder: &Folder, ctx: &Context) -> Outcome {
    // (As many regions at once as the agent allows now, `pace`: each operation's memory grows with
    // its region.)
    match rayon::ThreadPoolBuilder::new().num_threads(crate::inputs::pace::width()).build() {
        Ok(pool) => pool.install(|| resolve_all(folder, ctx)),
        Err(_) => resolve_all(folder, ctx),
    }
}

fn resolve_all(folder: &Folder, ctx: &Context) -> Outcome {
    let mut out = Outcome { problems: folder.problems.clone(), ..Default::default() };
    let mut state: BTreeMap<String, u8> = BTreeMap::new(); // 1: on the stack, 2: done
    let ids: Vec<String> = folder.regions.keys().cloned().collect();
    let mut held: BTreeSet<String> = BTreeSet::new();
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for id in ids {
        visit(&id, folder, ctx, &mut state, &mut Vec::new(), &mut held, &mut keys, &mut order, &mut out);
    }
    // Resolved in waves, each region once those it names are, a wave's regions at once. Regions
    // with water are `tiles::resolve`'s; one without naming one in its outline is held.
    let mut pending: Vec<(String, String)> = Vec::new();
    for (id, key) in order {
        let (path, r) = &folder.regions[&id];
        if r.has_water() {
            continue;
        }
        if let Some(d) = folder.deps(r).into_iter().find(|d| folder.regions.get(d).is_some_and(|(_, x)| x.has_water()))
        {
            out.problems.push(Problem {
                check: "rg-ref",
                about: vec![id.clone(), format!("region:{d}")],
                found: vec!["water".into()],
                files: vec![path.clone()],
                message: format!(
                    "{id}: region:{d} has water: a region without water can't name one (give it a water key too)"
                ),
            });
            held.insert(id);
            continue;
        }
        pending.push((id, key));
    }
    let regions = pending.len().max(1);
    while !pending.is_empty() {
        crate::agent::jobs::within((regions - pending.len()) as f64 / regions as f64);
        let (wave, rest): (Vec<(String, String)>, Vec<(String, String)>) = pending.into_iter().partition(|(id, _)| {
            folder.deps(&folder.regions[id].1).iter().all(|d| out.resolved.contains_key(d) || held.contains(d))
        });
        if wave.is_empty() {
            break;
        }
        let made: Vec<(String, Result<(Result<Resolved, Vec<Problem>>, Vec<Id>), Problem>)> = {
            let done = &out.resolved;
            let held = &held;
            crate::inputs::pace::par_map(wave.iter().collect(), |(id, key)| {
                let (path, r) = &folder.regions[id];
                if let Some(dep) = folder.deps(r).into_iter().find(|d| held.contains(d)) {
                    return (
                        id.clone(),
                        Err(Problem {
                            check: "rg-ref",
                            about: vec![id.to_string(), format!("region:{dep}")],
                            found: vec!["held".into()],
                            files: vec![path.clone()],
                            message: format!("{id}: region:{dep} is held, and so this region with it"),
                        }),
                    );
                }
                let kept = ctx.memo.and_then(|m| m.lock().unwrap().get(key).cloned());
                let k = match kept {
                    Some(k) => k,
                    None => {
                        let make = || {
                            let mut needs = BTreeSet::new();
                            let res = resolve_one(r, path, folder, ctx, done, done, false, &mut needs);
                            (res, needs.into_iter().collect::<Vec<Id>>())
                        };
                        // (Kept as a piece, task #897; given up once a pause came: the check
                        // stops, nothing of it kept.)
                        let (res, needs) = match ctx.kept {
                            None => make(),
                            Some(kp) => kp
                                .get_or_make(
                                    super::kept::RESOLVED,
                                    key,
                                    make,
                                    |(res, needs), p| {
                                        super::kept::result(p, res, super::kept::resolved);
                                        super::kept::ids(p, needs);
                                    },
                                    |g| {
                                        Ok((
                                            super::kept::get_result(g, super::kept::get_resolved)?,
                                            super::kept::get_ids(g)?,
                                        ))
                                    },
                                )
                                .unwrap_or_else(|| (Err(Vec::new()), Vec::new())),
                        };
                        if let Some(m) = ctx.memo {
                            m.lock().unwrap().insert(key.clone(), (res.clone(), needs.clone()));
                        }
                        (res, needs)
                    }
                };
                (id.clone(), Ok(k))
            })
        };
        for (id, m) in made {
            match m {
                Err(p) => {
                    out.problems.push(p);
                    held.insert(id);
                }
                Ok((res, needs)) => {
                    out.needs.extend(needs);
                    match res {
                        Ok(res) => {
                            out.resolved.insert(id, res);
                        }
                        Err(ps) => {
                            out.problems.extend(ps);
                            held.insert(id);
                        }
                    }
                }
            }
        }
        pending = rest;
    }
    super::tiles::resolve(folder, ctx, &mut out);
    let whole = whole_key(folder, ctx);
    for (id, (_, r)) in &folder.regions {
        if r.has_water() {
            keys.insert(id.clone(), whole.clone());
        }
    }
    keys.retain(|id, _| out.resolved.contains_key(id));
    out.keys = keys;
    out.problems.sort_by(|a, b| (a.check, &a.about, &a.found).cmp(&(b.check, &b.about, &b.found)));
    out.problems.dedup();
    out
}

/// The key of a region with water: the whole folder's (every file but the group files, which no
/// resolution reads) and the context's, as it's resolved with every other.
pub(super) fn whole_key(folder: &Folder, ctx: &Context) -> String {
    let mut parts = vec![
        ctx.names.clone(),
        ctx.pass.unwrap_or("-").to_string(),
        format!("{:?}", folder.pins),
        ctx.data.map(|d| d.names.clone()).unwrap_or_default(),
    ];
    parts.extend(folder.hashes.iter().map(|(p, h)| format!("{p} {h}")));
    store::naming::hash16(parts.join("\n").as_bytes())
}

#[allow(clippy::too_many_arguments)]
fn visit(
    id: &str,
    folder: &Folder,
    ctx: &Context,
    state: &mut BTreeMap<String, u8>,
    stack: &mut Vec<String>,
    held: &mut BTreeSet<String>,
    keys: &mut BTreeMap<String, String>,
    order: &mut Vec<(String, String)>,
    out: &mut Outcome,
) {
    match state.get(id) {
        Some(2) => return,
        Some(1) => {
            // A loop: every region in it named.
            let at = stack.iter().position(|s| s == id).unwrap_or(0);
            let mut ring: Vec<String> = stack[at..].to_vec();
            ring.sort();
            let files: Vec<String> = ring.iter().filter_map(|r| folder.regions.get(r).map(|x| x.0.clone())).collect();
            out.problems.push(Problem {
                check: "rg-loop",
                about: ring.clone(),
                found: Vec::new(),
                files,
                message: format!("a loop of region: references: {}", ring.join(" → ")),
            });
            held.extend(ring);
            return;
        }
        _ => {}
    }
    let Some((path, r)) = folder.regions.get(id) else { return };
    state.insert(id.to_string(), 1);
    stack.push(id.to_string());
    let deps = folder.deps(r);
    for dep in deps.iter().map(String::as_str) {
        if !folder.regions.contains_key(dep) {
            out.problems.push(Problem {
                check: "rg-ref",
                about: vec![id.to_string(), format!("region:{dep}")],
                found: Vec::new(),
                files: vec![path.clone()],
                message: format!("{id}: region:{dep} names no region"),
            });
            held.insert(id.to_string());
            continue;
        }
        visit(dep, folder, ctx, state, stack, held, keys, order, out);
    }
    stack.pop();
    state.insert(id.to_string(), 2);
    if held.contains(id) {
        return;
    }
    if let Some(dep) = deps.iter().map(String::as_str).find(|d| held.contains(*d)) {
        out.problems.push(Problem {
            check: "rg-ref",
            about: vec![id.to_string(), format!("region:{dep}")],
            found: vec!["held".into()],
            files: vec![path.clone()],
            message: format!("{id}: region:{dep} is held, and so this region with it"),
        });
        held.insert(id.to_string());
        return;
    }
    // What it's resolved from: its file, the pins, the shapes it names, those it names' own keys, the
    // context.
    let mut parts = vec![
        ctx.names.clone(),
        ctx.pass.unwrap_or("-").to_string(),
        path.clone(),
        folder.hashes.get(path).cloned().unwrap_or_default(),
        format!("{:?}", folder.pins),
    ];
    for n in r.shapes() {
        parts.push(format!("{n} {:?}", folder.shapes.get(n).map(|s| folder.hashes.get(&s.0))));
    }
    for d in &deps {
        parts.push(format!("{d} {:?}", keys.get(d)));
    }
    if r.needs_data() {
        parts.push(ctx.data.map(|d| d.names.clone()).unwrap_or_default());
    }
    let key = store::naming::hash16(parts.join("\n").as_bytes());
    keys.insert(id.to_string(), key.clone());
    order.push((id.to_string(), key));
}

fn problem(check: &'static str, r: &Region, path: &str, found: Vec<String>, message: String) -> Problem {
    Problem { check, about: vec![r.id.clone()], found, files: vec![path.to_string()], message }
}

/// A part's geometry before the frame: rings (or a line's points), E7, as read.
pub(super) enum Leaf {
    /// Rings (even–odd), each with the tolerance it's kept with (a `region:`'s shapes) and the
    /// country of an `osm:` part.
    Rings(Vec<(Vec<Ring>, u32)>, String),
    Line(Vec<Pt>),
}

fn rings64(rs: &[Vec<[i32; 2]>]) -> Vec<Ring> {
    rs.iter().map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect()).collect()
}

/// The relations a region names, through `region:` too.
fn relations_through(r: &Region, folder: &Folder, seen: &mut BTreeSet<String>, out: &mut BTreeSet<u64>) {
    if !seen.insert(r.id.clone()) {
        return;
    }
    out.extend(r.relations());
    for d in r.regions() {
        if let Some((_, dr)) = folder.regions.get(d) {
            relations_through(dr, folder, seen, out);
        }
    }
}

/// The leaves of a region's outline (`water` false) or water: its pin checked against the pass,
/// each part's rings read (relations, ways, shapes, the datasets, other regions as `done` and
/// `lands` hold them), its lines strung; with the pass and the relations' drawn outlines.
#[allow(clippy::type_complexity)]
pub(super) fn gather(
    r: &Region,
    path: &str,
    folder: &Folder,
    ctx: &Context,
    done: &BTreeMap<String, Resolved>,
    lands: &BTreeMap<String, Resolved>,
    water: bool,
    needs: &mut BTreeSet<Id>,
) -> Result<(BTreeMap<String, Leaf>, BTreeMap<u64, Vec<Vec<Vec<[i32; 2]>>>>, String, Vec<Problem>), Vec<Problem>> {
    let mut ps = Vec::new();
    // The pin.
    let Some(pin) = folder.pin(r) else {
        return Err(vec![problem(
            "rg-pin",
            r,
            path,
            vec!["none".into()],
            format!(
                "{}: no OSM pin: inputs/regions/_pins.toml names the pass the folder is written against (osm = \"{}\")",
                r.id,
                ctx.pass.unwrap_or("<the pass's date>")
            ),
        )]);
    };
    let Some(pass) = ctx.pass else {
        return Err(vec![problem(
            "rg-pin-mismatch",
            r,
            path,
            vec![pin.into(), "-".into()],
            format!("{}: pinned to OSM {pin}; the build has no OSM pass yet", r.id),
        )]);
    };
    if pin != pass {
        let mut rels = BTreeSet::new();
        relations_through(r, folder, &mut BTreeSet::new(), &mut rels);
        let (mut changed, mut same, mut missing, mut other) = (Vec::new(), 0usize, Vec::new(), Vec::new());
        for id in &rels {
            let now = ctx.outlines.and_then(|o| o.by_id(*id).map(|rec| o.rings(rec).ok()));
            match (now, ctx.others.get(pin)) {
                (None, _) if ctx.outlines.is_some() => missing.push(format!("osm:{id}")),
                (Some(now), Some(old)) => match old.by_id(*id).map(|rec| old.rings(rec).ok()) {
                    Some(was) if was == now => same += 1,
                    Some(_) => changed.push(format!("osm:{id}")),
                    None => other.push(format!("osm:{id}")),
                },
                _ => other.push(format!("osm:{id}")),
            }
        }
        let mut msg = format!(
            "{}: pinned to OSM {pin}; the build's pass is {pass}: move the pin in inputs/regions/_pins.toml.",
            r.id
        );
        if !changed.is_empty() || same > 0 || !missing.is_empty() {
            msg += &format!(
                " Changed: {}. Unchanged: {same}. Missing: {}.",
                if changed.is_empty() { "none".into() } else { changed.join(", ") },
                if missing.is_empty() { "none".into() } else { missing.join(", ") }
            );
        }
        if !other.is_empty() {
            msg += &format!(
                " Not compared (the pinned pass's outlines aren't in the records, or not in its outline set): {}.",
                other.join(", ")
            );
        }
        let ways = r.ways();
        if !ways.is_empty() {
            msg += &format!(
                " Its lines' ways (not compared): {}.",
                ways.iter().map(|w| format!("w{w}")).collect::<Vec<_>>().join(", ")
            );
        }
        return Err(vec![problem("rg-pin-mismatch", r, path, vec![pin.into(), pass.into()], msg)]);
    }
    // The leaves.
    let mut leaves: BTreeMap<String, Leaf> = BTreeMap::new();
    let mut drawn_osm: BTreeMap<u64, Vec<Vec<Vec<[i32; 2]>>>> = BTreeMap::new();
    for part in r.parts_of(water) {
        match part {
            Part::Osm(id) => {
                let key = part.to_string();
                if leaves.contains_key(&key) {
                    continue;
                }
                if let Some(o) = ctx.outlines {
                    if let Some(rec) = o.by_id(*id) {
                        match o.rings(rec) {
                            Ok(rs) => {
                                if let Ok(s) = o.simple_polygons(rec) {
                                    drawn_osm.insert(*id, s);
                                }
                                leaves.insert(key, Leaf::Rings(vec![(rings64(&rs), 1000)], o.country_code(rec)));
                            }
                            Err(e) => ps.push(problem(
                                "rg-osm-no-area",
                                r,
                                path,
                                vec![key.clone()],
                                format!("{}: {key}: the pass's outline can't be read: {e:#}", r.id),
                            )),
                        }
                        continue;
                    }
                }
                match ctx.extract.and_then(|e| e.relation(*id)) {
                    None => {
                        needs.insert(Id::R(*id));
                        ps.push(problem("rg-extract", r, path, vec![key.clone(), pass.into()], format!("{}: {key} isn't in the pass's outline set: waiting for the regions' extract of OSM {pass} to have it", r.id)));
                    }
                    Some(None) => ps.push(problem(
                        "rg-osm-missing",
                        r,
                        path,
                        vec![key.clone(), pass.into()],
                        format!("{}: {key}: OSM {pass} has no such relation", r.id),
                    )),
                    Some(Some(rel)) if rel.rings.is_empty() => ps.push(problem(
                        "rg-osm-no-area",
                        r,
                        path,
                        vec![key.clone(), pass.into()],
                        format!(
                            "{}: {key}: its rings don't close into an area in OSM {pass} ({})",
                            r.id,
                            rel.problem.as_deref().unwrap_or("no rings")
                        ),
                    )),
                    Some(Some(rel)) => {
                        leaves.insert(key, Leaf::Rings(vec![(rings64(&rel.rings), 1000)], String::new()));
                    }
                }
            }
            Part::Shape(n) => {
                let key = part.to_string();
                match folder.shapes.get(n) {
                    None => ps.push(problem(
                        "rg-shape",
                        r,
                        path,
                        vec![key.clone()],
                        format!("{}: {key}: no shape file of that name in the folder", r.id),
                    )),
                    Some((sp, Err(e))) => ps.push(Problem {
                        check: "rg-shape",
                        about: vec![r.id.clone()],
                        found: vec![key.clone(), e.clone()],
                        files: vec![path.to_string(), sp.clone()],
                        message: format!("{}: {key}: {e}", r.id),
                    }),
                    Some((_, Ok(polys))) => {
                        leaves.insert(key, Leaf::Rings(vec![(super::shapes::flat(polys), 0)], String::new()));
                    }
                }
            }
            Part::Eez(k) | Part::Ts12(k) | Part::NeLake(k) => {
                let key = part.to_string();
                let Some(data) = ctx.data else {
                    ps.push(problem("rg-data", r, path, vec![key.clone()], format!("{}: {key}: the regions' datasets (sea zones, lakes, the coastline's land) aren't read now: their sources aren't declared, or not in sources/fetched (docs/inputs.md §6.1.7)", r.id)));
                    continue;
                };
                let got = match part {
                    Part::Eez(_) => data.eez.get(k).cloned().or_else(|| data.internal.get(k).map(|_| Vec::new())),
                    Part::Ts12(_) => data.ts12.get(k).cloned(),
                    _ => data.lakes.get(k).map(|l| l.0.clone()),
                };
                match got {
                    Some(rs) => {
                        // (A zone's internal waters a shape of their own: the shapes are unioned.)
                        let mut shapes = vec![(rs, 0)];
                        if let (Part::Eez(_), Some(iw)) = (part, data.internal.get(k)) {
                            shapes.push((iw.clone(), 0));
                        }
                        leaves.insert(key, Leaf::Rings(shapes, String::new()));
                    }
                    None => ps.push(problem(
                        "rg-data-missing",
                        r,
                        path,
                        vec![key.clone()],
                        format!("{}: {key}: the dataset ({}) has no such id", r.id, data.names),
                    )),
                }
            }
            Part::OsmWay(w) => {
                let key = part.to_string();
                match ctx.extract.and_then(|e| e.way(*w)) {
                    None => {
                        needs.insert(Id::W(*w));
                        ps.push(problem(
                            "rg-extract",
                            r,
                            path,
                            vec![format!("w{w}"), pass.into()],
                            format!("{}: {key}: waiting for the regions' extract of OSM {pass} to have it", r.id),
                        ));
                    }
                    Some(None) => ps.push(problem(
                        "rg-osm-missing",
                        r,
                        path,
                        vec![format!("w{w}"), pass.into()],
                        format!("{}: OSM {pass} has no way {w}", r.id),
                    )),
                    Some(Some(way)) if way.ends[0] != way.ends[1] || way.line.len() < 4 => ps.push(problem(
                        "rg-osm-no-area",
                        r,
                        path,
                        vec![key.clone()],
                        format!("{}: {key}: the way isn't closed, so it has no area", r.id),
                    )),
                    Some(Some(way)) => {
                        let ring = rings64(&[way.line[..way.line.len() - 1].to_vec()]);
                        leaves.insert(key, Leaf::Rings(vec![(ring, 0)], String::new()));
                    }
                }
            }
            Part::Box(b, _) => {
                leaves.insert(part.to_string(), Leaf::Rings(vec![(geom::box_area(*b).rings(), 0)], String::new()));
            }
            Part::Earth => {
                leaves.insert(part.to_string(), Leaf::Rings(vec![(geom::earth().rings(), 0)], String::new()));
            }
            Part::Land(d) => {
                // (Its own land: only its water names it, read once the land is resolved.)
                let Some(res) = lands.get(d).or_else(|| done.get(d)) else { continue };
                let shapes = res.geometry.shapes.iter().map(|s| (rings64(&s.rings), s.tolerance_m)).collect();
                leaves.insert(part.to_string(), Leaf::Rings(shapes, String::new()));
            }
            Part::Group(g) => {
                let mut shapes = Vec::new();
                for m in folder.members(g, &r.id) {
                    if let Some(res) = done.get(&m) {
                        shapes.extend(res.geometry.shapes.iter().map(|s| (rings64(&s.rings), 0)));
                    }
                }
                if folder.members(g, &r.id).is_empty() {
                    ps.push(problem(
                        "rg-ref",
                        r,
                        path,
                        vec![part.to_string()],
                        format!("{}: {part}: no region in that folder", r.id),
                    ));
                    continue;
                }
                leaves.insert(part.to_string(), Leaf::Rings(shapes, String::new()));
            }
            Part::Region(d) => {
                let res = &done[d];
                let shapes = res.geometry.shapes.iter().map(|s| (rings64(&s.rings), s.tolerance_m)).collect();
                leaves.insert(
                    part.to_string(),
                    Leaf::Rings(shapes, res.geometry.shapes.first().map(|s| s.country.clone()).unwrap_or_default()),
                );
            }
            _ => {}
        }
    }
    for (name, line) in &r.lines {
        match line {
            Line::Coords(c) => {
                leaves.insert(format!("line:{name}"), Leaf::Line(geom::unwrap(c)));
            }
            Line::Ways(ws) => {
                let mut got: Vec<&super::extract::Way> = Vec::new();
                for w in ws {
                    match ctx.extract.and_then(|e| e.way(*w)) {
                        None => {
                            needs.insert(Id::W(*w));
                            ps.push(problem("rg-extract", r, path, vec![format!("w{w}"), pass.into()], format!("{}: line {name}: way {w}: waiting for the regions' extract of OSM {pass} to have it", r.id)));
                        }
                        Some(None) => ps.push(problem(
                            "rg-osm-missing",
                            r,
                            path,
                            vec![format!("w{w}"), pass.into()],
                            format!("{}: line {name}: OSM {pass} has no way {w}", r.id),
                        )),
                        Some(Some(way)) => got.push(way),
                    }
                }
                if got.len() == ws.len() {
                    match string_ways(ws, &got) {
                        Ok(pts) => {
                            leaves.insert(format!("line:{name}"), Leaf::Line(geom::unwrap(&rings64(&[pts])[0])));
                        }
                        Err((a, b)) => ps.push(problem(
                            "rg-line",
                            r,
                            path,
                            vec![name.clone(), format!("w{a}"), format!("w{b}")],
                            format!("{}: line {name}: ways {a} and {b} don't join (no end node shared)", r.id),
                        )),
                    }
                }
            }
        }
    }
    Ok((leaves, drawn_osm, pass.to_string(), ps))
}

/// A region resolved: its outline (`water` false: for a region with water, its land), or its water
/// as written (`water` true, `done` then holding the regions' final geometry and `lands` their land).
pub(super) fn resolve_one(
    r: &Region,
    path: &str,
    folder: &Folder,
    ctx: &Context,
    done: &BTreeMap<String, Resolved>,
    lands: &BTreeMap<String, Resolved>,
    water: bool,
    needs: &mut BTreeSet<Id>,
) -> Result<Resolved, Vec<Problem>> {
    let (leaves, drawn_osm, pass, mut ps) = gather(r, path, folder, ctx, done, lands, water, needs)?;
    let pass = pass.as_str();
    if r.has_water() && ctx.data.is_none_or(|d| !d.has_land()) {
        ps.push(problem("rg-data", r, path, vec!["land".into()], format!("{}: a region with water is cut back to the coastline's land, which isn't read now (the pass's regions-land: docs/inputs.md §6.1.7)", r.id)));
    }
    if !ps.is_empty() {
        return Err(ps);
    }
    let names = (r.relations(), r.ways());
    if r.is_plain() {
        // One shape per part, unchanged.
        let mut shapes = Vec::new();
        let mut drawn = BTreeMap::new();
        for part in &r.outline {
            let key = part.to_string();
            let Some(Leaf::Rings(rs, country)) = leaves.get(&key) else { continue };
            let (rings, tol) = &rs[0];
            let tol = if matches!(part, Part::Osm(_)) { *tol } else { 0 };
            let rings32: Vec<Vec<[i32; 2]>> = rings.iter().map(|r| super::shapes::to_i32(r)).collect();
            let polys = match part {
                Part::Osm(id) if drawn_osm.contains_key(id) => drawn_osm[id].clone(),
                _ => {
                    let t = crate::coverage::draw_tolerance_m(&rings32);
                    rings32.iter().map(|r| vec![crate::outlines::simplify(r, t)]).collect()
                }
            };
            drawn.insert(key.clone(), degrees(&polys));
            shapes.push(Shape { from: key, country: country.clone(), tolerance_m: tol, rings: rings32, water: false });
        }
        return Ok(Resolved {
            geometry: Geometry { id: r.id.clone(), osm: pass.to_string(), shapes, relations: names.0, ways: names.1 },
            drawn: Drawn { fmt: 1, id: r.id.clone(), name: r.name.clone(), outline: r.outline_text(), shapes: drawn },
            lakes: Area::default(),
        });
    }
    // Computed: every leaf in the region's frame.
    let mut spans: Vec<[i64; 2]> = Vec::new();
    let mut unwrapped: BTreeMap<String, Leaf> = BTreeMap::new();
    for (k, leaf) in leaves {
        let l = match leaf {
            Leaf::Rings(shapes, c) => Leaf::Rings(
                shapes.into_iter().map(|(rs, t)| (rs.iter().map(|r| geom::unwrap(r)).collect(), t)).collect(),
                c,
            ),
            Leaf::Line(p) => Leaf::Line(p),
        };
        match &l {
            Leaf::Rings(shapes, _) => {
                for r in shapes.iter().flat_map(|s| s.0.iter()) {
                    if let Some(b) = geom::bbox_of(r.iter()) {
                        spans.push([b[0], b[2]]);
                    }
                }
            }
            Leaf::Line(p) => {
                if let Some(b) = geom::bbox_of(p.iter()) {
                    spans.push([b[0], b[2]]);
                }
            }
        }
        unwrapped.insert(k, l);
    }
    let frame = Frame::for_spans(&spans);
    let placed: BTreeMap<String, Leaf> = unwrapped
        .into_iter()
        .map(|(k, l)| {
            let l = match l {
                Leaf::Rings(shapes, c) => Leaf::Rings(
                    shapes.into_iter().map(|(rs, t)| (rs.iter().map(|r| frame.place(r)).collect(), t)).collect(),
                    c,
                ),
                Leaf::Line(p) => Leaf::Line(frame.place(&p)),
            };
            (k, l)
        })
        .collect();
    let eval_all = |parts: &[Part], direct: bool, ps: &mut Vec<Problem>| -> Area {
        let areas: Vec<Area> =
            parts.iter().filter_map(|p| eval(p, direct, &placed, r, path).map_err(|e| ps.push(e)).ok()).collect();
        geom::union_all(&areas)
    };
    // (A region with water: its outline's parts exact, no coast tolerance; cut back to the land.)
    let mut result =
        if water { eval_all(&r.water, false, &mut ps) } else { eval_all(&r.outline, !r.has_water(), &mut ps) };
    if !water && !r.intersect.is_empty() {
        result = result.intersect(&eval_all(&r.intersect, false, &mut ps));
    }
    if !water && !r.minus.is_empty() {
        result = result.minus(&eval_all(&r.minus, false, &mut ps));
    }
    if !ps.is_empty() {
        return Err(ps);
    }
    let lakes = Area::default();
    if result.is_empty() && !r.has_water() {
        return Err(vec![problem(
            "rg-empty",
            r,
            path,
            Vec::new(),
            format!("{}: the region has no area (a minus taking everything, an intersect meeting nothing?)", r.id),
        )]);
    }
    let mut from = r.outline_text().join(", ");
    if !r.intersect.is_empty() {
        from += &format!(" ∩ {}", r.intersect.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
    }
    if !r.minus.is_empty() {
        from += &format!(" − {}", r.minus.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
    }
    // The country: its first `osm:` part's (or a `region:`'s first shape's).
    let country = r
        .parts()
        .into_iter()
        .find_map(|p| match (p, placed.get(&p.to_string())) {
            (Part::Osm(_) | Part::Region(_), Some(Leaf::Rings(_, c))) if !c.is_empty() => Some(c.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let pieces = Frame::split(&result);
    let n = pieces.len();
    let lakes_plain: Vec<Area> = Frame::split(&lakes).into_iter().map(|p| p.1).collect();
    let mut shapes = Vec::new();
    let mut drawn = BTreeMap::new();
    for (k, (_, piece)) in pieces.into_iter().enumerate() {
        let f = piece_name(&from, k, n);
        let rings32: Vec<Vec<[i32; 2]>> = piece.rings().iter().map(|r| super::shapes::to_i32(r)).collect();
        let t = crate::coverage::draw_tolerance_m(&rings32);
        let polys: Vec<Vec<Vec<[i32; 2]>>> = piece
            .polys
            .iter()
            .map(|p| p.iter().map(|r| crate::outlines::simplify(&super::shapes::to_i32(r), t)).collect())
            .collect();
        drawn.insert(f.clone(), degrees(&polys));
        shapes.push(Shape { from: f, country: country.clone(), tolerance_m: 0, rings: rings32, water: false });
    }
    Ok(Resolved {
        geometry: Geometry { id: r.id.clone(), osm: pass.to_string(), shapes, relations: names.0, ways: names.1 },
        drawn: Drawn { fmt: 1, id: r.id.clone(), name: r.name.clone(), outline: r.outline_text(), shapes: drawn },
        lakes: geom::union_all(&lakes_plain),
    })
}

/// A computed region's stored shape's `from`, piece `k` of `n` its result splits into at the
/// antimeridian: the expression alone for one piece; "(to 180°)" and "(from -180°)" for the two
/// sides of one crossing; "(part k+1 of n)" past that (a result spanning more than a turn), so each
/// is unique.
pub fn piece_name(from: &str, k: usize, n: usize) -> String {
    match (n, k) {
        (1, _) => from.to_string(),
        (2, 0) => format!("{from} (to 180°)"),
        (2, _) => format!("{from} (from -180°)"),
        _ => format!("{from} (part {} of {n})", k + 1),
    }
}

fn degrees(polys: &[Vec<Vec<[i32; 2]>>]) -> Vec<Vec<Vec<[f64; 2]>>> {
    polys.iter().map(|p| p.iter().map(|r| crate::coverage::ring_degrees(r)).collect()).collect()
}

/// Ways strung together end to end in the order given, each reversed if that's what meets the
/// previous one's end (the first reversed too, if that's what meets the second); the pair that
/// doesn't join, when one doesn't.
fn string_ways(ids: &[u64], ways: &[&super::extract::Way]) -> Result<Vec<[i32; 2]>, (u64, u64)> {
    let mut pts: Vec<[i32; 2]> = ways[0].line.clone();
    let mut end = ways[0].ends[1];
    if ways.len() > 1 && !ways[1].ends.contains(&end) && ways[1].ends.contains(&ways[0].ends[0]) {
        pts.reverse();
        end = ways[0].ends[0];
    }
    for (k, w) in ways.iter().enumerate().skip(1) {
        if w.ends[0] == end {
            pts.extend_from_slice(&w.line[1..]);
            end = w.ends[1];
        } else if w.ends[1] == end {
            pts.extend(w.line[..w.line.len() - 1].iter().rev());
            end = w.ends[0];
        } else {
            return Err((ids[k - 1], ids[k]));
        }
    }
    Ok(pts)
}

/// A part's area in the region's frame (`direct`: listed in `outline` itself, where the coast
/// tolerance is a real 1 km buffer).
pub(super) fn eval(
    part: &Part,
    direct: bool,
    placed: &BTreeMap<String, Leaf>,
    r: &Region,
    path: &str,
) -> Result<Area, Problem> {
    match part {
        Part::Rest => Ok(Area::default()),
        Part::Union(ps) => {
            Ok(geom::union_all(&ps.iter().map(|p| eval(p, false, placed, r, path)).collect::<Result<Vec<_>, _>>()?))
        }
        Part::Intersect(ps) => {
            let mut a = eval(&ps[0], false, placed, r, path)?;
            for p in &ps[1..] {
                a = a.intersect(&eval(p, false, placed, r, path)?);
            }
            Ok(a)
        }
        Part::Minus(ps) => {
            let a = eval(&ps[0], false, placed, r, path)?;
            let rest: Vec<Area> = ps[1..].iter().map(|p| eval(p, false, placed, r, path)).collect::<Result<_, _>>()?;
            Ok(if rest.is_empty() { a } else { a.minus(&geom::union_all(&rest)) })
        }
        Part::Nearest { of, to, than, step_m, .. } => {
            let of = eval(of, false, placed, r, path)?;
            let to: Vec<Area> = to.iter().map(|p| eval(p, false, placed, r, path)).collect::<Result<_, _>>()?;
            let than: Vec<Area> = than.iter().map(|p| eval(p, false, placed, r, path)).collect::<Result<_, _>>()?;
            Ok(geom::nearest_growing(&of, &to, &than, *step_m))
        }
        Part::Osm(_)
        | Part::Shape(_)
        | Part::Region(_)
        | Part::OsmWay(_)
        | Part::Eez(_)
        | Part::Ts12(_)
        | Part::NeLake(_)
        | Part::Box(..)
        | Part::Earth
        | Part::Land(_)
        | Part::Group(_) => {
            let Some(Leaf::Rings(shapes, _)) = placed.get(&part.to_string()) else { unreachable!("every leaf read") };
            let areas: Vec<Area> = shapes
                .iter()
                .map(|(rs, tol)| {
                    let a = Area::from_rings(rs);
                    if direct && *tol > 0 {
                        geom::buffer_area(&a, *tol as f64)
                    } else {
                        a
                    }
                })
                .collect();
            Ok(geom::union_all(&areas))
        }
        Part::Buffer { of, m, .. } => match &**of {
            // (A region with water: the coarser buffer, within a fiftieth of the distance.)
            Target::Part(p) if r.has_water() => Ok(geom::buffer_area_coarse(&eval(p, false, placed, r, path)?, *m)),
            Target::Part(p) => Ok(geom::buffer_area(&eval(p, false, placed, r, path)?, *m)),
            Target::Line(l) => {
                let Some(Leaf::Line(pts)) = placed.get(&format!("line:{l}")) else { unreachable!("every line read") };
                Ok(geom::buffer_line(pts, *m))
            }
        },
        Part::Cut { of, line, left } => {
            let Some(Leaf::Line(pts)) = placed.get(&format!("line:{line}")) else { unreachable!("every line read") };
            if let Some((i, j)) = geom::self_crossing(pts, false) {
                return Err(problem(
                    "rg-line",
                    r,
                    path,
                    vec![line.clone(), "crosses".into()],
                    format!(
                        "{}: line {line} crosses itself (its segments {i} and {j}): a line that cuts may not",
                        r.id
                    ),
                ));
            }
            let a = eval(of, false, placed, r, path)?;
            geom::cut(&a, pts, *left).map_err(|geom::CutError::EndsInside(at)| {
                let at = [at[0].rem_euclid(geom::TURN) - if at[0].rem_euclid(geom::TURN) > geom::HALF { geom::TURN } else { 0 }, at[1]];
                problem("rg-cut", r, path, vec![line.clone(), format!("{:?}", [at[0] / 1_000_000, at[1] / 1_000_000])], format!("{}: line {line} ends inside a piece it cuts (near {:.4}, {:.4}): draw it on past the shape's edge", r.id, at[0] as f64 * 1e-7, at[1] as f64 * 1e-7))
            })
        }
    }
}
