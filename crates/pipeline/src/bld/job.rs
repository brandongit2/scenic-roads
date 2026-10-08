//! `bldtiles T` (docs/buildings3d.md §3.1): z6 tile T's buildings that touch the coverage, their
//! heights filled ([`super::fill`]), as the z12–14 tiles ([`super::tiles`]) of the hi pack
//! `layers/buildings/hi/6-x-y`, a z8 area at a time.
//!
//! - **Which:** a building or part touches the coverage when its centroid or one of its vertices
//!   is inside it (with the 1 km buffer of `osm:` outlines), as a way does (plan.md §5). Its
//!   country, which the fill's fits go by, is that of its centroid's shape (`Coverage::shape_at`:
//!   the first outline holding it, else the first buffer reaching it), else its first vertex's
//!   with one (`Shape::country`; the coverage's fits where it's unknown).
//! - **The neighbours' rule** reads every building of the area's blocks and of the blocks within
//!   620 m around it, from T's file and its 8 neighbours', in the coverage or not.
//! - **Copies:** a building reaching into tiles of the area other than its own is copied into
//!   them (`tiles`: for the flat footprints), from the area itself or from 310 m around it; the
//!   latter filled here as their own area fills them (their 300 m all read).
//! - **Zooms:** z14 every building and part; z13 those 20 m tall or more, or with a footprint of
//!   2,000 m² or more; z12 those 40 m tall or more.
//! - Pure: the pack is a function of the work files' bytes and the coverage over T (its shapes and
//!   their countries).

use super::fill::{self, src, Near, Point};
use super::tiles::{self, Feat};
use super::work::{flag, Block, IndexEntry, Meta, WorkFile};
use super::{key_zxy, tile_box_deg, world7, EQ};
use crate::coverage::Coverage;
use crate::legacy::Unit;
use crate::out::Out;
use anyhow::{ensure, Result};
use det::Det;
use rayon::prelude::*;
use std::collections::BTreeMap;

/// How far beyond an area a building is copied into the area's tiles from (its centroid), ground
/// metres: one reaching further into them isn't copied (its flat footprint cut at the edge).
const COPY_M: f64 = 310.0;
/// How far beyond an area its blocks are read, ground metres: the buildings within [`COPY_M`] are
/// filled as their own area fills them, the neighbours' rule reading 300 m around each.
const MARGIN_M: f64 = COPY_M + fill::FAR_M + 10.0;

/// What a run made.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Summary {
    /// Buildings and parts drawn (touching the coverage), and those left out (not touching it).
    pub buildings: u64,
    pub parts: u64,
    pub outside: u64,
    /// The drawn buildings (not parts) by where their height comes from (`s`, 0–5).
    pub by_src: [u64; 6],
    /// Tiles, their gzip'd bytes and features, at z12, z13 and z14.
    pub tiles: [u64; 3],
    pub bytes: [u64; 3],
    pub features: [u64; 3],
    /// The copies made for the flat footprints (in `features` too).
    pub copies: u64,
    /// The heaviest tiles (z/x/y, gzip'd bytes, features), heaviest first; the fullest likewise.
    pub heaviest: Vec<(String, u32, u32)>,
    pub fullest: Vec<(String, u32, u32)>,
    /// The pack written (its content name), if any.
    pub pack: Option<String>,
}

impl Summary {
    fn note(&mut self, z: u8, x: u32, y: u32, bytes: u32, n: u32) {
        let k = (z - super::MINZOOM) as usize;
        self.tiles[k] += 1;
        self.bytes[k] += bytes as u64;
        self.features[k] += n as u64;
        let t = (format!("{z}/{x}/{y}"), bytes, n);
        keep(&mut self.heaviest, t.clone(), 1);
        keep(&mut self.fullest, t, 2);
    }

    /// Another run's (another area's) added: the counts, and the heaviest and fullest tiles of both
    /// (each list the eight first of the two, as one run noting every tile would have them).
    pub fn merge(&mut self, o: &Summary) {
        self.buildings += o.buildings;
        self.parts += o.parts;
        self.outside += o.outside;
        self.copies += o.copies;
        for k in 0..6 {
            self.by_src[k] += o.by_src[k];
        }
        for k in 0..3 {
            self.tiles[k] += o.tiles[k];
            self.bytes[k] += o.bytes[k];
            self.features[k] += o.features[k];
        }
        for t in &o.heaviest {
            keep(&mut self.heaviest, t.clone(), 1);
        }
        for t in &o.fullest {
            keep(&mut self.fullest, t.clone(), 2);
        }
    }
}

/// A tile put in a list of the eight first, heaviest first (`by` 1: by bytes, then features) or
/// fullest (2: by features, then bytes), ties by name: an order of its own, so a list is the same
/// whatever order its tiles came in.
fn keep(list: &mut Vec<(String, u32, u32)>, t: (String, u32, u32), by: usize) {
    list.push(t);
    list.sort_by(|a, b| if by == 1 { (b.1, b.2).cmp(&(a.1, a.2)) } else { (b.2, b.1).cmp(&(a.2, a.1)) }.then(a.0.cmp(&b.0)));
    list.truncate(8);
}

/// A block's file's codes, as the rules need them.
struct Codes<'a> {
    /// The `srcs` code of Microsoft's estimates (0: none in the file).
    est: u8,
    classes: &'a [String],
    subtypes: &'a [String],
}

impl<'a> Codes<'a> {
    fn of(m: &'a Meta) -> Codes<'a> {
        let est = m.srcs.iter().position(|s| s == fill::ESTIMATES).map_or(0, |i| i as u8 + 1);
        Codes { est, classes: &m.classes, subtypes: &m.subtypes }
    }
}

/// A record's place among the coverage's shapes: its centroid's (`Coverage::shape_at`: the first
/// outline holding it, else the first buffer), else that of the first of its vertices with one.
fn shape_of(cov: &Coverage, b: &Block, i: usize) -> Option<usize> {
    cov.shape_at(b.cen[i]).or_else(|| b.verts_of(i).iter().find_map(|&p| cov.shape_at(p)))
}

/// The country of shape `s` ("" none).
fn country(cov: &Coverage, s: Option<usize>) -> &str {
    s.map_or("", |s| cov.shapes[s].country.as_str())
}

/// A z14 tile's box in E7.
pub(super) fn box14(key: u64) -> [i32; 4] {
    let (_, x, y) = key_zxy(key);
    crate::hipack::tile_bounds(14, x, y)
}

/// The blocks area `a` (a z8 tile of T) reads: its own (T's blocks in it), then those within
/// [`MARGIN_M`] around it, as (file, entry) with `files` T and its neighbours ((dy + 1) × 3 + dx + 1).
pub(super) fn area_blocks(files: &[Option<WorkFile>], t: Unit, a: (u32, u32)) -> (Vec<(usize, IndexEntry)>, usize) {
    let own: Vec<(usize, IndexEntry)> = files[4].as_ref().map_or(Vec::new(), |f| f.index.iter().filter(|e| {
        let (_, x, y) = key_zxy(e.key);
        (x >> 6, y >> 6) == a
    }).map(|e| (4, *e)).collect());
    let n_own = own.len();
    let mut out = own;
    // The area's box grown by the margin, in world units.
    let g = grown_area(a, MARGIN_M);
    let c = |v: f64| (v * 16384.0).floor().clamp(0.0, 16383.0) as u32;
    for y in c(g[1])..=c(g[3]) {
        for x in c(g[0])..=c(g[2]) {
            if (x >> 6, y >> 6) == a {
                continue;
            }
            let (dx, dy) = ((x >> 8) as i64 - t.x as i64, (y >> 8) as i64 - t.y as i64);
            if dx.abs() > 1 || dy.abs() > 1 {
                continue;
            }
            let fi = ((dy + 1) * 3 + dx + 1) as usize;
            if let Some(e) = files[fi].as_ref().and_then(|f| f.find(roadcore::archive::tile_key(14, x, y))) {
                out.push((fi, *e));
            }
        }
    }
    (out, n_own)
}

/// An area's own record, filled: (top, base, s, floors, kind, k).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Filled {
    h: u16,
    m: u16,
    s: u8,
    f: u8,
    c: u8,
    k: u8,
}

/// Where `tiles_of` gives each tile: (z, x, y, the gzip'd tile, its raw length).
pub type AddTile<'a> = dyn FnMut(u8, u32, u32, &[u8], u32) -> Result<()> + 'a;

/// T's z8 areas with buildings (T's work file's, as `area_blocks` has them), in order.
pub fn areas_of(files: &[Option<WorkFile>]) -> Vec<(u32, u32)> {
    let Some(own) = files.get(4).and_then(Option::as_ref) else { return Vec::new() };
    let mut areas: Vec<(u32, u32)> = own.index.iter().map(|e| {
        let (_, x, y) = key_zxy(e.key);
        (x >> 6, y >> 6)
    }).collect();
    areas.sort_unstable();
    areas.dedup();
    areas
}

/// Every tile of T (its work file and its neighbours' in `files`, as `area_blocks` has them), in
/// order (each z8 area's, by zoom, x, y), given to `add`.
pub fn tiles_of(files: &[Option<WorkFile>], cov: &Coverage, t: Unit, add: &mut AddTile) -> Result<Summary> {
    ensure!(files.len() == 9, "T and its 8 neighbours");
    let mut sum = Summary::default();
    let areas = areas_of(files);
    for (k, &a) in areas.iter().enumerate() {
        crate::agent::jobs::report(k as u64, areas.len() as u64, "z8 areas done");
        sum.merge(&area_tiles(files, cov, t, a, add)?);
    }
    crate::agent::jobs::report(areas.len() as u64, areas.len() as u64, "z8 areas done");
    Ok(sum)
}

/// Area `a`'s tiles (a z8 tile of T: its buildings, and copies of those around it reaching into its
/// tiles), by zoom, x, y, given to `add`: what `tiles_of` makes of it. It reads only the blocks
/// `area_blocks` names, and the coverage where they are: a task's files (`super::task::cut`) give
/// the same tiles.
pub fn area_tiles(files: &[Option<WorkFile>], cov: &Coverage, t: Unit, a: (u32, u32), add: &mut AddTile) -> Result<Summary> {
    ensure!(files.len() == 9, "T and its 8 neighbours");
    let mut sum = Summary::default();
    let codes: Vec<Option<Codes>> = files.iter().map(|f| f.as_ref().map(|f| Codes::of(&f.meta))).collect();
    let (list, n_own) = area_blocks(files, t, a);
    let blocks: Vec<Block> = list.par_iter().map(|(fi, e)| files[*fi].as_ref().unwrap().block(e)).collect::<Result<_>>()?;
    // The records filled here: the area's own, and those within COPY_M around it (copied into
    // its tiles where they reach them).
    let reach = grown_area(a, COPY_M);
    let filled_here: Vec<Vec<bool>> = blocks
        .par_iter()
        .enumerate()
        .map(|(bi, b)| {
            (0..b.len())
                .map(|i| {
                    bi < n_own || {
                        let (wx, wy) = world7(b.cen[i]);
                        wx >= reach[0] && wx <= reach[2] && wy >= reach[1] && wy <= reach[3]
                    }
                })
                .collect()
        })
        .collect();
    // Their shapes (None: not touching the coverage, or not filled here).
    let shapes: Vec<Vec<Option<usize>>> = blocks
        .par_iter()
        .zip(&list)
        .zip(&filled_here)
        .map(|((b, (_, e)), here)| {
            if !here.iter().any(|&h| h) {
                return vec![None; b.len()];
            }
            match cov.box_shape(box14(e.key)) {
                Some(s) => here.iter().map(|&h| h.then_some(s)).collect(),
                None => (0..b.len()).map(|i| if here[i] { shape_of(cov, b, i) } else { None }).collect(),
            }
        })
        .collect();
    // Rules 0–2 for every building read.
    let first: Vec<Vec<Option<(u16, u8)>>> = blocks
        .par_iter()
        .zip(&list)
        .enumerate()
        .map(|(bi, (b, (fi, _)))| {
            let est = codes[*fi].as_ref().unwrap().est;
            (0..b.len())
                .map(|i| {
                    if b.flags[i] & flag::PART != 0 {
                        return None;
                    }
                    let (h, f) = (b.h[i], b.f[i]);
                    let is_est = est != 0 && b.hsrc[i] == est;
                    let taken = (fill::H_MIN_DM..=fill::H_MAX_DM).contains(&h);
                    if taken && !is_est {
                        Some((h, src::MEASURED))
                    } else if (1..=fill::F_MAX).contains(&f) {
                        let s = if filled_here[bi][i] { shapes[bi][i] } else { shape_of(cov, b, i) };
                        Some((fill::floors_dm(f, fill::storey(country(cov, s))), src::FLOORS))
                    } else if taken {
                        Some((h, src::MICROSOFT))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .collect();
    let mut pts = Vec::new();
    for (b, hs) in blocks.iter().zip(&first) {
        for (i, h) in hs.iter().enumerate() {
            if let Some((h, _)) = h {
                let (wx, wy) = world7(b.cen[i]);
                pts.push(Point { x: wx * EQ, y: wy * EQ, area: b.area[i], h: *h });
            }
        }
    }
    let bb = tile_box_deg(8, a.0, a.1);
    let near = Near::new(pts, (bb[1].abs().max(bb[3].abs()) + 0.05).min(85.0).to_radians().dcos());
    // The records filled here, filled (a record within COPY_M of the area has every building
    // within 300 m of it read: the same as its own area makes it).
    let filled: Vec<Vec<Option<Filled>>> = blocks
        .par_iter()
        .zip(&list)
        .enumerate()
        .map(|(bi, (b, (fi, _)))| {
            let cd = codes[*fi].as_ref().unwrap();
            let (mut s1, mut s2) = (Vec::new(), Vec::new());
            (0..b.len())
                .map(|i| {
                    let sh = shapes[bi][i]?;
                    let cc = country(cov, Some(sh));
                    let subtype = Meta::name(cd.subtypes, b.subtype[i]);
                    let part = b.flags[i] & flag::PART != 0;
                    let (h, m, s) = if part {
                        fill::part(b.h[i], cd.est != 0 && b.hsrc[i] == cd.est, b.f[i], b.m[i], b.mf[i], b.area[i], cc)
                    } else {
                        let (h, s) = first[bi][i]
                            .or_else(|| {
                                let (wx, wy) = world7(b.cen[i]);
                                let cos = (b.cen[i][1] as f64 * 1e-7).to_radians().dcos();
                                near.height(wx * EQ, wy * EQ, cos, b.area[i], &mut s1, &mut s2).map(|(h, _)| (h, src::NEIGHBOURS))
                            })
                            .unwrap_or_else(|| {
                                let class = Meta::name(cd.classes, b.class[i]);
                                fill::last_rules(b.ghsl[i], b.area[i], fill::size_bin(class, subtype, b.area[i]), cc)
                            });
                        (h, 0, s)
                    };
                    let k = if part { 1 } else if b.flags[i] & flag::HAS_PARTS != 0 { 2 } else { 0 };
                    Some(Filled { h, m, s, f: if s == src::FLOORS { b.f[i] } else { 0 }, c: tiles::kind_of(subtype), k })
                })
                .collect()
        })
        .collect();
    // The area's tiles: its own records, in the tiles of their centroids; and the copies, for
    // the flat footprints, of every building filled here into the area's tiles it reaches.
    let mut by_tile: BTreeMap<(u8, u32, u32), Vec<Feat>> = BTreeMap::new();
    for (bi, (b, (_, e))) in blocks.iter().zip(&list).enumerate() {
        let (_, x, y) = key_zxy(e.key);
        for (i, fl) in filled[bi].iter().enumerate() {
            let own = bi < n_own;
            let Some(fl) = fl else {
                if own {
                    sum.outside += 1;
                }
                continue;
            };
            if own {
                if fl.k == 1 {
                    sum.parts += 1;
                } else {
                    sum.buildings += 1;
                    sum.by_src[fl.s as usize] += 1;
                }
            }
            let polys: Vec<Vec<&[[i32; 2]]>> = b.polygons(i).map(|p| p.collect()).collect();
            let feat = |copy: bool| Feat {
                polys: polys.clone(),
                cen: b.cen[i],
                order: (e.key, i as u32),
                h: fl.h,
                m: fl.m,
                s: fl.s,
                f: fl.f,
                c: fl.c,
                k: fl.k,
                copy,
            };
            for z in [14u8, 13, 12] {
                let drawn = match z {
                    14 => true,
                    13 => fl.h >= 200 || b.area[i] >= 2000.0,
                    _ => fl.h >= 400,
                };
                if !drawn {
                    continue;
                }
                let tile = (x >> (14 - z), y >> (14 - z));
                if own {
                    by_tile.entry((z, tile.0, tile.1)).or_default().push(feat(false));
                }
                if fl.k == 1 {
                    continue; // (parts aren't drawn flat)
                }
                for (tx, ty) in tiles::reached(&polys, z, tile) {
                    if (tx >> (z - 8), ty >> (z - 8)) == a {
                        by_tile.entry((z, tx, ty)).or_default().push(feat(true));
                        sum.copies += 1;
                    }
                }
            }
        }
    }
    let list: Vec<((u8, u32, u32), Vec<Feat>)> = by_tile.into_iter().collect();
    let made: Vec<Option<(Vec<u8>, u32, u32)>> = list.par_iter().map(|((z, x, y), feats)| tiles::encode(*z, *x, *y, feats)).collect::<Result<_>>()?;
    for (((z, x, y), _), m) in list.iter().zip(made) {
        if let Some((gz, raw, n)) = m {
            add(*z, *x, *y, &gz, raw)?;
            sum.note(*z, *x, *y, gz.len() as u32, n);
        }
    }
    Ok(sum)
}

/// Area `a`'s box (a z8 tile) grown by `m` ground metres (at its latitude furthest from the
/// equator), in world units: w, n, e, s (y grows southward).
fn grown_area(a: (u32, u32), m: f64) -> [f64; 4] {
    let b = tile_box_deg(8, a.0, a.1);
    let cos = b[1].abs().max(b[3].abs()).min(85.0).to_radians().dcos();
    let g = m / (EQ * cos);
    [a.0 as f64 / 256.0 - g, a.1 as f64 / 256.0 - g, (a.0 + 1) as f64 / 256.0 + g, (a.1 + 1) as f64 / 256.0 + g]
}

/// T and its 8 neighbours' work files ((dy + 1) × 3 + dx + 1), from the build's records.
pub fn work_files(out: &Out, t: Unit) -> Result<Vec<Option<WorkFile>>> {
    let mut files = Vec::with_capacity(9);
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = (t.x as i64 + dx, t.y as i64 + dy);
            if !(0..64).contains(&x) || !(0..64).contains(&y) {
                files.push(None);
                continue;
            }
            files.push(match out.get(&super::work_logical(x as u32, y as u32)) {
                Some(n) => Some(WorkFile::open(&out.path(n))?),
                None => None,
            });
        }
    }
    Ok(files)
}

/// Runs `bldtiles T`: its hi pack made and uploaded (or, with no building touching the coverage,
/// the one it had dropped). With `offload` (the build Mac's coordinator), some of its z8 areas are
/// offered to workers as tasks (`super::task::Offers`) while the others are made here, in order;
/// every area's tiles go into the pack in its turn, the same bytes wherever it was made.
pub fn build(out: &mut Out, cov: &Coverage, t: Unit, offload: Option<&crate::offload::Offload>) -> Result<Summary> {
    ensure!(t.z == 6, "bldtiles takes z6 tiles ({} isn't one)", t.slash());
    let t0 = std::time::Instant::now();
    let files = work_files(out, t)?;
    let logical = super::pack_logical(t.x, t.y);
    let local = out.scratch_file(&format!("{logical}.pack"));
    let meta = serde_json::json!({"layer": super::LAYER, "scope": "hi", "root": t.slash(), "encoding": "mvt"});
    let mut w = store::pack::PackWriter::create(&local, meta, true)?;
    let mut n = 0u64;
    let areas = areas_of(&files);
    let mut offers = super::task::Offers::new(offload, &out.scratch, t, areas.len());
    let mut sum = Summary::default();
    for (k, &a) in areas.iter().enumerate() {
        crate::agent::jobs::report(k as u64, areas.len() as u64, "z8 areas done");
        offers.top_up(&files, cov, &areas, k)?;
        let mut add = |z: u8, x: u32, y: u32, gz: &[u8], raw: u32| {
            n += 1;
            w.add(z, x, y, gz, raw)
        };
        let s = match offers.result(&files, cov, &areas, k)? {
            Some(dir) => {
                let s = super::task::take_area(&dir, Unit { z: 8, x: a.0, y: a.1 }, &mut add)?;
                std::fs::remove_dir_all(&dir).ok();
                s
            }
            None => area_tiles(&files, cov, t, a, &mut add)?,
        };
        sum.merge(&s);
    }
    crate::agent::jobs::report(areas.len() as u64, areas.len() as u64, "z8 areas done");
    w.finish()?;
    if n == 0 {
        std::fs::remove_file(&local).ok();
        if out.get(&logical).is_some() {
            out.remove(&logical);
        }
        out.save()?;
        eprintln!("bldtiles {}: no building in the coverage ({:.0?})", t.slash(), t0.elapsed());
        return Ok(sum);
    }
    sum.pack = Some(out.put_file(&logical, "pack", &local)?);
    out.save()?;
    eprintln!("bldtiles {}: {} buildings and {} parts in {} tiles ({:.0?})", t.slash(), sum.buildings, sum.parts, sum.tiles.iter().sum::<u64>(), t0.elapsed());
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;
    use crate::bld::prep::{self, tests::{end, frame, square, B}};

    fn cov(outline: &str, country: &str) -> Coverage {
        let d = tempfile::tempdir().unwrap();
        let mut c = Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec![outline.into()] }], None, d.path()).unwrap();
        for s in &mut c.shapes {
            s.country = country.into();
        }
        c
    }

    /// A work file of tile `t` from buildings, in `dir`.
    fn work(dir: &std::path::Path, t: Unit, bs: &[B]) -> WorkFile {
        let mut s = prep::MAGIC.to_vec();
        frame(&mut s, false, bs);
        end(&mut s);
        let p = prep::read_stream(&mut &s[..], t).unwrap();
        let path = dir.join(format!("{}.sect", t.dash()));
        p.write(t, "2026-09-23.1", &path).unwrap();
        WorkFile::open(&path).unwrap()
    }

    #[test]
    fn a_tile_filled_and_tiled() {
        // Near Châtelet (6/32/22): a measured tower, buildings with floors, a block of unmeasured
        // houses among measured ones (the neighbours' rule), one far off (size), and one outside
        // the coverage (a 3 km circle).
        let t = Unit { z: 6, x: 32, y: 22 };
        let d = tempfile::tempdir().unwrap();
        let (lon, lat) = (2.3470, 48.8580);
        let mut bs = vec![
            B { id: 1, rings: vec![square(lon, lat, 0.0004)], height: 45.0, ..Default::default() },
            B { id: 2, rings: vec![square(lon + 0.001, lat, 0.0002)], floors: 6, ..Default::default() },
            B { id: 3, rings: vec![square(lon - 0.02, lat - 0.02, 0.0002)], class: "house", ..Default::default() },
            B { id: 4, rings: vec![square(lon + 0.06, lat, 0.0002)], height: 9.0, ..Default::default() },
        ];
        // Ten measured houses 12 m tall around (lon + 0.003, lat), and one unmeasured among them.
        for k in 0..10 {
            bs.push(B { id: 10 + k, rings: vec![square(lon + 0.003 + 0.0003 * (k % 5) as f64, lat + 0.0003 * (k / 5) as f64, 0.0002)], height: 12.0, ..Default::default() });
        }
        bs.push(B { id: 99, rings: vec![square(lon + 0.0036, lat + 0.0006, 0.0002)], ..Default::default() });
        let mut files: Vec<Option<WorkFile>> = (0..9).map(|_| None).collect();
        files[4] = Some(work(d.path(), t, &bs));
        let c = cov("place:2.3470,48.8580,3", "FR");
        let mut got: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
        let sum = tiles_of(&files, &c, t, &mut |z, x, y, gz, raw| {
            got.push((z, x, y, gz.to_vec(), raw));
            Ok(())
        })
        .unwrap();
        assert_eq!((sum.buildings, sum.parts, sum.outside), (14, 0, 1), "{sum:?}");
        // s: two measured towers... the tower and ten houses measured, floors, neighbours, size.
        assert_eq!(sum.by_src, [11, 1, 0, 1, 0, 1]);
        // z14 tiles, z13 for the 45 m tower, z12 for it too.
        assert!(got.iter().any(|t| t.0 == 12) && got.iter().any(|t| t.0 == 13));
        let z12: Vec<_> = got.iter().filter(|t| t.0 == 12).collect();
        let tile = names::mvt::Tile::decode(&names::mvt::gunzip_if_gzip(&z12[0].3).unwrap()).unwrap();
        assert_eq!(tile.layers[0].features.len(), 1);
        // The unmeasured house: its neighbours' 12 m. The far one: France's houses, 7 m.
        let z14: Vec<names::mvt::Tile> = got.iter().filter(|t| t.0 == 14).map(|t| names::mvt::Tile::decode(&names::mvt::gunzip_if_gzip(&t.3).unwrap()).unwrap()).collect();
        let mut hs: Vec<(u64, u64)> = Vec::new();
        for tl in &z14 {
            let l = &tl.layers[0];
            for f in &l.features {
                let get = |k: &str| f.tags.chunks(2).find(|kv| l.keys[kv[0] as usize] == k).map(|kv| match l.values[kv[1] as usize] { names::mvt::Value::Uint(v) => v, _ => 0 });
                hs.push((get("h").unwrap(), get("s").unwrap()));
            }
        }
        assert!(hs.contains(&(120, 3)), "{hs:?}");
        assert!(hs.contains(&(70, 5)), "{hs:?}");
        assert!(hs.contains(&(fill::floors_dm(6, fill::storey("FR")) as u64, 1)));
        // Two houses (k 3 and 8) reach past their tile's east edge (lon 2.35107): copied.
        assert_eq!(sum.copies, 2);
        // The same bytes on one thread.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let mut again: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
        pool.install(|| tiles_of(&files, &c, t, &mut |z, x, y, gz, raw| {
            again.push((z, x, y, gz.to_vec(), raw));
            Ok(())
        }))
        .unwrap();
        assert_eq!(again, got);
    }

    /// A tile's features' (h, s, o).
    type Props = Vec<(u64, u64, u64)>;

    /// A tile's features: (h, s, o) each.
    fn props(gz: &[u8]) -> Props {
        let t = names::mvt::Tile::decode(&names::mvt::gunzip_if_gzip(gz).unwrap()).unwrap();
        let l = &t.layers[0];
        l.features
            .iter()
            .map(|f| {
                let get = |k: &str| f.tags.chunks(2).find(|kv| l.keys[kv[0] as usize] == k).map_or(0, |kv| match l.values[kv[1] as usize] { names::mvt::Value::Uint(v) => v, _ => 0 });
                (get("h"), get("s"), get("o"))
            })
            .collect()
    }

    #[test]
    fn copies_for_the_flat_footprints() {
        // In 6/32/22, a building across the line between z14 tiles 8297 and 8298 (lon 2.3291…),
        // filled by the neighbours' rule (ten measured houses around it), and one across a z8
        // area's edge (lon 2.8125, between areas 129 and 130): each in its centroid's tile, and a
        // copy in the other, the same but for o.
        let t = Unit { z: 6, x: 32, y: 22 };
        let d = tempfile::tempdir().unwrap();
        let line = crate::bld::tile_box_deg(14, 8298, 5634)[0];
        let lat = 48.86;
        let mut bs = vec![B { id: 1, rings: vec![square(line - 0.0001, lat, 0.0003)], ..Default::default() }];
        for k in 0..10 {
            bs.push(B { id: 10 + k, rings: vec![square(line + 0.0008 + 0.0003 * (k % 5) as f64, lat + 0.0003 * (k / 5) as f64, 0.0002)], height: 12.0, ..Default::default() });
        }
        let edge = crate::bld::tile_box_deg(8, 130, 88)[0];
        bs.push(B { id: 2, rings: vec![square(edge - 0.0001, lat, 0.0003)], height: 30.0, ..Default::default() });
        let mut files: Vec<Option<WorkFile>> = (0..9).map(|_| None).collect();
        files[4] = Some(work(d.path(), t, &bs));
        let c = cov("place:2.5,48.86,40", "FR");
        let mut got: BTreeMap<(u8, u32, u32), Props> = BTreeMap::new();
        let sum = tiles_of(&files, &c, t, &mut |z, x, y, gz, _| {
            got.insert((z, x, y), props(gz));
            Ok(())
        })
        .unwrap();
        // A copy of each at z14, and of the 30 m one at z13 (the other isn't drawn there).
        assert_eq!(sum.copies, 3, "{got:?}");
        let (x0, y0) = crate::bld::tile_of(crate::bld::world(line + 0.00005, lat + 0.00015), 14);
        let own = &got[&(14, x0, y0)];
        let copy = &got[&(14, x0 - 1, y0)];
        assert!(own.contains(&(120, 3, 0)), "{own:?}");
        assert_eq!(copy, &vec![(120, 3, 1)]);
        let (x1, y1) = crate::bld::tile_of(crate::bld::world(edge + 0.00005, lat + 0.00015), 14);
        assert_eq!(got[&(14, x1, y1)], vec![(300, 0, 0)]);
        assert_eq!(got[&(14, x1 - 1, y1)], vec![(300, 0, 1)], "copied across the areas' edge");
        assert_eq!(got[&(13, x1 / 2, y1 / 2)], vec![(300, 0, 0)]);
        assert_eq!(got[&(13, x1 / 2 - 1, y1 / 2)], vec![(300, 0, 1)]);
    }
}

#[cfg(test)]
#[path = "job_holdout.rs"]
mod holdout;
