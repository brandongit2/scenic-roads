//! The terrain's z8 worldwide (docs/phase5.md "terrain-z8"): the elevation pool's base tiles at z8
//! (crate::terrain_base: the same datasets and order as the terrain and the roads), made as the
//! packs' tiles are and nothing more; the peaks' coarse stage reads it, the same whatever the
//! coverage. One pack of the 65,536 tiles (`terrain_pack::process` with no children or water) and
//! each tile's maximum after the peaks' despike (for skipping tiles in isolation searches), under
//! a logical name naming its version and the accepted elevation and sources versions the pool is
//! read from (`logical`): the agent makes it when the manifest lacks that name.

use crate::out::Out;
use crate::terrain_pack::{process, RawSource};
use anyhow::{Context, Result};
use rayon::prelude::*;
use roadcore::grid::decode_terrain;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const Z: u8 = 8;
/// Its version: a change to how it's made is a new logical name (6: lossless WebP,
/// crate::terrain_codec; 5: the elevation pool's tiles,
/// not the water: task #465).
pub const V: u32 = 6;

/// The prefix of its logical names.
const PREFIX: &str = "sources/terrain-z8-v";

/// The key `agent::input_digests` names the pool's digest by (`pool_digest`).
pub const INPUT: &str = "terrain-pool";

/// What its logical names name of the pool, of the accepted versions manifest `m` names under
/// `root`: the digest of its datasets' `keyed` digests and their heights' conversions (the geoid
/// sources they read: crate::dem::declared::tag), so a dataset or a conversion changed is a new
/// z8, and nothing else is. None while the pool can't be read (none accepted, or one from before it).
pub fn pool_digest(root: &Path, m: &BTreeMap<String, String>) -> Option<String> {
    let a = crate::inputs::open(root, m, crate::inputs::elevation::UNIT).ok()??;
    let mut ds = crate::dem::declared::accepted(&a).ok()?;
    let geoid = match crate::inputs::sources::open(root, m).ok()? {
        Some(s) => s.many(crate::inputs::sources::GEOID).to_vec(),
        None => Vec::new(),
    };
    let _ = crate::dem::declared::convert(&mut ds, &geoid);
    Some(crate::dem::declared::tag(&ds))
}

/// Its pack's logical name for pool digest `pool` (`pool_digest`).
pub fn logical(pool: &str) -> String {
    format!("{PREFIX}{V}-{pool}")
}

/// Its maxima's logical name for pool digest `pool`.
pub fn max_logical(pool: &str) -> String {
    format!("{}-max", logical(pool))
}

/// Whether `l` is one of its logical names (of this version, any pool).
pub fn is_logical(l: &str) -> bool {
    let Some(rest) = l.strip_prefix(&format!("{PREFIX}{V}-")) else { return false };
    let h = rest.strip_suffix("-max").unwrap_or(rest);
    h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Tiles made at once (the pool sampled for them together, then processed).
const CHUNK: usize = 256;

/// Makes it from the pool's base tiles `raw` (crate::terrain_base over the world) and uploads the
/// pack and the maxima under the manifest's names (`logical`). Returns (tiles, tiles no dataset has
/// data in).
pub fn build(out: &mut Out, raw: &dyn RawSource) -> Result<(usize, usize)> {
    use crate::timings::{phase, Class};
    let n = 1u32 << Z;
    let p = phase("the world's z8 tiles sampled and made", Class::Net);
    let all: Vec<(u32, u32)> = (0..n * n).map(|i| (i / n, i % n)).collect();
    let here = std::sync::atomic::AtomicU64::new(0);
    let mut made: Vec<Result<(u32, u32, Option<(Vec<u8>, f32)>)>> = Vec::with_capacity(all.len());
    for (k, chunk) in all.chunks(CHUNK).enumerate() {
        raw.prefetch_counted(Z, chunk, 1, &here)?;
        crate::agent::jobs::report((k * CHUNK) as u64, (n * n) as u64, "z8 tiles");
        made.par_extend(chunk.par_iter().map(|&(x, y)| -> Result<(u32, u32, Option<(Vec<u8>, f32)>)> {
            let (b, _) = raw.get(Z, x, y)?;
            let Some(b) = b else { return Ok((x, y, None)) };
            let (tile, _, _) = process(b, Z, x, y, &HashMap::new(), &crate::terrain_pack::Sources::default());
            let Some(tile) = tile else { return Ok((x, y, None)) };
            let mut e = decode_terrain(&tile).with_context(|| format!("8/{x}/{y}"))?;
            crate::peaks::despike(&mut e, Z, crate::peaks::tile_lat(Z, y));
            let mx = e.iter().cloned().fold(0f32, f32::max);
            Ok((x, y, Some((tile, mx))))
        }));
    }
    let mut tiles: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    let mut maxes = vec![0f32; (n * n) as usize];
    let mut none = 0;
    for m in made {
        let (x, y, t) = m?;
        match t {
            Some((png, mx)) => {
                maxes[(y * n + x) as usize] = mx;
                tiles.push((x, y, png));
            }
            None => none += 1,
        }
    }
    drop(p);
    // In key order (x, then y, at one zoom).
    let p = phase("the pack written", Class::Disk);
    tiles.sort_by_key(|t| (t.0, t.1));
    let local = out.scratch_file("terrain-z8.pack");
    let mut w = store::pack::PackWriter::create(
        &local,
        serde_json::json!({"layer": "terrain-z8", "encoding": crate::terrain_codec::ENCODING, "version": V}),
        false,
    )?;
    for (x, y, png) in &tiles {
        w.add(Z, *x, *y, png, png.len() as u32)?;
    }
    w.finish()?;
    let mfile = out.scratch_file("terrain-z8.max.f32");
    std::fs::write(&mfile, bytemuck::cast_slice(&maxes))?;
    drop(p);
    let p = phase("uploaded", Class::NasWrite);
    let pool = pool_digest(out.root(), &out.manifest).context("the elevation pool can't be read")?;
    let (l, ml) = (logical(&pool), max_logical(&pool));
    out.put_file(&l, "pack", &local)?;
    out.put_file(&ml, "f32", &mfile)?;
    drop(p);
    out.save()?;
    std::fs::remove_file(&local).ok();
    std::fs::remove_file(&mfile).ok();
    Ok((tiles.len(), none))
}

/// The artifact, read: tiles decoded and despiked on demand, kept in a bounded cache shared by
/// the threads; each tile's maximum. A tile no dataset has data in is the open sea (0 m).
pub struct Z8 {
    file: std::fs::File,
    idx: store::pack::PackIndex,
    maxes: Vec<f32>,
    cache: Mutex<HashMap<(u32, u32), Arc<Vec<f32>>>>,
    cap: usize,
}

struct FileSource<'a>(&'a std::fs::File, u64);

impl store::range::RangeRead for FileSource<'_> {
    fn len(&self) -> Result<u64, store::iopool::IoError> {
        Ok(self.1)
    }
    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::iopool::IoError> {
        use store::sys::PosIo;
        let mut b = vec![0u8; len];
        self.0.read_exact_at(&mut b, off).map_err(store::iopool::IoError::Io)?;
        Ok(b)
    }
}

impl Z8 {
    /// `cap`: decoded tiles kept (256 KB each).
    pub fn open(pack: &Path, maxes: &Path, cap: usize) -> Result<Z8> {
        let file = std::fs::File::open(pack).with_context(|| format!("open {}", pack.display()))?;
        let len = file.metadata()?.len();
        let idx = store::pack::PackIndex::read_from(&FileSource(&file, len))?;
        let maxes: Vec<f32> = bytemuck::pod_collect_to_vec(&std::fs::read(maxes)?);
        anyhow::ensure!(maxes.len() == 1 << (2 * Z), "terrain-z8 maxima: {} values", maxes.len());
        Ok(Z8 { file, idx, maxes, cache: Mutex::new(HashMap::new()), cap })
    }

    /// The tile's highest pixel (despiked; 0 for the open sea).
    pub fn max(&self, x: u32, y: u32) -> f32 {
        self.maxes[(y * (1 << Z) + x) as usize]
    }

    pub fn tile(&self, x: u32, y: u32) -> Result<Arc<Vec<f32>>> {
        if let Some(t) = self.cache.lock().unwrap().get(&(x, y)) {
            return Ok(t.clone());
        }
        let len = self.file.metadata()?.len();
        let t = match self.idx.get(&FileSource(&self.file, len), Z, x, y)? {
            Some((_, png)) => {
                let mut e = decode_terrain(&png).with_context(|| format!("terrain-z8 8/{x}/{y}"))?;
                crate::peaks::despike(&mut e, Z, crate::peaks::tile_lat(Z, y));
                Arc::new(e)
            }
            None => Arc::new(vec![0f32; 256 * 256]),
        };
        let mut c = self.cache.lock().unwrap();
        if c.len() >= self.cap {
            c.clear();
        }
        c.insert((x, y), t.clone());
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS's own values where its sources meet (roadcore::grid's tests): 9/145/195 around pixel
    /// 113,22, an 880 m tower beside a pit to −3 m, which the blobs' rules leave and the seam rule
    /// takes (a ringing; with its pit at 130 m, a needle, 2.6 pixel widths out of every pixel beside
    /// it).
    const MARYLAND_Z9: [[i16; 15]; 15] = [
        [88, 75, 76, 80, 97, 138, 132, 160, 184, 186, 212, 267, 355, 416, 355],
        [121, 97, 80, 77, 82, 89, 95, 147, 171, 191, 233, 292, 382, 398, 324],
        [179, 154, 114, 93, 80, 72, 79, 91, 133, 189, 257, 332, 387, 338, 248],
        [166, 147, 145, 147, 116, 90, 94, 93, 80, 124, 222, 320, 322, 294, 207],
        [155, 147, 151, 148, 139, 138, 128, 114, 75, 99, 138, 254, 233, 237, 178],
        [154, 153, 152, 134, 135, 164, 159, 144, 113, 81, 92, 143, 124, 112, 93],
        [153, 150, 142, 111, 115, 117, 105, 82, 108, 75, 71, 64, 62, 63, 73],
        [149, 141, 117, 84, 93, 81, -3, 880, 71, 182, 153, 146, 161, 144, 90],
        [147, 134, 88, 91, 109, 116, 118, 139, 192, 264, 273, 268, 258, 188, 125],
        [146, 97, 101, 128, 175, 173, 215, 212, 299, 330, 305, 256, 209, 163, 143],
        [96, 87, 132, 190, 189, 213, 266, 318, 338, 312, 266, 213, 175, 142, 143],
        [117, 116, 165, 204, 220, 271, 322, 348, 311, 250, 204, 175, 155, 126, 131],
        [106, 152, 179, 202, 256, 321, 358, 330, 262, 211, 165, 144, 134, 130, 135],
        [138, 170, 171, 208, 286, 349, 355, 293, 232, 196, 170, 138, 136, 148, 146],
        [163, 179, 171, 223, 307, 350, 324, 257, 206, 176, 167, 143, 140, 159, 156],
    ];

    /// A raw tile: a slope with a tower, a pit, a void and bathymetry, and Maryland's ringing and,
    /// beside it, its needle (AWS's values, on ground at 150 m).
    fn raw_tile() -> Vec<f32> {
        let mut e: Vec<f32> = (0..256 * 256).map(|i| 200.0 + 0.5 * (i % 256) as f32 + 0.3 * (i / 256) as f32).collect();
        for y in 100..104 {
            for x in 100..104 {
                e[y * 256 + x] += 900.0;
            }
        }
        e[50 * 256 + 50] -= 800.0;
        e[70 * 256 + 180] = -10000.0;
        for v in e[230 * 256..].iter_mut() {
            *v = -50.0;
        }
        for (ox, needle) in [(140, false), (170, true)] {
            for j in 0..25 {
                for i in 0..25 {
                    e[(140 + j) * 256 + ox + i - 5] = 150.0;
                }
            }
            for (j, row) in MARYLAND_Z9.iter().enumerate() {
                for (i, &v) in row.iter().enumerate() {
                    e[(145 + j) * 256 + ox + i] = v as f32;
                }
            }
            if needle {
                e[(145 + 7) * 256 + ox + 6] = 130.0;
            }
        }
        e
    }

    /// What `process` makes of `raw_tile` with no sources, as the z8 here and the peaks' z12
    /// outside the packs read it (crate::peaks::unit::UnitZ12), at z8, z9 (Maryland's tile) and
    /// z12: each tile's elevations as decoded (not its PNG's bytes, which an encoder's change would
    /// change alone), by their bits.
    fn raw_path() -> Vec<u64> {
        let e = raw_tile();
        let png = roadcore::grid::encode_terrain_png(&e, 256, 256).unwrap();
        let input = roadcore::grid::decode_terrain(&png).unwrap();
        [(Z, 40, 90), (9, 145, 195), (12, 640, 1440)]
            .iter()
            .map(|&(z, x, y)| {
                let (out, _, _) =
                    process(png.clone(), z, x, y, &HashMap::new(), &crate::terrain_pack::Sources::default());
                let got = roadcore::grid::decode_terrain(&out.unwrap()).unwrap();
                // (Its voids and bathymetry change it: the values below see them.)
                assert_ne!(got, input);
                let bits: Vec<u8> = got.iter().flat_map(|v| v.to_bits().to_le_bytes()).collect();
                store::naming::xxh3(&bits)
            })
            .collect()
    }

    /// Its names name the pool's digest.
    #[test]
    fn its_names_are_the_pools() {
        let d = tempfile::tempdir().unwrap();
        let mut m = BTreeMap::new();
        assert_eq!(pool_digest(d.path(), &m), None);
        crate::inputs::elevation::accept_fixture(d.path(), &mut m);
        let pool = pool_digest(d.path(), &m).unwrap();
        let (a, b) = (logical(&pool), logical("0123456789abcdef"));
        assert!(a != b && b.starts_with("sources/terrain-z8-v6-"));
        assert_eq!(max_logical(&pool), format!("{a}-max"));
        assert!(is_logical(&a) && is_logical(&max_logical(&pool)));
        assert!(
            !is_logical("sources/terrain-z8-v5")
                && !is_logical("sources/terrain-z8-v6-x")
                && !is_logical(&format!("{b}-other"))
        );
    }

    #[test]
    fn the_raw_path_is_versioned() {
        // A change here is a change to the z8 (bump `V`: a new logical name, so the summits and
        // every unit's peaks again) and to the peaks' z12 outside the packs (bump
        // crate::agent::build::TERRAIN_V, which the peaks' key names); then update the values.
        assert_eq!(
            (V, crate::agent::build::TERRAIN_V),
            (6, 7),
            "the raw path's versions changed: update the values below"
        );
        assert_eq!(
            raw_path(),
            vec![12970933280215579373, 12970933280215579373, 12970933280215579373],
            "terrain_pack::process's raw path makes other elevations: bump terrain_z8::V and TERRAIN_V"
        );
    }
}
