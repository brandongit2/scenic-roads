//! The coordinator (docs/workers.md): the build Mac's agent hands work to any worker over HTTP and
//! takes the results back. One planner, many workers: the agent plans from the build's records,
//! which it alone writes; a worker only asks for work that fits it, does it, and hands it back.
//!
//! Two kinds of work, by what a worker can reach (§2):
//! - **Jobs** of the plan (crate::agent::claims::SHARED: units, terrain, slope, tree cover,
//!   landmark candidates and peaks), for workers that mount the NAS (the M1's agent, `--helper`). A
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

/// What a job of `step` for `target` costs is kept under: a unit's by its target alone (as before
/// other steps were offered), another step's as "<step> <target>".
pub fn cost_key(step: &str, target: &str) -> String {
    if step == "unit" { target.to_string() } else { format!("{step} {target}") }
}

/// The memory a job of `step` for `target` is predicted to take (MB): a unit's by `unit_peak` (`size`
/// its piece's bytes); another step's what it took last time, else `size`, the estimate it was
/// offered with.
pub fn job_peak(costs: &BTreeMap<String, Cost>, step: &str, target: &str, size: u64) -> u64 {
    if step == "unit" {
        unit_peak(costs, target, size)
    } else {
        costs.get(&cost_key(step, target)).map(|c| c.peak_mb).unwrap_or(size)
    }
}

/// A step's work offered to the workers that mount the NAS: its targets in plan order (target, key,
/// size: a unit's piece bytes, another step's predicted peak memory in MB) and how many go in a job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Offer {
    pub step: String,
    pub targets: Vec<(String, String, u64)>,
    pub batch: usize,
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
    /// The pass, and the plan's work a worker that mounts the NAS may do, a step at a time in plan
    /// order. A worker's come from the far end; this Mac's own jobs take the near end.
    pub pass: String,
    pub offers: Vec<Offer>,
    /// Work done (journaled, or recorded by this Mac's job), (step, target) → key, under a key the
    /// plan hasn't shown yet: not offered again meanwhile.
    pub done: BTreeMap<(String, String), String>,
    /// A worker's failures: (worker, cost_key(step, target)) → (the last, how many): not offered to
    /// it again for an hour, doubling each time.
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

    /// The targets of offer `o` that `a`'s worker may do now, as many as a job of it takes (units: as
    /// many as the worker asks), from the far end of the plan: none held, done, failed by it lately,
    /// or needing more memory than it spares.
    fn pick(&self, o: &Offer, a: &Ask, now: Instant) -> Vec<(String, String)> {
        let held = self.leases.held(&o.step, now);
        let backoff = |t: &str| match self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))) {
            Some((at, n)) => now.duration_since(*at) < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5)),
            None => false,
        };
        let n = if o.step == "unit" { a.max.max(1) } else { o.batch.max(1) };
        o.targets
            .iter()
            .rev()
            .filter(|(t, k, size)| !held.contains(t) && self.done.get(&(o.step.clone(), t.clone())) != Some(k) && !backoff(t) && job_peak(&self.costs, &o.step, t, *size) <= a.mem_mb)
            .take(n)
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
    /// What this agent's jobs carry to offer tasks (`/task/…`), never published.
    pub job_token: String,
    #[cfg_attr(target_os = "wasi", allow(dead_code))]
    port: u16,
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
        let shared = Shared { leases, pass: String::new(), offers: Vec::new(), done: BTreeMap::new(), failed: BTreeMap::new(), tasks: task::Tasks::new(dir.join("tasks")), workers: BTreeMap::new(), costs, dir: dir.to_path_buf() };
        shared.save_leases();
        let shared = Arc::new(Mutex::new(shared));
        // This agent's jobs' own token (they offer tasks): never published, and gone with them.
        let job_token = http::random()?;
        let urls = http::serve(port, http::Ctx { shared: shared.clone(), token: token.clone(), job_token: job_token.clone(), journal: dir.join("journal"), wasm })?;
        let c = Coordinator { shared, contact: Contact { urls, token }, job_token, port, me: me.to_string() };
        c.write_page();
        Ok(c)
    }

    /// Writes the worker page's address, with its token, for this Mac's status bar (tools/status):
    /// over HTTPS when `tailscale serve` proxies the coordinator (a secure context, where the page
    /// may keep the screen on), else its first address; private to this user, like the token.
    #[cfg(not(target_os = "wasi"))]
    fn write_page(&self) {
        let Some(base) = http::served_https(self.port).or_else(|| self.contact.urls.first().map(|u| format!("{u}/"))) else { return };
        let page = self.shared.lock().unwrap().dir.join("page");
        let text = format!("{base}work/#k={}\n", self.contact.token);
        if std::fs::read_to_string(&page).ok().as_deref() != Some(text.as_str()) && crate::whole::write(&page, text.as_bytes()).is_ok() {
            if let Ok(f) = std::fs::File::open(&page) {
                store::sys::set_mode(&f, 0o600).ok();
            }
        }
    }

    /// Where the hand-offs it took are journaled (a folder per worker), for the agent to merge.
    pub fn journal(&self) -> PathBuf {
        self.shared.lock().unwrap().dir.join("journal")
    }

    /// Publishes how to reach it on the NAS, when what's there differs (written whole), and the
    /// worker page's address here (it moves to HTTPS once `tailscale serve` proxies the coordinator).
    pub fn publish(&self, root: &Path) -> Result<()> {
        #[cfg(not(target_os = "wasi"))]
        self.write_page();
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
            if let (true, Work::Job { step, targets }) = (done, &l.work) {
                for (t, k) in targets {
                    s.done.insert((step.clone(), t.clone()), k.clone());
                }
            }
            s.save_leases();
        }
    }

    /// The plan's work for workers that mount the NAS, every shared step's (a step not offered: none
    /// of it), and their pass. The plan's keys include what's journaled, so work done under the key
    /// the plan has now is no longer kept out by hand.
    pub fn offer(&self, pass: &str, offers: Vec<Offer>) {
        let mut s = self.shared.lock().unwrap();
        let planned: BTreeMap<(&str, &str), &str> = offers.iter().flat_map(|o| o.targets.iter().map(move |(t, k, _)| ((o.step.as_str(), t.as_str()), k.as_str()))).collect();
        let done = std::mem::take(&mut s.done);
        s.done = done.into_iter().filter(|((st, t), k)| planned.get(&(st.as_str(), t.as_str())) == Some(&k.as_str())).collect();
        s.pass = pass.to_string();
        s.offers = offers.into_iter().filter(|o| !o.targets.is_empty()).collect();
    }

    /// `offer` with units alone (target, key, piece bytes).
    pub fn offer_units(&self, pass: &str, units: Vec<(String, String, u64)>) {
        self.offer(pass, vec![Offer { step: "unit".into(), targets: units, batch: 0 }]);
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

/// Whether `l` is a file a job of `step` saves for `target`: a unit's base pack, road values, roads'
/// English and the grids its packs lacked; candidates' and peaks' own files; an area's (a z3
/// tile's) lo pack and its z6 tiles' hi packs of terrain, slope or the tree layers.
fn saves(step: &str, target: &str, l: &str) -> bool {
    let dash = target.replace('/', "-");
    match step {
        "unit" => crate::unit::saved_files(&dash).iter().any(|f| f == l),
        "pois" | "peaks" => l == format!("work/{step}/{dash}"),
        "terrain" | "slope" | "trees" => {
            let Some(q) = crate::legacy::Unit::parse(target).filter(|u| u.z == 3) else { return false };
            let layers: &[&str] = match step {
                "terrain" => &["terrain"],
                "slope" => &["slope"],
                _ => &crate::treepacks::LAYERS,
            };
            layers.iter().any(|layer| {
                l == format!("layers/{layer}/lo/{dash}")
                    || l.strip_prefix(&format!("layers/{layer}/hi/6-")).and_then(|r| r.split_once('-')).and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?))).is_some_and(|(x, y)| (x >> 3, y >> 3) == (q.x, q.y))
            })
        }
        _ => false,
    }
}

/// Whether raw tiles' archive area `area` ("3-x-y", "6-x-y", crate::rawpack) is one a job of `step`
/// for `target` fetches tiles of: a terrain area's own z3 and z6 areas; a unit's peaks', the z6
/// areas within two of its own and their z3 ones.
fn fetches(step: &str, target: &str, area: &str) -> bool {
    let (Some(t), Some(a)) = (crate::legacy::Unit::parse(target), crate::legacy::Unit::parse(&area.replacen('-', "/", 2))) else { return false };
    let near = |z: u8, r: i64| a.z == z && (a.x as i64 - (t.x >> (t.z - z)) as i64).abs() <= r && (a.y as i64 - (t.y >> (t.z - z)) as i64).abs() <= r;
    match (step, t.z) {
        ("terrain", 3) => a.z == 3 && (a.x, a.y) == (t.x, t.y) || a.z == 6 && (a.x >> 3, a.y >> 3) == (t.x, t.y),
        ("peaks", 6) => near(6, 2) || near(3, 1),
        _ => false,
    }
}

/// A job's hand-off, when it's its lease's: its done record is the lease's, every change is to one
/// of the files its step saves for one of the lease's targets (`saves`), each a content name of
/// that file, every upload it says it checked is one of its own, and each raw tiles' archive it put
/// on the NAS is of an area its targets fetch, named by its content.
#[cfg(not(target_os = "wasi"))]
fn check_handoff(h: &Handoff, step: &str, targets: &[(String, String)]) -> Result<()> {
    match &h.done {
        Some((s, ts)) => anyhow::ensure!(s == step && ts == targets, "its done record isn't its lease's"),
        None => anyhow::bail!("no done record"),
    }
    anyhow::ensure!(crate::agent::claims::SHARED.contains(&step), "a hand-off of {step} isn't work a worker does");
    for (area, p) in &h.raw {
        anyhow::ensure!(targets.iter().any(|(t, _)| fetches(step, t, area)), "raw tiles of {area} aren't its targets'");
        anyhow::ensure!(crate::rawpack::named_for(&p.name, area), "{} isn't an archive of {area}", p.name);
    }
    for (l, v) in &h.changes {
        anyhow::ensure!(targets.iter().any(|(t, _)| saves(step, t, l)), "{l} isn't one of its targets' files");
        if let Some(c) = v {
            anyhow::ensure!(store::naming::parse_content_name(c).is_some_and(|n| n.logical == l), "{c} isn't a content name of {l}");
        }
    }
    let saved: BTreeSet<&String> = h.changes.values().flatten().collect();
    for c in h.pending.keys() {
        anyhow::ensure!(saved.contains(c), "{c} isn't one of its saves");
    }
    for c in &h.checked {
        anyhow::ensure!(h.pending.contains_key(c), "{c} isn't one of its uploads");
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
            // The work only it can do first: a worker that mounts the NAS does a job of the plan (the
            // most work for what it fetches), the earliest step it can (what later steps wait on),
            // then a task; any other, a task.
            if !s.pass.is_empty() {
                let offers = s.offers.clone();
                for o in offers.iter().filter(|o| a.can.contains(&o.step)) {
                    let pick = s.pick(o, &a, now);
                    if pick.is_empty() {
                        continue;
                    }
                    let lease = s.leases.grant(&a.worker, Work::Job { step: o.step.clone(), targets: pick.clone() }, now);
                    s.save_leases();
                    eprintln!("coordinator: {} took {} {}", a.worker, o.step, pick.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" "));
                    let g = Grant { lease, ttl_s: TTL.as_secs(), work: Granted::Job { step: o.step.clone(), targets: pick, pass: s.pass.clone() } };
                    return Ok((200, serde_json::to_value(g)?));
                }
            }
            if let Some(id) = s.tasks.pick(&a.worker, &a.can, a.mem_mb) {
                let lease = s.leases.grant(&a.worker, Work::Task { id }, now);
                // (Its id taken for good: kept, so no lease after a restart has it.)
                s.save_leases();
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
                    // Refused (422): the worker gives the lease back as failed, and drops the work.
                    if let Err(e) = check_handoff(&h, step, targets) {
                        return Ok((422, serde_json::json!({ "error": format!("{e:#}") })));
                    }
                    // Its lease ended and its units kept out of offers now, then the journal written
                    // without the lock (a whole file, flushed); put back if that fails.
                    s.leases.finish(d.lease, &d.worker, now);
                    for (t, k) in targets {
                        s.done.insert((step.clone(), t.clone()), k.clone());
                        s.failed.remove(&(d.worker.clone(), cost_key(step, t)));
                    }
                    drop(s);
                    if let Err(e) = crate::handoff::write(&journal.join(folder(&d.worker)), &h) {
                        let mut s = shared.lock().unwrap();
                        for (t, _) in targets {
                            s.done.remove(&(step.clone(), t.clone()));
                        }
                        s.save_leases();
                        return Err(e.context("journal the hand-off"));
                    }
                    s = shared.lock().unwrap();
                    s.costs.extend(d.costs.into_iter().filter(|(u, _)| targets.iter().any(|t| *u == cost_key(step, &t.0))));
                    s.save_leases();
                    s.save_costs();
                }
                Work::Task { .. } => {
                    let unit = match s.tasks.done(d.lease, &d.worker, d.outputs, d.removed, d.secs, d.peak_mb) {
                        Ok(u) => u,
                        Err(e) => return Ok((422, serde_json::json!({ "error": format!("{e:#}") }))),
                    };
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
                Work::Job { step, targets } => {
                    for (t, _) in targets {
                        let e = s.failed.entry((f.worker.clone(), cost_key(step, t))).or_insert((now, 0));
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
            let offered: BTreeMap<&str, usize> = s.offers.iter().map(|o| (o.step.as_str(), o.targets.len())).collect();
            Ok((200, serde_json::json!({ "pass": s.pass, "offered": offered, "done": s.done.len(), "leases": leases, "workers": workers, "tasks": tasks })))
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

/// The HTTP side (not in WebAssembly): axum, on a runtime of its own. Every request is bounded in
/// time and size, a slow or stalled one holds a task (not a thread), and files stream both ways.
#[cfg(not(target_os = "wasi"))]
mod http {
    use super::*;
    use anyhow::Context;
    use axum::body::Body;
    use axum::extract::{ConnectInfo, Path as Url, Request, State};
    use axum::http::{header, HeaderMap, Method, StatusCode};
    use axum::middleware::Next;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{any, get, put};
    use axum::{Json, Router};
    use std::net::SocketAddr;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};

    /// The web worker page (docs/workers.md §7), built in: (path under /work/, type, contents). An
    /// app to install (its manifest, service worker and icons: docs/workers.md, The page as an app).
    const PAGE: &[(&str, &str, &[u8])] = &[
        ("", "text/html; charset=utf-8", include_bytes!("../../../../web/work/index.html")),
        ("index.html", "text/html; charset=utf-8", include_bytes!("../../../../web/work/index.html")),
        ("worker.js", "text/javascript", include_bytes!("../../../../web/work/worker.js")),
        ("runtime.js", "text/javascript", include_bytes!("../../../../web/work/runtime.js")),
        ("sw.js", "text/javascript", include_bytes!("../../../../web/work/sw.js")),
        ("manifest.webmanifest", "application/manifest+json", include_bytes!("../../../../web/work/manifest.webmanifest")),
        ("icons/icon-192.png", "image/png", include_bytes!("../../../../web/work/icons/icon-192.png")),
        ("icons/icon-512.png", "image/png", include_bytes!("../../../../web/work/icons/icon-512.png")),
        ("icons/icon-maskable-512.png", "image/png", include_bytes!("../../../../web/work/icons/icon-maskable-512.png")),
        ("icons/apple-touch-icon.png", "image/png", include_bytes!("../../../../web/work/icons/apple-touch-icon.png")),
        ("vendor/browser_wasi_shim/index.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/index.js")),
        ("vendor/browser_wasi_shim/wasi.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/wasi.js")),
        ("vendor/browser_wasi_shim/wasi_defs.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/wasi_defs.js")),
        ("vendor/browser_wasi_shim/fd.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/fd.js")),
        ("vendor/browser_wasi_shim/fs_mem.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/fs_mem.js")),
        ("vendor/browser_wasi_shim/fs_opfs.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/fs_opfs.js")),
        ("vendor/browser_wasi_shim/debug.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/debug.js")),
        ("vendor/browser_wasi_shim/strace.js", "text/javascript", include_bytes!("../../../../web/work/vendor/browser_wasi_shim/strace.js")),
    ];

    /// The page's version: its files' hash, in its service worker (sw.js's VERSION), so a new
    /// app's page is a new service worker, which takes over.
    pub(super) fn page_version() -> &'static str {
        static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        V.get_or_init(|| store::naming::hash16(&PAGE.iter().flat_map(|(n, _, b)| n.as_bytes().iter().chain(b.iter()).copied()).collect::<Vec<u8>>()))
    }

    /// The page's files' addresses for its service worker (sw.js's PAGE, kept as it installs): all
    /// but the worker itself.
    pub(super) fn page_list() -> String {
        let v: Vec<String> = PAGE.iter().filter(|(n, _, _)| *n != "sw.js").map(|(n, _, _)| format!("/work/{n}")).collect();
        serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())
    }

    /// The largest JSON body taken, and the largest upload.
    const JSON_MAX: usize = 16 << 20;
    const UPLOAD_MAX: u64 = 8 << 30;

    #[derive(Clone)]
    pub struct Ctx {
        pub shared: Arc<Mutex<Shared>>,
        /// What workers carry, and what this agent's jobs carry.
        pub token: String,
        pub job_token: String,
        pub journal: PathBuf,
        pub wasm: Option<PathBuf>,
    }

    pub use crate::net::{allowed, loopback, random, served_https, urls};

    /// The workers' token: made once and kept, so workers keep theirs across restarts.
    pub fn token(dir: &Path) -> Result<String> {
        crate::net::kept_token(&dir.join("token"))
    }

    /// Listens on `port`, on a runtime of its own; this Mac's addresses for workers.
    pub fn serve(port: u16, ctx: Ctx) -> Result<Vec<String>> {
        let listener = std::net::TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("listen on port {port}"))?;
        listener.set_nonblocking(true)?;
        let app = Router::new()
            .route("/work", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/work/")]) }))
            .route("/work/", get(|| page(Url(String::new()))))
            .route("/work/{*file}", get(page))
            .route("/work/ask", any(json))
            .route("/work/beat", any(json))
            .route("/work/done", any(json))
            .route("/work/fail", any(json))
            .route("/work/status", any(json))
            .route("/work/in/{lease}/{*path}", get(input))
            .route("/work/out/{lease}/{*path}", put(output))
            .route("/work/prog/{name}", get(prog))
            .route("/task/{*rest}", any(json))
            .layer(axum::middleware::from_fn_with_state(ctx.clone(), gate))
            .layer(tower_http::catch_panic::CatchPanicLayer::new())
            .with_state(ctx);
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).thread_name("coordinator").enable_all().build()?;
        std::thread::Builder::new().name("coordinator".into()).spawn(move || {
            rt.block_on(async move {
                match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => accept(l, app).await,
                    Err(e) => eprintln!("coordinator: its socket: {e}"),
                }
            });
        })?;
        Ok(urls(port))
    }

    /// The most connections at once, how long a request's headers may take to arrive (and an idle
    /// connection to stay open), and a connection's whole life.
    const CONNECTIONS: usize = 512;
    const HEADERS_MAX: Duration = Duration::from_secs(20);
    const CONNECTION_MAX: Duration = Duration::from_secs(3 * 3600);

    /// Takes connections: one from elsewhere, or past the cap, is closed before a byte is read;
    /// each is kept alive by TCP (a device gone without a word is noticed), its headers bounded in
    /// time (which closes idle ones too), and its whole life bounded.
    async fn accept(listener: tokio::net::TcpListener, app: Router) {
        use tower::ServiceExt;
        let room = Arc::new(tokio::sync::Semaphore::new(CONNECTIONS));
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("coordinator: taking a connection: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            if !allowed(peer.ip()) {
                continue;
            }
            let Ok(permit) = room.clone().try_acquire_owned() else { continue };
            let keepalive = socket2::TcpKeepalive::new().with_time(Duration::from_secs(60)).with_interval(Duration::from_secs(15));
            socket2::SockRef::from(&stream).set_tcp_keepalive(&keepalive).ok();
            let app = app.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                    req.extensions_mut().insert(ConnectInfo(peer));
                    app.clone().oneshot(req.map(Body::new))
                });
                let conn = hyper::server::conn::http1::Builder::new().timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(HEADERS_MAX).serve_connection(hyper_util::rt::TokioIo::new(stream), svc);
                tokio::time::timeout(CONNECTION_MAX, conn).await.ok();
                drop(permit);
            });
        }
    }

    fn error(code: StatusCode, why: impl std::fmt::Display) -> Response {
        (code, [(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "error": why.to_string() }))).into_response()
    }

    /// Who may ask what: this Mac, its LAN and the tailnet only; the page without a token, workers'
    /// requests with theirs, a job's (`/task/…`) with its own and from this Mac only.
    async fn gate(State(c): State<Ctx>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
        let ip = peer.ip();
        if !allowed(ip) {
            return error(StatusCode::FORBIDDEN, "not from here");
        }
        let path = req.uri().path();
        let is_page = req.method() == Method::GET && (path == "/work" || path.strip_prefix("/work/").is_some_and(|p| PAGE.iter().any(|(n, _, _)| *n == p)));
        if is_page {
            return next.run(req).await;
        }
        let job = path.starts_with("/task/");
        let want = format!("Bearer {}", if job { &c.job_token } else { &c.token });
        if req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some(want.as_str()) {
            return error(StatusCode::UNAUTHORIZED, "no or wrong token");
        }
        if job && !loopback(ip) {
            return error(StatusCode::FORBIDDEN, "a job's request comes from this Mac");
        }
        next.run(req).await
    }

    async fn page(Url(file): Url<String>) -> Response {
        match PAGE.iter().find(|(n, _, _)| *n == file) {
            Some(("sw.js", t, body)) => ([(header::CONTENT_TYPE, *t), (header::CACHE_CONTROL, "no-store")], String::from_utf8_lossy(body).replace("__VERSION__", page_version()).replace("__PAGE__", &page_list())).into_response(),
            Some((_, t, body)) => ([(header::CONTENT_TYPE, *t), (header::CACHE_CONTROL, "no-store")], *body).into_response(),
            None => error(StatusCode::NOT_FOUND, format!("no {file}")),
        }
    }

    /// How long a JSON request's body may take to arrive.
    const JSON_TIME: Duration = Duration::from_secs(120);

    /// A JSON request, answered by `route` off the runtime (it takes the lock and may write a file).
    async fn json(State(c): State<Ctx>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
        let path = req.uri().path().to_string();
        let body = match tokio::time::timeout(JSON_TIME, axum::body::to_bytes(req.into_body(), JSON_MAX)).await {
            Ok(Ok(b)) => b,
            Ok(Err(_)) => return error(StatusCode::PAYLOAD_TOO_LARGE, "too big, or cut short"),
            Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "its body didn't come"),
        };
        let local = loopback(peer.ip());
        match tokio::task::spawn_blocking(move || route(&path, &body, &c.shared, &c.journal, local)).await {
            Ok(Ok((204, _))) => (StatusCode::NO_CONTENT, [(header::CACHE_CONTROL, "no-store")]).into_response(),
            Ok(Ok((code, v))) => (StatusCode::from_u16(code).unwrap_or(StatusCode::OK), [(header::CACHE_CONTROL, "no-store")], Json(v)).into_response(),
            Ok(Err(e)) => error(StatusCode::BAD_REQUEST, format!("{e:#}")),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    }

    fn worker(h: &HeaderMap) -> String {
        h.get("x-worker").and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
    }

    /// A file streamed: whole, or the one range asked for (`bytes=a-b`).
    async fn send_file(path: &Path, ctype: &'static str, size: Option<u64>, range: Option<&str>) -> Response {
        let mut f = match tokio::fs::File::open(path).await {
            Ok(f) => f,
            Err(e) => return error(StatusCode::NOT_FOUND, e),
        };
        let len = match f.metadata().await {
            Ok(m) => m.len(),
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
        if size.is_some_and(|n| n != len) {
            return error(StatusCode::CONFLICT, "it changed since it was offered");
        }
        let r = range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-')).and_then(|(a, b)| {
            let a: u64 = a.parse().ok()?;
            let last = len.checked_sub(1)?;
            let b: u64 = if b.is_empty() { last } else { b.parse::<u64>().ok()?.min(last) };
            (a <= b).then_some((a, b))
        });
        let (code, start, n) = match r {
            Some((a, b)) => (StatusCode::PARTIAL_CONTENT, a, b - a + 1),
            None => (StatusCode::OK, 0, len),
        };
        if start > 0 && f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "seek");
        }
        let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(tokio::io::AsyncReadExt::take(f, n), 1 << 20));
        let mut resp = (code, [(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "no-store"), (header::ACCEPT_RANGES, "bytes")], body).into_response();
        resp.headers_mut().insert(header::CONTENT_LENGTH, n.into());
        if code == StatusCode::PARTIAL_CONTENT {
            if let Ok(v) = format!("bytes {start}-{}/{len}", start + n - 1).parse() {
                resp.headers_mut().insert(header::CONTENT_RANGE, v);
            }
        }
        resp
    }

    /// A program's WebAssembly build.
    async fn prog(State(c): State<Ctx>, Url(name): Url<String>) -> Response {
        let ok = name.strip_suffix(".wasm").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        match c.wasm.as_ref().filter(|_| ok) {
            Some(d) => send_file(&d.join(&name), "application/wasm", None, None).await,
            None => error(StatusCode::NOT_FOUND, format!("no {name}")),
        }
    }

    /// A task's input, to the worker holding it.
    async fn input(State(c): State<Ctx>, Url((lease, path)): Url<(u64, String)>, h: HeaderMap) -> Response {
        let found = c.shared.lock().unwrap().tasks.input(lease, &worker(&h), &path);
        match found {
            Some((file, size)) => send_file(&file, "application/octet-stream", Some(size), h.get(header::RANGE).and_then(|v| v.to_str().ok())).await,
            None => error(StatusCode::NOT_FOUND, "not an input of that lease"),
        }
    }

    /// How long an upload may go without a byte.
    const CHUNK_TIME: Duration = Duration::from_secs(120);

    /// A file being uploaded: deleted unless it was renamed into place.
    struct Part(PathBuf, bool);
    impl Drop for Part {
        fn drop(&mut self) {
            if !self.1 {
                std::fs::remove_file(&self.0).ok();
            }
        }
    }

    /// A task's output, from the worker holding it: streamed aside (its own temporary name), flushed,
    /// then renamed into place if the task is still the worker's.
    async fn output(State(c): State<Ctx>, Url((lease, path)): Url<(u64, String)>, h: HeaderMap, body: Body) -> Response {
        if h.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).is_some_and(|n| n > UPLOAD_MAX) {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "too big");
        }
        let w = worker(&h);
        let dest = c.shared.lock().unwrap().tasks.upload(lease, &w, &path);
        let Some(dest) = dest else { return error(StatusCode::GONE, "that lease is gone") };
        let mut part = Part(PathBuf::from(format!("{}.part", dest.display())), false);
        let r: Result<u64> = async {
            tokio::fs::create_dir_all(dest.parent().unwrap()).await?;
            let mut f = tokio::fs::File::create(&part.0).await?;
            let mut stream = body.into_data_stream();
            let mut n = 0u64;
            loop {
                let next = tokio::time::timeout(CHUNK_TIME, futures_util::StreamExt::next(&mut stream)).await.map_err(|_| anyhow::anyhow!("no byte for {} s", CHUNK_TIME.as_secs()))?;
                let Some(chunk) = next else { break };
                let chunk = chunk.map_err(|e| anyhow::anyhow!("{e}"))?;
                n += chunk.len() as u64;
                anyhow::ensure!(n <= UPLOAD_MAX, "too big");
                f.write_all(&chunk).await?;
            }
            f.flush().await?;
            f.sync_data().await?;
            Ok(n)
        }
        .await;
        let n = match r {
            Ok(n) => n,
            Err(e) => return error(StatusCode::BAD_REQUEST, format!("{e:#}")),
        };
        // (Taken back while it came: not put in place.)
        if c.shared.lock().unwrap().tasks.upload(lease, &w, &path).is_none() {
            return error(StatusCode::GONE, "that lease is gone");
        }
        if let Err(e) = tokio::fs::rename(&part.0, &dest).await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        part.1 = true;
        (StatusCode::OK, Json(serde_json::json!({ "size": n }))).into_response()
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
    fn a_helper_takes_the_earliest_shared_step_that_fits_it() {
        let (_d, c, w) = start();
        let o = |step: &str, ts: &[(&str, u64)], batch: usize| Offer { step: step.into(), targets: ts.iter().map(|(t, m)| (t.to_string(), format!("k {t}"), *m)).collect(), batch };
        // Terrain needing more than it spares, slope's fitting, units, and candidates of the same tile
        // as a unit (the steps don't mix).
        c.offer("p", vec![o("terrain", &[("3/1/2", 6000)], 1), o("slope", &[("3/2/2", 3000), ("3/2/3", 3000), ("3/1/3", 3000)], 2), o("unit", &[("6/1/1", 1 << 20)], 0), o("pois", &[("6/1/1", 1500)], 12)]);
        let can = |steps: &[&str]| Ask { can: steps.iter().map(|s| s.to_string()).collect(), ..ask(4096) };
        let g = w.ask(&can(&["terrain", "slope", "unit", "pois"])).unwrap().unwrap();
        let Granted::Job { step, targets, pass } = g.work else { panic!() };
        // Slope's, from the far end, as many as a job of it takes.
        assert_eq!((step.as_str(), pass.as_str()), ("slope", "p"));
        assert_eq!(targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["3/1/3", "3/2/3"]);
        // The rest of slope's next, then the unit, then the candidates of the same tile.
        let next = |c: &[&str]| match w.ask(&can(c)).unwrap().map(|g| g.work) {
            Some(Granted::Job { step, targets, .. }) => format!("{step} {}", targets[0].0),
            _ => "none".into(),
        };
        assert_eq!(next(&["terrain", "slope", "unit", "pois"]), "slope 3/2/2");
        assert_eq!(next(&["terrain", "slope", "unit", "pois"]), "unit 6/1/1");
        assert_eq!(next(&["terrain", "slope", "unit", "pois"]), "pois 6/1/1");
        assert_eq!(next(&["terrain", "slope", "unit", "pois"]), "none");
        // Terrain once a run said it fits.
        c.add_costs(&[(cost_key("terrain", "3/1/2"), Cost { peak_mb: 3500, secs: 1 })]);
        assert_eq!(next(&["terrain"]), "terrain 3/1/2");
        // A worker that can't do a step gets none of it.
        c.offer("p", vec![o("trees", &[("3/1/1", 1000)], 1)]);
        assert_eq!(next(&["unit"]), "none");
    }

    #[test]
    fn a_hand_off_may_touch_only_its_steps_files() {
        let ts = |t: &str| vec![(t.to_string(), "k".to_string())];
        let h = |step: &str, t: &str, files: &[&str], raw: &[(&str, &str)]| {
            let changes = files.iter().map(|l| (l.to_string(), Some(format!("{l}.0000000000000003.pack")))).collect();
            let raw = raw.iter().map(|(a, n)| (a.to_string(), crate::rawpack::Pack { name: n.to_string(), bytes: 1 })).collect();
            Handoff { changes, done: Some((step.into(), ts(t))), raw, ..Default::default() }
        };
        // An area's lo pack and its z6 tiles' hi packs; not another area's.
        assert!(check_handoff(&h("slope", "3/2/2", &["layers/slope/lo/3-2-2", "layers/slope/hi/6-16-16", "layers/slope/hi/6-23-23"], &[]), "slope", &ts("3/2/2")).is_ok());
        assert!(check_handoff(&h("slope", "3/2/2", &["layers/slope/hi/6-24-16"], &[]), "slope", &ts("3/2/2")).is_err());
        assert!(check_handoff(&h("slope", "3/2/2", &["layers/terrain/hi/6-16-16"], &[]), "slope", &ts("3/2/2")).is_err());
        assert!(check_handoff(&h("trees", "3/2/2", &["layers/trees-leaf/hi/6-17-17", "layers/trees-cover/lo/3-2-2"], &[]), "trees", &ts("3/2/2")).is_ok());
        assert!(check_handoff(&h("pois", "6/1/3", &["work/pois/6-1-3"], &[]), "pois", &ts("6/1/3")).is_ok());
        assert!(check_handoff(&h("pois", "6/1/3", &["work/peaks/6-1-3"], &[]), "pois", &ts("6/1/3")).is_err());
        // Raw tiles' archives: its own areas, named by their content.
        let terrain = |raw: &[(&str, &str)]| check_handoff(&h("terrain", "3/2/2", &["layers/terrain/lo/3-2-2"], raw), "terrain", &ts("3/2/2"));
        assert!(terrain(&[("6-20-21", "6-20-21.0123456789abcdef.tiles"), ("3-2-2", "3-2-2.0123456789abcdef.tiles")]).is_ok());
        assert!(terrain(&[("6-30-21", "6-30-21.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-20-21", "6-20-22.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-20-21", "../6-20-21.0123456789abcdef.tiles")]).is_err());
        let peaks = |a: &str| check_handoff(&h("peaks", "6/20/21", &["work/peaks/6-20-21"], &[(a, &format!("{a}.0123456789abcdef.tiles"))]), "peaks", &ts("6/20/21"));
        assert!(peaks("6-21-22").is_ok() && peaks("3-2-2").is_ok() && peaks("6-25-21").is_err());
        // A step no worker does.
        assert!(check_handoff(&h("catalog", "catalog", &[], &[]), "catalog", &ts("catalog")).is_err());
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
        assert!(matches!(w.done(&Done { lease: g.lease, handoff: Some(bad), ..Default::default() }).unwrap(), client::Handed::Refused(_)));
        assert_eq!(w.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")])), costs: vec![("6/1/3".into(), Cost { peak_mb: 2000, secs: 300 })], ..Default::default() }).unwrap(), client::Handed::Taken);
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
        assert_eq!(w.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")])), ..Default::default() }).unwrap(), client::Handed::Gone);
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
    fn a_hand_off_may_touch_only_its_units_files() {
        let ts = vec![("6/1/3".to_string(), "k3".to_string())];
        let ok = handoff(&[("6/1/3", "k3")]);
        assert!(check_handoff(&ok, "unit", &ts).is_ok());
        let with = |l: &str, v: Option<&str>| {
            let mut h = ok.clone();
            h.changes.insert(l.into(), v.map(str::to_string));
            check_handoff(&h, "unit", &ts)
        };
        assert!(with("global/roads/6-1-3", Some("global/roads/6-1-3.0000000000000004.roads")).is_ok());
        assert!(with("layers/grid-canopy/hi/6-1-3", None).is_ok());
        // Another step's file of the same tile, another unit's, a value that isn't its content name.
        assert!(with("sources/osm/2026-09-28/pieces/6-1-3", None).is_err());
        assert!(with("work/pois/6-1-3", None).is_err());
        assert!(with("global/roads/6-1-3", Some("../../../../etc/passwd")).is_err());
        assert!(with("global/roads/6-1-3", Some("base/6-1-3.0000000000000003.base")).is_err());
        // An upload it says it checked must be one of its own.
        let mut h = ok.clone();
        h.checked.push("base/6-9-9.0000000000000009.base".into());
        assert!(check_handoff(&h, "unit", &ts).is_err());
        let mut h = ok.clone();
        h.pending.insert("base/6-1-3.0000000000000003.base".into(), "ab".into());
        h.checked.push("base/6-1-3.0000000000000003.base".into());
        assert!(check_handoff(&h, "unit", &ts).is_ok());
        // Not its lease's record.
        assert!(check_handoff(&ok, "unit", &[("6/1/4".to_string(), "k4".to_string())]).is_err());
    }

    #[test]
    fn the_page_is_over_https_where_tailscale_serves_it() {
        let v = serde_json::json!({ "TCP": { "443": { "HTTPS": true } }, "Web": { "mac.tail1.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:8090" } } } } });
        assert_eq!(crate::net::https_in(&v, 8090).as_deref(), Some("https://mac.tail1.ts.net/"));
        assert_eq!(crate::net::https_in(&v, 8091), None);
        assert_eq!(crate::net::https_in(&serde_json::json!({}), 8090), None);
        // The map's own port, beside it; one under a path doesn't serve the pages' /api/….
        let both = serde_json::json!({ "Web": { "mac.tail1.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:8090" }, "/map": { "Proxy": "http://127.0.0.1:8080" } } }, "mac.tail1.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:18085" } } } } });
        assert_eq!(crate::net::https_in(&both, 18085).as_deref(), Some("https://mac.tail1.ts.net:8443/"));
        assert_eq!(crate::net::https_in(&both, 8080), None);
    }

    #[test]
    fn stalled_idle_and_bogus_connections_dont_stop_it() {
        use std::io::{Read, Write};
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let token = c.contact.token.clone();
        let mut held = Vec::new();
        // Headers never finished, connections never used, bodies announced (with the token) and
        // never sent, and a length past what exists.
        for i in 0..40 {
            let mut s = std::net::TcpStream::connect(&addr).unwrap();
            match i % 4 {
                0 => s.write_all(b"POST /work/ask HTTP/1.1\r\nHost: x\r\nContent-Ty").unwrap(),
                1 => {}
                2 => s.write_all(format!("POST /work/ask HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 5000\r\n\r\n").as_bytes()).unwrap(),
                _ => s.write_all(b"POST /work/ask HTTP/1.1\r\nHost: x\r\nContent-Length: 18446744073709551615\r\n\r\n").unwrap(),
            }
            held.push(s);
        }
        // A worker is still answered, and a job too.
        c.offer_units("p", vec![("6/1/1".into(), "k1".into(), 1 << 20)]);
        assert!(w.ask(&ask(4096)).unwrap().is_some());
        let job = client::Client::at(w.urls(), c.job_token.clone(), "job");
        assert_eq!(job.post_json("/task/workers", &serde_json::json!({ "kind": "tail" })).unwrap().0, 200);
        // A job's request with the workers' token: refused.
        let as_job = client::Client::at(w.urls(), c.contact.token.clone(), "job");
        assert!(as_job.post_json("/task/workers", &serde_json::json!({})).is_err());
        // The page, at the address everything gives, without a token.
        let mut s = std::net::TcpStream::connect(&addr).unwrap();
        s.write_all(b"GET /work/ HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut page = String::new();
        s.read_to_string(&mut page).unwrap();
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("Scenic worker"), "{}", &page[..page.len().min(200)]);
        // The page as an app: its manifest, service worker (its version filled in) and icons, likewise.
        let get = |path: &str| {
            let mut s = std::net::TcpStream::connect(&addr).unwrap();
            s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
            let mut b = Vec::new();
            s.read_to_end(&mut b).unwrap();
            b
        };
        let manifest = String::from_utf8(get("/work/manifest.webmanifest")).unwrap();
        assert!(manifest.starts_with("HTTP/1.1 200") && manifest.contains("application/manifest+json") && manifest.contains("\"start_url\": \"/work/\""));
        let sw = String::from_utf8(get("/work/sw.js")).unwrap();
        assert!(sw.starts_with("HTTP/1.1 200") && !sw.contains("__VERSION__") && sw.contains(&format!("const VERSION = \"{}\"", http::page_version())));
        // (Its list of the page's files, to keep as it installs: the page's own address among them,
        // not the worker's.)
        assert!(!sw.contains("__PAGE__") && sw.contains(r#"const PAGE = ["/work/","/work/index.html","/work/worker.js""#) && !sw.contains(r#""/work/sw.js""#));
        let icon = get("/work/icons/icon-192.png");
        assert!(icon.starts_with(b"HTTP/1.1 200") && icon.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"));
        // A connection whose headers don't come is closed once their time is up.
        let mut slow = std::net::TcpStream::connect(&addr).unwrap();
        slow.write_all(b"GET /work/ HTTP/1.1\r\nHost").unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
        let mut b = [0u8; 64];
        let t = Instant::now();
        let n = slow.read(&mut b).unwrap_or(0);
        assert!(t.elapsed() >= Duration::from_secs(15) && t.elapsed() < Duration::from_secs(35), "closed after {:?} with {n} bytes", t.elapsed());
        drop(held);
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
        assert_eq!(w2.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/1", "k1")])), ..Default::default() }).unwrap(), client::Handed::Taken);
    }

    #[test]
    fn a_task_goes_out_and_comes_back() {
        let (d, c, w) = start();
        let root = d.path().join("job");
        std::fs::create_dir_all(root.join("u")).unwrap();
        std::fs::write(root.join("u/in.bin"), b"input").unwrap();
        // The job's side, from this Mac.
        let job = client::Client::at(w.urls().to_vec(), c.job_token.clone(), "job");
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
        assert_eq!(ipad.done(&Done { lease: g.lease, outputs: vec![task::Output { path: "u/out.bin".into(), size: 7 }], secs: 2.0, peak_mb: 300, ..Default::default() }).unwrap(), client::Handed::Taken);
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
