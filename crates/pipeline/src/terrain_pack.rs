//! Terrain tiles (docs/plan.md §6, global-source layers): AWS's Terrarium tiles, repaired.
//!
//! Levels are made finest first: each tile is repaired (voids, towers and pits:
//! `roadcore::grid::repair_terrain`; then bathymetry to sea level), the pixels above a repaired one
//! are made again from it, and
//! from `REBUILD_Z` down every quarter whose child exists is made again from that child (AWS's
//! coarse levels come from coarser sources and lose peaks). Today's `terrain` step runs this over a
//! region's archive; `scenic-build terrain` runs it per z6 pack.

use det::Det;
use roadcore::grid::{decode_terrain_png, encode_terrain_png, repair_terrain_with};
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
        .user_agent("scenic-roads/0.1 (personal offline map)")
        .max_idle_connections(FETCH_THREADS * 2)
        .max_idle_connections_per_host(FETCH_THREADS * 2)
        .build()
        .into()
}

/// One of AWS's tiles; None when it has none (the open sea at fine zooms) or it can't be fetched.
pub fn fetch(agent: &ureq::Agent, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let url = format!("{URL}/{z}/{x}/{y}.png");
    crate::fetch::online(&url).ok()?;
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


/// What the terrain is made from besides AWS's tiles (docs/plan.md §6, Terrain): GLO-30 north of
/// 60°N (crate::terrain_north), OSM's water (crate::terrain_water), and AWS's z9 tiles for the
/// walled patches' rule (roadcore::grid::walled_patches). None of them: AWS's tiles repaired alone.
#[derive(Clone, Copy, Default)]
pub struct Sources<'a> {
    pub north: Option<&'a dyn crate::terrain_north::Cells>,
    pub water: Option<&'a dyn crate::terrain_water::WaterSource>,
    pub coarse: Option<&'a Coarse<'a>>,
}

impl Sources<'_> {
    /// What pins them, for the terrain's key: GLO-30's and the water's.
    pub fn pin(&self) -> String {
        format!("north {} | water {}", self.north.map_or("-".into(), |n| n.pin()), self.water.map_or("-".into(), |w| w.pin()))
    }
}

/// The basemap the terrain's water comes from: the latest pass's (`layers/basemap/world-<date>`),
/// its logical and content names. The terrain's keys name its water by digests of what each target
/// reads of it (`water_source_pin`, agent::build), so a new pass's basemap makes again the terrain
/// whose water changed.
pub fn water_pin(m: &std::collections::BTreeMap<String, String>) -> Option<(&str, &str)> {
    let prefix = "layers/basemap/world-";
    m.range(prefix.to_string()..).take_while(|(l, _)| l.starts_with(prefix)).last().map(|(l, c)| (l.as_str(), c.as_str()))
}

/// The pin of the terrain's water source as the jobs open it (`SourceFiles`: `WaterSource::pin`),
/// from the manifest alone: the latest pass's basemap's content name; None without one. What the
/// terrain's keys name the water by (agent::build: its digests, crate::terrain_water::WaterIdx).
pub fn water_source_pin(m: &std::collections::BTreeMap<String, String>) -> Option<String> {
    water_pin(m).map(|(_, c)| c.to_string())
}

/// The terrain's water source, open (its pin `water_source_pin`'s): the latest pass's basemap.
pub fn open_water(out: &Out) -> anyhow::Result<crate::terrain_water::BasemapWater> {
    use anyhow::Context;
    let (_, c) = water_pin(&out.manifest).context("no basemap (layers/basemap/world-<date>): the terrain's water is the latest pass's basemap's")?;
    crate::terrain_water::BasemapWater::open(&out.path(c), c)
}

/// GLO-30 as the terrain reads it (the bucket's tiles of May 2022), in the terrain's key.
pub const NORTH_PIN: &str = "glo30 2022-05";

/// The sources' files, open: GLO-30's store (the NAS's `sources/copernicus-dem/`) and the pass's
/// basemap (`layers/basemap/world-<date>`, its content name pinning it).
pub struct SourceFiles {
    pub north: crate::terrain_north::GloStore,
    pub water: crate::terrain_water::BasemapWater,
}

impl SourceFiles {
    /// Under `out`'s root, the latest pass's basemap; GLO-30's tiles fetched into the store when
    /// it lacks one the coverage wants (`fetch`).
    pub fn open(out: &Out, fetch: bool) -> anyhow::Result<SourceFiles> {
        let _p = crate::timings::phase("GLO-30's store and the basemap's water opened", crate::timings::Class::NasRead);
        let north = crate::terrain_north::GloStore::open(&out.root().join("sources/copernicus-dem"), fetch)?;
        Ok(SourceFiles { north, water: open_water(out)? })
    }

    pub fn sources<'a>(&'a self, coarse: Option<&'a Coarse<'a>>) -> Sources<'a> {
        Sources { north: Some(&self.north), water: Some(&self.water), coarse }
    }
}

/// Where a run reads AWS's raw tiles: this Mac's (`RawTiles`: its cache, the NAS's archives, else
/// AWS), or a task's own (crate::terrain_task: those its z8 subtrees read, cut by the job).
pub trait RawSource: Sync {
    /// The raw tile (None: AWS has none), and whether it came from AWS just now.
    fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)>;
    /// Makes the tiles of `tiles` (zoom `z`) ready to read, counting each into `done` as it is; the
    /// number that came from AWS.
    fn prefetch_counted(&self, z: u8, tiles: &[(u32, u32)], threads: usize, done: &std::sync::atomic::AtomicU64) -> anyhow::Result<usize>;
}

impl RawSource for RawTiles {
    fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
        RawTiles::get(self, z, x, y)
    }
    fn prefetch_counted(&self, z: u8, tiles: &[(u32, u32)], threads: usize, done: &std::sync::atomic::AtomicU64) -> anyhow::Result<usize> {
        RawTiles::prefetch_counted(self, z, tiles, threads, done)
    }
}

/// AWS's z9 tiles, raw and decoded, the last few kept: the walled patches' rule compares a z10–12
/// tile with the z9 one over it.
pub struct Coarse<'a> {
    pub raw: &'a dyn RawSource,
    kept: Mutex<(u64, HashMap<(u32, u32), (u64, Option<std::sync::Arc<Vec<f32>>>)>)>,
}

impl<'a> Coarse<'a> {
    pub fn new(raw: &'a dyn RawSource) -> Self {
        Coarse { raw, kept: Mutex::new((0, HashMap::new())) }
    }

    /// AWS's z9 tile over z/x/y, upsampled onto it (z10–12); None elsewhere or when AWS has none.
    pub fn over(&self, z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
        if !(10..=12).contains(&z) {
            return None;
        }
        let dz = z - 9;
        let k = (x >> dz, y >> dz);
        let got = {
            let mut g = self.kept.lock().unwrap();
            g.0 += 1;
            let tick = g.0;
            match g.1.get_mut(&k) {
                Some(e) => {
                    e.0 = tick;
                    Some(e.1.clone())
                }
                None => None,
            }
        };
        let tile = match got {
            Some(t) => t,
            None => {
                let t = self.raw.get(9, k.0, k.1).ok().and_then(|(b, _)| b).and_then(|b| decode_terrain_png(&b).ok()).map(std::sync::Arc::new);
                let mut g = self.kept.lock().unwrap();
                g.0 += 1;
                let tick = g.0;
                if g.1.len() >= 256 {
                    if let Some(old) = g.1.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k) {
                        g.1.remove(&old);
                    }
                }
                g.1.insert(k, (tick, t.clone()));
                t
            }
        }?;
        Some(roadcore::grid::upsampled(&tile, dz, x, y))
    }
}

/// A tile made but for its water (`prepare`): its elevations (AWS's, the pixels above changed
/// ones made again from them, repaired, GLO-30 blended in), AWS's as decoded, its water, and the
/// PNG it came as.
pub struct Prepared {
    pub png: Vec<u8>,
    pub e: Vec<f32>,
    pub before: Vec<f32>,
    pub water: Option<std::sync::Arc<crate::terrain_water::WaterTile>>,
    /// AWS's z9 tile over it (the walled patches' rule), for the repair after the water, and
    /// whether GLO-30 was blended in (then AWS's z9 tile, on another datum, isn't its coarser view).
    coarse: Option<Vec<f32>>,
    north: bool,
    decoded: bool,
    z: u8,
    y: u32,
}

impl Prepared {
    /// Its lakes' samples (crate::terrain_water::samples), for their levels.
    pub fn lake_samples(&self) -> HashMap<u64, crate::terrain_water::LakeSamples> {
        match &self.water {
            Some(w) if self.decoded => crate::terrain_water::samples(&self.e, w),
            _ => HashMap::new(),
        }
    }
}

/// A tile's elevations made (docs/plan.md §6, Terrain), but for its water: the pixels above the
/// changed ones below made again from them (`below`, the four children's changes: their 2×2
/// means); from REBUILD_Z down, each quarter whose child tile exists made again whole from it
/// (`quads`: AWS's coarse levels come from coarser sources, and lost peaks: Fuji's summit pixel
/// 3,106 m at z6, 2,368 m at z5, 2,134 m at z4; from z9, 3,378, 2,715 and 2,337 m); repaired
/// (`repair`, on AWS's values, bathymetry and all, so a pit reads as deep as AWS made it, with AWS's
/// z9 tile over it when `src` has them); GLO-30 blended in north of 59.5°N at z9 and finer, but for
/// the pixels made from the children (blended there already; z8 and coarser are made from them). Then `finish` flattens its water.
#[allow(clippy::too_many_arguments)]
pub fn prepare(png: Vec<u8>, z: u8, x: u32, y: u32, below: &HashMap<(u32, u32), Repaired>, quads: &HashMap<(u32, u32), Vec<f32>>, src: &Sources, repair: &dyn Fn(&mut [f32], u8, f64, Option<&[f32]>)) -> Prepared {
    let Ok(mut e) = decode_terrain_png(&png) else { return Prepared { png, e: Vec::new(), before: Vec::new(), water: None, coarse: None, north: false, decoded: false, z, y } };
    let before = e.clone();
    let mut kept = vec![false; e.len()];
    for k in 0..4u32 {
        let (dx, dy) = (k & 1, k >> 1);
        let Some(c) = below.get(&(x * 2 + dx, y * 2 + dy)) else { continue };
        for &i in &c.moved {
            let i = i as usize;
            let (cx, cy) = ((i % 256) & !1, (i / 256) & !1);
            let m = (c.e[cy * 256 + cx] + c.e[cy * 256 + cx + 1] + c.e[(cy + 1) * 256 + cx] + c.e[(cy + 1) * 256 + cx + 1]) * 0.25;
            let p = (dy as usize * 128 + cy / 2) * 256 + dx as usize * 128 + cx / 2;
            e[p] = m;
            kept[p] = true;
        }
    }
    if z <= REBUILD_Z {
        for k in 0..4u32 {
            let (dx, dy) = (k & 1, k >> 1);
            let Some(q) = quads.get(&(x * 2 + dx, y * 2 + dy)) else { continue };
            for j in 0..128 {
                let row = (dy as usize * 128 + j) * 256 + dx as usize * 128;
                e[row..row + 128].copy_from_slice(&q[j * 128..(j + 1) * 128]);
                kept[row..row + 128].iter_mut().for_each(|k| *k = true);
            }
        }
    }
    let coarse = src.coarse.and_then(|c| c.over(z, x, y));
    repair(&mut e, z, tile_lat(z, y), coarse.as_deref());
    // (From z9 up: coarser tiles are made from their children in the coverage, and GLO-30 is only
    // in the store there, so a z3 tile's would decode hundreds of its 1° tiles for nothing.)
    if let Some(n) = src.north.filter(|_| z > REBUILD_Z).and_then(|c| crate::terrain_north::north_tile(c, z, x, y)) {
        crate::terrain_north::blend(&mut e, &n, Some(&kept));
    }
    let water = src.water.and_then(|w| match crate::terrain_water::tile_water(w, z, x, y) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("terrain: the water of {z}/{x}/{y}: {err:#}");
            None
        }
    });
    let north = blends_north(src, z, y);
    Prepared { png, e, before, water, coarse, north, decoded: true, z, y }
}

/// Whether GLO-30 is blended into tile z/·/y (`prepare`): z9 and finer, north of SOUTH.
pub fn blends_north(src: &Sources, z: u8, y: u32) -> bool {
    let n = (1u64 << z) as f64;
    let top = (std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n)).dsinh().datan().to_degrees();
    src.north.is_some() && z > REBUILD_Z && top > crate::terrain_north::SOUTH
}

/// The repair of a tile as stored (`finish`'s last step, and what `terrain --scan` checks it
/// against): repair_terrain_with, AWS's z9 tile for the walled patches only where GLO-30 isn't
/// blended in, then bathymetry to sea level.
pub fn repair_stored(e: &mut [f32], z: u8, y: u32, coarse: Option<&[f32]>, north: bool) -> roadcore::grid::Repair {
    let (r, _) = repair_terrain_with(e, z, tile_lat(z, y), if north { None } else { coarse });
    for v in e.iter_mut() {
        if *v < 0.0 {
            *v = 0.0;
        }
    }
    r
}

/// A prepared tile finished: its water flattened (`levels`: the lakes' levels,
/// crate::terrain_water), bathymetry to sea level, and repaired once more as stored
/// (`repair_stored`). Returns the PNG to store (the original
/// bytes when nothing changes), its elevations and the pixels that moved if it changed, and from
/// REBUILD_Z + 1 down its 2×2 means for the level above.
pub fn finish(p: Prepared, levels: &HashMap<u64, f32>) -> (Vec<u8>, Option<Repaired>, Option<Vec<f32>>) {
    let Prepared { png, mut e, before, water, coarse, north, decoded, z, y } = p;
    if !decoded {
        return (png, None, None);
    }
    if let Some(w) = &water {
        crate::terrain_water::flatten(&mut e, w, levels);
    }
    for v in e.iter_mut() {
        if *v < 0.0 {
            *v = 0.0;
        }
    }
    // The repair once more, on the tile as stored: what it left on a shore stands on a flat sea now
    // (a stub on Casco Bay's), GLO-30's fills and AWS's moved into them can stand broken among the
    // St. Elias's ice (walls of 26 m a metre), and the tile stored must be one it changes nothing in.
    repair_stored(&mut e, z, y, coarse.as_deref(), north);
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
    let moved: Vec<u16> = e.iter().zip(&before).enumerate().filter(|(_, (a, b))| !((*a - *b).abs() <= 0.5)).map(|(i, _)| i as u16).collect();
    if moved.is_empty() {
        return (png, None, quad);
    }
    let out = encode_terrain_png(&e, 256, 256).unwrap_or(png);
    (out, Some(Repaired { e, moved }), quad)
}

/// A tile made whole (`prepare`, then `finish` with its lakes' levels from it alone, or `levels`
/// where it has them): what a tile made alone gets.
pub fn process(png: Vec<u8>, z: u8, x: u32, y: u32, below: &HashMap<(u32, u32), Repaired>, quads: &HashMap<(u32, u32), Vec<f32>>, src: &Sources) -> (Vec<u8>, Option<Repaired>, Option<Vec<f32>>) {
    process_with(png, z, x, y, below, quads, src, &HashMap::new(), &|e, z, lat, c| {
        repair_terrain_with(e, z, lat, c);
    })
}

/// `process` with another repair in its place (the scan's comparisons: `terrain --scan`), and the
/// lakes' levels known (`known`: those it has are kept).
#[allow(clippy::too_many_arguments)]
pub fn process_with(png: Vec<u8>, z: u8, x: u32, y: u32, below: &HashMap<(u32, u32), Repaired>, quads: &HashMap<(u32, u32), Vec<f32>>, src: &Sources, known: &HashMap<u64, f32>, repair: &dyn Fn(&mut [f32], u8, f64, Option<&[f32]>)) -> (Vec<u8>, Option<Repaired>, Option<Vec<f32>>) {
    let p = prepare(png, z, x, y, below, quads, src, repair);
    let mut levels = known.clone();
    crate::terrain_water::add_levels(&mut levels, &p.lake_samples());
    finish(p, &levels)
}

/// Levels made again from the level below where it exists (process): z8 from z9 (which covers the
/// ground within a z9 tile of a road; averaging our z12 instead gives the same within a few metres).
/// The finer levels stay AWS's, so the analysis grid (z11) and what follows from it don't change.
pub const REBUILD_Z: u8 = 8;

/// A tile changed by process: its elevations and the pixels that moved by more than half a metre
/// (a pixel of 256 × 256 in a u16: a coastal tile has tens of thousands).
pub struct Repaired {
    pub e: Vec<f32>,
    pub moved: Vec<u16>,
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
pub struct ManifestTiles {
    layer: String,
    /// The layer's packs as the manifest named them when it was made (logical → file): its own,
    /// so the job may write packs while it reads (slope_pack::build_q_with).
    files: HashMap<String, std::path::PathBuf>,
    open: Mutex<HashMap<String, Option<std::sync::Arc<OpenPack>>>>,
    /// Where a pack read tile by tile throughout is copied whole and read from then on
    /// (`with_copies`, `copy_here`), and the copies made, deleted with this.
    copies: Option<(std::path::PathBuf, Mutex<Vec<std::path::PathBuf>>)>,
}

/// A pack open: its file and index.
struct OpenPack {
    file: std::fs::File,
    idx: store::pack::PackIndex,
}

impl ManifestTiles {
    pub fn new(out: &Out, layer: &str) -> Self {
        let prefix = format!("layers/{layer}/");
        let files = out.manifest.range(prefix.clone()..).take_while(|(l, _)| l.starts_with(&prefix)).map(|(l, c)| (l.clone(), out.path(c))).collect();
        ManifestTiles { layer: layer.to_string(), files, open: Mutex::new(HashMap::new()), copies: None }
    }

    /// `new`, the packs `copy_here` is asked for copied into `dir` (the job's scratch) whole and
    /// read there; the copies deleted when this goes. The same bytes either way: a pack is
    /// content-named, never rewritten.
    pub fn with_copies(out: &Out, layer: &str, dir: std::path::PathBuf) -> Self {
        let mut t = Self::new(out, layer);
        t.copies = Some((dir, Mutex::new(Vec::new())));
        t
    }

    pub fn logical(layer: &str, z: u8, x: u32, y: u32) -> String {
        match z {
            0..=2 => format!("layers/{layer}/root/0-0-0"),
            3..=8 => format!("layers/{layer}/lo/3-{}-{}", x >> (z - 3), y >> (z - 3)),
            _ => format!("layers/{layer}/hi/6-{}-{}", x >> (z - 6), y >> (z - 6)),
        }
    }

    fn open_file(p: &std::path::Path) -> anyhow::Result<std::sync::Arc<OpenPack>> {
        let file = std::fs::File::open(p)?;
        let len = file.metadata()?.len();
        let idx = store::pack::PackIndex::read_from(&FileSource(&file, len))?;
        Ok(std::sync::Arc::new(OpenPack { file, idx }))
    }

    /// The open pack holding tile (z, x, y), if the manifest has it.
    fn pack(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<std::sync::Arc<OpenPack>>> {
        let logical = Self::logical(&self.layer, z, x, y);
        let mut open = self.open.lock().unwrap();
        if !open.contains_key(&logical) {
            let v = match self.files.get(&logical) {
                Some(p) => Some(Self::open_file(p)?),
                None => None,
            };
            open.insert(logical.clone(), v);
        }
        Ok(open.get(&logical).cloned().flatten())
    }

    pub fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
        use store::sys::PosIo;
        let Some(e) = self.pack(z, x, y)? else { return Ok(None) };
        let Some(ent) = e.idx.find(z, x, y) else { return Ok(None) };
        let mut b = vec![0u8; ent.len as usize];
        e.file.read_exact_at(&mut b, ent.offset)?;
        Ok(Some(b))
    }

    /// Tiles `want` (each as `get` gives it), read a pack at a time in spans: the tiles' bytes in the
    /// pack's order, neighbours a gap of under `SPAN_GAP` apart read together, up to `SPAN_MAX` a
    /// read, the spans in parallel. A unit reading thousands of a pack's tiles makes a few dozen
    /// reads of the share instead of a round trip a tile, and moves little it doesn't use.
    pub fn get_many(&self, want: &[(u8, u32, u32)]) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        use store::sys::PosIo;
        const SPAN_GAP: u64 = 256 << 10;
        const SPAN_MAX: u64 = 32 << 20;
        let mut got: Vec<Option<Vec<u8>>> = vec![None; want.len()];
        // (Each tile's pack and entry, then by pack, by offset.)
        let mut by_pack: HashMap<String, (std::sync::Arc<OpenPack>, Vec<(u64, u32, usize)>)> = HashMap::new();
        for (i, &(z, x, y)) in want.iter().enumerate() {
            let Some(e) = self.pack(z, x, y)? else { continue };
            let Some(ent) = e.idx.find(z, x, y) else { continue };
            by_pack.entry(Self::logical(&self.layer, z, x, y)).or_insert_with(|| (e, Vec::new())).1.push((ent.offset, ent.len, i));
        }
        let mut spans: Vec<(std::sync::Arc<OpenPack>, u64, u64, Vec<(u64, u32, usize)>)> = Vec::new();
        for (_, (e, mut ents)) in by_pack {
            ents.sort_unstable();
            for t in ents {
                let end = t.0 + t.1 as u64;
                match spans.last_mut() {
                    Some((p, s0, s1, ts)) if std::sync::Arc::ptr_eq(p, &e) && t.0 <= *s1 + SPAN_GAP && end - *s0 <= SPAN_MAX => {
                        *s1 = (*s1).max(end);
                        ts.push(t);
                    }
                    _ => spans.push((e.clone(), t.0, end, vec![t])),
                }
            }
        }
        let read: Vec<anyhow::Result<Vec<(usize, Vec<u8>)>>> = spans
            .par_iter()
            .map(|(e, s0, s1, ts)| {
                let mut b = vec![0u8; (s1 - s0) as usize];
                e.file.read_exact_at(&mut b, *s0)?;
                Ok(ts.iter().map(|&(o, l, i)| (i, b[(o - s0) as usize..(o - s0) as usize + l as usize].to_vec())).collect())
            })
            .collect();
        for r in read {
            for (i, b) in r? {
                got[i] = Some(b);
            }
        }
        Ok(got)
    }

    /// The pack holding tile (z, x, y) (z6 tile (x, y)'s hi pack: any of its tiles at zoom 9–12)
    /// copied into the copies' folder (`with_copies`) whole, one
    /// sequential read, and read there from now on: its bytes copied, or None when there's no such
    /// pack, no copies' folder, or the copy failed (then it's read from the NAS, as before).
    pub fn copy_here(&self, z: u8, x: u32, y: u32) -> Option<u64> {
        let logical = Self::logical(&self.layer, z, x, y);
        let (Some((dir, made)), Some(src)) = (&self.copies, self.files.get(&logical)) else { return None };
        let dst = dir.join(src.file_name()?);
        let copied = (|| -> anyhow::Result<(std::sync::Arc<OpenPack>, u64)> {
            std::fs::create_dir_all(dir)?;
            made.lock().unwrap().push(dst.clone());
            let n = store::sys::copy_data(src, &dst)?;
            anyhow::ensure!(n == std::fs::metadata(src)?.len(), "copied {n} bytes of {}", src.display());
            Ok((Self::open_file(&dst)?, n))
        })();
        match copied {
            Ok((e, n)) => {
                self.open.lock().unwrap().insert(logical, Some(e));
                Some(n)
            }
            Err(err) => {
                eprintln!("{logical}: not copied here, read from the NAS ({err:#})");
                None
            }
        }
    }

    /// Whether the layer has tile (z, x, y), from its pack's index alone.
    pub fn has(&self, z: u8, x: u32, y: u32) -> anyhow::Result<bool> {
        Ok(self.pack(z, x, y)?.is_some_and(|e| e.idx.find(z, x, y).is_some()))
    }
}

impl Drop for ManifestTiles {
    fn drop(&mut self) {
        if let Some((_, made)) = &self.copies {
            for p in made.lock().unwrap().iter() {
                std::fs::remove_file(p).ok();
            }
        }
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
/// an input. Each loose copy is made whole under a name of its own, then named (store::cachefile); every
/// tile is checked whole when read, a loose one or an archive's: one that isn't (cut short) is
/// passed over (a loose one deleted) and taken from the next source, the NAS's copy, else AWS.
pub struct RawTiles {
    dir: std::path::PathBuf,
    store: Option<std::path::PathBuf>,
    agent: ureq::Agent,
    /// The store's columns (`<z>/<x>/`) as listed once: over SMB each look is a round trip, and
    /// those, a tile's few, set a terrain job's pace. (The folders here are made as tiles are put:
    /// store::cachefile.)
    listed: Mutex<HashMap<(u8, u32), std::sync::Arc<std::collections::HashSet<String>>>>,
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
        RawTiles { dir: dir.to_path_buf(), store: None, agent: agent(), listed: Default::default(), index: Default::default(), archives: Default::default() }
    }

    /// The local cache `dir`, filled from the NAS's `store` where it has a tile.
    pub fn with_store(dir: &std::path::Path, store: &std::path::Path) -> Self {
        RawTiles { dir: dir.to_path_buf(), store: Some(store.to_path_buf()), agent: agent(), listed: Default::default(), index: Default::default(), archives: Default::default() }
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
                // (This Mac's copy held as it's read: store::cachefile.)
                let local = self.dir.join("packs").join(&p.name);
                let at = match store::cachefile::hold_existing(&local) {
                    Ok(Some(l)) => l,
                    _ => st.join("packs").join(&p.name),
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
            // (Held, and marked used: rawpack::local_copy, store::cachefile.)
            let opened = crate::rawpack::local_copy(&self.dir, st, &p.name).and_then(|l| roadcore::archive::Archive::open(&l));
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

    /// Whether tile (z, x, y) is in its area's archives, by its entry alone (no tile read): the
    /// prefetch passes those over, as the build reads them itself (and checks them whole, taking
    /// one that isn't again then).
    fn in_archive(&self, z: u8, x: u32, y: u32) -> bool {
        let Some(st) = self.store.as_ref() else { return false };
        let area = crate::rawpack::area(z, x, y);
        let cell = self.archives.lock().unwrap().entry(area.clone()).or_default().clone();
        let mut c = cell.lock().unwrap();
        if c.is_none() {
            match self.open_area(st, &area) {
                Some(o) => *c = Some(o),
                None => return false,
            }
        }
        let key = roadcore::archive::tile_key(z, x, y);
        match c.as_ref() {
            Some(Opened::Here(h)) => h.iter().any(|a| a.get(z, x, y).is_some()),
            Some(Opened::Ranged { archives, .. }) => archives.iter().any(|(_, e)| e.binary_search_by_key(&key, |e| e.key).is_ok()),
            None => false,
        }
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
        // On the NAS (its column listed once): copied here (its folder here made as it's put:
        // store::cachefile, which makes it again if room-making took it).
        if let Some(st) = &self.store {
            let sd = st.join(format!("{z}/{x}"));
            let col = self.column(st, z, x);
            let has = |n: String| col.as_ref().map_or_else(|| sd.join(&n).exists(), |c| c.contains(&n));
            if has(format!("{y}.png")) {
                if let Some(b) = nas_whole(&sd.join(format!("{y}.png"))) {
                    store::cachefile::put(&p, &b)?;
                    return Ok((Some(b), false));
                }
            }
            if has(format!("{y}.none")) {
                store::cachefile::put(&none, b"")?;
                return Ok((None, false));
            }
        }
        self.fetch(z, x, y).map(|b| (b, true))
    }

    /// Fetches the tiles of `tiles` (zoom `z`) not here yet, `threads` at a time: a download mostly
    /// waits on AWS, so far more of them than cores. The number that came from AWS.
    pub fn prefetch(&self, z: u8, tiles: &[(u32, u32)], threads: usize) -> anyhow::Result<usize> {
        self.prefetch_counted(z, tiles, threads, &std::sync::atomic::AtomicU64::new(0))
    }

    /// `prefetch`, counting into `done` each tile as it's here (at once for those that were).
    pub fn prefetch_counted(&self, z: u8, tiles: &[(u32, u32)], threads: usize, done: &std::sync::atomic::AtomicU64) -> anyhow::Result<usize> {
        use rayon::prelude::*;
        let todo: Vec<(u32, u32)> = tiles
            .iter()
            .copied()
            .filter(|&(x, y)| {
                let d = self.dir.join(format!("{z}/{x}"));
                !d.join(format!("{y}.png")).exists() && !d.join(format!("{y}.none")).exists() && !self.in_archive(z, x, y)
            })
            .collect();
        done.fetch_add((tiles.len() - todo.len()) as u64, std::sync::atomic::Ordering::Relaxed);
        if todo.is_empty() {
            return Ok(0);
        }
        let fetched = std::sync::atomic::AtomicUsize::new(0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
        pool.install(|| {
            todo.par_iter().try_for_each(|&(x, y)| -> anyhow::Result<()> {
                let got = self.get(z, x, y);
                done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if got?.1 {
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
        store::cachefile::discard(&d.join(format!("{y}.png")));
        self.fetch(z, x, y)
    }

    /// From AWS, into the local cache: the NAS's store gets them in bulk (its small-file writes, a
    /// tile at a time, set the job's pace: 25 a second against 119 here).
    fn fetch(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let d = self.dir.join(format!("{z}/{x}"));
        match fetch_checked(&self.agent, z, x, y)? {
            // (Made whole under a name of its own, then named: store::cachefile.)
            Some(b) => {
                store::cachefile::put(&d.join(format!("{y}.png")), &b)?;
                Ok(Some(b))
            }
            None => {
                store::cachefile::put(&d.join(format!("{y}.none")), b"")?;
                Ok(None)
            }
        }
    }
}

/// A kept tile's bytes when it's there and whole, read under a shared lock (store::cachefile);
/// one that isn't whole is deleted.
fn read_whole(p: &std::path::Path) -> Option<Vec<u8>> {
    let b = store::cachefile::read(p).ok()??;
    if crate::whole::png_whole(&b) {
        return Some(b);
    }
    eprintln!("terrain: {} isn't whole ({} bytes): fetched again", p.display(), b.len());
    store::cachefile::discard(p);
    None
}

/// A tile of the NAS's loose store when it's there and whole (not this Mac's cache: nothing
/// locked); one that isn't whole is deleted.
fn nas_whole(p: &std::path::Path) -> Option<Vec<u8>> {
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
    crate::fetch::online(&url)?;
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
pub fn build_q(out: &mut Out, raw: &RawTiles, q: (u32, u32), ts: &[(u32, u32)], cov: &Coverage, src: &Sources) -> anyhow::Result<PackReport> {
    build_q_with(out, raw, q, ts, cov, src, &|_, _, _| {})
}

/// What a terrain piece (a z6 tile's run: `build_piece`) keeps for its area's assembly
/// (`build_lo`), its mid: its z9 tiles' 2×2 means (128 × 128, f32: each a quarter of the z8 tile
/// above), and its lakes' levels as its own levels gave them (crate::terrain_water). Its z8–z6
/// can't be made with it: a lake's level at z8 is gathered from the whole area's z8 tiles, and the
/// area's levels start from every piece's lakes, the first piece's (in column, then row order)
/// kept for a lake two of them have.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mid {
    pub quads: std::collections::BTreeMap<(u32, u32), Vec<f32>>,
    pub levels: std::collections::BTreeMap<u64, f32>,
}

/// Z6 tile (`x`, `y`)'s terrain mid in the manifest (`Mid`).
pub fn mid_logical(x: u32, y: u32) -> String {
    format!("work/terrain-mid/6-{x}-{y}")
}

/// The mid's format.
const MID_FMT: u64 = 1;

/// Writes z6 tile `t`'s mid to `path`: a sectioned file (store::sect), meta `{"fmt": 1, "step":
/// "terrain", "tile": "6/x/y", "v": TERRAIN_V}`; a section `quad-9-<x>-<y>` for each z9 tile's
/// means (f32, little-endian, row by row), by column then row, then `lake-ids` (u64) and
/// `lake-levels` (f32), by id.
pub fn write_mid(path: &std::path::Path, t: (u32, u32), mid: &Mid) -> anyhow::Result<()> {
    let meta = serde_json::json!({ "fmt": MID_FMT, "step": "terrain", "tile": format!("6/{}/{}", t.0, t.1), "v": crate::agent::build::TERRAIN_V });
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut w = store::sect::SectWriter::create(path, meta)?;
    for (&(x, y), q) in &mid.quads {
        anyhow::ensure!((x >> 3, y >> 3) == t && q.len() == 128 * 128, "z9 tile 9/{x}/{y}'s means aren't z6 tile 6/{}/{}'s", t.0, t.1);
        w.add(&format!("quad-9-{x}-{y}"), &q.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())?;
    }
    w.add("lake-ids", &mid.levels.keys().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())?;
    w.add("lake-levels", &mid.levels.values().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())?;
    w.finish()?;
    Ok(())
}

/// A mid (`write_mid`), read and checked: its format, step and version (one made for another
/// version of the terrain isn't one to assemble), every section whole. Its z6 tile and the mid.
pub fn read_mid(path: &std::path::Path) -> anyhow::Result<((u32, u32), Mid)> {
    use anyhow::Context;
    let what = || path.display().to_string();
    let r = store::sect::SectReader::open(store::range::PlainFile::open(path).with_context(what)?).with_context(what)?;
    let m = r.meta();
    anyhow::ensure!(m["fmt"].as_u64() == Some(MID_FMT) && m["step"] == "terrain", "{}: not a terrain mid", what());
    anyhow::ensure!(m["v"].as_u64() == Some(crate::agent::build::TERRAIN_V as u64), "{}: a mid of terrain version {}, not {}", what(), m["v"], crate::agent::build::TERRAIN_V);
    let t = m["tile"].as_str().and_then(crate::legacy::Unit::parse).filter(|u| u.z == 6).with_context(|| format!("{}: no z6 tile", what()))?;
    let t = (t.x, t.y);
    let mut mid = Mid::default();
    let (mut ids, mut levels): (Vec<u64>, Vec<f32>) = (Vec::new(), Vec::new());
    for s in r.sections() {
        let b = r.read(&s.name).with_context(what)?;
        match s.name.as_str() {
            "lake-ids" => ids = b.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect(),
            "lake-levels" => levels = b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
            n => {
                let xy: Vec<u32> = n.strip_prefix("quad-9-").with_context(|| format!("{}: a section {n:?}", what()))?.split('-').map(|v| v.parse::<u32>()).collect::<Result<_, _>>().with_context(|| format!("{}: a section {n:?}", what()))?;
                anyhow::ensure!(xy.len() == 2 && (xy[0] >> 3, xy[1] >> 3) == t && b.len() == 128 * 128 * 4, "{}: a section {n:?}", what());
                mid.quads.insert((xy[0], xy[1]), b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect());
            }
        }
    }
    anyhow::ensure!(ids.len() == levels.len(), "{}: {} lakes and {} levels", what(), ids.len(), levels.len());
    mid.levels = ids.into_iter().zip(levels).collect();
    Ok((t, mid))
}

/// A tile as made: zoom, column, row, its bytes.
pub type Made = (u8, u32, u32, Vec<u8>);
/// A level as made: its tiles, the ones repaired (for the pixels above them), the 2×2 means.
type Level = (Vec<(u32, u32, Vec<u8>)>, HashMap<(u32, u32), Repaired>, HashMap<(u32, u32), Vec<f32>>);

/// What makes the terrain's levels: the raw tiles, the sources, and the counts of what it did (each
/// tile counts once here and once shaded: `done`).
struct Maker<'a> {
    raw: &'a dyn RawSource,
    src: &'a Sources<'a>,
    here: std::sync::atomic::AtomicU64,
    processed: std::sync::atomic::AtomicU64,
    fetched: std::sync::atomic::AtomicUsize,
    missing: std::sync::atomic::AtomicUsize,
    repaired: std::sync::atomic::AtomicUsize,
}

impl<'a> Maker<'a> {
    fn new(raw: &'a dyn RawSource, src: &'a Sources<'a>) -> Self {
        Maker { raw, src, here: Default::default(), processed: Default::default(), fetched: Default::default(), missing: Default::default(), repaired: Default::default() }
    }

    /// The tiles fetched (or reused) and shaded, each counted once.
    fn done(&self) -> u64 {
        (self.here.load(std::sync::atomic::Ordering::Relaxed) + self.processed.load(std::sync::atomic::Ordering::Relaxed)) / 2
    }

    fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let (b, new) = self.raw.get(z, x, y)?;
        if new {
            self.fetched.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if b.is_none() {
            self.missing.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(b)
    }

    /// One level: every tile fetched or reused, then made with what the level below made: first all
    /// but their water (`prepare`), then each lake's level from all its shore in the level (those
    /// known from finer levels kept: `levels`), then their water (`finish`).
    fn level(&self, z: u8, tiles: Vec<(u32, u32)>, below: &HashMap<(u32, u32), Repaired>, quads: &HashMap<(u32, u32), Vec<f32>>, levels: &mut HashMap<u64, f32>) -> anyhow::Result<Level> {
        use crate::timings::{sub, Class};
        // (A level's stages, each a sub-phase of the run's tiles made, added up over its levels.)
        let p = sub("raw tiles fetched (the cache, the NAS's archives, else AWS)", Class::NasRead);
        self.fetched.fetch_add(self.raw.prefetch_counted(z, &tiles, FETCH_THREADS, &self.here)?, std::sync::atomic::Ordering::Relaxed);
        drop(p);
        let p = sub("tiles prepared (repaired; GLO-30 and the water read)", Class::Compute);
        let src = self.src;
        let prepared: Vec<anyhow::Result<Option<(u32, u32, Prepared)>>> = tiles
            .par_iter()
            .map(|&(x, y)| {
                let Some(b) = self.get(z, x, y)? else {
                    self.processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(None);
                };
                Ok(Some((x, y, prepare(b, z, x, y, below, quads, src, &|e, z, lat, c| {
                    repair_terrain_with(e, z, lat, c);
                }))))
            })
            .collect();
        let mut prepared: Vec<(u32, u32, Prepared)> = prepared.into_iter().filter_map(|r| r.transpose()).collect::<anyhow::Result<_>>()?;
        drop(p);
        let _p = sub("tiles' lakes levelled, shaded and encoded", Class::Compute);
        let mut lakes = HashMap::new();
        for (_, _, p) in &prepared {
            crate::terrain_water::gather(&mut lakes, p.lake_samples());
        }
        crate::terrain_water::add_levels(levels, &lakes);
        let levels: &HashMap<u64, f32> = levels;
        let done: Vec<(u32, u32, Vec<u8>, Option<Repaired>, Option<Vec<f32>>)> = prepared
            .par_drain(..)
            .map(|(x, y, p)| {
                let (b, r, q) = finish(p, levels);
                self.processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                (x, y, b, r, q)
            })
            .collect();
        let (mut outs, mut nb, mut nq) = (Vec::new(), HashMap::new(), HashMap::new());
        for (x, y, b, r, q) in done {
            if let Some(r) = r {
                self.repaired.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                nb.insert((x, y), r);
            }
            if let Some(q) = q {
                nq.insert((x, y), q);
            }
            outs.push((x, y, b));
        }
        Ok((outs, nb, nq))
    }

    /// A piece (z6 tile `t`): its levels z12 → z9 (`levels`, `piece_levels`), its lakes' levels
    /// from them alone. Its hi tiles, sorted, and its mid.
    fn piece(&self, levels: Vec<(u8, Vec<(u32, u32)>)>) -> anyhow::Result<(Vec<Made>, Mid)> {
        let (mut below, mut quads) = (HashMap::new(), HashMap::new());
        let mut hi: Vec<Made> = Vec::new();
        let mut lakes: HashMap<u64, f32> = HashMap::new();
        for (z, tiles) in levels {
            let (outs, nb, nq) = self.level(z, tiles, &below, &quads, &mut lakes)?;
            hi.extend(outs.into_iter().map(|(x, y, b)| (z, x, y, b)));
            (below, quads) = (nb, nq);
        }
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        Ok((hi, Mid { quads: quads.into_iter().collect(), levels: lakes.into_iter().collect() }))
    }

    /// An area's assembly (z3 tile `q`): its levels z8 → z3, the whole of it, from its pieces' mids
    /// (`mids`, in column then row order: their z9 tiles' means taken in, each lake's level the
    /// first of them that has one, the levels' lakes added below them). Its lo tiles, sorted.
    ///
    /// (A z8 tile's quarter over a piece's z9 tile is made again whole from that tile's means: the
    /// pixels a z9 tile's repair moved, which `prepare` takes in first, are written over by them;
    /// so the mid needn't hold those.)
    fn lo(&self, q: (u32, u32), mids: &[((u32, u32), &Mid)]) -> anyhow::Result<Vec<Made>> {
        let mut quads: HashMap<(u32, u32), Vec<f32>> = HashMap::new();
        let mut lakes: HashMap<u64, f32> = HashMap::new();
        for (_, m) in mids {
            quads.extend(m.quads.iter().map(|(k, v)| (*k, v.clone())));
            for (k, v) in &m.levels {
                lakes.entry(*k).or_insert(*v);
            }
        }
        let mut below = HashMap::new();
        let mut lo: Vec<Made> = Vec::new();
        for (z, tiles) in upper_levels(q) {
            let (outs, nb, nq) = self.level(z, tiles, &below, &quads, &mut lakes)?;
            lo.extend(outs.into_iter().map(|(x, y, b)| (z, x, y, b)));
            (below, quads) = (nb, nq);
        }
        lo.sort_by_key(|t| (t.0, t.1, t.2));
        Ok(lo)
    }

    fn report(&self, rep: &mut PackReport) {
        rep.fetched = self.fetched.load(std::sync::atomic::Ordering::Relaxed);
        rep.missing = self.missing.load(std::sync::atomic::Ordering::Relaxed);
        rep.repaired = self.repaired.load(std::sync::atomic::Ordering::Relaxed);
    }
}

/// A piece's levels `levels` (z12 → z9, `piece_levels`, or a task's part of them:
/// crate::terrain_task) made from `raw` and `src`, its lakes' levels from them alone: its hi tiles,
/// sorted, and its mid.
pub fn make_piece(raw: &dyn RawSource, src: &Sources, levels: Vec<(u8, Vec<(u32, u32)>)>) -> anyhow::Result<(Vec<Made>, Mid)> {
    Maker::new(raw, src).piece(levels)
}

/// A piece's levels (z6 tile `t`'s), z12 → z9: the tiles near the coverage (20 km), as fine as the
/// latitude allows.
pub fn piece_levels(cov: &Coverage, t: (u32, u32)) -> Vec<(u8, Vec<(u32, u32)>)> {
    let (tx, ty) = t;
    (9..=12u8)
        .rev()
        .map(|z| {
            let s = 1u32 << (z - 6);
            let tiles = (tx * s..(tx + 1) * s).flat_map(|x| (ty * s..(ty + 1) * s).map(move |y| (x, y))).filter(|&(x, y)| z <= max_zoom_at(tile_lat(z, y)) && near_coverage(cov, z, x, y, 20.0)).collect();
            (z, tiles)
        })
        .collect()
}

/// Whether z6 tile `t`'s piece makes any hi tile (`piece_levels` not all empty): one near the
/// coverage at z6 can still have none of its z9–12 tiles near it (6/21/18).
pub fn makes_hi(cov: &Coverage, t: (u32, u32)) -> bool {
    piece_levels(cov, t).iter().any(|(_, ts)| !ts.is_empty())
}

/// Drops z6 tile `t`'s terrain hi pack and mid from the manifest (a piece the coverage has left:
/// docs/plan.md §5, Shrinking), saving it; how many it had.
pub fn drop_piece(out: &mut Out, t: (u32, u32)) -> anyhow::Result<usize> {
    let n = drop_named(out, &[format!("layers/terrain/hi/6-{}-{}", t.0, t.1), mid_logical(t.0, t.1)]);
    out.save()?;
    Ok(n)
}

/// Drops z3 tile `q`'s terrain lo pack from the manifest (a "none" assembly: the coverage has left
/// the z3 tile, docs/plan.md §5, Shrinking), saving it; how many it had.
pub fn drop_lo(out: &mut Out, q: (u32, u32)) -> anyhow::Result<usize> {
    let n = drop_named(out, &[format!("layers/terrain/lo/3-{}-{}", q.0, q.1)]);
    out.save()?;
    Ok(n)
}

/// Drops those of `logicals` the manifest names (not saved); how many.
pub(crate) fn drop_named(out: &mut Out, logicals: &[String]) -> usize {
    let gone: Vec<&String> = logicals.iter().filter(|l| out.get(l).is_some()).collect();
    for l in &gone {
        out.remove(l);
    }
    gone.len()
}

/// An area's levels (z3 tile `q`'s), z8 → z3, the whole of it.
fn upper_levels(q: (u32, u32)) -> Vec<(u8, Vec<(u32, u32)>)> {
    (3..=8u8)
        .rev()
        .map(|z| {
            let s = 1u32 << (z - 3);
            (z, (q.0 * s..(q.0 + 1) * s).flat_map(|x| (q.1 * s..(q.1 + 1) * s).map(move |y| (x, y))).collect())
        })
        .collect()
}

/// `run` with `mk`'s tiles said to `progress` as "tiles" (of `total`) every few seconds, and once
/// at the end.
fn saying<T>(mk: &Maker, total: u64, progress: crate::rawpack::Progress, run: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    let finished = std::sync::atomic::AtomicBool::new(false);
    // (Set however the run ends, a panic too: the scope waits for the reporter before it goes on.)
    struct Finished<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for Finished<'_> {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            let mut said = std::time::Instant::now();
            while !finished.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                if said.elapsed() >= Duration::from_secs(5) {
                    progress("tiles", mk.done().min(total), total);
                    said = std::time::Instant::now();
                }
            }
        });
        let _finished = Finished(&finished);
        run()
    })?;
    progress("tiles", total, total);
    Ok(r)
}

/// `build_q`, saying how far it is (`progress`): the area's tiles, every level's, each half done once
/// it's here (fetched from AWS, or read from the NAS's archives) and done once shaded ("tiles",
/// every few seconds; each z6 tile's hi pack written as its tiles are done), then its lo pack
/// written ("packs").
///
/// Its pieces, a z6 tile at a time (`build_piece`'s run: its levels z12 → z9 made, then its hi pack
/// written, so only its tiles are held), then its assembly (`build_lo`'s, from the pieces' mids held
/// here): the same bytes as the pieces' jobs and the assembly's, and the same hi packs dropped (a
/// piece's when it makes no hi tile; that of each z6 tile of `q` the coverage has left, as its
/// "none" piece's run drops it with its mid, `drop_piece`: a whole run writes no mids). A tile's making reads only its own raw
/// tile and its children's (`process`): the same bytes in any order.
pub fn build_q_with(out: &mut Out, raw: &RawTiles, q: (u32, u32), ts: &[(u32, u32)], cov: &Coverage, src: &Sources, progress: crate::rawpack::Progress) -> anyhow::Result<PackReport> {
    let mut rep = PackReport::default();
    let mine: Vec<Vec<(u8, Vec<(u32, u32)>)>> = ts.iter().map(|&t| piece_levels(cov, t)).collect();
    let total: u64 = mine.iter().flatten().map(|(_, t)| t.len() as u64).sum::<u64>() + 1365;
    let mk = Maker::new(raw, src);
    let made = crate::timings::phase("the area's tiles made and its hi packs uploaded", crate::timings::Class::Mixed);
    let lo = saying(&mk, total, progress, || {
        let mut mids: Vec<((u32, u32), Mid)> = Vec::new();
        for (&t, levels) in ts.iter().zip(mine) {
            let (hi, mid) = mk.piece(levels)?;
            rep.hi_tiles += hi.len();
            let mut it = hi.into_iter().map(|(z, x, y, b)| {
                let n = b.len() as u32;
                (z, x, y, b, n)
            });
            let _p = crate::timings::sub("hi packs uploaded", crate::timings::Class::NasWrite);
            if crate::layers::write_pack(out, "terrain", "terrarium-png", false, "hi", (6, t.0, t.1), &mut it)?.is_none() {
                drop_named(out, &[format!("layers/terrain/hi/6-{}-{}", t.0, t.1)]);
            }
            mids.push((t, mid));
        }
        // (The area's z6 tiles the coverage has left: their hi packs go. Not their mids, which
        // a whole run never writes (its lease's write-set: agent::steps::saves); their "none"
        // pieces drop those.)
        for x in q.0 * 8..(q.0 + 1) * 8 {
            for y in q.1 * 8..(q.1 + 1) * 8 {
                if !near_coverage(cov, 6, x, y, 20.0) {
                    drop_named(out, &[format!("layers/terrain/hi/6-{x}-{y}")]);
                }
            }
        }
        let refs: Vec<((u32, u32), &Mid)> = mids.iter().map(|(t, m)| (*t, m)).collect();
        mk.lo(q, &refs)
    })?;
    mk.report(&mut rep);
    drop(made);
    // Then q's lo pack.
    let up = crate::timings::phase("the lo pack uploaded", crate::timings::Class::NasWrite);
    progress("packs", 0, 1);
    rep.lo_tiles = lo.len();
    let mut it = lo.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    if crate::layers::write_pack(out, "terrain", "terrarium-png", false, "lo", (3, q.0, q.1), &mut it)?.is_none() {
        drop_named(out, &[format!("layers/terrain/lo/3-{}-{}", q.0, q.1)]);
    }
    drop(up);
    out.save()?;
    progress("packs", 1, 1);
    Ok(rep)
}

/// Makes z6 tile `t`'s terrain (a piece: its levels z12 → z9, `piece_levels`) and uploads it: its hi
/// pack (none when it made no tile: the manifest's then dropped, so no earlier run's hi tiles stay
/// above the z8–z6 its assembly makes from the raw tiles alone) and its mid (`Mid`, `mid_logical`),
/// which its area's assembly reads (`build_lo`). A z6 tile the coverage has left (not near it: no
/// piece) has its hi pack and mid dropped instead (`drop_piece`). `expect_same`: a piece made again
/// as it is (its mid made), whose hi pack must come out as the manifest has it (none where it has
/// none), else an error and nothing uploaded. Says how far it is as `build_q_with` does.
#[allow(clippy::too_many_arguments)]
pub fn build_piece(out: &mut Out, raw: &RawTiles, t: (u32, u32), cov: &Coverage, src: &Sources, expect_same: bool, progress: crate::rawpack::Progress) -> anyhow::Result<PackReport> {
    build_piece_with(out, raw, t, cov, src, expect_same, progress, None)
}

/// `build_piece`, some of its z8 subtrees offered to other workers through `offload` while workers
/// that take them are around (crate::terrain_task): the run makes the others, then takes theirs in
/// (or makes them too), the same bytes.
#[allow(clippy::too_many_arguments)]
pub fn build_piece_with(out: &mut Out, raw: &RawTiles, t: (u32, u32), cov: &Coverage, src: &Sources, expect_same: bool, progress: crate::rawpack::Progress, offload: Option<&crate::offload::Offload>) -> anyhow::Result<PackReport> {
    let offers = crate::terrain_task::Offers::offer(offload, t, &piece_levels(cov, t), raw, src);
    build_piece_offered(out, raw, t, cov, src, expect_same, progress, offers)
}

/// `build_piece` with its subtrees out as `offers` says (made for this piece: `Offers::offer`,
/// when it begins or, a job's next piece, ahead: crate::terrain_task::OFFER_AHEAD).
#[allow(clippy::too_many_arguments)]
pub fn build_piece_offered(out: &mut Out, raw: &RawTiles, t: (u32, u32), cov: &Coverage, src: &Sources, expect_same: bool, progress: crate::rawpack::Progress, offers: crate::terrain_task::Offers) -> anyhow::Result<PackReport> {
    anyhow::ensure!(offers.piece() == t, "offers for 6/{}/{}, not 6/{}/{}", offers.piece().0, offers.piece().1, t.0, t.1);
    let mut rep = PackReport::default();
    if !near_coverage(cov, 6, t.0, t.1, 20.0) {
        anyhow::ensure!(!expect_same, "6/{}/{}: the coverage has left it, so it can't be made as it is", t.0, t.1);
        let n = drop_piece(out, t)?;
        eprintln!("terrain 6/{}/{}: the coverage has left it; {n} files dropped", t.0, t.1);
        return Ok(rep);
    }
    let levels = piece_levels(cov, t);
    let total: u64 = levels.iter().map(|(_, t)| t.len() as u64).sum();
    let mk = Maker::new(raw, src);
    let p = crate::timings::phase("the piece's tiles made (z12 → z9)", crate::timings::Class::Mixed);
    let (hi, mid) = saying(&mk, total, progress, || {
        let mine = offers.rest(&levels);
        let n = mine.iter().map(|(_, t)| t.len()).sum();
        let began = std::time::Instant::now();
        let (mut hi, mut mid) = mk.piece(mine)?;
        offers.made_here(began.elapsed().as_secs_f64(), n);
        let _w = crate::timings::sub("the subtrees out taken in (or made here)", crate::timings::Class::Wait);
        offers.settle(&|lv| mk.piece(lv), &mut hi, &mut mid)?;
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        Ok((hi, mid))
    })?;
    drop(p);
    mk.report(&mut rep);
    progress("packs", 0, 1);
    let p = crate::timings::phase("its hi pack and mid written", crate::timings::Class::Disk);
    rep.hi_tiles = hi.len();
    let mut it = hi.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let pack = crate::layers::write_pack_local(out, "terrain", "terrarium-png", false, "hi", (6, t.0, t.1), &mut it)?;
    let ml = mid_logical(t.0, t.1);
    let mid_path = out.scratch_file(&format!("{ml}.sect"));
    write_mid(&mid_path, t, &mid)?;
    let hl = format!("layers/terrain/hi/6-{}-{}", t.0, t.1);
    if expect_same {
        let made = pack.as_ref().map(|p| p.content_name()).transpose()?;
        if out.get(&hl) != made.as_deref() {
            if let Some(p) = &pack {
                std::fs::remove_file(&p.local).ok();
            }
            std::fs::remove_file(&mid_path).ok();
            anyhow::bail!("piece 6/{}/{} was expected the same as the manifest has it, and isn't (nothing uploaded): {hl}: made {}, the manifest has {}", t.0, t.1, made.as_deref().unwrap_or("none"), out.get(&hl).unwrap_or("none"));
        }
    }
    drop(p);
    let up = crate::timings::phase("uploaded", crate::timings::Class::NasWrite);
    match pack {
        Some(p) => {
            out.put_file(&p.logical, "pack", &p.local)?;
        }
        None => {
            drop_named(out, &[hl]);
        }
    }
    out.put_file(&ml, "sect", &mid_path)?;
    drop(up);
    out.save()?;
    progress("packs", 1, 1);
    Ok(rep)
}

/// Makes z3 tile `q`'s zoomed-out terrain (an assembly: its levels z8 → z3, the whole of it) from
/// its pieces' mids (`ts`: its z6 tiles near the coverage, in column then row order, each with its
/// mid in the manifest), and uploads its lo pack (none when it made no tile: the manifest's then
/// dropped). Says how far it is as `build_q_with` does.
pub fn build_lo(out: &mut Out, raw: &RawTiles, q: (u32, u32), ts: &[(u32, u32)], src: &Sources, progress: crate::rawpack::Progress) -> anyhow::Result<PackReport> {
    use anyhow::Context;
    let mut rep = PackReport::default();
    let mut mids: Vec<((u32, u32), Mid)> = Vec::new();
    let p = crate::timings::phase("the pieces' mids read", crate::timings::Class::NasRead);
    for &t in ts {
        anyhow::ensure!((t.0 >> 3, t.1 >> 3) == q, "6/{}/{} isn't in 3/{}/{}", t.0, t.1, q.0, q.1);
        p.count(0, 1);
        let c = out.get(&mid_logical(t.0, t.1)).with_context(|| format!("6/{}/{} has no mid yet: its area can't be assembled", t.0, t.1))?;
        let (of, mid) = read_mid(&out.path(c))?;
        anyhow::ensure!(of == t, "{c} is 6/{}/{}'s mid", of.0, of.1);
        mids.push((t, mid));
    }
    drop(p);
    let mk = Maker::new(raw, src);
    let refs: Vec<((u32, u32), &Mid)> = mids.iter().map(|(t, m)| (*t, m)).collect();
    let p = crate::timings::phase("the area's tiles made (z8 → z3)", crate::timings::Class::Mixed);
    let lo = saying(&mk, 1365, progress, || mk.lo(q, &refs))?;
    drop(p);
    mk.report(&mut rep);
    let up = crate::timings::phase("the lo pack uploaded", crate::timings::Class::NasWrite);
    progress("packs", 0, 1);
    rep.lo_tiles = lo.len();
    let mut it = lo.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    if crate::layers::write_pack(out, "terrain", "terrarium-png", false, "lo", (3, q.0, q.1), &mut it)?.is_none() {
        drop_named(out, &[format!("layers/terrain/lo/3-{}-{}", q.0, q.1)]);
    }
    drop(up);
    out.save()?;
    progress("packs", 1, 1);
    Ok(rep)
}

/// The root pack (z0–2) remade from the 64 z3 tiles as stored in the lo packs (their 2×2 means),
/// over AWS's raw z0–2 tiles; deterministic given the lo packs.
pub fn build_root(out: &mut Out, raw: &RawTiles, src: &Sources) -> anyhow::Result<usize> {
    let p = crate::timings::phase("the z3 tiles read from the lo packs", crate::timings::Class::NasRead);
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
    drop(p);
    let p = crate::timings::phase("the root's tiles made (AWS's z0–2 read)", crate::timings::Class::Compute);
    let mut tiles: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
    let below: HashMap<(u32, u32), Repaired> = HashMap::new();
    for z in (0..=2u8).rev() {
        let n = 1u32 << z;
        let mut next = HashMap::new();
        for x in 0..n {
            for y in 0..n {
                let Some(b) = raw.get(z, x, y)?.0 else { continue };
                let (b, _, q) = process(b, z, x, y, &below, &quads, src);
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
    drop(p);
    let up = crate::timings::phase("the root pack uploaded", crate::timings::Class::NasWrite);
    let n = tiles.len();
    let mut it = tiles.into_iter();
    crate::layers::write_pack(out, "terrain", "terrarium-png", false, "root", (0, 0, 0), &mut it)?;
    drop(up);
    out.save()?;
    Ok(n)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A pack copied here reads the same tiles as the NAS's, and the copy goes with the reader: a
    /// pack it isn't asked to copy, or none there, is read where it is.
    #[test]
    fn a_pack_copied_here_reads_the_same_and_goes_with_its_reader() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::open(&root, &d.path().join("scratch")).unwrap();
        let mut tiles: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
        for z in 9..=10u8 {
            let s = 1u32 << (z - 6);
            for x in 5 * s..6 * s {
                for y in 7 * s..8 * s {
                    tiles.push((z, x, y, format!("tile {z}/{x}/{y}").into_bytes(), 1));
                }
            }
        }
        let want: Vec<(u8, u32, u32, Vec<u8>)> = tiles.iter().map(|t| (t.0, t.1, t.2, t.3.clone())).collect();
        crate::layers::write_pack(&mut out, "terrain", "terrarium-png", false, "hi", (6, 5, 7), &mut tiles.into_iter()).unwrap();
        let copies = d.path().join("copies");
        let plain = ManifestTiles::new(&out, "terrain");
        let here = ManifestTiles::with_copies(&out, "terrain", copies.clone());
        let read = |m: &ManifestTiles| want.iter().map(|t| m.get(t.0, t.1, t.2).unwrap()).collect::<Vec<_>>();
        let all: Vec<Option<Vec<u8>>> = want.iter().map(|t| Some(t.3.clone())).collect();
        assert_eq!(read(&plain), all);
        // Some read from the NAS's first, then the rest from the copy.
        assert_eq!(here.get(9, 40, 56).unwrap(), Some(b"tile 9/40/56".to_vec()));
        let n = here.copy_here(9, 40, 56).unwrap();
        assert_eq!(n, std::fs::metadata(out.path(out.get("layers/terrain/hi/6-5-7").unwrap())).unwrap().len());
        assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 1);
        assert_eq!(read(&here), all);
        assert!(here.has(10, 85, 120).unwrap() && !here.has(10, 200, 120).unwrap());
        assert_eq!(here.get(10, 200, 120).unwrap(), None);
        // No such pack: nothing copied. And none copied by a reader without a copies' folder.
        assert_eq!(here.copy_here(9, 0, 0), None);
        assert_eq!(plain.copy_here(9, 40, 56), None);
        drop(here);
        assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 0, "the copies go with their reader");
        // Many at once, in spans: the same bytes as one by one, in the order asked, None where
        // there's none.
        let mut ask: Vec<(u8, u32, u32)> = want.iter().rev().map(|t| (t.0, t.1, t.2)).collect();
        ask.insert(3, (10, 200, 120));
        ask.push((9, 0, 0));
        let one_by_one: Vec<Option<Vec<u8>>> = ask.iter().map(|t| plain.get(t.0, t.1, t.2).unwrap()).collect();
        assert_eq!(plain.get_many(&ask).unwrap(), one_by_one);
        assert_eq!(one_by_one.iter().filter(|t| t.is_none()).count(), 2);
    }

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
        build_q_with(&mut out, &raw, (1, 1), &[], &cov, &Sources::default(), &|w, d, t| said.lock().unwrap().push((w.to_string(), d, t))).unwrap();
        let said = said.into_inner().unwrap();
        // Every level's tiles (1 + 4 + … + 1024), then the one pack (lo), done; never past a total.
        assert!(said.contains(&("tiles".to_string(), 1365, 1365)));
        assert_eq!(said.last(), Some(&("packs".to_string(), 1, 1)));
        assert!(said.iter().all(|(_, d, t)| d <= t));
    }

    /// The area's terrain made as it was before it was written a z6 tile at a time: every level at
    /// once, every tile held until the packs are written (the reference for the test below).
    fn whole_area(out: &mut Out, raw: &RawTiles, q: (u32, u32), ts: &[(u32, u32)], cov: &Coverage) {
        let mut levels: Vec<(u8, Vec<(u32, u32)>)> = Vec::new();
        for z in (9..=12u8).rev() {
            let s = 1u32 << (z - 6);
            let tiles = ts.iter().flat_map(|&(tx, ty)| (tx * s..(tx + 1) * s).flat_map(move |x| (ty * s..(ty + 1) * s).map(move |y| (x, y)))).filter(|&(x, y)| z <= max_zoom_at(tile_lat(z, y)) && near_coverage(cov, z, x, y, 20.0)).collect();
            levels.push((z, tiles));
        }
        for z in (3..=8u8).rev() {
            let s = 1u32 << (z - 3);
            levels.push((z, (q.0 * s..(q.0 + 1) * s).flat_map(|x| (q.1 * s..(q.1 + 1) * s).map(move |y| (x, y))).collect()));
        }
        let (mut hi, mut lo): (HashMap<(u32, u32), Vec<(u8, u32, u32, Vec<u8>)>>, Vec<(u8, u32, u32, Vec<u8>)>) = (HashMap::new(), Vec::new());
        let (mut below, mut quads) = (HashMap::new(), HashMap::new());
        for (z, tiles) in levels {
            let (mut nb, mut nq) = (HashMap::new(), HashMap::new());
            for (x, y) in tiles {
                let Some(b) = raw.get(z, x, y).unwrap().0 else { continue };
                let (b, r, qd) = process(b, z, x, y, &below, &quads, &Sources::default());
                if let Some(r) = r {
                    nb.insert((x, y), r);
                }
                if let Some(qd) = qd {
                    nq.insert((x, y), qd);
                }
                if z >= 9 {
                    hi.entry((x >> (z - 6), y >> (z - 6))).or_default().push((z, x, y, b));
                } else {
                    lo.push((z, x, y, b));
                }
            }
            (below, quads) = (nb, nq);
        }
        let pack = |mut v: Vec<(u8, u32, u32, Vec<u8>)>| {
            v.sort_by_key(|t| (t.0, t.1, t.2));
            v.into_iter().map(|(z, x, y, b)| {
                let n = b.len() as u32;
                (z, x, y, b, n)
            })
        };
        for &(tx, ty) in ts {
            crate::layers::write_pack(out, "terrain", "terrarium-png", false, "hi", (6, tx, ty), &mut pack(hi.remove(&(tx, ty)).unwrap_or_default())).unwrap();
        }
        crate::layers::write_pack(out, "terrain", "terrarium-png", false, "lo", (3, q.0, q.1), &mut pack(lo)).unwrap();
        out.save().unwrap();
    }

    /// A small coverage in the far north and its area's raw tiles, written under `local` as a
    /// cache keeps them: the area, its z6 tiles near the coverage, and the coverage.
    fn north_tiles(d: &std::path::Path, local: &std::path::Path) -> ((u32, u32), Vec<(u32, u32)>, Coverage) {
        // A small coverage in the far north (z11 the finest there: fewer tiles) on two z6 tiles'
        // edge, and its area's z6 tiles near it.
        let cov = Coverage::from_recipes(&[crate::agent::recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:11.25,70.5,2".into()] }], None, d).unwrap();
        let by_q = crate::agent::build::coverage_tiles(&cov);
        let (&q, ts) = by_q.iter().next().unwrap();
        assert!(ts.len() >= 2, "{ts:?}");
        // Raw tiles: smooth slopes, every third with a spike (a repair, whose pixels the level above
        // takes in), every seventh missing (the open sea); every level's, z12 to z3.
        let mut want: Vec<(u8, u32, u32)> = Vec::new();
        for z in 9..=12u8 {
            let s = 1u32 << (z - 6);
            for &(tx, ty) in ts {
                for x in tx * s..(tx + 1) * s {
                    for y in ty * s..(ty + 1) * s {
                        if near_coverage(&cov, z, x, y, 20.0) {
                            want.push((z, x, y));
                        }
                    }
                }
            }
        }
        for z in 3..=8u8 {
            let s = 1u32 << (z - 3);
            for x in q.0 * s..(q.0 + 1) * s {
                for y in q.1 * s..(q.1 + 1) * s {
                    want.push((z, x, y));
                }
            }
        }
        // (Of the zoomed-out levels, the tiles over those z6 tiles alone: the others missing.)
        let over = |z: u8, x: u32, y: u32| z >= 9 || ts.iter().any(|&(tx, ty)| if z >= 6 { (x >> (z - 6), y >> (z - 6)) == (tx, ty) } else { (tx >> (6 - z), ty >> (6 - z)) == (x, y) });
        for &(z, x, y) in &want {
            std::fs::create_dir_all(local.join(format!("{z}/{x}"))).unwrap();
            let k = z as u32 + x + y;
            if k % 7 == 0 || !over(z, x, y) {
                std::fs::write(local.join(format!("{z}/{x}/{y}.none")), b"").unwrap();
                continue;
            }
            let mut e: Vec<f32> = (0..256 * 256).map(|i| 400.0 + (i % 256) as f32 * 0.7 + (i / 256) as f32 * 0.4 + (k % 11) as f32 * 30.0).collect();
            if k % 3 == 0 {
                e[128 * 256 + 128] += 900.0;
                e[64 * 256 + 200] -= 700.0;
            }
            std::fs::write(local.join(format!("{z}/{x}/{y}.png")), encode_terrain_png(&e, 256, 256).unwrap()).unwrap();
        }
        (q, ts.clone(), cov)
    }

    #[test]
    fn a_z6_tile_at_a_time_makes_what_the_whole_area_did() {
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("local");
        let (q, ts, cov) = north_tiles(d.path(), &local);
        let ts = &ts;
        let raw = RawTiles::with_store(&local, &d.path().join("store"));
        let made = |root: &str, way: &dyn Fn(&mut Out)| {
            let mut out = Out::open(&d.path().join(root), &d.path().join(format!("{root}-scratch"))).unwrap();
            way(&mut out);
            let out = Out::open(&d.path().join(root), &d.path().join(format!("{root}-scratch"))).unwrap();
            out.manifest.into_iter().filter(|(l, _)| l.starts_with("layers/terrain/")).collect::<Vec<_>>()
        };
        let now = made("now", &|out| {
            build_q(out, &raw, q, ts, &cov, &Sources::default()).unwrap();
        });
        let before = made("before", &|out| whole_area(out, &raw, q, ts, &cov));
        assert_eq!(now.len(), ts.len() + 1);
        assert_eq!(now, before, "the same packs, byte for byte (their content names)");
    }

    /// Room-making mid-job (docs/plan.md §8, store::cachefile): a terrain run whose raw tiles come
    /// from the NAS (half loose there, half in their area's archives), while every file of its
    /// cache no job holds is deleted as fast as it can be, makes the same packs, byte for byte, as
    /// a run left alone.
    #[test]
    fn a_run_whose_cache_is_deleted_under_it_makes_the_same_packs() {
        // (No network: what isn't on the scratch NAS fails, never downloads.)
        crate::fetch::go_offline();
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let store = root.join("sources/aws-terrarium");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let staged = d.path().join("staged");
        let (q, ts, cov) = north_tiles(d.path(), &staged);
        // Half the tiles loose on the NAS, the rest packed into their area's archives there.
        let mut files = Vec::new();
        crate::agent::room::walk_for_test(&staged, &mut files);
        let old = std::time::SystemTime::now() - Duration::from_secs(120);
        for (i, f) in files.iter().enumerate() {
            if i % 2 == 0 {
                let to = store.join(f.strip_prefix(&staged).unwrap());
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                std::fs::rename(f, &to).unwrap();
            } else {
                std::fs::File::options().append(true).open(f).unwrap().set_modified(old).unwrap();
            }
        }
        assert!(crate::rawpack::pack_local(&staged, &store, &root, false).unwrap() > 0);
        let made = |name: &str, cache: &std::path::Path| {
            let raw = RawTiles::with_store(cache, &store);
            let mut out = Out::open(&d.path().join(name), &d.path().join(format!("{name}-scratch"))).unwrap();
            build_q(&mut out, &raw, q, &ts, &cov, &Sources::default()).unwrap();
            let out = Out::open(&d.path().join(name), &d.path().join(format!("{name}-scratch"))).unwrap();
            out.manifest.into_iter().filter(|(l, _)| l.starts_with("layers/terrain/")).collect::<Vec<_>>()
        };
        let calm = made("calm", &d.path().join("calm-cache"));
        // The other run's cache deleted under it, file by file, as fast as it goes.
        let busy_cache = d.path().join("busy-cache");
        let stop = std::sync::atomic::AtomicBool::new(false);
        let (busy, freed) = std::thread::scope(|s| {
            let deleter = s.spawn(|| {
                let mut freed = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    freed += store::cachefile::remove_tree(&busy_cache).freed;
                }
                freed
            });
            // (A failure lets the deleter go too: the scope waits for it.)
            let busy = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| made("busy", &busy_cache)));
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            (busy.unwrap_or_else(|e| std::panic::resume_unwind(e)), deleter.join().unwrap())
        });
        assert!(freed > 0, "the deleter took nothing");
        assert_eq!(calm.len(), ts.len() + 1);
        assert_eq!(busy, calm, "the same packs, byte for byte (their content names)");
    }

    /// The timed demonstration (docs/plan.md §8, Room on the disk; run by hand: `cargo test
    /// --release -p pipeline demo_room_making_mid_job -- --ignored --nocapture`, scratch dir in
    /// SCENIC_DEMO_DIR, else a temporary one): a scratch agent cache holding copies of a scratch
    /// NAS's files (SCENIC_DEMO_MB of them, 1024 by default) and a terrain run reading its raw tiles
    /// through it; the room target set past the free space, so the agent's freeing toward it
    /// (room::toward, as its loop runs it) deletes while the run goes on. The free space before and
    /// after, what went, and the run's packs against a run left alone.
    #[test]
    #[ignore]
    fn demo_room_making_mid_job() {
        crate::fetch::go_offline();
        let keep = std::env::var_os("SCENIC_DEMO_DIR").map(std::path::PathBuf::from);
        let tmp = tempfile::tempdir_in(keep.as_deref().unwrap_or(&std::env::temp_dir())).unwrap();
        let d = tmp.path();
        let mb: u64 = std::env::var("SCENIC_DEMO_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
        let root = d.join("nas");
        let (sources, store) = (root.join("sources"), root.join("sources/aws-terrarium"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // The raw tiles, loose on the NAS.
        let staged = d.join("staged");
        let (q, ts, cov) = north_tiles(d, &staged);
        std::fs::create_dir_all(&sources).unwrap();
        std::fs::rename(&staged, &store).unwrap();
        // The NAS's records' files, and this Mac's copies of them (blobs/): what the target frees.
        let cache = d.join("cache");
        let n = mb / 32;
        for i in 0..n {
            let name = format!("layers/demo/6-{i}-0.{i:016x}.pack");
            let b: Vec<u8> = (0..32u64 << 20).map(|k| (k * 31 + i) as u8).collect();
            for at in [root.join(&name), cache.join("blobs").join(&name)] {
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::fs::write(&at, &b).unwrap();
            }
        }
        let made = |name: &str, cache: &std::path::Path| {
            let raw = RawTiles::with_store(&cache.join("aws-terrarium"), &store);
            let mut out = Out::open(&d.join(name), &d.join(format!("{name}-scratch"))).unwrap();
            build_q(&mut out, &raw, q, &ts, &cov, &Sources::default()).unwrap();
            let out = Out::open(&d.join(name), &d.join(format!("{name}-scratch"))).unwrap();
            out.manifest.into_iter().filter(|(l, _)| l.starts_with("layers/terrain/")).collect::<Vec<_>>()
        };
        let t = std::time::Instant::now();
        let calm = made("calm", &d.join("calm-cache"));
        let calm_s = t.elapsed().as_secs_f64();
        // The run, and the agent's freeing toward a target past the free space, at once.
        let free_before = crate::agent::room::disk_free(d).unwrap();
        let target = free_before + (mb << 20) * 3 / 4;
        let goal = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(target));
        let t = std::time::Instant::now();
        let (busy, freed, freeing_s) = std::thread::scope(|s| {
            let job = s.spawn(|| made("busy", &cache));
            // (Once the run is under way: its first tiles copied here.)
            while !cache.join("aws-terrarium").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            let t0 = std::time::Instant::now();
            let freed = crate::agent::room::toward(&cache, &sources, goal.clone(), &Default::default()).unwrap();
            let freeing_s = t0.elapsed().as_secs_f64();
            (job.join().unwrap(), freed, freeing_s)
        });
        let busy_s = t.elapsed().as_secs_f64();
        let free_after = crate::agent::room::disk_free(d).unwrap();
        let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
        println!("demo: scratch {} ({} MB of copies of the NAS's files in the cache, a terrain run of {} z6 tiles)", d.display(), n * 32, ts.len());
        println!("demo: free before {:.2} GB, target {:.2} GB; after {:.2} GB ({:+.2} GB)", gb(free_before), gb(target), gb(free_after), gb(free_after) - gb(free_before));
        println!("demo: freed toward the target in {freeing_s:.1} s while the run went on: {}", freed.say());
        println!("demo: the run alone {calm_s:.1} s, with the freeing {busy_s:.1} s; packs alike: {}", busy == calm);
        assert!(freed.bytes() > 0 && free_after > free_before);
        assert_eq!(busy, calm, "the same packs, byte for byte");
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
        // A prefetch passes archived tiles over, reading none of them: however often, the area's
        // archive isn't copied here (as reading its tiles that often would).
        let fresh = RawTiles::with_store(&d.path().join("fresh"), &store);
        for _ in 0..=RANGED / 2 {
            assert_eq!(fresh.prefetch(12, &[(2048, 1365), (2049, 1365)], 2).unwrap(), 0);
        }
        assert!(!d.path().join("fresh/12/2048/1365.png").exists() && !d.path().join("fresh/packs").exists());
        assert_eq!(fresh.get(12, 2048, 1365).unwrap(), (Some(png.clone()), false));
    }

    /// The area's terrain as main made it at 1fa03d8 (TERRAIN_V 3), before it was made as pieces and an
    /// assembly: the reference the pieces and assembly must match, byte for byte.
    fn area_v3(out: &mut Out, raw: &RawTiles, q: (u32, u32), ts: &[(u32, u32)], cov: &Coverage, src: &Sources, progress: crate::rawpack::Progress) -> anyhow::Result<PackReport> {
        let mut rep = PackReport::default();
        // Each z6 tile's levels' tiles, z12 → z9, near the coverage, as fine as the latitude allows; then
        // q's, z8 → z3, the whole of it.
        let own = |tx: u32, ty: u32| -> Vec<(u8, Vec<(u32, u32)>)> {
            (9..=12u8)
                .rev()
                .map(|z| {
                    let s = 1u32 << (z - 6);
                    let tiles = (tx * s..(tx + 1) * s).flat_map(|x| (ty * s..(ty + 1) * s).map(move |y| (x, y))).filter(|&(x, y)| z <= max_zoom_at(tile_lat(z, y)) && near_coverage(cov, z, x, y, 20.0)).collect();
                    (z, tiles)
                })
                .collect()
        };
        let mine: Vec<Vec<(u8, Vec<(u32, u32)>)>> = ts.iter().map(|&(tx, ty)| own(tx, ty)).collect();
        let upper: Vec<(u8, Vec<(u32, u32)>)> = (3..=8u8)
            .rev()
            .map(|z| {
                let s = 1u32 << (z - 3);
                (z, (q.0 * s..(q.0 + 1) * s).flat_map(|x| (q.1 * s..(q.1 + 1) * s).map(move |y| (x, y))).collect())
            })
            .collect();
        let total: u64 = mine.iter().flatten().chain(&upper).map(|(_, t)| t.len() as u64).sum();
        // (Here, and shaded: each tile counts in both.)
        let (here, processed) = (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0));
        let done = || (here.load(std::sync::atomic::Ordering::Relaxed) + processed.load(std::sync::atomic::Ordering::Relaxed)) / 2;
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
        // One level: every tile fetched or reused, then made with what the level below made: first all
        // but their water (`prepare`), then each lake's level from all its shore in the level (those
        // known from finer levels kept: `levels`), then their water (`finish`).
        let level = |z: u8, tiles: Vec<(u32, u32)>, below: &HashMap<(u32, u32), Repaired>, quads: &HashMap<(u32, u32), Vec<f32>>, levels: &mut HashMap<u64, f32>| -> anyhow::Result<(Vec<(u32, u32, Vec<u8>)>, HashMap<(u32, u32), Repaired>, HashMap<(u32, u32), Vec<f32>>)> {
            fetched.fetch_add(raw.prefetch_counted(z, &tiles, FETCH_THREADS, &here)?, std::sync::atomic::Ordering::Relaxed);
            let prepared: Vec<anyhow::Result<Option<(u32, u32, Prepared)>>> = tiles
                .par_iter()
                .map(|&(x, y)| {
                    let Some(b) = get(z, x, y)? else {
                        processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return Ok(None);
                    };
                    Ok(Some((x, y, prepare(b, z, x, y, below, quads, src, &|e, z, lat, c| {
                        repair_terrain_with(e, z, lat, c);
                    }))))
                })
                .collect();
            let mut prepared: Vec<(u32, u32, Prepared)> = prepared.into_iter().filter_map(|r| r.transpose()).collect::<anyhow::Result<_>>()?;
            let mut lakes = HashMap::new();
            for (_, _, p) in &prepared {
                crate::terrain_water::gather(&mut lakes, p.lake_samples());
            }
            crate::terrain_water::add_levels(levels, &lakes);
            let levels: &HashMap<u64, f32> = levels;
            let done: Vec<(u32, u32, Vec<u8>, Option<Repaired>, Option<Vec<f32>>)> = prepared
                .par_drain(..)
                .map(|(x, y, p)| {
                    let (b, r, q) = finish(p, levels);
                    processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    (x, y, b, r, q)
                })
                .collect();
            let (mut outs, mut nb, mut nq) = (Vec::new(), HashMap::new(), HashMap::new());
            for (x, y, b, r, q) in done {
                if let Some(r) = r {
                    repaired.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    nb.insert((x, y), r);
                }
                if let Some(q) = q {
                    nq.insert((x, y), q);
                }
                outs.push((x, y, b));
            }
            Ok((outs, nb, nq))
        };
        let mut lo: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
        // The z6 tiles, each written as it's done, then q's levels (z8 → z3 fold in the levels above
        // what was made just before them); the tiles done said every few seconds meanwhile.
        let finished = std::sync::atomic::AtomicBool::new(false);
        // (Set however the levels end, a panic too: the scope waits for the reporter before it goes on.)
        struct Finished<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Finished<'_> {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        std::thread::scope(|s| {
            s.spawn(|| {
                let mut said = std::time::Instant::now();
                while !finished.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                    if said.elapsed() >= Duration::from_secs(5) {
                        progress("tiles", done().min(total), total);
                        said = std::time::Instant::now();
                    }
                }
            });
            let _finished = Finished(&finished);
            let r = (|| -> anyhow::Result<()> {
                let (mut below9, mut quads9): (HashMap<(u32, u32), Repaired>, HashMap<(u32, u32), Vec<f32>>) = (HashMap::new(), HashMap::new());
                // (The lakes' levels: each z6 tile's, finest first, then all of them for q's levels.)
                let mut lakes_q: HashMap<u64, f32> = HashMap::new();
                for (&(tx, ty), levels) in ts.iter().zip(mine) {
                    let (mut below, mut quads) = (HashMap::new(), HashMap::new());
                    let mut hi: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
                    let mut lakes: HashMap<u64, f32> = HashMap::new();
                    for (z, tiles) in levels {
                        let (outs, nb, nq) = level(z, tiles, &below, &quads, &mut lakes)?;
                        hi.extend(outs.into_iter().map(|(x, y, b)| (z, x, y, b)));
                        (below, quads) = (nb, nq);
                    }
                    for (k, v) in lakes {
                        lakes_q.entry(k).or_insert(v);
                    }
                    // (Its z9 tiles': what q's z8 reads.)
                    below9.extend(below);
                    quads9.extend(quads);
                    hi.sort_by_key(|t| (t.0, t.1, t.2));
                    rep.hi_tiles += hi.len();
                    let mut it = hi.into_iter().map(|(z, x, y, b)| {
                        let n = b.len() as u32;
                        (z, x, y, b, n)
                    });
                    crate::layers::write_pack(out, "terrain", "terrarium-png", false, "hi", (6, tx, ty), &mut it)?;
                }
                let (mut below, mut quads) = (below9, quads9);
                for (z, tiles) in upper {
                    let (outs, nb, nq) = level(z, tiles, &below, &quads, &mut lakes_q)?;
                    lo.extend(outs.into_iter().map(|(x, y, b)| (z, x, y, b)));
                    (below, quads) = (nb, nq);
                }
                Ok(())
            })();
            drop(_finished);
            r
        })?;
        progress("tiles", total, total);
        rep.fetched = fetched.into_inner();
        rep.missing = missing.into_inner();
        rep.repaired = repaired.into_inner();
        // Then q's lo pack.
        progress("packs", 0, 1);
        lo.sort_by_key(|t| (t.0, t.1, t.2));
        rep.lo_tiles = lo.len();
        let mut it = lo.into_iter().map(|(z, x, y, b)| {
            let n = b.len() as u32;
            (z, x, y, b, n)
        });
        crate::layers::write_pack(out, "terrain", "terrarium-png", false, "lo", (3, q.0, q.1), &mut it)?;
        out.save()?;
        progress("packs", 1, 1);
        Ok(rep)
    }

    /// Raw tiles for the area of `ts` (z3 tile `q`): every z9–12 tile near `cov` and every z3–8 tile
    /// of q; smooth slopes, every third with a spike and a pit (repairs, which the level above takes
    /// in), every seventh missing (the open sea).
    pub(crate) fn synthetic_raw(local: &std::path::Path, cov: &Coverage, q: (u32, u32), ts: &[(u32, u32)]) {
        let mut want: Vec<(u8, u32, u32)> = Vec::new();
        for z in 9..=12u8 {
            let s = 1u32 << (z - 6);
            for &(tx, ty) in ts {
                for x in tx * s..(tx + 1) * s {
                    for y in ty * s..(ty + 1) * s {
                        if near_coverage(cov, z, x, y, 20.0) {
                            want.push((z, x, y));
                        }
                    }
                }
            }
        }
        want.extend(upper_levels(q).into_iter().flat_map(|(z, t)| t.into_iter().map(move |(x, y)| (z, x, y))));
        for &(z, x, y) in &want {
            std::fs::create_dir_all(local.join(format!("{z}/{x}"))).unwrap();
            let k = z as u32 + x + y;
            if k % 7 == 0 {
                std::fs::write(local.join(format!("{z}/{x}/{y}.none")), b"").unwrap();
                continue;
            }
            let mut e: Vec<f32> = (0..256 * 256).map(|i| 400.0 + (i % 256) as f32 * 0.7 + (i / 256) as f32 * 0.4 + (k % 11) as f32 * 30.0).collect();
            if k % 3 == 0 {
                e[128 * 256 + 128] += 900.0;
                e[64 * 256 + 200] -= 700.0;
            }
            std::fs::write(local.join(format!("{z}/{x}/{y}.png")), encode_terrain_png(&e, 256, 256).unwrap()).unwrap();
        }
    }

    /// A lake across two z6 tiles' edge (lon 11.25°, 6/33 and 6/34) and the sea north of it, in every
    /// zoom's tiles from z6 (as the basemap draws them).
    fn lake_across(z: u8, x: u32, y: u32) -> Vec<crate::terrain_water::Poly> {
        use crate::terrain_water::{Kind, Poly};
        if z < 6 {
            return Vec::new();
        }
        let n = (1u64 << z) as f64 * 256.0;
        let px = |lon: f64| (lon + 180.0) / 360.0 * n - x as f64 * 256.0;
        let py = |lat: f64| (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n - y as f64 * 256.0;
        let rect = |kind, id, w: f64, s: f64, e: f64, nn: f64| Poly { kind, id, rings: vec![vec![[px(w), py(nn)], [px(e), py(nn)], [px(e), py(s)], [px(w), py(s)]]] };
        vec![rect(Kind::Lake, 5, 11.0, 70.42, 11.5, 70.5), rect(Kind::Sea, 0, 10.9, 70.56, 11.6, 70.6)]
    }

    /// The pieces and the assembly (each from the mids as uploaded) make what main's area run made,
    /// byte for byte, with GLO-30, the water (a lake across two pieces: its level the first's) and
    /// AWS's z9 tiles for the walled patches; so does the area run made of them. A piece made again
    /// expecting the same changes nothing; one that isn't the same uploads nothing.
    #[test]
    fn pieces_and_their_assembly_make_the_area_runs_packs() {
        use crate::terrain_north::tests::FnCells;
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("local");
        let cov = Coverage::from_recipes(&[crate::agent::recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:11.25,70.5,2".into()] }], None, d.path()).unwrap();
        let by_q = crate::agent::build::coverage_tiles(&cov);
        let (&q, ts) = by_q.iter().next().unwrap();
        assert!(ts.len() >= 2, "{ts:?}");
        synthetic_raw(&local, &cov, q, ts);
        let raw = RawTiles::with_store(&local, &d.path().join("store"));
        let cells = FnCells::new(|lat, lon| (200.0 + (lat - 70.0) * 100.0 + (lon - 10.0) * 50.0) as f32);
        let water = crate::terrain_water::tests::Fixed(lake_across);
        let coarse = Coarse::new(&raw);
        let src = Sources { north: Some(&cells), water: Some(&water), coarse: Some(&coarse) };
        let open = |root: &str| Out::open(&d.path().join(root), &d.path().join(format!("{root}-scratch"))).unwrap();
        let terrain = |o: &Out| o.manifest.iter().filter(|(l, _)| l.starts_with("layers/terrain/")).map(|(l, c)| (l.clone(), c.clone())).collect::<Vec<_>>();
        let mut v3 = open("v3");
        let r = area_v3(&mut v3, &raw, q, ts, &cov, &src, &|_, _, _| {}).unwrap();
        assert!(r.repaired > 0, "{r:?}");
        let mut area = open("area");
        build_q(&mut area, &raw, q, ts, &cov, &src).unwrap();
        let mut parts = open("parts");
        for &t in ts {
            build_piece(&mut parts, &raw, t, &cov, &src, false, &|_, _, _| {}).unwrap();
        }
        // (The lake's level, from each of the two pieces' own shore.)
        let mids: Vec<Mid> = ts.iter().map(|t| read_mid(&parts.path(parts.get(&mid_logical(t.0, t.1)).unwrap())).unwrap().1).collect();
        assert!(mids.iter().filter(|m| m.levels.contains_key(&5)).count() >= 2, "{:?}", mids.iter().map(|m| &m.levels).collect::<Vec<_>>());
        assert!(mids.iter().any(|m| !m.quads.is_empty()));
        build_lo(&mut parts, &raw, q, ts, &src, &|_, _, _| {}).unwrap();
        assert_eq!(terrain(&v3).len(), ts.len() + 1);
        assert_eq!(terrain(&area), terrain(&v3), "the area run as pieces and an assembly in memory");
        assert_eq!(terrain(&parts), terrain(&v3), "the pieces' jobs and the assembly's");
        // A mid as written reads back bit for bit.
        let p = d.path().join("again.sect");
        write_mid(&p, ts[0], &mids[0]).unwrap();
        let (t, back) = read_mid(&p).unwrap();
        let bits = |m: &Mid| (m.quads.iter().map(|(k, v)| (*k, v.iter().map(|f| f.to_bits()).collect::<Vec<_>>())).collect::<Vec<_>>(), m.levels.iter().map(|(k, v)| (*k, v.to_bits())).collect::<Vec<_>>());
        assert_eq!((t, bits(&back)), (ts[0], bits(&mids[0])));
        // Made again expecting the same (its mid made again): nothing changes.
        let before = parts.manifest.clone();
        let (t, ml) = (ts[1], mid_logical(ts[1].0, ts[1].1));
        parts.remove(&ml);
        build_piece(&mut parts, &raw, t, &cov, &src, true, &|_, _, _| {}).unwrap();
        assert_eq!(parts.manifest, before);
        // A hi pack that isn't what the manifest has: refused, nothing uploaded.
        let hi = format!("layers/terrain/hi/6-{}-{}", t.0, t.1);
        parts.manifest.insert(hi.clone(), format!("{hi}.0000000000000000.pack"));
        parts.remove(&ml);
        let e = build_piece(&mut parts, &raw, t, &cov, &src, true, &|_, _, _| {}).unwrap_err().to_string();
        assert!(e.contains(&hi) && e.contains("nothing uploaded"), "{e}");
        assert!(parts.get(&ml).is_none(), "its mid wasn't uploaded");
        // An assembly without a piece's mid: refused.
        assert!(build_lo(&mut parts, &raw, q, ts, &src, &|_, _, _| {}).unwrap_err().to_string().contains("no mid"));
    }

    /// Byte identity on the build's own data, by hand on the build Mac (§10): the terrain and slope
    /// packs of the z3 tiles `P5_AREAS` ("3/2/2,3/0/2") in two roots' manifests, `P5_A` (an area's
    /// whole run, main's) and `P5_B` (its pieces and assembly), and `P5_LIVE` (the NAS's, if set):
    /// each pack's content name, and every tile's pixels (terrain's heights, slope's quarters),
    /// those along a z6 tile's edge counted apart.
    #[test]
    #[ignore]
    fn p5_compare_real() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} isn't set"));
        let roots: Vec<(String, std::path::PathBuf)> = ["P5_A", "P5_B", "P5_LIVE"].iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), std::path::PathBuf::from(v)))).collect();
        let manifests: Vec<std::collections::BTreeMap<String, String>> = roots.iter().map(|(_, r)| crate::out::read_record(&r.join("state/build/manifest.json")).unwrap()).collect();
        let mut failed = false;
        for a in env("P5_AREAS").split(',') {
            let q = crate::legacy::Unit::parse(a).filter(|u| u.z == 3).unwrap();
            for layer in ["terrain", "slope"] {
                let of_q = |l: &str| l.strip_prefix(&format!("layers/{layer}/")).is_some_and(|r| r == format!("lo/3-{}-{}", q.x, q.y) || r.strip_prefix("hi/").and_then(|t| crate::legacy::Unit::parse(&t.replace('-', "/"))).is_some_and(|u| (u.x >> 3, u.y >> 3) == (q.x, q.y)));
                let logicals: std::collections::BTreeSet<&String> = manifests.iter().flat_map(|m| m.keys().filter(|l| of_q(l))).collect();
                let (mut packs, mut same_names, mut tiles, mut same_px, mut edge, mut edge_same) = (0, 0, 0usize, 0usize, 0usize, 0usize);
                for l in logicals {
                    packs += 1;
                    let names: Vec<Option<&String>> = manifests.iter().map(|m| m.get(l)).collect();
                    if names.iter().all(|n| *n == names[0]) {
                        same_names += 1;
                    } else {
                        failed = true;
                        eprintln!("{l}: {:?}", roots.iter().zip(&names).map(|((k, _), n)| format!("{k} {}", n.map_or("none", |s| s.as_str()))).collect::<Vec<_>>());
                    }
                    // Every tile of the first root's pack, against the others'.
                    let open = |(r, n): (&std::path::PathBuf, Option<&String>)| n.map(|n| {
                        let f = store::range::PlainFile::open(&r.join(n)).unwrap();
                        let ix = store::pack::PackIndex::read_from(&f).unwrap();
                        (f, ix)
                    });
                    let opened: Vec<_> = roots.iter().map(|(_, r)| r).zip(names.iter().copied()).map(open).collect();
                    let Some((f0, ix0)) = &opened[0] else { continue };
                    for e in &ix0.entries {
                        let (z, x, y) = e.zxy();
                        let px = |f: &store::range::PlainFile, ix: &store::pack::PackIndex| -> Option<Vec<u32>> {
                            let e = ix.find(z, x, y)?;
                            let b = ix.read_blob(f, &e).ok()?;
                            if layer == "terrain" {
                                decode_terrain_png(&b).ok().map(|v| v.iter().map(|f| f.to_bits()).collect())
                            } else {
                                roadcore::slope::decode_slope4(&b).map(|v| v.iter().flat_map(|q| q.map(f32::to_bits)).collect())
                            }
                        };
                        let first = px(f0, ix0);
                        let same = opened[1..].iter().all(|o| o.as_ref().is_some_and(|(f, ix)| px(f, ix) == first));
                        tiles += 1;
                        same_px += same as usize;
                        // (Along a z6 tile's edge: its first or last column or row, z6 and finer.)
                        if z >= 6 {
                            let s = 1u32 << (z - 6);
                            if x % s == 0 || x % s == s - 1 || y % s == 0 || y % s == s - 1 {
                                edge += 1;
                                edge_same += same as usize;
                            }
                        }
                        if !same {
                            failed = true;
                            eprintln!("{l} {z}/{x}/{y}: pixels differ");
                        }
                    }
                }
                eprintln!("{a} {layer}: {packs} packs, {same_names} with the same content name in {}; {tiles} tiles, {same_px} with the same pixels; along the z6 tiles' edges {edge}, {edge_same} the same", roots.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", "));
            }
        }
        assert!(!failed);
    }

    /// A coverage near z6 tile 6/21/18 (its grown box meets it) whose z9–12 tiles none come near
    /// (each grown by less in longitude, at its own latitude): a small circle east of the tile's
    /// south-east corner, as the build's coverage left 6/21/18.
    pub(crate) fn near_without_hi(d: &std::path::Path) -> Coverage {
        let b = crate::stage::tile_box_grown(6, 21, 18, 0.0);
        let dx = |lat: f64| 20.0 / (111.320 * lat.to_radians().cos());
        let (lat, lon) = (b[1] + 0.05, b[2] + (dx(b[3]) + dx(b[1] + 0.5)) / 2.0);
        let c = Coverage::from_recipes(&[crate::agent::recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec![format!("place:{lon},{lat},0.5")] }], None, d).unwrap();
        assert!(near_coverage(&c, 6, 21, 18, 20.0) && !makes_hi(&c, (21, 18)) && piece_levels(&c, (21, 18)).iter().all(|(_, t)| t.is_empty()));
        c
    }

    /// A piece that makes no hi tile drops the hi pack an earlier run left (expected the same, it
    /// isn't: refused, nothing uploaded); a z6 tile the coverage has left drops its hi pack and mid
    /// (it can't be expected the same); neither touches another tile's.
    #[test]
    fn a_piece_making_no_hi_tile_or_left_by_the_coverage_drops_its_hi_pack() {
        let d = tempfile::tempdir().unwrap();
        let cov = near_without_hi(d.path());
        let raw = RawTiles::new(&d.path().join("raw"));
        let src = Sources { north: None, water: None, coarse: None };
        let mut out = Out::open(&d.path().join("root"), &d.path().join("scratch")).unwrap();
        // (Files an earlier run left: their bytes don't matter here.)
        let fake = |out: &mut Out, l: &str| {
            let ext = if l.starts_with("work/") { "sect" } else { "pack" };
            let c = out.put_bytes(l, ext, l.as_bytes()).unwrap();
            out.save().unwrap();
            c
        };
        let (hi, mid) = ("layers/terrain/hi/6-21-18".to_string(), mid_logical(21, 18));
        let other = "layers/terrain/hi/6-22-18".to_string();
        fake(&mut out, &hi);
        let kept = fake(&mut out, &other);
        let e = build_piece(&mut out, &raw, (21, 18), &cov, &src, true, &|_, _, _| {}).unwrap_err().to_string();
        assert!(e.contains(&hi) && e.contains("made none") && e.contains("nothing uploaded"), "{e}");
        assert!(out.get(&hi).is_some() && out.get(&mid).is_none());
        let r = build_piece(&mut out, &raw, (21, 18), &cov, &src, false, &|_, _, _| {}).unwrap();
        assert_eq!(r.hi_tiles, 0);
        assert!(out.get(&hi).is_none(), "the earlier run's hi pack dropped");
        assert!(out.get(&mid).is_some(), "its mid made");
        // Made again as it is: the same (no hi pack, the same mid).
        let before = out.manifest.clone();
        out.remove(&mid);
        build_piece(&mut out, &raw, (21, 18), &cov, &src, true, &|_, _, _| {}).unwrap();
        assert_eq!(out.manifest, before);
        // A z6 tile the coverage has left (far from it): its hi pack and mid go.
        let (lhi, lmid) = ("layers/terrain/hi/6-10-10".to_string(), mid_logical(10, 10));
        fake(&mut out, &lhi);
        fake(&mut out, &lmid);
        let e = build_piece(&mut out, &raw, (10, 10), &cov, &src, true, &|_, _, _| {}).unwrap_err().to_string();
        assert!(e.contains("the coverage has left it"), "{e}");
        build_piece(&mut out, &raw, (10, 10), &cov, &src, false, &|_, _, _| {}).unwrap();
        assert!(out.get(&lhi).is_none() && out.get(&lmid).is_none());
        assert_eq!(out.get(&other), Some(kept.as_str()), "another tile's hi pack stays");
        // Saved: the manifest on disk says the same.
        let again = Out::open(&d.path().join("root"), &d.path().join("scratch2")).unwrap();
        assert!(again.get(&hi).is_none() && again.get(&lhi).is_none() && again.get(&lmid).is_none() && again.get(&mid).is_some());
        assert_eq!(drop_piece(&mut out, (10, 10)).unwrap(), 0, "nothing left to drop");
        // An area's whole run drops the same hi packs: its piece's making no hi tile, and its z6
        // tile's the coverage has left (that tile's mid left to its "none" piece); its other
        // pieces' it writes. Every name it changes is one its lease may (agent::steps::saves).
        let by_q = crate::agent::build::coverage_tiles(&cov);
        let ts = by_q.get(&(2, 2)).unwrap().clone();
        assert!(ts.contains(&(21, 18)) && ts.contains(&(22, 18)), "{ts:?}");
        let local = d.path().join("area-raw");
        synthetic_raw(&local, &cov, (2, 2), &ts);
        let raw = RawTiles::with_store(&local, &d.path().join("store"));
        let mut area = Out::open(&d.path().join("area"), &d.path().join("area-scratch")).unwrap();
        let (ahi, alhi, almid) = ("layers/terrain/hi/6-21-18".to_string(), "layers/terrain/hi/6-16-17".to_string(), mid_logical(16, 17));
        for l in [&ahi, &alhi, &almid] {
            fake(&mut area, l);
        }
        let before = area.manifest.clone();
        build_q(&mut area, &raw, (2, 2), &ts, &cov, &src).unwrap();
        let changed: Vec<&String> = before.keys().chain(area.manifest.keys()).filter(|l| before.get(*l) != area.manifest.get(*l)).collect();
        assert!(changed.iter().any(|l| l.as_str() == alhi), "{changed:?}");
        for l in &changed {
            assert!(crate::agent::steps::saves("terrain", "3/2/2", l), "{l}: outside an area run's write-set");
        }
        assert!(area.get(&ahi).is_none() && area.get(&alhi).is_none() && area.get(&almid).is_some());
        assert!(area.get("layers/terrain/hi/6-22-18").is_some() && area.get("layers/terrain/lo/3-2-2").is_some());
    }

    #[test]
    fn the_north_from_glo30_and_a_lakes_one_level_across_tiles() {
        use crate::terrain_north::tests::FnCells;
        use crate::terrain_water::{tests::Fixed, Kind, Poly};
        // Two z11 tiles side by side at 70.5°N: AWS's 47 m above GLO-30 (ellipsoidal heights), a
        // lake across both (raised in AWS to 400 m), the sea in the first's top rows.
        const Z: u8 = 11;
        let n = (1u64 << Z) as f64;
        let y = ((1.0 - 70.5f64.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n) as u32;
        let x0 = ((10.0 + 180.0) / 360.0 * n) as u32;
        let cells = FnCells::new(|lat, lon| (200.0 + (lat - 70.0) * 100.0 + (lon - 10.0) * 50.0) as f32);
        fn polys(_z: u8, x: u32, _y: u32) -> Vec<Poly> {
            // (The lake from 200 to 300 in the pair's pixels across, rows 100 to 150; the sea's rows
            // 0 to 20 of the first.)
            let x0 = ((10.0 + 180.0) / 360.0 * 2048.0) as u32;
            let off = (x - x0) as f64 * 256.0;
            let mut v = vec![Poly { kind: Kind::Lake, id: 5, rings: vec![vec![[200.0 - off, 100.0], [300.0 - off, 100.0], [300.0 - off, 150.0], [200.0 - off, 150.0]]] }];
            if x == x0 {
                v.push(Poly { kind: Kind::Sea, id: 0, rings: vec![vec![[0.0, 0.0], [256.0, 0.0], [256.0, 20.0], [0.0, 20.0]]] });
            }
            v
        }
        let water = Fixed(polys);
        let src = Sources { north: Some(&cells), water: Some(&water), coarse: None };
        let raw = |x: u32| {
            let g = crate::terrain_north::north_tile(&cells, Z, x, y).unwrap().g;
            let mut e: Vec<f32> = g.iter().map(|v| v + 47.0).collect();
            for r in 100..150 {
                for c in 0..256 {
                    let wx = (x - x0) as usize * 256 + c;
                    if (200..300).contains(&wx) {
                        e[r * 256 + c] = 400.0;
                    }
                }
            }
            (encode_terrain_png(&e, 256, 256).unwrap(), g)
        };
        let none = HashMap::new();
        let made: Vec<(Prepared, Vec<f32>)> = [x0, x0 + 1].iter().map(|&x| {
            let (png, g) = raw(x);
            (prepare(png, Z, x, y, &none, &HashMap::new(), &src, &|e, z, lat, c| {
                repair_terrain_with(e, z, lat, c);
            }), g)
        }).collect();
        let mut lakes = HashMap::new();
        for (p, _) in &made {
            crate::terrain_water::gather(&mut lakes, p.lake_samples());
        }
        let mut levels = HashMap::new();
        crate::terrain_water::add_levels(&mut levels, &lakes);
        let level = levels[&5];
        let mut outs = Vec::new();
        for (p, g) in made {
            let (png, _, _) = finish(p, &levels);
            outs.push((decode_terrain_png(&png).unwrap(), g));
        }
        for (k, (e, g)) in outs.iter().enumerate() {
            for r in 0..256 {
                for c in 0..256 {
                    let (v, wx) = (e[r * 256 + c], k * 256 + c);
                    if (100..150).contains(&r) && (200..300).contains(&wx) {
                        assert_eq!(v, (level * 256.0).round() / 256.0, "the lake at {wx},{r}");
                    } else if k == 0 && r < 20 {
                        assert_eq!(v, 0.0, "the sea");
                    } else if !(98..152).contains(&r) && (r > 21 || k == 1) {
                        // GLO-30's, not AWS's 47 m above.
                        assert!((v - g[r * 256 + c]).abs() < 0.01, "{wx},{r}: {v} vs {}", g[r * 256 + c]);
                    }
                }
            }
        }
        // The lake's level: its shore's 10th percentile, GLO-30's land beside it, not AWS's 400 m.
        assert!(level < 300.0, "{level}");
    }
}
