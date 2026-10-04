//! Each unit's reach (docs/plan.md §5, Coverage): where the roads, rail and ferries of its OSM
//! piece go, worked out once per pass (`scenic-build reach`) and kept as `sources/osm/<date>/reach`.
//! The planner picks the units to build by it and keys them on the coverage they read.
//!
//! Most of a piece's ways stay within its tile grown by `LONG_KM`. For those only a box is kept: the
//! box of the ones the unit owns (first node inside its tile). The few that reach further (ferries,
//! long rural roads) are kept whole, with whether the unit owns each. Then:
//! - **A unit is built** when its owned box meets the coverage, or one of its own long ways touches
//!   it. So a road starting in a tile far from every outline is built when it enters the coverage,
//!   and a tile owning a ferry to a far coast isn't built for that coast unless the ferry touches it.
//! - **Its key** names the coverage inside its tile grown by `LONG_KM` (where its other ways are)
//!   and, for each long way, whether it touches the coverage: what decides which ways the unit
//!   keeps (the unit step keeps every way of the piece that touches it, owned or not, for junction
//!   context). Adding a region far along a ferry that already touched the coverage reruns nothing.
//!
//! The ways counted are those tagged `highway`, `railway` or `route=ferry`: a superset of what
//! `extract` keeps, so nothing a unit builds is missed (an extra way only costs a run that keeps
//! nothing).

use crate::coverage::Coverage;
use crate::legacy::Unit;
use anyhow::{Context, Result};
use osmpbf::{Element, ElementReader};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Bumped when what a reach holds changes (the pass's reaches are made again).
pub const REACH_V: u32 = 1;

/// Ways whose box stays within the tile grown by this are a unit's ordinary ways (the piece's own
/// buffer is 10 km).
pub const LONG_KM: f64 = 20.0;

/// A way reaching past its tile grown by `LONG_KM`, kept whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongWay {
    pub owned: bool,
    /// A ferry route (`extract` doesn't densify those).
    #[serde(default)]
    pub ferry: bool,
    /// Its nodes (E7).
    pub verts: Vec<[i32; 2]>,
}

impl LongWay {
    /// Whether the way touches the coverage as the unit step tests it: on the points `extract` puts
    /// along it (its nodes, and between them every 8 m in North America and Japan, 15 m elsewhere,
    /// by its first node; ferries not; `rules` "spacing"), so a straight stretch cutting a corner of
    /// an outline between two nodes counts as it will there.
    pub fn touches(&self, cov: &Coverage) -> bool {
        let Some(&first) = self.verts.first() else { return false };
        if cov.contains(first) {
            return true;
        }
        let (lon, lat) = (first[0] as f64 * roadcore::E7, first[1] as f64 * roadcore::E7);
        let japan = (122.5..154.0).contains(&lon) && (20.0..46.5).contains(&lat);
        let spacing = if lon < -40.0 || japan { UNIT_SPACING_M } else { UNIT_SPACING_M.max(COARSE_SPACING_M) };
        for s in self.verts.windows(2) {
            let (a, c) = (s[0], s[1]);
            if !self.ferry {
                let d = roadcore::dist_m(a[0] as f64 * roadcore::E7, a[1] as f64 * roadcore::E7, c[0] as f64 * roadcore::E7, c[1] as f64 * roadcore::E7);
                if d > spacing {
                    let k = (d / spacing).ceil() as i64;
                    for j in 1..k {
                        let t = j as f64 / k as f64;
                        let p = [(a[0] as f64 + (c[0] - a[0]) as f64 * t).round() as i32, (a[1] as f64 + (c[1] - a[1]) as f64 * t).round() as i32];
                        if cov.contains(p) {
                            return true;
                        }
                    }
                }
            }
            if cov.contains(c) {
                return true;
            }
        }
        false
    }
}

/// The unit step's densification (`unit::Tools::spacing_m`, `extract`'s spacing outside North America
/// and Japan): the points `LongWay::touches` tests.
const UNIT_SPACING_M: f64 = 8.0;
const COARSE_SPACING_M: f64 = 15.0;

/// A unit's reach.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    /// The box (w, s, e, n, E7) of the ordinary ways the unit owns; None when it owns none.
    pub owned: Option<[i32; 4]>,
    pub long: Vec<LongWay>,
}

/// A pass's reaches, by unit ("6/x/y"); units whose piece has no counted way aren't listed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reaches {
    pub fmt: u32,
    pub date: String,
    pub units: BTreeMap<String, Reach>,
}

/// The logical name of a pass's reaches.
pub fn logical(date: &str) -> String {
    format!("sources/osm/{date}/reach")
}

/// A unit's tile grown by `LONG_KM` (E7): where its ordinary ways are.
pub fn near_box(u: Unit) -> [i32; 4] {
    crate::hipack::grow(crate::hipack::tile_bounds(u.z, u.x, u.y), LONG_KM)
}

/// Why a pass's reaches couldn't be read.
#[derive(Debug)]
pub enum LoadError {
    /// The file couldn't be read (the NAS): try again later.
    Io(std::io::Error),
    /// It was read but doesn't decode (damaged, or another version's): make it again.
    Bad(String),
}

impl Reaches {
    /// The pass's reaches from the build manifest (None when the pass has none yet).
    pub fn load(root: &Path, m: &BTreeMap<String, String>, date: &str) -> Result<Option<Reaches>, LoadError> {
        let Some(c) = m.get(&logical(date)) else { return Ok(None) };
        let raw = std::fs::read(root.join(c)).map_err(LoadError::Io)?;
        let b = zstd::decode_all(&raw[..]).map_err(|e| LoadError::Bad(format!("{c}: {e}")))?;
        serde_json::from_slice(&b).map(Some).map_err(|e| LoadError::Bad(format!("{c}: {e}")))
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(zstd::encode_all(&serde_json::to_vec(self)?[..], 9)?)
    }

    pub fn get(&self, u: Unit) -> Option<&Reach> {
        self.units.get(&u.slash())
    }
}

impl Reach {
    /// Whether the coverage builds this unit: its owned box meets the coverage, or one of its own
    /// long ways touches it.
    pub fn builds(&self, cov: &Coverage) -> bool {
        self.owned.is_some_and(|o| cov.meets_rect(o)) || self.long.iter().any(|w| w.owned && w.touches(cov))
    }

    /// What of the coverage the unit reads, for its key: the coverage inside its tile grown by
    /// `LONG_KM`, and which of its long ways touch the coverage.
    pub fn coverage_key(&self, cov: &Coverage, u: Unit) -> String {
        let touching: String = self.long.iter().map(|w| if w.touches(cov) { '1' } else { '0' }).collect();
        format!("{}|{touching}", cov.fingerprint(near_box(u)))
    }

    /// The box of the ways the unit owns, wherever they go (E7): its tile, its owned box and its own
    /// long ways. A base pack's ways lie inside it (the map tiles are keyed by it).
    pub fn owned_extent(&self, u: Unit) -> [i32; 4] {
        let mut b = crate::hipack::tile_bounds(u.z, u.x, u.y);
        let mut add = |p: [i32; 4]| b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[2]), b[3].max(p[3])];
        if let Some(o) = self.owned {
            add(o);
        }
        for w in self.long.iter().filter(|w| w.owned) {
            for p in &w.verts {
                add([p[0], p[1], p[0], p[1]]);
            }
        }
        b
    }

    /// The box of everything the unit's ways reach (E7): its tile grown by `LONG_KM` and its long
    /// ways.
    pub fn extent(&self, u: Unit) -> [i32; 4] {
        let mut b = near_box(u);
        for p in self.long.iter().flat_map(|w| &w.verts) {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        b
    }
}

/// Whether a way counts (a road, a track or a ferry), and whether it's a ferry.
fn counted(w: &osmpbf::Way) -> Option<bool> {
    let mut road = false;
    for (k, v) in w.tags() {
        if k == "route" && v == "ferry" {
            return Some(true);
        }
        road |= k == "highway" || k == "railway";
    }
    road.then_some(false)
}

fn within(b: [i32; 4], p: [i32; 2]) -> bool {
    p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]
}

/// The reach of unit `u` from its piece (None when no way of the piece counts).
pub fn of_piece(piece: &Path, u: Unit) -> Result<Option<Reach>> {
    // Pass 1, ways: each counted way's node ids (flat).
    #[derive(Default)]
    struct Ways {
        refs: Vec<i64>,
        /// (way id, start in refs, count, a ferry)
        ways: Vec<(i64, usize, usize, bool)>,
    }
    let r = ElementReader::from_path(piece).with_context(|| format!("open {}", piece.display()))?;
    let mut w = r.par_map_reduce(
        |el| {
            let mut out = Ways::default();
            if let Element::Way(w) = el {
                if let Some(ferry) = counted(&w) {
                    out.refs.extend(w.refs());
                    if !out.refs.is_empty() {
                        out.ways.push((w.id(), 0, out.refs.len(), ferry));
                    }
                }
            }
            out
        },
        Ways::default,
        |mut a, b| {
            let off = a.refs.len();
            a.refs.extend_from_slice(&b.refs);
            a.ways.extend(b.ways.iter().map(|&(id, s, n, f)| (id, s + off, n, f)));
            a
        },
    )?;
    if w.ways.is_empty() {
        return Ok(None);
    }
    // (Blobs are reduced in no fixed order: the ways by id, so the long ways' order is stable.)
    w.ways.sort_unstable_by_key(|x| x.0);
    // Pass 2, nodes: the coordinates of the nodes those ways use.
    let mut needed = w.refs.clone();
    needed.sort_unstable();
    needed.dedup();
    let r = ElementReader::from_path(piece)?;
    let mut coords: Vec<(i64, [i32; 2])> = r.par_map_reduce(
        |el| {
            let (id, p) = match el {
                Element::Node(n) => (n.id(), [n.decimicro_lon(), n.decimicro_lat()]),
                Element::DenseNode(n) => (n.id(), [n.decimicro_lon(), n.decimicro_lat()]),
                _ => return Vec::new(),
            };
            if needed.binary_search(&id).is_ok() {
                vec![(id, p)]
            } else {
                Vec::new()
            }
        },
        Vec::new,
        |mut a, mut b| {
            a.append(&mut b);
            a
        },
    )?;
    coords.sort_unstable_by_key(|c| c.0);
    let at = |id: i64| coords.binary_search_by_key(&id, |c| c.0).ok().map(|i| coords[i].1);
    let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
    let near = near_box(u);
    let mut reach = Reach::default();
    for &(_, s, n, ferry) in &w.ways {
        let verts: Vec<[i32; 2]> = w.refs[s..s + n].iter().filter_map(|&id| at(id)).collect();
        let Some(&first) = verts.first() else { continue };
        // Owned by its first node the piece has, as `extract` (which drops missing nodes) and the
        // unit step see it.
        let owned = crate::unit::owns(tb, first);
        if verts.iter().all(|&p| within(near, p)) {
            if owned {
                let mut b = reach.owned.unwrap_or([first[0], first[1], first[0], first[1]]);
                for p in &verts {
                    b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                }
                reach.owned = Some(b);
            }
        } else {
            reach.long.push(LongWay { owned, ferry, verts });
        }
    }
    Ok(Some(reach))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;

    fn e7(x: f64, y: f64) -> [i32; 2] {
        [(x * 1e7) as i32, (y * 1e7) as i32]
    }

    #[test]
    fn long_ways_decide_by_touching() {
        let d = tempfile::tempdir().unwrap();
        let cov = |place: &str| Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec![place.into()] }], None, d.path()).unwrap();
        // A tile owning a ferry from its coast (-20, 65) far east to (0, 60); its own roads far from
        // the coverage.
        let u = Unit { z: 6, x: 28, y: 16 };
        let ferry = LongWay { owned: true, ferry: true, verts: vec![e7(-20.0, 65.0), e7(-10.0, 62.0), e7(0.0, 60.0)] };
        let r = Reach { owned: Some([e7(-21.0, 64.5)[0], e7(-21.0, 64.5)[1], e7(-20.5, 65.0)[0], e7(-20.5, 65.0)[1]]), long: vec![ferry] };
        // Coverage around the ferry's far end: built for the ferry.
        assert!(r.builds(&cov("place:0,60,30")));
        // Coverage near neither: not built.
        assert!(!r.builds(&cov("place:5,50,30")));
        // The far end's coverage grows; the ferry touched it before and after: the same key.
        assert_eq!(r.coverage_key(&cov("place:0,60,30"), u), r.coverage_key(&cov("place:0,60,40"), u));
        // The ferry no longer touches: another key.
        assert_ne!(r.coverage_key(&cov("place:0,60,30"), u), r.coverage_key(&cov("place:5,50,30"), u));
        assert!(r.extent(u)[2] >= e7(0.0, 60.0)[0]);
    }

    #[test]
    fn a_road_cutting_a_corner_between_nodes_touches() {
        let d = tempfile::tempdir().unwrap();
        // A 2 km circle; a straight road whose two nodes are 50 km apart on either side, passing
        // 1 km from its centre: no node inside, but the points extract puts along it are.
        let cov = Coverage::from_recipes(&[Recipe { id: "c".into(), name: "C".into(), outline: vec!["place:0,50.0,2".into()] }], None, d.path()).unwrap();
        let road = LongWay { owned: true, ferry: false, verts: vec![e7(-0.35, 50.009), e7(0.35, 50.009)] };
        assert!(!cov.touches(&road.verts));
        assert!(road.touches(&cov));
        // A ferry isn't densified: its nodes only.
        assert!(!LongWay { ferry: true, ..road }.touches(&cov));
    }

    #[test]
    fn reaches_round_trip() {
        let mut r = Reaches { fmt: 1, date: "2026-09-28".into(), ..Default::default() };
        r.units.insert("6/31/20".into(), Reach { owned: Some([1, 2, 3, 4]), long: vec![LongWay { owned: false, ferry: false, verts: vec![[5, 6], [7, 8]] }] });
        let back: Reaches = serde_json::from_slice(&zstd::decode_all(&r.encode().unwrap()[..]).unwrap()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.get(Unit { z: 6, x: 31, y: 20 }).unwrap().owned, Some([1, 2, 3, 4]));
    }
}
