//! The cut to one side of the coastline (docs/steps.md §14): a theme's layers hold a piece only on
//! its side, whatever its outline (an index's coastal tiles, a 1° tile, a block drawn over the sea).
//! The side is the theme's (`SIDES`): the terrain's and the buildings' the land, bathymetry's the
//! water: the sea, and the lakes, reservoirs and rivers inland (`Cut::sea`, super::inland).
//!
//! - **The land** is the coastline's, the pass's own (inputs::inputs::regions::land, the
//!   `regions-land` step's, by 2° cell): OSM's coastline, so a lake is land (the coastline goes round
//!   it: a terrain model's heights there are its water's surface, which the map's terrain uses) and
//!   the Caspian is sea (OSM maps it with coastline).
//! - **The water's side** is the land's mask inverted with the water layer's inland water added
//!   (super::inland: its z9 pixels, 153 m at the equator, whose inland share is half or more), so a
//!   lake's depth (GLOBathy's, a lake's survey) is kept. Past 85.05°, where the water layer has no
//!   tiles, the polar caps keep the sea alone.
//! - **The mask** is drawn as the pieces are (super::row_runs: a pixel on the land when its centre
//!   is), each cell's rings a polygon of one piece, so cells side by side add up and their shared
//!   edges make no seam; a row's runs kept where the mask says, the empty set elsewhere (`apply`).
//! - **Keys:** each land cell by the digest of its bytes (`lines`), and on the water's side each z9
//!   tile of the water layer under a z6 tile by its own (`tile_lines`), so a coastline or a water
//!   layer that moved makes again only the z6 tiles whose cells or tiles it moved.

use super::inland::{self, Inland};
use super::{plane_poly, row_runs, PlanePoly, Polygon, Runs, Sets};
use anyhow::{ensure, Context, Result};
use inputs::inputs::regions::land::Land;
use inputs::inputs::regions::tiles::{Cell, CELL};
use std::collections::BTreeMap;

/// Which side of the coastline a theme's layers keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Land,
    /// The sea and the inland water.
    Sea,
}

/// Each theme's side (every theme of inputs::inputs::extents::THEMES has one: `side_of`).
pub const SIDES: [(&str, Side); 3] = [("terrain", Side::Land), ("bathymetry", Side::Sea), ("buildings", Side::Land)];

/// Theme `theme`'s side.
pub fn side_of(theme: &str) -> Result<Side> {
    SIDES.iter().find(|(t, _)| *t == theme).map(|(_, s)| *s).with_context(|| format!("{theme}: no side of the coast"))
}

/// The land and the side kept, each land cell's digest read once; on the water's side, the inland
/// water.
pub struct Cut<'a> {
    land: &'a Land,
    side: Side,
    digests: BTreeMap<Cell, String>,
    inland: Option<Inland<'a>>,
}

/// Degrees in E7.
const E7: f64 = 1e7;

impl<'a> Cut<'a> {
    /// The land's side.
    pub fn new(land: &'a Land, side: Side) -> Result<Cut<'a>> {
        ensure!(side == Side::Land, "the water's side holds the inland water too: Cut::sea");
        Cut::of(land, side, None)
    }

    /// The water's side: the sea, and `inland`'s water.
    pub fn sea(land: &'a Land, inland: Inland<'a>) -> Result<Cut<'a>> {
        Cut::of(land, Side::Sea, Some(inland))
    }

    fn of(land: &'a Land, side: Side, inland: Option<Inland<'a>>) -> Result<Cut<'a>> {
        let mut digests = BTreeMap::new();
        for c in land.cells() {
            if let Some(b) = land.bytes(c).with_context(|| format!("the land's cell {c:?}"))? {
                digests.insert(c, store::naming::hash16(&b));
            }
        }
        Ok(Cut { land, side, digests, inland })
    }

    /// Theme `theme`'s cut: its side (`side_of`), the water's with the inland water `inland` opens
    /// (super::inland::Open: the water layer's packs).
    pub fn of_theme(land: &'a Land, theme: &str, inland: Box<inland::Open<'a>>) -> Result<Cut<'a>> {
        match side_of(theme)? {
            Side::Land => Cut::new(land, Side::Land),
            Side::Sea => Cut::sea(land, Inland::new(inland)),
        }
    }

    /// What a key names of the cut in z6 tile x/y: `lines` of its box, and on the water's side its
    /// inland water (super::inland::Inland::lines).
    pub fn tile_lines(&self, x: u32, y: u32) -> Result<Vec<String>> {
        let mut v = self.lines(roadcore::geo::tile_box(codecs::available::PACK_Z, x, y));
        if let Some(i) = &self.inland {
            v.extend(i.lines(x, y)?);
        }
        Ok(v)
    }

    /// `mask`, rows `r0..` of Web Mercator's plane at z12 over the z6 tiles `xs` of row `y`, with the
    /// inland water kept too on the water's side (each of its pixels four of the plane's a side).
    pub fn with_inland(&self, mut mask: Vec<Runs>, y: u32, xs: &[u32], r0: u32) -> Result<Vec<Runs>> {
        let Some(inl) = &self.inland else { return Ok(mask) };
        let band = super::BAND;
        let k = band / inland::SIDE6;
        let mut xs = xs.to_vec();
        xs.sort_unstable();
        let tiles: Vec<(u32, Vec<Runs>)> = xs.iter().map(|&x| Ok((x, inl.rows(x, y)?))).collect::<Result<_>>()?;
        for (j, m) in mask.iter_mut().enumerate() {
            let r = (r0 + j as u32 - y * band) / k;
            let mut row: Runs = vec![(0, 0)];
            for (x, rows) in &tiles {
                for &(px, v) in &rows[r as usize] {
                    row.push((x * band + px * k, v));
                }
                if (x + 1) * band < super::MERC_W {
                    row.push(((x + 1) * band, 0));
                }
            }
            // (Runs starting where the one before did: the later stands; side by side of one value:
            // one.)
            row.dedup_by(|b, a| {
                if a.0 == b.0 {
                    a.1 = b.1;
                    true
                } else {
                    false
                }
            });
            row.dedup_by(|b, a| a.1 == b.1);
            *m = inland::or(m, &row);
        }
        Ok(mask)
    }

    /// The land's cells meeting box `b` (west, south, east, north, degrees; its edges included).
    fn cells(&self, b: [f64; 4]) -> impl Iterator<Item = (&Cell, &String)> {
        let cell = CELL as f64 / E7;
        let at = |v: f64, lo: i64, hi: i64| ((v / cell).floor() as i64).clamp(lo, hi);
        let (x0, x1) = (at(b[0], -90, 89), at(b[2], -90, 89));
        let (y0, y1) = (at(b[1], -45, 44), at(b[3], -45, 44));
        (x0..=x1).flat_map(move |x| self.digests.range((x, y0)..=(x, y1)))
    }

    /// What a key names of the cut in box `b`: its side, and each land cell meeting the box by its
    /// digest.
    pub fn lines(&self, b: [f64; 4]) -> Vec<String> {
        let mut v = vec![format!("cut {:?}", self.side)];
        v.extend(self.cells(b).map(|(c, d)| format!("land {} {} {d}", c.0, c.1)));
        v
    }

    /// The rows `r0..r1` of a plane `width` pixels wide (`f`: longitude, latitude → its pixels) as
    /// runs of 1 where a pixel is kept and 0 where it's cut, from the land's cells meeting any of
    /// `boxes`; a pixel outside the boxes is answered as if its cells held no land (cut for the
    /// land's side, kept for the sea's), so only those within the boxes are to be asked.
    pub fn mask(
        &self,
        boxes: &[[f64; 4]],
        f: &dyn Fn(f64, f64) -> [f64; 2],
        width: u32,
        r0: u32,
        r1: u32,
    ) -> Result<Vec<Runs>> {
        let mut cells: Vec<Cell> = boxes.iter().flat_map(|b| self.cells(*b).map(|(c, _)| *c)).collect();
        cells.sort_unstable();
        cells.dedup();
        let mut polys: Vec<PlanePoly> = Vec::new();
        for c in cells {
            let rings: Vec<Vec<[f64; 2]>> = self
                .land
                .rings(c)?
                .iter()
                .map(|r| {
                    // (Closed, as a piece's rings are.)
                    let mut v: Vec<[f64; 2]> = r.iter().map(|p| [p[0] as f64 / E7, p[1] as f64 / E7]).collect();
                    if let Some(&p) = v.first() {
                        v.push(p);
                    }
                    v
                })
                .collect();
            let poly = Polygon { rings, bbox: [0.0; 4], digest: String::new() };
            polys.push(plane_poly(0, &poly, f));
        }
        // (The land a set of its own, 1; the sea the empty set, 0.)
        let mut sets = Sets::default();
        let keep = |s: u32| u32::from((s != 0) == (self.side == Side::Land));
        Ok(row_runs(&polys, width, r0, r1, &mut sets)
            .into_iter()
            .map(|rs| {
                let mut v: Runs = rs.into_iter().map(|(x, s)| (x, keep(s))).collect();
                v.dedup_by(|b, a| a.1 == b.1);
                v
            })
            .collect())
    }
}

/// A row's runs kept where its `mask` (runs of 1 and 0, `Cut::mask`) is 1, the empty set elsewhere.
pub fn apply(runs: &Runs, mask: &Runs) -> Runs {
    let mut out: Runs = Vec::with_capacity(runs.len());
    let (mut i, mut j, mut x) = (0, 0, 0);
    loop {
        let s = if mask[j].1 != 0 { runs[i].1 } else { 0 };
        if out.last().is_none_or(|l| l.1 != s) {
            out.push((x, s));
        }
        match (runs.get(i + 1).map(|r| r.0), mask.get(j + 1).map(|r| r.0)) {
            (None, None) => break,
            (Some(a), Some(b)) if a == b => (i, j, x) = (i + 1, j + 1, a),
            (Some(a), Some(b)) if a < b => (i, x) = (i + 1, a),
            (Some(a), None) => (i, x) = (i + 1, a),
            (_, Some(b)) => (j, x) = (j + 1, b),
        }
    }
    out
}
