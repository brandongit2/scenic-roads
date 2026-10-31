//! The translations' and the descriptions' accepted versions, kept on this Mac (docs/inputs.md
//! §4.6, §6.3, §6.4): the files the lead's records name for the unit, copied from the NAS through
//! the pool (pipeline::inputs::served), so they work offline and load fast. The server never reads
//! an inputs folder.

use crate::data::Data;
use pipeline::inputs::served::{self, Synced};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where unit `unit`'s accepted version is kept, under the server's home.
pub fn local_dir(home: &Path, unit: &str) -> PathBuf {
    home.join("accepted").join(unit)
}

/// Keeps unit `unit`'s accepted version in `local` in step with the records: whether it changed.
/// Nothing (false) while the NAS is away. Once the copy here is the version the records name, the
/// copy an earlier app kept of the unit's old folder (`old`) is removed: read no more, and kept
/// until then (offline too).
pub fn sync(data: &Data, unit: &str, local: &Path, old: &Path) -> anyhow::Result<bool> {
    let Some(s) = synced(data, unit, local)? else { return Ok(false) };
    if matches!(s, Synced::Kept(_)) && old.exists() {
        eprintln!("{unit}: the copy here is the records' version: an earlier app's copy of the old folder removed");
        std::fs::remove_dir_all(old).ok();
    }
    Ok(s == Synced::Kept(true))
}

/// Keeps unit `unit`'s accepted version in `local` in step with the records, saying how it ended:
/// None while the NAS is away; `Synced::Kept` once the copy here is the version the records name
/// now (whether it was copied now: `true`); `Synced::None` when they name none.
pub fn synced(data: &Data, unit: &str, local: &Path) -> anyhow::Result<Option<Synced>> {
    let (Some(root), Some(pool)) = (data.nas_root(), data.pool()) else { return Ok(None) };
    if !pool.is_online() {
        return Ok(None);
    }
    let read =
        |p: &str| -> anyhow::Result<Vec<u8>> { Ok(store::PooledFile::new(root.join(p), pool.clone()).read_all()?) };
    served::sync(&read, unit, local).map(Some)
}

/// The accepted files kept for unit `unit` in `local` (by inputs-folder path: the copy and its content
/// name); None before any.
pub fn files(local: &Path) -> Option<BTreeMap<String, (PathBuf, String)>> {
    served::files(local).map(|(_, f)| f)
}

/// Puts an accepted version of unit `unit` (`files`: inputs-folder path, content) on a NAS made for a
/// test, its index named in the records (for the server's tests).
#[cfg(test)]
pub fn put(nas: &store::nasfs::NasRoot, unit: &str, files: &[(&str, &str)]) -> String {
    let put = |name: &str, bytes: &[u8]| {
        nas.join(name).parent().unwrap().create_dir_all().unwrap();
        nas.join(name).write(bytes).unwrap();
    };
    let mut index =
        pipeline::inputs::Index { fmt: 1, unit: unit.into(), checks: format!("{unit} 1"), ..Default::default() };
    for (p, b) in files {
        let file = pipeline::inputs::copy_name(unit, p, &store::naming::hash16(b.as_bytes())).unwrap();
        put(&file, b.as_bytes());
        index.files.insert(
            p.to_string(),
            pipeline::inputs::FileEntry { file, size: b.len() as u64, keyed: "-".into(), facts: Default::default() },
        );
    }
    let ib = serde_json::to_vec(&index).unwrap();
    let name = format!("{}/@index.{}.json", pipeline::inputs::store_dir(unit), store::naming::hash16(&ib));
    put(&name, &ib);
    let records = nas.join(served::RECORDS);
    let mut m: BTreeMap<String, String> =
        records.read().ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    m.insert(pipeline::inputs::logical(unit), name.clone());
    put(served::RECORDS, &serde_json::to_vec(&m).unwrap());
    name
}
