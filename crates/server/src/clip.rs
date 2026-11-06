//! Away from the NAS, a tile shows only what lies inside the downloads (docs/macs.md, the offline
//! map): a tile coarser than the downloads' grid (Web Mercator's z6 tiles; a polar cap's blocks)
//! that only partly lies over them is clipped to them as it's served (Data::tile, the basemap's
//! tiles). Rasters' pixels outside become the value the layer has where it has nothing; vector
//! tiles lose what lies outside: points outside go, lines and areas are cut at the cells' edges.
//! One function per stored encoding (the catalogue's `encoding`).

use anyhow::{bail, Context, Result};
use det::Det;
use std::collections::{BTreeMap, HashSet};

/// Cells of one grid that the downloads hold: Web Mercator's tiles at zoom `z` (6), or a polar
/// cap's blocks (roadcore::polar::BLOCK_Z).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cells {
    pub z: u8,
    pub set: HashSet<(u32, u32)>,
}

/// How much of a tile the cells hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    All,
    Part,
    None,
}

/// Where a vector tile's features may lie: the cells meeting the tile grown by this share of it
/// on each side (its buffer: a feature there, in a cell held, is kept).
const MARGIN: f64 = 0.5;

impl Cells {
    /// How much of tile z/x/y the cells hold: a tile at the cells' zoom or finer lies in one cell.
    pub fn cover(&self, z: u8, x: u32, y: u32) -> Cover {
        let Some(d) = self.z.checked_sub(z) else {
            let d = z - self.z;
            return if self.set.contains(&(x >> d, y >> d)) { Cover::All } else { Cover::None };
        };
        let n = 1u32 << d;
        let held = self.set.iter().filter(|&&(cx, cy)| (cx >> d, cy >> d) == (x, y)).count();
        match held {
            0 => Cover::None,
            h if h == (n * n) as usize => Cover::All,
            _ => Cover::Part,
        }
    }

    /// Per cell under tile z/x/y (coarser than the cells), row by row, whether it's held; and the
    /// cells a side.
    fn under(&self, z: u8, x: u32, y: u32) -> (Vec<bool>, usize) {
        let d = self.z.saturating_sub(z);
        let n = 1usize << d;
        let mut g = vec![false; n * n];
        for &(cx, cy) in &self.set {
            if (cx >> d, cy >> d) == (x, y) {
                g[(cy as usize - ((y as usize) << d)) * n + (cx as usize - ((x as usize) << d))] = true;
            }
        }
        (g, n)
    }

    /// The cells as rectangles (rows' runs joined down), in tile z/x/y's units (0–1 across it):
    /// those meeting the tile grown by `margin` on each side, or every cell (`margin` None).
    fn rects(&self, z: u8, x: u32, y: u32, margin: Option<f64>) -> Vec<[f64; 4]> {
        // (A cell is 2^(z − cells' z) of the tile a side.)
        let s = 2f64.powi(i32::from(z) - i32::from(self.z));
        let at = |c: u32, t: u32| f64::from(c) * s - f64::from(t);
        let mut rows: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for &(cx, cy) in &self.set {
            let (x0, y0) = (at(cx, x), at(cy, y));
            if margin.is_none_or(|m| x0 < 1.0 + m && x0 + s > -m && y0 < 1.0 + m && y0 + s > -m) {
                rows.entry(cy).or_default().push(cx);
            }
        }
        // (Runs of a row, [first, past the last), and the rectangle each is the bottom row of.)
        let mut open: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
        let mut done = Vec::new();
        let mut prev_row = None;
        for (cy, mut xs) in rows {
            xs.sort_unstable();
            let mut runs = Vec::new();
            for cx in xs {
                match runs.last_mut() {
                    Some((_, end)) if *end == cx => *end += 1,
                    _ => runs.push((cx, cx + 1)),
                }
            }
            let mut next = BTreeMap::new();
            for r in runs {
                let top = match open.remove(&r) {
                    Some((top, last)) if prev_row == Some(last) => top,
                    Some(o) => {
                        done.push((r, o));
                        cy
                    }
                    None => cy,
                };
                next.insert(r, (top, cy));
            }
            done.extend(std::mem::take(&mut open));
            open = next;
            prev_row = Some(cy);
        }
        done.extend(open);
        done.into_iter().map(|((x0, x1), (y0, y1))| [at(x0, x), at(y0, y), at(x1, x), at(y1 + 1, y)]).collect()
    }
}

/// Tile z/x/y of `layer` as stored (`blob`, gzip'd when `gzip`), clipped to `cells`: the same
/// encoding, gzip'd alike. An error for an encoding this can't clip (none of them is stored coarser
/// than the cells), or a tile that won't decode: the caller serves none rather than all of it.
pub fn tile(
    encoding: &str,
    layer: &str,
    z: u8,
    x: u32,
    y: u32,
    blob: &[u8],
    gzip: bool,
    cells: &Cells,
) -> Result<Vec<u8>> {
    let raw = names::mvt::gunzip_if_gzip(blob)?;
    let out = match encoding {
        "terrarium-webp" => {
            let nothing = if layer == pipeline::depth::LAYER { pipeline::depth::NODATA } else { 0.0 };
            raster(&raw, z, x, y, cells, &roadcore::grid::terrarium_rgb(&[nothing]))?
        }
        "shade-webp" => raster(&raw, z, x, y, cells, &roadcore::shade::rgb(&[roadcore::shade::Px::default()]))?,
        "class-webp" | "water-png" => raster(&raw, z, x, y, cells, &[0; 3])?,
        "field5-webp" => field5(&raw, z, x, y, cells)?,
        "mvt" => mvt(&raw, z, x, y, cells)?,
        "rt7" => roads(&raw, z, x, y, cells)?,
        "rdmt" => marks(&raw, z, x, y, cells)?,
        "geojson-gz" => ferries(&raw, z, x, y, cells)?,
        e => bail!("{layer}: tiles of {e} can't be clipped"),
    };
    if gzip {
        names::mvt::gzip(&out)
    } else {
        Ok(out)
    }
}

/// The basemap's tile z/x/y (raw MVT, merged from its archives) clipped to `cells`.
pub fn basemap(raw: &[u8], z: u8, x: u32, y: u32, cells: &Cells) -> Result<Vec<u8>> {
    mvt(raw, z, x, y, cells)
}

// ---- rasters ------------------------------------------------------------------------------------

/// A raster's pixels outside the cells set to `nothing` (by the pixel's plane: `field5`'s blocks);
/// `w` × `h` pixels, `ch` bytes each, a tile `side` pixels a side (planes stacked down).
#[allow(clippy::too_many_arguments)]
fn mask_pixels(
    px: &mut [u8],
    w: usize,
    h: usize,
    ch: usize,
    side: usize,
    z: u8,
    x: u32,
    y: u32,
    cells: &Cells,
    nothing: &dyn Fn(usize) -> Vec<u8>,
) {
    let (held, n) = cells.under(z, x, y);
    for row in 0..h {
        let (plane, r) = (row / side, row % side);
        let fill = nothing(plane);
        let cy = r * n / side;
        for col in 0..w {
            if !held[cy * n + col * n / w] {
                px[(row * w + col) * ch..][..fill.len()].copy_from_slice(&fill);
            }
        }
    }
}

/// A lossless WebP's RGB pixels.
fn webp_rgb(b: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(b)).map_err(|e| anyhow::anyhow!("WebP: {e}"))?;
    let (w, h) = d.dimensions();
    let ch = if d.has_alpha() { 4 } else { 3 };
    let mut buf = vec![0u8; d.output_buffer_size().context("WebP: too large")?];
    d.read_image(&mut buf).map_err(|e| anyhow::anyhow!("WebP: {e}"))?;
    let rgb = if ch == 3 { buf } else { buf.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect() };
    Ok((rgb, w, h))
}

/// The tiles' lossless preset (pipeline::terrain_codec::LEVEL, the shading's alike).
fn webp(rgb: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    webpenc::lossless_rgb8(rgb, w, h, pipeline::terrain_codec::LEVEL).map_err(|m| anyhow::anyhow!(m))
}

/// A raster tile of one plane (terrain, depth, shading, classes, tree cover, water), its pixels
/// outside `nothing` (RGB; its first byte a grey pixel's, transparent where there's alpha but not
/// colour: the water's none): a lossless WebP as one, a PNG as one of the same kind (a pack from
/// before its layer's WebP, `terrarium-png` and the like, until its step is rebuilt).
fn raster(b: &[u8], z: u8, x: u32, y: u32, cells: &Cells, nothing: &[u8]) -> Result<Vec<u8>> {
    if b.starts_with(b"RIFF") {
        let (mut rgb, w, h) = webp_rgb(b)?;
        mask_pixels(&mut rgb, w as usize, h as usize, 3, h as usize, z, x, y, cells, &|_| nothing.to_vec());
        return webp(&rgb, w, h);
    }
    let mut d = png::Decoder::new(std::io::Cursor::new(b)).read_info().context("a PNG tile")?;
    let mut px = vec![0u8; d.output_buffer_size().context("a PNG tile's size")?];
    let info = d.next_frame(&mut px).context("a PNG tile")?;
    let fill: Vec<u8> = match (info.color_type, info.bit_depth) {
        (png::ColorType::Grayscale, png::BitDepth::Eight) => vec![nothing[0]],
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => vec![nothing[0], 0],
        (png::ColorType::Rgb, png::BitDepth::Eight) => nothing.to_vec(),
        (png::ColorType::Rgba, png::BitDepth::Eight) => [nothing, &[255]].concat(),
        (c, d) => bail!("a PNG tile of another kind ({c:?} {d:?})"),
    };
    let (w, h) = (info.width as usize, info.height as usize);
    px.truncate(w * h * fill.len());
    mask_pixels(&mut px, w, h, fill.len(), h, z, x, y, cells, &|_| fill.clone());
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w as u32, h as u32);
    e.set_color(info.color_type);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::High);
    let mut wr = e.write_header()?;
    wr.write_image_data(&px)?;
    wr.finish()?;
    Ok(out)
}

/// A `field5-webp` tile (docs/formats.md): the classes' byte, a WebP of a 512-row block per class
/// held and then one of its offsets per class held, and what follows it (a level 8 tile's heritage
/// list) as it is. Outside: no building, offsets (128, 128, 0).
fn field5(b: &[u8], z: u8, x: u32, y: u32, cells: &Cells) -> Result<Vec<u8>> {
    let (&classes, rest) = b.split_first().context("an empty field tile")?;
    let len = rest.get(4..8).map(|s| u32::from_le_bytes(s.try_into().unwrap()) as usize + 8).context("a field tile")?;
    let (img, after) = rest.split_at(len.min(rest.len()));
    let k = classes.count_ones() as usize;
    let (mut rgb, w, h) = webp_rgb(img)?;
    mask_pixels(&mut rgb, w as usize, h as usize, 3, w as usize, z, x, y, cells, &|plane| {
        if plane < k {
            vec![0, 0, 0]
        } else {
            vec![128, 128, 0]
        }
    });
    let mut out = vec![classes];
    out.extend(webp(&rgb, w, h)?);
    out.extend_from_slice(after);
    Ok(out)
}

// ---- geometry -----------------------------------------------------------------------------------

/// Whether point `p` lies in rectangle `r` ([x0, y0, x1, y1], its far edges out).
fn inside(r: &[f64; 4], p: [f64; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[2] && p[1] >= r[1] && p[1] < r[3]
}

/// Line `pts` cut to rectangle `r`: its pieces inside, each with each point's place along the line
/// it was cut from (the index of the point before it, and the share of the way to the next).
fn cut_line(pts: &[[f64; 2]], r: &[f64; 4]) -> Vec<Vec<([f64; 2], usize, f64)>> {
    let mut out: Vec<Vec<([f64; 2], usize, f64)>> = Vec::new();
    let mut cur: Vec<([f64; 2], usize, f64)> = Vec::new();
    for i in 0..pts.len().saturating_sub(1) {
        let (a, b) = (pts[i], pts[i + 1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        // Liang–Barsky.
        let (mut t0, mut t1) = (0f64, 1f64);
        let mut keep = true;
        for (p, q) in [(-dx, a[0] - r[0]), (dx, r[2] - a[0]), (-dy, a[1] - r[1]), (dy, r[3] - a[1])] {
            if p == 0.0 {
                if q < 0.0 {
                    keep = false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if !keep || t0 > t1 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let at = |t: f64| ([a[0] + t * dx, a[1] + t * dy], i, t);
        if t0 > 0.0 && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if cur.is_empty() {
            cur.push(at(t0));
        }
        cur.push(at(t1));
        if t1 < 1.0 {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|p| p.len() >= 2);
    out
}

/// Ring `ring` (closed, its last point not repeated) cut to rectangle `r` (Sutherland–Hodgman: its
/// winding kept).
fn cut_ring(ring: &[[f64; 2]], r: &[f64; 4]) -> Vec<[f64; 2]> {
    let mut pts = ring.to_vec();
    // (Each edge: inside when the coordinate `k` is on `side` of `v`.)
    for (k, v, low) in [(0, r[0], true), (0, r[2], false), (1, r[1], true), (1, r[3], false)] {
        if pts.is_empty() {
            break;
        }
        let ins = |p: &[f64; 2]| if low { p[k] >= v } else { p[k] <= v };
        let cross = |a: &[f64; 2], b: &[f64; 2]| {
            let t = (v - a[k]) / (b[k] - a[k]);
            [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
        };
        let mut out = Vec::with_capacity(pts.len() + 4);
        for i in 0..pts.len() {
            let (a, b) = (&pts[(i + pts.len() - 1) % pts.len()], &pts[i]);
            match (ins(a), ins(b)) {
                (true, true) => out.push(*b),
                (true, false) => out.push(cross(a, b)),
                (false, true) => {
                    out.push(cross(a, b));
                    out.push(*b);
                }
                (false, false) => {}
            }
        }
        pts = out;
    }
    pts
}

/// Twice a ring's signed area.
fn area2(ring: &[[i64; 2]]) -> i64 {
    (0..ring.len())
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

// ---- vector tiles -------------------------------------------------------------------------------

/// An MVT geometry's parts: each its points (the tile's units) and whether it was closed.
fn mvt_parts(g: &[u32]) -> Result<Vec<(Vec<[i64; 2]>, bool)>> {
    let mut parts: Vec<(Vec<[i64; 2]>, bool)> = Vec::new();
    let (mut cx, mut cy) = (0i64, 0i64);
    let mut i = 0;
    let unzz = |v: u32| (v >> 1) as i64 ^ -((v & 1) as i64);
    while i < g.len() {
        let (id, count) = (g[i] & 7, (g[i] >> 3) as usize);
        i += 1;
        match id {
            1 | 2 => {
                for k in 0..count {
                    let (Some(&dx), Some(&dy)) = (g.get(i), g.get(i + 1)) else { bail!("an MVT geometry cut short") };
                    i += 2;
                    cx += unzz(dx);
                    cy += unzz(dy);
                    if id == 1 || parts.is_empty() {
                        let _ = k;
                        parts.push((vec![[cx, cy]], false));
                    } else if let Some(p) = parts.last_mut() {
                        p.0.push([cx, cy]);
                    }
                }
            }
            7 => {
                if let Some(p) = parts.last_mut() {
                    p.1 = true;
                }
            }
            c => bail!("an MVT command {c}"),
        }
    }
    Ok(parts)
}

/// Parts as an MVT geometry of type `kind` (1 points, 2 lines, 3 polygons: rings closed).
fn mvt_geometry(kind: u32, parts: &[Vec<[i64; 2]>]) -> Vec<u32> {
    let zz = |v: i64| ((v << 1) ^ (v >> 63)) as u32;
    let cmd = |id: u32, n: usize| id | ((n as u32) << 3);
    let mut out = Vec::new();
    let (mut cx, mut cy) = (0i64, 0i64);
    let mut put = |out: &mut Vec<u32>, p: [i64; 2]| {
        out.push(zz(p[0] - cx));
        out.push(zz(p[1] - cy));
        (cx, cy) = (p[0], p[1]);
    };
    if kind == 1 {
        let pts: Vec<[i64; 2]> = parts.iter().flatten().copied().collect();
        if !pts.is_empty() {
            out.push(cmd(1, pts.len()));
            for p in pts {
                put(&mut out, p);
            }
        }
        return out;
    }
    for p in parts {
        out.push(cmd(1, 1));
        put(&mut out, p[0]);
        out.push(cmd(2, p.len() - 1));
        for &q in &p[1..] {
            put(&mut out, q);
        }
        if kind == 3 {
            out.push(cmd(7, 1));
        }
    }
    out
}

/// Points rounded to the tile's units, a point repeated dropped (and, a ring's, the last when it's
/// the first).
fn rounded(pts: impl Iterator<Item = [f64; 2]>, ring: bool) -> Vec<[i64; 2]> {
    let mut out: Vec<[i64; 2]> = Vec::new();
    for p in pts {
        let q = [p[0].round() as i64, p[1].round() as i64];
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    if ring && out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// A feature's geometry cut to `rects` (the tile's units); false when nothing of it is left.
fn clip_feature(f: &mut names::mvt::Feature, rects: &[[f64; 4]]) -> Result<bool> {
    let parts = mvt_parts(&f.geometry)?;
    let fl = |p: &[i64; 2]| [p[0] as f64, p[1] as f64];
    let kind = f.geom_type.unwrap_or(0);
    let out: Vec<Vec<[i64; 2]>> = match kind {
        1 => {
            let pts: Vec<[i64; 2]> = parts
                .iter()
                .flat_map(|p| p.0.iter())
                .filter(|p| rects.iter().any(|r| inside(r, fl(p))))
                .copied()
                .collect();
            if pts.is_empty() {
                Vec::new()
            } else {
                vec![pts]
            }
        }
        2 => {
            let mut out = Vec::new();
            for (pts, _) in &parts {
                let line: Vec<[f64; 2]> = pts.iter().map(fl).collect();
                for r in rects {
                    for piece in cut_line(&line, r) {
                        let p = rounded(piece.into_iter().map(|(p, _, _)| p), false);
                        if p.len() >= 2 {
                            out.push(p);
                        }
                    }
                }
            }
            out
        }
        3 => {
            // Polygons: an exterior ring (the first's winding), then its holes.
            let rings: Vec<&Vec<[i64; 2]>> = parts.iter().map(|p| &p.0).filter(|r| area2(r) != 0).collect();
            let Some(first) = rings.first() else { return Ok(false) };
            let outer = area2(first).signum();
            let mut polys: Vec<Vec<&Vec<[i64; 2]>>> = Vec::new();
            for r in rings {
                if area2(r).signum() == outer || polys.is_empty() {
                    polys.push(vec![r]);
                } else if let Some(p) = polys.last_mut() {
                    p.push(r);
                }
            }
            let mut out = Vec::new();
            for r in rects {
                for poly in &polys {
                    let cut = |ring: &Vec<[i64; 2]>| {
                        let c = rounded(cut_ring(&ring.iter().map(fl).collect::<Vec<_>>(), r).into_iter(), true);
                        (c.len() >= 3 && area2(&c).signum() == area2(ring).signum()).then_some(c)
                    };
                    let Some(ext) = cut(poly[0]) else { continue };
                    out.push(ext);
                    out.extend(poly[1..].iter().filter_map(|h| cut(h)));
                }
            }
            out
        }
        // (Of no known type: kept only when all of it lies inside.)
        _ => {
            let all = parts.iter().flat_map(|p| p.0.iter()).all(|p| rects.iter().any(|r| inside(r, fl(p))));
            return Ok(all);
        }
    };
    if out.is_empty() {
        return Ok(false);
    }
    f.geometry = mvt_geometry(kind, &out);
    Ok(true)
}

/// An MVT tile (raw) clipped to the cells.
fn mvt(raw: &[u8], z: u8, x: u32, y: u32, cells: &Cells) -> Result<Vec<u8>> {
    let mut t = names::mvt::Tile::decode(raw)?;
    let frac = cells.rects(z, x, y, Some(MARGIN));
    for l in &mut t.layers {
        let e = f64::from(l.extent);
        let rects: Vec<[f64; 4]> = frac.iter().map(|r| r.map(|v| v * e)).collect();
        let mut kept = Vec::with_capacity(l.features.len());
        for mut f in std::mem::take(&mut l.features) {
            if clip_feature(&mut f, &rects)? {
                kept.push(f);
            }
        }
        l.features = kept;
    }
    Ok(t.encode())
}

/// A road or rail tile (`rt7`, roadcore::tile) clipped to the cells: each line cut at their edges,
/// its points' values at a cut taken along the line.
fn roads(raw: &[u8], z: u8, x: u32, y: u32, cells: &Cells) -> Result<Vec<u8>> {
    let (lines, ext) = roadcore::tile::decode(raw)?;
    let e = f64::from(1u32 << ext);
    let rects: Vec<[f64; 4]> = cells.rects(z, x, y, Some(MARGIN)).iter().map(|r| r.map(|v| v * e)).collect();
    let mut out = Vec::new();
    for l in &lines {
        let pts: Vec<[f64; 2]> = l.pts.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect();
        let full: f64 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).dhypot(w[1][1] - w[0][1])).sum();
        for r in &rects {
            for piece in cut_line(&pts, r) {
                let mut nl = roadcore::tile::TileLine {
                    pts: Vec::new(),
                    elev: Vec::new(),
                    grade: Vec::new(),
                    drape: Vec::new(),
                    sc: Vec::new(),
                    ..l.clone()
                };
                for (p, i, t) in piece {
                    let q = [p[0].round() as i32, p[1].round() as i32];
                    if nl.pts.last() == Some(&q) {
                        continue;
                    }
                    let j = (i + 1).min(l.pts.len() - 1);
                    let mix = |a: f64, b: f64| a + (b - a) * t;
                    nl.pts.push(q);
                    nl.elev.push(mix(f64::from(l.elev[i]), f64::from(l.elev[j])).round() as i32);
                    nl.grade.push(mix(f64::from(l.grade[i]), f64::from(l.grade[j])).round() as u8);
                    nl.drape.push(mix(f64::from(l.drape[i]), f64::from(l.drape[j])).round() as i16);
                    nl.sc
                        .push(std::array::from_fn(|c| mix(f64::from(l.sc[i][c]), f64::from(l.sc[j][c])).round() as u8));
                }
                if nl.pts.len() < 2 {
                    continue;
                }
                let kept: f64 =
                    nl.pts.windows(2).map(|w| f64::from(w[1][0] - w[0][0]).dhypot(f64::from(w[1][1] - w[0][1]))).sum();
                if full > 0.0 {
                    nl.true_len_dm = (f64::from(l.true_len_dm) * kept / full).round() as u32;
                }
                out.push(nl);
            }
        }
    }
    Ok(roadcore::tile::encode(&out, ext))
}

/// A point's place in tile z/x/y's units (0–1 across it), from its lon/lat.
fn in_tile(z: u8, x: u32, y: u32, lon: f64, lat: f64) -> [f64; 2] {
    let (mx, my) = roadcore::geo::merc(lon, lat);
    let n = f64::from(1u32 << z);
    [mx * n - f64::from(x), my * n - f64::from(y)]
}

/// A landmarks tile (`rdmt`, codecs::marks) clipped to the cells: its points and speck cells
/// outside go.
fn marks(raw: &[u8], z: u8, x: u32, y: u32, cells: &Cells) -> Result<Vec<u8>> {
    use pipeline::marks::{deg, MarkTile, CELL_DZ};
    let t = MarkTile::decode(raw)?;
    let rects = cells.rects(z, x, y, Some(MARGIN));
    let held = |p: [f64; 2]| rects.iter().any(|r| inside(r, p));
    let keep: Vec<usize> =
        (0..t.ids.len()).filter(|&i| held(in_tile(z, x, y, deg(t.pts[i].lon), deg(t.pts[i].lat)))).collect();
    let side = f64::from(1u32 << CELL_DZ);
    let unmorton = |c: u32| {
        let squash = |mut v: u32| {
            v &= 0x5555_5555;
            v = (v | (v >> 1)) & 0x3333_3333;
            v = (v | (v >> 2)) & 0x0f0f_0f0f;
            v = (v | (v >> 4)) & 0x00ff_00ff;
            (v | (v >> 8)) & 0xffff
        };
        (squash(c), squash(c >> 1))
    };
    let out = MarkTile {
        ids: keep.iter().map(|&i| t.ids[i]).collect(),
        fvals: t.fvals.iter().map(|f| keep.iter().map(|&i| f[i]).collect()).collect(),
        pts: keep.iter().map(|&i| t.pts[i]).collect(),
        props: keep.iter().map(|&i| t.props[i].clone()).collect(),
        cells: t
            .cells
            .iter()
            .filter(|c| {
                let (cx, cy) = unmorton(c.code);
                held([(f64::from(cx) + 0.5) / side, (f64::from(cy) + 0.5) / side])
            })
            .copied()
            .collect(),
    };
    Ok(out.encode())
}

/// A block of ferries (`geojson-gz`, pipeline::ovconv: lines and terminals, and their lines'
/// records) clipped to the cells (all of them: a block reaches 30 km past its tile, and a way is
/// the same in every block). A way cut in pieces keeps its longest (the map draws a way as one
/// line); a terminal outside goes, and the records of no way kept.
fn ferries(raw: &[u8], z: u8, x: u32, y: u32, cells: &Cells) -> Result<Vec<u8>> {
    use serde_json::{json, Value};
    let mut fc: Value = serde_json::from_slice(raw).context("a ferry block")?;
    let rects = cells.rects(z, x, y, None);
    let n = f64::from(1u32 << z);
    let lonlat = |p: [f64; 2]| {
        let (mx, my) = ((p[0] + f64::from(x)) / n, (p[1] + f64::from(y)) / n);
        json!([roadcore::geo::merc_lon(mx), roadcore::geo::merc_lat(my)])
    };
    let pt = |c: &Value| Some(in_tile(z, x, y, c.get(0)?.as_f64()?, c.get(1)?.as_f64()?));
    let feats = fc["features"].as_array().cloned().unwrap_or_default();
    let mut kept = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();
    for mut f in feats {
        let g = &f["geometry"];
        match g["type"].as_str() {
            Some("Point") => {
                if pt(&g["coordinates"]).is_some_and(|p| rects.iter().any(|r| inside(r, p))) {
                    kept.push(f);
                }
            }
            Some("LineString") => {
                let line: Vec<[f64; 2]> = g["coordinates"].as_array().into_iter().flatten().filter_map(pt).collect();
                let longest = rects
                    .iter()
                    .flat_map(|r| cut_line(&line, r))
                    .map(|p| p.into_iter().map(|(p, _, _)| p).collect::<Vec<_>>())
                    .max_by(|a, b| {
                        let len = |v: &Vec<[f64; 2]>| {
                            v.windows(2).map(|w| (w[1][0] - w[0][0]).dhypot(w[1][1] - w[0][1])).sum::<f64>()
                        };
                        len(a).total_cmp(&len(b))
                    });
                let Some(piece) = longest else { continue };
                f["geometry"]["coordinates"] = Value::Array(piece.into_iter().map(lonlat).collect());
                ids.extend(
                    f["properties"]["lines"]
                        .as_str()
                        .unwrap_or("")
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                );
                kept.push(f);
            }
            _ => {}
        }
    }
    if let Some(recs) = fc.get_mut("lines").and_then(Value::as_object_mut) {
        recs.retain(|k, _| ids.contains(k));
    }
    fc["features"] = Value::Array(kept);
    Ok(serde_json::to_vec(&fc)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(z: u8, set: &[(u32, u32)]) -> Cells {
        Cells { z, set: set.iter().copied().collect() }
    }

    #[test]
    fn cover_and_rects() {
        // Two z6 tiles under z5 tile 0/0 (its top row), one under z5 tile 1/0.
        let c = cells(6, &[(0, 0), (1, 0), (2, 0)]);
        assert_eq!(c.cover(5, 0, 0), Cover::Part);
        assert_eq!(c.cover(6, 1, 0), Cover::All);
        assert_eq!(c.cover(7, 6, 1), Cover::None);
        assert_eq!(c.cover(4, 0, 0), Cover::Part);
        assert_eq!(cells(6, &[(0, 0), (1, 0), (0, 1), (1, 1)]).cover(5, 0, 0), Cover::All);
        // Joined into one rectangle: the top half of z5 0/0 and on into its neighbour.
        assert_eq!(c.rects(5, 0, 0, Some(0.5)), vec![[0.0, 0.0, 1.5, 0.5]]);
        // An L: two rectangles.
        let l = cells(6, &[(0, 0), (0, 1), (1, 1)]);
        let mut r = l.rects(5, 0, 0, Some(0.0));
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(r, vec![[0.0, 0.0, 0.5, 0.5], [0.0, 0.5, 1.0, 1.0]]);
    }

    #[test]
    fn lines_and_rings_are_cut_at_the_edges() {
        let r = [0.0, 0.0, 10.0, 10.0];
        let pieces = cut_line(&[[-5.0, 5.0], [5.0, 5.0], [5.0, 20.0], [6.0, 5.0]], &r);
        let pts: Vec<Vec<[f64; 2]>> = pieces.into_iter().map(|p| p.into_iter().map(|q| q.0).collect()).collect();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0], vec![[0.0, 5.0], [5.0, 5.0], [5.0, 10.0]]);
        assert_eq!(pts[1].last(), Some(&[6.0, 5.0]));
        let ring = cut_ring(&[[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]], &r);
        let ring = rounded(ring.into_iter(), true);
        assert_eq!(area2(&ring), 2 * 25);
    }

    /// An MVT tile of a point in each quarter, a line across and the whole tile's square, clipped to
    /// its top-left quarter: one point, the line's piece there, a quarter square.
    #[test]
    fn a_vector_tile_keeps_what_lies_inside() {
        use names::mvt::{Feature, Layer, Tile};
        let f = |kind: u32, parts: Vec<Vec<[i64; 2]>>| Feature {
            geom_type: Some(kind),
            geometry: mvt_geometry(kind, &parts),
            ..Default::default()
        };
        let t = Tile {
            layers: vec![Layer {
                name: "l".into(),
                version: 2,
                extent: 4096,
                keys: vec![],
                values: vec![],
                features: vec![
                    f(1, vec![vec![[100, 100], [3000, 100], [100, 3000], [3000, 3000]]]),
                    f(2, vec![vec![[0, 1000], [4096, 1000]]]),
                    f(3, vec![vec![[0, 0], [4096, 0], [4096, 4096], [0, 4096]]]),
                    f(1, vec![vec![[3000, 3000]]]),
                ],
                unknown: vec![],
            }],
            unknown: vec![],
        };
        let c = cells(6, &[(0, 0)]);
        let out = Tile::decode(&mvt(&t.encode(), 5, 0, 0, &c).unwrap()).unwrap();
        let fs = &out.layers[0].features;
        assert_eq!(fs.len(), 3);
        let parts = |g: &[u32]| mvt_parts(g).unwrap().into_iter().map(|p| p.0).collect::<Vec<_>>();
        assert_eq!(parts(&fs[0].geometry), vec![vec![[100, 100]]]);
        assert_eq!(parts(&fs[1].geometry), vec![vec![[0, 1000], [2048, 1000]]]);
        assert_eq!(area2(&parts(&fs[2].geometry)[0]), 2 * 2048 * 2048);
    }

    /// A terrain tile's pixels outside are flat (0 m), inside as they were.
    #[test]
    fn a_raster_keeps_its_pixels_inside() {
        let mut e = vec![123.0f32; 65536];
        let b = pipeline::terrain_codec::encode(&mut e, 0.1).unwrap();
        let out = tile("terrarium-webp", "terrain", 5, 0, 0, &b, false, &cells(6, &[(1, 1)])).unwrap();
        let back = roadcore::grid::decode_terrain(&out).unwrap();
        assert_eq!(back[0], 0.0);
        assert_eq!(back[200 * 256 + 200], 123.0);
        assert_eq!(back[200 * 256 + 20], 0.0);
    }

    /// A PNG tile (a terrain pack from before its WebP; the water's) stays a PNG of its kind, cut.
    #[test]
    fn a_png_raster_is_cut_as_one() {
        let png = |px: &[u8], ct: png::ColorType, side: u32| {
            let mut out = Vec::new();
            let mut e = png::Encoder::new(&mut out, side, side);
            e.set_color(ct);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(px).unwrap();
            w.finish().unwrap();
            out
        };
        let terrain = png(&roadcore::grid::terrarium_rgb(&vec![50.0; 65536]), png::ColorType::Rgb, 256);
        let out = tile("terrarium-webp", "terrain", 5, 0, 0, &terrain, false, &cells(6, &[(0, 1)])).unwrap();
        let back = roadcore::grid::decode_terrain(&out).unwrap();
        assert_eq!((back[200 * 256 + 20], back[20 * 256 + 20], back[200 * 256 + 200]), (50.0, 0.0, 0.0));
        let water = png(&[200, 30].repeat(512 * 512), png::ColorType::GrayscaleAlpha, 512);
        let out = tile("water-png", "water", 5, 0, 0, &water, false, &cells(6, &[(1, 0)])).unwrap();
        let c = pipeline::water::Cov::from_png(&out).unwrap();
        assert_eq!((c.value_at(400, 10), c.value_at(10, 10), c.value_at(400, 400)), ((200, 30), (0, 0), (0, 0)));
    }

    /// A road tile's lines cut at the edge, the values at the cut between its points'.
    #[test]
    fn a_road_tile_is_cut_at_the_edge() {
        use roadcore::tile::{TileLine, NCH};
        let l = TileLine {
            way: 5,
            true_len_dm: 1000,
            pts: vec![[0, 1000], [4096, 1000]],
            elev: vec![0, 100],
            grade: vec![0, 10],
            drape: vec![0, 10],
            sc: vec![[0; NCH], [10; NCH]],
            ..Default::default()
        };
        let b = roadcore::tile::encode(&[l], 12);
        let (out, _) = roadcore::tile::decode(&roads(&b, 5, 0, 0, &cells(6, &[(0, 0)])).unwrap()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pts, vec![[0, 1000], [2048, 1000]]);
        assert_eq!((out[0].elev[1], out[0].grade[1], out[0].sc[1][0], out[0].true_len_dm), (50, 5, 5, 500));
    }
}
