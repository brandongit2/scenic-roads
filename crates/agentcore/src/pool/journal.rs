//! The jobs' hand-offs as entries (`Entry`, keyed `<day>/<epoch>-<n>`) and what a member keeps of
//! them until its lead's saved records name them (`Mine`). A member posts them to its lead by HTTP
//! (crate::coord::handoffs: docs/coordination.md step 6); the lead merges them from memory
//! (crate::pool::driver). Nothing writes them to the NAS (task #1125: the NAS's journal,
//! `state/journal/`, written for a lead on an app before step 6, is gone; an app before it kept the
//! keys of what it wrote there, read back whole once by `Mine::read_back`).

use super::nas::Nas;
use anyhow::{Context, Result};
use buildstore::handoff::Handoff;
use jobrt::jobid::JobId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The NAS's journal's folder, which apps before task #1125 wrote a member's hand-offs to for a lead
/// on an app before docs/coordination.md step 6: read for `Mine::read_back` alone.
const DIR: &str = "state/journal";

/// A job's hand-off and what it was for.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// The member whose job made it.
    pub member: String,
    /// The job run's id, its first lease's (jobrt::jobid): what the merge orders entries by (lease
    /// order, docs/pool.md §6.5). Under the key `lease`, as apps before job ids read it.
    #[serde(rename = "lease")]
    pub job: JobId,
    /// The lease's step (what the lead checks the hand-off's writes against).
    pub step: String,
    /// What the job changed: manifest changes, uploads, its done record, raw tiles' archives.
    pub handoff: Handoff,
    /// When its job ended (unix seconds, its member's clock): its day.
    pub at: u64,
}

impl Entry {
    /// Its key, `<day>/<epoch>-<n>`: what records name it by; None when its time isn't a day (a
    /// damaged entry's).
    pub fn key(&self) -> Option<String> {
        Some(format!("{}/{}", day(self.at)?, self.job))
    }
}

/// The UTC day of unix time `at`, `YYYY-MM-DD`; None past the year 9999 (a time no member's clock
/// gives: a damaged entry's).
pub fn day(at: u64) -> Option<String> {
    let t = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(at))?;
    Some(crate::agent::backup::format_day(t)).filter(|d| is_day(d))
}

/// Whether `name` is a day folder's (`YYYY-MM-DD`).
pub fn is_day(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// A member's own hand-offs, kept whole in the agent's local folder until a lead's saved records
/// name them (applied, passed over or refused): posted to its lead by its beat
/// (crate::coord::handoffs), acknowledged in the beat's answers. Each is dropped once acknowledged:
/// a lead's take-up never starts from records older than those it last saved
/// (crate::pool::driver::Saved::records).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mine {
    /// By key. (Under the name apps before task #1125 kept them by: not written to the journal.)
    #[serde(default, rename = "unwritten", skip_serializing_if = "BTreeMap::is_empty")]
    kept: BTreeMap<String, Entry>,
    /// The keys of those an app before task #1125 wrote to the NAS's journal and no lead had
    /// acknowledged, as it kept them: read back whole (`read_back`), then kept as any. Kept until
    /// read, so a rollback finds them as it left them.
    #[serde(default, rename = "entries", skip_serializing_if = "BTreeSet::is_empty")]
    written: BTreeSet<String>,
}

impl Mine {
    /// Keeps job's hand-off `e` whole: its key, fixed now. An error when its time isn't a day.
    pub fn add(&mut self, e: Entry) -> Result<String> {
        let key = e.key().with_context(|| format!("job {}'s entry: its time {} isn't a day", e.job, e.at))?;
        self.kept.entry(key.clone()).or_insert(e);
        Ok(key)
    }

    /// Its hand-offs, by key.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.kept.keys()
    }

    /// Its hand-offs, themselves.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.kept.values()
    }

    /// Notes entry `key` acknowledged: a lead's saved records name it. Dropped.
    pub fn acked(&mut self, key: &str) {
        self.kept.remove(key);
        self.written.remove(key);
    }

    /// Notes entries `keys` acknowledged (`acked`).
    pub fn acked_all(&mut self, keys: &[String]) {
        keys.iter().for_each(|k| self.acked(k));
    }

    /// How many it keeps that no lead has acknowledged.
    pub fn unacked(&self) -> usize {
        self.kept.len() + self.written.len()
    }

    /// The hand-offs an app before task #1125 wrote to the NAS's journal (`written`), read back whole
    /// and kept as any, to post: what can't be (not there, not whole, or another's) is dropped, with
    /// why (its job's work done again); a read that fails is tried again at the next call. (Deleted
    /// once every Mac has run an app with it: none keeps such keys then.)
    pub fn read_back(&mut self, nas: &dyn Nas) -> (Vec<(String, String)>, Vec<(String, anyhow::Error)>) {
        let (mut dropped, mut failed) = (Vec::new(), Vec::new());
        for key in std::mem::take(&mut self.written) {
            match nas.read(&format!("{DIR}/{key}.json")) {
                Ok(Some(b)) => match serde_json::from_slice::<Entry>(&b) {
                    Ok(e) if e.key().as_deref() == Some(key.as_str()) => {
                        self.kept.entry(key).or_insert(e);
                    }
                    Ok(_) => dropped.push((key, "it's another entry".into())),
                    Err(err) => dropped.push((key, format!("it doesn't read whole: {err}"))),
                },
                Ok(None) => dropped.push((key, "it isn't there".into())),
                Err(err) => {
                    failed.push((key.clone(), err));
                    self.written.insert(key);
                }
            }
        }
        (dropped, failed)
    }
}

#[cfg(test)]
mod tests {
    use super::super::nas::Mem;
    use super::*;

    fn entry(epoch: u64, n: u64, at: u64) -> Entry {
        let h = Handoff {
            changes: [("base/6-1-1".to_string(), Some("base/6-1-1.1111111111111111.base".to_string()))].into(),
            done: Some(("unit".into(), vec![("6/1/1".into(), "k1".into())])),
            ..Default::default()
        };
        Entry { member: "m-000000000000000a".into(), job: JobId { epoch, n }, step: "unit".into(), handoff: h, at }
    }

    #[test]
    fn a_member_keeps_its_hand_offs_until_acknowledged() {
        let mut m = Mine::default();
        let (a, b) = (entry(3, 1, 1_791_200_000), entry(3, 2, 1_791_300_000));
        let (ka, kb) = (m.add(a).unwrap(), m.add(b.clone()).unwrap());
        assert_eq!(m.keys().collect::<Vec<_>>(), [&ka, &kb]);
        m.acked(&ka);
        m.acked("2026-10-06/9-9");
        assert_eq!(
            (m.keys().cloned().collect::<Vec<_>>(), m.unacked()),
            (vec![kb.clone()], 1),
            "dropped once acknowledged"
        );
        // Handed again (its folder not removed before a crash): kept once.
        m.add(b).unwrap();
        assert_eq!(m.unacked(), 1);
        // Kept in the agent's folder under the name apps before #1125 read.
        let v = serde_json::to_value(&m).unwrap();
        assert!(v["unwritten"][&kb].is_object() && v.get("entries").is_none(), "{v}");
    }

    #[test]
    fn a_corrupt_entry_time_is_damage_not_a_panic() {
        // (Review M2: `day` added the time to the epoch unchecked, and every reader panicked.)
        let bad = entry(1, 2, u64::MAX);
        assert_eq!(bad.key(), None);
        assert_eq!(day(253_402_300_800), None, "the year 10000");
        assert!(Mine::default().add(bad).is_err());
    }

    #[test]
    fn an_older_app_s_journal_entries_are_read_back_whole_once() {
        // An app before #1125 wrote three to the journal (an older lead's) and kept their keys,
        // unacknowledged; one of them is there whole, one cut short, one gone.
        let nas = Mem::default();
        let (a, b) = (entry(4, 1, 1_791_300_000), entry(4, 2, 1_791_300_000));
        let (ka, kb) = (a.key().unwrap(), b.key().unwrap());
        nas.write_whole(&format!("{DIR}/{ka}.json"), &serde_json::to_vec(&a).unwrap()).unwrap();
        nas.write_whole(&format!("{DIR}/{kb}.json"), b"{\"member\":").unwrap();
        let mut m: Mine = serde_json::from_value(serde_json::json!({ "entries": [ka, kb, "2026-10-06/4-3"] })).unwrap();
        assert_eq!(m.unacked(), 3);
        let (dropped, failed) = m.read_back(&nas);
        assert!(failed.is_empty());
        assert_eq!(dropped.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), [kb.as_str(), "2026-10-06/4-3"]);
        assert_eq!(m.keys().collect::<Vec<_>>(), [&ka], "kept whole, to post");
        assert_eq!(m.unacked(), 1);
        assert!(serde_json::to_value(&m).unwrap().get("entries").is_none(), "none left to read");
    }
}
