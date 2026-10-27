//! The coastline's land (docs/inputs.md §6.1.7): the land side of the pass's own coastline, made
//! once per pass by the `regions-land` step from its `coastline` set and kept on disk by 2° cell
//! (`sources/osm/<date>/regions-land`), each cell read where it's asked for.
//!
//! - **Assembly** (`assemble`): the `natural=coastline` ways joined end to start into rings (land on
//!   their left, as OSM draws them); a chain drawn the wrong way (two chains start at its start,
//!   two end at its end) turned round; chains that don't close linked end to start (by node, else
//!   the nearest start within `GAP_M`); what never comes round is left out (counted). A ring that goes round a pole (Antarctica's, its
//!   longitudes unwrapped a whole turn apart at its ends) is closed along the pole's latitude. Each
//!   ring simplified within `SIMPLIFY_M`, as the zones are. OSM closes Antarctica along 85.0511°S,
//!   so the Earth south of 85°S is added as land (`cells`).
//! - **The file** (`encode`): magic `RLAND001`, a JSON head (`{"fmt", "pass", "stats", "cells":
//!   [[col, row, offset, length], …]}`), then each cell's rings (even–odd, E7, the cell's
//!   canonical area) as zigzag varint deltas, a ring's point count first.
//! - **Reading** (`Land`): the head once; a cell's rings by its offset, decoded into an area, the
//!   last `CACHED` cells kept. A cell that can't be read is no land and fails the land
//!   (`Land::failed`), which fails the check that read it.

use super::geom::{self, Area, Pt, Ring};
use super::tiles::{Cell, TArea};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const MAGIC: &[u8; 8] = b"RLAND001";

/// The step's version: in its key, so a change to how the land is made makes it again.
pub const LAND_V: u32 = 1;

/// Rings simplified within this (metres) as they're assembled.
pub const SIMPLIFY_M: f64 = super::data::SIMPLIFY_M;

/// The widest gap between a chain's end and an open chain's start closed by a straight line (m):
/// the 2026-09-28 pass's coastline was broken by 5.5 km at the Outer Banks (the Americas' ring)
/// and by 1.1 km in Scotland (Great Britain's).
pub const GAP_M: f64 = 10_000.0;

/// The cells kept decoded.
const CACHED: usize = 256;

/// The land's records name: per pass.
pub fn logical(date: &str) -> String {
    format!("sources/osm/{date}/regions-land")
}

/// What the assembly met.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub ways: u64,
    pub rings: u64,
    /// Chains drawn the wrong way (a way reversed in OSM: its start another chain's start, its end
    /// another's end), turned round before they're linked.
    #[serde(default)]
    pub reversed: u64,
    /// Gaps between chains closed (each under `GAP_M`).
    pub gaps_closed: u64,
    /// Chains left out: open, with no chain's start within `GAP_M` of their end.
    pub chains_dropped: u64,
    pub points_dropped: u64,
    /// Nodes a way names that the set hasn't.
    pub nodes_missing: u64,
    /// Rings round a pole.
    pub polar: u64,
    /// The widest gaps closed (at most 20): their length (m), and where (degrees).
    #[serde(default)]
    pub gaps: Vec<(f64, [f64; 2])>,
    /// The largest chains left out (at most 20): their points, first and last point (degrees).
    #[serde(default)]
    pub dropped: Vec<(u64, [f64; 2], [f64; 2])>,
}

#[derive(Serialize, Deserialize)]
struct Head {
    fmt: u32,
    pass: String,
    stats: Stats,
    cells: Vec<(i64, i64, u64, u64)>,
}

/// The coastline's ways in `pbf` (a pass's `coastline` set), each its points in order (E7).
pub fn read_ways(pbf: &Path, stats: &mut Stats) -> Result<Vec<Vec<(i64, Pt)>>> {
    use osmpbf::{Element, ElementReader};
    let concat = |mut a: Vec<(i64, Vec<i64>)>, mut b: Vec<(i64, Vec<i64>)>| {
        a.append(&mut b);
        a
    };
    let mut ways: Vec<(i64, Vec<i64>)> =
        ElementReader::from_path(pbf).with_context(|| format!("open {}", pbf.display()))?.par_map_reduce(
            |el| match el {
                Element::Way(w) if w.tags().any(|(k, v)| k == "natural" && v == "coastline") => {
                    vec![(w.id(), w.refs().collect())]
                }
                _ => Vec::new(),
            },
            Vec::new,
            concat,
        )?;
    ways.sort_by_key(|w| w.0);
    stats.ways = ways.len() as u64;
    let mut ids: Vec<i64> = ways.iter().flat_map(|w| w.1.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let cat = |mut a: Vec<(i64, Pt)>, mut b: Vec<(i64, Pt)>| {
        a.append(&mut b);
        a
    };
    let mut nodes: Vec<(i64, Pt)> = ElementReader::from_path(pbf)?.par_map_reduce(
        |el| {
            let (id, lon, lat) = match el {
                Element::Node(n) => (n.id(), n.nano_lon(), n.nano_lat()),
                Element::DenseNode(n) => (n.id(), n.nano_lon(), n.nano_lat()),
                _ => return Vec::new(),
            };
            if ids.binary_search(&id).is_err() {
                return Vec::new();
            }
            // (Nanodegrees to E7, rounded.)
            let e7 = |v: i64| (v as f64 / 100.0).round() as i64;
            vec![(id, [e7(lon), e7(lat)])]
        },
        Vec::new,
        cat,
    )?;
    drop(ids);
    nodes.sort_unstable_by_key(|n| n.0);
    let mut out = Vec::with_capacity(ways.len());
    for (_, refs) in ways {
        let mut pts = Vec::with_capacity(refs.len());
        for r in refs {
            match nodes.binary_search_by_key(&r, |n| n.0) {
                Ok(k) => pts.push((r, nodes[k].1)),
                Err(_) => stats.nodes_missing += 1,
            }
        }
        if pts.len() >= 2 {
            out.push(pts);
        }
    }
    Ok(out)
}

/// The ways joined end to start into rings (see the module's doc), each simplified.
pub fn assemble(ways: Vec<Vec<(i64, Pt)>>, stats: &mut Stats) -> Vec<Ring> {
    // Chains: ways joined while one's last node is another's first.
    let mut by_start: HashMap<i64, Vec<usize>> = HashMap::new();
    for (k, w) in ways.iter().enumerate() {
        by_start.entry(w[0].0).or_default().push(k);
    }
    let mut used = vec![false; ways.len()];
    let mut rings: Vec<Vec<Pt>> = Vec::new();
    let mut open: Vec<Vec<(i64, Pt)>> = Vec::new();
    for k in 0..ways.len() {
        if used[k] {
            continue;
        }
        used[k] = true;
        let mut chain: Vec<(i64, Pt)> = ways[k].clone();
        loop {
            let last = chain[chain.len() - 1].0;
            if last == chain[0].0 {
                break;
            }
            let Some(&n) = by_start.get(&last).and_then(|v| v.iter().find(|&&j| !used[j])) else { break };
            used[n] = true;
            chain.extend_from_slice(&ways[n][1..]);
        }
        if chain.len() > 1 && chain[chain.len() - 1].0 == chain[0].0 {
            chain.pop();
            rings.push(chain.into_iter().map(|p| p.1).collect());
        } else {
            open.push(chain);
        }
    }
    drop(ways);
    // Open chains (a ring broken somewhere, its ways chained from wherever they started): each
    // chain's end linked to the chain starting at its last node, else to the nearest chain start
    // within GAP_M (the nearest pairs first; each start taken once). Linked chains that come round
    // are rings; the rest, lines that never close, are left out.
    // (A way drawn the wrong way leaves two chains starting at its start and two ending at its
    // end; turned round, it links by its nodes, where the nearest start would bridge past it: the
    // 2026-09-28 pass's Outer Banks had four.)
    let mut n_start: HashMap<i64, usize> = HashMap::new();
    let mut n_end: HashMap<i64, usize> = HashMap::new();
    for c in &open {
        *n_start.entry(c[0].0).or_default() += 1;
        *n_end.entry(c[c.len() - 1].0).or_default() += 1;
    }
    // (The shortest first: where a few ways are tangled, the wrong way is the short fix.)
    let mut order: Vec<usize> = (0..open.len()).collect();
    order.sort_by_key(|&k| (open[k].len(), open[k][0].0));
    for k in order {
        let c = &mut open[k];
        let (a, z) = (c[0].0, c[c.len() - 1].0);
        if n_start[&a] > 1 && n_end[&z] > 1 && !n_start.contains_key(&z) && !n_end.contains_key(&a) {
            *n_start.get_mut(&a).unwrap() -= 1;
            *n_end.get_mut(&z).unwrap() -= 1;
            *n_start.entry(z).or_default() += 1;
            *n_end.entry(a).or_default() += 1;
            c.reverse();
            stats.reversed += 1;
        }
    }
    let n = open.len();
    let mut next: Vec<Option<usize>> = vec![None; n];
    let mut prev: Vec<Option<usize>> = vec![None; n];
    let starts: HashMap<i64, usize> = open.iter().enumerate().map(|(j, c)| (c[0].0, j)).collect();
    for i in 0..n {
        if let Some(&j) = starts.get(&open[i][open[i].len() - 1].0) {
            if prev[j].is_none() {
                next[i] = Some(j);
                prev[j] = Some(i);
            }
        }
    }
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for i in (0..n).filter(|&i| next[i].is_none()) {
        let end = open[i][open[i].len() - 1].1;
        for j in (0..n).filter(|&j| prev[j].is_none()) {
            let d = geom::length_m(end, open[j][0].1);
            if d <= GAP_M {
                pairs.push((d, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let deg = |p: Pt| [p[0] as f64 / 1e7, p[1] as f64 / 1e7];
    for (d, i, j) in pairs {
        if next[i].is_none() && prev[j].is_none() {
            next[i] = Some(j);
            prev[j] = Some(i);
            stats.gaps_closed += 1;
            stats.gaps.push((d.round(), deg(open[i][open[i].len() - 1].1)));
        }
    }
    stats.gaps.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1[0].total_cmp(&b.1[0])));
    stats.gaps.truncate(20);
    let mut seen = vec![false; n];
    // (Lines first: from each chain nothing links to.)
    for s0 in (0..n).filter(|&j| prev[j].is_none()) {
        let (mut k, mut pts, mut at) = (Some(s0), 0usize, s0);
        while let Some(c) = k {
            seen[c] = true;
            pts += open[c].len();
            at = c;
            k = next[c];
        }
        stats.chains_dropped += 1;
        stats.points_dropped += pts as u64;
        stats.dropped.push((pts as u64, deg(open[s0][0].1), deg(open[at][open[at].len() - 1].1)));
    }
    stats.dropped.sort_by(|a, b| b.0.cmp(&a.0).then(a.1[0].total_cmp(&b.1[0])));
    stats.dropped.truncate(20);
    // (Then the cycles, each from its least chain.)
    for s0 in 0..n {
        if seen[s0] {
            continue;
        }
        let mut ring: Vec<Pt> = Vec::new();
        let mut k = s0;
        loop {
            seen[k] = true;
            let c = &open[k];
            // (A chain linked by its last node: that node is the next chain's first.)
            let exact = next[k].is_some_and(|j| open[j][0].0 == c[c.len() - 1].0);
            let take = if exact { c.len() - 1 } else { c.len() };
            ring.extend(c[..take].iter().map(|p| p.1));
            k = next[k].expect("a cycle");
            if k == s0 {
                break;
            }
        }
        rings.push(ring);
    }
    // Each ring unwrapped (the short way round each step); one going round a pole closed along its
    // latitude; then simplified.
    let mut out = Vec::with_capacity(rings.len());
    for r in rings {
        if r.len() < 3 {
            continue;
        }
        let u = geom::unwrap(&r);
        // (Where the ring's last step back to its first point lands, unwrapped: a whole turn
        // from it when the ring went round a pole.)
        let back = {
            let d = r[0][0] - r[r.len() - 1][0];
            let d = if d > geom::HALF {
                d - geom::TURN
            } else if d < -geom::HALF {
                d + geom::TURN
            } else {
                d
            };
            u[u.len() - 1][0] + d
        };
        let round = back - u[0][0];
        if round.abs() < geom::HALF {
            let s = geom::simplify_ring(&u, SIMPLIFY_M);
            if s.len() >= 3 {
                out.push(s);
            }
            continue;
        }
        // Round a pole: the south's when the ring runs west, land on its left; the north's
        // otherwise. The coast simplified as a line (its ends kept), then closed a whole turn from
        // its start, along the pole's latitude (no simplifying there: at the pole a degree of
        // longitude is no length).
        stats.polar += 1;
        let pole = if round < 0 { -900_000_000 } else { 900_000_000 };
        let (a, z) = (u[0], u[u.len() - 1]);
        let mut s = geom::simplify_ring_pinned(&u, SIMPLIFY_M, &|p| *p == a || *p == z);
        s.push([back, a[1]]);
        s.push([back, pole]);
        s.push([a[0], pole]);
        out.push(s);
    }
    stats.rings = out.len() as u64;
    out
}

/// OSM's Antarctica is closed along Web Mercator's edge (85.0511°S) and the antimeridian; the land
/// is all of the Earth south of this (E7), its interior ice.
pub const SOUTH_E7: i64 = -850_000_000;

/// The rings cut into 2° cells, the Earth south of `SOUTH_E7` added (OSM's coastline stops short
/// of the pole: Antarctica's ring runs along 85.0511°S).
pub fn cells(rings: &[Ring]) -> TArea {
    let mut t = TArea::from_rings(rings);
    let cell = super::tiles::CELL;
    for x in -90..90 {
        for y in -45..=SOUTH_E7.div_euclid(cell) {
            let c = (x, y);
            let b = super::tiles::cell_box(c);
            let south = geom::box_area([b[0], b[1], b[2], b[3].min(SOUTH_E7)]);
            let a = t.get(c).union(&south);
            t.set(c, a);
        }
    }
    t
}

/// The land file's bytes made from a coastline set (`pbf`), its cell count and what the assembly
/// met: the step's whole work, here or on the lead.
pub fn make(pbf: &Path, pass: &str) -> Result<(Vec<u8>, usize, Stats)> {
    let mut stats = Stats::default();
    let rings = assemble(read_ways(pbf, &mut stats)?, &mut stats);
    let land = cells(&rings);
    drop(rings);
    Ok((encode(pass, &stats, &land), land.cells.len(), stats))
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

/// The file's bytes: the land by cell.
pub fn encode(pass: &str, stats: &Stats, land: &TArea) -> Vec<u8> {
    let mut body = Vec::new();
    let mut cells = Vec::new();
    for (c, a) in &land.cells {
        let at = body.len() as u64;
        let rings = a.rings();
        varint(&mut body, rings.len() as u64);
        for r in rings {
            varint(&mut body, r.len() as u64);
            let mut prev = [0i64; 2];
            for p in r {
                varint(&mut body, zigzag(p[0] - prev[0]));
                varint(&mut body, zigzag(p[1] - prev[1]));
                prev = p;
            }
        }
        cells.push((c.0, c.1, at, body.len() as u64 - at));
    }
    let head = serde_json::to_vec(&Head { fmt: 1, pass: pass.into(), stats: stats.clone(), cells }).expect("a head");
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(&body);
    out
}

fn decode_cell(b: &[u8]) -> Result<Area> {
    let mut at = 0usize;
    let mut read = || -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *b.get(at).context("a cell's rings end early")?;
            at += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        bail!("a varint too long")
    };
    let unzig = |v: u64| ((v >> 1) as i64) ^ -((v & 1) as i64);
    let n = read()? as usize;
    let mut rings = Vec::with_capacity(n.min(1 << 20));
    for _ in 0..n {
        let k = read()? as usize;
        let mut r = Vec::with_capacity(k.min(1 << 24));
        let mut prev = [0i64; 2];
        for _ in 0..k {
            prev = [prev[0] + unzig(read()?), prev[1] + unzig(read()?)];
            r.push(prev);
        }
        rings.push(r);
    }
    Ok(Area::from_rings(&rings))
}

/// The land, by cell: in memory (tests, fixtures) or in a file read where asked.
pub enum Land {
    Memory(BTreeMap<Cell, Area>),
    File {
        f: Mutex<std::fs::File>,
        index: HashMap<Cell, (u64, u64)>,
        cache: Mutex<(HashMap<Cell, Arc<Area>>, Vec<Cell>)>,
        /// The first cell that couldn't be read, and why: sticky; `failed` says it.
        failed: Mutex<Option<String>>,
        stats: Stats,
    },
}

impl Land {
    /// Land from rings (even–odd), cut into cells.
    pub fn of_rings(rings: &[Ring]) -> Land {
        Land::Memory(TArea::from_rings(rings).cells)
    }

    /// A land file, open: its head read.
    pub fn open(mut f: std::fs::File) -> Result<Land> {
        use std::io::Read;
        let mut m = [0u8; 12];
        f.read_exact(&mut m)?;
        ensure!(&m[..8] == MAGIC, "not the regions' land (RLAND001)");
        let n = u32::from_le_bytes(m[8..12].try_into().unwrap()) as usize;
        let mut h = vec![0u8; n];
        f.read_exact(&mut h)?;
        let head: Head = serde_json::from_slice(&h).context("the land's head")?;
        ensure!(head.fmt == 1, "land format {}", head.fmt);
        let base = 12 + n as u64;
        let index = head.cells.iter().map(|&(x, y, o, l)| ((x, y), (base + o, l))).collect();
        Ok(Land::File {
            f: Mutex::new(f),
            index,
            cache: Default::default(),
            failed: Default::default(),
            stats: head.stats,
        })
    }

    /// Whether it has any land.
    pub fn is_empty(&self) -> bool {
        match self {
            Land::Memory(m) => m.is_empty(),
            Land::File { index, .. } => index.is_empty(),
        }
    }

    /// An error if a cell couldn't be read: what was made from the land since is wrong (that
    /// cell read as no land), so the check that read it must fail, not report it.
    pub fn failed(&self) -> Result<()> {
        if let Land::File { failed, .. } = self {
            if let Some(e) = failed.lock().unwrap().as_ref() {
                bail!("{e}");
            }
        }
        Ok(())
    }

    /// The cell's land (wrapped column), empty where it has none. A cell that can't be read is
    /// empty too, not kept, and recorded for `failed`.
    pub fn cell(&self, c: Cell) -> Arc<Area> {
        match self {
            Land::Memory(m) => Arc::new(m.get(&c).cloned().unwrap_or_default()),
            Land::File { f, index, cache, failed, .. } => {
                let Some(&(off, len)) = index.get(&c) else { return Arc::new(Area::default()) };
                if let Some(a) = cache.lock().unwrap().0.get(&c) {
                    return a.clone();
                }
                let mut b = vec![0u8; len as usize];
                let read = {
                    use store::sys::PosIo;
                    let f = f.lock().unwrap();
                    f.read_exact_at(&mut b, off)
                };
                let a = match read.map_err(anyhow::Error::from).and_then(|_| decode_cell(&b)) {
                    Ok(a) => Arc::new(a),
                    Err(e) => {
                        failed.lock().unwrap().get_or_insert_with(|| {
                            format!("the regions' land, the cell from {}°, {}°: {e:#}", c.0 * 2, c.1 * 2)
                        });
                        return Arc::new(Area::default());
                    }
                };
                let mut k = cache.lock().unwrap();
                // (Another thread may have read it meanwhile.)
                if let Some(a) = k.0.get(&c) {
                    return a.clone();
                }
                if k.1.len() >= CACHED {
                    let old = k.1.remove(0);
                    k.0.remove(&old);
                }
                k.1.push(c);
                k.0.insert(c, a.clone());
                a
            }
        }
    }
}
