//! Every summit, worldwide (docs/phase5.md "summits"): the pass's `summits` set (natural=peak or
//! volcano, nodes and ways), each with the position and `ele` it has as a candidate (extract's
//! rules: a node's position; a way's centre, the integer mean of its nodes, the closing node
//! counted twice, truncated; `ele` parsed and rounded, nodes only). Sorted by OSM type and id;
//! written as zstd JSON lines (`work/summits/<d>`). Their heights in the peaks' z8 overlay
//! (`z8_height`) are each peaks run's own, from the z8 it reads (crate::peaks::unit::Z8Base), so
//! the summits read no terrain.

use crate::peaks::z8::Z8;
use anyhow::{Context, Result};
use osmpbf::{Element, ElementReader};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::Path;

/// One summit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summit {
    /// "n123", "w45".
    pub id: String,
    /// "peak" or "volcano".
    pub kind: String,
    /// E7.
    pub lon: i32,
    pub lat: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ele: Option<f32>,
}

impl Summit {
    /// The order summits and candidates share: nodes, then ways, by id.
    pub fn order(&self) -> (u8, i64) {
        let n = self.id[1..].parse::<i64>().unwrap_or(0);
        (if self.id.starts_with('n') { 0 } else { 1 }, n)
    }
}

/// The highest summit there is (a tag above it is no summit).
pub const MAX_ELEV: f32 = 8900.0;

/// A tagged height for the z8 overlay, given the highest z8 pixel within one pixel of the summit:
/// at most 1,500 m over it and twice it plus 300 m (feet tagged as metres overshoot by 2.28 times
/// the height, which a fixed margin lets through on low summits), above 0, at most `MAX_ELEV`.
pub fn z8_height(ele: f32, max3: f32) -> Option<f32> {
    (ele > 0.0 && ele <= MAX_ELEV && ele <= max3 + 1500.0 && ele <= 2.0 * max3 + 300.0).then_some(ele)
}

fn kind_of<'a, 'b>(tags: impl Iterator<Item = (&'a str, &'b str)>) -> (Option<&'static str>, Option<&'b str>) {
    let (mut kind, mut ele) = (None, None);
    for (k, v) in tags {
        match (k, v) {
            ("natural", "peak") => kind = Some("peak"),
            ("natural", "volcano") => kind = Some("volcano"),
            ("ele", e) => ele = Some(e),
            _ => {}
        }
    }
    (kind, ele)
}

fn concat<T>(mut a: Vec<T>, mut b: Vec<T>) -> Vec<T> {
    a.append(&mut b);
    a
}

/// The summits of `pbf` (the `summits` set).
pub fn read_set(pbf: &Path) -> Result<Vec<Summit>> {
    // Pass 1: the summits mapped as nodes, and the ways' node lists.
    let (mut nodes, mut ways): (Vec<Summit>, Vec<(i64, &'static str, Vec<i64>)>) =
        ElementReader::from_path(pbf).with_context(|| format!("open {}", pbf.display()))?.par_map_reduce(
            |el| {
                let node = |id: i64, lon: i32, lat: i32, tags: &mut dyn Iterator<Item = (&str, &str)>| {
                    let (kind, ele) = kind_of(tags);
                    kind.map(|k| Summit {
                        id: format!("n{id}"),
                        kind: k.into(),
                        lon,
                        lat,
                        ele: crate::candidates::parse_ele(ele).map(f32::round),
                    })
                };
                match el {
                    Element::DenseNode(n) => (
                        node(n.id(), n.decimicro_lon(), n.decimicro_lat(), &mut n.tags()).into_iter().collect(),
                        Vec::new(),
                    ),
                    Element::Node(n) => (
                        node(n.id(), n.decimicro_lon(), n.decimicro_lat(), &mut n.tags()).into_iter().collect(),
                        Vec::new(),
                    ),
                    Element::Way(w) => match kind_of(w.tags()).0 {
                        Some(k) => (Vec::new(), vec![(w.id(), k, w.refs().collect())]),
                        None => (Vec::new(), Vec::new()),
                    },
                    _ => (Vec::new(), Vec::new()),
                }
            },
            || (Vec::new(), Vec::new()),
            |a, b| (concat(a.0, b.0), concat(a.1, b.1)),
        )?;
    // Pass 2: where the ways' nodes are, for their centres (extract's poi_ways rule).
    let mut need: Vec<i64> = ways.iter().flat_map(|w| w.2.iter().copied()).collect();
    need.sort_unstable();
    need.dedup();
    let mut pos: Vec<(i64, i32, i32)> = if need.is_empty() {
        Vec::new()
    } else {
        ElementReader::from_path(pbf)?.par_map_reduce(
            |el| match el {
                Element::DenseNode(n) if need.binary_search(&n.id()).is_ok() => {
                    vec![(n.id(), n.decimicro_lon(), n.decimicro_lat())]
                }
                Element::Node(n) if need.binary_search(&n.id()).is_ok() => {
                    vec![(n.id(), n.decimicro_lon(), n.decimicro_lat())]
                }
                _ => Vec::new(),
            },
            Vec::new,
            concat,
        )?
    };
    pos.sort_unstable();
    ways.sort_by_key(|w| w.0);
    for (id, kind, refs) in ways {
        let pts: Vec<(i64, i64)> = refs
            .iter()
            .filter_map(|r| pos.binary_search_by_key(r, |p| p.0).ok().map(|i| (pos[i].1 as i64, pos[i].2 as i64)))
            .collect();
        if pts.is_empty() {
            continue;
        }
        let n = pts.len() as i64;
        let (sx, sy) = pts.iter().fold((0i64, 0i64), |a, p| (a.0 + p.0, a.1 + p.1));
        nodes.push(Summit {
            id: format!("w{id}"),
            kind: kind.into(),
            lon: (sx / n) as i32,
            lat: (sy / n) as i32,
            ele: None,
        });
    }
    nodes.sort_by_key(Summit::order);
    Ok(nodes)
}

/// The summit's z8 pixel (global pixel coordinates at z8).
pub fn z8_pixel(lon: i32, lat: i32) -> (i64, i64) {
    let (x, y) = roadcore::geo::merc(lon as f64 * 1e-7, lat as f64 * 1e-7);
    let w = 256.0 * (1u64 << crate::peaks::z8::Z) as f64;
    ((x * w).floor() as i64, (y * w).floor() as i64)
}

/// The z8 pixels a z8 height on pixel `p` reads (`z8_max_around`): those within one pixel of it,
/// x wrapping, none past the poles.
fn z8_around((px, py): (i64, i64)) -> impl Iterator<Item = (i64, i64)> {
    let w = 256i64 << crate::peaks::z8::Z;
    (-1..=1i64)
        .flat_map(move |dy| (-1..=1i64).map(move |dx| ((px + dx).rem_euclid(w), py + dy)))
        .filter(move |&(_, gy)| (0..w).contains(&gy))
}

/// The z8 tiles a summit's z8 height reads (`z8_max_around`): none without a tagged height (its
/// z8 height is none whatever the terrain), else those of the pixels within one of its own. What
/// a peaks run records of a summit it read (crate::peaks::unit::run).
pub fn z8_tiles_read(s: &Summit) -> impl Iterator<Item = (u32, u32)> + '_ {
    s.ele
        .into_iter()
        .flat_map(move |_| z8_around(z8_pixel(s.lon, s.lat)))
        .map(|(gx, gy)| ((gx / 256) as u32, (gy / 256) as u32))
}

/// The highest z8 pixel within one of pixel `p` (0 m where nothing is built), which a tagged height
/// on it is held to (`z8_height`).
pub fn z8_max_around(p: (i64, i64), z8: &Z8) -> Result<f32> {
    let mut max3 = f32::MIN;
    for (gx, gy) in z8_around(p) {
        let t = z8.tile((gx / 256) as u32, (gy / 256) as u32)?;
        max3 = max3.max(t[(gy % 256 * 256 + gx % 256) as usize].max(0.0));
    }
    Ok(max3)
}

pub fn write(path: &Path, summits: &[Summit]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut w = zstd::Encoder::new(std::fs::File::create(&tmp)?, 9)?.auto_finish();
        for s in summits {
            serde_json::to_writer(&mut w, s)?;
            w.write_all(b"\n")?;
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read(path: &Path) -> Result<Vec<Summit>> {
    let r = std::io::BufReader::new(zstd::Decoder::new(
        std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?,
    )?);
    r.lines().map(|l| Ok(serde_json::from_str(&l?)?)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z8_heights_plausible_only() {
        // Matterhorn-like: 4,478 m over a 3,600 m pixel mean.
        assert_eq!(z8_height(4478.0, 3600.0), Some(4478.0));
        // Feet as metres on a 600 m hill (1,968 "m"): over twice the pixel plus 300.
        assert_eq!(z8_height(1968.0, 560.0), None);
        // A 1,000 m summit over a 400 m pixel: fine.
        assert_eq!(z8_height(1000.0, 400.0), Some(1000.0));
        assert_eq!(z8_height(10000.0, 8500.0), None);
        assert_eq!(z8_height(0.0, 10.0), None);
    }

    /// Task #1545: a summit's z8 height reads the tiles of the pixels around its own: one tile
    /// inside, two at a tile's edge, four at its corner, wrapping at the antimeridian; none
    /// without a tagged height.
    #[test]
    fn a_summits_z8_height_reads_the_tiles_around_its_pixel() {
        let at = |gx: f64, gy: f64, ele: Option<f32>| {
            // (A point at the centre of global z8 pixel (gx, gy).)
            let w = 256.0 * 256.0;
            let (x, y) = ((gx + 0.5) / w, (gy + 0.5) / w);
            let (lon, lat) = (x * 360.0 - 180.0, roadcore::geo::merc_lat(y));
            let s =
                Summit { id: "n1".into(), kind: "peak".into(), lon: (lon * 1e7) as i32, lat: (lat * 1e7) as i32, ele };
            let mut t: Vec<(u32, u32)> = z8_tiles_read(&s).collect();
            t.sort_unstable();
            t.dedup();
            t
        };
        assert_eq!(at(133.0 * 256.0 + 100.0, 90.0 * 256.0 + 100.0, Some(1000.0)), [(133, 90)]);
        assert_eq!(at(133.0 * 256.0 + 255.0, 90.0 * 256.0 + 100.0, Some(1000.0)), [(133, 90), (134, 90)]);
        assert_eq!(at(133.0 * 256.0, 90.0 * 256.0, Some(1000.0)), [(132, 89), (132, 90), (133, 89), (133, 90)]);
        assert_eq!(at(255.0 * 256.0 + 255.0, 90.0 * 256.0 + 100.0, Some(1000.0)), [(0, 90), (255, 90)]);
        assert_eq!(at(133.0 * 256.0 + 255.0, 90.0 * 256.0 + 100.0, None), []);
    }

    #[test]
    fn order_nodes_then_ways_by_id() {
        let s = |id: &str| Summit { id: id.into(), kind: "peak".into(), lon: 0, lat: 0, ele: None };
        let mut v = vec![s("w2"), s("n10"), s("n9")];
        v.sort_by_key(Summit::order);
        assert_eq!(v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["n9", "n10", "w2"]);
    }
}
