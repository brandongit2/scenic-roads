//! Resuming, and planning from the records alone (docs/pool.md §7.2 Resuming, §7.4; §12 phase 4's
//! eighth batch), behind the switch `state/pool/resume` (`SWITCH`; off while missing or `off` in
//! it: the agent as it was, exactly).
//!
//! - **A job's progress on its Mac's disk, named on the NAS.** A resumable job (`RESUMABLE`: the
//!   OSM pass, whose stages its scratch folder marks) noted as it starts and as it ends in its
//!   member's progress file, `state/pool/progress/<member>.json` (`path`): written whole by that
//!   member alone (principle 3: one file a member, as its heartbeat, rather than one a step that
//!   two Macs could both write), only when what it holds changes (a job's start or end, an entry
//!   dropped), never in a loop. The lead reads the files of the members it knows (by id, never a
//!   listing) as it plans.
//! - **Offered to that Mac alone, for 24 hours or until the owner releases it** (`decide`): while a
//!   member's file holds the job (`HOLD_S` from when its job last started or ended there, and no
//!   release of that very progress), the lead doesn't run it itself, and offers it to that member
//!   alone (crate::coord's `offer_resume`); a member asks for it only while its own file holds it
//!   live (`asks`). A job its Mac finished is held the same way, unoffered, until its pass is in the
//!   records (its entry merged), so a pass done on one Mac isn't begun again on another meanwhile.
//!   The owner's release (`release`, `scenic pool release <member> <job>`) is a file made once
//!   with create-new beside the member's (`release_path`, naming the progress it releases by its
//!   time), never a write to the member's own file.
//! - **The job sees its earlier attempts:** its agent lays this member's hand-offs for the job's
//!   step that its lead hasn't acknowledged (unwritten, or written and not yet in the current term's
//!   records) into a file of the job's folder (`unmerged`), named to the job by `UNMERGED_ENV`, which
//!   crate::out::Out lays over the snapshot as it opens: a pass restarted after sleep finds its
//!   earlier stages' outputs before the lead has merged them.
//! - **A pass complete by its records:** `sources/osm/<date>/pass` in the merged records (today's
//!   manifest, which the lead writes from them), not a `pass.*.json` file on the NAS
//!   (crate::osmpass::latest_pass_by), the agent's planning and its jobs' alike (`RESUME_ENV` in
//!   a job's environment).

use crate::handoff::Handoff;
use crate::pool::journal::Entry;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The switch (`on` in it, or present; `off` in it, or missing: off, the agent as it was).
pub const SWITCH: &str = "state/pool/resume";

/// Whether resuming is on, as the switch at `root` says; None when it can't be read now.
pub fn on(root: &store::nasfs::NasRoot) -> Option<bool> {
    match super::nasread::read_switch(&root.join(SWITCH)) {
        Ok(s) => Some(s.trim() != "off"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// The members' progress files' folder.
pub const DIR: &str = "state/pool/progress";
/// How long a job's progress is offered to its Mac alone, from when its job last started or ended
/// there (§7.2: 24 hours).
pub const HOLD_S: u64 = 24 * 3600;
/// A member's progress noted this recently and not finished may be its job running now (a lease
/// granted less than a loop before a lead was lost isn't in the state the next lead restores): the
/// lead doesn't run the job itself meanwhile (`running_elsewhere`). Its agent notes it again every
/// `REFRESH_S` while the job runs.
pub const FRESH_S: u64 = 2 * crate::coord::TTL.as_secs();
/// How often a running resumable job's `at` is noted again (half a lease's time to live).
pub const REFRESH_S: u64 = crate::coord::TTL.as_secs() / 2;
/// The steps whose jobs resume from their progress on their Mac's disk, offered back to it: the OSM
/// pass (its stages' markers in its scratch folder, crate::osmpass). (A depth, terrain-hi or
/// terrain-deep target's run resumes from its blocks' log when its Mac runs it again,
/// crate::resume_log, not offered back.) (A terrain z3 tile's raw tiles, the buildings scan's parts
/// and a rail-feeds run's zips are planned: their jobs are a shared step's, offered by target, and
/// start their targets afresh.)
pub const RESUMABLE: [&str; 1] = ["osm-pass"];
/// A job's environment: the file of its member's unacknowledged hand-offs for its step
/// (`unmerged`), which crate::out::Out lays over the snapshot.
pub const UNMERGED_ENV: &str = "SCENIC_UNMERGED";
/// A job's environment: a pass is complete by the records (crate::osmpass::latest_pass).
pub const RESUME_ENV: &str = crate::osmpass::RESUME_ENV;

/// Whether `job` (an agent's job id, `<step> <target>`) is of a resumable step.
pub fn resumable(job: &str) -> bool {
    job.split(' ').next().is_some_and(|s| RESUMABLE.contains(&s))
}

/// A member's progress file: the jobs whose progress is on its Mac's disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Its member.
    pub member: String,
    /// The name its agent asks the lead's coordinator for work by (its host name).
    pub worker: String,
    /// By job id (`osm-pass <date>`).
    #[serde(default)]
    pub jobs: BTreeMap<String, Held>,
}

/// A job's progress on a member's Mac.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// When its job last started or ended there (unix seconds, its member's clock).
    pub at: u64,
    /// When this progress was first noted there (its job's first start): offered to it alone until
    /// `HOLD_S` after, however often its job starts, fails or stops there meanwhile (§7.2's day).
    /// 0 (a file without it): `at`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub since: u64,
    /// Its job finished there: nothing left to run, its entry waiting to be merged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
    /// Its scratch folder on that Mac (for the owner).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scratch: String,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Held {
    /// When this progress began (`since`, else `at`): what its day and a release are reckoned by.
    pub fn began(&self) -> u64 {
        if self.since == 0 {
            self.at
        } else {
            self.since
        }
    }
}

/// Member `member`'s progress file.
pub fn path(member: &str) -> String {
    format!("{DIR}/{member}.json")
}

/// The owner's release of member `member`'s progress of `job` that began at `at` (`Held::began`):
/// made once, never changed (progress begun there later is its own, offered to it again).
pub fn release_path(member: &str, job: &str, at: u64) -> String {
    format!("{DIR}/released/{member}/{}-{at}", job.replace([' ', '/'], "_"))
}

/// Member `member`'s progress file at `root`: none when there's none; an error when it can't be
/// read now, or isn't one.
pub fn read(root: &store::nasfs::NasRoot, member: &str) -> Result<Option<Progress>> {
    let p = root.join(path(member));
    match super::nasread::read(&p) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    }
}

/// Writes this member's progress file whole (its own alone).
pub fn write(root: &store::nasfs::NasRoot, p: &Progress) -> Result<()> {
    root.join(DIR).create_dir_all()?;
    root.join(path(&p.member)).write_whole(serde_json::to_vec_pretty(p)?)
}

/// Whether the owner released `member`'s progress of `job` that began at `at`.
pub fn released(root: &store::nasfs::NasRoot, member: &str, job: &str, at: u64) -> bool {
    root.join(release_path(member, job, at)).exists()
}

/// The owner's release of `member`'s progress of `job` (`scenic pool release`): the lead runs it
/// wherever it fits from then. What it released, or why nothing.
pub fn release(root: &store::nasfs::NasRoot, member: &str, job: &str) -> Result<String> {
    let p = read(root, member)?.with_context(|| format!("{member} has no progress file"))?;
    let h = p.jobs.get(job).with_context(|| {
        format!("{member}'s progress holds no {job:?} (it holds {:?})", p.jobs.keys().collect::<Vec<_>>())
    })?;
    let r = root.join(release_path(member, job, h.began()));
    r.parent().unwrap().create_dir_all()?;
    match r.open_with(std::fs::OpenOptions::new().write(true).create_new(true)) {
        Ok(_) => Ok(format!("{job}'s progress on {member} ({}) released: the lead runs it wherever it fits", p.worker)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(format!("{job}'s progress on {member} was released already"))
        }
        Err(e) => Err(e).with_context(|| format!("make {}", r.display())),
    }
}

/// Whether progress `h` still holds its job for its Mac at `now` (released: `released`).
pub fn live(h: &Held, now: u64, released: bool) -> bool {
    !released && now < h.began().saturating_add(HOLD_S)
}

/// What the lead does with job `job` (its plan has it), by the members' progress files `files` it
/// read, as of `now`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// It runs it where it would have (no member holds it, or the lead itself does).
    Plan,
    /// It offers it to that member alone (`worker`, the name its asks go by), until `until`.
    Offer { member: String, worker: String, until: u64 },
    /// Neither: finished on a member, its entry waiting to be merged; why.
    Wait(String),
}

/// `Decision` for job `job` on the lead, member `me`: the lead's own live progress first (it runs
/// it, or waits for its finished entry); else the member's whose progress is the newest live one;
/// else as before. `released(member, at)`: the owner released that progress.
pub fn decide(job: &str, me: &str, files: &[Progress], now: u64, released: &dyn Fn(&str, u64) -> bool) -> Decision {
    let holds = |p: &Progress| p.jobs.get(job).filter(|h| live(h, now, released(&p.member, h.began()))).cloned();
    if let Some(h) = files.iter().filter(|p| p.member == me).find_map(holds) {
        return if h.done {
            Decision::Wait(format!(
                "finished on this Mac, its entry waiting to be merged (until {})",
                h.began() + HOLD_S
            ))
        } else {
            Decision::Plan
        };
    }
    let newest =
        files.iter().filter(|p| p.member != me).filter_map(|p| holds(p).map(|h| (p, h))).max_by_key(|(_, h)| h.at);
    match newest {
        Some((p, h)) if h.done => {
            Decision::Wait(format!("finished on {} ({}), its entry waiting to be merged", p.worker, p.member))
        }
        Some((p, h)) => {
            Decision::Offer { member: p.member.clone(), worker: p.worker.clone(), until: h.began() + HOLD_S }
        }
        None => Decision::Plan,
    }
}

/// Another member whose file shows job `job` not finished and noted within `FRESH_S` of `now`: it
/// may be running it now, so the lead doesn't run it too.
pub fn running_elsewhere<'a>(job: &str, me: &str, files: &'a [Progress], now: u64) -> Option<&'a Progress> {
    files
        .iter()
        .filter(|p| p.member != me)
        .find(|p| p.jobs.get(job).is_some_and(|h| !h.done && now < h.at.saturating_add(FRESH_S)))
}

/// The jobs member file `p` holds for its own Mac to ask for at `now` (live, not finished).
pub fn asks(p: &Progress, now: u64, released: &dyn Fn(&str, u64) -> bool) -> Vec<String> {
    p.jobs.iter().filter(|(j, h)| !h.done && live(h, now, released(j, h.began()))).map(|(j, _)| j.clone()).collect()
}

/// This member's written entries of `step` its lead hasn't acknowledged (`keys`), read from the
/// journal: one a lead refused (its why noted beside the journal: crate::pool::journal::refusal)
/// left out, as is one that can't be read now.
pub fn unacked(nas: &dyn crate::pool::nas::Nas, step: &str, keys: &[String]) -> Vec<Entry> {
    let mut out = Vec::new();
    for key in keys {
        let Ok(crate::pool::journal::Read::Entry(e)) = crate::pool::journal::read(nas, key) else { continue };
        if e.step != step || !matches!(crate::pool::journal::refusal(nas, key), Ok(None)) {
            continue;
        }
        out.push(e);
    }
    out
}

/// This member's entries for `step`, in lease order, laid one over another: what its jobs of
/// that step handed off that its lead hasn't acknowledged (`Out` lays it over the snapshot).
pub fn unmerged<'a>(step: &str, entries: impl IntoIterator<Item = &'a Entry>) -> Handoff {
    let mut es: Vec<&Entry> = entries.into_iter().filter(|e| e.step == step).collect();
    es.sort_by_key(|e| e.lease);
    es.dedup_by_key(|e| e.lease);
    let mut h = Handoff::default();
    for e in es {
        h.absorb(e.handoff.clone());
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::journal::LeaseId;

    fn file(member: &str, worker: &str, jobs: &[(&str, u64, bool)]) -> Progress {
        Progress {
            member: member.into(),
            worker: worker.into(),
            jobs: jobs
                .iter()
                .map(|(j, at, done)| (j.to_string(), Held { at: *at, done: *done, ..Default::default() }))
                .collect(),
        }
    }

    const PASS: &str = "osm-pass 2026-10-01";
    const T: u64 = 1_800_000_000;

    #[test]
    fn a_job_held_by_a_member_is_offered_to_it_alone_for_a_day() {
        let none = |_: &str, _: u64| false;
        // No one holds it: the lead runs it, as before.
        assert_eq!(decide(PASS, "m-lead", &[], T, &none), Decision::Plan);
        assert_eq!(
            decide(PASS, "m-lead", &[file("m-b", "mac-b", &[("osm-pass 2026-09-01", T, false)])], T, &none),
            Decision::Plan
        );
        // A member's: offered to it alone, until a day after its job last ran there.
        let b = file("m-b", "mac-b", &[(PASS, T, false)]);
        assert_eq!(
            decide(PASS, "m-lead", std::slice::from_ref(&b), T + 3600, &none),
            Decision::Offer { member: "m-b".into(), worker: "mac-b".into(), until: T + HOLD_S }
        );
        assert_eq!(
            decide(PASS, "m-lead", std::slice::from_ref(&b), T + HOLD_S - 1, &none),
            Decision::Offer { member: "m-b".into(), worker: "mac-b".into(), until: T + HOLD_S }
        );
        // A day on, or released by the owner: the lead's to place again.
        assert_eq!(decide(PASS, "m-lead", std::slice::from_ref(&b), T + HOLD_S, &none), Decision::Plan);
        let rel = |m: &str, at: u64| m == "m-b" && at == T;
        assert_eq!(decide(PASS, "m-lead", std::slice::from_ref(&b), T + 60, &rel), Decision::Plan);
        // (A release is of that progress: a later start there is offered again.)
        let later = file("m-b", "mac-b", &[(PASS, T + 600, false)]);
        assert!(matches!(decide(PASS, "m-lead", &[later], T + 700, &rel), Decision::Offer { .. }));
        // The lead's own progress first: it runs it.
        let mine = file("m-lead", "mac-a", &[(PASS, T, false)]);
        assert_eq!(decide(PASS, "m-lead", &[b.clone(), mine], T + 60, &none), Decision::Plan);
        // Two members': the newest.
        let c = file("m-c", "mac-c", &[(PASS, T + 10, false)]);
        assert!(
            matches!(decide(PASS, "m-lead", &[b.clone(), c], T + 60, &none), Decision::Offer { member, .. } if member == "m-c")
        );
        // Finished there: neither run nor offered until merged (or a day on).
        let done = file("m-b", "mac-b", &[(PASS, T, true)]);
        assert!(
            matches!(decide(PASS, "m-lead", std::slice::from_ref(&done), T + 60, &none), Decision::Wait(w) if w.contains("mac-b"))
        );
        assert_eq!(decide(PASS, "m-lead", &[done], T + HOLD_S + 1, &none), Decision::Plan);
        let mine_done = file("m-lead", "mac-a", &[(PASS, T, true)]);
        assert!(matches!(decide(PASS, "m-lead", &[mine_done], T + 60, &none), Decision::Wait(_)));
    }

    /// A pass that keeps stopping there holds its Mac only the day from when its progress began,
    /// however often its job starts again (`since`), and a release is of that progress.
    #[test]
    fn a_pass_failing_again_and_again_holds_its_mac_a_day_from_its_first_start() {
        let none = |_: &str, _: u64| false;
        let mut p = file("m-b", "mac-b", &[]);
        p.jobs.insert(PASS.into(), Held { at: T + HOLD_S - 60, since: T, done: false, scratch: String::new() });
        assert!(
            matches!(decide(PASS, "m-lead", std::slice::from_ref(&p), T + HOLD_S - 30, &none), Decision::Offer { until, .. } if until == T + HOLD_S)
        );
        assert_eq!(
            decide(PASS, "m-lead", std::slice::from_ref(&p), T + HOLD_S, &none),
            Decision::Plan,
            "its last failure a minute ago doesn't hold it past the day"
        );
        assert!(asks(&p, T + HOLD_S, &|_, _| false).is_empty());
        let rel = |m: &str, at: u64| m == "m-b" && at == T;
        assert_eq!(
            decide(PASS, "m-lead", std::slice::from_ref(&p), T + 60, &rel),
            Decision::Plan,
            "released by when it began"
        );
    }

    /// A member's progress noted lately, not finished: maybe running, whatever its day or a
    /// release says; older, or finished, or the lead's own: not.
    #[test]
    fn a_member_noted_lately_may_be_running_it() {
        let mut p = file("m-b", "mac-b", &[]);
        p.jobs.insert(PASS.into(), Held { at: T, since: T - HOLD_S, ..Default::default() });
        assert!(running_elsewhere(PASS, "m-lead", std::slice::from_ref(&p), T + FRESH_S - 1).is_some());
        assert!(running_elsewhere(PASS, "m-lead", std::slice::from_ref(&p), T + FRESH_S).is_none());
        assert!(running_elsewhere(PASS, "m-b", std::slice::from_ref(&p), T).is_none());
        p.jobs.get_mut(PASS).unwrap().done = true;
        assert!(running_elsewhere(PASS, "m-lead", &[p], T).is_none());
        assert!(REFRESH_S * 4 == FRESH_S);
    }

    /// The journal's entries a resumed job lays: its step's, a refused one left out.
    #[test]
    fn a_refused_entry_is_not_laid_over_the_snapshot() {
        let d = tempfile::tempdir().unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(d.path());
        let e = |n: u64, step: &str| Entry {
            member: "m-b".into(),
            lease: LeaseId { term: 2, n },
            step: step.into(),
            handoff: Handoff {
                changes: [(format!("sources/osm/d/x{n}"), Some("c".to_string()))].into(),
                ..Default::default()
            },
            at: T,
        };
        let keys: Vec<String> = [e(1, "osm-pass"), e(2, "osm-pass"), e(3, "terrain")]
            .iter()
            .map(|x| crate::pool::journal::write(&nas, x).unwrap())
            .collect();
        crate::pool::journal::note_refusal(&nas, &keys[1], "outside its step's write-set").unwrap();
        let mut missing = keys.clone();
        missing.push("2026-01-01/2-9".into());
        let got = unacked(&nas, "osm-pass", &missing);
        assert_eq!(got.iter().map(|e| e.lease.n).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_member_asks_only_for_what_its_own_file_holds_live() {
        let p = file(
            "m-b",
            "mac-b",
            &[(PASS, T, false), ("osm-pass 2026-09-01", T - HOLD_S, false), ("osm-pass 2026-08-01", T, true)],
        );
        assert_eq!(asks(&p, T + 1, &|_, _| false), vec![PASS.to_string()]);
        assert!(asks(&p, T + 1, &|j, at| j == PASS && at == T).is_empty());
        assert!(asks(&p, T + HOLD_S, &|_, _| false).is_empty());
        assert!(resumable(PASS) && !resumable("terrain 3/2/2") && !resumable("pass-sets 2026-10-01"));
    }

    #[test]
    fn progress_files_and_releases_on_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        assert!(read(&store::nasfs::NasRoot::local_for_test(r), "m-b").unwrap().is_none());
        let p = file("m-b", "mac-b", &[(PASS, T, false)]);
        write(&store::nasfs::NasRoot::local_for_test(r), &p).unwrap();
        assert_eq!(read(&store::nasfs::NasRoot::local_for_test(r), "m-b").unwrap(), Some(p.clone()));
        assert!(r.join("state/pool/progress/m-b.json").exists());
        assert!(!released(&store::nasfs::NasRoot::local_for_test(r), "m-b", PASS, T));
        assert!(release(&store::nasfs::NasRoot::local_for_test(r), "m-b", PASS).unwrap().contains("released"));
        assert!(
            released(&store::nasfs::NasRoot::local_for_test(r), "m-b", PASS, T)
                && !released(&store::nasfs::NasRoot::local_for_test(r), "m-b", PASS, T + 1)
        );
        assert!(release(&store::nasfs::NasRoot::local_for_test(r), "m-b", PASS).unwrap().contains("already"));
        assert!(
            release(&store::nasfs::NasRoot::local_for_test(r), "m-b", "osm-pass 2026-09-01").is_err()
                && release(&store::nasfs::NasRoot::local_for_test(r), "m-x", PASS).is_err()
        );
        std::fs::write(r.join(path("m-x")), "{").unwrap();
        assert!(
            read(&store::nasfs::NasRoot::local_for_test(r), "m-x").is_err(),
            "a file that isn't one is an error, not none"
        );
        // The switch.
        assert_eq!(on(&store::nasfs::NasRoot::local_for_test(r)), Some(false));
        std::fs::write(r.join(SWITCH), "").unwrap();
        assert_eq!(on(&store::nasfs::NasRoot::local_for_test(r)), Some(true));
        std::fs::write(r.join(SWITCH), "off\n").unwrap();
        assert_eq!(on(&store::nasfs::NasRoot::local_for_test(r)), Some(false));
    }

    #[test]
    fn unmerged_lays_a_steps_entries_in_lease_order() {
        let e = |n: u64, step: &str, l: &str, c: Option<&str>| Entry {
            member: "m-b".into(),
            lease: LeaseId { term: 2, n },
            step: step.into(),
            handoff: Handoff { changes: [(l.to_string(), c.map(str::to_string))].into(), ..Default::default() },
            at: T,
        };
        let es = [
            e(7, "osm-pass", "sources/osm/d/sets/rail", Some("b")),
            e(3, "osm-pass", "sources/osm/d/sets/rail", Some("a")),
            e(4, "osm-pass", "sources/osm/d/filtered", Some("f")),
            e(5, "terrain", "layers/terrain/x", Some("t")),
        ];
        let h = unmerged("osm-pass", &es);
        assert_eq!(h.changes.get("sources/osm/d/sets/rail"), Some(&Some("b".to_string())));
        assert_eq!(h.changes.get("sources/osm/d/filtered"), Some(&Some("f".to_string())));
        assert!(!h.changes.contains_key("layers/terrain/x"));
        assert!(unmerged("unit", &es).changes.is_empty());
    }
}
