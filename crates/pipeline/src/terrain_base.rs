//! The terrain's sampled tiles from the elevation pool (docs/steps.md §2, Terrain; docs/inputs.md
//! §6.8): a tile at its place's finest zoom (crate::terrain_zoom), and the relief outside every
//! region (`Pool::relief`). Every tile sampled from the same datasets, in the same order and blended at their edges
//! the same way, as the roads' elevations (crate::dem::generic), so the roads and the terrain use
//! one dataset at every point. Each pixel is the mean of 2 × 2 samples over it (`K`), each dataset
//! read at the level's detail: a GeoTIFF at its overview nearest half a pixel, a Terrarium pyramid at the level's own zoom (AWS's publishes every zoom),
//! another pyramid at its declared zoom, and only from z9 (`ROW_Z`) on: below, its next dataset serves
//! (a z8 tile would read 16,384 of GSI's z15 tiles over the network). The source's heights are kept
//! at sea and below 0 m too (the owner's, 5 Nov); where no dataset has data, sea level. Heights are
//! EGM2008 (each dataset's brought there as the roads' are). The tiles come out as Terrarium PNGs,
//! the terrain's levels made from them as from any raw tile (crate::terrain_pack: the coarser levels
//! from the finer).
//!
//! A Terrarium pyramid's tiles are read through `RawTiles` (this Mac's cache, the NAS's archives,
//! else AWS: crate::terrain_pack), not fetched by the sampler; a pyramid read tile by tile (GSI's)
//! from its store on the NAS first (crate::pyramids, `Pool::stored`); every other file as the roads read
//! them (the store, `sources/fetched/`, for a stored dataset, a file the job reads half or more of copied
//! whole to this Mac first where it has room: buildstore::localread; else the network).

use crate::dem::declared::{Plan, Source};
use crate::dem::generic::{tile_resolution, Copies, Opts, Sampler};
use crate::terrain_pack::{RawSource, RawTiles};
use anyhow::{Context, Result};
use buildstore::fetch::Fetch;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use store::range::RangeRead;

/// Samples a side over a pixel.
pub const K: usize = 2;
/// The finest zoom a plan is made for: the deepest the map stores.
const MAX_Z: u8 = crate::terrain_zoom::DEEPEST;
/// Tiles in flight at once while a level is sampled (`Tiles::prefetch_counted`): the memory
/// `sampling_mb` declares (in crate::terrain_pack::piece_peak_mb, and what terrain-hi 7's and
/// terrain-lo 2's runs measured, crate::coord::cost_version), shared among the chunks sampled at
/// once (`at_once`), each `chunk_tiles` of them.
const IN_FLIGHT: usize = 64;

/// Chunks sampled at once (task #896): the job's threads (its grant, `RAYON_NUM_THREADS`), a
/// chunk to each, so a thread waiting on a chunk's reads leaves the others' sampling going; at
/// most `IN_FLIGHT`, a tile a chunk.
fn at_once() -> usize {
    rayon::current_num_threads().clamp(1, IN_FLIGHT)
}

/// Tiles sampled in one call (each 256 × 256 × K² points) with `at_once` chunks at once: their
/// share of `IN_FLIGHT`, so the memory in flight stays `sampling_mb` whatever the threads.
fn chunk_tiles(at_once: usize) -> usize {
    IN_FLIGHT / at_once.max(1)
}

/// The memory (MB) a level's chunks in flight take at most (`Tiles::prefetch_counted`): `IN_FLIGHT`
/// tiles' K × K points a pixel, each point's longitude and latitude
/// (`points`), what the sampler gives it (crate::dem::generic::Sampled: its height, dataset and
/// blend) and works it out with (its height, first dataset and file).
pub const fn sampling_mb() -> u64 {
    use std::mem::size_of;
    let point = 2 * size_of::<f64>()
        + 2 * size_of::<f32>()
        + size_of::<u8>()
        + size_of::<bool>()
        + size_of::<u16>()
        + size_of::<u32>();
    (IN_FLIGHT * 256 * 256 * K * K * point / (1 << 20)) as u64
}

/// The Terrarium pyramid's URL template in the accepted elevation version manifest `m` names
/// under `root` (the last declared: the base's raw tiles are its, through `RawTiles`).
pub fn terrarium_template(
    root: &store::nasfs::NasRoot,
    m: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    use crate::dem::declared::Data;
    let a = crate::inputs::open(root, m, crate::inputs::elevation::UNIT)?
        .context("no version of the elevation datasets is accepted (docs/inputs.md §6.8)")?;
    crate::dem::declared::accepted(&a)?
        .iter()
        .rev()
        .find_map(|d| match &d.declaration.data {
            Data::Tiles { url, encoding, .. } if encoding == "terrarium" => Some(url.clone()),
            _ => None,
        })
        .context("no tile pyramid in the Terrarium encoding is declared (the terrain's raw tiles: docs/inputs.md §6.8)")
}

/// The box (west, south, east, north) of tiles `ts` of zoom `z`.
pub fn tiles_box(z: u8, ts: &[(u32, u32)]) -> [f64; 4] {
    let mut b = [180.0f64, 90.0f64, -180.0f64, -90.0f64];
    for &(x, y) in ts {
        let t = roadcore::geo::tile_box(z, x, y);
        b = [b[0].min(t[0]), b[1].min(t[1]), b[2].max(t[2]), b[3].max(t[3])];
    }
    b
}

/// The pool over an area, as the base reads it.
pub struct Pool {
    /// The datasets meeting the area (crate::dem::declared::plan_converted), in their order.
    pub plan: Plan,
    /// The Terrarium pyramid's URL template, where one is declared (read through `RawTiles`).
    pub terrarium: Option<String>,
    /// The stores' folder (`sources/fetched/`).
    pub store: store::nasfs::NasPath,
    /// The area (west, south, east, north): its middle latitude sets the detail each level reads.
    pub area: [f64; 4],
    /// This Mac's whole copies of the stores' files the area's jobs read much of (buildstore::localread:
    /// crate::dem::generic::COPY_SHARE); None, the NAS's read in place.
    pub local: Option<buildstore::localread::Local>,
    /// The stored pyramids' folder (crate::pyramids::DIR under the root): the pyramids read tile by
    /// tile (`by_tile`) read there before the network. None: fetched.
    pub pyramids: Option<store::nasfs::NasPath>,
}

impl Pool {
    /// The pool over box `b` (west, south, east, north) of the accepted elevation and sources
    /// versions manifest `m` names under `root`.
    pub fn read(
        root: &store::nasfs::NasRoot,
        m: &std::collections::BTreeMap<String, String>,
        b: [f64; 4],
    ) -> Result<Pool> {
        let a = crate::inputs::open(root, m, crate::inputs::elevation::UNIT)?.context(
            "no version of the elevation datasets is accepted: the terrain's are declared there (docs/inputs.md §6.8)",
        )?;
        let geoid = crate::inputs::sources::geoid(root, m)?;
        let mut pool = Pool::of(crate::dem::declared::plan_converted(&a, b, &geoid)?, root.join("sources/fetched"), b);
        pool.local = buildstore::localread::Local::from_env(root);
        pool.pyramids = Some(root.join(crate::pyramids::DIR));
        Ok(pool)
    }

    /// The pool of plan `plan` over area `area`, its stores in `store` (read in place).
    pub fn of(plan: Plan, store: store::nasfs::NasPath, area: [f64; 4]) -> Pool {
        let terrarium = plan.datasets.iter().rev().find_map(|d| match &d.source {
            Source::Tiles { url, encoding, .. } if encoding == "terrarium" => Some(url.clone()),
            _ => None,
        });
        Pool { plan, terrarium, store, area, local: None, pyramids: None }
    }

    /// The pool of the datasets published at every zoom alone (their declarations' `every_zoom`:
    /// crate::terrain_zoom::Set::every_zoom), each read at the tile's own zoom: the coarse relief outside every
    /// region (crate::terrain_pack::Fill::Relief), one of its tiles a tile, never a GeoTIFF's area.
    pub fn relief(&self) -> Pool {
        let mut plan = self.plan.clone();
        plan.datasets.retain(|d| matches!(&d.source, Source::Tiles { every_zoom: true, .. }));
        Pool {
            plan,
            terrarium: self.terrarium.clone(),
            store: self.store.clone(),
            area: self.area,
            local: None,
            pyramids: None,
        }
    }

    /// The plan read at zoom `z` over latitude `lat` (`Pool`'s rule). Below `ROW_Z` (the z3 tiles'
    /// levels, terrain-lo's) a pyramid published at one zoom only (any but Terrarium) is left out, the
    /// next dataset serving there: its tiles a z8 tile would read are 4^(zoom − 8) (GSI's z15: 16,384,
    /// a request each, task #704).
    fn at_zoom(&self, z: u8, lat: f64) -> Plan {
        let half = tile_resolution(z as u32, lat) / 2.0;
        let mut p = self.plan.clone();
        if z < ROW_Z {
            p.datasets.retain(|d| !by_tile(d));
        }
        for d in &mut p.datasets {
            match &mut d.source {
                Source::Geotiff { read_resolution, .. } => {
                    *read_resolution = Some(half);
                    d.resolution = d.resolution.max(half);
                }
                Source::Tiles { zoom, every_zoom: true, .. } if *zoom > z as u32 => {
                    *zoom = z as u32;
                    d.resolution = d.resolution.max(tile_resolution(z as u32, lat));
                }
                Source::Tiles { .. } => {}
            }
        }
        p
    }

    /// `inner` with the stored pyramids read first (crate::pyramids::Stored), where the pool has their
    /// folder.
    pub fn stored<'a>(&self, inner: &'a dyn Fetch) -> Option<crate::pyramids::Stored<'a>> {
        self.pyramids.as_ref().map(|d| crate::pyramids::Stored::new(inner, d, &self.plan))
    }

    /// `f` given the pool's tiles (`Tiles`), the Terrarium pyramid's read through `raw`.
    pub fn with_tiles<R>(&self, raw: &RawTiles, f: impl FnOnce(&Tiles) -> R) -> Result<R> {
        let net = buildstore::fetch::Fetcher::from_env();
        let stored = self.stored(&net);
        let fetch = RawFetch::new(raw, self.terrarium.as_deref(), stored.as_ref().map_or(&net as &dyn Fetch, |s| s));
        let o = Opts {
            fetch: &fetch,
            store: Some(self.store.clone()),
            local: self.local.as_ref().map(|local| Copies { local, area: self.area }),
        };
        let mid = (self.area[1] + self.area[3]) / 2.0;
        let plans: Vec<Plan> = (0..=MAX_Z).map(|z| self.at_zoom(z, mid)).collect();
        let by_row: Vec<OnceLock<Plan>> = (0..BY_ROW).map(|_| OnceLock::new()).collect();
        let row_plan = |z: u8, row: u32| self.at_zoom(z, row_mid(row));
        let t = Tiles::new(&plans, &o).by_row(&by_row, &row_plan);
        Ok(f(&t))
    }
}

/// Whether dataset `d` is a tile pyramid read tile by tile over the network at its one zoom: one
/// not declared published at every zoom (`every_zoom`). GSI's z15 layers: 64 requests a z12 tile
/// each.
pub(crate) fn by_tile(d: &crate::dem::declared::Dataset) -> bool {
    matches!(&d.source, Source::Tiles { every_zoom: false, .. })
}

/// The first zoom whose tiles read the pool at their own z6 tile's middle latitude (`row_mid`), not
/// the area's: the z6 tiles' levels (z9–12, crate::terrain_pack::piece_levels), so a base tile's
/// bytes are its own whatever else a job makes (a z6 tile's run, its neighbours', a deep target's
/// base: crate::terrain_pack::base_of).
pub const ROW_Z: u8 = 9;
/// The plans read by z6 row: one for each zoom from `ROW_Z` to `MAX_Z` and each of the 64 z6 rows.
const BY_ROW: usize = (MAX_Z - ROW_Z + 1) as usize * 64;

/// The middle latitude of z6 row `row` (its box's south and north edges averaged, as a z6 tile's
/// own area's: `Pool::with_tiles`).
pub fn row_mid(row: u32) -> f64 {
    let b = roadcore::geo::tile_box(6, 0, row);
    (b[1] + b[3]) / 2.0
}

/// The fetch the sampler reads through: a Terrarium pyramid's tiles from `raw`, the rest `inner`.
struct RawFetch<'a> {
    raw: &'a RawTiles,
    url: Option<&'a str>,
    inner: &'a dyn Fetch,
    /// The raw tiles read at once, and the most: the threads the job runs on, as when the
    /// sampler's threads fetched its tiles themselves (task #665 review: its pyramids' tiles are
    /// fetched `TILE_FETCHES` at once, and these are read from the NAS's archives and loose tiles,
    /// whose load stays as it was).
    reading: (Mutex<usize>, std::sync::Condvar),
    most: usize,
}

impl<'a> RawFetch<'a> {
    fn new(raw: &'a RawTiles, url: Option<&'a str>, inner: &'a dyn Fetch) -> RawFetch<'a> {
        RawFetch { raw, url, inner, reading: Default::default(), most: rayon::current_num_threads().max(1) }
    }

    /// Raw tile z/x/y, read once fewer than `most` are.
    fn raw_get(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        let (m, cv) = &self.reading;
        {
            let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
            while *n >= self.most {
                n = cv.wait(n).unwrap_or_else(|e| e.into_inner());
            }
            *n += 1;
        }
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.raw.get(z, x, y)));
        *m.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        cv.notify_one();
        match got {
            Ok(r) => Ok(r?.0),
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    /// Tile z/x/y of the Terrarium template, when `url` is one of its tiles.
    fn tile(&self, url: &str) -> Option<(u8, u32, u32)> {
        let t = self.url?;
        let (pre, rest) = t.split_once("{z}")?;
        let suf = rest.strip_prefix("/{x}/{y}")?;
        let mid = url.strip_prefix(pre)?.strip_suffix(suf)?;
        let mut it = mid.split('/').map(|v| v.parse::<u32>().ok());
        let (z, x, y) = (it.next()??, it.next()??, it.next()??);
        (it.next().is_none() && z <= 30).then_some((z as u8, x, y))
    }
}

impl Fetch for RawFetch<'_> {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
        match self.tile(url) {
            Some((z, x, y)) => Ok(self.raw_get(z, x, y)?.map(|b| Arc::new(b) as Arc<dyn RangeRead>)),
            None => self.inner.open(url),
        }
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.tile(url) {
            Some((z, x, y)) => self.raw_get(z, x, y),
            None => self.inner.get(url),
        }
    }

    // (The other pyramids' tiles held to the limits their reader asks, crate::dem::generic::TILE_WITHIN:
    // a silent connection given up in 30 s, not the client's 90 s a try.)
    fn get_within(&self, url: &str, within: buildstore::fetch::Within) -> Result<Option<Vec<u8>>> {
        match self.tile(url) {
            Some((z, x, y)) => self.raw_get(z, x, y),
            None => self.inner.get_within(url, within),
        }
    }
}

/// The pool's base tiles, as raw tiles (crate::terrain_pack::RawSource): each sampled once, those
/// of a level all at once (`prefetch_counted`), held until the level takes them.
pub struct Tiles<'a> {
    plans: &'a [Plan],
    /// The plans of the levels from `ROW_Z` on, by zoom and z6 row (`row_mid`), made as first read
    /// (`row_plan`); none: the area's (tests).
    by_row: &'a [OnceLock<Plan>],
    row_plan: Option<&'a (dyn Fn(u8, u32) -> Plan + Sync)>,
    o: &'a Opts<'a>,
    samplers: Vec<OnceLock<Result<Sampler<'a>, String>>>,
    held: Mutex<HashMap<(u8, u32, u32), Option<Vec<u8>>>>,
    /// By tile sampled, the datasets its pixels came from (their ids, sorted:
    /// crate::terrain_pack::RawSource::from), until asked for.
    from: Mutex<HashMap<(u8, u32, u32), Vec<String>>>,
    /// Whether the build is pausing (jobrt::control::draining_lately; a test's own): checked before each
    /// sampling, a chunk or a tile, which then stops with jobrt::control::Paused.
    draining: &'a (dyn Fn() -> bool + Sync),
}

impl<'a> Tiles<'a> {
    fn new(plans: &'a [Plan], o: &'a Opts<'a>) -> Tiles<'a> {
        Tiles {
            plans,
            by_row: &[],
            row_plan: None,
            o,
            samplers: plans.iter().map(|_| OnceLock::new()).collect(),
            held: Mutex::new(HashMap::new()),
            from: Mutex::new(HashMap::new()),
            draining: &jobrt::control::draining_lately,
        }
    }

    /// With the levels from `ROW_Z` on read by z6 row (`by_row`, `BY_ROW` slots, made by `row_plan`).
    fn by_row(mut self, by_row: &'a [OnceLock<Plan>], row_plan: &'a (dyn Fn(u8, u32) -> Plan + Sync)) -> Tiles<'a> {
        self.samplers.extend(by_row.iter().map(|_| OnceLock::new()));
        (self.by_row, self.row_plan) = (by_row, Some(row_plan));
        self
    }

    /// The slot of tile `z/x/y`'s plan and sampler: its zoom's, or from `ROW_Z` on its zoom's and
    /// z6 row's.
    fn slot(&self, z: u8, y: u32) -> usize {
        match self.row_plan {
            Some(_) if z >= ROW_Z && z <= MAX_Z => {
                self.plans.len() + (z - ROW_Z) as usize * 64 + (y >> (z - 6)) as usize
            }
            _ => (z as usize).min(self.plans.len() - 1),
        }
    }

    /// The plan of slot `k` (`slot`).
    fn plan(&self, k: usize) -> &'a Plan {
        match k.checked_sub(self.plans.len()) {
            None => &self.plans[k],
            Some(r) => {
                let (z, row) = (ROW_Z + (r / 64) as u8, (r % 64) as u32);
                let make = self.row_plan.expect("slots past the area's are by row");
                self.by_row[r].get_or_init(|| make(z, row))
            }
        }
    }

    fn sampler(&self, k: usize) -> Result<&Sampler<'a>> {
        self.samplers[k]
            .get_or_init(|| {
                // (Every level's sampler shares one cache of decoded blocks and fetched tiles.)
                let other = self.samplers.iter().find_map(|s| s.get().and_then(|r| r.as_ref().ok()));
                let plan = self.plan(k);
                match other {
                    Some(o) => Sampler::sharing(plan, self.o, o),
                    None => Sampler::new(plan, self.o),
                }
                .map_err(|e| format!("{e:#}"))
            })
            .as_ref()
            .map_err(|e| anyhow::anyhow!("the terrain's datasets (plan {k}): {e}"))
    }

    /// Tiles `ts` of zoom `z` sampled: each its Terrarium PNG, None where no dataset has data.
    pub fn sample(&self, z: u8, ts: &[(u32, u32)]) -> Result<Vec<Option<Vec<u8>>>> {
        if ts.is_empty() {
            return Ok(Vec::new());
        }
        // (Between chunks: a level's sampling runs for hours, and a pause waits for no more than
        // the chunks under way, task #705; counted in the job's measure, task #889.)
        jobrt::control::reached();
        jobrt::control::pause_point(self.draining)?;
        self.sample_all(z, ts)
    }

    /// Tiles `ts` of zoom `z` sampled, every one: by their plans (`slot`), in their order.
    fn sample_all(&self, z: u8, ts: &[(u32, u32)]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut by: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
        for (i, t) in ts.iter().enumerate() {
            by.entry(self.slot(z, t.1)).or_default().push(i);
        }
        let mut out: Vec<Option<Option<Vec<u8>>>> = vec![None; ts.len()];
        for (k, is) in by {
            let mine: Vec<(u32, u32)> = is.iter().map(|&i| ts[i]).collect();
            for (i, t) in is.into_iter().zip(self.sample_with(k, z, &mine)?) {
                out[i] = Some(t);
            }
        }
        Ok(out.into_iter().map(|t| t.expect("each tile sampled")).collect())
    }

    /// Tiles `ts` of zoom `z` sampled with slot `k`'s plan.
    fn sample_with(&self, k: usize, z: u8, ts: &[(u32, u32)]) -> Result<Vec<Option<Vec<u8>>>> {
        let s = self.sampler(k)?;
        let (lon, lat) = points(z, ts);
        // (Each point's height and its first dataset's id.)
        let got = s.sample(&lon, &lat, &|_| {})?;
        let p = self.plan(k);
        let (elev, ids): (Vec<f32>, Vec<Option<&str>>) =
            (got.elev, got.first.into_iter().map(|f| p.datasets.get(f as usize).map(|d| d.id.as_str())).collect());
        // (By tile, the datasets its pixels came from: `from`.)
        {
            let per = 256 * 256 * K * K;
            let mut c = self.from.lock().unwrap();
            for (t, &(x, y)) in ts.iter().enumerate() {
                let set: std::collections::BTreeSet<&str> =
                    ids[t * per..(t + 1) * per].iter().flatten().copied().collect();
                c.insert((z, x, y), set.into_iter().map(str::to_string).collect());
            }
        }
        let per = 256 * 256;
        let mut out = Vec::with_capacity(ts.len());
        for t in 0..ts.len() {
            let mut e = vec![0f32; per];
            let mut any = false;
            for (p, v) in e.iter_mut().enumerate() {
                let (j, i) = (p / 256, p % 256);
                let (mut sum, mut n) = (0f32, 0u32);
                for b in 0..K {
                    for a in 0..K {
                        let q = t * per * K * K + (j * K + b) * 256 * K + i * K + a;
                        let h = elev[q];
                        if h.is_finite() {
                            sum += h;
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    *v = sum / n as f32;
                    any = true;
                }
            }
            out.push(if any { Some(roadcore::grid::encode_terrain_png(&e, 256, 256)?) } else { None });
        }
        Ok(out)
    }
}

/// The sample points of tiles `ts` of zoom `z`: K × K a pixel, by tile, row (of samples) and
/// column.
pub fn points(z: u8, ts: &[(u32, u32)]) -> (Vec<f64>, Vec<f64>) {
    let n = (1u64 << z) as f64;
    let side = 256 * K;
    let (mut lon, mut lat) = (Vec::with_capacity(ts.len() * side * side), Vec::with_capacity(ts.len() * side * side));
    for &(x, y) in ts {
        for r in 0..side {
            let fy = (y as f64 + (r as f64 + 0.5) / side as f64) / n;
            let la = roadcore::geo::merc_lat(fy);
            for c in 0..side {
                lon.push((x as f64 + (c as f64 + 0.5) / side as f64) / n * 360.0 - 180.0);
                lat.push(la);
            }
        }
    }
    (lon, lat)
}

impl RawSource for Tiles<'_> {
    fn from(&self, z: u8, x: u32, y: u32) -> Option<Vec<String>> {
        Some(self.from.lock().unwrap().remove(&(z, x, y)).unwrap_or_default())
    }

    fn get(&self, z: u8, x: u32, y: u32) -> Result<(Option<Vec<u8>>, bool)> {
        if let Some(t) = self.held.lock().unwrap().remove(&(z, x, y)) {
            return Ok((t, false));
        }
        Ok((self.sample(z, &[(x, y)])?.pop().flatten(), false))
    }

    // (Its chunks `at_once` at a time, the first to fail stopping the rest, as one that finds the
    // build pausing does (jobrt::control::Paused, from `sample`). A chunk's reads mostly wait on the NAS, one at a time left a
    // job's threads idle, task #695. The same bytes: a tile's sampling reads nothing of another's.)
    fn prefetch_counted(&self, z: u8, tiles: &[(u32, u32)], _threads: usize, done: &AtomicU64) -> Result<usize> {
        // (As many at once as the job's threads, each a task of a pool as wide as the target's
        // grant (jobrt::control::on_outside_pool), its sampler's passes there too
        // (crate::dem::generic's, through jobrt::control::on_target_pool): the target's own thread
        // waits, working on nothing, so the job is on its grant's threads, no more. Never threads of
        // their own whose passes go on a pool: the target's own one's thread is waiting for them
        // (task #961), and another's beside them is twice the grant. One task a thread of the pool
        // (rayon's broadcast), never two on one: a thread waiting in one chunk's passes takes up no
        // other chunk, so no more than `at_once` are in flight.)
        let at_once = at_once();
        let chunks: Vec<&[(u32, u32)]> = tiles.chunks(chunk_tiles(at_once)).collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let failed = std::sync::atomic::AtomicBool::new(false);
        let run = || -> Result<()> {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(c) = chunks.get(i) else { return Ok(()) };
                if failed.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let got = self.sample(z, c).inspect_err(|_| failed.store(true, Ordering::Relaxed))?;
                let mut h = self.held.lock().unwrap();
                for (&(x, y), t) in c.iter().zip(got) {
                    h.insert((z, x, y), t);
                }
                done.fetch_add(c.len() as u64, Ordering::Relaxed);
            }
        };
        let k = at_once.min(chunks.len());
        jobrt::control::on_outside_pool(|| {
            rayon::broadcast(|b| if b.index() < k { run() } else { Ok(()) }).into_iter().collect::<Result<()>>()
        })?;
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dem::declared::Dataset;

    /// A pool of two: a GLO-30-like 1° tile in the store (its west half filled from another DEM, by
    /// its mask), then a Terrarium pyramid in the cache; a z12 tile across the mask's edge.
    #[test]
    fn a_base_tile_is_the_pools_order_and_its_voids_the_next_datasets() {
        let d = tempfile::tempdir().unwrap();
        let (store, local) = (d.path().join("fetched"), d.path().join("local"));
        std::fs::create_dir_all(store.join("glo")).unwrap();
        // (1/1000° pixels over 10–11°E, 45–46°N: 100 m plus 1 m a pixel east; the mask 3 in its
        // west half.)
        let n = 1000usize;
        let gt = [10.0, 0.001, 0.0, 46.0, 0.0, -0.001];
        let e: Vec<f32> = (0..n * n).map(|p| 100.0 + (p % n) as f32).collect();
        let m: Vec<f32> = (0..n * n).map(|p| if p % n < 500 { 3.0 } else { 1.0 }).collect();
        let tif = |v: &[f32]| buildstore::geotiff::write_f32(n as u32, n as u32, v, 256, gt, 4326, None).unwrap();
        std::fs::write(store.join("glo/T_DEM.tif"), tif(&e)).unwrap();
        std::fs::write(store.join("glo/T_FLM.tif"), tif(&m)).unwrap();
        // The z12 tile over 10.5°E, 45.5°N, and its Terrarium tile: 500 m flat.
        let z = 12u8;
        let nn = (1u64 << z) as f64;
        let x = (roadcore::geo::merc_x(10.5) * nn) as u32;
        let y = (roadcore::geo::merc_y(45.5) * nn) as u32;
        std::fs::create_dir_all(local.join(format!("{z}/{x}"))).unwrap();
        std::fs::write(
            local.join(format!("{z}/{x}/{y}.png")),
            roadcore::grid::encode_terrain_png(&vec![500.0; 65536], 256, 256).unwrap(),
        )
        .unwrap();
        let plan = Plan {
            fmt: 1,
            version: "test".into(),
            tag: "t".into(),
            datasets: vec![
                Dataset {
                    id: "glo".into(),
                    code: 3,
                    resolution: 78.0,
                    cover: Some(vec![[10.0, 45.0, 11.0, 46.0]]),
                    source: Source::Geotiff {
                        crs: 4326,
                        read_resolution: None,
                        store: true,
                        files: vec![("https://e.org/T/T_DEM.tif".into(), Some([10.0, 45.0, 11.0, 46.0]))],
                        masks: vec![Some("https://e.org/T/T_FLM.tif".into())],
                    },
                    convert: None,
                    outline: None,
                },
                Dataset {
                    id: "terrarium".into(),
                    code: 4,
                    resolution: tile_resolution(12, 45.5),
                    cover: None,
                    source: Source::Tiles {
                        url: crate::terrain_pack::TEST_BASE.into(),
                        zoom: 12,
                        encoding: "terrarium".into(),
                        every_zoom: true,
                    },
                    convert: None,
                    outline: None,
                },
            ],
        };
        let pool = Pool::of(plan, store::nasfs::NasPath::local_for_test(store), [10.0, 45.0, 11.0, 46.0]);
        assert_eq!(pool.terrarium.as_deref(), Some(crate::terrain_pack::TEST_BASE));
        let raw = RawTiles::new(&local, crate::terrain_pack::TEST_BASE);
        buildstore::fetch::go_offline();
        let (png, again) = pool
            .with_tiles(&raw, |t| {
                let done = AtomicU64::new(0);
                t.prefetch_counted(z, &[(x, y)], 1, &done).unwrap();
                assert_eq!(done.load(Ordering::Relaxed), 1);
                (t.get(z, x, y).unwrap().0.unwrap(), t.get(z, x, y).unwrap().0.unwrap())
            })
            .unwrap();
        assert_eq!(png, again, "held, then sampled again: the same");
        let h = roadcore::grid::decode_terrain(&png).unwrap();
        // Each pixel's longitude: east of the mask's edge (10.5°E) past the blend (four of its 78 m
        // pixels), GLO-30's field;
        // west of it, the Terrarium tiles' 500 m.
        let lon = |i: usize| (x as f64 + (i as f64 + 0.5) / 256.0) / nn * 360.0 - 180.0;
        let (mut east, mut west) = (0, 0);
        for i in 0..256 {
            let v = h[128 * 256 + i];
            let l = lon(i);
            if l > 10.506 {
                let want = 100.0 + ((l - 10.0) / 0.001 - 0.5) as f32;
                assert!((v - want).abs() < 1.5, "{l}: {v} for {want}");
                east += 1;
            } else if l < 10.4995 {
                assert!((v - 500.0).abs() < 0.01, "{l}: {v}");
                west += 1;
            }
        }
        assert!(east > 10 && west > 10, "{east} {west}");
    }

    /// A z9–12 tile reads the pool at its own z6 row's middle latitude (`row_mid`), whatever area the
    /// job's pool covers: the same bytes from a pool of its z6 tile alone and one of many rows (a job
    /// of several z6 tiles, a deep target's base). Read at the area's middle, a dataset finer than
    /// half its pixel blends over another band (its resolution: `at_zoom`), other bytes.
    #[test]
    fn a_hi_tile_reads_the_pool_at_its_z6_rows_latitude() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("fetched");
        std::fs::create_dir_all(store.join("fine")).unwrap();
        // (A 1/10000° raster over the tile, 100 m plus a slope; its mask 3 in its west half (filled
        // from the next dataset, as fine, 500 m flat), so the two blend across the mask's edge, over
        // four of their pixels as read at the level's detail: `at_zoom`.)
        let (z, lon, lat) = (12u8, 10.5f64, 45.5f64);
        let nn = (1u64 << z) as f64;
        let (x, y) = roadcore::geo::merc(lon, lat);
        let (x, y) = ((x * nn) as u32, (y * nn) as u32);
        let n = 1200usize;
        let gt = [10.44, 0.0001, 0.0, 45.56, 0.0, -0.0001];
        let e: Vec<f32> = (0..n * n).map(|p| 100.0 + (p % n) as f32 * 0.01).collect();
        let m: Vec<f32> = (0..n * n).map(|p| if p % n < 600 { 3.0 } else { 1.0 }).collect();
        let tif = |v: &[f32]| buildstore::geotiff::write_f32(n as u32, n as u32, v, 256, gt, 4326, None).unwrap();
        std::fs::write(store.join("fine/F_DEM.tif"), tif(&e)).unwrap();
        std::fs::write(store.join("fine/F_FLM.tif"), tif(&m)).unwrap();
        std::fs::create_dir_all(store.join("flat")).unwrap();
        std::fs::write(store.join("flat/G.tif"), tif(&vec![500.0; n * n])).unwrap();
        let plan = Plan {
            fmt: 1,
            version: "test".into(),
            tag: "t".into(),
            datasets: vec![
                Dataset {
                    id: "fine".into(),
                    code: 1,
                    resolution: 7.8,
                    cover: Some(vec![[10.44, 45.44, 10.56, 45.56]]),
                    source: Source::Geotiff {
                        crs: 4326,
                        read_resolution: None,
                        store: true,
                        files: vec![("https://e.org/F/F_DEM.tif".into(), Some([10.44, 45.44, 10.56, 45.56]))],
                        masks: vec![Some("https://e.org/F/F_FLM.tif".into())],
                    },
                    convert: None,
                    outline: None,
                },
                Dataset {
                    id: "flat".into(),
                    code: 2,
                    resolution: 7.8,
                    cover: None,
                    source: Source::Geotiff {
                        crs: 4326,
                        read_resolution: None,
                        store: true,
                        files: vec![("https://e.org/G/G.tif".into(), Some([10.44, 45.44, 10.56, 45.56]))],
                        masks: vec![None],
                    },
                    convert: None,
                    outline: None,
                },
            ],
        };
        let raw = RawTiles::new(&d.path().join("local"), crate::terrain_pack::TEST_BASE);
        buildstore::fetch::go_offline();
        let tile_of = |area: [f64; 4]| {
            Pool::of(plan.clone(), store::nasfs::NasPath::local_for_test(store.clone()), area)
                .with_tiles(&raw, |t| t.sample(z, &[(x, y)]).unwrap().pop().unwrap().unwrap())
                .unwrap()
        };
        let own = tile_of(roadcore::geo::tile_box(6, x >> 6, y >> 6));
        let wide = tile_of([10.0, 20.0, 11.0, 60.0]);
        assert_eq!(own, wide, "its own z6 row's latitude, whatever the area");
        // (Read at the wide area's middle, 40°N: another blend.)
        let pool =
            Pool::of(plan.clone(), store::nasfs::NasPath::local_for_test(store.clone()), [10.0, 20.0, 11.0, 60.0]);
        let net = buildstore::fetch::MapFetch::default();
        let fetch = RawFetch::new(&raw, pool.terrarium.as_deref(), &net);
        let o = Opts { fetch: &fetch, store: Some(store::nasfs::NasPath::local_for_test(store.clone())), local: None };
        let plans: Vec<Plan> = (0..=MAX_Z).map(|z| pool.at_zoom(z, 40.0)).collect();
        let at_area = Tiles::new(&plans, &o).sample(z, &[(x, y)]).unwrap().pop().unwrap().unwrap();
        assert_ne!(at_area, own, "the area's middle blends otherwise");
        assert!((45.0..49.0).contains(&row_mid(y >> 6)), "6/x/22: 45.1°N to 49.0°N");
    }

    #[test]
    fn the_raw_tiles_are_told_by_the_template() {
        let raw = RawTiles::new(std::path::Path::new("/nonexistent"), crate::terrain_pack::TEST_BASE);
        let net = buildstore::fetch::MapFetch::default();
        let f = RawFetch::new(&raw, Some("https://t.org/a/{z}/{x}/{y}.png"), &net);
        assert_eq!(f.tile("https://t.org/a/12/3/4.png"), Some((12, 3, 4)));
        assert_eq!(f.tile("https://t.org/a/12/3.png"), None);
        assert_eq!(f.tile("https://t.org/b/12/3/4.png"), None);
        assert_eq!(f.tile("https://t.org/a/12/3/4.png?x"), None);
    }

    #[test]
    fn a_level_reads_its_detail() {
        let ds = |id: &str, resolution: f64, source: Source| Dataset {
            id: id.into(),
            code: 1,
            resolution,
            cover: None,
            source,
            convert: None,
            outline: None,
        };
        let plan = Plan {
            fmt: 1,
            version: "t".into(),
            tag: "t".into(),
            datasets: vec![
                ds(
                    "lidar",
                    8.0,
                    Source::Geotiff { crs: 4326, read_resolution: None, store: false, files: vec![], masks: vec![] },
                ),
                ds(
                    "t",
                    38.0,
                    Source::Tiles { url: "u".into(), zoom: 12, encoding: "terrarium".into(), every_zoom: true },
                ),
                ds(
                    "g",
                    5.0,
                    Source::Tiles { url: "v".into(), zoom: 15, encoding: "gsi-png".into(), every_zoom: false },
                ),
            ],
        };
        let pool = Pool::of(plan, store::nasfs::NasPath::local_for_test("/s"), [0.0, -1.0, 1.0, 1.0]);
        let p12 = pool.at_zoom(12, 0.0);
        let half = tile_resolution(12, 0.0) / 2.0;
        assert!(
            matches!(&p12.datasets[0].source, Source::Geotiff { read_resolution: Some(r), .. } if (*r - half).abs() < 1e-9)
        );
        assert!(matches!(&p12.datasets[1].source, Source::Tiles { zoom: 12, .. }));
        let p5 = pool.at_zoom(5, 0.0);
        assert!(
            matches!(&p5.datasets[1].source, Source::Tiles { zoom: 5, .. }),
            "a Terrarium pyramid at the level's zoom"
        );
        assert_eq!(p5.datasets.len(), 2, "another pyramid left out below ROW_Z");
        assert!(p5.datasets[0].resolution > 1000.0);
        let p9 = pool.at_zoom(ROW_Z, 0.0);
        assert!(matches!(&p9.datasets[2].source, Source::Tiles { zoom: 15, .. }), "from ROW_Z at its own zoom");
    }

    /// A drain asked while a level's chunks are sampled (task #705): the chunk under way finishes
    /// whole and no other begins (which chunk is up to the threads), the sampling ending with jobrt::control::Paused; what it sampled is
    /// what a run never paused samples.
    #[test]
    fn a_drain_mid_sampling_stops_within_a_chunk() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("fetched");
        std::fs::create_dir_all(store.join("glo")).unwrap();
        let n = 1000usize;
        let gt = [10.0, 0.001, 0.0, 46.0, 0.0, -0.001];
        let e: Vec<f32> = (0..n * n).map(|p| 100.0 + (p % n) as f32 * 0.5 + (p / n) as f32 * 0.25).collect();
        let tif = buildstore::geotiff::write_f32(n as u32, n as u32, &e, 256, gt, 4326, None).unwrap();
        std::fs::write(store.join("glo/T_DEM.tif"), tif).unwrap();
        let plan = Plan {
            fmt: 1,
            version: "test".into(),
            tag: "t".into(),
            datasets: vec![Dataset {
                id: "glo".into(),
                code: 3,
                resolution: 78.0,
                cover: Some(vec![[10.0, 45.0, 11.0, 46.0]]),
                source: Source::Geotiff {
                    crs: 4326,
                    read_resolution: None,
                    store: true,
                    files: vec![("https://e.org/T/T_DEM.tif".into(), Some([10.0, 45.0, 11.0, 46.0]))],
                    masks: vec![None],
                },
                convert: None,
                outline: None,
            }],
        };
        let pool = Pool::of(plan, store::nasfs::NasPath::local_for_test(store), [10.0, 45.0, 11.0, 46.0]);
        let raw = RawTiles::new(&d.path().join("local"), crate::terrain_pack::TEST_BASE);
        buildstore::fetch::go_offline();
        let net = buildstore::fetch::Fetcher::from_env();
        let fetch = RawFetch::new(&raw, None, &net);
        let o = Opts { fetch: &fetch, store: Some(pool.store.clone()), local: None };
        let plans: Vec<Plan> = (0..=MAX_Z).map(|z| pool.at_zoom(z, 45.5)).collect();
        // (Three chunks' tiles of z12 over the raster.)
        let z = 12u8;
        let nn = (1u64 << z) as f64;
        let x0 = (roadcore::geo::merc_x(10.5) * nn) as u32;
        let y0 = (roadcore::geo::merc_y(45.5) * nn) as u32;
        let ts: Vec<(u32, u32)> = (x0 - 4..x0 + 4).flat_map(|x| (y0 - 2..y0 + 3).map(move |y| (x, y))).collect();
        let chunk = chunk_tiles(at_once());
        assert!(ts.len() > 2 * chunk);
        // The drain asked as the first sampling begins: every later one finds it.
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let draining = || asked.fetch_add(1, Ordering::Relaxed) >= 1;
        let paused = Tiles { draining: &draining, ..Tiles::new(&plans, &o) };
        let done = AtomicU64::new(0);
        let e = paused.prefetch_counted(z, &ts, 1, &done).unwrap_err();
        assert!(jobrt::control::is_paused(&e), "{e:#}");
        // Exactly one chunk sampled, whole: whichever worker asked first (under load the last,
        // shorter chunk's can), so its size is any chunk's, never part of one or two.
        let held: HashMap<(u8, u32, u32), Option<Vec<u8>>> = paused.held.lock().unwrap().clone();
        let one = ts.chunks(chunk).find(|c| held.contains_key(&(z, c[0].0, c[0].1))).expect("a chunk sampled");
        let mut keys: Vec<_> = held.keys().copied().collect();
        keys.sort();
        let mut want: Vec<_> = one.iter().map(|&(x, y)| (z, x, y)).collect();
        want.sort();
        assert_eq!(keys, want, "the one chunk under way, whole, no other");
        assert_eq!(done.load(Ordering::Relaxed), one.len() as u64);
        // Never paused: the same bytes.
        let whole = Tiles::new(&plans, &o);
        whole.prefetch_counted(z, &ts, 1, &AtomicU64::new(0)).unwrap();
        for (k, b) in &held {
            assert!(b.is_some());
            assert_eq!(whole.held.lock().unwrap().get(k), Some(b), "{k:?}");
        }
        // (Task #896: the chunks as many as the job's threads, each its share of `IN_FLIGHT`: the
        // same bytes at any threads.)
        let all = whole.held.lock().unwrap().clone();
        assert_eq!(all.len(), ts.len());
        for n in [1, 3, 7] {
            let p = rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
            let t = Tiles::new(&plans, &o);
            p.install(|| t.prefetch_counted(z, &ts, n, &AtomicU64::new(0))).unwrap();
            assert_eq!(*t.held.lock().unwrap(), all, "{n} threads");
        }
    }
}
