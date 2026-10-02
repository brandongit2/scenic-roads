//! Removing replaced files from the NAS (docs/plan.md §3, GC): every content-named file that no
//! catalog of the last `keep_days` references and that is itself older than `keep_days` (younger
//! files may belong to work in flight), plus abandoned `.tmp` files and old catalogs.
//!
//! Deletions go through SMB: on this share they're permanent (no Recycle Bin entry; tested
//! 2026-10-02), and the build Mac can't use SSH unattended (1Password asks each session).
//!
//! Only the folders catalogs index are swept (`base/`, `global/`, `layers/`, `hidata/`, …: the
//! first path component of every referenced file). Sources, the user's folders, state and the app
//! are never touched. With no readable catalog nothing is deleted.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub catalogs_kept: usize,
    pub catalogs_removed: usize,
    pub referenced: usize,
    pub files_seen: usize,
    pub removed: usize,
    pub removed_bytes: u64,
    pub tmp_removed: usize,
    /// Unreferenced but too young to remove.
    pub young: usize,
    pub dry_run: bool,
}

/// Folders never swept, whatever a catalog says.
const NEVER: [&str; 7] = ["translations", "descriptions", "inputs", "state", "app", "nas", "sources"];

pub fn run(root: &Path, keep_days: u64, dry_run: bool) -> Result<Report> {
    let keep = Duration::from_secs(keep_days * 86400);
    let now = SystemTime::now();
    let old = |t: SystemTime| now.duration_since(t).is_ok_and(|d| d > keep);
    let mut rep = Report { dry_run, ..Default::default() };

    // Catalogs: the latest always, and every one of the last `keep_days`.
    let cat_dir = root.join("catalog");
    let ns = if cat_dir.exists() { store::catalog::list(&cat_dir).context("list catalogs")? } else { Vec::new() };
    // Before the first catalog there's nothing to keep track of, so nothing is removed.
    let Some(&latest) = ns.last() else { return Ok(rep) };
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut drop_cats: Vec<PathBuf> = Vec::new();
    for &n in &ns {
        let p = cat_dir.join(store::catalog::file_name(n));
        let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).with_context(|| format!("stat {}", p.display()))?;
        if n != latest && old(mtime) {
            drop_cats.push(p);
            continue;
        }
        // A catalog in the window that can't be read stops the sweep: its files might be in use.
        let cat = store::catalog::read(&p).with_context(|| format!("read {}", p.display()))?;
        referenced.extend(cat.files.values().map(|f| f.file.clone()));
        rep.catalogs_kept += 1;
    }
    rep.referenced = referenced.len();
    let tops: BTreeSet<String> = referenced.iter().filter_map(|f| f.split('/').next()).filter(|t| !NEVER.contains(t)).map(str::to_string).collect();

    for top in &tops {
        let mut stack = vec![root.join(top)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    stack.push(p);
                    continue;
                }
                rep.files_seen += 1;
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
                let name = e.file_name().to_string_lossy().into_owned();
                let mtime = md.modified().unwrap_or(now);
                if name.ends_with(".tmp") {
                    // A write that never finished (two days: no write takes that long).
                    if now.duration_since(mtime).is_ok_and(|d| d > Duration::from_secs(2 * 86400)) {
                        remove(&p, dry_run)?;
                        rep.tmp_removed += 1;
                    }
                    continue;
                }
                if store::naming::parse_content_name(&rel).is_none() || referenced.contains(&rel) {
                    continue;
                }
                if !old(mtime) {
                    rep.young += 1;
                    continue;
                }
                remove(&p, dry_run)?;
                rep.removed += 1;
                rep.removed_bytes += md.len();
            }
        }
    }
    for p in drop_cats {
        remove(&p, dry_run)?;
        rep.catalogs_removed += 1;
    }
    Ok(rep)
}

fn remove(p: &Path, dry_run: bool) -> Result<()> {
    if dry_run {
        eprintln!("gc: would remove {}", p.display());
        return Ok(());
    }
    std::fs::remove_file(p).with_context(|| format!("remove {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, FileTimes};

    fn age(p: &Path, days: u64) {
        let t = SystemTime::now() - Duration::from_secs(days * 86400);
        fs::File::options().write(true).open(p).unwrap().set_times(FileTimes::new().set_modified(t)).unwrap();
    }

    #[test]
    fn keeps_referenced_and_young_removes_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, days);
        };
        let kept = "layers/roads/hi/6-1-2.0123456789abcdef.pack";
        put(kept, 30);
        put("layers/roads/hi/6-1-2.fedcba9876543210.pack", 30); // replaced, old: removed
        put("layers/roads/hi/6-1-3.1111111111111111.pack", 1); // unreferenced but young: kept
        put("layers/roads/hi/6-1-4.2222222222222222.pack.tmp", 5); // abandoned write: removed
        put("sources/osm/x.3333333333333333.osm.pbf", 300); // never swept
        put("layers/roads/README", 300); // not content-named: kept
        let mut cat = store::catalog::Catalog::new(1);
        cat.files.insert("layers/roads/hi/6-1-2".into(), store::catalog::FileRef { file: kept.into(), ..Default::default() });
        store::catalog::write(&root.join("catalog"), &cat).unwrap();
        let r = run(root, 14, false).unwrap();
        assert_eq!((r.removed, r.tmp_removed, r.young), (1, 1, 1));
        assert!(root.join(kept).exists());
        assert!(!root.join("layers/roads/hi/6-1-2.fedcba9876543210.pack").exists());
        assert!(root.join("layers/roads/hi/6-1-3.1111111111111111.pack").exists());
        assert!(root.join("sources/osm/x.3333333333333333.osm.pbf").exists());
        assert!(root.join("layers/roads/README").exists());
    }

    #[test]
    fn nothing_without_a_catalog() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("layers/roads/hi/6-1-2.0123456789abcdef.pack");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        age(&p, 300);
        let r = run(d.path(), 14, false).unwrap();
        assert_eq!((r.removed, r.files_seen), (0, 0));
        assert!(p.exists());
    }
}
