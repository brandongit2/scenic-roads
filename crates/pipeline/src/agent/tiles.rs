//! The terrain packs' indexes, for keys that name the terrain tiles a job reads by their contents
//! (docs/steps.md §12: a z6 tile's, `build::unit_terrain`): a tile's XXH3 comes from its pack's
//! index (store::pack), never from the tile. Each index is read once by its pack's content name (two
//! range reads: `PackIndex::read_from`) and kept in the agent's folder, `pack-idx/<hash16>.idx`
//! (`PackIndex::to_bytes`): a content-named pack never changes, so a kept index is never stale. Not
//! in `cache/`, which the caches' trims and clears empty: the build's ~530 terrain indexes are ~45 MB,
//! and half a minute's reading over SMB. A kept index whose pack the manifest no longer names goes
//! once it's been untouched for `KEEP_GONE`.

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
/// agent's loop, the regions' states and steps, plan::Planner::start_as_built: the indexes and the
/// memo shared by the copies, not copied; the build's 5,165 packs' indexes were 260 MB on 4 Nov,
/// task #930.)
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
    /// off the loop (Agent::start_as_built) and the loop fill and read one.
    memo: std::sync::Arc<std::sync::Mutex<HashMap<String, (String, Kept)>>>,
    /// The terrain's water digests the manifest names (crate::terrain_water::WaterIdx), by content
    /// name: read as the indexes are (kept beside them as `<hash16>.water`), a few KB each.
    water: HashMap<String, crate::terrain_water::WaterIdx>,
    /// The declared elevation datasets the z6 tiles' keys name once the build reads them
    /// (crate::inputs::elevation): the accepted index's content name, and each dataset with where
    /// it has data.
    elevation: Option<(String, std::sync::Arc<Elevation>)>,
    /// The accepted sources' index whose geoid sources converted the held datasets' heights
    /// (crate::dem::geoid; None: no sources accepted, nothing converted).
    elevation_geoid: Option<String>,
    /// The declared sources as the z6 tiles' and tree cover's keys name them (crate::inputs::sources):
    /// the accepted sources' index content name, and what they read of it (None: not declared, or
    /// not to be read now).
    buildings: Option<(String, Option<KeyedSources>)>,
    /// The declared depth sources (crate::depth::grids): the accepted sources' index content name,
    /// and them (None: they can't be read now; the depth's work waits).
    depth: Option<(String, Option<std::sync::Arc<Vec<crate::depth::grids::Source>>>)>,
    /// The elevation datasets finer than the worldwide ones (crate::terrain_deep), their heights'
    /// conversions as the roads': the accepted elevation and sources indexes' content names, and
    /// the sets (None: they can't be read now; the terrain's z6 tiles wait: their keys name them).
    fine: Option<(String, Option<std::sync::Arc<Vec<crate::terrain_deep::FineSet>>>)>,
}

/// What the z6 tiles' and tree cover's keys name of the declared sources: the buildings source's
/// release (the roadside buildings' index), and the land-cover, canopy and leaf-type declarations
/// where they aren't as staged (crate::inputs::sources::unit_extra, trees_extra).
#[derive(Clone, Debug, Default, PartialEq)]
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
pub type Elevation = Vec<(crate::dem::declared::Accepted, crate::dem::declared::Cover)>;

impl TerrainTiles {
    /// None held yet; `dir`: where indexes are kept (the agent's `pack-idx/`).
    pub fn new(dir: Option<PathBuf>) -> TerrainTiles {
        TerrainTiles { dir, ..Default::default() }
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
        self.load_elevation(root, m);
        self.load_buildings(root, m);
        self.load_depth(root, m);
        self.load_fine(root, m);
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
        use crate::inputs::elevation;
        let want = elevation::switched(m).then(|| m.get(&crate::inputs::logical(elevation::UNIT))).flatten();
        let Some(want) = want else {
            self.elevation = None;
            return;
        };
        // (Their heights' conversions by the accepted geoid sources: crate::dem::geoid.)
        let sources = m.get(&crate::inputs::logical(crate::inputs::sources::UNIT));
        if self.elevation.as_ref().is_some_and(|(c, _)| c == want) && self.elevation_geoid.as_ref() == sources {
            return;
        }
        let got = crate::inputs::open(root, m, elevation::UNIT).and_then(|a| {
            let mut ds = crate::dem::declared::accepted(&a.ok_or_else(|| anyhow::anyhow!("no accepted version"))?)?;
            if sources.is_some() {
                let s = crate::inputs::sources::read(root, m)?;
                for w in crate::dem::declared::convert(&mut ds, s.many(crate::inputs::sources::GEOID)) {
                    eprintln!("agent: elevation: {w}");
                }
            }
            // (Each with its coverage as sampled, its regions' outlines read: the keys name what's
            // inside a box of them, `cut_note`, never their files, task #832.)
            let read = |n: &str| -> anyhow::Result<Vec<u8>> { Ok(root.join(n).read()?) };
            ds.into_iter()
                .map(|d| Ok((crate::dem::declared::cover_read(&d, &read)?, d)))
                .collect::<anyhow::Result<Vec<_>>>()
        });
        self.elevation_geoid = sources.cloned();
        match got {
            Ok(ds) => {
                self.unread.remove(want);
                let e: Elevation = ds.into_iter().map(|(c, d)| (d, c)).collect();
                self.elevation = Some((want.clone(), std::sync::Arc::new(e)));
            }
            Err(e) => {
                self.elevation = None;
                self.unread.insert(want.clone(), format!("{e:#}"));
            }
        }
    }

    /// Holds the declared buildings source's release of the accepted sources `m` names.
    fn load_buildings(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(want) = m.get(&crate::inputs::logical(crate::inputs::sources::UNIT)) else {
            self.buildings = None;
            return;
        };
        if self.buildings.as_ref().is_some_and(|(c, r)| c == want && r.is_some()) {
            return;
        }
        let r = (|| -> anyhow::Result<KeyedSources> {
            let s = crate::inputs::sources::read(root, m)?;
            Ok(KeyedSources {
                release: crate::buildtiles::release_of(root, m)?,
                unit: crate::inputs::sources::unit_extra(&s)?,
                trees: crate::inputs::sources::trees_extra(&s)?,
            })
        })()
        .map_err(|e| {
            eprintln!("agent: the units and tree cover wait: the declared sources in {want} can't be read ({e:#})")
        })
        .ok();
        self.buildings = Some((want.clone(), r));
    }

    fn load_depth(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(want) = m.get(&crate::inputs::logical(crate::inputs::sources::UNIT)) else {
            self.depth = None;
            return;
        };
        if self.depth.as_ref().is_some_and(|(c, r)| c == want && r.is_some()) {
            return;
        }
        let r = crate::depth::grids::declared(root, m)
            .map_err(|e| {
                eprintln!("agent: the depth waits: the declared depth sources in {want} can't be read ({e:#})")
            })
            .ok()
            .map(std::sync::Arc::new);
        self.depth = Some((want.clone(), r));
    }

    /// What the fine sets are read from: the accepted elevation and sources indexes `m` names.
    fn fine_from(m: &BTreeMap<String, String>) -> Option<String> {
        let e = m.get(&crate::inputs::logical(crate::inputs::elevation::UNIT))?;
        let s = m.get(&crate::inputs::logical(crate::inputs::sources::UNIT)).map_or("-", String::as_str);
        Some(format!("{e} {s}"))
    }

    fn load_fine(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let Some(want) = Self::fine_from(m) else {
            self.fine = None;
            return;
        };
        if self.fine.as_ref().is_some_and(|(c, r)| *c == want && r.is_some()) {
            return;
        }
        let r = crate::terrain_deep::Fine::read(root, m)
            .map_err(|e| eprintln!("agent: the terrain waits: its fine datasets in {want} can't be read ({e:#})"))
            .ok()
            .map(|f| std::sync::Arc::new(f.map(|f| f.sets).unwrap_or_default()));
        self.fine = Some((want, r));
    }

    /// The elevation datasets finer than the terrain's base, of the accepted versions `m` names:
    /// none accepted, an empty list; None while they can't be read now.
    pub fn fine(&self, m: &BTreeMap<String, String>) -> Option<std::sync::Arc<Vec<crate::terrain_deep::FineSet>>> {
        let Some(want) = Self::fine_from(m) else { return Some(Default::default()) };
        match &self.fine {
            Some((c, Some(f))) if *c == want => Some(f.clone()),
            #[cfg(test)]
            None => Some(Default::default()),
            _ => None,
        }
    }

    /// Holds `f` as the fine sets of the versions `m` names (tests).
    #[cfg(test)]
    pub fn hold_fine(&mut self, m: &BTreeMap<String, String>, f: Vec<crate::terrain_deep::FineSet>) {
        self.fine = Some((Self::fine_from(m).unwrap_or_default(), Some(std::sync::Arc::new(f))));
    }

    /// The declared depth sources of the accepted sources `m` names: none declared, an empty list;
    /// None while they can't be read now (the depth's work waits).
    pub fn depth(&self, m: &BTreeMap<String, String>) -> Option<std::sync::Arc<Vec<crate::depth::grids::Source>>> {
        let Some(want) = m.get(&crate::inputs::logical(crate::inputs::sources::UNIT)) else {
            return Some(Default::default());
        };
        match &self.depth {
            Some((c, Some(d))) if c == want => Some(d.clone()),
            #[cfg(test)]
            None => Some(Default::default()),
            _ => None,
        }
    }

    /// Holds `d` as the depth sources of accepted sources index `content` (tests).
    #[cfg(test)]
    pub fn hold_depth(&mut self, content: &str, d: Vec<crate::depth::grids::Source>) {
        self.depth = Some((content.to_string(), Some(std::sync::Arc::new(d))));
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
                release: crate::buildtiles::TEST_RELEASE.into(),
                unit: None,
                trees: None,
            }));
        }
        let want = m.get(&crate::inputs::logical(crate::inputs::sources::UNIT))?;
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
    /// north): the held datasets meeting it (crate::dem::declared::meeting), as the roads' keys
    /// name them; None while they can't be told (the terrain's keys wait).
    pub fn terrain_datasets(&self, m: &BTreeMap<String, String>, b: [f64; 4]) -> Option<String> {
        // (Tests holding none: `TEST_SOURCES`.)
        #[cfg(test)]
        if self.elevation.is_none() {
            return Some(TEST_SOURCES.into());
        }
        let e = self.elevation(m).ok()??;
        Some(format!("elevation {}", crate::dem::declared::meeting(&e, b).join(",")))
    }

    /// The declared elevation datasets the z6 tiles' keys name: None before the build reads them; an
    /// error while their accepted version can't be read.
    pub fn elevation(&self, m: &BTreeMap<String, String>) -> Result<Option<std::sync::Arc<Elevation>>, Unread> {
        use crate::inputs::elevation;
        if !elevation::switched(m) {
            return Ok(None);
        }
        let want = m.get(&crate::inputs::logical(elevation::UNIT)).ok_or_else(|| {
            Unread("the build reads the declared elevation datasets, but the records name no accepted version".into())
        })?;
        match &self.elevation {
            Some((c, e)) if c == want => Ok(Some(e.clone())),
            _ => Err(Unread(format!(
                "the elevation datasets' index {want} isn't read{}",
                self.unread.get(want).map(|w| format!(": {w}")).unwrap_or_default()
            ))),
        }
    }

    /// Holds the water digests `m` names, and only those (one that can't be read now: tried again
    /// with the next load, its targets' keys unknown meanwhile).
    fn load_water(&mut self, root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) {
        let want: BTreeSet<&str> = m
            .range(crate::terrain_water::IDX_PREFIX.to_string()..)
            .take_while(|(l, _)| l.starts_with(crate::terrain_water::IDX_PREFIX))
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
                .and_then(|b| crate::terrain_water::WaterIdx::from_bytes(&b).ok());
            let got = match local {
                Some(w) => Ok(w),
                None => root.join(c).read().map_err(anyhow::Error::from).and_then(|b| {
                    let w = crate::terrain_water::WaterIdx::from_bytes(&b)?;
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
    pub fn water_idx(&self, m: &BTreeMap<String, String>, pin: &str) -> Option<&crate::terrain_water::WaterIdx> {
        self.water.get(m.get(&crate::terrain_water::idx_logical(pin))?).filter(|w| w.pin == pin)
    }

    /// Holds a water source's digests as given, under content name `content` (tests: no file).
    /// Holds the declared elevation datasets of accepted index `content` (as `load` reads them).
    pub fn hold_elevation(&mut self, content: &str, e: Elevation) {
        self.elevation = Some((content.to_string(), std::sync::Arc::new(e)));
    }

    pub fn hold_water(&mut self, content: &str, w: crate::terrain_water::WaterIdx) {
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

    /// The tile read for (z, x, y) where a missing tile is its nearest ancestor's, up to eight levels
    /// up (slope_pack::Terrain::tile): the tile itself if there is one, else that ancestor.
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
    /// (crate::layers::pack_of), deep past z12 by z10 tile (crate::terrain_deep).
    fn pack(&mut self, z: u8, x: u32, y: u32) -> Found<'a> {
        let d = crate::terrain_deep::PACK_Z;
        let (scope, pz, px, py) = match crate::layers::pack_of(z, x, y) {
            _ if z >= crate::terrain_deep::FIRST_Z => ("deep", d, x >> (z - d), y >> (z - d)),
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

    /// The tiles of z10 tile `t`'s deep pack (crate::terrain_deep), from its index.
    pub fn deep_in(&mut self, t: (u32, u32)) -> Result<Vec<(u8, u32, u32)>, Unread> {
        let d = crate::terrain_deep::FIRST_Z;
        let k = d - crate::terrain_deep::PACK_Z;
        Ok(match self.pack(d, t.0 << k, t.1 << k)? {
            Some(ts) => ts.iter().map(|&(key, _)| store::pack::tile_zxy(key)).collect(),
            None => Vec::new(),
        })
    }

    /// `TerrainTiles::resolve`.
    pub fn resolve(&mut self, z: u8, x: u32, y: u32) -> Result<Option<Tile>, Unread> {
        for dz in 0..=z.min(8) {
            let (pz, px, py) = (z - dz, x >> dz, y >> dz);
            if let Some(h) = self.hash(pz, px, py)? {
                return Ok(Some((pz, px, py, h)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use store::pack::PackWriter;

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
        // Another layer's pack: not the terrain's.
        m.insert("layers/slope/hi/6-28-16".into(), "layers/slope/hi/6-28-16.3333333333333333.pack".into());
        let mut t = TerrainTiles::new(Some(dir.clone()));
        assert_eq!(t.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 2);
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
        // Kept: another agent (the NAS's packs gone) reads them from here.
        std::fs::remove_dir_all(&root).unwrap();
        let mut again = TerrainTiles::new(Some(dir.clone()));
        assert_eq!(again.load(&store::nasfs::NasRoot::local_for_test(&root), &m), 2);
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
        // A water digest that can't be read: its reason stays said after the load.
        let water = format!("{}0-0-0", crate::terrain_water::IDX_PREFIX);
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
        for other in ["", "abc", "6/1/1 00000000000000FF", "06/1/1 00000000000000ff", "6/1/1 ff", "1/2/0 00000000000000ff", "6/1/1 00000000000000ff\n"] {
            let kept = Kept::of(other);
            assert!(matches!(kept, Kept::Text(_)), "{other:?}");
            assert_eq!(kept.text(), other);
        }
        let t = TerrainTiles::new(None);
        assert_eq!(t.memo("unit deep 6/1/1", "a", || Ok(tiles.to_string())).unwrap(), tiles);
        assert_eq!(t.memo("unit deep 6/1/1", "a", || unreachable!()).unwrap(), tiles, "kept, given back whole");
    }
}
