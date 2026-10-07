//! `bldprep T` (docs/buildings3d.md §3.1): z6 tile T's normalized buildings ([`super::work`]) from
//! the downloaded Overture files and GHSL tiles.
//!
//! `dem/bldprep.py` reads the row groups meeting T (pyarrow) and the GHSL windows under T
//! (rasterio), and writes their columns to stdout as frames ([`read_stream`]): Python only decodes
//! (§3.7). Here, for each building and part: its WKB is read; its centroid (area-weighted, as GEOS
//! has it, in degrees, in f64 in a fixed order) and footprint area (m²: degrees² × a degree's metres²
//! × cos(lat)) computed; its vertices and centroid rounded to E7; underground ones left out, and
//! those whose centroid isn't in T; GHSL sampled at the centroid. A building whose `has_parts` is set
//! is marked [`flag::HAS_PARTS`] when one of its parts was read (an outline none of whose parts are in
//! the files is drawn as a building). The records are sorted by (z14 tile, id), the strings coded
//! (each list sorted), and written as a block per z14 tile.

use super::work::{self, flag, osm, Block, Meta};
use super::{e7, z14_key, DEG_M};
use crate::legacy::Unit;
use crate::out::Out;
use anyhow::{bail, ensure, Context, Result};
use det::Det;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// The stream's first bytes.
pub const MAGIC: &[u8; 6] = b"BLDP1\n";

/// The frames' kinds.
pub mod kind {
    /// A row group's buildings (those whose box meets T).
    pub const BUILDINGS: u8 = 1;
    /// A row group's building parts (those whose box meets T grown by ~2 km).
    pub const PARTS: u8 = 2;
    /// A GHSL window: f32 rows, north first, in its tile's pixel grid.
    pub const GHSL: u8 = 3;
    /// The end: what was read.
    pub const END: u8 = 9;
}

/// A frame: its kind, its header (JSON) and its columns by name.
pub struct Frame {
    pub kind: u8,
    pub header: serde_json::Value,
    pub cols: HashMap<String, Vec<u8>>,
}

/// Reads a frame (`None` at a clean end of the stream): u8 kind, u32 header length, the header
/// (JSON, with `cols`: [[name, bytes], …]), then each column's bytes in that order.
pub fn read_frame(r: &mut dyn Read) -> Result<Option<Frame>> {
    let mut k = [0u8; 1];
    if r.read(&mut k)? == 0 {
        return Ok(None);
    }
    let mut l = [0u8; 4];
    r.read_exact(&mut l).context("a frame cut short")?;
    let mut h = vec![0u8; u32::from_le_bytes(l) as usize];
    r.read_exact(&mut h).context("a frame's header cut short")?;
    let header: serde_json::Value = serde_json::from_slice(&h).context("a frame's header")?;
    let mut cols = HashMap::new();
    for c in header["cols"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let (Some(name), Some(n)) = (c[0].as_str(), c[1].as_u64()) else { bail!("a frame's column {c}") };
        let mut b = vec![0u8; n as usize];
        r.read_exact(&mut b).with_context(|| format!("column {name} cut short"))?;
        cols.insert(name.to_string(), b);
    }
    Ok(Some(Frame { kind: k[0], header, cols }))
}

/// Writes a frame (tests, and anything else that hands records to `read_stream`).
pub fn write_frame(w: &mut dyn std::io::Write, kind: u8, mut header: serde_json::Value, cols: &[(&str, &[u8])]) -> Result<()> {
    header["cols"] = serde_json::Value::Array(cols.iter().map(|(n, b)| serde_json::json!([n, b.len()])).collect());
    let h = serde_json::to_vec(&header)?;
    w.write_all(&[kind])?;
    w.write_all(&(h.len() as u32).to_le_bytes())?;
    w.write_all(&h)?;
    for (_, b) in cols {
        w.write_all(b)?;
    }
    Ok(())
}

// ---- WKB -----------------------------------------------------------------------------------------

/// The polygons of a WKB Polygon or MultiPolygon (Z and M coordinates skipped), appended to
/// `pts` (every ring's points, as stored, closing point included) and `rings` (per ring: its polygon
/// within the geometry and its point count). The number of polygons.
pub fn wkb_polygons(b: &[u8], pts: &mut Vec<[f64; 2]>, rings: &mut Vec<(u32, u32)>) -> Result<u32> {
    struct W<'a> {
        b: &'a [u8],
        at: usize,
    }
    impl W<'_> {
        fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
            let s = self.b.get(self.at..self.at + N).context("WKB cut short")?;
            self.at += N;
            Ok(s.try_into().unwrap())
        }
        fn u32(&mut self, le: bool) -> Result<u32> {
            let a = self.bytes::<4>()?;
            Ok(if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
        }
        fn f64(&mut self, le: bool) -> Result<f64> {
            let a = self.bytes::<8>()?;
            Ok(if le { f64::from_le_bytes(a) } else { f64::from_be_bytes(a) })
        }
        /// A geometry's byte order and type: (little-endian, base type, coordinates a point).
        fn head(&mut self) -> Result<(bool, u32, usize)> {
            let le = match self.bytes::<1>()?[0] {
                0 => false,
                1 => true,
                o => bail!("WKB byte order {o}"),
            };
            let t = self.u32(le)?;
            // EWKB's flags (Z, M, SRID) and ISO's thousands (1000 Z, 2000 M, 3000 ZM).
            let (z, m) = (t & 0x8000_0000 != 0, t & 0x4000_0000 != 0);
            if t & 0x2000_0000 != 0 {
                self.u32(le)?; // the SRID
            }
            let base = t & 0x0fff_ffff;
            let (iso, b) = (base / 1000, base % 1000);
            let dims = 2 + usize::from(z || iso == 1 || iso == 3) + usize::from(m || iso == 2 || iso == 3);
            Ok((le, b, dims))
        }
        fn polygon(&mut self, le: bool, dims: usize, poly: u32, pts: &mut Vec<[f64; 2]>, rings: &mut Vec<(u32, u32)>) -> Result<()> {
            let nr = self.u32(le)?;
            for _ in 0..nr {
                let np = self.u32(le)?;
                ensure!(np as usize <= self.b.len() / 16, "WKB ring of {np} points");
                for _ in 0..np {
                    let x = self.f64(le)?;
                    let y = self.f64(le)?;
                    for _ in 2..dims {
                        self.f64(le)?;
                    }
                    pts.push([x, y]);
                }
                rings.push((poly, np));
            }
            Ok(())
        }
    }
    let mut w = W { b, at: 0 };
    let (le, t, dims) = w.head()?;
    match t {
        3 => {
            w.polygon(le, dims, 0, pts, rings)?;
            Ok(1)
        }
        6 => {
            let n = w.u32(le)?;
            for k in 0..n {
                let (le2, t2, dims2) = w.head()?;
                ensure!(t2 == 3, "a WKB MultiPolygon holding a geometry of type {t2}");
                w.polygon(le2, dims2, k, pts, rings)?;
            }
            Ok(n)
        }
        t => bail!("WKB geometry type {t} (not a polygon)"),
    }
}

/// The centroid (degrees) and area (degrees²) of polygons (`wkb_polygons`'s output): area-weighted
/// as GEOS has it, an exterior ring counting positive and a hole negative whatever their
/// orientation, in f64 relative to the first point, in the rings' order. A geometry without area
/// takes its exterior rings' mean point. None for no points.
pub fn centroid_area(pts: &[[f64; 2]], rings: &[(u32, u32)]) -> Option<([f64; 2], f64)> {
    let o = *pts.first()?;
    let (mut a, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
    let (mut mx, mut my, mut mn) = (0.0f64, 0.0f64, 0usize);
    let mut at = 0usize;
    let mut last_poly = u32::MAX;
    for &(poly, n) in rings {
        let ring = &pts[at..at + n as usize];
        at += n as usize;
        let exterior = poly != last_poly;
        last_poly = poly;
        let (mut ra, mut rx, mut ry) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..ring.len() {
            let p = [ring[i][0] - o[0], ring[i][1] - o[1]];
            let q = ring[(i + 1) % ring.len()];
            let q = [q[0] - o[0], q[1] - o[1]];
            let c = p[0] * q[1] - q[0] * p[1];
            ra += c;
            rx += (p[0] + q[0]) * c;
            ry += (p[1] + q[1]) * c;
        }
        // Exterior positive, holes negative.
        let s = if (ra >= 0.0) == exterior { 1.0 } else { -1.0 };
        a += s * ra / 2.0;
        sx += s * rx / 6.0;
        sy += s * ry / 6.0;
        if exterior {
            let k = if ring.len() > 1 && ring[0] == ring[ring.len() - 1] { ring.len() - 1 } else { ring.len() };
            for p in &ring[..k] {
                mx += p[0] - o[0];
                my += p[1] - o[1];
                mn += 1;
            }
        }
    }
    if a.abs() > 1e-20 {
        Some(([o[0] + sx / a, o[1] + sy / a], a.abs()))
    } else if mn > 0 {
        Some(([o[0] + mx / mn as f64, o[1] + my / mn as f64], 0.0))
    } else {
        None
    }
}

// ---- GHSL ------------------------------------------------------------------------------------------

/// A GHSL window: its tile's geotransform (c, a: the left edge and a pixel's width; f, e: the top and
/// a pixel's height, negative) and where it lies in the tile's pixel grid.
struct Window {
    t: [f64; 6],
    col0: i64,
    row0: i64,
    w: usize,
    h: usize,
    data: Vec<f32>,
}

/// GHSL's average building height at points, from the windows bldprep.py sent.
#[derive(Default)]
pub struct Ghsl {
    wins: Vec<Window>,
}

impl Ghsl {
    fn add(&mut self, f: &Frame) -> Result<()> {
        let h = &f.header;
        let t: Vec<f64> = h["transform"].as_array().context("GHSL window: transform")?.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect();
        ensure!(t.len() == 6 && t.iter().all(|v| v.is_finite()) && t[1] > 0.0 && t[5] < 0.0, "GHSL window: transform {t:?}");
        let (w, hh) = (h["width"].as_u64().context("width")? as usize, h["height"].as_u64().context("height")? as usize);
        let data = f.cols.get("data").context("GHSL window: data")?;
        ensure!(data.len() == w * hh * 4, "GHSL window: {} bytes for {w} × {hh}", data.len());
        self.wins.push(Window {
            t: [t[0], t[1], t[2], t[3], t[4], t[5]],
            col0: h["col_off"].as_i64().context("col_off")?,
            row0: h["row_off"].as_i64().context("row_off")?,
            w,
            h: hh,
            data: bytemuck::pod_collect_to_vec(data),
        });
        Ok(())
    }

    /// The value at an E7 point, decimetres (0: none, or no window holds it).
    pub fn at(&self, p: [i32; 2]) -> u16 {
        let (lon, lat) = (p[0] as f64 * 1e-7, p[1] as f64 * 1e-7);
        for w in &self.wins {
            let col = ((lon - w.t[0]) / w.t[1]).floor() as i64 - w.col0;
            let row = ((lat - w.t[3]) / w.t[5]).floor() as i64 - w.row0;
            if col >= 0 && row >= 0 && (col as usize) < w.w && (row as usize) < w.h {
                let v = w.data[row as usize * w.w + col as usize] as f64;
                return if v.is_finite() && v > 0.0 { (v * 10.0).round().min(65535.0) as u16 } else { 0 };
            }
        }
        0
    }
}

// ---- the records ---------------------------------------------------------------------------------

/// Strings coded as they come (the frames' own dictionaries), sorted at the end.
#[derive(Default)]
struct Strings {
    names: Vec<String>,
    by: HashMap<String, u16>,
}

impl Strings {
    /// A frame's dictionary as global codes (from 1; 0 none).
    fn map(&mut self, dict: &serde_json::Value) -> Result<Vec<u16>> {
        let mut out = Vec::new();
        for v in dict.as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let s = v.as_str().unwrap_or("").to_string();
            let n = self.names.len();
            let c = *self.by.entry(s.clone()).or_insert_with(|| n as u16 + 1);
            if c as usize == n + 1 {
                ensure!(n < 60_000, "too many distinct strings");
                self.names.push(s);
            }
            out.push(c);
        }
        Ok(out)
    }

    /// The names sorted, and each code's new code (u8: at most 255 names).
    fn sorted(&self, what: &str) -> Result<(Vec<String>, Vec<u8>)> {
        let mut order: Vec<usize> = (0..self.names.len()).collect();
        order.sort_by(|&a, &b| self.names[a].cmp(&self.names[b]));
        ensure!(order.len() <= 255, "{} distinct {what} (at most 255)", order.len());
        let mut remap = vec![0u8; self.names.len() + 1];
        for (k, &i) in order.iter().enumerate() {
            remap[i + 1] = k as u8 + 1;
        }
        Ok((order.iter().map(|&i| self.names[i].clone()).collect(), remap))
    }
}

/// One record, before it joins the rest.
struct Row {
    id: u128,
    key: u64,
    cen: [i32; 2],
    area: f32,
    nrings: Vec<u32>,
    ring_len: Vec<u32>,
    verts: Vec<[i32; 2]>,
    h: u16,
    m: u16,
    f: u8,
    mf: u8,
    class: u16,
    subtype: u16,
    roof: u16,
    flags: u8,
    hsrc: u16,
    ghsl: u16,
    osm: u64,
}

/// A z6 tile's records as read, column by column.
#[derive(Default)]
pub struct Prepped {
    id: Vec<u128>,
    key: Vec<u64>,
    cen: Vec<[i32; 2]>,
    area: Vec<f32>,
    /// Per record, its first polygon (prefix sums; one more than the records).
    poly0: Vec<u64>,
    nrings: Vec<u32>,
    /// Per polygon, its first ring (likewise).
    ring0: Vec<u64>,
    ring_len: Vec<u32>,
    /// Per ring, its first vertex (likewise).
    vert0: Vec<u64>,
    verts: Vec<[i32; 2]>,
    h: Vec<u16>,
    m: Vec<u16>,
    f: Vec<u8>,
    mf: Vec<u8>,
    class: Vec<u16>,
    subtype: Vec<u16>,
    roof: Vec<u16>,
    flags: Vec<u8>,
    hsrc: Vec<u16>,
    ghsl: Vec<u16>,
    osm: Vec<u64>,
    /// The building of every part drawn (in T or a neighbour: not underground, its geometry read).
    parents: Vec<u128>,
    classes: Strings,
    subtypes: Strings,
    roofs: Strings,
    datasets: Strings,
    pub ghsl_wins: Ghsl,
    /// What was read: bldprep.py's end frame.
    pub read: serde_json::Value,
    pub stats: Stats,
}

/// What reading found.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Stats {
    /// Rows read (buildings and parts whose box meets T).
    pub rows: u64,
    pub buildings: u64,
    pub parts: u64,
    pub underground: u64,
    /// Rows whose centroid isn't in T.
    pub outside: u64,
    /// Rows whose geometry isn't a polygon, or has no points.
    pub bad: u64,
    /// The same id read twice (the first kept).
    pub duplicates: u64,
}

/// A UUID (36 ASCII bytes with dashes, or 32 hex digits) as a number.
fn uuid(s: &[u8]) -> Option<u128> {
    let mut v = 0u128;
    let mut n = 0;
    for &c in s {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b'-' => continue,
            _ => return None,
        };
        v = (v << 4) | d as u128;
        n += 1;
    }
    (n == 32).then_some(v)
}

/// An OSM record id as Overture gives it ("w123@4", "r56@2") as [`osm`]'s number.
pub fn osm_id(s: &str) -> u64 {
    let t = match s.as_bytes().first() {
        Some(b'w') => osm::WAY,
        Some(b'r') => osm::RELATION,
        _ => return 0,
    };
    let num = s[1..].split('@').next().unwrap_or("");
    num.parse::<u64>().ok().filter(|&n| n > 0 && n <= osm::ID).map_or(0, |n| t | n)
}

fn col<'a>(f: &'a Frame, name: &str, n: usize, size: usize) -> Result<&'a [u8]> {
    let c = f.cols.get(name).with_context(|| format!("no column {name}"))?;
    ensure!(c.len() == n * size, "column {name}: {} bytes for {n} rows", c.len());
    Ok(c)
}

fn offsets(f: &Frame, name: &str, n: usize) -> Result<Vec<u64>> {
    let o: Vec<u64> = bytemuck::pod_collect_to_vec(col(f, name, n + 1, 8)?);
    ensure!(o.windows(2).all(|w| w[0] <= w[1]), "column {name}: offsets out of order");
    Ok(o)
}

impl Prepped {
    fn add_rows(&mut self, f: &Frame, t: Unit, part: bool) -> Result<()> {
        let h = &f.header;
        let n = h["n"].as_u64().context("a frame without n")? as usize;
        self.stats.rows += n as u64;
        let ids = col(f, "id", n, 36)?;
        let goff = offsets(f, "geom_off", n)?;
        let geom = f.cols.get("geom").context("no column geom")?;
        ensure!(goff.last().copied() == Some(geom.len() as u64), "column geom: its offsets don't end at its length");
        let height: Vec<f64> = bytemuck::pod_collect_to_vec(col(f, "height", n, 8)?);
        let min_height: Vec<f64> = bytemuck::pod_collect_to_vec(col(f, "min_height", n, 8)?);
        let floors: Vec<i32> = bytemuck::pod_collect_to_vec(col(f, "num_floors", n, 4)?);
        let min_floor: Vec<i32> = bytemuck::pod_collect_to_vec(col(f, "min_floor", n, 4)?);
        let under = col(f, "is_underground", n, 1)?;
        let fds: Vec<i32> = bytemuck::pod_collect_to_vec(col(f, "fds", n, 4)?);
        let hds: Vec<i32> = bytemuck::pod_collect_to_vec(col(f, "hds", n, 4)?);
        let roff = offsets(f, "rid_off", n)?;
        let rid = f.cols.get("rid").context("no column rid")?;
        let dicts = &h["dicts"];
        let ds = self.datasets.map(&dicts["dataset"])?;
        let roof_map = self.roofs.map(&dicts["roof"])?;
        let roof: Vec<i32> = bytemuck::pod_collect_to_vec(col(f, "roof", n, 4)?);
        // (Parts have no class, subtype or parts of their own.)
        let class: Vec<i32> = if part { vec![-1; n] } else { bytemuck::pod_collect_to_vec(col(f, "class", n, 4)?) };
        let subtype: Vec<i32> = if part { vec![-1; n] } else { bytemuck::pod_collect_to_vec(col(f, "subtype", n, 4)?) };
        let has_parts: &[u8] = if part { &[] } else { col(f, "has_parts", n, 1)? };
        let cmap = if part { Vec::new() } else { self.classes.map(&dicts["class"])? };
        let smap = if part { Vec::new() } else { self.subtypes.map(&dicts["subtype"])? };
        let parents: &[[u8; 36]] = if part { col(f, "parent", n, 36)?.as_chunks::<36>().0 } else { &[] };
        let osm_ds = dicts["dataset"].as_array().and_then(|a| a.iter().position(|v| v.as_str() == Some("OpenStreetMap")));
        let code = |m: &[u16], v: i32| -> u16 { if v >= 0 { m.get(v as usize).copied().unwrap_or(0) } else { 0 } };
        let dm = |v: f64| -> u16 { if v.is_finite() && v > 0.0 { (v * 10.0).round().min(65535.0) as u16 } else { 0 } };
        let fl = |v: i32| -> u8 { if v >= 1 { v.min(255) as u8 } else { 0 } };
        let ghsl = &self.ghsl_wins;
        let rows: Vec<Result<Option<Row>, u8>> = (0..n)
            .into_par_iter()
            .map(|i| {
                if under[i] != 0 {
                    return Err(1);
                }
                let id = uuid(&ids[i * 36..i * 36 + 36]).ok_or(2u8)?;
                let g = &geom[goff[i] as usize..goff[i + 1] as usize];
                let (mut pts, mut rings) = (Vec::new(), Vec::new());
                let np = wkb_polygons(g, &mut pts, &mut rings).map_err(|_| 2u8)?;
                let (c, a) = centroid_area(&pts, &rings).ok_or(2u8)?;
                let cen = [e7(c[0]), e7(c[1])];
                let key = z14_key(cen);
                let (_, x, y) = super::key_zxy(key);
                if (x >> 8, y >> 8) != (t.x, t.y) {
                    return Ok(None);
                }
                let area = (a * DEG_M * DEG_M * c[1].to_radians().dcos()) as f32;
                // Rounded to E7, each ring without its closing point.
                let mut nrings = vec![0u32; np as usize];
                let mut ring_len = Vec::with_capacity(rings.len());
                let mut verts = Vec::with_capacity(pts.len());
                let mut at = 0usize;
                for &(p, k) in &rings {
                    let r = &pts[at..at + k as usize];
                    at += k as usize;
                    let r = if r.len() > 1 && r[0] == r[r.len() - 1] { &r[..r.len() - 1] } else { r };
                    nrings[p as usize] += 1;
                    ring_len.push(r.len() as u32);
                    verts.extend(r.iter().map(|q| [e7(q[0]), e7(q[1])]));
                }
                let r0 = roff[i] as usize;
                let osm = if osm_ds.is_some_and(|o| fds[i] == o as i32) { std::str::from_utf8(&rid[r0..roff[i + 1] as usize]).map(osm_id).unwrap_or(0) } else { 0 };
                let hs = if hds[i] >= 0 { hds[i] } else { fds[i] };
                let mut flags = if part { flag::PART } else { 0 };
                if !part && has_parts[i] != 0 {
                    flags |= flag::HAS_PARTS;
                }
                Ok(Some(Row {
                    id,
                    key,
                    cen,
                    area,
                    nrings,
                    ring_len,
                    verts,
                    h: dm(height[i]),
                    m: dm(min_height[i]),
                    f: fl(floors[i]),
                    mf: fl(min_floor[i]),
                    class: code(&cmap, class[i]),
                    subtype: code(&smap, subtype[i]),
                    roof: code(&roof_map, roof[i]),
                    flags,
                    hsrc: code(&ds, hs),
                    ghsl: ghsl.at(cen),
                    osm,
                }))
            })
            .collect();
        // A building has parts when one is drawn: in T or a neighbour, not underground, its
        // geometry read (an outline whose parts are all underground is drawn itself).
        self.parents.extend(rows.iter().zip(parents).filter(|(r, _)| r.is_ok()).filter_map(|(_, c)| uuid(c)));
        for r in rows {
            match r {
                Ok(Some(r)) => self.push(r, part),
                Ok(None) => self.stats.outside += 1,
                Err(1) => self.stats.underground += 1,
                Err(_) => self.stats.bad += 1,
            }
        }
        Ok(())
    }

    fn push(&mut self, r: Row, part: bool) {
        if self.poly0.is_empty() {
            self.poly0.push(0);
            self.ring0.push(0);
            self.vert0.push(0);
        }
        if part {
            self.stats.parts += 1;
        } else {
            self.stats.buildings += 1;
        }
        self.id.push(r.id);
        self.key.push(r.key);
        self.cen.push(r.cen);
        self.area.push(r.area);
        for &k in &r.nrings {
            self.nrings.push(k);
            self.ring0.push(self.ring0.last().unwrap() + k as u64);
        }
        self.poly0.push(self.poly0.last().unwrap() + r.nrings.len() as u64);
        for &k in &r.ring_len {
            self.ring_len.push(k);
            self.vert0.push(self.vert0.last().unwrap() + k as u64);
        }
        self.verts.extend_from_slice(&r.verts);
        self.h.push(r.h);
        self.m.push(r.m);
        self.f.push(r.f);
        self.mf.push(r.mf);
        self.class.push(r.class);
        self.subtype.push(r.subtype);
        self.roof.push(r.roof);
        self.flags.push(r.flags);
        self.hsrc.push(r.hsrc);
        self.ghsl.push(r.ghsl);
        self.osm.push(r.osm);
    }

    pub fn len(&self) -> usize {
        self.id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }

    /// The records' order: by (z14 tile, id), the first read of an id kept.
    fn order(&mut self) -> Vec<u32> {
        let mut ord: Vec<u32> = (0..self.len() as u32).collect();
        ord.par_sort_unstable_by_key(|&i| (self.key[i as usize], self.id[i as usize], i));
        let before = ord.len();
        ord.dedup_by(|b, a| self.id[*a as usize] == self.id[*b as usize] && self.key[*a as usize] == self.key[*b as usize]);
        self.stats.duplicates = (before - ord.len()) as u64;
        ord
    }

    /// A block of the records `idx` (in order), its strings recoded.
    fn block(&self, idx: &[u32], remap: &[Vec<u8>; 4]) -> Block {
        let mut b = Block::default();
        for &i in idx {
            let i = i as usize;
            b.cen.push(self.cen[i]);
            b.area.push(self.area[i]);
            let (p0, p1) = (self.poly0[i] as usize, self.poly0[i + 1] as usize);
            b.npolys.push((p1 - p0) as u32);
            for p in p0..p1 {
                b.nrings.push(self.nrings[p]);
                for r in self.ring0[p] as usize..self.ring0[p + 1] as usize {
                    b.ring_len.push(self.ring_len[r]);
                    b.verts.extend_from_slice(&self.verts[self.vert0[r] as usize..self.vert0[r + 1] as usize]);
                }
            }
            b.h.push(self.h[i]);
            b.m.push(self.m[i]);
            b.f.push(self.f[i]);
            b.mf.push(self.mf[i]);
            b.class.push(remap[0][self.class[i] as usize]);
            b.subtype.push(remap[1][self.subtype[i] as usize]);
            b.roof.push(remap[2][self.roof[i] as usize]);
            b.flags.push(self.flags[i]);
            b.hsrc.push(remap[3][self.hsrc[i] as usize]);
            b.ghsl.push(self.ghsl[i]);
            b.osm.push(self.osm[i]);
        }
        b
    }

    /// The parts' buildings found: a building marked as having parts keeps the mark only when one
    /// of its parts was read.
    fn resolve_parts(&mut self) {
        self.parents.par_sort_unstable();
        self.parents.dedup();
        for i in 0..self.len() {
            if self.flags[i] & flag::HAS_PARTS != 0 && self.parents.binary_search(&self.id[i]).is_err() {
                self.flags[i] &= !flag::HAS_PARTS;
            }
        }
    }

    /// Writes the normalized file of tile `t` to `path`; its counts.
    pub fn write(mut self, t: Unit, release: &str, path: &Path) -> Result<Stats> {
        self.resolve_parts();
        let ord = self.order();
        let (classes, rc) = self.classes.sorted("classes")?;
        let (subtypes, rs) = self.subtypes.sorted("subtypes")?;
        let (roofs, rr) = self.roofs.sorted("roof shapes")?;
        let (srcs, rd) = self.datasets.sorted("datasets")?;
        let remap = [rc, rs, rr, rd];
        let (mut nb, mut np) = (0u64, 0u64);
        for &i in &ord {
            if self.flags[i as usize] & flag::PART != 0 {
                np += 1;
            } else {
                nb += 1;
            }
        }
        let meta = Meta { fmt: 1, tile: t.slash(), release: release.to_string(), buildings: nb, parts: np, srcs, classes, subtypes, roofs, ghsl: "R2023A".into(), read: self.read.clone() };
        // Blocks: the records of each z14 tile, a thousand tiles at a time compressed in parallel.
        let mut groups: Vec<(u64, std::ops::Range<usize>)> = Vec::new();
        let mut s = 0;
        for k in 1..=ord.len() {
            if k == ord.len() || self.key[ord[k] as usize] != self.key[ord[s] as usize] {
                groups.push((self.key[ord[s] as usize], s..k));
                s = k;
            }
        }
        let me = &self;
        let mut chunks = groups.chunks(1000).flat_map(|c| {
            let z: Vec<Result<(u64, Vec<u8>, u32)>> = c
                .par_iter()
                .map(|(key, r)| {
                    let b = me.block(&ord[r.clone()], &remap);
                    Ok((*key, zstd::bulk::compress(&b.encode(), work::ZSTD_LEVEL)?, r.len() as u32))
                })
                .collect();
            z
        });
        work::write(path, &meta, &mut chunks)?;
        let mut st = self.stats.clone();
        st.buildings = nb;
        st.parts = np;
        Ok(st)
    }
}

/// Reads bldprep.py's stream for tile `t` (its magic, then frames to the end frame).
pub fn read_stream(r: &mut dyn Read, t: Unit) -> Result<Prepped> {
    let mut m = [0u8; 6];
    r.read_exact(&mut m).context("bldprep.py wrote nothing")?;
    ensure!(&m == MAGIC, "not bldprep.py's stream");
    let mut p = Prepped::default();
    loop {
        let f = read_frame(r)?.context("bldprep.py's stream ended before its end frame")?;
        if p.take(&f, t)? {
            return Ok(p);
        }
    }
}

impl Prepped {
    /// Takes a frame of bldprep.py's stream: whether it was the end.
    fn take(&mut self, f: &Frame, t: Unit) -> Result<bool> {
        match f.kind {
            kind::GHSL => {
                // (The rows sample GHSL as they're read: none would have it.)
                ensure!(self.stats.rows == 0, "bldprep.py sent a GHSL window after rows");
                self.ghsl_wins.add(f)?;
            }
            kind::BUILDINGS => self.add_rows(f, t, false).with_context(|| format!("buildings of {}", f.header["src"]))?,
            kind::PARTS => self.add_rows(f, t, true).with_context(|| format!("parts of {}", f.header["src"]))?,
            kind::END => {
                self.read = f.header.clone();
                if let Some(o) = self.read.as_object_mut() {
                    o.remove("cols");
                }
                return Ok(true);
            }
            k => bail!("a frame of kind {k} from bldprep.py"),
        }
        Ok(false)
    }
}

/// Runs `bldprep T`: bldprep.py (in `dem`, through uv) for tile `t`, its stream read as it comes,
/// the normalized file written and uploaded as `work/bld/6-x-y` (or, when T has no building, the
/// one it had dropped). Its counts.
pub fn run(out: &mut Out, t: Unit, dem: &Path, release: &str) -> Result<Stats> {
    ensure!(t.z == 6, "bldprep takes z6 tiles ({} isn't one)", t.slash());
    let t0 = std::time::Instant::now();
    let mut child = std::process::Command::new("uv")
        .current_dir(dem)
        .args(["run", "python", "bldprep.py", "--root"])
        .arg(out.root())
        .args(["--tile", &t.slash(), "--release", release])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("run bldprep.py")?;
    // The stream read on its own thread, a few frames ahead, so Python reads on while records are
    // made here.
    let stdout = child.stdout.take().context("bldprep.py's stdout")?;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<Option<Frame>>>(8);
    let reader = std::thread::spawn(move || {
        let mut r = std::io::BufReader::with_capacity(8 << 20, stdout);
        let mut m = [0u8; 6];
        if let Err(e) = r.read_exact(&mut m).context("bldprep.py wrote nothing").and_then(|_| {
            ensure!(&m == MAGIC, "not bldprep.py's stream");
            Ok(())
        }) {
            tx.send(Err(e)).ok();
            return;
        }
        loop {
            let f = read_frame(&mut r);
            let end = !matches!(f, Ok(Some(ref x)) if x.kind != kind::END);
            if tx.send(f).is_err() || end {
                return;
            }
        }
    });
    let mut p = Prepped::default();
    let mut frames = 0u64;
    let read = (|| -> Result<u64> {
        loop {
            let f = rx.recv().context("bldprep.py's stream")??;
            let f = f.context("bldprep.py's stream ended before its end frame")?;
            frames += 1;
            if p.take(&f, t)? {
                return Ok(frames);
            }
            if let (kind::BUILDINGS | kind::PARTS, Some(k), Some(n)) = (f.kind, f.header["k"].as_u64(), f.header["of"].as_u64()) {
                if k % 25 == 0 || k + 1 == n {
                    crate::agent::jobs::report(k + 1, n, "row groups read");
                }
            }
        }
    })();
    let total = match read {
        Ok(n) => n,
        Err(e) => {
            // bldprep.py stopped, not left to find out at its next write: uv and the Python it
            // started, killed as a tree (`uv` alone left Python running). The reader thread ends at
            // the next frame it reads (nothing takes it).
            drop(rx);
            crate::sys::kill_tree(child.id() as i32);
            child.wait().ok();
            return Err(e.context(format!("bldprep {}", t.slash())));
        }
    };
    reader.join().ok();
    let st = child.wait().context("bldprep.py")?;
    ensure!(st.success(), "bldprep.py for {}: {st}", t.slash());
    eprintln!("bldprep {}: {} frames read in {:.0?}: {:?}", t.slash(), total, t0.elapsed(), p.stats);
    let logical = super::work_logical(t.x, t.y);
    if p.is_empty() {
        if out.get(&logical).is_some() {
            out.remove(&logical);
        }
        out.save()?;
        return Ok(p.stats);
    }
    let local = out.scratch_file(&format!("{logical}.sect"));
    let st = p.write(t, release, &local)?;
    let name = out.put_file(&logical, "sect", &local)?;
    out.save()?;
    eprintln!("bldprep {}: {} buildings, {} parts -> {name} ({:.0?})", t.slash(), st.buildings, st.parts, t0.elapsed());
    Ok(st)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian WKB polygon (rings closed).
    pub(crate) fn wkb_polygon(rings: &[&[[f64; 2]]]) -> Vec<u8> {
        let mut b = vec![1u8];
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&(rings.len() as u32).to_le_bytes());
        for r in rings {
            b.extend_from_slice(&(r.len() as u32 + 1).to_le_bytes());
            for p in r.iter().chain(std::iter::once(&r[0])) {
                b.extend_from_slice(&p[0].to_le_bytes());
                b.extend_from_slice(&p[1].to_le_bytes());
            }
        }
        b
    }

    /// A square of side `d` degrees at (lon, lat) (south-west corner).
    pub(crate) fn square(lon: f64, lat: f64, d: f64) -> Vec<[f64; 2]> {
        vec![[lon, lat], [lon + d, lat], [lon + d, lat + d], [lon, lat + d]]
    }

    /// One building for a stream: (id, rings, height, floors, class, subtype, dataset).
    pub(crate) struct B<'a> {
        pub id: u128,
        pub rings: Vec<Vec<[f64; 2]>>,
        pub height: f64,
        pub floors: i32,
        pub class: &'a str,
        pub dataset: &'a str,
        pub osm: &'a str,
        pub underground: bool,
        pub has_parts: bool,
        pub parent: u128,
    }

    impl Default for B<'_> {
        fn default() -> Self {
            B { id: 0, rings: Vec::new(), height: f64::NAN, floors: i32::MIN, class: "", dataset: "OpenStreetMap", osm: "", underground: false, has_parts: false, parent: 0 }
        }
    }

    fn id36(v: u128) -> Vec<u8> {
        let h = format!("{v:032x}");
        format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]).into_bytes()
    }

    /// A frame of buildings (or parts) as bldprep.py writes it.
    pub(crate) fn frame(w: &mut Vec<u8>, part: bool, bs: &[B]) {
        let n = bs.len();
        let mut ids = Vec::new();
        let mut parents = Vec::new();
        let (mut goff, mut geom) = (vec![0u64], Vec::new());
        let (mut roff, mut rid) = (vec![0u64], Vec::new());
        let (mut h, mut mh, mut nf, mut mf) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut cls, mut sub, mut roof, mut hp, mut ug, mut fds, mut hds) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let classes: Vec<&str> = {
            let mut v: Vec<&str> = bs.iter().map(|b| b.class).filter(|c| !c.is_empty()).collect();
            v.dedup();
            v
        };
        let datasets: Vec<&str> = {
            let mut v: Vec<&str> = bs.iter().map(|b| b.dataset).collect();
            v.sort();
            v.dedup();
            v
        };
        for b in bs {
            ids.extend(id36(b.id));
            parents.extend(id36(b.parent));
            let rings: Vec<&[[f64; 2]]> = b.rings.iter().map(Vec::as_slice).collect();
            geom.extend(wkb_polygon(&rings));
            goff.push(geom.len() as u64);
            rid.extend(b.osm.as_bytes());
            roff.push(rid.len() as u64);
            h.extend(b.height.to_le_bytes());
            mh.extend(f64::NAN.to_le_bytes());
            nf.extend(b.floors.to_le_bytes());
            mf.extend(i32::MIN.to_le_bytes());
            cls.extend(classes.iter().position(|c| *c == b.class).map_or(-1, |p| p as i32).to_le_bytes());
            sub.extend((-1i32).to_le_bytes());
            roof.extend((-1i32).to_le_bytes());
            hp.push(b.has_parts as u8);
            ug.push(b.underground as u8);
            fds.extend((datasets.iter().position(|d| *d == b.dataset).unwrap() as i32).to_le_bytes());
            hds.extend((-1i32).to_le_bytes());
        }
        let goff: Vec<u8> = bytemuck::cast_slice(&goff).to_vec();
        let roff: Vec<u8> = bytemuck::cast_slice(&roff).to_vec();
        let header = serde_json::json!({ "n": n, "src": "test", "dicts": { "class": classes, "subtype": [], "roof": [], "dataset": datasets } });
        let mut cols: Vec<(&str, &[u8])> = vec![("id", &ids), ("geom_off", &goff), ("geom", &geom), ("height", &h), ("min_height", &mh), ("num_floors", &nf), ("min_floor", &mf), ("roof", &roof), ("is_underground", &ug), ("fds", &fds), ("hds", &hds), ("rid_off", &roff), ("rid", &rid)];
        if part {
            cols.push(("parent", &parents));
        } else {
            cols.extend([("class", &cls[..]), ("subtype", &sub[..]), ("has_parts", &hp[..])]);
        }
        write_frame(w, if part { kind::PARTS } else { kind::BUILDINGS }, header, &cols).unwrap();
    }

    pub(crate) fn end(w: &mut Vec<u8>) {
        write_frame(w, kind::END, serde_json::json!({ "files": [] }), &[]).unwrap();
    }

    #[test]
    fn wkb_and_centroids() {
        // A 2 × 1 rectangle with a 0.5 × 0.5 hole at its west end.
        let outer = [[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]];
        let hole = [[0.25, 0.25], [0.25, 0.75], [0.75, 0.75], [0.75, 0.25]];
        let w = wkb_polygon(&[&outer, &hole]);
        let (mut pts, mut rings) = (Vec::new(), Vec::new());
        assert_eq!(wkb_polygons(&w, &mut pts, &mut rings).unwrap(), 1);
        assert_eq!(rings, vec![(0, 5), (0, 5)]);
        let (c, a) = centroid_area(&pts, &rings).unwrap();
        assert!((a - 1.75).abs() < 1e-12);
        // (2 × 1 at x 1.0) − (0.25 at x 0.5): x = (2 − 0.125) / 1.75.
        assert!((c[0] - 1.875 / 1.75).abs() < 1e-12 && (c[1] - 0.5).abs() < 1e-12, "{c:?}");
        // Either orientation, the same.
        let rev: Vec<[f64; 2]> = outer.iter().rev().copied().collect();
        let w2 = wkb_polygon(&[&rev, &hole]);
        let (mut p2, mut r2) = (Vec::new(), Vec::new());
        wkb_polygons(&w2, &mut p2, &mut r2).unwrap();
        let (c2, a2) = centroid_area(&p2, &r2).unwrap();
        assert!((a2 - a).abs() < 1e-12 && (c2[0] - c[0]).abs() < 1e-12);
        // A MultiPolygon of two unit squares; big-endian inside.
        let mut m = vec![1u8];
        m.extend(6u32.to_le_bytes());
        m.extend(2u32.to_le_bytes());
        m.extend(wkb_polygon(&[&square(0.0, 0.0, 1.0)]));
        let mut be = vec![0u8];
        be.extend(3u32.to_be_bytes());
        be.extend(1u32.to_be_bytes());
        be.extend(5u32.to_be_bytes());
        for p in square(3.0, 0.0, 1.0).iter().chain(std::iter::once(&[3.0, 0.0])) {
            be.extend(p[0].to_be_bytes());
            be.extend(p[1].to_be_bytes());
        }
        m.extend(be);
        let (mut p3, mut r3) = (Vec::new(), Vec::new());
        assert_eq!(wkb_polygons(&m, &mut p3, &mut r3).unwrap(), 2);
        let (c3, a3) = centroid_area(&p3, &r3).unwrap();
        assert!((a3 - 2.0).abs() < 1e-12 && (c3[0] - 2.0).abs() < 1e-12 && (c3[1] - 0.5).abs() < 1e-12);
        // Not a polygon: refused.
        let mut pt = vec![1u8];
        pt.extend(1u32.to_le_bytes());
        pt.extend(0f64.to_le_bytes());
        pt.extend(0f64.to_le_bytes());
        assert!(wkb_polygons(&pt, &mut Vec::new(), &mut Vec::new()).is_err());
        assert_eq!(osm_id("w123@4"), osm::WAY | 123);
        assert_eq!(osm_id("r5@1"), osm::RELATION | 5);
        assert_eq!(osm_id("n5@1"), 0);
        assert_eq!(uuid(b"66e3b7c0-0bfc-470b-93b0-9cd94327176c"), Some(0x66e3b7c00bfc470b93b09cd94327176c));
    }

    #[test]
    fn a_stream_makes_a_file() {
        // Tile 6/32/22 (Paris); buildings near Châtelet, one in the next tile, one underground, a
        // part and its outline, and an outline whose parts aren't there.
        let t = Unit { z: 6, x: 32, y: 22 };
        let mut s = MAGIC.to_vec();
        let b = |id: u128, lon: f64, lat: f64| B { id, rings: vec![square(lon, lat, 0.0002)], ..Default::default() };
        frame(&mut s, false, &[
            B { height: 21.5, class: "apartments", osm: "w77@3", ..b(3, 2.3470, 48.8580) },
            B { floors: 6, ..b(1, 2.3480, 48.8590) },
            b(2, 8.0, 48.8),
            B { underground: true, ..b(4, 2.35, 48.86) },
            B { has_parts: true, ..b(5, 2.3490, 48.8600) },
            B { has_parts: true, ..b(6, 2.3500, 48.8610) },
        ]);
        frame(&mut s, true, &[B { parent: 5, height: 30.0, ..b(7, 2.34905, 48.86005) }]);
        end(&mut s);
        let p = read_stream(&mut &s[..], t).unwrap();
        assert_eq!((p.stats.buildings, p.stats.parts, p.stats.underground, p.stats.outside), (4, 1, 1, 1));
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("w.sect");
        let st = p.write(t, "2026-09-23.1", &path).unwrap();
        assert_eq!((st.buildings, st.parts), (4, 1));
        let w = work::WorkFile::open(&path).unwrap();
        assert_eq!(w.meta.classes, vec!["apartments"]);
        assert_eq!(w.meta.srcs, vec!["OpenStreetMap"]);
        let mut all = Vec::new();
        for e in &w.index {
            let k = w.block(e).unwrap();
            for i in 0..k.len() {
                all.push((e.key, k.cen[i], k.h[i], k.f[i], k.flags[i], k.osm[i], k.class[i]));
            }
        }
        assert_eq!(all.len(), 5);
        let rec = |lon: f64| all.iter().find(|r| (r.1[0] - e7(lon + 0.0001)).abs() < 50).unwrap();
        assert_eq!((rec(2.3470).2, rec(2.3470).5, rec(2.3470).6), (215, osm::WAY | 77, 1));
        assert_eq!(rec(2.3480).3, 6);
        assert_eq!(rec(2.3490).4, flag::HAS_PARTS, "its part was read");
        assert_eq!(rec(2.3500).4, 0, "none of its parts were read: a building");
        assert_eq!(rec(2.34905).4, flag::PART);
        // Areas: 0.0002° squares at 48.86°N, about 22.2 × 14.6 m.
        let k = w.block(&w.index[0]).unwrap();
        assert!(k.area.iter().all(|&a| (a - 325.0).abs() < 10.0), "{:?}", k.area);
        // Sorted by (z14 tile, id), and the same bytes again.
        assert!(w.index.windows(2).all(|x| x[0].key < x[1].key));
        let p2 = read_stream(&mut &s[..], t).unwrap();
        let path2 = d.path().join("w2.sect");
        p2.write(t, "2026-09-23.1", &path2).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), std::fs::read(&path2).unwrap());
    }

    #[test]
    fn parts_drawn_and_ghsl_first() {
        let t = Unit { z: 6, x: 32, y: 22 };
        let b = |id: u128, lon: f64, lat: f64| B { id, rings: vec![square(lon, lat, 0.0002)], ..Default::default() };
        // An outline whose only part is underground: drawn itself, not as an outline with parts.
        let mut s = MAGIC.to_vec();
        frame(&mut s, false, &[B { has_parts: true, ..b(5, 2.3490, 48.8600) }]);
        frame(&mut s, true, &[B { parent: 5, underground: true, ..b(7, 2.34905, 48.86005) }]);
        end(&mut s);
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("w.sect");
        read_stream(&mut &s[..], t).unwrap().write(t, "2026-09-23.1", &path).unwrap();
        let w = work::WorkFile::open(&path).unwrap();
        let k = w.block(&w.index[0]).unwrap();
        assert_eq!((k.len(), k.flags[0]), (1, 0));
        // A GHSL window after rows: refused (the rows sample GHSL as they're read).
        let mut s = MAGIC.to_vec();
        frame(&mut s, false, &[b(1, 2.3470, 48.8580)]);
        let data = vec![0u8; 4];
        let h = serde_json::json!({ "name": "g", "transform": [2.0, 0.001, 0.0, 49.0, 0.0, -0.001], "col_off": 0, "row_off": 0, "width": 1, "height": 1 });
        write_frame(&mut s, kind::GHSL, h, &[("data", &data)]).unwrap();
        end(&mut s);
        let e = read_stream(&mut &s[..], t).err().expect("refused");
        assert!(format!("{e:#}").contains("GHSL window after rows"), "{e:#}");
    }
}
