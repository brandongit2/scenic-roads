//! The build agent (docs/plan.md §8): `scenic agent`, a login item on the build Mac under the
//! launcher. It works out what needs doing, runs two jobs at once at most (the plan's first, and a
//! second beside it: `SLOTS`, `SECOND`) when their conditions hold (power, the NAS), pauses each
//! when they lapse, and writes a heartbeat the app shows.
//!
//! Each loop (every 20 s, sooner when a job ends):
//! 1. conditions: power, the NAS (mounting it when missing), the user's activity, sleep;
//! 2. the running jobs: each finished (recorded; failures retried with a growing delay), paused or
//!    resumed, restarted after sleep when it touches the NAS;
//! 3. a free slot's job: the plan's first runnable job (the OSM pass when the NAS holds a newer
//!    planet, the regions' work, the chains, the rounds; backups and cleanup once a day), and
//!    beside it the first of `SECOND`'s steps that can run with it (docs/plan.md §8, Two jobs at
//!    once);
//! 4. this Mac's caches (`room`): the owner's ask to clear them taken up (cleared between jobs once
//!    the build is done, else declined, why said), else, with no job running, a trim once the build
//!    is done; either on a thread of its own, the loop beating meanwhile and no job starting here;
//! 5. the heartbeat: `state/status.json` on the NAS, and a copy in the agent's local folder.
//!
//! Nothing depends on the build Mac being available: until work is done, the map serves the last
//! catalog. A job is a child process (see `jobs`) that resumes from its own completion markers, so
//! stopping it at any time loses at most its current stage.

pub mod backup;
pub mod build;
pub mod claims;
pub mod cond;
pub mod forecast;
pub mod gc;
pub mod jobs;
pub mod recipes;
pub mod room;

use anyhow::{Context, Result};
use cond::{Conditions, SleepWatch};
use jobs::{now_s, JobSpec, Needs, Running};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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

/// How long a job asked to stop at its next safe point (the build pausing, the battery low) is given
/// before it's frozen where it is instead: a unit takes up to ~11 min, a map tile ~10.
const DRAIN_GRACE: Duration = Duration::from_secs(15 * 60);

/// How many jobs the build Mac runs at once (docs/plan.md §8, Two jobs at once): the plan's first it
/// can run, and a second beside it (`SECOND`). A helper runs one, as the build Mac leases them.
const SLOTS: usize = 2;

/// The steps the second job takes, in its order of preference: the trains' and the landmarks'
/// steps that mostly wait on the internet first, then the candidates and peaks (the landmarks wait
/// for them), then units and slope. A unit spent 380 of its 860 s writing to the NAS and reading
/// the caches, not computing (6/17/25, 2026-10-05): two at once build more.
const SECOND: [&str; 10] = ["heritage", "items", "rail-feeds", "rail", "marks", "overlays", "pois", "peaks", "unit", "slope"];

/// Those of them that mostly wait on the network, and run beside the first job while the Mac is in
/// use too (the others only while it isn't).
const LIGHT: [&str; 6] = ["heritage", "items", "rail-feeds", "rail", "marks", "overlays"];

/// The last round of publishing, in the agent's folder (build::Round): its jobs read the units of
/// the one under way there (crate::out::UNITS_AS_OF_ENV).
const ROUND_FILE: &str = "round.json";

/// Steps that run alone, never beside another job: the pass's worldwide jobs (the planet, the
/// world's buildings, a whole set at a time) and removing replaced files from the NAS.
const ALONE: [&str; 10] = ["osm-pass", "pass-sets", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels", "heritage-sites", "gc"];

/// Steps that read AWS's raw terrain tiles here, which a terrain run packs onto the NAS and
/// deletes here: never two at once.
const RAW: [&str; 4] = ["terrain", "terrain-root", "terrain-z8", "peaks"];

/// Steps that ask Wikidata and Wikipedia a great deal from this Mac's address, each pacing itself
/// as if it were alone (heritagedetails: under ten a minute): never two at once.
const WIKI: [&str; 2] = ["items", "heritage"];

/// Steps that keep the pass's Wikidata and Wikipedia answers here and on the NAS (crate::answers):
/// none starts while the agent sends them as it starts (`Agent::seed_answers`).
const ANSWERED: [&str; 3] = ["items", "heritage-sites", "heritage"];

/// Whether jobs of steps `a` and `b` can't run at once: either runs alone, both read the raw tiles,
/// both ask Wikidata, or they're the same step and it isn't a shared one (one job's work: its
/// targets held by nothing else). A shared step's targets are held apart (crate::coord), and each
/// slot has a scratch folder of its own.
fn clash(a: &str, b: &str) -> bool {
    let both = |set: &[&str]| set.contains(&a) && set.contains(&b);
    ALONE.contains(&a) || ALONE.contains(&b) || both(&RAW) || both(&WIKI) || (a == b && !claims::SHARED.contains(&a))
}

/// The memory a job of `step` is expected to take beside another (MB) before its own run has said:
/// the network steps' Python and osmium; the shared steps' first guesses (`first_peak`), a unit's
/// the most its batches took here (8.4 GB, 2026-10-05).
fn second_peak(step: &str) -> u64 {
    match step {
        "heritage" | "rail" => 6144,
        "items" => 3072,
        "rail-feeds" => 1024,
        "marks" | "overlays" => 4096,
        "unit" => 8600,
        "pois" => 2048,
        s => first_peak(s),
    }
}

/// The free space the second job starts with (it makes no room: `reads_caches`): the network steps'
/// a little, the others' the build Mac's reserve.
fn second_need(step: &str) -> u64 {
    if LIGHT.contains(&step) {
        10 << 30
    } else {
        room::RESERVE
    }
}

/// The build Mac's second job as a worker (the build's history, the forecast's machines, the worker
/// page): the Mac's name, marked.
pub fn second_worker(host: &str) -> String {
    format!("{host} (second job)")
}

/// A job slot's running job and what goes with it: its lease (the coordinator's, or a helper's
/// granted one), when it last beat and its claims were kept fresh, when it was asked to stop at its
/// next safe point, and its time left as the status last worked it out.
#[derive(Default)]
struct Slot {
    running: Option<Running>,
    lease: Option<Held>,
    beaten: Option<Instant>,
    claims_fresh: Option<Instant>,
    drain_since: Option<Instant>,
    job_eta: Option<u64>,
}

/// How a job's lease ended, for the coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// Every target built.
    Done,
    /// Stopped at a safe point, the build pausing: the targets it finished handed off.
    Paused,
    /// Stopped, not failed (the Mac slept, the agent stopped, its work went to another, it
    /// couldn't start here for its conditions): not held against its targets.
    Interrupted,
    Failed,
}

/// The free space a helper's jobs start with (its Mac has less room than the build Mac), but for
/// the steps that need more (`helper_need`).
const HELPER_RESERVE: u64 = 15 << 30;

/// The free space a helper's job of `step` needs: a terrain run's as on the build Mac (its area's
/// archives copied here and merged); tree cover's the build Mac's reserve (it copies every canopy
/// square its tile's coverage touches here first, ~2 GB each: tens of GB for a large tile); a
/// task's ("tail": a unit's last steps, its files fetched from the coordinator) 5 GB; the others'
/// `HELPER_RESERVE`.
fn helper_need(step: &str) -> u64 {
    match step {
        "terrain" => room::RESERVE + TERRAIN_SPACE,
        "trees" => room::RESERVE,
        "tail" => 5 << 30,
        _ => HELPER_RESERVE,
    }
}

/// The work a helper asks for (the shared steps, and "tail" for tasks): what its disk has free for
/// (a step's need and its margin), or can have, from the caches it may empty (`free` the disk's free
/// bytes, `cheap` what `make_room` can delete there: room::helper_cheap_bytes). A job granted that
/// still can't have its room once the caches are emptied is given back (`run_once`).
fn helper_steps(free: u64, cheap: u64) -> Vec<String> {
    claims::SHARED.iter().copied().chain(["tail"]).filter(|s| free.saturating_add(cheap) >= helper_need(s) + room::margin(helper_need(s))).map(str::to_string).collect()
}

/// What a terrain run needs past the others' room: its area's raw tiles held twice while they're
/// packed onto the NAS (loose, then in their archives), and on a run again the area's archives
/// copied here and merged (12 to 15 GB for a z3 area of land). The US's first runs took the build
/// Mac from 34 GB free to 14 GB (2026-10-05). The area's own archive copies, which the run reads at
/// once, are spared (`terrain_reads`).
const TERRAIN_SPACE: u64 = 25 << 30;

/// Whether `p` is an archive copy a terrain run (`id`: "terrain 3/x/y") reads at once: its z3
/// area's own (`3-x-y.…`) and its z6 tiles' (`6-X-Y.…` within it), crate::rawpack's areas.
fn terrain_reads(id: &str, p: &Path) -> bool {
    let tile = |s: &str| -> Option<(u32, u32, u32)> {
        let mut v = s.split(['/', '-']).map(|t| t.parse::<u32>().ok());
        Some((v.next()??, v.next()??, v.next()??))
    };
    let Some((3, x, y)) = id.strip_prefix("terrain ").and_then(tile) else { return false };
    let in_packs = p.parent().and_then(Path::file_name).is_some_and(|d| d == "packs");
    let area = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('.').next()).and_then(tile);
    in_packs && (matches!(area, Some((3, ax, ay)) if (ax, ay) == (x, y)) || matches!(area, Some((6, ax, ay)) if (ax >> 3, ay >> 3) == (x, y)))
}

/// The memory a shared step's job is expected to take (MB) before one has run for its target and
/// said (`SCENIC_COSTS`): tree cover's program holds a band of a block's rows on each thread and
/// the blocks made but not yet written (crate::trees: 1.05 GB on 14 threads for 3/2/2's 792 blocks
/// and for 3/4/2's 79, 2026-10-05), 2.5 GB with the job's own; slope holds a z6 tile's tiles at a
/// time (crate::slope_pack: under a GB, where holding its whole area's took up to 20 GB), 2 GB;
/// peaks, room to spare; a step shared later, 1.5 GB until it's measured. (Units and candidates are
/// offered by their piece's size, crate::coord::job_peak; terrain by its area's size,
/// `terrain_peak`.)
fn first_peak(step: &str) -> u64 {
    match step {
        "trees" => 2500,
        "slope" => 2000,
        "peaks" => 2500,
        _ => 1500,
    }
}

/// The memory a terrain run of an area of `z6` tiles near the coverage is expected to take (MB),
/// before one has said: it holds a z6 tile's shaded hi tiles at a time (z9–12, up to 5,440, ~270 KB
/// each until written: crate::terrain_pack::build_q_with) with its z12 repairs while its z11 is made
/// (up to 4,096 at 256 KB), each z6 tile's z9 repairs and quarters (~20 MB) and the area's zoomed-out
/// tiles (z3–8, 1,365), and half a GB besides: 3.3 to 4.6 GB, what a helper spares. (Measured
/// before it wrote a z6 tile at a time: 32.9 GB for 3/0/2's whole area at once.)
fn terrain_peak(z6: usize) -> u64 {
    500 + 5440 * 270 / 1024 + 4096 / 4 + 1365 * 270 / 1024 + z6 as u64 * 20
}

/// The memory a helper spares its jobs (MB): three eighths of its Mac's (6 GB of the M1's 16, the
/// owner's choice, 2026-10-05: a terrain area's 5–6 GB fits; its units took 3.7 GB at most over
/// its first 205).
/// The memory a helper spares (MB): three eighths of its Mac's; while its owner is away (`away`),
/// five eighths.
fn helper_memory(away: bool) -> u64 {
    let total = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok());
    total.map_or(6144, |b| (b >> 20) * if away { 5 } else { 3 } / 8)
}

/// Whether a helper's owner is away (on mains power, not used for a quarter of an hour): it then
/// spares more of its memory, for a job predicted to end within `AWAY_JOB` (before they're likely
/// back; one still running when they are ends soon).
fn helper_away(c: &Conditions) -> bool {
    c.ac && c.idle_s >= 15 * 60
}
const AWAY_JOB: u64 = 20 * 60;

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
            Some((v["unit"].as_str()?.to_string(), crate::coord::Cost { peak_mb: v["peak_mb"].as_u64()?, secs: v["secs"].as_u64().unwrap_or(0), worker: None, v: v["v"].as_u64().unwrap_or(0) as u32 }))
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
    /// The build Mac's second job, beside the first (docs/plan.md §8, Two jobs at once), and why
    /// there's none when its slot is free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside: Option<JobView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside_why: Option<String>,
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
    /// The build's pause (crate::control), while it's paused, as this agent knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<crate::control::Pause>,
    /// This Mac's memory, load, disk and caches, and the NAS's answer and room (cond::Resources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<cond::Resources>,
    /// When each step and region will be done and on the map, and what each machine does next
    /// (the build Mac's: crate::agent::forecast).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forecast: Option<forecast::Forecast>,
    /// The last catalog the plan read: its number and when it went out (seconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogSeen>,
    /// This Mac's build caches (room::Caches): what a clear would free, why they can't be cleared
    /// now, and the last trim after the build and the last clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caches: Option<room::Caches>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogSeen {
    pub n: u64,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobView {
    pub id: String,
    pub what: String,
    pub started: u64,
    /// Why it's paused (frozen where it is), when it is.
    pub paused: Option<String>,
    /// Why it's stopping at its next safe point (the build pausing), while it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pausing: Option<String>,
    /// Its log's last lines.
    pub tail: String,
    /// Its parts (a job of more than one says them, crate::agent::jobs::part), and the one it's on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<usize>,
    /// How far it says it is (its log's last `progress:` line), with an estimate of the time left
    /// from its pace since it started saying so.
    #[serde(default)]
    pub progress: Option<JobProgress>,
    /// The memory it holds now (MB: its processes', summed), and the worker threads it was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobProgress {
    pub done: f64,
    pub total: f64,
    pub unit: String,
    pub eta_s: Option<u64>,
    /// When it last moved on (seconds since the epoch): long ago, the job may be stuck.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Waiting {
    pub what: String,
    pub why: String,
    /// The step of the job it's about (the checklist's notes say why its step waits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
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
    /// When a catalog last started (seconds since the epoch): the next round of publishing waits an
    /// hour from it, whether or not it went out (build::PUBLISH_EVERY_S).
    #[serde(default)]
    catalog_at: u64,
    /// About how long a target of each step takes here (seconds, as its jobs went lately): the
    /// forecast's, for the steps no other worker does.
    #[serde(default)]
    step_secs: BTreeMap<String, f64>,
    /// When a job last ended here (seconds since the epoch; not a daily one, which reads no cache),
    /// and this Mac's caches' last trim after the build and last clear (room::Freed): a trim again
    /// only once a job has run here since.
    #[serde(default)]
    worked_at: u64,
    #[serde(default)]
    trimmed: Option<room::Freed>,
    #[serde(default)]
    cleared: Option<room::Freed>,
    /// The last ask to clear them not done, and why (the last done one kept apart).
    #[serde(default)]
    declined: Option<room::Freed>,
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

/// How long the NAS took to answer the last check (ms; u64::MAX: it didn't), for the status.
static ANSWERED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// Whether `root` answers within a few seconds (a stat on a worker thread; a hung share counts as
/// away, and its thread is left to finish on its own, the only one for that share until it does).
fn answers(root: &Path) -> bool {
    if !CHECKING.lock().unwrap().insert(root.to_path_buf()) {
        return false;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let p = root.to_path_buf();
    let t = Instant::now();
    std::thread::spawn(move || {
        let ok = std::fs::metadata(p.join("catalog")).is_ok();
        CHECKING.lock().unwrap().remove(&p);
        let _ = tx.send(ok);
    });
    let ok = rx.recv_timeout(Duration::from_secs(8)).unwrap_or(false);
    ANSWERED_MS.store(if ok { t.elapsed().as_millis() as u64 } else { u64::MAX }, std::sync::atomic::Ordering::Relaxed);
    ok
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

/// The coverage of the regions, and each region's own.
struct Coverages {
    all: crate::coverage::Coverage,
    each: Vec<(String, crate::coverage::Coverage)>,
}

/// What a coverage is made from, as keys: the recipes, the pass's outlines (their content name), and
/// the outline files (names, sizes and times, the Geofabrik folder's too); and what the owner's
/// edits change: the recipes and the outline files they name (any other file there, a Finder's
/// .DS_Store or an editor's swap file, is none). None when the outline files can't be read now.
fn coverage_key(recipes: &[recipes::Recipe], outlines: Option<&str>, dir: &Path) -> Option<(String, String)> {
    let mut parts: Vec<String> = recipes.iter().map(|r| format!("{} {}", r.id, r.outline.join(" "))).collect();
    let mut edits = parts.clone();
    for entry in recipes.iter().flat_map(|r| r.outline.iter()) {
        let file = match recipes::parse_outline(entry) {
            Ok(recipes::Outline::Poly(f)) => dir.join(f),
            Ok(recipes::Outline::Geofabrik(g)) => dir.join("geofabrik").join(format!("{}.poly", g.replace('/', "-"))),
            _ => continue,
        };
        match std::fs::metadata(&file) {
            Ok(m) => {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
                edits.push(format!("{entry} {} {t}", m.len()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => edits.push(format!("{entry} missing")),
            Err(_) => return None,
        }
    }
    let pass = format!("outlines {}", outlines.unwrap_or("-"));
    for d in [dir.to_path_buf(), dir.join("geofabrik")] {
        let rd = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        let mut files: Vec<String> = Vec::new();
        for e in rd {
            let e = e.ok()?;
            let m = e.metadata().ok()?;
            if m.is_file() {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
                files.push(format!("{} {} {t}", e.file_name().to_string_lossy(), m.len()));
            }
        }
        files.sort();
        parts.extend(files);
    }
    edits.sort();
    parts.push(pass);
    Some((store::naming::hash16(parts.join("\n").as_bytes()), store::naming::hash16(edits.join("\n").as_bytes())))
}

/// How long the regions' work waits after the owner changes a recipe or an outline file, for more
/// edits: three edits in a row built the same regions' heritage sites three times, and their
/// terrain twice (2026-10-05). At most `EDIT_HOLD_MAX` from the first of a run of edits, each within
/// `EDIT_HOLD` of the last: something touching a file for ever holds nothing for ever.
const EDIT_HOLD: Duration = Duration::from_secs(15 * 60);
const EDIT_HOLD_MAX: Duration = Duration::from_secs(60 * 60);

/// While a run of edits (its first and last: `Agent::edited_at`) holds the regions' work: how long
/// ago the last was, and how long the hold has left.
fn edit_held(edited: Option<(std::time::SystemTime, std::time::SystemTime)>) -> Option<(Duration, Duration)> {
    let (first, last) = edited?;
    let (run, age) = (first.elapsed().ok()?, last.elapsed().ok()?);
    (age < EDIT_HOLD && run < EDIT_HOLD_MAX).then(|| (age, (EDIT_HOLD - age).min(EDIT_HOLD_MAX - run)))
}

pub struct Agent {
    o: Options,
    /// Held unless another agent runs (then this one only plans and reports: a dry run).
    _lock: Option<AgentLock>,
    host: String,
    app: String,
    started: u64,
    mem: Memory,
    /// The jobs running: the first slot's, and on the build Mac a second's beside it (`SLOTS`).
    slots: [Slot; SLOTS],
    /// Why the second slot has nothing, when it hasn't, for the status.
    beside_why: Option<String>,
    /// The disk's free bytes as a test sets them (`disk_free`).
    free_set: Option<u64>,
    sleep: SleepWatch,
    last_mount_try: Option<Instant>,
    /// The heartbeat last written to the NAS (without its time) and when: written again only when it
    /// changes or every five minutes, so an idle NAS can rest.
    last_beat: Option<(Vec<u8>, Instant)>,
    /// How far each region is built, and when that was worked out.
    progress: Option<(Instant, BTreeMap<String, build::RegionState>, Vec<build::Step>)>,
    /// The pass's reaches as last read, by content name (large: read again only when they change).
    reach: std::cell::RefCell<Option<(String, std::rc::Rc<crate::reach::Reaches>)>>,
    /// The coverage and each region's as last made, with what they were made from (`coverage`).
    coverage: std::cell::RefCell<Option<(String, std::rc::Rc<Coverages>)>>,
    /// The recipes' and outline files' key as last seen, and when it changed while this agent ran
    /// (an edit: `EDIT_HOLD`): first in a run of edits, and last.
    edits: std::cell::RefCell<Option<String>>,
    edited_at: std::cell::Cell<Option<(std::time::SystemTime, std::time::SystemTime)>>,
    /// Who this agent is in claims ("<host> <pid>"; its second job's, `me_of`).
    me: String,
    /// The OSM pieces' sizes by content name (the coordinator sizes units by them; content-named
    /// files never change).
    piece_sizes: std::cell::RefCell<std::collections::HashMap<String, u64>>,
    /// Whether this Mac's earlier agent's claims were dropped (once the NAS answers), and when this
    /// Mac was last named the records' writer.
    claims_dropped: bool,
    writer_named: Option<Instant>,
    /// What the last plan found waiting and the regions it read, and when: while
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
    /// A helper's: the bytes its cheap caches held (room::cheap_bytes) and when they were counted.
    cheap: Option<(Instant, u64)>,
    /// The last catalog's folder and number, and its regions' outline entries by id (`on_map`).
    last_catalog: std::cell::RefCell<Option<((PathBuf, u64), BTreeMap<String, Vec<String>>)>>,
    /// The regions the last plan would publish as built (build::Plan::ready), for the checklist.
    ready: std::cell::RefCell<Vec<String>>,
    /// The build's pause as this agent knows it (crate::control): its coordinator's on the build
    /// Mac; a helper's as heard in its asks and beats, or as asked for here and not yet passed on
    /// (`pause_local`). Kept in `pause.json`.
    pause: Option<crate::control::Pause>,
    pause_local: bool,
    /// The build Mac's pause as last mirrored to the NAS, and whether this agent's own (kept from
    /// before its coordinator was up) has been given to the coordinator.
    mirrored: Option<Option<crate::control::Pause>>,
    pause_pushed: bool,
    /// Jobs an earlier agent left (a crash): each one's step and the targets it noted done, to record
    /// once the NAS answers.
    orphan_done: Vec<(String, Vec<(String, String)>)>,
    /// The bytes the caches hold, for the status: what room-making can free and what a clear would
    /// (room::sizes), counted on a thread of its own every ten minutes (a walk of many files), and
    /// again after a trim or a clear; and when.
    cache_size: std::sync::Arc<std::sync::Mutex<CacheCount>>,
    /// A helper's view of the build: the build Mac's heartbeat as last read, at most each minute.
    heard: Option<Heard>,
    /// A trim or a clear under way, and when one last failed (it's tried again ten minutes later).
    caches_task: Option<CachesTask>,
    trim_failed: Option<Instant>,
    /// The build Mac's: the pass's answers this Mac has sent to the NAS as the agent starts
    /// (crate::answers::seed), on a thread of its own; and whether it was started (once an agent).
    answers_seed: Option<std::thread::JoinHandle<Result<Vec<String>>>>,
    answers_seeded: bool,
    /// The build Mac's: the helpers' last trim, clear or declined ask noted in the history (its
    /// time), and what their last trim kept, by host.
    helper_caches: BTreeMap<String, (u64, u64)>,
    /// Jobs an earlier agent left that couldn't be shown stopped (jobs::Orphan): their process
    /// groups, running work while they're still the jobs' (jobs::Group::is_the_jobs).
    orphans: Vec<jobs::Group>,
    /// The conditions the last loop saw: a change is noted in the history (crate::coord::history).
    last_cond: Option<Conditions>,
    /// The last plan's forecast (crate::agent::forecast), and the last catalog it read, for the
    /// status.
    forecast: std::cell::RefCell<Option<forecast::Forecast>>,
    catalog_seen: std::cell::Cell<Option<CatalogSeen>>,
    /// The last round of publishing (build::Round, its folder's `round.json`): the one under way,
    /// which its jobs read the units of, or the last one over, for when it began.
    round: std::cell::RefCell<Option<build::Round>>,
}

/// The caches' sizes as last counted (room::sizes: what room-making can free, what a clear would),
/// and when that count began.
type CacheCount = (Option<Instant>, Option<room::Sizes>);

/// A trim or a clear under way on a thread of its own (`Agent::caches_task`).
struct CachesTask {
    /// The clear's ask (None: a trim), and when it began.
    ask: Option<room::ClearRequest>,
    began: Instant,
    thread: std::thread::JoinHandle<Result<room::Freed>>,
}

/// The build Mac's heartbeat as a helper last read it (`Agent::heard`).
struct Heard {
    /// When it was read, and when the build Mac beat (None: it couldn't be read).
    read: Instant,
    beat: Option<u64>,
    /// Why the build isn't done (`work_left`; None: it is).
    left: Option<String>,
}

/// The last plan's view, kept for the heartbeat between plans.
struct Planned {
    at: Instant,
    waiting: Vec<Waiting>,
    regions: Vec<recipes::Recipe>,
    bad: Vec<(String, String)>,
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
            match crate::coord::Coordinator::start(&o.home.join("coord"), Some(o.bin.join("wasm")), crate::coord::PORT, &cond::host_name(), &app) {
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
        // The build's pause as this agent last knew it (a helper that can't reach the build Mac stays
        // as it was).
        let pause: Option<crate::control::Pause> = std::fs::read(o.home.join("pause.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        // (A round's file that doesn't read: none under way, the next begins afresh.)
        let round: Option<build::Round> = std::fs::read(o.home.join(ROUND_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        Ok(Agent { host: cond::host_name(), app, started: now_s(), mem, slots: Default::default(), beside_why: None, free_set: None, sleep: SleepWatch::default(), last_mount_try: None, last_beat: None, progress: None, reach: Default::default(), coverage: Default::default(), edits: Default::default(), edited_at: Default::default(), _lock: lock, o, me, piece_sizes: Default::default(), claims_dropped: false, writer_named: None, planned: None, merged: None, coord, published: None, client: None, cheap: None, last_catalog: Default::default(), ready: Default::default(), pause, pause_local: false, mirrored: None, pause_pushed: false, orphan_done: Vec::new(), cache_size: Default::default(), heard: None, caches_task: None, trim_failed: None, answers_seed: None, answers_seeded: false, helper_caches: BTreeMap::new(), orphans: Vec::new(), last_cond: None, forecast: Default::default(), catalog_seen: Default::default(), round: std::cell::RefCell::new(round) })
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
                Ok(None) => waiting.push(Waiting { step: None, what: "Building".into(), why: "the build Mac isn't coordinating (its agent isn't running, or is on an older app)".into() }),
                Err(e) => waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e:#}") }),
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
        let running = match &self.slots[0].lease {
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
            // (No result: the agent stopped while it ran. What it finished, from its step and
            // targets kept beside its saves and the targets it noted done, handed off as an
            // interrupted job's.)
            let result = result.or_else(|| {
                let w: build::Work = serde_json::from_slice(&std::fs::read(d.join("work.json")).ok()?).ok()?;
                let names = crate::control::read_done(&d.join("done.txt"), &w.step);
                let done: Vec<(String, String)> = w.targets.iter().filter(|(t, _)| names.contains(t)).cloned().collect();
                Some(serde_json::json!({ "ok": !done.is_empty(), "done": if done.is_empty() { serde_json::Value::Null } else { serde_json::json!([w.step, done]) }, "interrupted": true, "error": "the helper's agent stopped while it ran" }))
            });
            let sent = (|| -> Result<crate::coord::client::Handed> {
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
                        // (Only the files of the targets it finished: a target it was on when it
                        // stopped may have saved part of its own.)
                        if let Some((step, done)) = h.done.clone() {
                            h.changes.retain(|l, _| done.iter().any(|(t, _)| crate::coord::saves(&step, t, l)));
                            let kept: std::collections::BTreeSet<String> = h.changes.values().flatten().cloned().collect();
                            h.pending.retain(|c, _| kept.contains(c));
                            let pending = h.pending.clone();
                            h.checked.retain(|c| pending.contains_key(c));
                        }
                        let costs = read_costs(&d.join("costs.jsonl"));
                        client.done(&crate::coord::Done { lease, handoff: Some(h), costs, failed: r["failed"].as_bool() == Some(true), ..Default::default() })
                    }
                    _ => {
                        let why = match &result {
                            Some(r) if r["ok"].as_bool() == Some(true) && r["done"].is_null() => "it ended without a result".to_string(),
                            Some(r) if r["ok"].as_bool() == Some(true) => "one of its saves is damaged".to_string(),
                            Some(r) => r["error"].as_str().unwrap_or("it failed").to_string(),
                            None => "the helper's agent stopped while it ran".to_string(),
                        };
                        // (Stopped, not failed: the build paused before it finished a target, the Mac
                        // slept, its agent stopped: given back, not held against its targets.)
                        let interrupted = result.as_ref().is_none_or(|r| r["interrupted"].as_bool() == Some(true));
                        if interrupted {
                            client.give_back(lease, &why).map(|()| crate::coord::client::Handed::Taken)
                        } else {
                            client.fail(lease, &why, None).map(|()| crate::coord::client::Handed::Taken)
                        }
                    }
                }
            })();
            match sent {
                Ok(handed) => {
                    match handed {
                        crate::coord::client::Handed::Taken => {}
                        crate::coord::client::Handed::Gone => eprintln!("agent: the coordinator no longer holds lease {lease}: its work is dropped (it was offered again)"),
                        crate::coord::client::Handed::Refused(why) => {
                            // Not what the lease asked for: given back as failed (not offered to
                            // this Mac again for a while), and dropped.
                            eprintln!("agent: the coordinator refused lease {lease}'s hand-off ({why}); giving it back as failed");
                            if let Err(e) = client.fail(lease, &format!("its hand-off was refused: {why}"), None) {
                                eprintln!("agent: giving lease {lease} back: {e:#} (it lapses)");
                            }
                        }
                    }
                    std::fs::remove_dir_all(&d).ok();
                }
                Err(e) => {
                    waiting.push(Waiting { step: None, what: "Handing work back".into(), why: format!("{e:#}; trying again") });
                    break;
                }
            }
        }
    }

    /// What a job of `step` is given after its targets (the pass's `date`, where this Mac's caches
    /// and programs are): the build Mac's own jobs' and a helper's alike.
    fn step_args(&self, step: &str, date: &str) -> Vec<String> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let cache = self.o.home.join("cache");
        let dem = || s(&self.o.bin.join("dem"));
        match step {
            "terrain" | "terrain-root" => vec!["--raw".into(), s(&cache.join("aws-terrarium"))],
            "pois" | "marks" | "stations" | "overlays" => vec!["--pass".into(), date.to_string()],
            "ferries" | "rail-feeds" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem()],
            "items" | "heritage-sites" | "heritage" | "rail" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem(), "--cache".into(), s(&cache)],
            "peaks" => vec!["--pass".into(), date.to_string(), "--raw".into(), s(&cache.join("aws-terrarium")), "--cache".into(), s(&cache), "--coarse-threads".into(), "6".into()],
            "unit" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem(), "--cache-dir".into(), s(&cache)],
            "trees" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem(), "--chm".into(), s(&cache.join("chm10"))],
            // The server's mirror on this Mac (the agent's home is inside the app's) has the same
            // files: used instead of a second copy where it has them.
            "pack" | "lo" => {
                let mut v = vec!["--cache".into(), s(&cache.join("base"))];
                if let Some(app) = self.o.home.parent() {
                    v.extend(["--mirror".into(), s(&app.join("mirror"))]);
                }
                v
            }
            _ => Vec::new(),
        }
    }

    /// A helper's next job: work of the plan the build Mac's coordinator leases it (a job's worth of
    /// a shared step's targets, none needing more memory than it spares), built from the pass it
    /// says, saving into the lease's outbox folder. Asked only when it could start now.
    fn helper_job(&mut self, root: &Path, c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let needs = Needs { cpu: true, nas: true, home: false };
        if let Some(why) = lapsed(&needs, c) {
            waiting.push(Waiting { step: None, what: "Building".into(), why });
            return Vec::new();
        }
        // Not while this Mac's own pause holds (its ask not yet with the build Mac: the start loop
        // says so), nor while a newer app waits to start: nothing would start. (While the build Mac
        // says the build is paused, it asks: the answer says when it goes on.)
        if self.pause_local {
            return Vec::new();
        }
        if self.newer_app() {
            waiting.push(Waiting { step: None, what: "Building".into(), why: "restarting into the newly installed app".into() });
            return Vec::new();
        }
        // The work its disk has room for (what its caches can free counted at most every ten
        // minutes, a walk of thousands of files, and again after a job or room-making).
        let cache = self.o.home.join("cache");
        let cheap = match self.cheap {
            Some((at, n)) if at.elapsed() < Duration::from_secs(600) => n,
            _ => {
                let n = room::helper_cheap_bytes(&cache);
                self.cheap = Some((Instant::now(), n));
                n
            }
        };
        let can = helper_steps(room::disk_free(&self.o.home).unwrap_or(0), cheap);
        if can.is_empty() {
            let need = helper_need("tail");
            waiting.push(Waiting { step: None, what: "Building".into(), why: format!("the disk has too little room ({:.1} GB free needed, with what its caches can free)", (need + room::margin(need)) as f64 / (1u64 << 30) as f64) });
            return Vec::new();
        }
        let away = helper_away(c);
        let ask = crate::coord::Ask {
            kind: "native".into(),
            label: Some(format!("{} (helper)", self.host)),
            can,
            mem_mb: helper_memory(false),
            cores: std::thread::available_parallelism().map_or(4, |n| n.get() as u32),
            max: batch_size("unit"),
            app: Some(self.app.clone()),
            more_mb: away.then(|| helper_memory(true)),
            max_secs: away.then_some(AWAY_JOB),
            ..Default::default()
        };
        let Some(client) = self.client(root, waiting) else { return Vec::new() };
        let asked = client.ask(&ask);
        let fail = |a: &Self, lease: u64, why: &str| {
            if let Some(c) = &a.client {
                c.fail(lease, why, None).ok();
            }
        };
        // (Work given: the build isn't paused.)
        if matches!(asked, Ok(Some(_))) {
            self.know_pause(None);
        }
        match asked {
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Job { step, targets, pass }, .. })) if claims::SHARED.contains(&step.as_str()) => {
                let dir = self.outbox().join(lease.to_string());
                // (Its step and targets, kept with its saves: should this agent stop, the next hands
                // off what of them it finished.)
                let kept = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("work.json"), serde_json::to_vec(&build::Work { step: step.clone(), targets: targets.clone() }).unwrap_or_default()));
                if let Err(e) = kept {
                    waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e}") });
                    fail(self, lease, &format!("its outbox: {e}"));
                    return Vec::new();
                }
                let s = |p: &Path| p.to_string_lossy().into_owned();
                // The build Mac's own command for it, its saves handed off (SCENIC_HANDOFF).
                let mut cmd = vec!["/usr/bin/env".to_string(), format!("SCENIC_HANDOFF={}", dir.display()), format!("SCENIC_COSTS={}", dir.join("costs.jsonl").display())];
                cmd.extend([s(&self.o.bin.join("scenic-build")), step.clone(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch").join(&step))]);
                cmd.extend(targets.iter().map(|t| t.0.clone()));
                cmd.extend(self.step_args(&step, &pass));
                let n = targets.len();
                let id = format!("{step} {}", targets.first().map(|t| t.0.as_str()).unwrap_or(""));
                let what = format!("{} ({n} area{}, for the build Mac)", build::label(&step), if n == 1 { "" } else { "s" });
                self.slots[0].lease = Some(Held::Leased { lease, dir });
                vec![JobSpec { id, what, cmd, needs, restart_after_sleep: true, record: Some(build::Work { step, targets }) }]
            }
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Task { id, task, .. }, .. })) => {
                // A task (a unit's tail for the build Mac's job), run here by `scenic run-task` over
                // its files fetched from the coordinator, reading the NAS's data where it lies
                // (only reading it: crate::unit::Tools::stores_read_only).
                let dir = self.outbox().join(lease.to_string());
                let spec = dir.join("spec.json");
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&spec, task.to_string())) {
                    waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e}") });
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
                    // (What a page reads through the coordinator, read on the NAS here.)
                    "--root".into(),
                    s(root),
                ];
                let unit = task["unit"].as_str().unwrap_or("").to_string();
                self.slots[0].lease = Some(Held::Leased { lease, dir });
                vec![JobSpec { id: format!("task {id}"), what: format!("Scenery for the build Mac's area {unit}"), cmd, needs: Needs { cpu: true, nas: true, home: false }, restart_after_sleep: false, record: None }]
            }
            Ok(Some(g)) => {
                fail(self, g.lease, "this helper can't do that work");
                Vec::new()
            }
            Ok(None) => {
                self.know_pause(None);
                waiting.push(Waiting { step: None, what: "Building".into(), why: "the build Mac has nothing for this Mac now".into() });
                Vec::new()
            }
            Err(e) => {
                let why = match e.downcast_ref::<crate::coord::client::Refused>() {
                    Some(r) => {
                        // (The build paused: this Mac too, until an answer says it goes on; refused
                        // for anything else, it isn't paused.)
                        if !self.pause_local {
                            self.know_pause(r.pause.clone());
                        }
                        r.why.clone()
                    }
                    None => format!("the build Mac can't be reached: {e:#}"),
                };
                // (Paused: the start loop says so.)
                if self.pause.is_none() {
                    waiting.push(Waiting { step: None, what: "Building".into(), why });
                }
                Vec::new()
            }
        }
    }

    /// Slot `k`'s job ended (or stopped): its lease ended with it. This Mac's own: done when it
    /// recorded its targets, and what its units cost learned; a helper's: its result in its outbox
    /// folder, to send.
    fn end_lease(&mut self, k: usize, outcome: Outcome, done: &[(String, String)], note: &str) {
        // (A job ended: what a helper's caches can free is counted again before it next asks.)
        self.cheap = None;
        let pid = self.slots[k].running.as_ref().map(|r| r.pgid as u32);
        match self.slots[k].lease.take() {
            Some(Held::Own(id)) => {
                if let Some(c) = &self.coord {
                    c.finish(id, done);
                    if let Some(p) = pid {
                        c.close_tasks(p);
                    }
                    let costs = self.costs_path(k);
                    c.add_costs_by(&read_costs(&costs), &self.worker_of(k));
                    std::fs::remove_file(costs).ok();
                }
            }
            Some(Held::Leased { dir, .. }) => {
                // What it did: every target, or (paused at a safe point) those it finished, handed
                // off; else given back, failed or (stopped, not failed) interrupted.
                // (A failed job's finished targets are handed off too, the rest held against it.)
                let step = self.slots[k].running.as_ref().and_then(|r| r.spec.record.as_ref().map(|w| w.step.clone()));
                let done = step.filter(|_| outcome != Outcome::Interrupted && !done.is_empty()).map(|s| (s, done.to_vec()));
                let failed = outcome == Outcome::Failed && done.is_some();
                // A task's: what `scenic run-task` wrote (its outputs are with the coordinator already).
                let task: Option<serde_json::Value> = std::fs::read(dir.join("task.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let ok = match &task {
                    Some(t) => outcome == Outcome::Done && t["ok"] == true,
                    None => done.is_some(),
                };
                let interrupted = !ok && matches!(outcome, Outcome::Paused | Outcome::Interrupted);
                let r = serde_json::json!({ "ok": ok, "done": done, "failed": failed, "interrupted": interrupted, "task": task.and_then(|t| t.get("task").cloned()), "error": note.chars().take(3000).collect::<String>() });
                if let Err(e) = crate::whole::write(&dir.join("result.json"), r.to_string().as_bytes()) {
                    eprintln!("agent: writing a job's result for the coordinator: {e:#}");
                }
            }
            None => {}
        }
    }

    /// Beats for slot `k`'s job's lease (each minute it isn't paused); false when it's lost: it
    /// lapsed and its work went to another worker. (This Mac's own lapses when its job paused past
    /// it, or this loop was stuck: it's held again when no one took its work meanwhile.)
    fn beat(&mut self, k: usize, root: Option<&Path>) -> bool {
        self.slots[k].beaten = Some(Instant::now());
        // (Its progress as the status last read it: what it said, out of sight or not.)
        let progress = self.slots[k].running.as_ref().and_then(|r| r.said.clone().or_else(|| jobs::progress(&r.log).map(|(d, t, u)| (d, t, u, None)))).map(|(d, t, u, _)| format!("{}/{t:.0} {u}", (d * 10.0).floor() / 10.0));
        match self.slots[k].lease.clone() {
            Some(Held::Own(id)) => {
                let Some(c) = &self.coord else { return true };
                if c.renew(id, progress) {
                    return true;
                }
                let work = self.slots[k].running.as_ref().and_then(|r| r.spec.record.clone());
                match work.and_then(|w| c.hold(&w.step, &w.targets)) {
                    Some(id) => {
                        self.slots[k].lease = Some(Held::Own(id));
                        true
                    }
                    None => false,
                }
            }
            Some(Held::Leased { lease, .. }) => {
                // (Without the NAS, through the client it has: its lease kept, the pause heard.)
                let client = match root {
                    Some(r) => self.client(r, &mut Vec::new()),
                    None => self.client.as_ref(),
                };
                match client.map(|c| c.beat_paused(lease, progress.as_deref())) {
                    // (The build's pause as the build Mac says: this job stops with it, or goes on;
                    // unless this Mac's own ask isn't with it yet.)
                    Some(Ok((alive, pause))) => {
                        if !self.pause_local {
                            self.know_pause(pause);
                        }
                        alive
                    }
                    // (The coordinator away: the job goes on, and is handed back once it's there.)
                    _ => true,
                }
            }
            None => true,
        }
    }

    /// Drops slot `k`'s job's claims (crate::agent::claims), as it ends.
    fn release_claims(&mut self, k: usize, root: Option<&Path>) {
        // (A helper's leased job holds no claim files.)
        if self.o.helper {
            self.slots[k].claims_fresh = None;
            return;
        }
        if let (Some((step, ts)), Some(r)) = (self.slots[k].running.as_ref().and_then(|j| shared_targets(&j.spec)), root) {
            claims::release(r, &step, &ts, &self.me_of(k));
        }
        self.slots[k].claims_fresh = None;
    }

    /// Keeps slot `k`'s job's claims fresh (every two minutes: a claim lasts `claims::STALE`), not
    /// while it's paused: a paused job's claims go stale, so the other Mac may take them. False when
    /// another agent holds one of them now (the job is then stopped, unrecorded: that one builds it).
    fn keep_claims(&mut self, k: usize, root: Option<&Path>) -> bool {
        // (A helper's leased job holds no claim files: its lease is kept by beats.)
        if self.o.helper {
            return true;
        }
        let me = self.me_of(k);
        let paused = self.pause.is_some();
        let slot = &mut self.slots[k];
        let (Some(j), Some(r)) = (slot.running.as_ref(), root) else { return true };
        let Some((step, ts)) = shared_targets(&j.spec) else { return true };
        if claims::lost(r, &step, &ts, &me) {
            return false;
        }
        if (j.paused.is_none() || paused) && slot.claims_fresh.is_none_or(|t| t.elapsed() >= Duration::from_secs(120)) {
            claims::refresh(r, &step, &ts, &me);
            slot.claims_fresh = Some(Instant::now());
        }
        true
    }

    fn record_path(&self, k: usize) -> PathBuf {
        self.o.home.join(if k == 0 { "job.json" } else { "job-2.json" })
    }

    /// Where slot `k`'s job notes what its units cost (`SCENIC_COSTS`).
    fn costs_path(&self, k: usize) -> PathBuf {
        self.o.home.join(if k == 0 { "costs.jsonl" } else { "costs-2.jsonl" })
    }

    /// Who slot `k`'s job is in claims: the second's apart from the first's, so neither takes the
    /// other's as its own.
    fn me_of(&self, k: usize) -> String {
        if k == 0 {
            self.me.clone()
        } else {
            format!("{} {}", self.me, k + 1)
        }
    }

    /// Slot `k`'s worker in the build's history and the forecast: this Mac, or its second job.
    fn worker_of(&self, k: usize) -> String {
        if k == 0 {
            self.host.clone()
        } else {
            second_worker(&self.host)
        }
    }

    /// Whether this agent runs a second job: the build Mac's (a helper's are leased one at a time),
    /// with its coordinator holding a shared step's targets apart, not a dry run.
    fn second_allowed(&self) -> bool {
        !self.o.helper && !self.o.dry_run && self._lock.is_some() && self.coord.is_some()
    }

    /// The free space on this Mac's disk (where the agent's folder is).
    fn disk_free(&self) -> u64 {
        self.free_set.unwrap_or_else(|| room::disk_free(&self.o.home).unwrap_or(0))
    }

    /// Whether neither slot runs a job.
    fn idle(&self) -> bool {
        self.slots.iter().all(|s| s.running.is_none())
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
            // (What they finished, recorded once the NAS answers: a helper's goes back with its
            // lease's outbox, send_outbox.)
            for k in 0..SLOTS {
                let orphan = jobs::stop_orphan(&self.record_path(k));
                self.orphans.extend(orphan.left);
                if let Some((w, file)) = orphan.done {
                    let names = crate::control::read_done(&file, &w.step);
                    let done: Vec<(String, String)> = w.targets.into_iter().filter(|(t, _)| names.contains(t)).collect();
                    if !done.is_empty() && !self.o.helper {
                        self.orphan_done.push((w.step, done));
                    }
                }
            }
        }
        loop {
            let quick = self.step()?;
            if self.o.once || STOP.load(Ordering::SeqCst) {
                break;
            }
            // A newer app is in place and the first job's slot is free: exit (the second job stops
            // here, its finished targets kept, and goes on under the new one), and the launcher
            // starts the new one.
            if self.slots[0].running.is_none() && self.newer_app() {
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
        // A trim or a clear under way: asked to end at its next file (also when a newer app takes
        // over), and waited for a minute at most: a call to a NAS that hangs may not return, and
        // the launcher waits for the agent. Left mid-way, nothing's lost: copies are written by
        // temporary names, a raw tile goes only once its archive is named in the index (one put up
        // and never named goes a day later), and a clear's ask stays aside (`room::CLEAR_TAKEN`).
        // The next agent trims again, or takes the ask up again.
        if let Some(t) = self.caches_task.take() {
            STOP.store(true, Ordering::SeqCst);
            let waited = Instant::now();
            while !t.thread.is_finished() && waited.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(200));
            }
            if !t.thread.is_finished() {
                eprintln!("agent: this Mac's caches' {} hasn't ended in a minute; left to the next agent", if t.ask.is_some() { "clear" } else { "trim" });
            }
        }
        let root = self.o.root.clone().or_else(|| self.root());
        for k in 0..SLOTS {
            if let Some(r) = self.slots[k].running.as_mut() {
                eprintln!("agent: stopping {}", r.spec.id);
                r.stop(Duration::from_secs(30));
                self.stopped(k, root.as_deref(), "the agent stopped");
                let r = self.slots[k].running.take().unwrap();
                std::fs::remove_file(self.record_path(k)).ok();
                // Its claims, free for the other Mac now rather than once stale.
                if let (Some((step, ts)), Some(root), false) = (shared_targets(&r.spec), &root, self.o.helper) {
                    claims::release(root, &step, &ts, &self.me_of(k));
                }
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
        if home && self.idle() && self.o.root.is_none() {
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
        self.note_conditions(&c, slept);
        let mut waiting: Vec<Waiting> = Vec::new();
        let mut ended = false;
        // The build's pause: this Mac's ask passed on; the build Mac's coordinator's.
        self.sync_pause(root.as_deref(), &mut waiting);
        // (A crashed agent's jobs' finished targets, recorded now the NAS answers.)
        if let Some(r) = root.as_deref() {
            let orphans = std::mem::take(&mut self.orphan_done);
            for (step, done) in orphans {
                if self.record_done(Some(r), &step, &done) {
                    eprintln!("agent: recorded {} target{} of {step} an earlier agent's job finished", done.len(), if done.len() == 1 { "" } else { "s" });
                } else {
                    self.orphan_done.push((step, done));
                }
            }
        }

        // The running jobs.
        for k in 0..SLOTS {
            ended |= self.tend(k, &c, root.as_deref(), slept)?;
        }
        // The pass's answers on the NAS, once this agent starts (the build Mac's).
        self.seed_answers(root.as_deref());

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

        // The coordinator: where the NAS is (tasks read it where it lies), how to reach it on the NAS
        // (looked at every five minutes: it may have been deleted, or this Mac's addresses changed),
        // and leases whose workers went quiet dropped.
        if let Some(c) = &self.coord {
            c.set_root(root.as_deref());
        }
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
        // alone writes them): each loop when the first job's slot is free, every two minutes while
        // a job runs there.
        let idle = self.slots[0].running.is_none();
        let merge_due = idle || ended || self.merged.is_none_or(|t| t.elapsed() >= Duration::from_secs(120));
        if let (Some(r), false, true, true) = (root.as_ref(), self.o.helper, self._lock.is_some() && !self.o.dry_run, merge_due) {
            match crate::handoff::merge_from(r, &self.o.home.join("scratch/handoff"), &self.handoff_bases(r)) {
                Ok(0) => {}
                Ok(n) => eprintln!("agent: merged {n} hand-off{} from other workers", if n == 1 { "" } else { "s" }),
                Err(e) => eprintln!("agent: merging other workers' hand-offs: {e:#}"),
            }
            self.merged = Some(Instant::now());
        }

        // The plan: start the first job that can run. Made when one could start (the first job's
        // slot free; the second's, each minute), when a job ends, and otherwise every five minutes
        // for the heartbeat (between, its last view is shown).
        let second_free = self.second_allowed() && self.slots[1].running.is_none();
        // (Not while the first runs alone: nothing would start beside it.)
        let alone = self.slots[0].running.as_ref().is_some_and(|r| ALONE.contains(&step_of(&r.spec.id).unwrap_or_default().as_str()));
        let every = Duration::from_secs(if second_free && !alone { 60 } else { 300 });
        let plan_due = idle || ended || self.planned.as_ref().is_none_or(|p| p.at.elapsed() >= every);
        let plan = match &root {
            Some(root) if self.o.helper => {
                // A helper plans nothing: it asks the build Mac for work, once its last is handed back.
                let unsent = std::fs::read_dir(self.outbox()).map(|mut d| d.next().is_some()).unwrap_or(false);
                // (Not while a trim or a clear runs here: `caches_busy`.)
                let plan = if idle && !unsent && self.caches_task.is_none() { self.helper_job(root, &c, &mut waiting) } else { Vec::new() };
                let regions = match &self.planned {
                    Some(p) if !plan_due => p.regions.clone(),
                    _ => recipes::load(&root.join("inputs/regions")).0,
                };
                self.planned = Some(Planned { at: Instant::now(), waiting: waiting.clone(), regions, bad: Vec::new() });
                plan
            }
            Some(root) if plan_due => {
                let plan = self.plan(root, &c, &mut waiting);
                let (regions, bad) = recipes::load(&root.join("inputs/regions"));
                self.planned = Some(Planned { at: Instant::now(), waiting: waiting.clone(), regions, bad });
                plan
            }
            Some(_) => {
                waiting.extend(self.planned.as_ref().map(|p| p.waiting.clone()).unwrap_or_default());
                Vec::new()
            }
            None => {
                waiting.push(Waiting { step: None, what: "All building".into(), why: "the NAS isn't reachable (away from home, or it's off)".into() });
                Vec::new()
            }
        };
        // A newer app installed: nothing new starts, so the loop exits once the first job's slot is
        // free and the launcher starts the new one (with work queued back to back, it would
        // otherwise never get a turn).
        let newer = self.newer_app();
        if newer && idle {
            waiting.push(Waiting { step: None, what: "Building".into(), why: "restarting into the newly installed app".into() });
        }
        // The build paused: nothing new starts (the running jobs stop above), and on the build Mac
        // its coordinator gives its workers nothing and has their jobs stop too.
        if let Some(p) = &self.pause {
            waiting.push(Waiting { step: None, what: "Building".into(), why: p.why() });
        }
        // A trim or a clear of this Mac's caches under way: nothing starts here until it's done.
        let caches_busy = self.caches_busy();
        if let Some(why) = caches_busy.clone() {
            waiting.push(Waiting { step: None, what: "Building".into(), why });
        }
        if idle && !newer && self.pause.is_none() && caches_busy.is_none() {
            self.start_first(&plan, &c, root.as_deref(), &mut waiting);
            // (Why the jobs before it wait: kept with the plan, so the status says so while the job
            // started runs, not only between jobs.)
            if let Some(p) = self.planned.as_mut() {
                p.waiting = waiting.clone();
            }
        } else if plan_due && !self.o.helper && !plan.is_empty() {
            // (Planned again while the first job runs: why the jobs before the next it would start
            // wait, as when it started.)
            self.why_waiting(&plan, &c, &mut waiting);
            if let Some(p) = self.planned.as_mut() {
                p.waiting = waiting.clone();
            }
        }
        // The second job, beside the first: the plan's first it may take (`start_second`).
        if second_free && !plan.is_empty() {
            if newer {
                self.beside_why = Some("restarting into the newly installed app".into());
            } else if let Some(p) = &self.pause {
                self.beside_why = Some(p.why());
            } else if caches_busy.is_some() {
                self.beside_why = caches_busy;
            } else {
                self.start_second(&plan, &c, root.as_deref());
            }
        } else if second_free && plan_due && root.is_some() {
            self.beside_why = Some("nothing to build now".into());
        } else if self.slots[1].running.is_some() {
            self.beside_why = None;
        }
        // A helper's lease for a job that didn't start (room on the disk, say): given back.
        if self.slots[0].running.is_none() && self.slots[0].lease.is_some() {
            self.end_lease(0, Outcome::Interrupted, &[], "it couldn't start on the helper");
        }
        // This Mac's caches, between jobs: the owner's ask to clear them, else a trim once the build
        // is done (room::clear, room::trim).
        let caches_why = self.tend_caches(root.as_deref(), c.home);

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
        let mut checklist = self.progress.as_ref().map(|(_, _, c)| c.clone()).unwrap_or_default();
        // (Read each loop: a helper's status changes between plans.)
        let helpers: Vec<Status> = match (&root, self.o.helper) {
            (Some(r), false) => helpers(r),
            _ => Vec::new(),
        };
        self.note_helpers_caches(&helpers);
        let now_steps: Vec<String> = self.slots.iter().filter_map(|s| s.running.as_ref().and_then(|r| step_of(&r.spec.id))).collect();
        annotate(&mut checklist, &now_steps, &helpers, &waiting);
        let job = self.job_view(0);
        let beside = self.job_view(1);
        let status = Status {
            host: self.host.clone(),
            pid: std::process::id(),
            app: self.app.clone(),
            beat: now_s(),
            started: self.started,
            conditions: c,
            job,
            beside,
            beside_why: if self.second_allowed() && self.slots[1].running.is_none() { self.beside_why.clone() } else { None },
            waiting,
            recent: self.mem.recent.clone(),
            regions,
            bad_recipes: bad,
            built,
            checklist,
            helpers,
            workers: self.coord.as_ref().map(|c| c.workers().into_iter().map(|(name, w)| WorkerView { name, label: w.label, kind: w.kind, what: w.what, mem_mb: w.mem_mb, done: w.done, failed: w.failed, bad: w.bad }).collect()).unwrap_or_default(),
            pause: self.pause.clone(),
            resources: Some(self.resources(root.as_deref())),
            forecast: if self.o.helper { None } else { self.forecast.borrow().clone() },
            catalog: self.catalog_seen.get(),
            caches: Some(self.caches_view(caches_why, c.home)),
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
        // (Not a dry run's, in a home of its own: the NAS's is the real agent's.)
        if let (Some(root), false) = (&root, self.o.dry_run) {
            // What's new since the last write, without the time and the user's idle seconds (which
            // change every loop): only whether the user is at the Mac counts.
            let idle_s = if c.user_active() { 0 } else { cond::AWAY_S };
            // (Nor this Mac's load, memory and the NAS's answer, which change every loop too.)
            let same = serde_json::to_vec(&Status { beat: 0, conditions: Conditions { idle_s, ..c }, resources: None, ..status.clone() })?;
            let due = self.last_beat.as_ref().is_none_or(|(b, t)| *b != same || t.elapsed() >= Duration::from_secs(120));
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

    /// Slot `k`'s running job, looked after: ended (recorded, its lease ended, a failure retried with
    /// a growing delay), restarted after a sleep when it touches the NAS, stopped when its lease
    /// lapsed or another Mac took its claims, paused or resumed as its conditions and the build's
    /// pause say. True when it ended.
    fn tend(&mut self, k: usize, c: &Conditions, root: Option<&Path>, slept: u64) -> Result<bool> {
        let Some(r) = self.slots[k].running.as_mut() else { return Ok(false) };
        if let Some(st) = r.poll()? {
            let secs = r.elapsed().as_secs();
            let ok = st.success();
            // Stopped at a safe point, the build pausing (crate::control): not a failure. (Only
            // when it was asked to: a job's own exit 75 for anything else is a failure.)
            let paused = st.code() == Some(crate::control::PAUSED_EXIT) && self.slots[k].drain_since.is_some();
            let of = r.spec.record.as_ref().map_or(0, |w| w.targets.len());
            let step = r.spec.record.as_ref().map(|w| w.step.clone()).unwrap_or_default();
            // The targets it finished: every one when it succeeded, else those it noted done as
            // each was saved (crate::control::done), whatever stopped it, so they aren't built
            // again. The build Mac's agent records them; a helper's go back to the coordinator.
            let done = self.finished_targets(k, ok);
            let recorded = self.record_done(root, &step, &done);
            let r = self.slots[k].running.as_mut().unwrap();
            let note = if ok {
                String::new()
            } else if paused {
                format!("paused at a safe point, {} of {of} done; the rest goes on when the build does", done.len())
            } else {
                format!("{st}{}\n{}", if done.is_empty() { String::new() } else { format!(" ({} of {of} done and kept)", done.len()) }, jobs::tail(&r.log, 20))
            };
            let (id, what) = (r.spec.id.clone(), r.spec.what.clone());
            eprintln!("agent: {id} {} after {secs} s", if ok { "finished" } else if paused { "paused at a safe point" } else { "failed" });
            let how = if ok { String::new() } else if paused { "paused at a safe point".to_string() } else { format!("failed ({st})") };
            self.note_end(k, &step, &done, secs, ok, &how);
            // (What a target of its step takes here, for the forecast: the first job's, and the
            // second's network work, whose time the job beside it doesn't change.)
            if ok && of > 0 && !step.is_empty() && (k == 0 || LIGHT.contains(&step.as_str())) {
                let each = secs as f64 / of as f64;
                let e = self.mem.step_secs.entry(step.clone()).or_insert(each);
                *e = 0.7 * *e + 0.3 * each;
            }
            if ok && step == "catalog" {
                let ready = self.ready.borrow().clone();
                self.note(crate::coord::history::Event { worker: Some(self.host.clone()), step: Some(step.clone()), targets: ready, ..crate::coord::history::Event::new("catalog") });
            }
            let outcome = if ok { Outcome::Done } else if paused { Outcome::Paused } else { Outcome::Failed };
            // (The coordinator's record of what's done: a helper's whatever it did, this Mac's
            // once it's in the keys.)
            let handed = if self.o.helper || recorded { done.clone() } else { Vec::new() };
            self.end_lease(k, outcome, &handed, &note);
            self.finished(&id, &what, ok || paused, secs, note);
            self.slots[k].drain_since = None;
            self.release_claims(k, root);
            self.slots[k].running = None;
            std::fs::remove_file(self.record_path(k)).ok();
            return Ok(true);
        }
        if slept > 30 && r.spec.restart_after_sleep && r.spec.needs.nas {
            // Open SMB handles often don't survive sleep: stop it; the plan below starts it again
            // from its completion markers once its conditions hold.
            eprintln!("agent: slept {slept} s; restarting {}", r.spec.id);
            r.stop(Duration::from_secs(30));
            self.stopped(k, root, "stopped: the Mac slept");
            self.release_claims(k, root);
            self.slots[k].running = None;
            std::fs::remove_file(self.record_path(k)).ok();
            return Ok(false);
        }
        let paused_job = self.slots[k].running.as_ref().unwrap().paused.is_some();
        if (!paused_job || self.pause.is_some()) && self.slots[k].lease.is_some() && self.slots[k].beaten.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) && !self.beat(k, root) {
            // (A job paused for its conditions doesn't beat: its lease lapses, and its work may go
            // to another. One paused by the user does, its lease kept, a helper hearing whether
            // its build Mac's pause is over.)
            let r = self.slots[k].running.as_mut().unwrap();
            eprintln!("agent: {}'s lease lapsed and its work went to another worker; stopping it", r.spec.id);
            r.stop(Duration::from_secs(30));
            self.stopped(k, root, "its lease lapsed");
            self.release_claims(k, root);
            self.slots[k].running = None;
            std::fs::remove_file(self.record_path(k)).ok();
            return Ok(true);
        }
        if !self.keep_claims(k, root) {
            // Another agent took its claims (they went stale while it was paused, or this Mac
            // was away): it builds them; this one stops, unrecorded, and drops the rest.
            let r = self.slots[k].running.as_mut().unwrap();
            eprintln!("agent: another Mac took {}'s areas; stopping it", r.spec.id);
            r.stop(Duration::from_secs(30));
            self.stopped(k, root, "another Mac took its areas");
            self.release_claims(k, root);
            self.slots[k].running = None;
            self.slots[k].claims_fresh = None;
            std::fs::remove_file(self.record_path(k)).ok();
            return Ok(true);
        }
        // Paused, or something it needs gone: at its next safe point (asked through its channel,
        // frozen after DRAIN_GRACE if it hasn't stopped), or at once (frozen where it is: it can't
        // save without the NAS); else going on.
        let control = self.control_path(k);
        let pause = self.pause.clone();
        let slot = &mut self.slots[k];
        let r = slot.running.as_mut().unwrap();
        match stop_for(&r.spec.needs, c, pause.as_ref()) {
            Some((crate::control::Mode::Freeze, why)) => {
                if r.paused.is_none() {
                    eprintln!("agent: pausing {}: {why}", r.spec.id);
                }
                r.pause(&why);
            }
            Some((crate::control::Mode::Drain, why)) => {
                let since = *slot.drain_since.get_or_insert_with(|| {
                    eprintln!("agent: {} stops at its next safe point: {why}", r.spec.id);
                    if let Err(e) = std::fs::write(&control, b"drain") {
                        eprintln!("agent: asking {} to stop: {e}", r.spec.id);
                    }
                    Instant::now()
                });
                if r.paused.is_none() && since.elapsed() >= DRAIN_GRACE {
                    eprintln!("agent: {} reached no safe point in {} min; frozen where it is", r.spec.id, DRAIN_GRACE.as_secs() / 60);
                    r.pause(&format!("{why}; frozen where it is (it reached no safe point in {} min): it goes on from there", DRAIN_GRACE.as_secs() / 60));
                } else if r.paused.is_some() && since.elapsed() < DRAIN_GRACE {
                    // (Frozen for a condition that's back: it goes on to its safe point.)
                    r.resume();
                }
                r.pausing = Some(why);
            }
            None => {
                if slot.drain_since.take().is_some() {
                    std::fs::write(&control, b"run").ok();
                }
                r.pausing = None;
                if r.paused.is_some() {
                    eprintln!("agent: resuming {}", r.spec.id);
                    r.resume();
                }
            }
        }
        Ok(false)
    }

    /// Starts `spec` in slot `k` when it can start now: its conditions hold, it isn't waiting out a
    /// failure, there's room on the disk for it, and (a shared step's) its targets are held for it;
    /// true when it started (a dry run: when it would have). Why not goes in `waiting` (the first
    /// slot's: the second says its own, `beside_why`).
    fn try_start(&mut self, k: usize, spec: JobSpec, c: &Conditions, root: Option<&Path>, waiting: &mut Vec<Waiting>) -> bool {
        let mut said: Vec<Waiting> = Vec::new();
        let started = self.try_start_said(k, spec, c, root, &mut said);
        if k == 0 {
            waiting.extend(said);
        } else if let Some(w) = said.into_iter().next() {
            self.beside_why = Some(format!("{}: {}", w.what, w.why));
        }
        started
    }

    /// The first job, when its slot is free: the plan's first that can start, not the second job's
    /// own, nor network work left to it (`left_to_second`); one that waits for a reason of its own
    /// passed over, one that waits for the job beside it (`waits_for_second`) waited for, not passed
    /// over. Why the jobs before it wait, in `waiting`.
    fn start_first(&mut self, plan: &[JobSpec], c: &Conditions, root: Option<&Path>, waiting: &mut Vec<Waiting>) {
        let beside = self.slots[1].running.as_ref().map(|r| r.spec.id.clone());
        for spec in plan {
            if beside.as_deref() == Some(spec.id.as_str()) || self.left_to_second(spec, plan, c) {
                continue;
            }
            if let Some(why) = self.wait_reason(spec, c) {
                waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
                continue;
            }
            if let Some(why) = self.waits_for_second(spec) {
                waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
                break;
            }
            if self.try_start(0, spec.clone(), c, root, waiting) {
                break;
            }
        }
    }

    /// Why `spec` can't start now for itself: a condition it needs gone, or a failure's wait.
    fn wait_reason(&self, spec: &JobSpec, c: &Conditions) -> Option<String> {
        if let Some(why) = lapsed(&spec.needs, c) {
            return Some(why);
        }
        // (A helper's job came from the coordinator, which keeps a target it failed from it for an
        // hour, doubling: no wait of its own on top.)
        match self.mem.retry.get(&spec.id).filter(|_| !self.o.helper) {
            Some(&(n, until)) if now_s() < until => Some(format!("failed {n} time{} in a row; trying again in {} min", if n == 1 { "" } else { "s" }, (until - now_s()).div_ceil(60))),
            _ => None,
        }
    }

    /// While the first job runs, why the plan's jobs before the next its slot would start wait
    /// (their own reasons, the job beside it), as the start loop says.
    fn why_waiting(&self, plan: &[JobSpec], c: &Conditions, waiting: &mut Vec<Waiting>) {
        let running: Vec<&str> = self.slots.iter().filter_map(|s| s.running.as_ref().map(|r| r.spec.id.as_str())).collect();
        for spec in plan.iter().filter(|s| !running.contains(&s.id.as_str()) && !self.left_to_second(s, plan, c)) {
            match self.wait_reason(spec, c).or_else(|| self.waits_for_second(spec)) {
                Some(why) => waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why }),
                None => break,
            }
        }
    }

    /// Whether the first slot leaves `spec`, network work (`LIGHT`), to the second job: while a
    /// second may run and the plan has other work the first can do (an hour of it would hold the
    /// first slot while the regions' terrain and units wait).
    fn left_to_second(&self, spec: &JobSpec, plan: &[JobSpec], c: &Conditions) -> bool {
        let light = |s: &JobSpec| LIGHT.contains(&step_of(&s.id).unwrap_or_default().as_str());
        self.second_allowed() && light(spec) && plan.iter().any(|p| !light(p) && self.wait_reason(p, c).is_none())
    }

    /// The job the first slot would start next: the plan's first that isn't running, left to the
    /// second job, or waiting for a reason of its own.
    fn head<'a>(&self, plan: &'a [JobSpec], c: &Conditions) -> Option<&'a JobSpec> {
        let running: Vec<&str> = self.slots.iter().filter_map(|s| s.running.as_ref().map(|r| r.spec.id.as_str())).collect();
        plan.iter().find(|s| !running.contains(&s.id.as_str()) && !self.left_to_second(s, plan, c) && self.wait_reason(s, c).is_none())
    }

    /// Why `spec` can't start in the first slot beside the second job's, when it can't: they don't
    /// run together (`clash`); it needs room made on the disk, which isn't while a second job runs
    /// (it may read what's deleted, and room-making holds the loop for minutes); or the two wouldn't
    /// fit three quarters of the memory. The first slot then waits for it rather than start later
    /// work, and the second starts nothing new meanwhile (`start_second`): neither starves it.
    fn waits_for_second(&self, spec: &JobSpec) -> Option<String> {
        let r = self.slots[1].running.as_ref()?;
        let (s, b) = (step_of(&spec.id).unwrap_or_default(), step_of(&r.spec.id).unwrap_or_default());
        let beside = build::label(&b);
        if clash(&s, &b) {
            return Some(format!("waits for the job beside it ({beside}) to end: they don't run together"));
        }
        // (Beside one that mostly waits on the network, room is made all the same: try_start_said.)
        let (need, free) = (self.need_of(0, spec), self.disk_free());
        if free < need && !LIGHT.contains(&b.as_str()) {
            return Some(format!("needs {} GB free on the disk ({} GB free): room is made once the job beside it ({beside}) ends", need >> 30, free >> 30));
        }
        let res = cond::resources(&self.o.home, None, None, None);
        let total = (res.mem_gb * 1024.0) as u64;
        let theirs = crate::sys::footprint_of_group(r.pgid).map_or(0, |b| b >> 20).max(self.spec_peak(&r.spec));
        let mine = self.spec_peak(spec);
        (mine + theirs > total * 3 / 4).then(|| format!("needs about {:.1} GB of memory: waits for the job beside it ({beside}, {:.1} GB) to end, in this Mac's {:.0} GB", mine as f64 / 1024.0, theirs as f64 / 1024.0, res.mem_gb))
    }

    /// The free space a job of `spec` starts with in slot `k`: a helper's its step's; the second
    /// job's its own (`second_need`); the first's the reserve, a terrain run's more, the OSM pass's
    /// its own less the pack cache it clears.
    fn need_of(&self, k: usize, spec: &JobSpec) -> u64 {
        let step = step_of(&spec.id).unwrap_or_default();
        if self.o.helper {
            helper_need(spec.record.as_ref().map_or("tail", |w| w.step.as_str()))
        } else if k > 0 {
            second_need(&step)
        } else if spec.id.starts_with("osm-pass") {
            PASS_SPACE.saturating_sub(dir_bytes(&self.o.home.join("cache").join("base"))).max(room::RESERVE)
        } else if spec.id.starts_with("terrain ") {
            room::RESERVE + TERRAIN_SPACE
        } else {
            room::RESERVE
        }
    }

    fn try_start_said(&mut self, k: usize, spec: JobSpec, c: &Conditions, root: Option<&Path>, waiting: &mut Vec<Waiting>) -> bool {
        if let Some(why) = self.wait_reason(&spec, c) {
            waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
            return false;
        }
        // (Never while a trim or a clear of this Mac's caches runs: it deletes what jobs read.)
        if let Some(why) = self.caches_busy() {
            waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
            return false;
        }
        // (Nor an items or heritage job while the answers they keep go to the NAS: `seed_answers`.)
        if self.answers_seed.as_ref().is_some_and(|t| !t.is_finished()) && step_of(&spec.id).is_some_and(|s| ANSWERED.contains(&s.as_str())) {
            waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why: "the pass's Wikidata and Wikipedia answers here are going to the NAS (seconds)".into() });
            return false;
        }
        if self.o.dry_run {
            waiting.push(Waiting { step: None, what: spec.what.clone(), why: "would start now (dry run)".into() });
            return true;
        }
        let (id, what) = (spec.id.clone(), spec.what.clone());
        let step = step_of(&id).unwrap_or_default();
        // Room on the disk for it, from the caches that are cheap to fill again (a terrain run's
        // more, its area's archive copies spared; the OSM pass's own need, less the pack cache it
        // clears; a helper's Mac has less room). Made before its targets are claimed: it can take
        // minutes, and a claim is refreshed only while a job runs. Made only while no other job
        // runs here (one may read what's deleted, and the loop that looks after it waits
        // meanwhile), or one that mostly waits on the network (it reads none of the caches, and
        // waits on its own: the first job doesn't wait hours for it): beside another, a job starts
        // only with the room there is.
        let cache = self.o.home.join("cache");
        let need = self.need_of(k, &spec);
        let others: Vec<String> = self.slots.iter().enumerate().filter(|(j, _)| *j != k).filter_map(|(_, s)| s.running.as_ref().map(|r| step_of(&r.spec.id).unwrap_or_default())).collect();
        let other = !others.is_empty() && !(k == 0 && others.iter().all(|s| LIGHT.contains(&s.as_str())));
        if k > 0 || other {
            let free = self.disk_free();
            if !self.o.helper && free < need {
                waiting.push(Waiting { step: Some(step), what: what.clone(), why: format!("needs {} GB free on the disk ({} GB free; no room is made beside another job)", need >> 30, free >> 30) });
                return false;
            }
        } else if let Some(r) = root {
            // (Never without the NAS: what goes here must be kept there.)
            // (The OSM pass without the margin: its need is what its conditions admitted it with.)
            let margin = if id.starts_with("osm-pass") { 0 } else { room::margin(need) };
            // (The pageview months stay for an items or heritage job, this one or one beside: they
            // read them.)
            let months = std::iter::once(&step).chain(&others).any(|s| s == "items" || s == "heritage");
            let pageviews = cache.join(room::MONTHS);
            match room::make_room(&cache, &r.join("sources"), need, margin, &|p| terrain_reads(&id, p) || (months && p.starts_with(&pageviews))) {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!("agent: {} GB of the cheap caches deleted (canopy squares, raw terrain tiles, copies of the records' files, pageview months) for {} GB free", n >> 30, (need + margin) >> 30);
                    self.cheap = None;
                }
                Err(e) => eprintln!("agent: making room on the disk: {e:#}"),
            }
        }
        // A helper's job its disk still has no room for (the caches emptied as far as they could
        // be): given back, not started on a Mac someone uses.
        if self.o.helper {
            let free = room::disk_free(&self.o.home).unwrap_or(0);
            if free < need {
                let why = format!("too little room on its disk: {} GB free, {} GB needed", free >> 30, need >> 30);
                waiting.push(Waiting { step: None, what: what.clone(), why: why.clone() });
                self.end_lease(k, Outcome::Failed, &[], &why);
                return false;
            }
        }
        // A job other workers may also do: its targets held first, by a lease in this Mac's
        // coordinator (and claim files, for a helper on an app from before it); when another holds
        // one, none starts (the next loop plans without it). A helper's job came with its lease.
        if let (Some((step, ts)), Some(r), false) = (shared_targets(&spec), root, self.o.helper) {
            let targets = spec.record.as_ref().map(|w| w.targets.clone()).unwrap_or_default();
            let lease = match &self.coord {
                Some(c) => match c.hold(&step, &targets) {
                    Some(id) => Some(id),
                    None => {
                        waiting.push(Waiting { step: None, what: what.clone(), why: "another worker took part of it; planning again".into() });
                        return false;
                    }
                },
                None => None,
            };
            let let_go = |a: &Self| {
                if let (Some(c), Some(id)) = (&a.coord, lease) {
                    c.finish(id, &[]);
                }
            };
            let me = self.me_of(k);
            if !claims::claim(r, &step, &ts, &me) {
                let_go(self);
                waiting.push(Waiting { step: None, what: what.clone(), why: "another Mac took part of it; planning again".into() });
                return false;
            }
            // Recorded since this loop read the keys (another worker built it meanwhile): not built
            // again.
            let done = self.planning_keys(r).map(|keys| spec.record.as_ref().is_some_and(|w| w.targets.iter().any(|(t, k)| keys.recorded(&w.step, t) == Some(k.as_str()))));
            if done.unwrap_or(true) {
                let_go(self);
                claims::release(r, &step, &ts, &me);
                waiting.push(Waiting { step: None, what: what.clone(), why: "another worker built part of it meanwhile (or the keys can't be read now); planning again".into() });
                return false;
            }
            self.slots[k].lease = lease.map(Held::Own);
            self.slots[k].claims_fresh = Some(Instant::now());
        }
        // (A catalog's start: the next round waits an hour from it, whether or not it goes out.)
        if spec.id.starts_with("catalog") {
            self.mem.catalog_at = now_s();
            self.save();
        }
        let shared = shared_targets(&spec);
        if let Err(e) = self.start(k, spec, c) {
            // It couldn't even start (a missing program, a full disk): retried later.
            eprintln!("agent: can't start {id}: {e:#}");
            self.end_lease(k, Outcome::Failed, &[], &format!("couldn't start: {e:#}"));
            if let (Some((step, ts)), Some(r), false) = (shared, root, self.o.helper) {
                claims::release(r, &step, &ts, &self.me_of(k));
            }
            self.finished(&id, &what, false, 0, format!("couldn't start: {e:#}"));
            return false;
        }
        true
    }

    /// The second job (docs/plan.md §8, Two jobs at once), when its slot is free: of the plan's
    /// jobs, the first by its steps' order (`SECOND`) that can run beside the first job's (`clash`),
    /// while the Mac is in use only one that mostly waits on the network (`LIGHT`), and that fits
    /// the memory beside it: the two jobs' within three quarters of the Mac's (the first's as
    /// predicted, or as it is now if more), and the second's free now with 2 GB to spare. Why none
    /// starts, in `beside_why`.
    fn start_second(&mut self, plan: &[JobSpec], c: &Conditions, root: Option<&Path>) {
        let rank = |s: &JobSpec| step_of(&s.id).and_then(|st| SECOND.iter().position(|x| *x == st));
        let mut picks: Vec<&JobSpec> = plan.iter().filter(|s| rank(s).is_some()).collect();
        picks.sort_by_key(|s| rank(s));
        let first = self.slots[0].running.as_ref().map(|r| (step_of(&r.spec.id).unwrap_or_default(), r.spec.id.clone(), crate::sys::footprint_of_group(r.pgid).map_or(0, |b| b >> 20), self.spec_peak(&r.spec)));
        if first.as_ref().is_some_and(|f| ALONE.contains(&f.0.as_str())) {
            self.beside_why = Some(format!("{} runs alone", build::label(&first.unwrap().0)));
            return;
        }
        // What the first slot starts next: one that runs alone, or needs room made on the disk
        // (made only while one job runs), isn't held up by a job started here now.
        if let Some(h) = self.head(plan, c) {
            let hs = step_of(&h.id).unwrap_or_default();
            if ALONE.contains(&hs.as_str()) {
                self.beside_why = Some(format!("{} runs alone, next: nothing starts beside the first job until it has", build::label(&hs)));
                return;
            }
            if self.disk_free() < self.need_of(0, h) {
                self.beside_why = Some(format!("{} needs room made on the disk next, which waits for one job alone: nothing starts beside the first job until it has", build::label(&hs)));
                return;
            }
        }
        let res = cond::resources(&self.o.home, None, None, None);
        let total_mb = (res.mem_gb * 1024.0) as u64;
        let free_mb = res.mem_free_pct.map_or(0, |p| total_mb * p as u64 / 100);
        let first_mb = first.as_ref().map_or(0, |f| f.2.max(f.3));
        let mut why: Option<String> = None;
        for spec in picks {
            let step = step_of(&spec.id).unwrap_or_default();
            if first.as_ref().is_some_and(|f| f.1 == spec.id || clash(&f.0, &step)) {
                continue;
            }
            if c.user_active() && !LIGHT.contains(&step.as_str()) {
                why.get_or_insert_with(|| "the Mac is in use: beside the first job, only work that mostly waits on the network".into());
                continue;
            }
            let mb = self.spec_peak(spec);
            if first_mb + mb > total_mb * 3 / 4 || free_mb < mb + 2048 {
                why.get_or_insert_with(|| format!("{} needs about {:.1} GB, too much beside the first job ({:.1} GB) in this Mac's {:.0} GB ({:.1} GB free)", build::label(&step), mb as f64 / 1024.0, first_mb as f64 / 1024.0, res.mem_gb, free_mb as f64 / 1024.0));
                continue;
            }
            self.beside_why = None;
            if self.try_start(1, spec.clone(), c, root, &mut Vec::new()) {
                return;
            }
            // (Why it didn't: try_start says, in beside_why; the next may.)
            why = why.or_else(|| self.beside_why.clone());
        }
        self.beside_why = Some(why.unwrap_or_else(|| "none of the work left can run beside the first job now".into()));
    }

    /// The memory a job of `spec` is expected to take (MB): its targets' largest, as measured or
    /// offered (crate::coord::Coordinator::peak), else its step's first guess (`second_peak`).
    fn spec_peak(&self, spec: &JobSpec) -> u64 {
        let step = step_of(&spec.id).unwrap_or_default();
        // (A unit's measure is its programs' most, one at a time: its job holds a GB besides.)
        let own = if step == "unit" { 1024 } else { 0 };
        match (&self.coord, spec.record.as_ref()) {
            (Some(c), Some(w)) => own + w.targets.iter().map(|(t, _)| c.peak(&w.step, t).unwrap_or_else(|| second_peak(&w.step))).max().unwrap_or(0),
            _ => second_peak(&step),
        }
    }

    /// Slot `k`'s job for the status: its parts (the last it said), its progress, and from its pace
    /// the time it has left (kept for the forecast).
    fn job_view(&mut self, k: usize) -> Option<JobView> {
        let slot = &mut self.slots[k];
        let r = slot.running.as_mut()?;
        if let Some(p) = jobs::parts(&r.log) {
            r.parts = Some(p);
        }
        let part = r.parts.as_ref().map(|p| p.0);
        // (What it last said holds while its own output since pushes the line out of sight, in the
        // same part.)
        match jobs::said(&r.log) {
            jobs::Said::Progress(d, t, u) => r.said = Some((d, t, u, part)),
            jobs::Said::NoneYet => r.said = None,
            jobs::Said::Unknown => r.said = r.said.take().filter(|s| s.3 == part),
        }
        let progress = r.said.clone().map(|(done, total, unit, _)| {
            let frac = done / total;
            // (Its pace measured again from a new unit, a new part, or a step back.)
            let key = jobs::unit_key(&unit).to_string();
            let fresh = r.progress_base.as_ref().is_none_or(|b| b.2 != key || b.3 != part || frac < b.1);
            if fresh {
                r.progress_base = Some((Instant::now(), frac, key, part));
            }
            if fresh || r.moved.is_none_or(|m| frac > m.0) {
                r.moved = Some((frac, now_s()));
            }
            let (t0, f0, _, _) = r.progress_base.as_ref().unwrap();
            let eta_s = (frac > *f0 && r.paused.is_none()).then(|| (t0.elapsed().as_secs_f64() * (1.0 - frac) / (frac - f0)) as u64);
            JobProgress { done, total, unit, eta_s, moved_at: r.moved.map(|m| m.1) }
        });
        slot.job_eta = progress.as_ref().and_then(|p| p.eta_s);
        Some(JobView {
            id: r.spec.id.clone(),
            what: r.spec.what.clone(),
            started: r.started,
            paused: r.paused.clone(),
            pausing: r.pausing.clone(),
            tail: jobs::tail(&r.log, 3),
            progress,
            parts: r.parts.as_ref().map(|p| p.1.clone()).unwrap_or_default(),
            part: r.parts.as_ref().map(|p| p.0),
            mem_mb: crate::sys::footprint_of_group(r.pgid).map(|b| b >> 20),
            threads: Some(r.threads),
        })
    }

    /// This Mac's resources for the status (cond::Resources); its caches counted again on a thread
    /// of their own when ten minutes old.
    fn resources(&self, root: Option<&Path>) -> cond::Resources {
        let (counted, bytes) = self.cache_size.lock().unwrap().clone();
        if counted.is_none_or(|t| t.elapsed() >= Duration::from_secs(600)) {
            // (Marked counted now: one walk at a time.)
            let now = Instant::now();
            self.cache_size.lock().unwrap().0 = Some(now);
            let (cache, helper, size, nas) = (self.o.home.join("cache"), self.o.helper, self.cache_size.clone(), root.map(Path::to_path_buf));
            std::thread::spawn(move || {
                let n = room::sizes(&cache, helper, nas.as_deref());
                // (Not over a count made since, a trim's or a clear's.)
                let mut s = size.lock().unwrap();
                if s.0 == Some(now) {
                    s.1 = Some(n);
                }
            });
        }
        let ms = ANSWERED_MS.load(std::sync::atomic::Ordering::Relaxed);
        cond::resources(&self.o.home, root, bytes.map(|b| (b.cheap as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0), (ms != u64::MAX && root.is_some()).then_some(ms))
    }

    /// This Mac's caches between jobs (room::trim, room::clear), each on a thread of its own while the
    /// loop goes on beating (no job starts here meanwhile: `caches_busy`): the owner's ask to clear
    /// them taken up (cleared once the build is done and no job runs here, else declined, why said),
    /// else, at home (through Tailscale, a trim would take hours), a trim once the build is done,
    /// once per finished state (again only after a job has run here since). Why they can't be
    /// cleared now (None: they can), for the status.
    fn tend_caches(&mut self, root: Option<&Path>, home: bool) -> Option<String> {
        // One under way: its result once it's done.
        if let Some(t) = self.caches_task.take() {
            if !t.thread.is_finished() {
                self.caches_task = Some(t);
                return self.caches_busy();
            }
            self.caches_done(t);
        }
        let why = self.caches_why_not(root);
        // (Not a dry run's, in another agent's folder: the real agent's to do.)
        if self._lock.is_none() || self.o.dry_run {
            return why;
        }
        let (cache, sources) = (self.o.home.join("cache"), root.map(|r| r.join("sources")));
        if let Some(ask) = room::take_clear(&self.o.home) {
            match (&why, sources) {
                (None, Some(s)) => self.caches_start(Some(ask), move || room::clear(&cache, &s)),
                _ => self.caches_record(room::Freed { why_not: why.clone(), ..Default::default() }, Some(ask)),
            }
        } else if let Some(s) = sources.filter(|_| why.is_none() && home && self.trim_due() && self.trim_failed.is_none_or(|t| t.elapsed() >= Duration::from_secs(600))) {
            // (The build Mac keeps its canopy squares: every pass's areas read them again.)
            let (squares, helper) = (cache.join("chm10"), self.o.helper);
            self.caches_start(None, move || room::trim(&cache, &s, &|p| !helper && p.starts_with(&squares)));
        }
        // (One loop at a time, `--once`: waited for, so its heartbeat says what it did.)
        if self.o.once {
            if let Some(t) = self.caches_task.take() {
                self.caches_done(t);
            }
        }
        self.caches_busy().or(why)
    }

    /// The build Mac's, once an agent, when no job runs here: the pass's answers its cache has, sent
    /// to the NAS where it hasn't their archive (crate::answers::seed: a cache from before the NAS
    /// kept them, or one a stopped job left, needn't wait for the steps' next run, perhaps the next
    /// pass's), on a thread of its own (a slow NAS doesn't hold the loop; no items or heritage job
    /// starts meanwhile). Done: what it did logged; failed: the steps' next start sends them.
    fn seed_answers(&mut self, root: Option<&Path>) {
        if let Some(t) = self.answers_seed.take_if(|t| t.is_finished()) {
            match t.join() {
                Ok(Ok(said)) => said.iter().for_each(|s| eprintln!("agent: {s}")),
                Ok(Err(e)) => eprintln!("agent: the pass's answers not sent to the NAS now ({e:#}); the items and heritage jobs' next start sends them"),
                Err(_) => eprintln!("agent: sending the pass's answers to the NAS failed midway"),
            }
        }
        self.orphans.retain(jobs::Group::is_the_jobs);
        let busy = self.slots.iter().any(|s| s.running.is_some()) || !self.orphans.is_empty();
        let Some(root) = root.filter(|_| !self.answers_seeded && !busy && !self.o.helper && self._lock.is_some() && !self.o.dry_run) else { return };
        self.answers_seeded = true;
        let (root, cache, scratch) = (root.to_path_buf(), self.o.home.join("cache"), self.o.home.join("scratch/answers"));
        match std::thread::Builder::new().name("answers".into()).spawn(move || crate::answers::seed(&root, &cache, &scratch)) {
            Ok(t) => self.answers_seed = Some(t),
            Err(e) => eprintln!("agent: the pass's answers not sent to the NAS now (its thread didn't start: {e})"),
        }
        // (One loop at a time, `--once`: waited for.)
        if self.o.once {
            while self.answers_seed.as_ref().is_some_and(|t| !t.is_finished()) {
                std::thread::sleep(Duration::from_millis(20));
            }
            self.seed_answers(None);
        }
    }

    /// Starts a trim or a clear (`ask`) on a thread of its own; one that can't start is a failure.
    fn caches_start(&mut self, ask: Option<room::ClearRequest>, work: impl FnOnce() -> Result<room::Freed> + Send + 'static) {
        match std::thread::Builder::new().name("caches".into()).spawn(work) {
            Ok(thread) => self.caches_task = Some(CachesTask { ask, began: Instant::now(), thread }),
            Err(e) => self.caches_failed(anyhow::anyhow!("its thread didn't start: {e}"), ask),
        }
    }

    /// Why nothing starts here now, while a trim or a clear of this Mac's caches runs.
    fn caches_busy(&self) -> Option<String> {
        let t = self.caches_task.as_ref()?;
        Some(format!("this Mac's caches are being {} ({} min so far): nothing starts here until that's done", if t.ask.is_some() { "cleared" } else { "trimmed" }, t.began.elapsed().as_secs() / 60))
    }

    /// A trim or a clear done (waited for, when it isn't yet), its result recorded. Asked to stop
    /// midway: nothing kept, the trim done again by the next agent, the ask left for it.
    fn caches_done(&mut self, t: CachesTask) {
        let r = t.thread.join().unwrap_or_else(|_| Err(anyhow::anyhow!("it failed midway")));
        if stopping() {
            return;
        }
        match r {
            Ok(f) => self.caches_record(f, t.ask),
            Err(e) => self.caches_failed(e, t.ask),
        }
    }

    /// A trim or a clear that failed: a clear's ask answered with why; a trim tried again in ten
    /// minutes.
    fn caches_failed(&mut self, e: anyhow::Error, ask: Option<room::ClearRequest>) {
        match ask {
            Some(ask) => self.caches_record(room::Freed { why_not: Some(format!("{e:#}")), ..Default::default() }, Some(ask)),
            None => {
                eprintln!("agent: trimming the caches: {e:#}; trying again in ten minutes");
                self.trim_failed = Some(Instant::now());
            }
        }
    }

    /// What a trim (`ask` None) or a clear did, or why a clear wasn't done: logged, kept for the
    /// status (a clear declined apart from the last one done), noted in the history (a trim only
    /// when it freed something or what it keeps changed), a clear's ask answered.
    fn caches_record(&mut self, mut f: room::Freed, ask: Option<room::ClearRequest>) {
        f.at = now_s();
        if let Some(a) = &ask {
            (f.asked, f.by) = (Some(a.at), Some(a.by.clone()));
        }
        let e = caches_event(&self.host, ask.is_none(), &f);
        eprintln!("agent: {}", e.note);
        if ask.is_some() || f.bytes() > 0 || f.left != self.mem.trimmed.as_ref().map_or(0, |t| t.left) {
            self.note(e);
        }
        let done = f.why_not.is_none();
        match (&ask, done) {
            (None, _) => self.mem.trimmed = Some(f),
            (Some(_), true) => self.mem.cleared = Some(f),
            (Some(_), false) => self.mem.declined = Some(f),
        }
        self.save();
        if ask.is_some() {
            room::clear_answered(&self.o.home);
        }
        if done {
            self.count_caches();
        }
    }

    /// This Mac's caches for the status (room::Caches), `why` they can't be cleared now: what a clear
    /// would free, as last counted, each cache with about how long it takes to come back at the
    /// NAS's speed here (measured: 60 MB/s on the LAN, 12 through Tailscale, plan §12).
    fn caches_view(&self, why: Option<String>, home: bool) -> room::Caches {
        let sizes = self.cache_size.lock().unwrap().1.clone();
        room::Caches {
            clearable: sizes.as_ref().map(|s| s.clear.values().sum()),
            each: sizes.map(|s| room::gone(&s.clear, if home { 60.0 } else { 12.0 })).unwrap_or_default(),
            why_not: why,
            trimmed: self.mem.trimmed.clone(),
            cleared: self.mem.cleared.clone(),
            declined: self.mem.declined.clone(),
        }
    }

    /// Why this Mac's caches can't be trimmed or cleared now, if they can't: a job runs here (it may
    /// read what would go), or one an earlier agent left (`orphans`), the NAS isn't reachable (what
    /// goes must be kept there), or the build has work left (`work_left`: by the build Mac's
    /// forecast, its own; a helper's, the one in the build Mac's heartbeat on the NAS, read at most
    /// each minute, with no job of the build Mac's running, nor beside it).
    fn caches_why_not(&mut self, root: Option<&Path>) -> Option<String> {
        if let Some(r) = self.slots.iter().find_map(|s| s.running.as_ref()) {
            return Some(format!("a job runs here ({})", r.spec.what));
        }
        self.orphans.retain(jobs::Group::is_the_jobs);
        if let Some(g) = self.orphans.first() {
            return Some(format!("a job an earlier agent left still runs here ({})", g.id));
        }
        let Some(root) = root else { return Some("the NAS isn't reachable".into()) };
        let since = self.mem.worked_at;
        if !self.o.helper {
            return work_left(self.forecast.borrow().as_ref(), since);
        }
        if self.heard.as_ref().is_none_or(|h| h.read.elapsed() >= Duration::from_secs(60)) {
            let st: Option<Status> = std::fs::read(root.join("state/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
            let left = st.as_ref().map(|s| match s.job.as_ref().or(s.beside.as_ref()) {
                Some(j) => Some(format!("the build Mac runs a job ({})", j.what)),
                None => work_left(s.forecast.as_ref(), since),
            });
            self.heard = Some(Heard { read: Instant::now(), beat: st.map(|s| s.beat), left: left.flatten() });
        }
        match self.heard.as_ref().map(|h| (h.beat, &h.left)) {
            Some((Some(beat), _)) if now_s().saturating_sub(beat) > 600 => Some(format!("the build Mac hasn't been heard from for {} min", now_s().saturating_sub(beat) / 60)),
            Some((Some(_), left)) => left.clone(),
            _ => Some("the build Mac's status can't be read now".into()),
        }
    }

    /// Whether this Mac's caches are due a trim: never trimmed nor cleared, or a job has run here
    /// since the last.
    fn trim_due(&self) -> bool {
        let last = [&self.mem.trimmed, &self.mem.cleared].into_iter().flatten().map(|f| f.at).max();
        last.is_none_or(|t| self.mem.worked_at > t)
    }

    /// A job of `id` ran here: this Mac's caches may hold more, to trim once the build is done
    /// again (not after the daily ones: they read no cache); the build Mac's heartbeat read again.
    fn worked(&mut self, id: &str) {
        if !matches!(step_of(id).as_deref(), Some("backup" | "gc")) {
            self.mem.worked_at = now_s();
            self.heard = None;
        }
    }

    /// Counts this Mac's caches again now (after a trim or a clear), for the status, and what a
    /// helper's can free before it next asks for work. (Not asking the NAS anything: on the loop, a
    /// NAS hanging then would hold it; links aside, the counting thread's checks are its.)
    fn count_caches(&mut self) {
        let n = room::sizes(&self.o.home.join("cache"), self.o.helper, None);
        *self.cache_size.lock().unwrap() = (Some(Instant::now()), Some(n));
        self.cheap = None;
    }

    /// The build Mac's: the helpers' trims, clears and declined asks noted in the history as their
    /// statuses show them, each once (those from before this agent started were noted by the one
    /// before); a trim only when it freed something or what it keeps changed.
    fn note_helpers_caches(&mut self, helpers: &[Status]) {
        if self.coord.is_none() {
            return;
        }
        for h in helpers {
            let Some(c) = &h.caches else { continue };
            let (seen, mut kept) = *self.helper_caches.entry(h.host.clone()).or_insert((self.started, 0));
            let mut new: Vec<(bool, &room::Freed)> = [(true, &c.trimmed), (false, &c.cleared), (false, &c.declined)].into_iter().filter_map(|(t, f)| Some((t, f.as_ref().filter(|f| f.at > seen)?))).collect();
            new.sort_by_key(|n| n.1.at);
            for (trimmed, f) in new {
                if !trimmed || f.bytes() > 0 || f.left != kept {
                    self.note(caches_event(&h.host, trimmed, f));
                }
                if trimmed {
                    kept = f.left;
                }
                self.helper_caches.insert(h.host.clone(), (f.at, kept));
            }
        }
    }

    fn start(&mut self, k: usize, mut spec: JobSpec, c: &Conditions) -> Result<()> {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        // Half the cores while the user is active, all of them when away (a helper, two fewer:
        // its Mac has less memory, and its user's work comes first). The second job: four for one
        // that mostly waits on the network, else half (the two jobs' threads share the cores).
        let step = step_of(&spec.id).unwrap_or_default();
        let threads = match (k, c.user_active(), self.o.helper) {
            (1.., _, _) if LIGHT.contains(&step.as_str()) => 4.min(cores),
            (1.., _, _) => (cores / 2).max(1),
            (_, true, _) => (cores / 2).max(1),
            (_, false, true) => cores.saturating_sub(2).max(1),
            (_, false, false) => cores,
        };
        // The second job's scratch folder its own (a step's work in it, wiped by its next run).
        if k > 0 {
            let first = self.o.home.join("scratch");
            let mine = self.o.home.join(format!("scratch-{}", k + 1));
            if let Some(i) = spec.cmd.iter().position(|a| a == "--scratch") {
                if let Some(p) = spec.cmd.get_mut(i + 1) {
                    if let Ok(rest) = Path::new(p.as_str()).strip_prefix(&first) {
                        *p = mine.join(rest).to_string_lossy().into_owned();
                    }
                }
            }
        }
        let log = self.o.home.join("logs").join(format!("{}.log", spec.id.replace([' ', '/'], "-")));
        eprintln!("agent: starting {}{} ({threads} threads)", spec.id, if k > 0 { " beside the first job" } else { "" });
        // The build Mac's jobs save the records (crate::out::Out::save trusts them by this, whatever
        // this Mac is named now), note what their units cost, and may offer tasks to other workers
        // through the coordinator; a helper's hand their saves off (SCENIC_HANDOFF, in their command).
        let mut env: Vec<(String, String)> = Vec::new();
        if !self.o.helper {
            env.push(("SCENIC_BUILD_MAC".into(), "1".into()));
            env.push(("SCENIC_COSTS".into(), self.costs_path(k).to_string_lossy().into_owned()));
            if let Some(c) = &self.coord {
                env.push(("SCENIC_COORD".into(), format!("http://127.0.0.1:{}", crate::coord::PORT)));
                env.push(("SCENIC_COORD_TOKEN".into(), c.job_token.clone()));
            }
            // A round's step that reads the units: as they were when the round began.
            let step = spec.record.as_ref().map(|w| w.step.as_str()).unwrap_or("");
            if let Some(r) = self.round.borrow().as_ref().filter(|r| !r.over && build::AS_OF_STEPS.contains(&step)) {
                env.push((crate::out::UNITS_AS_OF_ENV.into(), format!("{}#{}", self.o.home.join(ROUND_FILE).display(), r.began)));
            }
        }
        // Its channel, to stop at a safe point when the build pauses, and where it notes each target
        // done (crate::control), afresh.
        let control = self.control_path(k);
        std::fs::write(&control, b"run").with_context(|| format!("write {}", control.display()))?;
        let done = self.done_path(k);
        std::fs::remove_file(&done).ok();
        self.slots[k].drain_since = None;
        env.push((crate::control::CONTROL_ENV.into(), control.to_string_lossy().into_owned()));
        env.push((crate::control::DONE_ENV.into(), done.to_string_lossy().into_owned()));
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (step, targets) = spec.record.as_ref().map_or((spec.id.split(' ').next().map(str::to_string), Vec::new()), |w| (Some(w.step.clone()), w.targets.iter().map(|t| t.0.clone()).collect()));
        let what = spec.what.clone();
        self.slots[k].running = Some(Running::start(spec, threads, &env, log, &self.record_path(k), Some(&done))?);
        self.slots[k].beaten = None;
        self.note(crate::coord::history::Event { worker: Some(self.worker_of(k)), step, targets, what, ..crate::coord::history::Event::new("start") });
        Ok(())
    }

    /// Notes what happened in the build's history (the build Mac's coordinator's: a helper's jobs
    /// are there by their leases).
    fn note(&self, e: crate::coord::history::Event) {
        if let Some(c) = &self.coord {
            c.note(e);
        }
    }

    /// Notes slot `k`'s job's end: its step, the targets it finished, its time, and how it ended
    /// (`how`: empty when it succeeded).
    fn note_end(&self, k: usize, step: &str, done: &[(String, String)], secs: u64, ok: bool, how: &str) {
        // (A job with no record of its own, the pass's or the daily ones, by its id's first word.)
        let (id, what) = self.slots[k].running.as_ref().map_or((String::new(), String::new()), |r| (r.spec.id.clone(), r.spec.what.clone()));
        let step = if step.is_empty() { id.split(' ').next().unwrap_or("").to_string() } else { step.to_string() };
        self.note(crate::coord::history::Event { worker: Some(self.worker_of(k)), step: (!step.is_empty()).then_some(step), targets: done.iter().map(|t| t.0.clone()).collect(), secs: Some(secs as f64), ok: Some(ok), what, note: how.to_string(), ..crate::coord::history::Event::new("end") });
    }

    /// Notes the build Mac's conditions as they change (mains or battery, the NAS, home or away, a
    /// sleep), for the history.
    fn note_conditions(&mut self, c: &Conditions, slept: u64) {
        let mut said: Vec<String> = Vec::new();
        if slept > 60 {
            said.push(format!("slept {} min", slept / 60));
        }
        if let Some(was) = self.last_cond {
            if was.ac != c.ac {
                said.push(if c.ac { "on mains power".into() } else { format!("on battery{}", c.battery.map(|b| format!(" ({b}%)")).unwrap_or_default()) });
            }
            if was.nas != c.nas {
                said.push(if c.nas { "the NAS answers again".into() } else { "the NAS doesn't answer".into() });
            }
            if was.home != c.home {
                said.push(if c.home { "home".into() } else { "away from home (the NAS through Tailscale)".into() });
            }
        }
        self.last_cond = Some(*c);
        if !said.is_empty() {
            self.note(crate::coord::history::Event { worker: Some(self.host.clone()), note: said.join("; "), ..crate::coord::history::Event::new("conditions") });
        }
    }

    fn finished(&mut self, id: &str, what: &str, ok: bool, secs: u64, note: String) {
        // (One that ran: not one that couldn't start.)
        if secs > 0 {
            self.worked(id);
        }
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

    /// Whether this agent keeps the build's rounds (and its history): the one that runs its jobs,
    /// not a dry run beside it (planning only: its rounds are its own, in memory).
    fn keeps_rounds(&self) -> bool {
        self._lock.is_some() && !self.o.dry_run
    }

    /// A round begun (build::Plan::begins, its `began` set): written to its file, which its jobs read,
    /// before any of them starts.
    fn keep_round(&self, r: build::Round) -> Result<()> {
        if self.keeps_rounds() {
            crate::whole::write(&self.o.home.join(ROUND_FILE), &serde_json::to_vec(&r)?)?;
            self.note(crate::coord::history::Event { targets: r.regions.clone(), note: if r.last { "the last".into() } else { String::new() }, ..crate::coord::history::Event::new("round") });
        }
        eprintln!("agent: a round begins, publishing {} region(s){}", r.regions.len(), if r.last { " (the last)" } else { "" });
        *self.round.borrow_mut() = Some(r);
        Ok(())
    }

    /// The round under way is over (its catalog made, or nothing left it would publish): kept
    /// without its units, for when it began.
    fn end_round(&self) {
        let mut kept = self.round.borrow_mut();
        let Some(r) = kept.as_mut().filter(|r| !r.over) else { return };
        r.over = true;
        r.units.clear();
        if self.keeps_rounds() {
            if let Err(e) = serde_json::to_vec(&*r).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&self.o.home.join(ROUND_FILE), &b)) {
                eprintln!("agent: noting the round over: {e:#}");
            }
        }
        eprintln!("agent: the round that began {} min ago is over", now_s().saturating_sub(r.began) / 60);
    }

    /// The work there is, in order (docs/plan.md §8, Order).
    fn plan(&self, root: &Path, _c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let mut out = Vec::new();
        let build = self.o.bin.join("scenic-build");
        let me = self.o.bin.join("scenic");
        let s = |p: &Path| p.to_string_lossy().into_owned();

        // 1. The OSM pass, when the NAS holds a newer planet than the last complete pass.
        let have = crate::osmpass::latest_pass(root);
        let newer = crate::osmpass::newer_planet(root, have.as_deref()).ok().flatten();
        if let Some((planet, date)) = newer.clone() {
            let what = format!("Reading the OpenStreetMap planet of {date}");
            let scratch = self.o.home.join("scratch").join(format!("osm-{date}"));
            let jar = root.join("sources/basemap/planetiler.jar");
            // The pack cache is cleared when the pass starts (it refills from the mirror or the NAS):
            // its space counts as free, and so does the cheap caches' (room::make_room frees it).
            let pack_cache = self.o.home.join("cache").join("base");
            let mut free = cond::free_bytes(&self.o.home).unwrap_or(0) + dir_bytes(&pack_cache);
            if free < PASS_SPACE {
                free += room::cheap_bytes(&self.o.home.join("cache"), Some(&root.join("sources")));
            }
            let started = scratch.exists();
            if !jar.exists() {
                waiting.push(Waiting { step: Some("osm-pass".into()), what, why: "sources/basemap/planetiler.jar is missing on the NAS".into() });
            } else if !started && free < PASS_SPACE {
                waiting.push(Waiting { step: Some("osm-pass".into()), what, why: format!("needs {} GB free on this Mac ({} GB free)", PASS_SPACE >> 30, free >> 30) });
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
                    needs: Needs { cpu: true, nas: true, home: true },
                    restart_after_sleep: true,
                    record: None,
                });
            }
        }

        // The regions: terrain and slope near the coverage, base(U), pack(T), lo, a catalog.
        out.extend(self.region_work(root, have.as_deref(), newer.as_ref().map(|n| n.1.as_str()), waiting));

        // Daily: the user's folders backed up, replaced files removed.
        if self.due("backup", Duration::from_secs(86400)) {
            out.push(JobSpec {
                id: "backup".into(),
                what: "Backing up translations, descriptions and inputs".into(),
                cmd: vec![s(&me), "backup".into(), "--root".into(), s(root), "--local".into(), s(&self.o.home.join("backups"))],
                needs: Needs { cpu: false, nas: true, home: false },
                restart_after_sleep: true,
                record: None,
            });
        }
        // (Not while a round is under way: the units it reads as they were may be gone from the
        // manifest, and in no catalog yet.)
        if self.due("gc", Duration::from_secs(86400)) && self.round.borrow().as_ref().is_none_or(|r| r.over) {
            out.push(JobSpec {
                id: "gc".into(),
                what: "Removing replaced files from the NAS".into(),
                cmd: vec![s(&me), "gc".into(), "--root".into(), s(root)],
                needs: Needs { cpu: false, nas: true, home: false },
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
    fn region_work(&self, root: &Path, pass: Option<&str>, newer: Option<&str>, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let (recipes, unread) = recipes::load(&root.join("inputs/regions"));
        // (The records unreadable now: nothing planned until they are, rather than everything again.)
        let (manifest, keys): (BTreeMap<String, String>, build::Keys) = match crate::out::read_record(&root.join("state/build/manifest.json")).and_then(|m| Ok((m, self.planning_keys(root)?))) {
            Ok(r) => r,
            Err(e) => {
                waiting.push(Waiting { step: None, what: "Building".into(), why: format!("the build's records can't be read now: {e:#}") });
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
            JobSpec { id, what: what.into(), cmd, needs: Needs { cpu: true, nas: true, home }, restart_after_sleep: true, record }
        };
        // Per pass, worldwide: the sets it lacks in their current filters (a set added or changed
        // since it ran), the hiking routes' ends, AWS's z8 (once), Overture's buildings (once per
        // release), the summits, the labels.
        if let Some(date) = pass {
            let p = vec!["--pass".to_string(), date.to_string()];
            if !crate::osmpass::SETS.iter().all(|st| manifest.contains_key(&crate::osmpass::set_name(date, st.0))) {
                jobs.push(job(format!("pass-sets {date}"), "Cutting the OpenStreetMap sets the newest pass lacks", "pass-sets", p.clone(), None));
            }
            if let Some(w) = build::trailends_work(date, &manifest, &keys) {
                jobs.push(job(format!("trailends {date}"), "Finding the ends of the world's hiking routes", "trailends", p.clone(), Some(w)));
            }
            let reach_job = |w: build::Work| job(format!("reach {date}"), "Working out how far each area's roads reach, worldwide", "reach", p.clone(), Some(w));
            if let Some(w) = build::reach_work(date, &manifest, &keys) {
                jobs.push(reach_job(w));
            } else if let Err(why) = self.current_reach(root, &manifest, &keys, date) {
                match why {
                    // Made, but it doesn't decode: made again.
                    crate::reach::LoadError::Bad(e) => {
                        waiting.push(Waiting { step: None, what: "Building the areas".into(), why: format!("the pass's reaches don't read ({e}); making them again") });
                        if let Some(k) = build::reach_key(date, &manifest) {
                            jobs.push(reach_job(build::Work { step: "reach".into(), targets: vec![("reach".into(), k)] }));
                        }
                    }
                    crate::reach::LoadError::Io(e) => waiting.push(Waiting { step: None, what: "Building the areas".into(), why: format!("the pass's reaches can't be read now: {e}") }),
                }
            }
            if !manifest.contains_key(&crate::terrain_z8::logical()) {
                jobs.push(job("terrain-z8".into(), "Building coarse terrain for the whole world", "terrain-z8", vec!["--raw".into(), s(&self.o.home.join("cache").join("aws-terrarium"))], None));
            }
            if !manifest.contains_key(&crate::buildtiles::index_logical()) {
                jobs.push(job(format!("buildings {}", crate::buildtiles::RELEASE), "Collecting the world's roadside buildings (Overture)", "buildings", vec!["--dem".into(), s(&self.o.bin.join("dem"))], None));
            } else if let Ok(rd) = std::fs::read_dir(self.o.home.join("scratch/buildings")) {
                // Made: the scan's parts (~40 GB) go, even from a run stopped before it removed them.
                for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("parts-")) {
                    std::fs::remove_dir_all(e.path()).ok();
                }
            }
            if let Some(w) = build::summits_work(date, &manifest, &keys) {
                jobs.push(job(format!("summits {date}"), "Finding the world's summits", "summits", [p.clone(), vec!["--cache".into(), s(&self.o.home.join("cache"))]].concat(), Some(w)));
            }
            if let Some(w) = build::labels_work(date, &manifest, &keys) {
                jobs.push(job(format!("labels {date}"), "Ranking the world's place labels", "labels", [p.clone(), vec!["--dem".into(), s(&self.o.bin.join("dem"))]].concat(), Some(w)));
            }
        }
        if recipes.is_empty() {
            return jobs;
        }
        let Some(date) = pass else {
            waiting.push(Waiting { step: None, what: "Building the regions".into(), why: "the first OpenStreetMap pass (it makes the outlines regions are drawn from)".into() });
            return jobs;
        };
        // (The pass's worldwide jobs: the forecast's before the regions'.)
        let before = jobs.clone();
        let covs = match self.coverage(root, &manifest, date, &recipes, unread.is_empty()) {
            Ok(c) => c,
            Err(why) => {
                waiting.push(Waiting { step: None, what: "Building the regions".into(), why });
                return jobs;
            }
        };
        let cov = &covs.all;
        let done = keys;
        let inputs = input_digests(root);
        let held = root.join("inputs/hold-catalog").exists();
        let reach = self.current_reach(root, &manifest, &done, date).ok().flatten();
        if !manifest.contains_key(crate::rail::CATALOGUE) {
            waiting.push(Waiting { step: None, what: build::TRAINS.into(), why: "the rail sources aren't on the NAS yet (scenic-build rail-seed)".into() });
        } else if inputs.get("keys").map(String::as_str) == Some("?") {
            waiting.push(Waiting { step: None, what: build::TRAINS.into(), why: "inputs/keys.env can't be read now".into() });
        }
        // The regions the map's catalog has (or the held one's, when catalogs are held for review)
        // and how long ago it went out: what the plan publishes regions by.
        // (The held ones, once there are any: the first goes by what's served.)
        let held_dir = root.join("catalog-held");
        let dir = if held && store::catalog::list(&held_dir).is_ok_and(|ns| !ns.is_empty()) { held_dir } else { root.join("catalog") };
        let (on_map, since_publish) = self.on_map(&dir, &recipes);
        // (The map's: the served catalog's, held ones aside.)
        self.catalog_seen.set(served_catalog(&root.join("catalog")));
        // The round under way, if one is; the next an hour after the last began (before the agent
        // kept rounds, after the last catalog went out).
        let since_last = self.round.borrow().as_ref().map(|r| now_s().saturating_sub(r.began)).or(since_publish);
        let mut planned = {
            let kept = self.round.borrow();
            build::plan(&cov, date, &manifest, &done, &inputs, reach.as_deref(), build::Rounds { each: &covs.each, on_map: &on_map, since_last, current: kept.as_ref().filter(|r| !r.over), held })
        };
        let edit_hold = edit_held(self.edited_at.get());
        // (Hand-offs of work done not yet merged: counted as built, their files not yet in the
        // manifest.)
        let unmerged = self.handoff_bases(root).iter().any(|b| crate::handoff::waiting_in(b).map(|w| w.iter().any(|(_, h)| h.done.is_some())).unwrap_or(false));
        if planned.ends {
            self.end_round();
        }
        // A round begins: kept from now on, its work planned as such already. Not while a helper's
        // work waits to be merged (the units it counts as built would be missing from the round's),
        // nor while an edit is held: its own steps wait meanwhile.
        if let Some(mut r) = planned.begins.take() {
            let not_now = if unmerged {
                Some("waits for a helper's work to be merged".to_string())
            } else if edit_hold.is_some() {
                Some("waits for the regions' edits to settle".to_string())
            } else {
                r.began = now_s();
                self.keep_round(r).err().map(|e| format!("can't keep the round now ({e:#})"))
            };
            if let Some(why) = not_now {
                waiting.push(Waiting { step: Some("catalog".into()), what: build::PUBLISH.into(), why: format!("the next round {why}") });
                planned.work.retain(|w| !build::AS_OF_STEPS.contains(&w.step.as_str()) && w.step != "prune");
            }
        }
        // (The round under way's chain left, and when it began, for the forecast: one begun now,
        // now.)
        let round_left = std::mem::take(&mut planned.round_left);
        let since_last = self.round.borrow().as_ref().map(|r| now_s().saturating_sub(r.began)).or(since_last);
        let ready = planned.ready;
        *self.ready.borrow_mut() = ready.clone();
        // (As the catalog's `--ready`: each with the outline it was built with.)
        let ready_arg: String = ready.iter().map(|id| match recipes.iter().find(|r| &r.id == id) {
            Some(r) => format!("{id}={}", recipes::outline_digest(&r.outline)),
            None => id.clone(),
        }).collect::<Vec<_>>().join(",");
        let mut plan = planned.work;
        // Just edited: the regions' work waits a while for more edits (each would build again what
        // the last started); what runs carries on.
        if let Some((age, left)) = edit_hold.filter(|_| !plan.is_empty()) {
            let left = left.as_secs().div_ceil(60);
            waiting.push(Waiting { step: None, what: "Building the regions".into(), why: format!("a region's recipe or outline changed {} min ago: their work starts in {left} min, after any more edits", age.as_secs() / 60) });
            plan.clear();
        }
        // A round's catalog waits while another worker builds its regions' slope or tree cover (it
        // would go out without them, and they'd wait an hour), and while a helper's hand-offs wait
        // to be merged (its areas counted as built, their files not yet in the manifest).
        let held_by_others = |step: &str, t: &str| self.coord.as_ref().is_some_and(|c| c.held(step).contains(t)) || claims::others(root, step, &self.me).contains(t);
        let held_wait = planned.publish_waits.iter().find(|(st, t)| held_by_others(st, t));
        if let Some((st, t)) = held_wait.filter(|_| plan.iter().any(|w| w.step == "catalog")) {
            let mine = self.slots.iter().filter_map(|s| s.running.as_ref()).any(|r| r.spec.record.as_ref().is_some_and(|w| w.step == *st && w.targets.iter().any(|x| x.0 == *t)));
            let who = if mine { "this Mac's job" } else { "another worker" };
            waiting.push(Waiting { step: Some("catalog".into()), what: build::PUBLISH.into(), why: format!("waits for {who} building the {} of {t}", if st == "slope" { "slope" } else { "tree cover" }) });
            plan.retain(|w| w.step != "catalog");
        } else if unmerged && plan.iter().any(|w| w.step == "catalog") {
            waiting.push(Waiting { step: Some("catalog".into()), what: build::PUBLISH.into(), why: "waits for a helper's work to be merged".into() });
            plan.retain(|w| w.step != "catalog");
        }
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
        let mut offers: Vec<crate::coord::Offer> = Vec::new();
        // (A terrain area's z6 tiles near the coverage, for its run's expected memory: only when
        // there's terrain to offer.)
        let z6: BTreeMap<String, usize> = if plan.iter().any(|w| w.step == "terrain") {
            build::coverage_tiles(cov).into_iter().map(|(q, ts)| (format!("3/{}/{}", q.0, q.1), ts.len())).collect()
        } else {
            BTreeMap::new()
        };
        for w in plan.iter_mut().filter(|w| claims::SHARED.contains(&w.step.as_str())) {
            // What another worker builds now isn't planned here: what it leased from this Mac's
            // coordinator, or claimed (a helper on an app from before it).
            let mut others = claims::others(root, &w.step, &self.me);
            if let Some(c) = &self.coord {
                others.extend(c.held(&w.step));
            }
            w.targets.retain(|t| !others.contains(&t.0));
            // The rest, offered to the workers that mount the NAS: units with their pieces' sizes,
            // the others with the memory their jobs are expected to take (until one's run says);
            // each worker takes what fits its memory.
            let guess = |t: &str| match w.step.as_str() {
                "unit" | "pois" => size(t),
                "terrain" => terrain_peak(z6.get(t).copied().unwrap_or(64)),
                s => first_peak(s),
            };
            offers.push(crate::coord::Offer { step: w.step.clone(), targets: w.targets.iter().map(|(t, k)| (t.clone(), k.clone(), guess(t))).collect(), batch: batch_size(&w.step) });
        }
        // A step's targets offered together, in plan order (the plan lists a step's work by region:
        // a helper takes from the far end of all of it). (A step with nothing left: none offered.)
        let mut merged: Vec<crate::coord::Offer> = Vec::new();
        for o in offers {
            match merged.iter_mut().find(|m| m.step == o.step) {
                Some(m) => m.targets.extend(o.targets),
                None => merged.push(o),
            }
        }
        let mut offers = merged;
        offers.sort_by_key(|o| claims::SHARED.iter().position(|s| *s == o.step));
        if let Some(c) = &self.coord {
            c.offer(date, offers);
        }
        // The forecast of the work left (crate::agent::forecast): the build Mac's, for the status (made
        // again at most each minute: idle, the plan's made each loop).
        let stale = self.forecast.borrow().as_ref().is_none_or(|f| now_s().saturating_sub(f.at) >= 60);
        if !self.o.helper && stale {
            // (What can't be listed now: the regions' work after a new pass, or while the units wait.)
            let blind = match newer {
                Some(d) => Some(format!("a new OpenStreetMap pass (the planet of {d}) comes first; the regions' work is known once it's done")),
                None if planned.regions.is_empty() => Some("the areas wait for the pass's heritage sites, reaches and roadside buildings; what the regions need is known once they're made".to_string()),
                None => None,
            };
            let z6: BTreeMap<String, usize> = build::coverage_tiles(cov).into_iter().map(|(q, ts)| (format!("3/{}/{}", q.0, q.1), ts.len())).collect();
            let peak = |step: &str, t: &str| match step {
                "unit" | "pois" => crate::coord::unit_peak(&BTreeMap::new(), t, size(t)),
                "terrain" => terrain_peak(z6.get(t).copied().unwrap_or(64)),
                s => first_peak(s),
            };
            let mut before = before;
            before.extend(plan.iter().filter(|w| w.step == "heritage-sites").map(|w| job(format!("heritage-sites {date}"), "", "heritage-sites", Vec::new(), Some(w.clone()))));
            let chains = build::chains_left(cov, date, &manifest, &done, &inputs, reach.as_deref());
            // (A fault in it costs the status its forecast, never the agent.)
            let under_way = self.round.borrow().as_ref().filter(|r| !r.over).map(|r| (r.regions.clone(), r.last, round_left.clone()));
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.forecast_now(root, &before, &planned.regions, chains, since_last, under_way, blind, &peak)));
            match made {
                Ok(f) => *self.forecast.borrow_mut() = Some(f),
                Err(_) => {
                    eprintln!("agent: the forecast failed; the status goes without it");
                    *self.forecast.borrow_mut() = None;
                }
            }
        }
        plan.retain(|w| !w.targets.is_empty());
        for (w, total) in batches(plan) {
            // Held for review: the catalog goes to catalog-held/ (no server reads it), once.
            if w.step == "catalog" && held {
                let k = w.targets.first().map(|t| t.1.clone()).unwrap_or_default();
                if done.catalog_held.as_deref() == Some(k.as_str()) {
                    waiting.push(Waiting { step: None, what: build::PUBLISH.into(), why: "held for review (inputs/hold-catalog); its catalog is in catalog-held/".into() });
                    // (Its round's done: the held catalog is its.)
                    self.end_round();
                    continue;
                }
                let mut j = job("catalog-held".into(), "Publishing the new map data, held for review", "catalog", vec!["--held".into(), "--ready".into(), ready_arg.clone()], Some(build::Work { step: "catalog-held".into(), targets: vec![("catalog-held".into(), k)] }));
                j.needs = Needs { cpu: false, nas: true, home: false };
                jobs.push(j);
                continue;
            }
            let mut extra: Vec<String> = w.targets.iter().map(|t| t.0.clone()).filter(|t| !matches!(t.as_str(), "catalog" | "items" | "marks" | "roadunits" | "stations" | "ferries" | "heritage-sites" | "heritage" | "overlays" | "rail-feeds" | "rail") && !t.ends_with("-root")).collect();
            extra.extend(self.step_args(&w.step, date));
            // (The regions a catalog records as built: the plan's.)
            if w.step == "catalog" {
                extra.extend(["--ready".to_string(), ready_arg.clone()]);
            }
            let n = w.targets.len();
            // "3 areas", or "8 of 480 areas" for a batch.
            let areas = if n == total { format!("{n} area{}", if n == 1 { "" } else { "s" }) } else { format!("{n} of {total} areas") };
            // (The checklist names the steps alike: build::label.)
            let base = build::label(&w.step);
            let what = match w.step.as_str() {
                "terrain" | "slope" | "unit" | "pois" | "peaks" | "pack" => format!("{base} ({areas})"),
                "trees" => format!("{base} ({})", areas.replace("area", "large tile")),
                _ => base.to_string(),
            };
            let id = format!("{} {}", w.step, w.targets.first().map(|t| t.0.as_str()).unwrap_or(""));
            let step = w.step.clone();
            let mut j = job(id, &what, &step, extra, Some(w));
            // (A catalog and a prune only write a little: no power needed.)
            j.needs = Needs { cpu: !matches!(step.as_str(), "catalog" | "prune"), nas: true, home: false };
            jobs.push(j);
        }
        jobs
    }

    /// The forecast (crate::agent::forecast) of the work left: `before`, the build Mac's jobs before
    /// the regions'; `regions`, the plan's; `chains`, the roads', trains' and landmarks' work to come
    /// (build::chains_left); `blind`, why the work can't all be listed now. Each target's time (at
    /// the build Mac's pace: a time measured on a helper over its speed) and memory as last measured,
    /// else its step's mean or a first guess (`first_secs`; `peak` for its memory); the helpers at
    /// their measured speed; each machine free once its job under way is done.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn forecast_now(&self, root: &Path, before: &[JobSpec], regions: &[build::RegionLeft], chains: [Vec<build::Work>; 3], since_last: Option<u64>, under_way: Option<(Vec<String>, bool, Vec<build::Work>)>, blind: Option<String>, peak: &dyn Fn(&str, &str) -> u64) -> forecast::Forecast {
        use forecast::{Cost, Machine};
        let (costs, leased, events, mem) = match &self.coord {
            Some(c) => c.for_forecast(),
            None => Default::default(),
        };
        // Each worker's speed against the build Mac's, from the history (half until it's measured):
        // every one that measured a cost or did shared work, asleep or not (its costs are put at the
        // build Mac's pace by it); the helpers heard from lately are the machines.
        let helpers: Vec<Status> = helpers(root);
        let mut names: BTreeSet<String> = helpers.iter().map(|h| h.host.clone()).collect();
        names.extend(costs.values().filter_map(|c| c.worker.clone()));
        names.extend(events.iter().filter(|e| matches!(e.kind.as_str(), "done" | "lease")).filter_map(|e| e.worker.clone()));
        names.remove(&self.host);
        // (The second job: its units beside the first's, at about four fifths of its pace until
        // measured.)
        let second = self.worker_of(1);
        names.remove(&second);
        let names: Vec<String> = names.into_iter().collect();
        let mut speeds = forecast::speeds(&events, &self.host, &names, 0.5);
        speeds.extend(forecast::speeds(&events, &self.host, std::slice::from_ref(&second), 0.8));
        // (A time measured on a helper, at the build Mac's pace; one measured here or by a page, as it is.)
        let pace = |worker: Option<&String>| worker.and_then(|w| speeds.get(w)).map_or(1.0, |s| s.0);
        // Each shared step's mean of the targets measured (a unit's are kept by its target alone);
        // else what its jobs here took a target; else a first guess.
        let mut sums: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        for (k, c) in &costs {
            let step = k.split_once(' ').map_or("unit", |(s, _)| s);
            let e = sums.entry(step.to_string()).or_default();
            (e.0, e.1) = (e.0 + c.secs as f64 * pace(c.worker.as_ref()), e.1 + 1);
        }
        let per = |step: &str| -> f64 {
            match (sums.get(step).filter(|s| s.1 > 0), self.mem.step_secs.get(step)) {
                (Some(s), _) => s.0 / s.1 as f64,
                (None, Some(&t)) => t,
                (None, None) => first_secs(step),
            }
        };
        let cost = |step: &str, t: &str| -> Cost {
            match costs.get(&crate::coord::cost_key(step, t)) {
                // (Its memory only when measured the way the step runs now: crate::coord::cost_version.)
                Some(c) => Cost { secs: c.secs as f64 * pace(c.worker.as_ref()), known: true, peak_mb: if c.v >= crate::coord::cost_version(step) { c.peak_mb } else { peak(step, t) } },
                None => Cost { secs: per(step), known: false, peak_mb: peak(step, t) },
            }
        };
        // The build Mac's own steps: what a target took here lately (measured), else a first guess.
        let mine = |step: &str, n: usize| -> Cost {
            let (each, known) = self.mem.step_secs.get(step).map_or((first_secs(step), false), |&t| (t, true));
            Cost { secs: each * n.max(1) as f64, known, peak_mb: 0 }
        };
        let step_of = |id: &str| id.split(' ').next().unwrap_or("").to_string();
        // (Each job before the regions' is a step of its own; one running now is its slot's
        // running item below, its time left its slot's.)
        let before: Vec<forecast::Job> = before.iter().map(|j| (step_of(&j.id), j.id.clone(), mine(&step_of(&j.id), j.record.as_ref().map_or(1, |w| w.targets.len())))).collect();
        // A round: as the last ones took (their chains' jobs and catalog), else the chain's steps'
        // times; the last round, the roads' chain as it stands now, if more (none when it's done),
        // less what the round under way still does (the roads' chain counts its work too, which
        // goes out with it).
        let [roads, rail, landmarks] = chains;
        let chain_s = |works: &[build::Work]| -> f64 { works.iter().map(|w| if claims::SHARED.contains(&w.step.as_str()) { w.targets.iter().map(|t| cost(&w.step, &t.0).secs).sum() } else { mine(&w.step, w.targets.len()).secs }).sum() };
        let round_s = forecast::round_secs(&events).unwrap_or_else(|| ["prune", "roadunits", "stations", "ferries", "terrain-root", "slope-root", "catalog"].iter().map(|s| mine(s, 1).secs).sum::<f64>() + mine("pack", 8).secs + mine("lo", 2).secs);
        let ahead = under_way.as_ref().map_or(0.0, |u| chain_s(&u.2));
        let last_round_s = if roads.is_empty() { 0.0 } else { round_s.max(chain_s(&roads) - ahead) };
        // The trains' and the landmarks' chains from the start (their steps once what they read is
        // built: forecast::chain_deps), but the overlays (they read the built units) after the last
        // round; and a catalog after it with what the chains made since.
        let (mut chain_jobs, mut after): (Vec<forecast::Job>, Vec<forecast::Job>) = (Vec::new(), Vec::new());
        for w in rail.iter().chain(landmarks.iter()) {
            let jobs: Vec<forecast::Job> = if claims::SHARED.contains(&w.step.as_str()) {
                w.targets.iter().map(|t| (w.step.clone(), t.0.clone(), cost(&w.step, &t.0))).collect()
            } else {
                vec![(w.step.clone(), w.targets.first().map(|t| t.0.clone()).unwrap_or_default(), mine(&w.step, w.targets.len()))]
            };
            if w.step == "overlays" { after.extend(jobs) } else { chain_jobs.extend(jobs) }
        }
        if !rail.is_empty() || !landmarks.is_empty() {
            after.push(("catalog".into(), "after the chains".into(), mine("catalog", 1)));
        }
        // Each slot's job's time left: its pace says only its part's; its targets not yet done, as
        // they took last time, less what it spent on the one under way, if longer.
        let busy = |k: usize, speed: f64| {
            self.slots[k].running.as_ref().map_or(0.0, |r| {
                let left = match r.spec.record.as_ref() {
                    Some(w) => {
                        let done = crate::control::read_done(&self.done_path(k), &w.step);
                        let (mut todo, mut did) = (0.0, 0.0);
                        for (t, _) in &w.targets {
                            let c = cost(&w.step, t).secs;
                            if done.contains(t) { did += c } else { todo += c }
                        }
                        todo - (r.elapsed().as_secs_f64() - did).max(0.0)
                    }
                    // (One with no record, the pass's or a worldwide one: its step's time here.)
                    None => mine(&step_of(&r.spec.id), 1).secs - r.elapsed().as_secs_f64(),
                };
                // (At its own pace.)
                self.slots[k].job_eta.map(|e| e as f64).into_iter().chain([left / speed.max(0.1)]).fold(60.0, f64::max)
            })
        };
        let mut machines = vec![Machine { name: self.host.clone(), speed: 1.0, measured: true, helper: false, second: false, light_s: 0.0, mem_mb: u64::MAX, busy_s: busy(0, 1.0) }];
        // Its second job: what fits beside the first (a quarter of its memory, say); while the Mac's
        // in use, as it is now, its network work alone.
        if self.second_allowed() {
            let (speed, measured) = speeds.get(&second).copied().unwrap_or((0.8, false));
            let mem_mb = (cond::resources(&self.o.home, None, None, None).mem_gb * 256.0) as u64;
            let light_s = if self.last_cond.is_some_and(|c| c.user_active()) { forecast::IN_USE_S } else { 0.0 };
            machines.push(Machine { name: second.clone(), speed, measured, helper: false, second: true, light_s, mem_mb, busy_s: busy(1, speed) });
        }
        for h in &helpers {
            let (speed, measured) = speeds.get(&h.host).copied().unwrap_or((0.5, false));
            // (Its lease's targets at its pace, less the time since it took them, if longer than its
            // part's.)
            let lease_left = leased.iter().filter(|l| l.0 == h.host).map(|(_, step, ts, age)| ts.iter().map(|t| cost(step, t).secs).sum::<f64>() / speed - *age as f64).fold(0.0, f64::max);
            let eta = h.job.as_ref().map(|j| j.progress.as_ref().and_then(|p| p.eta_s).unwrap_or(600) as f64);
            let busy_s = eta.map_or(0.0, |e| e.max(lease_left).max(60.0));
            machines.push(Machine { name: h.host.clone(), speed, measured, helper: true, second: false, light_s: 0.0, mem_mb: mem.get(&h.host).copied().filter(|&m| m > 0).unwrap_or(6144), busy_s });
        }
        // What's being built now, and by which.
        let mut running: BTreeMap<(String, String), usize> = BTreeMap::new();
        for k in 0..SLOTS {
            let Some(m) = machines.iter().position(|m| m.name == self.worker_of(k)) else { continue };
            let Some(r) = self.slots[k].running.as_ref() else { continue };
            if let Some(b) = before.iter().find(|b| b.0 == step_of(&r.spec.id)) {
                running.insert((b.0.clone(), b.1.clone()), m);
            }
            if let Some(w) = r.spec.record.as_ref() {
                running.extend(w.targets.iter().map(|t| ((w.step.clone(), t.0.clone()), m)));
            }
        }
        for (worker, step, targets, _) in leased {
            if let Some(m) = machines.iter().position(|m| m.name == worker && m.helper) {
                running.extend(targets.into_iter().map(|t| ((step.clone(), t), m)));
            }
        }
        // (The round under way: its chain left as its steps take, a minute at least.)
        let under_way = under_way.map(|(ids, last, left)| (ids, last, chain_s(&left).max(60.0)));
        forecast::forecast(&forecast::Input { now: now_s(), before, regions, cost: &cost, round_s, last_round_s, blind, chains: chain_jobs, after, since_last, under_way, machines, running })
    }

    /// The build to the end (the status's checklist): the OSM pass (its stages, from the markers its
    /// scratch folder keeps), the pass's worldwide jobs, then the regions' steps (build::checklist).
    fn checklist(&self, root: &Path, regions: &[recipes::Recipe]) -> Vec<build::Step> {
        let mut out = Vec::new();
        let have = crate::osmpass::latest_pass(root);
        // The pass: a newer planet's under way (or waiting), else done.
        let pass_stages = ["filter", "sets", "outlines", "basemap", "cut", "roads"];
        let mut pass = build::Step { what: "Reading the newest OpenStreetMap planet".into(), steps: vec!["osm-pass".into()], total: Some(pass_stages.len()), unit: "stages".into(), ..Default::default() };
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
            out.push(build::Step { what: "Preparing the worldwide data: sets, route ends, roads' reach, buildings, summits, labels".into(), steps: ["pass-sets", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels"].iter().map(|s| s.to_string()).collect(), ..Default::default() });
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
            what: "Preparing the worldwide data: sets, route ends, roads' reach, buildings, summits, labels".into(),
            steps: ["pass-sets", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels"].iter().map(|s| s.to_string()).collect(),
            left: Some(left),
            ..Default::default()
        });
        if regions.is_empty() {
            return out;
        }
        let Ok(covs) = self.coverage(root, &manifest, &date, regions, false) else { return out };
        let cov = &covs.all;
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        out.extend(build::checklist(&cov, &date, &manifest, &keys, &input_digests(root), root.join("inputs/hold-catalog").exists(), reach.as_deref(), &self.ready.borrow()));
        out
    }

    /// Per region, how many of its areas are built (none before the first pass makes the outlines).
    fn region_progress(&self, root: &Path, regions: &[recipes::Recipe]) -> BTreeMap<String, build::RegionState> {
        let Some(date) = crate::osmpass::latest_pass(root) else { return BTreeMap::new() };
        let manifest: BTreeMap<String, String> = std::fs::read(root.join("state/build/manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let Ok(covs) = self.coverage(root, &manifest, &date, regions, false) else { return BTreeMap::new() };
        let (cov, each) = (&covs.all, &covs.each);
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

    /// The coverage of `recipes`, and each region's, as last made: made again only when the recipes,
    /// the pass's outlines or the outline files change. (An `osm:` outline is read from the pass's
    /// file on the NAS, 2.7 GB: made three times a loop, then each region's again, they had the loop
    /// take a quarter of an hour once the US's states were in, 2026-10-05.) `edits`: the recipes
    /// are all of them (every one read), and a change to them is the owner's edit (the plan's; not
    /// the heartbeat's, whose read may have failed).
    fn coverage(&self, root: &Path, manifest: &BTreeMap<String, String>, date: &str, recipes: &[recipes::Recipe], edits: bool) -> Result<std::rc::Rc<Coverages>, String> {
        let dir = root.join("inputs/outlines");
        let outlines = manifest.get(&format!("sources/osm/{date}/outlines")).cloned();
        let (key, seen) = coverage_key(recipes, outlines.as_deref(), &dir).unzip();
        if let Some(e) = seen.filter(|_| edits) {
            if self.edits.borrow().as_ref().is_some_and(|last| *last != e) {
                let now = std::time::SystemTime::now();
                let first = self.edited_at.get().filter(|(_, last)| last.elapsed().is_ok_and(|a| a < EDIT_HOLD)).map_or(now, |(first, _)| first);
                self.edited_at.set(Some((first, now)));
            }
            *self.edits.borrow_mut() = Some(e);
        }
        if let Some((k, c)) = self.coverage.borrow().as_ref() {
            if Some(k) == key.as_ref() {
                return Ok(c.clone());
            }
        }
        let o = outlines.map(|c| crate::outlines::Outlines::open(&root.join(c))).transpose().map_err(|e| format!("the pass's outlines: {e:#}"))?;
        let all = crate::coverage::Coverage::from_recipes(recipes, o.as_ref(), &dir).map_err(|e| format!("{e:#}"))?;
        let c = std::rc::Rc::new(Coverages { each: all.by_region(), all });
        // (Not kept when the outline files couldn't be listed: made again next time.)
        *self.coverage.borrow_mut() = key.map(|k| (k, c.clone()));
        Ok(c)
    }

    /// The regions the last catalog in `dir` has, by id: whether as its recipe is now (same outline
    /// entries); and how long ago it went out (seconds, by its file's time). Read once per catalog;
    /// none (nothing on the map: every region new) when there's none, or it can't be read now.
    fn on_map(&self, dir: &Path, recipes: &[recipes::Recipe]) -> (BTreeMap<String, bool>, Option<u64>) {
        // (The newest catalog that reads: a damaged one is passed over, as the map's server does,
        // not taken for none.)
        let mut cached = self.last_catalog.borrow_mut();
        let mut at = None;
        for n in store::catalog::list(dir).unwrap_or_default() {
            let path = dir.join(store::catalog::file_name(n));
            let Ok(t) = std::fs::metadata(&path).and_then(|m| m.modified()) else { continue };
            if cached.as_ref().is_none_or(|c| c.0 != (dir.to_path_buf(), n)) {
                let Ok(cat) = store::catalog::read(&path) else { continue };
                let regions: BTreeMap<String, Vec<String>> = cat.coverage.get("regions").and_then(serde_json::Value::as_array).into_iter().flatten().filter_map(|r| Some((r["id"].as_str()?.to_string(), serde_json::from_value(r["outline"].clone()).ok()?))).collect();
                *cached = Some(((dir.to_path_buf(), n), regions));
            }
            at = Some(t);
            break;
        }
        // How long ago the last catalog went out, or one last started (it may have failed): a
        // time ahead of this Mac's clock (the NAS's) counts as now.
        let now = std::time::SystemTime::now();
        let since = |t: std::time::SystemTime| now.duration_since(t).map_or(0, |d| d.as_secs());
        let started = (self.mem.catalog_at > 0).then(|| now_s().saturating_sub(self.mem.catalog_at));
        let since_publish = match (at.map(since), started) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if at.is_none() {
            return (BTreeMap::new(), since_publish);
        }
        let had = &cached.as_ref().unwrap().1;
        let on_map = had.iter().map(|(id, outline)| (id.clone(), recipes.iter().any(|r| &r.id == id && &r.outline == outline))).collect();
        (on_map, since_publish)
    }

    /// The build's pause as this agent knows it, brought up to date: this Mac's ask (its menu,
    /// `scenic pause`, the map: crate::control::Request) passed on, to this Mac's coordinator, or
    /// from a helper to the build Mac's (and this Mac paused or going on meanwhile, while that can't
    /// be reached); on the build Mac, then, its coordinator's, mirrored to the NAS
    /// (`state/build/pause.json`, for whoever reads the build's state there). A helper hears the
    /// build's otherwise in its asks and beats.
    fn sync_pause(&mut self, root: Option<&Path>, waiting: &mut Vec<Waiting>) {
        // (Not a dry run's, in another agent's folder: the asks are the real agent's to take up.)
        if self._lock.is_none() || self.o.dry_run {
            return;
        }
        // (This agent's own pause, kept from before a coordinator was up here, given to it: a
        // coordinator with none never lifts a pause no one asked to lift.)
        if let (Some(c), false) = (&self.coord, self.pause_pushed) {
            if let (None, Some(p)) = (c.pause(), &self.pause) {
                c.set_pause(Some(p.clone()), p.at);
            }
            self.pause_pushed = true;
        }
        match crate::control::take_request(&self.o.home) {
            Some(req) => {
                if let Some(c) = &self.coord {
                    c.set_pause(req.pause.clone(), req.at);
                    crate::control::clear_request(&self.o.home, &req);
                } else if self.o.helper {
                    let sent = match root {
                        Some(r) => self.client(r, waiting).map(|c| c.set_pause(req.pause.as_ref(), req.at)),
                        None => self.client.as_ref().map(|c| c.set_pause(req.pause.as_ref(), req.at)),
                    };
                    match sent {
                        Some(Ok(())) => {
                            crate::control::clear_request(&self.o.home, &req);
                            self.pause_local = false;
                        }
                        Some(Err(e)) => {
                            waiting.push(Waiting { step: None, what: "Pausing".into(), why: format!("this Mac's ask isn't with the build Mac yet ({e:#}); it holds here meanwhile") });
                            self.pause_local = true;
                        }
                        None => self.pause_local = true,
                    }
                    self.know_pause(req.pause);
                } else {
                    // (No coordinator here: one that couldn't start. This Mac's own, given to its
                    // coordinator once one's up.)
                    crate::control::clear_request(&self.o.home, &req);
                    self.pause_pushed = false;
                    self.know_pause(req.pause);
                }
            }
            // (No ask waiting: none is this Mac's own any more.)
            None => self.pause_local = false,
        }
        if let Some(c) = &self.coord {
            let p = c.pause();
            if p != self.pause {
                self.know_pause(p.clone());
            }
            // Mirrored to the NAS, until it's written there.
            if let (Some(r), false) = (root, self.mirrored.as_ref() == Some(&p)) {
                let path = r.join("state/build/pause.json");
                let kept = match &p {
                    Some(x) => serde_json::to_vec(x).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&path, &b)),
                    None => std::fs::remove_file(&path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }).map_err(anyhow::Error::from),
                };
                match kept {
                    Ok(()) => self.mirrored = Some(p),
                    Err(e) => eprintln!("agent: the pause on the NAS: {e:#}"),
                }
            }
        }
    }

    /// The build's pause as heard, or asked for here (`None`: going on), kept in the agent's folder
    /// (`pause.json`) so a restart keeps it.
    fn know_pause(&mut self, p: Option<crate::control::Pause>) {
        if p == self.pause {
            return;
        }
        eprintln!("agent: {}", p.as_ref().map_or("the build goes on".to_string(), |p| p.why()));
        let path = self.o.home.join("pause.json");
        let kept = match &p {
            Some(x) => serde_json::to_vec(x).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&path, &b)),
            None => std::fs::remove_file(&path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }).map_err(anyhow::Error::from),
        };
        if let Err(e) = kept {
            eprintln!("agent: keeping the pause: {e:#}");
        }
        self.pause = p;
    }

    /// Slot `k`'s job's channel (crate::control: "run" or "drain").
    fn control_path(&self, k: usize) -> PathBuf {
        self.o.home.join(if k == 0 { "control" } else { "control-2" })
    }

    /// Where slot `k`'s job notes the targets it finishes: a helper's leased job's in its lease's
    /// outbox folder (kept with its saves, should the agent stop), else in the agent's folder.
    fn done_path(&self, k: usize) -> PathBuf {
        match &self.slots[k].lease {
            Some(Held::Leased { dir, .. }) => dir.join("done.txt"),
            _ => self.o.home.join(if k == 0 { "done.txt" } else { "done-2.txt" }),
        }
    }

    /// What slot `k`'s job finished: every target (`all`: it succeeded), else those it noted done
    /// as each was saved (crate::control::done), whatever stopped it.
    fn finished_targets(&self, k: usize, all: bool) -> Vec<(String, String)> {
        let Some(w) = self.slots[k].running.as_ref().and_then(|r| r.spec.record.as_ref()) else { return Vec::new() };
        if all {
            return w.targets.clone();
        }
        let names = crate::control::read_done(&self.done_path(k), &w.step);
        w.targets.iter().filter(|(t, _)| names.contains(t)).cloned().collect()
    }

    /// Records `done` of `step` in the build's keys (the build Mac's agent; a helper's go back with
    /// its lease): whether they're recorded.
    fn record_done(&self, root: Option<&Path>, step: &str, done: &[(String, String)]) -> bool {
        let (false, false, Some(root)) = (self.o.helper, done.is_empty(), root) else { return false };
        match build::Keys::load_strict(root).and_then(|mut k| {
            k.record(step, done);
            k.save(root)
        }) {
            Ok(()) => {
                // (Its catalog made, a round is over: what changed meanwhile goes out with the next.)
                if matches!(step, "catalog" | "catalog-held") {
                    self.end_round();
                }
                true
            }
            Err(e) => {
                eprintln!("agent: recording {} of {step}: {e:#}", done.len());
                false
            }
        }
    }

    /// Slot `k`'s job stopped other than by its own end (the Mac slept, its lease lapsed, the agent
    /// stops, another Mac took its claims): what it finished is recorded (or, a helper's, handed
    /// off), and its lease given back, not held against its targets.
    fn stopped(&mut self, k: usize, root: Option<&Path>, why: &str) {
        let done = self.finished_targets(k, false);
        let step = self.slots[k].running.as_ref().and_then(|r| r.spec.record.as_ref().map(|w| w.step.clone())).unwrap_or_default();
        let secs = self.slots[k].running.as_ref().map_or(0, |r| r.elapsed().as_secs());
        if let Some(id) = self.slots[k].running.as_ref().map(|r| r.spec.id.clone()) {
            self.worked(&id);
            self.save();
        }
        self.note_end(k, &step, &done, secs, false, why);
        let recorded = self.record_done(root, &step, &done);
        let handed = if self.o.helper || recorded { done } else { Vec::new() };
        self.end_lease(k, Outcome::Interrupted, &handed, why);
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

/// Why the build isn't done, by its forecast (None: nothing left to build, no round under way, no
/// machine busy). One more than ten minutes old (the plan can't be made now), or made before the
/// last job here ended (`since`: a "done" from before it, as a job that starts and ends between two
/// forecasts leaves), doesn't say.
fn work_left(f: Option<&forecast::Forecast>, since: u64) -> Option<String> {
    match f {
        Some(f) if now_s().saturating_sub(f.at) > 600 => Some(format!("the build's forecast is {} min old", now_s().saturating_sub(f.at) / 60)),
        Some(f) if !f.nothing_left() => Some(f.why.as_ref().map_or_else(|| "the build has work left".to_string(), |w| format!("the build has work left: {w}"))),
        Some(f) if f.at < since => Some("the build's forecast is from before the last job here ended".into()),
        Some(_) => None,
        None => Some("the build has no forecast yet".into()),
    }
}

/// A trim's (`trimmed`) or a clear's event in the history, as `worker` did it, in words.
fn caches_event(worker: &str, trimmed: bool, f: &room::Freed) -> crate::coord::history::Event {
    let asked = f.by.as_ref().map(|b| format!(", as {b} asked")).unwrap_or_default();
    let note = match &f.why_not {
        Some(why) => format!("didn't clear its build caches{asked}: {why}"),
        None if trimmed => format!("trimmed its caches after the build: {}", f.say()),
        None => format!("cleared its build caches{asked}: {}", f.say()),
    };
    crate::coord::history::Event { worker: Some(worker.to_string()), note, ..crate::coord::history::Event::new("caches") }
}

/// A job's step: its id's first word ("unit 6/31/20": "unit").
fn step_of(id: &str) -> Option<String> {
    id.split(' ').next().filter(|s| !s.is_empty()).map(str::to_string)
}

/// The checklist as the status shows it. A step with work left that isn't this Mac's job now (`now`,
/// its step) says why: another Mac is on it, or its job waits (`waiting`: for the home network,
/// out a failure). Publishing, while a step above has work left, goes out as each region is done (at
/// most hourly) and once they all are: it's done only then.
fn annotate(list: &mut [build::Step], now: &[String], helpers: &[Status], waiting: &[Waiting]) {
    build::mark_shared(list);
    let busy = |s: &build::Step| now.iter().any(|n| s.steps.iter().any(|x| x == n));
    // (Work known to be left: a line not sized yet, as trains a day before its sources are seeded,
    // holds nothing up.)
    let known_left = |s: &build::Step| s.left.is_some_and(|l| l > 0) || s.total.is_some_and(|t| s.done < t);
    let before_left = list.iter().rev().skip(1).any(known_left);
    for s in list.iter_mut() {
        if s.finished() || busy(s) {
            continue;
        }
        let ours = |step: &Option<String>| step.as_ref().is_some_and(|st| s.steps.contains(st));
        if let Some(h) = helpers.iter().find(|h| ours(&h.job.as_ref().and_then(|j| step_of(&j.id)))) {
            s.note = Some(format!("on {}", h.host));
        } else if let Some(w) = waiting.iter().find(|w| ours(&w.step)) {
            s.note = Some(w.why.clone());
        }
    }
    if let Some(p) = list.last_mut().filter(|p| p.steps.iter().any(|s| s == "catalog")) {
        if before_left && !busy(p) {
            p.left = Some(p.left.unwrap_or(0).max(1));
            // (Its own, a catalog failing, says more.)
            p.note.get_or_insert_with(|| "as each region is done (at most hourly), and once the steps above are".into());
        }
    }
}

/// Why a running job stops, and how, if it does: at once (frozen where it is) without what it can't
/// save without, the NAS, or away from home with a job that needs it; as the build's pause says;
/// at its next safe point when the battery runs low (CPU work), before the charge runs out.
fn stop_for(n: &Needs, c: &Conditions, pause: Option<&crate::control::Pause>) -> Option<(crate::control::Mode, String)> {
    use crate::control::Mode;
    match lapsed(n, c) {
        Some(why) if (n.nas && !c.nas) || (n.home && !c.home) => Some((Mode::Freeze, why)),
        _ if pause.is_some() => pause.map(|p| (p.mode, p.why())),
        Some(why) => Some((Mode::Drain, why)),
        None => None,
    }
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
    if n.cpu && !c.ac && c.battery.is_none_or(|b| b < cond::BATTERY_MIN) {
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
    // (A rename over a file another Mac has open, its server reading the heartbeat: tried again.)
    crate::whole::rename_over(&tmp, p).with_context(|| format!("rename to {}", p.display()))
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

/// The newest catalog in `dir` that reads, its number and when it went out: what the map's server
/// serves (a damaged one passed over, as it does).
fn served_catalog(dir: &Path) -> Option<CatalogSeen> {
    let mut ns = store::catalog::list(dir).ok()?;
    ns.sort_unstable();
    ns.into_iter().rev().find_map(|n| {
        let path = dir.join(store::catalog::file_name(n));
        let t = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        store::catalog::read(&path).ok()?;
        Some(CatalogSeen { n, at: t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) })
    })
}

/// About how long a target of `step` takes on the build Mac (seconds) until it's been timed there:
/// the forecast's first guess (crate::agent::forecast), from the jobs' logs of 2026-10.
fn first_secs(step: &str) -> f64 {
    match step {
        "pass-sets" => 5000.0,
        "trailends" => 15.0,
        "reach" => 1800.0,
        "terrain-z8" | "buildings" | "items" => 3600.0,
        "summits" => 30.0,
        "labels" => 2200.0,
        "heritage-sites" => 240.0,
        "heritage" => 5400.0,
        "terrain" => 900.0,
        "slope" => 400.0,
        "trees" => 600.0,
        "unit" => 400.0,
        "prune" => 30.0,
        "pack" => 35.0,
        "lo" => 25.0,
        "roadunits" => 100.0,
        "stations" => 110.0,
        "ferries" => 30.0,
        "terrain-root" | "slope-root" => 10.0,
        "rail-feeds" => 240.0,
        "rail" => 540.0,
        "pois" => 10.0,
        "peaks" => 15.0,
        "marks" => 190.0,
        "overlays" => 50.0,
        "catalog" => 60.0,
        _ => 300.0,
    }
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
    fn a_terrain_run_reads_its_own_areas_archives() {
        let packs = Path::new("/c/aws-terrarium/packs");
        for (name, ours) in [("3-1-2.0123456789abcdef.tiles", true), ("6-8-16.0123456789abcdef.tiles", true), ("6-15-23.0123456789abcdef.tiles", true), ("6-16-16.0123456789abcdef.tiles", false), ("3-1-3.0123456789abcdef.tiles", false), ("root.0123456789abcdef.tiles", false)] {
            assert_eq!(terrain_reads("terrain 3/1/2", &packs.join(name)), ours, "{name}");
        }
        // Not a terrain run's, or not an archive copy.
        assert!(!terrain_reads("unit 6/8/16", &packs.join("6-8-16.0123456789abcdef.tiles")));
        assert!(!terrain_reads("terrain-root terrain-root", &packs.join("root.0123456789abcdef.tiles")));
        assert!(!terrain_reads("terrain 3/1/2", Path::new("/c/aws-terrarium/12/2048/1500.png")));
    }

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
    fn a_helper_asks_only_for_what_its_disk_has_room_for() {
        let gb = |n: u64| n << 30;
        // 20 GB free and 10 of caches it may empty: the 15 GB steps (and their margin) and tasks, not
        // tree cover's 30 nor a terrain run's 55.
        assert_eq!(helper_steps(gb(20), gb(10)), ["slope", "unit", "pois", "peaks", "tail"]);
        assert_eq!(helper_steps(gb(70), 0), [claims::SHARED.to_vec(), vec!["tail"]].concat());
        assert_eq!(helper_steps(gb(10), gb(5)), ["tail"]);
        assert!(helper_steps(gb(3), gb(2)).is_empty());
    }

    #[test]
    fn a_helper_runs_any_shared_steps_job_the_build_mac_leases() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        // The build Mac's coordinator offers slope, which this helper fits.
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        c.offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        let mut a = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: true }).unwrap();
        a.client = Some(crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1"));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut w = Vec::new();
        let jobs = a.helper_job(&root, &cond, &mut w);
        assert_eq!(jobs.len(), 1, "{w:?}");
        let j = &jobs[0];
        // The build Mac's own command for slope, its saves handed off to its lease's outbox.
        assert_eq!(j.id, "slope 3/2/2");
        assert!(j.cmd[1].starts_with("SCENIC_HANDOFF=") && j.cmd[2].starts_with("SCENIC_COSTS="));
        assert_eq!(&j.cmd[3..6], ["/app/scenic-build", "slope", "--root"]);
        assert!(j.cmd.contains(&"3/2/2".to_string()));
        assert_eq!(j.record.as_ref().map(|r| r.step.as_str()), Some("slope"));
        assert!(j.what.contains("for the build Mac"), "{}", j.what);
    }

    #[test]
    fn which_jobs_run_beside_each_other() {
        // Network work beside anything but what runs alone; two of one step only when it's shared
        // (its targets held apart); the raw tiles' readers one at a time.
        assert!(!clash("unit", "heritage") && !clash("trees", "items"));
        assert!(!clash("unit", "unit") && !clash("pois", "pois"));
        assert!(clash("items", "items") && clash("catalog", "catalog"));
        assert!(clash("terrain", "peaks") && clash("peaks", "peaks"));
        assert!(clash("osm-pass", "items") && clash("items", "gc") && clash("heritage-sites", "unit"));
        // Two asking Wikidata, each paced as if alone: not at once.
        assert!(clash("items", "heritage") && !clash("items", "rail-feeds"));
    }

    #[test]
    fn a_second_job_runs_beside_the_first() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.free_set = Some(40 << 30);
        assert!(a.second_allowed());
        // (Each a shell waiting, given its scratch folder as the plan's jobs are.)
        let job = |id: &str| {
            let step = id.split(' ').next().unwrap();
            let scratch = home.join("scratch").join(step).to_string_lossy().into_owned();
            let record = Some(build::Work { step: step.into(), targets: vec![(id.split(' ').nth(1).unwrap().into(), "k".into())] });
            JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into(), "--scratch".into(), scratch], needs: Needs { cpu: false, nas: false, home: false }, restart_after_sleep: false, record }
        };
        // Someone at the Mac: beside the map tiles, the heritage chain (network), not a unit.
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, job("pack 6/1/1"), &cond, Some(&root), &mut Vec::new()));
        let plan = vec![job("unit 6/1/1"), job("pack 6/1/1"), job("heritage heritage")];
        a.start_second(&plan, &cond, Some(&root));
        let second = a.slots[1].running.as_ref().map(|r| (r.spec.id.clone(), r.spec.cmd[4].clone()));
        // Its scratch folder its own.
        assert_eq!(second, Some(("heritage heritage".to_string(), home.join("scratch-2/heritage").to_string_lossy().into_owned())), "{:?}", a.beside_why);
        assert!(a.record_path(1).exists() && a.record_path(0).exists());
        // In the history as the second job.
        let (_, _, events, _) = a.coord.as_ref().unwrap().for_forecast();
        assert!(events.iter().any(|e| e.kind == "start" && e.worker == Some(second_worker(&a.host))), "{events:?}");
        for k in 0..SLOTS {
            if let Some(r) = a.slots[k].running.as_mut() {
                r.stop(Duration::from_secs(5));
            }
        }
        // Beside a job that runs alone (the OSM pass), none.
        a.slots = Default::default();
        assert!(a.try_start(0, JobSpec { record: None, ..job("osm-pass 2026-09-28") }, &cond, Some(&root), &mut Vec::new()));
        a.start_second(&plan, &cond, Some(&root));
        assert!(a.slots[1].running.is_none());
        assert!(a.beside_why.as_deref().is_some_and(|w| w.contains("alone")), "{:?}", a.beside_why);
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
    }

    #[test]
    fn the_first_job_isnt_starved_by_the_second() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.free_set = Some(40 << 30);
        let job = |id: &str| {
            let step = id.split(' ').next().unwrap();
            let scratch = home.join("scratch").join(step).to_string_lossy().into_owned();
            let record = Some(build::Work { step: step.into(), targets: vec![(id.split(' ').nth(1).unwrap().into(), "k".into())] });
            JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into(), "--scratch".into(), scratch], needs: Needs { cpu: false, nas: false, home: false }, restart_after_sleep: false, record }
        };
        let stop = |a: &mut Agent| {
            for s in a.slots.iter_mut() {
                if let Some(r) = s.running.as_mut() {
                    r.stop(Duration::from_secs(5));
                }
            }
            a.slots = Default::default();
        };
        let in_use = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        // The heritage chain beside: the first takes the map tiles, leaving the items' facts, listed
        // first, to the second (they'd hold the first slot an hour).
        assert!(a.try_start(1, job("heritage heritage"), &in_use, Some(&root), &mut Vec::new()));
        a.start_first(&[job("items items"), job("pack 6/1/1")], &in_use, Some(&root), &mut Vec::new());
        assert_eq!(a.slots[0].running.as_ref().map(|r| r.spec.id.as_str()), Some("pack 6/1/1"));
        stop(&mut a);
        // A job that runs alone next: waited for, not passed over for the map tiles after it.
        assert!(a.try_start(1, job("heritage heritage"), &in_use, Some(&root), &mut Vec::new()));
        let mut w = Vec::new();
        a.start_first(&[job("gc gc"), job("pack 6/1/2")], &in_use, Some(&root), &mut w);
        assert!(a.slots[0].running.is_none());
        assert!(w.iter().any(|x| x.what == "gc gc" && x.why.contains("don't run together")), "{w:?}");
        // One that needs room made on the disk: made beside the heritage chain (it reads none of the
        // caches), and it starts...
        a.free_set = Some(20 << 30);
        a.start_first(&[job("pack 6/1/3")], &in_use, Some(&root), &mut Vec::new());
        assert_eq!(a.slots[0].running.as_ref().map(|r| r.spec.id.as_str()), Some("pack 6/1/3"));
        stop(&mut a);
        // ...not beside a job that may read them (the landmarks' candidates, while the Mac's idle):
        // it waits for that one.
        let idle = Conditions { idle_s: 3600, ..in_use };
        a.free_set = Some(40 << 30);
        assert!(a.try_start(1, job("pois 6/9/9"), &idle, Some(&root), &mut Vec::new()));
        a.free_set = Some(20 << 30);
        let mut w = Vec::new();
        a.start_first(&[job("pack 6/1/3")], &idle, Some(&root), &mut w);
        assert!(a.slots[0].running.is_none() && w.iter().any(|x| x.why.contains("room is made once")), "{w:?}");
        a.free_set = Some(40 << 30);
        stop(&mut a);
        // And the second starts nothing beside the first while one that runs alone comes next.
        assert!(a.try_start(0, job("pack 6/1/1"), &in_use, Some(&root), &mut Vec::new()));
        a.start_second(&[job("gc gc"), job("heritage heritage")], &in_use, Some(&root));
        assert!(a.slots[1].running.is_none());
        assert!(a.beside_why.as_deref().is_some_and(|w| w.contains("runs alone, next")), "{:?}", a.beside_why);
        stop(&mut a);
    }

    #[test]
    fn a_rounds_catalog_made_ends_it_and_a_dry_run_keeps_its_rounds_to_itself() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let a = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        let r = build::Round { began: 100, regions: vec!["a".into()], last: false, units: [("base/6-1-1".to_string(), "base/6-1-1.1111111111111111.base".to_string())].into(), over: false };
        a.keep_round(r.clone()).unwrap();
        let file = || -> build::Round { serde_json::from_slice(&std::fs::read(home.join(ROUND_FILE)).unwrap()).unwrap() };
        assert_eq!(file(), r);
        // Another step's keys recorded: still under way. Its catalog's: over, kept without its
        // units (whatever changed meanwhile goes out with the next).
        assert!(a.record_done(Some(&root), "pack", &[("6/1/1".into(), "k".into())]));
        assert!(!file().over);
        assert!(a.record_done(Some(&root), "catalog", &[("catalog".into(), "k".into())]));
        let f = file();
        assert!(f.over && f.units.is_empty() && f.began == 100);
        // A dry run beside it (a second agent on this Mac, planning only): its rounds its own, never
        // the file the real one's jobs read.
        let dry = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
        dry.keep_round(build::Round { began: 200, ..r }).unwrap();
        assert_eq!(file().began, 100);
    }

    /// An agent that runs jobs (no dry run), the NAS at `root`: the build Mac's, or a helper's.
    fn running_agent(root: &Path, home: &Path, helper: bool) -> Agent {
        Agent::new(Options { root: Some(root.to_path_buf()), home: home.to_path_buf(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper }).unwrap()
    }

    /// A forecast made now: the build done (nothing left to build), or not.
    fn forecast_now(done: bool) -> forecast::Forecast {
        forecast::Forecast { at: now_s(), done_at: (!done).then(|| now_s() + 3600), why: done.then(|| forecast::NOTHING_LEFT.to_string()), ..Default::default() }
    }

    /// A job that waits half a minute, needing nothing.
    fn waiting_job(id: &str) -> JobSpec {
        JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()], needs: Needs { cpu: false, nas: false, home: false }, restart_after_sleep: false, record: None }
    }

    fn put(p: &Path, b: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }

    #[test]
    fn the_build_mac_trims_once_the_build_is_done_and_again_after_work() {
        // (The disk roomy: starting a job deletes nothing.)
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let c = home.join("cache");
        let copy = |n: u8| c.join(format!("blobs/layers/terrain/hi/6-1-{n}.000000000000000{n}.pack"));
        put(&c.join("chm10/a.tif"), &crate::whole::testfiles::tiff(false));
        put(&copy(1), &[1; 1000]);
        let mut a = running_agent(&root, &home, false);
        // Work left: nothing goes.
        *a.forecast.borrow_mut() = Some(forecast_now(false));
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.starts_with("the build has work left")));
        assert!(copy(1).exists() && a.mem.trimmed.is_none());
        // Done, but away from home (through Tailscale, a trim would take hours): not yet.
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        assert_eq!(a.tend_caches(Some(&root), false), None);
        assert!(copy(1).exists() && a.mem.trimmed.is_none());
        // Home: the copies go, the canopy squares stay; said in the status's memory, once.
        assert_eq!(a.tend_caches(Some(&root), true), None);
        assert!(!copy(1).exists() && c.join("chm10/a.tif").exists());
        assert_eq!(a.mem.trimmed.as_ref().map(|f| (f.bytes(), f.left)), Some((1000, 0)));
        assert_eq!(a.cache_size.lock().unwrap().1.as_ref().map(|n| n.cheap), Some(crate::whole::testfiles::tiff(false).len() as u64), "counted again");
        put(&copy(2), &[1; 1000]);
        a.tend_caches(Some(&root), true);
        assert!(copy(2).exists(), "once per finished state");
        // A daily job since: still not; a job of the build's: again, by a forecast made since it
        // ended (one made before, a "done" from then, says nothing).
        a.mem.trimmed.as_mut().unwrap().at -= 10;
        a.worked("gc");
        a.tend_caches(Some(&root), true);
        assert!(copy(2).exists());
        a.forecast.borrow_mut().as_mut().unwrap().at -= 5;
        a.worked("unit 6/1/1");
        assert_eq!(a.tend_caches(Some(&root), true).as_deref(), Some("the build's forecast is from before the last job here ended"));
        assert!(copy(2).exists());
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        a.tend_caches(Some(&root), true);
        assert!(!copy(2).exists());
        // Never while a job runs here.
        put(&copy(3), &[1; 1000]);
        a.mem.trimmed.as_mut().unwrap().at -= 10;
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, waiting_job("pack 6/1/1"), &cond, Some(&root), &mut Vec::new()));
        assert_eq!(a.tend_caches(Some(&root), true).as_deref(), Some("a job runs here (pack 6/1/1)"));
        assert!(copy(3).exists());
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        // Nor without the NAS (what goes must be kept there).
        a.worked("pack 6/1/1");
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        assert_eq!(a.tend_caches(None, true).as_deref(), Some("the NAS isn't reachable"));
        assert!(copy(3).exists());
        // A forecast the plan hasn't made again for a while says nothing.
        a.forecast.borrow_mut().as_mut().unwrap().at -= 3600;
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.contains("forecast is 60 min old")));
        assert!(copy(3).exists());
    }

    #[test]
    fn the_build_mac_sends_the_passs_answers_to_the_nas_once_as_it_starts() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        put(&root.join("sources/osm/2026-09-28/pass.0000000000000001.json"), b"{}");
        let archive = root.join("sources/items/2026-09-28/answers.tar.zst");
        for m in ["m1", "m4"] {
            put(&d.path().join(m).join("cache/items/facts-2026-09-28.jsonl"), b"{\"qid\":\"Q1\"}\n");
        }
        // A helper's: never (the steps that keep them are the build Mac's).
        let mut helper = running_agent(&root, &d.path().join("m1"), true);
        helper.seed_answers(Some(&root));
        assert!(!archive.exists());
        // The build Mac's: not while a job runs here, nor without the NAS; then once.
        let mut a = running_agent(&root, &d.path().join("m4"), false);
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, waiting_job("pack 6/1/1"), &cond, Some(&root), &mut Vec::new()));
        a.seed_answers(Some(&root));
        assert!(!archive.exists());
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        a.seed_answers(None);
        assert!(!archive.exists());
        a.seed_answers(Some(&root));
        assert!(archive.is_file());
        std::fs::remove_file(&archive).unwrap();
        a.seed_answers(Some(&root));
        assert!(!archive.exists(), "once an agent");
        // While it runs, no items or heritage job starts here; others do.
        let (go, wait) = std::sync::mpsc::channel::<()>();
        a.answers_seed = Some(std::thread::spawn(move || wait.recv().map(|()| Vec::new()).map_err(anyhow::Error::from)));
        let mut w = Vec::new();
        assert!(!a.try_start(0, waiting_job("items items"), &cond, Some(&root), &mut w));
        assert!(w.iter().any(|x| x.why.contains("answers here are going to the NAS")), "{w:?}");
        assert!(a.try_start(0, waiting_job("pack 6/1/2"), &cond, Some(&root), &mut Vec::new()));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        go.send(()).unwrap();
        while a.answers_seed.as_ref().is_some_and(|t| !t.is_finished()) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(a.try_start(0, waiting_job("items items"), &cond, Some(&root), &mut Vec::new()));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
    }

    #[test]
    fn a_trim_runs_on_a_thread_of_its_own_and_nothing_starts_meanwhile() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // (Not one loop at a time: the trim isn't waited for.)
        let mut a = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: false, helper: false }).unwrap();
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        // A trim under way (one that waits to be let go): the loop isn't held, no job starts.
        let (go, wait) = std::sync::mpsc::channel::<()>();
        a.caches_task = Some(CachesTask { ask: None, began: Instant::now(), thread: std::thread::spawn(move || wait.recv().map(|()| room::Freed { freed: BTreeMap::from([("blobs".to_string(), 1000)]), ..Default::default() }).map_err(anyhow::Error::from)) });
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.starts_with("this Mac's caches are being trimmed")));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut w = Vec::new();
        assert!(!a.try_start(0, waiting_job("pack 6/1/1"), &cond, Some(&root), &mut w));
        assert!(w.iter().any(|x| x.why.contains("nothing starts here until that's done")), "{w:?}");
        // Done: what it freed kept, and jobs start again.
        go.send(()).unwrap();
        while a.caches_task.as_ref().is_some_and(|t| !t.thread.is_finished()) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(a.tend_caches(Some(&root), true), None);
        assert_eq!(a.mem.trimmed.as_ref().map(|f| f.bytes()), Some(1000));
        assert!(a.caches_task.is_none());
        assert!(a.try_start(0, waiting_job("pack 6/1/1"), &cond, Some(&root), &mut Vec::new()));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
    }

    #[test]
    fn a_job_an_earlier_agent_left_holds_the_caches_until_its_gone() {
        use std::os::unix::process::CommandExt;
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut a = running_agent(&root, &home, false);
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        // (A process group of its own, as a job's.)
        let mut left = std::process::Command::new("/bin/sh").args(["-c", "sleep 30"]).process_group(0).spawn().unwrap();
        let pgid = left.id() as i32;
        let group = |leader_start: u64| jobs::Group { pgid, leader_start, started: now_s() - 5, id: "unit 6/1/1".into() };
        // Its group's id another program's now (another leader's start time): not the job's.
        a.orphans.push(group(1));
        assert_eq!(a.tend_caches(Some(&root), true), None);
        assert!(a.orphans.is_empty());
        // The job's: held while it runs, not once it's gone.
        a.orphans.push(group(crate::sys::process_start(pgid).unwrap()));
        assert_eq!(a.tend_caches(Some(&root), true).as_deref(), Some("a job an earlier agent left still runs here (unit 6/1/1)"));
        left.kill().unwrap();
        left.wait().unwrap();
        assert_eq!(a.tend_caches(Some(&root), true), None);
        assert!(a.orphans.is_empty());
    }

    #[test]
    fn an_ask_during_a_trim_waits_for_it_then_clears() {
        // (The review's case.)
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let c = home.join("cache");
        put(&c.join("base/base/6-1-1.0000000000000001.base"), &[1; 500]);
        let mut a = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: false, helper: false }).unwrap();
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        let (go, wait) = std::sync::mpsc::channel::<()>();
        a.caches_task = Some(CachesTask { ask: None, began: Instant::now(), thread: std::thread::spawn(move || wait.recv().map(|()| room::Freed::default()).map_err(anyhow::Error::from)) });
        room::request_clear(&home, "scenic clean on m4").unwrap();
        let settle = |a: &mut Agent| {
            while a.caches_task.as_ref().is_some_and(|t| !t.thread.is_finished()) {
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        // The ask waits for the trim under way; once it's done, it's taken up, and cleared.
        a.tend_caches(Some(&root), true);
        assert!(home.join(room::CLEAR_REQUEST).exists());
        go.send(()).unwrap();
        settle(&mut a);
        a.tend_caches(Some(&root), true);
        assert!(a.caches_task.as_ref().is_some_and(|t| t.ask.is_some()) && a.mem.trimmed.is_some());
        settle(&mut a);
        a.tend_caches(Some(&root), true);
        assert!(a.mem.cleared.as_ref().is_some_and(|f| f.why_not.is_none() && f.freed.get("base") == Some(&500)));
        assert!(!c.join("base").exists());
    }

    #[test]
    fn a_helper_trims_once_the_build_macs_heartbeat_says_the_build_is_done() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/writer"), "the-build-mac").unwrap();
        let c = home.join("cache");
        let tif = crate::whole::testfiles::tiff(false);
        put(&c.join("chm10/a.tif"), &tif);
        put(&root.join("sources/canopy/a.tif"), &tif);
        put(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), &[1; 1000]);
        let mut a = running_agent(&root, &home, true);
        let beat = |at: u64, done: bool, job: Option<&str>| {
            let job = job.map(|id| JobView { id: id.into(), what: format!("Ranking the world's place labels ({id})"), started: at, paused: None, pausing: None, tail: String::new(), parts: Vec::new(), part: None, progress: None, mem_mb: None, threads: None });
            let st = Status { host: "the-build-mac".into(), beat: at, job, forecast: Some(forecast::Forecast { at, ..forecast_now(done) }), ..Default::default() };
            std::fs::write(root.join("state/status.json"), serde_json::to_vec(&st).unwrap()).unwrap();
        };
        // No word from the build Mac: nothing goes.
        assert_eq!(a.tend_caches(Some(&root), true).as_deref(), Some("the build Mac's status can't be read now"));
        // Its heartbeat an hour old, the build with work left, or a job of the build Mac's running
        // (its forecast "done" all the same, as an older agent's said with a lone worldwide job):
        // still nothing.
        beat(now_s() - 3600, true, None);
        a.heard = None;
        assert_eq!(a.tend_caches(Some(&root), true).as_deref(), Some("the build Mac hasn't been heard from for 60 min"));
        beat(now_s(), false, None);
        a.heard = None;
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.starts_with("the build has work left")));
        beat(now_s(), true, Some("labels 2026-09-28"));
        a.heard = None;
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.starts_with("the build Mac runs a job (Ranking")));
        assert!(c.join("chm10/a.tif").exists());
        // Fresh, and the build done: every cheap cache goes, the canopy squares too.
        beat(now_s(), true, None);
        a.heard = None;
        assert_eq!(a.tend_caches(Some(&root), true), None);
        assert!(!c.join("chm10/a.tif").exists() && !c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack").exists());
        assert_eq!(a.mem.trimmed.as_ref().map(|f| (f.bytes(), f.left)), Some((tif.len() as u64 + 1000, 0)));
        // (Its own status says so: the build Mac notes it in the history from there.)
        assert!(a.mem.trimmed.as_ref().is_some_and(|f| f.why_not.is_none() && f.at >= a.started));
    }

    #[test]
    fn a_clear_ask_is_taken_up_between_jobs_and_reported() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let c = home.join("cache");
        let tif = crate::whole::testfiles::tiff(false);
        put(&c.join("chm10/a.tif"), &tif);
        put(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), &[1; 1000]);
        put(&c.join("base/base/6-1-1.0000000000000001.base"), &[1; 500]);
        put(&c.join("unit-stages.json"), b"{}");
        let mut a = running_agent(&root, &home, false);
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        // (Trimmed already: only the ask frees anything here.)
        a.mem.trimmed = Some(room::Freed { at: now_s(), ..Default::default() });
        let asked = |home: &Path| home.join(room::CLEAR_REQUEST).exists() || home.join(room::CLEAR_TAKEN).exists();
        // Asked while a job runs here: not cleared, and why said; the ask taken up all the same.
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, waiting_job("backup"), &cond, Some(&root), &mut Vec::new()));
        let r = room::request_clear(&home, "scenic clean on m4").unwrap();
        a.tend_caches(Some(&root), true);
        let f = a.mem.declined.clone().unwrap();
        assert_eq!((f.asked, f.bytes(), f.why_not.as_deref()), (Some(r.at), 0, Some("a job runs here (backup)")));
        assert!(!asked(&home) && c.join("chm10/a.tif").exists() && a.mem.cleared.is_none());
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        // Between jobs, the build done: cleared, the canopy squares too, and what it freed said.
        let r = room::request_clear(&home, "the menu bar on m4").unwrap();
        assert_eq!(a.tend_caches(Some(&root), true), None);
        let f = a.mem.cleared.clone().unwrap();
        assert_eq!((f.asked, f.by.as_deref(), f.why_not.as_deref()), (Some(r.at), Some("the menu bar on m4"), None));
        assert_eq!(f.freed, BTreeMap::from([("base".to_string(), 500), ("blobs".to_string(), 1000), ("canopy".to_string(), tif.len() as u64)]));
        assert!(!c.join("chm10/a.tif").exists() && root.join("sources/canopy/a.tif").exists() && c.join("unit-stages.json").exists());
        assert!(!asked(&home));
        assert_eq!(a.cache_size.lock().unwrap().1, Some(room::Sizes::default()), "counted again: nothing left to clear");
        // Declined later (no NAS): said apart, the last clear done kept.
        room::request_clear(&home, "scenic clean on m4").unwrap();
        a.tend_caches(None, true);
        assert_eq!(a.mem.declined.as_ref().and_then(|f| f.why_not.as_deref()), Some("the NAS isn't reachable"));
        assert_eq!(a.mem.cleared.as_ref().map(|f| f.asked), Some(Some(r.at)));
        // A dry run beside it (another agent runs the jobs) takes up no ask.
        room::request_clear(&home, "scenic clean on m4").unwrap();
        let mut dry = Agent::new(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
        *dry.forecast.borrow_mut() = Some(forecast_now(true));
        dry.tend_caches(Some(&root), true);
        assert!(home.join(room::CLEAR_REQUEST).exists());
    }

    #[test]
    fn the_build_mac_notes_a_helpers_trims_in_the_history_once() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = running_agent(&root, &home, false);
        a.coord = Some(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        let helper = |at: u64, bytes: u64, left: u64| Status { host: "m1".into(), caches: Some(room::Caches { trimmed: Some(room::Freed { at, freed: BTreeMap::from([("canopy".to_string(), bytes)]), left, ..Default::default() }), ..Default::default() }), ..Default::default() };
        // One from before this agent started (the one before noted it), then a new one, twice; one
        // that freed nothing, keeping nothing new (none); one that freed nothing but keeps more.
        a.note_helpers_caches(&[helper(a.started - 60, 3 << 30, 0)]);
        a.note_helpers_caches(&[helper(a.started + 5, 3 << 30, 0)]);
        a.note_helpers_caches(&[helper(a.started + 5, 3 << 30, 0)]);
        a.note_helpers_caches(&[helper(a.started + 9, 0, 0)]);
        a.note_helpers_caches(&[helper(a.started + 12, 0, 2 << 30)]);
        a.note_helpers_caches(&[helper(a.started + 15, 0, 2 << 30)]);
        let (_, _, events, _) = a.coord.as_ref().unwrap().for_forecast();
        let noted: Vec<(Option<&str>, &str)> = events.iter().filter(|e| e.kind == "caches").map(|e| (e.worker.as_deref(), e.note.as_str())).collect();
        assert_eq!(noted, [(Some("m1"), "trimmed its caches after the build: 3.0 GB freed (canopy squares 3.0 GB)"), (Some("m1"), "trimmed its caches after the build: 0 MB freed; 2.0 GB kept (the NAS hasn't it yet)")]);
    }

    #[test]
    fn the_checklist_says_which_steps_helpers_take() {
        let mut steps = build::checklist_to_come();
        build::mark_shared(&mut steps);
        let shared = |what: &str| steps.iter().find(|s| s.what == what).and_then(|s| s.shared.clone());
        assert_eq!(shared(build::TERRAIN).as_deref(), Some("all"));
        assert_eq!(shared(build::UNITS).as_deref(), Some("all"));
        assert_eq!(shared(build::LANDMARKS).as_deref(), Some("candidates and peaks"));
        assert_eq!(shared(build::TILES), None);
        assert_eq!(shared(build::PUBLISH), None);
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
    fn an_edit_to_a_recipe_or_an_outline_file_is_noticed_and_a_new_pass_is_not() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        let outlines = root.join("inputs/outlines");
        std::fs::create_dir_all(&outlines).unwrap();
        let r = |p: &str| vec![recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec![format!("place:{p},20")] }];
        // The pass's outlines change the coverage's key, not the edits'.
        let (k1, e1) = coverage_key(&r("-21.9,64.13"), Some("sources/osm/a.outlines"), &outlines).unwrap();
        let (k2, e2) = coverage_key(&r("-21.9,64.13"), Some("sources/osm/b.outlines"), &outlines).unwrap();
        assert!(k1 != k2 && e1 == e2);
        // A recipe's outline, or an outline file a recipe names, changes both; another file there
        // (a Finder's .DS_Store) only the coverage's.
        assert_ne!(coverage_key(&r("-21.9,64.2"), Some("sources/osm/a.outlines"), &outlines).unwrap().1, e1);
        let named = vec![recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec!["poly:x.poly".into()] }];
        let (k0, n1) = coverage_key(&named, Some("sources/osm/a.outlines"), &outlines).unwrap();
        std::fs::write(outlines.join(".DS_Store"), b"x").unwrap();
        let (k3, n2) = coverage_key(&named, Some("sources/osm/a.outlines"), &outlines).unwrap();
        assert!(n1 == n2 && k3 != k0);
        std::fs::write(outlines.join("x.poly"), b"x").unwrap();
        assert_ne!(coverage_key(&named, Some("sources/osm/a.outlines"), &outlines).unwrap().1, n2);
        // The agent: the first look isn't an edit; a change is (the plan's look, not the heartbeat's).
        let a = agent(&root, &home);
        let none = BTreeMap::new();
        a.coverage(&root, &none, "2026-09-28", &r("-21.9,64.13"), true).unwrap();
        a.coverage(&root, &none, "2026-09-28", &r("-21.9,64.13"), true).unwrap();
        a.coverage(&root, &none, "2026-09-28", &[], false).unwrap();
        assert!(a.edited_at.get().is_none());
        a.coverage(&root, &none, "2026-09-28", &r("-21.9,64.2"), true).unwrap();
        assert!(edit_held(a.edited_at.get()).is_some_and(|(age, left)| age < EDIT_HOLD && left <= EDIT_HOLD));
        // A run of edits holds the work an hour at most.
        let ago = |m: u64| std::time::SystemTime::now() - Duration::from_secs(m * 60);
        assert!(edit_held(Some((ago(50), ago(1)))).is_some_and(|(_, left)| left <= Duration::from_secs(10 * 60)));
        assert!(edit_held(Some((ago(61), ago(1)))).is_none());
        assert!(edit_held(Some((ago(20), ago(16)))).is_none());
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
    fn the_checklist_says_why_a_step_waits() {
        let step = |what: &str, steps: &[&str], left: usize| build::Step { what: what.into(), steps: steps.iter().map(|s| s.to_string()).collect(), left: Some(left), ..Default::default() };
        let list = || vec![step("Worldwide sets", &["pass-sets", "reach"], 1), step("Roads, elevations and scenery", &["unit"], 3), step("Landmarks", &["pois", "peaks", "items", "heritage", "marks", "overlays"], 2), step("Publishing the new map data", &["catalog", "catalog-held"], 0)];
        let helper = Status { host: "m1".into(), job: Some(JobView { id: "unit 6/1/2".into(), what: String::new(), started: 0, pausing: None, paused: None, tail: String::new(), progress: None, parts: Vec::new(), part: None, mem_mb: None, threads: None }), ..Default::default() };
        let waiting = [Waiting { step: Some("reach".into()), what: "How far…".into(), why: "away from home".into() }];
        let mut l = list();
        annotate(&mut l, &["heritage".to_string()], &[helper], &waiting);
        assert_eq!(l[0].note.as_deref(), Some("away from home"));
        assert_eq!(l[1].note.as_deref(), Some("on m1"));
        assert_eq!(l[2].note, None, "this Mac's job now");
        // Publishing waits for the steps above, though its catalog is current.
        assert!(!l[3].finished() && l[3].note.as_deref().is_some_and(|n| n.starts_with("as each region is done")));
        // All above done: done.
        let mut l: Vec<build::Step> = list().into_iter().map(|mut s| {
            s.left = Some(0);
            s
        }).collect();
        annotate(&mut l, &[], &[], &[]);
        assert!(l.iter().all(|s| s.finished() && s.note.is_none()));
        // A line not sized yet (trains a day before its sources) holds publishing up no more than a
        // done one; and publishing's own note (its catalog failing) is kept.
        l[1].left = None;
        annotate(&mut l, &[], &[], &[]);
        assert!(l[3].finished() && l[3].note.is_none());
        let mut l = list();
        l[3].left = Some(1);
        let failing = [Waiting { step: Some("catalog".into()), what: String::new(), why: "failed 3 times in a row".into() }];
        annotate(&mut l, &[], &[], &failing);
        assert_eq!(l[3].note.as_deref(), Some("failed 3 times in a row"));
    }

    #[test]
    fn conditions_gate_jobs() {
        let n = Needs { cpu: true, nas: true, home: false };
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
        // A running job stops at once without the NAS (it can't save), as the build's pause says, and
        // at its next safe point when the battery runs low.
        use crate::control::{Mode, Pause};
        let (drain, now) = (Pause::new(Mode::Drain, "the menu bar on m4"), Pause::new(Mode::Freeze, "scenic pause on m4"));
        let mode = |c: Conditions, p: Option<&Pause>| stop_for(&n, &c, p).map(|s| s.0);
        assert_eq!(mode(at(true, true, true, None), None), None);
        assert_eq!(mode(at(true, true, true, None), Some(&drain)), Some(Mode::Drain));
        assert_eq!(mode(at(true, true, true, None), Some(&now)), Some(Mode::Freeze));
        assert_eq!(mode(at(true, false, true, None), Some(&drain)), Some(Mode::Freeze));
        assert_eq!(mode(at(false, true, true, Some(25)), None), Some(Mode::Drain));
        assert!(stop_for(&n, &at(true, true, true, None), Some(&drain)).unwrap().1.contains("the menu bar on m4"));
        // An older heartbeat without `home` reads as at home; an older job's `ac` is `cpu`.
        let old: Conditions = serde_json::from_str(r#"{"ac": true, "nas": true, "idle_s": 0}"#).unwrap();
        assert!(old.home);
        let old: Needs = serde_json::from_str(r#"{"ac": true, "nas": true}"#).unwrap();
        assert!(old.cpu && old.nas);
    }
}
