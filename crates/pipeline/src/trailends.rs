//! Hiking routes' ends, worldwide (docs/phase5.md "trailends"), from the pass's `hikes` set
//! (route=hiking or foot relations with their member ways): each simple linear route's two ends.
//! A route's ends are its way ends used once (where it starts and finishes), kept when there are
//! exactly two, as extract.rs found them in a build's input (branches and loops don't say which
//! end is the way in). Units read them instead of working them out from their piece, where a
//! route that leaves the piece and comes back would show other ends.

use anyhow::{Context, Result};
use osmpbf::{Element, ElementReader};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;

/// One end of a route.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct End {
    pub rel: i64,
    pub node: i64,
    /// E7.
    pub lon: i32,
    pub lat: i32,
    /// The route's name, else its ref.
    pub name: String,
}

fn concat<T>(mut a: Vec<T>, mut b: Vec<T>) -> Vec<T> {
    a.append(&mut b);
    a
}

/// The ends of the routes in `pbf`, by relation then node.
pub fn ends(pbf: &Path) -> Result<Vec<End>> {
    // Pass 1: the routes and their member ways (as listed, repeats included).
    let mut hikes: Vec<(i64, String, Vec<i64>)> = ElementReader::from_path(pbf)
        .with_context(|| format!("open {}", pbf.display()))?
        .par_map_reduce(
            |el| match el {
                Element::Relation(r) => {
                    let mut route = None;
                    let (mut name, mut ref_) = (None, None);
                    for (k, v) in r.tags() {
                        match k {
                            "route" => route = Some(v),
                            "name" => name = Some(v),
                            "ref" => ref_ = Some(v),
                            _ => {}
                        }
                    }
                    if !matches!(route, Some("hiking" | "foot")) {
                        return Vec::new();
                    }
                    let ids: Vec<i64> = r.members().filter(|m| m.member_type == osmpbf::RelMemberType::Way).map(|m| m.member_id).collect();
                    if ids.is_empty() {
                        return Vec::new();
                    }
                    vec![(r.id(), name.or(ref_).unwrap_or("").replace('\n', " "), ids)]
                }
                _ => Vec::new(),
            },
            Vec::new,
            concat,
        )?;
    hikes.sort_by_key(|h| h.0);
    // Pass 2: the member ways' end nodes.
    let mut ids: Vec<i64> = hikes.iter().flat_map(|h| h.2.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut way_ends: Vec<(i64, i64, i64)> = ElementReader::from_path(pbf)?.par_map_reduce(
        |el| match el {
            Element::Way(w) if ids.binary_search(&w.id()).is_ok() => {
                let refs: Vec<i64> = w.refs().collect();
                match (refs.first(), refs.last()) {
                    (Some(&a), Some(&b)) => vec![(w.id(), a, b)],
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        },
        Vec::new,
        concat,
    )?;
    way_ends.sort_unstable();
    way_ends.dedup_by_key(|e| e.0);
    // A route's ends: way ends used once, when there are exactly two.
    let mut want: Vec<(i64, i64, &str)> = Vec::new();
    for (rel, name, ids) in &hikes {
        let mut deg: BTreeMap<i64, u32> = BTreeMap::new();
        for id in ids {
            if let Ok(i) = way_ends.binary_search_by_key(id, |e| e.0) {
                *deg.entry(way_ends[i].1).or_default() += 1;
                *deg.entry(way_ends[i].2).or_default() += 1;
            }
        }
        let ends: Vec<i64> = deg.iter().filter(|(_, &d)| d == 1).map(|(&n, _)| n).collect();
        if ends.len() == 2 {
            want.extend(ends.into_iter().map(|n| (*rel, n, name.as_str())));
        }
    }
    // Pass 3: where the end nodes are.
    let mut nodes: Vec<i64> = want.iter().map(|w| w.1).collect();
    nodes.sort_unstable();
    nodes.dedup();
    let mut pos: Vec<(i64, i32, i32)> = ElementReader::from_path(pbf)?.par_map_reduce(
        |el| match el {
            Element::DenseNode(n) if nodes.binary_search(&n.id()).is_ok() => vec![(n.id(), n.decimicro_lon(), n.decimicro_lat())],
            Element::Node(n) if nodes.binary_search(&n.id()).is_ok() => vec![(n.id(), n.decimicro_lon(), n.decimicro_lat())],
            _ => Vec::new(),
        },
        Vec::new,
        concat,
    )?;
    pos.sort_unstable();
    pos.dedup_by_key(|p| p.0);
    let mut out: Vec<End> = want
        .into_iter()
        .filter_map(|(rel, node, name)| {
            let i = pos.binary_search_by_key(&node, |p| p.0).ok()?;
            Some(End { rel, node, lon: pos[i].1, lat: pos[i].2, name: name.to_string() })
        })
        .collect();
    out.sort_by_key(|e| (e.rel, e.node));
    Ok(out)
}

/// Written as zstd JSON lines, one end per line.
pub fn write(path: &Path, ends: &[End]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut w = zstd::Encoder::new(std::fs::File::create(&tmp)?, 9)?.auto_finish();
        for e in ends {
            serde_json::to_writer(&mut w, e)?;
            w.write_all(b"\n")?;
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read(path: &Path) -> Result<Vec<End>> {
    let r = std::io::BufReader::new(zstd::Decoder::new(std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?)?);
    r.lines().map(|l| Ok(serde_json::from_str(&l?)?)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_and_read_alike() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("ends.jsonl.zst");
        let es = vec![End { rel: 5, node: 7, lon: -12_345_678, lat: 456_789_012, name: "Sentier \"des\" crêtes".into() }, End { rel: 5, node: 9, lon: 1, lat: 2, name: String::new() }];
        write(&p, &es).unwrap();
        assert_eq!(read(&p).unwrap(), es);
    }
}
