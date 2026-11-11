//! The jobs' hand-offs by HTTP (docs/coordination.md §8, step 6 of task #878): a member's agent
//! posts each of its jobs' hand-offs (crate::pool::journal::Entry) to the lead's coordinator,
//! `POST /pool/handoff`, which takes it into its store (crate::pool::store: `handoff/<key>`) and on
//! its disk before it answers. The lead's agent merges what's taken from memory
//! (crate::pool::driver::Heard::handed), and once a records snapshot naming them is saved
//! (`merged`) they leave the store, and each member's beat that lists them is answered so
//! (crate::coord::held::Answer::merged): only then does the member drop its copy.
//!
//! - **Posted twice, taken once**: a key taken, or answered merged, is taken again as it is; one
//!   posted after both (a late post, the lead restarted) is merged as any, and the records, which
//!   name it, pass it over.
//! - **What the lead lacks** of what a member's beat lists (neither taken nor merged: posted to a
//!   lead whose store was lost, or never) is said in the answer (`Answer::lacks`), and the member
//!   posts it: so a new epoch after a lost store gets them all again, and a hand-off is posted the
//!   beat after its job ends with no state of the member's for what it has posted.
//! - **The merged** are kept in memory only, until their member's beat no longer lists them: a
//!   restart forgets them, and the member's next beat, listing one still, has it posted again (taken,
//!   then answered merged as the records name it).
//!
//! Nothing here touches the NAS: the coordinator's handlers reach it (§2.3).

use crate::pool::journal::Entry;
use crate::pool::store::{Docs, Store};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

const PREFIX: &str = "handoff/";

#[derive(Debug, Default)]
struct Inner {
    /// Taken and not yet in a saved snapshot, by key.
    taken: BTreeMap<String, Entry>,
    /// In a saved snapshot, by key: their member.
    merged: BTreeMap<String, String>,
}

/// The hand-offs the lead holds: shared by its coordinator (which takes them) and its pool side
/// (which merges them).
#[derive(Debug, Default)]
pub struct Handoffs {
    inner: Mutex<Inner>,
    store: OnceLock<Arc<Store>>,
}

impl Handoffs {
    /// Kept in `store` from now (once): what it holds there read back.
    pub fn attach(&self, store: &Arc<Store>) {
        if self.store.set(store.clone()).is_err() {
            return;
        }
        let docs = store.snapshot().docs;
        let mut g = self.inner.lock().unwrap();
        for (k, v) in docs.range(PREFIX.to_string()..).take_while(|(k, _)| k.starts_with(PREFIX)) {
            match serde_json::from_value::<Entry>(v.clone()) {
                Ok(e) => {
                    g.taken.insert(k[PREFIX.len()..].to_string(), e);
                }
                Err(e) => eprintln!("coordinator: hand-off {k} in its store doesn't read: {e}"),
            }
        }
    }

    /// Takes hand-off `e` into the store (the caller waits for it to be durable before it answers:
    /// an error, not taken); its key. One taken or merged already is taken as it is.
    pub fn put(&self, e: Entry) -> Result<String> {
        anyhow::ensure!(crate::pool::is_member_id(&e.member), "{:?} isn't a member id", e.member);
        let key = e.key().with_context(|| format!("lease {}'s hand-off: its time {} isn't a day", e.lease, e.at))?;
        let store = self.store.get().ok_or_else(|| anyhow::anyhow!("the lead's store isn't open yet"))?;
        anyhow::ensure!(store.failing().is_none(), "the lead's store can't be written now");
        let mut g = self.inner.lock().unwrap();
        if g.taken.contains_key(&key) || g.merged.contains_key(&key) {
            return Ok(key);
        }
        store.submit([(format!("{PREFIX}{key}"), serde_json::to_value(&e)?)].into(), Vec::new());
        g.taken.insert(key.clone(), e);
        Ok(key)
    }

    /// The hand-offs taken and not yet in a saved snapshot.
    pub fn taken(&self) -> Vec<Entry> {
        self.inner.lock().unwrap().taken.values().cloned().collect()
    }

    /// Hand-offs `keys` are in a saved snapshot: out of the store, answered merged from now.
    pub fn merged(&self, keys: &[String]) {
        let mut g = self.inner.lock().unwrap();
        let mut del = Vec::new();
        for k in keys {
            if let Some(e) = g.taken.remove(k) {
                del.push(format!("{PREFIX}{k}"));
                g.merged.insert(k.clone(), e.member);
            }
        }
        if let Some(s) = self.store.get().filter(|_| !del.is_empty()) {
            s.submit(Docs::new(), del);
        }
    }

    /// The answer to member `member`'s beat listing the hand-offs it keeps (`listed`): those merged,
    /// and those it lacks (to post). Its merged that it no longer lists are forgotten.
    pub fn answer(&self, member: &str, listed: &[String]) -> (Vec<String>, Vec<String>) {
        let mut g = self.inner.lock().unwrap();
        let listed_set: BTreeSet<&String> = listed.iter().collect();
        g.merged.retain(|k, m| m != member || listed_set.contains(k));
        let merged = listed.iter().filter(|k| g.merged.contains_key(*k)).cloned().collect();
        let lacks =
            listed.iter().filter(|k| !g.merged.contains_key(*k) && !g.taken.contains_key(*k)).cloned().collect();
        (merged, lacks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::journal::LeaseId;

    fn entry(member: &str, n: u64) -> Entry {
        Entry {
            member: member.into(),
            lease: LeaseId { epoch: 7, n },
            step: "unit".into(),
            handoff: Default::default(),
            at: 1_791_300_000,
        }
    }

    const A: &str = "m-000000000000000a";

    #[test]
    fn a_hand_off_is_in_the_store_until_merged_and_answered_so() {
        let d = tempfile::tempdir().unwrap();
        let h = Handoffs::default();
        assert!(h.put(entry(A, 1)).is_err(), "no store: not taken");
        let store = Store::open(&d.path().join("store")).unwrap();
        h.attach(&store);
        let k = h.put(entry(A, 1)).unwrap();
        assert_eq!(h.put(entry(A, 1)).unwrap(), k, "posted twice: taken once");
        assert!(h.put(entry("someone", 2)).is_err());
        assert!(store.wait(store.last(), crate::pool::store::DURABLE_WAIT));
        assert_eq!(h.answer(A, &[k.clone(), "2026-10-06/7-9".into()]), (vec![], vec!["2026-10-06/7-9".to_string()]));
        store.stop();
        // (A new process: read back from its store.)
        let store = Store::open(&d.path().join("store")).unwrap();
        let h = Handoffs::default();
        h.attach(&store);
        assert_eq!(h.taken().len(), 1);
        h.merged(std::slice::from_ref(&k));
        assert!(h.taken().is_empty());
        assert_eq!(h.answer(A, std::slice::from_ref(&k)), (vec![k.clone()], vec![]));
        assert_eq!(h.put(entry(A, 1)).unwrap(), k);
        assert!(h.taken().is_empty(), "merged: not taken again");
        // No longer listed: forgotten; listed again (a lost answer, a restart): it lacks it.
        assert_eq!(h.answer(A, &[]), (vec![], vec![]));
        assert_eq!(h.answer(A, std::slice::from_ref(&k)), (vec![], vec![k.clone()]));
        assert!(store.wait(store.last(), crate::pool::store::DURABLE_WAIT));
        store.stop();
        let store = Store::open(&d.path().join("store")).unwrap();
        let h = Handoffs::default();
        h.attach(&store);
        assert!(h.taken().is_empty(), "out of the store once merged");
        store.stop();
    }
}
