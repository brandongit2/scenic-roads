//! The terrain's deep levels (docs/steps.md §2, Terrain): where a declared elevation dataset
//! (docs/inputs.md §6.8, the roads' datasets) is finer than the worldwide ones, the terrain goes on
//! past the base's finest zoom (`terrain_pack::max_zoom_at`) to the zoom that dataset supports
//! there, its native zoom (`native_zoom`), from the dataset read at its full resolution. One
//! terrain, its packs each a step's: the `terrain-deep` step makes them a z10 tile a target (`make`,
//! `build_target`: its deep pack), from the base it blends into, made from the elevation pool and
//! the water with the terrain-hi run's own code (crate::terrain_pack::Bases), never read from built
//! tiles, so the deep targets wait for no other and run beside the terrain-hi z6 tiles.
//!
//! - **Which datasets:** those whose full resolution (`Measured::full`, else `resolution`) is finer
//!   than the worldwide datasets' finest by over a tenth (`FINER`, `base_resolution`): HRDEM, USGS
//!   3DEP and GSI's, not GLO-30 (30 m: a worldwide one). None is named in code: one added to
//!   `inputs/elevation/` later is taken the same way.
//! - **How deep:** a dataset's zoom at a latitude is the finest whose pixels are no finer than half
//!   its cell (`zoom_for`): 2 m data to z16 in most of Canada, 3DEP's 10 m to z14 (z13 north of
//!   ~55°); a tile pyramid, its own zoom. A base tile's native zoom is the finest of the datasets
//!   meeting it there, and counts only past the base's.
//! - **What a deep tile is:** its pixels sampled from the datasets meeting its z8 tile (in their
//!   order, blended at their edges: crate::dem::generic; planned over that z8 tile's box, `SUB_Z`,
//!   whatever part of it a target makes), at their full resolution and brought to EGM2008 as the
//!   roads' are (crate::dem::geoid), with the base last (`BASE_URL`), so the data blends into the
//!   base over four base pixels (the base's at the z8 tile's middle latitude) where it ends; where
//!   only the base gives a height, the base read across its tiles' edges (`padded`: the z10 tile's
//!   base tiles and the ring of one tile around them, `base_want`, the neighbours' true pixels
//!   whichever z6 tile holds them), as the map draws the base. The base's water is kept: each pixel
//!   by its share of the base tile's sea and lakes (crate::terrain_water, at the base's zoom) takes
//!   the base's flattened height. Below zero to 0, as the base. Stored as every terrain tile is
//!   (crate::terrain_codec: lossless WebP), rounded to 1 cm (the native zoom's rounding: every deep
//!   level is at or past the base's).
//! - **Only where there's data:** a base tile whose datasets have no data in a 128 × 128 grid of
//!   it (`PRESENCE`, read as their roads read them) makes none; a deep tile none of whose pixels
//!   comes from a fine dataset isn't stored (the server makes it from its ancestor, as anywhere
//!   finer than the data).
//! - **Read once:** a target reads its datasets' GeoTIFF blocks ahead of their sampling, `FETCHES`
//!   at once, each once (`read_ahead`, kept in memory by file: `Kept`): first those the presence
//!   grids of its base tiles read, then those its base tiles with data read (each tile's area and
//!   its blends' reach past it: `plan_items`), `KEPT_BYTES` at most. Each file is opened once a
//!   target, its header read once.
//! - **Packs:** by z10 tile (`PACK_Z`): `layers/terrain/deep/10-x-y` (docs/formats.md, Pack), z13
//!   to `MAX_Z` (`FIRST_Z`), a few hundred MB at most; optional in a region's download
//!   (crate::areafiles: "Terrain detail"). A deep level of z12 or coarser (north of 67°, where the
//!   base ends at z11) goes in the z6 tile's hi pack with its base tiles, made by its terrain-hi run
//!   from its tiles as made (`hi_levels`), so every reader finds z9–12 in the hi packs and only z13
//!   on in the deep packs. A z10 tile with a deep pack and no target any more has it dropped by its
//!   run (`drop_target`), a retired fine pack too.

use crate::dem::declared::{self, Accepted, Cover, Data, Dataset, Plan, Source};
use crate::dem::generic::{tile_resolution, Opts, Sampler};
use crate::dem::proj::Projection;
use crate::out::Out;
use anyhow::{Context, Result};
use det::Det;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// The zoom a z6 tile's deep levels are made by: a z8 tile at a time ("8/x/y").
pub const SUB_Z: u8 = 8;
/// The finest zoom any dataset makes, whatever its resolution (the owner's, 23 Oct): 1 m data stops
/// at z16 as 2 m does. One cap for all, in `zoom_for` and a pyramid's zoom alike.
pub const MAX_Z: u8 = 16;
/// The zoom of a deep pack's root tile.
pub const PACK_Z: u8 = 10;
/// A deep pack's first zoom: coarser deep levels go in the hi packs (z9–12, by z6 tile), as every
/// reader looks for them (crate::terrain_pack::ManifestTiles, agent::tiles, the server). Repeated
/// for the app in web/src/config.ts (DEEP_FIRST_Z; PACK_Z as DEEP_PACK_Z).
pub const FIRST_Z: u8 = PACK_Z + 3;
/// A dataset is fine where its resolution is under this share of the terrain's finest worldwide
/// dataset's.
pub const FINER: f64 = 0.9;
/// The presence grid's side (points) over a base tile.
pub const PRESENCE: usize = 128;
/// The base tiles, as the sampler reads them (a tile pyramid served from the run's memory).
pub const BASE_URL: &str = "scenic-terrain:{z}/{x}/{y}";
/// Bumped with any change to the deep tiles' bytes from the same inputs (5: made in the terrain
/// step, as lossless WebP).
pub const DEEP_V: u32 = 5;

/// What a z6 tile's key names of the deep levels' code: their version, every constant their bytes
/// follow and the codec (crate::terrain_codec::version).
pub fn versions() -> String {
    format!(
        "deep {DEEP_V} {} finer {FINER} max {MAX_Z} presence {PRESENCE} pack {PACK_Z}",
        crate::terrain_codec::version()
    )
}

/// The finest zoom at latitude `lat` whose pixels are no finer than half of `res` metres.
pub fn zoom_for(res: f64, lat: f64) -> u8 {
    let mut z = 0u8;
    while z < MAX_Z && tile_resolution(z as u32 + 1, lat) >= res / 2.0 - 1e-9 {
        z += 1;
    }
    z
}

/// How a fine dataset's zoom is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    /// A tile pyramid: its own zoom.
    Pyramid(u8),
    /// A raster: its full resolution, in millimetres.
    Raster(u64),
}

impl Detail {
    pub fn zoom_at(&self, lat: f64) -> u8 {
        match self {
            Detail::Pyramid(z) => (*z).min(MAX_Z),
            Detail::Raster(mm) => zoom_for(*mm as f64 / 1000.0, lat),
        }
    }
}

/// A fine dataset as the keys and runs read it.
#[derive(Clone, Debug, PartialEq)]
pub struct FineSet {
    pub id: String,
    /// As the keys name it (crate::dem::declared::line: its `keyed` and its heights' conversion).
    pub line: String,
    pub cover: Cover,
    pub detail: Detail,
    /// Where its `regions` cut its coverage: what was measured (`Measured::uncut`, for
    /// declared::cut_note); None: not cut.
    pub cut: Option<declared::Measured>,
}

/// A dataset's resolution (m), as the input check measured it.
pub fn full_resolution(d: &Accepted) -> f64 {
    d.measured.resolution
}

/// The base's resolution among datasets `ds` (m): the finest full resolution of the worldwide
/// datasets, those ranked after every unranked one (`rank` above 0, as the pool declares GLO-30 and
/// the Terrarium tiles: docs/inputs.md §6.8); none: no dataset is finer than the base.
pub fn base_resolution(ds: &[Accepted]) -> f64 {
    ds.iter().filter(|d| d.declaration.rank.unwrap_or(0) > 0).map(full_resolution).fold(f64::INFINITY, f64::min)
}

/// The datasets `ds` (their conversions set: declared::convert) finer than the terrain's base (`FINER`
/// of `base_resolution`), in their order.
pub fn sets(ds: &[Accepted]) -> Vec<FineSet> {
    let base = base_resolution(ds);
    ds.iter()
        .filter(|d| full_resolution(d) < base * FINER)
        .map(|d| FineSet {
            id: d.id.clone(),
            line: declared::line(d),
            cover: Cover::of(d.measured.cover.as_deref()),
            detail: match &d.declaration.data {
                Data::Tiles { zoom, .. } => Detail::Pyramid((*zoom).min(MAX_Z as u32) as u8),
                Data::Geotiff { .. } => Detail::Raster((full_resolution(d) * 1000.0).round() as u64),
            },
            cut: d.measured.uncut.is_some().then(|| d.measured.clone()),
        })
        .collect()
}

/// Of `sets`, those meeting box `b` (w, s, e, n) and finer there than the base at latitude `lat`.
pub fn meeting(sets: &[FineSet], b: [f64; 4], lat: f64) -> Vec<&FineSet> {
    let base = crate::terrain_pack::max_zoom_at(lat);
    sets.iter().filter(|s| s.cover.meets(b) && s.detail.zoom_at(lat) > base).collect()
}

/// The poleward latitude of a box (where its datasets are finest).
fn poleward(b: [f64; 4]) -> f64 {
    b[1].abs().max(b[3].abs())
}

/// What tile `z/x/y`'s deep levels' key names of the fine datasets meeting it and finer there:
/// each one's line and the zoom it reaches there (so a dataset's measured resolution moving moves
/// the key). None: no deep levels there.
pub fn key_lines(sets: &[FineSet], z: u8, x: u32, y: u32) -> Vec<String> {
    let b = crate::stage::tile_box_grown(z, x, y, 0.0);
    let lat = poleward(b);
    meeting(sets, b, lat)
        .into_iter()
        .map(|s| match &s.cut {
            Some(m) => match declared::cut_note(m, &s.cover, b) {
                Some(n) => format!("{} {n} zoom {}", s.line, s.detail.zoom_at(lat)),
                None => format!("{} zoom {}", s.line, s.detail.zoom_at(lat)),
            },
            None => format!("{} zoom {}", s.line, s.detail.zoom_at(lat)),
        })
        .collect()
}

/// The finest zoom the fine datasets `sets` reach in tile `z/x/y` (at its poleward edge), where
/// past the base's: a deep pack's (`native_zoom`).
pub fn zoom_of(sets: &[FineSet], z: u8, x: u32, y: u32) -> Option<u8> {
    let b = crate::stage::tile_box_grown(z, x, y, 0.0);
    let lat = poleward(b);
    meeting(sets, b, lat).iter().map(|s| s.detail.zoom_at(lat)).max()
}

/// Z10 tile `t`'s native zoom: the deep levels' there (`zoom_of`), else the base's finest
/// (`terrain_pack::max_zoom_at`). The catalogue's `native_zoom` (by deep pack) and the map's zoom by
/// area (crate::layers).
pub fn native_zoom(sets: &[FineSet], t: (u32, u32)) -> u8 {
    zoom_of(sets, PACK_Z, t.0, t.1)
        .unwrap_or_else(|| crate::terrain_pack::max_zoom_at(crate::terrain_pack::tile_lat(PACK_Z, t.1)))
}

/// The z10 tiles of z6 tile `t` whose native zoom is past the base's (`zoom_of`): where its deep
/// levels are and its base tiles are rounded as coarser than native
/// (crate::terrain_pack::Sources::deeper).
pub fn deeper(sets: &[FineSet], t: (u32, u32)) -> std::collections::BTreeSet<(u32, u32)> {
    let k = PACK_Z - 6;
    let mut out = std::collections::BTreeSet::new();
    for x in t.0 << k..(t.0 + 1) << k {
        for y in t.1 << k..(t.1 + 1) << k {
            if zoom_of(sets, PACK_Z, x, y).is_some() {
                out.insert((x, y));
            }
        }
    }
    out
}

/// What terrain-hi z6 tile `t`'s key names of the fine datasets: its z10 tiles past the base's zoom
/// (`deeper`: its base tiles there rounded as coarser than native), and where it makes deep levels
/// coarser than `FIRST_Z` itself (`hi_levels`: its z8 tiles near the coverage, 20 km, whose base ends
/// short of them), their code (`versions`) and each such z8 tile's fine datasets' lines with the zoom
/// each reaches there (`key_lines`). Its deep levels from `FIRST_Z` on are its terrain-deep targets'
/// (`target_key_lines`), never its key's.
pub fn piece_key_lines(sets: &[FineSet], cov: &crate::coverage::Coverage, t: (u32, u32)) -> String {
    let deeper: Vec<String> = deeper(sets, t).iter().map(|(x, y)| format!("{PACK_Z}/{x}/{y}")).collect();
    let mut out = vec![format!("deeper {}", deeper.join(" "))];
    let k = SUB_Z - 6;
    let mut mine = Vec::new();
    for x in t.0 << k..(t.0 + 1) << k {
        for y in t.1 << k..(t.1 + 1) << k {
            let lines = key_lines(sets, SUB_Z, x, y);
            if short((x, y)) && !lines.is_empty() && crate::terrain_pack::near_coverage(cov, SUB_Z, x, y, 20.0) {
                mine.push(format!("{SUB_Z}/{x}/{y} {} (hi to z{})", lines.join(" | "), FIRST_Z - 1));
            }
        }
    }
    if !mine.is_empty() {
        out.push(versions());
        out.extend(mine);
    }
    out.join("\n")
}

/// The fine datasets as a run reads them: the accepted elevation version (their files, by its
/// index), the declared geoid sources converting their heights, and the sets.
#[derive(Clone, Debug)]
pub struct Fine {
    pub elevation: crate::inputs::Accepted,
    pub geoid: Vec<crate::inputs::sources::Source>,
    pub sets: Vec<FineSet>,
}

impl Fine {
    /// The fine datasets of the accepted versions `m` names under `root`, their heights converted by
    /// the accepted geoid sources (`geoid` from the accepted sources, if any); None without an
    /// accepted elevation version.
    pub fn read(root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) -> Result<Option<Fine>> {
        let Some(a) = crate::inputs::open(root, m, crate::inputs::elevation::UNIT)? else { return Ok(None) };
        let geoid = match crate::inputs::sources::open(root, m)? {
            Some(s) => s.many(crate::inputs::sources::GEOID).to_vec(),
            None => Vec::new(),
        };
        Ok(Some(Fine::of(a, geoid)?))
    }

    /// Of accepted version `a`, converted by `geoid`.
    pub fn of(a: crate::inputs::Accepted, geoid: Vec<crate::inputs::sources::Source>) -> Result<Fine> {
        let mut ds = declared::accepted(&a)?;
        // (Said by the roads' own plans: crate::dem::declared::plan_converted.)
        let _ = declared::convert(&mut ds, &geoid);
        let sets = sets(&ds);
        Ok(Fine { elevation: a, geoid, sets })
    }

    /// Whether a fine dataset meets z8 tile `t` and is finer there than the base (`meeting`, by
    /// the datasets' coverage as their `regions` cut it): a z8 tile none meets has nothing to read.
    pub fn meets(&self, t: (u32, u32)) -> bool {
        let b = crate::stage::tile_box_grown(SUB_Z, t.0, t.1, 0.0);
        !meeting(&self.sets, b, poleward(b)).is_empty()
    }

    /// The plans over box `b` (its poleward latitude `lat`): the presence plan (the fine datasets
    /// meeting it, read as their roads read them) and the full one (read at full resolution, the
    /// base last); None where no dataset is fine there.
    fn plans(&self, b: [f64; 4]) -> Result<Option<(Plan, Plan)>> {
        let ids: Vec<&str> = meeting(&self.sets, b, poleward(b)).iter().map(|s| s.id.as_str()).collect();
        if ids.is_empty() {
            return Ok(None);
        }
        let mut presence = declared::plan_converted(&self.elevation, b, &self.geoid)?;
        presence.datasets.retain(|d| ids.contains(&d.id.as_str()));
        if presence.datasets.is_empty() {
            return Ok(None);
        }
        let mut full = presence.clone();
        // (The base, by the band of latitude its finest zoom holds: crate::terrain_pack::max_zoom_at;
        // its resolution the base's at the box's middle, so its edge's blend is four of its pixels
        // there.)
        let mid = (b[1] + b[3]) / 2.0;
        let bands: [(u32, Vec<[f64; 4]>); 3] = [
            (12, vec![[-180.0, -67.0, 180.0, 67.0]]),
            (11, vec![[-180.0, 67.0, 180.0, 79.0], [-180.0, -79.0, 180.0, -67.0]]),
            (10, vec![[-180.0, 79.0, 180.0, 86.0], [-180.0, -86.0, 180.0, -79.0]]),
        ];
        for (zb, cover) in bands {
            full.datasets.push(Dataset {
                id: format!("terrain-z{zb}"),
                code: 0,
                resolution: tile_resolution(zb, mid),
                cover: Some(cover),
                source: Source::Tiles { url: BASE_URL.into(), zoom: zb, encoding: "terrarium".into() },
                convert: None,
            });
        }
        Ok(Some((presence, full)))
    }
}

/// The base tiles served to the sampler from memory (`BASE_URL`), everything else fetched; each
/// file opened once a target (`Kept`), whichever sampler or plan opens it.
struct BaseFetch<'a> {
    /// The base, set once it's made (`set_base`): read by the full plan alone, after the presence
    /// grids (which read the fine datasets alone) have said which base tiles need it.
    base: std::sync::OnceLock<HashMap<(u8, u32, u32), Vec<u8>>>,
    inner: &'a dyn crate::fetch::Fetch,
    opened: Mutex<HashMap<String, Option<Arc<Kept>>>>,
    /// The bytes of the files' small reads kept (`SMALL_BYTES` at most).
    small: Arc<AtomicUsize>,
}

impl<'a> BaseFetch<'a> {
    fn new(inner: &'a dyn crate::fetch::Fetch) -> Self {
        BaseFetch { base: Default::default(), inner, opened: Mutex::new(HashMap::new()), small: Default::default() }
    }

    /// The base served from now on (as PNGs: `make_tiles`).
    fn set_base(&self, base: HashMap<(u8, u32, u32), Vec<u8>>) {
        let _ = self.base.set(base);
    }

    fn base(&self, url: &str) -> Option<Option<Vec<u8>>> {
        let rest = url.strip_prefix("scenic-terrain:")?;
        let mut p = rest.split('/').map(|s| s.parse::<u32>().ok());
        let (z, x, y) = (p.next()??, p.next()??, p.next()??);
        Some(self.base.get().and_then(|b| b.get(&(z as u8, x, y)).cloned()))
    }

    /// `url`'s file, opened once.
    fn kept(&self, url: &str) -> Result<Option<Arc<Kept>>> {
        if let Some(k) = self.opened.lock().unwrap().get(url) {
            return Ok(k.clone());
        }
        let k = match self.inner.open(url)? {
            Some(f) => Some(Arc::new(Kept {
                len: f.len()?,
                inner: f,
                spans: Default::default(),
                small: Default::default(),
                small_bytes: self.small.clone(),
            })),
            None => None,
        };
        Ok(self.opened.lock().unwrap().entry(url.to_string()).or_insert(k).clone())
    }

    /// The blocks read ahead and kept but those of `keep` let go.
    fn keep_only(&self, keep: &[Item]) {
        let mine: std::collections::HashSet<(usize, u64)> =
            keep.iter().map(|i| (Arc::as_ptr(&i.file) as usize, i.off)).collect();
        for k in self.opened.lock().unwrap().values().flatten() {
            let p = Arc::as_ptr(k) as usize;
            k.spans.lock().unwrap().retain(|off, _| mine.contains(&(p, *off)));
        }
    }
}

impl crate::fetch::Fetch for BaseFetch<'_> {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
        Ok(self.kept(url)?.map(|k| k as Arc<dyn store::range::RangeRead>))
    }
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.base(url) {
            Some(b) => Ok(b),
            None => self.inner.get(url),
        }
    }
    fn get_within(&self, url: &str, within: crate::fetch::Within) -> Result<Option<Vec<u8>>> {
        match self.base(url) {
            Some(b) => Ok(b),
            None => self.inner.get_within(url, within),
        }
    }
}

/// How many of a target's blocks are fetched at once ahead of their sampling (`read_ahead`), and
/// its files opened at once.
pub const FETCHES: usize = 48;
/// The most bytes (as fetched) of a target's blocks read ahead and kept at once: each block is
/// fetched once a target and kept until the last z10 tile reading it is made (past this the
/// reading ahead waits, but for the z10 tile being made).
pub const KEPT_BYTES: usize = 2 << 30;
/// The most bytes of its files' small reads (their headers' windows) a target keeps, so each
/// sampler opening a file reads its header from memory.
const SMALL_BYTES: usize = 64 << 20;

/// The most memory (MB) deep tiles take as they're made, from their bounds: the blocks kept
/// (`KEPT_BYTES`) and the files' small reads (`SMALL_BYTES`); a deep pack's tiles held until written
/// (z13 to `MAX_Z` under a `PACK_Z` tile, each at most its raw RGB); the base tiles read, decoded and
/// served as PNG (at most raw RGB): a z10 tile's and their ring at the base's z12 (a terrain-deep
/// target, `make`), or a z8 tile's (a terrain-hi run's deep levels coarser than `FIRST_Z`,
/// `hi_levels`).
pub const fn peak_mb() -> u64 {
    let mut pack_tiles = 0u64;
    let mut z = FIRST_Z;
    while z <= MAX_Z {
        pack_tiles += 1 << (2 * (z - PACK_Z));
        z += 1;
    }
    let rgb = 256 * 256 * 3;
    let side = (1u64 << (12 - SUB_Z)) + 2;
    let bytes = KEPT_BYTES as u64 + SMALL_BYTES as u64 + pack_tiles * rgb + side * side * (256 * 256 * 4 + rgb);
    bytes >> 20
}

/// The most memory (MB) a terrain-deep target's run takes, from the code's bounds: its base made
/// first (crate::terrain_pack::Bases: at most a z6 tile's levels made whole, as its terrain-hi run,
/// crate::terrain_pack::piece_peak_mb), then its deep tiles (`peak_mb`), the base kept for them
/// counted there.
pub const fn target_peak_mb() -> u64 {
    let (a, b) = (crate::terrain_pack::piece_peak_mb(), peak_mb());
    if a > b {
        a
    } else {
        b
    }
}

/// Whether tile `z/x/y` (z ≤ `SUB_Z`) has deep levels to make: a fine dataset meets one of its z8
/// tiles and is finer there (as `Fine::meets`).
pub fn reaches(sets: &[FineSet], z: u8, x: u32, y: u32) -> bool {
    let k = SUB_Z - z;
    (x << k..(x + 1) << k).any(|a| {
        (y << k..(y + 1) << k).any(|b| {
            let bb = crate::stage::tile_box_grown(SUB_Z, a, b, 0.0);
            !meeting(sets, bb, poleward(bb)).is_empty()
        })
    })
}

/// Whether tile `z/x/y` (z ≤ `SUB_Z`) has deep levels its terrain-hi run makes (`hi_levels`): a fine
/// dataset meets one of its z8 tiles whose base ends short of `FIRST_Z - 1` (north of 67°) and is
/// finer there.
pub fn reaches_hi(sets: &[FineSet], z: u8, x: u32, y: u32) -> bool {
    let k = SUB_Z - z;
    (x << k..(x + 1) << k).any(|a| {
        (y << k..(y + 1) << k).any(|b| {
            let bb = crate::stage::tile_box_grown(SUB_Z, a, b, 0.0);
            short((a, b)) && !meeting(sets, bb, poleward(bb)).is_empty()
        })
    })
}

/// A fine dataset's file as a target's run reads it: its blocks read ahead (`read_ahead`) kept by
/// offset, its small reads kept as asked; anything else read from the file.
struct Kept {
    inner: Arc<dyn store::range::RangeRead>,
    len: u64,
    spans: Mutex<BTreeMap<u64, Arc<[u8]>>>,
    small: Mutex<HashMap<(u64, usize), Arc<[u8]>>>,
    small_bytes: Arc<AtomicUsize>,
}

impl Kept {
    /// What a read ahead of `n` bytes at `off` keeps: a header window's worth at least (`fetch`).
    fn span(&self, off: u64, n: u64) -> usize {
        n.max((crate::geotiff::HEADER_WINDOW as u64).min(self.len.saturating_sub(off))) as usize
    }

    fn kept(&self, off: u64, len: usize) -> Option<Vec<u8>> {
        let s = self.spans.lock().unwrap();
        let (at, b) = s.range(..=off).next_back()?;
        let o = (off - at) as usize;
        (o + len <= b.len()).then(|| b[o..o + len].to_vec())
    }

    /// Bytes `off..off + n` read and kept: a header window's worth at least (a small block is read
    /// by a window's read: crate::geotiff's `Windowed`).
    fn fetch(&self, off: u64, n: u64) -> Result<(), store::IoError> {
        let n = self.span(off, n);
        if self.spans.lock().unwrap().get(&off).is_some_and(|b| b.len() >= n) {
            return Ok(());
        }
        let b: Arc<[u8]> = self.inner.read_at(off, n)?.into();
        self.spans.lock().unwrap().insert(off, b);
        Ok(())
    }
}

impl store::range::RangeRead for Kept {
    fn len(&self) -> Result<u64, store::IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::IoError> {
        if let Some(b) = self.kept(off, len) {
            return Ok(b);
        }
        let small = len <= crate::geotiff::HEADER_WINDOW;
        if small {
            if let Some(b) = self.small.lock().unwrap().get(&(off, len)) {
                return Ok(b.to_vec());
            }
        }
        let b = self.inner.read_at(off, len)?;
        if small && self.small_bytes.fetch_add(len, Ordering::Relaxed) + len <= SMALL_BYTES {
            self.small.lock().unwrap().insert((off, len), b.as_slice().into());
        }
        Ok(b)
    }
}

/// A block a target reads: its file, its bytes' place, and the first and last groups (z10 tiles,
/// by their place in the run's order) reading it.
struct Item {
    file: Arc<Kept>,
    off: u64,
    n: u64,
    first: usize,
    last: usize,
}

impl Item {
    /// The bytes it keeps once read (`Kept::span`), as `KEPT_BYTES` counts them.
    fn size(&self) -> usize {
        self.file.span(self.off, self.n)
    }
}

/// A GeoTIFF file of a plan's datasets near a target, open to plan its reads: its CRS, the level
/// its plan reads and how far past a point its sampling reads (m: the blends' widest band).
struct PlanFile {
    file: Arc<Kept>,
    tiff: crate::geotiff::Tiff,
    crs: u16,
    level: usize,
    reach: f64,
}

/// `f` run on each of `items`, `threads` at once (network reads: their waits overlap).
fn at_once<T: Sync, R: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let out: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, items.len().max(1)) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(t) = items.get(i) else { return };
                *out[i].lock().unwrap() = Some(f(t));
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().unwrap().expect("each item run")).collect()
}

/// Lon, lat in a CRS: projected by `p` (crate::dem::proj::Projection::of the CRS); geographic (4326,
/// 4269: no projection) as they are.
fn native(p: Option<&Projection>, lon: f64, lat: f64) -> (f64, f64) {
    p.map_or((lon, lat), |p| p.project(lon, lat))
}

/// The side of the grid of points a base tile's reads are planned by (`grid`).
const GRID: usize = 33;

/// Box `b`'s grid of points (lon, lat), `GRID` a side, evenly in Web Mercator, its edges on them.
fn grid(b: [f64; 4]) -> Vec<(f64, f64)> {
    let merc = |lat: f64| {
        let r = lat.to_radians();
        (r.dtan() + 1.0 / r.dcos()).dln()
    };
    let (y0, y1, g) = (merc(b[1]), merc(b[3]), (GRID - 1) as f64);
    (0..GRID)
        .flat_map(|j| {
            let lat = (y0 + (y1 - y0) * j as f64 / g).dsinh().datan().to_degrees();
            (0..GRID).map(move |i| (b[0] + (b[2] - b[0]) * i as f64 / g, lat))
        })
        .collect()
}

/// The GeoTIFF files of `plan`'s datasets meeting box `b` (those fetched: not in a store, nor
/// added files) whose footprints come within their bands' reach of it, open (`FETCHES` at once).
fn plan_files(plan: &Plan, b: [f64; 4], bf: &BaseFetch) -> Result<Vec<PlanFile>> {
    use crate::dem::generic::M_PER_DEG;
    let lat = poleward(b);
    let here: Vec<&Dataset> = plan.datasets.iter().filter(|d| Cover::of(d.cover.as_deref()).meets(b)).collect();
    let reach = 4.0 * here.iter().map(|d| d.resolution).fold(0.0, f64::max);
    let pts = grid(b);
    let mut todo = Vec::new();
    for d in &here {
        let Source::Geotiff { crs, read_resolution, store: false, files, .. } = &d.source else { continue };
        // (The box in the files' CRS, grown by the bands' reach.)
        let p = Projection::of(*crs);
        let np: Vec<(f64, f64)> = pts.iter().map(|&(lo, la)| native(p.as_ref(), lo, la)).collect();
        let grow = if p.is_some() {
            (reach, reach)
        } else {
            (reach / (M_PER_DEG * lat.to_radians().cos()), reach / M_PER_DEG)
        };
        let nb = [
            np.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - grow.0,
            np.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - grow.1,
            np.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + grow.0,
            np.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) + grow.1,
        ];
        for (url, fp) in files {
            let apart = fp.is_some_and(|f| f[0] > nb[2] || f[2] < nb[0] || f[1] > nb[3] || f[3] < nb[1]);
            if !url.starts_with('/') && !apart {
                todo.push((url.as_str(), *crs, *read_resolution));
            }
        }
    }
    let opened = at_once(&todo, FETCHES, |&(url, crs, rr)| -> Result<Option<PlanFile>> {
        let Some(file) = bf.kept(url)? else { return Ok(None) };
        let tiff = crate::geotiff::Tiff::open(file.clone()).with_context(|| url.to_string())?;
        let level = crate::dem::generic::level_for(&tiff, crs, rr)?;
        Ok(Some(PlanFile { file, tiff, crs, level, reach }))
    });
    Ok(opened.into_iter().collect::<Result<Vec<_>>>()?.into_iter().flatten().collect())
}

/// The blocks of `files` that sampling the base tiles of `groups` reads (each tile's area and its
/// bands' reach past it, by a grid of its points: `grid`), in the groups' order, each once, with
/// the first and last group reading it.
fn plan_items(files: &[PlanFile], groups: &[Vec<(u8, u32, u32)>]) -> Result<Vec<Item>> {
    let mut at: HashMap<(usize, u32, u32), usize> = HashMap::new();
    let mut items: Vec<Item> = Vec::new();
    for (g, tiles) in groups.iter().enumerate() {
        for &(z, x, y) in tiles {
            let tb = crate::stage::tile_box_grown(z, x, y, 0.0);
            let lat = poleward(tb);
            let pts = grid(tb);
            let mut by_crs: HashMap<u16, Vec<(f64, f64)>> = HashMap::new();
            for (k, f) in files.iter().enumerate() {
                let np = by_crs.entry(f.crs).or_insert_with(|| {
                    let p = Projection::of(f.crs);
                    pts.iter().map(|&(lo, la)| native(p.as_ref(), lo, la)).collect()
                });
                let gt = f.tiff.transform(f.level)?;
                let img = f.tiff.level(f.level)?;
                let px: Vec<(f64, f64)> =
                    np.iter().map(|&(nx, ny)| ((nx - gt[0]) / gt[1], (ny - gt[3]) / gt[5])).collect();
                // (A point's reach in pixels: its bands', its sampling's neighbours, and half the
                // grid's step.)
                let (mx, my) = crate::dem::generic::pixel_m(f.crs, &gt, lat);
                let mut step: f64 = 0.0;
                for j in 0..GRID {
                    for i in 0..GRID {
                        let p = px[j * GRID + i];
                        let next = [
                            (i + 1 < GRID).then(|| px[j * GRID + i + 1]),
                            (j + 1 < GRID).then(|| px[(j + 1) * GRID + i]),
                        ];
                        for q in next.into_iter().flatten() {
                            step = step.max((p.0 - q.0).abs()).max((p.1 - q.1).abs());
                        }
                    }
                }
                let (pc, pr) = (f.reach / mx + 2.0 + step / 2.0, f.reach / my + 2.0 + step / 2.0);
                let (w, h) = (img.width as f64, img.height as f64);
                let (bw, bh) = (img.block_w, img.block_h);
                let mut mine = std::collections::BTreeSet::new();
                for &(c, r) in &px {
                    let (c0, c1) = ((c - pc).max(0.0), (c + pc).min(w - 1.0));
                    let (r0, r1) = ((r - pr).max(0.0), (r + pr).min(h - 1.0));
                    if c0 > c1 || r0 > r1 {
                        continue;
                    }
                    for by in r0 as u32 / bh..=r1 as u32 / bh {
                        for bx in c0 as u32 / bw..=c1 as u32 / bw {
                            mine.insert((bx, by));
                        }
                    }
                }
                for (bx, by) in mine {
                    match at.get(&(k, bx, by)) {
                        Some(&i) => items[i].last = g,
                        None => {
                            let Some((off, n)) = f.tiff.block_span(f.level, bx, by)? else { continue };
                            at.insert((k, bx, by), items.len());
                            items.push(Item { file: f.file.clone(), off, n, first: g, last: g });
                        }
                    }
                }
            }
        }
    }
    Ok(items)
}

/// `work` run on each group in turn (0 to `n - 1`) while `FETCHES` threads read `items`' blocks
/// (in their order: by their first group) into their files' `Kept`; a group is worked once its
/// blocks are in. The bytes read and not yet let go are held to `KEPT_BYTES`, past which only the
/// group being worked's blocks are read; with `release`, a block is let go once its last group is
/// worked. A failed read stops the reading ahead (the sampling's own read of that block says why);
/// so does `work`'s end, however it ends, a read waiting to retry too (crate::fetch::cancellable).
fn read_ahead(items: &[Item], n: usize, release: bool, mut work: impl FnMut(usize) -> Result<()>) -> Result<()> {
    if items.is_empty() {
        return (0..n).try_for_each(work);
    }
    // (By group: how many items are its first's, and those whose last it is.)
    let mut firsts = vec![0usize; n];
    let mut lasts: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, it) in items.iter().enumerate() {
        firsts[it.first] += 1;
        lasts[it.last].push(i);
    }
    /// The fetching's and the working's shared state.
    struct St {
        next: usize,
        /// By group, its first's items read.
        done: Vec<usize>,
        held: usize,
        /// The group being worked.
        at: usize,
        stop: bool,
    }
    let st = Mutex::new(St { next: 0, done: vec![0; n], held: 0, at: 0, stop: false });
    let cv = Condvar::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let lock = || st.lock().unwrap_or_else(|e| e.into_inner());
    let stop = || {
        cancel.store(true, Ordering::Relaxed);
        lock().stop = true;
        cv.notify_all();
    };
    // (Stopped however the working ends, a panic too, so the threads end.)
    struct Stop<F: Fn()>(F);
    impl<F: Fn()> Drop for Stop<F> {
        fn drop(&mut self) {
            (self.0)()
        }
    }
    std::thread::scope(|s| {
        for _ in 0..FETCHES.min(items.len()) {
            s.spawn(|| loop {
                let i = {
                    let mut g = lock();
                    while !g.stop && g.next < items.len() && g.held >= KEPT_BYTES && items[g.next].first > g.at {
                        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                    if g.stop || g.next >= items.len() {
                        return;
                    }
                    g.next += 1;
                    g.held += items[g.next - 1].size();
                    g.next - 1
                };
                let it = &items[i];
                if crate::fetch::cancellable(&cancel, || it.file.fetch(it.off, it.n)).is_err() {
                    stop();
                    return;
                }
                lock().done[it.first] += 1;
                cv.notify_all();
            });
        }
        let _stop = Stop(&stop);
        for k in 0..n {
            {
                let mut g = lock();
                g.at = k;
                cv.notify_all();
                while !g.stop && g.done[k] < firsts[k] {
                    g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                }
            }
            work(k)?;
            if release {
                let mut g = lock();
                for &i in &lasts[k] {
                    let it = &items[i];
                    it.file.spans.lock().unwrap().remove(&it.off);
                    g.held = g.held.saturating_sub(it.size());
                }
                cv.notify_all();
            }
        }
        Ok(())
    })
}

/// Pixel `(i, j)`'s centre of tile `z/x/y`: (lon, lat).
fn pixel_lonlat(z: u8, x: u32, y: u32, i: usize, j: usize) -> (f64, f64) {
    let n = (1u64 << z) as f64;
    let fx = (x as f64 + (i as f64 + 0.5) / 256.0) / n;
    let fy = (y as f64 + (j as f64 + 0.5) / 256.0) / n;
    (fx * 360.0 - 180.0, (std::f64::consts::PI * (1.0 - 2.0 * fy)).dsinh().datan().to_degrees())
}

/// `v` (`side` × `side`: a base tile at zoom `zb`, padded by `(side - 256) / 2` pixels of its
/// neighbours: `padded`) at fine tile `z/x/y`'s pixel `(i, j)`, bilinearly, its edges clamping.
#[allow(clippy::too_many_arguments)]
fn up(v: &[f32], side: usize, zb: u8, z: u8, x: u32, y: u32, i: usize, j: usize) -> f32 {
    let pad = ((side - 256) / 2) as f64;
    let k = (1u32 << (z - zb)) as f64;
    let bx = (x as f64 / k).floor();
    let by = (y as f64 / k).floor();
    let u = ((x as f64 + (i as f64 + 0.5) / 256.0) / k - bx) * 256.0 - 0.5 + pad;
    let w = ((y as f64 + (j as f64 + 0.5) / 256.0) / k - by) * 256.0 - 0.5 + pad;
    roadcore::grid::bilinear(v, side, u, w)
}

/// The side of a base tile padded by a pixel of its neighbours (`padded`).
pub const PADDED: usize = 258;

/// Base tile `b`'s heights (`tiles`: the base tiles decoded) padded by a pixel of each of its eight
/// neighbours (`PADDED` × `PADDED`), so a fine tile reads the base across the base tiles' edges as
/// MapLibre draws it (its DEM tiles' borders filled from their neighbours, our fork's
/// `DEMData.backfillFrom`): a neighbour of its zoom pixel for pixel; where none is stored, a stored
/// tile up to two zooms finer or coarser holding the pixel's centre, bilinearly there (clamped
/// inside that tile), as the server makes a missing tile from its ancestor
/// (roadcore::grid::upsampled: the map's base goes to z12 everywhere, so it draws those); else the
/// padding's next pixel in (a corner: the side neighbour's, where there is one).
pub fn padded(tiles: &HashMap<(u8, u32, u32), Vec<f32>>, b: (u8, u32, u32)) -> Vec<f32> {
    let (z, x, y) = b;
    let n = 1i64 << z;
    let p = PADDED as i64;
    let mut out = vec![f32::NAN; PADDED * PADDED];
    for j in 0..p {
        for i in 0..p {
            let (dx, dy) = ((i - 1).div_euclid(256), (j - 1).div_euclid(256));
            let ny = y as i64 + dy;
            if (0..n).contains(&ny) {
                if let Some(t) = tiles.get(&(z, (x as i64 + dx).rem_euclid(n) as u32, ny as u32)) {
                    out[(j * p + i) as usize] =
                        t[(j - 1).rem_euclid(256) as usize * 256 + (i - 1).rem_euclid(256) as usize];
                }
            }
        }
    }
    for j in 0..p {
        for i in 0..p {
            let k = (j * p + i) as usize;
            if out[k].is_nan() {
                // (The pixel's centre in tiles of zoom z, x within the world.)
                let fx = ((x as i64 * 256 + i - 1) as f64 + 0.5) / 256.0;
                let fy = ((y as i64 * 256 + j - 1) as f64 + 0.5) / 256.0;
                out[k] = other_zoom(tiles, z, fx.rem_euclid(n as f64), fy).unwrap_or(f32::NAN);
            }
        }
    }
    for j in 0..p {
        for i in 0..p {
            if out[(j * p + i) as usize].is_nan() {
                let (ci, cj) = (i.clamp(1, p - 2), j.clamp(1, p - 2));
                let v =
                    [(i, cj), (ci, j), (ci, cj)].iter().map(|&(a, b)| out[(b * p + a) as usize]).find(|v| !v.is_nan());
                out[(j * p + i) as usize] = v.unwrap_or(f32::NAN);
            }
        }
    }
    out
}

/// The height at `(fx, fy)` (in tiles of zoom `z`) from a stored tile up to two zooms finer or
/// coarser than `z` holding it, bilinearly at that point (clamped inside the tile): finer first.
fn other_zoom(tiles: &HashMap<(u8, u32, u32), Vec<f32>>, z: u8, fx: f64, fy: f64) -> Option<f32> {
    if !(0.0..(1u64 << z) as f64).contains(&fy) {
        return None;
    }
    [1i32, 2, -1, -2].iter().filter(|&&d| (0..=MAX_Z as i32).contains(&(z as i32 + d))).find_map(|&d| {
        let s = 2f64.powi(d);
        let (gx, gy) = (fx * s, fy * s);
        let (tx, ty) = (gx.floor(), gy.floor());
        let t = tiles.get(&((z as i32 + d) as u8, tx as u32, ty as u32))?;
        Some(roadcore::grid::bilinear(t, 256, (gx - tx) * 256.0 - 0.5, (gy - ty) * 256.0 - 0.5))
    })
}

/// A deep tile's heights finished: where the sampler has none, or only the base's (NaN), the base
/// (`base`: `padded`, so it meets its neighbours' as the map draws the base); each pixel by its share
/// of water (`water`, 0–1 at the base's 256 × 256 pixels) to the base's height; below zero to 0.
pub fn finish(e: &mut [f32], base: &[f32], water: Option<&[f32]>, zb: u8, z: u8, x: u32, y: u32) {
    for j in 0..256 {
        for i in 0..256 {
            let k = j * 256 + i;
            let b = up(base, PADDED, zb, z, x, y, i, j);
            let mut v = if e[k].is_finite() { e[k] } else { b };
            if let Some(w) = water {
                let s = up(w, 256, zb, z, x, y, i, j).clamp(0.0, 1.0);
                v = s * b + (1.0 - s) * v;
            }
            e[k] = v.max(0.0);
        }
    }
}

/// A deep tile's metres (256 × 256: `finish`) rounded to 1 cm and stored (crate::terrain_codec).
pub fn encode(e: &mut [f32]) -> Result<Vec<u8>> {
    crate::terrain_codec::encode(e, crate::terrain_codec::NATIVE_M)
}

/// Deep pack `x`, `y`'s logical name (a z10 tile).
pub fn pack_logical(x: u32, y: u32) -> String {
    format!("{PREFIX}{PACK_Z}-{x}-{y}")
}

/// The deep packs' logical names' prefix.
pub const PREFIX: &str = "layers/terrain/deep/";

/// The terrain's packs past the base from before one terrain (task #633: JPEG XL, by z10 tile
/// alike), never made now: their z6 tile's next run drops them (`named_under`).
pub const RETIRED: &str = "layers/terrain/fine/";

/// A deep pack's z10 tile, from its logical name (or a retired pack's, `RETIRED`).
pub fn pack_tile(l: &str) -> Option<(u32, u32)> {
    let p = [PREFIX, RETIRED].into_iter().find(|p| l.starts_with(p))?;
    let u = crate::legacy::TileId::parse(&l[p.len()..]).filter(|u| u.z == PACK_Z)?;
    (format!("{p}{PACK_Z}-{}-{}", u.x, u.y) == l).then_some((u.x, u.y))
}

/// The deep packs the manifest names under tile `z/x/y` (z at most `PACK_Z`), and the retired
/// ones there (`RETIRED`).
pub fn named_under(m: &BTreeMap<String, String>, z: u8, x: u32, y: u32) -> Vec<String> {
    [PREFIX, RETIRED]
        .into_iter()
        .flat_map(|p| m.range(p.to_string()..).take_while(move |(l, _)| l.starts_with(p)))
        .filter(|(l, _)| pack_tile(l).is_some_and(|f| (f.0 >> (PACK_Z - z), f.1 >> (PACK_Z - z)) == (x, y)))
        .map(|(l, _)| l.clone())
        .collect()
}

/// The base tiles a deep target's tiles blend into and fill from (`make`): each place's finest base
/// zoom (`terrain_pack::max_zoom_at`) under z10 tile `t` and in the ring of one tile around them
/// (a deep tile at `t`'s edge reads the base across it: `padded`, the sampler's base), whichever z6
/// tile holds them.
pub fn base_want(t: (u32, u32)) -> std::collections::BTreeSet<(u8, u32, u32)> {
    let mut out = std::collections::BTreeSet::new();
    for z in PACK_Z..=crate::terrain_base::MAX_Z {
        let s = z - PACK_Z;
        let n = 1i64 << z;
        let k = 1i64 << s;
        for dy in -1..=k {
            let y = ((t.1 as i64) << s) + dy;
            if !(0..n).contains(&y) || crate::terrain_pack::max_zoom_at(crate::terrain_pack::tile_lat(z, y as u32)) != z
            {
                continue;
            }
            for dx in -1..=k {
                let x = (((t.0 as i64) << s) + dx).rem_euclid(n);
                out.insert((z, x as u32, y as u32));
            }
        }
    }
    out
}

/// The base tiles under z8 tile `t` of a z6 tile's tiles `hi` as just made, and the ring of one tile
/// around them where `hi` has it: what its deep levels coarser than `FIRST_Z` read (`hi_levels`).
fn hi_base(hi: &[crate::terrain_pack::Made], t: (u32, u32)) -> HashMap<(u8, u32, u32), Vec<u8>> {
    hi.iter()
        .filter(|&&(z, x, y, _)| {
            z >= SUB_Z
                && z == crate::terrain_pack::max_zoom_at(crate::terrain_pack::tile_lat(z, y))
                && near(z, x, t.0)
                && near(z, y, t.1)
        })
        .map(|(z, x, y, b)| ((*z, *x, *y), b.clone()))
        .collect()
}

/// Whether column (or row) `v` of zoom `z` lies under z8 tile column `t`, or one tile past it
/// (columns across the antimeridian too).
fn near(z: u8, v: u32, t: u32) -> bool {
    let s = z - SUB_Z;
    let n = 1i64 << SUB_Z;
    let c = |v: i64| (v >> s).rem_euclid(n);
    [c(v as i64 - 1), c(v as i64), c(v as i64 + 1)].contains(&(t as i64))
}

/// Whether tile `z/x/y` lies under tile `t` of zoom `tz`.
fn under(z: u8, x: u32, y: u32, tz: u8, t: (u32, u32)) -> bool {
    z >= tz && (x >> (z - tz), y >> (z - tz)) == t
}

/// What a deep target's (or a z6 tile's deep levels for its hi pack) came to.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Report {
    pub base_tiles: usize,
    pub with_data: usize,
    pub tiles: usize,
    pub packs: usize,
    pub dropped: usize,
    pub bytes: u64,
}

/// What a z6 tile's deep levels coarser than `FIRST_Z` read besides its own tiles (north of 67°,
/// where the base ends at z11: `hi_levels`): the fine datasets (None: no elevation version
/// accepted), the coverage (a z8 tile further than 20 km from it makes none), and the fetching of
/// the datasets' files.
pub struct Deep<'a> {
    pub fine: Option<&'a Fine>,
    pub cov: &'a crate::coverage::Coverage,
    pub fetch: &'a dyn crate::fetch::Fetch,
}

/// Whether z8 tile `t`'s base ends before `FIRST_Z - 1` somewhere (north of 67°): its deep levels
/// coarser than `FIRST_Z` go in its z6 tile's hi pack (`hi_levels`).
fn short(t: (u32, u32)) -> bool {
    let rows = 1u32 << (12 - SUB_Z);
    [t.1 * rows, (t.1 + 1) * rows - 1]
        .iter()
        .any(|&r| crate::terrain_pack::max_zoom_at(crate::terrain_pack::tile_lat(12, r)) < FIRST_Z - 1)
}

/// Z6 tile `t`'s deep levels coarser than `FIRST_Z` (z12 north of 67°, where the base ends at
/// z11), from its tiles `hi` as just made and the water `water`, for its hi pack, where every reader
/// looks for z9–12 (crate::terrain_pack::ManifestTiles, agent::tiles, the server): each z8 tile near
/// the coverage that a fine dataset meets and whose base ends short of them (`short`). Its deep
/// levels from `FIRST_Z` on are its z10 tiles' terrain-deep targets' (`make`).
pub fn hi_levels(
    out: &Out,
    t: (u32, u32),
    hi: &[crate::terrain_pack::Made],
    water: Option<&dyn crate::terrain_water::WaterSource>,
    deep: &Deep,
    progress: crate::rawpack::Progress,
) -> Result<(Report, Vec<crate::terrain_pack::Made>)> {
    let mut rep = Report::default();
    let mut more = Vec::new();
    let Some(fine) = deep.fine else { return Ok((rep, more)) };
    let k = SUB_Z - 6;
    let subs: Vec<(u32, u32)> = (t.0 << k..(t.0 + 1) << k)
        .flat_map(|x| (t.1 << k..(t.1 + 1) << k).map(move |y| (x, y)))
        .filter(|&s| short(s) && fine.meets(s) && crate::terrain_pack::near_coverage(deep.cov, SUB_Z, s.0, s.1, 20.0))
        .collect();
    for (i, &s) in subs.iter().enumerate() {
        let base = hi_base(hi, s);
        let inside: Vec<(u8, u32, u32)> = base.keys().copied().filter(|&(z, x, y)| under(z, x, y, SUB_Z, s)).collect();
        let store = out.root().join("sources/fetched");
        let r = make_tiles(
            s,
            &inside,
            &mut |_| Ok(base.clone()),
            water,
            fine,
            deep.fetch,
            &store,
            0..FIRST_Z,
            None,
            &mut |tiles| {
                more.extend(tiles);
                Ok(())
            },
        )?;
        rep.base_tiles += r.base_tiles;
        rep.with_data += r.with_data;
        rep.tiles += r.tiles;
        rep.bytes += r.bytes;
        progress("deep z8 tiles", i as u64 + 1, subs.len() as u64);
    }
    more.sort_by_key(|t| (t.0, t.1, t.2));
    Ok((rep, more))
}

/// Whether z10 tile `t` is a terrain-deep target: near the coverage (20 km, as its z8 tile), a fine
/// dataset meets its z8 tile, and one of its base tiles reaches `FIRST_Z` (`zooms`).
pub fn is_target(sets: &[FineSet], cov: &crate::coverage::Coverage, t: (u32, u32)) -> bool {
    let s = (t.0 >> (PACK_Z - SUB_Z), t.1 >> (PACK_Z - SUB_Z));
    crate::terrain_pack::near_coverage(cov, PACK_Z, t.0, t.1, 20.0)
        && crate::terrain_pack::near_coverage(cov, SUB_Z, s.0, s.1, 20.0)
        && zooms(sets, t).iter().any(|&(_, zf)| zf >= FIRST_Z)
}

/// The terrain-deep targets under z6 tile `t` (`is_target`).
pub fn targets_in(sets: &[FineSet], cov: &crate::coverage::Coverage, t: (u32, u32)) -> Vec<(u32, u32)> {
    let k = PACK_Z - 6;
    if !reaches(sets, 6, t.0, t.1) {
        return Vec::new();
    }
    (t.0 << k..(t.0 + 1) << k)
        .flat_map(|x| (t.1 << k..(t.1 + 1) << k).map(move |y| (x, y)))
        .filter(|&u| is_target(sets, cov, u))
        .collect()
}

/// The base tiles under z10 tile `t` (each place's finest base zoom) a fine dataset meeting its z8
/// tile is finer than, with the zoom it reaches there (`make`'s).
fn zooms(sets: &[FineSet], t: (u32, u32)) -> Vec<((u8, u32, u32), u8)> {
    let s = (t.0 >> (PACK_Z - SUB_Z), t.1 >> (PACK_Z - SUB_Z));
    let b = crate::stage::tile_box_grown(SUB_Z, s.0, s.1, 0.0);
    let here = meeting(sets, b, poleward(b));
    if here.is_empty() {
        return Vec::new();
    }
    base_want(t)
        .into_iter()
        .filter(|&(z, x, y)| under(z, x, y, PACK_Z, t))
        .filter_map(|bt| zoom_here(&here, bt).map(|zf| (bt, zf)))
        .collect()
}

/// The finest zoom datasets `here` reach in base tile `b` (at its middle latitude), where past its own.
fn zoom_here(here: &[&FineSet], b: (u8, u32, u32)) -> Option<u8> {
    let (z, x, y) = b;
    let bb = crate::stage::tile_box_grown(z, x, y, 0.0);
    let lat = crate::terrain_pack::tile_lat(z, y);
    here.iter().filter(|s| s.cover.meets(bb)).map(|s| s.detail.zoom_at(lat)).max().filter(|&f| f > z)
}

/// What deep target `t`'s key names of the deep levels' code and the fine datasets: their code
/// (`versions`), the datasets meeting its z8 tile (its plans are that box's) and its z10 tile with
/// the zoom each reaches there (`key_lines`), and whether each z10 tile around it goes past the
/// base's zoom (its ring's base tiles rounded as coarser than native there:
/// crate::terrain_pack::Sources::deeper).
pub fn target_key_lines(sets: &[FineSet], t: (u32, u32)) -> String {
    let s = (t.0 >> (PACK_Z - SUB_Z), t.1 >> (PACK_Z - SUB_Z));
    let mut out = vec![versions(), format!("first {FIRST_Z}")];
    out.push(format!("{SUB_Z}/{}/{} {}", s.0, s.1, key_lines(sets, SUB_Z, s.0, s.1).join(" | ")));
    out.push(format!("{PACK_Z}/{}/{} {}", t.0, t.1, key_lines(sets, PACK_Z, t.0, t.1).join(" | ")));
    let n = 1i64 << PACK_Z;
    let mut ring = String::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = ((t.0 as i64 + dx).rem_euclid(n) as u32, t.1 as i64 + dy);
            let d = (0..n).contains(&y) && zoom_of(sets, PACK_Z, x, y as u32).is_some();
            ring.push(if d { 'd' } else { '-' });
        }
    }
    out.push(format!("deeper {ring}"));
    out.join("\n")
}

/// The base deep target `t` reads (`base_want`; with `near`, those near its base tiles with data
/// alone, `near_tile`: all its deep tiles read), made as the terrain-hi runs of their z6 tiles make
/// them (`bases`: crate::terrain_pack::Bases), by tile.
pub fn target_base(
    bases: &crate::terrain_pack::Bases,
    t: (u32, u32),
    near: Option<&[(u8, u32, u32)]>,
) -> Result<HashMap<(u8, u32, u32), Vec<u8>>> {
    let mut by: BTreeMap<(u32, u32), std::collections::BTreeSet<(u8, u32, u32)>> = BTreeMap::new();
    for (z, x, y) in base_want(t) {
        if near.is_some_and(|n| !n.iter().any(|&d| near_tile((z, x, y), d))) {
            continue;
        }
        by.entry((x >> (z - 6), y >> (z - 6))).or_default().insert((z, x, y));
    }
    let mut out = HashMap::new();
    for (p, want) in by {
        out.extend(bases.of(p, &want)?.into_iter().map(|(z, x, y, b)| ((z, x, y), b)));
    }
    Ok(out)
}

/// Deep target `t` made (`make`, from its base: `target_base`) and put: its deep pack, else (no
/// tile made) the deep pack and a retired one there dropped (not saved).
pub fn build_target(
    out: &mut Out,
    t: (u32, u32),
    bases: &crate::terrain_pack::Bases,
    water: Option<&dyn crate::terrain_water::WaterSource>,
    fine: &Fine,
    fetch: &dyn crate::fetch::Fetch,
) -> Result<Report> {
    build_target_kept(out, t, bases, water, fine, fetch, crate::resume_log::on())
}

/// `build_target`, its base tiles' deep tiles kept as made and those an earlier run kept taken
/// with `keep` (crate::terrain_resume::DeepLog; its log deleted once its pack is put).
pub(crate) fn build_target_kept(
    out: &mut Out,
    t: (u32, u32),
    bases: &crate::terrain_pack::Bases,
    water: Option<&dyn crate::terrain_water::WaterSource>,
    fine: &Fine,
    fetch: &dyn crate::fetch::Fetch,
    keep: bool,
) -> Result<Report> {
    let mut log = keep
        .then(|| {
            let pieces = base_want(t).into_iter().map(|(z, x, y)| (x >> (z - 6), y >> (z - 6))).collect();
            let id = crate::terrain_resume::deep_id(&out.manifest, bases.sources(), water, fine, t, &pieces);
            crate::terrain_resume::DeepLog::open(&out.scratch, t, &id)
        })
        .flatten();
    let mut base_of = |data: &[(u8, u32, u32)]| {
        let _p =
            crate::timings::sub("its base made (the pool sampled, its lakes levelled)", crate::timings::Class::Mixed);
        target_base(bases, t, Some(data))
    };
    let p = crate::timings::phase("its deep tiles made (the fine datasets read)", crate::timings::Class::Mixed);
    let (mut rep, pack) = make(out, t, &mut base_of, water, fine, fetch, log.as_mut())?;
    drop(p);
    let mut gone = named_under(&out.manifest, PACK_Z, t.0, t.1);
    if let Some(p) = pack {
        gone.retain(|l| *l != p.logical);
        let _p = crate::timings::phase("its deep pack uploaded", crate::timings::Class::NasWrite);
        out.put_file(&p.logical, "pack", &p.local)?;
    }
    rep.dropped = crate::terrain_pack::drop_named(out, &gone);
    std::fs::remove_file(crate::resume_log::path(&out.scratch, (PACK_Z, t.0, t.1))).ok();
    Ok(rep)
}

/// Z10 tile `t` no deep target any more (no fine dataset there, or the coverage gone): its deep
/// pack and a retired one there dropped (not saved); how many.
pub fn drop_target(out: &mut Out, t: (u32, u32)) -> usize {
    let gone = named_under(&out.manifest, PACK_Z, t.0, t.1);
    crate::terrain_pack::drop_named(out, &gone)
}

/// Z10 tile `t`'s deep tiles (`FIRST_Z` to `MAX_Z`) of its base tiles (each place's finest base zoom
/// under it) with data, from the base `base_of` gives for those (the base tiles near them, as their
/// terrain-hi runs store them: `target_base`), the water `water` and the fine datasets `fine`: its
/// deep pack (`pack_logical`) written here, not uploaded; None where it makes no tile.
pub fn make(
    out: &Out,
    t: (u32, u32),
    base_of: &mut dyn FnMut(&[(u8, u32, u32)]) -> Result<HashMap<(u8, u32, u32), Vec<u8>>>,
    water: Option<&dyn crate::terrain_water::WaterSource>,
    fine: &Fine,
    fetch: &dyn crate::fetch::Fetch,
    log: Option<&mut crate::terrain_resume::DeepLog>,
) -> Result<(Report, Option<crate::layers::LocalPack>)> {
    let s = (t.0 >> (PACK_Z - SUB_Z), t.1 >> (PACK_Z - SUB_Z));
    let inside: Vec<(u8, u32, u32)> = base_want(t).into_iter().filter(|&(z, x, y)| under(z, x, y, PACK_Z, t)).collect();
    let mut tiles = Vec::new();
    let store = out.root().join("sources/fetched");
    let mut rep = make_tiles(s, &inside, base_of, water, fine, fetch, &store, FIRST_Z..MAX_Z + 1, log, &mut |made| {
        tiles.extend(made);
        Ok(())
    })?;
    if tiles.is_empty() {
        return Ok((rep, None));
    }
    tiles.sort_by_key(|t| (t.0, t.1, t.2));
    let mut it = tiles.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let p = crate::layers::write_pack_local(
        out,
        "terrain",
        crate::terrain_codec::ENCODING,
        false,
        "deep",
        (PACK_Z, t.0, t.1),
        &mut it,
    )?;
    rep.packs = p.iter().count();
    Ok((rep, p))
}

/// The base tiles a deep tile reads, or a base tile's tiles in `make_tiles`: a base tile `t` and every
/// tile within one of its own tiles of it, whatever zoom (its ring, its padding's other zooms, the
/// sampler's reads of the base past its edge, a finer base tile under it), x across the
/// antimeridian too.
fn near_tile(a: (u8, u32, u32), t: (u8, u32, u32)) -> bool {
    let zm = a.0.max(t.0);
    let n = 1i64 << zm;
    let span = |(z, x, y): (u8, u32, u32), grow: i64| {
        let k = zm - z;
        (
            (((x as i64) - grow) << k, ((x as i64) + 1 + grow) << k),
            (((y as i64) - grow) << k, ((y as i64) + 1 + grow) << k),
        )
    };
    let ((ax0, ax1), (ay0, ay1)) = span(a, 0);
    let ((tx0, tx1), (ty0, ty1)) = span(t, 1);
    ay0 < ty1 && ty0 < ay1 && [-n, 0, n].iter().any(|d| ax0 < tx1 + d && tx0 + d < ax1)
}

/// The deep tiles of zooms `zs` of base tiles `inside` (each place's finest base zoom: of z8 tile
/// `s`, whose box the datasets are planned over, so each blend's band is the same whichever part of
/// it is made), each z10 tile's given to `sink` as made. First the presence grids, which read the
/// fine datasets alone, say which of `inside` have data; then `base_of` gives the base those read
/// (as their terrain-hi runs store it: each of those it has, and the tiles near them,
/// `near_tile`), none asked for where no tile has data, and the deep tiles are made of those it
/// has. The datasets' blocks read ahead z10 tile by z10 tile, each once (`read_ahead`), its base
/// tiles made at once.
#[allow(clippy::too_many_arguments)]
fn make_tiles(
    s: (u32, u32),
    inside: &[(u8, u32, u32)],
    base_of: &mut dyn FnMut(&[(u8, u32, u32)]) -> Result<HashMap<(u8, u32, u32), Vec<u8>>>,
    water: Option<&dyn crate::terrain_water::WaterSource>,
    fine: &Fine,
    fetch: &dyn crate::fetch::Fetch,
    store: &store::nasfs::NasPath,
    zs: std::ops::Range<u8>,
    mut log: Option<&mut crate::terrain_resume::DeepLog>,
    sink: &mut dyn FnMut(Vec<(u8, u32, u32, Vec<u8>)>) -> Result<()>,
) -> Result<Report> {
    let mut rep = Report { base_tiles: inside.len(), ..Report::default() };
    // (The base tiles an earlier run kept, with their deep tiles: neither their presence sampled
    // nor their data read again.)
    let kept = |t: &(u8, u32, u32)| log.as_ref().is_some_and(|l| l.kept.contains_key(t));
    let b = crate::stage::tile_box_grown(SUB_Z, s.0, s.1, 0.0);
    let plans = if inside.is_empty() { None } else { fine.plans(b)? };
    let Some((presence, full)) = plans else { return Ok(rep) };
    // (By z10 tile, its base tiles some fine dataset is finer than, with the zoom they reach, those
    // reaching `zs`.)
    let here = meeting(&fine.sets, b, poleward(b));
    let mut by_pack: BTreeMap<(u32, u32), Vec<((u8, u32, u32), u8)>> = BTreeMap::new();
    for &bt in inside {
        let e = by_pack.entry((bt.1 >> (bt.0 - PACK_Z), bt.2 >> (bt.0 - PACK_Z))).or_default();
        if let Some(zf) = zoom_here(&here, bt).filter(|&zf| zf >= zs.start && bt.0 + 1 < zs.end) {
            e.push((bt, zf));
        }
    }
    let groups: Vec<Vec<((u8, u32, u32), u8)>> = by_pack
        .into_values()
        .filter(|g| !g.is_empty())
        .map(|mut g| {
            g.sort();
            g
        })
        .collect();
    let n = groups.len();
    if n == 0 {
        return Ok(rep);
    }
    let bf = BaseFetch::new(fetch);
    let o = Opts { fetch: &bf, store: Some(store.clone()) };
    let presence_s = Sampler::new(&presence, &o)?;
    let full_s = Sampler::sharing(&full, &o, &presence_s)?;
    // (Its files' reads planned over its base tiles' box, not the z8 tile's.)
    let area = boxes(groups.iter().flatten().map(|g| g.0));
    // Which base tiles their datasets have data in, their blocks read ahead by z10 tile; then the
    // deep tiles of those, theirs read ahead likewise, each block once.
    let tiles_of = |keep: &dyn Fn(usize, usize) -> bool| -> Vec<Vec<(u8, u32, u32)>> {
        groups
            .iter()
            .enumerate()
            .map(|(g, ts)| (0..ts.len()).filter(|&i| keep(g, i)).map(|i| ts[i].0).collect())
            .collect()
    };
    let todo: Vec<Vec<bool>> = groups.iter().map(|g| g.iter().map(|(t, _)| !kept(t)).collect()).collect();
    let items = plan_items(&plan_files(&presence, area, &bf)?, &tiles_of(&|g, i| todo[g][i]))?;
    let mut with: Vec<Vec<bool>> = vec![Vec::new(); n];
    read_ahead(&items, n, false, |g| {
        with[g] = groups[g]
            .par_iter()
            .zip(&todo[g])
            .map(|(&(t, _), &d)| if d { has_data(&presence_s, t) } else { Ok(true) })
            .collect::<Result<_>>()?;
        Ok(())
    })?;
    let data: Vec<(u8, u32, u32)> = tiles_of(&|g, i| with[g][i] && todo[g][i]).into_iter().flatten().collect();
    // The base, of the tiles with data and those near them; of those with data, those it has.
    let base = if data.is_empty() { HashMap::new() } else { base_of(&data)? };
    for (g, ts) in groups.iter().enumerate() {
        for (i, (t, _)) in ts.iter().enumerate() {
            with[g][i] &= !todo[g][i] || base.contains_key(t);
        }
    }
    rep.with_data = with.iter().flatten().filter(|&&d| d).count();
    if rep.with_data == 0 {
        return Ok(rep);
    }
    let decoded: HashMap<(u8, u32, u32), Vec<f32>> =
        base.par_iter().map(|(&k, b)| Ok((k, roadcore::grid::decode_terrain(b)?))).collect::<Result<_>>()?;
    // (The base served to the sampler as a Terrarium pyramid of PNGs: crate::dem::encoding.)
    bf.set_base(
        decoded
            .par_iter()
            .map(|(&k, e)| Ok((k, roadcore::grid::encode_terrain_png(e, 256, 256)?)))
            .collect::<Result<_>>()?,
    );
    let items = plan_items(&plan_files(&full, area, &bf)?, &tiles_of(&|g, i| with[g][i] && todo[g][i]))?;
    bf.keep_only(&items);
    read_ahead(&items, n, true, |k| {
        let mine: Vec<(u8, u32, u32)> =
            groups[k].iter().zip(&with[k]).filter(|(_, &d)| d).map(|((t, _), _)| *t).collect();
        let mut tiles = Vec::new();
        for t in &mine {
            if let Some(l) = log.as_mut() {
                tiles.extend(l.kept.remove(t).unwrap_or_default());
            }
        }
        // (Each base tile's deep tiles kept as it's made: made on the pool's threads, kept here.)
        let (tx, rx) = std::sync::mpsc::channel();
        let made = std::thread::scope(|s| -> Result<()> {
            s.spawn(|| {
                groups[k].par_iter().zip(&with[k]).zip(&todo[k]).filter(|((_, &d), &t)| d && t).for_each_with(
                    tx,
                    |tx, ((&(t, zf), _), _)| {
                        tx.send((t, deep_tiles(&full_s, water, &decoded, t, zf, &zs))).ok();
                    },
                )
            });
            for (t, r) in rx {
                let r = r?;
                if let Some(l) = log.as_mut() {
                    l.keep(t, &r)?;
                }
                tiles.extend(r);
            }
            Ok(())
        });
        made?;
        rep.tiles += tiles.len();
        rep.bytes += tiles.iter().map(|t| t.3.len() as u64).sum::<u64>();
        sink(tiles)
    })?;
    Ok(rep)
}

/// The box (west, south, east, north) of tiles `ts`.
fn boxes(ts: impl Iterator<Item = (u8, u32, u32)>) -> [f64; 4] {
    let mut b = [180.0f64, 90.0f64, -180.0f64, -90.0f64];
    for (z, x, y) in ts {
        let t = crate::stage::tile_box_grown(z, x, y, 0.0);
        b = [b[0].min(t[0]), b[1].min(t[1]), b[2].max(t[2]), b[3].max(t[3])];
    }
    b
}

/// A base tile's samples: the points of `tiles` (each 256 × 256 pixel centres) of zoom `z`.
fn points(z: u8, tiles: &[(u32, u32)]) -> (Vec<f64>, Vec<f64>) {
    let (mut lon, mut lat) = (Vec::with_capacity(tiles.len() << 16), Vec::with_capacity(tiles.len() << 16));
    for &(x, y) in tiles {
        for j in 0..256 {
            for i in 0..256 {
                let (lo, la) = pixel_lonlat(z, x, y, i, j);
                lon.push(lo);
                lat.push(la);
            }
        }
    }
    (lon, lat)
}

/// Whether base tile `b`'s datasets have data in it at all: a grid of it (`PRESENCE`), as they're
/// read for the roads (`presence`).
fn has_data(presence: &Sampler, b: (u8, u32, u32)) -> Result<bool> {
    let (zb, bx, by) = b;
    let n = PRESENCE as f64;
    let nn = (1u64 << zb) as f64;
    let (mut lon, mut lat) = (Vec::with_capacity(PRESENCE * PRESENCE), Vec::with_capacity(PRESENCE * PRESENCE));
    for j in 0..PRESENCE {
        for i in 0..PRESENCE {
            let fx = (bx as f64 + (i as f64 + 0.5) / n) / nn;
            let fy = (by as f64 + (j as f64 + 0.5) / n) / nn;
            lon.push(fx * 360.0 - 180.0);
            lat.push((std::f64::consts::PI * (1.0 - 2.0 * fy)).dsinh().datan().to_degrees());
        }
    }
    Ok(presence.sample(&lon, &lat, &|_| {})?.elev.iter().any(|v| v.is_finite()))
}

/// Base tile `b`'s deep tiles to zoom `zf`, those of zooms `zs`, with data of a fine dataset (`full`;
/// `tiles`: the base tiles decoded, its neighbours with it).
fn deep_tiles(
    full: &Sampler,
    water: Option<&dyn crate::terrain_water::WaterSource>,
    tiles: &HashMap<(u8, u32, u32), Vec<f32>>,
    b: (u8, u32, u32),
    zf: u8,
    zs: &std::ops::Range<u8>,
) -> Result<Vec<(u8, u32, u32, Vec<u8>)>> {
    let (zb, bx, by) = b;
    let base = padded(tiles, b);
    let share = water_share(water, b)?;
    let mut out = Vec::new();
    for z in (zb + 1..=zf).filter(|z| zs.contains(z)) {
        let k = 1u32 << (z - zb);
        // (Not those under a finer base tile, where the base's finest zoom changes (79°): its own
        // deep levels make them.)
        let finer = |x: u32, y: u32| (zb + 1..=z).any(|zz| tiles.contains_key(&(zz, x >> (z - zz), y >> (z - zz))));
        let ts: Vec<(u32, u32)> = (by * k..(by + 1) * k)
            .flat_map(|y| (bx * k..(bx + 1) * k).map(move |x| (x, y)))
            .filter(|&(x, y)| !finer(x, y))
            .collect();
        // (A chunk of tiles a call: 1M points at most, ~30 MB a thread.)
        for chunk in ts.chunks(16) {
            let (lon, lat) = points(z, chunk);
            let s = full.sample(&lon, &lat, &|_| {})?;
            for (c, &(x, y)) in chunk.iter().enumerate() {
                let r = c << 16..(c + 1) << 16;
                if !s.src[r.clone()].iter().any(|&c| c != 0) {
                    continue;
                }
                // (Where only the base gives a height, the base read across its edges: `finish`.)
                let mut e: Vec<f32> =
                    r.map(|k| if s.src[k] == 0 && !s.blended[k] { f32::NAN } else { s.elev[k] }).collect();
                finish(&mut e, &base, share.as_deref(), zb, z, x, y);
                out.push((z, x, y, encode(&mut e)?));
            }
        }
    }
    Ok(out)
}

/// Base tile `b`'s share of water at each pixel (0–1: sea and lakes), where it has any.
fn water_share(water: Option<&dyn crate::terrain_water::WaterSource>, b: (u8, u32, u32)) -> Result<Option<Vec<f32>>> {
    let Some(w) = water else { return Ok(None) };
    let Some(wt) = crate::terrain_water::tile_water(w, b.0, b.1, b.2)? else { return Ok(None) };
    let whole = crate::terrain_water::FULL as f32;
    Ok(Some(wt.sea.iter().zip(&wt.lake).map(|(&s, &l)| (s as f32 + l as f32) / whole).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datasets_zoom_has_pixels_no_finer_than_half_its_cell() {
        // 2 m: z16 through Canada (z16's pixels 1.7 m at 45°, 1.2 m at 60°; z17's under 1 m).
        assert_eq!(zoom_for(2.0, 45.0), 16);
        assert_eq!(zoom_for(2.0, 60.0), 16);
        assert_eq!(zoom_for(2.0, 70.0), 15);
        // 1 m: z16 too, the cap (z17 down to 65° N without it).
        assert_eq!((zoom_for(1.0, 45.0), zoom_for(1.0, 70.0), Detail::Pyramid(18).zoom_at(45.0)), (16, 16, 16));
        // 3DEP's 10.3 m: z14 to ~55°, z13 north of it.
        assert_eq!(zoom_for(10.3, 40.0), 14);
        assert_eq!(zoom_for(10.3, 65.0), 13);
        // 30 m: the base's z12 at mid-latitudes.
        assert_eq!(zoom_for(30.0, 45.0), 12);
        for lat in [0.0, 30.0, 50.0, 70.0] {
            let z = zoom_for(5.0, lat);
            assert!(tile_resolution(z as u32, lat) >= 2.5 && tile_resolution(z as u32 + 1, lat) < 2.5);
        }
        assert_eq!(Detail::Pyramid(15).zoom_at(35.0), 15);
        assert_eq!(Detail::Raster(2000).zoom_at(45.0), 16);
    }

    fn set(id: &str, detail: Detail, cells: &[(i32, i32)]) -> FineSet {
        FineSet {
            id: id.into(),
            line: format!("{id} k"),
            cover: Cover { world: false, cells: cells.iter().copied().collect() },
            detail,
            cut: None,
        }
    }

    #[test]
    fn the_keys_name_the_fine_datasets_meeting_a_tile_and_the_zoom_they_reach() {
        let sets =
            vec![set("lidar", Detail::Raster(2000), &[(-75, 45)]), set("coarse", Detail::Raster(30000), &[(-75, 45)])];
        // 6/18/22 holds 75°W 45.5°N; z8 tile 8/74/91 too.
        assert_eq!(key_lines(&sets, 6, 18, 22), vec!["lidar k zoom 16".to_string()]);
        assert_eq!(key_lines(&sets, 8, 74, 91), vec!["lidar k zoom 16".to_string()]);
        assert!(key_lines(&sets, 6, 10, 22).is_empty(), "no fine dataset there: no target");
        // (Its measured resolution moving moves the key.)
        let read_coarser = vec![set("lidar", Detail::Raster(8000), &[(-75, 45)])];
        assert_ne!(key_lines(&read_coarser, 8, 74, 91), key_lines(&sets, 8, 74, 91));
        assert_eq!(zoom_of(&sets, 10, 299, 364), Some(16));
        assert_eq!(zoom_of(&sets, 10, 40, 364), None);
    }

    #[test]
    fn the_datasets_finer_than_the_terrains_are_fine() {
        let d = tempfile::tempdir().unwrap();
        let mut m = std::collections::BTreeMap::new();
        crate::inputs::elevation::accept_fixture(&store::nasfs::NasRoot::local_for_test(d.path()), &mut m);
        let a =
            crate::inputs::open(&store::nasfs::NasRoot::local_for_test(d.path()), &m, crate::inputs::elevation::UNIT)
                .unwrap()
                .unwrap();
        let mut ds = declared::accepted(&a).unwrap();
        assert!(sets(&ds).is_empty(), "the fixture's datasets are no finer than the terrain's base");
        assert!(Fine::of(a.clone(), Vec::new()).unwrap().sets.is_empty());
        // (A resolution finer than the base's makes it fine.)
        ds[0].measured.resolution = 2.0;
        let s = sets(&ds);
        assert_eq!(s.len(), 1);
        let Data::Tiles { zoom, .. } = &ds[0].declaration.data else { panic!("the fixture's is a pyramid") };
        assert_eq!(s[0].detail, Detail::Pyramid(*zoom as u8));
        assert_eq!(s[0].line, declared::line(&ds[0]));
    }

    #[test]
    fn a_deep_tile_takes_the_base_where_it_has_no_data_and_on_water() {
        // The base: 10 m plus its column; its lower half water.
        let (bx, by) = (100, 200);
        let one: Vec<f32> = (0..65536).map(|k| 10.0 + (k % 256) as f32).collect();
        let base = padded(&HashMap::from([((12, bx, by), one)]), (12, bx, by));
        let mut water = vec![0f32; 65536];
        for v in water.iter_mut().skip(128 * 256) {
            *v = 1.0;
        }
        // Fine tile 13/2bx/2by+1, under the base's lower half: data everywhere, but water.
        let mut e = vec![500.04f32; 65536];
        finish(&mut e, &base, Some(&water), 12, 13, bx * 2, by * 2 + 1);
        assert_eq!(e[100 * 256], 10.0, "the water's flattened height, the base's");
        // Fine tile 13/2bx/2by, dry: its data stored to the centimetre, the base past it.
        let mut e = vec![f32::NAN; 65536];
        e[0] = 500.044;
        finish(&mut e, &base, Some(&water), 12, 13, bx * 2, by * 2);
        // (Pixel 3 lies at the base's column 1.25.)
        assert!((e[3] - 11.25).abs() < 0.06, "{}", e[3]);
        let b = encode(&mut e).unwrap();
        assert!((e[0] - 500.04).abs() < 1e-3, "{}", e[0]);
        let d = roadcore::grid::decode_terrain(&b).unwrap();
        assert!(d.iter().zip(&e).all(|(a, b)| (a - b).abs() <= 1.0 / 512.0), "1 cm steps kept to Terrarium's 1/256 m");
    }

    #[test]
    fn the_deep_packs_under_a_tile() {
        let m: BTreeMap<String, String> = [
            ("layers/terrain/deep/10-288-352", "a"),
            ("layers/terrain/deep/10-303-367", "b"),
            ("layers/terrain/deep/10-304-352", "c"),
            ("layers/terrain/deep/10-0288-352", "d"),
            ("layers/terrain/hi/6-18-22", "e"),
            // (A retired pack: dropped with them.)
            ("layers/terrain/fine/10-289-352", "f"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(
            named_under(&m, 6, 18, 22),
            vec!["layers/terrain/deep/10-288-352", "layers/terrain/deep/10-303-367", "layers/terrain/fine/10-289-352"]
        );
        assert_eq!(
            named_under(&m, 8, 72, 88),
            vec!["layers/terrain/deep/10-288-352", "layers/terrain/fine/10-289-352"]
        );
        assert_eq!(pack_tile("layers/terrain/deep/10-288-352"), Some((288, 352)));
        assert_eq!(pack_tile("layers/terrain/deep/9-1-1"), None);
        // A deep target saves its own deep pack (and drops a retired one there), not another's; a
        // terrain-hi z6 tile none.
        let saves = |s: &str, t: &str, l: &str| crate::agent::steps::saves(s, t, l);
        assert!(
            saves("terrain-deep", "10/288/352", "layers/terrain/deep/10-288-352")
                && saves("terrain-deep", "10/289/352", "layers/terrain/fine/10-289-352")
        );
        assert!(!saves("terrain-deep", "10/288/353", "layers/terrain/deep/10-288-352"));
        assert!(
            !saves("terrain-hi", "6/18/22", "layers/terrain/deep/10-288-352")
                && saves("terrain-hi", "6/18/22", "layers/terrain/hi/6-18-22")
        );
    }

    #[test]
    fn the_base_meets_its_neighbours_across_a_base_tiles_edge() {
        // Base tile 12/x/y at 50 m, its east neighbour at 80 m; all water, so a fine tile is the
        // base's alone.
        let (x, y) = (1199, 1477);
        let tiles = HashMap::from([((12, x, y), vec![50f32; 65536]), ((12, x + 1, y), vec![80f32; 65536])]);
        let water = vec![1f32; 65536];
        let fine = |b: (u8, u32, u32), fx: u32| {
            let mut e = vec![f32::NAN; 65536];
            finish(&mut e, &padded(&tiles, b), Some(&water), 12, 16, fx, y * 16);
            e
        };
        // Z16 tiles either side of the edge: the last column of the west one, the first of the east.
        let (w, e) = (fine((12, x, y), x * 16 + 15), fine((12, x + 1, y), (x + 1) * 16));
        let (a, b) = (w[255], e[0]);
        // (A z16 pixel is a sixteenth of a base pixel: 30 m ÷ 16 apart.)
        assert!(a > 50.0 && b < 80.0 && (b - a) < 2.0, "a ramp across the edge, not a step: {a} → {b}");
        assert!(((a + b) / 2.0 - 65.0).abs() < 0.01, "halfway between the base tiles at the edge: {a} {b}");
        // (The west edge has no neighbour stored: its own edge.)
        assert_eq!(fine((12, x, y), x * 16)[0], 50.0);
        // The ring around a z8 tile: under it or a tile past it.
        let t = (74u32, 92u32);
        assert!(near(12, 74 * 16 - 1, t.0) && near(12, 75 * 16, t.0) && near(12, 74 * 16, t.0));
        assert!(!near(12, 74 * 16 - 2, t.0) && !near(12, 75 * 16 + 1, t.0));
    }

    #[test]
    fn the_base_meets_a_neighbour_of_another_zoom_as_the_server_and_the_map_make_it() {
        // Where the base's finest zoom changes with latitude (z12 to 67°, z11 past it): z12 tile
        // 12/x/y, its north neighbour stored at z11 only. Heights rising 1 m a z12 pixel northward.
        let rise = |z: u8, y: u32| -> Vec<f32> {
            let k = (1u32 << (12 - z)) as f32;
            (0..65536).map(|i| -((y as f32 * 256.0 + (i / 256) as f32 + 0.5) * k) + 5000.0).collect()
        };
        let (x, y) = (1000u32, 502u32);
        let tiles = HashMap::from([((12, x, y), rise(12, y)), ((11, x / 2, (y - 1) / 2), rise(11, (y - 1) / 2))]);
        let p = padded(&tiles, (12, x, y));
        // The padding's north row: the z11 tile at those pixel centres, bilinearly (a plane: exact),
        // as the server makes 12/x/(y-1) from it (roadcore::grid::upsampled), clamped inside it.
        let up = roadcore::grid::upsampled(&tiles[&(11, x / 2, (y - 1) / 2)], 1, x, y - 1);
        for i in 1..PADDED - 1 {
            assert!((p[i] - up[255 * 256 + i - 1]).abs() < 1e-3, "column {i}");
        }
        // (Its last pixel centres half a z12 pixel short of the padding's: the edge's, clamped.)
        assert!((p[1] - p[PADDED + 1] - 1.5).abs() < 1e-3);
        // The other way: a z11 tile's padding from a z12 neighbour, at the z11 pixel's centre (between
        // four z12 pixels: their mean).
        let tiles = HashMap::from([((11, 500, 250), rise(11, 250)), ((12, 1000, 502), rise(12, 502))]);
        let p = padded(&tiles, (11, 500, 250));
        let fine = &tiles[&(12, 1000, 502)];
        let want = (fine[0] + fine[1] + fine[256] + fine[257]) / 4.0;
        assert!((p[(PADDED - 1) * PADDED + 1] - want).abs() < 1e-3);
        // Across the antimeridian.
        assert!(near(12, 0, 255) && near(12, 4095, 0));
    }

    /// A file's reads, by offset and length.
    type Reads = Arc<Mutex<Vec<(u64, usize)>>>;

    struct Counted(crate::fetch::MapFetch, Reads);
    struct CountedFile(Arc<dyn store::range::RangeRead>, Reads);
    impl crate::fetch::Fetch for Counted {
        fn open(&self, url: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
            Ok(self.0.open(url)?.map(|f| Arc::new(CountedFile(f, self.1.clone())) as Arc<dyn store::range::RangeRead>))
        }
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            self.0.get(url)
        }
    }
    impl store::range::RangeRead for CountedFile {
        fn len(&self) -> Result<u64, store::IoError> {
            self.0.len()
        }
        fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::IoError> {
            self.1.lock().unwrap().push((off, len));
            self.0.read_at(off, len)
        }
    }

    #[test]
    fn a_targets_blocks_are_read_once_ahead_of_its_sampling_and_the_heights_are_the_same() {
        const PX: f64 = 1.0 / 7200.0;
        // A fine raster over base tile 12/2160/1400 and past it, in degrees.
        let t = (12u8, 2160u32, 1400u32);
        let tb = crate::stage::tile_box_grown(t.0, t.1, t.2, 0.0);
        let gt = [tb[0] - 0.01, PX, 0.0, tb[3] + 0.01, 0.0, -PX];
        read_ahead_samples_as_the_sampler(t, 4326, gt, PX * crate::dem::generic::M_PER_DEG);
    }

    #[test]
    fn a_utm_datasets_blocks_are_planned_read_ahead_and_sampled_where_they_lie() {
        // The same in NAD83 / UTM zone 18N (USGS's 1 m tiles' CRS), 10 m pixels, over base tile
        // 12/1194/1474 (75° W, 44.6° N) and past it: a block planned in the wrong place would be
        // read during the sampling, a point projected wrongly would sample other heights.
        let t = (12u8, 1194u32, 1474u32);
        let tb = crate::stage::tile_box_grown(t.0, t.1, t.2, 0.0);
        let utm = crate::dem::proj::Projection::of(26918).unwrap();
        let (cx, cy) = utm.project((tb[0] + tb[2]) / 2.0, (tb[1] + tb[3]) / 2.0);
        let gt = [(cx - 5120.0).floor(), 10.0, 0.0, (cy + 5120.0).floor(), 0.0, -10.0];
        read_ahead_samples_as_the_sampler(t, 26918, gt, 10.0);
    }

    /// Base tile `t`'s z13 points sampled after its target's blocks were read ahead (`plan_files`,
    /// `plan_items`, `read_ahead`) give the plain sampler's heights bit for bit, every block read
    /// once and none during the sampling: a fine raster (CRS `crs`, geotransform `gt`, `res` m
    /// pixels, 1024 px a side) with a hole of no data, over a coarse one in degrees (the blends
    /// read both).
    fn read_ahead_samples_as_the_sampler(t: (u8, u32, u32), crs: u16, gt: [f64; 6], res: f64) {
        use crate::dem::generic::M_PER_DEG;
        const PX: f64 = 1.0 / 7200.0;
        let tb = crate::stage::tile_box_grown(t.0, t.1, t.2, 0.0);
        let (w, h) = (1024usize, 1024usize);
        let nd = -9999.0f32;
        let mut data: Vec<f32> =
            (0..h).flat_map(|y| (0..w).map(move |x| 100.0 + 0.013 * x as f32 + 0.021 * y as f32)).collect();
        for y in 400..520 {
            for x in 300..450 {
                data[y * w + x] = nd;
            }
        }
        let tif = |w: usize, h: usize, d: &[f32], block: u32, gt: [f64; 6], crs: u16| {
            crate::geotiff::write_f32(w as u32, h as u32, d, block, gt, crs, Some(nd as f64)).unwrap()
        };
        let cgt = [tb[0] - 0.05, 3.0 * PX, 0.0, tb[3] + 0.05, 0.0, -3.0 * PX];
        let mut f = crate::fetch::MapFetch::default();
        f.0.insert("https://a.org/fine.tif".into(), Some(tif(w, h, &data, 256, gt, crs)));
        f.0.insert("https://b.org/coarse.tif".into(), Some(tif(1024, 1024, &vec![50.0f32; 1 << 20], 512, cgt, 4326)));
        let geo = |id: &str, code: u8, res: f64, crs: u16, url: &str| Dataset {
            convert: None,
            id: id.into(),
            code,
            resolution: res,
            cover: None,
            source: Source::Geotiff {
                crs,
                read_resolution: None,
                store: false,
                files: vec![(url.into(), None)],
                masks: Vec::new(),
            },
        };
        let plan = Plan {
            fmt: 1,
            version: "t".into(),
            tag: "1".into(),
            datasets: vec![
                geo("fine", 1, res, crs, "https://a.org/fine.tif"),
                geo("coarse", 2, 3.0 * PX * M_PER_DEG, 4326, "https://b.org/coarse.tif"),
            ],
        };
        let ts: Vec<(u32, u32)> = (0..2).flat_map(|j| (0..2).map(move |i| (t.1 * 2 + i, t.2 * 2 + j))).collect();
        let (lon, lat) = points(13, &ts);
        let want = crate::dem::generic::sample(&plan, &lon, &lat, &Opts { fetch: &f, store: None }, &|_| {}).unwrap();
        let reads: Reads = Default::default();
        let counted = Counted(f, reads.clone());
        let bf = BaseFetch::new(&counted);
        let o = Opts { fetch: &bf, store: None };
        let s = Sampler::new(&plan, &o).unwrap();
        let items = plan_items(&plan_files(&plan, tb, &bf).unwrap(), &[vec![t]]).unwrap();
        assert!(items.len() > 4, "{} blocks planned", items.len());
        let mut got = None;
        let mut during = Vec::new();
        read_ahead(&items, 1, true, |_| {
            let before = reads.lock().unwrap().len();
            got = Some(s.sample(&lon, &lat, &|_| {})?);
            during = reads.lock().unwrap()[before..].to_vec();
            Ok(())
        })
        .unwrap();
        let got = got.unwrap();
        let bits = |s: &crate::dem::generic::Sampled| s.elev.iter().map(|e| e.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&got), bits(&want));
        assert_eq!((&got.src, &got.blended), (&want.src, &want.blended));
        assert!(want.blended.iter().any(|b| *b) && want.src.contains(&2), "blends and the coarse file's heights");
        let big = |r: &&(u64, usize)| r.1 > crate::geotiff::HEADER_WINDOW;
        assert_eq!(during.iter().filter(big).count(), 0, "every block the sampling read was read ahead");
        let mut offs: Vec<u64> = reads.lock().unwrap().iter().filter(big).map(|r| r.0).collect();
        let n = offs.len();
        offs.sort();
        offs.dedup();
        assert_eq!(offs.len(), n, "each block read once");
        assert!(items.iter().all(|i| i.file.spans.lock().unwrap().is_empty()), "let go once worked");
    }

    /// The fixture's roads dataset's tiles, all 100 m (its z10 pyramid, read as if finer).
    struct Flat(Vec<u8>);
    impl crate::fetch::Fetch for Flat {
        fn open(&self, _: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
            Ok(None)
        }
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            Ok(url.starts_with("https://e.org/10/").then(|| self.0.clone()))
        }
    }

    /// A deep pack written here: its index and file.
    fn read_pack(p: &crate::layers::LocalPack) -> (store::pack::PackIndex, store::range::PlainFile) {
        let f = store::range::PlainFile::open(&p.local).unwrap();
        (store::pack::PackIndex::read_from(&f).unwrap(), f)
    }

    /// The fixture's elevation datasets, its world dataset taken as a pyramid to `zoom`.
    fn world_to(d: &std::path::Path, zoom: u8) -> Fine {
        let mut m = std::collections::BTreeMap::new();
        crate::inputs::elevation::accept_fixture(&store::nasfs::NasRoot::local_for_test(d), &mut m);
        let a = crate::inputs::open(&store::nasfs::NasRoot::local_for_test(d), &m, crate::inputs::elevation::UNIT)
            .unwrap()
            .unwrap();
        let mut fine = Fine::of(a, Vec::new()).unwrap();
        fine.sets = vec![FineSet {
            id: "world".into(),
            line: "world w".into(),
            cover: Cover::of(None),
            detail: Detail::Pyramid(zoom),
            cut: None,
        }];
        fine
    }

    #[test]
    fn a_z10_tile_is_made_from_its_base_tiles_and_their_ring() {
        let d = tempfile::tempdir().unwrap();
        let fine = world_to(d.path(), 13);
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        // Z10 tile 10/299/369 (in z8 tile 8/74/92, z6 tile 6/18/23): two of its base tiles (one at
        // its east edge), the ring tile past that edge (in z10 tile 10/300/369), one further, and a
        // z9; each z12 at 50 m.
        let t = (299u32, 369u32);
        let (edge, inside, ring, far) =
            ((12u8, 1199u32, 1477u32), (12, 1197, 1477), (12, 1200, 1477), (12, 1202, 1477));
        let want = base_want(t);
        assert_eq!(want.len(), 36, "its 16 z12 tiles and their ring");
        assert!([edge, inside, ring].iter().all(|b| want.contains(b)));
        assert!(!want.contains(&far) && !want.contains(&(9, 148, 184)));
        let tile = |m: f32| crate::terrain_codec::encode(&mut vec![m; 65536], 0.1).unwrap();
        let base: HashMap<(u8, u32, u32), Vec<u8>> = [edge, inside, ring].iter().map(|&b| (b, tile(50.0))).collect();
        let flat = Flat(roadcore::grid::encode_terrain_png(&vec![100.0; 65536], 256, 256).unwrap());
        let (rep, pack) = make(&out, t, &mut |_| Ok(base.clone()), None, &fine, &flat, None).unwrap();
        assert_eq!(
            (rep.base_tiles, rep.with_data, rep.tiles, rep.packs),
            (16, 2, 8, 1),
            "its 16 base tiles with data, the two the base has made; the ring's tiles not made"
        );
        let p = pack.unwrap();
        assert_eq!(p.logical, pack_logical(t.0, t.1));
        let (idx, f) = read_pack(&p);
        assert_eq!(idx.meta["encoding"], crate::terrain_codec::ENCODING, "the deep packs say what their tiles are");
        assert_eq!(idx.meta["scope"], "deep");
        assert!(idx.entries.iter().all(|e| under(e.zxy().0, e.zxy().1, e.zxy().2, PACK_Z, t)));
        let e = idx.find(13, edge.1 * 2 + 1, edge.2 * 2).unwrap();
        let v = roadcore::grid::decode_terrain(&idx.read_blob(&f, &e).unwrap()).unwrap();
        assert!(v.iter().all(|&h| (h - 100.0).abs() < 0.01), "the data's heights");
    }

    /// Raw tiles all at `m` metres, every one there.
    struct Level(f32);
    impl crate::terrain_pack::RawSource for Level {
        fn get(&self, _: u8, _: u32, _: u32) -> Result<(Option<Vec<u8>>, bool)> {
            Ok((Some(roadcore::grid::encode_terrain_png(&vec![self.0; 65536], 256, 256)?), false))
        }
        fn prefetch_counted(
            &self,
            _: u8,
            tiles: &[(u32, u32)],
            _: usize,
            done: &std::sync::atomic::AtomicU64,
        ) -> Result<usize> {
            done.fetch_add(tiles.len() as u64, Ordering::Relaxed);
            Ok(0)
        }
    }

    /// The world dataset's z10 tiles, each its own heights: a slope, raised by tile.
    struct Varied;
    impl crate::fetch::Fetch for Varied {
        fn open(&self, _: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
            Ok(None)
        }
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            let Some(r) = url.strip_prefix("https://e.org/10/") else { return Ok(None) };
            let v: Vec<u32> = r.split(['/', '.']).filter_map(|s| s.parse().ok()).collect();
            let k = (v[0] * 7 + v[1] * 3) % 11;
            let e: Vec<f32> =
                (0..65536).map(|p| 100.0 + k as f32 * 5.0 + (p % 256) as f32 * 0.3 + (p / 256) as f32 * 0.1).collect();
            Ok(Some(roadcore::grid::encode_terrain_png(&e, 256, 256)?))
        }
    }

    #[test]
    fn a_deep_target_stopped_and_resumed_makes_the_same_pack() {
        use crate::resume_log::STOP_AFTER;
        let d = tempfile::tempdir().unwrap();
        let cov = crate::coverage::Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:-75.5,44.6,3".into()],
            }],
            None,
            d.path(),
        )
        .unwrap();
        let src = crate::terrain_pack::Sources::default();
        let raw = Level(50.0);
        let bases = crate::terrain_pack::Bases::new(&raw, &src, &cov);
        let (f14, f13) = (world_to(&d.path().join("f14"), 14), world_to(&d.path().join("f13"), 13));
        let t = targets_in(&f14.sets, &cov, (18, 23))[0];
        // (Its deep pack's content name, the bytes' digest; None when stopped.)
        let run = |name: &str, fine: &Fine, stop: Option<usize>| -> Option<String> {
            let root = d.path().join(name);
            std::fs::create_dir_all(root.join("state/build")).unwrap();
            let scratch = d.path().join(format!("{name}-scratch"));
            let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
            STOP_AFTER.set(stop);
            let r = build_target_kept(&mut out, t, &bases, None, fine, &Varied, true);
            STOP_AFTER.set(None);
            let log = crate::resume_log::path(&scratch, (PACK_Z, t.0, t.1));
            if let Err(e) = r {
                assert!(e.to_string().contains("a test's interruption") && log.exists(), "{e:#}");
                return None;
            }
            assert!(r.unwrap().tiles > 0 && !log.exists(), "the log gone once put");
            Some(out.get(&pack_logical(t.0, t.1)).unwrap().to_string())
        };
        let fresh = run("fresh", &f14, None).unwrap();
        // Stopped after the first, a middle and the last but one of its 12 base tiles (made on
        // the pool's threads, so not always the same ones), then resumed: the same bytes.
        for stop in [1, 6, 11] {
            let name = format!("stop-{stop}");
            assert!(run(&name, &f14, Some(stop)).is_none(), "stopped after {stop}");
            assert_eq!(run(&name, &f14, None).as_ref(), Some(&fresh), "stopped after {stop}");
        }
        assert!(run("all", &f14, Some(13)).is_some(), "12 base tiles");
        assert!(run("twice", &f14, Some(1)).is_none() && run("twice", &f14, Some(4)).is_none());
        assert_eq!(run("twice", &f14, None).as_ref(), Some(&fresh), "stopped twice");
        // Another input (the data to z13, not z14) voids the log.
        let to13 = run("to13", &f13, None).unwrap();
        assert_ne!(to13, fresh);
        assert!(run("voided", &f14, Some(6)).is_none());
        assert_eq!(run("voided", &f13, None), Some(to13));
    }

    #[test]
    fn a_deep_target_makes_its_pack_from_its_own_base_and_drops_it_once_no_target() {
        let d = tempfile::tempdir().unwrap();
        let fine = world_to(d.path(), 14);
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        // Z6 tile 6/18/23 (~45°N), the coverage over a few of its base tiles.
        let cov = crate::coverage::Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:-75.5,44.6,3".into()],
            }],
            None,
            d.path(),
        )
        .unwrap();
        let t6 = (18u32, 23u32);
        let targets = targets_in(&fine.sets, &cov, t6);
        assert!(!targets.is_empty() && targets.iter().all(|&t| is_target(&fine.sets, &cov, t)));
        assert!(targets_in(&[], &cov, t6).is_empty(), "no fine dataset: no target");
        let t = targets[0];
        // (Its base: the raw tiles at 50 m, as the terrain-hi run makes them; the data at 100 m.)
        let src = crate::terrain_pack::Sources::default();
        let raw = Level(50.0);
        let bases = crate::terrain_pack::Bases::new(&raw, &src, &cov);
        let base = target_base(&bases, t, None).unwrap();
        assert!(!base.is_empty() && base.keys().all(|b| base_want(t).contains(b)));
        // (Near one base tile with data: it and the tiles around it alone.)
        let one = *base.keys().find(|k| under(k.0, k.1, k.2, PACK_Z, t)).unwrap();
        let near = target_base(&bases, t, Some(&[one])).unwrap();
        assert!(near.contains_key(&one) && near.len() <= 9 && near.keys().all(|&k| near_tile(k, one)));
        let flat = Flat(roadcore::grid::encode_terrain_png(&vec![100.0; 65536], 256, 256).unwrap());
        let rep = build_target(&mut out, t, &bases, None, &fine, &flat).unwrap();
        assert!(rep.tiles > 0 && rep.packs == 1 && rep.dropped == 0, "{rep:?}");
        let l = pack_logical(t.0, t.1);
        let (idx, f) = {
            let p = out.path(out.get(&l).unwrap());
            let f = p.open_plain().unwrap();
            (store::pack::PackIndex::read_from(&f).unwrap(), f)
        };
        let zs: std::collections::BTreeSet<u8> = idx.entries.iter().map(|e| e.zxy().0).collect();
        assert_eq!(zs, [13, 14].into(), "z13 on, to the data's zoom");
        let e = idx.entries.iter().find(|e| e.zxy().0 == 14).unwrap();
        let v = roadcore::grid::decode_terrain(&idx.read_blob(&f, e).unwrap()).unwrap();
        assert!(v.iter().all(|&h| (h - 100.0).abs() < 0.01), "the data's heights, not the base's");
        // (A retired pack there goes with its run.)
        out.manifest.insert(format!("{RETIRED}10-{}-{}", t.0, t.1), "x".into());
        build_target(&mut out, t, &bases, None, &fine, &flat).unwrap();
        assert_eq!(named_under(&out.manifest, PACK_Z, t.0, t.1), vec![l.clone()]);
        // No target any more: its pack goes.
        assert_eq!(drop_target(&mut out, t), 1);
        assert!(out.get(&l).is_none());
        // The z10 tiles past the base's zoom, and their native zoom.
        let deeper = deeper(&fine.sets, t6);
        assert_eq!(deeper.len(), 256, "the world dataset reaches past z12 everywhere");
        assert_eq!(native_zoom(&fine.sets, t), 14);
        assert_eq!(native_zoom(&[], t), 12);
        assert!(reaches(&fine.sets, 6, t6.0, t6.1) && !reaches(&[], 6, t6.0, t6.1));
        // Its memory: its base made as a z6 tile's run at most, or its deep tiles' (`peak_mb`, over
        // the blocks kept).
        assert!(peak_mb() > (KEPT_BYTES >> 20) as u64);
        assert_eq!(target_peak_mb(), peak_mb().max(crate::terrain_pack::piece_peak_mb()));
    }

    /// A deep target's key: the fine datasets meeting its z8 tile and itself, and which z10 tiles
    /// around it go past the base's zoom; not a dataset far from it.
    #[test]
    fn a_deep_targets_key_names_what_it_reads_of_the_fine_datasets() {
        // (Z10 tile 10/384/369: its west edge on 45°W, a cell's.)
        let t = (384u32, 369u32);
        let around: Vec<(i32, i32)> = (-46..=-44).flat_map(|x| (43..=45).map(move |y| (x, y))).collect();
        let lidar = |res: u64| set("lidar", Detail::Raster(res), &around);
        let k = target_key_lines(&[lidar(2000)], t);
        assert!(k.contains("10/384/369 lidar k zoom 16") && k.contains(&versions()), "{k}");
        assert!(k.ends_with("deeper ddddddddd"), "{k}");
        assert_eq!(k, target_key_lines(&[lidar(2000), set("far", Detail::Raster(2000), &[(100, -10)])], t));
        assert_ne!(k, target_key_lines(&[lidar(10300)], t), "its zoom there moved");
    }

    /// North of 67° the base ends at z11 and the deep levels begin at z12 (70°N, z6 tile 6/36/14):
    /// those of z12 go in the hi pack (`hi_levels`, the terrain-hi run's), z13 on in the deep pack
    /// (`make`, its terrain-deep target's), so the terrain's readers find each where they look
    /// (crate::terrain_pack::ManifestTiles: z9–12 in the hi packs), and the slope makes its z12 from
    /// them, its z13 in its deep pack.
    #[test]
    fn north_of_67_the_deep_z12_goes_in_the_hi_pack() {
        let d = tempfile::tempdir().unwrap();
        let fine = world_to(d.path(), 13);
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        let cov = crate::coverage::Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:25.0,70.0,3".into()],
            }],
            None,
            d.path(),
        )
        .unwrap();
        let (t6, b) = ((36u32, 14u32), (11u8, 1166u32, 458u32));
        assert!((crate::terrain_pack::tile_lat(11, b.2) - 70.0).abs() < 0.1);
        assert_eq!(crate::terrain_pack::max_zoom_at(crate::terrain_pack::tile_lat(11, b.2)), 11);
        let tile = |m: f32| crate::terrain_codec::encode(&mut vec![m; 65536], 0.1).unwrap();
        let mut hi: Vec<crate::terrain_pack::Made> = vec![(b.0, b.1, b.2, tile(50.0))];
        let flat = Flat(roadcore::grid::encode_terrain_png(&vec![100.0; 65536], 256, 256).unwrap());
        let deep = Deep { fine: Some(&fine), cov: &cov, fetch: &flat };
        let (rep, more) = hi_levels(&out, t6, &hi, None, &deep, &|_, _, _| {}).unwrap();
        let zs: Vec<(u8, u32, u32)> = more.iter().map(|m| (m.0, m.1, m.2)).collect();
        let z12: Vec<(u8, u32, u32)> = (0..4)
            .map(|k| (12, b.1 * 2 + (k & 1), b.2 * 2 + (k >> 1)))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!((zs, rep.tiles), (z12, 4), "its four z12 tiles for the hi pack, nothing past them");
        // Its z13 on: its z10 tile's deep target, from the same base.
        let t = (b.1 >> 1, b.2 >> 1);
        assert!(is_target(&fine.sets, &cov, t));
        let base: HashMap<(u8, u32, u32), Vec<u8>> = hi
            .iter()
            .filter(|m| base_want(t).contains(&(m.0, m.1, m.2)))
            .map(|m| ((m.0, m.1, m.2), m.3.clone()))
            .collect();
        let (rep, pack) = make(&out, t, &mut |_| Ok(base.clone()), None, &fine, &flat, None).unwrap();
        assert_eq!(rep.tiles, 16);
        let pack = pack.unwrap();
        let (idx, _) = read_pack(&pack);
        assert!(idx.entries.iter().all(|e| e.zxy().0 == 13) && idx.entries.len() == 16, "z13 alone in the deep pack");
        // Stored as the terrain runs store them: read where the readers look.
        hi.extend(more);
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        let mut it = hi.into_iter().map(|(z, x, y, b)| {
            let n = b.len() as u32;
            (z, x, y, b, n)
        });
        crate::layers::write_pack(
            &mut out,
            "terrain",
            crate::terrain_codec::ENCODING,
            false,
            "hi",
            (6, t6.0, t6.1),
            &mut it,
        )
        .unwrap();
        out.put_file(&pack.logical, "pack", &pack.local).unwrap();
        out.save().unwrap();
        let terr = crate::terrain_pack::ManifestTiles::new(&out, "terrain");
        let v = roadcore::grid::decode_terrain(&terr.get(12, b.1 * 2 + 1, b.2 * 2).unwrap().unwrap()).unwrap();
        assert!(v.iter().all(|&h| (h - 100.0).abs() < 0.01), "the deep z12, the data's heights");
        assert!(terr.get(13, b.1 * 4 + 3, b.2 * 4 + 2).unwrap().is_some());
        drop(terr);
        // The slope: its z10 tile's deep run (its deep pack, z13, and its deep mid), then the z6
        // tile's (its z12 from the deep mid).
        let r = crate::slope_pack::build_deep(&mut out, t, &|_, _, _| {}).unwrap();
        assert_eq!((r.deep_packs, r.deep_tiles), (1, 16), "{r:?}");
        out.save().unwrap();
        crate::slope_pack::build_piece(&mut out, t6, false, &|_, _, _| {}).unwrap();
        let slope = crate::terrain_pack::ManifestTiles::new(&out, "slope");
        assert!(
            slope.get(12, b.1 * 2, b.2 * 2 + 1).unwrap().is_some()
                && slope.get(13, b.1 * 4, b.2 * 4).unwrap().is_some()
        );
    }

    /// Where the base's finest zoom changes (79°: z10 row 130's middle is north of it, its child
    /// z11 row 261's south), a z10 base tile's deep levels leave out the finer base tile and what's
    /// under it, which that one's own make: no tile made twice.
    #[test]
    fn where_the_bases_zoom_changes_no_deep_tile_is_made_twice() {
        let d = tempfile::tempdir().unwrap();
        let fine = world_to(d.path(), 12);
        let lat = |z, y| crate::terrain_pack::max_zoom_at(crate::terrain_pack::tile_lat(z, y));
        assert_eq!((lat(10, 130), lat(11, 261)), (10, 11));
        let tile = |m: f32| crate::terrain_codec::encode(&mut vec![m; 65536], 0.1).unwrap();
        let base: HashMap<(u8, u32, u32), Vec<u8>> =
            [((10, 298, 130), tile(50.0)), ((11, 597, 261), tile(50.0))].into();
        let flat = Flat(roadcore::grid::encode_terrain_png(&vec![100.0; 65536], 256, 256).unwrap());
        let inside: Vec<(u8, u32, u32)> = base.keys().copied().collect();
        let mut zs: Vec<(u8, u32, u32)> = Vec::new();
        let rep = make_tiles(
            (74, 32),
            &inside,
            &mut |_| Ok(base.clone()),
            None,
            &fine,
            &flat,
            &store::nasfs::NasPath::local_for_test(d.path()),
            0..MAX_Z + 1,
            None,
            &mut |m| {
                zs.extend(m.into_iter().map(|t| (t.0, t.1, t.2)));
                Ok(())
            },
        )
        .unwrap();
        zs.sort();
        let n = zs.len();
        zs.dedup();
        assert_eq!(zs.len(), n, "none twice");
        assert_eq!((rep.tiles, zs.iter().filter(|t| t.0 == 11).count()), (3 + 16, 3), "{zs:?}");
        assert!(!zs.contains(&(11, 597, 261)));
    }
}
