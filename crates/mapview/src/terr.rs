//! Terrain lookups from the Terrarium tile archive (the same surface MapLibre renders in 3D).

use roadcore::archive::Archive;
use roadcore::grid::{bilinear, tile_with_fallback};
use std::collections::HashMap;
use std::sync::Arc;

const TZ: u8 = 12;
/// z12 pixels across the world.
const WORLD12: f64 = (1u64 << TZ) as f64 * 256.0;

/// Per-thread cache of decoded z12 tiles for point lookups.
pub struct TerrainCache<'a> {
    arc: &'a Archive,
    tiles: HashMap<(u32, u32), Option<Arc<Vec<f32>>>>,
}

impl<'a> TerrainCache<'a> {
    pub fn new(arc: &'a Archive) -> Self {
        Self { arc, tiles: HashMap::new() }
    }

    /// Elevation (m) at normalized Web-Mercator (x, y); 0 where no terrain exists.
    pub fn at(&mut self, mx: f64, my: f64) -> f32 {
        let (px, py) = (mx * WORLD12 - 0.5, my * WORLD12 - 0.5);
        let (tx, ty) = (((px + 0.5) / 256.0).floor() as u32, ((py + 0.5) / 256.0).floor() as u32);
        if self.tiles.len() > 96 {
            self.tiles.clear();
        }
        let arc = self.arc;
        let t = self.tiles.entry((tx, ty)).or_insert_with(|| tile_with_fallback(arc, TZ, tx, ty).map(Arc::new)).clone();
        match t {
            Some(t) => bilinear(&t, 256, px - tx as f64 * 256.0, py - ty as f64 * 256.0),
            None => 0.0,
        }
    }
}
