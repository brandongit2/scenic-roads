//! The build's records per term (docs/pool.md §6.2): the manifest, the job keys, the uploads not
//! yet verified, the raw tiles' archives waiting to be named, and the journal entries they reflect,
//! in one file per term, `state/build/term/<E>/records.json`, written whole by that term's lead
//! after each merge, with a sequence number one more than the last. A reader never pairs keys from
//! one version with a manifest of another (invariant 4); a lead that slept through a later term
//! writes its own term's file, which no one reads once that later term has records of its own
//! (invariant 5).
//!
//! Term 1 is today's layout, `state/build/manifest.json`, `jobs.json` and `pending.json` (nothing
//! moves when the pool begins): its first snapshot is made from them before term 1 itself is
//! (`first`), and they're written after each of its snapshots, for today's readers. The pool reads
//! them again only to take up from a first snapshot that can't be read (its maker stopped midway,
//! or its bytes landed out of order), which no snapshot of a lead's has replaced: three files can
//! be read half rewritten, one stale and another not, so term 1's snapshot exists before any Mac
//! knows of term 1, and reads whole like any term's.
//!
//! Taking up a term (`start`, then the merge and the save) starts from the records of the term
//! before (the newest snapshot a read finds, or the one a handover names), replays the journal
//! entries they don't name in (term, lease) order, and saves that as the new term's first snapshot.
//! A stale read only sends an entry to the replay, or to its member's telling the new lead again
//! (crate::pool::journal::Mine): none is lost (invariant 3). Each target keeps the lease that last
//! set it, so an older lease's entry merged later (a member back from sleep, a replay) doesn't undo
//! a newer one's.

use super::journal::{self, Entry, LeaseId};
use super::nas::{Created, Nas};
use super::term::Term;
use crate::agent::build::Keys;
use crate::rawpack::Pack;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Term 1's records in today's layout: the manifest, the job keys, the unverified uploads.
const TODAY: [&str; 3] = ["state/build/manifest.json", "state/build/jobs.json", "state/build/pending.json"];

/// A check of a journal entry before it's applied, given the records it would apply to (§7.3: its
/// writes within its step's write-set, its content names matching their logical names, its lease's
/// targets and keys): why it's refused, if it is.
pub type Check<'a> = &'a dyn Fn(&Entry, &Records) -> std::result::Result<(), String>;

/// A term's records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Records {
    /// The term they're of.
    pub term: u64,
    /// The snapshot's number in its term: one more each save, from 1 (0: none saved yet).
    pub seq: u64,
    /// Logical name → content name.
    pub manifest: BTreeMap<String, String>,
    /// The keys of the jobs that last succeeded.
    pub keys: Keys,
    /// Uploads not yet verified on the NAS: content name → SHA-256.
    pub pending: BTreeMap<String, String>,
    /// Raw terrain tiles' archives handed off and not yet named in the raw store's index: the lead
    /// names them (crate::rawpack::name_handed, which passes over any already named) and takes
    /// them off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<(String, Pack)>,
    /// The journal entries applied (or passed over: `apply`), by key (crate::pool::journal::Entry::key).
    #[serde(default)]
    pub reflected: BTreeSet<String>,
    /// The journal entries refused, with why.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rejected: BTreeMap<String, String>,
    /// The lease of the last done record applied, by step and target (`slots`): an entry of an older
    /// lease for one of them, merged later, changes nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub last: BTreeMap<String, BTreeMap<String, LeaseId>>,
    /// The day (YYYY-MM-DD) before which the journal's entries are forgotten (`forget_before`):
    /// GC removed them, every snapshot since naming them. Empty: none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub horizon: String,
    /// The coordinator's state as this term's lead wrote it last, settling a handover (§6.4): the
    /// next lead loads it with these records, so the handover's `seq` names both. Taken off at
    /// take-up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handed: Option<serde_json::Value>,
}

/// The path of term `term`'s snapshot.
pub fn path(term: u64) -> String {
    format!("state/build/term/{term}/records.json")
}

/// JSON file `p` parsed, or the default when there's none.
fn read_or_default<T: Default + serde::de::DeserializeOwned>(nas: &dyn Nas, p: &str) -> Result<T> {
    match nas.read(p)? {
        Some(b) => serde_json::from_slice(&b).with_context(|| format!("parse {p}")),
        None => Ok(T::default()),
    }
}

/// The slots a done record's target `t` of step `step` is kept under, for lease order (`last`): a
/// prune's `<kind> <at>` removes kind's key for `at` (and a pois prune the peaks' too:
/// crate::agent::build::Keys::record); any other target is its step's.
fn slots(step: &str, t: &str) -> Vec<(String, String)> {
    match t.split_once(' ').filter(|_| step == "prune") {
        Some(("pois", at)) => vec![("pois".into(), at.into()), ("peaks".into(), at.into())],
        Some((kind, at)) => vec![(kind.into(), at.into())],
        None => vec![(step.into(), t.into())],
    }
}

impl Records {
    /// Term `term`'s snapshot, as read now; None when it has none (its lead never saved one, or the
    /// read is stale). An error when what's there can't be read now, or parsed.
    pub fn load(nas: &dyn Nas, term: u64) -> Result<Option<Records>> {
        let Some(b) = nas.read(&path(term))? else { return Ok(None) };
        let r: Records = serde_json::from_slice(&b).with_context(|| format!("parse {}", path(term)))?;
        ensure!(r.term == term, "{} holds term {}'s records", path(term), r.term);
        Ok(Some(r))
    }

    /// The records a reader (a job, the map server, a catalog) has while term `term` is current:
    /// its snapshot, or while it has none yet (its lead taking it up, or gone before saving one)
    /// the newest of a term before it. None before any.
    pub fn newest(nas: &dyn Nas, term: u64) -> Result<Option<Records>> {
        for f in (1..=term).rev() {
            if let Some(r) = Records::load(nas, f)? {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// The build's records as today's three files hold them (term 1's, as the pool begins).
    pub fn today(nas: &dyn Nas) -> Result<Records> {
        Ok(Records { term: 1, manifest: read_or_default(nas, TODAY[0])?, keys: read_or_default(nas, TODAY[1])?, pending: read_or_default(nas, TODAY[2])?, ..Default::default() })
    }

    /// Saves them as their term's next snapshot (`seq` one more), whole; term 1's also to today's
    /// three files after. The number goes up even when the write fails, so no two versions that may
    /// land share one.
    pub fn save(&mut self, nas: &dyn Nas) -> Result<()> {
        ensure!(self.term >= 1, "records of no term");
        self.seq += 1;
        nas.write_whole(&path(self.term), &serde_json::to_vec(self)?)?;
        if self.term == 1 {
            nas.write_whole(TODAY[0], &serde_json::to_vec_pretty(&self.manifest)?)?;
            nas.write_whole(TODAY[2], &serde_json::to_vec_pretty(&self.pending)?)?;
            nas.write_whole(TODAY[1], &serde_json::to_vec_pretty(&self.keys)?)?;
        }
        Ok(())
    }

    /// Whether journal entry `key` is in them: applied (or passed over), or refused.
    pub fn handles(&self, key: &str) -> bool {
        self.reflected.contains(key) || self.rejected.contains_key(key)
    }

    /// Whether journal entry `key` is of a day they've forgotten (`forget_before`).
    pub fn forgot(&self, key: &str) -> bool {
        !self.horizon.is_empty() && key < self.horizon.as_str()
    }

    /// Applies journal entry `e`, under key `key`, as crate::handoff's merge did: its manifest
    /// changes, its uploads pending and checked, its done record in the keys, its raw archives to
    /// name; and names it. Passed over (named, nothing applied: false) when a later lease's entry
    /// set any of its targets already (lease order across merges: a member back from sleep tells
    /// of an older lease's entry after a newer one's was merged).
    pub fn apply(&mut self, key: &str, e: &Entry) -> bool {
        let h = &e.handoff;
        let slots: Vec<(String, String)> = h.done.iter().flat_map(|(step, ts)| ts.iter().flat_map(|(t, _)| slots(step, t))).collect();
        if slots.iter().any(|(s, t)| self.last.get(s).and_then(|m| m.get(t)).is_some_and(|l| *l > e.lease)) {
            self.reflected.insert(key.to_string());
            return false;
        }
        for (k, v) in &h.changes {
            match v {
                Some(n) => self.manifest.insert(k.clone(), n.clone()),
                None => self.manifest.remove(k),
            };
        }
        self.pending.extend(h.pending.iter().map(|(k, v)| (k.clone(), v.clone())));
        for c in &h.checked {
            self.pending.remove(c);
        }
        if let Some((step, targets)) = &h.done {
            self.keys.record(step, targets);
        }
        for r in &h.raw {
            if !self.raw.contains(r) {
                self.raw.push(r.clone());
            }
        }
        for (s, t) in slots {
            self.last.entry(s).or_default().insert(t, e.lease);
        }
        self.reflected.insert(key.to_string());
        true
    }

    /// Names journal entry `key` refused, with why.
    pub fn refuse(&mut self, key: &str, why: &str) {
        self.rejected.insert(key.to_string(), why.to_string());
    }

    /// Forgets the journal entries of the days before `day` (YYYY-MM-DD), once GC has removed those
    /// days from the journal (§7.3: every snapshot since names their entries): none is listed
    /// again, and one told again is answered as merged (`Merged::forgotten`), the horizon kept.
    pub fn forget_before(&mut self, day: &str) {
        self.reflected.retain(|k| k.as_str() >= day);
        self.rejected.retain(|k, _| k.as_str() >= day);
        if day > self.horizon.as_str() {
            self.horizon = day.to_string();
        }
    }
}

/// Makes term 1's first snapshot from today's three files, unless it has one: before term 1 is
/// made (crate::pool::term::bootstrap), so every Mac that learns of term 1 reads its snapshot whole.
/// (Made with create-new: of two Macs making it, the first's stays. One whose bytes didn't land is
/// left as it is: term 1's lead saves over it, and `start` takes term 1 up from today's files
/// meanwhile. Its maker finishing it later could land over that lead's snapshot.)
pub fn first(nas: &dyn Nas) -> Result<()> {
    if nas.exists(&path(1))? {
        return Ok(());
    }
    let b = serde_json::to_vec(&Records { seq: 1, ..Records::today(nas)? })?;
    match nas.create_new(&path(1), &b)? {
        Created::Made | Created::There | Created::Unwritten(_) => Ok(()),
    }
}

/// What a merge did, by journal key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Applied to the records.
    pub applied: Vec<String>,
    /// Named in the records with nothing applied: a later lease's entry set their targets already.
    pub overtaken: Vec<String>,
    /// Refused, with why: their refusals to note once the records naming them are saved.
    pub refused: Vec<(String, String)>,
    /// Of days the records have forgotten, and not in the journal: merged long ago, to acknowledge.
    pub forgotten: Vec<String>,
    /// Read and not whole (being written, not seen yet, cut short, or removed): to merge later.
    pub waiting: Vec<String>,
    /// Not read: the reading stopped (`merge_while`), or the read failed; to merge later.
    pub unread: Vec<String>,
}

/// Merges the journal entries `keys` the records don't name yet: each read, checked and applied,
/// or refused, in (term, lease) order: this lead's check decides, whatever another lead's refusal
/// (crate::pool::journal::refusal). Nothing is written: the caller saves the records, then notes
/// the refusals (crate::pool::journal::note_refusal) and acknowledges what the saved records name,
/// and the forgotten.
pub fn merge(nas: &dyn Nas, r: &mut Records, keys: &[String], check: Check) -> Merged {
    merge_while(nas, r, keys, check, &|| true)
}

/// `merge`, reading entries only while `more()` says so (asked before each read): on a share under
/// load a read takes seconds, and a backlog of thousands must not hold one loop for an hour. The
/// keys not read are left in `Merged::unread`, to merge later; give the oldest leases first, as
/// `by_lease` orders them, so the order of what's applied holds across merges too.
pub fn merge_while(nas: &dyn Nas, r: &mut Records, keys: &[String], check: Check, more: &dyn Fn() -> bool) -> Merged {
    let mut out = Merged::default();
    let mut read: BTreeMap<(LeaseId, String), Entry> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut stopped = false;
    for k in keys {
        if r.handles(k) || !seen.insert(k.as_str()) {
            continue;
        }
        stopped = stopped || !more();
        if stopped {
            out.unread.push(k.clone());
            continue;
        }
        match journal::read(nas, k) {
            Ok(journal::Read::Entry(e)) => {
                read.insert((e.lease, k.clone()), e);
            }
            Ok(journal::Read::Damaged(why)) => {
                r.refuse(k, &why);
                out.refused.push((k.clone(), why));
            }
            Ok(journal::Read::Missing) if r.forgot(k) => out.forgotten.push(k.clone()),
            Ok(journal::Read::Short | journal::Read::Missing) => out.waiting.push(k.clone()),
            Err(_) => out.unread.push(k.clone()),
        }
    }
    for ((_, k), e) in read {
        match check(&e, r) {
            Ok(()) => {
                if r.apply(&k, &e) {
                    out.applied.push(k);
                } else {
                    out.overtaken.push(k);
                }
            }
            Err(why) => {
                r.refuse(&k, &why);
                out.refused.push((k, why));
            }
        }
    }
    out
}

/// Journal keys in the order of their leases (`<day>/<term>-<n>`: by term, then number, then key):
/// the order a merge applies them in.
pub fn by_lease(keys: &mut [String]) {
    keys.sort_by_cached_key(|k| (k.rsplit('/').next().and_then(|l| l.parse::<LeaseId>().ok()), k.clone()));
}

/// The records term `t` starts from (§6.2): the newest snapshot a read finds, walking down from
/// `t`'s own (an earlier try's that landed) to the terms before it; `own` in place of a read for
/// the term they're of (a lead re-asserting or taking back has its own; an earlier try of this
/// take-up, its save failed, has its records numbered on). A handover's term (`t.seq`) starts only
/// from the snapshot it names, or a later one of its term: an error until a read gives that one
/// (a stale read gives an older), to try again shortly. Term 1's first snapshot that can't be read
/// is today's files (`first`'s source, which nothing has written since: a lead's saves are whole).
/// Numbered as `t`'s, on from a snapshot of `t`'s own, from 0 otherwise.
pub fn start(nas: &dyn Nas, t: &Term, own: Option<&Records>) -> Result<Records> {
    let mut base = None;
    let mut f = t.term;
    while f >= 1 {
        if let Some(o) = own.filter(|o| o.term == f) {
            base = Some(o.clone());
            break;
        }
        let loaded = if f == 1 {
            match nas.read(&path(1))? {
                Some(b) => match serde_json::from_slice::<Records>(&b) {
                    Ok(r) if r.term == 1 => Some(r),
                    _ => Some(Records { seq: 1, ..Records::today(nas)? }),
                },
                None => None,
            }
        } else {
            Records::load(nas, f)?
        };
        if let Some(r) = loaded {
            base = Some(r);
            break;
        }
        // (A handover's must be read, not one before it.)
        if f == t.from && t.seq.is_some() {
            break;
        }
        f -= 1;
    }
    let mut r = match (base, t.seq) {
        (Some(b), _) if b.term == t.term => b,
        (Some(b), Some(s)) if b.term == t.from && b.seq >= s => b,
        (b, Some(s)) => bail!("term {}'s records handed over at {s} aren't readable yet (read: {}): try again shortly", t.from, b.map_or("none".into(), |b| format!("term {}'s at {}", b.term, b.seq))),
        (Some(b), None) => b,
        (None, None) => bail!("no records to take term {} up from", t.term),
    };
    if r.term != t.term {
        r.term = t.term;
        r.seq = 0;
    }
    Ok(r)
}

/// What taking up a term gave.
#[derive(Clone, Debug)]
pub struct TakenUp {
    /// The term's records, saved as its first snapshot.
    pub records: Records,
    /// The journal entries replayed into them, refused (their refusals noted), or not readable
    /// whole yet.
    pub merged: Merged,
    /// A handover's: the coordinator's state handed over with the records it started from.
    pub handed: Option<serde_json::Value>,
}

/// Whether records `r`, which `start` gave for term `t`, carry a coordinator's state handed over
/// for `t` (`Records::handed`): `t` is a handover's term. Kept in the term's first snapshot (a try
/// again, or a restart, of its take-up finds it there), and off its later ones; a takeover's term
/// starts with none (one there is an earlier handover's, older than the term before's own state).
pub fn handed(r: &mut Records, t: &Term) -> Option<serde_json::Value> {
    if t.seq.is_none() {
        r.handed = None;
    }
    r.handed.clone()
}

/// Takes up term `t` (§6.2) in one go: `start` (from `own`, as it says), the journal entries `keys`
/// (a listing of the journal) replayed where the records don't name them, `check`ed first; saved
/// as `t`'s first snapshot, and the refusals noted. When the save fails, `own` keeps the
/// records as tried, numbered: passed again, the next try numbers on (no two versions of a snapshot
/// that may both land share a number).
pub fn take_up(nas: &dyn Nas, t: &Term, own: &mut Option<Records>, keys: &[String], check: Check) -> Result<TakenUp> {
    let mut r = start(nas, t, own.as_ref())?;
    let handed = handed(&mut r, t);
    let merged = merge(nas, &mut r, keys, check);
    if let Err(e) = r.save(nas) {
        *own = Some(r);
        return Err(e).with_context(|| format!("save term {}'s records", t.term));
    }
    r.handed = None;
    *own = None;
    for (k, why) in &merged.refused {
        // (Named refused in the records saved: a note not made now is only the owner's loss.)
        journal::note_refusal(nas, k, why).ok();
    }
    Ok(TakenUp { records: r, merged, handed })
}

#[cfg(test)]
mod tests {
    use super::super::journal::LeaseId;
    use super::super::nas::Mem;
    use super::super::term::{self, Current, Made};
    use super::super::Member;
    use super::*;
    use crate::handoff::Handoff;
    use std::cell::Cell;

    const DAY: u64 = 1_791_300_000;

    /// An entry of lease `term`-`n` building unit `t` with key `k`.
    fn built(term: u64, n: u64, t: &str, k: &str) -> Entry {
        let content = format!("base/{t}.{k}.base");
        let h = Handoff { changes: [(format!("base/{t}"), Some(content.clone()))].into(), pending: [(content, "sha".into())].into(), done: Some(("unit".into(), vec![(t.into(), k.into())])), ..Default::default() };
        Entry { member: "m-000000000000000b".into(), lease: LeaseId { term, n }, step: "unit".into(), handoff: h, at: DAY + n }
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    fn lead(app: &str) -> Member {
        Member { id: "m-000000000000000a".into(), host: "Mac-mini".into(), app: app.into() }
    }

    /// Takes up `t` in one go, the journal listed whole.
    fn up(nas: &dyn Nas, t: &Term, own: Option<Records>, check: Check) -> Result<TakenUp> {
        take_up(nas, t, &mut own.clone(), &journal::list(nas, None)?, check)
    }

    #[test]
    fn term_ones_first_snapshot_is_todays_files_and_they_follow_its_saves() {
        let nas = Mem::default();
        nas.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        nas.write_whole(TODAY[1], br#"{"unit": {"6-1-1": "k0"}}"#).unwrap();
        assert_eq!(Records::load(&nas, 1).unwrap(), None);
        first(&nas).unwrap();
        let mut r = Records::load(&nas, 1).unwrap().unwrap();
        assert_eq!((r.term, r.seq, r.manifest.len(), r.keys.unit.len(), r.pending.len()), (1, 1, 1, 1, 0));
        assert_eq!(r, Records { seq: 1, ..Records::today(&nas).unwrap() });
        assert_eq!(Records::load(&nas, 2).unwrap(), None);
        let e = built(1, 1, "6-1-2", "k1");
        r.apply(&e.key().unwrap(), &e);
        r.save(&nas).unwrap();
        assert_eq!(Records::load(&nas, 1).unwrap(), Some(r.clone()), "its snapshot");
        // Made once: a second Mac's try leaves it.
        first(&nas).unwrap();
        assert_eq!(Records::load(&nas, 1).unwrap(), Some(r.clone()));
        // Its maker stopped between its create and its bytes: readers can't read it, and term 1's
        // take-up starts from today's files.
        let stopped = Mem::default();
        stopped.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        stopped.create_new(&path(1), b"").unwrap();
        assert!(Records::load(&stopped, 1).is_err());
        let a = lead("development");
        let t1 = term::bootstrap(&stopped, &a, DAY, true).unwrap().unwrap();
        let taken = up(&stopped, &t1, None, &any).unwrap();
        assert_eq!((taken.records.seq, taken.records.manifest.len()), (2, 1));
        // Today's files, written after it, for today's readers.
        let m: BTreeMap<String, String> = serde_json::from_slice(&nas.read(TODAY[0]).unwrap().unwrap()).unwrap();
        assert_eq!(m, r.manifest);
        let k: Keys = serde_json::from_slice(&nas.read(TODAY[1]).unwrap().unwrap()).unwrap();
        assert_eq!(k.unit.get("6-1-2").map(String::as_str), Some("k1"));
        assert!(nas.read(TODAY[2]).unwrap().is_some());
    }

    #[test]
    fn a_merge_applies_in_lease_order_and_refuses_what_its_check_refuses() {
        let nas = Mem::default();
        let mut r = Records { term: 3, ..Default::default() };
        // Told in another order than leased: the later lease's key wins.
        let (a, b) = (built(3, 9, "6-1-1", "k9"), built(3, 2, "6-1-1", "k2"));
        let bad = Entry { step: "bogus".into(), ..built(3, 5, "6-1-3", "kx") };
        let keys: Vec<String> = [&a, &b, &bad].iter().map(|e| journal::write(&nas, e).unwrap()).collect();
        let short = "2026-10-06/3-7".to_string();
        nas.create_new(&journal::path(&short), b"").unwrap();
        let check = |e: &Entry, _: &Records| if e.step == "bogus" { Err("writes outside its step's names".to_string()) } else { Ok(()) };
        let mut told = keys.clone();
        told.push(short.clone());
        told.push(keys[0].clone());
        let m = merge(&nas, &mut r, &told, &check);
        assert_eq!(m.applied, [b.key().unwrap(), a.key().unwrap()]);
        assert_eq!(m.refused, [(bad.key().unwrap(), "writes outside its step's names".to_string())]);
        assert_eq!(m.waiting, [short]);
        assert_eq!(r.keys.unit.get("6-1-1").map(String::as_str), Some("k9"));
        assert_eq!(r.manifest.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.k9.base"));
        assert!(!r.manifest.contains_key("base/6-1-3") && r.handles(&bad.key().unwrap()) && r.pending.len() == 2);
        // Told again: nothing more.
        assert_eq!(merge(&nas, &mut r, &keys, &check), Merged::default());
        // The days GC removed, forgotten.
        r.refuse("2026-10-05/2-1", "no such step");
        r.forget_before("2026-10-06");
        assert!(!r.handles("2026-10-05/2-1") && r.handles(&a.key().unwrap()) && r.handles(&bad.key().unwrap()));
    }

    #[test]
    fn an_older_lease_merged_after_a_newer_one_is_passed_over() {
        // (Review M5: lease order held only within one merge, and an older lease's entry merged
        // later won.)
        let nas = Mem::default();
        let mut r = Records { term: 3, ..Default::default() };
        let newer = journal::write(&nas, &built(3, 9, "6-1-1", "k9")).unwrap();
        let older = journal::write(&nas, &built(3, 5, "6-1-1", "k5")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&newer), &any).applied, std::slice::from_ref(&newer));
        let m = merge(&nas, &mut r, std::slice::from_ref(&older), &any);
        assert_eq!((m.applied.len(), m.overtaken.clone()), (0, vec![older.clone()]));
        assert_eq!(r.keys.unit.get("6-1-1").map(String::as_str), Some("k9"), "the newer lease's key");
        assert_eq!(r.manifest.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.k9.base"));
        assert!(r.handles(&older), "named: it doesn't wait");
        // An older lease's entry for other targets, and a later prune of the unit, apply; a build
        // older than the prune doesn't bring it back.
        let other = journal::write(&nas, &built(3, 4, "6-1-2", "k4")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&other), &any).applied, [other]);
        let h = Handoff { changes: [("base/6-1-1".to_string(), None)].into(), done: Some(("prune".into(), vec![("unit 6-1-1".into(), String::new())])), ..Default::default() };
        let prune = journal::write(&nas, &Entry { step: "prune".into(), handoff: h, ..built(3, 12, "6-1-1", "-") }).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&prune), &any).applied, [prune]);
        let late = journal::write(&nas, &built(3, 10, "6-1-1", "k10")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&late), &any).overtaken, [late]);
        assert!(!r.manifest.contains_key("base/6-1-1") && !r.keys.unit.contains_key("6-1-1"));
        assert_eq!(r.last["unit"]["6-1-1"], LeaseId { term: 3, n: 12 });
        // Kept in the snapshot, for the next lead.
        assert_eq!(serde_json::from_slice::<Records>(&serde_json::to_vec(&r).unwrap()).unwrap(), r);
    }

    #[test]
    fn a_merge_reads_while_its_budget_lasts_and_leaves_the_rest() {
        // (Review 2: a lead taking up from an old snapshot read every entry since in one loop, on a
        // share under load an hour of reads.)
        let nas = Mem::default();
        let mut r = Records { term: 3, ..Default::default() };
        let mut keys: Vec<String> = [(3, 9, "6-1-1"), (2, 4, "6-1-2"), (3, 1, "6-1-3")].iter().map(|&(t, n, u)| journal::write(&nas, &built(t, n, u, "k")).unwrap()).collect();
        by_lease(&mut keys);
        assert_eq!(keys, ["2026-10-06/2-4", "2026-10-06/3-1", "2026-10-06/3-9"]);
        let reads = Cell::new(0);
        let m = merge_while(&nas, &mut r, &keys, &any, &|| {
            reads.set(reads.get() + 1);
            reads.get() <= 2
        });
        assert_eq!((m.applied, m.unread.clone()), (keys[..2].to_vec(), keys[2..].to_vec()));
        assert_eq!(merge(&nas, &mut r, &m.unread, &any).applied, keys[2..]);
    }

    #[test]
    fn a_read_that_fails_leaves_its_entry_unread_and_the_rest_read() {
        // (A share that doesn't answer says nothing of an entry: a lead refuses one only once its
        // reads answer that it isn't whole, for an hour: driver::UNREADABLE_S.)
        struct Failing(Mem, String);
        impl Nas for Failing {
            fn create_new(&self, p: &str, bytes: &[u8]) -> Result<Created> {
                self.0.create_new(p, bytes)
            }
            fn write_whole(&self, p: &str, bytes: &[u8]) -> Result<()> {
                self.0.write_whole(p, bytes)
            }
            fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
                if p == self.1 {
                    bail!("read {p}: the share didn't answer");
                }
                self.0.read(p)
            }
            fn exists(&self, p: &str) -> Result<bool> {
                self.0.exists(p)
            }
            fn list(&self, dir: &str) -> Result<Vec<String>> {
                self.0.list(dir)
            }
            fn remove(&self, p: &str) -> Result<()> {
                self.0.remove(p)
            }
        }
        let mem = Mem::default();
        let keys: Vec<String> = [(3, 1, "6-1-1"), (3, 2, "6-1-2")].iter().map(|&(t, n, u)| journal::write(&mem, &built(t, n, u, "k")).unwrap()).collect();
        let short = "2026-10-06/3-3".to_string();
        mem.write_whole(&journal::path(&short), b"{\"member\":").unwrap();
        let nas = Failing(mem, journal::path(&keys[0]));
        let mut r = Records { term: 3, ..Default::default() };
        let m = merge(&nas, &mut r, &[keys.clone(), vec![short.clone()]].concat(), &any);
        assert_eq!((m.unread, m.applied, m.waiting), (vec![keys[0].clone()], vec![keys[1].clone()], vec![short]));
    }

    #[test]
    fn an_entry_of_a_forgotten_day_told_again_is_answered() {
        // A member back after more than a week tells of an entry whose day GC removed and the
        // records forgot. (Review M8: it waited forever, and a lead that settles only with nothing
        // waiting never handed over.)
        let nas = Mem::default();
        let mut r = Records { term: 4, ..Default::default() };
        let x = journal::write(&nas, &built(4, 1, "6-1-1", "k1")).unwrap();
        merge(&nas, &mut r, std::slice::from_ref(&x), &any);
        nas.remove(&journal::path(&x)).unwrap();
        r.forget_before("2026-10-14");
        assert_eq!(r.horizon, "2026-10-14");
        let m = merge(&nas, &mut r, std::slice::from_ref(&x), &any);
        assert_eq!((m.forgotten.clone(), m.waiting.len()), (vec![x.clone()], 0));
        // One of a forgotten day that's in the journal (written late) is merged as any; an earlier
        // horizon doesn't move it back.
        let y = journal::write(&nas, &built(4, 2, "6-1-2", "k2")).unwrap();
        r.forget_before("2026-10-01");
        assert_eq!((merge(&nas, &mut r, std::slice::from_ref(&y), &any).applied, r.horizon.as_str()), (vec![y], "2026-10-14"));
    }

    #[test]
    fn a_new_lead_takes_up_from_the_term_before_and_replays_what_it_lacks() {
        let nas = Mem::default();
        let a = lead("20261005-2202-61eb22c");
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let up1 = up(&nas, &t1, None, &any).unwrap();
        assert_eq!((up1.records.term, up1.records.seq), (1, 2), "after its first snapshot");
        // Term 1's lead merges one entry and saves; another is written and never merged.
        let (e1, e2) = (built(1, 1, "6-1-1", "k1"), built(1, 2, "6-1-2", "k2"));
        let k1 = journal::write(&nas, &e1).unwrap();
        journal::write(&nas, &e2).unwrap();
        let mut r1 = up1.records;
        merge(&nas, &mut r1, &[k1], &any);
        r1.save(&nas).unwrap();
        // Taken over: term 2 starts from term 1's snapshot and replays the other.
        let b = Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "20261005-2202-61eb22c".into() };
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1.clone()) }, &b, "taken over", DAY + 60).unwrap().unwrap();
        // A reader while term 2 has no snapshot yet: term 1's.
        assert_eq!(Records::newest(&nas, 2).unwrap().map(|r| (r.term, r.seq)), Some((1, 3)));
        let up2 = up(&nas, &t2, None, &any).unwrap();
        assert_eq!(up2.merged.applied, [e2.key().unwrap()]);
        assert_eq!((up2.records.term, up2.records.seq), (2, 1));
        assert!(up2.records.handles(&e1.key().unwrap()) && up2.records.handles(&e2.key().unwrap()));
        assert_eq!(Records::load(&nas, 2).unwrap(), Some(up2.records.clone()));
        assert_eq!(Records::newest(&nas, 2).unwrap(), Some(up2.records.clone()));
        // Term 3 made, its lead gone before saving anything: term 4 starts from term 2's.
        let t3 = term::claim(&nas, &Current { term: 2, lead: Some(t2.clone()) }, &a, "taken over", DAY + 120).unwrap().unwrap();
        let t4 = term::claim(&nas, &Current { term: 3, lead: Some(t3) }, &b, "taken over", DAY + 900).unwrap().unwrap();
        assert_eq!(Records::newest(&nas, 4).unwrap().map(|r| r.term), Some(2), "readers meanwhile: term 2's (review L6: none)");
        let up4 = up(&nas, &t4, None, &any).unwrap();
        assert_eq!((up4.records.term, up4.records.reflected.len()), (4, 2));
        // Tried again after its save landed: from its own snapshot, its number going on.
        let again = up(&nas, &t4, None, &any).unwrap();
        assert_eq!((again.records.term, again.records.seq), (4, 2));
    }

    #[test]
    fn a_handover_is_taken_up_only_from_the_snapshot_it_names_with_the_coordinators_state() {
        let nas = Mem::default();
        let a = lead("20261005-2202-61eb22c");
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let mut r = up(&nas, &t1, None, &any).unwrap().records;
        let e = built(1, 1, "6-1-1", "k1");
        let k = journal::write(&nas, &e).unwrap();
        merge(&nas, &mut r, std::slice::from_ref(&k), &any);
        r.save(&nas).unwrap();
        let old = nas.read(&path(1)).unwrap().unwrap();
        // Settled: the coordinator's state saved with the records the handover names.
        let mut r2 = r.clone();
        r2.handed = Some(serde_json::json!({"leases": ["1-4"]}));
        r2.save(&nas).unwrap();
        // Handed over at seq 4 (term 1's snapshot read as at 3: a stale read): not yet.
        let b = Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "20261005-2202-61eb22c".into() };
        let mut t2 = Term::after(&Current { term: 1, lead: Some(t1) }, &b, "handed over by Mac-mini", DAY + 60).unwrap();
        t2.seq = Some(r2.seq);
        assert!(matches!(term::make(&nas, &t2).unwrap(), Made::Ours));
        let fresh = nas.read(&path(1)).unwrap().unwrap();
        nas.write_whole(&path(1), &old).unwrap();
        assert!(up(&nas, &t2, None, &any).is_err());
        assert_eq!(Records::load(&nas, 2).unwrap(), None, "nothing saved");
        // Read whole: taken up, the coordinator's state with it (kept in the new term's first
        // snapshot, for a take-up tried again; off its next).
        nas.write_whole(&path(1), &fresh).unwrap();
        let mut taken = up(&nas, &t2, None, &any).unwrap();
        assert!(taken.records.handles(&k) && taken.merged.applied.is_empty());
        assert_eq!(taken.handed, Some(serde_json::json!({"leases": ["1-4"]})));
        assert_eq!(Records::load(&nas, 2).unwrap().unwrap().handed, taken.handed);
        assert_eq!(taken.records.handed, None);
        taken.records.save(&nas).unwrap();
        assert_eq!(Records::load(&nas, 2).unwrap().unwrap().handed, None);
        // A lead re-asserting starts from its own records, unread.
        let mut own = taken.records.clone();
        let e4 = built(2, 4, "6-1-4", "k4");
        own.apply(&e4.key().unwrap(), &e4);
        let t3 = term::claim(&nas, &Current { term: 2, lead: Some(t2) }, &b, "re-asserted after a gap", DAY + 900).unwrap().unwrap();
        let up3 = up(&nas, &t3, Some(own), &any).unwrap();
        assert_eq!(up3.records.keys.unit.get("6-1-4").map(String::as_str), Some("k4"));
    }

    #[test]
    fn an_entry_a_stale_lead_acknowledged_reaches_the_next_lead_by_its_member() {
        // Term 2 taken over and taken up (the journal listed); then an entry is written, and term
        // 1's lead, not knowing of term 2 yet, merges it, saves and acknowledges it. Term 2's
        // records lack it, and no take-up will list the journal again: its member tells term 2's
        // lead of it, as of every entry term 2's lead hasn't acknowledged.
        let nas = Mem::default();
        let (a, b) = (lead("development"), Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "development".into() });
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let mut r1 = up(&nas, &t1, None, &any).unwrap().records;
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &b, "taken over by MacBook-Air", DAY + 700).unwrap().unwrap();
        let mut r2 = up(&nas, &t2, None, &any).unwrap().records;
        let mut mine = journal::Mine::default();
        let x = journal::write(&nas, &built(1, 9, "6-1-1", "k9")).unwrap();
        mine.wrote(&x);
        merge(&nas, &mut r1, std::slice::from_ref(&x), &any);
        r1.save(&nas).unwrap();
        mine.acked(&x, 1);
        assert!(!r2.handles(&x));
        // Told once only, it would be lost; told to the lead of the term its member knows now:
        assert_eq!(mine.to_tell(2), std::slice::from_ref(&x));
        assert_eq!(merge(&nas, &mut r2, &mine.to_tell(2), &any).applied, std::slice::from_ref(&x));
        assert_eq!(r2.keys.unit.get("6-1-1").map(String::as_str), Some("k9"));
    }

    #[test]
    fn a_refused_entrys_why_is_noted_once_its_refusal_is_saved() {
        let nas = Mem::default();
        let a = lead("development");
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let bad = Entry { step: "bogus".into(), ..built(1, 3, "6-1-3", "kx") };
        let k = journal::write(&nas, &bad).unwrap();
        let check = |e: &Entry, _: &Records| if e.step == "bogus" { Err("no such step".to_string()) } else { Ok(()) };
        let taken = up(&nas, &t1, None, &check).unwrap();
        assert_eq!(taken.records.rejected.get(&k).map(String::as_str), Some("no such step"));
        assert_eq!(journal::refusal(&nas, &k).unwrap().as_deref(), Some("no such step"));
        // Told again by its member, to a lead whose records lack it: checked again, refused again.
        let mut r = Records { term: 2, ..Default::default() };
        let m = merge(&nas, &mut r, std::slice::from_ref(&k), &check);
        assert_eq!(m.refused, [(k, "no such step".to_string())]);
    }

    #[test]
    fn a_stale_leads_refusal_is_checked_again_by_the_current_lead() {
        // A stale lead's check refuses an entry (say, it leased its targets again after the lease
        // lapsed, or its app's write-sets are older) and sets it aside. (Review M1: the current
        // lead, whose check would take it, refused it unread.)
        let nas = Mem::default();
        let (a, b) = (lead("development"), Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "development".into() });
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let mut r1 = up(&nas, &t1, None, &any).unwrap().records;
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &b, "taken over by MacBook-Air", DAY + 700).unwrap().unwrap();
        let mut r2 = up(&nas, &t2, None, &any).unwrap().records;
        let x = journal::write(&nas, &built(1, 5, "6-1-1", "k5")).unwrap();
        let stale = |_: &Entry, _: &Records| Err::<(), String>("6-1-1 leased again since".into());
        let m = merge(&nas, &mut r1, std::slice::from_ref(&x), &stale);
        r1.save(&nas).unwrap();
        for (k, why) in &m.refused {
            journal::note_refusal(&nas, k, why).unwrap();
        }
        // Told of it by its member, or (its member gone) listing it: its own check decides.
        assert_eq!(journal::list(&nas, None).unwrap(), std::slice::from_ref(&x));
        let m2 = merge(&nas, &mut r2.clone(), std::slice::from_ref(&x), &any);
        assert_eq!(m2.applied, std::slice::from_ref(&x), "{m2:?}");
        let m3 = merge(&nas, &mut r2, &journal::list(&nas, None).unwrap(), &any);
        assert_eq!(m3.applied, std::slice::from_ref(&x), "{m3:?}");
        assert_eq!(r2.keys.unit.get("6-1-1").map(String::as_str), Some("k5"));
    }

    /// A NAS on which term 2's first snapshot's save lands but answers an error, and reads it as
    /// missing a moment after (a stale read).
    #[derive(Default)]
    struct LostSave {
        mem: Mem,
        lost: Cell<bool>,
        stale: Cell<bool>,
    }

    impl Nas for LostSave {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            self.mem.create_new(path, bytes)
        }
        fn write_whole(&self, p: &str, bytes: &[u8]) -> Result<()> {
            self.mem.write_whole(p, bytes)?;
            if p == path(2) && self.lost.replace(false) {
                self.stale.set(true);
                bail!("write {p}: the answer was lost");
            }
            Ok(())
        }
        fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
            if p == path(2) && self.stale.replace(false) {
                return Ok(None);
            }
            self.mem.read(p)
        }
        fn exists(&self, p: &str) -> Result<bool> {
            self.mem.exists(p)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.mem.list(dir)
        }
        fn remove(&self, p: &str) -> Result<()> {
            self.mem.remove(p)
        }
    }

    #[test]
    fn a_take_up_tried_again_numbers_its_snapshot_on() {
        // Term 2's first save lands and answers an error; the next try reads it as missing.
        // (Review L1: the try started again from term 1's records at 0, and two versions of term
        // 2's records, with other entries, were both numbered 1.)
        let nas = LostSave::default();
        let (a, b) = (lead("development"), Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "development".into() });
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        up(&nas, &t1, None, &any).unwrap();
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &b, "taken over by MacBook-Air", DAY + 700).unwrap().unwrap();
        journal::write(&nas, &built(1, 1, "6-1-1", "k1")).unwrap();
        nas.lost.set(true);
        let mut own = None;
        assert!(take_up(&nas, &t2, &mut own, &journal::list(&nas, None).unwrap(), &any).is_err());
        let first = Records::load(&nas.mem, 2).unwrap().unwrap();
        assert_eq!(own.as_ref().map(|r| (r.term, r.seq)), Some((2, 1)), "kept as tried");
        journal::write(&nas, &built(1, 2, "6-1-2", "k2")).unwrap();
        let again = take_up(&nas, &t2, &mut own, &journal::list(&nas, None).unwrap(), &any).unwrap().records;
        assert_eq!((first.seq, again.seq, first.reflected.len(), again.reflected.len()), (1, 2, 1, 2));
        assert!(own.is_none());
    }

    #[test]
    fn term_ones_first_snapshot_with_a_hole_is_taken_up_from_todays_files() {
        // Its bytes landed out of order and its maker stopped: a hole of zeros mid-file, not a
        // short end. (Review L5: it couldn't be read, nor term 1 taken up.)
        let nas = Mem::default();
        nas.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        let mut b = serde_json::to_vec(&Records { seq: 1, ..Records::today(&nas).unwrap() }).unwrap();
        let n = b.len();
        b[n / 4..n / 2].fill(0);
        nas.create_new(&path(1), &b).unwrap();
        let a = lead("development");
        let t1 = term::bootstrap(&nas, &a, DAY, true).unwrap().unwrap();
        let taken = up(&nas, &t1, None, &any).unwrap();
        assert_eq!((taken.records.seq, taken.records.manifest.len()), (2, 1));
        assert_eq!(Records::load(&nas, 1).unwrap().map(|r| r.seq), Some(2), "replaced whole");
    }
}
