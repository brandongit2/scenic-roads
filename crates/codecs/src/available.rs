//! The data-available layers' tiles and packs (docs/steps.md §14; docs/formats.md, Data available):
//! a tile holds, per pixel, an index into its own palette of piece sets, each set the ids
//! (`<source>/<piece>`) of the source extents' pieces (docs/inputs.md §6.11) whose outline holds
//! the pixel's centre; above the finest zoom a pixel's set is the union of the sets under it. No
//! score is stored: the server scores the sets as it serves them.
//!
//! The packs: the Web Mercator layer `available-<theme>` holds z0–5 in its root pack and z6–12 in a
//! pack per z6 tile (`layers/<layer>/hi/6-x-y`); a polar cap's layer `available-<theme>-<n|s>`
//! holds z0–3 of the pole's grid (roadcore::polar) in its root. A tile all of one set at the finest
//! zoom under it isn't stored (z0 aside): its nearest stored ancestor's pixels say it.
//!
//! A tile's bytes (stored gzip'd, the pack's blobs flag):
//!
//! ```text
//! "RDAV" 1          magic and format
//! u8 width          bytes a pixel: 1 (256 sets or fewer), else 2
//! u32 ids           the piece ids it names, then each: u16 length, UTF-8 bytes (sorted)
//! u32 sets          its palette, then each set: u32 count, count × u32 (indices into the ids, ascending)
//! 256 × 256 pixels  rows from the top, each an index into the palette (little-endian)
//! ```
//!
//! All integers little-endian. The palette is in the order its sets first appear in the pixels, so a
//! tile's bytes depend on its pieces alone.

use anyhow::{bail, ensure, Context, Result};
use std::io::{Read, Write};

/// A tile's side in pixels.
pub const SIDE: usize = 256;
/// Pixels a tile.
pub const PIXELS: usize = SIDE * SIDE;
/// The finest zoom of the Web Mercator layer (a pixel 38 m at the equator).
pub const MAXZ: u8 = 12;
/// The zoom of its packs below the root: z6 to `MAXZ` in each.
pub const PACK_Z: u8 = 6;
/// The finest zoom of a polar cap's layer (its root's: a pixel 1,024 m of the grid).
pub const POLAR_MAXZ: u8 = 3;
/// The tiles' encoding, as the catalogue names it.
pub const ENCODING: &str = "avail-sets";
const MAGIC: &[u8; 4] = b"RDAV";
const FMT: u8 = 1;

/// A tile: its piece ids, its palette of sets (indices into `ids`), and each pixel's palette index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub ids: Vec<String>,
    pub sets: Vec<Vec<u32>>,
    /// `PIXELS` indices into `sets`, rows from the top.
    pub px: Vec<u16>,
}

impl Tile {
    /// The pieces of the set at pixel (`i`, `j`) (column, row), by id.
    pub fn at(&self, i: usize, j: usize) -> impl Iterator<Item = &str> {
        let s = &self.sets[self.px[j * SIDE + i] as usize];
        s.iter().map(|&k| self.ids[k as usize].as_str())
    }

    /// The tile's bytes, not gzip'd.
    pub fn encode(&self) -> Vec<u8> {
        let width = if self.sets.len() <= 256 { 1 } else { 2 };
        let mut b = Vec::with_capacity(16 + PIXELS * width);
        b.extend_from_slice(MAGIC);
        b.push(FMT);
        b.push(width as u8);
        b.extend_from_slice(&(self.ids.len() as u32).to_le_bytes());
        for id in &self.ids {
            b.extend_from_slice(&(id.len() as u16).to_le_bytes());
            b.extend_from_slice(id.as_bytes());
        }
        b.extend_from_slice(&(self.sets.len() as u32).to_le_bytes());
        for s in &self.sets {
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            for k in s {
                b.extend_from_slice(&k.to_le_bytes());
            }
        }
        for &p in &self.px {
            if width == 1 {
                b.push(p as u8);
            } else {
                b.extend_from_slice(&p.to_le_bytes());
            }
        }
        b
    }

    /// A tile from its bytes (gzip'd or not).
    pub fn decode(b: &[u8]) -> Result<Tile> {
        let raw;
        let b = if b.starts_with(&[0x1f, 0x8b]) {
            let mut v = Vec::new();
            flate2::read::GzDecoder::new(b).read_to_end(&mut v).context("an available tile's gzip")?;
            raw = v;
            &raw[..]
        } else {
            b
        };
        ensure!(b.len() >= 6 && &b[..4] == MAGIC, "not an available tile");
        ensure!(b[4] == FMT, "an available tile of format {}", b[4]);
        let width = b[5] as usize;
        ensure!(width == 1 || width == 2, "an available tile of {width} bytes a pixel");
        let mut at = 6;
        let mut take = |n: usize| -> Result<&[u8]> {
            let s = b.get(at..at + n).context("an available tile cut short")?;
            at += n;
            Ok(s)
        };
        let u32_of = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        let n = u32_of(take(4)?) as usize;
        let mut ids = Vec::with_capacity(n);
        for _ in 0..n {
            let l = take(2)?;
            let l = u16::from_le_bytes([l[0], l[1]]) as usize;
            ids.push(std::str::from_utf8(take(l)?).context("a piece id")?.to_string());
        }
        let n = u32_of(take(4)?) as usize;
        let mut sets = Vec::with_capacity(n);
        for _ in 0..n {
            let c = u32_of(take(4)?) as usize;
            let s: Vec<u32> = take(c * 4)?.chunks_exact(4).map(u32_of).collect();
            if s.iter().any(|&k| k as usize >= ids.len()) {
                bail!("an available tile's set names a piece it hasn't");
            }
            sets.push(s);
        }
        let p = take(PIXELS * width)?;
        let px: Vec<u16> = if width == 1 {
            p.iter().map(|&v| u16::from(v)).collect()
        } else {
            p.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
        };
        if px.iter().any(|&v| v as usize >= sets.len()) {
            bail!("an available tile's pixel names a set it hasn't");
        }
        Ok(Tile { ids, sets, px })
    }

    /// The tile's bytes gzip'd, as a pack stores them (no time in the header: the same tile, the
    /// same bytes).
    pub fn gzipped(&self) -> Vec<u8> {
        gzip(&self.encode())
    }

    /// Pixels where `keep` (column, row) is false set to the empty set (added to the palette when
    /// it has none): a tile cut to where the downloads reach.
    pub fn mask(&mut self, keep: &dyn Fn(usize, usize) -> bool) {
        let empty = match self.sets.iter().position(Vec::is_empty) {
            Some(e) => e as u16,
            None => {
                self.sets.push(Vec::new());
                (self.sets.len() - 1) as u16
            }
        };
        for j in 0..SIDE {
            for i in 0..SIDE {
                if !keep(i, j) {
                    self.px[j * SIDE + i] = empty;
                }
            }
        }
    }
}

/// `b` gzip'd with no time or name in its header.
pub fn gzip(b: &[u8]) -> Vec<u8> {
    let mut e =
        flate2::GzBuilder::new().mtime(0).write(Vec::with_capacity(b.len() / 4 + 64), flate2::Compression::default());
    e.write_all(b).expect("gzip to memory");
    e.finish().expect("gzip to memory")
}

/// Which grid a layer's tiles are of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grid {
    /// Web Mercator's, z0–`MAXZ`.
    Mercator,
    /// A polar cap's (roadcore::polar), z0–`POLAR_MAXZ`, by its pole's letter.
    Polar(char),
}

/// The Web Mercator layer of `theme`'s best data (docs/steps.md §14: `available-<theme>`).
pub fn layer(theme: &str) -> String {
    format!("available-{theme}")
}

/// The polar cap's layer of `theme` at pole `p` (`n` or `s`).
pub fn polar_layer(theme: &str, p: char) -> String {
    format!("available-{theme}-{p}")
}

/// What a layer name is: (its theme, its grid), or None for a layer not of these.
pub fn of_layer(name: &str) -> Option<(&str, Grid)> {
    let rest = name.strip_prefix("available-")?;
    match rest.rsplit_once('-') {
        Some((theme, p @ ("n" | "s"))) => Some((theme, Grid::Polar(p.chars().next()?))),
        _ => Some((rest, Grid::Mercator)),
    }
}

/// A layer's root pack (z0–5 of the Mercator layer, z0–3 of a polar one).
pub fn root_logical(layer: &str) -> String {
    format!("layers/{layer}/root/0-0-0")
}

/// The Mercator layer's pack of z6 tile (`x`, `y`) (z6–12 under it).
pub fn pack_logical(layer: &str, x: u32, y: u32) -> String {
    format!("layers/{layer}/hi/{PACK_Z}-{x}-{y}")
}

/// The step's own record of a theme's build (`work/available-<theme>/mids`): an RDPACK of every z6
/// tile holding a piece at z6 (the root is made from them), its meta each pack's key.
pub fn mids_logical(theme: &str) -> String {
    format!("work/available-{theme}/mids")
}

/// The pack holding tile z/x/y of a layer of `grid`, as the catalogue keys it: ("root", "0/0/0") or
/// ("hi", "6/x/y"); None past the layer's zooms.
pub fn pack_of(grid: Grid, z: u8, x: u32, y: u32) -> Option<(&'static str, String)> {
    match grid {
        Grid::Mercator if z < PACK_Z => Some(("root", "0/0/0".into())),
        Grid::Mercator if z <= MAXZ => {
            let d = z - PACK_Z;
            Some(("hi", format!("{PACK_Z}/{}/{}", x >> d, y >> d)))
        }
        Grid::Polar(_) if z <= POLAR_MAXZ => Some(("root", "0/0/0".into())),
        _ => None,
    }
}

/// The finest zoom of a layer of `grid`.
pub fn maxz(grid: Grid) -> u8 {
    match grid {
        Grid::Mercator => MAXZ,
        Grid::Polar(_) => POLAR_MAXZ,
    }
}

/// A stored tile of a layer, by zoom, x and y (None: not stored).
pub type Get<'a> = dyn FnMut(u8, u32, u32) -> Result<Option<Tile>> + 'a;

/// Tile z/x/y of a layer of `grid` as served: the stored tile, else the part of its nearest stored
/// ancestor over it, each of the ancestor's pixels drawn as the pixels it covers (a tile not stored
/// is all one set, which its ancestor's pixels there say). None with no stored ancestor (no piece
/// there) or past the layer's zooms.
pub fn tile_at(grid: Grid, z: u8, x: u32, y: u32, get: &mut Get) -> Result<Option<Tile>> {
    if z > maxz(grid) || x >> z != 0 || y >> z != 0 {
        return Ok(None);
    }
    for za in (0..=z).rev() {
        let d = z - za;
        let Some(t) = get(za, x >> d, y >> d)? else { continue };
        if d == 0 {
            return Ok(Some(t));
        }
        // (The ancestor's pixels over the tile: a square 256 >> d a side, from its offset.)
        let n = 1u32 << d;
        let (ox, oy) = ((x % n) as usize * SIDE, (y % n) as usize * SIDE);
        let px = (0..PIXELS)
            .map(|k| {
                let (i, j) = ((ox + k % SIDE) >> d, (oy + k / SIDE) >> d);
                t.px[j * SIDE + i]
            })
            .collect();
        return Ok(Some(Tile { px, ..t }));
    }
    Ok(None)
}

/// The pieces at pixel (`px`, `py`) of the finest zoom of a layer of `grid` (pixels across its
/// whole plane), by id: its set in the finest stored tile over it.
pub fn pieces_at(grid: Grid, px: u64, py: u64, get: &mut Get) -> Result<Vec<String>> {
    let zm = maxz(grid);
    let side = SIDE as u64;
    let Some(t) = tile_at(grid, zm, (px / side) as u32, (py / side) as u32, get)? else { return Ok(Vec::new()) };
    Ok(t.at((px % side) as usize, (py % side) as usize).map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile() -> Tile {
        let mut px = vec![0u16; PIXELS];
        for (k, p) in px.iter_mut().enumerate() {
            *p = (k % 3) as u16;
        }
        Tile { ids: vec!["a/1".into(), "b/2".into()], sets: vec![vec![], vec![0], vec![0, 1]], px }
    }

    #[test]
    fn a_tile_round_trips() {
        let t = tile();
        assert_eq!(Tile::decode(&t.encode()).unwrap(), t);
        assert_eq!(Tile::decode(&t.gzipped()).unwrap(), t);
        assert_eq!(t.gzipped(), t.gzipped(), "the same bytes each time");
        assert_eq!(t.at(2, 0).collect::<Vec<_>>(), ["a/1", "b/2"]);
        // (More than 256 sets: two bytes a pixel.)
        let mut w = t.clone();
        w.sets = (0..300).map(|k| vec![k % 2]).collect();
        w.px = (0..PIXELS).map(|k| (k % 300) as u16).collect();
        assert_eq!(w.encode()[5], 2);
        assert_eq!(Tile::decode(&w.encode()).unwrap(), w);
        assert!(Tile::decode(&t.encode()[..100]).is_err());
    }

    #[test]
    fn masking_empties_the_pixels_outside() {
        let mut t = tile();
        t.sets.remove(0);
        for p in t.px.iter_mut() {
            *p = 1;
        }
        t.mask(&|i, _| i < 128);
        assert_eq!(t.sets.len(), 3);
        assert_eq!(t.at(0, 0).count(), 2);
        assert_eq!(t.at(200, 0).count(), 0);
    }

    #[test]
    fn layers_and_packs() {
        assert_eq!(of_layer("available-terrain"), Some(("terrain", Grid::Mercator)));
        assert_eq!(of_layer(&polar_layer("terrain", 'n')), Some(("terrain", Grid::Polar('n'))));
        assert_eq!(of_layer("terrain"), None);
        assert_eq!(pack_of(Grid::Mercator, 5, 3, 3), Some(("root", "0/0/0".into())));
        assert_eq!(pack_of(Grid::Mercator, 12, 4095, 64), Some(("hi", "6/63/1".into())));
        assert_eq!(pack_of(Grid::Mercator, 13, 0, 0), None);
        assert_eq!(pack_of(Grid::Polar('s'), 3, 7, 7), Some(("root", "0/0/0".into())));
        assert_eq!(pack_of(Grid::Polar('s'), 4, 0, 0), None);
    }
}
