//! The members' messages to the lead, and the lead's to them, over HTTP (docs/pool.md §9;
//! docs/coordination.md step 5):
//!
//! - **To the lead**: `POST /pool/mail` on its coordinator, taken into its store
//!   (crate::pool::store: `inbox/<member>`) and on its disk before the answer, so a message
//!   answered survives the lead's crash. The lead's agent takes them with its mail
//!   (crate::agent::pool's `take_mail`), each by its sender's number, so one sent twice is taken
//!   once. (An agent before the store kept them in `inbox.json`: taken into the store once,
//!   `attach`.)
//! - **From the lead**: its agent queues them here (`queue`, from what it sent each member: its
//!   `mail.json`, on its disk), and each beat's answer carries those after the number the member
//!   said it took (`to`).

use crate::pool::store::{Docs, Store};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The most messages kept from one sender: its newest (every post carries all it holds that the
/// lead hasn't taken).
pub const KEPT: usize = 64;

/// A post: from member `from` to member `to` (the coordinator's own, else refused), its messages
/// numbered by the sender (higher is later; never repeated across its processes).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Post {
    pub from: String,
    pub to: String,
    pub msgs: Vec<(u64, serde_json::Value)>,
}

/// Messages by member: those taken and not yet read by the lead's agent, by sender; those the lead
/// sends, by recipient.
pub type Held = BTreeMap<String, Vec<(u64, serde_json::Value)>>;

#[derive(Debug)]
pub struct Inbox {
    /// An agent before the store's inbox file, taken into the store once (`attach`).
    old: PathBuf,
    held: Mutex<Held>,
    out: Mutex<Held>,
    store: OnceLock<Arc<Store>>,
}

fn key(from: &str) -> String {
    format!("inbox/{from}")
}

impl Inbox {
    /// The inbox, an earlier agent's file at `old` read (missing or unreadable: empty); kept in
    /// the lead's store once it's attached.
    pub fn open(old: &Path) -> Inbox {
        let held = std::fs::read(old).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Inbox { old: old.to_path_buf(), held: Mutex::new(held), out: Mutex::default(), store: OnceLock::new() }
    }

    /// Kept in `store` from now (once): what it holds there read back, the old file's taken in and
    /// removed once on its disk.
    pub fn attach(&self, store: &Arc<Store>) {
        if self.store.set(store.clone()).is_err() {
            return;
        }
        let docs = store.snapshot().docs;
        let mut g = self.held.lock().unwrap();
        let mut set = Docs::new();
        let from_file = !g.is_empty();
        for (k, v) in docs.range("inbox/".to_string()..).take_while(|(k, _)| k.starts_with("inbox/")) {
            let Ok(msgs) = serde_json::from_value::<Vec<(u64, serde_json::Value)>>(v.clone()) else { continue };
            merge(g.entry(k["inbox/".len()..].to_string()).or_default(), msgs);
        }
        if from_file {
            for (from, q) in g.iter() {
                if let Ok(v) = serde_json::to_value(q) {
                    set.insert(key(from), v);
                }
            }
            let seq = store.submit(set, Vec::new());
            if store.wait(seq, crate::pool::store::DURABLE_WAIT) {
                std::fs::remove_file(&self.old).ok();
            }
        }
    }

    /// Takes `msgs` from `from` into the store (the caller waits for it to be durable before it
    /// answers: an error, not taken). The highest number held from it.
    pub fn put(&self, from: &str, msgs: Vec<(u64, serde_json::Value)>) -> Result<u64> {
        anyhow::ensure!(crate::pool::is_member_id(from), "{from:?} isn't a member id");
        let store = self.store.get().ok_or_else(|| anyhow::anyhow!("the lead's store isn't open yet"))?;
        anyhow::ensure!(store.failing().is_none(), "the lead's store can't be written now");
        let mut g = self.held.lock().unwrap();
        let q = g.entry(from.to_string()).or_default();
        merge(q, msgs);
        let top = q.last().map_or(0, |(n, _)| *n);
        store.submit([(key(from), serde_json::to_value(&*q)?)].into(), Vec::new());
        Ok(top)
    }

    /// What it holds.
    pub fn held(&self) -> Held {
        self.held.lock().unwrap().clone()
    }

    /// Drops what it holds from each sender up to its mark in `marks` (read, on disk), and every
    /// sender's not in `keep` (forgotten).
    pub fn read_up_to(&self, marks: &BTreeMap<String, u64>, keep: impl Fn(&str) -> bool) {
        let mut g = self.held.lock().unwrap();
        let (mut set, mut del) = (Docs::new(), Vec::new());
        g.retain(|from, q| {
            let upto = marks.get(from).copied().unwrap_or(0);
            let before = q.len();
            q.retain(|(n, _)| *n > upto);
            let kept = !q.is_empty() && keep(from);
            if !kept {
                del.push(key(from));
            } else if q.len() != before {
                if let Ok(v) = serde_json::to_value(&*q) {
                    set.insert(key(from), v);
                }
            }
            kept
        });
        if let Some(s) = self.store.get().filter(|_| !set.is_empty() || !del.is_empty()) {
            s.submit(set, del);
        }
    }

    /// The lead's messages to member `to`, as its agent keeps them (its last `KEPT`).
    pub fn queue(&self, to: &str, msgs: Vec<(u64, serde_json::Value)>) {
        self.out.lock().unwrap().insert(to.to_string(), msgs);
    }

    /// The lead's messages to member `to` after number `took`.
    pub fn to(&self, to: &str, took: u64) -> Vec<(u64, serde_json::Value)> {
        let g = self.out.lock().unwrap();
        g.get(to).into_iter().flatten().filter(|(n, _)| *n > took).cloned().collect()
    }
}

/// `msgs` into `q`, each number once, in order, its newest `KEPT`.
fn merge(q: &mut Vec<(u64, serde_json::Value)>, msgs: Vec<(u64, serde_json::Value)>) {
    for (n, m) in msgs {
        if !q.iter().any(|(k, _)| *k == n) {
            q.push((n, m));
        }
    }
    q.sort_by_key(|(n, _)| *n);
    if q.len() > KEPT {
        q.drain(..q.len() - KEPT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "m-000000000000000a";

    #[test]
    fn a_post_is_in_the_store_and_taken_once() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::open(&d.path().join("store")).unwrap();
        let i = Inbox::open(&d.path().join("inbox.json"));
        assert!(i.put(A, vec![(5, serde_json::json!({"Leads": 3}))]).is_err(), "no store: not taken");
        i.attach(&store);
        assert_eq!(i.put(A, vec![(5, serde_json::json!({"Leads": 3}))]).unwrap(), 5);
        assert_eq!(
            i.put(A, vec![(5, serde_json::json!({"Leads": 3})), (7, serde_json::json!({"Leads": 4}))]).unwrap(),
            7
        );
        assert!(store.wait(store.last(), crate::pool::store::DURABLE_WAIT));
        store.stop();
        // (A new process: read back from its store, each once.)
        let store = Store::open(&d.path().join("store")).unwrap();
        let again = Inbox::open(&d.path().join("inbox.json"));
        again.attach(&store);
        assert_eq!(again.held()[A].iter().map(|(n, _)| *n).collect::<Vec<_>>(), [5, 7]);
        again.read_up_to(&[(A.to_string(), 5)].into(), |_| true);
        assert_eq!(again.held()[A].len(), 1);
        again.read_up_to(&BTreeMap::new(), |_| false);
        assert!(again.held().is_empty());
        assert!(again.put("someone", vec![]).is_err());
        store.stop();
        let store = Store::open(&d.path().join("store")).unwrap();
        let third = Inbox::open(&d.path().join("inbox.json"));
        third.attach(&store);
        assert!(third.held().is_empty(), "what was read is gone from the store");
        store.stop();
    }

    /// An earlier agent's `inbox.json` is taken into the store, and its file removed.
    #[test]
    fn an_older_agent_s_inbox_file_goes_into_the_store() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("inbox.json");
        std::fs::write(&old, serde_json::to_vec(&serde_json::json!({ A: [[3, {"Leads": 1}]] })).unwrap()).unwrap();
        let store = Store::open(&d.path().join("store")).unwrap();
        let i = Inbox::open(&old);
        i.attach(&store);
        assert!(!old.exists());
        assert!(store.snapshot().docs.contains_key(&key(A)));
        assert_eq!(i.held()[A][0].0, 3);
        store.stop();
    }

    #[test]
    fn the_lead_s_messages_go_after_what_the_member_took() {
        let i = Inbox::open(Path::new("/nonexistent/inbox.json"));
        i.queue(A, vec![(1, serde_json::json!(1)), (2, serde_json::json!(2))]);
        assert_eq!(i.to(A, 1), [(2, serde_json::json!(2))]);
        assert!(i.to(A, 2).is_empty() && i.to("m-b", 0).is_empty());
    }
}
