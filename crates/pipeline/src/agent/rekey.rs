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
//! reaches of a long way), and z8–z6 tiles of a z6 tile whose hi pack is stale (the coverage left
//! it: its area's runs since make those from the raw tiles alone, which the old key couldn't see) or
//! whose area's terrain is to be made again. A unit without outputs (none of its ways in the
//! coverage) is re-keyed whatever it reads: the terrain doesn't decide which ways it keeps.

use super::build::{self, Keys};
use super::tiles::{TerrainTiles, Tile};
use crate::coverage::Coverage;
use crate::legacy::Unit;
use crate::reach::Reaches;
use std::collections::{BTreeMap, BTreeSet};

/// The key schemes before 2026-10-06.
pub mod v1 {
    use super::super::build::{h, UNIT_V};
    use crate::coverage::Coverage;
    use crate::legacy::Unit;
    use crate::reach::{Reach, Reaches};
    use std::collections::BTreeMap;

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

/// What `rekey` did to the units' records (each by "6/x/y").
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
}

impl Rekeyed {
    /// Whether it changed the records.
    pub fn changed(&self) -> bool {
        !self.moved.is_empty() || !self.left.is_empty()
    }
}

/// Re-keys the units' records in `keys` from the old scheme (`v1::unit_keys`) to the new
/// (`build::unit_keys`), for the coverage `cov`, the pass `date`, the manifest `m`, the reaches,
/// `digests` (agent::input_digests) and the terrain packs' indexes.
pub fn rekey(keys: &mut Keys, cov: &Coverage, date: &str, m: &BTreeMap<String, String>, reach: Option<&Reaches>, digests: &BTreeMap<String, String>, tiles: &TerrainTiles) -> Rekeyed {
    let mut out = Rekeyed::default();
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
    let (terrain, _) = build::terrain_slope_targets(cov, m);
    let terrain_now: BTreeSet<String> = terrain.into_iter().filter(|(q, k)| keys.terrain.get(q) == Some(k)).map(|(q, _)| q).collect();
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
        let why = if outputs { unpinned(u, &read, m, &pieces, &terrain_now) } else { Vec::new() };
        if why.is_empty() {
            keys.unit.insert(t.clone(), build::unit_key(cov, date, m, u, piece, r, digests, &build::terrain_digest(&read)));
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

/// Why the tiles `read` that unit `u`'s new key names aren't all pinned by its old key: a line a
/// kind of tile that isn't, with how many and the first; none when they all are. `pieces`: the z6
/// tiles near the coverage; `terrain_now`: the areas whose terrain is current.
fn unpinned(u: Unit, read: &[Tile], m: &BTreeMap<String, String>, pieces: &BTreeSet<(u32, u32)>, terrain_now: &BTreeSet<String>) -> Vec<String> {
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
                    None
                }
            } else {
                None
            }
        };
        if let Some(k) = kind {
            kinds.entry(k).or_insert((0, (z, x, y))).0 += 1;
        }
    }
    kinds.into_iter().map(|(k, (n, (z, x, y)))| format!("{k}: {n}, {z}/{x}/{y} first")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::build::tests::{e7box, heritage_chain_done, heritage_done, tiles_for, unit_inputs};
    use crate::agent::build::{Rounds, Work};
    use crate::agent::recipes::Recipe;
    use crate::reach::{LongWay, Reach};

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
        loop {
            let w = plan(&c, &reach, &m, &done, &tiles_for(&m)).work;
            match w[0].step.as_str() {
                "heritage-sites" => heritage_done(&mut m, &mut done, "d", &w[0]),
                "terrain" => {
                    done.record("terrain", &w[0].targets);
                    m.insert("layers/terrain/lo/3-3-2".into(), "layers/terrain/lo/3-3-2.1111111111111111.pack".into());
                    for (x, y) in build::coverage_tiles(&c).into_values().flatten() {
                        m.insert(format!("layers/terrain/hi/6-{x}-{y}"), format!("layers/terrain/hi/6-{x}-{y}.2222222222222222.pack"));
                    }
                }
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

    fn plan(c: &Coverage, reach: &Reaches, m: &BTreeMap<String, String>, done: &Keys, tiles: &TerrainTiles) -> build::Plan {
        build::plan(c, "d", m, done, &BTreeMap::new(), Some(reach), tiles, Rounds { each: &c.by_region(), on_map: &BTreeMap::new(), since_last: None, current: None, held: false })
    }

    /// The units the plan builds.
    fn units(p: &build::Plan) -> Vec<String> {
        p.work.iter().filter(|w| w.step == "unit").flat_map(|w: &Work| w.targets.iter().map(|t| t.0.clone())).collect()
    }

    #[test]
    fn units_current_under_the_old_keys_are_re_keyed_once_and_not_built_again() {
        let (c, reach, m, mut done) = iceland(&|_| {});
        let tiles = tiles_for(&m);
        // Under the new keys as recorded: every unit would be built again.
        assert_eq!(units(&plan(&c, &reach, &m, &done, &tiles)).len(), 3);
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles);
        assert_eq!((r.moved.len(), r.left.len(), r.unknown.len(), r.empty.len()), (3, 0, 0, 0), "{r:?}");
        assert!(r.changed());
        assert!(units(&plan(&c, &reach, &m, &done, &tiles)).is_empty());
        // Again: nothing to do, nothing changed.
        let before = done.clone();
        let again = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles);
        assert_eq!(again, Rekeyed::default());
        assert!(!again.changed() && done == before);
        // A late record under the old key (a job an older app ran, merged since): re-keyed.
        let old = v1::unit_keys(&c, "d", &m, Some(&reach), &BTreeMap::new()).into_iter().find(|(u, _)| u.slash() == "6/28/16").unwrap().1;
        done.record("unit", &[("6/28/16".into(), old)]);
        assert_eq!(units(&plan(&c, &reach, &m, &done, &tiles)), ["6/28/16"]);
        assert_eq!(rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles).moved, ["6/28/16"]);
        assert!(units(&plan(&c, &reach, &m, &done, &tiles)).is_empty());
        // One stale under the old keys (its key isn't the old scheme's now): left as it is, built.
        done.record("unit", &[("6/29/16".into(), "0000000000000000".into())]);
        let before = done.clone();
        assert!(!rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles).changed());
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
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles_for(&m));
        assert_eq!(r.left.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), ["6/28/17"]);
        assert!(r.left[0].1.len() == 1 && r.left[0].1[0].starts_with("zoomed-out tiles (z5–z4)"), "{:?}", r.left);
        assert!(!done.unit.contains_key("6/28/17"));
        assert_eq!(r.moved.len(), 2);
        assert_eq!(units(&plan(&c, &reach, &m, &done, &tiles_for(&m))), ["6/28/17"]);
        // Without outputs (none of its ways kept), whatever it reads: re-keyed.
        let (c, reach, mut m, mut done) = iceland(&road);
        m.remove("base/6-28-17");
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles_for(&m));
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
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles);
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
        let r = rekey(&mut stale, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles);
        assert!(r.left.iter().any(|(u, why)| u == "6/28/16" && why[0].starts_with("z8–z6 tiles of an area whose terrain is to be made again")), "{r:?}");
        // Its terrain current: pinned by the hi pack.
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &tiles);
        assert!(r.left.is_empty() && r.moved.len() == 3, "{r:?}");
    }

    #[test]
    fn a_unit_whose_terrain_cant_be_read_now_keeps_its_record() {
        let (c, reach, m, mut done) = iceland(&|_| {});
        let before = done.clone();
        let r = rekey(&mut done, &c, "d", &m, Some(&reach), &BTreeMap::new(), &TerrainTiles::new(None));
        assert_eq!(r.unknown.len(), 3);
        assert!(!r.changed() && done == before);
    }
}
