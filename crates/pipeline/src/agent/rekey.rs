//! The build's records re-keyed when a step's key scheme changes (docs/plan.md §8, A new key
//! scheme), so that the change itself rebuilds only what it would make differently. A target whose
//! recorded key is what the old scheme computes now is current; it's recorded under the new scheme's
//! key when every input the new key names is pinned by the old key's (the same input, or a function
//! of what the old key named, of immutable sources, or of what's never been rewritten since), and
//! its record goes otherwise (it's built again). Safe because a target's outputs are a function of
//! the inputs its new key names (§8, Determinism), and those are what they were when it was built.
//! Any other record is left as it is: one stale under the old scheme is stale under the new too (its
//! key can't be either scheme's now). Idempotent: a new key never equals an old one, so a second
//! pass finds nothing. The build Mac's agent runs it under the build lock after merging hand-offs,
//! writing `state/build/jobs.json` whole when it changed anything, and on the keys it plans with
//! (a dry run plans as the switch will); `scenic-build rekey-check` shows what it would do. An old
//! scheme (`v1`) stays one release after its switch, for the records of jobs an older app ran.
//!
//! Units (2026-10-06): their keys named the terrain hi packs of the z6 tiles within 30 km
//! (`v1::unit_keys`); they name the terrain tiles a unit reads, by their contents
//! (build::unit_terrain). Of those, pinned:
//! - z9–12 tiles: by their hi pack, which the old key named;
//! - z8–z6 tiles of a z6 tile with a hi pack the old key named, that's a terrain piece now (near the
//!   coverage) in an area whose terrain is current: by that hi pack (a z8 tile is made from its raw
//!   tile and its z9 children alone, the z7 and z6 from those), the area's last run having had it as
//!   a piece;
//! - z8–z6 tiles of a z6 tile without a hi pack: by the raw tiles alone, never rewritten otherwise
//!   (the area's lo pack the new system's when the unit was built, or never rewritten:
//!   `rekey-check` shows the times).
//!
//! Not pinned: z5 and z4 tiles (each made from z6 tiles the old key mostly didn't name: the far
//! reaches of a long way), and z8–z6 tiles of a z6 tile whose hi pack is stale or whose area's
//! terrain is to be made again. A hi pack is stale when the coverage left its z6 tile, or when an
//! earlier run left it: a run that makes no hi tiles for a piece keeps its earlier pack
//! (terrain_pack::build_q_with), and makes the piece's z8–z6 from the raw tiles alone, which the old
//! key couldn't see; such a pack is older than its area's lo pack by the files' times
//! (`FileTimes::hi_older`). A unit without outputs (none of its ways in the coverage) is re-keyed
//! whatever it reads: the terrain doesn't decide which ways it keeps.
//!
//! Tree cover (2026-10-06): a z3 tile's whole run ("3/x/y", `v1::trees_targets`, keyed on the
//! coverage in the z3 tile) became a piece per z6 tile (keyed on the coverage in it) and an
//! assembly per z3 tile (keyed on its pieces' mids: crate::treepacks::targets). A z3 tile current
//! under the old scheme pins its pieces' inputs (the coverage in a z6 tile is a function of the
//! coverage in its z3 tile: Coverage::fingerprint), and its packs are what the pieces and assembly
//! make from them, byte for byte, when the trees program made them: its pieces and assembly are
//! recorded under their keys, without mids (none exist: "-" in the assembly's key), which are made
//! in idle time, expected the same (agent::build::TreeWork::backfill). Packs trees.py made (before
//! the program took its place, `TREES_PROGRAM_SINCE`, by their files' times) have the same pixels in
//! other bytes: not pinned, the z3 tile's record goes and its pieces are made again. Every z3
//! record goes: a stale one is built again as pieces either way, and one of "none" (the coverage
//! gone from it) has nothing left to build.
//!
//! A z3 record merged after the switch (a lease granted before it, handed off since; or an older
//! app's, rolled back to and forward again) comes with its whole run's packs, written over those of
//! the z3 tile's pieces and assembly made since. Current and the program's, it's re-keyed as above,
//! but for a piece recorded since under another key, or with a mid and no record: its mid isn't of
//! this coverage, so its record goes and it's made again. Otherwise it goes with the records of the
//! z3 tile's pieces and assembly, which are all made again.

use super::build::{self, Keys};
use super::tiles::{TerrainTiles, Tile};
use crate::coverage::Coverage;
use crate::legacy::Unit;
use crate::reach::Reaches;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// The key schemes before 2026-10-06: the units', and tree cover's before its pieces.
pub mod v1 {
    use super::super::build::{h, UNIT_V};
    use crate::coverage::Coverage;
    use crate::legacy::Unit;
    use crate::reach::{Reach, Reaches};
    use crate::treepacks::{LAYERS, TREES_V};
    use std::collections::BTreeMap;

    /// Tree cover's targets as they were, a z3 tile's whole run: each z3 tile the coverage meets,
    /// keyed on the step's version and the coverage there; and each z3 tile with tree packs (`m`,
    /// the build manifest) the coverage no longer meets, "none", whose run dropped them.
    pub fn trees_targets(cov: &Coverage, m: &BTreeMap<String, String>) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for x in 0..8 {
            for y in 0..8 {
                let b = crate::hipack::tile_bounds(3, x, y);
                let t = format!("3/{x}/{y}");
                let had = || LAYERS.iter().any(|l| m.contains_key(&format!("layers/{l}/lo/3-{x}-{y}")) || m.range(format!("layers/{l}/hi/6-")..).take_while(|(k, _)| k.starts_with(&format!("layers/{l}/hi/6-"))).any(|(k, _)| Unit::parse(&k[format!("layers/{l}/hi/").len()..]).is_some_and(|u| (u.x >> 3, u.y >> 3) == (x, y))));
                if cov.meets_rect(b) {
                    out.push((t.clone(), store::naming::hash16(format!("trees {TREES_V}|{t}|{}", cov.fingerprint(b)).as_bytes())));
                } else if had() {
                    out.push((t.clone(), trees_none(&t)));
                }
            }
        }
        out
    }

    /// Terrain's and slope's targets as they were, a z3 tile's whole run: each z3 tile with z6 tiles
    /// near the coverage, terrain keyed on its version, those z6 tiles, the coverage within 20 km,
    /// GLO-30 and the basemap; slope on its version, the area's terrain lo pack and its z6 tiles'
    /// hi packs.
    pub fn terrain_slope_targets(cov: &Coverage, m: &BTreeMap<String, String>) -> Targets {
        terrain_slope_targets_with(cov, m, crate::terrain_pack::water_pin(m).map_or("-", |(_, c)| c))
    }

    /// `terrain_slope_targets` with the basemap `water` (its content name, "-" for none) in the
    /// terrain's keys: one the terrain may have been made from before the latest.
    pub fn terrain_slope_targets_with(cov: &Coverage, m: &BTreeMap<String, String>, water: &str) -> Targets {
        use super::super::build::{coverage_tiles, grown_e7, SLOPE_V, TERRAIN_V};
        let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
        let (mut terrain, mut slope) = (Vec::new(), Vec::new());
        for (q, ts) in &coverage_tiles(cov) {
            let qs = format!("3/{}/{}", q.0, q.1);
            let tlist: Vec<String> = ts.iter().map(|t| format!("6/{}/{}", t.0, t.1)).collect();
            terrain.push((qs.clone(), h(&[&format!("terrain {TERRAIN_V}"), &tlist.join(" "), &cov.fingerprint(grown_e7(3, q.0, q.1, 20.0)), crate::terrain_pack::NORTH_PIN, water])));
            let mut inputs = vec![format!("slope {SLOPE_V}"), get(&format!("layers/terrain/lo/3-{}-{}", q.0, q.1)).to_string()];
            inputs.extend(ts.iter().map(|t| get(&format!("layers/terrain/hi/6-{}-{}", t.0, t.1)).to_string()));
            let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
            slope.push((qs, h(&refs)));
        }
        (terrain, slope)
    }

    /// Terrain's and slope's targets with their keys.
    pub type Targets = (Vec<(String, String)>, Vec<(String, String)>);

    /// A z3 tile's "none" key under the old scheme (the coverage gone from it).
    pub fn trees_none(t: &str) -> String {
        store::naming::hash16(format!("trees {TREES_V}|{t}|none").as_bytes())
    }

    /// The units the coverage builds, each with its key as it was (`unit_key`).
    pub fn unit_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, reach: Option<&Reaches>, digests: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
        let mut units: Vec<(Unit, String)> = Vec::new();
        let Some(reach) = reach else { return units };
        for (l, c) in m.range(format!("sources/osm/{date}/pieces/")..) {
            let Some(u) = l.strip_prefix(&format!("sources/osm/{date}/pieces/")).and_then(Unit::parse) else { break };
            let Some(r) = reach.get(u).filter(|r| r.builds(cov)) else { continue };
            units.push((u, unit_key(cov, date, m, u, c, r, digests)));
        }
        units
    }

    /// Unit `u`'s key as it was: its piece and road values, the coverage as its ways meet it, the
    /// location rules where they go, Taiwan's MOI DTM, the roadside buildings' index, and per z6
    /// tile within 30 km the terrain hi pack and the heritage slices.
    pub fn unit_key(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, u: Unit, piece: &str, r: &Reach, digests: &BTreeMap<String, String>) -> String {
        let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
        let mut inputs = vec![format!("unit {UNIT_V}"), piece.to_string(), get(&format!("sources/osm/{date}/roads/{}", u.dash())).to_string(), r.coverage_key(cov, u), crate::rules::versions_meeting(r.extent(u))];
        if crate::rules::meets_taiwan(r.extent(u)) {
            inputs.push(format!("moi-dtm {}", digests.get("moi-dtm").map(String::as_str).unwrap_or("-")));
        }
        inputs.push(format!("buildings {}", get(&crate::buildtiles::index_logical())));
        let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
        for (x, y) in crate::stage::tiles_in(6, b) {
            inputs.push(get(&format!("layers/terrain/hi/6-{x}-{y}")).to_string());
            inputs.push(get(&crate::heritage::pos_logical(date, x, y)).to_string());
            inputs.push(get(&crate::heritage::areas_logical(date, x, y)).to_string());
        }
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        h(&refs)
    }
}

/// Terrain's keys from its pieces (2026-10-08) to their water's digests (2026-10-09): a piece and an
/// assembly named the basemap their water comes from, by content name.
pub mod v2 {
    use super::super::build::{h, TERRAIN_LO_V, TERRAIN_V};

    /// Terrain piece `t`'s key as it was: `fp` the coverage within 20 km, `basemap` its content name.
    pub fn terrain_piece_key(t: (u32, u32), fp: &str, basemap: &str) -> String {
        h(&[&format!("terrain {TERRAIN_V}"), &format!("6/{}/{}", t.0, t.1), fp, crate::terrain_pack::NORTH_PIN, basemap])
    }

    /// Terrain's assembly of z3 tile `q`'s key as it was.
    pub fn terrain_lo_key(q: (u32, u32), basemap: &str, mids: &[String]) -> String {
        h(&[&format!("terrain-lo {TERRAIN_LO_V}"), &format!("terrain {TERRAIN_V}"), &format!("3/{}/{}", q.0, q.1), basemap, &mids.join(",")])
    }
}

/// What `rekey` did to the units' records (each by "6/x/y") and tree cover's (by z3 tile, "3/x/y").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rekeyed {
    /// Current under the old scheme, recorded under the new one now.
    pub moved: Vec<String>,
    /// Those of `moved` without outputs.
    pub empty: Vec<String>,
    /// Current under the old scheme, but naming tiles its old key didn't pin: their records gone
    /// (built again), each with why (a line a kind of tile).
    pub left: Vec<(String, Vec<String>)>,
    /// Current under the old scheme, but what they read can't be told now (a terrain pack's index
    /// unread): kept as they are, for the next pass.
    pub unknown: Vec<(String, String)>,
    /// Tree cover's z3 tiles current under the old scheme, recorded as their pieces and assembly.
    pub trees_moved: Vec<String>,
    /// Those whose records went, each with why: their packs trees.py's, stale under the old scheme
    /// (each built again as pieces), or "none" (nothing left to build); with them the records of
    /// their pieces and assembly made since the switch (a late record's whole run wrote over their
    /// packs).
    pub trees_dropped: Vec<(String, String)>,
    /// Those whose packs' times can't be read now: kept as they are, for the next pass.
    pub trees_unknown: Vec<(String, String)>,
}

impl Rekeyed {
    /// Whether it changed the records.
    pub fn changed(&self) -> bool {
        !self.moved.is_empty() || !self.left.is_empty() || !self.trees_moved.is_empty() || !self.trees_dropped.is_empty()
    }
}

/// When the trees program took trees.py's place in the published app (20261006-0711-5e3c69a,
/// 2026-10-06 07:11 UTC): tree packs written before are trees.py's, the same pixels in other WebP
/// bytes than the program's pieces make.
pub const TREES_PROGRAM_SINCE: i64 = 1_791_270_660;

/// What the re-keying reads of the NAS beyond the records and the manifest: its files' times
/// (`FileTimes`).
pub trait Times {
    /// Whether z6 tile (x, y)'s terrain hi pack was left by an earlier run (`FileTimes::hi_older`).
    fn hi_older(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool>;
    /// Whether z3 tile (x, y)'s tree packs were made by the trees program
    /// (`FileTimes::trees_by_program`).
    fn trees_by_program(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool>;
}

impl Times for FileTimes<'_> {
    fn hi_older(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
        FileTimes::hi_older(self, m, x, y)
    }
    fn trees_by_program(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
        FileTimes::trees_by_program(self, m, x, y)
    }
}

/// How much earlier than its area's lo pack a run can write a piece's hi pack: a run writes (or
/// touches: the same bytes) each piece's hi pack as it's done, then the area's lo pack; the largest
/// area's took 14 minutes on 2026-10-06, and a hi pack an earlier run left was 90 minutes older or
/// more.
pub const SAME_RUN_S: i64 = 3600;

/// The NAS's files' times, each read once.
pub struct FileTimes<'a> {
    root: &'a Path,
    seen: RefCell<HashMap<String, Option<i64>>>,
}

impl<'a> FileTimes<'a> {
    pub fn new(root: &'a Path) -> FileTimes<'a> {
        FileTimes { root, seen: RefCell::default() }
    }

    /// A file's time (seconds since the epoch), by content name; None when it can't be read now.
    fn time(&self, content: &str) -> Option<i64> {
        if let Some(&t) = self.seen.borrow().get(content) {
            return t;
        }
        let t = std::fs::metadata(self.root.join(content)).and_then(|md| md.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
        self.seen.borrow_mut().insert(content.to_string(), t);
        t
    }

    /// Whether z6 tile (x, y)'s terrain hi pack, as `m` names it, was written over `SAME_RUN_S`
    /// before its area's lo pack: an earlier run left it. Not when either isn't named; None when
    /// a file's time can't be read now.
    pub fn hi_older(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
        let (Some(hi), Some(lo)) = (m.get(&format!("layers/terrain/hi/6-{x}-{y}")), m.get(&format!("layers/terrain/lo/3-{}-{}", x >> 3, y >> 3))) else { return Some(false) };
        Some(self.time(lo)? - self.time(hi)? > SAME_RUN_S)
    }

    /// Whether z3 tile (x, y)'s tree packs, as `m` names them, were made by the trees program: its
    /// whole run's, all written together, by the time of its lo pack (its first layer's that has
    /// one, else its first hi pack) against `TREES_PROGRAM_SINCE`. With none, true: none to differ.
    /// None when the file's time can't be read now.
    pub fn trees_by_program(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
        let lo = crate::treepacks::LAYERS.iter().find_map(|l| m.get(&format!("layers/{l}/lo/3-{x}-{y}")));
        let hi = || {
            crate::treepacks::LAYERS.iter().find_map(|l| {
                let prefix = format!("layers/{l}/hi/");
                m.range(prefix.clone()..).take_while(|(k, _)| k.starts_with(&prefix)).find(|(k, _)| Unit::parse(&k[prefix.len()..]).is_some_and(|u| (u.x >> 3, u.y >> 3) == (x, y))).map(|(_, c)| c)
            })
        };
        let Some(c) = lo.or_else(hi) else { return Some(true) };
        Some(self.time(c)? >= TREES_PROGRAM_SINCE)
    }
}

/// Re-keys the records in `keys`, for the coverage `cov`, the pass `date`, the manifest `m`, the
/// reaches, `digests` (agent::input_digests) and the terrain packs' indexes: tree cover's from a z3
/// tile's whole run to pieces and assemblies (`rekey_trees`), and the units' from the old scheme
/// (`v1::unit_keys`) to the new (`build::unit_keys`). `times`: the NAS's files' times (`FileTimes`;
/// None where they can't be told now).
#[allow(clippy::too_many_arguments)]
pub fn rekey(keys: &mut Keys, cov: &Coverage, date: &str, m: &BTreeMap<String, String>, reach: Option<&Reaches>, digests: &BTreeMap<String, String>, tiles: &TerrainTiles, times: &dyn Times) -> Rekeyed {
    let mut out = Rekeyed::default();
    rekey_trees(keys, cov, m, times, &mut out);
    let older = |x: u32, y: u32| times.hi_older(m, x, y);
    let Some(reach) = reach else { return out };
    // (The units recorded, not all the pass's: whether the coverage builds a unit is a test of its
    // ways, seconds for all a pass's reaches.)
    let mut current: Vec<Unit> = Vec::new();
    for (t, k) in &keys.unit {
        let Some(u) = Unit::parse(t) else { continue };
        let (Some(r), Some(piece)) = (reach.get(u).filter(|r| r.builds(cov)), m.get(&format!("sources/osm/{date}/pieces/{}", u.dash()))) else { continue };
        if v1::unit_key(cov, date, m, u, piece, r, digests) == *k {
            current.push(u);
        }
    }
    if current.is_empty() {
        return out;
    }
    let pieces: BTreeSet<(u32, u32)> = build::coverage_tiles(cov).into_values().flatten().collect();
    // (An area's terrain is current by its whole run's record, or by its pieces' and assembly's.)
    let (terrain, _) = v1::terrain_slope_targets(cov, m);
    let mut terrain_now: BTreeSet<String> = terrain.into_iter().filter(|(q, k)| keys.terrain.get(q) == Some(k)).map(|(q, _)| q).collect();
    let tw = build::terrain_work(&build::terrain_slope_targets(cov, m, tiles), m, keys);
    terrain_now.extend(build::coverage_tiles(cov).into_keys().map(|q| format!("3/{}/{}", q.0, q.1)).filter(|q| !tw.terrain_left.contains(q) && keys.terrain_lo.contains_key(q)));
    for u in current {
        let t = u.slash();
        let (Some(r), Some(piece)) = (reach.get(u), m.get(&format!("sources/osm/{date}/pieces/{}", u.dash()))) else { continue };
        let read = match build::unit_terrain_tiles(u, r, m, tiles) {
            Ok(read) => read,
            Err(e) => {
                out.unknown.push((t, e.0));
                continue;
            }
        };
        let outputs = crate::out::UNIT_OUTPUTS.iter().any(|p| m.contains_key(&format!("{p}{}", u.dash())));
        let why = match if outputs { unpinned(u, &read, m, &pieces, &terrain_now, &older) } else { Ok(Vec::new()) } {
            Ok(why) => why,
            Err(e) => {
                out.unknown.push((t, e));
                continue;
            }
        };
        if why.is_empty() {
            keys.unit.insert(t.clone(), build::unit_key(cov, date, m, u, piece, r, &build::terrain_digest(&read)));
            if !outputs {
                out.empty.push(t.clone());
            }
            out.moved.push(t);
        } else {
            keys.unit.remove(&t);
            out.left.push((t, why));
        }
    }
    out
}

/// Re-keys tree cover's records of a z3 tile's whole run ("3/x/y", `v1::trees_targets`) as its
/// pieces and assembly (crate::treepacks::targets), into `out`: one current under the old scheme
/// whose packs the trees program made is recorded as its pieces (each the coverage meets) and its
/// assembly, under their keys now (a piece recorded since under another key, or with a mid and no
/// record, made again: its mid isn't of this coverage); every other z3 record goes (stale,
/// trees.py's packs, or "none"), and with it the records of the z3 tile's pieces and assembly (its
/// whole run, merged after the switch, wrote over their packs); one whose packs' times can't be
/// read now is kept for the next pass.
fn rekey_trees(keys: &mut Keys, cov: &Coverage, m: &BTreeMap<String, String>, times: &dyn Times, out: &mut Rekeyed) {
    let z3: Vec<(String, String)> = keys.trees.iter().filter(|(t, _)| Unit::parse(t).is_some_and(|u| u.z == 3)).map(|(t, k)| (t.clone(), k.clone())).collect();
    if z3.is_empty() {
        return;
    }
    let old: BTreeMap<String, String> = v1::trees_targets(cov, m).into_iter().collect();
    let tt = crate::treepacks::targets(cov, m);
    let has_mid = |t: &str| Unit::parse(t).is_some_and(|p| m.contains_key(&crate::treepacks::mid_logical(p.x, p.y)));
    for (q, k) in z3 {
        let Some(u) = Unit::parse(&q) else { continue };
        let why = match old.get(&q) {
            Some(now) if *now == k && *now == v1::trees_none(&q) => "current, the coverage gone from it: nothing left to build",
            Some(now) if *now == k => match times.trees_by_program(m, u.x, u.y) {
                None => {
                    out.trees_unknown.push((q, "its tree packs' times can't be read now".into()));
                    continue;
                }
                Some(false) => "its packs are trees.py's (made before the trees program, 2026-10-06 07:11 UTC): the same pixels in other bytes than the program's pieces make; made again as pieces",
                Some(true) => {
                    for (t, kt, _) in tt.pieces_of(&q) {
                        match keys.trees.get(t) {
                            Some(r) if r == kt => {}
                            None if !has_mid(t) => {
                                keys.trees.insert(t.clone(), kt.clone());
                            }
                            _ => {
                                keys.trees.remove(t);
                            }
                        }
                    }
                    if let Some((_, kl, _)) = tt.lo.iter().find(|l| l.0 == q) {
                        keys.trees_lo.insert(q.clone(), kl.clone());
                    }
                    keys.trees.remove(&q);
                    out.trees_moved.push(q);
                    continue;
                }
            },
            _ => "stale under the old scheme: made again as pieces either way",
        };
        keys.trees.remove(&q);
        let since: Vec<String> = keys.trees.keys().filter(|t| crate::treepacks::area_of(t).as_deref() == Some(q.as_str())).cloned().collect();
        for t in &since {
            keys.trees.remove(t);
        }
        let lo = keys.trees_lo.remove(&q).is_some();
        let why = match (since.len(), lo) {
            (0, false) => why.to_string(),
            (n, lo) => format!("{why}; with the records of its {n} pieces{} made since the switch, its whole run written over their packs: made again", if lo { " and its assembly" } else { "" }),
        };
        out.trees_dropped.push((q, why));
    }
}

/// Why the tiles `read` that unit `u`'s new key names aren't all pinned by its old key: a line a
/// kind of tile that isn't, with how many and the first; none when they all are; an error when
/// that can't be told now. `pieces`: the z6 tiles near the coverage; `terrain_now`: the areas whose
/// terrain is current; `older`: as `rekey`'s.
fn unpinned(u: Unit, read: &[Tile], m: &BTreeMap<String, String>, pieces: &BTreeSet<(u32, u32)>, terrain_now: &BTreeSet<String>, older: &dyn Fn(u32, u32) -> Option<bool>) -> Result<Vec<String>, String> {
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    // The z6 tiles whose hi packs its old key named.
    let [x0, x1, y0, y1] = crate::stage::tile_range(6, b);
    let named = |x: u32, y: u32| (x0..=x1).contains(&x) && (y0..=y1).contains(&y);
    let mut kinds: BTreeMap<&str, (usize, (u8, u32, u32))> = BTreeMap::new();
    for &(z, x, y, _) in read {
        let kind = if z <= 5 {
            Some("zoomed-out tiles (z5–z4), made from z6 tiles its old key didn't name")
        } else {
            let (x6, y6) = (x >> (z - 6), y >> (z - 6));
            if !named(x6, y6) {
                Some("tiles of z6 tiles whose hi packs its old key didn't name")
            } else if z <= 8 && m.contains_key(&format!("layers/terrain/hi/6-{x6}-{y6}")) {
                if !pieces.contains(&(x6, y6)) {
                    Some("z8–z6 tiles of a z6 tile whose hi pack is stale (the coverage left it)")
                } else if !terrain_now.contains(&format!("3/{}/{}", x6 >> 3, y6 >> 3)) {
                    Some("z8–z6 tiles of an area whose terrain is to be made again")
                } else {
                    match older(x6, y6) {
                        Some(true) => Some("z8–z6 tiles of a z6 tile whose hi pack is stale (an earlier run left it: older than its area's lo pack)"),
                        Some(false) => None,
                        None => return Err(format!("the times of 6/{x6}/{y6}'s terrain packs can't be read now")),
                    }
                }
            } else {
                None
            }
        };
        if let Some(k) = kind {
            kinds.entry(k).or_insert((0, (z, x, y))).0 += 1;
        }
    }
    Ok(kinds.into_iter().map(|(k, (n, (z, x, y)))| format!("{k}: {n}, {z}/{x}/{y} first")).collect())
}


/// What `derive` made of terrain's and slope's records of a z3 tile's whole run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Derived {
    /// Terrain's z3 tiles current under the old scheme: read as their pieces and assembly.
    pub terrain: Vec<String>,
    /// Slope's z3 tiles current under the old scheme: read as their pieces (those that read their
    /// area's terrain alone) and, with no other piece, their assembly.
    pub slope: Vec<String>,
    /// Slope's pieces in those that read terrain in another area's packs or the root, or none:
    /// left to make, each with the first such tile.
    pub slope_left: Vec<(String, String)>,
    /// The old records stale under the old scheme (or "none"): passed over, each with why.
    pub stale: Vec<(String, String)>,
    /// Slope's z3 tiles whose pieces' reads can't be told now (a terrain pack's index unread), and
    /// terrain's whose water's digests aren't there (yet): their records kept, for the next read.
    pub unknown: Vec<(String, String)>,
    /// Terrain's z3 tiles of `terrain` made from a basemap before the latest: each with it.
    pub older: Vec<(String, String)>,
    /// Terrain's pieces and assemblies recorded under the keys that named the basemap (`v2`): read
    /// under their keys now, each with the basemap.
    pub water: Vec<(String, String)>,
}

/// The records as every reader reads them (the plan, the checklist, the regions' state, the jobs'
/// `--expect-same`, `scenic-build p5-check`): re-keyed (`rekey`), then terrain's and slope's
/// derived (`derive`). The one way to read them.
#[allow(clippy::too_many_arguments)]
pub fn as_read(keys: &mut Keys, cov: &Coverage, date: &str, m: &BTreeMap<String, String>, reach: Option<&Reaches>, digests: &BTreeMap<String, String>, tiles: &TerrainTiles, times: &dyn Times) -> (Rekeyed, Derived) {
    let r = rekey(keys, cov, date, m, reach, digests, tiles, times);
    let d = derive(keys, cov, m, tiles);
    (r, d)
}

/// Terrain's and slope's records of a z3 tile's whole run ("3/x/y", `v1::terrain_slope_targets`)
/// read as its pieces and assemblies (build::terrain_slope_targets), in memory only: never saved
/// (the records, `jobs.json` or the pool's terms', change only by a job's save), so applied again
/// by every reader (`as_read`), the same each time. A record of the new scheme is never written
/// over: what a job recorded wins over what's derived. The z3 record itself is dropped from the
/// keys read.
///
/// A z3 tile's terrain record current under the old scheme (its key: the version, its z6 tiles near
/// the coverage, the coverage within 20 km of it, GLO-30 and the basemap) pins its pieces' inputs
/// (the coverage within 20 km of a z6 tile in it is a function of that), and its packs are what its
/// pieces and assembly make from them, byte for byte (terrain_pack::build_q_with is made of them):
/// each piece without a record of its own is read as current under its key, and the assembly, with
/// none, under its key with every mid "-" (as when no piece had one): a mid made since (a piece made
/// again, or backfilled) makes it stale, and it's made again, from all its pieces' mids (the mids
/// needed: build::terrain_work).
///
/// A z3 tile's slope record current under the old scheme (its key: the version, the area's terrain
/// lo pack and its pieces' hi packs) pins the reads of its pieces whose every terrain tile read
/// (build::slope_piece_reads) is in its area's packs: each without a record of its own is read as
/// current. Those reading another area's terrain, or the root's, or none, which the old key didn't
/// name (and a slope area's run could read a neighbour's terrain before it was made again), are
/// left to make; the assembly is read as current (every mid "-") only when none is.
///
/// The terrain's keys name the digest of the water each target reads (build::water_key; they
/// named the basemap before 2026-10-09, `v2`). A z3 tile's terrain record is read by the digests
/// of the basemap it was made from: the latest's, else an earlier one's whose digests the manifest
/// names (its v1 key names that basemap), so its pieces made from an earlier basemap are current
/// where the water they read is the latest's; one whose basemap's digests aren't there yet is kept
/// as it is (`Derived::unknown`). A piece's or an assembly's record under its `v2` key (an older
/// app's job), of the latest basemap or an earlier one with digests, is read under its key now by
/// that basemap's digests (its mids those of the manifest, or all "-").
pub fn derive(keys: &mut Keys, cov: &Coverage, m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> Derived {
    let mut out = Derived::default();
    let z3 = |map: &BTreeMap<String, String>| -> Vec<(String, String)> { map.iter().filter(|(t, _)| Unit::parse(t).is_some_and(|u| u.z == 3)).map(|(t, k)| (t.clone(), k.clone())).collect() };
    let (tz3, sz3) = (z3(&keys.terrain), z3(&keys.slope));
    let v2 = keys.terrain.iter().any(|(t, _)| Unit::parse(t).is_some_and(|u| u.z == 6)) || !keys.terrain_lo.is_empty();
    if tz3.is_empty() && sz3.is_empty() && !v2 {
        return out;
    }
    let by_q = build::coverage_tiles(cov);
    let tt = build::terrain_slope_targets(cov, m, tiles);
    let pieces = |q: &str| -> Vec<(u32, u32)> { Unit::parse(q).and_then(|u| by_q.get(&(u.x, u.y)).cloned()).unwrap_or_default() };
    let none_mids = |q: &str| -> Vec<String> { pieces(q).iter().map(|(x, y)| format!("6/{x}/{y}=-")).collect() };
    // The basemaps the terrain may have been made from: the latest's, then each earlier one whose
    // water's digests the manifest names (crate::terrain_water::WaterIdx). A record made from an
    // earlier one is read under the key its water's digests there give: current when the latest's
    // are the same.
    let latest = crate::terrain_pack::water_source_pin(m);
    let mut basemaps: Vec<Option<String>> = vec![latest.clone()];
    basemaps.extend(tiles.water_pins(m).into_iter().filter(|p| Some(p) != latest.as_ref()).map(Some));
    let water = |b: &Option<String>, t: &str, from: &str| build::water_key(b.as_deref(), b.as_deref().and_then(|p| tiles.water_idx(m, p)), t, from);
    let fp = |x: u32, y: u32| cov.fingerprint(build::grown_e7(6, x, y, 20.0));
    // Pieces and assemblies recorded under the keys that named the basemap (`v2`: an older app's).
    if v2 {
        let current: BTreeMap<&String, &String> = tt.terrain.iter().chain(&tt.terrain_lo).map(|(t, k)| (t, k)).collect();
        let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
        let mids = |q: &str| -> Vec<String> { pieces(q).iter().map(|(x, y)| format!("6/{x}/{y}={}", get(&crate::terrain_pack::mid_logical(*x, *y)))).collect() };
        let moved: Vec<(String, String, String)> = keys.terrain.iter().filter(|(t, k)| current.get(t) != Some(k)).filter_map(|(t, k)| {
            let u = Unit::parse(t).filter(|u| u.z == 6)?;
            let f = fp(u.x, u.y);
            basemaps.iter().find(|b| v2::terrain_piece_key((u.x, u.y), &f, b.as_deref().unwrap_or("-")) == *k).and_then(|b| Some((t.clone(), build::terrain_piece_key((u.x, u.y), &f, &water(b, t, &f)?), b.clone().unwrap_or_default())))
        }).collect();
        for (t, k, b) in moved {
            keys.terrain.insert(t.clone(), k);
            out.water.push((t, b));
        }
        let moved: Vec<(String, String, String)> = keys.terrain_lo.iter().filter(|(q, k)| current.get(q) != Some(k)).filter_map(|(q, k)| {
            let u = Unit::parse(q).filter(|u| u.z == 3)?;
            let lists = [mids(q), none_mids(q)];
            basemaps.iter().find_map(|b| lists.iter().find(|l| v2::terrain_lo_key((u.x, u.y), b.as_deref().unwrap_or("-"), l) == *k).map(|l| (b, l))).and_then(|(b, l)| Some((q.clone(), build::terrain_lo_key((u.x, u.y), &water(b, q, crate::terrain_water::AREA_FROM)?, l), b.clone().unwrap_or_default())))
        }).collect();
        for (q, k, b) in moved {
            keys.terrain_lo.insert(q.clone(), k);
            out.water.push((q, b));
        }
    }
    if tz3.is_empty() && sz3.is_empty() {
        return out;
    }
    // (The old scheme's keys, kept by `tiles` with what they're worked out from, as the new: the
    // latest basemap's, an earlier one's when a record isn't current with it.)
    let named = |prefix: &str| m.range(prefix.to_string()..).take_while(|(l, _)| l.starts_with(prefix)).map(|(l, c)| format!("{l}={c}")).collect::<Vec<_>>().join(",");
    let old_with = |b: &str| -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        let from = store::naming::hash16([build::coverage_all(cov), named("layers/terrain/"), b.to_string()].join("\n").as_bytes());
        let key = if latest.as_deref().unwrap_or("-") == b { "terrain-targets v1".to_string() } else { format!("terrain-targets v1 {b}") };
        let old = tiles.memo(&key, &from, || Ok(serde_json::to_string(&v1::terrain_slope_targets_with(cov, m, b)).unwrap_or_default()));
        let (t, s): (Vec<(String, String)>, Vec<(String, String)>) = old.ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_else(|| v1::terrain_slope_targets_with(cov, m, b));
        (t.into_iter().collect(), s.into_iter().collect())
    };
    let (old_t, old_s) = old_with(latest.as_deref().unwrap_or("-"));
    let mut earlier: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (q, k) in tz3 {
        let Some(u) = Unit::parse(&q) else { continue };
        let b = if old_t.get(&q) == Some(&k) {
            Some(latest.clone())
        } else {
            basemaps.iter().skip(1).find(|b| {
                let b = b.as_deref().unwrap_or("-");
                earlier.entry(b.to_string()).or_insert_with(|| old_with(b).0).get(&q) == Some(&k)
            }).cloned()
        };
        let Some(b) = b else {
            keys.terrain.remove(&q);
            out.stale.push((format!("terrain {q}"), "stale under the old scheme: its pieces made".into()));
            continue;
        };
        // Its pieces' and assembly's keys, by the water's digests of the basemap it was made from.
        let made: Option<Vec<(String, String)>> = pieces(&q).iter().map(|&(x, y)| {
            let t = format!("6/{x}/{y}");
            let f = fp(x, y);
            Some((t.clone(), build::terrain_piece_key((x, y), &f, &water(&b, &t, &f)?)))
        }).collect();
        let lo = water(&b, &q, crate::terrain_water::AREA_FROM).map(|w| build::terrain_lo_key((u.x, u.y), &w, &none_mids(&q)));
        let (Some(made), Some(lo)) = (made, lo) else {
            out.unknown.push((q, format!("the water's digests of {} aren't there yet (the terrain-water job): its record kept", b.as_deref().unwrap_or("-"))));
            continue;
        };
        keys.terrain.remove(&q);
        for (t, kt) in made {
            keys.terrain.entry(t).or_insert(kt);
        }
        keys.terrain_lo.entry(q.clone()).or_insert(lo);
        if b != latest {
            out.older.push((q.clone(), b.unwrap_or_default()));
        }
        out.terrain.push(q);
    }
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    for (q, k) in sz3 {
        if old_s.get(&q) != Some(&k) {
            keys.slope.remove(&q);
            out.stale.push((format!("slope {q}"), "stale under the old scheme: its pieces made".into()));
            continue;
        }
        let Some(u) = Unit::parse(&q) else { continue };
        // (A tile in the area's packs: its lo pack, or the hi pack of one of its z6 tiles.)
        let ours = |z: u8, x: u32, y: u32| z >= 3 && (x >> (z - 3), y >> (z - 3)) == (u.x, u.y);
        // (Every piece's reads told first: one that can't be leaves the record for the next pass.
        // Whether a piece reads outside its area, and the first such tile, kept with the packs it
        // read: a plan works it out again only when they change.)
        let mut reads: Vec<(String, Option<String>, String)> = Vec::new();
        let mut unread = None;
        for (t, kt) in tt.slope.iter().filter(|(t, _)| build::area_of(t).as_deref() == Some(q.as_str())) {
            let p = Unit::parse(t).unwrap();
            let outside = tiles.memo(&format!("slope-outside {t}"), &build::slope_piece_from((p.x, p.y), m), || {
                let r = build::slope_piece_reads((p.x, p.y), m, tiles)?;
                Ok(match r.iter().find(|x| !x.is_some_and(|(z, x, y, _)| ours(z, x, y))) {
                    None => String::new(),
                    Some(Some((z, x, y, _))) if *z <= 2 => format!("reads the terrain's root ({z}/{x}/{y})"),
                    Some(Some((z, x, y, _))) => format!("reads another area's terrain ({z}/{x}/{y})"),
                    Some(None) => "reads where there's no terrain tile".to_string(),
                })
            });
            match outside {
                Ok(why) => reads.push((t.clone(), kt.clone(), why)),
                Err(e) => {
                    unread = Some(e.0);
                    break;
                }
            }
        }
        if let Some(e) = unread {
            out.unknown.push((q, e));
            continue;
        }
        keys.slope.remove(&q);
        let mut left = 0;
        for (t, kt, why) in reads {
            if why.is_empty() {
                if let Some(kt) = kt {
                    keys.slope.entry(t).or_insert(kt);
                }
            } else {
                left += 1;
                out.slope_left.push((t, why));
            }
        }
        if left == 0 {
            keys.slope_lo.entry(q.clone()).or_insert_with(|| build::slope_lo_key((u.x, u.y), &none_mids(&q), get(&format!("layers/terrain/lo/3-{}-{}", u.x, u.y))));
        }
        out.slope.push(q);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::build::tests::{did, e7box, heritage_chain_done, heritage_done, tiles_for, unit_inputs};
    use crate::agent::build::{Rounds, Work};
    use crate::agent::recipes::Recipe;
    use crate::reach::{LongWay, Reach};

    /// The files' times as a test has them: whether a z6 tile's terrain hi pack is older than its
    /// area's lo pack, whether a z3 tile's tree packs are the trees program's.
    struct Stub<F, G>(F, G);

    impl<F: Fn(u32, u32) -> Option<bool>, G: Fn(u32, u32) -> Option<bool>> Times for Stub<F, G> {
        fn hi_older(&self, _: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
            (self.0)(x, y)
        }
        fn trees_by_program(&self, _: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
            (self.1)(x, y)
        }
    }

    type At = fn(u32, u32) -> Option<bool>;

    /// No hi pack older than its area's lo pack; every z3 tile's tree packs the program's.
    const NONE_OLDER: Stub<At, At> = Stub(|_, _| Some(false), |_, _| Some(true));

    /// Iceland: regions a (Reykjavik, unit 6/28/17) and b (Akureyri and Egilsstaðir: 6/28/16,
    /// 6/29/16), all in area 3/3/2; the heritage sites and the terrain done (its lo pack, and the hi
    /// packs of its pieces), each unit built under the old keys with its outputs. `reaches` changes
    /// the units' reaches first.
    fn iceland(reaches: &dyn Fn(&mut Reaches)) -> (Coverage, Reaches, BTreeMap<String, String>, Keys) {
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, places: &[&str]| Recipe { id: id.into(), name: id.to_uppercase(), outline: places.iter().map(|p| format!("place:{p},20")).collect() };
        let c = Coverage::from_recipes(&[r("a", &["-21.9,64.13"]), r("b", &["-18.1,65.68", "-14.4,65.26"])], None, d.path()).unwrap();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        for (u, b) in [("6/28/17", e7box(-22.0, 64.0, -21.7, 64.16)), ("6/28/16", e7box(-18.3, 65.6, -17.9, 65.8)), ("6/29/16", e7box(-14.6, 65.2, -14.2, 65.35))] {
            reach.units.insert(u.into(), Reach { owned: Some(b), long: vec![] });
        }
        reaches(&mut reach);
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-17", "6-28-16", "6-29-16"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        unit_inputs(&mut m, "d");
        let mut done = Keys::default();
        // (The terrain made by its area's whole run, as before its pieces.)
        m.insert("layers/terrain/lo/3-3-2".into(), "layers/terrain/lo/3-3-2.1111111111111111.pack".into());
        for (x, y) in build::coverage_tiles(&c).into_values().flatten() {
            m.insert(format!("layers/terrain/hi/6-{x}-{y}"), format!("layers/terrain/hi/6-{x}-{y}.2222222222222222.pack"));
        }
        done.record("terrain", &v1::terrain_slope_targets(&c, &m).0);
        loop {
            let w = plan(&c, &reach, &m, &done, &tiles_for(&m)).work;
            match w[0].step.as_str() {
                "heritage-sites" => heritage_done(&mut m, &mut done, "d", &w[0]),
                _ => break,
            }
        }
        heritage_chain_done(&c, &m, &mut done, "d");
        for (u, k) in v1::unit_keys(&c, "d", &m, Some(&reach), &BTreeMap::new()) {
            done.record("unit", &[(u.slash(), k)]);
            m.insert(format!("base/{}", u.dash()), format!("base/{}.6666666666666666.base", u.dash()));
        }
        (c, reach, m, done)
    }

    /// The plan, with the records as read (terrain's and slope's derived: `derive`).
    fn plan(c: &Coverage, reach: &Reaches, m: &BTreeMap<String, String>, done: &Keys, tiles: &TerrainTiles) -> build::Plan {
        let mut done = done.clone();
        derive(&mut done, c, m, tiles);
        build::plan(c, "d", m, &done, &BTreeMap::new(), Some(reach), tiles, Rounds { each: &c.by_region(), on_map: &BTreeMap::new(), since_last: None, current: None, held: false })
    }

    /// The units the plan builds.
    fn units(p: &build::Plan) -> Vec<String> {
        p.work.iter().filter(|w| w.step == "unit").flat_map(|w: &Work| w.targets.iter().map(|t| t.0.clone())).collect()
    }

    /// Terrain's and slope's work (build::terrain_work) for the records as read.
    fn tw(c: &Coverage, m: &BTreeMap<String, String>, done: &Keys) -> build::TerrainWork {
        let tiles = tiles_for(m);
        let mut read = done.clone();
        derive(&mut read, c, m, &tiles);
        build::terrain_work(&build::terrain_slope_targets(c, m, &tiles), m, &read)
    }

    fn names(v: &[(String, String)]) -> Vec<String> {
        v.iter().map(|t| t.0.clone()).collect()
    }

    #[test]
    fn terrain_current_under_the_old_scheme_is_read_as_its_pieces_and_assembly() {
        let (c, _, m, done) = iceland(&|_| {});
        let tiles = tiles_for(&m);
        let tt = build::terrain_slope_targets(&c, &m, &tiles);
        let pieces = names(&tt.terrain);
        assert!(pieces.len() >= 3, "{pieces:?}");
        let mut read = done.clone();
        let d = derive(&mut read, &c, &m, &tiles);
        assert_eq!(d.terrain, ["3/3/2"]);
        assert!(!read.terrain.contains_key("3/3/2"));
        assert!(tt.terrain.iter().all(|(t, k)| read.terrain.get(t) == Some(k)));
        // Nothing to make: the assembly current with its mids "-", its pieces' mids made in idle
        // time.
        let w = tw(&c, &m, &done);
        assert!(w.terrain.is_empty() && w.terrain_lo.is_empty() && w.terrain_left.is_empty(), "{w:?}");
        assert_eq!(w.backfill.iter().find(|b| b.step == "terrain").map(|b| names(&b.targets)), Some(pieces.clone()));
        // Twice is once.
        let mut twice = read.clone();
        assert_eq!(derive(&mut twice, &c, &m, &tiles), Derived::default());
        assert_eq!(twice, read);
        // A record of the new scheme wins over what's derived: a piece recorded under another key
        // is stale, and every other piece of its area is made again for its mid, the assembly then.
        let mut rec = done.clone();
        rec.terrain.insert(pieces[0].clone(), "0000000000000000".into());
        let mut r2 = rec.clone();
        derive(&mut r2, &c, &m, &tiles);
        assert_eq!(r2.terrain[&pieces[0]], "0000000000000000");
        let w = tw(&c, &m, &rec);
        assert_eq!(names(&w.terrain), pieces);
        assert!(w.terrain_left.contains("3/3/2") && w.terrain_lo.is_empty());
        // A mid backfilled: the assembly's key names it, so it's stale, and the other pieces' mids
        // are made for it (not idle work now).
        let mut m2 = m.clone();
        let l = crate::terrain_pack::mid_logical(28, 16);
        m2.insert(l.clone(), format!("{l}.5555555555555555.sect"));
        let w = tw(&c, &m2, &done);
        assert_eq!(names(&w.terrain), pieces.iter().filter(|t| *t != "6/28/16").cloned().collect::<Vec<_>>());
        assert!(w.backfill.iter().all(|b| b.step != "terrain") && w.terrain_lo.is_empty());
        // Every mid made: the assembly runs.
        for (x, y) in build::coverage_tiles(&c).into_values().flatten() {
            let l = crate::terrain_pack::mid_logical(x, y);
            m2.insert(l.clone(), format!("{l}.5555555555555555.sect"));
        }
        let w = tw(&c, &m2, &done);
        assert!(w.terrain.is_empty() && names(&w.terrain_lo) == ["3/3/2"], "{w:?}");
        // Stale under the old scheme: passed over, its pieces made.
        let mut stale = done.clone();
        stale.terrain.insert("3/3/2".into(), "0000000000000000".into());
        let mut s2 = stale.clone();
        let d = derive(&mut s2, &c, &m, &tiles);
        assert_eq!(d.stale.len(), 1);
        assert!(s2.terrain.is_empty() && s2.terrain_lo.is_empty());
        assert_eq!(names(&tw(&c, &m, &stale).terrain), pieces);
    }

    #[test]
    fn slope_current_under_the_old_scheme_keeps_its_pieces_that_read_their_area_alone() {
        let (c, reach, mut m, mut done) = iceland(&|_| {});
        m.insert("layers/slope/lo/3-3-2".into(), "layers/slope/lo/3-3-2.9999999999999999.pack".into());
        done.record("slope", &v1::terrain_slope_targets(&c, &m).1);
        let tiles = tiles_for(&m);
        let mut read = done.clone();
        let d = derive(&mut read, &c, &m, &tiles);
        assert_eq!(d.slope, ["3/3/2"]);
        // Those on the area's top row read 3/3/1's terrain (none: no pack there): made again.
        let left: Vec<&str> = d.slope_left.iter().map(|(t, _)| t.as_str()).collect();
        let pieces: Vec<(u32, u32)> = build::coverage_tiles(&c).into_values().flatten().collect();
        let top: Vec<String> = pieces.iter().filter(|p| p.1 == 16).map(|(x, y)| format!("6/{x}/{y}")).collect();
        assert_eq!(left, top.iter().map(String::as_str).collect::<Vec<_>>(), "{:?}", d.slope_left);
        assert!(pieces.iter().filter(|p| p.1 != 16).all(|(x, y)| read.slope.contains_key(&format!("6/{x}/{y}"))));
        assert!(!read.slope_lo.contains_key("3/3/2"), "its assembly after them");
        let p = plan(&c, &reach, &m, &done, &tiles);
        let slope: Vec<String> = p.work[..p.work.len() - p.backfill.len()].iter().filter(|w| w.step == "slope").flat_map(|w| names(&w.targets)).collect();
        assert_eq!(slope, top);
        // The others' mids made in idle time, expected the same.
        let idle: Vec<String> = p.backfill.iter().filter(|w| w.step == "slope").flat_map(|w| names(&w.targets)).collect();
        assert_eq!(idle, pieces.iter().filter(|p| p.1 != 16).map(|(x, y)| format!("6/{x}/{y}")).collect::<Vec<_>>());
        // Twice is once; a record of the new scheme wins.
        let mut twice = read.clone();
        assert_eq!(derive(&mut twice, &c, &m, &tiles), Derived::default());
        assert_eq!(twice, read);
        let (x, y) = *pieces.iter().find(|p| p.1 != 16).unwrap();
        let mut rec = done.clone();
        rec.slope.insert(format!("6/{x}/{y}"), "0000000000000000".into());
        derive(&mut rec, &c, &m, &tiles);
        assert_eq!(rec.slope[&format!("6/{x}/{y}")], "0000000000000000");
    }

    #[test]
    fn units_current_under_the_old_keys_are_re_keyed_once_and_not_built_again() {
        let (c, reach, m, mut done) = iceland(&|_| {});
        let tiles = tiles_for(&m);
        // Under the new keys as recorded: every unit would be built again.
        assert_eq!(units(&plan(&c, &reach, &m, &done, &tiles)).len(), 3);
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert_eq!((r.moved.len(), r.left.len(), r.unknown.len(), r.empty.len()), (3, 0, 0, 0), "{r:?}");
        assert!(r.changed());
        assert!(units(&plan(&c, &reach, &m, &done, &tiles)).is_empty());
        // Again: nothing to do, nothing changed.
        let before = done.clone();
        let again = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert_eq!(again, Rekeyed::default());
        assert!(!again.changed() && done == before);
        // A late record under the old key (a job an older app ran, merged since): re-keyed.
        let old = v1::unit_keys(&c, "d", &m, Some(&reach), &BTreeMap::new()).into_iter().find(|(u, _)| u.slash() == "6/28/16").unwrap().1;
        done.record("unit", &[("6/28/16".into(), old)]);
        assert_eq!(units(&plan(&c, &reach, &m, &done, &tiles)), ["6/28/16"]);
        assert_eq!(rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER).moved, ["6/28/16"]);
        assert!(units(&plan(&c, &reach, &m, &done, &tiles)).is_empty());
        // One stale under the old keys (its key isn't the old scheme's now): left as it is, built.
        done.record("unit", &[("6/29/16".into(), "0000000000000000".into())]);
        let before = done.clone();
        assert!(!rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER).changed());
        assert_eq!(done, before);
        assert_eq!(units(&plan(&c, &reach, &m, &done, &tiles)), ["6/29/16"]);
    }

    #[test]
    fn a_unit_reading_tiles_its_old_key_didnt_pin_is_built_again() {
        // Reykjavik's unit owns a road east along 64.1° N to 10° W, past its tile + 30 km, whose far
        // end reads the z4 tile; and its owned box reaches into 6/27/18, which has a stale hi pack
        // (no piece: the coverage is far) holding none of the tiles there, so it reads z8–z6 tiles.
        let road = |r: &mut Reaches| r.units.get_mut("6/28/17").unwrap().long = vec![LongWay { owned: true, ferry: false, verts: vec![[-218_000_000, 641_000_000], [-100_000_000, 641_000_000]] }];
        let (c, reach, m, mut done) = iceland(&road);
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles_for(&m), &NONE_OLDER);
        assert_eq!(r.left.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), ["6/28/17"]);
        assert!(r.left[0].1.len() == 1 && r.left[0].1[0].starts_with("zoomed-out tiles (z5–z4)"), "{:?}", r.left);
        assert!(!done.unit.contains_key("6/28/17"));
        assert_eq!(r.moved.len(), 2);
        assert_eq!(units(&plan(&c, &reach, &m, &done, &tiles_for(&m))), ["6/28/17"]);
        // Without outputs (none of its ways kept), whatever it reads: re-keyed.
        let (c, reach, mut m, mut done) = iceland(&road);
        m.remove("base/6-28-17");
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles_for(&m), &NONE_OLDER);
        assert_eq!((r.moved.len(), r.empty.clone(), r.left.len()), (3, vec!["6/28/17".to_string()], 0));
        // The stale hi pack.
        let into = |r: &mut Reaches| r.units.get_mut("6/28/17").unwrap().owned = Some(e7box(-22.6, 61.5, -21.7, 64.16));
        let (c, reach, mut m, mut done) = iceland(&into);
        assert!(!build::coverage_tiles(&c).into_values().flatten().any(|t| t == (27, 18)));
        m.insert("layers/terrain/hi/6-27-18".into(), "layers/terrain/hi/6-27-18.3333333333333333.pack".into());
        let mut tiles = tiles_for(&m);
        tiles.hold("layers/terrain/hi/6-27-18.3333333333333333.pack", [(12, 1760, 1180, 1)]);
        // (Its old key names the stale hi pack: recorded so, as it was built.)
        let old = v1::unit_keys(&c, "d", &m, Some(&reach), &BTreeMap::new()).into_iter().find(|(u, _)| u.slash() == "6/28/17").unwrap().1;
        done.record("unit", &[("6/28/17".into(), old)]);
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert_eq!(r.left.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), ["6/28/17"]);
        assert!(r.left[0].1[0].starts_with("z8–z6 tiles of a z6 tile whose hi pack is stale"), "{:?}", r.left);
    }

    #[test]
    fn a_piece_whose_area_is_to_be_made_again_doesnt_pin_its_zoomed_out_tiles() {
        let (c, reach, m, mut done) = iceland(&|_| {});
        // Akureyri's piece's hi pack without the tiles under its owned box: it reads z8–z6 there.
        let mut tiles = tiles_for(&m);
        tiles.hold("layers/terrain/hi/6-28-16.2222222222222222.pack", []);
        let mut stale = done.clone();
        stale.terrain.insert("3/3/2".into(), "0000000000000000".into());
        let r = rekey(&mut stale, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert!(r.left.iter().any(|(u, why)| u == "6/28/16" && why[0].starts_with("z8–z6 tiles of an area whose terrain is to be made again")), "{r:?}");
        // Its terrain current: pinned by the hi pack.
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert!(r.left.is_empty() && r.moved.len() == 3, "{r:?}");
    }

    #[test]
    fn a_piece_whose_hi_pack_an_earlier_run_left_doesnt_pin_its_zoomed_out_tiles() {
        // Akureyri's piece, its terrain current, its hi pack without the tiles under its owned box
        // (it reads z8–z6 there), and older than its area's lo pack: its area's last run made no hi
        // tiles for it, and its z8–z6 from the raw tiles alone.
        let (c, reach, m, done) = iceland(&|_| {});
        let mut tiles = tiles_for(&m);
        tiles.hold("layers/terrain/hi/6-28-16.2222222222222222.pack", []);
        let akureyri = |x: u32, y: u32| Some((x, y) == (28, 16));
        let r = rekey(&mut done.clone(), &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &Stub(akureyri, NONE_OLDER.1));
        assert_eq!(r.left.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), ["6/28/16"]);
        assert!(r.left[0].1[0].starts_with("z8–z6 tiles of a z6 tile whose hi pack is stale (an earlier run left it"), "{:?}", r.left);
        // Its files' times not readable now: left as it is, for the next pass.
        let unknown = |x: u32, y: u32| if (x, y) == (28, 16) { None } else { Some(false) };
        let mut kept = done.clone();
        let r = rekey(&mut kept, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &Stub(unknown, NONE_OLDER.1));
        assert_eq!(r.unknown.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), ["6/28/16"]);
        assert_eq!(kept.unit.get("6/28/16"), done.unit.get("6/28/16"));
    }

    #[test]
    fn a_hi_pack_older_than_its_areas_lo_pack_by_the_files_times() {
        let d = tempfile::tempdir().unwrap();
        let m: BTreeMap<String, String> = [("layers/terrain/hi/6-28-16", "a"), ("layers/terrain/hi/6-29-16", "b"), ("layers/terrain/lo/3-3-2", "c"), ("layers/terrain/hi/6-30-16", "missing")].into_iter().map(|(l, n)| (l.to_string(), format!("{l}.{n}.pack"))).collect();
        let at = |l: &str, secs_ago: u64| {
            let p = d.path().join(&m[l]);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let f = std::fs::File::create(&p).unwrap();
            f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago)).unwrap();
        };
        // The lo pack written last; 6/28/16's hi pack 20 minutes before it (the same run), 6/29/16's
        // three hours before (an earlier run's).
        at("layers/terrain/lo/3-3-2", 0);
        at("layers/terrain/hi/6-28-16", 20 * 60);
        at("layers/terrain/hi/6-29-16", 3 * 3600);
        let t = FileTimes::new(d.path());
        assert_eq!((t.hi_older(&m, 28, 16), t.hi_older(&m, 29, 16)), (Some(false), Some(true)));
        // One not on the NAS: can't be told; none named: not older.
        assert_eq!((t.hi_older(&m, 30, 16), t.hi_older(&m, 31, 16)), (None, Some(false)));
    }

    #[test]
    fn tree_covers_old_targets_were_the_z3_tiles_the_coverage_meets() {
        let d = tempfile::tempdir().unwrap();
        let cov = |o: &str| Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec![o.into()] }], None, d.path()).unwrap();
        // Reykjavik's 20 km circle: in z3 tile 3/3/2 only.
        let c = cov("place:-21.9,64.13,20");
        let m = BTreeMap::new();
        let t = v1::trees_targets(&c, &m);
        assert_eq!(t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2"]);
        // The coverage there is the key: a bigger circle, another key.
        assert_ne!(v1::trees_targets(&cov("place:-21.9,64.13,25"), &m)[0].1, t[0].1);
        assert_eq!(v1::trees_targets(&cov("place:-21.9,64.13,20"), &m)[0].1, t[0].1);
        // Tree packs where the coverage no longer is (a hi pack of z3 tile 3/4/2): "none".
        let m: BTreeMap<String, String> = [("layers/trees-cover/hi/6-33-23".to_string(), "x".to_string())].into();
        let t = v1::trees_targets(&c, &m);
        assert_eq!(t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2", "3/4/2"]);
        assert_eq!(t[1].1, v1::trees_none("3/4/2"));
    }

    #[test]
    fn tree_cover_current_under_the_old_scheme_is_re_keyed_as_its_pieces_once() {
        let (c, reach, mut m, mut done) = iceland(&|_| {});
        let tiles = tiles_for(&m);
        // Iceland's z3 tile's whole run recorded under the old scheme, its packs in the manifest.
        m.insert("layers/trees-cover/lo/3-3-2".into(), "layers/trees-cover/lo/3-3-2.5555555555555555.pack".into());
        m.insert("layers/trees-cover/hi/6-28-16".into(), "layers/trees-cover/hi/6-28-16.5555555555555555.pack".into());
        let old = v1::trees_targets(&c, &m);
        assert_eq!(old.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["3/3/2"]);
        done.record("trees", &old);
        // As recorded: every piece and the assembly made again.
        let tt = crate::treepacks::targets(&c, &m);
        assert_eq!(build::tree_work(&tt, &m, &done).pieces.len(), 3);
        // Its packs the trees program's: each piece the coverage meets and the assembly recorded
        // under their keys now, its z3 record gone; nothing stale, the pieces' mids made in idle
        // time.
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert_eq!((r.trees_moved.clone(), r.trees_dropped.len(), r.trees_unknown.len()), (vec!["3/3/2".to_string()], 0, 0));
        assert!(r.changed() && !done.trees.contains_key("3/3/2"));
        assert_eq!(done.trees.keys().collect::<Vec<_>>(), ["6/28/16", "6/28/17", "6/29/16"]);
        let w = build::tree_work(&tt, &m, &done);
        assert!(w.pieces.is_empty() && w.lo.is_empty() && w.stale_pieces.is_empty() && w.stale_lo.is_empty(), "{w:?}");
        assert_eq!(w.backfill.len(), 3);
        // Again: nothing to do, nothing changed.
        let before = done.clone();
        let again = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert!(!again.changed() && done == before);
        // A late record under the old scheme (a lease of an older app's, merged since): re-keyed.
        done.record("trees", &old);
        assert_eq!(rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER).trees_moved, ["3/3/2"]);
        assert_eq!(done, before);
        // Its packs trees.py's: its record goes, the pieces and assembly made again.
        let only = |k: &[(String, String)]| {
            let mut keys = Keys::default();
            keys.record("trees", k);
            keys
        };
        let mut py = only(&old);
        let r = rekey(&mut py, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &Stub(NONE_OLDER.0, |_, _| Some(false)));
        assert!(r.trees_dropped.len() == 1 && r.trees_dropped[0].1.contains("trees.py") && py.trees.is_empty() && py.trees_lo.is_empty(), "{r:?}");
        // Their times not readable now: kept, for the next pass.
        let mut unknown = only(&old);
        let r = rekey(&mut unknown, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &Stub(NONE_OLDER.0, |_, _| None));
        assert!(r.trees_unknown.len() == 1 && !r.changed() && unknown == only(&old), "{r:?}");
        // Stale under the old scheme: its record goes (its pieces made again either way).
        let mut stale = only(&[("3/3/2".to_string(), "0000000000000000".to_string())]);
        let r = rekey(&mut stale, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert!(r.trees_dropped[0].1.starts_with("stale") && stale.trees.is_empty(), "{r:?}");
        // "none", current (the coverage gone, the packs not yet dropped): nothing left to build.
        let none = [("3/4/2".to_string(), v1::trees_none("3/4/2"))];
        m.insert("layers/trees-cover/hi/6-33-23".into(), "x".into());
        let mut gone = only(&none);
        let r = rekey(&mut gone, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert!(r.trees_dropped[0].1.contains("nothing left to build") && gone.trees.is_empty(), "{r:?}");
    }

    #[test]
    fn a_late_whole_runs_record_goes_with_those_of_the_pieces_and_assembly_made_since() {
        let (c, reach, mut m, mut done) = iceland(&|_| {});
        let tiles = tiles_for(&m);
        // Since the switch: 3/3/2's pieces made (their mids in the manifest), then its assembly; and
        // a piece of another z3 tile.
        let pieces: Vec<(String, String)> = crate::treepacks::targets(&c, &m).pieces.iter().map(|p| (p.0.clone(), p.1.clone())).collect();
        did(&mut m, &mut done, &Work { step: "trees".into(), targets: pieces });
        let tt = crate::treepacks::targets(&c, &m);
        did(&mut m, &mut done, &Work { step: "trees-lo".into(), targets: tt.lo.iter().map(|l| (l.0.clone(), l.1.clone())).collect() });
        done.trees.insert("6/33/22".into(), "1111111111111111".into());
        let made = done.clone();
        assert_eq!(build::tree_work(&tt, &m, &done), build::TreeWork::default());
        let pieces_of = |k: &Keys| build::tree_work(&tt, &m, k).pieces.into_iter().map(|p| p.0).collect::<Vec<_>>();
        // A late hand-off of 3/3/2's whole run (its lease granted before the switch), for a coverage
        // there since changed: stale under the old scheme. Its record goes, and with it those of the
        // pieces and assembly made since (it wrote over their packs), which are made again; the other
        // z3 tile's piece stays.
        done.record("trees", &[("3/3/2".to_string(), "0000000000000000".to_string())]);
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert!(r.trees_dropped.len() == 1 && r.trees_dropped[0].1.contains("its 3 pieces and its assembly"), "{r:?}");
        assert_eq!((done.trees.keys().map(String::as_str).collect::<Vec<_>>(), done.trees_lo.len()), (vec!["6/33/22"], 0));
        assert_eq!((pieces_of(&done), build::tree_work(&tt, &m, &done).stale_lo.len()), (vec!["6/28/16".to_string(), "6/28/17".into(), "6/29/16".into()], 1));
        // Again: nothing to do.
        let before = done.clone();
        assert!(!rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER).changed() && done == before);
        // One current under the old scheme, the program's: re-keyed over what was made since, but a
        // piece recorded since under another key (the coverage there otherwise then), or with a mid
        // and no record, is made again: its mid isn't of this coverage.
        let mut late = made.clone();
        late.trees.insert("6/28/17".into(), "2222222222222222".into());
        late.trees.remove("6/29/16");
        late.record("trees", &v1::trees_targets(&c, &m));
        let r = rekey(&mut late, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles, &NONE_OLDER);
        assert_eq!(r.trees_moved, ["3/3/2"]);
        assert_eq!(late.trees.keys().collect::<Vec<_>>(), ["6/28/16", "6/33/22"]);
        assert_eq!((late.trees.get("6/28/16"), late.trees_lo.get("3/3/2")), (made.trees.get("6/28/16"), made.trees_lo.get("3/3/2")));
        assert_eq!(pieces_of(&late), ["6/28/17", "6/29/16"]);
    }

    #[test]
    fn tree_packs_made_by_the_trees_program_by_the_files_times() {
        let d = tempfile::tempdir().unwrap();
        let since = std::time::UNIX_EPOCH + std::time::Duration::from_secs(TREES_PROGRAM_SINCE as u64);
        let m: BTreeMap<String, String> = [("layers/trees-cover/lo/3-3-2", "a"), ("layers/trees-height/lo/3-4-2", "b"), ("layers/trees-leaf/hi/6-56-25", "c"), ("layers/trees-cover/lo/3-1-2", "missing")].into_iter().map(|(l, n)| (l.to_string(), format!("{l}.{n}.pack"))).collect();
        let at = |l: &str, t: std::time::SystemTime| {
            let p = d.path().join(&m[l]);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::File::create(&p).unwrap().set_modified(t).unwrap();
        };
        // 3/3/2's an hour before the program, 3/4/2's (its height layer's lo pack) a minute after,
        // 3/7/3's by its hi pack.
        at("layers/trees-cover/lo/3-3-2", since - std::time::Duration::from_secs(3600));
        at("layers/trees-height/lo/3-4-2", since + std::time::Duration::from_secs(60));
        at("layers/trees-leaf/hi/6-56-25", since + std::time::Duration::from_secs(60));
        let t = FileTimes::new(d.path());
        assert_eq!((t.trees_by_program(&m, 3, 2), t.trees_by_program(&m, 4, 2), t.trees_by_program(&m, 7, 3)), (Some(false), Some(true), Some(true)));
        // None at all: none to differ. One not on the NAS: can't be told.
        assert_eq!((t.trees_by_program(&m, 0, 0), t.trees_by_program(&m, 1, 2)), (Some(true), None));
    }

    #[test]
    fn a_unit_whose_terrain_cant_be_read_now_keeps_its_record() {
        let (c, reach, m, mut done) = iceland(&|_| {});
        let before = done.clone();
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &TerrainTiles::new(None), &NONE_OLDER);
        assert_eq!(r.unknown.len(), 3);
        assert!(!r.changed() && done == before);
    }

    // ---- the terrain keyed on its water (2026-10-09) ----------------------------------------------

    use crate::terrain_water::{tests::square, Kind, Poly, WaterIdx, WaterSource};

    /// A basemap's water as a test has it: a lake (a square, its own id) in each of `lakes`' tiles,
    /// the sea along the top of every z9 tile.
    struct Basemap {
        pin: String,
        lakes: Vec<(u8, u32, u32, u64)>,
    }

    impl WaterSource for Basemap {
        fn polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
            let mut v = Vec::new();
            if z == 9 {
                v.push(square(Kind::Sea, 0, 0.0, 0.0, 256.0, 10.0));
            }
            v.extend(self.lakes.iter().filter(|l| (l.0, l.1, l.2) == (z, x, y)).map(|l| square(Kind::Lake, l.3, 100.0, 100.0, 140.0, 130.0)));
            Ok(v)
        }
        fn pin(&self) -> String {
            self.pin.clone()
        }
    }

    /// A lake fill over a basemap (as lakefill's FilledWater): a lake in every tile (z6 and finer)
    /// of z6 tile `over`.
    struct Filled<'a> {
        base: &'a dyn WaterSource,
        over: (u32, u32),
    }

    impl WaterSource for Filled<'_> {
        fn polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
            let mut v = self.base.polys(z, x, y)?;
            if z >= 6 && (x >> (z - 6), y >> (z - 6)) == self.over {
                v.push(square(Kind::Lake, (1 << 62) | 7, 20.0, 20.0, 60.0, 50.0));
            }
            Ok(v)
        }
        fn pin(&self) -> String {
            format!("{} + fill", self.base.pin())
        }
    }

    /// The water digests the coverage's terrain wants of `src` (the terrain-water job's).
    fn digests(c: &Coverage, src: &dyn WaterSource) -> WaterIdx {
        crate::terrain_water::make_idx(src, None, &build::water_reads(c), &|_, _| {}).unwrap()
    }

    /// `m` naming basemap `b`'s content name as the latest pass's, and the digests `idx`.
    fn with_water(m: &BTreeMap<String, String>, basemap: Option<&str>, idx: &[&WaterIdx]) -> (BTreeMap<String, String>, TerrainTiles) {
        let mut m = m.clone();
        if let Some(b) = basemap {
            let l = b.split('.').next().unwrap();
            m.insert(l.into(), b.into());
        }
        let mut tiles = tiles_for(&m);
        for w in idx {
            let l = crate::terrain_water::idx_logical(&w.pin);
            let c = format!("{l}.{}.json", &store::naming::hash16(serde_json::to_string(w).unwrap().as_bytes()));
            m.insert(l, c.clone());
            tiles.hold_water(&c, (*w).clone());
        }
        (m, tiles)
    }

    const A: &str = "layers/basemap/world-d.aaaaaaaaaaaaaaaa.pmtiles";
    const B: &str = "layers/basemap/world-e.bbbbbbbbbbbbbbbb.pmtiles";

    /// Terrain's targets' keys, by target.
    fn tkeys(c: &Coverage, m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> BTreeMap<String, String> {
        let tt = build::terrain_slope_targets(c, m, tiles);
        tt.terrain.into_iter().chain(tt.terrain_lo).collect()
    }

    /// The first z9 tile piece `t` reads.
    fn z9_of(c: &Coverage, t: (u32, u32)) -> (u32, u32) {
        crate::terrain_pack::piece_levels(c, t).into_iter().find(|l| l.0 == 9).unwrap().1[0]
    }

    #[test]
    fn a_pieces_key_names_the_water_it_reads_alone() {
        let (c, _, m, _) = iceland(&|_| {});
        let pieces: Vec<(u32, u32)> = build::coverage_tiles(&c).into_values().flatten().collect();
        let (p, q) = (pieces[0], pieces[1]);
        let (pz9, qz9) = (z9_of(&c, p), z9_of(&c, q));
        let at = |pin: &str, lakes: Vec<(u8, u32, u32, u64)>| digests(&c, &Basemap { pin: pin.into(), lakes });
        let keys = |b: &str, w: &WaterIdx| {
            let (m, tiles) = with_water(&m, Some(b), &[w]);
            tkeys(&c, &m, &tiles)
        };
        let (ps, qs) = (format!("6/{}/{}", p.0, p.1), format!("6/{}/{}", q.0, q.1));
        let a = keys(A, &at(A, vec![(9, qz9.0, qz9.1, 5)]));
        assert!(a.values().all(|k| k != build::UNKNOWN), "{a:?}");
        // Another basemap whose water differs only far from p (another lake in q's tile): p's key
        // and the assembly's the same, q's not.
        let b = keys(B, &at(B, vec![(9, qz9.0, qz9.1, 6)]));
        assert_eq!(a[&ps], b[&ps]);
        assert_ne!(a[&qs], b[&qs]);
        assert_eq!(a["3/3/2"], b["3/3/2"]);
        assert_eq!(a.iter().filter(|(t, k)| b[*t] != **k).count(), 1);
        // A lake in a tile p reads: p's key changes.
        let b = keys(B, &at(B, vec![(9, qz9.0, qz9.1, 5), (9, pz9.0, pz9.1, 8)]));
        assert_ne!(a[&ps], b[&ps]);
        assert_eq!(a[&qs], b[&qs]);
        // One in a z7 tile: the assembly's alone (the pieces read z9–12).
        let b = keys(B, &at(B, vec![(9, qz9.0, qz9.1, 5), (7, p.0 << 1, p.1 << 1, 9)]));
        assert_ne!(a["3/3/2"], b["3/3/2"]);
        assert_eq!(a.iter().filter(|(t, k)| b[*t] != **k).count(), 1);
        // The same water under another name: the same keys.
        assert_eq!(a, keys(B, &at(B, vec![(9, qz9.0, qz9.1, 5)])));
        // Its digests not made yet (or not readable now): its keys unknown, the job offered, nothing
        // made meanwhile, and its area waits.
        let (m2, tiles) = with_water(&m, Some(A), &[]);
        assert!(tkeys(&c, &m2, &tiles).values().all(|k| k == build::UNKNOWN));
        let done = Keys::default();
        assert_eq!(build::terrain_water_work(&c, &m2, &done, &tiles).map(|w| w.step), Some("terrain-water".into()));
        let w = build::terrain_work(&build::terrain_slope_targets(&c, &m2, &tiles), &m2, &done);
        assert!(w.terrain.is_empty() && w.terrain_lo.is_empty() && w.terrain_left.contains("3/3/2"), "{w:?}");
        // Made: no job.
        let (m3, tiles) = with_water(&m, Some(A), &[&at(A, vec![])]);
        assert!(build::terrain_water_work(&c, &m3, &done, &tiles).is_none());
    }

    #[test]
    fn a_tiles_digest_is_of_what_the_terrain_makes_of_it() {
        use crate::terrain_water::{polys_digest, OWN_FOR_TESTS};
        let lake = |id| square(Kind::Lake, id, 1.0, 2.0, 30.0, 40.0);
        assert_eq!(polys_digest(&[]), 0);
        // A lake's key of its tile's own (no OSM id) by its feature's place: never its value.
        assert_eq!(polys_digest(&[lake(OWN_FOR_TESTS | 3)]), polys_digest(&[lake(OWN_FOR_TESTS | 12345)]));
        assert_ne!(polys_digest(&[lake(3)]), polys_digest(&[lake(4)]));
        assert_ne!(polys_digest(&[lake(3)]), polys_digest(&[square(Kind::Sea, 3, 1.0, 2.0, 30.0, 40.0)]));
        assert_ne!(polys_digest(&[lake(3)]), polys_digest(&[square(Kind::Lake, 3, 1.0, 2.0, 30.0, 40.5)]));
    }

    #[test]
    fn the_switch_reads_the_records_under_their_waters_keys_and_a_new_basemap_makes_where_its_water_changed() {
        let (c, _, m, mut done) = iceland(&|_| {});
        let pieces: Vec<(u32, u32)> = build::coverage_tiles(&c).into_values().flatten().collect();
        let p = pieces[0];
        let ps = format!("6/{}/{}", p.0, p.1);
        // The terrain made by its area's whole run from basemap A (its record: `v1`).
        let (ma, _) = with_water(&m, Some(A), &[]);
        done.terrain.clear();
        done.record("terrain", &v1::terrain_slope_targets(&c, &ma).0);
        let wa = digests(&c, &Basemap { pin: A.into(), lakes: vec![] });
        let tw = |m: &BTreeMap<String, String>, tiles: &TerrainTiles, done: &Keys| {
            let mut read = done.clone();
            let d = derive(&mut read, &c, m, tiles);
            (d, build::terrain_work(&build::terrain_slope_targets(&c, m, tiles), m, &read), read)
        };
        // Its digests not made yet: the record kept, nothing made, the area waits.
        let (m0, t0) = with_water(&m, Some(A), &[]);
        let (d, w, read) = tw(&m0, &t0, &done);
        assert_eq!((d.terrain.len(), d.unknown.len()), (0, 1), "{d:?}");
        assert!(read.terrain.contains_key("3/3/2") && w.terrain.is_empty() && w.terrain_left.contains("3/3/2"));
        // Made: nothing to make (the switch).
        let (m1, t1) = with_water(&m, Some(A), &[&wa]);
        let (d, w, read) = tw(&m1, &t1, &done);
        assert_eq!(d.terrain, ["3/3/2"]);
        assert!(d.older.is_empty() && w.terrain_pieces_left.is_empty() && w.terrain_lo_left.is_empty() && w.terrain_left.is_empty(), "{w:?}");
        assert!(!read.terrain.contains_key("3/3/2"));
        // A new basemap B, its water the same: still nothing (the record read by A's digests).
        let wb = WaterIdx { pin: B.into(), ..wa.clone() };
        let (m2, t2) = with_water(&m1, Some(B), &[&wa, &wb]);
        let (d, w, _) = tw(&m2, &t2, &done);
        assert_eq!(d.older, [("3/3/2".to_string(), A.to_string())]);
        assert!(w.terrain_pieces_left.is_empty() && w.terrain_lo_left.is_empty(), "{w:?}");
        // B with a lake in a tile p reads: p alone (and its area's other pieces for their mids).
        let pz9 = z9_of(&c, p);
        let wb = digests(&c, &Basemap { pin: B.into(), lakes: vec![(9, pz9.0, pz9.1, 5)] });
        let (m3, t3) = with_water(&m1, Some(B), &[&wa, &wb]);
        let (_, w, _) = tw(&m3, &t3, &done);
        assert_eq!(w.terrain_pieces_left.iter().cloned().collect::<Vec<_>>(), [ps.clone()]);
        assert!(w.terrain_lo_left.is_empty());
        // Without A's digests (not kept): every piece made again.
        let (m4, t4) = with_water(&m, Some(B), &[&wb]);
        let (d, w, _) = tw(&m4, &t4, &done);
        assert_eq!(d.stale.len(), 1);
        assert_eq!(w.terrain_pieces_left.len(), pieces.len());
        // A piece and an assembly recorded under the keys that named the basemap (an older app's
        // job, 2026-10-08): read under their keys now, by the digests of the basemap they name.
        let mut v2rec = Keys::default();
        let fp = c.fingerprint(build::grown_e7(6, p.0, p.1, 20.0));
        v2rec.terrain.insert(ps.clone(), v2::terrain_piece_key(p, &fp, A));
        let mids: Vec<String> = build::coverage_tiles(&c)[&(3, 2)].iter().map(|(x, y)| format!("6/{x}/{y}=-")).collect();
        v2rec.terrain_lo.insert("3/2/2".into(), "x".into());
        v2rec.terrain_lo.insert("3/3/2".into(), v2::terrain_lo_key((3, 2), A, &mids));
        let (d, w, read) = tw(&m2, &t2, &v2rec);
        assert_eq!(d.water.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), [ps.as_str(), "3/3/2"]);
        assert!(!w.terrain_pieces_left.contains(&ps) && !w.terrain_lo_left.contains("3/3/2"), "{w:?}");
        assert_eq!(read.terrain_lo["3/2/2"], "x");
        // Twice is once.
        let mut twice = read.clone();
        let again = derive(&mut twice, &c, &m2, &t2);
        assert!(again.water.is_empty() && twice == read);
    }

    #[test]
    fn a_lake_fill_changes_the_keys_of_the_pieces_it_touches_alone() {
        let (c, _, m, _) = iceland(&|_| {});
        let pieces: Vec<(u32, u32)> = build::coverage_tiles(&c).into_values().flatten().collect();
        let f = pieces[1];
        let base = Basemap { pin: A.into(), lakes: vec![] };
        let filled = Filled { base: &base, over: f };
        let (ma, ta) = with_water(&m, Some(A), &[&digests(&c, &base)]);
        let before = tkeys(&c, &ma, &ta);
        // (The fill's water as the jobs read it: its pin names it with the basemap.)
        let w = digests(&c, &filled);
        assert_eq!(w.pin, format!("{A} + fill"));
        let mut mf = ma.clone();
        let l = crate::terrain_water::idx_logical(&w.pin);
        mf.insert(l.clone(), format!("{l}.cccccccccccccccc.json"));
        let mut tf = tiles_for(&mf);
        tf.hold_water(&format!("{l}.cccccccccccccccc.json"), w.clone());
        let idx = tf.water_idx(&mf, &w.pin).unwrap();
        let key = |t: &str, from: &str| build::water_key(Some(&w.pin), Some(idx), t, from).unwrap();
        let after: BTreeMap<String, String> = before
            .keys()
            .map(|t| {
                let u = Unit::parse(t).unwrap();
                let k = if u.z == 6 {
                    let fp = c.fingerprint(build::grown_e7(6, u.x, u.y, 20.0));
                    build::terrain_piece_key((u.x, u.y), &fp, &key(t, &fp))
                } else {
                    let mids: Vec<String> = build::coverage_tiles(&c)[&(u.x, u.y)].iter().map(|(x, y)| format!("6/{x}/{y}=-")).collect();
                    build::terrain_lo_key((u.x, u.y), &key(t, crate::terrain_water::AREA_FROM), &mids)
                };
                (t.clone(), k)
            })
            .collect();
        let changed: Vec<&String> = before.keys().filter(|t| before[*t] != after[*t]).collect();
        // The piece under the fill and its area's assembly (its z6–8 tiles), nothing else.
        assert_eq!(changed, [&format!("3/{}/{}", f.0 >> 3, f.1 >> 3), &format!("6/{}/{}", f.0, f.1)]);
    }
}
