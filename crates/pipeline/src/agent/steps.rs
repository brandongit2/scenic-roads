//! The steps table (docs/pool.md §7.2): a row per step the agent runs as a job, saying what a job of
//! it needs (memory, disk), how it may share a Mac (alone, beside another,
//! the steps that never run two at once on one Mac), whether other members take it, how many
//! targets go in a job, and what it may write (its write-set: the logical names its jobs may add,
//! change or remove in the build's manifest).
//!
//! The step sets the agent and the coordinator test (`SHARED`, `SECOND`, `ALONE`, `RAW`,
//! `WIKI`, `NAS_READS`, `ANSWERED`) are made from the table at compile time (a set whose size isn't
//! what the table gives fails the build), and a step's first guess of memory, disk, needs and batch
//! are read from its row (`row`). (What depends on a target, not its step, stays with the agent: a
//! terrain area's whole run's disk, the memory a terrain area, a unit's piece or a 3D buildings tile
//! is offered with, the 3D buildings' smaller batches while the regions' own work is left.)
//!
//! The write-sets: the pool's lead checks every journal entry it merges against its step's
//! (`outside`), and reports what lies outside (crate::agent::pool), refusing nothing for it yet
//! (docs/pool.md §12, phase 4: the steps' write-sets are enforced in a later batch, once the
//! reports have shown them right on the real build). A shared step's files for its targets
//! (`saves`, which crate::coord's check of a helper's hand-off and the merge's phase 1 check use)
//! are enforced as they were.

use crate::legacy::Unit;

/// A step's row.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub name: &'static str,
    /// The memory a job of it is expected to take (MB) before a target's own run has said
    /// (`SCENIC_COSTS`, crate::coord::Cost): offered so to the members, and reckoned so beside
    /// another job. (Units and candidates are offered by their piece's size, terrain by its area's,
    /// the 3D buildings by their rows: crate::agent's offers.)
    pub mem_mb: u64,
    /// The free space a job of it starts with on the Mac it runs on, as its first job (bytes; the
    /// OSM pass less the pack cache it clears; a terrain area's whole run more: crate::agent's
    /// `need_of`). Beside another job, one that mostly waits on the network starts with 10 GB.
    pub disk: u64,
    /// Other members take its jobs (the shared steps), at this rank of preference (what later steps
    /// wait on first).
    pub shared: Option<u8>,
    /// A Mac's second job may be one of it, at this rank of preference (docs/plan.md §8, Two jobs
    /// at once).
    pub beside: Option<u8>,
    /// It runs alone on its Mac, never beside another job.
    pub alone: bool,
    /// The groups of steps of which two never run at once on one Mac.
    pub groups: &'static [Group],
    /// It keeps the pass's Wikidata and Wikipedia answers (crate::answers): none starts while the
    /// agent sends them as it starts.
    pub answered: bool,
    /// Targets a job takes (`usize::MAX`: all of them).
    pub batch: usize,
    /// Its write-set: whether a job of it may write `logical` (`removal`: removes it), for some
    /// target of its own.
    pub writes: fn(&str, bool) -> bool,
}

/// Steps of which two never run at once on one Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// They read AWS's raw terrain tiles here, which a terrain job packs onto the NAS and deletes.
    Raw,
    /// They ask Wikidata and Wikipedia a great deal from this Mac's address, each paced as if alone.
    Wiki,
    /// They read gigabytes of the NAS's sources a target (the 3D buildings' parquet).
    NasReads,
}

const GB: u64 = 1 << 30;
/// The free space a job starts with, at least (crate::agent::room::RESERVE).
const RESERVE: u64 = super::room::RESERVE;

/// A row with what most steps have: CPU work, the reserve on the disk, 1.5 GB until measured, none
/// of the sets, every target in one job.
const fn base(name: &'static str, writes: fn(&str, bool) -> bool) -> Step {
    Step { name, mem_mb: 1500, disk: RESERVE, shared: None, beside: None, alone: false, groups: &[], answered: false, batch: usize::MAX, writes }
}

/// The table, in no order of its own (the ranks order what's ranked).
pub const TABLE: [Step; 43] = [
    // The OSM pass: the planet filtered, its sets, pieces and road values (80 GB free: the filtered
    // planet with room to spare, less the pack cache it clears).
    Step { disk: super::PASS_SPACE, alone: true, ..base("osm-pass", w_osm_pass) },
    Step { alone: true, ..base("pass-sets", w_pass_sets) },
    Step { alone: true, ..base("trailends", w_trailends) },
    Step { alone: true, ..base("reach", w_reach) },
    Step { alone: true, groups: &[Group::Raw], ..base("terrain-z8", w_terrain_z8) },
    Step { alone: true, ..base("buildings", w_buildings) },
    Step { alone: true, ..base("summits", w_summits) },
    Step { alone: true, ..base("labels", w_labels) },
    // (The worldwide water: its z14 directory and stored tiles held, 7.7 GB at most measured,
    // 2026-10-08; its tiles' archive, 1.8 GB worldwide, and their packs written here first.)
    Step { mem_mb: 8192, disk: RESERVE + 5 * GB, alone: true, ..base("water", w_water) },
    Step { alone: true, answered: true, ..base("heritage-sites", w_heritage_sites) },
    Step { alone: true, ..base("gc", w_none) },
    Step { ..base("backup", w_none) },
    Step { ..base("spoken", w_spoken) },
    Step { ..base("names-todo", w_none) },
    // Terrain's pieces (a z6 tile each, eight a job; a z3 tile is an area's whole run, the scheme
    // before pieces): each z6 tile's raw tiles' archive copied here and the new raw tiles held
    // twice while they're packed onto the NAS, 5 GB past the reserve.
    Step { disk: RESERVE + 5 * GB, shared: Some(0), groups: &[Group::Raw], batch: 8, ..base("terrain", w_terrain) },
    // (Its z3 tile's 1,365 zoomed-out tiles, ~270 KB each made, its pieces' z9 means, up to 4 MB a
    // piece, and half a GB besides.)
    Step { mem_mb: 1200, disk: RESERVE + 5 * GB, groups: &[Group::Raw], batch: 4, ..base("terrain-lo", w_terrain_lo) },
    Step { ..base("terrain-water", w_terrain_water) },
    Step { groups: &[Group::Raw], ..base("terrain-root", w_terrain_root) },
    // (Slope holds a z6 tile's tiles at a time, under a GB; its assembly the area's lo tiles.)
    Step { mem_mb: 1500, shared: Some(1), beside: Some(10), batch: 8, ..base("slope", w_slope) },
    Step { mem_mb: 1000, batch: 4, ..base("slope-lo", w_slope_lo) },
    Step { ..base("slope-root", w_slope_root) },
    // (Tree cover's program holds a band of a block's rows on each thread and the blocks made but
    // not yet written: 1.05 GB on 14 threads for 3/2/2's 792 blocks, 2026-10-05; a piece has 16
    // blocks at most. Its assembly its z3 tile's blocks' zoom-8 values, compressed.)
    Step { mem_mb: 1000, shared: Some(2), batch: 4, ..base("trees", w_trees) },
    Step { mem_mb: 500, batch: 4, ..base("trees-lo", w_trees_lo) },
    // (A unit's most its batches took here, 8.4 GB, 2026-10-05; offered by its piece's size.)
    Step { mem_mb: 8600, shared: Some(3), beside: Some(9), batch: 6, ..base("unit", w_unit) },
    Step { mem_mb: 2048, shared: Some(4), beside: Some(7), batch: 24, ..base("pois", w_pois) },
    Step { mem_mb: 2500, shared: Some(5), beside: Some(8), groups: &[Group::Raw], batch: 12, ..base("peaks", w_peaks) },
    Step { batch: 16, ..base("pack", w_pack) },
    Step { batch: 2, ..base("lo", w_lo) },
    Step { mem_mb: 3072, beside: Some(1), groups: &[Group::Wiki], answered: true, ..base("items", w_items) },
    Step { mem_mb: 6144, beside: Some(0), groups: &[Group::Wiki], answered: true, ..base("heritage", w_heritage) },
    Step { mem_mb: 4096, beside: Some(5), ..base("marks", w_marks) },
    Step { mem_mb: 4096, beside: Some(6), ..base("overlays", w_overlays) },
    Step { ..base("roadunits", w_roadunits) },
    Step { ..base("stations", w_stations) },
    Step { ..base("ferries", w_ferries) },
    Step { mem_mb: 1024, beside: Some(2), ..base("rail-feeds", w_rail_feeds) },
    Step { mem_mb: 6144, beside: Some(3), ..base("rail", w_rail) },
    Step { mem_mb: 1024, beside: Some(4), ..base("bld-fetch", w_none) },
    // (The 3D buildings, until a target's own run says: the densest tile's, B1's Kantō 6/56/25:
    // 5.1 GB to read its 30.3 M rows, 3.3 GB to raise its tiles. Offered by the rows they read.)
    Step { mem_mb: 5200, shared: Some(6), beside: Some(11), groups: &[Group::NasReads], batch: 8, ..base("bldprep", w_bldprep) },
    Step { mem_mb: 3400, shared: Some(7), beside: Some(12), batch: 16, ..base("bldtiles", w_bldtiles) },
    Step { ..base("catalog", w_none) },
    Step { ..base("catalog-held", w_none) },
    Step { ..base("prune", w_prune) },
];

/// The row of `step`; None for one the table doesn't know (a task's kind, or a step of a newer
/// app's).
pub fn row(step: &str) -> Option<&'static Step> {
    TABLE.iter().find(|s| s.name == step)
}

/// The memory a job of `step` is expected to take before its own run has said (MB; 1.5 GB for a
/// step the table doesn't know).
pub fn mem_mb(step: &str) -> u64 {
    row(step).map_or(1500, |s| s.mem_mb)
}

/// Targets per job of `step` (all of them for a step the table doesn't know).
pub fn batch(step: &str) -> usize {
    row(step).map_or(usize::MAX, |s| s.batch)
}

/// Whether `step` runs alone on its Mac.
pub fn alone(step: &str) -> bool {
    row(step).is_some_and(|s| s.alone)
}

/// Whether `a` and `b` are in one of the groups of which two never run at once on one Mac.
pub fn grouped(a: &str, b: &str) -> bool {
    match (row(a), row(b)) {
        (Some(x), Some(y)) => x.groups.iter().any(|g| y.groups.contains(g)),
        _ => false,
    }
}

/// Which of a row's sets a step set is made of.
#[derive(Clone, Copy)]
enum Set {
    Shared,
    Beside,
    Alone,
    Raw,
    Wiki,
    NasReads,
    Answered,
}

/// Whether row `s` is in `set`, and its rank there (the shared and second job's steps by their
/// ranks, the others in the table's order).
const fn rank_in(s: &Step, set: Set, i: usize) -> Option<usize> {
    const fn has(g: &[Group], want: Group) -> bool {
        let mut k = 0;
        while k < g.len() {
            if g[k] as u8 == want as u8 {
                return true;
            }
            k += 1;
        }
        false
    }
    match set {
        Set::Shared => match s.shared {
            Some(r) => Some(r as usize),
            None => None,
        },
        Set::Beside => match s.beside {
            Some(r) => Some(r as usize),
            None => None,
        },
        Set::Alone if s.alone => Some(i),
        Set::Raw if has(s.groups, Group::Raw) => Some(i),
        Set::Wiki if has(s.groups, Group::Wiki) => Some(i),
        Set::NasReads if has(s.groups, Group::NasReads) => Some(i),
        Set::Answered if s.answered => Some(i),
        _ => None,
    }
}

/// Whether `set`'s ranks are its own, 0 to N-1 (the shared and second job's steps), not an order
/// taken from the table's order.
const fn dense(set: Set) -> bool {
    matches!(set, Set::Shared | Set::Beside)
}

/// The steps of `set`, by rank: N of them, or the build fails: two steps of one rank, a rank
/// missing from a set whose ranks are its own (`dense`), more steps than N or fewer.
const fn set_of<const N: usize>(set: Set) -> [&'static str; N] {
    let mut out = [""; N];
    let mut n = 0;
    // (Each rank in turn: the row with the next rank up.)
    let mut last: Option<usize> = None;
    loop {
        let mut best: Option<(usize, usize)> = None;
        let mut i = 0;
        while i < TABLE.len() {
            if let Some(r) = rank_in(&TABLE[i], set, i) {
                let after = match last {
                    Some(l) => r > l,
                    None => true,
                };
                let better = match best {
                    Some((b, _)) => r < b,
                    None => true,
                };
                if after && better {
                    best = Some((r, i));
                }
            }
            i += 1;
        }
        match best {
            Some((r, i)) => {
                // (No other row of the same rank: it would be passed over.)
                let mut k = 0;
                while k < TABLE.len() {
                    if k != i {
                        if let Some(r2) = rank_in(&TABLE[k], set, k) {
                            assert!(r2 != r, "two steps of the table share a rank in a set");
                        }
                    }
                    k += 1;
                }
                assert!(!dense(set) || r == n, "a rank is missing from a set of the table");
                assert!(n < N, "the table has more steps in a set than its size");
                out[n] = TABLE[i].name;
                n += 1;
                last = Some(r);
            }
            None => break,
        }
    }
    assert!(n == N, "the table has fewer steps in a set than its size");
    out
}

/// The shared steps: other members take their jobs (crate::coord leases them), in this order of
/// preference: what later steps wait on first.
pub const SHARED: [&str; 8] = set_of(Set::Shared);
/// The steps a Mac's second job takes, in its order of preference.
pub const SECOND: [&str; 13] = set_of(Set::Beside);
/// The steps that run alone, never beside another job.
pub const ALONE: [&str; 11] = set_of(Set::Alone);
/// The steps that read AWS's raw terrain tiles here: never two at once.
pub const RAW: [&str; 5] = set_of(Set::Raw);
/// The steps that ask Wikidata and Wikipedia a great deal from this Mac's address: never two at once.
pub const WIKI: [&str; 2] = set_of(Set::Wiki);
/// The steps that read gigabytes of the NAS's sources a target: never two at once on one Mac.
pub const NAS_READS: [&str; 1] = set_of(Set::NasReads);
/// The steps that keep the pass's Wikidata and Wikipedia answers.
pub const ANSWERED: [&str; 3] = set_of(Set::Answered);

/// Whether journal entry `e`'s changes are all within its step's write-set; why not when they
/// aren't (the first outside it, and how many), or when the table has no row for its step.
pub fn outside(e: &crate::pool::journal::Entry) -> Option<String> {
    let Some(s) = row(&e.step) else {
        return (!e.handoff.changes.is_empty()).then(|| format!("{} isn't a step the steps table knows ({} change{})", e.step, e.handoff.changes.len(), if e.handoff.changes.len() == 1 { "" } else { "s" }));
    };
    let out: Vec<&String> = e.handoff.changes.iter().filter(|(l, v)| !(s.writes)(l, v.is_none())).map(|(l, _)| l).collect();
    let first = out.first()?;
    let what = if e.handoff.changes[*first].is_none() { "removes" } else { "writes" };
    Some(format!("{what} {first}, outside {}'s write-set{}", e.step, if out.len() > 1 { format!(" (and {} more)", out.len() - 1) } else { String::new() }))
}

/// Whether `l` is a file a job of `step` saves for `target` (what a shared step's hand-off may
/// change for each target it did): a unit's base pack, road values, English and the grids its
/// packs lacked; candidates' and peaks' own files; a tree cover piece's (a z6 tile's) hi packs of
/// the tree layers and its mid, an assembly's (a z3 tile's) lo packs of them; terrain's and slope's
/// pieces (a z6 tile's) hi pack of their layer and its mid, an assembly's (a z3 tile's) lo pack; an
/// area's (a z3 tile's) lo pack and its z6 tiles' hi packs of terrain, slope, or the tree layers (a
/// z3 tile's whole run: a lease of the scheme before pieces); a z6 tile's normalized buildings
/// (`bldprep`) or 3D buildings' hi pack (`bldtiles`). False for the other steps.
pub fn saves(step: &str, target: &str, l: &str) -> bool {
    let dash = target.replace('/', "-");
    let tile = Unit::parse(target);
    match step {
        "unit" => crate::unit::saved_files(&dash).iter().any(|f| f == l),
        "pois" | "peaks" => l == format!("work/{step}/{dash}"),
        // (A z6 tile's normalized buildings, and its 3D buildings' hi pack.)
        "bldprep" => tile.is_some_and(|t| t.z == 6 && l == crate::bld::work_logical(t.x, t.y)),
        "bldtiles" => tile.is_some_and(|t| t.z == 6 && l == crate::bld::pack_logical(t.x, t.y)),
        "trees" if tile.is_some_and(|u| u.z == 6) => tile.is_some_and(|t| l == crate::treepacks::mid_logical(t.x, t.y) || crate::treepacks::LAYERS.iter().any(|layer| l == format!("layers/{layer}/hi/{dash}"))),
        "trees-lo" => tile.is_some_and(|u| u.z == 3) && crate::treepacks::LAYERS.iter().any(|layer| l == format!("layers/{layer}/lo/{dash}")),
        "terrain" if tile.is_some_and(|u| u.z == 6) => tile.is_some_and(|t| l == crate::terrain_pack::mid_logical(t.x, t.y) || l == format!("layers/terrain/hi/{dash}")),
        "slope" if tile.is_some_and(|u| u.z == 6) => tile.is_some_and(|t| l == crate::slope_pack::mid_logical(t.x, t.y) || l == format!("layers/slope/hi/{dash}")),
        "terrain-lo" | "slope-lo" => tile.is_some_and(|u| u.z == 3) && l == format!("layers/{}/lo/{dash}", step.trim_end_matches("-lo")),
        "terrain" | "slope" | "trees" => {
            let Some(q) = tile.filter(|u| u.z == 3) else { return false };
            let layers: &[&str] = match step {
                "terrain" => &["terrain"],
                "slope" => &["slope"],
                _ => &crate::treepacks::LAYERS,
            };
            layers.iter().any(|layer| {
                l == format!("layers/{layer}/lo/{dash}")
                    || l.strip_prefix(&format!("layers/{layer}/hi/6-")).and_then(|r| r.split_once('-')).and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?))).is_some_and(|(x, y)| (x >> 3, y >> 3) == (q.x, q.y))
            })
        }
        _ => false,
    }
}

// The write-sets. A name's parts, split at '/'; a tile in a name is `z-x-y` (`tile`), a pass by its
// planet's date (`date`).

/// Whether `s` is tile `z-x-y` of zoom `z` (x and y within it, no leading zeros but "0").
fn tile(s: &str, z: u32) -> bool {
    let mut p = s.split('-');
    let num = |t: Option<&str>| t.filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && (t.len() == 1 || !t.starts_with('0'))).and_then(|t| t.parse::<u32>().ok());
    match (num(p.next()), num(p.next()), num(p.next()), p.next()) {
        (Some(tz), Some(x), Some(y), None) => tz == z && (x as u64) < (1u64 << z) && (y as u64) < (1u64 << z),
        _ => false,
    }
}

/// Whether `s` is a day, `YYYY-MM-DD`.
fn date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Whether `l` is a pack of layer `layer` (or of a layer `layer` accepts) at one of `scopes` ("root",
/// "lo", "hi"): `layers/<layer>/root/0-0-0`, `…/lo/3-x-y`, `…/hi/6-x-y`.
fn pack_of(l: &str, layer: &dyn Fn(&str) -> bool, scopes: &[&str]) -> bool {
    let p: Vec<&str> = l.split('/').collect();
    match p.as_slice() {
        ["layers", ly, scope, t] if layer(ly) && scopes.contains(scope) => match *scope {
            "root" => *t == "0-0-0",
            "lo" => tile(t, 3),
            "hi" => tile(t, 6),
            _ => false,
        },
        _ => false,
    }
}

/// `pack_of` for one layer by name.
fn pack(l: &str, layer: &str, scopes: &[&str]) -> bool {
    pack_of(l, &|ly| ly == layer, scopes)
}

const ALL: [&str; 3] = ["root", "lo", "hi"];

fn parts(l: &str) -> Vec<&str> {
    l.split('/').collect()
}

fn w_none(_: &str, _: bool) -> bool {
    false
}

/// The pass's sources (`sources/osm/<date>/…`: its filtered planet, sets, outlines, pieces, road
/// values, the mark that it's complete) and its basemap; as it completes, the older passes' entries
/// removed (crate::osmpass::retire_older: their sources, items, heritage, summits, route ends).
fn w_osm_pass(l: &str, removal: bool) -> bool {
    match parts(l).as_slice() {
        ["sources", "osm", d, _, ..] => date(d),
        ["layers", "basemap", w] => w.strip_prefix("world-").is_some_and(date),
        ["sources", "items", d, _, ..] | ["work", "heritage", d, _, ..] => removal && date(d),
        ["work", "summits", d] | ["work", "trailends", d] => removal && date(d),
        _ => false,
    }
}

/// The sets a pass lacks in their current filters.
fn w_pass_sets(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "sets", _] if date(d))
}

fn w_trailends(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "trailends", d] if date(d))
}

fn w_reach(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "reach"] if date(d))
}

fn w_terrain_z8(l: &str, _: bool) -> bool {
    l == crate::terrain_z8::logical() || l == crate::terrain_z8::max_logical()
}

/// The world's roadside buildings' index, per Overture release.
fn w_buildings(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "buildings", _, "index"])
}

fn w_summits(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "summits", d] if date(d))
}

/// The labels' packs, every zoom (those it didn't write removed).
fn w_labels(l: &str, _: bool) -> bool {
    pack(l, "labels", &ALL)
}

/// The water's packs, every zoom (those it didn't write removed, and the small water's before it,
/// which it replaced).
fn w_water(l: &str, removal: bool) -> bool {
    pack(l, "water", &ALL) || (removal && pack(l, "smallwater", &ALL))
}

/// The heritage sites by z6 tile, their positions and areas (those of the pass it didn't write
/// removed), and the base files the chain reads.
fn w_heritage_sites(l: &str, _: bool) -> bool {
    match parts(l).as_slice() {
        ["work", "heritage", d, "pos" | "areas", t] => date(d) && tile(t, 6),
        ["work", "heritage", d, "base", _] => date(d),
        _ => false,
    }
}

fn w_spoken(l: &str, _: bool) -> bool {
    l == "global/spoken"
}

/// A piece's hi pack and mid (a z6 tile); an area's whole run's lo pack and its z6 tiles' hi packs.
fn w_terrain(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["lo", "hi"]) || matches!(parts(l).as_slice(), ["work", "terrain-mid", t] if tile(t, 6))
}

fn w_terrain_lo(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["lo"])
}

/// The water's index for the terrain's repairs, by the content it's made of.
fn w_terrain_water(l: &str, _: bool) -> bool {
    l.strip_prefix(crate::terrain_water::IDX_PREFIX).is_some_and(|k| !k.is_empty() && !k.contains('/'))
}

fn w_terrain_root(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["root"])
}

fn w_slope(l: &str, _: bool) -> bool {
    pack(l, "slope", &["lo", "hi"]) || matches!(parts(l).as_slice(), ["work", "slope-mid", t] if tile(t, 6))
}

fn w_slope_lo(l: &str, _: bool) -> bool {
    pack(l, "slope", &["lo"])
}

fn w_slope_root(l: &str, _: bool) -> bool {
    pack(l, "slope", &["root"])
}

/// The tree layers' hi packs and a piece's mid (a z6 tile); a z3 tile's whole run their lo packs
/// too.
fn w_trees(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| crate::treepacks::LAYERS.contains(&ly), &["lo", "hi"]) || matches!(parts(l).as_slice(), ["work", "trees-mid", t] if tile(t, 6))
}

fn w_trees_lo(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| crate::treepacks::LAYERS.contains(&ly), &["lo"])
}

/// A unit's files (crate::unit::saved_files), of a z6 tile.
fn w_unit(l: &str, _: bool) -> bool {
    l.rsplit_once('/').is_some_and(|(_, t)| tile(t, 6) && crate::unit::saved_files(t).iter().any(|f| f == l))
}

fn w_pois(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "pois", t] if tile(t, 6))
}

fn w_peaks(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "peaks", t] if tile(t, 6))
}

/// A z6 tile's map tiles: its roads' and rails' hi packs and its data.
fn w_pack(l: &str, _: bool) -> bool {
    pack(l, "roads", &["hi"]) || pack(l, "rails", &["hi"]) || matches!(parts(l).as_slice(), ["hidata", t] if tile(t, 6))
}

fn w_lo(l: &str, _: bool) -> bool {
    pack(l, "roads", &["lo"]) || pack(l, "rails", &["lo"])
}

/// The landmarks' Wikidata facts, pageviews and their meta, for the pass.
fn w_items(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "items", d, "facts" | "views" | "meta"] if date(d))
}

/// The heritage chain's files, at the top of the pass's heritage folder (those it didn't write
/// removed).
fn w_heritage(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "heritage", d, stem] if date(d) && !stem.is_empty())
}

/// The landmarks drawn: each kind's zoomed-out packs, their data by z6 tile, the heritage dots and
/// the summary (what it didn't write removed).
fn w_marks(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| ly.strip_prefix("marks-").is_some_and(|k| !k.is_empty()), &["root", "lo"])
        || matches!(parts(l).as_slice(), ["markdata", t] if tile(t, 6))
        || l == crate::markconv::HERITAGE_DOTS
        || l == "global/marks/summary"
}

/// The area overlays' packs, every zoom, their data by z3 tile, and the heritage summaries (what it
/// didn't write removed).
fn w_overlays(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| ["ov-heritage-areas", "ov-indigenous", "ov-special", "ov-whs"].contains(&ly), &ALL) || matches!(parts(l).as_slice(), ["ovdata", t] if tile(t, 3)) || matches!(parts(l).as_slice(), ["global", "heritage", s] if !s.is_empty())
}

fn w_roadunits(l: &str, _: bool) -> bool {
    l == "global/roadunits"
}

fn w_stations(l: &str, _: bool) -> bool {
    pack(l, "stations", &ALL)
}

fn w_ferries(l: &str, _: bool) -> bool {
    pack(l, "ferries", &ALL)
}

/// The rail feeds: each feed's zip, and the lists of those checked, fetched and kept.
fn w_rail_feeds(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "rail", "checked" | "fetched" | "feeds"]) || matches!(parts(l).as_slice(), ["sources", "rail", "gtfs", id] if !id.is_empty())
}

fn w_rail(l: &str, _: bool) -> bool {
    l == "work/rail/used" || l == "global/railfreq"
}

fn w_bldprep(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "bld", t] if tile(t, 6))
}

fn w_bldtiles(l: &str, _: bool) -> bool {
    pack(l, crate::bld::LAYER, &["hi"])
}

/// What the regions no longer cover, removed: a unit's files, candidates and peaks, map tiles, the
/// 3D buildings'. Nothing written.
fn w_prune(l: &str, removal: bool) -> bool {
    removal && (w_unit(l, false) || w_pois(l, false) || w_peaks(l, false) || w_pack(l, false) || w_lo(l, false) || w_bldprep(l, false) || w_bldtiles(l, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::Handoff;
    use crate::pool::journal::{Entry, LeaseId};

    #[test]
    fn the_sets_are_todays() {
        // (As they were before the table: the agent's and the coordinator's rules unchanged.)
        assert_eq!(SHARED, ["terrain", "slope", "trees", "unit", "pois", "peaks", "bldprep", "bldtiles"]);
        assert_eq!(SECOND, ["heritage", "items", "rail-feeds", "rail", "bld-fetch", "marks", "overlays", "pois", "peaks", "unit", "slope", "bldprep", "bldtiles"]);
        assert_eq!(ALONE, ["osm-pass", "pass-sets", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels", "water", "heritage-sites", "gc"]);
        let set = |v: &[&'static str]| v.iter().copied().collect::<std::collections::BTreeSet<&str>>();
        assert_eq!(set(&RAW), set(&["terrain", "terrain-lo", "terrain-root", "terrain-z8", "peaks"]));
        assert_eq!(set(&WIKI), set(&["items", "heritage"]));
        assert_eq!(NAS_READS, ["bldprep"]);
        assert_eq!(set(&ANSWERED), set(&["items", "heritage-sites", "heritage"]));
    }

    #[test]
    fn the_rows_are_todays() {
        // The memory first guessed (what the agent's first_peak and second_peak gave, the latter's
        // where they differed: the network steps', a unit's and candidates', which nothing offered
        // by the former), the batches, the disk.
        let mem: [(&str, u64); 19] = [("heritage", 6144), ("rail", 6144), ("items", 3072), ("rail-feeds", 1024), ("bld-fetch", 1024), ("marks", 4096), ("overlays", 4096), ("unit", 8600), ("pois", 2048), ("trees", 1000), ("trees-lo", 500), ("terrain-lo", 1200), ("slope", 1500), ("slope-lo", 1000), ("peaks", 2500), ("water", 8192), ("bldprep", 5200), ("bldtiles", 3400), ("terrain", 1500)];
        for (s, mb) in mem {
            assert_eq!(mem_mb(s), mb, "{s}");
        }
        assert_eq!(mem_mb("labels"), 1500);
        assert_eq!(mem_mb("not a step"), 1500);
        let batches: [(&str, usize); 13] = [("terrain", 8), ("slope", 8), ("terrain-lo", 4), ("slope-lo", 4), ("trees", 4), ("trees-lo", 4), ("bldprep", 8), ("bldtiles", 16), ("lo", 2), ("unit", 6), ("peaks", 12), ("pack", 16), ("pois", 24)];
        for (s, n) in batches {
            assert_eq!(batch(s), n, "{s}");
        }
        assert_eq!(batch("catalog"), usize::MAX);
        assert_eq!(row("terrain").unwrap().disk, RESERVE + 5 * GB);
        assert_eq!(row("water").unwrap().disk, RESERVE + 5 * GB);
        assert_eq!(row("osm-pass").unwrap().disk, super::super::PASS_SPACE);
        assert_eq!(row("unit").unwrap().disk, RESERVE);
        // No row twice.
        for s in TABLE.iter().map(|s| s.name) {
            assert_eq!(TABLE.iter().filter(|r| r.name == s).count(), 1, "{s}");
        }
    }

    #[test]
    fn a_shared_steps_saves_are_in_its_write_set() {
        for (step, t) in [("unit", "6/3/4"), ("pois", "6/3/4"), ("peaks", "6/3/4"), ("bldprep", "6/3/4"), ("bldtiles", "6/3/4"), ("trees", "6/3/4"), ("trees", "3/1/1"), ("trees-lo", "3/1/1"), ("terrain", "6/3/4"), ("terrain", "3/1/1"), ("slope", "6/3/4"), ("slope", "3/1/1"), ("terrain-lo", "3/1/1"), ("slope-lo", "3/1/1")] {
            let dash = t.replace('/', "-");
            let mut names: Vec<String> = Vec::new();
            for l in ["base", "global/roads", "global/roaden", "layers/grid-class/hi", "layers/grid-canopy/hi", "layers/grid-cover/hi", "work/pois", "work/peaks", "work/bld", "layers/buildings/hi", "work/trees-mid", "work/terrain-mid", "work/slope-mid", "layers/terrain/hi", "layers/terrain/lo", "layers/slope/hi", "layers/slope/lo", "layers/trees-cover/hi", "layers/trees-leaf/lo"] {
                names.push(format!("{l}/{dash}"));
                names.push(format!("{l}/6-8-9"));
            }
            let saved: Vec<&String> = names.iter().filter(|l| saves(step, t, l)).collect();
            assert!(!saved.is_empty(), "{step} {t} saves something");
            for l in saved {
                assert!((row(step).unwrap().writes)(l, false), "{step} {t}: {l}");
            }
        }
        assert!(!saves("pack", "6/3/4", "hidata/6-3-4"), "not a shared step");
    }

    #[test]
    fn every_step_the_agent_runs_has_a_row() {
        // The steps the agent makes jobs of, as its source names them: the planner's work
        // (`Work { step: "…"`), the jobs it titles (`job(format!("…`, `id: "…"`), the steps the
        // checklist labels (build::label's arms), and those whose names it puts together.
        let (agent, build) = (include_str!("mod.rs"), include_str!("build.rs"));
        let mut named = std::collections::BTreeSet::<String>::new();
        let mut take = |src: &str, pat: &str| {
            for (i, _) in src.match_indices(pat) {
                let name: String = src[i + pat.len()..].chars().take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-').collect();
                if name.len() > 1 {
                    named.insert(name);
                }
            }
        };
        take(build, "Work { step: \"");
        take(agent, "Work { step: \"");
        take(agent, "job(format!(\"");
        take(agent, "id: \"");
        take(agent, "id: format!(\"");
        let label = &build[build.find("pub fn label(").unwrap()..];
        let label = &label[..label.find("\n}\n").unwrap()];
        for arm in label.lines().filter_map(|l| l.trim().strip_prefix('"')) {
            for alt in arm.split(" | ") {
                take(&format!("\"{alt}"), "\"");
            }
        }
        named.extend(["stations", "ferries", "terrain-lo", "slope-lo", "terrain-root", "slope-root", "unit", "terrain-z8"].map(String::from));
        // (Not steps: a task's lease.)
        named.remove("task");
        assert!(named.len() >= 40, "{named:?}");
        let missing: Vec<&String> = named.iter().filter(|s| row(s).is_none()).collect();
        assert!(missing.is_empty(), "steps the agent runs with no row: {missing:?} (of {named:?})");
        // And no row for a step the agent never runs.
        let unused: Vec<&str> = TABLE.iter().map(|s| s.name).filter(|s| !named.contains(*s)).collect();
        assert!(unused.is_empty(), "rows of no step the agent runs: {unused:?}");
    }

    fn entry(step: &str, changes: &[(&str, bool)]) -> Entry {
        let mut h = Handoff::default();
        for (l, removed) in changes {
            h.changes.insert(l.to_string(), (!removed).then(|| format!("{l}.0123456789abcdef.bin")));
        }
        Entry { member: "m-0000000000000001".into(), lease: LeaseId { term: 1, n: 1 }, step: step.into(), handoff: h, at: 1_791_500_000 }
    }

    #[test]
    fn entries_within_their_write_sets() {
        let ok: [(&str, &[(&str, bool)]); 24] = [
            ("osm-pass", &[("sources/osm/2026-09-28/pieces/6-1-2", false), ("sources/osm/2026-09-28/pass", false), ("layers/basemap/world-2026-09-28", false), ("sources/osm/2026-08-01/sets/water", true), ("work/heritage/2026-08-01/base/heritage", true), ("work/summits/2026-08-01", true)]),
            ("pass-sets", &[("sources/osm/2026-09-28/sets/water-v2", false)]),
            ("water", &[("layers/water/root/0-0-0", false), ("layers/water/lo/3-1-2", false), ("layers/water/hi/6-1-2", false), ("layers/smallwater/hi/6-1-2", true)]),
            ("labels", &[("layers/labels/hi/6-1-2", false), ("layers/labels/root/0-0-0", true)]),
            ("spoken", &[("global/spoken", false)]),
            ("heritage-sites", &[("work/heritage/2026-09-28/pos/6-1-2", false), ("work/heritage/2026-09-28/base/area-shapes", false), ("work/heritage/2026-09-28/areas/6-3-3", true)]),
            ("terrain", &[("layers/terrain/hi/6-10-20", false), ("work/terrain-mid/6-10-20", false), ("layers/terrain/lo/3-1-2", false)]),
            ("terrain-lo", &[("layers/terrain/lo/3-1-2", false)]),
            ("terrain-water", &[("work/water-idx/0f1a2b3c4d5e6f70", false)]),
            ("slope-root", &[("layers/slope/root/0-0-0", false)]),
            ("trees", &[("layers/trees-cover/hi/6-1-2", false), ("work/trees-mid/6-1-2", true)]),
            ("unit", &[("base/6-1-2", false), ("global/roaden/6-1-2", true), ("layers/grid-canopy/hi/6-1-2", false)]),
            ("pack", &[("hidata/6-1-2", false), ("layers/roads/hi/6-1-2", false), ("layers/rails/hi/6-1-2", true)]),
            ("lo", &[("layers/roads/lo/3-1-2", false)]),
            ("items", &[("sources/items/2026-09-28/facts", false)]),
            ("heritage", &[("work/heritage/2026-09-28/details-harea", false), ("work/heritage/2026-09-28/layer-whs", true)]),
            ("marks", &[("layers/marks-peak/lo/3-1-2", false), ("markdata/6-1-2", true), ("global/marks/summary", false), ("work/marks/heritage-dots", false)]),
            ("overlays", &[("layers/ov-whs/hi/6-1-2", false), ("ovdata/3-1-2", false), ("global/heritage/layer-summary", false)]),
            ("rail-feeds", &[("sources/rail/gtfs/de-db", false), ("sources/rail/feeds", false)]),
            ("rail", &[("global/railfreq", false)]),
            ("bldprep", &[("work/bld/6-56-25", true)]),
            ("bldtiles", &[("layers/buildings/hi/6-56-25", false)]),
            ("prune", &[("base/6-1-2", true), ("layers/roads/lo/3-1-2", true)]),
            ("catalog", &[]),
        ];
        for (step, ch) in ok {
            assert_eq!(outside(&entry(step, ch)), None, "{step}");
        }
        let z8 = [crate::terrain_z8::logical(), crate::terrain_z8::max_logical()];
        assert_eq!(outside(&entry("terrain-z8", &[(&z8[0], false), (&z8[1], false)])), None);
        assert!(outside(&entry("terrain-z8", &[("sources/terrain-z8-v0", false)])).is_some());
        // Outside: another step's files, a name of the wrong zoom, another layer, a pass's files
        // removed by a step that doesn't retire passes, a prune that writes, a removal the step
        // doesn't make.
        let bad: [(&str, &[(&str, bool)]); 9] = [
            ("pack", &[("hidata/6-1-2", false), ("layers/terrain/hi/6-1-2", false)]),
            ("terrain", &[("work/terrain-mid/3-1-2", false)]),
            ("lo", &[("layers/roads/lo/6-1-2", false)]),
            ("labels", &[("layers/stations/hi/6-1-2", false)]),
            ("items", &[("sources/items/2026-08-01/facts", false), ("work/summits/2026-08-01", true)]),
            ("prune", &[("base/6-1-2", false)]),
            ("water", &[("layers/smallwater/hi/6-1-2", false)]),
            ("catalog", &[("catalog/1", false)]),
            ("unit", &[("base/6-64-2", false)]),
        ];
        for (step, ch) in bad {
            assert!(outside(&entry(step, ch)).is_some(), "{step}: {ch:?}");
        }
        let why = outside(&entry("pack", &[("hidata/6-1-2", false), ("layers/terrain/hi/6-1-2", false), ("work/x", true)])).unwrap();
        assert!(why.contains("layers/terrain/hi/6-1-2") && why.contains("and 1 more"), "{why}");
        // A step the table doesn't know: outside, if it changes anything.
        assert!(outside(&entry("newer-step", &[("x/y", false)])).unwrap().contains("isn't a step"));
        assert_eq!(outside(&entry("newer-step", &[])), None);
    }

    /// Every entry of a copy of the NAS's journal (`SCENIC_JOURNAL`: a folder of its day folders)
    /// within its step's write-set: `SCENIC_JOURNAL=… cargo test -p pipeline real_journal --
    /// --ignored`.
    #[test]
    #[ignore]
    fn real_journal_entries_are_within_their_write_sets() {
        let Some(dir) = std::env::var_os("SCENIC_JOURNAL") else { panic!("SCENIC_JOURNAL names no folder") };
        let (mut n, mut bad) = (0, Vec::new());
        let mut steps = std::collections::BTreeMap::<String, usize>::new();
        for day in std::fs::read_dir(&dir).unwrap().flatten() {
            for f in std::fs::read_dir(day.path()).into_iter().flatten().flatten() {
                let Ok(b) = std::fs::read(f.path()) else { continue };
                let Ok(e) = serde_json::from_slice::<Entry>(&b) else { continue };
                n += 1;
                *steps.entry(e.step.clone()).or_default() += 1;
                if let Some(why) = outside(&e) {
                    bad.push(format!("{}: {why}", f.path().display()));
                }
            }
        }
        eprintln!("{n} entries: {steps:?}");
        assert!(n > 0);
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
