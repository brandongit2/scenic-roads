//! The inland water the sea's side keeps (docs/steps.md §14, the cut): bathymetry's layers hold
//! the lakes, reservoirs and rivers as well as the sea, which the coastline's land holds (OSM's
//! coastline goes round no lake). They are the water layer's (inputs::water, docs/steps.md §4): its
//! stored tiles' inland share at their finest stored zoom, z9's 512 px a tile (a pixel 153 m at the
//! equator, four of the data-available layers' z12 pixels a side), a pixel inland water where its
//! share is half or more (`LAKE`). A z9 tile not stored is one value throughout, its nearest stored
//! ancestor's there (inputs::water::uniform_from_ancestor's rule).
//!
//! - **Keys:** each z9 tile under a z6 tile by its stored bytes' hash, or, not stored, by whether
//!   its ancestor makes it inland water (`lines`): a new water layer makes again only the z6 tiles
//!   whose tiles moved.
//! - **The polar caps** take none: the water layer's tiles end at 85.05°.

use super::Runs;
use anyhow::{Context, Result};
use inputs::water::{self as wt, Cov};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use store::pack::{Entry, PackIndex};
use store::range::RangeRead;

/// The inland share (of 255) from which a water pixel is inland water.
pub const LAKE: u8 = 128;
/// The water layer's zoom read: its finest stored.
const Z: u8 = wt::STORED_MAXZ;
/// Its z9 tiles a z6 tile's side.
const PER: u32 = 1 << (Z - codecs::available::PACK_Z);
/// A z6 tile's side in the water layer's pixels.
pub const SIDE6: u32 = PER * wt::SIZE as u32;

/// Opens a pack of the water layer by its logical name: its content name and its bytes (None
/// where the records have none).
pub type Open<'a> = dyn Fn(&str) -> Result<Option<(String, Arc<dyn RangeRead>)>> + Sync + 'a;

/// The water layer's packs as the records name them (`manifest`), each opened from `root` for
/// positioned reads as it's asked for.
pub fn from_records(
    root: &store::nasfs::NasRoot,
    manifest: &std::collections::BTreeMap<String, String>,
) -> Box<Open<'static>> {
    let prefix = format!("layers/{}/", wt::LAYER);
    let packs: HashMap<String, String> = manifest
        .range(prefix.clone()..)
        .take_while(|(l, _)| l.starts_with(&prefix))
        .map(|(l, c)| (l.clone(), c.clone()))
        .collect();
    let root = root.clone();
    Box::new(move |l: &str| match packs.get(l) {
        Some(c) => {
            let f = root.join(c).open_plain().with_context(|| format!("open {c}"))?;
            Ok(Some((c.clone(), Arc::new(f) as Arc<dyn RangeRead>)))
        }
        None => Ok(None),
    })
}

/// A pack read: its bytes and index.
struct Pack {
    src: Arc<dyn RangeRead>,
    idx: PackIndex,
}

/// Where a z9 tile's water comes from: its stored tile, or one value, whether it's inland water.
#[derive(Clone)]
enum Src {
    Tile(Arc<Pack>, Entry),
    Uniform(bool),
}

/// The water layer's inland water, read as it's asked for, each pack's index and each ancestor
/// tile once.
pub struct Inland<'a> {
    open: Box<Open<'a>>,
    packs: Mutex<HashMap<String, Option<Arc<Pack>>>>,
    ancestors: Mutex<HashMap<(u8, u32, u32), Option<Arc<Cov>>>>,
}

impl<'a> Inland<'a> {
    pub fn new(open: Box<Open<'a>>) -> Inland<'a> {
        Inland { open, packs: Mutex::default(), ancestors: Mutex::default() }
    }

    /// The pack `logical`, its index read once.
    fn pack(&self, logical: &str) -> Result<Option<Arc<Pack>>> {
        if let Some(p) = self.packs.lock().unwrap().get(logical) {
            return Ok(p.clone());
        }
        let p = match (self.open)(logical)? {
            Some((name, src)) => {
                let idx = PackIndex::read_from(&*src).with_context(|| format!("read {name}'s index"))?;
                Some(Arc::new(Pack { src, idx }))
            }
            None => None,
        };
        self.packs.lock().unwrap().insert(logical.to_string(), p.clone());
        Ok(p)
    }

    /// The pack holding the water layer's tile z/x/y (layers' packs: the root z0–2, a lo pack a
    /// z3 tile for z3–8, a hi pack a z6 tile for z9 on).
    fn pack_of(&self, z: u8, x: u32, y: u32) -> Result<Option<Arc<Pack>>> {
        let (scope, _, px, py) = crate::layers::pack_of(z, x, y);
        let l = match scope {
            "root" => crate::layers::root_logical(wt::LAYER),
            "lo" => crate::layers::lo_logical(wt::LAYER, px, py),
            _ => crate::layers::hi_logical(wt::LAYER, px, py),
        };
        self.pack(&l)
    }

    /// Tile z/x/y decoded, if the layer stores it (each ancestor once).
    fn ancestor(&self, z: u8, x: u32, y: u32) -> Result<Option<Arc<Cov>>> {
        if let Some(c) = self.ancestors.lock().unwrap().get(&(z, x, y)) {
            return Ok(c.clone());
        }
        let c = match self.pack_of(z, x, y)? {
            Some(p) => match p.idx.get(&*p.src, z, x, y)? {
                Some((_, b)) => Some(Arc::new(Cov::from_png(&b).with_context(|| format!("water {z}/{x}/{y}"))?)),
                None => None,
            },
            None => None,
        };
        self.ancestors.lock().unwrap().insert((z, x, y), c.clone());
        Ok(c)
    }

    /// Z9 tile x/y's source: its stored tile, else its nearest stored ancestor's value there (none
    /// stored at all: no water).
    fn source(&self, x: u32, y: u32) -> Result<Src> {
        if let Some(p) = self.pack_of(Z, x, y)? {
            if let Some(e) = p.idx.find(Z, x, y) {
                return Ok(Src::Tile(p, e));
            }
        }
        // (inputs::water::uniform_from_ancestor's rule, the ancestor not copied: the middle of
        // this tile's part of it.)
        for a in (0..Z).rev() {
            let d = Z - a;
            if let Some(c) = self.ancestor(a, x >> d, y >> d)? {
                let span = wt::SIZE as f64 / f64::from(1u32 << d);
                let px = |v: u32, base: u32| ((f64::from(v - (base << d)) + 0.5) * span) as usize;
                return Ok(Src::Uniform(c.value_at(px(x, x >> d), px(y, y >> d)).1 >= LAKE));
            }
        }
        Ok(Src::Uniform(false))
    }

    /// The z9 tiles under z6 tile x/y, row by row.
    fn sources(&self, x6: u32, y6: u32) -> Result<Vec<Src>> {
        let mut v = Vec::with_capacity((PER * PER) as usize);
        for j in 0..PER {
            for i in 0..PER {
                v.push(self.source(x6 * PER + i, y6 * PER + j)?);
            }
        }
        Ok(v)
    }

    /// What a key names of z6 tile x/y's inland water: each z9 tile under it by its stored bytes'
    /// hash, or not stored, by whether it's inland water.
    pub fn lines(&self, x6: u32, y6: u32) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for (k, s) in self.sources(x6, y6)?.iter().enumerate() {
            let (x, y) = (x6 * PER + k as u32 % PER, y6 * PER + k as u32 / PER);
            out.push(match s {
                Src::Tile(_, e) => format!("inland {Z}/{x}/{y} {}", e.etag()),
                Src::Uniform(lake) => format!("inland {Z}/{x}/{y} ={}", u8::from(*lake)),
            });
        }
        Ok(out)
    }

    /// Z6 tile x/y's inland water: `SIDE6` rows of runs over its `SIDE6` pixels (the water layer's
    /// z9 pixels), 1 inland water, 0 not.
    pub fn rows(&self, x6: u32, y6: u32) -> Result<Vec<Runs>> {
        let n = wt::SIZE;
        let mut rows: Vec<Runs> = vec![Vec::new(); SIDE6 as usize];
        for (k, s) in self.sources(x6, y6)?.iter().enumerate() {
            let (i, j) = (k % PER as usize, k / PER as usize);
            let x0 = (i * n) as u32;
            let tile = match s {
                Src::Tile(p, e) => Some(Cov::from_png(&p.idx.read_blob(&*p.src, e)?).context("a water tile")?),
                Src::Uniform(_) => None,
            };
            for r in 0..n {
                let row = &mut rows[j * n + r];
                let mut push = |x: u32, v: u32| {
                    if row.last().is_none_or(|l: &(u32, u32)| l.1 != v) {
                        row.push((x, v));
                    }
                };
                match (&tile, s) {
                    (_, Src::Uniform(lake)) => push(x0, u32::from(*lake)),
                    (Some(c), _) => {
                        for c0 in 0..n {
                            push(x0 + c0 as u32, u32::from(c.value_at(c0, r).1 >= LAKE));
                        }
                    }
                    (None, _) => unreachable!("a stored tile is read"),
                }
            }
        }
        Ok(rows)
    }
}

/// Runs `a` or `b` (runs of 1 and 0 over one row): 1 where either is.
pub fn or(a: &Runs, b: &Runs) -> Runs {
    let mut out: Runs = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j, mut x) = (0, 0, 0);
    loop {
        let v = u32::from(a[i].1 != 0 || b[j].1 != 0);
        if out.last().is_none_or(|l| l.1 != v) {
            out.push((x, v));
        }
        match (a.get(i + 1).map(|r| r.0), b.get(j + 1).map(|r| r.0)) {
            (None, None) => break,
            (Some(p), Some(q)) if p == q => (i, j, x) = (i + 1, j + 1, p),
            (Some(p), Some(q)) if p < q => (i, x) = (i + 1, p),
            (Some(p), None) => (i, x) = (i + 1, p),
            (_, Some(q)) => (j, x) = (j + 1, q),
        }
    }
    out
}
