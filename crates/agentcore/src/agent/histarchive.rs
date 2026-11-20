//! The history's days, one tar a day on the NAS (docs/pool.md §7.5, tasks #531, #1119):
//! `state/coord/history-archive/<day>.tar`, a member a writer (`<day>/<member>.jsonl`), kept for
//! good; `pubtimes::load_history` reads them.
//!
//! Run by the daily `gc` job (lead only, outside the agent's loop and the pool's lock), every NAS
//! call under `nasread`'s deadlines: the lead's history (`run`'s `local`: its coordinator's
//! `coord/history.jsonl`, on its own disk, the last week's), each day before today packed under the
//! declared lead's member, its lines merged into what the day's tar holds (`merge_lines`: those
//! already there not again), so each finished day is on the NAS by the next run, and the lead's
//! disk lost loses today's alone (and those since the last run). A Mac that led and leads no more
//! packs what its file holds, every day of it under its own member, from its agent's start as a
//! member (`spawn_former`): the days since its GC's last run, the day it handed the lead over
//! among them; the file is removed only once a later look, a settling time after, finds every day
//! held there and the file unchanged (`Former`, task #1442). A day's tar is built in memory,
//! written in one stream to a temporary name (one fsync), read back and checked against what was
//! built, then renamed into place (`merge_into_tar`, which the forecasts' archive uses too:
//! agent::fcarchive); a tar that can't be read is never replaced, the day waits, said. Another
//! writer's merge of the same day renamed over this one's loses this one's lines from the tar: the
//! lead's file packs them again at its GC's next run, a former lead's at its next look. Temporary
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
    let Some(today) = journal::day(now) else { return Ok(rep) };
    let days = days_of(f, Some(&today))?;
    if days.is_empty() {
        return Ok(rep);
    }
    let Some(lead) = buildstore::records::lead::declared(root)? else {
        rep.warnings.push("the lead's history not archived: no lead declared".into());
        return Ok(rep);
    };
    pack(root, days, &lead.member, dry_run, between, &mut rep);
    Ok(rep)
}

/// How often a former lead's history is looked at (`spawn_former`) while the NAS is away, a pack
/// waits to settle or the last look found nothing wrong.
pub const FORMER_EVERY: Duration = Duration::from_secs(60);

/// The longest a look waits after one that went wrong (a day's tar unreadable, the file not read):
/// each such look doubles the wait from `FORMER_EVERY`, up to this.
pub const FORMER_BACKOFF_MAX: Duration = Duration::from_secs(3600);

/// How long after its pack a former lead's history is checked again before its file is removed
/// (`Former::look`, from when the pack returned): what it covers is another writer's merge of the
/// same day whose read just before its rename (`merge_into_tar`) came before this pack's rename,
/// its rename landing after; those two calls are back to back, each well inside any of the
/// agent's NAS deadlines, and this is twice the longest of them (agent::nasread's big read's,
/// 5 min), so such a rename has landed or failed by then. A writer frozen longer between its
/// read and its rename reads again before renaming and merges this one's lines in.
pub fn former_settle() -> Duration {
    2 * super::nasread::big_read_deadline()
}

/// Archives a former lead's history (`local`: the coordinator's file its process left as it led,
/// on this Mac's disk while it is a member), every day of it, today's too (no process of a member
/// writes it: the day it handed the lead over ends there), under `member`, this Mac's: the lead's
/// daily GC packs its own file alone, so the days since its last run were lost to the archive
/// (task #1442). On a thread of its own (its NAS calls under `nasread`'s deadlines, never the
/// agent's loop's), looked at (`Former::look`) with the root `root` gives (None: the NAS away)
/// every `every`, a look that went wrong doubling it up to `FORMER_BACKOFF_MAX`, until the file
/// is gone: removed once held a `settle` after its pack. The agent's leftovers' sweep leaves it
/// (room::is_member_leftover). What it archived said in the log, its warnings each time they
/// change. Called once a process: as a member's agent starts, or as one started not knowing its
/// role learns it is a member (agent::Agent::declared_role).
pub fn spawn_former(
    root: impl Fn() -> Option<store::nasfs::NasRoot> + Send + 'static,
    local: std::path::PathBuf,
    member: String,
    every: Duration,
    settle: Duration,
) {
    if !local.exists() {
        return;
    }
    let mut former = Former {
        local,
        member,
        settle,
        packed: None,
        #[cfg(test)]
        before_remove: None,
    };
    let packing = move || {
        let (mut wait, mut said) = (every, None::<Vec<String>>);
        loop {
            let r = root();
            let (gone, rep) = former.look(r.as_ref(), std::time::Instant::now());
            if rep.days_archived > 0 {
                eprintln!("agent: a former lead's history archived: {} days", rep.days_archived);
            }
            let now_said = if r.is_none() { vec!["history waits for the NAS".to_string()] } else { rep.warnings };
            if said.as_ref() != Some(&now_said) {
                for w in &now_said {
                    eprintln!("agent: a former lead's {w}");
                }
                said = Some(now_said);
            }
            if gone {
                break;
            }
            wait = backoff(wait, every, r.is_some() && said.as_ref().is_some_and(|w| !w.is_empty()));
            std::thread::sleep(wait);
        }
    };
    if let Err(e) = std::thread::Builder::new().name("history archive".into()).spawn(packing) {
        eprintln!("agent: a former lead's history not archived: {e}");
    }
}

/// The wait after a look (`spawn_former`): `every` after one that went well (or found the NAS
/// away), else twice `wait`, at most `FORMER_BACKOFF_MAX`.
fn backoff(wait: Duration, every: Duration, wrong: bool) -> Duration {
    if wrong {
        (wait * 2).min(FORMER_BACKOFF_MAX.max(every))
    } else {
        every
    }
}

/// A former lead's history file (`local`) on its way into the archive under `member`
/// (`spawn_former`): its contents as last packed, and when.
struct Former {
    local: std::path::PathBuf,
    member: String,
    settle: Duration,
    packed: Option<(std::time::Instant, Vec<u8>)>,
    /// A test's write to the file between its days found held and its removal.
    #[cfg(test)]
    before_remove: Option<fn(&Path)>,
}

impl Former {
    /// A look at `now`, the NAS at `root` (None: away, nothing asked of it). The file as it was
    /// last packed and `settle` since: each day's tar read for its lines, and the file removed if
    /// all hold them; any not, packed again. Packed, less than `settle` since: nothing. Not packed,
    /// or changed since: packed, its time and contents kept. Whether the file is gone; what it did.
    fn look(&mut self, root: Option<&store::nasfs::NasRoot>, now: std::time::Instant) -> (bool, Report) {
        let mut rep = Report::default();
        let read = match std::fs::read(&self.local) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return (true, rep),
            Err(e) => {
                rep.warnings.push(format!("history not read: {e}"));
                return (false, rep);
            }
        };
        let Some(root) = root else { return (false, rep) };
        let days = days_in(&String::from_utf8_lossy(&read), None);
        if let Some((at, b)) = &self.packed {
            if *b == read && now.duration_since(*at) < self.settle {
                return (false, rep);
            }
            if *b == read && held(root, &days, &self.member, &mut rep) {
                // (Lines written since the read, by a lead's process still handing over: kept.)
                #[cfg(test)]
                if let Some(f) = self.before_remove {
                    f(&self.local);
                }
                if std::fs::read(&self.local).ok().as_ref() != Some(&read) {
                    self.packed = None;
                    return (false, rep);
                }
                return match std::fs::remove_file(&self.local) {
                    Ok(()) => (true, rep),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (true, rep),
                    Err(e) => {
                        rep.warnings.push(format!("history held, not removed: {e}"));
                        (false, rep)
                    }
                };
            }
        }
        // (Settled from when the pack's last rename landed, not from the look's start: a week of
        // days on a slow NAS takes a while.)
        let began = std::time::Instant::now();
        let packed = pack(root, days, &self.member, false, &|| {}, &mut rep);
        self.packed = packed.then(|| (now + began.elapsed(), read));
        (false, rep)
    }
}

/// Whether every day of `days` (each day's lines) is held in its day's tar under `member`
/// (`holds`); a tar not read said in `rep`.
fn held(
    root: &store::nasfs::NasRoot,
    days: &std::collections::BTreeMap<String, Vec<u8>>,
    member: &str,
    rep: &mut Report,
) -> bool {
    let arch = root.join(ARCHIVE);
    days.iter().all(|(day, b)| {
        let tar = arch.join(format!("{day}.tar"));
        match nasread::read(&tar).map_err(anyhow::Error::from).and_then(|t| list_tar(&t)) {
            Ok(t) => holds(&t, &format!("{day}/{member}.jsonl"), b),
            Err(e) => {
                rep.warnings.push(format!("history {day} not read back: {e:#}"));
                false
            }
        }
    })
}

/// The lines of history file `f` by day, the days before `before` (None: every day); none where
/// there's no file.
fn days_of(f: &Path, before: Option<&str>) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
    match std::fs::read_to_string(f) {
        Ok(t) => Ok(days_in(&t, before)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", f.display())),
    }
}

/// `text`'s history lines by day, the days before `before` (None: every day); a line that isn't an
/// event left out.
fn days_in(text: &str, before: Option<&str>) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut days: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
    for l in text.lines() {
        let Ok(e) = serde_json::from_str::<crate::coord::history::Event>(l) else { continue };
        let Some(day) = journal::day(e.t).filter(|d| before.is_none_or(|b| d.as_str() < b)) else { continue };
        let b = days.entry(day).or_default();
        b.extend_from_slice(l.as_bytes());
        b.push(b'\n');
    }
    days
}

/// `days` (each day's lines) merged into their days' tars under `member`, the writer's; `between`
/// before each day. Whether every day's was merged (`merge_into_tar`).
fn pack(
    root: &store::nasfs::NasRoot,
    days: std::collections::BTreeMap<String, Vec<u8>>,
    member: &str,
    dry_run: bool,
    between: &dyn Fn(),
    rep: &mut Report,
) -> bool {
    let arch = root.join(ARCHIVE);
    if !dry_run {
        sweep_tmp(&arch, rep);
    }
    let mut all = true;
    for (day, b) in days {
        between();
        if dry_run {
            continue;
        }
        let tar = arch.join(format!("{day}.tar"));
        match merge_into_tar(&tar, vec![(format!("{day}/{member}.jsonl"), b)], merge_lines) {
            Ok(true) => rep.days_archived += 1,
            Ok(false) => {}
            Err(e) => {
                all = false;
                rep.warnings.push(format!("history {day} not archived: {e:#}"));
            }
        }
    }
    all
}

/// Whether `tar`'s member `name` holds each of `lines`' lines.
fn holds(tar: &[(String, Vec<u8>)], name: &str, lines: &[u8]) -> bool {
    let Some((_, have)) = tar.iter().find(|(n, _)| n == name) else { return lines.is_empty() };
    let held: std::collections::HashSet<&[u8]> = have.split(|&c| c == b'\n').collect();
    lines.split(|&c| c == b'\n').filter(|l| !l.is_empty()).all(|l| held.contains(l))
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

/// How often `merge_into_tar` merges again when the tar changed under it.
const MERGE_TRIES: usize = 3;

/// `members` merged into the tar at `tar_path` (`merge`; made when there's none): what a tar
/// already there holds (an earlier run's, or a run stopped before its files' removals) is kept,
/// never written over. A tar that can't be read is never replaced: an error,
/// the caller's files kept. Built in memory, written in one stream to a temporary name (one fsync),
/// read back and checked, and the tar read again just before the rename: changed since its read
/// (another writer's merge renamed in meanwhile), it is merged again from what is there now,
/// `MERGE_TRIES` times at most, then an error; so a writer frozen between its read and its rename
/// (a job SIGSTOPped by the memory guard, a lead's laptop asleep) can't put back a tar without
/// another's lines, but in the moment between that last read and its rename. Whether it was
/// written (false: it held them all).
pub(crate) fn merge_into_tar(
    tar_path: &NasPath,
    members: Vec<(String, Vec<u8>)>,
    merge: fn(&[(String, Vec<u8>)], Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)>,
) -> Result<bool> {
    let read = || -> Result<Vec<(String, Vec<u8>)>> {
        match nasread::read(tar_path) {
            Ok(t) => list_tar(&t).with_context(|| format!("{} unreadable, left as it is", tar_path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e).with_context(|| format!("read {}", tar_path.display())),
        }
    };
    for _ in 0..MERGE_TRIES {
        let old = read()?;
        let packed = merge(&old, members.clone());
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
            #[cfg(test)]
            if let Some(f) = BEFORE_RENAME.with(|h| h.take()) {
                f(tar_path);
            }
            if read()? != old {
                return Ok(false);
            }
            tmp.rename_over(tar_path)?;
            Ok(true)
        });
        match r {
            Ok(true) => return Ok(true),
            Ok(false) => {
                let _ = tmp.remove_file();
            }
            Err(e) => {
                let _ = tmp.remove_file();
                return Err(e);
            }
        }
    }
    anyhow::bail!("{} changed under each of {MERGE_TRIES} merges", tar_path.display())
}

#[cfg(test)]
thread_local! {
    /// A test's write to a tar between `merge_into_tar`'s read and its rename, once.
    static BEFORE_RENAME: std::cell::Cell<Option<fn(&NasPath)>> = const { std::cell::Cell::new(None) };
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

    /// The settling time the tests' `Former`s wait (`Former::look`'s `settle`).
    const SETTLE: Duration = Duration::from_secs(600);
    /// Past `SETTLE` by more than a pack takes here: a pack's settle starts as it returns.
    const LATER: Duration = Duration::from_secs(1);

    /// A `Former` of file `f` under member `m-2`, nothing packed yet.
    fn former(f: &Path) -> Former {
        Former { local: f.to_path_buf(), member: "m-2".into(), settle: SETTLE, packed: None, before_remove: None }
    }

    /// Task #1442: a former lead's history (left on its disk, the declared lead another Mac now)
    /// packed whole under its own member, the day it handed over (today) too, beside the new lead's
    /// member in the day's tar; looked at again before `settle`, nothing; after it, both tars hold
    /// it, so the file is removed; again, nothing.
    #[test]
    fn a_former_leads_history_is_packed_whole_under_its_member() {
        let r = tempfile::tempdir().unwrap();
        let one = NOW - 86_400;
        let (root, f) = lead_and_history(r.path(), &[(1, one), (2, NOW)]);
        let theirs = vec![(format!("{}/m-1.jsonl", journal::day(NOW).unwrap()), ev(9, NOW).into_bytes())];
        merge_into_tar(&root.join(ARCHIVE).join(format!("{}.tar", journal::day(NOW).unwrap())), theirs, merge_lines)
            .unwrap();
        let (mut fm, t0) = (former(&f), std::time::Instant::now());
        let (gone, rep) = fm.look(Some(&root), t0);
        assert!(!gone && f.exists(), "packed, kept until it settles");
        assert_eq!((rep.days_archived, rep.warnings.len()), (2, 0), "{rep:?}");
        let tar = |t: u64| list_tar(&std::fs::read(tar_of(r.path(), t)).unwrap()).unwrap();
        let day = |t: u64| journal::day(t).unwrap();
        assert_eq!(tar(one), [(format!("{}/m-2.jsonl", day(one)), ev(1, one).into_bytes())]);
        assert_eq!(
            tar(NOW),
            [
                (format!("{}/m-1.jsonl", day(NOW)), ev(9, NOW).into_bytes()),
                (format!("{}/m-2.jsonl", day(NOW)), ev(2, NOW).into_bytes())
            ],
            "the day it handed over, beside the new lead's"
        );
        assert_eq!(fm.look(Some(&root), t0 + SETTLE / 2), (false, Report::default()), "not settled: nothing");
        let (gone, rep) = fm.look(Some(&root), t0 + (SETTLE + LATER));
        assert!(gone && !f.exists(), "held once settled: removed, {rep:?}");
        assert_eq!(rep.days_archived, 0, "nothing written again");
        assert_eq!(fm.look(Some(&root), t0 + 2 * (SETTLE + LATER)), (true, Report::default()), "no file: nothing");
    }

    /// Task #1442 review: a former lead's history stays on its disk until every day's tar holds it:
    /// the NAS away, nothing tried; a day's tar unreadable, the other day packed and the file kept,
    /// said, and nothing counted as packed; that tar mended, the file packed, and once settled
    /// removed.
    #[test]
    fn a_former_leads_history_stays_until_packed() {
        let r = tempfile::tempdir().unwrap();
        let one = NOW - 86_400;
        let (root, f) = lead_and_history(r.path(), &[(1, one), (2, NOW)]);
        let (mut fm, t0) = (former(&f), std::time::Instant::now());
        assert_eq!(fm.look(None, t0), (false, Report::default()));
        assert!(f.exists() && !r.path().join(ARCHIVE).exists(), "the NAS away: kept, nothing written");
        let bad = tar_of(r.path(), one);
        std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
        std::fs::write(&bad, b"cut short").unwrap();
        let (gone, rep) = fm.look(Some(&root), t0);
        assert!(!gone && f.exists(), "a day not packed: kept");
        assert_eq!((rep.days_archived, rep.warnings.len()), (1, 1), "{rep:?}");
        let (gone, rep) = fm.look(Some(&root), t0 + (SETTLE + LATER));
        assert!(!gone && f.exists() && rep.warnings.len() == 1, "not packed, never removed: {rep:?}");
        std::fs::remove_file(&bad).unwrap();
        let (gone, rep) = fm.look(Some(&root), t0 + 2 * (SETTLE + LATER));
        assert!(!gone && rep.days_archived == 1, "the day left packed, the other held already: {rep:?}");
        assert!(fm.look(Some(&root), t0 + 3 * (SETTLE + LATER)).0 && !f.exists());
    }

    /// Task #1442 review: another writer's merge of a day landing over this one's after its pack
    /// (its tar read before this one's rename, renamed after) drops this one's lines from the
    /// tar: the look once settled finds them missing and packs them again, the file kept, and the
    /// next, once settled, removes it.
    #[test]
    fn lines_another_writer_drops_are_packed_again_before_the_file_goes() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW)]);
        let (mut fm, t0) = (former(&f), std::time::Instant::now());
        assert_eq!(fm.look(Some(&root), t0).1.days_archived, 1);
        let theirs = vec![(format!("{}/m-1.jsonl", journal::day(NOW).unwrap()), ev(9, NOW).into_bytes())];
        std::fs::write(tar_of(r.path(), NOW), codecs::tar::build(&theirs).unwrap()).unwrap();
        let (gone, rep) = fm.look(Some(&root), t0 + (SETTLE + LATER));
        assert!(!gone && f.exists() && rep.days_archived == 1, "missing: packed again, kept: {rep:?}");
        assert!(fm.look(Some(&root), t0 + 2 * (SETTLE + LATER)).0 && !f.exists());
        let t = list_tar(&std::fs::read(tar_of(r.path(), NOW)).unwrap()).unwrap();
        assert!(holds(&t, &format!("{}/m-2.jsonl", journal::day(NOW).unwrap()), ev(1, NOW).as_bytes()), "{t:?}");
    }

    /// Task #1442 review: a line written to the file after its pack (a lead's process still
    /// handing over) keeps it: the look once settled packs it, and the next removes the file, the
    /// line in the archive.
    #[test]
    fn a_file_written_since_its_pack_is_packed_again_not_removed() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW)]);
        let (mut fm, t0) = (former(&f), std::time::Instant::now());
        fm.look(Some(&root), t0);
        std::fs::write(&f, ev(1, NOW) + &ev(3, NOW)).unwrap();
        let (gone, rep) = fm.look(Some(&root), t0 + (SETTLE + LATER));
        assert!(!gone && f.exists() && rep.days_archived == 1, "written since: packed again, kept: {rep:?}");
        assert!(fm.look(Some(&root), t0 + 2 * (SETTLE + LATER)).0 && !f.exists());
        let t = list_tar(&std::fs::read(tar_of(r.path(), NOW)).unwrap()).unwrap();
        assert!(holds(&t, &format!("{}/m-2.jsonl", journal::day(NOW).unwrap()), ev(3, NOW).as_bytes()), "{t:?}");
    }

    /// Task #1442 review: a line written while the look reads the tars back (after the file's
    /// read, before its removal) keeps the file, packed at the next look.
    #[test]
    fn a_file_written_while_its_days_are_read_back_is_kept() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW)]);
        let (mut fm, t0) = (former(&f), std::time::Instant::now());
        fm.look(Some(&root), t0);
        fm.before_remove = Some(|f| {
            let mut b = std::fs::read(f).unwrap();
            b.extend_from_slice(ev(4, NOW).as_bytes());
            std::fs::write(f, b).unwrap();
        });
        let (gone, _) = fm.look(Some(&root), t0 + (SETTLE + LATER));
        assert!(!gone && f.exists(), "written since its read: kept");
        fm.before_remove = None;
        assert_eq!(fm.look(Some(&root), t0 + (SETTLE + LATER)).1.days_archived, 1, "the new line packed");
    }

    /// Task #1442 review round 3: another writer's tar renamed in between a merge's read and its
    /// rename (a writer frozen in between) is read again just before the rename and merged with:
    /// both members' lines in the tar, neither lost.
    #[test]
    fn a_tar_changed_under_a_merge_is_merged_again() {
        let r = tempfile::tempdir().unwrap();
        let (root, _) = lead_and_history(r.path(), &[]);
        let tar = root.join(ARCHIVE).join("d.tar");
        merge_into_tar(&tar, vec![("d/m-1.jsonl".into(), b"a\n".to_vec())], merge_lines).unwrap();
        BEFORE_RENAME.with(|h| {
            h.set(Some(|t| {
                let theirs = vec![("d/m-1.jsonl".to_string(), b"a\nb\n".to_vec())];
                std::fs::write(t.unchecked_path(), codecs::tar::build(&theirs).unwrap()).unwrap();
            }))
        });
        assert!(merge_into_tar(&tar, vec![("d/m-2.jsonl".into(), b"x\n".to_vec())], merge_lines).unwrap());
        let t = list_tar(&std::fs::read(r.path().join(ARCHIVE).join("d.tar")).unwrap()).unwrap();
        assert_eq!(
            t,
            [("d/m-1.jsonl".to_string(), b"a\nb\n".to_vec()), ("d/m-2.jsonl".to_string(), b"x\n".to_vec())],
            "the other writer's line and ours"
        );
    }

    /// Task #1442 review round 3: a file rewritten to a subset of what was packed (lines gone from
    /// it) is no file as packed: a look before the pack has settled keeps it, though every line
    /// it holds is in the archive, and packs it as it is.
    #[test]
    fn a_file_rewritten_since_its_pack_is_kept_before_the_settle() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW), (2, NOW)]);
        let (mut fm, t0) = (former(&f), std::time::Instant::now());
        fm.look(Some(&root), t0);
        std::fs::write(&f, ev(1, NOW)).unwrap();
        let (gone, _) = fm.look(Some(&root), t0 + SETTLE / 2);
        assert!(!gone && f.exists(), "changed since its pack: kept until it settles again");
        assert!(fm.look(Some(&root), t0 + SETTLE / 2 + SETTLE + LATER).0 && !f.exists());
    }

    /// Task #1442 review: a look that went wrong doubles the wait, at most `FORMER_BACKOFF_MAX`;
    /// one that went well goes back to `every`.
    #[test]
    fn a_look_gone_wrong_backs_off_up_to_an_hour() {
        let every = FORMER_EVERY;
        let mut w = every;
        for _ in 0..20 {
            w = backoff(w, every, true);
        }
        assert_eq!(w, FORMER_BACKOFF_MAX);
        assert_eq!(backoff(every, every, true), 2 * every);
        assert_eq!(backoff(w, every, false), every);
    }

    /// Task #1442 review: `spawn_former` tries again every `every` while the NAS is away, and packs
    /// and removes the file once it is found and settled.
    #[test]
    fn a_former_leads_history_is_packed_once_the_nas_is_found() {
        let r = tempfile::tempdir().unwrap();
        let (root, f) = lead_and_history(r.path(), &[(1, NOW)]);
        let found = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (seen, tries) = (found.clone(), std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let n = tries.clone();
        spawn_former(
            move || {
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                seen.load(std::sync::atomic::Ordering::SeqCst).then(|| root.clone())
            },
            f.clone(),
            "m-2".into(),
            Duration::from_millis(5),
            Duration::from_millis(20),
        );
        store::testkit::eventually("tried again while the NAS is away", || {
            tries.load(std::sync::atomic::Ordering::SeqCst) >= 3
        });
        assert!(f.exists());
        found.store(true, std::sync::atomic::Ordering::SeqCst);
        store::testkit::eventually("packed and removed", || !f.exists());
        assert!(tar_of(r.path(), NOW).exists());
    }

    /// Task #1442 review: `holds` reads a member for every line of the writer's, whatever else it
    /// holds; a tar another writer replaced without them doesn't.
    #[test]
    fn a_tar_holds_a_writers_lines_only_when_each_is_there() {
        let tar = vec![("d/m-2.jsonl".to_string(), b"a\nb\nc\n".to_vec())];
        assert!(holds(&tar, "d/m-2.jsonl", b"a\nc\n"));
        assert!(!holds(&tar, "d/m-2.jsonl", b"a\nd\n"));
        assert!(!holds(&[("d/m-1.jsonl".to_string(), b"a\n".to_vec())], "d/m-2.jsonl", b"a\n"));
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
