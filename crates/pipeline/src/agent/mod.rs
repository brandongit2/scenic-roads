//! The build agent (docs/plan.md §8): `scenic agent`, a login item on the build Mac under the
//! launcher. It works out what needs doing, runs one job at a time when that job's conditions
//! hold (mains power, the NAS), pauses it when they lapse, and writes a heartbeat the app shows.
//!
//! Each loop (every 20 s, sooner when a job ends):
//! 1. conditions: power, the NAS (mounting it when missing), the user's activity, sleep;
//! 2. the running job: finished (recorded; failures retried with a growing delay), paused or
//!    resumed, restarted after sleep when it touches the NAS;
//! 3. otherwise the first runnable job of the plan: the OSM pass when the NAS holds a newer planet,
//!    then backups and cleanup once a day (later phases add layers, units and packs);
//! 4. the heartbeat: `state/status.json` on the NAS, and a copy in the agent's local folder.
//!
//! Nothing depends on the build Mac being available: until work is done, the map serves the last
//! catalog. A job is a child process (see `jobs`) that resumes from its own completion markers, so
//! stopping it at any time loses at most its current stage.

pub mod backup;
pub mod build;
pub mod claims;
pub mod cond;
pub mod gc;
pub mod jobs;
pub mod recipes;
pub mod room;

use anyhow::{Context, Result};
use cond::{Conditions, SleepWatch};
use jobs::{now_s, JobSpec, Needs, Running};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};


/// Free space the OSM pass needs on the build Mac to start: the filtered planet (about half the
/// planet's 95 GB) with room to spare. The planet is read from the NAS when there's no room to
/// copy it, the filtered file too once space runs short, and the pieces are cut a quarter at a
/// time, so the pass's peak stays near the filtered file's size.
pub const PASS_SPACE: u64 = 80 << 30;

/// Bytes of the files under `dir` (0 when it isn't there).
fn dir_bytes(dir: &Path) -> u64 {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => n += e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => {}
            }
        }
    }
    n
}

/// Where things are.
#[derive(Clone, Debug)]
pub struct Options {
    /// The NAS project folder; None: find the share (and mount it when missing).
    pub root: Option<PathBuf>,
    /// The agent's local folder (`~/Library/Application Support/scenic/agent`).
    pub home: PathBuf,
    /// The folder of the programs jobs run (`scenic-build`, `extract`): the agent's own.
    pub bin: PathBuf,
    /// Plan and report, start nothing.
    pub dry_run: bool,
    /// One loop, then exit (tests, `scenic agent --once`).
    pub once: bool,
    /// A helper on another Mac (`scenic agent --helper`, docs/plan.md §8, Two Macs): it plans
    /// nothing, but asks the build Mac's coordinator (crate::coord) for work that fits it, and
    /// reports to `state/helpers/<host>.json`, never the heartbeat.
    pub helper: bool,
}

/// The free space a helper's jobs start with (its Mac has less room than the build Mac).
const HELPER_RESERVE: u64 = 15 << 30;

/// The memory a helper spares its jobs (MB): a quarter of its Mac's (4 GB of the M1's 16, which its
/// units fit: over its first 205, its steps' programs took 3.7 GB at most).
fn helper_memory() -> u64 {
    let total = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok());
    total.map_or(4096, |b| (b >> 20) / 4)
}

/// The running job's lease.
#[derive(Clone, Debug)]
enum Held {
    /// The build Mac's own job's, in its coordinator.
    Own(u64),
    /// A helper's job's, from the build Mac's coordinator: the job saves into `dir` (its outbox),
    /// and the agent hands that back, with the job's record, as one.
    Leased { lease: u64, dir: PathBuf },
}

/// A worker as the heartbeat shows it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkerView {
    pub name: String,
    pub label: String,
    pub kind: String,
    pub what: String,
    pub mem_mb: u64,
    pub done: u32,
    pub failed: u32,
    pub bad: bool,
}

/// What a unit job's units cost (`SCENIC_COSTS`: a JSON line per unit).
fn read_costs(p: &Path) -> Vec<(String, crate::coord::Cost)> {
    let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
    s.lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            Some((v["unit"].as_str()?.to_string(), crate::coord::Cost { peak_mb: v["peak_mb"].as_u64()?, secs: v["secs"].as_u64().unwrap_or(0) }))
        })
        .collect()
}

/// The heartbeat (`state/status.json`), what the app's status bar and `scenic status` show.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Status {
    pub host: String,
    pub pid: u32,
    /// The app version the agent runs (its folder under app/), or "development".
    pub app: String,
    /// Seconds since the epoch.
    pub beat: u64,
    pub started: u64,
    pub conditions: Conditions,
    pub job: Option<JobView>,
    /// Work that can't run yet, and why, in plain words.
    pub waiting: Vec<Waiting>,
    /// The last jobs to finish, newest first.
    pub recent: Vec<Done>,
    /// Region recipes, and the ones that don't parse.
    pub regions: Vec<recipes::Recipe>,
    pub bad_recipes: Vec<(String, String)>,
    /// Per region, how many of its areas are built (after the first pass).
    #[serde(default)]
    pub built: BTreeMap<String, build::RegionState>,
    /// The build to the end, step by step: the pass's steps, then the regions'.
    #[serde(default)]
    pub checklist: Vec<build::Step>,
    /// The workers the coordinator heard from in the last two minutes (helpers, web pages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<WorkerView>,
    /// Helpers on other Macs building now (`state/helpers/<host>.json`, fresh within ten minutes):
    /// their own status, with their job.
    #[serde(default)]
    pub helpers: Vec<Status>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobView {
    pub id: String,
    pub what: String,
    pub started: u64,
    /// Why it's paused, when it is.
    pub paused: Option<String>,
    /// Its log's last lines.
    pub tail: String,
    /// How far it says it is (its log's last `progress:` line), with an estimate of the time left
    /// from its pace since it started saying so.
    #[serde(default)]
    pub progress: Option<JobProgress>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobProgress {
    pub done: f64,
    pub total: f64,
    pub unit: String,
    pub eta_s: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Waiting {
    pub what: String,
    pub why: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Done {
    pub id: String,
    pub what: String,
    pub ok: bool,
    pub ended: u64,
    pub secs: u64,
    /// The log's last lines when it failed.
    pub note: String,
}

/// What the agent remembers between runs (its local folder's `state.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Memory {
    /// Per job id: failures in a row and when it may run again.
    retry: BTreeMap<String, (u32, u64)>,
    /// The app that last saved this: a newer one tries failed jobs again at once (it may be the
    /// fix).
    #[serde(default)]
    app: String,
    /// When each daily job last succeeded (seconds since the epoch).
    last_ok: BTreeMap<String, u64>,
    recent: Vec<Done>,
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: i32) {
    STOP.store(true, Ordering::SeqCst);
}

/// Whether the agent has been asked to stop (SIGTERM, SIGINT): long work between loops (making
/// room) ends early for it.
pub(crate) fn stopping() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// The NAS project folder: the share's mount, by whatever name it's mounted. When it's missing and
/// `mount` is set, it's mounted: at home by the LAN name, away through Tailscale when the Keychain
/// has the bare name's password (else nothing: a dialog would ask for it).
pub fn find_root(mount: bool) -> Option<PathBuf> {
    use store::nas::{find_mount, HOST, PROJECT, SHARE};
    if let Some(m) = find_mount(HOST, SHARE) {
        return Some(m.point.join(PROJECT));
    }
    if mount {
        let url = if store::nas::at_home() { Some(store::nas::smb_url()) } else { store::nas::tunnel_url() };
        match url {
            Some(url) => {
                if let Err(e) = store::nas::mount(url, Duration::from_secs(60)) {
                    eprintln!("agent: mounting the NAS: {e:#}");
                }
            }
            None => eprintln!("agent: away from home, and the Keychain has no password for the NAS's Tailscale name ({HOST})"),
        }
        return find_mount(HOST, SHARE).map(|m| m.point.join(PROJECT));
    }
    None
}

/// The shares with a check still waiting on a hung mount: no second one starts meanwhile.
static CHECKING: std::sync::Mutex<std::collections::BTreeSet<PathBuf>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Whether `root` answers within a few seconds (a stat on a worker thread; a hung share counts as
/// away, and its thread is left to finish on its own, the only one for that share until it does).
fn answers(root: &Path) -> bool {
    if !CHECKING.lock().unwrap().insert(root.to_path_buf()) {
        return false;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let p = root.to_path_buf();
    std::thread::spawn(move || {
        let ok = std::fs::metadata(p.join("catalog")).is_ok();
        CHECKING.lock().unwrap().remove(&p);
        let _ = tx.send(ok);
    });
    rx.recv_timeout(Duration::from_secs(8)).unwrap_or(false)
}

/// The agent's lock (one agent per Mac): an exclusive flock on a local file, held while it runs.
pub struct AgentLock(#[allow(dead_code)] std::fs::File);

impl AgentLock {
    /// None when another agent holds it.
    pub fn try_take(home: &Path) -> Result<Option<AgentLock>> {
        std::fs::create_dir_all(home)?;
        let f = std::fs::File::options().create(true).truncate(false).write(true).open(home.join("agent.lock"))?;
        if !crate::sys::lock(&f, false)? {
            return Ok(None);
        }
        Ok(Some(AgentLock(f)))
    }
}

pub struct Agent {
    o: Options,
    /// Held unless another agent runs (then this one only plans and reports: a dry run).
    _lock: Option<AgentLock>,
    host: String,
    app: String,
    started: u64,
    mem: Memory,
    running: Option<Running>,
    sleep: SleepWatch,
    last_mount_try: Option<Instant>,
    /// The heartbeat last written to the NAS (without its time) and when: written again only when it
    /// changes or every five minutes, so an idle NAS can rest.
    last_beat: Option<(Vec<u8>, Instant)>,
    /// How far each region is built, and when that was worked out.
    progress: Option<(Instant, BTreeMap<String, build::RegionState>, Vec<build::Step>)>,
    /// The pass's reaches as last read, by content name (large: read again only when they change).
    reach: std::cell::RefCell<Option<(String, std::rc::Rc<crate::reach::Reaches>)>>,
    /// Who this agent is in claims ("<host> <pid>"), and when its running job's claims were last
    /// kept fresh.
    me: String,
    claims_fresh: Option<Instant>,
    /// The OSM pieces' sizes by content name (the coordinator sizes units by them; content-named
    /// files never change).
    piece_sizes: std::cell::RefCell<std::collections::HashMap<String, u64>>,
    /// Whether this Mac's earlier agent's claims were dropped (once the NAS answers), and when this
    /// Mac was last named the records' writer.
    claims_dropped: bool,
    writer_named: Option<Instant>,
    /// What the last plan found waiting, the regions it read and the helpers it saw, and when: while
    /// a job runs nothing new can start, so the plan (the manifest, the keys, a dozen NAS listings)
    /// is made again only every five minutes, for the status, and when the job ends.
    planned: Option<Planned>,
    /// When the workers' hand-offs were last merged (each loop when idle; every two minutes while a
    /// job runs).
    merged: Option<Instant>,
    /// The build Mac's coordinator (crate::coord), and when its contact on the NAS was last checked.
    coord: Option<crate::coord::Coordinator>,
    published: Option<Instant>,
    /// A helper's way to the build Mac's coordinator (made once its contact is on the NAS).
    client: Option<crate::coord::client::Client>,
    /// The running job's lease, and when it last beat.
    lease: Option<Held>,
    beaten: Option<Instant>,
}

/// The last plan's view, kept for the heartbeat between plans.
struct Planned {
    at: Instant,
    waiting: Vec<Waiting>,
    regions: Vec<recipes::Recipe>,
    bad: Vec<(String, String)>,
    helpers: Vec<Status>,
}

impl Agent {
    pub fn new(mut o: Options) -> Result<Agent> {
        std::fs::create_dir_all(&o.home).with_context(|| format!("create {}", o.home.display()))?;
        let lock = AgentLock::try_take(&o.home)?;
        if lock.is_none() {
            anyhow::ensure!(o.dry_run, "another agent is running on this Mac (its lock is {})", o.home.join("agent.lock").display());
            eprintln!("agent: another agent is running; planning only");
        }
        if lock.is_none() {
            o.dry_run = true;
        }
        let mut mem: Memory = std::fs::read(o.home.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let app = app_version(&o.bin);
        if mem.app != app {
            if !mem.retry.is_empty() {
                eprintln!("agent: a new app ({app}); failed jobs may run again at once");
            }
            mem.retry.clear();
            mem.app = app.clone();
        }
        let me = format!("{} {}", cond::host_name(), std::process::id());
        // The build Mac's agent coordinates (not a helper's, nor a dry run's).
        let coord = if !o.helper && lock.is_some() && !o.dry_run && o.root.is_none() {
            match crate::coord::Coordinator::start(&o.home.join("coord"), Some(o.bin.join("wasm")), crate::coord::PORT, &cond::host_name()) {
                Ok(c) => {
                    eprintln!("agent: coordinating at {}", c.contact.urls.join(", "));
                    Some(c)
                }
                Err(e) => {
                    eprintln!("agent: no coordinator ({e:#}): this Mac builds alone");
                    None
                }
            }
        } else {
            None
        };
        Ok(Agent { host: cond::host_name(), app, started: now_s(), mem, running: None, sleep: SleepWatch::default(), last_mount_try: None, last_beat: None, progress: None, reach: Default::default(), _lock: lock, o, me, claims_fresh: None, piece_sizes: Default::default(), claims_dropped: false, writer_named: None, planned: None, merged: None, coord, published: None, client: None, lease: None, beaten: None })
    }

    /// The keys to plan with: on the NAS, with the done records of the hand-offs waiting to be merged
    /// on top (a worker's job built and handed back, not yet merged: not built again).
    fn planning_keys(&self, root: &Path) -> Result<build::Keys> {
        build::Keys::load_with(root, &self.handoff_bases(root))
    }

    /// Where hand-offs wait to be merged: the coordinator's journal on this Mac, and the NAS's folder
    /// (a helper on an app from before the coordinator).
    fn handoff_bases(&self, root: &Path) -> Vec<PathBuf> {
        vec![crate::handoff::nas_base(root), self.o.home.join("coord/journal")]
    }

    /// Where a helper's leased jobs save (`<outbox>/<lease>/`), and its agent puts their result, until
    /// the coordinator has them.
    fn outbox(&self) -> PathBuf {
        self.o.home.join("outbox")
    }

    /// A helper's way to the build Mac's coordinator; None (with why, in `waiting`) when there's none.
    fn client(&mut self, root: &Path, waiting: &mut Vec<Waiting>) -> Option<&crate::coord::client::Client> {
        if self.client.is_none() {
            match crate::coord::client::Client::from_nas(root, &self.host) {
                Ok(Some(c)) => self.client = Some(c),
                Ok(None) => waiting.push(Waiting { what: "Building".into(), why: "the build Mac isn't coordinating (its agent isn't running, or is on an older app)".into() }),
                Err(e) => waiting.push(Waiting { what: "Building".into(), why: format!("{e:#}") }),
            }
        }
        self.client.as_ref()
    }

    /// Hands a helper's finished jobs back: each leased job's saves (merged into one hand-off, in the
    /// order written) with its done record and what its units cost, or its failure. A folder stays
    /// until the coordinator has it (or says its lease is gone: then its work was offered again,
    /// and it's dropped), across restarts; one left by a job that died with an earlier agent counts
    /// as failed.
    fn send_outbox(&mut self, root: &Path, waiting: &mut Vec<Waiting>) {
        let Ok(rd) = std::fs::read_dir(self.outbox()) else { return };
        let running = match &self.lease {
            Some(Held::Leased { dir, .. }) => Some(dir.clone()),
            _ => None,
        };
        let dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && Some(p) != running.as_ref()).collect();
        if dirs.is_empty() {
            return;
        }
        let Some(client) = self.client(root, waiting) else { return };
        for d in dirs {
            let Some(lease) = d.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok()) else { continue };
            let result: Option<serde_json::Value> = std::fs::read(d.join("result.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
            let sent = (|| -> Result<bool> {
                // A task's: its outputs and what it took.
                if let Some(t) = result.as_ref().filter(|r| r["ok"].as_bool() == Some(true)).map(|r| &r["task"]).filter(|t| t.is_object()) {
                    let d = crate::coord::Done { lease, outputs: serde_json::from_value(t["outputs"].clone())?, removed: serde_json::from_value(t["removed"].clone())?, secs: t["secs"].as_f64().unwrap_or(0.0), peak_mb: t["peak_mb"].as_u64().unwrap_or(0), ..Default::default() };
                    return client.done(&d);
                }
                // (A unit job's has its done record; one without, a task's cut short, went wrong.)
                let saves = match result.as_ref().filter(|r| r["ok"].as_bool() == Some(true) && !r["done"].is_null()) {
                    Some(_) => crate::handoff::written_in(&d)?,
                    None => None,
                };
                match (result.as_ref(), saves) {
                    (Some(r), Some(saves)) => {
                        let mut h = crate::handoff::Handoff::default();
                        for x in saves {
                            h.absorb(x);
                        }
                        h.done = serde_json::from_value(r["done"].clone())?;
                        let costs = read_costs(&d.join("costs.jsonl"));
                        client.done(&crate::coord::Done { lease, handoff: Some(h), costs, ..Default::default() })
                    }
                    _ => {
                        let why = match &result {
                            Some(r) if r["ok"].as_bool() == Some(true) && r["done"].is_null() => "it ended without a result".to_string(),
                            Some(r) if r["ok"].as_bool() == Some(true) => "one of its saves is damaged".to_string(),
                            Some(r) => r["error"].as_str().unwrap_or("it failed").to_string(),
                            None => "the helper's agent stopped while it ran".to_string(),
                        };
                        client.fail(lease, &why, None).map(|()| true)
                    }
                }
            })();
            match sent {
                Ok(taken) => {
                    if !taken {
                        eprintln!("agent: the coordinator no longer holds lease {lease}: its work is dropped (it was offered again)");
                    }
                    std::fs::remove_dir_all(&d).ok();
                }
                Err(e) => {
                    waiting.push(Waiting { what: "Handing work back".into(), why: format!("{e:#}; trying again") });
                    break;
                }
            }
        }
    }

    /// A helper's next job: units the build Mac's coordinator leases it (as many as a batch, none
    /// needing more memory than it spares), built from the pass it says, saving into the lease's
    /// outbox folder. Asked only when it could start now.
    fn helper_job(&mut self, root: &Path, c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let needs = Needs { ac: true, nas: true, home: false };
        if let Some(why) = lapsed(&needs, c) {
            waiting.push(Waiting { what: "Building".into(), why });
            return Vec::new();
        }
        let ask = crate::coord::Ask { kind: "native".into(), label: Some(format!("{} (helper)", self.host)), can: vec!["unit".into(), "tail".into()], mem_mb: helper_memory(), cores: std::thread::available_parallelism().map_or(4, |n| n.get() as u32), max: batch_size("unit"), ..Default::default() };
        let Some(client) = self.client(root, waiting) else { return Vec::new() };
        let asked = client.ask(&ask);
        let fail = |a: &Self, lease: u64, why: &str| {
            if let Some(c) = &a.client {
                c.fail(lease, why, None).ok();
            }
        };
        match asked {
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Job { step, targets, pass }, .. })) if step == "unit" => {
                let dir = self.outbox().join(lease.to_string());
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    waiting.push(Waiting { what: "Building".into(), why: format!("{e}") });
                    fail(self, lease, &format!("its outbox: {e}"));
                    return Vec::new();
                }
                let s = |p: &Path| p.to_string_lossy().into_owned();
                let cache = self.o.home.join("cache");
                let mut cmd = vec!["/usr/bin/env".to_string(), format!("SCENIC_HANDOFF={}", dir.display()), format!("SCENIC_COSTS={}", dir.join("costs.jsonl").display())];
                cmd.extend([s(&self.o.bin.join("scenic-build")), "unit".into(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch/unit"))]);
                cmd.extend(targets.iter().map(|t| t.0.clone()));
                cmd.extend(["--pass".into(), pass, "--dem".into(), s(&self.o.bin.join("dem")), "--cache-dir".into(), s(&cache)]);
                let n = targets.len();
                let id = format!("unit {}", targets.first().map(|t| t.0.as_str()).unwrap_or(""));
                let what = format!("Roads, elevations and scenery ({n} area{} for the build Mac)", if n == 1 { "" } else { "s" });
                self.lease = Some(Held::Leased { lease, dir });
                vec![JobSpec { id, what, cmd, needs, restart_after_sleep: true, record: Some(build::Work { step, targets }) }]
            }
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Task { id, task, .. }, .. })) => {
                // A task (a unit's last steps for the build Mac's job), run here by `scenic run-task`
                // over its files fetched from the coordinator; it needs no NAS.
                let dir = self.outbox().join(lease.to_string());
                let spec = dir.join("spec.json");
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&spec, task.to_string())) {
                    waiting.push(Waiting { what: "Building".into(), why: format!("{e}") });
                    fail(self, lease, &format!("its outbox: {e}"));
                    return Vec::new();
                }
                let s = |p: &Path| p.to_string_lossy().into_owned();
                let contact = self.client.as_ref().map(|c| (c.urls().join(","), c.token())).unwrap_or_default();
                let cmd = vec![
                    "/usr/bin/env".to_string(),
                    format!("SCENIC_COORD_URLS={}", contact.0),
                    format!("SCENIC_COORD_TOKEN={}", contact.1),
                    format!("SCENIC_WORKER={}", self.host),
                    s(&self.o.bin.join("scenic")),
                    "run-task".into(),
                    "--spec".into(),
                    s(&spec),
                    "--lease".into(),
                    lease.to_string(),
                    "--dir".into(),
                    s(&self.o.home.join("scratch/task")),
                    "--result".into(),
                    s(&dir.join("task.json")),
                ];
                let unit = task["unit"].as_str().unwrap_or("").to_string();
                self.lease = Some(Held::Leased { lease, dir });
                vec![JobSpec { id: format!("task {id}"), what: format!("Scenery for the build Mac's area {unit}"), cmd, needs: Needs { ac: true, nas: false, home: false }, restart_after_sleep: false, record: None }]
            }
            Ok(Some(g)) => {
                fail(self, g.lease, "this helper can't do that work");
                Vec::new()
            }
            Ok(None) => {
                waiting.push(Waiting { what: "Building".into(), why: "the build Mac has nothing for this Mac now".into() });
                Vec::new()
            }
            Err(e) => {
                waiting.push(Waiting { what: "Building".into(), why: format!("the build Mac can't be reached: {e:#}") });
                Vec::new()
            }
        }
    }

    /// The running job ended (or stopped): its lease ended with it. This Mac's own: done when it
    /// recorded its targets, and what its units cost learned; a helper's: its result in its outbox
    /// folder, to send.
    fn end_lease(&mut self, ok: bool, note: &str) {
        let pid = self.running.as_ref().map(|r| r.pgid as u32);
        match self.lease.take() {
            Some(Held::Own(id)) => {
                if let Some(c) = &self.coord {
                    c.finish(id, ok);
                    if let Some(p) = pid {
                        c.close_tasks(p);
                    }
                    let costs = self.o.home.join("costs.jsonl");
                    c.add_costs(&read_costs(&costs));
                    std::fs::remove_file(costs).ok();
                }
            }
            Some(Held::Leased { dir, .. }) => {
                let done = self.running.as_ref().and_then(|r| r.spec.record.clone()).filter(|_| ok).map(|w| (w.step, w.targets));
                // A task's: what `scenic run-task` wrote (its outputs are with the coordinator already).
                let task: Option<serde_json::Value> = std::fs::read(dir.join("task.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let ok = ok && task.as_ref().is_none_or(|t| t["ok"] == true);
                let r = serde_json::json!({ "ok": ok, "done": done, "task": task.and_then(|t| t.get("task").cloned()), "error": note.chars().take(3000).collect::<String>() });
                if let Err(e) = crate::whole::write(&dir.join("result.json"), r.to_string().as_bytes()) {
                    eprintln!("agent: writing a job's result for the coordinator: {e:#}");
                }
            }
            None => {}
        }
    }

    /// Beats for the running job's lease (each minute it isn't paused); false when it's lost: it
    /// lapsed and its work went to another worker. (This Mac's own lapses when its job paused past
    /// it, or this loop was stuck: it's held again when no one took its work meanwhile.)
    fn beat(&mut self, root: Option<&Path>) -> bool {
        self.beaten = Some(Instant::now());
        let progress = self.running.as_ref().and_then(|r| jobs::progress(&r.log)).map(|(d, t, u)| format!("{d:.0}/{t:.0} {u}"));
        match self.lease.clone() {
            Some(Held::Own(id)) => {
                let Some(c) = &self.coord else { return true };
                if c.renew(id, progress) {
                    return true;
                }
                let work = self.running.as_ref().and_then(|r| r.spec.record.clone());
                match work.and_then(|w| c.hold(&w.step, &w.targets)) {
                    Some(id) => {
                        self.lease = Some(Held::Own(id));
                        true
                    }
                    None => false,
                }
            }
            Some(Held::Leased { lease, .. }) => {
                let Some(r) = root else { return true };
                // (The coordinator away: the job goes on, and is handed back once it's there.)
                !matches!(self.client(r, &mut Vec::new()).map(|c| c.beat(lease, progress.as_deref())), Some(Ok(false)))
            }
            None => true,
        }
    }

    /// Drops the running job's claims (crate::agent::claims), as it ends.
    fn release_claims(&mut self, root: Option<&Path>) {
        // (A helper's leased job holds no claim files.)
        if self.o.helper {
            self.claims_fresh = None;
            return;
        }
        if let (Some((step, ts)), Some(r)) = (self.running.as_ref().and_then(|j| shared_targets(&j.spec)), root) {
            claims::release(r, &step, &ts, &self.me);
        }
        self.claims_fresh = None;
    }

    /// Keeps the running job's claims fresh (every two minutes: a claim lasts `claims::STALE`), not
    /// while it's paused: a paused job's claims go stale, so the other Mac may take them. False when
    /// another agent holds one of them now (the job is then stopped, unrecorded: that one builds it).
    fn keep_claims(&mut self, root: Option<&Path>) -> bool {
        // (A helper's leased job holds no claim files: its lease is kept by beats.)
        if self.o.helper {
            return true;
        }
        let (Some(j), Some(r)) = (self.running.as_ref(), root) else { return true };
        let Some((step, ts)) = shared_targets(&j.spec) else { return true };
        if claims::lost(r, &step, &ts, &self.me) {
            return false;
        }
        if j.paused.is_none() && self.claims_fresh.is_none_or(|t| t.elapsed() >= Duration::from_secs(120)) {
            claims::refresh(r, &step, &ts, &self.me);
            self.claims_fresh = Some(Instant::now());
        }
        true
    }

    fn record_path(&self) -> PathBuf {
        self.o.home.join("job.json")
    }

    fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(&self.mem) {
            let tmp = self.o.home.join("state.json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                std::fs::rename(&tmp, self.o.home.join("state.json")).ok();
            }
        }
    }

    /// The NAS root, when mounted and answering; tries to mount it every five minutes otherwise.
    fn root(&mut self) -> Option<PathBuf> {
        let root = match &self.o.root {
            Some(r) => Some(r.clone()),
            None => {
                let try_mount = self.last_mount_try.is_none_or(|t| t.elapsed() > Duration::from_secs(300));
                let r = find_root(false).or_else(|| {
                    if try_mount {
                        self.last_mount_try = Some(Instant::now());
                        find_root(true)
                    } else {
                        None
                    }
                });
                r
            }
        };
        root.filter(|r| answers(r))
    }

    /// Runs until stopped (SIGTERM, SIGINT), or for one loop with `once`.
    pub fn run(&mut self) -> Result<()> {
        // (The handler only stores to an atomic.)
        crate::sys::on_terminate(on_signal);
        if self._lock.is_some() {
            jobs::stop_orphan(&self.record_path());
        }
        loop {
            let quick = self.step()?;
            if self.o.once || STOP.load(Ordering::SeqCst) {
                break;
            }
            // A newer app is in place and nothing runs: exit, and the launcher starts the new one.
            if self.running.is_none() && self.newer_app() {
                eprintln!("agent: a newer app is installed; restarting into it");
                break;
            }
            let wait = if quick { 2 } else { 20 };
            for _ in 0..wait {
                if STOP.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let root = self.o.root.clone().or_else(|| self.root());
        if let Some(r) = self.running.as_mut() {
            eprintln!("agent: stopping {}", r.spec.id);
            r.stop(Duration::from_secs(30));
            self.end_lease(false, "the agent stopped");
            let r = self.running.take().unwrap();
            std::fs::remove_file(self.record_path()).ok();
            // Its claims, free for the other Mac now rather than once stale.
            if let (Some((step, ts)), Some(root), false) = (shared_targets(&r.spec), &root, self.o.helper) {
                claims::release(root, &step, &ts, &self.me);
            }
        }
        // The coordinator's contact off the NAS: workers wait for the next agent.
        if let (Some(c), Some(root)) = (&self.coord, &root) {
            c.unpublish(root);
        }
        Ok(())
    }

    /// One loop; true when a job just ended (look again soon).
    fn step(&mut self) -> Result<bool> {
        let slept = self.sleep.slept();
        let home = store::nas::at_home();
        // Back home with the share mounted through Tailscale (mounted while away): unmounted while
        // nothing runs, and mounted again by the LAN name below, at the LAN's speed.
        if home && self.running.is_none() && self.o.root.is_none() {
            if let Some(m) = store::nas::find_mount(store::nas::HOST, store::nas::SHARE).filter(|m| !store::nas::by_lan_name(m)) {
                eprintln!("agent: home again; remounting the NAS by its LAN name (it was mounted as {})", m.from);
                if let Err(e) = store::nas::unmount(&m.point, Duration::from_secs(30)) {
                    eprintln!("agent: {e:#}");
                }
                self.last_mount_try = None;
            }
        }
        let root = self.root();
        let (ac, battery) = cond::power();
        let c = Conditions { ac, battery, nas: root.is_some(), home, idle_s: cond::idle_seconds() };
        let mut waiting: Vec<Waiting> = Vec::new();
        let mut ended = false;

        // The running job.
        if let Some(r) = self.running.as_mut() {
            if let Some(st) = r.poll()? {
                let secs = r.elapsed().as_secs();
                let ok = st.success();
                // The build Mac's agent records it; a helper's goes back to the coordinator (below).
                let mut recorded = false;
                if let (true, false, Some(w), Some(root)) = (ok, self.o.helper, r.spec.record.clone(), root.as_ref()) {
                    let rec = build::Keys::load_strict(root).and_then(|mut k| {
                        k.record(&w.step, &w.targets);
                        k.save(root)
                    });
                    match rec {
                        Ok(()) => recorded = true,
                        Err(e) => eprintln!("agent: recording {}: {e:#}", r.spec.id),
                    }
                }
                let note = if ok { String::new() } else { format!("{st}\n{}", jobs::tail(&r.log, 20)) };
                let (id, what) = (r.spec.id.clone(), r.spec.what.clone());
                eprintln!("agent: {id} {} after {secs} s", if ok { "finished" } else { "failed" });
                self.end_lease(ok && (recorded || self.o.helper), &note);
                self.finished(&id, &what, ok, secs, note);
                self.release_claims(root.as_deref());
                self.running = None;
                std::fs::remove_file(self.record_path()).ok();
                ended = true;
            } else if slept > 30 && r.spec.restart_after_sleep && r.spec.needs.nas {
                // Open SMB handles often don't survive sleep: stop it; the plan below starts it again
                // from its completion markers once its conditions hold.
                eprintln!("agent: slept {slept} s; restarting {}", r.spec.id);
                r.stop(Duration::from_secs(30));
                self.end_lease(false, "stopped: the Mac slept");
                self.release_claims(root.as_deref());
                self.running = None;
                std::fs::remove_file(self.record_path()).ok();
            } else if self.running.as_ref().unwrap().paused.is_none() && self.lease.is_some() && self.beaten.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) && !self.beat(root.as_deref()) {
                // (A paused job doesn't beat: its lease lapses, and its work may go to another.)
                let r = self.running.as_mut().unwrap();
                eprintln!("agent: {}'s lease lapsed and its work went to another worker; stopping it", r.spec.id);
                r.stop(Duration::from_secs(30));
                self.end_lease(false, "its lease lapsed");
                self.release_claims(root.as_deref());
                self.running = None;
                std::fs::remove_file(self.record_path()).ok();
                ended = true;
            } else if !self.keep_claims(root.as_deref()) {
                // Another agent took its claims (they went stale while it was paused, or this Mac
                // was away): it builds them; this one stops, unrecorded, and drops the rest.
                let r = self.running.as_mut().unwrap();
                eprintln!("agent: another Mac took {}'s areas; stopping it", r.spec.id);
                r.stop(Duration::from_secs(30));
                self.release_claims(root.as_deref());
                self.running = None;
                self.claims_fresh = None;
                std::fs::remove_file(self.record_path()).ok();
                ended = true;
            } else if let Some(why) = lapsed(&self.running.as_ref().unwrap().spec.needs, &c) {
                let r = self.running.as_mut().unwrap();
                if r.paused.is_none() {
                    eprintln!("agent: pausing {}: {why}", r.spec.id);
                }
                r.pause(&why);
            } else if self.running.as_ref().unwrap().paused.is_some() {
                let r = self.running.as_mut().unwrap();
                eprintln!("agent: resuming {}", r.spec.id);
                r.resume();
            }
        }

        // Once the NAS answers: this Mac's earlier agent's claims dropped (it stopped or crashed: free
        // for the other Mac), and every five minutes the build Mac named the records' one writer,
        // by its name now (crate::out::Out::save).
        if let (Some(r), true) = (root.as_ref(), self._lock.is_some() && !self.o.dry_run) {
            if !self.claims_dropped {
                claims::release_host(r, &self.host, &self.me);
                self.claims_dropped = true;
            }
            if !self.o.helper && self.writer_named.is_none_or(|t| t.elapsed() >= Duration::from_secs(300)) {
                let name = cond::host_name();
                let writer = r.join("state/build/writer");
                if std::fs::read_to_string(&writer).ok().as_deref().map(str::trim) != Some(name.as_str()) {
                    if let Err(e) = crate::whole::write(&writer, name.as_bytes()) {
                        eprintln!("agent: naming this Mac the build's writer: {e:#}");
                    }
                }
                self.writer_named = Some(Instant::now());
            }
        }

        // The coordinator: how to reach it on the NAS (looked at every five minutes: it may have been
        // deleted, or this Mac's addresses changed), and leases whose workers went quiet dropped.
        if let (Some(c), Some(r)) = (&self.coord, &root) {
            if self.published.is_none_or(|t| t.elapsed() >= Duration::from_secs(300)) {
                match c.publish(r) {
                    Ok(()) => self.published = Some(Instant::now()),
                    Err(e) => eprintln!("agent: publishing the coordinator's contact: {e:#}"),
                }
            }
            for l in c.expire() {
                eprintln!("agent: {}'s lease of {} lapsed (no word for {} min); offered again", l.worker, l.what(), crate::coord::TTL.as_secs() / 60);
            }
        }
        // A helper's finished jobs not yet with the coordinator: sent before anything new.
        if let (true, Some(r)) = (self.o.helper, root.clone()) {
            self.send_outbox(&r, &mut waiting);
        }

        // A helper's hand-offs, merged into the build's records before planning (the build Mac
        // alone writes them): each loop when nothing runs, every two minutes while a job does.
        let idle = self.running.is_none();
        let merge_due = idle || ended || self.merged.is_none_or(|t| t.elapsed() >= Duration::from_secs(120));
        if let (Some(r), false, true, true) = (root.as_ref(), self.o.helper, self._lock.is_some() && !self.o.dry_run, merge_due) {
            match crate::handoff::merge_from(r, &self.o.home.join("scratch/handoff"), &self.handoff_bases(r)) {
                Ok(0) => {}
                Ok(n) => eprintln!("agent: merged {n} hand-off{} from other workers", if n == 1 { "" } else { "s" }),
                Err(e) => eprintln!("agent: merging other workers' hand-offs: {e:#}"),
            }
            self.merged = Some(Instant::now());
        }

        // The plan: start the first job that can run. Made when one could start, when a job ends,
        // and otherwise every five minutes for the heartbeat (between, its last view is shown).
        let plan_due = idle || ended || self.planned.as_ref().is_none_or(|p| p.at.elapsed() >= Duration::from_secs(300));
        let plan = match &root {
            Some(root) if self.o.helper => {
                // A helper plans nothing: it asks the build Mac for work, once its last is handed back.
                let unsent = std::fs::read_dir(self.outbox()).map(|mut d| d.next().is_some()).unwrap_or(false);
                let plan = if idle && !unsent { self.helper_job(root, &c, &mut waiting) } else { Vec::new() };
                let regions = match &self.planned {
                    Some(p) if !plan_due => p.regions.clone(),
                    _ => recipes::load(&root.join("inputs/regions")).0,
                };
                self.planned = Some(Planned { at: Instant::now(), waiting: waiting.clone(), regions, bad: Vec::new(), helpers: Vec::new() });
                plan
            }
            Some(root) if plan_due => {
                let plan = self.plan(root, &c, &mut waiting);
                let (regions, bad) = recipes::load(&root.join("inputs/regions"));
                self.planned = Some(Planned { at: Instant::now(), waiting: waiting.clone(), regions, bad, helpers: helpers(root) });
                plan
            }
            Some(_) => {
                waiting.extend(self.planned.as_ref().map(|p| p.waiting.clone()).unwrap_or_default());
                Vec::new()
            }
            None => {
                waiting.push(Waiting { what: "All building".into(), why: "the NAS isn't reachable (away from home, or it's off)".into() });
                Vec::new()
            }
        };
        // A newer app installed: nothing new starts, so the loop exits between jobs and the launcher
        // starts the new one (with work queued back to back, it would otherwise never get a turn).
        let newer = self.running.is_none() && self.newer_app();
        if newer {
            waiting.push(Waiting { what: "Building".into(), why: "restarting into the newly installed app".into() });
        }
        if self.running.is_none() && !newer {
            for spec in plan {
                if let Some(why) = lapsed(&spec.needs, &c) {
                    waiting.push(Waiting { what: spec.what.clone(), why });
                    continue;
                }
                if let Some(&(n, until)) = self.mem.retry.get(&spec.id) {
                    if now_s() < until {
                        waiting.push(Waiting { what: spec.what.clone(), why: format!("failed {n} time{} in a row; trying again in {} min", if n == 1 { "" } else { "s" }, (until - now_s()).div_ceil(60)) });
                        continue;
                    }
                }
                if self.o.dry_run {
                    waiting.push(Waiting { what: spec.what.clone(), why: "would start now (dry run)".into() });
                    break;
                }
                let (id, what) = (spec.id.clone(), spec.what.clone());
                // Room on the disk for it, from the caches that are cheap to fill again (the OSM
                // pass's own need, less the pack cache it clears; a helper's Mac has less room).
                // Made before its targets are claimed: it can take minutes, and a claim is
                // refreshed only while a job runs.
                let cache = self.o.home.join("cache");
                let need = if self.o.helper {
                    HELPER_RESERVE
                } else if id.starts_with("osm-pass") {
                    PASS_SPACE.saturating_sub(dir_bytes(&cache.join("base"))).max(room::RESERVE)
                } else {
                    room::RESERVE
                };
                // (Never without the NAS: what goes here must be kept there.)
                // (The OSM pass without the margin: its need is what its conditions admitted it with.)
                let margin = if id.starts_with("osm-pass") { 0 } else { room::margin(need) };
                if let Some(r) = &root {
                    match room::make_room(&cache, &r.join("sources"), need, margin) {
                        Ok(0) => {}
                        Ok(n) => eprintln!("agent: {} GB of cached canopy squares and terrain tiles deleted for {} GB free", n >> 30, (need + margin) >> 30),
                        Err(e) => eprintln!("agent: making room on the disk: {e:#}"),
                    }
                }
                // A job other workers may also do: its targets held first, by a lease in this Mac's
                // coordinator (and claim files, for a helper on an app from before it); when another
                // holds one, none starts (the next loop plans without it). A helper's job came
                // with its lease.
                if let (Some((step, ts)), Some(r), false) = (shared_targets(&spec), &root, self.o.helper) {
                    let targets = spec.record.as_ref().map(|w| w.targets.clone()).unwrap_or_default();
                    let lease = match &self.coord {
                        Some(c) => match c.hold(&step, &targets) {
                            Some(id) => Some(id),
                            None => {
                                waiting.push(Waiting { what: what.clone(), why: "another worker took part of it; planning again".into() });
                                continue;
                            }
                        },
                        None => None,
                    };
                    let let_go = |a: &Self| {
                        if let (Some(c), Some(id)) = (&a.coord, lease) {
                            c.finish(id, false);
                        }
                    };
                    if !claims::claim(r, &step, &ts, &self.me) {
                        let_go(self);
                        waiting.push(Waiting { what: what.clone(), why: "another Mac took part of it; planning again".into() });
                        continue;
                    }
                    // Recorded since this loop read the keys (another worker built it meanwhile):
                    // not built again.
                    let done = self.planning_keys(r).map(|keys| spec.record.as_ref().is_some_and(|w| w.targets.iter().any(|(t, k)| keys.recorded(&w.step, t) == Some(k.as_str()))));
                    if done.unwrap_or(true) {
                        let_go(self);
                        claims::release(r, &step, &ts, &self.me);
                        waiting.push(Waiting { what: what.clone(), why: "another worker built part of it meanwhile (or the keys can't be read now); planning again".into() });
                        continue;
                    }
                    self.lease = lease.map(Held::Own);
                    self.claims_fresh = Some(Instant::now());
                }
                let shared = shared_targets(&spec);
                if let Err(e) = self.start(spec, &c) {
                    // It couldn't even start (a missing program, a full disk): retried later.
                    eprintln!("agent: can't start {id}: {e:#}");
                    self.end_lease(false, &format!("couldn't start: {e:#}"));
                    if let (Some((step, ts)), Some(r), false) = (shared, &root, self.o.helper) {
                        claims::release(r, &step, &ts, &self.me);
                    }
                    self.finished(&id, &what, false, 0, format!("couldn't start: {e:#}"));
                    continue;
                }
                break;
            }
        }
        // A helper's lease for a job that didn't start (room on the disk, say): given back.
        if self.running.is_none() && self.lease.is_some() {
            self.end_lease(false, "it couldn't start on the helper");
        }

        // The heartbeat.
        let (regions, bad) = match (&root, &self.planned) {
            (Some(_), Some(p)) => (p.regions.clone(), p.bad.clone()),
            _ => Default::default(),
        };
        // (Recomputed after a job ends, or every five minutes: it reads the manifest and outlines.)
        if ended || self.progress.as_ref().is_none_or(|(t, _, _)| t.elapsed() >= Duration::from_secs(300)) {
            if let Some(r) = root.as_ref() {
                self.progress = Some((Instant::now(), self.region_progress(r, &regions), self.checklist(r, &regions)));
            }
        }
        let built = self.progress.as_ref().map(|(_, b, _)| b.clone()).unwrap_or_default();
        let checklist = self.progress.as_ref().map(|(_, _, c)| c.clone()).unwrap_or_default();
        // The running job's progress, and from its pace the time it has left.
        let job_progress = self.running.as_mut().and_then(|r| {
            let (done, total, unit) = jobs::progress(&r.log)?;
            let frac = done / total;
            let fresh = r.progress_base.as_ref().is_none_or(|b| b.2 != unit || frac < b.1);
            if fresh {
                r.progress_base = Some((Instant::now(), frac, unit.clone()));
            }
            let (t0, f0, _) = r.progress_base.as_ref().unwrap();
            let eta_s = (frac > *f0 && r.paused.is_none()).then(|| (t0.elapsed().as_secs_f64() * (1.0 - frac) / (frac - f0)) as u64);
            Some(JobProgress { done, total, unit, eta_s })
        });
        let status = Status {
            host: self.host.clone(),
            pid: std::process::id(),
            app: self.app.clone(),
            beat: now_s(),
            started: self.started,
            conditions: c,
            job: self.running.as_ref().map(|r| JobView { id: r.spec.id.clone(), what: r.spec.what.clone(), started: r.started, paused: r.paused.clone(), tail: jobs::tail(&r.log, 3), progress: job_progress.clone() }),
            waiting,
            recent: self.mem.recent.clone(),
            regions,
            bad_recipes: bad,
            built,
            checklist,
            helpers: if self.o.helper || root.is_none() { Vec::new() } else { self.planned.as_ref().map(|p| p.helpers.clone()).unwrap_or_default() },
            workers: self.coord.as_ref().map(|c| c.workers().into_iter().map(|(name, w)| WorkerView { name, label: w.label, kind: w.kind, what: w.what, mem_mb: w.mem_mb, done: w.done, failed: w.failed, bad: w.bad }).collect()).unwrap_or_default(),
        };
        let body = serde_json::to_vec_pretty(&status)?;
        if self._lock.is_none() {
            // Another agent writes the heartbeat; this one only reports.
            eprintln!("{}", String::from_utf8_lossy(&body));
            return Ok(ended);
        }
        // A helper's own status, never the heartbeat (the Macs' status bars show the main agent's,
        // and its helpers'), here and on the NAS.
        let (local, shared) = if self.o.helper { ("helper.json".to_string(), format!("state/helpers/{}.json", self.host)) } else { ("status.json".to_string(), "state/status.json".to_string()) };
        write_replace(&self.o.home.join(&local), &body).ok();
        if let Some(root) = &root {
            // What's new since the last write, without the time and the user's idle seconds (which
            // change every loop): only whether the user is at the Mac counts.
            let idle_s = if c.user_active() { 0 } else { cond::AWAY_S };
            let same = serde_json::to_vec(&Status { beat: 0, conditions: Conditions { idle_s, ..c }, ..status.clone() })?;
            let due = self.last_beat.as_ref().is_none_or(|(b, t)| *b != same || t.elapsed() >= Duration::from_secs(300));
            if due {
                std::fs::create_dir_all(root.join("state/helpers")).ok();
                match write_replace(&root.join(&shared), &body) {
                    Ok(()) => self.last_beat = Some((same, Instant::now())),
                    Err(e) => eprintln!("agent: heartbeat: {e:#}"),
                }
            }
        }
        Ok(ended)
    }

    fn start(&mut self, spec: JobSpec, c: &Conditions) -> Result<()> {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        // Half the cores while the user is active, all of them when away (a helper, two fewer:
        // its Mac has less memory, and its user's work comes first).
        let threads = match (c.user_active(), self.o.helper) {
            (true, _) => (cores / 2).max(1),
            (false, true) => cores.saturating_sub(2).max(1),
            (false, false) => cores,
        };
        let log = self.o.home.join("logs").join(format!("{}.log", spec.id.replace([' ', '/'], "-")));
        eprintln!("agent: starting {} ({threads} threads)", spec.id);
        // The build Mac's jobs save the records (crate::out::Out::save trusts them by this, whatever
        // this Mac is named now), note what their units cost, and may offer tasks to other workers
        // through the coordinator; a helper's hand their saves off (SCENIC_HANDOFF, in their command).
        let mut env: Vec<(String, String)> = Vec::new();
        if !self.o.helper {
            env.push(("SCENIC_BUILD_MAC".into(), "1".into()));
            env.push(("SCENIC_COSTS".into(), self.o.home.join("costs.jsonl").to_string_lossy().into_owned()));
            if let Some(c) = &self.coord {
                env.push(("SCENIC_COORD".into(), format!("http://127.0.0.1:{}", crate::coord::PORT)));
                env.push(("SCENIC_COORD_TOKEN".into(), c.job_token.clone()));
            }
        }
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.running = Some(Running::start(spec, threads, &env, log, &self.record_path())?);
        Ok(())
    }

    fn finished(&mut self, id: &str, what: &str, ok: bool, secs: u64, note: String) {
        if ok {
            self.mem.retry.remove(id);
            self.mem.last_ok.insert(id.to_string(), now_s());
        } else {
            let n = self.mem.retry.get(id).map(|r| r.0).unwrap_or(0) + 1;
            // 10 min, 20, 40 … up to 6 h.
            let delay = (600u64 << (n - 1).min(6)).min(6 * 3600);
            self.mem.retry.insert(id.to_string(), (n, now_s() + delay));
        }
        self.mem.recent.insert(0, Done { id: id.into(), what: what.into(), ok, ended: now_s(), secs, note });
        self.mem.recent.truncate(20);
        self.save();
    }

    /// Due when it last succeeded more than `every` ago (or never).
    fn due(&self, id: &str, every: Duration) -> bool {
        self.mem.last_ok.get(id).is_none_or(|&t| now_s().saturating_sub(t) >= every.as_secs())
    }

    /// The work there is, in order (docs/plan.md §8, Order).
    fn plan(&self, root: &Path, _c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let mut out = Vec::new();
        let build = self.o.bin.join("scenic-build");
        let me = self.o.bin.join("scenic");
        let s = |p: &Path| p.to_string_lossy().into_owned();

        // 1. The OSM pass, when the NAS holds a newer planet than the last complete pass.
        let have = crate::osmpass::latest_pass(root);
        if let Ok(Some((planet, date))) = crate::osmpass::newer_planet(root, have.as_deref()) {
            let what = format!("OpenStreetMap pass (planet of {date})");
            let scratch = self.o.home.join("scratch").join(format!("osm-{date}"));
            let jar = root.join("sources/basemap/planetiler.jar");
            // The pack cache is cleared when the pass starts (it refills from the mirror or the NAS):
            // its space counts as free, and so does the cheap caches' (room::make_room frees it).
            let pack_cache = self.o.home.join("cache").join("base");
            let mut free = cond::free_bytes(&self.o.home).unwrap_or(0) + dir_bytes(&pack_cache);
            if free < PASS_SPACE {
                free += room::cheap_bytes(&self.o.home.join("cache"));
            }
            let started = scratch.exists();
            if !jar.exists() {
                waiting.push(Waiting { what, why: "sources/basemap/planetiler.jar is missing on the NAS".into() });
            } else if !started && free < PASS_SPACE {
                waiting.push(Waiting { what, why: format!("needs {} GB free on this Mac ({} GB free)", PASS_SPACE >> 30, free >> 30) });
            } else {
                out.push(JobSpec {
                    id: format!("osm-pass {date}"),
                    what,
                    cmd: vec![
                        s(&build),
                        "osm-pass".into(),
                        "--root".into(),
                        s(root),
                        "--scratch".into(),
                        s(&scratch),
                        "--planet".into(),
                        s(&planet),
                        "--date".into(),
                        date,
                        "--extract".into(),
                        s(&self.o.bin.join("extract")),
                        "--planetiler".into(),
                        s(&jar),
                        "--clear".into(),
                        s(&pack_cache),
                    ],
                    // (It reads the whole planet: at home only.)
                    needs: Needs { ac: true, nas: true, home: true },
                    restart_after_sleep: true,
                    record: None,
                });
            }
        }

        // The regions: terrain and slope near the coverage, base(U), pack(T), lo, a catalog.
        out.extend(self.region_work(root, have.as_deref(), waiting));

        // Daily: the user's folders backed up, replaced files removed.
        if self.due("backup", Duration::from_secs(86400)) {
            out.push(JobSpec {
                id: "backup".into(),
                what: "Backing up translations, descriptions and inputs".into(),
                cmd: vec![s(&me), "backup".into(), "--root".into(), s(root), "--local".into(), s(&self.o.home.join("backups"))],
                needs: Needs { ac: false, nas: true, home: false },
                restart_after_sleep: true,
                record: None,
            });
        }
        if self.due("gc", Duration::from_secs(86400)) {
            out.push(JobSpec {
                id: "gc".into(),
                what: "Removing replaced files from the NAS".into(),
                cmd: vec![s(&me), "gc".into(), "--root".into(), s(root)],
                needs: Needs { ac: false, nas: true, home: false },
                restart_after_sleep: true,
                record: None,
            });
        }
        out
    }

    /// The build steps for the regions, as jobs (docs/plan.md §8), in order: the newest pass's
    /// worldwide jobs, each gated by its own inputs, then what `build::plan` finds stale (its
    /// chains), each step's targets in one run of `scenic-build`. The agent runs the first not
    /// waiting out a failure, so one failing job doesn't hold up the others.
    fn region_work(&self, root: &Path, pass: Option<&str>, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let (recipes, _) = recipes::load(&root.join("inputs/regions"));
        // (The records unreadable now: nothing planned until they are, rather than everything again.)
        let (manifest, keys): (BTreeMap<String, String>, build::Keys) = match crate::out::read_record(&root.join("state/build/manifest.json")).and_then(|m| Ok((m, self.planning_keys(root)?))) {
            Ok(r) => r,
            Err(e) => {
                waiting.push(Waiting { what: "Building".into(), why: format!("the build's records can't be read now: {e:#}") });
                return Vec::new();
            }
        };
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let build_bin = s(&self.o.bin.join("scenic-build"));
        let mut jobs: Vec<JobSpec> = Vec::new();
        let job = |id: String, what: &str, step: &str, extra: Vec<String>, record: Option<build::Work>| {
            let scratch = self.o.home.join("scratch").join(step);
            let mut cmd = vec![build_bin.clone(), step.to_string(), "--root".into(), s(root), "--scratch".into(), s(&scratch)];
            cmd.extend(extra);
            // The pass's whole-planet reads (its missing sets, the units' reach), and the world's
            // buildings (tens of GB onto the NAS), wait for home.
            let home = matches!(step, "pass-sets" | "reach" | "buildings");
            JobSpec { id, what: what.into(), cmd, needs: Needs { ac: true, nas: true, home }, restart_after_sleep: true, record }
        };
        // Per pass, worldwide: the sets it lacks in their current filters (a set added or changed
        // since it ran), the hiking routes' ends, AWS's z8 (once), Overture's buildings (once per
        // release), the summits, the labels.
        if let Some(date) = pass {
            let p = vec!["--pass".to_string(), date.to_string()];
            if !crate::osmpass::SETS.iter().all(|st| manifest.contains_key(&crate::osmpass::set_name(date, st.0))) {
                jobs.push(job(format!("pass-sets {date}"), "OpenStreetMap sets the newest pass lacks", "pass-sets", p.clone(), None));
            }
            if let Some(w) = build::trailends_work(date, &manifest, &keys) {
                jobs.push(job(format!("trailends {date}"), "Hiking routes' ends for the whole world", "trailends", p.clone(), Some(w)));
            }
            let reach_job = |w: build::Work| job(format!("reach {date}"), "How far each area's roads reach, for the whole world", "reach", p.clone(), Some(w));
            if let Some(w) = build::reach_work(date, &manifest, &keys) {
                jobs.push(reach_job(w));
            } else if let Err(why) = self.current_reach(root, &manifest, &keys, date) {
                match why {
                    // Made, but it doesn't decode: made again.
                    crate::reach::LoadError::Bad(e) => {
                        waiting.push(Waiting { what: "Building the areas".into(), why: format!("the pass's reaches don't read ({e}); making them again") });
                        if let Some(k) = build::reach_key(date, &manifest) {
                            jobs.push(reach_job(build::Work { step: "reach".into(), targets: vec![("reach".into(), k)] }));
                        }
                    }
                    crate::reach::LoadError::Io(e) => waiting.push(Waiting { what: "Building the areas".into(), why: format!("the pass's reaches can't be read now: {e}") }),
                }
            }
            if !manifest.contains_key(&crate::terrain_z8::logical()) {
                jobs.push(job("terrain-z8".into(), "Coarse terrain for the whole world", "terrain-z8", vec!["--raw".into(), s(&self.o.home.join("cache").join("aws-terrarium"))], None));
            }
            if !manifest.contains_key(&crate::buildtiles::index_logical()) {
                jobs.push(job(format!("buildings {}", crate::buildtiles::RELEASE), "Roadside buildings for the whole world (Overture)", "buildings", vec!["--dem".into(), s(&self.o.bin.join("dem"))], None));
            } else if let Ok(rd) = std::fs::read_dir(self.o.home.join("scratch/buildings")) {
                // Made: the scan's parts (~40 GB) go, even from a run stopped before it removed them.
                for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("parts-")) {
                    std::fs::remove_dir_all(e.path()).ok();
                }
            }
            if let Some(w) = build::summits_work(date, &manifest, &keys) {
                jobs.push(job(format!("summits {date}"), "Summits for the whole world", "summits", [p.clone(), vec!["--cache".into(), s(&self.o.home.join("cache"))]].concat(), Some(w)));
            }
            if let Some(w) = build::labels_work(date, &manifest, &keys) {
                jobs.push(job(format!("labels {date}"), "Place labels for the whole world", "labels", [p.clone(), vec!["--dem".into(), s(&self.o.bin.join("dem"))]].concat(), Some(w)));
            }
        }
        if recipes.is_empty() {
            return jobs;
        }
        let Some(date) = pass else {
            waiting.push(Waiting { what: "Building the regions".into(), why: "the first OpenStreetMap pass (it makes the outlines regions are drawn from)".into() });
            return jobs;
        };
        let outlines = manifest.get(&format!("sources/osm/{date}/outlines")).map(|c| crate::outlines::Outlines::open(&root.join(c))).transpose();
        let outlines = match outlines {
            Ok(o) => o,
            Err(e) => {
                waiting.push(Waiting { what: "Building the regions".into(), why: format!("the pass's outlines: {e:#}") });
                return jobs;
            }
        };
        let cov = match crate::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &root.join("inputs/outlines")) {
            Ok(c) => c,
            Err(e) => {
                waiting.push(Waiting { what: "Building the regions".into(), why: format!("{e:#}") });
                return jobs;
            }
        };
        let done = keys;
        let inputs = input_digests(root);
        let held = root.join("inputs/hold-catalog").exists();
        let cache = self.o.home.join("cache");
        let reach = self.current_reach(root, &manifest, &done, date).ok().flatten();
        if !manifest.contains_key(crate::rail::CATALOGUE) {
            waiting.push(Waiting { what: "Trains a day".into(), why: "the rail sources aren't on the NAS yet (scenic-build rail-seed)".into() });
        } else if inputs.get("keys").map(String::as_str) == Some("?") {
            waiting.push(Waiting { what: "Trains a day".into(), why: "inputs/keys.env can't be read now".into() });
        }
        let mut plan = build::plan(&cov, date, &manifest, &done, &inputs, reach.as_deref());
        // A unit's piece's size (content-named files never change: each looked up once).
        let size = |u: &str| {
            let Some(c) = manifest.get(&format!("sources/osm/{date}/pieces/{}", u.replace('/', "-"))) else { return u64::MAX };
            if let Some(&n) = self.piece_sizes.borrow().get(c) {
                return n;
            }
            let n = std::fs::metadata(root.join(c)).map_or(u64::MAX, |m| m.len());
            if n != u64::MAX {
                self.piece_sizes.borrow_mut().insert(c.clone(), n);
            }
            n
        };
        for w in plan.iter_mut().filter(|w| claims::SHARED.contains(&w.step.as_str())) {
            // What another worker builds now isn't planned here: what it leased from this Mac's
            // coordinator, or claimed (a helper on an app from before it).
            let mut others = claims::others(root, &w.step, &self.me);
            if let Some(c) = &self.coord {
                others.extend(c.held(&w.step));
            }
            w.targets.retain(|t| !others.contains(&t.0));
            // The rest, offered to the workers that mount the NAS, with their pieces' sizes: each
            // takes what fits its memory.
            if let (Some(c), "unit") = (&self.coord, w.step.as_str()) {
                c.offer_units(date, w.targets.iter().map(|(t, k)| (t.clone(), k.clone(), size(t))).collect());
            }
        }
        // Nothing left to build: nothing to offer either.
        if let (Some(c), false) = (&self.coord, plan.iter().any(|w| w.step == "unit" && !w.targets.is_empty())) {
            c.offer_units(date, Vec::new());
        }
        plan.retain(|w| !w.targets.is_empty());
        for (w, total) in batches(plan) {
            // Held for review: the catalog goes to catalog-held/ (no server reads it), once.
            if w.step == "catalog" && held {
                let k = w.targets.first().map(|t| t.1.clone()).unwrap_or_default();
                if done.catalog_held.as_deref() == Some(k.as_str()) {
                    waiting.push(Waiting { what: "Publishing the new map data".into(), why: "held for review (inputs/hold-catalog); its catalog is in catalog-held/".into() });
                    continue;
                }
                let mut j = job("catalog-held".into(), "The new map data, held for review", "catalog", vec!["--held".into()], Some(build::Work { step: "catalog-held".into(), targets: vec![("catalog-held".into(), k)] }));
                j.needs = Needs { ac: false, nas: true, home: false };
                jobs.push(j);
                continue;
            }
            let mut extra: Vec<String> = w.targets.iter().map(|t| t.0.clone()).filter(|t| !matches!(t.as_str(), "catalog" | "items" | "marks" | "roadunits" | "stations" | "ferries" | "heritage-sites" | "heritage" | "overlays" | "rail-feeds" | "rail") && !t.ends_with("-root")).collect();
            match w.step.as_str() {
                "terrain" | "terrain-root" => extra.extend(["--raw".into(), s(&cache.join("aws-terrarium"))]),
                "pois" | "marks" | "stations" | "overlays" => extra.extend(["--pass".into(), date.to_string()]),
                "ferries" | "rail-feeds" => extra.extend(["--pass".into(), date.to_string(), "--dem".into(), s(&self.o.bin.join("dem"))]),
                "items" | "heritage-sites" | "heritage" | "rail" => extra.extend(["--pass".into(), date.to_string(), "--dem".into(), s(&self.o.bin.join("dem")), "--cache".into(), s(&cache)]),
                "peaks" => extra.extend(["--pass".into(), date.to_string(), "--raw".into(), s(&cache.join("aws-terrarium")), "--cache".into(), s(&cache), "--coarse-threads".into(), "6".into()]),
                "unit" => extra.extend(["--pass".into(), date.to_string(), "--dem".into(), s(&self.o.bin.join("dem")), "--cache-dir".into(), s(&cache)]),
                "trees" => extra.extend(["--pass".into(), date.to_string(), "--dem".into(), s(&self.o.bin.join("dem")), "--chm".into(), s(&cache.join("chm10"))]),
                // The server's mirror on this Mac (the agent's home is inside the app's) has the
                // same files: used instead of a second copy where it has them.
                "pack" | "lo" => {
                    extra.extend(["--cache".into(), s(&cache.join("base"))]);
                    if let Some(app) = self.o.home.parent() {
                        extra.extend(["--mirror".into(), s(&app.join("mirror"))]);
                    }
                }
                _ => {}
            }
            let n = w.targets.len();
            // "3 areas", or "8 of 480 areas" for a batch.
            let areas = if n == total { format!("{n} area{}", if n == 1 { "" } else { "s" }) } else { format!("{n} of {total} areas") };
            let what = match w.step.as_str() {
                "terrain" => format!("Terrain for the regions ({areas})"),
                "slope" => format!("Slope for the regions ({areas})"),
                "unit" => format!("Roads, elevations and scenery ({areas})"),
                "pois" => format!("Landmark candidates ({areas})"),
                "peaks" => format!("Peaks' prominence and isolation ({areas})"),
                "items" => "Wikidata facts and Wikipedia pageviews for the landmarks".to_string(),
                "heritage-sites" => "Heritage sites and designated areas for the regions".to_string(),
                "heritage" => "Heritage sites' details, fame and outlines".to_string(),
                "overlays" => "Area overlays for the map".to_string(),
                "marks" => "Landmarks for the map".to_string(),
                "roadunits" => "Which areas each road crosses".to_string(),
                "stations" => "Rail stops near the regions".to_string(),
                "ferries" => "Ferries for the whole world".to_string(),
                "rail-feeds" => "Rail timetables for the regions".to_string(),
                "rail" => "Trains a day on the regions' rail".to_string(),
                "pack" => format!("Map tiles ({areas})"),
                "trees" => format!("Tree cover ({})", areas.replace("area", "large tile")),
                "lo" => "Zoomed-out map tiles".to_string(),
                "terrain-root" | "slope-root" => "World-level terrain and slope".to_string(),
                "prune" => "Removing what the regions no longer cover".to_string(),
                _ => "Publishing the new map data".to_string(),
            };
            let id = format!("{} {}", w.step, w.targets.first().map(|t| t.0.as_str()).unwrap_or(""));
            let step = w.step.clone();
            let mut j = job(id, &what, &step, extra, Some(w));
            // (A catalog and a prune only write a little: no power needed.)
            j.needs = Needs { ac: !matches!(step.as_str(), "catalog" | "prune"), nas: true, home: false };
            jobs.push(j);
        }
        jobs
    }

    /// The build to the end (the status's checklist): the OSM pass (its stages, from the markers its
    /// scratch folder keeps), the pass's worldwide jobs, then the regions' steps (build::checklist).
    fn checklist(&self, root: &Path, regions: &[recipes::Recipe]) -> Vec<build::Step> {
        let mut out = Vec::new();
        let have = crate::osmpass::latest_pass(root);
        // The pass: a newer planet's under way (or waiting), else done.
        let pass_stages = ["filter", "sets", "outlines", "basemap", "cut", "roads"];
        let mut pass = build::Step { what: "OpenStreetMap pass".into(), steps: vec!["osm-pass".into()], total: Some(pass_stages.len()), unit: "stages".into(), ..Default::default() };
        match crate::osmpass::newer_planet(root, have.as_deref()) {
            Ok(Some((_, date))) => {
                let scratch = self.o.home.join("scratch").join(format!("osm-{date}"));
                pass.done = pass_stages.iter().filter(|s| scratch.join(format!("{s}.done")).exists()).count();
            }
            _ => pass.done = if have.is_some() { pass_stages.len() } else { 0 },
        }
        out.push(pass);
        let Some(date) = have else {
            // Nothing to size the rest by until a pass is complete: its steps, to come.
            out.push(build::Step { what: "Worldwide sets, route ends, roads' reach, buildings, summits, labels".into(), steps: ["pass-sets", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels"].iter().map(|s| s.to_string()).collect(), ..Default::default() });
            out.extend(build::checklist_to_come());
            return out;
        };
        let manifest: BTreeMap<String, String> = std::fs::read(root.join("state/build/manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let keys = build::Keys::load(root);
        // The pass's worldwide jobs.
        let left = [
            !crate::osmpass::SETS.iter().all(|st| manifest.contains_key(&crate::osmpass::set_name(&date, st.0))),
            build::trailends_work(&date, &manifest, &keys).is_some() || !manifest.contains_key(&crate::osmpass::set_name(&date, "hikes")),
            build::reach_work(&date, &manifest, &keys).is_some(),
            !manifest.contains_key(&crate::terrain_z8::logical()),
            !manifest.contains_key(&crate::buildtiles::index_logical()),
            build::summits_work(&date, &manifest, &keys).is_some() || !manifest.contains_key(&format!("work/summits/{date}")),
            build::labels_work(&date, &manifest, &keys).is_some(),
        ]
        .iter()
        .filter(|&&l| l)
        .count();
        out.push(build::Step {
            what: "Worldwide sets, route ends, roads' reach, buildings, summits, labels".into(),
            steps: ["pass-sets", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels"].iter().map(|s| s.to_string()).collect(),
            left: Some(left),
            ..Default::default()
        });
        if regions.is_empty() {
            return out;
        }
        let outlines = manifest.get(&format!("sources/osm/{date}/outlines")).and_then(|c| crate::outlines::Outlines::open(&root.join(c)).ok());
        let Ok(cov) = crate::coverage::Coverage::from_recipes(regions, outlines.as_ref(), &root.join("inputs/outlines")) else { return out };
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        out.extend(build::checklist(&cov, &date, &manifest, &keys, &input_digests(root), root.join("inputs/hold-catalog").exists(), reach.as_deref()));
        out
    }

    /// Per region, how many of its areas are built (none before the first pass makes the outlines).
    fn region_progress(&self, root: &Path, regions: &[recipes::Recipe]) -> BTreeMap<String, build::RegionState> {
        let Some(date) = crate::osmpass::latest_pass(root) else { return BTreeMap::new() };
        let manifest: BTreeMap<String, String> = std::fs::read(root.join("state/build/manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let outlines = manifest.get(&format!("sources/osm/{date}/outlines")).and_then(|c| crate::outlines::Outlines::open(&root.join(c)).ok());
        let dir = root.join("inputs/outlines");
        let Ok(cov) = crate::coverage::Coverage::from_recipes(regions, outlines.as_ref(), &dir) else { return BTreeMap::new() };
        let each: Vec<(String, crate::coverage::Coverage)> = regions
            .iter()
            .filter_map(|r| crate::coverage::Coverage::from_recipes(std::slice::from_ref(r), outlines.as_ref(), &dir).ok().map(|c| (r.id.clone(), c)))
            .collect();
        let keys = build::Keys::load(root);
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        build::region_states(&cov, &each, &date, &manifest, &keys, reach.as_deref(), &input_digests(root))
    }

    /// The pass's reaches (crate::reach), once they're made for the current version (Ok(None)
    /// until then: no unit is planned, and the reach job comes first among the pass's worldwide
    /// jobs), or why they can't be read.
    fn current_reach(&self, root: &Path, manifest: &BTreeMap<String, String>, keys: &build::Keys, date: &str) -> Result<Option<std::rc::Rc<crate::reach::Reaches>>, crate::reach::LoadError> {
        if build::reach_work(date, manifest, keys).is_some() {
            return Ok(None);
        }
        let Some(c) = manifest.get(&crate::reach::logical(date)) else { return Ok(None) };
        let mut cached = self.reach.borrow_mut();
        if let Some((have, r)) = cached.as_ref() {
            if have == c {
                return Ok(Some(r.clone()));
            }
        }
        let Some(r) = crate::reach::Reaches::load(root, manifest, date)? else { return Ok(None) };
        let r = std::rc::Rc::new(r);
        *cached = Some((c.clone(), r.clone()));
        Ok(Some(r))
    }

    /// A newer app is installed locally (`../current` points elsewhere than the agent's folder).
    fn newer_app(&self) -> bool {
        let Some(apps) = self.o.bin.parent() else { return false };
        if self.app == "development" {
            return false;
        }
        std::fs::read_link(apps.join("current")).ok().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned())).is_some_and(|cur| cur != self.app)
    }
}

/// What jobs read from inputs/ beside the manifest, by digest: the ferry timetables
/// ("ferries-freq", by content), Taiwan's MOI DTM ("moi-dtm", by names, sizes and times: large
/// files, put there by hand), and which keys inputs/keys.env holds ("keys", `key_names`).
fn input_digests(root: &Path) -> BTreeMap<String, String> {
    let mut inputs: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(root.join("inputs/ferries/freq")) {
        let mut files: Vec<(String, Vec<u8>)> = rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).ok()?))).collect();
        files.sort();
        let all: Vec<u8> = files.iter().flat_map(|(n, b)| n.bytes().chain(b.iter().copied())).collect();
        inputs.insert("ferries-freq".into(), store::naming::hash16(&all));
    }
    // The regions as a catalog records them; "?" when they can't be read now (build::catalog_work
    // then waits).
    inputs.insert("regions".into(), regions_digest(root).unwrap_or_else(|| "?".into()));
    if let Ok(rd) = std::fs::read_dir(root.join("inputs/moi-dtm")) {
        let mut files: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "tif"))
            .filter_map(|e| {
                let md = e.metadata().ok()?;
                let t = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
                Some(format!("{} {} {t}", e.file_name().to_string_lossy(), md.len()))
            })
            .collect();
        files.sort();
        if !files.is_empty() {
            inputs.insert("moi-dtm".into(), store::naming::hash16(files.join("\n").as_bytes()));
        }
    }
    inputs.insert("keys".into(), key_names(&root.join("inputs/keys.env")).unwrap_or_else(|| "?".into()));
    inputs
}

/// The names of the keys a `KEY=value` file holds a value for, sorted and comma-separated ("" when
/// there's no file; None when it can't be read). Never their values: the names go in job keys.
fn key_names(path: &Path) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(String::new()),
        Err(_) => return None,
    };
    let mut names: Vec<&str> = text.lines().filter_map(|l| l.split_once('=')).filter(|(k, v)| !k.trim().starts_with('#') && !v.trim().is_empty()).map(|(k, _)| k.trim()).collect();
    names.sort_unstable();
    names.dedup();
    Some(names.join(","))
}

/// The recipes, and the outline files they name (by size and time), hashed; None when a read fails.
fn regions_digest(root: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for e in std::fs::read_dir(root.join("inputs/regions")).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".toml") {
            continue;
        }
        let text = std::fs::read_to_string(e.path()).ok()?;
        for entry in recipes::parse(&name, &text).map(|r| r.outline).unwrap_or_default() {
            let file = match recipes::parse_outline(&entry) {
                Ok(recipes::Outline::Poly(f)) => root.join("inputs/outlines").join(f),
                Ok(recipes::Outline::Geofabrik(g)) => root.join("inputs/outlines/geofabrik").join(format!("{}.poly", g.replace('/', "-"))),
                _ => continue,
            };
            match std::fs::metadata(&file) {
                Ok(md) => {
                    let t = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                    parts.push(format!("{entry} {} {t}", md.len()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => parts.push(format!("{entry} missing")),
                Err(_) => return None,
            }
        }
        parts.push(format!("{name} {text}"));
    }
    parts.sort();
    Some(store::naming::hash16(parts.join("\n").as_bytes()))
}

/// Why a job can't run under `c`, if it can't.
fn lapsed(n: &Needs, c: &Conditions) -> Option<String> {
    if n.nas && !c.nas {
        return Some("the NAS isn't reachable".into());
    }
    if n.home && !c.home {
        return Some("away from home: it moves the whole planet or world through the NAS, which waits for the home network".into());
    }
    // CPU work: on mains power, or on battery down to BATTERY_MIN.
    if n.ac && !c.ac && c.battery.is_none_or(|b| b < cond::BATTERY_MIN) {
        let at = c.battery.map(|b| format!(" at {b}%")).unwrap_or_default();
        return Some(format!("on battery{at}: waiting for mains power (it builds on battery down to {}%)", cond::BATTERY_MIN));
    }
    None
}

/// The app version of the programs in `bin` (`…/app/<version>/`), or "development".
pub fn app_version(bin: &Path) -> String {
    let canon = bin.canonicalize().unwrap_or_else(|_| bin.to_path_buf());
    match (canon.parent().and_then(|p| p.file_name()), canon.file_name()) {
        (Some(app), Some(v)) if app == "app" => v.to_string_lossy().into_owned(),
        _ => "development".into(),
    }
}

/// Writes a small file that is replaced each time (status files), through a `.tmp` and a rename.
fn write_replace(p: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    // On the NAS's SMB share a rename over a file another Mac has open (its server reading the
    // heartbeat) fails as busy for a moment: tried again a few times before giving up this beat.
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, p) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy && tries < 8 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(anyhow::Error::new(e).context(format!("rename to {}", p.display()))),
        }
    }
}

/// The status a `scenic status` shows: the NAS's heartbeat, else this Mac's copy.
pub fn read_status(root: Option<&Path>, home: &Path) -> Option<Status> {
    root.and_then(|r| std::fs::read(r.join("state/status.json")).ok())
        .or_else(|| std::fs::read(home.join("status.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// A step's targets in batches, each its own job recording its own targets (a failure or a restart
/// into a new app costs one batch, not the whole wave), with the step's total.
fn batches(plan: Vec<build::Work>) -> Vec<(build::Work, usize)> {
    let mut out = Vec::new();
    for w in plan {
        let (n, total) = (batch_size(&w.step), w.targets.len());
        if total <= n {
            out.push((w, total));
            continue;
        }
        for chunk in w.targets.chunks(n) {
            out.push((build::Work { step: w.step.clone(), targets: chunk.to_vec() }, total));
        }
    }
    out
}

/// The helpers building now: `state/helpers/<host>.json` beaten within ten minutes.
fn helpers(root: &Path) -> Vec<Status> {
    let Ok(rd) = std::fs::read_dir(root.join("state/helpers")) else { return Vec::new() };
    let mut out: Vec<Status> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<Status>(&b).ok()))
        .filter(|h| now_s().saturating_sub(h.beat) < 600)
        .map(|h| Status { regions: Vec::new(), checklist: Vec::new(), built: BTreeMap::new(), recent: Vec::new(), ..h })
        .collect();
    out.sort_by(|a, b| a.host.cmp(&b.host));
    out
}

/// A job's step and targets when both Macs run its step (crate::agent::claims::SHARED).
fn shared_targets(spec: &JobSpec) -> Option<(String, Vec<String>)> {
    spec.record.as_ref().filter(|w| claims::SHARED.contains(&w.step.as_str())).map(|w| (w.step.clone(), w.targets.iter().map(|t| t.0.clone()).collect()))
}

/// Targets per job for the steps whose work is per area (each z3 pack of terrain or slope takes
/// tens of minutes; an area's roads and scenery minutes; candidates, peaks and map tiles less).
fn batch_size(step: &str) -> usize {
    match step {
        "terrain" | "trees" => 1,
        "slope" | "lo" => 2,
        "unit" => 6,
        "peaks" => 12,
        "pack" => 16,
        "pois" => 24,
        _ => usize::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_with_the_hand_offs_waiting_on_the_nas_and_in_the_journal() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("root"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let a = agent(&root, &home);
        let mut keys = build::Keys::default();
        keys.record("unit", &[("6/1/1".to_string(), "k1".to_string())]);
        keys.save(&root).unwrap();
        // An older helper's hand-off on the NAS, and one the coordinator journaled here: both count
        // as built until merged; then the merge writes them into the keys and deletes them.
        let done = |u: &str, k: &str| crate::handoff::Handoff { done: Some(("unit".into(), vec![(u.into(), k.into())])), ..Default::default() };
        crate::handoff::write(&crate::handoff::dir(&root, "old-m1"), &done("6/1/2", "k2")).unwrap();
        crate::handoff::write(&home.join("coord/journal/m1"), &done("6/1/3", "k3")).unwrap();
        let k = a.planning_keys(&root).unwrap();
        assert_eq!((k.recorded("unit", "6/1/1"), k.recorded("unit", "6/1/2"), k.recorded("unit", "6/1/3")), (Some("k1"), Some("k2"), Some("k3")));
        assert_eq!(crate::handoff::merge_from(&root, &home.join("scratch/handoff"), &a.handoff_bases(&root)).unwrap(), 2);
        let k = build::Keys::load(&root);
        assert_eq!((k.recorded("unit", "6/1/2"), k.recorded("unit", "6/1/3")), (Some("k2"), Some("k3")));
        assert!(crate::handoff::waiting_in(&home.join("coord/journal")).unwrap().is_empty());
    }

    fn agent(root: &Path, home: &Path) -> Agent {
        Agent::new(Options { root: Some(root.to_path_buf()), home: home.to_path_buf(), bin: PathBuf::from("/nonexistent/bin"), dry_run: true, once: true, helper: false }).unwrap()
    }

    #[test]
    fn plans_daily_jobs_and_writes_a_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        std::fs::create_dir_all(root.join("inputs/regions")).unwrap();
        recipes::add(&root.join("inputs/regions"), &recipes::Recipe { id: "x".into(), name: "X".into(), outline: vec!["osm:1".into()] }).unwrap();
        let mut a = agent(&root, &home);
        a.step().unwrap();
        let st = read_status(Some(&root), &home).unwrap();
        assert_eq!(st.regions.len(), 1);
        assert!(st.conditions.nas);
        assert!(st.waiting.iter().any(|w| w.what.starts_with("Backing up")), "{:?}", st.waiting);
        // Once backed up today, it isn't due.
        a.mem.last_ok.insert("backup".into(), now_s());
        let mut w = Vec::new();
        assert!(!a.plan(&root, &st.conditions, &mut w).iter().any(|j| j.id == "backup"));
    }

    #[test]
    fn a_newer_app_starts_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        // The agent runs from app/v1; app/current points at v2.
        let apps = d.path().join("app");
        std::fs::create_dir_all(apps.join("v1")).unwrap();
        std::fs::create_dir_all(apps.join("v2")).unwrap();
        std::os::unix::fs::symlink("v2", apps.join("current")).unwrap();
        let mut a = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: apps.join("v1"), dry_run: true, once: true, helper: false }).unwrap();
        assert_eq!(a.app, "v1");
        a.step().unwrap();
        let st = read_status(Some(&root), &home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("newly installed app")), "{:?}", st.waiting);
        assert!(!st.waiting.iter().any(|w| w.why.contains("would start")), "{:?}", st.waiting);
        // Pointing back at its own version: work starts again.
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        a.step().unwrap();
        let st = read_status(Some(&root), &home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("would start")), "{:?}", st.waiting);
    }

    #[test]
    fn osm_pass_waits_for_space_or_runs() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("sources/osm/2026-09-28")).unwrap();
        std::fs::write(root.join("sources/osm/2026-09-28/planet.osm.pbf"), b"x").unwrap();
        std::fs::create_dir_all(root.join("sources/basemap")).unwrap();
        std::fs::write(root.join("sources/basemap/planetiler.jar"), b"x").unwrap();
        let a = agent(&root, &home);
        let mut w = Vec::new();
        let plan = a.plan(&root, &Conditions { ac: true, nas: true, idle_s: 0, ..Default::default() }, &mut w);
        let free = cond::free_bytes(&home).unwrap_or(0);
        if free >= PASS_SPACE {
            assert!(plan.iter().any(|j| j.id == "osm-pass 2026-09-28"));
        } else {
            assert!(w.iter().any(|x| x.why.contains("GB free")), "{w:?}");
        }
        // A complete pass of that planet: nothing to do.
        std::fs::write(root.join("sources/osm/2026-09-28/pass.0123456789abcdef.json"), b"{}").unwrap();
        let mut w = Vec::new();
        assert!(!a.plan(&root, &Conditions::default(), &mut w).iter().any(|j| j.id.starts_with("osm-pass")));
    }

    #[test]
    fn steps_in_batches() {
        let w = |step: &str, n: usize| build::Work { step: step.into(), targets: (0..n).map(|i| (format!("6/{i}/0"), format!("k{i}"))).collect() };
        let b = batches(vec![w("unit", 14), w("roadunits", 1), w("terrain", 2), w("pois", 3)]);
        let shape: Vec<(String, usize, usize)> = b.iter().map(|(w, t)| (w.step.clone(), w.targets.len(), *t)).collect();
        assert_eq!(
            shape,
            [("unit", 6, 14), ("unit", 6, 14), ("unit", 2, 14), ("roadunits", 1, 1), ("terrain", 1, 2), ("terrain", 1, 2), ("pois", 3, 3)].map(|(s, n, t)| (s.to_string(), n, t))
        );
        // Every target once, in order, with its key.
        let units: Vec<&(String, String)> = b.iter().filter(|(w, _)| w.step == "unit").flat_map(|(w, _)| &w.targets).collect();
        assert_eq!(units.len(), 14);
        assert!(units.iter().enumerate().all(|(i, t)| t.0 == format!("6/{i}/0") && t.1 == format!("k{i}")));
    }

    #[test]
    fn key_names_only() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("keys.env");
        assert_eq!(key_names(&p).as_deref(), Some(""), "no file: no keys");
        std::fs::write(&p, "LTA_ACCOUNT_KEY=s3cr3t\nODPT_KEY=\n# OLD_KEY=x\n TDX_CLIENT_ID = abc \nnot a line\n").unwrap();
        let names = key_names(&p).unwrap();
        assert_eq!(names, "LTA_ACCOUNT_KEY,TDX_CLIENT_ID", "the keys with values, by name");
        assert!(!names.contains("s3cr3t") && !names.contains("abc"));
    }

    #[test]
    fn conditions_gate_jobs() {
        let n = Needs { ac: true, nas: true, home: false };
        let at = |ac: bool, nas: bool, home: bool, battery: Option<u8>| Conditions { ac, nas, home, idle_s: 0, battery };
        assert!(lapsed(&n, &at(true, true, true, None)).is_none());
        assert!(lapsed(&n, &at(false, true, true, None)).unwrap().contains("battery"));
        assert!(lapsed(&n, &at(true, false, true, None)).unwrap().contains("NAS"));
        // On battery: on down to 30 %, then waiting.
        assert!(lapsed(&n, &at(false, true, true, Some(30))).is_none());
        assert!(lapsed(&n, &at(false, true, true, Some(29))).unwrap().contains("at 29%"));
        // Away from home, through Tailscale: on, except the whole-planet reads.
        assert!(lapsed(&n, &at(true, true, false, None)).is_none());
        assert!(lapsed(&Needs { home: true, ..n }, &at(true, true, false, None)).unwrap().contains("away from home"));
        // An older heartbeat without `home` reads as at home.
        let old: Conditions = serde_json::from_str(r#"{"ac": true, "nas": true, "idle_s": 0}"#).unwrap();
        assert!(old.home);
    }
}
