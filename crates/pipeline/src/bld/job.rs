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
//!   310 m around it, from T's file and its 8 neighbours', in the coverage or not.
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

/// How far beyond an area the neighbours' rule reads, ground metres.
const MARGIN_M: f64 = 310.0;

/// What a run made.
#[derive(Clone, Debug, Default, serde::Serialize)]
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
        for (list, by) in [(&mut self.heaviest, 1usize), (&mut self.fullest, 2)] {
            list.push(t.clone());
            list.sort_by(|a, b| if by == 1 { (b.1, b.2).cmp(&(a.1, a.2)) } else { (b.2, b.1).cmp(&(a.2, a.1)) }.then(a.0.cmp(&b.0)));
            list.truncate(8);
        }
    }
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
fn box14(key: u64) -> [i32; 4] {
    let (_, x, y) = key_zxy(key);
    crate::hipack::tile_bounds(14, x, y)
}

/// The blocks area `a` (a z8 tile of T) reads: its own (T's blocks in it), then those within
/// [`MARGIN_M`] around it, as (file, entry) with `files` T and its neighbours ((dy + 1) × 3 + dx + 1).
fn area_blocks(files: &[Option<WorkFile>], t: Unit, a: (u32, u32)) -> (Vec<(usize, IndexEntry)>, usize) {
    let own: Vec<(usize, IndexEntry)> = files[4].as_ref().map_or(Vec::new(), |f| f.index.iter().filter(|e| {
        let (_, x, y) = key_zxy(e.key);
        (x >> 6, y >> 6) == a
    }).map(|e| (4, *e)).collect());
    let n_own = own.len();
    let mut out = own;
    // The area's box grown by the margin, in world units (at its latitude furthest from the equator).
    let b = tile_box_deg(8, a.0, a.1);
    let cos = b[1].abs().max(b[3].abs()).min(85.0).to_radians().dcos();
    let g = MARGIN_M / (EQ * cos);
    let (w0, w1) = (a.0 as f64 / 256.0 - g, (a.0 + 1) as f64 / 256.0 + g);
    let (h0, h1) = (a.1 as f64 / 256.0 - g, (a.1 + 1) as f64 / 256.0 + g);
    let c = |v: f64| (v * 16384.0).floor().clamp(0.0, 16383.0) as u32;
    for y in c(h0)..=c(h1) {
        for x in c(w0)..=c(w1) {
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

/// Every tile of T (its work file and its neighbours' in `files`, as `area_blocks` has them), in
/// order (each z8 area's, by zoom, x, y), given to `add`.
pub fn tiles_of(files: &[Option<WorkFile>], cov: &Coverage, t: Unit, add: &mut AddTile) -> Result<Summary> {
    ensure!(files.len() == 9, "T and its 8 neighbours");
    let mut sum = Summary::default();
    let Some(own) = files[4].as_ref() else { return Ok(sum) };
    let mut areas: Vec<(u32, u32)> = own.index.iter().map(|e| {
        let (_, x, y) = key_zxy(e.key);
        (x >> 6, y >> 6)
    }).collect();
    areas.sort_unstable();
    areas.dedup();
    let codes: Vec<Option<Codes>> = files.iter().map(|f| f.as_ref().map(|f| Codes::of(&f.meta))).collect();
    for (k, &a) in areas.iter().enumerate() {
        crate::agent::jobs::report(k as u64, areas.len() as u64, "z8 areas done");
        let (list, n_own) = area_blocks(files, t, a);
        let blocks: Vec<Block> = list.par_iter().map(|(fi, e)| files[*fi].as_ref().unwrap().block(e)).collect::<Result<_>>()?;
        // The own records' shapes (None: not touching the coverage).
        let shapes: Vec<Vec<Option<usize>>> = blocks[..n_own]
            .par_iter()
            .zip(&list[..n_own])
            .map(|(b, (_, e))| match cov.box_shape(box14(e.key)) {
                Some(s) => vec![Some(s); b.len()],
                None => (0..b.len()).map(|i| shape_of(cov, b, i)).collect(),
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
                            let s = if bi < n_own { shapes[bi][i] } else { shape_of(cov, b, i) };
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
        // The own records filled.
        let filled: Vec<Vec<Option<Filled>>> = blocks[..n_own]
            .par_iter()
            .zip(&list[..n_own])
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
        // The area's tiles.
        let mut by_tile: BTreeMap<(u8, u32, u32), Vec<Feat>> = BTreeMap::new();
        for (bi, (b, (_, e))) in blocks[..n_own].iter().zip(&list[..n_own]).enumerate() {
            let (_, x, y) = key_zxy(e.key);
            for (i, fl) in filled[bi].iter().enumerate() {
                let Some(fl) = fl else {
                    sum.outside += 1;
                    continue;
                };
                if fl.k == 1 {
                    sum.parts += 1;
                } else {
                    sum.buildings += 1;
                    sum.by_src[fl.s as usize] += 1;
                }
                let feat = || Feat {
                    polys: b.polygons(i).map(|p| p.collect()).collect(),
                    cen: b.cen[i],
                    order: (e.key, i as u32),
                    h: fl.h,
                    m: fl.m,
                    s: fl.s,
                    f: fl.f,
                    c: fl.c,
                    k: fl.k,
                };
                by_tile.entry((14, x, y)).or_default().push(feat());
                if fl.h >= 200 || b.area[i] >= 2000.0 {
                    by_tile.entry((13, x >> 1, y >> 1)).or_default().push(feat());
                }
                if fl.h >= 400 {
                    by_tile.entry((12, x >> 2, y >> 2)).or_default().push(feat());
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
    }
    crate::agent::jobs::report(areas.len() as u64, areas.len() as u64, "z8 areas done");
    Ok(sum)
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
/// the one it had dropped).
pub fn build(out: &mut Out, cov: &Coverage, t: Unit) -> Result<Summary> {
    ensure!(t.z == 6, "bldtiles takes z6 tiles ({} isn't one)", t.slash());
    let t0 = std::time::Instant::now();
    let files = work_files(out, t)?;
    let logical = super::pack_logical(t.x, t.y);
    let local = out.scratch_file(&format!("{logical}.pack"));
    let meta = serde_json::json!({"layer": super::LAYER, "scope": "hi", "root": t.slash(), "encoding": "mvt"});
    let mut w = store::pack::PackWriter::create(&local, meta, true)?;
    let mut n = 0u64;
    let mut sum = tiles_of(&files, cov, t, &mut |z, x, y, gz, raw| {
        n += 1;
        w.add(z, x, y, gz, raw)
    })?;
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
}
