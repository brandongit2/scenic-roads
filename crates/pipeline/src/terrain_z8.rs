//! AWS's z8 terrain worldwide, repaired as the packs' tiles are and nothing more (docs/phase5.md
//! "terrain-z8"): the peaks' coarse stage reads it, the same whatever the coverage. One pack of
//! the 65,536 tiles (`terrain_pack::process` with no children) and each tile's maximum after the
//! peaks' despike (for skipping tiles in isolation searches), under a versioned logical name: the
//! agent makes it when the manifest lacks the current version.

use crate::out::Out;
use crate::terrain_pack::{process, RawTiles};
use anyhow::{Context, Result};
use rayon::prelude::*;
use roadcore::grid::decode_terrain_png;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const Z: u8 = 8;
/// Its version: a change to how it's made is a new logical name.
pub const V: u32 = 1;

pub fn logical() -> String {
    format!("sources/terrain-z8-v{V}")
}

pub fn max_logical() -> String {
    format!("sources/terrain-z8-v{V}-max")
}

/// Makes it from the raw tiles (fetched into `raw`'s cache when missing; a fetch that fails fails
/// the build) and uploads the pack and the maxima. Returns (tiles, tiles AWS has none of).
pub fn build(out: &mut Out, raw: &RawTiles) -> Result<(usize, usize)> {
    let n = 1u32 << Z;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let made: Vec<Result<(u32, u32, Option<(Vec<u8>, f32)>)>> = (0..n * n)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i / n, i % n);
            let (b, _) = raw.get(Z, x, y)?;
            let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if k % 4096 == 0 {
                eprintln!("  terrain-z8 {k}/{}", n * n);
            }
            let Some(b) = b else { return Ok((x, y, None)) };
            let (png, _, _) = process(b, Z, x, y, &HashMap::new(), &HashMap::new());
            let mut e = decode_terrain_png(&png).with_context(|| format!("8/{x}/{y}"))?;
            crate::peaks::despike(&mut e, Z, crate::peaks::tile_lat(Z, y));
            let mx = e.iter().cloned().fold(0f32, f32::max);
            Ok((x, y, Some((png, mx))))
        })
        .collect();
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
    // In key order (x, then y, at one zoom).
    tiles.sort_by_key(|t| (t.0, t.1));
    let local = out.scratch_file("terrain-z8.pack");
    let mut w = store::pack::PackWriter::create(&local, serde_json::json!({"layer": "terrain-z8", "encoding": "terrarium-png", "version": V}), false)?;
    for (x, y, png) in &tiles {
        w.add(Z, *x, *y, png, png.len() as u32)?;
    }
    w.finish()?;
    let mfile = out.scratch_file("terrain-z8.max.f32");
    std::fs::write(&mfile, bytemuck::cast_slice(&maxes))?;
    out.put_file(&logical(), "pack", &local)?;
    out.put_file(&max_logical(), "f32", &mfile)?;
    out.save()?;
    std::fs::remove_file(&local).ok();
    std::fs::remove_file(&mfile).ok();
    Ok((tiles.len(), none))
}

/// The artifact, read: tiles decoded and despiked on demand, kept in a bounded cache shared by
/// the threads; each tile's maximum. A tile AWS has none of is the open sea (0 m).
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
        use std::os::unix::fs::FileExt;
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
                let mut e = decode_terrain_png(&png).with_context(|| format!("terrain-z8 8/{x}/{y}"))?;
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
