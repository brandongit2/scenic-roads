//! The translations' and the descriptions' accepted versions, kept on this Mac (docs/inputs.md
//! §4.6, §6.3, §6.4): the files the lead's records name for the unit, copied from the NAS through
//! the pool (pipeline::inputs::served), so they work offline and load fast. The server never reads
//! a drop box.
//!
//! While the move under `inputs/` is under way (docs/inputs.md §4.11: two publishes, so no Mac's
//! server sees an empty folder), the old folder at the NAS's root is copied as before
//! (crate::livefolder) and read until there's an accepted version and the old folder holds no
//! `.jsonl` file any more: then the old copy goes, and from then on only the accepted version is
//! read (`choose`). The next publish deletes the old folder's copy.

use crate::data::Data;
use pipeline::inputs::served::{self, Synced};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where unit `unit`'s accepted version is kept, under the server's home.
pub fn local_dir(home: &Path, unit: &str) -> PathBuf {
    home.join("accepted").join(unit)
}

/// Keeps unit `unit`'s accepted version in `local` in step with the records.
/// How it ended: None while the NAS is away; `Synced::Kept` once the copy here is the version the
/// records name now (whether it was copied now: `true`); `Synced::None` when they name none.
pub fn synced(data: &Data, unit: &str, local: &Path) -> anyhow::Result<Option<Synced>> {
    let (Some(root), Some(pool)) = (data.nas_root(), data.pool()) else { return Ok(None) };
    if !pool.is_online() {
        return Ok(None);
    }
    let read = |p: &str| -> anyhow::Result<Vec<u8>> { Ok(crate::views::RemoteFile::new(root.join(p), pool.clone()).read_all()?) };
    served::sync(&read, unit, local).map(Some)
}

/// The accepted files kept for unit `unit` in `local` (by drop-box path: the copy and its content
/// name); None before any.
pub fn files(local: &Path) -> Option<BTreeMap<String, (PathBuf, String)>> {
    served::files(local).map(|(_, f)| f)
}

/// What a unit is read from (the move's first publish: `choose`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    /// The old folder's copy (`old_local`), as before the move.
    Old,
    /// The accepted version.
    Accepted,
}

/// Which to read unit `unit` from, the copies brought up to date first (`old_rel`: its old folder
/// on the NAS, copied into `old_local`), and whether what it says to read changed. The old copy is
/// read until the copy here is the accepted version the records name now and the old folder on the
/// NAS holds no `.jsonl` file: then it's removed, and from then on (offline too) the accepted
/// version is read.
pub fn choose(data: &Data, unit: &str, acc_local: &Path, old_rel: &str, old_local: &Path) -> (From, bool) {
    let (changed, current) = match synced(data, unit, acc_local) {
        Ok(Some(Synced::Kept(c))) => (c, true),
        Ok(_) => (false, false),
        Err(e) => {
            eprintln!("{unit}: the accepted version: {e:#}");
            (false, false)
        }
    };
    if files(acc_local).is_some() && !old_local.exists() {
        return (From::Accepted, changed);
    }
    // (Switched to only with the copy here the records' version now, just synced: one behind, or
    // unfinished, keeps the old copy read.)
    let have = current && files(acc_local).is_some();
    // (The old copy never loses a file during the move: one emptied while this Mac's accepted copy
    // is unfinished leaves it with the old lines, never none; the copy goes whole at the switch.)
    match crate::livefolder::sync(data, old_rel, old_local, false) {
        Ok(Some((_, 0))) if have => {
            eprintln!("{unit}: the old folder is empty and there's an accepted version: read from it from now on");
            std::fs::remove_dir_all(old_local).ok();
            (From::Accepted, true)
        }
        Ok(Some((c, _))) => (From::Old, changed | c),
        Ok(None) => (From::Old, changed),
        Err(e) => {
            eprintln!("{unit}: the old folder: {e:#}");
            (From::Old, changed)
        }
    }
}

/// Puts an accepted version of unit `unit` (`files`: drop-box path, content) on a NAS made for a
/// test, its index named in the records (for the server's tests).
#[cfg(test)]
pub fn put(nas: &Path, unit: &str, files: &[(&str, &str)]) -> String {
    let put = |name: &str, bytes: &[u8]| {
        std::fs::create_dir_all(nas.join(name).parent().unwrap()).unwrap();
        std::fs::write(nas.join(name), bytes).unwrap();
    };
    let mut index = pipeline::inputs::Index { fmt: 1, unit: unit.into(), checks: format!("{unit} 1"), ..Default::default() };
    for (p, b) in files {
        let file = pipeline::inputs::copy_name(unit, p, &store::naming::hash16(b.as_bytes())).unwrap();
        put(&file, b.as_bytes());
        index.files.insert(p.to_string(), pipeline::inputs::FileEntry { file, size: b.len() as u64, keyed: "-".into(), facts: Default::default() });
    }
    let ib = serde_json::to_vec(&index).unwrap();
    let name = format!("{}/@index.{}.json", pipeline::inputs::store_dir(unit), store::naming::hash16(&ib));
    put(&name, &ib);
    let records = nas.join(served::RECORDS);
    let mut m: BTreeMap<String, String> = std::fs::read(&records).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    m.insert(pipeline::inputs::logical(unit), name.clone());
    put(served::RECORDS, &serde_json::to_vec(&m).unwrap());
    name
}
