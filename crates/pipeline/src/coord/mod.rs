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
//! twice and a worker that goes quiet (or pauses for its conditions: it doesn't beat) gives its work
//! back; while the build is paused, every lease is held. A hand-off
//! for a lease that's gone is refused (410): its work was offered again, and a late save could put
//! an older build in the manifest. The token, the jobs' leases and what each unit cost are kept on
//! this Mac's disk, so the agent restarting (a new app) is a pause to every worker, nothing more.
//!
//! Workers find it in `state/coordinator.json` on the NAS: its addresses (Tailscale's first, then
//! the LAN's) and the token the Macs' agents carry. A web page helps with a key of its own, its
//! device's, once the build Mac's owner accepts it there (`devices`): for its own tasks and pausing
//! the build, nothing more. It answers this Mac, its LAN and the tailnet only, a request naming this
//! Mac and from no web page elsewhere (crate::net::ours, from_the_page); the owner's (the devices)
//! and a running job's (`/task/…`) from this Mac itself, through no proxy (crate::net::own).

pub mod client;
pub mod devices;
pub mod history;
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

/// What a job's target cost last time (a unit's, another shared step's, a task's): its peak memory
/// (MB) and its wall time, the worker it was measured on (None: the build Mac, or not said), and
/// the way its step was run then (`cost_version`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub peak_mb: u64,
    pub secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub v: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// The way a step runs now, as far as its memory goes: a cost measured another way says nothing of
/// what a run takes now (terrain 2 and slope 2: each z6 tile's pack written as it's made, where
/// they held their whole area's; tree cover 1: the trees program, a band of a block's rows at a
/// time on each thread, where trees.py's workers each held a block's every zoom-12 value, 12 to
/// 36 GB together).
pub fn cost_version(step: &str) -> u32 {
    match step {
        "terrain" | "slope" => 2,
        "trees" => 1,
        _ => 0,
    }
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
/// its piece's bytes); candidates' what they took last time, else the unit's (they read the same
/// piece, with one of the unit's programs: `size` its bytes too); another step's what it took last
/// time, else `size`, the estimate it was offered with.
/// How long `target` of `step` is predicted to take `worker`: its last run's time (twice another
/// worker's: the helpers run at about half the build Mac's pace); None when it hasn't run the way
/// the step runs now.
pub fn job_secs(costs: &BTreeMap<String, Cost>, step: &str, target: &str, worker: &str) -> Option<u64> {
    let c = costs.get(&cost_key(step, target)).filter(|c| c.v >= cost_version(step))?;
    Some(if c.worker.as_deref() == Some(worker) { c.secs } else { c.secs * 2 })
}

pub fn job_peak(costs: &BTreeMap<String, Cost>, step: &str, target: &str, size: u64) -> u64 {
    // (Only what was measured the way the step runs now.)
    match (step, costs.get(&cost_key(step, target)).filter(|c| c.v >= cost_version(step))) {
        ("unit", _) => unit_peak(costs, target, size),
        (_, Some(c)) => c.peak_mb,
        ("pois", None) => unit_peak(costs, target, size),
        (_, None) => size,
    }
}

/// A step's work offered to the workers that mount the NAS: its targets in plan order (target, key,
/// size: a unit's or candidates' piece bytes, another step's predicted peak memory in MB) and how
/// many go in a job.
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
    /// The work it does (the shared steps it builds: crate::agent::claims::SHARED; "tail": tasks),
    /// the memory it spares (MB) and its cores.
    pub can: Vec<String>,
    pub mem_mb: u64,
    pub cores: u32,
    /// The app it runs (an agent's: crate::agent::app_version), when it says.
    pub app: Option<String>,
    /// A page: whether it was in front when it last asked (an iPhone or iPad stops one that isn't).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// Its last ask, for why it gets no work (`/work/swarm`).
    #[serde(skip)]
    pub ask: Option<Ask>,
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
    /// The app this Mac's agent runs: an agent on another builds with other code than the keys it
    /// would record say, so it gets no work ("" in tests: any).
    pub app: String,
    /// The build's pause (crate::control), while it's paused: no work is given, a worker's beat is
    /// told (its job pauses too), and no lease lapses. Kept in `pause.json`, so a restart keeps it.
    pub paused: Option<crate::control::Pause>,
    /// When the pause last changed, as the ask said (unix seconds): an ask older than that (a Mac's
    /// held while it couldn't reach this one) is passed over.
    pub pause_at: u64,
    /// What happened, the last week's (`history`): the worker page's activity.
    pub history: history::History,
    /// The devices that asked to help through the build page, and those accepted (`devices`).
    pub devices: devices::Devices,
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

    /// The devices kept (`durable`: flushed to the disk, an answer's).
    fn save_devices(&self, durable: bool) {
        if let Err(e) = self.devices.save(&self.dir.join("devices.json"), durable) {
            eprintln!("coordinator: saving the devices: {e:#}");
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
    /// or needing more memory than it spares; `away`, those that fit the more it spares while its
    /// owner's away, and end in the time it gives (none when it gives none: the ask's caller tries
    /// every step's usual pick first).
    fn pick(&self, o: &Offer, a: &Ask, now: Instant, away: bool) -> Vec<(String, String)> {
        let held = self.leases.held(&o.step, now);
        let backoff = |t: &str| match self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))) {
            Some((at, n)) => now.duration_since(*at) < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5)),
            None => false,
        };
        let n = if o.step == "unit" { a.max.max(1) } else { o.batch.max(1) };
        // Terrain from the near end: the build Mac's next units wait on it, so a helper builds the
        // next region's while the build Mac builds this one's units. The rest from the far end,
        // away from the build Mac's own (and near each other, for the caches).
        let open: Vec<&(String, String, u64)> = {
            let order: Box<dyn Iterator<Item = &(String, String, u64)>> = if o.step == "terrain" { Box::new(o.targets.iter()) } else { Box::new(o.targets.iter().rev()) };
            order.filter(|(t, k, _)| !held.contains(t) && self.done.get(&(o.step.clone(), t.clone())) != Some(k) && !backoff(t)).collect()
        };
        let take = |v: Vec<&(String, String, u64)>| v.into_iter().map(|(t, k, _)| (t.clone(), k.clone())).collect::<Vec<_>>();
        if !away {
            return take(open.iter().copied().filter(|(t, _, size)| job_peak(&self.costs, &o.step, t, *size) <= a.mem_mb).take(n).collect());
        }
        let (Some(more), Some(max)) = (a.more_mb, a.max_secs) else { return Vec::new() };
        // Its owner away: a job that fits the more it spares then, as long as it ends in time.
        let mut left = max;
        take(
            open.into_iter()
                .filter(|(t, _, size)| job_peak(&self.costs, &o.step, t, *size) <= more && job_secs(&self.costs, &o.step, t, &a.worker).is_some_and(|s| s <= max))
                .take_while(|(t, _, _)| match job_secs(&self.costs, &o.step, t, &a.worker) {
                    Some(s) if s <= left => {
                        left -= s;
                        true
                    }
                    _ => false,
                })
                .take(n)
                .collect(),
        )
    }

    /// Why a worker gets the shared steps' work it does, or doesn't: for each step it does that's
    /// offered, the targets offered, then those held by another, done, kept from it after it failed
    /// them, too large for the memory it spares, and those left that it may take (`pick`'s rules,
    /// counted).
    fn fit(&self, a: &Ask, now: Instant) -> Vec<serde_json::Value> {
        self.offers
            .iter()
            .filter(|o| a.can.contains(&o.step))
            .map(|o| {
                let held = self.leases.held(&o.step, now);
                let (mut h, mut d, mut b, mut big, mut long, mut fits) = (0, 0, 0, 0, 0, 0);
                for (t, k, size) in &o.targets {
                    let backoff = self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))).is_some_and(|(at, n)| now.duration_since(*at) < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5)));
                    match () {
                        _ if held.contains(t) => h += 1,
                        _ if self.done.get(&(o.step.clone(), t.clone())) == Some(k) => d += 1,
                        _ if backoff => b += 1,
                        _ if job_peak(&self.costs, &o.step, t, *size) <= a.mem_mb => fits += 1,
                        _ if a.more_mb.is_none_or(|m| job_peak(&self.costs, &o.step, t, *size) > m) => big += 1,
                        _ if !job_secs(&self.costs, &o.step, t, &a.worker).is_some_and(|s| Some(s) <= a.max_secs) => long += 1,
                        _ => fits += 1,
                    }
                }
                serde_json::json!({ "step": o.step, "offered": o.targets.len(), "held": h, "done": d, "kept_from": b, "too_big": big, "too_long": long, "fits": fits })
            })
            .collect()
    }

    /// Marks `worker`'s request (what it said, and itself as `a` describes it).
    fn seen(&mut self, worker: &str, what: String, a: Option<&Ask>, now: Instant) {
        if !self.workers.contains_key(worker) {
            let note = a.and_then(|a| a.label.clone()).unwrap_or_default();
            self.history.add(history::Event { worker: Some(worker.to_string()), note, ..history::Event::new("worker") });
        }
        let w = self.workers.entry(worker.to_string()).or_insert_with(|| Worker { kind: String::new(), label: worker.to_string(), can: Vec::new(), mem_mb: 0, cores: 0, app: None, visible: None, ask: None, seen: now, what: String::new(), done: 0, failed: 0, checked: 0, bad: false });
        w.seen = now;
        w.what = what;
        if let Some(a) = a {
            (w.kind, w.can, w.mem_mb, w.cores, w.app, w.visible) = (a.kind.clone(), a.can.clone(), a.mem_mb, a.cores, a.app.clone(), a.visible);
            w.ask = Some(a.clone());
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
    /// The NAS's project folder while the agent has it, for tasks' reads where the data lies
    /// (`/work/net/…/nas/…`).
    root: Arc<Mutex<Option<PathBuf>>>,
}

/// The data servers whose files a task may read through the coordinator (`/work/net/…/web/…`): the
/// elevations' (crate::dem) and land cover's (crate::landcover). Nothing else is fetched for a
/// worker.
pub const WEB_HOSTS: [&str; 5] = ["data.bris.ac.uk", "cyberjapandata.gsi.go.jp", "canelevation-dem.s3.ca-central-1.amazonaws.com", "prd-tnm.s3.amazonaws.com", "esa-worldcover.s3.eu-central-1.amazonaws.com"];

/// What of the NAS a task may read through the coordinator (`/work/net/…/nas/…`): the sources the
/// steps read (canopy squares, FABDEM, the tree cover's rasters, raw terrain), Taiwan's DTM.
pub const NAS_PATHS: [&str; 2] = ["sources/", "inputs/moi-dtm/"];

/// A request for work.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ask {
    pub worker: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The kinds of work it does: a shared step's jobs (crate::agent::claims::SHARED: it mounts
    /// the NAS), "tail" (a task).
    pub can: Vec<String>,
    /// The memory it spares now (MB).
    pub mem_mb: u64,
    #[serde(default)]
    pub cores: u32,
    /// At most this many targets in a job.
    #[serde(default)]
    pub max: usize,
    /// The app it runs (an agent's, crate::agent::app_version; a page's is the coordinator's own).
    #[serde(default)]
    pub app: Option<String>,
    /// A page: whether it's in front (an iPhone or iPad stops one that isn't).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// While its owner is away: the more memory it spares (MB) for a job predicted (from its
    /// targets' last runs) to end within `max_secs`, before they're likely back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_secs: Option<u64>,
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
    /// How far it is (0–1), when it says (a page's slot).
    #[serde(default)]
    pub frac: Option<f64>,
}

/// Work done: a job's one hand-off (its saves and done record) and what its units cost, or a task's
/// uploaded outputs, time and peak memory.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Done {
    pub worker: String,
    pub lease: u64,
    #[serde(default)]
    pub handoff: Option<Handoff>,
    /// Its job failed after the targets its hand-off says it did: the lease's others are held
    /// against the worker (as a failure's).
    #[serde(default)]
    pub failed: bool,
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
    /// Stopped, not failed (the build paused, the Mac slept, its agent restarted, it couldn't start
    /// there): its targets aren't kept from the worker.
    #[serde(default)]
    pub interrupted: bool,
}

impl Coordinator {
    /// Starts answering on `port` (not in WebAssembly, where nothing listens), keeping its state in
    /// `dir`; `wasm`: the folder of the programs' WebAssembly builds a page fetches; `app`: the app
    /// this Mac's agent runs, the one an agent asking for work must run too.
    #[cfg(target_os = "wasi")]
    pub fn start(_dir: &Path, _wasm: Option<PathBuf>, _port: u16, _me: &str, _app: &str) -> Result<Coordinator> {
        anyhow::bail!("no coordinator in WebAssembly")
    }

    #[cfg(not(target_os = "wasi"))]
    pub fn start(dir: &Path, wasm: Option<PathBuf>, port: u16, me: &str, app: &str) -> Result<Coordinator> {
        std::fs::create_dir_all(dir.join("journal"))?;
        let token = http::token(dir)?;
        let now = Instant::now();
        let mut leases = Leases::load(&dir.join("leases.json"), TTL, now);
        // This Mac's own: its jobs ended with the agent that ran them.
        for l in leases.drop_where(|l| l.worker == me) {
            eprintln!("coordinator: {}'s lease ended with the agent before this one", l.what());
        }
        let costs = std::fs::read(dir.join("costs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        // The build's pause, as it was when the agent before this one stopped (`{pause, at}`; an older
        // agent's, the pause alone).
        let kept: serde_json::Value = std::fs::read(dir.join("pause.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let (paused, pause_at): (Option<crate::control::Pause>, u64) = match kept.get("at") {
            Some(at) if kept.get("pause").is_some() => (serde_json::from_value(kept["pause"].clone()).ok().flatten(), at.as_u64().unwrap_or(0)),
            _ => {
                let p: Option<crate::control::Pause> = serde_json::from_value(kept).ok();
                let at = p.as_ref().map_or(0, |p| p.at);
                (p, at)
            }
        };
        // (No later than a minute from now: an ask's time past that is passed over, `AHEAD_S`.)
        let pause_at = pause_at.min(unix_now() + AHEAD_S);
        let mut history = history::History::load(Some(&dir.join("history.jsonl")));
        history.add(history::Event { worker: Some(me.to_string()), note: format!("app {app}"), ..history::Event::new("agent") });
        let devices = devices::Devices::load(&dir.join("devices.json"));
        let shared = Shared { leases, pass: String::new(), offers: Vec::new(), done: BTreeMap::new(), failed: BTreeMap::new(), tasks: task::Tasks::new(dir.join("tasks")), workers: BTreeMap::new(), costs, app: app.to_string(), paused, pause_at, history, devices, dir: dir.to_path_buf() };
        shared.save_leases();
        let shared = Arc::new(Mutex::new(shared));
        // This agent's jobs' own token (they offer tasks): never published, and gone with them.
        let job_token = http::random()?;
        let root = Arc::new(Mutex::new(None));
        let urls = http::serve(port, http::Ctx { shared: shared.clone(), token: token.clone(), job_token: job_token.clone(), journal: dir.join("journal"), wasm, root: root.clone(), remote: Default::default() })?;
        let c = Coordinator { shared, contact: Contact { urls, token }, job_token, port, me: me.to_string(), root };
        c.write_page();
        Ok(c)
    }

    /// Writes the build page's address for this Mac's status bar (tools/status): over HTTPS when
    /// `tailscale serve` proxies the coordinator (a secure context, where the page may keep the
    /// screen on), else its first address. (No key in it: a device that's to help asks this Mac.)
    #[cfg(not(target_os = "wasi"))]
    fn write_page(&self) {
        let Some(base) = http::served_https(self.port).or_else(|| self.contact.urls.first().map(|u| format!("{u}/"))) else { return };
        let page = self.shared.lock().unwrap().dir.join("page");
        let text = format!("{base}work/\n");
        if std::fs::read_to_string(&page).ok().as_deref() != Some(text.as_str()) && crate::whole::write(&page, text.as_bytes()).is_ok() {
            if let Ok(f) = std::fs::File::open(&page) {
                store::sys::set_mode(&f, 0o600).ok();
            }
        }
    }

    /// The NAS's project folder as the agent has it now (None: away), for tasks' reads.
    pub fn set_root(&self, root: Option<&Path>) {
        *self.root.lock().unwrap() = root.map(Path::to_path_buf);
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

    /// Ends this Mac's job's lease; `done`: the targets it recorded (not offered again until the plan
    /// shows them; the rest are free again).
    pub fn finish(&self, id: u64, done: &[(String, String)]) {
        let mut s = self.shared.lock().unwrap();
        if let Some(l) = s.leases.finish(id, &self.me, Instant::now()) {
            if let Work::Job { step, .. } = &l.work {
                for (t, k) in done {
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

    /// The build paused (how, by whom), or going on, as asked at `at` (unix seconds; an ask older
    /// than the last change is passed over): no work given and no lease lapsing meanwhile, and
    /// workers told. Kept on disk.
    pub fn set_pause(&self, pause: Option<crate::control::Pause>, at: u64) {
        set_pause(&mut self.shared.lock().unwrap(), pause, at);
    }

    /// The build's pause, while it's paused.
    pub fn pause(&self) -> Option<crate::control::Pause> {
        self.shared.lock().unwrap().paused.clone()
    }

    /// The targets of `step` held now, by anyone.
    pub fn held(&self, step: &str) -> BTreeSet<String> {
        self.shared.lock().unwrap().leases.held(step, Instant::now())
    }

    /// Leases past their deadline dropped (their workers went quiet): a task's offered again.
    pub fn expire(&self) -> Vec<Lease> {
        let mut s = self.shared.lock().unwrap();
        // (None while the build is paused: every lease held, a paused or asleep worker's work not
        // given to another.)
        if s.paused.is_some() {
            s.leases.hold_all(Instant::now());
            return Vec::new();
        }
        let gone = s.leases.expire(Instant::now());
        for l in &gone {
            if let Work::Task { .. } = l.work {
                s.tasks.lapsed(l.id);
            }
            let (step, targets) = match &l.work {
                Work::Job { step, targets } => (Some(step.clone()), targets.iter().map(|t| t.0.clone()).collect()),
                Work::Task { .. } => (Some("tail".to_string()), Vec::new()),
            };
            s.history.add(history::Event { worker: Some(l.worker.clone()), lease: Some(l.id), step, targets, note: l.what(), ..history::Event::new("lapse") });
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

    /// What this Mac's job's units cost (measured here).
    pub fn add_costs(&self, costs: &[(String, Cost)]) {
        self.add_costs_by(costs, &self.me.clone());
    }

    /// `add_costs`, measured by `worker` (this Mac's second job: crate::agent::second_worker).
    pub fn add_costs_by(&self, costs: &[(String, Cost)], worker: &str) {
        if costs.is_empty() {
            return;
        }
        let mut s = self.shared.lock().unwrap();
        s.costs.extend(costs.iter().cloned().map(|(u, c)| (u, Cost { worker: Some(worker.to_string()), ..c })));
        s.save_costs();
    }

    /// The memory a job of `step` for `target` is expected to take (MB), as `pick` sizes it: as its
    /// last run measured (the way the step runs now), else by what the plan offered it with (a
    /// unit's piece, another step's guess); None when it's neither (a step not offered).
    pub fn peak(&self, step: &str, target: &str) -> Option<u64> {
        let s = self.shared.lock().unwrap();
        let measured = s.costs.get(&cost_key(step, target)).filter(|c| c.v >= cost_version(step)).map(|c| c.peak_mb);
        let offered = s.offers.iter().find(|o| o.step == step).and_then(|o| o.targets.iter().find(|t| t.0 == target)).map(|t| job_peak(&s.costs, step, target, t.2));
        measured.or(offered)
    }

    /// Notes what happened (`history`): this Mac's jobs started and ended, catalogs, its conditions.
    pub fn note(&self, e: history::Event) {
        self.shared.lock().unwrap().history.add(e);
    }

    /// What the forecast reads (crate::agent::forecast): what each target cost, the jobs leased now
    /// (worker, step, targets), the history kept, and the memory each worker spares (MB).
    pub fn for_forecast(&self) -> (BTreeMap<String, Cost>, Vec<(String, String, Vec<String>, u64)>, Vec<history::Event>, BTreeMap<String, u64>) {
        let s = self.shared.lock().unwrap();
        let now = Instant::now();
        // (Each with how long ago it was granted.)
        let leased = s
            .leases
            .all(now)
            .iter()
            .filter_map(|l| match &l.work {
                Work::Job { step, targets } => Some((l.worker.clone(), step.clone(), targets.iter().map(|t| t.0.clone()).collect(), now.duration_since(l.granted).as_secs())),
                Work::Task { .. } => None,
            })
            .collect();
        let mem = s.workers.iter().map(|(n, w)| (n.clone(), w.mem_mb)).collect();
        (s.costs.clone(), leased, s.history.since(0, usize::MAX), mem)
    }

    /// The workers around now (asked within two minutes), for the heartbeat: (name, worker).
    pub fn workers(&self) -> Vec<(String, Worker)> {
        let s = self.shared.lock().unwrap();
        s.workers.iter().filter(|(_, w)| w.seen.elapsed() < AROUND).map(|(n, w)| (n.clone(), w.clone())).collect()
    }
}

/// Whether an agent on app `theirs` may build for a coordinator on `ours`: the same, or a newer
/// published one (a step a newer app changed is built again under the keys that say so, once this
/// Mac runs it; an older app's work would be recorded as current). `ours` empty (tests): any.
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn app_ok(theirs: Option<&str>, ours: &str) -> bool {
    // (By bytes: a version is a worker's to say, and slicing a string mid-character would panic with
    // the coordinator's lock held.)
    let published = |v: &[u8]| v.len() > 14 && v[8] == b'-' && v[13] == b'-' && v[..8].iter().chain(&v[9..13]).all(u8::is_ascii_digit);
    match theirs {
        _ if ours.is_empty() => true,
        Some(t) if t == ours => true,
        Some(t) => published(t.as_bytes()) && published(ours.as_bytes()) && t.as_bytes()[..13] > ours.as_bytes()[..13],
        None => false,
    }
}

/// An app version (`20261005-1508-84142d3`: its UTC publish time, then its commit) in words: "5 Oct
/// 15:08 UTC"; as it is when it isn't one.
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn app_when(v: &str) -> String {
    let p: Vec<&str> = v.split('-').collect();
    let month = |m: &str| ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].get(m.parse::<usize>().ok()?.checked_sub(1)?).copied();
    match p.as_slice() {
        [d, t, _] if d.len() == 8 && t.len() == 4 && d.bytes().chain(t.bytes()).all(|b| b.is_ascii_digit()) => match month(&d[4..6]) {
            Some(m) => format!("{} {m} {}:{} UTC", d[6..8].trim_start_matches('0'), &t[..2], &t[2..]),
            None => v.to_string(),
        },
        _ => v.to_string(),
    }
}

/// A worker's name as a folder name.
pub fn folder(w: &str) -> String {
    w.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Whether `l` is a file a job of `step` saves for `target`: a unit's base pack, road values, roads'
/// English and the grids its packs lacked; candidates' and peaks' own files; an area's (a z3
/// tile's) lo pack and its z6 tiles' hi packs of terrain, slope or the tree layers.
pub fn saves(step: &str, target: &str, l: &str) -> bool {
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

/// A job's hand-off, when it's its lease's: its done record is the lease's, every change is to one
/// of the files its step saves for one of the lease's targets (`saves`), each a content name of
/// that file, every upload it says it checked is one of its own, and each raw tiles' archive it put
/// on the NAS is named as packing names one for its area. (Of any area: a job packs every loose
/// tile in the helper's cache, an earlier job's left there too, a job stopped before it packed:
/// tiles of the NAS's own, whatever job fetched them.)
#[cfg(not(target_os = "wasi"))]
fn check_handoff(h: &Handoff, step: &str, targets: &[(String, String)]) -> Result<()> {
    // (Its lease's targets, or some of them: a job paused at a safe point hands off what it did.)
    match &h.done {
        Some((s, ts)) => anyhow::ensure!(s == step && !ts.is_empty() && ts.iter().all(|t| targets.contains(t)), "its done record isn't its lease's"),
        None => anyhow::bail!("no done record"),
    }
    anyhow::ensure!(crate::agent::claims::SHARED.contains(&step), "a hand-off of {step} isn't work a worker does");
    for (area, p) in &h.raw {
        anyhow::ensure!(crate::rawpack::is_area(area), "{area} isn't an area of raw tiles");
        anyhow::ensure!(crate::rawpack::named_for(&p.name, area), "{} isn't an archive of {area}", p.name);
    }
    // (Of the targets it says it did: one it didn't finish may have saved part of its files.)
    let did: Vec<&(String, String)> = h.done.as_ref().map(|d| d.1.iter().collect()).unwrap_or_default();
    for (l, v) in &h.changes {
        anyhow::ensure!(did.iter().any(|(t, _)| saves(step, t, l)), "{l} isn't one of the files of the targets it did");
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

/// The build paused (how, by whom) or going on (`Coordinator::set_pause`, `/work/pause`), kept in
/// `pause.json` so the next agent starts with it.
fn set_pause(s: &mut Shared, pause: Option<crate::control::Pause>, at: u64) {
    if at < s.pause_at {
        eprintln!("coordinator: an ask to {} from before the last change passed over", if pause.is_some() { "pause" } else { "go on" });
        return;
    }
    s.pause_at = at;
    // (Every lease held a whole TTL again, as the build pauses or goes on: none lapses for the time it
    // was paused.)
    s.leases.hold_all(Instant::now());
    if s.paused != pause {
        eprintln!("coordinator: {}", pause.as_ref().map_or("the build goes on".to_string(), |p| p.why()));
        let e = match &pause {
            Some(p) => history::Event { note: format!("{} ({})", p.by, if p.mode == crate::control::Mode::Freeze { "at once" } else { "at safe points" }), ..history::Event::new("pause") },
            None => history::Event::new("resume"),
        };
        s.history.add(e);
    }
    let kept = serde_json::to_vec(&serde_json::json!({ "pause": pause, "at": at })).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&s.dir.join("pause.json"), &b));
    if let Err(e) = kept {
        eprintln!("coordinator: keeping the pause: {e:#} (it holds until this agent stops)");
    }
    s.paused = pause;
}

/// The raw tiles' archives of a hand-off not taken, each well named for its area, journaled on their
/// own (crate::handoff::RAW_AGAIN) for the next merge to name: they're on the NAS, and unnamed they'd
/// go two days later, their tiles fetched again.
#[cfg(not(target_os = "wasi"))]
fn raw_again(journal: &Path, h: Option<&Handoff>) {
    let raw: Vec<(String, crate::rawpack::Pack)> = h.into_iter().flat_map(|h| h.raw.iter()).filter(|(a, p)| crate::rawpack::is_area(a) && crate::rawpack::named_for(&p.name, a)).cloned().collect();
    if raw.is_empty() {
        return;
    }
    if let Err(e) = crate::handoff::write(&journal.join(crate::handoff::RAW_AGAIN), &Handoff { raw, ..Default::default() }) {
        eprintln!("coordinator: a hand-off's raw tiles' archives not kept for naming ({e:#}): they go as unnamed ones do");
    }
}

/// Seconds since the epoch.
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// How far ahead of this Mac's clock an agent's ask to pause may say it was made (the Macs' clocks
/// differ a little): later, it's taken as made a minute from now.
#[cfg_attr(target_os = "wasi", allow(dead_code))]
const AHEAD_S: u64 = 60;

/// Who's asking a request, as `http::gate` found: this Mac itself (`local`: from loopback, through
/// no proxy), with the build's own key (`owner`: the Macs' agents, this Mac's menu bar and
/// `scenic`), or as an accepted device (its page's id, and what it is); and from where
/// (crate::net::source).
#[cfg(not(target_os = "wasi"))]
#[derive(Clone, Debug, Default)]
struct Caller {
    local: bool,
    owner: bool,
    device: Option<(String, String)>,
    from: String,
}

/// What a request asks of the shared state (JSON in, JSON out): (status, body).
#[cfg(not(target_os = "wasi"))]
fn route(path: &str, body: &[u8], shared: &Mutex<Shared>, journal: &Path, caller: &Caller) -> Result<(u16, serde_json::Value)> {
    let local = caller.local;
    let now = Instant::now();
    // (While the build is paused, every lease is held, whatever went quiet meanwhile.)
    {
        let mut s = shared.lock().unwrap();
        if s.paused.is_some() {
            s.leases.hold_all(now);
        }
    }
    let ok = serde_json::json!({ "ok": true });
    // (An accepted device works as itself alone: a worker named for its page's id.)
    let mine = |worker: &str| caller.device.as_ref().is_none_or(|(id, _)| devices::names(worker, id));
    let not_mine = || -> Result<(u16, serde_json::Value)> { Ok((403, serde_json::json!({ "error": "a device helps with its own tasks, as itself, alone" }))) };
    match path {
        // A page asking to help, with the secret it made (no key: the owner's answer makes the
        // secret its key); where its ask stands, with its code while it waits (shown on the build
        // Mac with it); its ask withdrawn.
        "/work/join" | "/work/join/state" | "/work/join/cancel" => {
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let (id, secret) = (b["id"].as_str().unwrap_or(""), b["secret"].as_str().unwrap_or(""));
            let now = unix_now();
            match path {
                "/work/join/state" => {
                    let (st, code) = shared.lock().unwrap().devices.state(id, secret, now);
                    Ok((200, serde_json::json!({ "state": st.name(), "code": code })))
                }
                "/work/join/cancel" => {
                    let mut s = shared.lock().unwrap();
                    let cancelled = s.devices.cancel(id, secret);
                    if cancelled {
                        s.save_devices(false);
                    }
                    Ok((200, serde_json::json!({ "cancelled": cancelled })))
                }
                _ => {
                    // (Its name and code from fresh randomness, read before the lock.)
                    let rand = u128::from_str_radix(&crate::net::random()?, 16)?;
                    let label = b["label"].as_str().unwrap_or("");
                    let mut s = shared.lock().unwrap();
                    match s.devices.ask(&devices::Asked { id, label, secret, from: &caller.from }, now, rand)? {
                        devices::Joined::Stands { state, code, new } => {
                            if new {
                                s.save_devices(false);
                            }
                            Ok((200, serde_json::json!({ "state": state.name(), "code": code })))
                        }
                        devices::Joined::TooMany(why) => Ok((429, serde_json::json!({ "error": why }))),
                    }
                }
            }
        }
        // The owner's, on this Mac itself (the menu bar, `scenic devices`): the asks and the devices;
        // an ask answered by its code (with its name, from a notification: that ask alone), devices
        // forgotten (by an ask's name, or their page's id).
        p if p.starts_with("/work/devices") && !(local && caller.owner) => anyhow::bail!("{p} is for this Mac's owner"),
        "/work/devices" => Ok((200, serde_json::to_value(shared.lock().unwrap().devices.view(unix_now()))?)),
        "/work/devices/accept" | "/work/devices/decline" => {
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let accept = path.ends_with("accept");
            let mut s = shared.lock().unwrap();
            let Some(d) = s.devices.answer(b["ask"].as_str(), b["code"].as_str().unwrap_or(""), accept, unix_now()) else {
                return Ok((404, serde_json::json!({ "error": "no ask waits with that code: answered, lapsed or cancelled" })));
            };
            s.save_devices(true);
            s.history.add(history::Event { worker: Some(format!("{} {}", d.label, d.id)), note: (if accept { "accepted: it may help" } else { "declined" }).into(), ..history::Event::new("device") });
            Ok((200, serde_json::json!({ "id": d.id, "label": d.label })))
        }
        "/work/devices/forget" => {
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let mut s = shared.lock().unwrap();
            let gone = s.devices.forget(b["which"].as_str().unwrap_or(""));
            if gone.is_empty() {
                return Ok((404, serde_json::json!({ "error": "no such device" })));
            }
            s.save_devices(true);
            for d in &gone {
                s.history.add(history::Event { worker: Some(format!("{} {}", d.label, d.id)), note: "forgotten: it no longer helps".into(), ..history::Event::new("device") });
            }
            Ok((200, serde_json::json!({ "forgotten": gone.len() })))
        }
        "/work/ask" => {
            let a: Ask = serde_json::from_slice(body)?;
            anyhow::ensure!(!a.worker.is_empty() && a.worker.len() <= 120, "a worker needs a name");
            // (A device: a page's tasks, the tails, as itself.)
            if caller.device.is_some() && (a.kind != "web" || a.can.iter().any(|c| c != "tail") || !mine(&a.worker)) {
                return not_mine();
            }
            let mut s = shared.lock().unwrap();
            // The build paused: nothing (an agent told why, and the pause; a page, nothing now).
            if let Some(p) = s.paused.clone() {
                s.seen(&a.worker, format!("waiting: {}", p.why()), None, now);
                return Ok(if a.kind == "native" { (409, serde_json::json!({ "error": p.why(), "pause": p })) } else { (204, serde_json::Value::Null) });
            }
            // An agent on an older app than this one's (its updater not yet run) waits for this one's
            // or a newer (this Mac's agent switches between jobs): its work would be built with
            // other code than the keys it records say. (Before it's seen as a worker that does
            // tasks: a unit job's tails would wait for it.)
            if a.kind == "native" && !app_ok(a.app.as_deref(), &s.app) {
                let theirs = a.app.as_deref().map_or("an older one".to_string(), app_when);
                let why = format!("on the app of {theirs}, the build Mac on {}'s: it builds once it runs that one or a newer", app_when(&s.app));
                s.seen(&a.worker, why.clone(), None, now);
                return Ok((409, serde_json::json!({ "error": why })));
            }
            s.seen(&a.worker, format!("asked for {}", a.can.join(" or ")), Some(&a), now);
            if s.workers[&a.worker].bad {
                return Ok((204, serde_json::Value::Null));
            }
            // The work only it can do first: a worker that mounts the NAS does a job of the plan (the
            // most work for what it fetches), the earliest step it can (what later steps wait on),
            // then a task; any other, a task.
            // (What fits its usual memory first, of any step; then, its owner away, what fits the
            // more it spares then.)
            if !s.pass.is_empty() {
                let offers = s.offers.clone();
                let passes: &[bool] = if a.more_mb.is_some() && a.max_secs.is_some() { &[false, true] } else { &[false] };
                for &away in passes {
                    for o in offers.iter().filter(|o| a.can.contains(&o.step)) {
                        let pick = s.pick(o, &a, now, away);
                        if pick.is_empty() {
                            continue;
                        }
                        let lease = s.leases.grant(&a.worker, Work::Job { step: o.step.clone(), targets: pick.clone() }, now);
                        s.save_leases();
                        eprintln!("coordinator: {} took {} {}", a.worker, o.step, pick.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" "));
                        s.history.add(history::Event { worker: Some(a.worker.clone()), lease: Some(lease), step: Some(o.step.clone()), targets: pick.iter().map(|t| t.0.clone()).collect(), ..history::Event::new("lease") });
                        let g = Grant { lease, ttl_s: TTL.as_secs(), work: Granted::Job { step: o.step.clone(), targets: pick, pass: s.pass.clone() } };
                        return Ok((200, serde_json::to_value(g)?));
                    }
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
            if !mine(&b.worker) {
                return not_mine();
            }
            let mut s = shared.lock().unwrap();
            let alive = s.leases.renew_frac(b.lease, &b.worker, b.progress.clone(), b.frac, now);
            s.seen(&b.worker, b.progress.unwrap_or_else(|| "working".into()), None, now);
            // (The build paused: its job pauses too, its lease kept meanwhile.)
            Ok((200, serde_json::json!({ "ok": alive, "pause": s.paused })))
        }
        "/work/pause" => {
            // A Mac's ask (its menu, `scenic pause`, the map), passed on by its agent, or an accepted
            // device's (its page's Pause): the build paused or going on, for every worker.
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let mut pause: Option<crate::control::Pause> = serde_json::from_value(b["pause"].clone())?;
            // (When it was asked: an agent says, by its Mac's clock, no later than AHEAD_S from now by
            // this one's; a page or an older helper doesn't, and it's now, by this Mac's: a browser's
            // clock isn't to be trusted with the order. A device's pause is by what it is.)
            let unix = unix_now();
            let at = match b["at"].as_u64().filter(|_| caller.device.is_none()) {
                Some(at) => at.min(unix + AHEAD_S),
                None => {
                    if let Some(p) = pause.as_mut() {
                        p.at = unix;
                    }
                    unix
                }
            };
            if let (Some(p), Some((_, label))) = (pause.as_mut(), &caller.device) {
                p.by = label.clone();
            }
            set_pause(&mut shared.lock().unwrap(), pause, at);
            Ok((200, ok))
        }
        "/work/done" => {
            let d: Done = serde_json::from_slice(body)?;
            if !mine(&d.worker) {
                return not_mine();
            }
            let mut s = shared.lock().unwrap();
            let Some(l) = s.leases.get(d.lease, &d.worker, now).cloned() else {
                // Its work was offered again (or done by another): this one's hand-off is dropped,
                // but for the raw tiles' archives it put on the NAS (the NAS's own tiles, whatever
                // became of the lease: their names kept, `raw_again`).
                drop(s);
                raw_again(journal, d.handoff.as_ref());
                return Ok((410, serde_json::json!({ "error": "that lease is gone" })));
            };
            match &l.work {
                // (A device hands back tasks alone.)
                Work::Job { .. } if caller.device.is_some() => return not_mine(),
                Work::Job { step, targets } => {
                    let h = d.handoff.unwrap_or_default();
                    // Refused (422): the worker gives the lease back as failed, and drops the work
                    // (but for its raw tiles' archives, as above).
                    if let Err(e) = check_handoff(&h, step, targets) {
                        // (From a worker on a newer app than this Mac's, whose step may save what
                        // this one's doesn't know of: its lease ends, not held against its targets.)
                        if s.workers.get(&d.worker).is_some_and(|w| !s.app.is_empty() && w.app.as_deref() != Some(s.app.as_str())) {
                            s.leases.finish(d.lease, &d.worker, now);
                            s.save_leases();
                        }
                        drop(s);
                        raw_again(journal, Some(&h));
                        return Ok((422, serde_json::json!({ "error": format!("{e:#}") })));
                    }
                    // Its lease ended and its units kept out of offers now (those it did: a job paused
                    // at a safe point did some), then the journal written without the lock (a whole
                    // file, flushed); put back if that fails.
                    s.leases.finish(d.lease, &d.worker, now);
                    let lease_targets = targets;
                    let targets: Vec<(String, String)> = h.done.as_ref().map(|d| d.1.clone()).unwrap_or_default();
                    for (t, k) in &targets {
                        s.done.insert((step.clone(), t.clone()), k.clone());
                        s.failed.remove(&(d.worker.clone(), cost_key(step, t)));
                    }
                    // (Failed after these: the rest held against the worker, as a failure's.)
                    if d.failed {
                        for (t, _) in lease_targets.iter().filter(|t| !targets.contains(t)) {
                            let e = s.failed.entry((d.worker.clone(), cost_key(step, t))).or_insert((now, 0));
                            *e = (now, e.1 + 1);
                        }
                    }
                    drop(s);
                    if let Err(e) = crate::handoff::write(&journal.join(folder(&d.worker)), &h) {
                        let mut s = shared.lock().unwrap();
                        for (t, _) in &targets {
                            s.done.remove(&(step.clone(), t.clone()));
                        }
                        s.save_leases();
                        return Err(e.context("journal the hand-off"));
                    }
                    s = shared.lock().unwrap();
                    let by = d.worker.clone();
                    s.costs.extend(d.costs.into_iter().filter(|(u, _)| targets.iter().any(|t| *u == cost_key(step, &t.0))).map(|(u, c)| (u, Cost { worker: Some(by.clone()), ..c })));
                    s.save_leases();
                    s.save_costs();
                    let secs = now.duration_since(l.granted).as_secs_f64();
                    s.history.add(history::Event { worker: Some(d.worker.clone()), lease: Some(d.lease), step: Some(step.clone()), targets: targets.iter().map(|t| t.0.clone()).collect(), secs: Some(secs.round()), ok: Some(!d.failed), ..history::Event::new("done") });
                }
                Work::Task { .. } => {
                    let unit = match s.tasks.done(d.lease, &d.worker, d.outputs, d.removed, d.secs, d.peak_mb) {
                        Ok(u) => u,
                        Err(e) => return Ok((422, serde_json::json!({ "error": format!("{e:#}") }))),
                    };
                    // What its unit's task takes, for the next time it's offered.
                    if let Some(u) = &unit {
                        s.costs.insert(format!("tail {u}"), Cost { peak_mb: d.peak_mb, secs: d.secs as u64, worker: Some(d.worker.clone()), v: 0 });
                        s.save_costs();
                    }
                    s.leases.finish(d.lease, &d.worker, now);
                    s.history.add(history::Event { worker: Some(d.worker.clone()), lease: Some(d.lease), step: Some("tail".into()), targets: unit.into_iter().collect(), secs: Some(d.secs.round()), ok: Some(true), ..history::Event::new("task") });
                }
            }
            s.workers.get_mut(&d.worker).map(|w| w.done += 1);
            s.seen(&d.worker, format!("finished {}", l.what()), None, now);
            eprintln!("coordinator: {} finished {}", d.worker, l.what());
            Ok((200, ok))
        }
        "/work/fail" => {
            let f: Fail = serde_json::from_slice(body)?;
            if !mine(&f.worker) {
                return not_mine();
            }
            let mut s = shared.lock().unwrap();
            let Some(l) = s.leases.finish(f.lease, &f.worker, now) else { return Ok((410, serde_json::json!({ "error": "that lease is gone" }))) };
            match &l.work {
                Work::Job { step, targets } => {
                    if !f.interrupted {
                        for (t, _) in targets {
                            let e = s.failed.entry((f.worker.clone(), cost_key(step, t))).or_insert((now, 0));
                            *e = (now, e.1 + 1);
                        }
                    }
                    s.save_leases();
                }
                Work::Task { .. } => {
                    if let (Some(id), Some(peak)) = (s.tasks.fail_how(f.lease, &f.worker, &f.error, f.oom_mb, f.interrupted), f.oom_mb.filter(|_| !f.interrupted)) {
                        // Out of memory at `peak`: it takes more than that, next time too.
                        if let Some(u) = s.tasks.by_id.get(&id).and_then(|t| t.spec["unit"].as_str().map(str::to_string)) {
                            let c = s.costs.entry(format!("tail {u}")).or_default();
                            c.peak_mb = c.peak_mb.max(peak + peak / 4);
                            s.save_costs();
                        }
                    }
                }
            }
            if !f.interrupted {
                s.workers.get_mut(&f.worker).map(|w| w.failed += 1);
            }
            let why: String = f.error.chars().take(300).collect();
            let e = match &l.work {
                Work::Job { step, targets } => history::Event { step: Some(step.clone()), targets: targets.iter().map(|t| t.0.clone()).collect(), ..history::Event::new("fail") },
                Work::Task { .. } => history::Event { step: Some("tail".into()), ..history::Event::new("task-fail") },
            };
            let note = if f.interrupted { format!("stopped: {why}") } else { why.clone() };
            s.history.add(history::Event { worker: Some(f.worker.clone()), lease: Some(f.lease), secs: Some(now.duration_since(l.granted).as_secs_f64().round()), ok: Some(false), note, ..e });
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
            Ok((200, serde_json::json!({ "pass": s.pass, "paused": s.paused, "offered": offered, "done": s.done.len(), "leases": leases, "workers": workers, "tasks": tasks })))
        }
        "/work/swarm" => {
            // The whole build at a glance, for the worker page: the build Mac's heartbeat (its
            // agent's status, written each loop beside this folder: its job, the checklist to the
            // end, the forecast, the regions, its helpers), the leases, the workers, the tasks and
            // the history's hours.
            let s = shared.lock().unwrap();
            let agent: serde_json::Value = s.dir.parent().and_then(|h| std::fs::read(h.join("status.json")).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(serde_json::Value::Null);
            // Each lease: whose, what (its step and targets), for how long, what it last said and how
            // far, since its last beat and until it lapses.
            let leases: Vec<serde_json::Value> = s
                .leases
                .all(now)
                .iter()
                .map(|l| {
                    let (step, n) = match &l.work {
                        Work::Job { step, targets } => (step.clone(), targets.len()),
                        Work::Task { .. } => ("tail".to_string(), 1),
                    };
                    let left = l.left(now).as_secs();
                    serde_json::json!({ "id": l.id, "worker": l.worker, "what": l.what(), "step": step, "n": n, "for_s": now.duration_since(l.granted).as_secs(), "progress": l.progress, "frac": l.frac, "beat_s": TTL.as_secs().saturating_sub(left), "lapses_in_s": left })
                })
                .collect();
            // Each worker, and for one that does the shared steps, how the work offered fits it.
            let workers: Vec<serde_json::Value> = s
                .workers
                .iter()
                .map(|(n, w)| {
                    let fit = w.ask.as_ref().filter(|a| a.kind == "native").map(|a| s.fit(a, now)).unwrap_or_default();
                    serde_json::json!({ "name": n, "label": w.label, "kind": w.kind, "what": w.what, "mem_mb": w.mem_mb, "cores": w.cores, "done": w.done, "failed": w.failed, "checked": w.checked, "bad": w.bad, "app": w.app, "visible": w.visible, "can": w.can, "fit": fit, "seen_s": now.duration_since(w.seen).as_secs() })
                })
                .collect();
            // The tasks, by state.
            let mut tasks: BTreeMap<&str, usize> = BTreeMap::new();
            for t in s.tasks.by_id.values() {
                let st = match t.state {
                    task::State::Offered => "offered",
                    task::State::Leased { .. } => "leased",
                    task::State::Done { .. } => "done",
                    task::State::Failed { .. } => "failed",
                };
                *tasks.entry(st).or_default() += 1;
            }
            let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            // The history's last number (the page asks `/work/history` for what's after the one it
            // has) and the last day by the hour.
            Ok((200, serde_json::json!({ "now": unix, "pause": s.paused, "agent": agent, "leases": leases, "workers": workers, "tasks": tasks, "seq": s.history.seq(), "rates": s.history.rates(unix, 24) })))
        }
        "/work/history" => {
            // What happened after event `since` (the oldest first, at most `max`, 500 by default).
            let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let since = b["since"].as_u64().unwrap_or(0);
            let max = b["max"].as_u64().map_or(500, |m| m.min(5000)) as usize;
            let s = shared.lock().unwrap();
            Ok((200, serde_json::json!({ "seq": s.history.seq(), "events": s.history.since(since, max) })))
        }
        p if p.starts_with("/task/") && !local => anyhow::bail!("{p} is for this Mac's jobs"),
        "/task/offer" => {
            let mut o: task::Offer = serde_json::from_slice(body)?;
            let mut s = shared.lock().unwrap();
            // What its unit's task took last time (a worker's measure, with some room), in place of
            // the guess.
            if let Some(c) = o.spec["unit"].as_str().and_then(|u| s.costs.get(&format!("tail {u}"))).filter(|c| c.peak_mb > 0) {
                o.mem_mb = c.peak_mb + c.peak_mb / 10;
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
        // The build at a glance, both pages': its script and styles.
        ("dash.js", "text/javascript", include_bytes!("../../../../web/work/dash.js")),
        ("dash.css", "text/css; charset=utf-8", include_bytes!("../../../../web/work/dash.css")),
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
        /// The NAS's project folder (Coordinator::set_root), and the data servers' files opened for
        /// tasks (`net`).
        pub root: Arc<Mutex<Option<PathBuf>>>,
        pub remote: Arc<Remote>,
    }

    /// The data servers' files opened for tasks, kept (the most recent `REMOTE_FILES`), each with
    /// the block cache crate::fetch keeps for it (16 MB at most): a task reads a file a block at a
    /// time. Fetched over HTTPS only, following no redirect: a task names only the servers
    /// `WEB_HOSTS` allows, and a redirect would take the fetch elsewhere.
    #[derive(Default)]
    pub struct Remote {
        files: Mutex<std::collections::VecDeque<(String, Option<Arc<dyn store::range::RangeRead>>)>>,
    }

    impl Remote {
        /// `url`'s file (None: the server has none), opened once.
        fn open(&self, url: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
            if let Some((_, f)) = self.files.lock().unwrap().iter().find(|(u, _)| u == url) {
                return Ok(f.clone());
            }
            use crate::fetch::Fetch;
            let fetch = crate::fetch::Fetcher::strict();
            // (A small file a server won't read in ranges, a map tile: whole.)
            let f = match fetch.open(url) {
                Ok(f) => f,
                Err(e) => match fetch.get(url) {
                    Ok(b) => b.map(|b| Arc::new(b) as Arc<dyn store::range::RangeRead>),
                    Err(_) => return Err(e),
                },
            };
            let mut files = self.files.lock().unwrap();
            files.push_back((url.to_string(), f.clone()));
            while files.len() > REMOTE_FILES {
                files.pop_front();
            }
            Ok(f)
        }
    }

    /// The data servers' files kept open for tasks (`Remote`): a few hundred MB of blocks at most.
    const REMOTE_FILES: usize = 24;
    /// The most of a file one request reads (a page reads 1 MB blocks).
    const RANGE_MAX: u64 = 8 << 20;

    pub use crate::net::{allowed, random, served_https, urls};

    /// The workers' token: made once and kept (`workers-token`), so workers keep theirs across
    /// restarts. (`token`, the one pages carried before devices asked, in their address and their
    /// storage, is no key any more: gone. The Macs' agents read the new one from the NAS once
    /// theirs is refused.)
    pub fn token(dir: &Path) -> Result<String> {
        std::fs::remove_file(dir.join("token")).ok();
        crate::net::kept_token(&dir.join("workers-token"))
    }

    /// Listens on `port`, on a runtime of its own; this Mac's addresses for workers.
    pub fn serve(port: u16, ctx: Ctx) -> Result<Vec<String>> {
        let listener = std::net::TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("listen on port {port}"))?;
        listener.set_nonblocking(true)?;
        let app = Router::new()
            .route("/work", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/work/")]) }))
            .route("/work/", get(|| page(Url(String::new()))))
            // (The watching page's old address: the page is the dashboard now, helping opt-in.)
            .route("/work/watch", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/work/")]) }))
            .route("/work/watch/", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/work/")]) }))
            .route("/work/{*file}", get(page))
            .route("/work/ask", any(json))
            .route("/work/beat", any(json))
            .route("/work/pause", any(json))
            .route("/work/done", any(json))
            .route("/work/fail", any(json))
            .route("/work/status", any(json))
            .route("/work/swarm", any(json))
            .route("/work/history", any(json))
            .route("/work/join", any(json))
            .route("/work/join/state", any(json))
            .route("/work/join/cancel", any(json))
            .route("/work/devices", any(json))
            .route("/work/devices/accept", any(json))
            .route("/work/devices/decline", any(json))
            .route("/work/devices/forget", any(json))
            .route("/work/in/{lease}/{*path}", get(input))
            .route("/work/net/{lease}/{*path}", get(net))
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

    /// A request's bearer credential.
    fn bearer(h: &HeaderMap) -> Option<&str> {
        h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "))
    }

    /// Who may ask what. This Mac, its LAN and the tailnet alone, and a request naming this Mac and
    /// from no web page elsewhere (crate::net::ours, from_the_page: DNS rebinding, a site's
    /// requests through a browser that reaches here). Then: the page, what its dashboard reads (the
    /// build at a glance, its history) and a page's ask to help, without a key; the Macs' agents'
    /// requests, with the build's key; an accepted device's (crate::coord::devices), with its own,
    /// for its own tasks (its asks, beats, hand-backs and failures, its tasks' files and the
    /// programs: `route` sees the worker it names is its own) and pausing the build alone; the
    /// owner's (the asks and the devices), with the build's key, and a job's (`/task/…`), with its
    /// own, from this Mac itself (crate::net::own: `tailscale serve` hands the tailnet's requests
    /// over from loopback). Who it is goes on to the request (`Caller`).
    async fn gate(State(c): State<Ctx>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
        let ip = peer.ip();
        if !allowed(ip) {
            return error(StatusCode::FORBIDDEN, "not from here");
        }
        // (No Host at all: not a browser.)
        if req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).is_some_and(|h| !crate::net::ours(h)) {
            return error(StatusCode::FORBIDDEN, "not this build Mac's address");
        }
        if !crate::net::from_the_page(req.headers()) {
            return error(StatusCode::FORBIDDEN, "not from the build page");
        }
        let path = req.uri().path().to_string();
        let key = bearer(req.headers()).map(str::to_string);
        let owner = key.as_deref().is_some_and(|k| crate::net::same(k, &c.token));
        let device = match key.as_deref() {
            Some(k) if !owner => c.shared.lock().unwrap().devices.knows(k).map(|d| (d.id.clone(), d.label.clone())),
            _ => None,
        };
        let caller = Caller { local: crate::net::own(ip, req.headers()), owner, device: device.clone(), from: crate::net::source(ip, req.headers()) };
        let local = caller.local;
        req.extensions_mut().insert(caller);
        let is_page = req.method() == Method::GET && (matches!(path.as_str(), "/work" | "/work/watch" | "/work/watch/") || path.strip_prefix("/work/").is_some_and(|p| PAGE.iter().any(|(n, _, _)| *n == p)));
        let is_view = req.method() == Method::POST && matches!(path.as_str(), "/work/swarm" | "/work/history" | "/work/join" | "/work/join/state" | "/work/join/cancel");
        if is_page || is_view {
            return next.run(req).await;
        }
        // The owner's and a job's: with their key, from this Mac itself.
        let job = key.as_deref().is_some_and(|k| crate::net::same(k, &c.job_token));
        let theirs = match () {
            _ if path.starts_with("/task/") => Some((job, "this Mac's job's")),
            _ if path.starts_with("/work/devices") => Some((owner, "this Mac's owner's")),
            _ => None,
        };
        if let Some((ok, whose)) = theirs {
            return match () {
                _ if !ok => error(StatusCode::UNAUTHORIZED, format!("not {whose}")),
                _ if !local => error(StatusCode::FORBIDDEN, "that comes from the build Mac itself"),
                _ => next.run(req).await,
            };
        }
        if owner {
            return next.run(req).await;
        }
        let Some((id, _)) = device else { return error(StatusCode::UNAUTHORIZED, "not a helper's: ask the build Mac to let this device help") };
        let files = ["/work/in/", "/work/net/", "/work/out/"].iter().any(|p| path.starts_with(p));
        let theirs = match *req.method() {
            Method::POST => matches!(path.as_str(), "/work/ask" | "/work/beat" | "/work/done" | "/work/fail" | "/work/pause"),
            Method::GET => (files && !path.starts_with("/work/out/")) || path.starts_with("/work/prog/"),
            Method::PUT => path.starts_with("/work/out/"),
            _ => false,
        };
        if !theirs || (files && !devices::names(&worker(req.headers()), &id)) {
            return error(StatusCode::FORBIDDEN, "a device helps with its own tasks, as itself, alone");
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

    /// A JSON request, answered by `route` off the runtime (it takes the lock and may write a file):
    /// a POST of JSON alone (a page elsewhere can't send that without asking first, which nothing
    /// here answers), from whom the gate found.
    async fn json(State(c): State<Ctx>, req: Request) -> Response {
        let path = req.uri().path().to_string();
        let is_json = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).and_then(|t| t.split(';').next()).is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
        if req.method() != Method::POST || !is_json {
            return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "a POST of JSON (application/json)");
        }
        let caller = req.extensions().get::<Caller>().cloned().unwrap_or_default();
        let body = match tokio::time::timeout(JSON_TIME, axum::body::to_bytes(req.into_body(), JSON_MAX)).await {
            Ok(Ok(b)) => b,
            Ok(Err(_)) => return error(StatusCode::PAYLOAD_TOO_LARGE, "too big, or cut short"),
            Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "its body didn't come"),
        };
        match tokio::task::spawn_blocking(move || route(&path, &body, &c.shared, &c.journal, &caller)).await {
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

    /// What a task reads where it lies (docs/workers.md §3, Read where they lie), for the worker
    /// holding its lease: `nas/<path>` the build's data on the NAS (under `NAS_PATHS`), `web/<host>/
    /// <path>` a data server's file (one of `WEB_HOSTS`, over HTTPS). `?probe`: what's there
    /// ({kind, size}; 404 when nothing is); `?list`: a NAS folder's entries; else its bytes, a range
    /// at a time.
    async fn net(State(c): State<Ctx>, Url((lease, path)): Url<(u64, String)>, q: axum::extract::RawQuery, h: HeaderMap) -> Response {
        if c.shared.lock().unwrap().tasks.by_lease(lease, &worker(&h)).is_none() {
            return error(StatusCode::GONE, "that lease is gone");
        }
        let query = q.0.unwrap_or_default();
        let range = h.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
        if let Some(rel) = path.strip_prefix("nas/") {
            let Some(rel) = task::safe(rel).filter(|r| NAS_PATHS.iter().any(|n| r.to_string_lossy().starts_with(n) || n.trim_end_matches('/') == r.to_string_lossy())) else {
                return error(StatusCode::FORBIDDEN, "not what a task reads");
            };
            let Some(root) = c.root.lock().unwrap().clone() else { return error(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't here now") };
            let p = root.join(rel);
            // (Not there, or the NAS not answering: a task fails rather than take the second for
            // the first, as Taiwan's elevations would, from FABDEM.)
            let meta = match tokio::fs::metadata(&p).await {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return error(StatusCode::NOT_FOUND, "nothing there"),
                Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, format!("the NAS didn't answer: {e}")),
            };
            return match query.as_str() {
                "probe" => Json(if meta.is_dir() { serde_json::json!({ "kind": "dir" }) } else { serde_json::json!({ "kind": "file", "size": meta.len() }) }).into_response(),
                "list" => {
                    // (A listing read in part, the NAS not answering, is an error, never fewer
                    // entries: Taiwan's MOI DTM files listed as none would be FABDEM's elevations.
                    // An entry gone since it was listed is just gone.)
                    let listed = (|| -> std::io::Result<Vec<(String, &str, u64)>> {
                        let mut entries = Vec::new();
                        for e in std::fs::read_dir(&p)? {
                            let e = e?;
                            let m = match e.metadata() {
                                Ok(m) => m,
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                                Err(e) => return Err(e),
                            };
                            entries.push((e.file_name().to_string_lossy().into_owned(), if m.is_dir() { "dir" } else { "file" }, m.len()));
                        }
                        entries.sort();
                        Ok(entries)
                    })();
                    match listed {
                        Ok(entries) => Json(serde_json::json!({ "entries": entries })).into_response(),
                        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, format!("the NAS didn't list it: {e}")),
                    }
                }
                _ => send_file(&p, "application/octet-stream", None, range.as_deref()).await,
            };
        }
        let Some((host, rest)) = path.strip_prefix("web/").and_then(|r| r.split_once('/')) else { return error(StatusCode::NOT_FOUND, "nas/… or web/<host>/…") };
        if !WEB_HOSTS.contains(&host) {
            return error(StatusCode::FORBIDDEN, "not a data server a task reads");
        }
        let url = format!("https://{host}/{rest}");
        let remote = c.remote.clone();
        let read = tokio::task::spawn_blocking(move || -> Result<Response> {
            let Some(f) = remote.open(&url)? else { return Ok(error(StatusCode::NOT_FOUND, "the server has none")) };
            let len = f.len().map_err(|e| anyhow::anyhow!("{e}"))?;
            if query == "probe" {
                return Ok(Json(serde_json::json!({ "kind": "file", "size": len })).into_response());
            }
            let (a, b) = match range.as_deref().and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-')) {
                Some((a, b)) => (a.parse::<u64>()?, if b.is_empty() { len.saturating_sub(1) } else { b.parse::<u64>()?.min(len.saturating_sub(1)) }),
                None => (0, len.saturating_sub(1)),
            };
            anyhow::ensure!(a <= b && b < len && b - a < RANGE_MAX, "a range of at most {} MB in the file", RANGE_MAX >> 20);
            let bytes = f.read_at(a, (b - a + 1) as usize).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut resp = (StatusCode::PARTIAL_CONTENT, [(header::CONTENT_TYPE, "application/octet-stream"), (header::CACHE_CONTROL, "no-store")], bytes).into_response();
            if let Ok(v) = format!("bytes {a}-{b}/{len}").parse() {
                resp.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            Ok(resp)
        })
        .await;
        match read {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => error(StatusCode::BAD_GATEWAY, format!("{e:#}")),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
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
        // (Named by its lease too: a stale upload of the same file, still streaming for the lease
        // before, never shares or removes it.)
        let mut part = Part(PathBuf::from(format!("{}.{lease}.part", dest.display())), false);
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
        let c = Coordinator::start(&d.path().join("coord"), None, port, "m4", "").unwrap();
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
    fn a_cost_measured_another_way_says_nothing_of_memory() {
        // Terrain measured holding its whole area (before it wrote a z6 tile at a time): its
        // estimate instead; measured since: what it took.
        let mut costs = BTreeMap::new();
        costs.insert(cost_key("terrain", "3/0/2"), Cost { peak_mb: 32_900, secs: 3000, worker: None, v: 0 });
        assert_eq!(job_peak(&costs, "terrain", "3/0/2", 2400), 2400);
        costs.insert(cost_key("terrain", "3/0/2"), Cost { peak_mb: 2100, secs: 3000, worker: None, v: cost_version("terrain") });
        assert_eq!(job_peak(&costs, "terrain", "3/0/2", 2400), 2100);
        // Slope likewise (written a z6 tile at a time too); other steps' measures stand.
        costs.insert(cost_key("slope", "3/0/2"), Cost { peak_mb: 20_000, secs: 300, worker: None, v: 0 });
        assert_eq!(job_peak(&costs, "slope", "3/0/2", 2000), 2000);
        costs.insert(cost_key("peaks", "6/1/1"), Cost { peak_mb: 5000, secs: 300, worker: None, v: 0 });
        assert_eq!(job_peak(&costs, "peaks", "6/1/1", 2500), 5000);
    }

    #[test]
    fn the_history_and_the_swarm_say_what_happened_and_why() {
        let (_d, c, w) = start();
        let units: Vec<(String, String, u64)> = (1..=3).map(|i| (format!("6/1/{i}"), format!("k{i}"), 100 << 20)).collect();
        c.offer_units("2026-09-28", units);
        // A worker that spares 4 GB takes two areas (the third too big), hands one back.
        c.shared.lock().unwrap().costs.insert("6/1/1".into(), Cost { peak_mb: 9000, secs: 60, worker: None, v: 0 });
        let g = w.ask(&Ask { max: 2, ..ask(4096) }).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g.work else { panic!("a job") };
        let one: Vec<(&str, &str)> = targets.iter().take(1).map(|(t, k)| (t.as_str(), k.as_str())).collect();
        w.done(&Done { worker: "m1".into(), lease: g.lease, handoff: Some(handoff(&one)), ..Default::default() }).unwrap();
        c.note(history::Event { worker: Some("m4".into()), targets: vec!["ohio".into()], ..history::Event::new("catalog") });
        // What happened, in order, after a number.
        let (code, h) = w.post_json("/work/history", &serde_json::json!({ "since": 0 })).unwrap();
        assert_eq!(code, 200);
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["agent", "worker", "lease", "done", "catalog"]);
        let last = h["seq"].as_u64().unwrap();
        assert!(w.post_json("/work/history", &serde_json::json!({ "since": last })).unwrap().1["events"].as_array().unwrap().is_empty());
        // The swarm: the history's number and the hours; the worker's fit (the area it holds, the one
        // too big for it); the tasks by state.
        let (_, sw) = w.post_json("/work/swarm", &serde_json::json!({})).unwrap();
        assert_eq!(sw["seq"].as_u64(), Some(last));
        assert_eq!(sw["rates"]["rows"].as_array().unwrap().len(), 24);
        let m1 = sw["workers"].as_array().unwrap().iter().find(|x| x["name"] == "m1").unwrap();
        let fit = &m1["fit"][0];
        assert_eq!((fit["offered"].as_u64(), fit["done"].as_u64(), fit["too_big"].as_u64(), fit["fits"].as_u64()), (Some(3), Some(1), Some(1), Some(1)));
        assert!(sw["tasks"].is_object() && sw["leases"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_builds_pause_holds_for_every_worker() {
        use crate::control::{Mode, Pause};
        let (d, c, w) = start();
        let units: Vec<(String, String, u64)> = (1..=3).map(|i| (format!("6/1/{i}"), format!("k{i}"), 100 << 20)).collect();
        c.offer_units("2026-09-28", units.clone());
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        // Paused (from a helper's menu, passed on by its agent): no work; an agent's told why and the
        // pause, a page nothing; a worker's beat hears it; no lease lapses meanwhile.
        let p = Pause::new(Mode::Drain, "the menu bar on m1");
        w.set_pause(Some(&p), p.at).unwrap();
        // (An ask from before it, held by a Mac that couldn't reach this one: passed over.)
        w.set_pause(None, p.at - 60).unwrap();
        assert_eq!(c.pause().map(|p| p.by), Some("the menu bar on m1".to_string()));
        let e = w.ask(&ask(4096)).unwrap_err();
        let r = e.downcast_ref::<client::Refused>().unwrap();
        assert!(r.why.contains("paused") && r.pause.as_ref().is_some_and(|p| p.mode == Mode::Drain));
        let page = client::Client::at(w.urls(), c.contact.token.clone(), "ipad");
        assert!(page.ask(&Ask { kind: "web".into(), can: vec!["tail".into()], ..ask(1024) }).unwrap().is_none());
        assert!(w.beat_paused(g.lease, None).unwrap().1.is_some());
        assert!(c.expire().is_empty());
        // A restart keeps it.
        let port2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        drop(c);
        let c = Coordinator::start(&d.path().join("coord"), None, port2, "m4", "").unwrap();
        assert!(c.pause().is_some());
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port2}")], c.contact.token.clone(), "m1");
        c.offer_units("2026-09-28", units.clone());
        // Going on: work again. A job paused at a safe point hands off what it did (those done, the
        // rest offered again); one stopped before any is given back, not held against its targets.
        c.set_pause(None, p.at + 1);
        assert_eq!(w.beat_paused(g.lease, None).unwrap(), (true, None), "its lease kept through the pause and the restart");
        let Granted::Job { targets, .. } = &g.work else { panic!() };
        assert_eq!(targets.len(), 3);
        let part = handoff(&[("6/1/3", "k3")]);
        assert_eq!(w.done(&Done { lease: g.lease, handoff: Some(part), ..Default::default() }).unwrap(), client::Handed::Taken);
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g.work else { panic!() };
        assert_eq!(targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["6/1/2", "6/1/1"]);
        w.give_back(g.lease, "the build paused before it finished an area").unwrap();
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g.work else { panic!() };
        assert_eq!(targets.len(), 2, "given back, not failed: offered to it again at once");
    }

    #[test]
    fn candidates_are_expected_to_take_what_their_unit_did() {
        let cost = |mb: u64| Cost { peak_mb: mb, secs: 1, worker: None, v: 2 };
        let mut costs = BTreeMap::new();
        // Before either ran: as a unit on its piece would (ten times it, at least 3.7 GB).
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 500 << 20), 5000);
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 100 << 20), 3700);
        // The unit's run, then its own.
        costs.insert("6/1/1".to_string(), cost(4200));
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 100 << 20), 4200);
        costs.insert("pois 6/1/1".to_string(), cost(1200));
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 100 << 20), 1200);
        // Another step: its own, else its offer's guess.
        assert_eq!(job_peak(&costs, "slope", "3/2/2", 3000), 3000);
    }

    #[test]
    fn an_agent_on_another_app_gets_no_work() {
        let d = tempfile::tempdir().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = Coordinator::start(&d.path().join("coord"), None, port, "m4", "20261005-1508-84142d3").unwrap();
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.offer_units("2026-09-28", vec![("6/1/1".into(), "k1".into(), 100 << 20)]);
        // One from before agents said (an older helper's ask), or on another: refused, why given.
        let e = w.ask(&ask(4096)).unwrap_err();
        assert!(e.downcast_ref::<client::Refused>().is_some_and(|r| r.why.contains("an older one") && r.why.contains("5 Oct 15:08 UTC") && r.pause.is_none()), "{e:#}");
        let e = w.ask(&Ask { app: Some("20261005-0819-7638722".into()), ..ask(4096) }).unwrap_err();
        assert!(e.to_string().contains("5 Oct 08:19 UTC"), "{e:#}");
        assert!(c.held("unit").is_empty());
        // On the same: the unit.
        let g = w.ask(&Ask { app: Some("20261005-1508-84142d3".into()), ..ask(4096) }).unwrap().unwrap();
        assert!(matches!(g.work, Granted::Job { ref step, .. } if step == "unit"));
        // A page (its code is this coordinator's) never says.
        assert_eq!(app_when("development"), "development");
        // A newer one builds (the build Mac's agent finishing a job on the last); development ones
        // only with their like.
        assert!(app_ok(Some("20261005-1613-d05125b"), "20261005-1508-84142d3"));
        assert!(!app_ok(Some("20261005-1508-aaaaaaa"), "20261005-1613-d05125b") && !app_ok(None, "20261005-1508-84142d3"));
        assert!(app_ok(Some("development"), "development") && !app_ok(Some("development"), "20261005-1508-84142d3") && !app_ok(Some("20261005-1613-d05125b"), "development"));
        assert!(app_ok(None, ""));
        // Never a panic, whatever a worker says.
        assert!(!app_ok(Some("20261005-150é-x"), "20261005-1508-84142d3") && !app_ok(Some("é"), "20261005-1508-84142d3") && !app_ok(Some(""), "x"));
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
        c.add_costs(&[(cost_key("terrain", "3/1/2"), Cost { peak_mb: 3500, secs: 1, worker: None, v: 2 })]);
        assert_eq!(next(&["terrain"]), "terrain 3/1/2");
        // A worker that can't do a step gets none of it.
        c.offer("p", vec![o("trees", &[("3/1/1", 1000)], 1)]);
        assert_eq!(next(&["unit"]), "none");
    }

    #[test]
    fn terrain_from_the_near_end_and_more_memory_while_the_owner_is_away_for_short_jobs() {
        let (_d, c, w) = start();
        let o = |step: &str, ts: &[(&str, u64)], batch: usize| Offer { step: step.into(), targets: ts.iter().map(|(t, m)| (t.to_string(), format!("k {t}"), *m)).collect(), batch };
        let next = |a: &Ask| match w.ask(a).unwrap().map(|g| g.work) {
            Some(Granted::Job { step, targets, .. }) => format!("{step} {}", targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" ")),
            _ => "none".into(),
        };
        // Terrain: the plan's first, the build Mac's next units' (not the far end's).
        c.offer("p", vec![o("terrain", &[("3/1/2", 3000), ("3/2/2", 3000)], 1)]);
        let can = |steps: &[&str], mem: u64| Ask { can: steps.iter().map(|s| s.to_string()).collect(), ..ask(mem) };
        assert_eq!(next(&can(&["terrain"], 4096)), "terrain 3/1/2");
        // Slope's three areas: one needs 5 GB, measured at 10 minutes for this helper; one 8 GB,
        // an hour; one 5 GB, never run the way slope runs now.
        c.offer("p", vec![o("slope", &[("3/1/1", 5000), ("3/1/2", 8000), ("3/1/3", 5000)], 3)]);
        c.add_costs_by(&[(cost_key("slope", "3/1/1"), Cost { peak_mb: 5000, secs: 600, worker: None, v: 2 }), (cost_key("slope", "3/1/2"), Cost { peak_mb: 8000, secs: 3600, worker: None, v: 2 })], "m1");
        // At its desk: none fits its 4 GB.
        assert_eq!(next(&can(&["slope"], 4096)), "none");
        // Away, sparing 10 GB for twenty minutes: the short one alone (the hour-long one, and the
        // one never measured, wait).
        let away = Ask { more_mb: Some(10240), max_secs: Some(1200), ..can(&["slope"], 4096) };
        assert_eq!(next(&away), "slope 3/1/1");
        assert_eq!(next(&away), "none");
        let fit = c.shared.lock().unwrap().fit(&away, Instant::now());
        assert_eq!((fit[0]["too_long"].as_u64(), fit[0]["held"].as_u64()), (Some(2), Some(1)));
        // What fits its usual memory comes first, whatever its time and its step (the agent offers
        // slope before the candidates).
        c.offer("p", vec![o("slope", &[("3/2/1", 5000)], 3), o("pois", &[("6/1/1", 1500)], 12)]);
        c.add_costs_by(&[(cost_key("slope", "3/2/1"), Cost { peak_mb: 5000, secs: 60, worker: None, v: 2 })], "m1");
        let both = Ask { more_mb: Some(10240), max_secs: Some(1200), ..can(&["slope", "pois"], 4096) };
        assert_eq!(next(&both), "pois 6/1/1");
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
        // Part of a lease's targets (a job paused at a safe point, or stopped): those it did, and
        // only their files.
        let two = vec![("3/2/2".to_string(), "k".to_string()), ("3/3/2".to_string(), "k".to_string())];
        assert!(check_handoff(&h("slope", "3/2/2", &["layers/slope/lo/3-2-2"], &[]), "slope", &two).is_ok());
        assert!(check_handoff(&h("slope", "3/2/2", &["layers/slope/lo/3-2-2", "layers/slope/lo/3-3-2"], &[]), "slope", &two).is_err(), "a file of a target it didn't do");
        assert!(check_handoff(&h("slope", "3/2/2", &["layers/slope/hi/6-24-16"], &[]), "slope", &ts("3/2/2")).is_err());
        assert!(check_handoff(&h("slope", "3/2/2", &["layers/terrain/hi/6-16-16"], &[]), "slope", &ts("3/2/2")).is_err());
        assert!(check_handoff(&h("trees", "3/2/2", &["layers/trees-leaf/hi/6-17-17", "layers/trees-cover/lo/3-2-2"], &[]), "trees", &ts("3/2/2")).is_ok());
        assert!(check_handoff(&h("pois", "6/1/3", &["work/pois/6-1-3"], &[]), "pois", &ts("6/1/3")).is_ok());
        assert!(check_handoff(&h("pois", "6/1/3", &["work/peaks/6-1-3"], &[]), "pois", &ts("6/1/3")).is_err());
        // Raw tiles' archives: its own areas, named by their content.
        let terrain = |raw: &[(&str, &str)]| check_handoff(&h("terrain", "3/2/2", &["layers/terrain/lo/3-2-2"], raw), "terrain", &ts("3/2/2"));
        assert!(terrain(&[("6-20-21", "6-20-21.0123456789abcdef.tiles"), ("3-2-2", "3-2-2.0123456789abcdef.tiles")]).is_ok());
        // Another area's (an earlier job's tiles, packed with this one's) too, and the root's.
        assert!(terrain(&[("6-30-21", "6-30-21.0123456789abcdef.tiles"), ("root", "root.0123456789abcdef.tiles")]).is_ok());
        assert!(terrain(&[("6-64-21", "6-64-21.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("5-20-21", "5-20-21.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-020-21", "6-020-21.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-20-21", "6-20-22.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-20-21", "../6-20-21.0123456789abcdef.tiles")]).is_err());
        let peaks = |a: &str| check_handoff(&h("peaks", "6/20/21", &["work/peaks/6-20-21"], &[(a, &format!("{a}.0123456789abcdef.tiles"))]), "peaks", &ts("6/20/21"));
        assert!(peaks("6-21-22").is_ok() && peaks("3-2-2").is_ok() && peaks("6-25-21").is_ok() && peaks("../6-25-21").is_err());
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
        // (Its raw tiles' archives, on the NAS whatever became of it, kept for naming: the well
        // named alone.)
        let mut bad = handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")]);
        bad.changes.insert("base/6-9-9".into(), None);
        bad.raw = vec![("6-1-1".into(), crate::rawpack::Pack { name: "6-1-1.0123456789abcdef.tiles".into(), bytes: 9 }), ("6-1-1".into(), crate::rawpack::Pack { name: "../x.tiles".into(), bytes: 9 })];
        assert!(matches!(w.done(&Done { lease: g.lease, handoff: Some(bad), ..Default::default() }).unwrap(), client::Handed::Refused(_)));
        let kept = crate::handoff::waiting_in(&c.journal()).unwrap();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].0.parent().unwrap().ends_with(crate::handoff::RAW_AGAIN) && kept[0].1.done.is_none() && kept[0].1.changes.is_empty());
        assert_eq!(kept[0].1.raw.iter().map(|r| r.1.name.as_str()).collect::<Vec<_>>(), ["6-1-1.0123456789abcdef.tiles"]);
        std::fs::remove_file(&kept[0].0).unwrap();
        assert_eq!(w.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")])), costs: vec![("6/1/3".into(), Cost { peak_mb: 2000, secs: 300, worker: None, v: 0 })], ..Default::default() }).unwrap(), client::Handed::Taken);
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
        c.finish(own, &[("6/1/4".to_string(), "k4".to_string())]);
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
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("Scenic build"), "{}", &page[..page.len().min(200)]);
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
        let c2 = Coordinator::start(&d.path().join("coord"), None, port, "m4", "").unwrap();
        assert_eq!(c2.contact.token, token);
        let w2 = client::Client::at(vec![format!("http://127.0.0.1:{port}")], token, "m1");
        assert!(w2.beat(g.lease, None).unwrap(), "the helper's lease lives on");
        assert!(!c2.renew(own, None), "this Mac's own ended with its agent");
        assert_eq!(w2.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/1", "k1")])), ..Default::default() }).unwrap(), client::Handed::Taken);
    }

    /// A request as a browser, a page or a proxy sends it, straight to `addr`: `headers` over the
    /// usual ones (Host x, JSON); the answer's status, where a redirect leads, and its JSON.
    fn send(addr: &str, method: &str, path: &str, key: Option<&str>, headers: &[(&str, &str)], body: &serde_json::Value) -> (u16, String, serde_json::Value) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        let b = body.to_string();
        let mut h: Vec<(String, String)> = [("Host", "x"), ("Content-Type", "application/json"), ("X-Worker", "a page")].iter().filter(|(k, _)| !headers.iter().any(|(o, _)| o.eq_ignore_ascii_case(k))).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        h.extend(headers.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        if let Some(k) = key {
            h.push(("Authorization".into(), format!("Bearer {k}")));
        }
        let head: String = h.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        write!(s, "{method} {path} HTTP/1.1\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len()).unwrap();
        let mut r = Vec::new();
        s.read_to_end(&mut r).unwrap();
        let text = String::from_utf8_lossy(&r).to_string();
        let code = text.split(' ').nth(1).unwrap().parse().unwrap();
        let location = text.lines().find_map(|l| l.strip_prefix("location: ").or_else(|| l.strip_prefix("Location: "))).unwrap_or("").trim().to_string();
        let json = text.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str(b.trim()).ok()).unwrap_or_default();
        (code, location, json)
    }

    #[test]
    fn the_build_page_reads_without_a_key_and_helping_or_pausing_needs_it() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let ask = |method: &str, path: &str, key: Option<&str>| -> (u16, String) {
            let (code, location, _) = send(&addr, method, path, key, &[], &serde_json::json!({}));
            (code, location)
        };
        // The page and what its dashboard reads: no key.
        assert_eq!(ask("GET", "/work/", None).0, 200);
        assert_eq!(ask("POST", "/work/swarm", None).0, 200);
        assert_eq!(ask("POST", "/work/history", None).0, 200);
        // The old watching-only address leads to it.
        assert_eq!(ask("GET", "/work/watch/", None), (302, "/work/".to_string()));
        // Helping and pausing: the key.
        for path in ["/work/ask", "/work/beat", "/work/pause", "/work/status"] {
            assert_eq!(ask("POST", path, None).0, 401, "{path}");
        }
        assert_eq!(ask("POST", "/work/pause", Some(&c.contact.token)).0, 200);
        // (A wrong key is no key.)
        assert_eq!(ask("POST", "/work/pause", Some("0123456789abcdef0123456789abcdef")).0, 401);
    }

    #[test]
    fn only_a_request_naming_this_mac_from_its_own_page_and_the_owners_and_jobs_from_this_mac_itself() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let post = |path: &str, key: Option<&str>, headers: &[(&str, &str)]| send(&addr, "POST", path, key, headers, &serde_json::json!({ "kind": "tail" })).0;
        // A name a web page elsewhere could point here (DNS rebinding), a page elsewhere, or a JSON
        // request sent as a form is: refused, with the key or not.
        assert_eq!(post("/work/swarm", None, &[("Host", "evil.example")]), 403);
        assert_eq!(post("/work/swarm", None, &[("Origin", "https://evil.example")]), 403);
        assert_eq!(post("/work/pause", Some(&c.contact.token), &[("Origin", "http://192.168.1.66:8000")]), 403);
        assert_eq!(post("/work/swarm", None, &[("Content-Type", "text/plain")]), 415);
        // The build page's own, at the address it asked for: straight, or through tailscale serve.
        assert_eq!(post("/work/swarm", None, &[("Host", "100.70.85.80:8090"), ("Origin", "http://100.70.85.80:8090")]), 200);
        assert_eq!(post("/work/swarm", None, &[("Host", "127.0.0.1:8090"), ("X-Forwarded-Host", "mac.tail1.ts.net"), ("X-Forwarded-For", "100.101.1.2"), ("Origin", "https://mac.tail1.ts.net")]), 200);
        // A job's and the owner's, from this Mac itself: not handed over by a proxy, whatever key.
        assert_eq!(post("/task/workers", Some(&c.job_token), &[]), 200);
        assert_eq!(post("/task/workers", Some(&c.job_token), &[("X-Forwarded-For", "100.101.1.2")]), 403);
        assert_eq!(post("/task/workers", Some(&c.contact.token), &[]), 401);
        assert_eq!(post("/work/devices", Some(&c.contact.token), &[]), 200);
        assert_eq!(post("/work/devices", Some(&c.contact.token), &[("Tailscale-User-Login", "someone@example.com")]), 403);
        assert_eq!(post("/work/devices", Some(&c.job_token), &[]), 401);
    }

    #[test]
    fn the_key_pages_carried_before_devices_asked_is_no_key() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("coord");
        std::fs::create_dir_all(&dir).unwrap();
        let old = "0123456789abcdef0123456789abcdef";
        std::fs::write(dir.join("token"), old).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = Coordinator::start(&dir, None, port, "m4", "").unwrap();
        assert!(c.contact.token != old && !dir.join("token").exists());
        let addr = format!("127.0.0.1:{port}");
        assert_eq!(send(&addr, "POST", "/work/pause", Some(old), &[], &serde_json::json!({})).0, 401);
        assert_eq!(send(&addr, "POST", "/work/pause", Some(&c.contact.token), &[], &serde_json::json!({})).0, 200);
    }

    #[test]
    fn a_device_asks_to_help_and_helps_as_itself_once_the_owner_accepts_its_code_here() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let token = c.contact.token.clone();
        // A JSON request as a page makes it: the answer's status and body.
        let page = |path: &str, key: Option<&str>, body: serde_json::Value| -> (u16, serde_json::Value) {
            let (code, _, v) = send(&addr, "POST", path, key, &[], &body);
            (code, v)
        };
        let secret = "00112233445566778899aabbccddeeff";
        let me = serde_json::json!({ "id": "ipad01", "label": "Safari on iPad", "secret": secret });
        // Asked, with no key: its code, made here.
        let (code, a) = page("/work/join", None, me.clone());
        assert_eq!((code, a["state"].as_str()), (200, Some("asking")));
        let shown = a["code"].as_str().unwrap().to_string();
        assert_eq!(shown.len(), 4);
        assert_eq!(page("/work/join/state", None, me.clone()).1["code"].as_str(), Some(shown.as_str()));
        // Not yet a helper; and the ask is the owner's to see alone: neither the build page's reads
        // nor its history tell of it.
        assert_eq!(page("/work/pause", Some(secret), serde_json::json!({})).0, 401);
        for path in ["/work/swarm", "/work/history"] {
            assert!(!page(path, None, serde_json::json!({})).1.to_string().contains("ipad01"), "{path}");
        }
        // The owner's, here, with the build's key (the menu bar, `scenic devices`): not a device's.
        let (code, v) = page("/work/devices", Some(&token), serde_json::json!({}));
        assert_eq!((code, v["asking"][0]["code"].as_str(), v["asking"][0]["label"].as_str()), (200, Some(shown.as_str()), Some("Safari on iPad")));
        let name = v["asking"][0]["ask"].as_str().unwrap().to_string();
        assert_eq!(page("/work/devices", Some(secret), serde_json::json!({})).0, 401);
        // An answer naming another ask, or with another code, answers nothing; with its own, it.
        let other = if shown == "0000" { "0001" } else { "0000" };
        assert_eq!(page("/work/devices/accept", Some(&token), serde_json::json!({ "ask": "0000000000000000", "code": shown })).0, 404);
        assert_eq!(page("/work/devices/accept", Some(&token), serde_json::json!({ "code": other })).0, 404);
        assert_eq!(page("/work/devices/accept", Some(&token), serde_json::json!({ "ask": name, "code": shown })).0, 200);
        assert_eq!(page("/work/devices", Some(secret), serde_json::json!({})).0, 401);
        // Its page learns so, and its secret is its key: for its own tasks, as itself, alone.
        assert_eq!(page("/work/join/state", None, me.clone()).1["state"], "accepted");
        assert_eq!(page("/work/join/state", None, serde_json::json!({ "id": "ipad01", "secret": "ffffffffffffffffffffffffffffffff" })).1["state"], "none");
        let asks = |worker: &str, kind: &str, can: &[&str]| serde_json::json!({ "worker": worker, "kind": kind, "can": can, "mem_mb": 1000, "cores": 4 });
        assert_eq!(page("/work/ask", Some(secret), asks("Safari on iPad ipad01", "web", &["tail"])).0 / 100, 2);
        assert_eq!(page("/work/ask", Some(secret), asks("m1", "web", &["tail"])).0, 403);
        assert_eq!(page("/work/ask", Some(secret), asks("Safari on iPad ipad01", "native", &["unit"])).0, 403);
        assert_eq!(page("/work/beat", Some(secret), serde_json::json!({ "worker": "m4", "lease": 1 })).0, 403);
        assert_eq!(page("/work/fail", Some(secret), serde_json::json!({ "worker": "m4", "lease": 1, "error": "x" })).0, 403);
        assert_eq!(page("/work/status", Some(secret), serde_json::json!({})).0, 403);
        assert_eq!(send(&addr, "GET", "/work/in/1/a", Some(secret), &[("X-Worker", "m1")], &serde_json::json!({})).0, 403);
        // Its pause: made now by this Mac's clock, by what it is, whatever it says.
        let forever = serde_json::json!({ "pause": { "mode": "drain", "by": "the owner", "at": u64::MAX }, "at": u64::MAX });
        assert_eq!(page("/work/pause", Some(secret), forever.clone()).0, 200);
        let p = c.shared.lock().unwrap().paused.clone().unwrap();
        assert!(p.by == "Safari on iPad" && p.at <= unix_now() && c.shared.lock().unwrap().pause_at <= unix_now());
        // (So the owner's resume goes through. An agent's time is its own, a minute ahead at most.)
        assert_eq!(page("/work/pause", Some(&token), serde_json::json!({ "pause": null })).0, 200);
        assert!(c.shared.lock().unwrap().paused.is_none());
        assert_eq!(page("/work/pause", Some(&token), forever).0, 200);
        assert!(c.shared.lock().unwrap().pause_at <= unix_now() + AHEAD_S);
        assert_eq!(page("/work/pause", Some(&token), serde_json::json!({ "pause": null, "at": unix_now() + AHEAD_S })).0, 200);
        assert!(c.shared.lock().unwrap().paused.is_none());
        // Another page with the device's id and another secret is another ask, the device kept;
        // withdrawn by its page.
        let theirs = serde_json::json!({ "id": "ipad01", "label": "Safari on iPad", "secret": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" });
        assert_eq!(page("/work/join", None, theirs.clone()).1["state"], "asking");
        assert_eq!(page("/work/join/cancel", None, theirs.clone()).1["cancelled"], true);
        assert_eq!(page("/work/join/state", None, theirs).1["state"], "none");
        assert_eq!(page("/work/join/state", None, me.clone()).1["state"], "accepted");
        // Declined: its page learns so.
        let mac = serde_json::json!({ "id": "mac01", "label": "Chrome on Mac", "secret": "cccccccccccccccccccccccccccccccc" });
        let mc = page("/work/join", None, mac.clone()).1["code"].as_str().unwrap().to_string();
        assert_eq!(page("/work/devices/decline", Some(&token), serde_json::json!({ "code": mc })).0, 200);
        assert_eq!(page("/work/join/state", None, mac).1["state"], "declined");
        // Forgotten: its secret no longer works.
        assert_eq!(page("/work/devices/forget", Some(&token), serde_json::json!({ "which": "ipad01" })).0, 200);
        assert_eq!(page("/work/pause", Some(secret), serde_json::json!({})).0, 401);
        // (The history tells what the owner did, and nothing of the asks.)
        let notes: Vec<String> = c.shared.lock().unwrap().history.since(0, 100).iter().filter(|e| e.kind == "device").map(|e| e.note.clone()).collect();
        assert_eq!(notes, ["accepted: it may help", "declined", "forgotten: it no longer helps"]);
    }

    #[test]
    fn a_task_reads_the_nas_where_it_lies() {
        let (d, c, w) = start();
        let (job_root, nas) = (d.path().join("job"), d.path().join("nas"));
        std::fs::create_dir_all(job_root.join("u")).unwrap();
        std::fs::write(job_root.join("u/in.bin"), b"input").unwrap();
        std::fs::create_dir_all(nas.join("sources/canopy")).unwrap();
        std::fs::create_dir_all(nas.join("state/build")).unwrap();
        let square: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(nas.join("sources/canopy/sq.tif"), &square).unwrap();
        std::fs::write(nas.join("state/build/manifest.json"), b"{}").unwrap();
        let job = client::Client::at(w.urls().to_vec(), c.job_token.clone(), "job");
        let offer = task::Offer { owner: 42, kind: "tail".into(), spec: serde_json::json!({ "unit": "6/1/1" }), root: job_root.clone(), inputs: [("u/in.bin".to_string(), 5)].into(), mem_mb: 100 };
        job.post_json("/task/offer", &serde_json::to_value(&offer).unwrap()).unwrap();
        let ipad = client::Client::at(w.urls().to_vec(), c.contact.token.clone(), "ipad");
        let g = ipad.ask(&Ask { worker: "ipad".into(), kind: "web".into(), can: vec!["tail".into()], mem_mb: 1000, cores: 4, ..Default::default() }).unwrap().unwrap();
        // (A request as the page makes it: its token, who it is, a range.)
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let get = |path: &str, who: &str, range: Option<&str>| -> (u16, Vec<u8>) {
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(&addr).unwrap();
            let range = range.map(|r| format!("Range: bytes={r}\r\n")).unwrap_or_default();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nX-Worker: {who}\r\n{range}Connection: close\r\n\r\n", c.contact.token).unwrap();
            let mut b = Vec::new();
            s.read_to_end(&mut b).unwrap();
            let end = b.windows(4).position(|x| x == b"\r\n\r\n").unwrap();
            let head = String::from_utf8_lossy(&b[..end]).to_string();
            let code: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
            let body = b[end + 4..].to_vec();
            // (Chunked or not: the tests' bodies are small, whole.)
            let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
                let t = String::from_utf8_lossy(&body).to_string();
                let (n, rest) = t.split_once("\r\n").unwrap();
                rest.as_bytes()[..usize::from_str_radix(n.trim(), 16).unwrap()].to_vec()
            } else {
                body
            };
            (code, body)
        };
        let base = format!("/work/net/{}", g.lease);
        // Before the agent says where the NAS is: not there.
        assert_eq!(get(&format!("{base}/nas/sources/canopy/sq.tif?probe"), "ipad", None).0, 503);
        c.set_root(Some(&nas));
        let json = |b: &[u8]| serde_json::from_slice::<serde_json::Value>(b).unwrap();
        let (code, b) = get(&format!("{base}/nas/sources/canopy/sq.tif?probe"), "ipad", None);
        assert_eq!((code, json(&b)), (200, serde_json::json!({ "kind": "file", "size": 3000 })));
        assert_eq!(json(&get(&format!("{base}/nas/sources/canopy?probe"), "ipad", None).1), serde_json::json!({ "kind": "dir" }));
        assert_eq!(json(&get(&format!("{base}/nas/sources/canopy?list"), "ipad", None).1)["entries"], serde_json::json!([["sq.tif", "file", 3000]]));
        // A folder that can't be read (as the NAS not answering): an error, not an empty listing.
        let locked = nas.join("sources/canopy/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        let (code, _) = get(&format!("{base}/nas/sources/canopy/locked?list"), "ipad", None);
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(code, 503);
        // A range of it, as a program reads it.
        let (code, b) = get(&format!("{base}/nas/sources/canopy/sq.tif"), "ipad", Some("1000-1099"));
        assert_eq!((code, b), (206, square[1000..1100].to_vec()));
        // Nothing else of the NAS, nothing that isn't there, nothing for another worker, no other server.
        assert_eq!(get(&format!("{base}/nas/state/build/manifest.json?probe"), "ipad", None).0, 403);
        assert_eq!(get(&format!("{base}/nas/sources/../state/build/manifest.json"), "ipad", None).0, 403);
        assert_eq!(get(&format!("{base}/nas/sources/canopy/none.tif?probe"), "ipad", None).0, 404);
        assert_eq!(get(&format!("{base}/nas/sources/canopy/sq.tif?probe"), "phone", None).0, 410);
        assert_eq!(get(&format!("{base}/web/example.com/x.tif?probe"), "ipad", None).0, 403);
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
        let id2 = job.post_json("/task/offer", &serde_json::to_value(&task::Offer { mem_mb: 100, ..offer.clone() }).unwrap()).unwrap().1["id"].as_u64().unwrap();
        assert!(ipad.ask(&web(1000)).unwrap().is_none());
        assert_eq!(job.post_json(&format!("/task/{id2}/withdraw"), &serde_json::json!({})).unwrap().1["withdrawn"], true);
        // What a worker measured for the unit replaces the guess, more or less: 300 MB and a tenth.
        let id3 = job.post_json("/task/offer", &serde_json::to_value(&task::Offer { mem_mb: 5000, ..offer }).unwrap()).unwrap().1["id"].as_u64().unwrap();
        let phone = client::Client::at(w.urls().to_vec(), c.contact.token.clone(), "phone");
        let g = phone.ask(&Ask { worker: "phone".into(), ..web(500) }).unwrap().unwrap();
        let Granted::Task { id: tid, mem_mb, .. } = g.work else { panic!() };
        assert_eq!((tid, mem_mb), (id3, 330));
    }
}
