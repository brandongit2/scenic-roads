//! The terrain packs' indexes, for keys that name the terrain tiles a job reads by their contents
//! (docs/steps.md §12: a z6 tile's, `build::unit_terrain`): a tile's XXH3 comes from its pack's
//! index (store::pack), never from the tile. Each index is read once by its pack's content name (two
//! range reads: `PackIndex::read_from`) and kept in the agent's folder, `pack-idx/<hash16>.idx`
//! (`PackIndex::to_bytes`): a content-named pack never changes, so a kept index is never stale. Not
//! in `cache/`, which the caches' trims and clears empty: the ~5,200 terrain packs the manifest
//! named on 4 Nov have 700 MB of indexes kept, 260 MB held, read over SMB a few at a time. A kept
//! index whose pack the manifest no longer names goes once it's been untouched for `KEEP_GONE`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;
use store::pack::PackIndex;

/// A tile with its content: zoom, x, y, and its XXH3 (its pack's index's, of the bytes stored).
pub type Tile = (u8, u32, u32, u64);

/// A pack whose index couldn't be read (its content name, and why): what it holds can't be told now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unread(pub String);

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How long a kept index whose pack the manifest no longer names stays (as GC keeps a replaced file
/// for a fortnight: its pack may be named again).
const KEEP_GONE: Duration = Duration::from_secs(14 * 86400);

/// A pack's tiles as held: (tile key, XXH3), sorted by key; shared by the copies (`TerrainTiles`).
type Held = std::sync::Arc<[(u64, u64)]>;

/// The tiles of the pack a manifest names for a tile: none when it names none.
type Found<'a> = Result<Option<&'a [(u64, u64)]>, Unread>;

/// The terrain packs' indexes the agent holds, by content name. (Cloned for a count off the
/// agent's loop, the regions' steps, plan::Planner::start_region_steps: the indexes and the
/// memo shared by the copies, not copied: task #930.)
#[derive(Clone, Default)]
pub struct TerrainTiles {
    /// Each pack's tiles.
    held: HashMap<String, Held>,
    /// The packs the last load couldn't read, by content name: why.
    unread: BTreeMap<String, String>,
    /// Where the indexes are kept (None: read into memory only).
    dir: Option<PathBuf>,
    /// Values worked out from the tiles, each kept with what it was worked out from (`memo`): a
    /// z6 tile's terrain digest (build::unit_terrain), by z6 tile. Shared by the copies, so a count
    /// off the loop (plan::Planner::start_region_steps) and the loop fill and read one.
    memo: std::sync::Arc<std::sync::Mutex<HashMap<String, (String, Kept)>>>,
    /// The depth's water digests the manifest names (codecs::terrain_water::WaterIdx), by content
    /// name: read as the indexes are (kept beside them as `<hash16>.water`), a few KB each.
    water: HashMap<String, codecs::terrain_water::WaterIdx>,
    /// The declared elevation datasets the z6 tiles' keys name once the build reads them
    /// (inputs::inputs::elevation): the accepted index's content name, each dataset with where
    /// it has data, and where they are fine (`spacing`: made once from them, not by every key
    /// that names it).
    elevation: Option<(String, std::sync::Arc<Elevation>, std::sync::Arc<inputs::dem::spacing::Fine>)>,
    /// The geoid sources that converted the held datasets' heights (inputs::dem::geoid), as the keys
    /// name them (`Source::named`).
    elevation_geoid: Vec<String>,
    /// The declared geoid sources (inputs::inputs::sources::geoid): the accepted sources' index
    /// content name, and them (None: they can't be read now; the terrain's keys naming them wait).
    geoid: Option<(String, Option<std::sync::Arc<Vec<inputs::inputs::sources::Source>>>)>,
    /// The declared sources as the z6 tiles' and tree cover's keys name them (inputs::inputs::sources):
    /// the accepted sources' index content name, and what they read of it (None: not declared, or
    /// not to be read now).
    buildings: Option<(String, Option<KeyedSources>)>,
    /// The declared depth sources (terrain::depth::grids): the accepted sources' index content name
    /// and the lake-depth packs' digest (`grids::estimates_key`), and them with those packs
    /// (None: they can't be read now; the depth's work waits).
    depth: Option<(String, Option<std::sync::Arc<Vec<terrain::depth::grids::DepthSource>>>)>,
    /// The same without the lake-depth packs (`grids::declared_alone`), by the accepted sources'
    /// index content name: read once, not again as the packs change.
    depth_alone: Option<(String, std::sync::Arc<Vec<terrain::depth::grids::DepthSource>>)>,
    /// The seas closed off from the world ocean the water step wrote (inputs::water::Enclosed), by
    /// content name, which the depth's keys read where an enclosed sea's surface is (None: it can't
    /// be read now; those keys wait).
    enclosed: Option<(String, Option<std::sync::Arc<inputs::water::Enclosed>>)>,
    /// The declared elevation datasets (terrain::terrain_zoom), their heights
    /// conversions as the roads': what they're read from (`sets_from`), and
    /// the sets (None: they can't be read now; the terrain's z6 tiles wait: their keys name them).
    sets: Option<(String, Option<std::sync::Arc<Vec<terrain::terrain_zoom::Set>>>)>,
    /// The terrain-deep targets and their levels' digests worked out so far (`deep`,
    /// terrain::terrain_deep::Kept). Shared by the copies, as `memo` is.
    deep: std::sync::Arc<terrain::terrain_deep::Kept>,
    /// What the steps keyed on what they asked of the coverage last asked (buildstore::asked: the
    /// records `m` names under `work/asked/`), by content name (None: it can't be read now).
    asked: HashMap<String, Option<std::sync::Arc<buildstore::asked::Record>>>,
    /// The root the last load read from: where `outlines` opens the outlines a record's question
    /// names, only when asked (never held open: a coverage of the version a record asked asks
    /// nothing again).
    root: Option<store::nasfs::NasRoot>,
}

/// What the z6 tiles' and tree cover's keys name of the declared sources: the buildings source's
/// release (the roadside buildings' index), and the land-cover, canopy and leaf-type declarations
/// where they aren't as staged (inputs::inputs::sources::unit_extra, trees_extra).
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub struct KeyedSources {
    pub release: String,
    pub unit: Option<String>,
    pub trees: Option<String>,
}

/// What a terrain target's key names of the elevation pool in tests holding no datasets
/// (`terrain_datasets`).
#[cfg(test)]
pub const TEST_SOURCES: &str = "elevation test";

/// The declared elevation datasets as the z6 tiles' keys name them: each with its coverage.
pub type Elevation = Vec<(inputs::dem::declared::Accepted, inputs::dem::declared::Cover)>;

impl TerrainTiles {
    /// None held yet; `dir`: where indexes are kept (the agent's `pack-idx/`).
    pub fn new(dir: Option<PathBuf>) -> TerrainTiles {
        TerrainTiles { dir, ..Default::default() }
    }

    /// A digest of what the keys read of it now: the packs whose indexes it holds and those it
    /// couldn't read, the water digests, and each declaration it read by content name, read or not
    /// (each content-named: the same name, the same contents). A load can change it with the same
    /// manifest (an index or a declaration that couldn't be read now read): build::KeyMemo serves
    /// only the state it was filled with. (Its own memos, `memo` and `deep`, keep what each answer
    /// was worked out from: not in it.)
    pub fn state(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        fn sorted<'a>(k: impl Iterator<Item = &'a String>) -> Vec<&'a String> {
            let mut k: Vec<&String> = k.collect();
            k.sort_unstable();
            k
        }
        sorted(self.held.keys()).hash(&mut h);
        self.unread.hash(&mut h);
        sorted(self.water.keys()).hash(&mut h);
        self.elevation.as_ref().map(|e| &e.0).hash(&mut h);
        self.elevation_geoid.hash(&mut h);
        self.geoid.as_ref().map(|g| (&g.0, g.1.is_some())).hash(&mut h);
        self.buildings.hash(&mut h);
        self.depth.as_ref().map(|d| (&d.0, d.1.is_some())).hash(&mut h);
        self.depth_alone.as_ref().map(|d| &d.0).hash(&mut h);
        self.sets.as_ref().map(|d| (&d.0, d.1.is_some())).hash(&mut h);
        let mut a: Vec<(&String, bool)> = self.asked.iter().map(|(c, r)| (c, r.is_some())).collect();
        a.sort_unstable();
        a.hash(&mut h);
        h.finish()
    }

    /// The terrain packs `m` names: (logical, content name).
    pub fn packs(m: &BTreeMap<String, String>) -> impl Iterator<Item = (&String, &String)> {
        m.range("layers/terrain/".to_string()..).take_while(|(l, _)| l.starts_with("layers/terrain/"))
    }

    /// Holds the index of every terrain pack `m` names, and only those: each read from where it's
    /// kept, else from the NAS (several at a time) and kept; how many were read. One that can't be
    /// read now is tried again with the next load (`unread`).
    pub fn load(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) -> usize {
        use rayon::prelude::*;
        let want: BTreeSet<&str> = Self::packs(m).map(|(_, c)| c.as_str()).collect();
        // (What couldn't be read, dropped before the loads below say it again: kept after them, a
        // water digest's or the elevation index's reason was dropped, its wait saying none.)
        self.unread.retain(|c, _| want.contains(c.as_str()));
        self.load_water(root, m);
        self.load_geoid(root, m);
        self.load_elevation(root, m);
        self.load_buildings(root, m);
        self.load_depth(root, m);
        self.load_enclosed(root, m);
        self.load_sets(root, m);
        self.load_asked(root, m);
        self.held.retain(|c, _| want.contains(c.as_str()));
        let missing: Vec<&str> = want.iter().copied().filter(|c| !self.held.contains_key(*c)).collect();
        if missing.is_empty() {
            return 0;
        }
        let dir = self.dir.as_deref();
        let got: Vec<(&str, Result<Held, String>)> = missing.par_iter().map(|c| (*c, read(dir, root, c))).collect();
        let mut n = 0;
        for (c, r) in got {
            match r {
                Ok(t) => {
                    self.unread.remove(c);
                    self.held.insert(c.to_string(), t);
                    n += 1;
                }
                Err(why) => {
                    self.unread.insert(c.to_string(), why);
                }
            }
        }
        // (New packs: those gone from the manifest a fortnight go from here.)
        if n > 0 {
            self.tidy(&want);
        }
        n
    }

    /// Holds the declared elevation datasets of the accepted version `m` names, once the build reads
    /// them (one that can't be read now: the z6 tiles' keys wait, tried again with the next load).
    fn load_elevation(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        use inputs::inputs::elevation;
        let want = elevation::switched(m).then(|| m.get(&inputs::inputs::logical(elevation::UNIT))).flatten();
        let Some(want) = want else {
            self.elevation = None;
            return;
        };
        // (Their heights' conversions by the accepted geoid sources, inputs::dem::geoid: read again
        // when one of those changes, not for another source's declaration.)
        let Some(geoid) = self.geoid(m) else {
            self.elevation = None;
            self.unread.insert(want.clone(), "the declared geoid sources can't be read now".into());
            return;
        };
        let named: Vec<String> = geoid.iter().map(inputs::inputs::sources::Source::named).collect();
        if self.elevation.as_ref().is_some_and(|e| e.0 == *want) && self.elevation_geoid == named {
            return;
        }
        let got = inputs::inputs::open(root, m, elevation::UNIT).and_then(|a| {
            let mut ds = inputs::dem::declared::accepted(&a.ok_or_else(|| anyhow::anyhow!("no accepted version"))?)?;
            for w in inputs::dem::declared::convert(&mut ds, &geoid) {
                eprintln!("agent: elevation: {w}");
            }
            // (Each with its coverage as sampled, its regions' outlines read: the keys name what's
            // inside a box of them, `cut_note`, never their files, task #832.)
            let read = |n: &str| -> anyhow::Result<Vec<u8>> { Ok(root.join(n).read()?) };
            ds.into_iter()
                .map(|d| Ok((inputs::dem::declared::cover_read(&d, &read)?, d)))
                .collect::<anyhow::Result<Vec<_>>>()
        });
        self.elevation_geoid = named;
        match got {
            Ok(ds) => {
                self.unread.remove(want);
                let e: Elevation = ds.into_iter().map(|(c, d)| (d, c)).collect();
                self.hold_elevation(want, e);
            }
            Err(e) => {
                self.elevation = None;
                self.unread.insert(want.clone(), format!("{e:#}"));
            }
        }
    }

    /// Holds the declared geoid sources of the accepted sources `m` names (inputs::inputs::sources::geoid).
    fn load_geoid(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(want) = m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT)) else {
            self.geoid = None;
            return;
        };
        if self.geoid.as_ref().is_some_and(|(c, g)| c == want && g.is_some()) {
            return;
        }
        let g = inputs::inputs::sources::geoid(root, m)
            .map_err(|e| {
                eprintln!("agent: the terrain waits: the declared geoid sources in {want} can't be read ({e:#})")
            })
            .ok()
            .map(std::sync::Arc::new);
        self.geoid = Some((want.clone(), g));
    }

    /// The declared geoid sources of the accepted sources `m` names (inputs::inputs::sources::geoid):
    /// none accepted, none; None while they can't be read now (the terrain's keys naming them wait).
    pub fn geoid(&self, m: &BTreeMap<String, String>) -> Option<std::sync::Arc<Vec<inputs::inputs::sources::Source>>> {
        let Some(want) = m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT)) else {
            return Some(Default::default());
        };
        match &self.geoid {
            Some((c, Some(g))) if c == want => Some(g.clone()),
            // (Tests holding none: none declared.)
            #[cfg(test)]
            None => Some(Default::default()),
            _ => None,
        }
    }

    /// Holds the declared buildings source's release of the accepted sources `m` names.
    fn load_buildings(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(want) = m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT)) else {
            self.buildings = None;
            return;
        };
        if self.buildings.as_ref().is_some_and(|(c, r)| c == want && r.is_some()) {
            return;
        }
        let r = (|| -> anyhow::Result<KeyedSources> {
            let s = inputs::inputs::sources::read(root, m)?;
            Ok(KeyedSources {
                release: bld::buildtiles::release_of(root, m)?,
                unit: inputs::inputs::sources::unit_extra(&s)?,
                trees: inputs::inputs::sources::trees_extra(&s)?,
            })
        })()
        .map_err(|e| {
            eprintln!("agent: the units and tree cover wait: the declared sources in {want} can't be read ({e:#})")
        })
        .ok();
        self.buildings = Some((want.clone(), r));
    }

    fn load_depth(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(key) = Self::depth_key(m) else {
            self.depth = None;
            return;
        };
        if self.depth.as_ref().is_some_and(|(c, r)| *c == key && r.is_some()) {
            return;
        }
        let want = &m[&inputs::inputs::logical(inputs::inputs::sources::UNIT)];
        if self.depth_alone.as_ref().is_none_or(|(c, _)| c != want) {
            match terrain::depth::grids::declared_alone(root, m) {
                Ok(d) => self.depth_alone = Some((want.clone(), std::sync::Arc::new(d))),
                Err(e) => {
                    eprintln!("agent: the depth waits: the declared depth sources in {want} can't be read ({e:#})");
                    self.depth = Some((key, None));
                    return;
                }
            }
        }
        let alone = self.depth_alone.as_ref().map(|(_, d)| d.as_ref().clone()).unwrap_or_default();
        let r = terrain::depth::grids::with_estimates(alone, root, m);
        self.depth = Some((key, Some(std::sync::Arc::new(r))));
    }

    fn load_enclosed(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(c) = m.get(inputs::water::ENCLOSED) else {
            self.enclosed = None;
            return;
        };
        if self.enclosed.as_ref().is_some_and(|(k, e)| k == c && e.is_some()) {
            return;
        }
        let got =
            root.join(c).read().map_err(anyhow::Error::from).and_then(|b| inputs::water::Enclosed::from_bytes(&b));
        let e = match got {
            Ok(e) => {
                self.unread.remove(c);
                Some(std::sync::Arc::new(e))
            }
            Err(e) => {
                self.unread.insert(c.clone(), format!("{e:#}"));
                None
            }
        };
        self.enclosed = Some((c.clone(), e));
    }

    /// Holds the coverage records `m` names (buildstore::asked::logical), each read once by content
    /// name (one that can't be read now: tried again with the next load).
    fn load_asked(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        self.root = Some(root.clone());
        let want: BTreeSet<&String> = m
            .range("work/asked/".to_string()..)
            .take_while(|(l, _)| l.starts_with("work/asked/"))
            .map(|(_, c)| c)
            .collect();
        self.asked.retain(|c, r| want.contains(c) && r.is_some());
        for c in want {
            if self.asked.contains_key(c) {
                continue;
            }
            let r = root
                .join(c)
                .read()
                .map_err(anyhow::Error::from)
                .and_then(|b| buildstore::asked::Record::from_bytes(&b));
            let r = match r {
                Ok(r) => Some(std::sync::Arc::new(r)),
                Err(e) => {
                    eprintln!("agent: {c} can't be read now ({e:#})");
                    None
                }
            };
            self.asked.insert(c.clone(), r);
        }
    }

    /// The coverage record of content name `c` (buildstore::asked), None while it can't be read now.
    pub fn asked(&self, c: &str) -> Option<std::sync::Arc<buildstore::asked::Record>> {
        self.asked.get(c)?.clone()
    }

    /// The outlines of content name `c` a record's question names, opened now: Some(None) when `m`
    /// no longer names them (their pass gone: its answer is `inputs::rail::GONE`), None while they
    /// can't be read now.
    pub fn outlines(&self, m: &BTreeMap<String, String>, c: &str) -> Option<Option<buildstore::outlines::Outlines>> {
        if !m.values().any(|v| v == c) {
            return Some(None);
        }
        let root = self.root.as_ref()?;
        buildstore::outlines::Outlines::open(&root.join(c))
            .map_err(|e| eprintln!("agent: the outlines {c} can't be read now ({e:#})"))
            .ok()
            .map(Some)
    }

    /// Holds record `r` under content name `content` (tests: no file).
    #[cfg(test)]
    pub fn hold_asked(&mut self, content: &str, r: buildstore::asked::Record) {
        self.asked.insert(content.to_string(), Some(std::sync::Arc::new(r)));
    }

    /// The seas closed off from the world ocean `m` names (Some(None): none written); None while
    /// they can't be read now.
    pub fn enclosed(&self, m: &BTreeMap<String, String>) -> Option<Option<std::sync::Arc<inputs::water::Enclosed>>> {
        let Some(c) = m.get(inputs::water::ENCLOSED) else { return Some(None) };
        match &self.enclosed {
            Some((k, Some(e))) if k == c => Some(Some(e.clone())),
            _ => None,
        }
    }

    /// Holds `e` as the enclosed seas of content name `content` (tests).
    #[cfg(test)]
    pub fn hold_enclosed(&mut self, content: &str, e: inputs::water::Enclosed) {
        self.enclosed = Some((content.to_string(), Some(std::sync::Arc::new(e))));
    }

    /// What the depth sources are held by (`depth`): the accepted sources' index content name and the
    /// lake-depth packs' digest; None with no sources accepted.
    fn depth_key(m: &BTreeMap<String, String>) -> Option<String> {
        let want = m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT))?;
        Some(format!("{want} {}", terrain::depth::grids::estimates_key(m)))
    }

    /// What the declared sets are read from: the accepted elevation index `m` names and the geoid
    /// sources (`geoid`; another source's declaration changed, they're as they were).
    fn sets_from(&self, m: &BTreeMap<String, String>) -> Option<String> {
        let e = m.get(&inputs::inputs::logical(inputs::inputs::elevation::UNIT))?;
        let g = self.geoid(m).map_or_else(
            || "geoid unread".to_string(),
            |g| g.iter().map(inputs::inputs::sources::Source::named).collect::<Vec<_>>().join(", "),
        );
        Some(format!("{e} {g}"))
    }

    fn load_sets(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(want) = self.sets_from(m) else {
            self.sets = None;
            return;
        };
        if self.sets.as_ref().is_some_and(|(c, r)| *c == want && r.is_some()) {
            return;
        }
        let r = terrain::terrain_zoom::Declared::read(root, m)
            .map_err(|e| eprintln!("agent: the terrain waits: its declared datasets in {want} can't be read ({e:#})"))
            .ok()
            .map(|f| std::sync::Arc::new(f.map(|f| f.sets).unwrap_or_default()));
        self.sets = Some((want, r));
    }

    /// The declared elevation datasets (terrain::terrain_zoom: each place's finest zoom), of the
    /// accepted versions `m` names: none accepted, an empty list; None while they can't be read now.
    pub fn zoom_sets(&self, m: &BTreeMap<String, String>) -> Option<std::sync::Arc<Vec<terrain::terrain_zoom::Set>>> {
        let Some(want) = self.sets_from(m) else {
            #[cfg(test)]
            return Some(std::sync::Arc::new(terrain::terrain_zoom::test_world(12)));
            #[cfg(not(test))]
            return Some(Default::default());
        };
        match &self.sets {
            Some((c, Some(f))) if *c == want => Some(f.clone()),
            // (Tests hold none: one dataset holding the world, every place's finest zoom z12.)
            #[cfg(test)]
            None => Some(std::sync::Arc::new(terrain::terrain_zoom::test_world(12))),
            _ => None,
        }
    }

    /// The terrain-deep targets and their levels' digests of coverage `cov` and the declared sets
    /// of the versions `m` names (`zoom_sets`), kept by both: worked out once for every plan and
    /// key that asks while neither changes; None while the sets can't be read now.
    pub fn deep<'a>(
        &self,
        m: &BTreeMap<String, String>,
        cov: &'a buildstore::coverage::Coverage,
    ) -> Option<terrain::terrain_deep::Deep<'a>> {
        Some(self.deep.get(cov, self.zoom_sets(m)?))
    }

    /// Holds `f` as the declared sets of the versions `m` names (tests).
    #[cfg(test)]
    pub fn hold_zoom_sets(&mut self, m: &BTreeMap<String, String>, f: Vec<terrain::terrain_zoom::Set>) {
        self.sets = Some((self.sets_from(m).unwrap_or_default(), Some(std::sync::Arc::new(f))));
    }

    /// The declared depth sources of the accepted sources `m` names: none declared, an empty list;
    /// None while they can't be read now (the depth's work waits).
    pub fn depth(
        &self,
        m: &BTreeMap<String, String>,
    ) -> Option<std::sync::Arc<Vec<terrain::depth::grids::DepthSource>>> {
        let Some(key) = Self::depth_key(m) else { return Some(Default::default()) };
        match &self.depth {
            Some((c, Some(d))) if *c == key => Some(d.clone()),
            #[cfg(test)]
            None => Some(Default::default()),
            _ => None,
        }
    }

    /// Holds `d` as the depth sources of accepted sources index `content`, no lake-depth pack made
    /// (tests).
    #[cfg(test)]
    pub fn hold_depth(&mut self, content: &str, d: Vec<terrain::depth::grids::DepthSource>) {
        let key = format!("{content} {}", terrain::depth::grids::estimates_key(&BTreeMap::new()));
        self.depth = Some((key, Some(std::sync::Arc::new(d))));
    }

    /// Holds `d` as the depth sources `m` names, with the lake-depth packs it names (tests).
    #[cfg(test)]
    pub fn hold_depth_in(&mut self, m: &BTreeMap<String, String>, d: Vec<terrain::depth::grids::DepthSource>) {
        let root = store::nasfs::NasRoot::local_for_test("/r");
        let d = terrain::depth::grids::with_estimates(d, &root, m);
        self.depth = Some((Self::depth_key(m).unwrap_or_default(), Some(std::sync::Arc::new(d))));
    }

    /// The declared buildings source's release, of the accepted sources `m` names; None while
    /// there's none, or it can't be read now (the z6 tiles' keys wait).
    pub fn buildings_release(&self, m: &BTreeMap<String, String>) -> Option<&str> {
        self.keyed(m).map(|k| k.release.as_str())
    }

    /// What the keys name of the declared sources, of the accepted sources `m` names; None while
    /// there's none, or they can't be read now (the z6 tiles' and tree cover's keys wait). Tests holding
    /// none: the staged declarations' (nothing beside the release).
    pub fn keyed(&self, m: &BTreeMap<String, String>) -> Option<&KeyedSources> {
        #[cfg(test)]
        if self.buildings.is_none() {
            static TEST: std::sync::OnceLock<KeyedSources> = std::sync::OnceLock::new();
            return Some(TEST.get_or_init(|| KeyedSources {
                release: bld::buildtiles::TEST_RELEASE.into(),
                unit: None,
                trees: None,
            }));
        }
        let want = m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT))?;
        match &self.buildings {
            Some((c, Some(k))) if c == want => Some(k),
            _ => None,
        }
    }

    /// Holds `k` as the keys' declared sources of accepted sources index `content` (tests).
    #[cfg(test)]
    pub fn hold_keyed(&mut self, content: &str, k: KeyedSources) {
        self.buildings = Some((content.to_string(), Some(k)));
    }

    /// What a terrain target's key names of the elevation pool over box `b` (west, south, east,
    /// north): the held datasets meeting it (inputs::dem::declared::meeting), as the roads' keys
    /// name them; None while they can't be told (the terrain's keys wait).
    pub fn terrain_datasets(&self, m: &BTreeMap<String, String>, b: [f64; 4]) -> Option<String> {
        // (Tests holding none: `TEST_SOURCES`.)
        #[cfg(test)]
        if self.elevation.is_none() {
            return Some(TEST_SOURCES.into());
        }
        let e = self.elevation(m).ok()??;
        Some(format!("elevation {}", inputs::dem::declared::meeting(&e, b).join(",")))
    }

    /// The declared elevation datasets the z6 tiles' keys name: None before the build reads them; an
    /// error while their accepted version can't be read.
    pub fn elevation(&self, m: &BTreeMap<String, String>) -> Result<Option<std::sync::Arc<Elevation>>, Unread> {
        use inputs::inputs::elevation;
        if !elevation::switched(m) {
            return Ok(None);
        }
        let want = m.get(&inputs::inputs::logical(elevation::UNIT)).ok_or_else(|| {
            Unread("the build reads the declared elevation datasets, but the records name no accepted version".into())
        })?;
        match &self.elevation {
            Some((c, e, _)) if c == want => Ok(Some(e.clone())),
            _ => Err(Unread(format!(
                "the elevation datasets' index {want} isn't read{}",
                self.unread.get(want).map(|w| format!(": {w}")).unwrap_or_default()
            ))),
        }
    }

    /// Where the declared elevation datasets `m` names are fine (inputs::dem::spacing): none before
    /// the build reads them or while they can't be read (every way at the coarse spacing; the z6
    /// tiles wait).
    pub fn spacing(&self, m: &BTreeMap<String, String>) -> std::sync::Arc<inputs::dem::spacing::Fine> {
        match (self.elevation(m), &self.elevation) {
            (Ok(Some(_)), Some((_, _, fine))) => fine.clone(),
            _ => Default::default(),
        }
    }

    /// Holds the water digests `m` names, and only those (one that can't be read now: tried again
    /// with the next load, its targets' keys unknown meanwhile).
    fn load_water(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let want: BTreeSet<&str> = m
            .range(codecs::terrain_water::IDX_PREFIX.to_string()..)
            .take_while(|(l, _)| l.starts_with(codecs::terrain_water::IDX_PREFIX))
            .map(|(_, c)| c.as_str())
            .collect();
        self.water.retain(|c, _| want.contains(c.as_str()));
        for c in want {
            if self.water.contains_key(c) {
                continue;
            }
            let kept = store::naming::parse_content_name(c)
                .and_then(|n| Some(self.dir.as_deref()?.join(format!("{}.water", n.hash16))));
            let local = kept
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| codecs::terrain_water::WaterIdx::from_bytes(&b).ok());
            let got = match local {
                Some(w) => Ok(w),
                None => root.join(c).read().map_err(anyhow::Error::from).and_then(|b| {
                    let w = codecs::terrain_water::WaterIdx::from_bytes(&b)?;
                    if let Some(p) = &kept {
                        if let Err(e) = std::fs::create_dir_all(p.parent().unwrap())
                            .map_err(anyhow::Error::from)
                            .and_then(|()| store::whole::write(p, &b))
                        {
                            eprintln!("tiles: {c} not kept: {e:#}");
                        }
                    }
                    Ok(w)
                }),
            };
            match got {
                Ok(w) => {
                    self.unread.remove(c);
                    self.water.insert(c.to_string(), w);
                }
                Err(e) => {
                    self.unread.insert(c.to_string(), format!("{e:#}"));
                }
            }
        }
    }

    /// The water digests `m` names for the water source pinned `pin` (None: none, or not read now).
    pub fn water_idx(&self, m: &BTreeMap<String, String>, pin: &str) -> Option<&codecs::terrain_water::WaterIdx> {
        self.water.get(m.get(&codecs::terrain_water::idx_logical(pin))?).filter(|w| w.pin == pin)
    }

    /// Holds the declared elevation datasets of accepted index `content` (as `load` reads them),
    /// with where they are fine.
    pub fn hold_elevation(&mut self, content: &str, e: Elevation) {
        let fine = inputs::dem::spacing::Fine::of(&e);
        self.elevation = Some((content.to_string(), std::sync::Arc::new(e), std::sync::Arc::new(fine)));
    }

    /// Holds a water source's digests as given, under content name `content` (tests: no file).
    pub fn hold_water(&mut self, content: &str, w: codecs::terrain_water::WaterIdx) {
        self.water.insert(content.to_string(), w);
    }

    /// The packs whose index the last load couldn't read: (content name, why).
    pub fn unread(&self) -> impl Iterator<Item = (&String, &String)> {
        self.unread.iter()
    }

    /// Removes the kept indexes of packs not in `want` untouched for `KEEP_GONE` (and temporary
    /// files a write cut short left, an hour old).
    fn tidy(&self, want: &BTreeSet<&str>) {
        let Some(dir) = &self.dir else { return };
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let named: BTreeSet<&str> =
            want.iter().filter_map(|c| store::naming::parse_content_name(c)).map(|c| c.hash16).collect();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let age = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).unwrap_or_default();
            let gone = match name.strip_suffix(".idx") {
                Some(h) => !named.contains(h) && age > KEEP_GONE,
                None => store::whole::is_tmp(&e.path()) && age > Duration::from_secs(3600),
            };
            if gone {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }

    /// Lookups in the packs `m` names.
    pub fn reader<'a>(&'a self, m: &'a BTreeMap<String, String>) -> Reader<'a> {
        Reader { tiles: self, m, packs: HashMap::new() }
    }

    /// Tile (z, x, y)'s XXH3 as the packs `m` names hold it (None: there's no such tile).
    pub fn hash(&self, m: &BTreeMap<String, String>, z: u8, x: u32, y: u32) -> Result<Option<u64>, Unread> {
        self.reader(m).hash(z, x, y)
    }

    /// The tile read for (z, x, y) where a missing tile is its nearest ancestor's, down to
    /// slope_pack::fallback_floor's zoom (slope_pack::Terrain::tile): the tile itself if there is
    /// one, else that ancestor.
    pub fn resolve(&self, m: &BTreeMap<String, String>, z: u8, x: u32, y: u32) -> Result<Option<Tile>, Unread> {
        self.reader(m).resolve(z, x, y)
    }

    /// `make`'s value for `key`, kept with what it was made from (`from`): made again only when that
    /// changes. A value that can't be made now isn't kept. (Kept packed when it's lines of tiles:
    /// `Kept`.)
    pub fn memo(&self, key: &str, from: &str, make: impl FnOnce() -> Result<String, Unread>) -> Result<String, Unread> {
        if let Some((f, v)) = self.memo.lock().unwrap_or_else(|e| e.into_inner()).get(key) {
            if f == from {
                return Ok(v.text());
            }
        }
        let v = make()?;
        self.memo.lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string(), (from.to_string(), Kept::of(&v)));
        Ok(v)
    }

    /// Holds a pack's tiles as given (tests: no pack to read).
    #[cfg(test)]
    pub fn hold(&mut self, content: &str, tiles: impl IntoIterator<Item = Tile>) {
        let mut t: Vec<(u64, u64)> =
            tiles.into_iter().map(|(z, x, y, h)| (store::pack::tile_key(z, x, y), h)).collect();
        t.sort_unstable();
        self.unread.remove(content);
        self.held.insert(content.to_string(), t.into());
    }
}

/// An index's tiles, as held (sorted: the index's own order).
fn compact(ix: &PackIndex) -> Held {
    ix.entries.iter().map(|e| (e.key, e.hash)).collect()
}

/// A memo's value as kept (`TerrainTiles::memo`). Most of what it keeps are the key lines of the
/// terrain tiles a target reads (build::unit_deep_tiles, build::slope_tile_lines: "z/x/y xxh3", or
/// "-" for a tile none of its packs has), hundreds of thousands a z6 tile's: as text they were
/// 900 MB of the lead's memory on 4 Nov (task #930). Such a value is kept as its tiles, 16 bytes a
/// line (the tile's key, store::pack::tile_key, and its XXH3), and given back as the same text;
/// any other as it is.
enum Kept {
    Text(Box<str>),
    Tiles(Box<[(u64, u64)]>),
}

/// A "-" line's tile key (`Kept::Tiles`): never a tile's, whose zoom (its top six bits) is 29 at
/// most.
const NO_TILE: u64 = u64::MAX;

impl Kept {
    /// `v` as kept: its tiles when every line is one, written again byte for byte (`text`), else
    /// its text.
    fn of(v: &str) -> Kept {
        let mut tiles = Vec::new();
        let mut again = String::new();
        for l in v.split('\n') {
            let Some(t) = parse_line(l) else { return Kept::Text(v.into()) };
            again.clear();
            write_line(&mut again, t);
            if again != l {
                return Kept::Text(v.into());
            }
            tiles.push(t);
        }
        Kept::Tiles(tiles.into())
    }

    /// The value as it was made.
    fn text(&self) -> String {
        match self {
            Kept::Text(s) => s.to_string(),
            Kept::Tiles(t) => {
                let mut s = String::with_capacity(t.len() * 32);
                for (i, &tile) in t.iter().enumerate() {
                    if i > 0 {
                        s.push('\n');
                    }
                    write_line(&mut s, tile);
                }
                s
            }
        }
    }
}

/// A tile's line, "z/x/y xxh3" (the XXH3 as 16 hex digits) or "-", as its tile's key and XXH3
/// (`Kept::Tiles`); None for any other line.
fn parse_line(l: &str) -> Option<(u64, u64)> {
    if l == "-" {
        return Some((NO_TILE, 0));
    }
    let (zxy, h) = l.split_once(' ')?;
    let mut it = zxy.split('/');
    let (z, x, y) = (it.next()?.parse::<u8>().ok()?, it.next()?.parse::<u32>().ok()?, it.next()?.parse::<u32>().ok()?);
    if it.next().is_some() || h.len() != 16 || !store::pack::valid_tile(z, x, y) {
        return None;
    }
    Some((store::pack::tile_key(z, x, y), u64::from_str_radix(h, 16).ok()?))
}

/// A tile's line (`parse_line`) onto `s`.
fn write_line(s: &mut String, (key, h): (u64, u64)) {
    use std::fmt::Write;
    if key == NO_TILE {
        s.push('-');
    } else {
        let (z, x, y) = store::pack::tile_zxy(key);
        let _ = write!(s, "{z}/{x}/{y} {h:016x}");
    }
}

/// Where pack `content`'s index is kept under `dir` (None: none is, or it isn't a content name).
fn kept(dir: Option<&Path>, content: &str) -> Option<PathBuf> {
    let c = store::naming::parse_content_name(content)?;
    Some(dir?.join(format!("{}.idx", c.hash16)))
}

/// Pack `content`'s tiles, from where its index is kept under `dir`, else from the NAS (then kept).
fn read(dir: Option<&Path>, root: &store::nasfs::NasRoot, content: &str) -> Result<Held, String> {
    let kept = kept(dir, content);
    if let Some(p) = &kept {
        match std::fs::read(p) {
            Ok(b) => match PackIndex::from_bytes(&b) {
                Ok(ix) => return Ok(compact(&ix)),
                Err(e) => eprintln!("tiles: {} is damaged ({e:#}); read again from the NAS", p.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("tiles: can't read {} ({e}); read from the NAS", p.display()),
        }
    }
    let f = root.join(content).open_plain().map_err(|e| format!("{content}: {e}"))?;
    let ix = PackIndex::read_from(&f).map_err(|e| format!("{content}: {e:#}"))?;
    if let Some(p) = &kept {
        // (Not kept: read from the NAS again next time, which is only slower.)
        if let Err(e) = std::fs::create_dir_all(p.parent().unwrap())
            .map_err(anyhow::Error::from)
            .and_then(|()| store::whole::write(p, &ix.to_bytes()))
        {
            eprintln!("tiles: {content}'s index not kept: {e:#}");
        }
    }
    Ok(compact(&ix))
}

/// Lookups in the packs a manifest names, each pack looked up once (a z6 tile's key reads thousands of
/// tiles from a few packs).
pub struct Reader<'a> {
    tiles: &'a TerrainTiles,
    m: &'a BTreeMap<String, String>,
    /// By pack (its root tile: zoom 0, 3 or 6, x, y).
    packs: HashMap<(u8, u32, u32), Found<'a>>,
}

impl<'a> Reader<'a> {
    /// The tiles of the pack holding (z, x, y): root z0–2, lo z3–8 by z3 tile, hi z9–12 by z6 tile
    /// (terrain::layers::pack_of), deep past z12 by z10 tile (terrain::terrain_deep).
    fn pack(&mut self, z: u8, x: u32, y: u32) -> Found<'a> {
        let d = terrain::terrain_deep::PACK_Z;
        let (scope, pz, px, py) = match terrain::layers::pack_of(z, x, y) {
            _ if z >= terrain::terrain_deep::FIRST_Z => ("deep", d, x >> (z - d), y >> (z - d)),
            p => p,
        };
        let (tiles, m) = (self.tiles, self.m);
        self.packs
            .entry((pz, px, py))
            .or_insert_with(|| {
                let Some(c) = m.get(&format!("layers/terrain/{scope}/{pz}-{px}-{py}")) else { return Ok(None) };
                match tiles.held.get(c) {
                    Some(t) => Ok(Some(&t[..])),
                    None => Err(Unread(format!(
                        "{c}: {}",
                        tiles.unread.get(c).map_or("its index isn't read yet", String::as_str)
                    ))),
                }
            })
            .clone()
    }

    /// Tile (z, x, y)'s XXH3 (None: there's no such tile).
    pub fn hash(&mut self, z: u8, x: u32, y: u32) -> Result<Option<u64>, Unread> {
        if !store::pack::valid_tile(z, x, y) {
            return Ok(None);
        }
        let Some(t) = self.pack(z, x, y)? else { return Ok(None) };
        let k = store::pack::tile_key(z, x, y);
        Ok(t.binary_search_by_key(&k, |e| e.0).ok().map(|i| t[i].1))
    }

    /// The tiles of z10 tile `t`'s deep pack (terrain::terrain_deep), from its index.
    pub fn deep_in(&mut self, t: (u32, u32)) -> Result<Vec<(u8, u32, u32)>, Unread> {
        let d = terrain::terrain_deep::FIRST_Z;
        let k = d - terrain::terrain_deep::PACK_Z;
        Ok(match self.pack(d, t.0 << k, t.1 << k)? {
            Some(ts) => ts.iter().map(|&(key, _)| store::pack::tile_zxy(key)).collect(),
            None => Vec::new(),
        })
    }

    /// `TerrainTiles::resolve`.
    pub fn resolve(&mut self, z: u8, x: u32, y: u32) -> Result<Option<Tile>, Unread> {
        for dz in 0..=z - terrain::slope_pack::fallback_floor(z) {
            let (pz, px, py) = (z - dz, x >> dz, y >> dz);
            if let Some(h) = self.hash(pz, px, py)? {
                return Ok(Some((pz, px, py, h)));
            }
        }
        Ok(None)
    }
}

// (The plan's own tests: pipeline's alone, not where agentcore compiles the plan in, agentcore/build.rs.)
#[cfg(all(test, not(in_agentcore)))]
mod tests {
    use super::*;
    use serde_json::json;
    use store::pack::PackWriter;

    /// Task #968: what the terrain names of the declared sources (terrain::terrain_resume::inputs, as
    /// the held geoids give it to the terrain-targets memo and the runs read it for their resume
    /// identities; and what the declared sets are held by, `sets_from`) is the geoid sources alone,
    /// each by its own declaration. Another source's declaration changed (on 5 Nov NONNA's `lakes`)
    /// leaves them as they were, so no run under way starts again; a geoid's changed, they change.
    #[test]
    fn the_terrain_keys_name_the_geoid_declarations_alone() {
        use inputs::inputs::sources;
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path());
        let geoid = |url: &str| {
            format!(
                "name = \"EGM2008 geoid undulations\"\ncredit = \"NGA\"\nlicence = \"public domain\"\n\
                 licence_url = \"https://earth-info.nga.mil/\"\nsource = \"https://cdn.proj.org/\"\n\
                 retrieved = \"2026-10-18\"\nterritory = \"world\"\n[data]\nrole = \"geoid\"\nkind = \"data\"\n\
                 format = \"geoid-grid\"\nurl = \"{url}\"\n[data.params]\nfrom = \"EGM2008\"\n\
                 to = \"ellipsoid-WGS84\"\nstore = true\n"
            )
        };
        // (The records with the staged declarations, NE's lakes at `lakes_version`, and the EGM2008
        // grid at `url`: the sources' index, the keys' lines, a run's inputs, the sets' source.)
        let keys = |lakes_version: &str, url: &str| {
            let (_, ne) = sources::STAGED.iter().find(|(p, _)| *p == "ne-lakes.toml").unwrap();
            let ne = String::from_utf8(ne.to_vec()).unwrap().replace("version = \"5.0.0\"", lakes_version);
            let g = geoid(url);
            let mut decls: Vec<(&str, &[u8])> =
                sources::STAGED.iter().copied().filter(|(p, _)| *p != "ne-lakes.toml").collect();
            decls.extend([("ne-lakes.toml", ne.as_bytes()), ("egm2008.toml", g.as_bytes())]);
            let mut m = BTreeMap::new();
            inputs::inputs::elevation::accept_fixture(&root, &mut m);
            sources::accept_files(&root, &mut m, &decls);
            let mut t = TerrainTiles::default();
            t.load(&root, &m);
            (
                m[&inputs::inputs::logical(sources::UNIT)].clone(),
                terrain::terrain_resume::inputs(&m, &t.geoid(&m).unwrap()),
                terrain::terrain_resume::inputs_read(&root, &m).unwrap(),
                t.sets_from(&m).unwrap(),
            )
        };
        let url = "https://cdn.proj.org/us_nga_egm08_25.tif";
        let (index, lines, inputs, sets) = keys("version = \"5.0.0\"", url);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with(&format!("{}=", inputs::inputs::logical(inputs::inputs::elevation::UNIT))));
        assert!(lines[1].starts_with("source egm2008 "), "{lines:?}");
        assert_eq!(inputs, lines.join("\n"));
        // Another declaration changed: a new sources index, the same lines.
        let (index2, lines2, inputs2, sets2) = keys("version = \"5.1.0\"", url);
        assert_ne!(index2, index);
        assert_eq!((lines2, inputs2, sets2), (lines.clone(), inputs.clone(), sets.clone()));
        // The geoid's changed: new ones.
        let (_, lines3, inputs3, sets3) = keys("version = \"5.0.0\"", "https://cdn.proj.org/us_nga_egm08_1.tif");
        assert_eq!(lines3[0], lines[0]);
        assert_ne!(lines3[1], lines[1]);
        assert_ne!(inputs3, inputs);
        assert_ne!(sets3, sets);
    }

    /// Task #1120: declaring the planet (`osm-planet.toml` added to the accepted sources) makes a
    /// new sources index, and every key that reads the declared sources reads them as before it:
    /// the terrain's lines, a run's inputs and the sets' source (the geoid), the z6 tiles' and tree
    /// cover's declared sources (the buildings' release, the land cover, canopy and leaf types),
    /// the depth sources and the planner's input digests. Its description's credit is new: the
    /// catalogue's key alone changes.
    #[test]
    fn declaring_the_planet_changes_no_key_but_the_catalogues() {
        use inputs::inputs::sources;
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path());
        let read = |planet: bool| {
            let decls: Vec<(&str, &[u8])> =
                sources::STAGED.iter().copied().filter(|(p, _)| planet || *p != "osm-planet.toml").collect();
            let mut m = BTreeMap::new();
            inputs::inputs::elevation::accept_fixture(&root, &mut m);
            sources::accept_files(&root, &mut m, &decls);
            let mut t = TerrainTiles::default();
            t.load(&root, &m);
            let geoid = t.geoid(&m).unwrap();
            let held = (
                terrain::terrain_resume::inputs(&m, &geoid),
                terrain::terrain_resume::inputs_read(&root, &m).unwrap(),
                t.sets_from(&m).unwrap(),
                t.keyed(&m).cloned().expect("the declared sources the z6 tiles' keys name"),
                t.depth(&m).map(|d| d.as_ref().clone()),
                crate::agent::theplan::input_digests(&root),
            );
            let mut ins = BTreeMap::new();
            if let Some(c) = inputs::inputs::credits::digest(&inputs::inputs::credits::described(&root, &m).unwrap()) {
                ins.insert("credits".to_string(), c);
            }
            let catalogue = crate::agent::build::catalogue_key(&m, &ins, &[]);
            (m[&inputs::inputs::logical(sources::UNIT)].clone(), held, catalogue)
        };
        let (index, held, catalogue) = read(false);
        let (index2, held2, catalogue2) = read(true);
        assert_ne!(index2, index, "a new sources index");
        assert_eq!(held2, held);
        assert_ne!(catalogue2, catalogue, "the planet's credit: a new catalogue");
    }

    /// Task #965: the terrain-deep targets and their levels' digests as `deep` keeps them are the
    /// plain functions' (terrain::terrain_deep::targets_in, levels_digest of its levels),
    /// worked out once for the copies and every plan while the coverage and the sets stay, again
    /// when the sets change.
    #[test]
    fn deep_answers_are_the_plain_ones_kept_by_coverage_and_sets() {
        let d = tempfile::tempdir().unwrap();
        let cov = buildstore::coverage::Coverage::fixture(
            &[buildstore::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:10.5,45.5,5".into()],
            }],
            None,
            d.path(),
        )
        .unwrap();
        // (An accepted elevation named: the sets held are its, not the tests' world at z12.)
        let m = BTreeMap::from([(inputs::inputs::logical(inputs::inputs::elevation::UNIT), "e".to_string())]);
        let mut tiles = TerrainTiles::default();
        tiles.hold_zoom_sets(&m, terrain::terrain_zoom::test_world(14));
        let sets = tiles.zoom_sets(&m).unwrap();
        let t6 = terrain::terrain_zoom::tile_at(10.5, 45.5, 6);
        let plain = terrain::terrain_deep::targets_in(&sets, &cov, t6);
        assert!(!plain.is_empty());
        let copy = tiles.clone();
        let dp = copy.deep(&m, &cov).unwrap();
        assert_eq!(dp.targets_in(t6).to_vec(), plain);
        for &t in plain.iter().chain([&(0, 0)]) {
            let digest = terrain::terrain_deep::levels_digest(&terrain::terrain_deep::levels(&cov, &sets, t));
            assert_eq!(dp.coverage(t), digest, "{t:?}");
        }
        // (Kept: the same answers' memo for the original and a later plan.)
        assert!(std::sync::Arc::ptr_eq(&tiles.deep(&m, &cov).unwrap().memo(), dp.memo()));
        // New sets: worked out again.
        tiles.hold_zoom_sets(&m, terrain::terrain_zoom::test_world(12));
        let dn = tiles.deep(&m, &cov).unwrap();
        assert!(!std::sync::Arc::ptr_eq(dn.memo(), dp.memo()));
        assert!(dn.targets_in(t6).is_empty(), "z12 everywhere: no deep target");
    }

    /// A terrain pack of `tiles` (each tile's bytes: its z/x/y and `salt`) at its content name under
    /// `root`, as the manifest names it.
    fn pack(
        root: &store::nasfs::NasRoot,
        m: &mut BTreeMap<String, String>,
        logical: &str,
        hash16: &str,
        tiles: &[(u8, u32, u32)],
        salt: &str,
    ) -> BTreeMap<(u8, u32, u32), u64> {
        let content = format!("{logical}.{hash16}.pack");
        let p = root.join(&content);
        p.parent().unwrap().create_dir_all().unwrap();
        let mut w = PackWriter::create(&p.unchecked_path(), json!({"layer": "terrain"}), false).unwrap();
        let mut out = BTreeMap::new();
        for &(z, x, y) in tiles {
            let b = format!("{z}/{x}/{y} {salt}").into_bytes();
            w.add(z, x, y, &b, 0).unwrap();
            out.insert((z, x, y), store::naming::xxh3(&b));
        }
        w.finish().unwrap();
        m.insert(logical.to_string(), content);
        out
    }

    #[test]
    fn tiles_come_from_the_packs_indexes_kept_here() {
        let d = tempfile::tempdir().unwrap();
        let (root, dir) = (d.path().join("nas"), d.path().join("pack-idx"));
        let mut m = BTreeMap::new();
        let hi = pack(
            &store::nasfs::NasRoot::local_for_test(&root),
            &mut m,
            "layers/terrain/hi/6-28-16",
            "1111111111111111",
            &[(9, 224, 128), (12, 1792, 1024)],
            "a",
        );
        let lo = pack(
            &store::nasfs::NasRoot::local_for_test(&root),
            &mut m,
            "layers/terrain/lo/3-3-2",
            "2222222222222222",
            &[(3, 3, 2), (6, 28, 16), (8, 112, 64)],
            "a",
        );
        let deep = pack(
            &store::nasfs::NasRoot::local_for_test(&root),
            &mut m,
            "layers/terrain/deep/10-448-256",
            "4444444444444444",
            &[(13, 3584, 2048)],
            "a",
        );
        // Another layer's pack: not the terrain's.
        m.insert("layers/slope/hi/6-28-16".into(), "layers/slope/hi/6-28-16.3333333333333333.pack".into());
        let mut t = TerrainTiles::new(Some(dir.clone()));
        assert_eq!(t.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 3);
        assert_eq!(t.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 0, "held: nothing read again");
        assert!(dir.join("1111111111111111.idx").exists() && dir.join("2222222222222222.idx").exists());
        assert_eq!(t.hash(&m, 12, 1792, 1024).unwrap(), Some(hi[&(12, 1792, 1024)]));
        assert_eq!(t.hash(&m, 8, 112, 64).unwrap(), Some(lo[&(8, 112, 64)]));
        // A tile its pack hasn't, and one in a pack the manifest doesn't name: none.
        assert_eq!(t.hash(&m, 12, 1792, 1025).unwrap(), None);
        assert_eq!(t.hash(&m, 9, 0, 0).unwrap(), None);
        // A missing tile resolves to its nearest ancestor, across packs, up to eight levels up.
        assert_eq!(t.resolve(&m, 12, 1792, 1024).unwrap(), Some((12, 1792, 1024, hi[&(12, 1792, 1024)])));
        assert_eq!(t.resolve(&m, 12, 1793, 1025).unwrap(), Some((9, 224, 128, hi[&(9, 224, 128)])));
        assert_eq!(t.resolve(&m, 11, 896, 520).unwrap(), Some((6, 28, 16, lo[&(6, 28, 16)])));
        assert_eq!(t.resolve(&m, 7, 56, 32).unwrap(), Some((6, 28, 16, lo[&(6, 28, 16)])));
        assert_eq!(t.resolve(&m, 12, 1800, 1100).unwrap(), None, "its z3 tile is nine levels up");
        // A deep tile's, within the deep levels alone (slope_pack::fallback_floor).
        assert_eq!(t.resolve(&m, 14, 7169, 4097).unwrap(), Some((13, 3584, 2048, deep[&(13, 3584, 2048)])));
        assert_eq!(t.resolve(&m, 13, 3585, 2049).unwrap(), None, "never its z12 tile");
        // Kept: another agent (the NAS's packs gone) reads them from here.
        std::fs::remove_dir_all(&root).unwrap();
        let mut again = TerrainTiles::new(Some(dir.clone()));
        assert_eq!(again.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 3);
        assert_eq!(again.hash(&m, 12, 1792, 1024).unwrap(), Some(hi[&(12, 1792, 1024)]));
    }

    #[test]
    fn a_damaged_kept_index_is_read_again_and_one_unread_is_said() {
        let d = tempfile::tempdir().unwrap();
        let (root, dir) = (d.path().join("nas"), d.path().join("pack-idx"));
        let mut m = BTreeMap::new();
        let hi = pack(
            &store::nasfs::NasRoot::local_for_test(&root),
            &mut m,
            "layers/terrain/hi/6-28-16",
            "1111111111111111",
            &[(12, 1792, 1024)],
            "a",
        );
        TerrainTiles::new(Some(dir.clone())).load(&store::nasfs::NasRoot::local_for_test(&root), &m);
        let kept = dir.join("1111111111111111.idx");
        let good = std::fs::read(&kept).unwrap();
        std::fs::write(&kept, &good[..good.len() - 3]).unwrap();
        let mut t = TerrainTiles::new(Some(dir.clone()));
        assert_eq!(t.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 1);
        assert_eq!(t.hash(&m, 12, 1792, 1024).unwrap(), Some(hi[&(12, 1792, 1024)]));
        assert_eq!(std::fs::read(&kept).unwrap(), good, "kept again, whole");
        // A pack that can't be read (gone from the NAS, nor kept here): its tiles can't be told, a
        // pack the manifest doesn't name has none, and the next load tries it again.
        m.insert("layers/terrain/hi/6-29-16".into(), "layers/terrain/hi/6-29-16.4444444444444444.pack".into());
        assert_eq!(t.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 0);
        assert_eq!(
            t.unread().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
            ["layers/terrain/hi/6-29-16.4444444444444444.pack"]
        );
        let e = t.hash(&m, 12, 1856, 1024).unwrap_err();
        assert!(e.0.starts_with("layers/terrain/hi/6-29-16.4444444444444444.pack: "), "{e}");
        assert_eq!(t.hash(&m, 12, 1920, 1024).unwrap(), None);
        let (unread_state, named) = (t.state(), m.clone());
        let made = pack(
            &store::nasfs::NasRoot::local_for_test(&root),
            &mut m,
            "layers/terrain/hi/6-29-16",
            "4444444444444444",
            &[(12, 1856, 1024)],
            "b",
        );
        assert_eq!(t.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 1);
        assert_eq!(t.unread().count(), 0);
        assert_eq!(t.hash(&m, 12, 1856, 1024).unwrap(), Some(made[&(12, 1856, 1024)]));
        // (The same manifest, read now: another state, the keys' memo not served across it,
        // build::KeyMemo.)
        assert_eq!(m, named);
        assert_ne!(t.state(), unread_state);
        // A water digest that can't be read: its reason stays said after the load.
        let water = format!("{}0-0-0", codecs::terrain_water::IDX_PREFIX);
        m.insert(water.clone(), "work/water-idx/0-0-0.5555555555555555.idx".into());
        t.load(&store::nasfs::NasRoot::local_for_test(&root), &m);
        assert!(t.unread().any(|(c, w)| c == &m[&water] && !w.is_empty()), "{:?}", t.unread().collect::<Vec<_>>());
        m.remove(&water);
        // A pack the manifest no longer names isn't held; its kept index stays (for a fortnight).
        m.remove("layers/terrain/hi/6-29-16");
        t.load(&store::nasfs::NasRoot::local_for_test(&root), &m);
        assert!(!t.held.contains_key("layers/terrain/hi/6-29-16.4444444444444444.pack"));
        assert!(dir.join("4444444444444444.idx").exists());
    }

    #[test]
    fn an_index_folder_none_reads_into_memory_only() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let mut m = BTreeMap::new();
        let hi = pack(
            &store::nasfs::NasRoot::local_for_test(&root),
            &mut m,
            "layers/terrain/hi/6-28-16",
            "1111111111111111",
            &[(12, 1792, 1024)],
            "a",
        );
        let before: Vec<_> = walk(d.path());
        let mut t = TerrainTiles::new(None);
        assert_eq!(t.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 1);
        assert_eq!(t.hash(&m, 12, 1792, 1024).unwrap(), Some(hi[&(12, 1792, 1024)]));
        assert_eq!(walk(d.path()), before, "nothing written");
    }

    fn walk(p: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(p).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                out.extend(walk(&e.path()));
            } else {
                out.push(e.path());
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_value_is_kept_with_what_it_was_made_from() {
        let t = TerrainTiles::new(None);
        let made = std::cell::Cell::new(0);
        let make = |v: &str| {
            made.set(made.get() + 1);
            Ok(v.to_string())
        };
        assert_eq!(t.memo("6/1/1", "a", || make("x")).unwrap(), "x");
        assert_eq!(t.memo("6/1/1", "a", || make("y")).unwrap(), "x");
        assert_eq!(t.memo("6/1/1", "b", || make("y")).unwrap(), "y");
        assert_eq!(made.get(), 2);
        // One that can't be made now isn't kept.
        assert!(t.memo("6/1/2", "a", || Err(Unread("p: gone".into()))).is_err());
        assert_eq!(t.memo("6/1/2", "a", || make("z")).unwrap(), "z");
    }

    /// Lines of tiles are kept as tiles, 16 bytes a line, and given back byte for byte; anything
    /// else, a line that would be written back otherwise among them, as its text (task #930).
    #[test]
    fn tile_lines_are_kept_packed_and_given_back_as_made() {
        let tiles = "13/4100/2900 00000000000000ff\n-\n12/1792/1024 ffffffffffffffff\n0/0/0 0123456789abcdef";
        let kept = Kept::of(tiles);
        assert!(matches!(&kept, Kept::Tiles(t) if t.len() == 4));
        assert_eq!(kept.text(), tiles);
        for other in [
            "",
            "abc",
            "6/1/1 00000000000000FF",
            "06/1/1 00000000000000ff",
            "6/1/1 ff",
            "1/2/0 00000000000000ff",
            "6/1/1 00000000000000ff\n",
        ] {
            let kept = Kept::of(other);
            assert!(matches!(kept, Kept::Text(_)), "{other:?}");
            assert_eq!(kept.text(), other);
        }
        let t = TerrainTiles::new(None);
        assert_eq!(t.memo("unit deep 6/1/1", "a", || Ok(tiles.to_string())).unwrap(), tiles);
        assert_eq!(t.memo("unit deep 6/1/1", "a", || unreachable!()).unwrap(), tiles, "kept, given back whole");
    }
}
