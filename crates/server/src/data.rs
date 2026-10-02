//! The map's data as the server sees it (docs/plan.md §3–4): the current catalog, every file it
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
pub const SMB_URL: &str = store::nas::SMB_URL;
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

/// Whether the NAS answers on the SMB port (TCP 445), within two seconds.
fn smb_reachable() -> bool {
    use std::net::ToSocketAddrs;
    let Ok(addrs) = (format!("{NAS_HOST}.local"), 445).to_socket_addrs() else { return false };
    addrs.into_iter().any(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_secs(2)).is_ok())
}

/// Memory budgets for what's read from the NAS (a Mac whose mirror isn't complete).
const BASES_BYTES: u64 = 3 << 30;
const HIDATA_BYTES: u64 = 1 << 30;
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
    roadunits: Mutex<Option<(String, Arc<RoadUnits>)>>,
    globals: Mutex<Bounded<Arc<Vec<u8>>>>,
    /// Set when the catalog changes, so caches built from the old one are dropped.
    pub generation: std::sync::atomic::AtomicU64,
    /// Set when the mirror has copied files, so archives opened from the NAS are opened again.
    pub mirror_gen: std::sync::atomic::AtomicU64,
    /// The last attempt to mount the share.
    last_mount: Mutex<Option<std::time::Instant>>,
}

/// A data development override: serve a local folder laid out like the NAS project folder.
pub struct Options {
    pub home: PathBuf,
    pub nas_root: Option<PathBuf>,
    pub mirror: bool,
    pub reserve_gb: u64,
}

impl Data {
    pub fn open(o: Options) -> Result<Arc<Data>> {
        std::fs::create_dir_all(o.home.join("catalog"))?;
        let mirror = if o.mirror { Some(Arc::new(store::mirror::Mirror::open(o.home.clone(), o.reserve_gb << 30)?)) } else { None };
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
            his: Mutex::new(Bounded::with_bytes(512, HIDATA_BYTES)),
            roadunits: Mutex::new(None),
            globals: Mutex::new(Bounded::new(128)),
            generation: Default::default(),
            mirror_gen: Default::default(),
            last_mount: Mutex::new(None),
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
        let found = store::nas::find_mount(NAS_HOST, NAS_SHARE).map(|m| m.point.join(PROJECT));
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
        if !smb_reachable() {
            return;
        }
        if let Err(e) = store::nas::mount(SMB_URL, Duration::from_secs(20)) {
            eprintln!("NAS: can't mount {SMB_URL}: {e:#}");
        }
        self.set_nas(store::nas::find_mount(NAS_HOST, NAS_SHARE).map(|m| m.point.join(PROJECT)));
    }

    /// Follow a newer catalog on the NAS; true when the NAS was read (whether or not it changed).
    pub fn refresh_catalog(&self) -> bool {
        let (Some(root), Some(pool)) = (self.nas_root(), self.pool()) else { return false };
        match store::catalog::latest_nas(&pool, &root.join("catalog")) {
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
                    *self.cat.write().unwrap() = Arc::new(c);
                    self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        if let Some(p) = self.mirror.as_ref().and_then(|m| m.local(content)) {
            let f = std::fs::File::open(&p).with_context(|| format!("open {}", p.display()))?;
            // SAFETY: mirrored files are content-named and never modified; eviction unlinks them.
            let m = Arc::new(unsafe { Mmap::map(&f)? });
            if let Some(mi) = &self.mirror {
                mi.touch(content);
            }
            self.maps.lock().unwrap().put(content.to_string(), m.clone());
            return Ok(Src::Local(m));
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
        let w = bs.remote_bytes() + rs.remote_bytes();
        let v = Arc::new(BaseView::new(bs, rs)?);
        self.bases.lock().unwrap().put_weighed(key, v.clone(), w);
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
        let sv = SectView::open(self.src(&content)?)?;
        let w = sv.remote_bytes();
        let v = Arc::new(HiView::new(sv)?);
        self.his.lock().unwrap().put_weighed(content, v.clone(), w);
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
        match z {
            0..=2 => l.root.clone(),
            3..=8 => l.lo.get(&format!("3/{}/{}", x >> (z - 3), y >> (z - 3))).cloned(),
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
        Ok(self.index(&content)?.find(z, x, y).map(|e| e.hash))
    }

    /// A version token for a layer's URLs: changes whenever any of its packs does.
    pub fn layer_version(&self, layer: &str) -> String {
        let cat = self.catalog();
        let Some(l) = cat.layers.get(layer) else { return String::new() };
        let mut h = blake3::Hasher::new();
        for logical in l.root.iter().chain(l.lo.values()).chain(l.hi.values()) {
            if let Some(f) = cat.files.get(logical) {
                h.update(f.file.as_bytes());
            }
        }
        h.finalize().to_hex()[..12].to_string()
    }

    /// The basemap's archives (content names), in the catalog's order.
    pub fn basemaps(&self) -> Result<Vec<(String, Src)>> {
        let cat = self.catalog();
        cat.basemap.iter().filter_map(|l| self.content(l)).map(|c| Ok((c.clone(), self.src(&c)?))).collect()
    }

    /// Drops what was opened from the NAS and is now on this Mac, so the local copy serves it
    /// (offline too). Views read from the NAS are dropped whole; they're rebuilt on next use.
    pub fn forget_remote(&self) {
        let local = |c: &str| self.mirror.as_ref().is_some_and(|m| m.local(c).is_some());
        self.remotes.lock().unwrap().retain(|c, _| !local(c));
        self.sects.lock().unwrap().retain(|_, v| !v.is_remote());
        self.bases.lock().unwrap().retain(|_, v| !v.is_remote());
        self.his.lock().unwrap().retain(|_, v| !v.is_remote());
        self.mirror_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Start the background work: the NAS mount, new catalogs, the mirror.
    pub fn spawn_background(self: &Arc<Self>) {
        let d = self.clone();
        std::thread::Builder::new()
            .name("nas".into())
            .spawn(move || {
                let mut last = std::time::Instant::now();
                loop {
                    // While the map is in use: every 30 s. Idle: every 10 minutes, so the NAS rests.
                    let every = if crate::updater::in_use(600) { 30 } else { 600 };
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
        if let Some(m) = self.mirror.clone() {
            let d = self.clone();
            std::thread::Builder::new()
                .name("mirror".into())
                .spawn(move || loop {
                    if let (Some(root), Some(pool)) = (d.nas_root(), d.pool()) {
                        if pool.is_online() {
                            let cat = d.catalog();
                            let pause = || false;
                            match m.sync(&cat, &root, &pool, &pause) {
                                Ok(s) => {
                                    if s.copied > 0 {
                                        eprintln!("mirror: {s:?}");
                                        d.forget_remote();
                                    }
                                }
                                Err(e) => eprintln!("mirror: {e:#}"),
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_secs(60));
                })
                .ok();
        }
    }
}

/// `~/Library/Application Support/scenic`.
pub fn default_home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/scenic")
}
