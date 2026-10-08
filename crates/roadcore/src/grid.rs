//! Analysis rasters on the Web-Mercator z11 tile grid (256 × 256 cells per tile, ~54 m at
//! 45° N), stored only for tiles near roads. All layers share one tile list:
//!
//!   grid.idx          [u32; 2] tile x, y per slot
//!   grid.terrain.i16  elevation, metres            (terrain stage)
//!   grid.canopy.u8    canopy height, metres (p75)   (canopy stage)
//!   grid.class.u8     land cover class (`class`)    (landcover stage)
//!
//! Global cell coordinates are `tile * 256 + pixel` at zoom 11.

use det::Det;
use anyhow::{bail, Result};
use crate::Mmap;
use std::path::Path;

pub const Z: u8 = 11;
pub const TS: usize = 256;
pub const CELLS: usize = TS * TS;
/// Cells across the whole world at zoom 11.
pub const WORLD: f64 = (1u64 << Z) as f64 * TS as f64;

/// Land-cover classes (collapsed from ESA WorldCover).
pub mod class {
    pub const NONE: u8 = 0;
    pub const TREES: u8 = 1;
    pub const SHRUB: u8 = 2;
    pub const OPEN: u8 = 3; // grassland, cropland, bare, moss/lichen
    pub const BUILT: u8 = 4;
    pub const WATER: u8 = 5;
    pub const WETLAND: u8 = 6;
    pub const SNOW: u8 = 7;
}

pub struct GridIndex {
    pub tiles: Vec<[u32; 2]>,
    x0: i64,
    y0: i64,
    w: i64,
    h: i64,
    dense: Vec<i32>,
}

impl GridIndex {
    pub fn new(mut tiles: Vec<[u32; 2]>) -> Self {
        tiles.sort_unstable_by_key(|t| (t[1], t[0]));
        tiles.dedup();
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for t in &tiles {
            x0 = x0.min(t[0] as i64);
            y0 = y0.min(t[1] as i64);
            x1 = x1.max(t[0] as i64);
            y1 = y1.max(t[1] as i64);
        }
        if tiles.is_empty() {
            (x0, y0, x1, y1) = (0, 0, 0, 0);
        }
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut dense = vec![-1i32; (w * h) as usize];
        for (i, t) in tiles.iter().enumerate() {
            dense[((t[1] as i64 - y0) * w + (t[0] as i64 - x0)) as usize] = i as i32;
        }
        Self { tiles, x0, y0, w, h, dense }
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let b = std::fs::read(dir.join("grid.idx"))?;
        if b.len() % 8 != 0 {
            bail!("grid.idx: bad size");
        }
        let tiles: Vec<[u32; 2]> = bytemuck::pod_collect_to_vec(&b);
        Ok(Self::new(tiles))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, bytemuck::cast_slice(&self.tiles))?;
        Ok(())
    }

    #[inline]
    pub fn slot(&self, tx: i64, ty: i64) -> Option<usize> {
        let (dx, dy) = (tx - self.x0, ty - self.y0);
        if dx < 0 || dy < 0 || dx >= self.w || dy >= self.h {
            return None;
        }
        let s = self.dense[(dy * self.w + dx) as usize];
        (s >= 0).then_some(s as usize)
    }

    /// Flat index of global cell (gx, gy), if its tile is stored.
    #[inline]
    pub fn cell(&self, gx: i64, gy: i64) -> Option<usize> {
        let s = self.slot(gx >> 8, gy >> 8)?;
        Some(s * CELLS + ((gy & 255) as usize) * TS + (gx & 255) as usize)
    }
}

/// Read-only layer over a memory-mapped file of `CELLS` values per slot.
pub struct Layer<T: bytemuck::Pod> {
    map: Mmap,
    _t: std::marker::PhantomData<T>,
}

impl<T: bytemuck::Pod> Layer<T> {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self { map: crate::mmap(path)?, _t: std::marker::PhantomData })
    }
    #[inline]
    pub fn data(&self) -> &[T] {
        bytemuck::cast_slice(&self.map[..])
    }
}

/// Bilinear terrain (m) at fractional global cell coordinates (cell centres at +0.5).
pub fn terrain_bilinear(idx: &GridIndex, terrain: &[i16], gx: f64, gy: f64) -> Option<f32> {
    let (x, y) = (gx - 0.5, gy - 0.5);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let v = |cx: i64, cy: i64| idx.cell(cx, cy).map(|i| terrain[i] as f32);
    let v00 = v(x0, y0)?;
    let v01 = v(x0 + 1, y0).unwrap_or(v00);
    let v10 = v(x0, y0 + 1).unwrap_or(v00);
    let v11 = v(x0 + 1, y0 + 1).unwrap_or(v00);
    Some((v00 * (1.0 - fx) + v01 * fx) * (1.0 - fy) + (v10 * (1.0 - fx) + v11 * fx) * fy)
}

/// Global z11 cell coordinates (fractional) of a lon/lat.
#[inline]
pub fn cell_of(lon: f64, lat: f64) -> (f64, f64) {
    let (x, y) = crate::merc(lon, lat);
    (x * WORLD, y * WORLD)
}

/// Metres per z11 cell at a latitude.
#[inline]
pub fn cell_m(lat: f64) -> f64 {
    40_075_016.686 * lat.to_radians().dcos() / WORLD
}

/// Decode a Terrarium-encoded RGB(A) buffer into metres.
pub fn terrarium_decode(rgb: &[u8], channels: usize, out: &mut [f32]) {
    for (i, o) in out.iter_mut().enumerate() {
        let p = &rgb[i * channels..];
        *o = (p[0] as f32 * 256.0 + p[1] as f32 + p[2] as f32 / 256.0) - 32768.0;
    }
}

/// Decode a PNG terrain tile to metres (256 × 256).
pub fn decode_terrain_png(bytes: &[u8]) -> Result<Vec<f32>> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info()?;
    let mut buf = vec![0u8; r.output_buffer_size().unwrap_or(TS * TS * 4)];
    let info = r.next_frame(&mut buf)?;
    let ch = info.color_type.samples();
    let n = (info.width * info.height) as usize;
    if ch < 3 {
        bail!("unexpected PNG colour type {:?}", info.color_type);
    }
    let mut out = vec![0f32; n];
    terrarium_decode(&buf[..n * ch], ch, &mut out);
    Ok(out)
}

/// Above Everest: not an elevation. AWS Terrain Tiles fill some voids with 32767 m (a z9 pixel on
/// the Toyama shore, clusters of hundreds of z12 pixels along the US–Canada border).
pub const MAX_ELEV: f32 = 8900.0;
/// Below the deepest sea: not an elevation either (an empty pixel decodes to −32,768 m).
pub const MIN_ELEV: f32 = -11_500.0;
/// The least a blob must stand out of the ground it meets (m) to be repaired: smaller things are
/// whatever the source has (Tokyo's wharf cranes and towers, needles of 40–66 m, stay).
pub const BLOB_RISE: f64 = 100.0;
/// The steepest real terrain standing out on every side (a summit, a needle, a sea stack), metres up
/// per metre across, over `l` metres: 3 (72°) over 50 m or less, falling as the fourth root of the
/// width (2, 63°, at 250 m; 1.4, 54°, at 1 km), never below 1 (45°). Pixels are means: the wider
/// they are, the less steep the steepest ground they show.
pub fn steepest(l: f64) -> f64 {
    (3.0 * (50.0 / l).sqrt().sqrt()).clamp(1.0, 3.0)
}
/// The most pixels a blob may have to be repaired: larger ones (islands, sea stacks, mesas, a
/// mountain's top) stay whatever their shape.
pub const BLOB_MAX: u32 = 4096;
/// The most stages the repair takes (repair_terrain_blobs: each judges the tile with what was found
/// before filled in). The coverage's tiles take 15 at most (a cluster of spikes on an artifact,
/// one lobe a stage); almost all one or three.
const BLOB_STAGES: usize = 32;
/// The most pixels a spike may have (BlobKind::Spike): an islet larger than that stays.
pub const SPIKE_MAX: u32 = 16;
/// How many times the roughness of the ground around it (the middle half's spread, from two pixels
/// out to twice its radius) a blob must stand out of that ground's median to be on flat ground: an
/// artifact towers over water or lowland (26 to 1,200 times in AWS's tiles), while a summit AWS
/// drew too sharp stands among rough ground (2 to 12 times: a 3,534 m peak in the St. Elias as an
/// 81° cone, 1,204 m tall), and stays. A pit down to sea level in raised ground needs no such
/// margin: it's AWS's filler where its source had none.
pub const ROUGH: f64 = 20.0;
/// The same for a spike (BlobKind::Spike), small: 10.
pub const ROUGH_SPIKE: f64 = 10.0;
/// How steep a spike must be over its width (its height as the map shows it over its inradius)
/// when it stands alone, walled on flat ground but beside nothing taken: 2 (63°). AWS draws
/// buttes, plugs and islets less steep at the scale of its pixels (in the coverage: Monument
/// Valley's and Lake Powell's buttes at z11, 1.6 to 2; Beacon Rock, 2.2 its upper part, as a
/// spike only on the plane through the Columbia's gorge; islets off Japan and Hong Kong, 1.1 to 1.4).
pub const ALONE: f64 = 2.0;
/// How steep a spike must be over its width to stand on flat ground on a smooth slope (out of the
/// plane through the ground around it, ROUGH_SPIKE times that ground's spread about the plane): 4
/// (76°), a needle (Ogasawara's of 290 m on a slope at z12: 5.6).
pub const NEEDLE: f64 = 4.0;
/// How many times steeper than terrain can be (`steepest`) a blob that stands more than BLOB_RISE
/// out is broken whatever the ground around it: no summit AWS draws comes near (the sharpest in
/// the coverage, a 3,534 m peak of the St. Elias, is 3.1 times; of OSM's summits with a height
/// where AWS has them within 150 m, 1.3 at most), while the blocks where AWS's sources meet
/// among glaciers are 6 to 20 (one of 4,900 m on the St. Elias's ice at 2,300 m: 9).
pub const STEEP_ANYWAY: f64 = 6.0;

/// (Tests: a pixel whose blobs' weighing is printed.)
#[cfg(test)]
static TRACE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

/// What `repair_terrain` changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Repair {
    /// Undefined pixels filled (above MAX_ELEV, below MIN_ELEV, NaN).
    pub voids: usize,
    /// Towers and pits flattened, and their pixels.
    pub blobs: usize,
    pub blob_pixels: usize,
    /// Broken blobs under the sea, filled though the map shows sea level there either way.
    pub unseen: usize,
    /// The stages it took (repair_terrain_blobs): one when it found nothing, or nothing more.
    pub stages: usize,
}

impl Repair {
    pub fn changed(&self) -> bool {
        self.voids + self.blob_pixels + self.unseen > 0
    }
}

/// Why a blob was taken (`Blob`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlobKind {
    /// Steeper over its footprint than terrain can be, and towering over the ground around it.
    Broken,
    /// A small blob, not cut by the tile's edge, steeper than 45° over its width (its inradius)
    /// and two of: walled so on a quarter of its edge at least, on flat ground, beside a blob
    /// taken, a void or a pit of a ringing under the sea; beside nothing, steeper than 63° (ALONE),
    /// as buttes, plugs and islets aren't. A spike over water or lowland, a needle (NEEDLE) on a
    /// smooth slope, or a lobe of an artifact's ringing (a resampling's overshoot beside an edge
    /// of AWS's source, a tower beside a pit); or a pit down to sea level in raised ground.
    Spike,
    /// Broken, but under the sea: the map shows sea level there either way.
    Unseen,
}

/// A blob `repair_terrain` flattened, as it weighed it (`repair_terrain_blobs`: the scan's report).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blob {
    pub kind: BlobKind,
    /// Its top: the highest pixel of a tower, the lowest of a pit.
    pub top: u32,
    pub pit: bool,
    /// Its pixels (those it took that no other blob or void had).
    pub pixels: u32,
    /// How far it stood out (m) of the level where it met the ground, and that level.
    pub rise: f32,
    pub level: f32,
    /// Its radius as weighed (m), and whether it met the tile's edge.
    pub reach: f32,
    pub edge: bool,
    /// The ground around it as the map shows it, where it was weighed: its median and its
    /// roughness (m: the middle half's spread).
    pub ground: f32,
    pub rough: f32,
    /// The stage that found it (repair_terrain_blobs), from 1.
    pub stage: u8,
}

/// Repairs a 256 × 256 terrain tile in place, in one pass: what is broken or undefined is filled
/// from the clean ground around it, and nothing else changes.
/// - Voids: values above MAX_ELEV or below MIN_ELEV, or NaN.
/// - Towers and pits: blobs, each a component of the tile's level sets (the pixels above a level,
///   connected, around a peak; below it, around a pit), so a cluster is judged whole, against the
///   level at which it meets the rest, whatever its size, and can't hide behind its own pixels.
///   One that stands out more than BLOB_RISE as the map shows it is broken (BlobKind) when it's
///   steeper over its footprint, as the map shows it, than terrain can be (`steepest`, taken where
///   it does most, the flanks of a smooth bump with it) and towers over the ground around it
///   (ROUGH: a summit AWS drew too sharp, among rough ground, stays) or is far steeper than any
///   summit AWS draws (STEEP_ANYWAY), or when it's a pit down to sea level in raised ground (AWS's
///   filler); a small one steeper than 45° over its width is a spike when two of three hold:
///   walled, on flat ground, beside a blob taken, and when it's beside nothing it must be steeper
///   than 63° (buttes, plugs and islets aren't: Monument Valley's, Beacon Rock, islets off Japan);
///   one under the sea is filled there. A summit or a ridge widens as it goes down, a cliff is the
///   edge of something larger, and an island, a sea stack or a mesa larger than BLOB_MAX pixels
///   stays whatever it is.
/// The tile is taken as AWS has it, before bathymetry goes to sea level: a pit in a lake reads as
/// deep as AWS made it (−655 m in Shumarinai's, 274 m up, where a tower of 2,740 m rings).
/// It's judged in stages, each on AWS's tile with what was found before filled in (the smoothest
/// surface through the ground around it: `fill_from_around`), until one finds nothing, and then
/// once more as it will be stored (at sea level and over: a pit on land whose deepest part is below
/// zero shows as a hole to sea level): so a lesser tower that stood on a greater one's flank, or a
/// lobe of its ringing, is judged on the ground beneath it, and the tile returned is one it finds
/// nothing in as stored. Deterministic.
pub fn repair_terrain(t: &mut [f32], z: u8, lat: f64) -> Repair {
    repair_terrain_blobs(t, z, lat).0
}

/// `repair_terrain`, and the blobs it flattened.
pub fn repair_terrain_blobs(t: &mut [f32], z: u8, lat: f64) -> (Repair, Vec<Blob>) {
    debug_assert_eq!(t.len(), CELLS);
    let px = 40_075_016.7 * lat.to_radians().dcos() / ((1u64 << z) as f64 * TS as f64);
    let mut rep = Repair::default();
    let mut hole: Vec<bool> = t.iter().map(|&v| !(v <= MAX_ELEV && v >= MIN_ELEV)).collect();
    rep.voids = hole.iter().filter(|&&h| h).count();
    let mut blobs = Vec::new();
    if rep.voids == t.len() {
        t.fill(0.0);
        return (rep, blobs);
    }
    let aws = t.to_vec();
    if rep.voids > 0 {
        fill_from_around(t, &hole);
    }
    let (lo, hi) = t.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    if (hi.max(0.0) as f64 - lo.max(0.0) as f64) > BLOB_RISE {
        // Each stage judges the tile as it will be: AWS's, with what was found so far filled in from
        // the ground around it. One that stood on a blob found, or beside it (a lesser tower joined
        // to a greater by its flank, a lobe of its ringing), is judged on the ground beneath. It
        // ends at a stage that finds nothing, so the tile returned is one it found nothing in.
        let mut under = vec![false; t.len()];
        let mut ringing = vec![false; t.len()];
        // (Whether the stage judges the tile as it's stored, at sea level and over: once a stage
        // finds nothing in AWS's values, one does, and the repair ends when that one finds nothing
        // too, so the tile returned is one nothing is found in as stored.)
        let mut stored = false;
        for stage in 1..=BLOB_STAGES {
            rep.stages = stage;
            let n0 = blobs.len();
            let before = hole.clone();
            let base: Vec<f32> = if stored { t.iter().map(|&v| v.max(0.0)).collect() } else { t.to_vec() };
            let neg: Vec<f32> = base.iter().map(|&v| -v).collect();
            // (Towers are judged knowing the pits of a resampling's ringing that the stage before
            // found, and this stage's pits are found for the next; as stored, there are none.)
            let mut rung = vec![false; t.len()];
            let mut known = if stored { vec![false; t.len()] } else { ringing.clone() };
            for (pit, v) in [(false, &base[..]), (true, &neg[..])] {
                let n = blobs.len();
                broken_blobs(v, if pit { -1.0 } else { 1.0 }, &before, px, &mut hole, &mut under, if pit { &mut rung } else { &mut known }, &mut blobs);
                for b in &mut blobs[n..] {
                    b.pit = pit;
                    b.stage = stage as u8;
                    if pit {
                        b.level = -b.level;
                    }
                }
            }
            // (It ends at a stage as stored that found nothing, after one in AWS's values that found
            // nothing, its towers judged knowing the ringing's pits.)
            let found = blobs.len() > n0;
            if stored {
                if !found {
                    break;
                }
                stored = false;
            } else {
                let same = rung == ringing;
                ringing = rung;
                if !found && same {
                    stored = true;
                    continue;
                }
            }
            t.copy_from_slice(&aws);
            fill_from_around(t, &hole);
            // (What was under the sea stays there: the map shows it at sea level either way.)
            for (v, &u) in t.iter_mut().zip(&under) {
                if u {
                    *v = v.min(0.0);
                }
            }
        }
        let seen = |b: &&Blob| b.kind != BlobKind::Unseen;
        rep.blobs = blobs.iter().filter(seen).count();
        rep.blob_pixels = blobs.iter().filter(seen).map(|b| b.pixels as usize).sum();
        rep.unseen = blobs.iter().filter(|b| b.kind == BlobKind::Unseen).map(|b| b.pixels as usize).sum();
    }
    (rep, blobs)
}

/// Marks in `out` the broken blobs of `v`'s upper level sets, and adds them to `blobs` (`held`: the
/// pixels filled in at the stage's start, voids and blobs found before). The component tree is made
/// by union–find over the pixels from the highest down (8-connected; ties by index): when a pixel
/// joins a component, that component stood above the pixel's level until then, and is weighed
/// (repair_terrain). Where two meet, the one with the higher top goes on and the other's chain
/// ends: each component is weighed once, for its top.
fn broken_blobs(v: &[f32], sign: f32, held: &[bool], px: f64, out: &mut [bool], under: &mut [bool], ringing: &mut [bool], blobs: &mut Vec<Blob>) {
    const NONE: u32 = u32::MAX;
    let w = TS as i32;
    let mut order: Vec<u32> = (0..v.len() as u32).collect();
    order.sort_unstable_by(|&a, &b| v[b as usize].total_cmp(&v[a as usize]).then(a.cmp(&b)));
    let mut rank = vec![NONE; v.len()];
    for (k, &p) in order.iter().enumerate() {
        rank[p as usize] = k as u32;
    }
    let mut parent: Vec<u32> = (0..v.len() as u32).collect();
    let mut area = vec![0u32; v.len()];
    let mut top = vec![0u32; v.len()];
    let mut sides = vec![0u8; v.len()];
    // (Pixels beside a void or a blob already found, and the components that have one.)
    let near_px: Vec<bool> = (0..v.len())
        .map(|i| {
            let (x, y) = ((i % TS) as i32, (i / TS) as i32);
            !held[i] && [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].iter().any(|&(dx, dy)| {
                let (xx, yy) = (x + dx, y + dy);
                xx >= 0 && yy >= 0 && xx < w && yy < w && held[(yy * w + xx) as usize]
            })
        })
        .collect();
    let mut near = vec![false; v.len()];
    // Per component (at its root): its best so far (the kind first, then the excess), the rank at
    // which it stood so, and how it was weighed then (rise, level, reach, edge).
    // Per component (at its root), a candidate of each kind: a broken blob and an unseen one where
    // it stood out most, a spike where it first did (lower, its width grows little as flat ground
    // joins it: what's left of it is weighed in the next stage); each the rank at which it stood so
    // and how it was weighed then.
    type Best = (BlobKind, f64, u32, f32, f32, f32, bool);
    let mut best: Vec<[Option<Best>; 3]> = vec![[None; 3]; v.len()];
    let mut comp: Vec<u32> = Vec::with_capacity(SPIKE_MAX as usize + 1);
    // (Pits that are a ringing's other half, small and steep and more than BLOB_RISE deep below
    // the ground they meet, as AWS has them: their tops and ranks, and whether a chain has one.)
    let mut rings: Vec<(u32, u32)> = Vec::new();
    let mut ringed = vec![false; v.len()];
    let mut found: Vec<(u32, Best)> = Vec::new();
    fn find(parent: &mut [u32], mut a: u32) -> u32 {
        let mut r = a;
        while parent[r as usize] != r {
            r = parent[r as usize];
        }
        while parent[a as usize] != r {
            let next = parent[a as usize];
            parent[a as usize] = r;
            a = next;
        }
        r
    }
    let side = |p: u32| -> u8 {
        let (x, y) = (p % TS as u32, p / TS as u32);
        (x == 0) as u8 | ((x == TS as u32 - 1) as u8) << 1 | ((y == 0) as u8) << 2 | ((y == TS as u32 - 1) as u8) << 3
    };
    for (k, &p) in order.iter().enumerate() {
        let k = k as u32;
        let level = v[p as usize] as f64;
        let (x, y) = ((p % TS as u32) as i32, (p / TS as u32) as i32);
        let mut roots = [NONE; 8];
        let mut nr = 0;
        for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (xx, yy) = (x + dx, y + dy);
            if xx < 0 || yy < 0 || xx >= w || yy >= w {
                continue;
            }
            let q = (yy * w + xx) as u32;
            if rank[q as usize] < k {
                let r = find(&mut parent, q);
                if !roots[..nr].contains(&r) {
                    roots[nr] = r;
                    nr += 1;
                }
            }
        }
        // Each component the pixel meets stood above its level until now.
        for &r in &roots[..nr] {
            let a = area[r as usize];
            if a > BLOB_MAX {
                continue;
            }
            // (Weighed on AWS's values, but only what the map shows counts towards BLOB_RISE:
            // bathymetry goes to sea level, so what stands out of the sea floor alone, or a pit in
            // it, isn't seen; it's taken too when it's broken, which changes nothing on the map.)
            let peak = v[top[r as usize] as usize] as f64;
            let rise = peak - level;
            if rise <= BLOB_RISE {
                continue;
            }
            let (s, sl) = (sign as f64 * peak, sign as f64 * level);
            let seen = (s.max(0.0) - sl.max(0.0)).abs();
            let a_eff = (a as f64) * (1u32 << sides[r as usize].count_ones()) as f64;
            let l = ((a_eff / std::f64::consts::PI).sqrt() + 0.5) * px;
            let allowed = steepest(l) * l;
            let edge = sides[r as usize] != 0;
            let slot = &mut best[r as usize];
            // (Steepness as the map shows it: what's below sea level, the sea floor or a pit's
            // depth below zero, makes nothing steeper on the map.)
            if seen > BLOB_RISE && seen > allowed && slot[0].is_none_or(|b| seen - allowed > b.1) {
                slot[0] = Some((BlobKind::Broken, seen - allowed, k, rise as f32, level as f32, l as f32, edge));
            }
            // (A ringing's pit is under the sea, where nothing else makes one so steep and deep.)
            let ring = sign < 0.0 && s.max(sl) <= 0.0 && !ringed[r as usize];
            if (seen > BLOB_RISE || ring) && a <= SPIKE_MAX && !edge && (slot[1].is_none() || ring) {
                // (A spike's width: its inradius, the steps from its innermost pixel to the ground.)
                comp.clear();
                comp.push(top[r as usize]);
                let mut i = 0;
                while i < comp.len() {
                    let (cx, cy) = ((comp[i] % TS as u32) as i32, (comp[i] / TS as u32) as i32);
                    for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                        let (xx, yy) = (cx + dx, cy + dy);
                        if xx >= 0 && yy >= 0 && xx < w && yy < w {
                            let q = (yy * w + xx) as u32;
                            if rank[q as usize] < k && !comp.contains(&q) {
                                comp.push(q);
                            }
                        }
                    }
                    i += 1;
                }
                let width = inradius(&comp) as f64 * px;
                if seen > BLOB_RISE && seen > width && slot[1].is_none() {
                    slot[1] = Some((BlobKind::Spike, seen - width, k, rise as f32, level as f32, width as f32, edge));
                }
                if ring && rise > width {
                    rings.push((top[r as usize], k));
                    ringed[r as usize] = true;
                }
            }
            let excess = rise - allowed;
            if s.max(sl) <= 0.0 && excess > 0.0 && slot[2].is_none_or(|b| excess > b.1) {
                slot[2] = Some((BlobKind::Unseen, excess, k, rise as f32, level as f32, l as f32, edge));
            }
        }
        if nr == 0 {
            area[p as usize] = 1;
            top[p as usize] = p;
            sides[p as usize] = side(p);
            near[p as usize] = near_px[p as usize];
            continue;
        }
        let roots = &roots[..nr];
        let dom = *roots.iter().min_by_key(|&&r| rank[top[r as usize] as usize]).unwrap();
        for &r in roots {
            if r == dom {
                continue;
            }
            for b in best[r as usize].into_iter().flatten() {
                found.push((top[r as usize], b));
            }
            parent[r as usize] = dom;
            area[dom as usize] += area[r as usize];
            sides[dom as usize] |= sides[r as usize];
            near[dom as usize] |= near[r as usize];
        }
        parent[p as usize] = dom;
        area[dom as usize] += 1;
        sides[dom as usize] |= side(p);
        near[dom as usize] |= near_px[p as usize];
    }
    for &p in &order {
        if parent[p as usize] == p {
            for b in best[p as usize].into_iter().flatten() {
                found.push((top[p as usize], b));
            }
        }
    }
    // The ringing's pits: their pixels as they stood (towers beside them are judged so).
    for &(t0, at) in &rings {
        let mut st = vec![t0];
        ringing[t0 as usize] = true;
        while let Some(p) = st.pop() {
            let (x, y) = ((p % TS as u32) as i32, (p / TS as u32) as i32);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (xx, yy) = (x + dx, y + dy);
                if xx >= 0 && yy >= 0 && xx < w && yy < w {
                    let q = (yy * w + xx) as usize;
                    if !ringing[q] && rank[q] < at {
                        ringing[q] = true;
                        st.push(q as u32);
                    }
                }
            }
        }
    }
    // Each blob: its top's component as it stood at its best (the pixels ranked before then).
    let mut stamp = vec![NONE; v.len()];
    let mut stack: Vec<u32> = Vec::new();
    let mut pixels: Vec<u32> = Vec::new();
    // (A chain whose broken blob was taken is done with: its other candidates aren't weighed.)
    let mut taken_top = NONE;
    for (b, &(t0, (kind, excess, at, rise, level, reach, edge))) in found.iter().enumerate() {
        if t0 == taken_top {
            continue;
        }
        let b = b as u32;
        pixels.clear();
        stamp[t0 as usize] = b;
        stack.push(t0);
        while let Some(p) = stack.pop() {
            pixels.push(p);
            let (x, y) = ((p % TS as u32) as i32, (p / TS as u32) as i32);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (xx, yy) = (x + dx, y + dy);
                if xx < 0 || yy < 0 || xx >= w || yy >= w {
                    continue;
                }
                let q = (yy * w + xx) as usize;
                if stamp[q] != b && rank[q] < at {
                    stamp[q] = b;
                    stack.push(q as u32);
                }
            }
        }
        // As the map shows it (bathymetry at sea level): how far it stands out of the median of the
        // ground around it, against that ground's roughness (on flat ground when ROUGH times).
        let shown = |i: usize| (sign * v[i]).max(0.0) as f64;
        let g = ground(v, sign, &pixels, out, held, &mut stamp, b, t0);
        let (median, rough) = (g.median, g.rough);
        let out_of = sign as f64 * (shown(t0 as usize) - median as f64);
        // (How far a spike stands out of the level it meets, as the map shows it, over its width:
        // a needle, steeper than 76°, stands on flat ground on a smooth slope too, out of the plane
        // through it.)
        let steep = (shown(t0 as usize) - (sign * level).max(0.0) as f64).abs() / (reach as f64).max(1.0);
        let flat = if kind == BlobKind::Spike {
            out_of > ROUGH_SPIKE * rough as f64 || (steep > NEEDLE && sign as f64 * (shown(t0 as usize) - g.plane as f64) > ROUGH_SPIKE * g.plane_rough as f64)
        } else {
            out_of > ROUGH * rough as f64
        };
        let at_sea = sign < 0.0 && -v[t0 as usize] <= 1.0;
        #[cfg(test)]
        let traced = pixels.contains(&TRACE.load(std::sync::atomic::Ordering::Relaxed));
        // (Only a blob that changes something is weighed: one with a pixel not taken yet, or, one
        // the map shows, a pixel taken as under the sea, the part below zero of a pit in raised
        // ground found first: what the map shows taken whole is filled from the ground around it.)
        if !pixels.iter().any(|&p| !out[p as usize] || (kind != BlobKind::Unseen && under[p as usize])) {
            continue;
        }
        #[cfg(test)]
        if traced {
            eprintln!("  {kind:?} top {},{} px {} rise {rise:.0} level {level:.0} reach {reach:.0} | out_of {out_of:.0} median {median:.0} rough {rough:.1} flat {flat} at_sea {at_sea}", t0 % 256, t0 / 256, pixels.len());
        }
        // (Far steeper than any summit AWS draws, it's broken whatever its ground.)
        let wild = kind == BlobKind::Broken && excess > (STEEP_ANYWAY - 1.0) * steepest(reach as f64) * reach as f64;
        if kind == BlobKind::Broken && !(flat || at_sea || wild) {
            continue;
        }
        if kind == BlobKind::Spike {
            // Two of: walled steeper than 45° (a quarter of the steps down its edge, at least: an
            // islet comes out of the sea more gently all round), on flat ground, beside a blob
            // taken, a void, or a pit of a ringing under the sea (small, steeper than 45° over its
            // width and more than BLOB_RISE below the sea floor it meets: nothing else makes one).
            let (mut steps, mut beside) = (Vec::new(), false);
            for &p in &pixels {
                let (x, y) = ((p % TS as u32) as i32, (p / TS as u32) as i32);
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx < 0 || yy < 0 || xx >= w || yy >= w {
                        continue;
                    }
                    let q = (yy * w + xx) as usize;
                    if stamp[q] == b {
                        continue;
                    }
                    beside |= out[q] || ringing[q];
                    let d = if dx != 0 && dy != 0 { std::f64::consts::SQRT_2 } else { 1.0 };
                    steps.push(sign as f64 * (shown(p as usize) - shown(q)) / (d * px));
                }
            }
            steps.sort_by(|a, b| a.total_cmp(b));
            let wall = steps.get(steps.len() * 3 / 4).copied().unwrap_or(0.0);
            #[cfg(test)]
            if traced {
                eprintln!("    wall {wall:.2} beside {beside}");
            }
            // (Two of three: steep over its width alone, a spike may be an island's top,
            // Minami-Iwo-jima's at z8, or a plug, Shiprock's at z10, both on flat ground and walled
            // less, 0.64 and 0.88; a lobe of an artifact is beside a blob taken.)
            // (Beside nothing, walled on flat ground, it must be steeper than 63° over its width:
            // buttes, plugs and islets, as AWS draws them, aren't. A pit down to sea level in
            // raised ground is AWS's filler, steep enough as it is.)
            let two = [wall >= 1.0, flat, beside].iter().filter(|&&c| c).count() >= 2;
            if !(at_sea || (two && (beside || steep > ALONE))) {
                continue;
            }
        }
        if kind == BlobKind::Broken {
            taken_top = t0;
        }
        let mut marked = 0;
        for &p in &pixels {
            let p = p as usize;
            if kind == BlobKind::Unseen {
                under[p] |= !out[p];
            } else {
                under[p] = false;
            }
            if !out[p] {
                out[p] = true;
                marked += 1;
            }
        }
        blobs.push(Blob { kind, top: t0, pit: false, pixels: marked, rise, level, reach, edge, ground: median, rough, stage: 0 });
    }
}

/// How the repair would weigh the top at `p` of tile `t` (towers only), for checking it against
/// summits known elsewhere (OSM's, in `terrain --scan`): its pixels joined highest first, as the
/// repair's tree does, until one higher than it (there its chain ends) or BLOB_MAX of them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weighing {
    /// Its most, over the levels below it, of its rise over what the repair allows (1: broken by
    /// steepness), and there how it stood out of the ground around it against that ground's
    /// roughness (ROUGH: on flat ground).
    pub steep: f32,
    pub steep_flat: f32,
    /// Where it first qualified as a spike, if it did: its wall (the upper quartile of the steps
    /// down its edge, m per m: 1 is 45°) and how it stood out of the plane through the ground
    /// around it against that ground's spread about the plane (ROUGH_SPIKE: flat).
    pub spike: bool,
    pub spike_wall: f32,
    pub spike_flat: f32,
}

pub fn weigh_top(t: &[f32], p: u32, z: u8, lat: f64) -> Weighing {
    #[derive(PartialEq)]
    struct H(f32, u32);
    impl Eq for H {}
    impl PartialOrd for H {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for H {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&o.0).then(o.1.cmp(&self.1))
        }
    }
    let w = TS as i32;
    let px = 40_075_016.7 * lat.to_radians().dcos() / ((1u64 << z) as f64 * TS as f64);
    let top = t[p as usize];
    let mut inside = vec![false; t.len()];
    let mut queued = vec![false; t.len()];
    let mut heap = std::collections::BinaryHeap::new();
    heap.push(H(top, p));
    queued[p as usize] = true;
    let (mut members, mut sides) = (Vec::<u32>::new(), 0u8);
    let (mut best_steep, mut steep_at) = (0f64, 0usize);
    let mut spike_at = None;
    let mut out = Weighing::default();
    while let Some(H(v, q)) = heap.pop() {
        if v > top || !v.is_finite() || members.len() as u32 > BLOB_MAX {
            break;
        }
        if !members.is_empty() {
            let a = members.len() as f64;
            let seen = (top.max(0.0) - v.max(0.0)) as f64;
            let a_eff = a * (1u32 << sides.count_ones()) as f64;
            let l = ((a_eff / std::f64::consts::PI).sqrt() + 0.5) * px;
            if seen > BLOB_RISE {
                let r = seen / (steepest(l) * l);
                if r > best_steep {
                    (best_steep, steep_at) = (r, members.len());
                }
                if spike_at.is_none() && members.len() as u32 <= SPIKE_MAX && seen > inradius(&members) as f64 * px {
                    spike_at = Some(members.len());
                }
            }
        }
        let (x, y) = ((q % TS as u32) as i32, (q / TS as u32) as i32);
        sides |= (x == 0) as u8 | ((x == w - 1) as u8) << 1 | ((y == 0) as u8) << 2 | ((y == w - 1) as u8) << 3;
        inside[q as usize] = true;
        members.push(q);
        for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (xx, yy) = (x + dx, y + dy);
            if xx >= 0 && yy >= 0 && xx < w && yy < w && !queued[(yy * w + xx) as usize] {
                queued[(yy * w + xx) as usize] = true;
                heap.push(H(t[(yy * w + xx) as usize], (yy * w + xx) as u32));
            }
        }
    }
    let none = vec![false; t.len()];
    let mut stamp = vec![u32::MAX; t.len()];
    let shown = |i: usize| t[i].max(0.0) as f64;
    let mut flat_of = |pix: &[u32], b: u32, plane: bool| {
        for &q in pix {
            stamp[q as usize] = b;
        }
        let g = ground(t, 1.0, pix, &none, &none, &mut stamp, b, p);
        if plane {
            ((shown(p as usize) - g.plane as f64) / (g.plane_rough as f64).max(0.01)) as f32
        } else {
            ((shown(p as usize) - g.median as f64) / (g.rough as f64).max(0.01)) as f32
        }
    };
    out.steep = best_steep as f32;
    if steep_at > 0 {
        out.steep_flat = flat_of(&members[..steep_at], 0, false);
    }
    let wall_of = |pix: &[u32]| {
        let mut steps = Vec::new();
        for &q in pix {
            let (x, y) = ((q % TS as u32) as i32, (q / TS as u32) as i32);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (xx, yy) = (x + dx, y + dy);
                if xx < 0 || yy < 0 || xx >= w || yy >= w || pix.contains(&((yy * w + xx) as u32)) {
                    continue;
                }
                let d = if dx != 0 && dy != 0 { std::f64::consts::SQRT_2 } else { 1.0 };
                steps.push((shown(q as usize) - shown((yy * w + xx) as usize)) / (d * px));
            }
        }
        steps.sort_by(|a, b| a.total_cmp(b));
        steps.get(steps.len() * 3 / 4).copied().unwrap_or(0.0) as f32
    };
    if let Some(n) = spike_at {
        out.spike = true;
        out.spike_flat = flat_of(&members[..n], 1, true);
        out.spike_wall = wall_of(&members[..n]);
    }
    out
}

/// A small blob's inradius: the most steps (8-connected) from one of its pixels to one outside it.
fn inradius(pix: &[u32]) -> u32 {
    let inside = |q: u32| pix.contains(&q);
    let mut dist: Vec<u32> = vec![u32::MAX; pix.len()];
    // (Pixels next to the outside are one step from it; the rest one more than their nearest.)
    let mut changed = true;
    for (i, &q) in pix.iter().enumerate() {
        let (x, y) = ((q % TS as u32) as i32, (q / TS as u32) as i32);
        let edge = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].iter().any(|&(dx, dy)| {
            let (xx, yy) = (x + dx, y + dy);
            xx < 0 || yy < 0 || xx >= TS as i32 || yy >= TS as i32 || !inside((yy * TS as i32 + xx) as u32)
        });
        if edge {
            dist[i] = 1;
        }
    }
    while changed {
        changed = false;
        for i in 0..pix.len() {
            let (x, y) = ((pix[i] % TS as u32) as i32, (pix[i] / TS as u32) as i32);
            for (j, &q) in pix.iter().enumerate() {
                let (qx, qy) = ((q % TS as u32) as i32, (q / TS as u32) as i32);
                if (qx - x).abs() <= 1 && (qy - y).abs() <= 1 && dist[j] != u32::MAX && dist[j] + 1 < dist[i] {
                    dist[i] = dist[j] + 1;
                    changed = true;
                }
            }
        }
    }
    dist.into_iter().filter(|&d| d != u32::MAX).max().unwrap_or(1)
}

/// The ground around a blob (`pixels`, stamped `b` in `stamp`) as the map shows it (`sign` × `v`,
/// bathymetry at sea level), from two pixels out to twice its radius (at least four), but what was
/// taken in this stage (`out` and not `held`): its median, and its roughness, the spread of its
/// middle half. (0, 0) when there's none.
/// The ground around a blob as the map shows it (`ground`): its median and roughness (the middle
/// half's spread), and the plane through it (least squares) at the blob's top, with the spread of
/// the middle half of its pixels about that plane (a smooth slope strays little from it).
#[derive(Clone, Copy, Debug, Default)]
struct Ground {
    median: f32,
    rough: f32,
    plane: f32,
    plane_rough: f32,
}

fn ground(v: &[f32], sign: f32, pixels: &[u32], out: &[bool], held: &[bool], stamp: &mut [u32], b: u32, top: u32) -> Ground {
    let w = TS as i32;
    let reach = ((2.0 * (pixels.len() as f64 / std::f64::consts::PI).sqrt()).ceil() as usize).max(4);
    // (Rings outward, one Chebyshev step at a time; their pixels stamped so as not to be counted
    // twice: with `b | 1 << 31`, apart from the blob's own.)
    let ring_mark = b | 1 << 31;
    let mut front: Vec<u32> = pixels.to_vec();
    let (mut vals, mut at): (Vec<f32>, Vec<u32>) = (Vec::new(), Vec::new());
    for d in 1..=reach {
        let mut next = Vec::new();
        for &p in &front {
            let (x, y) = ((p % TS as u32) as i32, (p / TS as u32) as i32);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (xx, yy) = (x + dx, y + dy);
                if xx < 0 || yy < 0 || xx >= w || yy >= w {
                    continue;
                }
                let q = (yy * w + xx) as usize;
                if stamp[q] != b && stamp[q] != ring_mark {
                    stamp[q] = ring_mark;
                    next.push(q as u32);
                    if d >= 2 && (held[q] || !out[q]) && v[q].is_finite() {
                        vals.push((sign * v[q]).max(0.0));
                        at.push(q as u32);
                    }
                }
            }
        }
        front = next;
    }
    if vals.is_empty() {
        return Ground::default();
    }
    // The plane z = c0 + c1·dx + c2·dy (dx, dy from the top), by the normal equations.
    let (tx, ty) = ((top % TS as u32) as f64, (top / TS as u32) as f64);
    let mut m = [[0f64; 3]; 3];
    let mut r = [0f64; 3];
    for (&q, &z) in at.iter().zip(&vals) {
        let f = [1.0, (q % TS as u32) as f64 - tx, (q / TS as u32) as f64 - ty];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += f[i] * f[j];
            }
            r[i] += f[i] * z as f64;
        }
    }
    let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(&m);
    let c = if d.abs() > 1e-9 {
        let mut c = [0f64; 3];
        for (k, ck) in c.iter_mut().enumerate() {
            let mut mk = m;
            for i in 0..3 {
                mk[i][k] = r[i];
            }
            *ck = det(&mk) / d;
        }
        c
    } else {
        [r[0] / m[0][0].max(1.0), 0.0, 0.0]
    };
    let mut res: Vec<f32> = at.iter().zip(&vals).map(|(&q, &z)| (z as f64 - c[0] - c[1] * ((q % TS as u32) as f64 - tx) - c[2] * ((q / TS as u32) as f64 - ty)) as f32).collect();
    let n = vals.len();
    let q1 = *vals.select_nth_unstable_by(n / 4, |a, b| a.total_cmp(b)).1;
    let median = *vals.select_nth_unstable_by(n / 2, |a, b| a.total_cmp(b)).1;
    let q3 = *vals.select_nth_unstable_by(n * 3 / 4, |a, b| a.total_cmp(b)).1;
    let r1 = *res.select_nth_unstable_by(n / 4, |a, b| a.total_cmp(b)).1;
    let r3 = *res.select_nth_unstable_by(n * 3 / 4, |a, b| a.total_cmp(b)).1;
    Ground { median, rough: q3 - q1, plane: c[0] as f32, plane_rough: r3 - r1 }
}

/// Fills the pixels of `hole` from the others (a 256 × 256 tile with some of them): the smoothest
/// surface through the ground around (harmonic: each filled pixel the mean of its four neighbours
/// in the tile), so it has no bump or dip of its own, nothing higher than the highest pixel around
/// it or lower than the lowest. Solved by over-relaxation from `pull_push`'s fill, until no pixel
/// moves a millimetre.
fn fill_from_around(t: &mut [f32], hole: &[bool]) {
    pull_push(t, hole);
    let idx: Vec<usize> = (0..t.len()).filter(|&i| hole[i]).collect();
    let mut u: Vec<f64> = t.iter().map(|&v| v as f64).collect();
    for _ in 0..20_000 {
        let mut big = 0f64;
        for &i in &idx {
            let (x, y) = (i % TS, i / TS);
            let (mut s, mut n) = (0f64, 0u32);
            if x > 0 {
                s += u[i - 1];
                n += 1;
            }
            if x + 1 < TS {
                s += u[i + 1];
                n += 1;
            }
            if y > 0 {
                s += u[i - TS];
                n += 1;
            }
            if y + 1 < TS {
                s += u[i + TS];
                n += 1;
            }
            let d = s / n as f64 - u[i];
            u[i] += 1.9 * d;
            big = big.max(d.abs());
        }
        if big < 0.001 {
            break;
        }
    }
    for &i in &idx {
        t[i] = u[i] as f32;
    }
}

/// A first fill of `hole` from the other pixels: their weighted means halved level by level down
/// to one pixel, then each level's gaps filled from the level below, bilinearly, back up.
fn pull_push(t: &mut [f32], hole: &[bool]) {
    let mut levels: Vec<(usize, Vec<f64>, Vec<f64>)> = Vec::new();
    let mut val: Vec<f64> = t.iter().zip(hole).map(|(&v, &h)| if h { 0.0 } else { v as f64 }).collect();
    let mut wt: Vec<f64> = hole.iter().map(|&h| if h { 0.0 } else { 1.0 }).collect();
    let mut n = TS;
    while n > 1 {
        let m = n / 2;
        let (mut nv, mut nw) = (vec![0f64; m * m], vec![0f64; m * m]);
        for j in 0..m {
            for i in 0..m {
                let (mut s, mut ws) = (0.0, 0.0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let k = (2 * j + dy) * n + 2 * i + dx;
                    s += val[k] * wt[k];
                    ws += wt[k];
                }
                nv[j * m + i] = if ws > 0.0 { s / ws } else { 0.0 };
                nw[j * m + i] = ws.min(1.0);
            }
        }
        levels.push((n, std::mem::replace(&mut val, nv), std::mem::replace(&mut wt, nw)));
        n = m;
    }
    // (val is now the 1 × 1 level.)
    let mut up = val;
    for (n, v, w) in levels.into_iter().rev() {
        let m = n / 2;
        let at = |i: usize| -> (usize, usize, f64) {
            let c = (i as f64 + 0.5) / 2.0 - 0.5;
            let f = c.floor();
            let (c0, c1) = ((f.max(0.0) as usize).min(m - 1), ((f + 1.0).max(0.0) as usize).min(m - 1));
            (c0, c1, c - f)
        };
        let mut next = vec![0f64; n * n];
        for j in 0..n {
            let (y0, y1, fy) = at(j);
            for i in 0..n {
                let (x0, x1, fx) = at(i);
                let a = up[y0 * m + x0] * (1.0 - fx) + up[y0 * m + x1] * fx;
                let b = up[y1 * m + x0] * (1.0 - fx) + up[y1 * m + x1] * fx;
                let k = j * n + i;
                next[k] = w[k] * v[k] + (1.0 - w[k]) * (a * (1.0 - fy) + b * fy);
            }
        }
        up = next;
    }
    for (k, h) in hole.iter().enumerate() {
        if *h {
            t[k] = up[k] as f32;
        }
    }
}

/// Encode metres as a Terrarium RGB PNG (quantised to 1/256 m).
pub fn encode_terrain_png(elev: &[f32], w: u32, h: u32) -> Result<Vec<u8>> {
    let mut rgb = Vec::with_capacity(elev.len() * 3);
    for &e in elev {
        let v = ((e + 32768.0) * 256.0).round().clamp(0.0, 16_777_215.0) as u32;
        rgb.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc.write_header()?;
        wr.write_image_data(&rgb)?;
    }
    Ok(out)
}

/// Decode tile (z, x, y); if absent, crop + bilinearly upsample the nearest stored ancestor.
pub fn tile_with_fallback(arc: &crate::archive::Archive, z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
    tile_with_fallback_by(&|z, x, y| arc.get(z, x, y).map(<[u8]>::to_vec), z, x, y)
}

/// `tile_with_fallback` over any source of Terrarium PNG tiles.
pub fn tile_with_fallback_by(get: &dyn Fn(u8, u32, u32) -> Option<Vec<u8>>, z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
    if let Some(b) = get(z, x, y) {
        return decode_terrain_png(&b).ok();
    }
    for dz in 1..=z.min(8) {
        let (pz, px, py) = (z - dz, x >> dz, y >> dz);
        let Some(b) = get(pz, px, py) else { continue };
        let p = decode_terrain_png(&b).ok()?;
        return Some(upsampled(&p, dz, x, y));
    }
    None
}

/// Tile (z, x, y) from its ancestor `dz` levels up (`p`, decoded): bilinear at its pixel centres.
pub fn upsampled(p: &[f32], dz: u8, x: u32, y: u32) -> Vec<f32> {
    let (px, py) = (x >> dz, y >> dz);
    let n = 1u32 << dz;
    let (ox, oy) = ((x - (px << dz)) as f64 * 256.0 / n as f64, (y - (py << dz)) as f64 * 256.0 / n as f64);
    let s = 1.0 / n as f64;
    let mut out = vec![0f32; 256 * 256];
    for j in 0..256 {
        for i in 0..256 {
            out[j * 256 + i] = bilinear(p, 256, ox + (i as f64 + 0.5) * s - 0.5, oy + (j as f64 + 0.5) * s - 0.5);
        }
    }
    out
}

/// Bilinear sample of a square `w`-wide grid at pixel-centre coordinates (clamped).
#[inline]
pub fn bilinear(a: &[f32], w: usize, x: f64, y: f64) -> f32 {
    let h = a.len() / w;
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let v00 = a[y0 * w + x0];
    let v01 = a[y0 * w + x1];
    let v10 = a[y1 * w + x0];
    let v11 = a[y1 * w + x1];
    (v00 * (1.0 - fx) + v01 * fx) * (1.0 - fy) + (v10 * (1.0 - fx) + v11 * fx) * fy
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    /// A tile from a function of the pixel (x, y).
    fn tile(f: impl Fn(f32, f32) -> f32) -> Vec<f32> {
        (0..TS * TS).map(|i| f((i % TS) as f32, (i / TS) as f32)).collect()
    }

    fn slope_tile() -> Vec<f32> {
        tile(|x, _| x * 2.0 + 100.0)
    }

    /// Rough ground: hills of a few hundred metres at several scales (deterministic).
    fn hills(base: f32) -> Vec<f32> {
        tile(|x, y| {
            base + 180.0 * ((x * 0.031).sin() * (y * 0.027).cos())
                + 60.0 * ((x * 0.17 + 1.3).sin() * (y * 0.13 + 0.4).sin())
                + 12.0 * ((x * 0.9).sin() + (y * 1.1).cos())
        })
    }

    /// The repair, then the repair of its own output, which must change nothing.
    fn repaired(t: &mut [f32], z: u8, lat: f64) -> Repair {
        let r = repair_terrain(t, z, lat);
        // Repaired again as stored (at sea level and over), it changes nothing.
        let mut stored: Vec<f32> = t.iter().map(|v| v.max(0.0)).collect();
        let once = stored.clone();
        let again = repair_terrain(&mut stored, z, lat);
        assert!(!again.changed() && again.blobs == 0, "a second pass found more: {again:?} ({r:?} first)");
        assert_eq!(stored, once);
        r
    }

    #[test]
    fn fills_voids_from_their_surroundings() {
        let mut t = slope_tile();
        for y in 100..120 {
            for x in 100..120 {
                t[y * TS + x] = 32767.0;
            }
        }
        t[5 * TS + 7] = f32::NAN;
        t[9 * TS + 9] = -32768.0;
        let r = repaired(&mut t, 12, 45.0);
        assert_eq!(r.voids, 402);
        // a plane, filled from around: close to the plane inside
        for (i, &v) in t.iter().enumerate() {
            let want = (i % TS) as f32 * 2.0 + 100.0;
            assert!((v - want).abs() < 25.0, "{v} vs {want}");
        }
        // a tile with nothing defined: sea level
        let mut all = vec![32767f32; TS * TS];
        assert_eq!(repair_terrain(&mut all, 10, 40.0).voids, TS * TS);
        assert!(all.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn flattens_towers_over_water_and_keeps_ridges() {
        // a 2-pixel tower in a bay (z9, as off Shinagawa)
        let mut t = vec![0f32; TS * TS];
        t[100 * TS + 100] = 841.0;
        t[100 * TS + 101] = 665.0;
        let r = repaired(&mut t, 9, 35.6);
        assert_eq!((r.blobs, r.blob_pixels), (1, 2));
        assert!(t.iter().all(|&v| v.abs() < 1.0));
        // a sharp ridge 300 m above its valleys at z12 (67° flanks): kept
        let mut r = tile(|x, _| (1300.0 - (x - 128.0).abs() * 60.0).max(1000.0));
        let before = r.clone();
        repaired(&mut r, 12, 45.0);
        assert_eq!(r, before);
    }

    #[test]
    fn flattens_whole_clusters_in_one_pass() {
        // a block of 6 × 6 pixels 1,500 m up off a flat shore (z10): every pixel, at once
        let mut t = vec![3f32; TS * TS];
        for y in 60..66 {
            for x in 80..86 {
                t[y * TS + x] = 1500.0 + (x * y) as f32;
            }
        }
        let r = repaired(&mut t, 10, 44.3);
        assert_eq!(r.blob_pixels, 36);
        assert!(t.iter().all(|&v| (v - 3.0).abs() < 0.01));
        // a smooth bump with flanks, 2.3 km on coastal flats (z12, as on Mweelrea's shore): the
        // flanks too, down to the last few metres (cut where they stop rising faster than ground)
        let mut b = tile(|x, y| 10.0 + 0.05 * x + 2300.0 * (-((x - 160.0).powi(2) / 1.5 + (y - 76.0).powi(2) / 2.5)).exp());
        let flats = tile(|x, _| 10.0 + 0.05 * x);
        let r = repaired(&mut b, 12, 53.6);
        assert!(r.blob_pixels >= 9, "{r:?}");
        assert!(b.iter().zip(&flats).all(|(v, f)| (v - f).abs() < 10.0));
        // towers ringing with pits in a lake 274 m up (z10, Shumarinai): a pit as AWS has it
        // (below sea level), beside each tower
        let mut l = vec![274f32; TS * TS];
        for (y, up, down) in [(20, 1185.0, -92.0), (21, 2430.0, -553.0), (22, 2740.0, -655.0), (23, 1770.0, -275.0)] {
            l[y * TS + 145] = up;
            l[y * TS + 144] = down;
        }
        let r = repaired(&mut l, 10, 44.3);
        assert_eq!(r.blob_pixels, 8);
        assert!(l.iter().all(|&v| (v - 274.0).abs() < 0.01));
    }

    #[test]
    fn keeps_summits_cliffs_islands_and_mesas() {
        // a real summit at z8 (~430 m pixels at 45°), a cone 300 m higher a pixel in
        let mut s = tile(|x, y| (3600.0 - 300.0 * (x - 80.0).abs().max((y - 80.0).abs())).max(0.0));
        let before = s.clone();
        repaired(&mut s, 8, 45.0);
        assert_eq!(s, before);
        // Soffeh, a ridge 700 m above the Isfahan plain, a pixel or two wide at z7 (~1 km pixels)
        let mut r = tile(|x, y| {
            let d = (x - 90.0 - 0.3 * (y - 120.0)).abs();
            1580.0 + 4.0 * (x * 0.2).sin() + if (100.0..116.0).contains(&y) { (700.0 - 450.0 * d).max(0.0) } else { 0.0 }
        });
        let before = r.clone();
        repaired(&mut r, 7, 32.6);
        assert_eq!(r, before);
        // a cliff 800 m high, straight down (z12)
        let mut c = tile(|x, y| if x + 0.3 * y > 140.0 { 900.0 + y } else { 100.0 });
        let before = c.clone();
        repaired(&mut c, 12, 45.0);
        assert_eq!(c, before);
        // an island 300 m high and ~500 m across, steep out of the sea (z12, ~27 m pixels)
        let mut i = tile(|x, y| (300.0 - 33.0 * ((x - 128.0).powi(2) + (y - 128.0).powi(2)).sqrt()).max(-40.0));
        let before = i.clone();
        repaired(&mut i, 12, 45.0);
        assert_eq!(i, before);
        // a sea stack of 90 m, one pixel (z12): below BLOB_RISE
        let mut st = vec![-5f32; TS * TS];
        st[40 * TS + 40] = 90.0;
        let before = st.clone();
        repaired(&mut st, 12, 58.9);
        assert_eq!(st, before);
        // a mesa: 20 × 20 pixels, 300 m up, walls straight down (z12)
        let mut m = tile(|x, y| if (100.0..120.0).contains(&x) && (50.0..70.0).contains(&y) { 1500.0 } else { 1200.0 + 0.1 * x });
        let before = m.clone();
        repaired(&mut m, 12, 36.0);
        assert_eq!(m, before);
        // a lone needle 2 km up, one z12 pixel: gone; a crane of 66 m beside it: kept
        let mut n = vec![500f32; TS * TS];
        n[50 * TS + 50] = 2500.0;
        n[52 * TS + 60] = 566.0;
        let r = repaired(&mut n, 12, 45.0);
        assert_eq!(r.blob_pixels, 1);
        assert_eq!((n[50 * TS + 50], n[52 * TS + 60]), (500.0, 566.0));
    }

    /// AWS's own values (15 × 15 pixels around a cluster), set in a tile of the ground around them.
    fn aws(win: &[[i16; 15]; 15], ground: f32) -> Vec<f32> {
        let mut t = vec![ground; TS * TS];
        for (j, row) in win.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                t[(100 + j) * TS + 100 + i] = v as f32;
            }
        }
        t
    }

    fn at(t: &[f32], i: usize, j: usize) -> f32 {
        t[(100 + j) * TS + 100 + i]
    }

    /// 10/916/371 around pixel 145,19 (44.32 N, 142.23 E): Lake Shumarinai, 274 m up, with towers to
    /// 2,740 m ringing with pits to −655 m along its shore (1 October's third pass still took 2,466 m).
    const SHUMARINAI_Z10: [[i16; 15]; 15] = [
        [284, 287, 291, 294, 291, 296, 304, 245, 225, 274, 273, 274, 280, 286, 316],
        [283, 287, 291, 293, 282, 309, 368, 34, 185, 274, 274, 274, 279, 265, 339],
        [282, 287, 291, 291, 289, 287, 297, 172, 315, 274, 273, 274, 279, 298, 292],
        [282, 288, 290, 289, 330, 182, -92, 1185, 783, 274, 273, 275, 283, 447, 96],
        [283, 290, 288, 286, 378, 60, -553, 2430, 274, 275, 272, 276, 290, 616, -121],
        [282, 288, 285, 284, 387, 31, -655, 2740, 274, 275, 271, 277, 299, 653, -158],
        [278, 283, 282, 282, 343, 129, -275, 1770, 1025, 275, 271, 278, 302, 508, 42],
        [275, 279, 281, 281, 270, 288, 324, 242, 196, 275, 271, 277, 298, 261, 354],
        [275, 278, 282, 280, 202, 431, 835, -1025, -732, 275, 272, 275, 290, 23, 627],
        [276, 278, 278, 279, 164, 506, 274, 274, 274, 274, 273, 275, 284, -97, 761],
        [278, 278, 276, 277, 168, 497, 274, 274, 274, 274, 273, 274, 281, -47, 720],
        [277, 277, 278, 276, 274, 274, 274, 274, 274, 274, 273, 274, 281, 85, 589],
        [277, 276, 277, 275, 274, 274, 274, 274, 274, 274, 273, 274, 282, 125, 541],
        [275, 274, 274, 274, 274, 274, 274, 274, 274, 274, 273, 274, 281, 63, 588],
        [274, 274, 274, 274, 274, 274, 274, 274, 274, 274, 274, 274, 278, 51, 570],
    ];

    /// 12/1935/1322 around pixel 161,78 (53.63 N, 9.88 W): a bump of 2,307 m on the coastal flats
    /// below Mweelrea, its flanks down to the flats, cut at a seam above it.
    const MWEELREA_Z12: [[i16; 15]; 15] = [
        [15, 15, 16, 17, 18, 18, 19, 20, 21, 22, 22, 23, 24, 25, 26],
        [15, 15, 16, 17, 17, 18, 19, 20, 21, 21, 22, 23, 24, 25, 26],
        [15, 15, 16, 17, 17, 18, 19, 20, 20, 21, 22, 23, 24, 25, 26],
        [15, 15, 16, 17, 17, 18, 19, 20, 20, 21, 22, 23, 24, 25, 26],
        [14, 15, 16, 16, 17, 18, 19, 19, 20, 21, 22, 23, 24, 25, 26],
        [9, 11, 10, 11, 14, 283, 1563, 2307, 1388, 284, 10, 5, 4, 3, 2],
        [6, 13, 14, 15, 18, 154, 801, 1305, 820, 174, 12, 8, 6, 4, 4],
        [6, 14, 16, 19, 21, 81, 363, 610, 396, 93, 15, 11, 9, 7, 6],
        [7, 14, 17, 20, 23, 41, 121, 193, 131, 42, 18, 15, 12, 10, 8],
        [10, 13, 16, 8, 9, 10, 11, 12, 11, 11, 10, 9, 8, 7, 6],
        [8, 10, 13, 7, 8, 9, 10, 10, 10, 10, 10, 8, 7, 7, 6],
        [6, 7, 9, 6, 6, 7, 8, 9, 9, 9, 8, 8, 7, 6, 6],
        [4, 5, 6, 8, 11, 13, 7, 7, 7, 7, 7, 6, 6, 5, 5],
        [5, 6, 7, 8, 7, 10, 6, 6, 5, 6, 6, 5, 4, 4, 4],
        [5, 6, 7, 8, 5, 7, 5, 4, 3, 4, 4, 3, 3, 4, 4],
    ];

    /// 10/902/399 around pixel 38,112 (36.76 N, 137.17 E): towers to 986 m ringing with pits to
    /// −3,388 m off the Toyama shore (the owner's view of 1 October), a lesser tower (496 m) joined
    /// to a greater one (672 m) by its flank.
    const TOYAMA_Z10: [[i16; 15]; 15] = [
        [2, 1, 1, -14, -12, -10, -8, -6, -4, -3, -1, 1, 2, 3, 4],
        [2, 6, 89, -70, -15, -12, -10, -7, -4, -2, 0, 2, 4, 6, 7],
        [2, 4, 45, -25, 3, 3, 2, 1, 1, 1, 1, 4, 6, 8, 9],
        [2, 4, 467, -254, -3232, 3, 2, 2, -3002, -1889, 702, -55, 7, 9, 11],
        [3, 3, 645, -358, 3, 3, 2, 2, -2455, -1890, 986, 28, 1, 1, 1],
        [4, 3, 282, -156, -2090, 2, 1, 2, -939, -768, 431, 17, 13, 12, 15],
        [4, 2, -60, 33, 439, -954, -3012, -1804, 222, 175, -89, -1, 1, 1, -5],
        [3, 1, -68, 37, 496, -1080, -3388, -2025, 266, 204, -101, -1, 0, 0, -2],
        [2, 0, 13, -7, -100, 217, 672, 401, -57, -41, 23, 1, 1, 2, 4],
        [0, 0, 11, -5, -76, 165, 510, 305, -44, -31, 17, 2, 2, 3, 4],
        [-1, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 2, 3, 3, 4],
        [-2, 0, 1, 1, 0, -1, 0, 1, 2, 1, 1, 1, 2, 2, 2],
        [-2, 0, 0, 0, -1, -1, 0, 1, 2, 1, 1, 1, 1, 1, 1],
        [-2, -1, -1, -1, -1, -1, 1, 2, 3, 2, 1, 2, 2, 1, 0],
        [-1, -1, -1, -1, -1, 0, 2, 3, 3, 2, 2, 2, 2, 2, 0],
    ];

    #[test]
    fn clears_awss_own_clusters_whole() {
        // Shumarinai: the towers and the pits beside them, in one pass; the lake left as it was.
        let mut t = aws(&SHUMARINAI_Z10, 274.0);
        repaired(&mut t, 10, 44.32);
        for (j, row) in SHUMARINAI_Z10.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                let now = at(&t, i, j);
                if v >= 783 {
                    assert!(now < 450.0, "tower {v} at {i},{j}: {now}");
                }
                if (5..=8).contains(&i) && v < 0 {
                    assert!(now > 200.0, "pit {v} at {i},{j}: {now}");
                }
                if v == 274 {
                    assert_eq!(now, 274.0, "the lake at {i},{j}");
                }
            }
        }
        // Mweelrea's bump, flanks and all, back to the flats.
        let mut m = aws(&MWEELREA_Z12, 15.0);
        repaired(&mut m, 12, 53.63);
        for j in 5..9 {
            for i in 5..10 {
                assert!(at(&m, i, j) < 30.0, "{i},{j}: {}", at(&m, i, j));
            }
        }
        // Toyama: every tower down to the sea, the lesser one too (it stood on the greater's flank),
        // and the map's sea left at sea level.
        let mut y = aws(&TOYAMA_Z10, 0.0);
        repaired(&mut y, 10, 36.76);
        for j in 0..15 {
            for i in 0..15 {
                assert!(at(&y, i, j).max(0.0) < 300.0, "{i},{j}: {}", at(&y, i, j));
            }
        }
    }

    /// 9/454/201 around pixel 212,172 (35.65 N, 139.80 E): off Odaiba at z9, a tower of 1,767 m
    /// and a column of 468 and 225 m standing on the waterfront's flat ground.
    const ODAIBA_Z9: [[i16; 15]; 15] = [
        [4, 2, 1, 1, 8, 6, 8, 10, 6, 7, 6, 5, 5, 4, 2],
        [8, 8, 3, 6, 11, 7, 10, -2, 16, 17, -5, 8, 4, 8, 5],
        [19, 11, 3, 8, 7, 7, -1, 33, -60, -20, 55, -8, 10, 9, 3],
        [17, 14, 7, 6, 5, 4, 27, -102, 284, 22, -134, 55, 5, 8, 6],
        [5, 11, 13, 8, 7, 6, 59, -196, 404, 1767, -531, 136, -17, 12, 6],
        [2, 6, 0, 6, 9, 10, 30, -48, 5, 5, 6, 5, 4, 5, 6],
        [-1, 1, 4, 3, 5, 6, 14, -51, 2, -152, -38, 20, 7, 5, 8],
        [-1, 4, 6, 6, 3, 1, 0, 24, 1, -73, 53, -10, 4, 6, 8],
        [3, 5, 6, 7, 3, 3, 17, -76, 468, 6, 12, 14, 10, 8, 4],
        [1, 6, 4, 3, 3, 3, 14, -26, 225, 7, 20, 4, 10, 3, 3],
        [2, 1, 3, 3, 4, 4, 0, 50, -182, 5, 18, 11, 5, 0, 6],
        [3, 3, 4, 4, 5, -3, -1, 36, -86, 2, -1, 0, 0, 0, 4],
        [6, 6, 6, 1, 1, 0, 9, -4, 55, 1, 1, 0, -1, -1, 5],
        [13, 2, 4, 10, 8, 10, 7, 6, 5, 1, 11, 0, -1, -1, 5],
        [13, 7, 18, 20, 13, 24, 12, 4, 3, 2, 8, -1, -1, -1, 1],
    ];

    /// 10/317/369 around pixel 230,156 (44.69 N, 68.24 W): on the Maine coast, a lake 65 m up
    /// ringing along its shore, towers to 2,017 m beside pits to −960 m (1 October's third pass
    /// still took 1,562 m).
    const MAINE_Z10: [[i16; 15]; 15] = [
        [95, 99, 98, 92, 83, 77, 66, 55, 94, 77, -47, -67, 35, 86, 90],
        [100, 103, 102, 96, 88, 84, 59, 29, 136, 88, -258, -316, -39, 96, 98],
        [102, 106, 105, 99, 91, 77, 71, 69, 44, 56, 132, 149, 93, 64, 64],
        [101, 108, 107, 103, 93, 66, 90, 136, -70, 38, 65, 66, 295, 16, 10],
        [96, 106, 108, 107, 96, 66, 90, 149, -22, 91, 65, 65, 294, 26, 18],
        [89, 99, 106, 109, 102, 87, 62, 67, 267, 246, 65, 65, -33, 122, 120],
        [81, 91, 102, 107, 108, 144, -18, -185, 947, 65, 65, 65, 65, 65, 65],
        [77, 84, 96, 97, 112, 235, -147, -610, 2017, 66, 65, 65, 65, 65, 65],
        [74, 78, 87, 83, 111, 314, -250, -960, 71, 1497, 65, 65, 65, 65, 65],
        [72, 74, 81, 74, 105, 320, -226, -918, 72, 1634, 65, 65, 65, 65, 65],
        [66, 69, 76, 70, 94, 250, -88, -506, 1520, 1414, 65, 65, 65, 65, 65],
        [60, 64, 69, 67, 82, 164, 44, -90, 430, 969, 65, 65, 65, 65, 65],
        [54, 56, 59, 61, 73, 109, 89, 73, 27, 508, 64, 65, 65, 65, 65],
        [49, 50, 51, 55, 64, 79, 79, 75, 53, 169, 64, 65, 65, 65, 65],
        [46, 48, 49, 50, 53, 58, 69, 75, 70, 24, 67, 64, 65, 65, 65],
    ];

    #[test]
    fn takes_spikes_on_flat_ground_whole() {
        // Odaiba at z9: the tower; the rest as it was (as the map shows it: at or above sea level).
        // The column of 468 m on the waterfront, 1.8 times as tall as it's wide at z9's 250 m
        // pixels, stays in this tile alone, as a butte or an islet would: the terrain job takes
        // it at z10, and makes z9 again from there.
        let mut t = aws(&ODAIBA_Z9, 3.0);
        repaired(&mut t, 9, 35.65);
        for (j, row) in ODAIBA_Z9.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                let now = at(&t, i, j).max(0.0);
                if v >= 1000 {
                    assert!(now < 60.0, "{v} at {i},{j}: {now}");
                } else if (0..100).contains(&v) && !(7..=9).contains(&i) {
                    assert_eq!(now, v as f32, "{i},{j}");
                }
            }
        }
        // The Maine shore: its towers in one pass; the lake left as it was.
        let mut m = aws(&MAINE_Z10, 65.0);
        repaired(&mut m, 10, 44.69);
        for (j, row) in MAINE_Z10.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                let now = at(&m, i, j);
                if v >= 900 {
                    assert!(now < 150.0, "{v} at {i},{j}: {now}");
                }
                if v == 65 {
                    assert_eq!(now, 65.0, "the lake at {i},{j}");
                }
            }
        }
    }

    /// 12/3632/1593 from pixel 61,4 (37.06 N, 139.26 E): towers to 8,105 m and pits to −2,786 m
    /// ringing on mountain ground 710–870 m up.
    const AIZU_Z12: [[i16; 15]; 15] = [
        [739, 768, 736, 727, 743, 752, 759, 768, 779, 794, 814, 829, 842, 853, 866],
        [769, 795, 741, 712, 725, 737, 749, 760, 772, 787, 804, 817, 827, 836, 848],
        [714, 715, 718, 722, 713, 723, 738, 749, 764, 779, 793, 804, 813, 823, 836],
        [714, 713, 714, 717, 690, 716, 729, 740, 757, 772, 784, 793, 805, 818, 831],
        [714, 712, 711, 713, 716, 721, 728, 733, 750, 764, 775, 785, 799, 815, 828],
        [716, 712, 710, 710, 712, 717, 723, 1228, 1008, 704, 739, 797, 796, 812, 823],
        [719, 714, 711, 709, 710, 714, 719, 1472, 1083, 671, 720, 800, 790, 804, 812],
        [724, 717, 712, 710, 710, 712, 716, -2682, -932, 1033, 945, 679, 782, 793, 802],
        [723, 717, 715, 711, 710, 711, 714, 720, -1052, 996, 924, 672, 771, 783, 794],
        [731, 720, 718, 713, 710, 710, 713, 718, 8105, -319, 25, 1093, 766, 784, 793],
        [740, 723, 721, 715, 711, 710, 711, 715, 721, -2086, -1063, 1619, 764, 785, 795],
        [743, 723, 724, 717, 712, 709, 710, 712, 717, -2786, -1086, 1675, 762, 766, 794],
        [743, 724, 715, 1048, 713, 710, 709, 710, 714, 720, -154, 1260, 756, 740, 785],
        [746, 728, 718, 571, 715, 711, 709, 709, 712, 717, 597, 798, 748, 744, 770],
        [754, 737, 723, 678, 719, 713, 710, 710, 711, 714, 720, 726, 737, 762, 754],
    ];

    /// 12/3627/1590 from pixel 69,37 (37.27 N, 138.82 E): a lake 142 m up, towers to 2,939 m and
    /// pits to −280 m along its shore.
    const NIIGATA_Z12: [[i16; 15]; 15] = [
        [94, 102, 110, 114, 117, 118, 118, 118, 121, 126, 131, 133, 132, 131, 132],
        [105, 110, 115, 120, 124, 126, 126, 120, 117, 127, 136, 133, 130, 130, 132],
        [116, 116, 119, 124, 130, 134, 134, 134, 136, 137, 137, 134, 132, 133, 136],
        [126, 123, 121, 125, 136, 137, 127, 190, 260, 199, 124, 132, 140, 137, 141],
        [133, 128, 124, 127, 135, 142, 143, 153, 161, 150, 140, 143, 144, 144, 142],
        [139, 132, 128, 130, 125, 150, 199, -29, -280, -66, 198, 170, 138, 142, 142],
        [141, 134, 132, 135, 132, 139, 155, 80, -1, 79, 171, 159, 148, 142, 142],
        [140, 136, 135, 137, 170, 98, -57, 700, 1514, 142, 142, 142, 142, 142, 142],
        [137, 138, 138, 139, 205, 65, -238, 1263, 2918, 142, 142, 142, 142, 142, 142],
        [135, 138, 140, 140, 207, 70, -189, 1186, 2939, 142, 142, 142, 142, 142, 142],
        [136, 139, 141, 140, 181, 99, -77, 856, 143, 143, 142, 142, 142, 142, 142],
        [141, 141, 142, 140, 156, 126, -40, 621, 143, 143, 142, 142, 142, 142, 142],
        [146, 143, 142, 141, 142, 145, 144, 143, 143, 143, 142, 142, 142, 142, 143],
        [146, 144, 144, 144, 140, 152, 143, 143, 143, 143, 142, 142, 142, 142, 143],
        [143, 144, 146, 147, 146, 150, 143, 143, 143, 143, 142, 142, 142, 142, 142],
    ];

    #[test]
    fn fills_pits_on_land_from_the_land_though_they_reach_below_zero() {
        // A pit in raised ground whose deepest part is below zero is filled from the ground around
        // it, as the map shows it whole (its part below zero, found on its own, isn't under the
        // sea): no hole to sea level is left in the mountains or beside the lake.
        let mut t = aws(&AIZU_Z12, 715.0);
        repaired(&mut t, 12, 37.06);
        for j in 0..15 {
            for i in 0..15 {
                let now = at(&t, i, j);
                assert!((560.0..=900.0).contains(&now), "{} at {i},{j}: {now}", AIZU_Z12[j][i]);
            }
        }
        let mut n = aws(&NIIGATA_Z12, 142.0);
        repaired(&mut n, 12, 37.27);
        for j in 0..15 {
            for i in 0..15 {
                let now = at(&n, i, j);
                assert!((60.0..=270.0).contains(&now), "{} at {i},{j}: {now}", NIIGATA_Z12[j][i]);
            }
        }
    }

    /// 10/895/406 from pixel 236,208 (34.6 N, 134.9 E): the Akashi Strait at z10, a band of towers
    /// to 2,605 m beside pits to −2,132 m, and nearer the shore its ringing, lobes of 55–268 m
    /// beside pits.
    const AKASHI_Z10: [[i16; 15]; 15] = [
        [1, 1, 3, 4, 3, 3, 4, 4, 4, 4, 4, 4, 5, 4, 2],
        [0, 2, 2, 2, 2, 2, 3, 3, 4, 5, 6, 6, 6, 6, 5],
        [3, 2, 0, 0, 2, 2, 1, 3, 6, 6, 7, 7, 7, 8, 7],
        [6, 0, -3, 0, 4, 3, -1, 3, 8, 5, 5, 7, 6, 6, 6],
        [-14, 5, 14, 5, -8, -5, 4, -7, -15, -1, 7, 5, 4, 3, 4],
        [1, 11, 31, 10, -20, -13, 11, -17, -37, -7, 9, 3, 1, 1, 1],
        [1, -29, -85, -25, 55, 37, -25, 55, 110, 27, -16, 1, 2, 1, 1],
        [-45, -45, -45, -46, 141, 94, -66, 130, 268, 64, -41, 0, 4, 3, 2],
        [-53, -53, -53, -53, -162, -110, 79, -151, -312, -71, 53, 3, -2, 1, 2],
        [-61, -61, -61, -61, -60, -59, -58, -57, -55, -53, -51, -48, -45, -41, -38],
        [-69, -68, -68, -68, -67, -66, -65, -63, -62, -59, -57, -54, -51, -47, -44],
        [2553, -799, -2132, -462, 1550, 1014, -671, 1228, 2605, 609, -426, -14, 32, 11, 2],
        [-82, -1331, -81, -824, 2577, 1691, -1135, 2096, -73, 1033, -723, -24, 53, 18, 3],
        [-88, -87, -86, -85, -84, -83, -82, -80, -78, -75, -73, -70, -67, -64, -61],
        [-92, -92, -91, -90, -89, -87, -86, -84, -82, -80, -77, -75, -72, -69, -66],
    ];

    /// 8/228/110 from pixel 145,51 (24.23 N, 141.46 E): Minami-Iwo-jima at z8, 916 m up on 3.5 km²
    /// of sea floor: as AWS draws it, steeper than 45° over its width, and less walled.
    const MINAMI_IWO_Z8: [[i16; 15]; 15] = [
        [-603, -472, -348, -243, -153, -83, -39, -23, -29, -53, -87, -127, -173, -227, -286],
        [-561, -436, -335, -258, -175, -86, -27, -8, -15, -39, -76, -123, -178, -239, -306],
        [-530, -415, -305, -203, -117, -53, -13, 4, -3, -32, -76, -132, -194, -261, -333],
        [-517, -411, -267, -101, -8, -1, -1, 16, 12, -26, -84, -151, -220, -291, -366],
        [-518, -413, -244, -41, 49, 16, 1, 38, 43, -12, -90, -169, -247, -324, -403],
        [-534, -421, -248, -49, 36, 5, 6, 67, 76, 2, -97, -188, -274, -358, -444],
        [-568, -446, -280, -99, 1, 16, 271, 512, 305, -20, -114, -214, -309, -399, -489],
        [-616, -490, -338, -177, -52, 23, 399, 801, 369, -17, -153, -256, -355, -449, -540],
        [-671, -551, -416, -278, -153, -58, 365, 450, 158, -2, -224, -322, -417, -508, -597],
        [-724, -619, -507, -398, -303, -227, 1, -23, -182, -239, -317, -403, -489, -572, -656],
        [-770, -682, -600, -532, -473, -420, -365, -319, -312, -349, -412, -485, -560, -635, -713],
        [-809, -736, -684, -655, -626, -582, -521, -458, -431, -450, -498, -560, -627, -694, -768],
        [-846, -786, -748, -732, -712, -674, -618, -560, -531, -542, -579, -631, -690, -755, -826],
        [-884, -834, -794, -767, -740, -706, -666, -629, -613, -622, -652, -696, -751, -815, -886],
        [-920, -875, -832, -793, -758, -727, -702, -685, -681, -691, -716, -754, -807, -871, -944],
    ];

    /// 10/202/399 from pixel 100,166 (36.69 N, 108.84 W): Shiprock at z10, 300 m above the plain.
    const SHIPROCK_Z10: [[i16; 15]; 15] = [
        [1675, 1673, 1668, 1666, 1667, 1670, 1674, 1675, 1672, 1670, 1669, 1668, 1665, 1663, 1663],
        [1679, 1680, 1680, 1689, 1693, 1695, 1696, 1694, 1690, 1683, 1678, 1676, 1673, 1668, 1666],
        [1680, 1685, 1694, 1712, 1723, 1728, 1731, 1724, 1708, 1695, 1688, 1684, 1679, 1673, 1669],
        [1680, 1689, 1706, 1730, 1746, 1759, 1772, 1766, 1740, 1716, 1700, 1691, 1685, 1677, 1671],
        [1679, 1690, 1709, 1734, 1751, 1769, 1793, 1800, 1778, 1743, 1714, 1699, 1690, 1680, 1673],
        [1680, 1691, 1708, 1731, 1745, 1769, 1813, 1847, 1840, 1783, 1725, 1704, 1695, 1682, 1674],
        [1686, 1694, 1708, 1732, 1743, 1779, 1860, 1935, 1935, 1834, 1729, 1704, 1700, 1686, 1677],
        [1697, 1700, 1710, 1736, 1745, 1792, 1905, 2008, 2006, 1868, 1727, 1700, 1702, 1689, 1679],
        [1712, 1711, 1713, 1734, 1739, 1784, 1897, 1995, 1987, 1851, 1719, 1698, 1702, 1688, 1678],
        [1722, 1719, 1714, 1726, 1728, 1761, 1841, 1904, 1890, 1795, 1709, 1697, 1699, 1684, 1676],
        [1719, 1718, 1714, 1718, 1720, 1740, 1781, 1808, 1794, 1745, 1704, 1696, 1693, 1681, 1674],
        [1706, 1710, 1711, 1711, 1717, 1728, 1737, 1741, 1733, 1717, 1701, 1693, 1687, 1679, 1674],
        [1697, 1703, 1706, 1707, 1714, 1720, 1714, 1707, 1703, 1701, 1696, 1688, 1682, 1677, 1673],
        [1696, 1699, 1702, 1705, 1712, 1716, 1709, 1701, 1696, 1693, 1690, 1684, 1680, 1676, 1672],
        [1696, 1698, 1701, 1706, 1714, 1719, 1712, 1703, 1698, 1692, 1687, 1683, 1680, 1677, 1674],
    ];

    /// 10/902/399 from pixel 138,104 (36.77 N, 137.30 E): Toyama's shore at z10 as the terrain job
    /// makes it (its pixels over repaired z11 ones made again from them), a band of towers to 164 m
    /// along the shore beside a pit of 200 m below the bay's floor.
    const TOYAMA_SHORE_Z10: [[i16; 15]; 15] = [
        [-79, -82, -83, -83, -81, -79, -75, -72, -67, -63, -59, -56, -52, -49, -46],
        [-76, -79, -80, -79, -77, -74, -71, -66, -62, -57, -53, -49, -46, -42, -39],
        [-72, -75, -76, -75, -73, -70, -66, -61, -56, -52, -47, -43, -40, -36, -34],
        [-67, -70, -71, -70, -68, -65, -60, -56, -51, -46, -42, -38, -34, -31, -28],
        [-61, -63, -64, -63, -61, -58, -54, -50, -45, -40, -36, -32, -29, -26, -24],
        [-53, -55, 1, 3, 2, -51, 0, 3, 3, 1, 1, 2, -25, -22, -20],
        [-44, -29, 5, 3, -103, -200, 0, 0, 0, 14, 133, 108, 21, -24, -14],
        [-35, 93, 8, 1, 45, 113, 160, 164, 2, 1, 1, 1, 2, 0, 0],
        [-11, 1, 2, 11, 33, 0, 113, 116, 78, -2, -54, 0, -3, 0, 0],
        [5, 2, 1, 0, -8, -8, -10, -11, -10, 0, 7, 0, 0, 0, 0],
        [3, -3, 8, -2, -3, -8, -11, -12, 0, 0, 8, 5, 0, 0, 0],
        [0, 1, -5, 1, 4, 3, 1, 0, 0, 0, 2, 1, 0, 0, 0],
        [1, -2, -2, 1, 1, 2, 1, 0, 0, 0, 0, 1, 1, 0, 0],
        [2, 3, 3, 1, 1, 1, 1, 0, 0, 0, 1, 1, 1, 1, 2],
        [2, 3, 3, 1, 0, 0, 1, 0, 0, 0, 0, 0, 1, 1, 2],
    ];

    #[test]
    fn takes_towers_beside_a_ringing_pit_under_the_sea() {
        // Toyama's shore: the band (walled 0.92, on flat ground) goes, as it stands beside the
        // pit its ringing dug in the bay; the shore as it was.
        let mut t = aws(&TOYAMA_SHORE_Z10, -40.0);
        repaired(&mut t, 10, 36.77);
        for (j, row) in TOYAMA_SHORE_Z10.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                let now = at(&t, i, j).max(0.0);
                if (7..=8).contains(&j) && (4..=8).contains(&i) {
                    assert!(now <= 60.0, "{v} at {i},{j}: {now}");
                } else if (0..100).contains(&v) {
                    assert_eq!(now, v as f32, "the shore at {i},{j}");
                }
            }
        }
    }

    #[test]
    fn takes_lobes_beside_a_blob_taken_and_keeps_steep_islands_and_plugs() {
        // The Akashi Strait: the band and its lobes (two of three: steep over its width beside a
        // blob taken on flat ground, walled or not), the shore as it was.
        let mut t = aws(&AKASHI_Z10, -60.0);
        repaired(&mut t, 10, 34.6);
        for (j, row) in AKASHI_Z10.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                let now = at(&t, i, j).max(0.0);
                if j >= 6 {
                    assert!(now <= 60.0, "{v} at {i},{j}: {now}");
                } else if v >= 0 {
                    assert_eq!(now, v as f32, "the shore at {i},{j}");
                }
            }
        }
        // An island and a plug as steep, but neither walled nor beside a blob taken: kept.
        for (win, ground, z, lat) in [(&MINAMI_IWO_Z8, -700.0, 8, 24.23), (&SHIPROCK_Z10, 1690.0, 10, 36.69)] {
            let mut t = aws(win, ground);
            let before = t.clone();
            repaired(&mut t, z, lat);
            assert_eq!(t, before);
        }
    }

    /// 10/281/385 from pixel 5,223 (40.58 N, 81.03 W): a pit in Ohio's hills, 290–430 m up, down
    /// to −393 m, its part below zero its own pit.
    const OHIO_Z10: [[i16; 15]; 15] = [
        [329, 349, 355, 351, 345, 342, 323, 308, 314, 316, 294, 299, 311, 320, 324],
        [333, 355, 360, 347, 333, 329, 316, 309, 319, 316, 294, 298, 310, 325, 336],
        [340, 359, 362, 352, 337, 325, 315, 309, 307, 293, 294, 296, 307, 330, 347],
        [343, 354, 355, 355, 346, 331, 334, 328, 292, 261, 294, 296, 310, 339, 356],
        [339, 342, 339, 344, 341, 338, 389, 399, 303, 241, 294, 302, 321, 350, 365],
        [327, 325, 320, 325, 339, 357, 386, 369, 286, 252, 299, 313, 337, 360, 370],
        [313, 311, 311, 312, 360, 397, 249, 110, 191, 294, 307, 327, 352, 362, 366],
        [301, 302, 308, 306, 386, 430, 52, -231, 293, 300, 333, 343, 354, 354, 354],
        [291, 295, 306, 301, 389, 428, -57, -393, 293, 299, 332, 335, 349, 340, 345],
        [285, 290, 300, 295, 362, 386, -24, -293, 294, 297, 317, 317, 333, 324, 337],
        [282, 286, 292, 290, 322, 329, 105, -31, 295, 296, 303, 301, 315, 312, 328],
        [282, 285, 287, 286, 286, 282, 253, 246, 295, 295, 296, 296, 302, 305, 317],
        [289, 291, 289, 287, 273, 268, 340, 389, 294, 295, 294, 296, 294, 299, 305],
        [303, 305, 299, 292, 283, 284, 334, 356, 296, 295, 295, 295, 295, 295, 294],
        [321, 325, 313, 299, 299, 308, 302, 280, 301, 299, 297, 298, 297, 296, 294],
    ];

    /// 12/448/1166 from pixel 66,108 (60.3 N, 140.6 W): in the St. Elias, where two of AWS's
    /// sources meet 1,000 m apart, bands of the higher across the lower, and between them a block of
    /// four pixels 1,030 m above the ground around, its walls on 19 m pixels.
    const ST_ELIAS_STEPS_Z12: [[i16; 15]; 15] = [
        [2459, 2460, 3412, 3433, 3455, 3475, 3492, 3506, 3517, 3525, 3532, 2508, 2522, 2540, 2559],
        [2461, 2463, 3426, 3450, 3473, 3494, 3512, 3528, 3539, 3549, 3556, 2510, 2524, 2541, 2560],
        [2464, 2466, 2468, 2470, 2473, 3512, 3531, 3549, 3563, 3575, 3586, 3594, 3598, 2540, 2558],
        [2467, 2468, 2470, 2473, 2476, 3523, 3543, 3562, 3580, 3596, 3610, 3620, 3626, 2539, 2556],
        [2469, 2471, 2473, 2475, 2478, 2482, 2485, 2487, 2491, 3608, 3624, 2512, 2523, 2537, 2552],
        [2472, 2473, 2475, 2478, 2481, 2485, 2489, 2492, 2496, 3612, 3630, 2514, 2523, 2535, 2548],
        [2474, 2475, 2477, 2480, 2485, 3504, 3529, 2498, 2502, 3614, 3633, 3644, 3648, 2533, 2544],
        [2477, 2478, 2481, 2484, 2490, 3496, 3522, 2505, 2508, 3617, 3638, 3650, 3653, 2533, 2541],
        [2481, 2482, 2485, 2490, 2496, 2502, 2507, 2511, 2514, 3623, 3646, 3658, 3662, 3660, 2540],
        [2485, 2488, 2491, 2496, 2502, 2508, 2512, 2516, 2519, 3628, 3650, 3663, 3668, 3669, 2540],
        [2491, 2494, 2498, 2503, 2508, 2513, 2516, 2520, 2523, 2525, 3647, 3660, 3667, 3671, 2542],
        [2497, 2500, 2504, 2509, 2513, 2517, 2519, 2522, 2526, 2529, 3632, 3645, 3655, 3663, 2546],
        [2503, 2507, 2510, 2514, 2517, 2519, 2521, 2524, 2528, 2532, 3608, 3622, 3635, 3647, 2550],
        [2509, 2512, 2515, 2518, 2520, 2522, 2523, 2526, 2530, 2535, 3586, 3601, 3615, 3629, 2554],
        [2515, 2518, 2520, 2522, 2524, 2524, 2526, 2529, 2533, 2538, 3575, 3588, 3602, 3614, 2558],
    ];

    #[test]
    fn judges_the_tile_as_stored_and_takes_what_is_far_too_steep() {
        // Ohio: the pit is filled from the hills (its part below zero, at sea level as stored, is no
        // hole either: the repair judges the tile as stored before it ends).
        let mut t = aws(&OHIO_Z10, 300.0);
        repaired(&mut t, 10, 40.58);
        for j in 6..=10 {
            for i in 6..=7 {
                let now = at(&t, i, j);
                assert!(now >= 240.0, "{} at {i},{j}: {now}", OHIO_Z10[j][i]);
            }
        }
        // The St. Elias: the block of four goes, 50 times steeper than terrain allows, though its
        // ground is AWS's steps; the steps themselves stay (cliffs: the edges of something larger).
        let mut e = aws(&ST_ELIAS_STEPS_Z12, 2500.0);
        repaired(&mut e, 12, 60.3);
        for (j, row) in ST_ELIAS_STEPS_Z12.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                let now = at(&e, i, j);
                if (6..=7).contains(&j) && (5..=6).contains(&i) {
                    assert!(now < 2700.0, "{v} at {i},{j}: {now}");
                }
            }
        }
        // Among rough mountains, where a summit drawn too sharp stays, a block of 5 × 5 pixels
        // 1,500 m tall (8 times steeper than terrain allows, too large for a spike) goes all the
        // same.
        let rough = |x: f32, y: f32| 2100.0 + 250.0 * (x * 0.16).sin() * (y * 0.13).cos() + 100.0 * (x * 0.5 + y * 0.3).sin();
        let mut h = tile(rough);
        let base = h.clone();
        let block = || (118..123).flat_map(|y| (118..123).map(move |x| (x, y)));
        for (x, y) in block() {
            h[y * TS + x] += 1500.0;
        }
        repaired(&mut h, 12, 60.3);
        for (x, y) in block() {
            assert!(h[y * TS + x] < base[y * TS + x] + 300.0, "{x},{y}: {}", h[y * TS + x]);
        }
    }

    /// 11/391/796 from pixel 48,185 (37.05 N, 111.23 W): Gunsight Butte in Lake Powell, 1,129 m
    /// up, at z11.
    const GUNSIGHT_BUTTE_Z11: [[i16; 15]; 15] = [
        [1129, 1129, 1129, 1128, 1128, 1128, 1127, 1124, 1124, 1129, 1129, 1129, 1129, 1129, 1129],
        [1129, 1129, 1129, 1131, 1132, 1130, 1130, 1134, 1133, 1122, 1129, 1129, 1129, 1129, 1129],
        [1129, 1129, 1129, 1133, 1139, 1142, 1160, 1205, 1209, 1153, 1126, 1128, 1129, 1129, 1129],
        [1129, 1129, 1129, 1131, 1138, 1140, 1162, 1238, 1281, 1228, 1141, 1126, 1129, 1129, 1129],
        [1129, 1129, 1129, 1128, 1129, 1123, 1126, 1194, 1294, 1307, 1186, 1127, 1127, 1130, 1129],
        [1129, 1129, 1129, 1128, 1130, 1125, 1123, 1182, 1299, 1357, 1237, 1137, 1127, 1130, 1129],
        [1129, 1129, 1129, 1129, 1130, 1126, 1146, 1230, 1323, 1385, 1306, 1155, 1125, 1131, 1130],
        [1129, 1129, 1129, 1130, 1129, 1124, 1164, 1283, 1376, 1417, 1378, 1215, 1127, 1123, 1130],
        [1129, 1129, 1129, 1130, 1130, 1125, 1154, 1269, 1399, 1434, 1419, 1311, 1162, 1121, 1129],
        [1129, 1129, 1129, 1129, 1130, 1127, 1134, 1201, 1322, 1390, 1413, 1380, 1258, 1155, 1127],
        [1129, 1129, 1129, 1129, 1129, 1128, 1125, 1148, 1210, 1251, 1285, 1294, 1257, 1202, 1155],
        [1129, 1129, 1129, 1129, 1129, 1129, 1129, 1134, 1141, 1135, 1152, 1162, 1174, 1194, 1169],
        [1129, 1129, 1129, 1129, 1129, 1129, 1129, 1128, 1127, 1129, 1132, 1134, 1130, 1133, 1138],
        [1129, 1129, 1129, 1129, 1129, 1129, 1129, 1129, 1128, 1130, 1128, 1127, 1125, 1123, 1126],
        [1129, 1129, 1129, 1129, 1129, 1129, 1129, 1129, 1130, 1130, 1130, 1130, 1131, 1131, 1128],
    ];

    /// 11/329/731 from pixel 206,156 (45.63 N, 122.02 W): Beacon Rock on the Columbia at z11.
    const BEACON_ROCK_Z11: [[i16; 15]; 15] = [
        [119, 102, 89, 106, 125, 143, 152, 148, 133, 117, 101, 77, 68, 57, 38],
        [113, 90, 87, 108, 131, 143, 139, 129, 116, 94, 85, 72, 65, 57, 36],
        [108, 84, 89, 110, 137, 136, 118, 100, 96, 79, 69, 58, 53, 47, 32],
        [97, 78, 86, 112, 125, 119, 100, 85, 70, 60, 56, 42, 41, 38, 28],
        [83, 74, 78, 102, 107, 95, 76, 60, 65, 46, 40, 35, 43, 37, 26],
        [72, 70, 71, 94, 94, 74, 87, 102, 32, 41, 38, 39, 46, 31, 17],
        [66, 68, 70, 81, 77, 79, 107, 234, 117, 31, 20, 26, 30, 17, 8],
        [65, 66, 68, 77, 76, 88, 115, 244, 201, 115, 35, 10, 14, 8, 9],
        [65, 65, 66, 79, 88, 89, 99, 185, 229, 199, 72, 7, 9, 10, 8],
        [64, 63, 65, 71, 87, 86, 80, 103, 182, 128, 27, 9, 10, 7, 6],
        [63, 62, 65, 68, 78, 79, 63, 53, 60, 25, 6, 9, 6, 4, 5],
        [62, 62, 64, 69, 76, 71, 51, 35, 15, 9, 11, 5, 4, 4, 4],
        [62, 62, 64, 71, 75, 64, 45, 29, 28, 14, 4, 4, 4, 4, 4],
        [62, 64, 64, 69, 65, 51, 40, 34, 25, 5, 4, 4, 4, 4, 4],
        [57, 63, 61, 58, 49, 43, 44, 39, 14, 3, 5, 4, 4, 4, 4],
    ];

    /// 12/3665/1726 from pixel 181,155 (27.18 N, 142.16 E): on Ototojima's smooth slope at z12, a
    /// needle of 290 m and towers to 591 m ringing with pits to −2,607 m.
    const OTOTOJIMA_Z12: [[i16; 15]; 15] = [
        [0, 0, 0, 0, 2, 6, 11, 19, 30, 43, 58, 91, 98, 104, 112],
        [0, 0, 0, 1, 3, 7, 13, 22, 33, 46, 62, 82, 89, 104, 117],
        [-1, -1, 0, 1, 4, 8, 15, 24, 36, 50, 67, 82, 88, 110, 126],
        [-1, -1, 0, 1, 5, 10, 17, 27, 39, 54, 72, 93, 98, 120, 138],
        [-2, -2, -1, 2, 5, 11, 19, 29, 43, 591, -125, 109, 114, 131, 150],
        [-3, -2, -1, 2, 6, 12, 21, -2607, 502, 412, -39, 118, 125, 138, 157],
        [-3, -3, -1, 2, 7, 13, 22, -75, 98, 104, 95, 120, 129, 140, 157],
        [-3, -3, -1, 2, 7, 14, 23, 290, 20, 52, 114, 120, 130, 140, 154],
        [-3, -3, -1, 2, 7, 14, 23, -18, 35, 72, 96, 116, 130, 143, 158],
        [-3, -2, -1, 2, 7, 14, 11, 19, 25, 59, 88, 109, 128, 149, 168],
        [-2, -2, 0, 2, 7, 13, -42, -21, 30, 56, 77, 102, 124, 147, 171],
        [-2, -2, 0, 2, 6, 12, -7, 6, 23, 47, 71, 96, 116, 139, 163],
        [-2, -1, 0, 2, 5, 11, -2, 14, 14, 42, 67, 86, 105, 129, 155],
        [-2, -1, 0, 1, 4, 9, -3, 16, 13, 41, 62, 79, 101, 129, 154],
        [-1, -1, -1, 1, 3, 8, 14, 8, 24, 43, 57, 77, 112, 144, 160],
    ];

    #[test]
    fn keeps_buttes_and_plugs_and_takes_a_needle_on_a_slope() {
        // Gunsight Butte stands 300 m out of Lake Powell and Beacon Rock 240 m over the Columbia:
        // walled, on flat ground, beside nothing, and as AWS draws them less steep than 63° over
        // their width. Kept.
        for (win, ground, lat) in [(&GUNSIGHT_BUTTE_Z11, 1129.0, 37.05), (&BEACON_ROCK_Z11, 60.0, 45.63)] {
            let mut t = aws(win, ground);
            let before = t.clone();
            repaired(&mut t, 11, lat);
            assert_eq!(t, before);
        }
        // On Ototojima's slope, the needle of 290 m (76° and more over its width) and the towers
        // ringing beside their pits go.
        let mut o = aws(&OTOTOJIMA_Z12, 0.0);
        repaired(&mut o, 12, 27.18);
        for (i, j) in [(7, 7), (9, 4), (8, 5), (9, 5)] {
            let now = at(&o, i, j);
            assert!(now < 150.0, "{} at {i},{j}: {now}", OTOTOJIMA_Z12[j][i]);
        }
    }

    #[test]
    fn keeps_a_summit_drawn_too_sharp_among_rough_ground() {
        // As AWS has a 3,534 m peak of the St. Elias (12/447/1143): a cone of 1,200 m, ten pixels
        // across its base (81°: broken in shape), among mountains whose ground is rough: kept, as
        // its summit is there.
        let base = |x: f32, y: f32| 2100.0 + 250.0 * (x * 0.16).sin() * (y * 0.13).cos() + 100.0 * (x * 0.5 + y * 0.3).sin();
        let mut c = tile(|x, y| base(x, y) + 1200.0 * (1.0 - ((x - 120.0).powi(2) + (y - 130.0).powi(2)).sqrt() / 10.0).max(0.0));
        let before = c.clone();
        repaired(&mut c, 12, 61.96);
        assert_eq!(c, before);
    }

    #[test]
    fn leaves_lagoons_and_holes_at_sea_level() {
        // An island's rim 10–51 m high around a hole down to AWS's filler (1 m) and the sea floor
        // (−181 m), in a sea 10 m up, as AWS has Hans Island at z10 (80.8 N, 24 m pixels): a tile
        // alone can't tell it from a lagoon, so it stays.
        let mut h = tile(|x, y| {
            let r = ((x - 128.0).powi(2) / 1.4 + (y - 128.0).powi(2)).sqrt();
            if r < 18.0 {
                if x < 132.0 { 1.0 } else { -181.0 + 0.4 * (x - 132.0) }
            } else if r < 24.0 {
                51.0 - 6.8 * (r - 18.0)
            } else {
                10.0
            }
        });
        let before = h.clone();
        repaired(&mut h, 10, 80.83);
        assert_eq!(h, before);
    }

    #[test]
    fn rough_ground_is_left_alone_and_its_towers_go() {
        for z in [7u8, 9, 10, 12] {
            let mut h = hills(800.0);
            let before = h.clone();
            let r = repaired(&mut h, z, 46.0);
            assert!(!r.changed() && r.blobs == 0, "z{z}: {r:?}");
            assert_eq!(h, before);
        }
        // towers and pits of a few pixels on those hills (z11): gone, the hills around kept
        let mut h = hills(800.0);
        let clean = h.clone();
        let mut bad = Vec::new();
        for (k, (x, y)) in [(30usize, 40usize), (200, 30), (120, 220), (250, 128), (0, 0)].into_iter().enumerate() {
            for (dx, dy) in [(0, 0), (1, 0), (0, 1)] {
                let (xx, yy) = ((x + dx).min(TS - 1), (y + dy).min(TS - 1));
                h[yy * TS + xx] += if k % 2 == 0 { 1800.0 } else { -1500.0 };
                bad.push(yy * TS + xx);
            }
        }
        repaired(&mut h, 11, 46.0);
        for (i, (&v, &c)) in h.iter().zip(&clean).enumerate() {
            if bad.contains(&i) {
                assert!((v - c).abs() < 60.0, "{i}: {v} vs {c}");
            } else {
                assert_eq!(v, c, "{i}");
            }
        }
    }
}

#[cfg(test)]
mod repair_debug {
    use super::*;
    /// SCENIC_BENCH=<folder of z-x-y.raw.f32 tiles> (`terrain --scan`'s views): the repair's time a
    /// tile, over them.
    #[test]
    #[ignore]
    fn bench() {
        let Ok(dir) = std::env::var("SCENIC_BENCH") else { return };
        let mut tiles = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".raw.f32") else { continue };
            let k: Vec<u32> = stem.split('-').filter_map(|v| v.parse().ok()).collect();
            let b = std::fs::read(e.path()).unwrap();
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (k[2] as f64 + 0.5) / (1u64 << k[0]) as f64)).sinh().atan().to_degrees();
            tiles.push((k[0] as u8, lat, bytemuck::cast_slice::<u8, f32>(&b).to_vec()));
        }
        let t0 = std::time::Instant::now();
        let mut stages = 0;
        for (z, lat, t) in &tiles {
            let mut t = t.clone();
            stages += repair_terrain(&mut t, *z, *lat).stages;
        }
        let per = t0.elapsed().as_secs_f64() * 1e3 / tiles.len() as f64;
        eprintln!("{} tiles, {per:.2} ms a tile, {stages} stages", tiles.len());
    }

    /// SCENIC_BATCH="<folder in> <folder out>": every `z-x-y.raw.f32` of the folder (`terrain
    /// --scan`'s views) repaired, written as `z-x-y.v2.f32` at sea level and over, as the packs
    /// store it (for trying a change of the rules on many tiles).
    #[test]
    #[ignore]
    fn batch() {
        let Ok(spec) = std::env::var("SCENIC_BATCH") else { return };
        let a: Vec<&str> = spec.split_whitespace().collect();
        std::fs::create_dir_all(a[1]).unwrap();
        for e in std::fs::read_dir(a[0]).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".raw.f32") else { continue };
            let k: Vec<u32> = stem.split('-').filter_map(|v| v.parse().ok()).collect();
            let b = std::fs::read(e.path()).unwrap();
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (k[2] as f64 + 0.5) / (1u64 << k[0]) as f64)).sinh().atan().to_degrees();
            let mut t: Vec<f32> = bytemuck::cast_slice(&b).to_vec();
            let r = repair_terrain(&mut t, k[0] as u8, lat);
            for v in t.iter_mut() {
                *v = v.max(0.0);
            }
            std::fs::write(format!("{}/{stem}.v2.f32", a[1]), bytemuck::cast_slice(&t)).unwrap();
            // (Its stages, and what a second pass on it as stored changes.)
            let mut again = t.clone();
            let r2 = repair_terrain(&mut again, k[0] as u8, lat);
            let moved = t.iter().zip(&again).filter(|(a, b)| (a.max(0.0) - b.max(0.0)).abs() > 0.5).count();
            let most = t.iter().zip(&again).map(|(a, b)| (a.max(0.0) - b.max(0.0)).abs()).fold(0f32, f32::max);
            if r.stages >= 12 || moved > 0 {
                eprintln!("{stem}: {} stages, again {} px (most {most:.0} m, {} blobs)", r.stages, moved, r2.blobs);
            }
        }
    }

    /// SCENIC_STAGES="<raw .f32 file> <z> <lat>": the blobs each stage found in that tile.
    #[test]
    #[ignore]
    fn stages() {
        let Ok(spec) = std::env::var("SCENIC_STAGES") else { return };
        let a: Vec<&str> = spec.split_whitespace().collect();
        let b = std::fs::read(a[0]).unwrap();
        let mut t: Vec<f32> = bytemuck::cast_slice(&b).to_vec();
        let (z, lat): (u8, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
        let (r, blobs) = repair_terrain_blobs(&mut t, z, lat);
        eprintln!("{r:?}");
        for b in &blobs {
            eprintln!("  stage {} {:?}{} top {},{} px {} rise {:.0} level {:.0} reach {:.0} ground {:.0} rough {:.1}", b.stage, b.kind, if b.pit { " pit" } else { "" }, b.top % 256, b.top / 256, b.pixels, b.rise, b.level, b.reach, b.ground, b.rough);
        }
        // (Again on the tile as stored: at sea level and over.)
        let mut again: Vec<f32> = t.iter().map(|v| v.max(0.0)).collect();
        let (r2, b2) = repair_terrain_blobs(&mut again, z, lat);
        eprintln!("again: {r2:?}");
        for b in &b2 {
            eprintln!("  stage {} {:?}{} top {},{} px {} rise {:.0} level {:.0} reach {:.0} ground {:.0} rough {:.1}", b.stage, b.kind, if b.pit { " pit" } else { "" }, b.top % 256, b.top / 256, b.pixels, b.rise, b.level, b.reach, b.ground, b.rough);
        }
    }

    /// SCENIC_TRACE="<raw .f32 file> <z> <lat> <x> <y>": the blobs at pixel (x, y) of that tile,
    /// each stage's weighing printed.
    #[test]
    #[ignore]
    fn trace() {
        let Ok(spec) = std::env::var("SCENIC_TRACE") else { return };
        let a: Vec<&str> = spec.split_whitespace().collect();
        let b = std::fs::read(a[0]).unwrap();
        let mut t: Vec<f32> = bytemuck::cast_slice(&b).to_vec();
        let (z, lat, x, y): (u8, f64, u32, u32) = (a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
        TRACE.store(y * 256 + x, std::sync::atomic::Ordering::Relaxed);
        let (r, _) = repair_terrain_blobs(&mut t, z, lat);
        eprintln!("{r:?}; ({x},{y}) now {}", t[(y * 256 + x) as usize]);
    }
}
