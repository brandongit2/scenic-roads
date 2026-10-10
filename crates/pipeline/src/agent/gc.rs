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
//! The gate's checked copies (`sources/inputs/`, docs/inputs.md §4.9) by the same rule, kept while
//! the manifest names them, a held report or an accepted index it names lists them, or a journal
//! entry not yet merged does (`inputs_kept`; none swept when one of those can't be read).
//! The newest pass, a planet waiting for its pass, the rest of `sources/`, the user's folders (the
//! drop boxes), state and the app are never touched. With no readable catalog nothing is deleted.

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
    /// Of the removed, the gate's checked copies; and why none were swept, when they weren't.
    pub inputs_removed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs_skipped: Option<String>,
    pub dry_run: bool,
}

/// Folders never swept, whatever a catalog says (`sources/` but its retired passes and the gate's
/// checked copies, `sources/inputs/`, swept on their own rules).
const NEVER: [&str; 7] = ["translations", "descriptions", "inputs", "state", "app", "nas", "sources"];

/// What GC keeps of `sources/inputs/` (docs/inputs.md §4.9): what the records `manifest` name (the
/// units' accepted indexes and held reports), the files each such index lists, and the same of
/// every journal entry the newest records don't reflect yet (pool.md §7.3: an entry not merged may
/// name a version the records will). An error when any of them can't be read now: nothing there is
/// swept then.
pub fn inputs_kept(root: &Path, manifest: &std::collections::BTreeMap<String, String>) -> Result<BTreeSet<String>> {
    use crate::pool::{journal, records::Records, term};
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let mut named: Vec<(String, String)> = manifest.iter().filter(|(_, c)| c.starts_with("sources/inputs/")).map(|(l, c)| (l.clone(), c.clone())).collect();
    // The journal's entries not merged yet (with the pool on).
    match std::fs::metadata(root.join(journal::DIR)) {
        Ok(_) => {
            let nas = crate::pool::nas::Share::new(root);
            let cur = term::current(&nas).context("the current term")?;
            let r = Records::newest(&nas, cur.term, true).context("the newest records")?.unwrap_or_default();
            for key in journal::list(&nas, None).context("the journal")? {
                if r.reflected.contains(&key) || r.rejected.contains_key(&key) {
                    continue;
                }
                match journal::read(&nas, &key)? {
                    journal::Read::Entry(e) => named.extend(e.handoff.changes.into_iter().filter_map(|(l, c)| c.filter(|c| c.starts_with("sources/inputs/")).map(|c| (l, c)))),
                    journal::Read::Short => anyhow::bail!("journal entry {key} isn't whole yet"),
                    journal::Read::Missing | journal::Read::Damaged(_) => {}
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("the journal"),
    }
    for (l, c) in named {
        if crate::inputs::unit_of(&l).is_some() && l.ends_with("/index") {
            keep.extend(crate::inputs::read_index(root, &c)?.files.into_values().map(|f| f.file));
        }
        keep.insert(c);
    }
    Ok(keep)
}

pub fn run(root: &Path, keep_days: u64, dry_run: bool) -> Result<Report> {
    use crate::timings::{phase, Class};
    let keep = Duration::from_secs(keep_days * 86400);
    let now = SystemTime::now();
    let old = |t: SystemTime| now.duration_since(t).is_ok_and(|d| d > keep);
    let mut rep = Report { dry_run, ..Default::default() };

    // Catalogs: the newest always, and every one of the last `keep_days`.
    let cat_dir = root.join("catalog");
    let read = phase("catalogs read", Class::NasRead);
    let ns = if cat_dir.exists() { store::catalog::list(&cat_dir).context("list catalogs")? } else { Vec::new() };
    // Before the first catalog there's nothing to keep track of, so nothing is removed.
    let Some(&latest) = ns.iter().max() else { return Ok(rep) };
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut drop_cats: Vec<PathBuf> = Vec::new();
    // (How far it is, for the status: the catalogs read, then the folders swept.)
    crate::agent::jobs::stage(0, 2, "steps (reading the catalogs)");
    for (k, &n) in ns.iter().enumerate() {
        crate::agent::jobs::within(k as f64 / ns.len() as f64);
        let p = cat_dir.join(store::catalog::file_name(n));
        let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).with_context(|| format!("stat {}", p.display()))?;
        if n != latest && old(mtime) {
            drop_cats.push(p);
            continue;
        }
        // A catalog in the window that can't be read stops the sweep: its files might be in use.
        let cat = store::catalog::read(&p).with_context(|| format!("read {}", p.display()))?;
        read.count(std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0), 1);
        referenced.extend(cat.files.values().map(|f| f.file.clone()));
        rep.catalogs_kept += 1;
    }
    // The build's manifest: everything a build has uploaded and may publish next (work in flight,
    // and outputs reused by name). Unreadable, nothing is removed.
    // (Not `exists()`: it's false on an I/O error too, and then the manifest's files would go.)
    drop(read);
    let m: std::collections::BTreeMap<String, String> = {
        let _p = phase("build manifest read", Class::NasRead);
        crate::out::read_record(&root.join("state/build/manifest.json")).context("the build manifest")?
    };
    // (What the gate's copies are kept by, read before the manifest is given up.)
    let inputs_keep = match inputs_kept(root, &m) {
        Ok(k) => Some(k),
        Err(e) => {
            rep.inputs_skipped = Some(format!("{e:#}"));
            None
        }
    };
    referenced.extend(m.into_values());
    rep.referenced = referenced.len();
    let tops: BTreeSet<String> = referenced.iter().filter_map(|f| f.split('/').next()).filter(|t| !NEVER.contains(t)).map(str::to_string).collect();

    crate::agent::jobs::stage(1, 2, "steps (sweeping the folders)");
    // (Listing the folders and removing what's due, file by file: one phase, the removals counted.)
    let sweep = phase("folders swept", Class::NasWrite);
    for (k, top) in tops.iter().enumerate() {
        crate::agent::jobs::within(k as f64 / tops.len() as f64);
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
                sweep.count(md.len(), 1);
            }
        }
    }
    drop(sweep);
    // The gate's checked copies: what's kept named, the rest once old.
    if let Some(keep) = inputs_keep {
        let _p = phase("the inputs' copies swept", Class::NasWrite);
        let mut stack = vec![root.join("sources/inputs")];
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
                let mtime = md.modified().unwrap_or(now);
                if e.file_name().to_string_lossy().ends_with(".tmp") {
                    if now.duration_since(mtime).is_ok_and(|d| d > Duration::from_secs(2 * 86400)) {
                        remove(&p, dry_run)?;
                        rep.tmp_removed += 1;
                    }
                    continue;
                }
                if store::naming::parse_content_name(&rel).is_none() || keep.contains(&rel) || referenced.contains(&rel) {
                    continue;
                }
                if !old(mtime) {
                    rep.young += 1;
                    continue;
                }
                remove(&p, dry_run)?;
                crate::timings::count(md.len(), 1);
                rep.removed += 1;
                rep.removed_bytes += md.len();
                rep.inputs_removed += 1;
            }
        }
    }
    // Retired passes' sources: passes older than the newest complete one.
    if let Some(latest) = crate::osmpass::latest_pass(root) {
        let _p = phase("retired passes swept", Class::NasWrite);
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
    let _p = phase("old catalogs removed", Class::NasWrite);
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
                crate::timings::count(md.len(), 1);
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

    /// The gate's copies (docs/inputs.md §4.9): kept while the manifest's index or report names
    /// them, or a journal entry not merged yet does; the rest of `sources/` and the drop boxes
    /// never swept; none swept while an index can't be read.
    #[test]
    fn the_inputs_copies_go_once_nothing_names_them() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, body: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, body).unwrap();
            age(&p, days);
        };
        let u = "sources/inputs/_gate-test";
        let kept = format!("{u}/a.1111111111111111.jsonl");
        let index = |files: &[&str]| {
            let files: serde_json::Map<String, serde_json::Value> = files.iter().enumerate().map(|(i, f)| (format!("f{i}.jsonl"), serde_json::json!({"file": f, "size": 1, "keyed": "-"}))).collect();
            serde_json::json!({"fmt": 1, "unit": "_gate-test", "checks": "_gate-test 1", "files": files}).to_string()
        };
        put(&kept, "x", 60);
        put(&format!("{u}/a.2222222222222222.jsonl"), "x", 60); // an old version: goes
        put(&format!("{u}/a.3333333333333333.jsonl"), "x", 3); // young: stays
        put(&format!("{u}/b.4444444444444444.jsonl"), "x", 60); // an unmerged entry's index lists it
        put(&format!("{u}/index.5555555555555555.json"), &index(&[&kept]), 60);
        put(&format!("{u}/index.6666666666666666.json"), &index(&[&format!("{u}/b.4444444444444444.jsonl")]), 60);
        put(&format!("{u}/held.7777777777777777.json"), "{}", 60);
        put(&format!("{u}/held.8888888888888888.json"), "{}", 60); // an old report: goes
        put("inputs/_gate-test/a.jsonl", "x", 300); // the drop box: never
        put("sources/registers/legacy.9999999999999999.tar.zst", "x", 300); // the rest of sources/: never
        store::catalog::write(&root.join("catalog"), &store::catalog::Catalog::new(1)).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), serde_json::json!({format!("{u}/index"): format!("{u}/index.5555555555555555.json"), format!("{u}/held"): format!("{u}/held.7777777777777777.json")}).to_string()).unwrap();
        // A journal entry the records don't reflect, naming the newer index.
        let entry = crate::pool::journal::Entry { member: "m-0000000000000001".into(), lease: crate::pool::journal::LeaseId { term: 1, n: 7 }, step: "inputs".into(), handoff: crate::handoff::Handoff { changes: [(format!("{u}/index"), Some(format!("{u}/index.6666666666666666.json")))].into(), ..Default::default() }, at: 1_791_500_000 };
        crate::pool::journal::write(&crate::pool::nas::Share::new(root), &entry).unwrap();
        let r = run(root, 14, false).unwrap();
        assert_eq!(r.inputs_skipped, None);
        assert_eq!(r.inputs_removed, 2, "{r:?}");
        assert!(!root.join(format!("{u}/a.2222222222222222.jsonl")).exists() && !root.join(format!("{u}/held.8888888888888888.json")).exists());
        for k in [kept.clone(), format!("{u}/a.3333333333333333.jsonl"), format!("{u}/b.4444444444444444.jsonl"), format!("{u}/index.5555555555555555.json"), format!("{u}/index.6666666666666666.json"), format!("{u}/held.7777777777777777.json"), "inputs/_gate-test/a.jsonl".into(), "sources/registers/legacy.9999999999999999.tar.zst".into()] {
            assert!(root.join(&k).exists(), "{k}");
        }
        // An index named that can't be read: nothing there swept.
        put(&format!("{u}/a.aaaaaaaaaaaaaaaa.jsonl"), "x", 60);
        fs::write(root.join(format!("{u}/index.5555555555555555.json")), b"{not json").unwrap();
        let r = run(root, 14, false).unwrap();
        assert!(r.inputs_skipped.is_some() && r.inputs_removed == 0);
        assert!(root.join(format!("{u}/a.aaaaaaaaaaaaaaaa.jsonl")).exists());
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
