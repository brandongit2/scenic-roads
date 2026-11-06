//! An enclosed sea's surface (docs/steps.md §3): the depth over a sea closed off from the world
//! ocean (inputs::water::Enclosed, the water step's `global/water-enclosed`: the Caspian) is
//! measured below that sea's own surface, not below sea level. Its sources' values there are
//! heights on their own datum (GEBCO's above mean sea level), so each is taken less the surface.
//! The surface is the terrain's heights there as the server takes them for the water's surface
//! (server depth::surface): the terrain tile of the block's zoom, else its nearest stored ancestor
//! scaled up (roadcore::grid::tile_with_fallback_by), read from the terrain's lo packs (z3–8;
//! finer zooms take z8's scaled up). Nothing is typed in: the Caspian's −28 m is GLO-30's. Where
//! the lo packs have no terrain over it, the values stay as their source has them.
//!
//! The world ocean's surface is sea level (0 m), and a lake's or a river's pixels take the sources
//! drawn in them below their own surface already (`Source::drawn_in`): neither is touched.

use super::compose::{Block, PAD};
use anyhow::Result;
use inputs::water::Enclosed;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The terrain's zooms an enclosed sea's surface is read at (its lo packs').
const ZOOMS: (u8, u8) = (3, 8);

/// What a depth job reads an enclosed sea's surface from: the enclosed seas and the terrain's lo
/// packs, with the terrain tiles read for the level being made.
pub struct Surface {
    enclosed: Option<Arc<Enclosed>>,
    terrain: crate::terrain_pack::ManifestTiles,
    tiles: HashMap<(u8, u32, u32), Option<Arc<Vec<f32>>>>,
}

impl Surface {
    /// The surface the records `out` name: the enclosed seas the water step wrote (none before it
    /// wrote them: every sea the world ocean's) over the terrain's packs.
    pub fn read(out: &buildstore::out::Out) -> Result<Surface> {
        let enclosed = match out.manifest.get(inputs::water::ENCLOSED) {
            Some(c) => Some(Arc::new(Enclosed::from_bytes(&out.path(c).read()?)?)),
            None => None,
        };
        Ok(Surface {
            enclosed,
            terrain: crate::terrain_pack::ManifestTiles::new(out, "terrain"),
            tiles: HashMap::new(),
        })
    }

    /// A level done: its terrain tiles let go.
    pub fn next_level(&mut self) {
        self.tiles.clear();
    }

    /// Block `b`'s surface where it isn't sea level: each pixel on an enclosed sea the terrain's
    /// height there, 0 elsewhere; empty when no pixel of it is on one (or no terrain is under any).
    pub fn block(&mut self, b: Block) -> Result<Vec<f32>> {
        let (w, h) = (b.w(), b.h());
        let (c0, r0) = b.origin();
        let Some(e) = self.enclosed.clone() else { return Ok(Vec::new()) };
        let on = e.in_window(b.z, c0, r0, w, h);
        if on.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = vec![0f32; w * h];
        let mut any = false;
        let n = 1i64 << b.z;
        for ty in i64::from(b.ty0) - 1..=i64::from(b.ty0 + b.ny) {
            for tx in i64::from(b.tx0) - 1..=i64::from(b.tx0 + b.nx) {
                if !(0..n).contains(&ty) || !(0..n).contains(&tx) {
                    continue;
                }
                let (bx, by) = ((tx - i64::from(b.tx0)) * 256 + PAD as i64, (ty - i64::from(b.ty0)) * 256 + PAD as i64);
                let inside = |p: i64, side: usize| (0..side as i64).contains(&p);
                let pixels: Vec<(usize, usize)> = (0..256i64)
                    .filter(|py| inside(by + py, h))
                    .flat_map(|py| (0..256i64).filter(move |px| inside(bx + px, w)).map(move |px| (px, py)))
                    .map(|(px, py)| ((py * 256 + px) as usize, ((by + py) as usize) * w + (bx + px) as usize))
                    .filter(|&(_, o)| on[o])
                    .collect();
                if pixels.is_empty() {
                    continue;
                }
                let Some(t) = self.tile(b.z, tx as u32, ty as u32)? else { continue };
                for (s, o) in pixels {
                    out[o] = t[s];
                    any = true;
                }
            }
        }
        Ok(if any { out } else { Vec::new() })
    }

    /// Terrain tile z/x/y as the server serves it from the lo packs: stored, else its nearest stored
    /// ancestor there scaled up; a tile finer than them from its z8 ancestor.
    fn tile(&mut self, z: u8, x: u32, y: u32) -> Result<Option<Arc<Vec<f32>>>> {
        if let Some(t) = self.tiles.get(&(z, x, y)) {
            return Ok(t.clone());
        }
        let failed = std::cell::RefCell::new(None);
        let get = |z: u8, x: u32, y: u32| {
            if !(ZOOMS.0..=ZOOMS.1).contains(&z) {
                return None;
            }
            self.terrain.get(z, x, y).unwrap_or_else(|e| {
                failed.borrow_mut().get_or_insert(e);
                None
            })
        };
        let t = roadcore::grid::tile_with_fallback_by(&get, z, x, y).map(Arc::new);
        if let Some(e) = failed.into_inner() {
            return Err(e.context(format!("the terrain under depth {z}/{x}/{y}")));
        }
        self.tiles.insert((z, x, y), t.clone());
        Ok(t)
    }
}

/// Whether depth target `t`'s windows (crate::depth::build::windows) meet an enclosed sea of `e`:
/// it reads its surface.
pub fn meets(e: &Enclosed, t: (u8, u32, u32)) -> bool {
    crate::depth::build::windows(t).iter().any(|&(z, c0, r0, w, h)| e.meets_window(z, c0, r0, w, h))
}

/// The terrain tiles (the lo packs' zooms) depth target `t`'s surface may read (`Surface::block`):
/// those under its windows on an enclosed sea of `e`, and their ancestors. A block reads a tile
/// only where a pixel of it lies on one, else its nearest stored ancestor, and a zoom past them its
/// z8 ancestor's; so a change to the terrain elsewhere changes nothing it reads.
pub fn tiles_read(e: &Enclosed, t: (u8, u32, u32)) -> Vec<(u8, u32, u32)> {
    let mut v = Vec::new();
    for (wz, c0, r0, w, h) in crate::depth::build::windows(t) {
        for z in ZOOMS.0..=ZOOMS.1 {
            let n = 1i64 << z;
            // (A global pixel of the window's zoom to its tile at zoom z.)
            let at = |p: i64| if z >= wz { (p << (z - wz)).div_euclid(256) } else { (p >> (wz - z)).div_euclid(256) };
            let (x0, x1) = (at(c0).max(0), at(c0 + w as i64 - 1).min(n - 1));
            let (y0, y1) = (at(r0).max(0), at(r0 + h as i64 - 1).min(n - 1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if e.meets_window(z, x * 256, y * 256, 256, 256) {
                        // (With its ancestors, which it falls back to.)
                        v.extend((0..=z - ZOOMS.0).map(|d| (z - d, (x >> d) as u32, (y >> d) as u32)));
                    }
                }
            }
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The terrain's lo packs (their z3 tiles) holding the tiles depth target `t`'s surface may read
/// (`tiles_read`).
pub fn lo_packs(e: &Enclosed, t: (u8, u32, u32)) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> =
        tiles_read(e, t).into_iter().map(|(z, x, y)| (x >> (z - ZOOMS.0), y >> (z - ZOOMS.0))).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// What depth target `t` reads for an enclosed sea's surface (`Surface`), as `<logical>=<content>`
/// lines of the records `m`: the enclosed seas' file, and the terrain's lo packs holding the tiles
/// it may read (`lo_packs`) that `m` names; none where its windows meet no enclosed sea (`e`, those
/// `m` names: None, none written).
pub fn reads(m: &BTreeMap<String, String>, e: Option<&Enclosed>, t: (u8, u32, u32)) -> Reads {
    let (Some(e), Some(c)) = (e, m.get(inputs::water::ENCLOSED)) else { return Reads::default() };
    if !meets(e, t) {
        return Reads::default();
    }
    let terrain = lo_packs(e, t)
        .into_iter()
        .map(|(x, y)| crate::layers::lo_logical("terrain", x, y))
        .filter_map(|l| m.get(&l).map(|c| format!("{l}={c}")))
        .collect();
    Reads { enclosed: vec![format!("{}={c}", inputs::water::ENCLOSED)], terrain }
}

/// A depth target's reads for an enclosed sea's surface (`reads`): of the water step's, of the
/// terrain's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reads {
    pub enclosed: Vec<String>,
    pub terrain: Vec<String>,
}

impl Surface {
    /// What target `t`'s composing reads of it, as one line of lines (`reads`): a resumed run's
    /// identity.
    pub fn pin(&self, m: &BTreeMap<String, String>, t: (u8, u32, u32)) -> String {
        let r = reads(m, self.enclosed.as_deref(), t);
        r.enclosed.iter().chain(&r.terrain).cloned().collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task #982: a depth target reads an enclosed sea's surface (the enclosed seas' file, and the
    /// terrain's tiles on it under its windows, from their lo packs) only where its windows meet one:
    /// the Caspian's targets re-key, no other.
    #[test]
    fn only_a_target_meeting_an_enclosed_sea_reads_its_surface() {
        // An enclosed sea in z6 tile 40/23 (z6's pixels, 512 a tile), in z3 tile 5/2.
        let e = Enclosed { z: 6, runs: vec![[23 * 512 + 100, 40 * 512 + 10, 40 * 512 + 20]], bodies: Vec::new() };
        let m: BTreeMap<String, String> = [
            (inputs::water::ENCLOSED, "global/water-enclosed.e.bin"),
            ("layers/terrain/lo/3-5-2", "t52"),
            ("layers/terrain/lo/3-4-3", "t43"),
            ("layers/terrain/lo/3-1-1", "t11"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let lines = |t| {
            let r = reads(&m, Some(&e), t);
            (r.enclosed, r.terrain)
        };
        let enc = vec!["global/water-enclosed=global/water-enclosed.e.bin".to_string()];
        // A z6 tile's piece reads its ring and margin (z3 tiles 4–5 by 2–3), of the terrain only
        // the tiles on the sea, at each of the lo packs' zooms: in z3 tile 5/2's pack alone.
        let on = vec![(3, 5, 2), (4, 10, 5), (5, 20, 11), (6, 40, 23), (7, 80, 46), (8, 160, 92)];
        assert_eq!(tiles_read(&e, (6, 40, 23)), on);
        assert_eq!(tiles_read(&e, (6, 41, 23)), on, "its neighbour's ring");
        assert_eq!(lo_packs(&e, (6, 40, 23)), vec![(5, 2)]);
        let t = vec!["layers/terrain/lo/3-5-2=t52".to_string()];
        assert_eq!(lines((6, 40, 23)), (enc.clone(), t.clone()));
        // The assembly of 5/2 reads the same, and so does its neighbour 4/2's (its blocks' margin).
        assert_eq!(lines((3, 5, 2)), (enc.clone(), t.clone()));
        assert_eq!(lines((3, 4, 2)), (enc, t));
        assert!(tiles_read(&e, (3, 1, 1)).is_empty() && tiles_read(&e, (6, 30, 23)).is_empty());
        // Away from it, or with no enclosed sea written: nothing.
        assert_eq!(lines((3, 1, 1)), (Vec::new(), Vec::new()));
        assert_eq!(lines((6, 30, 23)), (Vec::new(), Vec::new()));
        assert_eq!(reads(&m, None, (3, 5, 2)), Reads::default());
    }
}
