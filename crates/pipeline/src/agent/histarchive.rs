//! The history's old days, one tar a day (docs/pool.md §7.5, task #531): `state/coord/history/<day>/`
//! folders hold a file a writer, so a week of them is hundreds of small files on a share that
//! makes 20-55 a second. A day the page view no longer reads (`history::DAYS`: today's and the six
//! before) is packed as `state/coord/history-archive/<day>.tar` and its folder removed.
//!
//! Run by the daily `gc` job (lead only, outside the agent's loop and the pool's lock), every NAS
//! call under `nasread`'s deadlines. A day:
//! - its files read whole (one read each), the tar built in memory and written in one stream to a
//!   temporary name (one fsync), read back and its listing and bytes checked against what was
//!   read, then renamed into place;
//! - only then the folder's files and the folder removed, and only if the folder still lists what
//!   was packed (a late append to an old day leaves the folder; the next run packs the day again,
//!   replacing the tar);
//! - a tar already there that matches the folder (a run stopped between the rename and the
//!   removals) is not written again;
//! - temporary files a stopped run left are removed once an hour old.

use super::nasread;
use crate::coord::history::DAYS;
use crate::pool::journal;
use anyhow::{ensure, Context, Result};
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const DIR: &str = "state/coord/history";
pub const ARCHIVE: &str = "state/coord/history-archive";
/// A day larger than this (bytes) is passed over, said as a warning: it is held in memory whole.
const DAY_MAX: u64 = 512 << 20;
/// A temporary file older than this is a stopped run's.
const TMP_OLD: Duration = Duration::from_secs(3600);

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub days_archived: usize,
    pub files_removed: usize,
    pub tmp_removed: usize,
    /// NAS metadata work, for the job's costs (crate::costin::meta): folders listed, files statted.
    pub listed: u64,
    pub stats: u64,
    pub warnings: Vec<String>,
}

/// The first day folder kept: those before it are archived (the page view reads `DAYS` days back).
pub fn cutoff(now: u64) -> Option<String> {
    journal::day(now.saturating_sub((DAYS - 1) * 86_400))
}

/// Archives every day folder before the cutoff. `between` runs before each day (a safe point).
pub fn run(root: &Path, now: u64, dry_run: bool, between: &dyn Fn()) -> Result<Report> {
    let mut rep = Report::default();
    let Some(cut) = cutoff(now) else { return Ok(rep) };
    let dir = root.join(DIR);
    let names = match list(&dir) {
        Ok(n) => n,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(rep),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    rep.listed += 1;
    let mut days: Vec<String> = names.into_iter().filter(|n| journal::is_day(n) && *n < cut).collect();
    days.sort();
    let arch = root.join(ARCHIVE);
    if !dry_run && !days.is_empty() {
        sweep_tmp(&arch, &mut rep);
    }
    for day in days {
        between();
        if let Err(e) = archive_day(&dir.join(&day), &arch, &day, dry_run, &mut rep) {
            rep.warnings.push(format!("history {day} not archived: {e:#}"));
        }
    }
    Ok(rep)
}

fn list(dir: &Path) -> io::Result<Vec<String>> {
    let d = dir.to_path_buf();
    nasread::with(dir, move || {
        let mut v = Vec::new();
        for e in std::fs::read_dir(&d)? {
            v.push(e?.file_name().to_string_lossy().into_owned());
        }
        Ok(v)
    })
}

/// The day folder's regular files (name, size), temporary ones left out, sorted; an error for
/// anything that is not a file.
fn files(dir: &Path) -> io::Result<Vec<(String, u64)>> {
    let d = dir.to_path_buf();
    nasread::with(dir, move || {
        let mut v = Vec::new();
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let md = e.metadata()?;
            let name = e.file_name().to_string_lossy().into_owned();
            if !md.is_file() {
                return Err(io::Error::other(format!("{name} is not a file")));
            }
            if crate::whole::is_tmp(Path::new(&name)) {
                continue;
            }
            v.push((name, md.len()));
        }
        v.sort();
        Ok(v)
    })
}

/// Removes the archive folder's temporary files a stopped run left (older than `TMP_OLD`).
fn sweep_tmp(arch: &Path, rep: &mut Report) {
    let Ok(names) = list(arch) else { return };
    rep.listed += 1;
    for n in names {
        let p = arch.join(&n);
        if !crate::whole::is_tmp(&p) {
            continue;
        }
        let q = p.clone();
        let old = nasread::with(&p, move || std::fs::metadata(&q)?.modified())
            .ok()
            .is_some_and(|t| SystemTime::now().duration_since(t).is_ok_and(|d| d > TMP_OLD));
        rep.stats += 1;
        if old && std::fs::remove_file(&p).is_ok() {
            rep.tmp_removed += 1;
        }
    }
}

fn archive_day(folder: &Path, arch: &Path, day: &str, dry_run: bool, rep: &mut Report) -> Result<()> {
    let listed = files(folder).with_context(|| format!("list {}", folder.display()))?;
    rep.listed += 1;
    if listed.is_empty() {
        if !dry_run {
            let _ = std::fs::remove_dir(folder);
        }
        return Ok(());
    }
    let total: u64 = listed.iter().map(|f| f.1).sum();
    ensure!(total <= DAY_MAX, "{total} bytes, over {DAY_MAX}");
    let tar_path = arch.join(format!("{day}.tar"));
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, size) in &listed {
        let b = nasread::read(&folder.join(name)).with_context(|| format!("read {name}"))?;
        rep.stats += 1;
        ensure!(b.len() as u64 == *size, "{name} read {} of {size} bytes", b.len());
        members.push((format!("{day}/{name}"), b));
    }
    if dry_run {
        return Ok(());
    }
    // A complete tar of the same files is there already: a run stopped before the removals.
    let have = nasread::read(&tar_path).ok().is_some_and(|t| list_tar(&t).is_ok_and(|l| l == members));
    if !have {
        let tar = build(&members)?;
        std::fs::create_dir_all(arch)?;
        let tmp = crate::whole::tmp_name(&tar_path);
        let (t, bytes) = (tmp.clone(), tar);
        let written = nasread::write(&tmp, move || {
            use std::io::Write;
            let mut f = std::fs::File::create(&t)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            Ok(())
        });
        let r = written.and_then(|()| {
            let back = nasread::read(&tmp)?;
            ensure!(list_tar(&back)? == members, "the written tar does not match what was read");
            crate::whole::rename_over(&tmp, &tar_path)?;
            Ok(())
        });
        if let Err(e) = r {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        rep.days_archived += 1;
    }
    // The tar is whole: the folder goes, if it still lists what was packed.
    let now = files(folder)?;
    rep.listed += 1;
    ensure!(now == listed, "the folder changed while it was packed (kept, packed again next run)");
    for (name, _) in &listed {
        std::fs::remove_file(folder.join(name)).with_context(|| format!("remove {name}"))?;
        rep.files_removed += 1;
    }
    std::fs::remove_dir(folder).with_context(|| format!("remove {}", folder.display()))?;
    Ok(())
}

/// A tar (ustar) of `members`, in order: the whole stream.
pub fn build(members: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (name, b) in members {
        ensure!(name.len() <= 100, "{name}: name over 100 bytes");
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[108..115].copy_from_slice(b"0000000");
        h[116..123].copy_from_slice(b"0000000");
        h[124..135].copy_from_slice(format!("{:011o}", b.len()).as_bytes());
        h[136..147].copy_from_slice(b"00000000000");
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&c| c as u32).sum();
        h[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
        h[154] = 0;
        h[155] = b' ';
        out.extend_from_slice(&h);
        out.extend_from_slice(b);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    Ok(out)
}

/// The members (name, bytes) of a tar made by `build`; an error when it is cut short or damaged.
pub fn list_tar(t: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut v = Vec::new();
    let mut at = 0;
    loop {
        ensure!(t.len() >= at + 512, "tar cut short");
        let h = &t[at..at + 512];
        if h.iter().all(|&c| c == 0) {
            ensure!(t.len() == at + 1024 && t[at + 512..].iter().all(|&c| c == 0), "tar end damaged");
            return Ok(v);
        }
        let end = h[..100].iter().position(|&c| c == 0).unwrap_or(100);
        let name = String::from_utf8(h[..end].to_vec())?;
        let size = usize::from_str_radix(std::str::from_utf8(&h[124..135])?, 8).context("size")?;
        let mut c = h.to_vec();
        c[148..156].copy_from_slice(b"        ");
        let sum: u32 = c.iter().map(|&x| x as u32).sum();
        ensure!(
            u32::from_str_radix(std::str::from_utf8(&h[148..154])?, 8).ok() == Some(sum),
            "{name}: header checksum"
        );
        let (a, b) = (at + 512, at + 512 + size);
        ensure!(t.len() >= b.div_ceil(512) * 512, "{name}: cut short");
        v.push((name, t[a..b].to_vec()));
        at = b.div_ceil(512) * 512;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_791_600_000; // 2026-10-10

    fn day_of(back: u64) -> String {
        journal::day(NOW - back * 86_400).unwrap()
    }

    fn put(root: &Path, day: &str, member: &str, body: &str) {
        let d = root.join(DIR).join(day);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{member}.jsonl")), body).unwrap();
    }

    #[test]
    fn tar_round_trips_and_detects_damage() {
        let m = vec![("a/x.jsonl".to_string(), vec![1u8; 700]), ("a/y.jsonl".to_string(), b"hi".to_vec())];
        let t = build(&m).unwrap();
        assert_eq!(t.len() % 512, 0);
        assert_eq!(list_tar(&t).unwrap(), m);
        assert!(list_tar(&t[..t.len() - 600]).is_err());
        let mut bad = t.clone();
        bad[3] ^= 1;
        assert!(list_tar(&bad).is_err());
    }

    #[test]
    fn old_days_archived_and_removed_recent_kept() {
        let r = tempfile::tempdir().unwrap();
        let (old, edge, keep) = (day_of(7), day_of(6), day_of(0));
        put(r.path(), &old, "m1", "a\n");
        put(r.path(), &old, "m2", "bb\n");
        put(r.path(), &edge, "m1", "c\n");
        put(r.path(), &keep, "m1", "d\n");
        let rep = run(r.path(), NOW, false, &|| {}).unwrap();
        assert_eq!((rep.days_archived, rep.files_removed), (1, 2), "{rep:?}");
        assert!(rep.warnings.is_empty());
        assert!(!r.path().join(DIR).join(&old).exists());
        assert!(r.path().join(DIR).join(&edge).exists());
        assert!(r.path().join(DIR).join(&keep).exists());
        let t = std::fs::read(r.path().join(ARCHIVE).join(format!("{old}.tar"))).unwrap();
        let l = list_tar(&t).unwrap();
        assert_eq!(
            l.iter().map(|m| m.0.clone()).collect::<Vec<_>>(),
            [format!("{old}/m1.jsonl"), format!("{old}/m2.jsonl")]
        );
        assert_eq!(l[1].1, b"bb\n");
        // Again: nothing to do.
        assert_eq!(run(r.path(), NOW, false, &|| {}).unwrap().days_archived, 0);
    }

    #[test]
    fn dry_run_changes_nothing() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(9);
        put(r.path(), &old, "m1", "a\n");
        run(r.path(), NOW, true, &|| {}).unwrap();
        assert!(r.path().join(DIR).join(&old).join("m1.jsonl").exists());
        assert!(!r.path().join(ARCHIVE).exists());
    }

    #[test]
    fn a_complete_tar_left_by_a_stopped_run_is_kept_and_the_folder_removed() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(r.path(), &old, "m1", "a\n");
        let tar = build(&[(format!("{old}/m1.jsonl"), b"a\n".to_vec())]).unwrap();
        std::fs::create_dir_all(r.path().join(ARCHIVE)).unwrap();
        let tp = r.path().join(ARCHIVE).join(format!("{old}.tar"));
        std::fs::write(&tp, &tar).unwrap();
        let rep = run(r.path(), NOW, false, &|| {}).unwrap();
        assert_eq!((rep.days_archived, rep.files_removed), (0, 1));
        assert_eq!(std::fs::read(&tp).unwrap(), tar);
        assert!(!r.path().join(DIR).join(&old).exists());
    }

    #[test]
    fn a_damaged_or_stale_tar_is_replaced_from_the_folder() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(r.path(), &old, "m1", "a\nb\n");
        std::fs::create_dir_all(r.path().join(ARCHIVE)).unwrap();
        let tp = r.path().join(ARCHIVE).join(format!("{old}.tar"));
        std::fs::write(&tp, b"cut short").unwrap();
        run(r.path(), NOW, false, &|| {}).unwrap();
        assert_eq!(list_tar(&std::fs::read(&tp).unwrap()).unwrap()[0].1, b"a\nb\n");
        assert!(!r.path().join(DIR).join(&old).exists());
    }

    #[test]
    fn a_late_write_to_an_archived_day_is_packed_again_over_the_tar() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(r.path(), &old, "m1", "a\n");
        run(r.path(), NOW, false, &|| {}).unwrap();
        put(r.path(), &old, "m3", "later\n");
        let rep = run(r.path(), NOW, false, &|| {}).unwrap();
        assert_eq!(rep.days_archived, 1);
        let t = list_tar(&std::fs::read(r.path().join(ARCHIVE).join(format!("{old}.tar"))).unwrap()).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].1, b"later\n");
    }

    #[test]
    fn a_stopped_runs_old_temporary_file_is_swept_a_young_one_not() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(r.path(), &old, "m1", "a\n");
        let a = r.path().join(ARCHIVE);
        std::fs::create_dir_all(&a).unwrap();
        let (stale, fresh) = (a.join("x.tar.h.1.tmp"), a.join("y.tar.h.2.tmp"));
        std::fs::write(&stale, "x").unwrap();
        std::fs::write(&fresh, "y").unwrap();
        let f = std::fs::File::options().write(true).open(&stale).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
        let rep = run(r.path(), NOW, false, &|| {}).unwrap();
        assert_eq!(rep.tmp_removed, 1);
        assert!(!stale.exists() && fresh.exists());
    }

    #[test]
    fn a_folder_with_something_else_in_it_is_passed_over_with_a_warning() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(r.path(), &old, "m1", "a\n");
        std::fs::create_dir(r.path().join(DIR).join(&old).join("sub")).unwrap();
        let rep = run(r.path(), NOW, false, &|| {}).unwrap();
        assert_eq!(rep.warnings.len(), 1);
        assert!(r.path().join(DIR).join(&old).join("m1.jsonl").exists());
    }

    #[test]
    fn the_cutoff_is_the_page_views_window() {
        assert_eq!(cutoff(NOW).unwrap(), day_of(DAYS - 1));
    }
}
