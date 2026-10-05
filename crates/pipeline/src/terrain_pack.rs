//! Terrain tiles (docs/plan.md §6, global-source layers): AWS's Terrarium tiles, repaired.
//!
//! Levels are made finest first: each tile is repaired (bathymetry to sea level, voids and spikes,
//! `roadcore::grid::repair_terrain`), the pixels above a repaired one are made again from it, and
//! from `REBUILD_Z` down every quarter whose child exists is made again from that child (AWS's
//! coarse levels come from coarser sources and lose peaks). Today's `terrain` step runs this over a
//! region's archive; `scenic-build terrain` runs it per z6 pack.

use det::Det;
use roadcore::grid::{decode_terrain_png, encode_terrain_png, repair_terrain};
use std::collections::HashMap;
use std::time::Duration;

pub const URL: &str = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium";
/// Raw tiles downloaded at once (each mostly waits on S3).
const FETCH_THREADS: usize = 64;

/// An HTTP agent for AWS's tiles, keeping a connection for each of the fetching threads (ureq keeps
/// 3 a host by default: the others would make a new connection, a lookup and a TLS handshake, for
/// every tile).
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent("road-elevations/0.1 (personal offline map)")
        .max_idle_connections(FETCH_THREADS * 2)
        .max_idle_connections_per_host(FETCH_THREADS * 2)
        .build()
        .into()
}

/// One of AWS's tiles; None when it has none (the open sea at fine zooms) or it can't be fetched.
pub fn fetch(agent: &ureq::Agent, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let url = format!("{URL}/{z}/{x}/{y}.png");
    for attempt in 0..5 {
        match agent.get(&url).call() {
            Ok(mut r) => {
                if let Ok(b) = r.body_mut().with_config().limit(20_000_000).read_to_vec() {
                    return Some(b);
                }
            }
            Err(ureq::Error::StatusCode(404 | 403)) => return None,
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(300 << attempt));
    }
    None
}


/// A tile's elevations repaired (bathymetry to sea level, repair_terrain, then the pixels above the
/// repaired ones below made again from them: `below`, the four children's repairs). From REBUILD_Z
/// down, each quarter whose child tile exists is made again whole from it (`quads`: the children's
/// 2×2 means): AWS's coarse levels come from coarser sources, and lost peaks (Fuji's summit pixel:
/// 3,106 m at z6, 2,368 m at z5, 2,134 m at z4; from z9, 3,378, 2,715 and 2,337 m). Returns the PNG
/// to store (the original bytes when nothing changes), its elevations and the pixels that moved if
/// it changed, and from REBUILD_Z + 1 down its 2×2 means for the level above.
pub fn process(
    png: Vec<u8>,
    z: u8,
    x: u32,
    y: u32,
    below: &HashMap<(u32, u32), Repaired>,
    quads: &HashMap<(u32, u32), Vec<f32>>,
) -> (Vec<u8>, Option<Repaired>, Option<Vec<f32>>) {
    let Ok(mut e) = decode_terrain_png(&png) else { return (png, None, None) };
    let before = e.clone();
    for v in e.iter_mut() {
        if *v < 0.0 {
            *v = 0.0;
        }
    }
    for k in 0..4u32 {
        let (dx, dy) = (k & 1, k >> 1);
        let Some(c) = below.get(&(x * 2 + dx, y * 2 + dy)) else { continue };
        for &i in &c.moved {
            let (cx, cy) = ((i % 256) & !1, (i / 256) & !1);
            let m = (c.e[cy * 256 + cx] + c.e[cy * 256 + cx + 1] + c.e[(cy + 1) * 256 + cx] + c.e[(cy + 1) * 256 + cx + 1]) * 0.25;
            e[(dy as usize * 128 + cy / 2) * 256 + dx as usize * 128 + cx / 2] = m;
        }
    }
    if z <= REBUILD_Z {
        for k in 0..4u32 {
            let (dx, dy) = (k & 1, k >> 1);
            let Some(q) = quads.get(&(x * 2 + dx, y * 2 + dy)) else { continue };
            for j in 0..128 {
                let row = (dy as usize * 128 + j) * 256 + dx as usize * 128;
                e[row..row + 128].copy_from_slice(&q[j * 128..(j + 1) * 128]);
            }
        }
    }
    repair_terrain(&mut e, z, tile_lat(z, y));
    let quad = (z >= 1 && z <= REBUILD_Z + 1).then(|| {
        let mut q = vec![0f32; 128 * 128];
        for j in 0..128 {
            for i in 0..128 {
                let (cx, cy) = (i * 2, j * 2);
                q[j * 128 + i] = (e[cy * 256 + cx] + e[cy * 256 + cx + 1] + e[(cy + 1) * 256 + cx] + e[(cy + 1) * 256 + cx + 1]) * 0.25;
            }
        }
        q
    });
    let moved: Vec<usize> = e.iter().zip(&before).enumerate().filter(|(_, (a, b))| !((*a - *b).abs() <= 0.5)).map(|(i, _)| i).collect();
    if moved.is_empty() {
        return (png, None, quad);
    }
    let out = encode_terrain_png(&e, 256, 256).unwrap_or(png);
    (out, Some(Repaired { e, moved }), quad)
}

/// Levels made again from the level below where it exists (process): z8 from z9 (which covers the
/// ground within a z9 tile of a road; averaging our z12 instead gives the same within a few metres).
/// The finer levels stay AWS's, so the analysis grid (z11) and what follows from it don't change.
pub const REBUILD_Z: u8 = 8;

/// A tile changed by process: its elevations and the pixels that moved by more than half a metre.
pub struct Repaired {
    pub e: Vec<f32>,
    pub moved: Vec<usize>,
}


/// Latitude of a tile's centre.
pub fn tile_lat(z: u8, ty: u32) -> f64 {
    let y = (ty as f64 + 0.5) / (1u64 << z) as f64;
    (std::f64::consts::PI * (1.0 - 2.0 * y)).dsinh().datan().to_degrees()
}


// ---- per pack ----------------------------------------------------------------------------------

use crate::coverage::Coverage;
use crate::out::Out;
use rayon::prelude::*;
use std::sync::Mutex;

/// Finest zoom per latitude (pixels stay at least ~15 m): z12 to 67°, z11 to 79°, z10 beyond.
pub fn max_zoom_at(lat: f64) -> u8 {
    match lat.abs() {
        l if l <= 67.0 => 12,
        l if l <= 79.0 => 11,
        _ => 10,
    }
}

/// Whether the coverage comes within `km` of the tile: the tile grown by `km` meets it (exactly, so
/// a small region between sample points isn't missed: Singapore in its z6 tile).
pub fn near_coverage(cov: &Coverage, z: u8, x: u32, y: u32, km: f64) -> bool {
    let b = crate::stage::tile_box_grown(z, x, y, km);
    let e7 = |v: f64| (v * 1e7).round() as i32;
    cov.meets_rect([e7(b[0]), e7(b[1]), e7(b[2]), e7(b[3])])
}

/// A layer's tiles as the build manifest has them now (its latest uploads).
pub struct ManifestTiles<'a> {
    out: &'a Out,
    layer: String,
    open: Mutex<HashMap<String, Option<std::sync::Arc<(std::fs::File, u64, store::pack::PackIndex)>>>>,
}

impl<'a> ManifestTiles<'a> {
    pub fn new(out: &'a Out, layer: &str) -> Self {
        ManifestTiles { out, layer: layer.to_string(), open: Mutex::new(HashMap::new()) }
    }

    pub fn logical(layer: &str, z: u8, x: u32, y: u32) -> String {
        match z {
            0..=2 => format!("layers/{layer}/root/0-0-0"),
            3..=8 => format!("layers/{layer}/lo/3-{}-{}", x >> (z - 3), y >> (z - 3)),
            _ => format!("layers/{layer}/hi/6-{}-{}", x >> (z - 6), y >> (z - 6)),
        }
    }

    /// The open pack holding tile (z, x, y), if the manifest has it.
    fn pack(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<std::sync::Arc<(std::fs::File, u64, store::pack::PackIndex)>>> {
        let logical = Self::logical(&self.layer, z, x, y);
        let mut open = self.open.lock().unwrap();
        if !open.contains_key(&logical) {
            let v = match self.out.get(&logical) {
                Some(c) => {
                    let f = std::fs::File::open(self.out.path(c))?;
                    let len = f.metadata()?.len();
                    let src = FileSource(&f, len);
                    let idx = store::pack::PackIndex::read_from(&src)?;
                    Some(std::sync::Arc::new((f, len, idx)))
                }
                None => None,
            };
            open.insert(logical.clone(), v);
        }
        Ok(open.get(&logical).cloned().flatten())
    }

    pub fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
        use store::sys::PosIo;
        let Some(e) = self.pack(z, x, y)? else { return Ok(None) };
        let Some(ent) = e.2.find(z, x, y) else { return Ok(None) };
        let mut b = vec![0u8; ent.len as usize];
        e.0.read_exact_at(&mut b, ent.offset)?;
        Ok(Some(b))
    }

    /// Whether the layer has tile (z, x, y), from its pack's index alone.
    pub fn has(&self, z: u8, x: u32, y: u32) -> anyhow::Result<bool> {
        Ok(self.pack(z, x, y)?.is_some_and(|e| e.2.find(z, x, y).is_some()))
    }
}

struct FileSource<'a>(&'a std::fs::File, u64);

impl store::range::RangeRead for FileSource<'_> {
    fn len(&self) -> Result<u64, store::iopool::IoError> {
        Ok(self.1)
    }
    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::iopool::IoError> {
        use store::sys::PosIo;
        let mut b = vec![0u8; len];
        self.0.read_exact_at(&mut b, off).map_err(store::iopool::IoError::Io)?;
        Ok(b)
    }
}

/// An area's raw tile archives as read (RawTiles): its first tiles straight from the NAS's (each
/// archive's entries read once, then a ranged read a tile), so a job wanting a tile or two of an
/// area doesn't copy hundreds of MB; past RANGED of them, copied here whole and opened (one large
/// read each, then local, as a terrain job reads thousands). One whose copy failed is read by range
/// for the rest of the job (`stay`), not copied again for each tile.
enum Opened {
    Ranged { archives: Vec<(std::fs::File, Vec<roadcore::archive::Entry>)>, reads: usize, stay: bool },
    Here(std::sync::Arc<[roadcore::archive::Archive]>),
}

/// The tiles of an area read straight from the NAS's archives before they're copied here whole.
const RANGED: usize = 16;

/// AWS's raw tiles, kept so each is downloaded once: in the build Mac's cache as they come, then on
/// the NAS packed (crate::rawpack: archives grouped as the terrain is, packed at the end of the job
/// that fetched them, or by room-making), read from there (`Opened`: a few tiles of an area by
/// range, else its archives copied here whole); and the NAS's loose tiles from before (`sources/aws-terrarium/<z>/<x>/<y>.png`,
/// `.none` for a tile AWS doesn't have), while they're there. Packs are always made from the same
/// immutable source: processing a tile twice isn't idempotent, so stored (processed) tiles are never
/// an input. Each loose copy is written straight to its name (crate::whole::write_in_place); every
/// tile is checked whole when read, a loose one or an archive's: one that isn't (cut short) is
/// passed over (a loose one deleted) and taken from the next source, the NAS's copy, else AWS.
pub struct RawTiles {
    dir: std::path::PathBuf,
    store: Option<std::path::PathBuf>,
    agent: ureq::Agent,
    /// The store's columns (`<z>/<x>/`) as listed once, and the folders made, here and there: over
    /// SMB each look or mkdir is a round trip, and those, a tile's few, set a terrain job's pace.
    listed: Mutex<HashMap<(u8, u32), std::sync::Arc<std::collections::HashSet<String>>>>,
    made: Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// The store's archives' index: read when first wanted (tried again a minute after it can't
    /// be), and again when an archive it names is gone from the NAS (a job that outlived it).
    index: Mutex<(Option<std::sync::Arc<crate::rawpack::Index>>, Option<std::time::Instant>)>,
    /// Each area's archives, newest first, opened when first wanted: by the area's own lock, so
    /// reading one from the NAS holds up only the tiles of that area.
    archives: Mutex<HashMap<String, std::sync::Arc<Mutex<Option<Opened>>>>>,
}

impl RawTiles {
    /// A local cache alone (no NAS store).
    pub fn new(dir: &std::path::Path) -> Self {
        RawTiles { dir: dir.to_path_buf(), store: None, agent: agent(), listed: Default::default(), made: Default::default(), index: Default::default(), archives: Default::default() }
    }

    /// The local cache `dir`, filled from the NAS's `store` where it has a tile.
    pub fn with_store(dir: &std::path::Path, store: &std::path::Path) -> Self {
        RawTiles { dir: dir.to_path_buf(), store: Some(store.to_path_buf()), agent: agent(), listed: Default::default(), made: Default::default(), index: Default::default(), archives: Default::default() }
    }

    /// The names in the store's column `z/x`, listed once (none when it isn't there); None when it
    /// can't be listed whole now (then each tile is looked for itself, and it's listed again later).
    fn column(&self, st: &std::path::Path, z: u8, x: u32) -> Option<std::sync::Arc<std::collections::HashSet<String>>> {
        if let Some(c) = self.listed.lock().unwrap().get(&(z, x)) {
            return Some(c.clone());
        }
        let names: std::collections::HashSet<String> = match std::fs::read_dir(st.join(format!("{z}/{x}"))) {
            Ok(rd) => rd.map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned())).collect::<std::io::Result<_>>().ok()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(_) => return None,
        };
        let c = std::sync::Arc::new(names);
        self.listed.lock().unwrap().insert((z, x), c.clone());
        Some(c)
    }

    /// The store's archives' index: as read before unless `fresh`; None when it can't be read (three
    /// tries a moment apart, then none for a minute: each try waits on the NAS).
    fn index(&self, st: &std::path::Path, fresh: bool) -> Option<std::sync::Arc<crate::rawpack::Index>> {
        let mut s = self.index.lock().unwrap();
        if let (Some(ix), false) = (&s.0, fresh) {
            return Some(ix.clone());
        }
        if s.1.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return s.0.clone();
        }
        for attempt in 0..3 {
            match crate::rawpack::Index::load(st) {
                Ok(ix) => {
                    *s = (Some(std::sync::Arc::new(ix)), None);
                    return s.0.clone();
                }
                Err(e) if attempt == 2 => eprintln!("terrain: the raw tiles' archives: {e:#}"),
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        s.1 = Some(std::time::Instant::now());
        s.0.clone()
    }

    /// An area's archives, newest first: here when each has its copy here, else to be read by range
    /// (each one's entries read now, from its copy here or the NAS's). None when one can't be (not
    /// remembered: the NAS may answer next time). An archive the index names that's gone from the
    /// NAS (a job that outlived the index it read) has the index read again.
    fn open_area(&self, st: &std::path::Path, area: &str) -> Option<Opened> {
        for fresh in [false, true] {
            let index = self.index(st, fresh)?;
            let packs = index.of(area);
            if packs.iter().all(|p| self.dir.join("packs").join(&p.name).exists()) {
                if let Some(here) = self.copy_area(st, packs) {
                    return Some(Opened::Here(here));
                }
            }
            let mut v = Vec::new();
            for p in packs.iter().rev() {
                let local = self.dir.join("packs").join(&p.name);
                let at = if local.exists() {
                    touch(&local);
                    local
                } else {
                    st.join("packs").join(&p.name)
                };
                match crate::rawpack::entries_of(&at) {
                    Ok(e) => v.push(e),
                    // (Gone from the NAS: the index read before is out of date.)
                    Err(e) if !fresh && crate::rawpack::not_found(&e) => break,
                    Err(e) => {
                        eprintln!("terrain: the raw tiles of {area}: {e:#}");
                        return None;
                    }
                }
            }
            if v.len() == packs.len() {
                return Some(Opened::Ranged { archives: v, reads: 0, stay: false });
            }
        }
        None
    }

    /// Archives `packs`' copies here, newest first, each copied from the NAS whole the first time
    /// (checked against its name) and marked used (room-making deletes the least recently used);
    /// None when one can't be.
    fn copy_area(&self, st: &std::path::Path, packs: &[crate::rawpack::Pack]) -> Option<std::sync::Arc<[roadcore::archive::Archive]>> {
        let mut v = Vec::new();
        for p in packs.iter().rev() {
            let opened = crate::rawpack::local_copy(&self.dir, st, &p.name).and_then(|l| {
                touch(&l);
                roadcore::archive::Archive::open(&l)
            });
            match opened {
                Ok(a) => v.push(a),
                Err(e) => {
                    eprintln!("terrain: raw tile archive {}: {e:#}", p.name);
                    return None;
                }
            }
        }
        Some(v.into())
    }

    /// A tile from its area's archives: Some(None) when AWS hasn't it, None when they (or the
    /// store) haven't it whole.
    fn archived(&self, z: u8, x: u32, y: u32) -> Option<Option<Vec<u8>>> {
        let st = self.store.as_ref()?;
        let area = crate::rawpack::area(z, x, y);
        let cell = self.archives.lock().unwrap().entry(area.clone()).or_default().clone();
        let key = roadcore::archive::tile_key(z, x, y);
        let whole = |b: &[u8]| -> Option<Option<Vec<u8>>> {
            if b.is_empty() {
                Some(None)
            } else if crate::whole::png_whole(b) {
                Some(Some(b.to_vec()))
            } else {
                eprintln!("terrain: {z}/{x}/{y} in an archive of {area} isn't whole: passed over");
                None
            }
        };
        let here = {
            let mut c = cell.lock().unwrap();
            if c.is_none() {
                *c = Some(self.open_area(st, &area)?);
            }
            // (Read a few times: copied here whole, if it can be; else read by range from now on.)
            if let Some(Opened::Ranged { reads, stay: stay @ false, .. }) = c.as_mut() {
                if *reads >= RANGED {
                    match self.index(st, false).and_then(|index| self.copy_area(st, index.of(&area))) {
                        Some(h) => *c = Some(Opened::Here(h)),
                        None => *stay = true,
                    }
                }
            }
            match c.as_mut()? {
                Opened::Here(h) => h.clone(),
                Opened::Ranged { archives, reads, .. } => {
                    *reads += 1;
                    for (f, entries) in archives.iter() {
                        let Ok(i) = entries.binary_search_by_key(&key, |e| e.key) else { continue };
                        let mut b = vec![0u8; entries[i].len as usize];
                        if let Err(e) = store::sys::PosIo::read_exact_at(f, &mut b, entries[i].offset) {
                            eprintln!("terrain: {z}/{x}/{y} from an archive of {area}: {e}");
                            return None;
                        }
                        if let Some(t) = whole(&b) {
                            return Some(t);
                        }
                    }
                    return None;
                }
            }
        };
        here.iter().find_map(|a| a.get(z, x, y).and_then(|b| whole(b)))
    }

    /// Makes folder `d` (once).
    fn make(&self, d: &std::path::Path) -> std::io::Result<()> {
        if self.made.lock().unwrap().contains(d) {
            return Ok(());
        }
        std::fs::create_dir_all(d)?;
        self.made.lock().unwrap().insert(d.to_path_buf());
        Ok(())
    }

    /// The raw tile, and whether it came from AWS just now.
    pub fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
        let d = self.dir.join(format!("{z}/{x}"));
        let p = d.join(format!("{y}.png"));
        if let Some(b) = read_whole(&p) {
            return Ok((Some(b), false));
        }
        let none = d.join(format!("{y}.none"));
        if none.exists() {
            return Ok((None, false));
        }
        // In its area's archive.
        if let Some(b) = self.archived(z, x, y) {
            return Ok((b, false));
        }
        self.make(&d)?;
        // On the NAS (its column listed once): copied here.
        if let Some(st) = &self.store {
            let sd = st.join(format!("{z}/{x}"));
            let col = self.column(st, z, x);
            let has = |n: String| col.as_ref().map_or_else(|| sd.join(&n).exists(), |c| c.contains(&n));
            if has(format!("{y}.png")) {
                if let Some(b) = read_whole(&sd.join(format!("{y}.png"))) {
                    crate::whole::write_in_place(&p, &b)?;
                    return Ok((Some(b), false));
                }
            }
            if has(format!("{y}.none")) {
                std::fs::write(&none, b"")?;
                return Ok((None, false));
            }
        }
        self.fetch(z, x, y).map(|b| (b, true))
    }

    /// Fetches the tiles of `tiles` (zoom `z`) not here yet, `threads` at a time: a download mostly
    /// waits on AWS, so far more of them than cores. The number that came from AWS.
    pub fn prefetch(&self, z: u8, tiles: &[(u32, u32)], threads: usize) -> anyhow::Result<usize> {
        use rayon::prelude::*;
        let todo: Vec<(u32, u32)> = tiles
            .iter()
            .copied()
            .filter(|&(x, y)| {
                let d = self.dir.join(format!("{z}/{x}"));
                !d.join(format!("{y}.png")).exists() && !d.join(format!("{y}.none")).exists()
            })
            .collect();
        if todo.is_empty() {
            return Ok(0);
        }
        let fetched = std::sync::atomic::AtomicUsize::new(0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
        pool.install(|| {
            todo.par_iter().try_for_each(|&(x, y)| -> anyhow::Result<()> {
                if self.get(z, x, y)?.1 {
                    fetched.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(())
            })
        })?;
        Ok(fetched.into_inner())
    }

    /// The raw tile fetched again (a cached one that doesn't decode), replacing the cached one.
    pub fn refetch(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let d = self.dir.join(format!("{z}/{x}"));
        std::fs::remove_file(d.join(format!("{y}.png"))).ok();
        std::fs::create_dir_all(&d)?;
        self.fetch(z, x, y)
    }

    /// From AWS, into the local cache: the NAS's store gets them in bulk (its small-file writes, a
    /// tile at a time, set the job's pace: 25 a second against 119 here).
    fn fetch(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let d = self.dir.join(format!("{z}/{x}"));
        match fetch_checked(&self.agent, z, x, y)? {
            Some(b) => {
                crate::whole::write_in_place(&d.join(format!("{y}.png")), &b)?;
                Ok(Some(b))
            }
            None => {
                std::fs::write(d.join(format!("{y}.none")), b"")?;
                Ok(None)
            }
        }
    }
}

/// Marks a copy of an archive used (room-making deletes the least recently used first).
fn touch(p: &std::path::Path) {
    std::fs::File::options().append(true).open(p).and_then(|f| f.set_modified(std::time::SystemTime::now())).ok();
}

/// A kept tile's bytes when it's there and whole; one that isn't whole is deleted.
fn read_whole(p: &std::path::Path) -> Option<Vec<u8>> {
    let b = std::fs::read(p).ok()?;
    if crate::whole::png_whole(&b) {
        return Some(b);
    }
    eprintln!("terrain: {} isn't whole ({} bytes): fetched again", p.display(), b.len());
    std::fs::remove_file(p).ok();
    None
}

/// One of AWS's tiles, whole: None when AWS says it has none (404, or S3's 403), twice, a moment
/// apart (it's remembered for good); an error when it can't be fetched (so a network failure isn't
/// remembered as "no tile").
pub fn fetch_checked(agent: &ureq::Agent, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let url = format!("{URL}/{z}/{x}/{y}.png");
    let mut last = None;
    let mut missing = 0;
    for attempt in 0..5 {
        match agent.get(&url).call() {
            Ok(mut r) => match r.body_mut().with_config().limit(20_000_000).read_to_vec() {
                Ok(b) if crate::whole::png_whole(&b) => return Ok(Some(b)),
                Ok(b) => last = Some(anyhow::anyhow!("{url}: not a whole PNG ({} bytes)", b.len())),
                Err(e) => last = Some(anyhow::anyhow!("{url}: {e}")),
            },
            Err(ureq::Error::StatusCode(c @ (404 | 403))) => {
                missing += 1;
                if missing == 2 {
                    return Ok(None);
                }
                last = Some(anyhow::anyhow!("{url}: {c}"));
            }
            Err(e) => last = Some(anyhow::anyhow!("{url}: {e}")),
        }
        std::thread::sleep(Duration::from_millis(if missing > 0 { 2000 } else { 300 << attempt }));
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("{url}: no answer")))
}

#[derive(Debug, Default, serde::Serialize)]
pub struct PackReport {
    pub hi_tiles: usize,
    pub lo_tiles: usize,
    pub fetched: usize,
    pub missing: usize,
    pub repaired: usize,
}

/// Makes the terrain of the z6 tiles `ts` (all in z3 tile `q`) near the coverage: their hi packs
/// (z9–12, or coarser at high latitudes), then `q`'s lo pack (z3–8) with them folded in. Always
/// from AWS's raw tiles (`raw`, cached locally), so the same coverage gives the same bytes.
pub fn build_q(out: &mut Out, raw: &RawTiles, q: (u32, u32), ts: &[(u32, u32)], cov: &Coverage) -> anyhow::Result<PackReport> {
    build_q_with(out, raw, q, ts, cov, &|_, _, _| {})
}

/// `build_q`, saying how far it is (`progress`): the area's tiles fetched and shaded, every level's
/// ("tiles", every few seconds), then its packs written ("packs").
pub fn build_q_with(out: &mut Out, raw: &RawTiles, q: (u32, u32), ts: &[(u32, u32)], cov: &Coverage, progress: crate::rawpack::Progress) -> anyhow::Result<PackReport> {
    let mut rep = PackReport::default();
    // Each level's tiles, z12 → z3: z12 → z9 inside each z6 tile, near the coverage, as fine as the
    // latitude allows; z8 → z3 the whole of q.
    let mut levels: Vec<(u8, Vec<(u32, u32)>)> = Vec::new();
    for z in (9..=12u8).rev() {
        let mut tiles = Vec::new();
        for &(tx, ty) in ts {
            let s = 1u32 << (z - 6);
            for x in tx * s..(tx + 1) * s {
                for y in ty * s..(ty + 1) * s {
                    if z <= max_zoom_at(tile_lat(z, y)) && near_coverage(cov, z, x, y, 20.0) {
                        tiles.push((x, y));
                    }
                }
            }
        }
        levels.push((z, tiles));
    }
    for z in (3..=8u8).rev() {
        let s = 1u32 << (z - 3);
        levels.push((z, (q.0 * s..(q.0 + 1) * s).flat_map(|x| (q.1 * s..(q.1 + 1) * s).map(move |y| (x, y))).collect()));
    }
    let total: u64 = levels.iter().map(|(_, t)| t.len() as u64).sum();
    let processed = std::sync::atomic::AtomicU64::new(0);
    let fetched = std::sync::atomic::AtomicUsize::new(0);
    let missing = std::sync::atomic::AtomicUsize::new(0);
    let repaired = std::sync::atomic::AtomicUsize::new(0);
    let get = |z: u8, x: u32, y: u32| -> anyhow::Result<Option<Vec<u8>>> {
        let (b, new) = raw.get(z, x, y)?;
        if new {
            fetched.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if b.is_none() {
            missing.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(b)
    };
    // One level: every tile fetched or reused, then processed with what the level below made.
    let level = |z: u8, tiles: Vec<(u32, u32)>, below: &HashMap<(u32, u32), Repaired>, quads: &HashMap<(u32, u32), Vec<f32>>| -> anyhow::Result<(Vec<(u32, u32, Vec<u8>)>, HashMap<(u32, u32), Repaired>, HashMap<(u32, u32), Vec<f32>>)> {
        fetched.fetch_add(raw.prefetch(z, &tiles, FETCH_THREADS)?, std::sync::atomic::Ordering::Relaxed);
        let done: Vec<anyhow::Result<Option<(u32, u32, Vec<u8>, Option<Repaired>, Option<Vec<f32>>)>>> = tiles
            .par_iter()
            .map(|&(x, y)| {
                let got = get(z, x, y);
                processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(b) = got? else { return Ok(None) };
                let (b, r, q) = process(b, z, x, y, below, quads);
                Ok(Some((x, y, b, r, q)))
            })
            .collect();
        let (mut outs, mut nb, mut nq) = (Vec::new(), HashMap::new(), HashMap::new());
        for d in done {
            if let Some((x, y, b, r, q)) = d? {
                if let Some(r) = r {
                    repaired.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    nb.insert((x, y), r);
                }
                if let Some(q) = q {
                    nq.insert((x, y), q);
                }
                outs.push((x, y, b));
            }
        }
        Ok((outs, nb, nq))
    };
    let mut hi: HashMap<(u32, u32), Vec<(u8, u32, u32, Vec<u8>)>> = HashMap::new();
    let mut lo: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
    // The levels, each with what the level below made (z8 → z3 fold in the levels above what was
    // made just before them); the tiles done said every few seconds meanwhile.
    let finished = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut said = std::time::Instant::now();
            while !finished.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                if said.elapsed() >= Duration::from_secs(5) {
                    progress("tiles", processed.load(std::sync::atomic::Ordering::Relaxed), total);
                    said = std::time::Instant::now();
                }
            }
        });
        let r = (|| -> anyhow::Result<()> {
            let mut below: HashMap<(u32, u32), Repaired> = HashMap::new();
            let mut quads: HashMap<(u32, u32), Vec<f32>> = HashMap::new();
            for (z, tiles) in levels {
                let (outs, nb, nq) = level(z, tiles, &below, &quads)?;
                for (x, y, b) in outs {
                    if z >= 9 {
                        hi.entry((x >> (z - 6), y >> (z - 6))).or_default().push((z, x, y, b));
                    } else {
                        lo.push((z, x, y, b));
                    }
                }
                (below, quads) = (nb, nq);
            }
            Ok(())
        })();
        finished.store(true, std::sync::atomic::Ordering::Relaxed);
        r
    })?;
    progress("tiles", total, total);
    rep.fetched = fetched.into_inner();
    rep.missing = missing.into_inner();
    rep.repaired = repaired.into_inner();
    // Upload: each z6 tile's hi pack, then q's lo pack.
    let packs = ts.len() as u64 + 1;
    for (i, &(tx, ty)) in ts.iter().enumerate() {
        progress("packs", i as u64, packs);
        let mut tiles = hi.remove(&(tx, ty)).unwrap_or_default();
        tiles.sort_by_key(|t| (t.0, t.1, t.2));
        rep.hi_tiles += tiles.len();
        let mut it = tiles.into_iter().map(|(z, x, y, b)| {
            let n = b.len() as u32;
            (z, x, y, b, n)
        });
        crate::layers::write_pack(out, "terrain", "terrarium-png", false, "hi", (6, tx, ty), &mut it)?;
    }
    progress("packs", ts.len() as u64, packs);
    lo.sort_by_key(|t| (t.0, t.1, t.2));
    rep.lo_tiles = lo.len();
    let mut it = lo.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    crate::layers::write_pack(out, "terrain", "terrarium-png", false, "lo", (3, q.0, q.1), &mut it)?;
    out.save()?;
    progress("packs", packs, packs);
    Ok(rep)
}

/// The root pack (z0–2) remade from the 64 z3 tiles as stored in the lo packs (their 2×2 means),
/// over AWS's raw z0–2 tiles; deterministic given the lo packs.
pub fn build_root(out: &mut Out, raw: &RawTiles) -> anyhow::Result<usize> {
    let have = ManifestTiles::new(out, "terrain");
    let mut quads: HashMap<(u32, u32), Vec<f32>> = HashMap::new();
    for x in 0..8u32 {
        for y in 0..8u32 {
            let stored = match have.get(3, x, y)? {
                Some(b) => Some(b),
                None => raw.get(3, x, y)?.0,
            };
            let Some(b) = stored else { continue };
            let Ok(e) = decode_terrain_png(&b) else { continue };
            let mut q = vec![0f32; 128 * 128];
            for j in 0..128 {
                for i in 0..128 {
                    let (cx, cy) = (i * 2, j * 2);
                    q[j * 128 + i] = (e[cy * 256 + cx] + e[cy * 256 + cx + 1] + e[(cy + 1) * 256 + cx] + e[(cy + 1) * 256 + cx + 1]) * 0.25;
                }
            }
            quads.insert((x, y), q);
        }
    }
    let mut tiles: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
    let below: HashMap<(u32, u32), Repaired> = HashMap::new();
    for z in (0..=2u8).rev() {
        let n = 1u32 << z;
        let mut next = HashMap::new();
        for x in 0..n {
            for y in 0..n {
                let Some(b) = raw.get(z, x, y)?.0 else { continue };
                let (b, _, q) = process(b, z, x, y, &below, &quads);
                if let Some(q) = q {
                    next.insert((x, y), q);
                }
                let l = b.len() as u32;
                tiles.push((z, x, y, b, l));
            }
        }
        quads = next;
    }
    drop(have);
    tiles.sort_by_key(|t| (t.0, t.1, t.2));
    let n = tiles.len();
    let mut it = tiles.into_iter();
    crate::layers::write_pack(out, "terrain", "terrarium-png", false, "root", (0, 0, 0), &mut it)?;
    out.save()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_says_how_far_it_is() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch, local) = (d.path().join("root"), d.path().join("scratch"), d.path().join("local"));
        // Every tile of 3/1/1 from z8 to z3 known to be missing (no AWS here), and no z6 tiles.
        for z in 3..=8u8 {
            let s = 1u32 << (z - 3);
            for x in s..2 * s {
                std::fs::create_dir_all(local.join(format!("{z}/{x}"))).unwrap();
                for y in s..2 * s {
                    std::fs::write(local.join(format!("{z}/{x}/{y}.none")), b"").unwrap();
                }
            }
        }
        let raw = RawTiles::with_store(&local, &d.path().join("store"));
        let cov = Coverage::from_recipes(&[], None, d.path()).unwrap();
        let mut out = Out::open(&root, &scratch).unwrap();
        let said = Mutex::new(Vec::new());
        build_q_with(&mut out, &raw, (1, 1), &[], &cov, &|w, d, t| said.lock().unwrap().push((w.to_string(), d, t))).unwrap();
        let said = said.into_inner().unwrap();
        // Every level's tiles (1 + 4 + … + 1024), then the one pack (lo), done; never past a total.
        assert!(said.contains(&("tiles".to_string(), 1365, 1365)));
        assert_eq!(said.last(), Some(&("packs".to_string(), 1, 1)));
        assert!(said.iter().all(|(_, d, t)| d <= t));
    }

    #[test]
    fn raw_tiles_from_the_store() {
        let d = tempfile::tempdir().unwrap();
        let (local, store) = (d.path().join("local"), d.path().join("store"));
        let png = crate::whole::testfiles::png();
        std::fs::create_dir_all(store.join("9/5")).unwrap();
        std::fs::write(store.join("9/5/7.png"), &png).unwrap();
        std::fs::write(store.join("9/5/8.none"), b"").unwrap();
        // A tile cut short in the store: deleted, so it's taken again (from AWS: not reached here).
        std::fs::write(store.join("9/5/9.png"), &png[..png.len() / 2]).unwrap();
        let raw = RawTiles::with_store(&local, &store);
        let (b, fetched) = raw.get(9, 5, 7).unwrap();
        assert_eq!((b.as_deref(), fetched), (Some(&png[..]), false));
        assert_eq!(std::fs::read(local.join("9/5/7.png")).unwrap(), png, "copied here");
        assert_eq!(raw.get(9, 5, 8).unwrap(), (None, false));
        assert!(local.join("9/5/8.none").exists());
        assert!(read_whole(&store.join("9/5/9.png")).is_none() && !store.join("9/5/9.png").exists());
    }

    #[test]
    fn raw_tiles_from_their_areas_archive() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let store = root.join("sources/aws-terrarium");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let png = crate::whole::testfiles::png();
        // Tiles packed onto the NAS from another cache (a minute old: packable).
        let other = d.path().join("other");
        for (rel, b) in [("12/2048/1365.png", &png[..]), ("12/2049/1365.none", &[][..])] {
            std::fs::create_dir_all(other.join(rel).parent().unwrap()).unwrap();
            std::fs::write(other.join(rel), b).unwrap();
            std::fs::File::options().append(true).open(other.join(rel)).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(120)).unwrap();
        }
        assert_eq!(crate::rawpack::pack_local(&other, &store, &root, true).unwrap(), 2);
        // A fresh cache reads them from their area's archive on the NAS, a tile at a time (no loose
        // file here or there, no copy of the archive yet)...
        let local = d.path().join("local");
        let raw = RawTiles::with_store(&local, &store);
        assert_eq!(raw.get(12, 2048, 1365).unwrap(), (Some(png.clone()), false));
        assert_eq!(raw.get(12, 2049, 1365).unwrap(), (None, false), "AWS hasn't it");
        let copies = || std::fs::read_dir(local.join("packs")).map_or(0, |d| d.count());
        assert!(!local.join("12/2048/1365.png").exists() && copies() == 0);
        // ...and copies it here whole once it's read more.
        for _ in 0..RANGED {
            assert_eq!(raw.get(12, 2048, 1365).unwrap(), (Some(png.clone()), false));
        }
        assert_eq!(copies(), 1);
        assert_eq!(raw.get(12, 2049, 1365).unwrap(), (None, false));
        // Another job finds the copy here.
        let again = RawTiles::with_store(&local, &store);
        assert_eq!(again.get(12, 2048, 1365).unwrap(), (Some(png.clone()), false));
    }
}
