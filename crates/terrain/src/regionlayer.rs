//! The step `region-tiles` (docs/steps.md §15): the region layer's tiles from the regions' accepted
//! geometry, its full precision (`@geometry/<id>.rgeo`, docs/inputs.md §6.1.4).
//!
//! Each region's shapes (its land and water, its parts, the cells it was resolved by) are read
//! with its inside on their left: each shape even–odd; the edges between its own pieces have it on
//! both sides and the topology drops them; only shapes that overlap are unioned (`orient`). The topology, the simplification and the
//! tiles are codecs::regionlayer's. The tiles go into packs as every layer's do (the root z0–2, lo
//! packs by z3 tile, hi packs by z6 tile: `codecs::regionlayer::pack_of`); a pack whose bytes are
//! those already recorded is left as it is, so one region's change writes again only the packs its
//! arcs reach (and the root, which every border reaches), and a pack no tile is in now goes.

use anyhow::{Context, Result};
use buildstore::out::Out;
use codecs::regionlayer::{self as fmt, Region};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// The version of what the tiles are made from and how (the step's key): codecs::regionlayer's
/// topology, simplification and tiles, and this module's reading.
pub const KEY_V: u32 = 1;

/// The regions of the accepted version `a` (every one with geometry, disabled ones too: the layer
/// draws them), each as rings with its inside on their left (`area_of`), sorted by id.
pub fn regions(root: &store::nasfs::NasRoot, a: &inputs::inputs::Accepted) -> Result<Vec<Region>> {
    let listed = inputs::inputs::regions::read::listed(a)?;
    let mut v: Vec<Region> = listed
        .par_iter()
        .filter_map(|l| Some((l, l.stored.as_ref()?)))
        .map(|(l, s)| -> Result<Region> {
            let b = root.join(&s.geometry).read().with_context(|| format!("read {}", s.geometry))?;
            let g = inputs::inputs::regions::stored::Geometry::decode(&b).with_context(|| s.geometry.clone())?;
            let mut folders: Vec<String> = l.path.split('/').map(str::to_string).collect();
            folders.pop();
            Ok(Region { id: l.recipe.id.clone(), folders, rings: area_of(&g) })
        })
        .collect::<Result<_>>()?;
    v.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(v)
}

/// A region's stored shapes as rings (E7, unclosed, its inside on their left): points held to the
/// plane (±180°, ±90°), each shape even–odd; shapes that only touch read as they are (`orient`), two
/// that overlap unioned (`union`).
pub fn area_of(g: &inputs::inputs::regions::stored::Geometry) -> Vec<Vec<[i32; 2]>> {
    let (h, p) = (fmt::HALF as i64, fmt::POLE as i64);
    let shapes: Vec<Vec<Vec<[i64; 2]>>> = g
        .shapes
        .iter()
        .map(|s| {
            s.rings
                .iter()
                .map(|r| r.iter().map(|q| [(q[0] as i64).clamp(-h, h), (q[1] as i64).clamp(-p, p)]).collect())
                .collect()
        })
        .collect();
    let rings = orient::oriented(&shapes).unwrap_or_else(|| union(&shapes));
    rings.into_iter().map(|r| r.into_iter().map(|q| [q[0] as i32, q[1] as i32]).collect()).collect()
}

/// Overlapping shapes' union: each even–odd, then their union (canonical rings).
fn union(shapes: &[Vec<Vec<[i64; 2]>>]) -> Vec<Vec<[i64; 2]>> {
    use inputs::inputs::regions::geom::{union_all, Area};
    let areas: Vec<Area> = shapes.iter().map(|s| Area::from_rings(s)).collect();
    union_all(&areas).polys.into_iter().flatten().collect()
}

/// What a run did and what it cost.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Made {
    pub topology: fmt::topo::Stats,
    pub zooms: Vec<fmt::simplify::ZoomStats>,
    /// Tiles by zoom, and their bytes (gzip'd) by zoom.
    pub tiles: Vec<u64>,
    pub bytes: Vec<u64>,
    /// The packs written, those whose bytes were as recorded (left), and those gone.
    pub written: Vec<String>,
    pub kept: usize,
    pub dropped: Vec<String>,
}

/// A pack's tiles: zoom, x, y, gzip'd bytes and their length unzipped.
type Stored = (u8, u32, u32, Vec<u8>, u32);

/// The pack of tile z/x/y by logical name, and its root tile.
fn pack_logical(z: u8, x: u32, y: u32) -> (String, &'static str, (u8, u32, u32)) {
    let (scope, r) = fmt::pack_of(z, x, y);
    (format!("layers/{}/{scope}/{}-{}-{}", fmt::LAYER, r.0, r.1, r.2), scope, r)
}

/// The layer's tiles from `regions` (see the module's comment), `report` hearing the stage done of
/// those to do and what it is.
pub fn build(out: &mut Out, regions: &[Region], report: &dyn Fn(u64, u64, &str)) -> Result<Made> {
    let mut seen = BTreeSet::new();
    for r in regions {
        anyhow::ensure!(seen.insert(r.id.as_str()), "region {} twice", r.id);
    }
    let total = 2 + u64::from(fmt::MAXZ) + 1;
    report(0, total, "the topology");
    let topo = fmt::topo::build(regions);
    report(1, total, "simplifying the borders");
    let s = fmt::simplify::simplify(&topo, &|_, _| {});
    report(2, total, "the tiles");
    let packs: Mutex<BTreeMap<String, (&'static str, (u8, u32, u32), Vec<Stored>)>> = Mutex::default();
    let n = (fmt::MAXZ + 1) as usize;
    let (tiles, bytes) = (Mutex::new(vec![0u64; n]), Mutex::new(vec![0u64; n]));
    fmt::tiles::all(
        &topo,
        &s,
        &|(z, x, y, raw)| {
            let gz = codecs::available::gzip(&raw);
            tiles.lock().unwrap()[z as usize] += 1;
            bytes.lock().unwrap()[z as usize] += gz.len() as u64;
            let (l, scope, root) = pack_logical(z, x, y);
            packs.lock().unwrap().entry(l).or_insert_with(|| (scope, root, Vec::new())).2.push((
                z,
                x,
                y,
                gz,
                raw.len() as u32,
            ));
        },
        &|z| report(3 + u64::from(z), total, "the tiles"),
    );
    let mut made = Made {
        topology: topo.stats,
        zooms: s.zooms.clone(),
        tiles: tiles.into_inner().unwrap(),
        bytes: bytes.into_inner().unwrap(),
        ..Default::default()
    };
    drop(s);
    let packs = packs.into_inner().unwrap();
    let names: BTreeSet<String> = packs.keys().cloned().collect();
    for (l, (scope, root, mut ts)) in packs {
        ts.sort_by_key(|t| roadcore::archive::tile_key(t.0, t.1, t.2));
        let Some(p) =
            crate::layers::write_pack_local(out, fmt::LAYER, fmt::ENCODING, true, scope, root, &mut ts.into_iter())?
        else {
            continue;
        };
        if out.get(&l) == Some(p.content_name()?.as_str()) {
            std::fs::remove_file(&p.local).ok();
            made.kept += 1;
            continue;
        }
        out.put_file(&l, "pack", &p.local)?;
        made.written.push(l);
    }
    let prefix = format!("layers/{}/", fmt::LAYER);
    let gone: Vec<String> = out
        .manifest
        .range(prefix.clone()..)
        .take_while(|(l, _)| l.starts_with(&prefix))
        .map(|(l, _)| l.clone())
        .filter(|l| !names.contains(l))
        .collect();
    for l in gone {
        out.remove(&l);
        made.dropped.push(l);
    }
    Ok(made)
}

mod orient;
#[cfg(test)]
mod tests;
