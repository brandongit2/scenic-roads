//! A folder of the user's on the NAS (`translations/`, `descriptions/`), copied to this Mac so it
//! works offline and loads fast: new and changed `.jsonl` files once they've held still for 10 s
//! (not being written), with their modification times; local files gone from the NAS removed.
//! Generated `todo/` folders and the NAS's own (`@eaDir`, `#recycle`) are skipped.

use crate::data::Data;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// One pass over `rel` (under the NAS project folder) into `local`; true when anything changed.
pub fn sync(data: &Data, rel_dir: &str, local_dir: &Path) -> anyhow::Result<bool> {
    let (Some(root), Some(pool)) = (data.nas_root(), data.pool()) else { return Ok(false) };
    if !pool.is_online() {
        return Ok(false);
    }
    let src = root.join(rel_dir);
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut changed = false;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let items = match pool.list(&src.join(&rel)) {
            Ok(i) => i,
            // No such folder on the NAS yet: nothing to copy, and nothing local to keep.
            Err(store::iopool::IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound && rel.as_os_str().is_empty() => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        for it in items {
            if it.name.starts_with('.') || it.name.starts_with('@') || it.name.starts_with('#') {
                continue;
            }
            let r = rel.join(&it.name);
            if it.is_dir {
                if it.name != "todo" {
                    stack.push(r);
                }
                continue;
            }
            if !it.name.ends_with(".jsonl") {
                continue;
            }
            seen.insert(r.clone());
            // Stable for 10 s: not being written.
            let stable = it.modified.and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|d| d >= Duration::from_secs(10));
            if !stable {
                continue;
            }
            let local = local_dir.join(&r);
            let same = std::fs::metadata(&local).ok().is_some_and(|m| m.len() == it.len && m.modified().ok() == it.modified);
            if same {
                continue;
            }
            // In pieces through the pool: one slow whole-file read would trip the breaker.
            let bytes = crate::views::RemoteFile::new(src.join(&r), pool.clone()).read_all()?;
            if let Some(p) = local.parent() {
                std::fs::create_dir_all(p)?;
            }
            let tmp = local.with_extension("jsonl.tmp");
            std::fs::write(&tmp, &bytes)?;
            if let Some(m) = it.modified {
                let f = std::fs::File::options().write(true).open(&tmp)?;
                f.set_modified(m)?;
            }
            std::fs::rename(&tmp, &local)?;
            changed = true;
        }
    }
    // Local files the NAS no longer has.
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(local_dir.join(&rel)) else { continue };
        for e in rd.flatten() {
            let r = rel.join(e.file_name());
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(r);
            } else if r.extension().is_some_and(|x| x == "jsonl") && !seen.contains(&r) {
                std::fs::remove_file(local_dir.join(&r)).ok();
                changed = true;
            }
        }
    }
    Ok(changed)
}
