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
/// before filled in).
const BLOB_STAGES: usize = 8;
/// The most pixels a spike may have (BlobKind::Spike): an islet larger than that stays.
pub const SPIKE_MAX: u32 = 16;
/// How many times the roughness of the ground around it (the middle half's spread, from two pixels
/// out to twice its radius) a blob must stand out of that ground's median to be on flat ground: an
/// artifact towers over water or lowland (26 to 1,200 times in AWS's tiles), while a summit AWS
/// drew too sharp stands among rough ground (2 to 12 times: a 3,534 m peak in the St. Elias as an
/// 81° cone, 1,204 m tall), and stays. A pit down to sea level in raised ground needs no such
/// margin: it's AWS's filler where its source had none.
pub const ROUGH: f64 = 20.0;
/// The same for a spike (BlobKind::Spike), small and walled: 10.
pub const ROUGH_SPIKE: f64 = 10.0;

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
    /// A small blob steeper than 45° over its width and walled so on a quarter of its edge at
    /// least, on flat ground or beside a blob taken or a void: a spike over water or lowland, or a
    /// lobe of an artifact's ringing (a resampling's overshoot beside an edge of AWS's source).
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
}

/// Repairs a 256 × 256 terrain tile in place, in one pass: what is broken or undefined is filled
/// from the clean ground around it, and nothing else changes.
/// - Voids: values above MAX_ELEV or below MIN_ELEV, or NaN.
/// - Towers and pits: blobs, each a component of the tile's level sets (the pixels above a level,
///   connected, around a peak; below it, around a pit), so a cluster is judged whole, against the
///   level at which it meets the rest, whatever its size, and can't hide behind its own pixels.
///   One that stands out more than BLOB_RISE as the map shows it is broken (BlobKind) when it's
///   steeper over its footprint than terrain can be (`steepest`, taken where it does most, the
///   flanks of a smooth bump with it) and towers over the ground around it (ROUGH: a summit AWS
///   drew too sharp, among rough ground, stays), or when it's a pit down to sea level in raised
///   ground (AWS's filler); a small walled one is a spike when it's on flat ground or beside a blob
///   taken; one under the sea is filled there. A summit or a ridge widens as it goes down, a cliff
///   is the edge of something larger, and an island, a sea stack or a mesa larger than BLOB_MAX
///   pixels stays whatever it is.
/// The tile is taken as AWS has it, before bathymetry goes to sea level: a pit in a lake reads as
/// deep as AWS made it (−655 m in Shumarinai's, 274 m up, where a tower of 2,740 m rings).
/// It's judged in stages, each on AWS's tile with what was found before filled in (the smoothest
/// surface through the ground around it: `fill_from_around`), until one finds nothing: so a lesser
/// tower that stood on a greater one's flank, or a lobe of its ringing, is judged on the ground
/// beneath it, and the tile returned is one it has found nothing in. Deterministic.
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
        for stage in 1..=BLOB_STAGES {
            rep.stages = stage;
            let n0 = blobs.len();
            let before = hole.clone();
            let neg: Vec<f32> = t.iter().map(|&v| -v).collect();
            for (pit, v) in [(false, &*t), (true, &neg[..])] {
                let n = blobs.len();
                broken_blobs(v, if pit { -1.0 } else { 1.0 }, &before, px, &mut hole, &mut under, &mut blobs);
                for b in &mut blobs[n..] {
                    b.pit = pit;
                    if pit {
                        b.level = -b.level;
                    }
                }
            }
            if blobs.len() == n0 {
                break;
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
fn broken_blobs(v: &[f32], sign: f32, held: &[bool], px: f64, out: &mut [bool], under: &mut [bool], blobs: &mut Vec<Blob>) {
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
    // (Each component's perimeter: its pixels' sides facing other pixels of the tile.)
    let mut perim = vec![0u32; v.len()];
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
        let (mut sides4, mut joined4) = (0u32, 0u32);
        for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
            let (xx, yy) = (x + dx, y + dy);
            if xx >= 0 && yy >= 0 && xx < w && yy < w {
                sides4 += 1;
                joined4 += (rank[(yy * w + xx) as usize] < k) as u32;
            }
        }
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
            let excess = rise - steepest(l) * l;
            // (A spike's width: twice its area over its perimeter, a line's own width.)
            let thin = (2.0 * a as f64 / perim[r as usize].max(1) as f64 + 0.5) * px;
            let edge = sides[r as usize] != 0;
            let slot = &mut best[r as usize];
            if seen > BLOB_RISE && excess > 0.0 && slot[0].is_none_or(|b| excess > b.1) {
                slot[0] = Some((BlobKind::Broken, excess, k, rise as f32, level as f32, l as f32, edge));
            }
            if seen > BLOB_RISE && a <= SPIKE_MAX && rise > thin && slot[1].is_none() {
                slot[1] = Some((BlobKind::Spike, rise - thin, k, rise as f32, level as f32, thin as f32, edge));
            }
            if s.max(sl) <= 0.0 && excess > 0.0 && slot[2].is_none_or(|b| excess > b.1) {
                slot[2] = Some((BlobKind::Unseen, excess, k, rise as f32, level as f32, l as f32, edge));
            }
        }
        if nr == 0 {
            area[p as usize] = 1;
            top[p as usize] = p;
            sides[p as usize] = side(p);
            near[p as usize] = near_px[p as usize];
            perim[p as usize] = sides4;
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
            perim[dom as usize] += perim[r as usize];
        }
        parent[p as usize] = dom;
        area[dom as usize] += 1;
        sides[dom as usize] |= side(p);
        near[dom as usize] |= near_px[p as usize];
        perim[dom as usize] = perim[dom as usize] + sides4 - 2 * joined4;
    }
    for &p in &order {
        if parent[p as usize] == p {
            for b in best[p as usize].into_iter().flatten() {
                found.push((top[p as usize], b));
            }
        }
    }
    // Each blob: its top's component as it stood at its best (the pixels ranked before then).
    let mut stamp = vec![NONE; v.len()];
    let mut stack: Vec<u32> = Vec::new();
    let mut pixels: Vec<u32> = Vec::new();
    // (A chain whose broken blob was taken is done with: its other candidates aren't weighed.)
    let mut taken_top = NONE;
    for (b, &(t0, (kind, _, at, rise, level, reach, edge))) in found.iter().enumerate() {
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
        let (median, rough) = ground(v, sign, &pixels, out, held, &mut stamp, b);
        let out_of = sign as f64 * (shown(t0 as usize) - median as f64);
        let flat = out_of > if kind == BlobKind::Spike { ROUGH_SPIKE } else { ROUGH } * rough as f64;
        let at_sea = sign < 0.0 && -v[t0 as usize] <= 1.0;
        #[cfg(test)]
        let traced = pixels.contains(&TRACE.load(std::sync::atomic::Ordering::Relaxed));
        if pixels.iter().all(|&p| out[p as usize] && !held[p as usize]) {
            continue;
        }
        #[cfg(test)]
        if traced {
            eprintln!("  {kind:?} top {},{} px {} rise {rise:.0} level {level:.0} reach {reach:.0} | out_of {out_of:.0} median {median:.0} rough {rough:.1} flat {flat} at_sea {at_sea}", t0 % 256, t0 / 256, pixels.len());
        }
        if kind == BlobKind::Broken && !(flat || at_sea) {
            continue;
        }
        if kind == BlobKind::Spike {
            // Walled steeper than 45° (a quarter of the steps down its edge, at least: an islet
            // comes out of the sea more gently all round), on flat ground or beside a blob taken or
            // a void.
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
                    beside |= out[q];
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
            if !(wall >= 1.0 && (flat || beside)) {
                continue;
            }
        }
        if kind == BlobKind::Broken {
            taken_top = t0;
        }
        let mut marked = 0;
        for &p in &pixels {
            if !out[p as usize] {
                out[p as usize] = true;
                marked += 1;
            }
            under[p as usize] |= kind == BlobKind::Unseen;
        }
        blobs.push(Blob { kind, top: t0, pit: false, pixels: marked, rise, level, reach, edge, ground: median, rough });
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
    /// down its edge, m per m: 1 is 45°) and how it stood out of the ground (ROUGH_SPIKE: flat).
    pub spike: bool,
    pub spike_wall: f32,
    pub spike_flat: f32,
    /// The same, its width taken as its inradius (the steps from its innermost pixel to the ground
    /// around: a line's half width) rather than twice its area over its perimeter.
    pub spike_in: bool,
    pub spike_in_wall: f32,
    pub spike_in_flat: f32,
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
    let (mut members, mut perim, mut sides) = (Vec::<u32>::new(), 0u32, 0u8);
    let (mut best_steep, mut steep_at) = (0f64, 0usize);
    let (mut spike_at, mut spike_in_at) = (None, None);
    let mut out = Weighing::default();
    while let Some(H(v, q)) = heap.pop() {
        if v > top || !v.is_finite() || members.len() as u32 > BLOB_MAX {
            break;
        }
        if !members.is_empty() {
            let a = members.len() as f64;
            let rise = (top - v) as f64;
            let seen = (top.max(0.0) - v.max(0.0)) as f64;
            let a_eff = a * (1u32 << sides.count_ones()) as f64;
            let l = ((a_eff / std::f64::consts::PI).sqrt() + 0.5) * px;
            if seen > BLOB_RISE {
                let r = rise / (steepest(l) * l);
                if r > best_steep {
                    (best_steep, steep_at) = (r, members.len());
                }
                let thin = (2.0 * a / perim.max(1) as f64 + 0.5) * px;
                if spike_at.is_none() && members.len() as u32 <= SPIKE_MAX && rise > thin {
                    spike_at = Some(members.len());
                }
                if spike_in_at.is_none() && members.len() as u32 <= SPIKE_MAX && rise > inradius(&members) as f64 * px {
                    spike_in_at = Some(members.len());
                }
            }
        }
        let (x, y) = ((q % TS as u32) as i32, (q / TS as u32) as i32);
        let (mut sides4, mut joined4) = (0u32, 0u32);
        for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
            let (xx, yy) = (x + dx, y + dy);
            if xx >= 0 && yy >= 0 && xx < w && yy < w {
                sides4 += 1;
                joined4 += inside[(yy * w + xx) as usize] as u32;
            }
        }
        perim = perim + sides4 - 2 * joined4;
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
    let mut flat_of = |pix: &[u32], b: u32| {
        for &q in pix {
            stamp[q as usize] = b;
        }
        let (median, rough) = ground(t, 1.0, pix, &none, &none, &mut stamp, b);
        ((shown(p as usize) - median as f64) / (rough as f64).max(0.01)) as f32
    };
    out.steep = best_steep as f32;
    if steep_at > 0 {
        out.steep_flat = flat_of(&members[..steep_at], 0);
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
        out.spike_flat = flat_of(&members[..n], 1);
        out.spike_wall = wall_of(&members[..n]);
    }
    if let Some(n) = spike_in_at {
        out.spike_in = true;
        out.spike_in_flat = flat_of(&members[..n], 2);
        out.spike_in_wall = wall_of(&members[..n]);
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
fn ground(v: &[f32], sign: f32, pixels: &[u32], out: &[bool], held: &[bool], stamp: &mut [u32], b: u32) -> (f32, f32) {
    let w = TS as i32;
    let reach = ((2.0 * (pixels.len() as f64 / std::f64::consts::PI).sqrt()).ceil() as usize).max(4);
    // (Rings outward, one Chebyshev step at a time; their pixels stamped so as not to be counted
    // twice: with `b | 1 << 31`, apart from the blob's own.)
    let ring_mark = b | 1 << 31;
    let mut front: Vec<u32> = pixels.to_vec();
    let mut vals: Vec<f32> = Vec::new();
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
                    }
                }
            }
        }
        front = next;
    }
    if vals.is_empty() {
        return (0.0, 0.0);
    }
    let n = vals.len();
    let q1 = *vals.select_nth_unstable_by(n / 4, |a, b| a.total_cmp(b)).1;
    let median = *vals.select_nth_unstable_by(n / 2, |a, b| a.total_cmp(b)).1;
    let q3 = *vals.select_nth_unstable_by(n * 3 / 4, |a, b| a.total_cmp(b)).1;
    (median, q3 - q1)
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
        let once = t.to_vec();
        let again = repair_terrain(t, z, lat);
        assert!(!again.changed() && again.blobs == 0, "a second pass found more: {again:?} ({r:?} first)");
        assert_eq!(t, &once[..]);
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
        // Odaiba at z9: the tower, and the column standing on the waterfront beside it; the rest
        // as it was (as the map shows it: at or above sea level).
        let mut t = aws(&ODAIBA_Z9, 3.0);
        repaired(&mut t, 9, 35.65);
        for (j, row) in ODAIBA_Z9.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                let now = at(&t, i, j).max(0.0);
                if v >= 225 {
                    assert!(now < 60.0, "{v} at {i},{j}: {now}");
                } else if (0..100).contains(&v) {
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
