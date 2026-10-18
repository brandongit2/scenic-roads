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
use std::path::Path;

/// The file, under the project's root.
pub const FILE: &str = "state/build/extents.json";

/// Base packs' extents by content name.
pub type Extents = BTreeMap<String, [i32; 4]>;

/// The extents kept (none when the file isn't there or can't be read).
pub fn load(root: &Path) -> Extents {
    std::fs::read(root.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// `known`, with the extents of manifest `m`'s base packs it lacks read from their headers, and
/// those of packs `m` doesn't list dropped. How many headers were read. Writes nothing.
pub fn fill(root: &Path, m: &BTreeMap<String, String>, mut known: Extents) -> Result<(Extents, usize)> {
    let contents: std::collections::BTreeSet<&String> = m.iter().filter(|(l, _)| l.starts_with("base/")).map(|(_, c)| c).collect();
    let mut read = 0;
    for c in &contents {
        if known.contains_key(*c) {
            continue;
        }
        let r = store::sect::SectReader::open(store::range::PlainFile::open(&root.join(c))?)?;
        let e: Vec<i32> = serde_json::from_value(r.meta().get("extent").cloned().unwrap_or_default())?;
        anyhow::ensure!(e.len() == 4, "{c}: no extent in its header");
        known.insert((*c).clone(), [e[0], e[1], e[2], e[3]]);
        read += 1;
    }
    known.retain(|c, _| contents.contains(c));
    Ok((known, read))
}

/// The lead's keeping (as it plans, with its manifest `m`): the file brought in line with `m`
/// (`fill`), written, by a temporary name of this Mac and process then renamed, only when it
/// changed. How many headers were read.
pub fn keep(root: &Path, m: &BTreeMap<String, String>) -> Result<usize> {
    let old = load(root);
    let (new, read) = fill(root, m, old.clone())?;
    if new != old {
        let path = root.join(FILE);
        let host = store::sys::hostname().unwrap_or_else(|| "host".into());
        let tmp = path.with_extension(format!("json.{host}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&new)?)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(read)
}

/// What the file is now (its size and time), for a reader that keeps what it read.
pub fn stamp(root: &Path) -> String {
    std::fs::metadata(root.join(FILE)).map(|m| format!("{} {:?}", m.len(), m.modified().ok())).unwrap_or_default()
}
