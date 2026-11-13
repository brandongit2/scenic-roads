//! The history's days, one tar a day on the NAS (docs/pool.md §7.5, tasks #531, #1119):
//! `state/coord/history-archive/<day>.tar`, a member a writer (`<day>/<member>.jsonl`), kept for
//! good; `pubtimes::load_history` reads them.
//!
//! Run by the daily `gc` job (lead only, outside the agent's loop and the pool's lock), every NAS
//! call under `nasread`'s deadlines:
//! - **The lead's history** (`run`'s `local`: its coordinator's `coord/history.jsonl`, on its own
//!   disk, the last week's): each day before today packed under the declared lead's member, its
//!   lines merged into what the day's tar holds (`merge_lines`: those already there not again), so
//!   each finished day is on the NAS by the next run, and the lead's disk lost loses today's alone
//!   (and those since the last run).
//! - **The day folders an app before #1119 wrote** (`state/coord/history/<day>/`, a file per
//!   writer): a day older than the history's week (`history::DAYS`) is packed and its folder
//!   removed, until none is left (then this part finds nothing: on 13 Nov the NAS held seven,
//!   the last written 12 Nov). A day:
//! - its files read whole (one read each) and merged into what its tar already holds (`merge`:
//!   a late write, an event of an old day appended after the day was packed, is added, never
//!   written over the tar; a tar that can't be read is never replaced, the day waits, said);
//! - the tar built in memory, written in one stream to a temporary name (one fsync), read back
//!   and checked against what was built, then renamed into place;
//! - only then the folder's files and the folder removed, and only if the folder still lists
//!   what was packed (names and sizes; else it waits for the next run). An append landing between
//!   that listing and the file's removal (milliseconds, and only to a week-old day) is the one
//!   way a line is lost;
//! - temporary files a stopped run left are removed once an hour old.

use super::nasread;
use crate::coord::history::DAYS;
use crate::pool::journal;
use anyhow::{ensure, Context, Result};
use codecs::tar::{build, list_tar};
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};
use store::nasfs::NasPath;

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
    /// NAS metadata work, for the job's costs (buildstore::costin::meta): folders listed, files statted.
    pub listed: u64,
    pub stats: u64,
    pub warnings: Vec<String>,
}

/// The first day folder kept: those before it are archived (the page view reads `DAYS` days back).
pub fn cutoff(now: u64) -> Option<String> {
    journal::day(now.saturating_sub((DAYS - 1) * 86_400))
}

/// Archives the lead's history (`local`: its coordinator's file) of every day before today's, then
/// every day folder before the cutoff. `between` runs before each day (a safe point).
pub fn run(
    root: &store::nasfs::NasRoot,
    local: Option<&Path>,
    now: u64,
    dry_run: bool,
    between: &dyn Fn(),
) -> Result<Report> {
    let mut rep = Report::default();
    if let Some(f) = local {
        archive_local(root, f, now, dry_run, between, &mut rep)?;
    }
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

/// The lead's history file `f`'s days before today's, each merged into its day's tar under the
/// declared lead's member (none declared: nothing packed, said).
fn archive_local(
    root: &store::nasfs::NasRoot,
    f: &Path,
    now: u64,
    dry_run: bool,
    between: &dyn Fn(),
    rep: &mut Report,
) -> Result<()> {
    let text = match std::fs::read_to_string(f) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", f.display())),
    };
    let Some(today) = journal::day(now) else { return Ok(()) };
    let mut days: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
    for l in text.lines() {
        let Ok(e) = serde_json::from_str::<crate::coord::history::Event>(l) else { continue };
        let Some(day) = journal::day(e.t).filter(|d| *d < today) else { continue };
        let b = days.entry(day).or_default();
        b.extend_from_slice(l.as_bytes());
        b.push(b'\n');
    }
    if days.is_empty() {
        return Ok(());
    }
    let Some(lead) = buildstore::records::lead::declared(root)? else {
        rep.warnings.push("the lead's history not archived: no lead declared".into());
        return Ok(());
    };
    let arch = root.join(ARCHIVE);
    if !dry_run {
        sweep_tmp(&arch, rep);
    }
    for (day, b) in days {
        between();
        if dry_run {
            continue;
        }
        let tar = arch.join(format!("{day}.tar"));
        match merge_into_tar(&tar, vec![(format!("{day}/{}.jsonl", lead.member), b)], merge_lines) {
            Ok(true) => rep.days_archived += 1,
            Ok(false) => {}
            Err(e) => rep.warnings.push(format!("history {day} not archived: {e:#}")),
        }
    }
    Ok(())
}

fn list(dir: &NasPath) -> io::Result<Vec<String>> {
    let d = dir.clone();
    nasread::with(dir, move || Ok(d.read_dir()?.into_iter().map(|e| e.name).collect()))
}

/// The day folder's regular files (name, size), temporary ones left out, sorted; an error for
/// anything that is not a file.
pub(crate) fn files(dir: &NasPath) -> io::Result<Vec<(String, u64)>> {
    let d = dir.clone();
    nasread::with(dir, move || {
        let mut v = Vec::new();
        for e in d.read_dir()? {
            let md = e.symlink_metadata()?;
            let name = e.name;
            if !md.is_file() {
                return Err(io::Error::other(format!("{name} is not a file")));
            }
            if store::whole::is_tmp(Path::new(&name)) {
                continue;
            }
            v.push((name, md.len()));
        }
        v.sort();
        Ok(v)
    })
}

/// Removes the archive folder's temporary files a stopped run left (older than `TMP_OLD`).
pub(crate) fn sweep_tmp(arch: &NasPath, rep: &mut Report) {
    let Ok(names) = list(arch) else { return };
    rep.listed += 1;
    for n in names {
        let p = arch.join(&n);
        if !store::whole::is_tmp(Path::new(&n)) {
            continue;
        }
        let q = p.clone();
        let old = nasread::with(&p, move || q.metadata()?.modified())
            .ok()
            .is_some_and(|t| SystemTime::now().duration_since(t).is_ok_and(|d| d > TMP_OLD));
        rep.stats += 1;
        if old && p.remove_file().is_ok() {
            rep.tmp_removed += 1;
        }
    }
}

fn archive_day(folder: &NasPath, arch: &NasPath, day: &str, dry_run: bool, rep: &mut Report) -> Result<()> {
    let listed = files(folder).with_context(|| format!("list {}", folder.display()))?;
    rep.listed += 1;
    if listed.is_empty() {
        if !dry_run {
            let _ = folder.remove_dir();
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
    if merge_into_tar(&tar_path, members, merge)? {
        rep.days_archived += 1;
    }
    // The tar is whole: the folder goes, if it still lists what was packed.
    let now = files(folder)?;
    rep.listed += 1;
    ensure!(now == listed, "the folder changed while it was packed (kept, packed again next run)");
    for (name, _) in &listed {
        folder.join(name).remove_file().with_context(|| format!("remove {name}"))?;
        rep.files_removed += 1;
    }
    folder.remove_dir().with_context(|| format!("remove {}", folder.display()))?;
    Ok(())
}

/// `members` merged into the tar at `tar_path` (`merge`; made when there's none): what a tar
/// already there holds (an earlier run's, the folder since written to late, or a run stopped before
/// the removals) is kept, never written over. A tar that can't be read is never replaced: an error,
/// the caller's files kept. Built in memory, written in one stream to a temporary name (one fsync),
/// read back and checked, then renamed into place. Whether it was written (false: it held them all).
pub(crate) fn merge_into_tar(
    tar_path: &NasPath,
    members: Vec<(String, Vec<u8>)>,
    merge: fn(&[(String, Vec<u8>)], Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)>,
) -> Result<bool> {
    let old = match nasread::read(tar_path) {
        Ok(t) => list_tar(&t).with_context(|| format!("{} unreadable, left as it is", tar_path.display()))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", tar_path.display())),
    };
    let packed = merge(&old, members);
    if packed == old {
        return Ok(false);
    }
    let tar = build(&packed)?;
    tar_path.parent().context("a tar's folder")?.create_dir_all()?;
    let tmp = tar_path.tmp_name();
    let (t, bytes) = (tmp.clone(), tar);
    let written = nasread::write(&tmp, move || {
        use std::io::Write;
        let mut f = t.create()?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        f.close()?;
        Ok(())
    });
    let r = written.and_then(|()| {
        let back = nasread::read(&tmp)?;
        ensure!(list_tar(&back)? == packed, "the written tar does not match what was read");
        tmp.rename_over(tar_path)?;
        Ok(())
    });
    if let Err(e) = r {
        let _ = tmp.remove_file();
        return Err(e);
    }
    Ok(true)
}

/// The tar's members with `new`'s lines merged in, by name: each line a member doesn't hold yet
/// appended to it, in order (the lead's history packed again as its day fills: a line packed
/// before not again).
fn merge_lines(old: &[(String, Vec<u8>)], new: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    let mut m: std::collections::BTreeMap<String, Vec<u8>> = old.iter().cloned().collect();
    for (name, b) in new {
        let have = m.entry(name).or_default();
        let held: std::collections::HashSet<Vec<u8>> = have.split(|&c| c == b'\n').map(<[u8]>::to_vec).collect();
        for l in b.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
            if !held.contains(l) {
                have.extend_from_slice(l);
                have.push(b'\n');
            }
        }
    }
    m.into_iter().collect()
}

/// The tar's members with the folder's merged in, by name: a folder file that begins with the
/// member's bytes (the same file, packed by a stopped run, maybe appended to since) takes its place;
/// one the member ends with was merged by a run stopped before the removals; any other (a file
/// begun again after its day was packed: a late write) is appended to it.
pub(crate) fn merge(old: &[(String, Vec<u8>)], folder: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    let mut m: std::collections::BTreeMap<String, Vec<u8>> = old.iter().cloned().collect();
    for (name, b) in folder {
        match m.get_mut(&name) {
            Some(have) if b.starts_with(have) => *have = b,
            Some(have) if have.ends_with(&b) => {}
            Some(have) => have.extend_from_slice(&b),
            None => {
                m.insert(name, b);
            }
        }
    }
    m.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_791_600_000; // 2026-10-10

    fn day_of(back: u64) -> String {
        journal::day(NOW - back * 86_400).unwrap()
    }

    fn put(root: &store::nasfs::NasRoot, day: &str, member: &str, body: &str) {
        let d = root.join(DIR).join(day);
        d.create_dir_all().unwrap();
        d.join(format!("{member}.jsonl")).write(body).unwrap();
    }

    #[test]
    fn old_days_archived_and_removed_recent_kept() {
        let r = tempfile::tempdir().unwrap();
        let (old, edge, keep) = (day_of(7), day_of(6), day_of(0));
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "a\n");
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m2", "bb\n");
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &edge, "m1", "c\n");
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &keep, "m1", "d\n");
        let rep = run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap();
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
        assert_eq!(
            run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap().days_archived,
            0
        );
    }

    #[test]
    fn dry_run_changes_nothing() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(9);
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "a\n");
        run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, true, &|| {}).unwrap();
        assert!(r.path().join(DIR).join(&old).join("m1.jsonl").exists());
        assert!(!r.path().join(ARCHIVE).exists());
    }

    #[test]
    fn a_complete_tar_left_by_a_stopped_run_is_kept_and_the_folder_removed() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "a\n");
        let tar = build(&[(format!("{old}/m1.jsonl"), b"a\n".to_vec())]).unwrap();
        std::fs::create_dir_all(r.path().join(ARCHIVE)).unwrap();
        let tp = r.path().join(ARCHIVE).join(format!("{old}.tar"));
        std::fs::write(&tp, &tar).unwrap();
        let rep = run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap();
        assert_eq!((rep.days_archived, rep.files_removed), (0, 1));
        assert_eq!(std::fs::read(&tp).unwrap(), tar);
        assert!(!r.path().join(DIR).join(&old).exists());
    }

    #[test]
    fn a_damaged_tar_is_never_replaced_and_its_day_kept() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "a\nb\n");
        std::fs::create_dir_all(r.path().join(ARCHIVE)).unwrap();
        let tp = r.path().join(ARCHIVE).join(format!("{old}.tar"));
        std::fs::write(&tp, b"cut short").unwrap();
        let rep = run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap();
        assert_eq!(rep.warnings.len(), 1, "{rep:?}");
        assert_eq!(std::fs::read(&tp).unwrap(), b"cut short");
        assert!(r.path().join(DIR).join(&old).join("m1.jsonl").exists());
    }

    #[test]
    fn a_late_write_to_an_archived_day_is_merged_into_its_tar() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "a\n");
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m2", "b\n");
        run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap();
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "later\n");
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m3", "new\n");
        let rep = run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap();
        assert_eq!((rep.days_archived, rep.files_removed), (1, 2), "{rep:?}");
        assert!(!r.path().join(DIR).join(&old).exists());
        let t = list_tar(&std::fs::read(r.path().join(ARCHIVE).join(format!("{old}.tar"))).unwrap()).unwrap();
        let want: Vec<(String, Vec<u8>)> = vec![
            (format!("{old}/m1.jsonl"), b"a\nlater\n".to_vec()),
            (format!("{old}/m2.jsonl"), b"b\n".to_vec()),
            (format!("{old}/m3.jsonl"), b"new\n".to_vec()),
        ];
        assert_eq!(t, want);
    }

    /// Task #1119: the lead's history, on its own disk, packed a day a tar under the declared lead
    /// as each day ends (today's not yet), its lines merged: packed again, none doubled; a day's
    /// later lines added.
    #[test]
    fn the_leads_history_is_packed_as_each_day_ends() {
        let r = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(r.path());
        std::fs::create_dir_all(r.path().join("state/build")).unwrap();
        std::fs::write(r.path().join(buildstore::records::lead::LEAD), r#"{"member":"m-1","host":"h"}"#).unwrap();
        let ev = |seq: u64, t: u64| {
            serde_json::to_string(&crate::coord::history::Event { seq, t, ..crate::coord::history::Event::new("x") })
                .unwrap()
                + "\n"
        };
        let f = r.path().join("history.jsonl");
        let (two, one) = (NOW - 2 * 86_400, NOW - 86_400);
        std::fs::write(&f, ev(1, two) + &ev(2, one) + &ev(3, NOW)).unwrap();
        let rep = run(&root, Some(&f), NOW, false, &|| {}).unwrap();
        assert_eq!(rep.days_archived, 2, "{rep:?}");
        let tar = |t: u64| {
            let d = journal::day(t).unwrap();
            list_tar(&std::fs::read(r.path().join(ARCHIVE).join(format!("{d}.tar"))).unwrap()).unwrap()
        };
        assert_eq!(tar(one), [(format!("{}/m-1.jsonl", journal::day(one).unwrap()), ev(2, one).into_bytes())]);
        assert!(
            !r.path().join(ARCHIVE).join(format!("{}.tar", journal::day(NOW).unwrap())).exists(),
            "today's not yet"
        );
        assert_eq!(run(&root, Some(&f), NOW, false, &|| {}).unwrap().days_archived, 0, "packed again: nothing new");
        std::fs::write(&f, ev(1, two) + &ev(2, one) + &ev(4, one + 60) + &ev(3, NOW)).unwrap();
        assert_eq!(run(&root, Some(&f), NOW, false, &|| {}).unwrap().days_archived, 1);
        assert_eq!(tar(one)[0].1, (ev(2, one) + &ev(4, one + 60)).into_bytes(), "the day's later line added once");
    }

    #[test]
    fn merging_again_after_a_stop_neither_loses_nor_doubles() {
        let n = |s: &str| s.to_string();
        let tar = vec![(n("d/m1"), b"a\nb\n".to_vec()), (n("d/m2"), b"c\n".to_vec())];
        // Packed then appended to (a stopped run, then a write): the folder's file takes its place.
        assert_eq!(merge(&tar, vec![(n("d/m1"), b"a\nb\nz\n".to_vec())])[0].1, b"a\nb\nz\n");
        // A late file merged, the run stopped before its removal: not appended twice.
        assert_eq!(merge(&tar, vec![(n("d/m1"), b"b\n".to_vec())]), tar);
        // A late file: appended.
        assert_eq!(merge(&tar, vec![(n("d/m2"), b"x\n".to_vec())])[1].1, b"c\nx\n");
    }

    #[test]
    fn a_stopped_runs_old_temporary_file_is_swept_a_young_one_not() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "a\n");
        let a = r.path().join(ARCHIVE);
        std::fs::create_dir_all(&a).unwrap();
        let (stale, fresh) = (a.join("x.tar.h.1.tmp"), a.join("y.tar.h.2.tmp"));
        std::fs::write(&stale, "x").unwrap();
        std::fs::write(&fresh, "y").unwrap();
        let f = std::fs::File::options().write(true).open(&stale).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
        let rep = run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap();
        assert_eq!(rep.tmp_removed, 1);
        assert!(!stale.exists() && fresh.exists());
    }

    #[test]
    fn a_folder_with_something_else_in_it_is_passed_over_with_a_warning() {
        let r = tempfile::tempdir().unwrap();
        let old = day_of(8);
        put(&store::nasfs::NasRoot::local_for_test(r.path()), &old, "m1", "a\n");
        std::fs::create_dir(r.path().join(DIR).join(&old).join("sub")).unwrap();
        let rep = run(&store::nasfs::NasRoot::local_for_test(r.path()), None, NOW, false, &|| {}).unwrap();
        assert_eq!(rep.warnings.len(), 1);
        assert!(r.path().join(DIR).join(&old).join("m1.jsonl").exists());
    }
}
