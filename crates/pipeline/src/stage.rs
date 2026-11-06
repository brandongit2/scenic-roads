//! A z6 tile's local copies of the global-source layers, in the files the z6 tile's programs read
//! (docs/steps.md §6): `terrain.tiles` (z0–12) and the z11 grids (`grid.idx`,
//! `grid.terrain.i16`, `grid.class.u8`, `grid.areas.u8`, and canopy and cover when the catalogue has
//! them), over what the z6 tile's own ways' samples see (`Area`).
//!
//! Read from the packs with plain reads (never mmapped), each pack's index once: the ones this
//! build's manifest names now (`Source::Manifest`, what the z6 tile keys hash), through this Mac's
//! local copies of them when it has a `Blobs` cache (each pack copied once, whole: neighbouring
//! z6 tiles share most of theirs, and a pack read tile by tile over SMB took thousands of small reads;
//! but a neighbour's terrain hi pack the margin only clips is read where it is, its few tiles in
//! spans, unless it's here already: `copied_whole`), or a published catalogue's (`Source::Catalogue`,
//! another root's for a pilot).

use crate::terrain_pack::ManifestTiles;
use anyhow::{Context, Result};
use buildstore::out::Out;
use roadcore::archive::ArchiveWriter;
use roadcore::grid::{decode_terrain, CELLS};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use store::catalogue::Catalogue;
use store::nasfs::Place;
use store::pack::PackIndex;
use store::range::PlainFile;

/// How far a sample sees, km: its far field (roadcore::scenic::FAR_MAX_M).
pub const FAR_KM: f64 = roadcore::scenic::FAR_MAX_M / 1000.0;

/// How far past the z6 tile the copies reach, km: the ordinary ways it owns lie within its tile
/// grown by `reach::LONG_KM` (crate::reach), and each of their samples sees `FAR_KM` past that. (The
/// long ways it owns, which go further, are staged along: `Area`.)
pub const MARGIN_KM: f64 = crate::reach::LONG_KM + FAR_KM;

/// What a z6 tile stages: its tile grown by `MARGIN_KM` (`b`), and along each long way it owns
/// (crate::reach::LongWay; ferries have no samples) its nodes in runs, each run's box grown by
/// `FAR_KM`: every sample of a way the z6 tile owns sees staged terrain and canopy to its far
/// field's end. Its key reads the same (crate::agent::build::unit_terrain_tiles).
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    /// Its tile grown by `MARGIN_KM` (w, s, e, n, degrees).
    pub b: [f64; 4],
    /// The boxes along its long ways (degrees).
    along: Vec<[f64; 4]>,
}

impl Area {
    /// The most a run of a long way's nodes spans, degrees each way: a run's box grown by `FAR_KM`
    /// stays within about twice the far field, and a long way of 600 km makes some 40 boxes.
    const RUN_DEG: f64 = FAR_KM / roadcore::geo::KM_PER_DEG;

    /// Z6 tile `u`'s, its long ways from its reach (none: its tile grown by `MARGIN_KM` alone).
    pub fn of(u: codecs::legacy::TileId, reach: Option<&crate::reach::Reach>) -> Area {
        let b = roadcore::geo::tile_box_grown(u.z, u.x, u.y, MARGIN_KM);
        let mut along = Vec::new();
        for w in reach.into_iter().flat_map(|r| &r.long).filter(|w| w.owned && !w.ferry) {
            let deg = |p: [i32; 2]| [p[0] as f64 * 1e-7, p[1] as f64 * 1e-7];
            let grown = |r: [f64; 4], p: [f64; 2]| [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])];
            let point = |p: [f64; 2]| [p[0], p[1], p[0], p[1]];
            let (mut run, mut last): (Option<[f64; 4]>, Option<[f64; 2]>) = (None, None);
            for p in w.verts.iter().map(|&p| deg(p)) {
                run = Some(match (run, last) {
                    (Some(r), Some(q)) => {
                        let g = grown(r, p);
                        if g[2] - g[0] <= Self::RUN_DEG && g[3] - g[1] <= Self::RUN_DEG {
                            g
                        } else {
                            // (The next run starts at this run's last node: the stretch between
                            // them is in it.)
                            along.push(roadcore::geo::within_tiles(roadcore::geo::grow(r, FAR_KM)));
                            grown(point(q), p)
                        }
                    }
                    _ => point(p),
                });
                last = Some(p);
            }
            along.extend(run.map(|r| roadcore::geo::within_tiles(roadcore::geo::grow(r, FAR_KM))));
        }
        Area { b, along }
    }

    /// Its tiles at zoom `z`, sorted.
    pub fn tiles(&self, z: u8) -> Vec<(u32, u32)> {
        let t: std::collections::BTreeSet<(u32, u32)> =
            tiles_in(z, self.b).into_iter().chain(self.along_tiles(z)).collect();
        t.into_iter().collect()
    }

    /// Its tiles at zoom `z` along its long ways, sorted (some may be in `b` too).
    pub fn along_tiles(&self, z: u8) -> Vec<(u32, u32)> {
        let t: std::collections::BTreeSet<(u32, u32)> = self.along.iter().flat_map(|&a| tiles_in(z, a)).collect();
        t.into_iter().collect()
    }
}

/// Where the global-source layers are read from.
pub enum Source<'a> {
    /// This build's manifest as it is now: the packs the z6 tile keys hash, including what earlier
    /// jobs of the same plan wrote (the catalogue is published only at its end); read through the
    /// local copies when there's a cache.
    Manifest(&'a Out, Option<&'a store::blobs::Blobs>),
    /// A published catalogue under a root (another root's, for a pilot built against the real one).
    Catalogue(&'a store::nasfs::NasRoot, &'a Catalogue),
}

impl Source<'_> {
    /// The pack (logical name) holding a tile of `layer`: root z0–2, lo z3–8 by z3 tile, hi z9–14
    /// by z6 tile; the terrain's deep levels, z13–16, by z10 tile (crate::terrain_deep).
    fn pack_of(&self, layer: &str, z: u8, x: u32, y: u32) -> Option<String> {
        match self {
            Source::Manifest(..) => Some(ManifestTiles::logical(layer, z, x, y)),
            Source::Catalogue(_, cat) => {
                let l = cat.layers.get(layer)?;
                let d = crate::terrain_deep::PACK_Z;
                match z {
                    0..=2 => l.root.clone(),
                    3..=8 => l.lo.get(&format!("3/{}/{}", x >> (z - 3), y >> (z - 3))).cloned(),
                    _ if z >= crate::terrain_deep::FIRST_Z => {
                        l.deep.get(&format!("{d}/{}/{}", x >> (z - d), y >> (z - d))).cloned()
                    }
                    _ => l.hi.get(&format!("6/{}/{}", x >> (z - 6), y >> (z - 6))).cloned(),
                }
            }
        }
    }

    /// The file of a pack or worldwide file, by logical name (None: there's none): the local copy
    /// when there's a cache (the NAS's if copying fails: the read is only slower).
    fn file(&self, logical: &str) -> Option<Place> {
        match self {
            Source::Manifest(out, None) => out.get(logical).map(|c| Place::Nas(out.path(c))),
            Source::Manifest(out, Some(blobs)) => out.get(logical).map(|c| match blobs.get(out.root(), c) {
                Ok(l) => Place::Local(l),
                Err(e) => {
                    eprintln!("stage: {c} not copied here ({e}); read from the NAS");
                    Place::Nas(out.path(c))
                }
            }),
            Source::Catalogue(root, cat) => cat.files.get(logical).map(|f| Place::Nas(root.join(&f.file))),
        }
    }

    /// The file of a pack, by logical name, without copying it here: this Mac's copy when it has
    /// one, else the NAS's (`file` copies it first).
    fn file_here(&self, logical: &str) -> Option<Place> {
        match self {
            Source::Manifest(out, Some(blobs)) => {
                out.get(logical).map(|c| blobs.have(c).map_or_else(|| Place::Nas(out.path(c)), Place::Local))
            }
            _ => self.file(logical),
        }
    }

    /// The content names of the packs `stage` reads for `area` (manifest sources only): what to
    /// copy ahead, while the z6 tile before it builds. A neighbour's terrain hi pack that its box only
    /// clips isn't (`copied_whole`), nor one along its long ways: its strip is read where it is.
    pub fn pack_contents(&self, area: &Area) -> Vec<String> {
        let Source::Manifest(out, _) = self else { return Vec::new() };
        let b = area.b;
        let mut logical: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for z in 0..=12u8 {
            for (x, y) in area.tiles(z) {
                logical.extend(self.pack_of("terrain", z, x, y).filter(|_| copied_whole(z, x, y, b)));
            }
        }
        let z11 = area.tiles(11);
        for var in ["class", "canopy", "cover"] {
            for &(x, y) in &z11 {
                logical.extend(self.pack_of(&format!("grid-{var}"), 11, x, y));
            }
        }
        logical.iter().filter_map(|l| out.get(l).map(str::to_string)).collect()
    }

    fn has_layer(&self, layer: &str) -> bool {
        match self {
            Source::Manifest(out, _) => {
                let p = format!("layers/{layer}/");
                out.manifest.range(p.clone()..).next().is_some_and(|(l, _)| l.starts_with(&p))
            }
            Source::Catalogue(_, cat) => cat.layers.contains_key(layer),
        }
    }
}

/// Tiles of a layer, from its packs.
pub struct LayerReader<'a> {
    src: &'a Source<'a>,
    layer: String,
    open: HashMap<String, Option<(PlainFile, PackIndex)>>,
}

impl<'a> LayerReader<'a> {
    pub fn new(src: &'a Source<'a>, layer: &str) -> LayerReader<'a> {
        LayerReader { src, layer: layer.to_string(), open: HashMap::new() }
    }

    pub fn exists(&self) -> bool {
        self.src.has_layer(&self.layer)
    }

    /// A tile's blob as stored (None when the layer has no such tile).
    pub fn get(&mut self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        let Some(logical) = self.src.pack_of(&self.layer, z, x, y) else { return Ok(None) };
        if !self.open.contains_key(&logical) {
            let opened = match self.src.file(&logical) {
                Some(p) => {
                    let p = p.loc();
                    let pf = p.open_plain().with_context(|| format!("open {}", p.display()))?;
                    let idx = PackIndex::read_from(&pf).with_context(|| format!("index of {}", p.display()))?;
                    Some((pf, idx))
                }
                None => None,
            };
            self.open.insert(logical.clone(), opened);
        }
        let Some((pf, idx)) = self.open.get(&logical).and_then(Option::as_ref) else { return Ok(None) };
        Ok(idx.get(pf, z, x, y)?.map(|(_, b)| b))
    }
}

/// The tiles at `z` meeting the box (w, s, e, n in degrees).
pub fn tiles_in(z: u8, b: [f64; 4]) -> Vec<(u32, u32)> {
    let [x0, x1, y0, y1] = tile_range(z, b);
    (x0..=x1).flat_map(|tx| (y0..=y1).map(move |ty| (tx, ty))).collect()
}

/// `tiles_in` as ranges: [x0, x1, y0, y1], both ends in.
pub fn tile_range(z: u8, b: [f64; 4]) -> [u32; 4] {
    let n = 1u32 << z;
    // (A billionth of a tile up before the floor: a box edge on a tile boundary, from
    // roadcore::geo::tile_box_grown's round trip through degrees, can come back a hair below it
    // and would take in the tile before.)
    let tile = |t: f64| ((t * n as f64 + 1e-9).floor().max(0.0) as u32).min(n - 1);
    let x = |lon: f64| tile(roadcore::geo::merc_x(lon));
    let y = |lat: f64| tile(roadcore::geo::merc_clamped(0.0, lat).1);
    [x(b[0]), x(b[2]), y(b[3]), y(b[1])]
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Staged {
    pub terrain_tiles: usize,
    pub grid_tiles: usize,
    pub grids: Vec<String>,
    /// Per grid, the z11 tiles its packs don't have (new coverage): made in the folder instead.
    pub missing: std::collections::BTreeMap<String, usize>,
}

/// Whether the terrain pack holding tile (z, x, y) is copied here whole for box `b` (`Blobs`): a lo
/// or root pack, or a z6 tile's hi pack the box holds at least half of (its own z6 tile's), not a
/// neighbour's the box's margin only clips, whose few tiles are read where they are (`read_tiles`),
/// nor a deep pack (a z10 tile's every deep tile: a z6 tile's roads read a few of them).
fn copied_whole(z: u8, x: u32, y: u32, b: [f64; 4]) -> bool {
    if z < 9 {
        return true;
    }
    if z > 12 {
        return false;
    }
    let (x6, y6) = (x >> (z - 6), y >> (z - 6));
    let [x0, x1, y0, y1] = tile_range(12, b);
    let (ax, ay) = ((x6 << 6).max(x0), (y6 << 6).max(y0));
    let (bx, by) = (((x6 + 1) << 6).min(x1 + 1), ((y6 + 1) << 6).min(y1 + 1));
    let inside = (bx.saturating_sub(ax) as u64) * (by.saturating_sub(ay) as u64);
    2 * inside >= 64 * 64
}

/// The terrain's tiles `want` that it has, for box `b`, a pack at a time: from this Mac's whole copy
/// of a pack the box holds most of (`copied_whole`, copied first when missing), or one it has
/// already; else from the NAS's, its wanted tiles in spans (`read_tiles`).
fn terrain_tiles(src: &Source, b: [f64; 4], want: &[(u8, u32, u32)]) -> Result<HashMap<(u8, u32, u32), Vec<u8>>> {
    let mut by_pack: std::collections::BTreeMap<String, Vec<(u8, u32, u32)>> = std::collections::BTreeMap::new();
    for &(z, x, y) in want {
        if let Some(l) = src.pack_of("terrain", z, x, y) {
            by_pack.entry(l).or_default().push((z, x, y));
        }
    }
    let mut got = HashMap::with_capacity(want.len());
    for (logical, tiles) in by_pack {
        let (z, x, y) = tiles[0];
        let file = if copied_whole(z, x, y, b) { src.file(&logical) } else { src.file_here(&logical) };
        let Some(p) = file else { continue };
        let p = p.loc();
        let pf = p.open_plain().with_context(|| format!("open {}", p.display()))?;
        let idx = PackIndex::read_from(&pf).with_context(|| format!("index of {}", p.display()))?;
        for (t, blob) in read_tiles(&pf, &idx, &tiles).with_context(|| p.display())? {
            got.insert(t, blob);
        }
    }
    Ok(got)
}

/// Tiles `tiles` of a pack that it has, read in spans: their blobs in the pack's order, neighbours
/// under `SPAN_GAP` apart read together (up to `SPAN_MAX`), the spans in parallel, each blob checked
/// against its checksum as `PackIndex::read_blob` does.
fn read_tiles(pf: &PlainFile, idx: &PackIndex, tiles: &[(u8, u32, u32)]) -> Result<Vec<((u8, u32, u32), Vec<u8>)>> {
    use rayon::prelude::*;
    use store::range::RangeRead;
    const SPAN_GAP: u64 = 256 << 10;
    const SPAN_MAX: u64 = 32 << 20;
    let mut ents: Vec<(store::pack::Entry, (u8, u32, u32))> =
        tiles.iter().filter_map(|&(z, x, y)| idx.find(z, x, y).map(|e| (e, (z, x, y)))).collect();
    ents.sort_unstable_by_key(|(e, _)| (e.offset, e.key));
    let mut spans: Vec<(u64, u64, Vec<(store::pack::Entry, (u8, u32, u32))>)> = Vec::new();
    for (e, t) in ents {
        let end = e.offset + e.len as u64;
        match spans.last_mut() {
            Some((s0, s1, v)) if e.offset <= *s1 + SPAN_GAP && end - *s0 <= SPAN_MAX => {
                *s1 = (*s1).max(end);
                v.push((e, t));
            }
            _ => spans.push((e.offset, end, vec![(e, t)])),
        }
    }
    let read: Vec<Result<Vec<((u8, u32, u32), Vec<u8>)>>> = spans
        .par_iter()
        .map(|(s0, s1, v)| {
            let b = pf.read_at(*s0, (s1 - s0) as usize)?;
            v.iter()
                .map(|(e, t)| {
                    let blob = b[(e.offset - s0) as usize..(e.offset - s0) as usize + e.len as usize].to_vec();
                    anyhow::ensure!(
                        store::naming::xxh3(&blob) == e.hash,
                        "tile {}/{}/{}: checksum mismatch (a damaged pack or read)",
                        t.0,
                        t.1,
                        t.2
                    );
                    Ok((*t, blob))
                })
                .collect()
        })
        .collect();
    let mut out = Vec::with_capacity(tiles.len());
    for r in read {
        out.extend(r?);
    }
    Ok(out)
}

/// Writes `terrain.tiles` and the grids for `area` into `dir`: the terrain's z0–12 over it, and
/// its deep tiles `deep` (z13–16, those the roads' heights read:
/// crate::road_terrain::deep_tiles) where it has them, as stored (lossless WebP,
/// codecs::terrain_codec) in one archive (crate::road_terrain::TERRAIN_FILE).
pub fn stage(
    src: &Source,
    area: &Area,
    deep: &std::collections::BTreeSet<(u8, u32, u32)>,
    dir: &Path,
) -> Result<Staged> {
    std::fs::create_dir_all(dir)?;
    let b = area.b;
    let mut st = Staged::default();
    let mut want: Vec<(u8, u32, u32)> =
        (0..=12u8).flat_map(|z| area.tiles(z).into_iter().map(move |(x, y)| (z, x, y))).collect();
    want.extend(deep.iter().copied().filter(|t| t.0 > 12));
    let mut got = terrain_tiles(src, b, &want)?;
    want.sort_unstable_by_key(|t| roadcore::archive::tile_key(t.0, t.1, t.2));
    want.dedup();
    let file = crate::road_terrain::TERRAIN_FILE;
    let tmp = dir.join(format!("{file}.tmp"));
    let mut w = ArchiveWriter::create(&tmp, r#"{"format":"webp","encoding":"terrarium"}"#)?;
    let mut z11: HashMap<(u32, u32), Vec<u8>> = HashMap::new();
    for (z, x, y) in want {
        if let Some(blob) = got.remove(&(z, x, y)) {
            w.add(z, x, y, &blob, blob.len())?;
            st.terrain_tiles += 1;
            if z == 11 {
                z11.insert((x, y), blob);
            }
        }
    }
    w.finish()?;
    std::fs::rename(&tmp, dir.join(file))?;

    // The grids over its z11 tiles.
    let mut tiles: Vec<[u32; 2]> = area.tiles(11).into_iter().map(|(x, y)| [x, y]).collect();
    tiles.sort_unstable_by_key(|t| (t[1], t[0]));
    let idx = roadcore::grid::GridIndex::new(tiles);
    let tiles = idx.tiles.clone();
    st.grid_tiles = tiles.len();
    let mut terr: Vec<i16> = vec![0; tiles.len() * CELLS];
    for (s, t) in tiles.iter().enumerate() {
        let Some(png) = z11.get(&(t[0], t[1])) else { continue };
        let e = decode_terrain(png).with_context(|| format!("terrain 11/{}/{}", t[0], t[1]))?;
        for (o, v) in terr[s * CELLS..(s + 1) * CELLS].iter_mut().zip(e) {
            *o = v.round().clamp(-500.0, 9000.0) as i16;
        }
    }
    write_file(dir, "grid.terrain.i16", bytemuck::cast_slice(&terr))?;
    st.grids.push("terrain".into());
    // (The area flags are rasterised per z6 tile: crate::heritage, codecs::areaflags.)
    for var in ["class", "canopy", "cover"] {
        let mut l = LayerReader::new(src, &format!("grid-{var}"));
        let mut data = vec![0u8; tiles.len() * CELLS];
        let mut missing: Vec<u32> = Vec::new();
        for (s, t) in tiles.iter().enumerate() {
            match if l.exists() { l.get(11, t[0], t[1])? } else { None } {
                Some(z) => {
                    let cells = zstd::decode_all(&z[..]).with_context(|| format!("grid-{var} 11/{}/{}", t[0], t[1]))?;
                    anyhow::ensure!(cells.len() == CELLS, "grid-{var} 11/{}/{}: {} cells", t[0], t[1], cells.len());
                    data[s * CELLS..(s + 1) * CELLS].copy_from_slice(&cells);
                }
                None => missing.push(s as u32),
            }
        }
        write_file(dir, &format!("grid.{var}.u8"), &data)?;
        // The slots (grid.idx order) the packs lack, for the step that makes them (the `landcover`
        // program's --only, for class).
        write_file(dir, &format!("grid.{var}.missing.u32"), bytemuck::cast_slice(&missing))?;
        st.grids.push(var.into());
        st.missing.insert(var.into(), missing.len());
    }
    idx.save(&dir.join("grid.idx.tmp"))?;
    std::fs::rename(dir.join("grid.idx.tmp"), dir.join("grid.idx"))?;
    Ok(st)
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let p = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ranges() {
        // The z6 tile of Newcastle (6/31/19) grown by 30 km meets its neighbours.
        let b = roadcore::geo::tile_box_grown(6, 31, 19, 30.0);
        let t = tiles_in(6, b);
        assert!(t.contains(&(31, 19)) && t.contains(&(30, 19)) && t.contains(&(32, 20)), "{t:?}");
        assert_eq!(tiles_in(11, roadcore::geo::tile_box(6, 31, 19)).len(), 33 * 33);
    }

    /// Every sample of a way a z6 tile owns has its far field staged: an ordinary way's (within the
    /// tile grown by `reach::LONG_KM`) inside the tile grown by `MARGIN_KM`, at every latitude; a long
    /// way's along it, at every node and between; a ferry's and a neighbour's long ways not.
    #[test]
    fn the_area_holds_every_owned_samples_far_field() {
        use crate::reach::{LongWay, Reach};
        use codecs::legacy::TileId;
        let inside = |a: [f64; 4], b: [f64; 4]| a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3];
        for (x, y) in [(31, 19), (20, 22), (10, 5), (60, 58), (33, 31)] {
            let u = TileId { z: 6, x, y };
            let tile = roadcore::geo::tile_box(6, x, y);
            let seen = roadcore::geo::within_tiles(roadcore::geo::grow(
                roadcore::geo::grow(tile, crate::reach::LONG_KM),
                FAR_KM,
            ));
            assert!(inside(seen, Area::of(u, None).b), "{x}/{y}: {seen:?} past {:?}", Area::of(u, None).b);
        }
        assert_eq!(MARGIN_KM, 35.0);
        // A long way from 6/31/19 (Newcastle) 300 km south, with a node every 2 km, and two not
        // staged: a ferry's, a neighbour's.
        let u = TileId { z: 6, x: 31, y: 19 };
        let e7 = |lon: f64, lat: f64| [(lon * 1e7) as i32, (lat * 1e7) as i32];
        let verts: Vec<[i32; 2]> = (0..150).map(|k| e7(-1.6 + 0.004 * k as f64, 55.0 - 0.018 * k as f64)).collect();
        let far = vec![e7(10.0, 40.0), e7(11.0, 41.0)];
        let reach = Reach {
            long: vec![
                LongWay { owned: true, ferry: false, verts: verts.clone() },
                LongWay { owned: true, ferry: true, verts: far.clone() },
                LongWay { owned: false, ferry: false, verts: far.clone() },
            ],
            ..Default::default()
        };
        let area = Area::of(u, Some(&reach));
        let z11: std::collections::BTreeSet<(u32, u32)> = area.tiles(11).into_iter().collect();
        for w in verts.windows(2) {
            let (a, b) = (w[0].map(|v| v as f64 * 1e-7), w[1].map(|v| v as f64 * 1e-7));
            let seg = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
            for t in tiles_in(11, roadcore::geo::grow(seg, FAR_KM)) {
                assert!(z11.contains(&t), "{t:?} along {seg:?}");
            }
        }
        assert!(tiles_in(11, [10.0, 40.0, 11.0, 41.0]).iter().all(|t| !z11.contains(t)));
        assert!(area.tiles(11).len() > Area::of(u, None).tiles(11).len());
        // (In runs: a box every 15 km or so of the way's 300, not one a node.)
        assert!(area.along.len() < 40, "{}", area.along.len());
    }

    /// A z6 tile's own terrain pack is copied here whole, its neighbours' that its margin only clips
    /// aren't (their strips read in spans where they are, or from a copy here if there's one), and
    /// what's staged is the same as reading every tile from the NAS's packs.
    #[test]
    fn a_neighbours_strip_is_read_where_it_is_and_staged_the_same() {
        let d = tempfile::tempdir().unwrap();
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.path().join("root")), &d.path().join("scratch"))
                .unwrap();
        let (x6, y6) = (31u32, 19u32);
        let area = Area::of(codecs::legacy::TileId { z: 6, x: x6, y: y6 }, None);
        let b = area.b;
        assert!(
            copied_whole(12, x6 << 6, y6 << 6, b) && copied_whole(9, x6 << 3, y6 << 3, b) && copied_whole(8, 0, 0, b)
        );
        assert!(!copied_whole(12, (x6 + 1) << 6, y6 << 6, b) && !copied_whole(10, (x6 - 1) << 4, (y6 + 1) << 4, b));
        // One terrain tile for every z11 tile (staging decodes them), other tiles' bytes their own.
        let png = codecs::terrain_codec::encode(&mut vec![123.0f32; 256 * 256], 0.1).unwrap();
        for (px, py) in [(x6, y6), (x6 + 1, y6), (x6, y6 + 1)] {
            let mut tiles: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
            for z in 9..=12u8 {
                let s = 1u32 << (z - 6);
                for x in px * s..(px + 1) * s {
                    for y in py * s..(py + 1) * s {
                        let blob = if z == 11 { png.clone() } else { format!("{z}/{x}/{y}").into_bytes() };
                        tiles.push((z, x, y, blob, 1));
                    }
                }
            }
            crate::layers::write_pack(
                &mut out,
                "terrain",
                codecs::terrain_codec::ENCODING,
                false,
                "hi",
                (6, px, py),
                &mut tiles.into_iter(),
            )
            .unwrap();
        }
        let blobs = store::blobs::Blobs::new(d.path().join("blobs"));
        let staged = |src: &Source, to: &str| {
            let dir = d.path().join(to);
            stage(src, &area, &Default::default(), &dir).unwrap();
            ["terrain.tiles", "grid.terrain.i16", "grid.idx"].map(|f| std::fs::read(dir.join(f)).unwrap())
        };
        let plain = staged(&Source::Manifest(&out, None), "plain");
        let here = staged(&Source::Manifest(&out, Some(&blobs)), "here");
        assert!(plain == here, "staged the same");
        let copies: Vec<String> =
            blobs.list().iter().map(|f| f.0.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert!(copies[0].starts_with(&format!("6-{x6}-{y6}.")), "its own pack alone: {copies:?}");
        let src = Source::Manifest(&out, Some(&blobs));
        let ahead: Vec<&str> = src
            .pack_contents(&area)
            .iter()
            .map(|c| c.rsplit('/').next().unwrap().split('.').next().unwrap())
            .map(|n| if n == format!("6-{x6}-{y6}") { "own" } else { "other" })
            .collect();
        assert_eq!(ahead, ["own"], "only its own copied ahead");
        // A neighbour's copy here, where there is one, is read instead: the same staged.
        blobs.get(out.root(), out.get(&format!("layers/terrain/hi/6-{}-{y6}", x6 + 1)).unwrap()).unwrap();
        assert!(staged(&Source::Manifest(&out, Some(&blobs)), "again") == plain);
        assert_eq!(blobs.list().len(), 2);
    }
}

/// The grid tiles of `var` inside z6 tile (x, y) from a build folder, as a hi pack's tiles
/// (zstd, as the converted grids are stored).
pub fn grid_tiles_in(dir: &Path, var: &str, x6: u32, y6: u32) -> Result<Vec<(u8, u32, u32, Vec<u8>, u32)>> {
    let idx = roadcore::grid::GridIndex::load(dir)?;
    let data = std::fs::read(dir.join(format!("grid.{var}.u8")))?;
    anyhow::ensure!(data.len() == idx.tiles.len() * CELLS, "grid.{var}.u8 out of step with grid.idx");
    let mut out = Vec::new();
    for (s, t) in idx.tiles.iter().enumerate() {
        if (t[0] >> 5, t[1] >> 5) != (x6, y6) {
            continue;
        }
        let cells = &data[s * CELLS..(s + 1) * CELLS];
        out.push((11u8, t[0], t[1], zstd::encode_all(cells, 9)?, CELLS as u32));
    }
    out.sort_by_key(|t| (t.1, t.2));
    Ok(out)
}
