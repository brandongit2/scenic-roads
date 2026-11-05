//! Terrain tiles (docs/steps.md §2, Terrain): one pyramid by one rule (`Fill`): a tile is sampled
//! from the elevation pool (crate::terrain_base) only at its place's finest zoom
//! (crate::terrain_zoom), each coarser quarter its children's 2×2 means (of their heights before
//! rounding), outside every region the relief (`Fills`). Levels are made finest first: each tile's
//! water is flattened and its bathymetry goes to sea level, and it's rounded only as it's stored.
//! The z6 tiles' runs (`build_piece`), the z3 tiles' assemblies (`build_lo`) and the root
//! (`build_root`) are here; a z10 tile's band past z12 is crate::terrain_deep's.

use roadcore::grid::decode_terrain;
use std::collections::HashMap;
use std::time::Duration;

/// Raw tiles got at once (each mostly waits on the NAS or S3; S3's requests themselves held to
/// crate::fetch::HOST_REQUESTS a host).
const FETCH_THREADS: usize = 64;
/// A z3 tile's mids read at once (`build_lo`): SMB's throughput wants a few calls in flight.
const MID_READERS: usize = 8;

/// The base's template in tests (nothing fetched: a test goes offline, or has its tiles).
#[cfg(test)]
pub const TEST_BASE: &str = "https://tiles.test/{z}/{x}/{y}.png";

/// Tile `z/x/y`'s URL by template `url`.
pub fn tile_url(url: &str, z: u8, x: u32, y: u32) -> String {
    url.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string())
}

/// What the terrain is made from besides its sampled tiles (docs/steps.md §2, Terrain): OSM's water
/// (crate::terrain_water), and which tiles are at their place's finest zoom. Default: no water,
/// every tile coarser than native.
#[derive(Clone, Copy, Default)]
pub struct Sources<'a> {
    pub water: Option<&'a dyn crate::terrain_water::WaterSource>,
    /// Whether tile `z/x/y` is at its place's finest zoom (crate::terrain_zoom::finest): its heights
    /// rounded to the centimetre (`quantum`).
    pub native: Option<&'a (dyn Fn(u8, u32, u32) -> bool + Sync)>,
    /// The declared datasets (crate::terrain_zoom): each place's finest zoom, which tiles a z6 tile
    /// makes (`piece_levels`).
    pub sets: &'a [crate::terrain_zoom::Set],
}

impl Sources<'_> {
    /// What tile `z/x/y`'s heights are rounded to as stored (crate::terrain_codec): to the
    /// centimetre at its place's finest zoom (`native`), else to the decimetre.
    pub fn quantum(&self, z: u8, x: u32, y: u32) -> f64 {
        if self.native.is_some_and(|n| n(z, x, y)) {
            crate::terrain_codec::NATIVE_M
        } else {
            crate::terrain_codec::COARSER_M
        }
    }
}

/// Where a quarter of a tile comes from (docs/steps.md §2, the rule), by its child one zoom finer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// The child is built (inside a region, at or above its place's finest zoom): its 2×2 means.
    Child,
    /// The child is past its place's finest zoom: this tile's own sample of the pool there.
    Sample,
    /// The child lies outside every region: the coarse relief, read from the datasets published at
    /// every zoom at this tile's zoom (crate::terrain_zoom::relief), one tile a tile.
    Relief,
}

/// The rule's quarters (`Fill`) for a terrain whose regions are `cov` (20 km about it) and whose
/// sources are `sets`: by child tile (its zoom, column, row). `deep`: the tiles past z12 a z6
/// tile's run takes in (its z10 tiles' deep mids, crate::terrain_deep): a z13 child there is
/// built only where one is.
pub fn fill_of<'a>(
    cov: &'a Coverage,
    sets: &'a [crate::terrain_zoom::Set],
    deep: &'a (dyn Fn(u32, u32) -> bool + Sync),
) -> impl Fn(u8, u32, u32) -> Fill + Sync + 'a {
    move |z, x, y| {
        if !near_coverage(cov, z, x, y) {
            Fill::Relief
        } else if z > crate::terrain_zoom::finest(sets, z, x, y) {
            Fill::Sample
        } else if z == crate::terrain_deep::FIRST_Z && !deep(x, y) {
            // (A z13 child no deep mid holds: its fine datasets have no data there.)
            Fill::Sample
        } else {
            Fill::Child
        }
    }
}

/// Whether tile `z/x/y` is at its place's finest zoom somewhere: one of its quarters is sampled
/// (`fill`), so its heights are rounded to the centimetre (`Sources::native`).
pub fn native_of(fill: &(dyn Fn(u8, u32, u32) -> Fill + Sync), z: u8, x: u32, y: u32) -> bool {
    (0..4u32).any(|k| fill(z + 1, 2 * x + (k & 1), 2 * y + (k >> 1)) == Fill::Sample)
}

/// The tiles a run reads (`RawSource`) by the rule (docs/steps.md §2): each tile's quarters by
/// `fill` from the pool's sample (`pool`) or the relief (`relief`), a quarter from its children
/// left void (their means go there: `prepare`); none where every quarter is its children's. So a
/// tile coarser than its place's finest zoom never samples the pool, and outside every region
/// only the relief's one tile is read.
///
/// Past z12 (the deep band, crate::terrain_deep) a tile is the place's finest only where some of its
/// pixels came from a dataset reaching its zoom (`reaching`, crate::terrain_zoom::reaching):
/// one that would only upsample a coarser dataset isn't kept (`keep`), and its parent takes that
/// quarter from its own sample (`built`).
pub struct Fills<'a> {
    pub pool: &'a dyn RawSource,
    pub relief: &'a dyn RawSource,
    pub fill: &'a (dyn Fn(u8, u32, u32) -> Fill + Sync),
    pub reaching: &'a (dyn Fn(u8, u32, u32) -> Vec<String> + Sync),
    built: Mutex<std::collections::HashSet<(u8, u32, u32)>>,
    kept: Mutex<HashMap<(u8, u32, u32), bool>>,
}

impl<'a> Fills<'a> {
    pub fn new(
        pool: &'a dyn RawSource,
        relief: &'a dyn RawSource,
        fill: &'a (dyn Fn(u8, u32, u32) -> Fill + Sync),
        reaching: &'a (dyn Fn(u8, u32, u32) -> Vec<String> + Sync),
    ) -> Self {
        Fills { pool, relief, fill, reaching, built: Default::default(), kept: Default::default() }
    }

    fn quarters(&self, z: u8, x: u32, y: u32) -> [Fill; 4] {
        let built = self.built.lock().unwrap();
        std::array::from_fn(|k| {
            let c = (2 * x + (k as u32 & 1), 2 * y + (k as u32 >> 1));
            match (self.fill)(z + 1, c.0, c.1) {
                // (A child the run hasn't built: none of its datasets has data there.)
                Fill::Child if z + 1 > crate::terrain_deep::FIRST_Z && !built.contains(&(z + 1, c.0, c.1)) => {
                    Fill::Sample
                }
                f => f,
            }
        })
    }
}

impl RawSource for Fills<'_> {
    fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
        let q = self.quarters(z, x, y);
        let take = |f: Fill, src: &dyn RawSource| -> anyhow::Result<(Option<Vec<f32>>, bool)> {
            if !q.contains(&f) {
                return Ok((None, false));
            }
            let (b, new) = src.get(z, x, y)?;
            Ok((b.and_then(|b| decode_terrain(&b).ok()), new))
        };
        let (sample, a) = take(Fill::Sample, self.pool)?;
        let (relief, b) = take(Fill::Relief, self.relief)?;
        if z >= crate::terrain_deep::FIRST_Z {
            // (By dataset, not code: datasets share codes, HRDEM's 1 m and 2 m alike.)
            let fine = sample.is_some()
                && self.pool.from(z, x, y).is_none_or(|from| {
                    let reach = (self.reaching)(z, x, y);
                    from.iter().any(|d| reach.contains(d))
                });
            // (The relief, read at its own declared zoom, never reaches the band: a deep tile of it
            // and of coarser samples is an upsampling, kept only where it has children.)
            self.kept.lock().unwrap().insert((z, x, y), fine);
        }
        if sample.is_none() && relief.is_none() {
            return Ok((None, a || b));
        }
        let mut e = vec![f32::NAN; 256 * 256];
        for (k, f) in q.iter().enumerate() {
            let from = match f {
                Fill::Sample => sample.as_ref(),
                Fill::Relief => relief.as_ref(),
                Fill::Child => None,
            };
            let Some(from) = from else { continue };
            let (ox, oy) = ((k & 1) * 128, (k >> 1) * 128);
            for j in 0..128 {
                let r = (oy + j) * 256 + ox;
                e[r..r + 128].copy_from_slice(&from[r..r + 128]);
            }
        }
        Ok((Some(roadcore::grid::encode_terrain_png(&e, 256, 256)?), a || b))
    }

    fn prefetch_counted(
        &self,
        z: u8,
        tiles: &[(u32, u32)],
        threads: usize,
        done: &std::sync::atomic::AtomicU64,
    ) -> anyhow::Result<usize> {
        let (mut sample, mut relief) = (Vec::new(), Vec::new());
        for &(x, y) in tiles {
            let q = self.quarters(z, x, y);
            if q.contains(&Fill::Sample) {
                sample.push((x, y));
            }
            if q.contains(&Fill::Relief) {
                relief.push((x, y));
            }
        }
        let none = std::sync::atomic::AtomicU64::new(0);
        let n = self.pool.prefetch_counted(z, &sample, threads, &none)?
            + self.relief.prefetch_counted(z, &relief, threads, &none)?;
        done.fetch_add(tiles.len() as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }

    fn built(&self, z: u8, tiles: &[(u32, u32)]) {
        self.built.lock().unwrap().extend(tiles.iter().map(|&(x, y)| (z, x, y)));
    }

    fn keep(&self, z: u8, x: u32, y: u32) -> bool {
        self.kept.lock().unwrap().remove(&(z, x, y)).unwrap_or(true)
    }
}

/// The basemap the terrain's water comes from: the latest pass's (`layers/basemap/world-<date>`),
/// its logical and content names. The terrain's keys name its water by digests of what each target
/// reads of it (`water_source_pin`, agent::build), so a new pass's basemap makes again the terrain
/// whose water changed.
pub fn water_pin(m: &std::collections::BTreeMap<String, String>) -> Option<(&str, &str)> {
    let prefix = "layers/basemap/world-";
    m.range(prefix.to_string()..)
        .take_while(|(l, _)| l.starts_with(prefix))
        .last()
        .map(|(l, c)| (l.as_str(), c.as_str()))
}

/// The pin of the terrain's water source as the jobs open it (`SourceFiles`: `WaterSource::pin`),
/// from the manifest alone: the latest pass's basemap's content name, and the lakes filled from
/// Canada's own data's after it when the manifest has them for that basemap
/// (crate::lakefill::current, filled_pin); None
/// without a basemap. What the terrain's keys name the water by (agent::build: its digests,
/// crate::terrain_water::WaterIdx).
pub fn water_source_pin(m: &std::collections::BTreeMap<String, String>) -> Option<String> {
    let (_, c) = water_pin(m)?;
    Some(crate::lakefill::filled_pin(c, crate::lakefill::current(m).map(|(_, f)| f)))
}

/// The terrain's water source, open (its pin `water_source_pin`'s): the latest pass's basemap, with
/// the fill's lakes when the manifest has a fill.
pub fn open_water(out: &Out) -> anyhow::Result<crate::lakefill::FilledWater<crate::terrain_water::BasemapWater>> {
    open_water_with(out, None)
}

/// `open_water`, the basemap read from `local`'s whole copy where it gives one (crate::localread:
/// the steps reading it in thousands of ranges, its shared cache's, crate::needs::CACHES).
pub fn open_water_with(
    out: &Out,
    local: Option<&crate::localread::Local>,
) -> anyhow::Result<crate::lakefill::FilledWater<crate::terrain_water::BasemapWater>> {
    use anyhow::Context;
    let (_, c) = water_pin(&out.manifest)
        .context("no basemap (layers/basemap/world-<date>): the terrain's water is the latest pass's basemap's")?;
    let base = crate::terrain_water::BasemapWater::open(&crate::localread::path(local, &out.path(c)), c)?;
    let fill = match crate::lakefill::current(&out.manifest) {
        Some((_, f)) => Some(crate::lakefill::Fill::open(&out.path(f), f)?),
        None => None,
    };
    Ok(crate::lakefill::FilledWater { base, fill })
}

/// Where a run reads AWS's raw tiles: this Mac's (`RawTiles`: its cache, the NAS's archives, else
/// AWS), or a task's own (crate::terrain_task: those its z8 subtrees read, cut by the job).
pub trait RawSource: Sync {
    /// The raw tile (None: AWS has none), and whether it came from AWS just now.
    fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)>;
    /// Makes the tiles of `tiles` (zoom `z`) ready to read, counting each into `done` as it is; the
    /// number that came from AWS.
    fn prefetch_counted(
        &self,
        z: u8,
        tiles: &[(u32, u32)],
        threads: usize,
        done: &std::sync::atomic::AtomicU64,
    ) -> anyhow::Result<usize>;
    /// The tiles of zoom `z` built (a level's, or what it starts from: its children's means), so a
    /// quarter whose child isn't among them is the tile's own sample (`Fills`).
    fn built(&self, _z: u8, _tiles: &[(u32, u32)]) {}
    /// Whether tile `z/x/y` as read is kept where none of its children is (`Fills::keep`).
    fn keep(&self, _z: u8, _x: u32, _y: u32) -> bool {
        true
    }
    /// The datasets (their ids) tile `z/x/y`'s pixels came from as sampled, once (None: not told,
    /// as from every one).
    fn from(&self, _z: u8, _x: u32, _y: u32) -> Option<Vec<String>> {
        None
    }
}

impl RawSource for RawTiles {
    fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
        RawTiles::get(self, z, x, y)
    }
    fn prefetch_counted(
        &self,
        z: u8,
        tiles: &[(u32, u32)],
        threads: usize,
        done: &std::sync::atomic::AtomicU64,
    ) -> anyhow::Result<usize> {
        RawTiles::prefetch_counted(self, z, tiles, threads, done)
    }
}

/// A tile made but for its water (`prepare`): its elevations (the raw tile's, the quarters over
/// children and the pixels above changed ones made again from them), the raw tile's as decoded,
/// its water, and what it's rounded to stored (`Sources::quantum`). None of it when the raw tile
/// doesn't decode (no tile).
pub struct Prepared {
    pub e: Vec<f32>,
    pub before: Vec<f32>,
    pub water: Option<std::sync::Arc<crate::terrain_water::WaterTile>>,
    decoded: bool,
    z: u8,
    quantum: f64,
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

/// A tile's elevations made (docs/steps.md §2, Terrain), but for its water: its raw tile's (`raw`:
/// its sampled and relief quarters, `Fills`; None: every quarter its children's), each quarter whose
/// child tile exists made from it (`quads`, the children's 2×2 means of their heights before
/// rounding, their water flattened: a coarser zoom's pixel is the mean of the finer ones under it);
/// its voids (pixels not `roadcore::grid::defined`) at sea level. Then `finish` flattens its water.
/// Not decoded (no tile) with neither. An error where its water can't be read: a tile made without
/// it would be kept under a key naming that water.
pub fn prepare(
    raw: Option<Vec<u8>>,
    z: u8,
    x: u32,
    y: u32,
    quads: &HashMap<(u32, u32), Vec<f32>>,
    src: &Sources,
) -> anyhow::Result<Prepared> {
    let quantum = src.quantum(z, x, y);
    let kids = (0..4u32).any(|k| quads.contains_key(&(x * 2 + (k & 1), y * 2 + (k >> 1))));
    let mut e = match raw.map(|b| decode_terrain(&b)) {
        Some(Ok(e)) => e,
        _ if kids => vec![f32::NAN; 256 * 256],
        _ => return Ok(Prepared { e: Vec::new(), before: Vec::new(), water: None, decoded: false, z, quantum }),
    };
    let before = e.clone();
    for k in 0..4u32 {
        let (dx, dy) = (k & 1, k >> 1);
        let Some(q) = quads.get(&(x * 2 + dx, y * 2 + dy)) else { continue };
        for j in 0..128 {
            let row = (dy as usize * 128 + j) * 256 + dx as usize * 128;
            e[row..row + 128].copy_from_slice(&q[j * 128..(j + 1) * 128]);
        }
    }
    for v in e.iter_mut() {
        if !roadcore::grid::defined(*v) {
            *v = 0.0;
        }
    }
    let water = match src.water {
        Some(w) => crate::terrain_water::tile_water(w, z, x, y, true)
            .map_err(|e| e.context(format!("the water of {z}/{x}/{y}")))?,
        None => None,
    };
    Ok(Prepared { e, before, water, decoded: true, z, quantum })
}

/// A prepared tile finished: its water flattened (`levels`: the lakes' levels,
/// crate::terrain_water), bathymetry to sea level, rounded and encoded (crate::terrain_codec).
/// Returns the tile to store (None: its raw tile didn't decode), whether its water or children
/// moved a pixel by over half a metre from the raw tile's, and from z1 on its 2×2 means (of the
/// heights as stored) for the level above.
pub fn finish(p: Prepared, levels: &HashMap<u64, f32>) -> (Option<Vec<u8>>, bool, Option<Vec<f32>>) {
    let Prepared { mut e, before, water, decoded, z, quantum } = p;
    if !decoded {
        return (None, false, None);
    }
    if let Some(w) = &water {
        crate::terrain_water::flatten(&mut e, w, levels);
    }
    for v in e.iter_mut() {
        if *v < 0.0 {
            *v = 0.0;
        }
    }
    let changed = e.iter().zip(&before).any(|(a, b)| !((*a - *b).abs() <= 0.5));
    // (The level above takes its heights before rounding: rounded once, as stored, not a level at a
    // time; the owner's, 4 Nov.)
    let quad = (z >= 1).then(|| quad_mean(&e));
    let out = crate::terrain_codec::encode(&mut e, quantum).ok();
    (out, changed, quad)
}

/// A 256-pixel tile's heights at half the size: each pixel the plain mean of the 2×2 under it
/// (summed in this order, so a parent's bytes stay as they are).
fn quad_mean(e: &[f32]) -> Vec<f32> {
    let mut q = vec![0f32; 128 * 128];
    for j in 0..128 {
        for i in 0..128 {
            let (cx, cy) = (i * 2, j * 2);
            q[j * 128 + i] =
                (e[cy * 256 + cx] + e[cy * 256 + cx + 1] + e[(cy + 1) * 256 + cx] + e[(cy + 1) * 256 + cx + 1]) * 0.25;
        }
    }
    q
}

/// A tile made whole (`prepare`, then `finish` with its lakes' levels from it alone): what a tile
/// made alone gets.
pub fn process(
    raw: Option<Vec<u8>>,
    z: u8,
    x: u32,
    y: u32,
    quads: &HashMap<(u32, u32), Vec<f32>>,
    src: &Sources,
) -> anyhow::Result<(Option<Vec<u8>>, bool, Option<Vec<f32>>)> {
    let p = prepare(raw, z, x, y, quads, src)?;
    let mut levels = HashMap::new();
    crate::terrain_water::add_levels(&mut levels, &p.lake_samples());
    Ok(finish(p, &levels))
}

/// Latitude of a tile's centre.
pub fn tile_lat(z: u8, ty: u32) -> f64 {
    let y = (ty as f64 + 0.5) / (1u64 << z) as f64;
    roadcore::geo::merc_lat(y)
}

// ---- per pack ----------------------------------------------------------------------------------

use crate::coverage::Coverage;
use crate::out::Out;
use rayon::prelude::*;
use std::sync::Mutex;

/// Whether the coverage comes within `roadcore::geo::NEAR_COVERAGE_KM` of the tile: the tile grown by
/// it meets it (exactly, so a small region between sample points isn't missed: Singapore in its z6
/// tile).
pub fn near_coverage(cov: &Coverage, z: u8, x: u32, y: u32) -> bool {
    cov.meets_rect(crate::agent::build::grown_e7(z, x, y, roadcore::geo::NEAR_COVERAGE_KM))
}

/// A layer's tiles as the build manifest has them now (its latest uploads).
pub struct ManifestTiles {
    layer: String,
    /// The layer's packs as the manifest named them when it was made (logical → file): its own,
    /// so the job may write packs while it reads (a slope run writing its own while it reads the
    /// terrain's).
    files: HashMap<String, store::nasfs::NasPath>,
    open: Mutex<HashMap<String, Option<std::sync::Arc<OpenPack>>>>,
    /// Where a pack read tile by tile throughout is copied whole and read from then on
    /// (`with_copies`, `copy_here`), and the copies made or being made, by logical name: deleted
    /// when let go (`keep_only`) or with this.
    copies: Option<(std::path::PathBuf, Mutex<HashMap<String, std::path::PathBuf>>)>,
}

/// A pack open: its file and index.
struct OpenPack {
    file: std::fs::File,
    /// What its reads wait on (the NAS, or this Mac's disk for a copy): held around each.
    on: store::waits::On,
    idx: store::pack::PackIndex,
}

impl ManifestTiles {
    pub fn new(out: &Out, layer: &str) -> Self {
        let prefix = format!("layers/{layer}/");
        let files = out
            .manifest
            .range(prefix.clone()..)
            .take_while(|(l, _)| l.starts_with(&prefix))
            .map(|(l, c)| (l.clone(), out.path(c)))
            .collect();
        ManifestTiles { layer: layer.to_string(), files, open: Mutex::new(HashMap::new()), copies: None }
    }

    /// `new`, the packs `copy_here` is asked for copied into `dir` (the job's scratch) whole and
    /// read there; the copies deleted when let go (`keep_only`) or when this goes. The same bytes
    /// either way: a pack is content-named, never rewritten.
    pub fn with_copies(out: &Out, layer: &str, dir: std::path::PathBuf) -> Self {
        let mut t = Self::new(out, layer);
        t.copies = Some((dir, Mutex::new(HashMap::new())));
        t
    }

    pub fn logical(layer: &str, z: u8, x: u32, y: u32) -> String {
        let deep = crate::terrain_deep::PACK_Z;
        match z {
            0..=2 => crate::layers::root_logical(layer),
            3..=8 => crate::layers::lo_logical(layer, x >> (z - 3), y >> (z - 3)),
            // (The terrain's deep levels and their shading, by z10 tile: crate::terrain_deep,
            // crate::slope_pack.)
            _ if (layer == "terrain" || layer == "slope") && z >= crate::terrain_deep::FIRST_Z => {
                crate::layers::deep_logical(layer, x >> (z - deep), y >> (z - deep))
            }
            _ => crate::layers::hi_logical(layer, x >> (z - 6), y >> (z - 6)),
        }
    }

    /// A pack opened and its index read, here (a copy) or on the NAS, under `nas_call`'s deadline
    /// and retries.
    fn open_file(p: store::nasfs::Place) -> anyhow::Result<std::sync::Arc<OpenPack>> {
        let shown = p.loc().display();
        let open = nas_call(&shown, NAS_DEADLINE, &store::retry::waits(), move || {
            let file = p.loc().open()?;
            let on = p.loc().on();
            let _w = store::waits::wait(on);
            let len = file.metadata()?.len();
            let idx = store::pack::PackIndex::read_from(&FileSource(&file, len)).map_err(store::retry::as_io)?;
            Ok(std::sync::Arc::new(OpenPack { file, on, idx }))
        });
        anyhow::Context::with_context(open, || format!("open the pack {shown}"))
    }

    /// The open pack holding tile (z, x, y), if the manifest has it.
    fn pack(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Option<std::sync::Arc<OpenPack>>> {
        self.open_logical(Self::logical(&self.layer, z, x, y))
    }

    /// The tiles of pack `logical`, from its index (none when the manifest lacks it).
    pub fn tiles_in(&self, logical: &str) -> anyhow::Result<Vec<(u8, u32, u32)>> {
        Ok(self
            .open_logical(logical.to_string())?
            .map_or_else(Vec::new, |e| e.idx.entries.iter().map(|e| e.zxy()).collect()))
    }

    /// Pack `logical` open, if the manifest has it.
    fn open_logical(&self, logical: String) -> anyhow::Result<Option<std::sync::Arc<OpenPack>>> {
        let mut open = self.open.lock().unwrap();
        if !open.contains_key(&logical) {
            let v = match self.files.get(&logical) {
                Some(p) => Some(Self::open_file(store::nasfs::Place::Nas(p.clone()))?),
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
        let (off, len) = (ent.offset, ent.len as usize);
        let b = nas_call(&format!("{}'s {z}/{x}/{y}", self.layer), NAS_DEADLINE, &store::retry::waits(), move || {
            let mut b = vec![0u8; len];
            let _w = store::waits::wait(e.on);
            e.file.read_exact_at(&mut b, off)?;
            Ok(b)
        })?;
        Ok(Some(b))
    }

    /// Tiles `want` (each as `get` gives it), read a pack at a time in spans: the tiles' bytes in the
    /// pack's order, neighbours a gap of under `SPAN_GAP` apart read together, up to `SPAN_MAX` a
    /// read, the spans in parallel. A z6 tile reading thousands of a pack's tiles makes a few dozen
    /// reads of the share instead of a round trip a tile, and moves little it doesn't use. Each
    /// span's read is under `nas_call`'s deadline and retries, on `SPAN_READERS` threads of its own
    /// (not rayon's: a job computing meanwhile, the peaks' next z6 tile read ahead, doesn't hold them up).
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
            by_pack
                .entry(Self::logical(&self.layer, z, x, y))
                .or_insert_with(|| (e, Vec::new()))
                .1
                .push((ent.offset, ent.len, i));
        }
        let mut spans: Vec<(std::sync::Arc<OpenPack>, u64, u64, Vec<(u64, u32, usize)>)> = Vec::new();
        for (_, (e, mut ents)) in by_pack {
            ents.sort_unstable();
            for t in ents {
                let end = t.0 + t.1 as u64;
                match spans.last_mut() {
                    Some((p, s0, s1, ts))
                        if std::sync::Arc::ptr_eq(p, &e) && t.0 <= *s1 + SPAN_GAP && end - *s0 <= SPAN_MAX =>
                    {
                        *s1 = (*s1).max(end);
                        ts.push(t);
                    }
                    _ => spans.push((e.clone(), t.0, end, vec![t])),
                }
            }
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        let read: Vec<anyhow::Result<Vec<(usize, Vec<u8>)>>> = std::thread::scope(|sc| {
            let readers: Vec<_> = (0..SPAN_READERS.min(spans.len()))
                .map(|_| {
                    sc.spawn(|| {
                        let mut mine = Vec::new();
                        loop {
                            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some((e, s0, s1, ts)) = spans.get(k) else { break };
                            let (e, s0, n) = (e.clone(), *s0, (s1 - s0) as usize);
                            let what = format!("{}'s bytes {s0}..{s1}", self.layer);
                            let b = nas_call(&what, NAS_DEADLINE, &store::retry::waits(), move || {
                                let mut b = vec![0u8; n];
                                let _w = store::waits::wait(e.on);
                                e.file.read_exact_at(&mut b, s0)?;
                                Ok(b)
                            });
                            mine.push(b.map_err(anyhow::Error::from).map(|b| {
                                ts.iter()
                                    .map(|&(o, l, i)| {
                                        (i, b[(o - s0) as usize..(o - s0) as usize + l as usize].to_vec())
                                    })
                                    .collect()
                            }));
                        }
                        mine
                    })
                })
                .collect();
            readers.into_iter().flat_map(|r| r.join().expect("a span reader panicked")).collect()
        });
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
    /// pack, no copies' folder, the copy failed (then it's read from the NAS, as before, and copied
    /// when asked again), or it is copied here already (or being copied, on another thread) and
    /// not let go since (`keep_only`): a pack kept is copied once. Safe beside reads of it and of
    /// the others: until its copy is whole, its reads go to the NAS's.
    pub fn copy_here(&self, z: u8, x: u32, y: u32) -> Option<u64> {
        let logical = Self::logical(&self.layer, z, x, y);
        let (Some((dir, made)), Some(src)) = (&self.copies, self.files.get(&logical)) else { return None };
        let dst = dir.join(src.file_name()?);
        {
            let mut made = made.lock().unwrap();
            if made.contains_key(&logical) {
                return None;
            }
            made.insert(logical.clone(), dst.clone());
        }
        let copied = (|| -> anyhow::Result<(std::sync::Arc<OpenPack>, u64)> {
            std::fs::create_dir_all(dir)?;
            let n = src.copy_data_to_local(&dst)?;
            anyhow::ensure!(n == src.metadata()?.len(), "copied {n} bytes of {}", src.display());
            Ok((Self::open_file(store::nasfs::Place::Local(dst.clone()))?, n))
        })();
        // (Taken only while it's still kept: one let go while it was copied is deleted, as
        // `keep_only` would have.)
        let mut made = made.lock().unwrap();
        let kept = made.get(&logical) == Some(&dst);
        match copied {
            Ok((e, n)) if kept => {
                self.open.lock().unwrap().insert(logical, Some(e));
                Some(n)
            }
            Ok(_) => {
                std::fs::remove_file(&dst).ok();
                None
            }
            Err(err) => {
                eprintln!("{logical}: not copied here, read from the NAS ({err:#})");
                if kept {
                    made.remove(&logical);
                }
                std::fs::remove_file(&dst).ok();
                None
            }
        }
    }

    /// Whether the layer has tile (z, x, y), from its pack's index alone.
    pub fn has(&self, z: u8, x: u32, y: u32) -> anyhow::Result<bool> {
        Ok(self.pack(z, x, y)?.is_some_and(|e| e.idx.find(z, x, y).is_some()))
    }

    /// The packs open, copied here or not, kept only where `keep` says (by logical name): the rest
    /// closed and their copies deleted (one being made too: `copy_here` drops it as it ends), so a
    /// reader kept for a whole job holds what's still to be read and no more. A pack let go and
    /// read again is opened again where it is, the NAS's, or copied again when asked.
    pub fn keep_only(&self, keep: &dyn Fn(&str) -> bool) {
        // (Copies first, under their lock: a copy ending meanwhile then finds itself let go, and
        // its open pack never outlives this.)
        if let Some((_, made)) = &self.copies {
            made.lock().unwrap().retain(|l, p| {
                keep(l) || {
                    std::fs::remove_file(p).ok();
                    false
                }
            });
        }
        self.open.lock().unwrap().retain(|l, _| keep(l));
    }
}

impl Drop for ManifestTiles {
    fn drop(&mut self) {
        self.keep_only(&|_| false);
    }
}

/// How long one read or open of a pack may take (a span is 32 MB at most: over a minute even at
/// 0.5 MB/s) before it fails: an SMB call can stay stuck in the kernel for many minutes.
pub const NAS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// The threads `get_many` reads a layer's spans on (SMB's throughput wants a few calls in flight).
const SPAN_READERS: usize = 8;

/// `f` on a thread of its own, waited for `deadline` at most (past it, `TimedOut`, not tried again:
/// the thread stuck in it left behind, the job's own retry takes over), tried again after each of
/// `waits` while it fails as the share does for a moment (store::retry::transient_or_missing),
/// each retry logged. No call through it blocks for ever.
pub fn nas_call<T: Send + 'static>(
    what: &str,
    deadline: std::time::Duration,
    waits: &[std::time::Duration],
    f: impl Fn() -> std::io::Result<T> + Send + Sync + 'static,
) -> std::io::Result<T> {
    let f = std::sync::Arc::new(f);
    store::retry::io(what, None, waits, store::retry::transient_or_missing, || {
        let (tx, rx) = std::sync::mpsc::channel();
        let g = f.clone();
        // (The worker is this retry's try: a NasPath call in it tries once, store::retry::attempt.)
        let nested = store::retry::in_attempt();
        std::thread::Builder::new().name("pack-read".into()).spawn(move || {
            let _a = nested.then(store::retry::attempt);
            let _ = tx.send(g());
        })?;
        rx.recv_timeout(deadline).unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{what}: over {} s (the NAS's share stuck?)", deadline.as_secs()),
            ))
        })
    })
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

/// The index file a version of the raw tiles' index is (`Stamp`): its inode (on the NAS, the
/// server's file ID on the share), time and size. The index is only ever written by a temporary
/// name and renamed over (rawpack's `Index::save`, crate::whole), and a renamed file has a new file
/// ID as the share gives them, so two versions don't share a stamp even written a moment apart with
/// an archive swapped for another of a name as long (the same time and size). A change the stamp
/// missed would cost a re-fetch (a tile asked of AWS that a newer archive had), not correctness:
/// the archives looked in are those the index they were opened by named, held open. None: no
/// index there.
type Stamp = Option<(u64, std::time::SystemTime, u64)>;

fn stamp_of(m: &std::fs::Metadata) -> Stamp {
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(m);
    #[cfg(not(unix))]
    let ino = 0;
    Some((ino, m.modified().ok()?, m.len()))
}

/// The raw tiles' archives' index in `store` as read now, with the stamp of the very file it was
/// read from (taken from the open file, not its name again); none there, empty.
fn load_index(store: &store::nasfs::NasPath) -> anyhow::Result<(crate::rawpack::Index, Stamp)> {
    use std::io::Read;
    let p = store.join(crate::rawpack::INDEX);
    let mut f = match p.open() {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((crate::rawpack::Index::default(), None)),
        Err(e) => return Err(anyhow::Error::from(e).context(format!("read {}", p.display()))),
    };
    let stamp = f.metadata().ok().as_ref().and_then(stamp_of);
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|e| anyhow::anyhow!("read {}: {e}", p.display()))?;
    Ok((serde_json::from_slice(&b).map_err(|e| anyhow::anyhow!("parse {}: {e}", p.display()))?, stamp))
}

/// The stamp of the index in `store` as it is now (a stat: `Stamp`); Err when it can't be told.
fn index_stamp(store: &store::nasfs::NasPath) -> Result<Stamp, ()> {
    match store.join(crate::rawpack::INDEX).metadata() {
        Ok(m) => Ok(stamp_of(&m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// A shard's raw tile archives as read (RawTiles): its first tiles straight from the NAS's (each
/// archive's entries read once, then a ranged read a tile), so a job wanting a tile or two of an
/// shard doesn't copy hundreds of MB; past RANGED of them, copied here whole and opened (one large
/// read each, then local, as a terrain job reads thousands). One whose copy failed is read by range
/// for the rest of the job (`stay`), not copied again for each tile.
enum Opened {
    Ranged { archives: RangedArchives, reads: usize, stay: bool },
    Here(std::sync::Arc<[roadcore::archive::Archive]>),
}

/// A shard's archives read by range: each one's NAS file and its entries. Shared, so a tile is
/// read from them outside the shard's lock (`archived_by`: a read of the NAS held every other
/// thread wanting a tile of the shard).
type RangedArchives = std::sync::Arc<Vec<(std::fs::File, Vec<roadcore::archive::Entry>, store::waits::On)>>;

/// The tiles of a shard read straight from the NAS's archives before they're copied here whole.
const RANGED: usize = 16;

/// AWS's raw tiles, kept so each is downloaded once: in the lead's cache as they come, then on
/// the NAS packed (crate::rawpack: archives grouped as the terrain is, packed at the end of the job
/// that fetched them, or by room-making), read from there (`Opened`: a few tiles of a shard by
/// range, else its archives copied here whole); and the NAS's loose tiles from before (`sources/aws-terrarium/<z>/<x>/<y>.png`,
/// `.none` for a tile AWS doesn't have), while they're there. Packs are always made from the same
/// immutable source: processing a tile twice isn't idempotent, so stored (processed) tiles are never
/// an input. Each loose copy is made whole under a name of its own, then named (store::cachefile); every
/// tile is checked whole when read, a loose one or an archive's: one that isn't (cut short) is
/// passed over (a loose one deleted) and taken from the next source, the NAS's copy, else AWS.
pub struct RawTiles {
    dir: std::path::PathBuf,
    /// The base's URL template (crate::terrain_sources::Declared::base).
    url: String,
    store: Option<store::nasfs::NasPath>,
    /// The store's columns (`<z>/<x>/`) as listed once: over SMB each look is a round trip, and
    /// those, a tile's few, set a terrain job's pace. (The folders here are made as tiles are put:
    /// store::cachefile.)
    listed: Mutex<HashMap<(u8, u32), std::sync::Arc<std::collections::HashSet<String>>>>,
    /// The store's archives' index: read when first wanted (tried again a minute after it can't
    /// be), and again when an archive it names is gone from the NAS (a job that outlived it).
    index: Mutex<(Option<std::sync::Arc<crate::rawpack::Index>>, Option<std::time::Instant>)>,
    /// The index's stamp as it was read (`load_index`): one changed since (an archive packed meanwhile,
    /// by another job here or room-making, of tiles this job's cache held) has a shard's archives
    /// opened again before a tile they lack is fetched from AWS.
    index_stamp: Mutex<Stamp>,
    /// The index's stamp each shard's archives were opened by (`index_stamp` as it
    /// was then): a shard whose index has changed since is opened again before AWS is asked for a
    /// tile it lacks, whichever shard read the index last.
    opened_at: Mutex<HashMap<String, Stamp>>,
    /// Each shard's archives, newest first, opened when first wanted: by the shard's own lock, so
    /// reading one from the NAS holds up only the tiles of that shard.
    archives: Mutex<HashMap<String, std::sync::Arc<Mutex<Option<Opened>>>>>,
}

impl RawTiles {
    /// A local cache alone (no NAS store), fetched by URL template `url`.
    pub fn new(dir: &std::path::Path, url: &str) -> Self {
        RawTiles {
            dir: dir.to_path_buf(),
            url: url.to_string(),
            store: None,
            listed: Default::default(),
            index: Default::default(),
            index_stamp: Default::default(),
            opened_at: Default::default(),
            archives: Default::default(),
        }
    }

    /// The local cache `dir`, filled from the NAS's `store` where it has a tile, else by URL template
    /// `url`.
    pub fn with_store(dir: &std::path::Path, store: &store::nasfs::NasPath, url: &str) -> Self {
        RawTiles {
            dir: dir.to_path_buf(),
            url: url.to_string(),
            store: Some(store.clone()),
            listed: Default::default(),
            index: Default::default(),
            index_stamp: Default::default(),
            opened_at: Default::default(),
            archives: Default::default(),
        }
    }

    /// The names in the store's column `z/x`, listed once (none when it isn't there); None when it
    /// can't be listed whole now (then each tile is looked for itself, and it's listed again later).
    fn column(
        &self,
        st: &store::nasfs::NasPath,
        z: u8,
        x: u32,
    ) -> Option<std::sync::Arc<std::collections::HashSet<String>>> {
        if let Some(c) = self.listed.lock().unwrap().get(&(z, x)) {
            return Some(c.clone());
        }
        let names: std::collections::HashSet<String> = match st.join(format!("{z}/{x}")).read_dir() {
            Ok(rd) => rd.into_iter().map(|e| e.name).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(_) => return None,
        };
        let c = std::sync::Arc::new(names);
        self.listed.lock().unwrap().insert((z, x), c.clone());
        Some(c)
    }

    /// The store's archives' index: as read before unless `fresh`; None when it can't be read (three
    /// tries a moment apart, then none for a minute: each try waits on the NAS).
    fn index(&self, st: &store::nasfs::NasPath, fresh: bool) -> Option<std::sync::Arc<crate::rawpack::Index>> {
        self.index_pair(st, fresh).map(|p| p.0)
    }

    /// `index`, with the stamp of the bytes it was read from (`load_index`), taken together under
    /// its lock: a shard opened by it is known by the stamp of the index it was opened by, not one
    /// another thread read since.
    fn index_pair(
        &self,
        st: &store::nasfs::NasPath,
        fresh: bool,
    ) -> Option<(std::sync::Arc<crate::rawpack::Index>, Stamp)> {
        let mut s = self.index.lock().unwrap();
        let pair = |s: &(Option<std::sync::Arc<crate::rawpack::Index>>, Option<std::time::Instant>)| {
            s.0.clone().map(|ix| (ix, *self.index_stamp.lock().unwrap()))
        };
        if let (Some(_), false) = (&s.0, fresh) {
            return pair(&s);
        }
        if s.1.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return pair(&s);
        }
        for attempt in 0..3 {
            match load_index(st) {
                Ok((ix, stamp)) => {
                    *self.index_stamp.lock().unwrap() = stamp;
                    *s = (Some(std::sync::Arc::new(ix)), None);
                    return pair(&s);
                }
                Err(e) if attempt == 2 => eprintln!("terrain: the raw tiles' archives: {e:#}"),
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        s.1 = Some(std::time::Instant::now());
        pair(&s)
    }

    /// A shard's archives, newest first: here when each has its copy here, else to be read by range
    /// (each one's entries read now, from its copy here or the NAS's). None when one can't be (not
    /// remembered: the NAS may answer next time). An archive the index names that's gone from the
    /// NAS (a job that outlived the index it read) has the index read again.
    fn open_shard(&self, st: &store::nasfs::NasPath, shard: &str) -> Option<Opened> {
        for fresh in [false, true] {
            let (index, stamp) = self.index_pair(st, fresh)?;
            // (Known by the stamp of the index it's opened by, as `get` compares.)
            self.opened_at.lock().unwrap().insert(shard.to_string(), stamp);
            let packs = index.of(shard);
            if packs.iter().all(|p| self.dir.join("packs").join(&p.name).exists()) {
                if let Some(here) = self.copy_shard(st, packs) {
                    return Some(Opened::Here(here));
                }
            }
            let mut v = Vec::new();
            for p in packs.iter().rev() {
                // (This Mac's copy held as it's read: store::cachefile.)
                let local = self.dir.join("packs").join(&p.name);
                let at = match store::cachefile::hold_existing(&local) {
                    Ok(Some(l)) => store::nasfs::Place::Local(l),
                    _ => store::nasfs::Place::Nas(st.join("packs").join(&p.name)),
                };
                match crate::rawpack::entries_of(&at) {
                    Ok(e) => v.push((e.0, e.1, at.loc().on())),
                    // (Gone from the NAS: the index read before is out of date.)
                    Err(e) if !fresh && crate::rawpack::not_found(&e) => break,
                    Err(e) => {
                        eprintln!("terrain: the raw tiles of {shard}: {e:#}");
                        return None;
                    }
                }
            }
            if v.len() == packs.len() {
                return Some(Opened::Ranged { archives: std::sync::Arc::new(v), reads: 0, stay: false });
            }
        }
        None
    }

    /// Archives `packs`' copies here, newest first, each copied from the NAS whole the first time
    /// (checked against its name) and marked used (room-making deletes the least recently used);
    /// None when one can't be.
    fn copy_shard(
        &self,
        st: &store::nasfs::NasPath,
        packs: &[crate::rawpack::Pack],
    ) -> Option<std::sync::Arc<[roadcore::archive::Archive]>> {
        let mut v = Vec::new();
        for p in packs.iter().rev() {
            // (Held, and marked used: rawpack::local_copy, store::cachefile.)
            let opened =
                crate::rawpack::local_copy(&self.dir, st, &p.name).and_then(|l| roadcore::archive::Archive::open(&l));
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

    /// A tile from its shard's archives: Some(None) when AWS hasn't it, None when they (or the
    /// store) haven't it whole.
    fn archived(&self, z: u8, x: u32, y: u32) -> Option<Option<Vec<u8>>> {
        self.archived_by(z, x, y, &mut None)
    }

    /// `archived`, saying in `used` the stamp of the index the shard's archives it looked in were
    /// opened by (taken under the shard's lock: another thread may open them again a moment after),
    /// so `get` opens them again only when the index has changed since those were opened. The
    /// shard's lock covers opening its archives and choosing which to read; the tile's read (a NAS
    /// round trip by range) is outside it, so the threads sampling a level read a shard's tiles
    /// beside each other.
    fn archived_by(&self, z: u8, x: u32, y: u32, used: &mut Option<Stamp>) -> Option<Option<Vec<u8>>> {
        let st = self.store.as_ref()?;
        let shard = crate::rawpack::shard(z, x, y);
        let cell = self.archives.lock().unwrap().entry(shard.clone()).or_default().clone();
        let key = roadcore::archive::tile_key(z, x, y);
        let whole = |b: &[u8]| -> Option<Option<Vec<u8>>> {
            if b.is_empty() {
                Some(None)
            } else if crate::whole::png_whole(b) {
                Some(Some(b.to_vec()))
            } else {
                eprintln!("terrain: {z}/{x}/{y} in an archive of {shard} isn't whole: passed over");
                None
            }
        };
        let here = {
            let mut c = cell.lock().unwrap();
            if c.is_none() {
                let opened = self.open_shard(st, &shard);
                *used = Some(self.opened_at.lock().unwrap().get(&shard).copied().flatten());
                *c = Some(opened?);
            }
            // (Read a few times: copied here whole, if it can be, by the index as it is now, which
            // its archives are then known by; else read by range from now on.)
            if let Some(Opened::Ranged { reads, stay: stay @ false, .. }) = c.as_mut() {
                if *reads >= RANGED {
                    match self
                        .index_pair(st, false)
                        .and_then(|(index, stamp)| self.copy_shard(st, index.of(&shard)).map(|h| (h, stamp)))
                    {
                        Some((h, stamp)) => {
                            *c = Some(Opened::Here(h));
                            self.opened_at.lock().unwrap().insert(shard.clone(), stamp);
                        }
                        None => *stay = true,
                    }
                }
            }
            *used = Some(self.opened_at.lock().unwrap().get(&shard).copied().flatten());
            match c.as_mut()? {
                Opened::Here(h) => Ok(h.clone()),
                Opened::Ranged { archives, reads, .. } => {
                    *reads += 1;
                    Err(archives.clone())
                }
            }
        };
        match here {
            Ok(here) => here.iter().find_map(|a| a.get(z, x, y).and_then(|b| whole(b))),
            Err(archives) => {
                for (f, entries, on) in archives.iter() {
                    let Ok(i) = entries.binary_search_by_key(&key, |e| e.key) else { continue };
                    let mut b = vec![0u8; entries[i].len as usize];
                    let w = store::waits::wait(*on);
                    if let Err(e) = store::sys::PosIo::read_exact_at(f, &mut b, entries[i].offset) {
                        eprintln!("terrain: {z}/{x}/{y} from an archive of {shard}: {e}");
                        return None;
                    }
                    drop(w);
                    if let Some(t) = whole(&b) {
                        return Some(t);
                    }
                }
                None
            }
        }
    }

    /// Whether tile (z, x, y) is in its shard's archives, by its entry alone (no tile read): the
    /// prefetch passes those over, as the build reads them itself (and checks them whole, taking
    /// one that isn't again then).
    fn in_archive(&self, z: u8, x: u32, y: u32) -> bool {
        let Some(st) = self.store.as_ref() else { return false };
        let shard = crate::rawpack::shard(z, x, y);
        let cell = self.archives.lock().unwrap().entry(shard.clone()).or_default().clone();
        let mut c = cell.lock().unwrap();
        if c.is_none() {
            match self.open_shard(st, &shard) {
                Some(o) => *c = Some(o),
                None => return false,
            }
        }
        let key = roadcore::archive::tile_key(z, x, y);
        match c.as_ref() {
            Some(Opened::Here(h)) => h.iter().any(|a| a.get(z, x, y).is_some()),
            Some(Opened::Ranged { archives, .. }) => {
                archives.iter().any(|(_, e, _)| e.binary_search_by_key(&key, |e| e.key).is_ok())
            }
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
        // In its shard's archive.
        let mut used = None;
        if let Some(b) = self.archived_by(z, x, y, &mut used) {
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
            // Packed into an archive since this job opened its shard's (its loose copy here taken by
            // room-making or another job's packing meanwhile): its shard's archives opened again,
            // the index read afresh, before AWS is asked (docs/pool.md §7.7: two raw-tile jobs at
            // once on one Mac).
            let shard = crate::rawpack::shard(z, x, y);
            if self.refresh_shard(st, &shard, used) {
                if let Some(b) = self.archived(z, x, y) {
                    return Ok((b, false));
                }
            }
        }
        self.fetch(z, x, y).map(|b| (b, true))
    }

    /// Whether shard `shard`'s archives may hold a tile they lacked when looked in (`used`: the stamp of
    /// the index they were opened by then; None: none looked in): decided, and acted on, under the
    /// shard's lock, so no other thread opens them from an older index between. Opened again by the
    /// index read afresh when the index has changed since, or when they couldn't be opened (true);
    /// true too when another thread has opened them again since that look (look again); false when
    /// nothing has changed (the tile isn't in them: AWS is asked).
    fn refresh_shard(&self, st: &store::nasfs::NasPath, shard: &str, used: Option<Stamp>) -> bool {
        let Some(used) = used else { return false };
        // (The map let go before the shard's lock is waited on.)
        let cell = self.archives.lock().unwrap().entry(shard.to_string()).or_default().clone();
        let mut c = cell.lock().unwrap();
        let cur = self.opened_at.lock().unwrap().get(shard).copied().flatten();
        let now = index_stamp(st);
        if c.is_some() && now == Ok(cur) {
            return cur != used;
        }
        self.index(st, true);
        *c = self.open_shard(st, shard);
        true
    }

    /// Fetches the tiles of `tiles` (zoom `z`) not here yet, `threads` at a time: a download mostly
    /// waits on AWS, so far more of them than cores. The number that came from AWS.
    pub fn prefetch(&self, z: u8, tiles: &[(u32, u32)], threads: usize) -> anyhow::Result<usize> {
        self.prefetch_counted(z, tiles, threads, &std::sync::atomic::AtomicU64::new(0))
    }

    /// `prefetch`, counting into `done` each tile as it's here (at once for those that were).
    pub fn prefetch_counted(
        &self,
        z: u8,
        tiles: &[(u32, u32)],
        threads: usize,
        done: &std::sync::atomic::AtomicU64,
    ) -> anyhow::Result<usize> {
        use rayon::prelude::*;
        let todo: Vec<(u32, u32)> = tiles
            .iter()
            .copied()
            .filter(|&(x, y)| {
                let d = self.dir.join(format!("{z}/{x}"));
                !d.join(format!("{y}.png")).exists()
                    && !d.join(format!("{y}.none")).exists()
                    && !self.in_archive(z, x, y)
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
        match fetch_checked(&self.url, z, x, y)? {
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
fn nas_whole(p: &store::nasfs::NasPath) -> Option<Vec<u8>> {
    let b = p.read().ok()?;
    if crate::whole::png_whole(&b) {
        return Some(b);
    }
    eprintln!("terrain: {} isn't whole ({} bytes): fetched again", p.display(), b.len());
    p.remove_file().ok();
    None
}

/// One of AWS's tiles, whole: None when AWS says it has none (404, or S3's 403) twice, a moment
/// apart (it's remembered for good); an error when it can't be fetched or isn't a whole PNG (so
/// a network failure isn't remembered as "no tile"). Fetched by crate::fetch::get: its retries,
/// its wait for this Mac's agent through a restart.
fn fetch_checked(url: &str, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let url = tile_url(url, z, x, y);
    let got = match crate::fetch::get(&url)? {
        None => {
            std::thread::sleep(Duration::from_secs(2));
            crate::fetch::get(&url)?
        }
        got => got,
    };
    match got {
        Some(b) if !crate::whole::png_whole(&b) => anyhow::bail!("{url}: not a whole PNG ({} bytes)", b.len()),
        got => Ok(got),
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct PackReport {
    pub hi_tiles: usize,
    pub lo_tiles: usize,
    pub fetched: usize,
    pub missing: usize,
    pub changed: usize,
}

/// What a terrain z6 tile (a z6 tile's run: `build_piece`) keeps for its z3 tile's assembly
/// (`build_lo`), its mid: its z9 tiles' 2×2 means (128 × 128, f32: each a quarter of the z8 tile
/// above), and its lakes' levels as its own levels gave them (crate::terrain_water). Its z8–z6
/// can't be made with it: a lake's level at z8 is gathered from the whole z3 tile's z8 tiles, and the
/// z3 tile's levels start from every z6 tile's lakes, the first z6 tile's (in column, then row order)
/// kept for a lake two of them have.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mid {
    pub quads: std::collections::BTreeMap<(u32, u32), Vec<f32>>,
    pub levels: std::collections::BTreeMap<u64, f32>,
}

/// Z6 tile (`x`, `y`)'s terrain mid in the manifest (`Mid`).
pub fn mid_logical(x: u32, y: u32) -> String {
    crate::layers::mid_logical("terrain", x, y)
}

/// The mid's format (2: any tile's, its quarters three zooms down; heights before rounding).
const MID_FMT: u64 = 2;

/// Writes z6 tile `t`'s mid to `path` (`write_mid_of`).
pub fn write_mid(path: &std::path::Path, t: (u32, u32), mid: &Mid) -> anyhow::Result<()> {
    write_mid_of(path, (6, t.0, t.1), mid)
}

/// Writes tile `t`'s mid (a z6 tile's for its assembly, a z10 tile's deep levels' for its z6
/// tile: crate::terrain_deep) to `path`: a sectioned file (store::sect), meta `{"fmt": 2, "step":
/// "terrain", "tile": "z/x/y", "v": TERRAIN_V}`; a section `quad-<z+3>-<x>-<y>` for each of its
/// tiles three zooms down with its 2×2 means (f32, little-endian, row by row), by column then row,
/// then `lake-ids` (u64) and `lake-levels` (f32), by id.
pub fn write_mid_of(path: &std::path::Path, t: (u8, u32, u32), mid: &Mid) -> anyhow::Result<()> {
    let (tz, tx, ty) = t;
    let meta = serde_json::json!({ "fmt": MID_FMT, "step": "terrain", "tile": format!("{tz}/{tx}/{ty}"), "v": crate::agent::build::TERRAIN_V });
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut w = store::sect::SectWriter::create(path, meta)?;
    for (&(x, y), q) in &mid.quads {
        anyhow::ensure!(
            (x >> 3, y >> 3) == (tx, ty) && q.len() == 128 * 128,
            "{}/{x}/{y}'s means aren't {tz}/{tx}/{ty}'s",
            tz + 3
        );
        w.add(&format!("quad-{}-{x}-{y}", tz + 3), &q.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())?;
    }
    w.add("lake-ids", &mid.levels.keys().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())?;
    w.add("lake-levels", &mid.levels.values().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())?;
    w.finish()?;
    Ok(())
}

/// A z6 tile's mid (`read_mid_of`).
pub fn read_mid<'a>(path: impl Into<store::nasfs::Loc<'a>>) -> anyhow::Result<((u32, u32), Mid)> {
    let (t, mid) = read_mid_of(path, 6)?;
    Ok(((t.1, t.2), mid))
}

/// A mid of a zoom `tz` tile (`write_mid_of`), read and checked: its format, step and version (one
/// made for another version of the terrain isn't one to take in), every section whole. Its tile and
/// the mid.
pub fn read_mid_of<'a>(path: impl Into<store::nasfs::Loc<'a>>, tz: u8) -> anyhow::Result<((u8, u32, u32), Mid)> {
    use anyhow::Context;
    let path = path.into();
    let what = || path.display();
    let r = store::sect::SectReader::open(path.open_plain().with_context(what)?).with_context(what)?;
    let m = r.meta();
    anyhow::ensure!(m["fmt"].as_u64() == Some(MID_FMT) && m["step"] == "terrain", "{}: not a terrain mid", what());
    anyhow::ensure!(
        m["v"].as_u64() == Some(crate::agent::build::TERRAIN_V as u64),
        "{}: a mid of terrain version {}, not {}",
        what(),
        m["v"],
        crate::agent::build::TERRAIN_V
    );
    let t = m["tile"]
        .as_str()
        .and_then(crate::legacy::TileId::parse)
        .filter(|u| u.z == tz)
        .with_context(|| format!("{}: no z{tz} tile", what()))?;
    let prefix = format!("quad-{}-", tz + 3);
    let mut mid = Mid::default();
    let (mut ids, mut levels): (Vec<u64>, Vec<f32>) = (Vec::new(), Vec::new());
    for s in r.sections() {
        let b = r.read(&s.name).with_context(what)?;
        match s.name.as_str() {
            "lake-ids" => ids = b.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect(),
            "lake-levels" => levels = b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
            n => {
                let xy: Vec<u32> = n
                    .strip_prefix(prefix.as_str())
                    .with_context(|| format!("{}: a section {n:?}", what()))?
                    .split('-')
                    .map(|v| v.parse::<u32>())
                    .collect::<Result<_, _>>()
                    .with_context(|| format!("{}: a section {n:?}", what()))?;
                anyhow::ensure!(
                    xy.len() == 2 && (xy[0] >> 3, xy[1] >> 3) == (t.x, t.y) && b.len() == 128 * 128 * 4,
                    "{}: a section {n:?}",
                    what()
                );
                mid.quads.insert(
                    (xy[0], xy[1]),
                    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
                );
            }
        }
    }
    anyhow::ensure!(ids.len() == levels.len(), "{}: {} lakes and {} levels", what(), ids.len(), levels.len());
    mid.levels = ids.into_iter().zip(levels).collect();
    Ok(((tz, t.x, t.y), mid))
}

/// A tile as made: zoom, column, row, its bytes.
pub type Made = (u8, u32, u32, Vec<u8>);
/// A level as made: its tiles and their 2×2 means.
type Level = (Vec<(u32, u32, Vec<u8>)>, HashMap<(u32, u32), Vec<f32>>);

/// What makes the terrain's levels: the raw tiles, the sources, and the counts of what it did (each
/// tile counts once here and once shaded: `done`).
struct Maker<'a> {
    raw: &'a dyn RawSource,
    src: &'a Sources<'a>,
    here: std::sync::atomic::AtomicU64,
    processed: std::sync::atomic::AtomicU64,
    fetched: std::sync::atomic::AtomicUsize,
    missing: std::sync::atomic::AtomicUsize,
    changed: std::sync::atomic::AtomicUsize,
    /// A block's levels up (`blocks`): BLOCK_UP, fewer in tests to have more blocks.
    block_up: u8,
}

impl<'a> Maker<'a> {
    fn new(raw: &'a dyn RawSource, src: &'a Sources<'a>) -> Self {
        Maker {
            raw,
            src,
            here: Default::default(),
            processed: Default::default(),
            fetched: Default::default(),
            missing: Default::default(),
            changed: Default::default(),
            block_up: BLOCK_UP,
        }
    }

    /// The tiles fetched (or reused) and shaded, each counted once.
    fn done(&self) -> u64 {
        (self.here.load(std::sync::atomic::Ordering::Relaxed)
            + self.processed.load(std::sync::atomic::Ordering::Relaxed))
            / 2
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
    ///
    /// `by_block`: its tiles prepared a block at a time (`blocks`: the tiles under one tile four
    /// levels up, a z8 subtree's at z12), not all at once. A tile whose lakes all have their levels
    /// once its block is prepared (known from finer levels, or a lake no other block's tile has:
    /// `lakes_by_block`) is finished then and its preparation let go; the others wait for the
    /// level's last block, their lakes' levels from all their shore. The same bytes as all at once:
    /// a lake's level is of its samples whatever their order (crate::terrain_water::level), and a
    /// tile's finishing reads only its own lakes' levels.
    fn level(
        &self,
        z: u8,
        tiles: Vec<(u32, u32)>,
        quads: &HashMap<(u32, u32), Vec<f32>>,
        levels: &mut HashMap<u64, f32>,
        by_block: bool,
        mut log: Option<&mut crate::terrain_resume::HiLog>,
    ) -> anyhow::Result<Level> {
        use crate::terrain_resume::Kept;
        use crate::timings::sub;
        let up = z.min(self.block_up);
        let blocks = if by_block { blocks(z, &tiles, self.block_up) } else { vec![tiles.clone()] };
        // (What a resumed run's log kept of the level: its blocks, its end.)
        let mut kept = log.as_deref_mut().map(|l| l.level(z)).unwrap_or_default();
        // (A level of no tile, made whole, is one empty block.)
        let key = |b: &[(u32, u32)]| b.first().map_or((0, 0), |t| (t.0 >> up, t.1 >> up));
        let (mut outs, mut nq) = (Vec::new(), HashMap::new());
        // (A block kept: its tiles as finished, its lakes' levels.)
        let take = |k: &Kept,
                    outs: &mut Vec<(u32, u32, Vec<u8>)>,
                    nq: &mut HashMap<(u32, u32), Vec<f32>>,
                    levels: &mut HashMap<u64, f32>| {
            let n = k.done.len() as u64;
            self.here.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            self.processed.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            levels.extend(k.lakes.iter().copied());
            self.finished_into(k.done.clone(), outs, nq);
        };
        if let Some(end) = &kept.end {
            for k in kept.blocks.values().chain([end]) {
                take(k, &mut outs, &mut nq, levels);
            }
            return Ok((outs, nq));
        }
        let todo: Vec<usize> = (0..blocks.len()).filter(|&bi| !kept.blocks.contains_key(&key(&blocks[bi]))).collect();
        // (What this level starts from: its children's means, the tiles built under it.)
        let mut under: Vec<(u32, u32)> = quads.keys().copied().collect();
        under.sort_unstable();
        self.raw.built(z + 1, &under);
        let p = sub("sampling the terrain pool into raw tiles");
        // (Nothing kept: the level's tiles in their order, as a run without a log reads them.)
        let want: Vec<(u32, u32)> = if todo.len() == blocks.len() {
            tiles
        } else {
            todo.iter()
                .flat_map(|&bi| blocks[bi].iter().copied())
                .chain(kept.blocks.values().flat_map(|k| k.waiting.iter().copied()))
                .collect()
        };
        self.fetched.fetch_add(
            self.raw.prefetch_counted(z, &want, FETCH_THREADS, &self.here)?,
            std::sync::atomic::Ordering::Relaxed,
        );
        drop(p);
        // (Each lake's one block, None for one in two or more; without the water's lakes read, no
        // lake is one block's: every tile with a lake not yet levelled waits for the last block.)
        let one_block: Option<HashMap<u64, Option<usize>>> = match (blocks.len() > 1, self.src.water) {
            (true, Some(w)) => {
                let _p = sub("finding the level's lakes, block by block");
                lakes_by_block(z, &blocks, w)
            }
            _ => None,
        };
        let src = self.src;
        let mut waiting: Vec<(u32, u32, Prepared)> = Vec::new();
        let mut shared: HashMap<u64, crate::terrain_water::LakeSamples> = HashMap::new();
        let prep = |block: &[(u32, u32)]| -> anyhow::Result<Vec<(u32, u32, Prepared)>> {
            let prepared: Vec<anyhow::Result<Option<(u32, u32, Prepared)>>> = block
                .par_iter()
                .map(|&(x, y)| {
                    let raw = self.get(z, x, y)?;
                    let kids = (0..4u32).any(|k| quads.contains_key(&(x * 2 + (k & 1), y * 2 + (k >> 1))));
                    let p = prepare(raw.filter(|_| kids || self.raw.keep(z, x, y)), z, x, y, quads, src)?;
                    if !p.decoded {
                        self.processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return Ok(None);
                    }
                    Ok(Some((x, y, p)))
                })
                .collect();
            prepared.into_iter().filter_map(|r| r.transpose()).collect()
        };
        let ob = &one_block;
        let own_of = |bi: usize| move |id: &u64| ob.as_ref().is_some_and(|m| m.get(id) == Some(&Some(bi)));
        // (The blocks kept taken, their waiting tiles prepared again: their samples of the lakes
        // shared with other blocks, which the level's end reads; those of their own lakes are in
        // the levels kept.)
        let p = sub("preparing the tiles with the water");
        for (bi, b) in blocks.iter().enumerate() {
            let Some(k) = kept.blocks.remove(&key(b)) else { continue };
            take(&k, &mut outs, &mut nq, levels);
            let own = own_of(bi);
            for (x, y, p) in prep(&k.waiting)? {
                let rest: HashMap<_, _> = p.lake_samples().into_iter().filter(|(id, _)| !own(id)).collect();
                crate::terrain_water::gather(&mut shared, rest);
                waiting.push((x, y, p));
            }
        }
        let mut next = todo.first().map(|&bi| prep(&blocks[bi])).transpose()?.unwrap_or_default();
        drop(p);
        // (The levels a block or the end works out, as kept.)
        let added = |levels: &HashMap<u64, f32>, ids: Vec<u64>| -> Vec<(u64, f32)> {
            ids.into_iter().filter_map(|id| levels.get(&id).map(|&l| (id, l))).collect()
        };
        let new_ids = |levels: &HashMap<u64, f32>, all: &HashMap<u64, crate::terrain_water::LakeSamples>| -> Vec<u64> {
            let mut v: Vec<u64> = all.keys().filter(|id| !levels.contains_key(id)).copied().collect();
            v.sort_unstable();
            v
        };
        for (k, &bi) in todo.iter().enumerate() {
            let prepared = std::mem::take(&mut next);
            // (The block's own lakes levelled now; the others' samples kept for the level's end.)
            let own = own_of(bi);
            let mut mine = HashMap::new();
            for (_, _, p) in &prepared {
                let (a, b): (HashMap<_, _>, HashMap<_, _>) = p.lake_samples().into_iter().partition(|(id, _)| own(id));
                crate::terrain_water::gather(&mut mine, a);
                crate::terrain_water::gather(&mut shared, b);
            }
            let ids = new_ids(levels, &mine);
            crate::terrain_water::add_levels(levels, &mine);
            let (now, later): (Vec<_>, Vec<_>) = prepared.into_iter().partition(|(_, _, p)| {
                p.water.as_ref().is_none_or(|w| w.ids.iter().all(|id| levels.contains_key(id) || own(id)))
            });
            let later_at: Vec<(u32, u32)> = later.iter().map(|&(x, y, _)| (x, y)).collect();
            waiting.extend(later);
            // (This block's tiles finished while the next is prepared: the threads one leaves idle
            // at its end take the other's, so no block waits on its slowest tile. Two blocks'
            // tiles held at most, besides those waiting.)
            let lv: &HashMap<u64, f32> = levels;
            let p = sub("preparing tiles, levelling, shading and encoding the block before them");
            let (done, n) =
                rayon::join(|| self.finish_all(now, lv), || todo.get(k + 1).map(|&b| prep(&blocks[b])).transpose());
            drop(p);
            next = n?.unwrap_or_default();
            let done = match log.as_deref_mut() {
                Some(l) => {
                    let kb = Kept { done, lakes: added(levels, ids), waiting: later_at };
                    l.block(z, key(&blocks[bi]), &kb)?;
                    kb.done
                }
                None => done,
            };
            self.finished_into(done, &mut outs, &mut nq);
        }
        let _p = sub("levelling the tiles' lakes, shading and encoding them");
        let ids = new_ids(levels, &shared);
        crate::terrain_water::add_levels(levels, &shared);
        let done = self.finish_all(waiting, levels);
        let done = match log {
            Some(l) => {
                let kb = Kept { done, lakes: added(levels, ids), waiting: Vec::new() };
                l.end(z, &kb)?;
                kb.done
            }
            None => done,
        };
        self.finished_into(done, &mut outs, &mut nq);
        Ok((outs, nq))
    }

    /// Prepared tiles finished (`finish`): each its tile, whether it changed, its 2×2 means.
    #[allow(clippy::type_complexity)]
    fn finish_all(
        &self,
        mut prepared: Vec<(u32, u32, Prepared)>,
        levels: &HashMap<u64, f32>,
    ) -> Vec<(u32, u32, Option<Vec<u8>>, bool, Option<Vec<f32>>)> {
        prepared
            .par_drain(..)
            .map(|(x, y, p)| {
                let (b, r, q) = finish(p, levels);
                self.processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                (x, y, b, r, q)
            })
            .collect()
    }

    /// Finished tiles (`finish_all`) into a level's tiles and 2×2 means, the changed ones counted.
    #[allow(clippy::type_complexity)]
    fn finished_into(
        &self,
        done: Vec<(u32, u32, Option<Vec<u8>>, bool, Option<Vec<f32>>)>,
        outs: &mut Vec<(u32, u32, Vec<u8>)>,
        nq: &mut HashMap<(u32, u32), Vec<f32>>,
    ) {
        for (x, y, b, changed, q) in done {
            if changed {
                self.changed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(q) = q {
                nq.insert((x, y), q);
            }
            if let Some(b) = b {
                outs.push((x, y, b));
            }
        }
    }

    /// A z6 tile (z6 tile `t`): its levels z12 → z9 (`levels`, `piece_levels`), its lakes' levels
    /// from them alone, each level a block at a time (`by_block`, `level`; else all at once: the
    /// way before task #447, kept to check the bytes against). Its hi tiles, sorted, and its mid.
    fn piece(
        &self,
        levels: Vec<(u8, Vec<(u32, u32)>)>,
        by_block: bool,
        mut log: Option<&mut crate::terrain_resume::HiLog>,
        start: Mid,
    ) -> anyhow::Result<(Vec<Made>, Mid)> {
        let mut quads: HashMap<(u32, u32), Vec<f32>> = start.quads.into_iter().collect();
        let mut hi: Vec<Made> = Vec::new();
        let mut lakes: HashMap<u64, f32> = start.levels.into_iter().collect();
        for (z, tiles) in levels {
            let (outs, nq) = self.level(z, tiles, &quads, &mut lakes, by_block, log.as_deref_mut())?;
            hi.extend(outs.into_iter().map(|(x, y, b)| (z, x, y, b)));
            quads = nq;
        }
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        Ok((hi, Mid { quads: quads.into_iter().collect(), levels: lakes.into_iter().collect() }))
    }

    /// A z3 tile's assembly (z3 tile `q`): its levels z8 → z3, the whole of it, from its z6 tiles' mids
    /// (`mids`, in column then row order: their z9 tiles' means taken in, each lake's level the
    /// first of them that has one, the levels' lakes added below them). Its lo tiles, sorted.
    ///
    fn lo(&self, q: (u32, u32), mids: &[((u32, u32), &Mid)]) -> anyhow::Result<Vec<Made>> {
        let mut quads: HashMap<(u32, u32), Vec<f32>> = HashMap::new();
        let mut lakes: HashMap<u64, f32> = HashMap::new();
        for (_, m) in mids {
            quads.extend(m.quads.iter().map(|(k, v)| (*k, v.clone())));
            for (k, v) in &m.levels {
                lakes.entry(*k).or_insert(*v);
            }
        }
        let mut lo: Vec<Made> = Vec::new();
        for (z, tiles) in upper_levels(q) {
            let (outs, nq) = self.level(z, tiles, &quads, &mut lakes, false, None)?;
            lo.extend(outs.into_iter().map(|(x, y, b)| (z, x, y, b)));
            quads = nq;
        }
        lo.sort_by_key(|t| (t.0, t.1, t.2));
        Ok(lo)
    }

    fn report(&self, rep: &mut PackReport) {
        rep.fetched = self.fetched.load(std::sync::atomic::Ordering::Relaxed);
        rep.missing = self.missing.load(std::sync::atomic::Ordering::Relaxed);
        rep.changed = self.changed.load(std::sync::atomic::Ordering::Relaxed);
    }
}

/// A z6 tile's levels `levels` (z12 → z9, `piece_levels`, or a task's part of them:
/// crate::terrain_task) made from `raw` and `src`, its lakes' levels from them alone: its hi tiles,
/// sorted, and its mid.
pub fn make_piece(
    raw: &dyn RawSource,
    src: &Sources,
    levels: Vec<(u8, Vec<(u32, u32)>)>,
) -> anyhow::Result<(Vec<Made>, Mid)> {
    Maker::new(raw, src).piece(levels, true, None, Mid::default())
}

/// A z6 tile's levels `levels` made from `raw` and `src` as its run makes them, its z10 tiles'
/// deep mids `deep` taken in (its z12 tiles' quarters there, its lakes' levels first): by block
/// (`by_block`), else each level's tiles all prepared at once (`make_piece_whole`'s way). Its hi
/// tiles, sorted, and its mid (`scenic-build terrain-same`).
pub fn make_piece_from(
    raw: &dyn RawSource,
    src: &Sources,
    levels: Vec<(u8, Vec<(u32, u32)>)>,
    deep: Mid,
    by_block: bool,
) -> anyhow::Result<(Vec<Made>, Mid)> {
    Maker::new(raw, src).piece(levels, by_block, None, deep)
}

/// `make_piece` with its blocks kept as made and an earlier run's taken (`log`): a z10 tile's band
/// (crate::terrain_deep).
pub fn make_piece_logged(
    raw: &dyn RawSource,
    src: &Sources,
    levels: Vec<(u8, Vec<(u32, u32)>)>,
    log: Option<&mut crate::terrain_resume::HiLog>,
) -> anyhow::Result<(Vec<Made>, Mid)> {
    Maker::new(raw, src).piece(levels, true, log, Mid::default())
}

/// `make_piece` with each level's tiles all prepared at once (the way before task #447, which held
/// a z6 tile's every z12 tile prepared: 7 GB at the 90th percentile): the same bytes, for checking
/// them (`scenic-build terrain-same --whole`, tests).
pub fn make_piece_whole(
    raw: &dyn RawSource,
    src: &Sources,
    levels: Vec<(u8, Vec<(u32, u32)>)>,
) -> anyhow::Result<(Vec<Made>, Mid)> {
    Maker::new(raw, src).piece(levels, false, None, Mid::default())
}

/// The most memory (MB) a z6 tile's levels take as one run makes them (`make_piece`), from its
/// bounds: its hi tiles held until written (z9–12, up to 5,440, ~270 KB each made), a level's tiles
/// while the next coarser is made (up to 4,096 at 256 KB), its quarters (~20 MB), the pool's chunks
/// being sampled (crate::terrain_base::sampling_mb) and the samplers' caches
/// (crate::dem::generic::samplers_mb).
pub const fn piece_peak_mb() -> u64 {
    5440 * 270 / 1024 + 4096 / 4 + 20 + crate::terrain_base::sampling_mb() + crate::dem::generic::samplers_mb()
}

/// The most memory (MB) a z6 tile's run holds of its z10 tiles' deep mids (crate::terrain_deep): a
/// 128 × 128 f32 quarter for each of its z13 tiles at most (16,384, its z12 tiles' quarters).
pub const DEEP_MIDS_MB: u64 = (1u64 << (2 * (crate::terrain_deep::FIRST_Z - 6))) * 128 * 128 * 4 / (1 << 20);

/// How many levels up a block's tile is (`blocks`): a z8 subtree's tiles at z12, 256 at most.
const BLOCK_UP: u8 = 4;

/// `make_piece` with blocks `up` levels up (`blocks`), for tests: more blocks on a small z6 tile.
#[cfg(test)]
pub fn make_piece_up(
    raw: &dyn RawSource,
    src: &Sources,
    levels: Vec<(u8, Vec<(u32, u32)>)>,
    up: u8,
) -> anyhow::Result<(Vec<Made>, Mid)> {
    let mut m = Maker::new(raw, src);
    m.block_up = up;
    m.piece(levels, true, None, Mid::default())
}

/// `build_piece` keeping its blocks (crate::terrain_resume::HiLog), for tests: none offered out.
#[cfg(test)]
pub fn build_piece_keeping(
    out: &mut Out,
    raw: &dyn RawSource,
    t: (u32, u32),
    cov: &Coverage,
    src: &Sources,
) -> anyhow::Result<PackReport> {
    let offers = crate::terrain_task::Offers::offer(None, t, &piece_levels(cov, src.sets, t), raw, src);
    let rep = build_piece_kept(out, raw, t, cov, src, false, &|_, _, _| {}, offers, Mid::default(), true)?;
    out.save()?;
    Ok(rep)
}

/// A level's tiles (zoom `z`) in blocks, each those under one tile `up` levels up (a z8 subtree's
/// at z12, at most 256), in the order of their first tile; the tiles in each in their order.
fn blocks(z: u8, tiles: &[(u32, u32)], up: u8) -> Vec<Vec<(u32, u32)>> {
    let up = z.min(up);
    let mut at: HashMap<(u32, u32), usize> = HashMap::new();
    let mut out: Vec<Vec<(u32, u32)>> = Vec::new();
    for &(x, y) in tiles {
        let i = *at.entry((x >> up, y >> up)).or_insert_with(|| {
            out.push(Vec::new());
            out.len() - 1
        });
        out[i].push((x, y));
    }
    out
}

/// Each lake key (crate::terrain_water::WaterTile's `ids`: those of the tile's lake polygons) of
/// the tiles of `blocks` (zoom `z`), with the one block whose tiles have it, or None when two or
/// more blocks' have it; None when a tile's water can't be read (then no lake is one block's).
fn lakes_by_block(
    z: u8,
    blocks: &[Vec<(u32, u32)>],
    water: &dyn crate::terrain_water::WaterSource,
) -> Option<HashMap<u64, Option<usize>>> {
    let tiles: Vec<(usize, u32, u32)> =
        blocks.iter().enumerate().flat_map(|(i, b)| b.iter().map(move |&(x, y)| (i, x, y))).collect();
    let ids: Vec<Option<Vec<u64>>> = tiles
        .par_iter()
        .map(|&(_, x, y)| {
            water
                .polys(z, x, y)
                .ok()
                .map(|ps| ps.into_iter().filter(|p| p.kind == crate::terrain_water::Kind::Lake).map(|p| p.id).collect())
        })
        .collect();
    let mut by: HashMap<u64, Option<usize>> = HashMap::new();
    for (&(b, ..), ids) in tiles.iter().zip(ids) {
        for id in ids? {
            let e = by.entry(id).or_insert(Some(b));
            if *e != Some(b) {
                *e = None;
            }
        }
    }
    Some(by)
}

/// A z6 tile's levels (z6 tile `t`'s), z12 → z9: the tiles near the coverage (20 km) no finer than
/// their place's finest zoom (crate::terrain_zoom::finest, of `sets`): those built (`Fill::Child`).
pub fn piece_levels(cov: &Coverage, sets: &[crate::terrain_zoom::Set], t: (u32, u32)) -> Vec<(u8, Vec<(u32, u32)>)> {
    let (tx, ty) = t;
    (9..=12u8)
        .rev()
        .map(|z| {
            let s = 1u32 << (z - 6);
            let tiles = (tx * s..(tx + 1) * s)
                .flat_map(|x| (ty * s..(ty + 1) * s).map(move |y| (x, y)))
                .filter(|&(x, y)| near_coverage(cov, z, x, y) && z <= crate::terrain_zoom::finest(sets, z, x, y))
                .collect();
            (z, tiles)
        })
        .collect()
}

/// Whether z6 tile `t` makes any hi tile (`piece_levels` not all empty): one near the coverage at
/// z6 can still have none of its z9 tiles near it (6/21/18), or none whose place reaches z9. Its z9
/// tiles near the coverage once per coverage version and z6 tile (`Coverage::kept`).
pub fn makes_hi(cov: &Coverage, sets: &[crate::terrain_zoom::Set], t: (u32, u32)) -> bool {
    let near = {
        let kept = cov.kept();
        let got = kept.makes_hi.lock().unwrap().get(&t).copied();
        got.unwrap_or_else(|| {
            let v = (t.0 * 8..t.0 * 8 + 8).any(|x| (t.1 * 8..t.1 * 8 + 8).any(|y| near_coverage(cov, 9, x, y)));
            kept.makes_hi.lock().unwrap().insert(t, v);
            v
        })
    };
    near && (t.0 * 8..t.0 * 8 + 8).any(|x| {
        (t.1 * 8..t.1 * 8 + 8).any(|y| crate::terrain_zoom::finest(sets, 9, x, y) >= 9 && near_coverage(cov, 9, x, y))
    })
}

/// Drops z6 tile `t`'s terrain hi pack and mid from the manifest (a z6 tile the coverage has left:
/// docs/regions.md, Shrinking), saving it; how many it had. (Its deep packs are their terrain-deep
/// targets': crate::terrain_deep.)
pub fn drop_piece(out: &mut Out, t: (u32, u32)) -> anyhow::Result<usize> {
    let gone = vec![crate::layers::hi_logical("terrain", t.0, t.1), mid_logical(t.0, t.1)];
    let n = crate::layers::drop(out, &gone);
    out.save()?;
    Ok(n)
}

/// Drops z3 tile `q`'s terrain lo pack from the manifest (a "none" assembly: the coverage has left
/// the z3 tile, docs/regions.md, Shrinking), saving it; how many it had.
pub fn drop_lo(out: &mut Out, q: (u32, u32)) -> anyhow::Result<usize> {
    let n = crate::layers::drop(out, &[crate::layers::lo_logical("terrain", q.0, q.1)]);
    out.save()?;
    Ok(n)
}

/// A z3 tile's levels (z3 tile `q`'s), z8 → z3, the whole of it.
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
fn saying<T>(
    mk: &Maker,
    total: u64,
    progress: crate::rawpack::Progress,
    run: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
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

/// Makes z6 tile `t`'s terrain (a z6 tile: its levels z12 → z9, `piece_levels`) and uploads it: its hi
/// pack (none when it made no tile: the manifest's then dropped, so no earlier run's hi tiles stay
/// above the z8–z6 its assembly makes from the raw tiles alone) and its mid (`Mid`, `mid_logical`),
/// which its z3 tile's assembly reads (`build_lo`). A z6 tile the coverage has left (not near it: no
/// z6 tile) has its hi pack and mid dropped instead (`drop_piece`). `expect_same`: a z6 tile made again
/// as it is (its mid made), whose hi pack must come out as the manifest has it (none where it has
/// none), else an error and nothing uploaded. Says how far it is (`saying`: its tiles, then "packs").
#[allow(clippy::too_many_arguments)]
pub fn build_piece(
    out: &mut Out,
    raw: &dyn RawSource,
    t: (u32, u32),
    cov: &Coverage,
    src: &Sources,
    expect_same: bool,
    progress: crate::rawpack::Progress,
) -> anyhow::Result<PackReport> {
    build_piece_with(out, raw, t, cov, src, expect_same, progress, None)
}

/// `build_piece`, some of its z8 subtrees offered to other workers through `offload` while workers
/// that take them are around (crate::terrain_task): the run makes the others, then takes theirs in
/// (or makes them too), the same bytes.
#[allow(clippy::too_many_arguments)]
pub fn build_piece_with(
    out: &mut Out,
    raw: &dyn RawSource,
    t: (u32, u32),
    cov: &Coverage,
    src: &Sources,
    expect_same: bool,
    progress: crate::rawpack::Progress,
    offload: Option<&crate::offload::Offload>,
) -> anyhow::Result<PackReport> {
    let offers = crate::terrain_task::Offers::offer(offload, t, &piece_levels(cov, src.sets, t), raw, src);
    build_piece_offered(out, raw, t, cov, src, expect_same, progress, offers)
}

/// `build_piece` with its subtrees out as `offers` says (made for this z6 tile: `Offers::offer`,
/// when it begins or, a job's next z6 tile, ahead: crate::terrain_task::offer_ahead).
#[allow(clippy::too_many_arguments)]
pub fn build_piece_offered(
    out: &mut Out,
    raw: &dyn RawSource,
    t: (u32, u32),
    cov: &Coverage,
    src: &Sources,
    expect_same: bool,
    progress: crate::rawpack::Progress,
    offers: crate::terrain_task::Offers,
) -> anyhow::Result<PackReport> {
    let rep = build_piece_later(out, raw, t, cov, src, expect_same, progress, offers, Mid::default())?;
    out.save()?;
    out.claim_into_costin();
    progress("packs", 1, 1);
    Ok(rep)
}

/// `build_piece_offered`, its uploads put off (crate::out::Out::put_file_later): they go on beside
/// what the job does next, and the z6 tile is saved (and noted done) by a later save
/// (`Out::save_upto` the mark after it). `deep`: its z10 tiles' deep mids (crate::terrain_deep::read_mids),
/// its z12 tiles' quarters there and its lakes' levels first; a z6 tile with some offers none of
/// its subtrees.
#[allow(clippy::too_many_arguments)]
pub fn build_piece_later(
    out: &mut Out,
    raw: &dyn RawSource,
    t: (u32, u32),
    cov: &Coverage,
    src: &Sources,
    expect_same: bool,
    progress: crate::rawpack::Progress,
    offers: crate::terrain_task::Offers,
    deep: Mid,
) -> anyhow::Result<PackReport> {
    build_piece_kept(out, raw, t, cov, src, expect_same, progress, offers, deep, crate::resume_log::on())
}

/// `build_piece_later`, its levels' blocks kept as made and those an earlier run kept taken with
/// `keep` (crate::terrain_resume::HiLog; its log deleted once its hi pack and mid are written).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_piece_kept(
    out: &mut Out,
    raw: &dyn RawSource,
    t: (u32, u32),
    cov: &Coverage,
    src: &Sources,
    expect_same: bool,
    progress: crate::rawpack::Progress,
    offers: crate::terrain_task::Offers,
    deep: Mid,
    keep: bool,
) -> anyhow::Result<PackReport> {
    anyhow::ensure!(
        offers.piece() == t,
        "offers for 6/{}/{}, not 6/{}/{}",
        offers.piece().0,
        offers.piece().1,
        t.0,
        t.1
    );
    let mut rep = PackReport::default();
    if !near_coverage(cov, 6, t.0, t.1) {
        anyhow::ensure!(!expect_same, "6/{}/{}: the coverage has left it, so it can't be made as it is", t.0, t.1);
        let n = drop_piece(out, t)?;
        eprintln!("terrain 6/{}/{}: the coverage has left it; {n} files dropped", t.0, t.1);
        return Ok(rep);
    }
    let levels = piece_levels(cov, src.sets, t);
    let total: u64 = levels.iter().map(|(_, t)| t.len() as u64).sum();
    let mk = Maker::new(raw, src);
    let p = crate::timings::phase("making the piece's terrain tiles, z12 down to z9");
    let (hi, mid) = saying(&mk, total, progress, || {
        let mine = offers.rest(&levels);
        anyhow::ensure!(
            deep.quads.is_empty() || mine == levels,
            "6/{}/{}: subtrees offered out of a z6 tile taking in deep mids",
            t.0,
            t.1
        );
        let n = mine.iter().map(|(_, t)| t.len()).sum();
        let mut log = keep
            .then(|| {
                let id = crate::terrain_resume::hi_id(&out.manifest, src, (6, t.0, t.1), &mine, mk.block_up);
                crate::terrain_resume::HiLog::open(&out.scratch, (6, t.0, t.1), &id)
            })
            .flatten();
        let began = std::time::Instant::now();
        let (mut hi, mut mid) = mk.piece(mine, true, log.as_mut(), deep)?;
        offers.made_here(began.elapsed().as_secs_f64(), n);
        let _w = crate::timings::sub("waiting for the subtrees other workers make (or making them here)");
        let _o = store::waits::other();
        offers.settle(&|lv| mk.piece(lv, true, None, Mid::default()), &mut hi, &mut mid)?;
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        Ok((hi, mid))
    })?;
    drop(p);
    mk.report(&mut rep);
    progress("packs", 0, 1);
    let p = crate::timings::phase("writing its hi pack and mid");
    rep.hi_tiles = hi.len();
    let mut it = hi.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let pack = crate::layers::write_pack_local(
        out,
        "terrain",
        crate::terrain_codec::ENCODING,
        false,
        "hi",
        (6, t.0, t.1),
        &mut it,
    )?;
    let ml = mid_logical(t.0, t.1);
    let mid_path = out.scratch_file(&format!("{ml}.sect"));
    write_mid(&mid_path, t, &mid)?;
    std::fs::remove_file(crate::resume_log::path(&out.scratch, (6, t.0, t.1))).ok();
    let hl = crate::layers::hi_logical("terrain", t.0, t.1);
    if expect_same {
        let made = pack.as_ref().map(|p| p.content_name()).transpose()?;
        let mut differ: Vec<String> = Vec::new();
        if out.get(&hl) != made.as_deref() {
            differ.push(format!(
                "{hl}: made {}, the manifest has {}",
                made.as_deref().unwrap_or("none"),
                out.get(&hl).unwrap_or("none")
            ));
        }
        if !differ.is_empty() {
            for p in pack.iter() {
                std::fs::remove_file(&p.local).ok();
            }
            std::fs::remove_file(&mid_path).ok();
            anyhow::bail!(
                "piece 6/{}/{} was expected the same as the manifest has it, and isn't (nothing uploaded): {}",
                t.0,
                t.1,
                differ.join("; ")
            );
        }
    }
    drop(p);
    match pack {
        Some(p) => out.put_file_later(&p.logical, "pack", &p.local)?,
        None => {
            crate::layers::drop(out, &[hl]);
        }
    }
    out.put_file_later(&ml, "sect", &mid_path)?;
    Ok(rep)
}

/// Makes z3 tile `q`'s zoomed-out terrain (an assembly: its levels z8 → z3, the whole of it) from
/// its z6 tiles' mids (`ts`: its z6 tiles near the coverage, in column then row order, each with its
/// mid in the manifest), and uploads its lo pack (none when it made no tile: the manifest's then
/// dropped). Says how far it is (`saying`: its tiles, then "packs").
pub fn build_lo(
    out: &mut Out,
    raw: &dyn RawSource,
    q: (u32, u32),
    ts: &[(u32, u32)],
    src: &Sources,
    progress: crate::rawpack::Progress,
) -> anyhow::Result<PackReport> {
    let (lo, mut rep) = make_lo(out, raw, q, ts, src, progress)?;
    let up = crate::timings::phase("uploading the lo pack");
    progress("packs", 0, 1);
    rep.lo_tiles = lo.len();
    let mut it = lo.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    if crate::layers::write_pack(out, "terrain", crate::terrain_codec::ENCODING, false, "lo", (3, q.0, q.1), &mut it)?
        .is_none()
    {
        crate::layers::drop(out, &[crate::layers::lo_logical("terrain", q.0, q.1)]);
    }
    drop(up);
    out.save()?;
    progress("packs", 1, 1);
    Ok(rep)
}

/// `build_lo`'s tiles, sorted, and what it did; nothing written (`scenic-build terrain-same 3/x/y`
/// too).
pub fn make_lo(
    out: &Out,
    raw: &dyn RawSource,
    q: (u32, u32),
    ts: &[(u32, u32)],
    src: &Sources,
    progress: crate::rawpack::Progress,
) -> anyhow::Result<(Vec<Made>, PackReport)> {
    use anyhow::Context;
    let mut rep = PackReport::default();
    let p = crate::timings::phase("reading the pieces' mids");
    let mut paths = Vec::with_capacity(ts.len());
    for &t in ts {
        anyhow::ensure!((t.0 >> 3, t.1 >> 3) == q, "6/{}/{} isn't in 3/{}/{}", t.0, t.1, q.0, q.1);
        let c = out
            .get(&mid_logical(t.0, t.1))
            .with_context(|| format!("6/{}/{} has no mid yet: its z3 tile can't be assembled", t.0, t.1))?;
        paths.push((t, c.to_string(), out.path(c)));
    }
    p.count(0, paths.len() as u64);
    // (Kept in `ts`'s order, which `lo` takes the lakes' levels by.)
    let mids: Vec<((u32, u32), Mid)> = crate::layers::read_each(paths.len(), MID_READERS, |i| {
        let (t, c, path) = &paths[i];
        let (of, mid) = read_mid(path)?;
        anyhow::ensure!(of == *t, "{c} is 6/{}/{}'s mid", of.0, of.1);
        Ok((*t, mid))
    })?;
    drop(p);
    let mk = Maker::new(raw, src);
    let refs: Vec<((u32, u32), &Mid)> = mids.iter().map(|(t, m)| (*t, m)).collect();
    let p = crate::timings::phase("making the z3 tile's coarser levels, z8 down to z3");
    let lo = saying(&mk, 1365, progress, || mk.lo(q, &refs))?;
    drop(p);
    mk.report(&mut rep);
    Ok((lo, rep))
}

/// Z3 tile `q`'s lo tiles (z8 → z3), sorted, from its z6 tiles' mids `mids` (column, then row
/// order) and the tiles `raw` gives by the rule: what an assembly makes (`make_lo`), its mids given.
pub fn make_lo_from(
    raw: &dyn RawSource,
    src: &Sources,
    q: (u32, u32),
    mids: &[((u32, u32), &Mid)],
) -> anyhow::Result<Vec<Made>> {
    Maker::new(raw, src).lo(q, mids)
}

/// The root pack (z0–2) made from the 64 z3 tiles as stored in the lo packs (`make_root`) and put;
/// its tiles.
pub fn build_root(out: &mut Out, raw: &dyn RawSource, src: &Sources) -> anyhow::Result<usize> {
    let tiles = make_root(out, raw, src)?;
    let up = crate::timings::phase("uploading the root pack");
    let n = tiles.len();
    let mut it = tiles.into_iter().map(|(z, x, y, b)| {
        let l = b.len() as u32;
        (z, x, y, b, l)
    });
    crate::layers::write_pack(out, "terrain", crate::terrain_codec::ENCODING, false, "root", (0, 0, 0), &mut it)?;
    drop(up);
    out.save()?;
    Ok(n)
}

/// The root's tiles (z0–2), sorted, from the 64 z3 tiles as stored in the lo packs (their 2×2
/// means) by the rule (docs/steps.md §2): a quarter whose z3 tile isn't in them lies outside every
/// region and takes the relief (`raw`: `Fills`, its one tile a root tile), never the pool;
/// deterministic given the lo packs and the relief. Nothing written.
pub fn make_root(out: &Out, raw: &dyn RawSource, src: &Sources) -> anyhow::Result<Vec<Made>> {
    let p = crate::timings::phase("reading the z3 tiles from the lo packs");
    let have = ManifestTiles::new(out, "terrain");
    let mut quads: HashMap<(u32, u32), Vec<f32>> = HashMap::new();
    for x in 0..8u32 {
        for y in 0..8u32 {
            let Some(b) = have.get(3, x, y)? else { continue };
            let Ok(e) = decode_terrain(&b) else { continue };
            quads.insert((x, y), quad_mean(&e));
        }
    }
    drop(have);
    drop(p);
    let _p = crate::timings::phase("making the top tiles (z0–2) from the z3 tiles and the relief");
    let mk = Maker::new(raw, src);
    let mut lakes: HashMap<u64, f32> = HashMap::new();
    let mut tiles: Vec<Made> = Vec::new();
    for z in (0..=2u8).rev() {
        let n = 1u32 << z;
        let level: Vec<(u32, u32)> = (0..n).flat_map(|x| (0..n).map(move |y| (x, y))).collect();
        let (outs, nq) = mk.level(z, level, &quads, &mut lakes, false, None)?;
        tiles.extend(outs.into_iter().map(|(x, y, b)| (z, x, y, b)));
        quads = nq;
    }
    tiles.sort_by_key(|t| (t.0, t.1, t.2));
    Ok(tiles)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Raw tiles `inner`'s, sampled as crate::terrain_base::Tiles samples the pool: `CHUNK` tiles a
    /// call of `prefetch_counted`, or a tile read not prefetched; once `left` of those have run, the
    /// next finds the build pausing (crate::control::Paused, task #705).
    pub(crate) struct PauseAfter<'a> {
        inner: &'a dyn RawSource,
        left: std::sync::atomic::AtomicUsize,
        held: std::sync::Mutex<std::collections::HashSet<(u8, u32, u32)>>,
    }

    /// Tiles sampled in one call (crate::terrain_base's).
    const CHUNK: usize = 16;

    impl<'a> PauseAfter<'a> {
        pub(crate) fn new(inner: &'a dyn RawSource, left: usize) -> Self {
            PauseAfter { inner, left: left.into(), held: Default::default() }
        }

        /// One sampling, or the build pausing.
        fn sampling(&self) -> anyhow::Result<()> {
            let ord = std::sync::atomic::Ordering::Relaxed;
            let went = self.left.fetch_update(ord, ord, |n| n.checked_sub(1)).is_ok();
            crate::control::pause_point(&|| !went)
        }
    }

    impl RawSource for PauseAfter<'_> {
        fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
            if !self.held.lock().unwrap().remove(&(z, x, y)) {
                self.sampling()?;
            }
            self.inner.get(z, x, y)
        }
        fn prefetch_counted(
            &self,
            z: u8,
            tiles: &[(u32, u32)],
            threads: usize,
            done: &std::sync::atomic::AtomicU64,
        ) -> anyhow::Result<usize> {
            let mut n = 0;
            for c in tiles.chunks(CHUNK) {
                self.sampling()?;
                n += self.inner.prefetch_counted(z, c, threads, done)?;
                self.held.lock().unwrap().extend(c.iter().map(|&(x, y)| (z, x, y)));
            }
            Ok(n)
        }
    }

    /// The files under `d` (none when it isn't there).
    pub(crate) fn files_under(d: &std::path::Path) -> Vec<std::path::PathBuf> {
        let Ok(rd) = std::fs::read_dir(d) else { return Vec::new() };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    /// A tile whose water can't be read fails (6/18/22 on 25 Oct: every tile's water read failed
    /// while the NAS restarted, the tiles were made dry and kept under a key naming the water).
    #[test]
    fn a_tile_whose_water_cant_be_read_fails() {
        struct Down;
        impl crate::terrain_water::WaterSource for Down {
            fn polys(&self, _: u8, _: u32, _: u32) -> anyhow::Result<Vec<crate::terrain_water::Poly>> {
                anyhow::bail!("Input/output error (os error 5)")
            }
            fn pin(&self) -> String {
                "down".into()
            }
        }
        let raw = roadcore::grid::encode_terrain_png(&vec![10.0; 256 * 256], 256, 256).unwrap();
        let src = Sources { water: Some(&Down), ..Default::default() };
        let err = prepare(Some(raw.clone()), 12, 1, 2, &HashMap::new(), &src).err().expect("an error");
        assert!(format!("{err:#}").contains("the water of 12/1/2"), "{err:#}");
        assert!(process(Some(raw), 12, 1, 2, &HashMap::new(), &src).is_err());
    }

    /// A pack copied here reads the same tiles as the NAS's, and the copy goes with the reader: a
    /// pack it isn't asked to copy, or none there, is read where it is.
    #[test]
    fn a_pack_copied_here_reads_the_same_and_goes_with_its_reader() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
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
        crate::layers::write_pack(
            &mut out,
            "terrain",
            crate::terrain_codec::ENCODING,
            false,
            "hi",
            (6, 5, 7),
            &mut tiles.into_iter(),
        )
        .unwrap();
        let copies = d.path().join("copies");
        let plain = ManifestTiles::new(&out, "terrain");
        let here = ManifestTiles::with_copies(&out, "terrain", copies.clone());
        let read = |m: &ManifestTiles| want.iter().map(|t| m.get(t.0, t.1, t.2).unwrap()).collect::<Vec<_>>();
        let all: Vec<Option<Vec<u8>>> = want.iter().map(|t| Some(t.3.clone())).collect();
        assert_eq!(read(&plain), all);
        // Some read from the NAS's first, then the rest from the copy.
        assert_eq!(here.get(9, 40, 56).unwrap(), Some(b"tile 9/40/56".to_vec()));
        let n = here.copy_here(9, 40, 56).unwrap();
        assert_eq!(n, (out.path(out.get("layers/terrain/hi/6-5-7").unwrap())).metadata().unwrap().len());
        assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 1);
        assert_eq!(read(&here), all);
        // Copied once while kept; let go, its copy deleted and its reads the NAS's, then copied again.
        assert_eq!(here.copy_here(10, 85, 120), None, "the same pack, here already");
        here.keep_only(&|l| l == "layers/terrain/hi/6-5-7");
        assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 1, "kept");
        here.keep_only(&|_| false);
        assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 0, "let go");
        assert_eq!(read(&here), all);
        assert_eq!(here.copy_here(9, 40, 56), Some(n));
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

    /// A NAS call stuck past its deadline fails (`TimedOut`, not tried again); a transient failure
    /// is tried again after each wait; any other fails at once.
    #[test]
    fn nas_calls_have_a_deadline_and_retry_transient_failures() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let ms = std::time::Duration::from_millis;
        let t = std::time::Instant::now();
        let e = nas_call("stuck", ms(100), &[ms(1)], move || {
            std::thread::sleep(ms(30_000));
            Ok(())
        })
        .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
        store::testkit::ended_before(t, ms(30_000), "the stuck call ends at its deadline");
        let tries = std::sync::Arc::new(AtomicUsize::new(0));
        let n = tries.clone();
        let got =
            nas_call("flaky", ms(1000), &[ms(1), ms(1), ms(1)], move || match n.fetch_add(1, Ordering::Relaxed) {
                0 => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
                1 => Err(std::io::Error::from_raw_os_error(libc::EIO)),
                _ => Ok(7),
            });
        assert_eq!((got.unwrap(), tries.load(Ordering::Relaxed)), (7, 3));
        let tries = std::sync::Arc::new(AtomicUsize::new(0));
        let n = tries.clone();
        let e = nas_call("bad", ms(1000), &[ms(1), ms(1)], move || -> std::io::Result<()> {
            n.fetch_add(1, Ordering::Relaxed);
            Err(std::io::Error::from(std::io::ErrorKind::InvalidData))
        })
        .unwrap_err();
        assert_eq!((e.kind(), tries.load(Ordering::Relaxed)), (std::io::ErrorKind::InvalidData, 1));
        // Transient every time: the last error, after every wait.
        let e = nas_call("gone", ms(1000), &[ms(1), ms(1)], || -> std::io::Result<()> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
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
        let raw = RawTiles::with_store(
            &local,
            &store::nasfs::NasPath::local_for_test(&store),
            crate::terrain_pack::TEST_BASE,
        );
        let (b, fetched) = raw.get(9, 5, 7).unwrap();
        assert_eq!((b.as_deref(), fetched), (Some(&png[..]), false));
        assert_eq!(std::fs::read(local.join("9/5/7.png")).unwrap(), png, "copied here");
        assert_eq!(raw.get(9, 5, 8).unwrap(), (None, false));
        assert!(local.join("9/5/8.none").exists());
        assert!(read_whole(&store.join("9/5/9.png")).is_none() && !store.join("9/5/9.png").exists());
    }

    #[test]
    fn raw_tiles_from_their_shards_archive() {
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
            std::fs::File::options()
                .append(true)
                .open(other.join(rel))
                .unwrap()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
                .unwrap();
        }
        assert_eq!(
            crate::rawpack::pack_local(
                &other,
                &store::nasfs::NasPath::local_for_test(&store),
                &store::nasfs::NasRoot::local_for_test(&root),
                true
            )
            .unwrap(),
            2
        );
        // A fresh cache reads them from their shard's archive on the NAS, a tile at a time (no loose
        // file here or there, no copy of the archive yet)...
        let local = d.path().join("local");
        let raw = RawTiles::with_store(
            &local,
            &store::nasfs::NasPath::local_for_test(&store),
            crate::terrain_pack::TEST_BASE,
        );
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
        let again = RawTiles::with_store(
            &local,
            &store::nasfs::NasPath::local_for_test(&store),
            crate::terrain_pack::TEST_BASE,
        );
        assert_eq!(again.get(12, 2048, 1365).unwrap(), (Some(png.clone()), false));
        // A prefetch passes archived tiles over, reading none of them: however often, the shard's
        // archive isn't copied here (as reading its tiles that often would).
        let fresh = RawTiles::with_store(
            &d.path().join("fresh"),
            &store::nasfs::NasPath::local_for_test(&store),
            crate::terrain_pack::TEST_BASE,
        );
        for _ in 0..=RANGED / 2 {
            assert_eq!(fresh.prefetch(12, &[(2048, 1365), (2049, 1365)], 2).unwrap(), 0);
        }
        assert!(!d.path().join("fresh/12/2048/1365.png").exists() && !d.path().join("fresh/packs").exists());
        assert_eq!(fresh.get(12, 2048, 1365).unwrap(), (Some(png.clone()), false));
    }

    /// Raw tiles for the z3 tile of `ts` (z3 tile `q`): every z9–12 tile near `cov` and every z3–8 tile
    /// of q; smooth slopes, every third with a one-pixel spike and a one-pixel pit (kept as read;
    /// no repair; the level above takes them in), every seventh missing (the open sea).
    pub(crate) fn synthetic_raw(local: &std::path::Path, cov: &Coverage, q: (u32, u32), ts: &[(u32, u32)]) {
        let mut want: Vec<(u8, u32, u32)> = Vec::new();
        for z in 9..=12u8 {
            let s = 1u32 << (z - 6);
            for &(tx, ty) in ts {
                for x in tx * s..(tx + 1) * s {
                    for y in ty * s..(ty + 1) * s {
                        if near_coverage(cov, z, x, y) {
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
            let mut e: Vec<f32> = (0..256 * 256)
                .map(|i| 400.0 + (i % 256) as f32 * 0.7 + (i / 256) as f32 * 0.4 + (k % 11) as f32 * 30.0)
                .collect();
            if k % 3 == 0 {
                e[128 * 256 + 128] += 900.0;
                e[64 * 256 + 200] -= 700.0;
            }
            std::fs::write(
                local.join(format!("{z}/{x}/{y}.png")),
                roadcore::grid::encode_terrain_png(&e, 256, 256).unwrap(),
            )
            .unwrap();
        }
    }

    /// A coverage near z6 tile 6/21/18 (its grown box meets it) whose z9–12 tiles none come near
    /// (each grown by less in longitude, at its own latitude): a small circle east of the tile's
    /// south-east corner, as the build's coverage left 6/21/18.
    pub(crate) fn near_without_hi(d: &std::path::Path) -> Coverage {
        let b = roadcore::geo::tile_box(6, 21, 18);
        let dx = |lat: f64| roadcore::geo::lon_span(roadcore::geo::NEAR_COVERAGE_KM, lat);
        let (lat, lon) = (b[1] + 0.05, b[2] + (dx(b[3]) + dx(b[1] + 0.5)) / 2.0);
        let c = Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec![format!("place:{lon},{lat},0.5")],
            }],
            None,
            d,
        )
        .unwrap();
        assert!(
            near_coverage(&c, 6, 21, 18)
                && !makes_hi(&c, &crate::terrain_zoom::test_world(12), (21, 18))
                && piece_levels(&c, &crate::terrain_zoom::test_world(12), (21, 18)).iter().all(|(_, t)| t.is_empty())
        );
        c
    }

    #[test]
    fn a_lakes_one_level_across_tiles() {
        use crate::terrain_water::{tests::Fixed, Kind, Poly};
        // Two z11 tiles side by side at 70.5°N: a slope, a lake across both (raised to 400 m), the
        // sea in the first's top rows.
        const Z: u8 = 11;
        let n = (1u64 << Z) as f64;
        let y = (roadcore::geo::merc_y(70.5) * n) as u32;
        let x0 = (roadcore::geo::merc_x(10.0) * n) as u32;
        fn polys(_z: u8, x: u32, _y: u32) -> Vec<Poly> {
            // (The lake from 200 to 300 in the pair's pixels across, rows 100 to 150; the sea's rows
            // 0 to 20 of the first.)
            let x0 = ((10.0 + 180.0) / 360.0 * 2048.0) as u32;
            let off = (x - x0) as f64 * 256.0;
            let mut v = vec![Poly {
                kind: Kind::Lake,
                id: 5,
                rings: vec![vec![
                    [200.0 - off, 100.0],
                    [300.0 - off, 100.0],
                    [300.0 - off, 150.0],
                    [200.0 - off, 150.0],
                ]],
            }];
            if x == x0 {
                v.push(Poly {
                    kind: Kind::Sea,
                    id: 0,
                    rings: vec![vec![[0.0, 0.0], [256.0, 0.0], [256.0, 20.0], [0.0, 20.0]]],
                });
            }
            v
        }
        let water = Fixed(polys);
        let native = |_: u8, _: u32, _: u32| true;
        let src = Sources { water: Some(&water), native: Some(&native), ..Default::default() };
        let raw = |x: u32| {
            let g: Vec<f32> = (0..65536)
                .map(|p| 200.0 + ((x - x0) as usize * 256 + p % 256) as f32 * 0.5 - (p / 256) as f32 * 0.25)
                .collect();
            let mut e = g.clone();
            for r in 100..150 {
                for c in 0..256 {
                    let wx = (x - x0) as usize * 256 + c;
                    if (200..300).contains(&wx) {
                        e[r * 256 + c] = 400.0;
                    }
                }
            }
            (roadcore::grid::encode_terrain_png(&e, 256, 256).unwrap(), g)
        };
        let made: Vec<(Prepared, Vec<f32>)> = [x0, x0 + 1]
            .iter()
            .map(|&x| {
                let (png, g) = raw(x);
                (prepare(Some(png), Z, x, y, &HashMap::new(), &src).unwrap(), g)
            })
            .collect();
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
            outs.push((decode_terrain(&png.unwrap()).unwrap(), g));
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
                        // The raw tile's, as it was.
                        assert!((v - g[r * 256 + c]).abs() < 0.01, "{wx},{r}: {v} vs {}", g[r * 256 + c]);
                    }
                }
            }
        }
        // The lake's level: its shore's 10th percentile, the land beside it, not the raised 400 m.
        assert!(level < 300.0, "{level}");
    }

    /// Tiles of one height each, counting what's asked for: a pool or a relief (`Fills`).
    struct Const {
        h: f32,
        asked: Mutex<Vec<(u8, u32, u32)>>,
        /// The datasets its tiles say they came from (`RawSource::from`); None: not told.
        from: Option<Vec<String>>,
    }

    impl Const {
        fn new(h: f32) -> Self {
            Const { h, asked: Mutex::new(Vec::new()), from: None }
        }

        fn from(h: f32, ids: &[&str]) -> Self {
            Const { from: Some(ids.iter().map(|s| s.to_string()).collect()), ..Const::new(h) }
        }
    }

    impl RawSource for Const {
        fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
            self.asked.lock().unwrap().push((z, x, y));
            Ok((Some(roadcore::grid::encode_terrain_png(&vec![self.h; 65536], 256, 256)?), false))
        }
        fn from(&self, _z: u8, _x: u32, _y: u32) -> Option<Vec<String>> {
            self.from.clone()
        }
        fn prefetch_counted(
            &self,
            _z: u8,
            _tiles: &[(u32, u32)],
            _threads: usize,
            _done: &std::sync::atomic::AtomicU64,
        ) -> anyhow::Result<usize> {
            Ok(0)
        }
    }

    /// The rule (docs/steps.md §2, Terrain): a z6 tile's run samples only its finest zoom's tiles; a
    /// coarser quarter is its children's, one past a region's 20 km the relief's, never the pool's;
    /// a z3 tile's assembly and the root read the relief alone.
    #[test]
    fn coarser_tiles_are_their_childrens_and_the_relief_never_the_pools() {
        let d = tempfile::tempdir().unwrap();
        // (A place 5 km wide: its z6 tile's z11 tiles near it, and quarters past its 20 km.)
        let cov = Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:10.5,45.5,5".into()],
            }],
            None,
            d.path(),
        )
        .unwrap();
        // (A place whose finest zoom is z11 everywhere.)
        let sets = crate::terrain_zoom::test_world(11);
        let t = crate::terrain_zoom::tile_at(10.5, 45.5, 6);
        let (pool, relief) = (Const::new(100.0), Const::new(7.0));
        let fill = fill_of(&cov, &sets, &|_, _| false);
        let reaching = |z: u8, x: u32, y: u32| crate::terrain_zoom::reaching(&sets, z, x, y);
        let raw = Fills::new(&pool, &relief, &fill, &reaching);
        let native = |z: u8, x: u32, y: u32| native_of(&fill, z, x, y);
        let src = Sources { native: Some(&native), sets: &sets, ..Default::default() };
        // (The z6 tile 20 km off the region: its tiles near it, no z12.)
        let levels = piece_levels(&cov, &sets, t);
        assert!(levels.iter().all(|(z, ts)| *z <= 11 || ts.is_empty()));
        assert!(levels.iter().any(|(z, ts)| *z == 11 && !ts.is_empty()));
        let (hi, mid) = make_piece(&raw, &src, levels.clone()).unwrap();
        let sampled: std::collections::BTreeSet<u8> = pool.asked.lock().unwrap().iter().map(|a| a.0).collect();
        assert_eq!(sampled, [11].into(), "only the finest zoom sampled");
        for (z, x, y, b) in &hi {
            let e = decode_terrain(b).unwrap();
            if *z == 11 {
                // (Sampled, a quarter past the region's 20 km the relief's.)
                for k in 0..4u32 {
                    let near = near_coverage(&cov, 12, 2 * x + (k & 1), 2 * y + (k >> 1));
                    let v = e[((k >> 1) as usize * 128 + 64) * 256 + (k & 1) as usize * 128 + 64];
                    let want = if near { 100.0 } else { 7.0 };
                    assert!((v - want).abs() < 0.01, "{z}/{x}/{y} quarter {k}: {v}, not {want}");
                }
            } else {
                // (A quarter whose child isn't built is the relief's, whole; one whose child is, its
                // means: the pool's and, past the region, the relief's, never other.)
                for k in 0..4u32 {
                    let c = (2 * x + (k & 1), 2 * y + (k >> 1));
                    let built = levels.iter().any(|(lz, ts)| *lz == z + 1 && ts.contains(&c));
                    for j in 0..128usize {
                        for i in 0..128usize {
                            let v = e[((k >> 1) as usize * 128 + j) * 256 + (k & 1) as usize * 128 + i];
                            if built {
                                assert!((6.9..=100.1).contains(&v), "{z}/{x}/{y} quarter {k}: {v}");
                            } else {
                                assert!((v - 7.0).abs() < 0.1, "{z}/{x}/{y} quarter {k}: {v}, not the relief's");
                            }
                        }
                    }
                }
            }
        }
        // The assembly: from the mid and the relief, nothing sampled.
        pool.asked.lock().unwrap().clear();
        let q = (t.0 >> 3, t.1 >> 3);
        let lo = Maker::new(&raw, &src).lo(q, &[(t, &mid)]).unwrap();
        assert!(pool.asked.lock().unwrap().is_empty(), "the pool read by the assembly");
        assert!(lo.iter().any(|m| m.0 == 8) && lo.iter().any(|m| m.0 == 3));
        // The root's tiles read the relief, one each at most.
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.path().join("root")), &d.path().join("s")).unwrap();
        relief.asked.lock().unwrap().clear();
        build_root(&mut out, &raw, &src).unwrap();
        assert!(pool.asked.lock().unwrap().is_empty());
        let r = relief.asked.lock().unwrap().clone();
        assert!(r.iter().all(|a| a.0 <= 2) && r.len() <= 21, "{r:?}");
    }

    /// Past z12 a tile none of whose pixels came from a dataset reaching its zoom isn't kept, and a
    /// quarter whose child the run didn't build is its tile's own sample (the depth's rule). Told by
    /// dataset, not code (task #929 review: HRDEM's 1 m and 2 m share theirs, so a tile of the 2 m
    /// alone was kept at z16 where the 1 m reaches it).
    #[test]
    fn a_deep_tile_only_upsampling_a_coarser_dataset_isnt_kept() {
        let (pool, relief) = (Const::from(100.0, &["hrdem"]), Const::new(7.0));
        let child = |_: u8, _: u32, _: u32| Fill::Child;
        let none = |_: u8, _: u32, _: u32| Vec::<String>::new();
        let other = |_: u8, _: u32, _: u32| vec!["hrdem-1m".to_string()];
        let some = |_: u8, _: u32, _: u32| vec!["hrdem-1m".to_string(), "hrdem".to_string()];
        let z = crate::terrain_deep::FIRST_Z + 1;
        let f = Fills::new(&pool, &relief, &child, &none);
        // (No child built: every quarter the tile's own sample, and with no dataset reaching it, not kept.)
        assert!(f.get(z, 10, 10).unwrap().0.is_some() && !f.keep(z, 10, 10));
        let f = Fills::new(&pool, &relief, &child, &other);
        assert!(f.get(z, 10, 10).unwrap().0.is_some() && !f.keep(z, 10, 10), "another dataset reaches it, not its own");
        let f = Fills::new(&pool, &relief, &child, &some);
        assert!(f.get(z, 10, 10).unwrap().0.is_some() && f.keep(z, 10, 10));
        // (Its children built: none of it read.)
        f.built(z + 1, &[(20, 20), (21, 20), (20, 21), (21, 21)]);
        assert!(f.get(z, 10, 10).unwrap().0.is_none());
        // (At z12 and coarser, kept as read.)
        let f = Fills::new(&pool, &relief, &|_, _, _| Fill::Sample, &none);
        assert!(f.get(12, 10, 10).unwrap().0.is_some() && f.keep(12, 10, 10));
        // (A deep tile whose quarters are the relief's, past the region's 20 km, and samples of
        // nothing reaching it: not kept, the relief alone never reaching the band.)
        let edge = |_: u8, x: u32, _: u32| if x % 2 == 0 { Fill::Relief } else { Fill::Sample };
        let f = Fills::new(&pool, &relief, &edge, &none);
        assert!(f.get(z, 10, 10).unwrap().0.is_some() && !f.keep(z, 10, 10));
    }

    /// Carried unrounded within a job and rounded ties to even once as stored, a chained pyramid's
    /// coarsest level stays within about a quantum of a one-pass mean of its finest, its error's
    /// mean near zero (the owner's, 4 Nov).
    #[test]
    fn chained_means_stay_within_a_quantum_of_a_one_pass_mean() {
        // (A synthetic z(n) → z(n−5) pyramid of quantised heights: ties at every level.)
        let side = 256usize << 5;
        let fine: Vec<f32> = (0..side * 256)
            .map(|p| {
                let (i, j) = (p % side, p / side);
                (((i * 7 + j * 13) % 97) as f32 * 0.37 + 120.0 + ((i / 64) as f32) * 0.05).round_ties_even()
            })
            .collect();
        // One pass: the 32 × 32 mean under each coarsest pixel, of the finest as stored.
        let mut stored = fine.clone();
        crate::terrain_codec::round(&mut stored, crate::terrain_codec::NATIVE_M);
        let one = |w: &[f32], width: usize, f: usize| -> Vec<f64> {
            let (cw, ch) = (width / f, w.len() / width / f);
            let mut out = vec![0f64; cw * ch];
            for (p, v) in w.iter().enumerate() {
                out[(p / width / f) * cw + (p % width) / f] += *v as f64 / (f * f) as f64;
            }
            out
        };
        let reference = one(&stored, side, 32);
        // Chained: each level the 2×2 means of the one below, before rounding; rounded once.
        let mut lv = stored.clone();
        let mut w = side;
        for _ in 0..5 {
            let (nw, nh) = (w / 2, lv.len() / w / 2);
            let mut next = vec![0f32; nw * nh];
            for j in 0..nh {
                for i in 0..nw {
                    let a = |di: usize, dj: usize| lv[(2 * j + dj) * w + 2 * i + di];
                    next[j * nw + i] = (a(0, 0) + a(1, 0) + a(0, 1) + a(1, 1)) * 0.25;
                }
            }
            lv = next;
            w = nw;
        }
        crate::terrain_codec::round(&mut lv, crate::terrain_codec::COARSER_M);
        let errs: Vec<f64> = lv.iter().zip(&reference).map(|(a, b)| *a as f64 - b).collect();
        let worst = errs.iter().fold(0f64, |m, e| m.max(e.abs()));
        let mean = errs.iter().sum::<f64>() / errs.len() as f64;
        assert!(worst <= crate::terrain_codec::COARSER_M * 0.5 + 1e-3, "worst {worst}");
        assert!(mean.abs() < crate::terrain_codec::COARSER_M * 0.05, "mean {mean}");
    }
}
