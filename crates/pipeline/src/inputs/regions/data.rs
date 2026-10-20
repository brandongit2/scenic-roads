//! The regions' datasets (docs/inputs.md §6.1.3 "Land and water", §6.1.7): what `eez:`, `ts12:`
//! and `ne-lake:` parts name, and the coastline's land a region with `water` is cut back to. Each is
//! a declared source (§6.9, the `regions-*` roles) kept in `sources/fetched/<id>/`:
//!
//! | role | source | read |
//! |---|---|---|
//! | `regions-sea` | Marine Regions' Maritime Boundaries, EEZ v12 (a GeoJSON) | each zone by `mrgid` |
//! | `regions-internal-waters` | Marine Regions' Internal Waters v4 (a GeoJSON) | each by `mrgid_eez`, added to its zone |
//! | `regions-territorial-seas` | Marine Regions' Territorial Seas v4 (a GeoJSON) | each by `mrgid` |
//! | `regions-lakes` | Natural Earth's 1:10m lakes (a shapefile zip) | each by `ne_id`; those over 100 km² are water |
//! | `regions-land` | OSM's land polygons, split, in degrees (a shapefile zip) | the land side of the coastline |
//!
//! Read whole (`Datasets::read`), and only once a region in the folder names them (the check reads
//! them lazily: `Prepared::data_from`); a region naming them waits, held (`rg-data`), while they
//! can't be read. Features of one id are unioned (`merge`).

use super::geom::{self, Area, Ring};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// The roles (crate::inputs::sources::ROLES).
pub const SEA: &str = "regions-sea";
pub const INTERNAL: &str = "regions-internal-waters";
pub const TERRITORIAL: &str = "regions-territorial-seas";
pub const LAKES: &str = "regions-lakes";
pub const LAND: &str = "regions-land";

/// A lake is water, not land, from this size (km², as Natural Earth draws it).
pub const BIG_LAKE_KM2: f64 = 100.0;

/// The datasets, read: rings E7 (even–odd), in −180…180.
#[derive(Default)]
pub struct Datasets {
    /// EEZ zones with their internal waters, by MRGID.
    pub eez: HashMap<u64, Vec<Ring>>,
    /// The internal waters drawn for each zone (by the zone's MRGID): unioned with the zone, not
    /// read even–odd with it (the zones' polygons already take in most of them).
    pub internal: HashMap<u64, Vec<Ring>>,
    pub ts12: HashMap<u64, Vec<Ring>>,
    /// Lakes by `ne_id`: rings and size (km²).
    pub lakes: HashMap<u64, (Vec<Ring>, f64)>,
    /// The coastline's land, in pieces with their boxes.
    /// (Kept as E7 `i32`, half an `i64` ring's size: it's most of the datasets.)
    pub land: Vec<([i64; 4], Vec<Vec<[i32; 2]>>)>,
    /// A bucket grid (1°) of `land`'s pieces, and of the big lakes.
    land_grid: HashMap<(i64, i64), Vec<usize>>,
    lake_grid: HashMap<(i64, i64), Vec<u64>>,
    /// What they were read from (for the check's key): each source's id and version.
    pub names: String,
}

const CELL: i64 = 10_000_000;

fn cells(b: [i64; 4]) -> impl Iterator<Item = (i64, i64)> {
    let (x0, x1, y0, y1) = (b[0].div_euclid(CELL), b[2].div_euclid(CELL), b[1].div_euclid(CELL), b[3].div_euclid(CELL));
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

fn bbox(rings: &[Ring]) -> Option<[i64; 4]> {
    geom::bbox_of(rings.iter().flatten())
}

impl Datasets {
    /// Datasets from parts in memory (tests, fixtures): zones, their internal waters, territorial
    /// seas, lakes, land.
    pub fn from_parts(
        eez: HashMap<u64, Vec<Ring>>,
        internal: HashMap<u64, Vec<Ring>>,
        ts12: HashMap<u64, Vec<Ring>>,
        lakes: HashMap<u64, Vec<Ring>>,
        land: Vec<Vec<Ring>>,
        names: &str,
    ) -> Datasets {
        let lakes = lakes.into_iter().map(|(k, r)| {
            let a = geom::area_km2(&Area::from_rings(&r));
            (k, (r, a))
        });
        let mut d = Datasets { eez, internal, ts12, lakes: lakes.collect(), names: names.into(), ..Default::default() };
        d.land = land
            .into_iter()
            .filter_map(|r| bbox(&r).map(|b| (b, r.iter().map(|x| super::shapes::to_i32(x)).collect())))
            .collect();
        d.index();
        d
    }

    fn index(&mut self) {
        self.land_grid.clear();
        for (k, (b, _)) in self.land.iter().enumerate() {
            for c in cells(*b) {
                self.land_grid.entry(c).or_default().push(k);
            }
        }
        self.lake_grid.clear();
        let mut ids: Vec<&u64> = self.lakes.keys().collect();
        ids.sort();
        for id in ids {
            let (r, a) = &self.lakes[id];
            if *a < BIG_LAKE_KM2 {
                continue;
            }
            if let Some(b) = bbox(r) {
                for c in cells(b) {
                    self.lake_grid.entry(c).or_default().push(*id);
                }
            }
        }
    }

    /// The coastline's land in the box `b` (plain frame), clipped to it.
    pub fn land_in(&self, b: [i64; 4]) -> Area {
        let mut ks: Vec<usize> = cells(b).filter_map(|c| self.land_grid.get(&c)).flatten().copied().collect();
        ks.sort();
        ks.dedup();
        let areas: Vec<Area> = ks
            .into_iter()
            .filter(|&k| {
                let p = self.land[k].0;
                p[0] <= b[2] && p[2] >= b[0] && p[1] <= b[3] && p[3] >= b[1]
            })
            .map(|k| {
                // (Each ring clipped to the box first: a piece of the polar land may span the
                // Earth's width.)
                Area::from_rings(
                    &self.land[k]
                        .1
                        .iter()
                        .filter_map(|r| {
                            let r: Ring = r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect();
                            super::tiles::clip(&r, b)
                        })
                        .collect::<Vec<Ring>>(),
                )
            })
            .collect();
        geom::union_all(&areas)
    }

    /// The lakes over `BIG_LAKE_KM2` in the box `b` (plain frame), clipped to it.
    pub fn big_lakes_in(&self, b: [i64; 4]) -> Area {
        let mut ids: Vec<u64> = cells(b).filter_map(|c| self.lake_grid.get(&c)).flatten().copied().collect();
        ids.sort();
        ids.dedup();
        let areas: Vec<Area> = ids
            .iter()
            .map(|id| {
                Area::from_rings(&self.lakes[id].0.iter().filter_map(|r| super::tiles::clip(r, b)).collect::<Vec<_>>())
            })
            .collect();
        geom::union_all(&areas)
    }

    /// Whether it has the coastline's land (a region with `water` needs it).
    pub fn has_land(&self) -> bool {
        !self.land.is_empty()
    }

    /// The datasets the accepted sources `s` declare, read from `root`'s `sources/fetched/<id>/`
    /// (each declaration's `params.path`). None when no `regions-sea` is declared.
    pub fn read(root: &Path, s: &crate::inputs::sources::Sources) -> Result<Option<Datasets>> {
        let Ok(sea) = s.role(SEA) else { return Ok(None) };
        let mut names = Vec::new();
        let mut path = |src: &crate::inputs::sources::Source| -> Result<std::path::PathBuf> {
            names.push(format!("{} {}", src.id, src.version().unwrap_or("-")));
            Ok(root.join(src.param_str("path")?))
        };
        let mut d = Datasets::default();
        // (The zones read whole first, then simplified with the borders any two of their rings
        // share kept: neighbours' borders stay the same on both sides, no slivers between them.)
        let p = path(sea)?;
        let eez = read_geojson_raw(&p, sea.param_str("id")?)?;
        let iw = match s.role(INTERNAL) {
            Ok(iw) => read_geojson_raw(&path(iw)?, iw.param_str("id")?)?,
            Err(_) => Raw::new(),
        };
        let ts = match s.role(TERRITORIAL) {
            Ok(ts) => read_geojson_raw(&path(ts)?, ts.param_str("id")?)?,
            Err(_) => Raw::new(),
        };
        let pins = shared_points(&[&eez, &iw, &ts]);
        d.eez = simplified(eez, &pins);
        d.internal = simplified(iw, &pins);
        d.ts12 = simplified(ts, &pins);
        drop(pins);
        if let Ok(l) = s.role(LAKES) {
            let p = path(l)?;
            for (id, rings) in read_shapefile(&p, l.param_str("layer")?, Some(l.param_str("id")?))? {
                let a = geom::area_km2(&Area::from_rings(&rings));
                d.lakes.insert(id, (rings, a));
            }
        }
        if let Ok(l) = s.role(LAND) {
            let p = path(l)?;
            let land = &mut d.land;
            each_shape(&p, l.param_str("layer")?, None, &mut |_, rings| {
                if let Some(b) = bbox(&rings) {
                    land.push((b, rings.iter().map(|x| super::shapes::to_i32(x)).collect()));
                }
            })?;
            anyhow::ensure!(!d.land.is_empty(), "{}: no land polygons", p.display());
        }
        d.names = names.join(", ");
        d.index();
        Ok(Some(d))
    }
}

#[derive(Deserialize)]
struct Fc {
    features: Vec<Feature>,
}

#[derive(Deserialize)]
struct Feature {
    properties: serde_json::Map<String, serde_json::Value>,
    geometry: Option<Geometry>,
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "coordinates")]
enum Geometry {
    Polygon(Vec<Vec<[f64; 2]>>),
    MultiPolygon(Vec<Vec<Vec<[f64; 2]>>>),
}

/// The datasets' rings are simplified within this (metres) as they're read: a region's edge needs
/// no finer, and a continent's coast at full detail is what makes the operations large.
pub const SIMPLIFY_M: f64 = 20.0;

/// A feature's rings added to those of its id: the first as they are (even–odd), each further one
/// unioned (features of one id may overlap: read together even–odd they'd cancel).
pub fn merge(have: &mut Vec<Ring>, rings: Vec<Ring>) {
    if have.is_empty() {
        *have = rings;
    } else {
        *have = Area::from_rings(have).union(&Area::from_rings(&rings)).rings();
    }
}

fn e7_raw(r: &[[f64; 2]]) -> Ring {
    let mut v: Ring = r.iter().map(|p| [(p[0] * 1e7).round() as i64, (p[1] * 1e7).round() as i64]).collect();
    if v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    v
}

fn e7(r: &[[f64; 2]]) -> Ring {
    geom::simplify_ring(&e7_raw(r), SIMPLIFY_M)
}

/// A FeatureCollection's features as read (rings not simplified): (the integer property `id`,
/// its rings).
pub type Raw = Vec<(u64, Vec<Ring>)>;

pub fn read_geojson_raw(p: &Path, id: &str) -> Result<Raw> {
    let b = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    parse_geojson_raw(&b, id).with_context(|| format!("parse {}", p.display()))
}

pub fn parse_geojson_raw(b: &[u8], id: &str) -> Result<Raw> {
    let fc: Fc = serde_json::from_slice(b)?;
    let mut out = Raw::new();
    for f in fc.features {
        let Some(k) = f.properties.get(id).and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|x| x as u64))) else {
            continue;
        };
        let rings: Vec<Ring> = match f.geometry {
            Some(Geometry::Polygon(p)) => p.iter().map(|r| e7_raw(r)).collect(),
            Some(Geometry::MultiPolygon(m)) => m.iter().flatten().map(|r| e7_raw(r)).collect(),
            None => continue,
        };
        out.push((k, rings));
    }
    Ok(out)
}

/// The points more than one ring of `sets` has: the borders they share.
pub fn shared_points(sets: &[&Raw]) -> std::collections::HashSet<geom::Pt> {
    let mut first: HashMap<geom::Pt, u32> = HashMap::new();
    let mut shared = std::collections::HashSet::new();
    let mut n = 0u32;
    for set in sets {
        for (_, rings) in set.iter() {
            for r in rings {
                n += 1;
                for p in r {
                    match first.get(p) {
                        None => {
                            first.insert(*p, n);
                        }
                        Some(&m) if m != n => {
                            shared.insert(*p);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    shared
}

/// Features simplified within `SIMPLIFY_M`, the `pins` kept, and joined by id.
pub fn simplified(raw: Raw, pins: &std::collections::HashSet<geom::Pt>) -> HashMap<u64, Vec<Ring>> {
    let mut out: HashMap<u64, Vec<Ring>> = HashMap::new();
    for (k, rings) in raw {
        let rings = rings.iter().map(|r| geom::simplify_ring_pinned(r, SIMPLIFY_M, &|p| pins.contains(p))).collect();
        merge(out.entry(k).or_default(), rings);
    }
    out
}

/// A FeatureCollection's (Multi)Polygons, by the integer property `id` (features of one id joined),
/// the borders its features share kept as they are.
pub fn parse_geojson(b: &[u8], id: &str) -> Result<HashMap<u64, Vec<Ring>>> {
    let raw = parse_geojson_raw(b, id)?;
    let pins = shared_points(&[&raw]);
    Ok(simplified(raw, &pins))
}

/// A shapefile layer's polygons (in a zip or a folder), by the integer field `id` (or by record
/// number without one).
pub fn read_shapefile(p: &Path, layer: &str, id: Option<&str>) -> Result<BTreeMap<u64, Vec<Ring>>> {
    let mut out: BTreeMap<u64, Vec<Ring>> = BTreeMap::new();
    each_shape(p, layer, id, &mut |k, rings| merge(out.entry(k).or_default(), rings))?;
    anyhow::ensure!(!out.is_empty(), "{}: no {layer} polygons", p.display());
    Ok(out)
}

/// A shapefile layer's polygons one at a time, as read: (its id or record number, its rings).
pub fn each_shape(p: &Path, layer: &str, id: Option<&str>, f: &mut dyn FnMut(u64, Vec<Ring>)) -> Result<()> {
    use crate::lakefill::shp::{DbfReader, ShpReader};
    let mut n = 0u64;
    for (shp, dbf, _) in crate::lakefill::run::shapefiles(p, layer)? {
        let mut s = ShpReader::new(shp.open()?)?;
        let mut d = DbfReader::new(dbf.open()?)?;
        let col = match id {
            Some(f) => Some(d.col(f).with_context(|| format!("{}: no field {f}", dbf.name()))?),
            None => None,
        };
        while let Some(poly) = s.next_polygon()? {
            let row = d.next_row()?.context("the table ends before the shapes")?;
            n += 1;
            let k = match col {
                Some(c) => row[c].trim().parse::<f64>().map(|x| x as u64).unwrap_or(0),
                None => n,
            };
            if poly.is_empty() {
                continue;
            }
            f(k, poly.iter().map(|r| e7(r)).collect());
        }
    }
    Ok(())
}
