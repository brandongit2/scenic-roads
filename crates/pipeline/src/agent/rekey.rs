//! The build's records re-keyed when a step's key scheme changes (docs/building.md §2, A new key
//! scheme), so that the change itself rebuilds only what it would make differently. A target whose
//! recorded key is what the old scheme computes now is current; it's recorded under the new scheme's
//! key when every input the new key names is pinned by the old key's, and its record goes otherwise
//! (it's built again). Applied in memory by every reader of the records (`as_read`), never saved: a
//! record changes only by a job's save; `scenic-build p5-check trees` shows what it would do.
//!
//! Tree cover (2026-10-06): a z3 tile's whole run ("3/x/y", `v1::trees_targets`, keyed on the
//! coverage in the z3 tile) became a z6 tile per z6 tile (keyed on the coverage in it) and an
//! assembly per z3 tile (keyed on its z6 tiles' mids: crate::treepacks::targets). A z3 tile current
//! under the old scheme pins its z6 tiles' inputs (the coverage in a z6 tile is a function of the
//! coverage in its z3 tile: Coverage::fingerprint), and its packs are what the z6 tiles and assembly
//! make from them, byte for byte, when the trees program made them: its z6 tiles and assembly are
//! recorded under their keys, without mids (none exist: "-" in the assembly's key), which are made
//! in idle time, expected the same (agent::build::TreeWork::backfill). Packs trees.py made (before
//! the program took its place, `TREES_PROGRAM_SINCE`, by their files' times) have the same pixels in
//! other bytes: not pinned, the z3 tile's record goes and its z6 tiles are made again. Every z3
//! record goes: a stale one is built again as extracts either way, and one of "none" (the coverage
//! gone from it) has nothing left to build.
//!
//! A z3 record merged after the switch (a lease granted before it, handed off since; or an older
//! app's, rolled back to and forward again) comes with its whole run's packs, written over those of
//! the z3 tile's z6 tiles and assembly made since. Current and the program's, it's re-keyed as above,
//! but for a z6 tile recorded since under another key, or with a mid and no record: its mid isn't of
//! this coverage, so its record goes and it's made again. Otherwise it goes with the records of the
//! z3 tile's z6 tiles and assembly, which are all made again.

use super::build::Keys;
use super::tiles::TerrainTiles;
use crate::coverage::Coverage;
use crate::legacy::TileId;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Tree cover's key scheme before 2026-10-06, before its z6 tiles.
pub mod v1 {
    use crate::coverage::Coverage;
    use crate::legacy::TileId;
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
                let had = || {
                    LAYERS.iter().any(|l| {
                        m.contains_key(&format!("layers/{l}/lo/3-{x}-{y}"))
                            || m.range(format!("layers/{l}/hi/6-")..)
                                .take_while(|(k, _)| k.starts_with(&format!("layers/{l}/hi/6-")))
                                .any(|(k, _)| {
                                    TileId::parse(&k[format!("layers/{l}/hi/").len()..])
                                        .is_some_and(|u| (u.x >> 3, u.y >> 3) == (x, y))
                                })
                    })
                };
                if cov.meets_rect(b) {
                    out.push((
                        t.clone(),
                        store::naming::hash16(format!("trees {TREES_V}|{t}|{}", cov.fingerprint(b)).as_bytes()),
                    ));
                } else if had() {
                    out.push((t.clone(), trees_none(&t)));
                }
            }
        }
        out
    }

    /// A z3 tile's "none" key under the old scheme (the coverage gone from it).
    pub fn trees_none(t: &str) -> String {
        store::naming::hash16(format!("trees {TREES_V}|{t}|none").as_bytes())
    }
}

/// What `rekey` did to tree cover's records (by z3 tile, "3/x/y").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rekeyed {
    /// Tree cover's z3 tiles current under the old scheme, recorded as their z6 tiles and assembly.
    pub trees_moved: Vec<String>,
    /// Those whose records went, each with why: their packs trees.py's, stale under the old scheme
    /// (each built again as z6 tiles), or "none" (nothing left to build); with them the records of
    /// their z6 tiles and assembly made since the switch (a late record's whole run wrote over their
    /// packs).
    pub trees_dropped: Vec<(String, String)>,
    /// Those whose packs' times can't be read now: kept as they are, for the next pass.
    pub trees_unknown: Vec<(String, String)>,
}

impl Rekeyed {
    /// Whether it changed the records.
    pub fn changed(&self) -> bool {
        !self.trees_moved.is_empty() || !self.trees_dropped.is_empty()
    }
}

/// When the trees program took trees.py's place in the published app (20261006-0711-5e3c69a,
/// 2026-10-06 07:11 UTC): tree packs written before are trees.py's, the same pixels in other WebP
/// bytes than the program's z6 tiles make.
pub const TREES_PROGRAM_SINCE: i64 = 1_791_270_660;

/// What the re-keying reads of the NAS beyond the records and the manifest: its files' times
/// (`FileTimes`).
pub trait Times {
    /// Whether z3 tile (x, y)'s tree packs were made by the trees program
    /// (`FileTimes::trees_by_program`).
    fn trees_by_program(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool>;
}

impl Times for FileTimes<'_> {
    fn trees_by_program(&self, m: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
        FileTimes::trees_by_program(self, m, x, y)
    }
}

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
        let t = std::fs::metadata(self.root.join(content))
            .and_then(|md| md.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        self.seen.borrow_mut().insert(content.to_string(), t);
        t
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
                m.range(prefix.clone()..)
                    .take_while(|(k, _)| k.starts_with(&prefix))
                    .find(|(k, _)| TileId::parse(&k[prefix.len()..]).is_some_and(|u| (u.x >> 3, u.y >> 3) == (x, y)))
                    .map(|(_, c)| c)
            })
        };
        let Some(c) = lo.or_else(hi) else { return Some(true) };
        Some(self.time(c)? >= TREES_PROGRAM_SINCE)
    }
}

/// Re-keys the records in `keys`, for the coverage `cov`, the manifest `m` and the declared sources
/// the keys name (`tiles`): tree cover's from a z3 tile's whole run to z6 tiles and assemblies
/// (`rekey_trees`). `times`: the NAS's files' times (`FileTimes`; None where they can't be told now).
pub fn rekey(
    keys: &mut Keys,
    cov: &Coverage,
    m: &BTreeMap<String, String>,
    tiles: &TerrainTiles,
    times: &dyn Times,
) -> Rekeyed {
    let mut out = Rekeyed::default();
    // (What the keys name of the declared sources: none, and tree cover waits.)
    let Some(keyed) = tiles.keyed(m) else { return out };
    rekey_trees(keys, cov, m, times, keyed.trees.as_deref(), &mut out);
    out
}

/// Re-keys tree cover's records of a z3 tile's whole run ("3/x/y", `v1::trees_targets`) as its
/// z6 tiles and assembly (crate::treepacks::targets), into `out`: one current under the old scheme
/// whose packs the trees program made is recorded as its z6 tiles (each the coverage meets) and its
/// assembly, under their keys now (a z6 tile recorded since under another key, or with a mid and no
/// record, made again: its mid isn't of this coverage); every other z3 record goes (stale,
/// trees.py's packs, or "none"), and with it the records of the z3 tile's z6 tiles and assembly (its
/// whole run, merged after the switch, wrote over their packs); one whose packs' times can't be
/// read now is kept for the next pass.
fn rekey_trees(
    keys: &mut Keys,
    cov: &Coverage,
    m: &BTreeMap<String, String>,
    times: &dyn Times,
    sources: Option<&str>,
    out: &mut Rekeyed,
) {
    let z3: Vec<(String, String)> = keys
        .trees
        .iter()
        .filter(|(t, _)| TileId::parse(t).is_some_and(|u| u.z == 3))
        .map(|(t, k)| (t.clone(), k.clone()))
        .collect();
    if z3.is_empty() {
        return;
    }
    let old: BTreeMap<String, String> = v1::trees_targets(cov, m).into_iter().collect();
    let tt = crate::treepacks::targets_with(cov, m, sources);
    let has_mid = |t: &str| TileId::parse(t).is_some_and(|p| m.contains_key(&crate::treepacks::mid_logical(p.x, p.y)));
    for (q, k) in z3 {
        let Some(u) = TileId::parse(&q) else { continue };
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
        let since: Vec<String> =
            keys.trees.keys().filter(|t| crate::treepacks::z3_of(t).as_deref() == Some(q.as_str())).cloned().collect();
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

/// What the steps' keys named of the sources declared in `inputs/sources/` (#145) while they were in
/// code: the digest (`Source::digest`) of each source's declaration as staged for the switch
/// (producers/sources/), whose fields are what the code had (each consumer's tests). While the
/// accepted declaration has that digest a key is what it was (`Source::key_part`, `key_extra`), so
/// nothing is made again for the move; a declaration that names anything else is a new key.
pub mod declared_before {
    /// The lake fill's (canvec-lakes.toml): its key named the zips (their pin, `run::source_pin`),
    /// not the layer and values the fill keeps.
    pub const LAKE_FILL: &str = "25eb06ca157bccd7";
    /// The land cover's (esa-worldcover.toml), the canopy's (meta-canopy.toml, with its index) and
    /// the leaf types' (eea-leaf-type.toml, nalcms.toml): no key named them in code (the z6 tiles' grids
    /// and tree cover from fixed datasets); a key names them once they're other than staged.
    pub const LAND_COVER: &str = "8f325c73404dbb3e";
    pub const CANOPY: &str = "1b78cff281da155d";
    pub const LEAF_EUROPE: &str = "fc5221f05e2c6ff7";
    pub const LEAF_NORTH_AMERICA: &str = "79b40a5faa30a391";
    /// The buildings' (overture-buildings.toml): the roadside buildings' and the 3D buildings' keys
    /// named the release alone (2026-09-23.1, then in code).
    pub const BUILDINGS: &str = "c3a4a3fb72797382";
    /// The building heights' (ghsl-built-h.toml): the bldprep keys named the GHSL tiles it read (by
    /// name and size), not where they come from.
    pub const BUILDING_HEIGHTS: &str = "909a01a4faed972e";
}

/// The records as every reader reads them (the plan, the checklist, the regions' state, the jobs'
/// `--expect-same`): re-keyed (`rekey`). The one way to read them.
pub fn as_read(
    keys: &mut Keys,
    cov: &Coverage,
    m: &BTreeMap<String, String>,
    tiles: &TerrainTiles,
    times: &dyn Times,
) -> Rekeyed {
    rekey(keys, cov, m, tiles, times)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The files' times as a test has them: whether a z3 tile's tree packs are the trees program's.
    struct Stub<G>(G);

    impl<G: Fn(u32, u32) -> Option<bool>> Times for Stub<G> {
        fn trees_by_program(&self, _: &BTreeMap<String, String>, x: u32, y: u32) -> Option<bool> {
            (self.0)(x, y)
        }
    }

    type At = fn(u32, u32) -> Option<bool>;

    /// Every z3 tile's tree packs the program's.
    const NONE_OLDER: Stub<At> = Stub(|_, _| Some(true));
    use crate::agent::build;
    use crate::agent::build::tests::{did, e7box, heritage_chain_done, heritage_done, tiles_for, unit_inputs};
    use crate::agent::build::{Publish, Work};
    use crate::agent::recipes::Recipe;
    use crate::reach::{Reach, Reaches};

    /// Iceland: regions a (Reykjavik, z6 tile 6/28/17) and b (Akureyri and Egilsstaðir: 6/28/16,
    /// 6/29/16), all in z3 tile 3/3/2; the heritage sites and the terrain done (its lo pack, and the hi
    /// packs of its extracts), each z6 tile built under the old keys with its outputs. `reaches` changes
    /// the z6 tiles' reaches first.
    fn iceland(reaches: &dyn Fn(&mut Reaches)) -> (Coverage, Reaches, BTreeMap<String, String>, Keys) {
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, places: &[&str]| Recipe {
            id: id.into(),
            name: id.to_uppercase(),
            outline: places.iter().map(|p| format!("place:{p},20")).collect(),
        };
        let c = Coverage::fixture(&[r("a", &["-21.9,64.13"]), r("b", &["-18.1,65.68", "-14.4,65.26"])], None, d.path())
            .unwrap();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        for (u, b) in [
            ("6/28/17", e7box(-22.0, 64.0, -21.7, 64.16)),
            ("6/28/16", e7box(-18.3, 65.6, -17.9, 65.8)),
            ("6/29/16", e7box(-14.6, 65.2, -14.2, 65.35)),
        ] {
            reach.units.insert(u.into(), Reach { cells: vec![], owned: Some(b), long: vec![] });
        }
        reaches(&mut reach);
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-17", "6-28-16", "6-29-16"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        unit_inputs(&mut m, "d");
        let mut done = Keys::default();
        // (The terrain made by its z3 tile's whole run, as before its z6 tiles.)
        m.insert("layers/terrain/lo/3-3-2".into(), "layers/terrain/lo/3-3-2.1111111111111111.pack".into());
        for (x, y) in build::coverage_tiles(&c).into_values().flatten() {
            m.insert(
                format!("layers/terrain/hi/6-{x}-{y}"),
                format!("layers/terrain/hi/6-{x}-{y}.2222222222222222.pack"),
            );
        }
        let tt = build::terrain_slope_targets(&c, &m, &tiles_for(&m));
        done.record("terrain", &tt.terrain);
        done.record("terrain-lo", &tt.terrain_lo);
        loop {
            let w = plan(&c, &reach, &m, &done, &tiles_for(&m)).work;
            match w[0].step.as_str() {
                "heritage-sites" => heritage_done(&mut m, &mut done, "d", &w[0]),
                _ => break,
            }
        }
        heritage_chain_done(&c, &m, &mut done, "d");
        for (u, k) in build::tests::ukeys(&c, "d", &m, Some(&reach)) {
            done.record("unit", &[(u.slash(), k)]);
            m.insert(format!("base/{}", u.dash()), format!("base/{}.6666666666666666.base", u.dash()));
        }
        (c, reach, m, done)
    }

    /// The plan.
    fn plan(
        c: &Coverage,
        reach: &Reaches,
        m: &BTreeMap<String, String>,
        done: &Keys,
        tiles: &TerrainTiles,
    ) -> build::Plan {
        build::plan(
            c,
            "d",
            m,
            done,
            &BTreeMap::new(),
            Some(reach),
            tiles,
            Publish {
                each: &c.by_region(),
                on_map: &BTreeMap::new(),
                served: &BTreeMap::new(),
                since_catalog: None,
                held_for: None,
                held: false,
                way: Default::default(),
            },
        )
    }

    #[test]
    fn tree_covers_old_targets_were_the_z3_tiles_the_coverage_meets() {
        let d = tempfile::tempdir().unwrap();
        let cov = |o: &str| {
            Coverage::fixture(&[Recipe { id: "r".into(), name: "R".into(), outline: vec![o.into()] }], None, d.path())
                .unwrap()
        };
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
        let (c, _, mut m, mut done) = iceland(&|_| {});
        let tiles = tiles_for(&m);
        // Iceland's z3 tile's whole run recorded under the old scheme, its packs in the manifest.
        m.insert("layers/trees-cover/lo/3-3-2".into(), "layers/trees-cover/lo/3-3-2.5555555555555555.pack".into());
        m.insert("layers/trees-cover/hi/6-28-16".into(), "layers/trees-cover/hi/6-28-16.5555555555555555.pack".into());
        let old = v1::trees_targets(&c, &m);
        assert_eq!(old.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["3/3/2"]);
        done.record("trees", &old);
        // As recorded: every z6 tile and the assembly made again.
        let tt = crate::treepacks::targets(&c, &m);
        assert_eq!(build::tree_work(&tt, &m, &done).pieces.len(), 3);
        // Its packs the trees program's: each z6 tile the coverage meets and the assembly recorded
        // under their keys now, its z3 record gone; nothing stale, the z6 tiles' mids made in idle
        // time.
        let r = rekey(&mut done, &c, &m, &tiles, &NONE_OLDER);
        assert_eq!(
            (r.trees_moved.clone(), r.trees_dropped.len(), r.trees_unknown.len()),
            (vec!["3/3/2".to_string()], 0, 0)
        );
        assert!(r.changed() && !done.trees.contains_key("3/3/2"));
        assert_eq!(done.trees.keys().collect::<Vec<_>>(), ["6/28/16", "6/28/17", "6/29/16"]);
        let w = build::tree_work(&tt, &m, &done);
        assert!(w.pieces.is_empty() && w.lo.is_empty() && w.stale_pieces.is_empty() && w.stale_lo.is_empty(), "{w:?}");
        assert_eq!(w.backfill.len(), 3);
        // Again: nothing to do, nothing changed.
        let before = done.clone();
        let again = rekey(&mut done, &c, &m, &tiles, &NONE_OLDER);
        assert!(!again.changed() && done == before);
        // A late record under the old scheme (a lease of an older app's, merged since): re-keyed.
        done.record("trees", &old);
        assert_eq!(rekey(&mut done, &c, &m, &tiles, &NONE_OLDER).trees_moved, ["3/3/2"]);
        assert_eq!(done, before);
        // Its packs trees.py's: its record goes, the z6 tiles and assembly made again.
        let only = |k: &[(String, String)]| {
            let mut keys = Keys::default();
            keys.record("trees", k);
            keys
        };
        let mut py = only(&old);
        let r = rekey(&mut py, &c, &m, &tiles, &Stub(|_, _| Some(false)));
        assert!(
            r.trees_dropped.len() == 1
                && r.trees_dropped[0].1.contains("are trees.py's")
                && py.trees.is_empty()
                && py.trees_lo.is_empty(),
            "{r:?}"
        );
        // Their times not readable now: kept, for the next pass.
        let mut unknown = only(&old);
        let r = rekey(&mut unknown, &c, &m, &tiles, &Stub(|_, _| None));
        assert!(r.trees_unknown.len() == 1 && !r.changed() && unknown == only(&old), "{r:?}");
        // Stale under the old scheme: its record goes (its extracts made again either way).
        let mut stale = only(&[("3/3/2".to_string(), "0000000000000000".to_string())]);
        let r = rekey(&mut stale, &c, &m, &tiles, &NONE_OLDER);
        assert!(r.trees_dropped[0].1.starts_with("stale") && stale.trees.is_empty(), "{r:?}");
        // "none", current (the coverage gone, the packs not yet dropped): nothing left to build.
        let none = [("3/4/2".to_string(), v1::trees_none("3/4/2"))];
        m.insert("layers/trees-cover/hi/6-33-23".into(), "x".into());
        let mut gone = only(&none);
        let r = rekey(&mut gone, &c, &m, &tiles, &NONE_OLDER);
        assert!(r.trees_dropped[0].1.contains("nothing left to build") && gone.trees.is_empty(), "{r:?}");
    }

    #[test]
    fn a_late_whole_runs_record_goes_with_those_of_the_pieces_and_assembly_made_since() {
        let (c, _, mut m, mut done) = iceland(&|_| {});
        let tiles = tiles_for(&m);
        // Since the switch: 3/3/2's z6 tiles made (their mids in the manifest), then its assembly; and
        // a z6 tile of another z3 tile.
        let pieces: Vec<(String, String)> =
            crate::treepacks::targets(&c, &m).pieces.iter().map(|p| (p.0.clone(), p.1.clone())).collect();
        did(&mut m, &mut done, &Work { step: "trees".into(), targets: pieces });
        let tt = crate::treepacks::targets(&c, &m);
        did(
            &mut m,
            &mut done,
            &Work { step: "trees-lo".into(), targets: tt.lo.iter().map(|l| (l.0.clone(), l.1.clone())).collect() },
        );
        done.trees.insert("6/33/22".into(), "1111111111111111".into());
        let made = done.clone();
        assert_eq!(build::tree_work(&tt, &m, &done), build::TreeWork::default());
        let pieces_of = |k: &Keys| build::tree_work(&tt, &m, k).pieces.into_iter().map(|p| p.0).collect::<Vec<_>>();
        // A late hand-off of 3/3/2's whole run (its lease granted before the switch), for a coverage
        // there since changed: stale under the old scheme. Its record goes, and with it those of the
        // z6 tiles and assembly made since (it wrote over their packs), which are made again; the other
        // z3 tile's z6 tile stays.
        done.record("trees", &[("3/3/2".to_string(), "0000000000000000".to_string())]);
        let r = rekey(&mut done, &c, &m, &tiles, &NONE_OLDER);
        assert!(r.trees_dropped.len() == 1 && r.trees_dropped[0].1.contains("its 3 pieces and its assembly"), "{r:?}");
        assert_eq!(
            (done.trees.keys().map(String::as_str).collect::<Vec<_>>(), done.trees_lo.len()),
            (vec!["6/33/22"], 0)
        );
        assert_eq!(
            (pieces_of(&done), build::tree_work(&tt, &m, &done).stale_lo.len()),
            (vec!["6/28/16".to_string(), "6/28/17".into(), "6/29/16".into()], 1)
        );
        // Again: nothing to do.
        let before = done.clone();
        assert!(!rekey(&mut done, &c, &m, &tiles, &NONE_OLDER).changed() && done == before);
        // One current under the old scheme, the program's: re-keyed over what was made since, but a
        // z6 tile recorded since under another key (the coverage there otherwise then), or with a mid
        // and no record, is made again: its mid isn't of this coverage.
        let mut late = made.clone();
        late.trees.insert("6/28/17".into(), "2222222222222222".into());
        late.trees.remove("6/29/16");
        late.record("trees", &v1::trees_targets(&c, &m));
        let r = rekey(&mut late, &c, &m, &tiles, &NONE_OLDER);
        assert_eq!(r.trees_moved, ["3/3/2"]);
        assert_eq!(late.trees.keys().collect::<Vec<_>>(), ["6/28/16", "6/33/22"]);
        assert_eq!(
            (late.trees.get("6/28/16"), late.trees_lo.get("3/3/2")),
            (made.trees.get("6/28/16"), made.trees_lo.get("3/3/2"))
        );
        assert_eq!(pieces_of(&late), ["6/28/17", "6/29/16"]);
    }

    #[test]
    fn tree_packs_made_by_the_trees_program_by_the_files_times() {
        let d = tempfile::tempdir().unwrap();
        let since = std::time::UNIX_EPOCH + std::time::Duration::from_secs(TREES_PROGRAM_SINCE as u64);
        let m: BTreeMap<String, String> = [
            ("layers/trees-cover/lo/3-3-2", "a"),
            ("layers/trees-height/lo/3-4-2", "b"),
            ("layers/trees-leaf/hi/6-56-25", "c"),
            ("layers/trees-cover/lo/3-1-2", "missing"),
        ]
        .into_iter()
        .map(|(l, n)| (l.to_string(), format!("{l}.{n}.pack")))
        .collect();
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
        assert_eq!(
            (t.trees_by_program(&m, 3, 2), t.trees_by_program(&m, 4, 2), t.trees_by_program(&m, 7, 3)),
            (Some(false), Some(true), Some(true))
        );
        // None at all: none to differ. One not on the NAS: can't be told.
        assert_eq!((t.trees_by_program(&m, 0, 0), t.trees_by_program(&m, 1, 2)), (Some(true), None));
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
            v.extend(
                self.lakes
                    .iter()
                    .filter(|l| (l.0, l.1, l.2) == (z, x, y))
                    .map(|l| square(Kind::Lake, l.3, 100.0, 100.0, 140.0, 130.0)),
            );
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
    fn with_water(
        m: &BTreeMap<String, String>,
        basemap: Option<&str>,
        idx: &[&WaterIdx],
    ) -> (BTreeMap<String, String>, TerrainTiles) {
        let mut m = m.clone();
        if let Some(b) = basemap {
            let l = b.split('.').next().unwrap();
            m.insert(l.into(), b.into());
        }
        let mut tiles = tiles_for(&m);
        for w in idx {
            let l = crate::terrain_water::idx_logical(&w.pin);
            let c = format!("{l}.{}.json", store::naming::hash16(serde_json::to_string(w).unwrap().as_bytes()));
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

    /// The first z9 tile z6 tile `t` reads.
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
        // One in a z7 tile: the assembly's alone (the z6 tiles read z9–12).
        let b = keys(B, &at(B, vec![(9, qz9.0, qz9.1, 5), (7, p.0 << 1, p.1 << 1, 9)]));
        assert_ne!(a["3/3/2"], b["3/3/2"]);
        assert_eq!(a.iter().filter(|(t, k)| b[*t] != **k).count(), 1);
        // The same water under another name: the same keys.
        assert_eq!(a, keys(B, &at(B, vec![(9, qz9.0, qz9.1, 5)])));
        // Its digests not made yet (or not readable now): its keys unknown, the job offered, nothing
        // made meanwhile, and its z3 tile waits.
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
                let u = TileId::parse(t).unwrap();
                let k = if u.z == 6 {
                    let fp = c.fingerprint(build::grown_e7(6, u.x, u.y, 20.0));
                    let ds = tf.terrain_datasets(&mf, crate::stage::tile_box_grown(6, u.x, u.y, 0.0)).unwrap();
                    // (And what its deep levels read: crate::terrain_deep.)
                    let deep = crate::terrain_deep::piece_key_lines(&tf.fine(&mf).unwrap(), &c, (u.x, u.y));
                    let ds = format!("{ds}\n{deep}");
                    build::terrain_piece_key((u.x, u.y), &fp, &ds, &key(t, &fp))
                } else {
                    let mids: Vec<String> =
                        build::coverage_tiles(&c)[&(u.x, u.y)].iter().map(|(x, y)| format!("6/{x}/{y}=-")).collect();
                    build::terrain_lo_key(
                        (u.x, u.y),
                        &key(t, crate::terrain_water::AREA_FROM),
                        &tf.terrain_datasets(&mf, crate::stage::tile_box_grown(3, u.x, u.y, 0.0)).unwrap(),
                        &mids,
                    )
                };
                (t.clone(), k)
            })
            .collect();
        let changed: Vec<&String> = before.keys().filter(|t| before[*t] != after[*t]).collect();
        // The z6 tile under the fill and its z3 tile's assembly (its z6–8 tiles), nothing else.
        assert_eq!(changed, [&format!("3/{}/{}", f.0 >> 3, f.1 >> 3), &format!("6/{}/{}", f.0, f.1)]);
    }
}
