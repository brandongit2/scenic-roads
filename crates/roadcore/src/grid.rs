//! Analysis rasters on the Web-Mercator z11 tile grid (256 × 256 cells per tile, ~54 m at
//! 45° N), stored only for tiles near roads. All layers share one tile list:
//!
//!   grid.idx          [u32; 2] tile x, y per slot
//!   grid.terrain.i16  elevation, metres            (terrain stage)
//!   grid.canopy.u8    canopy height, metres (p75)   (canopy stage)
//!   grid.class.u8     land cover class (`class`)    (landcover stage)
//!
//! Global cell coordinates are `tile * 256 + pixel` at zoom 11.

use anyhow::{bail, Result};
use memmap2::Mmap;
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
    40_075_016.686 * lat.to_radians().cos() / WORLD
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

/// Encode metres as a Terrarium RGB PNG (quantised to 1/256 m).
/// Above Everest: not an elevation. AWS Terrain Tiles fill some voids with 32767 m (a z9 pixel on
/// the Toyama shore, clusters of hundreds of z12 pixels along the US–Canada border).
pub const MAX_ELEV: f32 = 8900.0;
/// A single-pixel spike or pit: more than this many pixel sizes above or below all its neighbours.
pub const SPIKE_PX: f64 = 3.0;

/// Repairs a 256 × 256 terrain tile in place:
/// - impossible values (above MAX_ELEV, or NaN) filled in from their valid neighbours, ring by ring
///   inward (0 if the tile has none);
/// - spikes standing out of the ground around them, set to the median of the pixels two to three
///   pixels out (the ring): more than `rise` (max(100 m, half a pixel's size)) above the ring's
///   highest pixel and by more than twice the ring's range (a summit or a ridge has high flanks in
///   it); or, where the ring is flat (its middle half within a quarter of `rise`: water, a plain),
///   more than `rise` and twice a pixel's size (steeper than 45° out to it) above its upper
///   quartile, which also takes lines through it; a real ridge off a plain is less steep at the
///   size it's a pixel or two wide (Soffeh, 700 m above the Isfahan plain at z7). AWS's tiles over
///   Tokyo Bay have clusters at every zoom (1,767 m off Toyosu at z9, an 841 / 665 m pair off
///   Shinagawa), so that each distance showed its own towers, and a band of 5–24 km values runs
///   through the Akashi Strait. (Quartiles alone took 1,350 m off Fuji's summit at z7.)
/// - single-pixel spikes and pits clamped to their neighbours: interior pixels more than max(150 m,
///   SPIKE_PX × the pixel size) above or below all eight of them, a wall steeper than 70° all
///   round, which real terrain doesn't have at these sizes (the peaks step clamps at 1.2 × for its
///   floods; for the map, that would shave a sharp summit at z8).
/// Returns how many pixels were filled and how many clamped (spikes of either kind and pits).
pub fn repair_terrain(t: &mut [f32], z: u8, lat: f64) -> (usize, usize) {
    let w = TS;
    let bad = |v: f32| !(v <= MAX_ELEV);
    let mut filled = 0;
    if t.iter().any(|&v| bad(v)) {
        if t.iter().all(|&v| bad(v)) {
            filled = t.len();
            t.fill(0.0);
        } else {
            loop {
                let src = t.to_vec();
                let mut left = 0;
                for y in 0..w {
                    for x in 0..w {
                        if !bad(src[y * w + x]) {
                            continue;
                        }
                        let (mut sum, mut n) = (0f32, 0);
                        for dy in -1i32..=1 {
                            for dx in -1i32..=1 {
                                let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                                if (dx, dy) == (0, 0) || xx < 0 || yy < 0 || xx >= w as i32 || yy >= w as i32 {
                                    continue;
                                }
                                let v = src[yy as usize * w + xx as usize];
                                if !bad(v) {
                                    sum += v;
                                    n += 1;
                                }
                            }
                        }
                        if n > 0 {
                            t[y * w + x] = sum / n as f32;
                            filled += 1;
                        } else {
                            left += 1;
                        }
                    }
                }
                if left == 0 {
                    break;
                }
            }
        }
    }
    let px_m = 40_075_016.7 * lat.to_radians().cos() / ((1u64 << z) as f64 * w as f64);
    let mut clamped = 0;
    // Spikes over flatter ground (the ring: pixels two to three out).
    let rise = (0.5 * px_m).max(100.0) as f32;
    let src = t.to_vec();
    let mut ring: Vec<f32> = Vec::with_capacity(40);
    for y in 0..w {
        for x in 0..w {
            let v = src[y * w + x];
            // (cheap first: some pixel beside it that much lower)
            let mut low = f32::MAX;
            for (dx, dy) in [(-1i32, -1i32), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                if xx >= 0 && yy >= 0 && xx < w as i32 && yy < w as i32 {
                    low = low.min(src[yy as usize * w + xx as usize]);
                }
            }
            if !(v > low + rise) {
                continue;
            }
            ring.clear();
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    if dx.abs().max(dy.abs()) < 2 {
                        continue;
                    }
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx >= 0 && yy >= 0 && xx < w as i32 && yy < w as i32 {
                        ring.push(src[yy as usize * w + xx as usize]);
                    }
                }
            }
            if ring.len() < 12 {
                continue;
            }
            ring.sort_unstable_by(|a, b| a.total_cmp(b));
            let n = ring.len();
            let (min, max, q1, q3) = (ring[0], ring[n - 1], ring[n / 4], ring[n * 3 / 4]);
            if v > max + rise.max(2.0 * (max - min)) || (q3 - q1 <= 0.25 * rise && v > q3 + rise.max(2.0 * px_m as f32)) {
                t[y * w + x] = ring[n / 2];
                clamped += 1;
            }
        }
    }
    // Single-pixel spikes and pits.
    let thr = (SPIKE_PX * px_m).max(150.0) as f32;
    let src = t.to_vec();
    for y in 1..w - 1 {
        for x in 1..w - 1 {
            let v = src[y * w + x];
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for (dx, dy) in [(-1i32, -1i32), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let n = src[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize];
                lo = lo.min(n);
                hi = hi.max(n);
            }
            if v > hi + thr {
                t[y * w + x] = hi;
                clamped += 1;
            } else if v < lo - thr {
                t[y * w + x] = lo;
                clamped += 1;
            }
        }
    }
    (filled, clamped)
}

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
        let n = 1u32 << dz;
        let (ox, oy) = ((x - (px << dz)) as f64 * 256.0 / n as f64, (y - (py << dz)) as f64 * 256.0 / n as f64);
        let s = 1.0 / n as f64;
        let mut out = vec![0f32; 256 * 256];
        for j in 0..256 {
            for i in 0..256 {
                out[j * 256 + i] = bilinear(&p, 256, ox + (i as f64 + 0.5) * s - 0.5, oy + (j as f64 + 0.5) * s - 0.5);
            }
        }
        return Some(out);
    }
    None
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

    fn slope_tile() -> Vec<f32> {
        (0..TS * TS).map(|i| (i % TS) as f32 * 2.0 + 100.0).collect()
    }

    #[test]
    fn fills_voids_from_their_surroundings() {
        let mut t = slope_tile();
        for y in 100..120 {
            for x in 100..120 {
                t[y * TS + x] = 32767.0;
            }
        }
        let (filled, _) = repair_terrain(&mut t, 12, 45.0);
        assert_eq!(filled, 400);
        // a plane, filled from its edges: close to the plane inside
        for y in 100..120 {
            for x in 100..120 {
                let want = x as f32 * 2.0 + 100.0;
                assert!((t[y * TS + x] - want).abs() < 25.0, "{} vs {want}", t[y * TS + x]);
            }
        }
    }

    #[test]
    fn flattens_spike_clusters_over_flat_ground_and_keeps_ridges() {
        // a 2-pixel tower in a bay (z9, as off Shinagawa)
        let mut t = vec![0f32; TS * TS];
        t[100 * TS + 100] = 841.0;
        t[100 * TS + 101] = 665.0;
        let (_, c) = repair_terrain(&mut t, 9, 35.6);
        assert_eq!(c, 2);
        assert!(t[100 * TS + 100] < 1.0 && t[100 * TS + 101] < 1.0);
        // a sharp ridge 300 m above its valleys at z12: kept
        let mut r: Vec<f32> = (0..TS * TS).map(|i| { let x = (i % TS) as f32; 1000.0 + 300.0 - (x - 128.0).abs() * 60.0 }).map(|v| v.max(1000.0)).collect();
        let before = r.clone();
        repair_terrain(&mut r, 12, 45.0);
        assert_eq!(r, before);
    }

    #[test]
    fn clamps_lone_spikes_and_keeps_summits() {
        let mut t = vec![500f32; TS * TS];
        t[50 * TS + 50] = 2500.0; // a 2 km needle one z12 pixel wide
        // a real summit at z8 (~430 m pixels at 45°), a cone 300 m higher a pixel in: kept
        let mut s: Vec<f32> = (0..TS * TS).map(|i| { let (x, y) = ((i % TS) as i32 - 80, (i / TS) as i32 - 80); 3600.0 - 300.0 * x.abs().max(y.abs()) as f32 }).map(|v| v.max(0.0)).collect();
        let (_, c) = repair_terrain(&mut t, 12, 45.0);
        assert_eq!(c, 1);
        assert_eq!(t[50 * TS + 50], 500.0);
        let (_, c) = repair_terrain(&mut s, 8, 45.0);
        assert_eq!(c, 0);
        assert_eq!(s[80 * TS + 80], 3600.0);
    }
}
