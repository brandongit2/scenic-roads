//! The depth layers' fallback (docs/plan.md §6, Depth): depth follows each source's own detail, so
//! the deepest zoom stored varies from place to place (4 m surveys nearshore, the 15″ grid
//! offshore, estimated lakes capped at z12). A tile the layer lacks is made from its nearest stored
//! ancestor: that tile's part scaled up, bilinear in metres for depth (Terrarium PNG) and nearest
//! for classes (8-bit grey PNG), with the ancestor's neighbours' edge pixels, so tiles cut from
//! different ancestors meet without a seam. The server makes them so (crates/server/src/depth.rs),
//! and the build scales a coarser level up the same way (crate::depth::compose), so a stored tile
//! and a made one beside it agree.
//!
//! Depth tiles mark "no data" with values at or above `NODATA_MIN` (the build writes +12,000 m);
//! interpolation never mixes them into real depths.

use anyhow::{anyhow, ensure, Result};

pub use super::{NODATA, NODATA_MIN};
pub const TILE: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Depth in metres, Terrarium-encoded RGB.
    Terrarium,
    /// A class per pixel (measured, interpolated, estimated …), 8-bit grey.
    Class,
}

impl Kind {
    /// The kind of a depth layer's tiles (`super::LAYER`, `super::CLASS_LAYER`).
    pub fn of_layer(layer: &str) -> Option<Kind> {
        match layer {
            super::LAYER => Some(Kind::Terrarium),
            super::CLASS_LAYER => Some(Kind::Class),
            _ => None,
        }
    }
}

/// A decoded tile.
#[derive(Clone, Debug, PartialEq)]
pub enum Grid {
    F(Vec<f32>),
    C(Vec<u8>),
}

pub fn decode(kind: Kind, bytes: &[u8]) -> Result<Grid> {
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(png::Transformations::EXPAND);
    let mut r = d.read_info()?;
    let mut buf = vec![0u8; r.output_buffer_size().ok_or_else(|| anyhow!("PNG too large"))?];
    let info = r.next_frame(&mut buf)?;
    ensure!(
        info.width as usize == TILE && info.height as usize == TILE,
        "a {}×{} tile (want {TILE})",
        info.width,
        info.height
    );
    ensure!(info.bit_depth == png::BitDepth::Eight, "a {:?}-bit tile", info.bit_depth);
    let ch = info.color_type.samples();
    let px = &buf[..info.buffer_size()];
    Ok(match kind {
        Kind::Terrarium => {
            ensure!(ch >= 3, "a depth tile in {:?}", info.color_type);
            Grid::F(
                px.chunks_exact(ch)
                    .map(|p| f32::from(p[0]) * 256.0 + f32::from(p[1]) + f32::from(p[2]) / 256.0 - 32768.0)
                    .collect(),
            )
        }
        Kind::Class => Grid::C(px.chunks_exact(ch).map(|p| p[0]).collect()),
    })
}

pub fn encode(g: &Grid) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, TILE as u32, TILE as u32);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        match g {
            Grid::F(v) => {
                enc.set_color(png::ColorType::Rgb);
                let mut rgb = Vec::with_capacity(TILE * TILE * 3);
                for &m in v {
                    // Terrarium: (m + 32768) as R·256 + G + B/256.
                    let c = ((f64::from(m) + 32768.0) * 256.0).round().clamp(0.0, 16_777_215.0) as u32;
                    rgb.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
                }
                enc.write_header()?.write_image_data(&rgb)?;
            }
            Grid::C(v) => {
                enc.set_color(png::ColorType::Grayscale);
                enc.write_header()?.write_image_data(v)?;
            }
        }
    }
    Ok(out)
}

/// Tile z/x/y made from its ancestor `k` zooms up: `anc` is that ancestor, `nb` its neighbours at
/// the same zoom (north-west, north, north-east, west, east, south-west, south, south-east), each
/// when the archive has it (else the ancestor's own edge is repeated).
pub fn upsample(k: u8, x: u32, y: u32, anc: &Grid, nb: [Option<&Grid>; 8]) -> Grid {
    let n = 1u32 << k;
    let span = TILE as f64 / f64::from(n); // ancestor pixels across the child tile
    let (x0, y0) = (f64::from(x % n) * span, f64::from(y % n) * span);
    let step = 1.0 / f64::from(n);
    match anc {
        Grid::F(a) => {
            let ext = extended_f(a, nb);
            let w = TILE + 2;
            let at = |r: usize, c: usize| ext[r * w + c];
            let mut out = vec![0f32; TILE * TILE];
            for j in 0..TILE {
                // Sample index space: sample s sits at s + 0.5; +1 for the border.
                let py = y0 + (j as f64 + 0.5) * step - 0.5 + 1.0;
                let r0 = (py.floor() as isize).clamp(0, (w - 2) as isize) as usize;
                let ty = (py - r0 as f64).clamp(0.0, 1.0) as f32;
                for i in 0..TILE {
                    let px = x0 + (i as f64 + 0.5) * step - 0.5 + 1.0;
                    let c0 = (px.floor() as isize).clamp(0, (w - 2) as isize) as usize;
                    let tx = (px - c0 as f64).clamp(0.0, 1.0) as f32;
                    let q = [at(r0, c0), at(r0, c0 + 1), at(r0 + 1, c0), at(r0 + 1, c0 + 1)];
                    let wts = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
                    out[j * TILE + i] = blend(q, wts);
                }
            }
            Grid::F(out)
        }
        Grid::C(a) => {
            let mut out = vec![0u8; TILE * TILE];
            for j in 0..TILE {
                let r = ((y0 + (j as f64 + 0.5) * step).floor() as usize).min(TILE - 1);
                for i in 0..TILE {
                    let c = ((x0 + (i as f64 + 0.5) * step).floor() as usize).min(TILE - 1);
                    out[j * TILE + i] = a[r * TILE + c];
                }
            }
            Grid::C(out)
        }
    }
}

/// A stored tile, decoded, as `served` asks for it: None when the layer lacks it.
pub type Stored = Option<std::sync::Arc<Grid>>;

/// Tile z/x/y as the map is given it: the stored tile, else made from its nearest stored ancestor
/// (`upsample`, with that ancestor's neighbours for depth), else None (no ancestor: outside the
/// layer). With the stored tile it says so (`true`).
pub fn served(
    get: &dyn Fn(u8, u32, u32) -> Result<Stored>,
    kind: Kind,
    z: u8,
    x: u32,
    y: u32,
) -> Result<Option<(Grid, bool)>> {
    // (No such tile: past the layer's zooms, or off the world.)
    if !in_layer(z, x, y) {
        return Ok(None);
    }
    if let Some(g) = get(z, x, y)? {
        return Ok(Some(((*g).clone(), true)));
    }
    for k in 1..=z {
        let (az, ax, ay) = (z - k, x >> k, y >> k);
        let Some(anc) = get(az, ax, ay)? else { continue };
        let mut nbs: Vec<Stored> = Vec::with_capacity(8);
        let n = 1i64 << az;
        for (dx, dy) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (nx, ny) = ((i64::from(ax) + dx).rem_euclid(n), i64::from(ay) + dy);
            nbs.push(if kind == Kind::Terrarium && (0..n).contains(&ny) {
                get(az, nx as u32, ny as u32)?
            } else {
                None
            });
        }
        let nb: [Option<&Grid>; 8] = std::array::from_fn(|i| nbs[i].as_deref());
        return Ok(Some((upsample(k, x, y, &anc, nb), false)));
    }
    Ok(None)
}

/// Whether z/x/y is a tile of the depth layers' zooms (`super::ZOOMS`'s deepest and coarser).
pub fn in_layer(z: u8, x: u32, y: u32) -> bool {
    z <= super::ZOOMS.1 && u64::from(x) < 1u64 << z && u64::from(y) < 1u64 << z
}

/// Four corner values and their bilinear weights: no data where the nearest corner has none,
/// else the corners that have data, reweighted.
fn blend(q: [f32; 4], w: [f32; 4]) -> f32 {
    if q.iter().all(|&v| v < NODATA_MIN) {
        return q[0] * w[0] + q[1] * w[1] + q[2] * w[2] + q[3] * w[3];
    }
    let near = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap_or(0);
    if q[near] >= NODATA_MIN {
        return NODATA;
    }
    let (mut s, mut ws) = (0.0f32, 0.0f32);
    for i in 0..4 {
        if q[i] < NODATA_MIN {
            s += q[i] * w[i];
            ws += w[i];
        }
    }
    if ws > 0.0 {
        s / ws
    } else {
        NODATA
    }
}

/// The ancestor with a one-pixel border from its neighbours ((TILE + 2)², row-major).
fn extended_f(a: &[f32], nb: [Option<&Grid>; 8]) -> Vec<f32> {
    let w = TILE + 2;
    let mut e = vec![0f32; w * w];
    for r in 0..TILE {
        e[(r + 1) * w + 1..(r + 1) * w + 1 + TILE].copy_from_slice(&a[r * TILE..(r + 1) * TILE]);
    }
    fn f(g: Option<&Grid>) -> Option<&[f32]> {
        match g {
            Some(Grid::F(v)) => Some(v.as_slice()),
            _ => None,
        }
    }
    let last = TILE - 1;
    // Edges: the neighbour's facing row or column, else this tile's own.
    for c in 0..TILE {
        e[c + 1] = f(nb[1]).map_or(a[c], |n| n[last * TILE + c]);
        e[(w - 1) * w + c + 1] = f(nb[6]).map_or(a[last * TILE + c], |n| n[c]);
    }
    for r in 0..TILE {
        e[(r + 1) * w] = f(nb[3]).map_or(a[r * TILE], |n| n[r * TILE + last]);
        e[(r + 1) * w + w - 1] = f(nb[4]).map_or(a[r * TILE + last], |n| n[r * TILE]);
    }
    // Corners: the diagonal neighbour's, else the nearest edge value.
    e[0] = f(nb[0]).map_or(e[1], |n| n[last * TILE + last]);
    e[w - 1] = f(nb[2]).map_or(e[w - 2], |n| n[last * TILE]);
    e[(w - 1) * w] = f(nb[5]).map_or(e[(w - 1) * w + 1], |n| n[last]);
    e[w * w - 1] = f(nb[7]).map_or(e[w * w - 2], |n| n[0]);
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(f: impl Fn(usize, usize) -> f32) -> Grid {
        Grid::F((0..TILE * TILE).map(|i| f(i / TILE, i % TILE)).collect())
    }

    #[test]
    fn no_tile_past_the_layer_or_off_the_world() {
        let get = |_: u8, _: u32, _: u32| -> Result<Stored> {
            Ok(Some(std::sync::Arc::new(Grid::F(vec![-1.0; TILE * TILE]))))
        };
        assert!(served(&get, Kind::Terrarium, 16, 0, 0).unwrap().is_some());
        for (z, x, y) in [(17, 0, 0), (40, 0, 0), (3, 8, 0), (3, 0, 8), (255, u32::MAX, 0)] {
            assert!(served(&get, Kind::Terrarium, z, x, y).unwrap().is_none(), "{z}/{x}/{y}");
        }
    }

    #[test]
    fn terrarium_round_trip() {
        let g = tile(|r, c| -(r as f32) * 3.5 - c as f32 / 7.0);
        let back = decode(Kind::Terrarium, &encode(&g).unwrap()).unwrap();
        let (Grid::F(a), Grid::F(b)) = (&g, &back) else { panic!() };
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() <= 1.0 / 256.0, "{x} {y}");
        }
    }

    #[test]
    fn a_plane_stays_a_plane() {
        // A linear ramp upsampled by bilinear interpolation stays the same ramp (away from the
        // repeated edges, where there are no neighbours).
        let anc = tile(|r, c| -(r as f32) - 2.0 * c as f32);
        let Grid::F(out) = upsample(2, 5, 6, &anc, [None; 8]) else { panic!() };
        // Child (x=5 % 4 = 1, y=6 % 4 = 2): ancestor columns 64..128, rows 128..192.
        for (j, i) in [(10usize, 10usize), (100, 200), (255, 0)] {
            let ac = 64.0 + (i as f32 + 0.5) / 4.0 - 0.5;
            let ar = 128.0 + (j as f32 + 0.5) / 4.0 - 0.5;
            let want = -ar - 2.0 * ac;
            assert!((out[j * TILE + i] - want).abs() < 1e-3, "{j},{i}: {} vs {want}", out[j * TILE + i]);
        }
    }

    #[test]
    fn no_data_never_leaks_into_depths() {
        let anc = tile(|_, c| if c < 128 { -10.0 } else { NODATA });
        let Grid::F(out) = upsample(1, 0, 0, &anc, [None; 8]) else { panic!() };
        // Child x=0 covers ancestor columns 0..128: all depth, edge included.
        assert!(out.iter().all(|&v| (v + 10.0).abs() < 1e-4));
        let Grid::F(out) = upsample(1, 1, 0, &anc, [None; 8]) else { panic!() };
        // Child x=1 covers 128..256: its first column's nearest sample has no data.
        assert!(out.iter().all(|&v| v >= NODATA_MIN || (v + 10.0).abs() < 1e-4));
    }

    #[test]
    fn neighbours_fill_the_border() {
        let anc = tile(|_, _| -5.0);
        let east = tile(|_, _| -9.0);
        let Grid::F(out) = upsample(1, 1, 0, &anc, [None, None, None, None, Some(&east), None, None, None]) else {
            panic!()
        };
        // The last column leans toward the eastern neighbour's -9.
        assert!(out[TILE - 1] < -5.5, "{}", out[TILE - 1]);
        assert!((out[0] + 5.0).abs() < 1e-4);
    }
}
