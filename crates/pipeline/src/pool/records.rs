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
//! (`first`), and they're written after each of its snapshots, for today's readers. The pool never
//! reads them again: three files can be read half rewritten, one stale and another not, and a Mac
//! that read term 1's snapshot as missing a moment before it was made would be sent to them. So
//! term 1's snapshot exists before any Mac knows of term 1, and reads whole like any term's.
//!
//! Taking up a term (`take_up`) starts from the records of the term before (the newest snapshot a
//! read finds, or the one a handover names), replays every journal entry they don't name in
//! (term, lease) order, and saves that as the new term's first snapshot. A stale read only sends an
//! entry to the replay, or to its member's telling the new lead again (crate::pool::journal::Mine):
//! none is lost (invariant 3).

use super::journal::{self, Entry};
use super::nas::{short, Nas};
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
    /// The journal entries applied, by key (crate::pool::journal::Entry::key).
    #[serde(default)]
    pub reflected: BTreeSet<String>,
    /// The journal entries refused, with why.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rejected: BTreeMap<String, String>,
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

impl Records {
    /// Term `term`'s snapshot, as read now; None when it has none (its lead never saved one, or the
    /// read is stale). An error when what's there can't be read now, or parsed.
    pub fn load(nas: &dyn Nas, term: u64) -> Result<Option<Records>> {
        let Some(b) = nas.read(&path(term))? else { return Ok(None) };
        let r: Records = serde_json::from_slice(&b).with_context(|| format!("parse {}", path(term)))?;
        ensure!(r.term == term, "{} holds term {}'s records", path(term), r.term);
        Ok(Some(r))
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

    /// Whether journal entry `key` is in them: applied, or refused.
    pub fn handles(&self, key: &str) -> bool {
        self.reflected.contains(key) || self.rejected.contains_key(key)
    }

    /// Applies journal entry `e` as crate::handoff's merge did: its manifest changes, its uploads
    /// pending and checked, its done record in the keys, its raw archives to name; and names it.
    pub fn apply(&mut self, e: &Entry) {
        let h = &e.handoff;
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
        self.reflected.insert(e.key());
    }

    /// Names journal entry `key` refused, with why.
    pub fn refuse(&mut self, key: &str, why: &str) {
        self.rejected.insert(key.to_string(), why.to_string());
    }

    /// Forgets the journal entries of the days before `day` (YYYY-MM-DD), once GC has removed those
    /// days from the journal (§7.3: every snapshot since names their entries) and members have
    /// forgotten them (crate::pool::journal::Mine::forget_before): none is listed or told again.
    pub fn forget_before(&mut self, day: &str) {
        self.reflected.retain(|k| k.as_str() >= day);
        self.rejected.retain(|k, _| k.as_str() >= day);
    }
}

/// Makes term 1's first snapshot from today's three files, unless it has one: before term 1 is
/// made (crate::pool::term::bootstrap), so every Mac that learns of term 1 reads its snapshot whole.
/// (Made with create-new: of two Macs making it, the first's stays.)
pub fn first(nas: &dyn Nas) -> Result<()> {
    if nas.exists(&path(1))? {
        return Ok(());
    }
    let r = Records { seq: 1, ..Records::today(nas)? };
    nas.create_new(&path(1), &serde_json::to_vec(&r)?)?;
    Ok(())
}

/// What a merge did, by journal key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Applied to the records.
    pub applied: Vec<String>,
    /// Refused, with why: to set aside once the records naming them are saved.
    pub refused: Vec<(String, String)>,
    /// Not readable whole now (being written, not seen yet, or a read that failed): to merge later.
    pub waiting: Vec<String>,
}

/// Merges the journal entries `keys` the records don't name yet: each read, checked and applied,
/// or refused, in (term, lease) order. Nothing is written: the caller saves the records, then sets
/// the refused aside (crate::pool::journal::set_aside) and acknowledges what the saved records
/// name.
pub fn merge(nas: &dyn Nas, r: &mut Records, keys: &[String], check: Check) -> Merged {
    let mut out = Merged::default();
    let mut read: BTreeMap<(journal::LeaseId, String), Entry> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for k in keys {
        if r.handles(k) || !seen.insert(k.as_str()) {
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
            Ok(journal::Read::SetAside(why)) => {
                r.refuse(k, &why);
                out.refused.push((k.clone(), why));
            }
            Ok(journal::Read::Short | journal::Read::Missing) | Err(_) => out.waiting.push(k.clone()),
        }
    }
    for ((_, k), e) in read {
        match check(&e, r) {
            Ok(()) => {
                r.apply(&e);
                out.applied.push(k);
            }
            Err(why) => {
                r.refuse(&k, &why);
                out.refused.push((k, why));
            }
        }
    }
    out
}

/// What taking up a term gave.
#[derive(Clone, Debug)]
pub struct TakenUp {
    /// The term's records, saved as its first snapshot.
    pub records: Records,
    /// The journal entries replayed into them, refused (and set aside), or not readable whole yet.
    pub merged: Merged,
}

/// Takes up term `t` (§6.2): the records of the newest term before it with a snapshot (`own`, for
/// the term they're of: a lead re-asserting or taking back has its own and needn't read them), with
/// every journal entry they don't name replayed in (term, lease) order, `check`ed first; saved as
/// `t`'s first snapshot, and the refused entries set aside. `since` leaves the journal's earlier
/// days unlisted (an entry there the records lack comes back when its member tells the new lead
/// again). A handover's term (`t.seq`) starts from the snapshot it names: an error until a read
/// gives that one (a stale read gives an older), to try again shortly. A snapshot of `t` itself (an
/// earlier try's that landed) is started from as it is.
pub fn take_up(nas: &dyn Nas, t: &Term, own: Option<Records>, since: Option<&str>, check: Check) -> Result<TakenUp> {
    let mut own = own;
    let mut base = None;
    let mut f = t.term;
    while f >= 1 {
        if let Some(o) = own.take_if(|o| o.term == f) {
            base = Some(o);
            break;
        }
        let loaded = match Records::load(nas, f) {
            // Term 1's first snapshot, still being made (its maker stopped midway): today's files,
            // as `first` makes it, which nothing has written since (term 1's lead, taking it up
            // now, is their only other writer).
            Err(e) if f == 1 && t.term == 1 => match nas.read(&path(1))? {
                Some(b) if short(&b) => Some(Records { seq: 1, ..Records::today(nas)? }),
                _ => return Err(e),
            },
            r => r?,
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
    let keys = journal::list(nas, since)?;
    let merged = merge(nas, &mut r, &keys, check);
    r.save(nas).with_context(|| format!("save term {}'s records", t.term))?;
    for (k, why) in &merged.refused {
        // (Named refused in the records saved: one not set aside now is passed over.)
        journal::set_aside(nas, k, why).ok();
    }
    Ok(TakenUp { records: r, merged })
}

#[cfg(test)]
mod tests {
    use super::super::journal::LeaseId;
    use super::super::nas::Mem;
    use super::super::term::{self, Current};
    use super::super::Member;
    use super::*;
    use crate::handoff::Handoff;

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
        r.apply(&built(1, 1, "6-1-2", "k1"));
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
        let up = take_up(&stopped, &t1, None, None, &any).unwrap();
        assert_eq!((up.records.seq, up.records.manifest.len()), (2, 1));
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
        assert_eq!(m.applied, [b.key(), a.key()]);
        assert_eq!(m.refused, [(bad.key(), "writes outside its step's names".to_string())]);
        assert_eq!(m.waiting, [short]);
        assert_eq!(r.keys.unit.get("6-1-1").map(String::as_str), Some("k9"));
        assert_eq!(r.manifest.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.k9.base"));
        assert!(!r.manifest.contains_key("base/6-1-3") && r.handles(&bad.key()) && r.pending.len() == 2);
        // Told again: nothing more.
        assert_eq!(merge(&nas, &mut r, &keys, &check), Merged::default());
        // The days GC removed, forgotten.
        r.refuse("2026-10-05/2-1", "no such step");
        r.forget_before("2026-10-06");
        assert!(!r.handles("2026-10-05/2-1") && r.handles(&a.key()) && r.handles(&bad.key()));
    }

    #[test]
    fn a_new_lead_takes_up_from_the_term_before_and_replays_what_it_lacks() {
        let nas = Mem::default();
        let a = lead("20261005-2202-61eb22c");
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let up1 = take_up(&nas, &t1, None, None, &any).unwrap();
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
        let up2 = take_up(&nas, &t2, None, None, &any).unwrap();
        assert_eq!(up2.merged.applied, [e2.key()]);
        assert_eq!((up2.records.term, up2.records.seq), (2, 1));
        assert!(up2.records.handles(&e1.key()) && up2.records.handles(&e2.key()));
        assert_eq!(Records::load(&nas, 2).unwrap(), Some(up2.records.clone()));
        // Term 3 made, its lead gone before saving anything: term 4 starts from term 2's.
        let t3 = term::claim(&nas, &Current { term: 2, lead: Some(t2.clone()) }, &a, "taken over", DAY + 120).unwrap().unwrap();
        let t4 = term::claim(&nas, &Current { term: 3, lead: Some(t3) }, &b, "taken over", DAY + 900).unwrap().unwrap();
        let up4 = take_up(&nas, &t4, None, None, &any).unwrap();
        assert_eq!((up4.records.term, up4.records.reflected.len()), (4, 2));
        // Tried again after its save landed: from its own snapshot, its number going on.
        let again = take_up(&nas, &t4, None, None, &any).unwrap();
        assert_eq!((again.records.term, again.records.seq), (4, 2));
    }

    #[test]
    fn a_handover_is_taken_up_only_from_the_snapshot_it_names() {
        let nas = Mem::default();
        let a = lead("20261005-2202-61eb22c");
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let mut r = take_up(&nas, &t1, None, None, &any).unwrap().records;
        let e = built(1, 1, "6-1-1", "k1");
        let k = journal::write(&nas, &e).unwrap();
        merge(&nas, &mut r, std::slice::from_ref(&k), &any);
        r.save(&nas).unwrap();
        let old = nas.read(&path(1)).unwrap().unwrap();
        let mut r2 = r.clone();
        r2.save(&nas).unwrap();
        // Handed over at seq 3 (term 1's snapshot read as at 2: a stale read): not yet.
        let b = Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "20261005-2202-61eb22c".into() };
        let mut t2 = Term::after(&Current { term: 1, lead: Some(t1) }, &b, "handed over by Mac-mini", DAY + 60).unwrap();
        t2.seq = Some(r2.seq);
        assert!(term::make(&nas, &t2).unwrap());
        let fresh = nas.read(&path(1)).unwrap().unwrap();
        nas.write_whole(&path(1), &old).unwrap();
        assert!(take_up(&nas, &t2, None, None, &any).is_err());
        assert_eq!(Records::load(&nas, 2).unwrap(), None, "nothing saved");
        // Read whole: taken up.
        nas.write_whole(&path(1), &fresh).unwrap();
        let up = take_up(&nas, &t2, None, None, &any).unwrap();
        assert!(up.records.handles(&k) && up.merged.applied.is_empty());
        // A lead re-asserting starts from its own records, unread.
        let mut own = up.records.clone();
        own.apply(&built(2, 4, "6-1-4", "k4"));
        let t3 = term::claim(&nas, &Current { term: 2, lead: Some(t2) }, &b, "re-asserted after a gap", DAY + 900).unwrap().unwrap();
        let up3 = take_up(&nas, &t3, Some(own), None, &any).unwrap();
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
        let mut r1 = take_up(&nas, &t1, None, None, &any).unwrap().records;
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &b, "taken over by MacBook-Air", DAY + 700).unwrap().unwrap();
        let mut r2 = take_up(&nas, &t2, None, None, &any).unwrap().records;
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
    fn a_refused_entry_is_set_aside_once_its_refusal_is_saved() {
        let nas = Mem::default();
        let a = lead("development");
        let t1 = term::bootstrap(&nas, &a, DAY, false).unwrap().unwrap();
        let bad = Entry { step: "bogus".into(), ..built(1, 3, "6-1-3", "kx") };
        let k = journal::write(&nas, &bad).unwrap();
        let check = |e: &Entry, _: &Records| if e.step == "bogus" { Err("no such step".to_string()) } else { Ok(()) };
        let up = take_up(&nas, &t1, None, None, &check).unwrap();
        assert_eq!(up.records.rejected.get(&k).map(String::as_str), Some("no such step"));
        assert!(journal::list(&nas, None).unwrap().is_empty());
        // Told again by its member, to a lead whose records lack it: refused again, as set aside.
        let mut r = Records { term: 2, ..Default::default() };
        let m = merge(&nas, &mut r, std::slice::from_ref(&k), &check);
        assert_eq!(m.refused, [(k, "no such step".to_string())]);
    }
}
