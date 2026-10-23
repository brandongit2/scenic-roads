//! The roads' heights from the finished terrain (docs/steps.md §6; docs/inputs.md §6.8): each
//! vertex's height is the stored terrain's there, bilinearly between pixel centres (across tile
//! edges, as the map draws it), at the finest zoom stored there: a fine tile (z13–16,
//! crate::terrain_fine) where one is, else the base's (z12, coarser at high latitudes:
//! crate::terrain_pack::max_zoom_at), else coarser. So a road and the ground under it come from one
//! dataset, detail and blend, already brought to EGM2008, and a unit's elevations read no server.
//!
//! The unit job stages what this reads into the unit's folder: the base tiles (`terrain.tiles`,
//! crate::stage), the fine tiles under its vertices and their neighbours across a near edge
//! (`fine.tiles`: `fine_tiles`, `stage_fine`), and the pool's datasets' codes by coverage
//! (`codes.json`: `Codes`), which name each vertex's source (`src.u8`): the first dataset in the
//! order whose coverage (its measured 1° cells) holds it. That's a label, not the terrain's own
//! record: in a cell a dataset covers only in part, it names that dataset where the terrain took the
//! next one through its voids.

use crate::dem::declared::Cover;
use crate::dem::spacing::{cell, Fine};
use anyhow::{Context, Result};
use roadcore::archive::Archive;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

/// The staged fine tiles in a unit's folder.
pub const FINE_FILE: &str = "fine.tiles";
/// The staged base tiles (crate::stage).
pub const BASE_FILE: &str = "terrain.tiles";
/// The datasets' codes by coverage (`Codes`).
pub const CODES_FILE: &str = "codes.json";

/// Decoded tiles kept at once while sampling (256 KB each: 512 MB).
const KEEP: usize = 2048;

/// A vertex's tile and pixel position at zoom `z` (E7 point): the tile's x, y and the position in
/// global pixels (pixel centres at .5).
fn pixel(z: u8, p: [i32; 2]) -> (f64, f64) {
    let (lon, lat) = (p[0] as f64 * 1e-7, p[1] as f64 * 1e-7);
    let n = (1u64 << z) as f64 * 256.0;
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

/// The fine tiles the heights at `verts` read: each vertex's at the finer zoom its cell has
/// (`Fine::zoom`), and its neighbours' where the vertex is within a pixel of their edge.
pub fn fine_tiles(verts: &[[i32; 2]], fine: &Fine) -> BTreeSet<(u8, u32, u32)> {
    let mut out = BTreeSet::new();
    for &v in verts {
        let Some(z) = fine.zoom(cell(v)) else { continue };
        let (x, y) = pixel(z, v);
        let n = 1i64 << z;
        for gx in [x - 1.0, x + 1.0] {
            for gy in [y - 1.0, y + 1.0] {
                let (tx, ty) = ((gx / 256.0).floor() as i64, (gy / 256.0).floor() as i64);
                if (0..n).contains(&ty) {
                    out.insert((z, tx.rem_euclid(n) as u32, ty as u32));
                }
            }
        }
    }
    out
}

/// The pool's datasets' codes in their order, each with its coverage (as `Cover`'s boxes): a
/// vertex's source is the first whose coverage holds it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Codes(pub Vec<(u8, Option<Vec<[f64; 4]>>)>);

impl Codes {
    /// Of the accepted datasets (`declared::accepted`, in their order).
    pub fn of(ds: &[crate::dem::declared::Accepted]) -> Codes {
        Codes(ds.iter().map(|d| (d.declaration.code, d.measured.cover.clone())).collect())
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec(self)?).with_context(|| path.display().to_string())
    }

    pub fn read(path: &Path) -> Result<Codes> {
        serde_json::from_slice(&std::fs::read(path).with_context(|| path.display().to_string())?)
            .with_context(|| format!("{}: not the datasets' codes", path.display()))
    }
}

/// The heights at `verts` from the tiles staged in `dir`, and each vertex's source code; NaN and 0
/// where no tile is stored.
pub fn heights(dir: &Path, verts: &[[i32; 2]]) -> Result<(Vec<f32>, Vec<u8>)> {
    let base = Archive::open(&dir.join(BASE_FILE))?;
    let fine = match dir.join(FINE_FILE) {
        p if p.exists() => Some(Archive::open(&p)?),
        _ => None,
    };
    let codes = Codes::read(&dir.join(CODES_FILE))?;
    let covers: Vec<(u8, Cover)> = codes.0.iter().map(|(c, b)| (*c, Cover::of(b.as_deref()))).collect();
    let mut s = Sampler { base: &base, fine: fine.as_ref(), kept: HashMap::new(), tick: 0 };
    // (Vertices in tile order, so each tile is decoded once, the kept ones few.)
    let mut order: Vec<u32> = (0..verts.len() as u32).collect();
    order.sort_unstable_by_key(|&i| {
        let (x, y) = pixel(12, verts[i as usize]);
        ((y / 256.0) as u64, (x / 256.0) as u64)
    });
    let (mut elev, mut src) = (vec![f32::NAN; verts.len()], vec![0u8; verts.len()]);
    for i in order {
        let v = verts[i as usize];
        elev[i as usize] = s.height(v)?;
        let (lon, lat) = (v[0] as f64 * 1e-7, v[1] as f64 * 1e-7);
        src[i as usize] = covers.iter().find(|(_, c)| c.contains(lon, lat)).map_or(0, |(c, _)| *c);
    }
    Ok((elev, src))
}

struct Sampler<'a> {
    base: &'a Archive,
    fine: Option<&'a Archive>,
    kept: HashMap<(u8, u32, u32), (u64, Option<Arc<Vec<f32>>>)>,
    tick: u64,
}

impl Sampler<'_> {
    /// Tile z/x/y decoded (None: not staged).
    fn tile(&mut self, z: u8, x: u32, y: u32) -> Result<Option<Arc<Vec<f32>>>> {
        self.tick += 1;
        if let Some(e) = self.kept.get_mut(&(z, x, y)) {
            e.0 = self.tick;
            return Ok(e.1.clone());
        }
        let a = if z > 12 { self.fine } else { Some(self.base) };
        let t = match a.and_then(|a| a.get(z, x, y)) {
            Some(b) => {
                Some(Arc::new(roadcore::grid::decode_terrain(b).with_context(|| format!("terrain tile {z}/{x}/{y}"))?))
            }
            None => None,
        };
        if self.kept.len() >= KEEP {
            let mut ticks: Vec<u64> = self.kept.values().map(|e| e.0).collect();
            ticks.sort_unstable();
            let cut = ticks[KEEP / 2];
            self.kept.retain(|_, e| e.0 > cut);
        }
        self.kept.insert((z, x, y), (self.tick, t.clone()));
        Ok(t)
    }

    /// The height at zoom `z` (None when a tile it reads isn't staged).
    fn at(&mut self, z: u8, v: [i32; 2]) -> Result<Option<f32>> {
        let (x, y) = pixel(z, v);
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
        let n = (1i64 << z) * 256;
        let mut c = [0f32; 4];
        for (k, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            // (Clamped at the world's top and bottom; across the antimeridian, wrapped.)
            let gx = (x0 as i64 + dx).rem_euclid(n);
            let gy = (y0 as i64 + dy).clamp(0, n - 1);
            let Some(t) = self.tile(z, (gx / 256) as u32, (gy / 256) as u32)? else { return Ok(None) };
            c[k] = t[(gy % 256) as usize * 256 + (gx % 256) as usize];
        }
        Ok(Some((c[0] * (1.0 - tx) + c[1] * tx) * (1.0 - ty) + (c[2] * (1.0 - tx) + c[3] * tx) * ty))
    }

    /// The height at the finest zoom stored there.
    fn height(&mut self, v: [i32; 2]) -> Result<f32> {
        let lat = v[1] as f64 * 1e-7;
        if self.fine.is_some() {
            for z in (13..=crate::terrain_fine::MAX_Z).rev() {
                if let Some(h) = self.at(z, v)? {
                    return Ok(h);
                }
            }
        }
        for z in (0..=crate::terrain_pack::max_zoom_at(lat)).rev() {
            if let Some(h) = self.at(z, v)? {
                return Ok(h);
            }
        }
        Ok(f32::NAN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roadcore::archive::ArchiveWriter;

    fn archive(path: &Path, tiles: &[((u8, u32, u32), Vec<f32>)]) {
        let mut v: Vec<&((u8, u32, u32), Vec<f32>)> = tiles.iter().collect();
        v.sort_by_key(|t| roadcore::archive::tile_key(t.0 .0, t.0 .1, t.0 .2));
        let mut w = ArchiveWriter::create(path, "{}").unwrap();
        for ((z, x, y), e) in v {
            let b = roadcore::grid::encode_terrain_png(e, 256, 256).unwrap();
            w.add(*z, *x, *y, &b, b.len()).unwrap();
        }
        w.finish().unwrap();
    }

    /// A vertex takes the finest stored tile's height there, bilinear between pixel centres and
    /// across a tile edge; where no fine tile is, the base's; its source the first coverage holding it.
    #[test]
    fn a_vertex_takes_the_finest_stored_terrain_there() {
        let d = tempfile::tempdir().unwrap();
        let e7 = |x: f64, y: f64| [(x * 1e7).round() as i32, (y * 1e7).round() as i32];
        // A z12 base: each tile its column times 10 m, plus the pixel's column (a slope east).
        let p = e7(10.5, 45.5);
        let (gx, gy) = pixel(12, p);
        let (bx, by) = ((gx / 256.0) as u32, (gy / 256.0) as u32);
        let slope = |off: f32| (0..65536).map(|i| off + (i % 256) as f32).collect::<Vec<f32>>();
        archive(&d.path().join(BASE_FILE), &[((12, bx, by), slope(0.0)), ((12, bx + 1, by), slope(256.0))]);
        // A fine tile at z16 over a second point, 500 m flat.
        let q = e7(11.5, 45.5);
        let (fx, fy) = pixel(16, q);
        archive(&d.path().join(FINE_FILE), &[((16, (fx / 256.0) as u32, (fy / 256.0) as u32), vec![500.0; 65536])]);
        Codes(vec![(1, Some(vec![[11.0, 45.0, 12.0, 46.0]])), (3, None)]).write(&d.path().join(CODES_FILE)).unwrap();
        // (A vertex on the base's tile edge reads both tiles: continuous.)
        let edge = {
            let lon = ((bx + 1) as f64 * 256.0 / (1u64 << 12) as f64 / 256.0) * 360.0 - 180.0;
            e7(lon, 45.5)
        };
        let (h, src) = heights(d.path(), &[p, q, edge]).unwrap();
        let want_p = (gx - 0.5 - bx as f64 * 256.0) as f32;
        assert!((h[0] - want_p).abs() < 0.01, "{} vs {want_p}", h[0]);
        assert_eq!(h[1], 500.0);
        assert!((h[2] - 255.5).abs() < 0.6, "{}", h[2]);
        assert_eq!(src, vec![3, 1, 3]);
        // The fine tiles a vertex reads: its own at its cell's zoom, and a neighbour's near an edge.
        let fine = Fine { cells: vec![([11, 45], 16)] };
        let t = fine_tiles(&[q], &fine);
        assert!(t.contains(&(16, (fx / 256.0) as u32, (fy / 256.0) as u32)) && t.len() <= 4);
        assert!(fine_tiles(&[e7(20.5, 45.5)], &fine).is_empty());
    }
}
