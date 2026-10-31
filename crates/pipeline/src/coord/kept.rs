//! What the coordinator keeps in the pool's store (crate::pool::store; docs/coordination.md §4,
//! task #878), in place of its files (`leases.json`, `costs.json`, `floors.json`, `disks.json`,
//! the jobs' disks and caches' fills, `pause.json`) and of the per-term state file on the NAS
//! (`coord::keep`, gone with it):
//!
//! - its documents: `leases` (the next id, the holds, the cancels), `lease/<id>` (each job's
//!   lease), `cost/<key>`, `floor/<key>`, `disk/<key>`, `job_disks`, `fills`, `pause`, and
//!   `member/<host>` (what a member speaks and when the lead last heard it: `Heard`);
//! - each save submits what changed since the last (`Mirror`), never the whole state;
//! - opened by `Coordinator::start_on` after the coordinator's start: a store found is the truth (the
//!   files start read are a store's first fill alone: an agent before the store's); none, it's
//!   filled from them.
//!
//! The members it keeps (`Heard`) are for leases by their holder's word (§6, step 2: crate::coord::held).

use super::{Cost, Floor};
use crate::pool::nas::Nas;
use crate::pool::store::{self, Docs, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// A member as the lead last heard it (`member/<host>`): the protocol pieces its app speaks, and
/// when (unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    #[serde(default)]
    pub speaks: Vec<String>,
    pub at: u64,
}

/// A member's `Heard` is saved again once its time moved this far (its beats come every 10 s; the
/// 12-h rule needs no finer).
const HEARD_SAVE_S: u64 = 60;

#[derive(Debug, Default)]
pub struct Kept {
    store: OnceLock<Arc<Store>>,
    m: Mutex<Mirror>,
    backing: std::sync::atomic::AtomicBool,
}

/// What was last submitted, by document.
#[derive(Debug, Default)]
struct Mirror {
    leases: BTreeMap<String, Value>,
    costs: BTreeMap<String, Cost>,
    floors: BTreeMap<String, Floor>,
    disks: BTreeMap<String, u64>,
    members: BTreeMap<String, Heard>,
}

/// The coordinator's state as its store holds it.
#[derive(Debug, Default)]
pub struct Loaded {
    /// As `lease::Leases::restore` takes it.
    pub leases: Value,
    pub costs: BTreeMap<String, Cost>,
    pub floors: BTreeMap<String, Floor>,
    pub disks: BTreeMap<String, u64>,
    pub job_disks: BTreeMap<String, Vec<u64>>,
    pub fills: BTreeMap<String, u64>,
    pub pause: Option<(Option<crate::control::Pause>, u64)>,
    pub members: BTreeMap<String, Heard>,
}

fn of_prefix<T: serde::de::DeserializeOwned>(docs: &Docs, prefix: &str) -> BTreeMap<String, T> {
    docs.range(prefix.to_string()..)
        .take_while(|(k, _)| k.starts_with(prefix))
        .filter_map(|(k, v)| Some((k[prefix.len()..].to_string(), serde_json::from_value(v.clone()).ok()?)))
        .collect()
}

impl Loaded {
    pub fn of(docs: &Docs) -> Loaded {
        let mut leases = docs.get("leases").cloned().unwrap_or_else(|| serde_json::json!({ "next": 1 }));
        let each: Vec<Value> = of_prefix::<Value>(docs, "lease/").into_values().collect();
        leases["leases"] = Value::Array(each);
        Loaded {
            leases,
            costs: of_prefix(docs, "cost/"),
            floors: of_prefix(docs, "floor/"),
            disks: of_prefix(docs, "disk/"),
            job_disks: docs.get("job_disks").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default(),
            fills: docs.get("fills").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default(),
            pause: docs
                .get("pause")
                .and_then(|v| Some((serde_json::from_value(v.get("pause")?.clone()).ok()?, v.get("at")?.as_u64()?))),
            members: of_prefix(docs, "member/"),
        }
    }

    /// As the pool's per-term state was (`PoolState`): what a lead taking up another's backup loads
    /// (crate::agent::pool's `took_up`).
    pub fn pool_state(self) -> super::PoolState {
        let (pause, pause_at) = self.pause.unwrap_or_default();
        super::PoolState {
            leases: self.leases,
            costs: self.costs,
            floors: self.floors,
            disks: self.disks,
            job_disks: BTreeMap::new(),
            job_peaks: self.job_disks,
            fills: self.fills,
            disks_v: super::DISKS_V,
            failed: Vec::new(),
            pause,
            pause_at,
        }
    }
}

/// The changes from `was` to `now` under `prefix`, `was` made `now`.
fn diff<T: Serialize + PartialEq + Clone>(
    prefix: &str,
    now: &BTreeMap<String, T>,
    was: &mut BTreeMap<String, T>,
    set: &mut Docs,
    del: &mut Vec<String>,
) {
    for k in was.keys().filter(|k| !now.contains_key(*k)) {
        del.push(format!("{prefix}{k}"));
    }
    for (k, v) in now {
        if was.get(k) != Some(v) {
            if let Ok(j) = serde_json::to_value(v) {
                set.insert(format!("{prefix}{k}"), j);
            }
        }
    }
    if was.len() != now.len() || !set.is_empty() || !del.is_empty() {
        *was = now.clone();
    }
}

impl Kept {
    /// Takes `store` from now on, its documents as `loaded` read them.
    pub fn open(&self, store: Arc<Store>, loaded: &Loaded) {
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let docs = store.snapshot().docs;
        m.leases = of_prefix(&docs, "lease/");
        (m.costs, m.floors, m.disks) = (loaded.costs.clone(), loaded.floors.clone(), loaded.disks.clone());
        m.members = loaded.members.clone();
        drop(m);
        self.store.set(store).ok();
    }

    pub fn store(&self) -> Option<&Arc<Store>> {
        self.store.get()
    }

    fn submit(&self, set: Docs, del: Vec<String>) {
        if let Some(s) = self.store.get() {
            s.submit(set, del);
        }
    }

    /// The leases (`lease::Leases::kept`) as they are now.
    pub fn leases(&self, meta: Value, leases: Vec<(u64, Value)>) {
        if self.store.get().is_none() {
            return;
        }
        let now: BTreeMap<String, Value> = leases.into_iter().map(|(id, v)| (id.to_string(), v)).collect();
        let (mut set, mut del) = (Docs::new(), Vec::new());
        diff("lease/", &now, &mut self.m.lock().unwrap_or_else(|e| e.into_inner()).leases, &mut set, &mut del);
        set.insert("leases".into(), meta);
        self.submit(set, del);
    }

    /// The measures as they are now.
    pub fn measures(
        &self,
        costs: &BTreeMap<String, Cost>,
        floors: &BTreeMap<String, Floor>,
        disks: &BTreeMap<String, u64>,
        job_disks: &BTreeMap<String, Vec<u64>>,
        fills: &BTreeMap<String, u64>,
    ) {
        if self.store.get().is_none() {
            return;
        }
        let (mut set, mut del) = (Docs::new(), Vec::new());
        {
            let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
            diff("cost/", costs, &mut m.costs, &mut set, &mut del);
            diff("floor/", floors, &mut m.floors, &mut set, &mut del);
            diff("disk/", disks, &mut m.disks, &mut set, &mut del);
        }
        for (k, v) in [("job_disks", serde_json::to_value(job_disks)), ("fills", serde_json::to_value(fills))] {
            if let Ok(v) = v {
                set.insert(k.into(), v);
            }
        }
        self.submit(set, del);
    }

    /// The build's pause, as it changed at `at`.
    pub fn pause(&self, pause: Option<&crate::control::Pause>, at: u64) {
        self.submit([("pause".into(), serde_json::json!({ "pause": pause, "at": at }))].into(), Vec::new());
    }

    /// `member` heard at `at`, speaking `speaks`: saved when what it speaks changed, or its time by
    /// `HEARD_SAVE_S`.
    pub fn heard(&self, member: &str, speaks: Vec<String>, at: u64) {
        let h = Heard { speaks, at };
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let save = m.members.get(member).is_none_or(|o| o.speaks != h.speaks || h.at >= o.at + HEARD_SAVE_S);
        if !save {
            return;
        }
        m.members.insert(member.to_string(), h.clone());
        drop(m);
        if let Ok(v) = serde_json::to_value(&h) {
            self.submit([(format!("member/{member}"), v)].into(), Vec::new());
        }
    }

    /// When `member` was last heard (unix seconds, to `HEARD_SAVE_S`), and whether it speaks `what`.
    pub fn member(&self, member: &str) -> Option<Heard> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).members.get(member).cloned()
    }

    /// The members that speak `what` and were heard after `since` (unix seconds).
    pub fn speaking(&self, what: &str, since: u64) -> BTreeSet<String> {
        let m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        m.members
            .iter()
            .filter(|(_, h)| h.at > since && h.speaks.iter().any(|s| s == what))
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Every member as last heard.
    pub fn members(&self) -> BTreeMap<String, Heard> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).members.clone()
    }

    /// The last change submitted (0: no store).
    pub fn last(&self) -> u64 {
        self.store.get().map_or(0, |s| s.last())
    }

    /// Whether change `seq` is on disk within `store::DURABLE_WAIT` (true without a store).
    pub fn durable(&self, seq: u64) -> bool {
        self.store.get().is_none_or(|s| s.wait(seq, store::DURABLE_WAIT))
    }

    /// Backs the store up to the NAS each `store::BACKUP_EVERY`, as `member` (started once).
    pub fn back_up(&self, nas: Arc<dyn Nas + Send + Sync>, member: &str, every: Duration) {
        let Some(s) = self.store.get() else { return };
        if !self.backing.swap(true, std::sync::atomic::Ordering::Relaxed) {
            store::back_up(s, nas, member, every);
        }
    }

    pub fn stop(&self) {
        if let Some(s) = self.store.get() {
            s.stop();
        }
    }
}
