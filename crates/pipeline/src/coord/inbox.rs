//! The members' messages to the lead over HTTP (docs/pool.md §9, task #642): `POST /pool/mail` on
//! the lead's coordinator, taken into an inbox in its agent's pool folder (`inbox.json`), written
//! whole and flushed to its disk before the answer, so a message answered survives the lead's
//! crash. The lead's agent takes them with its mail (crate::agent::pool's `take_mail`), each by its
//! sender's number, so one that came both ways is taken once.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The most messages kept from one sender: its newest (a sender's mailbox keeps as many, and every
/// post carries all it holds that the lead hasn't taken).
pub const KEPT: usize = 64;

/// A post: from member `from` to member `to` (the coordinator's own, else refused), its messages
/// numbered by the sender (higher is later; never repeated across its processes).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Post {
    pub from: String,
    pub to: String,
    pub msgs: Vec<(u64, serde_json::Value)>,
}

/// The messages taken and not yet read by the lead's agent, by sender.
pub type Held = BTreeMap<String, Vec<(u64, serde_json::Value)>>;

#[derive(Debug)]
pub struct Inbox {
    path: PathBuf,
    held: Mutex<Held>,
}

impl Inbox {
    /// The inbox kept at `path` (missing or unreadable: empty).
    pub fn open(path: &Path) -> Inbox {
        let held = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Inbox { path: path.to_path_buf(), held: Mutex::new(held) }
    }

    /// Takes `msgs` from `from`, on disk before it returns (an error: not taken, and not answered
    /// as taken). The highest number held from it.
    pub fn put(&self, from: &str, msgs: Vec<(u64, serde_json::Value)>) -> Result<u64> {
        anyhow::ensure!(crate::pool::is_member_id(from), "{from:?} isn't a member id");
        let mut g = self.held.lock().unwrap();
        let mut next = g.clone();
        let q = next.entry(from.to_string()).or_default();
        for (n, m) in msgs {
            if !q.iter().any(|(k, _)| *k == n) {
                q.push((n, m));
            }
        }
        q.sort_by_key(|(n, _)| *n);
        if q.len() > KEPT {
            q.drain(..q.len() - KEPT);
        }
        let top = q.last().map_or(0, |(n, _)| *n);
        // (Under the lock: one writer of the file at a time.)
        store::whole::write(&self.path, &serde_json::to_vec(&next)?)?;
        *g = next;
        Ok(top)
    }

    /// What it holds.
    pub fn held(&self) -> Held {
        self.held.lock().unwrap().clone()
    }

    /// Drops what it holds from each sender up to its mark in `marks` (read, on disk), and every
    /// sender's not in `keep` (forgotten); rewritten when that changed.
    pub fn read_up_to(&self, marks: &BTreeMap<String, u64>, keep: impl Fn(&str) -> bool) {
        let mut g = self.held.lock().unwrap();
        let mut next = g.clone();
        next.retain(|from, q| {
            let upto = marks.get(from).copied().unwrap_or(0);
            q.retain(|(n, _)| *n > upto);
            !q.is_empty() && keep(from)
        });
        if next != *g {
            match serde_json::to_vec(&next)
                .map_err(anyhow::Error::from)
                .and_then(|b| store::whole::write(&self.path, &b))
            {
                Ok(()) => *g = next,
                Err(e) => eprintln!("pool: the inbox: {e:#}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "m-000000000000000a";

    #[test]
    fn a_post_is_on_disk_before_it_is_answered_and_taken_once() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("inbox.json");
        let i = Inbox::open(&p);
        assert_eq!(i.put(A, vec![(5, serde_json::json!({"Leads": 3}))]).unwrap(), 5);
        assert_eq!(
            i.put(A, vec![(5, serde_json::json!({"Leads": 3})), (7, serde_json::json!({"Leads": 4}))]).unwrap(),
            7
        );
        // (A new process: read back from its disk, each once.)
        let again = Inbox::open(&p);
        assert_eq!(again.held()[A].iter().map(|(n, _)| *n).collect::<Vec<_>>(), [5, 7]);
        again.read_up_to(&[(A.to_string(), 5)].into(), |_| true);
        assert_eq!(Inbox::open(&p).held()[A].len(), 1);
        again.read_up_to(&BTreeMap::new(), |_| false);
        assert!(Inbox::open(&p).held().is_empty());
        assert!(i.put("someone", vec![]).is_err());
    }

    #[test]
    fn a_post_not_written_is_not_taken() {
        let d = tempfile::tempdir().unwrap();
        // (Its folder a file: no write can land.)
        std::fs::write(d.path().join("pool"), b"").unwrap();
        let i = Inbox::open(&d.path().join("pool/inbox.json"));
        assert!(i.put(A, vec![(1, serde_json::json!({"Leads": 3}))]).is_err());
        assert!(i.held().is_empty());
    }
}
