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
//! again) to take up from records that don't name or forget every key it lists, or haven't the floor.
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

/// Whether records `r` may be taken up from (the fence): an error saying why not.
pub fn fence(nas: &dyn Nas, r: &Records) -> Result<()> {
    match read(nas)? {
        Some(m) if !m.covers(r) => anyhow::bail!(
            "the journal's entries before {} are being removed and term {}'s records at {} don't name them all \
             (a stale read): try again shortly",
            m.horizon,
            r.term,
            r.seq
        ),
        _ => Ok(()),
    }
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
