//! AWS's raw terrain tiles on the NAS, packed (docs/plan.md §3, Downloads), in archives grouped the
//! way the terrain's packs are (crate::terrain_pack::build_q), so a job copies the archives of what
//! it makes and no more: z9–12 by their z6 tile (`6-<x>-<y>`), z3–8 by their z3 tile
//! (`3-<x>-<y>`), z0–2 together (`root`). Each is roadcore::archive's RDTILES1 (an empty entry for
//! a tile AWS doesn't have), content-named (`<area>.<hash16>.tiles`) in
//! `sources/aws-terrarium/packs/`, so a copy anywhere is right for good; `packs/index.json` lists
//! each area's archives, oldest first.
//!
//! New tiles reach the NAS as an archive of their own beside their area's others: one large write
//! (the NAS takes small files a tile at a time at ~23 a second, and stalls doing it), and nothing
//! there is written again for them. An area's archives are kept each more than twice the size of
//! all those after it (archives from the first that isn't are merged into one), so an area has a
//! dozen at most, and a tile is rewritten a dozen times at most, however it came.
//!
//! On the NAS, the build Mac's archives are named by the index or listed in its `gone`, with when:
//! an archive is listed before it's put there and taken off once named (named only once it's there
//! whole), and one the index stops naming (merged) is listed then. A day later, unnamed, it's
//! deleted: so a job that read the index before stays right, and one cut short putting an archive
//! there leaves nothing behind (its temporary file is swept a day later too). An archive the index
//! names that's gone from the NAS is passed over and taken out of it. The build Mac alone changes
//! the index (it alone writes the build's records: crate::out::check_writer), under its build lock.
//! A helper's jobs pack too, every loose tile in its cache (`SCENIC_HANDOFF`): they put their
//! archives on the NAS unlisted and hand them off; the build Mac names them as it merges the
//! hand-off (`name_handed`). One a day old that's still neither named nor listed (its hand-off
//! never came) is listed to go then, so it's deleted a day after that.
//!
//! The build Mac's cache keeps the tiles AWS just gave (`<z>/<x>/<y>.png`, `.none`) until they're
//! packed, and copies of the archives it reads (`packs/`), each copied whole from the NAS once and
//! checked against its name.

use anyhow::{bail, ensure, Context, Result};
use roadcore::archive::{tile_key, Archive, ArchiveWriter, Entry, MAGIC};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use store::sys::PosIo;

/// The archives' index, under the store.
pub const INDEX: &str = "packs/index.json";

/// How long an archive stays on the NAS once listed in the index's `gone` (unnamed): a job that
/// read the index before may still want it.
const GRACE: u64 = 24 * 3600;

/// What's spooled, and the new archives made from it, before they're put on the NAS and named (two
/// changes of the index): bounds the disk the NAS's own tiles take here while they're packed.
const GROUP: u64 = 1 << 30;

/// How far a long step is, for the status: `(what, done, total)` ("tiles", "areas" …).
pub type Progress<'a> = &'a (dyn Fn(&str, u64, u64) + Sync);

const META: &str = r#"{"kind":"aws-terrarium raw tiles"}"#;

/// The area a tile is packed in, as the terrain is: its z6 tile's for z9–12, its z3 tile's for
/// z3–8, `root` for z0–2.
pub fn area(z: u8, x: u32, y: u32) -> String {
    match z {
        9.. => format!("6-{}-{}", x >> (z - 6), y >> (z - 6)),
        3..=8 => format!("3-{}-{}", x >> (z - 3), y >> (z - 3)),
        _ => "root".into(),
    }
}

/// An archive the index names: its name and size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pack {
    pub name: String,
    pub bytes: u64,
}

/// Each area's archives, oldest first; and the archives on the NAS it doesn't name, with when they
/// were listed (unix seconds): deleted GRACE later.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub areas: BTreeMap<String, Vec<Pack>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gone: BTreeMap<String, u64>,
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

    /// The index to change and save (`Packer::commit`, `name_handed`): `load`, but an error when its
    /// file is missing while the store holds archives (lost, or moved aside): one saved now would
    /// name none of them, and the sweep would then delete them all. Nothing is changed until it's
    /// back.
    /// (`naming`: the archives about to be named, `name_handed`'s: a store whose only archives are
    /// those never had an index, as when a helper put the first ones.)
    fn load_to_change(store: &Path, naming: &[&str]) -> Result<Index> {
        if let Err(e) = std::fs::metadata(store.join(INDEX)) {
            if e.kind() == std::io::ErrorKind::NotFound {
                let others = match std::fs::read_dir(store.join("packs")) {
                    Ok(rd) => rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).any(|n| n.ends_with(".tiles") && !naming.contains(&n.as_str())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                    Err(e) => return Err(e).with_context(|| format!("list {}", store.join("packs").display())),
                };
                ensure!(!others, "{} is missing, but its archives are there: put it back (nothing is changed until then)", store.join(INDEX).display());
            }
        }
        Index::load(store)
    }

    pub fn save(&self, store: &Path) -> Result<()> {
        std::fs::create_dir_all(store.join("packs"))?;
        crate::whole::write(&store.join(INDEX), &serde_json::to_vec_pretty(self)?)
    }

    /// An area's archives, oldest first.
    pub fn of(&self, area: &str) -> &[Pack] {
        self.areas.get(area).map_or(&[], Vec::as_slice)
    }

    fn names(&self, name: &str) -> bool {
        self.areas.values().flatten().any(|p| p.name == name)
    }
}

/// Archives copied from the NAS at once by a process: a terrain job's reads span dozens of areas,
/// and the NAS's disks serve a few large reads well, dozens at once badly.
const COPIES: usize = 3;

/// A turn to copy an archive from the NAS (`COPIES` at once), given back when dropped.
struct Turn;

static TURNS: (std::sync::Mutex<usize>, std::sync::Condvar) = (std::sync::Mutex::new(0), std::sync::Condvar::new());

impl Turn {
    fn take() -> Turn {
        let mut n = TURNS.0.lock().unwrap();
        while *n >= COPIES {
            n = TURNS.1.wait(n).unwrap();
        }
        *n += 1;
        Turn
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        *TURNS.0.lock().unwrap() -= 1;
        TURNS.1.notify_one();
    }
}

/// Archive `name`'s copy here (`dir/packs/`), copied whole from the NAS's `store` the first time
/// (`COPIES` at once) and checked against its name: a copy cut short or changed on the way is never
/// used.
pub fn local_copy(dir: &Path, store: &Path, name: &str) -> Result<PathBuf> {
    // (Held by this process: store::cachefile.)
    let local = dir.join("packs").join(name);
    if let Some(p) = store::cachefile::hold_existing(&local)? {
        return Ok(p);
    }
    let _turn = Turn::take();
    let src = store.join("packs").join(name);
    store::cachefile::hold(&local, &mut |tmp| {
        let (n, want) = (store::sys::copy_data(&src, tmp)?, std::fs::metadata(&src)?.len());
        if n != want {
            return Err(std::io::Error::other(format!("copy {name} from the NAS: {n} of {want} bytes")));
        }
        if !named_right(tmp, name).map_err(std::io::Error::other)? {
            return Err(std::io::Error::other(format!("{name} on the NAS isn't what its name says")));
        }
        Ok(())
    })
    .with_context(|| format!("copy {name} from the NAS"))
}

/// Deletes this process's copy here of an archive (`dir/packs/<name>`), unless another job holds
/// it (store::cachefile: then room-making takes it later).
fn drop_copy(p: &Path) {
    store::cachefile::release(p);
    store::cachefile::try_remove(p);
}

/// Whether the file at `p` holds what archive `name` names (its hash).
fn named_right(p: &Path, name: &str) -> Result<bool> {
    let want = name.strip_suffix(".tiles").and_then(|n| n.rsplit('.').next()).unwrap_or("");
    Ok(store::naming::hash16_file(p)? == want)
}

/// An archive's entries, read alone (not the whole archive: over SMB, one from the NAS's), and the
/// file, open.
pub fn entries_of(p: &Path) -> Result<(std::fs::File, Vec<Entry>)> {
    let f = std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
    let mut h = [0u8; 24];
    f.read_exact_at(&mut h, 0).with_context(|| format!("read {}", p.display()))?;
    ensure!(&h[..8] == MAGIC, "{}: not a tile archive", p.display());
    let off = u64::from_le_bytes(h[8..16].try_into()?);
    let n = u64::from_le_bytes(h[16..24].try_into()?);
    let size = std::mem::size_of::<Entry>() as u64;
    let end = n.checked_mul(size).and_then(|b| b.checked_add(off));
    ensure!(end.is_some_and(|e| e <= f.metadata().map_or(0, |m| m.len())), "{}: its entries run past its end", p.display());
    let mut b = vec![0u8; (n * size) as usize];
    f.read_exact_at(&mut b, off).with_context(|| format!("read {}", p.display()))?;
    let entries: Vec<Entry> = b.chunks_exact(size as usize).map(bytemuck::pod_read_unaligned::<Entry>).collect();
    ensure!(entries.iter().all(|e| e.offset >= 28 && e.offset.checked_add(e.len as u64).is_some_and(|end| end <= off)), "{}: a tile outside it", p.display());
    Ok((f, entries))
}

/// Where the archives due merging (into one) start: the first that isn't more than twice the size
/// of all those after it. None when each is.
fn due(list: &[Pack]) -> Option<usize> {
    let mut after: u64 = list.iter().map(|p| p.bytes).sum();
    for (i, p) in list.iter().enumerate() {
        after -= p.bytes;
        if i + 1 < list.len() && p.bytes <= after.saturating_mul(2) {
            return Some(i);
        }
    }
    None
}

/// Whether `e` is a file not found (an archive gone from the NAS).
pub fn not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A name for a file of this process's own, among those of others in a cache (and of other packers
/// in it).
fn own(stem: &str, ext: &str) -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("{stem}.{}-{}.{ext}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// An area's tiles: those its archives have (keys), and the new ones (key → their place in the
/// spool: offset, length; length 0 for one AWS doesn't have).
#[derive(Default)]
struct Area {
    have: HashSet<u64>,
    new: BTreeMap<u64, (u64, u32)>,
}

/// Packs tiles: each area's new ones (those its archives lack) into an archive of their own, put on
/// the NAS and named in the index, then each area's newest archives merged where due.
pub struct Packer {
    /// The local cache: archives are made in its `packs/`, and stay there when `keep`.
    dir: PathBuf,
    store: PathBuf,
    root: PathBuf,
    /// The index as last read.
    index: Index,
    /// Every new tile's bytes as they come, in one file: each area's archive is written from it at
    /// the end, its tiles in key order.
    spool: BufWriter<std::fs::File>,
    spool_path: PathBuf,
    pos: u64,
    areas: BTreeMap<String, Area>,
    /// The last tile's area, and the areas given new archives.
    last: Option<String>,
    touched: std::collections::BTreeSet<String>,
    /// Archives the index names that the NAS lacks (area, name): taken out of it at the next change.
    missing: Vec<(String, String)>,
    /// Tiles added that their areas' archives didn't have.
    pub added: usize,
    /// Whether the archives made stay in `dir/packs` (a job's: read again soon) or go once on the
    /// NAS (the NAS's own tiles packed: tens of GB).
    pub keep: bool,
    /// A helper's job's outbox (`SCENIC_HANDOFF`): its archives go onto the NAS, and the build Mac,
    /// which alone writes the index, names them from its hand-off (`handed`, crate::handoff). None:
    /// the build Mac's own packing, the index changed here.
    handoff: Option<PathBuf>,
    handed: Vec<(String, Pack)>,
}

impl Packer {
    /// Packing into the local cache `dir` (its `packs/`) for the NAS's `store`, the build's at
    /// `root`: an error on a Mac that doesn't write its records.
    pub fn new(dir: &Path, store: &Path, root: &Path) -> Result<Packer> {
        Packer::open(dir, store, root, std::env::var_os("SCENIC_HANDOFF").map(PathBuf::from))
    }

    /// `new`, a helper's job's (its outbox `handoff`) or the build Mac's (None).
    fn open(dir: &Path, store: &Path, root: &Path, handoff: Option<PathBuf>) -> Result<Packer> {
        if handoff.is_none() {
            crate::out::check_writer(root)?;
        }
        let index = Index::load(store)?;
        std::fs::create_dir_all(dir.join("packs"))?;
        let spool_path = dir.join("packs").join(own("packing", "spool"));
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&spool_path).with_context(|| format!("open {}", spool_path.display()))?;
        // (Held while it's written: room-making leaves it.)
        store::cachefile::lock_shared(&f)?;
        Ok(Packer { dir: dir.to_path_buf(), store: store.to_path_buf(), root: root.to_path_buf(), index, spool: BufWriter::with_capacity(1 << 20, f), spool_path, pos: 0, areas: BTreeMap::new(), last: None, touched: Default::default(), missing: Vec::new(), added: 0, keep: true, handoff, handed: Vec::new() })
    }

    /// Adds a tile (None: AWS doesn't have it); whether it's new to its area's archives. Past an
    /// area with GROUP spooled, what's spooled goes up first: the disk it takes here stays small,
    /// and tiles given in area order are packed an area once.
    pub fn add(&mut self, z: u8, x: u32, y: u32, png: Option<&[u8]>) -> Result<bool> {
        let a = area(z, x, y);
        if self.pos >= GROUP && self.last.as_ref() != Some(&a) {
            self.flush()?;
        }
        self.last = Some(a.clone());
        if !self.areas.contains_key(&a) {
            let have = match self.keys(&a, false) {
                Ok(h) => h,
                // (An archive named in the index as read before is gone: that's out of date. One
                // the index read again still names is passed over, and taken out of it.)
                Err(e) if not_found(&e) => {
                    self.index = Index::load(&self.store)?;
                    self.keys(&a, true)?
                }
                Err(e) => return Err(e),
            };
            self.areas.insert(a.clone(), Area { have, new: BTreeMap::new() });
        }
        let s = self.areas.get_mut(&a).unwrap();
        let key = tile_key(z, x, y);
        // (A tile both AWS's and "none", as two files can say: AWS's.)
        let none_yet = s.new.get(&key).is_some_and(|e| e.1 == 0) && png.is_some();
        if s.have.contains(&key) || (s.new.contains_key(&key) && !none_yet) {
            return Ok(false);
        }
        let b = png.unwrap_or(&[]);
        self.spool.write_all(b)?;
        s.new.insert(key, (self.pos, b.len() as u32));
        self.pos += b.len() as u64;
        if !none_yet {
            self.added += 1;
        }
        Ok(true)
    }

    /// The tiles `area`'s archives have: from their copies here, else the NAS's (their entries
    /// alone). `pass`: one the NAS lacks is passed over (listed as missing), not an error.
    fn keys(&mut self, area: &str, pass: bool) -> Result<HashSet<u64>> {
        let mut have = HashSet::new();
        for p in self.index.of(area).to_vec() {
            // (This Mac's copy held as it's read, store::cachefile: one room-making took between a
            // look and the read would pass for the NAS's own missing.)
            let local = self.dir.join("packs").join(&p.name);
            let at = match store::cachefile::hold_existing(&local) {
                Ok(Some(l)) => l,
                _ => self.store.join("packs").join(&p.name),
            };
            match entries_of(&at) {
                Ok((_, e)) => have.extend(e.iter().map(|e| e.key)),
                Err(e) if pass && not_found(&e) => {
                    eprintln!("rawpack: {} is named but not on the NAS: passed over, and taken out of the index", p.name);
                    self.missing.push((area.to_string(), p.name.clone()));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(have)
    }

    /// Writes each area's new tiles so far into an archive of their own and puts them on the NAS,
    /// named in the index (GROUP's worth at a time); the spool starts again, empty.
    pub fn flush(&mut self) -> Result<()> {
        self.spool.flush()?;
        let mut pending = Vec::new();
        for (area, a) in &mut self.areas {
            if a.new.is_empty() {
                continue;
            }
            let new = std::mem::take(&mut a.new);
            a.have.extend(new.keys());
            pending.push((area.clone(), new.into_iter().map(|(k, (o, l))| (k, o, l)).collect::<Vec<_>>()));
        }
        let mut group: Vec<(String, Pack)> = Vec::new();
        for (area, tiles) in pending {
            let spool = self.spool.get_ref();
            let pack = self.write(&area, tiles.len(), |i, b| {
                let (k, o, l) = tiles[i];
                b.resize(l as usize, 0);
                spool.read_exact_at(b, o)?;
                Ok(k)
            })?;
            self.touched.insert(area.clone());
            group.push((area, pack));
            if group.iter().map(|g| g.1.bytes).sum::<u64>() >= GROUP {
                self.put_up(std::mem::take(&mut group))?;
            }
        }
        self.put_up(group)?;
        self.spool.get_ref().set_len(0)?;
        self.spool.seek(std::io::SeekFrom::Start(0))?;
        self.pos = 0;
        Ok(())
    }

    /// `flush`, then each area given new archives has its newest merged where due (one that can't
    /// be now is merged when it next has new tiles). Those areas.
    pub fn finish(self) -> Result<Vec<String>> {
        self.finish_with(&|_, _, _| {})
    }

    /// `finish`, saying how far it is: `progress("areas", done, total)` as each area given new
    /// archives has had its merging done.
    pub fn finish_with(mut self, progress: Progress) -> Result<Vec<String>> {
        self.flush()?;
        let touched: Vec<String> = std::mem::take(&mut self.touched).into_iter().collect();
        // A helper's: its archives handed off for the build Mac to name (and merge, when it packs
        // those areas next).
        if let Some(dir) = self.handoff.clone() {
            if !self.handed.is_empty() {
                crate::handoff::write(&dir, &crate::handoff::Handoff { raw: std::mem::take(&mut self.handed), ..Default::default() })?;
            }
            progress("areas", touched.len() as u64, touched.len() as u64);
            return Ok(touched);
        }
        for (i, area) in touched.iter().enumerate() {
            // (The agent asked to stop: the rest are merged when they next have new tiles.)
            if crate::agent::stopping() {
                break;
            }
            progress("areas", i as u64, touched.len() as u64);
            if let Err(e) = self.merge_due(area) {
                eprintln!("rawpack: {area}'s archives not merged now ({e:#})");
            }
        }
        progress("areas", touched.len() as u64, touched.len() as u64);
        if !touched.is_empty() {
            self.sweep();
        }
        Ok(touched)
    }

    /// Temporary files a day old, left by a process cut short: on the NAS, an archive's copy
    /// (`<name>.<host>.<pid>.tmp`); here, its own (`.part`, `.spool`).
    fn sweep(&self) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(GRACE);
        for (d, exts) in [(self.store.join("packs"), &["tmp"][..]), (self.dir.join("packs"), &["part", "spool"][..])] {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                let ours = p.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.contains(&x));
                if ours && e.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < old) {
                    // (Here, not one a live job writes: store::cachefile.)
                    if d == self.dir.join("packs") {
                        store::cachefile::try_remove(&p);
                    } else {
                        std::fs::remove_file(&p).ok();
                    }
                }
            }
        }
    }

    /// An archive of `n` tiles written here, named by its content: `tile(i, b)` puts the i-th's
    /// bytes (in key order) in `b` and gives its key.
    fn write(&self, area: &str, n: usize, mut tile: impl FnMut(usize, &mut Vec<u8>) -> Result<u64>) -> Result<Pack> {
        // (Written held, store::cachefile's scratch file beside its folder: room-making leaves it.)
        let (part, held) = store::cachefile::scratch(&self.dir.join("packs").join(own(area, "part")))?;
        let r = (|| -> Result<()> {
            let mut w = ArchiveWriter::create(&part, META)?;
            let mut b = Vec::new();
            for i in 0..n {
                let k = tile(i, &mut b)?;
                w.add((k >> 58) as u8, ((k >> 29) & 0x1fff_ffff) as u32, (k & 0x1fff_ffff) as u32, &b, b.len())?;
            }
            w.finish()?;
            Ok(())
        })();
        if let Err(e) = r {
            std::fs::remove_file(&part).ok();
            return Err(e);
        }
        let name = format!("{area}.{}.tiles", store::naming::hash16_file(&part)?);
        let bytes = std::fs::metadata(&part)?.len();
        // (Named only if it isn't there: one of the same name has the same tiles.)
        store::cachefile::publish(held, &part, &self.dir.join("packs").join(&name))?;
        Ok(Pack { name, bytes })
    }

    /// Puts archive `p` (made here) on the NAS, unless it's there whole already.
    fn put(&self, p: &Pack) -> Result<()> {
        let nas = self.store.join("packs").join(&p.name);
        if std::fs::metadata(&nas).map(|m| m.len()).ok() != Some(p.bytes) {
            std::fs::create_dir_all(nas.parent().unwrap())?;
            crate::whole::copy(&self.dir.join("packs").join(&p.name), &nas).with_context(|| format!("put {} on the NAS", p.name))?;
        }
        Ok(())
    }

    /// Puts new archives on the NAS and names them in the index: listed in its `gone` first, so one
    /// cut short there goes later; then each added to its area's archives. The copies here go
    /// unless kept.
    fn put_up(&mut self, group: Vec<(String, Pack)>) -> Result<()> {
        if group.is_empty() {
            return Ok(());
        }
        // A helper's: on the NAS whole, then handed off (`finish`); the index isn't its to change.
        if self.handoff.is_some() {
            for (area, p) in group {
                self.put(&p)?;
                if !self.keep {
                    drop_copy(&self.dir.join("packs").join(&p.name));
                }
                self.handed.push((area, p));
            }
            return Ok(());
        }
        let now = unix_now();
        self.commit(|ix| {
            for (_, p) in &group {
                if !ix.names(&p.name) {
                    ix.gone.insert(p.name.clone(), now);
                }
            }
        })?;
        for (_, p) in &group {
            self.put(p)?;
        }
        // (Named only once it's there whole: a put stopped past GRACE may have seen it deleted.)
        let (store, mut absent) = (self.store.clone(), Vec::new());
        self.commit(|ix| {
            for (area, p) in &group {
                if !there(&store, p) {
                    ix.gone.insert(p.name.clone(), unix_now());
                    absent.push(p.name.clone());
                    continue;
                }
                let l = ix.areas.entry(area.clone()).or_default();
                if !l.contains(p) {
                    l.push(p.clone());
                }
            }
        })?;
        ensure!(absent.is_empty(), "not on the NAS whole when named: {}", absent.join(", "));
        if !self.keep {
            for (_, p) in &group {
                drop_copy(&self.dir.join("packs").join(&p.name));
            }
        }
        Ok(())
    }

    /// Merges `area`'s newest archives into one while `due`, each copied here whole first; the
    /// merged archive is named in their place if they're still there (another packer may have
    /// merged them first: then it goes, unnamed, and that packer carries on).
    fn merge_due(&mut self, area: &str) -> Result<()> {
        while let Some(from) = due(self.index.of(area)) {
            let run = self.index.of(area)[from..].to_vec();
            let merged = self.merge(area, &run)?;
            let now = unix_now();
            self.commit(|ix| {
                if !ix.names(&merged.name) {
                    ix.gone.insert(merged.name.clone(), now);
                }
            })?;
            self.put(&merged)?;
            let (store, mut named, mut absent) = (self.store.clone(), false, false);
            self.commit(|ix| {
                if !there(&store, &merged) {
                    ix.gone.insert(merged.name.clone(), unix_now());
                    absent = true;
                    return;
                }
                let l = ix.areas.entry(area.to_string()).or_default();
                if let Some(i) = l.windows(run.len()).position(|w| w == run.as_slice()) {
                    l.splice(i..i + run.len(), [merged.clone()]);
                    for p in &run {
                        ix.gone.insert(p.name.clone(), now);
                    }
                    named = true;
                }
            })?;
            if named {
                // (One of them may be the merged archive itself: one that held all the others' tiles.)
                for p in run.iter().filter(|p| p.name != merged.name) {
                    drop_copy(&self.dir.join("packs").join(&p.name));
                }
            }
            if !named || !self.keep {
                drop_copy(&self.dir.join("packs").join(&merged.name));
            }
            ensure!(!absent, "{} wasn't on the NAS whole when named", merged.name);
            if !named {
                break;
            }
        }
        Ok(())
    }

    /// One archive here of the tiles of `run`'s archives (copied here whole first).
    fn merge(&self, area: &str, run: &[Pack]) -> Result<Pack> {
        let archives = run.iter().map(|p| Archive::open(&local_copy(&self.dir, &self.store, &p.name)?)).collect::<Result<Vec<_>>>()?;
        // (A tile in two: the newer's, the same bytes.)
        let mut tiles: BTreeMap<u64, (usize, Entry)> = BTreeMap::new();
        for (i, a) in archives.iter().enumerate() {
            for e in a.entries() {
                tiles.insert(e.key, (i, *e));
            }
        }
        let tiles: Vec<(usize, Entry)> = tiles.into_values().collect();
        self.write(area, tiles.len(), |i, b| {
            let (a, e) = tiles[i];
            b.clear();
            b.extend_from_slice(archives[a].get_entry(&e));
            Ok(e.key)
        })
    }

    /// Changes the index under the build's lock (`root`'s), read again first: another packer may
    /// have changed it. Archives listed in its `gone` GRACE ago or more, and named by nothing, are
    /// deleted then (one that can't be stays listed).
    fn commit(&mut self, f: impl FnOnce(&mut Index)) -> Result<()> {
        let missing = std::mem::take(&mut self.missing);
        let lock = crate::out::BuildLock::take(&self.root)?;
        let mut ix = Index::load_to_change(&self.store, &[])?;
        f(&mut ix);
        // (Archives found missing, still so.)
        for (area, name) in missing {
            if std::fs::metadata(self.store.join("packs").join(&name)).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
                if let Some(l) = ix.areas.get_mut(&area) {
                    l.retain(|p| p.name != name);
                }
            }
        }
        ix.areas.retain(|_, l| !l.is_empty());
        let named: HashSet<String> = ix.areas.values().flatten().map(|p| p.name.clone()).collect();
        let now = unix_now();
        let mut gone = BTreeMap::new();
        for (name, at) in std::mem::take(&mut ix.gone) {
            if named.contains(&name) {
                continue;
            }
            if now < at.saturating_add(GRACE) {
                gone.insert(name, at);
                continue;
            }
            match std::fs::remove_file(self.store.join("packs").join(&name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    gone.insert(name, at);
                    continue;
                }
            }
            drop_copy(&self.dir.join("packs").join(&name));
        }
        // An archive on the NAS the index neither names nor lists to go, a day old: a helper's whose
        // hand-off never came. Listed to go (GRACE later), as a replaced one. (Never from an index
        // that was missing: `load_to_change` refuses one while there are archives; nor while the
        // index names nothing, besides.)
        if let (false, Ok(rd)) = (named.is_empty(), std::fs::read_dir(self.store.join("packs"))) {
            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(GRACE);
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.ends_with(".tiles") && !named.contains(&n) && !gone.contains_key(&n) && e.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < old) {
                    gone.insert(n, now);
                }
            }
        }
        ix.gone = gone;
        ix.save(&self.store)?;
        drop(lock);
        self.index = ix;
        Ok(())
    }
}

/// The archives a helper's jobs put on the NAS (crate::handoff::Handoff::raw), named in `store`'s
/// index, each that's there whole (one that isn't goes as an unnamed one does); how many were new
/// to it. The caller holds the build lock (merging the hand-offs).
pub fn name_handed(store: &Path, handed: &[(String, Pack)], _held: &crate::out::BuildLock) -> Result<Named> {
    if handed.is_empty() {
        return Ok(Named::default());
    }
    let naming: Vec<&str> = handed.iter().map(|(_, p)| p.name.as_str()).collect();
    let mut ix = Index::load_to_change(store, &naming)?;
    let mut n = Named::default();
    for (area, p) in handed {
        if !there(store, p) {
            eprintln!("rawpack: {} was handed over, but isn't on the NAS whole: not named", p.name);
            continue;
        }
        // (Its tiles its area's, all of them: one misfiled, or not an archive, would stop every job
        // that reads the area. Its entries alone are read.)
        match entries_of(&store.join("packs").join(&p.name)) {
            Ok((_, es)) if es.iter().all(|e| area_of_key(e.key) == *area) => {}
            Ok(_) => {
                eprintln!("rawpack: {} was handed over for {area}, but has tiles of other areas: not named", p.name);
                continue;
            }
            Err(e) => {
                // (The NAS slow or away now, most likely: tried again with the next merge.)
                eprintln!("rawpack: {} was handed over, but can't be read now ({e:#}): named later", p.name);
                n.again.push((area.clone(), p.clone()));
                continue;
            }
        }
        ix.gone.remove(&p.name);
        let l = ix.areas.entry(area.clone()).or_default();
        if !l.contains(p) {
            l.push(p.clone());
            n.new += 1;
        }
    }
    ix.save(store)?;
    Ok(n)
}

/// What `name_handed` did: how many archives were new to the index, and those that couldn't be read
/// now (to be named with a later merge).
#[derive(Debug, Default)]
pub struct Named {
    pub new: usize,
    pub again: Vec<(String, Pack)>,
}

/// The area of the tile an archive's key is (roadcore::archive::tile_key).
fn area_of_key(key: u64) -> String {
    let m = (1u64 << 29) - 1;
    area((key >> 58) as u8, ((key >> 29) & m) as u32, (key & m) as u32)
}

/// Whether `a` is an area as `area` names one: `root`, or a z3 or z6 tile's `z-x-y`.
pub fn is_area(a: &str) -> bool {
    if a == "root" {
        return true;
    }
    let v: Vec<&str> = a.split('-').collect();
    let n = |s: &str| s.parse::<u32>().ok().filter(|n| n.to_string() == s);
    match (v.as_slice(), v.first().and_then(|z| n(z))) {
        ([_, x, y], Some(z @ (3 | 6))) => n(x).zip(n(y)).is_some_and(|(x, y)| x < 1 << z && y < 1 << z),
        _ => false,
    }
}

/// Whether `name` is an archive of `area` as packing names one: `<area>.<16 hex>.tiles`.
pub fn named_for(name: &str, area: &str) -> bool {
    name.strip_prefix(area).and_then(|r| r.strip_prefix('.')).and_then(|r| r.strip_suffix(".tiles")).is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
}

/// Whether archive `p` is on the NAS whole (its size).
fn there(store: &Path, p: &Pack) -> bool {
    std::fs::metadata(store.join("packs").join(&p.name)).map(|m| m.len()).ok() == Some(p.bytes)
}

impl Drop for Packer {
    fn drop(&mut self) {
        std::fs::remove_file(&self.spool_path).ok();
    }
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
/// still be writing the newest; none through a link) into archives on the NAS's `store`, and
/// deletes them here once they're named there; how many were packed. The archives made stay here
/// when `keep` (a job's tiles, which the next jobs read again), else go (room-making's: the disk is
/// short). The agent asked to stop: it stops between tiles, every loose tile kept.
pub fn pack_local(dir: &Path, store: &Path, root: &Path, keep: bool) -> Result<usize> {
    pack_local_with(dir, store, root, keep, &|_, _, _| {})
}

/// `pack_local`, saying how far it is (`progress`): the tiles packed of those waiting ("raw tiles",
/// their archives put on the NAS as they go), then the areas whose archives were merged ("areas").
pub fn pack_local_with(dir: &Path, store: &Path, root: &Path, keep: bool, progress: Progress) -> Result<usize> {
    pack_local_to(dir, store, root, keep, std::env::var_os("SCENIC_HANDOFF").map(PathBuf::from), progress)
}

/// `pack_local_with`, for a helper's job's outbox `handoff` or (None) the build Mac.
fn pack_local_to(dir: &Path, store: &Path, root: &Path, keep: bool, handoff: Option<PathBuf>, progress: Progress) -> Result<usize> {
    let mut loose: Vec<(PathBuf, u8, u32, u32, bool)> = Vec::new();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    // (Not through a link, at any level: a tile packed is deleted where it lies.)
    let real = |e: &std::fs::DirEntry| e.file_type().is_ok_and(|t| !t.is_symlink());
    for z in std::fs::read_dir(dir).into_iter().flatten().flatten().filter(real) {
        let zname = z.file_name().to_string_lossy().into_owned();
        if zname.parse::<u8>().is_err() {
            continue;
        }
        for x in std::fs::read_dir(z.path()).into_iter().flatten().flatten().filter(real) {
            for t in std::fs::read_dir(x.path()).into_iter().flatten().flatten().filter(real) {
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
    let mut p = Packer::open(dir, store, root, handoff)?;
    // (A helper keeps none: the M1's disk is small, and the NAS has them.)
    p.keep = keep && p.handoff.is_none();
    let mut packed = Vec::new();
    let (n, mut said) = (loose.len() as u64, std::time::Instant::now());
    progress("raw tiles", 0, n);
    for (i, (path, z, x, y, has)) in loose.into_iter().enumerate() {
        // (The agent asked to stop: what's put up is named; the rest, and every loose tile, waits
        // for the next run. A tile at a time: a GB of archives at most goes up in between.)
        anyhow::ensure!(!crate::agent::stopping(), "the agent is stopping");
        // (Every few seconds: a flush puts a GB of archives on the NAS in between.)
        if said.elapsed() >= std::time::Duration::from_secs(5) {
            progress("raw tiles", i as u64, n);
            said = std::time::Instant::now();
        }
        let png = if has {
            // (Gone meanwhile: room-making's, or another packer's.)
            let Some(b) = store::cachefile::read(&path)? else { continue };
            if !crate::whole::png_whole(&b) {
                eprintln!("rawpack: {} isn't whole: deleted, not packed", path.display());
                store::cachefile::discard(&path);
                continue;
            }
            Some(b)
        } else {
            None
        };
        p.add(z, x, y, png.as_deref())?;
        packed.push(path);
    }
    // (The last tiles' archives put on the NAS before they count as packed.)
    p.flush()?;
    progress("raw tiles", n, n);
    let added = p.added;
    let areas = p.finish_with(progress)?;
    // (Not one a job reads now: store::cachefile; packed again next time, the same bytes.)
    for path in &packed {
        store::cachefile::try_remove(path);
    }
    if !areas.is_empty() {
        eprintln!("rawpack: {added} tiles packed onto the NAS ({} area{})", areas.len(), if areas.len() == 1 { "" } else { "s" });
    }
    Ok(packed.len())
}

/// What `pack_tar` packed: the files in the stream; the tiles in them, those new to their archives,
/// and those left out (not whole PNGs: taken again from AWS when needed); and the other files.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Packed {
    pub files: usize,
    pub tiles: usize,
    pub added: usize,
    pub broken: usize,
    pub other: usize,
}

/// Packs the tiles of a tar stream (the NAS's own, sent by `tar` over SSH: tools/nas/raw-pack.sh)
/// into archives made in `dir` (gone from it once on the NAS). `expect`: the files listed for it,
/// an error after when it held fewer (those packed are named; a run again packs the rest).
pub fn pack_tar(input: impl Read, dir: &Path, store: &Path, root: &Path, expect: Option<usize>) -> Result<Packed> {
    let mut p = Packer::new(dir, store, root)?;
    p.keep = false;
    let mut r = Packed::default();
    for entry in Tar::new(input) {
        let (name, data) = entry?;
        r.files += 1;
        let Some((z, x, y, has)) = parse_tile(&name) else {
            r.other += 1;
            continue;
        };
        if has && !crate::whole::png_whole(&data) {
            eprintln!("rawpack: {name} on the NAS isn't whole: left out");
            r.broken += 1;
            continue;
        }
        p.add(z, x, y, has.then_some(&data[..]))?;
        r.tiles += 1;
        if r.tiles % 20_000 == 0 {
            eprintln!("rawpack: {} tiles read", r.tiles);
        }
    }
    r.added = p.added;
    let areas = p.finish()?;
    eprintln!("rawpack: {} tiles, {} new to their archives ({} areas), {} left out, {} other files", r.tiles, r.added, areas.len(), r.broken, r.other);
    if let Some(n) = expect {
        ensure!(r.files == n, "the stream held {} of the {n} files listed: those packed are named; run it again for the rest", r.files);
    }
    ensure!(r.other == 0, "{} files in the stream weren't tiles (a name misread?)", r.other);
    Ok(r)
}

/// Paths of tiles (`./<z>/<x>/<y>.png`, a line each), in their areas' order, for a tar stream that
/// `pack_tar` then packs an area once; lines that aren't tiles, after. Those `have` says are packed
/// already are left out, so a run again (one stopped partway) streams only what's left.
pub fn order(paths: &str, have: impl Fn(&str, u64) -> bool) -> String {
    let mut v: Vec<(Option<String>, &str)> = paths
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| match parse_tile(l) {
            Some((z, x, y, _)) => {
                let a = area(z, x, y);
                (!have(&a, tile_key(z, x, y))).then_some((Some(a), l))
            }
            None => Some((None, l)),
        })
        .collect();
    v.sort_by(|a, b| (a.0.is_none(), &a.0, a.1).cmp(&(b.0.is_none(), &b.0, b.1)));
    v.into_iter().map(|(_, l)| format!("{l}\n")).collect()
}

/// The tiles the store's archives hold, by area (their entries read, not the archives).
pub fn packed(store: &Path) -> Result<std::collections::HashMap<String, HashSet<u64>>> {
    let index = Index::load(store)?;
    let mut out = std::collections::HashMap::new();
    for (area, packs) in &index.areas {
        let mut keys = HashSet::new();
        for p in packs {
            match entries_of(&store.join("packs").join(&p.name)) {
                Ok((_, e)) => keys.extend(e.iter().map(|e| e.key)),
                // (Named but gone: its tiles are streamed again, as a packer passes it over.)
                Err(e) if not_found(&e) => eprintln!("rawpack: {} is named but not on the NAS: its tiles count as not packed", p.name),
                Err(e) => return Err(e),
            }
        }
        out.insert(area.clone(), keys);
    }
    Ok(out)
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
            // (The stream ends with a block of zeros: one cut short before it is an error, not the
            // end, so a pack never takes part of one for the whole.)
            let mut h = [0u8; 512];
            if let Err(e) = self.r.read_exact(&mut h) {
                self.done = true;
                return Some(Err(anyhow::Error::from(e).context("the tar stream ended before its end")));
            }
            if h.iter().all(|&b| b == 0) {
                self.done = true;
                return None;
            }
            let field = |a: usize, b: usize| String::from_utf8_lossy(&h[a..b]).trim_end_matches('\0').to_string();
            let sum: u64 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { b as u64 }).sum();
            let size = match (u64::from_str_radix(field(124, 136).trim(), 8), u64::from_str_radix(field(148, 156).trim_matches(|c| c == ' ' || c == '\0'), 8)) {
                (Ok(s), Ok(c)) if c == sum && s <= 1 << 30 => s,
                _ => {
                    self.done = true;
                    return Some(Err(anyhow::anyhow!("a garbled tar header (its size or checksum)")));
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
                // (A pax header: its `path`, the next file's name.)
                b'x' => {
                    let mut rest = &data[..];
                    while let Some(sp) = rest.iter().position(|&b| b == b' ') {
                        let Some(len) = std::str::from_utf8(&rest[..sp]).ok().and_then(|l| l.parse::<usize>().ok()).filter(|&l| l > sp && l <= rest.len()) else { break };
                        if let Some(path) = rest[sp + 1..len].strip_prefix(b"path=") {
                            self.long = Some(String::from_utf8_lossy(path).trim_end_matches('\n').to_string());
                        }
                        rest = &rest[len..];
                    }
                    continue;
                }
                b'0' | 0 => {
                    let name = self.long.take().unwrap_or_else(|| {
                        let (prefix, name) = (field(345, 500), field(0, 100));
                        if &h[257..263] == b"ustar\0" && !prefix.is_empty() { format!("{prefix}/{name}") } else { name }
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

/// What `check` found.
#[derive(Debug, Default)]
pub struct Checked {
    pub archives: usize,
    pub tiles: usize,
    /// Archives that aren't what their names say, or can't be read.
    pub bad: Vec<String>,
    /// Tiles matched against the NAS's loose copies, and those that differ.
    pub sampled: usize,
    pub differ: Vec<String>,
}

/// Checks the NAS's archives: each the index names, read whole and matched against its name; and
/// one tile in `every` of each, matched against the NAS's loose copy while that's there (0: none).
pub fn check(store: &Path, every: usize) -> Result<Checked> {
    let index = Index::load(store)?;
    let mut c = Checked::default();
    for p in index.areas.values().flatten() {
        c.archives += 1;
        let at = store.join("packs").join(&p.name);
        match named_right(&at, &p.name).and_then(|ok| Ok((ok, entries_of(&at)?))) {
            Ok((true, (f, entries))) => {
                c.tiles += entries.len();
                for e in entries.iter().step_by(every.max(1)).take(if every == 0 { 0 } else { usize::MAX }) {
                    let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & 0x1fff_ffff) as u32, (e.key & 0x1fff_ffff) as u32);
                    let mut b = vec![0u8; e.len as usize];
                    f.read_exact_at(&mut b, e.offset)?;
                    let loose = if b.is_empty() { store.join(format!("{z}/{x}/{y}.none")) } else { store.join(format!("{z}/{x}/{y}.png")) };
                    match std::fs::read(&loose) {
                        Ok(l) => {
                            c.sampled += 1;
                            if l != b {
                                c.differ.push(format!("{z}/{x}/{y} in {}", p.name));
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e).with_context(|| format!("read {}", loose.display())),
                    }
                }
            }
            Ok((false, _)) => c.bad.push(format!("{}: not what its name says", p.name)),
            Err(e) => c.bad.push(format!("{}: {e:#}", p.name)),
        }
    }
    Ok(c)
}

/// An error unless `p` is an archive of raw tiles (for checks).
pub fn check_archive(p: &Path) -> Result<usize> {
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

    /// A NAS (its store) and a cache, with the build's folder.
    fn nas(d: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let (root, dir) = (d.join("nas"), d.join("cache"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        (root.join("sources/aws-terrarium"), dir, root)
    }

    /// A tile AWS gave, waiting in a cache for `age` seconds.
    fn put(dir: &Path, rel: &str, b: &[u8], age: u64) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b).unwrap();
        std::fs::File::options().append(true).open(&p).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(age)).unwrap();
    }

    fn names(store: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(store.join("packs")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".tiles")).collect();
        v.sort();
        v
    }

    #[test]
    fn tiles_go_up_packed_by_area_and_come_back() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        // Tiles AWS gave, waiting here (a minute old), one it hasn't, and one too new to pack.
        put(&dir, "12/2048/1365.png", &png(), 120);
        put(&dir, "12/2049/1365.none", b"", 120);
        put(&dir, "8/128/85.png", &png(), 120);
        put(&dir, "2/1/1.png", &png(), 120);
        put(&dir, "12/2050/1366.png", &png(), 1);
        assert_eq!(pack_local(&dir, &store, &root, true).unwrap(), 4);
        let index = Index::load(&store).unwrap();
        // 12/2048/1365 is under z6 tile 32/21; 8/128/85 under z3 tile 4/2; 2/1/1 above z3.
        assert_eq!(index.areas.keys().collect::<Vec<_>>(), ["3-4-2", "6-32-21", "root"]);
        assert!(index.gone.is_empty(), "named, so off the list");
        for p in index.areas.values().flatten() {
            assert!(store.join("packs").join(&p.name).exists() && dir.join("packs").join(&p.name).exists());
            assert_eq!(std::fs::metadata(store.join("packs").join(&p.name)).unwrap().len(), p.bytes);
        }
        assert!(!dir.join("12/2048/1365.png").exists() && dir.join("12/2050/1366.png").exists(), "packed tiles go; the newest waits");
        let a = Archive::open(&dir.join("packs").join(&index.of("6-32-21")[0].name)).unwrap();
        assert_eq!(a.get(12, 2048, 1365), Some(&png()[..]));
        assert_eq!(a.get(12, 2049, 1365), Some(&[][..]), "AWS hasn't it: an empty entry");
        assert_eq!(a.get(12, 2050, 1366), None);
        assert_eq!(check_archive(&dir.join("packs").join(&index.of("3-4-2")[0].name)).unwrap(), 1);
        // A fresh cache takes an archive from the NAS whole, checked against its name.
        let fresh = d.path().join("fresh");
        let name = &index.of("6-32-21")[0].name;
        let local = local_copy(&fresh, &store, name).unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), std::fs::read(store.join("packs").join(name)).unwrap());
        // One changed on the NAS is refused, and leaves nothing here.
        let other = &index.of("root")[0].name;
        let mut b = std::fs::read(store.join("packs").join(other)).unwrap();
        *b.last_mut().unwrap() ^= 1;
        std::fs::write(store.join("packs").join(other), b).unwrap();
        assert!(local_copy(&fresh, &store, other).is_err());
        assert_eq!(std::fs::read_dir(fresh.join("packs")).unwrap().count(), 1);
        // The check finds it too, and matches the rest against the loose tiles they came from.
        let c = check(&store, 1).unwrap();
        assert_eq!((c.archives, c.tiles, c.bad.len()), (3, 3, 1));
    }

    #[test]
    fn a_helpers_archives_go_up_and_are_named_from_its_hand_off() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        let outbox = d.path().join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        // The NAS's store names one area's archive already; a helper fetched more of it, and another.
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let before = Index::load(&store).unwrap();
        put(&dir, "12/2048/1366.png", &png(), 120);
        put(&dir, "12/2049/1365.png", &png(), 120);
        put(&dir, "8/128/85.png", &png(), 120);
        assert_eq!(pack_local_to(&dir, &store, &root, true, Some(outbox.clone()), &|_, _, _| {}).unwrap(), 3);
        // Its archives are on the NAS, none kept here, the index as it was, the loose tiles gone.
        assert_eq!(Index::load(&store).unwrap(), before);
        assert!(!dir.join("12/2048/1366.png").exists());
        let handed: Vec<crate::handoff::Handoff> = crate::handoff::written_in(&outbox).unwrap().unwrap();
        let raw: Vec<(String, Pack)> = handed.into_iter().flat_map(|h| h.raw).collect();
        assert_eq!(raw.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["3-4-2", "6-32-21"]);
        for (area, p) in &raw {
            assert!(there(&store, p) && named_for(&p.name, area), "{}", p.name);
            assert!(!dir.join("packs").join(&p.name).exists());
        }
        // The build Mac names them; a missing one isn't.
        let lock = crate::out::BuildLock::take(&root).unwrap();
        let ghost = ("6-1-1".to_string(), Pack { name: "6-1-1.0123456789abcdef.tiles".into(), bytes: 9 });
        assert_eq!(name_handed(&store, &[raw.clone(), vec![ghost]].concat(), &lock).unwrap().new, 2);
        drop(lock);
        let ix = Index::load(&store).unwrap();
        assert_eq!(ix.of("6-32-21").len(), 2);
        assert!(ix.of("6-1-1").is_empty());
        let a = Archive::open(&store.join("packs").join(&ix.of("6-32-21")[1].name)).unwrap();
        assert_eq!((a.get(12, 2048, 1366), a.get(12, 2049, 1365), a.get(12, 2048, 1365)), (Some(&png()[..]), Some(&png()[..]), None));
        assert!(!named_for("6-32-21.0123456789ABCDEF.tiles", "6-32-21") && !named_for("6-32-2.0123456789abcdef.tiles", "6-32-21"));
    }

    #[test]
    fn a_lost_index_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let named = Index::load(&store).unwrap().of("6-32-21")[0].name.clone();
        // Its file gone (moved aside, say), the archives there: nothing packed, named or swept.
        std::fs::rename(store.join(INDEX), d.path().join("index.json.aside")).unwrap();
        put(&dir, "12/2050/1365.png", &png(), 120);
        assert!(pack_local(&dir, &store, &root, true).unwrap_err().to_string().contains("is missing, but its archives are there"));
        let lock = crate::out::BuildLock::take(&root).unwrap();
        assert!(name_handed(&store, &[("6-32-21".into(), Pack { name: "6-32-21.0123456789abcdef.tiles".into(), bytes: 1 })], &lock).is_err());
        drop(lock);
        assert!(!store.join(INDEX).exists() && store.join("packs").join(&named).exists());
        // Put back: packing goes on (the area's archives merged as they grow).
        std::fs::rename(d.path().join("index.json.aside"), store.join(INDEX)).unwrap();
        assert_eq!(pack_local(&dir, &store, &root, true).unwrap(), 1);
        let ix = Index::load(&store).unwrap();
        assert!(ix.of("6-32-21").iter().any(|p| p.name == named) || ix.gone.contains_key(&named));
    }

    #[test]
    fn a_helpers_first_archives_make_the_index() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        let outbox = d.path().join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        // A store with no index yet: a helper's archives are its first.
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local_to(&dir, &store, &root, true, Some(outbox.clone()), &|_, _, _| {}).unwrap();
        assert!(!store.join(INDEX).exists());
        let raw: Vec<(String, Pack)> = crate::handoff::written_in(&outbox).unwrap().unwrap().into_iter().flat_map(|h| h.raw).collect();
        let lock = crate::out::BuildLock::take(&root).unwrap();
        assert_eq!(name_handed(&store, &raw, &lock).unwrap().new, 1);
        drop(lock);
        assert_eq!(Index::load(&store).unwrap().of(&raw[0].0).len(), 1);
    }

    #[test]
    fn a_handed_archive_of_another_areas_tiles_isnt_named() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        let outbox = d.path().join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        // (The build Mac's store has its index: one missing while there are archives isn't written.)
        put(&dir, "8/128/85.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local_to(&dir, &store, &root, true, Some(outbox.clone()), &|_, _, _| {}).unwrap();
        let raw: Vec<(String, Pack)> = crate::handoff::written_in(&outbox).unwrap().unwrap().into_iter().flat_map(|h| h.raw).collect();
        // The same archive said to be another area's (renamed to match): not named; as its own, named.
        let (area, p) = raw[0].clone();
        let other = Pack { name: p.name.replacen(&area, "6-33-21", 1), bytes: p.bytes };
        std::fs::copy(store.join("packs").join(&p.name), store.join("packs").join(&other.name)).unwrap();
        let lock = crate::out::BuildLock::take(&root).unwrap();
        assert_eq!(name_handed(&store, &[("6-33-21".into(), other)], &lock).unwrap().new, 0);
        assert_eq!(name_handed(&store, &[(area.clone(), p)], &lock).unwrap().new, 1);
        drop(lock);
        assert!(Index::load(&store).unwrap().of("6-33-21").is_empty());
    }

    #[test]
    fn an_archive_no_one_names_goes_a_day_later() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        // A helper's archive whose hand-off never came: a day old, and a fresh one.
        let (stale, fresh) = (store.join("packs/6-1-1.0123456789abcdef.tiles"), store.join("packs/6-1-2.0123456789abcdef.tiles"));
        std::fs::write(&stale, b"x").unwrap();
        std::fs::write(&fresh, b"y").unwrap();
        std::fs::File::options().append(true).open(&stale).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(GRACE + 60)).unwrap();
        // The next packing lists the old one to go, not the new one, nor the named.
        put(&dir, "12/2050/1366.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let ix = Index::load(&store).unwrap();
        assert!(ix.gone.contains_key("6-1-1.0123456789abcdef.tiles"));
        assert!(!ix.gone.contains_key("6-1-2.0123456789abcdef.tiles"));
        assert!(ix.of("6-32-21").iter().all(|p| !ix.gone.contains_key(&p.name)));
    }

    #[test]
    fn packing_says_how_far_it_is() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        put(&dir, "12/2048/1365.png", &png(), 120);
        put(&dir, "12/2049/1365.none", b"", 120);
        put(&dir, "8/128/85.png", &png(), 120);
        let said = std::sync::Mutex::new(Vec::new());
        assert_eq!(pack_local_with(&dir, &store, &root, true, &|w, d, t| said.lock().unwrap().push((w.to_string(), d, t))).unwrap(), 3);
        let said = said.into_inner().unwrap();
        // The tiles from none to all, then the two areas' merging, done.
        assert_eq!(said.first(), Some(&("raw tiles".to_string(), 0, 3)));
        assert!(said.contains(&("raw tiles".to_string(), 3, 3)));
        assert_eq!(said.last(), Some(&("areas".to_string(), 2, 2)));
        assert!(said.iter().all(|(_, d, t)| d <= t));
        let at = |w: &str| said.iter().position(|s| s.0 == w).unwrap();
        assert!(at("raw tiles") < at("areas"));
    }

    #[test]
    fn new_tiles_go_up_beside_an_areas_archive_and_merge_when_due() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        // An area's archive of twenty tiles.
        for y in 0..20 {
            put(&dir, &format!("12/2048/{}.png", 1344 + y), &png(), 120);
        }
        pack_local(&dir, &store, &root, true).unwrap();
        let base = Index::load(&store).unwrap().of("6-32-21").to_vec();
        assert_eq!(base.len(), 1);
        // One more: an archive of its own beside it; the first isn't written again.
        put(&dir, "12/2049/1344.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let two = Index::load(&store).unwrap().of("6-32-21").to_vec();
        assert_eq!((two.len(), &two[0]), (2, &base[0]));
        assert!(two[1].bytes * 2 < two[0].bytes);
        // Ten more: together with the one before, half the first's size and more: all merged.
        for y in 0..10 {
            put(&dir, &format!("12/2050/{}.png", 1344 + y), &png(), 120);
        }
        pack_local(&dir, &store, &root, true).unwrap();
        let index = Index::load(&store).unwrap();
        let merged = index.of("6-32-21");
        assert_eq!(merged.len(), 1);
        let a = Archive::open(&store.join("packs").join(&merged[0].name)).unwrap();
        assert_eq!(a.entries().len(), 31);
        // The merged archives stay on the NAS a day (listed as gone), not here.
        assert_eq!(index.gone.len(), 3);
        for p in two.iter() {
            assert!(index.gone.contains_key(&p.name) && store.join("packs").join(&p.name).exists() && !dir.join("packs").join(&p.name).exists());
        }
        assert_eq!(names(&store).len(), 4);
    }

    #[test]
    fn archives_listed_as_gone_go_a_day_later() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let named = Index::load(&store).unwrap().of("6-32-21")[0].name.clone();
        // One listed two days ago, one an hour ago, and the named one listed too (put up again).
        std::fs::create_dir_all(store.join("packs")).unwrap();
        for n in ["6-1-1.0000000000000001.tiles", "6-1-1.0000000000000002.tiles"] {
            std::fs::write(store.join("packs").join(n), b"x").unwrap();
        }
        let mut ix = Index::load(&store).unwrap();
        let now = unix_now();
        ix.gone.insert("6-1-1.0000000000000001.tiles".into(), now - 2 * 86400);
        ix.gone.insert("6-1-1.0000000000000002.tiles".into(), now - 3600);
        ix.gone.insert(named.clone(), now - 2 * 86400);
        ix.save(&store).unwrap();
        let mut p = Packer::new(&dir, &store, &root).unwrap();
        p.commit(|_| {}).unwrap();
        let ix = Index::load(&store).unwrap();
        assert_eq!(ix.gone.keys().collect::<Vec<_>>(), ["6-1-1.0000000000000002.tiles"]);
        assert!(!store.join("packs/6-1-1.0000000000000001.tiles").exists());
        assert!(store.join("packs").join(&named).exists(), "named: kept");
    }

    #[test]
    fn a_merge_another_packer_made_first_is_dropped() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        // Two archives for an area, due merging: the second put up and named, not yet merged.
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let mut p = Packer::new(&dir, &store, &root).unwrap();
        p.add(12, 2048, 1366, Some(&png())).unwrap();
        p.flush().unwrap();
        assert_eq!(p.index.of("6-32-21").len(), 2);
        // Another packer merges them meanwhile.
        let mut q = Packer::new(&d.path().join("other"), &store, &root).unwrap();
        q.merge_due("6-32-21").unwrap();
        let theirs = Index::load(&store).unwrap().of("6-32-21").to_vec();
        assert_eq!(theirs.len(), 1);
        // This one, from the index it read before, merges them too: the same tiles make the same
        // archive, found named already; nothing is lost, doubled or deleted.
        p.merge_due("6-32-21").unwrap();
        let ix = Index::load(&store).unwrap();
        assert_eq!(ix.of("6-32-21"), &theirs[..]);
        assert!(!ix.gone.contains_key(&theirs[0].name) && store.join("packs").join(&theirs[0].name).exists());
        assert_eq!(ix.gone.len(), 2);
    }

    #[test]
    fn an_archive_made_again_under_a_name_the_index_has_is_never_deleted() {
        // A job's tiles (taken from the NAS's loose ones) packed while the NAS's own are: the job's
        // archive holds a part of the other's, so merging the two makes the other's again, by name.
        let d = tempfile::tempdir().unwrap();
        let (store, _, root) = nas(d.path());
        let mut all = Packer::new(&d.path().join("a"), &store, &root).unwrap();
        let mut part = Packer::new(&d.path().join("b"), &store, &root).unwrap();
        for y in 0..4 {
            all.add(12, 2048, 1344 + y, Some(&png())).unwrap();
        }
        part.add(12, 2048, 1344, Some(&png())).unwrap();
        part.finish().unwrap();
        let first = Index::load(&store).unwrap().of("6-32-21").to_vec();
        all.finish().unwrap();
        let ix = Index::load(&store).unwrap();
        for p in ix.areas.values().flatten() {
            assert!(store.join("packs").join(&p.name).exists(), "{} is named and there", p.name);
        }
        let l = ix.of("6-32-21");
        assert_eq!((l.len(), Archive::open(&store.join("packs").join(&l[0].name)).unwrap().entries().len()), (1, 4));
        assert_eq!(ix.gone.keys().collect::<Vec<_>>(), [&first[0].name]);
    }

    #[test]
    fn a_job_that_outlived_its_index_reads_on() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        // A packer begun now, and the area's archive merged away (and gone from the NAS, as a day
        // later) before it reaches the area: it reads the index again.
        let mut p = Packer::new(&d.path().join("late"), &store, &root).unwrap();
        put(&dir, "12/2048/1366.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let ix = Index::load(&store).unwrap();
        for n in ix.gone.keys() {
            std::fs::remove_file(store.join("packs").join(n)).unwrap();
        }
        assert!(!p.add(12, 2048, 1365, Some(&png())).unwrap(), "known, from the index read again");
        assert!(p.add(12, 2048, 1367, Some(&png())).unwrap());
        p.finish().unwrap();
        let l = Index::load(&store).unwrap().of("6-32-21").to_vec();
        let tiles: usize = l.iter().map(|p| Archive::open(&store.join("packs").join(&p.name)).unwrap().entries().len()).sum();
        assert_eq!(tiles, 3);
    }

    #[test]
    fn a_tar_stream_of_the_nas_tiles_is_packed() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        // A tar of a store's loose tiles, as `tar -cf -` makes it (with one cut short; and another
        // with an odd file among them).
        let src = d.path().join("src");
        let half = png()[..png().len() / 2].to_vec();
        for (rel, b) in [("12/2048/1365.png", png()), ("12/2049/1365.none", Vec::new()), ("9/256/170.png", png()), ("9/256/171.png", half), ("notes.txt", b"x".to_vec())] {
            std::fs::create_dir_all(src.join(rel).parent().unwrap()).unwrap();
            std::fs::write(src.join(rel), b).unwrap();
        }
        let tar = std::process::Command::new("tar").env("COPYFILE_DISABLE", "1").args(["--no-mac-metadata", "--no-xattrs", "-cf", "-", "-C"]).arg(&src).args(["./12/2048/1365.png", "./12/2049/1365.none", "./9/256/170.png", "./9/256/171.png"]).output().unwrap();
        assert!(tar.status.success());
        let odd = std::process::Command::new("tar").env("COPYFILE_DISABLE", "1").args(["--no-mac-metadata", "--no-xattrs", "-cf", "-", "-C"]).arg(&src).args(["./notes.txt"]).output().unwrap();
        assert!(pack_tar(&odd.stdout[..], &dir, &store, &root, Some(1)).is_err(), "a file that isn't a tile: a name misread");
        // Cut short (in a header, in a tile): an error, nothing named.
        for cut in [512 * 3 + 100, 700] {
            assert!(pack_tar(&tar.stdout[..cut], &dir, &store, &root, None).is_err());
        }
        assert!(!store.join(INDEX).exists());
        // A garbled header: an error too.
        let mut bad = tar.stdout.clone();
        bad[0] ^= 1;
        assert!(pack_tar(&bad[..], &dir, &store, &root, None).is_err());
        assert_eq!(pack_tar(&tar.stdout[..], &dir, &store, &root, Some(4)).unwrap(), Packed { files: 4, tiles: 3, added: 3, broken: 1, other: 0 });
        let index = Index::load(&store).unwrap();
        assert_eq!(index.areas.keys().collect::<Vec<_>>(), ["6-32-21"]);
        let a = Archive::open(&store.join("packs").join(&index.of("6-32-21")[0].name)).unwrap();
        assert_eq!(a.get(9, 256, 170), Some(&png()[..]));
        assert_eq!(a.get(12, 2049, 1365), Some(&[][..]));
        assert_eq!(std::fs::read_dir(dir.join("packs")).unwrap().count(), 0, "nothing kept here");
        // Again: nothing new, the same archive; fewer files than listed: an error.
        assert_eq!(pack_tar(&tar.stdout[..], &dir, &store, &root, None).unwrap(), Packed { files: 4, tiles: 3, added: 0, broken: 1, other: 0 });
        assert!(pack_tar(&tar.stdout[..], &dir, &store, &root, Some(5)).is_err());
        assert_eq!(Index::load(&store).unwrap(), index);
        // The archives match the loose tiles (here, the source folder as the store).
        std::fs::create_dir_all(src.join("packs")).unwrap();
        for e in std::fs::read_dir(store.join("packs")).unwrap() {
            let e = e.unwrap();
            store::sys::copy_data(e.path(), src.join("packs").join(e.file_name())).unwrap();
        }
        let c = check(&src, 1).unwrap();
        assert_eq!((c.archives, c.tiles, c.sampled, c.bad.len(), c.differ.len()), (1, 3, 3, 0, 0));
    }

    #[test]
    fn two_packers_at_once_keep_each_others_tiles() {
        let d = tempfile::tempdir().unwrap();
        let (store, _, root) = nas(d.path());
        // Both begin from the same (empty) index for one area; each adds its own tile.
        let mut a = Packer::new(&d.path().join("a"), &store, &root).unwrap();
        let mut b = Packer::new(&d.path().join("b"), &store, &root).unwrap();
        a.add(12, 2048, 1365, Some(&png())).unwrap();
        b.add(12, 2048, 1366, Some(&png())).unwrap();
        a.finish().unwrap();
        b.finish().unwrap();
        // The second's archive went up beside the first's, and the two merged: both tiles there.
        let index = Index::load(&store).unwrap();
        let l = index.of("6-32-21");
        assert_eq!(l.len(), 1);
        let arch = Archive::open(&store.join("packs").join(&l[0].name)).unwrap();
        assert!(arch.get(12, 2048, 1365).is_some() && arch.get(12, 2048, 1366).is_some());
        // On the NAS, each archive is named or listed as gone.
        for n in names(&store) {
            assert!(index.names(&n) || index.gone.contains_key(&n), "{n}");
        }
    }

    #[test]
    fn the_newest_archives_merge_while_they_add_up() {
        let l = |s: &[u64]| s.iter().map(|&b| Pack { name: String::new(), bytes: b }).collect::<Vec<_>>();
        assert_eq!(due(&l(&[])), None);
        assert_eq!(due(&l(&[100])), None);
        assert_eq!(due(&l(&[100, 10])), None);
        assert_eq!(due(&l(&[100, 10, 10])), Some(1));
        assert_eq!(due(&l(&[100, 60])), Some(0));
        assert_eq!(due(&l(&[100, 40, 20])), Some(0));
        assert_eq!(due(&l(&[100, 30, 10])), None);
        assert_eq!(due(&l(&[100, 45, 20])), Some(0), "the first isn't more than twice the rest");
        assert_eq!(due(&l(&[1000, 100, 45, 20])), Some(1));
    }

    #[test]
    fn an_archive_is_named_only_once_its_on_the_nas_whole() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        let mut p = Packer::new(&dir, &store, &root).unwrap();
        let pack = p
            .write("6-32-21", 1, |_, b| {
                b.clear();
                b.extend_from_slice(&png());
                Ok(tile_key(12, 2048, 1365))
            })
            .unwrap();
        // (As if cut short there: a size the NAS's copy won't have.)
        let cut = Pack { bytes: pack.bytes + 1, ..pack.clone() };
        assert!(p.put_up(vec![("6-32-21".into(), cut)]).is_err());
        let ix = Index::load(&store).unwrap();
        assert!(ix.areas.is_empty() && ix.gone.contains_key(&pack.name), "listed as gone, not named");
    }

    #[test]
    fn an_archive_named_but_gone_from_the_nas_is_passed_over_and_dropped() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let lost = Index::load(&store).unwrap().of("6-32-21")[0].name.clone();
        std::fs::remove_file(store.join("packs").join(&lost)).unwrap();
        std::fs::remove_file(dir.join("packs").join(&lost)).unwrap();
        // More of that area packs, and the index stops naming the lost archive.
        put(&dir, "12/2048/1366.png", &png(), 120);
        assert_eq!(pack_local(&dir, &store, &root, true).unwrap(), 1);
        let ix = Index::load(&store).unwrap();
        assert!(!ix.names(&lost));
        assert_eq!(ix.of("6-32-21").len(), 1);
    }

    #[test]
    fn a_pax_streams_long_names_are_read_whole() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        let src = d.path().join("src");
        let name = format!("12/2048/{:0>120}.png", 1365);
        std::fs::create_dir_all(src.join("12/2048")).unwrap();
        std::fs::write(src.join(&name), png()).unwrap();
        let tar = std::process::Command::new("tar").env("COPYFILE_DISABLE", "1").args(["--no-mac-metadata", "--no-xattrs", "--format", "pax", "-cf", "-", "-C"]).arg(&src).arg(format!("./{name}")).output().unwrap();
        assert!(tar.status.success());
        let r = pack_tar(&tar.stdout[..], &dir, &store, &root, Some(1)).unwrap();
        assert_eq!((r.tiles, r.other), (1, 0));
        let a = Archive::open(&store.join("packs").join(&Index::load(&store).unwrap().of("6-32-21")[0].name)).unwrap();
        assert_eq!(a.get(12, 2048, 1365), Some(&png()[..]));
    }

    #[test]
    fn only_the_build_mac_packs() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        std::fs::write(root.join("state/build/writer"), "another-mac\n").unwrap();
        put(&dir, "12/2048/1365.png", &png(), 120);
        if std::env::var_os("SCENIC_BUILD_MAC").is_none() {
            assert!(pack_local(&dir, &store, &root, true).is_err());
            assert!(dir.join("12/2048/1365.png").exists() && !store.exists());
        }
    }

    #[test]
    fn a_stream_in_area_order() {
        let list = "./12/2049/1365.png\n./notes.txt\n./8/128/85.png\n./12/2048/1365.png\n./2/1/1.png\n";
        assert_eq!(order(list, |_, _| false), "./8/128/85.png\n./12/2048/1365.png\n./12/2049/1365.png\n./2/1/1.png\n./notes.txt\n");
        // What's packed already is left out.
        assert_eq!(order(list, |a, k| a == "6-32-21" && k == tile_key(12, 2048, 1365)), "./8/128/85.png\n./12/2049/1365.png\n./2/1/1.png\n./notes.txt\n");
    }

    #[test]
    fn a_run_again_streams_only_whats_left() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        put(&dir, "12/2048/1365.png", &png(), 120);
        pack_local(&dir, &store, &root, true).unwrap();
        let have = packed(&store).unwrap();
        let left = order("./12/2048/1365.png\n./12/2048/1366.png\n", |a, k| have.get(a).is_some_and(|s| s.contains(&k)));
        assert_eq!(left, "./12/2048/1366.png\n");
    }

    #[test]
    fn a_big_pack_goes_up_as_it_goes() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        let mut p = Packer::new(&dir, &store, &root).unwrap();
        p.add(12, 2048, 1365, Some(&png())).unwrap();
        p.add(12, 2048, 1366, Some(&png())).unwrap();
        assert!(!store.join(INDEX).exists());
        // (Past the area with the spool full, as if: what's spooled goes up, the spool starts again.)
        p.pos = GROUP;
        p.add(12, 4096, 1365, Some(&png())).unwrap();
        assert_eq!(Index::load(&store).unwrap().areas.keys().collect::<Vec<_>>(), ["6-32-21"]);
        assert_eq!(p.pos, png().len() as u64);
        p.finish().unwrap();
        assert_eq!(Index::load(&store).unwrap().areas.len(), 2);
    }

    #[test]
    fn a_tile_both_aws_and_none_is_aws() {
        let d = tempfile::tempdir().unwrap();
        let (store, dir, root) = nas(d.path());
        let mut p = Packer::new(&dir, &store, &root).unwrap();
        assert!(p.add(12, 2048, 1365, None).unwrap());
        assert!(p.add(12, 2048, 1365, Some(&png())).unwrap());
        assert!(!p.add(12, 2048, 1365, None).unwrap());
        assert_eq!(p.added, 1);
        p.finish().unwrap();
        let ix = Index::load(&store).unwrap();
        let a = Archive::open(&store.join("packs").join(&ix.of("6-32-21")[0].name)).unwrap();
        assert_eq!(a.get(12, 2048, 1365), Some(&png()[..]));
    }

    #[test]
    fn tiles_are_named_by_their_place() {
        assert_eq!(parse_tile("./12/690/949.png"), Some((12, 690, 949, true)));
        assert_eq!(parse_tile("8/1/2.none"), Some((8, 1, 2, false)));
        assert_eq!(parse_tile("8/1/2.png.tmp"), None);
        assert_eq!(parse_tile("@eaDir/8/1/2.png"), None);
        assert_eq!(area(12, 690, 949), "6-10-14");
        assert_eq!(area(9, 5, 7), "6-0-0");
        assert_eq!(area(8, 128, 85), "3-4-2");
        assert_eq!(area(3, 4, 2), "3-4-2");
        assert_eq!(area(5, 3, 3), "3-0-0");
        assert_eq!(area(2, 1, 1), "root");
    }
}
