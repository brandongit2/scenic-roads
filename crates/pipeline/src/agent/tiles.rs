//! The terrain packs' indexes, for keys that name the terrain tiles a job reads by their contents
//! (docs/plan.md §6, Job keys: a unit's, `build::unit_terrain`): a tile's XXH3 comes from its pack's
//! index (store::pack), never from the tile. Each index is read once by its pack's content name (two
//! range reads: `PackIndex::read_from`) and kept in the agent's folder, `pack-idx/<hash16>.idx`
//! (`PackIndex::to_bytes`): a content-named pack never changes, so a kept index is never stale. Not
//! in `cache/`, which the caches' trims and clears empty: the build's ~530 terrain indexes are ~45 MB,
//! and half a minute's reading over SMB. A kept index whose pack the manifest no longer names goes
//! once it's been untouched for `KEEP_GONE`.

use std::cell::RefCell;
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

/// A pack's tiles as held: (tile key, XXH3), sorted by key.
type Held = Vec<(u64, u64)>;

/// The tiles of the pack a manifest names for a tile: none when it names none.
type Found<'a> = Result<Option<&'a [(u64, u64)]>, Unread>;

/// The terrain packs' indexes the agent holds, by content name.
#[derive(Default)]
pub struct TerrainTiles {
    /// Each pack's tiles.
    held: HashMap<String, Held>,
    /// The packs the last load couldn't read, by content name: why.
    unread: BTreeMap<String, String>,
    /// Where the indexes are kept (None: read into memory only, as by `scenic-build rekey-check`,
    /// which writes nothing).
    dir: Option<PathBuf>,
    /// Values worked out from the tiles, each kept with what it was worked out from (`memo`): a
    /// unit's terrain digest (build::unit_terrain), by unit.
    memo: RefCell<HashMap<String, (String, String)>>,
}

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
    pub fn load(&mut self, root: &Path, m: &BTreeMap<String, String>) -> usize {
        use rayon::prelude::*;
        let want: BTreeSet<&str> = Self::packs(m).map(|(_, c)| c.as_str()).collect();
        self.held.retain(|c, _| want.contains(c.as_str()));
        self.unread.retain(|c, _| want.contains(c.as_str()));
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

    /// The packs whose index the last load couldn't read: (content name, why).
    pub fn unread(&self) -> impl Iterator<Item = (&String, &String)> {
        self.unread.iter()
    }

    /// Removes the kept indexes of packs not in `want` untouched for `KEEP_GONE` (and temporary
    /// files a write cut short left, an hour old).
    fn tidy(&self, want: &BTreeSet<&str>) {
        let Some(dir) = &self.dir else { return };
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let named: BTreeSet<&str> = want.iter().filter_map(|c| store::naming::parse_content_name(c)).map(|c| c.hash16).collect();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let age = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).unwrap_or_default();
            let gone = match name.strip_suffix(".idx") {
                Some(h) => !named.contains(h) && age > KEEP_GONE,
                None => crate::whole::is_tmp(&e.path()) && age > Duration::from_secs(3600),
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
    /// changes. A value that can't be made now isn't kept.
    pub fn memo(&self, key: &str, from: &str, make: impl FnOnce() -> Result<String, Unread>) -> Result<String, Unread> {
        if let Some((f, v)) = self.memo.borrow().get(key) {
            if f == from {
                return Ok(v.clone());
            }
        }
        let v = make()?;
        self.memo.borrow_mut().insert(key.to_string(), (from.to_string(), v.clone()));
        Ok(v)
    }

    /// Holds a pack's tiles as given (tests: no pack to read).
    #[cfg(test)]
    pub fn hold(&mut self, content: &str, tiles: impl IntoIterator<Item = Tile>) {
        let mut t: Vec<(u64, u64)> = tiles.into_iter().map(|(z, x, y, h)| (store::pack::tile_key(z, x, y), h)).collect();
        t.sort_unstable();
        self.unread.remove(content);
        self.held.insert(content.to_string(), t);
    }
}

/// An index's tiles, as held (sorted: the index's own order).
fn compact(ix: &PackIndex) -> Held {
    ix.entries.iter().map(|e| (e.key, e.hash)).collect()
}

/// Where pack `content`'s index is kept under `dir` (None: none is, or it isn't a content name).
fn kept(dir: Option<&Path>, content: &str) -> Option<PathBuf> {
    let c = store::naming::parse_content_name(content)?;
    Some(dir?.join(format!("{}.idx", c.hash16)))
}

/// Pack `content`'s tiles, from where its index is kept under `dir`, else from the NAS (then kept).
fn read(dir: Option<&Path>, root: &Path, content: &str) -> Result<Held, String> {
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
    let f = store::range::PlainFile::open(&root.join(content)).map_err(|e| format!("{content}: {e}"))?;
    let ix = PackIndex::read_from(&f).map_err(|e| format!("{content}: {e:#}"))?;
    if let Some(p) = &kept {
        // (Not kept: read from the NAS again next time, which is only slower.)
        if let Err(e) = std::fs::create_dir_all(p.parent().unwrap()).map_err(anyhow::Error::from).and_then(|()| crate::whole::write(p, &ix.to_bytes())) {
            eprintln!("tiles: {content}'s index not kept: {e:#}");
        }
    }
    Ok(compact(&ix))
}

/// Lookups in the packs a manifest names, each pack looked up once (a unit's key reads thousands of
/// tiles from a few packs).
pub struct Reader<'a> {
    tiles: &'a TerrainTiles,
    m: &'a BTreeMap<String, String>,
    /// By pack (its root tile: zoom 0, 3 or 6, x, y).
    packs: HashMap<(u8, u32, u32), Found<'a>>,
}

impl<'a> Reader<'a> {
    /// The tiles of the pack holding (z, x, y): root z0–2, lo z3–8 by z3 tile, hi z9 and finer by
    /// z6 tile (crate::layers::pack_of).
    fn pack(&mut self, z: u8, x: u32, y: u32) -> Found<'a> {
        let (scope, pz, px, py) = crate::layers::pack_of(z, x, y);
        let (tiles, m) = (self.tiles, self.m);
        self.packs
            .entry((pz, px, py))
            .or_insert_with(|| {
                let Some(c) = m.get(&format!("layers/terrain/{scope}/{pz}-{px}-{py}")) else { return Ok(None) };
                match tiles.held.get(c) {
                    Some(t) => Ok(Some(&t[..])),
                    None => Err(Unread(format!("{c}: {}", tiles.unread.get(c).map_or("its index isn't read yet", String::as_str)))),
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
    fn pack(root: &Path, m: &mut BTreeMap<String, String>, logical: &str, hash16: &str, tiles: &[(u8, u32, u32)], salt: &str) -> BTreeMap<(u8, u32, u32), u64> {
        let content = format!("{logical}.{hash16}.pack");
        let p = root.join(&content);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut w = PackWriter::create(&p, json!({"layer": "terrain"}), false).unwrap();
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
        let hi = pack(&root, &mut m, "layers/terrain/hi/6-28-16", "1111111111111111", &[(9, 224, 128), (12, 1792, 1024)], "a");
        let lo = pack(&root, &mut m, "layers/terrain/lo/3-3-2", "2222222222222222", &[(3, 3, 2), (6, 28, 16), (8, 112, 64)], "a");
        // Another layer's pack: not the terrain's.
        m.insert("layers/slope/hi/6-28-16".into(), "layers/slope/hi/6-28-16.3333333333333333.pack".into());
        let mut t = TerrainTiles::new(Some(dir.clone()));
        assert_eq!(t.load(&root, &m), 2);
        assert_eq!(t.load(&root, &m), 0, "held: nothing read again");
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
        assert_eq!(again.load(&root, &m), 2);
        assert_eq!(again.hash(&m, 12, 1792, 1024).unwrap(), Some(hi[&(12, 1792, 1024)]));
    }

    #[test]
    fn a_damaged_kept_index_is_read_again_and_one_unread_is_said() {
        let d = tempfile::tempdir().unwrap();
        let (root, dir) = (d.path().join("nas"), d.path().join("pack-idx"));
        let mut m = BTreeMap::new();
        let hi = pack(&root, &mut m, "layers/terrain/hi/6-28-16", "1111111111111111", &[(12, 1792, 1024)], "a");
        TerrainTiles::new(Some(dir.clone())).load(&root, &m);
        let kept = dir.join("1111111111111111.idx");
        let good = std::fs::read(&kept).unwrap();
        std::fs::write(&kept, &good[..good.len() - 3]).unwrap();
        let mut t = TerrainTiles::new(Some(dir.clone()));
        assert_eq!(t.load(&root, &m), 1);
        assert_eq!(t.hash(&m, 12, 1792, 1024).unwrap(), Some(hi[&(12, 1792, 1024)]));
        assert_eq!(std::fs::read(&kept).unwrap(), good, "kept again, whole");
        // A pack that can't be read (gone from the NAS, nor kept here): its tiles can't be told, a
        // pack the manifest doesn't name has none, and the next load tries it again.
        m.insert("layers/terrain/hi/6-29-16".into(), "layers/terrain/hi/6-29-16.4444444444444444.pack".into());
        assert_eq!(t.load(&root, &m), 0);
        assert_eq!(t.unread().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["layers/terrain/hi/6-29-16.4444444444444444.pack"]);
        let e = t.hash(&m, 12, 1856, 1024).unwrap_err();
        assert!(e.0.starts_with("layers/terrain/hi/6-29-16.4444444444444444.pack: "), "{e}");
        assert_eq!(t.hash(&m, 12, 1920, 1024).unwrap(), None);
        let made = pack(&root, &mut m, "layers/terrain/hi/6-29-16", "4444444444444444", &[(12, 1856, 1024)], "b");
        assert_eq!(t.load(&root, &m), 1);
        assert_eq!(t.unread().count(), 0);
        assert_eq!(t.hash(&m, 12, 1856, 1024).unwrap(), Some(made[&(12, 1856, 1024)]));
        // A pack the manifest no longer names isn't held; its kept index stays (for a fortnight).
        m.remove("layers/terrain/hi/6-29-16");
        t.load(&root, &m);
        assert!(!t.held.contains_key("layers/terrain/hi/6-29-16.4444444444444444.pack"));
        assert!(dir.join("4444444444444444.idx").exists());
    }

    #[test]
    fn rekey_check_reads_into_memory_only() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let mut m = BTreeMap::new();
        let hi = pack(&root, &mut m, "layers/terrain/hi/6-28-16", "1111111111111111", &[(12, 1792, 1024)], "a");
        let before: Vec<_> = walk(d.path());
        let mut t = TerrainTiles::new(None);
        assert_eq!(t.load(&root, &m), 1);
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
}
