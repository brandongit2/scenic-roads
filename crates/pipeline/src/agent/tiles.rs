//! The terrain packs' indexes, for keys that name the terrain tiles a job reads by their contents
//! (docs/steps.md §12: a z6 tile's, `build::unit_terrain`): a tile's XXH3 comes from its pack's
//! index (store::pack), never from the tile. Each index is read once by its pack's content name (two
//! range reads: `PackIndex::read_from`) and kept in the agent's folder, `pack-idx/<hash16>.idx`
//! (`PackIndex::to_bytes`): a content-named pack never changes, so a kept index is never stale. Not
//! in `cache/`, which the caches' trims and clears empty: the ~5,200 terrain packs the manifest
//! named on 4 Nov have 700 MB of indexes kept, 260 MB held, read over SMB a few at a time. A kept
//! index whose pack the manifest no longer names goes once it's been untouched for `KEEP_GONE`.

use buildstore::reads::{Manifest, Reads};
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

/// A target record's head as kept here (`<hash16>.asked-head`, `TerrainTiles::asked_heads`), with
/// the record format it was read by (buildstore::asked::RECORD_FMT): one of another is read again,
/// so a build reading records otherwise can't take an old head for one it would parse; with the
/// targets of other steps its questions read in (`reads`, idents::question_read: what its target's
/// `Read::Built` reads wait for, task #1556). A head kept without them (None) stands for a step
/// whose waits don't read them (`idents::waits_by_record`), read again only for one whose do: the
/// peaks' 1,937 small records, not every step's (bldtiles' are 3.5 GB). Its cheap test (`version`,
/// `answering`) and `line` are its record's, or a test the record was asked again under and the
/// line that gave (`TerrainTiles::answered`: its own line when it answered the same, else the new
/// one), so a record isn't asked again on every start once the coverage's geometry or answering
/// code moved under it. (A world step's record, held whole, has one too, for that test alone.)
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Head {
    fmt: u32,
    version: String,
    /// buildstore::asked::Record::answering (a head kept before it was carried: its record's, 1).
    #[serde(default = "buildstore::asked::first_answering")]
    answering: u32,
    line: String,
    #[serde(default)]
    reads: Option<Vec<String>>,
}

impl Head {
    fn of(r: &buildstore::asked::Record) -> Head {
        let mut reads: Vec<String> = r.also.iter().filter_map(|(q, _)| super::idents::question_read(q)).collect();
        reads.sort_unstable();
        reads.dedup();
        Head {
            fmt: buildstore::asked::RECORD_FMT,
            version: r.version.clone(),
            answering: r.answering,
            line: r.line.clone(),
            reads: Some(reads),
        }
    }

    /// Whether it serves the record of content name `c` as kept: of this build's record format,
    /// with the reads its step's waits read.
    fn serves(&self, c: &str) -> bool {
        self.fmt == buildstore::asked::RECORD_FMT && (self.reads.is_some() || !super::idents::waits_by_record(c))
    }
}

/// What's kept in the agent's folder by content hash, with the manifest entries naming them: each
/// kept file's extension and the prefix of the logical names whose contents it's of (`tidy`).
const KEPT: [(&str, &str); 3] =
    [("idx", "layers/terrain/"), ("water", codecs::terrain_water::IDX_PREFIX), ("asked-head", "work/asked/")];

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
    /// What the world steps keyed on what they asked of the coverage last asked (buildstore::asked:
    /// the records `m` names, `asked::logical`), by content name (None: it can't be read now;
    /// Some(None): it reads but doesn't parse, buildstore::asked::Prev::Unparsed).
    asked: HashMap<String, Option<Option<std::sync::Arc<buildstore::asked::Record>>>>,
    /// Of what each target keyed so last asked (`asked::target_logical`), by content name: its cheap
    /// test (what it read of the coverage by geometry, the answering code: `Record::version`,
    /// `answering`, or a test it was asked again under, `answered`) and
    /// the line it ran under, all a key reads of it while that test holds; the record itself is read
    /// again only once it doesn't (`target_asked`). Read once each, several at a time, and kept in
    /// this Mac's folder by content hash (`<hash>.asked-head`), so an agent's start reads only those
    /// new since (one that can't be read now: tried again with the next load). None: one that reads
    /// but doesn't parse (buildstore::asked::Prev::Unparsed). The world steps' records, held whole
    /// (`asked`), have theirs here too, for the cheap test.
    asked_heads: HashMap<String, Option<Head>>,
    /// Files kept in the agent's folder since its last `tidy` (water digests, record heads; new
    /// packs' indexes are counted by `load`).
    kept_new: usize,
    /// The root the last load read from: where `outlines` opens the outlines a record's question
    /// names, only when asked (never held open: a coverage of the version a record asked asks
    /// nothing again).
    root: Option<store::nasfs::NasRoot>,
    /// The records asked again off the plan's thread, shared by the copies (`Reask`).
    reask: std::sync::Arc<buildstore::asked::Reask>,
    /// Whether its keys leave a record that must be asked again to a pass off the plan's thread
    /// while one runs (`Reask::running`), waiting meanwhile: the agent's plan's (`defer`) and its
    /// copies (the shadow's, the regions' counts'), never reading a record for a key then; not a
    /// test's, nor the pass's own copy, which asks them.
    defers: bool,
    /// The memory available above this Mac's floor now, as the memory guard last read it (bytes;
    /// None: unread, the guard guarding nothing): what the records the pass asks again may hold in
    /// flight (`Reask::admit`). The pass's copy's alone (`for_reask`): no other reads a record but
    /// a few on the plan's thread, never waiting for memory there.
    room: Option<buildstore::asked::Room>,
    /// A probe's (`for_probe`): every key of a record that must be asked again waits, the record
    /// named here, none read.
    probe: Option<std::sync::Arc<buildstore::asked::Found>>,
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

    /// The agent's plan's: its keys leave the records that must be asked again to a pass off the
    /// plan's thread while one runs (`defers`).
    pub fn defer(&mut self) {
        self.defers = true;
    }

    /// The records asked again off the plan's thread (`Reask`).
    pub fn reask(&self) -> &std::sync::Arc<buildstore::asked::Reask> {
        &self.reask
    }

    /// A copy for that pass: it asks the records itself, each held within `room`, the memo and
    /// `Reask` shared.
    pub fn for_reask(&self, room: buildstore::asked::Room) -> TerrainTiles {
        TerrainTiles { defers: false, room: Some(room), probe: None, ..self.clone() }
    }

    /// A copy that finds the records that must be asked again (into `found`: (step, target), the
    /// world steps' as (step, "")) and reads none: each key of one waits. Its `Reask` its own, the
    /// memo shared.
    pub fn for_probe(&self, found: std::sync::Arc<buildstore::asked::Found>) -> TerrainTiles {
        TerrainTiles { defers: true, room: None, probe: Some(found), reask: Default::default(), ..self.clone() }
    }

    /// Where a probe names what must be asked again (`for_probe`).
    pub fn probe(&self) -> Option<&buildstore::asked::Found> {
        self.probe.as_deref()
    }

    /// Whether its keys leave the records that must be asked again to a pass off the plan's thread
    /// (`defer`): the agent's plan's.
    pub fn defers(&self) -> bool {
        self.defers
    }

    /// Whether a key now leaves a record that must be asked again to the pass (`defers`, one
    /// running), its target waiting.
    pub fn defers_now(&self) -> bool {
        self.defers && (self.probe.is_some() || self.reask.now().running)
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
        self.reask_state().hash(&mut h);
        h.finish()
    }

    /// What of `Reask` the keys read (`state`, `load_state`): whether a pass runs (they wait) and
    /// how many ended (the lines they put in the memo).
    fn reask_state(&self) -> (bool, u64) {
        let s = self.reask.now();
        (self.defers && s.running, s.ended)
    }

    /// A digest of what of it the manifest's entries don't show: what it can't read now (the packs'
    /// indexes and water digests, the declarations, the enclosed seas, the coverage records). What
    /// it holds is by content name, which the keys read in the manifest; a key that met something it
    /// can't read recorded so (`Manifest::met_load`), and is moved when this changes
    /// (buildstore::reads::Diff::load).
    pub fn load_state(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.unread.hash(&mut h);
        self.geoid.as_ref().filter(|(_, g)| g.is_none()).map(|(c, _)| c).hash(&mut h);
        self.buildings.as_ref().filter(|(_, b)| b.is_none()).map(|(c, _)| c).hash(&mut h);
        self.depth.as_ref().filter(|(_, d)| d.is_none()).map(|(c, _)| c).hash(&mut h);
        self.enclosed.as_ref().filter(|(_, e)| e.is_none()).map(|(c, _)| c).hash(&mut h);
        self.sets.as_ref().filter(|(_, s)| s.is_none()).map(|(c, _)| c).hash(&mut h);
        let mut a: Vec<&String> = self.asked.iter().filter(|(_, r)| r.is_none()).map(|(c, _)| c).collect();
        a.sort_unstable();
        a.hash(&mut h);
        // (A key that waited for the pass met it as a load: moved when it ends.)
        self.reask_state().hash(&mut h);
        h.finish()
    }

    /// The terrain packs `m` names: (logical, content name).
    pub fn packs(m: &BTreeMap<String, String>) -> impl Iterator<Item = (&String, &String)> {
        m.under("layers/terrain/")
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
            // (New water digests or heads kept: what's no longer named goes, as below.)
            if self.kept_new > 0 {
                self.tidy(m);
                self.kept_new = 0;
            }
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
        // (New packs, digests or heads: those gone from the manifest a fortnight go from here.)
        if n + self.kept_new > 0 {
            self.tidy(m);
            self.kept_new = 0;
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
    pub fn geoid<'a>(&self, m: impl Manifest<'a>) -> Option<std::sync::Arc<Vec<inputs::inputs::sources::Source>>> {
        let Some(want) = m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT)) else {
            return Some(Default::default());
        };
        match &self.geoid {
            Some((c, Some(g))) if c == want => Some(g.clone()),
            // (Tests holding none: none declared.)
            #[cfg(test)]
            None => Some(Default::default()),
            _ => {
                m.met_load();
                None
            }
        }
    }

    /// Holds `g` as the declared geoid sources of accepted sources index `content` (tests).
    #[cfg(test)]
    pub fn hold_geoid(&mut self, content: &str, g: Vec<inputs::inputs::sources::Source>) {
        self.geoid = Some((content.to_string(), Some(std::sync::Arc::new(g))));
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
        use rayon::prelude::*;
        self.root = Some(root.clone());
        let all = || m.under("work/asked/");
        let named: BTreeSet<&String> = all().map(|(_, c)| c).collect();
        self.asked_heads.retain(|c, _| named.contains(c));
        // (The targets' records: their heads.)
        let missing: Vec<&String> = all()
            .filter(|(l, c)| !buildstore::asked::is_world(l) && !self.asked_heads.contains_key(*c))
            .map(|(_, c)| c)
            .collect();
        // (Its head kept here, of this build's record format (`Head`), else read whole and kept; an
        // I/O error tried again, a record that doesn't parse taken as one, never tried again.)
        let got: Vec<(&String, std::io::Result<Option<Head>>)> = missing
            .par_iter()
            .map(|c| {
                if let Some(h) = self.kept_head(c) {
                    return (*c, Ok(Some(h)));
                }
                let head = buildstore::asked::Record::read(&root.join(c)).map(|r| r.map(|r| Head::of(&r)));
                if let Ok(Some(h)) = &head {
                    self.keep_head(c, h);
                }
                (*c, head)
            })
            .collect();
        self.kept_new += got.iter().filter(|(_, h)| matches!(h, Ok(Some(_)))).count();
        for (c, r) in got {
            match r {
                Ok(h) => {
                    self.unread.remove(c);
                    self.asked_heads.insert(c.clone(), h);
                }
                Err(e) => {
                    self.unread.insert(c.clone(), format!("{e:#}"));
                }
            }
        }
        let want: BTreeSet<&String> = all().filter(|(l, _)| buildstore::asked::is_world(l)).map(|(_, c)| c).collect();
        self.asked.retain(|c, r| want.contains(c) && r.is_some());
        for c in want {
            if self.asked.contains_key(c) {
                continue;
            }
            // (One that can't be read now: tried again with the next load; one that doesn't
            // parse, held as such: its step re-runs once.)
            let r = match buildstore::asked::Record::read(&root.join(c)) {
                Ok(r) => Some(r.map(std::sync::Arc::new)),
                Err(e) => {
                    eprintln!("agent: {c} can't be read now ({e:#})");
                    None
                }
            };
            // (Its head, for the cheap test alone: one kept here once it answered the same under
            // another, else its own.)
            if let Some(Some(r)) = &r {
                let h = self.kept_head(c).unwrap_or_else(|| Head::of(r));
                self.asked_heads.insert(c.clone(), Some(h));
            }
            self.asked.insert(c.clone(), r);
        }
    }

    /// Where the head of the record of content name `c` is kept (None: nowhere, `dir` none).
    fn head_path(&self, c: &str) -> Option<PathBuf> {
        let n = store::naming::parse_content_name(c)?;
        Some(self.dir.as_deref()?.join(format!("{}.asked-head", n.hash16)))
    }

    /// The head kept here of the record of content name `c`, when it serves it (`Head::serves`).
    fn kept_head(&self, c: &str) -> Option<Head> {
        let b = std::fs::read(self.head_path(c)?).ok()?;
        serde_json::from_slice::<Head>(&b).ok().filter(|h| h.serves(c))
    }

    /// Keeps `h` here as the head of the record of content name `c` (a failure said, not fatal: it's
    /// read again from the NAS).
    fn keep_head(&self, c: &str, h: &Head) {
        let Some(p) = self.head_path(c) else { return };
        let b = serde_json::to_vec(h).expect("a head serializes");
        if let Err(e) = std::fs::create_dir_all(p.parent().unwrap())
            .map_err(anyhow::Error::from)
            .and_then(|()| store::whole::write(&p, &b))
        {
            eprintln!("tiles: {c}'s head not kept: {e:#}");
        }
    }

    /// Record `r` of content name `c`, asked again under cheap test `now` by this build's answering
    /// code (buildstore::coverage::ANSWERING), giving key line `line` (its own when it answered the
    /// same, else a new one naming it): its head kept here with that test and line, so the next
    /// start takes the line without asking again, whether its target reran yet or not (this one
    /// finds it in the memo its key asked through). A local file for each record a geometry or
    /// answering change asks again, never the NAS.
    pub fn answered(&self, c: &str, r: &buildstore::asked::Record, now: &str, line: &str) {
        let h = Head {
            version: now.to_string(),
            answering: buildstore::coverage::ANSWERING,
            line: line.to_string(),
            ..Head::of(r)
        };
        self.keep_head(c, &h);
    }

    /// The coverage record of content name `c` (buildstore::asked): Some(None) for one that doesn't
    /// parse, None while it can't be read now.
    pub fn asked(&self, c: &str) -> Option<Option<std::sync::Arc<buildstore::asked::Record>>> {
        self.asked.get(c)?.clone()
    }

    /// The cheap test of the record of content name `c` (`Head`: what it read of the coverage, by
    /// geometry for a target's, and the answering code it was answered by) and the line it ran under
    /// (`asked_heads`): Some(None) for one that doesn't parse, None while it can't be read now.
    pub fn asked_head(&self, c: &str) -> Option<Option<(String, u32, String)>> {
        self.asked_heads.get(c).map(|h| h.as_ref().map(|h| (h.version.clone(), h.answering, h.line.clone())))
    }

    /// The targets of other steps a target's record of content name `c` says its run read in
    /// (`Head`'s `reads`, "<step> <target>"): Some(None) for one that doesn't parse, None while it
    /// can't be read now.
    pub fn asked_reads(&self, c: &str) -> Option<Option<&[String]>> {
        self.asked_heads.get(c).map(|h| h.as_ref().map(|h| h.reads.as_deref().unwrap_or(&[])))
    }

    /// A target's record of content name `c`, read now (one held, `hold_asked`: that): Ok(None) for
    /// one that doesn't parse, an error only while it can't be read.
    pub fn target_asked<T>(
        &self,
        c: &str,
        f: impl FnOnce(Option<&buildstore::asked::Record>) -> T,
    ) -> Result<T, Unread> {
        if let Some(Some(Some(r))) = self.asked.get(c) {
            return Ok(f(Some(r)));
        }
        let root = self.root.as_ref().ok_or_else(|| Unread(format!("{c}: no records read yet")))?;
        // (By the content name its key read in the manifest: store::keyread's recorded read.)
        let _r = store::keyread::recorded();
        let p = root.join(c);
        // (Held, by the pass's copy alone, while it's read, parsed and asked again: `f`.)
        let _a = match &self.room {
            Some(room) => {
                let cost = buildstore::asked::record_cost(&p).map_err(|e| Unread(format!("{c}: {e:#}")))?;
                Some(self.reask.admit(cost, Some(room), agentcore::agent::memguard::EVERY))
            }
            None => None,
        };
        let r = buildstore::asked::Record::read(&p).map_err(|e| Unread(format!("{c}: {e:#}")))?;
        Ok(f(r.as_ref()))
    }

    /// The outlines of content name `c` a record's question names, opened now: Some(None) when `m`
    /// no longer names them (their pass gone: its answer is `inputs::rail::GONE`), None while they
    /// can't be read now.
    pub fn outlines<'a>(&self, m: impl Manifest<'a>, c: &str) -> Option<Option<buildstore::outlines::Outlines>> {
        if !m.holds(c) {
            return Some(None);
        }
        let root = self.root.as_ref()?;
        // (By the content name its key read in the manifest: store::keyread's recorded read.)
        let _r = store::keyread::recorded();
        buildstore::outlines::Outlines::open(&root.join(c))
            .map_err(|e| eprintln!("agent: the outlines {c} can't be read now ({e:#})"))
            .ok()
            .map(Some)
    }

    /// Holds target record `r` under content name `content`, its head as a load reads it (tests:
    /// no file).
    #[cfg(test)]
    pub fn hold_asked(&mut self, content: &str, r: buildstore::asked::Record) {
        self.asked_heads.insert(content.to_string(), Some(Head::of(&r)));
        self.asked.insert(content.to_string(), Some(Some(std::sync::Arc::new(r))));
    }

    /// The seas closed off from the world ocean `m` names (Some(None): none written); None while
    /// they can't be read now.
    pub fn enclosed<'a>(&self, m: impl Manifest<'a>) -> Option<Option<std::sync::Arc<inputs::water::Enclosed>>> {
        let Some(c) = m.get(inputs::water::ENCLOSED) else { return Some(None) };
        match &self.enclosed {
            Some((k, Some(e))) if k == c => Some(Some(e.clone())),
            _ => {
                m.met_load();
                None
            }
        }
    }

    /// Holds `e` as the enclosed seas of content name `content` (tests).
    #[cfg(test)]
    pub fn hold_enclosed(&mut self, content: &str, e: inputs::water::Enclosed) {
        self.enclosed = Some((content.to_string(), Some(std::sync::Arc::new(e))));
    }

    /// What the depth sources are held by (`depth`): the accepted sources' index content name and the
    /// lake-depth packs' digest; None with no sources accepted.
    fn depth_key<'a>(m: impl Manifest<'a>) -> Option<String> {
        let want = m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT))?;
        Some(format!("{want} {}", terrain::depth::grids::estimates_key(m)))
    }

    /// What the declared sets are read from: the accepted elevation index `m` names and the geoid
    /// sources (`geoid`; another source's declaration changed, they're as they were).
    fn sets_from<'a>(&self, m: impl Manifest<'a>) -> Option<String> {
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
    pub fn zoom_sets<'a>(&self, m: impl Manifest<'a>) -> Option<std::sync::Arc<Vec<terrain::terrain_zoom::Set>>> {
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
            _ => {
                m.met_load();
                None
            }
        }
    }

    /// The terrain-deep targets and their levels' digests of coverage `cov` and the declared sets
    /// of the versions `m` names (`zoom_sets`), kept by both: worked out once for every plan and
    /// key that asks while neither changes; None while the sets can't be read now.
    pub fn deep<'a, 'm>(
        &self,
        m: impl Manifest<'m>,
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
    pub fn depth<'a>(&self, m: impl Manifest<'a>) -> Option<std::sync::Arc<Vec<terrain::depth::grids::DepthSource>>> {
        let Some(key) = Self::depth_key(m) else { return Some(Default::default()) };
        match &self.depth {
            Some((c, Some(d))) if *c == key => Some(d.clone()),
            #[cfg(test)]
            None => Some(Default::default()),
            _ => {
                m.met_load();
                None
            }
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
    pub fn buildings_release<'a>(&self, m: impl Manifest<'a>) -> Option<&str> {
        self.keyed(m).map(|k| k.release.as_str())
    }

    /// What the keys name of the declared sources, of the accepted sources `m` names; None while
    /// there's none, or they can't be read now (the z6 tiles' and tree cover's keys wait). Tests holding
    /// none: the staged declarations' (nothing beside the release).
    pub fn keyed<'a>(&self, m: impl Manifest<'a>) -> Option<&KeyedSources> {
        // (Tests holding none: the staged declarations', held by the index the manifest names, read
        // so as a build reads it.)
        #[cfg(test)]
        if self.buildings.is_none() {
            m.get(&inputs::inputs::logical(inputs::inputs::sources::UNIT));
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
            _ => {
                m.met_load();
                None
            }
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
    pub fn terrain_datasets<'a>(&self, m: impl Manifest<'a>, b: [f64; 4]) -> Option<String> {
        // (Tests holding none: `TEST_SOURCES`, held by the index the manifest names, read so as a
        // build reads it.)
        #[cfg(test)]
        if self.elevation.is_none() {
            m.get(&inputs::inputs::logical(inputs::inputs::elevation::UNIT));
            return Some(TEST_SOURCES.into());
        }
        let e = self.elevation(m).ok()??;
        Some(format!("elevation {}", inputs::dem::declared::meeting(&e, b).join(",")))
    }

    /// The declared elevation datasets the z6 tiles' keys name: None before the build reads them; an
    /// error while their accepted version can't be read.
    pub fn elevation<'a>(&self, m: impl Manifest<'a>) -> Result<Option<std::sync::Arc<Elevation>>, Unread> {
        use inputs::inputs::elevation;
        if !elevation::switched(m) {
            return Ok(None);
        }
        let want = m.get(&inputs::inputs::logical(elevation::UNIT)).ok_or_else(|| {
            Unread("the build reads the declared elevation datasets, but the records name no accepted version".into())
        })?;
        match &self.elevation {
            // (Held by the geoid sources too, which converted their heights (`load_elevation`): read,
            // so a key naming them moves when the accepted sources do.)
            Some((c, e, _)) if c == want => {
                self.geoid(m);
                Ok(Some(e.clone()))
            }
            _ => {
                m.met_load();
                Err(Unread(format!(
                    "the elevation datasets' index {want} isn't read{}",
                    self.unread.get(want).map(|w| format!(": {w}")).unwrap_or_default()
                )))
            }
        }
    }

    /// Where the declared elevation datasets `m` names are fine (inputs::dem::spacing): none before
    /// the build reads them or while they can't be read (every way at the coarse spacing; the z6
    /// tiles wait).
    pub fn spacing<'a>(&self, m: impl Manifest<'a>) -> std::sync::Arc<inputs::dem::spacing::Fine> {
        match (self.elevation(m), &self.elevation) {
            (Ok(Some(_)), Some((_, _, fine))) => fine.clone(),
            _ => Default::default(),
        }
    }

    /// Holds the water digests `m` names, and only those (one that can't be read now: tried again
    /// with the next load, its targets' keys unknown meanwhile).
    fn load_water(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let want: BTreeSet<&str> = m.under(codecs::terrain_water::IDX_PREFIX).map(|(_, c)| c.as_str()).collect();
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
            let from_nas = local.is_none();
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
                    self.kept_new += usize::from(from_nas);
                    self.water.insert(c.to_string(), w);
                }
                Err(e) => {
                    self.unread.insert(c.to_string(), format!("{e:#}"));
                }
            }
        }
    }

    /// The water digests `m` names for the water source pinned `pin` (None: none, or not read now).
    pub fn water_idx<'a>(&self, m: impl Manifest<'a>, pin: &str) -> Option<&codecs::terrain_water::WaterIdx> {
        self.water.get(m.get(&codecs::terrain_water::idx_logical(pin))?).filter(|w| w.pin == pin)
    }

    /// Holds the declared elevation datasets of accepted index `content` (as `load` reads them),
    /// with where they are fine.
    pub fn hold_elevation(&mut self, content: &str, e: Elevation) {
        let fine = inputs::dem::spacing::Fine::of_accepted(e.iter().map(|(d, _)| d));
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

    /// Removes what's kept in the agent's folder by content hash that `m` no longer names (`KEPT`:
    /// the packs' indexes, the water digests, the targets' record heads), once untouched for
    /// `KEEP_GONE`, and temporary files a write cut short left, an hour old.
    fn tidy(&self, m: &BTreeMap<String, String>) {
        let Some(dir) = &self.dir else { return };
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let named: Vec<(&str, BTreeSet<&str>)> = KEPT
            .iter()
            .map(|&(ext, p)| {
                let hs =
                    m.under(&p).filter_map(|(_, c)| store::naming::parse_content_name(c)).map(|c| c.hash16).collect();
                (ext, hs)
            })
            .collect();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let age = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).unwrap_or_default();
            let kept = named.iter().find_map(|(ext, hs)| Some((name.strip_suffix(&format!(".{ext}"))?, hs)));
            let gone = match kept {
                Some((h, hs)) => !hs.contains(h) && age > KEEP_GONE,
                None => store::whole::is_tmp(&e.path()) && age > Duration::from_secs(3600),
            };
            if gone {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }

    /// Lookups in the packs `m` names.
    pub fn reader<'a, 'm: 'a>(&'a self, m: impl Manifest<'m>) -> Reader<'a> {
        Reader { tiles: self, m: m.view(), packs: HashMap::new() }
    }

    /// Tile (z, x, y)'s XXH3 as the packs `m` names hold it (None: there's no such tile).
    pub fn hash<'m>(&self, m: impl Manifest<'m>, z: u8, x: u32, y: u32) -> Result<Option<u64>, Unread> {
        self.reader(m).hash(z, x, y)
    }

    /// The tile read for (z, x, y) where a missing tile is its nearest ancestor's, down to
    /// slope_pack::fallback_floor's zoom (slope_pack::Terrain::tile): the tile itself if there is
    /// one, else that ancestor.
    pub fn resolve<'m>(&self, m: impl Manifest<'m>, z: u8, x: u32, y: u32) -> Result<Option<Tile>, Unread> {
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

    /// `memo`'s value for `key`, when kept from `from`; nothing made or kept (a shadow plan's
    /// read: build::KeyMemo::shadow).
    pub fn memo_found(&self, key: &str, from: &str) -> Option<String> {
        let memo = self.memo.lock().unwrap_or_else(|e| e.into_inner());
        memo.get(key).filter(|(f, _)| f == from).map(|(_, v)| v.text())
    }

    /// Holds a pack's tiles as given (tests: no pack to read).
    #[cfg(test)]
    pub fn hold(&mut self, content: &str, tiles: impl IntoIterator<Item = Tile>) {
        let mut t: Vec<(u64, u64)> =
            tiles.into_iter().map(|(z, x, y, h)| (roadcore::archive::tile_key(z, x, y), h)).collect();
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
/// line (the tile's key, roadcore::archive::tile_key, and its XXH3), and given back as the same text;
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
        // (Room for every line at once, once the first is a tile's: grown line by line, its lists
        // were 1 GB asked of the allocator in the first plan, task #1335.)
        if v.split('\n').next().and_then(parse_line).is_none() {
            return Kept::Text(v.into());
        }
        let mut tiles = Vec::with_capacity(v.bytes().filter(|&b| b == b'\n').count() + 1);
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
    // (A dash form, or any other text, isn't written back the same: `Kept::of` keeps it as text.)
    let u = codecs::tile::TileId::parse(zxy)?;
    if h.len() != 16 {
        return None;
    }
    Some((u.key(), u64::from_str_radix(h, 16).ok()?))
}

/// A tile's line (`parse_line`) onto `s`.
fn write_line(s: &mut String, (key, h): (u64, u64)) {
    use std::fmt::Write;
    if key == NO_TILE {
        s.push('-');
    } else {
        let (z, x, y) = roadcore::archive::tile_zxy(key);
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
    m: Reads<'a>,
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
                    None => {
                        m.met_load();
                        Err(Unread(format!(
                            "{c}: {}",
                            tiles.unread.get(c).map_or("its index isn't read yet", String::as_str)
                        )))
                    }
                }
            })
            .clone()
    }

    /// Tile (z, x, y)'s XXH3 (None: there's no such tile).
    pub fn hash(&mut self, z: u8, x: u32, y: u32) -> Result<Option<u64>, Unread> {
        if !roadcore::archive::valid_tile(z, x, y) {
            return Ok(None);
        }
        let Some(t) = self.pack(z, x, y)? else { return Ok(None) };
        let k = roadcore::archive::tile_key(z, x, y);
        Ok(t.binary_search_by_key(&k, |e| e.0).ok().map(|i| t[i].1))
    }

    /// The tiles of z10 tile `t`'s deep pack (terrain::terrain_deep), from its index.
    pub fn deep_in(&mut self, t: (u32, u32)) -> Result<Vec<(u8, u32, u32)>, Unread> {
        let d = terrain::terrain_deep::FIRST_Z;
        let k = d - terrain::terrain_deep::PACK_Z;
        Ok(match self.pack(d, t.0 << k, t.1 << k)? {
            Some(ts) => ts.iter().map(|&(key, _)| roadcore::archive::tile_zxy(key)).collect(),
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

    /// Task #1473 review: a target's record is read for its head once, then kept here by content
    /// hash (an agent's start reads none again); one that reads but doesn't parse is held as such
    /// (re-keyed once, never tried again); one that can't be read now is unread, tried again.
    #[test]
    fn target_records_heads_are_kept_here_and_one_unparsed_is_held_as_such() {
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        // (Task #1556: with the lo packs its z8 questions read in, each once.)
        let also = ["z8 8/133/90", "z8 8/134/90", "z8 8/160/90"].map(|q| (q.to_string(), "-".to_string())).to_vec();
        let r = buildstore::asked::Record { version: "geo".into(), line: "l".into(), also, ..Default::default() };
        let (good, bad, gone) = (
            "work/asked/peaks/6-1-2.1111111111111111.json.zst",
            "work/asked/pois/6-1-3.2222222222222222.json.zst",
            "work/asked/pois/6-1-4.3333333333333333.json.zst",
        );
        for s in ["peaks", "pois"] {
            std::fs::create_dir_all(d.path().join("nas/work/asked").join(s)).unwrap();
        }
        std::fs::write(d.path().join("nas").join(good), r.to_bytes()).unwrap();
        std::fs::write(d.path().join("nas").join(bad), b"not a record").unwrap();
        let m: BTreeMap<String, String> =
            [good, bad, gone].iter().map(|c| (c.split('.').next().unwrap().to_string(), c.to_string())).collect();
        let mut t = TerrainTiles::new(Some(d.path().join("kept")));
        t.load_asked(&root, &m);
        assert_eq!(t.asked_head(good), Some(Some(("geo".into(), 0, "l".into()))));
        let reads = ["terrain-lo 3/4/2".to_string(), "terrain-lo 3/5/2".to_string()];
        assert_eq!(t.asked_reads(good), Some(Some(&reads[..])));
        assert_eq!(t.asked_head(bad), Some(None));
        assert!(t.target_asked(bad, |r| r.is_none()).unwrap());
        assert_eq!(t.asked_head(gone), None);
        assert!(t.unread.contains_key(gone) && !t.unread.contains_key(bad));
        // Another start: the head from here, the NAS's file gone.
        let nas_good = d.path().join("nas").join(good);
        std::fs::rename(&nas_good, d.path().join("aside")).unwrap();
        let mut again = TerrainTiles::new(Some(d.path().join("kept")));
        again.load_asked(&root, &m);
        assert_eq!(again.asked_head(good), Some(Some(("geo".into(), 0, "l".into()))));
        assert_eq!(again.asked_reads(good), Some(Some(&reads[..])));
        // A head kept by another record format, or a peaks record's without what its questions
        // read (its waits read them): read again from the NAS (here, gone: unread).
        let kept = d.path().join("kept/1111111111111111.asked-head");
        for old in [
            serde_json::json!({"fmt": buildstore::asked::RECORD_FMT - 1, "version": "geo", "line": "old", "reads": []}),
            serde_json::json!({"fmt": buildstore::asked::RECORD_FMT, "version": "geo", "line": "old"}),
        ] {
            std::fs::write(&kept, old.to_string()).unwrap();
            let mut other = TerrainTiles::new(Some(d.path().join("kept")));
            other.load_asked(&root, &m);
            assert_eq!(other.asked_head(good), None, "{old}");
        }
        // Another step's head kept without them stands, its record not read again (here, none on
        // the NAS): its waits don't read them.
        let plain = "work/asked/pois/6-1-5.4444444444444444.json.zst";
        let old = serde_json::json!({"fmt": buildstore::asked::RECORD_FMT, "version": "geo", "line": "old"});
        std::fs::write(d.path().join("kept/4444444444444444.asked-head"), old.to_string()).unwrap();
        let mut with_plain = m.clone();
        with_plain.insert("work/asked/pois/6-1-5".into(), plain.into());
        let mut other = TerrainTiles::new(Some(d.path().join("kept")));
        other.load_asked(&root, &with_plain);
        assert_eq!(other.asked_head(plain), Some(Some(("geo".into(), 1, "old".into()))));
        assert_eq!(other.asked_reads(plain), Some(Some(&[][..])));
        assert!(!other.unread.contains_key(plain));
        let mut other = TerrainTiles::new(Some(d.path().join("kept")));
        std::fs::rename(d.path().join("aside"), &nas_good).unwrap();
        other.load_asked(&root, &m);
        assert_eq!(other.asked_head(good), Some(Some(("geo".into(), 0, "l".into()))));
        assert_eq!(other.asked_reads(good), Some(Some(&reads[..])));
    }

    /// Task #1590: a change to how the coverage works out answers (buildstore::coverage::ANSWERING)
    /// asks every z6 tile's record again though its geometry reads the same. One the old code
    /// answered differently moves its key (it rebuilds); one answered the same keeps it, its head
    /// kept here with this build's code, so the next start takes its line without reading the
    /// record again. The world steps' records alike.
    #[test]
    fn an_answering_change_rebuilds_only_what_it_answers_differently() {
        use crate::agent::build::KeyCtx;
        use buildstore::asked::{logical, target_logical, Record};
        use buildstore::coverage::{fixture_circle, Asking, Coverage, Shape, ANSWERING};
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        let shape = Shape::new("r0: c".into(), vec![fixture_circle(-21.9, 64.13, 20.0)], 1000.0);
        let c = Coverage { shapes: vec![shape], ..Default::default() };
        let inputs = BTreeMap::new();
        // Its runs asked of Reykjavik (in); the old code, as one record has it, answered it out
        // (that record's digest a point's out: what a code change can do to one answer).
        let asking = Asking::new(&c);
        assert!(asking.contains([-219_000_000, 641_300_000]));
        let asked = asking.asked();
        let out = Asking::new(&c);
        assert!(!out.contains([-200_000_000, 641_300_000]));
        let wrong = Record::new(&c, out.asked(), Vec::new(), String::new()).answers;
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut put = |l: String, hash: &str, r: &Record| {
            let content = format!("{l}.{hash}.json.zst");
            let p = d.path().join("nas").join(&content);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, r.to_bytes()).unwrap();
            m.insert(l, content);
        };
        let (same, differs) = ("6/28/17", "6/27/17");
        let geo = |t: &str| crate::agent::idents::clip_geo(&c, codecs::tile::TileId::parse(t).unwrap());
        for (t, hash, answers) in [(same, "1111111111111111", None), (differs, "2222222222222222", Some(&wrong))] {
            let mut r = Record::read_as(&c, geo(t), asked.clone(), Vec::new(), format!("ran {t}"));
            r.answering = 1;
            if let Some(a) = answers {
                r.answers = a.clone();
            }
            put(target_logical("pois", t), hash, &r);
        }
        let mut world = Record::new(&c, asked.clone(), Vec::new(), "ran rail".into());
        world.answering = 1;
        put(logical("rail"), "3333333333333333", &world);
        let world = Record { answers: wrong.clone(), line: "ran bld-fetch".into(), ..world };
        put(logical("bld-fetch"), "4444444444444444", &world);
        let kept = d.path().join("kept");
        let lines = |tiles: &TerrainTiles| {
            let x = KeyCtx::new(&c, "d", &m, &inputs, None, tiles);
            let pois = [same, differs].map(|t| crate::agent::idents::coverage_clip(t, &x).unwrap().remove(0));
            let world = ["rail", "bld-fetch"].map(|s| crate::agent::idents::coverage_asked(s, &x).unwrap().remove(0));
            (pois, world)
        };
        let mut t = TerrainTiles::new(Some(kept.clone()));
        t.load_asked(&root, &m);
        let (pois, world) = lines(&t);
        assert_eq!(pois[0], format!("ran {same}"));
        assert_ne!(pois[1], format!("ran {differs}"));
        assert_eq!(world[0], "ran rail");
        assert_ne!(world[1], "ran bld-fetch");
        // Each asked again: its head kept with this build's code, the test asked and the line it
        // gave, its own or the new one (task #1590 review: one answered differently, not read
        // again on every start until its target reruns).
        let head = |hash: &str| -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(kept.join(format!("{hash}.asked-head"))).unwrap()).unwrap()
        };
        for (hash, now, line) in [
            ("1111111111111111", geo(same), &pois[0]),
            ("2222222222222222", geo(differs), &pois[1]),
            ("3333333333333333", c.version(), &world[0]),
            ("4444444444444444", c.version(), &world[1]),
        ] {
            let h = head(hash);
            assert_eq!(
                (&h["answering"], &h["version"], &h["line"]),
                (&ANSWERING.into(), &now.into(), &line.as_str().into())
            );
        }
        // The next start: no record asked again (the targets' gone from the NAS; the world steps',
        // held whole, asked nothing: no line of theirs in the memo); the same lines.
        for t in [same, differs] {
            std::fs::remove_file(d.path().join("nas").join(&m[&target_logical("pois", t)])).unwrap();
        }
        let mut again = TerrainTiles::new(Some(kept));
        again.load_asked(&root, &m);
        assert!(again.unread.is_empty(), "{:?}", again.unread);
        assert_eq!(lines(&again), (pois, world));
        for s in ["rail", "bld-fetch"] {
            let (key, from) = (format!("coverage:asked {s}"), format!("{} {} []", c.version(), m[&logical(s)]));
            assert!(t.memo_found(&key, &from).is_some(), "{s} asked again the first time");
            assert_eq!(again.memo_found(&key, &from), None, "{s} asked again");
        }
    }

    /// Task #1590 review: the agent's plan leaves the records that must be asked again to a pass
    /// off its thread. A probe names them and reads none; while the pass runs, the key of a record
    /// it hasn't asked yet waits (None), met as a load, and the keys of every other target are
    /// worked out as ever: the plan offers them; once the pass has asked it, the key is its line,
    /// and the tiles' state has moved.
    #[test]
    fn a_record_left_to_the_pass_waits_alone() {
        use crate::agent::build::KeyCtx;
        use buildstore::asked::{target_logical, Found, Record};
        use buildstore::coverage::{fixture_circle, Asking, Coverage, Shape};
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        let shape = Shape::new("r0: c".into(), vec![fixture_circle(-21.9, 64.13, 20.0)], 1000.0);
        let c = Coverage { shapes: vec![shape], ..Default::default() };
        let inputs = BTreeMap::new();
        let asking = Asking::new(&c);
        assert!(asking.contains([-219_000_000, 641_300_000]));
        let asked = asking.asked();
        let (old, fresh, none) = ("6/28/17", "6/27/17", "6/26/17");
        let geo = |t: &str| crate::agent::idents::clip_geo(&c, codecs::tile::TileId::parse(t).unwrap());
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for (t, hash, answering) in [(old, "1111111111111111", 1), (fresh, "2222222222222222", 2)] {
            let mut r = Record::read_as(&c, geo(t), asked.clone(), Vec::new(), format!("ran {t}"));
            r.answering = answering;
            let content = format!("{}.{hash}.json.zst", target_logical("pois", t));
            let p = d.path().join("nas").join(&content);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, r.to_bytes()).unwrap();
            m.insert(target_logical("pois", t), content);
        }
        let mut t = TerrainTiles::new(Some(d.path().join("kept")));
        t.defer();
        t.load_asked(&root, &m);
        macro_rules! x {
            ($t:expr) => {
                KeyCtx::new(&c, "d", &m, &inputs, None, $t)
            };
        }
        let key =
            |tiles: &TerrainTiles, u: &str| crate::agent::idents::coverage_clip(u, &x!(tiles)).map(|mut l| l.remove(0));
        // No pass running: asked on the plan's thread, as a test's plan does.
        assert!(!t.defers_now());
        let (state, load) = (t.state(), t.load_state());
        // A probe: the record named, none read, the other keys as ever; the plan's tiles untouched.
        let found = std::sync::Arc::new(Found::default());
        let probe = t.for_probe(found.clone());
        crate::agent::idents::ask_all(&x!(&probe));
        assert_eq!(key(&probe, old), None);
        assert_eq!(key(&probe, none), Some(geo(none)));
        assert_eq!(found.take(), vec![("pois".to_string(), old.to_string())]);
        assert!(!t.defers_now() && t.state() == state && t.reask().now().done == 0);
        let pass = t.reask().pass(1).expect("none running");
        assert!(t.defers_now() && t.state() != state && t.load_state() != load);
        assert_eq!(key(&t, old), None, "left to the pass");
        assert_eq!(key(&t, fresh).as_deref(), Some(format!("ran {fresh}").as_str()));
        assert_eq!(key(&t, none), Some(geo(none)));
        // The pass, on its copy: it asks the record, admitted within its room (the copy's alone
        // has one), and the plan's key finds its line.
        let p = t.for_reask(std::sync::Arc::new(|| Some(1 << 40)));
        assert!(!p.defers_now());
        crate::agent::idents::warm_all(&x!(&p));
        assert_eq!(t.reask().now().room, Some(1 << 40), "admitted by the room");
        assert_eq!(t.reask().now().done, 1, "the record whose cheap test moved");
        assert!(t.reask().now().took.contains_key("pois"));
        drop(pass);
        assert_eq!(key(&t, old).as_deref(), Some(format!("ran {old}").as_str()));
        assert!(!t.defers_now() && t.state() != state);
        // Nothing new to ask: no pass due.
        assert!(!crate::agent::idents::warm_due(&x!(&t)));
    }

    /// Task #1473 re-review: what's kept here by content hash (packs' indexes, water digests,
    /// record heads) goes once the manifest no longer names it and it's been untouched for
    /// `KEEP_GONE`; what it names, or was touched since, stays.
    #[test]
    fn kept_files_the_manifest_no_longer_names_go_after_a_while() {
        let d = tempfile::tempdir().unwrap();
        let t = TerrainTiles::new(Some(d.path().into()));
        let (named, gone, fresh) = ("1111111111111111", "2222222222222222", "3333333333333333");
        let m: BTreeMap<String, String> = [
            ("layers/terrain/hi/6-1-2", format!("layers/terrain/hi/6-1-2.{named}.pack")),
            (codecs::terrain_water::IDX_PREFIX, format!("{}x.{named}.bin", codecs::terrain_water::IDX_PREFIX)),
            ("work/asked/pois/6-1-2", format!("work/asked/pois/6-1-2.{named}.json.zst")),
        ]
        .into_iter()
        .map(|(l, c)| (l.to_string(), c))
        .collect();
        let old = std::time::SystemTime::now() - KEEP_GONE - Duration::from_secs(60);
        let mut files = Vec::new();
        for ext in ["idx", "water", "asked-head"] {
            for (h, at) in [(named, Some(old)), (gone, Some(old)), (fresh, None)] {
                let f = d.path().join(format!("{h}.{ext}"));
                std::fs::write(&f, b"x").unwrap();
                if let Some(at) = at {
                    std::fs::File::options().write(true).open(&f).unwrap().set_modified(at).unwrap();
                }
                files.push((f, h != gone));
            }
        }
        let other = d.path().join("other.txt");
        std::fs::write(&other, b"x").unwrap();
        t.tidy(&m);
        for (f, stays) in files {
            assert_eq!(f.exists(), stays, "{}", f.display());
        }
        assert!(other.exists());
    }

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
            "6-1-1 00000000000000ff",
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
