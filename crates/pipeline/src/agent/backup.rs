//! Dated copies of the user's folders (docs/plan.md §3, Backups): `translations/`,
//! `descriptions/` and `inputs/`, kept 30 days under `state/backups/` on the NAS and mirrored to
//! the build Mac.
//!
//! Content-addressed, so an unchanged file is stored once however many days keep it:
//! `blobs/<hash16>` holds each version of a file, and `<YYYY-MM-DD>.json` lists the folder as it
//! was that day (path, size, modification time, blob). A day is written only when something
//! changed. Generated `todo/` folders are skipped.
//!
//! To restore a file: find it in a day's list and copy `blobs/<hash>` back to its path.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const FOLDERS: [&str; 3] = ["translations", "descriptions", "inputs"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub mtime: u64,
    pub blob: String,
}

/// A day's list: path (relative to the NAS root) → entry.
pub type Manifest = BTreeMap<String, Entry>;

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub files: usize,
    pub new_blobs: usize,
    pub new_bytes: u64,
    /// The day written, when anything changed.
    pub written: Option<String>,
    pub days_removed: usize,
    pub blobs_removed: usize,
}

fn mtime_s(md: &std::fs::Metadata) -> u64 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn skip(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('@') || name.starts_with('#') || name == "todo" || name.ends_with(".tmp")
}

/// The user's folders as they are now; files whose size and time match `prev` keep its blob
/// without being read again.
fn scan(root: &Path, prev: &Manifest) -> Result<Vec<(String, PathBuf, u64, u64, Option<String>)>> {
    let mut out = Vec::new();
    for top in FOLDERS {
        let mut stack = vec![root.join(top)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if skip(&name) {
                    continue;
                }
                let p = e.path();
                let md = e.metadata().with_context(|| format!("stat {}", p.display()))?;
                if md.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
                let (size, mtime) = (md.len(), mtime_s(&md));
                let known = prev.get(&rel).filter(|e| e.size == size && e.mtime == mtime).map(|e| e.blob.clone());
                out.push((rel, p, size, mtime, known));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The newest day's list in `dir`.
fn latest(dir: &Path) -> Option<(String, Manifest)> {
    let day = days(dir).pop()?;
    let b = std::fs::read(dir.join(format!("{day}.json"))).ok()?;
    Some((day.clone(), serde_json::from_slice(&b).ok()?))
}

/// The days kept in `dir`, oldest first.
fn days(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-').map(str::to_string)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Copies `src` to `dst` through a `.tmp` and a rename.
fn copy_atomic(src: &Path, dst: &Path) -> Result<u64> {
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = dst.with_extension("tmp");
    let n = store::sys::copy_data(src, &tmp).with_context(|| format!("copy {} to {}", src.display(), tmp.display()))?;
    std::fs::rename(&tmp, dst)?;
    crate::timings::count(n, 1);
    Ok(n)
}

fn write_atomic(dst: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = dst.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

/// Backs up the user's folders under `root` into `root/state/backups` (and mirrors that to
/// `local`), dated `today`, keeping `keep_days`.
pub fn run(root: &Path, local: Option<&Path>, today: &str, keep_days: u64) -> Result<Report> {
    let dir = root.join("state/backups");
    let blobs = dir.join("blobs");
    use crate::timings::{phase, Class};
    std::fs::create_dir_all(&blobs)?;
    let prev = {
        let _p = phase("last backup read", Class::NasRead);
        latest(&dir).map(|(_, m)| m).unwrap_or_default()
    };
    let mut rep = Report::default();
    let mut now: Manifest = BTreeMap::new();
    let files = {
        let _p = phase("folders scanned", Class::NasRead);
        scan(root, &prev)?
    };
    // (Each changed file copied into the blobs and hashed there: the NAS read and written.)
    let copying = phase("changed files copied", Class::NasWrite);
    let (n, mut said) = (files.len() as u64, std::time::Instant::now());
    for (k, (rel, p, size, mtime, known)) in files.into_iter().enumerate() {
        // (How far it is, for the status, at most once a second.)
        if k == 0 || said.elapsed() >= std::time::Duration::from_secs(1) {
            said = std::time::Instant::now();
            crate::agent::jobs::report(k as u64, n, "files backed up");
        }
        let blob = match known {
            Some(b) if blobs.join(&b).exists() => b,
            _ => {
                // Copy first, then name the copy by its own hash: a file changing meanwhile can't end
                // up under another content's name.
                let tmp = blobs.join(format!(".incoming-{}.tmp", std::process::id()));
                let n = store::sys::copy_data(&p, &tmp).with_context(|| format!("copy {}", p.display()))?;
                copying.count(n, 1);
                let h = store::naming::hash16_file(&tmp)?;
                let dst = blobs.join(&h);
                if dst.exists() {
                    std::fs::remove_file(&tmp)?;
                } else {
                    std::fs::rename(&tmp, &dst)?;
                    rep.new_bytes += n;
                    rep.new_blobs += 1;
                }
                h
            }
        };
        now.insert(rel, Entry { size, mtime, blob });
    }
    drop(copying);
    rep.files = now.len();
    if now != prev {
        let _p = phase("day's list written", Class::NasWrite);
        write_atomic(&dir.join(format!("{today}.json")), &serde_json::to_vec_pretty(&now)?)?;
        rep.written = Some(today.to_string());
    }
    let (d, b) = {
        let _p = phase("old days pruned", Class::NasWrite);
        prune(&dir, today, keep_days)?
    };
    rep.days_removed = d;
    rep.blobs_removed = b;
    if let Some(l) = local {
        let _p = phase("mirrored to this Mac", Class::Disk);
        mirror(&dir, l, today, keep_days)?;
    }
    Ok(rep)
}

/// Removes the days older than `keep_days` (always keeping the newest) and the blobs no kept day
/// lists.
fn prune(dir: &Path, today: &str, keep_days: u64) -> Result<(usize, usize)> {
    let all = days(dir);
    let cutoff = day_before(today, keep_days);
    let mut removed = 0;
    let mut kept: Vec<String> = Vec::new();
    for (i, d) in all.iter().enumerate() {
        if d.as_str() < cutoff.as_str() && i + 1 < all.len() {
            std::fs::remove_file(dir.join(format!("{d}.json")))?;
            removed += 1;
        } else {
            kept.push(d.clone());
        }
    }
    let mut live: BTreeSet<String> = BTreeSet::new();
    for d in &kept {
        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join(format!("{d}.json")))?)?;
        live.extend(m.into_values().map(|e| e.blob));
    }
    let mut blobs_removed = 0;
    if let Ok(rd) = std::fs::read_dir(dir.join("blobs")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with('.') {
                continue;
            }
            if !live.contains(&n) {
                std::fs::remove_file(e.path())?;
                blobs_removed += 1;
            }
        }
    }
    Ok((removed, blobs_removed))
}

/// Mirrors the backups to the build Mac: missing blobs and days copied, the same pruning.
fn mirror(dir: &Path, local: &Path, today: &str, keep_days: u64) -> Result<()> {
    std::fs::create_dir_all(local.join("blobs"))?;
    for d in days(dir) {
        let f = format!("{d}.json");
        if !local.join(&f).exists() || d.as_str() == today {
            copy_atomic(&dir.join(&f), &local.join(&f))?;
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir.join("blobs")) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let dst = local.join("blobs").join(e.file_name());
            if !dst.exists() {
                copy_atomic(&e.path(), &dst)?;
            }
        }
    }
    prune(local, today, keep_days)?;
    Ok(())
}

/// The day `n` days before `day` (YYYY-MM-DD), by the calendar.
fn day_before(day: &str, n: u64) -> String {
    let secs = parse_day(day).map(|s| s.saturating_sub(n * 86400)).unwrap_or(0);
    format_day(UNIX_EPOCH + Duration::from_secs(secs))
}

fn parse_day(day: &str) -> Option<u64> {
    let (y, m, d) = (day.get(0..4)?.parse::<i64>().ok()?, day.get(5..7)?.parse::<i64>().ok()?, day.get(8..10)?.parse::<i64>().ok()?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400).ok()
}

/// YYYY-MM-DD of a time (UTC).
pub fn format_day(t: SystemTime) -> String {
    let days = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86400).unwrap_or(0) as i64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn days_round_trip() {
        for d in ["1970-01-01", "2026-10-02", "2024-02-29", "2000-03-01", "2026-12-31"] {
            assert_eq!(format_day(UNIX_EPOCH + Duration::from_secs(parse_day(d).unwrap())), d);
        }
        assert_eq!(day_before("2026-10-02", 30), "2026-09-02");
        assert_eq!(day_before("2026-03-01", 1), "2026-02-28");
    }

    #[test]
    fn backs_up_changes_only_and_prunes() {
        let d = tempfile::tempdir().unwrap();
        let (root, local) = (d.path().join("nas"), d.path().join("mac"));
        fs::create_dir_all(root.join("translations/jp")).unwrap();
        fs::create_dir_all(root.join("translations/todo")).unwrap();
        fs::create_dir_all(root.join("inputs/regions")).unwrap();
        fs::write(root.join("translations/jp/places-jp.jsonl"), b"{\"n\":\"a\"}\n").unwrap();
        fs::write(root.join("translations/todo/jp.jsonl"), b"skipped").unwrap();
        fs::write(root.join("inputs/regions/x.toml"), b"id = \"x\"").unwrap();
        let r = run(&root, Some(&local), "2026-09-01", 30).unwrap();
        assert_eq!((r.files, r.new_blobs, r.written.as_deref()), (2, 2, Some("2026-09-01")));
        // Nothing changed: no new day.
        let r = run(&root, Some(&local), "2026-09-02", 30).unwrap();
        assert_eq!((r.new_blobs, r.written), (0, None));
        // A change: a new day with one new blob; the old blob stays (the first day lists it).
        fs::write(root.join("inputs/regions/x.toml"), b"id = \"x\"\nname = \"X\"").unwrap();
        let r = run(&root, Some(&local), "2026-09-03", 30).unwrap();
        assert_eq!((r.new_blobs, r.written.as_deref()), (1, Some("2026-09-03")));
        assert_eq!(fs::read_dir(root.join("state/backups/blobs")).unwrap().count(), 3);
        // A month later the first day goes, and with it the blob only it listed.
        let r = run(&root, Some(&local), "2026-10-04", 30).unwrap();
        assert_eq!((r.days_removed, r.blobs_removed), (1, 1));
        assert_eq!(fs::read_dir(local.join("blobs")).unwrap().count(), 2);
        assert!(local.join("2026-09-03.json").exists());
        assert!(!local.join("2026-09-01.json").exists());
    }
}
