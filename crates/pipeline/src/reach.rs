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
    /// Its vertices (E7).
    pub verts: Vec<[i32; 2]>,
}

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

impl Reaches {
    /// The pass's reaches from the build manifest (None when the pass has none yet).
    pub fn load(root: &Path, m: &BTreeMap<String, String>, date: &str) -> Option<Reaches> {
        let c = m.get(&logical(date))?;
        let b = zstd::decode_all(std::fs::File::open(root.join(c)).ok()?).ok()?;
        serde_json::from_slice(&b).ok()
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
        self.owned.is_some_and(|o| cov.meets_rect(o)) || self.long.iter().any(|w| w.owned && cov.touches(&w.verts))
    }

    /// What of the coverage the unit reads, for its key: the coverage inside its tile grown by
    /// `LONG_KM`, and which of its long ways touch the coverage.
    pub fn coverage_key(&self, cov: &Coverage, u: Unit) -> String {
        let touching: String = self.long.iter().map(|w| if cov.touches(&w.verts) { '1' } else { '0' }).collect();
        format!("{}|{touching}", cov.fingerprint(near_box(u)))
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

fn counted(w: &osmpbf::Way) -> bool {
    w.tags().any(|(k, v)| k == "highway" || k == "railway" || (k == "route" && v == "ferry"))
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
        /// (way id, start in refs, count)
        ways: Vec<(i64, usize, usize)>,
    }
    let r = ElementReader::from_path(piece).with_context(|| format!("open {}", piece.display()))?;
    let mut w = r.par_map_reduce(
        |el| {
            let mut out = Ways::default();
            if let Element::Way(w) = el {
                if counted(&w) {
                    out.refs.extend(w.refs());
                    if !out.refs.is_empty() {
                        out.ways.push((w.id(), 0, out.refs.len()));
                    }
                }
            }
            out
        },
        Ways::default,
        |mut a, b| {
            let off = a.refs.len();
            a.refs.extend_from_slice(&b.refs);
            a.ways.extend(b.ways.iter().map(|&(id, s, n)| (id, s + off, n)));
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
    for &(_, s, n) in &w.ways {
        let verts: Vec<[i32; 2]> = w.refs[s..s + n].iter().filter_map(|&id| at(id)).collect();
        let Some(&first) = verts.first() else { continue };
        // (A way whose first node is missing from the piece can't be owned by anyone here.)
        let owned = at(w.refs[s]).is_some_and(|p| crate::unit::owns(tb, p));
        if verts.iter().all(|&p| within(near, p)) {
            if owned {
                let mut b = reach.owned.unwrap_or([first[0], first[1], first[0], first[1]]);
                for p in &verts {
                    b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                }
                reach.owned = Some(b);
            }
        } else {
            reach.long.push(LongWay { owned, verts });
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
        let ferry = LongWay { owned: true, verts: vec![e7(-20.0, 65.0), e7(-10.0, 62.0), e7(0.0, 60.0)] };
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
    fn reaches_round_trip() {
        let mut r = Reaches { fmt: 1, date: "2026-09-28".into(), ..Default::default() };
        r.units.insert("6/31/20".into(), Reach { owned: Some([1, 2, 3, 4]), long: vec![LongWay { owned: false, verts: vec![[5, 6], [7, 8]] }] });
        let back: Reaches = serde_json::from_slice(&zstd::decode_all(&r.encode().unwrap()[..]).unwrap()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.get(Unit { z: 6, x: 31, y: 20 }).unwrap().owned, Some([1, 2, 3, 4]));
    }
}
