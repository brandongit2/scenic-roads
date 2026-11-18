//! The history's days, one tar a day on the NAS (docs/pool.md §7.5, tasks #531, #1119):
//! `state/coord/history-archive/<day>.tar`, a member a writer (`<day>/<member>.jsonl`), kept for
//! good; `pubtimes::load_history` reads them.
//!
//! Run by the daily `gc` job (lead only, outside the agent's loop and the pool's lock), every NAS
//! call under `nasread`'s deadlines: the lead's history (`run`'s `local`: its coordinator's
//! `coord/history.jsonl`, on its own disk, the last week's), each day before today packed under the
//! declared lead's member, its lines merged into what the day's tar holds (`merge_lines`: those
//! already there not again), so each finished day is on the NAS by the next run, and the lead's
//! disk lost loses today's alone (and those since the last run). A day's tar is built in memory,
//! written in one stream to a temporary name (one fsync), read back and checked against what was
//! built, then renamed into place (`merge_into_tar`, which the forecasts' archive uses too:
//! agent::fcarchive); a tar that can't be read is never replaced, the day waits, said. Temporary
//! files a stopped run left are removed once an hour old.

use super::nasread;
use crate::coord::history::DAYS;
use crate::pool::journal;
use anyhow::{ensure, Context, Result};
use codecs::tar::{build, list_tar};
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};
use store::nasfs::NasPath;

pub const ARCHIVE: &str = "state/coord/history-archive";
/// A temporary file older than this is a stopped run's.
const TMP_OLD: Duration = Duration::from_secs(3600);

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub days_archived: usize,
    pub tmp_removed: usize,
    /// NAS metadata work, for the job's costs (buildstore::costin::meta): folders listed, files statted.
    pub listed: u64,
    pub stats: u64,
    pub warnings: Vec<String>,
}

/// The first day of the history's window (`DAYS` days back from `now`'s): the forecasts' archive
/// packs the days before it (agent::fcarchive).
pub fn cutoff(now: u64) -> Option<String> {
    journal::day(now.saturating_sub((DAYS - 1) * 86_400))
}

/// Archives the lead's history (`local`: its coordinator's file; None: nothing) of every day before
/// today's, each merged into its day's tar under the declared lead's member (none declared: nothing
/// packed, said). `between` runs before each day (a safe point).
pub fn run(
    root: &store::nasfs::NasRoot,
    local: Option<&Path>,
    now: u64,
    dry_run: bool,
    between: &dyn Fn(),
) -> Result<Report> {
    let mut rep = Report::default();
    let Some(f) = local else { return Ok(rep) };
    let text = match std::fs::read_to_string(f) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(rep),
        Err(e) => return Err(e).with_context(|| format!("read {}", f.display())),
    };
    let Some(today) = journal::day(now) else { return Ok(rep) };
    let mut days: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
    for l in text.lines() {
        let Ok(e) = serde_json::from_str::<crate::coord::history::Event>(l) else { continue };
        let Some(day) = journal::day(e.t).filter(|d| *d < today) else { continue };
        let b = days.entry(day).or_default();
        b.extend_from_slice(l.as_bytes());
        b.push(b'\n');
    }
    if days.is_empty() {
        return Ok(rep);
    }
    let Some(lead) = buildstore::records::lead::declared(root)? else {
        rep.warnings.push("the lead's history not archived: no lead declared".into());
        return Ok(rep);
    };
    let arch = root.join(ARCHIVE);
    if !dry_run {
        sweep_tmp(&arch, &mut rep);
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
    Ok(rep)
}

fn list(dir: &NasPath) -> io::Result<Vec<String>> {
    let d = dir.clone();
    nasread::with(dir, move || Ok(d.read_dir()?.into_iter().map(|e| e.name).collect()))
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

/// `members` merged into the tar at `tar_path` (`merge`; made when there's none): what a tar
/// already there holds (an earlier run's, or a run stopped before its files' removals) is kept,
/// never written over. A tar that can't be read is never replaced: an error,
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

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_791_600_000; // 2026-10-10

    /// A root declaring lead `m-1`, and a history file of `events` (seq, t) beside it.
    fn lead_and_history(r: &Path, events: &[(u64, u64)]) -> (store::nasfs::NasRoot, std::path::PathBuf) {
        std::fs::create_dir_all(r.join("state/build")).unwrap();
        std::fs::write(r.join(buildstore::records::lead::LEAD), r#"{"member":"m-1","host":"h"}"#).unwrap();
        let f = r.join("history.jsonl");
        std::fs::write(&f, events.iter().map(|&(seq, t)| ev(seq, t)).collect::<String>()).unwrap();
        (store::nasfs::NasRoot::local_for_test(r), f)
    }

    fn ev(seq: u64, t: u64) -> String {
        serde_json::to_string(&crate::coord::history::Event { seq, t, ..crate::coord::history::Event::new("x") })
            .unwrap()
            + "\n"
    }

    fn tar_of(r: &Path, t: u64) -> std::path::PathBuf {
        r.join(ARCHIVE).join(format!("{}.tar", journal::day(t).unwrap()))
    }

    /// Task #1119: the lead's history, on its own disk, packed a day a tar under the declared lead
    /// as each day ends (today's not yet), its lines merged: packed again, none doubled; a day's
    /// later lines added.
    #[test]
    fn the_leads_history_is_packed_as_each_day_ends() {
        let r = tempfile::tempdir().unwrap();
        let (two, one) = (NOW - 2 * 86_400, NOW - 86_400);
        let (root, f) = lead_and_history(r.path(), &[(1, two), (2, one), (3, NOW)]);
        let rep = run(&root, Some(&f), NOW, false, &|| {}).unwrap();
        assert_eq!(rep.days_archived, 2, "{rep:?}");
        let tar = |t: u64| list_tar(&std::fs::read(tar_of(r.path(), t)).unwrap()).unwrap();
        assert_eq!(tar(one), [(format!("{}/m-1.jsonl", journal::day(one).unwrap()), ev(2, one).into_bytes())]);
        assert!(!tar_of(r.path(), NOW).exists(), "today's not yet");
        assert_eq!(run(&root, Some(&f), NOW, false, &|| {}).unwrap().days_archived, 0, "packed again: nothing new");
        std::fs::write(&f, ev(1, two) + &ev(2, one) + &ev(4, one + 60) + &ev(3, NOW)).unwrap();
        assert_eq!(run(&root, Some(&f), NOW, false, &|| {}).unwrap().days_archived, 1);
        assert_eq!(tar(one)[0].1, (ev(2, one) + &ev(4, one + 60)).into_bytes(), "the day's later line added once");
    }

    #[test]
    fn dry_run_changes_nothing() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW - 86_400)]);
        run(&root, Some(&f), NOW, true, &|| {}).unwrap();
        assert!(!r.path().join(ARCHIVE).exists());
    }

    #[test]
    fn a_damaged_tar_is_never_replaced_and_said() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW - 86_400)]);
        let tp = tar_of(r.path(), NOW - 86_400);
        std::fs::create_dir_all(tp.parent().unwrap()).unwrap();
        std::fs::write(&tp, b"cut short").unwrap();
        let rep = run(&root, Some(&f), NOW, false, &|| {}).unwrap();
        assert_eq!((rep.days_archived, rep.warnings.len()), (0, 1), "{rep:?}");
        assert_eq!(std::fs::read(&tp).unwrap(), b"cut short");
    }

    #[test]
    fn a_stopped_runs_old_temporary_file_is_swept_a_young_one_not() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW - 86_400)]);
        let a = r.path().join(ARCHIVE);
        std::fs::create_dir_all(&a).unwrap();
        let (stale, fresh) = (a.join("x.tar.h.1.tmp"), a.join("y.tar.h.2.tmp"));
        std::fs::write(&stale, "x").unwrap();
        std::fs::write(&fresh, "y").unwrap();
        let file = std::fs::File::options().write(true).open(&stale).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
        let rep = run(&root, Some(&f), NOW, false, &|| {}).unwrap();
        assert_eq!(rep.tmp_removed, 1);
        assert!(!stale.exists() && fresh.exists());
    }
}
