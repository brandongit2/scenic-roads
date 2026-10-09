//! A unit's local copies of the global-source layers, in the files the unit's programs read
//! (docs/plan.md §6, base(U)): `terrain.tiles` (z0–12) and the z11 grids (`grid.idx`,
//! `grid.terrain.i16`, `grid.class.u8`, `grid.areas.u8`, and canopy and cover when the catalog has
//! them), over the unit grown by a margin (viewsheds see 15 km past the unit's buffer ways).
//!
//! Read from the packs with plain reads (never mmapped), each pack's index once: the ones this
//! build's manifest names now (`Source::Manifest`, what the unit keys hash), through this Mac's
//! local copies of them when it has a `Blobs` cache (each pack copied once, whole: neighbouring
//! units share most of theirs, and a pack read tile by tile over SMB took thousands of small reads;
//! but a neighbour's terrain hi pack the margin only clips is read where it is, its few tiles in
//! spans, unless it's here already: `copied_whole`), or a published catalog's (`Source::Catalog`,
//! another root's for a pilot).

use det::Det;
use crate::out::Out;
use crate::terrain_pack::ManifestTiles;
use anyhow::{Context, Result};
use roadcore::archive::ArchiveWriter;
use roadcore::grid::{decode_terrain_png, CELLS};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use store::catalog::Catalog;
use store::pack::PackIndex;
use store::range::PlainFile;

/// How far past the unit the copies reach, km (10 km of buffer ways, then 15 km of view, rounded up).
pub const MARGIN_KM: f64 = 30.0;


/// Where the global-source layers are read from.
pub enum Source<'a> {
    /// This build's manifest as it is now: the packs the unit keys hash, including what earlier
    /// jobs of the same plan wrote (the catalog is published only at its end); read through the
    /// local copies when there's a cache.
    Manifest(&'a Out, Option<&'a store::blobs::Blobs>),
    /// A published catalog under a root (another root's, for a pilot built against the real one).
    Catalog(&'a Path, &'a Catalog),
}

impl Source<'_> {
    /// The pack (logical name) holding a tile of `layer`: root z0–2, lo z3–8 by z3 tile, hi z9–14
    /// by z6 tile.
    fn pack_of(&self, layer: &str, z: u8, x: u32, y: u32) -> Option<String> {
        match self {
            Source::Manifest(..) => Some(ManifestTiles::logical(layer, z, x, y)),
            Source::Catalog(_, cat) => {
                let l = cat.layers.get(layer)?;
                match z {
                    0..=2 => l.root.clone(),
                    3..=8 => l.lo.get(&format!("3/{}/{}", x >> (z - 3), y >> (z - 3))).cloned(),
                    _ => l.hi.get(&format!("6/{}/{}", x >> (z - 6), y >> (z - 6))).cloned(),
                }
            }
        }
    }

    /// The file of a pack or worldwide file, by logical name (None: there's none): the local copy
    /// when there's a cache (the NAS's if copying fails: the read is only slower).
    fn file(&self, logical: &str) -> Option<PathBuf> {
        match self {
            Source::Manifest(out, None) => out.get(logical).map(|c| out.path(c)),
            Source::Manifest(out, Some(blobs)) => out.get(logical).map(|c| {
                blobs.get(out.root(), c).unwrap_or_else(|e| {
                    eprintln!("stage: {c} not copied here ({e}); read from the NAS");
                    out.path(c)
                })
            }),
            Source::Catalog(root, cat) => cat.files.get(logical).map(|f| root.join(&f.file)),
        }
    }

    /// The file of a pack, by logical name, without copying it here: this Mac's copy when it has
    /// one, else the NAS's (`file` copies it first).
    fn file_here(&self, logical: &str) -> Option<PathBuf> {
        match self {
            Source::Manifest(out, Some(blobs)) => out.get(logical).map(|c| blobs.have(c).unwrap_or_else(|| out.path(c))),
            _ => self.file(logical),
        }
    }

    /// The content names of the packs `stage` reads for box `b` (manifest sources only): what to
    /// copy ahead, while the unit before it builds. A neighbour's terrain hi pack that the box only
    /// clips isn't (`copied_whole`): its strip is read where it is.
    pub fn pack_contents(&self, b: [f64; 4]) -> Vec<String> {
        let Source::Manifest(out, _) = self else { return Vec::new() };
        let mut logical: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for z in 0..=12u8 {
            for (x, y) in tiles_in(z, b) {
                logical.extend(self.pack_of("terrain", z, x, y).filter(|_| copied_whole(z, x, y, b)));
            }
        }
        for var in ["class", "canopy", "cover"] {
            for (x, y) in tiles_in(11, b) {
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
            Source::Catalog(_, cat) => cat.layers.contains_key(layer),
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
                    let pf = PlainFile::open(&p).with_context(|| format!("open {}", p.display()))?;
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
    // tile_box_grown's round trip through degrees, can come back a hair below it and would take in
    // the tile before.)
    let tile = |t: f64| ((t * n as f64 + 1e-9).floor().max(0.0) as u32).min(n - 1);
    let x = |lon: f64| tile((lon + 180.0) / 360.0);
    let y = |lat: f64| {
        let r = lat.clamp(-85.05, 85.05).to_radians();
        tile((1.0 - (r.dtan() + 1.0 / r.dcos()).dln() / std::f64::consts::PI) / 2.0)
    };
    [x(b[0]), x(b[2]), y(b[3]), y(b[1])]
}

/// A z/x/y tile's box (degrees) grown by `km`.
pub fn tile_box_grown(z: u8, x: u32, y: u32, km: f64) -> [f64; 4] {
    let n = (1u64 << z) as f64;
    let lon = |t: f64| t / n * 360.0 - 180.0;
    let lat = |t: f64| (std::f64::consts::PI * (1.0 - 2.0 * t / n)).dsinh().datan().to_degrees();
    let (w, e, s, nn) = (lon(x as f64), lon(x as f64 + 1.0), lat(y as f64 + 1.0), lat(y as f64));
    let dy = km / 110.574;
    let dx = km / (111.320 * nn.abs().max(s.abs()).min(85.0).to_radians().dcos());
    [w - dx, (s - dy).max(-85.05), e + dx, (nn + dy).min(85.05)]
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Staged {
    pub terrain_tiles: usize,
    pub grid_tiles: usize,
    pub grids: Vec<String>,
    /// Per grid, the z11 tiles its packs don't have (new coverage): made in the folder instead.
    pub missing: std::collections::BTreeMap<String, usize>,
}

/// Whether the terrain pack holding tile (z, x, y) is copied here whole for box `b` (`Blobs`): a lo
/// or root pack, or a z6 tile's hi pack the box holds at least half of (its own unit's), not a
/// neighbour's the box's margin only clips, whose few tiles are read where they are (`read_tiles`).
fn copied_whole(z: u8, x: u32, y: u32, b: [f64; 4]) -> bool {
    if z < 9 {
        return true;
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
        let pf = PlainFile::open(&p).with_context(|| format!("open {}", p.display()))?;
        let idx = PackIndex::read_from(&pf).with_context(|| format!("index of {}", p.display()))?;
        for (t, blob) in read_tiles(&pf, &idx, &tiles).with_context(|| format!("{}", p.display()))? {
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
    let mut ents: Vec<(store::pack::Entry, (u8, u32, u32))> = tiles.iter().filter_map(|&(z, x, y)| idx.find(z, x, y).map(|e| (e, (z, x, y)))).collect();
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
                    anyhow::ensure!(store::naming::xxh3(&blob) == e.hash, "tile {}/{}/{}: checksum mismatch (a damaged pack or read)", t.0, t.1, t.2);
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

/// Writes `terrain.tiles` and the grids for the box `b` (degrees) into `dir`.
pub fn stage(src: &Source, b: [f64; 4], dir: &Path) -> Result<Staged> {
    std::fs::create_dir_all(dir)?;
    let mut st = Staged::default();
    // Terrain z0–12 as a tile archive (PNG blobs as they are).
    let want: Vec<(u8, u32, u32)> = (0..=12u8).flat_map(|z| tiles_in(z, b).into_iter().map(move |(x, y)| (z, x, y))).collect();
    let mut got = terrain_tiles(src, b, &want)?;
    let tmp = dir.join("terrain.tiles.tmp");
    let mut w = ArchiveWriter::create(&tmp, r#"{"format":"png","encoding":"terrarium"}"#)?;
    let mut z11: HashMap<(u32, u32), Vec<u8>> = HashMap::new();
    for z in 0..=12u8 {
        for (x, y) in tiles_in(z, b) {
            if let Some(blob) = got.remove(&(z, x, y)) {
                w.add(z, x, y, &blob, blob.len())?;
                st.terrain_tiles += 1;
                if z == 11 {
                    z11.insert((x, y), blob);
                }
            }
        }
    }
    w.finish()?;
    std::fs::rename(&tmp, dir.join("terrain.tiles"))?;

    // The grids over the z11 tiles of the box.
    let mut tiles: Vec<[u32; 2]> = tiles_in(11, b).into_iter().map(|(x, y)| [x, y]).collect();
    tiles.sort_unstable_by_key(|t| (t[1], t[0]));
    let idx = roadcore::grid::GridIndex::new(tiles);
    let tiles = idx.tiles.clone();
    st.grid_tiles = tiles.len();
    let mut terr: Vec<i16> = vec![0; tiles.len() * CELLS];
    for (s, t) in tiles.iter().enumerate() {
        let Some(png) = z11.get(&(t[0], t[1])) else { continue };
        let e = decode_terrain_png(png).with_context(|| format!("terrain 11/{}/{}", t[0], t[1]))?;
        for (o, v) in terr[s * CELLS..(s + 1) * CELLS].iter_mut().zip(e) {
            *o = v.round().clamp(-500.0, 9000.0) as i16;
        }
    }
    write_file(dir, "grid.terrain.i16", bytemuck::cast_slice(&terr))?;
    st.grids.push("terrain".into());
    // (The area flags are rasterised per unit: crate::heritage, crate::areaflags.)
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
        let b = tile_box_grown(6, 31, 19, 30.0);
        let t = tiles_in(6, b);
        assert!(t.contains(&(31, 19)) && t.contains(&(30, 19)) && t.contains(&(32, 20)), "{t:?}");
        assert_eq!(tiles_in(11, tile_box_grown(6, 31, 19, 0.0)).len(), 33 * 33);
    }

    /// A unit's own terrain pack is copied here whole, its neighbours' that its margin only clips
    /// aren't (their strips read in spans where they are, or from a copy here if there's one), and
    /// what's staged is the same as reading every tile from the NAS's packs.
    #[test]
    fn a_neighbours_strip_is_read_where_it_is_and_staged_the_same() {
        let d = tempfile::tempdir().unwrap();
        let mut out = Out::open(&d.path().join("root"), &d.path().join("scratch")).unwrap();
        let (x6, y6) = (31u32, 19u32);
        let b = tile_box_grown(6, x6, y6, MARGIN_KM);
        assert!(copied_whole(12, x6 << 6, y6 << 6, b) && copied_whole(9, x6 << 3, y6 << 3, b) && copied_whole(8, 0, 0, b));
        assert!(!copied_whole(12, (x6 + 1) << 6, y6 << 6, b) && !copied_whole(10, (x6 - 1) << 4, (y6 + 1) << 4, b));
        // One terrain PNG for every z11 tile (staging decodes them), other tiles' bytes their own.
        let png = roadcore::grid::encode_terrain_png(&vec![123.0f32; 256 * 256], 256, 256).unwrap();
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
            crate::layers::write_pack(&mut out, "terrain", "terrarium-png", false, "hi", (6, px, py), &mut tiles.into_iter()).unwrap();
        }
        let blobs = store::blobs::Blobs::new(d.path().join("blobs"));
        let staged = |src: &Source, to: &str| {
            let dir = d.path().join(to);
            stage(src, b, &dir).unwrap();
            ["terrain.tiles", "grid.terrain.i16", "grid.idx"].map(|f| std::fs::read(dir.join(f)).unwrap())
        };
        let plain = staged(&Source::Manifest(&out, None), "plain");
        let here = staged(&Source::Manifest(&out, Some(&blobs)), "here");
        assert!(plain == here, "staged the same");
        let copies: Vec<String> = blobs.list().iter().map(|f| f.0.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert!(copies[0].starts_with(&format!("6-{x6}-{y6}.")), "its own pack alone: {copies:?}");
        let src = Source::Manifest(&out, Some(&blobs));
        let ahead: Vec<&str> = src.pack_contents(b).iter().map(|c| c.rsplit('/').next().unwrap().split('.').next().unwrap()).map(|n| if n == format!("6-{x6}-{y6}") { "own" } else { "other" }).collect();
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

