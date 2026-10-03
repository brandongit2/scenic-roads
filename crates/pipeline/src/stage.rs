//! A unit's local copies of the global-source layers, in the files the legacy steps read
//! (docs/plan.md §6, base(U)): `terrain.tiles` (z0–12) and the z11 grids (`grid.idx`,
//! `grid.terrain.i16`, `grid.class.u8`, `grid.areas.u8`, and canopy and cover when the catalog has
//! them), over the unit grown by a margin (viewsheds see 15 km past the unit's buffer ways).
//!
//! Read from the packs on the NAS with plain reads (never mmapped), each pack's index once: the
//! ones this build's manifest names now (`Source::Manifest`, what the unit keys hash), or a
//! published catalog's (`Source::Catalog`, another root's for a pilot).
//!
//! Also today's heritage sites (`heritage.json`, for the flags step), clipped from the converted
//! worldwide file (`Heritage`).

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
    /// jobs of the same plan wrote (the catalog is published only at its end).
    Manifest(&'a Out),
    /// A published catalog under a root (another root's, for a pilot built against the real one).
    Catalog(&'a Path, &'a Catalog),
}

impl Source<'_> {
    /// The pack (logical name) holding a tile of `layer`: root z0–2, lo z3–8 by z3 tile, hi z9–14
    /// by z6 tile.
    fn pack_of(&self, layer: &str, z: u8, x: u32, y: u32) -> Option<String> {
        match self {
            Source::Manifest(_) => Some(ManifestTiles::logical(layer, z, x, y)),
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

    /// The file of a pack or worldwide file, by logical name (None: there's none).
    fn file(&self, logical: &str) -> Option<PathBuf> {
        match self {
            Source::Manifest(out) => out.get(logical).map(|c| out.path(c)),
            Source::Catalog(root, cat) => cat.files.get(logical).map(|f| root.join(&f.file)),
        }
    }

    fn has_layer(&self, layer: &str) -> bool {
        match self {
            Source::Manifest(out) => {
                let p = format!("layers/{layer}/");
                out.manifest.range(p.clone()..).next().is_some_and(|(l, _)| l.starts_with(&p))
            }
            Source::Catalog(_, cat) => cat.layers.contains_key(layer),
        }
    }

    /// A worldwide file (`what` as in `Catalog::global`: `global/<what>`): its path and size.
    fn global(&self, what: &str) -> Result<Option<(PathBuf, u64)>> {
        match self {
            Source::Manifest(out) => {
                let Some(c) = out.get(&format!("global/{what}")) else { return Ok(None) };
                let p = out.path(c);
                let size = std::fs::metadata(&p).with_context(|| format!("{}", p.display()))?.len();
                Ok(Some((p, size)))
            }
            Source::Catalog(root, cat) => {
                let Some(logical) = cat.global.get(what) else { return Ok(None) };
                let f = cat.files.get(logical).with_context(|| format!("catalog: no file for {logical}"))?;
                Ok(Some((root.join(&f.file), f.size)))
            }
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
    let n = 1u32 << z;
    let x = |lon: f64| (((lon + 180.0) / 360.0 * n as f64).floor().max(0.0) as u32).min(n - 1);
    let y = |lat: f64| {
        let r = lat.clamp(-85.05, 85.05).to_radians();
        (((1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n as f64).floor().max(0.0) as u32).min(n - 1)
    };
    let (x0, x1, y0, y1) = (x(b[0]), x(b[2]), y(b[3]), y(b[1]));
    (x0..=x1).flat_map(|tx| (y0..=y1).map(move |ty| (tx, ty))).collect()
}

/// A z/x/y tile's box (degrees) grown by `km`.
pub fn tile_box_grown(z: u8, x: u32, y: u32, km: f64) -> [f64; 4] {
    let n = (1u64 << z) as f64;
    let lon = |t: f64| t / n * 360.0 - 180.0;
    let lat = |t: f64| (std::f64::consts::PI * (1.0 - 2.0 * t / n)).sinh().atan().to_degrees();
    let (w, e, s, nn) = (lon(x as f64), lon(x as f64 + 1.0), lat(y as f64 + 1.0), lat(y as f64));
    let dy = km / 110.574;
    let dx = km / (111.320 * nn.abs().max(s.abs()).min(85.0).to_radians().cos());
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

/// Writes `terrain.tiles` and the grids for the box `b` (degrees) into `dir`.
pub fn stage(src: &Source, b: [f64; 4], dir: &Path) -> Result<Staged> {
    std::fs::create_dir_all(dir)?;
    let mut st = Staged::default();
    // Terrain z0–12 as the legacy archive (PNG blobs as they are).
    let mut terrain = LayerReader::new(src, "terrain");
    let tmp = dir.join("terrain.tiles.tmp");
    let mut w = ArchiveWriter::create(&tmp, r#"{"format":"png","encoding":"terrarium"}"#)?;
    let mut z11: HashMap<(u32, u32), Vec<u8>> = HashMap::new();
    for z in 0..=12u8 {
        for (x, y) in tiles_in(z, b) {
            if let Some(blob) = terrain.get(z, x, y)? {
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
    for var in ["class", "areas", "canopy", "cover"] {
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
        // The slots (grid.idx order) the packs lack, for the step that makes them (landcover.py
        // --only for class).
        write_file(dir, &format!("grid.{var}.missing.u32"), bytemuck::cast_slice(&missing))?;
        st.grids.push(var.into());
        st.missing.insert(var.into(), missing.len());
    }
    idx.save(&dir.join("grid.idx.tmp"))?;
    std::fs::rename(dir.join("grid.idx.tmp"), dir.join("grid.idx"))?;
    Ok(st)
}

/// A local copy of one of the worldwide files (`what` as in `Catalog::global`), made once under
/// `cache/global/` (content-named, so a copy is never stale; older copies go). None when there's no
/// such file.
pub fn local_global(src: &Source, what: &str, cache: &Path) -> Result<Option<PathBuf>> {
    let Some((file, size)) = src.global(what)? else { return Ok(None) };
    let name = file.file_name().context("global file name")?.to_string_lossy().into_owned();
    let dir = cache.join("global").join(what.replace('/', "-"));
    let local = dir.join(&name);
    if std::fs::metadata(&local).is_ok_and(|m| m.len() == size) {
        return Ok(Some(local));
    }
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::copy(&file, &tmp).with_context(|| format!("copy {}", file.display()))?;
    anyhow::ensure!(std::fs::metadata(&tmp)?.len() == size, "{}: not the expected size", file.display());
    std::fs::rename(&tmp, &local)?;
    for e in std::fs::read_dir(&dir)?.flatten() {
        if e.file_name().to_string_lossy() != name {
            std::fs::remove_file(e.path()).ok();
        }
    }
    Ok(Some(local))
}

/// Today's heritage sites (the converted `heritage.json`, `legacy/heritage` in the catalog), held
/// for a run: each unit's folder gets the sites in its box (the flags step marks samples within
/// 500 m).
pub struct Heritage {
    sites: Vec<([f64; 2], Box<serde_json::value::RawValue>)>,
}

impl Heritage {
    /// None when there are no heritage sites.
    pub fn load(src: &Source, cache: &Path) -> Result<Option<Heritage>> {
        let Some(p) = local_global(src, "legacy/heritage", cache)? else { return Ok(None) };
        Ok(Some(Heritage::parse(&std::fs::read(&p)?).with_context(|| format!("parse {}", p.display()))?))
    }

    fn parse(bytes: &[u8]) -> Result<Heritage> {
        #[derive(serde::Deserialize)]
        struct Fc {
            features: Vec<Box<serde_json::value::RawValue>>,
        }
        #[derive(serde::Deserialize)]
        struct Feature {
            geometry: Geometry,
        }
        #[derive(serde::Deserialize)]
        struct Geometry {
            #[serde(rename = "type")]
            kind: String,
            coordinates: serde_json::Value,
        }
        let fc: Fc = serde_json::from_slice(bytes)?;
        let mut sites = Vec::with_capacity(fc.features.len());
        for raw in fc.features {
            let f: Feature = serde_json::from_str(raw.get())?;
            // Points only, as the flags step reads them.
            if let (true, Some(lon), Some(lat)) = (f.geometry.kind == "Point", f.geometry.coordinates[0].as_f64(), f.geometry.coordinates[1].as_f64()) {
                sites.push(([lon, lat], raw));
            }
        }
        Ok(Heritage { sites })
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }

    /// Writes `heritage.json` with the sites inside `b` (w, s, e, n, degrees); returns how many.
    pub fn write_in(&self, b: [f64; 4], dir: &Path) -> Result<usize> {
        let inside: Vec<&str> = self.sites.iter().filter(|(p, _)| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]).map(|(_, r)| r.get()).collect();
        let body = format!(r#"{{"type":"FeatureCollection","features":[{}]}}"#, inside.join(","));
        write_file(dir, "heritage.json", body.as_bytes())?;
        Ok(inside.len())
    }
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
    fn heritage_sites_in_a_box() {
        let h = Heritage::parse(
            br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","geometry":{"type":"Point","coordinates":[-1.5,55.0]},"properties":{"name":"a"}},
            {"type":"Feature","geometry":{"type":"Point","coordinates":[2.0,48.8]},"properties":{"name":"b"}},
            {"type":"Feature","geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]},"properties":{}}]}"#,
        )
        .unwrap();
        assert_eq!(h.len(), 2);
        let d = tempfile::tempdir().unwrap();
        assert_eq!(h.write_in([-2.0, 54.0, -1.0, 56.0], d.path()).unwrap(), 1);
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(d.path().join("heritage.json")).unwrap()).unwrap();
        assert_eq!(v["features"][0]["properties"]["name"], "a");
        assert_eq!(v["features"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn tile_ranges() {
        // The z6 tile of Newcastle (6/31/19) grown by 30 km meets its neighbours.
        let b = tile_box_grown(6, 31, 19, 30.0);
        let t = tiles_in(6, b);
        assert!(t.contains(&(31, 19)) && t.contains(&(30, 19)) && t.contains(&(32, 20)), "{t:?}");
        assert_eq!(tiles_in(11, tile_box_grown(6, 31, 19, 0.0)).len(), 33 * 33);
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

