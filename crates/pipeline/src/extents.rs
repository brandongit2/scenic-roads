//! Each base pack's extent (its header's `extent`, E7: where its ways lie), by content name, kept in
//! `state/build/extents.json` by the lead (`keep`, as it plans) so a pack's header is read once, not
//! each time the owners of a map tile are worked out. Jobs only read the file, filling what it
//! lacks in memory from their own manifest's packs (`fill`), and write nothing: one writer, so no
//! update is lost between jobs, and the tiles' keys change only when the lead's file does. A base
//! pack's ways should lie where its unit's reach puts them (crate::agent::build::unit_ways); one
//! built by other reaches may lie further, and the owners take that in
//! (crate::agent::build::unit_reach).

use anyhow::Result;
use std::collections::BTreeMap;

/// The file, under the project's root.
pub const FILE: &str = "state/build/extents.json";

/// Base packs' extents by content name.
pub type Extents = BTreeMap<String, [i32; 4]>;

/// The extents kept (none when the file isn't there or can't be read).
pub fn load(root: &store::nasfs::NasRoot) -> Extents {
    root.join(FILE).read_replaced().ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// `known`, with the extents of manifest `m`'s base packs it lacks read from their headers, and
/// those of packs `m` doesn't list dropped. How many headers were read. Writes nothing. A pack
/// whose header can't be read, or has no extent, is said and left out (looked at again next time),
/// the rest kept: one left out is taken to lie within its reach (`unit_reach`).
pub fn fill(
    root: &store::nasfs::NasRoot,
    m: &BTreeMap<String, String>,
    mut known: Extents,
) -> Result<(Extents, usize)> {
    let contents: std::collections::BTreeSet<&String> =
        m.iter().filter(|(l, _)| l.starts_with("base/")).map(|(_, c)| c).collect();
    let mut read = 0;
    for c in &contents {
        if known.contains_key(*c) {
            continue;
        }
        match header_extent(&root.join(c)) {
            Ok(e) => {
                known.insert((*c).clone(), e);
                read += 1;
            }
            Err(e) => eprintln!("extents: {c} left out: {e:#}"),
        }
    }
    known.retain(|c, _| contents.contains(c));
    Ok((known, read))
}

/// A base pack's extent, from its header.
fn header_extent(p: &store::nasfs::NasPath) -> Result<[i32; 4]> {
    let r = store::sect::SectReader::open(p.open_plain()?)?;
    let e: Vec<i32> = serde_json::from_value(r.meta().get("extent").cloned().unwrap_or_default())?;
    anyhow::ensure!(e.len() == 4, "no extent in its header");
    Ok([e[0], e[1], e[2], e[3]])
}

/// The lead's keeping (as it plans, with its manifest `m`): the file brought in line with `m`
/// (`fill`), written whole (store::whole::write), only when it changed. How many headers were read.
pub fn keep(root: &store::nasfs::NasRoot, m: &BTreeMap<String, String>) -> Result<usize> {
    let old = load(root);
    let (new, read) = fill(root, m, old.clone())?;
    if new != old {
        root.join(FILE).write_whole(serde_json::to_vec(&new)?)?;
    }
    Ok(read)
}

/// What the file is now (its size and time), for a reader that keeps what it read.
pub fn stamp(root: &store::nasfs::NasRoot) -> String {
    root.join(FILE).metadata().map(|m| format!("{} {:?}", m.len(), m.modified().ok())).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    /// A pack whose header can't be read, or has no extent, is left out; the good ones are kept
    /// and written, so the next plan reads only the bad ones again.
    #[test]
    fn a_bad_pack_leaves_the_good_ones_kept() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("base")).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut m = std::collections::BTreeMap::new();
        for (u, ext) in [("6-1-1", Some([1, 2, 3, 4])), ("6-1-2", None), ("6-1-3", Some([5, 6, 7, 8]))] {
            let name = format!("base/{u}.000000000000000{}.sect", &u[4..]);
            let meta = match ext {
                Some(e) => serde_json::json!({ "extent": e }),
                None => serde_json::json!({}),
            };
            let mut w = store::sect::SectWriter::create(&root.join(&name), meta).unwrap();
            w.add("ways", &[]).unwrap();
            w.finish().unwrap();
            m.insert(format!("base/{u}"), name);
        }
        // (And one that isn't there at all.)
        m.insert("base/6-1-4".into(), "base/6-1-4.0000000000000004.sect".into());
        assert_eq!(super::keep(&store::nasfs::NasRoot::local_for_test(root), &m).unwrap(), 2);
        let kept = super::load(&store::nasfs::NasRoot::local_for_test(root));
        assert_eq!(kept.values().copied().collect::<Vec<_>>(), [[1, 2, 3, 4], [5, 6, 7, 8]]);
        assert_eq!(
            super::keep(&store::nasfs::NasRoot::local_for_test(root), &m).unwrap(),
            0,
            "only the bad ones looked at again"
        );
    }
}
