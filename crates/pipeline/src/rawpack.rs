//! AWS's raw terrain tiles on the NAS, packed (docs/plan.md §3, Downloads): an archive per z6 area
//! (roadcore::archive's RDTILES1: every tile under that z6 tile, zooms 6 and up, and an empty entry
//! for each one AWS doesn't have), and one more for the tiles above z6 (`low`).
//! `sources/aws-terrarium/packs/index.json` names each area's archive, content-named
//! (`<area>.<hash16>.tiles`), so a copy anywhere is right for good. A tile reaches the NAS in its
//! area's archive, made again with it: one large write, where the NAS takes small files a tile at a
//! time at ~23 a second and stalls doing it; and what it has is read from one file, not found by
//! listing its folders.
//!
//! The build Mac's cache keeps the tiles AWS just gave (`<z>/<x>/<y>.png`, `.none`) until they're
//! packed, and copies of the archives it reads (`packs/`), each copied whole from the NAS once.

use anyhow::{bail, Context, Result};
use roadcore::archive::{tile_key, Archive, Entry, MAGIC};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The archives' index, under the store.
pub const INDEX: &str = "packs/index.json";

/// The area a tile is packed in: its z6 tile's (`6-<x>-<y>`), or `low` above z6.
pub fn area(z: u8, x: u32, y: u32) -> String {
    if z < 6 {
        "low".into()
    } else {
        format!("6-{}-{}", x >> (z - 6), y >> (z - 6))
    }
}

/// Which archive holds each area's tiles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub areas: BTreeMap<String, String>,
}

impl Index {
    /// The store's index (none yet: empty); an error when it can't be read now.
    pub fn load(store: &Path) -> Result<Index> {
        match std::fs::read(store.join(INDEX)) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parse {}", store.join(INDEX).display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", store.join(INDEX).display())),
        }
    }

    pub fn save(&self, store: &Path) -> Result<()> {
        std::fs::create_dir_all(store.join("packs"))?;
        crate::whole::write(&store.join(INDEX), &serde_json::to_vec_pretty(self)?)
    }
}

/// An area's archive here (`dir/packs/`), copied whole from the store the first time; None when the
/// store has none for it.
pub fn local_archive(dir: &Path, store: &Path, index: &Index, area: &str) -> Result<Option<PathBuf>> {
    let Some(name) = index.areas.get(area) else { return Ok(None) };
    let local = dir.join("packs").join(name);
    if !local.exists() {
        std::fs::create_dir_all(local.parent().unwrap())?;
        crate::whole::copy(&store.join("packs").join(name), &local).with_context(|| format!("copy {name} from the NAS"))?;
    }
    Ok(Some(local))
}

/// An area's archive being made: its file, where its next tile goes, and its tiles (key → offset,
/// length; length 0 for one AWS doesn't have).
struct Staged {
    w: BufWriter<std::fs::File>,
    path: PathBuf,
    pos: u64,
    tiles: BTreeMap<u64, (u64, u32)>,
    /// Tiles its archive didn't have.
    new: usize,
    /// The archive it began from (another packer may have made a newer one meanwhile).
    base: Option<String>,
}

/// Makes areas' archives again with new tiles, each with the tiles its archive has already, then
/// puts them on the NAS and in the index.
pub struct Packer {
    /// Where the archives are made (and kept: the local cache's `packs/`).
    dir: PathBuf,
    store: PathBuf,
    index: Index,
    areas: BTreeMap<String, Staged>,
    /// Tiles added that the areas' archives didn't have.
    pub added: usize,
    /// Whether the archives made stay in `dir/packs` (a job's: read again soon) or go once on the
    /// NAS (the store's own tiles packed: tens of GB).
    pub keep: bool,
}

impl Packer {
    /// Packing into the local cache `dir` (its `packs/`) for the NAS's `store`.
    pub fn new(dir: &Path, store: &Path) -> Result<Packer> {
        let index = Index::load(store)?;
        std::fs::create_dir_all(dir.join("packs"))?;
        Ok(Packer { dir: dir.to_path_buf(), store: store.to_path_buf(), index, areas: BTreeMap::new(), added: 0, keep: true })
    }

    /// Adds a tile (None: AWS doesn't have it); whether it's new to its area's archive.
    pub fn add(&mut self, z: u8, x: u32, y: u32, png: Option<&[u8]>) -> Result<bool> {
        let a = area(z, x, y);
        if !self.areas.contains_key(&a) {
            let s = self.start(&a)?;
            self.areas.insert(a.clone(), s);
        }
        let s = self.areas.get_mut(&a).unwrap();
        let key = tile_key(z, x, y);
        if s.tiles.contains_key(&key) {
            return Ok(false);
        }
        let b = png.unwrap_or(&[]);
        s.w.write_all(b)?;
        s.tiles.insert(key, (s.pos, b.len() as u32));
        s.pos += b.len() as u64;
        s.new += 1;
        self.added += 1;
        Ok(true)
    }

    /// An area's archive begun, with the tiles its archive on the NAS has.
    fn start(&self, area: &str) -> Result<Staged> {
        // (Tiles as they come, in a file of their own: the archive's written in its order at the end.)
        let mut s = self.begin(area, self.index.areas.get(area).cloned())?;
        merge(&mut s, &self.dir, &self.store, &self.index, area)?;
        Ok(s)
    }

    /// Finishes the areas with new tiles. Each archive is written whole, its tiles in key order (the
    /// same tiles, the same bytes), named by its content and put on the NAS: harmless until the
    /// index names it, so done without the build's lock. Then, under it (`root`'s), the index is
    /// read again: an area another packer made anew meanwhile is made again with that archive merged
    /// in; the index (written whole) names them, and the archives they replace go. The areas packed.
    pub fn finish(mut self, root: &Path) -> Result<Vec<String>> {
        let mut ready: Vec<(String, String, Option<String>)> = Vec::new();
        for (area, s) in std::mem::take(&mut self.areas) {
            let base = s.base.clone();
            if let Some(name) = self.put_up(&area, s)? {
                ready.push((area, name, base));
            }
        }
        if ready.is_empty() {
            return Ok(Vec::new());
        }
        let lock = crate::out::BuildLock::take(root)?;
        let mut index = Index::load(&self.store)?;
        let (mut done, mut gone, mut made) = (Vec::new(), Vec::new(), Vec::new());
        for (area, mut name, base) in ready {
            if index.areas.get(&area) != base.as_ref() {
                let mut s = self.begin(&area, index.areas.get(&area).cloned())?;
                merge_file(&mut s, &self.dir.join("packs").join(&name))?;
                merge(&mut s, &self.dir, &self.store, &index, &area)?;
                s.new = 1;
                let ours = name;
                match self.put_up(&area, s)? {
                    Some(n) => name = n,
                    None => continue,
                }
                gone.push(ours);
            }
            made.push(name.clone());
            if let Some(old) = index.areas.insert(area.clone(), name) {
                gone.push(old);
            }
            done.push(area);
        }
        index.save(&self.store)?;
        drop(lock);
        // (The archives replaced, here and there: nothing names them now. The ones made stay here
        // only for a job, which reads them again.)
        for old in gone {
            std::fs::remove_file(self.dir.join("packs").join(&old)).ok();
            std::fs::remove_file(self.store.join("packs").join(&old)).ok();
        }
        if !self.keep {
            for n in made {
                std::fs::remove_file(self.dir.join("packs").join(n)).ok();
            }
        }
        Ok(done)
    }

    /// An area's archive begun empty (`base`: the archive in the index it starts from).
    fn begin(&self, area: &str, base: Option<String>) -> Result<Staged> {
        let path = self.dir.join("packs").join(format!("{area}.staged"));
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path)?;
        Ok(Staged { w: BufWriter::with_capacity(256 << 10, f), path, pos: 0, tiles: BTreeMap::new(), new: 0, base })
    }

    /// Writes an area's archive (when it has new tiles) here, named by its content, and puts it on
    /// the NAS; its name.
    fn put_up(&self, area: &str, s: Staged) -> Result<Option<String>> {
        let Staged { w, path: staged, tiles, new, .. } = s;
        let data = w.into_inner().map_err(|e| e.into_error())?;
        if new == 0 {
            std::fs::remove_file(&staged).ok();
            return Ok(None);
        }
        let path = self.dir.join("packs").join(format!("{area}.part"));
        let r = write_archive(&data, &tiles, &path);
        drop(data);
        std::fs::remove_file(&staged).ok();
        r?;
        let name = format!("{area}.{}.tiles", store::naming::hash16_file(&path)?);
        let local = self.dir.join("packs").join(&name);
        std::fs::rename(&path, &local)?;
        let nas = self.store.join("packs").join(&name);
        std::fs::create_dir_all(nas.parent().unwrap())?;
        if std::fs::metadata(&nas).map(|m| m.len()).ok() != std::fs::metadata(&local).map(|m| m.len()).ok() {
            crate::whole::copy(&local, &nas).with_context(|| format!("put {name} on the NAS"))?;
        }
        Ok(Some(name))
    }
}

/// Adds the tiles of the archive at `p` that `s` lacks.
fn merge_file(s: &mut Staged, p: &Path) -> Result<()> {
    let a = Archive::open(p)?;
    for e in a.entries() {
        if s.tiles.contains_key(&e.key) {
            continue;
        }
        let b = a.get_entry(e);
        s.w.write_all(b)?;
        s.tiles.insert(e.key, (s.pos, b.len() as u32));
        s.pos += b.len() as u64;
    }
    Ok(())
}

/// Adds the tiles of `area`'s archive in `index` that `s` lacks.
fn merge(s: &mut Staged, dir: &Path, store: &Path, index: &Index, area: &str) -> Result<()> {
    match local_archive(dir, store, index, area)? {
        Some(old) => merge_file(s, &old),
        None => Ok(()),
    }
}

/// Writes an archive of `tiles` (key → their offset and length in `data`), in key order, flushed.
fn write_archive(data: &std::fs::File, tiles: &BTreeMap<u64, (u64, u32)>, path: &Path) -> Result<()> {
    use store::sys::PosIo;
    let meta = br#"{"kind":"aws-terrarium raw tiles"}"#;
    let mut w = BufWriter::with_capacity(4 << 20, std::fs::File::create(path)?);
    w.write_all(MAGIC)?;
    w.write_all(&[0u8; 16])?;
    w.write_all(&(meta.len() as u32).to_le_bytes())?;
    w.write_all(meta)?;
    let mut pos = 28 + meta.len() as u64;
    let mut entries = Vec::with_capacity(tiles.len());
    let mut b = Vec::new();
    for (&key, &(offset, len)) in tiles {
        b.resize(len as usize, 0);
        data.read_exact_at(&mut b, offset)?;
        w.write_all(&b)?;
        entries.push(Entry { key, offset: pos, len, raw_len: len });
        pos += len as u64;
    }
    let pad = (8 - pos % 8) % 8;
    w.write_all(&vec![0u8; pad as usize])?;
    let index_off = pos + pad;
    w.write_all(bytemuck::cast_slice(&entries))?;
    let mut f = w.into_inner().map_err(|e| e.into_error())?;
    f.seek(SeekFrom::Start(8))?;
    f.write_all(&index_off.to_le_bytes())?;
    f.write_all(&(entries.len() as u64).to_le_bytes())?;
    f.sync_all()?;
    Ok(())
}

/// A tile's place in a cache or store (`<z>/<x>/<y>.png` or `.none`): its zoom, column, row and
/// whether AWS has it.
pub fn parse_tile(rel: &str) -> Option<(u8, u32, u32, bool)> {
    let rel = rel.strip_prefix("./").unwrap_or(rel);
    let mut parts = rel.split('/');
    let (z, x, file) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (y, has) = match (file.strip_suffix(".png"), file.strip_suffix(".none")) {
        (Some(y), _) => (y, true),
        (_, Some(y)) => (y, false),
        _ => return None,
    };
    Some((z, x, y.parse().ok()?, has))
}

/// Packs the tiles AWS gave that wait in the local cache `dir` (a minute old at least: a job may
/// still be writing the newest) into their areas' archives on the NAS's `store`, and deletes them
/// here once they're there; how many were packed.
pub fn pack_local(dir: &Path, store: &Path, root: &Path) -> Result<usize> {
    let mut loose: Vec<(PathBuf, u8, u32, u32, bool)> = Vec::new();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    for z in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let zname = z.file_name().to_string_lossy().into_owned();
        if zname.parse::<u8>().is_err() {
            continue;
        }
        for x in std::fs::read_dir(z.path()).into_iter().flatten().flatten() {
            for t in std::fs::read_dir(x.path()).into_iter().flatten().flatten() {
                let rel = format!("{zname}/{}/{}", x.file_name().to_string_lossy(), t.file_name().to_string_lossy());
                let Some((z, x, y, has)) = parse_tile(&rel) else { continue };
                if t.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < old) {
                    loose.push((t.path(), z, x, y, has));
                }
            }
        }
    }
    if loose.is_empty() {
        return Ok(0);
    }
    let mut p = Packer::new(dir, store)?;
    let mut packed = Vec::new();
    for (path, z, x, y, has) in loose {
        let png = if has {
            let b = std::fs::read(&path)?;
            if !crate::whole::png_whole(&b) {
                eprintln!("rawpack: {} isn't whole: deleted, not packed", path.display());
                std::fs::remove_file(&path).ok();
                continue;
            }
            Some(b)
        } else {
            None
        };
        p.add(z, x, y, png.as_deref())?;
        packed.push(path);
    }
    let areas = p.finish(root)?;
    for path in &packed {
        std::fs::remove_file(path).ok();
    }
    if !areas.is_empty() {
        eprintln!("rawpack: {} tiles packed into {} area archive{} on the NAS", packed.len(), areas.len(), if areas.len() == 1 { "" } else { "s" });
    }
    Ok(packed.len())
}

/// Packs the tiles of a tar stream (the NAS's own, sent by `tar` over SSH: tools/nas/raw-pack.sh)
/// into their areas' archives (made in `dir`, gone from it once on the NAS); how many tiles it held,
/// and how many were new.
pub fn pack_tar(input: impl Read, dir: &Path, store: &Path, root: &Path) -> Result<(usize, usize)> {
    let mut p = Packer::new(dir, store)?;
    p.keep = false;
    let mut n = 0;
    for entry in Tar::new(input) {
        let (name, data) = entry?;
        let Some((z, x, y, has)) = parse_tile(&name) else { continue };
        if has && !crate::whole::png_whole(&data) {
            eprintln!("rawpack: {name} on the NAS isn't whole: left out (taken again from AWS when needed)");
            continue;
        }
        p.add(z, x, y, has.then_some(&data[..]))?;
        n += 1;
        if n % 20_000 == 0 {
            eprintln!("rawpack: {n} tiles read");
        }
    }
    let added = p.added;
    let areas = p.finish(root)?;
    eprintln!("rawpack: {n} tiles, {added} new to their archives, in {} area archives", areas.len());
    Ok((n, added))
}

/// The regular files of a tar stream (ustar, with GNU long names): (name, bytes).
struct Tar<R: Read> {
    r: R,
    long: Option<String>,
    done: bool,
}

impl<R: Read> Tar<R> {
    fn new(r: R) -> Self {
        Tar { r, long: None, done: false }
    }
}

impl<R: Read> Iterator for Tar<R> {
    type Item = Result<(String, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }
            let mut h = [0u8; 512];
            if let Err(e) = self.r.read_exact(&mut h) {
                self.done = true;
                return if e.kind() == std::io::ErrorKind::UnexpectedEof { None } else { Some(Err(e.into())) };
            }
            if h.iter().all(|&b| b == 0) {
                self.done = true;
                return None;
            }
            let field = |a: usize, b: usize| String::from_utf8_lossy(&h[a..b]).trim_end_matches('\0').to_string();
            let size = match u64::from_str_radix(field(124, 136).trim(), 8) {
                Ok(s) => s,
                Err(_) => {
                    self.done = true;
                    return Some(Err(anyhow::anyhow!("a tar header with no size")));
                }
            };
            let mut data = vec![0u8; size as usize];
            if let Err(e) = self.r.read_exact(&mut data) {
                self.done = true;
                return Some(Err(e.into()));
            }
            let pad = (512 - size % 512) % 512;
            if let Err(e) = std::io::copy(&mut (&mut self.r).take(pad), &mut std::io::sink()) {
                self.done = true;
                return Some(Err(e.into()));
            }
            match h[156] {
                b'L' => {
                    self.long = Some(String::from_utf8_lossy(&data).trim_end_matches('\0').to_string());
                    continue;
                }
                b'0' | 0 => {
                    let name = self.long.take().unwrap_or_else(|| {
                        let (prefix, name) = (field(345, 500), field(0, 100));
                        if &h[257..262] == b"ustar" && !prefix.is_empty() { format!("{prefix}/{name}") } else { name }
                    });
                    return Some(Ok((name, data)));
                }
                _ => {
                    self.long = None;
                    continue;
                }
            }
        }
    }
}

/// An error unless `p` is an archive of raw tiles (for checks).
pub fn check(p: &Path) -> Result<usize> {
    let a = Archive::open(p)?;
    if !a.meta_json.contains("raw tiles") {
        bail!("{}: not an archive of raw tiles", p.display());
    }
    Ok(a.entries().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        crate::whole::testfiles::png()
    }

    #[test]
    fn tiles_go_up_packed_by_area_and_come_back() {
        let d = tempfile::tempdir().unwrap();
        let (root, dir) = (d.path().join("nas"), d.path().join("cache"));
        let store = root.join("sources/aws-terrarium");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // Tiles AWS gave, waiting here (a minute old), one it hasn't, and one too new to pack.
        let put = |rel: &str, b: &[u8], age: u64| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b).unwrap();
            std::fs::File::options().append(true).open(&p).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(age)).unwrap();
        };
        put("12/2048/1365.png", &png(), 120);
        put("12/2049/1365.none", b"", 120);
        put("8/128/85.png", &png(), 120);
        put("3/4/2.png", &png(), 120);
        put("12/2050/1366.png", &png(), 1);
        assert_eq!(pack_local(&dir, &store, &root).unwrap(), 4);
        let index = Index::load(&store).unwrap();
        // 12/2048/1365 and 8/128/85 are under z6 tile 32/21; 3/4/2 is above z6.
        assert_eq!(index.areas.keys().collect::<Vec<_>>(), ["6-32-21", "low"]);
        for name in index.areas.values() {
            assert!(store.join("packs").join(name).exists() && dir.join("packs").join(name).exists());
        }
        assert!(!dir.join("12/2048/1365.png").exists() && dir.join("12/2050/1366.png").exists(), "packed tiles go; the newest waits");
        let a = Archive::open(&dir.join("packs").join(&index.areas["6-32-21"])).unwrap();
        assert_eq!(a.get(12, 2048, 1365), Some(&png()[..]));
        assert_eq!(a.get(12, 2049, 1365), Some(&[][..]), "AWS hasn't it: an empty entry");
        assert_eq!(a.get(8, 128, 85), Some(&png()[..]));
        assert_eq!(a.get(12, 2050, 1366), None);
        // Later tiles: the area's archive made again with them, the old one gone, the index on.
        put("12/2050/1366.png", &png(), 120);
        let first = index.areas["6-32-21"].clone();
        assert_eq!(pack_local(&dir, &store, &root).unwrap(), 1);
        let index2 = Index::load(&store).unwrap();
        assert_ne!(index2.areas["6-32-21"], first);
        assert_eq!(index2.areas["low"], index.areas["low"]);
        assert!(!store.join("packs").join(&first).exists());
        let a = Archive::open(&store.join("packs").join(&index2.areas["6-32-21"])).unwrap();
        assert_eq!(a.entries().len(), 4);
        assert_eq!(a.get(12, 2050, 1366), Some(&png()[..]));
        assert_eq!(check(&store.join("packs").join(&index2.areas["6-32-21"])).unwrap(), 4);
        // A fresh cache takes an area's archive from the NAS whole.
        let fresh = d.path().join("fresh");
        let local = local_archive(&fresh, &store, &index2, "6-32-21").unwrap().unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), std::fs::read(store.join("packs").join(&index2.areas["6-32-21"])).unwrap());
        assert!(local_archive(&fresh, &store, &index2, "6-1-1").unwrap().is_none());
    }

    #[test]
    fn a_tar_stream_of_the_nas_tiles_is_packed() {
        let d = tempfile::tempdir().unwrap();
        let (root, dir) = (d.path().join("nas"), d.path().join("cache"));
        let store = root.join("sources/aws-terrarium");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // A tar of a store's loose tiles, as `tar -cf -` makes it (with an odd file among them).
        let src = d.path().join("src");
        for (rel, b) in [("12/2048/1365.png", png()), ("12/2049/1365.none", Vec::new()), ("9/256/170.png", png()), ("notes.txt", b"x".to_vec())] {
            std::fs::create_dir_all(src.join(rel).parent().unwrap()).unwrap();
            std::fs::write(src.join(rel), b).unwrap();
        }
        let tar = std::process::Command::new("tar").env("COPYFILE_DISABLE", "1").args(["--no-mac-metadata", "--no-xattrs", "-cf", "-", "-C"]).arg(&src).args(["./12/2048/1365.png", "./12/2049/1365.none", "./9/256/170.png", "./notes.txt"]).output().unwrap();
        assert!(tar.status.success());
        let (n, added) = pack_tar(&tar.stdout[..], &dir, &store, &root).unwrap();
        assert_eq!((n, added), (3, 3));
        let index = Index::load(&store).unwrap();
        assert_eq!(index.areas.keys().collect::<Vec<_>>(), ["6-32-21"]);
        let a = Archive::open(&store.join("packs").join(&index.areas["6-32-21"])).unwrap();
        assert_eq!(a.get(9, 256, 170), Some(&png()[..]));
        assert_eq!(a.get(12, 2049, 1365), Some(&[][..]));
        // Again: nothing new, the same archive.
        let (n, added) = pack_tar(&tar.stdout[..], &dir, &store, &root).unwrap();
        assert_eq!((n, added), (3, 0));
        assert_eq!(Index::load(&store).unwrap(), index);
    }

    #[test]
    fn two_packers_at_once_keep_each_others_tiles() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let store = root.join("sources/aws-terrarium");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // Both begin from the same (empty) index for one area; each adds its own tile.
        let mut a = Packer::new(&d.path().join("a"), &store).unwrap();
        let mut b = Packer::new(&d.path().join("b"), &store).unwrap();
        a.add(12, 2048, 1365, Some(&png())).unwrap();
        b.add(12, 2048, 1366, Some(&png())).unwrap();
        a.finish(&root).unwrap();
        b.finish(&root).unwrap();
        // The second made its archive again with the first's merged in: both tiles there.
        let index = Index::load(&store).unwrap();
        let arch = Archive::open(&store.join("packs").join(&index.areas["6-32-21"])).unwrap();
        assert!(arch.get(12, 2048, 1365).is_some() && arch.get(12, 2048, 1366).is_some());
        assert_eq!(std::fs::read_dir(store.join("packs")).unwrap().count(), 2, "the area's archive and the index, nothing left over");
    }

    #[test]
    fn tiles_are_named_by_their_place() {
        assert_eq!(parse_tile("./12/690/949.png"), Some((12, 690, 949, true)));
        assert_eq!(parse_tile("8/1/2.none"), Some((8, 1, 2, false)));
        assert_eq!(parse_tile("8/1/2.png.tmp"), None);
        assert_eq!(parse_tile("@eaDir/8/1/2.png"), None);
        assert_eq!(area(12, 690, 949), "6-10-14");
        assert_eq!(area(5, 3, 3), "low");
    }
}
