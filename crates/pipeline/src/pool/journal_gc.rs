//! GC of the journal's old days (docs/pool.md §7.3, task #644): removing the entries of days over
//! a week old that every records a take-up may start from name.
//!
//! The journal is what makes a stale read harmless (crate::pool::records): a take-up starts from
//! the newest snapshot its reads find, which may be an older one (a stale read of the newest, or
//! none found and the term before's read instead), and replays every entry its records don't name.
//! An entry removed is replayed no more, so before any is removed every take-up must start from
//! records that name it. That's the fence: a marker, `state/journal/gc.json`, naming the entries to
//! remove (`Marker::keys`), the horizon they're before, and the horizon the records must have
//! forgotten up to already (`floor`, the last GC's). `records::start` refuses (for now: it's tried
//! again) to take up from records that don't name or forget every key it lists, or haven't the floor
//! (`Fenced`); one that fences for good is raised as an alert and lifted, where safe, by `plan`.
//!
//! Two runs of the daily `gc` job on the lead (off its loop and the pool's lock, its records
//! re-asserted and caught up first: crate::agent's sweeps), at least `WAIT_S` apart:
//!
//! 1. **Mark.** Once the last marker's entries are removed and the current term's snapshot has
//!    forgotten them: the days before `KEEP_DAYS` ago, up to the first day holding an entry that
//!    snapshot doesn't name (held, or not readable yet); every entry in them, all named by the
//!    current term's snapshot as read back from the share (only what's on the share counts: a
//!    save that failed, or whose answer was lost, counts for nothing). Written whole; nothing removed.
//! 2. **Remove**, `WAIT_S` later, so every Mac's reads see the marker before a file goes (a stale
//!    read of it would let a take-up pass the fence): only when every member's heartbeat says its
//!    app keeps the fence (`Beat::journal`: an app from before would take up from any records it
//!    reads, the days gone), the current term's snapshot (read back) still names or has forgotten
//!    every key, and the term hasn't changed; the marker's files, a day at a time, the marker read
//!    again before each, at most `MAX_REMOVES` a run (the share takes some 20 to 55 small-file
//!    operations a second: the rest waits for the next run). A day emptied, its folder too. Then
//!    marked done.
//!
//! The lead then forgets the marker's keys (`Records::forget`) at its next listing of every day
//! (crate::pool::driver), its snapshot's horizon the marker's: told of one again, it answers it as
//! merged (`Merged::forgotten`). An entry written into a removed day later (its member asleep with
//! it unwritten) isn't in the marker: it stays, and is merged as any.

use super::beat::Beat;
use super::journal;
use super::nas::Nas;
use super::records::Records;
use super::term;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The marker.
pub const PATH: &str = "state/journal/gc.json";
/// What a heartbeat's `journal` says when its app keeps the fence (`Beat::journal`).
pub const FENCE: u32 = 1;
/// The days kept, whatever the records name: a member back from a few days asleep writes and tells
/// of its entries under the days they were first tried.
pub const KEEP_DAYS: u64 = 7;
/// The least time between the marker written and its first file removed (s): far past any stale
/// read seen (40 minutes, 24 Oct), and short of the daily job's day, so the next run removes.
pub const WAIT_S: u64 = 12 * 3600;
/// The most entries removed in one run.
pub const MAX_REMOVES: usize = 3000;

/// The marker: the entries to remove, and what records must name to be taken up from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// The day the keys are before.
    pub horizon: String,
    /// The horizon records must have already (the last GC's): its keys are gone.
    #[serde(default)]
    pub floor: String,
    /// The term and snapshot that named every key when it was written.
    pub term: u64,
    pub seq: u64,
    /// When it was written (the lead's clock).
    pub at: u64,
    /// The entries to remove, by key.
    pub keys: BTreeSet<String>,
    /// Every one of them removed.
    #[serde(default)]
    pub done: bool,
}

impl Marker {
    /// Whether records `r` can be taken up from with the marker's entries gone: they name or have
    /// forgotten every one, and have forgotten the floor's.
    /// Whether journal entry `key` is one a GC removed, or may have: the marker's own, or of the
    /// days before its floor (an earlier GC's; a late entry of those days, missing from a read, is
    /// taken as removed: two GCs back, over two weeks asleep).
    pub fn removed(&self, key: &str) -> bool {
        self.keys.contains(key) || key < self.floor.as_str()
    }

    pub fn covers(&self, r: &Records) -> bool {
        r.horizon.as_str() >= self.floor.as_str() && self.keys.iter().all(|k| r.handles(k) || r.forgot(k))
    }
}

/// The marker; None when there's none. An error when it can't be read or parsed: a reader fails
/// closed.
pub fn read(nas: &dyn Nas) -> Result<Option<Marker>> {
    match nas.read(PATH)? {
        Some(b) => serde_json::from_slice(&b).map(Some).with_context(|| format!("parse {PATH}")),
        None => Ok(None),
    }
}

/// The owner's kill switch (`SWITCH`): the journal's GC runs unless it says `off`; missing, on.
pub const SWITCH: &str = "state/pool/journal-gc";

/// Whether the journal's GC runs: on unless the switch says `off`. Unreadable (an error): off this
/// run, as it deletes on the NAS.
pub fn enabled(nas: &dyn Nas) -> bool {
    match nas.read(SWITCH) {
        Ok(None) => true,
        Ok(Some(b)) => String::from_utf8_lossy(&b).trim() != "off",
        Err(_) => false,
    }
}

/// Why take-ups are refused by the marker (`fence`): its error, told apart from a failing read so
/// the lead's heartbeat can say so (`Beat::fenced`) and every view alert the owner
/// (crate::agent::lead::alert). Transient on a stale read; for good when the marker is corrupt or
/// the records never come to cover it: `scenic pool journal-gc` says which and lifts it when safe
/// (`plan`).
#[derive(Debug)]
pub struct Fenced(pub String);

impl std::fmt::Display for Fenced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fenced {}

/// Whether records `r` may be taken up from (the fence): an error saying why not, a `Fenced` when
/// it's the marker's doing (a failing read stays a plain error).
pub fn fence(nas: &dyn Nas, r: &Records) -> Result<()> {
    let Some(b) = nas.read(PATH)? else { return Ok(()) };
    let m: Marker = serde_json::from_slice(&b)
        .map_err(|e| Fenced(format!("the journal GC's marker ({PATH}) can't be parsed ({e})")))?;
    if m.covers(r) {
        return Ok(());
    }
    Err(Fenced(format!(
        "the journal's entries before {} are being removed and term {}'s records at {} don't name them all \
         (a stale read, or records older than the marker's: `scenic pool journal-gc` says which)",
        m.horizon, r.term, r.seq
    ))
    .into())
}

/// What `scenic pool journal-gc` finds (`plan`), and the marker that would lift the fence safely.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// How it stands, in words.
    pub said: String,
    /// The marker to write in its place: None when nothing is to be done or it isn't safe.
    pub write: Option<Marker>,
}

/// The marker checked against records `r` (the newest a take-up would start from), at `now`, and
/// the safe way out when they don't cover it (docs/pool.md §7.3):
/// - **Covered, or none:** nothing to do (a take-up refused meanwhile read stale).
/// - **Records short of the floor:** not safe: the last GC's entries are gone and these records
///   haven't forgotten them; restore newer records.
/// - **Entries the records don't name:** each still on the share is dropped from the marker (a
///   take-up replays it from the journal, so the fence needn't hold it); one gone isn't safe: it
///   was removed, and any take-up from these records loses it.
/// - **Corrupt:** made again from `r` as `mark` makes one, a superset of what it can have named:
///   every entry `r` names of the days before `KEEP_DAYS` ago (up to the first day holding one on
///   the share it doesn't name), on the share or not, its floor `r`'s horizon. Safe as long as `r`
///   is at least as new as the records the lost marker was made from (it was made from records
///   read back from the share, and `r` is the newest there now).
pub fn plan(nas: &dyn Nas, r: &Records, now: u64) -> Result<Plan> {
    let b = match nas.read(PATH)? {
        None => return Ok(Plan { said: format!("No marker ({PATH}): nothing fences take-ups."), write: None }),
        Some(b) => b,
    };
    let m: Marker = match serde_json::from_slice(&b) {
        Ok(m) => m,
        Err(e) => {
            let Some(mut horizon) = journal::day(now.saturating_sub(KEEP_DAYS * 86400)) else {
                anyhow::bail!("the clock")
            };
            if let Some(k) =
                journal::list(nas, None)?.into_iter().find(|k| k.as_str() < horizon.as_str() && !r.handles(k))
            {
                horizon = k.split('/').next().unwrap_or_default().to_string();
            }
            let keys: BTreeSet<String> = r
                .reflected
                .iter()
                .chain(r.rejected.keys())
                .filter(|k| k.as_str() < horizon.as_str())
                .cloned()
                .collect();
            let fresh =
                Marker { horizon, floor: r.horizon.clone(), term: r.term, seq: r.seq, at: now, keys, done: false };
            return Ok(Plan {
                said: format!(
                    "The marker can't be parsed ({e}): made again from term {}'s records at {} ({} entries before {}, \
                     floor {:?}), which name every entry it can have named.",
                    r.term,
                    r.seq,
                    fresh.keys.len(),
                    fresh.horizon,
                    fresh.floor
                ),
                write: Some(fresh),
            });
        }
    };
    if m.covers(r) {
        return Ok(Plan {
            said: format!(
                "The marker (entries before {}, {}) is covered by term {}'s records at {}: take-ups pass it.",
                m.horizon,
                if m.done { "removed" } else { "being removed" },
                r.term,
                r.seq
            ),
            write: None,
        });
    }
    if r.horizon.as_str() < m.floor.as_str() {
        return Ok(Plan {
            said: format!(
                "Not safe to lift: term {}'s records at {} haven't forgotten the entries before {} (the last GC's, \
                 removed): they're older than that GC. Restore newer records (state/build/term/) from a backup.",
                r.term, r.seq, m.floor
            ),
            write: None,
        });
    }
    let missing: Vec<&String> = m.keys.iter().filter(|k| !(r.handles(k) || r.forgot(k))).collect();
    let on: BTreeSet<String> = journal::list(nas, None)?.into_iter().collect();
    let gone: Vec<&&String> = missing.iter().filter(|k| !on.contains(k.as_str())).collect();
    if !gone.is_empty() {
        return Ok(Plan {
            said: format!(
                "Not safe to lift: {} marked entries the records (term {} at {}) don't name are already removed \
                 ({}{}): a take-up from these records would lose them. Restore newer records from a backup.",
                gone.len(),
                r.term,
                r.seq,
                gone.iter().take(3).map(|k| k.as_str()).collect::<Vec<_>>().join(", "),
                if gone.len() > 3 { ", …" } else { "" }
            ),
            write: None,
        });
    }
    let keys = m.keys.iter().filter(|k| !missing.contains(k)).cloned().collect();
    Ok(Plan {
        said: format!(
            "Term {}'s records at {} don't name {} marked entries, all still in the journal: dropped from the \
             marker (a take-up replays them), the rest kept.",
            r.term,
            r.seq,
            missing.len()
        ),
        write: Some(Marker { keys, ..m }),
    })
}

/// `scenic pool journal-gc [--repair]` on the share at `root`: `plan` against the newest records,
/// and with `repair` its marker written, only while the kill switch says `off` (no GC run then
/// changes the marker or removes a file meanwhile).
pub fn check(nas: &dyn Nas, now: u64, repair: bool) -> Result<String> {
    let cur = term::current(nas).context("the current term")?;
    let r = Records::newest(nas, cur.term, true)?.context("no records to check the marker against")?;
    let p = plan(nas, &r, now)?;
    let m = match p.write {
        Some(m) if repair => m,
        Some(_) => return Ok(format!("{}\nRun `scenic pool journal-gc --repair` to write it.", p.said)),
        None => return Ok(p.said),
    };
    anyhow::ensure!(
        !enabled(nas),
        "{}\nNot written: say `off` in {SWITCH} first (the GC mustn't run meanwhile), then again.",
        p.said
    );
    nas.write_whole(PATH, &serde_json::to_vec(&m)?)?;
    Ok(format!("{}\nWritten. Take-ups pass it now; {SWITCH} back to on (or removed) lets the GC run again.", p.said))
}

/// What a run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Entries marked to remove.
    pub marked: usize,
    /// Entries removed, and day folders emptied.
    pub removed: usize,
    pub days: usize,
    /// The marker's entries all removed.
    pub done: bool,
    /// Why it did nothing, or stopped.
    pub waits: Vec<String>,
    /// Folders, listings and reads made (the job's NAS weight).
    pub listed: u64,
}

/// One run (see the module's comment): marks, or removes what was marked `WAIT_S` before. `now`:
/// the lead's clock; `between`: a safe point, called before each day (the job may stop there);
/// `rmdir`: removes an emptied day's folder (an error, a folder not empty, is let be).
pub fn run(nas: &dyn Nas, now: u64, between: &dyn Fn(), rmdir: &dyn Fn(&str)) -> Result<Report> {
    let mut rep = Report::default();
    let cur = term::current(nas).context("the current term")?;
    if cur.lead.is_none() {
        rep.waits.push(format!("term {} has no lead known", cur.term));
        return Ok(rep);
    }
    let Some(r) = Records::load(nas, cur.term)? else {
        rep.waits.push(format!("term {}'s records aren't readable", cur.term));
        return Ok(rep);
    };
    match read(nas)? {
        Some(m) if !m.done => remove(nas, &m, cur.term, now, between, rmdir, &mut rep)?,
        Some(m) if r.horizon < m.horizon => {
            rep.waits.push(format!("the records haven't forgotten the entries before {} yet", m.horizon))
        }
        last => mark(nas, &r, last.map(|m| m.horizon).unwrap_or_default(), now, &mut rep)?,
    }
    Ok(rep)
}

/// Marks the entries to remove (see the module's comment).
fn mark(nas: &dyn Nas, r: &Records, floor: String, now: u64, rep: &mut Report) -> Result<()> {
    let Some(mut horizon) = journal::day(now.saturating_sub(KEEP_DAYS * 86400)) else { return Ok(()) };
    let mut keys = BTreeSet::new();
    rep.listed += 1;
    for d in nas.list(journal::DIR)? {
        if !journal::is_day(&d) || d >= horizon {
            continue;
        }
        rep.listed += 1;
        let names = nas.list(&format!("{}/{d}", journal::DIR))?;
        let day: Vec<String> = names
            .iter()
            .filter_map(|n| n.strip_suffix(".json").filter(|s| s.parse::<journal::LeaseId>().is_ok()))
            .map(|s| format!("{d}/{s}"))
            .collect();
        if let Some(k) = day.iter().find(|k| !r.handles(k)) {
            rep.waits.push(format!("{k} isn't merged yet: its day and the days after kept"));
            horizon = d;
            break;
        }
        keys.extend(day);
    }
    keys.retain(|k| k.as_str() < horizon.as_str());
    if keys.is_empty() {
        return Ok(());
    }
    let m = Marker { horizon, floor, term: r.term, seq: r.seq, at: now, keys, done: false };
    nas.write_whole(PATH, &serde_json::to_vec(&m)?)?;
    rep.marked = m.keys.len();
    Ok(())
}

/// Removes marker `m`'s entries, `WAIT_S` after it was written (see the module's comment).
fn remove(
    nas: &dyn Nas,
    m: &Marker,
    term: u64,
    now: u64,
    between: &dyn Fn(),
    rmdir: &dyn Fn(&str),
    rep: &mut Report,
) -> Result<()> {
    if now < m.at.saturating_add(WAIT_S) {
        rep.waits.push(format!("marked {} min ago: removed once every read sees it", now.saturating_sub(m.at) / 60));
        return Ok(());
    }
    // Every member's app keeps the fence.
    rep.listed += 1;
    for n in nas.list("state/pool/members")? {
        let Some(id) = n.strip_suffix(".json") else { continue };
        match Beat::read(nas, id)? {
            Some(b) if b.journal >= FENCE => {}
            Some(b) => {
                rep.waits.push(format!("{} runs {}, an app from before the journal's GC", b.host, b.app));
                return Ok(());
            }
            None => {
                rep.waits.push(format!("member {id}'s heartbeat can't be read"));
                return Ok(());
            }
        }
    }
    // The current term's records, as on the share, cover it.
    match Records::load(nas, term)? {
        Some(r) if m.covers(&r) => {}
        _ => {
            rep.waits.push(format!("term {term}'s records don't name every entry marked"));
            return Ok(());
        }
    }
    let mut by_day: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for k in &m.keys {
        by_day.entry(k.split('/').next().unwrap_or_default()).or_default().push(k);
    }
    let mut left = MAX_REMOVES;
    for (day, keys) in by_day {
        between();
        // (The term and the marker as they were: a lead that lost its term, or a marker replaced,
        // stops here.)
        if term::next(nas, term)? {
            rep.waits.push(format!("term {term} isn't current any more"));
            return Ok(());
        }
        if read(nas)?.as_ref() != Some(m) {
            rep.waits.push("the marker changed".into());
            return Ok(());
        }
        let dir = format!("{}/{day}", journal::DIR);
        rep.listed += 1;
        let names: BTreeSet<String> = nas.list(&dir)?.into_iter().collect();
        for k in keys {
            let n = format!("{}.json", k.rsplit('/').next().unwrap_or_default());
            if !names.contains(&n) {
                continue;
            }
            if left == 0 {
                rep.waits.push(format!("{MAX_REMOVES} removed: the rest next run"));
                return Ok(());
            }
            nas.remove(&journal::path(k))?;
            rep.removed += 1;
            left -= 1;
        }
        let set: BTreeSet<&str> = m.keys.iter().map(String::as_str).collect();
        if names.iter().all(|n| n.strip_suffix(".json").is_some_and(|s| set.contains(format!("{day}/{s}").as_str()))) {
            rmdir(&dir);
            rep.days += 1;
        }
    }
    let done = Marker { done: true, ..m.clone() };
    nas.write_whole(PATH, &serde_json::to_vec(&done)?)?;
    rep.done = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! `plan` (task #651): a marker the records don't cover lifted only where no entry is lost.
    use super::*;
    use crate::pool::nas::Mem;

    const NOW: u64 = 1_790_000_000;

    fn on_share(nas: &Mem, keys: &[&str]) {
        for k in keys {
            nas.write_whole(&journal::path(k), b"{}").unwrap();
        }
    }

    fn marker(nas: &Mem, keys: &[&str], floor: &str) -> Marker {
        let m = Marker {
            horizon: "2026-09-10".into(),
            floor: floor.into(),
            term: 4,
            seq: 9,
            at: NOW,
            keys: keys.iter().map(|k| k.to_string()).collect(),
            done: false,
        };
        nas.write_whole(PATH, &serde_json::to_vec(&m).unwrap()).unwrap();
        m
    }

    fn records(named: &[&str], horizon: &str) -> Records {
        Records {
            term: 3,
            seq: 2,
            reflected: named.iter().map(|k| k.to_string()).collect(),
            horizon: horizon.into(),
            ..Default::default()
        }
    }

    #[test]
    fn entries_the_records_dont_name_are_dropped_from_the_marker_only_while_still_in_the_journal() {
        let nas = Mem::default();
        let m = marker(&nas, &["2026-09-01/3-1", "2026-09-01/3-2", "2026-09-02/3-3"], "");
        let r = records(&["2026-09-01/3-1"], "");
        // Not named, still on the share: dropped (a take-up replays them), the rest kept.
        on_share(&nas, &["2026-09-01/3-2", "2026-09-02/3-3"]);
        let p = plan(&nas, &r, NOW).unwrap();
        let w = p.write.expect(&p.said);
        assert_eq!(w.keys, ["2026-09-01/3-1".to_string()].into(), "{}", p.said);
        assert!(w.covers(&r) && Marker { keys: w.keys.clone(), ..m.clone() } == w);
        // One of them already removed: not safe, nothing written.
        nas.remove(&journal::path("2026-09-02/3-3")).unwrap();
        let p = plan(&nas, &r, NOW).unwrap();
        assert!(p.write.is_none() && p.said.starts_with("Not safe") && p.said.contains("2026-09-02/3-3"), "{}", p.said);
    }

    #[test]
    fn records_short_of_the_floor_or_covering_it_change_nothing() {
        let nas = Mem::default();
        marker(&nas, &["2026-09-01/3-1"], "2026-08-20");
        let p = plan(&nas, &records(&["2026-09-01/3-1"], "2026-08-01"), NOW).unwrap();
        assert!(p.write.is_none() && p.said.starts_with("Not safe"), "{}", p.said);
        let p = plan(&nas, &records(&["2026-09-01/3-1"], "2026-08-20"), NOW).unwrap();
        assert!(p.write.is_none() && p.said.contains("covered"), "{}", p.said);
    }

    #[test]
    fn a_corrupt_marker_is_made_again_naming_every_old_entry_the_records_name() {
        let nas = Mem::default();
        nas.write_whole(PATH, b"{\"hori").unwrap();
        let e = fence(&nas, &Records::default()).unwrap_err();
        assert!(e.is::<Fenced>(), "{e:#}");
        // Named: one removed (not on the share), one there; an unnamed one stops the days there.
        on_share(&nas, &["2026-09-02/3-2", "2026-09-05/3-4", "2026-09-06/3-5"]);
        let r = records(&["2026-09-01/3-1", "2026-09-02/3-2", "2026-09-06/3-5"], "2026-08-20");
        let w = plan(&nas, &r, NOW).unwrap().write.expect("made again");
        assert_eq!(
            (w.horizon.as_str(), w.floor.as_str(), w.done),
            ("2026-09-05", "2026-08-20", false),
            "up to the day of the entry it doesn't name"
        );
        assert_eq!(w.keys, ["2026-09-01/3-1".to_string(), "2026-09-02/3-2".to_string()].into());
        assert!(w.covers(&r));
        // Records missing the removed one are fenced, as by the marker lost.
        assert!(!w.covers(&records(&["2026-09-02/3-2"], "2026-08-20")));
    }
}
