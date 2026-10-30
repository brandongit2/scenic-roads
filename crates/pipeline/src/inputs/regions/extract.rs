//! The regions' extract (docs/inputs.md §6.1.6, §13 item 21): the geometry of exactly the relations
//! outside the pass's outline set and the ways the regions' lines name, from the pinned pass's
//! planet, content-named as `sources/osm/<date>/regions-extract`. An id the pass doesn't have is
//! `null` in it, so "missing" is a fact of the extract. A relation's rings are assembled as the
//! outline set's are (crate::outlines::rings_of: ways joined at shared ends, gaps up to 1 km
//! closed). It never shrinks within a pass: a new job extracts the ids already there with the new.
//!
//! The `regions-extract` job (`scenic-build regions-extract --pass <date> --ids r1,w2,…`) reads in
//! three scans (relations, then their ways and the lines', then those ways' nodes; each by 256 MiB
//! ranges of blobs, a range's finds kept as a piece, task #897, so a resume scans only the rest) the pass's
//! filtered planet first (`sources/osm/<date>/filtered`, 60 GB for 2026-09-28: it keeps the
//! relations and ways of the pipeline's tags with their members, not every one), then, for the ids
//! it lacks, the pass's planet (95 GB; GC sweeps a pass's planet only once the pass is retired, so
//! the current one's is there). It runs alone on its Mac: its time is the step's measured timing.

use anyhow::{Context, Result};
use osmpbf::Element;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// A relation as extracted: its rings (E7, unclosed; none when they don't close into an area, with
/// why) and its tags.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub rings: Vec<Vec<[i32; 2]>>,
    pub tags: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// A way as extracted: its end nodes and its points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Way {
    pub ends: [u64; 2],
    pub line: Vec<[i32; 2]>,
}

/// The extract (`fmt` 1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Extract {
    pub fmt: u32,
    pub pass: String,
    pub relations: BTreeMap<String, Option<Relation>>,
    pub ways: BTreeMap<String, Option<Way>>,
}

/// The records' entry naming pass `date`'s extract.
pub fn logical(date: &str) -> String {
    format!("sources/osm/{date}/regions-extract")
}

/// An id as the extract and a check's needs name it: `r<id>` (a relation), `w<id>` (a way).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Id {
    R(u64),
    W(u64),
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Id::R(i) => write!(f, "r{i}"),
            Id::W(i) => write!(f, "w{i}"),
        }
    }
}

impl std::str::FromStr for Id {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Id> {
        let n =
            |t: &str| t.parse::<u64>().ok().filter(|v| *v > 0).with_context(|| format!("{s:?}: r<relation> or w<way>"));
        match s.split_at_checked(1) {
            Some(("r", t)) => Ok(Id::R(n(t)?)),
            Some(("w", t)) => Ok(Id::W(n(t)?)),
            _ => anyhow::bail!("{s:?}: r<relation> or w<way>"),
        }
    }
}

impl Extract {
    pub fn read(bytes: &[u8]) -> Result<Extract> {
        let e: Extract = serde_json::from_slice(bytes).context("parse the regions' extract")?;
        anyhow::ensure!(e.fmt == 1, "extract format {}", e.fmt);
        Ok(e)
    }

    /// Whether it has `id` (as found or as missing).
    pub fn has(&self, id: Id) -> bool {
        match id {
            Id::R(r) => self.relations.contains_key(&r.to_string()),
            Id::W(w) => self.ways.contains_key(&w.to_string()),
        }
    }

    /// Every id it has.
    pub fn ids(&self) -> BTreeSet<Id> {
        self.relations
            .keys()
            .filter_map(|k| k.parse().ok().map(Id::R))
            .chain(self.ways.keys().filter_map(|k| k.parse().ok().map(Id::W)))
            .collect()
    }

    pub fn relation(&self, id: u64) -> Option<&Option<Relation>> {
        self.relations.get(&id.to_string())
    }

    pub fn way(&self, id: u64) -> Option<&Option<Way>> {
        self.ways.get(&id.to_string())
    }
}

/// The job's key: the pass's date and the sorted ids.
pub fn key(date: &str, ids: &BTreeSet<Id>) -> String {
    let mut parts = vec![format!("regions-extract 1 {date}")];
    parts.extend(ids.iter().map(|i| i.to_string()));
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    crate::agent::build::h(&refs)
}

/// How much of a scanned file a piece is (task #897): the blobs whose headers start in each
/// 256 MiB of it, read, decoded and kept as one piece (`scan`): a few seconds of the NAS's reading.
pub const RANGE_BYTES: u64 = 256 << 20;

/// A relation as scan 1 finds it: its id, tags and member ways (role, id) in order.
type Rel = (i64, BTreeMap<String, String>, Vec<(String, i64)>);

/// The extract of `ids` from the OSM file `pbf` of pass `date`: three scans, each by ranges of
/// `range` bytes kept as pieces (`scan`) under `tag` (which of the job's files it is); a resumed
/// run scans only the ranges its last didn't keep, and as each range's finds are the same, so is
/// the extract.
pub fn make<'a>(
    pbf: impl Into<store::nasfs::Loc<'a>>,
    date: &str,
    ids: &BTreeSet<Id>,
    pieces: &mut crate::resume_log::Pieces,
    tag: u8,
    range: u64,
) -> Result<Extract> {
    let pbf = pbf.into();
    let want_r: HashSet<i64> =
        ids.iter().filter_map(|i| if let Id::R(r) = i { Some(*r as i64) } else { None }).collect();
    let want_w: HashSet<i64> =
        ids.iter().filter_map(|i| if let Id::W(w) = i { Some(*w as i64) } else { None }).collect();
    // Scan 1: the relations, their tags and member ways (role, id) in order.
    let rels: Vec<Rel> = scan(&pbf, pieces, (tag, 1), range, &|el| match el {
        Element::Relation(r) if want_r.contains(&r.id()) => {
            let tags = r.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            let ms = r
                .members()
                .filter(|m| m.member_type == osmpbf::RelMemberType::Way)
                .map(|m| (m.role().unwrap_or("").to_string(), m.member_id))
                .collect();
            Some((r.id(), tags, ms))
        }
        _ => None,
    })?;
    let member_ways: HashSet<i64> =
        rels.iter().flat_map(|r| r.2.iter().map(|m| m.1)).chain(want_w.iter().copied()).collect();
    // Scan 2: those ways' nodes.
    let ways: HashMap<i64, Vec<i64>> = scan(&pbf, pieces, (tag, 2), range, &|el| match el {
        Element::Way(w) if member_ways.contains(&w.id()) => Some((w.id(), w.refs().collect::<Vec<i64>>())),
        _ => None,
    })?
    .into_iter()
    .collect();
    let node_ids: HashSet<i64> = ways.values().flatten().copied().collect();
    // Scan 3: the nodes' points.
    let nodes: HashMap<i64, [i32; 2]> = scan(&pbf, pieces, (tag, 3), range, &|el| match el {
        Element::Node(n) if node_ids.contains(&n.id()) => Some((n.id(), [n.decimicro_lon(), n.decimicro_lat()])),
        Element::DenseNode(n) if node_ids.contains(&n.id()) => Some((n.id(), [n.decimicro_lon(), n.decimicro_lat()])),
        _ => None,
    })?
    .into_iter()
    .collect();
    Ok(assemble(date, ids, &rels, &ways, &nodes))
}

/// One scan of `pbf`: what `find` finds in its elements, in the file's order, by ranges of `range`
/// bytes (the blobs whose headers start in each), each range's finds a piece (block
/// `(tag.0 * 4 + tag.1, k, 0)` for range k; a safe point once kept). A range an earlier run kept is
/// passed over by its blobs' headers alone, no blob read.
fn scan<T: Serialize + serde::de::DeserializeOwned + Send>(
    pbf: &store::nasfs::Loc<'_>,
    pieces: &mut crate::resume_log::Pieces,
    tag: (u8, u8),
    range: u64,
    find: &(dyn Fn(Element) -> Option<T> + Sync),
) -> Result<Vec<T>> {
    use rayon::prelude::*;
    let f = pbf.open().with_context(|| format!("open {}", pbf.display()))?;
    let mut r = osmpbf::BlobReader::new_seekable(std::io::BufReader::with_capacity(4 << 20, f))?;
    let block = |k: u64| (tag.0 * 4 + tag.1, u32::try_from(k).unwrap_or(u32::MAX), 0);
    let mut all = Vec::new();
    // (A blob read past the end of the range it followed: the next range's first.)
    let mut carried: Option<(u64, osmpbf::Blob)> = None;
    loop {
        // The next blob's offset: its range.
        let at = match &carried {
            Some((at, _)) => *at,
            None => match r.next_header_skip_blob() {
                None => break,
                Some(h) => {
                    let at = h?.1.context("a blob's offset")?.0;
                    // (Back to it: read whole below, unless its range is kept.)
                    r.seek(osmpbf::ByteOffset(at))?;
                    at
                }
            },
        };
        let k = at / range;
        let end = (k + 1) * range;
        if let Some(p) = pieces.take(block(k)) {
            all.extend(serde_json::from_slice::<Vec<T>>(&p).context("a kept range's finds")?);
            carried = None;
            // (Its blobs passed over by their headers, to the first past it.)
            while let Some(h) = r.next_header_skip_blob() {
                let at = h?.1.context("a blob's offset")?.0;
                if at >= end {
                    r.seek(osmpbf::ByteOffset(at))?;
                    break;
                }
            }
            continue;
        }
        let mut blobs: Vec<osmpbf::Blob> = carried.take().map(|(_, b)| b).into_iter().collect();
        for b in r.by_ref() {
            let b = b?;
            let at = b.offset().context("a blob's offset")?.0;
            if at >= end {
                carried = Some((at, b));
                break;
            }
            blobs.push(b);
        }
        let found: Vec<Vec<T>> = blobs
            .par_iter()
            .map(|b| -> Result<Vec<T>> {
                Ok(match b.decode()? {
                    osmpbf::BlobDecode::OsmData(block) => block.elements().filter_map(find).collect(),
                    _ => Vec::new(),
                })
            })
            .collect::<Result<_>>()?;
        let found: Vec<T> = found.into_iter().flatten().collect();
        pieces.keep(block(k), &serde_json::to_vec(&found)?)?;
        all.extend(found);
        if carried.is_none() {
            break;
        }
    }
    Ok(all)
}

/// The extract from what the scans found.
pub fn assemble(
    date: &str,
    ids: &BTreeSet<Id>,
    rels: &[(i64, BTreeMap<String, String>, Vec<(String, i64)>)],
    ways: &HashMap<i64, Vec<i64>>,
    nodes: &HashMap<i64, [i32; 2]>,
) -> Extract {
    let mut e = Extract { fmt: 1, pass: date.to_string(), ..Default::default() };
    // A way's nodes with their points; None when a node is missing.
    let way_points = |w: i64| -> Option<Vec<(u64, [i32; 2])>> {
        ways.get(&w)?.iter().map(|n| nodes.get(n).map(|p| (*n as u64, *p))).collect()
    };
    let by_id: HashMap<i64, &(i64, BTreeMap<String, String>, Vec<(String, i64)>)> =
        rels.iter().map(|r| (r.0, r)).collect();
    for id in ids {
        match *id {
            Id::R(r) => {
                let v = by_id.get(&(r as i64)).map(|(_, tags, ms)| {
                    let mut problem = None;
                    let mut ws = Vec::new();
                    for (_, w) in ms {
                        match way_points(*w) {
                            Some(p) => ws.push(p),
                            None => problem = Some(format!("way {w}, a member, or a node of it isn't in the pass")),
                        }
                    }
                    let rings = if problem.is_some() {
                        Vec::new()
                    } else {
                        let mut segs = crate::outlines::segments_of(ws.iter());
                        match crate::outlines::rings_of(&ws, &mut segs) {
                            Ok((rings, _)) => rings
                                .into_iter()
                                .map(|mut r| {
                                    if r.len() > 1 && r.first() == r.last() {
                                        r.pop();
                                    }
                                    r
                                })
                                .collect(),
                            Err(err) => {
                                problem = Some(format!("{err:#}"));
                                Vec::new()
                            }
                        }
                    };
                    let problem = problem.or_else(|| rings.is_empty().then(|| "no ways".to_string()));
                    Relation { rings, tags: tags.clone(), problem }
                });
                e.relations.insert(r.to_string(), v);
            }
            Id::W(w) => {
                let v = way_points(w as i64)
                    .filter(|p| p.len() >= 2)
                    .map(|p| Way { ends: [p[0].0, p[p.len() - 1].0], line: p.iter().map(|x| x.1).collect() });
                e.ways.insert(w.to_string(), v);
            }
        }
    }
    e
}

/// The job: pass `date`'s extract of `ids` and every id its current one has (never shrinking within
/// a pass), stored and named in the records.
pub fn run(out: &mut crate::out::Out, date: &str, ids: &BTreeSet<Id>) -> Result<String> {
    use crate::timings::phase;
    let mut all = ids.clone();
    if let Some(name) = out.get(&logical(date)).map(str::to_string) {
        let e = Extract::read(&out.root().join(&name).read().with_context(|| format!("read {name}"))?)?;
        all.extend(e.ids());
    }
    let planet = out.root().join(format!("sources/osm/{date}/planet.osm.pbf"));
    let filtered = out.get(&format!("sources/osm/{date}/filtered")).map(|n| out.root().join(n));
    // (Each scan's ranges kept as pieces, task #897: by the ids, the filtered planet's name and the
    // planet's length, what the scans read.)
    let id = crate::resume_log::job_id(
        "regions-extract",
        "1",
        date,
        &[
            &key(date, &all),
            &crate::resume_log::names(out, [format!("sources/osm/{date}/filtered")]),
            &planet.metadata().map_or(0, |m| m.len()).to_string(),
        ],
    );
    let mut pieces =
        crate::resume_log::Pieces::open(&out.scratch, (0, 0, 0), "regions-extract", &id, crate::resume_log::on());
    let mut e = match &filtered {
        Some(f) => {
            let _p = phase("reading the filtered planet three times (relations, ways, nodes)");
            make(f, date, &all, &mut pieces, 0, RANGE_BYTES)?
        }
        None => {
            let _p = phase("reading the planet three times (relations, ways, nodes)");
            make(&planet, date, &all, &mut pieces, 1, RANGE_BYTES)?
        }
    };
    // (The ids the filtered planet lacks: from the planet itself, when it's there.)
    let lacking: BTreeSet<Id> = all
        .iter()
        .copied()
        .filter(|i| match i {
            Id::R(r) => e.relation(*r) == Some(&None),
            Id::W(w) => e.way(*w) == Some(&None),
        })
        .collect();
    if filtered.is_some() && !lacking.is_empty() && planet.exists() {
        let _p = phase("reading the planet three times for the ids the filtered planet lacks");
        let more = make(&planet, date, &lacking, &mut pieces, 1, RANGE_BYTES)?;
        e.relations.extend(more.relations);
        e.ways.extend(more.ways);
    }
    let p = phase("storing the extract");
    let name = out.store_bytes(&logical(date), "json", &serde_json::to_vec_pretty(&e)?)?;
    out.set(&logical(date), Some(name.clone()));
    out.save()?;
    pieces.finish();
    drop(p);
    let missing =
        e.relations.values().filter(|v| v.is_none()).count() + e.ways.values().filter(|v| v.is_none()).count();
    Ok(format!(
        "regions-extract {date}: {} relations, {} ways ({missing} not in the pass): {name}",
        e.relations.len(),
        e.ways.len()
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal OSM PBF (uncompressed blobs, granularity 100): what the extract's scans read.
    pub(crate) mod pbf {
        fn varint(out: &mut Vec<u8>, mut v: u64) {
            while v >= 0x80 {
                out.push((v as u8) | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        }
        fn key(out: &mut Vec<u8>, field: u32, wire: u8) {
            varint(out, ((field as u64) << 3) | wire as u64);
        }
        fn bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
            key(out, field, 2);
            varint(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        fn int(out: &mut Vec<u8>, field: u32, v: u64) {
            key(out, field, 0);
            varint(out, v);
        }
        fn zz(v: i64) -> u64 {
            ((v << 1) ^ (v >> 63)) as u64
        }
        fn packed(out: &mut Vec<u8>, field: u32, vs: &[u64]) {
            let mut b = Vec::new();
            for v in vs {
                varint(&mut b, *v);
            }
            bytes(out, field, &b);
        }
        fn blob(out: &mut Vec<u8>, kind: &str, data: &[u8]) {
            let mut blob = Vec::new();
            bytes(&mut blob, 1, data);
            let mut head = Vec::new();
            bytes(&mut head, 1, kind.as_bytes());
            int(&mut head, 3, blob.len() as u64);
            out.extend_from_slice(&(head.len() as u32).to_be_bytes());
            out.extend_from_slice(&head);
            out.extend_from_slice(&blob);
        }

        /// Nodes (id, lon, lat E7), ways (id, node ids), relations (id, tags, member ways (role, id)).
        pub fn file(
            nodes: &[(i64, i32, i32)],
            ways: &[(i64, Vec<i64>)],
            rels: &[(i64, Vec<(&str, &str)>, Vec<(&str, i64)>)],
        ) -> Vec<u8> {
            let mut out = Vec::new();
            let mut hb = Vec::new();
            bytes(&mut hb, 4, b"OsmSchema-V0.6");
            blob(&mut out, "OSMHeader", &hb);
            // The strings: "" first, then every tag's and role's.
            let mut strings: Vec<String> = vec![String::new()];
            let sid = |s: &str, strings: &mut Vec<String>| -> u64 {
                match strings.iter().position(|x| x == s) {
                    Some(i) => i as u64,
                    None => {
                        strings.push(s.to_string());
                        (strings.len() - 1) as u64
                    }
                }
            };
            let mut group = Vec::new();
            for (id, lon, lat) in nodes {
                let mut n = Vec::new();
                key(&mut n, 1, 0);
                varint(&mut n, zz(*id));
                key(&mut n, 8, 0);
                varint(&mut n, zz(*lat as i64));
                key(&mut n, 9, 0);
                varint(&mut n, zz(*lon as i64));
                bytes(&mut group, 1, &n);
            }
            let mut wgroup = Vec::new();
            for (id, refs) in ways {
                let mut w = Vec::new();
                int(&mut w, 1, *id as u64);
                let mut prev = 0i64;
                let deltas: Vec<u64> = refs
                    .iter()
                    .map(|r| {
                        let d = zz(r - prev);
                        prev = *r;
                        d
                    })
                    .collect();
                packed(&mut w, 8, &deltas);
                bytes(&mut wgroup, 3, &w);
            }
            let mut rgroup = Vec::new();
            for (id, tags, ms) in rels {
                let mut r = Vec::new();
                int(&mut r, 1, *id as u64);
                let ks: Vec<u64> = tags.iter().map(|t| sid(t.0, &mut strings)).collect();
                let vs: Vec<u64> = tags.iter().map(|t| sid(t.1, &mut strings)).collect();
                packed(&mut r, 2, &ks);
                packed(&mut r, 3, &vs);
                let roles: Vec<u64> = ms.iter().map(|m| sid(m.0, &mut strings)).collect();
                packed(&mut r, 8, &roles);
                let mut prev = 0i64;
                let mem: Vec<u64> = ms
                    .iter()
                    .map(|m| {
                        let d = zz(m.1 - prev);
                        prev = m.1;
                        d
                    })
                    .collect();
                packed(&mut r, 9, &mem);
                packed(&mut r, 10, &vec![1; ms.len()]);
                bytes(&mut rgroup, 4, &r);
            }
            let mut st = Vec::new();
            for s in &strings {
                bytes(&mut st, 1, s.as_bytes());
            }
            // (A blob each: nodes, ways, relations, so a scan's ranges can part them.)
            for g in [group, wgroup, rgroup] {
                if !g.is_empty() {
                    let mut block = Vec::new();
                    bytes(&mut block, 1, &st);
                    bytes(&mut block, 2, &g);
                    blob(&mut out, "OSMData", &block);
                }
            }
            out
        }
    }

    #[test]
    fn the_extract_has_the_ids_asked_and_says_which_are_missing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("planet.osm.pbf");
        // A square island of two ways (relation 7), a river of two ways (11, 12), a way of a node
        // that isn't there (13).
        let nodes =
            [(1, 0, 0), (2, 10_000_000, 0), (3, 10_000_000, 10_000_000), (4, 0, 10_000_000), (5, 20_000_000, 0)];
        let ways = vec![(9, vec![1, 2, 3]), (10, vec![3, 4, 1]), (11, vec![2, 5]), (12, vec![5, 3]), (13, vec![1, 99])];
        let rels = vec![(7, vec![("type", "multipolygon"), ("natural", "water")], vec![("outer", 9), ("outer", 10)])];
        std::fs::write(&p, pbf::file(&nodes, &ways, &rels)).unwrap();
        let ids: BTreeSet<Id> = ["r7", "r8", "w11", "w12", "w13"].iter().map(|s| s.parse().unwrap()).collect();
        let none = crate::resume_log::Pieces::none;
        let e = make(&p, "2026-09-28", &ids, &mut none(), 0, RANGE_BYTES).unwrap();
        let r = e.relation(7).unwrap().as_ref().unwrap();
        assert_eq!(r.rings, vec![vec![[0, 0], [10_000_000, 0], [10_000_000, 10_000_000], [0, 10_000_000]]]);
        assert_eq!(r.tags["natural"], "water");
        assert_eq!(r.problem, None);
        assert_eq!(e.relation(8), Some(&None), "missing: a fact of the extract");
        assert_eq!(
            e.way(11).unwrap().as_ref().unwrap(),
            &Way { ends: [2, 5], line: vec![[10_000_000, 0], [20_000_000, 0]] }
        );
        assert_eq!(e.way(13), Some(&None));
        assert!(e.has(Id::W(12)) && !e.has(Id::W(14)));
        assert_eq!(e.ids(), ids);
        // The same ids, the same bytes.
        assert_eq!(
            serde_json::to_vec(&e).unwrap(),
            serde_json::to_vec(&make(&p, "2026-09-28", &ids, &mut none(), 0, 1).unwrap()).unwrap(),
            "a range a blob: the same"
        );
        assert_eq!(key("2026-09-28", &ids), key("2026-09-28", &ids.iter().rev().copied().collect()));
    }

    /// Task #897: the extract paused after every range of every scan kept (ranges of a byte: a blob
    /// each), resumed each time, is the same as a run never paused.
    #[test]
    fn an_extract_paused_at_every_range_resumes_to_the_same() {
        use crate::resume_log::{Pieces, STOP_AFTER};
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("planet.osm.pbf");
        let nodes = [(1, 0, 0), (2, 10_000_000, 0), (3, 10_000_000, 10_000_000), (4, 0, 10_000_000)];
        let ways = vec![(9, vec![1, 2, 3]), (10, vec![3, 4, 1]), (11, vec![2, 3])];
        let rels = vec![(7, vec![("type", "multipolygon")], vec![("outer", 9), ("outer", 10)])];
        std::fs::write(&p, pbf::file(&nodes, &ways, &rels)).unwrap();
        let ids: BTreeSet<Id> = ["r7", "w11", "w12"].iter().map(|s| s.parse().unwrap()).collect();
        let fresh = make(&p, "2026-09-28", &ids, &mut Pieces::none(), 0, 1).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let mut stops = 0;
        let resumed = loop {
            STOP_AFTER.set(Some(1));
            let r = make(&p, "2026-09-28", &ids, &mut Pieces::open(scratch.path(), (0, 0, 0), "x", "a", true), 0, 1);
            STOP_AFTER.set(None);
            match r {
                Err(e) => {
                    assert!(crate::control::is_paused(&e), "{e:#}");
                    stops += 1;
                }
                Ok(e) => break e,
            }
        };
        // (Four blobs, the header's among them, each its own range, in each of the three scans.)
        assert_eq!(stops, 12, "a stop after each range of each scan");
        assert_eq!(serde_json::to_vec(&resumed).unwrap(), serde_json::to_vec(&fresh).unwrap());
        assert!(fresh.relation(7).unwrap().is_some() && fresh.way(11).unwrap().is_some());
    }
}
