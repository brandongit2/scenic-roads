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
pub mod lead;
pub mod memguard;
pub mod pool;
pub mod recipes;
pub mod rekey;
pub mod room;
pub mod shadow;
pub mod steps;
pub mod tiles;

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

/// The steps the second job takes, in its order of preference (the steps table's: crate::agent::
/// steps); the steps that run alone; those that keep the pass's Wikidata and Wikipedia answers here
/// and on the NAS (crate::answers: none starts while the agent sends them as it starts,
/// `Agent::seed_answers`).
use steps::{ALONE, ANSWERED, SECOND};

/// The last round of publishing, in the agent's folder (build::Round): its jobs read the units of
/// the one under way there (crate::out::UNITS_AS_OF_ENV).
const ROUND_FILE: &str = "round.json";

/// The build's records as they were before the first re-keying that changed them (agent::rekey;
/// the units' keys, 2026-10-06), on the NAS beside them: written once, never over one there, for
/// going back to an app from before the units' keys (README, How it's built; one from before tree
/// cover's pieces needs no copy: it drops the assemblies' records and makes the tree cover again).
pub const REKEY_COPY: &str = "state/build/jobs.pre-rekey.json";

/// The pass's worldwide jobs, as the checklist says them.
const WORLDWIDE: &str = "Preparing the worldwide data: sets, route ends, roads' reach, buildings, summits, labels, water";
const WORLDWIDE_STEPS: [&str; 8] = ["pass-sets", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels", "water"];

/// Whether jobs of steps `a` and `b` can't run at once: either runs alone; both are in one of the
/// steps table's groups of which two never run at once on one Mac (they read the raw tiles here,
/// which a terrain run packs onto the NAS and deletes; they ask Wikidata, each pacing itself as if
/// it were alone; they read gigabytes of the NAS's sources a target); or they're the same step and
/// it isn't a shared one (one job's work: its targets held by nothing else). A shared step's targets
/// are held apart (crate::coord), and each slot has a scratch folder of its own.
fn clash(a: &str, b: &str) -> bool {
    steps::alone(a) || steps::alone(b) || steps::grouped(a, b) || (a == b && !steps::SHARED.contains(&a))
}

/// The free space the second job starts with (it makes no room: `reads_caches`): the reserve.
fn second_need(_step: &str) -> u64 {
    room::RESERVE
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
    /// Why its job stops at its next safe point for the memory guard (crate::agent::memguard):
    /// kept until it ends.
    mem_drain: Option<String>,
    /// That drain is for its job alone past the limit while the Mac isn't short of memory: only
    /// trouble stops it at once, not the time a pause gives a safe point (a long single-target job,
    /// an area's whole terrain run, may have none for hours).
    mem_drain_alone: bool,
    /// Its job is being stopped by the memory guard (its lease ends so: kept from this Mac).
    guard_stopped: bool,
    /// Its job took claim files as it started (crate::agent::claims: a job of this Mac's own, planned
    /// while it wasn't a helper, the lead's with the pool on or the build Mac's with it off): kept
    /// fresh and released by that, whatever part the process plays since.
    claimed: bool,
    /// Its own job's timings, taken as it ended, for the lead when this Mac no longer leads (the
    /// job's hand-off carries them: `end_lease`).
    timings_out: Option<crate::timings::RunRec>,
}

/// How long the first slot may pass over a job that can't share the Mac with the second's (it then
/// waits for it, and the second starts nothing new until it has).
const PASS_MAX: Duration = Duration::from_secs(30 * 60);

/// How long a job the memory guard stopped is kept from this Mac (a member's lead keeps it from it
/// as long, its lease ended as failed).
const GUARD_BACKOFF_S: u64 = 3600;

/// What the memory guard holds of a plan (`Agent::guard_holds`): the targets held here, those held
/// everywhere, those tried again in a job of their own, and why, for the status.
#[derive(Default)]
struct GuardHolds {
    here: BTreeSet<(String, String)>,
    all: BTreeSet<(String, String)>,
    alone: BTreeSet<(String, String)>,
    why: Vec<Waiting>,
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
/// archives copied here and merged); a task's ("tail": a unit's last steps, its files fetched from
/// the coordinator; "bldtile": a 3D buildings' z8 area, its blocks) 5 GB, but tree cover's row of
/// z8 blocks ("treeblock": it reads the squares where they lie, and writes ~80 MB) and a terrain
/// piece's z8 subtrees ("terrainsub": a file of tens of MB, and their tiles) 1 GB; the others' `HELPER_RESERVE` (tree cover's a z6 tile a run: the one to
/// four canopy squares its blocks touch copied here, ~2 GB each, where a z3 tile's whole run copied
/// tens of GB).
fn helper_need(step: &str) -> u64 {
    match step {
        "tail" | "bldtile" => 5 << 30,
        "treeblock" | "terrainsub" => 1 << 30,
        _ => HELPER_RESERVE,
    }
}

/// The work a helper asks for (the shared steps, and "tail", "bldtile", "treeblock" and "terrainsub" for tasks): what its disk has free for
/// (a step's need and its margin), or can have, from the caches it may empty (`free` the disk's free
/// bytes, `cheap` what `make_room` can delete there: room::helper_cheap_bytes). A job granted that
/// still can't have its room once the caches are emptied is given back (`run_once`). With the
/// owner's disk room target (`floor`, room::Target), that much more stays free.
fn helper_steps(free: u64, cheap: u64, floor: u64) -> Vec<String> {
    steps::SHARED.iter().copied().chain(["tail", crate::bld::task::KIND, crate::trees::task::KIND, crate::terrain_task::KIND]).filter(|s| free.saturating_add(cheap) >= floor.saturating_add(helper_need(s) + room::margin(helper_need(s)))).map(str::to_string).collect()
}

/// What a terrain run needs past the others' room: its area's raw tiles held twice while they're
/// packed onto the NAS (loose, then in their archives), and on a run again the area's archives
/// copied here and merged (12 to 15 GB for a z3 area of land). The US's first runs took the build
/// Mac from 34 GB free to 14 GB (2026-10-05). The area's own archive copies, which the run reads at
/// once, are spared (`terrain_reads`).
const TERRAIN_SPACE: u64 = 25 << 30;

/// Whether `p` is an archive copy a terrain job's target (`id`: "terrain 3/x/y", an area's whole
/// run; "terrain 6/x/y", a piece; "terrain-lo 3/x/y", an assembly) reads at once: an area's run its
/// z3 area's own (`3-x-y.…`) and its z6 tiles' (`6-X-Y.…` within it), a piece its z6 tile's, an
/// assembly its z3 area's (crate::rawpack's areas).
fn terrain_reads(id: &str, p: &Path) -> bool {
    let tile = |s: &str| -> Option<(u32, u32, u32)> {
        let mut v = s.split(['/', '-']).map(|t| t.parse::<u32>().ok());
        Some((v.next()??, v.next()??, v.next()??))
    };
    let in_packs = p.parent().and_then(Path::file_name).is_some_and(|d| d == "packs");
    let area = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('.').next()).and_then(tile);
    if !in_packs {
        return false;
    }
    match (id.split_once(' '), area) {
        (Some(("terrain", t)), Some(a)) => match tile(t) {
            Some((3, x, y)) => matches!(a, (3, ax, ay) if (ax, ay) == (x, y)) || matches!(a, (6, ax, ay) if (ax >> 3, ay >> 3) == (x, y)),
            Some((6, x, y)) => a == (6, x, y),
            _ => false,
        },
        (Some(("terrain-lo", t)), Some(a)) => tile(t).is_some_and(|(z, x, y)| z == 3 && a == (3, x, y)),
        _ => false,
    }
}

/// The memory a step's job is expected to take (MB) before one has run for its target and said
/// (`SCENIC_COSTS`): its row's in the steps table. (Units and candidates are offered by their
/// piece's size, crate::coord::job_peak; terrain by its area's size, `terrain_peak`; the 3D
/// buildings by their rows, `bld_peak`.)
fn first_peak(step: &str) -> u64 {
    steps::mem_mb(step)
}

/// The memory a 3D buildings job of a z6 tile is expected to take (MB), before one has said, by the
/// rows of the row groups its normalized file is read from (`rows`: crate::bld::sources): B1's six
/// tiles took 0.3 GB and 160 B a row read for `bldprep`, 0.25 GB and 280 B a building of its
/// largest z8 area for `bldtiles` (Kantō's 10.8 M of 30.3 M; taken as two fifths of the rows).
fn bld_peak(step: &str, rows: u64) -> u64 {
    match step {
        "bldprep" => 300 + rows * 160 / (1 << 20),
        _ => 250 + rows * 2 / 5 * 280 / (1 << 20),
    }
}

/// The memory a terrain run of an area of `z6` tiles near the coverage is expected to take (MB),
/// before one has said: it holds a z6 tile's shaded hi tiles at a time (z9–12, up to 5,440, ~270 KB
/// each until written: crate::terrain_pack::build_q_with) with its z12 repairs while its z11 is made
/// (up to 4,096 at 256 KB), each z6 tile's z9 repairs and quarters (~20 MB) and the area's zoomed-out
/// tiles (z3–8, 1,365), and half a GB besides: 3.3 to 4.6 GB, what a helper spares. (Measured
/// before it wrote a z6 tile at a time: 32.9 GB for 3/0/2's whole area at once.)
/// (A piece: one z6 tile, `terrain_peak(1)`.)
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
    /// A job's while the pool is on (docs/pool.md §7.3), the lead's own (`own`: its coordinator's
    /// lease) or a member's (the lead's): it saves into `dir` (crate::agent::pool::job_dir), and
    /// the agent hands that to the pool's journal as an entry of its lease, `<term>-<id>`.
    Pooled { id: u64, term: u64, dir: PathBuf, own: bool },
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

/// A helper's job's timings' record, in its outbox folder (`SCENIC_TIMINGS`).
const TIMINGS_FILE: &str = "timings.json";

/// The timings' record a helper's job left in its folder `dir`, for its hand-off.
fn read_timings(dir: &Path) -> Option<crate::timings::RunRec> {
    std::fs::read(dir.join(TIMINGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
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
    /// This Mac in the pool, while it's on (docs/pool.md §12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<PoolView>,
    /// The memory guard (crate::agent::memguard): its switch, this Mac's limit, what the jobs hold
    /// now, what it last did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<memguard::View>,
    /// The gate (docs/inputs.md §4.7): an entry per gate unit, its version, whether it's checking
    /// or held, and what holds it (the lead's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<crate::inputs::view::InputView>,
}

/// This Mac in the pool, for the status: its member, its part, what the driver lets it do, the
/// members it knows, its jobs' entries the current lead hasn't acknowledged (none: every one is in
/// its records), and why it restarts once its first job's slot is free.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolView {
    pub member: String,
    pub role: pool::Role,
    pub gates: pool::Gates,
    pub members: Vec<String>,
    #[serde(default)]
    pub unacked: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    /// The pool as the controls show it (crate::agent::lead: docs/pool.md §10, §11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<lead::View>,
    /// Leading: the entries it merged with a change outside their step's write-set (the newest,
    /// `pool::OUTSIDE_KEPT`), and how many since its agent started (crate::agent::steps: reported,
    /// not refused, until the write-sets are enforced).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outside: Vec<pool::Outside>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub outside_n: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
    /// About how long a target of each step takes here (seconds, as its jobs went lately), by
    /// `secs_key`: the forecast's, for the steps no other worker does.
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
    /// The last freeing toward the owner's disk room target (room::toward).
    #[serde(default)]
    toward: Option<room::Freed>,
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

/// The coordinator's port: `crate::coord::PORT`, or another for a test agent beside the real one
/// (`SCENIC_COORD_PORT`; a unit test's own, `TEST_PORT`).
fn coord_port() -> u16 {
    #[cfg(test)]
    if let Some(p) = TEST_PORT.with(|p| p.get()) {
        return p;
    }
    std::env::var("SCENIC_COORD_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(crate::coord::PORT)
}

#[cfg(test)]
thread_local! {
    static TEST_PORT: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
    /// Whether this Mac can lead, whatever its conditions say (the pool's tests).
    static TEST_ABLE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
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
/// ago the last was, and how long the hold has left (at `now`).
fn edit_held(edited: Option<(std::time::SystemTime, std::time::SystemTime)>, now: std::time::SystemTime) -> Option<(Duration, Duration)> {
    let (first, last) = edited?;
    let (run, age) = (now.duration_since(first).ok()?, now.duration_since(last).ok()?);
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
    /// The owner's disk room target (room::Target), read each loop from the agent's folder; the
    /// last freeing toward it started (the target, and when, unix seconds); and the last time a
    /// job's room-making fell short of it (the target and the room it was after, and when): the
    /// jobs after it wait without making room again, until a job ends, the target changes or ten
    /// minutes pass.
    room_target: Option<room::Target>,
    toward_tried: Option<(u64, u64)>,
    floor_short: Option<(u64, u64, Instant)>,
    /// The room (its need and the target) of the last job tried that waited for the target:
    /// `start_first` tries none after it that needs as much.
    floor_held: Option<u64>,
    /// What a freeing under way frees toward (`goal`): set again each loop, so a target lowered or
    /// cleared midway stops it (room::toward reads it as it goes).
    toward_goal: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The memory's total and free MB as a test sets them (`start_second`), so a test doesn't
    /// depend on what the Mac running it has free.
    mem_set: Option<(u64, u64)>,
    /// The memory guard (crate::agent::memguard): its switch as last read (None: not read yet, as
    /// its default), the slot and job the guard drained the one beside (nothing starts beside that
    /// job until it ends), what it last did, for the status; and what each slot's job holds as
    /// a test sets it (MB).
    guard_on: Option<bool>,
    guard_hold: Option<(usize, String)>,
    guard_last: Option<(u64, String)>,
    /// The guard's sampler (its thread started by `run`), this Mac's memory as read this loop (MB;
    /// 0: unknown), and the jobs it stopped, kept from this Mac until when (unix seconds), and why.
    sampler: memguard::Sampler,
    total: u64,
    guard_backoff: BTreeMap<String, (u64, String)>,
    /// The jobs the first slot passed over, each since it was first (`passable`).
    passed_over: BTreeMap<String, Instant>,
    /// The pool's switch for a part changing in this process (`pool::SLOTS`), as last read; the
    /// idle-sleep assertion it holds while it leads with leases out; the app it last handed the lead
    /// on for (a newer one installed, its update waiting on its own job: `hand_for_update`).
    slots_on: bool,
    lead_awake: Option<std::process::Child>,
    /// The NAS's project folder as the loop last found it (a job ending between loops tells the
    /// lead through a client made from it).
    last_root: Option<PathBuf>,
    update_handed: Option<String>,
    /// The Mac it reads its conditions and resources from (a test's fixed one: cond::Mac::TEST).
    pub(crate) mac: cond::Mac,
    sleep: SleepWatch,
    last_mount_try: Option<Instant>,
    /// The heartbeat last written to the NAS (without its time) and when: written again only when it
    /// changes or every five minutes, so an idle NAS can rest.
    last_beat: Option<(Vec<u8>, Instant)>,
    /// How far each region is built, and when that was worked out.
    progress: Option<(Instant, BTreeMap<String, build::RegionState>, Vec<build::Step>)>,
    /// The pass's reaches as last read, by content name (large: read again only when they change).
    reach: std::cell::RefCell<Option<(String, std::rc::Rc<crate::reach::Reaches>)>>,
    /// The terrain packs' indexes, which the units' keys read (`tiles`, kept in `pack-idx/`): those
    /// the manifest names, each read once.
    tiles: std::cell::RefCell<tiles::TerrainTiles>,
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
    /// The work the last plan had left (its steps and targets): what room-making lets go last
    /// (`hints`).
    queued: std::cell::RefCell<Vec<build::Work>>,
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
    /// The pool (docs/pool.md §12, crate::agent::pool): its switch on the NAS as this process
    /// started (None: the NAS wasn't reached then; the agent is as it was until it is, then restarts
    /// if the pool is on or shadowed), its part while it's on, a shadow run beside it, and why this
    /// process restarts once its first job's slot is free (the switch changed).
    pool_mode: Option<pool::Mode>,
    pool: Option<pool::Run>,
    shadow: Option<shadow::Shadow>,
    shadow_failed: bool,
    restart_for: Option<String>,
    /// The switch as the last loop read it, when it differed from this process's: acted on once two
    /// loops in a row read it so (a stat answering wrongly once restarts nothing).
    switch_seen: Option<pool::Mode>,
    /// The gate (crate::inputs): the drop boxes' listing thread, the status's entries as the last
    /// plan made them, what they read, and the units a full check was asked of.
    inputs: Gate,
}

/// The agent's part of the gate (docs/inputs.md §4.2, §4.7).
#[derive(Default)]
struct Gate {
    /// The drop boxes listed off the loop (the lead's, and a lone build Mac's), with the root it
    /// lists.
    watch: Option<(PathBuf, crate::inputs::watch::Watch)>,
    view: std::cell::RefCell<Vec<crate::inputs::view::InputView>>,
    cache: std::cell::RefCell<crate::inputs::view::Cache>,
    full: std::cell::RefCell<BTreeSet<String>>,
    /// The descriptions' credits' digest as last read (None inside: there are none), for the
    /// catalog's key while they can't be read.
    credits: std::cell::RefCell<Option<Option<String>>>,
}

/// The caches' sizes as last counted (room::sizes: what room-making can free, what a clear would),
/// and when that count began.
type CacheCount = (Option<Instant>, Option<room::Sizes>);

/// A trim or a clear under way on a thread of its own (`Agent::caches_task`).
struct CachesTask {
    /// The clear's ask (None: a trim, or a freeing toward the room target), and when it began.
    ask: Option<room::ClearRequest>,
    /// A freeing toward the owner's disk room target (room::toward): the target, and the room it
    /// frees toward as it began (`goal`: the target, or more for a job waiting for it), bytes.
    toward: Option<(u64, u64)>,
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
        // The pool's switch, as this process starts (the NAS answering; else the agent is as it was
        // until it does). On, this Mac's member takes its first step, which says its part: it leads
        // (as the build Mac did) or works as a member (as a helper did), whatever `--helper` says.
        let mut pool_mode = None;
        let mut run: Option<pool::Run> = None;
        let mut first: Option<(crate::pool::driver::Out, PathBuf)> = None;
        if lock.is_some() && !o.dry_run {
            if let Some(r) = o.root.clone().or_else(|| find_root(false)).filter(|r| answers(r)) {
                pool_mode = pool::mode(&r);
                if pool_mode == Some(pool::Mode::On) {
                    let nas: pool::SharedNas = std::sync::Arc::new(crate::pool::nas::Share::new(&r));
                    let locks = o.home.parent().unwrap_or(&o.home).to_path_buf();
                    match pool::Side::open(&o.home, &o.home.join("pool"), &locks, &app, nas, false)? {
                        Some(side) => {
                            let (r1, out) = pool::Run::start(side, &o.home);
                            o.helper = r1.role == pool::Role::Member;
                            eprintln!("agent: the pool is on: this Mac's member {} {} (term {})", r1.side.member().id, if o.helper { "works as a member" } else { "leads" }, out.term);
                            run = Some(r1);
                            first = Some((out, r.clone()));
                        }
                        // (Its lock held by another process: this one starts nothing, and tries the
                        // lock again each loop, restarting into the pool once it has it.)
                        None => eprintln!("agent: another process is this Mac's member in the pool; waiting for its lock"),
                    }
                }
            }
        }
        // The build Mac's agent coordinates (not a helper's, nor a dry run's); in the pool, its lead
        // (with a root of its own too: a test's, on a port of its own, SCENIC_COORD_PORT).
        let pooled = run.is_some();
        let lockless = pool_mode == Some(pool::Mode::On) && run.is_none();
        let coord = if !o.helper && lock.is_some() && !o.dry_run && !lockless && (o.root.is_none() || pooled) { make_coordinator(&o, run.as_ref(), &app, &cond::host_name()) } else { None };
        if let (Some(r), Some((out, root))) = (run.as_mut(), &first) {
            pool::took_up(r, coord.as_ref(), out, &cond::host_name(), &[]);
            // (The terms' events a member's process before this one kept, for this coordinator's
            // history; and this first step's.)
            if let Some(c) = &coord {
                lead::replay(&o.home.join("pool"), c);
            }
            lead::after(r, out, root, &o.home, coord.as_ref());
        }
        // The build's pause as this agent last knew it (a helper that can't reach the build Mac stays
        // as it was).
        let pause: Option<crate::control::Pause> = std::fs::read(o.home.join("pause.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        // (A round's file that doesn't read: none under way, the next begins afresh.)
        let round: Option<build::Round> = std::fs::read(o.home.join(ROUND_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let tiles = std::cell::RefCell::new(tiles::TerrainTiles::new(Some(o.home.join("pack-idx"))));
        Ok(Agent { host: cond::host_name(), app, started: now_s(), mem, slots: Default::default(), beside_why: None, free_set: None, room_target: None, toward_tried: None, floor_short: None, floor_held: None, toward_goal: Default::default(), mem_set: None, guard_on: None, guard_hold: None, guard_last: None, sampler: memguard::Sampler::new(SLOTS), total: 0, guard_backoff: BTreeMap::new(), passed_over: BTreeMap::new(), slots_on: false, lead_awake: None, last_root: None, update_handed: None, mac: cond::Mac::Real, sleep: SleepWatch::default(), last_mount_try: None, last_beat: None, progress: None, reach: Default::default(), tiles, coverage: Default::default(), edits: Default::default(), edited_at: Default::default(), _lock: lock, o, me, piece_sizes: Default::default(), claims_dropped: false, writer_named: None, planned: None, queued: Default::default(), merged: None, coord, published: None, client: None, cheap: None, last_catalog: Default::default(), ready: Default::default(), pause, pause_local: false, mirrored: None, pause_pushed: false, orphan_done: Vec::new(), cache_size: Default::default(), heard: None, caches_task: None, trim_failed: None, answers_seed: None, answers_seeded: false, helper_caches: BTreeMap::new(), orphans: Vec::new(), last_cond: None, forecast: Default::default(), catalog_seen: Default::default(), round: std::cell::RefCell::new(round), pool_mode, pool: run, shadow: None, shadow_failed: false, restart_for: None, switch_seen: None, inputs: Gate::default() })
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
        // (In the pool a job's folder from before it is drained into the journal: tasks' alone here.)
        let pooled = self.pool.is_some();
        let dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && Some(p) != running.as_ref() && !(pooled && p.join("work.json").exists())).collect();
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
                    let d = crate::coord::Done { lease, outputs: serde_json::from_value(t["outputs"].clone())?, removed: serde_json::from_value(t["removed"].clone())?, secs: t["secs"].as_f64().unwrap_or(0.0), peak_mb: t["peak_mb"].as_u64().unwrap_or(0), timings: read_timings(&d), ..Default::default() };
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
                        client.done(&crate::coord::Done { lease, handoff: Some(h), costs, failed: r["failed"].as_bool() == Some(true), timings: read_timings(&d), ..Default::default() })
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
            "terrain" | "terrain-lo" | "terrain-root" => vec!["--raw".into(), s(&cache.join("aws-terrarium"))],
            "pois" | "marks" | "stations" | "overlays" => vec!["--pass".into(), date.to_string()],
            "ferries" | "rail-feeds" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem()],
            // (bld-fetch's coverage, and bldtiles' countries, from the pass's outlines.)
            "bld-fetch" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem()],
            "bldprep" => vec!["--dem".into(), dem()],
            "bldtiles" => vec!["--pass".into(), date.to_string()],
            "terrain-water" => vec!["--pass".into(), date.to_string()],
            "items" | "heritage-sites" | "heritage" | "rail" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem(), "--cache".into(), s(&cache)],
            "peaks" => vec!["--pass".into(), date.to_string(), "--raw".into(), s(&cache.join("aws-terrarium")), "--cache".into(), s(&cache), "--coarse-threads".into(), "6".into()],
            "unit" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem(), "--cache-dir".into(), s(&cache)],
            "trees" => vec!["--pass".into(), date.to_string(), "--dem".into(), dem(), "--chm".into(), s(&cache.join("chm10"))],
            "trees-lo" => vec!["--pass".into(), date.to_string()],
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
        let needs = Needs { nas: true };
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
        // (A job given back for the owner's disk room target lately: no work asked for until a job
        // ends, the target changes or ten minutes pass; the caches are freed toward it meanwhile.)
        if let Some((t, n, at)) = self.floor_short.filter(|f| f.0 == self.floor() && f.2.elapsed() < Duration::from_secs(600)) {
            if room::disk_free(&self.o.home).unwrap_or(0) < n {
                waiting.push(Waiting { step: None, what: "Building".into(), why: format!("a job was given back for the disk room target ({}) {} min ago: asking for work again once the disk has room past it, or in ten minutes", room::size(t), at.elapsed().as_secs() / 60) });
                return Vec::new();
            }
        }
        let can = helper_steps(room::disk_free(&self.o.home).unwrap_or(0), cheap, self.floor());
        if can.is_empty() {
            let need = helper_need(crate::trees::task::KIND);
            let floor = self.floor();
            let past = if floor > 0 { format!(", past the disk room target's {}", room::size(floor)) } else { String::new() };
            waiting.push(Waiting { step: None, what: "Building".into(), why: format!("the disk has too little room ({:.1} GB free needed{past}, with what its caches can free)", (need + room::margin(need)) as f64 / (1u64 << 30) as f64) });
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
            limit_mb: Some(memguard::limit_mb(self.total_mb())).filter(|&l| l > 0),
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
            Ok(Some(crate::coord::Grant { lease, term, work: crate::coord::Granted::Job { step, targets, pass }, .. })) if steps::SHARED.contains(&step.as_str()) => {
                // (In the pool, its folder is its lease's, `<term>-<lease>`, handed to the journal.)
                let pooled = self.pool.is_some().then_some(crate::pool::journal::LeaseId { term, n: lease });
                let dir = match pooled {
                    Some(l) => pool::job_dir(&self.o.home, l),
                    None => self.outbox().join(lease.to_string()),
                };
                // (Its step and targets, kept with its saves: should this agent stop, the next hands
                // off what of them it finished.)
                let work = match pooled {
                    Some(l) => serde_json::to_vec(&pool::JobKept { step: step.clone(), targets: targets.clone(), lease: l }),
                    None => serde_json::to_vec(&build::Work { step: step.clone(), targets: targets.clone() }),
                };
                let kept = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("work.json"), work.unwrap_or_default()));
                if let Err(e) = kept {
                    waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e}") });
                    fail(self, lease, &format!("its outbox: {e}"));
                    return Vec::new();
                }
                let s = |p: &Path| p.to_string_lossy().into_owned();
                // The build Mac's own command for it, its saves handed off (SCENIC_HANDOFF).
                let mut cmd = vec!["/usr/bin/env".to_string(), format!("SCENIC_HANDOFF={}", dir.display()), format!("SCENIC_COSTS={}", dir.join("costs.jsonl").display()), format!("{}={}", crate::timings::TIMINGS_ENV, dir.join(TIMINGS_FILE).display())];
                cmd.extend([s(&self.o.bin.join("scenic-build")), step.clone(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch").join(&step))]);
                cmd.extend(targets.iter().map(|t| t.0.clone()));
                cmd.extend(self.step_args(&step, &pass));
                // (Pieces made again as they are, by the build's records as read: expected the
                // same. Records that can't be read now expect nothing.)
                if matches!(step.as_str(), "trees" | "terrain" | "slope") {
                    let same = build::Keys::load_with_handoffs(root).map(|k| expect_same(&build::Work { step: step.clone(), targets: targets.clone() }, &self.as_read(root, k))).unwrap_or_default();
                    if !same.is_empty() {
                        cmd.extend(["--expect-same".to_string(), same.join(",")]);
                    }
                }
                let n = targets.len();
                let id = format!("{step} {}", targets.first().map(|t| t.0.as_str()).unwrap_or(""));
                let what = format!("{} ({n} {}{}, for the build Mac)", build::label(&step), if matches!(step.as_str(), "trees" | "terrain" | "slope") { "tile" } else { "area" }, if n == 1 { "" } else { "s" });
                self.slots[0].lease = Some(match pooled {
                    Some(l) => Held::Pooled { id: lease, term: l.term, dir, own: false },
                    None => Held::Leased { lease, dir },
                });
                vec![JobSpec { id, what, cmd, needs, restart_after_sleep: true, record: Some(build::Work { step, targets }) }]
            }
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Task { id, task, .. }, .. })) => {
                // A task (a unit's tail, or a 3D buildings' z8 area, for the build Mac's job), run here
                // by `scenic run-task` over its files fetched from the coordinator, reading the NAS's
                // data where it lies (only reading it: crate::unit::Tools::stores_read_only).
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
                    format!("{}={}", crate::timings::TIMINGS_ENV, dir.join(TIMINGS_FILE).display()),
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
                let what = if task["runs"][0]["prog"] == crate::bld::task::KIND {
                    format!("3D buildings of the build Mac's area {unit}")
                } else if task["runs"][0]["what"] == crate::trees::task::KIND {
                    format!("Tree cover of the build Mac's blocks {}", task["blocks"].as_str().unwrap_or(&unit))
                } else if task["runs"][0]["what"] == crate::terrain_task::KIND {
                    format!("Terrain of the build Mac's subtrees {}", task["subtrees"].as_str().unwrap_or(&unit))
                } else {
                    format!("Scenery for the build Mac's area {unit}")
                };
                vec![JobSpec { id: format!("task {id}"), what, cmd, needs: Needs { nas: true }, restart_after_sleep: false, record: None }]
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
        // What its targets took at least, as the guard's sampler saw them (crate::agent::memguard),
        // but those its run measured: learned whatever the guard does.
        // (The lead's client, made again when it isn't there, as `beat` makes it: a lead a moment
        // ago has none, its job's end told to the new lead all the same.)
        if self.client.is_none() && self.coord.is_none() && matches!(self.slots[k].lease, Some(Held::Pooled { .. } | Held::Leased { .. })) {
            if let Some(r) = self.last_root.clone() {
                self.client(&r, &mut Vec::new());
            }
        }
        let watch = self.sampler.unwatch(k).unwrap_or_default();
        let floors_of = |costs: &[(String, crate::coord::Cost)]| watch.floors(&costs.iter().map(|(u, _)| u.clone()).collect::<Vec<_>>());
        let guard_stopped = self.slots[k].guard_stopped;
        // An own job's timings, taken whichever way it ends (none left for the slot's next job).
        let own_timings = self.slots[k].timings_out.take();
        match self.slots[k].lease.take() {
            Some(Held::Own(id)) => {
                if let Some(c) = &self.coord {
                    c.finish(id, done);
                    if let Some(p) = pid {
                        c.close_tasks(p);
                    }
                    let costs = self.costs_path(k);
                    let measured = read_costs(&costs);
                    c.add_floors(&floors_of(&measured));
                    c.add_costs_by(&measured, &self.worker_of(k));
                    std::fs::remove_file(costs).ok();
                }
            }
            Some(Held::Pooled { id, dir, own, .. }) => {
                // Its hand-off (its saves, the targets it finished, whatever stopped it) to the
                // journal, by the pool's next step; its folder kept until a saved state holds it.
                let kept: Option<pool::JobKept> = std::fs::read(dir.join("work.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let costs = read_costs(&if own { self.costs_path(k) } else { dir.join("costs.jsonl") });
                let entry = match (&kept, self.pool.as_ref()) {
                    (Some(w), Some(run)) => match pool::entry_of(&dir, &run.side.member().id, w.lease, &w.step, done, now_s()) {
                        Ok(e) => e,
                        Err(e) => {
                            eprintln!("agent: {}'s hand-off can't be read ({e:#}); its work is done again", dir.display());
                            None
                        }
                    },
                    _ => None,
                };
                // (Through the lead wherever it is now, as `beat` does.)
                if self.coord.is_some() {
                    if let Some(c) = &self.coord {
                        c.finish(id, done);
                        if let Some(p) = pid {
                            c.close_tasks(p);
                        }
                        c.add_floors(&floors_of(&costs));
                        c.add_costs_by(&costs, &self.worker_of(k));
                        std::fs::remove_file(self.costs_path(k)).ok();
                    }
                } else if let Some(c) = &self.client {
                    // (Its lease ended with the lead: its targets kept out of offers until merged.
                    // Best effort: unanswered, the lease lapses, and the merged records say they're
                    // built.)
                    // (One the memory guard stopped ends as failed: its lead keeps its targets from this
                    // Mac for an hour, doubling.)
                    let failed = outcome == Outcome::Failed || guard_stopped;
                    let floors = floors_of(&costs);
                    let r = match entry.as_ref().filter(|e| e.handoff.done.is_some()) {
                        Some(e) => c.done(&crate::coord::Done { lease: id, handoff: Some(e.handoff.clone()), costs, floors, failed, journaled: true, timings: if own { own_timings } else { read_timings(&dir) }, ..Default::default() }).map(|_| ()),
                        None if matches!(outcome, Outcome::Paused | Outcome::Interrupted) && !guard_stopped => c.give_back_with(id, note, &floors),
                        None => c.fail_with(id, note, None, &floors),
                    };
                    if let Err(e) = r {
                        eprintln!("agent: telling the lead lease {id} ended: {e:#} (it lapses)");
                    }
                }
                match (entry, self.pool.as_mut()) {
                    (Some(e), Some(run)) => run.entries.push((e, Some(dir))),
                    // (Nothing to hand off: the folder goes; no pool any more, it waits for the next
                    // process, which hands it off.)
                    (None, Some(_)) => {
                        std::fs::remove_dir_all(&dir).ok();
                    }
                    _ => {}
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
                // (One the memory guard stopped isn't interrupted: kept from this Mac for a while.)
                let interrupted = !ok && matches!(outcome, Outcome::Paused | Outcome::Interrupted) && !guard_stopped;
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
            // (Its lease through the lead wherever it is now: this process's coordinator while it
            // leads, the lead's by HTTP while it's a member, whoever granted it: docs/pool.md §7.6.)
            Some(Held::Own(id) | Held::Pooled { id, .. }) if self.coord.is_some() => {
                let Some(c) = &self.coord else { return true };
                if c.renew(id, progress) {
                    return true;
                }
                let work = self.slots[k].running.as_ref().and_then(|r| r.spec.record.clone());
                match work.and_then(|w| c.hold(&w.step, &w.targets)) {
                    Some(id) => {
                        // (Held again under a new id; its hand-off keeps the lease it began under,
                        // its files where it writes them.)
                        self.slots[k].lease = Some(match self.slots[k].lease.take() {
                            Some(Held::Pooled { term, dir, own, .. }) => Held::Pooled { id, term, dir, own },
                            _ => Held::Own(id),
                        });
                        true
                    }
                    None => false,
                }
            }
            Some(Held::Own(_)) => true,
            Some(Held::Leased { lease, .. } | Held::Pooled { id: lease, .. }) => {
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
        // (A job that took none, a leased one's, holds no claim files.)
        if !std::mem::take(&mut self.slots[k].claimed) {
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
        // (A job that took none, a leased one's, holds no claim files: its lease is kept by beats.)
        if !self.slots[k].claimed {
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

    /// Where slot `k`'s job leaves its timings' record (`SCENIC_TIMINGS`).
    fn timings_path(&self, k: usize) -> PathBuf {
        self.o.home.join(if k == 0 { "timings-run.json" } else { "timings-run-2.json" })
    }

    /// Slot `k`'s ended job's timings (crate::timings): kept in this Mac's log (`timings.jsonl`),
    /// and in the build's, its coordinator's (`coord/timings.jsonl`), here; a helper's go with its
    /// hand-off (`send_outbox`, `end_lease`), from its folder.
    fn take_timings(&mut self, k: usize) {
        // (By where the job writes them, whoever leads now: a member's leased job in its folder,
        // this Mac's own in the agent's.)
        let handed = match &self.slots[k].lease {
            Some(Held::Leased { dir, .. } | Held::Pooled { dir, own: false, .. }) => Some(dir.join(TIMINGS_FILE)),
            _ => None,
        };
        let rec = match &handed {
            // (Read, not taken: it goes with the hand-off.)
            Some(_) => handed.as_deref().and_then(|p| p.parent()).and_then(read_timings).map(|r| crate::timings::RunRec { host: self.worker_of(k), ..r }),
            None => crate::timings::take_record(&self.timings_path(k), &self.worker_of(k)),
        };
        let Some(rec) = rec else { return };
        if let Err(e) = crate::timings::append(&self.o.home.join("timings.jsonl"), &rec) {
            eprintln!("agent: keeping a job's timings: {e}");
        }
        // (With a coordinator here, its lease ends here, `end_lease`: its timings kept here too; this
        // Mac's own job's without one, a lead a moment ago, go to the lead with its hand-off.)
        match (&self.coord, &handed) {
            (Some(c), _) => c.add_timings(&rec),
            (None, None) => self.slots[k].timings_out = Some(rec),
            (None, Some(_)) => {}
        }
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

    /// The owner's disk room target on this Mac (bytes; 0: none): what stays free past every job's
    /// own room (room::Target).
    fn floor(&self) -> u64 {
        self.room_target.as_ref().map_or(0, |t| t.bytes)
    }

    /// The target as a job of `step` keeps it: none for the daily backup and GC (they read no
    /// cache and write little here; the NAS's backup doesn't stop for this Mac's disk).
    fn floor_for(&self, step: &str) -> u64 {
        if matches!(step, "backup" | "gc") {
            0
        } else {
            self.floor()
        }
    }

    /// What the caches are freed toward (0: nothing): the target, or, while a job waits for it
    /// (`floor_short`), the room it needs past it, when more.
    fn goal(&self) -> u64 {
        let target = self.floor();
        match self.floor_short {
            Some((t, n, _)) if target > 0 && t == target => target.max(n),
            _ => target,
        }
    }

    /// The owner's disk room target in a waiting's words: ", the disk room target 100.0 GB kept
    /// free past it"; none without one.
    fn floor_words(&self) -> String {
        match self.floor() {
            0 => String::new(),
            f => format!(", the disk room target's {} kept free past it", room::size(f)),
        }
    }

    /// Why a job that needs `need` free waits for the owner's disk room target.
    fn floor_why(&self, need: u64) -> String {
        format!(
            "waits for the disk room target: it needs {} free past the target's {} ({} free now), more than this Mac's caches can free now; it starts once the target is lowered or off (`scenic room`), or the disk has room",
            room::size(need),
            room::size(self.floor()),
            room::size(self.disk_free())
        )
    }

    /// The free space on this Mac's disk (where the agent's folder is).
    fn disk_free(&self) -> u64 {
        self.free_set.unwrap_or_else(|| room::disk_free(&self.o.home).unwrap_or(0))
    }

    /// Whether every job running here holds the cache files it uses (store::cachefile), so the
    /// caches may lose files meanwhile: none runs that an earlier agent left (`orphans`), whose
    /// programs may be an app's from before the locks. This agent's own jobs run its app's.
    fn locks_kept(&mut self) -> bool {
        self.orphans.retain(jobs::Group::is_the_jobs);
        self.orphans.is_empty()
    }

    /// What the jobs queued here and running read of the caches (room::Hints): what room-making lets
    /// go last.
    fn hints(&self) -> room::Hints {
        let mut h = room::Hints::default();
        let running = self.slots.iter().filter_map(|s| s.running.as_ref()).filter_map(|r| r.spec.record.clone());
        for w in self.queued.borrow().iter().cloned().chain(running) {
            h.add(&w.step, &w.targets.into_iter().map(|t| t.0).collect::<Vec<_>>());
        }
        for r in self.slots.iter().filter_map(|s| s.running.as_ref()) {
            if let Some(t) = r.spec.id.strip_prefix("terrain ") {
                h.add("terrain", &[t.to_string()]);
            }
            if let Some(st) = step_of(&r.spec.id) {
                h.add(&st, &[]);
            }
        }
        h
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
        // The memory guard's sampler, on a thread of its own (crate::agent::memguard), its latest
        // sample written for the status whatever this loop waits on.
        self.sampler.write_to(self.o.home.join(memguard::LIVE));
        self.sampler.spawn();
        if self._lock.is_some() {
            // (What they finished, recorded once the NAS answers: a helper's goes back with its
            // lease's outbox, send_outbox.)
            for k in 0..SLOTS {
                let orphan = jobs::stop_orphan(&self.record_path(k));
                self.orphans.extend(orphan.left);
                if let Some((w, file)) = orphan.done {
                    let names = crate::control::read_done(&file, &w.step);
                    let done: Vec<(String, String)> = w.targets.into_iter().filter(|(t, _)| names.contains(t)).collect();
                    if !done.is_empty() && !self.o.helper && self.pool.is_none() {
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
            // The pool's switch changed, or this process's part in it: restarting into it.
            if let (None, Some(why)) = (&self.slots[0].running, self.pool_restart()) {
                eprintln!("agent: {why}; restarting");
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
                // (By what the job took, whatever part the process plays since.)
                if let (Some((step, ts)), Some(root), true) = (shared_targets(&r.spec), &root, std::mem::take(&mut self.slots[k].claimed)) {
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
        let home = self.mac.at_home();
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
        if root.is_some() {
            self.last_root = root.clone();
        }
        // The owner's disk room target, as set now (`scenic room`, the menu bar).
        let target = room::target(&self.o.home);
        if target.as_ref().map(|t| t.bytes) != self.room_target.as_ref().map(|t| t.bytes) {
            match &target {
                Some(t) => eprintln!("agent: the disk room target is {} ({}): its caches freed to it, and no job starts that would cross it", room::size(t.bytes), t.by),
                None => eprintln!("agent: the disk room target is off"),
            }
            (self.toward_tried, self.floor_short) = (None, None);
        }
        self.room_target = target;
        // (A freeing under way frees toward the goal as it is now: lowered or off, it stops.)
        if self.caches_task.as_ref().is_some_and(|t| t.toward.is_some()) {
            self.toward_goal.store(self.goal(), std::sync::atomic::Ordering::Relaxed);
        }
        let (ac, battery) = self.mac.power();
        let c = Conditions { ac, battery, nas: root.is_some(), home, idle_s: self.mac.idle_seconds() };
        self.note_conditions(&c, slept);
        self.pool_switch(root.as_deref());
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

        // The running jobs; then what they hold together, against this Mac's limit (the memory
        // guard: crate::agent::memguard), its switch as the NAS says.
        if let Some(on) = root.as_deref().and_then(memguard::on) {
            if self.guard_on != Some(on) {
                eprintln!("agent: the memory guard is {}", if on { "on" } else { "off" });
            }
            self.guard_on = Some(on);
        }
        self.total = self.mac_total_mb();
        for k in 0..SLOTS {
            ended |= self.tend(k, &c, root.as_deref(), slept)?;
        }
        ended |= self.guard(root.as_deref());
        // (A job ended: room for the target may be made again.)
        if ended {
            self.floor_short = None;
        }
        // The pool's step, once the jobs that ended handed their work over (docs/pool.md §12):
        // what this Mac may do now, as its lead or a member.
        let gates = self.pool_step(root.as_deref(), &c, &mut waiting);
        self.lead_awake();
        // (Leading, nothing new starts while the driver says it may not: settling a handover, its
        // view not fresh. Restarting into a new part, nothing either.)
        let pool_holds: Option<String> = match (&gates, self.pool_restart()) {
            (_, Some(why)) => Some(format!("restarting: {why}")),
            (None, None) if self.pool_mode == Some(pool::Mode::On) => Some("another process is this Mac's member in the pool: waiting for its lock".into()),
            (Some(g), None) if !self.o.helper && !g.duties => Some(if g.settle { "the lead is handing the build over".into() } else if g.leads.is_some() { "the lead re-asserts its term first".into() } else { "this Mac no longer leads the build".into() }),
            _ => None,
        };
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
            if !self.o.helper && self.pool_off() && self.writer_named.is_none_or(|t| t.elapsed() >= Duration::from_secs(300)) {
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
        // (In the pool, only while this Mac leads: a lead that stepped down takes its contact off,
        // so members find the new lead's.)
        let leading = self.pool.as_ref().is_none_or(|p| p.gates.leads.is_some());
        if let (Some(c), Some(r), false) = (&self.coord, &root, leading) {
            if self.published.is_some() {
                c.unpublish(r);
                self.published = None;
            }
        }
        if let (Some(c), Some(r), true) = (&self.coord, &root, leading) {
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
        if let (Some(r), false, true, true, true) = (root.as_ref(), self.o.helper, self._lock.is_some() && !self.o.dry_run, merge_due, self.pool_off()) {
            match crate::handoff::merge_from(r, &self.o.home.join("scratch/handoff"), &self.handoff_bases(r)) {
                Ok(0) => {}
                Ok(n) => eprintln!("agent: merged {n} hand-off{} from other workers", if n == 1 { "" } else { "s" }),
                Err(e) => eprintln!("agent: merging other workers' hand-offs: {e:#}"),
            }
            self.merged = Some(Instant::now());
            // Then the records re-keyed where a key scheme changed (agent::rekey; once done, and a
            // merged record of an older app's job translated, it finds nothing).
            match self.rekey_records(r) {
                Ok(Some(k)) if k.changed() => eprintln!(
                    "agent: re-keyed the records: units {} moved to their new keys ({} without outputs), {} to build again; tree cover's z3 tiles {} recorded as their pieces and assemblies, {} dropped ({})",
                    k.moved.len(),
                    k.empty.len(),
                    k.left.len(),
                    k.trees_moved.len(),
                    k.trees_dropped.len(),
                    k.trees_dropped.iter().map(|(q, why)| format!("{q}: {why}")).collect::<Vec<_>>().join("; ")
                ),
                Ok(_) => {}
                Err(e) => eprintln!("agent: re-keying the records: {e:#}"),
            }
        }

        // The gate: the drop boxes listed off the loop (the lead's), and the asks of it from this
        // Mac's menu bar, map, `scenic inputs` and (leading) the build page.
        if let Some(r) = root.as_deref() {
            self.tend_gate(r);
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
        if let Some(why) = &pool_holds {
            waiting.push(Waiting { step: None, what: "Building".into(), why: why.clone() });
        }
        // (A trim, a clear or a freeing toward the room target under way: jobs start meanwhile, what
        // they use held, store::cachefile.)
        if idle && !newer && self.pause.is_none() && pool_holds.is_none() {
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
            } else if pool_holds.is_some() {
                self.beside_why = pool_holds;
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
        // The owner's disk room target, while the disk is short of it and stays so.
        if let Some(why) = self.room_short(root.is_some()) {
            waiting.push(Waiting { step: None, what: "The disk room target".into(), why });
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
            caches: Some(self.caches_view(caches_why, c.home, c.nas)),
            pool: self.pool.as_ref().map(|p| PoolView { member: p.side.member().id.clone(), role: p.role, gates: p.gates.clone(), members: p.side.members().iter().cloned().collect(), unacked: p.side.driver().mine().to_tell(p.gates.term).len(), restart: p.restart.clone(), lead: p.controls.view.clone(), outside: p.outside.iter().cloned().collect(), outside_n: p.outside_n }),
            memory: Some(self.guard_view()),
            inputs: if self.o.helper { Vec::new() } else { self.inputs.view.borrow().clone() },
        };
        let body = serde_json::to_vec_pretty(&status)?;
        if let Some(sh) = self.shadow.as_mut() {
            sh.tick();
        }
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
            if ok && of > 0 && !step.is_empty() && k == 0 {
                let each = secs as f64 / of as f64;
                let e = self.mem.step_secs.entry(secs_key(&step)).or_insert(each);
                *e = 0.7 * *e + 0.3 * each;
            }
            if ok && step == "catalog" {
                let ready = self.ready.borrow().clone();
                self.note(crate::coord::history::Event { worker: Some(self.host.clone()), step: Some(step.clone()), targets: ready, ..crate::coord::history::Event::new("catalog") });
            }
            let outcome = if ok { Outcome::Done } else if paused { Outcome::Paused } else { Outcome::Failed };
            // (The coordinator's record of what's done: a helper's whatever it did, this Mac's
            // once it's in the keys.)
            let handed = if self.o.helper || recorded || self.pool.is_some() { done.clone() } else { Vec::new() };
            // (In the pool a catalog's done record is handed off, not recorded: its round is over.)
            if self.pool.is_some() && ok && matches!(step.as_str(), "catalog" | "catalog-held") {
                self.end_round();
            }
            self.take_timings(k);
            self.end_lease(k, outcome, &handed, &note);
            self.finished(&id, &what, ok || paused, secs, note);
            self.slots[k].drain_since = None;
            self.slots[k].mem_drain = None;
            self.slots[k].mem_drain_alone = false;
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
        // (The memory guard's drain holds whatever else stops it: asked through its channel, timed
        // from then, so a job frozen meanwhile, by a pause or a condition, is stopped once a pause
        // would have frozen it, `Agent::guard`.)
        if let Some(why) = &slot.mem_drain {
            if slot.drain_since.is_none() {
                eprintln!("agent: {} stops at its next safe point: {why}", r.spec.id);
                if let Err(e) = std::fs::write(&control, b"drain") {
                    eprintln!("agent: asking {} to stop: {e}", r.spec.id);
                }
                slot.drain_since = Some(Instant::now());
            }
        }
        match stop_for(&r.spec.needs, c, pause.as_ref()).or_else(|| slot.mem_drain.clone().map(|why| (crate::control::Mode::Drain, why))) {
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
                if slot.mem_drain.is_none() && slot.drain_since.take().is_some() {
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
    /// own; one that waits for a reason of its own
    /// passed over, one that waits for the job beside it (`waits_for_second`) waited for, not passed
    /// over. Why the jobs before it wait, in `waiting`.
    fn start_first(&mut self, plan: &[JobSpec], c: &Conditions, root: Option<&Path>, waiting: &mut Vec<Waiting>) {
        let beside = self.slots[1].running.as_ref().map(|r| r.spec.id.clone());
        // (The least room a job held by the disk room target needed: one after it needing as much
        // waits too, unsaid; one needing less is tried.)
        let mut held = u64::MAX;
        self.floor_held = None;
        let mut passed: Vec<String> = Vec::new();
        for spec in plan {
            if beside.as_deref() == Some(spec.id.as_str()) {
                continue;
            }
            if self.need_of(0, spec).saturating_add(self.floor_for(&step_of(&spec.id).unwrap_or_default())) >= held {
                continue;
            }
            if let Some(why) = self.wait_reason(spec, c) {
                waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
                continue;
            }
            if let Some(why) = self.waits_for_second(spec) {
                waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
                // (One that only can't share the Mac with the job beside it, not passed over for long:
                // the next that can start starts meanwhile; it starts once the clash ends.)
                if self.passable(spec) {
                    passed.push(spec.id.clone());
                    continue;
                }
                break;
            }
            if self.try_start(0, spec.clone(), c, root, waiting) {
                break;
            }
            if let Some(h) = self.floor_held.take() {
                held = held.min(h);
            }
        }
        // (When each was first passed over, kept while it still is.)
        let now = Instant::now();
        self.passed_over.retain(|id, _| passed.contains(id));
        for id in passed {
            self.passed_over.entry(id).or_insert(now);
        }
    }

    /// Whether the first slot may pass over `spec` for the next job in plan order that can start:
    /// it can't start beside the second slot's job only because the two can't share the Mac (one of
    /// the steps table's groups: two Wikidata steps, two raw-tile readers, two heavy NAS readers;
    /// not a job that runs alone, which drains the Mac, nor one short of room or memory), and it
    /// hasn't been passed over for `PASS_MAX` (then the slot waits for it, and the second starts
    /// nothing new until it has: `starving`).
    fn passable(&self, spec: &JobSpec) -> bool {
        let Some(r) = self.slots[1].running.as_ref() else { return false };
        let (s, b) = (step_of(&spec.id).unwrap_or_default(), step_of(&r.spec.id).unwrap_or_default());
        let only_grouped = steps::grouped(&s, &b) && !steps::alone(&s) && !steps::alone(&b) && self.guard_held(1).is_none();
        only_grouped && self.passed_over.get(&spec.id).is_none_or(|t| t.elapsed() < PASS_MAX)
    }

    /// A job of the plan the first slot has passed over for `PASS_MAX` (the second slot then starts
    /// nothing new but it, so it starts as soon as the job in its way ends; one not in the plan any
    /// more, done or waiting for a reason of its own, counts for nothing).
    fn starving(&self, plan: &[JobSpec]) -> Option<&String> {
        self.passed_over.iter().find(|(id, t)| t.elapsed() >= PASS_MAX && plan.iter().any(|p| p.id == **id)).map(|(id, _)| id)
    }

    /// Why `spec` can't start now for itself: a condition it needs gone, or a failure's wait.
    fn wait_reason(&self, spec: &JobSpec, c: &Conditions) -> Option<String> {
        if let Some(why) = lapsed(&spec.needs, c) {
            return Some(why);
        }
        // (A job of no targets of its own, the OSM pass's: held by the memory guard when what it
        // held before passes this Mac's limit, the targets of a job with them dropped from the plan
        // already, `guard_holds`.)
        let limit = memguard::limit_mb(self.total_mb());
        if let (None, Some(co), true) = (spec.record.as_ref(), self.coord.as_ref(), self.guard_stops() && limit > 0) {
            let (step, rest) = spec.id.split_once(' ').unwrap_or((spec.id.as_str(), spec.id.as_str()));
            if let Some(f) = co.holding_floor(step, rest).filter(|f| f.mb > limit) {
                return Some(format!("needs about {:.1} GB, past this Mac's limit of {:.1} GB, and no other Mac runs it: for the owner to see to (`scenic pool floors --clear` once it's fixed)", f.mb as f64 / 1024.0, limit as f64 / 1024.0));
            }
        }
        // (One the memory guard stopped, kept from this Mac an hour.)
        if let Some((until, why)) = self.guard_backoff.get(&spec.id).filter(|(u, _)| now_s() < *u && !self.o.helper) {
            return Some(format!("{why}; tried here again in {} min", (until - now_s()).div_ceil(60)));
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
        for spec in plan.iter().filter(|s| !running.contains(&s.id.as_str())) {
            match self.wait_reason(spec, c).or_else(|| self.waits_for_second(spec)) {
                Some(why) => waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why }),
                None => break,
            }
        }
    }

    /// The job the first slot would start next: the plan's first that isn't running, left to the
    /// second job, or waiting for a reason of its own.
    fn head<'a>(&self, plan: &'a [JobSpec], c: &Conditions) -> Option<&'a JobSpec> {
        let running: Vec<&str> = self.slots.iter().filter_map(|s| s.running.as_ref().map(|r| r.spec.id.as_str())).collect();
        plan.iter().find(|s| !running.contains(&s.id.as_str()) && self.wait_reason(s, c).is_none())
    }

    /// Why `spec` can't start in the first slot beside the second job's, when it can't: they don't
    /// run together (`clash`); it needs room made on the disk, which isn't while a second job runs
    /// (it may read what's deleted, and room-making holds the loop for minutes); or the two wouldn't
    /// fit three quarters of the memory. The first slot then waits for it rather than start later
    /// work, and the second starts nothing new meanwhile (`start_second`): neither starves it.
    fn waits_for_second(&self, spec: &JobSpec) -> Option<String> {
        if let Some(id) = self.guard_held(1) {
            return Some(format!("waits for the job beside it ({id}) to end: the jobs here held more memory together than this Mac's limit lately"));
        }
        let r = self.slots[1].running.as_ref()?;
        let (s, b) = (step_of(&spec.id).unwrap_or_default(), step_of(&r.spec.id).unwrap_or_default());
        let beside = build::label(&b);
        if clash(&s, &b) {
            return Some(format!("waits for the job beside it ({beside}) to end: they don't run together"));
        }
        let (need, free) = (self.need_of(0, spec).saturating_add(self.floor_for(&s)), self.disk_free());
        if free < need {
            return Some(format!("needs {} GB free on the disk ({} GB free{}): room is made once the job beside it ({beside}) ends", need >> 30, free >> 30, self.floor_words()));
        }
        let res = self.mac.resources(&self.o.home, None, None, None);
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
        } else if spec.id.starts_with("terrain 3/") {
            room::RESERVE + TERRAIN_SPACE
        } else {
            steps::row(&step).map_or(room::RESERVE, |s| s.disk)
        }
    }

    fn try_start_said(&mut self, k: usize, spec: JobSpec, c: &Conditions, root: Option<&Path>, waiting: &mut Vec<Waiting>) -> bool {
        if let Some(why) = self.wait_reason(&spec, c) {
            waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
            return false;
        }
        // (Nor an items or heritage job while the answers they keep go to the NAS: `seed_answers`.)
        if self.answers_seed.as_ref().is_some_and(|t| !t.is_finished()) && step_of(&spec.id).is_some_and(|s| ANSWERED.contains(&s.as_str())) {
            waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why: "the pass's Wikidata and Wikipedia answers here are going to the NAS (seconds)".into() });
            return false;
        }
        // The pool's lead publishes only once its records reflect the journal, and sweeps only on a
        // step that re-asserted its term, caught up (docs/pool.md §6.2, §6.6): an entry not merged
        // yet may hold uploads the records don't name.
        if let (Some(run), false) = (self.pool.as_mut(), self.o.helper) {
            let st = step_of(&spec.id).unwrap_or_default();
            if pool::PUBLISHES.contains(&st.as_str()) && !run.gates.publish() {
                waiting.push(Waiting { step: Some(st), what: spec.what.clone(), why: "waits for the pool's records to reflect the journal (a listing of every day merged)".into() });
                return false;
            }
            if pool::SWEEPS.contains(&st.as_str()) && !run.gates.sweep() {
                run.reassert = true;
                waiting.push(Waiting { step: Some(st), what: spec.what.clone(), why: "this Mac re-asserts its term first, its records caught up".into() });
                return false;
            }
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
        // runs here (the loop that looks after it waits meanwhile): beside another, a job starts only with
        // the room there is, the target's freeing on a thread of its own (`tend_caches`). Nothing a
        // running job uses goes (it holds it: store::cachefile); not while a freeing of the caches
        // runs (it frees already), nor while a job an earlier agent left runs (`locks_kept`).
        let cache = self.o.home.join("cache");
        let need = self.need_of(k, &spec);
        // (The owner's disk room target stays free past the job's own room: room::Target.)
        let floor = self.floor_for(&step);
        let room = need.saturating_add(floor);
        let others: Vec<String> = self.slots.iter().enumerate().filter(|(j, _)| *j != k).filter_map(|(_, s)| s.running.as_ref().map(|r| step_of(&r.spec.id).unwrap_or_default())).collect();
        let other = !others.is_empty();
        // (Room-making fell short of the target lately: not tried again until a job ends, the
        // target changes or ten minutes pass; the job waits.)
        let short = self.floor_short.is_some_and(|(t, n, at)| t == floor && room >= n && at.elapsed() < Duration::from_secs(600));
        if !(k > 0 || other) && floor > 0 && !self.o.helper && short && self.disk_free() < room {
            waiting.push(Waiting { step: Some(step), what: what.clone(), why: self.floor_why(need) });
            self.floor_held = Some(room);
            return false;
        }
        if k > 0 || other {
            let free = self.disk_free();
            if (!self.o.helper || floor > 0) && free < room {
                waiting.push(Waiting { step: Some(step), what: what.clone(), why: format!("needs {} GB free on the disk ({} GB free{}; beside another job, room is made only toward the disk room target)", room >> 30, free >> 30, if floor > 0 { self.floor_words() } else { String::new() }) });
                return false;
            }
        } else if let Some(r) = root.filter(|_| self.caches_task.is_none() && self.locks_kept()) {
            // (Never without the NAS: what goes here must be kept there.)
            // (The OSM pass without the margin: its need is what its conditions admitted it with.)
            let margin = if id.starts_with("osm-pass") { 0 } else { room::margin(need) };
            // (What this job and the queued ones read goes last: a terrain run's own area's
            // archive copies, the pageview months for an items or heritage job, …)
            let mut hints = self.hints();
            hints.add(&step, &spec.record.as_ref().map(|w| w.targets.iter().map(|t| t.0.clone()).collect::<Vec<_>>()).unwrap_or_default());
            if let Some(t) = id.strip_prefix("terrain ") {
                hints.add("terrain", &[t.to_string()]);
            }
            match room::make_room(&cache, &r.join("sources"), room, margin, &hints) {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!("agent: {} GB of the cheap caches deleted (canopy squares, raw terrain tiles, copies of the records' files, pageview months) for {} GB free", n >> 30, room.saturating_add(margin) >> 30);
                    self.cheap = None;
                }
                Err(e) => eprintln!("agent: making room on the disk: {e:#}"),
            }
            // Still short of the target past its room: the job waits (it would fill what the owner
            // keeps free), as do the jobs after it; the caches' others are freed toward the target
            // between jobs (`tend_caches`).
            // The caches' others are then freed toward it between jobs (`tend_caches`, `goal`: the
            // least room a job held needs), and room-making tried again once that's done.
            if floor > 0 && !self.o.helper && self.disk_free() < room {
                waiting.push(Waiting { step: Some(step), what: what.clone(), why: self.floor_why(need) });
                self.floor_held = Some(room);
                self.floor_short = Some((floor, self.floor_short.filter(|f| f.0 == floor).map_or(room, |f| f.1.min(room)), Instant::now()));
                return false;
            }
        } else if floor > 0 && !self.o.helper && self.disk_free() < room {
            // (Without the NAS no room is made, nor while a freeing runs: it waits for it.)
            waiting.push(Waiting { step: Some(step), what: what.clone(), why: self.floor_why(need) });
            self.floor_held = Some(room);
            return false;
        }
        // A helper's job its disk still has no room for (the caches emptied as far as they could
        // be): given back, not started on a Mac someone uses.
        if self.o.helper {
            let free = room::disk_free(&self.o.home).unwrap_or(0);
            if free < room {
                let why = format!("too little room on its disk: {} GB free, {} GB needed{}", free >> 30, room >> 30, if floor > 0 { self.floor_words() } else { String::new() });
                waiting.push(Waiting { step: None, what: what.clone(), why: why.clone() });
                // (Held by the owner's target alone: not held against its targets, and no work
                // asked for meanwhile (`helper_job`), but freed toward (`goal`).)
                let by_target = free >= need;
                if by_target {
                    self.floor_short = Some((floor, room, Instant::now()));
                }
                self.end_lease(k, if by_target { Outcome::Interrupted } else { Outcome::Failed }, &[], &why);
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
            self.slots[k].claimed = true;
        }
        // In the pool, every job of the lead's holds a lease of its coordinator, and saves into a
        // folder of its own (`<term>-<id>`), handed to the journal as it ends (docs/pool.md §7.3).
        if let (Some(_), false) = (&self.pool, self.o.helper) {
            let w = spec.record.clone().unwrap_or_else(|| build::Work { step: step_of(&id).unwrap_or_default(), targets: Vec::new() });
            let held = match self.slots[k].lease.take() {
                Some(Held::Own(n)) => Some(n),
                _ => self.coord.as_ref().and_then(|c| c.hold(&w.step, &w.targets)),
            };
            let (Some(n), Some(c)) = (held, &self.coord) else {
                waiting.push(Waiting { step: Some(w.step.clone()), what: what.clone(), why: if self.coord.is_none() { "this Mac's coordinator isn't running: no lease to hand its work off under".into() } else { "another worker holds part of it; planning again".into() } });
                if let (Some((step, ts)), Some(r), true) = (shared_targets(&spec), root, std::mem::take(&mut self.slots[k].claimed)) {
                    claims::release(r, &step, &ts, &self.me_of(k));
                }
                return false;
            };
            let lease = crate::pool::journal::LeaseId { term: c.lease_term(n), n };
            let dir = pool::job_dir(&self.o.home, lease);
            let kept = pool::JobKept { step: w.step.clone(), targets: w.targets.clone(), lease };
            if let Err(e) = std::fs::create_dir_all(&dir).map_err(anyhow::Error::from).and_then(|()| crate::whole::write(&dir.join("work.json"), &serde_json::to_vec(&kept)?)) {
                c.finish(n, &[]);
                waiting.push(Waiting { step: Some(w.step), what: what.clone(), why: format!("its folder: {e:#}") });
                return false;
            }
            self.slots[k].lease = Some(Held::Pooled { id: n, term: lease.term, dir, own: true });
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
            if let (Some((step, ts)), Some(r), true) = (shared, root, std::mem::take(&mut self.slots[k].claimed)) {
                claims::release(r, &step, &ts, &self.me_of(k));
            }
            self.finished(&id, &what, false, 0, format!("couldn't start: {e:#}"));
            return false;
        }
        true
    }

    /// The second job (docs/plan.md §8, Two jobs at once), when its slot is free: of the plan's
    /// jobs, the first by its steps' order (`SECOND`) that can run beside the first job's (`clash`),
    /// whoever is at the Mac, and that fits
    /// the memory beside it: the two jobs' within three quarters of the Mac's (the first's as
    /// predicted, or as it is now if more), and the second's free now with 2 GB to spare. Why none
    /// starts, in `beside_why`.
    fn start_second(&mut self, plan: &[JobSpec], c: &Conditions, root: Option<&Path>) {
        // (The memory guard drained a job beside the first: nothing starts beside it until it ends.)
        if let Some(id) = self.guard_held(0) {
            self.beside_why = Some(format!("the jobs here held more memory together than this Mac's limit lately: nothing starts beside {id} until it ends"));
            return;
        }
        // (A job the first slot passed over too long: nothing new here but it, so it starts as soon
        // as the job in its way ends.)
        let starving = self.starving(plan).cloned();
        let rank = |s: &JobSpec| step_of(&s.id).and_then(|st| SECOND.iter().position(|x| *x == st));
        let mut picks: Vec<&JobSpec> = plan.iter().filter(|s| rank(s).is_some() && starving.as_ref().is_none_or(|id| *id == s.id)).collect();
        if let (Some(id), true) = (&starving, picks.is_empty()) {
            self.beside_why = Some(format!("nothing starts here until {id}, passed over for {} min, has", PASS_MAX.as_secs() / 60));
            return;
        }
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
            if self.disk_free() < self.need_of(0, h).saturating_add(self.floor_for(&hs)) {
                self.beside_why = Some(format!("{} needs room made on the disk next, which waits for one job alone: nothing starts beside the first job until it has", build::label(&hs)));
                return;
            }
        }
        let res = self.mac.resources(&self.o.home, None, None, None);
        let (total_mb, free_mb) = self.mem_set.unwrap_or_else(|| {
            let total_mb = (res.mem_gb * 1024.0) as u64;
            (total_mb, res.mem_free_pct.map_or(0, |p| total_mb * p as u64 / 100))
        });
        let first_mb = first.as_ref().map_or(0, |f| f.2.max(f.3));
        let mut why: Option<String> = None;
        for spec in picks {
            let step = step_of(&spec.id).unwrap_or_default();
            if first.as_ref().is_some_and(|f| f.1 == spec.id || clash(&f.0, &step)) {
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
    /// offered (crate::coord::Coordinator::peak), else its step's first guess (`first_peak`).
    fn spec_peak(&self, spec: &JobSpec) -> u64 {
        let step = step_of(&spec.id).unwrap_or_default();
        match (&self.coord, spec.record.as_ref()) {
            (Some(c), Some(w)) => w.targets.iter().map(|(t, _)| c.peak(&w.step, t).unwrap_or_else(|| first_peak(&w.step)).max(c.floor(&w.step, t).map_or(0, |f| f.mb))).max().unwrap_or(0),
            _ => first_peak(&step),
        }
    }

    /// This Mac's memory (MB): as a test sets it, else as read once this loop (`step`); 0 when it
    /// can't be read (the memory guard then guards nothing).
    fn total_mb(&self) -> u64 {
        self.mem_set.map_or(self.total, |m| m.0)
    }

    /// This Mac's memory (MB) as the Mac says it now (0: unknown).
    fn mac_total_mb(&self) -> u64 {
        match self.mac {
            cond::Mac::Real => crate::sys::memsize().map_or(0, |b| b >> 20),
            _ => (self.mac.resources(&self.o.home, None, None, None).mem_gb * 1024.0) as u64,
        }
    }

    /// Whether the memory guard stops jobs: as its switch says, as its default until it's read.
    fn guard_stops(&self) -> bool {
        self.guard_on.unwrap_or(memguard::DEFAULT)
    }

    /// What slot `k`'s job holds now (MB: its processes' footprints summed), as the guard's sampler
    /// last saw it; None with no job (or its memory unknown).
    fn held_mb(&self, k: usize) -> Option<u64> {
        self.slots[k].running.as_ref()?;
        self.sampler.held(k)
    }

    /// Where slot `k`'s job notes what its targets cost (`SCENIC_COSTS`): this Mac's own job's in
    /// the agent's folder, a member's leased job's in its lease's folder.
    fn slot_costs(&self, k: usize) -> PathBuf {
        match &self.slots[k].lease {
            Some(Held::Leased { dir, .. } | Held::Pooled { dir, own: false, .. }) => dir.join("costs.jsonl"),
            _ => self.costs_path(k),
        }
    }

    /// The memory guard (crate::agent::memguard), once a loop after the jobs are tended, by what its
    /// sampler saw: while the jobs here hold more together than this Mac's limit, the job beside the
    /// largest stops at its next safe point when the largest fits alone (and nothing starts beside
    /// it until it ends); the largest, past the limit alone, stops at once while the Mac is in
    /// trouble (the sampler froze it, or the pressure or the swap says so now), else at its next
    /// safe point too; a job drained that reached no safe point in the time a pause gives one stops
    /// at once. One it stops is given back, not failed, kept from this Mac for an hour (a member's
    /// lease ended as failed, so its lead keeps it from this Mac as long), what it held its target's
    /// floor. With the switch off, or this Mac's memory unknown, nothing, and a job the sampler froze
    /// goes on. True when it stopped a job.
    fn guard(&mut self, root: Option<&Path>) -> bool {
        let limit = memguard::limit_mb(self.total_mb());
        let on = self.guard_stops() && limit > 0;
        self.sampler.set(on, limit);
        if let Some(last) = self.sampler.take_froze().pop() {
            self.guard_last = Some(last);
        }
        if !on {
            for k in 0..SLOTS {
                if self.sampler.thaw(k) {
                    eprintln!("agent: memory guard: slot {k}'s job goes on (the guard is off)");
                }
            }
            return false;
        }
        let held: Vec<Option<u64>> = (0..SLOTS).map(|k| self.held_mb(k)).collect();
        let total: u64 = held.iter().flatten().sum();
        if total <= limit {
            return false;
        }
        let draining: Vec<bool> = self.slots.iter().map(|s| s.mem_drain.is_some()).collect();
        let late = (0..SLOTS).find(|&k| draining[k] && !self.slots[k].mem_drain_alone && self.slots[k].drain_since.is_some_and(|t| t.elapsed() >= DRAIN_GRACE));
        let gb = |mb: u64| mb as f64 / 1024.0;
        let id_of = |a: &Self, k: usize| a.slots[k].running.as_ref().map(|r| r.spec.id.clone()).unwrap_or_default();
        let stop = match (late, memguard::decide(&held, limit, &draining)) {
            (Some(k), _) => Some((k, format!("stopped by the memory guard: it reached no safe point in {} min while the jobs here held {:.1} GB together, past this Mac's limit of {:.1} GB", DRAIN_GRACE.as_secs() / 60, gb(total), gb(limit)))),
            (None, memguard::Act::Nothing) => None,
            (None, memguard::Act::Drain(k)) => {
                let big = (0..SLOTS).find(|&b| b != k && held[b].is_some()).unwrap_or(0);
                let beside = id_of(self, big);
                let why = format!("the jobs here hold {:.1} GB together, past this Mac's limit of {:.1} GB: it makes room for {beside} ({:.1} GB)", gb(total), gb(limit), gb(held[big].unwrap_or(0)));
                let id = id_of(self, k);
                eprintln!("agent: memory guard: {id} stops at its next safe point: {why}");
                self.guard_last = Some((now_s(), format!("{id} stopped at its next safe point: {why}")));
                self.slots[k].mem_drain = Some(why);
                self.slots[k].mem_drain_alone = false;
                self.guard_hold = Some((big, beside));
                None
            }
            (None, memguard::Act::Over(k)) => {
                let mb = held[k].unwrap_or(0);
                if self.sampler.frozen(k) || self.sampler.in_trouble() {
                    Some((k, format!("stopped by the memory guard: it held {:.1} GB, past this Mac's limit of {:.1} GB, the Mac short of memory (what it held is kept as the least its target takes, so it goes to a Mac with room)", gb(mb), gb(limit))))
                } else {
                    if !draining[k] {
                        let why = format!("it holds {:.1} GB, past this Mac's limit of {:.1} GB (the Mac not short of memory yet: it stops at its next safe point; what it held is kept as the least its target takes)", gb(mb), gb(limit));
                        let id = id_of(self, k);
                        eprintln!("agent: memory guard: {id} stops at its next safe point: {why}");
                        self.guard_last = Some((now_s(), format!("{id} stopped at its next safe point: {why}")));
                        self.slots[k].mem_drain = Some(why);
                        self.slots[k].mem_drain_alone = true;
                    }
                    None
                }
            }
        };
        let Some((k, why)) = stop else { return false };
        // (Frozen by the sampler: let go on first, so it can take its termination.)
        self.sampler.thaw(k);
        let Some(r) = self.slots[k].running.as_mut() else { return false };
        let id = r.spec.id.clone();
        eprintln!("agent: memory guard: {id} {why}");
        r.stop(Duration::from_secs(30));
        self.guard_last = Some((now_s(), format!("{id} {why}")));
        self.guard_backoff.insert(id, (now_s() + GUARD_BACKOFF_S, why.clone()));
        self.slots[k].guard_stopped = true;
        self.stopped(k, root, &why);
        self.release_claims(k, root);
        self.slots[k].running = None;
        self.slots[k].drain_since = None;
        self.slots[k].mem_drain = None;
        self.slots[k].mem_drain_alone = false;
        self.slots[k].guard_stopped = false;
        std::fs::remove_file(self.record_path(k)).ok();
        true
    }

    /// The job in slot `k` the memory guard drained the one beside, while it runs: nothing starts
    /// in the other slot until it ends.
    fn guard_held(&self, k: usize) -> Option<String> {
        let (slot, id) = self.guard_hold.clone()?;
        (slot == k && self.slots[slot].running.as_ref().map(|r| &r.spec.id) == Some(&id)).then_some(id)
    }

    /// The memory guard for the status.
    fn guard_view(&self) -> memguard::View {
        let limit = memguard::limit_mb(self.total_mb());
        let why_off = match (self.guard_stops(), limit) {
            (false, _) => Some("switched off (state/pool/memory-guard)".to_string()),
            (true, 0) => Some("this Mac's memory can't be read: nothing is stopped".to_string()),
            _ => None,
        };
        memguard::View { on: why_off.is_none(), limit_mb: limit, held_mb: (0..SLOTS).filter_map(|k| self.held_mb(k)).sum(), why_off, last: self.guard_last.clone() }
    }

    /// The targets of the plan's `works` the memory guard holds (its switch on): those whose floor,
    /// learned alone the way its step runs now, passes this Mac's limit (`here`), and of them those
    /// past every Mac's in the pool, as the lead knows them (`all`: a step only this Mac runs, this
    /// Mac's limit alone), with why for the status; and those whose floor past it was a batch's
    /// (`alone`: tried again in a job of their own before they're held).
    fn guard_holds(&self, works: &[build::Work]) -> GuardHolds {
        let mut h = GuardHolds::default();
        let limit = memguard::limit_mb(self.total_mb());
        let Some(c) = self.coord.as_ref().filter(|_| self.guard_stops() && limit > 0) else { return h };
        let largest = c.largest_limit().unwrap_or(0).max(limit);
        let gb = |mb: u64| mb as f64 / 1024.0;
        for w in works {
            let most = if steps::SHARED.contains(&w.step.as_str()) { largest } else { limit };
            let (mut n_here, mut n_all, mut need) = (0, 0, 0u64);
            for (t, _) in &w.targets {
                let key = (w.step.clone(), t.clone());
                match c.holding_floor(&w.step, t) {
                    Some(f) if f.mb > limit && f.alone => {
                        need = need.max(f.mb);
                        h.here.insert(key.clone());
                        n_here += 1;
                        if f.mb > most {
                            h.all.insert(key);
                            n_all += 1;
                        }
                    }
                    Some(f) if f.mb > limit => {
                        h.alone.insert(key);
                    }
                    _ => {}
                }
            }
            if n_all > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: build::label(&w.step).into(), why: format!("{n_all} target{} need{} about {:.1} GB, more than any Mac in the pool has (the most: {:.1} GB): a step whose memory grows with its target, for the owner to see to (`scenic pool floors --clear` once it's fixed)", if n_all == 1 { "" } else { "s" }, if n_all == 1 { "s" } else { "" }, gb(need), gb(most)) });
            } else if n_here > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: build::label(&w.step).into(), why: format!("{n_here} target{} need{} about {:.1} GB, past this Mac's limit of {:.1} GB: left to a Mac with room", if n_here == 1 { "" } else { "s" }, if n_here == 1 { "s" } else { "" }, gb(need), gb(limit)) });
            }
        }
        h
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
        self.mac.resources(&self.o.home, root, bytes.map(|b| (b.cheap as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0), (ms != u64::MAX && root.is_some()).then_some(ms))
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
        } else if let (Some(goal), Some(s)) = (self.toward_due(), sources.clone()) {
            // The owner's disk room target: the caches freed toward it (or the room a job waiting
            // for it needs), as far as needed, whether or not the build has work left (only while
            // no job runs here).
            self.toward_tried = Some((goal, now_s()));
            self.toward_goal.store(goal, std::sync::atomic::Ordering::Relaxed);
            let at = self.toward_goal.clone();
            let target = self.floor();
            let hints = self.hints();
            self.caches_start_toward(target, goal, move || room::toward(&cache, &s, at, &hints));
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
            Ok(thread) => self.caches_task = Some(CachesTask { ask, toward: None, began: Instant::now(), thread }),
            Err(e) => self.caches_failed(anyhow::anyhow!("its thread didn't start: {e}"), ask),
        }
    }

    /// Starts a freeing toward the owner's disk room target (`target`; `goal`, the room it frees
    /// toward, bytes) on a thread of its own; one that can't start is tried again with the next
    /// (`toward_due`).
    fn caches_start_toward(&mut self, target: u64, goal: u64, work: impl FnOnce() -> Result<room::Freed> + Send + 'static) {
        match std::thread::Builder::new().name("caches".into()).spawn(work) {
            Ok(thread) => self.caches_task = Some(CachesTask { ask: None, toward: Some((target, goal)), began: Instant::now(), thread }),
            Err(e) => eprintln!("agent: freeing the caches toward the disk room target: its thread didn't start: {e}"),
        }
    }

    /// What the caches are due a freeing toward (bytes, `goal`: the owner's disk room target, or
    /// the room a job held by it needs past it): the disk is short of it, no job runs here (nor one an earlier agent left), and it wasn't tried for
    /// this target since the last job ended, or in the last ten minutes. (The agent's own: not a
    /// dry run's.)
    fn toward_due(&mut self) -> Option<u64> {
        let target = self.goal();
        if target == 0 || self.disk_free() >= target || !self.locks_kept() {
            return None;
        }
        // (Tried lately toward as much or more: not again. The target and a held job's room past it
        // don't take turns.)
        let fresh = self.toward_tried.is_some_and(|(t, at)| t >= target && now_s().saturating_sub(at) < 600 && self.mem.worked_at <= at);
        (!fresh).then_some(target)
    }

    /// Why the disk is short of the owner's room target and stays so, for the status (None: it
    /// has it, or the agent frees toward it now or soon).
    fn room_short(&self, nas: bool) -> Option<String> {
        let target = self.floor();
        let free = self.disk_free();
        if target == 0 || free >= self.goal() || self.caches_task.as_ref().is_some_and(|t| t.toward.is_some()) {
            return None;
        }
        if !nas {
            return Some(format!("{} free, the target {}: the NAS isn't reachable, and what goes from the caches must be kept there; they're freed toward it once it is", room::size(free), room::size(target)));
        }
        if free >= target {
            return None;
        }
        if let Some(g) = self.orphans.first() {
            return Some(format!("{} free of the target's {}: its caches are freed toward it once the job an earlier agent left ({}) ends (its programs may not keep what they use)", room::size(free), room::size(target), g.id));
        }
        let last = self.mem.toward.as_ref().filter(|f| f.target == Some(target))?;
        Some(format!(
            "{} free of the target's {}: this Mac's build caches have nothing more to free now (the last freeing: {}); the rest of the disk is the map's offline copy, which keeps its own reserve, and other files",
            room::size(free),
            room::size(target),
            last.say()
        ))
    }

    /// What's done to this Mac's caches now, while a trim, a clear or a freeing toward the target
    /// runs (jobs run and start meanwhile: what they use stays, store::cachefile).
    fn caches_busy(&self) -> Option<String> {
        let t = self.caches_task.as_ref()?;
        let doing = match (&t.ask, t.toward) {
            (Some(_), _) => "cleared".to_string(),
            (None, Some((t, g))) => format!("freed toward the disk room target ({}){}", room::size(t), room_past(t, g)),
            (None, None) => "trimmed".to_string(),
        };
        Some(format!("this Mac's caches are being {doing} ({} min so far); jobs run meanwhile, keeping what they use", t.began.elapsed().as_secs() / 60))
    }

    /// A trim or a clear done (waited for, when it isn't yet), its result recorded. Asked to stop
    /// midway: nothing kept, the trim done again by the next agent, the ask left for it.
    fn caches_done(&mut self, t: CachesTask) {
        let r = t.thread.join().unwrap_or_else(|_| Err(anyhow::anyhow!("it failed midway")));
        if stopping() {
            return;
        }
        if let Some((target, goal)) = t.toward {
            match r {
                Ok(f) => self.toward_record(f, target, goal),
                Err(e) => eprintln!("agent: freeing the caches toward the disk room target: {e:#}; trying again in ten minutes"),
            }
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

    /// What a freeing toward the owner's disk room target did: logged, kept for the status, noted in
    /// the history when it freed something, the caches counted again.
    fn toward_record(&mut self, mut f: room::Freed, target: u64, goal: u64) {
        f.at = now_s();
        (f.target, f.goal) = (Some(target), (goal > target).then_some(goal));
        let free = self.disk_free();
        let note = format!("freed its caches toward the disk room target ({}){}: {}; {} free", room::size(target), room_past(target, goal), f.say(), room::size(free));
        eprintln!("agent: {note}");
        if f.bytes() > 0 {
            self.note(crate::coord::history::Event { worker: Some(self.host.clone()), note, ..crate::coord::history::Event::new("caches") });
        }
        self.mem.toward = Some(f);
        // (The jobs held by the target wait on: each starts once the disk has its room, and room-
        // making, which frees no more than this did, isn't tried again for ten minutes, or until a
        // job ends. Nor is a freeing toward as much: `toward_due`.)
        self.save();
        self.count_caches();
    }

    /// This Mac's caches for the status (room::Caches), `why` they can't be cleared now: what a clear
    /// would free, as last counted, each cache with about how long it takes to come back at the
    /// NAS's speed here (measured: 60 MB/s on the LAN, 12 through Tailscale, plan §12).
    fn caches_view(&self, why: Option<String>, home: bool, nas: bool) -> room::Caches {
        let sizes = self.cache_size.lock().unwrap().1.clone();
        room::Caches {
            clearable: sizes.as_ref().map(|s| s.clear.values().sum()),
            each: sizes.map(|s| room::gone(&s.clear, if home { 60.0 } else { 12.0 })).unwrap_or_default(),
            why_not: why,
            trimmed: self.mem.trimmed.clone(),
            cleared: self.mem.cleared.clone(),
            declined: self.mem.declined.clone(),
            room: Some(room::RoomView { target: self.room_target.clone(), free: self.disk_free(), toward: self.mem.toward.clone(), short: self.room_short(nas) }),
        }
    }

    /// Why this Mac's caches can't be trimmed or cleared now, if they can't: a job an earlier agent
    /// left runs here (`orphans`: its programs may not hold what they use), the NAS isn't reachable (what
    /// goes must be kept there), or the build has work left (`work_left`: by the build Mac's
    /// forecast, its own; a helper's, the one in the build Mac's heartbeat on the NAS, read at most
    /// each minute, with no job of the build Mac's running, nor beside it).
    fn caches_why_not(&mut self, root: Option<&Path>) -> Option<String> {
        // (A job of this agent's running is no reason: what it uses it holds, store::cachefile.)
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

    fn start(&mut self, k: usize, mut spec: JobSpec, _c: &Conditions) -> Result<()> {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        // All the cores, whoever is at the Mac (the owner's choice; a helper, two fewer: its Mac has
        // less memory). The second job: half (the two jobs' threads share the cores).
        let threads = match (k, self.o.helper) {
            (1.., _) => (cores / 2).max(1),
            (_, true) => cores.saturating_sub(2).max(1),
            (_, false) => cores,
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
        // (No timings left from the slot's last job, for this one's hand-off to carry.)
        self.slots[k].timings_out = None;
        let log = self.o.home.join("logs").join(format!("{}.log", spec.id.replace([' ', '/'], "-")));
        eprintln!("agent: starting {}{} ({threads} threads)", spec.id, if k > 0 { " beside the first job" } else { "" });
        // The build Mac's jobs save the records (crate::out::Out::save trusts them by this, whatever
        // this Mac is named now), note what their units cost, and may offer tasks to other workers
        // through the coordinator; a helper's hand their saves off (SCENIC_HANDOFF, in their command).
        let mut env: Vec<(String, String)> = Vec::new();
        if !self.o.helper {
            // (In the pool no job writes the records: the lead's too hand off, docs/pool.md §7.3.)
            match &self.slots[k].lease {
                Some(Held::Pooled { dir, .. }) => env.push(("SCENIC_HANDOFF".into(), dir.to_string_lossy().into_owned())),
                _ => env.push(("SCENIC_BUILD_MAC".into(), "1".into())),
            }
            env.push(("SCENIC_COSTS".into(), self.costs_path(k).to_string_lossy().into_owned()));
            if let Some(c) = &self.coord {
                env.push(("SCENIC_COORD".into(), format!("http://127.0.0.1:{}", coord_port())));
                env.push(("SCENIC_COORD_TOKEN".into(), c.job_token.clone()));
            }
            // A round's step that reads the units: as they were when the round began.
            let step = spec.record.as_ref().map(|w| w.step.as_str()).unwrap_or("");
            if let Some(r) = self.round.borrow().as_ref().filter(|r| !r.over && build::AS_OF_STEPS.contains(&step)) {
                env.push((crate::out::UNITS_AS_OF_ENV.into(), format!("{}#{}", self.o.home.join(ROUND_FILE).display(), r.began)));
            }
        }
        // This Mac's cache, where a job keeps what it reads again and again (the coverage:
        // pipeline::coverage::Coverage::load).
        env.push((crate::coverage::CACHE_ENV.into(), self.o.home.join("cache").to_string_lossy().into_owned()));
        // Its channel, to stop at a safe point when the build pauses, and where it notes each target
        // done (crate::control), afresh.
        let control = self.control_path(k);
        std::fs::write(&control, b"run").with_context(|| format!("write {}", control.display()))?;
        let done = self.done_path(k);
        std::fs::remove_file(&done).ok();
        self.slots[k].drain_since = None;
        env.push((crate::control::CONTROL_ENV.into(), control.to_string_lossy().into_owned()));
        env.push((crate::control::DONE_ENV.into(), done.to_string_lossy().into_owned()));
        // Where it leaves its timings' record (crate::timings; a helper's leased job: its outbox
        // folder, in its command), taken in as it ends (`take_timings`).
        let timings = self.timings_path(k);
        std::fs::remove_file(&timings).ok();
        env.push((crate::timings::TIMINGS_ENV.into(), timings.to_string_lossy().into_owned()));
        env.push((crate::timings::JOB_ID_ENV.into(), spec.id.clone()));
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (step, targets) = spec.record.as_ref().map_or((spec.id.split(' ').next().map(str::to_string), Vec::new()), |w| (Some(w.step.clone()), w.targets.iter().map(|t| t.0.clone()).collect()));
        let what = spec.what.clone();
        // (Started: no longer passed over, in either slot.)
        self.passed_over.remove(&spec.id);
        // (Its costs file afresh: a line an earlier job left, a target begun and never ended, would
        // name a target under way that isn't, crate::agent::memguard.)
        let costs = self.slot_costs(k);
        std::fs::remove_file(&costs).ok();
        let own_key = match spec.record.as_ref() {
            None if spec.id.starts_with("osm-pass ") => spec.id.split_once(' ').map(|(s, t)| crate::coord::cost_key(s, t)),
            _ => None,
        };
        let (w_step, w_targets) = (step.clone().unwrap_or_default(), targets.clone());
        self.slots[k].running = Some(Running::start(spec, threads, &env, log, &self.record_path(k), Some(&done))?);
        self.slots[k].beaten = None;
        if let Some(pgid) = self.slots[k].running.as_ref().map(|r| r.pgid) {
            self.sampler.watch(k, memguard::Watch { pgid, costs, done: done.clone(), step: w_step, targets: w_targets, own_key, ..Default::default() });
        }
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
            // (A full check asked for, done.)
            if let Some(unit) = id.strip_prefix("inputs ").and_then(|r| r.strip_suffix(" full")) {
                self.inputs.full.borrow_mut().remove(unit);
            }
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
                    needs: Needs { nas: true },
                    restart_after_sleep: true,
                    record: None,
                });
            }
        }

        // The regions: terrain and slope near the coverage, base(U), pack(T), lo, a catalog.
        out.extend(self.region_work(root, have.as_deref(), newer.as_ref().map(|n| n.1.as_str()), waiting));

        // The gate's checks before every other step (docs/inputs.md §4.3): a change is checked
        // before the plan builds with stale inputs.
        let (checks, rest): (Vec<JobSpec>, Vec<JobSpec>) = std::mem::take(&mut out).into_iter().partition(|j| j.record.as_ref().is_some_and(|w| w.step == "inputs"));
        out = checks;
        out.extend(rest);
        // Daily: the user's folders backed up, replaced files removed.
        if self.due("backup", Duration::from_secs(86400)) {
            out.push(JobSpec {
                id: "backup".into(),
                what: "Backing up translations, descriptions, inputs and the acceptances".into(),
                cmd: vec![s(&me), "backup".into(), "--root".into(), s(root), "--local".into(), s(&self.o.home.join("backups"))],
                needs: Needs { nas: true },
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
                needs: Needs { nas: true },
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
        let mut jobs: Vec<JobSpec> = self.gate_work(root, &manifest, &keys, waiting);
        let job = |id: String, what: &str, step: &str, extra: Vec<String>, record: Option<build::Work>| {
            let scratch = self.o.home.join("scratch").join(step);
            let mut cmd = vec![build_bin.clone(), step.to_string(), "--root".into(), s(root), "--scratch".into(), s(&scratch)];
            cmd.extend(extra);
            // (Every job needs the NAS; none waits for power or home, the owner's choice: the pass's
            // whole-planet reads go over Tailscale when away, slowly.)
            JobSpec { id, what: what.into(), cmd, needs: Needs { nas: true }, restart_after_sleep: true, record }
        };
        // Per pass, worldwide: the sets it lacks in their current filters (a set added or changed
        // since it ran), the hiking routes' ends, AWS's z8 (once), Overture's buildings (once per
        // release), the summits, the labels, the water.
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
            if let Some(w) = build::spoken_work(date, &manifest, &keys) {
                jobs.push(job(format!("spoken {date}"), "Mapping the languages spoken where, for names", "spoken", p.clone(), Some(w)));
            }
            if let Some(w) = build::water_work(date, &manifest, &keys) {
                jobs.push(job(format!("water {date}"), "Drawing the world's water at every zoom", "water", p.clone(), Some(w)));
            }
        }
        // After each catalog the map serves: the names to translate and descriptions to write.
        if let Some(w) = build::names_todo_work(served_catalog(&root.join("catalog")).map(|c| c.n), &keys) {
            let n = w.targets[0].1.clone();
            jobs.push(job(format!("names-todo {n}"), "Listing the names to translate and the descriptions to write", "names-todo", Vec::new(), Some(w)));
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
        let mut done = keys;
        let mut inputs = input_digests(root);
        // (The inputs' descriptions' credits, for the catalog's key: crate::inputs::credits. None
        // yet, no line, so the catalog's key is as it was. Unreadable now: the last digest read
        // stands, or, with none read yet, the catalog waits; never a key that flips on a failed
        // read.)
        let mut credits_unread = None;
        match crate::inputs::credits::described(root, &manifest) {
            Ok(d) => *self.inputs.credits.borrow_mut() = Some(crate::inputs::credits::digest(&d)),
            Err(e) if self.inputs.credits.borrow().is_none() => credits_unread = Some(format!("{e:#}")),
            Err(_) => {}
        }
        if let Some(Some(c)) = self.inputs.credits.borrow().clone() {
            inputs.insert("credits".into(), c);
        }
        let held = root.join("inputs/hold-catalog").exists();
        let reach = self.current_reach(root, &manifest, &done, date).ok().flatten();
        if !manifest.contains_key(crate::rail::CATALOGUE) {
            waiting.push(Waiting { step: None, what: build::TRAINS.into(), why: "the rail sources' catalogue isn't on the NAS yet (put by hand: docs/plan.md, Hand-made inputs)".into() });
        } else if inputs.get("keys").map(String::as_str) == Some("?") {
            waiting.push(Waiting { step: None, what: build::TRAINS.into(), why: "inputs/keys.env can't be read now".into() });
        }
        if inputs.get("bld-release").map(String::as_str) == Some("?") {
            waiting.push(Waiting { step: None, what: build::BUILDINGS.into(), why: "the 3D buildings' sources' indexes on the NAS can't be read now".into() });
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
            let tiles = self.terrain_tiles(root, &manifest);
            // (The records as re-keyed: what they are once the loop has re-keyed them, as a dry run,
            // or a loop that couldn't take the build lock, plans too.)
            let times = rekey::FileTimes::new(root);
            rekey::as_read(&mut done, cov, date, &manifest, reach.as_deref(), &inputs, &tiles, &times);
            build::plan(&cov, date, &manifest, &done, &inputs, reach.as_deref(), &tiles, build::Rounds { each: &covs.each, on_map: &on_map, since_last, current: kept.as_ref().filter(|r| !r.over), held })
        };
        // Units whose terrain can't be worked out now (a pack's index unread): they wait for it.
        if !planned.unknown.is_empty() {
            let why = self.tiles.borrow().unread().next().map(|(c, e)| format!(": {c}, {e}")).unwrap_or_default();
            waiting.push(Waiting { step: Some("unit".into()), what: "Building the areas".into(), why: format!("the terrain's indexes can't be read now for {} of them{why}", planned.unknown.len()) });
        }
        let edit_hold = edit_held(self.edited_at.get(), std::time::SystemTime::now());
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
        if let Some(why) = credits_unread.filter(|_| plan.iter().any(|w| w.step == "catalog")) {
            waiting.push(Waiting { step: Some("catalog".into()), what: build::PUBLISH.into(), why: format!("the inputs' descriptions (their credits) can't be read now: {why}") });
            plan.retain(|w| w.step != "catalog");
        }
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
            waiting.push(Waiting { step: Some("catalog".into()), what: build::PUBLISH.into(), why: format!("waits for {who} building the {} of {t}", if st.starts_with("slope") { "slope" } else { "tree cover" }) });
            plan.retain(|w| w.step != "catalog");
        } else if unmerged && plan.iter().any(|w| w.step == "catalog") {
            waiting.push(Waiting { step: Some("catalog".into()), what: build::PUBLISH.into(), why: "waits for a helper's work to be merged".into() });
            plan.retain(|w| w.step != "catalog");
        }
        // (What room-making lets go last: `hints`.)
        *self.queued.borrow_mut() = plan.clone();
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
        // A 3D buildings tile's rows read (crate::bld::sources::digests), for its jobs' memory.
        let bld_rows = |t: &str| inputs.get(&format!("bldprep-rows {t}")).and_then(|v| v.parse::<u64>().ok());
        let mut offers: Vec<crate::coord::Offer> = Vec::new();
        let regions = regions_left(&plan);
        // (A terrain area's z6 tiles near the coverage, for its run's expected memory: only when
        // there's terrain to offer.)
        let z6: BTreeMap<String, usize> = if plan.iter().any(|w| w.step == "terrain") {
            build::coverage_tiles(cov).into_iter().map(|(q, ts)| (format!("3/{}/{}", q.0, q.1), ts.len())).collect()
        } else {
            BTreeMap::new()
        };
        // (The memory guard's holds: a target past every Mac's limit offered to none, one past this
        // Mac's built only by a Mac with room.)
        let holds = self.guard_holds(&plan);
        waiting.extend(holds.why.iter().cloned());
        for w in plan.iter_mut() {
            w.targets.retain(|t| !holds.all.contains(&(w.step.clone(), t.0.clone())));
        }
        for w in plan.iter_mut().filter(|w| steps::SHARED.contains(&w.step.as_str())) {
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
                "terrain" => terrain_peak(if t.starts_with("6/") { 1 } else { z6.get(t).copied().unwrap_or(64) }),
                s @ ("bldprep" | "bldtiles") => bld_rows(t).map_or(first_peak(s), |n| bld_peak(s, n)),
                s => first_peak(s),
            };
            offers.push(crate::coord::Offer { step: w.step.clone(), targets: w.targets.iter().map(|(t, k)| (t.clone(), k.clone(), guess(t))).collect(), batch: job_size(&w.step, regions) });
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
        offers.sort_by_key(|o| steps::SHARED.iter().position(|s| *s == o.step));
        if let Some(c) = &self.coord {
            c.offer(date, offers);
        }
        // (Held here: left to a Mac with room. A target whose floor past this Mac's limit was a
        // batch's: a job of its own, so a floor learned alone holds it, or its measure frees it.)
        let mut alone: Vec<build::Work> = Vec::new();
        for w in plan.iter_mut() {
            w.targets.retain(|t| !holds.here.contains(&(w.step.clone(), t.0.clone())));
            let (one, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut w.targets).into_iter().partition(|t| holds.alone.contains(&(w.step.clone(), t.0.clone())));
            w.targets = rest;
            alone.extend(one.into_iter().map(|t| build::Work { step: w.step.clone(), targets: vec![t] }));
        }
        plan.extend(alone);
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
                "terrain" => terrain_peak(if t.starts_with("6/") { 1 } else { z6.get(t).copied().unwrap_or(64) }),
                s @ ("bldprep" | "bldtiles") => bld_rows(t).map_or(first_peak(s), |n| bld_peak(s, n)),
                s => first_peak(s),
            };
            let mut before = before;
            before.extend(plan.iter().filter(|w| w.step == "heritage-sites").map(|w| job(format!("heritage-sites {date}"), "", "heritage-sites", Vec::new(), Some(w.clone()))));
            let chains = build::chains_left(cov, date, &manifest, &done, &inputs, reach.as_deref());
            // (A fault in it costs the status its forecast, never the agent.)
            let under_way = self.round.borrow().as_ref().filter(|r| !r.over).map(|r| (r.regions.clone(), r.last, round_left.clone()));
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.forecast_now(root, &before, &planned.regions, &planned.backfill, chains, since_last, under_way, blind, &peak)));
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
                j.needs = Needs { nas: true };
                jobs.push(j);
                continue;
            }
            let mut extra: Vec<String> = w.targets.iter().map(|t| t.0.clone()).filter(|t| !matches!(t.as_str(), "catalog" | "items" | "marks" | "roadunits" | "stations" | "ferries" | "heritage-sites" | "heritage" | "overlays" | "rail-feeds" | "rail" | "bld-fetch" | "terrain-water") && !t.ends_with("-root")).collect();
            extra.extend(self.step_args(&w.step, date));
            // (Tree cover pieces made again as they are, their mids made: expected the same.)
            let same = expect_same(&w, &done);
            if !same.is_empty() {
                extra.extend(["--expect-same".to_string(), same.join(",")]);
            }
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
                "unit" | "pois" | "peaks" | "pack" | "trees-lo" | "terrain-lo" | "slope-lo" => format!("{base} ({areas})"),
                "trees" if same.len() == n => format!("Making the tree cover's mids, checking it the same ({})", areas.replace("area", "tile")),
                "terrain" if same.len() == n => format!("Making the terrain's mids, checking it the same ({})", areas.replace("area", "tile")),
                "slope" if same.len() == n => format!("Making the slope's mids, checking it the same ({})", areas.replace("area", "tile")),
                "trees" | "terrain" | "slope" | "bldprep" | "bldtiles" => format!("{base} ({})", areas.replace("area", "tile")),
                _ => base.to_string(),
            };
            let id = format!("{} {}", w.step, w.targets.first().map(|t| t.0.as_str()).unwrap_or(""));
            let step = w.step.clone();
            jobs.push(job(id, &what, &step, extra, Some(w)));
        }
        jobs
    }

    /// The forecast (crate::agent::forecast) of the work left: `before`, the build Mac's jobs before
    /// the regions'; `regions`, the plan's; `backfill`, the pieces whose mids it makes in
    /// idle time (build::Plan::backfill); `chains`, the roads', trains' and landmarks' work to come
    /// (build::chains_left); `blind`, why the work can't all be listed now. Each target's time (at
    /// the build Mac's pace: a time measured on a helper over its speed) and memory as last measured,
    /// else its step's mean or a first guess (`first_secs`; `peak` for its memory); the helpers at
    /// their measured speed; each machine free once its job under way is done.
    #[allow(clippy::too_many_arguments)]
    fn forecast_now(&self, root: &Path, before: &[JobSpec], regions: &[build::RegionLeft], backfill: &[build::Work], chains: [Vec<build::Work>; 4], since_last: Option<u64>, under_way: Option<(Vec<String>, bool, Vec<build::Work>)>, blind: Option<String>, peak: &dyn Fn(&str, &str) -> u64) -> forecast::Forecast {
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
        // (Each measured the way its step runs now: crate::coord::cost_version. A z3 tile's tree
        // cover isn't a z6 tile's.)
        let mut sums: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        for (k, c) in &costs {
            let step = k.split_once(' ').map_or("unit", |(s, _)| s);
            if c.v < crate::coord::cost_version(step) {
                continue;
            }
            let e = sums.entry(step.to_string()).or_default();
            (e.0, e.1) = (e.0 + c.secs as f64 * pace(c.worker.as_ref()), e.1 + 1);
        }
        let per = |step: &str| -> f64 {
            match (sums.get(step).filter(|s| s.1 > 0), self.mem.step_secs.get(&secs_key(step))) {
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
            let (each, known) = self.mem.step_secs.get(&secs_key(step)).map_or((first_secs(step), false), |&t| (t, true));
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
        let [roads, rail, landmarks, buildings] = chains;
        let chain_s = |works: &[build::Work]| -> f64 { works.iter().map(|w| if steps::SHARED.contains(&w.step.as_str()) { w.targets.iter().map(|t| cost(&w.step, &t.0).secs).sum() } else { mine(&w.step, w.targets.len()).secs }).sum() };
        let round_s = forecast::round_secs(&events).unwrap_or_else(|| ["prune", "roadunits", "stations", "ferries", "terrain-root", "slope-root", "catalog"].iter().map(|s| mine(s, 1).secs).sum::<f64>() + mine("pack", 8).secs + mine("lo", 2).secs);
        let ahead = under_way.as_ref().map_or(0.0, |u| chain_s(&u.2));
        let last_round_s = if roads.is_empty() { 0.0 } else { round_s.max(chain_s(&roads) - ahead) };
        // The trains' and the landmarks' chains from the start (their steps once what they read is
        // built: forecast::chain_deps), but the overlays (they read the built units) after the last
        // round; and a catalog after it with what the chains made since.
        let (mut chain_jobs, mut after): (Vec<forecast::Job>, Vec<forecast::Job>) = (Vec::new(), Vec::new());
        for w in rail.iter().chain(landmarks.iter()).chain(buildings.iter()) {
            let jobs: Vec<forecast::Job> = if steps::SHARED.contains(&w.step.as_str()) {
                w.targets.iter().map(|t| (w.step.clone(), t.0.clone(), cost(&w.step, &t.0))).collect()
            } else {
                vec![(w.step.clone(), w.targets.first().map(|t| t.0.clone()).unwrap_or_default(), mine(&w.step, w.targets.len()))]
            };
            if w.step == "overlays" { after.extend(jobs) } else { chain_jobs.extend(jobs) }
        }
        if !rail.is_empty() || !landmarks.is_empty() || !buildings.is_empty() {
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
        // (A worker that takes tails around: each of this Mac's units the moment its job gives it to
        // take its tail, crate::offload.)
        let unit_extra_s = if self.coord.as_ref().is_some_and(|c| c.tail_takers()) { crate::offload::LEASE_WAIT.as_secs_f64() } else { 0.0 };
        let mut machines = vec![Machine { name: self.host.clone(), speed: 1.0, measured: true, helper: false, second: false, mem_mb: u64::MAX, busy_s: busy(0, 1.0), unit_extra_s }];
        // Its second job: what fits beside the first (a quarter of its memory, say); while the Mac's
        // in use, as it is now, its network work alone.
        if self.second_allowed() {
            let (speed, measured) = speeds.get(&second).copied().unwrap_or((0.8, false));
            let mem_mb = (self.mac.resources(&self.o.home, None, None, None).mem_gb * 256.0) as u64;
            machines.push(Machine { name: second.clone(), speed, measured, helper: false, second: true, mem_mb, busy_s: busy(1, speed), unit_extra_s });
        }
        for h in &helpers {
            let (speed, measured) = speeds.get(&h.host).copied().unwrap_or((0.5, false));
            // (Its lease's targets at its pace, less the time since it took them, if longer than its
            // part's.)
            let lease_left = leased.iter().filter(|l| l.0 == h.host).map(|(_, step, ts, age)| ts.iter().map(|t| cost(step, t).secs).sum::<f64>() / speed - *age as f64).fold(0.0, f64::max);
            let eta = h.job.as_ref().map(|j| j.progress.as_ref().and_then(|p| p.eta_s).unwrap_or(600) as f64);
            let busy_s = eta.map_or(0.0, |e| e.max(lease_left).max(60.0));
            machines.push(Machine { name: h.host.clone(), speed, measured, helper: true, second: false, mem_mb: mem.get(&h.host).copied().filter(|&m| m > 0).unwrap_or(6144), busy_s, unit_extra_s: 0.0 });
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
        // (The pieces' mids made in idle time, last.)
        let idle: Vec<forecast::Job> = backfill.iter().flat_map(|w| w.targets.iter().map(|(t, _)| (w.step.clone(), t.clone(), cost(&w.step, t)))).collect();
        forecast::forecast(&forecast::Input { now: now_s(), before, regions, cost: &cost, round_s, last_round_s, blind, chains: chain_jobs, after, idle, since_last, under_way, machines, running })
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
            out.push(build::Step { what: WORLDWIDE.into(), steps: WORLDWIDE_STEPS.iter().map(|s| s.to_string()).collect(), ..Default::default() });
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
            build::water_work(&date, &manifest, &keys).is_some(),
        ]
        .iter()
        .filter(|&&l| l)
        .count();
        out.push(build::Step {
            what: WORLDWIDE.into(),
            steps: WORLDWIDE_STEPS.iter().map(|s| s.to_string()).collect(),
            left: Some(left),
            ..Default::default()
        });
        if regions.is_empty() {
            return out;
        }
        let Ok(covs) = self.coverage(root, &manifest, &date, regions, false) else { return out };
        let cov = &covs.all;
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        let tiles = self.terrain_tiles(root, &manifest);
        let inputs = input_digests(root);
        // (As re-keyed: agent::rekey.)
        let mut keys = keys;
        let times = rekey::FileTimes::new(root);
        rekey::as_read(&mut keys, cov, &date, &manifest, reach.as_deref(), &inputs, &tiles, &times);
        out.extend(build::checklist(cov, &date, &manifest, &keys, &inputs, root.join("inputs/hold-catalog").exists(), reach.as_deref(), &self.ready.borrow(), &tiles));
        out
    }

    /// Per region, how many of its areas are built (none before the first pass makes the outlines).
    fn region_progress(&self, root: &Path, regions: &[recipes::Recipe]) -> BTreeMap<String, build::RegionState> {
        let Some(date) = crate::osmpass::latest_pass(root) else { return BTreeMap::new() };
        let manifest: BTreeMap<String, String> = std::fs::read(root.join("state/build/manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let Ok(covs) = self.coverage(root, &manifest, &date, regions, false) else { return BTreeMap::new() };
        let (cov, each) = (&covs.all, &covs.each);
        let mut keys = build::Keys::load(root);
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        let tiles = self.terrain_tiles(root, &manifest);
        let inputs = input_digests(root);
        // (As re-keyed: agent::rekey.)
        let times = rekey::FileTimes::new(root);
        rekey::as_read(&mut keys, cov, &date, &manifest, reach.as_deref(), &inputs, &tiles, &times);
        build::region_states(cov, each, &date, &manifest, &keys, reach.as_deref(), &tiles)
    }

    /// The records re-keyed (agent::rekey), and written whole when that changed anything: the build
    /// Mac's agent's, after its merge (the caller's: not a helper's, nor a dry run's). What it reads
    /// is made ready first (the indexes: up to a minute over SMB, the first time; the coverage, the
    /// reaches and the files' times), and the re-keying worked out: only when that changes the
    /// records is the build lock taken, as a merge takes it (a job saving holds it), and the
    /// records, read again under it, re-keyed and written. Before the first such write, a copy of
    /// them as they were (`REKEY_COPY`, never written over: what an older app goes back to). None
    /// when there's nothing to re-key by now (no pass or regions; a recipe that can't be read now,
    /// which would leave the coverage short), or the lock is held. (Without the pass's reaches, the
    /// units wait for them.)
    fn rekey_records(&self, root: &Path) -> Result<Option<rekey::Rekeyed>> {
        let Some(date) = crate::osmpass::latest_pass(root) else { return Ok(None) };
        let (recipes, unread) = recipes::load(&root.join("inputs/regions"));
        if recipes.is_empty() || !unread.is_empty() {
            return Ok(None);
        }
        let inputs = input_digests(root);
        let times = rekey::FileTimes::new(root);
        let rekeyed = |manifest: &BTreeMap<String, String>, keys: &mut build::Keys| -> Result<Option<rekey::Rekeyed>> {
            let covs = self.coverage(root, manifest, &date, &recipes, false).map_err(anyhow::Error::msg)?;
            let reach = self.current_reach(root, manifest, keys, &date).ok().flatten();
            let tiles = self.terrain_tiles(root, manifest);
            Ok(Some(rekey::rekey(keys, &covs.all, &date, manifest, reach.as_deref(), &inputs, &tiles, &times)))
        };
        let ahead: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json"))?;
        let r = rekeyed(&ahead, &mut build::Keys::load_strict(root)?)?;
        if !r.as_ref().is_some_and(rekey::Rekeyed::changed) {
            return Ok(r);
        }
        let Some(_lock) = crate::out::BuildLock::try_take(root)? else { return Ok(None) };
        let manifest: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json"))?;
        let jobs = root.join("state/build/jobs.json");
        let before = match std::fs::read(&jobs) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("read {}", jobs.display())),
        };
        let mut keys: build::Keys = serde_json::from_slice(&before).with_context(|| format!("parse {}", jobs.display()))?;
        let r = rekeyed(&manifest, &mut keys)?;
        if r.as_ref().is_some_and(rekey::Rekeyed::changed) {
            let copy = root.join(REKEY_COPY);
            match std::fs::metadata(&copy) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => crate::whole::write(&copy, &before)?,
                Err(e) => return Err(e).with_context(|| format!("look for {}", copy.display())),
            }
            keys.save(root)?;
        }
        Ok(r)
    }

    /// `keys` as every reader reads them (agent::rekey::as_read: re-keyed, terrain's and slope's
    /// derived), for a job's `--expect-same`; as they are when what that reads can't be told now.
    fn as_read(&self, root: &Path, mut keys: build::Keys) -> build::Keys {
        let Some(date) = crate::osmpass::latest_pass(root) else { return keys };
        let (recipes, unread) = recipes::load(&root.join("inputs/regions"));
        if recipes.is_empty() || !unread.is_empty() {
            return keys;
        }
        let Ok(manifest) = crate::out::read_record::<BTreeMap<String, String>>(&root.join("state/build/manifest.json")) else { return keys };
        let Ok(covs) = self.coverage(root, &manifest, &date, &recipes, false) else { return keys };
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        let tiles = self.terrain_tiles(root, &manifest);
        let times = rekey::FileTimes::new(root);
        rekey::as_read(&mut keys, &covs.all, &date, &manifest, reach.as_deref(), &input_digests(root), &tiles, &times);
        keys
    }

    /// The terrain packs' indexes `manifest` names, those not yet held read (from `pack-idx/`, else
    /// the NAS: half a minute for the whole build's, once).
    fn terrain_tiles(&self, root: &Path, manifest: &BTreeMap<String, String>) -> std::cell::Ref<'_, tiles::TerrainTiles> {
        let t = std::time::Instant::now();
        let n = self.tiles.borrow_mut().load(root, manifest);
        if n > 0 {
            eprintln!("agent: read the indexes of {n} terrain pack{} in {:.1} s", if n == 1 { "" } else { "s" }, t.elapsed().as_secs_f64());
        }
        self.tiles.borrow()
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
            Some(Held::Leased { dir, .. } | Held::Pooled { dir, .. }) => dir.join("done.txt"),
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
        // (In the pool no one records: the job's hand-off goes to the journal.)
        let (false, false, Some(root), true) = (self.o.helper, done.is_empty(), root, self.pool.is_none()) else { return false };
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
        let handed = if self.o.helper || recorded || self.pool.is_some() { done } else { Vec::new() };
        self.end_lease(k, Outcome::Interrupted, &handed, why);
    }

    /// Whether the pool's switch is known to be off (or shadowed): today's coordination goes on (the
    /// writer named, hand-offs merged, the records re-keyed). Not while it's unknown (the NAS not
    /// read yet) nor on.
    fn pool_off(&self) -> bool {
        matches!(self.pool_mode, Some(pool::Mode::Off | pool::Mode::Shadow))
    }

    /// Why this process restarts once its first job's slot is free (the pool's switch changed, or
    /// its part in the pool); None: it goes on.
    fn pool_restart(&self) -> Option<String> {
        self.restart_for.clone().or_else(|| self.pool.as_ref().and_then(|p| p.restart.clone()))
    }

    /// The pool's switch on the NAS, looked at each loop: a change from the one this process
    /// started with restarts it (into the new one: on, the agent's part is the terms'); off as it
    /// started, the agent is as it was. Shadowed, the shadow run beside it (crate::agent::shadow,
    /// in a folder of its own: `<home>/shadow`).
    fn pool_switch(&mut self, root: Option<&Path>) {
        let Some(r) = root.filter(|_| self._lock.is_some() && !self.o.dry_run) else { return };
        if let Some(on) = pool::slots_on(r) {
            if on != self.slots_on {
                eprintln!("agent: a part changing in this process is {}", if on { "on" } else { "off" });
            }
            self.slots_on = on;
            if let Some(run) = self.pool.as_mut() {
                run.in_process = on;
            }
        }
        let Some(m) = pool::mode(r) else { return };
        match self.pool_mode {
            None if m == pool::Mode::Off => self.pool_mode = Some(m),
            Some(was) if was == m => self.switch_seen = None,
            // (A change, read once: acted on if the next loop reads it too.)
            _ if self.switch_seen != Some(m) => self.switch_seen = Some(m),
            was => {
                if self.restart_for.is_none() {
                    let why = format!("the pool's switch says {} (this process started with it {})", serde_json::to_value(m).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(), was.map_or("unknown".to_string(), |w| serde_json::to_value(w).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()));
                    eprintln!("agent: {why}");
                    self.restart_for = Some(why);
                }
            }
        }
        // On, its member's lock held by another process as this one started: tried again, and once
        // taken, this process restarts into the pool (its part, its coordinator).
        if self.pool_mode == Some(pool::Mode::On) && self.pool.is_none() && self.restart_for.is_none() {
            let locks = self.o.home.parent().unwrap_or(&self.o.home).to_path_buf();
            if let Ok(id) = crate::pool::member_id(&self.o.home) {
                if let Ok(Some(_lock)) = crate::pool::MemberLock::take(&locks, &id) {
                    let why = "this Mac's member's lock is free again".to_string();
                    eprintln!("agent: {why}");
                    self.restart_for = Some(why);
                }
            }
        }
        if self.pool_mode == Some(pool::Mode::Shadow) && self.shadow.is_none() && !self.shadow_failed && self.restart_for.is_none() {
            match shadow::Shadow::open(r, &self.o.home.join("shadow"), &self.o.home, &self.app) {
                Ok(Some(sh)) => self.shadow = Some(sh.on(self.mac)),
                Ok(None) => {}
                Err(e) => {
                    eprintln!("agent: the pool's shadow run: {e:#}; not tried again in this process");
                    self.shadow_failed = true;
                }
            }
        }
    }

    /// The pool's step while it's on (docs/pool.md §12): the hand-offs from before the pool drained
    /// into the journal, settling's state handed over, the driver's step; then, leading, its
    /// coordinator told the term and whether it grants, and the files the lead keeps written (today's
    /// records for their readers, the raw tiles' archives named, the coordinator's state per term,
    /// its history). What it may do now; None when the pool isn't on.
    fn pool_step(&mut self, root: Option<&Path>, c: &Conditions, waiting: &mut Vec<Waiting>) -> Option<pool::Gates> {
        let lead = self.pool.as_ref()?.role == pool::Role::Lead;
        let Some(r) = root else {
            // (No NAS: no step; leading, nothing the driver hasn't said.)
            let run = self.pool.as_mut()?;
            run.gates = pool::Gates { term: run.gates.term, leads: run.gates.leads, ..Default::default() };
            if let Some(co) = &self.coord {
                co.set_moving(Some("the NAS isn't reachable".into()));
            }
            return Some(run.gates.clone());
        };
        // Settling a handover: the duties in flight cancelled (a catalog killed: it writes once at
        // its end; a sweep stopped), then the coordinator's state written, for the next step.
        // (So too when it no longer leads: a catalog or a sweep of a term it lost.)
        let settling = self.pool.as_ref().is_some_and(|p| p.gates.settle && p.settled.is_none());
        let lost = lead && self.pool.as_ref().is_some_and(|p| p.gates.leads.is_none());
        if settling || lost {
            for k in 0..SLOTS {
                let duty = self.slots[k].running.as_ref().and_then(|j| step_of(&j.spec.id)).is_some_and(|st| pool::PUBLISHES.contains(&st.as_str()) || pool::SWEEPS.contains(&st.as_str()));
                if duty {
                    let j = self.slots[k].running.as_mut().unwrap();
                    let why = if settling { "the lead hands the build over" } else { "this Mac no longer leads" };
                    eprintln!("agent: {} stopped: {why}", j.spec.id);
                    j.stop(Duration::from_secs(30));
                    self.stopped(k, root, &format!("stopped: {why}"));
                    self.slots[k].running = None;
                    std::fs::remove_file(self.record_path(k)).ok();
                }
            }
            if let (Some(co), Some(run), true) = (&self.coord, self.pool.as_mut(), settling) {
                co.set_moving(Some("settling a handover".into()));
                run.settled = serde_json::to_value(co.pool_state()).ok();
            }
        }
        let able = c.home && (c.ac || c.battery.is_none_or(|b| b >= cond::BATTERY_MIN)) && self.disk_free() >= room::RESERVE;
        #[cfg(test)]
        let able = TEST_ABLE.with(|t| t.get()).unwrap_or(able);
        let home = self.o.home.clone();
        // (Its update waiting on its own job: the lead to a member on the newer app.)
        self.hand_for_update();
        let run = self.pool.as_mut()?;
        // (The jobs an earlier process left, once `run` stopped any still running.)
        run.take_left(&home);
        run.drain(lead.then(|| home.join("coord/journal")).as_deref(), lead.then(|| crate::handoff::nas_base(r)).as_deref(), &home.join("outbox"));
        // The owner's lead asks (crate::agent::lead): this Mac's, and the build page's.
        run.conds = Some(pool::Conds { home: c.home, ac: c.ac, battery: c.battery, able });
        if let Some(co) = &self.coord {
            run.controls.page.extend(co.take_lead_asks());
        }
        lead::take(run, &home);
        let out = run.step(able);
        // (Its part changed, in this process: its coordinator started or stopped, its jobs going on;
        // not tried again once a restart is asked for, its coordinator failing to start.)
        if run.in_process && run.restart.is_none() {
            self.change_part(r, &out);
        }
        let keep: Vec<u64> = self.slots.iter().filter_map(|s| match &s.lease {
            Some(Held::Pooled { id, .. } | Held::Own(id)) => Some(*id),
            _ => None,
        }).collect();
        let run = self.pool.as_mut()?;
        pool::took_up(run, self.coord.as_ref(), &out, &self.host, &keep);
        lead::after(run, &out, r, &home, self.coord.as_ref());
        for e in &out.events {
            if let crate::pool::driver::Event::Failed { what, why } | crate::pool::driver::Event::Waits { what, why } = e {
                waiting.push(Waiting { step: None, what: "The pool".into(), why: format!("couldn't {what} yet: {why}") });
            }
        }
        let g = run.gates.clone();
        if let Some(co) = &self.coord {
            co.set_moving(match () {
                _ if g.leads.is_some() && g.duties => None,
                _ if g.settle => Some("settling a handover".into()),
                _ if g.leads.is_some() => Some("re-asserting its term".into()),
                _ => Some("no longer leads".into()),
            });
        }
        if lead && g.leads.is_some() {
            let saved = !out.events.iter().any(|e| matches!(e, crate::pool::driver::Event::Failed { what: "save the records", .. }));
            self.pool_lead_files(r, saved);
            // (A catalog and GC read today's files: caught up only once they hold these records.)
            let run = self.pool.as_mut()?;
            let behind = run.side.driver().records().is_some_and(|rec| rec.term >= 2 && run.today != Some((rec.term, rec.seq)));
            if behind {
                (run.gates.caught_up, run.gates.fresh) = (false, false);
                waiting.push(Waiting { step: None, what: "The pool".into(), why: "today's records files aren't written from the lead's yet".into() });
            }
            return Some(run.gates.clone());
        }
        Some(g)
    }

    /// This process's part changed with its step `out` (docs/pool.md §7.6, `pool::SLOTS` on): it took
    /// up a term (a member a moment ago), so it starts its coordinator and leads, the jobs it runs
    /// going on, their leases its own coordinator's from now (handed over with the term, or held
    /// again: `beat`); or it no longer leads one, so its coordinator stops (its contact taken off
    /// the NAS) and it works as a member, its jobs going on through the lead, wherever it is, by
    /// HTTP. A coordinator that can't start leaves the part to a restart, as before.
    fn change_part(&mut self, root: &Path, out: &crate::pool::driver::Out) {
        let Some(run) = self.pool.as_mut() else { return };
        match (run.role, out.leads) {
            (pool::Role::Member, Some(term)) => match make_coordinator(&self.o, Some(run), &self.app, &self.host) {
                Some(c) => {
                    eprintln!("agent: this Mac leads term {term}: its coordinator started, its jobs going on");
                    // (The terms' events its member kept meanwhile, for this coordinator's history.)
                    lead::replay(&self.o.home.join("pool"), &c);
                    run.role = pool::Role::Lead;
                    self.o.helper = false;
                    self.coord = Some(c);
                    self.published = None;
                }
                None => {
                    run.restart.get_or_insert_with(|| format!("it leads term {term}, and its coordinator didn't start in this process"));
                }
            },
            (pool::Role::Lead, None) if out.stop.is_none() => {
                eprintln!("agent: this Mac no longer leads: its coordinator stopped, its jobs going on through the lead");
                run.role = pool::Role::Member;
                // (Its coordinator first: what ends from here, the duties stopped below too, ends
                // through the new lead, `end_lease`.)
                let old = self.coord.take();
                if let Some(c) = &old {
                    c.unpublish(root);
                    c.stop();
                }
                self.o.helper = true;
                self.client = None;
                // (Its duties in flight stopped, a catalog or a sweep: the lead's alone.)
                for k in 0..SLOTS {
                    let duty = self.slots[k].running.as_ref().and_then(|j| step_of(&j.spec.id)).is_some_and(|st| pool::PUBLISHES.contains(&st.as_str()) || pool::SWEEPS.contains(&st.as_str()));
                    if duty {
                        let j = self.slots[k].running.as_mut().unwrap();
                        eprintln!("agent: {} stopped: this Mac no longer leads", j.spec.id);
                        j.stop(Duration::from_secs(30));
                        self.stopped(k, Some(root), "stopped: this Mac no longer leads");
                        self.slots[k].running = None;
                        std::fs::remove_file(self.record_path(k)).ok();
                    }
                }
                drop(old);
                self.published = None;
                self.lead_awake_off();
            }
            _ => {}
        }
    }

    /// The idle-sleep assertion the lead holds while it leads with leases out (docs/pool.md §6.7), so
    /// no member stalls behind a lead that dozed off with no job of its own; let go otherwise.
    fn lead_awake(&mut self) {
        let leading = self.pool.as_ref().is_some_and(|p| p.role == pool::Role::Lead && p.gates.leads.is_some());
        let leases = self.coord.as_ref().is_some_and(|c| c.leases_out() > 0);
        if !(leading && leases && self.slots_on) {
            self.lead_awake_off();
            return;
        }
        if self.lead_awake.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))) {
            return;
        }
        self.lead_awake = std::process::Command::new("/usr/bin/caffeinate").args(["-i", "-w", &std::process::id().to_string()]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().ok();
    }

    fn lead_awake_off(&mut self) {
        if let Some(mut c) = self.lead_awake.take() {
            c.kill().ok();
            c.wait().ok();
        }
    }

    /// A lead whose update waits on its own job (a newer app installed, a slot busy: it restarts
    /// into it only once both are free) hands the lead to a member already on that app, which the
    /// same-app rule otherwise leaves idle meanwhile (docs/plan.md §8, The same app): asked once per
    /// app, as the owner's ask would be, if such a member can lead now.
    fn hand_for_update(&mut self) {
        let Some(newer) = self.pending_app() else { return };
        let busy = self.slots.iter().any(|s| s.running.is_some());
        if !busy || !self.slots_on || self.update_handed.as_deref() == Some(newer.as_str()) {
            return;
        }
        let Some(run) = self.pool.as_mut().filter(|p| p.role == pool::Role::Lead && p.gates.leads.is_some()) else { return };
        let Some(m) = run.controls.view.as_ref().and_then(|v| v.members.iter().find(|m| !m.me && m.app == newer && m.can_lead).cloned()) else { return };
        eprintln!("agent: a newer app ({newer}) waits on this Mac's job; handing the lead to {}, on it", m.host);
        run.controls.page.push(crate::control::LeadRequest { ask: crate::control::LeadAsk::Give { to: m.member.clone() }, by: format!("this Mac's update to {newer}, waiting on its job"), at: now_s() });
        self.update_handed = Some(newer);
    }

    /// What the lead keeps written besides its records (docs/pool.md §12): today's three files from
    /// a term's records after the first (term 1's own saves write them; `saved`: this step's
    /// records are on the NAS), the raw tiles' archives its records hold named in the raw store's
    /// index, the coordinator's state of its term, its history's new events.
    fn pool_lead_files(&mut self, root: &Path, saved: bool) {
        let Some(run) = self.pool.as_mut() else { return };
        if let Some(rec) = run.side.driver().records() {
            if saved && rec.term >= 2 && rec.seq > 0 && run.today != Some((rec.term, rec.seq)) {
                match pool::write_today(run.side.nas(), rec) {
                    Ok(()) => run.today = Some((rec.term, rec.seq)),
                    Err(e) => eprintln!("agent: today's records from term {}'s: {e:#}", rec.term),
                }
            }
            let new: Vec<(String, crate::rawpack::Pack)> = rec.raw.iter().filter(|(_, p)| !run.named_raw.contains(&p.name)).cloned().collect();
            if !new.is_empty() {
                if let Ok(Some(lock)) = crate::out::BuildLock::try_take(root) {
                    match crate::rawpack::name_handed(&root.join("sources/aws-terrarium"), &new, &lock) {
                        Ok(n) => {
                            let again: BTreeSet<&str> = n.again.iter().map(|(_, p)| p.name.as_str()).collect();
                            run.named_raw.extend(new.iter().filter(|(_, p)| !again.contains(p.name.as_str())).map(|(_, p)| p.name.clone()));
                            if n.new > 0 {
                                eprintln!("agent: named {} raw tiles' archive{} in the raw store's index", n.new, if n.new == 1 { "" } else { "s" });
                            }
                        }
                        Err(e) => eprintln!("agent: naming the raw tiles' archives: {e:#}"),
                    }
                }
            }
        }
        let Some(co) = &self.coord else { return };
        let term = run.gates.leads.unwrap_or(run.gates.term);
        if let Ok(b) = serde_json::to_vec(&co.pool_state()) {
            if run.state_written.as_ref() != Some(&(term, b.clone())) {
                match run.side.nas().write_whole(&pool::state_path(term), &b) {
                    Ok(()) => run.state_written = Some((term, b)),
                    Err(e) => eprintln!("agent: the coordinator's state of term {term}: {e:#}"),
                }
            }
        }
        let events = co.history_since(run.history_seq);
        if run.history_seq == 0 {
            // (From this process's start: what came before is in the coordinator's own history.)
            run.history_seq = events.last().map_or(0, |e| e.seq).max(1);
        } else if !events.is_empty() {
            // (The terms' events are on the NAS already: crate::agent::lead appends them as they come.)
            let last = events.last().map_or(run.history_seq, |e| e.seq);
            let events: Vec<crate::coord::history::Event> = events.into_iter().filter(|e| e.kind != "term").collect();
            match pool::append_history(root, &run.side.member().id, &events) {
                Ok(()) => run.history_seq = last,
                Err(e) => eprintln!("agent: the history on the NAS: {e:#}"),
            }
        }
    }

    /// The newer app installed locally, when there's one (`newer_app`): its version.
    fn pending_app(&self) -> Option<String> {
        let apps = self.o.bin.parent()?;
        if self.app == "development" {
            return None;
        }
        std::fs::read_link(apps.join("current")).ok().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned())).filter(|cur| *cur != self.app)
    }

    /// The gate's part of a loop: the listing thread started (on the lead, a lone build Mac, a dry
    /// run: not a member's), and the asks of it taken up: a listing now (and a full check), and
    /// acceptances, which this Mac writes itself (docs/inputs.md §4.5).
    fn tend_gate(&mut self, root: &Path) {
        if !self.o.helper && self.inputs.watch.as_ref().is_none_or(|(r, _)| r != root) {
            self.inputs.watch = Some((root.to_path_buf(), crate::inputs::watch::Watch::start(root.to_path_buf(), crate::inputs::watch::EVERY)));
        }
        // (Not a dry run's: the asks are the agent's that runs.)
        if self.o.dry_run {
            return;
        }
        let mut asks = crate::inputs::take_asks(&self.o.home);
        if let Some(c) = &self.coord {
            asks.extend(c.take_inputs_asks());
        }
        if asks.is_empty() {
            return;
        }
        let member = crate::inputs::member_of(&self.o.home);
        for a in asks {
            if a.check || a.full {
                if a.full {
                    self.inputs.full.borrow_mut().insert(a.unit.clone());
                }
                eprintln!("agent: {} asked to check {}{}", if a.by.is_empty() { "someone" } else { &a.by }, a.unit, if a.full { " in full" } else { "" });
            }
            if a.all || !a.accept.is_empty() || !a.unaccept.is_empty() {
                // The unit's held report as the last plan read it, else as the records name it now.
                let shown = self.inputs.view.borrow().iter().find(|v| v.unit == a.unit).map(|v| crate::inputs::Report { findings: v.findings.iter().map(|s| s.finding.clone()).collect(), ..Default::default() });
                let held = shown.or_else(|| {
                    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).ok()?;
                    crate::inputs::read_report(root, m.get(&crate::inputs::held_logical(&a.unit))?).ok()
                });
                match crate::inputs::apply_ask(root, &a, held.as_ref(), &member) {
                    Ok(said) => {
                        for s in said {
                            eprintln!("agent: {} ({}): {s}", a.unit, a.by);
                        }
                    }
                    Err(e) => eprintln!("agent: {}'s ask of {}: {e:#}", a.by, a.unit),
                }
            }
        }
        if let Some((_, w)) = &self.inputs.watch {
            w.ask();
        }
    }

    /// The gate's checks to run (docs/inputs.md §4.3): each unit on the gate whose check's key
    /// (its listing, acceptances, accepted index) isn't the one last recorded, a full check daily
    /// or when asked; and the status's entries, from the records it plans with.
    fn gate_work(&self, root: &Path, manifest: &BTreeMap<String, String>, keys: &build::Keys, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let units = crate::inputs::units(root);
        let mut jobs = Vec::new();
        let mut checking: BTreeSet<String> = BTreeSet::new();
        // (Running now, on this Mac.)
        for r in self.slots.iter().filter_map(|s| s.running.as_ref()) {
            if let Some(w) = r.spec.record.as_ref().filter(|w| w.step == "inputs") {
                checking.extend(w.targets.iter().filter_map(|t| t.0.strip_prefix("inputs/").map(str::to_string)));
            }
        }
        if let (Some((_, watch)), false) = (&self.inputs.watch, self.o.helper) {
            let s = |p: &Path| p.to_string_lossy().into_owned();
            for unit in &units {
                let Some(checks) = crate::inputs::checks(unit) else { continue };
                if let Some(why) = watch.failed(unit) {
                    waiting.push(Waiting { step: Some("inputs".into()), what: format!("Checking {unit}"), why: format!("its drop box can't be listed now: {why}") });
                }
                let Some(l) = watch.get(unit) else { continue };
                let full = self.inputs.full.borrow().contains(*unit) || self.due(&format!("inputs {unit} full"), Duration::from_secs(86400));
                let mut key = crate::inputs::check_key(checks, &l.listing, &l.accepted, manifest);
                if full {
                    key = build::h(&[&key, "full"]);
                }
                let target = format!("inputs/{unit}");
                if keys.lo.get(&target) == Some(&key) {
                    continue;
                }
                checking.insert(unit.to_string());
                // (The listing and acceptances the key is made from, for the job to check exactly
                // them: inputs::Planned.)
                let planned = self.o.home.join("inputs-listing").join(format!("{unit}.{key}.json"));
                let wrote = std::fs::create_dir_all(planned.parent().unwrap()).map_err(anyhow::Error::from).and_then(|()| {
                    for e in std::fs::read_dir(planned.parent().unwrap())?.flatten() {
                        if e.file_name().to_string_lossy().starts_with(&format!("{unit}.")) && e.path() != planned {
                            std::fs::remove_file(e.path()).ok();
                        }
                    }
                    crate::whole::write(&planned, &serde_json::to_vec(&crate::inputs::Planned { listing: l.listing.clone(), accepted: l.accepted.clone() })?)
                });
                if let Err(e) = wrote {
                    waiting.push(Waiting { step: Some("inputs".into()), what: format!("Checking {unit}"), why: format!("its listing can't be kept for the check: {e:#}") });
                    continue;
                }
                let mut cmd = vec![s(&self.o.bin.join("scenic-build")), "inputs".into(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch").join("inputs")), unit.to_string(), "--listing".into(), s(&planned)];
                if full {
                    cmd.push("--full".into());
                }
                let id = if full { format!("inputs {unit} full") } else { format!("inputs {unit}") };
                jobs.push(JobSpec { id, what: format!("Checking the inputs dropped in {unit}{}", if full { ", every file" } else { "" }), cmd, needs: Needs { nas: true }, restart_after_sleep: true, record: Some(build::Work { step: "inputs".into(), targets: vec![(target, key)] }) });
            }
        }
        let accepted = self.inputs.watch.as_ref().map(|(_, w)| w.accepted()).unwrap_or_default();
        *self.inputs.view.borrow_mut() = crate::inputs::view::of(root, manifest, &units, &checking, &accepted, &mut self.inputs.cache.borrow_mut());
        jobs
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
/// ("ferries-freq", by content), which keys inputs/keys.env holds ("keys", `key_names`), and the 3D
/// buildings' downloaded sources ("bld-release", "bldprep 6/x/y", "bldprep-rows 6/x/y":
/// crate::bld::sources::digests).
pub fn input_digests(root: &Path) -> BTreeMap<String, String> {
    let mut inputs: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(root.join("inputs/ferries/freq")) {
        // (Read as kept while unchanged: crate::smallfiles.)
        let mut files: Vec<(String, std::sync::Arc<Vec<u8>>)> = rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), crate::smallfiles::read(&e.path()).ok()?))).collect();
        files.sort();
        let all: Vec<u8> = files.iter().flat_map(|(n, b)| n.bytes().chain(b.iter().copied())).collect();
        inputs.insert("ferries-freq".into(), store::naming::hash16(&all));
    }
    // The regions as a catalog records them; "?" when they can't be read now (build::catalog_work
    // then waits).
    inputs.insert("regions".into(), regions_digest(root).unwrap_or_else(|| "?".into()));
    inputs.insert("keys".into(), key_names(&root.join("inputs/keys.env")).unwrap_or_else(|| "?".into()));
    // The 3D buildings' sources: what each z6 tile's bldprep reads, and its rows.
    inputs.extend(crate::bld::sources::digests(root, crate::buildtiles::RELEASE));
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
        let text = crate::smallfiles::read_to_string(&e.path()).ok()?;
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

/// A freeing toward the room target's goal past the target, in words: ", and the 30.0 GB a job
/// waiting for it needs past it"; none when it's the target.
fn room_past(target: u64, goal: u64) -> String {
    if goal > target {
        format!(", and the {} a job waiting for it needs past it", room::size(goal - target))
    } else {
        String::new()
    }
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
        Some(why) => Some((Mode::Freeze, why)),
        None => pause.map(|p| (p.mode, p.why())),
    }
}

/// Why a job can't run under `c`, if it can't.
fn lapsed(n: &Needs, c: &Conditions) -> Option<String> {
    (n.nas && !c.nas).then(|| "the NAS isn't reachable".into())
}

/// The lead's coordinator for an agent of options `o` on app `app`, on Mac `host` (its own jobs'
/// leases held under its name), the pool's `run` when it's on
/// (its token copied from the pool first: the same on every lead); None when it can't start (this
/// Mac then builds alone, or, a member that took the lead in its process, restarts into it).
fn make_coordinator(o: &Options, run: Option<&pool::Run>, app: &str, host: &str) -> Option<crate::coord::Coordinator> {
    if let Some(r) = run {
        if let Err(e) = pool::seed(r.side.nas(), &o.home.join("coord")) {
            eprintln!("agent: the pool's token: {e:#}");
        }
    }
    match crate::coord::Coordinator::start(&o.home.join("coord"), Some(o.bin.join("wasm")), coord_port(), host, app) {
        Ok(c) => {
            eprintln!("agent: coordinating at {}", c.contact.urls.join(", "));
            Some(c)
        }
        Err(e) => {
            eprintln!("agent: no coordinator ({e:#})");
            None
        }
    }
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

/// The targets of `w` made again as they are, expected the same (scenic-build trees, terrain and
/// slope `--expect-same`): tree cover's, terrain's and slope's pieces the records as read (`done`:
/// agent::rekey::as_read) have under the key they're built with, their mids made
/// (build::TreeWork::backfill, build::TerrainWork::backfill, and those an assembly needs). Any
/// other step's: none.
fn expect_same(w: &build::Work, done: &build::Keys) -> Vec<String> {
    if !matches!(w.step.as_str(), "trees" | "terrain" | "slope") {
        return Vec::new();
    }
    w.targets.iter().filter(|(t, k)| t.starts_with("6/") && done.recorded(&w.step, t) == Some(k.as_str())).map(|t| t.0.clone()).collect()
}

/// A step's targets in batches, each its own job recording its own targets (a failure or a restart
/// into a new app costs one batch, not the whole wave), with the step's total.
fn batches(plan: Vec<build::Work>) -> Vec<(build::Work, usize)> {
    let regions = regions_left(&plan);
    let mut out = Vec::new();
    for w in plan {
        let (n, total) = (job_size(&w.step, regions), w.targets.len());
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

/// A job's step and targets when both Macs run its step (crate::agent::steps::SHARED).
fn shared_targets(spec: &JobSpec) -> Option<(String, Vec<String>)> {
    spec.record.as_ref().filter(|w| steps::SHARED.contains(&w.step.as_str())).map(|w| (w.step.clone(), w.targets.iter().map(|t| t.0.clone()).collect()))
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

/// What a step's time here is kept under (`Memory::step_secs`): its name, with the way it runs now
/// once that's changed (crate::coord::cost_version), so a time measured another way (a z3 tile's
/// tree cover, a z6 tile's now) is never taken for it.
fn secs_key(step: &str) -> String {
    match crate::coord::cost_version(step) {
        0 => step.to_string(),
        v => format!("{step} v{v}"),
    }
}

/// About how long a target of `step` takes on the build Mac (seconds) until it's been timed there:
/// the forecast's first guess (crate::agent::forecast), from the jobs' logs of 2026-10 (a tree cover
/// piece's from its z3 tiles' whole runs: 8,171 s for ~370 z6 tiles).
fn first_secs(step: &str) -> f64 {
    match step {
        "pass-sets" => 5000.0,
        "trailends" => 15.0,
        "reach" => 1800.0,
        "terrain-z8" | "buildings" | "items" => 3600.0,
        "summits" => 30.0,
        "labels" => 2200.0,
        "water" => 3600.0,
        "heritage-sites" => 240.0,
        // (Reading the latest basemap's water under every piece near the coverage.)
        "terrain-water" => 900.0,
        "heritage" => 5400.0,
        // (Terrain's and slope's pieces a z6 tile, their assemblies a z3 tile's zoomed-out levels:
        // the areas' last runs, 9,707 s and ~4,100 s over ~400 z6 tiles, 2026-10.)
        "terrain" => 25.0,
        "terrain-lo" => 60.0,
        "slope" => 10.0,
        "slope-lo" => 15.0,
        "trees" => 25.0,
        "trees-lo" => 10.0,
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
        // (A z6 tile on average, B2's estimate from B1's pilot on the build Mac: bldprep 2.2–4.7 s a
        // million rows and ~2.5 s to start, ~440 M rows over 380 tiles; bldtiles 0.5–2.9 s a
        // million buildings, ~342 M. Fetching what's there already: listings, and the coverage
        // unioned.)
        "bld-fetch" => 300.0,
        "bldprep" => 6.0,
        "bldtiles" => 2.0,
        "catalog" => 60.0,
        "names-todo" => 1070.0,
        "spoken" => 60.0,
        _ => 300.0,
    }
}

/// Whether the regions' own work is in `plan` (terrain, slope, tree cover, units): the 3D buildings
/// then go in smaller jobs (`job_size`).
fn regions_left(plan: &[build::Work]) -> bool {
    plan.iter().any(|w| matches!(w.step.as_str(), "terrain" | "terrain-lo" | "slope" | "slope-lo" | "trees" | "trees-lo" | "unit"))
}

/// Targets per job of `step` (`batch_size`), but the 3D buildings' while the regions' own work is
/// left (`regions`): 2 bldprep and 4 bldtiles, so a job of theirs, the second job's beside the
/// regions', or a helper's, ends within a couple of minutes and the regions' work comes back first.
fn job_size(step: &str, regions: bool) -> usize {
    match step {
        "bldprep" if regions => 2,
        "bldtiles" if regions => 4,
        s => batch_size(s),
    }
}

/// Targets per job for the steps whose work is per area or tile (each z3 pack of terrain or slope
/// takes tens of minutes; an area's roads and scenery minutes; a z6 tile's tree cover, a z3 tile's
/// assembly of it, candidates, peaks and map tiles less: a job of a few minutes, for leases and
/// pausing): the steps table's.
fn batch_size(step: &str) -> usize {
    steps::batch(step)
}

/// An agent for a test: it reads the test's fixed Mac (cond::Mac::TEST), not the one running it.
#[cfg(test)]
fn test_agent(o: Options) -> Result<Agent> {
    Agent::new(o).map(|mut a| {
        a.mac = cond::Mac::TEST;
        a
    })
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
        // A piece its z6 tile's; an assembly its area's.
        assert!(terrain_reads("terrain 6/8/16", &packs.join("6-8-16.0123456789abcdef.tiles")) && !terrain_reads("terrain 6/8/16", &packs.join("6-8-17.0123456789abcdef.tiles")) && !terrain_reads("terrain 6/8/16", &packs.join("3-1-2.0123456789abcdef.tiles")));
        assert!(terrain_reads("terrain-lo 3/1/2", &packs.join("3-1-2.0123456789abcdef.tiles")) && !terrain_reads("terrain-lo 3/1/2", &packs.join("6-8-16.0123456789abcdef.tiles")));
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
        // The files they save count too (a tree cover piece's mid: build::tree_work), but one a
        // later hand-off removes.
        let save = |l: &str, c: Option<&str>| crate::handoff::Handoff { changes: [(l.to_string(), c.map(str::to_string))].into(), ..Default::default() };
        crate::handoff::write(&home.join("coord/journal/m1"), &save("work/trees-mid/6-28-16", Some("work/trees-mid/6-28-16.1111111111111111.sect"))).unwrap();
        crate::handoff::write(&home.join("coord/journal/m1"), &save("layers/trees-cover/hi/6-28-17", Some("layers/trees-cover/hi/6-28-17.2222222222222222.pack"))).unwrap();
        crate::handoff::write(&home.join("coord/journal/m1"), &save("layers/trees-cover/hi/6-28-17", None)).unwrap();
        let k = a.planning_keys(&root).unwrap();
        assert_eq!((k.recorded("unit", "6/1/1"), k.recorded("unit", "6/1/2"), k.recorded("unit", "6/1/3")), (Some("k1"), Some("k2"), Some("k3")));
        assert_eq!(k.handed.iter().collect::<Vec<_>>(), ["work/trees-mid/6-28-16"]);
        assert_eq!(crate::handoff::merge_from(&root, &home.join("scratch/handoff"), &a.handoff_bases(&root)).unwrap(), 5);
        let k = build::Keys::load(&root);
        assert_eq!((k.recorded("unit", "6/1/2"), k.recorded("unit", "6/1/3")), (Some("k2"), Some("k3")));
        assert!(crate::handoff::waiting_in(&home.join("coord/journal")).unwrap().is_empty());
        assert!(k.handed.is_empty() && a.planning_keys(&root).unwrap().handed.is_empty(), "merged: in the manifest, never in the keys saved");
    }

    fn agent(root: &Path, home: &Path) -> Agent {
        test_agent(Options { root: Some(root.to_path_buf()), home: home.to_path_buf(), bin: PathBuf::from("/nonexistent/bin"), dry_run: true, once: true, helper: false }).unwrap()
    }

    #[test]
    fn a_helper_asks_only_for_what_its_disk_has_room_for() {
        let gb = |n: u64| n << 30;
        // 20 GB free and 10 of caches it may empty: the 15 GB steps (and their margin: tree cover's
        // pieces, terrain's and slope's, and the 3D buildings' among them) and tasks.
        assert_eq!(helper_steps(gb(20), gb(10), 0), ["terrain", "slope", "trees", "unit", "pois", "peaks", "bldprep", "bldtiles", "tail", "bldtile", "treeblock", "terrainsub"]);
        assert_eq!(helper_steps(gb(70), 0, 0), [steps::SHARED.to_vec(), vec!["tail", "bldtile", "treeblock", "terrainsub"]].concat());
        assert_eq!(helper_steps(gb(10), gb(5), 0), ["tail", "bldtile", "treeblock", "terrainsub"]);
        // (Tree cover's rows and terrain's subtrees need 1 GB.)
        assert_eq!(helper_steps(gb(3), gb(2), 0), ["treeblock", "terrainsub"]);
        assert!(helper_steps(gb(1), 0, 0).is_empty());
        // The owner's disk room target stays free past them: 70 GB free with a 60 GB target leaves
        // room for a task alone.
        assert_eq!(helper_steps(gb(70), 0, gb(60)), ["tail", "bldtile", "treeblock", "terrainsub"]);
        assert!(helper_steps(gb(70), 0, gb(69)).is_empty());
    }

    #[test]
    fn the_room_target_holds_jobs_and_frees_the_caches_toward_it() {
        room::TEST_FREE.with(|c| c.set(Some(40 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("sources")).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str| JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: None };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        // A 20 GB target, which the disk has (40 GB free): a terrain run's 55 GB and a unit's 30
        // past it don't fit, nor can the (empty) caches make them. Each waits, saying why; a job
        // after them needing as much isn't tried (the second unit), one needing less is: the
        // daily backup, which keeps no target, starts.
        room::set_target(&home, Some(20 << 30), "a test").unwrap();
        a.room_target = room::target(&home);
        let mut w = Vec::new();
        a.start_first(&[job("terrain 3/1/1"), job("unit 6/1/1"), job("unit 6/1/2"), job("backup daily")], &cond, Some(&root), &mut w);
        assert_eq!(w.iter().map(|w| w.what.as_str()).collect::<Vec<_>>(), ["terrain 3/1/1", "unit 6/1/1"], "{w:?}");
        assert!(w.iter().all(|w| w.why.contains("disk room target")), "{w:?}");
        assert_eq!(a.slots[0].running.as_ref().map(|r| r.spec.id.as_str()), Some("backup daily"));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots = Default::default();
        // The disk has the target but not the unit's room past it: the caches are due a freeing
        // toward that room (the least a held job needs), not only toward the target; the status
        // says nothing's short of the target, but without the NAS, that it can't be freed now.
        assert_eq!(a.toward_due(), Some(50 << 30));
        assert!(a.room_short(true).is_none());
        assert!(a.room_short(false).is_some_and(|s| s.contains("NAS isn't reachable")));
        // Tried again within ten minutes: held without making room again.
        let mut w = Vec::new();
        assert!(!a.try_start(0, job("unit 6/1/1"), &cond, Some(&root), &mut w));
        assert!(w[0].why.contains("disk room target"), "{w:?}");
        // The second slot's head needs as much: nothing starts beside it either.
        assert!(a.disk_free() < a.need_of(0, &job("unit 6/1/1")) + a.floor());
        // A 5 GB target: 35 GB, which the disk has; it starts.
        room::set_target(&home, Some(5 << 30), "a test").unwrap();
        a.room_target = room::target(&home);
        a.floor_short = None;
        assert!(a.try_start(0, job("unit 6/1/1"), &cond, Some(&root), &mut Vec::new()));
        // A target past what's free: between jobs (none runs), the caches are freed toward
        // it (all they hold here: their others too, not the canopy squares alone), and once they
        // have nothing more, the status says so.
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.slots = Default::default();
        let c = home.join("cache");
        std::fs::create_dir_all(c.join("base/base")).unwrap();
        std::fs::write(c.join("base/base/6-1-1.0000000000000001.base"), vec![0u8; 1000]).unwrap();
        std::fs::write(c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf"), vec![0u8; 100]).unwrap();
        // (The planet it's clipped from on the NAS: it can be made again.)
        std::fs::create_dir_all(root.join("sources/osm/2026-09-28")).unwrap();
        std::fs::write(root.join("sources/osm/2026-09-28/filtered.0000000000000001.osm.pbf"), b"x").unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), br#"{"sources/osm/2026-09-28/filtered":"sources/osm/2026-09-28/filtered.0000000000000001.osm.pbf"}"#).unwrap();
        // (The freeing's thread sees this Mac's disk: a target past what it has free.)
        let big = cond::free_bytes(&home).unwrap() + (100 << 30);
        room::set_target(&home, Some(big), "a test").unwrap();
        a.room_target = room::target(&home);
        assert_eq!(a.toward_due(), Some(big));
        // (One loop at a time: the freeing waited for, its disk this test's.)
        a.tend_caches(Some(&root), true);
        assert!(a.caches_task.is_none());
        let t = a.mem.toward.clone().unwrap();
        assert_eq!((t.target, t.bytes()), (Some(big), 1100), "{t:?}");
        assert!(!c.join("base/base/6-1-1.0000000000000001.base").exists() && !c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf").exists());
        assert_eq!(a.toward_due(), None, "not again for ten minutes, or until a job ends");
        let short = a.room_short(true).unwrap();
        assert!(short.contains("nothing more to free"), "{short}");
        let v = a.caches_view(None, true, true).room.unwrap();
        assert_eq!((v.target.map(|t| t.bytes), v.free), (Some(big), 40 << 30));
        // A job ended since: tried again; the target off, nothing is short.
        a.mem.worked_at = now_s() + 1;
        assert_eq!(a.toward_due(), Some(big), "a job ended since: tried again");
        room::set_target(&home, None, "a test").unwrap();
        a.room_target = room::target(&home);
        assert!(a.toward_due().is_none() && a.room_short(true).is_none());
    }

    #[test]
    fn toward_the_target_it_frees_while_a_job_runs_but_not_what_the_job_holds() {
        room::TEST_FREE.with(|c| c.set(Some(40 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("sources")).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.free_set = Some(40 << 30);
        let (used, idle) = (home.join("cache/base/base/6-1-1.0000000000000001.base"), home.join("cache/base/base/6-1-2.0000000000000002.base"));
        put(&used, &[0; 1000]);
        put(&idle, &[0; 500]);
        // A job runs (the daily backup: it keeps no target), holding one of the packs.
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, waiting_job("backup daily"), &cond, Some(&root), &mut Vec::new()));
        store::cachefile::hold_existing(&used).unwrap();
        // A target past what's free (the freeing's thread sees this Mac's disk): freed toward now,
        // the job running, all but what it holds.
        let target = cond::free_bytes(&home).unwrap() + (100 << 30);
        room::set_target(&home, Some(target), "a test").unwrap();
        a.room_target = room::target(&home);
        assert!(!a.idle());
        assert_eq!(a.toward_due(), Some(target));
        a.tend_caches(Some(&root), true);
        let f = a.mem.toward.clone().unwrap();
        assert_eq!((f.target, f.bytes()), (Some(target), 500), "{f:?}");
        assert!(used.exists() && !idle.exists());
        store::cachefile::release(&used);
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
    }

    #[test]
    fn short_of_the_target_with_a_job_held_it_frees_once() {
        room::TEST_FREE.with(|c| c.set(Some(40 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("sources")).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str| JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: None };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        std::fs::create_dir_all(home.join("cache/base/base")).unwrap();
        std::fs::write(home.join("cache/base/base/6-1-1.0000000000000001.base"), vec![0u8; 1000]).unwrap();
        // A target past what's free (the freeing's thread sees this Mac's disk), and a unit held by
        // it: the caches are due a freeing toward the unit's room past the target.
        let target = cond::free_bytes(&home).unwrap() + (100 << 30);
        let room = target + (30 << 30);
        room::set_target(&home, Some(target), "a test").unwrap();
        a.room_target = room::target(&home);
        let mut w = Vec::new();
        a.start_first(&[job("unit 6/1/1")], &cond, Some(&root), &mut w);
        assert!(a.idle() && w[0].why.contains("disk room target"), "{w:?}");
        assert_eq!(a.toward_due(), Some(room));
        a.tend_caches(Some(&root), true);
        let f = a.mem.toward.clone().unwrap();
        assert_eq!((f.target, f.goal, f.bytes()), (Some(target), Some(room), 1000), "{f:?}");
        // Done, and short of both: no freeing again within ten minutes, toward the unit's room nor
        // toward the target, loop after loop (nor once the unit's wait is over).
        for _ in 0..3 {
            a.tend_caches(Some(&root), true);
            assert!(a.caches_task.is_none());
            assert_eq!(a.mem.toward.as_ref().map(|t| t.at), Some(f.at));
            assert_eq!(a.toward_due(), None);
        }
        a.floor_short = None;
        assert_eq!(a.toward_due(), None, "the target alone: tried lately toward more");
        // The status says why it's short, of the target the owner set.
        let short = a.room_short(true).unwrap();
        assert!(short.contains("nothing more to free"), "{short}");
        // A job ended since: tried again.
        a.mem.worked_at = now_s() + 1;
        assert_eq!(a.toward_due(), Some(target));
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
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: true }).unwrap();
        a.client = Some(crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1"));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut w = Vec::new();
        let jobs = a.helper_job(&root, &cond, &mut w);
        assert_eq!(jobs.len(), 1, "{w:?}");
        let j = &jobs[0];
        // The build Mac's own command for slope, its saves handed off to its lease's outbox.
        assert_eq!(j.id, "slope 3/2/2");
        assert!(j.cmd[1].starts_with("SCENIC_HANDOFF=") && j.cmd[2].starts_with("SCENIC_COSTS="));
        // (Its timings' record in its folder too, to go with its hand-off.)
        assert!(j.cmd[3].starts_with("SCENIC_TIMINGS=") && j.cmd[3].ends_with("/timings.json"));
        assert_eq!(&j.cmd[4..7], ["/app/scenic-build", "slope", "--root"]);
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
        // The 3D buildings: their tiles two at once, their parquet read one tile at a time; the
        // fetch beside them; nothing beside the water or the pass.
        assert!(!clash("bldtiles", "bldtiles") && clash("bldprep", "bldprep") && !clash("bldprep", "bldtiles"));
        assert!(!clash("bld-fetch", "bldprep") && clash("water", "bldtiles") && clash("bldprep", "osm-pass"));
        assert!(SECOND.contains(&"bld-fetch") && SECOND.ends_with(&["bldprep", "bldtiles"]));
    }

    #[test]
    fn a_3d_buildings_job_is_offered_by_the_rows_it_reads() {
        // B2's runs on the M1: Paris's 6/32/22 (16.5 M rows in its row groups) took 2.4 GB to read
        // and 1.2 GB to raise; Vermont's 6/19/23 (6.8 M) 1.2 and 0.9 GB. Offered above each.
        assert_eq!((bld_peak("bldprep", 16_505_246), bld_peak("bldtiles", 16_505_246)), (2818, 2012));
        assert!(bld_peak("bldprep", 6_771_191) > 1243 && bld_peak("bldtiles", 6_771_191) > 935);
        // A tile reading nothing (a GHSL tile alone): its fixed part.
        assert_eq!((bld_peak("bldprep", 0), bld_peak("bldtiles", 0)), (300, 250));
        assert_eq!((batch_size("bldprep"), batch_size("bldtiles")), (8, 16));
        // While the regions' own work is left, smaller jobs of them.
        let w = |step: &str, n: usize| build::Work { step: step.into(), targets: (0..n).map(|i| (format!("6/{i}/0"), format!("k{i}"))).collect() };
        let sizes = |plan: Vec<build::Work>| batches(plan).into_iter().filter(|(w, _)| w.step.starts_with("bld")).map(|(w, _)| (w.step, w.targets.len())).collect::<Vec<_>>();
        assert_eq!(sizes(vec![w("bldprep", 3), w("bldtiles", 5)]), [("bldprep".to_string(), 3), ("bldtiles".to_string(), 5)]);
        assert_eq!(sizes(vec![w("unit", 1), w("bldprep", 3), w("bldtiles", 5)]), [("bldprep".to_string(), 2), ("bldprep".to_string(), 1), ("bldtiles".to_string(), 4), ("bldtiles".to_string(), 1)]);
    }

    #[test]
    fn a_second_job_runs_beside_the_first() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.free_set = Some(40 << 30);
        a.mem_set = Some((16 << 10, 12 << 10));
        assert!(a.second_allowed());
        // (Each a shell waiting, given its scratch folder as the plan's jobs are.)
        let job = |id: &str| {
            let step = id.split(' ').next().unwrap();
            let scratch = home.join("scratch").join(step).to_string_lossy().into_owned();
            let record = Some(build::Work { step: step.into(), targets: vec![(id.split(' ').nth(1).unwrap().into(), "k".into())] });
            JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into(), "--scratch".into(), scratch], needs: Needs { nas: false }, restart_after_sleep: false, record }
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
    fn the_memory_guard_drains_the_job_beside_then_stops_the_one_past_the_limit_and_learns() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.free_set = Some(40 << 30);
        // (A Mac of 16 GB: its limit 12 GB.)
        a.mem_set = Some((16 << 10, 12 << 10));
        let job = |id: &str, targets: &[&str]| {
            let step = id.split(' ').next().unwrap();
            let record = Some(build::Work { step: step.into(), targets: targets.iter().map(|t| (t.to_string(), "k".to_string())).collect() });
            JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record }
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        // (A costs file an earlier job left, naming a target begun and never ended: gone as the next
        // starts, so it can't be blamed.)
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(a.costs_path(0), "{\"unit\":\"water water\",\"started\":1}\n").unwrap();
        assert!(a.try_start(0, job("terrain 6/1/1", &["6/1/1", "6/1/2"]), &cond, Some(&root), &mut Vec::new()));
        assert!(!a.costs_path(0).exists());
        assert!(a.try_start(1, job("bldtiles 6/2/2", &["6/2/2"]), &cond, Some(&root), &mut Vec::new()));
        let tick = |a: &mut Agent, held: Vec<Option<u64>>, trouble: bool| {
            a.sampler.set_test(Some(held), Some(trouble));
            a.sampler.tick();
            for k in 0..SLOTS {
                a.tend(k, &cond, Some(&root), 0).unwrap();
            }
        };
        // Together within the limit: nothing.
        tick(&mut a, vec![Some(7 << 10), Some(4 << 10)], false);
        assert!(!a.guard(Some(&root)) && a.slots[1].mem_drain.is_none());
        // Together past it, the larger fitting alone: the one beside it stops at its next safe point
        // (asked through its channel), and nothing starts beside the larger meanwhile.
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert!(!a.guard(Some(&root)));
        assert!(a.slots[1].mem_drain.as_deref().is_some_and(|w| w.contains("past this Mac's limit of 12.0 GB")), "{:?}", a.slots[1].mem_drain);
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert_eq!(std::fs::read_to_string(a.control_path(1)).unwrap(), "drain");
        a.start_second(&[job("unit 6/3/3", &["6/3/3"])], &cond, Some(&root));
        assert!(a.beside_why.as_deref().is_some_and(|w| w.contains("nothing starts beside terrain 6/1/1")), "{:?}", a.beside_why);
        // The drained one reaching no safe point in the time a pause gives: stopped at once.
        a.slots[1].drain_since = Some(Instant::now() - DRAIN_GRACE - Duration::from_secs(1));
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert!(a.guard(Some(&root)) && a.slots[1].running.is_none());
        // The larger past the limit by itself, the Mac not short of memory: it stops at its next safe
        // point; short of it, at once (the sampler froze it), given back, not failed, kept from this
        // Mac for an hour; what it held its target's floor, a batch's (two targets).
        tick(&mut a, vec![Some(13 << 10), None], false);
        assert!(!a.guard(Some(&root)) && a.slots[0].mem_drain.is_some() && a.slots[0].running.is_some());
        // (Past the time a pause gives a safe point, not short of memory: still only drained; a long
        // job alone may have none for hours.)
        a.slots[0].drain_since = Some(Instant::now() - DRAIN_GRACE - Duration::from_secs(1));
        tick(&mut a, vec![Some(13 << 10), None], false);
        assert!(!a.guard(Some(&root)) && a.slots[0].running.is_some());
        tick(&mut a, vec![Some(13 << 10), None], true);
        assert!(a.sampler.frozen(0));
        assert!(a.guard(Some(&root)) && a.slots[0].running.is_none());
        let c = a.coord.as_ref().unwrap();
        assert_eq!((c.floor("terrain", "6/1/1").map(|f| (f.mb, f.alone)), c.floor("terrain", "6/1/2")), (Some((13 << 10, false)), None));
        assert!(!a.mem.retry.contains_key("terrain 6/1/1"), "not held against it as a failure");
        assert!(a.wait_reason(&job("terrain 6/1/1", &["6/1/1"]), &cond).is_some_and(|w| w.contains("tried here again in 60 min")));
        let v = a.guard_view();
        assert!(v.on && v.limit_mb == 12 << 10 && v.last.as_ref().is_some_and(|(_, w)| w.contains("terrain 6/1/1 stopped by the memory guard: it held 13.0 GB")), "{v:?}");
        // A batch's floor past the limit: the target tried again in a job of its own, not held.
        let works = vec![build::Work { step: "terrain".into(), targets: vec![("6/1/1".into(), "k".into()), ("6/1/2".into(), "k".into())] }];
        let h = a.guard_holds(&works);
        assert!(h.here.is_empty() && h.alone.contains(&("terrain".to_string(), "6/1/1".to_string())));
        // Learned alone (a job of that one target): held here, and everywhere while no Mac has room,
        // the status saying why; with a Mac of more room in the pool, left to it.
        a.guard_backoff.clear();
        assert!(a.try_start(0, job("terrain 6/1/1", &["6/1/1"]), &cond, Some(&root), &mut Vec::new()));
        tick(&mut a, vec![Some(14 << 10), None], true);
        assert!(a.guard(Some(&root)));
        let c = a.coord.as_ref().unwrap();
        assert_eq!(c.floor("terrain", "6/1/1").map(|f| (f.mb, f.alone)), Some((14 << 10, true)));
        let h = a.guard_holds(&works);
        assert!(h.here.contains(&("terrain".to_string(), "6/1/1".to_string())) && h.here.len() == 1 && h.all.len() == 1 && h.alone.is_empty());
        assert!(h.why[0].why.contains("more than any Mac in the pool has"), "{:?}", h.why);
        let w = crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{}", c.contact.urls[0].rsplit(':').next().unwrap())], c.contact.token.clone(), "big");
        let _ = w.ask(&crate::coord::Ask { kind: "native".into(), can: vec!["terrain".into()], mem_mb: 30 << 10, limit_mb: Some(42 << 10), ..Default::default() });
        let h = a.guard_holds(&works);
        assert!(h.here.len() == 1 && h.all.is_empty() && h.why[0].why.contains("left to a Mac with room"), "{:?}", h.why);
        // The job beside the larger drained when the larger is the second slot's: the first slot
        // starts nothing until the larger ends.
        assert!(a.try_start(1, job("bldtiles 6/5/5", &["6/5/5"]), &cond, Some(&root), &mut Vec::new()));
        assert!(a.try_start(0, job("pack 6/4/4", &["6/4/4"]), &cond, Some(&root), &mut Vec::new()));
        tick(&mut a, vec![Some(4 << 10), Some(9 << 10)], false);
        assert!(!a.guard(Some(&root)) && a.slots[0].mem_drain.is_some());
        assert!(a.waits_for_second(&job("unit 6/3/3", &["6/3/3"])).is_some_and(|w| w.contains("held more memory together")));
        // This Mac's memory unknown: nothing stopped, the status saying why.
        a.mem_set = Some((0, 0));
        tick(&mut a, vec![Some(20 << 10), Some(20 << 10)], true);
        assert!(!a.guard(Some(&root)) && a.slots[0].running.is_some() && a.slots[1].running.is_some());
        assert!(a.guard_view().why_off.is_some_and(|w| w.contains("can't be read")));
        // The switch off: nothing stopped, nothing held, a job the sampler froze let go on; what's
        // learned is kept all the same.
        a.mem_set = Some((16 << 10, 12 << 10));
        std::fs::create_dir_all(root.join("state/pool")).unwrap();
        std::fs::write(root.join(memguard::SWITCH), "off").unwrap();
        a.guard_on = memguard::on(&root);
        tick(&mut a, vec![None, Some(20 << 10)], true);
        assert!(!a.guard(Some(&root)) && a.slots[1].running.is_some());
        assert!(a.guard_holds(&works).here.is_empty());
        for k in 0..SLOTS {
            if let Some(r) = a.slots[k].running.as_mut() {
                r.stop(Duration::from_secs(5));
            }
        }
    }

    #[test]
    fn the_first_job_isnt_starved_by_the_second() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.free_set = Some(40 << 30);
        let job = |id: &str| {
            let step = id.split(' ').next().unwrap();
            let scratch = home.join("scratch").join(step).to_string_lossy().into_owned();
            let record = Some(build::Work { step: step.into(), targets: vec![(id.split(' ').nth(1).unwrap().into(), "k".into())] });
            JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into(), "--scratch".into(), scratch], needs: Needs { nas: false }, restart_after_sleep: false, record }
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
        // The heritage chain beside: the items' facts, listed first, can't share the Mac with it (two
        // Wikidata steps): the first slot takes the map tiles meanwhile, the facts said waiting.
        assert!(a.try_start(1, job("heritage heritage"), &in_use, Some(&root), &mut Vec::new()));
        let plan = [job("items items"), job("pack 6/1/1")];
        let mut w = Vec::new();
        a.start_first(&plan, &in_use, Some(&root), &mut w);
        assert_eq!(a.slots[0].running.as_ref().map(|r| r.spec.id.as_str()), Some("pack 6/1/1"));
        assert!(w.iter().any(|x| x.what == "items items" && x.why.contains("don't run together")), "{w:?}");
        assert!(a.passed_over.contains_key("items items"));
        // The heritage chain over: the facts start in the free slot.
        if let Some(r) = a.slots[1].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.slots[1] = Slot::default();
        a.start_second(&plan, &in_use, Some(&root));
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.spec.id.as_str()), Some("items items"), "{:?}", a.beside_why);
        stop(&mut a);
        // Passed over, started in the second slot, ended more than half an hour after it was first
        // passed over: it holds nothing up (no longer passed over once it started, and only a job of
        // the plan counts): the second slot starts other work.
        assert!(a.try_start(0, job("pack 6/1/8"), &in_use, Some(&root), &mut Vec::new()));
        a.passed_over.insert("items items".into(), Instant::now() - PASS_MAX - Duration::from_secs(1));
        assert!(a.try_start(1, job("items items"), &in_use, Some(&root), &mut Vec::new()));
        assert!(!a.passed_over.contains_key("items items"));
        if let Some(r) = a.slots[1].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.slots[1] = Slot::default();
        a.passed_over.insert("items items".into(), Instant::now() - PASS_MAX - Duration::from_secs(1));
        a.start_second(&[job("unit 6/3/3")], &in_use, Some(&root));
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.spec.id.as_str()), Some("unit 6/3/3"), "{:?}", a.beside_why);
        stop(&mut a);
        a.passed_over.clear();
        // Passed over too long: the first slot waits for it, and the second starts nothing new but it.
        assert!(a.try_start(1, job("heritage heritage"), &in_use, Some(&root), &mut Vec::new()));
        a.passed_over.insert("items items".into(), Instant::now() - PASS_MAX - Duration::from_secs(1));
        let mut w = Vec::new();
        a.start_first(&plan, &in_use, Some(&root), &mut w);
        assert!(a.slots[0].running.is_none(), "it waits for the facts");
        if let Some(r) = a.slots[1].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.slots[1] = Slot::default();
        assert!(a.try_start(0, job("pack 6/1/9"), &in_use, Some(&root), &mut Vec::new()));
        a.start_second(&[job("unit 6/3/3"), job("items items")], &in_use, Some(&root));
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.spec.id.as_str()), Some("items items"), "{:?}", a.beside_why);
        stop(&mut a);
        a.passed_over.clear();
        // A job that runs alone next: waited for, not passed over for the map tiles after it.
        assert!(a.try_start(1, job("heritage heritage"), &in_use, Some(&root), &mut Vec::new()));
        let mut w = Vec::new();
        a.start_first(&[job("gc gc"), job("pack 6/1/2")], &in_use, Some(&root), &mut w);
        assert!(a.slots[0].running.is_none());
        assert!(w.iter().any(|x| x.what == "gc gc" && x.why.contains("don't run together")), "{w:?}");
        stop(&mut a);
        // One that needs room made on the disk: not beside another job (it may read what's deleted):
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
        let a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
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
        let dry = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
        dry.keep_round(build::Round { began: 200, ..r }).unwrap();
        assert_eq!(file().began, 100);
    }

    /// An agent that runs jobs (no dry run), the NAS at `root`: the build Mac's, or a helper's.
    fn running_agent(root: &Path, home: &Path, helper: bool) -> Agent {
        test_agent(Options { root: Some(root.to_path_buf()), home: home.to_path_buf(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper }).unwrap()
    }

    /// A forecast made now: the build done (nothing left to build), or not.
    fn forecast_now(done: bool) -> forecast::Forecast {
        forecast::Forecast { at: now_s(), done_at: (!done).then(|| now_s() + 3600), why: done.then(|| forecast::NOTHING_LEFT.to_string()), ..Default::default() }
    }

    /// A job that waits half a minute, needing nothing.
    fn waiting_job(id: &str) -> JobSpec {
        JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: None }
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
        // A job running here is no reason (what it uses it holds, store::cachefile): trimmed, but
        // for what it holds.
        put(&copy(3), &[1; 1000]);
        put(&copy(4), &[1; 1000]);
        store::cachefile::hold_existing(&copy(4)).unwrap();
        a.mem.trimmed.as_mut().unwrap().at -= 10;
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, waiting_job("pack 6/1/1"), &cond, Some(&root), &mut Vec::new()));
        assert_eq!(a.tend_caches(Some(&root), true), None);
        assert!(!copy(3).exists() && copy(4).exists());
        assert_eq!(a.mem.trimmed.as_ref().map(|t| (t.bytes(), t.left)), Some((1000, 1000)));
        store::cachefile::release(&copy(4));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        // Not without the NAS (what goes must be kept there).
        put(&copy(3), &[1; 1000]);
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
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: false, helper: false }).unwrap();
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        // A trim under way (one that waits to be let go): the loop isn't held, and jobs start
        // meanwhile (what they use they hold: store::cachefile).
        let (go, wait) = std::sync::mpsc::channel::<()>();
        a.caches_task = Some(CachesTask { ask: None, toward: None, began: Instant::now(), thread: std::thread::spawn(move || wait.recv().map(|()| room::Freed { freed: BTreeMap::from([("blobs".to_string(), 1000)]), ..Default::default() }).map_err(anyhow::Error::from)) });
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.starts_with("this Mac's caches are being trimmed")));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, waiting_job("pack 6/1/1"), &cond, Some(&root), &mut Vec::new()));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        // Done: what it freed kept.
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
        let mut left = std::process::Command::new("/bin/sh").args(["-c", "sleep 3600"]).process_group(0).spawn().unwrap();
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
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: false, helper: false }).unwrap();
        *a.forecast.borrow_mut() = Some(forecast_now(true));
        let (go, wait) = std::sync::mpsc::channel::<()>();
        a.caches_task = Some(CachesTask { ask: None, toward: None, began: Instant::now(), thread: std::thread::spawn(move || wait.recv().map(|()| room::Freed::default()).map_err(anyhow::Error::from)) });
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
        // Asked while a job an earlier agent left runs here (its programs may not hold what they
        // use): not cleared, and why said; the ask taken up all the same.
        use std::os::unix::process::CommandExt;
        let mut left = std::process::Command::new("/bin/sh").args(["-c", "sleep 3600"]).process_group(0).spawn().unwrap();
        let pgid = left.id() as i32;
        a.orphans.push(jobs::Group { pgid, leader_start: crate::sys::process_start(pgid).unwrap(), started: now_s() - 5, id: "backup".into() });
        let r = room::request_clear(&home, "scenic clean on m4").unwrap();
        a.tend_caches(Some(&root), true);
        let f = a.mem.declined.clone().unwrap();
        assert_eq!((f.asked, f.bytes(), f.why_not.as_deref()), (Some(r.at), 0, Some("a job an earlier agent left still runs here (backup)")));
        assert!(!asked(&home) && c.join("chm10/a.tif").exists() && a.mem.cleared.is_none());
        left.kill().unwrap();
        left.wait().unwrap();
        // The build done, a job of this agent's running: cleared, the canopy squares too, and what
        // it freed said.
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, waiting_job("backup"), &cond, Some(&root), &mut Vec::new()));
        let r = room::request_clear(&home, "the menu bar on m4").unwrap();
        assert_eq!(a.tend_caches(Some(&root), true), None);
        let f = a.mem.cleared.clone().unwrap();
        assert_eq!((f.asked, f.by.as_deref(), f.why_not.as_deref()), (Some(r.at), Some("the menu bar on m4"), None));
        assert_eq!(f.freed, BTreeMap::from([("base".to_string(), 500), ("blobs".to_string(), 1000), ("canopy".to_string(), tif.len() as u64)]));
        assert!(!c.join("chm10/a.tif").exists() && root.join("sources/canopy/a.tif").exists() && c.join("unit-stages.json").exists());
        assert!(!asked(&home));
        assert_eq!(a.cache_size.lock().unwrap().1, Some(room::Sizes::default()), "counted again: nothing left to clear");
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        // Declined later (no NAS): said apart, the last clear done kept.
        room::request_clear(&home, "scenic clean on m4").unwrap();
        a.tend_caches(None, true);
        assert_eq!(a.mem.declined.as_ref().and_then(|f| f.why_not.as_deref()), Some("the NAS isn't reachable"));
        assert_eq!(a.mem.cleared.as_ref().map(|f| f.asked), Some(Some(r.at)));
        // A dry run beside it (another agent runs the jobs) takes up no ask.
        room::request_clear(&home, "scenic clean on m4").unwrap();
        let mut dry = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
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

    /// A root as the build has it before the units' keys changed: a pass whose reaches build
    /// Reykjavik's unit (6/28/17) for region a, the heritage sites and the terrain made (the area's
    /// lo pack a real pack), and the unit built under the old keys, with its outputs. Its old key.
    fn rekey_root(root: &Path) -> String {
        let date = "2026-09-28";
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        put(&root.join(format!("sources/osm/{date}/pass.1111111111111111.json")), b"{}");
        recipes::add(&root.join("inputs/regions"), &recipes::Recipe { id: "a".into(), name: "A".into(), outline: vec!["place:-21.9,64.13,20".into()] }).unwrap();
        let (recipes, _) = recipes::load(&root.join("inputs/regions"));
        let cov = crate::coverage::Coverage::from_recipes(&recipes, None, &root.join("inputs/outlines")).unwrap();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let piece = format!("sources/osm/{date}/pieces/6-28-17");
        m.insert(piece.clone(), format!("{piece}.4444444444444444.osm.pbf"));
        let mut reaches = crate::reach::Reaches { fmt: 1, date: date.into(), ..Default::default() };
        reaches.units.insert("6/28/17".into(), crate::reach::Reach { owned: Some([-220_000_000, 640_000_000, -217_000_000, 641_600_000]), long: vec![] });
        let rc = format!("{}.5555555555555555.json.zst", crate::reach::logical(date));
        put(&root.join(&rc), &reaches.encode().unwrap());
        m.insert(crate::reach::logical(date), rc);
        // (What the units wait for: the heritage sites' inputs and outputs, the buildings' index.)
        m.insert(crate::osmpass::set_name(date, "areas"), format!("sources/osm/{date}/sets/areas.1212121212121212.osm.pbf"));
        m.insert("sources/registers/legacy".into(), "sources/registers/legacy.3434343434343434.tar.zst".into());
        m.insert(crate::buildtiles::index_logical(), format!("{}.6767676767676767.json", crate::buildtiles::index_logical()));
        m.insert(crate::heritage::base_logical(date, "heritage-sources"), format!("work/heritage/{date}/base/heritage-sources.5656565656565656.json"));
        let lo = "layers/terrain/lo/3-3-2.1111111111111111.pack";
        std::fs::create_dir_all(root.join(lo).parent().unwrap()).unwrap();
        let mut w = store::pack::PackWriter::create(&root.join(lo), serde_json::json!({"layer": "terrain"}), false).unwrap();
        for z in 3..=8u8 {
            for x in 3 << (z - 3)..4 << (z - 3) {
                for y in 2 << (z - 3)..3 << (z - 3) {
                    w.add(z, x, y, format!("{z}/{x}/{y}").as_bytes(), 0).unwrap();
                }
            }
        }
        w.finish().unwrap();
        m.insert("layers/terrain/lo/3-3-2".into(), lo.into());
        m.insert("base/6-28-17".into(), "base/6-28-17.6666666666666666.base".into());
        // Tree cover's z3 tile, its whole run's packs written now (by the trees program).
        for (l, c) in [("layers/trees-cover/lo/3-3-2", "layers/trees-cover/lo/3-3-2.7777777777777777.pack"), ("layers/trees-cover/hi/6-28-17", "layers/trees-cover/hi/6-28-17.8888888888888888.pack")] {
            put(&root.join(c), b"pack");
            m.insert(l.into(), c.into());
        }
        put(&root.join("state/build/manifest.json"), &serde_json::to_vec(&m).unwrap());
        let mut keys = build::Keys::default();
        keys.lo.insert("reach".into(), build::reach_key(date, &m).unwrap());
        keys.record("heritage-sites", &build::heritage_sites_work(&cov, date, &m, &keys).unwrap().targets);
        keys.record("terrain", &rekey::v1::terrain_slope_targets(&cov, &m).0);
        keys.record("trees", &rekey::v1::trees_targets(&cov, &m));
        let old = rekey::v1::unit_keys(&cov, date, &m, Some(&reaches), &input_digests(root)).pop().unwrap();
        assert_eq!(old.0.slash(), "6/28/17");
        keys.record("unit", &[("6/28/17".into(), old.1.clone())]);
        // (Under the new keys as recorded, it would be built again.)
        let tiles = {
            let mut t = tiles::TerrainTiles::new(None);
            t.load(root, &m);
            t
        };
        // (Terrain's z3 record read as its pieces and assembly, as every reader reads it.)
        let mut read = keys.clone();
        rekey::derive(&mut read, &cov, &m, &tiles);
        let plan = build::plan(&cov, date, &m, &read, &input_digests(root), Some(&reaches), &tiles, build::Rounds { each: &cov.by_region(), on_map: &BTreeMap::new(), since_last: None, current: None, held: false });
        assert!(plan.work.iter().any(|w| w.step == "unit" && w.targets[0].0 == "6/28/17"), "{:?}", plan.work);
        keys.save(root).unwrap();
        old.1
    }

    #[test]
    fn the_build_macs_loop_re_keys_the_records_once_a_dry_run_or_a_helper_never() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let old = rekey_root(&root);
        let jobs = root.join("state/build/jobs.json");
        let before = std::fs::read(&jobs).unwrap();
        // A dry run plans as the records will be (the unit built), and writes nothing.
        let mut dry = agent(&root, &d.path().join("dry"));
        dry.step().unwrap();
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(!dry.plan(&root, &cond, &mut Vec::new()).iter().any(|j| j.id.starts_with("unit ")));
        assert_eq!(std::fs::read(&jobs).unwrap(), before);
        // Nor a helper.
        running_agent(&root, &d.path().join("helper"), true).step().unwrap();
        assert_eq!(std::fs::read(&jobs).unwrap(), before);
        // The build Mac's (paused: nothing starts), while a job saving holds the build lock: what it
        // reads is made ready (its indexes kept in its folder), and nothing's written.
        let home = d.path().join("m4");
        let mut a = running_agent(&root, &home, false);
        a.pause = Some(crate::control::Pause::new(crate::control::Mode::Drain, "the test"));
        let held = crate::out::BuildLock::take(&root).unwrap();
        assert!(a.rekey_records(&root).unwrap().is_none());
        assert!(home.join("pack-idx/1111111111111111.idx").exists());
        assert_eq!(std::fs::read(&jobs).unwrap(), before);
        drop(held);
        // (A sibling test's child may hold the lock a moment, between its fork and its exec: stepped
        // again until the records are re-keyed, five minutes a watchdog.)
        let rekeyed = |a: &mut Agent| {
            let t0 = Instant::now();
            loop {
                a.step().unwrap();
                if build::Keys::load(&root).unit["6/28/17"] != old {
                    break;
                }
                assert!(t0.elapsed() < Duration::from_secs(300), "not re-keyed");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        // Then once, the records as they were kept beside them first: the unit's, and tree cover's
        // z3 tile as its pieces and assembly.
        rekeyed(&mut a);
        let k = build::Keys::load(&root).unit["6/28/17"].clone();
        let keys = build::Keys::load(&root);
        assert!(!keys.trees.contains_key("3/3/2") && keys.trees.contains_key("6/28/17") && keys.trees_lo.contains_key("3/3/2"), "{:?} {:?}", keys.trees, keys.trees_lo);
        // Terrain's z3 record read as its pieces and assembly, in memory only: never written.
        assert_eq!((keys.terrain.keys().map(String::as_str).collect::<Vec<_>>(), keys.terrain_lo.len(), keys.slope_lo.len()), (vec!["3/3/2"], 0, 0));
        assert_eq!(std::fs::read(root.join(REKEY_COPY)).unwrap(), before);
        let (after, at) = (std::fs::read(&jobs).unwrap(), std::fs::metadata(&jobs).unwrap().modified().unwrap());
        a.step().unwrap();
        assert_eq!((std::fs::read(&jobs).unwrap(), std::fs::metadata(&jobs).unwrap().modified().unwrap()), (after, at), "not written again");
        // A late record under the old key (an older app's job, merged): re-keyed; the copy stays
        // the first's.
        let mut late = build::Keys::load(&root);
        late.record("unit", &[("6/28/17".into(), old.clone())]);
        late.save(&root).unwrap();
        rekeyed(&mut a);
        assert_eq!(build::Keys::load(&root).unit["6/28/17"], k);
        assert_eq!(std::fs::read(root.join(REKEY_COPY)).unwrap(), before);
        // Its new key is the plan's: nothing to build.
        assert!(!a.plan(&root, &cond, &mut Vec::new()).iter().any(|j| j.id.starts_with("unit ")));
    }

    #[test]
    fn the_checklist_says_which_steps_helpers_take() {
        let mut steps = build::checklist_to_come();
        build::mark_shared(&mut steps);
        let shared = |what: &str| steps.iter().find(|s| s.what == what).and_then(|s| s.shared.clone());
        assert_eq!(shared(build::TERRAIN).as_deref(), Some("tiles"), "the pieces, not the assemblies");
        assert_eq!(shared(build::SLOPE).as_deref(), Some("tiles"));
        assert_eq!(shared(build::UNITS).as_deref(), Some("all"));
        assert_eq!(shared(build::LANDMARKS).as_deref(), Some("candidates and peaks"));
        assert_eq!(shared(build::TREES).as_deref(), Some("tiles"), "the pieces, not the assemblies");
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

    /// The gate in the agent (docs/inputs.md §4.2, §4.3, §4.7): the drop boxes listed off the loop,
    /// a unit's check planned before every other job while its key isn't the one recorded (a full
    /// one daily), the status's entry; an acceptance asked of the agent written by it.
    #[test]
    fn the_gate_plans_a_check_first_and_shows_in_the_status() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let unit = crate::inputs::TEST_UNIT;
        let dropbox = root.join("inputs").join(unit);
        std::fs::create_dir_all(&dropbox).unwrap();
        let put = |text: &str| {
            let p = dropbox.join("a.jsonl");
            std::fs::write(&p, text).unwrap();
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(60)).unwrap();
        };
        put("{\"k\": \"x\", \"v\": -1}\n");
        let mut a = agent(&root, &home);
        // Off the gate (no flag): nothing planned.
        a.step().unwrap();
        let c = read_status(Some(&root), &home).unwrap().conditions;
        let mut w = Vec::new();
        assert!(!a.plan(&root, &c, &mut w).iter().any(|j| j.id.starts_with("inputs ")));
        std::fs::create_dir_all(root.join("state/inputs")).unwrap();
        std::fs::write(root.join(crate::inputs::TEST_FLAG), b"").unwrap();
        let watch = |a: &Agent| a.inputs.watch.as_ref().unwrap().1.get(unit);
        let ask = |a: &Agent| a.inputs.watch.as_ref().unwrap().1.ask();
        ask(&a);
        let t = Instant::now();
        while watch(&a).is_none() {
            assert!(t.elapsed() < Duration::from_secs(30), "listed");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The first check: a full one, before every other job.
        let plan = a.plan(&root, &c, &mut w);
        assert_eq!(plan[0].id, format!("inputs {unit} full"));
        assert!(plan[0].cmd[1] == "inputs" && plan[0].cmd.ends_with(&["--full".to_string()]) && plan[0].cmd.contains(&unit.to_string()));
        // (Given the listing and acceptances its key was made from: inputs::Planned.)
        let at = plan[0].cmd.iter().position(|a| a == "--listing").unwrap();
        let given: crate::inputs::Planned = serde_json::from_slice(&std::fs::read(&plan[0].cmd[at + 1]).unwrap()).unwrap();
        assert_eq!(given.listing.files.keys().cloned().collect::<Vec<_>>(), ["a.jsonl"]);
        let work = plan[0].record.clone().unwrap();
        assert_eq!(work.targets[0].0, format!("inputs/{unit}"));
        // Run (as its job would) and recorded: a normal check planned next (its key without the full
        // one's), then nothing while nothing changes.
        let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
        crate::inputs::gate::run(&mut out, unit, true, None).unwrap();
        let mut keys = build::Keys::load_strict(&root).unwrap();
        keys.record("inputs", &work.targets);
        keys.save(&root).unwrap();
        a.mem.last_ok.insert(format!("inputs {unit} full"), now_s());
        let plan = a.plan(&root, &c, &mut w);
        assert_eq!(plan[0].id, format!("inputs {unit}"));
        let work = plan[0].record.clone().unwrap();
        let mut keys = build::Keys::load_strict(&root).unwrap();
        keys.record("inputs", &work.targets);
        keys.save(&root).unwrap();
        assert!(!a.plan(&root, &c, &mut w).iter().any(|j| j.id.starts_with("inputs ")));
        // Held (the negative v): the status says so.
        a.step().unwrap();
        let st = read_status(Some(&root), &home).unwrap();
        let v = st.inputs.iter().find(|v| v.unit == unit).unwrap();
        assert_eq!(v.state, crate::inputs::view::State::Held);
        assert_eq!(v.held, ["a.jsonl"]);
        // An acceptance asked of the agent: written by it (not by a dry run's), and the drop boxes
        // listed again, so its key changes and the unit's checked again.
        a.o.dry_run = false;
        crate::inputs::ask(&home, &crate::inputs::Ask { unit: unit.into(), all: true, by: "a test".into(), ..Default::default() }).unwrap();
        a.tend_gate(&root);
        a.o.dry_run = true;
        assert_eq!(crate::inputs::acceptances(&root, unit).unwrap().len(), 1);
        let t = Instant::now();
        while watch(&a).is_none_or(|l| l.accepted.is_empty()) {
            assert!(t.elapsed() < Duration::from_secs(30), "listed again");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(a.plan(&root, &c, &mut w)[0].id, format!("inputs {unit}"));
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
        let (first, last) = a.edited_at.get().unwrap();
        assert_eq!(edit_held(Some((first, last)), last), Some((Duration::ZERO, EDIT_HOLD)));
        // A run of edits holds the work an hour at most.
        let now = std::time::SystemTime::now();
        let ago = |m: u64| now - Duration::from_secs(m * 60);
        assert_eq!(edit_held(Some((ago(50), ago(1))), now), Some((Duration::from_secs(60), Duration::from_secs(10 * 60))));
        assert!(edit_held(Some((ago(61), ago(1))), now).is_none());
        assert!(edit_held(Some((ago(20), ago(16))), now).is_none());
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
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: apps.join("v1"), dry_run: true, once: true, helper: false }).unwrap();
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
            [("unit", 6, 14), ("unit", 6, 14), ("unit", 2, 14), ("roadunits", 1, 1), ("terrain", 2, 2), ("pois", 3, 3)].map(|(s, n, t)| (s.to_string(), n, t))
        );
        // Every target once, in order, with its key.
        let units: Vec<&(String, String)> = b.iter().filter(|(w, _)| w.step == "unit").flat_map(|(w, _)| &w.targets).collect();
        assert_eq!(units.len(), 14);
        assert!(units.iter().enumerate().all(|(i, t)| t.0 == format!("6/{i}/0") && t.1 == format!("k{i}")));
    }

    /// A "none" terrain or slope piece (the coverage gone from its z6 tile) is never expected the
    /// same, its piece's earlier record or its own (a flap back) whatever: its job drops files.
    #[test]
    fn none_pieces_are_never_expected_the_same() {
        let mut done = build::Keys::default();
        done.terrain.insert("6/40/20".into(), "piece".into());
        done.slope.insert("6/40/20".into(), "piece".into());
        done.terrain.insert("6/41/20".into(), "earlier".into());
        for step in ["terrain", "slope"] {
            let w = build::Work { step: step.into(), targets: vec![("6/40/20".into(), build::none_piece_key(step, "6/40/20")), ("6/41/20".into(), build::none_piece_key(step, "6/41/20"))] };
            assert!(expect_same(&w, &done).is_empty(), "{step}");
        }
    }

    #[test]
    fn tree_pieces_made_again_under_their_recorded_key_are_expected_the_same() {
        let mut done = build::Keys::default();
        for (t, k) in [("6/28/16", "a"), ("6/28/17", "b"), ("3/3/2", "c")] {
            done.trees.insert(t.into(), k.into());
        }
        done.trees_lo.insert("3/3/2".into(), "d".into());
        let w = |step: &str, ts: &[(&str, &str)]| build::Work { step: step.into(), targets: ts.iter().map(|(t, k)| (t.to_string(), k.to_string())).collect() };
        // One by one in a batch (a helper takes the far end of an offer, so a batch can mix them): a
        // piece the records have under the key it's made with (its mid made), not one under another
        // key (stale) nor one never made.
        assert_eq!(expect_same(&w("trees", &[("6/28/16", "a"), ("6/28/17", "b2"), ("6/29/16", "e")]), &done), ["6/28/16"]);
        // A z3 tile's whole run, an assembly, another step: never.
        assert!(expect_same(&w("trees", &[("3/3/2", "c")]), &done).is_empty());
        assert!(expect_same(&w("trees-lo", &[("3/3/2", "d")]), &done).is_empty());
        assert!(expect_same(&w("unit", &[("6/28/16", "a")]), &done).is_empty());
        // The times kept here go by the way a step runs now: a z3 tile's tree cover isn't a piece's.
        assert_eq!([secs_key("trees"), secs_key("trees-lo"), secs_key("unit")], ["trees v2", "trees-lo v1", "unit v1"]);
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
        let n = Needs { nas: true };
        let at = |ac: bool, nas: bool, home: bool, battery: Option<u8>| Conditions { ac, nas, home, idle_s: 0, battery };
        assert!(lapsed(&n, &at(true, true, true, None)).is_none());
        assert!(lapsed(&n, &at(true, false, true, None)).unwrap().contains("NAS"));
        // At any charge, at home or away (the owner's choice): only the NAS.
        assert!(lapsed(&n, &at(false, true, true, None)).is_none());
        assert!(lapsed(&n, &at(false, true, true, Some(5))).is_none());
        assert!(lapsed(&n, &at(true, true, false, None)).is_none());
        assert!(lapsed(&Needs { nas: false }, &at(true, false, true, None)).is_none());
        // A running job stops at once without the NAS (it can't save), and as the build's pause says;
        // never for the battery.
        use crate::control::{Mode, Pause};
        let (drain, now) = (Pause::new(Mode::Drain, "the menu bar on m4"), Pause::new(Mode::Freeze, "scenic pause on m4"));
        let mode = |c: Conditions, p: Option<&Pause>| stop_for(&n, &c, p).map(|s| s.0);
        assert_eq!(mode(at(true, true, true, None), None), None);
        assert_eq!(mode(at(true, true, true, None), Some(&drain)), Some(Mode::Drain));
        assert_eq!(mode(at(true, true, true, None), Some(&now)), Some(Mode::Freeze));
        assert_eq!(mode(at(true, false, true, None), Some(&drain)), Some(Mode::Freeze));
        assert_eq!(mode(at(false, true, true, Some(25)), None), None);
        assert!(stop_for(&n, &at(true, true, true, None), Some(&drain)).unwrap().1.contains("the menu bar on m4"));
        // An older heartbeat without `home` reads as at home; an older job's record, with its power
        // and home, as its NAS alone.
        let old: Conditions = serde_json::from_str(r#"{"ac": true, "nas": true, "idle_s": 0}"#).unwrap();
        assert!(old.home);
        let old: Needs = serde_json::from_str(r#"{"cpu": true, "nas": true, "home": true}"#).unwrap();
        assert_eq!(old, Needs { nas: true });
    }
}

#[cfg(test)]
mod pool_tests {
    //! The agent with the pool (docs/pool.md §12, phase 1's integration): off, on (leading, and as
    //! a member), its gates, its switch changing, and shadowed.
    use super::*;
    use crate::pool::journal::LeaseId;

    /// A NAS folder as the pool begins: today's records, this Mac named their writer.
    fn nas(d: &Path) -> PathBuf {
        let r = d.join("nas");
        std::fs::create_dir_all(r.join("state/build")).unwrap();
        std::fs::create_dir_all(r.join("catalog")).unwrap();
        std::fs::write(r.join("state/build/manifest.json"), r#"{"base/6-1-1": "base/6-1-1.1111111111111111.base"}"#).unwrap();
        std::fs::write(r.join("state/build/jobs.json"), r#"{"unit": {"6/1/1": "k1"}}"#).unwrap();
        std::fs::write(r.join("state/build/pending.json"), "{}").unwrap();
        std::fs::write(r.join("state/build/writer"), format!("{}\n", cond::host_name())).unwrap();
        r
    }

    fn switch_on(r: &Path, which: &str) {
        std::fs::create_dir_all(r.join("state/pool")).unwrap();
        std::fs::write(r.join(which), "").unwrap();
    }

    /// An app folder whose programs (`scenic`, `scenic-build`) hand off a save of their step's and
    /// note what they were given (their environment, in `<app>/env-<step>`).
    fn app(d: &Path) -> PathBuf {
        let bin = d.join("app");
        std::fs::create_dir_all(&bin).unwrap();
        let script = format!(
            "#!/bin/sh\nstep=\"$1\"\nenv > \"{bin}/env-$step\"\nif [ -n \"$SCENIC_HANDOFF\" ]; then\n  mkdir -p \"$SCENIC_HANDOFF\"\n  case \"$step\" in\n    slope) printf '{{\"changes\":{{\"layers/slope/lo/3-2-2\":\"layers/slope/lo/3-2-2.0123456789abcdef.pack\"}}}}' > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n    *) printf '{{\"changes\":{{\"work/%s\":\"work/%s.0123456789abcdef.json\"}}}}' \"$step\" \"$step\" > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n  esac\nfi\nexit 0\n",
            bin = bin.display()
        );
        for p in ["scenic", "scenic-build"] {
            std::fs::write(bin.join(p), &script).unwrap();
            std::fs::set_permissions(bin.join(p), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        bin
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
    }

    /// Loops `a` until `done` says so, at most a minute.
    /// Steps `a` until `done` (its jobs and listings run meanwhile, as long as a busy Mac takes:
    /// `WATCHDOG` is a watchdog, not a measure).
    fn until(a: &mut Agent, done: impl Fn(&Agent) -> bool) {
        let end = Instant::now() + WATCHDOG;
        loop {
            a.step().unwrap();
            if done(a) {
                return;
            }
            assert!(Instant::now() < end, "not done in {WATCHDOG:?}: {:?}", read_status(None, &a.o.home).map(|s| s.waiting));
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    const WATCHDOG: Duration = Duration::from_secs(300);

    fn stop_jobs(a: &mut Agent) {
        for k in 0..SLOTS {
            if let Some(r) = a.slots[k].running.as_mut() {
                r.stop(Duration::from_secs(5));
            }
        }
    }

    #[test]
    fn the_pool_off_leaves_the_agent_as_it_was() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let mut a = test_agent(Options { root: Some(r.clone()), home: d.path().join("home"), bin: PathBuf::from("/nonexistent/bin"), dry_run: false, once: true, helper: false }).unwrap();
        assert!(a.pool.is_none() && a.pool_mode == Some(pool::Mode::Off));
        a.step().unwrap();
        // The build Mac named the writer, as before; nothing of the pool's written.
        std::fs::write(r.join("state/build/writer"), "another-mac").unwrap();
        a.writer_named = None;
        a.step().unwrap();
        assert_eq!(std::fs::read_to_string(r.join("state/build/writer")).unwrap(), cond::host_name());
        assert!(!r.join("state/pool").exists() && !r.join("state/build/terms").exists());
        assert!(read_status(Some(&r), &a.o.home).unwrap().pool.is_none());
        assert!(a.pool_restart().is_none());
    }

    #[test]
    fn the_switch_changing_restarts_the_agent_into_it() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let mut a = test_agent(Options { root: Some(r.clone()), home: d.path().join("home"), bin: PathBuf::from("/nonexistent/bin"), dry_run: false, once: true, helper: false }).unwrap();
        a.step().unwrap();
        assert!(a.pool_restart().is_none());
        switch_on(&r, pool::ENABLED);
        // (Read once: nothing yet; twice in a row: it restarts.)
        a.step().unwrap();
        assert!(a.pool_restart().is_none());
        a.step().unwrap();
        assert!(a.pool_restart().is_some_and(|w| w.contains("on")), "{:?}", a.pool_restart());
        let st = read_status(Some(&r), &a.o.home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.starts_with("restarting")), "{:?}", st.waiting);
    }

    #[test]
    fn the_build_mac_leads_and_hands_its_jobs_off_once_the_pool_is_on() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        let bin = app(d.path());
        let home = d.path().join("app-folder/agent");
        // (`--helper` passed, as the M1's launch file does: the pool says what it is.)
        let mut a = test_agent(Options { root: Some(r.clone()), home: home.clone(), bin: bin.clone(), dry_run: false, once: true, helper: true }).unwrap();
        let run = a.pool.as_ref().expect("the pool's part");
        assert_eq!((run.role, run.gates.leads), (pool::Role::Lead, Some(1)));
        assert!(!a.o.helper && a.coord.is_some(), "it leads: its coordinator runs");
        let me = run.side.member().id.clone();
        // (Its writer's file changed after term 1: no one names the writer any more.)
        std::fs::write(r.join("state/build/writer"), "another-mac").unwrap();
        // Its member's lock is in the app's folder; its saved state in its own.
        assert!(d.path().join(format!("app-folder/pool-{me}.lock")).exists() && home.join("pool/saved.json").exists());
        // Its first job (the daily backup) runs under a lease of its term, its saves handed off; the
        // records it merges are today's files' too (term 1's saves write them).
        until(&mut a, |a| crate::agent::build::Keys::load(a.o.root.as_ref().unwrap()).lo.is_empty() && crate::out::read_record::<BTreeMap<String, String>>(&a.o.root.as_ref().unwrap().join("state/build/manifest.json")).unwrap().contains_key("work/backup"));
        let env = std::fs::read_to_string(bin.join("env-backup")).unwrap();
        assert!(env.contains("SCENIC_HANDOFF=") && !env.contains("SCENIC_BUILD_MAC"), "{env}");
        assert!(env.contains(&format!("{}", home.join("pool/jobs/1-").display())), "{env}");
        let rec = a.pool.as_ref().unwrap().side.driver().records().unwrap().clone();
        assert!(rec.reflected.iter().any(|k| k.ends_with(&format!("/1-{}", rec.reflected.iter().next().unwrap().rsplit('-').next().unwrap()))));
        assert!(std::fs::read_dir(home.join("pool/jobs")).map_or(true, |mut d| d.next().is_none()), "its folder removed once its saved state held the entry");
        // The writer isn't named any more (its file as it was), the coordinator's state of its term
        // and its history are on the NAS, its heartbeat names it.
        assert_eq!(std::fs::read_to_string(r.join("state/build/writer")).unwrap(), "another-mac");
        assert!(r.join(pool::state_path(1)).exists());
        assert!(std::fs::read_dir(r.join("state/coord/history")).unwrap().flatten().any(|day| day.path().join(format!("{me}.jsonl")).exists()));
        assert!(r.join(crate::pool::beat::path(&me)).exists());
        let st = read_status(Some(&r), &home).unwrap();
        let pv = st.pool.expect("the pool in its status");
        assert_eq!((pv.role, pv.member.as_str()), (pool::Role::Lead, me.as_str()));
        // The daily sweep waits for a re-assertion, then runs on the step that made it, caught up:
        // term 2, whose records are written to today's files too.
        until(&mut a, |a| a.mem.last_ok.contains_key("gc"));
        assert!(a.pool.as_ref().unwrap().gates.term >= 2);
        assert!(r.join("state/build/terms/2.json").exists());
        let m: BTreeMap<String, String> = crate::out::read_record(&r.join("state/build/manifest.json")).unwrap();
        assert!(m.contains_key("work/gc"), "{m:?}");
        stop_jobs(&mut a);
    }

    #[test]
    fn a_member_asks_the_lead_and_its_job_reaches_the_records_through_the_journal() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        let bin = app(d.path());
        // The lead (this Mac is the writer: term 1 is its), and a member, another process with a
        // folder (and member) of its own; its `--helper` or not, the pool says.
        let dry = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: true, once: true, helper: false }).unwrap();
        assert!(dry.pool.is_none(), "a dry run takes no part");
        drop(dry);
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        assert_eq!(m.pool.as_ref().unwrap().role, pool::Role::Member);
        assert!(m.o.helper && m.coord.is_none());
        let (il, im) = (lead.pool.as_ref().unwrap().side.member().id.clone(), m.pool.as_ref().unwrap().side.member().id.clone());
        assert_ne!(il, im);
        // The lead's coordinator offers slope; the member asks, and runs it under `1-<lease>`.
        lead.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        until(&mut m, |m| m.slots[0].running.is_some() || m.pool.as_ref().is_some_and(|p| !p.entries.is_empty()));
        let env = std::fs::read_to_string(bin.join("env-slope")).unwrap_or_default();
        until(&mut m, |m| m.slots[0].running.is_none() && m.pool.as_ref().is_some_and(|p| p.entries.is_empty()));
        assert!(std::fs::read_to_string(bin.join("env-slope")).unwrap().contains(&format!("{}", d.path().join("m/agent/pool/jobs/1-").display())), "{env}");
        // Its entry in the journal, told to the lead by mail; the lead, knowing the member, merges it.
        lead.pool.as_mut().unwrap().side.know(&im);
        until(&mut lead, |l| l.pool.as_ref().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/2/2") == Some("k")));
        let rec = lead.pool.as_ref().unwrap().side.driver().records().unwrap().clone();
        assert_eq!(rec.manifest.get("layers/slope/lo/3-2-2").map(String::as_str), Some("layers/slope/lo/3-2-2.0123456789abcdef.pack"));
        let key = rec.reflected.iter().find(|k| k.contains("/1-")).cloned().unwrap();
        assert!(r.join(crate::pool::journal::path(&key)).exists());
        // Acknowledged: the member tells it no more.
        until(&mut m, |m| m.pool.as_ref().unwrap().side.driver().mine().to_tell(1).is_empty());
        // The lead's coordinator ended the lease, its target kept out of offers until the plan shows
        // it built; its own jobs hold none of it meanwhile.
        assert!(lead.coord.as_ref().unwrap().held("slope").is_empty());
        let lid = LeaseId { term: 1, n: key.rsplit('-').next().unwrap().parse().unwrap() };
        assert_eq!(lid.term, 1);
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    #[test]
    fn a_member_on_another_app_than_the_leads_gets_nothing_until_they_run_the_same() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        // The lead on a newer app than the member's (its updater ran first): the member gets
        // nothing, saying why, however long it asks.
        let theirs = m.app.clone();
        lead.coord.as_ref().unwrap().shared.lock().unwrap().app = "20261010-0900-aaaaaaa".into();
        lead.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        for _ in 0..3 {
            m.step().unwrap();
        }
        assert!(m.slots[0].running.is_none());
        let st: Status = serde_json::from_slice(&std::fs::read(m.o.home.join("helper.json")).unwrap()).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("it builds once it runs that one")), "{:?}", st.waiting);
        // The lead on the member's app (it updated, or the lead went to a Mac on it): it builds.
        lead.coord.as_ref().unwrap().shared.lock().unwrap().app = theirs;
        until(&mut m, |m| m.slots[0].running.is_some() || m.pool.as_ref().is_some_and(|p| !p.entries.is_empty()));
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    #[test]
    fn the_owners_ask_mid_job_hands_the_lead_over_and_the_job_reaches_the_new_lead() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        let bin = app(d.path());
        // (Its slope job runs until the test opens its gate: the ask comes while it runs, however
        // slowly a busy Mac takes the handover.)
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = slope ] && while [ ! -e '{}' ]; do sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        let im = m.pool.as_ref().unwrap().side.member().id.clone();
        lead.pool.as_mut().unwrap().side.know(&im);
        lead.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        until(&mut m, |m| m.slots[0].running.is_some());
        // The owner asks the lead, from its menu, to hand the build to the member, mid-job.
        crate::control::request_lead(&lead.o.home, crate::control::LeadAsk::Give { to: im.clone() }, "the menu bar on l").unwrap();
        fn both(lead: &mut Agent, m: &mut Agent, done: &dyn Fn(&Agent, &Agent) -> bool) {
            let end = Instant::now() + WATCHDOG;
            loop {
                lead.step().unwrap();
                m.step().unwrap();
                if done(lead, m) {
                    return;
                }
                assert!(Instant::now() < end, "not done in {WATCHDOG:?}: {:?}", lead.pool.as_ref().unwrap().controls.kept.asked);
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        both(&mut lead, &mut m, &|_, m| m.pool.as_ref().unwrap().side.driver().leads() == Some(2));
        // The member leads term 2 in this process (it restarts into its part once its slot is free);
        // the lead's ask is done, its process restarting into a member's.
        let asked = lead.pool.as_ref().unwrap().controls.kept.asked.clone().unwrap();
        assert_eq!(asked.ask, crate::control::LeadAsk::Give { to: im.clone() });
        both(&mut lead, &mut m, &|l, _| l.pool.as_ref().unwrap().controls.kept.asked.as_ref().is_some_and(|a| a.state == lead::State::Done));
        assert!(lead.pool_restart().is_some_and(|w| w.contains("no longer leads")), "{:?}", lead.pool_restart());
        assert!(m.pool_restart().is_some_and(|w| w.contains("leads term 2")));
        assert!(m.slots[0].running.is_some(), "the job still runs");
        std::fs::write(&gate, b"").unwrap();
        // The job, granted in term 1, ran on: its entry reached the new lead's records.
        both(&mut lead, &mut m, &|_, m| m.slots[0].running.is_none() && m.pool.as_ref().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/2/2") == Some("k")));
        // The statuses say so: the old lead's view, the history's terms.
        let st = read_status(None, &lead.o.home).unwrap();
        let v = st.pool.and_then(|p| p.lead).expect("the controls' view");
        assert_eq!(v.lead.as_ref().map(|l| l.member.as_str()), Some(im.as_str()));
        let events = lead.coord.as_ref().unwrap().history_since(0);
        assert!(events.iter().any(|e| e.kind == "term" && e.note.starts_with("term 2: handed over by")), "{:?}", events.iter().filter(|e| e.kind == "term").collect::<Vec<_>>());
        // And on the NAS, in the old lead's own history file, once each, with its step down.
        let il = lead.pool.as_ref().unwrap().side.member().id.clone();
        let day = crate::pool::journal::day(now_s()).unwrap();
        let mine = std::fs::read_to_string(r.join("state/coord/history").join(day).join(format!("{il}.jsonl"))).unwrap();
        assert_eq!(mine.matches("term 2: handed over by").count(), 1, "{mine}");
        assert!(mine.contains("stepped down from term 1: handed over to"), "{mine}");
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    #[test]
    fn a_part_changes_in_its_process_its_jobs_going_on_through_the_lead_wherever_it_is() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = slope ] && while [ ! -e '{}' ]; do sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.step().unwrap();
        assert!(lead.slots_on && m.slots_on);
        let (il, im) = (lead.pool.as_ref().unwrap().side.member().id.clone(), m.pool.as_ref().unwrap().side.member().id.clone());
        lead.pool.as_mut().unwrap().side.know(&im);
        // The member's job, granted by the lead in term 1, runs while the owner hands the lead to it.
        lead.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        until(&mut m, |m| m.slots[0].running.is_some());
        crate::control::request_lead(&lead.o.home, crate::control::LeadAsk::Give { to: im.clone() }, "the menu bar on l").unwrap();
        fn both(lead: &mut Agent, m: &mut Agent, done: &dyn Fn(&Agent, &Agent) -> bool) {
            let end = Instant::now() + WATCHDOG;
            loop {
                lead.step().unwrap();
                m.step().unwrap();
                if done(lead, m) {
                    return;
                }
                assert!(Instant::now() < end, "not done in {WATCHDOG:?}: {:?}", lead.pool.as_ref().unwrap().controls.kept.asked);
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        both(&mut lead, &mut m, &|l, m| m.pool.as_ref().unwrap().side.driver().leads() == Some(2) && m.coord.is_some() && l.coord.is_none());
        // Neither restarts: the member leads in its process, its coordinator started; the old lead
        // works as a member, its coordinator stopped; the job runs on.
        assert_eq!((lead.pool_restart(), m.pool_restart()), (None, None));
        assert!(m.slots[0].running.is_some(), "the job still runs");
        assert_eq!((m.pool.as_ref().unwrap().role, m.o.helper), (pool::Role::Lead, false));
        assert_eq!((lead.pool.as_ref().unwrap().role, lead.o.helper), (pool::Role::Member, true));
        std::fs::write(&gate, b"").unwrap();
        // The job, granted in term 1, ends through its own Mac's coordinator now: its entry in the new
        // lead's records, its lease ended there.
        both(&mut lead, &mut m, &|_, m| m.slots[0].running.is_none() && m.pool.as_ref().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/2/2") == Some("k")));
        assert!(m.coord.as_ref().unwrap().held("slope").is_empty());
        // The old lead, a member now, asks the new lead for work by HTTP and runs it, its entry
        // reaching the new lead's records; still no restart.
        m.pool.as_mut().unwrap().side.know(&il);
        m.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/3/2".into(), "k2".into(), 100)], batch: 2 }]);
        both(&mut lead, &mut m, &|_, m| m.pool.as_ref().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/3/2") == Some("k2")));
        assert_eq!((lead.pool_restart(), m.pool_restart()), (None, None));
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    #[test]
    fn the_leads_own_job_goes_on_across_a_handover_through_the_new_lead() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = pack ] && while [ ! -e '{}' ]; do sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.step().unwrap();
        let im = m.pool.as_ref().unwrap().side.member().id.clone();
        lead.pool.as_mut().unwrap().side.know(&im);
        // (Two Macs' names: the member's coordinator holds its own jobs' leases under its own, so
        // the old lead's, under the old lead's, stay when it takes up.)
        m.host = "the-member".into();
        // The lead's own job, under its coordinator's lease, its files in the agent's folder.
        let job = JobSpec { id: "pack 6/4/4".into(), what: "map tiles".into(), cmd: vec![bin.join("scenic-build").to_string_lossy().into_owned(), "pack".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "pack".into(), targets: vec![("6/4/4".into(), "kn".into())] }) };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, job, &cond, Some(&r), &mut Vec::new()));
        assert!(matches!(lead.slots[0].lease, Some(Held::Pooled { own: true, .. })));
        crate::control::request_lead(&lead.o.home, crate::control::LeadAsk::Give { to: im.clone() }, "the menu bar on l").unwrap();
        let both = |lead: &mut Agent, m: &mut Agent, done: &dyn Fn(&Agent, &Agent) -> bool| {
            let end = Instant::now() + WATCHDOG;
            loop {
                lead.step().unwrap();
                m.step().unwrap();
                if done(lead, m) {
                    return;
                }
                assert!(Instant::now() < end, "not done in {WATCHDOG:?}: {:?}", lead.pool.as_ref().unwrap().controls.kept.asked);
                std::thread::sleep(Duration::from_millis(250));
            }
        };
        both(&mut lead, &mut m, &|l, m| m.pool.as_ref().unwrap().side.driver().leads().is_some() && m.coord.is_some() && l.coord.is_none());
        // The old lead's job runs on, its lease renewed with the new lead by HTTP (it knows it: the
        // term's state handed over).
        assert!(lead.slots[0].running.is_some());
        let lid = match lead.slots[0].lease { Some(Held::Pooled { id, .. }) => id, _ => panic!() };
        assert!(m.coord.as_ref().unwrap().held("pack").contains("6/4/4"), "the new lead holds its lease");
        lead.slots[0].beaten = None;
        assert!(lead.beat(0, Some(&r)), "renewed by HTTP");
        assert!(matches!(lead.slots[0].lease, Some(Held::Pooled { id, .. }) if id == lid));
        // It ends: its entry in the new lead's records (the new lead reading the old one's mail),
        // its lease ended there.
        let il = lead.pool.as_ref().unwrap().side.member().id.clone();
        m.pool.as_mut().unwrap().side.know(&il);
        std::fs::write(&gate, b"").unwrap();
        both(&mut lead, &mut m, &|l, m| l.slots[0].running.is_none() && m.pool.as_ref().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("pack", "6/4/4") == Some("kn")));
        assert!(m.coord.as_ref().unwrap().held("pack").is_empty());
        assert_eq!((lead.pool_restart(), m.pool_restart()), (None, None));
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    #[test]
    fn a_coordinator_that_cant_start_in_the_process_leaves_the_part_to_a_restart() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin, dry_run: false, once: true, helper: false }).unwrap();
        m.step().unwrap();
        assert_eq!(m.pool.as_ref().unwrap().role, pool::Role::Member);
        // (The port its coordinator would take is the lead's, still answering.)
        let out = crate::pool::driver::Out { leads: Some(2), ..Default::default() };
        m.change_part(&r, &out);
        assert!(m.coord.is_none() && m.o.helper);
        assert!(m.pool_restart().is_some_and(|w| w.contains("didn't start")), "{:?}", m.pool_restart());
        drop(lead);
    }

    #[test]
    fn an_own_jobs_timings_for_the_lead_are_the_slots_job_alone() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let mut a = test_agent(Options { root: Some(r.clone()), home: d.path().join("agent"), bin: app(d.path()), dry_run: false, once: true, helper: false }).unwrap();
        let rec = || Some(crate::timings::RunRec { kind: "unit".into(), ..Default::default() });
        // (Left from the slot's last job.) A job starting there carries none of them.
        a.slots[0].timings_out = rec();
        let spec = JobSpec { id: "x x".into(), what: "x".into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: None };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        a.start(0, spec, &cond).unwrap();
        assert_eq!(a.slots[0].timings_out, None);
        // Taken as its lease ends, whichever way: a stopped job's too.
        a.slots[0].timings_out = rec();
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.end_lease(0, Outcome::Interrupted, &[], "stopped");
        assert_eq!(a.slots[0].timings_out, None);
        a.slots[0].running.take();
    }

    #[test]
    fn a_lead_that_loses_its_term_in_its_process_stops_its_duties_and_its_coordinator() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin, dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let catalog = JobSpec { id: "catalog catalog".into(), what: "catalog".into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "catalog".into(), targets: vec![("catalog".into(), "kc".into())] }) };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, catalog, &cond, Some(&r), &mut Vec::new()));
        let port = TEST_PORT.with(|p| p.get()).unwrap();
        // (Its step said it leads no term any more.)
        let out = crate::pool::driver::Out::default();
        lead.change_part(&r, &out);
        assert!(lead.slots[0].running.is_none(), "its catalog stopped");
        assert!(lead.coord.is_none() && lead.o.helper && lead.pool.as_ref().unwrap().role == pool::Role::Member);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(std::net::TcpListener::bind(("0.0.0.0", port)).is_ok(), "its port free");
    }

    #[test]
    fn a_coordinator_stopped_frees_its_port() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("a"), "m4", "");
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
        c.stop();
        drop(c);
        // (The next on the same port starts, a few tries while the first's listener closes.)
        let c2 = crate::coord::Coordinator::start(&d.path().join("b"), None, port, "m4", "").unwrap();
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
        drop(c2);
    }

    #[test]
    fn a_lead_whose_update_waits_on_its_job_hands_the_lead_to_a_member_on_it() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        switch_on(&r, pool::SLOTS);
        // Two published apps; the member runs the newer, which this Mac has installed (`current`)
        // while its job holds it on the older.
        let apps = d.path().join("v/app");
        let made = app(d.path());
        let (old, new) = (apps.join("20261010-0100-aaaaaaa"), apps.join("20261010-0200-bbbbbbb"));
        for v in [&old, &new] {
            std::fs::create_dir_all(v).unwrap();
            for p in ["scenic", "scenic-build"] {
                std::fs::copy(made.join(p), v.join(p)).unwrap();
            }
        }
        std::os::unix::fs::symlink(&new, apps.join("current")).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: old.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        // (A job of its own, holding its update.)
        let job = JobSpec { id: "pack 6/1/1".into(), what: "pack".into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "pack".into(), targets: vec![("6/1/1".into(), "k".into())] }) };
        lead.step().unwrap();
        assert_eq!(lead.pending_app().as_deref(), Some("20261010-0200-bbbbbbb"));
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: new.clone(), dry_run: false, once: true, helper: false }).unwrap();
        let im = m.pool.as_ref().unwrap().side.member().id.clone();
        lead.pool.as_mut().unwrap().side.know(&im);
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, job, &cond, Some(&r), &mut Vec::new()));
        let end = Instant::now() + WATCHDOG;
        while m.pool.as_ref().unwrap().side.driver().leads() != Some(2) {
            lead.step().unwrap();
            m.step().unwrap();
            assert!(Instant::now() < end, "not handed over: {:?}", lead.pool.as_ref().unwrap().controls.kept.asked);
            std::thread::sleep(Duration::from_millis(250));
        }
        assert!(lead.pool.as_ref().unwrap().controls.kept.asked.as_ref().is_some_and(|a| a.by.contains("update to 20261010-0200-bbbbbbb")), "{:?}", lead.pool.as_ref().unwrap().controls.kept.asked);
        assert_eq!(lead.update_handed.as_deref(), Some("20261010-0200-bbbbbbb"));
        assert!(lead.slots[0].running.is_some(), "its job runs on");
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    #[test]
    fn the_leads_gates_hold_a_catalog_and_a_sweep() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let home = d.path().join("home");
        let mut a = agent_dry(&r, &home);
        let nas_: pool::SharedNas = std::sync::Arc::new(crate::pool::nas::Share::new(&r));
        let side = pool::Side::open(&home, &home.join("pool"), d.path(), "development", nas_, false).unwrap().unwrap();
        a.pool = Some(pool::Run::new(side, pool::Role::Lead, pool::Gates { term: 1, leads: Some(1), duties: true, ..Default::default() }));
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 9999 };
        let job = |id: &str| JobSpec { id: id.into(), what: id.into(), cmd: vec!["/usr/bin/true".into()], needs: Needs { nas: true }, restart_after_sleep: false, record: None };
        // Not caught up: no catalog; a sweep asks for a re-assertion first.
        let mut w = Vec::new();
        assert!(!a.try_start(0, job("catalog"), &c, Some(&r), &mut w));
        assert!(w.iter().any(|w| w.why.contains("reflect the journal")), "{w:?}");
        assert!(!a.try_start(0, job("gc"), &c, Some(&r), &mut w));
        assert!(a.pool.as_ref().unwrap().reassert);
        // Caught up: the catalog; the sweep only on a step that re-asserted.
        a.pool.as_mut().unwrap().gates.caught_up = true;
        assert!(a.try_start(0, job("catalog"), &c, Some(&r), &mut Vec::new()), "would start (dry run)");
        assert!(!a.try_start(0, job("gc"), &c, Some(&r), &mut Vec::new()));
        a.pool.as_mut().unwrap().gates.fresh = true;
        assert!(a.try_start(0, job("gc"), &c, Some(&r), &mut Vec::new()));
        // Other work as before.
        a.pool.as_mut().unwrap().gates = pool::Gates::default();
        assert!(a.try_start(0, job("backup"), &c, Some(&r), &mut Vec::new()));
    }

    #[test]
    fn a_member_whose_lock_is_held_waits_for_it_then_restarts_into_the_pool() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::ENABLED);
        let home = d.path().join("app-folder/agent");
        let id = crate::pool::member_id(&home).unwrap();
        let held = crate::pool::MemberLock::take(&d.path().join("app-folder"), &id).unwrap().unwrap();
        let mut a = test_agent(Options { root: Some(r.clone()), home: home.clone(), bin: PathBuf::from("/nonexistent/bin"), dry_run: false, once: true, helper: false }).unwrap();
        assert!(a.pool.is_none() && a.coord.is_none() && !a.o.dry_run, "waiting, not a dry run for good");
        a.step().unwrap();
        let st = read_status(Some(&r), &home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("waiting for its lock")), "{:?}", st.waiting);
        assert!(a.pool_restart().is_none() && a.slots.iter().all(|s| s.running.is_none()));
        // (Today's coordination not taken up meanwhile: no writer named.)
        std::fs::write(r.join("state/build/writer"), "another-mac").unwrap();
        a.step().unwrap();
        assert_eq!(std::fs::read_to_string(r.join("state/build/writer")).unwrap(), "another-mac");
        drop(held);
        a.step().unwrap();
        assert!(a.pool_restart().is_some_and(|w| w.contains("lock")), "{:?}", a.pool_restart());
    }

    #[test]
    fn a_hand_off_said_journaled_is_journaled_while_the_coordinators_pool_is_off() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        c.offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        let cl = crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        let ask = crate::coord::Ask { kind: "native".into(), can: vec!["slope".into()], mem_mb: 64_000, max: 1, ..Default::default() };
        let g = cl.ask(&ask).unwrap().expect("granted");
        let h = crate::handoff::Handoff { changes: [("layers/slope/lo/3-2-2".to_string(), Some("layers/slope/lo/3-2-2.0123456789abcdef.pack".to_string()))].into(), done: Some(("slope".into(), vec![("3/2/2".into(), "k".into())])), ..Default::default() };
        cl.done(&crate::coord::Done { lease: g.lease, handoff: Some(h), journaled: true, ..Default::default() }).unwrap();
        // (Switched off with the member's job under way: its hand-off kept here, to be merged.)
        assert_eq!(crate::handoff::waiting_in(&c.journal()).unwrap().len(), 1);
    }

    #[test]
    fn scenic_pool_on_off_and_status() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let st = |pool: Option<PoolView>, job: bool| Status { host: "Brandons-MacBook-Pro".into(), app: "20261009-0000-bbbbbbb".into(), beat: now_s(), pool, job: job.then(|| JobView { id: "unit 6/1/1".into(), what: String::new(), started: 0, paused: None, pausing: None, tail: String::new(), parts: Vec::new(), part: None, progress: None, mem_mb: None, threads: None }), ..Default::default() };
        let write = |s: &Status| std::fs::write(r.join("state/status.json"), serde_json::to_vec(s).unwrap()).unwrap();
        // An agent on another app: not yet; its earlier files there: not yet either.
        write(&st(None, false));
        assert!(pool::switch_on(&r, "20261010-0000-ccccccc", false).unwrap_err().to_string().contains("update it first"));
        std::fs::create_dir_all(r.join("state/build/terms")).unwrap();
        assert!(pool::switch_on(&r, "20261009-0000-bbbbbbb", false).unwrap_err().to_string().contains("moves them aside"));
        std::fs::remove_dir(r.join("state/build/terms")).unwrap();
        let said = pool::switch_on(&r, "20261009-0000-bbbbbbb", false).unwrap();
        assert!(said.contains("term 1 will be made by") && r.join(pool::ENABLED).exists(), "{said}");
        assert!(pool::switch_on(&r, "20261009-0000-bbbbbbb", false).is_err(), "on already");
        // On, in the pool: off waits for the lead to be caught up and the job to end.
        std::fs::create_dir_all(r.join("state/build/terms")).unwrap();
        std::fs::write(r.join("state/build/terms/1.json"), "{}").unwrap();
        let lead = PoolView { member: "m-000000000000000a".into(), role: pool::Role::Lead, gates: pool::Gates { term: 1, leads: Some(1), ..Default::default() }, members: Vec::new(), unacked: 0, restart: None, lead: None, outside: Vec::new(), outside_n: 0 };
        write(&st(Some(lead.clone()), true));
        assert!(pool::status(&r).contains("the pool: on"));
        let e = pool::switch_off(&r, false).unwrap_err().to_string();
        assert!(e.contains("runs unit 6/1/1") && e.contains("don't reflect the journal"), "{e}");
        write(&st(Some(PoolView { gates: pool::Gates { caught_up: true, ..lead.gates.clone() }, ..lead.clone() }), false));
        let said = pool::switch_off(&r, false).unwrap();
        assert!(!r.join(pool::ENABLED).exists() && said.contains("run `scenic pool off` again"), "{said}");
        assert!(r.join("state/build/terms/1.json").exists(), "not moved while an agent is in the pool");
        // Restarted as before: its files moved aside.
        write(&st(None, false));
        let said = pool::switch_off(&r, false).unwrap();
        assert!(said.contains("moved aside") && !r.join("state/build/terms").exists(), "{said}");
        assert!(std::fs::read_dir(r.join("state/pool-off")).unwrap().next().is_some());
        assert!(pool::switch_on(&r, "20261009-0000-bbbbbbb", false).is_ok(), "on again, afresh");
    }

    fn agent_dry(root: &Path, home: &Path) -> Agent {
        test_agent(Options { root: Some(root.to_path_buf()), home: home.to_path_buf(), bin: PathBuf::from("/nonexistent/bin"), dry_run: true, once: true, helper: false }).unwrap()
    }

    #[test]
    fn shadowed_the_agent_runs_the_pool_beside_and_writes_only_its_shadow() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::SHADOW);
        let home = d.path().join("home");
        let mut a = test_agent(Options { root: Some(r.clone()), home: home.clone(), bin: PathBuf::from("/nonexistent/bin"), dry_run: false, once: true, helper: false }).unwrap();
        assert_eq!(a.pool_mode, Some(pool::Mode::Shadow));
        assert!(a.pool.is_none());
        a.step().unwrap();
        assert!(a.shadow.is_some());
        // Its terms and records under the shadow's folder; the build's own as they were, the writer
        // named as before.
        assert!(r.join(pool::SHADOW_ROOT).join("state/build/terms/1.json").exists());
        assert!(!r.join("state/build/terms").exists() && !r.join("state/pool/members").exists());
        assert_eq!(std::fs::read_to_string(r.join("state/build/writer")).unwrap().trim(), cond::host_name());
        let log = std::fs::read_to_string(home.join("shadow/pool-shadow/shadow.jsonl")).unwrap();
        assert!(log.contains("took up term 1"), "{log}");
        assert!(a.pool_restart().is_none());
    }
}
