//! The cut to one side of the coastline (docs/steps.md §14): a theme's layers hold a piece only on
//! its side, whatever its outline (an index's coastal tiles, a 1° tile, a block drawn over the sea).
//! The side is the theme's (`SIDES`): the terrain's the land, bathymetry's (planned) the water.
//! **Planned (task #1351):** `Side::Sea` is the land's mask inverted, and the land here is the
//! coastline's, which holds every lake: a bathymetry layer cut by it would lose every lake's depth
//! (GLOBathy's and the like). Its side needs the water layer's lakes added to the sea first.
//!
//! - **The land** is the coastline's, the pass's own (inputs::inputs::regions::land, the
//!   `regions-land` step's, by 2° cell): OSM's coastline, so a lake is land (the coastline goes round
//!   it: a terrain model's heights there are its water's surface, which the map's terrain uses) and
//!   the Caspian is sea (OSM maps it with coastline).
//! - **The mask** is drawn as the pieces are (super::row_runs: a pixel on the land when its centre
//!   is), each cell's rings a polygon of one piece, so cells side by side add up and their shared
//!   edges make no seam; a row's runs kept where the mask says, the empty set elsewhere (`apply`).
//! - **Keys:** each land cell by the digest of its bytes (`lines`), so a coastline that moved makes
//!   again only the z6 tiles whose cells it moved.

use super::{plane_poly, row_runs, PlanePoly, Polygon, Runs, Sets};
use anyhow::{Context, Result};
use inputs::inputs::regions::land::Land;
use inputs::inputs::regions::tiles::{Cell, CELL};
use std::collections::BTreeMap;

/// Which side of the coastline a theme's layers keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Land,
    Sea,
}

/// Each theme's side (every theme of inputs::inputs::extents::THEMES has one: `side_of`).
pub const SIDES: [(&str, Side); 1] = [("terrain", Side::Land)];

/// Theme `theme`'s side.
pub fn side_of(theme: &str) -> Result<Side> {
    SIDES.iter().find(|(t, _)| *t == theme).map(|(_, s)| *s).with_context(|| format!("{theme}: no side of the coast"))
}

/// The land and the side kept, each land cell's digest read once.
pub struct Cut<'a> {
    land: &'a Land,
    side: Side,
    digests: BTreeMap<Cell, String>,
}

/// Degrees in E7.
const E7: f64 = 1e7;

impl<'a> Cut<'a> {
    pub fn new(land: &'a Land, side: Side) -> Result<Cut<'a>> {
        let mut digests = BTreeMap::new();
        for c in land.cells() {
            if let Some(b) = land.bytes(c).with_context(|| format!("the land's cell {c:?}"))? {
                digests.insert(c, store::naming::hash16(&b));
            }
        }
        Ok(Cut { land, side, digests })
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
