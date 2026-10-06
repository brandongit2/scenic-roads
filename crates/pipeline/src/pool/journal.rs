//! The journal (docs/pool.md §7.3): every job's hand-off, written by the member whose job made it
//! straight to the NAS, `state/journal/<day>/<term>-<n>.json`, made with create-new and written
//! whole, named after the job's lease (`<term>-<n>`: the term it was leased in, and its number
//! there, unique by construction). The lead merges entries into its term's records
//! (crate::pool::records) as members tell it of them, and a new lead lists the journal at take-up
//! and replays what its records don't name. It's a log, not a queue: nothing is removed on the
//! merge path, so a lead taking over finds every result; an entry the lead refuses moves to
//! `rejected/`, with why.
//!
//! An entry is safe once written, but it's only in the records once a lead has merged it, so its
//! member tells the lead of the term it knows is current of it until that lead acknowledges it,
//! and tells each later term's lead again (`Mine`): a take-up that read an older snapshot, or
//! listed the journal stale, or a lead that acknowledged it while a later term began without its
//! knowing, only delays it (invariant 3).

use super::nas::{short, Nas};
use crate::handoff::Handoff;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// The journal's folder.
pub const DIR: &str = "state/journal";
/// Where refused entries go, under their keys, each with a `.why`.
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
    /// Its key, `<day>/<term>-<n>`: what records name it by, and where it is under the journal.
    pub fn key(&self) -> String {
        format!("{}/{}", day(self.at), self.lease)
    }
}

/// The UTC day of unix time `at`, `YYYY-MM-DD`.
pub fn day(at: u64) -> String {
    crate::agent::backup::format_day(std::time::UNIX_EPOCH + std::time::Duration::from_secs(at))
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

/// Writes `e` to the journal (create-new, whole); its key. An earlier try that left it short is
/// written over (the lease is this member's alone, and so is the file); another entry under the
/// same lease is an error.
pub fn write(nas: &dyn Nas, e: &Entry) -> Result<String> {
    let key = e.key();
    let p = path(&key);
    let b = serde_json::to_vec(e)?;
    if nas.create_new(&p, &b)? {
        return Ok(key);
    }
    match nas.read(&p)? {
        Some(got) if got == b => Ok(key),
        Some(got) if short(&got) => {
            nas.write_whole(&p, &b)?;
            Ok(key)
        }
        Some(_) => bail!("{p} holds another entry of lease {}", e.lease),
        None => bail!("{p} was there a moment ago and isn't now: try again"),
    }
}

/// A journal entry as read.
#[derive(Clone, Debug)]
pub enum Read {
    Entry(Entry),
    /// There, but not whole: being written (or its member stopped, and writes it again).
    Short,
    /// Not there as read now (not yet, or a stale read), and not set aside.
    Missing,
    /// Set aside as refused, with why.
    SetAside(String),
    /// Can't be an entry: not one, or another entry's; why.
    Damaged(String),
}

/// The entry with key `key`.
pub fn read(nas: &dyn Nas, key: &str) -> Result<Read> {
    let Some(b) = nas.read(&path(key))? else {
        return Ok(match nas.read(&format!("{REJECTED}/{key}.why"))? {
            Some(w) => Read::SetAside(String::from_utf8_lossy(&w).trim().to_string()),
            None => Read::Missing,
        });
    };
    if short(&b) {
        return Ok(Read::Short);
    }
    Ok(match serde_json::from_slice::<Entry>(&b) {
        Ok(e) if e.key() == key => Read::Entry(e),
        Ok(e) => Read::Damaged(format!("it's {}'s, under {key}", e.key())),
        Err(err) => Read::Damaged(format!("it isn't an entry: {err}")),
    })
}

/// Every entry's key in the journal, by day (oldest first) and then name; `since` leaves out the
/// days before it. Lists each day's folder, `rejected/` left out: slow, for take-up and GC.
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

/// Sets refused entry `key` aside: why, and the entry as it was, under `rejected/`, then the entry
/// removed from its day, so no take-up lists it again. The records name it refused meanwhile, so
/// this can fail, or be cut short, and be done again.
pub fn set_aside(nas: &dyn Nas, key: &str, why: &str) -> Result<()> {
    let base = format!("{REJECTED}/{key}");
    // (There already: an earlier try's.)
    nas.create_new(&format!("{base}.why"), why.as_bytes())?;
    if let Some(b) = nas.read(&path(key))? {
        nas.create_new(&format!("{base}.json"), &b)?;
        nas.remove(&path(key))?;
    }
    Ok(())
}

/// A member's own journal entries, each with the term whose lead acknowledged it (applied or
/// refused), kept in the agent's local folder. Each loop the member tells the lead of the term it
/// knows is current of every entry that lead hasn't acknowledged, so after a change of lead it
/// tells the new one of them all: the new lead answers at once for those its records name, and
/// merges the others.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mine {
    entries: BTreeMap<String, Option<u64>>,
}

impl Mine {
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

    /// Forgets the entries of the days before `day` (YYYY-MM-DD): those GC removed from the
    /// journal, every snapshot since naming them.
    pub fn forget_before(&mut self, day: &str) {
        self.entries.retain(|k, _| k.as_str() >= day);
    }
}

#[cfg(test)]
mod tests {
    use super::super::nas::Mem;
    use super::*;

    fn entry(term: u64, n: u64, at: u64) -> Entry {
        let h = Handoff { changes: [("base/6-1-1".to_string(), Some("base/6-1-1.1111111111111111.base".to_string()))].into(), done: Some(("unit".into(), vec![("6/1/1".into(), "k1".into())])), ..Default::default() };
        Entry { member: "m-000000000000000a".into(), lease: LeaseId { term, n }, step: "unit".into(), handoff: h, at }
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
        // Another under the same lease: an error.
        let mut other = e.clone();
        other.step = "pois".into();
        assert!(write(&nas, &other).is_err());
        // One left short by a try cut short: being written, then written over.
        let f = entry(4, 18, 1_791_300_000);
        nas.create_new(&path(&f.key()), b"{\"member\":").unwrap();
        assert!(matches!(read(&nas, &f.key()).unwrap(), Read::Short));
        nas.write_whole(&path(&f.key()), b"").unwrap();
        assert!(matches!(read(&nas, &f.key()).unwrap(), Read::Short));
        write(&nas, &f).unwrap();
        assert!(matches!(read(&nas, &f.key()).unwrap(), Read::Entry(_)));
        // Not there; not an entry; another's.
        assert!(matches!(read(&nas, "2026-10-06/4-19").unwrap(), Read::Missing));
        nas.write_whole(&path("2026-10-06/4-20"), b"[1, 2]").unwrap();
        assert!(matches!(read(&nas, "2026-10-06/4-20").unwrap(), Read::Damaged(_)));
        nas.write_whole(&path("2026-10-06/4-21"), &serde_json::to_vec(&e).unwrap()).unwrap();
        assert!(matches!(read(&nas, "2026-10-06/4-21").unwrap(), Read::Damaged(_)));
    }

    #[test]
    fn the_listing_takes_every_day_and_leaves_refused_entries_aside() {
        let nas = Mem::default();
        for (n, at) in [(1, 1_791_200_000), (2, 1_791_300_000), (3, 1_791_300_100)] {
            write(&nas, &entry(5, n, at)).unwrap();
        }
        nas.write_whole("state/journal/2026-10-06/notes.txt", b"").unwrap();
        assert_eq!(list(&nas, None).unwrap(), ["2026-10-05/5-1", "2026-10-06/5-2", "2026-10-06/5-3"]);
        assert_eq!(list(&nas, Some("2026-10-06")).unwrap(), ["2026-10-06/5-2", "2026-10-06/5-3"]);
        set_aside(&nas, "2026-10-06/5-2", "writes outside its step's names").unwrap();
        assert_eq!(list(&nas, None).unwrap(), ["2026-10-05/5-1", "2026-10-06/5-3"]);
        assert!(matches!(read(&nas, "2026-10-06/5-2").unwrap(), Read::SetAside(w) if w == "writes outside its step's names"));
        assert!(nas.exists("state/journal/rejected/2026-10-06/5-2.json").unwrap());
        // Done again (a first try cut short): nothing more.
        set_aside(&nas, "2026-10-06/5-2", "again").unwrap();
        assert!(matches!(read(&nas, "2026-10-06/5-2").unwrap(), Read::SetAside(w) if w == "writes outside its step's names"));
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
        m.forget_before("2026-10-06");
        assert_eq!(m.to_tell(5), ["2026-10-06/3-2"]);
    }
}
