//! The map's data as the server sees it (docs/storage.md–4): the map served, as a catalogue (the
//! map's log followed: crate::maplog), every file it names read from this Mac's mirror when it's
//! there (memory-mapped) or from the NAS (through the bounded I/O pool, never mapped), and the
//! background work that keeps the NAS mounted.
//!
//! **Away from the NAS, the map is what's downloaded** (`Data::content_in`): while the NAS isn't
//! read (not mounted, or its pool's breaker open), a file not on this Mac is none, as if the map had
//! none there, so every feature answers from the downloads alone, at once, and is empty outside
//! them. What's downloaded that covers more than the downloads (a region's lo and root packs, the
//! basemap's zooms 0–10, the landmarks' bundle) is read only where they reach (`Data::reaches`). A
//! share found mounted is read only once it has answered a probe (`find_nas`): a mount left from
//! home never holds a request.

use crate::cache::Lru;
use crate::views::{BaseView, Blob, HiView, RoadUnits, SectView, Src};
use anyhow::{anyhow, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use store::catalogue::Catalogue;
use store::iopool::IoPool;
use store::pack::PackIndex;
use store::range::{MmapFile, PooledFile, RangeRead};

/// The NAS share and the project folder on it.
pub const NAS_HOST: &str = store::nas::HOST;
pub const NAS_SHARE: &str = store::nas::SHARE;
pub const PROJECT: &str = store::nas::PROJECT;

/// Memory budgets for what's read from the NAS (a Mac whose mirror isn't complete): sectioned
/// files read whole, and the base views' names. Base packs' and hidata's sections are paged
/// (`pages`, with its own budget).
const BASES_BYTES: u64 = 512 << 20;
const SECTS_BYTES: u64 = 1 << 30;

pub struct Data {
    /// ~/Library/Application Support/scenic
    pub home: PathBuf,
    /// The project folder on the NAS, while the share is mounted.
    nas: RwLock<Option<store::nasfs::NasRoot>>,
    pool: RwLock<Option<Arc<IoPool>>>,
    pub mirror: Option<Arc<store::mirror::Mirror>>,
    /// The map served, as a catalogue (crate::maplog's): swapped whole, never changed in place (a
    /// request reads one from start to end), never back to an older one.
    cat: RwLock<Arc<Catalogue>>,
    /// Local maps of mirrored files, and open NAS files, by content name.
    maps: Mutex<Lru<String, Arc<MmapFile>>>,
    remotes: Mutex<Lru<String, Arc<PooledFile>>>,
    indexes: Mutex<Lru<String, Arc<PackIndex>>>,
    sects: Mutex<Lru<String, Arc<SectView>>>,
    bases: Mutex<Lru<String, Arc<BaseView>>>,
    his: Mutex<Lru<String, Arc<HiView>>>,
    marks: Mutex<Lru<String, Arc<crate::markview::MarkView>>>,
    ovs: Mutex<Lru<String, Arc<crate::ovdata::OvView>>>,
    roadunits: Mutex<Option<(String, Arc<RoadUnits>)>>,
    globals: Mutex<Lru<String, Arc<Vec<u8>>>>,
    /// The older files the downloads behind the catalogue keep, by logical name (`set_older`).
    older: RwLock<Arc<HashMap<String, String>>>,
    /// Where the downloads reach (`set_reach`): what's read away from the NAS keeps to it (`reaches`).
    reach: RwLock<Reach>,
    /// Set when the catalogue changes, what's read of the downloads behind it (`set_older`), where
    /// they reach (`set_reach`), or whether the NAS is read (`online`), so caches built from what
    /// was read before are dropped.
    pub generation: Arc<std::sync::atomic::AtomicU64>,
    /// Set when the mirror has copied files, so archives opened from the NAS are opened again.
    pub mirror_gen: std::sync::atomic::AtomicU64,
    /// The last attempt to mount the share.
    last_mount: Mutex<Option<std::time::Instant>>,
    /// The lead's heartbeat and when it was read (`lead_status`).
    lead: Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    /// A mount by the bare name has been reported (once).
    warned_tunnel: std::sync::atomic::AtomicBool,
}

/// A data development override: serve a local folder laid out like the NAS project folder.
pub struct Options {
    pub home: PathBuf,
    pub nas_root: Option<store::nasfs::NasRoot>,
    pub mirror: bool,
    /// The free space the mirror leaves on the disk (bytes).
    pub reserve: u64,
}

impl Data {
    pub fn open(o: Options) -> Result<Arc<Data>> {
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
            cat: RwLock::new(Arc::new(Catalogue::default())),
            older: Default::default(),
            reach: Default::default(),
            maps: Mutex::new(Lru::entries(4096)),
            remotes: Mutex::new(Lru::entries(128)),
            indexes: Mutex::new(Lru::entries(8192)),
            sects: Mutex::new(Lru::bounded(512, SECTS_BYTES)),
            bases: Mutex::new(Lru::bounded(256, BASES_BYTES)),
            his: Mutex::new(Lru::entries(512)),
            marks: Mutex::new(Lru::entries(4096)),
            ovs: Mutex::new(Lru::entries(512)),
            roadunits: Mutex::new(None),
            globals: Mutex::new(Lru::entries(128)),
            generation: Default::default(),
            mirror_gen: Default::default(),
            last_mount: Mutex::new(None),
            lead: Mutex::new(None),
            warned_tunnel: Default::default(),
        });
        match o.nas_root {
            Some(root) => {
                d.set_nas(Some(root), false);
                // (A folder laid out as the NAS's, served as it is: its newest map snapshot, read
                // once. This Mac's server follows the lead's log instead: crate::maplog::spawn.)
                d.serve_root_map();
            }
            None => d.find_nas(),
        }
        Ok(d)
    }

    pub fn catalogue(&self) -> Arc<Catalogue> {
        self.cat.read().unwrap().clone()
    }

    /// The newest map snapshot of the NAS (or the folder laid out as it: `Options::nas_root`)
    /// served, when it's newer than the map served; whether one was read.
    pub fn serve_root_map(&self) -> bool {
        let Some(root) = self.nas_root() else { return false };
        match store::map::newest_nas(&root).and_then(|m| m.map(|m| m.catalogue()).transpose()) {
            Ok(Some(c)) => {
                self.set_map(Arc::new(c));
                true
            }
            Ok(None) => {
                eprintln!("maplog: no map in {}", root.display());
                false
            }
            Err(e) => {
                eprintln!("maplog: the map in {}: {e:#}", root.display());
                false
            }
        }
    }

    /// The map's catalogue (crate::maplog) served from now on: swapped whole when its number moves
    /// on, the generation with it; never back to an older one (a lead's snapshot behind the map
    /// held, a lead restored from an older NAS snapshot).
    pub fn set_map(&self, c: Arc<Catalogue>) {
        let mut cur = self.cat.write().unwrap();
        if c.n > cur.n {
            eprintln!("map {} ({} files)", c.n, c.files.len());
            *cur = c;
            drop(cur);
            self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn nas_root(&self) -> Option<store::nasfs::NasRoot> {
        self.nas.read().unwrap().clone()
    }

    pub fn pool(&self) -> Option<Arc<IoPool>> {
        self.pool.read().unwrap().clone()
    }

    /// Whether the NAS is read now: mounted, and its pool's breaker closed. When it isn't, the map
    /// is this Mac's downloads alone (`content_in`).
    pub fn online(&self) -> bool {
        self.pool().is_some_and(|p| p.is_online())
    }

    /// The NAS's project folder read from now on (None: not mounted). A share found mounted
    /// (`probe_first`) is read once it answers a probe, made at once: until then, and whenever its
    /// breaker is open, the map is the downloads alone. Each change of whether the NAS is read moves
    /// the generation on (what's read changes with it).
    fn set_nas(&self, root: Option<store::nasfs::NasRoot>, probe_first: bool) {
        let mut cur = self.nas.write().unwrap();
        if *cur == root {
            return;
        }
        let was = self.online();
        // (The pool's probe: a stat of the root itself, on its own thread: tests/nas_access.rs.)
        let pool = root.as_ref().map(|r| IoPool::for_root(r, 8, Duration::from_secs(8)));
        if let Some(p) = &pool {
            let g = self.generation.clone();
            p.set_listener(move |_| {
                g.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
            if probe_first {
                p.probe_first();
            }
        }
        *self.pool.write().unwrap() = pool;
        *cur = root;
        if self.online() != was {
            self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
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
        // (The server's reads retried briefly, never for minutes: an HTTP worker waits on them.)
        let found = mount.map(|m| store::nasfs::NasRoot::from_arg(m.point.join(PROJECT)).short_retry());
        if found.is_some() {
            self.set_nas(found, true);
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
        self.set_nas(
            store::nas::find_mount(NAS_HOST, NAS_SHARE)
                .map(|m| store::nasfs::NasRoot::from_arg(m.point.join(PROJECT)).short_retry()),
            true,
        );
    }

    /// The content name read for a logical name of the catalogue served (`content_in`).
    pub fn content(&self, logical: &str) -> Option<String> {
        self.content_in(&self.catalogue(), logical)
    }

    /// The content name read for a logical name of `cat`: where the server chooses what it reads. A
    /// download behind the catalogue reads its own older file while it's on this Mac (the region
    /// stays as it was downloaded, docs/macs.md, "Mirror, per Mac"); else `cat`'s file, here or on
    /// the NAS while it's read (`online`), else here alone: away from the NAS, what isn't
    /// downloaded is none.
    pub fn content_in(&self, cat: &Catalogue, logical: &str) -> Option<String> {
        let cur = cat.files.get(logical).map(|f| f.file.clone());
        let older = self.older.read().unwrap().get(logical).cloned();
        let m = self.mirror.as_deref();
        match (older, m) {
            (Some(o), Some(m)) if m.has(&o) => Some(o),
            _ if self.online() => cur,
            _ => cur.filter(|c| m.is_some_and(|m| m.has(c))),
        }
    }

    /// Where the downloads reach, away from the NAS, as z6 tiles: what's read of the files that
    /// reach past them (the lo and root packs, the basemap's zooms 0–10: `reaches`) and the place
    /// search (crate::places) keep to it. None while the NAS is read, or with the World downloaded
    /// (zoomed out, everywhere).
    pub fn reach(&self) -> Option<Arc<HashSet<(u32, u32)>>> {
        if self.online() {
            return None;
        }
        match &*self.reach.read().unwrap() {
            Reach::World => None,
            Reach::Tiles(t) => Some(t.clone()),
        }
    }

    /// A token of `reach` while it keeps what's read (None while the NAS is read, or with the
    /// World): every version the app's URLs carry has it after it then (main.rs `meta_json`), so
    /// an answer made for one reach (a tile outside it, none) is neither reused for another nor
    /// kept at home, by the browser's cache or an open page (which asks again for what a new
    /// version names).
    pub fn reach_token(&self) -> Option<String> {
        let r = self.reach()?;
        let mut tiles: Vec<&(u32, u32)> = r.iter().collect();
        tiles.sort_unstable();
        let mut h = blake3::Hasher::new();
        h.update(b"reach");
        for (x, y) in tiles {
            h.update(&x.to_le_bytes());
            h.update(&y.to_le_bytes());
        }
        Some(format!("r{}", &h.finalize().to_hex()[..8]))
    }

    /// Whether tile z/x/y is where the downloads reach (`reach`): from z6 its z6 tile is one of
    /// theirs, coarser one of theirs is under it. A region's lo packs and the basemap's zooms 0–10
    /// cover far more than the region: away from the NAS, read only over it.
    pub fn reaches(&self, z: u8, x: u32, y: u32) -> bool {
        let Some(r) = self.reach() else { return true };
        match z.checked_sub(6) {
            Some(d) => r.contains(&(x >> d, y >> d)),
            None => r.iter().any(|&(a, b)| (a >> (6 - z), b >> (6 - z)) == (x, y)),
        }
    }

    /// `reaches` for a tile key ("6/32/21"; the landmark points' and area details' files, one
    /// bundle of every tile's for the landmarks: what's read of them keeps to the downloads).
    fn reaches_key(&self, key: &str) -> bool {
        let mut p = key.split('/').map(str::parse::<u32>);
        match (p.next(), p.next(), p.next()) {
            (Some(Ok(z)), Some(Ok(x)), Some(Ok(y))) => self.reaches(z as u8, x, y),
            _ => true,
        }
    }

    /// Sets where the downloads reach (crate::downloads); a change moves the generation on.
    pub fn set_reach(&self, r: Reach) {
        let mut g = self.reach.write().unwrap();
        if *g != r {
            *g = r;
            self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// A token of the older files read (`set_older`): "" when there are none; it changes with them,
    /// for the URLs of what's read across many files (way info, the basemap).
    pub fn older_token(&self) -> String {
        let g = self.older.read().unwrap();
        if g.is_empty() {
            return String::new();
        }
        let mut v: Vec<&String> = g.values().collect();
        v.sort();
        let mut h = blake3::Hasher::new();
        for c in v {
            h.update(c.as_bytes());
        }
        format!("-o{}", &h.finalize().to_hex()[..8])
    }

    /// Sets the older files the downloads behind the catalogue keep (logical name → content
    /// name: crate::downloads); a change bumps the generation, so what was read before is read
    /// again. Whether it changed.
    pub fn set_older(&self, older: HashMap<String, String>) -> bool {
        let mut g = self.older.write().unwrap();
        if **g == older {
            return false;
        }
        *g = Arc::new(older);
        self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        true
    }

    /// A content-named file: mapped from the mirror, else on the NAS.
    fn src(&self, content: &str) -> Result<Src> {
        if let Some(m) = self.maps.lock().unwrap().get(content) {
            return Ok(Src::Local(m));
        }
        if let Some((mi, p)) = self.mirror.as_ref().and_then(|mi| Some((mi, mi.local(content)?))) {
            // (Mirrored files are content-named and never modified; removal unlinks them, and
            // `forget_removed` drops the maps.)
            match MmapFile::open(&p) {
                Ok(m) => {
                    let m = Arc::new(m);
                    // Kept only while the mirror has the file: a map kept after it let the file go
                    // would hold its room. A removal drops the file from the mirror's list, then
                    // what's cached of it (`forget_removed`): asked again once cached, the list
                    // has dropped it, or the cache's drop is still to come.
                    if mi.has(content) {
                        self.maps.lock().unwrap().put(content.to_string(), m.clone(), 0);
                        if !mi.has(content) {
                            self.maps.lock().unwrap().retain(|c, _| c != content);
                        }
                    }
                    return Ok(Src::Local(m));
                }
                // Let go just now: the NAS's copy.
                Err(e) if not_found(&e) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(r) = self.remotes.lock().unwrap().get(content) {
            return Ok(Src::Remote(r));
        }
        let (Some(root), Some(pool)) = (self.nas_root(), self.pool()) else {
            return Err(anyhow!("{content}: not on this Mac, and the NAS isn't mounted"));
        };
        let r = Arc::new(PooledFile::new(root.join(content), pool));
        self.remotes.lock().unwrap().put(content.to_string(), r.clone(), 0);
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
        self.sects.lock().unwrap().put(content, s.clone(), w);
        Ok(Some(s))
    }

    /// A unit's base pack with its road values ("6/32/21"). Cached by content: a new catalogue's
    /// pack for the same unit is another entry.
    pub fn base(&self, unit: &str) -> Result<Option<Arc<BaseView>>> {
        let cat = self.catalogue();
        let (Some(b), Some(r)) = (cat.base.get(unit), cat.roads.get(unit)) else { return Ok(None) };
        let (Some(bc), Some(rc)) = (self.content(b), self.content(r)) else { return Ok(None) };
        let key = format!("{bc}|{rc}");
        if let Some(v) = self.bases.lock().unwrap().get(&key) {
            return Ok(Some(v));
        }
        let (bs, rs) = (SectView::open(self.src(&bc)?)?, SectView::open(self.src(&rc)?)?);
        let v = Arc::new(BaseView::new(bs, rs).with_context(|| format!("base pack {bc}"))?);
        self.bases.lock().unwrap().put(key, v.clone(), v.weight());
        Ok(Some(v))
    }

    /// A z6 tile's hidata ("6/32/21"), cached by content.
    pub fn hidata(&self, tile: &str) -> Result<Option<Arc<HiView>>> {
        let cat = self.catalogue();
        let Some(l) = cat.hidata.get(tile) else { return Ok(None) };
        let Some(content) = self.content(l) else { return Ok(None) };
        self.hidata_of(&content).map(Some)
    }

    /// A hidata file by content name (of whichever catalogue named it).
    pub fn hidata_of(&self, content: &str) -> Result<Arc<HiView>> {
        if let Some(v) = self.his.lock().unwrap().get(content) {
            return Ok(v);
        }
        let v =
            Arc::new(HiView::new(SectView::open(self.src(content)?)?).with_context(|| format!("hidata {content}"))?);
        self.his.lock().unwrap().put(content.to_string(), v.clone(), 0);
        Ok(v)
    }

    /// What names a z6 tile's landmark points ("6/32/21") in caches and ETags: its file's content
    /// name, with the tile when the file is the marks job's bundle of every tile (docs/formats.md
    /// "Bundles"; task #449). None when the catalogue has none there.
    pub fn markdata_id(&self, tile: &str) -> Option<String> {
        if !self.reaches_key(tile) {
            return None;
        }
        let content = self.content(self.catalogue().markdata.get(tile)?)?;
        Some(if is_bundle(&content) { format!("{content}#{tile}") } else { content })
    }

    /// A z6 tile's landmark points ("6/32/21"), cached by `markdata_id`: its own file, or its member
    /// of the bundle (read in place: one range read of the bundle's index, kept as a pack's is).
    pub fn markdata(&self, tile: &str) -> Result<Option<Arc<crate::markview::MarkView>>> {
        if !self.reaches_key(tile) {
            return Ok(None);
        }
        let cat = self.catalogue();
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
        self.marks.lock().unwrap().put(id, v.clone(), 0);
        Ok(Some(v))
    }

    /// A z3 tile's area details and parks ("3/x/y"), cached by content.
    pub fn ovdata(&self, tile: &str) -> Result<Option<Arc<crate::ovdata::OvView>>> {
        if !self.reaches_key(tile) {
            return Ok(None);
        }
        let cat = self.catalogue();
        let Some(l) = cat.ovdata.get(tile) else { return Ok(None) };
        let Some(content) = self.content(l) else { return Ok(None) };
        if let Some(v) = self.ovs.lock().unwrap().get(&content) {
            return Ok(Some(v));
        }
        let v = Arc::new(crate::ovdata::OvView::new(
            SectView::open(self.src(&content)?).with_context(|| format!("ovdata {content}"))?,
        ));
        self.ovs.lock().unwrap().put(content, v.clone(), 0);
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
            Src::Local(m) => m.bytes().to_vec(),
            Src::Remote(r) => r.read_all()?,
        };
        let b = Arc::new(bytes);
        self.globals.lock().unwrap().put(content, b.clone(), 0);
        Ok(Some(b))
    }

    /// The pack holding a tile of `layer`, by logical name: none for a root or lo pack's tile where
    /// the downloads don't reach (`reaches`), away from the NAS.
    pub(crate) fn pack_of(&self, layer: &str, z: u8, x: u32, y: u32) -> Option<String> {
        let cat = self.catalogue();
        let l = cat.layers.get(layer)?;
        let f = pipeline::terrain_deep::PACK_Z;
        // (A polar layer's: z0–3 in its root, z4–9 in its blocks, pipeline::polar.)
        if !l.polar.is_empty() {
            let b = pipeline::polar::BLOCK_Z;
            return match z {
                _ if z < b => l.root.clone(),
                _ => l.polar.get(&format!("{b}/{}/{}", x >> (z - b), y >> (z - b))).cloned(),
            };
        }
        match z {
            0..=8 if !self.reaches(z, x, y) => None,
            0..=2 => l.root.clone(),
            3..=8 => l.lo.get(&format!("3/{}/{}", x >> (z - 3), y >> (z - 3))).cloned(),
            // (The terrain's deep levels: its deep packs by z10 tile, pipeline::terrain_deep.)
            _ if z >= pipeline::terrain_deep::FIRST_Z && !l.deep.is_empty() => {
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
                Src::Local(m) => PackIndex::parse_bytes(m.bytes()),
                Src::Remote(r) => PackIndex::read_from(&*r),
            }
        };
        let idx = match &self.mirror {
            Some(m) => m.index(content, load)?,
            None => load()?,
        };
        let idx = Arc::new(idx);
        self.indexes.lock().unwrap().put(content.to_string(), idx.clone(), 0);
        Ok(idx)
    }

    /// A tile of one of our layers: its bytes as stored, and whether they are gzip'd (its pack's flag,
    /// `PackIndex::gzip`).
    pub fn tile(&self, layer: &str, z: u8, x: u32, y: u32) -> Result<Option<(Blob, bool)>> {
        let Some(logical) = self.pack_of(layer, z, x, y) else { return Ok(None) };
        let Some(content) = self.content(&logical) else { return Ok(None) };
        let idx = self.index(&content)?;
        let Some(e) = idx.find(z, x, y) else { return Ok(None) };
        let blob = match self.src(&content)? {
            Src::Local(m) => Blob::Map(m, e.offset as usize, e.len as usize),
            Src::Remote(r) => Blob::from_vec(r.read_at(e.offset, e.len as usize)?),
        };
        Ok(Some((blob, idx.gzip())))
    }

    /// The content hash of a tile, without reading it (for 304s): None when there's no such tile,
    /// an error when its pack's index can't be read.
    pub fn tile_hash(&self, layer: &str, z: u8, x: u32, y: u32) -> Result<Option<u64>> {
        let Some(logical) = self.pack_of(layer, z, x, y) else { return Ok(None) };
        let Some(content) = self.content(&logical) else { return Ok(None) };
        // (A 304 is a use too: the area is in view.)
        Ok(self.index(&content)?.find(z, x, y).map(|e| e.hash))
    }

    /// Each tile at zoom `z` of the pack named `content` (a content name, so of whichever catalogue
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
                    .bytes()
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
        let cat = self.catalogue();
        let Some(l) = cat.layers.get(layer) else { return String::new() };
        let mut h = blake3::Hasher::new();
        for logical in
            l.root.iter().chain(l.lo.values()).chain(l.hi.values()).chain(l.deep.values()).chain(l.polar.values())
        {
            if let Some(c) = self.content(logical) {
                h.update(c.as_bytes());
            }
        }
        h.finalize().to_hex()[..12].to_string()
    }

    /// The basemap's archives in the current catalogue: their content names, in the catalogue's order.
    pub fn basemap_names(&self) -> Vec<String> {
        let cat = self.catalogue();
        cat.basemap.iter().filter_map(|l| cat.files.get(l).map(|f| f.file.clone())).collect()
    }

    /// Where tile z/x/y of the basemap archive `archive` (a content name) is read from: the
    /// mirror's piece holding it when it's here (its path in the mirror, `.basemap/…`; a download
    /// behind the catalogue's piece of its older archive first, as `content`), else the archive itself
    /// (on the NAS while it's read, or whole in the mirror); None away from the NAS with neither here,
    /// or where the downloads don't reach (`reaches`: the zooms 0–10 piece is worldwide).
    pub fn basemap_key(&self, archive: &str, z: u8, x: u32, y: u32) -> Option<String> {
        if !self.reaches(z, x, y) {
            return None;
        }
        let older =
            store::naming::parse_content_name(archive).and_then(|c| self.older.read().unwrap().get(c.logical).cloned());
        let piece = self.mirror.as_ref().zip(store::pieces::Piece::of(z, x, y)).and_then(|(m, p)| {
            older.iter().map(String::as_str).chain([archive]).find(|a| m.has_piece(a, p)).map(|a| (a, p))
        });
        match piece {
            Some((archive, p)) => Some(store::mirror::Mirror::piece_key(archive, p)),
            None if self.online() || self.mirror.as_ref().is_some_and(|m| m.has(archive)) => Some(archive.to_string()),
            None => None,
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
        // (A piece is never modified once in place; removal unlinks it, and `forget_removed` drops
        // the map.)
        let m = Arc::new(MmapFile::open(&mirror.path(key))?);
        self.maps.lock().unwrap().put(key.to_string(), m.clone(), 0);
        Ok(Src::Local(m))
    }

    /// The lead's heartbeat (pipeline::agent::LEAD_STATUS on the NAS), as it wrote it: Null when
    /// there's none or the NAS is away. Read at most every 30 s.
    pub fn lead_status(&self) -> serde_json::Value {
        let mut g = self.lead.lock().unwrap();
        if let Some((t, v)) = g.as_ref() {
            if t.elapsed() < Duration::from_secs(30) {
                return v.clone();
            }
        }
        let v = match (self.nas_root(), self.pool()) {
            (Some(root), Some(pool)) if pool.is_online() => pool
                .read_all(&root.join(pipeline::agent::LEAD_STATUS))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };
        *g = Some((std::time::Instant::now(), v.clone()));
        v
    }

    /// Whether the lead is running a job (its heartbeat, fresh, with a job that isn't paused).
    pub fn agent_busy(&self) -> bool {
        runs(&self.lead_status(), 600, &["job"])
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
                gone.insert(r.id());
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
        self.maps.lock().unwrap().retain(|c, _| !gone.contains(c.as_str()));
        self.sects.lock().unwrap().retain(|c, _| !gone.contains(c.as_str()));
        self.bases.lock().unwrap().retain(|k, _| !k.split('|').any(|c| gone.contains(c)));
        self.his.lock().unwrap().retain(|c, _| !gone.contains(c.as_str()));
        self.marks.lock().unwrap().retain(|c, _| !gone.contains(c.as_str()));
        self.ovs.lock().unwrap().retain(|c, _| !gone.contains(c.as_str()));
        let mut ru = self.roadunits.lock().unwrap();
        if ru.as_ref().is_some_and(|(c, _)| gone.contains(c.as_str())) {
            *ru = None;
        }
        drop(ru);
        self.mirror_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Start the background work: the NAS mounted again while it's away, looked at every 30 s. (The
    /// map's is crate::maplog's thread, the mirror's downloads::spawn_mirror's.)
    pub fn spawn_background(self: &Arc<Self>) {
        let d = self.clone();
        std::thread::Builder::new()
            .name("nas".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                if d.nas_root().is_none() || !d.online() {
                    d.find_nas();
                }
            })
            .ok();
    }
}

/// Where the downloads reach (`Data::reach`).
#[derive(Clone, Debug, PartialEq)]
pub enum Reach {
    /// The World is downloaded: everywhere, zoomed out.
    World,
    /// The z6 tiles of the regions downloaded (none: nothing downloaded).
    Tiles(Arc<HashSet<(u32, u32)>>),
}

impl Default for Reach {
    fn default() -> Self {
        Reach::Tiles(Default::default())
    }
}

/// This Mac's name, for its server's and mirror's cursors in the map's log (crate::maplog).
pub fn host() -> &'static str {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(store::sys::host_name)
}

/// `~/Library/Application Support/scenic`.
pub fn default_home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/scenic")
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
        let mut old = Catalogue::new(1);
        let mut new = Catalogue::new(1);
        let mut w = store::pack::PackWriter::create(&made.join("all.pack"), json!({"bundle": "markdata"}), false)
            .unwrap()
            .aligned(64);
        for (&(x, y), p) in tiles.iter().zip(&files) {
            let b = std::fs::read(p).unwrap();
            let l = format!("markdata/6-{x}-{y}");
            let c = store::naming::write_atomic(
                &store::nasfs::NasRoot::local_for_test(nas.path()),
                &l,
                "sect",
                store::naming::Source::Bytes(&b),
            )
            .unwrap();
            old.files
                .insert(l.clone(), store::catalogue::FileRef { file: c, size: b.len() as u64, ..Default::default() });
            old.markdata.insert(format!("6/{x}/{y}"), l);
            new.markdata.insert(format!("6/{x}/{y}"), "markdata/all".into());
            w.add(6, x, y, &b, b.len() as u32).unwrap();
        }
        w.finish().unwrap();
        let b = std::fs::read(made.join("all.pack")).unwrap();
        let c = store::naming::write_atomic(
            &store::nasfs::NasRoot::local_for_test(nas.path()),
            "markdata/all",
            "pack",
            store::naming::Source::Bytes(&b),
        )
        .unwrap();
        new.files.insert(
            "markdata/all".into(),
            store::catalogue::FileRef { file: c.clone(), size: b.len() as u64, ..Default::default() },
        );
        let open = |home: &std::path::Path, cat: &Catalogue| {
            std::fs::remove_dir_all(nas.path().join(store::map::NAS_DIR)).ok();
            store::testkit::map_root(nas.path(), cat);
            Data::open(Options {
                home: home.to_owned(),
                nas_root: Some(store::nasfs::NasRoot::local_for_test(nas.path().to_owned())),
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
            let m = Arc::new(MmapFile::open(&nas.path().join(&c)).unwrap());
            let e = store::pack::PackIndex::parse_bytes(m.bytes()).unwrap().find(6, x, y).unwrap();
            let s = SectView::open_at(Src::Local(m), e.offset, u64::from(e.len)).unwrap();
            // (Its own name: the blocks are cached by it.)
            let vm = crate::markview::MarkView::new(s, format!("mapped {t}")).unwrap();
            assert_eq!(rows(&vm), rows(&va), "{t} mapped");
        }
        assert!(z.markdata("6/1/1").unwrap().is_none());
    }
}

/// A file's bytes whole and in place (`Data::whole`): mapped from the mirror, or read from the NAS.
pub enum Whole {
    Mapped(Arc<MmapFile>),
    Read(Vec<u8>),
}

impl store::range::Mapped for Whole {
    fn bytes(&self) -> &[u8] {
        match self {
            Whole::Mapped(m) => m.bytes(),
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

/// Whether opening a file failed because it isn't there (store::range::MmapFile's error).
fn not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound))
}

/// Whether a content name is a bundle of files (an RDPACK whose members are files: docs/formats.md
/// "Bundles"), not a file of its own: the markdata's (task #449).
pub fn is_bundle(content: &str) -> bool {
    content.ends_with(".pack")
}
