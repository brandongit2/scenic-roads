//! Each z6 tile's reach (docs/regions.md, Coverage): where the roads, rail and ferries of its OSM
//! extract go, worked out once per pass (`scenic-build reach`) and kept as `sources/osm/<date>/reach`.
//! The planner picks the z6 tiles to build by it and keys them on the coverage they read.
//!
//! Most of a extract's ways stay within its tile grown by `LONG_KM`. For those only a box is kept: the
//! box of the ones the z6 tile owns (first node inside its tile). The few that reach further (ferries,
//! long rural roads) are kept whole, with whether the z6 tile owns each. Then:
//! - **A z6 tile is built** when its owned box meets the coverage, or one of its own long ways touches
//!   it. So a road starting in a tile far from every outline is built when it enters the coverage,
//!   and a tile owning a ferry to a far coast isn't built for that coast unless the ferry touches it.
//! - **Its key** names the coverage inside its tile grown by `LONG_KM` (where its other ways are)
//!   and, for each long way, whether it touches the coverage: what decides which ways the z6 tile
//!   keeps (the `unit` step keeps every way of the extract that touches it, owned or not, for junction
//!   context). Adding a region far along a ferry that already touched the coverage reruns nothing.
//!
//! The ways counted are those tagged `highway`, `railway` or `route=ferry`: a superset of what
//! `extract` keeps, so nothing a z6 tile builds is missed (an extra way only costs a run that keeps
//! nothing).

use crate::coverage::Coverage;
use crate::dem::spacing::Spacing;
use crate::legacy::TileId;
use anyhow::{Context, Result};
use osmpbf::{Element, ElementReader};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Bumped when what a reach holds changes (the pass's reaches are made again). (2: `cells`.)
pub const REACH_V: u32 = 2;

/// Ways whose box stays within the tile grown by this are a z6 tile's ordinary ways (the extract's own
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
    /// Whether the way touches the coverage as the `unit` step tests it: on the points `extract` puts
    /// along it (its nodes, and between them every `sp.at` its first node apart: crate::dem::spacing;
    /// ferries not), so a straight stretch cutting a corner of an outline between two nodes counts
    /// as it will there.
    pub fn touches(&self, cov: &Coverage, sp: &dyn Spacing) -> bool {
        let Some(&first) = self.verts.first() else { return false };
        let spacing = sp.at(first);
        // (Each shape's answer kept for these points: crate::coverage's `Memo`.)
        let mut d = blake3::Hasher::new();
        d.update(&[self.ferry as u8]);
        d.update(&spacing.to_le_bytes());
        d.update(bytemuck::cast_slice(&self.verts));
        let key: [u8; 32] = d.finalize().into();
        let mut b = [first[0], first[1], first[0], first[1]];
        for p in &self.verts {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        cov.shapes.iter().any(|s| s.touches_way(&key, b, || self.points(spacing)))
    }

    /// The points `touches` tests, in the box of its nodes: its nodes, and between them every
    /// `spacing`; a ferry's nodes alone.
    fn points(&self, spacing: f64) -> impl Iterator<Item = [i32; 2]> + '_ {
        let ferry = self.ferry;
        self.verts.first().copied().into_iter().chain(self.verts.windows(2).flat_map(move |s| {
            let (a, c) = (s[0], s[1]);
            let k = if ferry {
                1
            } else {
                let d = roadcore::dist_m(
                    a[0] as f64 * roadcore::E7,
                    a[1] as f64 * roadcore::E7,
                    c[0] as f64 * roadcore::E7,
                    c[1] as f64 * roadcore::E7,
                );
                if d > spacing {
                    (d / spacing).ceil() as i64
                } else {
                    1
                }
            };
            (1..k)
                .map(move |j| {
                    let t = j as f64 / k as f64;
                    [
                        (a[0] as f64 + (c[0] - a[0]) as f64 * t).round() as i32,
                        (a[1] as f64 + (c[1] - a[1]) as f64 * t).round() as i32,
                    ]
                })
                .chain(std::iter::once(c))
        }))
    }
}

/// A z6 tile's reach.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    /// The box (w, s, e, n, E7) of the ordinary ways the z6 tile owns; None when it owns none.
    pub owned: Option<[i32; 4]>,
    pub long: Vec<LongWay>,
    /// The 1° cells (crate::dem::spacing::cell) of the first nodes of its extract's ways, ferries
    /// left out, sorted: where `extract`'s spacing is decided for them (owned or not: the `unit` step
    /// keeps every way touching the coverage). Required, never `serde(default)`: a reach from
    /// before it would read as no cells, which every spacing agrees on (`rekey::spaced_as_before`
    /// vacuously true) and its key would name none; it fails to decode instead and is made again.
    pub cells: Vec<[i32; 2]>,
}

/// A pass's reaches, by z6 tile ("6/x/y"); z6 tiles whose extract has no counted way aren't listed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reaches {
    pub fmt: u32,
    pub date: String,
    /// (Stored as `units` in `reach.json.zst`, from when a z6 tile was a "unit": kept, as every pass's file has it.)
    pub units: BTreeMap<String, Reach>,
    /// The base packs' extents kept with them (crate::extents), read as the reaches are: what
    /// their headers say, by content name. Not part of the reaches' file.
    #[serde(skip)]
    pub extents: crate::extents::Extents,
}

/// The logical name of a pass's reaches.
pub fn logical(date: &str) -> String {
    format!("sources/osm/{date}/reach")
}

/// Z6 tiles `units`' reaches (`make`'s: none for a tile with no counted way), each a piece kept
/// (crate::resume_log::Pieces, task #890; a safe point once kept): a resumed run makes only those
/// its last didn't keep, the same values (whole numbers all, as JSON). By z6 tile ("6/x/y").
pub fn each_kept(
    pieces: &mut crate::resume_log::Pieces,
    units: &[TileId],
    make: &mut dyn FnMut(TileId) -> anyhow::Result<Option<Reach>>,
) -> anyhow::Result<BTreeMap<String, Reach>> {
    let mut all = BTreeMap::new();
    for &u in units {
        let p = pieces.piece((u.z, u.x, u.y), || Ok(serde_json::to_vec(&make(u)?)?))?;
        if let Some(r) = serde_json::from_slice::<Option<Reach>>(&p)? {
            all.insert(u.slash(), r);
        }
    }
    Ok(all)
}

/// A z6 tile's tile grown by `LONG_KM` (E7): where its ordinary ways are.
pub fn near_box(u: TileId) -> [i32; 4] {
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
    pub fn load(
        root: &store::nasfs::NasRoot,
        m: &BTreeMap<String, String>,
        date: &str,
    ) -> Result<Option<Reaches>, LoadError> {
        let Some(c) = m.get(&logical(date)) else { return Ok(None) };
        let raw = root.join(c).read().map_err(LoadError::Io)?;
        let b = zstd::decode_all(&raw[..]).map_err(|e| LoadError::Bad(format!("{c}: {e}")))?;
        let mut r: Reaches = serde_json::from_slice(&b).map_err(|e| LoadError::Bad(format!("{c}: {e}")))?;
        r.extents = crate::extents::load(root);
        Ok(Some(r))
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(zstd::encode_all(&serde_json::to_vec(self)?[..], 9)?)
    }

    pub fn get(&self, u: TileId) -> Option<&Reach> {
        self.units.get(&u.slash())
    }
}

impl Reach {
    /// Whether the coverage builds this z6 tile: its owned box meets the coverage, or one of its own
    /// long ways touches it.
    pub fn builds(&self, cov: &Coverage, sp: &dyn Spacing) -> bool {
        self.owned.is_some_and(|o| cov.meets_rect(o)) || self.long.iter().any(|w| w.owned && w.touches(cov, sp))
    }

    /// What of the coverage the z6 tile reads, for its key: the coverage inside its tile grown by
    /// `LONG_KM`, and which of its long ways touch the coverage.
    pub fn coverage_key(&self, cov: &Coverage, u: TileId, sp: &dyn Spacing) -> String {
        let touching: String = self.long.iter().map(|w| if w.touches(cov, sp) { '1' } else { '0' }).collect();
        format!("{}|{touching}", cov.fingerprint(near_box(u)))
    }

    /// The box of the ways the z6 tile owns, wherever they go (E7): its tile, its owned box and its own
    /// long ways. A base pack's ways lie inside it (the map tiles are keyed by it).
    pub fn owned_extent(&self, u: TileId) -> [i32; 4] {
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

    /// The box of everything the z6 tile's ways reach (E7): its tile grown by `LONG_KM` and its long
    /// ways.
    pub fn extent(&self, u: TileId) -> [i32; 4] {
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

/// The reach of z6 tile `u` from its extract (None when no way of the extract counts).
pub fn of_piece(piece: &Path, u: TileId) -> Result<Option<Reach>> {
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
    let mut cells = std::collections::BTreeSet::new();
    for &(_, s, n, ferry) in &w.ways {
        let verts: Vec<[i32; 2]> = w.refs[s..s + n].iter().filter_map(|&id| at(id)).collect();
        let Some(&first) = verts.first() else { continue };
        if !ferry {
            cells.insert(crate::dem::spacing::cell(first));
        }
        // Owned by its first node the extract has, as `extract` (which drops missing nodes) and the
        // `unit` step see it.
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
    reach.cells = cells.into_iter().collect();
    Ok(Some(reach))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;
    use crate::dem::spacing::{Fine, Uniform, FLOOR_M};

    fn e7(x: f64, y: f64) -> [i32; 2] {
        [(x * 1e7) as i32, (y * 1e7) as i32]
    }

    /// Task #890: the reaches paused at every z6 tile kept (`each_kept`), resumed each time, are
    /// the same as a run never paused, each tile worked out once.
    #[test]
    fn reaches_paused_at_every_tile_resume_to_the_same() {
        use crate::resume_log::{Pieces, STOP_AFTER};
        let units: Vec<TileId> = (0..5).map(|x| TileId { z: 6, x, y: 20 }).collect();
        let made = std::cell::Cell::new(0);
        let mut make = |u: TileId| -> Result<Option<Reach>> {
            made.set(made.get() + 1);
            Ok((u.x != 2).then(|| Reach {
                owned: Some([u.x as i32, 1, 2, 3]),
                long: vec![LongWay { owned: true, ferry: u.x == 3, verts: vec![[u.x as i32, -7], [9, 9]] }],
                cells: vec![[u.x as i32, 4]],
            }))
        };
        let fresh = each_kept(&mut Pieces::none(), &units, &mut make).unwrap();
        made.set(0);
        let d = tempfile::tempdir().unwrap();
        let mut stops = 0;
        let resumed = loop {
            STOP_AFTER.set(Some(1));
            let r = each_kept(&mut Pieces::open(d.path(), (0, 0, 0), "reach", "a", true), &units, &mut make);
            STOP_AFTER.set(None);
            match r {
                Err(e) => {
                    assert!(crate::control::is_paused(&e), "{e:#}");
                    stops += 1;
                }
                Ok(r) => break r,
            }
        };
        assert_eq!(stops, units.len(), "a stop after each tile kept");
        assert_eq!(resumed, fresh);
        assert_eq!(fresh.len(), 4, "a tile with no counted way isn't listed");
        assert_eq!(made.get(), units.len(), "each tile once");
    }

    /// A ferry crossing the open sea between two coasts, the sea a region with no land (the high
    /// seas: the coverage's `depth_only`): built for the land it reaches, its whole route kept (its
    /// z6 tile is built and the way touches the coverage), whatever the sea between is; a sea alone
    /// builds no z6 tile.
    #[test]
    fn a_ferry_across_a_sea_with_no_land_is_kept_by_its_coasts() {
        let d = tempfile::tempdir().unwrap();
        let fixture = |place: &str| {
            Coverage::fixture(
                &[Recipe { id: "r".into(), name: "R".into(), outline: vec![place.into()] }],
                None,
                d.path(),
            )
            .unwrap()
        };
        let ferry = LongWay { owned: true, ferry: true, verts: vec![e7(-20.0, 65.0), e7(-10.0, 62.0), e7(0.0, 60.0)] };
        let r = Reach {
            owned: Some([e7(-21.0, 64.5)[0], e7(-21.0, 64.5)[1], e7(-20.5, 65.0)[0], e7(-20.5, 65.0)[1]]),
            long: vec![ferry],
            cells: vec![],
        };
        let sp = &Uniform(15.0);
        // The sea between, a region with no land resolved as the regions are (cell by cell, its
        // land and the coastline's land none here): kept apart for the depth alone.
        use crate::inputs::regions::{data::Datasets, read, resolve};
        let files: std::collections::BTreeMap<String, Vec<u8>> = [
            ("_pins.toml", "osm = \"2026-09-28\"\n"),
            ("sea.toml", "id = \"sea\"\nname = \"Sea\"\nwater = [\"box:-16,58,-4,66\"]\n"),
        ]
        .iter()
        .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
        .collect();
        let folder = resolve::Folder::read(files.keys().cloned(), &|p| Ok(files[p].clone())).unwrap();
        let data = Datasets::from_parts(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            vec![vec![vec![[-200_000_000, 640_000_000], [-205_000_000, 640_000_000], [-205_000_000, 650_000_000]]]],
            "fixture",
        );
        let others = std::collections::BTreeMap::new();
        let ctx = resolve::Context {
            pass: Some("2026-09-28"),
            outlines: None,
            extract: None,
            others: &others,
            memo: None,
            names: String::new(),
            data: Some(&data),
        };
        let out = resolve::resolve(&folder, &ctx);
        assert!(out.problems.is_empty(), "{:?}", out.problems);
        let cov = read::coverage_of(&[out.resolved["sea"].geometry.clone()]);
        assert!(cov.shapes.is_empty() && cov.depth_only.len() == 1, "the sea is the depth's alone");
        let sea = cov.depth_only;
        // With the coast it reaches: built, and the way touches.
        let mut c = fixture("place:0,60,30");
        c.depth_only = sea.clone();
        assert!(r.builds(&c, sp) && r.long[0].touches(&c, sp));
        // The sea alone: nothing built for it.
        let alone = Coverage { shapes: Vec::new(), depth_only: sea };
        assert!(!r.builds(&alone, sp));
    }

    #[test]
    fn long_ways_decide_by_touching() {
        let d = tempfile::tempdir().unwrap();
        let cov = |place: &str| {
            Coverage::fixture(
                &[Recipe { id: "r".into(), name: "R".into(), outline: vec![place.into()] }],
                None,
                d.path(),
            )
            .unwrap()
        };
        // A tile owning a ferry from its coast (-20, 65) far east to (0, 60); its own roads far from
        // the coverage.
        let u = TileId { z: 6, x: 28, y: 16 };
        let ferry = LongWay { owned: true, ferry: true, verts: vec![e7(-20.0, 65.0), e7(-10.0, 62.0), e7(0.0, 60.0)] };
        let r = Reach {
            owned: Some([e7(-21.0, 64.5)[0], e7(-21.0, 64.5)[1], e7(-20.5, 65.0)[0], e7(-20.5, 65.0)[1]]),
            long: vec![ferry],
            cells: vec![],
        };
        let sp = &Uniform(15.0);
        // Coverage around the ferry's far end: built for the ferry.
        assert!(r.builds(&cov("place:0,60,30"), sp));
        // Coverage near neither: not built.
        assert!(!r.builds(&cov("place:5,50,30"), sp));
        // The far end's coverage grows; the ferry touched it before and after: the same key.
        assert_eq!(r.coverage_key(&cov("place:0,60,30"), u, sp), r.coverage_key(&cov("place:0,60,40"), u, sp));
        // The ferry no longer touches: another key.
        assert_ne!(r.coverage_key(&cov("place:0,60,30"), u, sp), r.coverage_key(&cov("place:5,50,30"), u, sp));
        assert!(r.extent(u)[2] >= e7(0.0, 60.0)[0]);
    }

    #[test]
    fn a_road_cutting_a_corner_between_nodes_touches() {
        let d = tempfile::tempdir().unwrap();
        // A 2 km circle; a straight road whose two nodes are 50 km apart on either side, passing
        // 1 km from its centre: no node inside, but the points extract puts along it are.
        let cov = Coverage::fixture(
            &[Recipe { id: "c".into(), name: "C".into(), outline: vec!["place:0,50.0,2".into()] }],
            None,
            d.path(),
        )
        .unwrap();
        let road = LongWay { owned: true, ferry: false, verts: vec![e7(-0.35, 50.009), e7(0.35, 50.009)] };
        assert!(!cov.touches(&road.verts));
        assert!(road.touches(&cov, &Uniform(15.0)));
        // A ferry isn't densified: its nodes only.
        assert!(!LongWay { ferry: true, ..road.clone() }.touches(&cov, &Uniform(15.0)));
        // By the terrain's spacing at its first node: a 4 m circle on a 60 m way through its centre,
        // met by points `FLOOR_M` apart (the lidar's: 2 m off it), not the base's (~24 m: 20 m
        // apart, 10 m off it).
        let small = Coverage::fixture(
            &[Recipe { id: "s".into(), name: "S".into(), outline: vec!["place:0.5,50.5,0.004".into()] }],
            None,
            d.path(),
        )
        .unwrap();
        let half = 30.0 / (111_195.0 * 50.5f64.to_radians().cos());
        let road = LongWay { owned: true, ferry: false, verts: vec![e7(0.5 - half, 50.5), e7(0.5 + half, 50.5)] };
        let fine = Fine { cells: vec![([0, 50], 16)] };
        assert_eq!(fine.at(road.verts[0]), FLOOR_M);
        assert!(road.touches(&small, &fine));
        assert!(!road.touches(&small, &Fine::default()));
    }

    #[test]
    fn reaches_round_trip() {
        let mut r = Reaches { fmt: 1, date: "2026-09-28".into(), ..Default::default() };
        r.units.insert(
            "6/31/20".into(),
            Reach {
                owned: Some([1, 2, 3, 4]),
                long: vec![LongWay { owned: false, ferry: false, verts: vec![[5, 6], [7, 8]] }],
                cells: vec![[0, 0], [1, 0]],
            },
        );
        let back: Reaches = serde_json::from_slice(&zstd::decode_all(&r.encode().unwrap()[..]).unwrap()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.get(TileId { z: 6, x: 31, y: 20 }).unwrap().owned, Some([1, 2, 3, 4]));
    }
}
