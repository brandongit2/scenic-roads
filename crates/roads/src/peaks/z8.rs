//! The terrain packs' z8, as the peaks' coarse stage and the summits read it (docs/phase5.md
//! "peaks"): each terrain lo pack's z8 tiles (`layers/terrain/lo/3-x-y`, terrain-lo's, made from
//! the built z9 tiles), read from local copies, decoded and despiked on demand and kept in a
//! bounded cache shared by the threads; each tile's highest pixel worked out on its first use and
//! kept. Where nothing is built (no lo pack, or a pack without the tile) there are no heights: 0 m,
//! as the open sea.

use anyhow::{Context, Result};
use roadcore::grid::decode_terrain;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const Z: u8 = 8;

/// The lo packs' logical names' prefix.
const LO: &str = "layers/terrain/lo/";

/// The terrain lo packs manifest `m` names: (logical, z3 tile).
pub fn lo_packs(m: &BTreeMap<String, String>) -> Vec<(String, (u32, u32))> {
    m.range(LO.to_string()..)
        .take_while(|(l, _)| l.starts_with(LO))
        .filter_map(|(l, _)| {
            let mut it = l[LO.len()..].split('-');
            (it.next() == Some("3")).then_some(())?;
            let x = it.next()?.parse().ok()?;
            let y = it.next()?.parse().ok()?;
            Some((l.clone(), (x, y)))
        })
        .collect()
}

/// What the z8 is made of in manifest `m`: the digest of its lo packs' logical and content names
/// (the summits' and the peaks' keys name it: a lo pack made again is a new z8).
pub fn digest(m: &BTreeMap<String, String>) -> String {
    let named: Vec<String> =
        m.range(LO.to_string()..).take_while(|(l, _)| l.starts_with(LO)).map(|(l, c)| format!("{l}={c}")).collect();
    format!("{:016x}", store::naming::xxh3(named.join(",").as_bytes()))
}

struct Pack {
    file: std::fs::File,
    idx: store::pack::PackIndex,
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

/// The z8, read.
pub struct Z8 {
    /// By z3 tile.
    packs: HashMap<(u32, u32), Pack>,
    maxes: Mutex<HashMap<(u32, u32), f32>>,
    cache: Mutex<HashMap<(u32, u32), Arc<Vec<f32>>>>,
    cap: usize,
}

impl Z8 {
    /// From the lo packs' files `packs` (by z3 tile); `cap`: decoded tiles kept (256 KB each).
    pub fn open(packs: &[((u32, u32), PathBuf)], cap: usize) -> Result<Z8> {
        let mut by = HashMap::new();
        for (q, p) in packs {
            let file = std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
            let len = file.metadata()?.len();
            let idx = store::pack::PackIndex::read_from(&FileSource(&file, len))
                .with_context(|| format!("the index of {}", p.display()))?;
            by.insert(*q, Pack { file, idx });
        }
        Ok(Z8 { packs: by, maxes: Mutex::new(HashMap::new()), cache: Mutex::new(HashMap::new()), cap })
    }

    /// Tile `x`/`y` decoded and despiked; None where nothing is built (no lo pack, or not in it).
    fn decode(&self, x: u32, y: u32) -> Result<Option<Vec<f32>>> {
        let Some(p) = self.packs.get(&(x >> (Z - 3), y >> (Z - 3))) else { return Ok(None) };
        let len = p.file.metadata()?.len();
        let Some((_, png)) = p.idx.get(&FileSource(&p.file, len), Z, x, y)? else { return Ok(None) };
        let mut e = decode_terrain(&png).with_context(|| format!("the terrain's z8 8/{x}/{y}"))?;
        crate::peaks::despike(&mut e, Z, crate::peaks::tile_lat(Z, y));
        Ok(Some(e))
    }

    fn max_of(e: &[f32]) -> f32 {
        e.iter().cloned().fold(0f32, f32::max)
    }

    /// The tile's elevations, despiked (zeros where nothing is built).
    pub fn tile(&self, x: u32, y: u32) -> Result<Arc<Vec<f32>>> {
        if let Some(t) = self.cache.lock().unwrap().get(&(x, y)) {
            return Ok(t.clone());
        }
        let t = Arc::new(self.decode(x, y)?.unwrap_or_else(|| vec![0f32; 256 * 256]));
        self.maxes.lock().unwrap().entry((x, y)).or_insert_with(|| Self::max_of(&t));
        let mut c = self.cache.lock().unwrap();
        if c.len() >= self.cap {
            c.clear();
        }
        c.insert((x, y), t.clone());
        Ok(t)
    }

    /// The tile's highest pixel (despiked; 0 where nothing is built). Worked out on its first use
    /// and kept; the tile it decodes isn't kept (the isolation searches ask far more tiles' maxima
    /// than they read: kept, they'd push the floods' tiles out of the cache).
    pub fn max(&self, x: u32, y: u32) -> Result<f32> {
        if let Some(m) = self.maxes.lock().unwrap().get(&(x, y)) {
            return Ok(*m);
        }
        let m = self.decode(x, y)?.map_or(0.0, |e| Self::max_of(&e));
        self.maxes.lock().unwrap().insert((x, y), m);
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_packs_and_digest_are_the_lo_packs() {
        let mut m: BTreeMap<String, String> = [
            ("layers/terrain/hi/6-1-1", "a"),
            ("layers/terrain/lo/3-2-5", "b"),
            ("layers/terrain/lo/3-10-0", "c"),
            ("layers/terrain/root/0-0-0", "d"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let got: Vec<(u32, u32)> = lo_packs(&m).into_iter().map(|p| p.1).collect();
        assert_eq!(got, vec![(10, 0), (2, 5)]);
        let d = digest(&m);
        m.insert("layers/terrain/hi/6-1-1".into(), "x".into());
        assert_eq!(digest(&m), d, "the hi packs aren't the z8");
        m.insert("layers/terrain/lo/3-2-5".into(), "x".into());
        assert_ne!(digest(&m), d);
    }

    /// A tile in a lo pack is read with its maximum; elsewhere nothing is built: 0 m.
    #[test]
    fn tiles_from_the_lo_packs_else_none() {
        let d = tempfile::tempdir().unwrap();
        let e: Vec<f32> = (0..256 * 256).map(|i| 100.0 + (i % 256) as f32).collect();
        let png = roadcore::grid::encode_terrain_png(&e, 256, 256).unwrap();
        let p = d.path().join("lo.pack");
        let mut w = store::pack::PackWriter::create(&p, serde_json::json!({"layer": "terrain"}), false).unwrap();
        w.add(Z, 40, 90, &png, png.len() as u32).unwrap();
        w.finish().unwrap();
        let z8 = Z8::open(&[((40 >> 5, 90 >> 5), p)], 4).unwrap();
        assert_eq!(z8.max(40, 90).unwrap(), 355.0);
        assert_eq!(z8.tile(40, 90).unwrap()[255], 355.0);
        assert_eq!(z8.max(41, 90).unwrap(), 0.0);
        assert_eq!(z8.max(200, 9).unwrap(), 0.0);
        assert!(z8.tile(200, 9).unwrap().iter().all(|&v| v == 0.0));
    }
}
