//! The journal (docs/pool.md §7.3): every job's hand-off, written by the member whose job made it
//! straight to the NAS, `state/journal/<day>/<term>-<n>.json`, written whole (a temporary name,
//! renamed: a reader never sees it half written) by that member alone, named after the job's
//! lease (`<term>-<n>`: the term it was leased in, and its number there, unique by
//! construction). The lead merges entries into its term's records (crate::pool::records) as
//! members tell it of them, and lists the journal after taking up a term and now and then,
//! replaying what its records don't name. It's a log, not a queue: nothing is removed on the merge
//! path, so a lead taking over finds every result. An entry a lead refuses stays where it is, its
//! why noted beside it, under `rejected/`, for the owner: a lead whose records don't name it (the
//! refusal a lead's no longer current) checks it itself, told of it or listing it.
//!
//! An entry is safe once written, but it's only in the records once a lead has merged it, so its
//! member keeps it whole until it's written (its key fixed by its first try), tells the lead of
//! the term it knows is current of it until that lead acknowledges it, and tells each later
//! term's lead again (`Mine`): a take-up that read an older snapshot, or listed the journal stale,
//! or a lead that acknowledged it while a later term began without its knowing, only delays it
//! (invariant 3).

use super::nas::{short, Created, Nas};
use crate::handoff::Handoff;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// The journal's folder.
pub const DIR: &str = "state/journal";
/// Where a refused entry's why is noted, under its key (`<key>.why`).
pub const REJECTED: &str = "state/journal/rejected";

/// A lease's id (§6.4, §7.5): the term it was granted in and its number there, `<term>-<n>`, so no
/// two leads ever give the same one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseId {
    /// The term it was granted in.
    pub term: u64,
    /// Its number in that term.
    pub n: u64,
}

impl fmt::Display for LeaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.term, self.n)
    }
}

impl FromStr for LeaseId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<LeaseId> {
        let Some((t, n)) = s.split_once('-') else { bail!("{s:?} isn't a lease: <term>-<n>") };
        match (t.parse(), n.parse()) {
            (Ok(term), Ok(n)) => Ok(LeaseId { term, n }),
            _ => bail!("{s:?} isn't a lease: <term>-<n>"),
        }
    }
}

impl Serialize for LeaseId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for LeaseId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<LeaseId, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// A journal entry: a job's hand-off and what it was for.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// The member whose job made it.
    pub member: String,
    /// The job's lease.
    pub lease: LeaseId,
    /// The lease's step (what the lead checks the hand-off's writes against).
    pub step: String,
    /// What the job changed: manifest changes, uploads, its done record, raw tiles' archives.
    pub handoff: Handoff,
    /// When it was written (unix seconds, its member's clock): its day.
    pub at: u64,
}

impl Entry {
    /// Its key, `<day>/<term>-<n>`: what records name it by, and where it is under the journal;
    /// None when its time isn't a day (a damaged entry's).
    pub fn key(&self) -> Option<String> {
        Some(format!("{}/{}", day(self.at)?, self.lease))
    }
}

/// The UTC day of unix time `at`, `YYYY-MM-DD`; None past the year 9999 (a time no member's clock
/// gives: a damaged entry's).
pub fn day(at: u64) -> Option<String> {
    let t = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(at))?;
    Some(crate::agent::backup::format_day(t)).filter(|d| is_day(d))
}

/// The path of the entry with key `key`.
pub fn path(key: &str) -> String {
    format!("{DIR}/{key}.json")
}

/// Whether `name` is a day folder's (`YYYY-MM-DD`).
fn is_day(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Writes `e` to the journal, whole, by a temporary name (a reader sees all of it or none); its
/// key. The file is this member's alone (its lease's): an earlier try's that landed is left as it
/// is, and another entry under the same lease is an error.
pub fn write(nas: &dyn Nas, e: &Entry) -> Result<String> {
    let key = e.key().with_context(|| format!("lease {}'s entry: its time {} isn't a day", e.lease, e.at))?;
    let p = path(&key);
    let b = serde_json::to_vec(e)?;
    match nas.read(&p)?.map(|got| serde_json::from_slice::<Entry>(&got)) {
        // (The same entry, whatever its bytes: an earlier write's whose answer was lost, or one an
        // app of another version wrote, its fields in another order or some left to their
        // defaults.)
        Some(Ok(got)) if serde_json::to_value(&got)? == serde_json::to_value(e)? => return Ok(key),
        Some(Ok(_)) => bail!("{p} holds another entry of lease {}", e.lease),
        _ => {}
    }
    nas.write_whole(&p, &b)?;
    Ok(key)
}

/// A journal entry as read.
#[derive(Clone, Debug)]
pub enum Read {
    Entry(Entry),
    /// There, but not whole (written in place, not whole, or a read cut short).
    Short,
    /// Not there as read now (not yet, a stale read, or a day GC removed).
    Missing,
    /// Can't be an entry: not one, or another entry's; why.
    Damaged(String),
}

/// What bytes read under key `key` are.
fn parse(b: &[u8], key: &str) -> Read {
    if short(b) {
        return Read::Short;
    }
    match serde_json::from_slice::<Entry>(b) {
        Ok(e) => match e.key() {
            Some(k) if k == key => Read::Entry(e),
            Some(k) => Read::Damaged(format!("it's {k}'s, under {key}")),
            None => Read::Damaged(format!("its time {} isn't a day", e.at)),
        },
        Err(err) => Read::Damaged(format!("it isn't an entry: {err}")),
    }
}

/// The entry with key `key`.
pub fn read(nas: &dyn Nas, key: &str) -> Result<Read> {
    Ok(match nas.read(&path(key))? {
        Some(b) => parse(&b, key),
        None => Read::Missing,
    })
}

/// Every entry's key in the journal, by day (oldest first) and then name; `since` leaves out the
/// days before it. Lists each day's folder, `rejected/` left out: slow, off the lead's loop.
pub fn list(nas: &dyn Nas, since: Option<&str>) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    for d in nas.list(DIR)? {
        if !is_day(&d) || since.is_some_and(|s| d.as_str() < s) {
            continue;
        }
        for n in nas.list(&format!("{DIR}/{d}"))? {
            if let Some(stem) = n.strip_suffix(".json").filter(|s| s.parse::<LeaseId>().is_ok()) {
                keys.push(format!("{d}/{stem}"));
            }
        }
    }
    Ok(keys)
}

/// Notes why entry `key` was refused, beside the journal (`rejected/<key>.why`), for the owner,
/// once records naming it refused are saved; the first refusal's why is kept. The entry stays in
/// its day: a refusal can be a lead's that's no longer current (its check depends on its state),
/// and a lead whose records don't name the entry checks it itself, told of it or listing it.
pub fn note_refusal(nas: &dyn Nas, key: &str, why: &str) -> Result<()> {
    let p = format!("{REJECTED}/{key}.why");
    if let Created::Unwritten(_) = nas.create_new(&p, why.as_bytes())? {
        nas.write_whole(&p, why.as_bytes())?;
    }
    Ok(())
}

/// Why entry `key` was refused first, as noted; None when it wasn't.
pub fn refusal(nas: &dyn Nas, key: &str) -> Result<Option<String>> {
    Ok(nas.read(&format!("{REJECTED}/{key}.why"))?.map(|w| String::from_utf8_lossy(&w).trim().to_string()))
}

/// A member's own journal entries, kept in the agent's local folder: those not written yet, whole
/// (so a try after midnight writes them under the key their first try had), and those written,
/// each with the term whose lead acknowledged it (applied, passed over or refused). Each loop the
/// member writes what's unwritten and tells the lead of the term it knows is current of every entry
/// that lead hasn't acknowledged, so after a change of lead it tells the new one of them all: the
/// new lead answers at once for those its records name, and merges the others.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mine {
    entries: BTreeMap<String, Option<u64>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    unwritten: BTreeMap<String, Entry>,
}

impl Mine {
    /// Keeps job's hand-off `e` to write (`write`): its key, fixed now. An error when its time
    /// isn't a day.
    pub fn add(&mut self, e: Entry) -> Result<String> {
        let key = e.key().with_context(|| format!("lease {}'s entry: its time {} isn't a day", e.lease, e.at))?;
        if !self.entries.contains_key(&key) {
            self.unwritten.insert(key.clone(), e);
        }
        Ok(key)
    }

    /// Writes the entries not written yet: those that couldn't be, with why (kept, to try again).
    pub fn write(&mut self, nas: &dyn Nas) -> Vec<(String, anyhow::Error)> {
        self.write_while(nas, &|| true)
    }

    /// `write`, while `more()` says so (asked before each): the rest are kept, to write later.
    pub fn write_while(&mut self, nas: &dyn Nas, more: &dyn Fn() -> bool) -> Vec<(String, anyhow::Error)> {
        let mut failed = Vec::new();
        for (k, e) in std::mem::take(&mut self.unwritten) {
            if !more() {
                self.unwritten.insert(k, e);
                continue;
            }
            match write(nas, &e) {
                Ok(_) => self.wrote(&k),
                Err(err) => {
                    failed.push((k.clone(), err));
                    self.unwritten.insert(k, e);
                }
            }
        }
        failed
    }

    /// The entries kept and not written yet, by key.
    pub fn unwritten(&self) -> impl Iterator<Item = &String> {
        self.unwritten.keys()
    }

    /// Its hand-offs not written yet, alone: what another member id (this Mac's member file lost)
    /// keeps of them, the same bytes whoever writes them.
    pub fn unwritten_only(self) -> Mine {
        Mine { entries: BTreeMap::new(), unwritten: self.unwritten }
    }

    /// Notes entry `key` written.
    pub fn wrote(&mut self, key: &str) {
        self.entries.entry(key.to_string()).or_insert(None);
    }

    /// Notes entry `key` acknowledged by the lead of `term`.
    pub fn acked(&mut self, key: &str, term: u64) {
        if let Some(t) = self.entries.get_mut(key) {
            *t = Some(t.map_or(term, |was| was.max(term)));
        }
    }

    /// The entries to tell the lead of `term` of: those it hasn't acknowledged.
    pub fn to_tell(&self, term: u64) -> Vec<String> {
        self.entries.iter().filter(|(_, t)| **t != Some(term)).map(|(k, _)| k.clone()).collect()
    }

    /// Forgets the entries of the days before `day` (YYYY-MM-DD) a lead acknowledged: GC removed
    /// those days from the journal, every snapshot since naming them. One not acknowledged yet is
    /// kept, told until it is (written late, into a day GC had removed, it's in no snapshot).
    pub fn forget_before(&mut self, day: &str) {
        self.entries.retain(|k, t| k.as_str() >= day || t.is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::super::nas::Mem;
    use super::super::records::{self, Records};
    use super::*;
    use std::cell::RefCell;

    fn entry(term: u64, n: u64, at: u64) -> Entry {
        let h = Handoff { changes: [("base/6-1-1".to_string(), Some("base/6-1-1.1111111111111111.base".to_string()))].into(), done: Some(("unit".into(), vec![("6/1/1".into(), "k1".into())])), ..Default::default() };
        Entry { member: "m-000000000000000a".into(), lease: LeaseId { term, n }, step: "unit".into(), handoff: h, at }
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    #[test]
    fn leases_read_back_and_order_by_term_then_number() {
        let l: LeaseId = "12-345".parse().unwrap();
        assert_eq!(l, LeaseId { term: 12, n: 345 });
        assert_eq!(serde_json::to_string(&l).unwrap(), "\"12-345\"");
        assert_eq!(serde_json::from_str::<LeaseId>("\"12-345\"").unwrap(), l);
        assert!("12".parse::<LeaseId>().is_err() && "a-1".parse::<LeaseId>().is_err());
        let mut v = [LeaseId { term: 3, n: 1 }, LeaseId { term: 2, n: 10 }, LeaseId { term: 2, n: 9 }];
        v.sort();
        assert_eq!(v.map(|l| l.to_string()), ["2-9", "2-10", "3-1"]);
    }

    #[test]
    fn an_entry_is_written_once_and_read_back_whole() {
        let nas = Mem::default();
        let e = entry(4, 17, 1_791_300_000);
        let key = write(&nas, &e).unwrap();
        assert_eq!(key, "2026-10-06/4-17");
        assert!(matches!(read(&nas, &key).unwrap(), Read::Entry(x) if x.lease == e.lease));
        // Written again (its first call's answer lost): the same.
        assert_eq!(write(&nas, &e).unwrap(), key);
        // The same entry in other bytes (another app's): the same, left as it is. (Re-review 2,
        // L2.)
        let pretty = serde_json::to_vec_pretty(&e).unwrap();
        nas.write_whole(&path(&key), &pretty).unwrap();
        assert_eq!(write(&nas, &e).unwrap(), key);
        assert_eq!(nas.read(&path(&key)).unwrap().unwrap(), pretty, "not written again");
        // Another under the same lease: an error.
        let mut other = e.clone();
        other.step = "pois".into();
        assert!(write(&nas, &other).is_err());
        // One left short (by a write in place cut short): not whole, then written over.
        let f = entry(4, 18, 1_791_300_000);
        let fk = f.key().unwrap();
        nas.create_new(&path(&fk), b"{\"member\":").unwrap();
        assert!(matches!(read(&nas, &fk).unwrap(), Read::Short));
        nas.write_whole(&path(&fk), b"").unwrap();
        assert!(matches!(read(&nas, &fk).unwrap(), Read::Short));
        write(&nas, &f).unwrap();
        assert!(matches!(read(&nas, &fk).unwrap(), Read::Entry(_)));
        // Not there; not an entry; another's.
        assert!(matches!(read(&nas, "2026-10-06/4-19").unwrap(), Read::Missing));
        nas.write_whole(&path("2026-10-06/4-20"), b"[1, 2]").unwrap();
        assert!(matches!(read(&nas, "2026-10-06/4-20").unwrap(), Read::Damaged(_)));
        nas.write_whole(&path("2026-10-06/4-21"), &serde_json::to_vec(&e).unwrap()).unwrap();
        assert!(matches!(read(&nas, "2026-10-06/4-21").unwrap(), Read::Damaged(_)));
    }

    #[test]
    fn the_listing_takes_every_day_and_refused_entries_stay() {
        let nas = Mem::default();
        for (n, at) in [(1, 1_791_200_000), (2, 1_791_300_000), (3, 1_791_300_100)] {
            write(&nas, &entry(5, n, at)).unwrap();
        }
        nas.write_whole("state/journal/2026-10-06/notes.txt", b"").unwrap();
        assert_eq!(list(&nas, None).unwrap(), ["2026-10-05/5-1", "2026-10-06/5-2", "2026-10-06/5-3"]);
        assert_eq!(list(&nas, Some("2026-10-06")).unwrap(), ["2026-10-06/5-2", "2026-10-06/5-3"]);
        note_refusal(&nas, "2026-10-06/5-2", "writes outside its step's names").unwrap();
        assert_eq!(list(&nas, None).unwrap(), ["2026-10-05/5-1", "2026-10-06/5-2", "2026-10-06/5-3"], "still listed");
        assert!(matches!(read(&nas, "2026-10-06/5-2").unwrap(), Read::Entry(e) if e.lease == LeaseId { term: 5, n: 2 }));
        assert_eq!(refusal(&nas, "2026-10-06/5-2").unwrap().as_deref(), Some("writes outside its step's names"));
        // Noted again (another lead's, or a first try cut short): the first why kept.
        note_refusal(&nas, "2026-10-06/5-2", "again").unwrap();
        assert_eq!(refusal(&nas, "2026-10-06/5-2").unwrap().as_deref(), Some("writes outside its step's names"));
        assert_eq!(refusal(&nas, "2026-10-06/5-3").unwrap(), None);
    }

    #[test]
    fn a_member_tells_each_new_lead_of_what_it_wrote() {
        let mut m = Mine::default();
        m.wrote("2026-10-05/3-1");
        m.wrote("2026-10-06/3-2");
        assert_eq!(m.to_tell(3), ["2026-10-05/3-1", "2026-10-06/3-2"]);
        m.acked("2026-10-05/3-1", 3);
        m.acked("2026-10-06/9-9", 3);
        assert_eq!(m.to_tell(3), ["2026-10-06/3-2"]);
        // A new lead: told of them all again; an older lead's late answer counts for nothing.
        m.acked("2026-10-06/3-2", 3);
        assert_eq!(m.to_tell(4), ["2026-10-05/3-1", "2026-10-06/3-2"]);
        m.acked("2026-10-05/3-1", 4);
        m.acked("2026-10-05/3-1", 3);
        assert_eq!(m.to_tell(4), ["2026-10-06/3-2"]);
        // The days GC removed: what a lead acknowledged forgotten; one not yet, kept.
        m.wrote("2026-10-04/3-0");
        m.forget_before("2026-10-06");
        assert_eq!(m.to_tell(5), ["2026-10-04/3-0", "2026-10-06/3-2"]);
    }

    #[test]
    fn a_corrupt_entry_time_is_damage_not_a_panic() {
        // (Review M2: `day` added the time to the epoch unchecked, and every reader panicked.)
        let nas = Mem::default();
        let key = write(&nas, &entry(1, 1, 1_791_300_000)).unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&nas.read(&path(&key)).unwrap().unwrap()).unwrap();
        v["at"] = serde_json::json!(u64::MAX);
        nas.write_whole(&path(&key), &serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(matches!(read(&nas, &key).unwrap(), Read::Damaged(why) if why.contains("isn't a day")));
        let mut r = Records { term: 2, ..Default::default() };
        let m = records::merge(&nas, &mut r, std::slice::from_ref(&key), &any);
        assert_eq!(m.refused.len(), 1, "{m:?}");
        // Nor can one be written, or kept to write.
        let bad = entry(1, 2, u64::MAX);
        assert_eq!(bad.key(), None);
        assert_eq!(day(253_402_300_800), None, "the year 10000");
        assert!(write(&nas, &bad).is_err() && Mine::default().add(bad).is_err());
    }

    /// What runs in between, as another Mac.
    type Between = Box<dyn Fn(&Torn)>;

    /// A NAS on which a file being made reads with a hole of zeros until its last bytes land, a
    /// temporary file isn't seen under the name it's renamed to; `during` runs in between (another
    /// Mac's read).
    #[derive(Default)]
    struct Torn {
        mem: Mem,
        during: RefCell<Option<Between>>,
    }

    impl Torn {
        fn between(&self) {
            let f = self.during.borrow_mut().take();
            if let Some(f) = f {
                f(self);
            }
        }
    }

    impl Nas for Torn {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            let mut holed = bytes.to_vec();
            let n = holed.len();
            holed[n / 4..n / 2].fill(0);
            if let Created::There = self.mem.create_new(path, &holed)? {
                return Ok(Created::There);
            }
            self.between();
            self.mem.write_whole(path, bytes)?;
            Ok(Created::Made)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.between();
            self.mem.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.mem.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.mem.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.mem.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.mem.remove(path)
        }
    }

    #[test]
    fn an_entry_is_never_read_half_written() {
        // A lead reads an entry while its member writes it. (Review M3: it was made with
        // create-new and its bytes written after, so a read between could find a hole of zeros:
        // refused as damaged, and set aside for good.)
        let nas = Torn::default();
        let e = entry(4, 2, 1_791_300_000);
        let key = e.key().unwrap();
        let r = std::rc::Rc::new(RefCell::new(Records { term: 4, ..Default::default() }));
        let seen = std::rc::Rc::new(RefCell::new(None));
        {
            let (r, seen, key) = (r.clone(), seen.clone(), key.clone());
            *nas.during.borrow_mut() = Some(Box::new(move |n: &Torn| {
                *seen.borrow_mut() = Some(records::merge(n, &mut r.borrow_mut(), std::slice::from_ref(&key), &any));
            }));
        }
        write(&nas, &e).unwrap();
        let during = seen.borrow_mut().take().expect("a read while it was written");
        assert!(during.refused.is_empty() && during.waiting == [key.clone()], "{during:?}");
        let after = records::merge(&nas, &mut r.borrow_mut(), std::slice::from_ref(&key), &any);
        assert_eq!(after.applied, [key]);
    }

    #[test]
    fn an_entry_kept_to_write_keeps_its_key_past_midnight() {
        // Its job ended a second before midnight; the NAS away, it's written after. (Review L4: an
        // entry made again for the retry took the new day, another key for the same lease.)
        let nas = Mem::default();
        let mut mine = Mine::default();
        let e = entry(4, 30, 1_791_331_199);
        let key = mine.add(e.clone()).unwrap();
        assert_eq!(key, "2026-10-06/4-30");
        assert!(mine.to_tell(4).is_empty(), "not told before it's written");
        let saved: Mine = serde_json::from_slice(&serde_json::to_vec(&mine).unwrap()).unwrap();
        assert_eq!(saved.unwritten().collect::<Vec<_>>(), [&key], "kept whole in the agent's folder");
        let mut mine = saved;
        assert!(mine.write(&nas).is_empty());
        assert_eq!(list(&nas, None).unwrap(), std::slice::from_ref(&key));
        assert_eq!(mine.to_tell(4), std::slice::from_ref(&key));
        assert_eq!(mine.unwritten().count(), 0);
        // Added again: nothing more.
        mine.add(e).unwrap();
        assert_eq!(mine.unwritten().count(), 0);
    }
}
