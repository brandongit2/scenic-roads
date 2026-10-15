//! The regions' stored geometry (docs/inputs.md §6.1.4): per region, what the coverage reads
//! (`sources/inputs/regions/@geometry/<id>.<hash16>.rgeo`: the magic, a JSON head, each shape's rings
//! as zigzag varint deltas; no compression, so the same geometry is the same bytes whatever library
//! a Mac has) and what the catalog and the map's region layer draw
//! (`sources/inputs/regions/@drawn/<id>.<hash16>.json`).

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAGIC: &[u8; 8] = b"RGEO0001";

/// A stored shape: what one `coverage::Shape` is made from.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// The part it is (`osm:1629145`), or the region's expression for a computed one.
    pub from: String,
    /// Its country's ISO 3166-1 code ("" when none is known), as `Shape::country`.
    pub country: String,
    /// 1,000 for an `osm:` part kept with the coast tolerance, else 0 (metres).
    pub tolerance_m: u32,
    /// E7, unclosed, even–odd.
    pub rings: Vec<Vec<[i32; 2]>>,
}

/// A region's stored geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub id: String,
    /// The OSM pass it was resolved against.
    pub osm: String,
    pub shapes: Vec<Shape>,
    /// The relations and ways it names (sorted).
    pub relations: Vec<u64>,
    pub ways: Vec<u64>,
}

#[derive(Serialize, Deserialize)]
struct Head {
    fmt: u32,
    id: String,
    osm: String,
    shapes: Vec<ShapeHead>,
    #[serde(rename = "box")]
    bbox: [i32; 4],
    names: Names,
}

#[derive(Serialize, Deserialize)]
struct ShapeHead {
    from: String,
    country: String,
    tolerance_m: u32,
    rings: u64,
    points: u64,
}

#[derive(Serialize, Deserialize)]
struct Names {
    osm: Vec<u64>,
    ways: Vec<u64>,
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

impl Geometry {
    /// West, south, east, north of every ring (E7); all zero when it has none.
    pub fn bbox(&self) -> [i32; 4] {
        let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for p in self.shapes.iter().flat_map(|s| s.rings.iter().flatten()) {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        if b[0] > b[2] { [0; 4] } else { b }
    }

    /// The file's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let head = Head {
            fmt: 1,
            id: self.id.clone(),
            osm: self.osm.clone(),
            shapes: self.shapes.iter().map(|s| ShapeHead { from: s.from.clone(), country: s.country.clone(), tolerance_m: s.tolerance_m, rings: s.rings.len() as u64, points: s.rings.iter().map(|r| r.len() as u64).sum() }).collect(),
            bbox: self.bbox(),
            names: Names { osm: self.relations.clone(), ways: self.ways.clone() },
        };
        let h = serde_json::to_vec(&head).expect("a head serialises");
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&(h.len() as u32).to_le_bytes());
        out.extend_from_slice(&h);
        for s in &self.shapes {
            for r in &s.rings {
                varint(&mut out, r.len() as u64);
                let mut prev = [0i64; 2];
                for p in r {
                    varint(&mut out, zigzag(p[0] as i64 - prev[0]));
                    varint(&mut out, zigzag(p[1] as i64 - prev[1]));
                    prev = [p[0] as i64, p[1] as i64];
                }
            }
        }
        out
    }

    /// A file's bytes read back.
    pub fn decode(b: &[u8]) -> Result<Geometry> {
        ensure!(b.len() >= 12 && &b[..8] == MAGIC, "not a region's geometry (RGEO0001)");
        let n = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        let head: Head = serde_json::from_slice(b.get(12..12 + n).context("a short head")?).context("the head")?;
        ensure!(head.fmt == 1, "geometry format {}", head.fmt);
        let mut at = 12 + n;
        let mut read = || -> Result<u64> {
            let mut v = 0u64;
            for shift in (0..64).step_by(7) {
                let byte = *b.get(at).context("the rings end early")?;
                at += 1;
                v |= ((byte & 0x7f) as u64) << shift;
                if byte & 0x80 == 0 {
                    return Ok(v);
                }
            }
            bail!("a varint too long")
        };
        let unzig = |v: u64| ((v >> 1) as i64) ^ -((v & 1) as i64);
        let mut shapes = Vec::with_capacity(head.shapes.len());
        for sh in head.shapes {
            let mut rings = Vec::with_capacity(sh.rings as usize);
            for _ in 0..sh.rings {
                let k = read()? as usize;
                let mut r = Vec::with_capacity(k.min(1 << 24));
                let mut prev = [0i64; 2];
                for _ in 0..k {
                    let p = [prev[0] + unzig(read()?), prev[1] + unzig(read()?)];
                    r.push([i32::try_from(p[0]).context("a longitude out of range")?, i32::try_from(p[1]).context("a latitude out of range")?]);
                    prev = p;
                }
                rings.push(r);
            }
            shapes.push(Shape { from: sh.from, country: sh.country, tolerance_m: sh.tolerance_m, rings });
        }
        ensure!(at == b.len(), "bytes after the rings");
        Ok(Geometry { id: head.id, osm: head.osm, shapes, relations: head.names.osm, ways: head.names.ways })
    }
}

/// A region as drawn (the catalog's `DrawnRegion` shapes, the map's region layer): each stored
/// shape's polygons simplified for drawing, GeoJSON coordinates in degrees, by `from`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Drawn {
    pub fmt: u32,
    pub id: String,
    pub name: String,
    /// The outline entries as text (`Region::outline_text`), as a catalog records a region's.
    pub outline: Vec<String>,
    pub shapes: BTreeMap<String, Vec<Vec<Vec<[f64; 2]>>>>,
}

impl Drawn {
    /// As the catalog records a region (`coverage::DrawnRegion`).
    pub fn region(&self) -> crate::coverage::DrawnRegion {
        crate::coverage::DrawnRegion { id: self.id.clone(), name: self.name.clone(), outline: self.outline.clone(), shapes: self.shapes.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_reads_back_as_written() {
        let g = Geometry {
            id: "azores".into(),
            osm: "2026-09-28".into(),
            shapes: vec![
                Shape { from: "osm:1629146".into(), country: "PT".into(), tolerance_m: 1000, rings: vec![vec![[-311_234_567, 378_123_456], [-311_000_000, 378_000_000], [-310_000_000, 379_000_000]]] },
                Shape { from: "shape:x.geojson".into(), country: String::new(), tolerance_m: 0, rings: vec![vec![[1_799_999_999, -1], [-1_800_000_000, 0], [0, 900_000_000]], vec![]] },
            ],
            relations: vec![1629146],
            ways: vec![],
        };
        let b = g.encode();
        assert_eq!(Geometry::decode(&b).unwrap(), g);
        assert_eq!(g.encode(), b, "the same bytes twice");
        assert!(Geometry::decode(&b[..b.len() - 1]).is_err());
    }
}
