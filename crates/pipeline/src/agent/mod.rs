//! The build agent (docs/plan.md §8): `scenic agent`, a login item on the lead under the
//! launcher. It works out what needs doing, runs two jobs at once at most (the plan's first, and a
//! second beside it: `SLOTS`, `SECOND`) when their conditions hold (power, the NAS), pauses each
//! when they lapse, and writes a heartbeat the app shows.
//!
//! Each loop (every 20 s, sooner when a job ends):
//! 1. conditions: power, the NAS (mounting it when missing), the user's activity, sleep;
//! 2. the running jobs: each finished (recorded; failures retried with a growing delay), paused or
//!    resumed, restarted after sleep when it touches the NAS;
//! 3. a free slot's job: the plan's first runnable job (the OSM pass when the NAS holds a newer
//!    planet, the regions' work, the chains, the map updates; backups and cleanup once a day), and
//!    beside it the first of `SECOND`'s steps that can run with it (docs/plan.md §8, Two jobs at
//!    once);
//! 4. this Mac's caches (`room`): the owner's ask to clear them taken up (cleared between jobs once
//!    the build is done, else declined, why said), else, with no job running, a trim once the build
//!    is done; either on a thread of its own, the loop beating meanwhile and no job starting here;
//! 5. the heartbeat: `state/status.json` on the NAS, and a copy in the agent's local folder.
//!
//! Nothing depends on the lead being available: until work is done, the map serves the last
//! catalog. A job is a child process (see `jobs`) that resumes from its own completion markers, so
//! stopping it at any time loses at most its current stage.

pub mod backup;
pub mod brokers;
pub mod build;
pub mod cond;
pub mod every;
pub mod forecast;
pub mod gc;
pub mod jobs;
pub mod lead;
pub mod memguard;
pub mod placement;
pub mod pool;
pub mod publish;
pub mod recipes;
pub mod rekey;
pub mod room;
pub mod resume;
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


/// Free space the OSM pass needs on the lead to start: the filtered planet (about half the
/// planet's 95 GB) with room to spare. The planet is read from the NAS when there's no room to
/// copy it, the filtered file too once space runs short, and the pieces are cut a quarter at a
/// time, so the pass's peak stays near the filtered file's size.
pub const PASS_SPACE: u64 = 80 << 30;

/// A job's scratch folder (its `--scratch`), when it has one.
fn scratch_of(spec: &JobSpec) -> Option<PathBuf> {
    spec.cmd.iter().position(|a| a == "--scratch").and_then(|i| spec.cmd.get(i + 1)).map(PathBuf::from)
}

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
    /// nothing, but asks the lead's coordinator (crate::coord) for work that fits it, and
    /// reports to `state/helpers/<host>.json`, never the heartbeat.
    pub helper: bool,
}

/// How long a job asked to stop at its next safe point (the build pausing, the battery low) is given
/// before it's frozen where it is instead: a unit takes up to ~11 min, a map tile ~10.
const DRAIN_GRACE: Duration = Duration::from_secs(15 * 60);

/// How many jobs the lead runs at once (docs/plan.md §8, Two jobs at once): the plan's first it
/// can run, and a second beside it (`SECOND`). A member runs one, as the lead leases them.
const SLOTS: usize = 2;

/// The steps the second job takes, in its order of preference (the steps table's: crate::agent::
/// steps; by measure, any: `Agent::start_second`); those that keep the pass's Wikidata and Wikipedia answers here
/// and on the NAS (crate::answers: none starts while the agent sends them as it starts,
/// `Agent::seed_answers`).
use steps::SECOND;

// The last map update, in the agent's folder (build::MapUpdate): its jobs read the units of
// the one under way there (crate::out::UNITS_AS_OF_ENV).
use crate::out::MAP_UPDATE_FILE;
/// The last map update kept in `home` (`MAP_UPDATE_FILE`). None: it doesn't read.
fn read_map_update(home: &Path) -> Option<build::MapUpdate> {
    std::fs::read(home.join(MAP_UPDATE_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Keeps the map update in `home` (`MAP_UPDATE_FILE`).
fn write_map_update(home: &Path, r: &build::MapUpdate) -> Result<()> {
    crate::whole::write(&home.join(MAP_UPDATE_FILE), &serde_json::to_vec(r)?)
}


/// The pass's worldwide steps the checklist counts, after the planet's own.
const WORLDWIDE_STEPS: [&str; 10] = ["pass-sets", "outlines", "trailends", "reach", "terrain-z8", "buildings", "summits", "labels", "lakefill", "water"];

/// The loop's pool steps passed over in a row, a tick holding the run, before the status says the
/// pool's step hangs (about a minute of the loop's steps; or at once past `STALL_S`).
const HUNG_SKIPS: u32 = 3;
/// How long the loop tries for the run when no tick is said to hold it (the pool's thread's moment's
/// check, or a tick begun and not yet said), before passing its pool step over.
const TRY_FOR: Duration = Duration::from_millis(200);

/// Whether job `id` is one of the lead's duties (a catalog, a sweep: crate::agent::pool's
/// `PUBLISHES`, `SWEEPS`).
fn is_duty(id: &str) -> bool {
    step_of(id).is_some_and(|st| pool::PUBLISHES.contains(&st.as_str()) || pool::SWEEPS.contains(&st.as_str()))
}

/// Whether jobs of steps `a` and `b` can't run at once on one Mac by the steps alone: they're the
/// same step and it isn't a shared one (one job's work: its targets held by nothing else). A shared
/// step's targets are held apart (crate::coord), and each slot has a scratch folder of its own;
/// what else two jobs share is guarded where it's used (docs/pool.md §7.7: the caches' accessor,
/// the budgets' proxy, the answers' lease), and their memory and disk measured (`fits_beside`).
fn clash(a: &str, b: &str) -> bool {
    a == b && !steps::SHARED.contains(&a)
}

/// The cost key a job of no targets of its own is measured by (the OSM pass's `osm-pass <date>`, GC's
/// `gc gc`): its id's step and the rest of its id, or its id again.
fn own_cost_key(id: &str) -> String {
    let (s, t) = id.split_once(' ').unwrap_or((id, id));
    crate::coord::cost_key(s, t)
}

/// The lead's second job as a worker (the build's history, the forecast's machines, the worker
/// page): the Mac's name, marked.
pub fn second_worker(host: &str) -> String {
    format!("{host} (second job)")
}

/// A job slot's running job and what goes with it: its lease (the coordinator's, or a helper's
/// granted one), when it last beat, when it was asked to stop at its
/// next safe point, and its time left as the status last worked it out.
#[derive(Default)]
struct Slot {
    running: Option<Running>,
    lease: Option<Held>,
    beaten: Option<Instant>,
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
    /// Its own job's timings, taken as it ended, for the lead when this Mac no longer leads (the
    /// job's hand-off carries them: `end_lease`).
    timings_out: Option<crate::timings::RunRec>,
    /// Its job started beside one that waited on a worker (`waiting_on`), that job's id: while that
    /// job runs (`yielding`), it's the one that yields
    /// to the memory guard when the two then hold too much together (memguard::decide).
    yields: Option<String>,
    /// Its job is being stopped for yielding (`yield_stop`): given back, nothing learned of it.
    yield_stopped: bool,
}

/// How long the first slot may pass over a job that can't share the Mac with the second's (it then
/// waits for it, and the second starts nothing new until it has).

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

/// How a pooled job's end is told to a lead over HTTP (`end_lease`): done (its hand-off in the
/// journal), given back, or failed, with what its targets took at least.
#[derive(Clone, Debug)]
enum Tell {
    Done(Box<crate::coord::Done>),
    GiveBack { id: u64, why: String, floors: Vec<(String, memguard::Floor)>, disks: Vec<(String, u64)>, job_disk: u64 },
    Fail { id: u64, why: String, floors: Vec<(String, memguard::Floor)>, disks: Vec<(String, u64)>, job_disk: u64 },
}

impl Tell {
    /// Tells lead `c`: true once it answered (taken, or the lease gone); said when it refused it or
    /// didn't answer.
    fn to(&self, c: &crate::coord::client::Client) -> bool {
        let (id, r) = match self {
            Tell::Done(d) => (d.lease, c.done(d).map(|h| if let crate::coord::client::Handed::Refused(why) = h { eprintln!("agent: the lead refused lease {}'s end: {why} (it lapses)", d.lease) })),
            Tell::GiveBack { id, why, floors, disks, job_disk } => (*id, c.end_with(*id, why, true, floors, disks, *job_disk)),
            Tell::Fail { id, why, floors, disks, job_disk } => (*id, c.end_with(*id, why, false, floors, disks, *job_disk)),
        };
        match r {
            Ok(()) => true,
            Err(e) => {
                eprintln!("agent: telling the lead lease {id} ended: {e:#}");
                false
            }
        }
    }
}

/// How long an end owed to the lead is tried (s): as long as its lease lasts unrenewed.
const OWED_S: u64 = crate::coord::TTL.as_secs();

/// The free space a member's jobs start with (its Mac has less room than the lead), but for
/// the steps that need more (`helper_need`).
const HELPER_RESERVE: u64 = 15 << 30;

/// The free space a member's job of `step` needs: a terrain run's as on the lead (its area's
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
/// tiles took 0.3 GB and 160 B a row read for `bldprep`; `bldtiles`, `bldtiles_peak`.
fn bld_peak(step: &str, rows: u64) -> u64 {
    match step {
        "bldprep" => 300 + rows * 160 / (1 << 20),
        _ => bldtiles_peak(rows),
    }
}

/// `bldtiles`' (B5's pilot with the field and its centroids' offsets, 2026-10-15, on the
/// M4's 14 threads): 1.8 GB and 1,100 B a record of its largest z8 area (taken as two fifths of
/// the rows), the field's cells in flight the fixed part. Measured 1.3–2.9 GB for tiles of up to
/// 52 k records, 3.8 GB for Toronto's 9.5 M, 4.5 GB for Annecy's 10.0 M, 7.9 GB for New York's
/// 14.6 M and 10.2 GB for Kantō's 30.3 M.
pub const fn bldtiles_peak(rows: u64) -> u64 {
    1800 + rows * 2 / 5 * 1100 / (1 << 20)
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
/// five eighths. `total_mb`: its Mac's memory (`Agent::mac_total_mb`: a test's Mac as it sets it,
/// never the machine running the test); 0, unknown: 6 GB.
fn helper_memory(total_mb: u64, away: bool) -> u64 {
    match total_mb {
        0 => 6144,
        t => t * if away { 5 } else { 3 } / 8,
    }
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
    /// The lead's own job's, in its coordinator.
    Own(u64),
    /// A member's job's, from the lead's coordinator: the job saves into `dir` (its outbox),
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
    /// The lead's second job, beside the first (docs/plan.md §8, Two jobs at once), and why
    /// there's none when its slot is free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside: Option<JobView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside_why: Option<String>,
    /// SCENIC_TEST_DISK_EXTRA_MB when set (MB counted free past what the disk has): a test's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_disk_extra_mb: Option<u64>,
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
    pub checklist: Vec<build::Line>,
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
    /// (the lead's: crate::agent::forecast).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forecast: Option<forecast::Forecast>,
    /// The decision on the next map update as the lead last planned it, and why
    /// (build::UpdateDecision; docs/plan.md §8, Faster publishing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_update: Option<build::UpdateDecision>,
    /// The timetables' to-do list's counts and the routes matching nothing on the map
    /// (crate::timetables, docs/inputs.md §6.7.3): the lead's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timetables: Option<crate::timetables::View>,
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
    /// The budgets, while they're on (crate::budget, docs/pool.md §7.7): the proxy's address, what
    /// it has opened, holds told, the NAS out of reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets: Option<crate::budget::View>,
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
    /// The members heard from lately, itself among them (`Side::heard_lately`): what the budgets
    /// divide by.
    #[serde(default)]
    pub heard: usize,
    #[serde(default)]
    pub unacked: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    /// The pool as the controls show it (crate::agent::lead: docs/pool.md §10, §11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<lead::View>,
}

impl PoolView {
    /// The members heard from lately as view `p` last said (none, or an older app's without the
    /// count: this one alone), for the budgets while the pool's run is busy.
    pub fn heard_of(p: Option<&PoolView>) -> usize {
        p.map_or(1, |p| p.heard.max(1))
    }
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
    /// What it waits on, while it waits on a worker holding its unit's tail and nothing else (its
    /// slot's room then counted as free beside it: docs/pool.md §7.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
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
    /// When a catalog last started (seconds since the epoch): the next map update waits an
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
    /// A member's broker's port, beside its lead's coordinator in the same process (`broker_port`).
    static TEST_BROKER_PORT: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
    /// Whether this Mac can lead, whatever its conditions say (the pool's tests).
    static TEST_ABLE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// What `find_root` finds in a test: nothing unless a test sets it (never the real NAS).
    static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Whether `p` is the real NAS's or the app's real home: a test never writes there.
pub fn is_live_path(p: &Path) -> bool {
    p.starts_with("/Volumes") || std::env::var_os("HOME").is_some_and(|h| p.starts_with(Path::new(&h).join("Library/Application Support/scenic")))
}

/// The NAS project folder: the share's mount, by whatever name it's mounted. When it's missing and
/// `mount` is set, it's mounted: at home by the LAN name, away through Tailscale when the Keychain
/// has the bare name's password (else nothing: a dialog would ask for it).
pub fn find_root(mount: bool) -> Option<PathBuf> {
    // (A test never finds the mounted NAS: only a root a test set, never one on the NAS.)
    #[cfg(test)]
    {
        let _ = mount;
        let r = TEST_ROOT.with(|t| t.borrow().clone());
        assert!(r.as_deref().is_none_or(|r| !is_live_path(r)), "a test's root is the real NAS: {r:?}");
        r
    }
    #[cfg(not(test))]
    find_live_root(mount)
}

#[cfg(not(test))]
fn find_live_root(mount: bool) -> Option<PathBuf> {
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
pub struct AgentLock(#[allow(dead_code)] crate::sys::PathLock);

impl AgentLock {
    /// None when another agent holds it.
    pub fn try_take(home: &Path) -> Result<Option<AgentLock>> {
        std::fs::create_dir_all(home)?;
        Ok(crate::sys::PathLock::take(&home.join("agent.lock"), false)?.map(AgentLock))
    }
}

/// The coverage of the regions, and each region's own.
struct Coverages {
    all: crate::coverage::Coverage,
    each: Vec<(String, crate::coverage::Coverage)>,
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
    /// The jobs running: the first slot's, and on the lead a second's beside it (`SLOTS`).
    slots: [Slot; SLOTS],
    /// Why the second slot has nothing, when it hasn't, for the status.
    beside_why: Option<String>,
    /// The disk's free bytes as a test sets them (`disk_free`).
    free_set: Option<u64>,
    /// How long a job draining is given to reach a safe point (`DRAIN_GRACE`; a test's own, so no
    /// test dates an `Instant` back past the Mac's uptime).
    grace: Duration,
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
    /// The pool's switch for a part changing in this process (`pool::SLOTS`), as last read; the
    /// idle-sleep assertion it holds while it leads with leases out; the app it last handed the lead
    /// on for (a newer one installed, its update waiting on its own job: `hand_for_update`).
    slots_on: bool,
    /// The budgets (docs/pool.md §7.7): every job's connections out through this agent's proxy,
    /// each Wikimedia request taken from the pool's one budget on the NAS, the job sandboxed so it
    /// has no other way out; the answers' lease. (And the Mac shared by measure: a job starts beside
    /// another only if both jobs' measured memory and disk fit what the Mac has left, a cold start
    /// alone; the disk guard on.)
    proxy: Option<crate::budget::Proxy>,
    /// Resuming, and planning from the records alone (docs/pool.md §7.2, §7.4; crate::agent::resume,
    /// `state/pool/resume`), in the pool: a resumable job's progress named on the NAS and offered to
    /// its Mac alone, its job given this member's unmerged hand-offs, a pass complete by the records.
    resume: bool,
    /// Placement by how crucial a job is, and work offered ahead (crate::agent::placement,
    /// `state/pool/placement`), as the switch was last read: it acts only in the pool
    /// (`placement_on`).
    placement: bool,
    /// Tasks brokered by every member (crate::agent::brokers, `state/pool/brokers`), as the switch
    /// was last read: it acts only in the pool (`brokers_on`).
    brokers: bool,
    /// A member's broker: its coordinator for its own jobs' tasks alone (the lead's are its
    /// coordinator's, `coord`), and when one last failed to start.
    broker: Option<std::sync::Arc<crate::coord::Coordinator>>,
    broker_failed: Option<Instant>,
    /// Its address over HTTPS last looked for, and what it was.
    broker_https: Option<(Instant, Option<String>)>,
    /// This member's progress file as it last wrote it (or read it, as the process began).
    held_progress: Option<resume::Progress>,
    /// The disk guard's room-making for the jobs it froze (when it began): its result decides
    /// whether they go on or one is given back.
    disk_room: Option<Instant>,
    /// Whether the NAS's folder answers now (`answers`: a stat on a thread, eight seconds at most); a
    /// test's own, deciding it without the clock or the Mac's load (`test_agent`).
    probe: fn(&Path) -> bool,
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
    progress: Option<(Instant, BTreeMap<String, build::RegionState>, Vec<build::Line>)>,
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
    /// Who this agent is ("<host> <pid>").
    me: String,
    /// The OSM pieces' sizes by content name (the coordinator sizes units by them; content-named
    /// files never change).
    piece_sizes: std::cell::RefCell<std::collections::HashMap<String, u64>>,
    /// What the last plan found waiting and the regions it read, and when: while
    /// a job runs nothing new can start, so the plan (the manifest, the keys, a dozen NAS listings)
    /// is made again only every five minutes, for the status, and when the job ends.
    planned: Option<Planned>,
    /// The work the last plan had left (its steps and targets): what room-making lets go last
    /// (`hints`).
    queued: std::cell::RefCell<Vec<build::Work>>,
    /// The lead's coordinator (crate::coord), and when its contact on the NAS was last checked.
    coord: Option<std::sync::Arc<crate::coord::Coordinator>>,
    published: Option<Instant>,
    /// A member's way to the lead's coordinator (made once its contact is on the NAS).
    client: Option<crate::coord::client::Client>,
    /// Its own jobs' ends owed to the lead: ended while this process led a term it had lost without
    /// its step knowing yet (it froze, a later term made meanwhile), told to the lead through its
    /// client once its part has changed (`end_lease`, `tell_owed`); and a member's ends the lead
    /// didn't take when they ended (it didn't answer). Each told each loop until `OWED_S` after it
    /// ended (its lease has lapsed by then).
    owed: Vec<(u64, Tell)>,
    /// A helper's: the bytes its cheap caches held (room::cheap_bytes) and when they were counted.
    cheap: Option<(Instant, u64)>,
    /// The last catalog's folder and number, and its regions' outline entries by id (`on_map`).
    last_catalog: std::cell::RefCell<Option<((PathBuf, u64), BTreeMap<String, Vec<String>>)>>,
    /// The regions the last plan would publish as built (build::Plan::ready), for the checklist.
    ready: std::cell::RefCell<Vec<String>>,
    /// The build's pause as this agent knows it (crate::control): its coordinator's on the
    /// lead; a member's as heard in its asks and beats, or as asked for here and not yet passed on
    /// (`pause_local`). Kept in `pause.json`.
    pause: Option<crate::control::Pause>,
    pause_local: bool,
    /// The lead's pause as last mirrored to the NAS, and whether this agent's own (kept from
    /// before its coordinator was up) has been given to the coordinator.
    mirrored: Option<Option<crate::control::Pause>>,
    pause_pushed: bool,
    /// The bytes the caches hold, for the status: what room-making can free and what a clear would
    /// (room::sizes), counted on a thread of its own every ten minutes (a walk of many files), and
    /// again after a trim or a clear; and when.
    cache_size: std::sync::Arc<std::sync::Mutex<CacheCount>>,
    /// A member's view of the build: the lead's heartbeat as last read, at most each minute.
    heard: Option<Heard>,
    /// A trim or a clear under way, and when one last failed (it's tried again ten minutes later).
    caches_task: Option<CachesTask>,
    trim_failed: Option<Instant>,
    /// The lead's: the pass's answers this Mac has sent to the NAS as the agent starts
    /// (crate::answers::seed), on a thread of its own; and whether it was started (once an agent).
    answers_seed: Option<std::thread::JoinHandle<Result<Vec<String>>>>,
    answers_seeded: bool,
    /// The lead's: the members' last trim, clear or declined ask noted in the history (its
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
    /// The lead's decision on the next map update as last planned (build::UpdateDecision), and the key
    /// of the last one logged (logged again only when it changes).
    update_decision: std::cell::RefCell<Option<build::UpdateDecision>>,
    /// The timetables' report as last read (its content name, and what the status shows).
    timetables: std::cell::RefCell<Option<(String, crate::timetables::View)>>,
    update_logged: std::cell::RefCell<String>,
    catalog_seen: std::cell::Cell<Option<CatalogSeen>>,
    /// The last map update (build::MapUpdate, its folder's `map-update.json`): the one under way,
    /// which its jobs read the units of, or the last one over, for when it began.
    map_update: std::cell::RefCell<Option<build::MapUpdate>>,
    /// Faster publishing's switches as last read (crate::agent::publish), and the map updates' measured
    /// time (forecast::update_secs, as the last forecast read the history): a map update per region's gap.
    publishing: std::cell::Cell<publish::Switches>,
    update_measured: std::cell::Cell<Option<f64>>,
    /// The pool (docs/pool.md, crate::agent::pool), always on: this Mac's part in it (None while this
    /// process isn't in it: the NAS not reached as it started, or its member's lock held by another
    /// process; it starts nothing then, and restarts into the pool once it can), and why this
    /// process restarts once its first job's slot is free.
    /// Shared with the pool's own thread (`ticker`, started with the loop: `pool::Ticker`), which
    /// steps it while the loop is busy; each takes it under its lock.
    pool: Option<pool::Shared>,
    ticker: Option<pool::Ticker>,
    /// This member's id, and its run as the loop's last pool step left it (`pool::Run::pool_view`):
    /// what the loop reads of the pool outside its pool step, never waiting on the run's lock (a
    /// tick holds it across the share's operations, which may hang).
    member_id: Option<String>,
    pool_seen: Option<PoolView>,
    /// Its jobs' hand-offs that ended while a tick held the run, for the next pool step; and the
    /// switch's part change likewise (`pool::SLOTS`).
    held_entries: Vec<(crate::pool::journal::Entry, Option<PathBuf>)>,
    in_process_owed: Option<bool>,
    /// Whether a tick holds the run, and since when (`pool::Ticking`, read without its lock); the
    /// loop's pool steps passed over in a row for it, and since when it's taken for hung
    /// (`pool_skipped`).
    ticking: Option<pool::Ticking>,
    /// The coordinator's contact's publications, counted (an unpublish begun before the last one
    /// doesn't take it off), and the unpublish on a thread of its own still under way, if one is.
    publish_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    unpublishing: Option<std::thread::JoinHandle<()>>,
    pool_skips: u32,
    pool_hung: Option<u64>,
    restart_for: Option<String>,
    /// The gate (crate::inputs): the drop boxes' listing thread, the status's entries as the last
    /// plan made them, what they read, and the units a full check was asked of.
    inputs: Gate,
    /// The last start held by something on a short timer (the pool's step under way, the answers
    /// going to the NAS): the idle agent plans again next loop.
    start_soon: std::cell::Cell<bool>,
}

/// The agent's part of the gate (docs/inputs.md §4.2, §4.7).
#[derive(Default)]
struct Gate {
    /// The drop boxes listed off the loop (the lead's), with the root it
    /// lists.
    watch: Option<(PathBuf, crate::inputs::watch::Watch)>,
    view: std::cell::RefCell<Vec<crate::inputs::view::InputView>>,
    cache: std::cell::RefCell<crate::inputs::view::Cache>,
    full: std::cell::RefCell<BTreeSet<String>>,
    /// The descriptions' credits' digest as last read (None inside: there are none), for the
    /// catalog's key while they can't be read.
    credits: std::cell::RefCell<Option<Option<String>>>,
    /// Each unit's last full check's time, by its check's state's content name (crate::inputs::
    /// Listed: content-named, so read once).
    full_at: std::cell::RefCell<std::collections::HashMap<String, Option<u64>>>,
    /// Units off the gate whose records were asked to be forgotten.
    forget: std::cell::RefCell<BTreeSet<String>>,
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
    /// Room made for a job the disk guard froze (`disk_guard`).
    disk: bool,
    began: Instant,
    thread: std::thread::JoinHandle<Result<room::Freed>>,
}

/// The lead's heartbeat as a member last read it (`Agent::heard`).
struct Heard {
    /// When it was read, and when the lead beat (None: it couldn't be read).
    read: Instant,
    beat: Option<u64>,
    /// Why the build isn't done (`work_left`; None: it is).
    left: Option<String>,
}

/// The last plan's view, kept for the heartbeat between plans.
struct Planned {
    at: Instant,
    /// What the plan depended on when made (`plan_seen`); whether a start was held by something on
    /// a short timer (`Agent::start_soon`); when its timed waits end (`Agent::replan_at`).
    seen: u64,
    soon: bool,
    replan_at: Option<u64>,
    waiting: Vec<Waiting>,
    regions: Vec<recipes::Recipe>,
    bad: Vec<(String, String)>,
}

impl Agent {
    pub fn new(o: Options) -> Result<Agent> {
        Agent::new_with(o, answers)
    }

    /// `new`, with how it asks whether the NAS's folder answers (`probe`: the real one, `answers`;
    /// a test's).
    fn new_with(mut o: Options, probe: fn(&Path) -> bool) -> Result<Agent> {
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
        // The pool, as this process starts (the NAS answering; else it starts nothing until it does,
        // then restarts into it): this Mac's member takes its first step, which says its part: it
        // leads or works as a member.
        let mut run: Option<pool::Run> = None;
        let mut first: Option<(crate::pool::driver::Out, PathBuf)> = None;
        if lock.is_some() && !o.dry_run {
            if let Some(r) = o.root.clone().or_else(|| find_root(false)).filter(|r| probe(r)) {
                {
                    let nas: pool::SharedNas = std::sync::Arc::new(crate::pool::nas::Share::new(&r));
                    let locks = o.home.parent().unwrap_or(&o.home).to_path_buf();
                    match pool::Side::open(&o.home, &o.home.join("pool"), &locks, &app, nas)? {
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
        // The lead's agent coordinates (not a member's, nor a dry run's, nor a process not in the
        // pool), with a root of its own too (a test's, on a port of its own, SCENIC_COORD_PORT).
        let pooled = run.is_some();
        let coord = if !o.helper && lock.is_some() && !o.dry_run && pooled { make_coordinator(&o, run.as_ref(), &app, &cond::host_name()).map(std::sync::Arc::new) } else { None };
        if let (Some(r), Some((out, root))) = (run.as_mut(), &first) {
            pool::took_up(r, coord.as_deref(), &out.events, out.leads, &cond::host_name(), &[]);
            // (The terms' events a member's process before this one kept, for this coordinator's
            // history; and this first step's.)
            if let Some(c) = &coord {
                lead::replay(&o.home.join("pool"), c);
            }
            lead::after(r, out, root, &o.home, coord.as_deref());
        }
        // The build's pause as this agent last knew it (a member that can't reach the lead stays
        // as it was).
        let pause: Option<crate::control::Pause> = std::fs::read(o.home.join("pause.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        // (A map update's file that doesn't read: none under way, the next begins afresh.)
        let map_update = read_map_update(&o.home);
        let tiles = std::cell::RefCell::new(tiles::TerrainTiles::new(Some(o.home.join("pack-idx"))));
        Ok(Agent { host: cond::host_name(), app, started: now_s(), mem, slots: Default::default(), beside_why: None, free_set: None, grace: DRAIN_GRACE, room_target: None, toward_tried: None, floor_short: None, floor_held: None, toward_goal: Default::default(), mem_set: None, guard_on: None, guard_hold: None, guard_last: None, sampler: memguard::Sampler::new(SLOTS), total: 0, guard_backoff: BTreeMap::new(), slots_on: false, proxy: None, resume: false, placement: false, brokers: false, broker: None, broker_failed: None, broker_https: None, held_progress: None, disk_room: None, probe, lead_awake: None, last_root: None, update_handed: None, mac: cond::Mac::Real, sleep: SleepWatch::default(), last_mount_try: None, last_beat: None, progress: None, reach: Default::default(), tiles, coverage: Default::default(), edits: Default::default(), edited_at: Default::default(), _lock: lock, o, me, piece_sizes: Default::default(), planned: None, queued: Default::default(), coord, published: None, client: None, owed: Vec::new(), cheap: None, last_catalog: Default::default(), ready: Default::default(), pause, pause_local: false, mirrored: None, pause_pushed: false, cache_size: Default::default(), heard: None, caches_task: None, trim_failed: None, answers_seed: None, answers_seeded: false, helper_caches: BTreeMap::new(), orphans: Vec::new(), last_cond: None, forecast: Default::default(), update_decision: Default::default(), timetables: Default::default(), update_logged: Default::default(), catalog_seen: Default::default(), map_update: std::cell::RefCell::new(map_update), publishing: Default::default(), update_measured: Default::default(), member_id: run.as_ref().map(|r| r.side.member().id.clone()), ticking: run.as_ref().map(|r| r.ticking.clone()), publish_gen: Default::default(), unpublishing: None, pool_skips: 0, pool_hung: None, pool_seen: run.as_ref().map(|r| r.pool_view()), held_entries: Vec::new(), in_process_owed: None, pool: run.map(|r| std::sync::Arc::new(std::sync::Mutex::new(r))), ticker: None, restart_for: None, inputs: Gate::default(), start_soon: Default::default() })
    }

    /// The keys to plan with: on the NAS, with the done records of the hand-offs waiting to be merged
    /// on top (a worker's job built and handed back, not yet merged: not built again).
    fn planning_keys(&self, root: &Path) -> Result<build::Keys> {
        build::Keys::load_with(root, &self.handoff_bases(root))
    }

    /// Where hand-offs wait to be drained into the journal: the coordinator's journal on this Mac.
    fn handoff_bases(&self, _root: &Path) -> Vec<PathBuf> {
        vec![self.o.home.join("coord/journal")]
    }

    /// Where a helper's leased jobs save (`<outbox>/<lease>/`), and its agent puts their result, until
    /// the coordinator has them.
    fn outbox(&self) -> PathBuf {
        self.o.home.join("outbox")
    }

    /// A member's way to the lead's coordinator; None (with why, in `waiting`) when there's none.
    fn client(&mut self, root: &Path, waiting: &mut Vec<Waiting>) -> Option<&crate::coord::client::Client> {
        if self.client.is_none() {
            match crate::coord::client::Client::from_nas(root, &self.host) {
                Ok(Some(c)) => {
                    // (In rounds, one a loop: `step`.)
                    c.new_round();
                    self.client = Some(c);
                }
                Ok(None) => {
                    // (In the pool, the lead is known: its contact not written yet, not "isn't running".)
                    let lead = self.pool_seen.as_ref().and_then(|p| p.lead.as_ref()).and_then(|v| v.lead.clone());
                    let why = match lead {
                        Some(l) => format!("{} leads term {}, but its coordinator's address isn't on the NAS yet (it's taking up the term, or its agent stopped)", l.host, l.term),
                        None => "the lead isn't coordinating (its agent isn't running, or is on an older app)".into(),
                    };
                    waiting.push(Waiting { step: None, what: "Building".into(), why });
                }
                Err(e) => waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e:#}") }),
            }
        }
        self.client.as_ref()
    }

    /// Why a request to the lead's coordinator failed, in words (docs/pool.md §6.8): its cause (timed
    /// out, refused) and what's likely behind it, naming the lead when the pool says who it is.
    fn unreached_why(&self, e: &anyhow::Error) -> String {
        let text = format!("{e:#}");
        let lead = self.pool_seen.as_ref().and_then(|p| p.lead.as_ref()).and_then(|v| v.lead.clone());
        let host = lead.as_ref().map_or("the lead".to_string(), |l| l.host.clone());
        let cause = crate::pool::reachable::cause_of(&text);
        let r = crate::pool::reachable::Reach { member: String::new(), what: "coordinator".into(), ok: false, at: 0, since: 0, cause: Some(cause), why: text.clone() };
        match cause {
            crate::pool::reachable::Cause::Other => format!("{host}'s coordinator can't be reached: {text}"),
            _ => format!("{host}'s coordinator can't be reached from this Mac: {} ({text})", crate::pool::reachable::words(&r, &host)),
        }
    }

    /// Hands this member's finished tasks back (a task leased from the lead's coordinator, run in
    /// its outbox folder: crate::offload): its outputs and what it took, or its failure. A folder
    /// stays until the coordinator has it (or says its lease is gone: then its work was offered
    /// again, and it's dropped), across restarts; one left by a task that died with an earlier agent
    /// is given back. (A job's work goes through the journal: crate::agent::pool.)
    fn send_outbox(&mut self, root: &Path, waiting: &mut Vec<Waiting>) {
        let Ok(rd) = std::fs::read_dir(self.outbox()) else { return };
        let running = match &self.slots[0].lease {
            Some(Held::Leased { dir, .. }) => Some(dir.clone()),
            _ => None,
        };
        // (A job's folder from before the pool, drained into the journal: tasks' alone here.)
        let dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && Some(p) != running.as_ref() && !p.join("work.json").exists()).collect();
        if dirs.is_empty() {
            return;
        }
        let Some(client) = self.client(root, waiting) else { return };
        for d in dirs {
            let Some(lease) = d.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok()) else { continue };
            let result: Option<serde_json::Value> = std::fs::read(d.join("result.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
            let sent = (|| -> Result<crate::coord::client::Handed> {
                // A task's: its outputs and what it took.
                if let Some(t) = result.as_ref().filter(|r| r["ok"].as_bool() == Some(true)).map(|r| &r["task"]).filter(|t| t.is_object()) {
                    let d = crate::coord::Done { lease, outputs: serde_json::from_value(t["outputs"].clone())?, removed: serde_json::from_value(t["removed"].clone())?, secs: t["secs"].as_f64().unwrap_or(0.0), peak_mb: t["peak_mb"].as_u64().unwrap_or(0), timings: read_timings(&d), ..Default::default() };
                    return client.done(&d);
                }
                let why = match &result {
                    Some(r) if r["ok"].as_bool() == Some(true) => "it ended without a result".to_string(),
                    Some(r) => r["error"].as_str().unwrap_or("it failed").to_string(),
                    None => "this Mac's agent stopped while it ran".to_string(),
                };
                // (Stopped, not failed: the build paused, the Mac slept, its agent stopped: given
                // back, not held against its targets.)
                let interrupted = result.as_ref().is_none_or(|r| r["interrupted"].as_bool() == Some(true));
                if interrupted {
                    client.give_back(lease, &why).map(|()| crate::coord::client::Handed::Taken)
                } else {
                    client.fail(lease, &why, None).map(|()| crate::coord::client::Handed::Taken)
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
    /// and programs are): the lead's own jobs' and a member's alike.
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

    /// A member's next job: work of the plan the lead's coordinator leases it (a job's worth of
    /// a shared step's targets, none needing more memory than it spares), built from the pass it
    /// says, saving into the lease's outbox folder. Asked only when it could start now.
    fn helper_job(&mut self, root: &Path, c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let needs = Needs { nas: true };
        if let Some(why) = lapsed(&needs, c) {
            waiting.push(Waiting { step: None, what: "Building".into(), why });
            return Vec::new();
        }
        // Not while this Mac's own pause holds (its ask not yet with the lead: the start loop
        // says so), nor while a newer app waits to start: nothing would start. (While the lead
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
        let mut can = helper_steps(room::disk_free(&self.o.home).unwrap_or(0), cheap, self.floor());
        // (Every step to every member: every step the lead may offer, its room by the measures at
        // the lead's coordinator, this Mac's disk room said with the ask; crate::agent::every.)
        let every = self.every_on();
        if every {
            for s in steps::TABLE.iter().map(|r| r.name).filter(|s| every::offered(s)) {
                if !can.iter().any(|c| c == s) {
                    can.push(s.to_string());
                }
            }
        }
        // (Resuming on: a resumable job whose progress this Mac's file holds, asked for too; the
        // lead offers it to this Mac alone. Its scratch is here already: no room asked for it.)
        if self.resume_on() {
            if let Some(p) = self.own_progress(root) {
                let now = self.resume_now();
                for job in resume::asks(&p, now, &|j, at| resume::released(root, &p.member, j, at)) {
                    if let Some(step) = job.split(' ').next().filter(|s| !can.iter().any(|c| c == s)) {
                        can.push(step.to_string());
                    }
                }
            }
        }
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
            mem_mb: helper_memory(self.mac_total_mb(), false),
            cores: std::thread::available_parallelism().map_or(4, |n| n.get() as u32),
            max: batch_size("unit"),
            app: Some(self.app.clone()),
            more_mb: away.then(|| helper_memory(self.mac_total_mb(), true)),
            max_secs: away.then_some(AWAY_JOB),
            limit_mb: Some(memguard::limit_mb(self.total_mb())).filter(|&l| l > 0),
            // (Its free space now, less the reserve and the owner's target, as the lead reckons its
            // own: `own_room`. What its caches could free isn't counted: nothing frees it first.)
            disk_mb: every.then(|| self.disk_free().saturating_sub(room::RESERVE + self.floor()) >> 20),
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
            Ok(Some(crate::coord::Grant { lease, term, work: crate::coord::Granted::Job { step, targets, pass }, as_of, .. })) if steps::SHARED.contains(&step.as_str()) || (every && every::offered(&step)) || (self.resume_on() && resume::RESUMABLE.contains(&step.as_str())) => {
                // (A resumable job, its progress on this Mac: resumed in its own scratch folder,
                // its step's whole hand-off as the lead's own job's, no done record: its target,
                // the pass's date, is no step's target. crate::agent::resume.)
                let resumed = resume::RESUMABLE.contains(&step.as_str()) && !every::offered(&step);
                // (In the pool, its folder is its lease's, `<term>-<lease>`, handed to the journal.)
                let pooled = self.pool.is_some().then_some(crate::pool::journal::LeaseId { term, n: lease });
                let dir = match pooled {
                    Some(l) => pool::job_dir(&self.o.home, l),
                    None => self.outbox().join(lease.to_string()),
                };
                // (Its step and targets, kept with its saves: should this agent stop, the next hands
                // off what of them it finished.)
                let work = match pooled {
                    Some(l) => serde_json::to_vec(&pool::JobKept { step: step.clone(), targets: if resumed { Vec::new() } else { targets.clone() }, lease: l }),
                    None => serde_json::to_vec(&build::Work { step: step.clone(), targets: targets.clone() }),
                };
                let kept = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("work.json"), work.unwrap_or_default()));
                if let Err(e) = kept {
                    waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e}") });
                    fail(self, lease, &format!("its outbox: {e}"));
                    return Vec::new();
                }
                let s = |p: &Path| p.to_string_lossy().into_owned();
                // The lead's own command for it, its saves handed off (SCENIC_HANDOFF).
                let mut cmd = vec!["/usr/bin/env".to_string(), format!("SCENIC_HANDOFF={}", dir.display()), format!("SCENIC_COSTS={}", dir.join("costs.jsonl").display()), format!("{}={}", crate::timings::TIMINGS_ENV, dir.join(TIMINGS_FILE).display())];
                if resumed {
                    // (Its target the pass's date, its key the planet's path in the NAS's folder.)
                    let (date, planet) = targets.first().cloned().unwrap_or_default();
                    cmd.extend(self.osm_pass_cmd(root, &root.join(&planet), &date));
                    let lease_kept = match pooled {
                        Some(l) => Held::Pooled { id: lease, term: l.term, dir, own: false },
                        None => Held::Leased { lease, dir },
                    };
                    self.slots[0].lease = Some(lease_kept);
                    let what = format!("Reading the OpenStreetMap planet of {date} (resumed here, its progress on this Mac)");
                    return vec![JobSpec { id: format!("{step} {date}"), what, cmd, needs, restart_after_sleep: true, record: None }];
                }
                // (A map update's step: the map update's units, as the lead granted them, read from its folder
                // as the lead's own job reads its map update's file.)
                if let Some(a) = as_of {
                    let f = dir.join(MAP_UPDATE_FILE);
                    let wrote = serde_json::to_vec(&serde_json::json!({ "began": a.began, "units": a.units })).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&f, &b));
                    if let Err(e) = wrote {
                        waiting.push(Waiting { step: None, what: "Building".into(), why: format!("the map update's units: {e:#}") });
                        fail(self, lease, &format!("the map update's units: {e:#}"));
                        return Vec::new();
                    }
                    cmd.push(format!("{}={}#{}", crate::out::UNITS_AS_OF_ENV, f.display(), a.began));
                }
                cmd.extend([s(&self.o.bin.join("scenic-build")), step.clone(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch").join(&step))]);
                cmd.extend(targets.iter().map(|t| t.0.clone()).filter(|t| every::names_target(t) && *t != step));
                cmd.extend(self.step_args(&step, &pass));
                // (Pieces made again as they are, by the build's records as read: expected the
                // same. Records that can't be read now expect nothing.)
                if matches!(step.as_str(), "trees" | "terrain" | "slope") {
                    let same = build::Keys::load_with(root, &[]).map(|k| expect_same(&build::Work { step: step.clone(), targets: targets.clone() }, &self.as_read(root, k))).unwrap_or_default();
                    if !same.is_empty() {
                        cmd.extend(["--expect-same".to_string(), same.join(",")]);
                    }
                }
                let n = targets.len();
                let id = format!("{step} {}", targets.first().map(|t| t.0.as_str()).unwrap_or(""));
                let what = format!("{}, for the lead", steps::said(&step, n, None));
                self.slots[0].lease = Some(match pooled {
                    Some(l) => Held::Pooled { id: lease, term: l.term, dir, own: false },
                    None => Held::Leased { lease, dir },
                });
                vec![JobSpec { id, what, cmd, needs, restart_after_sleep: true, record: Some(build::Work { step, targets }) }]
            }
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Task { id, task, .. }, .. })) => {
                // A task (a unit's tail, or a 3D buildings' z8 area, for the lead's job), run here
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
                    format!("3D buildings of the lead's area {unit}")
                } else if task["runs"][0]["what"] == crate::trees::task::KIND {
                    format!("Tree cover of the lead's blocks {}", task["blocks"].as_str().unwrap_or(&unit))
                } else if task["runs"][0]["what"] == crate::terrain_task::KIND {
                    format!("Terrain of the lead's subtrees {}", task["subtrees"].as_str().unwrap_or(&unit))
                } else {
                    format!("Scenery for the lead's area {unit}")
                };
                vec![JobSpec { id: format!("task {id}"), what, cmd, needs: Needs { nas: true }, restart_after_sleep: false, record: None }]
            }
            // (A resumable job granted as resuming was switched off here: given back, not failed.)
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Job { step, .. }, .. })) if resume::RESUMABLE.contains(&step.as_str()) => {
                if let Some(c) = &self.client {
                    if let Err(e) = c.give_back(lease, "resuming is off on this Mac now") {
                        eprintln!("agent: giving lease {lease} of {step} back: {e:#} (it lapses)");
                    }
                }
                Vec::new()
            }
            // (A step of the plan granted as every step to every member was switched off here: given
            // back, not failed.)
            Ok(Some(crate::coord::Grant { lease, work: crate::coord::Granted::Job { step, .. }, .. })) if every::offered(&step) => {
                if let Some(c) = &self.client {
                    if let Err(e) = c.give_back(lease, "every step to every member is off on this Mac now") {
                        eprintln!("agent: giving lease {lease} of {step} back: {e:#} (it lapses)");
                    }
                }
                Vec::new()
            }
            Ok(Some(g)) => {
                fail(self, g.lease, "this helper can't do that work");
                Vec::new()
            }
            Ok(None) => {
                self.know_pause(None);
                waiting.push(Waiting { step: None, what: "Building".into(), why: "the lead has nothing for this Mac now".into() });
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
                    None => self.unreached_why(&e),
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
        // (None from a job stopped for yielding: what it held says nothing of its targets.)
        let no_floors = self.slots[k].yield_stopped;
        let floors_of = |costs: &[(String, crate::coord::Cost)]| if no_floors { Vec::new() } else { watch.floors(&costs.iter().map(|(u, _)| u.clone()).collect::<Vec<_>>()) };
        // (And what they took on the disk at least, and the job over its run: learned whatever the
        // switch, docs/pool.md §7.7.)
        let disks = watch.disk_floors();
        let job_disk = watch.job_disk_mb();
        let job_step = watch.step.clone();
        if watch.disk_fell > 0 {
            eprintln!("agent: slot {k}'s job: the disk fell {} while it ran alone, its counts {} (a weaker signal, not a floor)", room::size(watch.disk_fell), room::size(watch.disk_peak));
        }
        let guard_stopped = self.slots[k].guard_stopped;
        // An own job's timings, taken whichever way it ends (none left for the slot's next job).
        let own_timings = self.slots[k].timings_out.take();
        // (A member's job's tasks, at this Mac's broker: gone with it.)
        if let (Some(b), Some(p)) = (&self.broker, pid) {
            b.close_tasks(p);
        }
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
                    c.add_disks(&disks);
                    c.add_job_disk(&job_step, job_disk);
                    c.add_costs_by(&measured, &self.worker_of(k));
                    std::fs::remove_file(costs).ok();
                }
            }
            Some(Held::Pooled { id, dir, own, .. }) => {
                // Its hand-off (its saves, the targets it finished, whatever stopped it) to the
                // journal, by the pool's next step; its folder kept until a saved state holds it.
                let kept: Option<pool::JobKept> = std::fs::read(dir.join("work.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let costs = read_costs(&if own { self.costs_path(k) } else { dir.join("costs.jsonl") });
                let entry = match (&kept, self.member_id.as_deref()) {
                    (Some(w), Some(me)) => match pool::entry_of(&dir, me, w.lease, &w.step, done, now_s()) {
                        Ok(e) => e,
                        Err(e) => {
                            eprintln!("agent: {}'s hand-off can't be read ({e:#}); its work is done again", dir.display());
                            None
                        }
                    },
                    _ => None,
                };
                // (Through the lead wherever it is now, as `beat` does: this process's coordinator
                // only while it still holds its term; one that froze and woke to a later term ends
                // it through that term's lead, once its part has changed: `owed`.)
                let local = self.coord.is_some() && !self.lost_term();
                if local {
                    if let Some(c) = &self.coord {
                        c.finish(id, done);
                        if let Some(p) = pid {
                            c.close_tasks(p);
                        }
                        c.add_floors(&floors_of(&costs));
                        c.add_disks(&disks);
                        c.add_job_disk(&job_step, job_disk);
                        c.add_costs_by(&costs, &self.worker_of(k));
                        std::fs::remove_file(self.costs_path(k)).ok();
                    }
                } else {
                    // (Its lease ended with the lead: its targets kept out of offers until merged.
                    // Best effort: unanswered, the lease lapses, and the merged records say they're
                    // built.)
                    // (One the memory guard stopped ends as failed: its lead keeps its targets from this
                    // Mac for an hour, doubling.)
                    let failed = outcome == Outcome::Failed || guard_stopped;
                    let floors = floors_of(&costs);
                    let timings = if own { own_timings } else { read_timings(&dir) };
                    // (A job with targets is done by its done record; one of no targets of its own,
                    // a lead's duty as pass-sets', by having ended so, whatever it handed off.)
                    let no_targets = kept.as_ref().is_some_and(|w| w.targets.is_empty());
                    let tell = match entry.as_ref() {
                        Some(e) if e.handoff.done.is_some() => Tell::Done(Box::new(crate::coord::Done { lease: id, handoff: Some(e.handoff.clone()), costs, floors, disks, job_disk_mb: job_disk, failed, journaled: true, timings, ..Default::default() })),
                        e if no_targets && outcome == Outcome::Done && !guard_stopped => Tell::Done(Box::new(crate::coord::Done { lease: id, handoff: e.map(|e| e.handoff.clone()), costs, floors, disks, job_disk_mb: job_disk, journaled: true, timings, ..Default::default() })),
                        _ if matches!(outcome, Outcome::Paused | Outcome::Interrupted) && !guard_stopped => Tell::GiveBack { id, why: note.to_string(), floors, disks, job_disk },
                        _ => Tell::Fail { id, why: note.to_string(), floors, disks, job_disk },
                    };
                    if self.coord.is_some() {
                        eprintln!("agent: lease {id} ended as this Mac finds its term lost: told to the lead once it works as a member");
                        self.owed.push((now_s(), tell));
                    } else {
                        self.tell_or_owe(tell);
                    }
                }
                let held = match (entry, self.try_pooled()) {
                    (Some(e), Some(Some(mut run))) => {
                        run.entries.push((e, Some(dir)));
                        None
                    }
                    // (A tick holds the run: handed at the next pool step.)
                    (Some(e), Some(None)) => Some((e, Some(dir))),
                    // (Nothing to hand off: the folder goes; no pool any more, it waits for the next
                    // process, which hands it off.)
                    (None, Some(_)) => {
                        std::fs::remove_dir_all(&dir).ok();
                        None
                    }
                    _ => None,
                };
                self.held_entries.extend(held);
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

    /// Whether this process leads a term a later one has followed, its step not knowing yet (it
    /// froze, and a takeover made the next meanwhile): a stat of the next term's file, as the
    /// driver's step makes. False when it leads none, or the NAS can't say now.
    fn lost_term(&self) -> bool {
        // (A tick holds the run: it can't say now.)
        let Some(Some(run)) = self.try_pooled() else { return false };
        let Some(e) = run.side.driver().leads() else { return false };
        crate::pool::term::next(run.side.nas(), e).unwrap_or(false)
    }

    /// Tells the lead a job's end, or owes it (not told: the lead didn't answer, its round failed
    /// already, or there's no client): told again each loop until `OWED_S` (`tell_owed`), never
    /// lost to a fast fail.
    fn tell_or_owe(&mut self, tell: Tell) {
        if !self.client.as_ref().is_some_and(|c| tell.to(c)) {
            self.owed.push((now_s(), tell));
        }
    }

    /// Begins a round of the lead's client, at each loop's start (`step`): once the lead doesn't
    /// answer, the loop's later calls to it fail at once rather than each waiting out its timeout;
    /// the next loop tries it again, so a lead back is heard within a loop
    /// (crate::coord::client::Client::new_round).
    fn begin_round(&self) {
        if let Some(c) = &self.client {
            c.new_round();
        }
    }

    /// The ends owed to the lead (`owed`), told once this process works as a member, through the
    /// lead's client: each kept until the lead answers, or `OWED_S` after it ended.
    fn tell_owed(&mut self, root: &Path) {
        if self.owed.is_empty() || self.coord.is_some() {
            return;
        }
        let now = now_s();
        let owed = std::mem::take(&mut self.owed);
        let Some(c) = self.client(root, &mut Vec::new()) else {
            self.owed = owed.into_iter().filter(|(at, _)| now.saturating_sub(*at) < OWED_S).collect();
            return;
        };
        let left: Vec<(u64, Tell)> = owed.into_iter().filter(|(at, t)| !t.to(c) && now.saturating_sub(*at) < OWED_S).collect();
        self.owed = left;
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
                    // (The build's pause as the lead says: this job stops with it, or goes on;
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

    fn record_path(&self, k: usize) -> PathBuf {
        self.o.home.join(if k == 0 { "job.json" } else { "job-2.json" })
    }

    /// Where slot `k`'s job leaves its timings' record (`SCENIC_TIMINGS`).
    /// Slot `k`'s running job's own time, from the timings it leaves as it ends (crate::timings:
    /// its id's), when it has left them.
    fn run_time(&self, k: usize) -> Option<f64> {
        let id = &self.slots[k].running.as_ref()?.spec.id;
        let p = match &self.slots[k].lease {
            Some(Held::Leased { dir, .. } | Held::Pooled { dir, own: false, .. }) => dir.join(TIMINGS_FILE),
            _ => self.timings_path(k),
        };
        let rec: crate::timings::RunRec = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
        (rec.id == *id || rec.id.is_empty()).then_some(rec.wall_s)
    }

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

    /// Where slot `k`'s job's accessors keep the bytes they've added to the caches
    /// (store::cachefile::WRITTEN_ENV), for the disk guard's sampler.
    fn written_path(&self, k: usize) -> PathBuf {
        self.o.home.join(if k == 0 { "written" } else { "written-2" })
    }

    /// Where slot `k`'s job keeps the file that says it waits on a worker (crate::offload's
    /// WAITING_ENV), while it does.
    fn waiting_path(&self, k: usize) -> PathBuf {
        self.o.home.join(if k == 0 { "waiting" } else { "waiting-2" })
    }

    /// What slot `k`'s job waits on, when it waits on a worker holding its unit's tail and nothing
    /// else (its file, `waiting_path`): None with no job, no file, or a stale one (its process gone,
    /// or not of this job's group).
    fn waiting_on(&self, k: usize) -> Option<crate::offload::WaitNote> {
        let r = self.slots[k].running.as_ref()?;
        let n = crate::offload::WaitNote::read(&self.waiting_path(k))?;
        let pid = i32::try_from(n.pid).ok()?;
        (pid == r.pgid || crate::sys::group_members(r.pgid).contains(&pid)).then_some(n)
    }

    /// What slot `k`'s job waits on, in words, while it waits on a worker (`waiting_on`).
    fn waiting_words(&self, k: usize) -> Option<String> {
        let n = self.waiting_on(k)?;
        let who = if n.worker.is_empty() { "a worker" } else { n.worker.as_str() };
        Some(format!("waiting on {who} for its unit's tail ({} min so far): its room free for a job beside it", now_s().saturating_sub(n.since) / 60))
    }

    /// Slot `k`'s worker in the build's history and the forecast: this Mac, or its second job.
    fn worker_of(&self, k: usize) -> String {
        if k == 0 {
            self.host.clone()
        } else {
            second_worker(&self.host)
        }
    }

    /// Whether this agent runs a second job: the lead's (a member's are leased one at a time),
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
        // (No target: by measure, the reserve alone.)
        if self.floor() == 0 {
            return format!("waits for room on the disk: it needs {} free ({} free now), more than this Mac's caches can free now; it starts once the disk has room", room::size(need), room::size(self.disk_free()));
        }
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
        root.filter(|r| (self.probe)(r))
    }

    /// Runs until stopped (SIGTERM, SIGINT), or for one loop with `once`.
    pub fn run(&mut self) -> Result<()> {
        // (The handler only stores to an atomic.)
        crate::sys::on_terminate(on_signal);
        // The memory guard's sampler, on a thread of its own (crate::agent::memguard), its latest
        // sample written for the status whatever this loop waits on.
        self.sampler.write_to(self.o.home.join(memguard::LIVE));
        self.sampler.spawn();
        // The pool's own thread: its driver stepped while this loop is busy (`pool::Ticker`).
        self.ticker = self.pool.clone().map(pool::Ticker::start);
        if self._lock.is_some() {
            // (What they finished went to the journal with their hand-offs, as they saved.)
            for k in 0..SLOTS {
                let orphan = jobs::stop_orphan(&self.record_path(k));
                self.orphans.extend(orphan.left);
            }
        }
        loop {
            let quick = self.step()?;
            if self.o.once || STOP.load(Ordering::SeqCst) {
                break;
            }
            // A newer app is in place and the first job's slot is free (its job stopped at a safe
            // point, `tend`, or past the grace still draining): exit (a job still running stops
            // here, its finished targets kept, and goes on under the new one), and the launcher
            // starts the new one.
            if self.update_due() {
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
        // The pool's thread stopped first: nothing steps the driver after this loop (its tick under
        // way finished, waited for a minute at most: a NAS that hangs may not return).
        if let Some(t) = self.ticker.take() {
            t.stop(Duration::from_secs(60));
        }
        // (Hand-offs held for a pool step that never came, `held_entries`, are lost with this
        // process: their folders stay, and the next one hands them off, `take_left`.)
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
                self.stopped(k, "the agent stopped");
                self.slots[k].running = None;
                std::fs::remove_file(self.record_path(k)).ok();
            }
        }
        // The coordinator's contact off the NAS: workers wait for the next agent.
        if let (Some(c), Some(root)) = (&self.coord, &root) {
            c.unpublish(root);
        }
        Ok(())
    }

    /// This member's run in the pool, locked (`pool::lock`): held only for what reads or changes it,
    /// the pool's own thread waiting meanwhile.
    fn pooled(&self) -> Option<std::sync::MutexGuard<'_, pool::Run>> {
        self.pool.as_ref().map(pool::lock)
    }

    /// The run, locked, if no tick holds it now (outer None: no pool; inner None: a tick holds it,
    /// maybe on a share that hangs). What the loop reads of the pool outside its pool step goes
    /// through this, or `pool_seen`: it never waits on a tick.
    fn try_pooled(&self) -> Option<Option<std::sync::MutexGuard<'_, pool::Run>>> {
        self.pool.as_ref().map(pool::try_lock)
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
        self.begin_round();
        // The build's pause: this Mac's ask passed on; the lead's coordinator's.
        self.sync_pause(root.as_deref(), &mut waiting);

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
        ended |= self.guard();
        ended |= self.disk_guard(root.as_deref());
        // (A job ended: room for the target may be made again.)
        if ended {
            self.floor_short = None;
        }
        // The pool's step, once the jobs that ended handed their work over (docs/pool.md §12):
        // what this Mac may do now, as its lead or a member.
        let gates = self.pool_step(root.as_deref(), &c, &mut waiting);
        // (Ends owed to the lead, once this process works as a member.)
        if let Some(r) = root.as_deref() {
            self.tell_owed(r);
        }
        self.lead_awake();
        // (Leading, nothing new starts while the driver says it may not: settling a handover, its
        // view not fresh. Restarting into a new part, nothing either.)
        let pool_holds: Option<String> = match (&gates, self.pool_restart()) {
            (_, Some(why)) => Some(format!("restarting: {why}")),
            (None, None) if self._lock.is_some() && !self.o.dry_run => Some("not in the pool yet: waiting for the NAS, or for this Mac's member's lock (another process holds it)".into()),
            (Some(g), None) if !self.o.helper && !g.duties => Some(if g.settle { "the lead is handing the build over".into() } else if g.leads.is_some() { "the lead re-asserts its term first".into() } else { "this Mac no longer leads the build".into() }),
            _ => None,
        };
        // The pass's answers on the NAS, once this agent starts (the lead's).
        self.seed_answers(root.as_deref());

        // The coordinator: where the NAS is (tasks read it where it lies), how to reach it on the NAS
        // (looked at every five minutes: it may have been deleted, or this Mac's addresses changed),
        // and leases whose workers went quiet dropped.
        if let Some(c) = &self.coord {
            c.set_root(root.as_deref());
        }
        // (In the pool, only while this Mac leads: a lead that stepped down takes its contact off,
        // so members find the new lead's.)
        let leading = self.pool_seen.as_ref().is_some_and(|p| p.gates.leads.is_some());
        if let (Some(c), Some(r), false) = (&self.coord, &root, leading) {
            if self.published.is_some() {
                c.unpublish(r);
                self.published = None;
            }
        }
        // (Not while the pool's step is taken for hung: its contact stays off, `pool_skipped`.)
        let leading = leading && self.pool_hung.is_none();
        if let (Some(c), Some(r), true) = (&self.coord, &root, leading) {
            if self.published.is_none_or(|t| t.elapsed() >= Duration::from_secs(300)) {
                self.publish_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

        let idle = self.slots[0].running.is_none();
        // The gate: the drop boxes listed off the loop (the lead's), and the asks of it from this
        // Mac's menu bar, map, `scenic inputs` and (leading) the build page.
        if let Some(r) = root.as_deref() {
            self.tend_gate(r);
        }

        // The plan: start the first job that can run. Made when a job ends; with the first job's
        // slot free, when what it depends on changed, a start held briefly, a timed wait ended, or as a fallback (`plan_due`; once each loop
        // took a core on an idle lead, task #154); while it runs, each minute for the second slot,
        // else every five minutes for the heartbeat (between, its last view is shown).
        let second_free = self.second_allowed() && self.slots[1].running.is_none();
        // (Not while the first runs alone: nothing would start beside it.)
        let alone = self.slots[0].running.as_ref().is_some_and(|r| self.runs_alone(&r.spec));
        let every = Duration::from_secs(if second_free && !alone { 60 } else { 300 });
        // (Idle, made again only when something it depends on changed, or as a fallback: `plan_due`.)
        let seen = match (&root, idle && !ended && !self.o.helper) {
            (Some(r), true) => self.plan_seen(r),
            _ => 0,
        };
        let last = self.planned.as_ref().map(|p| LastPlan { age: p.at.elapsed(), seen: p.seen, soon: p.soon, at: p.replan_at });
        let plan_due = plan_due(idle && !self.o.helper, ended, last, seen, now_s(), every);
        let plan = match &root {
            Some(root) if self.o.helper => {
                // A member plans nothing: it asks the lead for work, once its last is handed back.
                let unsent = std::fs::read_dir(self.outbox()).map(|mut d| d.next().is_some()).unwrap_or(false);
                // (Not while a trim or a clear runs here: `caches_busy`.)
                let plan = if idle && !unsent && self.caches_task.is_none() { self.helper_job(root, &c, &mut waiting) } else { Vec::new() };
                let regions = match &self.planned {
                    Some(p) if !plan_due => p.regions.clone(),
                    _ => load_regions(root).0,
                };
                self.planned = Some(Planned { at: Instant::now(), seen: 0, soon: false, replan_at: None, waiting: waiting.clone(), regions, bad: Vec::new() });
                plan
            }
            Some(root) if plan_due => {
                // (What it depends on read before planning: a change while it plans is seen next loop.)
                let seen = if seen == 0 { self.plan_seen(root) } else { seen };
                let plan = self.plan(root, &c, &mut waiting);
                let (regions, bad) = load_regions(root);
                self.planned = Some(Planned { at: Instant::now(), seen, soon: false, replan_at: None, waiting: waiting.clone(), regions, bad });
                plan
            }
            Some(_) => {
                waiting.extend(self.planned.as_ref().map(|p| p.waiting.clone()).unwrap_or_default());
                Vec::new()
            }
            None => {
                // (The NAS back: planned at once, whatever the digest.)
                if let Some(p) = self.planned.as_mut() {
                    p.seen = 0;
                }
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
        // The build paused: nothing new starts (the running jobs stop above), and on the lead
        // its coordinator gives its workers nothing and has their jobs stop too.
        if let Some(p) = &self.pause {
            waiting.push(Waiting { step: None, what: "Building".into(), why: p.why() });
        }
        if let Some(why) = &pool_holds {
            waiting.push(Waiting { step: None, what: "Building".into(), why: why.clone() });
        }
        // (A trim, a clear or a freeing toward the room target under way: jobs start meanwhile, what
        // they use held, store::cachefile.)
        // (Idle and not planned again: nothing to try, the last plan's waits shown.)
        if idle && !newer && self.pause.is_none() && pool_holds.is_none() && (plan_due || !plan.is_empty()) {
            self.start_soon.set(false);
            self.start_first(&plan, &c, root.as_deref(), &mut waiting);
            // (A start held by something on a short timer: planned again next loop; timed waits, at
            // the earliest's end: `plan_due`.)
            let (soon, at) = (self.start_soon.get(), self.replan_at(&plan, now_s()));
            // (Why the jobs before it wait: kept with the plan, so the status says so while the job
            // started runs, not only between jobs.)
            if let Some(p) = self.planned.as_mut() {
                p.waiting = waiting.clone();
                p.soon = soon;
                p.replan_at = at;
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
        // (The jobs started: their leases are in the coordinator's state, so a tick may settle a
        // handover from here; a duty that started runs, so it may not until this loop stops it.)
        let duty = self.slots.iter().any(|s| s.running.as_ref().is_some_and(|j| is_duty(&j.spec.id)));
        if let Some(Some(mut run)) = self.try_pooled() {
            run.cx.starting = false;
            run.cx.duty = duty;
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
        // (Read each loop: a helper's status changes between plans. Not this Mac's own, from
        // while it was a member: it leads now, and that file goes stale by itself.)
        let helpers: Vec<Status> = match (&root, self.o.helper) {
            (Some(r), false) => helpers(r).into_iter().filter(|h| h.host != self.host).collect(),
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
            test_disk_extra_mb: Some(cond::test_disk_extra_mb()).filter(|&m| m > 0),
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
            map_update: if self.o.helper { None } else { self.update_decision.borrow().clone() },
            timetables: if self.o.helper { None } else { self.timetables.borrow().as_ref().map(|t| t.1.clone()) },
            catalog: self.catalog_seen.get(),
            caches: Some(self.caches_view(caches_why, c.home, c.nas)),
            pool: match self.try_pooled() {
                Some(Some(p)) => Some(p.pool_view()),
                _ => self.pool_seen.clone(),
            },
            memory: Some(self.guard_view()),
            budgets: self.proxy.as_ref().map(|p| p.view()),
            inputs: if self.o.helper { Vec::new() } else { self.inputs.view.borrow().clone() },
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
        let run_time = self.run_time(k);
        let Some(r) = self.slots[k].running.as_mut() else { return Ok(false) };
        if let Some(st) = r.poll()? {
            // (Its own run's time as it says, else as seen here: an ended job is seen at the next
            // loop, up to 20 s later.)
            let wall = run_time.filter(|w| *w <= r.elapsed().as_secs_f64() + 1.0).unwrap_or_else(|| r.elapsed().as_secs_f64());
            let secs = wall.round() as u64;
            let ok = st.success();
            // Stopped at a safe point, the build pausing (crate::control): not a failure. (Only
            // when it was asked to: a job's own exit 75 for anything else is a failure.)
            let paused = st.code() == Some(crate::control::PAUSED_EXIT) && self.slots[k].drain_since.is_some();
            let of = r.spec.record.as_ref().map_or(0, |w| w.targets.len());
            let step = r.spec.record.as_ref().map(|w| w.step.clone()).unwrap_or_default();
            // The targets it finished: every one when it succeeded, else those it noted done as
            // each was saved (crate::control::done), whatever stopped it, so they aren't built
            // again: handed off with the job's lease (the journal has their saves).
            let done = self.finished_targets(k, ok);
            let r = self.slots[k].running.as_mut().unwrap();
            let note = if ok {
                // (GC's warnings, its job's note in the status: agent::gc::Report::warnings.)
                if step_of(&r.spec.id).as_deref() == Some("gc") { jobs::tail(&r.log, 50).lines().filter(|l| l.contains("gc: warning:")).collect::<Vec<_>>().join("\n") } else { String::new() }
            } else if paused {
                format!("paused at a safe point, {} of {of} done; the rest goes on when the build does", done.len())
            } else {
                format!("{st}{}\n{}", if done.is_empty() { String::new() } else { format!(" ({} of {of} done and kept)", done.len()) }, jobs::tail(&r.log, 20))
            };
            let (id, what) = (r.spec.id.clone(), r.spec.what.clone());
            self.progress_ended(&id, ok);
            eprintln!("agent: {id} {} after {} s", if ok { "finished" } else if paused { "paused at a safe point" } else { "failed" }, if wall < 10.0 { format!("{wall:.1}") } else { secs.to_string() });
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
            // (The coordinator's record of what's done.)
            let handed = done.clone();
            // (A catalog's done record handed off: its map update is over.)
            if ok && matches!(step.as_str(), "catalog" | "catalog-held") {
                self.end_update();
            }
            self.take_timings(k);
            self.end_lease(k, outcome, &handed, &note);
            self.finished(&id, &what, ok || paused, secs, note);
            self.slots[k].drain_since = None;
            self.slots[k].mem_drain = None;
            self.slots[k].mem_drain_alone = false;
            self.slots[k].running = None;
            self.slots[k].yields = None;
            std::fs::remove_file(self.record_path(k)).ok();
            return Ok(true);
        }
        if slept > 30 && r.spec.restart_after_sleep && r.spec.needs.nas {
            // Open SMB handles often don't survive sleep: stop it; the plan below starts it again
            // from its completion markers once its conditions hold.
            eprintln!("agent: slept {slept} s; restarting {}", r.spec.id);
            r.stop(Duration::from_secs(30));
            self.stopped(k, "stopped: the Mac slept");
            self.slots[k].running = None;
            std::fs::remove_file(self.record_path(k)).ok();
            return Ok(false);
        }
        let paused_job = self.slots[k].running.as_ref().unwrap().paused.is_some();
        if (!paused_job || self.pause.is_some()) && self.slots[k].lease.is_some() && self.slots[k].beaten.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) && !self.beat(k, root) {
            // (A job paused for its conditions doesn't beat: its lease lapses, and its work may go
            // to another. One paused by the user does, its lease kept, a member hearing whether
            // the lead's pause is over.)
            let r = self.slots[k].running.as_mut().unwrap();
            eprintln!("agent: {}'s lease lapsed and its work went to another worker; stopping it", r.spec.id);
            r.stop(Duration::from_secs(30));
            self.stopped(k, "its lease lapsed");
            self.slots[k].running = None;
            std::fs::remove_file(self.record_path(k)).ok();
            return Ok(true);
        }
        // Paused, or something it needs gone: at its next safe point (asked through its channel,
        // frozen after DRAIN_GRACE if it hasn't stopped), or at once (frozen where it is: it can't
        // save without the NAS); else going on.
        let control = self.control_path(k);
        let pause = self.pause.clone();
        let grace = self.grace;
        // (A newer app installed: every job stops at its next safe point, whatever part this Mac
        // plays, lead, member, broker or helper, and the loop restarts into it once the first job's
        // slot is free, `Agent::update_due`.)
        let update = self.pending_app().map(|a| update_why(&a));
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
        match stop_for(&r.spec.needs, c, pause.as_ref()).or_else(|| slot.mem_drain.clone().map(|why| (crate::control::Mode::Drain, why))).or_else(|| update.clone().map(|why| (crate::control::Mode::Drain, why))) {
            Some((crate::control::Mode::Freeze, why)) => {
                if r.paused.is_none() {
                    eprintln!("agent: pausing {}: {why}", r.spec.id);
                }
                r.pause(&why);
            }
            Some((crate::control::Mode::Drain, why)) => {
                // (The update's drain timed from itself: one begun for another reason, a pause
                // lifted, say, starts its grace again.)
                let for_update = update.as_deref() == Some(why.as_str());
                if for_update && r.pausing.as_deref() != Some(why.as_str()) {
                    slot.drain_since = None;
                }
                let since = *slot.drain_since.get_or_insert_with(|| {
                    eprintln!("agent: {} stops at its next safe point: {why}", r.spec.id);
                    if let Err(e) = std::fs::write(&control, b"drain") {
                        eprintln!("agent: asking {} to stop: {e}", r.spec.id);
                    }
                    Instant::now()
                });
                // (For the update, not frozen past the grace: the loop stops it then, its finished
                // targets kept, and restarts, `Agent::update_due`.)
                if r.paused.is_none() && since.elapsed() >= grace && !for_update {
                    eprintln!("agent: {} reached no safe point in {} min; frozen where it is", r.spec.id, DRAIN_GRACE.as_secs() / 60);
                    r.pause(&format!("{why}; frozen where it is (it reached no safe point in {} min): it goes on from there", DRAIN_GRACE.as_secs() / 60));
                } else if r.paused.is_some() && since.elapsed() < grace {
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
    /// own; one that waits for a reason of its own passed over, one that waits for the job beside it
    /// (`waits_for_second`) waited for. Why the jobs before it wait, in `waiting`.
    fn start_first(&mut self, plan: &[JobSpec], c: &Conditions, root: Option<&Path>, waiting: &mut Vec<Waiting>) {
        let beside = self.slots[1].running.as_ref().map(|r| r.spec.id.clone());
        // (The least room a job held by the disk room target needed: one after it needing as much
        // waits too, unsaid; one needing less is tried.)
        let mut held = u64::MAX;
        self.floor_held = None;
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
                break;
            }
            if self.try_start(0, spec.clone(), c, root, waiting) {
                break;
            }
            if let Some(h) = self.floor_held.take() {
                held = held.min(h);
            }
        }
    }

    /// Why `spec` can't start now for itself: a condition it needs gone, or a failure's wait.
    fn wait_reason(&self, spec: &JobSpec, c: &Conditions) -> Option<String> {
        if let Some(why) = lapsed(&spec.needs, c) {
            return Some(why);
        }
        // (A job's only way out is the proxy: none starts without it.)
        if self.proxy.is_none() && !self.o.dry_run {
            return Some("waits for the budgets' proxy to start (every job's connections go through it)".into());
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
        let beside = steps::title(&b);
        if self.clashes(&s, &b) {
            return Some(format!("waits for the job beside it ({beside}) to end: they don't run together"));
        }
        self.fits_beside(spec, 1).map(|why| format!("waits for the job beside it ({beside}) to end: {why}"))
    }

    /// The free space a job of `spec` starts with in slot `k`: a member's its step's; else the
    /// reserve, and beside another job its measured disk too.
    fn need_of(&self, k: usize, spec: &JobSpec) -> u64 {
        if self.o.helper {
            helper_need(spec.record.as_ref().map_or("tail", |w| w.step.as_str()))
        } else {
            // (Beside another job: the reserve and what it was measured to take on the
            // disk. Alone in the first slot: the reserve alone, the disk guard its backstop, so a
            // stale or high disk floor, which only rises, never holds the build, the owner's room
            // target past it or not; a cold start likewise. Room is still made for its measured
            // disk as the caches allow: `try_start_said`.)
            let alone = k == 0 && self.slots.iter().skip(1).all(|s| s.running.is_none());
            room::RESERVE + if alone { 0 } else { self.measured_job(spec).map_or(0, |m| m.disk_mb << 20) }
        }
    }

    fn try_start_said(&mut self, k: usize, spec: JobSpec, c: &Conditions, root: Option<&Path>, waiting: &mut Vec<Waiting>) -> bool {
        if let Some(why) = self.wait_reason(&spec, c) {
            waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
            return false;
        }
        // The pool's lead publishes only once its records reflect the journal, and sweeps only on a
        // step that re-asserted its term, caught up (docs/pool.md §6.2, §6.6): an entry not merged
        // yet may hold uploads the records don't name.
        // (The gates as they are now, under the run's lock: the pool's thread may have stepped since
        // this loop's step, a handover settling. A duty that passes is marked as running from here,
        // so a tick doesn't settle under it: `pool::Cx::duty`.)
        if let (Some(run), false) = (self.try_pooled(), self.o.helper) {
            let st = step_of(&spec.id).unwrap_or_default();
            let Some(mut run) = run else {
                waiting.push(Waiting { step: Some(st), what: spec.what.clone(), why: "the pool's step is under way (its share slow to answer)".into() });
                self.start_soon.set(true);
                return false;
            };
            if run.gates.leads.is_some() && !run.gates.duties {
                waiting.push(Waiting { step: Some(st), what: spec.what.clone(), why: if run.gates.settle { "the lead is handing the build over".into() } else { "the lead re-asserts its term first".into() } });
                return false;
            }
            if pool::PUBLISHES.contains(&st.as_str()) && !run.gates.publish() {
                waiting.push(Waiting { step: Some(st), what: spec.what.clone(), why: "waits for the pool's records to reflect the journal (a listing of every day merged)".into() });
                return false;
            }
            if pool::SWEEPS.contains(&st.as_str()) && !run.gates.sweep() {
                run.reassert = true;
                waiting.push(Waiting { step: Some(st), what: spec.what.clone(), why: "this Mac re-asserts its term first, its records caught up".into() });
                return false;
            }
            run.cx.duty |= is_duty(&spec.id);
            run.cx.starting = true;
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
        // (By measure, the first slot alone waits for the reserve too, the target none: started
        // short of it, the disk guard would freeze it at once and give it back, and the next job
        // likewise, each kept from this Mac an hour.)
        let held = true;
        if !(k > 0 || other) && held && !self.o.helper && short && self.disk_free() < room {
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
            // (Its own scratch, kept from a give-back, its own again before room is made: never
            // room-making's, the OSM pass's above all.)
            if let Some(d) = scratch_of(&spec) {
                room::given_back_drop(&self.o.home, &d);
            }
            // (By measure, alone: room is made, as it can, for its measured disk too, though it
            // waits only on the reserve and the target, `need_of`: the disk guard its backstop.)
            let measured = if !self.o.helper { self.measured_job(&spec).map_or(0, |m| m.disk_mb << 20) } else { 0 };
            let sized = room.saturating_add(measured);
            match room::make_room(&cache, &r.join("sources"), sized, margin, &hints) {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!("agent: {} GB of the cheap caches deleted (canopy squares, raw terrain tiles, copies of the records' files, pageview months) for {} GB free", n >> 30, sized.saturating_add(margin) >> 30);
                    self.cheap = None;
                }
                Err(e) => eprintln!("agent: making room on the disk: {e:#}"),
            }
            // (Still short, the caches freed: the scratch of jobs given back here, the oldest first.)
            if self.disk_free() < room.saturating_add(margin) {
                self.free_given_back(room.saturating_add(margin));
            }
            // Still short of the target past its room: the job waits (it would fill what the owner
            // keeps free), as do the jobs after it; the caches' others are freed toward the target
            // between jobs (`tend_caches`).
            // The caches' others are then freed toward it between jobs (`tend_caches`, `goal`: the
            // least room a job held needs), and room-making tried again once that's done.
            if held && !self.o.helper && self.disk_free() < room {
                waiting.push(Waiting { step: Some(step), what: what.clone(), why: self.floor_why(need) });
                self.floor_held = Some(room);
                self.floor_short = Some((floor, self.floor_short.filter(|f| f.0 == floor).map_or(room, |f| f.1.min(room)), Instant::now()));
                return false;
            }
        } else if held && !self.o.helper && self.disk_free() < room {
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
        // coordinator; when another holds
        // one, none starts (the next loop plans without it). A helper's job came with its lease.
        if let (Some((step, _)), Some(r), false) = (shared_targets(&spec), root, self.o.helper) {
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
            // Recorded since this loop read the keys (another worker built it meanwhile): not built
            // again.
            let done = self.planning_keys(r).map(|keys| spec.record.as_ref().is_some_and(|w| w.targets.iter().any(|(t, k)| keys.recorded(&w.step, t) == Some(k.as_str()))));
            if done.unwrap_or(true) {
                let_go(self);
                waiting.push(Waiting { step: None, what: what.clone(), why: "another worker built part of it meanwhile (or the keys can't be read now); planning again".into() });
                return false;
            }
            self.slots[k].lease = lease.map(Held::Own);
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
        // (A catalog's start: the next map update waits an hour from it, whether or not it goes out.)
        if spec.id.starts_with("catalog") {
            self.mem.catalog_at = now_s();
            self.save();
        }
        if let Err(e) = self.start(k, spec, c) {
            // It couldn't even start (a missing program, a full disk): retried later.
            eprintln!("agent: can't start {id}: {e:#}");
            self.end_lease(k, Outcome::Failed, &[], &format!("couldn't start: {e:#}"));
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
        // (Any job of the plan, in its order.)
        let picks: Vec<&JobSpec> = plan.iter().collect();
        let first = self.slots[0].running.as_ref().map(|r| (step_of(&r.spec.id).unwrap_or_default(), r.spec.id.clone(), crate::sys::footprint_of_group(r.pgid).map_or(0, |b| b >> 20), self.spec_peak(&r.spec)));
        if self.slots[0].running.as_ref().is_some_and(|r| self.runs_alone(&r.spec)) {
            let f = first.unwrap().0;
            let what = self.slots[0].running.as_ref().map_or_else(|| steps::title(&f).to_string(), |r| r.spec.what.clone());
            self.beside_why = Some(format!("{what} is a cold start (no run of its step measured yet): it runs alone"));
            return;
        }
        // What the first slot starts next: one that runs alone, or needs room made on the disk
        // (made only while one job runs), isn't held up by a job started here now.
        if let Some(h) = self.head(plan, c) {
            let hs = step_of(&h.id).unwrap_or_default();
            if self.runs_alone(h) {
                self.beside_why = Some(format!("{} runs alone, next (a cold start): nothing starts beside the first job until it has", h.what));
                return;
            }
            if self.disk_free() < self.need_of(0, h).saturating_add(self.floor_for(&hs)) {
                self.beside_why = Some(format!("{} needs room made on the disk next, which waits for one job alone: nothing starts beside the first job until it has", h.what));
                return;
            }
        }
        let mut why: Option<String> = None;
        for spec in picks {
            let step = step_of(&spec.id).unwrap_or_default();
            if first.as_ref().is_some_and(|f| f.1 == spec.id || self.clashes(&f.0, &step)) {
                continue;
            }
            // (Both jobs' measured memory and disk within what the Mac has left.)
            if let Some(w) = self.fits_beside(spec, 0) {
                why.get_or_insert_with(|| format!("{}: {w}", steps::title(&step)));
                continue;
            }
            self.beside_why = None;
            let beside_waiting = self.waiting_on(0).and(self.slots[0].running.as_ref().map(|r| r.spec.id.clone()));
            if self.try_start(1, spec.clone(), c, root, &mut Vec::new()) {
                self.slots[1].yields = beside_waiting;
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

    /// This Mac's room now, as a member's (MB: memory, disk), for a cold start with every step to
    /// every member (crate::coord::Every): its memory guard's limit less what its jobs hold, its
    /// disk's free space less the reserve and the owner's room target.
    fn own_room(&self) -> (u64, u64) {
        // (Free now: none while it can start no job, its slots full or a job running alone.)
        let running: Vec<&JobSpec> = self.slots.iter().filter_map(|s| s.running.as_ref().map(|r| &r.spec)).collect();
        if running.len() >= SLOTS || running.iter().any(|j| self.runs_alone(j)) {
            return (0, 0);
        }
        let held: u64 = (0..SLOTS).filter_map(|k| self.held_mb(k)).sum();
        // (Before the loop's first read of the Mac's memory, as the Mac says it now.)
        let total = match self.total_mb() {
            0 => self.mac_total_mb(),
            t => t,
        };
        let mem = memguard::limit_mb(total).saturating_sub(held);
        let disk = self.disk_free().saturating_sub(room::RESERVE + self.floor()) >> 20;
        (mem, disk)
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
    fn guard(&mut self) -> bool {
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
        // (A job started beside one waiting on a worker, that one gone on: stopped at once, not at
        // a safe point, once the two hold more than the limit or the Mac is short of memory; it was
        // placed by what the other held while it waited, so it did nothing wrong: `yield_stop`.)
        if let Some(k) = (0..SLOTS).find(|&k| self.yielding(k)) {
            let trouble = self.sampler.in_trouble();
            if total > limit || trouble {
                let gb = |mb: u64| mb as f64 / 1024.0;
                let why = if trouble { "the Mac short of memory".to_string() } else { format!("the jobs here holding {:.1} GB together, past this Mac's limit of {:.1} GB", gb(total), gb(limit)) };
                let beside = self.slots.iter().enumerate().find(|(b, _)| *b != k).and_then(|(_, s)| s.running.as_ref()).map(|r| r.spec.id.clone()).unwrap_or_default();
                return self.yield_stop(k, &format!("given back by the memory guard: started beside {beside} while it waited on a worker, it yields now that {beside} goes on, {why}"));
            }
        }
        if total <= limit {
            return false;
        }
        let draining: Vec<bool> = self.slots.iter().map(|s| s.mem_drain.is_some()).collect();
        let late = (0..SLOTS).find(|&k| draining[k] && !self.slots[k].mem_drain_alone && self.slots[k].drain_since.is_some_and(|t| t.elapsed() >= self.grace));
        let gb = |mb: u64| mb as f64 / 1024.0;
        let id_of = |a: &Self, k: usize| a.slots[k].running.as_ref().map(|r| r.spec.id.clone()).unwrap_or_default();
        let yields: Vec<bool> = (0..SLOTS).map(|k| self.yielding(k)).collect();
        let stop = match (late, memguard::decide(&held, limit, &draining, &yields)) {
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
        self.guard_stop(k, "memory guard", &why)
    }

    /// Whether slot `k`'s job yields: started beside a job that waited on a worker (`yields`), that
    /// job still running in another slot.
    fn yielding(&self, k: usize) -> bool {
        let Some(id) = self.slots[k].running.as_ref().and(self.slots[k].yields.as_ref()) else { return false };
        self.slots.iter().enumerate().any(|(b, s)| b != k && s.running.as_ref().is_some_and(|r| &r.spec.id == id))
    }

    /// Stops slot `k`'s job at once for yielding (`yielding`), `why`: given back as an interrupted
    /// job is, not kept from this Mac, and what it held learned as no floor (it was placed by what
    /// the job beside held while it waited, not by anything it did). True when it stopped one.
    fn yield_stop(&mut self, k: usize, why: &str) -> bool {
        self.sampler.thaw(k);
        self.sampler.thaw_disk(k);
        let Some(r) = self.slots[k].running.as_mut() else { return false };
        let (id, scratch) = (r.spec.id.clone(), scratch_of(&r.spec));
        eprintln!("agent: memory guard: {id} {why}");
        r.stop(Duration::from_secs(30));
        self.guard_last = Some((now_s(), format!("{id} {why}")));
        self.slots[k].yield_stopped = true;
        self.stopped(k, why);
        if let Some(d) = &scratch {
            room::given_back_add(&self.o.home, d, now_s());
        }
        self.slots[k].running = None;
        self.slots[k].drain_since = None;
        self.slots[k].mem_drain = None;
        self.slots[k].mem_drain_alone = false;
        self.slots[k].yield_stopped = false;
        self.slots[k].yields = None;
        std::fs::remove_file(self.record_path(k)).ok();
        true
    }

    /// Stops slot `k`'s job for a guard (`what`: the memory guard, the disk guard), `why`: given back,
    /// not failed, kept from this Mac an hour (a member's lease ends as failed, so its lead keeps it
    /// from this Mac as long), what it took learned as its target's floors. True when it stopped one.
    fn guard_stop(&mut self, k: usize, what: &str, why: &str) -> bool {
        let why = why.to_string();
        // (Frozen by the sampler: let go on first, so it can take its termination.)
        self.sampler.thaw(k);
        self.sampler.thaw_disk(k);
        let Some(r) = self.slots[k].running.as_mut() else { return false };
        let id = r.spec.id.clone();
        let scratch = scratch_of(&r.spec);
        eprintln!("agent: {what}: {id} {why}");
        r.stop(Duration::from_secs(30));
        self.guard_last = Some((now_s(), format!("{id} {why}")));
        self.guard_backoff.insert(id, (now_s() + GUARD_BACKOFF_S, why.clone()));
        self.slots[k].guard_stopped = true;
        self.stopped(k, &why);
        // (Its lease ended, given back: its scratch kept, its work with it, until room-making is
        // short of the reserve, `room::free_given_back`.)
        if let Some(d) = &scratch {
            room::given_back_add(&self.o.home, d, now_s());
        }
        self.slots[k].running = None;
        self.slots[k].drain_since = None;
        self.slots[k].mem_drain = None;
        self.slots[k].mem_drain_alone = false;
        self.slots[k].guard_stopped = false;
        self.slots[k].yields = None;
        std::fs::remove_file(self.record_path(k)).ok();
        true
    }

    /// The disk guard (docs/pool.md §7.7), once a loop, with the Mac shared by measure: a job the
    /// sampler froze, the disk short of the reserve and it growing fastest, goes on once the disk
    /// has the reserve free again; else room is made from the caches no job holds (store::cachefile's
    /// exclusive locks: room::toward, on a thread of its own), and once that's done, the disk still
    /// short, the job is given back (`guard_stop`: not failed, its work kept by its hand-off, its
    /// disk floor learned). True when it stopped a job.
    fn disk_guard(&mut self, root: Option<&Path>) -> bool {
        self.sampler.set_disk(true, room::RESERVE, &self.o.home);
        let frozen: Vec<usize> = (0..SLOTS).filter(|&k| self.slots[k].running.is_some() && self.sampler.disk_frozen(k)).collect();
        if frozen.is_empty() {
            self.disk_room = None;
            return false;
        }
        if self.disk_free() >= room::RESERVE {
            for &k in &frozen {
                if self.sampler.thaw_disk(k) {
                    let why = format!("goes on: the disk has {} free again", room::size(self.disk_free()));
                    eprintln!("agent: disk guard: slot {k}'s job {why}");
                    self.guard_last = Some((now_s(), format!("slot {k}'s job {why}")));
                }
            }
            self.disk_room = None;
            return false;
        }
        // (Of several frozen, the one taking the most is given back first: the most room back.)
        let k = frozen.iter().copied().max_by_key(|&k| (self.sampler.disk_now(k).unwrap_or(0), std::cmp::Reverse(k))).unwrap_or(frozen[0]);
        let ours = self.caches_task.as_ref().is_some_and(|t| t.disk);
        match self.disk_room {
            Some(_) if ours => false,
            // (Made, and still short: the scratch of jobs given back here, the oldest first; still
            // short, given back.)
            Some(_) if self.caches_task.is_none() => {
                if self.free_given_back(room::RESERVE + room::margin(room::RESERVE)) > 0 && self.disk_free() >= room::RESERVE {
                    return false;
                }
                self.disk_room = None;
                let why = format!("given back by the disk guard: the disk had {} free, short of the {} kept free, with no more room to be made from the caches no job holds (what it took on the disk is kept as the least its target takes)", room::size(self.disk_free()), room::size(room::RESERVE));
                self.guard_stop(k, "disk guard", &why)
            }
            // (A freeing of the caches under way frees already: its end waited for.)
            _ if self.caches_task.is_some() => false,
            _ => {
                let Some(r) = root else { return false };
                let (cache, sources) = (self.o.home.join("cache"), r.join("sources"));
                let goal = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(room::RESERVE + room::margin(room::RESERVE)));
                let hints = self.hints();
                match std::thread::Builder::new().name("caches".into()).spawn(move || room::toward(&cache, &sources, goal, &hints)) {
                    Ok(thread) => {
                        self.caches_task = Some(CachesTask { ask: None, toward: None, disk: true, began: Instant::now(), thread });
                        self.disk_room = Some(Instant::now());
                        // (One loop at a time, `--once`: waited for.)
                        if self.o.once {
                            if let Some(t) = self.caches_task.take() {
                                self.caches_done(t);
                            }
                        }
                    }
                    Err(e) => eprintln!("agent: disk guard: room-making's thread didn't start: {e}"),
                }
                false
            }
        }
    }

    /// Room-making's last resort: the scratch of jobs given back here deleted, the oldest first,
    /// until the disk has `need` free; never one a job running or frozen here uses. The bytes freed.
    fn free_given_back(&self, need: u64) -> u64 {
        let busy: Vec<PathBuf> = self.slots.iter().filter_map(|s| s.running.as_ref().and_then(|r| scratch_of(&r.spec))).collect();
        room::free_given_back(&self.o.home, need, &busy, &|| self.disk_free())
    }

    /// What a job of `spec` is measured to take (docs/pool.md §7.7, crate::coord::Coordinator::
    /// measured): the most of its targets' memory and disk (MB), or for a job of no targets of its
    /// own, its id's; None for a cold start (a target of a step none of whose targets has run), or
    /// with no coordinator to ask.
    fn measured_job(&self, spec: &JobSpec) -> Option<crate::coord::Measured> {
        let c = self.coord.as_ref()?;
        match spec.record.as_ref() {
            Some(w) if !w.targets.is_empty() => c.measured_batch(&w.step, &w.targets.iter().map(|t| t.0.clone()).collect::<Vec<_>>()),
            _ => {
                let key = own_cost_key(&spec.id);
                let step = memguard::step_of_key(&key).to_string();
                let target = key.strip_prefix(&format!("{step} ")).unwrap_or(&key).to_string();
                c.measured_batch(&step, &[target])
            }
        }
    }

    /// What slot `k`'s job has still to take on the disk, by measure (bytes): its targets not noted
    /// done yet, their disks summed (the one under way whole), or what the most a job of its step
    /// took over its run leaves past what it has taken, the larger.
    fn disk_left(&self, k: usize) -> u64 {
        let (Some(r), Some(c)) = (self.slots[k].running.as_ref(), self.coord.as_ref()) else { return 0 };
        let now = self.sampler.disk_now(k).unwrap_or(0);
        let (step, left): (String, Vec<String>) = match r.spec.record.as_ref() {
            Some(w) if !w.targets.is_empty() => {
                let done = crate::control::read_done(&self.done_path(k), &w.step);
                (w.step.clone(), w.targets.iter().map(|t| t.0.clone()).filter(|t| !done.contains(t)).collect())
            }
            _ => {
                let key = own_cost_key(&r.spec.id);
                let step = memguard::step_of_key(&key).to_string();
                let target = key.strip_prefix(&format!("{step} ")).unwrap_or(&key).to_string();
                (step, vec![target])
            }
        };
        let targets: u64 = left.iter().filter_map(|t| c.measured(&step, t)).map(|m| m.disk_mb << 20).sum();
        targets.max((c.job_disk(&step) << 20).saturating_sub(now))
    }

    /// Whether `spec` runs alone on this Mac: a cold start (`measured_job` none).
    fn runs_alone(&self, spec: &JobSpec) -> bool {
        self.measured_job(spec).is_none()
    }

    /// Whether jobs of steps `a` and `b` can't run at once on this Mac by the groups and the steps
    /// alone (`clash`); by measure, the raw tiles' group and the run-alone list gone (a raw tile is
    /// read under its accessor's lock, packing deletes only tiles no job holds: store::cachefile);
    /// with the budgets on, the Wikidata and heavy NAS reads' groups gone (`group_holds`).
    fn clashes(&self, a: &str, b: &str) -> bool {
        clash(a, b)
    }

    /// By measure, why `spec` can't start beside the job in slot `other` (None: both jobs' measured
    /// memory and disk fit what the Mac has left): a cold start runs alone, as does a job beside
    /// one; their memory past this Mac's limit (the other's as measured, or as it holds now if
    /// more); their disk past the disk's free space less the reserve (the other's as measured, less
    /// what it has taken so far).
    fn fits_beside(&self, spec: &JobSpec, other: usize) -> Option<String> {
        let r = self.slots[other].running.as_ref()?;
        let beside = steps::title(&step_of(&r.spec.id).unwrap_or_default()).to_string();
        let Some(mine) = self.measured_job(spec) else { return Some(format!("a cold start (no run of its step measured yet): it runs alone, once {beside} ends")) };
        let Some(theirs) = self.measured_job(&r.spec) else { return Some(format!("{beside} beside it is a cold start, which runs alone")) };
        let limit = memguard::limit_mb(self.total_mb());
        if limit == 0 {
            return Some("this Mac's memory can't be read: nothing starts beside another job".into());
        }
        // (One waiting on a worker holding its unit's tail: as it holds now, its own work done
        // until the tail is back; the memory guard has the job beside it yield if the two then
        // hold too much, `yields`.)
        let theirs_mb = match (self.waiting_on(other), self.held_mb(other)) {
            (Some(_), Some(now)) => now,
            _ => theirs.mem_mb.max(self.held_mb(other).unwrap_or(0)),
        };
        let gb = |mb: u64| mb as f64 / 1024.0;
        if mine.mem_mb + theirs_mb > limit {
            return Some(format!("measured at {:.1} GB, with {beside}'s {:.1} GB past this Mac's limit of {:.1} GB", gb(mine.mem_mb), gb(theirs_mb), gb(limit)));
        }
        let left = self.disk_left(other);
        let (need, free) = ((mine.disk_mb << 20) + left + room::RESERVE, self.disk_free());
        if need > free {
            return Some(format!("measured at {:.1} GB on the disk, with {beside}'s {:.1} GB still to come and the {} GB kept free, past the disk's {} free", gb(mine.disk_mb), left as f64 / (1u64 << 30) as f64, room::RESERVE >> 30, room::size(free)));
        }
        None
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
            if n_all > 0 && !steps::SHARED.contains(&w.step.as_str()) {
                // A step only the lead runs: judged against the lead's limit alone. A member whose
                // limit fits it is named, with how to hand it the lead (not done here: moving the
                // lead is the owner's call, `scenic lead give`).
                let (s, v) = if n_all == 1 { ("", "s") } else { ("s", "") };
                let remedy = match c.roomiest_agent().filter(|(_, l)| *l >= need) {
                    Some((m, l)) => format!("{m}'s limit is {:.1} GB: hand it the lead (`scenic lead give {m}`), or fix the step's memory", gb(l)),
                    None => "no member's limit fits it: a step whose memory grows with its target, for the owner to see to".to_string(),
                };
                h.why.push(Waiting { step: Some(w.step.clone()), what: steps::title(&w.step).into(), why: format!("{n_all} target{s} need{v} about {:.1} GB, past the lead's limit of {:.1} GB; this step runs only on the lead. {remedy} (`scenic pool floors --clear` once it's fixed)", gb(need), gb(limit)) });
            } else if n_all > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: steps::title(&w.step).into(), why: format!("{n_all} target{} need{} about {:.1} GB, more than any Mac in the pool has (the most: {:.1} GB): a step whose memory grows with its target, for the owner to see to (`scenic pool floors --clear` once it's fixed)", if n_all == 1 { "" } else { "s" }, if n_all == 1 { "s" } else { "" }, gb(need), gb(most)) });
            } else if n_here > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: steps::title(&w.step).into(), why: format!("{n_here} target{} need{} about {:.1} GB, past this Mac's limit of {:.1} GB: left to a Mac with room", if n_here == 1 { "" } else { "s" }, if n_here == 1 { "s" } else { "" }, gb(need), gb(limit)) });
            }
        }
        h
    }

    /// Slot `k`'s job for the status: its parts (the last it said), its progress, and from its pace
    /// the time it has left (kept for the forecast).
    fn job_view(&mut self, k: usize) -> Option<JobView> {
        let waiting = self.waiting_words(k);
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
            waiting,
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
            // (The lead keeps its canopy squares: every pass's areas read them again.)
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

    /// The lead's, once an agent, when no job runs here: the pass's answers its cache has, sent
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
        // (With the budgets on, alone among the steps keeping answers here by their lock, and each
        // archive sent under the answers' lease on the NAS: crate::answers::Local, Lease.)
        let leased = true;
        match std::thread::Builder::new().name("answers".into()).spawn(move || crate::answers::seed(&root, &cache, &scratch, leased)) {
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
            Ok(thread) => self.caches_task = Some(CachesTask { ask, toward: None, disk: false, began: Instant::now(), thread }),
            Err(e) => self.caches_failed(anyhow::anyhow!("its thread didn't start: {e}"), ask),
        }
    }

    /// Starts a freeing toward the owner's disk room target (`target`; `goal`, the room it frees
    /// toward, bytes) on a thread of its own; one that can't start is tried again with the next
    /// (`toward_due`).
    fn caches_start_toward(&mut self, target: u64, goal: u64, work: impl FnOnce() -> Result<room::Freed> + Send + 'static) {
        match std::thread::Builder::new().name("caches".into()).spawn(work) {
            Ok(thread) => self.caches_task = Some(CachesTask { ask: None, toward: Some((target, goal)), disk: false, began: Instant::now(), thread }),
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
        if t.disk {
            match r {
                Ok(f) => {
                    let note = format!("made room for the job the disk guard froze: {}; {} free", f.say(), room::size(self.disk_free()));
                    eprintln!("agent: {note}");
                    if f.bytes() > 0 {
                        self.note(crate::coord::history::Event { worker: Some(self.host.clone()), note, ..crate::coord::history::Event::new("caches") });
                    }
                    self.count_caches();
                }
                Err(e) => eprintln!("agent: making room for the job the disk guard froze: {e:#}"),
            }
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
    /// goes must be kept there), or the build has work left (`work_left`: by the lead's
    /// forecast, its own; a member's, the one in the lead's heartbeat on the NAS, read at most
    /// each minute, with no job of the lead's running, nor beside it).
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
                Some(j) => Some(format!("the lead runs a job ({})", j.what)),
                None => work_left(s.forecast.as_ref(), since),
            });
            self.heard = Some(Heard { read: Instant::now(), beat: st.map(|s| s.beat), left: left.flatten() });
        }
        match self.heard.as_ref().map(|h| (h.beat, &h.left)) {
            Some((Some(beat), _)) if now_s().saturating_sub(beat) > 600 => Some(format!("the lead hasn't been heard from for {} min", now_s().saturating_sub(beat) / 60)),
            Some((Some(_), left)) => left.clone(),
            _ => Some("the lead's status can't be read now".into()),
        }
    }

    /// Whether this Mac's caches are due a trim: never trimmed nor cleared, or a job has run here
    /// since the last.
    fn trim_due(&self) -> bool {
        let last = [&self.mem.trimmed, &self.mem.cleared].into_iter().flatten().map(|f| f.at).max();
        last.is_none_or(|t| self.mem.worked_at > t)
    }

    /// A job of `id` ran here: this Mac's caches may hold more, to trim once the build is done
    /// again (not after the daily ones: they read no cache); the lead's heartbeat read again.
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

    /// The lead's: the members' trims, clears and declined asks noted in the history as their
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
        // (Beside a job that waits on a worker, its cores idle: all of them.)
        let threads = match (k, self.o.helper) {
            (1.., _) if self.waiting_on(0).is_some() => cores,
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
        // (Its scratch its own again, if a job given back left it: not room-making's.)
        if let Some(d) = scratch_of(&spec) {
            room::given_back_drop(&self.o.home, &d);
        }
        // (No timings left from the slot's last job, for this one's hand-off to carry.)
        self.slots[k].timings_out = None;
        let log = self.o.home.join("logs").join(format!("{}.log", spec.id.replace([' ', '/'], "-")));
        eprintln!("agent: starting {}{} ({threads} threads)", spec.id, if k > 0 { " beside the first job" } else { "" });
        // The lead's jobs hand their saves off as every job does (no job writes the records,
        // docs/pool.md §7.3), note what their units cost, and may offer tasks to other workers
        // through the coordinator; a member's hand theirs off likewise (SCENIC_HANDOFF, in their
        // command).
        let mut env: Vec<(String, String)> = Vec::new();
        if !self.o.helper {
            if let Some(Held::Pooled { dir, .. }) = &self.slots[k].lease {
                env.push(("SCENIC_HANDOFF".into(), dir.to_string_lossy().into_owned()));
            }
            env.push(("SCENIC_COSTS".into(), self.costs_path(k).to_string_lossy().into_owned()));
            if let Some(c) = &self.coord {
                env.push(("SCENIC_COORD".into(), format!("http://127.0.0.1:{}", c.port())));
                env.push(("SCENIC_COORD_TOKEN".into(), c.job_token.clone()));
            }
            // A map update's step that reads the units: as they were when the map update began.
            let step = spec.record.as_ref().map(|w| w.step.as_str()).unwrap_or("");
            if let Some(r) = self.map_update.borrow().as_ref().filter(|r| !r.over && build::AS_OF_STEPS.contains(&step)) {
                env.push((crate::out::UNITS_AS_OF_ENV.into(), format!("{}#{}", self.o.home.join(MAP_UPDATE_FILE).display(), r.began)));
            }
        } else if let Some(b) = &self.broker {
            // (A member's job offers its tasks through this Mac's broker, docs/pool.md §8.)
            env.push(("SCENIC_COORD".into(), format!("http://127.0.0.1:{}", b.port())));
            env.push(("SCENIC_COORD_TOKEN".into(), b.job_token.clone()));
        }
        // Placement on: its tasks offered ahead, with their head starts (crate::agent::placement).
        if self.placement_on() {
            env.push((placement::AHEAD_ENV.into(), "1".into()));
        }
        // This Mac's cache, where a job keeps what it reads again and again (the coverage:
        // pipeline::coverage::Coverage::load).
        env.push((crate::coverage::CACHE_ENV.into(), self.o.home.join("cache").to_string_lossy().into_owned()));
        // Resuming on (crate::agent::resume): a pass complete by the records; a resumable job given
        // this member's hand-offs of its step its lead hasn't merged (laid over the snapshot as it
        // opens: crate::out::Out), and its progress noted as this Mac's.
        if self.resume_on() {
            env.push((resume::RESUME_ENV.into(), "1".into()));
            if resume::resumable(&spec.id) {
                if let Some(Held::Pooled { dir, .. }) = &self.slots[k].lease {
                    let step = step_of(&spec.id).unwrap_or_default();
                    let f = dir.join("unmerged.json");
                    crate::whole::write(&f, &serde_json::to_vec(&self.unmerged_for(&step)?)?)?;
                    env.push((resume::UNMERGED_ENV.into(), f.to_string_lossy().into_owned()));
                }
                self.progress_started(&spec);
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
        // Where it leaves its timings' record (crate::timings; a helper's leased job: its outbox
        // folder, in its command), taken in as it ends (`take_timings`).
        let timings = self.timings_path(k);
        std::fs::remove_file(&timings).ok();
        env.push((crate::timings::TIMINGS_ENV.into(), timings.to_string_lossy().into_owned()));
        env.push((crate::timings::JOB_ID_ENV.into(), spec.id.clone()));
        // Where its accessors count the bytes they add to the caches, afresh (docs/pool.md §7.7).
        let written = self.written_path(k);
        std::fs::remove_file(&written).ok();
        env.push((store::cachefile::WRITTEN_ENV.into(), written.to_string_lossy().into_owned()));
        // Where it says it waits on a worker holding its unit's tail, while it does (`waiting_on`),
        // afresh.
        let waiting = self.waiting_path(k);
        std::fs::remove_file(&waiting).ok();
        env.push((crate::offload::WAITING_ENV.into(), waiting.to_string_lossy().into_owned()));
        // Its connections out through the proxy (Wikimedia's metered by the pool's budget), this
        // Mac's own direct, and no other way out (the sandbox).
        let sandbox = match &self.proxy {
            Some(p) => {
                env.extend(p.env());
                Some(crate::budget::SANDBOX)
            }
            None => None,
        };
        let scratch =spec.cmd.iter().position(|a| a == "--scratch").and_then(|i| spec.cmd.get(i + 1)).map(PathBuf::from);
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (step, targets) = spec.record.as_ref().map_or((spec.id.split(' ').next().map(str::to_string), Vec::new()), |w| (Some(w.step.clone()), w.targets.iter().map(|t| t.0.clone()).collect()));
        let what = spec.what.clone();
        // (Its costs file afresh: a line an earlier job left, a target begun and never ended, would
        // name a target under way that isn't, crate::agent::memguard.)
        let costs = self.slot_costs(k);
        std::fs::remove_file(&costs).ok();
        // (A job of no targets of its own, the OSM pass, GC, a catalog: measured by its id, so it's
        // placed by measure as any, docs/pool.md §7.7.)
        let own_key = match spec.record.as_ref() {
            None => Some(own_cost_key(&spec.id)),
            _ => None,
        };
        let (w_step, w_targets) = (step.clone().unwrap_or_default(), targets.clone());
        self.slots[k].running = Some(Running::start(spec, threads, &env, log, &self.record_path(k), Some(&done), sandbox)?);
        self.slots[k].beaten = None;
        if let Some(pgid) = self.slots[k].running.as_ref().map(|r| r.pgid) {
            self.sampler.watch(k, memguard::Watch { pgid, costs, done: done.clone(), step: w_step, targets: w_targets, own_key, written, scratch, ..Default::default() });
        }
        self.note(crate::coord::history::Event { worker: Some(self.worker_of(k)), step, targets, what, ..crate::coord::history::Event::new("start") });
        Ok(())
    }

    /// Notes what happened in the build's history (the lead's coordinator's: a member's jobs
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

    /// Notes the lead's conditions as they change (mains or battery, the NAS, home or away, a
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
            if let Some(unit) = id.strip_prefix("inputs ").and_then(|r| r.strip_suffix(" forget")) {
                self.inputs.forget.borrow_mut().remove(unit);
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

    /// Whether this agent keeps the build's map updates (and its history): the one that runs its jobs,
    /// not a dry run beside it (planning only: its map updates are its own, in memory).
    fn keeps_map_updates(&self) -> bool {
        self._lock.is_some() && !self.o.dry_run
    }

    /// A map update begun (build::Plan::begins, its `began` set): written to its file, which its jobs read,
    /// before any of them starts.
    fn keep_map_update(&self, mut r: build::MapUpdate) -> Result<()> {
        // (When the low zooms were last made: this one's beginning if it makes them, else the
        // map update before's.)
        r.low_at = if r.low { Some(r.began) } else { self.map_update.borrow().as_ref().and_then(|p| p.low_at) };
        if self.keeps_map_updates() {
            write_map_update(&self.o.home, &r)?;
            self.note(crate::coord::history::Event { targets: r.regions.clone(), note: if r.last { "the last".into() } else { String::new() }, ..crate::coord::history::Event::new("map-update") });
        }
        eprintln!("agent: a map update begins, publishing {} region(s){}", r.regions.len(), if r.last { " (the last)" } else { "" });
        *self.map_update.borrow_mut() = Some(r);
        Ok(())
    }

    /// The decision on the next map update (build::UpdateDecision) kept for the status, its gap's basis
    /// and the pause said, and logged when it or its reasons change.
    fn record_update(&self, mut u: build::UpdateDecision, per_region: bool) {
        u.gap_basis = match (per_region, self.update_measured.get()) {
            (false, _) => "the hour: a map update per region is off".into(),
            (true, None) => "the hour: no map update measured yet".into(),
            (true, Some(s)) if u.gap_s >= build::PUBLISH_EVERY_S => format!("the hour: twice the measured {s:.0} s is more"),
            (true, Some(s)) => format!("twice the last map updates' measured {s:.0} s"),
        };
        if let Some(p) = &self.pause {
            u.why = format!("{} (but {})", u.why, p.why());
        }
        let key = u.key() + &self.pause.is_some().to_string();
        if *self.update_logged.borrow() != key {
            eprintln!("agent: {}", u.line());
            *self.update_logged.borrow_mut() = key;
        }
        *self.update_decision.borrow_mut() = Some(u);
    }

    /// The map update under way told to this Mac's coordinator, with the plan's offers made from the
    /// same map update (every step to every member: a map update's step is granted only with it).
    fn offer_update(&self, c: &crate::coord::Coordinator) {
        let map_update = self.map_update.borrow().as_ref().filter(|r| !r.over && self.every_on()).map(|r| (r.began, r.units.clone()));
        if map_update.as_ref().map(|r| r.0) != c.map_update_began() {
            c.set_map_update(map_update.map(|(began, units)| crate::coord::AsOf { began, units }));
        }
    }

    /// The map update under way is over (its catalog made, or nothing left it would publish): kept
    /// without its units, for when it began.
    fn end_update(&self) {
        // (Its units no longer granted with a map update's step.)
        if let Some(c) = &self.coord {
            c.set_map_update(None);
        }
        let mut kept = self.map_update.borrow_mut();
        let Some(r) = kept.as_mut().filter(|r| !r.over) else { return };
        r.over = true;
        r.units.clear();
        if self.keeps_map_updates() {
            if let Err(e) = write_map_update(&self.o.home, r) {
                eprintln!("agent: noting the map update over: {e:#}");
            }
        }
        eprintln!("agent: the map update that began {} min ago is over", now_s().saturating_sub(r.began) / 60);
    }

    /// The timetables' report, read again when the records name another (a small file).
    fn note_timetables(&self, root: &Path, manifest: &BTreeMap<String, String>) {
        let Some(c) = manifest.get(crate::timetables::REPORT) else { return };
        if self.timetables.borrow().as_ref().is_some_and(|t| &t.0 == c) {
            return;
        }
        if let Some(v) = std::fs::read(root.join(c)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
            *self.timetables.borrow_mut() = Some((c.clone(), crate::timetables::View::of(&v)));
        }
    }

    /// The work there is, in order (docs/plan.md §8, Order).
    /// A digest of what an idle agent's plan depends on, read cheaply: the build's records (the
    /// manifest, the jobs' keys, the hand-offs waiting), the gate's view of the inputs, the pool
    /// (membership, role, gates), its mode and switch, and the pause. Equal digests: the plan would
    /// come out the same but for what `IDLE_REPLAN` covers (time, disk room, planets newly on the NAS).
    fn plan_seen(&self, root: &Path) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let stat = |h: &mut std::collections::hash_map::DefaultHasher, p: &Path| {
            let m = std::fs::metadata(p).ok();
            m.as_ref().map(|m| (m.len(), m.modified().ok())).hash(h);
        };
        for f in ["state/build/manifest.json", "state/build/jobs.json"] {
            stat(&mut h, &root.join(f));
        }
        for b in self.handoff_bases(root) {
            stat(&mut h, &b);
            let mut hosts: Vec<PathBuf> = std::fs::read_dir(&b).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            hosts.sort();
            for d in hosts {
                d.hash(&mut h);
                stat(&mut h, &d);
            }
        }
        serde_json::to_string(&*self.inputs.view.borrow()).unwrap_or_default().hash(&mut h);
        if let Some(p) = &self.pool_seen {
            (&p.member, p.role, &p.members).hash(&mut h);
            serde_json::to_string(&p.gates).unwrap_or_default().hash(&mut h);
        }
        serde_json::to_string(&self.pause).unwrap_or_default().hash(&mut h);
        // (Faster publishing's switches, the hold on catalogs, the catalogs' folders.)
        for f in [publish::UPDATE_PER_REGION, publish::LOW_ZOOM_LATER, publish::UPDATE_SHARE, publish::DELTA_CATALOG, publish::NEAR_SLOPE, "inputs/hold-catalog", "catalog", "catalog-held"] {
            stat(&mut h, &root.join(f));
        }
        // (The coordinator's leases: one lapsed, given back or taken frees or holds targets.)
        self.coord.as_ref().map(|c| c.lease_gen()).hash(&mut h);
        self.newer_app().hash(&mut h);
        // (The pool's switches as this agent holds them.)
        (self.resume, self.placement, self.brokers, self.slots_on, self.guard_on).hash(&mut h);
        // (Never 0: that's "not read".)
        h.finish() | 1
    }

    /// When the plan's timed waits end (epoch seconds, past `now`), the earliest: a job's failure or
    /// memory guard wait, room-making's ten minutes, the regions' edit hold, the next map update's
    /// gap. An idle agent plans again then (`plan_due`).
    fn replan_at(&self, plan: &[JobSpec], now: u64) -> Option<u64> {
        let mut at: Vec<u64> = Vec::new();
        for spec in plan {
            at.extend(self.guard_backoff.get(&spec.id).map(|(u, _)| *u));
            at.extend(self.mem.retry.get(&spec.id).map(|(_, u)| *u));
        }
        if let Some((_, _, t)) = self.floor_short {
            at.push(ends_in(now, Duration::from_secs(600).saturating_sub(t.elapsed())));
        }
        if let Some((_, left)) = edit_held(self.edited_at.get(), std::time::SystemTime::now()) {
            at.push(ends_in(now, left));
        }
        if let Some(u) = self.update_decision.borrow().as_ref().filter(|u| u.left_s > 0) {
            at.push(now + u.left_s);
        }
        // (The budgets' proxy not started: tried again each loop, `tend_proxy`; the plan again each
        // minute, not each loop, should it keep failing.)
        if self.proxy.is_none() && !self.o.dry_run && !plan.is_empty() {
            at.push(now + PROXY_REPLAN.as_secs());
        }
        earliest_after(at, now)
    }

    fn plan(&self, root: &Path, c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let jobs = self.plan_unchecked(root, c, waiting);
        // (In the tests, every planned command checked against its program's table, crate::cli.)
        #[cfg(test)]
        for j in &jobs {
            if let Some(Err(e)) = crate::cli::tables::check_command(&j.cmd) {
                panic!("{e}");
            }
        }
        jobs
    }

    fn plan_unchecked(&self, root: &Path, _c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        let mut out = Vec::new();
        let me = self.o.bin.join("scenic");
        let s = |p: &Path| p.to_string_lossy().into_owned();

        // 1. The OSM pass, when the NAS holds a newer planet than the last complete pass.
        let have = self.latest_pass(root);
        let newer = crate::osmpass::newer_planet(root, have.as_deref()).ok().flatten();
        // (Resuming on: held for, and offered to, the member whose Mac has its progress.)
        let held = self.resume_hold(root, newer.as_ref());
        if let Some((planet, date)) = newer.clone() {
            let what = format!("Reading the OpenStreetMap planet of {date}");
            let scratch = self.o.home.join("scratch").join(format!("osm-{date}"));
            let jar = root.join("sources/basemap/planetiler.jar");
            // The pack cache is cleared when the pass starts (it refills from the mirror or the NAS):
            // its space counts as free, and so does the cheap caches' (room::make_room frees it).
            let pack_cache = self.o.home.join("cache").join("base");
            let mut free = self.disk_free() + dir_bytes(&pack_cache);
            if free < PASS_SPACE {
                free += room::cheap_bytes(&self.o.home.join("cache"), Some(&root.join("sources")));
            }
            let started = scratch.exists();
            if let Some(w) = held {
                waiting.push(w);
            } else if !jar.exists() {
                waiting.push(Waiting { step: Some("osm-pass".into()), what, why: "sources/basemap/planetiler.jar is missing on the NAS".into() });
            } else if !started && free < PASS_SPACE {
                waiting.push(Waiting { step: Some("osm-pass".into()), what, why: format!("needs {} GB free on this Mac ({} GB free)", PASS_SPACE >> 30, free >> 30) });
            } else {
                out.push(JobSpec { id: format!("osm-pass {date}"), what, cmd: self.osm_pass_cmd(root, &planet, &date), needs: Needs { nas: true }, restart_after_sleep: true, record: None });
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
                what: steps::title("backup").into(),
                cmd: vec![s(&me), "backup".into(), "--root".into(), s(root), "--local".into(), s(&self.o.home.join("backups"))],
                needs: Needs { nas: true },
                restart_after_sleep: true,
                record: None,
            });
        }
        // (Not while a map update is under way: the units it reads as they were may be gone from the
        // manifest, and in no catalog yet.)
        if self.due("gc", Duration::from_secs(86400)) && self.map_update.borrow().as_ref().is_none_or(|r| r.over) {
            out.push(JobSpec {
                id: "gc".into(),
                what: steps::title("gc").into(),
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
        let mut held = Holds::default();
        let jobs = self.region_jobs(root, pass, newer, waiting, &mut held);
        let jobs = hold_for_sources(jobs, held.sources, waiting);
        hold_for_lakefill(jobs, held.lakefill, waiting)
    }

    /// `region_work`'s jobs before the holds: `held.lakefill` set when the pass's lake fill holds
    /// the jobs that read it (`hold_for_lakefill`), `held.sources` while no version of the source
    /// declarations is accepted (`hold_for_sources`).
    fn region_jobs(&self, root: &Path, pass: Option<&str>, newer: Option<&str>, waiting: &mut Vec<Waiting>, held: &mut Holds) -> Vec<JobSpec> {
        // (The records unreadable now: nothing planned until they are, rather than everything again.)
        let (manifest, keys): (BTreeMap<String, String>, build::Keys) = match crate::out::read_record(&root.join("state/build/manifest.json")).and_then(|m| Ok((m, self.planning_keys(root)?))) {
            Ok(r) => r,
            Err(e) => {
                waiting.push(Waiting { step: None, what: "Building".into(), why: format!("the build's records can't be read now: {e:#}") });
                return Vec::new();
            }
        };
        // (Once a readers' switch is recorded, an app older than the one it was recorded on reads
        // the wrong regions: it builds nothing, and says so.)
        let regions_app = crate::inputs::regions::read::switch_app(&manifest, &|n| std::fs::read(root.join(n)).with_context(|| format!("read {n}"))).map_err(|e| format!("{e:#}"));
        // (And the coordinator grants nothing to an app older than it, a page's included.)
        if let (Some(c), Ok(min)) = (&self.coord, &regions_app) {
            c.set_min_app(min.clone());
        }
        if let Err(why) = crate::inputs::regions::read::may_lead(&self.app, &regions_app) {
            waiting.push(Waiting { step: None, what: "All building".into(), why: format!("this Mac can't build now: {why}") });
            return Vec::new();
        }
        // (The regions from the same records as the coverage.)
        let (recipes, unread): (Vec<recipes::Recipe>, Vec<(String, String)>) = match crate::inputs::regions::read::source(root, &manifest).and_then(|a| crate::inputs::regions::read::recipes(&a)) {
            Ok(r) => (r, Vec::new()),
            Err(e) => (Vec::new(), vec![("regions".into(), format!("{e:#}"))]),
        };
        if let Some((_, why)) = unread.iter().find(|(f, _)| f == "regions") {
            waiting.push(Waiting { step: None, what: "Building the regions".into(), why: format!("the regions can't be read now: {why}") });
        }
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let build_bin = s(&self.o.bin.join("scenic-build"));
        let mut jobs: Vec<JobSpec> = self.gate_work(root, &manifest, &keys, waiting);
        held.sources = !crate::inputs::sources::accepted(&manifest);
        // (A job's name is its step's title, from the steps table: `step` is the scenic-build
        // command, the step by its id's.)
        let job = |id: String, step: &str, extra: Vec<String>, record: Option<build::Work>| {
            let scratch = self.o.home.join("scratch").join(step);
            let mut cmd = vec![build_bin.clone(), step.to_string(), "--root".into(), s(root), "--scratch".into(), s(&scratch)];
            cmd.extend(extra);
            let what = steps::title(step_of(&id).as_deref().unwrap_or(step)).to_string();
            // (Every job needs the NAS; none waits for power or home, the owner's choice: the pass's
            // whole-planet reads go over Tailscale when away, slowly.)
            JobSpec { id, what, cmd, needs: Needs { nas: true }, restart_after_sleep: true, record }
        };
        // Per pass, worldwide: the sets it lacks in their current filters (a set added or changed
        // since it ran), the hiking routes' ends, AWS's z8 (once), Overture's buildings (once per
        // release), the summits, the labels, the water.
        if let Some(date) = pass {
            let p = vec!["--pass".to_string(), date.to_string()];
            if !crate::osmpass::SETS.iter().all(|st| manifest.contains_key(&crate::osmpass::set_name(date, st.0))) {
                jobs.push(job(format!("pass-sets {date}"), "pass-sets", p.clone(), None));
            }
            // The pass's outlines (their own step: made again when their assembly changes, not at
            // the next pass). While they have work, nothing else is planned (the regions' coverage,
            // the languages spoken, the lake fill, the rail feeds and the catalog read them), so
            // nothing is made from the outlines they replace; unless the job has failed
            // `build::OUTLINES_HOLDS` times in a row (counted on this Mac) and the pass has
            // outlines: the rest then goes on with those.
            if let Some(w) = build::outlines_work(date, &manifest, &keys) {
                let failures = self.mem.retry.get(&format!("outlines {date}")).map_or(0, |r| r.0);
                let made = manifest.contains_key(&format!("sources/osm/{date}/outlines"));
                jobs.push(job(format!("outlines {date}"), "outlines", p.clone(), Some(w)));
                if build::outlines_hold(made, failures) {
                    waiting.push(Waiting { step: None, what: "Building".into(), why: "waits for the pass's outlines to be assembled (the outlines job)".into() });
                    return jobs;
                }
                waiting.push(Waiting { step: Some("outlines".into()), what: steps::title("outlines").into(), why: format!("failed {failures} times in a row: the rest goes on with the outlines the pass has meanwhile") });
            }
            if let Some(w) = build::trailends_work(date, &manifest, &keys) {
                jobs.push(job(format!("trailends {date}"), "trailends", p.clone(), Some(w)));
            }
            let reach_job = |w: build::Work| job(format!("reach {date}"), "reach", p.clone(), Some(w));
            if let Some(w) = build::reach_work(date, &manifest, &keys) {
                jobs.push(reach_job(w));
            } else if let Err(why) = self.current_reach(root, &manifest, &keys, date) {
                match why {
                    // Made, but it doesn't decode: made again.
                    crate::reach::LoadError::Bad(e) => {
                        waiting.push(Waiting { step: None, what: steps::stage_title("roads").into(), why: format!("the pass's reaches don't read ({e}); making them again") });
                        if let Some(k) = build::reach_key(date, &manifest) {
                            jobs.push(reach_job(build::Work { step: "reach".into(), targets: vec![("reach".into(), k)] }));
                        }
                    }
                    crate::reach::LoadError::Io(e) => waiting.push(Waiting { step: None, what: steps::stage_title("roads").into(), why: format!("the pass's reaches can't be read now: {e}") }),
                }
            }
            if !manifest.contains_key(&crate::terrain_z8::logical()) {
                jobs.push(job("terrain-z8".into(), "terrain-z8", vec!["--raw".into(), s(&self.o.home.join("cache").join("aws-terrarium"))], None));
            }
            // (Of the declared release: none before the source declarations are accepted.)
            let release = crate::buildtiles::release_of(root, &manifest).ok();
            if let Some(release) = release.as_deref().filter(|r| !manifest.contains_key(&crate::buildtiles::index_logical(r))) {
                jobs.push(job(format!("buildings {release}"), "buildings", vec!["--dem".into(), s(&self.o.bin.join("dem"))], None));
            } else if let Ok(rd) = std::fs::read_dir(self.o.home.join("scratch/buildings")) {
                // Made: the scan's parts (~40 GB) go, even from a run stopped before it removed them.
                for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("parts-")) {
                    std::fs::remove_dir_all(e.path()).ok();
                }
            }
            if let Some(w) = build::summits_work(date, &manifest, &keys) {
                jobs.push(job(format!("summits {date}"), "summits", [p.clone(), vec!["--cache".into(), s(&self.o.home.join("cache"))]].concat(), Some(w)));
            }
            if let Some(w) = build::labels_work(date, &manifest, &keys) {
                jobs.push(job(format!("labels {date}"), "labels", [p.clone(), vec!["--dem".into(), s(&self.o.bin.join("dem"))]].concat(), Some(w)));
            }
            let table = manifest.get(&crate::inputs::logical(crate::inputs::languages::UNIT));
            if let (Some(w), Some(table)) = (build::spoken_work(date, &manifest, &keys), table) {
                // (The languages' accepted version the key names.)
                jobs.push(job(format!("spoken {date}"), "spoken", [p.clone(), vec!["--languages-index".to_string(), table.clone()]].concat(), Some(w)));
            } else if table.is_none() && manifest.contains_key(&format!("sources/osm/{date}/outlines")) {
                waiting.push(Waiting { step: Some("spoken".into()), what: steps::title("spoken").into(), why: "the languages' first accepted version (the input check takes inputs/languages/ in)".into() });
            }
            if !manifest.contains_key(&crate::inputs::logical(crate::inputs::heritage::UNIT)) {
                waiting.push(Waiting { step: Some("heritage-sites".into()), what: steps::title("heritage-sites").into(), why: "the heritage registers' first accepted version (the input check takes inputs/heritage/ in)".into() });
            }
            // (The lakes filled from Canada's own data first: while the lakefill job has work, unless
            // it has failed LAKEFILL_HOLDS times in a row (counted on this Mac, from nought again with
            // a new app), the jobs that read the fill wait (`hold_for_lakefill`: the water, the
            // terrain's water digests, the catalog), so the pass's water, terrain and map all have
            // the new fill, or after that many failures all go without it. While the source's folder
            // can't be read, or while its source isn't declared, they wait too.)
            match crate::lakefill::run::planned_source(root, &manifest) {
                Err(e) => {
                    held.lakefill = true;
                    waiting.push(Waiting { step: Some("lakefill".into()), what: steps::title("lakefill").into(), why: e });
                }
                Ok(source) => {
                    let lakefill = build::lakefill_work(date, &manifest, &keys, source.as_deref());
                    let failures = self.mem.retry.get(&format!("lakefill {date}")).map_or(0, |r| r.0);
                    held.lakefill = build::water_waits_for_lakefill(lakefill.is_some(), failures);
                    if let Some(w) = lakefill {
                        if failures >= build::LAKEFILL_HOLDS {
                            waiting.push(Waiting { step: Some("lakefill".into()), what: steps::title("lakefill").into(), why: format!("failed {failures} times in a row: the water, terrain and map go without this pass's fill meanwhile") });
                        }
                        jobs.push(job(format!("lakefill {date}"), "lakefill", p.clone(), Some(w)));
                    }
                }
            }
            if let Some(w) = build::water_work(date, &manifest, &keys) {
                jobs.push(job(format!("water {date}"), "water", p.clone(), Some(w)));
            }
        }
        // After each catalog the map serves: the names to translate and descriptions to write.
        // (From the translations' and descriptions' accepted versions, which the agent names: the
        // job reads them through the accessor, never the drop boxes.)
        let served = served_catalog(&root.join("catalog")).map(|c| c.n);
        if let Some(w) = build::names_todo_work(served, &manifest, &keys) {
            let n = w.targets[0].1.clone();
            let mut args = Vec::new();
            for unit in [crate::inputs::translations::UNIT, crate::inputs::descriptions::UNIT] {
                args.extend([format!("--{unit}-index"), manifest.get(&crate::inputs::logical(unit)).cloned().unwrap_or_default()]);
            }
            jobs.push(job(format!("names-todo {n}"), "names-todo", args, Some(w)));
        } else if served.is_some() && [crate::inputs::translations::UNIT, crate::inputs::descriptions::UNIT].iter().any(|u| !manifest.contains_key(&crate::inputs::logical(u))) {
            waiting.push(Waiting { step: Some("names-todo".into()), what: steps::title("names-todo").into(), why: "the translations' or the descriptions' first accepted version (the input check takes them in)".into() });
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
        // The timetables' to-do list and report, after each rail and ferries run (lead-only: it
        // writes into the static timetables' drop box's todo/, never read as an input).
        if let Some(w) = build::timetables_todo_work(cov, &manifest, &keys) {
            jobs.push(job("timetables-todo".into(), "timetables-todo", vec!["--pass".to_string(), date.to_string()], Some(w)));
        }
        self.note_timetables(root, &manifest);
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
        // (The key names the timetable feeds' declarations fetch with, for rail-feeds' key: "?" while
        // their index can't be read, and rail-feeds waits.)
        if let Some(ix) = manifest.get(&crate::inputs::logical(crate::inputs::feeds::UNIT)) {
            inputs.insert("feed-keys".into(), crate::inputs::feeds::index_key_names(root, ix).map(|k| k.join(",")).unwrap_or_else(|_| "?".into()));
        }
        let held = root.join("inputs/hold-catalog").exists();
        let reach = self.current_reach(root, &manifest, &done, date).ok().flatten();
        if !manifest.contains_key(&crate::inputs::logical(crate::inputs::feeds::UNIT)) {
            waiting.push(Waiting { step: None, what: steps::stage_title("trains").into(), why: "the timetable feeds' declarations' first accepted version (inputs/timetables/feeds/: the input check takes them in)".into() });
        } else if inputs.get("keys").map(String::as_str) == Some("?") {
            waiting.push(Waiting { step: None, what: steps::stage_title("trains").into(), why: "inputs/keys.env can't be read now".into() });
        } else if inputs.get("feed-keys").map(String::as_str) == Some("?") {
            waiting.push(Waiting { step: None, what: steps::stage_title("trains").into(), why: "the timetable feeds' accepted declarations can't be read now".into() });
        }
        // (The ferries count the ferry feeds: without their list, rail-feeds' to make, they wait.)
        if manifest.contains_key(&crate::osmpass::set_name(date, "ferries")) && !manifest.contains_key(crate::rail::FERRY_FEEDS) {
            waiting.push(Waiting { step: Some("ferries".into()), what: steps::title("ferries").into(), why: "the ferry feeds' list (rail-feeds makes it once the timetable feeds' declarations are accepted)".into() });
        }
        // (The trains and the ferries count the static files too: without their accepted version,
        // they wait, rather than be counted without them.)
        if !manifest.contains_key(&crate::inputs::logical(crate::inputs::gtfs::UNIT)) {
            let why = "the static timetables' first accepted version (inputs/timetables/gtfs/: the input check takes them in)";
            if manifest.contains_key(crate::rail::FEEDS) {
                waiting.push(Waiting { step: Some("rail".into()), what: steps::stage_title("trains").into(), why: why.into() });
            }
            if manifest.contains_key(&crate::osmpass::set_name(date, "ferries")) {
                waiting.push(Waiting { step: Some("ferries".into()), what: steps::title("ferries").into(), why: why.into() });
            }
        }
        if inputs.get("bld-release").map(String::as_str) == Some("?") {
            waiting.push(Waiting { step: None, what: steps::stage_title("buildings").into(), why: "the 3D buildings' sources' indexes on the NAS can't be read now".into() });
        }
        // The regions the map's catalog has (or the held one's, when catalogs are held for review)
        // and how long ago it went out: what the plan publishes regions by.
        // (The held ones, once there are any: the first goes by what's served.)
        let held_dir = root.join("catalog-held");
        let dir = if held && store::catalog::list(&held_dir).is_ok_and(|ns| !ns.is_empty()) { held_dir } else { root.join("catalog") };
        let (on_map, since_publish) = self.on_map(&dir, &recipes);
        // (The map's: the served catalog's, held ones aside.)
        self.catalog_seen.set(served_catalog(&root.join("catalog")));
        // The map update under way, if one is; the next an hour after the last began (before the agent
        // kept map updates, after the last catalog went out).
        let since_last = self.map_update.borrow().as_ref().map(|r| now_s().saturating_sub(r.began)).or(since_publish);
        // (Faster publishing's switches: crate::agent::publish; all off, the map updates as before.)
        let sw = publish::Switches::read(root, self.publishing.get());
        if sw != self.publishing.get() {
            eprintln!("agent: faster publishing: {sw:?}");
        }
        self.publishing.set(sw);
        let way = build::Publishing {
            gap: sw.per_region.then(|| publish::update_gap(self.update_measured.get())),
            low_later: sw.low_later,
            since_low: self.map_update.borrow().as_ref().and_then(|r| r.low_at).map(|t| now_s().saturating_sub(t)),
            near: sw.near,
        };
        let mut planned = {
            let kept = self.map_update.borrow();
            let tiles = self.terrain_tiles(root, &manifest);
            // (The records as re-keyed: what they are once the loop has re-keyed them, as a dry run,
            // or a loop that couldn't take the build lock, plans too.)
            let times = rekey::FileTimes::new(root);
            rekey::as_read(&mut done, cov, date, &manifest, reach.as_deref(), &inputs, &tiles, &times);
            build::plan(&cov, date, &manifest, &done, &inputs, reach.as_deref(), &tiles, build::MapUpdates { each: &covs.each, on_map: &on_map, since_last, current: kept.as_ref().filter(|r| !r.over), held, way })
        };
        // The units held until the heritage registers are first accepted: said, not a silent stall.
        if let Some(why) = build::heritage_awaits_acceptance(cov, date, &manifest, &inputs) {
            waiting.push(Waiting { step: Some("heritage-sites".into()), what: steps::stage_title("sites").into(), why: format!("the regions' units wait: {why}") });
        }
        // Units whose terrain can't be worked out now (a pack's index unread): they wait for it.
        // Or the declared elevation datasets: before their switch (docs/inputs.md §6.8), or while
        // their accepted version can't be read.
        if !planned.unknown.is_empty() {
            let why = match self.tiles.borrow().elevation(&manifest) {
                Ok(None) => "the records don't say the build reads the declared elevation datasets yet (their switch, docs/inputs.md §6.8)".to_string(),
                Err(e) => e.0,
                Ok(Some(_)) => {
                    let why = self.tiles.borrow().unread().next().map(|(c, e)| format!(": {c}, {e}")).unwrap_or_default();
                    format!("the terrain's indexes can't be read now for {} of them{why}", planned.unknown.len())
                }
            };
            waiting.push(Waiting { step: Some("unit".into()), what: steps::stage_title("roads").into(), why });
        }
        let edit_hold = edit_held(self.edited_at.get(), std::time::SystemTime::now());
        // (Hand-offs of work done not yet merged: counted as built, their files not yet in the
        // manifest.)
        let unmerged = self.handoff_bases(root).iter().any(|b| crate::handoff::waiting_in(b).map(|w| w.iter().any(|(_, h)| h.done.is_some())).unwrap_or(false));
        if planned.ends {
            self.end_update();
        }
        // A map update begins: kept from now on, its work planned as such already. Not while a helper's
        // work waits to be merged (the units it counts as built would be missing from the map update's),
        // nor while an edit is held: its own steps wait meanwhile.
        if let Some(mut r) = planned.begins.take() {
            let not_now = if unmerged {
                Some("waits for a helper's work to be merged".to_string())
            } else if edit_hold.is_some() {
                Some("waits for the regions' edits to settle".to_string())
            } else {
                r.began = now_s();
                self.keep_map_update(r).err().map(|e| format!("can't keep the map update now ({e:#})"))
            };
            if let Some(why) = &not_now {
                planned.update.why = format!("{} (but it {why})", planned.update.why);
            }
            if let Some(why) = not_now {
                waiting.push(Waiting { step: Some("catalog".into()), what: steps::stage_title("publish").into(), why: format!("the next map update {why}") });
                planned.work.retain(|w| !build::AS_OF_STEPS.contains(&w.step.as_str()) && w.step != "prune");
            }
        }
        self.record_update(std::mem::take(&mut planned.update), sw.per_region);
        // (The map update under way's chain left, and when it began, for the forecast: one begun now,
        // now.)
        let update_left = std::mem::take(&mut planned.update_left);
        let since_last = self.map_update.borrow().as_ref().map(|r| now_s().saturating_sub(r.began)).or(since_last);
        let ready = planned.ready;
        *self.ready.borrow_mut() = ready.clone();
        // (As the catalog's `--ready`: each with the outline it was built with.)
        let ready_arg: String = ready.iter().map(|id| match recipes.iter().find(|r| &r.id == id) {
            Some(r) => format!("{id}={}", recipes::outline_digest(&r.outline)),
            None => id.clone(),
        }).collect::<Vec<_>>().join(",");
        let mut plan = planned.work;
        if let Some(why) = credits_unread.filter(|_| plan.iter().any(|w| w.step == "catalog")) {
            waiting.push(Waiting { step: Some("catalog".into()), what: steps::stage_title("publish").into(), why: format!("the inputs' descriptions (their credits) can't be read now: {why}") });
            plan.retain(|w| w.step != "catalog");
        }
        // Just edited: the regions' work waits a while for more edits (each would build again what
        // the last started); what runs carries on.
        if let Some((age, left)) = edit_hold.filter(|_| !plan.is_empty()) {
            let left = left.as_secs().div_ceil(60);
            waiting.push(Waiting { step: None, what: "Building the regions".into(), why: format!("a region's recipe or outline changed {} min ago: their work starts in {left} min, after any more edits", age.as_secs() / 60) });
            plan.clear();
        }
        // A map update's catalog waits while another worker builds its regions' slope or tree cover (it
        // would go out without them, and they'd wait an hour), and while a helper's hand-offs wait
        // to be merged (its areas counted as built, their files not yet in the manifest).
        // (The coordinator's leases alone say what another worker builds.)
        let every_on = self.every_on();
        let held_by_others = |step: &str, t: &str| self.coord.as_ref().is_some_and(|c| c.held(step).contains(t));
        let held_wait = planned.publish_waits.iter().find(|(st, t)| held_by_others(st, t));
        if let Some((st, t)) = held_wait.filter(|_| plan.iter().any(|w| w.step == "catalog")) {
            let mine = self.slots.iter().filter_map(|s| s.running.as_ref()).any(|r| r.spec.record.as_ref().is_some_and(|w| w.step == *st && w.targets.iter().any(|x| x.0 == *t)));
            let who = if mine { "this Mac's job" } else { "another worker" };
            waiting.push(Waiting { step: Some("catalog".into()), what: steps::stage_title("publish").into(), why: format!("waits for {who} building the {} of {t}", if st.starts_with("slope") { "slope" } else { "tree cover" }) });
            plan.retain(|w| w.step != "catalog");
        } else if unmerged && plan.iter().any(|w| w.step == "catalog") {
            waiting.push(Waiting { step: Some("catalog".into()), what: steps::stage_title("publish").into(), why: "waits for a helper's work to be merged".into() });
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
        // What other workers hold leases of isn't planned here, whatever the switch (one switched off
        // while a member runs a job of a step only it offered).
        if let Some(c) = &self.coord {
            for w in plan.iter_mut().filter(|w| !steps::SHARED.contains(&w.step.as_str())) {
                let others = c.held_by_others(&w.step);
                w.targets.retain(|t| !others.contains(&t.0));
            }
        }
        // A cold start of a step another worker runs now: none of the lead's own beside it (every
        // step to every member: a cold start runs alone in the pool).
        if let (Some(c), true) = (&self.coord, every_on) {
            for w in plan.iter_mut().filter(|w| !w.targets.is_empty() && c.cold_under_way(&w.step)) {
                waiting.push(Waiting { step: Some(w.step.clone()), what: steps::title(&w.step).to_string(), why: "its first job, never measured, runs on another member now: its others wait for its measure".into() });
                w.targets.clear();
            }
        }
        // (Every step to every member: the plan's every step offered but the lead's own,
        // crate::agent::every.)
        for w in plan.iter_mut().filter(|w| steps::SHARED.contains(&w.step.as_str()) || (every_on && every::offered(&w.step))) {
            // What another worker builds now isn't planned here: what it leased from this Mac's
            // coordinator.
            let mut others = BTreeSet::new();
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
        // (The shared steps by their rank, then the others in plan order.)
        offers.sort_by_key(|o| steps::SHARED.iter().position(|s| *s == o.step).unwrap_or(usize::MAX));
        if let Some(c) = &self.coord {
            self.offer_update(c);
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
        // The forecast of the work left (crate::agent::forecast): the lead's, for the status (made
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
            before.extend(plan.iter().filter(|w| w.step == "heritage-sites").map(|w| job(format!("heritage-sites {date}"), "heritage-sites", Vec::new(), Some(w.clone()))));
            let chains = build::chains_left(cov, date, &manifest, &done, &inputs, reach.as_deref());
            // (A fault in it costs the status its forecast, never the agent.)
            let under_way = self.map_update.borrow().as_ref().filter(|r| !r.over).map(|r| (r.regions.clone(), r.last, update_left.clone()));
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
        // (A map update shared: the lead's own job of a map update's map tiles takes its share, the rest left
        // to the members asking: crate::agent::publish.)
        let share_on = self.publishing.get().share && self.every_on() && self.map_update.borrow().as_ref().is_some_and(|r| !r.over);
        let coord = self.coord.as_ref().filter(|_| share_on);
        let size = |step: &str, total: usize, n: usize| match coord.filter(|_| publish::SHARED_IN_UPDATE.contains(&step)) {
            Some(c) => publish::lead_share(total, n, c.members_asking(step)),
            None => n,
        };
        for (w, total) in batches_sized(plan, &size) {
            // Held for review: the catalog goes to catalog-held/ (no server reads it), once.
            if w.step == "catalog" && held {
                let k = w.targets.first().map(|t| t.1.clone()).unwrap_or_default();
                if done.catalog_held.as_deref() == Some(k.as_str()) {
                    waiting.push(Waiting { step: None, what: steps::stage_title("publish").into(), why: "held for review (inputs/hold-catalog); its catalog is in catalog-held/".into() });
                    // (Its map update's done: the held catalog is its.)
                    self.end_update();
                    continue;
                }
                let mut j = job("catalog-held".into(), "catalog", vec!["--held".into(), "--ready".into(), ready_arg.clone()], Some(build::Work { step: "catalog-held".into(), targets: vec![("catalog-held".into(), k)] }));
                j.needs = Needs { nas: true };
                jobs.push(j);
                continue;
            }
            let mut extra: Vec<String> = w.targets.iter().map(|t| t.0.clone()).filter(|t| every::names_target(t) && *t != w.step).collect();
            extra.extend(self.step_args(&w.step, date));
            // (The timetable feeds' declarations: the accepted version it was planned with.)
            if w.step == "rail-feeds" {
                extra.extend(["--feeds-index".to_string(), manifest.get(&crate::inputs::logical(crate::inputs::feeds::UNIT)).cloned().unwrap_or_default()]);
            }
            // (The static timetables: the accepted version the trains and the ferries were planned
            // with.)
            if w.step == "rail" || w.step == "ferries" {
                extra.extend(["--gtfs-index".to_string(), manifest.get(&crate::inputs::logical(crate::inputs::gtfs::UNIT)).cloned().unwrap_or_default()]);
            }
            // (The heritage registers' and matches' accepted versions the plan read: the job reads
            // them, never the drop boxes.)
            if w.step == "heritage-sites" {
                for unit in [crate::inputs::heritage::UNIT, crate::inputs::heritage_matches::UNIT] {
                    if let Some(i) = manifest.get(&crate::inputs::logical(unit)) {
                        extra.extend([format!("--{unit}-index"), i.clone()]);
                    }
                }
            }
            // (Tree cover pieces made again as they are, their mids made: expected the same.)
            let same = expect_same(&w, &done);
            if !same.is_empty() {
                extra.extend(["--expect-same".to_string(), same.join(",")]);
            }
            // (The regions a catalog records as built: the plan's.)
            if w.step == "catalog" {
                extra.extend(["--ready".to_string(), ready_arg.clone()]);
                // (Delta catalogs: crate::agent::publish.)
                if self.publishing.get().delta {
                    extra.push("--delta".into());
                }
            }
            let n = w.targets.len();
            // "Terrain: 3 tiles", or "Terrain: 8 of 480 tiles" for a batch (the checklist names the
            // steps alike: crate::agent::steps::said); the pieces made again for their mids say so.
            let mut what = steps::said(&w.step, n, Some(total));
            if same.len() == n && matches!(w.step.as_str(), "trees" | "terrain" | "slope") {
                what += ", their mids made and checked the same";
            }
            let id = format!("{} {}", w.step, w.targets.first().map(|t| t.0.as_str()).unwrap_or(""));
            let step = w.step.clone();
            let mut j = job(id, &step, extra, Some(w));
            j.what = what;
            jobs.push(j);
        }
        jobs
    }

    /// The forecast (crate::agent::forecast) of the work left: `before`, the lead's jobs before
    /// the regions'; `regions`, the plan's; `backfill`, the pieces whose mids it makes in
    /// idle time (build::Plan::backfill); `chains`, the roads', trains' and landmarks' work to come
    /// (build::chains_left); `blind`, why the work can't all be listed now. Each target's time (at
    /// the lead's pace: a time measured on a member over its speed) and memory as last measured,
    /// else its step's mean or a first guess (`first_secs`; `peak` for its memory); the helpers at
    /// their measured speed; each machine free once its job under way is done.
    #[allow(clippy::too_many_arguments)]
    fn forecast_now(&self, root: &Path, before: &[JobSpec], regions: &[build::RegionLeft], backfill: &[build::Work], chains: [Vec<build::Work>; 4], since_last: Option<u64>, under_way: Option<(Vec<String>, bool, Vec<build::Work>)>, blind: Option<String>, peak: &dyn Fn(&str, &str) -> u64) -> forecast::Forecast {
        use forecast::{Cost, Machine};
        let (costs, leased, events, mem) = match &self.coord {
            Some(c) => c.for_forecast(),
            None => Default::default(),
        };
        // Each worker's speed against the lead's, from the history (half until it's measured):
        // every one that measured a cost or did shared work, asleep or not (its costs are put at the
        // lead's pace by it); the members heard from lately are the machines.
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
        // (A time measured on a member, at the lead's pace; one measured here or by a page, as it is.)
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
        // The lead's own steps: what a target took here lately (measured), else a first guess.
        let mine = |step: &str, n: usize| -> Cost {
            let (each, known) = self.mem.step_secs.get(&secs_key(step)).map_or((first_secs(step), false), |&t| (t, true));
            Cost { secs: each * n.max(1) as f64, known, peak_mb: 0 }
        };
        let step_of = |id: &str| id.split(' ').next().unwrap_or("").to_string();
        // (Each job before the regions' is a step of its own; one running now is its slot's
        // running item below, its time left its slot's.)
        let before: Vec<forecast::Job> = before.iter().map(|j| (step_of(&j.id), j.id.clone(), mine(&step_of(&j.id), j.record.as_ref().map_or(1, |w| w.targets.len())))).collect();
        // A map update: as the last ones took (beginning to catalog), else the chain's steps'
        // times; the last map update, the roads' chain as it stands now, if more (none when it's done),
        // less what the map update under way still does (the roads' chain counts its work too, which
        // goes out with it).
        let [roads, rail, landmarks, buildings] = chains;
        let chain_s = |works: &[build::Work]| -> f64 { works.iter().map(|w| if steps::SHARED.contains(&w.step.as_str()) { w.targets.iter().map(|t| cost(&w.step, &t.0).secs).sum() } else { mine(&w.step, w.targets.len()).secs }).sum() };
        self.update_measured.set(forecast::update_secs(&events));
        let update_s = forecast::update_secs(&events).unwrap_or_else(|| ["prune", "roadunits", "stations", "ferries", "terrain-root", "slope-root", "catalog"].iter().map(|s| mine(s, 1).secs).sum::<f64>() + mine("pack", 8).secs + mine("lo", 2).secs);
        let ahead = under_way.as_ref().map_or(0.0, |u| chain_s(&u.2));
        let last_update_s = if roads.is_empty() { 0.0 } else { update_s.max(chain_s(&roads) - ahead) };
        // The trains' and the landmarks' chains from the start (their steps once what they read is
        // built: forecast::chain_deps), but the overlays (they read the built units) after the last
        // map update; and a catalog after it with what the chains made since.
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
        let mut machines = vec![Machine { name: self.host.clone(), speed: 1.0, measured: true, helper: false, second: false, equal: false, mem_mb: u64::MAX, busy_s: busy(0, 1.0), unit_extra_s }];
        // Its second job: what fits beside the first (a quarter of its memory, say); while the Mac's
        // in use, as it is now, its network work alone.
        if self.second_allowed() {
            let (speed, measured) = speeds.get(&second).copied().unwrap_or((0.8, false));
            let mem_mb = (self.mac.resources(&self.o.home, None, None, None).mem_gb * 256.0) as u64;
            machines.push(Machine { name: second.clone(), speed, measured, helper: false, second: true, equal: false, mem_mb, busy_s: busy(1, speed), unit_extra_s });
        }
        for h in &helpers {
            let (speed, measured) = speeds.get(&h.host).copied().unwrap_or((0.5, false));
            // (Its lease's targets at its pace, less the time since it took them, if longer than its
            // part's.)
            let lease_left = leased.iter().filter(|l| l.0 == h.host).map(|(_, step, ts, age)| ts.iter().map(|t| cost(step, t).secs).sum::<f64>() / speed - *age as f64).fold(0.0, f64::max);
            let eta = h.job.as_ref().map(|j| j.progress.as_ref().and_then(|p| p.eta_s).unwrap_or(600) as f64);
            let busy_s = eta.map_or(0.0, |e| e.max(lease_left).max(60.0));
            machines.push(Machine { name: h.host.clone(), speed, measured, helper: true, second: false, equal: false, mem_mb: mem.get(&h.host).copied().filter(|&m| m > 0).unwrap_or(6144), busy_s, unit_extra_s: 0.0 });
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
        for (worker, step, targets, _) in leased.iter().cloned() {
            if let Some(m) = machines.iter().position(|m| m.name == worker && m.helper) {
                running.extend(targets.into_iter().map(|t| ((step.clone(), t), m)));
            }
        }
        // (The map update under way: its chain left as its steps take, a minute at least.)
        let under_way = under_way.map(|(ids, last, left)| (ids, last, chain_s(&left).max(60.0)));
        // (The pieces' mids made in idle time, last.)
        let idle: Vec<forecast::Job> = backfill.iter().flat_map(|w| w.targets.iter().map(|(t, _)| (w.step.clone(), t.clone(), cost(&w.step, t)))).collect();
        // (A region's map update an hour after the last began, or a map update per region's gap.)
        let gap_s = if self.publishing.get().per_region { publish::update_gap(self.update_measured.get()) } else { build::PUBLISH_EVERY_S } as f64;
        let inp = forecast::Input { now: now_s(), before, regions, cost: &cost, update_s, last_update_s, blind, chains: chain_jobs, after, idle, since_last, gap_s, under_way, machines, running };
        let mut f = forecast::forecast(&inp);
        // (A slot's job waiting on a worker holding its unit's tail: said beside its machine.)
        f.waiting = (0..SLOTS).filter_map(|k| Some((self.worker_of(k), self.waiting_words(k)?))).collect();
        // In the pool, the forecast for equal machines beside it (docs/pool.md §12 4.12), reported
        // alone: each member's slots as lanes (this Mac's first: its map updates; its second job's
        // memory a quarter of the Mac's, as above), the pages as one (forecast::pool_lanes), each at
        // its speed measured against this Mac (placement::measured_speeds; one not measured, this
        // Mac's), busy with its leases' targets left.
        if let (Some(c), true) = (&self.coord, self.pool.is_some()) {
            let workers: Vec<forecast::PoolWorker> = c.workers().into_iter().map(|(n, w)| (n, w.kind == "web", w.mem_mb)).collect();
            let names: Vec<String> = workers.iter().map(|w| w.0.clone()).filter(|n| *n != self.host).collect();
            let measured = placement::measured_speeds(&events, &self.host, &names);
            let lead = Machine { name: self.host.clone(), speed: 1.0, measured: true, helper: false, second: false, equal: false, mem_mb: u64::MAX, busy_s: busy(0, 1.0), unit_extra_s };
            let second = self.second_allowed().then(|| {
                let sp = measured.get(&second).copied().unwrap_or(1.0);
                let mem_mb = (self.mac.resources(&self.o.home, None, None, None).mem_gb * 256.0) as u64;
                Machine { name: second.clone(), speed: sp, measured: measured.contains_key(&second), helper: false, second: false, equal: false, mem_mb, busy_s: busy(1, sp), unit_extra_s: 0.0 }
            });
            let (lanes, mut running) = forecast::pool_lanes(lead, second, &workers, &measured, &leased, &cost);
            for k in 0..SLOTS {
                let Some(m) = lanes.iter().position(|m| m.name == self.worker_of(k)) else { continue };
                let Some(r) = self.slots[k].running.as_ref() else { continue };
                if let Some(b) = inp.before.iter().find(|b| b.0 == step_of(&r.spec.id)) {
                    running.insert((b.0.clone(), b.1.clone()), m);
                }
                if let Some(w) = r.spec.record.as_ref() {
                    running.extend(w.targets.iter().map(|t| ((w.step.clone(), t.0.clone()), m)));
                }
            }
            f.equal = Some(Box::new(forecast::equal_machines(&inp, lanes, running)));
        }
        f
    }

    /// The build to the end (the status's checklist): a line per stage (crate::agent::steps::STAGES)
    /// with an item per step counted: the OSM pass (the planet, read when a newer one is on the
    /// NAS, and its worldwide steps), then the regions' steps (build::checklist).
    fn checklist(&self, root: &Path, regions: &[recipes::Recipe]) -> Vec<build::Line> {
        let mut items = Vec::new();
        let mut next: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut notes: BTreeMap<&str, String> = BTreeMap::new();
        let have = self.latest_pass(root);
        // The planet: a newer one's under way (or waiting), else done.
        let newer = matches!(crate::osmpass::newer_planet(root, have.as_deref()), Ok(Some(_)));
        items.push(build::Item { step: "osm-pass".into(), left: Some((newer || have.is_none()) as usize), ..Default::default() });
        let Some(date) = have else {
            // Nothing to size the rest by until a pass is complete: its steps, to come.
            items.extend(WORLDWIDE_STEPS.iter().map(|s| build::Item { step: s.to_string(), ..Default::default() }));
            items.extend(build::checklist_to_come());
            return build::fold(&items, &next, &notes);
        };
        let manifest: BTreeMap<String, String> = std::fs::read(root.join("state/build/manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let keys = build::Keys::load(root);
        // Countries the pass couldn't assemble, kept as the previous pass's outlines: said on the
        // pass's line until a pass has them whole (crate::outlines::assemble).
        if let Some(c) = manifest.get(&format!("sources/osm/{date}/outlines")) {
            if let Ok(stale) = crate::outlines::Outlines::stale_in(&root.join(c)) {
                if !stale.is_empty() {
                    notes.insert("pass", format!("Outlines kept from an earlier pass (broken in this planet's OpenStreetMap): {}", stale.iter().map(|(id, from)| format!("relation {id} (the {from} pass's)")).collect::<Vec<_>>().join(", ")));
                }
            }
        }
        // The pass's worldwide steps, each one run left or none.
        let left = [
            !crate::osmpass::SETS.iter().all(|st| manifest.contains_key(&crate::osmpass::set_name(&date, st.0))),
            build::outlines_work(&date, &manifest, &keys).is_some() || !manifest.contains_key(&format!("sources/osm/{date}/outlines")),
            build::trailends_work(&date, &manifest, &keys).is_some() || !manifest.contains_key(&crate::osmpass::set_name(&date, "hikes")),
            build::reach_work(&date, &manifest, &keys).is_some(),
            !manifest.contains_key(&crate::terrain_z8::logical()),
            crate::buildtiles::release_of(root, &manifest).map_or(true, |r| !manifest.contains_key(&crate::buildtiles::index_logical(&r))),
            build::summits_work(&date, &manifest, &keys).is_some() || !manifest.contains_key(&format!("work/summits/{date}")),
            build::labels_work(&date, &manifest, &keys).is_some(),
            // (A source folder that can't be read: held, left, as the jobs see it.)
            crate::lakefill::run::planned_source(root, &manifest).map_or(true, |s| build::lakefill_work(&date, &manifest, &keys, s.as_deref()).is_some()),
            build::water_work(&date, &manifest, &keys).is_some(),
        ];
        items.extend(WORLDWIDE_STEPS.iter().zip(left).map(|(s, l)| build::Item { step: s.to_string(), left: Some(l as usize), ..Default::default() }));
        next.insert("pass", WORLDWIDE_STEPS.iter().zip(left).filter(|(_, l)| *l).map(|(s, _)| steps::title(s).to_string()).collect());
        if regions.is_empty() {
            return build::fold(&items, &next, &notes);
        }
        let Ok(covs) = self.coverage(root, &manifest, &date, regions, false) else { return build::fold(&items, &next, &notes) };
        let cov = &covs.all;
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        let tiles = self.terrain_tiles(root, &manifest);
        let inputs = input_digests(root);
        // (As re-keyed: agent::rekey.)
        let mut keys = keys;
        let times = rekey::FileTimes::new(root);
        rekey::as_read(&mut keys, cov, &date, &manifest, reach.as_deref(), &inputs, &tiles, &times);
        let (more, more_next) = build::checklist(cov, &date, &manifest, &keys, &inputs, root.join("inputs/hold-catalog").exists(), reach.as_deref(), &self.ready.borrow(), &tiles);
        items.extend(more);
        next.extend(more_next);
        build::fold(&items, &next, &notes)
    }

    /// Per region, how many of its areas are built (none before the first pass makes the outlines).
    fn region_progress(&self, root: &Path, regions: &[recipes::Recipe]) -> BTreeMap<String, build::RegionState> {
        let Some(date) = self.latest_pass(root) else { return BTreeMap::new() };
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

    /// `keys` as every reader reads them (agent::rekey::as_read: re-keyed, terrain's and slope's
    /// derived), for a job's `--expect-same`; as they are when what that reads can't be told now.
    fn as_read(&self, root: &Path, mut keys: build::Keys) -> build::Keys {
        let Some(date) = self.latest_pass(root) else { return keys };
        let (recipes, unread) = load_regions(root);
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
        let _ = (date, recipes);
        // (The accepted version: keyed by its index's name, an edit being a new version.)
        let a = crate::inputs::regions::read::source(root, manifest).map_err(|e| format!("the regions' accepted version: {e:#}"))?;
        let (key, seen) = (Some(format!("accepted {}", a.version)), Some(a.version.clone()));
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
        let all = crate::coverage::Coverage::accepted(root, &a, None).map_err(|e| format!("{e:#}"))?;
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
            // (Whole, or what changed over the one before: store::catalog::read_n.)
            let Some(path) = store::catalog::path_n(dir, n) else { continue };
            let Ok(t) = std::fs::metadata(&path).and_then(|m| m.modified()) else { continue };
            if cached.as_ref().is_none_or(|c| c.0 != (dir.to_path_buf(), n)) {
                let Ok(cat) = store::catalog::read_n(dir, n) else { continue };
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
    /// from a member to the lead's (and this Mac paused or going on meanwhile, while that can't
    /// be reached); on the lead, then, its coordinator's, mirrored to the NAS
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
                            waiting.push(Waiting { step: None, what: "Pausing".into(), why: format!("this Mac's ask isn't with the lead yet ({e:#}); it holds here meanwhile") });
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

    /// Slot `k`'s job stopped other than by its own end (the Mac slept, its lease lapsed, the agent
    /// stops): what it finished is handed off, and its lease given back, not held against its
    /// targets.
    fn stopped(&mut self, k: usize, why: &str) {
        let done = self.finished_targets(k, false);
        let step = self.slots[k].running.as_ref().and_then(|r| r.spec.record.as_ref().map(|w| w.step.clone())).unwrap_or_default();
        let secs = self.slots[k].running.as_ref().map_or(0, |r| r.elapsed().as_secs());
        if let Some(id) = self.slots[k].running.as_ref().map(|r| r.spec.id.clone()) {
            self.worked(&id);
            self.save();
        }
        self.note_end(k, &step, &done, secs, false, why);
        self.end_lease(k, Outcome::Interrupted, &done, why);
    }

    /// Why this process restarts once its first job's slot is free (the pool's switch changed, or
    /// its part in the pool); None: it goes on.
    fn pool_restart(&self) -> Option<String> {
        self.restart_for.clone().or_else(|| match self.try_pooled()? {
            Some(p) => p.restart.clone(),
            None => self.pool_seen.as_ref()?.restart.clone(),
        })
    }

    /// The budgets' proxy: started once (taking from the pool's budget on the NAS's project folder
    /// `root`, as this agent); told the members this Mac knows (its share of the rate with the NAS
    /// away).
    fn tend_proxy(&mut self, root: &Path) {
        if self.proxy.is_none() {
            let me = self.member_id.clone().unwrap_or_else(|| self.me.clone());
            match crate::budget::start_proxy(root, &me) {
                Ok(p) => {
                    eprintln!("agent: the budgets' proxy listens on {}", p.url());
                    self.proxy = Some(p);
                }
                Err(e) => eprintln!("agent: the budgets' proxy didn't start ({e:#}); no job starts until it has"),
            }
        }
        let n = match self.try_pooled() {
            None => 1,
            Some(Some(run)) => run.side.heard_lately(),
            Some(None) => PoolView::heard_of(self.pool_seen.as_ref()),
        };
        if let Some(p) = &self.proxy {
            p.set_members(n);
        }
    }

    /// Resuming (crate::agent::resume, `state/pool/resume`) acts: switched on, in the pool. Off, or
    /// out of the pool, the agent is as it was.
    fn resume_on(&self) -> bool {
        self.resume && self.pool.is_some()
    }

    /// Every step to every member (crate::agent::every) acts: in the pool.
    fn every_on(&self) -> bool {
        self.pool.is_some()
    }

    /// Placement by how crucial a job is (crate::agent::placement, `state/pool/placement`) acts:
    /// switched on, in the pool. Off, or out of the pool, the agent and its coordinator are as they
    /// were.
    fn placement_on(&self) -> bool {
        self.placement && self.pool.is_some()
    }

    /// What this Mac's coordinator and broker place by, placement on: the forecast's slack (none
    /// yet: no slack), and each worker its coordinator knows measured against this Mac by the
    /// build's history (`forecast::speeds`, under the names they ask with: the forecast's own
    /// machines are the helpers' status files', not the pool's members), one not measured the
    /// lead's.
    fn placement_now(&self) -> Option<placement::Placement> {
        if !self.placement_on() {
            return None;
        }
        let mut p = self.forecast.borrow().as_ref().map(placement::Placement::from_forecast).unwrap_or_default();
        if let Some(c) = &self.coord {
            let names: Vec<String> = c.workers().into_iter().map(|(n, _)| n).filter(|n| *n != self.host).collect();
            let events = c.for_forecast().2;
            p.speed = placement::measured_speeds(&events, &self.host, &names);
        }
        Some(p)
    }

    /// Tasks brokered by every member (crate::agent::brokers, `state/pool/brokers`) act: switched on,
    /// in the pool. Off, or out of the pool, the agent is as it was (a member brokers nothing).
    fn brokers_on(&self) -> bool {
        self.brokers && self.pool.is_some()
    }

    /// Tasks brokered by every member, each loop (no wait on the pool's run: a tick may hold it): a
    /// member's broker started (its jobs' tasks its own) or, switched off, stopped; its leases
    /// lapsing, its pause the lead's; its address over HTTPS looked for. The heartbeat's `broker` and
    /// the members' brokers read from theirs are the run's, under its lock, in its steps (the loop's
    /// pool step and the ticks: `pool::Run::tend_brokers`, told this Mac's broker through `Cx`).
    /// Off: nothing of it, every coordinator told none.
    fn tend_brokers(&mut self) {
        if !self.brokers_on() {
            if let Some(b) = self.broker.take() {
                eprintln!("agent: this Mac's broker stopped (tasks brokered by every member are off): its jobs run their tasks themselves");
                b.stop();
            }
            if let Some(c) = self.coord.as_ref().filter(|c| c.shared.lock().unwrap().brokers.is_some()) {
                c.set_brokers(None);
            }
            // (A tick from here advertises no broker: the run told now when no tick holds it, else
            // at the loop's next pool step.)
            if let Some(Some(mut run)) = self.try_pooled() {
                run.cx.broker = None;
                run.side.broker = None;
            }
            return;
        }
        let now = Instant::now();
        // A member's broker, for its own jobs' tasks (tried again five minutes after it failed; not
        // while a tick holds the run: the next loop).
        if self.coord.is_none() && self.broker.is_none() && self.o.helper && !self.o.dry_run && self._lock.is_some() && brokers::stale(self.broker_failed, now, brokers::HTTPS_EVERY) {
            let tried = match self.try_pooled() {
                Some(Some(run)) => Some(make_coordinator_on(&self.o, Some(&run), &self.app, &self.host, broker_port())),
                _ => None,
            };
            if let Some(made) = tried {
                match made {
                    Some(b) => {
                        b.become_broker();
                        eprintln!("agent: this Mac's broker started at {}: its jobs' tasks are offered there", b.contact.urls.join(", "));
                        self.broker = Some(std::sync::Arc::new(b));
                    }
                    None => self.broker_failed = Some(now),
                }
            }
        }
        let Some(c) = self.coord.as_ref().or(self.broker.as_ref()) else { return };
        if let Some(b) = &self.broker {
            for l in b.expire() {
                eprintln!("agent: {}'s lease of {} lapsed at this Mac's broker (no word for {} min); offered again", l.worker, l.what(), crate::coord::TTL.as_secs() / 60);
            }
            // (Its pages paused as the build is: the lead's pause, as this member knows it.)
            if b.pause() != self.pause {
                b.set_pause(self.pause.clone(), now_s());
            }
        }
        if brokers::stale(self.broker_https.as_ref().map(|h| h.0), now, brokers::HTTPS_EVERY) {
            self.broker_https = Some((now, served_https(c.port())));
        }
    }

    /// The coordinator brokering this Mac's tasks, for the run's steps (`Cx::broker`): the lead's,
    /// or a member's broker; None with tasks brokered by every member off.
    fn brokering(&self) -> Option<std::sync::Arc<crate::coord::Coordinator>> {
        self.brokers_on().then(|| self.coord.clone().or_else(|| self.broker.clone())).flatten()
    }

    /// The newest complete pass: by the merged records with resuming on (docs/pool.md §7.4), else
    /// by the NAS's `pass.*.json` files, as before.
    fn latest_pass(&self, root: &Path) -> Option<String> {
        crate::osmpass::latest_pass_by(root, self.resume_on())
    }

    /// This member's clock, as its pool's steps read it (unix seconds).
    fn resume_now(&self) -> u64 {
        match self.try_pooled() {
            Some(Some(p)) => p.side.now(),
            _ => now_s(),
        }
    }

    /// This member's progress file: as this process last wrote it, else read from the NAS (once,
    /// or again after a read failed). None out of the pool, or when it can't be read now.
    fn own_progress(&mut self, root: &Path) -> Option<resume::Progress> {
        if self.held_progress.is_none() {
            let me = self.member_id.clone()?;
            match resume::read(root, &me) {
                Ok(p) => self.held_progress = Some(p.unwrap_or(resume::Progress { member: me, worker: self.host.clone(), jobs: BTreeMap::new() })),
                Err(e) => {
                    eprintln!("agent: this Mac's progress file can't be read now ({e:#})");
                    return None;
                }
            }
        }
        self.held_progress.clone()
    }

    /// Notes job `job`'s progress on this Mac in its member's progress file (None: dropped),
    /// written whole when that changes it, and only then.
    fn note_progress(&mut self, root: &Path, job: &str, held: Option<resume::Held>) {
        let Some(mut p) = self.own_progress(root) else { return };
        let before = p.clone();
        match held {
            Some(h) => {
                p.jobs.insert(job.to_string(), h);
            }
            None => {
                p.jobs.remove(job);
            }
        }
        p.worker = self.host.clone();
        if p == before {
            return;
        }
        match resume::write(root, &p) {
            Ok(()) => self.held_progress = Some(p),
            Err(e) => eprintln!("agent: this Mac's progress of {job} not noted on the NAS ({e:#}): the lead may run it elsewhere"),
        }
    }

    /// A resumable job starting here (resuming on): its progress noted as this Mac's, from now.
    fn progress_started(&mut self, spec: &JobSpec) {
        if !self.resume_on() || !resume::resumable(&spec.id) {
            return;
        }
        let Some(root) = self.last_root.clone() else { return };
        let scratch = scratch_of(spec).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let at = self.resume_now();
        // (Its day from when its progress began here, however often it starts again: the file read
        // first, so a read that failed before doesn't begin it anew.)
        let Some(mine) = self.own_progress(&root) else { return };
        let since = mine.jobs.get(&spec.id).filter(|h| !h.done).map_or(at, resume::Held::began);
        self.note_progress(&root, &spec.id, Some(resume::Held { at, since, done: false, scratch }));
    }

    /// A resumable job ended here (resuming on): its progress noted from now, finished or not (a
    /// finished one is held, unoffered, until its pass is in the records: `tend_progress`).
    fn progress_ended(&mut self, id: &str, ok: bool) {
        if !self.resume_on() || !resume::resumable(id) {
            return;
        }
        let Some(root) = self.last_root.clone() else { return };
        let was = self.held_progress.as_ref().and_then(|p| p.jobs.get(id)).cloned();
        let at = self.resume_now();
        let (since, scratch) = was.map_or((at, String::new()), |h| (h.began(), h.scratch));
        self.note_progress(&root, id, Some(resume::Held { at, since, done: ok, scratch }));
    }

    /// Each loop, resuming on: this member's progress no longer held dropped from its file (a
    /// pass the records hold complete, finished here; a day past its job's last run here, or
    /// released by the owner), its scratch folder left to room-making as before. A job running
    /// here is left as it is.
    fn tend_progress(&mut self, root: &Path) {
        if !self.resume_on() {
            return;
        }
        let Some(p) = self.own_progress(root) else { return };
        if p.jobs.is_empty() {
            return;
        }
        let now = self.resume_now();
        let running: BTreeSet<String> = self.slots.iter().filter_map(|s| s.running.as_ref()).map(|r| r.spec.id.clone()).collect();
        // (The records read only for a finished job's: the manifest is large.)
        let latest = p.jobs.values().any(|h| h.done).then(|| self.latest_pass(root)).flatten();
        for (job, h) in &p.jobs {
            if running.contains(job) {
                // (Noted again while it runs, so the lead sees it may be running: `at` alone.)
                if now >= h.at.saturating_add(resume::REFRESH_S) {
                    self.note_progress(root, job, Some(resume::Held { at: now, ..h.clone() }));
                }
                continue;
            }
            let complete = h.done && job.strip_prefix("osm-pass ").is_some_and(|d| latest.as_deref().is_some_and(|l| l >= d));
            let released = resume::released(root, &p.member, job, h.began());
            if complete || !resume::live(h, now, released) {
                let why = if complete { "its pass is in the records" } else if released { "the owner released it" } else { "a day since its job last ran here" };
                eprintln!("agent: {job}'s progress here no longer held for this Mac ({why})");
                self.note_progress(root, job, None);
            }
        }
    }

    /// What the lead's plan does with the OSM pass of `newer` (its planet and date), resuming on
    /// (crate::agent::resume::decide): None to plan it here as before; else why not, the pass
    /// offered meanwhile to the one member whose Mac holds its progress (the coordinator's
    /// `offer_resume`, none otherwise).
    fn resume_hold(&self, root: &Path, newer: Option<&(PathBuf, String)>) -> Option<Waiting> {
        let offer = |to: Option<(String, crate::coord::Offer)>| {
            if let Some(c) = &self.coord {
                c.offer_resume(to);
            }
        };
        // (Off: none offered, as before.)
        if !self.resume_on() {
            offer(None);
            return None;
        }
        let Some((planet, date)) = newer else {
            offer(None);
            return None;
        };
        let me = self.member_id.clone()?;
        // (The members as the run knows them now, or, a tick holding it, as the last pool step did.)
        let mut members: BTreeSet<String> = match self.try_pooled() {
            Some(Some(run)) => run.side.members().clone(),
            _ => self.pool_seen.as_ref().map(|p| p.members.iter().cloned().collect()).unwrap_or_default(),
        };
        let id = format!("osm-pass {date}");
        let what = format!("Reading the OpenStreetMap planet of {date}");
        members.insert(me.clone());
        let mut files = Vec::new();
        for m in &members {
            match resume::read(root, m) {
                Ok(p) => files.extend(p),
                Err(e) => {
                    offer(None);
                    return Some(Waiting { step: Some("osm-pass".into()), what, why: format!("a member's progress can't be read now ({e:#})") });
                }
            }
        }
        // A member running it under a lease of this coordinator: not run here too, whatever its
        // file says now (released, its day out, or read stale meanwhile).
        if self.coord.as_ref().is_some_and(|c| c.held("osm-pass").contains(date)) {
            return Some(Waiting { step: Some("osm-pass".into()), what, why: "running on a member under its lease (its progress is on that Mac)".into() });
        }
        let now = self.resume_now();
        match resume::decide(&id, &me, &files, now, &|m, at| resume::released(root, m, &id, at)) {
            resume::Decision::Plan => {
                offer(None);
                // (A member that noted it lately may be running it under a lease the restored
                // state lacks: not run here too until its note is old.)
                resume::running_elsewhere(&id, &me, &files, now).map(|p| Waiting { step: Some("osm-pass".into()), what, why: format!("may be running on {} ({}): its progress noted {} min ago", p.worker, p.member, now.saturating_sub(p.jobs[&id].at) / 60) })
            }
            resume::Decision::Offer { member, worker, until } => {
                let key = planet.strip_prefix(root).unwrap_or(planet.as_path()).to_string_lossy().into_owned();
                offer(Some((worker.clone(), crate::coord::Offer { step: "osm-pass".into(), targets: vec![(date.clone(), key, 0)], batch: 1 })));
                let h = until.saturating_sub(now).div_ceil(3600);
                Some(Waiting { step: Some("osm-pass".into()), what, why: format!("its progress is on {worker}'s disk ({member}): offered to it alone for {h} h more (a member asleep or away holds it so), or until the owner releases it (scenic pool release {member} \"{id}\")") })
            }
            resume::Decision::Wait(why) => {
                offer(None);
                Some(Waiting { step: Some("osm-pass".into()), what, why })
            }
        }
    }

    /// The OSM pass's command for the planet `planet` of `date`, in this Mac's scratch folder of it
    /// (where its stages' markers are: crate::osmpass), the lead's own and a member's resumed alike.
    fn osm_pass_cmd(&self, root: &Path, planet: &Path, date: &str) -> Vec<String> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let scratch = self.o.home.join("scratch").join(format!("osm-{date}"));
        vec![
            s(&self.o.bin.join("scenic-build")),
            "osm-pass".into(),
            "--root".into(),
            s(root),
            "--scratch".into(),
            s(&scratch),
            "--planet".into(),
            s(planet),
            "--date".into(),
            date.to_string(),
            "--extract".into(),
            s(&self.o.bin.join("extract")),
            "--planetiler".into(),
            s(&root.join("sources/basemap/planetiler.jar")),
            "--clear".into(),
            s(&self.o.home.join("cache").join("base")),
        ]
    }

    /// This member's hand-offs for `step` its lead hasn't acknowledged, laid in lease order
    /// (crate::agent::resume::unmerged): unwritten, handed from a job and not yet stepped, or
    /// written and not acknowledged by the current term's lead (read from the journal; one that
    /// can't be read now left out).
    fn unmerged_for(&self, step: &str) -> Result<crate::handoff::Handoff> {
        let Some(shared) = &self.pool else { return Ok(Default::default()) };
        // (A tick holding the run, on a slow share: waited for a while, never for good.)
        let since = Instant::now();
        let run = loop {
            if let Some(r) = pool::try_lock(shared) {
                break r;
            }
            anyhow::ensure!(since.elapsed() < Duration::from_secs(30), "the pool's step holds its state (its share slow to answer)");
            std::thread::sleep(Duration::from_millis(100));
        };
        let mine = run.side.driver().mine();
        let mut es: Vec<crate::pool::journal::Entry> = mine.unwritten_entries().cloned().collect();
        es.extend(run.entries.iter().map(|(e, _)| e.clone()));
        es.extend(resume::unacked(run.side.nas(), step, &mine.to_tell(run.gates.term)));
        Ok(resume::unmerged(step, &es))
    }

    /// The pool's switches on the NAS, looked at each loop; and, not in the pool, whether this
    /// process can be now (it restarts into it).
    fn pool_switch(&mut self, root: Option<&Path>) {
        let Some(r) = root.filter(|_| self._lock.is_some() && !self.o.dry_run) else { return };
        self.tend_proxy(r);
        if let Some(on) = resume::on(r) {
            if on != self.resume {
                eprintln!("agent: resuming on its Mac, and a pass complete by the records, is {}", if on { "on" } else { "off" });
            }
            self.resume = on;
        }
        self.tend_progress(r);
        if let Some(on) = placement::on(r) {
            if on != self.placement {
                eprintln!("agent: placement by how crucial a job is, and work offered ahead, is {}", if on { "on" } else { "off" });
            }
            self.placement = on;
        }
        if let Some(on) = brokers::on(r) {
            if on != self.brokers {
                eprintln!("agent: tasks brokered by every member are {}", if on { "on (this Mac's jobs' tasks its own broker's; pages from the members' addresses answered)" } else { "off" });
            }
            self.brokers = on;
        }
        self.tend_brokers();
        // (The lead's coordinator told, with this Mac's room now, each loop; its map update with the
        // plan's offers: `offer_update`.)
        // (Placement worked out once a loop: the history read once.)
        let placed = self.placement_now();
        if let Some(c) = &self.coord {
            c.set_every(self.every_on().then(|| crate::coord::Every { own: Some(self.own_room()) }));
            c.set_placement(placed.clone());
            if !self.every_on() {
                c.set_map_update(None);
            }
        }
        // (A member's broker draws its pages' tasks the same way: by their paces at the kind as
        // that broker measured them, its own jobs' tasks against this Mac's runs. It has no
        // history and no forecast, so no member speeds nor slack: those only order the plan's
        // targets, which a broker doesn't offer.)
        if let Some(b) = &self.broker {
            b.set_placement(placed);
        }
        if let Some(on) = pool::slots_on(r) {
            if on != self.slots_on {
                eprintln!("agent: a part changing in this process is {}", if on { "on" } else { "off" });
            }
            self.slots_on = on;
            let busy = match self.try_pooled() {
                Some(Some(mut run)) => {
                    run.in_process = on;
                    false
                }
                Some(None) => true,
                None => false,
            };
            if busy {
                self.in_process_owed = Some(on);
            }
        }
        // Not in the pool (the NAS not reached as this process started, or its member's lock held
        // by another process): once the lock can be taken with the NAS answering, this process
        // restarts into the pool (its part, its coordinator).
        if self.pool.is_none() && self.restart_for.is_none() {
            let locks = self.o.home.parent().unwrap_or(&self.o.home).to_path_buf();
            if let Ok(id) = crate::pool::member_id(&self.o.home) {
                if let Ok(Some(_lock)) = crate::pool::MemberLock::take(&locks, &id) {
                    let why = "the NAS answers and this Mac's member's lock is free: into the pool".to_string();
                    eprintln!("agent: {why}");
                    self.restart_for = Some(why);
                }
            }
        }
    }

    /// The pool's step while it's on (docs/pool.md §12): the hand-offs from before the pool drained
    /// into the journal, settling's state handed over, the driver's step; then, leading, its
    /// coordinator told the term and whether it grants, and the files the lead keeps written (today's
    /// records for their readers, the raw tiles' archives named, the coordinator's state per term,
    /// its history). What it may do now; None when the pool isn't on. The pool's own thread
    /// (`pool::Ticker`) steps nothing from its start to its end, and steps the driver itself while
    /// this loop is busy elsewhere (`pool::Run::tick`); what it needs of the agent for that is set
    /// here (`pool::Cx`).
    fn pool_step(&mut self, root: Option<&Path>, c: &Conditions, waiting: &mut Vec<Waiting>) -> Option<pool::Gates> {
        let shared = self.pool.clone()?;
        {
            // (A tick under way, on a share slow to answer: this loop goes on without the pool's
            // step, nothing granted or started on its behalf, rather than wait on it.)
            // (Held only for the pool's thread's moment's check, or a tick just begun and not yet
            // said: tried again a while, `TRY_FOR`, never waited on for good.)
            let ticking = self.ticking.clone().unwrap_or_default();
            let since = Instant::now();
            let run = loop {
                if let Some(r) = pool::try_lock(&shared) {
                    break Some(r);
                }
                if ticking.since().is_some() || since.elapsed() >= TRY_FOR {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let Some(mut run) = run else {
                return Some(self.pool_skipped(root, ticking.since(), waiting));
            };
            if std::mem::take(&mut self.pool_hung).is_some() {
                eprintln!("agent: the pool's thread's step ended; the pool steps again");
            }
            self.pool_skips = 0;
            run.in_step = true;
            if let Some(on) = self.in_process_owed.take() {
                run.in_process = on;
            }
            run.entries.append(&mut self.held_entries);
        }
        // (`in_step` cleared however this step ends, a panic too: else no tick ever again.)
        let _clear = pool::InStep(shared.clone());
        let g = self.pool_step_in(root, c, waiting);
        let mut run = pool::lock(&shared);
        run.cx.coord = self.coord.clone();
        run.cx.broker = self.brokering();
        run.stepped_full();
        self.pool_seen = Some(run.pool_view());
        g
    }

    /// The loop's pool step passed over, a tick holding the run since `since` (this Mac's clock):
    /// the gates held (nothing granted or started on its behalf). Held past `STALL_S`, the tick is
    /// taken for hung on the share: a lead's coordinator stops granting and its contact comes off
    /// the NAS (members find no lead to ask, rather than one that won't answer them), and from
    /// `HUNG_SKIPS` passed over the status says so, naming the cause; all of it until a pool step
    /// runs again.
    fn pool_skipped(&mut self, root: Option<&Path>, since: Option<u64>, waiting: &mut Vec<Waiting>) -> pool::Gates {
        self.pool_skips += 1;
        // (By the awake clock: a sleep isn't a hang.)
        let held = since.map_or(0, |t| pool::Ticking::awake().saturating_sub(t));
        if held > crate::pool::driver::STALL_S {
            if self.pool_hung.is_none() {
                eprintln!("agent: the pool's thread's step has held its state {} min: taken for hung (the NAS not answering); its coordinator grants nothing", held / 60);
            }
            self.pool_hung = since;
            if let Some(co) = &self.coord {
                co.set_moving(Some("its pool step hangs (the NAS not answering)".into()));
                // (On a thread of its own, not waited for: the NAS it reads and removes on hangs. One
                // at a time; and not once the contact was published again since it began.)
                let busy = self.unpublishing.as_ref().is_some_and(|t| !t.is_finished());
                if let (Some(r), true, false) = (root, self.published.is_some(), busy) {
                    let (c, r, gens) = (co.clone(), r.to_path_buf(), self.publish_gen.clone());
                    let at = gens.load(std::sync::atomic::Ordering::SeqCst);
                    self.unpublishing = Some(std::thread::spawn(move || c.unpublish_unless(&r, &|| gens.load(std::sync::atomic::Ordering::SeqCst) != at)));
                    self.published = None;
                }
            }
        }
        let why = if self.pool_hung.is_some() {
            format!("its step on the pool's thread has hung for {} min (the NAS isn't answering it): nothing is granted or started on its behalf until it ends", held / 60)
        } else if self.pool_skips >= HUNG_SKIPS {
            format!("its step on the pool's thread has been under way {held} s (its share slow to answer): nothing is granted or started on its behalf until it ends")
        } else {
            "its step is under way on the pool's thread (its share slow to answer)".to_string()
        };
        waiting.push(Waiting { step: None, what: "The pool".into(), why });
        let g = self.pool_seen.as_ref().map(|p| p.gates.clone()).unwrap_or_default();
        pool::Gates { term: g.term, leads: g.leads, ..Default::default() }
    }

    fn pool_step_in(&mut self, root: Option<&Path>, c: &Conditions, waiting: &mut Vec<Waiting>) -> Option<pool::Gates> {
        let lead = self.pooled()?.role == pool::Role::Lead;
        let Some(r) = root else {
            // (No NAS: no step; leading, nothing the driver hasn't said. Nor a tick.)
            let mut run = self.pooled()?;
            run.gates = pool::Gates { term: run.gates.term, leads: run.gates.leads, ..Default::default() };
            run.cx.root = None;
            if let Some(co) = &self.coord {
                co.set_moving(Some("the NAS isn't reachable".into()));
            }
            return Some(run.gates.clone());
        };
        // Settling a handover: the duties in flight cancelled (a catalog killed: it writes once at
        // its end; a sweep stopped), then the coordinator's state written, for the next step.
        // (So too when it no longer leads: a catalog or a sweep of a term it lost.)
        let settling = self.pooled().is_some_and(|p| p.gates.settle && p.settled.is_none());
        let lost = lead && self.pooled().is_some_and(|p| p.gates.leads.is_none());
        if settling || lost {
            for k in 0..SLOTS {
                if self.slots[k].running.as_ref().is_some_and(|j| is_duty(&j.spec.id)) {
                    let j = self.slots[k].running.as_mut().unwrap();
                    let why = if settling { "the lead hands the build over" } else { "this Mac no longer leads" };
                    eprintln!("agent: {} stopped: {why}", j.spec.id);
                    j.stop(Duration::from_secs(30));
                    self.stopped(k, &format!("stopped: {why}"));
                    self.slots[k].running = None;
                    std::fs::remove_file(self.record_path(k)).ok();
                }
            }
            if let (Some(co), Some(mut run), true) = (&self.coord, self.pooled(), settling) {
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
        let duty = self.slots.iter().any(|s| s.running.as_ref().is_some_and(|j| is_duty(&j.spec.id)));
        let answered = self.client.as_ref().and_then(|c| c.answered());
        let (lead_urls, lead_failed) = self.client.as_ref().map(|c| (c.urls(), c.failed())).unwrap_or_default();
        let mut run = self.pooled()?;
        // (Only an answer from the current term's lead's coordinator, as it said it is.)
        let cur = run.side.driver().current().lead.clone();
        let lead_answered = pool::lead_answered(answered.as_ref(), cur.as_ref());
        if let Some(co) = &self.coord {
            co.set_member(&run.side.member().id);
        }
        run.cx = pool::Cx { root: Some(r.to_path_buf()), home: home.clone(), coord: self.coord.clone(), able, duty, starting: false, lead_answered, broker: self.brokering(), broker_https: self.broker_https.as_ref().and_then(|h| h.1.clone()), lead_urls, lead_failed };
        // (The jobs an earlier process left, once `run` stopped any still running.)
        run.take_left(&home);
        run.drain(lead.then(|| home.join("coord/journal")).as_deref(), &home.join("outbox"));
        // The owner's lead asks (crate::agent::lead): this Mac's, and the build page's.
        run.conds = Some(pool::Conds { home: c.home, ac: c.ac, battery: c.battery, able });
        if let Some(co) = &self.coord {
            run.controls.page.extend(co.take_lead_asks());
        }
        lead::take(&mut run, &home);
        run.tend_brokers();
        run.tend_reach();
        let out = run.step(able);
        // (Its part changed, in this process: its coordinator started or stopped, its jobs going on;
        // not tried again once a restart is asked for, its coordinator failing to start.)
        let change = run.in_process && run.restart.is_none();
        drop(run);
        if change {
            self.change_part(r, &out);
        }
        let keep: Vec<u64> = self.slots.iter().filter_map(|s| match &s.lease {
            Some(Held::Pooled { id, .. } | Held::Own(id)) => Some(*id),
            _ => None,
        }).collect();
        let mut run = self.pooled()?;
        // (With the terms a tick took up, whose coordinator's part is this step's.)
        let mut took = std::mem::take(&mut run.pending_took_up);
        took.extend(out.events.iter().filter(|e| matches!(e, crate::pool::driver::Event::TookUp { .. })).cloned());
        pool::took_up(&mut run, self.coord.as_deref(), &took, out.leads, &self.host, &keep);
        lead::after(&mut run, &out, r, &home, self.coord.as_deref());
        run.cx.coord = self.coord.clone();
        run.cx.broker = self.brokering();
        for e in &out.events {
            if let crate::pool::driver::Event::Failed { what, why } | crate::pool::driver::Event::Waits { what, why } = e {
                waiting.push(Waiting { step: None, what: "The pool".into(), why: format!("couldn't {what} yet: {why}") });
            }
        }
        let g = run.gates.clone();
        if let Some(co) = &self.coord {
            co.set_moving(pool::moving(&g, false));
        }
        drop(run);
        if lead && g.leads.is_some() {
            let saved = !out.events.iter().any(|e| matches!(e, crate::pool::driver::Event::Failed { what: "save the records", .. }));
            self.pool_lead_files(r, saved);
            // (A catalog and GC read today's files: caught up only once they hold these records.)
            let mut run = self.pooled()?;
            if run.behind() {
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
        let Some(role) = self.pooled().map(|r| r.role) else { return };
        match (role, out.leads) {
            (pool::Role::Member, Some(term)) => {
                let made = self.broker.take().or_else(|| make_coordinator(&self.o, self.pooled().as_deref(), &self.app, &self.host).map(std::sync::Arc::new));
                match made {
                    Some(c) => {
                        eprintln!("agent: this Mac leads term {term}: its coordinator started, its jobs going on");
                        // (The terms' events its member kept meanwhile, for this coordinator's history.)
                        lead::replay(&self.o.home.join("pool"), &c);
                        if let Some(mut run) = self.pooled() {
                            run.role = pool::Role::Lead;
                        }
                        self.o.helper = false;
                        self.coord = Some(c);
                        self.published = None;
                    }
                    None => {
                        if let Some(mut run) = self.pooled() {
                            run.restart.get_or_insert_with(|| format!("it leads term {term}, and its coordinator didn't start in this process"));
                        }
                    }
                }
            }
            (pool::Role::Lead, None) if out.stop.is_none() => {
                eprintln!("agent: this Mac no longer leads: its jobs going on through the lead");
                if let Some(mut run) = self.pooled() {
                    run.role = pool::Role::Member;
                    // (Nothing of the coordinator's for a tick from here.)
                    run.cx.coord = None;
                    run.cx.broker = None;
                }
                // (Its coordinator first: what ends from here, the duties stopped below too, ends
                // through the new lead, `end_lease`.)
                let mut old = self.coord.take();
                if let Some(c) = &old {
                    c.unpublish(root);
                    // (Tasks brokered by every member: kept on as this Mac's broker, its tasks and
                    // the pages holding them going on; the jobs' leases the new lead's.)
                    if self.brokers_on() {
                        c.become_broker();
                        eprintln!("agent: its coordinator kept on as this Mac's broker");
                    } else {
                        eprintln!("agent: its coordinator stopped");
                        c.stop();
                    }
                }
                if self.brokers_on() {
                    self.broker = old.take();
                }
                self.o.helper = true;
                self.client = None;
                // (Its duties in flight stopped, a catalog or a sweep: the lead's alone.)
                for k in 0..SLOTS {
                    if self.slots[k].running.as_ref().is_some_and(|j| is_duty(&j.spec.id)) {
                        let j = self.slots[k].running.as_mut().unwrap();
                        eprintln!("agent: {} stopped: this Mac no longer leads", j.spec.id);
                        j.stop(Duration::from_secs(30));
                        self.stopped(k, "stopped: this Mac no longer leads");
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
        let leading = self.pool_seen.as_ref().is_some_and(|p| p.role == pool::Role::Lead && p.gates.leads.is_some());
        let leases = self.coord.as_ref().is_some_and(|c| c.leases_out() > 0);
        if !(leading && leases && self.slots_on) {
            self.lead_awake_off();
            return;
        }
        if self.lead_awake.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))) {
            return;
        }
        self.lead_awake = store::spawn::spawn(store::spawn::command("/usr/bin/caffeinate").args(["-i", "-w", &std::process::id().to_string()]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())).ok();
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
        {
            let Some(mut run) = self.pooled().filter(|p| p.role == pool::Role::Lead && p.gates.leads.is_some()) else { return };
            let Some(m) = run.controls.view.as_ref().and_then(|v| v.members.iter().find(|m| !m.me && m.app == newer && m.can_lead).cloned()) else { return };
            eprintln!("agent: a newer app ({newer}) waits on this Mac's job; handing the lead to {}, on it", m.host);
            run.controls.page.push(crate::control::LeadRequest { ask: crate::control::LeadAsk::Give { to: m.member.clone() }, by: format!("this Mac's update to {newer}, waiting on its job"), at: now_s() });
        }
        self.update_handed = Some(newer);
    }

    /// What the lead keeps written besides its records (docs/pool.md §12): today's three files from
    /// a term's records after the first (term 1's own saves write them; `saved`: this step's
    /// records are on the NAS), the raw tiles' archives its records hold named in the raw store's
    /// index, the coordinator's state of its term, its history's new events.
    fn pool_lead_files(&mut self, root: &Path, saved: bool) {
        let Some(mut guard) = self.pooled() else { return };
        let run = &mut *guard;
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

    /// The gate's part of a loop: the listing thread started (on the lead or in a dry
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
        // (A member's: what changed the lead's check's key, told to the lead's coordinator, so it
        // lists the drop boxes and the acceptances now rather than at its next listing.)
        let mut tell: Vec<serde_json::Value> = Vec::new();
        for a in asks {
            if a.forget {
                if self.o.helper {
                    tell.push(serde_json::json!({ "unit": a.unit, "forget": true }));
                } else {
                    self.inputs.forget.borrow_mut().insert(a.unit.clone());
                    eprintln!("agent: {} asked to forget {}'s records", if a.by.is_empty() { "someone" } else { &a.by }, a.unit);
                }
                continue;
            }
            if self.o.helper {
                tell.push(serde_json::json!({ "unit": a.unit, "check": true, "full": a.full }));
            }
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
        if !tell.is_empty() {
            let mut waiting = Vec::new();
            match self.client(root, &mut waiting) {
                Some(c) => {
                    for t in tell {
                        if let Err(e) = c.post_json("/work/inputs", &t) {
                            eprintln!("agent: telling the lead of {t}: {e:#} (it lists the drop boxes within two minutes all the same)");
                        }
                    }
                }
                None => eprintln!("agent: the lead's coordinator can't be reached ({}); it lists the drop boxes within two minutes all the same", waiting.first().map(|w| w.why.clone()).unwrap_or_default()),
            }
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
                // (Nothing dropped yet and nothing ever taken in or held: nothing to check.)
                let empty = l.listing.files.is_empty() && l.listing.strays.is_empty() && l.listing.misplaced.is_empty();
                if empty && [crate::inputs::logical(unit), crate::inputs::held_logical(unit)].iter().all(|r| !manifest.contains_key(r)) {
                    continue;
                }
                // (Daily by the records' last full check, whichever Mac made it: a new lead doesn't
                // repeat one made less than a day ago.)
                let full_at = manifest.get(&crate::inputs::listed_logical(unit)).and_then(|n| {
                    let mut seen = self.inputs.full_at.borrow_mut();
                    *seen.entry(n.clone()).or_insert_with(|| crate::inputs::read_listed_file(root, n).ok().and_then(|l| l.full_at))
                });
                let full = self.inputs.full.borrow().contains(*unit) || full_at.is_none_or(|t| now_s().saturating_sub(t) >= 86400);
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
                // (Named by the unit flat, `timetables+gtfs`: a nested unit's name is one file's.)
                let flat = crate::inputs::flat(unit);
                let planned = self.o.home.join("inputs-listing").join(format!("{flat}.{key}.json"));
                let wrote = std::fs::create_dir_all(planned.parent().unwrap()).map_err(anyhow::Error::from).and_then(|()| {
                    for e in std::fs::read_dir(planned.parent().unwrap())?.flatten() {
                        if e.file_name().to_string_lossy().starts_with(&format!("{flat}.")) && e.path() != planned {
                            std::fs::remove_file(e.path()).ok();
                        }
                    }
                    crate::whole::write(&planned, &serde_json::to_vec(&crate::inputs::Planned { listing: l.listing.clone(), accepted: l.accepted.clone() })?)
                });
                if let Err(e) = wrote {
                    waiting.push(Waiting { step: Some("inputs".into()), what: format!("Checking {unit}"), why: format!("its listing can't be kept for the check: {e:#}") });
                    continue;
                }
                let mut cmd = vec![s(&self.o.bin.join("scenic-build")), "inputs".into(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch").join("inputs")), unit.to_string(), "--listing".into(), s(&planned), "--dem".into(), s(&self.o.bin.join("dem"))];
                if full {
                    cmd.push("--full".into());
                }
                let id = if full { format!("inputs {unit} full") } else { format!("inputs {unit}") };
                jobs.push(JobSpec { id, what: format!("Checking the inputs dropped in {unit}{}", if full { ", every file" } else { "" }), cmd, needs: Needs { nas: true }, restart_after_sleep: true, record: Some(build::Work { step: "inputs".into(), targets: vec![(target, key)] }) });
            }
        }
        // The regions' extract, when their check holds files waiting for ids it lacks (its held
        // report's `needs`, of the current pass): a job extracting them with those it has.
        if !self.o.helper {
            if let Some(job) = self.extract_job(root, manifest, keys) {
                jobs.push(job);
            }
        }
        // A unit off the gate whose records were asked to be forgotten (`scenic inputs test off
        // --forget`): a job that removes them from the records.
        if !self.o.helper {
            let s = |p: &Path| p.to_string_lossy().into_owned();
            for unit in self.inputs.forget.borrow().iter() {
                let named = [crate::inputs::logical(unit), crate::inputs::held_logical(unit), crate::inputs::listed_logical(unit)].iter().any(|l| manifest.contains_key(l));
                if units.contains(&unit.as_str()) || !named || !crate::inputs::valid_unit(unit) {
                    continue;
                }
                let cmd = vec![s(&self.o.bin.join("scenic-build")), "inputs".into(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch").join("inputs")), unit.clone(), "--forget".into()];
                jobs.push(JobSpec { id: format!("inputs {unit} forget"), what: format!("Forgetting {unit}'s records"), cmd, needs: Needs { nas: true }, restart_after_sleep: true, record: Some(build::Work { step: "inputs".into(), targets: vec![(format!("inputs/{unit}"), "forget".into())] }) });
            }
        }
        let accepted = self.inputs.watch.as_ref().map(|(_, w)| w.accepted()).unwrap_or_default();
        *self.inputs.view.borrow_mut() = crate::inputs::view::of(root, manifest, &units, &checking, &accepted, &mut self.inputs.cache.borrow_mut());
        jobs
    }

    /// The regions' extract job (docs/inputs.md §6.1.6), when the regions' held report needs ids of
    /// the current pass and the job for them (and those the extract has) isn't done.
    fn extract_job(&self, root: &Path, manifest: &BTreeMap<String, String>, keys: &build::Keys) -> Option<JobSpec> {
        use crate::inputs::regions::extract;
        let held = manifest.get(&crate::inputs::held_logical(crate::inputs::regions::UNIT))?;
        let report = crate::inputs::read_report(root, held).ok()?;
        let date = report.needs.first()?.strip_prefix("pass ")?.to_string();
        if crate::osmpass::latest_pass_in(manifest).as_deref() != Some(date.as_str()) {
            return None;
        }
        let mut ids: BTreeSet<extract::Id> = report.needs.iter().skip(1).filter_map(|s| s.parse().ok()).collect();
        if ids.is_empty() {
            return None;
        }
        if let Some(e) = manifest.get(&extract::logical(&date)).and_then(|n| std::fs::read(root.join(n)).ok()).and_then(|b| extract::Extract::read(&b).ok()) {
            ids.extend(e.ids());
        }
        let key = extract::key(&date, &ids);
        let target = format!("regions-extract/{date}");
        if keys.lo.get(&target) == Some(&key) {
            return None;
        }
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let list: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        let cmd = vec![s(&self.o.bin.join("scenic-build")), "regions-extract".into(), "--root".into(), s(root), "--scratch".into(), s(&self.o.home.join("scratch").join("regions-extract")), "--pass".into(), date.clone(), "--ids".into(), list.join(",")];
        Some(JobSpec { id: format!("regions-extract {date}"), what: "Extracting the regions' OSM relations and ways".into(), cmd, needs: Needs { nas: true }, restart_after_sleep: true, record: Some(build::Work { step: "regions-extract".into(), targets: vec![(target, key)] }) })
    }

    /// Whether the loop restarts into a newer app now: one is installed and the first job's slot
    /// is free, or its job, asked to stop at its next safe point for the update, has had the time a
    /// pause gives (`DRAIN_GRACE`) and reached none (a long job may have none for hours).
    fn update_due(&self) -> bool {
        let Some(app) = self.pending_app() else { return false };
        let why = update_why(&app);
        match &self.slots[0].running {
            None => true,
            Some(r) => r.pausing.as_deref() == Some(why.as_str()) && self.slots[0].drain_since.is_some_and(|t| t.elapsed() >= self.grace),
        }
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

/// What jobs read from inputs/ beside the manifest, by digest: which keys inputs/keys.env holds
/// ("keys", `key_names`), and the 3D buildings' downloaded sources ("bld-release", "bld-key", "bldprep 6/x/y",
/// "bldprep-rows 6/x/y": crate::bld::sources::digests).
pub fn input_digests(root: &Path) -> BTreeMap<String, String> {
    let mut inputs: BTreeMap<String, String> = BTreeMap::new();
    // The heritage registers as the heritage sites' key names them (crate::heritage::registers_line);
    // "?" when they can't be read now (the heritage sites then wait), none without an accepted version.
    match crate::heritage::registers_now(root) {
        Ok(Some(line)) => {
            inputs.insert("heritage".into(), line);
        }
        Ok(None) => {}
        Err(_) => {
            inputs.insert("heritage".into(), "?".into());
        }
    }
    // The regions as a catalog records them; "?" when they can't be read now (build::catalog_work
    // then waits).
    inputs.insert("regions".into(), crate::inputs::regions::read::source_now(root).map(|a| a.version).unwrap_or_else(|_| "?".into()));
    inputs.insert("keys".into(), key_names(&root.join("inputs/keys.env")).unwrap_or_else(|| "?".into()));
    // The 3D buildings' sources, of the declared releases (crate::inputs::sources): what each z6
    // tile's bldprep reads, and its rows, and what the keys name of the declarations ("bld-key");
    // none before they're accepted (no 3D buildings' work), "?" while they can't be read now.
    match crate::inputs::sources::open_now(root).and_then(|s| s.map(|s| crate::bld::sources::declared(&s)).transpose()) {
        Ok(Some((release, ghsl, key))) => {
            inputs.extend(crate::bld::sources::digests(root, &release, &ghsl));
            inputs.insert("bld-key".into(), key);
        }
        Ok(None) => {}
        Err(_) => {
            inputs.insert("bld-release".into(), "?".into());
        }
    }
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

/// How often an idle agent plans again when nothing it depends on (`Agent::plan_seen`) changed
/// and no timed wait ends sooner: the fallback for what the digest doesn't cover.
const IDLE_REPLAN: Duration = Duration::from_secs(300);

/// The last plan, as `plan_due` weighs it: its age, its digest, whether a start was held by
/// something on a short timer, and when its timed waits end (epoch seconds).
#[derive(Clone, Copy, Debug)]
struct LastPlan {
    age: Duration,
    seen: u64,
    soon: bool,
    at: Option<u64>,
}

/// Whether the loop plans: when a job ended; with no plan yet; idle (the first job's slot free),
/// when a start was held by something on a short timer, what the plan depends on changed (`seen`
/// against the last plan's), its timed waits' earliest end (`at`) came (`now`, epoch seconds), or
/// `IDLE_REPLAN` passed; while the first job runs, every `every`.
fn plan_due(idle: bool, ended: bool, last: Option<LastPlan>, seen: u64, now: u64, every: Duration) -> bool {
    let Some(l) = last else { return true };
    if ended {
        return true;
    }
    if idle {
        return l.soon || l.seen != seen || l.at.is_some_and(|t| now >= t) || l.age >= IDLE_REPLAN;
    }
    l.age >= every
}

/// The earliest of `at` from `now` on (one at `now` kept: due at once).
fn earliest_after(at: impl IntoIterator<Item = u64>, now: u64) -> Option<u64> {
    at.into_iter().filter(|t| *t >= now).min()
}

/// When a wait with `left` to go ends, in whole epoch seconds from `now`, rounded up: never
/// before it does, and a part of a second left not taken for none.
fn ends_in(now: u64, left: Duration) -> u64 {
    now + left.as_millis().div_ceil(1000) as u64
}

/// How often an idle agent plans again while the budgets' proxy won't start.
const PROXY_REPLAN: Duration = Duration::from_secs(60);

/// The regions as the readers read them now (the accepted version: crate::inputs::regions::read),
/// sorted by id, and why none can be read when it can't (one line).

fn load_regions(root: &Path) -> (Vec<recipes::Recipe>, Vec<(String, String)>) {
    match crate::inputs::regions::read::source_now(root) {
        Ok(a) => match crate::inputs::regions::read::recipes(&a) {
            Ok(r) => (r, Vec::new()),
            Err(e) => (Vec::new(), vec![("regions".into(), format!("{e:#}"))]),
        },
        Err(e) => (Vec::new(), vec![("regions".into(), format!("{e:#}"))]),
    }
}


/// Why the build isn't done, by its forecast (None: nothing left to build, no map update under way, no
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
pub fn step_of(id: &str) -> Option<String> {
    id.split(' ').next().filter(|s| !s.is_empty()).map(str::to_string)
}

/// What holds the region work's jobs that read an input (`Agent::region_jobs`).
#[derive(Clone, Copy, Debug, Default)]
struct Holds {
    /// The pass's lake fill (`hold_for_lakefill`).
    lakefill: bool,
    /// No accepted version of the source declarations (`hold_for_sources`).
    sources: bool,
}

/// The jobs that read the declared sources (crate::inputs::sources, build::reads_sources) taken
/// out of `jobs` while no version of them is accepted (`held`), each said in `waiting`: they wait
/// for the input check to take the declarations in, rather than fail.
fn hold_for_sources(jobs: Vec<JobSpec>, held: bool, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
    if !held {
        return jobs;
    }
    let (kept, out): (Vec<JobSpec>, Vec<JobSpec>) = jobs.into_iter().partition(|j| !build::reads_sources(&step_of(&j.id).unwrap_or_default()));
    for j in out {
        waiting.push(Waiting { step: step_of(&j.id), what: j.what, why: "the source declarations' first accepted version (the input check takes inputs/sources/ in)".into() });
    }
    kept
}

/// The jobs that read the pass's lake fill (crate::lakefill: the water, which draws it; the terrain's
/// water digests, whose pin names it; the catalogs, which name it to the server), taken out of
/// `jobs` while it `held`s them (the lakefill job has work and hasn't failed
/// build::LAKEFILL_HOLDS times in a row, or its source can't be read), each said in `waiting`.
/// Nothing reads a fill made from an older basemap meanwhile, and nothing is made twice.
fn hold_for_lakefill(jobs: Vec<JobSpec>, held: bool, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
    if !held {
        return jobs;
    }
    let (kept, out): (Vec<JobSpec>, Vec<JobSpec>) = jobs.into_iter().partition(|j| !build::reads_lakefill(&step_of(&j.id).unwrap_or_default()));
    for j in out {
        waiting.push(Waiting { step: step_of(&j.id), what: j.what, why: "waits for Canada's missing lakes to be filled (the lakefill job)".into() });
    }
    kept
}

/// The checklist as the status shows it. A step with work left that isn't this Mac's job now (`now`,
/// its step) says why: another Mac is on it, or its job waits (`waiting`: for the home network,
/// out a failure). Publishing, while a step above has work left, goes out as each region is done (at
/// most hourly) and once they all are: it's done only then.
fn annotate(list: &mut [build::Line], now: &[String], helpers: &[Status], waiting: &[Waiting]) {
    let busy = |s: &build::Line| now.iter().any(|n| s.steps.iter().any(|x| x == n));
    // (Work known to be left: a line not sized yet, as trains a day before its sources are seeded,
    // holds nothing up.)
    let known_left = |s: &build::Line| s.left.is_some_and(|l| l > 0) || s.total.is_some_and(|t| s.done < t);
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

/// Why a running job stops at its next safe point for a newer app installed.
fn update_why(app: &str) -> String {
    format!("restarting into the newly installed app ({app})")
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
    make_coordinator_on(o, run, app, host, coord_port())
}

/// `make_coordinator`, on `port` (a member's broker's: `broker_port`).
fn make_coordinator_on(o: &Options, run: Option<&pool::Run>, app: &str, host: &str, port: u16) -> Option<crate::coord::Coordinator> {
    if let Some(r) = run {
        if let Err(e) = pool::seed(r.side.nas(), &o.home.join("coord")) {
            eprintln!("agent: the pool's token: {e:#}");
        }
    }
    // (A test's port, held open by it from the start: no other process can take it between.)
    #[cfg(test)]
    let held = crate::coord::take_test_listener(port);
    #[cfg(not(test))]
    let held: Option<std::net::TcpListener> = None;
    let started = match held {
        Some(l) => crate::coord::Coordinator::start_on(&o.home.join("coord"), Some(o.bin.join("wasm")), l, host, app),
        None => crate::coord::Coordinator::start(&o.home.join("coord"), Some(o.bin.join("wasm")), port, host, app),
    };
    match started {
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

/// The port a member's broker answers on (docs/pool.md §8): the coordinator's (one runs at a time
/// on a Mac: the lead's, or a member's broker); a unit test's own (`TEST_BROKER_PORT`), beside
/// its lead's in the same process.
fn broker_port() -> u16 {
    #[cfg(test)]
    if let Some(p) = TEST_BROKER_PORT.with(|p| p.get()) {
        return p;
    }
    coord_port()
}

/// A port's address over HTTPS, when `tailscale serve` proxies it (crate::net::served_https); none
/// in a unit test (no Mac's own Tailscale decides it) nor in WebAssembly.
#[cfg(not(any(test, target_os = "wasi")))]
fn served_https(port: u16) -> Option<String> {
    crate::net::served_https(port)
}

#[cfg(any(test, target_os = "wasi"))]
fn served_https(_port: u16) -> Option<String> {
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
    // (A rename over a file another Mac has open, its server reading the heartbeat: tried again,
    // then the old file renamed aside: crate::whole::rename_or_aside.)
    crate::whole::rename_or_aside(&tmp, p).map(drop).with_context(|| format!("rename to {}", p.display()))
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
#[cfg(test)]
fn batches(plan: Vec<build::Work>) -> Vec<(build::Work, usize)> {
    batches_sized(plan, &|_, _, n| n)
}

/// `batches`, each step's batch as `size` says from its step, its total and its batch (a map update
/// shared: the lead's share of a map update's map tiles, crate::agent::publish::lead_share).
fn batches_sized(plan: Vec<build::Work>, size: &dyn Fn(&str, usize, usize) -> usize) -> Vec<(build::Work, usize)> {
    let regions = regions_left(&plan);
    let mut out = Vec::new();
    for w in plan {
        let total = w.targets.len();
        let n = size(&w.step, total, job_size(&w.step, regions)).max(1);
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
/// serves (a damaged one passed over, as it does). (One read whole once: by its file and time, the
/// last that read is remembered, so a delta's chain isn't read again each loop.)
fn served_catalog(dir: &Path) -> Option<CatalogSeen> {
    static READ: std::sync::Mutex<Option<(PathBuf, std::time::SystemTime)>> = std::sync::Mutex::new(None);
    let mut ns = store::catalog::list(dir).ok()?;
    ns.sort_unstable();
    ns.into_iter().rev().find_map(|n| {
        let path = store::catalog::path_n(dir, n)?;
        let t = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        let seen = Some((path.clone(), t));
        if *READ.lock().unwrap_or_else(|e| e.into_inner()) != seen {
            store::catalog::read_n(dir, n).ok()?;
            *READ.lock().unwrap_or_else(|e| e.into_inner()) = seen;
        }
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

/// About how long a target of `step` takes on the lead (seconds) until it's been timed there:
/// the forecast's first guess (crate::agent::forecast), from the jobs' logs of 2026-10 (a tree cover
/// piece's from its z3 tiles' whole runs: 8,171 s for ~370 z6 tiles).
fn first_secs(step: &str) -> f64 {
    match step {
        "pass-sets" => 5000.0,
        "outlines" => 330.0,
        "trailends" => 15.0,
        "reach" => 1800.0,
        "terrain-z8" | "buildings" | "items" => 3600.0,
        "summits" => 30.0,
        "labels" => 2200.0,
        "water" => 3600.0,
        // (The survey, 47 s, and the fill by bands, about 15 min on the M1, 2026-10-09.)
        "lakefill" => 1200.0,
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
        // (A z6 tile on average, B2's estimate from B1's pilot on the M4: bldprep 2.2–4.7 s a
        // million rows and ~2.5 s to start, ~440 M rows over 380 tiles; bldtiles 0.5–2.9 s a
        // million buildings, ~342 M. Fetching what's there already: listings, and the coverage
        // unioned.)
        "bld-fetch" => 300.0,
        "bldprep" => 6.0,
        "bldtiles" => 2.0,
        "catalog" => 60.0,
        "names-todo" => 1070.0,
        "timetables-todo" => 10.0,
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
    // (Never the real NAS or the app's home: a test with no root would find the mounted NAS.)
    let root = o.root.as_deref().expect("a test agent needs a root of its own (a temp folder)");
    assert!(!is_live_path(root), "a test agent's root is the real NAS: {}", root.display());
    assert!(!is_live_path(&o.home), "a test agent's home is the app's real one: {}", o.home.display());
    // (The NAS's answer as the real probe decides it, by its folder, but at once: a loaded Mac's
    // threads can't make a test's NAS look away.)
    // (Its jobs go out through the budgets' proxy, as every agent's: started as its first loop
    // would.)
    Agent::new_with(o, |r| r.join("catalog").is_dir()).map(|mut a| {
        a.mac = cond::Mac::TEST;
        if let (Some(r), true, false) = (a.o.root.clone(), a._lock.is_some(), a.o.dry_run) {
            a.tend_proxy(&r);
        }
        a
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An idle agent plans again only when what its plan depends on changed, a start was held by
    /// something on a short timer, a timed wait ended, or its fallback is due (task #154); a job
    /// ended, or none planned yet, always; a busy one every `every`. (Times passed in: no clock.)
    #[test]
    fn an_idle_agent_plans_when_something_changed() {
        let s = Duration::from_secs;
        let every = s(60);
        let last = |age: u64, soon: bool, at: Option<u64>| Some(LastPlan { age: s(age), seen: 7, soon, at });
        assert!(plan_due(true, false, None, 7, 1000, every));
        assert!(plan_due(false, true, last(0, false, None), 7, 1000, every));
        // Idle, nothing changed (paused with jobs left, say: the pause is in the digest): not each
        // loop, nor at the busy interval; at the fallback.
        assert!(!plan_due(true, false, last(20, false, None), 7, 1000, every));
        assert!(!plan_due(true, false, last(120, false, None), 7, 1000, every));
        assert!(!plan_due(true, false, last(IDLE_REPLAN.as_secs() - 1, false, None), 7, 1000, every));
        assert!(plan_due(true, false, last(IDLE_REPLAN.as_secs(), false, None), 7, 1000, every));
        // Idle, something changed (the pause lifted): at once.
        assert!(plan_due(true, false, last(0, false, None), 8, 1000, every));
        // A start held by something on a short timer: next loop.
        assert!(plan_due(true, false, last(0, true, None), 7, 1000, every));
        // A timed wait (a failure's, say) ending at 1100: not before, then.
        assert!(!plan_due(true, false, last(20, false, Some(1100)), 7, 1099, every));
        assert!(plan_due(true, false, last(20, false, Some(1100)), 7, 1100, every));
        // Busy: every `every`, whatever the digest.
        assert!(!plan_due(false, false, last(59, true, Some(0)), 8, 1000, every));
        assert!(plan_due(false, false, last(60, false, None), 7, 1000, every));
    }

    /// A job's failure wait and the memory guard's set when an idle agent plans again: the earliest
    /// end to come, none past.
    #[test]
    fn a_timed_wait_sets_when_to_plan_again() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
        let job = |id: &str| JobSpec { id: id.into(), what: id.into(), cmd: vec![], needs: Needs { nas: true }, restart_after_sleep: true, record: None };
        let plan = vec![job("backup"), job("gc"), job("pass-sets d")];
        assert_eq!(a.replan_at(&plan, 1000), None);
        a.mem.retry.insert("backup".into(), (2, 1600));
        a.mem.retry.insert("gc".into(), (1, 900));
        a.guard_backoff.insert("pass-sets d".into(), (1300, "held".into()));
        a.mem.retry.insert("not planned".into(), (1, 1001));
        assert_eq!(a.replan_at(&plan, 1000), Some(1300));
        assert_eq!(a.replan_at(&plan, 1300), Some(1300));
        assert_eq!(a.replan_at(&plan, 1301), Some(1600));
        assert_eq!(a.replan_at(&plan, 1601), None);
        assert_eq!(earliest_after([5, 3, 9], 4), Some(5));
        // A wait with under a second left: due the next second, never dropped; one due now kept.
        assert_eq!(ends_in(1000, Duration::from_millis(400)), 1001);
        assert_eq!(ends_in(1000, Duration::from_millis(1400)), 1002);
        assert_eq!(ends_in(1000, Duration::ZERO), 1000);
        assert_eq!(earliest_after([1000, 1200], 1000), Some(1000));
        // The budgets' proxy not started: planned again each minute, not each loop.
        a.mem.retry.clear();
        a.guard_backoff.clear();
        a.o.dry_run = false;
        assert_eq!(a.replan_at(&plan, 1000), Some(1000 + PROXY_REPLAN.as_secs()));
    }

    /// The digest of what an idle agent's plan depends on: the same while nothing changes; changed
    /// by the records, a hand-off waiting, the pool's switches.
    #[test]
    fn the_plans_digest_follows_what_it_depends_on() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), "{}").unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
        let mut last = a.plan_seen(&root);
        assert_ne!(last, 0);
        assert_eq!(a.plan_seen(&root), last);
        // (Each change a different length: the digest doesn't rest on the file times' resolution.)
        std::fs::write(root.join("state/build/manifest.json"), "{\"a\":\"b\"}").unwrap();
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        last = now;
        std::fs::write(root.join("state/build/jobs.json"), "{}").unwrap();
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        last = now;
        std::fs::create_dir_all(a.handoff_bases(&root)[0].join("other-mac")).unwrap();
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        last = now;
        // Faster publishing's switches and the hold on catalogs.
        for f in [publish::UPDATE_PER_REGION, publish::NEAR_SLOPE, "inputs/hold-catalog"] {
            std::fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            std::fs::write(root.join(f), "on").unwrap();
            let now = a.plan_seen(&root);
            assert_ne!(now, last, "{f}");
            last = now;
        }
        // A held catalog's folder made.
        std::fs::create_dir_all(root.join("catalog-held")).unwrap();
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        last = now;
        // The coordinator's leases: this Mac's own taken and let go (a start that didn't go
        // through) changes nothing; another worker's taken, then given back, does.
        let co = std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.coord = Some(co.clone());
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        last = now;
        let id = co.hold("gc", &[("t".into(), "k".into())]).unwrap();
        assert_eq!(a.plan_seen(&root), last);
        co.finish(id, &[]);
        assert_eq!(a.plan_seen(&root), last);
        let id = {
            let mut s = co.shared.lock().unwrap();
            let t = s.now();
            s.leases.grant("other-mac", crate::coord::lease::Work::Job { step: "gc".into(), targets: vec![("t".into(), "k".into())] }, t)
        };
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        last = now;
        co.shared.lock().unwrap().leases.cancel(id).unwrap();
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        last = now;
        // The pool's switches the agent holds.
        a.placement = true;
        let now = a.plan_seen(&root);
        assert_ne!(now, last);
        assert_eq!(a.plan_seen(&root), now);
    }

    /// Every scenic-build command line the agent makes, by its own code (step_args, the targets as
    /// named, the OSM pass's), passes the steps' table (crate::cli): none refused at its start.
    #[test]
    fn the_agents_commands_pass_the_steps_table() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        for st in steps::TABLE.iter() {
            let step = st.name.to_string();
            // (gc and backup: `scenic`'s, their commands checked as the plan makes them.)
            // (catalog-held: `catalog --held --ready …`, as the plan makes it.)
            if crate::cli::tables::SCENIC.get(&step).is_some() || step == "catalog-held" {
                continue;
            }
            for target in [step.as_str(), "6/1/1", "3/1/1", "inputs/regions"] {
                if crate::cli::tables::SCENIC_BUILD.get(&step).is_some_and(|c| c.pos == 0) && target != step {
                    continue;
                }
                let mut cmd = vec!["SCENIC_X=1".to_string(), s(&a.o.bin.join("scenic-build")), step.clone(), "--root".into(), s(&root), "--scratch".into(), s(&home.join("scratch").join(&step))];
                cmd.extend([target.to_string()].into_iter().filter(|t| every::names_target(t) && *t != step));
                cmd.extend(a.step_args(&step, "2026-10-01"));
                if matches!(step.as_str(), "trees" | "terrain" | "slope") {
                    cmd.extend(["--expect-same".to_string(), "6/1/1".into()]);
                }
                if let Some(Err(e)) = crate::cli::tables::check_command(&cmd) {
                    panic!("{e}");
                }
            }
        }
        let pass = a.osm_pass_cmd(&root, &root.join("planet.osm.pbf"), "2026-10-01");
        assert_eq!(crate::cli::tables::check_command(&pass), Some(Ok(())));
    }

    /// A unit's tail as it runs its programs (scenic-metrics and the rest: crate::unit::tail)
    /// passes their tables.
    #[test]
    fn a_units_programs_pass_their_tables() {
        let u = crate::legacy::Unit::parse("6/1/1").unwrap();
        let mut seen = 0;
        for r in crate::unit::tail(u, true, true) {
            let cmd: Vec<String> = std::iter::once(format!("/app/{}", r.prog)).chain(r.args.iter().cloned()).collect();
            if let Some(c) = crate::cli::tables::check_command(&cmd) {
                c.unwrap();
                seen += 1;
            }
        }
        assert!(seen >= 5, "{seen}");
    }

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
    fn plans_with_the_hand_offs_waiting_in_the_journal() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("root"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let a = agent(&root, &home);
        let mut keys = build::Keys::default();
        keys.record("unit", &[("6/1/1".to_string(), "k1".to_string())]);
        keys.save(&root).unwrap();
        // Hand-offs the coordinator journaled here: they count as built until drained.
        let done = |u: &str, k: &str| crate::handoff::Handoff { done: Some(("unit".into(), vec![(u.into(), k.into())])), ..Default::default() };
        crate::handoff::write(&home.join("coord/journal/m1"), &done("6/1/2", "k2")).unwrap();
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
        assert!(build::Keys::load(&root).handed.is_empty(), "never in the keys saved");
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
        // A 20 GB target, which the disk has (40 GB free): a job's reserve, 30 GB, past it doesn't
        // fit, nor can the (empty) caches make it. The first waits, saying why; a job after it
        // needing as much isn't tried (the units), one keeping no target is: the daily backup
        // starts.
        room::set_target(&home, Some(20 << 30), "a test").unwrap();
        a.room_target = room::target(&home);
        let mut w = Vec::new();
        a.start_first(&[job("terrain 3/1/1"), job("unit 6/1/1"), job("unit 6/1/2"), job("backup daily")], &cond, Some(&root), &mut w);
        assert_eq!(w.iter().map(|w| w.what.as_str()).collect::<Vec<_>>(), ["terrain 3/1/1"], "{w:?}");
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
        // (The freeing's thread sees a test's disk, 1 TB free unless it says: a target past it.)
        let big = (1u64 << 40) + (100 << 30);
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
        let target = (1u64 << 40) + (100 << 30);
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
        let target = (1u64 << 40) + (100 << 30);
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

    /// A job's end the lead doesn't take (it doesn't answer, or its round failed already) is owed,
    /// not lost; the lead answering, it's told and not owed.
    #[test]
    fn an_end_the_lead_doesnt_take_is_owed() {
        let d = tempfile::tempdir().unwrap();
        // (A root of its own, no catalog/: the NAS away, the pool off.)
        let mut a = test_agent(Options { root: Some(d.path().join("nas")), home: d.path().join("home"), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: true }).unwrap();
        let fail = |id| Tell::Fail { id, why: "x".into(), floors: Vec::new(), disks: Vec::new(), job_disk: 0 };
        a.tell_or_owe(fail(1));
        assert_eq!(a.owed.len(), 1, "no client: owed");
        a.client = Some(crate::coord::client::Client::at(vec![crate::coord::client::tests::dead()], "t".into(), "m1"));
        a.begin_round();
        a.tell_or_owe(fail(2));
        assert!(a.client.as_ref().unwrap().round_failed());
        a.tell_or_owe(Tell::GiveBack { id: 3, why: "x".into(), floors: Vec::new(), disks: Vec::new(), job_disk: 0 });
        a.tell_or_owe(Tell::Done(Box::new(crate::coord::Done { lease: 4, ..Default::default() })));
        assert_eq!(a.owed.iter().map(|(_, t)| match t { Tell::Fail { id, .. } | Tell::GiveBack { id, .. } => *id, Tell::Done(d) => d.lease }).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        let (url, tried) = crate::coord::client::tests::answering(200);
        a.client = Some(crate::coord::client::Client::at(vec![url], "t".into(), "m1"));
        a.owed.clear();
        a.tell_or_owe(fail(5));
        assert!(a.owed.is_empty(), "taken");
        assert_eq!(tried.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// The guards: no test finds the real NAS, nor builds an agent or a share on it.
    #[test]
    fn tests_never_reach_the_nas() {
        assert_eq!(find_root(false), None);
        assert_eq!(find_root(true), None);
        assert!(is_live_path(Path::new("/Volumes/personal/projects/scenic-roads")));
        let d = tempfile::tempdir().unwrap();
        assert!(!is_live_path(d.path()));
        let o = |root: Option<PathBuf>| Options { root, home: d.path().join("home"), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: true };
        assert!(std::panic::catch_unwind(|| test_agent(o(None))).is_err(), "no root");
        assert!(std::panic::catch_unwind(|| test_agent(o(Some("/Volumes/personal/projects/scenic-roads".into())))).is_err(), "the NAS");
        if let Some(h) = std::env::var_os("HOME") {
            let live = Options { home: Path::new(&h).join("Library/Application Support/scenic"), ..o(Some(d.path().join("nas"))) };
            assert!(std::panic::catch_unwind(|| test_agent(live)).is_err(), "the app's home");
        }
        assert!(std::panic::catch_unwind(|| crate::pool::nas::Share::new(Path::new("/Volumes/x/scenic-roads"))).is_err(), "a share on the NAS");
        TEST_ROOT.with(|t| *t.borrow_mut() = Some("/Volumes/personal/projects/scenic-roads".into()));
        let tripped = std::panic::catch_unwind(|| find_root(false)).is_err();
        TEST_ROOT.with(|t| *t.borrow_mut() = None);
        assert!(tripped, "a test root on the NAS");
    }

    /// Each loop begins a round of the lead's client: a round the lead failed is open again.
    #[test]
    fn a_loop_begins_a_round() {
        let d = tempfile::tempdir().unwrap();
        // (A root of its own, no catalog/: the NAS away, the pool off.)
        let mut a = test_agent(Options { root: Some(d.path().join("nas")), home: d.path().join("home"), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: true }).unwrap();
        a.begin_round();
        a.client = Some(crate::coord::client::Client::at(vec![crate::coord::client::tests::dead()], "t".into(), "m1"));
        a.begin_round();
        assert!(a.client.as_ref().unwrap().beat(1, None).is_err());
        assert!(a.client.as_ref().unwrap().round_failed());
        a.begin_round();
        assert!(!a.client.as_ref().unwrap().round_failed());
    }

    #[test]
    fn a_member_runs_any_shared_steps_job_the_lead_leases() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        // The lead's coordinator offers slope, which this member fits.
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        c.offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: true }).unwrap();
        a.client = Some(crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1"));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut w = Vec::new();
        let jobs = a.helper_job(&root, &cond, &mut w);
        assert_eq!(jobs.len(), 1, "{w:?}");
        let j = &jobs[0];
        // The lead's own command for slope, its saves handed off to its lease's outbox.
        assert_eq!(j.id, "slope 3/2/2");
        assert!(j.cmd[1].starts_with("SCENIC_HANDOFF=") && j.cmd[2].starts_with("SCENIC_COSTS="));
        // (Its timings' record in its folder too, to go with its hand-off.)
        assert!(j.cmd[3].starts_with("SCENIC_TIMINGS=") && j.cmd[3].ends_with("/timings.json"));
        assert_eq!(&j.cmd[4..7], ["/app/scenic-build", "slope", "--root"]);
        assert!(j.cmd.contains(&"3/2/2".to_string()));
        assert_eq!(j.record.as_ref().map(|r| r.step.as_str()), Some("slope"));
        assert!(j.what.contains("for the lead"), "{}", j.what);
    }

    #[test]
    fn which_jobs_run_beside_each_other() {
        // Two of one step only when it's shared (its targets held apart); any two steps else (what
        // they share guarded where it's used, their memory and disk by their measures).
        assert!(!clash("unit", "heritage") && !clash("trees", "items") && !clash("items", "heritage"));
        assert!(!clash("unit", "unit") && !clash("pois", "pois") && !clash("bldtiles", "bldtiles"));
        assert!(clash("items", "items") && clash("catalog", "catalog") && clash("bldprep", "bldprep") == !steps::SHARED.contains(&"bldprep"));
        assert!(!clash("terrain", "peaks") && !clash("osm-pass", "items") && !clash("water", "bldtiles"));
    }

    #[test]
    fn a_3d_buildings_job_is_offered_by_the_rows_it_reads() {
        // B2's runs on the M1: Paris's 6/32/22 (16.5 M rows in its row groups) took 2.4 GB to read;
        // Vermont's 6/19/23 (6.8 M) 1.2 GB. B5's pilot raised New York's 14.6 M records with the
        // field in 7.9 GB, Toronto's 9.5 M in 3.8 GB, Kantō's 30.3 M in 10.2 GB. Offered above each.
        assert_eq!((bld_peak("bldprep", 16_505_246), bld_peak("bldtiles", 16_505_246)), (2818, 8725));
        assert!(bld_peak("bldprep", 6_771_191) > 1243 && bld_peak("bldtiles", 14_611_533) > 7889 && bld_peak("bldtiles", 9_496_532) > 3843 && bld_peak("bldtiles", 30_320_309) > 10200);
        // A tile reading nothing (a GHSL tile alone): its fixed part.
        assert_eq!((bld_peak("bldprep", 0), bld_peak("bldtiles", 0)), (300, 1800));
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
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
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
        // Beside the map tiles, the heritage chain, not a unit (too large beside them, by their
        // measures).
        let cost = |step: &str, mb: u64| crate::coord::Cost { peak_mb: mb, secs: 60, worker: None, v: crate::coord::cost_version(step) };
        a.coord.as_ref().unwrap().add_costs(&[(crate::coord::cost_key("pack", "6/1/1"), cost("pack", 1024)), (crate::coord::cost_key("heritage", "heritage"), cost("heritage", 2048)), (crate::coord::cost_key("unit", "6/1/1"), cost("unit", 15 << 10))]);
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
        // Beside a cold start (the OSM pass, never measured), none.
        a.slots = Default::default();
        assert!(a.try_start(0, JobSpec { record: None, ..job("osm-pass 2026-09-28") }, &cond, Some(&root), &mut Vec::new()));
        a.start_second(&plan, &cond, Some(&root));
        assert!(a.slots[1].running.is_none());
        assert!(a.beside_why.as_deref().is_some_and(|w| w.contains("alone")), "{:?}", a.beside_why);
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
    }

    /// The Mac shared by measure (docs/pool.md §7.7): a job starts beside another only if both jobs'
    /// measured memory and disk fit what the Mac has left; a cold start runs alone.
    #[test]
    fn by_measure_a_second_job_starts_or_is_held_by_the_measures_alone() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.free_set = Some(100 << 30);
        // (A Mac of 16 GB: its limit 12 GB.)
        a.mem_set = Some((16 << 10, 12 << 10));
        let job = |step: &str, t: &str| JobSpec { id: format!("{step} {t}"), what: step.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: step.into(), targets: vec![(t.into(), "k".into())] }) };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let stop = |a: &mut Agent| {
            for k in 0..SLOTS {
                if let Some(r) = a.slots[k].running.as_mut() {
                    r.stop(Duration::from_secs(5));
                    a.end_lease(k, Outcome::Interrupted, &[], "stopped");
                }
                a.slots[k].running = None;
            }
        };
        // A cold start (no target of its step measured): alone, nothing beside it.
        assert!(a.try_start(0, job("terrain", "6/1/1"), &cond, Some(&root), &mut Vec::new()));
        a.start_second(&[job("pack", "6/2/2")], &cond, Some(&root));
        assert!(a.slots[1].running.is_none() && a.beside_why.as_deref().is_some_and(|w| w.contains("cold start")), "{:?}", a.beside_why);
        stop(&mut a);
        // Measured: terrain 4 GB and 10 GB of disk; labels (a step the table runs alone) 6 GB; peaks
        // (a raw-tile reader, as terrain) 5 GB; a unit 9 GB.
        let c = a.coord.as_ref().unwrap();
        let cost = |step: &str, mb: u64| crate::coord::Cost { peak_mb: mb, secs: 60, worker: None, v: crate::coord::cost_version(step) };
        c.add_costs(&[(crate::coord::cost_key("terrain", "6/1/1"), cost("terrain", 4096)), (crate::coord::cost_key("labels", "labels"), cost("labels", 6144)), (crate::coord::cost_key("peaks", "6/2/2"), cost("peaks", 5120)), (crate::coord::cost_key("unit", "6/3/3"), cost("unit", 9216))]);
        c.add_disks(&[(crate::coord::cost_key("terrain", "6/1/1"), 10 << 10), (crate::coord::cost_key("peaks", "6/2/2"), 25 << 10)]);
        // Beside terrain: labels starts, though the table runs it alone and it isn't of the second
        // job's list; the unit before it in the plan doesn't fit the memory left.
        assert!(a.try_start(0, job("terrain", "6/1/1"), &cond, Some(&root), &mut Vec::new()));
        a.start_second(&[job("unit", "6/3/3"), job("labels", "labels")], &cond, Some(&root));
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.spec.id.clone()).as_deref(), Some("labels labels"), "{:?}", a.beside_why);
        // The first slot beside labels: a unit waits for it, past the limit together.
        assert!(a.waits_for_second(&job("unit", "6/3/3")).is_some_and(|w| w.contains("past this Mac's limit")));
        stop(&mut a);
        // A raw-tile reader beside another (peaks beside terrain): by measure, when the disk has room
        // for both, the reserve kept; held when it hasn't.
        assert!(a.try_start(0, job("terrain", "6/1/1"), &cond, Some(&root), &mut Vec::new()));
        a.free_set = Some(50 << 30);
        a.start_second(&[job("peaks", "6/2/2")], &cond, Some(&root));
        assert!(a.slots[1].running.is_none() && a.beside_why.as_deref().is_some_and(|w| w.contains("on the disk")), "{:?}", a.beside_why);
        a.free_set = Some(100 << 30);
        a.start_second(&[job("peaks", "6/2/2")], &cond, Some(&root));
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.spec.id.clone()).as_deref(), Some("peaks 6/2/2"), "{:?}", a.beside_why);
        stop(&mut a);
        // A target of a measured step, itself never run: by its step's largest (a prior), not cold.
        assert!(a.measured_job(&job("peaks", "6/9/9")).is_some_and(|m| m.mem_mb == 5120 && !m.own));
    }

    /// A unit job waiting on a worker holding its unit's tail (its `WAITING_ENV` file) counts, beside
    /// it, as the memory it holds now, its cores idle: a second job starts in the room it frees; a
    /// stale file (its process not the job's) counts for nothing; when the first goes on and the two
    /// hold too much, the second yields to the memory guard, the larger or not.
    #[test]
    fn a_job_waiting_on_a_worker_frees_its_room_and_the_job_beside_it_yields() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.free_set = Some(100 << 30);
        // (A Mac of 16 GB: its limit 12 GB.)
        a.mem_set = Some((16 << 10, 12 << 10));
        let job = |step: &str, t: &str| JobSpec { id: format!("{step} {t}"), what: step.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: step.into(), targets: vec![(t.into(), "k".into())] }) };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let c = a.coord.as_ref().unwrap();
        let cost = |step: &str, mb: u64| crate::coord::Cost { peak_mb: mb, secs: 60, worker: None, v: crate::coord::cost_version(step) };
        c.add_costs(&[(crate::coord::cost_key("unit", "6/3/3"), cost("unit", 9216)), (crate::coord::cost_key("labels", "labels"), cost("labels", 6144))]);
        assert!(a.try_start(0, job("unit", "6/3/3"), &cond, Some(&root), &mut Vec::new()));
        let tick = |a: &mut Agent, held: Vec<Option<u64>>| {
            a.sampler.set_test(Some(held), Some(false));
            a.sampler.tick();
            for k in 0..SLOTS {
                a.tend(k, &cond, Some(&root), 0).unwrap();
            }
        };
        // Its tail out, holding 1 GB now, but not saying it waits: by its measure (9 GB), labels
        // (6 GB) doesn't fit beside it.
        tick(&mut a, vec![Some(1 << 10), None]);
        let labels = [job("labels", "labels")];
        a.start_second(&labels, &cond, Some(&root));
        assert!(a.slots[1].running.is_none() && a.beside_why.as_deref().is_some_and(|w| w.contains("past this Mac's limit")), "{:?}", a.beside_why);
        // A stale file (a process not of the job's): the same.
        let note = |pid: u32| crate::offload::WaitNote { pid, worker: "ipad".into(), task: 7, since: now_s() };
        assert!(note(1).keep(&a.waiting_path(0)).map(std::mem::forget).is_some());
        assert!(a.waiting_on(0).is_none());
        a.start_second(&labels, &cond, Some(&root));
        assert!(a.slots[1].running.is_none());
        // Its own (the job's process): labels starts beside it, with all the cores, yielding.
        let pgid = a.slots[0].running.as_ref().unwrap().pgid as u32;
        assert!(note(pgid).keep(&a.waiting_path(0)).map(std::mem::forget).is_some());
        assert_eq!(a.waiting_on(0).map(|n| n.worker), Some("ipad".to_string()));
        assert!(a.job_view(0).and_then(|j| j.waiting).is_some_and(|w| w.contains("waiting on ipad")));
        a.start_second(&labels, &cond, Some(&root));
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.spec.id.clone()).as_deref(), Some("labels labels"), "{:?}", a.beside_why);
        assert_eq!(a.slots[1].yields.as_deref(), Some("unit 6/3/3"));
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.threads), Some(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)));
        // The tail back, the unit going on (its file gone): within the limit, the Mac not short of
        // memory, nothing.
        std::fs::remove_file(a.waiting_path(0)).unwrap();
        tick(&mut a, vec![Some(5 << 10), Some(6 << 10)]);
        assert!(!a.guard() && a.slots[1].running.is_some());
        // Past the limit together, the second the larger: it's stopped at once (no safe point
        // waited for), given back, not kept from this Mac, no floor learned of it; the unit goes on.
        tick(&mut a, vec![Some(5 << 10), Some(8 << 10)]);
        assert!(a.guard());
        assert!(a.slots[1].running.is_none() && a.slots[1].yields.is_none() && a.slots[0].running.is_some() && a.slots[0].mem_drain.is_none());
        assert!(a.guard_backoff.is_empty(), "{:?}", a.guard_backoff);
        assert!(a.coord.as_ref().unwrap().floor("labels", "labels").is_none());
        // Started beside it again, the Mac short of memory though within the limit: stopped at once.
        assert!(note(pgid).keep(&a.waiting_path(0)).map(std::mem::forget).is_some());
        tick(&mut a, vec![Some(1 << 10), None]);
        a.start_second(&labels, &cond, Some(&root));
        assert!(a.slots[1].running.is_some(), "{:?}", a.beside_why);
        std::fs::remove_file(a.waiting_path(0)).unwrap();
        a.sampler.set_test(Some(vec![Some(3 << 10), Some(3 << 10)]), Some(true));
        a.sampler.tick();
        assert!(a.guard() && a.slots[1].running.is_none());
        // Its mark tied to the job it yields to: a job in the second slot marked for a job no
        // longer running (the first slot's ended, another started) doesn't yield.
        a.sampler.set_test(Some(vec![Some(1 << 10), None]), Some(false));
        a.sampler.tick();
        assert!(a.try_start(1, job("labels", "labels"), &cond, Some(&root), &mut Vec::new()));
        a.slots[1].yields = Some("unit 6/3/3".into());
        assert!(a.yielding(1));
        a.slots[1].yields = Some("unit 6/9/9".into());
        assert!(!a.yielding(1));
        for k in 0..SLOTS {
            if let Some(r) = a.slots[k].running.as_mut() {
                r.stop(Duration::from_secs(5));
                a.end_lease(k, Outcome::Interrupted, &[], "stopped");
            }
            a.slots[k].running = None;
        }
    }

    /// The budgets (docs/pool.md §7.7): the agent's proxy runs and every job goes out through it,
    /// sandboxed (a connection around it denied); the Wikidata steps share the Mac, as do two heavy
    /// NAS readers, two raw-tile readers and a step the table once ran alone; no job starts without
    /// the proxy.
    #[test]
    fn jobs_go_out_through_the_proxy_and_no_group_keeps_steps_apart() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/pool")).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.free_set = Some(100 << 30);
        a.mem_set = Some((48 << 10, 42 << 10));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let seen = d.path().join("seen");
        let job = |step: &str, t: &str| {
            let script = format!("env > {0}/{step}.env; /usr/bin/nc -v -z -G 5 192.0.2.1 9 2> {0}/{step}.out; echo done >> {0}/{step}.out", seen.display());
            JobSpec { id: format!("{step} {t}"), what: step.into(), cmd: vec!["/bin/sh".into(), "-c".into(), script], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: step.into(), targets: vec![(t.into(), "k".into())] }) }
        };
        std::fs::create_dir_all(&seen).unwrap();
        let stop = |a: &mut Agent| {
            for k in 0..SLOTS {
                if let Some(r) = a.slots[k].running.as_mut() {
                    r.stop(Duration::from_secs(5));
                    a.end_lease(k, Outcome::Interrupted, &[], "stopped");
                }
                a.slots[k].running = None;
            }
        };
        a.pool_switch(Some(&root));
        let url = a.proxy.as_ref().map(|p| p.url()).expect("the proxy started");
        assert!(!a.clashes("items", "heritage") && !a.clashes("inputs", "bldprep") && !a.clashes("bldprep", "bldprep"));
        assert!(!a.clashes("terrain", "peaks") && !a.clashes("labels", "unit"));
        assert!(a.clashes("items", "items"), "the same unshared step: one job's work");
        assert!(a.try_start(0, job("items", "items"), &cond, Some(&root), &mut Vec::new()));
        assert!(a.try_start(1, job("heritage", "heritage"), &cond, Some(&root), &mut Vec::new()));
        // (Each job ends by itself once it has looked: its exit waited for, bounded by nc's own
        // five seconds at most; no clock in the test.)
        for k in 0..SLOTS {
            assert!(a.slots[k].running.as_mut().unwrap().wait().unwrap().success());
        }
        for step in ["items", "heritage"] {
            let out = seen.join(format!("{step}.out"));
            assert!(std::fs::read_to_string(&out).unwrap().contains("done"));
            let env = std::fs::read_to_string(seen.join(format!("{step}.env"))).unwrap();
            assert!(env.contains(&format!("HTTPS_PROXY={url}\n")) && env.contains(&format!("{}={url}\n", crate::budget::PROXY_ENV)) && env.contains("NO_PROXY=localhost,127.0.0.1,::1\n"), "{env}");
            assert!(std::fs::read_to_string(&out).unwrap().contains("Operation not permitted"), "{step}: sandboxed");
        }
        stop(&mut a);
        // No proxy: no job starts.
        let p = a.proxy.take();
        assert!(a.wait_reason(&job("items", "items"), &cond).is_some_and(|w| w.contains("proxy")));
        a.proxy = p;
        assert!(a.wait_reason(&job("items", "items"), &cond).is_none());
    }

    /// The disk guard (by measure): short of the reserve, the job growing fastest frozen, room made
    /// from what no job holds, and it goes on once the disk has room; given back when none can be
    /// made, what it took on the disk learned as its target's disk floor.
    #[test]
    fn the_disk_guard_freezes_makes_room_and_lets_go_on_or_gives_back() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.mem_set = Some((16 << 10, 12 << 10));
        let g: u64 = 1 << 30;
        let job = |t: &str| JobSpec { id: format!("terrain {t}"), what: "terrain".into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "terrain".into(), targets: vec![(t.into(), "k".into())] }) };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        a.free_set = Some(100 * g);
        assert!(a.try_start(0, job("6/1/1"), &cond, Some(&root), &mut Vec::new()));
        a.sampler.set_test(Some(vec![None, None]), Some(false));
        a.sampler.set_disk(true, room::RESERVE, &home);
        // The disk falls short: frozen; room made (here none to make), then the disk has room again
        // (another program let go of its files): it goes on.
        a.free_set = Some(20 * g);
        a.sampler.set_test_disk(Some(20 * g), Some(vec![Some(5 * g), None]));
        a.sampler.tick();
        assert!(a.sampler.disk_frozen(0));
        assert!(!a.disk_guard(Some(&root)) && a.disk_room.is_some(), "room being made");
        a.free_set = Some(40 * g);
        assert!(!a.disk_guard(Some(&root)));
        assert!(!a.sampler.disk_frozen(0) && a.slots[0].running.is_some(), "it goes on");
        // Short again, and no room to be made: given back, not failed, its disk floor learned.
        a.free_set = Some(20 * g);
        a.sampler.set_test_disk(Some(20 * g), Some(vec![Some(7 * g), None]));
        a.sampler.tick();
        assert!(a.sampler.disk_frozen(0));
        assert!(!a.disk_guard(Some(&root)));
        assert!(a.disk_guard(Some(&root)) && a.slots[0].running.is_none(), "given back");
        assert!(!a.mem.retry.contains_key("terrain 6/1/1"), "not held against it as a failure");
        assert!(a.coord.as_ref().unwrap().disks().get(&crate::coord::cost_key("terrain", "6/1/1")).is_some_and(|mb| *mb >= 7 << 10), "{:?}", a.coord.as_ref().unwrap().disks());
        assert!(a.guard_last.as_ref().is_some_and(|(_, w)| w.contains("given back by the disk guard")), "{:?}", a.guard_last);
        // Two frozen (the second filling the disk past half the reserve meanwhile), no room to make:
        // the one taking the most given back first, the other kept frozen until the next decision.
        a.free_set = Some(200 * g);
        assert!(a.try_start(0, job("6/2/2"), &cond, Some(&root), &mut Vec::new()));
        assert!(a.try_start(1, job("6/3/3"), &cond, Some(&root), &mut Vec::new()));
        a.sampler.set_test_disk(Some(20 * g), Some(vec![Some(2 * g), None]));
        a.sampler.tick();
        a.sampler.set_test_disk(Some(10 * g), Some(vec![Some(2 * g), Some(9 * g)]));
        a.sampler.tick();
        assert!(a.sampler.disk_frozen(0) && a.sampler.disk_frozen(1));
        a.free_set = Some(10 * g);
        assert!(!a.disk_guard(Some(&root)));
        assert!(a.disk_guard(Some(&root)) && a.slots[1].running.is_none() && a.slots[0].running.is_some(), "the larger given back");
        assert!(a.sampler.disk_frozen(0));
        a.free_set = Some(40 * g);
        assert!(!a.disk_guard(Some(&root)) && !a.sampler.disk_frozen(0), "the other goes on once there's room");
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
    }

    /// A batch's disk by measure (docs/pool.md §7.7): its targets' disks summed, not its largest (a
    /// batch's cache writes add up), and what's still to come of the one running by its targets not
    /// done yet; a stale or high floor never stalls the first slot: alone, it starts with the room
    /// there is, the disk guard its backstop.
    #[test]
    fn by_measure_a_batchs_disk_is_its_targets_summed() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.mem_set = Some((64 << 10, 56 << 10));
        let g: u64 = 1 << 30;
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        // 40 map tiles of 1 GB each on the disk, one of 3 GB; a unit of 10 GB.
        let tiles: Vec<String> = (0..40).map(|i| format!("6/{i}/0")).collect();
        let c = a.coord.as_ref().unwrap();
        let cost = |step: &str, mb: u64| crate::coord::Cost { peak_mb: mb, secs: 60, worker: None, v: crate::coord::cost_version(step) };
        c.add_costs(&[(crate::coord::cost_key("pack", "6/0/0"), cost("pack", 1000)), (crate::coord::cost_key("unit", "6/9/9"), cost("unit", 4000))]);
        let mut disks: Vec<(String, u64)> = tiles.iter().map(|t| (crate::coord::cost_key("pack", t), 1024)).collect();
        disks[39].1 = 3072;
        disks.push((crate::coord::cost_key("unit", "6/9/9"), 10 << 10));
        c.add_disks(&disks);
        let batch = JobSpec { id: "pack 6/0/0".into(), what: "map tiles".into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "pack".into(), targets: tiles.iter().map(|t| (t.clone(), "k".into())).collect() }) };
        assert_eq!(a.measured_job(&batch).map(|m| m.disk_mb), Some(39 * 1024 + 3072), "summed, not its largest");
        a.free_set = Some(100 * g);
        assert!(a.try_start(0, batch, &cond, Some(&root), &mut Vec::new()));
        // Five done, 5 GB taken so far: 34 of 1 GB and one of 3 GB still to come.
        std::fs::write(a.done_path(0), (0..5).map(|i| format!("pack 6/{i}/0\n")).collect::<String>()).unwrap();
        a.sampler.set_test(Some(vec![None, None]), Some(false));
        a.sampler.set_test_disk(Some(60 * g), Some(vec![Some(5 * g), None]));
        a.sampler.tick();
        assert_eq!(a.disk_left(0), (34 * 1024 + 3072) << 20);
        let unit = JobSpec { id: "unit 6/9/9".into(), what: "a unit".into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "unit".into(), targets: vec![("6/9/9".into(), "k".into())] }) };
        // 10 GB, 37 GB to come and the 30 kept free: past 60 GB free (its largest alone, 3 GB, would
        // have let it start, and the guard give it back an hour on, and again).
        a.free_set = Some(60 * g);
        assert!(a.fits_beside(&unit, 0).is_some_and(|w| w.contains("still to come")), "{:?}", a.fits_beside(&unit, 0));
        a.free_set = Some(90 * g);
        assert_eq!(a.fits_beside(&unit, 0), None);
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.slots[0].running = None;
        // A floor far past the disk (stale, or an outlier's): alone, the first slot starts it all the
        // same once room-making has done what it can.
        c_disks_huge(a.coord.as_ref().unwrap());
        a.free_set = Some(40 * g);
        assert!(a.try_start(0, unit.clone(), &cond, Some(&root), &mut Vec::new()), "started alone, the guard its backstop");
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.end_lease(0, Outcome::Interrupted, &[], "stopped");
        a.slots[0].running = None;
        // Short of the reserve itself, no target: it waits, said why (started, the disk guard would
        // freeze it at once and give it back, and the next job likewise: the two-Mac run's loop).
        a.free_set = Some(room::RESERVE - g);
        // (Its scratch kept from a give-back of its own, and another's: room-making takes only the
        // other's, the job's own its again before room is made.)
        let (mine, other) = (a.o.home.join("scratch/unit"), a.o.home.join("scratch/pack"));
        for p in [&mine, &other] {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(p.join("work.bin"), b"kept").unwrap();
            room::given_back_add(&a.o.home, p, 1);
        }
        let mut scratched = unit.clone();
        scratched.cmd.extend(["--scratch".into(), mine.to_string_lossy().into_owned()]);
        let mut w = Vec::new();
        assert!(!a.try_start(0, scratched, &cond, Some(&root), &mut w));
        assert!(a.slots[0].running.is_none() && w.iter().any(|x| x.why.contains("waits for room on the disk")), "{w:?}");
        assert!(mine.exists() && !other.exists(), "its own scratch kept, the other's freed for room");
        a.floor_short = None;
        // So too with the owner's room target set: alone, the reserve and the target are what it
        // starts with, not its floor past them.
        a.room_target = Some(room::Target { bytes: 20 * g, by: "a test".into(), at: now_s() });
        a.free_set = Some(room::RESERVE + 21 * g);
        let mut w = Vec::new();
        assert!(a.try_start(0, unit.clone(), &cond, Some(&root), &mut w), "started alone past a high floor, the target kept: {w:?}");
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
    }

    fn c_disks_huge(c: &crate::coord::Coordinator) {
        c.add_disks(&[(crate::coord::cost_key("unit", "6/9/9"), 500 << 10)]);
    }

    #[test]
    fn the_memory_guard_drains_the_job_beside_then_stops_the_one_past_the_limit_and_learns() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
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
        assert!(!a.guard() && a.slots[1].mem_drain.is_none());
        // Together past it, the larger fitting alone: the one beside it stops at its next safe point
        // (asked through its channel), and nothing starts beside the larger meanwhile.
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert!(!a.guard());
        assert!(a.slots[1].mem_drain.as_deref().is_some_and(|w| w.contains("past this Mac's limit of 12.0 GB")), "{:?}", a.slots[1].mem_drain);
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert_eq!(std::fs::read_to_string(a.control_path(1)).unwrap(), "drain");
        a.start_second(&[job("unit 6/3/3", &["6/3/3"])], &cond, Some(&root));
        assert!(a.beside_why.as_deref().is_some_and(|w| w.contains("nothing starts beside terrain 6/1/1")), "{:?}", a.beside_why);
        // The drained one reaching no safe point in the time a pause gives: stopped at once.
        a.grace = Duration::ZERO;
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert!(a.guard() && a.slots[1].running.is_none());
        a.grace = DRAIN_GRACE;
        // The larger past the limit by itself, the Mac not short of memory: it stops at its next safe
        // point; short of it, at once (the sampler froze it), given back, not failed, kept from this
        // Mac for an hour; what it held its target's floor, a batch's (two targets).
        tick(&mut a, vec![Some(13 << 10), None], false);
        assert!(!a.guard() && a.slots[0].mem_drain.is_some() && a.slots[0].running.is_some());
        // (Past the time a pause gives a safe point, not short of memory: still only drained; a long
        // job alone may have none for hours.)
        a.grace = Duration::ZERO;
        tick(&mut a, vec![Some(13 << 10), None], false);
        assert!(!a.guard() && a.slots[0].running.is_some());
        tick(&mut a, vec![Some(13 << 10), None], true);
        assert!(a.sampler.frozen(0));
        assert!(a.guard() && a.slots[0].running.is_none());
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
        assert!(a.guard());
        let c = a.coord.as_ref().unwrap();
        assert_eq!(c.floor("terrain", "6/1/1").map(|f| (f.mb, f.alone)), Some((14 << 10, true)));
        let h = a.guard_holds(&works);
        assert!(h.here.contains(&("terrain".to_string(), "6/1/1".to_string())) && h.here.len() == 1 && h.all.len() == 1 && h.alone.is_empty());
        assert!(h.why[0].why.contains("more than any Mac in the pool has"), "{:?}", h.why);
        let w = crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{}", c.contact.urls[0].rsplit(':').next().unwrap())], c.contact.token.clone(), "big");
        let _ = w.ask(&crate::coord::Ask { kind: "native".into(), label: Some("big (helper)".into()), can: vec!["terrain".into()], mem_mb: 30 << 10, limit_mb: Some(42 << 10), ..Default::default() });
        let h = a.guard_holds(&works);
        assert!(h.here.len() == 1 && h.all.is_empty() && h.why[0].why.contains("left to a Mac with room"), "{:?}", h.why);
        // A step only the lead runs (heritage), its floor past the lead's limit: judged against the
        // lead's limit, held, the status naming the member whose limit fits it and how to hand it
        // the lead; past every member's, no member named.
        let c = a.coord.as_ref().unwrap();
        let lead_only = vec![build::Work { step: "heritage".into(), targets: vec![("heritage".into(), "k".into())] }];
        let floor = |mb| vec![(crate::coord::cost_key("heritage", "heritage"), memguard::Floor { mb, alone: true, v: crate::coord::cost_version("heritage") })];
        c.add_floors(&floor(14 << 10));
        let h = a.guard_holds(&lead_only);
        assert!(h.all.len() == 1, "{:?}", h.why);
        let why = &h.why[0].why;
        assert!(why.contains("past the lead's limit of 12.0 GB; this step runs only on the lead") && why.contains("big's limit is 42.0 GB") && why.contains("`scenic lead give big`") && !why.contains("(helper)") && !why.contains("more than any Mac"), "{why}");
        c.add_floors(&floor(50 << 10));
        let why = &a.guard_holds(&lead_only).why[0].why;
        assert!(why.contains("past the lead's limit of 12.0 GB") && why.contains("no member's limit fits it") && !why.contains("lead give"), "{why}");
        c.floors(true, Some("heritage"), None);
        // The job beside the larger drained when the larger is the second slot's: the first slot
        // starts nothing until the larger ends.
        assert!(a.try_start(1, job("bldtiles 6/5/5", &["6/5/5"]), &cond, Some(&root), &mut Vec::new()));
        assert!(a.try_start(0, job("pack 6/4/4", &["6/4/4"]), &cond, Some(&root), &mut Vec::new()));
        tick(&mut a, vec![Some(4 << 10), Some(9 << 10)], false);
        assert!(!a.guard() && a.slots[0].mem_drain.is_some());
        assert!(a.waits_for_second(&job("unit 6/3/3", &["6/3/3"])).is_some_and(|w| w.contains("held more memory together")));
        // This Mac's memory unknown: nothing stopped, the status saying why.
        a.mem_set = Some((0, 0));
        tick(&mut a, vec![Some(20 << 10), Some(20 << 10)], true);
        assert!(!a.guard() && a.slots[0].running.is_some() && a.slots[1].running.is_some());
        assert!(a.guard_view().why_off.is_some_and(|w| w.contains("can't be read")));
        // The switch off: nothing stopped, nothing held, a job the sampler froze let go on; what's
        // learned is kept all the same.
        a.mem_set = Some((16 << 10, 12 << 10));
        std::fs::create_dir_all(root.join("state/pool")).unwrap();
        std::fs::write(root.join(memguard::SWITCH), "off").unwrap();
        a.guard_on = memguard::on(&root);
        tick(&mut a, vec![None, Some(20 << 10)], true);
        assert!(!a.guard() && a.slots[1].running.is_some());
        assert!(a.guard_holds(&works).here.is_empty());
        for k in 0..SLOTS {
            if let Some(r) = a.slots[k].running.as_mut() {
                r.stop(Duration::from_secs(5));
            }
        }
    }

    #[test]
    fn a_map_updates_catalog_made_ends_it_and_a_dry_run_keeps_its_map_updates_to_itself() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: false, once: true, helper: false }).unwrap();
        let r = build::MapUpdate { began: 100, regions: vec!["a".into()], last: false, units: [("base/6-1-1".to_string(), "base/6-1-1.1111111111111111.base".to_string())].into(), over: false, low: true, low_at: None };
        a.keep_map_update(r.clone()).unwrap();
        let file = || -> build::MapUpdate { serde_json::from_slice(&std::fs::read(home.join(MAP_UPDATE_FILE)).unwrap()).unwrap() };
        // (It makes the low zooms: noted as when they were last made.)
        assert_eq!(file(), build::MapUpdate { low_at: Some(100), ..r.clone() });
        // Its catalog made: over, kept without its units (whatever changed meanwhile goes out with
        // the next).
        assert!(!file().over);
        a.end_update();
        let f = file();
        assert!(f.over && f.units.is_empty() && f.began == 100);
        // A dry run beside it (a second agent on this Mac, planning only): its map updates its own, never
        // the file the real one's jobs read.
        let dry = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: PathBuf::from("/app"), dry_run: true, once: true, helper: false }).unwrap();
        dry.keep_map_update(build::MapUpdate { began: 200, ..r.clone() }).unwrap();
        assert_eq!(file().began, 100);
        // The next map update leaving the low zooms (crate::agent::publish): when they were last made carried.
        a.keep_map_update(build::MapUpdate { began: 300, low: false, ..r.clone() }).unwrap();
        assert_eq!((file().began, file().low_at), (300, Some(100)));
        a.keep_map_update(build::MapUpdate { began: 400, ..r }).unwrap();
        assert_eq!(file().low_at, Some(400));
        // (A file from before the low zooms later: it made the low zooms, none noted.)
        let old: build::MapUpdate = serde_json::from_str(r#"{"began":1,"regions":[],"last":false,"units":{}}"#).unwrap();
        assert!(old.low && old.low_at.is_none());
    }

    #[test]
    fn a_map_update_shared_the_leads_own_job_of_its_map_tiles_takes_its_share() {
        let w = |step: &str, n: usize| build::Work { step: step.into(), targets: (0..n).map(|i| (format!("6/1/{i}"), "k".into())).collect() };
        let sizes = |b: Vec<(build::Work, usize)>| b.into_iter().map(|(w, t)| (w.step, w.targets.len(), t)).collect::<Vec<_>>();
        // (One member asking: half each, the lead's from the front; the other steps as they were.)
        let share = |step: &str, total: usize, n: usize| if publish::SHARED_IN_UPDATE.contains(&step) { publish::lead_share(total, n, 1) } else { n };
        assert_eq!(sizes(batches_sized(vec![w("pack", 10), w("roadunits", 1)], &share)), [("pack".to_string(), 5, 10), ("pack".to_string(), 5, 10), ("roadunits".to_string(), 1, 1)]);
        assert_eq!(sizes(batches(vec![w("pack", 10)])), [("pack".to_string(), 10, 10)]);
    }

    /// An agent that runs jobs (no dry run), the NAS at `root`: the lead's, or a member's.
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
    fn the_lead_trims_once_the_build_is_done_and_again_after_work() {
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
    fn the_lead_sends_the_passs_answers_to_the_nas_once_as_it_starts() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        put(&root.join("sources/osm/2026-09-28/pass.0000000000000001.json"), b"{}");
        let archive = root.join("sources/items/2026-09-28/answers.tar.zst");
        for m in ["m1", "m4"] {
            put(&d.path().join(m).join("cache/items/facts-2026-09-28.jsonl"), b"{\"qid\":\"Q1\"}\n");
        }
        // A member's: never (the steps that keep them are the lead's).
        let mut helper = running_agent(&root, &d.path().join("m1"), true);
        helper.seed_answers(Some(&root));
        assert!(!archive.exists());
        // The lead's: not while a job runs here, nor without the NAS; then once.
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
        // (While it runs, the steps that keep the answers wait on its hold of them here, their
        // own: crate::answers::Local.)
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
        a.caches_task = Some(CachesTask { ask: None, toward: None, disk: false, began: Instant::now(), thread: std::thread::spawn(move || wait.recv().map(|()| room::Freed { freed: BTreeMap::from([("blobs".to_string(), 1000)]), ..Default::default() }).map_err(anyhow::Error::from)) });
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
        let mut left = store::spawn::spawn(store::spawn::command("/bin/sh").args(["-c", "sleep 3600"]).process_group(0)).unwrap();
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
        a.caches_task = Some(CachesTask { ask: None, toward: None, disk: false, began: Instant::now(), thread: std::thread::spawn(move || wait.recv().map(|()| room::Freed::default()).map_err(anyhow::Error::from)) });
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
    fn a_member_trims_once_the_leads_heartbeat_says_the_build_is_done() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/writer"), "the-lead").unwrap();
        let c = home.join("cache");
        let tif = crate::whole::testfiles::tiff(false);
        put(&c.join("chm10/a.tif"), &tif);
        put(&root.join("sources/canopy/a.tif"), &tif);
        put(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), &[1; 1000]);
        let mut a = running_agent(&root, &home, true);
        let beat = |at: u64, done: bool, job: Option<&str>| {
            let job = job.map(|id| JobView { id: id.into(), what: format!("Ranking the world's place labels ({id})"), started: at, paused: None, pausing: None, tail: String::new(), parts: Vec::new(), part: None, progress: None, mem_mb: None, threads: None, waiting: None });
            let st = Status { host: "the-lead".into(), beat: at, job, forecast: Some(forecast::Forecast { at, ..forecast_now(done) }), ..Default::default() };
            std::fs::write(root.join("state/status.json"), serde_json::to_vec(&st).unwrap()).unwrap();
        };
        // No word from the lead: nothing goes.
        assert_eq!(a.tend_caches(Some(&root), true).as_deref(), Some("the lead's status can't be read now"));
        // Its heartbeat an hour old, the build with work left, or a job of the lead's running
        // (its forecast "done" all the same, as an older agent's said with a lone worldwide job):
        // still nothing.
        beat(now_s() - 3600, true, None);
        a.heard = None;
        assert_eq!(a.tend_caches(Some(&root), true).as_deref(), Some("the lead hasn't been heard from for 60 min"));
        beat(now_s(), false, None);
        a.heard = None;
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.starts_with("the build has work left")));
        beat(now_s(), true, Some("labels 2026-09-28"));
        a.heard = None;
        assert!(a.tend_caches(Some(&root), true).is_some_and(|w| w.starts_with("the lead runs a job (Ranking")));
        assert!(c.join("chm10/a.tif").exists());
        // Fresh, and the build done: every cheap cache goes, the canopy squares too.
        beat(now_s(), true, None);
        a.heard = None;
        assert_eq!(a.tend_caches(Some(&root), true), None);
        assert!(!c.join("chm10/a.tif").exists() && !c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack").exists());
        assert_eq!(a.mem.trimmed.as_ref().map(|f| (f.bytes(), f.left)), Some((tif.len() as u64 + 1000, 0)));
        // (Its own status says so: the lead notes it in the history from there.)
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
        let mut left = store::spawn::spawn(store::spawn::command("/bin/sh").args(["-c", "sleep 3600"]).process_group(0)).unwrap();
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
    fn the_lead_notes_a_members_trims_in_the_history_once() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = running_agent(&root, &home, false);
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
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
        rekey_root_with(root, [-220_000_000, 640_000_000, -217_000_000, 641_600_000])
    }

    /// `rekey_root`, the unit's ways where `owned` says (E7).
    fn rekey_root_with(root: &Path, owned: [i32; 4]) -> String {
        let date = "2026-09-28";
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        put(&root.join(format!("sources/osm/{date}/pass.1111111111111111.json")), b"{}");
        let recipes = vec![recipes::Recipe { id: "a".into(), name: "A".into(), outline: vec!["place:-21.9,64.13,20".into()] }];
        let cov = crate::coverage::Coverage::fixture(&recipes, None, &root.join("inputs/outlines")).unwrap();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        crate::inputs::regions::accept_fixture(root, &root.join("inputs/outlines"), &recipes, &mut m);
        let piece = format!("sources/osm/{date}/pieces/6-28-17");
        m.insert(piece.clone(), format!("{piece}.4444444444444444.osm.pbf"));
        let mut reaches = crate::reach::Reaches { fmt: 1, date: date.into(), ..Default::default() };
        reaches.units.insert("6/28/17".into(), crate::reach::Reach { owned: Some(owned), long: vec![] });
        let rc = format!("{}.5555555555555555.json.zst", crate::reach::logical(date));
        put(&root.join(&rc), &reaches.encode().unwrap());
        m.insert(crate::reach::logical(date), rc);
        // (What the units wait for: the heritage sites' inputs and outputs, the buildings' index.)
        m.insert(crate::osmpass::set_name(date, "areas"), format!("sources/osm/{date}/sets/areas.1212121212121212.osm.pbf"));
        crate::inputs::heritage::accept_fixture(root, &mut m);
        crate::inputs::elevation::accept_fixture(root, &mut m);
        m.insert(crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE), format!("{}.6767676767676767.json", crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE)));
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
        keys.record("heritage-sites", &build::heritage_sites_work(&cov, date, &m, &keys, &input_digests(root)).unwrap().targets);
        keys.record("terrain", &rekey::v1::terrain_slope_targets(&cov, &m).0);
        keys.record("trees", &rekey::v1::trees_targets(&cov, &m));
        let old = rekey::v1::unit_keys(&cov, date, &m, Some(&reaches), &input_digests(root), crate::buildtiles::TEST_RELEASE).pop().unwrap();
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
        let plan = build::plan(&cov, date, &m, &read, &input_digests(root), Some(&reaches), &tiles, build::MapUpdates { each: &cov.by_region(), on_map: &BTreeMap::new(), since_last: None, current: None, held: false, way: Default::default() });
        assert!(plan.work.iter().any(|w| w.step == "unit" && w.targets[0].0 == "6/28/17"), "{:?}", plan.work);
        keys.save(root).unwrap();
        old.1
    }

    /// Not a test: seeds the scratch folder `SCENIC_SCRATCH_MAP_UPDATE` names (never the project's) with
    /// a region done that the map hasn't, so its lead begins a map update (tools/check/pool-two-macs.sh
    /// `map-update`): `rekey_root`'s state, the unit's ways across four z6 tiles (map tiles to share), and
    /// the unit built as the plan wants it now.
    #[test]
    #[ignore = "seeds a scratch folder for tools/check/pool-two-macs.sh map-update"]
    fn seed_a_scratch_map_update() {
        let Some(root) = std::env::var_os("SCENIC_SCRATCH_MAP_UPDATE").map(PathBuf::from) else { return };
        assert!(!root.to_string_lossy().contains("projects/scenic-roads"), "not the project folder");
        let date = "2026-09-28";
        rekey_root_with(&root, [-220_000_000, 640_000_000, -10_000_000, 641_600_000]);
        let m: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(root.join("state/build/manifest.json")).unwrap()).unwrap();
        let cov = crate::coverage::Coverage::accepted(&root, &crate::inputs::regions::read::source(&root, &m).unwrap(), None).unwrap();
        let reaches = crate::reach::Reaches::load(&root, &m, date).ok().flatten().expect("the reaches");
        let mut keys = build::Keys::load(&root);
        let tiles = {
            let mut t = tiles::TerrainTiles::new(None);
            t.load(&root, &m);
            t
        };
        let mut read = keys.clone();
        rekey::derive(&mut read, &cov, &m, &tiles);
        let plan = build::plan(&cov, date, &m, &read, &input_digests(&root), Some(&reaches), &tiles, build::MapUpdates { each: &cov.by_region(), on_map: &BTreeMap::new(), since_last: None, current: None, held: false, way: Default::default() });
        let unit = plan.work.iter().find(|w| w.step == "unit").expect("the unit to build").targets.clone();
        keys.record("unit", &unit);
        keys.save(&root).unwrap();
        eprintln!("seeded {}: unit {:?} built", root.display(), unit);
    }

    #[test]
    fn the_passs_outlines_come_first_and_hold_the_rest() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        rekey_root(&root);
        let date = "2026-09-28";
        let mf = root.join("state/build/manifest.json");
        let mut m: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(&mf).unwrap()).unwrap();
        m.insert(crate::osmpass::set_name(date, "outlines"), format!("sources/osm/{date}/sets/outlines.2323232323232323.osm.pbf"));
        // (Outlines the pass made itself, before the step: no record of the step's.)
        m.insert(format!("sources/osm/{date}/outlines"), format!("sources/osm/{date}/outlines.2424242424242424.sect"));
        put(&mf, &serde_json::to_vec(&m).unwrap());
        let a = agent(&root, &d.path().join("home"));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let ids = |plan: &[JobSpec]| plan.iter().map(|j| j.id.clone()).collect::<Vec<_>>();
        let mut w = Vec::new();
        let plan = a.plan(&root, &cond, &mut w);
        let o = plan.iter().find(|j| j.id == format!("outlines {date}")).unwrap_or_else(|| panic!("{:?}", ids(&plan)));
        assert_eq!(o.record.as_ref().map(|r| r.step.as_str()), Some("outlines"));
        assert!(o.cmd[1] == "outlines" && o.cmd.windows(2).any(|p| p == ["--pass", date]), "{:?}", o.cmd);
        // Nothing else of the build meanwhile (the z8 terrain, which waits for nothing else, among it).
        assert!(!ids(&plan).iter().any(|i| i.starts_with("terrain-z8") || i.starts_with("unit ") || i.starts_with("spoken") || i.starts_with("lakefill")), "{:?}", ids(&plan));
        assert!(w.iter().any(|x| x.why.contains("outlines job")), "{w:?}");
        // Made under its key: the rest comes back, and no outlines job.
        let mut keys = build::Keys::load(&root);
        keys.record("outlines", &build::outlines_work(date, &m, &keys).unwrap().targets);
        keys.save(&root).unwrap();
        let plan = a.plan(&root, &cond, &mut Vec::new());
        assert!(!ids(&plan).iter().any(|i| i.starts_with("outlines")) && ids(&plan).iter().any(|i| i.starts_with("terrain-z8")), "{:?}", ids(&plan));
    }


    #[test]
    fn the_checklist_says_which_steps_helpers_take() {
        let steps = build::fold(&build::checklist_to_come(), &BTreeMap::new(), &BTreeMap::new());
        let shared = |id: &str| steps.iter().find(|s| s.id == id).and_then(|s| s.shared.clone());
        assert_eq!(shared("terrain").as_deref(), Some("Terrain"), "the pieces, not the assemblies");
        assert_eq!(shared("slope").as_deref(), Some("Slope"));
        assert_eq!(shared("roads").as_deref(), Some("all"));
        assert_eq!(shared("landmarks").as_deref(), Some("Landmark candidates and Peaks"));
        assert_eq!(shared("trees").as_deref(), Some("Tree cover"), "the pieces, not the assemblies");
        assert_eq!(shared("tiles"), None);
        assert_eq!(shared("publish"), None);
    }

    #[test]
    fn an_app_older_than_the_regions_switch_plans_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        let sw = crate::inputs::switched_logical(crate::inputs::regions::UNIT);
        let name = format!("{sw}.1111111111111111.json");
        put(&root.join(&name), br#"{"same": true, "app": "29991231-0000-aaaaaaa"}"#);
        put(&root.join("state/build/manifest.json"), &serde_json::to_vec(&BTreeMap::from([(sw.clone(), name)])).unwrap());
        let a = agent(&root, &home);
        let (mut w, mut held) = (Vec::new(), Holds::default());
        assert!(a.region_jobs(&root, None, None, &mut w, &mut held).is_empty());
        assert!(w.iter().any(|x| x.why.contains("older than the one the newer readers' switch (the regions', the elevation datasets') was recorded on")), "{w:?}");
        // Its file unreadable: unknown, so nothing either.
        put(&root.join("state/build/manifest.json"), &serde_json::to_vec(&BTreeMap::from([(sw, "gone.json".to_string())])).unwrap());
        let mut w = Vec::new();
        assert!(a.region_jobs(&root, None, None, &mut w, &mut held).is_empty());
        assert!(w.iter().any(|x| x.why.contains("can't be read now")), "{w:?}");
    }

    #[test]
    fn plans_daily_jobs_and_writes_a_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let mut m = BTreeMap::new();
        crate::inputs::regions::accept_fixture(&root, &root, &[recipes::Recipe { id: "x".into(), name: "X".into(), outline: vec!["osm:1".into()] }], &mut m);
        put(&root.join("state/build/manifest.json"), &serde_json::to_vec(&m).unwrap());
        let mut a = agent(&root, &home);
        a.step().unwrap();
        let st = read_status(Some(&root), &home).unwrap();
        assert_eq!(st.regions.len(), 1);
        assert!(st.conditions.nas);
        // (The regions' check of their accepted version comes first, the gate's jobs first of all;
        // the backup after it.)
        assert!(st.waiting.iter().any(|w| w.what.starts_with("Backing up") || w.what.starts_with("Checking the inputs dropped in regions")), "{:?}", st.waiting);
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
        let mut out = crate::out::Out::unbound(&root, &d.path().join("s")).unwrap();
        crate::inputs::gate::run(&mut out, unit, true, None).unwrap();
        let mut keys = build::Keys::load_strict(&root).unwrap();
        keys.record("inputs", &work.targets);
        keys.save(&root).unwrap();
        // (The full check's time is the records', not this agent's: no daily full check due now,
        // for this agent or a new lead's, `b`.)
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
        drop(a);
        let mut b = agent(&root, &d.path().join("home-b"));
        b.step().unwrap();
        let t = Instant::now();
        while watch(&b).is_none() {
            assert!(t.elapsed() < Duration::from_secs(30), "listed");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(b.plan(&root, &c, &mut w).iter().filter(|j| j.id.starts_with("inputs ")).all(|j| !j.id.ends_with(" full")), "a new lead doesn't repeat a full check less than a day old");
        // Off the gate: no longer shown, nor checked; its records forgotten when asked, by a job of
        // the lead's.
        std::fs::remove_file(root.join(crate::inputs::TEST_FLAG)).unwrap();
        b.step().unwrap();
        assert!(read_status(Some(&root), &d.path().join("home-b")).unwrap().inputs.is_empty());
        b.o.dry_run = false;
        crate::inputs::ask(&d.path().join("home-b"), &crate::inputs::Ask { unit: unit.into(), forget: true, ..Default::default() }).unwrap();
        b.tend_gate(&root);
        b.o.dry_run = true;
        let plan = b.plan(&root, &c, &mut w);
        let f = plan.iter().find(|j| j.id == format!("inputs {unit} forget")).expect("a job forgetting its records");
        assert!(f.cmd.ends_with(&["--forget".to_string()]));
        let mut out = crate::out::Out::unbound(&root, &d.path().join("s")).unwrap();
        crate::inputs::gate::forget(&mut out, unit).unwrap();
        let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
        assert!(!m.keys().any(|l| crate::inputs::unit_of(l) == Some(unit)));
        assert!(!b.plan(&root, &c, &mut w).iter().any(|j| j.id.starts_with("inputs ")), "nothing left to forget");
    }

    #[test]
    fn an_edit_to_the_regions_is_noticed() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        let r = |p: &str| vec![recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec![format!("place:{p},20")] }];
        let mut m1 = BTreeMap::new();
        crate::inputs::regions::accept_fixture(&root, &root, &r("-21.9,64.13"), &mut m1);
        let mut m2 = BTreeMap::new();
        crate::inputs::regions::accept_fixture(&root, &root, &r("-21.9,64.2"), &mut m2);
        // The agent: the first look isn't an edit; a new accepted version is (the plan's look, not
        // the heartbeat's).
        let a = agent(&root, &home);
        a.coverage(&root, &m1, "2026-09-28", &[], true).unwrap();
        a.coverage(&root, &m1, "2026-09-28", &[], true).unwrap();
        a.coverage(&root, &m1, "2026-09-28", &[], false).unwrap();
        assert!(a.edited_at.get().is_none());
        a.coverage(&root, &m2, "2026-09-28", &[], true).unwrap();
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
    fn a_newer_app_stops_the_running_jobs_at_their_safe_points_then_restarts() {
        // (Task #210: a Mac that had handed the lead over for its update, a member then, ran its
        // jobs to their ends, hours, before restarting. Whatever part it plays, its jobs are asked to
        // stop at their next safe points as a pause asks.)
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let apps = d.path().join("app");
        std::fs::create_dir_all(apps.join("v1")).unwrap();
        std::fs::create_dir_all(apps.join("v2")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        let mut a = test_agent(Options { root: Some(root.clone()), home: home.clone(), bin: apps.join("v1"), dry_run: false, once: true, helper: false }).unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str, t: &str| JobSpec { id: id.into(), what: id.into(), cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "bldtiles".into(), targets: vec![(t.into(), "k".into())] }) };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        // (The two `sleep`s killed whatever the test's end, an assertion failing too.)
        struct Kill(Vec<i32>);
        impl Drop for Kill {
            fn drop(&mut self) {
                for pid in &self.0 {
                    unsafe { libc::kill(-*pid, libc::SIGKILL) };
                }
            }
        }
        assert!(a.try_start(0, job("bldtiles 6/1/1", "6/1/1"), &cond, Some(&root), &mut Vec::new()));
        assert!(a.try_start(1, job("bldtiles 6/2/2", "6/2/2"), &cond, Some(&root), &mut Vec::new()));
        let _kill = Kill(a.slots.iter().map(|s| s.running.as_ref().unwrap().pgid).collect());
        let tend = |a: &mut Agent| {
            for k in 0..SLOTS {
                a.tend(k, &cond, Some(&root), 0).unwrap();
            }
        };
        // Its own app current: the jobs go on, no restart.
        tend(&mut a);
        assert!(a.slots.iter().all(|s| s.running.as_ref().is_some_and(|r| r.pausing.is_none())) && !a.update_due());
        // A newer app installed: both asked to stop at their next safe points; no restart yet.
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v2", apps.join("current")).unwrap();
        tend(&mut a);
        for k in 0..SLOTS {
            assert_eq!(std::fs::read_to_string(a.control_path(k)).unwrap(), "drain");
            assert_eq!(a.slots[k].running.as_ref().unwrap().pausing.as_deref(), Some("restarting into the newly installed app (v2)"));
        }
        assert!(!a.update_due());
        // Past the grace with no safe point: not frozen (it would never end), the restart due; the
        // loop stops it, its finished targets kept.
        a.grace = Duration::ZERO;
        tend(&mut a);
        assert!(a.slots[0].running.as_ref().is_some_and(|r| r.paused.is_none()));
        assert!(a.update_due());
        // The install undone (current back on its own): the jobs told to go on, no restart.
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        tend(&mut a);
        assert!(!a.update_due());
        for k in 0..SLOTS {
            assert_eq!(std::fs::read_to_string(a.control_path(k)).unwrap(), "run");
            assert!(a.slots[k].running.as_ref().unwrap().pausing.is_none());
        }
        // The first slot free with a newer app: due at once.
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v2", apps.join("current")).unwrap();
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        assert!(a.update_due());
        a.slots[1].running.as_mut().unwrap().stop(Duration::from_secs(5));
    }

    #[test]
    fn osm_pass_waits_for_space_or_runs() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("sources/osm/2026-09-28")).unwrap();
        std::fs::write(root.join("sources/osm/2026-09-28/planet.osm.pbf"), b"x").unwrap();
        std::fs::create_dir_all(root.join("sources/basemap")).unwrap();
        std::fs::write(root.join("sources/basemap/planetiler.jar"), b"x").unwrap();
        let mut a = agent(&root, &home);
        // (The disk as the test says, both ways: not this Mac's.)
        a.free_set = Some(PASS_SPACE);
        let mut w = Vec::new();
        let plan = a.plan(&root, &Conditions { ac: true, nas: true, idle_s: 0, ..Default::default() }, &mut w);
        assert!(plan.iter().any(|j| j.id == "osm-pass 2026-09-28"));
        a.free_set = Some(PASS_SPACE / 4);
        let mut w = Vec::new();
        let plan = a.plan(&root, &Conditions { ac: true, nas: true, idle_s: 0, ..Default::default() }, &mut w);
        assert!(!plan.iter().any(|j| j.id == "osm-pass 2026-09-28") && w.iter().any(|x| x.why.contains("GB free")), "{w:?}");
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

    /// While the pass's lake fill is being made, the jobs that read it wait, said in the checklist;
    /// once it's made, or after its failures, they're queued.
    #[test]
    fn the_jobs_reading_the_lake_fill_wait_for_it() {
        let spec = |id: &str| JobSpec { id: id.into(), what: format!("doing {id}"), cmd: Vec::new(), needs: Needs { nas: true }, restart_after_sleep: true, record: None };
        let jobs = || ["lakefill 2026-09-28", "labels 2026-09-28", "water 2026-09-28", "terrain-water terrain-water", "terrain 6/18/20", "catalog catalog", "catalog-held catalog-held", "unit 6/1/2"].map(spec).to_vec();
        let ids = |v: &[JobSpec]| v.iter().map(|j| j.id.clone()).collect::<Vec<_>>();
        let mut waiting = Vec::new();
        let kept = hold_for_lakefill(jobs(), true, &mut waiting);
        assert_eq!(ids(&kept), ["lakefill 2026-09-28", "labels 2026-09-28", "terrain 6/18/20", "unit 6/1/2"]);
        assert_eq!(waiting.iter().map(|w| w.step.clone().unwrap()).collect::<Vec<_>>(), ["water", "terrain-water", "catalog", "catalog-held"]);
        assert!(waiting.iter().all(|w| w.why.contains("lakefill")));
        // Not held (the fill made, or failed LAKEFILL_HOLDS times): all queued, nothing said.
        let mut waiting = Vec::new();
        assert_eq!(ids(&hold_for_lakefill(jobs(), false, &mut waiting)), ids(&jobs()));
        assert!(waiting.is_empty());
        // What holds: the lakefill job's work, until its failures reach LAKEFILL_HOLDS.
        assert!(build::water_waits_for_lakefill(true, build::LAKEFILL_HOLDS - 1) && !build::water_waits_for_lakefill(true, build::LAKEFILL_HOLDS));
    }

    /// Before the source declarations' first accepted version, the jobs that read them wait, said in
    /// the checklist; the rest go on.
    #[test]
    fn the_jobs_reading_the_declared_sources_wait_for_them() {
        let spec = |id: &str| JobSpec { id: id.into(), what: format!("doing {id}"), cmd: Vec::new(), needs: Needs { nas: true }, restart_after_sleep: true, record: None };
        let jobs = || ["labels 2026-09-28", "unit 6/1/2", "terrain 6/18/20", "trees 6/3/4", "items 2026-09-28", "heritage 2026-09-28", "heritage-sites 2026-09-28"].map(spec).to_vec();
        let ids = |v: &[JobSpec]| v.iter().map(|j| j.id.clone()).collect::<Vec<_>>();
        let mut waiting = Vec::new();
        assert_eq!(ids(&hold_for_sources(jobs(), true, &mut waiting)), ["labels 2026-09-28", "terrain 6/18/20", "heritage-sites 2026-09-28"]);
        assert_eq!(waiting.iter().map(|w| w.step.clone().unwrap()).collect::<Vec<_>>(), ["unit", "trees", "items", "heritage"]);
        assert!(waiting.iter().all(|w| w.why.contains("inputs/sources/")));
        let mut waiting = Vec::new();
        assert_eq!(ids(&hold_for_sources(jobs(), false, &mut waiting)), ids(&jobs()));
        assert!(waiting.is_empty());
    }

    #[test]
    fn the_checklist_says_why_a_step_waits() {
        let step = |what: &str, steps: &[&str], left: usize| build::Line { id: what.to_lowercase(), what: what.into(), steps: steps.iter().map(|s| s.to_string()).collect(), left: Some(left), ..Default::default() };
        let list = || vec![step("Pass", &["pass-sets", "reach"], 1), step("Roads", &["unit"], 3), step("Landmarks", &["pois", "peaks", "items", "heritage", "marks", "overlays"], 2), step("Publishing", &["catalog", "catalog-held"], 0)];
        let helper = Status { host: "m1".into(), job: Some(JobView { id: "unit 6/1/2".into(), what: String::new(), started: 0, pausing: None, paused: None, tail: String::new(), progress: None, parts: Vec::new(), part: None, mem_mb: None, threads: None, waiting: None }), ..Default::default() };
        let waiting = [Waiting { step: Some("reach".into()), what: "How far…".into(), why: "away from home".into() }];
        let mut l = list();
        annotate(&mut l, &["heritage".to_string()], &[helper], &waiting);
        assert_eq!(l[0].note.as_deref(), Some("away from home"));
        assert_eq!(l[1].note.as_deref(), Some("on m1"));
        assert_eq!(l[2].note, None, "this Mac's job now");
        // Publishing waits for the steps above, though its catalog is current.
        assert!(!l[3].finished() && l[3].note.as_deref().is_some_and(|n| n.starts_with("as each region is done")));
        // All above done: done.
        let mut l: Vec<build::Line> = list().into_iter().map(|mut s| {
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
            "#!/bin/sh\nstep=\"$1\"\nenv > \"{bin}/env-$step\"\n[ -n \"$SCENIC_UNITS_AS_OF\" ] && cp \"${{SCENIC_UNITS_AS_OF%#*}}\" \"{bin}/as-of-$step\"\nif [ -n \"$SCENIC_HANDOFF\" ]; then\n  mkdir -p \"$SCENIC_HANDOFF\"\n  case \"$step\" in\n    slope) printf '{{\"changes\":{{\"layers/slope/lo/3-2-2\":\"layers/slope/lo/3-2-2.0123456789abcdef.pack\"}}}}' > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n    pack) printf '{{\"changes\":{{\"layers/roads/hi/6-1-1\":\"layers/roads/hi/6-1-1.0123456789abcdef.pack\"}}}}' > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n    *) printf '{{\"changes\":{{}},\"pending\":{{\"work/%s.0123456789abcdef.json\":\"aa\"}}}}' \"$step\" > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n  esac\nfi\nexit 0\n",
            bin = bin.display()
        );
        for p in ["scenic", "scenic-build"] {
            std::fs::write(bin.join(p), &script).unwrap();
            std::fs::set_permissions(bin.join(p), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        bin
    }

    /// Waits until process `pid` (a child of this one) has exited, leaving it to be reaped (waitid
    /// with WNOWAIT): no clock decides it.
    fn exited(pid: i32) {
        // SAFETY: waitid on a child of this process, into a zeroed siginfo it owns.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
        assert_eq!(r, 0, "waitid: {}", std::io::Error::last_os_error());
    }

    fn free_port() -> u16 {
        crate::coord::test_port()
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

    /// A member's Mac with more room than its lead's test Mac (`cond::Mac::TEST`'s 16 GB), as the
    /// M4's 48 GB: a cold start (no job of its step measured) goes to it, whatever Mac runs the test.
    const MEMBER_MAC: cond::Mac = cond::Mac::Fixed { ac: true, battery: None, idle_s: 0, home: true, mem_gb: 48.0, mem_free_pct: 75 };

    fn stop_jobs(a: &mut Agent) {
        for k in 0..SLOTS {
            if let Some(r) = a.slots[k].running.as_mut() {
                r.stop(Duration::from_secs(5));
            }
        }
    }

    #[test]
    fn the_pool_is_always_on_with_no_switch_read() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        assert!(!r.join("state/pool").exists());
        let mut a = test_agent(Options { root: Some(r.clone()), home: d.path().join("app-folder/agent"), bin: PathBuf::from("/nonexistent/bin"), dry_run: false, once: true, helper: false }).unwrap();
        assert!(a.pool.is_some(), "this Mac's member in the pool, with no switch on the NAS");
        a.step().unwrap();
        assert!(r.join(crate::pool::term::DIR).join("1.json").exists(), "term 1 made");
        assert!(read_status(Some(&r), &a.o.home).unwrap().pool.is_some());
        assert!(a.pool_restart().is_none());
    }

    #[test]
    fn a_tick_that_never_returns_stops_the_leads_grants_and_says_so() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut a = test_agent(Options { root: Some(r.clone()), home: d.path().join("app-folder/agent"), bin, dry_run: false, once: true, helper: false }).unwrap();
        a.mem.last_ok.insert("backup".into(), now_s());
        a.mem.last_ok.insert("gc".into(), now_s());
        a.step().unwrap();
        assert!(a.published.is_some() && a.pool_hung.is_none(), "it leads, its contact on the NAS");
        let moving = |a: &Agent| a.coord.as_ref().unwrap().shared.lock().unwrap().moving.clone();
        // The pool's thread's moment's check (no tick): waited for, never a skip.
        let shared = a.pool.clone().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let s2 = shared.clone();
        let t = std::thread::spawn(move || {
            let _g = pool::lock(&s2);
            held_tx.send(()).unwrap();
            rx.recv().ok();
        });
        held_rx.recv().unwrap();
        tx.send(()).unwrap();
        a.step().unwrap();
        t.join().unwrap();
        assert_eq!(a.pool_skips, 0);
        // A tick that took the lock and hasn't said so yet, and then hangs: the loop tries a while
        // (`TRY_FOR`), never waits on it for good, and passes its step over.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let s2 = shared.clone();
        let t = std::thread::spawn(move || {
            let _g = pool::lock(&s2);
            held_tx.send(()).unwrap();
            rx.recv().ok();
        });
        held_rx.recv().unwrap();
        a.step().unwrap();
        assert_eq!(a.pool_skips, 1, "passed over, not waited on");
        tx.send(()).unwrap();
        t.join().unwrap();
        a.step().unwrap();
        assert_eq!(a.pool_skips, 0);
        // A tick that never returns (the share hung): held past STALL_S by its own account.
        let ticking = a.ticking.clone().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let s2 = shared.clone();
        let t = std::thread::spawn(move || {
            let _g = pool::lock(&s2);
            held_tx.send(()).unwrap();
            rx.recv().ok();
        });
        held_rx.recv().unwrap();
        ticking.start_at(pool::Ticking::awake() - 60);
        a.step().unwrap();
        assert!(a.pool_skips == 1 && a.pool_hung.is_none() && a.published.is_some(), "a minute: under way");
        ticking.start_at(pool::Ticking::awake() - crate::pool::driver::STALL_S - 1);
        a.step().unwrap();
        assert_eq!(moving(&a).as_deref(), Some("its pool step hangs (the NAS not answering)"));
        assert!(a.published.is_none() && a.pool_hung.is_some(), "its contact off the NAS");
        a.step().unwrap();
        assert!(a.published.is_none(), "kept off while it hangs");
        let st = read_status(Some(&r), &a.o.home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("has hung") && w.why.contains("the NAS isn't answering")), "{:?}", st.waiting);
        // It returns: cleared, granting and published again.
        ticking.end();
        tx.send(()).unwrap();
        t.join().unwrap();
        a.step().unwrap();
        assert!(a.pool_hung.is_none() && a.pool_skips == 0 && a.published.is_some());
        assert_ne!(moving(&a).as_deref(), Some("its pool step hangs (the NAS not answering)"));
        let st = read_status(Some(&r), &a.o.home).unwrap();
        assert!(!st.waiting.iter().any(|w| w.why.contains("has hung")), "{:?}", st.waiting);
        stop_jobs(&mut a);
    }

    #[test]
    fn the_lead_leads_and_hands_its_jobs_off() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let home = d.path().join("app-folder/agent");
        // (`--helper` passed, as the M1's launch file does: the pool says what it is.)
        let mut a = test_agent(Options { root: Some(r.clone()), home: home.clone(), bin: bin.clone(), dry_run: false, once: true, helper: true }).unwrap();
        let (role, leads, me) = a.pooled().map(|run| (run.role, run.gates.leads, run.side.member().id.clone())).expect("the pool's part");
        assert_eq!((role, leads), (pool::Role::Lead, Some(1)));
        assert!(!a.o.helper && a.coord.is_some(), "it leads: its coordinator runs");
        // (Its writer's file changed after term 1: no one names the writer any more.)
        std::fs::write(r.join("state/build/writer"), "another-mac").unwrap();
        // Its member's lock is in the app's folder; its saved state in its own.
        assert!(d.path().join(format!("app-folder/pool-{me}.lock")).exists() && home.join("pool/saved.json").exists());
        // Its first job (the daily backup) runs under a lease of its term, its saves handed off; the
        // records it merges are today's files' too (term 1's saves write them).
        until(&mut a, |a| crate::agent::build::Keys::load(a.o.root.as_ref().unwrap()).lo.is_empty() && a.pooled().is_some_and(|p| p.side.driver().records().is_some_and(|r| r.pending.contains_key("work/backup.0123456789abcdef.json"))));
        let env = std::fs::read_to_string(bin.join("env-backup")).unwrap();
        assert!(env.contains("SCENIC_HANDOFF="), "{env}");
        assert!(env.contains(&format!("{}", home.join("pool/jobs/1-").display())), "{env}");
        let rec = a.pooled().unwrap().side.driver().records().unwrap().clone();
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
        assert!(a.pooled().unwrap().gates.term >= 2);
        assert!(r.join("state/build/terms/2.json").exists());
        let m: BTreeMap<String, String> = crate::out::read_record(&r.join("state/build/pending.json")).unwrap();
        assert!(m.contains_key("work/gc.0123456789abcdef.json"), "{m:?}");
        stop_jobs(&mut a);
    }

    #[test]
    fn a_member_asks_the_lead_and_its_job_reaches_the_records_through_the_journal() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
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
        m.mac = MEMBER_MAC;
        assert_eq!(m.pooled().unwrap().role, pool::Role::Member);
        assert!(m.o.helper && m.coord.is_none());
        let (il, im) = (lead.pooled().unwrap().side.member().id.clone(), m.pooled().unwrap().side.member().id.clone());
        assert_ne!(il, im);
        // The lead's coordinator offers slope; the member asks, and runs it under `1-<lease>`.
        lead.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        until(&mut m, |m| m.slots[0].running.is_some() || m.pooled().is_some_and(|p| !p.entries.is_empty()));
        let env = std::fs::read_to_string(bin.join("env-slope")).unwrap_or_default();
        until(&mut m, |m| m.slots[0].running.is_none() && m.pooled().is_some_and(|p| p.entries.is_empty()));
        assert!(std::fs::read_to_string(bin.join("env-slope")).unwrap().contains(&format!("{}", d.path().join("m/agent/pool/jobs/1-").display())), "{env}");
        // Its entry in the journal, told to the lead by mail; the lead, knowing the member, merges it.
        lead.pooled().unwrap().side.know(&im);
        until(&mut lead, |l| l.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/2/2") == Some("k")));
        let rec = lead.pooled().unwrap().side.driver().records().unwrap().clone();
        assert_eq!(rec.manifest.get("layers/slope/lo/3-2-2").map(String::as_str), Some("layers/slope/lo/3-2-2.0123456789abcdef.pack"));
        let key = rec.reflected.iter().find(|k| k.contains("/1-")).cloned().unwrap();
        assert!(r.join(crate::pool::journal::path(&key)).exists());
        // Acknowledged: the member tells it no more.
        until(&mut m, |m| m.pooled().unwrap().side.driver().mine().to_tell(1).is_empty());
        // The lead's coordinator ended the lease, its target kept out of offers until the plan shows
        // it built; its own jobs hold none of it meanwhile.
        assert!(lead.coord.as_ref().unwrap().held("slope").is_empty());
        let lid = LeaseId { term: 1, n: key.rsplit('-').next().unwrap().parse().unwrap() };
        assert_eq!(lid.term, 1);
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    /// Every step to every member (crate::agent::every): a member asks for every step the lead may
    /// offer with its disk's room, takes `pack` by its measures, runs it in its lease's folder, and
    /// its entry reaches the lead's records through the journal.
    #[test]
    fn every_step_a_member_takes_pack_and_its_entry_is_merged() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        // `pack` of 6/1/1 measured (a run of it said what it took), offered.
        let c = lead.coord.as_ref().unwrap();
        c.add_costs(&[(crate::coord::cost_key("pack", "6/1/1"), crate::coord::Cost { peak_mb: 100, secs: 60, worker: None, v: crate::coord::cost_version("pack") })]);
        c.offer("2026-09-28", vec![crate::coord::Offer { step: "pack".into(), targets: vec![("6/1/1".into(), "kp".into(), 100)], batch: 16 }]);
        let asked = |l: &Agent| l.coord.as_ref().unwrap().workers().into_iter().find(|(_, w)| w.kind == "native").map(|(_, w)| (w.can.clone(), w.ask.as_ref().and_then(|a| a.disk_mb)));
        // The lead's coordinator told (with its own room, and its map update under way: `pack` is a
        // map update's step, its job reading the units as the map update began), the member asking for every
        // step.
        *lead.map_update.borrow_mut() = Some(build::MapUpdate { began: 1_791_000_000, regions: Vec::new(), last: true, units: [("base/6-1-1".to_string(), "base/6-1-1.1111111111111111.base".to_string())].into(), over: false, low: true, low_at: None });
        lead.pool_switch(Some(&r));
        // (Told with the plan's offers, as the lead's planning does.)
        lead.offer_update(lead.coord.as_ref().unwrap());
        m.pool_switch(Some(&r));
        assert!(lead.coord.as_ref().unwrap().shared.lock().unwrap().every.is_some_and(|e| e.own.is_some()));
        until(&mut m, |m| m.slots[0].running.is_some() || bin.join("env-pack").exists());
        until(&mut m, |m| m.slots[0].running.is_none() && m.pooled().is_some_and(|p| p.entries.is_empty()));
        let (can, disk) = asked(&lead).unwrap();
        assert!(can.iter().any(|s| s == "pack") && can.iter().any(|s| s == "lo") && !can.iter().any(|s| s == "catalog") && disk.is_some(), "{can:?}");
        let env = std::fs::read_to_string(bin.join("env-pack")).unwrap();
        assert!(env.contains(&format!("{}", d.path().join("m/agent/pool/jobs/1-").display())), "{env}");
        let as_of = env.lines().find_map(|l| l.strip_prefix(&format!("{}=", crate::out::UNITS_AS_OF_ENV))).expect("the map update's units");
        assert!(as_of.ends_with("/map-update.json#1791000000"), "{as_of}");
        let kept: serde_json::Value = serde_json::from_slice(&std::fs::read(bin.join("as-of-pack")).unwrap()).unwrap();
        assert_eq!(kept["units"]["base/6-1-1"], "base/6-1-1.1111111111111111.base");
        // The map update over: its units no longer granted.
        lead.end_update();
        assert!(lead.coord.as_ref().unwrap().map_update_began().is_none());
        // Its entry merged by the lead, within pack's write-set.
        until(&mut lead, |l| l.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("pack", "6/1/1") == Some("kp")));
        let rec = lead.pooled().unwrap().side.driver().records().unwrap().clone();
        assert_eq!(rec.manifest.get("layers/roads/hi/6-1-1").map(String::as_str), Some("layers/roads/hi/6-1-1.0123456789abcdef.pack"));
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    /// Placement (`state/pool/placement`, crate::agent::placement), with every step to every
    /// member: off, the lead's coordinator places as before and no job is told to offer ahead; on,
    /// the lead's coordinator is told (the forecast's slack and measured speeds), the member alone
    /// asking is drawn its target and takes it, its job told to offer its tasks ahead
    /// (`AHEAD_ENV`); off again, as before.
    #[test]
    fn placement_a_member_drawn_its_target_and_its_job_told_to_offer_ahead() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        // Off: not told.
        lead.pool_switch(Some(&r));
        m.pool_switch(Some(&r));
        assert!(lead.coord.as_ref().unwrap().shared.lock().unwrap().placement.is_none());
        // On: told, the forecast's slack and speeds in it.
        switch_on(&r, placement::SWITCH);
        *lead.forecast.borrow_mut() = Some(forecast::Forecast { done_at: Some(1000), steps: vec![forecast::StepFc { step: "slope".into(), done_at: Some(1000), ..Default::default() }], ..Default::default() });
        lead.pool_switch(Some(&r));
        m.pool_switch(Some(&r));
        assert_eq!(lead.coord.as_ref().unwrap().shared.lock().unwrap().placement.as_ref().map(|p| p.0.slack_of("slope")), Some(0));
        // `slope` of 3/2/2 measured, offered: the member alone asking, drawn to it.
        let c = lead.coord.as_ref().unwrap();
        c.add_costs(&[(crate::coord::cost_key("slope", "3/2/2"), crate::coord::Cost { peak_mb: 100, secs: 60, worker: None, v: crate::coord::cost_version("slope") })]);
        c.offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 1 }]);
        // (Read once its job has ended: the fake job's `env >` makes the file before it writes it,
        // so the file's being there says nothing of its lines. Only the member's job runs slope:
        // the lead isn't stepped after it's offered.)
        until(&mut m, |m| bin.join("env-slope").exists() && m.slots[0].running.is_none());
        let env = std::fs::read_to_string(bin.join("env-slope")).unwrap();
        assert!(env.lines().any(|l| l == format!("{}=1", placement::AHEAD_ENV)), "{env}");
        // Off again: as before.
        std::fs::write(r.join(placement::SWITCH), "off").unwrap();
        lead.pool_switch(Some(&r));
        m.pool_switch(Some(&r));
        assert!(lead.coord.as_ref().unwrap().shared.lock().unwrap().placement.is_none());
        assert!(!m.placement_on());
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    /// Resuming (`state/pool/resume`, crate::agent::resume): an OSM pass stopped mid-stage on a
    /// member's Mac is offered to that member alone, which resumes it in its own scratch folder with
    /// its unmerged hand-offs; finished there, it's neither run nor offered until merged; a day on
    /// (or released), the lead runs it itself. Off, the lead plans it as before.
    #[test]
    fn an_osm_pass_stopped_on_a_member_is_resumed_there_alone_and_released_after_a_day() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let date = "2026-10-01";
        let pass = format!("osm-pass {date}");
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.free_set = Some(400 << 30);
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        // (The planet after the lead's first step: only the test's plans below see it.)
        std::fs::create_dir_all(r.join(format!("sources/osm/{date}"))).unwrap();
        std::fs::write(r.join(format!("sources/osm/{date}/planet.osm.pbf")), "planet").unwrap();
        std::fs::create_dir_all(r.join("sources/basemap")).unwrap();
        std::fs::write(r.join("sources/basemap/planetiler.jar"), "jar").unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        let planned = |a: &Agent| {
            let mut w = Vec::new();
            let p = a.plan(&r, &Conditions::default(), &mut w);
            (p.iter().any(|j| j.id == pass), w)
        };
        // The member's Mac holds the pass's progress (its filter done), noted an hour ago.
        let scratch = d.path().join(format!("m/agent/scratch/osm-{date}"));
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("filter.done"), "").unwrap();
        let held = |at: u64, done: bool| resume::Progress { member: im.clone(), worker: cond::host_name(), jobs: [(pass.clone(), resume::Held { at, done, scratch: scratch.to_string_lossy().into_owned(), since: 0 })].into() };
        resume::write(&r, &held(now_s() - 3600, false)).unwrap();
        // Off: the lead plans it as before, whoever holds progress.
        lead.pool_switch(Some(&r));
        m.pool_switch(Some(&r));
        assert!(planned(&lead).0);
        assert!(lead.coord.as_ref().unwrap().shared.lock().unwrap().resume.is_none());
        // On: not here; offered to the member alone, which resumes it in its scratch folder.
        switch_on(&r, resume::SWITCH);
        lead.pool_switch(Some(&r));
        m.pool_switch(Some(&r));
        let (here, w) = planned(&lead);
        assert!(!here && w.iter().any(|w| w.why.contains("offered to it alone")), "{w:?}");
        // (Its earlier stage's entry, handed and not yet merged: laid over the snapshot for its job.)
        let earlier = crate::pool::journal::Entry { member: im.clone(), lease: LeaseId { term: 1, n: 99 }, step: "osm-pass".into(), handoff: crate::handoff::Handoff { changes: [(format!("sources/osm/{date}/filtered"), Some(format!("sources/osm/{date}/filtered.2222222222222222.osm.pbf")))].into(), ..Default::default() }, at: now_s() };
        m.pooled().unwrap().entries.push((earlier, None));
        assert!(m.unmerged_for("osm-pass").unwrap().changes.contains_key(&format!("sources/osm/{date}/filtered")) && m.unmerged_for("unit").unwrap().changes.is_empty());
        m.pooled().unwrap().entries.clear();
        until(&mut m, |m| m.slots[0].running.as_ref().is_some_and(|j| j.spec.id == pass) || std::fs::read_to_string(bin.join("env-osm-pass")).is_ok());
        until(&mut m, |m| m.slots[0].running.is_none());
        let env = std::fs::read_to_string(bin.join("env-osm-pass")).unwrap();
        assert!(env.contains("SCENIC_RESUME=1") && env.contains("SCENIC_UNMERGED=") && env.contains("SCENIC_HANDOFF="), "{env}");
        assert!(env.contains(&format!("{}", d.path().join("m/agent/pool/jobs/1-").display())), "{env}");
        assert!(lead.coord.as_ref().unwrap().held("osm-pass").is_empty(), "its lease ended at the lead, not lapsing");
        // Finished there: held, unoffered, until merged.
        let p = resume::read(&r, &im).unwrap().unwrap();
        assert!(p.jobs[&pass].done, "{p:?}");
        let (here, w) = planned(&lead);
        assert!(!here && w.iter().any(|w| w.why.contains("waiting to be merged")), "{w:?}");
        // Released, or its day out, while the member runs it under a lease: not run here too.
        let lease = lead.coord.as_ref().unwrap().hold("osm-pass", &[(date.to_string(), format!("sources/osm/{date}/planet.osm.pbf"))]).unwrap();
        resume::write(&r, &held(now_s() - resume::HOLD_S - 1, false)).unwrap();
        let (here, w) = planned(&lead);
        assert!(!here && w.iter().any(|w| w.why.contains("under its lease")), "{w:?}");
        lead.coord.as_ref().unwrap().finish(lease, &[]);
        // Its day out or released, but noted lately and not finished: maybe running under a lease the
        // lead lost, so not run here too.
        let mut lately = held(now_s() - 60, false);
        lately.jobs.get_mut(&pass).unwrap().since = now_s() - resume::HOLD_S - 1;
        resume::write(&r, &lately).unwrap();
        let (here, w) = planned(&lead);
        assert!(!here && w.iter().any(|w| w.why.contains("may be running")), "{w:?}");
        // A day on with nothing more there: the lead's to run.
        resume::write(&r, &held(now_s() - resume::HOLD_S - 1, false)).unwrap();
        assert!(planned(&lead).0);
        // Released by the owner: the same. (Noted half an hour ago: not maybe running now.)
        let at = now_s() - 1800;
        resume::write(&r, &held(at, false)).unwrap();
        assert!(!planned(&lead).0);
        resume::release(&r, &im, &pass).unwrap();
        assert!(planned(&lead).0);
        // Switched off again: the offer taken back, the plan as before. (Progress begun a minute
        // before the released one: not that release's.)
        resume::write(&r, &held(at - 60, false)).unwrap();
        assert!(!planned(&lead).0 && lead.coord.as_ref().unwrap().shared.lock().unwrap().resume.is_some());
        std::fs::write(r.join(resume::SWITCH), "off").unwrap();
        lead.pool_switch(Some(&r));
        assert!(planned(&lead).0 && lead.coord.as_ref().unwrap().shared.lock().unwrap().resume.is_none());
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    #[test]
    fn a_member_on_another_app_than_the_leads_gets_nothing_until_they_run_the_same() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.mac = MEMBER_MAC;
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
        until(&mut m, |m| m.slots[0].running.is_some() || m.pooled().is_some_and(|p| !p.entries.is_empty()));
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
        let bin = app(d.path());
        // (Its slope job runs until the test opens its gate: the ask comes while it runs, however
        // slowly a busy Mac takes the handover.)
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = slope ] && while [ ! -e '{}' ]; do kill -0 $PPID 2>/dev/null || exit 1; sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.mac = MEMBER_MAC;
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
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
                assert!(Instant::now() < end, "not done in {WATCHDOG:?}: {:?}", lead.pooled().unwrap().controls.kept.asked);
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        both(&mut lead, &mut m, &|_, m| m.pooled().unwrap().side.driver().leads() == Some(2));
        // The member leads term 2 in this process (it restarts into its part once its slot is free);
        // the lead's ask is done, its process restarting into a member's.
        let asked = lead.pooled().unwrap().controls.kept.asked.clone().unwrap();
        assert_eq!(asked.ask, crate::control::LeadAsk::Give { to: im.clone() });
        both(&mut lead, &mut m, &|l, _| l.pooled().unwrap().controls.kept.asked.as_ref().is_some_and(|a| a.state == lead::State::Done));
        assert!(lead.pool_restart().is_some_and(|w| w.contains("no longer leads")), "{:?}", lead.pool_restart());
        assert!(m.pool_restart().is_some_and(|w| w.contains("leads term 2")));
        assert!(m.slots[0].running.is_some(), "the job still runs");
        std::fs::write(&gate, b"").unwrap();
        // The job, granted in term 1, ran on: its entry reached the new lead's records.
        both(&mut lead, &mut m, &|_, m| m.slots[0].running.is_none() && m.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/2/2") == Some("k")));
        // The statuses say so: the old lead's view, the history's terms.
        let st = read_status(None, &lead.o.home).unwrap();
        let v = st.pool.and_then(|p| p.lead).expect("the controls' view");
        assert_eq!(v.lead.as_ref().map(|l| l.member.as_str()), Some(im.as_str()));
        let events = lead.coord.as_ref().unwrap().history_since(0);
        assert!(events.iter().any(|e| e.kind == "term" && e.note.starts_with("term 2: handed over by")), "{:?}", events.iter().filter(|e| e.kind == "term").collect::<Vec<_>>());
        // And on the NAS, in the old lead's own history file, once each, with its step down.
        let il = lead.pooled().unwrap().side.member().id.clone();
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
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = slope ] && while [ ! -e '{}' ]; do kill -0 $PPID 2>/dev/null || exit 1; sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.mac = MEMBER_MAC;
        m.step().unwrap();
        assert!(lead.slots_on && m.slots_on);
        let (il, im) = (lead.pooled().unwrap().side.member().id.clone(), m.pooled().unwrap().side.member().id.clone());
        lead.pooled().unwrap().side.know(&im);
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
                assert!(Instant::now() < end, "not done in {WATCHDOG:?}: {:?}", lead.pooled().unwrap().controls.kept.asked);
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        both(&mut lead, &mut m, &|l, m| m.pooled().unwrap().side.driver().leads() == Some(2) && m.coord.is_some() && l.coord.is_none());
        // Neither restarts: the member leads in its process, its coordinator started; the old lead
        // works as a member, its coordinator stopped; the job runs on.
        assert_eq!((lead.pool_restart(), m.pool_restart()), (None, None));
        assert!(m.slots[0].running.is_some(), "the job still runs");
        assert_eq!((m.pooled().unwrap().role, m.o.helper), (pool::Role::Lead, false));
        assert_eq!((lead.pooled().unwrap().role, lead.o.helper), (pool::Role::Member, true));
        std::fs::write(&gate, b"").unwrap();
        // The job, granted in term 1, ends through its own Mac's coordinator now: its entry in the new
        // lead's records, its lease ended there.
        both(&mut lead, &mut m, &|_, m| m.slots[0].running.is_none() && m.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/2/2") == Some("k")));
        assert!(m.coord.as_ref().unwrap().held("slope").is_empty());
        // The old lead, a member now, asks the new lead for work by HTTP and runs it, its entry
        // reaching the new lead's records; still no restart.
        m.pooled().unwrap().side.know(&il);
        m.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/3/2".into(), "k2".into(), 100)], batch: 2 }]);
        both(&mut lead, &mut m, &|_, m| m.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/3/2") == Some("k2")));
        assert_eq!((lead.pool_restart(), m.pool_restart()), (None, None));
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    /// Tasks brokered by every member (`state/pool/brokers`, crate::agent::brokers), two agents in
    /// one process, a part changing in its process: the member runs a broker and gives its job its
    /// address; its heartbeat says a task waits there, and the lead answers a page's "where is
    /// work" with it; a page from the lead's address takes it straight from the member's broker,
    /// and the lead's own task from the lead. The lead handed to the member: its broker becomes its
    /// coordinator, the old lead's coordinator its broker, and neither page's task is dropped (both
    /// beat and hand back where they were taken). Off: the old lead's broker stops.
    #[test]
    fn brokers_a_members_task_is_taken_by_a_page_from_it_and_a_handover_drops_no_task() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let bport = free_port();
        TEST_BROKER_PORT.with(|p| p.set(Some(bport)));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::SLOTS);
        switch_on(&r, brokers::SWITCH);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = slope ] && while [ ! -e '{}' ]; do kill -0 $PPID 2>/dev/null || exit 1; sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let lport = lead.coord.as_ref().unwrap().port();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.mac = MEMBER_MAC;
        m.step().unwrap();
        let (il, im) = (lead.pooled().unwrap().side.member().id.clone(), m.pooled().unwrap().side.member().id.clone());
        lead.pooled().unwrap().side.know(&im);
        m.pooled().unwrap().side.know(&il);
        // The member's broker, on a port of its own; the lead's coordinator brokers the lead's.
        assert!(m.coord.is_none() && m.broker.as_ref().is_some_and(|b| b.port() == bport) && lead.broker.is_none());
        // Its job given its broker's address (it waits at the gate meanwhile).
        lead.coord.as_ref().unwrap().offer("2026-09-28", vec![crate::coord::Offer { step: "slope".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], batch: 2 }]);
        until(&mut m, |m| m.slots[0].running.is_some());
        // A task each: the member's job's at its broker, the lead's own at its coordinator.
        let offer = |port: u16, token: &str, unit: &str| -> u64 {
            let job = crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], token.to_string(), "job");
            let root = d.path().join(format!("task-{port}"));
            std::fs::create_dir_all(&root).unwrap();
            let o = crate::coord::task::Offer { owner: 42, kind: "tail".into(), spec: serde_json::json!({ "unit": unit }), root, inputs: Default::default(), mem_mb: 300, ahead: None };
            job.post_json("/task/offer", &serde_json::to_value(&o).unwrap()).unwrap().1["id"].as_u64().unwrap()
        };
        let mt = offer(bport, &m.broker.as_ref().unwrap().job_token, "6/1/1");
        let lt = offer(lport, &lead.coord.as_ref().unwrap().job_token, "6/1/2");
        // The heartbeats say so, and each reads the other's (now: not waiting the half minute).
        let fresh = |a: &mut Agent| {
            a.pooled().unwrap().brokers_read = None;
            a.step().unwrap();
        };
        let told = |a: &Agent, who: &str| a.coord.as_ref().or(a.broker.as_ref()).unwrap().shared.lock().unwrap().brokers.as_ref().is_some_and(|b| b.peers.iter().any(|p| p.member == who && p.broker.as_ref().is_some_and(|b| b.waiting.get("tail").is_some_and(|w| w.n == 1))));
        let end = Instant::now() + WATCHDOG;
        while !(told(&lead, &im) && told(&m, &il)) {
            fresh(&mut lead);
            fresh(&mut m);
            assert!(Instant::now() < end, "the brokers' heartbeats not read");
            std::thread::sleep(Duration::from_millis(250));
        }
        // A page from the lead's address: where is work? The member's broker and the lead's.
        let page_origin = crate::coord::brokers::origin(&lead.coord.as_ref().unwrap().contact.urls[0]).unwrap();
        let call = |port: u16, path: &str, body: serde_json::Value| -> (u16, serde_json::Value, Option<String>) {
            let b = body.to_string();
            let (code, json, acao) = http_post(port, path, &[("Origin", &page_origin), ("X-Worker", "ipad")], &b);
            (code, json, acao)
        };
        let (code, w, _) = call(lport, "/work/where", serde_json::json!({ "can": ["tail"], "mem_mb": 1000 }));
        assert_eq!(code, 200);
        let at: Vec<&str> = w["brokers"].as_array().unwrap().iter().map(|b| b["base"].as_str().unwrap()).collect();
        let base_of = |c: &crate::coord::Coordinator| crate::coord::brokers::base(&crate::coord::brokers::Broker { urls: c.contact.urls.clone(), ..Default::default() }, false).unwrap();
        assert!(at.contains(&base_of(m.broker.as_ref().unwrap()).as_str()) && at.contains(&base_of(lead.coord.as_ref().unwrap()).as_str()), "{w}");
        assert_eq!(w["lead"].as_str(), crate::coord::brokers::origin(&lead.coord.as_ref().unwrap().contact.urls[0]).as_deref());
        // The page takes the member's task from the member's broker (another Mac's page: CORS),
        // and the lead's from the lead.
        let ask = serde_json::json!({ "worker": "ipad", "kind": "web", "can": ["tail"], "mem_mb": 1000, "cores": 4 });
        let (code, g, acao) = call(bport, "/work/ask", ask.clone());
        assert_eq!((code, g["id"].as_u64(), acao.as_deref()), (200, Some(mt), Some(page_origin.as_str())));
        let ml = g["lease"].as_u64().unwrap();
        let (code, g, _) = call(lport, "/work/ask", ask);
        assert_eq!((code, g["id"].as_u64()), (200, Some(lt)));
        let ll = g["lease"].as_u64().unwrap();
        // The lead handed to the member, in their processes.
        crate::control::request_lead(&lead.o.home, crate::control::LeadAsk::Give { to: im.clone() }, "the menu bar on l").unwrap();
        let end = Instant::now() + WATCHDOG;
        while !(m.pooled().unwrap().side.driver().leads() == Some(2) && m.coord.is_some() && lead.coord.is_none()) {
            lead.step().unwrap();
            m.step().unwrap();
            assert!(Instant::now() < end, "not handed over");
            std::thread::sleep(Duration::from_millis(250));
        }
        assert_eq!((lead.pool_restart(), m.pool_restart()), (None, None));
        // The member's broker is its coordinator now, the old lead's coordinator its broker.
        assert!(m.broker.is_none() && m.coord.as_ref().unwrap().port() == bport);
        assert_eq!(lead.broker.as_ref().map(|b| b.port()), Some(lport));
        // Neither task dropped: each page's lease beats and hands back where it was taken.
        for (port, lease, id) in [(bport, ml, mt), (lport, ll, lt)] {
            let (code, b, _) = call(port, "/work/beat", serde_json::json!({ "worker": "ipad", "lease": lease }));
            assert_eq!((code, b["ok"].as_bool()), (200, Some(true)), "task {id}'s lease at {port}");
            let (code, _, _) = call(port, "/work/done", serde_json::json!({ "worker": "ipad", "lease": lease, "outputs": [], "secs": 1.0, "peak_mb": 100 }));
            assert_eq!(code, 200, "task {id} handed back at {port}");
        }
        assert_eq!(lead.broker.as_ref().unwrap().brokered().1, 0);
        // The member's job (granted in term 1) had its broker's address, and ends through it, the
        // new lead.
        std::fs::write(&gate, b"").unwrap();
        let end = Instant::now() + WATCHDOG;
        while !(m.slots[0].running.is_none() && m.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("slope", "3/2/2") == Some("k"))) {
            lead.step().unwrap();
            m.step().unwrap();
            assert!(Instant::now() < end, "the job's end not merged");
            std::thread::sleep(Duration::from_millis(250));
        }
        let env = std::fs::read_to_string(bin.join("env-slope")).unwrap();
        assert!(env.contains(&format!("SCENIC_COORD=http://127.0.0.1:{bport}")), "{env}");
        // Off: the old lead's broker stops, the new lead's coordinator told none, and the heartbeat
        // says none from the run's next step.
        std::fs::write(r.join(brokers::SWITCH), "off").unwrap();
        lead.step().unwrap();
        m.step().unwrap();
        assert!(lead.broker.is_none() && lead.pooled().unwrap().side.broker.is_none());
        assert!(m.coord.as_ref().unwrap().shared.lock().unwrap().brokers.is_none());
        assert_eq!(http_post(bport, "/work/where", &[("Origin", &page_origin)], "{}").0, 403);
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
        TEST_BROKER_PORT.with(|p| p.set(None));
    }

    /// A POST of JSON to this process's coordinator on `port`, as a page sends it: its status, its
    /// JSON, and the origin its answer allows (CORS).
    fn http_post(port: u16, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, serde_json::Value, Option<String>) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let head: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        s.write_all(format!("POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
        let mut r = Vec::new();
        s.read_to_end(&mut r).unwrap();
        let text = String::from_utf8_lossy(&r).to_string();
        let (h, b) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let code = h.split(' ').nth(1).unwrap().parse().unwrap();
        let acao = h.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("access-control-allow-origin")).map(|(_, v)| v.trim().to_string()));
        (code, serde_json::from_str(b.trim()).unwrap_or_default(), acao)
    }

    #[test]
    fn the_leads_own_job_goes_on_across_a_handover_through_the_new_lead() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = pack ] && while [ ! -e '{}' ]; do kill -0 $PPID 2>/dev/null || exit 1; sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.step().unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
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
                assert!(Instant::now() < end, "not done in {WATCHDOG:?}: {:?}", lead.pooled().unwrap().controls.kept.asked);
                std::thread::sleep(Duration::from_millis(250));
            }
        };
        both(&mut lead, &mut m, &|l, m| m.pooled().unwrap().side.driver().leads().is_some() && m.coord.is_some() && l.coord.is_none());
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
        let il = lead.pooled().unwrap().side.member().id.clone();
        m.pooled().unwrap().side.know(&il);
        std::fs::write(&gate, b"").unwrap();
        both(&mut lead, &mut m, &|l, m| l.slots[0].running.is_none() && m.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("pack", "6/4/4") == Some("kn")));
        assert!(m.coord.as_ref().unwrap().held("pack").is_empty());
        assert_eq!((lead.pool_restart(), m.pool_restart()), (None, None));
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    /// The lead's own job of no targets of its own (a duty, as pass-sets'), ending after the lead
    /// was handed over: told to the new lead as done (it has no done record), not failed.
    #[test]
    fn a_job_of_no_targets_ending_through_the_new_lead_is_done() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = pass-sets ] && while [ ! -e '{}' ]; do kill -0 $PPID 2>/dev/null || exit 1; sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.mem.last_ok.insert("backup".into(), now_s());
        m.mem.last_ok.insert("gc".into(), now_s());
        m.step().unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        m.host = "the-member".into();
        let job = JobSpec { id: "pass-sets 2026-09-28".into(), what: "the pass's sets".into(), cmd: vec![bin.join("scenic-build").to_string_lossy().into_owned(), "pass-sets".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: None };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, job, &cond, Some(&r), &mut Vec::new()));
        crate::control::request_lead(&lead.o.home, crate::control::LeadAsk::Give { to: im.clone() }, "the menu bar on l").unwrap();
        let both = |lead: &mut Agent, m: &mut Agent, done: &dyn Fn(&Agent, &Agent) -> bool| {
            let end = Instant::now() + WATCHDOG;
            loop {
                lead.step().unwrap();
                m.step().unwrap();
                if done(lead, m) {
                    return;
                }
                assert!(Instant::now() < end, "not done in {WATCHDOG:?}");
                std::thread::sleep(Duration::from_millis(250));
            }
        };
        both(&mut lead, &mut m, &|l, m| m.coord.is_some() && l.coord.is_none());
        assert_eq!(m.coord.as_ref().unwrap().leases_out(), 1, "the new lead holds its lease");
        std::fs::write(&gate, b"").unwrap();
        both(&mut lead, &mut m, &|l, _| l.slots[0].running.is_none());
        let h = m.coord.as_ref().unwrap().history_since(0);
        assert!(h.iter().any(|e| e.kind == "done" && e.step.as_deref() == Some("pass-sets") && e.ok == Some(true)), "{h:?}");
        assert!(!h.iter().any(|e| e.kind == "fail" && e.step.as_deref() == Some("pass-sets")), "{h:?}");
        assert!(m.coord.as_ref().unwrap().leases_out() == 0, "its lease ended at once");
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    /// The lead's own job ending as it wakes from a freeze, a takeover made meanwhile, before its
    /// step has noticed: its end isn't finished in its own coordinator (about to stop), but told to
    /// the new lead once its part has changed, so the new lead doesn't hold its lease until it
    /// lapses.
    #[test]
    fn an_own_job_ending_as_the_lead_wakes_to_a_lost_term_ends_with_the_new_lead() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace("step=\"$1\"\n", &format!("step=\"$1\"\n[ \"$step\" = pack ] && while [ ! -e '{}' ]; do kill -0 $PPID 2>/dev/null || exit 1; sleep 0.05; done\n", gate.display()));
        std::fs::write(bin.join("scenic-build"), script).unwrap();
        // (Each coordinator on a port of its own: the frozen lead's still holds its own.)
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        m.mem.last_ok.insert("backup".into(), now_s());
        m.mem.last_ok.insert("gc".into(), now_s());
        m.step().unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        let il = lead.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        m.pooled().unwrap().side.know(&il);
        m.host = "the-member".into();
        let job = JobSpec { id: "pack 6/4/4".into(), what: "map tiles".into(), cmd: vec![bin.join("scenic-build").to_string_lossy().into_owned(), "pack".into()], needs: Needs { nas: false }, restart_after_sleep: false, record: Some(build::Work { step: "pack".into(), targets: vec![("6/4/4".into(), "kn".into())] }) };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, job, &cond, Some(&r), &mut Vec::new()));
        // (Its coordinator's state, its job's lease with it, saved for the term: the loop after the grant.)
        lead.step().unwrap();
        // The lead "frozen" (not stepped): the member takes over, forced, on a port of its own.
        TEST_PORT.with(|p| p.set(Some(free_port())));
        crate::control::request_lead(&m.o.home, crate::control::LeadAsk::Take { force: true, downgrade: false }, "scenic lead take on m").unwrap();
        until(&mut m, |m| m.coord.is_some() && m.published.is_some());
        assert!(m.coord.as_ref().unwrap().held("pack").contains("6/4/4"), "the new lead holds the frozen lead's lease");
        // The job ends while the lead is frozen; the lead wakes, its next loop ending it.
        std::fs::write(&gate, b"").unwrap();
        // (Its process waited for until it has exited, not reaped: the lead's next loop reaps it,
        // however long a loaded Mac takes.)
        let pid = lead.slots[0].running.as_ref().unwrap().pgid;
        exited(pid);
        lead.step().unwrap();
        assert!(lead.slots[0].running.is_none() && lead.coord.is_none(), "its job ended, its coordinator stopped");
        until(&mut m, |m| !m.coord.as_ref().unwrap().held("pack").contains("6/4/4"));
        assert!(lead.owed.is_empty());
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_ABLE.with(|c| c.set(None));
    }

    #[test]
    fn a_coordinator_that_cant_start_in_the_process_leaves_the_part_to_a_restart() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, pool::SLOTS);
        let bin = app(d.path());
        let mut lead = test_agent(Options { root: Some(r.clone()), home: d.path().join("l/agent"), bin: bin.clone(), dry_run: false, once: true, helper: false }).unwrap();
        lead.step().unwrap();
        let mut m = test_agent(Options { root: Some(r.clone()), home: d.path().join("m/agent"), bin, dry_run: false, once: true, helper: false }).unwrap();
        m.step().unwrap();
        assert_eq!(m.pooled().unwrap().role, pool::Role::Member);
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
        assert!(lead.coord.is_none() && lead.o.helper && lead.pooled().unwrap().role == pool::Role::Member);
        // (Its listener closed as its stop returned: the port can be listened on again.)
        crate::coord::port_free(port).unwrap_or_else(|e| panic!("its listener closed: {e}"));
    }

    #[test]
    fn a_coordinator_stopped_frees_its_port() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("a"), "m4", "");
        assert!(crate::coord::port_free(port).is_err(), "it listens");
        c.stop();
        drop(c);
        // (Its listener closed as its stop returned, however loaded the Mac: the port can be
        // listened on again.)
        crate::coord::port_free(port).unwrap_or_else(|e| panic!("its listener closed: {e}"));
    }

    #[test]
    fn a_lead_whose_update_waits_on_its_job_hands_the_lead_to_a_member_on_it() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        TEST_ABLE.with(|c| c.set(Some(true)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
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
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, job, &cond, Some(&r), &mut Vec::new()));
        let end = Instant::now() + WATCHDOG;
        while m.pooled().unwrap().side.driver().leads() != Some(2) {
            lead.step().unwrap();
            m.step().unwrap();
            assert!(Instant::now() < end, "not handed over: {:?}", lead.pooled().unwrap().controls.kept.asked);
            std::thread::sleep(Duration::from_millis(250));
        }
        assert!(lead.pooled().unwrap().controls.kept.asked.as_ref().is_some_and(|a| a.by.contains("update to 20261010-0200-bbbbbbb")), "{:?}", lead.pooled().unwrap().controls.kept.asked);
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
        let side = pool::Side::open(&home, &home.join("pool"), d.path(), "development", nas_).unwrap().unwrap();
        a.pool = Some(std::sync::Arc::new(std::sync::Mutex::new(pool::Run::new(side, pool::Role::Lead, pool::Gates { term: 1, leads: Some(1), duties: true, ..Default::default() }))));
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 9999 };
        let job = |id: &str| JobSpec { id: id.into(), what: id.into(), cmd: vec!["/usr/bin/true".into()], needs: Needs { nas: true }, restart_after_sleep: false, record: None };
        // Not caught up: no catalog; a sweep asks for a re-assertion first.
        let mut w = Vec::new();
        assert!(!a.try_start(0, job("catalog"), &c, Some(&r), &mut w));
        assert!(w.iter().any(|w| w.why.contains("reflect the journal")), "{w:?}");
        assert!(!a.try_start(0, job("gc"), &c, Some(&r), &mut w));
        assert!(a.pooled().unwrap().reassert);
        // Caught up: the catalog; the sweep only on a step that re-asserted.
        a.pooled().unwrap().gates.caught_up = true;
        assert!(a.try_start(0, job("catalog"), &c, Some(&r), &mut Vec::new()), "would start (dry run)");
        assert!(!a.try_start(0, job("gc"), &c, Some(&r), &mut Vec::new()));
        a.pooled().unwrap().gates.fresh = true;
        assert!(a.try_start(0, job("gc"), &c, Some(&r), &mut Vec::new()));
        // Other work as before.
        a.pooled().unwrap().gates = pool::Gates::default();
        assert!(a.try_start(0, job("backup"), &c, Some(&r), &mut Vec::new()));
    }

    #[test]
    fn a_member_whose_lock_is_held_waits_for_it_then_restarts_into_the_pool() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let home = d.path().join("app-folder/agent");
        let id = crate::pool::member_id(&home).unwrap();
        let held = crate::pool::MemberLock::take(&d.path().join("app-folder"), &id).unwrap().unwrap();
        let mut a = test_agent(Options { root: Some(r.clone()), home: home.clone(), bin: PathBuf::from("/nonexistent/bin"), dry_run: false, once: true, helper: false }).unwrap();
        assert!(a.pool.is_none() && a.coord.is_none() && !a.o.dry_run, "waiting, not a dry run for good");
        a.step().unwrap();
        let st = read_status(Some(&r), &home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("not in the pool yet") && w.why.contains("member's lock")), "{:?}", st.waiting);
        assert!(a.pool_restart().is_none() && a.slots.iter().all(|s| s.running.is_none()));
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

    fn agent_dry(root: &Path, home: &Path) -> Agent {
        test_agent(Options { root: Some(root.to_path_buf()), home: home.to_path_buf(), bin: PathBuf::from("/nonexistent/bin"), dry_run: true, once: true, helper: false }).unwrap()
    }

}
