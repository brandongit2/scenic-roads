//! A unit's accepted version kept on a Mac for the map's server (docs/inputs.md §4.6, §6.3, §6.4):
//! the translations and the descriptions are served from the accepted copies the lead's records
//! (`state/build/manifest.json`) name, copied to the Mac so they work offline and load fast, and
//! read from that copy. The server never reads an inputs folder.
//!
//! On the Mac, in the unit's folder: `@index.json` (the index's content name and the index), and
//! each accepted file under its content name. A sync reads the records, and when they name another
//! index, copies the files it doesn't have yet (each checked against its content name's hash),
//! then the index, then removes the files no longer named.

use super::{logical, Index};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The records the map's server reads (the lead's, rewritten with every save: pool.md §6.2).
pub const RECORDS: &str = "state/build/manifest.json";

/// The local copy's index.
const LOCAL: &str = "@index.json";

/// Accepted files by inputs-folder path: the local copy and its content name.
pub type Files = BTreeMap<String, (PathBuf, String)>;

/// The local copy's index: the accepted index's content name, and the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub version: String,
    pub index: Index,
}

/// The accepted files kept in `local`: by inputs-folder path, the local copy and its content name. None
/// before a version was kept.
pub fn files(local: &Path) -> Option<(String, Files)> {
    let k: Kept = serde_json::from_slice(&std::fs::read(local.join(LOCAL)).ok()?).ok()?;
    let files = k.index.files.iter().map(|(p, f)| (p.clone(), (local.join(&f.file), f.file.clone()))).collect();
    Some((k.version, files))
}

/// What a sync found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Synced {
    /// The records name no accepted version of the unit yet.
    None,
    /// The version kept is the records' (whether it was copied now: `true`).
    Kept(bool),
}

/// Keeps unit `unit`'s accepted version in `local`, reading the NAS project folder's files with
/// `read` (a path relative to it: the server's reads go through its pool, so a slow NAS trips its
/// breaker rather than hang). An error leaves the copy as it was.
pub fn sync(read: &dyn Fn(&str) -> Result<Vec<u8>>, unit: &str, local: &Path) -> Result<Synced> {
    sync_with(read, unit, local, &[])
}

/// `sync`, copying besides each accepted file the stored files its facts `facts` name (the regions'
/// drawn files, `facts.drawn`: what the map's region layer draws).
pub fn sync_with(read: &dyn Fn(&str) -> Result<Vec<u8>>, unit: &str, local: &Path, facts: &[&str]) -> Result<Synced> {
    let records: BTreeMap<String, String> =
        serde_json::from_slice(&read(RECORDS)?).with_context(|| format!("parse {RECORDS}"))?;
    let Some(version) = records.get(&logical(unit)) else { return Ok(Synced::None) };
    if files(local).is_some_and(|(v, _)| &v == version) {
        return Ok(Synced::Kept(false));
    }
    let index: Index = serde_json::from_slice(&read(version)?).with_context(|| format!("parse {version}"))?;
    anyhow::ensure!(index.unit == unit, "{version} is {}'s index, not {unit}'s", index.unit);
    // (Each accepted file, and the stored files its facts `facts` name, by content name.)
    let wanted: Vec<(String, Option<u64>)> = index
        .files
        .values()
        .flat_map(|f| {
            std::iter::once((f.file.clone(), Some(f.size))).chain(
                facts.iter().filter_map(|k| f.facts.get(*k).and_then(|v| v.as_str())).map(|n| (n.to_string(), None)),
            )
        })
        .collect();
    for (name, size) in &wanted {
        let p = local.join(name);
        if std::fs::metadata(&p).is_ok_and(|m| size.is_none_or(|s| m.len() == s)) {
            continue;
        }
        let bytes = read(name)?;
        let want = super::hash_of(name).with_context(|| format!("{name} isn't a content name"))?;
        anyhow::ensure!(
            store::naming::hash16(&bytes) == want,
            "{name}: its bytes aren't its name's (copied while being written?)"
        );
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
        }
        store::whole::write(&p, &bytes)?;
    }
    std::fs::create_dir_all(local)?;
    store::whole::write(
        &local.join(LOCAL),
        &serde_json::to_vec(&Kept { version: version.clone(), index: index.clone() })?,
    )?;
    // (The files the version kept no longer names go.)
    let named: BTreeSet<PathBuf> = wanted.iter().map(|(n, _)| local.join(n)).collect();
    let mut stack = vec![local.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            } else if p != local.join(LOCAL) && !named.contains(&p) {
                std::fs::remove_file(&p).ok();
            }
        }
    }
    Ok(Synced::Kept(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accepted_copies_are_kept_and_followed() {
        let d = tempfile::tempdir().unwrap();
        let (nas, local) = (d.path().join("nas"), d.path().join("local"));
        let read = |p: &str| std::fs::read(nas.join(p)).with_context(|| p.to_string());
        let put = |name: &str, bytes: &[u8]| {
            std::fs::create_dir_all(nas.join(name).parent().unwrap()).unwrap();
            std::fs::write(nas.join(name), bytes).unwrap();
        };
        let accept = |files: &[(&str, &[u8])]| {
            let mut index =
                Index { fmt: 1, unit: "translations".into(), checks: "translations 1".into(), ..Default::default() };
            for (p, b) in files {
                let file = crate::inputs::copy_name("translations", p, &store::naming::hash16(b)).unwrap();
                put(&file, b);
                index.files.insert(
                    p.to_string(),
                    crate::inputs::FileEntry {
                        file,
                        size: b.len() as u64,
                        keyed: "-".into(),
                        facts: Default::default(),
                    },
                );
            }
            let ib = serde_json::to_vec(&index).unwrap();
            let name = format!("sources/inputs/translations/@index.{}.json", store::naming::hash16(&ib));
            put(&name, &ib);
            put(RECORDS, &serde_json::to_vec(&BTreeMap::from([(logical("translations"), name.clone())])).unwrap());
            name
        };
        // No version yet: none.
        put(RECORDS, b"{}");
        assert_eq!(sync(&read, "translations", &local).unwrap(), Synced::None);
        assert!(files(&local).is_none());
        // One: copied, then kept.
        let v1 = accept(&[("a.jsonl", b"one\n"), ("x/b.jsonl", b"two\n")]);
        assert_eq!(sync(&read, "translations", &local).unwrap(), Synced::Kept(true));
        assert_eq!(sync(&read, "translations", &local).unwrap(), Synced::Kept(false));
        let (v, f) = files(&local).unwrap();
        assert_eq!(v, v1);
        assert_eq!(f.keys().collect::<Vec<_>>(), ["a.jsonl", "x/b.jsonl"]);
        assert_eq!(std::fs::read(&f["x/b.jsonl"].0).unwrap(), b"two\n");
        // The next: a file edited, one gone; the old copies go.
        accept(&[("a.jsonl", b"one, edited\n")]);
        assert_eq!(sync(&read, "translations", &local).unwrap(), Synced::Kept(true));
        let (_, f2) = files(&local).unwrap();
        assert_eq!(f2.keys().collect::<Vec<_>>(), ["a.jsonl"]);
        assert!(!f["a.jsonl"].0.exists() && !f["x/b.jsonl"].0.exists());
        // The NAS away: an error, the copy as it was.
        std::fs::remove_file(nas.join(RECORDS)).unwrap();
        assert!(sync(&read, "translations", &local).is_err());
        assert_eq!(std::fs::read(&files(&local).unwrap().1["a.jsonl"].0).unwrap(), b"one, edited\n");
        // A copy whose bytes aren't its name's: refused.
        let v3 = accept(&[("c.jsonl", b"three\n")]);
        let index: Index = serde_json::from_slice(&std::fs::read(nas.join(&v3)).unwrap()).unwrap();
        std::fs::write(nas.join(&index.files["c.jsonl"].file), b"three, changed\n").unwrap();
        assert!(sync(&read, "translations", &local).is_err());
        assert_eq!(files(&local).unwrap().1.keys().collect::<Vec<_>>(), ["a.jsonl"]);
    }
}
