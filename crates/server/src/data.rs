//! The map's data as the server sees it (docs/storage.md–4): the current catalog, every file it
//! names read from this Mac's mirror when it's there (memory-mapped) or from the NAS (through the
//! bounded I/O pool, never mapped), and the background work that keeps all that current: finding
//! and mounting the NAS, following new catalogs, filling the mirror.

use crate::views::{BaseView, Blob, HiView, RoadUnits, SectView, Src};
use anyhow::{anyhow, Context, Result};
use memmap2::Mmap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use store::catalog::Catalog;
use store::iopool::IoPool;
use store::pack::PackIndex;

/// The NAS share and the project folder on it.
pub const NAS_HOST: &str = store::nas::HOST;
pub const NAS_SHARE: &str = store::nas::SHARE;
pub const PROJECT: &str = store::nas::PROJECT;

/// A bounded cache (least recently inserted out first), by entries and by bytes: entries read
/// from the NAS weigh their size in memory, mapped local files nothing (the OS pages them).
struct Bounded<V> {
    map: HashMap<String, (V, u64)>,
    order: std::collections::VecDeque<String>,
    cap: usize,
    bytes: u64,
    cap_bytes: u64,
}

impl<V: Clone> Bounded<V> {
    fn new(cap: usize) -> Self {
        Self::with_bytes(cap, u64::MAX)
    }
    fn with_bytes(cap: usize, cap_bytes: u64) -> Self {
        Bounded { map: HashMap::new(), order: Default::default(), cap, bytes: 0, cap_bytes }
    }
    fn get(&self, k: &str) -> Option<V> {
        self.map.get(k).map(|(v, _)| v.clone())
    }
    fn put(&mut self, k: String, v: V) {
        self.put_weighed(k, v, 0)
    }
    fn put_weighed(&mut self, k: String, v: V, weight: u64) {
        if let Some((_, w)) = self.map.insert(k.clone(), (v, weight)) {
            self.bytes = self.bytes - w + weight;
            return;
        }
        self.bytes += weight;
        self.order.push_back(k);
        while self.order.len() > self.cap || (self.bytes > self.cap_bytes && self.order.len() > 1) {
            if let Some(old) = self.order.pop_front() {
                if let Some((_, w)) = self.map.remove(&old) {
                    self.bytes -= w;
                }
            }
        }
    }
    /// Drops the entries `drop` picks.
    fn retain(&mut self, mut keep: impl FnMut(&str, &V) -> bool) {
        let gone: Vec<String> = self.map.iter().filter(|(k, (v, _))| !keep(k, v)).map(|(k, _)| k.clone()).collect();
        for k in gone {
            if let Some((_, w)) = self.map.remove(&k) {
                self.bytes -= w;
            }
            self.order.retain(|x| x != &k);
        }
    }
}

/// Memory budgets for what's read from the NAS (a Mac whose mirror isn't complete): sectioned
/// files read whole, and the base views' names. Base packs' and hidata's sections are paged
/// (`pages`, with its own budget).
const BASES_BYTES: u64 = 512 << 20;
const SECTS_BYTES: u64 = 1 << 30;

pub struct Data {
    /// ~/Library/Application Support/scenic
    pub home: PathBuf,
    /// The project folder on the NAS, while the share is mounted.
    nas: RwLock<Option<PathBuf>>,
    pool: RwLock<Option<Arc<IoPool>>>,
    pub mirror: Option<Arc<store::mirror::Mirror>>,
    cat: RwLock<Arc<Catalog>>,
    /// Local maps of mirrored files, and open NAS files, by content name.
    maps: Mutex<Bounded<Arc<Mmap>>>,
    remotes: Mutex<Bounded<Arc<crate::views::RemoteFile>>>,
    indexes: Mutex<Bounded<Arc<PackIndex>>>,
    sects: Mutex<Bounded<Arc<SectView>>>,
    bases: Mutex<Bounded<Arc<BaseView>>>,
    his: Mutex<Bounded<Arc<HiView>>>,
    marks: Mutex<Bounded<Arc<crate::markview::MarkView>>>,
    ovs: Mutex<Bounded<Arc<crate::ovdata::OvView>>>,
    roadunits: Mutex<Option<(String, Arc<RoadUnits>)>>,
    globals: Mutex<Bounded<Arc<Vec<u8>>>>,
    /// Set when the catalog changes, so caches built from the old one are dropped.
    pub generation: std::sync::atomic::AtomicU64,
    /// Set when the mirror has copied files, so archives opened from the NAS are opened again.
    pub mirror_gen: std::sync::atomic::AtomicU64,
    /// The last attempt to mount the share.
    last_mount: Mutex<Option<std::time::Instant>>,
    /// Whether the build agent runs a job, and when that was read.
    busy: Mutex<Option<(std::time::Instant, bool)>>,
    /// A mount by the bare name has been reported (once).
    warned_tunnel: std::sync::atomic::AtomicBool,
    /// Held while a catalog is followed (the background poll and /api/catalog/refresh: one at a time).
    refreshing: Mutex<()>,
}

/// A data development override: serve a local folder laid out like the NAS project folder.
pub struct Options {
    pub home: PathBuf,
    pub nas_root: Option<PathBuf>,
    pub mirror: bool,
    /// The free space the mirror leaves on the disk (bytes).
    pub reserve: u64,
}

impl Data {
    pub fn open(o: Options) -> Result<Arc<Data>> {
        std::fs::create_dir_all(o.home.join("catalog"))?;
        let mirror = if o.mirror {
            let m = store::mirror::Mirror::open(o.home.clone(), o.reserve)?;
            // This Mac's build agent's room target, when above the reserve (docs/macs.md, Mirror).
            let target = pipeline::agent::settings::RoomTarget::new(&o.home.join("agent"));
            m.set_floor(move || target.bytes());
            Some(Arc::new(m))
        } else {
            None
        };
        let d = Arc::new(Data {
            home: o.home.clone(),
            nas: RwLock::new(None),
            pool: RwLock::new(None),
            mirror,
            cat: RwLock::new(Arc::new(Catalog::default())),
            maps: Mutex::new(Bounded::new(4096)),
            remotes: Mutex::new(Bounded::new(128)),
            indexes: Mutex::new(Bounded::new(8192)),
            sects: Mutex::new(Bounded::with_bytes(512, SECTS_BYTES)),
            bases: Mutex::new(Bounded::with_bytes(256, BASES_BYTES)),
            his: Mutex::new(Bounded::new(512)),
            marks: Mutex::new(Bounded::new(4096)),
            ovs: Mutex::new(Bounded::new(512)),
            roadunits: Mutex::new(None),
            globals: Mutex::new(Bounded::new(128)),
            generation: Default::default(),
            mirror_gen: Default::default(),
            last_mount: Mutex::new(None),
            busy: Mutex::new(None),
            warned_tunnel: Default::default(),
            refreshing: Mutex::new(()),
        });
        match o.nas_root {
            Some(root) => d.set_nas(Some(root)),
            None => d.find_nas(),
        }
        // The newest catalog: the NAS's, else the last one this Mac kept.
        if !d.refresh_catalog() || d.catalog().n == 0 {
            let kept = match &d.mirror {
                Some(m) => m.saved_catalog().ok().flatten(),
                None => store::catalog::latest(&o.home.join("catalog")).ok().flatten(),
            };
            if let Some(c) = kept {
                eprintln!("offline: serving catalog {} from this Mac", c.n);
                *d.cat.write().unwrap() = Arc::new(c);
            }
        }
        Ok(d)
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        self.cat.read().unwrap().clone()
    }

    pub fn nas_root(&self) -> Option<PathBuf> {
        self.nas.read().unwrap().clone()
    }

    pub fn pool(&self) -> Option<Arc<IoPool>> {
        self.pool.read().unwrap().clone()
    }

    /// Whether the NAS is reachable now.
    pub fn online(&self) -> bool {
        self.pool().is_some_and(|p| p.is_online())
    }

    fn set_nas(&self, root: Option<PathBuf>) {
        let mut cur = self.nas.write().unwrap();
        if *cur == root {
            return;
        }
        *self.pool.write().unwrap() = root.as_ref().map(|r| IoPool::new(8, Duration::from_secs(8), r.clone()));
        *cur = root;
    }

    /// Find the share (mounting it if it's missing: only when the NAS answers on the SMB port, so
    /// away from home nothing tries, and at most every five minutes).
    pub fn find_nas(&self) {
        let mount = store::nas::find_mount(NAS_HOST, NAS_SHARE);
        if let Some(m) = &mount {
            if !store::nas::by_lan_name(m) && !self.warned_tunnel.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("NAS: mounted as {} (Tailscale's DNS can route that through the tunnel, at a fraction of the LAN's speed); new mounts use {}", m.from, store::nas::LAN_HOST);
            }
        }
        let found = mount.map(|m| m.point.join(PROJECT));
        if found.is_some() {
            self.set_nas(found);
            return;
        }
        {
            let mut last = self.last_mount.lock().unwrap();
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(300)) {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        if !store::nas::at_home() {
            return;
        }
        let url = store::nas::smb_url();
        if let Err(e) = store::nas::mount(url, Duration::from_secs(20)) {
            eprintln!("NAS: can't mount {url}: {e:#}");
        }
        self.set_nas(store::nas::find_mount(NAS_HOST, NAS_SHARE).map(|m| m.point.join(PROJECT)));
    }

    /// Follow a newer catalog on the NAS; true when the NAS was read (whether or not it changed).
    /// From the one served now: unchanged, nothing read but the folder's listing; newer as deltas
    /// (docs/formats.md, Delta catalogs), only those read and applied to it.
    pub fn refresh_catalog(&self) -> bool {
        let _one = self.refreshing.lock().unwrap_or_else(|e| e.into_inner());
        let (Some(root), Some(pool)) = (self.nas_root(), self.pool()) else { return false };
        let have = self.catalog();
        match store::catalog::latest_nas_from(
            &pool,
            &root.join("catalog"),
            Some(&have).filter(|c| c.n > 0).map(|c| c.as_ref()),
        ) {
            Ok(Some(c)) => {
                if c.n != self.catalog().n {
                    eprintln!("catalog {} ({} files)", c.n, c.files.len());
                    // Keep a copy on this Mac for offline starts (and so the mirror protects it).
                    let kept = match &self.mirror {
                        Some(m) => m.save_catalog(&c),
                        None => store::catalog::write_copy(&self.home.join("catalog"), &c).map(|_| ()),
                    };
                    if let Err(e) = kept {
                        eprintln!("catalog: can't keep a local copy: {e:#}");
                    }
                    let n = c.n;
                    *self.cat.write().unwrap() = Arc::new(c);
                    self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    publish_event(&root, "adopt", n, None);
                }
                true
            }
            Ok(None) => true,
            Err(e) => {
                eprintln!("catalog: {e:#}");
                false
            }
        }
    }

    /// A logical name's content name in the current catalog.
    pub fn content(&self, logical: &str) -> Option<String> {
        self.catalog().files.get(logical).map(|f| f.file.clone())
    }

    /// A content-named file: mapped from the mirror, else on the NAS.
    fn src(&self, content: &str) -> Result<Src> {
        if let Some(m) = self.maps.lock().unwrap().get(content) {
            return Ok(Src::Local(m));
        }
        if let Some((mi, p)) = self.mirror.as_ref().and_then(|mi| Some((mi, mi.local(content)?))) {
            match std::fs::File::open(&p) {
                Ok(f) => {
                    // SAFETY: mirrored files are content-named and never modified; removal
                    // unlinks them (and `forget_removed` drops the maps).
                    let m = Arc::new(unsafe { Mmap::map(&f)? });
                    // Kept only while the mirror has the file: a map kept after it let the file go
                    // would hold its room. A removal drops the file from the mirror's list, then
                    // what's cached of it (`forget_removed`): asked again once cached, the list
                    // has dropped it, or the cache's drop is still to come.
                    if mi.has(content) {
                        self.maps.lock().unwrap().put(content.to_string(), m.clone());
                        if !mi.has(content) {
                            self.maps.lock().unwrap().retain(|c, _| c != content);
                        }
                    }
                    return Ok(Src::Local(m));
                }
                // Let go just now: the NAS's copy.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("open {}", p.display())),
            }
        }
        if let Some(r) = self.remotes.lock().unwrap().get(content) {
            return Ok(Src::Remote(r));
        }
        let (Some(root), Some(pool)) = (self.nas_root(), self.pool()) else {
            return Err(anyhow!("{content}: not on this Mac, and the NAS isn't mounted"));
        };
        let r = Arc::new(crate::views::RemoteFile::new(root.join(content), pool));
        self.remotes.lock().unwrap().put(content.to_string(), r.clone());
        Ok(Src::Remote(r))
    }

    /// A sectioned file by logical name.
    pub fn sect(&self, logical: &str) -> Result<Option<Arc<SectView>>> {
        let Some(content) = self.content(logical) else { return Ok(None) };
        if let Some(s) = self.sects.lock().unwrap().get(&content) {
            return Ok(Some(s));
        }
        let s = Arc::new(SectView::open(self.src(&content)?).with_context(|| format!("open {content}"))?);
        let w = s.remote_bytes();
        self.sects.lock().unwrap().put_weighed(content, s.clone(), w);
        Ok(Some(s))
    }

    /// A unit's base pack with its road values ("6/32/21"). Cached by content: a new catalog's
    /// pack for the same unit is another entry.
    pub fn base(&self, unit: &str) -> Result<Option<Arc<BaseView>>> {
        let cat = self.catalog();
        let (Some(b), Some(r)) = (cat.base.get(unit), cat.roads.get(unit)) else { return Ok(None) };
        let (Some(bc), Some(rc)) = (self.content(b), self.content(r)) else { return Ok(None) };
        let key = format!("{bc}|{rc}");
        if let Some(v) = self.bases.lock().unwrap().get(&key) {
            return Ok(Some(v));
        }
        let (bs, rs) = (SectView::open(self.src(&bc)?)?, SectView::open(self.src(&rc)?)?);
        let v = Arc::new(BaseView::new(bs, rs).with_context(|| format!("base pack {bc}"))?);
        self.bases.lock().unwrap().put_weighed(key, v.clone(), v.weight());
        Ok(Some(v))
    }

    /// A z6 tile's hidata ("6/32/21"), cached by content.
    pub fn hidata(&self, tile: &str) -> Result<Option<Arc<HiView>>> {
        let cat = self.catalog();
        let Some(l) = cat.hidata.get(tile) else { return Ok(None) };
        let Some(content) = self.content(l) else { return Ok(None) };
        if let Some(v) = self.his.lock().unwrap().get(&content) {
            return Ok(Some(v));
        }
        let v =
            Arc::new(HiView::new(SectView::open(self.src(&content)?)?).with_context(|| format!("hidata {content}"))?);
        self.his.lock().unwrap().put(content, v.clone());
        Ok(Some(v))
    }

    /// What names a z6 tile's landmark points ("6/32/21") in caches and ETags: its file's content
    /// name, with the tile when the file is the marks job's bundle of every tile (docs/formats.md
    /// "Bundles"; task #449). None when the catalog has none there.
    pub fn markdata_id(&self, tile: &str) -> Option<String> {
        let content = self.content(self.catalog().markdata.get(tile)?)?;
        Some(if is_bundle(&content) { format!("{content}#{tile}") } else { content })
    }

    /// A z6 tile's landmark points ("6/32/21"), cached by `markdata_id`: its own file, or its member
    /// of the bundle (read in place: one range read of the bundle's index, kept as a pack's is).
    pub fn markdata(&self, tile: &str) -> Result<Option<Arc<crate::markview::MarkView>>> {
        let cat = self.catalog();
        let Some(l) = cat.markdata.get(tile) else { return Ok(None) };
        let Some(content) = self.content(l) else { return Ok(None) };
        let id = if is_bundle(&content) { format!("{content}#{tile}") } else { content.clone() };
        if let Some(v) = self.marks.lock().unwrap().get(&id) {
            return Ok(Some(v));
        }
        let s = if is_bundle(&content) {
            let mut p = tile.split('/').skip(1).map(|v| v.parse::<u32>().ok());
            let (Some(Some(x)), Some(Some(y))) = (p.next(), p.next()) else { return Ok(None) };
            let Some(e) = self.index(&content)?.find(6, x, y) else { return Ok(None) };
            SectView::open_at(self.src(&content)?, e.offset, u64::from(e.len))
        } else {
            SectView::open(self.src(&content)?)
        };
        let v = Arc::new(crate::markview::MarkView::new(s?, id.clone()).with_context(|| format!("markdata {id}"))?);
        self.marks.lock().unwrap().put(id, v.clone());
        Ok(Some(v))
    }

    /// A z3 tile's area details and parks ("3/x/y"), cached by content.
    pub fn ovdata(&self, tile: &str) -> Result<Option<Arc<crate::ovdata::OvView>>> {
        let cat = self.catalog();
        let Some(l) = cat.ovdata.get(tile) else { return Ok(None) };
        let Some(content) = self.content(l) else { return Ok(None) };
        if let Some(v) = self.ovs.lock().unwrap().get(&content) {
            return Ok(Some(v));
        }
        let v = Arc::new(crate::ovdata::OvView::new(
            SectView::open(self.src(&content)?).with_context(|| format!("ovdata {content}"))?,
        ));
        self.ovs.lock().unwrap().put(content, v.clone());
        Ok(Some(v))
    }

    /// The road → units index.
    pub fn roadunits(&self) -> Result<Option<Arc<RoadUnits>>> {
        let Some(content) = self.content("global/roadunits") else { return Ok(None) };
        if let Some((c, r)) = self.roadunits.lock().unwrap().as_ref() {
            if *c == content {
                return Ok(Some(r.clone()));
            }
        }
        let s = SectView::open(self.src(&content)?)?;
        let r = Arc::new(RoadUnits::new(&s)?);
        *self.roadunits.lock().unwrap() = Some((content, r.clone()));
        Ok(Some(r))
    }

    /// A whole file by logical name, its bytes in place (not cached here: its reader keeps it): the
    /// mirror's copy mapped, never read into memory; else read whole from the NAS (a NAS file is
    /// never mapped: store::range). With its content name.
    pub fn whole(&self, logical: &str) -> Result<Option<(String, Whole)>> {
        let Some(content) = self.content(logical) else { return Ok(None) };
        let w = match self.src(&content)? {
            Src::Local(m) => Whole::Mapped(m),
            Src::Remote(r) => Whole::Read(r.read_all()?),
        };
        Ok(Some((content, w)))
    }

    /// A whole small file by logical name (global/…), cached.
    pub fn global(&self, logical: &str) -> Result<Option<Arc<Vec<u8>>>> {
        let Some(content) = self.content(logical) else { return Ok(None) };
        if let Some(b) = self.globals.lock().unwrap().get(&content) {
            return Ok(Some(b));
        }
        let bytes = match self.src(&content)? {
            Src::Local(m) => m.to_vec(),
            Src::Remote(r) => r.read_all()?,
        };
        let b = Arc::new(bytes);
        self.globals.lock().unwrap().put(content, b.clone());
        Ok(Some(b))
    }

    /// The pack holding a tile of `layer`, by logical name.
    fn pack_of(&self, layer: &str, z: u8, x: u32, y: u32) -> Option<String> {
        let cat = self.catalog();
        let l = cat.layers.get(layer)?;
        let f = pipeline::terrain_deep::PACK_Z;
        match z {
            0..=2 => l.root.clone(),
            3..=8 => l.lo.get(&format!("3/{}/{}", x >> (z - 3), y >> (z - 3))).cloned(),
            // (The terrain's deep levels: its deep packs by z10 tile, pipeline::terrain_deep.)
            _ if z > f + 2 && !l.deep.is_empty() => {
                l.deep.get(&format!("{f}/{}/{}", x >> (z - f), y >> (z - f))).cloned()
            }
            _ => l.hi.get(&format!("6/{}/{}", x >> (z - 6), y >> (z - 6))).cloned(),
        }
    }

    /// A pack's index (cached in memory, and on this Mac's disk so it survives restarts).
    fn index(&self, content: &str) -> Result<Arc<PackIndex>> {
        if let Some(i) = self.indexes.lock().unwrap().get(content) {
            return Ok(i);
        }
        let load = || -> Result<PackIndex> {
            match self.src(content)? {
                Src::Local(m) => PackIndex::parse_bytes(&m),
                Src::Remote(r) => PackIndex::read_from(&*r),
            }
        };
        let idx = match &self.mirror {
            Some(m) => m.index(content, load)?,
            None => load()?,
        };
        let idx = Arc::new(idx);
        self.indexes.lock().unwrap().put(content.to_string(), idx.clone());
        Ok(idx)
    }

    /// A tile of one of our layers: its bytes as stored, and its content hash.
    pub fn tile(&self, layer: &str, z: u8, x: u32, y: u32) -> Result<Option<(Blob, u64)>> {
        let Some(logical) = self.pack_of(layer, z, x, y) else { return Ok(None) };
        let Some(content) = self.content(&logical) else { return Ok(None) };
        let idx = self.index(&content)?;
        let Some(e) = idx.find(z, x, y) else { return Ok(None) };
        let blob = match self.src(&content)? {
            Src::Local(m) => Blob::Map(m, e.offset as usize, e.len as usize),
            Src::Remote(r) => Blob::from_vec(r.read_at(e.offset, e.len as usize)?),
        };
        Ok(Some((blob, e.hash)))
    }

    /// The content hash of a tile, without reading it (for 304s): None when there's no such tile,
    /// an error when its pack's index can't be read.
    pub fn tile_hash(&self, layer: &str, z: u8, x: u32, y: u32) -> Result<Option<u64>> {
        let Some(logical) = self.pack_of(layer, z, x, y) else { return Ok(None) };
        let Some(content) = self.content(&logical) else { return Ok(None) };
        // (A 304 is a use too: the area is in view.)
        Ok(self.index(&content)?.find(z, x, y).map(|e| e.hash))
    }

    /// Each tile at zoom `z` of the pack named `content` (a content name, so of whichever catalog
    /// named it): its x, y and bytes as stored, in the order they're stored. Read a MB or so at a
    /// time, several tiles a read, from this Mac's mirror, else the NAS: not through a map, as a
    /// tile served is (a whole layer read so would stay in the server's memory as long as its maps
    /// are kept), nor whole (a big block, which macOS's allocator keeps once freed). Fails when the
    /// pack can't be read, after the tiles before.
    pub fn pack_tiles(&self, content: &str, z: u8, mut each: impl FnMut(u32, u32, &[u8])) -> Result<()> {
        use std::os::unix::fs::FileExt;
        let idx = self.index(content)?;
        // (Sorted by key: a zoom's tiles are together.)
        let first = idx.entries.partition_point(|e| e.key >> 58 < u64::from(z));
        let mut es: Vec<store::pack::Entry> =
            idx.entries[first..].iter().take_while(|e| e.key >> 58 == u64::from(z)).copied().collect();
        es.sort_by_key(|e| e.offset);
        fn end(e: &store::pack::Entry) -> u64 {
            e.offset + u64::from(e.len)
        }
        enum From {
            Mirror(std::fs::File),
            Src(Src),
        }
        // (Not a use of the pack: the server reads it on its own, for the place search.)
        let local = self.mirror.as_ref().and_then(|m| m.local(content)).map(|p| (std::fs::File::open(&p), p));
        let from = match local {
            Some((Ok(f), _)) => From::Mirror(f),
            // Let go just now: the NAS's copy.
            Some((Err(e), _)) if e.kind() == std::io::ErrorKind::NotFound => From::Src(self.src(content)?),
            Some((Err(e), p)) => return Err(e).with_context(|| format!("open {}", p.display())),
            None => From::Src(self.src(content)?),
        };
        let mut buf = Vec::new();
        let mut i = 0;
        while i < es.len() {
            let lo = es[i].offset;
            let n = es[i..].iter().take_while(|e| end(e) - lo <= 1 << 20).count().max(1);
            let hi = es[i..i + n].iter().map(end).max().unwrap_or(lo);
            let b: &[u8] = match &from {
                From::Mirror(f) => {
                    buf.resize((hi - lo) as usize, 0);
                    f.read_exact_at(&mut buf, lo).with_context(|| format!("read {content}"))?;
                    &buf
                }
                From::Src(Src::Local(m)) => m
                    .get(lo as usize..hi as usize)
                    .ok_or_else(|| anyhow!("{content}: its index points past its end"))?,
                From::Src(Src::Remote(r)) => {
                    buf = r.read_at(lo, (hi - lo) as usize).with_context(|| format!("read {content}"))?;
                    &buf
                }
            };
            for e in &es[i..i + n] {
                let (_, x, y) = e.zxy();
                each(x, y, &b[(e.offset - lo) as usize..(end(e) - lo) as usize]);
            }
            i += n;
        }
        Ok(())
    }

    /// A version token for a layer's URLs: changes whenever any of its packs does.
    pub fn layer_version(&self, layer: &str) -> String {
        let cat = self.catalog();
        let Some(l) = cat.layers.get(layer) else { return String::new() };
        let mut h = blake3::Hasher::new();
        for logical in l.root.iter().chain(l.lo.values()).chain(l.hi.values()).chain(l.deep.values()) {
            if let Some(f) = cat.files.get(logical) {
                h.update(f.file.as_bytes());
            }
        }
        h.finalize().to_hex()[..12].to_string()
    }

    /// The basemap's archives in the current catalog: their content names, in the catalog's order.
    pub fn basemap_names(&self) -> Vec<String> {
        let cat = self.catalog();
        cat.basemap.iter().filter_map(|l| cat.files.get(l).map(|f| f.file.clone())).collect()
    }

    /// Where tile z/x/y of the basemap archive `archive` (a content name) is read from: the
    /// mirror's piece holding it when it's here (its path in the mirror, `.basemap/…`), else the
    /// archive itself (on the NAS, or whole in the mirror).
    pub fn basemap_key(&self, archive: &str, z: u8, x: u32, y: u32) -> String {
        let piece =
            self.mirror.as_ref().zip(store::pieces::Piece::of(z, x, y)).filter(|(m, p)| m.has_piece(archive, *p));
        match piece {
            Some((m, p)) => m
                .piece_path(archive, p)
                .strip_prefix(m.root().join("mirror"))
                .map_or_else(|_| archive.to_string(), |r| r.to_string_lossy().into_owned()),
            None => archive.to_string(),
        }
    }

    /// A basemap source by `basemap_key`: a piece mapped from the mirror, or the archive.
    pub fn basemap_src(&self, key: &str) -> Result<Src> {
        if !key.starts_with(".basemap/") {
            return self.src(key);
        }
        if let Some(m) = self.maps.lock().unwrap().get(key) {
            return Ok(Src::Local(m));
        }
        let mirror = self.mirror.as_ref().context("no mirror")?;
        let p = mirror.root().join("mirror").join(key);
        let f = std::fs::File::open(&p).with_context(|| format!("open {}", p.display()))?;
        // SAFETY: a piece is never modified once in place; removal unlinks it (and
        // `forget_removed` drops the map).
        let m = Arc::new(unsafe { Mmap::map(&f)? });
        self.maps.lock().unwrap().put(key.to_string(), m.clone());
        Ok(Src::Local(m))
    }

    /// Whether the lead is running a job (its heartbeat on the NAS, fresh, with a job that
    /// isn't paused). Read at most every 30 s.
    pub fn agent_busy(&self) -> bool {
        let mut g = self.busy.lock().unwrap();
        if let Some((t, b)) = *g {
            if t.elapsed() < Duration::from_secs(30) {
                return b;
            }
        }
        let b = (|| -> Option<bool> {
            let (root, pool) = (self.nas_root()?, self.pool()?);
            let v: serde_json::Value =
                serde_json::from_slice(&pool.read_all(&root.join("state/status.json")).ok()?).ok()?;
            Some(runs(&v, 600, &["job"]))
        })()
        .unwrap_or(false);
        *g = Some((std::time::Instant::now(), b));
        b
    }

    /// Drops what was opened from the NAS and is now on this Mac, so the local copy serves it
    /// (offline too). Views read from the NAS are dropped whole; they're rebuilt on next use.
    pub fn forget_remote(&self) {
        let local = |c: &str| self.mirror.as_ref().is_some_and(|m| m.local(c).is_some());
        // What was read from the files now here goes with their handles (other NAS files keep
        // theirs: they're still read from the NAS).
        let mut gone: std::collections::HashSet<u64> = Default::default();
        self.remotes.lock().unwrap().retain(|c, r| {
            let keep = !local(c);
            if !keep {
                gone.insert(r.id);
            }
            keep
        });
        crate::pages::forget(&gone);
        self.sects.lock().unwrap().retain(|_, v| !v.is_remote());
        self.bases.lock().unwrap().retain(|_, v| !v.is_remote());
        self.his.lock().unwrap().retain(|_, v| !v.is_remote());
        self.marks.lock().unwrap().retain(|_, v| !v.is_remote());
        self.ovs.lock().unwrap().retain(|_, v| !v.is_remote());
        // The road → units index too: reopened from the mirror on next use (offline then works).
        let mut ru = self.roadunits.lock().unwrap();
        if ru.as_ref().is_some_and(|(_, r)| r.is_remote()) {
            *ru = None;
        }
        drop(ru);
        self.mirror_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Drops what's mapped of what the mirror has just deleted (`names`: content names, and
    /// basemap pieces by their paths in it), and the views over them, so the disk gets their room
    /// back; whatever needs them again reads them from the NAS.
    pub fn forget_removed(&self, names: &[String]) {
        let gone: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
        self.maps.lock().unwrap().retain(|c, _| !gone.contains(c));
        self.sects.lock().unwrap().retain(|c, _| !gone.contains(c));
        self.bases.lock().unwrap().retain(|k, _| !k.split('|').any(|c| gone.contains(c)));
        self.his.lock().unwrap().retain(|c, _| !gone.contains(c));
        self.marks.lock().unwrap().retain(|c, _| !gone.contains(c));
        self.ovs.lock().unwrap().retain(|c, _| !gone.contains(c));
        let mut ru = self.roadunits.lock().unwrap();
        if ru.as_ref().is_some_and(|(c, _)| gone.contains(c.as_str())) {
            *ru = None;
        }
        drop(ru);
        self.mirror_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Start the background work: the NAS mount, new catalogs. (The mirror's thread is
    /// downloads::spawn_mirror's.)
    pub fn spawn_background(self: &Arc<Self>) {
        let d = self.clone();
        std::thread::Builder::new()
            .name("nas".into())
            .spawn(move || {
                let mut last = std::time::Instant::now();
                loop {
                    let every = poll_every(crate::updater::in_use(600), d.agent_busy());
                    if last.elapsed() >= Duration::from_secs(every) {
                        if d.nas_root().is_none() || !d.online() {
                            d.find_nas();
                        }
                        d.refresh_catalog();
                        last = std::time::Instant::now();
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
            })
            .ok();
    }
}

/// This Mac's name, for the map servers' log (pipeline::pubtimes).
pub fn host() -> &'static str {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(pipeline::agent::cond::host_name)
}

/// Notes in the map servers' log on the NAS (`root`; pipeline::pubtimes::SERVERS, an append to
/// this Mac's file) this server's adoption of catalog `n` or its mirror's sync of it (`kind`): on a
/// thread of its own, as the share may be slow, best effort.
pub fn publish_event(root: &std::path::Path, kind: &str, n: u64, secs: Option<f64>) {
    let (root, e) = (
        root.to_path_buf(),
        pipeline::pubtimes::ServerEvent { t: 0, host: host().to_string(), kind: kind.into(), n, secs },
    );
    let _ = std::thread::Builder::new().name("publog".into()).spawn(move || {
        if let Err(err) = pipeline::pubtimes::append_server(&root, e) {
            eprintln!("publishing's log on the NAS: {err}");
        }
    });
}

/// `~/Library/Application Support/scenic`.
pub fn default_home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/scenic")
}

/// POST /api/catalog/refresh: follow a new catalog on the NAS now. The build agent's word after
/// writing one (pipeline::net::catalog_written), so this Mac adopts it at once instead of at the
/// next poll. This Mac's own requests alone (loopback, not through a proxy). Answers whether the
/// NAS was read and the catalog served after.
pub async fn refresh_h(
    axum::extract::State(s): axum::extract::State<crate::S>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    h: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !pipeline::net::own(peer.ip(), &h) {
        return (axum::http::StatusCode::FORBIDDEN, "this Mac's own requests alone").into_response();
    }
    let s2 = s.clone();
    let read = tokio::task::spawn_blocking(move || s2.data.refresh_catalog()).await.unwrap_or(false);
    let body = serde_json::json!({ "read": read, "n": s.data.catalog().n });
    ([(axum::http::header::CACHE_CONTROL, "no-store")], axum::Json(body)).into_response()
}

/// How often (seconds) the background thread looks for a new catalog: every 10 s while the lead's
/// heartbeat says a build runs (catalogs come often then); else every 30 s while the map is in use,
/// and every 10 minutes idle, so the NAS rests. Each look is one listing of catalog/ (an unchanged
/// catalog reads nothing more: refresh_catalog). The lead's own server is told at once
/// (/api/catalog/refresh).
pub fn poll_every(in_use: bool, building: bool) -> u64 {
    if building {
        10
    } else if in_use {
        30
    } else {
        600
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether heartbeat `v` beat in the last `fresh_s` seconds with a job in one of `slots` ("job";
/// the second slot's, "beside") that isn't paused.
fn runs(v: &serde_json::Value, fresh_s: u64, slots: &[&str]) -> bool {
    runs_at(v, fresh_s, slots, unix_now())
}

/// `runs` at `now` (unix seconds).
fn runs_at(v: &serde_json::Value, fresh_s: u64, slots: &[&str], now: u64) -> bool {
    let fresh = v.get("beat").and_then(|b| b.as_u64()).is_some_and(|b| now.saturating_sub(b) < fresh_s);
    fresh
        && slots.iter().any(|k| v.get(*k).is_some_and(|j| !j.is_null() && j.get("paused").is_none_or(|p| p.is_null())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_heartbeat_runs_a_job_while_fresh_in_the_slots_asked() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // A job in either slot asked; not a paused one, none, or a stale beat.
        let both = ["job", "beside"];
        assert!(runs_at(&json!({"beat": now, "job": null, "beside": {"id": "lo"}}), 120, &both, now));
        assert!(!runs_at(
            &json!({"beat": now, "job": {"id": "pack 6/52/27", "paused": "the user asked"}}),
            120,
            &both,
            now
        ));
        assert!(!runs_at(&json!({"beat": now, "job": null}), 120, &both, now));
        assert!(!runs_at(&json!({"beat": now - 600, "job": {"id": "pack 6/52/27"}}), 120, &both, now));
        // The NAS's heartbeat (the lead's, read by every Mac): the first slot, fresh for ten
        // minutes.
        assert!(runs_at(&json!({"beat": now - 300, "job": {"id": "pack 6/52/27"}}), 600, &["job"], now));
        assert!(!runs_at(&json!({"beat": now - 300, "job": null, "beside": {"id": "lo"}}), 600, &["job"], now));
    }

    /// Task #449: a tile's landmark points read from the marks job's bundle of every tile (from the
    /// NAS, or mapped here) are those of its own file: the same rows, properties and named peaks;
    /// each tile its own cache and ETag name.
    #[test]
    fn markdata_from_a_bundle_is_the_tiles_own_file() {
        use pipeline::marks::{self, MarkPt, Row, SummitRec, KINDS};
        let (nas, homes) = (tempfile::tempdir().unwrap(), [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()]);
        let made = nas.path().join("made");
        std::fs::create_dir_all(&made).unwrap();
        let tiles = [(9u32, 20u32), (10, 20)];
        let mut files = Vec::new();
        for (n, &(x, y)) in tiles.iter().enumerate() {
            let mut rows: Vec<Vec<Row>> = (0..KINDS.len()).map(|_| Vec::new()).collect();
            let k = marks::kind_index("waterfall").unwrap();
            for i in 0..3u64 {
                rows[k].push(Row {
                    id: 100 * n as u64 + i,
                    pt: MarkPt { lon: -700_000_000 + i as i32, lat: 470_000_000, rank: i as u32, ..Default::default() },
                    fvals: vec![f64::NAN; marks::fields("waterfall").len()],
                    props: format!(r#"{{"name":"falls {n}.{i}"}}"#).into_bytes(),
                    info: br#"{"qid":"Q1"}"#.to_vec(),
                });
            }
            let summits = [(SummitRec { rank: n as u32, lon: 1, lat: 2, pad: 0, ele: 1234.5 }, format!("peak {n}"))];
            let p = made.join(format!("6-{x}-{y}.sect"));
            marks::write_markdata(&p, (x, y), &rows, &summits).unwrap();
            files.push(p);
        }
        // The two forms: a file per tile, and the bundle (docs/formats.md "Bundles").
        let mut old = Catalog::new(1);
        let mut new = Catalog::new(1);
        let mut w = store::pack::PackWriter::create(&made.join("all.pack"), json!({"bundle": "markdata"}), false)
            .unwrap()
            .aligned(64);
        for (&(x, y), p) in tiles.iter().zip(&files) {
            let b = std::fs::read(p).unwrap();
            let l = format!("markdata/6-{x}-{y}");
            let c = store::naming::write_atomic(nas.path(), &l, "sect", store::naming::Source::Bytes(&b)).unwrap();
            old.files
                .insert(l.clone(), store::catalog::FileRef { file: c, size: b.len() as u64, ..Default::default() });
            old.markdata.insert(format!("6/{x}/{y}"), l);
            new.markdata.insert(format!("6/{x}/{y}"), "markdata/all".into());
            w.add(6, x, y, &b, b.len() as u32).unwrap();
        }
        w.finish().unwrap();
        let b = std::fs::read(made.join("all.pack")).unwrap();
        let c =
            store::naming::write_atomic(nas.path(), "markdata/all", "pack", store::naming::Source::Bytes(&b)).unwrap();
        new.files.insert(
            "markdata/all".into(),
            store::catalog::FileRef { file: c.clone(), size: b.len() as u64, ..Default::default() },
        );
        let open = |home: &std::path::Path, cat: &Catalog| {
            let dir = nas.path().join("catalog");
            std::fs::remove_dir_all(&dir).ok();
            store::catalog::write(&dir, cat).unwrap();
            Data::open(Options {
                home: home.to_owned(),
                nas_root: Some(nas.path().to_owned()),
                mirror: false,
                reserve: 0,
            })
            .unwrap()
        };
        let (a, z) = (open(homes[0].path(), &old), open(homes[1].path(), &new));
        let rows = |v: &crate::markview::MarkView| {
            let (ids, pts, fvals) = v.scan().unwrap();
            let props: Vec<Vec<u8>> = (0..ids.cast::<u64>().len()).map(|i| v.props(i).unwrap()).collect();
            let info: Vec<Vec<u8>> = (0..ids.cast::<u64>().len()).map(|i| v.info(i).unwrap()).collect();
            (
                v.tile,
                ids.bytes().to_vec(),
                pts.cast::<MarkPt>().to_vec(),
                fvals.bytes().to_vec(),
                props,
                info,
                v.summits.clone(),
                v.summit_name(0).unwrap().to_string(),
            )
        };
        for &(x, y) in &tiles {
            let t = format!("6/{x}/{y}");
            let (va, vz) = (a.markdata(&t).unwrap().unwrap(), z.markdata(&t).unwrap().unwrap());
            assert_eq!(rows(&vz), rows(&va), "{t}");
            assert_eq!(z.markdata_id(&t).unwrap(), format!("{c}#{t}"));
            // Mapped here (a mirror's copy): the same.
            let m = Arc::new(unsafe { Mmap::map(&std::fs::File::open(nas.path().join(&c)).unwrap()).unwrap() });
            let e = store::pack::PackIndex::parse_bytes(&m).unwrap().find(6, x, y).unwrap();
            let s = SectView::open_at(Src::Local(m), e.offset, u64::from(e.len)).unwrap();
            // (Its own name: the blocks are cached by it.)
            let vm = crate::markview::MarkView::new(s, format!("mapped {t}")).unwrap();
            assert_eq!(rows(&vm), rows(&va), "{t} mapped");
        }
        assert!(z.markdata("6/1/1").unwrap().is_none());
    }

    /// The agent's word after a catalog: adopted at once, from this Mac alone (not the LAN, the
    /// tailnet, or a device through a proxy on this Mac).
    #[tokio::test]
    async fn a_refresh_from_this_mac_adopts_the_new_catalog_at_once() {
        use axum::http::StatusCode;
        use tower::ServiceExt;
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dir = nas.path().join("catalog");
        store::catalog::write_copy(&dir, &Catalog::new(1)).unwrap();
        let s = crate::test_state(home.path(), nas.path());
        assert_eq!(s.data.catalog().n, 1);
        store::catalog::write_copy(&dir, &Catalog::new(2)).unwrap();
        let ask = |peer: &str, proxied: bool| {
            let app =
                axum::Router::new().route("/api/catalog/refresh", axum::routing::post(refresh_h)).with_state(s.clone());
            let mut b = axum::http::Request::builder().method("POST").uri("/api/catalog/refresh");
            if proxied {
                b = b.header("x-forwarded-for", "100.101.102.103");
            }
            let mut req = b.body(axum::body::Body::empty()).unwrap();
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(peer.parse().unwrap(), 50000)));
            async move { app.oneshot(req).await.unwrap().status() }
        };
        for (peer, proxied) in [("192.168.1.66", false), ("100.101.102.103", false), ("127.0.0.1", true)] {
            assert_eq!(ask(peer, proxied).await, StatusCode::FORBIDDEN, "{peer}");
        }
        assert_eq!(s.data.catalog().n, 1, "refused: nothing read");
        // Adopted at once: the new catalog read by the time the answer comes, not at the next poll.
        assert_eq!(ask("127.0.0.1", false).await, StatusCode::OK);
        assert_eq!(s.data.catalog().n, 2);
        assert_eq!(ask("::1", false).await, StatusCode::OK);
    }

    /// A build running (the lead's heartbeat): a look every 10 s, in use or not; else 30 s / 10 min.
    #[test]
    fn catalogs_are_looked_for_every_ten_seconds_while_a_build_runs() {
        assert_eq!((poll_every(true, true), poll_every(false, true)), (10, 10));
        assert_eq!((poll_every(true, false), poll_every(false, false)), (30, 600));
        // As the background thread asks it: the lead's heartbeat on the NAS, a fresh one with a job.
        let beat = |job: serde_json::Value| {
            let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            std::fs::create_dir_all(nas.path().join("state")).unwrap();
            std::fs::write(nas.path().join("state/status.json"), json!({"beat": unix_now(), "job": job}).to_string())
                .unwrap();
            let d = Data::open(Options {
                home: home.path().to_owned(),
                nas_root: Some(nas.path().to_owned()),
                mirror: false,
                reserve: 0,
            })
            .unwrap();
            poll_every(false, d.agent_busy())
        };
        assert_eq!(beat(json!({"id": "catalog"})), 10);
        assert_eq!(beat(serde_json::Value::Null), 600);
    }

    /// Delta catalogs (docs/formats.md): the server follows them from the catalog it serves, as the
    /// whole catalog would be, keeping a whole copy on this Mac; the deltas after the one it has are
    /// all it reads (the whole one under them gone from the NAS, it still follows).
    #[test]
    fn the_server_follows_delta_catalogs_from_the_one_it_has() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dir = nas.path().join("catalog");
        let cat = |n: u64, files: &[&str]| {
            let mut c = Catalog::new(n);
            for f in files {
                let logical = format!("layers/roads/hi/{f}");
                c.files.insert(
                    logical.clone(),
                    store::catalog::FileRef {
                        file: format!("{logical}.{:016x}.pack", n),
                        size: n,
                        ..Default::default()
                    },
                );
                c.layers.entry("roads".into()).or_default().hi.insert(f.replace('-', "/"), logical);
            }
            c
        };
        let (one, two, three) = (cat(1, &["6-1-1"]), cat(2, &["6-1-1", "6-1-2"]), cat(3, &["6-1-2", "6-1-3"]));
        store::catalog::write(&dir, &one).unwrap();
        let d = Data::open(Options {
            home: home.path().to_owned(),
            nas_root: Some(nas.path().to_owned()),
            mirror: false,
            reserve: 0,
        })
        .unwrap();
        assert_eq!(*d.catalog(), one);
        store::catalog::write_delta(&dir, &one, &two).unwrap();
        assert!(d.refresh_catalog());
        assert_eq!(*d.catalog(), two);
        assert_eq!(store::catalog::read(&home.path().join("catalog/2.json.zst")).unwrap(), two, "kept whole here");
        // The whole one under the deltas gone: catalog 3 still followed, from 2.
        store::catalog::write_delta(&dir, &two, &three).unwrap();
        std::fs::remove_file(dir.join("1.json.zst")).unwrap();
        assert!(d.refresh_catalog());
        assert_eq!(*d.catalog(), three);
        // (Nothing newer: as it is.)
        assert!(d.refresh_catalog());
        assert_eq!(*d.catalog(), three);
    }
}

/// A file's bytes whole and in place (`Data::whole`): mapped from the mirror, or read from the NAS.
pub enum Whole {
    Mapped(Arc<Mmap>),
    Read(Vec<u8>),
}

impl store::range::Mapped for Whole {
    fn bytes(&self) -> &[u8] {
        match self {
            Whole::Mapped(m) => m,
            Whole::Read(v) => v,
        }
    }
}

impl store::RangeRead for Whole {
    fn len(&self) -> Result<u64, store::IoError> {
        Ok(store::range::Mapped::bytes(self).len() as u64)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::IoError> {
        let b = store::range::Mapped::bytes(self);
        let end = off.checked_add(len as u64).filter(|&e| e <= b.len() as u64);
        match end {
            Some(e) => Ok(b[off as usize..e as usize].to_vec()),
            None => Err(store::IoError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("{len} bytes at {off} past the end ({})", b.len()),
            ))),
        }
    }
}

/// Whether a content name is a bundle of files (an RDPACK whose members are files: docs/formats.md
/// "Bundles"), not a file of its own: the markdata's (task #449).
pub fn is_bundle(content: &str) -> bool {
    content.ends_with(".pack")
}
