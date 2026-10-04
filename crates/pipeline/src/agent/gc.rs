//! Removing replaced files from the NAS (docs/plan.md §3, GC): every content-named file that
//! neither the newest catalog, nor a catalog of the last `keep_days`, nor the build's manifest
//! references, and that is itself older than `keep_days`; abandoned `.tmp` files; old catalogs.
//!
//! Deletions go through SMB: on this share they're permanent (no Recycle Bin entry; tested
//! 2026-10-02), and the build Mac can't use SSH unattended (1Password asks each session).
//!
//! Only the folders catalogs index are swept (`base/`, `global/`, `layers/`, `hidata/`, …: the
//! first path component of every referenced file), and the sources of passes older than the newest
//! complete one (`sources/osm/<date>/`, `sources/items/<date>/`): their content-named files by the
//! same rule, the rest (the planet download) once the newer pass has been complete for `keep_days`.
//! The newest pass, a planet waiting for its pass, the rest of `sources/`, the user's folders, state
//! and the app are never touched. With no readable catalog nothing is deleted.

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
    /// Of the removed, files of retired passes' sources (and their bytes).
    pub retired_removed: usize,
    pub retired_bytes: u64,
    pub dry_run: bool,
}

/// Folders never swept, whatever a catalog says.
const NEVER: [&str; 7] = ["translations", "descriptions", "inputs", "state", "app", "nas", "sources"];

pub fn run(root: &Path, keep_days: u64, dry_run: bool) -> Result<Report> {
    let keep = Duration::from_secs(keep_days * 86400);
    let now = SystemTime::now();
    let old = |t: SystemTime| now.duration_since(t).is_ok_and(|d| d > keep);
    let mut rep = Report { dry_run, ..Default::default() };

    // Catalogs: the newest always, and every one of the last `keep_days`.
    let cat_dir = root.join("catalog");
    let ns = if cat_dir.exists() { store::catalog::list(&cat_dir).context("list catalogs")? } else { Vec::new() };
    // Before the first catalog there's nothing to keep track of, so nothing is removed.
    let Some(&latest) = ns.iter().max() else { return Ok(rep) };
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
    // The build's manifest: everything a build has uploaded and may publish next (work in flight,
    // and outputs reused by name). Unreadable, nothing is removed.
    // (Not `exists()`: it's false on an I/O error too, and then the manifest's files would go.)
    let m: std::collections::BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).context("the build manifest")?;
    referenced.extend(m.into_values());
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
    // Retired passes' sources: passes older than the newest complete one.
    if let Some(latest) = crate::osmpass::latest_pass(root) {
        // When the newest pass completed (its summary's time): a retired pass's plain files (the
        // planet download) go once that's `keep_days` ago, so a fresh pass can still be compared.
        let done_at = std::fs::read_dir(root.join("sources/osm").join(&latest))
            .ok()
            .and_then(|rd| rd.flatten().find(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("pass.") && n.ends_with(".json"))))
            .and_then(|e| e.metadata().ok()?.modified().ok());
        let plain_due = done_at.is_some_and(old);
        for kind in ["osm", "items"] {
            let Ok(rd) = std::fs::read_dir(root.join("sources").join(kind)) else { continue };
            let mut dates: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|d| crate::osmpass::is_date(d) && *d < latest).collect();
            dates.sort();
            for d in dates {
                let dir = root.join("sources").join(kind).join(&d);
                sweep_retired(root, &dir, &referenced, &old, plain_due, dry_run, &mut rep)?;
            }
        }
    }
    for p in drop_cats {
        remove(&p, dry_run)?;
        rep.catalogs_removed += 1;
    }
    Ok(rep)
}

/// One retired pass's folder: content-named files no catalog or manifest names, once old; any other
/// file once `plain_due`; then the folders left empty.
fn sweep_retired(root: &Path, dir: &Path, referenced: &BTreeSet<String>, old: &dyn Fn(SystemTime) -> bool, plain_due: bool, dry_run: bool, rep: &mut Report) -> Result<()> {
    let mut dirs = vec![dir.to_path_buf()];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(p.clone());
                dirs.push(p);
                continue;
            }
            rep.files_seen += 1;
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
            let named = store::naming::parse_content_name(&rel).is_some();
            let due = if named { !referenced.contains(&rel) && old(md.modified().unwrap_or(SystemTime::now())) } else { plain_due };
            if due {
                remove(&p, dry_run)?;
                rep.removed += 1;
                rep.removed_bytes += md.len();
                rep.retired_removed += 1;
                rep.retired_bytes += md.len();
            } else if named && !referenced.contains(&rel) {
                rep.young += 1;
            }
        }
    }
    // Deepest first: a folder emptied above may let its parent go too.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        if std::fs::read_dir(&d).is_ok_and(|mut rd| rd.next().is_none()) && !dry_run {
            std::fs::remove_dir(&d).ok();
        }
    }
    Ok(())
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
    fn keeps_the_newest_catalog_however_old() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let mk = |n: u64, file: &str| {
            let p = root.join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, 60);
            let mut cat = store::catalog::Catalog::new(n);
            let logical = file.rsplit_once('.').unwrap().0.rsplit_once('.').unwrap().0;
            cat.files.insert(logical.into(), store::catalog::FileRef { file: file.into(), ..Default::default() });
            let c = store::catalog::write(&root.join("catalog"), &cat).unwrap();
            age(&c, 60);
        };
        mk(1, "layers/a/hi/6-1-1.1111111111111111.pack");
        mk(2, "layers/a/hi/6-1-1.2222222222222222.pack");
        mk(3, "layers/a/hi/6-1-1.3333333333333333.pack");
        let r = run(root, 14, false).unwrap();
        assert_eq!((r.catalogs_kept, r.catalogs_removed, r.removed), (1, 2, 2));
        assert!(root.join("layers/a/hi/6-1-1.3333333333333333.pack").exists());
        assert_eq!(store::catalog::latest(&root.join("catalog")).unwrap().unwrap().n, 3);
        assert_eq!(store::catalog::next_n(&root.join("catalog")).unwrap(), 4);
    }

    #[test]
    fn the_build_manifest_keeps_its_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let f = "base/6-1-1.4444444444444444.sect";
        fs::create_dir_all(root.join("base")).unwrap();
        fs::write(root.join(f), b"x").unwrap();
        age(&root.join(f), 60);
        let cat = store::catalog::Catalog::new(1);
        store::catalog::write(&root.join("catalog"), &cat).unwrap();
        // Not in the catalog, old, but the build uploaded it: kept. (A catalog file elsewhere so
        // `base/` is swept at all.)
        let mut cat2 = store::catalog::Catalog::new(2);
        cat2.files.insert("base/6-1-2".into(), store::catalog::FileRef { file: "base/6-1-2.5555555555555555.sect".into(), ..Default::default() });
        store::catalog::write(&root.join("catalog"), &cat2).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), format!("{{\"base/6-1-1\": \"{f}\"}}")).unwrap();
        let r = run(root, 14, false).unwrap();
        assert_eq!(r.removed, 0);
        assert!(root.join(f).exists());
    }

    #[test]
    fn retired_passes_go_after_the_newer_one_settles() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, days);
        };
        // The old pass (retired from the manifest), the newest (complete 20 days ago), and a planet
        // waiting for its pass.
        put("sources/osm/2026-03-01/planet.osm.pbf", 200);
        put("sources/osm/2026-03-01/pieces/6-1-1.1111111111111111.osm.pbf", 200);
        put("sources/items/2026-03-01/facts.2222222222222222.json", 200);
        put("sources/osm/2026-09-28/planet.osm.pbf", 30);
        put("sources/osm/2026-09-28/pass.3333333333333333.json", 20);
        put("sources/osm/2026-09-28/pieces/6-1-1.4444444444444444.osm.pbf", 25);
        put("sources/osm/2027-03-01/planet.osm.pbf", 1);
        put("sources/registers/legacy.5555555555555555.tar.zst", 300);
        store::catalog::write(&root.join("catalog"), &store::catalog::Catalog::new(1)).unwrap();
        let r = run(root, 14, false).unwrap();
        assert_eq!(r.retired_removed, 3, "{r:?}");
        assert!(!root.join("sources/osm/2026-03-01").exists() && !root.join("sources/items/2026-03-01").exists(), "the retired pass's folders, emptied, go too");
        for kept in ["sources/osm/2026-09-28/planet.osm.pbf", "sources/osm/2026-09-28/pieces/6-1-1.4444444444444444.osm.pbf", "sources/osm/2027-03-01/planet.osm.pbf", "sources/registers/legacy.5555555555555555.tar.zst"] {
            assert!(root.join(kept).exists(), "{kept}");
        }
        // A newer pass completed only days ago: the old planet waits (its content-named files don't).
        put("sources/osm/2026-03-01/planet.osm.pbf", 200);
        put("sources/osm/2026-03-01/roads/6-1-1.6666666666666666.bin", 200);
        put("sources/osm/2026-09-28/pass.3333333333333333.json", 3);
        let r = run(root, 14, false).unwrap();
        assert_eq!(r.retired_removed, 1);
        assert!(root.join("sources/osm/2026-03-01/planet.osm.pbf").exists());
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
