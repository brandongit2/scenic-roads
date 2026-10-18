//! Each base pack's extent (its header's `extent`, E7: where its ways lie), by content name, kept in
//! `state/build/extents.json` so a pack's header is read once (`ensure`, by the pack and lo jobs),
//! not each time the owners of a map tile are worked out. A base pack's ways should lie where its
//! unit's reach puts them (crate::agent::build::unit_ways); one built by other reaches may lie
//! further, and the owners take that in (`crate::agent::build::unit_reach`).

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

/// The extents of the base packs manifest `m` lists: those kept, the rest read from their packs'
/// headers and kept too (with those of packs `m` no longer lists dropped). How many were read.
pub fn ensure(root: &Path, m: &BTreeMap<String, String>) -> Result<(Extents, usize)> {
    let mut known = load(root);
    let contents: Vec<&String> = m.iter().filter(|(l, _)| l.starts_with("base/")).map(|(_, c)| c).collect();
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
    let before = known.len();
    known.retain(|c, _| contents.iter().any(|x| *x == c));
    if read > 0 || known.len() != before {
        let path = root.join(FILE);
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&known)?)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok((known, read))
}

/// What the file is now (its size and time), for a reader that keeps what it read.
pub fn stamp(root: &Path) -> String {
    std::fs::metadata(root.join(FILE)).map(|m| format!("{} {:?}", m.len(), m.modified().ok())).unwrap_or_default()
}
