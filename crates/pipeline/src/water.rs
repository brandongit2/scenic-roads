//! The water layer (docs/plan.md §6 "Water"; docs/formats.md "Water"): each pixel's exact share
//! of water, sea and inland apart, at every zoom, from the basemap's own water at its deepest zoom.
//!
//! The basemap (Planetiler's OpenMapTiles profile) simplifies its water and leaves small polygons
//! out zoomed out, and below z6 has only Natural Earth's. Its z14 tiles have everything (simplified
//! by 0.0625 px of a 256-px z14 tile, one unit of its 4,096, about 0.6 m at the equator; nothing
//! over 1/256 px² left out). Here each
//! output pixel gets the area of that water inside it over its own (watercov::Raster: exact,
//! however small the water), so a tile of any zoom shows the shore exactly as the full detail
//! would, anti-aliased, and a district of ponds too small to draw one by one reads by how much of
//! it is water. The map draws it as a raster at the screen's density (512 px for 256 CSS px), so
//! tilting and 3D terrain treat it as any raster.
//!
//! - A tile is `SIZE` px a side, two channels: the sea's share (OpenMapTiles' class `ocean`, from
//!   the water polygons) and the inland water's (lakes, reservoirs, rivers' and canals' areas, the
//!   rest of the basemap's `water` layer, water in tunnels left out as the map leaves it out).
//! - Zooms to `STORED_MAXZ` are made once (`build`): every z`DRAW_Z` tile drawn from its 256 z14
//!   tiles, each coarser tile the mean of its four children's pixels (exact, as a pixel is the
//!   mean of the four under it), stored as PNG (grey the sea, alpha the inland water) where not
//!   one value throughout. A tile not stored is uniform: its ancestor's pixels over it say which.
//! - Deeper zooms are made when asked (`cover`: the server), from the z14 tiles under the tile or
//!   over it.

use crate::watercov::Raster;
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The layer (`layers/water/…`, served at `/tiles/water`).
pub const LAYER: &str = "water";
/// The layer's version: a change to what it holds or how it's drawn makes the agent build it
/// again.
pub const VERSION: u32 = 1;
/// A tile's pixels a side.
pub const SIZE: usize = 512;
/// The basemap's deepest zoom, whose water is drawn.
pub const BASE_Z: u8 = 14;
/// The zoom `build` draws, averaging down from it.
pub const DRAW_Z: u8 = 10;
/// The deepest zoom stored; deeper ones are made when asked.
pub const STORED_MAXZ: u8 = 9;
/// The deepest zoom served (deeper, the map overzooms it): 0.3 m a pixel, past the basemap's
/// detail.
pub const MAXZ: u8 = 18;
/// The Planetiler whose z14 water this takes as full detail (simplified by 0.0625 px of a 256-px z14
/// tile, one unit of its 4,096, about 0.6 m at the equator; nothing over 1/256 px² left out), checked against the shoreline check's reference
/// (tools/coastcheck: 0.0002–0.002 mean difference at z14, 2026-10-08). The pass stops on another
/// one's jar (pipeline::osmpass::check_planetiler) until it's checked again.
pub const PLANETILER_VERSION: &str = "0.10.2";

// ---- a tile's coverage -----------------------------------------------------------------------------

/// A tile's coverage: per pixel, row by row, the sea's share and the inland water's (0–1).
#[derive(Clone, Debug, PartialEq)]
pub struct Cov {
    pub sea: Vec<f32>,
    pub inland: Vec<f32>,
}

/// Coverage as stored: one share each throughout, or per pixel.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Uniform(u8, u8),
    Cov(Cov),
}

/// A share as a byte.
pub fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl Cov {
    pub fn uniform(sea: f32, inland: f32) -> Cov {
        Cov { sea: vec![sea; SIZE * SIZE], inland: vec![inland; SIZE * SIZE] }
    }

    /// Its bytes' one value each, if it has one.
    pub fn uniform_bytes(&self) -> Option<(u8, u8)> {
        let (s, i) = (byte(self.sea[0]), byte(self.inland[0]));
        (self.sea.iter().all(|&v| byte(v) == s) && self.inland.iter().all(|&v| byte(v) == i)).then_some((s, i))
    }

    /// The tile whose four quarters are `kids` (top left, top right, bottom left, bottom right):
    /// each pixel the mean of the four under it.
    pub fn average(kids: [&Node; 4]) -> Cov {
        let h = SIZE / 2;
        let mut out = Cov::uniform(0.0, 0.0);
        for (k, kid) in kids.iter().enumerate() {
            let (qx, qy) = ((k % 2) * h, (k / 2) * h);
            match kid {
                Node::Uniform(s, i) => {
                    let (s, i) = (f32::from(*s) / 255.0, f32::from(*i) / 255.0);
                    for y in 0..h {
                        let o = (qy + y) * SIZE + qx;
                        out.sea[o..o + h].fill(s);
                        out.inland[o..o + h].fill(i);
                    }
                }
                Node::Cov(c) => {
                    for y in 0..h {
                        for x in 0..h {
                            let i0 = 2 * y * SIZE + 2 * x;
                            let m = |v: &[f32]| (v[i0] + v[i0 + 1] + v[i0 + SIZE] + v[i0 + SIZE + 1]) * 0.25;
                            let o = (qy + y) * SIZE + qx + x;
                            out.sea[o] = m(&c.sea);
                            out.inland[o] = m(&c.inland);
                        }
                    }
                }
            }
        }
        out
    }

    /// As stored: an 8-bit grey-and-alpha PNG, grey the sea's share, alpha the inland water's.
    pub fn png(&self) -> Vec<u8> {
        let px: Vec<u8> = self.sea.iter().zip(&self.inland).flat_map(|(&s, &i)| [byte(s), byte(i)]).collect();
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, SIZE as u32, SIZE as u32);
        e.set_color(png::ColorType::GrayscaleAlpha);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::High);
        let mut w = e.write_header().expect("png header");
        w.write_image_data(&px).expect("png data");
        w.finish().expect("png end");
        out
    }

    /// A stored tile read back.
    pub fn from_png(b: &[u8]) -> Result<Cov> {
        let mut d = png::Decoder::new(std::io::Cursor::new(b)).read_info().context("a water tile")?;
        let mut px = vec![0u8; d.output_buffer_size().context("a water tile's size")?];
        let info = d.next_frame(&mut px).context("a water tile")?;
        if info.width as usize != SIZE || info.height as usize != SIZE || info.color_type != png::ColorType::GrayscaleAlpha || info.bit_depth != png::BitDepth::Eight {
            bail!("a water tile of another kind ({}×{} {:?} {:?})", info.width, info.height, info.color_type, info.bit_depth);
        }
        let (mut sea, mut inland) = (Vec::with_capacity(SIZE * SIZE), Vec::with_capacity(SIZE * SIZE));
        for p in px[..SIZE * SIZE * 2].as_chunks::<2>().0 {
            sea.push(f32::from(p[0]) / 255.0);
            inland.push(f32::from(p[1]) / 255.0);
        }
        Ok(Cov { sea, inland })
    }

    /// The pixels `ox`, `oy` (and `side` a side) of this tile, scaled up to a whole tile (nearest:
    /// for a uniform region of a stored ancestor, all one value).
    pub fn value_at(&self, x: usize, y: usize) -> (u8, u8) {
        let i = y.min(SIZE - 1) * SIZE + x.min(SIZE - 1);
        (byte(self.sea[i]), byte(self.inland[i]))
    }
}

// ---- the basemap's water ----------------------------------------------------------------------------

/// A basemap tile's water polygons: their rings (tile units, as the tile winds them: outer rings
/// clockwise on the map, holes anticlockwise) and whether each is the sea's.
#[derive(Clone, Debug, Default)]
pub struct TileWater {
    pub extent: f64,
    pub rings: Vec<(bool, Vec<[f64; 2]>)>,
}

fn varint(b: &[u8], i: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for s in (0..64).step_by(7) {
        let Some(&c) = b.get(*i) else { bail!("a varint cut short") };
        *i += 1;
        v |= u64::from(c & 0x7f) << s;
        if c & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("a varint over 64 bits")
}

/// The fields of a protobuf message: (number, wire type, the bytes of a length-delimited one).
fn fields(b: &[u8]) -> Result<Vec<(u64, u64, &[u8])>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let (num, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                varint(b, &mut i)?;
                out.push((num, wire, &b[0..0]));
            }
            1 => i += 8,
            5 => i += 4,
            2 => {
                let n = usize::try_from(varint(b, &mut i)?)?;
                let s = b.get(i..i + n).context("a field cut short")?;
                out.push((num, wire, s));
                i += n;
            }
            _ => bail!("wire type {wire}"),
        }
    }
    Ok(out)
}

/// A basemap tile's water, as stored (gzip'd or not): its `water` layer alone is decoded.
pub fn tile_water(stored: &[u8]) -> Result<TileWater> {
    let raw = names::mvt::gunzip_if_gzip(stored)?;
    for (num, wire, layer) in fields(&raw)? {
        if num != 3 || wire != 2 {
            continue;
        }
        let lf = fields(layer)?;
        if !lf.iter().any(|&(n, w, s)| n == 1 && w == 2 && s == b"water") {
            continue;
        }
        let l = names::mvt::Layer::decode(layer)?;
        let key = |name: &str| l.keys.iter().position(|k| k == name).map(|k| k as u32);
        let (class, brunnel) = (key("class"), key("brunnel"));
        let value_is = |f: &names::mvt::Feature, k: Option<u32>, v: &str| {
            k.is_some_and(|k| f.tags.as_chunks::<2>().0.iter().any(|kv| kv[0] == k && l.values.get(kv[1] as usize).and_then(names::mvt::Value::as_str) == Some(v)))
        };
        let mut out = TileWater { extent: f64::from(l.extent), rings: Vec::new() };
        for f in &l.features {
            // (Water in a tunnel the map doesn't draw.)
            if f.geom_type != Some(3) || value_is(f, brunnel, "tunnel") {
                continue;
            }
            let sea = value_is(f, class, "ocean");
            for r in rings(&f.geometry) {
                out.rings.push((sea, r));
            }
        }
        return Ok(out);
    }
    Ok(TileWater::default())
}

/// An MVT polygon geometry's rings (tile units).
fn rings(g: &[u32]) -> Vec<Vec<[f64; 2]>> {
    let mut out = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    let (mut x, mut y, mut i) = (0i64, 0i64, 0usize);
    let zz = |v: u32| i64::from(v >> 1) ^ -i64::from(v & 1);
    while i < g.len() {
        let (cmd, n) = (g[i] & 7, g[i] >> 3);
        i += 1;
        match cmd {
            1 | 2 => {
                for _ in 0..n {
                    if i + 1 >= g.len() {
                        break;
                    }
                    x += zz(g[i]);
                    y += zz(g[i + 1]);
                    i += 2;
                    if cmd == 1 && !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push([x as f64, y as f64]);
                }
            }
            7 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => break,
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Draws a tile's water into the rasters, the tile's square at `ox`, `oy` (pixels), `side` a
/// side. The rasters must lie within the square: the tile holds the water only there (and a
/// buffer round it, which counts only as what it is).
fn draw(w: &TileWater, sea: &mut Raster, inland: &mut Raster, ox: f64, oy: f64, side: f64) {
    if w.rings.is_empty() {
        return;
    }
    let k = side / w.extent;
    for (is_sea, r) in &w.rings {
        let ras = if *is_sea { &mut *sea } else { &mut *inland };
        let px = |p: [f64; 2]| [ox + p[0] * k, oy + p[1] * k];
        let mut prev = px(r[r.len() - 1]);
        for &p in r {
            let q = px(p);
            ras.edge(prev, q);
            prev = q;
        }
    }
}

// ---- drawing a tile ---------------------------------------------------------------------------------

/// A basemap tile as stored, with a key for its bytes: the same key only for the same bytes (the
/// open sea's tile, shared by thousands), across archives too (a key names its archive: it outlives
/// none of them in `Blocks`).
pub struct Stored {
    pub key: u64,
    pub bytes: Arc<Vec<u8>>,
}

/// The z14 tiles' water drawn at some size, where it's one value throughout (the open sea's tile,
/// shared by thousands): by (key, size). At most `BLOCKS_KEPT`: past that, emptied (the few that
/// matter come back at once).
#[derive(Default)]
pub struct Blocks(Mutex<HashMap<(u64, usize), (f32, f32)>>);

/// The most `Blocks` keeps.
pub const BLOCKS_KEPT: usize = 4096;

impl Blocks {
    fn get(&self, k: (u64, usize)) -> Option<(f32, f32)> {
        self.0.lock().unwrap().get(&k).copied()
    }

    fn put(&self, k: (u64, usize), v: (f32, f32)) {
        let mut m = self.0.lock().unwrap();
        if m.len() >= BLOCKS_KEPT {
            m.clear();
        }
        m.insert(k, v);
    }
}

/// Tile z/x/y's coverage (z ≥ 5), from the z14 tiles under it or the one over it: `get(x, y)`, the
/// z14 tile's water in each of the basemap's archives that has it (summed, as abutting water).
pub fn cover(z: u8, x: u32, y: u32, get: &(dyn Fn(u32, u32) -> Result<Vec<Stored>> + Sync), blocks: &Blocks) -> Result<Cov> {
    if !(5..=24).contains(&z) {
        bail!("water tiles are drawn at z5–24, not z{z}");
    }
    if z > BASE_Z {
        // One z14 tile over it: its square, `side` px, puts this tile at the origin.
        let d = z - BASE_Z;
        let (bx, by) = (x >> d, y >> d);
        let side = (SIZE << d) as f64;
        let (ox, oy) = (-f64::from(x - (bx << d)) * SIZE as f64, -f64::from(y - (by << d)) * SIZE as f64);
        let (mut s, mut i) = (Raster::new(SIZE, SIZE), Raster::new(SIZE, SIZE));
        for t in get(bx, by)? {
            draw(&tile_water(&t.bytes)?, &mut s, &mut i, ox, oy, side);
        }
        return Ok(Cov { sea: s.coverage(), inland: i.coverage() });
    }
    // The z14 tiles under it, each a square of `side` px drawn on its own.
    let d = BASE_Z - z;
    let n = 1u32 << d;
    let side = SIZE >> d;
    let blocks_of: Vec<(Vec<f32>, Vec<f32>)> = (0..n * n)
        .into_par_iter()
        .map(|k| -> Result<(Vec<f32>, Vec<f32>)> {
            let (cx, cy) = ((x << d) + k % n, (y << d) + k / n);
            let tiles = get(cx, cy)?;
            // A tile shared by many (the open sea's): drawn once.
            let key = (tiles.len() == 1).then(|| (tiles[0].key, side));
            if let Some((s, i)) = key.and_then(|k| blocks.get(k)) {
                return Ok((vec![s; side * side], vec![i; side * side]));
            }
            let (mut s, mut i) = (Raster::new(side, side), Raster::new(side, side));
            for t in &tiles {
                draw(&tile_water(&t.bytes)?, &mut s, &mut i, 0.0, 0.0, side as f64);
            }
            let (s, i) = (s.coverage(), i.coverage());
            if let Some(k) = key {
                if s.iter().all(|&v| v == s[0]) && i.iter().all(|&v| v == i[0]) {
                    blocks.put(k, (s[0], i[0]));
                }
            }
            Ok((s, i))
        })
        .collect::<Result<_>>()?;
    let mut out = Cov::uniform(0.0, 0.0);
    for (k, (s, i)) in blocks_of.iter().enumerate() {
        let (bx, by) = ((k as u32 % n) as usize * side, (k as u32 / n) as usize * side);
        for r in 0..side {
            let o = (by + r) * SIZE + bx;
            out.sea[o..o + side].copy_from_slice(&s[r * side..(r + 1) * side]);
            out.inland[o..o + side].copy_from_slice(&i[r * side..(r + 1) * side]);
        }
    }
    Ok(out)
}

/// A tile not stored, from its nearest stored ancestor (`stored(z, x, y)`): one value throughout,
/// the ancestor's over it.
pub fn uniform_from_ancestor(z: u8, x: u32, y: u32, stored: &dyn Fn(u8, u32, u32) -> Result<Option<Cov>>) -> Result<Option<(u8, u8)>> {
    for a in (0..z).rev() {
        let d = z - a;
        if let Some(c) = stored(a, x >> d, y >> d)? {
            // The middle of this tile's region of the ancestor.
            let span = SIZE as f64 / f64::from(1u32 << d);
            let px = |v: u32, base: u32| ((f64::from(v - (base << d)) + 0.5) * span) as usize;
            return Ok(Some(c.value_at(px(x, x >> d), px(y, y >> d))));
        }
    }
    Ok(None)
}

// ---- the build --------------------------------------------------------------------------------------

/// A stored tile: z, x, y and its PNG.
pub type TileOut = (u8, u32, u32, Vec<u8>);

/// The basemap's z14 tiles, as their directory has them: (first tile id, run, absolute offset,
/// length), by tile id.
pub struct Z14 {
    entries: Vec<(u64, u32, u64, u32)>,
}

const Z14_BASE: u64 = ((1u64 << 28) - 1) / 3;

impl Z14 {
    pub fn read(pm: &store::pmtiles::PmTiles) -> Result<Z14> {
        let h = pm.header();
        let (lo, hi) = (Z14_BASE, Z14_BASE + (1u64 << 28));
        let mut entries = Vec::new();
        pm.for_each_entry(|e| {
            if e.tile_id + u64::from(e.run_length) > lo && e.tile_id < hi {
                entries.push((e.tile_id, e.run_length, h.data_offset + e.offset, e.length));
            }
            Ok(())
        })?;
        Ok(Z14 { entries })
    }

    /// The entries for z14 tile ids `[a, b)`.
    fn range(&self, a: u64, b: u64) -> &[(u64, u32, u64, u32)] {
        let i = self.entries.partition_point(|e| e.0 + u64::from(e.1) <= a);
        let j = self.entries.partition_point(|e| e.0 < b);
        &self.entries[i..j.max(i)]
    }

    /// Where z14 tile `id` is, if the archive has it.
    fn find(&self, id: u64) -> Option<(u64, u32)> {
        let i = self.entries.partition_point(|e| e.0 + u64::from(e.1) <= id);
        self.entries.get(i).filter(|e| e.0 <= id).map(|e| (e.2, e.3))
    }
}

/// The Hilbert position of z/x/y (as PMTiles numbers its tiles).
fn hilbert(z: u8, x: u32, y: u32) -> u64 {
    store::pmtiles::zxy_to_tile_id(z, x, y).expect("a tile") - ((1u64 << (2 * u32::from(z))) - 1) / 3
}

/// What a build made.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Made {
    /// Tiles stored by zoom, and their bytes.
    pub tiles: Vec<u64>,
    pub bytes: Vec<u64>,
    /// z`DRAW_Z` tiles drawn from their z14 tiles, and z14 tiles read.
    pub drawn: u64,
    pub read: u64,
}

/// Builds the stored zooms (0–`STORED_MAXZ`) from the basemap (`pm`, its z14 directory `z14`),
/// over the z5 tiles `only` (all when None: then z0–4 too). Returns the stored tiles (z, x, y,
/// PNG) in key order, and what was made. Reads each z7 tile's z14 tiles in a few long reads (the
/// archive is on the NAS), and draws z5 tiles in parallel.
pub fn build(pm: &store::pmtiles::PmTiles, z14: &Z14, only: Option<&[(u32, u32)]>, said: &(dyn Fn(u64, u64) + Sync)) -> Result<(Vec<TileOut>, Made)> {
    let fives: Vec<(u32, u32)> = match only {
        Some(o) => o.to_vec(),
        None => (0..32u32).flat_map(|y| (0..32u32).map(move |x| (x, y))).collect(),
    };
    let blocks = Blocks::default();
    let done = std::sync::atomic::AtomicU64::new(0);
    let total = fives.len() as u64;
    let drawn = std::sync::atomic::AtomicU64::new(0);
    let read = std::sync::atomic::AtomicU64::new(0);
    // Each z5 tile's subtree: its node and the tiles stored under it (z5–9).
    let subtrees: Vec<(Node, Vec<TileOut>)> = fives
        .par_iter()
        .map(|&(x5, y5)| -> Result<(Node, Vec<TileOut>)> {
            let mut out = Vec::new();
            let node = subtree(pm, z14, &blocks, 5, x5, y5, &mut out, &drawn, &read)?;
            said(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, total);
            Ok((node, out))
        })
        .collect::<Result<_>>()?;
    let mut tiles: Vec<TileOut> = Vec::new();
    let mut nodes: HashMap<(u32, u32), Node> = HashMap::new();
    for ((x, y), (node, ts)) in fives.iter().zip(subtrees) {
        tiles.extend(ts);
        nodes.insert((*x, *y), node);
    }
    if only.is_none() {
        // z4 to z0 from the z5 nodes.
        let mut level = nodes;
        for z in (0..5u8).rev() {
            let mut up = HashMap::new();
            let n = 1u32 << z;
            for y in 0..n {
                for x in 0..n {
                    let kid = |dx: u32, dy: u32| level.get(&(2 * x + dx, 2 * y + dy)).cloned().unwrap_or(Node::Uniform(0, 0));
                    let kids = [kid(0, 0), kid(1, 0), kid(0, 1), kid(1, 1)];
                    let node = combine(&kids);
                    if let Node::Cov(c) = &node {
                        tiles.push((z, x, y, c.png()));
                    }
                    up.insert((x, y), node);
                }
            }
            level = up;
        }
    }
    tiles.sort_by_key(|t| (t.0, t.2, t.1));
    let mut made = Made { tiles: vec![0; usize::from(STORED_MAXZ) + 1], bytes: vec![0; usize::from(STORED_MAXZ) + 1], ..Default::default() };
    for t in &tiles {
        made.tiles[usize::from(t.0)] += 1;
        made.bytes[usize::from(t.0)] += t.3.len() as u64;
    }
    made.drawn = drawn.into_inner();
    made.read = read.into_inner();
    Ok((tiles, made))
}

/// Four children's parent: uniform when they're all one uniform value.
fn combine(kids: &[Node; 4]) -> Node {
    if let Node::Uniform(s, i) = kids[0] {
        if kids.iter().all(|k| *k == Node::Uniform(s, i)) {
            return Node::Uniform(s, i);
        }
    }
    let c = Cov::average([&kids[0], &kids[1], &kids[2], &kids[3]]);
    match c.uniform_bytes() {
        Some((s, i)) => Node::Uniform(s, i),
        None => Node::Cov(c),
    }
}

#[allow(clippy::too_many_arguments)]
fn subtree(pm: &store::pmtiles::PmTiles, z14: &Z14, blocks: &Blocks, z: u8, x: u32, y: u32, out: &mut Vec<TileOut>, drawn: &std::sync::atomic::AtomicU64, read: &std::sync::atomic::AtomicU64) -> Result<Node> {
    use std::sync::atomic::Ordering::Relaxed;
    let d = 2 * u32::from(BASE_Z - z);
    let first = Z14_BASE + (hilbert(z, x, y) << d);
    let entries = z14.range(first, first + (1u64 << d));
    if entries.is_empty() {
        // No basemap tile under it: land.
        return Ok(Node::Uniform(0, 0));
    }
    if z == 7 {
        // The z14 tiles under this z7 tile, read in a few long reads: runs of nearby bytes.
        let mut spans: Vec<(u64, u64)> = Vec::new();
        let mut offs: Vec<(u64, u32)> = entries.iter().map(|e| (e.2, e.3)).collect();
        offs.sort_unstable();
        offs.dedup();
        for (o, l) in offs {
            match spans.last_mut() {
                Some(s) if o <= s.1 + (1 << 20) && o + u64::from(l) - s.0 <= 256 << 20 => s.1 = s.1.max(o + u64::from(l)),
                _ => spans.push((o, o + u64::from(l))),
            }
        }
        let mut bytes: HashMap<u64, Arc<Vec<u8>>> = HashMap::new();
        for (a, b) in spans {
            let buf = pm.source().read_at(a, usize::try_from(b - a)?)?;
            for e in entries {
                if e.2 >= a && e.2 + u64::from(e.3) <= b {
                    bytes.entry(e.2).or_insert_with(|| Arc::new(buf[(e.2 - a) as usize..(e.2 - a) as usize + e.3 as usize].to_vec()));
                }
            }
        }
        read.fetch_add(bytes.len() as u64, Relaxed);
        let get = |cx: u32, cy: u32| -> Result<Vec<Stored>> {
            let id = store::pmtiles::zxy_to_tile_id(BASE_Z, cx, cy)?;
            Ok(z14.find(id).and_then(|(o, _)| bytes.get(&o).map(|b| Stored { key: o, bytes: b.clone() })).into_iter().collect())
        };
        return walk(z, x, y, &get, blocks, out, drawn);
    }
    let mut kids = Vec::with_capacity(4);
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        kids.push(subtree(pm, z14, blocks, z + 1, 2 * x + dx, 2 * y + dy, out, drawn, read)?);
    }
    let node = combine(&[kids[0].clone(), kids[1].clone(), kids[2].clone(), kids[3].clone()]);
    if let Node::Cov(c) = &node {
        if z <= STORED_MAXZ {
            out.push((z, x, y, c.png()));
        }
    }
    Ok(node)
}

/// Below z7, with the z14 tiles at hand: down to `DRAW_Z`, drawn there, averaged back up.
fn walk(z: u8, x: u32, y: u32, get: &(dyn Fn(u32, u32) -> Result<Vec<Stored>> + Sync), blocks: &Blocks, out: &mut Vec<TileOut>, drawn: &std::sync::atomic::AtomicU64) -> Result<Node> {
    let node = if z == DRAW_Z {
        // No basemap tile under it: land. (The open sea's tiles, all one, are drawn once: Blocks.)
        let d = BASE_Z - z;
        let n = 1u32 << d;
        drawn.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut any = false;
        for k in 0..n * n {
            if !get((x << d) + k % n, (y << d) + k / n)?.is_empty() {
                any = true;
                break;
            }
        }
        if any { node_of(cover(z, x, y, get, blocks)?) } else { Node::Uniform(0, 0) }
    } else {
        let mut kids = Vec::with_capacity(4);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            kids.push(walk(z + 1, 2 * x + dx, 2 * y + dy, get, blocks, out, drawn)?);
        }
        combine(&[kids[0].clone(), kids[1].clone(), kids[2].clone(), kids[3].clone()])
    };
    if let Node::Cov(c) = &node {
        if z <= STORED_MAXZ {
            out.push((z, x, y, c.png()));
        }
    }
    Ok(node)
}

fn node_of(c: Cov) -> Node {
    match c.uniform_bytes() {
        Some((s, i)) => Node::Uniform(s, i),
        None => Node::Cov(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile with one water layer: polygons (rings in tile units, extent 4096) with a class.
    fn mvt(polys: &[(&str, Vec<Vec<[i32; 2]>>)]) -> Vec<u8> {
        use names::mvt::{Feature, Layer, Tile, Value};
        let mut values = vec![Value::String("ocean".into()), Value::String("lake".into())];
        values.push(Value::String("tunnel".into()));
        let features = polys
            .iter()
            .map(|(class, rings)| {
                let mut g = Vec::new();
                let (mut cx, mut cy) = (0i32, 0i32);
                let zz = |v: i32| ((v << 1) ^ (v >> 31)) as u32;
                for r in rings {
                    g.push(1 | (1 << 3));
                    g.push(zz(r[0][0] - cx));
                    g.push(zz(r[0][1] - cy));
                    (cx, cy) = (r[0][0], r[0][1]);
                    g.push(2 | ((r.len() as u32 - 1) << 3));
                    for p in &r[1..] {
                        g.push(zz(p[0] - cx));
                        g.push(zz(p[1] - cy));
                        (cx, cy) = (p[0], p[1]);
                    }
                    g.push(7 | (1 << 3));
                }
                let tags = match *class {
                    "ocean" => vec![0, 0],
                    "tunnel" => vec![0, 1, 1, 2],
                    _ => vec![0, 1],
                };
                Feature { id: None, tags, geom_type: Some(3), geometry: g, unknown: Vec::new() }
            })
            .collect();
        let l = Layer { name: "water".into(), version: 2, extent: 4096, keys: vec!["class".into(), "brunnel".into()], values, features, unknown: Vec::new() };
        let other = Layer { name: "roads".into(), version: 2, extent: 4096, keys: vec![], values: vec![], features: vec![], unknown: Vec::new() };
        names::mvt::gzip(&Tile { layers: vec![other, l], unknown: Vec::new() }.encode()).unwrap()
    }

    /// A clockwise square (tile units, y down).
    fn sq(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<[i32; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn a_tile_s_water_sea_and_inland_apart_tunnels_left_out() {
        let t = mvt(&[("ocean", vec![sq(0, 0, 4096, 2048)]), ("lake", vec![sq(1024, 3072, 2048, 4096)]), ("tunnel", vec![sq(3072, 3072, 4096, 4096)])]);
        let w = tile_water(&t).unwrap();
        assert_eq!(w.rings.len(), 2);
        assert!(w.rings[0].0 && !w.rings[1].0);
        let get = |_: u32, _: u32| -> Result<Vec<Stored>> { Ok(vec![Stored { key: 1, bytes: Arc::new(t.clone()) }]) };
        // z14 itself: the sea the top half, the lake a sixteenth in the bottom row.
        let c = cover(14, 100, 200, &get, &Blocks::default()).unwrap();
        let sum = |v: &[f32]| v.iter().map(|&a| f64::from(a)).sum::<f64>() / (SIZE * SIZE) as f64;
        assert!((sum(&c.sea) - 0.5).abs() < 1e-6 && (sum(&c.inland) - 1.0 / 16.0).abs() < 1e-6, "{} {}", sum(&c.sea), sum(&c.inland));
        // z16: a quarter of it, the top left: all sea.
        let c = cover(16, 400, 800, &get, &Blocks::default()).unwrap();
        assert!(c.sea.iter().all(|&v| v == 1.0) && c.inland.iter().all(|&v| v == 0.0));
        // z12: 16 such tiles, each a 128-px block.
        let c = cover(12, 25, 50, &get, &Blocks::default()).unwrap();
        assert!((sum(&c.sea) - 0.5).abs() < 1e-6 && (sum(&c.inland) - 1.0 / 16.0).abs() < 1e-6);
        assert_eq!(c.value_at(10, 10), (255, 0));
        assert_eq!(c.value_at(128 + 40, 120), (0, 255));
    }

    #[test]
    fn averaging_down_keeps_the_area_and_the_png_keeps_the_bytes() {
        let mut c = Cov::uniform(0.0, 0.0);
        for (i, v) in c.sea.iter_mut().enumerate() {
            *v = ((i * 7919) % 256) as f32 / 255.0;
        }
        c.inland[5] = 0.5;
        let kids = [Node::Cov(c.clone()), Node::Uniform(255, 0), Node::Uniform(0, 0), Node::Uniform(0, 255)];
        let p = Cov::average([&kids[0], &kids[1], &kids[2], &kids[3]]);
        let mean = |v: &[f32]| v.iter().map(|&a| f64::from(a)).sum::<f64>() / v.len() as f64;
        let want = (mean(&c.sea) + 1.0) / 4.0;
        assert!((mean(&p.sea) - want).abs() < 1e-5);
        assert_eq!(p.value_at(SIZE - 1, 0), (255, 0));
        assert_eq!(p.value_at(SIZE - 1, SIZE - 1), (0, 255));
        let back = Cov::from_png(&p.png()).unwrap();
        assert!(back.sea.iter().zip(&p.sea).all(|(a, b)| byte(*a) == byte(*b)));
        assert_eq!(p.png(), p.png());
        assert_eq!(combine(&[Node::Uniform(3, 4), Node::Uniform(3, 4), Node::Uniform(3, 4), Node::Uniform(3, 4)]), Node::Uniform(3, 4));
    }

    #[test]
    fn the_blocks_kept_are_capped() {
        let b = Blocks::default();
        for k in 0..BLOCKS_KEPT as u64 + 10 {
            b.put((k, 32), (1.0, 0.0));
        }
        assert!(b.0.lock().unwrap().len() <= BLOCKS_KEPT);
        assert_eq!(b.get((BLOCKS_KEPT as u64 + 9, 32)), Some((1.0, 0.0)));
        assert_eq!(b.get((BLOCKS_KEPT as u64 + 9, 16)), None);
    }

    #[test]
    fn a_tile_not_stored_takes_its_ancestor_s_value() {
        // z3's top left quarter sea, the rest land: z5 tile 1/1 is in the sea, 6/6 on land.
        let mut c = Cov::uniform(0.0, 0.0);
        for y in 0..SIZE / 2 {
            for x in 0..SIZE / 2 {
                c.sea[y * SIZE + x] = 1.0;
            }
        }
        let stored = |z: u8, x: u32, y: u32| -> Result<Option<Cov>> { Ok((z == 3 && x == 0 && y == 0).then(|| c.clone())) };
        assert_eq!(uniform_from_ancestor(5, 1, 1, &stored).unwrap(), Some((255, 0)));
        assert_eq!(uniform_from_ancestor(5, 3, 3, &stored).unwrap(), Some((0, 0)));
        assert_eq!(uniform_from_ancestor(2, 0, 0, &stored).unwrap(), None);
    }
}
