//! The coordinator (docs/workers.md): the build Mac's agent hands work to any worker over HTTP and
//! takes the results back. One planner, many workers: the agent plans from the build's records,
//! which it alone writes; a worker only asks for work that fits it, does it, and hands it back.
//!
//! Two kinds of work, by what a worker can reach (§2):
//! - **Jobs** of the plan (units), for workers that mount the NAS (the M1's agent, `--helper`). A
//!   worker's job saves through the coordinator: its hand-off (its manifest changes and done record,
//!   one per job) is journaled on this Mac's disk, written whole, then merged into the records by the
//!   agent (crate::handoff::merge_from), all of it or none.
//! - **Tasks** (`task`): pure work a running job offers, for any worker, a web page's included.
//!
//! Leases (`lease`) say who does what; this Mac's own jobs hold them too, so a job is never done
//! twice and a worker that goes quiet (or pauses: it doesn't beat) gives its work back. A hand-off
//! for a lease that's gone is refused (410): its work was offered again, and a late save could put
//! an older build in the manifest. The token, the jobs' leases and what each unit cost are kept on
//! this Mac's disk, so the agent restarting (a new app) is a pause to every worker, nothing more.
//!
//! Workers find it in `state/coordinator.json` on the NAS: its addresses (Tailscale's first, then
//! the LAN's) and the token every request but the page's carries. It answers this Mac, its LAN and
//! the tailnet only; a running job's requests (`/task/…`) from this Mac only.

pub mod client;
pub mod lease;
pub mod task;

use crate::handoff::Handoff;
use anyhow::Result;
use lease::{Lease, Leases, Work};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The coordinator's port.
pub const PORT: u16 = 8090;
/// How long a lease lasts without a heartbeat (workers beat every minute).
pub const TTL: Duration = Duration::from_secs(600);
/// How long a worker counts as around after its last request.
const AROUND: Duration = Duration::from_secs(120);

/// How workers reach the coordinator: `state/coordinator.json` on the NAS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    /// Its addresses, in the order to try.
    pub urls: Vec<String>,
    pub token: String,
}

pub fn contact_path(root: &Path) -> PathBuf {
    root.join("state/coordinator.json")
}

/// What a unit cost to build last time: its programs' peak memory and its wall time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub peak_mb: u64,
    pub secs: u64,
}

/// A unit's predicted peak memory (MB): what it took last time, else about ten times its piece (the
/// densest measured: 6.6 GB for 668 MB), and never under 3.7 GB (the canopy step's).
pub fn unit_peak(costs: &BTreeMap<String, Cost>, unit: &str, piece: u64) -> u64 {
    costs.get(unit).map(|c| c.peak_mb).unwrap_or_else(|| (piece >> 20).saturating_mul(10).max(3700))
}

/// A worker, as its requests describe it.
#[derive(Clone, Debug, Serialize)]
pub struct Worker {
    /// "native" (an agent) or "web" (a page).
    pub kind: String,
    pub label: String,
    /// The work it does ("unit", "tail"), the memory it spares (MB) and its cores.
    pub can: Vec<String>,
    pub mem_mb: u64,
    pub cores: u32,
    #[serde(skip)]
    pub seen: Instant,
    pub what: String,
    pub done: u32,
    pub failed: u32,
    /// Its results a job checked against its own run, and whether one differed (it then gets
    /// nothing more: determinism makes any difference a fault).
    pub checked: u32,
    pub bad: bool,
}

/// What the request handlers and the agent share.
#[derive(Debug)]
pub struct Shared {
    pub leases: Leases,
    /// The pass and the plan's units a worker that mounts the NAS may build, in plan order: (target,
    /// key, piece bytes). A worker's come from the far end; this Mac's own jobs take the near end.
    pub pass: String,
    pub units: Vec<(String, String, u64)>,
    /// Units done (journaled, or recorded by this Mac's job) under a key the plan hasn't shown yet:
    /// not offered again meanwhile.
    pub done: BTreeMap<String, String>,
    /// A worker's failures: (worker, target) → (the last, how many): not offered to it again for an
    /// hour, doubling each time.
    pub failed: BTreeMap<(String, String), (Instant, u32)>,
    pub tasks: task::Tasks,
    pub workers: BTreeMap<String, Worker>,
    pub costs: BTreeMap<String, Cost>,
    /// Where the token, leases, costs and journal are kept.
    dir: PathBuf,
}

#[cfg_attr(target_os = "wasi", allow(dead_code))]
impl Shared {
    fn save_leases(&self) {
        if let Err(e) = self.leases.save(&self.dir.join("leases.json")) {
            eprintln!("coordinator: saving the leases: {e:#}");
        }
    }

    fn save_costs(&self) {
        let r = serde_json::to_vec(&self.costs).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&self.dir.join("costs.json"), &b));
        if let Err(e) = r {
            eprintln!("coordinator: saving the costs: {e:#}");
        }
    }

    /// The units `a`'s worker may build now, at most `a.max`, from the far end of the plan: none
    /// held, done, failed by it lately, or needing more memory than it spares.
    fn pick_units(&self, a: &Ask, now: Instant) -> Vec<(String, String)> {
        let held = self.leases.held("unit", now);
        let backoff = |t: &str| match self.failed.get(&(a.worker.clone(), t.to_string())) {
            Some((at, n)) => now.duration_since(*at) < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5)),
            None => false,
        };
        self.units
            .iter()
            .rev()
            .filter(|(t, k, size)| !held.contains(t) && self.done.get(t) != Some(k) && !backoff(t) && unit_peak(&self.costs, t, *size) <= a.mem_mb)
            .take(a.max.max(1))
            .map(|(t, k, _)| (t.clone(), k.clone()))
            .collect()
    }

    /// Marks `worker`'s request (what it said, and itself as `a` describes it).
    fn seen(&mut self, worker: &str, what: String, a: Option<&Ask>, now: Instant) {
        let w = self.workers.entry(worker.to_string()).or_insert_with(|| Worker { kind: String::new(), label: worker.to_string(), can: Vec::new(), mem_mb: 0, cores: 0, seen: now, what: String::new(), done: 0, failed: 0, checked: 0, bad: false });
        w.seen = now;
        w.what = what;
        if let Some(a) = a {
            (w.kind, w.can, w.mem_mb, w.cores) = (a.kind.clone(), a.can.clone(), a.mem_mb, a.cores);
            if let Some(l) = &a.label {
                w.label = l.chars().take(80).collect();
            }
        }
    }
}

pub struct Coordinator {
    pub shared: Arc<Mutex<Shared>>,
    pub contact: Contact,
    /// This Mac's name: its own jobs' leases are held under it.
    me: String,
}

/// A request for work.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ask {
    pub worker: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The kinds of work it does: "unit" (a job; it mounts the NAS), "tail" (a task).
    pub can: Vec<String>,
    /// The memory it spares now (MB).
    pub mem_mb: u64,
    #[serde(default)]
    pub cores: u32,
    /// At most this many targets in a job.
    #[serde(default)]
    pub max: usize,
}

/// Work granted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grant {
    pub lease: u64,
    pub ttl_s: u64,
    #[serde(flatten)]
    pub work: Granted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Granted {
    /// A job of the plan: its step, targets with keys, and the pass they're built from (the
    /// worker's own view of the NAS may lag).
    Job { step: String, targets: Vec<(String, String)>, pass: String },
    /// A task: what to run and fetch (task::Task::spec), and its predicted memory.
    Task { id: u64, task: serde_json::Value, mem_mb: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Beat {
    pub worker: String,
    pub lease: u64,
    #[serde(default)]
    pub progress: Option<String>,
}

/// Work done: a job's one hand-off (its saves and done record) and what its units cost, or a task's
/// uploaded outputs, time and peak memory.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Done {
    pub worker: String,
    pub lease: u64,
    #[serde(default)]
    pub handoff: Option<Handoff>,
    #[serde(default)]
    pub costs: Vec<(String, Cost)>,
    #[serde(default)]
    pub outputs: Vec<task::Output>,
    /// Inputs a task's programs removed.
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub secs: f64,
    #[serde(default)]
    pub peak_mb: u64,
}

/// Work failed: why, and for a task out of memory, the peak it reached (MB).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Fail {
    pub worker: String,
    pub lease: u64,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub oom_mb: Option<u64>,
}

impl Coordinator {
    /// Starts answering on `port` (not in WebAssembly, where nothing listens), keeping its state in
    /// `dir`; `wasm`: the folder of the programs' WebAssembly builds a page fetches.
    #[cfg(target_os = "wasi")]
    pub fn start(_dir: &Path, _wasm: Option<PathBuf>, _port: u16, _me: &str) -> Result<Coordinator> {
        anyhow::bail!("no coordinator in WebAssembly")
    }

    #[cfg(not(target_os = "wasi"))]
    pub fn start(dir: &Path, wasm: Option<PathBuf>, port: u16, me: &str) -> Result<Coordinator> {
        std::fs::create_dir_all(dir.join("journal"))?;
        let token = http::token(dir)?;
        let now = Instant::now();
        let mut leases = Leases::load(&dir.join("leases.json"), TTL, now);
        // This Mac's own: its jobs ended with the agent that ran them.
        for l in leases.drop_where(|l| l.worker == me) {
            eprintln!("coordinator: {}'s lease ended with the agent before this one", l.what());
        }
        let costs = std::fs::read(dir.join("costs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let shared = Shared { leases, pass: String::new(), units: Vec::new(), done: BTreeMap::new(), failed: BTreeMap::new(), tasks: task::Tasks::new(dir.join("tasks")), workers: BTreeMap::new(), costs, dir: dir.to_path_buf() };
        shared.save_leases();
        let shared = Arc::new(Mutex::new(shared));
        let urls = http::serve(port, http::Ctx { shared: shared.clone(), token: token.clone(), journal: dir.join("journal"), wasm })?;
        Ok(Coordinator { shared, contact: Contact { urls, token }, me: me.to_string() })
    }

    /// Where the hand-offs it took are journaled (a folder per worker), for the agent to merge.
    pub fn journal(&self) -> PathBuf {
        self.shared.lock().unwrap().dir.join("journal")
    }

    /// Publishes how to reach it on the NAS, when what's there differs (written whole).
    pub fn publish(&self, root: &Path) -> Result<()> {
        let there = std::fs::read(contact_path(root)).ok().and_then(|b| serde_json::from_slice::<Contact>(&b).ok());
        if there.as_ref() != Some(&self.contact) {
            crate::whole::write(&contact_path(root), &serde_json::to_vec_pretty(&self.contact)?)?;
        }
        Ok(())
    }

    /// Takes it off the NAS (the agent stopping), when it's this one's.
    pub fn unpublish(&self, root: &Path) {
        let there = std::fs::read(contact_path(root)).ok().and_then(|b| serde_json::from_slice::<Contact>(&b).ok());
        if there.is_some_and(|c| c.token == self.contact.token) {
            std::fs::remove_file(contact_path(root)).ok();
        }
    }

    /// A lease for this Mac's own job of `step` over `targets`, unless a worker holds one of them;
    /// its id.
    pub fn hold(&self, step: &str, targets: &[(String, String)]) -> Option<u64> {
        let now = Instant::now();
        let mut s = self.shared.lock().unwrap();
        let held = s.leases.held(step, now);
        if targets.iter().any(|t| held.contains(&t.0)) {
            return None;
        }
        let id = s.leases.grant(&self.me, Work::Job { step: step.to_string(), targets: targets.to_vec() }, now);
        s.save_leases();
        Some(id)
    }

    /// Keeps this Mac's job's lease alive; false when it lapsed (the job paused too long, or the
    /// agent was stuck) and another may be building its targets.
    pub fn renew(&self, id: u64, progress: Option<String>) -> bool {
        self.shared.lock().unwrap().leases.renew(id, &self.me, progress, Instant::now())
    }

    /// Ends this Mac's job's lease; `done`: it recorded its targets (not offered again until the
    /// plan shows it).
    pub fn finish(&self, id: u64, done: bool) {
        let mut s = self.shared.lock().unwrap();
        if let Some(l) = s.leases.finish(id, &self.me, Instant::now()) {
            if done {
                for (t, k) in l.targets() {
                    s.done.insert(t.clone(), k.clone());
                }
            }
            s.save_leases();
        }
    }

    /// The plan's units for workers that mount the NAS (target, key, piece bytes), and their pass.
    /// The plan's keys include what's journaled, so a unit done under the key the plan has now is
    /// no longer kept out by hand.
    pub fn offer_units(&self, pass: &str, units: Vec<(String, String, u64)>) {
        let mut s = self.shared.lock().unwrap();
        let planned: BTreeMap<&String, &String> = units.iter().map(|(t, k, _)| (t, k)).collect();
        let done = std::mem::take(&mut s.done);
        s.done = done.into_iter().filter(|(t, k)| planned.get(t) == Some(&k)).collect();
        s.pass = pass.to_string();
        s.units = units;
    }

    /// The targets of `step` held now, by anyone.
    pub fn held(&self, step: &str) -> BTreeSet<String> {
        self.shared.lock().unwrap().leases.held(step, Instant::now())
    }

    /// Leases past their deadline dropped (their workers went quiet): a task's offered again.
    pub fn expire(&self) -> Vec<Lease> {
        let mut s = self.shared.lock().unwrap();
        let gone = s.leases.expire(Instant::now());
        for l in &gone {
            if let Work::Task { .. } = l.work {
                s.tasks.lapsed(l.id);
            }
        }
        if gone.iter().any(|l| matches!(l.work, Work::Job { .. })) {
            s.save_leases();
        }
        // (Failures past their longest wait forgotten.)
        let now = Instant::now();
        s.failed.retain(|_, (at, _)| now.duration_since(*at) < Duration::from_secs(3600 * 32));
        gone
    }

    /// Ends every task job `owner` offered (it ended), and their leases.
    pub fn close_tasks(&self, owner: u32) {
        let mut s = self.shared.lock().unwrap();
        for l in s.tasks.close_owner(owner) {
            s.leases.cancel(l);
        }
    }

    /// What this Mac's job's units cost.
    pub fn add_costs(&self, costs: &[(String, Cost)]) {
        if costs.is_empty() {
            return;
        }
        let mut s = self.shared.lock().unwrap();
        s.costs.extend(costs.iter().cloned());
        s.save_costs();
    }

    /// The workers around now (asked within two minutes), for the heartbeat: (name, worker).
    pub fn workers(&self) -> Vec<(String, Worker)> {
        let s = self.shared.lock().unwrap();
        s.workers.iter().filter(|(_, w)| w.seen.elapsed() < AROUND).map(|(n, w)| (n.clone(), w.clone())).collect()
    }
}

/// A worker's name as a folder name.
pub fn folder(w: &str) -> String {
    w.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// A job's hand-off, when it's its lease's: its done record is the lease's, and every file it saves
/// is one of the lease's units' (each unit's names end with its tile: `base/6-31-20`,
/// `layers/grid-canopy/hi/6-31-20`).
#[cfg(not(target_os = "wasi"))]
fn check_handoff(h: &Handoff, step: &str, targets: &[(String, String)]) -> Result<()> {
    match &h.done {
        Some((s, ts)) => anyhow::ensure!(s == step && ts == targets, "its done record isn't its lease's"),
        None => anyhow::bail!("no done record"),
    }
    let tails: Vec<String> = targets.iter().map(|t| format!("/{}", t.0.replace('/', "-"))).collect();
    for l in h.changes.keys() {
        anyhow::ensure!(tails.iter().any(|t| l.ends_with(t.as_str())), "{l} isn't one of its units' files");
    }
    for c in h.pending.keys() {
        anyhow::ensure!(h.changes.keys().any(|l| c.starts_with(&format!("{l}."))), "{c} isn't one of its saves");
    }
    Ok(())
}

/// What a request asks of the shared state (JSON in, JSON out): (status, body).
#[cfg(not(target_os = "wasi"))]
fn route(path: &str, body: &[u8], shared: &Mutex<Shared>, journal: &Path, local: bool) -> Result<(u16, serde_json::Value)> {
    let now = Instant::now();
    let ok = serde_json::json!({ "ok": true });
    match path {
        "/work/ask" => {
            let a: Ask = serde_json::from_slice(body)?;
            anyhow::ensure!(!a.worker.is_empty() && a.worker.len() <= 120, "a worker needs a name");
            let mut s = shared.lock().unwrap();
            s.seen(&a.worker, format!("asked for {}", a.can.join(" or ")), Some(&a), now);
            if s.workers[&a.worker].bad {
                return Ok((204, serde_json::Value::Null));
            }
            // The work only it can do first: a worker that mounts the NAS builds a unit (the most
            // work for what it fetches), then a task; any other, a task.
            if a.can.iter().any(|c| c == "unit") && !s.pass.is_empty() {
                let pick = s.pick_units(&a, now);
                if !pick.is_empty() {
                    let lease = s.leases.grant(&a.worker, Work::Job { step: "unit".into(), targets: pick.clone() }, now);
                    s.save_leases();
                    eprintln!("coordinator: {} took unit {}", a.worker, pick.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" "));
                    let g = Grant { lease, ttl_s: TTL.as_secs(), work: Granted::Job { step: "unit".into(), targets: pick, pass: s.pass.clone() } };
                    return Ok((200, serde_json::to_value(g)?));
                }
            }
            if let Some(id) = s.tasks.pick(&a.worker, &a.can, a.mem_mb) {
                let lease = s.leases.grant(&a.worker, Work::Task { id }, now);
                let t = s.tasks.by_id.get_mut(&id).unwrap();
                t.state = task::State::Leased { lease, worker: a.worker.clone() };
                let g = Grant { lease, ttl_s: TTL.as_secs(), work: Granted::Task { id, task: t.spec.clone(), mem_mb: t.mem_mb } };
                eprintln!("coordinator: {} took task {id} ({})", a.worker, t.kind);
                return Ok((200, serde_json::to_value(g)?));
            }
            Ok((204, serde_json::Value::Null))
        }
        "/work/beat" => {
            let b: Beat = serde_json::from_slice(body)?;
            let mut s = shared.lock().unwrap();
            let alive = s.leases.renew(b.lease, &b.worker, b.progress.clone(), now);
            s.seen(&b.worker, b.progress.unwrap_or_else(|| "working".into()), None, now);
            Ok((200, serde_json::json!({ "ok": alive })))
        }
        "/work/done" => {
            let d: Done = serde_json::from_slice(body)?;
            let mut s = shared.lock().unwrap();
            let Some(l) = s.leases.get(d.lease, &d.worker, now).cloned() else {
                // Its work was offered again (or done by another): this one's hand-off is dropped.
                return Ok((410, serde_json::json!({ "error": "that lease is gone" })));
            };
            match &l.work {
                Work::Job { step, targets } => {
                    let h = d.handoff.unwrap_or_default();
                    check_handoff(&h, step, targets)?;
                    crate::handoff::write(&journal.join(folder(&d.worker)), &h)?;
                    s.leases.finish(d.lease, &d.worker, now);
                    for (t, k) in targets {
                        s.done.insert(t.clone(), k.clone());
                        s.failed.remove(&(d.worker.clone(), t.clone()));
                    }
                    s.costs.extend(d.costs.into_iter().filter(|(u, _)| targets.iter().any(|t| &t.0 == u)));
                    s.save_leases();
                    s.save_costs();
                }
                Work::Task { .. } => {
                    let unit = s.tasks.done(d.lease, &d.worker, d.outputs, d.removed, d.secs, d.peak_mb)?;
                    // What its unit's task takes, for the next time it's offered.
                    if let Some(u) = unit {
                        s.costs.insert(format!("tail {u}"), Cost { peak_mb: d.peak_mb, secs: d.secs as u64 });
                        s.save_costs();
                    }
                    s.leases.finish(d.lease, &d.worker, now);
                }
            }
            s.workers.get_mut(&d.worker).map(|w| w.done += 1);
            s.seen(&d.worker, format!("finished {}", l.what()), None, now);
            eprintln!("coordinator: {} finished {}", d.worker, l.what());
            Ok((200, ok))
        }
        "/work/fail" => {
            let f: Fail = serde_json::from_slice(body)?;
            let mut s = shared.lock().unwrap();
            let Some(l) = s.leases.finish(f.lease, &f.worker, now) else { return Ok((410, serde_json::json!({ "error": "that lease is gone" }))) };
            match &l.work {
                Work::Job { targets, .. } => {
                    for (t, _) in targets {
                        let e = s.failed.entry((f.worker.clone(), t.clone())).or_insert((now, 0));
                        *e = (now, e.1 + 1);
                    }
                    s.save_leases();
                }
                Work::Task { .. } => {
                    if let (Some(id), Some(peak)) = (s.tasks.fail(f.lease, &f.worker, &f.error, f.oom_mb), f.oom_mb) {
                        // Out of memory at `peak`: it takes more than that, next time too.
                        if let Some(u) = s.tasks.by_id.get(&id).and_then(|t| t.spec["unit"].as_str().map(str::to_string)) {
                            let c = s.costs.entry(format!("tail {u}")).or_default();
                            c.peak_mb = c.peak_mb.max(peak + peak / 4);
                            s.save_costs();
                        }
                    }
                }
            }
            s.workers.get_mut(&f.worker).map(|w| w.failed += 1);
            let why: String = f.error.chars().take(300).collect();
            s.seen(&f.worker, format!("failed {}: {why}", l.what()), None, now);
            eprintln!("coordinator: {} failed {}: {why}", f.worker, l.what());
            Ok((200, ok))
        }
        "/work/status" => {
            let s = shared.lock().unwrap();
            let leases: Vec<serde_json::Value> = s.leases.all(now).iter().map(|l| serde_json::json!({ "id": l.id, "worker": l.worker, "what": l.what(), "for_s": now.duration_since(l.granted).as_secs(), "progress": l.progress })).collect();
            let workers: BTreeMap<&String, serde_json::Value> = s.workers.iter().map(|(n, w)| (n, serde_json::json!({ "worker": w, "seen_s": now.duration_since(w.seen).as_secs() }))).collect();
            let tasks: Vec<serde_json::Value> = s.tasks.by_id.values().map(|t| serde_json::json!({ "id": t.id, "kind": t.kind, "mem_mb": t.mem_mb, "state": format!("{:?}", t.state).split([' ', '{']).next().unwrap_or("") })).collect();
            Ok((200, serde_json::json!({ "pass": s.pass, "units": s.units.len(), "done": s.done.len(), "leases": leases, "workers": workers, "tasks": tasks })))
        }
        p if p.starts_with("/task/") && !local => anyhow::bail!("{p} is for this Mac's jobs"),
        "/task/offer" => {
            let mut o: task::Offer = serde_json::from_slice(body)?;
            let mut s = shared.lock().unwrap();
            // What its unit's task took last time (a worker's measure, with some room), when that's more.
            if let Some(c) = o.spec["unit"].as_str().and_then(|u| s.costs.get(&format!("tail {u}"))) {
                o.mem_mb = o.mem_mb.max(c.peak_mb + c.peak_mb / 10);
            }
            let id = s.tasks.offer(o, now)?;
            Ok((200, serde_json::json!({ "id": id })))
        }
        "/task/workers" => {
            // Who's around to take tasks of a kind, and the most memory one spares.
            let kind: String = serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|v| v["kind"].as_str().map(str::to_string)).unwrap_or_default();
            let s = shared.lock().unwrap();
            let around: Vec<&Worker> = s.workers.values().filter(|w| !w.bad && now.duration_since(w.seen) < AROUND && w.can.contains(&kind)).collect();
            Ok((200, serde_json::json!({ "workers": around.len(), "mem_mb": around.iter().map(|w| w.mem_mb).max().unwrap_or(0) })))
        }
        p if p.starts_with("/task/") => {
            // /task/<id>, /task/<id>/withdraw, /task/<id>/close
            let mut parts = p["/task/".len()..].split('/');
            let id: u64 = parts.next().unwrap_or("").parse()?;
            let mut s = shared.lock().unwrap();
            match parts.next() {
                None => {
                    let Some(t) = s.tasks.by_id.get(&id) else { return Ok((404, serde_json::json!({ "error": "no such task" }))) };
                    let v = match &t.state {
                        task::State::Offered => serde_json::json!({ "state": "offered" }),
                        task::State::Leased { worker, .. } => serde_json::json!({ "state": "leased", "worker": worker }),
                        task::State::Done { worker, outputs, removed, secs, peak_mb } => {
                            // A worker's first results are all checked, then one in eight.
                            let w = s.workers.get(worker);
                            let check = w.is_none_or(|w| w.checked < 3) || id % 8 == 0;
                            serde_json::json!({ "state": "done", "worker": worker, "out": t.out, "outputs": outputs, "removed": removed, "secs": secs, "peak_mb": peak_mb, "check": check })
                        }
                        task::State::Failed { why } => serde_json::json!({ "state": "failed", "why": why }),
                    };
                    Ok((200, v))
                }
                Some("withdraw") => Ok((200, serde_json::json!({ "withdrawn": s.tasks.withdraw(id) }))),
                Some("close") => {
                    // With the job's check of the result, if it made one.
                    let checked: Option<bool> = serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|v| v["checked"].as_bool());
                    if let (Some(c), Some(task::State::Done { worker, .. })) = (checked, s.tasks.by_id.get(&id).map(|t| t.state.clone())) {
                        if let Some(w) = s.workers.get_mut(&worker) {
                            w.checked += 1;
                            if !c {
                                w.bad = true;
                                eprintln!("coordinator: {worker}'s result for task {id} differs from this Mac's: it gets no more work");
                            }
                        }
                    }
                    if let Some(l) = s.tasks.close(id) {
                        s.leases.cancel(l);
                    }
                    Ok((200, ok))
                }
                Some(x) => anyhow::bail!("no such task request: {x}"),
            }
        }
        _ => anyhow::bail!("no such endpoint: {path}"),
    }
}

/// The HTTP side (not in WebAssembly).
#[cfg(not(target_os = "wasi"))]
mod http {
    use super::*;
    use anyhow::Context;
    use std::io::{Read, Seek, SeekFrom};
    use std::net::IpAddr;

    type Resp = tiny_http::Response<Box<dyn Read + Send>>;

    /// The web worker page (docs/workers.md §7), built in: (path under /work/, type, contents).
    const PAGE: &[(&str, &str, &str)] = &[
        ("", "text/html; charset=utf-8", include_str!("../../../../web/work/index.html")),
        ("index.html", "text/html; charset=utf-8", include_str!("../../../../web/work/index.html")),
        ("worker.js", "text/javascript", include_str!("../../../../web/work/worker.js")),
        ("runtime.js", "text/javascript", include_str!("../../../../web/work/runtime.js")),
        ("vendor/browser_wasi_shim/index.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/index.js")),
        ("vendor/browser_wasi_shim/wasi.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/wasi.js")),
        ("vendor/browser_wasi_shim/wasi_defs.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/wasi_defs.js")),
        ("vendor/browser_wasi_shim/fd.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/fd.js")),
        ("vendor/browser_wasi_shim/fs_mem.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/fs_mem.js")),
        ("vendor/browser_wasi_shim/fs_opfs.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/fs_opfs.js")),
        ("vendor/browser_wasi_shim/debug.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/debug.js")),
        ("vendor/browser_wasi_shim/strace.js", "text/javascript", include_str!("../../../../web/work/vendor/browser_wasi_shim/strace.js")),
    ];
    /// The largest JSON body taken, and the largest upload.
    const JSON_MAX: u64 = 16 << 20;
    const UPLOAD_MAX: u64 = 8 << 30;

    pub struct Ctx {
        pub shared: Arc<Mutex<Shared>>,
        pub token: String,
        pub journal: PathBuf,
        pub wasm: Option<PathBuf>,
    }

    /// The token: made once (128 random bits) and kept, so workers keep theirs across restarts.
    pub fn token(dir: &Path) -> Result<String> {
        let p = dir.join("token");
        if let Ok(t) = std::fs::read_to_string(&p) {
            if t.trim().len() == 32 {
                return Ok(t.trim().to_string());
            }
        }
        let mut b = [0u8; 16];
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).context("read /dev/urandom")?;
        let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
        crate::whole::write(&p, t.as_bytes())?;
        if let Ok(f) = std::fs::File::open(&p) {
            store::sys::set_mode(&f, 0o600).ok();
        }
        Ok(t)
    }

    /// Whether `ip` may ask: this Mac, its LAN, the tailnet (100.64.0.0/10, fd7a:115c:a1e0::/48).
    fn allowed(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])),
            IpAddr::V6(v) => match v.to_ipv4_mapped() {
                Some(v4) => allowed(IpAddr::V4(v4)),
                None => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
            },
        }
    }

    /// This Mac's addresses for workers: its Tailscale address (100.64.0.0/10), then its LAN name.
    fn urls(port: u16) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(o) = std::process::Command::new("/sbin/ifconfig").output() {
            for w in String::from_utf8_lossy(&o.stdout).split_whitespace().collect::<Vec<_>>().windows(2) {
                let ["inet", a] = w else { continue };
                let Ok(ip) = a.parse::<std::net::Ipv4Addr>() else { continue };
                let url = format!("http://{ip}:{port}");
                if ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]) && !out.contains(&url) {
                    out.push(url);
                }
            }
        }
        out.push(format!("http://{}.local:{port}", crate::agent::cond::host_name()));
        out
    }

    /// Listens on `port` with eight threads; this Mac's addresses for workers.
    pub fn serve(port: u16, ctx: Ctx) -> Result<Vec<String>> {
        let server = Arc::new(tiny_http::Server::http(("0.0.0.0", port)).map_err(|e| anyhow::anyhow!("listen on port {port}: {e}"))?);
        let ctx = Arc::new(ctx);
        for _ in 0..8 {
            let (server, ctx) = (server.clone(), ctx.clone());
            std::thread::Builder::new().name("coordinator".into()).spawn(move || {
                while let Ok(mut req) = server.recv() {
                    let resp = handle(&mut req, &ctx).unwrap_or_else(|e| reply(400, "application/json", serde_json::json!({ "error": format!("{e:#}") }).to_string().into_bytes()));
                    req.respond(resp).ok();
                }
            })?;
        }
        Ok(urls(port))
    }

    fn header(k: &str, v: &str) -> tiny_http::Header {
        tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
    }

    fn reply(code: u16, ctype: &str, data: Vec<u8>) -> Resp {
        let n = data.len();
        tiny_http::Response::new(tiny_http::StatusCode(code), vec![header("Content-Type", ctype), header("Cache-Control", "no-store")], Box::new(std::io::Cursor::new(data)), Some(n), None)
    }

    fn get_header<'a>(req: &'a tiny_http::Request, name: &'static str) -> Option<&'a str> {
        req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
    }

    fn handle(req: &mut tiny_http::Request, ctx: &Ctx) -> Result<Resp> {
        let ip = req.remote_addr().map(|a| a.ip());
        if !ip.is_some_and(allowed) {
            return Ok(reply(403, "text/plain", b"not from here".to_vec()));
        }
        let local = ip.is_some_and(|ip| ip.is_loopback() || matches!(ip, IpAddr::V6(v) if v.to_ipv4_mapped().is_some_and(|v| v.is_loopback())));
        let url = req.url().to_string();
        let path = url.split(['?', '#']).next().unwrap_or("").to_string();
        let method = req.method().clone();
        // The page itself needs no token (it reads it from its address's fragment).
        if method == tiny_http::Method::Get {
            if let Some((_, t, body)) = path.strip_prefix("/work/").and_then(|p| PAGE.iter().find(|(n, _, _)| *n == p)) {
                return Ok(reply(200, t, body.as_bytes().to_vec()));
            }
            if path == "/work" {
                let mut r = reply(302, "text/plain", Vec::new());
                r.add_header(header("Location", "/work/"));
                return Ok(r);
            }
        }
        let authed = get_header(req, "Authorization") == Some(format!("Bearer {}", ctx.token).as_str());
        if !authed {
            return Ok(reply(401, "application/json", br#"{"error":"no or wrong token"}"#.to_vec()));
        }
        let worker = get_header(req, "X-Worker").unwrap_or("").to_string();
        match (&method, path.as_str()) {
            (tiny_http::Method::Get, p) if p.starts_with("/work/prog/") => {
                // A program's WebAssembly build.
                let name = &p["/work/prog/".len()..];
                let ok = name.strip_suffix(".wasm").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
                let file = ctx.wasm.as_ref().filter(|_| ok).map(|d| d.join(name)).and_then(|p| std::fs::File::open(p).ok());
                match file {
                    Some(f) => Ok(file_reply(f, "application/wasm", None)?),
                    None => Ok(reply(404, "text/plain", format!("no {name}").into_bytes())),
                }
            }
            (tiny_http::Method::Get, p) if p.starts_with("/work/in/") => {
                // A task's input, to the worker holding it.
                let (lease, rel) = lease_path(&p["/work/in/".len()..])?;
                let found = ctx.shared.lock().unwrap().tasks.input(lease, &worker, rel);
                let Some((file, size)) = found else { return Ok(reply(404, "text/plain", b"not an input of that lease".to_vec())) };
                let f = std::fs::File::open(&file).with_context(|| format!("open {}", file.display()))?;
                anyhow::ensure!(f.metadata()?.len() == size, "{rel} changed since it was offered");
                Ok(file_reply(f, "application/octet-stream", get_header(req, "Range"))?)
            }
            (tiny_http::Method::Put, p) if p.starts_with("/work/out/") => {
                // A task's output, from the worker holding it: written aside, then renamed into place.
                let (lease, rel) = lease_path(&p["/work/out/".len()..])?;
                let dest = ctx.shared.lock().unwrap().tasks.upload(lease, &worker, rel);
                let Some(dest) = dest else { return Ok(reply(410, "application/json", br#"{"error":"that lease is gone"}"#.to_vec())) };
                std::fs::create_dir_all(dest.parent().unwrap())?;
                let part = dest.with_extension("part");
                let mut f = std::fs::File::create(&part)?;
                let n = std::io::copy(&mut req.as_reader().take(UPLOAD_MAX + 1), &mut f)?;
                anyhow::ensure!(n <= UPLOAD_MAX, "too big");
                f.sync_data().ok();
                std::fs::rename(&part, &dest)?;
                Ok(reply(200, "application/json", serde_json::json!({ "size": n }).to_string().into_bytes()))
            }
            (tiny_http::Method::Post, p) | (tiny_http::Method::Get, p) => {
                let mut body = Vec::new();
                req.as_reader().take(JSON_MAX + 1).read_to_end(&mut body)?;
                anyhow::ensure!(body.len() as u64 <= JSON_MAX, "too big");
                let (code, v) = route(p, &body, &ctx.shared, &ctx.journal, local)?;
                let data = if code == 204 { Vec::new() } else { v.to_string().into_bytes() };
                Ok(reply(code, "application/json", data))
            }
            _ => Ok(reply(405, "text/plain", Vec::new())),
        }
    }

    /// "<lease>/<path>" split.
    fn lease_path(s: &str) -> Result<(u64, &str)> {
        let (l, rel) = s.split_once('/').context("<lease>/<path>")?;
        Ok((l.parse()?, rel))
    }

    /// A file, whole or the one range asked for (`bytes=a-b`).
    fn file_reply(mut f: std::fs::File, ctype: &str, range: Option<&str>) -> Result<Resp> {
        let size = f.metadata()?.len();
        let r = range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-')).and_then(|(a, b)| {
            let a: u64 = a.parse().ok()?;
            let b: u64 = if b.is_empty() { size.checked_sub(1)? } else { b.parse::<u64>().ok()?.min(size.checked_sub(1)?) };
            (a <= b).then_some((a, b))
        });
        let mut headers = vec![header("Content-Type", ctype), header("Accept-Ranges", "bytes"), header("Cache-Control", "no-store")];
        match r {
            Some((a, b)) => {
                f.seek(SeekFrom::Start(a))?;
                let n = b - a + 1;
                headers.push(header("Content-Range", &format!("bytes {a}-{b}/{size}")));
                Ok(tiny_http::Response::new(tiny_http::StatusCode(206), headers, Box::new(f.take(n)), Some(n as usize), None))
            }
            None => Ok(tiny_http::Response::new(tiny_http::StatusCode(200), headers, Box::new(f), Some(size as usize), None)),
        }
    }
}

#[cfg(all(test, not(target_os = "wasi")))]
mod tests {
    use super::*;

    fn start() -> (tempfile::TempDir, Coordinator, client::Client) {
        let d = tempfile::tempdir().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = Coordinator::start(&d.path().join("coord"), None, port, "m4").unwrap();
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        (d, c, w)
    }

    fn ask(mem_mb: u64) -> Ask {
        Ask { worker: "m1".into(), kind: "native".into(), can: vec!["unit".into()], mem_mb, cores: 8, max: 6, ..Default::default() }
    }

    fn handoff(units: &[(&str, &str)]) -> Handoff {
        let changes = units.iter().map(|(u, _)| (format!("base/{}", u.replace('/', "-")), Some(format!("base/{}.0000000000000003.base", u.replace('/', "-"))))).collect();
        Handoff { changes, done: Some(("unit".into(), units.iter().map(|(u, k)| (u.to_string(), k.to_string())).collect())), ..Default::default() }
    }

    #[test]
    fn a_helper_asks_beats_and_hands_back_once() {
        let (d, c, w) = start();
        let units = vec![("6/1/1".to_string(), "k1".to_string(), 10u64 << 20), ("6/1/2".into(), "k2".into(), 500 << 20), ("6/1/3".into(), "k3".into(), 20 << 20), ("6/1/4".into(), "k4".into(), 30 << 20)];
        c.offer_units("2026-09-28", units.clone());
        // This Mac's own job holds 6/1/4.
        let own = c.hold("unit", &[("6/1/4".into(), "k4".into())]).unwrap();
        // From the far end, not what this Mac builds, nothing needing more than it spares (6/1/2
        // needs ~5 GB); the pass comes with it.
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let Granted::Job { targets, pass, .. } = &g.work else { panic!() };
        assert_eq!(targets, &vec![("6/1/3".to_string(), "k3".to_string()), ("6/1/1".into(), "k1".into())]);
        assert_eq!(pass, "2026-09-28");
        assert!(w.ask(&ask(4096)).unwrap().is_none(), "held: nothing left");
        assert!(w.beat(g.lease, Some("1/2 areas")).unwrap());
        // A hand-off naming another unit's files is refused; its own, journaled whole.
        let mut bad = handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")]);
        bad.changes.insert("base/6-9-9".into(), None);
        assert!(w.done(&Done { lease: g.lease, handoff: Some(bad), ..Default::default() }).is_err());
        assert!(w.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")])), costs: vec![("6/1/3".into(), Cost { peak_mb: 2000, secs: 300 })], ..Default::default() }).unwrap());
        let waiting = crate::handoff::waiting_in(&c.journal()).unwrap();
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].1.done.is_some() && waiting[0].1.changes.contains_key("base/6-1-3"));
        assert!(!w.beat(g.lease, None).unwrap(), "the lease ended with its hand-off");
        // Done: not offered again before the plan shows it, nor after under the same key; again
        // under a new key.
        assert!(w.ask(&ask(4096)).unwrap().is_none());
        c.offer_units("2026-09-28", units.clone());
        assert!(w.ask(&ask(4096)).unwrap().is_none());
        // A second hand-off for the ended lease: gone.
        assert!(!w.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")])), ..Default::default() }).unwrap());
        c.finish(own, true);
        let mut changed = units.clone();
        changed[0].1 = "k1b".into();
        c.offer_units("2026-09-28", changed);
        let g2 = w.ask(&ask(4096)).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g2.work else { panic!() };
        assert_eq!(targets, &vec![("6/1/1".to_string(), "k1b".to_string())]);
        // It fails: not offered to it again for a while.
        w.fail(g2.lease, "boom", None).unwrap();
        assert!(w.ask(&ask(4096)).unwrap().is_none());
        // What 6/1/3 cost is learned.
        assert_eq!(c.shared.lock().unwrap().costs["6/1/3"].peak_mb, 2000);
        // The wrong token: refused.
        let bad = client::Client::at(w.urls().to_vec(), "nope".into(), "m1");
        assert!(bad.ask(&ask(4096)).is_err());
        drop(d);
    }

    #[test]
    fn leases_and_the_token_outlive_a_restart() {
        let (d, c, w) = start();
        c.offer_units("p", vec![("6/1/1".into(), "k1".into(), 1 << 20), ("6/1/2".into(), "k2".into(), 1 << 20)]);
        let own = c.hold("unit", &[("6/1/2".into(), "k2".into())]).unwrap();
        let g = w.ask(&Ask { max: 1, ..ask(4096) }).unwrap().unwrap();
        let token = c.contact.token.clone();
        drop(c);
        // (A new port: the old listener's threads live on in this process.)
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c2 = Coordinator::start(&d.path().join("coord"), None, port, "m4").unwrap();
        assert_eq!(c2.contact.token, token);
        let w2 = client::Client::at(vec![format!("http://127.0.0.1:{port}")], token, "m1");
        assert!(w2.beat(g.lease, None).unwrap(), "the helper's lease lives on");
        assert!(!c2.renew(own, None), "this Mac's own ended with its agent");
        assert!(w2.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/1", "k1")])), ..Default::default() }).unwrap());
    }

    #[test]
    fn a_task_goes_out_and_comes_back() {
        let (d, c, w) = start();
        let root = d.path().join("job");
        std::fs::create_dir_all(root.join("u")).unwrap();
        std::fs::write(root.join("u/in.bin"), b"input").unwrap();
        // The job's side, from this Mac.
        let job = client::Client::at(w.urls().to_vec(), c.contact.token.clone(), "job");
        let offer = task::Offer { owner: 42, kind: "tail".into(), spec: serde_json::json!({ "unit": "6/1/1" }), root: root.clone(), inputs: [("u/in.bin".to_string(), 5)].into(), mem_mb: 800 };
        let id = job.post_json("/task/offer", &serde_json::to_value(&offer).unwrap()).unwrap().1["id"].as_u64().unwrap();
        // A worker that spares too little gets nothing; one that spares enough, the task.
        let web = |mem| Ask { worker: "ipad".into(), kind: "web".into(), can: vec!["tail".into()], mem_mb: mem, cores: 4, ..Default::default() };
        let ipad = client::Client::at(w.urls().to_vec(), c.contact.token.clone(), "ipad");
        assert!(ipad.ask(&web(500)).unwrap().is_none());
        let g = ipad.ask(&web(1000)).unwrap().unwrap();
        let Granted::Task { id: tid, .. } = g.work else { panic!() };
        assert_eq!(tid, id);
        assert_eq!(job.post_json(&format!("/task/{id}"), &serde_json::json!({})).unwrap().1["state"], "leased");
        // Its input, then its output.
        assert_eq!(ipad.get_bytes(&format!("/work/in/{}/u/in.bin", g.lease)).unwrap(), b"input");
        assert!(ipad.get_bytes(&format!("/work/in/{}/u/other.bin", g.lease)).is_err());
        ipad.put_bytes(&format!("/work/out/{}/u/out.bin", g.lease), b"output!").unwrap();
        assert!(ipad.done(&Done { lease: g.lease, outputs: vec![task::Output { path: "u/out.bin".into(), size: 7 }], secs: 2.0, peak_mb: 300, ..Default::default() }).unwrap());
        let st = job.post_json(&format!("/task/{id}"), &serde_json::json!({})).unwrap().1;
        assert_eq!(st["state"], "done");
        assert_eq!(st["check"], true, "a worker's first results are checked");
        let out = PathBuf::from(st["out"].as_str().unwrap());
        assert_eq!(std::fs::read(out.join("u/out.bin")).unwrap(), b"output!");
        // The job's check found it different: the worker gets nothing more.
        job.post_json(&format!("/task/{id}/close"), &serde_json::json!({ "checked": false })).unwrap();
        assert!(!out.exists());
        let id2 = job.post_json("/task/offer", &serde_json::to_value(&task::Offer { mem_mb: 100, ..offer }).unwrap()).unwrap().1["id"].as_u64().unwrap();
        assert!(ipad.ask(&web(1000)).unwrap().is_none());
        assert_eq!(job.post_json(&format!("/task/{id2}/withdraw"), &serde_json::json!({})).unwrap().1["withdrawn"], true);
    }
}
