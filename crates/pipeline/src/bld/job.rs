//! `bldtiles T` (docs/buildings3d.md §3.1): z6 tile T's buildings that touch the coverage, their
//! heights filled ([`super::fill`]), as the z14 tiles ([`super::tiles`]) of the hi pack
//! `layers/buildings/hi/6-x-y`, a z8 tile at a time.
//!
//! - **Which:** a building or part touches the coverage when its centroid or one of its vertices
//!   is inside it (with the 1 km buffer of `osm:` outlines), as a way does (docs/regions.md). Its
//!   country, which the fill's fits go by, is that of its centroid's shape (`Coverage::shape_at`:
//!   the first outline holding it, else the first buffer reaching it), else its first vertex's
//!   with one (`Shape::country`; the coverage's fits where it's unknown).
//! - **The neighbours' rule** reads every building of the z8 tile's blocks and of the blocks within
//!   620 m around it, from T's file and its 8 neighbours', in the coverage or not.
//! - **Copies:** a building reaching into tiles of the z8 tile other than its own is copied into
//!   them (`tiles`: for the flat footprints), from the z8 tile itself or from 310 m around it; the
//!   latter filled here as their own z8 tile fills them (their 300 m all read).
//! - **Zooms:** z14 alone, every building and part (below zoom 14 the field draws them:
//!   [`super::field`]).
//! - Pure: the pack is a function of the work files' bytes and the coverage over T (its shapes and
//!   their countries).

use super::field;
use super::fill::{self, src, Near, Point};
use super::tiles::{self, Feat};
use super::work::{flag, Block, IndexEntry, Meta, WorkFile};
use super::{key_zxy, tile_box_deg, world7, EQ};
use crate::coverage::Coverage;
use crate::legacy::TileId;
use crate::out::Out;
use anyhow::{ensure, Context, Result};
use det::Det;
use rayon::prelude::*;
use std::collections::BTreeMap;

/// How far beyond a z8 tile a building is copied into the z8 tile's tiles from (its centroid), ground
/// metres: one reaching further into them isn't copied (its flat footprint cut at the edge).
const COPY_M: f64 = 310.0;
/// How far beyond a z8 tile its blocks are read, ground metres: the buildings within [`COPY_M`] are
/// filled as their own z8 tile fills them, the neighbours' rule reading 300 m around each.
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
    /// Tiles (z14), their gzip'd bytes and features.
    pub tiles: u64,
    pub bytes: u64,
    pub features: u64,
    /// The copies made for the flat footprints (in `features` too).
    pub copies: u64,
    /// The heaviest tiles (z/x/y, gzip'd bytes, features), heaviest first; the fullest likewise.
    pub heaviest: Vec<(String, u32, u32)>,
    pub fullest: Vec<(String, u32, u32)>,
    /// The pack written (its content name), if any.
    pub pack: Option<String>,
    /// The field's tiles and their bytes at levels 9–14 (docs/buildings3d.md §4.8: `field`), and
    /// the buildings and parts in its heritage lists.
    #[serde(default)]
    pub field_tiles: [u64; 6],
    #[serde(default)]
    pub field_bytes: [u64; 6],
    #[serde(default)]
    pub heritage: u64,
    /// The field's hi pack and mid written (their content names), if any.
    #[serde(default)]
    pub field_pack: Option<String>,
    #[serde(default)]
    pub field_mid: Option<String>,
}

impl Summary {
    fn note(&mut self, z: u8, x: u32, y: u32, bytes: u32, n: u32) {
        self.tiles += 1;
        self.bytes += bytes as u64;
        self.features += n as u64;
        let t = (format!("{z}/{x}/{y}"), bytes, n);
        keep(&mut self.heaviest, t.clone(), 1);
        keep(&mut self.fullest, t, 2);
    }

    /// Another run's (another z8 tile's) added: the counts, and the heaviest and fullest tiles of both
    /// (each list the eight first of the two, as one run noting every tile would have them).
    pub fn merge(&mut self, o: &Summary) {
        self.buildings += o.buildings;
        self.parts += o.parts;
        self.outside += o.outside;
        self.copies += o.copies;
        self.tiles += o.tiles;
        self.bytes += o.bytes;
        self.features += o.features;
        for k in 0..6 {
            self.by_src[k] += o.by_src[k];
        }
        for k in 0..6 {
            self.field_tiles[k] += o.field_tiles[k];
            self.field_bytes[k] += o.field_bytes[k];
        }
        self.heritage += o.heritage;
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
    list.sort_by(|a, b| {
        if by == 1 { (b.1, b.2).cmp(&(a.1, a.2)) } else { (b.2, b.1).cmp(&(a.2, a.1)) }.then(a.0.cmp(&b.0))
    });
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

/// The blocks z8 tile `a` (a z8 tile of T) reads: its own (T's blocks in it), then those within
/// [`MARGIN_M`] around it, as (file, entry) with `files` T and its neighbours ((dy + 1) × 3 + dx + 1).
pub(super) fn z8_blocks(files: &[Option<WorkFile>], t: TileId, a: (u32, u32)) -> (Vec<(usize, IndexEntry)>, usize) {
    let own: Vec<(usize, IndexEntry)> = files[4].as_ref().map_or(Vec::new(), |f| {
        f.index
            .iter()
            .filter(|e| {
                let (_, x, y) = key_zxy(e.key);
                (x >> 6, y >> 6) == a
            })
            .map(|e| (4, *e))
            .collect()
    });
    let n_own = own.len();
    let mut out = own;
    // The z8 tile's box grown by the margin, in world units.
    let g = grown_z8(a, MARGIN_M);
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

/// A z8 tile's own record, filled: (top, base, s, floors, kind, k).
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

/// T's z8 tiles with buildings (T's work file's, as `z8_blocks` has them), in order.
pub fn z8_tiles_of(files: &[Option<WorkFile>]) -> Vec<(u32, u32)> {
    let Some(own) = files.get(4).and_then(Option::as_ref) else { return Vec::new() };
    let mut z8s: Vec<(u32, u32)> = own
        .index
        .iter()
        .map(|e| {
            let (_, x, y) = key_zxy(e.key);
            (x >> 6, y >> 6)
        })
        .collect();
    z8s.sort_unstable();
    z8s.dedup();
    z8s
}

/// Every tile of T (its work file and its neighbours' in `files`, as `z8_blocks` has them), in
/// order (each z8 tile's, by zoom, x, y), given to `add` (the field's left out).
pub fn tiles_of(
    files: &[Option<WorkFile>],
    cov: &Coverage,
    t: TileId,
    sites: &[[i32; 2]],
    add: &mut AddTile,
) -> Result<Summary> {
    ensure!(files.len() == 9, "T and its 8 neighbours");
    let mut sum = Summary::default();
    let z8s = z8_tiles_of(files);
    for (k, &a) in z8s.iter().enumerate() {
        crate::agent::jobs::report(k as u64, z8s.len() as u64, "z8 tiles done");
        sum.merge(&z8_tiles(files, cov, t, a, sites, add)?.0);
    }
    crate::agent::jobs::report(z8s.len() as u64, z8s.len() as u64, "z8 tiles done");
    Ok(sum)
}

/// Area `a`'s tiles (a z8 tile of T: its buildings, and copies of those around it reaching into its
/// tiles), by zoom, x, y, given to `add`: what `tiles_of` makes of it; and its field (`field::z8 tile`:
/// every building and part extruded that meets the z8 tile, its own and those filled here around it,
/// its mid carrying the heritage list of its own that hold one of `sites`, E7). It reads
/// only the blocks `z8_blocks` names, and the coverage where they are: a task's files
/// (`super::task::cut`) give the same tiles.
pub fn z8_tiles(
    files: &[Option<WorkFile>],
    cov: &Coverage,
    t: TileId,
    a: (u32, u32),
    sites: &[[i32; 2]],
    add: &mut AddTile,
) -> Result<(Summary, field::Z8Field)> {
    ensure!(files.len() == 9, "T and its 8 neighbours");
    let mut sum = Summary::default();
    let codes: Vec<Option<Codes>> = files.iter().map(|f| f.as_ref().map(|f| Codes::of(&f.meta))).collect();
    let (list, n_own) = z8_blocks(files, t, a);
    let blocks: Vec<Block> =
        list.par_iter().map(|(fi, e)| files[*fi].as_ref().unwrap().block(e)).collect::<Result<_>>()?;
    // The records filled here: the z8 tile's own, and those within COPY_M around it (copied into
    // its tiles where they reach them).
    let reach = grown_z8(a, COPY_M);
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
    // The records filled here, filled (a record within COPY_M of the z8 tile has every building
    // within 300 m of it read: the same as its own z8 tile makes it).
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
                                near.height(wx * EQ, wy * EQ, cos, b.area[i], &mut s1, &mut s2)
                                    .map(|(h, _)| (h, src::NEIGHBOURS))
                            })
                            .unwrap_or_else(|| {
                                let class = Meta::name(cd.classes, b.class[i]);
                                fill::last_rules(b.ghsl[i], b.area[i], fill::size_bin(class, subtype, b.area[i]), cc)
                            });
                        (h, 0, s)
                    };
                    let k = if part {
                        1
                    } else if b.flags[i] & flag::HAS_PARTS != 0 {
                        2
                    } else {
                        0
                    };
                    Some(Filled {
                        h,
                        m,
                        s,
                        f: if s == src::FLOORS { b.f[i] } else { 0 },
                        c: tiles::kind_of(subtype),
                        k,
                    })
                })
                .collect()
        })
        .collect();
    // The z8 tile's tiles: its own records, in the tiles of their centroids; and the copies, for
    // the flat footprints, of every building filled here into the z8 tile's tiles it reaches.
    let mut by_tile: BTreeMap<(u8, u32, u32), Vec<Feat>> = BTreeMap::new();
    // The field's footprints (every one extruded: not an outline with parts), and the z8 tile's own.
    let mut feet: Vec<field::Foot> = Vec::new();
    let mut own_feet: Vec<usize> = Vec::new();
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
            if fl.k != 2 {
                if own {
                    own_feet.push(feet.len());
                }
                feet.push(field::Foot { polys: polys.clone(), h: fl.h, m: fl.m });
            }
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
            let z = super::ZOOM;
            if own {
                by_tile.entry((z, x, y)).or_default().push(feat(false));
            }
            // (Parts aren't drawn flat.)
            if fl.k != 1 {
                for (tx, ty) in tiles::reached(&polys, z, (x, y)) {
                    if (tx >> (z - 8), ty >> (z - 8)) == a {
                        by_tile.entry((z, tx, ty)).or_default().push(feat(true));
                        sum.copies += 1;
                    }
                }
            }
        }
    }
    let list: Vec<((u8, u32, u32), Vec<Feat>)> = by_tile.into_iter().collect();
    let made: Vec<Option<(Vec<u8>, u32, u32)>> =
        list.par_iter().map(|((z, x, y), feats)| tiles::encode(*z, *x, *y, feats)).collect::<Result<_>>()?;
    for (((z, x, y), _), m) in list.iter().zip(made) {
        if let Some((gz, raw, n)) = m {
            add(*z, *x, *y, &gz, raw)?;
            sum.note(*z, *x, *y, gz.len() as u32, n);
        }
    }
    drop(list);
    let her = {
        let own: Vec<field::Foot> = own_feet
            .iter()
            .map(|&i| field::Foot { polys: feet[i].polys.clone(), h: feet[i].h, m: feet[i].m })
            .collect();
        field::heritage(a, &own, sites)
    };
    if her.len() >= 4 {
        sum.heritage = u32::from_le_bytes(her[..4].try_into().unwrap()) as u64;
    }
    let f = field::z8(a, &feet, &her);
    for (z, _, _, b) in &f.tiles {
        let k = (*z - field::LEVEL_HI_MIN) as usize;
        sum.field_tiles[k] += 1;
        sum.field_bytes[k] += b.len() as u64;
    }
    Ok((sum, f))
}

/// Area `a`'s box (a z8 tile) grown by `m` ground metres (at its latitude furthest from the
/// equator), in world units: w, n, e, s (y grows southward).
fn grown_z8(a: (u32, u32), m: f64) -> [f64; 4] {
    let b = tile_box_deg(8, a.0, a.1);
    let cos = b[1].abs().max(b[3].abs()).min(85.0).to_radians().dcos();
    let g = m / (EQ * cos);
    [a.0 as f64 / 256.0 - g, a.1 as f64 / 256.0 - g, (a.0 + 1) as f64 / 256.0 + g, (a.1 + 1) as f64 / 256.0 + g]
}

/// T and its 8 neighbours' work files ((dy + 1) × 3 + dx + 1), from the build's records.
pub fn work_files(out: &Out, t: TileId) -> Result<Vec<Option<WorkFile>>> {
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

/// Runs `bldfield-lo Q` (a z3 tile): its field's levels 8–4 from its z6 tiles' mids
/// (`field::lo`), as the lo pack `layers/buildings-field/lo/3-x-y`, or that dropped with no mid
/// there. Its tiles.
pub fn build_lo(out: &mut Out, q: TileId) -> Result<usize> {
    build_lo_keeping(out, q, crate::resume_log::on())
}

/// `build_lo`, its z6 tiles' levels kept as pieces with `keep` (crate::resume_log::Pieces).
pub fn build_lo_keeping(out: &mut Out, q: TileId, keep: bool) -> Result<usize> {
    ensure!(q.z == 3, "bldfield-lo takes z3 tiles ({} isn't one)", q.slash());
    use crate::timings::phase;
    let t0 = std::time::Instant::now();
    let mut paths: BTreeMap<(u32, u32), store::nasfs::NasPath> = BTreeMap::new();
    for y in q.y * 8..q.y * 8 + 8 {
        for x in q.x * 8..q.x * 8 + 8 {
            if let Some(c) = out.get(&field::mid_logical(x, y)) {
                paths.insert((x, y), out.path(c));
            }
        }
    }
    let p = phase("reading the mids and making the levels");
    let names = paths.keys().map(|&(x, y)| field::mid_logical(x, y));
    let id = crate::resume_log::job_id(
        "bldfield-lo",
        &field::LO_V.to_string(),
        &q.slash(),
        &[&crate::resume_log::names(out, names)],
    );
    let mut pieces = crate::resume_log::Pieces::open(&out.scratch, (3, q.x, q.y), "bldfield-lo", &id, keep);
    let tiles = field::lo(
        (q.x, q.y),
        &|x, y| paths.get(&(x, y)).map(|p| p.read().with_context(|| format!("read {}", p.display()))).transpose(),
        &mut pieces,
    )?;
    drop(p);
    let logical = field::lo_logical(q.x, q.y);
    if tiles.is_empty() {
        if out.get(&logical).is_some() {
            out.remove(&logical);
        }
        out.save()?;
        pieces.finish();
        eprintln!("bldfield-lo {}: no mid; nothing made", q.slash());
        return Ok(0);
    }
    let local = out.scratch_file(&format!("{logical}.pack"));
    let meta =
        serde_json::json!({"layer": field::LAYER, "scope": "lo", "root": q.slash(), "encoding": field::ENCODING});
    let mut w = store::pack::PackWriter::create(&local, meta, false)?;
    let mut bytes = 0usize;
    for (z, x, y, b) in &tiles {
        w.add(*z, *x, *y, b, b.len() as u32)?;
        bytes += b.len();
    }
    w.finish()?;
    let p = phase("uploading");
    out.put_file(&logical, "pack", &local)?;
    drop(p);
    out.save()?;
    pieces.finish();
    eprintln!(
        "bldfield-lo {}: {} mids, {} tiles ({:.1} MB) ({:.0?})",
        q.slash(),
        paths.len(),
        tiles.len(),
        bytes as f64 / 1e6,
        t0.elapsed()
    );
    Ok(tiles.len())
}

/// T's heritage sites for the field's heritage lists: the newest pass's slice of T's sites
/// (`crate::heritage::pos_logical`) less its World Heritage components (`parts_logical`), as the
/// map's dots are; none without a slice.
/// What the manifest names of T's work files and its neighbours' (`work_files`'), for a run's
/// identity.
fn neighbours_names(out: &Out, t: TileId) -> String {
    let mut ls = Vec::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = (t.x as i64 + dx, t.y as i64 + dy);
            if (0..64).contains(&x) && (0..64).contains(&y) {
                ls.push(super::work_logical(x as u32, y as u32));
            }
        }
    }
    crate::resume_log::names(out, ls)
}

/// A z8 tile's results as a piece kept (crate::resume_log::Pieces): its tiles, summary, field
/// tiles and mid.
fn z8_piece(a: (u32, u32), tiles: &[(u8, u32, u32, Vec<u8>, u32)], s: &Summary, f: &field::Z8Field) -> Result<Vec<u8>> {
    let mut p = crate::resume_log::Put::default();
    p.u64(tiles.len() as u64);
    for (z, x, y, gz, raw) in tiles {
        p.u8(*z);
        p.u32(*x);
        p.u32(*y);
        p.bytes(gz);
        p.u32(*raw);
    }
    p.bytes(&serde_json::to_vec(s)?);
    p.u64(f.tiles.len() as u64);
    for (z, x, y, b) in &f.tiles {
        p.u8(*z);
        p.u32(*x);
        p.u32(*y);
        p.bytes(b);
    }
    match &f.mid {
        Some(m) => p.bytes(&field::write_mid(&BTreeMap::from([(a, m.clone())]))?),
        None => p.bytes(&[]),
    }
    Ok(p.0)
}

/// `z8_piece`'s results read back.
type Z8Piece = (Vec<(u8, u32, u32, Vec<u8>, u32)>, Summary, field::Z8Field);

fn z8_unpiece(a: (u32, u32), p: &[u8]) -> Result<Z8Piece> {
    let mut g = crate::resume_log::Get(p);
    let mut tiles = Vec::new();
    for _ in 0..g.count(21)? {
        tiles.push((g.u8()?, g.u32()?, g.u32()?, g.bytes()?, g.u32()?));
    }
    let s: Summary = serde_json::from_slice(&g.bytes()?)?;
    let mut f = field::Z8Field::default();
    for _ in 0..g.count(17)? {
        f.tiles.push((g.u8()?, g.u32()?, g.u32()?, g.bytes()?));
    }
    let m = g.bytes()?;
    if !m.is_empty() {
        f.mid = field::read_mid(&m)?.remove(&a);
    }
    g.end()?;
    Ok((tiles, s, f))
}

pub fn sites(out: &Out, t: TileId) -> Result<Vec<[i32; 2]>> {
    let Some(d) = crate::heritage::latest_date(&out.manifest) else { return Ok(Vec::new()) };
    let read = |l: String| -> Result<Vec<[i32; 2]>> {
        match out.get(&l) {
            Some(c) => Ok(serde_json::from_slice(&out.path(c).read()?).with_context(|| format!("{l}"))?),
            None => Ok(Vec::new()),
        }
    };
    let parts: std::collections::BTreeSet<[i32; 2]> =
        read(crate::heritage::parts_logical(&d, t.x, t.y))?.into_iter().collect();
    let mut s: Vec<[i32; 2]> =
        read(crate::heritage::pos_logical(&d, t.x, t.y))?.into_iter().filter(|p| !parts.contains(p)).collect();
    s.sort_unstable();
    s.dedup();
    Ok(s)
}

/// Runs `bldtiles T`: its hi pack, its field's hi pack and its field's mid made and uploaded (or,
/// with no building touching the coverage, those it had dropped). With `offload` (the lead's coordinator), some of its z8 tiles are
/// offered to workers as tasks (`super::task::Offers`) while the others are made here, in order;
/// every z8 tile's tiles go into the pack in its turn, the same bytes wherever it was made.
pub fn build(out: &mut Out, cov: &Coverage, t: TileId, offload: Option<&crate::offload::Offload>) -> Result<Summary> {
    let sum = build_later(out, cov, t, offload)?;
    out.save()?;
    out.claim_into_costin();
    Ok(sum)
}

/// `build`, its uploads put off (crate::out::Out::put_file_later): they go on beside what the job
/// does next, and the tile is saved (and noted done) by a later save (`Out::save_upto` the mark
/// after it). Its summary names the files as they'll land (their content names, made here).
pub fn build_later(
    out: &mut Out,
    cov: &Coverage,
    t: TileId,
    offload: Option<&crate::offload::Offload>,
) -> Result<Summary> {
    build_later_keeping(out, cov, t, offload, crate::resume_log::on())
}

/// `build_later`, its z8 tiles kept as pieces with `keep` (crate::resume_log::Pieces).
pub fn build_later_keeping(
    out: &mut Out,
    cov: &Coverage,
    t: TileId,
    offload: Option<&crate::offload::Offload>,
    keep: bool,
) -> Result<Summary> {
    ensure!(t.z == 6, "bldtiles takes z6 tiles ({} isn't one)", t.slash());
    use crate::timings::phase;
    let t0 = std::time::Instant::now();
    let p = phase("reading the normalized files");
    let files = work_files(out, t)?;
    drop(p);
    let logical = super::pack_logical(t.x, t.y);
    let local = out.scratch_file(&format!("{logical}.pack"));
    let meta = serde_json::json!({"layer": super::LAYER, "scope": "hi", "root": t.slash(), "encoding": "mvt"});
    let mut w = store::pack::PackWriter::create(&local, meta, true)?;
    let mut n = 0u64;
    // The field's hi pack (levels 14–9), and its z8 tiles' level-8 planes and heritage lists for the mid.
    let field_logical = field::hi_logical(t.x, t.y);
    let field_local = out.scratch_file(&format!("{field_logical}.pack"));
    let fmeta =
        serde_json::json!({"layer": field::LAYER, "scope": "hi", "root": t.slash(), "encoding": field::ENCODING});
    let mut fw = store::pack::PackWriter::create(&field_local, fmeta, false)?;
    let mut mids: BTreeMap<(u32, u32), field::Mid> = BTreeMap::new();
    let sites = sites(out, t)?;
    let z8s = z8_tiles_of(&files);
    // (Each z8 tile's results a piece kept, task #889: a resumed run makes, or offers, only the z8
    // tiles its last didn't keep.)
    let mut pieces = crate::resume_log::Pieces::open(
        &out.scratch,
        (6, t.x, t.y),
        "bldtiles",
        &crate::resume_log::job_id(
            "bldtiles",
            &super::BUILDINGS_V.to_string(),
            &t.slash(),
            &[&neighbours_names(out, t), &cov.digest(), &serde_json::to_string(&sites)?],
        ),
        keep,
    );
    let todo: Vec<(u32, u32)> = z8s.iter().copied().filter(|a| pieces.kept((8, a.0, a.1)).is_none()).collect();
    let mut offers = super::task::Offers::new(offload, &out.scratch, t, todo.len(), &sites);
    let mut sum = Summary::default();
    let mut j = 0;
    for (k, &a) in z8s.iter().enumerate() {
        crate::agent::jobs::report(k as u64, z8s.len() as u64, "z8 tiles done");
        let made = (todo.get(j) == Some(&a)).then(|| {
            j += 1;
            j - 1
        });
        let p = pieces.piece((8, a.0, a.1), || {
            let j = made.context("a z8 tile neither kept nor to make")?;
            {
                let _p = phase("offering z8 tiles to other workers");
                offers.top_up(&files, cov, &todo, j)?;
            }
            let mut tiles = Vec::new();
            let mut add = |z: u8, x: u32, y: u32, gz: &[u8], raw: u32| {
                tiles.push((z, x, y, gz.to_vec(), raw));
                Ok(())
            };
            let (s, f) = match offers.result(&files, cov, &todo, j)? {
                Some(dir) => {
                    let _p = phase("taking in another worker's z8 tile");
                    let s = super::task::take_z8(&dir, TileId { z: 8, x: a.0, y: a.1 }, &mut add)?;
                    std::fs::remove_dir_all(&dir).ok();
                    s
                }
                None => {
                    let _p = phase("making the z8 tile's slope tiles here");
                    let here = std::time::Instant::now();
                    let (s, f) = z8_tiles(&files, cov, t, a, &sites, &mut add)?;
                    offers.made_here(s.buildings + s.parts + s.outside, here.elapsed().as_secs_f64());
                    (s, f)
                }
            };
            z8_piece(a, &tiles, &s, &f)
        })?;
        let (tiles, s, f) = z8_unpiece(a, &p)?;
        for (z, x, y, gz, raw) in &tiles {
            n += 1;
            w.add(*z, *x, *y, gz, *raw)?;
        }
        sum.merge(&s);
        for (z, x, y, b) in &f.tiles {
            fw.add(*z, *x, *y, b, b.len() as u32)?;
        }
        if let Some(m) = f.mid {
            mids.insert(a, m);
        }
    }
    crate::agent::jobs::report(z8s.len() as u64, z8s.len() as u64, "z8 tiles done");
    w.finish()?;
    fw.finish()?;
    pieces.finish();
    // The field's hi pack and mid, and the meshes': each uploaded, or dropped with nothing in it.
    let mid_logical = field::mid_logical(t.x, t.y);
    let mid_local = out.scratch_file(&format!("{mid_logical}.zst"));
    let has_field = !mids.is_empty();
    if has_field {
        std::fs::write(&mid_local, field::write_mid(&mids)?)?;
    }
    drop(mids);
    for (k, (logical, local, ext, keep)) in [
        (&logical, &local, "pack", n > 0),
        (&field_logical, &field_local, "pack", has_field),
        (&mid_logical, &mid_local, "zst", has_field),
    ]
    .into_iter()
    .enumerate()
    {
        if keep {
            let name = Some(store::naming::content_name(logical, &store::naming::hash16_file(local)?, ext));
            out.put_file_later(logical, ext, local)?;
            match k {
                0 => sum.pack = name,
                1 => sum.field_pack = name,
                _ => sum.field_mid = name,
            }
        } else {
            std::fs::remove_file(local).ok();
            if out.get(logical).is_some() {
                out.remove(logical);
            }
        }
    }
    if n == 0 {
        eprintln!("bldtiles {}: no building in the coverage ({:.0?})", t.slash(), t0.elapsed());
        return Ok(sum);
    }
    eprintln!(
        "bldtiles {}: {} buildings and {} parts in {} tiles, the field's {} tiles ({:.1} MB) ({:.0?})",
        t.slash(),
        sum.buildings,
        sum.parts,
        sum.tiles,
        sum.field_tiles.iter().sum::<u64>(),
        sum.field_bytes.iter().sum::<u64>() as f64 / 1e6,
        t0.elapsed()
    );
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;
    use crate::bld::prep::{
        self,
        tests::{end, frame, square, B},
    };

    fn cov(outline: &str, country: &str) -> Coverage {
        let d = tempfile::tempdir().unwrap();
        let mut c = Coverage::fixture(
            &[Recipe { id: "r".into(), name: "R".into(), outline: vec![outline.into()] }],
            None,
            d.path(),
        )
        .unwrap();
        for s in &mut c.shapes {
            s.country = country.into();
        }
        c
    }

    /// A work file of tile `t` from buildings, in `dir`.
    fn work(dir: &std::path::Path, t: TileId, bs: &[B]) -> WorkFile {
        let mut s = Vec::new();
        frame(&mut s, false, bs);
        end(&mut s);
        let p = prep::Prepped::from_frames(s, t).unwrap();
        let path = dir.join(format!("{}.sect", t.dash()));
        p.write(t, "2026-09-23.1", &path).unwrap();
        WorkFile::open(&path).unwrap()
    }

    /// Task #889: a z6 tile's buildings paused at every one of their safe points (each z8 tile
    /// kept), resumed each time, upload the same packs and mid as a run never paused; and its z3
    /// tile's field likewise (each z6 tile's levels kept).
    #[test]
    fn bldtiles_and_bldfield_lo_paused_at_every_safe_point_resume_to_the_same_bytes() {
        use crate::resume_log::STOP_AFTER;
        let t = TileId { z: 6, x: 32, y: 22 };
        let d = tempfile::tempdir().unwrap();
        let (lon, lat) = (2.3470, 48.8580);
        // Buildings in three z8 tiles of 6/32/22 (each z8 tile is ~1.4° wide).
        let bs: Vec<B> = [(0.0, 0.0), (1.5, 0.0), (0.0, -1.0)]
            .iter()
            .enumerate()
            .map(|(k, (dx, dy))| B {
                id: 1 + k as u128,
                rings: vec![square(lon + dx, lat + dy, 0.0004)],
                height: 20.0 + k as f64,
                ..Default::default()
            })
            .collect();
        let wf = d.path().join("w");
        std::fs::create_dir_all(&wf).unwrap();
        work(&wf, t, &bs);
        let c = cov("place:3.0,48.3,150", "FR");
        let open = |name: &str| {
            let mut o = Out::unbound(
                &store::nasfs::NasRoot::local_for_test(&d.path().join(name)),
                &d.path().join(format!("{name}-s")),
            )
            .unwrap();
            // (A copy each: put_file takes its file.)
            let f = d.path().join(format!("{name}.sect"));
            std::fs::copy(wf.join(format!("{}.sect", t.dash())), &f).unwrap();
            o.put_file(&super::super::work_logical(t.x, t.y), "sect", &f).unwrap();
            o
        };
        let outputs = |out: &Out, ls: &[String]| -> Vec<Option<Vec<u8>>> {
            ls.iter().map(|l| out.get(l).map(|c| out.path(c).read().unwrap())).collect()
        };
        let hi = [super::super::pack_logical(t.x, t.y), field::hi_logical(t.x, t.y), field::mid_logical(t.x, t.y)];
        let lo = [field::lo_logical(4, 2)];
        let q = TileId { z: 3, x: 4, y: 2 };
        let mut fresh = open("fresh");
        build_later_keeping(&mut fresh, &c, t, None, false).unwrap();
        fresh.save().unwrap();
        build_lo_keeping(&mut fresh, q, false).unwrap();
        let mut out = open("paused");
        let mut stops = 0;
        loop {
            STOP_AFTER.set(Some(1));
            let r = build_later_keeping(&mut out, &c, t, None, true);
            STOP_AFTER.set(None);
            match r {
                Err(e) => {
                    assert!(crate::control::is_paused(&e), "{e:#}");
                    stops += 1;
                }
                Ok(_) => break,
            }
        }
        out.save().unwrap();
        assert!(stops >= 3, "a z8 tile a safe point: {stops}");
        assert!(outputs(&out, &hi) == outputs(&fresh, &hi), "paused at each of its {stops} safe points");
        assert!(outputs(&out, &hi).iter().all(Option::is_some));
        let mut stops = 0;
        loop {
            STOP_AFTER.set(Some(1));
            let r = build_lo_keeping(&mut out, q, true);
            STOP_AFTER.set(None);
            match r {
                Err(e) => {
                    assert!(crate::control::is_paused(&e), "{e:#}");
                    stops += 1;
                }
                Ok(_) => break,
            }
        }
        assert!(stops >= 1, "a z6 tile a safe point: {stops}");
        assert!(outputs(&out, &lo) == outputs(&fresh, &lo) && outputs(&out, &lo)[0].is_some());
    }

    #[test]
    fn a_tile_filled_and_tiled() {
        // Near Châtelet (6/32/22): a measured tower, buildings with floors, a block of unmeasured
        // houses among measured ones (the neighbours' rule), one far off (size), and one outside
        // the coverage (a 3 km circle).
        let t = TileId { z: 6, x: 32, y: 22 };
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
            bs.push(B {
                id: 10 + k,
                rings: vec![square(lon + 0.003 + 0.0003 * (k % 5) as f64, lat + 0.0003 * (k / 5) as f64, 0.0002)],
                height: 12.0,
                ..Default::default()
            });
        }
        bs.push(B { id: 99, rings: vec![square(lon + 0.0036, lat + 0.0006, 0.0002)], ..Default::default() });
        let mut files: Vec<Option<WorkFile>> = (0..9).map(|_| None).collect();
        files[4] = Some(work(d.path(), t, &bs));
        let c = cov("place:2.3470,48.8580,3", "FR");
        let mut got: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
        let sum = tiles_of(&files, &c, t, &[], &mut |z, x, y, gz, raw| {
            got.push((z, x, y, gz.to_vec(), raw));
            Ok(())
        })
        .unwrap();
        assert_eq!((sum.buildings, sum.parts, sum.outside), (14, 0, 1), "{sum:?}");
        // s: two measured towers... the tower and ten houses measured, floors, neighbours, size.
        assert_eq!(sum.by_src, [11, 1, 0, 1, 0, 1]);
        // Z14 tiles alone, the 45 m tower's too.
        assert!(got.iter().all(|t| t.0 == 14));
        // The unmeasured house: its neighbours' 12 m. The far one: France's houses, 7 m.
        let z14: Vec<names::mvt::Tile> = got
            .iter()
            .filter(|t| t.0 == 14)
            .map(|t| names::mvt::Tile::decode(&names::mvt::gunzip_if_gzip(&t.3).unwrap()).unwrap())
            .collect();
        let mut hs: Vec<(u64, u64)> = Vec::new();
        for tl in &z14 {
            let l = &tl.layers[0];
            for f in &l.features {
                let get = |k: &str| {
                    f.tags.chunks(2).find(|kv| l.keys[kv[0] as usize] == k).map(|kv| match l.values[kv[1] as usize] {
                        names::mvt::Value::Uint(v) => v,
                        _ => 0,
                    })
                };
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
        pool.install(|| {
            tiles_of(&files, &c, t, &[], &mut |z, x, y, gz, raw| {
                again.push((z, x, y, gz.to_vec(), raw));
                Ok(())
            })
        })
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
                let get = |k: &str| {
                    f.tags.chunks(2).find(|kv| l.keys[kv[0] as usize] == k).map_or(0, |kv| {
                        match l.values[kv[1] as usize] {
                            names::mvt::Value::Uint(v) => v,
                            _ => 0,
                        }
                    })
                };
                (get("h"), get("s"), get("o"))
            })
            .collect()
    }

    #[test]
    fn copies_for_the_flat_footprints() {
        // In 6/32/22, a building across the line between z14 tiles 8297 and 8298 (lon 2.3291…),
        // filled by the neighbours' rule (ten measured houses around it), and one across a z8
        // z8 tile's edge (lon 2.8125, between z8 tiles 129 and 130): each in its centroid's tile, and a
        // copy in the other, the same but for o.
        let t = TileId { z: 6, x: 32, y: 22 };
        let d = tempfile::tempdir().unwrap();
        let line = crate::bld::tile_box_deg(14, 8298, 5634)[0];
        let lat = 48.86;
        let mut bs = vec![B { id: 1, rings: vec![square(line - 0.0001, lat, 0.0003)], ..Default::default() }];
        for k in 0..10 {
            bs.push(B {
                id: 10 + k,
                rings: vec![square(line + 0.0008 + 0.0003 * (k % 5) as f64, lat + 0.0003 * (k / 5) as f64, 0.0002)],
                height: 12.0,
                ..Default::default()
            });
        }
        let edge = crate::bld::tile_box_deg(8, 130, 88)[0];
        bs.push(B { id: 2, rings: vec![square(edge - 0.0001, lat, 0.0003)], height: 30.0, ..Default::default() });
        let mut files: Vec<Option<WorkFile>> = (0..9).map(|_| None).collect();
        files[4] = Some(work(d.path(), t, &bs));
        let c = cov("place:2.5,48.86,40", "FR");
        let mut got: BTreeMap<(u8, u32, u32), Props> = BTreeMap::new();
        let sum = tiles_of(&files, &c, t, &[], &mut |z, x, y, gz, _| {
            got.insert((z, x, y), props(gz));
            Ok(())
        })
        .unwrap();
        // A copy of each.
        assert_eq!(sum.copies, 2, "{got:?}");
        let (x0, y0) = crate::bld::tile_of(crate::bld::world(line + 0.00005, lat + 0.00015), 14);
        let own = &got[&(14, x0, y0)];
        let copy = &got[&(14, x0 - 1, y0)];
        assert!(own.contains(&(120, 3, 0)), "{own:?}");
        assert_eq!(copy, &vec![(120, 3, 1)]);
        let (x1, y1) = crate::bld::tile_of(crate::bld::world(edge + 0.00005, lat + 0.00015), 14);
        assert_eq!(got[&(14, x1, y1)], vec![(300, 0, 0)]);
        assert_eq!(got[&(14, x1 - 1, y1)], vec![(300, 0, 1)], "copied across the z8 tiles' edge");
        assert!(got.keys().all(|k| k.0 == 14));
    }
}

#[cfg(test)]
#[path = "job_holdout.rs"]
mod holdout;
