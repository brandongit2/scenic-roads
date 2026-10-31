//! The build agent (docs/building.md §2): `scenic agent`, a login item on every Mac under the
//! launcher. It works out what needs doing (leading), runs as many jobs at once as its lead's
//! coordinator grants it (`job_slots`) when their conditions hold (power, the NAS), pauses each
//! when they lapse, and writes a heartbeat the app shows.
//!
//! It is in parts, each on a thread of its own (docs/agent-parts.md): **control** (`asks`: the
//! owner's asks at once), **beat** (`beat`: the heartbeat every 10 s), **nas** (`nas`: the mount,
//! under deadlines), **jobs**' thread (`slots`: reaps and cancels), the lead's **plan** (`plan`: the
//! plan on changes, its offers, the gate, the checklist and counts), and the pool's thread
//! (`pool::Ticker`). `run` starts them and goes round **jobs**' own round (`Agent::round`; every
//! 20 s, at once when a job ends or a part wakes it):
//! 1. conditions: power, the NAS (as **nas** last saw it), the user's activity, sleep;
//! 2. the running jobs: each finished (recorded; failures retried with a growing delay), paused or
//!    resumed, restarted after sleep when it touches the NAS; the memory and disk guards;
//! 3. the pool's step (until #878), and the free slots' jobs: every job by grant (docs/pool.md
//!    §7.2), from the last plan, which **plan** offers through this Mac's coordinator (the lead's;
//!    `Agent::lead_grants`); a member asks the lead's;
//! 4. this Mac's caches (`room`): the owner's ask to clear them taken up at any moment, to its level
//!    (room::Level; declined, why said, only while a job an earlier agent left runs or without the
//!    NAS), else, toward the owner's room target, the caches freed as far as it needs; either on a
//!    thread of its own, jobs running and starting (what they use they hold);
//! 5. its view published for the heartbeat (`beat`: `state/status.json` on the NAS, and a copy in
//!    the agent's local folder).
//!
//! Nothing depends on the lead being available: until work is done, the map serves the last
//! catalogue. A job is a child process (see `jobs`) that resumes from its own completion markers, so
//! stopping it at any time loses at most its current stage.

pub mod asks;
pub mod backup;
pub mod beat;
pub mod brokers;
pub mod build;
pub mod buildmem;
pub mod builtview;
pub mod bus;
pub mod cataloguesview;
pub mod cond;
pub mod every;
pub mod fcarchive;
pub mod fccheck;
pub mod forecast;
pub mod gc;
pub mod heal;
pub mod histarchive;
pub mod jobs;
pub mod lead;
pub mod live;
pub mod memguard;
pub mod nas;
pub mod nasread;
pub mod pageview;
pub mod placement;
pub mod plan;
pub mod pool;
pub mod publish;
pub mod recipes;
pub mod redo;
pub mod regionsview;
pub mod rekey;
pub mod restore;
pub mod resume;
pub mod room;
pub mod settings;
pub mod share;
pub mod slots;
pub mod steps;
pub mod takeover;
pub mod tiles;

use crate::pool::term::Role;
use anyhow::{Context, Result};
use cond::{Conditions, SleepWatch};
use jobs::{now_s, JobId, JobSpec, Needs, Running};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use store::nasfs::{NasPath, NasRoot};

/// Free space the OSM pass needs on the lead to start: the filtered planet (about half the
/// planet's 95 GB) with room to spare. The planet is read from the NAS when there's no room to
/// copy it, the filtered file too once space runs short, and the extracts are cut a quarter at a
/// time, so the pass's peak stays near the filtered file's size.
pub const PASS_SPACE: u64 = 80 << 30;

/// A job's scratch folder (its `--scratch`), when it has one.
fn scratch_of(spec: &JobSpec) -> Option<PathBuf> {
    spec.cmd.iter().position(|a| a == "--scratch").and_then(|i| spec.cmd.get(i + 1)).map(PathBuf::from)
}

/// Where things are.
#[derive(Clone, Debug)]
pub struct Options {
    /// The NAS project folder; None: find the share (and mount it when missing).
    pub root: Option<store::nasfs::NasRoot>,
    /// The agent's local folder (`~/Library/Application Support/scenic/agent`).
    pub home: PathBuf,
    /// The folder of the programs jobs run (`scenic-build`, `extract`): the agent's own.
    pub bin: PathBuf,
    /// Plan and report, start nothing.
    pub dry_run: bool,
    /// One loop, then exit (tests, `scenic agent --once`).
    pub once: bool,
    /// A test's role out of the pool (no NAS, a dry run): a member's (true) or the lead's. In the
    /// pool the declaration says (`pool::term::role`); out of it a real agent's is unknown.
    #[cfg(test)]
    pub member: bool,
}

/// How long a job asked to stop at its next safe point (the build pausing, the battery low) is given
/// before it's frozen where it is instead: a z6 tile takes up to ~11 min, a map tile ~10.
/// (The bound on a step's gap between safe points, crate::control::SAFE_GAP_BOUND_S, is a third of it.)
pub(crate) const DRAIN_GRACE: Duration = Duration::from_secs(15 * 60);
/// Why an agent is out of the pool (`Status::out_of_pool`): the NAS not answering as it started.
const NAS_AWAY: &str = "the NAS didn't answer as this agent started";
/// Or this Mac's member's lock held by another process past `JOIN_WAIT`.
const LOCK_HELD: &str = "this Mac's member's lock is held by another process, most often the agent before this one still draining after an update";
/// How long an agent starting tries for this Mac's member's lock, held by another process (the
/// agent before it, draining after an update), before it starts out of the pool.
#[cfg(not(test))]
const JOIN_WAIT: Duration = Duration::from_secs(120);
#[cfg(test)]
const JOIN_WAIT: Duration = Duration::from_secs(3);
/// The wait between its tries (`JOIN_WAIT`).
#[cfg(not(test))]
const JOIN_TRY: Duration = Duration::from_secs(1);
#[cfg(test)]
const JOIN_TRY: Duration = Duration::from_millis(50);

/// How often a member reads its hold on the NAS (crate::control::hold_path), set there by another
/// Mac's `scenic pause <member>`.
const HOLD_READ_EVERY: Duration = Duration::from_secs(30);

/// An update's drain (`Agent::update_wait`): the time the target under way is expected to need,
/// times UPDATE_MARGIN, at most UPDATE_CAP, never less than DRAIN_GRACE. ×1.5 because a target's
/// measured time varies by about a third between runs (another job beside it, the NAS's load), and
/// a pace read early in a part runs ahead of its slower end; 3 h because a later app's fixes (often
/// the reason for the update) shouldn't wait longer than a night's long target, a z3 tile's terrain,
/// takes at most. A job that used less than a core-minute of CPU over UPDATE_STALL (its sampler's
/// `busy_at`, crate::agent::memguard) holds it no longer, whatever its progress lines say: a step
/// may print none for hours.
const UPDATE_MARGIN: f64 = 1.5;
const UPDATE_CAP: Duration = Duration::from_secs(3 * 3600);
const UPDATE_STALL: Duration = Duration::from_secs(30 * 60);

/// The lead's slots as its ramp starts them (`Agent::tend_ramp`), and the clean stretch after which
/// it grows them by one (s).
pub(crate) const RAMP_START: usize = 2;
pub(crate) const RAMP_CLEAN_S: u64 = 6 * 3600;
/// Why the lead's slots are held at `RAMP_START` (`RampView::held`): only this holds them.
pub(crate) const RAMP_HELD_WHY: &str =
    "no NAS limits yet (no probe landed), or the probes failing (two or more in a row)";

/// The lead's slots' ramp as the status shows it (`Status::ramp`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RampView {
    /// The slots it may run now, those its ramp reached, and the most its cores allow (`job_slots`).
    pub slots: usize,
    pub reached: usize,
    #[serde(default)]
    pub most: usize,
    /// Why its slots are held at the start now (the NAS's limits not known): only this holds them;
    /// its stretch goes on meanwhile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
    /// Why its stretch isn't clean now (a stall): it stops their growth alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_clean: Option<String>,
    /// Since when its stretch has been clean (unix s; 0: not yet).
    pub clean_since: u64,
    /// When it grows next if its stretch stays clean and it runs its slots at once (unix s): none
    /// at the most, or not clean now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grows_at: Option<u64>,
}

/// The lead's slots' ramp, kept in its state (`Memory::ramp`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Ramp {
    /// The slots reached (0: not started, `RAMP_START`).
    #[serde(default)]
    slots: usize,
    /// Since when its stretch has been clean (unix s; 0: not yet).
    #[serde(default)]
    clean_since: u64,
    /// The most jobs it ran at once in the stretch: it grows only once it ran `slots` at once.
    #[serde(default)]
    peak: usize,
}

/// The slots an agent keeps at least: a Mac runs as many jobs as its lead grants it, each in a slot
/// of its own, the slots past these grown and shrunk with its leases (`Agent::slots`, `job_slots`).
pub(crate) const SLOTS: usize = 2;

/// How long a job uses under a core-minute of CPU (its sampler's `busy_at`) before its Mac's
/// capacity names it idle (`Capacity::idle`): its lead counts none of its cores, so CPU work goes
/// beside a job waiting on the network (task #665: four terrain jobs fetching GSI's tiles held
/// the M4's cores for 4 h at half a CPU-second a minute).
pub const IDLE_CPU_S: u64 = 10 * 60;

/// The most slots an agent holds (a bound on a Mac's jobs, whatever its cores; the names of its
/// slots' files and workers by number below it).
pub(crate) const MAX_SLOTS: usize = 64;

/// The jobs a Mac runs at once, the lead's and a member's alike: one a core past the two kept for
/// its owner, at least one and at most `MAX_SLOTS`. Its lead grants fewer when its room (memory,
/// disk, the cores its jobs measured) holds fewer.
pub(crate) fn job_slots(cores: usize) -> usize {
    cores.saturating_sub(2).clamp(1, MAX_SLOTS)
}

/// What an agent asks for (crate::coord::Ask::want): its slots free (`slots` its most, `running` its
/// jobs now).
pub(crate) fn want(slots: usize, running: usize) -> usize {
    slots.saturating_sub(running)
}

/// This Mac's role as its pool's driver last read the declaration (docs/roles.md §1).
fn role_by(run: &pool::Run) -> Role {
    crate::pool::term::role_of(run.side.driver().declared(), Some(&run.side.member().id))
}

/// What only the lead's parts keep in the agent's folder `home`, removed on a member (docs/roles.md
/// §5 step 2): the terrain packs' indexes the plan reads (`pack-idx/`, agent::plan's
/// `TerrainTiles`; no job reads them).
fn drop_lead_caches(home: &Path) {
    let idx = home.join("pack-idx");
    if idx.exists() {
        match std::fs::remove_dir_all(&idx) {
            Ok(()) => eprintln!("agent: removed {} (a lead's, this Mac a member)", idx.display()),
            Err(e) => eprintln!("agent: removing {}: {e}", idx.display()),
        }
    }
}

/// A role in words, for the log.
fn role_name(r: Role) -> &'static str {
    match r {
        Role::Lead => "the lead",
        Role::Member => "a member",
        Role::Unknown => "not known",
    }
}

/// The build's records' manifest and keys as a reader has them now (crate::out::brief_now), read
/// within `nasread::DEADLINE` (a call on the briefs' folder, `BRIEFS`): none (empty ones) when they
/// can't be read now.
fn records_deadlined(root: &store::nasfs::NasRoot) -> crate::pool::records::Brief {
    let r = root.clone();
    nasread::with(&root.join(BRIEFS), move || crate::out::brief_now(&r).map_err(std::io::Error::other))
        .unwrap_or_default()
}

/// The folder of the terms' briefs (crate::pool::records::brief_path), as the NAS calls reading
/// them name it.
const BRIEFS: &str = "state/build/term";

/// `SCENIC_TEST_WORKER` when set: the name the agent works and reports under in place of its Mac's
/// (the two-Mac test's alone, tools/check/pool-two-macs.sh: a member beside the lead on one Mac,
/// asking under a name of its own). Never a real agent's.
fn test_worker() -> Option<String> {
    let w = std::env::var("SCENIC_TEST_WORKER").ok().filter(|w| !w.trim().is_empty())?;
    eprintln!(
        "agent: WARNING: SCENIC_TEST_WORKER: this agent works as {w} (the two-Mac test's alone, never a real agent's)"
    );
    Some(w)
}

/// Why a member's job ended as failed, for its lead: its note, or, the note empty (a job that
/// ended well with nothing to hand off), what's known, so the lead's record never says nothing.
fn fail_why(outcome: Outcome, note: &str, handed: bool) -> String {
    if !note.trim().is_empty() {
        return note.to_string();
    }
    match (outcome, handed) {
        (Outcome::Done, false) => {
            "it ended well, but its hand-off couldn't be read here: its work is done again".into()
        }
        (Outcome::Done, true) => "it ended well, but its hand-off has no done record".into(),
        _ => "it failed, saying nothing".into(),
    }
}

/// A slot's files' suffix in the agent's folder: none for the first, `-2`, `-3`, … after
/// (`job.json`, `job-2.json`, …; `scratch/`, `scratch-2/`, …).
fn slot_suffix(k: usize) -> String {
    if k == 0 {
        String::new()
    } else {
        format!("-{}", k + 1)
    }
}

/// The lead's record of its catalogues (build::LastCatalogue), in the agent's folder.
const PUBLISHED_FILE: &str = "publishing.json";
/// The lead's record of its catalogues kept in `home` (`PUBLISHED_FILE`); none when it doesn't read.
fn read_published(home: &Path) -> build::LastCatalogue {
    std::fs::read(home.join(PUBLISHED_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Keeps the lead's record of its catalogues in `home` (`PUBLISHED_FILE`).
fn write_published(home: &Path, r: &build::LastCatalogue) -> Result<()> {
    store::whole::write(&home.join(PUBLISHED_FILE), &serde_json::to_vec(r)?)
}

/// The pass's worldwide steps the checklist counts, after the planet's own.
const WORLDWIDE_STEPS: [&str; 9] =
    ["pass-sets", "outlines", "trailends", "reach", "buildings", "summits", "labels", "lakefill", "water"];

/// The loop's pool steps passed over in a row, a tick holding the run, before the status says the
/// pool's step hangs (about a minute of the loop's steps; or at once past `STALL_S`).
const HUNG_SKIPS: u32 = 3;
/// What the coordinator says while its pool step hangs on the share (`Agent::pool_skipped`).
const HUNG: &str = "its pool step hangs (the NAS not answering)";
/// How long the loop tries for the run when no tick is said to hold it (the pool's thread's moment's
/// check, or a tick begun and not yet said), before passing its pool step over.
const TRY_FOR: Duration = Duration::from_millis(200);

/// Whether job `id` is one of the lead's duties (a catalogue, a sweep: crate::agent::pool's
/// `PUBLISHES`, `SWEEPS`).
fn is_duty(id: &str) -> bool {
    step_of(id).is_some_and(|st| pool::PUBLISHES.contains(&st.as_str()) || pool::SWEEPS.contains(&st.as_str()))
}

/// The cost key a job of no targets of its own is measured by (the OSM pass's `osm-pass <date>`, GC's
/// `gc gc`): its id's step and the rest of its id, or its id again.
fn own_cost_key(id: &str) -> String {
    let j = JobId::parse(id);
    crate::coord::cost_key(j.step, j.target())
}

/// Why the lead's catalogue waits while its pool's records don't reflect the journal (`Gates::publish`).
const UNPUBLISHED: &str = "waits for the pool's records to reflect the journal (a listing of every day merged)";

/// Whether `spec` is an offered step's work in the plan (crate::agent::every::offered): it marks the
/// step's place in the plan's order, its targets granted to any member (this Mac's own agent among
/// them) as many as fit a quarter of an hour, never started as planned (`Agent::offer_plan`).
fn offered_job(spec: &JobSpec) -> bool {
    spec.record.as_ref().is_some_and(|w| every::offered(&w.step))
}

/// The targets the lead's own planned job `spec` is offered with: its record's, or for a job of no
/// targets of its own (the OSM pass, GC), its id's after its step (`own_cost_key`'s), keyless (its
/// lease holds none: crate::coord's `pack`).
fn own_targets(spec: &JobSpec) -> Vec<(String, String)> {
    match spec.record.as_ref().filter(|w| !w.targets.is_empty()) {
        Some(w) => w.targets.clone(),
        None => vec![(JobId::parse(&spec.id).target().to_string(), String::new())],
    }
}

/// Slot `k`'s job as a worker (the build's history, the forecast's machines, the worker page): the
/// Mac's name, and past the first slot its number (`<host> #2`, `#3`, …).
pub fn slot_worker(host: &str, k: usize) -> String {
    if k == 0 {
        host.to_string()
    } else {
        format!("{host} #{}", k + 1)
    }
}

/// Whether worker `name` is one of `host`'s slots' (`slot_worker`).
pub fn is_slot_of(name: &str, host: &str) -> bool {
    match name.strip_prefix(host) {
        Some("") => true,
        Some(rest) => rest.strip_prefix(" #").is_some_and(|n| n.parse::<usize>().is_ok_and(|n| n >= 2)),
        None => false,
    }
}

/// Every name `host`'s slots go by (`slot_worker` up to `MAX_SLOTS`), for what drops a process's
/// leases by its workers.
pub fn slot_workers(host: &str) -> Vec<String> {
    (0..MAX_SLOTS).map(|k| slot_worker(host, k)).collect()
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
    /// An update's drain under way: how long it waits for its job's safe point, and why
    /// (`Agent::update_wait`), worked out once when it began.
    /// (None: the time a pause gives, `Agent::grace`.)
    update_wait: Option<(Option<Duration>, String)>,
    /// Why its job stops at its next safe point for the memory guard (crate::agent::memguard):
    /// kept until it ends.
    mem_drain: Option<String>,
    /// That drain is for its job alone past the limit while the Mac isn't short of memory: only
    /// trouble stops it at once, not the time a pause gives a safe point (a long single-target job,
    /// a z3 tile's whole terrain run, may have none for hours).
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
    /// The memory guard froze its job, the Mac short of memory with the jobs together past the
    /// limit (`Agent::pressure`): when, that job's id, and the largest job's it made room for.
    pressure_frozen: Option<(Instant, String, String)>,
    /// The threads its lead granted the job given it (crate::coord::Grant::threads), for its start.
    granted: Option<u32>,
    /// The disk its lead measured the job given it to take (MB: crate::coord::Grant::disk_mb),
    /// reserved while it holds the lease (`Agent::reservation`).
    reserve_mb: Option<u64>,
    /// The memory this member spared its jobs when it asked for the job given it (MB: the ask's
    /// `mem_mb`: the build's memory, `Agent::build_room`): a member's job far past it is given
    /// back (`Agent::over_grant`). None: the lead's own job.
    spare_mb: Option<u64>,
    /// Its job given back for being far over `spare_mb` alone (`Agent::over_grant`): the target
    /// under way's cost key and what the job held, sent as a floor learned alone (`end_lease`).
    over_floor: Option<(String, u64)>,
    /// The owner's cancel of its job (crate::control::Cancel), asked here or told by the lead:
    /// stopped and given back at the loop's next turn (`Agent::cancel_slot`).
    cancel: Option<crate::control::Cancel>,
    /// What was last saved of it with its job's record (`Agent::save_hands`), not saved again
    /// while it's the same.
    hand_saved: Option<(Hand, Option<String>)>,
}

/// How long the memory guard keeps a job frozen for the Mac's pressure before it stops it, the Mac
/// still short of memory (`Agent::pressure`).
const PRESSURE_HOLD: Duration = Duration::from_secs(60);

/// An agent's slots (`Agent::slots`): `SLOTS` of them fresh, more as a member's packed jobs take
/// them.
struct Slots(Vec<Slot>);

impl Default for Slots {
    fn default() -> Slots {
        Slots((0..SLOTS).map(|_| Slot::default()).collect())
    }
}

impl std::ops::Deref for Slots {
    type Target = Vec<Slot>;
    fn deref(&self) -> &Vec<Slot> {
        &self.0
    }
}

impl std::ops::DerefMut for Slots {
    fn deref_mut(&mut self) -> &mut Vec<Slot> {
        &mut self.0
    }
}

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
    /// Tells lead `c`, saying how it answered: None not answered; Some(true) it refused the
    /// hand-off; said when it refused it or didn't answer.
    fn tell(&self, c: &crate::coord::client::Client) -> Option<bool> {
        let (id, r) = match self {
            Tell::Done(d) => (
                d.lease,
                c.done(d).map(|h| match h {
                    crate::coord::client::Handed::Refused(why) => {
                        eprintln!("agent: the lead refused lease {}'s end: {why} (it lapses)", d.lease);
                        true
                    }
                    _ => false,
                }),
            ),
            Tell::GiveBack { id, why, floors, disks, job_disk } => {
                (*id, c.end_with(*id, why, true, floors, disks, *job_disk).map(|()| false))
            }
            Tell::Fail { id, why, floors, disks, job_disk } => {
                (*id, c.end_with(*id, why, false, floors, disks, *job_disk).map(|()| false))
            }
        };
        match r {
            Ok(refused) => Some(refused),
            Err(e) => {
                eprintln!("agent: telling the lead lease {id} ended: {e:#}");
                None
            }
        }
    }

    /// Its lease.
    fn lease(&self) -> u64 {
        match self {
            Tell::Done(d) => d.lease,
            Tell::GiveBack { id, .. } | Tell::Fail { id, .. } => *id,
        }
    }
}

/// The free space a member's job of `step` needs: a task's its kind's (crate::agent::steps::Task's
/// `disk`); a step's none of its own: their grant's measured disk is reserved for them
/// (`reservation`), and the owner's room target stays free past it, with no fixed reserve (task
/// #421).
fn helper_need(step: &str) -> u64 {
    steps::task(step).map_or(0, |t| t.disk)
}

/// Of the work a helper could ask for (`can`), what starts beside a running job (`try_start_said`):
/// its need (`helper_need`) free past `held` (the owner's room target and the jobs' reservations),
/// none made for it.
fn room_beside(can: Vec<String>, free: u64, held: u64) -> Vec<String> {
    can.into_iter().filter(|s| free >= held.saturating_add(helper_need(s))).collect()
}

/// What a granted task of `kind` does, for the status ("bldtile: z8 tile 8/4/5", "tail 6/1/1").
fn task_what(kind: &str, task: &serde_json::Value, unit: &str) -> String {
    match kind {
        crate::bld::task::KIND => format!("{kind}: z8 tile {unit}"),
        crate::trees::task::KIND => format!("{kind}: blocks {}", task["blocks"].as_str().unwrap_or(unit)),
        crate::terrain_task::KIND => format!("{kind}: subtrees {}", task["subtrees"].as_str().unwrap_or(unit)),
        _ => format!("tail {unit}"),
    }
}

/// A leased task's kind: its job id's step (`JobId`, "<kind> <unit>"); "tail", the most room, for
/// an id naming no kind (a job adopted from an agent before task #849, "task <lease>").
fn task_kind(spec: &JobSpec) -> &str {
    let step = JobId::parse(&spec.id).step;
    if steps::task(step).is_some() {
        step
    } else {
        "tail"
    }
}

/// The work a helper asks for (the shared steps, and "tail", "bldtile", "treeblock" and "terrainsub" for tasks): what its disk has free for
/// (a step's need and its margin), or can have, from the caches it may empty (`free` the disk's free
/// bytes, `cheap` what `make_room` can delete there: room::helper_cheap_bytes). A job granted that
/// still can't have its room once the caches are emptied is given back (`run_once`). With the
/// owner's disk room target (`floor`, room::Target), that much more stays free.
fn helper_steps(free: u64, cheap: u64, floor: u64) -> Vec<String> {
    steps::SHARED
        .iter()
        .copied()
        .chain(steps::TASKS.iter().map(|t| t.name))
        .filter(|s| free.saturating_add(cheap) >= floor.saturating_add(helper_need(s) + room::margin(helper_need(s))))
        .map(str::to_string)
        .collect()
}

/// Whether `p` is an archive copy a terrain job's target (`id`: "terrain-hi 3/x/y", a z3 tile's
/// whole run; "terrain-hi 6/x/y", a z6 tile; "terrain-deep 10/x/y", a z10 tile's deep levels;
/// "terrain-lo 3/x/y", an assembly; an older app's "terrain …" as terrain-hi's) reads at once: a z3
/// tile's run its shard's own (`3-x-y.…`) and its z6 tiles' (`6-X-Y.…` within it), a z6 tile its z6
/// tile's, a deep target those of the z6 tiles its base is made of
/// (crate::terrain_deep::base_want), an assembly its z3 tile's (crate::rawpack's shards).
/// How often a member asks the lead for its tiles to come (`Agent::ask_upcoming`): the lead makes
/// a forecast each `forecast::EVERY`, and the order of a Mac's next jobs moves little in minutes;
/// an answer is every Mac's (29 Oct's forecast: 5,397 jobs, ~0.5 MB).
const UPCOMING_EVERY: Duration = Duration::from_secs(5 * 60);

/// Tiles to come older than this (s from their forecast's time) hint nothing (`Agent::hints`): a
/// member that can't reach the lead keeps no stale order for long.
const UPCOMING_STALE_S: u64 = 3600;

/// This Mac's (`host`) tiles to come in a `/work/tocome` answer (`forecast::ToCome::answer`), as
/// cache hints at the forecast's time, with that time (none placed on it: no hints); None when the
/// answer isn't a forecast's (a lead before its first answers 200 with `making`).
fn upcoming_of(code: u16, v: &serde_json::Value, host: &str) -> Option<(u64, room::Hints)> {
    let at = v["at"].as_u64().filter(|_| code == 200 && v["macs"].is_object())?;
    let mine: BTreeMap<String, Vec<(String, u32)>> = match v["macs"].get(host) {
        Some(m) => serde_json::from_value(m.clone()).ok()?,
        None => BTreeMap::new(),
    };
    Some((at, room::Hints::of_tocome(&mine, at)))
}

#[cfg(test)]
fn terrain_reads(id: &str, p: &Path) -> bool {
    archive_shard(p).is_some_and(|a| terrain_shards(id).contains(&a))
}

/// The shard (z, x, y) of an archive copy of the raw tiles (`…/packs/<z>-<x>-<y>.<hash>.…`); None
/// for any other file.
pub(crate) fn archive_shard(p: &Path) -> Option<(u32, u32, u32)> {
    if !p.parent().and_then(Path::file_name).is_some_and(|d| d == "packs") {
        return None;
    }
    p.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('.').next()).and_then(tile_of)
}

fn tile_of(s: &str) -> Option<(u32, u32, u32)> {
    let mut v = s.split(['/', '-']).map(|t| t.parse::<u32>().ok());
    Some((v.next()??, v.next()??, v.next()??))
}

/// The archive shards (`archive_shard`) a terrain job's target reads (`terrain_reads`'s rule);
/// none for any other step's.
pub(crate) fn terrain_shards(id: &str) -> BTreeSet<(u32, u32, u32)> {
    let j = JobId::parse(id);
    let (s, Some(t)) = (steps::current(j.step), j.rest) else { return BTreeSet::new() };
    let Some((z, x, y)) = tile_of(t) else { return BTreeSet::new() };
    match (s, z) {
        ("terrain-deep", z) if z == crate::terrain_deep::PACK_Z as u32 => crate::terrain_deep::base_want((x, y))
            .iter()
            .map(|&(bz, bx, by)| (6, bx >> (bz - 6), by >> (bz - 6)))
            .collect(),
        ("terrain-hi", 3) => std::iter::once((3, x, y))
            .chain((0..8).flat_map(|i| (0..8).map(move |j| (6, x * 8 + i, y * 8 + j))))
            .collect(),
        ("terrain-hi", 6) => BTreeSet::from([(6, x, y)]),
        ("terrain-lo", 3) => BTreeSet::from([(3, x, y)]),
        _ => BTreeSet::new(),
    }
}

/// The memory a step's job is expected to take (MB) before one has run for its target and said
/// (`SCENIC_COSTS`): its row's in the steps table. (Z6 tiles and candidates are offered by their
/// extract's size, crate::coord::job_peak; terrain by its z3 tile's size, `terrain_peak`; the 3D
/// buildings by their rows, `bld_peak`.)
fn first_peak(step: &str) -> u64 {
    steps::mem_mb(step)
}

/// The memory a 3D buildings job of a z6 tile is expected to take (MB), before one has said, by the
/// rows of the row groups its normalized file is read from (`rows`: crate::bld::sources):
/// `bldprep_peak`, `bldtiles_peak`.
fn bld_peak(step: &str, rows: u64) -> u64 {
    match step {
        "bldprep" => bldprep_peak(rows),
        _ => bldtiles_peak(rows),
    }
}

/// `bldprep`'s, read in Rust (task #579, the Mac mini, 23 Oct): peak footprints of 0.85 GB for
/// 6/53/21's 55 k rows kept, 1.9 GB for 6/10/24's 5.8 M, 3.1 GB for Paris's 15.3 M and 5.6 GB for
/// Kantō's 30.3 M: about 1.0 GB and 150 B a row, given a quarter more.
pub const fn bldprep_peak(rows: u64) -> u64 {
    1300 + rows * 190 / (1 << 20)
}

/// `bldtiles`' (B5's pilot with the field and its centroids' offsets, 2026-10-15, on the
/// M4's 14 threads): 1.8 GB and 1,100 B a record of its largest z8 tile (taken as two fifths of
/// the rows), the field's cells in flight the fixed part. Measured 1.3–2.9 GB for tiles of up to
/// 52 k records, 3.8 GB for Toronto's 9.5 M, 4.5 GB for Annecy's 10.0 M, 7.9 GB for New York's
/// 14.6 M and 10.2 GB for Kantō's 30.3 M.
pub const fn bldtiles_peak(rows: u64) -> u64 {
    1800 + rows * 2 / 5 * 1100 / (1 << 20)
}

/// The memory a terrain-hi run of a z3 tile of `z6` tiles near the coverage is expected to take
/// (MB), before one has said: a z6 tile's levels at a time (crate::terrain_pack::piece_peak_mb), each
/// further z6 tile's quarters (~20 MB) and the z3 tile's zoomed-out tiles (z3–8, 1,365 at ~270 KB);
/// with `deep` (a fine dataset meets it north of 67°, where its deep z12 goes in its hi pack:
/// crate::terrain_deep::reaches_hi), those levels' bound besides (crate::terrain_deep::peak_mb). (A
/// z6 tile: one z6 tile, `terrain_peak(1, …)`.)
fn terrain_peak(z6: usize, deep: bool) -> u64 {
    let deep = if deep { crate::terrain_deep::peak_mb() } else { 0 };
    crate::terrain_pack::piece_peak_mb() + 1365 * 270 / 1024 + (z6 as u64).saturating_sub(1) * 20 + deep
}

/// Past what memory (MB) a member's jobs are far over the `spare` it gave them (`Agent::over_grant`):
/// half as much again, 4 GB more at least (the caller caps it at the Mac's limit).
fn far_over(spare: u64) -> u64 {
    spare + (spare / 2).max(4096)
}

/// Whether `spec`'s job is paced (crate::inputs::pace, `Agent::tend_threads`): an input check (the
/// `inputs` step's), or tree cover (`trees`, task #606: its blocks take the cores the Mac has idle).
fn is_paced(spec: &JobSpec) -> bool {
    matches!(step_of(&spec.id).as_deref(), Some("inputs" | "trees"))
}

/// This Mac's cores.
fn mac_cores() -> u32 {
    std::thread::available_parallelism().map_or(4, |n| n.get() as u32)
}

/// What an agent saves of a slot with its job's record (jobs::save_hand), for the agent after it to
/// take the job over (`Agent::adopt_jobs`): its lease, and what its grant said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Hand {
    lease: Option<Held>,
    #[serde(default)]
    reserve_mb: Option<u64>,
    #[serde(default)]
    spare_mb: Option<u64>,
    /// The budgets' proxy's port its job was told (the next agent's proxy listens there again).
    #[serde(default)]
    proxy_port: Option<u16>,
}

/// The port the jobs the agent before left running in `home` were told their proxy is on
/// (`Hand::proxy_port`, their records'), if any.
fn told_proxy_port(home: &Path) -> Option<u16> {
    (0..MAX_SLOTS).find_map(|k| {
        let v = jobs::saved_hand(&home.join(format!("job{}.json", slot_suffix(k))))?;
        serde_json::from_value::<Hand>(v).ok()?.proxy_port
    })
}

/// The running job's lease.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
}

/// A helper's job's timings' record, in its outbox folder (`SCENIC_TIMINGS`).
const TIMINGS_FILE: &str = "timings.json";

/// The timings' record a helper's job left in its folder `dir`, for its hand-off.
fn read_timings(dir: &Path) -> Option<crate::timings::RunRec> {
    std::fs::read(dir.join(TIMINGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// What a `unit` job's z6 tiles cost (`SCENIC_COSTS`: a JSON line per z6 tile).
fn read_costs(p: &Path) -> Vec<(String, crate::coord::Cost)> {
    let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
    s.lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            // (`sizes`: a line from a job before them has none.)
            let sizes = v["sizes"]
                .as_object()
                .map(|m| m.iter().filter_map(|(k, n)| Some((k.clone(), n.as_u64()?))).collect())
                .unwrap_or_default();
            Some((
                v["unit"].as_str()?.to_string(),
                crate::coord::Cost {
                    peak_mb: v["peak_mb"].as_u64()?,
                    secs: v["secs"].as_u64().unwrap_or(0),
                    worker: None,
                    v: v["v"].as_u64().unwrap_or(0) as u32,
                    sizes,
                },
            ))
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
    /// Other app versions its running jobs run programs from (jobs an agent of an older app
    /// started, taken over: `Agent::adopt_jobs`); the map server's updater keeps their folders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub job_apps: Vec<String>,
    /// Seconds since the epoch.
    pub beat: u64,
    /// How old the loop's part of this status is (s), as the beat wrote it (crate::agent::beat), or
    /// how long the lead's plan has run, the older: the heartbeat's time says the agent is alive,
    /// this whether its loop is slow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_age_s: Option<u64>,
    /// A `loop_age_s` past this (s) is a stuck loop (crate::agent::beat::LOOP_STUCK, which the plan's
    /// stall log goes by too): the threshold the build page and the menu bar flag it by (task #899).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_stuck_s: Option<u64>,
    /// The stuck part's phase while it is stuck (task #899): the round's (crate::agent::nasread::phase),
    /// or "the plan: <its phase>".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_phase: Option<String>,
    pub started: u64,
    pub conditions: Conditions,
    /// Its running jobs, every slot's in slot order (each says its slot: `JobView::slot`). Read
    /// them through `Status::jobs`, which takes an older agent's from the fields below.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<JobView>,
    /// Every member of the pool reports the new app (its asks carry `Ask::cap`), as its lead's
    /// coordinator last said (`Coordinator::all_new`): packing several jobs per member is on, and
    /// the old fields below are no longer written. False from an older agent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_new: bool,
    /// The old wire's jobs (an older reader's): the first slot's, the second's beside it and the
    /// rest. Written only while the pool isn't all on the new app (`all_new`); read through
    /// `Status::jobs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside: Option<JobView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<JobView>,
    /// SCENIC_TEST_DISK_EXTRA_MB when set (MB counted free past what the disk has): a test's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_disk_extra_mb: Option<u64>,
    /// Work that can't run yet, and why, in plain words.
    pub waiting: Vec<Waiting>,
    /// Things to know that hold nothing back (the build page's info lines, scenic status's
    /// "note:"): declared elevation datasets left in their own height reference.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<Notice>,
    /// The last jobs to finish, newest first.
    pub recent: Vec<Done>,
    /// Region recipes, and the ones that don't parse.
    pub regions: Vec<recipes::Recipe>,
    pub bad_recipes: Vec<(String, String)>,
    /// Per region, how many of its z6 tiles are built (after the first pass).
    #[serde(default)]
    pub built: BTreeMap<String, build::RegionState>,
    /// The build to the end, step by step: the pass's steps, then the regions'.
    #[serde(default)]
    pub checklist: Vec<build::Line>,
    /// The workers the coordinator heard from in the last two minutes (helpers, web pages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<WorkerView>,
    /// Each member's capacity as the lead's coordinator last heard it (crate::coord::Capacity):
    /// the lead's own first, then each member whose ask carried one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capacity: Vec<(String, crate::coord::Capacity)>,
    /// Why nothing more fits each member, as the lead's coordinator last packed its ask
    /// (crate::coord::Coordinator::refusals): by its worker's name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub packing: BTreeMap<String, String>,
    /// Each member's share of the NAS budget now, its leased jobs' weights in thousandths of the
    /// whole (crate::coord::Coordinator::nas_shares): by its worker's name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nas_share: BTreeMap<String, u32>,
    /// What one job of each step leased now weighs of the NAS budget, thousandths of the whole
    /// (crate::coord::Coordinator::nas_step_weights): by step.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nas_step: BTreeMap<String, u32>,
    /// Each step's phases as mean shares of wall time and bytes over its last runs, measured from
    /// the build's timings by the lead's coordinator (crate::coord::Coordinator::phase_shares): by
    /// step. Empty on a member, and until the lead has read them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub phase_shares: BTreeMap<String, crate::timings::PhaseShares>,
    /// Each step's time by phase over the build page's chart ranges, summed from the build's
    /// timings by the lead's coordinator (crate::coord::Coordinator::phase_times): the page's step
    /// rows' time breakdown. None on a member, and until the lead has read them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_times: Option<crate::timings::PhaseTimes>,
    /// The NAS as the lead sees it: its probes, limits, disk load and the pool's traffic
    /// (crate::coord::Coordinator::nas_view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas: Option<crate::coord::NasView>,
    /// Helpers on other Macs building now (`state/helpers/<host>.json`, fresh within ten minutes):
    /// their own status, with their job.
    #[serde(default)]
    pub helpers: Vec<Status>,
    /// The build's pause (crate::control), while it's paused, as this agent knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<crate::control::Pause>,
    /// This Mac paused alone (crate::control::Hold, task #224), while it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<crate::control::Hold>,
    /// This Mac's own pause's stage (crate::control::HoldView, task #846): asked, pausing (its jobs
    /// finishing), paused, resuming. Set by `Status::say`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<crate::control::HoldView>,
    /// This Mac's state in a word and a line (`Status::say`, task #846): what the menu bar and the
    /// build page show for it, out of touch aside (theirs to tell, by `out_of_touch_s`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// A heartbeat older than this (s) is out of touch (crate::pool::beat::OUT_OF_TOUCH_S): the
    /// one threshold every reader uses (task #846).
    #[serde(default)]
    pub out_of_touch_s: u64,
    /// Why this Mac's agent isn't in the pool, while it isn't (none in the pool, or a dry run's):
    /// waiting to join, and what for (`Status::role`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_of_pool: Option<String>,
    /// This Mac's memory, load, disk and caches, and the NAS's answer and room (cond::Resources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<cond::Resources>,
    /// When each step and region will be done and on the map, and what each machine does next
    /// (the lead's: crate::agent::forecast).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forecast: Option<forecast::Forecast>,
    /// Publishing as the lead last planned it, and why (build::PublishLine; docs/building.md §2,
    /// Publishing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publishing: Option<build::PublishLine>,
    /// The timetables' to-do list's counts and the routes matching nothing on the map
    /// (crate::timetables, docs/inputs.md §6.7.3): the lead's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timetables: Option<crate::timetables::View>,
    /// The last catalogue the plan read: its number and when it went out (seconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "catalog")] // wire key: old spelling
    pub catalogue: Option<CatalogueSeen>,
    /// Publishing as it stands (`Published`; the lead's): the build page's map KPI (its last catalogue) and
    /// `scenic status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<Published>,
    /// This Mac's build caches (room::Caches): what a clear would free, why they can't be cleared
    /// now, and the last clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caches: Option<room::Caches>,
    /// This Mac in the pool, while it's on (docs/pool.md §12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<PoolView>,
    /// The memory guard (crate::agent::memguard): its switch, this Mac's limit, what the jobs hold
    /// now, what it last did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<memguard::View>,
    /// The lead's slots' ramp (docs/pool.md §7.1): the slots it may run, why they're held, and
    /// since when its stretch has been clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp: Option<RampView>,
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
pub struct CatalogueSeen {
    pub n: u64,
    pub at: u64,
}

/// Publishing as it stands, for the build page's map KPI and `scenic status` (the
/// lead's; docs/building.md §2, Publishing): the last catalogue, whether it went with the low zooms
/// (build::LastCatalogue::low_at), the map tiles being drawn by Mac (pack's targets leased), and the
/// catalogue each Mac's map server last adopted (crate::pubtimes, the servers' log on the NAS).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Published {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<CatalogueSeen>,
    #[serde(default)]
    pub low: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drawing: BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub adopted: BTreeMap<String, CatalogueSeen>,
}

/// The Mac a worker is (`slot_worker`: its slots' names folded to the host's; a page's its own).
pub fn host_of(worker: &str) -> &str {
    match worker.rsplit_once(" #") {
        Some((host, n)) if n.parse::<usize>().is_ok_and(|n| n >= 2) => host,
        _ => worker,
    }
}

/// The `seq` each Mac's map server last adopted and when (docs/maplog.md §6.1): its cursor in the
/// map's log (`server/<host>`, store::map::Cursor), as the lead heard it.
fn adopted(cursors: &BTreeMap<String, store::map::Cursor>) -> BTreeMap<String, CatalogueSeen> {
    cursors
        .iter()
        .filter_map(|(id, c)| Some((id.strip_prefix(SERVER_CURSOR)?.to_string(), CatalogueSeen { n: c.seq, at: c.at })))
        .collect()
}

/// A map server's cursor's name in the map's log, before its host (crates/server's maplog).
pub const SERVER_CURSOR: &str = "server/";

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
    /// What it waits on, while it waits on a worker holding its z6 tile's tail and nothing else (its
    /// slot's room then counted as free beside it: docs/pool.md §7.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    /// Its slot (0: the first; `slot_worker`): slots may be sparse, so not its place in the list.
    /// None from an older agent: its place in the list (`Status::jobs`' order) stands for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
    /// What it's doing now: its phase, its target, its bytes (jobs::Live).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<jobs::Live>,
}

impl Status {
    /// This Mac's part in the pool, in words, for `scenic status`: "lead, coordinator", "member",
    /// or "not in the pool: waiting to join (why)". An older agent's status, saying neither its
    /// pool nor why it's out: "lead, coordinator", as before.
    pub fn role(&self) -> String {
        if let Some(why) = &self.out_of_pool {
            return format!("not in the pool: waiting to join ({why})");
        }
        match self.pool.as_ref().map(|p| p.role) {
            Some(pool::Role::Member) => "member".into(),
            _ => "lead, coordinator".into(),
        }
    }

    /// Its running jobs, each with its slot: `jobs`, else an older agent's old fields (`job`,
    /// `beside`, `more`) in order; a job not saying its slot (an older agent's) has its place.
    pub fn jobs(&self) -> Vec<JobView> {
        let all: Vec<&JobView> = if self.jobs.is_empty() {
            self.job.iter().chain(self.beside.iter()).chain(self.more.iter()).collect()
        } else {
            self.jobs.iter().collect()
        };
        all.into_iter().enumerate().map(|(i, j)| JobView { slot: Some(j.slot.unwrap_or(i)), ..j.clone() }).collect()
    }

    /// Its hold (`state`, `held`), the hold's stage (`hold`: the ask waiting, `asked`; `resuming`,
    /// a going-on not yet taken up by the loop), its verdict and the out-of-touch threshold, from
    /// its jobs, pause, waits and last jobs: what every surface prints (task #846).
    pub fn say(
        &mut self,
        asked: Option<&crate::control::HoldRequest>,
        state: &crate::control::HoldState,
        resuming: bool,
    ) {
        let jobs = self.jobs();
        let finishing: Vec<String> = jobs.iter().filter(|j| j.paused.is_none()).map(|j| j.id.clone()).collect();
        self.held = state.held.clone();
        self.hold = crate::control::hold_view(asked, state, resuming, &finishing);
        self.out_of_touch_s = crate::pool::beat::OUT_OF_TOUCH_S;
        self.verdict = Some(self.verdict_of());
    }

    /// Its state in a word and a line (`Verdict`).
    fn verdict_of(&self) -> Verdict {
        use crate::control::HoldStage;
        let v = |state: &str, said: String| Verdict { state: state.into(), said };
        let jobs = self.jobs();
        let n = |k: usize| format!("{k} job{}", if k == 1 { "" } else { "s" });
        if let Some(h) = &self.hold {
            match h.stage {
                HoldStage::Pausing => return v("pausing", h.said.clone()),
                HoldStage::Paused => return v("paused", h.said.clone()),
                HoldStage::Asked => return v("asked", h.said.clone()),
                HoldStage::Resuming => return v("resuming", h.said.clone()),
            }
        }
        let live = jobs.iter().filter(|j| j.paused.is_none() && j.pausing.is_none()).count();
        if live > 0 {
            return v(
                "building",
                if jobs.len() == 1 {
                    format!("Building: {}", jobs[0].what)
                } else {
                    format!("Building: {}", n(jobs.len()))
                },
            );
        }
        let stopping = jobs.iter().filter(|j| j.paused.is_none() && j.pausing.is_some()).count();
        if stopping > 0 {
            let what = if stopping == 1 { "it's" } else { "they're" };
            return v("stopping", format!("Pausing: {} finishing what {what} on", n(stopping)));
        }
        if let Some(j) = jobs.iter().find(|j| j.paused.is_some()) {
            return v("frozen", format!("Paused: {}", j.paused.clone().unwrap_or_default()));
        }
        if let Some(p) = &self.pause {
            return v("paused", format!("Paused with the build (from {})", p.by));
        }
        if let Some(w) = self.waiting.iter().find(|w| w.why.contains("failed")) {
            return v("failed", format!("Failed: {}", w.what));
        }
        if let Some(d) = self.recent.first().filter(|d| !d.ok) {
            return v("failed", format!("Failed: {}", d.what));
        }
        if let Some(w) = self.waiting.first() {
            return v("waiting", format!("Waiting: {}", w.what));
        }
        v("idle", "Nothing to build".into())
    }

    /// Sets its jobs (`views`, each with its slot), and the old fields too unless `all_new`.
    pub fn set_jobs(&mut self, views: Vec<JobView>, all_new: bool) {
        self.all_new = all_new;
        (self.job, self.beside, self.more) = (None, None, Vec::new());
        if !all_new {
            let at = |k: usize| views.iter().find(|j| j.slot == Some(k)).cloned();
            self.job = at(0);
            self.beside = at(1);
            self.more = views.iter().filter(|j| j.slot.is_some_and(|k| k >= SLOTS)).cloned().collect();
        }
        self.jobs = views;
    }
}

/// A Mac's state as its own agent says it (`Status::say`): a word ("building", "asked",
/// "pausing", "paused", "resuming", "stopping", "frozen", "failed", "waiting", "idle") and a line.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Verdict {
    pub state: String,
    pub said: String,
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

/// A thing to know that holds nothing back: what it's about, and why, in plain words.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notice {
    pub what: String,
    pub why: String,
}

/// The notices for jobs running an older app than their Mac's agent (`Capacity::old_app`, each
/// Mac's as its lead last heard it): taken over at an update, they run that release's code to
/// their end (task #688).
fn old_app_notices(capacity: &[(String, crate::coord::Capacity)]) -> Vec<Notice> {
    capacity
        .iter()
        .flat_map(|(mac, c)| {
            c.old_app.iter().map(move |o| Notice {
                what: format!("{mac}: {}", o.job),
                why: format!(
                    "runs app {}, older than its agent's: taken over at an update, it runs that release's code until it ends",
                    o.app
                ),
            })
        })
        .collect()
}

/// The waiting lines for targets the lead's NAS budget has held back (`waits`: step, target,
/// minutes; coord::Coordinator::nas_waits), once each: those already in `waiting` (a plan's lines,
/// kept and said again between plans) replaced (task #444: each was listed twice). While the
/// budget is held by the NAS's own load (`held`, coord::Coordinator::nas_held), a line says so,
/// and each target's says it too.
fn nas_wait_lines(waiting: &mut Vec<Waiting>, waits: Vec<(String, String, u64)>, held: Option<String>) {
    const WHY: &str = "for room in the pool's NAS budget (docs/pool.md §7.7)";
    waiting.retain(|w| !w.why.ends_with(WHY));
    let by = held.as_ref().map_or(String::new(), |h| format!(", {h},"));
    if let Some(h) = &held {
        waiting.push(Waiting { what: "the NAS".into(), why: format!("{h}: new jobs wait {WHY}"), step: None });
    }
    for (step, t, min) in waits {
        waiting.push(Waiting {
            what: format!("{step} {t}"),
            why: format!("waiting {min} min{by} {WHY}"),
            step: Some(step),
        });
    }
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
    /// Cancelled by the owner (crate::control::Cancel): not done, not failed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancelled: bool,
}

/// What the agent remembers between runs (its local folder's `state.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Memory {
    /// Per job id: failures in a row and when it may run again.
    retry: BTreeMap<String, (u32, u64)>,
    /// The lead's slots' ramp (`Agent::tend_ramp`): the slots it has reached, and since when its
    /// stretch has been clean (no memory guard stop, no NAS budget overrun, no stall; unix s).
    #[serde(default)]
    ramp: Ramp,
    /// The app that last saved this: a newer one tries failed jobs again at once (it may be the
    /// fix).
    #[serde(default)]
    app: String,
    /// When each daily job last succeeded (seconds since the epoch).
    last_ok: BTreeMap<String, u64>,
    recent: Vec<Done>,
    /// When a catalogue last started (seconds since the epoch): the next waits
    /// build::CATALOGUE_EVERY_S from it, whether or not it went out.
    #[serde(default, rename = "catalog_at")] // stored key: old spelling
    catalogue_at: u64,
    /// About how long a target of each step takes here (seconds, as its jobs went lately), by
    /// `secs_key`: the forecast's, for the steps no other worker does.
    #[serde(default)]
    step_secs: BTreeMap<String, f64>,
    /// When a job last ended here (seconds since the epoch; not a daily one, which reads no cache),
    /// and this Mac's caches' last clear (room::Freed).
    #[serde(default)]
    worked_at: u64,
    #[serde(default)]
    cleared: Option<room::Freed>,
    /// The last deep clear (room::Level::Deep).
    #[serde(default)]
    cleared_deep: Option<room::Freed>,
    /// The last ask to clear them not done, and why (the last done one kept apart).
    #[serde(default)]
    declined: Option<room::Freed>,
    /// The last freeing toward the owner's disk room target (room::toward).
    #[serde(default)]
    toward: Option<room::Freed>,
}

/// What a job of `step` that ended `outcome`, its targets `done`, took on the disk over its run
/// (`mb`) told the coordinator: a run of its step, and, done whole, maybe its shared cache's fill
/// (crate::coord::Coordinator::add_job_run); a paused or stopped one's isn't a fill.
fn job_run(c: &crate::coord::Coordinator, step: &str, outcome: Outcome, done: &[(String, String)], mb: u64) {
    if outcome == Outcome::Done {
        c.add_job_run(step, &done.iter().map(|t| t.0.clone()).collect::<Vec<_>>(), mb);
    } else {
        c.add_job_disk(step, mb);
    }
}

/// What this Mac's cache folder `cache` holds of each shared cache (crate::needs::CACHES; task
/// #392), read off the disk: the whole local copies (crate::localread::held). Read as it asks, so
/// whatever emptied them (room-making, the OSM pass's clear, a hand) counts.
fn caches_here(cache: &Path) -> BTreeMap<String, crate::coord::CacheHere> {
    let have_mb = crate::localread::held(cache) >> 20;
    BTreeMap::from([(crate::localread::CACHE.to_string(), crate::coord::CacheHere { have_mb })])
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
/// (`SCENIC_COORD_PORT`; a unit test's own, `TEST_PORT`, else any free port).
fn coord_port() -> u16 {
    #[cfg(test)]
    if let Some(p) = TEST_PORT.with(|p| p.get()) {
        return p;
    }
    // (A unit test without one of its own: any free port, never the pool's. Its 127.0.0.1:8090
    // beside a running agent's wildcard listener took that lead's jobs' requests, #906.)
    #[cfg(test)]
    return 0;
    #[cfg(not(test))]
    std::env::var("SCENIC_COORD_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(crate::coord::PORT)
}

#[cfg(test)]
thread_local! {
    static TEST_PORT: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
    /// A member's broker's port, beside its lead's coordinator in the same process (`broker_port`).
    static TEST_BROKER_PORT: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
    /// What `find_root` finds in a test: nothing unless a test sets it (never the real NAS).
    static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// The NAS project folder: the share's mount (store::nasfs::NasRoot::find, mounted first when
/// `mount` is set).
pub fn find_root(mount: bool) -> Option<NasRoot> {
    // (A test never finds the mounted NAS: only a root a test set, never one on the NAS.)
    #[cfg(test)]
    {
        let _ = mount;
        TEST_ROOT.with(|t| t.borrow().clone()).map(NasRoot::local_for_test)
    }
    #[cfg(not(test))]
    NasRoot::find(mount)
}

/// Whether `root` answers: a stat of its catalogue, on **nas**'s look worker under its deadline
/// (crate::agent::nas: one look at a time, a hung one left to the kernel).
fn answers(root: &store::nasfs::NasRoot) -> bool {
    root.join("catalog").metadata().is_ok()
}

/// `probe`'s answer within eight seconds, at the start before **nas** runs (a hung share counts as
/// away, its thread left to finish on its own).
fn answers_soon(probe: fn(&NasRoot) -> bool, root: &NasRoot) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let p = root.clone();
    std::thread::spawn(move || {
        let _ = tx.send(probe(&p));
    });
    rx.recv_timeout(Duration::from_secs(8)).unwrap_or(false)
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
fn edit_held(
    edited: Option<(std::time::SystemTime, std::time::SystemTime)>,
    now: std::time::SystemTime,
) -> Option<(Duration, Duration)> {
    let (first, last) = edited?;
    let (run, age) = (now.duration_since(first).ok()?, now.duration_since(last).ok()?);
    (age < EDIT_HOLD && run < EDIT_HOLD_MAX).then(|| (age, (EDIT_HOLD - age).min(EDIT_HOLD_MAX - run)))
}

pub struct Agent {
    o: Options,
    /// This Mac's role (docs/roles.md §1, `pool::term::role`), decided as it starts: its parts are
    /// built by it (a lead's, or a member's for a member and an unknown one). A member's unknown
    /// role learnt a member's stays; a declaration naming another role restarts the agent
    /// (`declared_role`).
    role: Role,
    host: String,
    app: String,
    started: u64,
    mem: Memory,
    /// The jobs running, a slot each, one a lease it holds: `SLOTS` always there, the slots past
    /// them grown as its jobs are granted (`free_slot`) and dropped once empty (`trim_slots`).
    slots: Slots,
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
    /// The owner's in-use share (share::Share: the cores kept free of its jobs while they use the
    /// Mac), read each loop from the agent's folder.
    in_use_share: Option<share::Share>,
    /// The memory the owner gives the build's jobs on this Mac (buildmem::Memory, `scenic memory`),
    /// read each loop from the agent's folder: None, the memory guard's limit alone.
    build_memory: Option<buildmem::Memory>,
    toward_tried: Option<(u64, u64)>,
    floor_short: Option<(u64, u64, Instant)>,
    /// What a freeing under way frees toward (`goal`): set again each loop, so a target lowered or
    /// cleared midway stops it (room::toward reads it as it goes).
    toward_goal: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The memory's total and free MB as a test sets them, so a test doesn't
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
    /// The agent's own footprint as measured this loop (MB: crate::sys::footprint; 0 where a test
    /// sets the memory, `mem_set`), off what its jobs may hold (`jobs_limit`, #619).
    own_mb: u64,
    guard_backoff: BTreeMap<String, (u64, String)>,
    /// The memory this member spared its jobs in its last ask (MB: `Slot::spare_mb`).
    ask_spare_mb: Option<u64>,
    /// The budgets (docs/pool.md §7.7): every job's connections out through this agent's proxy,
    /// each Wikimedia request taken from the pool's one budget on the NAS, the job sandboxed so it
    /// has no other way out; the answers' lease. (And the Mac shared by measure: a job starts beside
    /// another only if both jobs' measured memory and disk fit what the Mac has left, a cold start
    /// alone; the disk guard on.) (Declared before `_lock`, so dropped first: the proxy's port is
    /// free before the next agent can take the lock and bind it.)
    proxy: Option<crate::budget::Proxy>,
    /// Held unless another agent runs (then this one only plans and reports: a dry run).
    _lock: Option<AgentLock>,
    /// The port the proxy listened on for the jobs this agent took over (`adopt_jobs`): its own
    /// listens there again, so they reach it (their sandbox lets them out only through it).
    proxy_port: Option<u16>,
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
    /// The page's data read from the NAS (crate::agent::pageview), served on this Mac's port by its
    /// coordinator, its broker, or with neither a viewer alone (`viewer`; tried again five minutes
    /// after it failed).
    pageview: std::sync::Arc<pageview::PageView>,
    /// The build page's Catalogues map (crate::agent::cataloguesview): made once the NAS's root is
    /// known, read only while a page asks.
    catalogues_view: Option<std::sync::Arc<cataloguesview::CataloguesView>>,
    /// Whether the Catalogues map follows this Mac's own log (the lead's) or the lead's routes.
    catalogues_lead: Option<bool>,
    /// The build page's map's finished tiles (crate::agent::builtview): made with `catalogues_view`.
    built_view: Option<std::sync::Arc<builtview::BuiltView>>,
    pageview_reading: bool,
    /// This Mac's live store (crate::agent::live), kept in its folder and answered on its port's
    /// `/work/live`: made as the loop starts, on a real Mac with the agent's lock alone.
    live: Option<std::sync::Arc<live::Store>>,
    /// The pages' cancel and retry asks waiting for the pool's run (a tick held it).
    page_acts: Vec<crate::pool::driver::Act>,
    viewer: Option<std::sync::Arc<crate::coord::Coordinator>>,
    viewer_failed: Option<Instant>,
    broker_failed: Option<Instant>,
    /// Its address over HTTPS last looked for, and what it was.
    broker_https: Option<(Instant, Option<String>)>,
    /// This member's progress file as it last wrote it (or read it, as the process began).
    held_progress: Option<resume::Progress>,
    /// The disk guard's room-making for the jobs it froze (when it began): its result decides
    /// whether they go on or one is given back.
    disk_room: Option<Instant>,
    /// **nas**: the share's mount, its probe and remounts, under a deadline (crate::agent::nas).
    nas: nas::Handle,
    /// The idle-sleep assertion it holds while it leads with leases out (`lead_awake`).
    lead_awake: Option<store::spawn::Detached>,
    /// The NAS's project folder as the loop last found it (a job ending between loops tells the
    /// lead through a client made from it).
    last_root: Option<NasRoot>,
    /// Quitting as the app asked (crate::control::Quit): whether the loop ended for it (the process
    /// then exits with `QUIT_EXIT`).
    quit_done: bool,
    /// The Mac it reads its conditions and resources from (a test's fixed one: cond::Mac::TEST).
    pub(crate) mac: cond::Mac,
    sleep: SleepWatch,
    /// The self-heal for stale SMB handles: stuck NAS calls seen, its drain and remounts
    /// (crate::agent::heal).
    heal: heal::Heal,
    /// How many of this process's stuck calls were taken in (`store::whole::stuck_after`).
    heal_seen: u64,
    /// Who this agent is ("<host> <pid>").
    me: String,
    /// The placement's per-step speeds last worked out and what from (`placement::step_speeds_key`):
    /// the history is read again only when it changed.
    step_speeds: std::cell::RefCell<placement::StepSpeedsKept>,
    /// The lead's coordinator (crate::coord), and when its contact on the NAS was last checked.
    coord: Option<std::sync::Arc<crate::coord::Coordinator>>,
    published: Option<Instant>,
    /// A member's way to the lead's coordinator (made once its contact is on the NAS).
    client: Option<crate::coord::client::Client>,
    /// Its own jobs' ends owed to the lead: ended while this process led a term it had lost without
    /// its step knowing yet (it froze, a later term made meanwhile), told to the lead through its
    /// client once its part has changed (`end_lease`, `tell_owed`); and a member's ends the lead
    /// didn't take when they ended (it didn't answer). Each told each loop until the lead answers:
    /// its lease is held meanwhile (listed in the beat, `beat_lead`; docs/coordination.md §6).
    owed: Vec<Tell>,
    /// When this member last beat to the lead (`beat_lead`).
    lead_beaten: Option<Instant>,
    /// The folders of the jobs that ended out of the pool, by lease: their hand-offs go to the lead
    /// to journal (`journaled: false`), and each folder goes once the lead answers (`told`).
    unjournaled: BTreeMap<u64, PathBuf>,
    /// A helper's: the bytes its cheap caches held (room::cheap_bytes) and when they were counted.
    cheap: Option<(Instant, u64)>,
    /// The build's pause as this agent knows it (crate::control): its coordinator's on the
    /// lead; a member's as heard in its asks and beats, or as asked for here and not yet passed on
    /// (`pause_local`). Kept in `pause.json`.
    pause: Option<crate::control::Pause>,
    pause_local: bool,
    /// The lead's pause as last mirrored to the NAS, and whether this agent's own (kept from
    /// before its coordinator was up) has been given to the coordinator.
    mirrored: Option<Option<crate::control::Pause>>,
    pause_pushed: bool,
    /// This Mac paused alone (crate::control::Hold, task #224) is control's (`control.hold`,
    /// `Agent::hold`); the generation of it last written to the NAS (or read there); when the
    /// NAS's was last read; this member's id as last seen.
    hold_mirrored: Option<u64>,
    hold_read: Option<Instant>,
    hold_me: Option<String>,
    /// **control** (crate::agent::asks, docs/agent-parts.md §4.1): the owner's asks of this Mac
    /// (its hold, the quit, the settings) taken up at once, on its thread once the agent runs.
    control: asks::Handle,
    /// **jobs**' own thread (crate::agent::slots, docs/agent-parts.md §4.5): the jobs' ends reaped, their
    /// cancels and other members' hold asks taken at once.
    jobs_part: slots::Handle,
    /// The heartbeat and status, written on a timer of their own (crate::agent::beat), from what
    /// the loop publishes.
    beat: beat::Handle,
    /// The bytes the caches hold, for the status: what room-making can free and what a clear would
    /// (room::sizes), counted on a thread of its own every ten minutes (a walk of many files), and
    /// again after a clear or a freeing; and when.
    cache_size: std::cell::RefCell<CacheCount>,
    /// A count's result from its thread, by when it began: taken in only if no count began since
    /// (a clear's or a freeing's).
    cache_sized: (std::sync::mpsc::Sender<(Instant, room::Sizes)>, std::sync::mpsc::Receiver<(Instant, room::Sizes)>),
    /// The lead's heartbeat's `Status::all_new` as a member last read it, and when (`all_new`).
    lead_all_new: Option<(Instant, bool)>,
    /// A clear, a freeing toward the room target or room-making under way.
    caches_task: Option<CachesTask>,
    /// The lead's: the pass's answers this Mac has sent to the NAS as the agent starts
    /// (crate::answers::seed), on a thread of its own; and whether it was started (once an agent).
    answers_seed: Option<std::thread::JoinHandle<Result<Vec<String>>>>,
    answers_seeded: bool,
    /// The sweep of what no current code reads of the cache (room::retire): when it last started,
    /// and the one under way.
    retired_at: Option<Instant>,
    retire_task: Option<std::thread::JoinHandle<BTreeMap<String, u64>>>,
    /// The lead's: the members' last clear or declined ask noted in the history (its time), by host.
    helper_caches: BTreeMap<String, u64>,
    /// Jobs an earlier agent left that couldn't be shown stopped (jobs::Orphan): their process
    /// groups, running work while they're still the jobs' (jobs::Group::is_the_jobs).
    orphans: Vec<jobs::Group>,
    /// The conditions the last loop saw: a change is noted in the history (crate::coord::history).
    last_cond: Option<Conditions>,
    fc_archiver: std::cell::OnceCell<Option<fcarchive::Archiver>>,
    /// **plan** (crate::agent::plan, docs/agent-parts.md §4.4): the lead's plan, on its thread (in
    /// line for a single loop); the forecast's files it handed over for the pool step to write; the
    /// newest plan's number seen (the lead's own, a member's in its grants), and each cancelled
    /// target's or job's plan number when it was cancelled: a grant from a plan no newer is dropped.
    plan: plan::Handle,
    fc_pending: (Option<Vec<u8>>, Option<fcarchive::Entry>),
    plan_n: u64,
    cancels_at: BTreeMap<String, u64>,
    /// This Mac's tiles to come in the lead's last forecast, as cache hints (room::Hints::of_tocome,
    /// task #843), with that forecast's time: the lead's from its own forecast (`take_forecast`), a
    /// member's asked of the lead (`ask_upcoming`). Room-making lets go first what none of them
    /// reads, then what's read furthest ahead (`hints`).
    upcoming: std::cell::RefCell<Option<(u64, std::sync::Arc<room::Hints>)>>,
    /// A member's ask for them under way (`ask_upcoming`), and when the last began.
    upcoming_ask: (Option<std::thread::JoinHandle<Option<(u64, room::Hints)>>>, Option<Instant>),
    /// The map's log, written in shadow (crate::agent::publish::Shadow).
    maplog: publish::Shadow,
    /// The `seq` each Mac's map server was last noted adopting (`tend_pageview`).
    adoptions: BTreeMap<String, u64>,
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
    /// Its jobs' hand-offs that ended, for the round's pool step (which follows the jobs' ends).
    held_entries: Vec<(crate::pool::journal::Entry, Option<PathBuf>)>,
    /// A sweep waiting to start asked the next pool step to re-assert the term (`Run::reassert`).
    reassert: bool,
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
    /// The lead's slots held at `RAMP_START` while the NAS's limits aren't known, and the trouble
    /// its ramp's stretch is in now, if any (`tend_ramp`; not kept: a restart starts it afresh).
    ramp_held: bool,
    ramp_trouble: Option<String>,
    restart_for: Option<String>,
    /// Why this process restarts into another role (`restart_for_part`): at once, its jobs handed
    /// over to the next process.
    role_restart: Option<String>,
    /// Why this process isn't in the pool, while it isn't (`NAS_AWAY`, `LOCK_HELD`): the status's
    /// `out_of_pool`.
    out_why: String,
}

/// The agent's part of the gate (docs/inputs.md §4.2, §4.7).
#[derive(Default)]
struct Gate {
    /// The inputs folders listed off the loop (the lead's), with the root it
    /// lists.
    watch: Option<(NasRoot, crate::inputs::watch::Watch)>,
    view: std::cell::RefCell<Vec<crate::inputs::view::InputView>>,
    cache: std::cell::RefCell<crate::inputs::view::Cache>,
    full: std::cell::RefCell<BTreeSet<String>>,
    /// The descriptions' credits' digest as last read (None inside: there are none), for the
    /// catalogue's key while they can't be read.
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
/// What the build page's regions were made of: the regions' accepted version and the catalogue.
type RegionsKey = (String, Option<u64>);

/// A clear, a freeing toward the room target or room-making under way on a thread of its own (`Agent::caches_task`).
struct CachesTask {
    /// The clear's ask (None: a freeing toward the room target, or room-making), and when it began.
    ask: Option<room::ClearRequest>,
    /// A freeing toward the owner's disk room target (room::toward): the target, and the room it
    /// frees toward as it began (`goal`: the target, or more for a job waiting for it), bytes.
    toward: Option<(u64, u64)>,
    /// Room made for a job the disk guard froze (`disk_guard`).
    disk: bool,
    began: Instant,
    thread: std::thread::JoinHandle<Result<room::Freed>>,
}

impl Agent {
    pub fn new(o: Options) -> Result<Agent> {
        Agent::new_with(o, answers)
    }

    /// `new`, with how it asks whether the NAS's folder answers (`probe`: the real one, `answers`;
    /// a test's).
    fn new_with(mut o: Options, probe: fn(&NasRoot) -> bool) -> Result<Agent> {
        // Its programs' folder resolved (launchd starts it by `app/current`): its jobs start from
        // its own version's folder, which they name in their command, so the agent after it lists
        // that version as theirs (`job_apps`) once `current` names a newer one.
        o.bin = o.bin.canonicalize().unwrap_or(o.bin);
        std::fs::create_dir_all(&o.home).with_context(|| format!("create {}", o.home.display()))?;
        let mut lock = AgentLock::try_take(&o.home)?;
        // (Held by an agent outside this one's launcher: ended, and the lock taken over.)
        if lock.is_none() && !o.dry_run {
            lock = takeover::take_over(&o.home, takeover::WAIT)?;
        }
        if lock.is_none() {
            anyhow::ensure!(
                o.dry_run,
                "another agent is running on this Mac (its lock is {})",
                o.home.join("agent.lock").display()
            );
            eprintln!("agent: another agent is running; planning only");
        }
        if lock.is_none() {
            o.dry_run = true;
        }
        let mut mem: Memory = std::fs::read(o.home.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let app = app_version(&o.bin);
        if mem.app != app {
            if !mem.retry.is_empty() {
                eprintln!("agent: a new app ({app}); failed jobs may run again at once");
            }
            mem.retry.clear();
            mem.app = app.clone();
        }
        let me = format!("{} {}", store::sys::host_name(), std::process::id());
        // The budgets' proxy for the jobs the agent before left running, at once (not once this
        // one has joined the pool, which may take minutes): on the port they were told, which the
        // agent before freed before its lock (its `proxy` dropped first), so their connections are
        // refused for moments at most, within their retries. (That port not to be had: tried
        // again by `tend_proxy`.)
        let early_proxy = match (&lock, told_proxy_port(&o.home), o.root.clone().or_else(|| find_root(false))) {
            (Some(_), Some(port), Some(root)) if !o.dry_run => crate::budget::listen(port)
                .ok()
                .and_then(|l| crate::budget::start_proxy(&root, &me, l).ok())
                .inspect(|p| eprintln!("agent: the budgets' proxy listens on {} (its jobs left running)", p.url())),
            _ => None,
        };
        // The pool, as this process starts (the NAS answering; else it starts nothing until it does,
        // then restarts into it): this Mac's member takes its first step, which says its part: it
        // leads or works as a member.
        let mut run: Option<pool::Run> = None;
        // (Out of the pool: not known, a member's parts; restarted into the pool, it's read.)
        #[cfg(not(test))]
        let mut role = Role::Unknown;
        #[cfg(test)]
        let mut role = if o.member { Role::Member } else { Role::Lead };
        let mut out_why = NAS_AWAY.to_string();
        let mut first: Option<(crate::pool::driver::Out, NasRoot)> = None;
        if lock.is_some() && !o.dry_run {
            if let Some(r) = o.root.clone().or_else(|| find_root(false)).filter(|r| answers_soon(probe, r)) {
                {
                    let nas: pool::SharedNas =
                        std::sync::Arc::new(nasread::Deadlined::new(std::sync::Arc::new(r.no_retry())));
                    let locks = o.home.parent().unwrap_or(&o.home).to_path_buf();
                    // (The member's lock held by another process, most often the agent before this
                    // one still draining after an update: tried again each second for `JOIN_WAIT`,
                    // so this process joins as it goes rather than restarting into the pool later.)
                    let until = Instant::now() + JOIN_WAIT;
                    let mut said = false;
                    let mut said_err = false;
                    let side = loop {
                        match pool::Side::open(&o.home, &o.home.join("pool"), &locks, &app, nas.clone()) {
                            Ok(Some(s)) => break Some(s),
                            // (A passing error, its folder unreadable a moment: tried again within
                            // the wait as a lock held is; past it, the start fails as before.)
                            Err(e) if Instant::now() < until && !STOP.load(Ordering::SeqCst) => {
                                // (Said once: a lasting error isn't logged each second.)
                                if !said_err {
                                    eprintln!(
                                        "agent: opening this Mac's member in the pool: {e:#}; trying again for up to {} s",
                                        JOIN_WAIT.as_secs()
                                    );
                                    said_err = true;
                                }
                                std::thread::sleep(JOIN_TRY);
                            }
                            Err(e) => return Err(e),
                            Ok(None) if Instant::now() < until && !STOP.load(Ordering::SeqCst) => {
                                if !said {
                                    eprintln!(
                                        "agent: this Mac's member's lock is held by another process; trying it again for up to {} s",
                                        JOIN_WAIT.as_secs()
                                    );
                                    said = true;
                                }
                                std::thread::sleep(JOIN_TRY);
                            }
                            Ok(None) => break None,
                        }
                    };
                    match side {
                        Some(side) => {
                            let (r1, out) = pool::Run::start(side, &o.home);
                            role = role_by(&r1);
                            eprintln!(
                                "agent: the pool is on: this Mac's member {} is {} (term {})",
                                r1.side.member().id,
                                match role {
                                    Role::Lead => "the declared lead",
                                    Role::Member => "a member",
                                    Role::Unknown => "of a role not known yet (no lead declared): a member's parts",
                                },
                                out.term
                            );
                            run = Some(r1);
                            first = Some((out, r.clone()));
                        }
                        // (Its lock still held past `JOIN_WAIT`: this one starts nothing new, keeps
                        // the jobs it took over beating their leases, and tries the lock again each
                        // loop, restarting into the pool at once when it has it.)
                        None => {
                            eprintln!("agent: another process is this Mac's member in the pool; waiting for its lock");
                            out_why = LOCK_HELD.to_string();
                        }
                    }
                }
            }
        }
        // The lead's agent coordinates (not a member's, nor a dry run's, nor a process not in the
        // pool), with a root of its own too (a test's, on a port of its own, SCENIC_COORD_PORT).
        let pooled = run.is_some();
        let coord = if role == Role::Lead && lock.is_some() && !o.dry_run && pooled {
            make_coordinator(&o, run.as_ref(), &app, &store::sys::host_name()).map(std::sync::Arc::new)
        } else {
            None
        };
        if let (Some(r), Some((out, root))) = (run.as_mut(), &first) {
            pool::took_up(r, coord.as_deref(), &out.events, out.leads, &store::sys::host_name(), &[]);
            // (The terms' events a member's process before this one kept, for this coordinator's
            // history; and this first step's.)
            if let Some(c) = &coord {
                lead::replay(&o.home.join("pool"), c);
            }
            lead::after(r, out, root, &o.home, coord.as_deref());
        }
        // The build's pause as this agent last knew it (a member that can't reach the lead stays
        // as it was).
        let hold = crate::control::read_hold(&o.home);
        let hold_snap = bus::Snap::new(hold.clone());
        let mut beat = beat::Handle::new(o.home.clone(), hold_snap.clone());
        let control = asks::Handle::new(o.home.clone(), hold_snap, beat.sender());
        // (Given a root, that one, never mounted; a test never mounts nor unmounts the real share.)
        let finder: nas::Finder = match o.root.clone() {
            Some(r) => std::sync::Arc::new(move |_| Some(r.clone())),
            None => std::sync::Arc::new(find_root),
        };
        let nas = nas::Handle::new(finder, o.root.is_none() && !cfg!(test), probe);
        let jobs_part = slots::Handle::new(o.home.clone(), control.jobs.clone());
        let pause: Option<crate::control::Pause> =
            std::fs::read(o.home.join("pause.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        // (A record that doesn't read: none.)
        let catalogues = read_published(&o.home);
        // **plan** (crate::agent::plan): the lead's, or a dry run's (a member plans nothing).
        let host = test_worker().unwrap_or_else(store::sys::host_name);
        let plan = if role != Role::Lead && !o.dry_run {
            plan::Handle::none()
        } else {
            plan::Handle::new(plan::Planner::new(
                o.clone(),
                host.clone(),
                app.clone(),
                now_s(),
                coord.clone(),
                lock.is_some(),
                control.hold.clone(),
                catalogues,
            ))
        };
        if plan.plans() {
            beat.watch_plan(plan.running());
        }
        if role == Role::Member && lock.is_some() && !o.dry_run {
            drop_lead_caches(&o.home);
        }
        let mut a = Agent {
            role,
            plan,
            cancels_at: BTreeMap::new(),
            plan_n: 0,
            fc_pending: Default::default(),
            host,
            app,
            started: now_s(),
            mem,
            slots: Slots::default(),
            free_set: None,
            grace: DRAIN_GRACE,
            room_target: None,
            in_use_share: None,
            build_memory: None,
            toward_tried: None,
            floor_short: None,
            toward_goal: Default::default(),
            mem_set: None,
            guard_on: None,
            guard_hold: None,
            guard_last: None,
            sampler: memguard::Sampler::new(SLOTS),
            total: 0,
            own_mb: 0,
            guard_backoff: BTreeMap::new(),
            ask_spare_mb: None,
            proxy: early_proxy,
            proxy_port: None,
            resume: false,
            placement: false,
            brokers: false,
            broker: None,
            pageview: pageview::PageView::new(),
            catalogues_view: None,
            catalogues_lead: None,
            built_view: None,
            pageview_reading: false,
            live: None,
            page_acts: Vec::new(),
            viewer: None,
            viewer_failed: None,
            broker_failed: None,
            broker_https: None,
            held_progress: None,
            disk_room: None,
            nas,
            lead_awake: None,
            last_root: None,
            quit_done: false,
            mac: cond::Mac::Real,
            sleep: SleepWatch::default(),
            heal: Default::default(),
            heal_seen: 0,
            _lock: lock,
            o,
            me,
            step_speeds: Default::default(),
            coord,
            published: None,
            client: None,
            owed: Vec::new(),
            lead_beaten: None,
            unjournaled: BTreeMap::new(),
            cheap: None,
            pause,
            pause_local: false,
            mirrored: None,
            pause_pushed: false,
            jobs_part,
            control,
            beat,
            hold_mirrored: None,
            hold_read: None,
            hold_me: None,
            cache_size: Default::default(),
            cache_sized: std::sync::mpsc::channel(),
            lead_all_new: None,
            caches_task: None,
            answers_seed: None,
            answers_seeded: false,
            retired_at: None,
            retire_task: None,
            helper_caches: BTreeMap::new(),
            orphans: Vec::new(),
            last_cond: None,
            fc_archiver: Default::default(),
            upcoming: Default::default(),
            upcoming_ask: (None, None),
            maplog: Default::default(),
            adoptions: BTreeMap::new(),
            member_id: run.as_ref().map(|r| r.side.member().id.clone()),
            ticking: run.as_ref().map(|r| r.ticking.clone()),
            publish_gen: Default::default(),
            unpublishing: None,
            pool_skips: 0,
            pool_hung: None,
            ramp_held: true,
            ramp_trouble: None,
            pool_seen: run.as_ref().map(|r| r.pool_view()),
            held_entries: Vec::new(),
            reassert: false,
            pool: run.map(|r| std::sync::Arc::new(std::sync::Mutex::new(r))),
            ticker: None,
            restart_for: None,
            role_restart: None,
            out_why,
        };
        // (The jobs the agent before this one left running, taken over before anything is
        // planned or offered.)
        a.adopt_jobs();
        Ok(a)
    }

    /// The keys to plan with, as a start checks an offered step's targets against them.
    fn planning_keys_here(&self, root: &NasRoot) -> Result<build::Keys> {
        planning_keys(root, self.handoff_bases(root))
    }

    /// Where hand-offs wait to be drained into the journal: the coordinator's journal on this Mac.
    fn handoff_bases(&self, _root: &NasRoot) -> Vec<PathBuf> {
        handoff_bases(&self.o)
    }

    /// Where a helper's leased jobs save (`<outbox>/<lease>/`), and its agent puts their result, until
    /// the coordinator has them.
    fn outbox(&self) -> PathBuf {
        self.o.home.join("outbox")
    }

    /// A member's way to the lead's coordinator; None (with why, in `waiting`) when there's none.
    fn client(
        &mut self,
        root: &store::nasfs::NasRoot,
        waiting: &mut Vec<Waiting>,
    ) -> Option<&crate::coord::client::Client> {
        // (A coordinator that last answered as an earlier term's lead than the pool's view: the
        // lead before a change of lead, kept on as its Mac's broker, which answers every ask with nothing
        // and never fails a request, so the contact was never read again. Task #815: the M4,
        // paused through a change of lead, got no job for hours. Its contact read again. By the term,
        // not the member alone: a view lagging the term, the new lead's answer is kept, not read
        // again at every ask until the view catches up.)
        let lead = self.pool_seen.as_ref().and_then(|p| p.lead.as_ref()).and_then(|v| v.lead.as_ref());
        if let (Some(c), Some(l)) = (&self.client, lead) {
            let stale = c.answered().filter(|a| a.term.is_some_and(|t| t < l.term)).and_then(|a| a.member);
            if let Some(m) = stale {
                eprintln!(
                    "agent: the coordinator it asked answers as {m}, not term {}'s lead {}: reading the lead's contact again",
                    l.term, l.host
                );
                self.client = None;
            }
        }
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
        let r = crate::pool::reachable::Reach {
            member: String::new(),
            what: "coordinator".into(),
            ok: false,
            at: 0,
            since: 0,
            cause: Some(cause),
            why: text.clone(),
        };
        match cause {
            crate::pool::reachable::Cause::Other => format!("{host}'s coordinator can't be reached: {text}"),
            _ => format!(
                "{host}'s coordinator can't be reached from this Mac: {} ({text})",
                crate::pool::reachable::words(&r, &host)
            ),
        }
    }

    /// Hands this member's finished tasks back (a task leased from the lead's coordinator, run in
    /// its outbox folder: crate::offload): its outputs and what it took, or its failure. A folder
    /// stays until the coordinator has it (or says its lease is gone: then its work was offered
    /// again, and it's dropped), across restarts; one left by a task that died with an earlier agent
    /// is given back. (A job's work goes through the journal: crate::agent::pool.)
    fn send_outbox(&mut self, root: &store::nasfs::NasRoot, waiting: &mut Vec<Waiting>) {
        let Ok(rd) = std::fs::read_dir(self.outbox()) else { return };
        // (Every slot's: a member's packed jobs' too.)
        let running: Vec<PathBuf> = self
            .slots
            .iter()
            .filter_map(|s| match &s.lease {
                Some(Held::Leased { dir, .. }) => Some(dir.clone()),
                _ => None,
            })
            .collect();
        // (A job's folder, run out of the pool, drained into the journal: pool::Run::drain_outbox;
        // tasks' alone here.)
        let dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !running.contains(p) && !p.join("work.json").exists())
            .collect();
        if dirs.is_empty() {
            return;
        }
        let Some(client) = self.client(root, waiting) else { return };
        for d in dirs {
            let Some(lease) = d.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok()) else {
                continue;
            };
            let result: Option<serde_json::Value> =
                std::fs::read(d.join("result.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
            let sent = (|| -> Result<crate::coord::client::Handed> {
                // A task's: its outputs and what it took.
                if let Some(t) = result
                    .as_ref()
                    .filter(|r| r["ok"].as_bool() == Some(true))
                    .map(|r| &r["task"])
                    .filter(|t| t.is_object())
                {
                    let d = crate::coord::Done {
                        lease,
                        outputs: serde_json::from_value(t["outputs"].clone())?,
                        removed: serde_json::from_value(t["removed"].clone())?,
                        secs: t["secs"].as_f64().unwrap_or(0.0),
                        peak_mb: t["peak_mb"].as_u64().unwrap_or(0),
                        timings: read_timings(&d),
                        ..Default::default()
                    };
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
                // (What its targets held at least, its job's end said: the lead's floors.)
                let floors: Vec<(String, memguard::Floor)> =
                    result.as_ref().and_then(|r| serde_json::from_value(r["floors"].clone()).ok()).unwrap_or_default();
                if interrupted {
                    client.give_back_with(lease, &why, &floors).map(|()| crate::coord::client::Handed::Taken)
                } else {
                    client.fail_with(lease, &why, None, &floors).map(|()| crate::coord::client::Handed::Taken)
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
                    waiting.push(Waiting {
                        step: None,
                        what: "Handing work back".into(),
                        why: format!("{e:#}; trying again"),
                    });
                    break;
                }
            }
        }
    }

    /// What a job of `step` is given after its targets (the pass's `date`, where this Mac's caches
    /// and programs are): the lead's own jobs' and a member's alike.
    fn step_args(&self, step: &str, date: &str) -> Vec<String> {
        step_args(&self.o, step, date)
    }

    /// A member's tiles to come in the lead's forecast (`/work/tocome`, `{"raw": true}`: every
    /// Mac's, as the lead last made them), this Mac's taken as cache hints (`upcoming`, task #843):
    /// asked on a thread of its own (the loop never waits on the lead), at most every
    /// `UPCOMING_EVERY`; the last answer kept while a new one fails.
    fn ask_upcoming(&mut self, root: &store::nasfs::NasRoot) {
        let done = self.upcoming_ask.0.as_ref().is_some_and(|h| h.is_finished());
        if done {
            if let Some(Ok(Some((at, h)))) = self.upcoming_ask.0.take().map(|h| h.join()) {
                *self.upcoming.borrow_mut() = Some((at, std::sync::Arc::new(h)));
            }
        }
        let due = self.upcoming_ask.1.is_none_or(|t| t.elapsed() >= UPCOMING_EVERY);
        if self.upcoming_ask.0.is_some() || !due {
            return;
        }
        let Some(c) = self.client(root, &mut Vec::new()) else { return };
        let (urls, token, host) = (c.urls(), c.token(), self.host.clone());
        self.upcoming_ask.1 = Some(Instant::now());
        let ask = move || -> Option<(u64, room::Hints)> {
            let c = crate::coord::client::Client::at(urls, token, &host);
            let (code, v) = c.post_json("/work/tocome", &serde_json::json!({ "raw": true })).ok()?;
            upcoming_of(code, &v, &host)
        };
        match std::thread::Builder::new().name("upcoming".into()).spawn(ask) {
            Ok(h) => self.upcoming_ask.0 = Some(h),
            Err(e) => eprintln!("agent: the tiles to come's thread: {e}"),
        }
    }

    /// A member's next job: work of the plan the lead's coordinator leases it (a job's worth of
    /// a shared step's targets, none needing more memory than it spares), built from the pass it
    /// says, saving into the lease's outbox folder. Asked only when it could start now. (Its jobs
    /// alone: `helper_grants`.)
    #[cfg(test)]
    fn helper_job(&mut self, root: &store::nasfs::NasRoot, c: &Conditions, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
        self.helper_grants(root, c, waiting).into_iter().map(|(_, spec, _)| spec).collect()
    }

    /// A member's next jobs (`helper_job`), each with the slot it goes in: as many as it has slots
    /// free (`want`), each granted put in a slot of its own (`take_grant`).
    fn helper_grants(
        &mut self,
        root: &store::nasfs::NasRoot,
        c: &Conditions,
        waiting: &mut Vec<Waiting>,
    ) -> Vec<(usize, JobSpec, u64)> {
        if !self.takes_more() {
            return Vec::new();
        }
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
            waiting.push(Waiting {
                step: None,
                what: "Building".into(),
                why: "restarting into the newly installed app".into(),
            });
            return Vec::new();
        }
        // (Nor while Scenic quits, or this Mac is paused alone: no new work taken.)
        if let Some(why) = self.quit_why().or_else(|| self.hold_why()) {
            waiting.push(Waiting { step: None, what: "Building".into(), why });
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
        if let Some((t, n, at)) =
            self.floor_short.filter(|f| f.0 == self.floor() && f.2.elapsed() < Duration::from_secs(600))
        {
            if room::disk_free(&self.o.home).unwrap_or(0) < n {
                waiting.push(Waiting { step: None, what: "Building".into(), why: format!("a job was given back for the disk room target ({}) {} min ago: asking for work again once the disk has room past it, or in ten minutes", room::size(t), at.elapsed().as_secs() / 60) });
                return Vec::new();
            }
        }
        let mut can = helper_steps(room::disk_free(&self.o.home).unwrap_or(0), cheap, self.floor());
        // (Every step to every member: every step the lead may offer, its room by the measures at
        // the lead's coordinator, this Mac's disk room said with the ask; crate::agent::every.)
        for s in steps::TABLE.iter().map(|r| r.name).filter(|s| every::offered(s)) {
            if !can.iter().any(|c| c == s) {
                can.push(s.to_string());
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
        // (Beside a running job no room is made and the owner's target holds (`try_start_said`):
        // only the work the disk has room for now, past the target and the jobs' reservations;
        // asked for regardless, each grant would be given back at once and offered again, #703.)
        let floor = self.floor();
        if floor > 0 && self.running_jobs() > 0 {
            let (free, held) = (self.disk_free(), floor.saturating_add(self.reserved(None)));
            can = room_beside(can, free, held);
            if can.is_empty() {
                waiting.push(Waiting {
                    step: None,
                    what: "Building".into(),
                    why: format!(
                        "no more work beside the jobs running: {} GB free, the disk room target and their reservations {} GB",
                        free >> 30,
                        held >> 30
                    ),
                });
                return Vec::new();
            }
        }
        if can.is_empty() {
            let need = helper_need(crate::trees::task::KIND);
            let floor = self.floor();
            let past =
                if floor > 0 { format!(", past the disk room target's {}", room::size(floor)) } else { String::new() };
            waiting.push(Waiting {
                step: None,
                what: "Building".into(),
                why: format!(
                    "the disk has too little room ({:.1} GB free needed{past}, with what its caches can free)",
                    (need + room::margin(need)) as f64 / (1u64 << 30) as f64
                ),
            });
            return Vec::new();
        }
        // (The old field for the limit only while a member of the pool is on an older app: its capacity says it.)
        let all_new = self.all_new(Some(root));
        let ask = crate::coord::Ask {
            kind: "native".into(),
            label: Some(format!("{} (helper)", self.host)),
            can,
            // (The memory its owner gives the build here, within the guard's limit: `build_room`.)
            mem_mb: self.build_room(),
            cores: std::thread::available_parallelism().map_or(4, |n| n.get() as u32),
            app: Some(self.app.clone()),
            limit_mb: Some(self.jobs_limit(self.total_mb())).filter(|&l| l > 0 && !all_new),
            // (Its free space now, less the owner's target and its jobs' reservations.
            // What its caches could free isn't counted: nothing frees it first.)
            disk_mb: Some(self.disk_room() >> 20),
            cap: Some(self.capacity(c)),
            want: want(self.job_slots(), self.running_jobs()),
            ..Default::default()
        };
        self.ask_spare_mb = Some(ask.mem_mb);
        let Some(client) = self.client(root, waiting) else { return Vec::new() };
        let asked = client.ask(&ask);
        // (Work given: the build isn't paused.)
        if matches!(asked, Ok(Some(_))) {
            self.know_pause(None);
        }
        match asked {
            Ok(Some(mut g)) => {
                // (Several granted at once: each in a slot of its own.)
                let more = std::mem::take(&mut g.more);
                let mut got = Vec::new();
                for g in std::iter::once(g).chain(more) {
                    // (The newest plan the lead granted from: a cancel taken after it drops a grant
                    // from a plan no newer, `start_granted`.)
                    let plan = g.plan;
                    self.plan_n = self.plan_n.max(plan);
                    got.extend(self.take_grant(g, root, waiting).map(|(k, spec)| (k, spec, plan)));
                }
                got
            }
            Ok(None) => {
                self.know_pause(None);
                waiting.push(Waiting {
                    step: None,
                    what: "Building".into(),
                    why: "the lead has nothing for this Mac now".into(),
                });
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

    /// A job granted to this member (`helper_job`), put in its slot: the first free, or a new one
    /// (`free_slot`). Its lease in the slot, and its job to start there; None when it can't be
    /// (given back, or failed, as before).
    fn take_grant(
        &mut self,
        grant: crate::coord::Grant,
        root: &store::nasfs::NasRoot,
        waiting: &mut Vec<Waiting>,
    ) -> Option<(usize, JobSpec)> {
        let needs = Needs { nas: true };
        let fail = |a: &Self, lease: u64, why: &str| {
            if let Some(c) = &a.client {
                c.fail(lease, why, None).ok();
            }
        };
        let Some(k) = self.free_slot() else {
            if let Some(c) = &self.client {
                if let Err(e) = c.give_back(grant.lease, "no slot free on this Mac") {
                    eprintln!("agent: giving lease {} back: {e:#} (it lapses)", grant.lease);
                }
            }
            return None;
        };
        self.slots[k].granted = Some(grant.threads).filter(|&t| t > 0);
        self.slots[k].reserve_mb = Some(grant.disk_mb).filter(|&d| d > 0);
        self.slots[k].spare_mb = self.ask_spare_mb;
        match grant {
            crate::coord::Grant { lease, term, work: crate::coord::Granted::Job { step, targets, pass }, .. }
                if every::offered(&step) || (self.resume_on() && resume::RESUMABLE.contains(&step.as_str())) =>
            {
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
                    Some(l) => serde_json::to_vec(&pool::JobKept {
                        step: step.clone(),
                        targets: if resumed { Vec::new() } else { targets.clone() },
                        lease: l,
                    }),
                    None => serde_json::to_vec(&build::Work { step: step.clone(), targets: targets.clone() }),
                };
                let kept = std::fs::create_dir_all(&dir)
                    .and_then(|()| std::fs::write(dir.join("work.json"), work.unwrap_or_default()));
                if let Err(e) = kept {
                    waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e}") });
                    fail(self, lease, &format!("its outbox: {e}"));
                    return None;
                }
                // The lead's own command for it, its saves handed off (SCENIC_HANDOFF).
                let mut cmd = vec![
                    "/usr/bin/env".to_string(),
                    format!("SCENIC_HANDOFF={}", dir.display()),
                    format!("SCENIC_COSTS={}", dir.join("costs.jsonl").display()),
                    format!("{}={}", crate::timings::TIMINGS_ENV, dir.join(TIMINGS_FILE).display()),
                ];
                if resumed {
                    // (Its target the pass's date, its key the planet's path in the NAS's folder.)
                    let (date, planet) = targets.first().cloned().unwrap_or_default();
                    cmd.extend(self.osm_pass_cmd(root, &root.join(&planet), &date));
                    let lease_kept = match pooled {
                        Some(l) => Held::Pooled { id: lease, term: l.term, dir, own: false },
                        None => Held::Leased { lease, dir },
                    };
                    self.slots[k].lease = Some(lease_kept);
                    let what =
                        format!("Reading the OpenStreetMap planet of {date} (resumed here, its progress on this Mac)");
                    return Some((
                        k,
                        JobSpec {
                            id: format!("{step} {date}"),
                            what,
                            cmd,
                            needs,
                            restart_after_sleep: true,
                            record: None,
                        },
                    ));
                }
                cmd.extend(self.step_cmd(root, &step, &targets, &pass));
                let n = targets.len();
                let id = format!("{step} {}", targets.first().map(|t| t.0.as_str()).unwrap_or(""));
                let what = format!("{}, for the lead", steps::said(&step, n));
                self.slots[k].lease = Some(match pooled {
                    Some(l) => Held::Pooled { id: lease, term: l.term, dir, own: false },
                    None => Held::Leased { lease, dir },
                });
                Some((
                    k,
                    JobSpec {
                        id,
                        what,
                        cmd,
                        needs,
                        restart_after_sleep: true,
                        record: Some(build::Work { step, targets }),
                    },
                ))
            }
            crate::coord::Grant { lease, work: crate::coord::Granted::Task { task, task_kind, .. }, .. } => {
                // A task (a z6 tile's tail, or a 3D buildings' z8 tile, for the lead's job), run here
                // by `scenic run-task` over its files fetched from the coordinator, reading the NAS's
                // data where it lies (only reading it: crate::unit::Tools::stores_read_only).
                let dir = self.outbox().join(lease.to_string());
                let spec = dir.join("spec.json");
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&spec, task.to_string())) {
                    waiting.push(Waiting { step: None, what: "Building".into(), why: format!("{e}") });
                    fail(self, lease, &format!("its outbox: {e}"));
                    return None;
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
                    s(&self.slot_scratch(k).join("task")),
                    "--result".into(),
                    s(&dir.join("task.json")),
                    // (What a page reads through the coordinator, read on the NAS here.)
                    "--root".into(),
                    jobs::root_arg(root),
                ];
                let unit = task["unit"].as_str().unwrap_or("").to_string();
                self.slots[k].lease = Some(Held::Leased { lease, dir });
                // (Its kind as the coordinator sent it; an older lead's grant names none: its
                // spec's, crate::offload::task_kind, until every lead sends it.)
                let kind = if task_kind.is_empty() { crate::offload::task_kind(&task) } else { task_kind };
                let what = task_what(&kind, &task, &unit);
                Some((
                    k,
                    JobSpec {
                        id: JobId { step: &kind, rest: Some(&unit) }.to_string(),
                        what,
                        cmd,
                        needs: Needs { nas: true },
                        restart_after_sleep: false,
                        record: None,
                    },
                ))
            }
            // (A resumable job granted as resuming was switched off here: given back, not failed.)
            crate::coord::Grant { lease, work: crate::coord::Granted::Job { step, .. }, .. }
                if resume::RESUMABLE.contains(&step.as_str()) =>
            {
                if let Some(c) = &self.client {
                    if let Err(e) = c.give_back(lease, "resuming is off on this Mac now") {
                        eprintln!("agent: giving lease {lease} of {step} back: {e:#} (it lapses)");
                    }
                }
                None
            }
            g => {
                fail(self, g.lease, "this helper can't do that work");
                None
            }
        }
    }

    /// Leading: this Mac's next jobs, asked of its own coordinator as a member asks its lead's
    /// (crate::coord::Coordinator::ask_own, docs/pool.md §7.2), each with the slot it goes in: as
    /// many as it has slots free (`want`), packed by the same measures as a member's, the lead's own
    /// planned jobs (`jobs`) among them but those that can't start now for a reason of this agent's
    /// own (`wait_reason`). It asks with its Mac's room: the memory guard's limit, its disk room
    /// (`disk_room`) and capacity; with no job here, for one job, its disk not bounded (the owner's
    /// target is waited on as it starts, room made for it then, the disk guard
    /// behind it: a stale disk floor never holds the build).
    fn lead_grants(
        &mut self,
        jobs: &[JobSpec],
        plan: u64,
        c: &Conditions,
        root: &store::nasfs::NasRoot,
        waiting: &mut Vec<Waiting>,
    ) -> Vec<(usize, JobSpec, u64)> {
        if !self.takes_more() {
            return Vec::new();
        }
        // (A dry run: the plan's first job that could start said, nothing asked for.)
        if self.o.dry_run {
            if let Some(j) = jobs.iter().find(|j| self.wait_reason(j, c).is_none()) {
                waiting.push(Waiting {
                    step: step_of(&j.id),
                    what: j.what.clone(),
                    why: "would start now (dry run)".into(),
                });
            }
            return Vec::new();
        }
        let Some(co) = self.coord.clone() else {
            waiting.push(Waiting {
                step: None,
                what: "Building".into(),
                why: "this Mac's coordinator isn't running: no job is granted".into(),
            });
            return Vec::new();
        };
        // (Scenic quitting, or this Mac paused alone: no new work taken.)
        if let Some(why) = self.quit_why().or_else(|| self.hold_why()) {
            waiting.push(Waiting { step: None, what: "Building".into(), why });
            return Vec::new();
        }
        let mut skip: BTreeSet<String> =
            jobs.iter().filter(|j| !offered_job(j) && self.wait_reason(j, c).is_some()).map(|j| j.id.clone()).collect();
        // (A catalogue while the pool's records don't reflect the journal (`Gates::publish`): not asked
        // for, said waiting here. Granted, it would only wait in `try_start_said`, and be granted,
        // logged and leased again every loop: 41 times in the 40 minutes of task #630. The gates as
        // the round's last pool step left them: the grant never waits on the pool's run.)
        if self.pool_seen.as_ref().is_some_and(|p| !p.gates.publish()) {
            for j in jobs.iter().filter(|j| step_of(&j.id).is_some_and(|s| pool::PUBLISHES.contains(&s.as_str()))) {
                skip.insert(j.id.clone());
                waiting.push(Waiting { step: step_of(&j.id), what: j.what.clone(), why: UNPUBLISHED.into() });
            }
        }
        let mut can: Vec<String> = steps::TABLE.iter().map(|r| r.name.to_string()).collect();
        for j in jobs {
            let step = j.record.as_ref().map(|w| w.step.clone()).or_else(|| step_of(&j.id)).unwrap_or_default();
            if !can.contains(&step) {
                can.push(step);
            }
        }
        let total = match self.total_mb() {
            0 => self.mac_total_mb(),
            t => t,
        };
        let limit = self.jobs_limit(total);
        let alone = self.running_jobs() == 0;
        // (A job waiting on a worker: said in its capacity, `Capacity::waiting`, as a member's is.)
        let ask = crate::coord::Ask {
            kind: "native".into(),
            label: Some(format!("{} (lead)", self.host)),
            can,
            // (The memory its owner gives the build here, within the guard's limit: `build_room`.)
            mem_mb: if limit > 0 || self.build_memory.is_some() { self.build_room() } else { u64::MAX >> 1 },
            cores: mac_cores(),
            app: Some(self.app.clone()),
            limit_mb: Some(limit).filter(|&l| l > 0 && !co.all_new()),
            disk_mb: (!alone).then(|| self.disk_room() >> 20),
            cap: Some(self.capacity(c)),
            want: if alone { 1 } else { want(self.job_slots(), self.running_jobs()) },
            ..Default::default()
        };
        let Some(mut g) = co.ask_own(&ask, &skip) else { return Vec::new() };
        let more = std::mem::take(&mut g.more);
        let mut got = Vec::new();
        for g in std::iter::once(g).chain(more) {
            got.extend(self.own_grant(g, &co, jobs, root, waiting).map(|(k, spec)| (k, spec, plan)));
        }
        got
    }

    /// The jobs granted (`lead_grants`, `helper_grants`), each started in its slot: its conditions,
    /// its room on the disk (`try_start_said`); one beside a job waiting on a worker yields to it
    /// (`yields`).
    fn start_granted(
        &mut self,
        granted: Vec<(usize, JobSpec, u64)>,
        c: &Conditions,
        root: Option<&store::nasfs::NasRoot>,
        waiting: &mut Vec<Waiting>,
    ) {
        for (k, spec, plan) in granted {
            // (A grant from a plan no newer than a cancel of its job or a target of it: dropped,
            // its lease given back; docs/agent-parts.md §5.4.)
            if let Some(why) = self.cancelled_since(&spec, plan) {
                eprintln!("agent: {} granted from plan {plan}, dropped: {why}", spec.id);
                self.end_lease(k, Outcome::Interrupted, &[], &why);
                continue;
            }
            let beside_waiting = (0..self.slots.len())
                .find(|&j| j != k && self.waiting_on(j).is_some())
                .and_then(|j| self.slots[j].running.as_ref().map(|r| r.spec.id.clone()));
            let said = waiting.len();
            if self.try_start_said(k, spec, c, root, waiting) {
                self.slots[k].yields = beside_waiting;
            } else if self.slots[k].running.is_none() && self.slots[k].lease.is_some() {
                // Not started: its lease given back with why (the lead's history says it).
                let why =
                    waiting.get(said..).and_then(|w| w.last()).map_or("it couldn't start here", |w| w.why.as_str());
                let why = why.to_string();
                self.end_lease(k, Outcome::Interrupted, &[], &why);
            }
        }
    }

    /// A job this Mac's coordinator granted it (`lead_grants`), put in a slot of its own (the first
    /// free, or a new one): the lead's own planned job by its id (`Grant::job`), else an offered
    /// step's targets as a member runs them (`step_cmd`). Its lease in the slot (`lease_own`); None
    /// when it can't be (the lease ended).
    fn own_grant(
        &mut self,
        g: crate::coord::Grant,
        co: &crate::coord::Coordinator,
        jobs: &[JobSpec],
        root: &store::nasfs::NasRoot,
        waiting: &mut Vec<Waiting>,
    ) -> Option<(usize, JobSpec)> {
        let spec = match (&g.job, &g.work) {
            (Some(id), _) => jobs.iter().find(|j| j.id == *id).cloned(),
            (None, crate::coord::Granted::Job { step, targets, pass }) => {
                let n = targets.len();
                Some(JobSpec {
                    id: format!("{step} {}", targets.first().map(|t| t.0.as_str()).unwrap_or("")),
                    what: steps::said(step, n),
                    cmd: self.step_cmd(root, step, targets, pass),
                    needs: Needs { nas: true },
                    restart_after_sleep: true,
                    record: Some(build::Work { step: step.clone(), targets: targets.clone() }),
                })
            }
            _ => None,
        };
        let (Some(spec), Some(k)) = (spec, self.free_slot()) else {
            co.finish(g.lease, &[]);
            return None;
        };
        if !self.lease_own(k, g.lease, co, &spec, waiting) {
            return None;
        }
        self.slots[k].granted = Some(g.threads).filter(|&t| t > 0);
        self.slots[k].reserve_mb = Some(g.disk_mb).filter(|&d| d > 0);
        Some((k, spec))
    }

    /// Lease `n` of this Mac's coordinator held for `spec` in slot `k`: in the pool, with the folder
    /// its job saves into (`<term>-<n>`, handed to the journal as it ends: docs/pool.md §7.3), its
    /// step and targets kept there; out of it, the lease alone. False (the lease ended, why said)
    /// when its folder can't be made.
    fn lease_own(
        &mut self,
        k: usize,
        n: u64,
        co: &crate::coord::Coordinator,
        spec: &JobSpec,
        waiting: &mut Vec<Waiting>,
    ) -> bool {
        if self.pool.is_none() {
            self.slots[k].lease = Some(Held::Own(n));
            return true;
        }
        let w = held_work(spec);
        let lease = crate::pool::journal::LeaseId { term: co.lease_term(n), n };
        let dir = pool::job_dir(&self.o.home, lease);
        let kept = pool::JobKept { step: w.step.clone(), targets: w.targets.clone(), lease };
        if let Err(e) = std::fs::create_dir_all(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|()| store::whole::write(&dir.join("work.json"), &serde_json::to_vec(&kept)?))
        {
            co.finish(n, &[]);
            waiting.push(Waiting { step: Some(w.step), what: spec.what.clone(), why: format!("its folder: {e:#}") });
            return false;
        }
        self.slots[k].lease = Some(Held::Pooled { id: n, term: lease.term, dir, own: true });
        true
    }

    /// The command of a job of an offered `step` over `targets` (crate::agent::every::offered), from
    /// `pass`, the lead's and a member's alike: `scenic-build` with the step's arguments, the z6 tiles
    /// made again as they are expected the same (by the build's records as read; records that can't
    /// be read now expect nothing).
    fn step_cmd(
        &self,
        root: &store::nasfs::NasRoot,
        step: &str,
        targets: &[(String, String)],
        pass: &str,
    ) -> Vec<String> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let mut cmd = vec![
            s(&self.o.bin.join("scenic-build")),
            step.to_string(),
            "--root".into(),
            jobs::root_arg(root),
            "--scratch".into(),
            s(&self.o.home.join("scratch").join(step)),
        ];
        cmd.extend(targets.iter().map(|t| t.0.clone()).filter(|t| every::names_target(t) && *t != step));
        cmd.extend(self.step_args(step, pass));
        if matches!(step, "trees" | "terrain-hi" | "slope-hi") {
            // (Under the NAS's deadline: none read, nothing expected the same.)
            let keys = records_deadlined(root).keys;
            let same = expect_same(
                &build::Work { step: step.to_string(), targets: targets.to_vec() },
                &self.as_read(root, keys),
            );
            if !same.is_empty() {
                cmd.extend(["--expect-same".to_string(), same.join(",")]);
            }
        }
        cmd
    }

    /// Slot `k`'s job ended (or stopped): its lease ended with it. This Mac's own: done when it
    /// recorded its targets, and what its z6 tiles cost learned; a helper's: its result in its outbox
    /// folder, to send.
    fn end_lease(&mut self, k: usize, outcome: Outcome, done: &[(String, String)], note: &str) {
        // (A job ended: what a helper's caches can free is counted again before it next asks.)
        self.cheap = None;
        let pid = self.slots[k].running.as_ref().map(|r| r.pgid as u32);
        // What its targets took at least, as the guard's sampler saw them (crate::agent::memguard),
        // but those its run measured: learned whatever the guard does.
        // (The lead's client, made again when it isn't there, as `beat` makes it: a lead a moment
        // ago has none, its job's end told to the new lead all the same.)
        if self.client.is_none()
            && self.coord.is_none()
            && matches!(self.slots[k].lease, Some(Held::Pooled { .. } | Held::Leased { .. }))
        {
            if let Some(r) = self.last_root.clone() {
                self.client(&r, &mut Vec::new());
            }
        }
        let watch = self.sampler.unwatch(k).unwrap_or_default();
        // (None from a job stopped for yielding: what it held says nothing of its targets.)
        let no_floors = self.slots[k].yield_stopped;
        // (Given back far over what this member spared, alone: its target under way's floor is
        // what the job held, learned alone, so the lead places it on a Mac with room.)
        let over = self.slots[k].over_floor.take();
        let floors_of = |costs: &[(String, crate::coord::Cost)]| {
            if no_floors {
                return Vec::new();
            }
            let mut f = watch.floors(&costs.iter().map(|(u, _)| u.clone()).collect::<Vec<_>>());
            if let Some((key, mb)) = &over {
                f.retain(|(k, _)| k != key);
                let v = crate::coord::cost_version(&watch.step);
                f.push((key.clone(), memguard::Floor { mb: *mb, alone: true, v }));
            }
            f
        };
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
        // (Its reservation released with its lease: handed off or given back.)
        self.slots[k].reserve_mb = None;
        self.slots[k].spare_mb = None;
        match self.slots[k].lease.take() {
            Some(Held::Own(id)) => {
                if let Some(c) = &self.coord {
                    // (Failed, or stopped by the memory guard: its targets not done kept from this
                    // Mac as a member's are, an hour and doubling, `finish_failed`.)
                    if outcome == Outcome::Failed || guard_stopped {
                        c.finish_failed(id, done);
                    } else {
                        c.finish(id, done);
                    }
                    if let Some(p) = pid {
                        c.close_tasks(p);
                    }
                    let costs = self.costs_path(k);
                    let measured = read_costs(&costs);
                    c.add_floors(&floors_of(&measured));
                    c.add_disks(&disks);
                    job_run(c, &job_step, outcome, done, job_disk);
                    c.add_costs_by(&measured, &self.worker_of(k));
                    std::fs::remove_file(costs).ok();
                }
            }
            Some(Held::Pooled { id, dir, own, .. }) => {
                // Its hand-off (its saves, the targets it finished, whatever stopped it) to the
                // journal, by the pool's next step; its folder kept until a saved state holds it.
                let kept: Option<pool::JobKept> =
                    std::fs::read(dir.join("work.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let costs = read_costs(&if own { self.costs_path(k) } else { dir.join("costs.jsonl") });
                // (Out of the pool, its member's lock held by another process a while: this Mac's
                // member all the same, its hand-off read as the pool's would. Without it the job
                // would go to the lead as failed, with no reason, and be built again.)
                let me = self.member_id.clone().or_else(|| crate::pool::member_id(&self.o.home).ok());
                let entry = match (&kept, me.as_deref()) {
                    (Some(w), Some(me)) => match pool::entry_of(&dir, me, w.lease, &w.step, done, now_s()) {
                        Ok(e) => e,
                        Err(e) => {
                            eprintln!(
                                "agent: {}'s hand-off can't be read ({e:#}); its work is done again",
                                dir.display()
                            );
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
                        if outcome == Outcome::Failed || guard_stopped {
                            c.finish_failed(id, done);
                        } else {
                            c.finish(id, done);
                        }
                        if let Some(p) = pid {
                            c.close_tasks(p);
                        }
                        c.add_floors(&floors_of(&costs));
                        c.add_disks(&disks);
                        job_run(c, &job_step, outcome, done, job_disk);
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
                    // Out of the pool (its member's lock held by another process), nothing goes to
                    // the pool's journal from this process: the hand-off goes to the lead to journal
                    // itself (`journaled: false`; its coordinator journals it whole before the
                    // lease ends, or answers an error and it's told again), and the folder goes once
                    // the lead answers (`told`). Saying "journaled" here would end the lease with
                    // nothing journaled yet, the work left to the next process's `left_jobs`, which
                    // rebuilds it from what the job noted done and removes a folder it can't read:
                    // the lead would then hold the targets done with no entry ever merged. Told
                    // twice (the lead's answer lost, the folder then handed off as left), the same
                    // saves merge twice, which changes nothing; never told (the process ends
                    // first), the folder goes to the journal through the next process as before.
                    // (A give-back or failure out of the pool carries no hand-off: its folder stays
                    // for the next process, which hands what it finished off as left.)
                    let pooled = self.pool.is_some();
                    let tell = match entry.as_ref() {
                        Some(e) if e.handoff.done.is_some() => Tell::Done(Box::new(crate::coord::Done {
                            lease: id,
                            handoff: Some(e.handoff.clone()),
                            costs,
                            floors,
                            disks,
                            job_disk_mb: job_disk,
                            failed,
                            journaled: pooled,
                            timings,
                            ..Default::default()
                        })),
                        e if no_targets && outcome == Outcome::Done && !guard_stopped => {
                            Tell::Done(Box::new(crate::coord::Done {
                                lease: id,
                                handoff: e.map(|e| e.handoff.clone()),
                                costs,
                                floors,
                                disks,
                                job_disk_mb: job_disk,
                                journaled: pooled,
                                timings,
                                ..Default::default()
                            }))
                        }
                        _ if matches!(outcome, Outcome::Paused | Outcome::Interrupted) && !guard_stopped => {
                            Tell::GiveBack { id, why: note.to_string(), floors, disks, job_disk }
                        }
                        _ => Tell::Fail { id, why: fail_why(outcome, note, entry.is_some()), floors, disks, job_disk },
                    };
                    if !pooled && matches!(tell, Tell::Done(_)) {
                        self.unjournaled.insert(id, dir.clone());
                    }
                    if self.coord.is_some() {
                        eprintln!("agent: lease {id} ended as this Mac finds its term lost: told to the lead once it works as a member");
                        self.owed.push(tell);
                    } else {
                        self.tell_or_owe(tell);
                    }
                }
                // (Handed to the pool at the round's pool step, which follows the jobs' ends.)
                let held = match (entry, self.pool.is_some()) {
                    (Some(e), true) => Some((e, Some(dir))),
                    // (Nothing to hand off: the folder goes; no pool any more, it waits for the next
                    // process, which hands it off.)
                    (None, true) => {
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
                let done =
                    step.filter(|_| outcome != Outcome::Interrupted && !done.is_empty()).map(|s| (s, done.to_vec()));
                let failed = outcome == Outcome::Failed && done.is_some();
                // A task's: what `scenic run-task` wrote (its outputs are with the coordinator already).
                let task: Option<serde_json::Value> =
                    std::fs::read(dir.join("task.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let ok = match &task {
                    Some(t) => outcome == Outcome::Done && t["ok"] == true,
                    None => done.is_some(),
                };
                // (One the memory guard stopped isn't interrupted: kept from this Mac for a while.)
                let interrupted = !ok && matches!(outcome, Outcome::Paused | Outcome::Interrupted) && !guard_stopped;
                // (What its targets held at least, for the lead's floors when it isn't done.)
                let floors = if ok { Vec::new() } else { floors_of(&read_costs(&dir.join("costs.jsonl"))) };
                let r = serde_json::json!({ "ok": ok, "done": done, "failed": failed, "interrupted": interrupted, "task": task.and_then(|t| t.get("task").cloned()), "error": note.chars().take(3000).collect::<String>(), "floors": floors });
                if let Err(e) = store::whole::write(&dir.join("result.json"), r.to_string().as_bytes()) {
                    eprintln!("agent: writing a job's result for the coordinator: {e:#}");
                }
            }
            None => {}
        }
    }

    /// Whether this process leads a term a later one has followed, its step not knowing yet (it
    /// froze, and a later term was made meanwhile: the owner's move of the lead): a stat of the next term's file, as the
    /// driver's step makes. False when it leads none, or the NAS can't say now.
    fn lost_term(&self) -> bool {
        // (A tick holds the run: it can't say now.)
        // (The pool's run, the pool's part until #878: a tick holding it, it can't say now.)
        let Some(Some(run)) = self.pool.as_ref().map(pool::try_lock) else { return false };
        let Some(e) = run.side.driver().leads() else { return false };
        crate::pool::term::next(run.side.nas(), e).unwrap_or(false)
    }

    /// Tells the lead a job's end, or owes it (not told: the lead didn't answer, its round failed
    /// already, or there's no client): told again each loop until the lead answers (`tell_owed`), never
    /// lost to a fast fail.
    fn tell_or_owe(&mut self, tell: Tell) {
        match self.client.as_ref().and_then(|c| tell.tell(c)) {
            Some(refused) => self.told(tell.lease(), refused),
            None => self.owed.push(tell),
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
    /// lead's client: each kept until the lead answers (its lease no longer lapses meanwhile: the
    /// member's beat lists it, `beat_lead`).
    fn tell_owed(&mut self, root: &store::nasfs::NasRoot) {
        if self.owed.is_empty() || self.coord.is_some() {
            return;
        }
        let owed = std::mem::take(&mut self.owed);
        let Some(c) = self.client(root, &mut Vec::new()) else {
            self.owed = owed;
            return;
        };
        let mut told = Vec::new();
        let mut left = Vec::new();
        for t in owed {
            match t.tell(c) {
                Some(refused) => told.push((t.lease(), refused)),
                None => left.push(t),
            }
        }
        self.owed = left;
        for (id, refused) in told {
            self.told(id, refused);
        }
    }

    /// The member's beat to the lead each `held::BEAT_EVERY` (docs/coordination.md §6.1;
    /// crate::coord::held): the lead's leases it holds, its slots' jobs' and those whose ends it
    /// owes; a lease it leaves out the lead releases. Its slots whose leases the lead says ended
    /// beat at once (refused: their jobs stop). An older lead answers 404: nothing changes.
    fn beat_lead(&mut self) {
        use crate::coord::held;
        if self.coord.is_some() || self.lead_beaten.is_some_and(|t| t.elapsed() < held::BEAT_EVERY) {
            return;
        }
        let Some(c) = self.client.as_ref() else { return };
        self.lead_beaten = Some(Instant::now());
        let held = self.held_of_lead();
        let b = held::Beat { member: self.host.clone(), speaks: vec![held::SPEAKS.to_string()], held };
        match c.pool_beat(&b) {
            Ok(Some(a)) => {
                for s in self.slots.iter_mut() {
                    if matches!(&s.lease, Some(Held::Leased { lease: id, .. } | Held::Pooled { id, own: false, .. }) if a.released.contains(id))
                    {
                        s.beaten = None;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("agent: its beat to the lead: {e:#}"),
        }
    }

    /// The lead's leases this member holds: its slots' jobs', and those whose ends it owes.
    fn held_of_lead(&self) -> BTreeSet<u64> {
        let slots = self.slots.iter().filter_map(|s| match &s.lease {
            Some(Held::Leased { lease, .. } | Held::Pooled { id: lease, own: false, .. }) => Some(*lease),
            _ => None,
        });
        slots.chain(self.owed.iter().map(Tell::lease)).collect()
    }

    /// Lease `id`'s end answered by the lead: the folder of a job that ended out of the pool, its
    /// hand-off given to the lead to journal (`unjournaled`), removed, so the next process doesn't
    /// hand it off again (`pool::left_jobs`); kept when the lead `refused` it (its checks are a
    /// hand-off's, stricter than a journal entry's), for the next process to hand off as a job it
    /// left.
    fn told(&mut self, id: u64, refused: bool) {
        if let Some(dir) = self.unjournaled.remove(&id) {
            if !refused {
                std::fs::remove_dir_all(&dir).ok();
            }
        }
    }

    /// Beats for slot `k`'s job's lease (each minute it isn't paused); false when it's lost: it
    /// lapsed and its work went to another worker. (One of this Mac's running jobs never lapses
    /// while it leads, `keep_own`, docs/pool.md §7.6; one gone all the same, as across a move of the lead,
    /// is held again when no one took its work meanwhile.)
    fn beat(&mut self, k: usize, root: Option<&store::nasfs::NasRoot>) -> bool {
        self.slots[k].beaten = Some(Instant::now());
        // (Its progress as the status last read it: what it said, out of sight or not.)
        let progress = self.slots[k]
            .running
            .as_ref()
            .and_then(|r| r.said.clone().or_else(|| jobs::progress(&r.log).map(|(d, t, u)| (d, t, u, None))))
            .map(|(d, t, u, _)| format!("{}/{t:.0} {u}", (d * 10.0).floor() / 10.0));
        let live = self.job_live(k);
        match self.slots[k].lease.clone() {
            // (Its lease through the lead wherever it is now: this process's coordinator while it
            // leads, the lead's by HTTP while it's a member, whoever granted it: docs/pool.md §7.6.)
            Some(Held::Own(id) | Held::Pooled { id, .. }) if self.coord.is_some() => {
                let Some(c) = &self.coord else { return true };
                if c.renew_live(id, progress, live) {
                    return true;
                }
                let work = self.slots[k].running.as_ref().map(|r| held_work(&r.spec));
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
                match client.map(|c| c.beat_told(lease, progress.as_deref(), live, Some(k as u32))) {
                    // (The build's pause as the lead says: this job stops with it, or goes on;
                    // unless this Mac's own ask isn't with it yet. The owner's cancel: it stops at
                    // the next loop, `tend_cancels`.)
                    Some(Ok((alive, pause, cancel))) => {
                        if !self.pause_local {
                            self.know_pause(pause);
                        }
                        if cancel.is_some() && self.slots[k].cancel.is_none() {
                            self.slots[k].cancel = cancel;
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
        self.o.home.join(format!("job{}.json", slot_suffix(k)))
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
        self.o.home.join(format!("timings-run{}.json", slot_suffix(k)))
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
            Some(_) => handed
                .as_deref()
                .and_then(|p| p.parent())
                .and_then(read_timings)
                .map(|r| crate::timings::RunRec { host: self.worker_of(k), ..r }),
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

    /// Where slot `k`'s job notes what its z6 tiles cost (`SCENIC_COSTS`).
    fn costs_path(&self, k: usize) -> PathBuf {
        self.o.home.join(format!("costs{}.jsonl", slot_suffix(k)))
    }

    /// Where slot `k`'s job's accessors keep the bytes they've added to the caches
    /// (store::cachefile::WRITTEN_ENV), for the disk guard's sampler.
    fn written_path(&self, k: usize) -> PathBuf {
        self.o.home.join(format!("written{}", slot_suffix(k)))
    }

    /// The folder slot `k`'s job's processes count their NAS traffic in (store::nasio::ENV), on
    /// this Mac's disk, for the live store (crate::agent::live).
    fn nas_counts_path(&self, k: usize) -> PathBuf {
        self.o.home.join(format!("nascounts{}", slot_suffix(k)))
    }

    /// Where slot `k`'s job keeps the file that says it waits on a worker (crate::offload's
    /// WAITING_ENV), while it does.
    fn waiting_path(&self, k: usize) -> PathBuf {
        self.o.home.join(format!("waiting{}", slot_suffix(k)))
    }

    /// What slot `k`'s job waits on, when it waits on a worker holding its z6 tile's tail and nothing
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
        Some(format!(
            "waiting on {who} for its unit's tail ({} min so far): its room free for a job beside it",
            now_s().saturating_sub(n.since) / 60
        ))
    }

    /// Slot `k`'s worker in the build's history and the forecast: this Mac, or its job's slot
    /// (`slot_worker`).
    fn worker_of(&self, k: usize) -> String {
        slot_worker(&self.host, k)
    }

    /// The folder slot `k`'s jobs keep their scratch in: `scratch/` the first's, `scratch-<k+1>/`
    /// another's (`start` moves a job's `--scratch` there).
    fn slot_scratch(&self, k: usize) -> PathBuf {
        self.o.home.join(format!("scratch{}", slot_suffix(k)))
    }

    /// The slot a member's next granted job goes in: the first free (no job, no lease), else a new
    /// one past the last (up to `MAX_SLOTS`; None past it).
    fn free_slot(&mut self) -> Option<usize> {
        if let Some(k) = self.slots.iter().position(|s| s.running.is_none() && s.lease.is_none()) {
            return Some(k);
        }
        (self.slots.len() < MAX_SLOTS).then(|| {
            self.slots.push(Slot::default());
            self.slots.len() - 1
        })
    }

    /// The slots past `SLOTS` that hold nothing (no job, no lease), dropped from the end: a member's
    /// packed jobs' slots, once they ended.
    fn trim_slots(&mut self) {
        while self.slots.len() > SLOTS && self.slots.last().is_some_and(|s| s.running.is_none() && s.lease.is_none()) {
            let k = self.slots.len() - 1;
            self.sampler.unwatch(k);
            self.slots.pop();
        }
    }

    /// The most jobs this Mac runs at once (`job_slots` of its cores).
    fn job_slots(&self) -> usize {
        lead_slots(self.is_member(), self.ramp_held, &self.mem.ramp)
    }

    /// The lead's slots' ramp: they start at `RAMP_START` and grow by one only after a clean stretch
    /// of `RAMP_CLEAN_S`: no memory guard stop (`guard_stop_as` restarts the stretch), no stall
    /// (the pool's step taken for hung). The count reached is kept in the lead's state
    /// (`state.json`), up to its cores' `job_slots`.
    ///
    /// While the NAS's limits aren't known (crate::coord::Coordinator::nas_known: before the
    /// coordinator's first probe lands, each start) the slots are held at `RAMP_START`, but the
    /// stretch goes on: a hold isn't trouble (task #433: every restart into a published app, a dozen
    /// a day, began the hold a moment before the probe landed and started the six hours again, so
    /// the lead never grew past two slots). The probes failing (`Coordinator::nas_failing`: two or
    /// more in a row; one alone, a hiccup every few hours, is neither a hold nor trouble) holds
    /// them too and is trouble. Not the pool's leased jobs' NAS weight past the whole
    /// (`Coordinator::nas_over`): the budget grants that by its own rules (the lead's own jobs alone,
    /// an aged job weighing the whole, §7.7) and holds the lead's own jobs at grant whatever the
    /// slots, so it says nothing about them.
    fn tend_ramp(&mut self) {
        if self.is_member() || self.o.dry_run {
            return;
        }
        let Some(c) = self.coord.as_ref() else { return };
        let now = now_s();
        let most = job_slots(mac_cores() as usize);
        let failing = c.nas_failing();
        let held = !c.nas_known() || failing;
        let trouble = if failing {
            Some("the NAS probes failing (two or more in a row)")
        } else if self.pool_hung.is_some() {
            Some("the pool's step stalled")
        } else {
            None
        };
        let holding = held && !self.ramp_held;
        self.ramp_held = held;
        let running = self.running_jobs();
        // (Trouble entered: the stretch reset, said and kept once; while it lasts, nothing more;
        // as it ends, the stretch starts then, kept.)
        let entering = trouble.is_some() && self.ramp_trouble.is_none();
        let leaving = trouble.is_none() && self.ramp_trouble.is_some();
        self.ramp_trouble = trouble.map(str::to_string);
        let r = &mut self.mem.ramp;
        let was = *r;
        r.slots = r.slots.max(RAMP_START);
        // (The most jobs it ran at once in the stretch: idle time doesn't count toward the next.)
        r.peak = r.peak.max(running);
        if holding {
            eprintln!("agent: the lead's slots held at {RAMP_START} ({RAMP_HELD_WHY}); its clean stretch goes on");
        }
        if let Some(why) = trouble {
            if entering {
                eprintln!("agent: the lead's slots stay at {} ({why}: its clean stretch starts again)", r.slots);
                (r.clean_since, r.peak) = (now, running);
            } else {
                // (Lasting: kept as it is, the peak not counted meanwhile.)
                r.peak = was.peak;
            }
        } else if leaving || r.clean_since == 0 {
            (r.clean_since, r.peak) = (now, running);
        } else if held {
            // (Held: the slots can't be filled, so the stretch runs on without growing.)
        } else if r.slots < most && r.peak >= r.slots && now.saturating_sub(r.clean_since) >= RAMP_CLEAN_S {
            r.slots += 1;
            (r.clean_since, r.peak) = (now, running);
            eprintln!("agent: the lead's slots grow to {} after {} h clean", r.slots, RAMP_CLEAN_S / 3600);
        }
        // (Every reset of the stretch, and every step up, kept; the peak alone as it changes.)
        if *r != was {
            self.save();
        }
    }

    /// Whether a grant asks for more work now (the lead's of its own coordinator, a member's of its
    /// lead's), whatever the plan is doing: a slot free (`job_slots`), and, with a job running,
    /// nothing holding more back: the build not paused, no job stopping at its safe point (a pause,
    /// an update, the memory guard: `drain_since`, `mem_drain`), none the memory guard drained
    /// another for (`guard_held`) or a guard froze.
    fn takes_more(&self) -> bool {
        let running = self.running_jobs();
        if running >= self.job_slots() {
            return false;
        }
        running == 0
            || (self.pause.is_none()
                && (0..self.slots.len()).all(|k| {
                    let s = &self.slots[k];
                    s.running.is_none()
                        || (s.drain_since.is_none()
                            && s.mem_drain.is_none()
                            && self.guard_held(k).is_none()
                            && !self.sampler.frozen(k)
                            && !self.sampler.disk_frozen(k))
                }))
    }

    /// The jobs running in this agent's slots.
    fn running_jobs(&self) -> usize {
        self.slots.iter().filter(|s| s.running.is_some()).count()
    }

    /// Each slot's rank for the memory guard (memguard::decide_by: the highest drains first): the
    /// newest job highest, then the least progressed (by its progress's fraction as last said).
    fn guard_ranks(&self) -> Vec<u64> {
        self.slots
            .iter()
            .map(|s| {
                s.running.as_ref().map_or(0, |r| {
                    let frac =
                        r.said.as_ref().map_or(0.0, |p| if p.1 > 0.0 { (p.0 / p.1).clamp(0.0, 1.0) } else { 0.0 });
                    // (Its start's minute, so jobs started within one count as equally new; then
                    // its progress left, in thousandths.)
                    (r.started / 60).saturating_mul(1000) + ((1.0 - frac) * 999.0) as u64
                })
            })
            .collect()
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

    /// What the caches are freed toward (0: nothing): the target and the jobs' reservations past
    /// it (`reserved`), or, while a job waits for it (`floor_short`), the room it needs past it,
    /// when more.
    fn goal(&self) -> u64 {
        let target = self.floor();
        match self.floor_short {
            Some((t, n, _)) if target > 0 && t == target => target.max(n),
            // (With a target, the jobs' reservations past it: room made for each, `reserved`.)
            _ if target > 0 => target.saturating_add(self.reserved(None)),
            _ => target,
        }
    }

    /// The owner's disk room target in a waiting's words: ", the disk room target 100.0 GB kept
    /// free past it"; none without one.
    fn floor_words(&self) -> String {
        floor_words(self.floor())
    }

    /// Why a job that needs `need` free waits for the owner's disk room target.
    fn floor_why(&self, need: u64) -> String {
        // (No target: by measure, the job's own room alone.)
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
        let view = self.plan.view.get();
        for w in view.queued.iter().cloned().chain(running) {
            h.add(&w.step, &w.targets.into_iter().map(|t| t.0).collect::<Vec<_>>());
        }
        for r in self.slots.iter().filter_map(|s| s.running.as_ref()) {
            let j = JobId::parse(&r.spec.id);
            if let Some(t) = j.rest.filter(|_| steps::current(j.step).starts_with("terrain-")) {
                h.add(j.step, &[t.to_string()]);
            }
            if let Some(st) = step_of(&r.spec.id) {
                h.add(&st, &[]);
            }
        }
        // (And what the forecast has this Mac read next, at its time: task #843.)
        if let Some((_, u)) = view
            .upcoming
            .as_ref()
            .or(self.upcoming.borrow().as_ref())
            .filter(|(at, _)| now_s().saturating_sub(*at) < UPCOMING_STALE_S)
        {
            h.merge(u);
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

    /// The NAS root, when mounted and answering, as **nas** last saw it (crate::agent::nas: a mount
    /// tried every five minutes otherwise, each look under its deadline, never the loop's wait).
    /// Its reads are never retried (`NasRoot::no_retry`): the loop plans, gates, steps the pool and
    /// feeds the coordinator with it, and a read riding out an outage for minutes would stall them
    /// all (25 Oct); a failed read fails this beat, and the next reads again. A thread the loop
    /// starts and never waits on takes `long_retry()`.
    fn root(&mut self) -> Option<NasRoot> {
        self.nas.root()
    }

    /// Runs until stopped (SIGTERM, SIGINT), or for one loop with `once`.
    pub fn run(&mut self) -> Result<()> {
        // (The handler only stores to an atomic.)
        crate::sys::on_terminate(on_signal);
        // The memory guard's sampler, on a thread of its own (crate::agent::memguard), its latest
        // sample written for the status whatever this loop waits on.
        self.sampler.write_to(self.o.home.join(memguard::LIVE));
        self.sampler.spawn();
        // This Mac's measures over time (crate::agent::live), on threads of their own.
        if self._lock.is_some() && !self.o.dry_run && self.mac == cond::Mac::Real {
            let store = live::Store::new(Some(self.o.home.join(live::FILE)), &self.host);
            live::spawn(store.clone(), self.o.home.clone(), self.sampler.clone());
            self.live = Some(store);
        }
        // The pool's own thread: its driver stepped while this loop is busy (`pool::Ticker`).
        self.ticker = self.pool.clone().map(pool::Ticker::start);
        // The owner's asks taken up at once (crate::agent::asks: control), and the heartbeat on its
        // own timer (crate::agent::beat), on threads of their own whatever the loop waits on.
        if self._lock.is_some() && !self.o.dry_run && !self.o.once {
            self.spawn_jobs_part();
            self.control.spawn();
            self.beat.spawn();
            self.nas.spawn();
            // **plan** on its thread: it plans on changes, the round granting from its last plan.
            self.plan.spawn();
        }
        if self._lock.is_some() {
            // (What they finished went to the journal with their hand-offs, as they saved.)
            // (A member's packed jobs' slots too: every record an earlier agent may have left.)
            // (But those this agent took over, `adopt_jobs`.)
            for k in 0..MAX_SLOTS {
                if self.slots.get(k).is_some_and(|s| s.running.is_some()) {
                    continue;
                }
                let orphan = jobs::stop_orphan(&self.record_path(k));
                self.orphans.extend(orphan.left);
            }
        }
        loop {
            let quick = self.round()?;
            nasread::phase("between rounds");
            // (Each job's slot with its record as it changes: a restart or a crash leaves what the
            // next agent takes it over by.)
            self.save_hands();
            if self.o.once || STOP.load(Ordering::SeqCst) {
                break;
            }
            // The app quits (crate::control::Quit): "now", its jobs stopped and given back at once;
            // the loop ends once none runs and the lead is handed away (`quit_due`).
            if let Some(q) = self.quit_ask() {
                if q.mode == crate::control::QuitMode::Now {
                    self.stop_jobs(&format!("stopped: Scenic quit ({})", q.by));
                }
                if self.quit_due() {
                    eprintln!("agent: Scenic quit ({}); stopping", q.by);
                    self.quit_done = true;
                    break;
                }
            }
            // A newer app is in place and every slot is free (its job stopped at a safe point,
            // `tend`) or its drain's wait is over: exit (a job still running is left running, and
            // the new agent takes it over, `hand_over_jobs`), and the launcher starts the new one.
            if self.update_due() {
                eprintln!("agent: a newer app is installed; restarting into it");
                break;
            }
            // The pool's switch changed, or this process's part in it: restarting into it.
            if let Some(why) = self.restart_now() {
                eprintln!("agent: {why}; restarting");
                break;
            }
            // (A job ended this round: its slot filled at once. Else 20 s, cut short by what control
            // took up (a hold, a quit, a setting), by **jobs**' thread (a job's end, a cancel it
            // took: crate::agent::slots), and by the regions' states in: the loop takes them up
            // at once, as it did when it counted them itself.)
            if quick {
                continue;
            }
            nasread::phase(nasread::WAIT);
            let until = Instant::now() + Duration::from_secs(20); // (bound ok: the wait between rounds, cut short by every part)
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                if STOP.load(Ordering::SeqCst) {
                    break;
                }
                // (bound ok: the STOP flag looked at each second)
                if self.control.woken.recv_timeout(Duration::from_secs(1).min(left)).is_ok() {
                    while self.control.woken.try_recv().is_ok() {}
                    break;
                }
            }
        }
        // (The live store kept for the next agent.)
        if let Some(l) = &self.live {
            l.save();
        }
        // The jobs running left running for the next agent, first (launchd may kill this process
        // within seconds of the SIGTERM that ended the loop): it takes them over (`adopt_jobs`).
        // Not when the app quits (its jobs stopped or ended first, `quit_due`), nor a single loop's.
        if !self.quit_done && !self.o.once {
            self.hand_over_jobs();
        }
        // The pool's thread stopped first: nothing steps the driver after this loop (its tick under
        // way finished, waited for a minute at most: a NAS that hangs may not return).
        if let Some(t) = self.ticker.take() {
            t.stop(Duration::from_secs(60));
        }
        // The plan's thread stopped too (a plan under way finished first, a minute at most).
        self.plan.stop(Duration::from_secs(60));
        // (Hand-offs held for a pool step that never came, `held_entries`, are lost with this
        // process: their folders stay, and the next one hands them off, `take_left`.)
        // A clear or a freeing under way: asked to end at its next file (also when a newer app takes
        // over), and waited for a minute at most: a call to a NAS that hangs may not return, and
        // the launcher waits for the agent. Left mid-way, nothing's lost: copies are written by
        // temporary names, a raw tile goes only once its archive is named in the index (one put up
        // and never named goes a day later), and a clear's ask stays aside (`room::CLEAR_TAKEN`).
        // The next agent frees again as needed, or takes the ask up again.
        if let Some(t) = self.caches_task.take() {
            STOP.store(true, Ordering::SeqCst);
            let waited = Instant::now();
            while !t.thread.is_finished() && waited.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(200));
            }
            if !t.thread.is_finished() {
                eprintln!(
                    "agent: this Mac's caches' {} hasn't ended in a minute; left to the next agent",
                    if t.ask.is_some() { "clear" } else { "freeing" }
                );
            }
        }
        let root = self.o.root.clone().or_else(|| self.root());
        self.stop_jobs("the agent stopped");
        // The coordinator's contact off the NAS: workers wait for the next agent.
        if let (Some(c), Some(root)) = (&self.coord, &root) {
            c.unpublish(root);
        }
        // (Quit as the app asked: the ask done, so the next start, the app opened again, builds.)
        if self.quit_done {
            crate::control::clear_quit(&self.o.home);
        }
        Ok(())
    }

    /// Whether the loop ended because the app quit (crate::control::Quit): the process exits with
    /// `QUIT_EXIT`, which its launcher takes as the end.
    pub fn quit_done(&self) -> bool {
        self.quit_done
    }

    /// Every job running here stopped (`why`): what it finished handed off, its lease given back,
    /// its scratch left on the disk.
    fn stop_jobs(&mut self, why: &str) {
        for k in 0..self.slots.len() {
            if let Some(r) = self.slots[k].running.as_mut() {
                eprintln!("agent: stopping {}", r.spec.id);
                r.stop(Duration::from_secs(30));
                self.stopped(k, why);
                self.slots[k].running = None;
                std::fs::remove_file(self.record_path(k)).ok();
            }
        }
    }

    /// The app versions (`<apps>/<version>/`) the running jobs' programs are in, other than this
    /// agent's: those of jobs taken over from an older app's agent (`Status::job_apps`).
    fn job_apps(&self) -> Vec<String> {
        let cmds = self.slots.iter().filter_map(|s| s.running.as_ref()).filter_map(|r| r.spec.cmd.first());
        job_apps_of(cmds.map(String::as_str), &self.o.bin, &self.app)
    }

    /// The app a job taken over (`adopt_jobs`) runs the programs of, when it isn't this agent's
    /// (`job_apps_of`): an older one's, its job started before the update.
    fn old_app_of(&self, r: &jobs::Running) -> Option<String> {
        let cmd = r.spec.cmd.first().filter(|_| r.adopted)?;
        job_apps_of(std::iter::once(cmd.as_str()), &self.o.bin, &self.app).into_iter().next()
    }

    /// Slot `k`'s state an agent after this one needs to take its job over (`Hand`).
    fn hand_of(&self, k: usize) -> Hand {
        let s = &self.slots[k];
        Hand {
            lease: s.lease.clone(),
            reserve_mb: s.reserve_mb,
            spare_mb: s.spare_mb,
            proxy_port: self.proxy.as_ref().map(|p| p.port()),
        }
    }

    /// Each running job's slot saved with its record (jobs::save_hand) when it changed since it
    /// was last: a restart, or a crash, at any moment leaves what the next agent needs to take the
    /// job over (`adopt_jobs`). Not a helper's leased job's (its outbox is sent by its agent,
    /// `send_outbox`): stopped at a restart, as before.
    fn save_hands(&mut self) {
        if self._lock.is_none() || self.o.dry_run {
            return;
        }
        for k in 0..self.slots.len() {
            let Some(r) = self.slots[k].running.as_ref() else {
                self.slots[k].hand_saved = None;
                continue;
            };
            if matches!(self.slots[k].lease, Some(Held::Leased { .. })) {
                continue;
            }
            let now = (self.hand_of(k), r.paused.clone());
            if self.slots[k].hand_saved.as_ref() == Some(&now) {
                continue;
            }
            if let Ok(v) = serde_json::to_value(&now.0) {
                jobs::save_hand(&self.record_path(k), r, v);
                self.slots[k].hand_saved = Some(now);
            }
        }
    }

    /// The jobs running here left running as this agent ends (a newer app, its launcher restarted,
    /// SIGTERM): each one's slot saved with its record, its lease kept, for the next agent to take
    /// it over (`adopt_jobs`). A helper's leased job, which can't be, stopped (`stop_jobs`).
    fn hand_over_jobs(&mut self) {
        self.save_hands();
        for k in 0..self.slots.len() {
            if self.slots[k].running.is_none() || matches!(self.slots[k].lease, Some(Held::Leased { .. })) {
                continue;
            }
            let hand = serde_json::to_value(self.hand_of(k)).unwrap_or_default();
            if let Some(r) = self.slots[k].running.take() {
                eprintln!("agent: leaving {} running for the next agent", r.spec.id);
                r.hand_over(&self.record_path(k), hand);
            }
            self.slots[k].lease = None;
        }
        self.stop_jobs("the agent stopped");
    }

    /// The jobs the agent before this one left running (`hand_over_jobs`, or a crash after their
    /// slots were saved, `save_hands`), taken over into their slots (jobs::adopt): their leases
    /// (the lead's own held again in its coordinator, under a new id if the one it had ended with
    /// that agent, as a beat does; a member's kept with its lead), their grants, their channels
    /// set to run (the drain that agent asked for is this one's to ask again), and watched by the
    /// memory guard. Those that can't be are stopped as before (`run`, jobs::stop_orphan).
    fn adopt_jobs(&mut self) {
        if self._lock.is_none() || self.o.dry_run {
            return;
        }
        for k in 0..MAX_SLOTS {
            let Some((r, v)) = jobs::adopt(&self.record_path(k)) else { continue };
            let Ok(hand) = serde_json::from_value::<Hand>(v) else { continue };
            while self.slots.len() <= k {
                self.slots.push(Slot::default());
            }
            let id = r.spec.id.clone();
            let w = held_work(&r.spec);
            let lease = match (hand.lease, &self.coord) {
                (Some(Held::Own(n)), Some(c)) => {
                    Some(Held::Own(if c.renew(n, None) { n } else { c.hold(&w.step, &w.targets).unwrap_or(n) }))
                }
                (Some(Held::Pooled { id: n, term, dir, own: true }), Some(c)) => {
                    let n = if c.renew(n, None) { n } else { c.hold(&w.step, &w.targets).unwrap_or(n) };
                    Some(Held::Pooled { id: n, term, dir, own: true })
                }
                (l, _) => l,
            };
            // (One of an older app runs that release's programs to its end: said, task #688.)
            let old = self.old_app_of(&r).map(|v| format!(", on app {v} (older than this agent's {})", self.app));
            eprintln!(
                "agent: took over {} (group {}), left running by the agent before this one{}{}",
                id,
                r.pgid,
                if k > 0 { format!(", in slot {}", k + 1) } else { String::new() },
                old.clone().unwrap_or_default()
            );
            // (Its channel "run" unless this Mac's build is paused, the pool's or this Mac's alone:
            // then "drain", so it passes no safe point before `tend` asks it again.)
            let paused = self.own_pause().is_some();
            std::fs::write(self.control_path(k), if paused { &b"drain"[..] } else { b"run" }).ok();
            if let Some(p) = hand.proxy_port {
                self.proxy_port = Some(p);
            }
            let pgid = r.pgid;
            let s = &mut self.slots[k];
            s.lease = lease;
            s.reserve_mb = hand.reserve_mb;
            s.spare_mb = hand.spare_mb;
            s.running = Some(r);
            // (One already ended, `pgid` 0, isn't watched: its end is seen at the next look.)
            if pgid > 0 {
                self.watch_job(k, pgid);
            }
            self.note(crate::coord::history::Event {
                worker: Some(self.worker_of(k)),
                step: Some(w.step.clone()).filter(|s| !s.is_empty()),
                targets: w.targets.iter().map(|t| t.0.clone()).collect(),
                note: format!("taken over from the agent before{}", old.unwrap_or_default()),
                ..crate::coord::history::Event::new("adopt")
            });
        }
    }

    /// The jobs taken over (`adopt_jobs`) still running stopped (`why`), as interrupted: what they
    /// finished handed off, their leases given back.
    fn stop_adopted(&mut self, why: &str) {
        for k in 0..self.slots.len() {
            if !self.slots[k].running.as_ref().is_some_and(|r| r.adopted) {
                continue;
            }
            if let Some(r) = self.slots[k].running.as_mut() {
                eprintln!("agent: stopping {}: {why}", r.spec.id);
                r.stop(Duration::from_secs(30));
            }
            self.stopped(k, &format!("stopped: {why}"));
            self.slots[k].running = None;
            std::fs::remove_file(self.record_path(k)).ok();
        }
    }

    /// The memory guard's sampler told of slot `k`'s job (its group `pgid`), as it starts or is
    /// taken over.
    fn watch_job(&mut self, k: usize, pgid: i32) {
        let Some(spec) = self.slots[k].running.as_ref().map(|r| r.spec.clone()) else { return };
        let (step, targets) =
            spec.record.as_ref().map_or((spec.id.split(' ').next().map(str::to_string), Vec::new()), |w| {
                (Some(w.step.clone()), w.targets.iter().map(|t| t.0.clone()).collect())
            });
        let own_key = match spec.record.as_ref() {
            None => Some(own_cost_key(&spec.id)),
            _ => None,
        };
        let scratch =
            spec.cmd.iter().position(|a| a == "--scratch").and_then(|i| spec.cmd.get(i + 1)).map(PathBuf::from);
        self.sampler.watch(
            k,
            memguard::Watch {
                pgid,
                costs: self.slot_costs(k),
                done: self.done_path(k),
                step: step.unwrap_or_default(),
                targets,
                own_key,
                written: self.written_path(k),
                nas_counts: self.nas_counts_path(k),
                scratch,
                ..Default::default()
            },
        );
    }

    /// The app's ask to quit, if one waits (only the agent holding the lock acts on it).
    fn quit_ask(&self) -> Option<crate::control::Quit> {
        self._lock.as_ref()?;
        self.control.quit.get().as_ref().clone()
    }

    /// Why nothing new starts here: the app quits (`quit_ask`).
    fn quit_why(&self) -> Option<String> {
        let q = self.quit_ask()?;
        Some(match q.mode {
            crate::control::QuitMode::Finish => format!("Scenic quits once the job under way ends ({})", q.by),
            crate::control::QuitMode::Now => format!("Scenic quits ({})", q.by),
        })
    }

    /// Whether the quit can end the loop: no job runs here. (The lead quitting: the build waits for
    /// it, its members finishing their jobs; the lead is fixed, task #881.)
    fn quit_due(&self) -> bool {
        !self.slots.iter().any(|s| s.running.is_some())
    }

    /// This Mac's own jobs' pause: the build's, else this Mac's alone (crate::control::Hold: at
    /// their next safe point).
    fn own_pause(&self) -> Option<crate::control::Pause> {
        self.pause.clone().or_else(|| self.hold().held.as_ref().map(|h| h.as_pause()))
    }

    /// Why nothing new starts here: this Mac is paused alone.
    fn hold_why(&self) -> Option<String> {
        self.hold().held.as_ref().map(|h| h.why()).or_else(|| self.heal.drain_why())
    }

    /// Where this Mac's jobs report their NAS calls stuck busy (store::whole::STUCK_ENV).
    fn stuck_log(&self) -> PathBuf {
        self.o.home.join("nas-stuck.log")
    }

    /// Whether none of the agent's own threads that use the NAS is under way: the caches' trims,
    /// clears and freeing, the answers' seeding, the plan and its forecast, the coordinator's
    /// unpublishing.
    fn nas_threads_idle(&self) -> bool {
        let busy = |h: Option<bool>| h.unwrap_or(false);
        self.caches_task.is_none()
            && !busy(self.answers_seed.as_ref().map(|h| !h.is_finished()))
            && !self.plan.busy()
            && !self.plan.view.get().forecasting
            && !busy(self.unpublishing.as_ref().map(|h| !h.is_finished()))
    }

    /// The self-heal's round (crate::agent::heal): this process's and its jobs' stuck calls taken
    /// in; once drained, **nas** asked to unmount the share under the pool's lock (no tick
    /// mid-write) and mount it again at once.
    fn heal_step(&mut self) {
        let (mut new, seen) = store::whole::stuck_after(self.heal_seen);
        self.heal_seen = seen;
        new.extend(store::whole::take_reported(&self.stuck_log()));
        let idle = self.idle() && self.nas_threads_idle();
        if self.heal.tick(new, now_s(), idle) != heal::Act::Remount {
            return;
        }
        // (**nas** unmounts it under the pool's lock, a tick never writing as the share goes, and
        // mounts it again at once.)
        self.nas.ask(nas::Req::Remount(self.pool.clone()));
        self.heal.remounted(now_s());
    }

    /// This Mac's hold as control has it now.
    fn hold(&self) -> std::sync::Arc<crate::control::HoldState> {
        self.control.hold.get()
    }

    /// This Mac's hold set (`held`, as of `at`), by control, unless a later one is; whether it was.
    fn set_hold(&mut self, held: Option<crate::control::Hold>, at: u64) -> bool {
        self.control.set_hold(crate::control::HoldState { held, at })
    }

    /// What **jobs**' thread needs for other members' hold asks (crate::agent::slots::Members).
    fn members(&self) -> slots::Members {
        slots::Members {
            pool: self.pool.clone(),
            app: self.app.clone(),
            nas: self.nas.view.clone(),
            hold: self.control.hold.clone(),
        }
    }

    /// **jobs**' own thread started (crate::agent::slots): control tells it of cancels and other
    /// members' hold asks from now, and it wakes the loop for a job's end and a cancel it took.
    fn spawn_jobs_part(&mut self) {
        self.control.tell_jobs(self.jobs_part.teller());
        let (wake, members, setter) = (self.control.waker(), self.members(), self.control.setter());
        self.jobs_part.spawn(wake, members, setter);
    }

    /// This Mac's hold and the NAS (task #224, docs/pool.md §11): this Mac's file on the NAS read
    /// every `HOLD_READ_EVERY` (a later setting there, another Mac's ask, given control), and its
    /// own written there once it changed. An ask for another member is **jobs**' thread's
    /// (`slots::take_member_hold`, at once), or taken here while that thread doesn't run.
    fn hold_on_nas(&mut self, root: Option<&store::nasfs::NasRoot>) {
        use crate::control::HoldState;
        if self._lock.is_none() || self.o.dry_run {
            return;
        }
        if !self.jobs_part.spawned() {
            let (home, members) = (self.o.home.clone(), self.members());
            slots::take_member_hold(&home, &members, &mut |st| self.control.set_hold(st));
        }
        // (This member's id as the pool's last step said it: never waiting on the pool's lock.)
        let me = self.pool_seen.as_ref().map(|p| p.member.clone()).or_else(|| self.hold_me.clone());
        self.hold_me = me.clone();
        let (Some(r), Some(me)) = (root, me) else { return };
        let path = r.join(crate::control::hold_path(&me));
        if self.hold_read.is_none_or(|t| t.elapsed() >= HOLD_READ_EVERY) {
            self.hold_read = Some(Instant::now());
            if let Ok(h) = nasread::read_switch(&path).map(|t| serde_json::from_str::<HoldState>(&t)) {
                match h {
                    Ok(h) if h.at > self.hold().at => {
                        self.set_hold(h.held.clone(), h.at);
                        let (now, g) = self.control.hold.read();
                        self.hold_mirrored = (*now == h).then_some(g);
                    }
                    Ok(h) => {
                        // (The same: mirrored. Older there, and not this: this Mac's written back.)
                        let (now, g) = self.control.hold.read();
                        self.hold_mirrored = (*now == h).then_some(g);
                    }
                    _ => self.hold_mirrored = None,
                }
            }
        }
        let (hold, g) = self.control.hold.read();
        if self.hold_mirrored != Some(g) && hold.at > 0 {
            match write_hold(&path, &hold) {
                Ok(()) => self.hold_mirrored = Some(g),
                Err(e) => eprintln!("agent: this Mac's pause on the NAS: {e:#}"),
            }
        }
    }

    /// This member's run in the pool, locked (`pool::lock`): held only for what reads or changes it,
    /// the pool's own thread waiting meanwhile.
    /// Whether this process runs the lead's parts (its role as it started, `role`).
    fn leads(&self) -> bool {
        self.role == Role::Lead
    }

    /// Whether it runs a member's parts: a member's, or an unknown role's.
    fn is_member(&self) -> bool {
        !self.leads()
    }

    fn pooled(&self) -> Option<std::sync::MutexGuard<'_, pool::Run>> {
        self.pool.as_ref().map(pool::lock)
    }

    /// **jobs**' round (docs/agent-parts.md §4.5): the NAS's view, the owner's settings, the running
    /// jobs tended and guarded, the pool's step (the pool's part, until #878), grants from the last
    /// plan and their starts, the caches, and the status published for **beat**. It never plans
    /// (**plan** does, on its thread: `tell_plan`) and never writes the status (**beat** does).
    /// True when a job just ended (look again at once).
    fn round(&mut self) -> Result<bool> {
        nasread::phase("the NAS's mount");
        let slept = self.sleep.slept();
        let home = self.mac.at_home();
        // Back home with the share mounted through Tailscale (mounted while away): unmounted while
        // nothing runs, and mounted again by the LAN name below, at the LAN's speed.
        // (**nas**'s to do: the loop never waits on it.)
        if home && self.idle() && self.o.root.is_none() {
            self.nas.ask(nas::Req::Lan);
        }
        // NAS calls staying busy past whole's retries (a stale handle of this Mac's session):
        // new work drained, then the share remounted (crate::agent::heal).
        if self.o.root.is_none() {
            self.heal_step();
        }
        let root = self.root();
        if root.is_some() {
            self.last_root = root.clone();
        }
        nasread::phase("this Mac's settings");
        let home = cond::home(home, root.is_some() && self.mac.lan_mounted());
        // The owner's asks and settings as control took them up (crate::agent::asks; in line for
        // an agent whose parts don't run): the room target, the in-use share, the build's memory.
        self.control.look();
        let set = self.control.settings.get();
        if set.room.as_ref().map(|t| t.bytes) != self.room_target.as_ref().map(|t| t.bytes) {
            (self.toward_tried, self.floor_short) = (None, None);
        }
        self.room_target = set.room.clone();
        self.in_use_share = set.share.clone();
        self.build_memory = set.memory.clone();
        // (A freeing under way frees toward the goal as it is now: lowered or off, it stops.)
        if self.caches_task.as_ref().is_some_and(|t| t.toward.is_some()) {
            self.toward_goal.store(self.goal(), std::sync::atomic::Ordering::Relaxed);
        }
        let (ac, battery) = self.mac.power();
        let c = Conditions { ac, battery, nas: root.is_some(), home, idle_s: self.mac.idle_seconds() };
        self.note_conditions(&c, slept);
        nasread::phase("the pool's switch and the live view");
        self.pool_switch(root.as_ref());
        self.tend_live(root.as_ref(), &c);
        let mut waiting: Vec<Waiting> = Vec::new();
        if let Some(why) = self.heal.warning() {
            waiting.push(Waiting { step: None, what: "the NAS".into(), why });
        }
        // (A mount or probe hung: only the NAS's work waits, said.)
        if let Some(why) = self.nas.view.get().stuck_why() {
            waiting.push(Waiting { step: None, what: "the NAS".into(), why });
        }
        let mut ended = false;
        self.begin_round();
        nasread::phase("the pause and hold");
        // The build's pause: this Mac's ask passed on; the lead's coordinator's.
        self.sync_pause(root.as_ref(), &mut waiting);
        // This Mac paused alone, or going on (task #224): its setting on the NAS, another member's.
        self.hold_on_nas(root.as_ref());

        nasread::phase("the running jobs");
        // The running jobs; then what they hold together, against this Mac's limit (the memory
        // guard: crate::agent::memguard), its switch as the NAS says.
        if let Some(on) = root.as_ref().and_then(memguard::on) {
            if self.guard_on != Some(on) {
                eprintln!("agent: the memory guard is {}", if on { "on" } else { "off" });
            }
            self.guard_on = Some(on);
        }
        self.total = self.mac_total_mb();
        if self.mem_set.is_none() {
            self.own_mb = crate::sys::footprint().map_or(0, |b| b >> 20);
        }
        // The owner's cancels (`scenic job cancel`, the menu bar, the lead's word): those jobs stopped
        // and given back first.
        ended |= self.tend_cancels(root.as_ref(), &mut waiting);
        self.tend_ramp();
        for k in 0..self.slots.len() {
            ended |= self.tend(k, &c, root.as_ref(), slept)?;
        }
        // (The leases its slots run jobs under, which its coordinator keeps however long this loop
        // stalls: docs/pool.md §7.6.)
        if let Some(co) = &self.coord {
            co.keep_own(
                self.slots
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.running.is_some())
                    .filter_map(|(k, s)| match &s.lease {
                        Some(Held::Own(id) | Held::Pooled { id, .. }) => Some((*id, k as u32)),
                        _ => None,
                    })
                    .collect(),
            );
        }
        nasread::phase("the memory and disk guards");
        ended |= self.guard();
        ended |= self.disk_guard(root.as_ref());
        // (A job ended: room for the target may be made again.)
        if ended {
            self.floor_short = None;
        }
        // The pool's step, once the jobs that ended handed their work over (docs/pool.md §12):
        // what this Mac may do now, as its lead or a member.
        nasread::phase("the pool's step");
        let gates = self.pool_step(root.as_ref(), &c, &mut waiting);
        // (Ends owed to the lead, once this process works as a member; then its beat, listing the
        // leases it holds.)
        if let Some(r) = root.as_ref() {
            self.tell_owed(r);
        }
        self.beat_lead();
        self.lead_awake();
        // (Leading, nothing new starts while the driver says it may not: its view not fresh.
        // Restarting into a new part, nothing either.)
        let pool_holds: Option<String> = match (&gates, self.pool_restart()) {
            (_, Some(why)) => Some(format!("restarting: {why}")),
            (None, None) if self._lock.is_some() && !self.o.dry_run => Some(
                "not in the pool yet: waiting for the NAS, or for this Mac's member's lock (another process holds it)"
                    .into(),
            ),
            (Some(g), None) if self.leads() && !g.duties => Some(if g.leads.is_some() {
                "the lead re-asserts its term first".into()
            } else {
                "this Mac no longer leads the build".into()
            }),
            _ => None,
        };
        // The pass's answers on the NAS, once this agent starts (the lead's).
        self.seed_answers(root.as_ref());

        nasread::phase("the coordinator's contact");
        // The coordinator: where the NAS is (its NAS probe), how to reach it on the NAS
        // (looked at every five minutes: it may have been deleted, or this Mac's addresses changed),
        // and leases whose workers went quiet dropped.
        if let Some(c) = &self.coord {
            c.set_root(root.as_ref());
        }
        // (Where the owner's settings lie, for the build page's `/work/settings`, whichever serves it.)
        for c in [&self.coord, &self.broker, &self.viewer].into_iter().flatten() {
            c.set_settings(&self.o.home, root.as_ref());
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
                eprintln!(
                    "agent: {}'s lease of {} lapsed (no word for {} min); offered again",
                    l.worker,
                    l.what(),
                    crate::coord::TTL.as_secs() / 60
                );
            }
        }
        nasread::phase("the outbox");
        // A helper's finished jobs not yet with the coordinator: sent before anything new.
        if let (true, Some(r)) = (self.is_member(), root.clone()) {
            self.send_outbox(&r, &mut waiting);
        }

        // **plan** told this round's side (a job's end plans again at once); in line, its look made
        // here. On its thread it plans on its own: the round never waits for a plan, and grants
        // from the last one (docs/agent-parts.md §4.4).
        nasread::phase("the plan's view");
        let view = self.tell_plan(root.as_ref(), &c, ended);
        if let Some(why) = self.plan.stalled(&self.o.home) {
            waiting.push(Waiting { step: None, what: "The plan".into(), why });
        }
        // (The jobs granted this round, each with the slot it was granted into and its plan's number.)
        let mut granted: Vec<(usize, JobSpec, u64)> = Vec::new();
        match &root {
            Some(root) if self.is_member() => {
                // A member plans nothing: it asks the lead for work, once its last is handed back.
                // (A folder of a job running here isn't one: its other jobs run beside it.)
                let held: Vec<PathBuf> = self
                    .slots
                    .iter()
                    .filter_map(|s| match &s.lease {
                        Some(Held::Leased { dir, .. }) => Some(dir.clone()),
                        _ => None,
                    })
                    .collect();
                let unsent = std::fs::read_dir(self.outbox())
                    .map(|d| d.flatten().any(|e| !held.contains(&e.path())))
                    .unwrap_or(false);
                // (This Mac's jobs to come in the lead's forecast, for its caches: each round, busy
                // or not, so a member running long jobs still has them fresh when room is made at
                // the next start: `ask_upcoming`.)
                self.ask_upcoming(root);
                // (Its gate asks passed to the lead.)
                self.tell_gate(root);
                // (Not while a clear or a freeing runs here: `caches_busy`.)
                if !unsent && self.caches_task.is_none() {
                    granted = self.helper_grants(root, &c, &mut waiting);
                }
            }
            Some(_) => {
                waiting.extend(view.waiting.iter().cloned());
                // (Why the lead's own planned jobs can't start now.)
                for spec in view.jobs.iter().filter(|s| !offered_job(s)) {
                    if let Some(why) = self.wait_reason(spec, &c) {
                        waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
                    }
                }
            }
            None => {
                waiting.push(Waiting {
                    step: None,
                    what: "All building".into(),
                    why: "the NAS isn't reachable (away from home, or it's off)".into(),
                });
            }
        }
        nasread::phase("the grants");
        // A newer app installed: nothing new starts, so the loop exits once every slot is free (or
        // its job had its wait) and the launcher starts the new one (with work queued back to back,
        // it would otherwise never get a turn).
        let newer = self.newer_app();
        if newer {
            // (A job under way given time to reach its safe point: how long, and why.)
            let drain = (0..self.slots.len()).find_map(|k| {
                let s = &self.slots[k];
                s.running.as_ref()?;
                Some((s.update_wait.clone()?, s.drain_since?))
            });
            let why = match drain {
                Some(((g, words), t)) => {
                    let g = g.unwrap_or(self.grace);
                    format!("restarting into the newly installed app once its jobs reach a safe point, at most {} min more ({} min in all: {words})", g.saturating_sub(t.elapsed()).as_secs().div_ceil(60), g.as_secs().div_ceil(60))
                }
                None => "restarting into the newly installed app".into(),
            };
            waiting.push(Waiting { step: None, what: "Building".into(), why });
        }
        // The build paused: nothing new starts (the running jobs stop above), and on the lead
        // its coordinator gives its workers nothing and has their jobs stop too.
        if let Some(p) = &self.pause {
            waiting.push(Waiting { step: None, what: "Building".into(), why: p.why() });
        }
        if let Some(why) = &pool_holds {
            waiting.push(Waiting { step: None, what: "Building".into(), why: why.clone() });
        }
        // (The lead's coordinator: targets its NAS budget has held back over half an hour.)
        nas_wait_lines(
            &mut waiting,
            self.coord.as_ref().map(|c| c.nas_waits()).unwrap_or_default(),
            self.coord.as_ref().and_then(|c| c.nas_held()),
        );
        // Leading: this Mac's own agent asks its coordinator for work as a member asks the lead's
        // (`lead_grants`), from the last plan, each round a slot is free. (A clear or a freeing
        // toward the room target under way: jobs start meanwhile, what they use held,
        // store::cachefile.)
        if let (Some(r), false) = (root.clone(), self.is_member()) {
            if !newer && self.pause.is_none() && pool_holds.is_none() {
                granted = self.lead_grants(&view.jobs, view.n, &c, &r, &mut waiting);
            }
        }
        // The jobs granted, each in its slot (`start_granted`).
        if !newer && self.pause.is_none() && pool_holds.is_none() {
            self.start_granted(granted, &c, root.as_ref(), &mut waiting);
        }
        // A lease for a job that didn't start (room on the disk, say): given back.
        for k in 0..self.slots.len() {
            if self.slots[k].running.is_none() && self.slots[k].lease.is_some() {
                self.end_lease(k, Outcome::Interrupted, &[], "it couldn't start here");
            }
        }
        self.trim_slots();
        nasread::phase("the caches");
        // This Mac's caches, between jobs: the owner's ask to clear them (room::clear), else the
        // freeing toward the room target.
        let caches_why = self.tend_caches(root.as_ref());
        // The owner's disk room target, while the disk is short of it and stays so.
        if let Some(why) = self.room_short(root.is_some()) {
            waiting.push(Waiting { step: None, what: "The disk room target".into(), why });
        }

        nasread::phase("the status");
        // The heartbeat: the plan's part from its last view.
        let (regions, bad) = match &root {
            Some(_) => (view.regions.clone(), view.bad.clone()),
            None => Default::default(),
        };
        let built = view.built.clone();
        let mut checklist = view.checklist.clone();
        // (Read each loop: a helper's status changes between plans.)
        let helpers: Vec<Status> = match (&root, self.is_member()) {
            (Some(r), false) => helpers(r, &self.host),
            _ => Vec::new(),
        };
        self.note_helpers_caches(&helpers);
        let now_steps: Vec<String> =
            self.slots.iter().filter_map(|s| s.running.as_ref().and_then(|r| step_of(&r.spec.id))).collect();
        annotate(&mut checklist, &now_steps, &helpers, &waiting);
        let checklist: Vec<build::Line> = checklist.into_iter().map(build::Line::said).collect();
        let views: Vec<JobView> = (0..self.slots.len()).filter_map(|k| self.job_view(k)).collect();
        let all_new = self.all_new(root.as_ref());
        let capacity = self.coord.as_ref().map(|c| c.capacities()).unwrap_or_default();
        let mut status = Status {
            host: self.host.clone(),
            pid: std::process::id(),
            app: self.app.clone(),
            job_apps: self.job_apps(),
            beat: now_s(),
            started: self.started,
            conditions: c,
            test_disk_extra_mb: Some(cond::test_disk_extra_mb()).filter(|&m| m > 0),
            waiting,
            notices: old_app_notices(&capacity),
            recent: self.mem.recent.clone(),
            regions,
            bad_recipes: bad,
            built,
            checklist,
            helpers,
            capacity,
            packing: self.coord.as_ref().map(|c| c.refusals()).unwrap_or_default(),
            nas_share: self.coord.as_ref().map(|c| c.nas_shares()).unwrap_or_default(),
            nas_step: self.coord.as_ref().map(|c| c.nas_step_weights()).unwrap_or_default(),
            phase_shares: self.coord.as_ref().map(|c| c.phase_shares()).unwrap_or_default(),
            phase_times: self.coord.as_ref().and_then(|c| c.phase_times()).map(|t| (*t).clone()),
            nas: self.coord.as_ref().and_then(|c| c.nas_view()),
            workers: self
                .coord
                .as_ref()
                .map(|c| {
                    c.workers()
                        .into_iter()
                        .map(|(name, w)| WorkerView {
                            name,
                            label: w.label,
                            kind: w.kind,
                            what: w.what,
                            mem_mb: w.mem_mb,
                            done: w.done,
                            failed: w.failed,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            pause: self.pause.clone(),
            held: self.hold().held.clone(),
            resources: Some(self.resources(root.as_ref())),
            forecast: view.forecast.clone(),
            publishing: view.publishing.clone(),
            timetables: view.timetables.clone(),
            catalogue: view.catalogue,
            published: view.published.clone(),
            caches: Some(self.caches_view(caches_why, c.home, c.nas)),
            pool: self.pool_seen.clone(),
            out_of_pool: self.out_of_pool(),
            memory: Some(self.guard_view()),
            ramp: (self.leads()).then(|| {
                let most = job_slots(mac_cores() as usize);
                let reached = self.mem.ramp.slots.max(RAMP_START);
                let trouble = self.ramp_trouble.clone();
                let clean_since = self.mem.ramp.clean_since;
                RampView {
                    slots: self.job_slots(),
                    reached,
                    most,
                    held: self.ramp_held.then(|| RAMP_HELD_WHY.to_string()),
                    not_clean: trouble.clone(),
                    clean_since,
                    grows_at: (trouble.is_none() && !self.ramp_held && reached < most && clean_since > 0)
                        .then_some(clean_since + RAMP_CLEAN_S),
                }
            }),
            budgets: self.proxy.as_ref().map(|p| p.view()),
            inputs: view.inputs.clone(),
            ..Default::default()
        };
        nasread::phase("the status");
        status.set_jobs(views, all_new);
        let hold = self.hold();
        status.say(crate::control::peek_hold_request(&self.o.home).as_ref(), &hold, false);
        self.publish_jobs();
        if self._lock.is_none() {
            // Another agent writes the heartbeat; this one only reports.
            eprintln!("{}", String::from_utf8_lossy(&serde_json::to_vec_pretty(&status)?));
            return Ok(ended);
        }
        // A helper's own status, never the heartbeat (the Macs' status bars show the main agent's,
        // and its helpers'), here and on the NAS.
        let (local, shared) = if self.is_member() {
            ("helper.json".to_string(), format!("state/helpers/{}.json", self.host))
        } else {
            ("status.json".to_string(), "state/status.json".to_string())
        };
        // Published for the beat, which writes it (crate::agent::beat): at once, and every 10 s
        // after with its time and the hold as they are then. (Not a dry run's on the NAS, in a home
        // of its own: the NAS's is the real agent's.)
        let hold_at = hold.at;
        self.beat.publish(beat::LoopView {
            status,
            local: self.o.home.join(&local),
            shared: root.as_ref().filter(|_| !self.o.dry_run).map(|r| r.join(&shared)),
            hold_at,
            at: Instant::now(),
        });
        Ok(ended)
    }

    /// Why a grant of `spec` from plan `plan` is dropped: its job or a target of it was cancelled
    /// since (a cancel taken while that plan, or an older one, was the newest): None otherwise, and
    /// for a grant of no plan's number (an older lead's).
    fn cancelled_since(&self, spec: &JobSpec, plan: u64) -> Option<String> {
        if plan == 0 {
            return None;
        }
        let targets = spec.record.iter().flat_map(|w| w.targets.iter().map(|t| t.0.as_str()));
        std::iter::once(spec.id.as_str())
            .chain(targets)
            .find(|t| self.cancels_at.get(*t).is_some_and(|&at| plan <= at))
            .map(|t| format!("{t} was cancelled after plan {plan} was made"))
    }

    /// **plan** told this round's side (`plan::Inputs`; `ended`: a job ended, so it plans again at
    /// once), and in line its look made now; the forecast's files it handed over kept for the
    /// lead's pool step (`pool_lead_files`). Its view as it is now: the last plan, whatever one
    /// runs meanwhile.
    fn tell_plan(&mut self, root: Option<&NasRoot>, c: &Conditions, ended: bool) -> std::sync::Arc<plan::PlanView> {
        if self.plan.plans() {
            let inputs = self.plan_inputs(root, c, ended);
            self.plan.send(plan::Msg::Inputs(Box::new(inputs)));
            self.plan.look_inline();
            while let Ok(plan::Out::Forecast(bytes, entry)) = self.plan.out.try_recv() {
                if bytes.is_some() {
                    self.fc_pending.0 = bytes;
                }
                self.fc_pending.1 = Some(entry);
            }
        }
        let view = self.plan.view.get();
        self.plan_n = self.plan_n.max(view.n);
        view
    }

    /// The pool's view taken from its run now, as the round's pool step takes it (a test that sets
    /// the run's gates or members by hand).
    #[cfg(test)]
    fn seen_pool(&mut self) {
        self.pool_seen = self.pooled().map(|r| r.pool_view());
    }

    /// The planner in line, told this agent's side as it is now: how a test plans (the agent's own
    /// run tells it each round, `tell_plan`).
    #[cfg(test)]
    fn planner(&mut self) -> &mut plan::Planner {
        let root = self.o.root.clone();
        let (ac, battery) = self.mac.power();
        let c = Conditions { ac, battery, nas: root.is_some(), home: true, idle_s: 0 };
        let inputs = self.plan_inputs(root.as_ref(), &c, false);
        let (o, locked) = (self.o.clone(), self._lock.is_some());
        let p = self.plan.inline().expect("a planner in line");
        p.take(plan::Msg::Inputs(Box::new(inputs)));
        (p.o, p.locked) = (o, locked);
        p
    }

    /// This round's side as **plan** reads it (`plan::Inputs`): a copy, never a lock.
    fn plan_inputs(&self, root: Option<&NasRoot>, c: &Conditions, ended: bool) -> plan::Inputs {
        let slots = (0..self.slots.len())
            .map(|k| plan::SlotSeen {
                running: self.slots[k].running.as_ref().map(|r| plan::RunSeen { spec: r.spec.clone() }),
                job: self.slot_job(k),
                waiting: self.waiting_words(k),
            })
            .collect();
        let total = match self.total_mb() {
            0 => self.mac_total_mb(),
            t => t,
        };
        plan::Inputs {
            root: root.cloned(),
            conditions: *c,
            mac: self.mac,
            mem: self.mem.clone(),
            slots,
            free_set: self.free_set,
            room_floor: self.floor(),
            total_mb: self.total_mb(),
            jobs_limit: self.jobs_limit(total),
            build_room: self.build_room(),
            guard_on: self.guard_on,
            guard_backoff: self.guard_backoff.clone(),
            floor_short: self.floor_short,
            proxy_up: self.proxy.is_some(),
            pause: self.pause.clone(),
            pool_seen: self.pool_seen.clone(),
            member_id: self.member_id.clone(),
            resume: self.resume,
            placement: self.placement,
            brokers: self.brokers,
            pooled: self.pool.is_some(),
            resume_now: self.resume_now(),
            ramp_held: self.ramp_held,
            ramp_trouble: self.ramp_trouble.clone(),
            broker: self.broker.clone(),
            viewer: self.viewer.clone(),
            coord: self.coord.clone(),
            ended,
            map: self.maplog.started(),
        }
    }

    /// A member's gate asks (from its menu bar, map, `scenic inputs`) passed to the lead's
    /// coordinator, so it lists the inputs folders and the acceptances now rather than at its next
    /// listing; acceptances this Mac writes itself (docs/inputs.md §4.5). The lead's are **plan**'s
    /// (`plan::Planner::tend_gate`).
    fn tell_gate(&mut self, root: &NasRoot) {
        if self.o.dry_run {
            return;
        }
        let mut asks = crate::inputs::take_asks(&self.o.home);
        for c in [&self.broker, &self.viewer].into_iter().flatten() {
            asks.extend(c.take_inputs_asks());
        }
        if asks.is_empty() {
            return;
        }
        let member = crate::inputs::member_of(&self.o.home);
        let mut tell: Vec<serde_json::Value> = Vec::new();
        for a in asks {
            if a.forget {
                tell.push(serde_json::json!({ "unit": a.unit, "forget": true }));
                continue;
            }
            tell.push(serde_json::json!({ "unit": a.unit, "check": true, "full": a.full }));
            if a.all || !a.accept.is_empty() || !a.unaccept.is_empty() {
                apply_gate_ask(root, &a, None, &member);
            }
        }
        let mut waiting = Vec::new();
        match self.client(root, &mut waiting) {
            Some(c) => {
                for t in tell {
                    if let Err(e) = c.post_json("/work/inputs", &t) {
                        eprintln!("agent: telling the lead of {t}: {e:#} (it lists the inputs folders within two minutes all the same)");
                    }
                }
            }
            None => eprintln!("agent: the lead's coordinator can't be reached ({}); it lists the inputs folders within two minutes all the same", waiting.first().map(|w| w.why.clone()).unwrap_or_default()),
        }
    }

    /// The running jobs published (`slots::JobRef`: each one's channel, group, status file and
    /// targets), for control to tell them at once and for **jobs**' thread to reap and cancel them.
    fn publish_jobs(&self) {
        let jobs = (0..self.slots.len())
            .filter_map(|k| {
                let r = self.slots[k].running.as_ref()?;
                let (step, targets) = r.spec.record.as_ref().map_or((None, Vec::new()), |w| {
                    (Some(w.step.clone()), w.targets.iter().map(|t| t.0.clone()).collect())
                });
                Some(slots::JobRef {
                    chan: self.control_path(k),
                    group: r.group(),
                    status: jobs::status_path(&self.record_path(k)),
                    step,
                    targets,
                })
            })
            .collect();
        self.control.jobs.set(jobs);
    }

    /// Slot `k`'s running job, looked after: ended (recorded, its lease ended, a failure retried with
    /// a growing delay), restarted after a sleep when it touches the NAS, stopped when its lease
    /// lapsed or another Mac took its claims, paused or resumed as its conditions and the build's
    /// pause say. True when it ended.
    fn tend(&mut self, k: usize, c: &Conditions, root: Option<&store::nasfs::NasRoot>, slept: u64) -> Result<bool> {
        // (A cancel **jobs**' thread took since `tend_cancels` and signalled: its job recorded as
        // cancelled before its end is seen as a failure. The thread passes a cancel on before it
        // signals, so an end seen here after a look finds its cancel in the second.)
        let mut cancelled = self.take_cancels(root);
        if self.slots[k].running.as_mut().is_some_and(|r| matches!(r.poll(), Ok(Some(_)))) {
            cancelled |= self.take_cancels(root);
        }
        Ok(self.tend_slot(k, c, root, slept)? || cancelled)
    }

    fn tend_slot(
        &mut self,
        k: usize,
        c: &Conditions,
        root: Option<&store::nasfs::NasRoot>,
        slept: u64,
    ) -> Result<bool> {
        let run_time = self.run_time(k);
        // (Or told by control, before this loop's tend: crate::agent::asks.)
        let draining = self.slots[k].drain_since.is_some() || self.told_drain(k);
        let Some(r) = self.slots[k].running.as_mut() else { return Ok(false) };
        if let Some(st) = r.poll()? {
            // (Its own run's time as it says, else as seen here: an ended job is seen at the next
            // loop, up to 20 s later.)
            let wall =
                run_time.filter(|w| *w <= r.elapsed().as_secs_f64() + 1.0).unwrap_or_else(|| r.elapsed().as_secs_f64());
            let secs = wall.round() as u64;
            let ok = st.success();
            // Stopped at a safe point, the build pausing (crate::control): not a failure. (Only
            // when it was asked to: a job's own exit 75 for anything else is a failure.)
            // (An adopted job may have been asked by the agent before this one.)
            let paused = st.code() == Some(crate::control::PAUSED_EXIT) && (draining || r.adopted);
            // (An adopted job killed while no agent tended it: interrupted, not failed.)
            let vanished = r.vanished;
            let of = r.spec.record.as_ref().map_or(0, |w| w.targets.len());
            let step = r.spec.record.as_ref().map(|w| w.step.clone()).unwrap_or_default();
            // (A job with a drop among its targets says nothing of its step's pace: crate::needs::DROPS.)
            let drops = r
                .spec
                .record
                .as_ref()
                .is_some_and(|w| w.targets.iter().any(|(t, key)| build::is_none_key(&w.step, t, key)));
            // The targets it finished: every one when it succeeded, else those it noted done as
            // each was saved (crate::control::done), whatever stopped it, so they aren't built
            // again: handed off with the job's lease (the journal has their saves).
            let done = self.finished_targets(k, ok);
            let r = self.slots[k].running.as_mut().unwrap();
            let note = if ok {
                // (GC's warnings, its job's note in the status: agent::gc::Report::warnings.)
                if step_of(&r.spec.id).as_deref() == Some("gc") {
                    jobs::tail(&r.log, 50).lines().filter(|l| l.contains("gc: warning:")).collect::<Vec<_>>().join("\n")
                } else {
                    String::new()
                }
            } else if paused {
                format!("paused at a safe point, {} of {of} done; the rest goes on when the build does", done.len())
            } else {
                // (Its error first, wherever it is in the log: the timings' table may follow it.)
                let err = jobs::error_of(&r.log);
                format!(
                    "{st}{}\n{}{}",
                    if done.is_empty() { String::new() } else { format!(" ({} of {of} done and kept)", done.len()) },
                    if err.is_empty() { String::new() } else { format!("{err}\n…\n") },
                    jobs::tail(&r.log, 20)
                )
            };
            // (A failure's status and error's first line in the agent's log too.)
            let why = if ok || paused {
                String::new()
            } else {
                let e = jobs::error_of(&r.log);
                match e.lines().next() {
                    Some(l) => format!(" ({st}): {l}"),
                    None => format!(" ({st})"),
                }
            };
            let (id, what) = (r.spec.id.clone(), r.spec.what.clone());
            // (A catalogue's own line of what it did: crate::pubtimes::CatalogueStats.)
            let cat_stats = if ok && step == "catalog" {
                crate::pubtimes::CatalogueStats::from_log(&jobs::tail(&r.log, 60))
            } else {
                None
            };
            self.progress_ended(&id, ok);
            // (What it read from this Mac's disk, its caches above all, and from the NAS: how far
            // the caches spared it the NAS, task #843.)
            let reads = store::nasio::sum_dir(&self.nas_counts_path(k)).map_or(String::new(), |n| {
                let mb = |c: store::nasio::Count| n[c as usize] >> 20;
                format!(
                    "; read {} MB here, {} MB from the NAS",
                    mb(store::nasio::Count::Local),
                    mb(store::nasio::Count::ReadBytes)
                )
            });
            eprintln!(
                "agent: {id} {} after {} s{reads}{why}",
                if ok {
                    "finished"
                } else if paused {
                    "paused at a safe point"
                } else {
                    "failed"
                },
                if wall < 10.0 { format!("{wall:.1}") } else { secs.to_string() }
            );
            let how = if ok {
                String::new()
            } else if paused {
                "paused at a safe point".to_string()
            } else {
                format!("failed ({st})")
            };
            self.note_end(k, &step, &done, secs, ok, &how);
            // (What a target of its step takes here, for the forecast: the first slot's job's, whose
            // pace the jobs beside it change least.)
            // (Not a PER_WORK step's: its targets' times differ by the work each found, crate::needs.)
            if ok && of > 0 && !step.is_empty() && k == 0 && !is_per_work(&step) && !drops {
                let each = secs as f64 / of as f64;
                let e = self.mem.step_secs.entry(secs_key(&step)).or_insert(each);
                *e = 0.7 * *e + 0.3 * each;
            }
            if ok && step == "catalog" {
                // (This Mac's map server adopts it now.)
                crate::net::catalogue_written();
                let ready = self.plan.view.get().ready.clone();
                let (n, data) = (cat_stats.as_ref().map(|c| c.n), cat_stats.and_then(|c| serde_json::to_value(c).ok()));
                // (And the map's log, in shadow: crate::agent::publish::Shadow; its batch says whether
                // it went out with the low zooms made.)
                if let Some(root) = root.filter(|_| self.keeps_publishing()) {
                    let low = self.plan.view.get().catalogue_low;
                    self.maplog.published(&self.o.home, root, n, low);
                }
                self.note(crate::coord::history::Event {
                    worker: Some(self.host.clone()),
                    step: Some(step.clone()),
                    targets: ready,
                    secs: Some(wall),
                    n,
                    data,
                    ..crate::coord::history::Event::new("catalog")
                });
            }
            let outcome = if ok {
                Outcome::Done
            } else if paused {
                Outcome::Paused
            } else if vanished {
                Outcome::Interrupted
            } else {
                Outcome::Failed
            };
            // (The coordinator's record of what's done.)
            let handed = done.clone();
            // (A catalogue made: the gate's count of one held back starts again.)
            if ok && matches!(step.as_str(), "catalog" | "catalog-held") {
                self.plan.send(plan::Msg::CatalogueMade);
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
        if (!paused_job || self.own_pause().is_some())
            && self.slots[k].lease.is_some()
            && self.slots[k].beaten.is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
            && !self.beat(k, root)
        {
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
        let pause = self.own_pause();
        let grace = self.grace;
        // (A newer app installed: every job stops at its next safe point, whatever part this Mac
        // plays, lead, member, broker or helper, and the loop restarts into it once the first job's
        // slot is free, `Agent::update_due`.)
        let update = self.pending_app().map(|a| update_why(&a));
        // (The update's wait, worked out as its drain begins: the target under way's time left.)
        // (Only for a drain that is the update's: a pause, a condition or the memory guard first.)
        let for_update_only = {
            let s = &self.slots[k];
            s.mem_drain.is_none()
                && s.running.as_ref().is_some_and(|r| stop_for(&r.spec.needs, c, pause.as_ref()).is_none())
        };
        let wait = match (&update, &self.slots[k].update_wait) {
            (Some(_), None) if for_update_only => Some(self.update_wait(k)),
            _ => None,
        };
        // (A quiet step's job, the clean-up, yields the NAS at its next safe point to a job held
        // back for it long, which it alone keeps out: never urgent, docs/pool.md §7.7, task #451.)
        let nas_yield = self.slots[k].running.as_ref().and_then(|r| {
            let step = step_of(&r.spec.id)?;
            if !crate::budget::nas::QUIET_STEPS.contains(&step.as_str()) {
                return None;
            }
            let ts: Vec<String> = own_targets(&r.spec).into_iter().map(|t| t.0).collect();
            self.coord.as_ref()?.nas_yield(&step, &ts)
        });
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
        match stop_for(&r.spec.needs, c, pause.as_ref())
            .or_else(|| slot.mem_drain.clone().map(|why| (crate::control::Mode::Drain, why)))
            .or_else(|| update.clone().map(|why| (crate::control::Mode::Drain, why)))
            .or_else(|| nas_yield.map(|why| (crate::control::Mode::Drain, why)))
        {
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
                    slot.update_wait = None;
                }
                if for_update && slot.update_wait.is_none() {
                    let (g, words) = wait.unwrap_or((None, String::new()));
                    eprintln!(
                        "agent: {} has {} min to reach a safe point before the agent restarts into the update, leaving it running: {words}",
                        r.spec.id,
                        g.unwrap_or(grace).as_secs().div_ceil(60)
                    );
                    slot.update_wait = Some((g, words));
                }
                let since = *slot.drain_since.get_or_insert_with(|| {
                    eprintln!("agent: {} stops at its next safe point: {why}", r.spec.id);
                    if let Err(e) = std::fs::write(&control, b"drain") {
                        eprintln!("agent: asking {} to stop: {e}", r.spec.id);
                    }
                    Instant::now()
                });
                // (For the update, not frozen past the grace: the loop restarts then, leaving it
                // running for the new agent to take over, `Agent::update_due`, `hand_over_jobs`.)
                if r.paused.is_none() && since.elapsed() >= grace && !for_update {
                    eprintln!(
                        "agent: {} reached no safe point in {} min; frozen where it is",
                        r.spec.id,
                        DRAIN_GRACE.as_secs() / 60
                    );
                    r.pause(&format!(
                        "{why}; frozen where it is (it reached no safe point in {} min): it goes on from there",
                        DRAIN_GRACE.as_secs() / 60
                    ));
                } else if r.paused.is_some() && since.elapsed() < grace {
                    // (Frozen for a condition that's back: it goes on to its safe point.)
                    r.resume();
                }
                r.pausing = Some(why);
            }
            None => {
                slot.update_wait = None;
                // (Or told "drain" by control and resumed before this loop timed it.)
                let told = std::fs::read(&control).is_ok_and(|b| b.starts_with(b"drain"));
                if slot.mem_drain.is_none() && (slot.drain_since.take().is_some() || told) {
                    std::fs::write(&control, b"run").ok();
                }
                r.pausing = None;
                if r.paused.is_some() {
                    eprintln!("agent: resuming {}", r.spec.id);
                    r.resume();
                }
            }
        }
        self.tend_threads(k, c);
        Ok(false)
    }

    /// A test's start of `spec` in slot `k`, its lease held in this Mac's coordinator as a grant
    /// holds it (`own_grant`), or with no coordinator a lease of none: `try_start_said`.
    #[cfg(test)]
    fn try_start(
        &mut self,
        k: usize,
        spec: JobSpec,
        c: &Conditions,
        root: Option<&store::nasfs::NasRoot>,
        waiting: &mut Vec<Waiting>,
    ) -> bool {
        if self.leads() && self.slots[k].lease.is_none() {
            match self.coord.clone() {
                Some(co) => {
                    let step =
                        spec.record.as_ref().map(|w| w.step.clone()).or_else(|| step_of(&spec.id)).unwrap_or_default();
                    let targets = spec.record.as_ref().map(|w| w.targets.clone()).unwrap_or_default();
                    // (Held by another, or a cancel's hold: refused, why said.)
                    let Some(n) = co.hold(&step, &targets) else {
                        return self.try_start_said(k, spec, c, root, waiting);
                    };
                    if !self.lease_own(k, n, &co, &spec, waiting) {
                        return false;
                    }
                }
                None => self.slots[k].lease = Some(Held::Own(0)),
            }
        }
        self.try_start_said(k, spec, c, root, waiting)
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
        let limit = self.jobs_limit(self.total_mb());
        if let (None, Some(co), true) = (spec.record.as_ref(), self.coord.as_ref(), self.guard_stops() && limit > 0) {
            let j = JobId::parse(&spec.id);
            if let Some(f) = co.holding_floor(j.step, j.target()).filter(|f| f.mb > limit) {
                return Some(format!("needs about {:.1} GB, past this Mac's limit of {:.1} GB, and no other Mac runs it: for the owner to see to (`scenic pool floors --clear` once it's fixed)", f.mb as f64 / 1024.0, limit as f64 / 1024.0));
            }
        }
        // (One the memory guard stopped, kept from this Mac an hour.)
        if let Some((until, why)) = self.guard_backoff.get(&spec.id).filter(|(u, _)| now_s() < *u && self.leads()) {
            return Some(format!("{why}; tried here again in {} min", (until - now_s()).div_ceil(60)));
        }
        // (A helper's job came from the coordinator, which keeps a target it failed from it for an
        // hour, doubling: no wait of its own on top.)
        match self.mem.retry.get(&spec.id).filter(|_| self.leads()) {
            Some(&(n, until)) if now_s() < until => Some(format!(
                "failed {n} time{} in a row; trying again in {} min",
                if n == 1 { "" } else { "s" },
                (until - now_s()).div_ceil(60)
            )),
            _ => None,
        }
    }

    /// The free space a job of `spec` starts with in slot `k`, past the owner's room target: a
    /// member's its step's; else, beside another job, its measured disk; alone, none of its own.
    fn need_of(&self, k: usize, spec: &JobSpec) -> u64 {
        if self.is_member() {
            helper_need(spec.record.as_ref().map_or_else(|| task_kind(spec), |w| w.step.as_str()))
        } else {
            // (Beside another job: what it was measured to take on the disk. Alone in the first
            // slot: nothing past the target, the disk guard its backstop, so a
            // stale or high disk floor, which only rises, never holds the build, the owner's room
            // target past it or not; a cold start likewise. Room is still made for its measured
            // disk as the caches allow: `try_start_said`.)
            let alone = k == 0 && self.slots.iter().skip(1).all(|s| s.running.is_none());
            if alone {
                0
            } else {
                self.measured_job(spec).map_or(0, |m| m.disk_mb << 20)
            }
        }
    }

    fn try_start_said(
        &mut self,
        k: usize,
        spec: JobSpec,
        c: &Conditions,
        root: Option<&store::nasfs::NasRoot>,
        waiting: &mut Vec<Waiting>,
    ) -> bool {
        // (Scenic quitting: no new work taken. A target the owner cancelled to rerun on a newer
        // app: held until the lead runs one.)
        if let Some(why) = self
            .quit_why()
            .or_else(|| self.hold_why())
            .or_else(|| self.held_why(&spec))
            .or_else(|| self.wait_reason(&spec, c))
        {
            waiting.push(Waiting { step: step_of(&spec.id), what: spec.what.clone(), why });
            return false;
        }
        // The pool's lead publishes only once its records reflect the journal, and sweeps only on a
        // step that re-asserted its term, caught up (docs/pool.md §6.2, §6.6): an entry not merged
        // yet may hold uploads the records don't name.
        // (The gates as the round's pool step left them, just before: never waiting on the run.)
        if let (Some(p), false) = (self.pool_seen.as_ref().filter(|_| self.pool.is_some()), self.is_member()) {
            let st = step_of(&spec.id).unwrap_or_default();
            let gates = p.gates.clone();
            if gates.leads.is_some() && !gates.duties {
                waiting.push(Waiting {
                    step: Some(st),
                    what: spec.what.clone(),
                    why: "the lead re-asserts its term first".into(),
                });
                return false;
            }
            if pool::PUBLISHES.contains(&st.as_str()) && !gates.publish() {
                waiting.push(Waiting { step: Some(st), what: spec.what.clone(), why: UNPUBLISHED.into() });
                return false;
            }
            if pool::SWEEPS.contains(&st.as_str()) && !gates.sweep() {
                // (Asked of the next pool step, which holds the run.)
                self.reassert = true;
                waiting.push(Waiting {
                    step: Some(st),
                    what: spec.what.clone(),
                    why: "this Mac re-asserts its term first, its records caught up".into(),
                });
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
        // more, its shard's archive copies spared; the OSM pass's own need, less the pack cache it
        // clears; a helper's Mac has less room). Made before its targets are claimed: it can take
        // minutes, and a claim is refreshed only while a job runs. Made only while no other job
        // runs here (the loop that looks after it waits meanwhile): beside another, a job starts only with
        // the room there is, the target's freeing on a thread of its own (`tend_caches`). Nothing a
        // running job uses goes (it holds it: store::cachefile); not while a freeing of the caches
        // runs (it frees already), nor while a job an earlier agent left runs (`locks_kept`).
        let cache = self.o.home.join("cache");
        // (The other slots' reservations stay free past it too: what their jobs have still to take,
        // and the jobs granted here not yet started, `reserved`.)
        let need = self.need_of(k, &spec).saturating_add(self.reserved(Some(k)));
        // (The owner's disk room target stays free past the job's own room: room::Target.)
        let floor = self.floor_for(&step);
        let room = need.saturating_add(floor);
        let others: Vec<String> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .filter_map(|(_, s)| s.running.as_ref().map(|r| step_of(&r.spec.id).unwrap_or_default()))
            .collect();
        let other = !others.is_empty();
        // (Room-making fell short of the target lately: not tried again until a job ends, the
        // target changes or ten minutes pass; the job waits.)
        let short = self
            .floor_short
            .is_some_and(|(t, n, at)| t == floor && room >= n && at.elapsed() < Duration::from_secs(600));
        // (By measure, the first slot alone waits for the target, when room-making fell short of it: started
        // short of it, the disk guard would freeze it at once and give it back, and the next job
        // likewise, each kept from this Mac an hour.)
        if !(k > 0 || other) && self.leads() && short && self.disk_free() < room {
            waiting.push(Waiting { step: Some(step), what: what.clone(), why: self.floor_why(need) });
            return false;
        }
        if k > 0 || other {
            let free = self.disk_free();
            if (self.leads() || floor > 0) && free < room {
                let held = self.reserved(Some(k));
                let held = if held > 0 {
                    format!(", {} of it reserved by the jobs beside it", room::size(held))
                } else {
                    String::new()
                };
                waiting.push(Waiting { step: Some(step), what: what.clone(), why: format!("needs {} GB free on the disk ({} GB free{held}{}; beside another job, room is made only toward the disk room target)", room >> 30, free >> 30, if floor > 0 { self.floor_words() } else { String::new() }) });
                return false;
            }
        } else if let Some(r) = root.filter(|_| self.caches_task.is_none() && self.locks_kept()) {
            // (Never without the NAS: what goes here must be kept there.)
            // (The OSM pass without the margin: its need is what its conditions admitted it with.)
            let margin = if id.starts_with("osm-pass") { 0 } else { room::margin(need) };
            // (What this job and the queued ones read goes last: a terrain run's own shard's
            // archive copies, the pageview months for an items or heritage job, …)
            let mut hints = self.hints();
            hints.add(
                &step,
                &spec
                    .record
                    .as_ref()
                    .map(|w| w.targets.iter().map(|t| t.0.clone()).collect::<Vec<_>>())
                    .unwrap_or_default(),
            );
            let j = JobId::parse(&id);
            if let Some(t) = j.rest.filter(|_| steps::current(j.step).starts_with("terrain-")) {
                hints.add(j.step, &[t.to_string()]);
            }
            // (Its own scratch, kept from a give-back, its own again before room is made: never
            // room-making's, the OSM pass's above all.)
            if let Some(d) = scratch_of(&spec) {
                room::given_back_drop(&self.o.home, &d);
            }
            // (By measure, alone: room is made, as it can, for its measured disk too, though it
            // waits only on the target, `need_of`: the disk guard its backstop.)
            let measured = if self.leads() { self.measured_job(&spec).map_or(0, |m| m.disk_mb << 20) } else { 0 };
            let sized = room.saturating_add(measured);
            match room::make_room(&cache, &r.join("sources"), sized, margin, &hints) {
                Ok(by) if by.values().sum::<u64>() == 0 => {}
                Ok(by) => {
                    // (By kind, for what the order of task #843 lets go: room::Hints.)
                    let n: u64 = by.values().sum();
                    let kinds = by.iter().map(|(k, b)| format!("{k} {} MB", b >> 20)).collect::<Vec<_>>().join(", ");
                    eprintln!(
                        "agent: {} GB of the cheap caches deleted ({kinds}) for {} GB free to start {id}",
                        n >> 30,
                        sized.saturating_add(margin) >> 30
                    );
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
            if self.leads() && self.disk_free() < room {
                waiting.push(Waiting { step: Some(step), what: what.clone(), why: self.floor_why(need) });
                self.floor_short = Some((
                    floor,
                    self.floor_short.filter(|f| f.0 == floor).map_or(room, |f| f.1.min(room)),
                    Instant::now(),
                ));
                return false;
            }
        } else if self.leads() && self.disk_free() < room {
            // (Without the NAS no room is made, nor while a freeing runs: it waits for it.)
            waiting.push(Waiting { step: Some(step), what: what.clone(), why: self.floor_why(need) });
            return false;
        }
        // A helper's job its disk still has no room for (the caches emptied as far as they could
        // be): given back, not started on a Mac someone uses.
        if self.is_member() {
            let free = room::disk_free(&self.o.home).unwrap_or(0);
            if free < room {
                let why = format!(
                    "too little room on its disk: {} GB free, {} GB needed{}",
                    free >> 30,
                    room >> 30,
                    if floor > 0 { self.floor_words() } else { String::new() }
                );
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
        // Leading, every job starts by its grant (`lead_grants`): its lease in its slot, none without
        // one. (A member's came with its lease too.)
        if self.leads() && self.slots[k].lease.is_none() {
            waiting.push(Waiting {
                step: Some(step),
                what: what.clone(),
                why: "not granted by this Mac's coordinator (every job starts by a grant)".into(),
            });
            return false;
        }
        // An offered step's targets recorded since this loop read the keys (another worker built
        // them meanwhile): not built again, its lease given back (the loop's).
        if let (Some(r), false, Some(w)) =
            (root, self.is_member(), spec.record.as_ref().filter(|w| every::offered(&w.step)))
        {
            let done = self
                .planning_keys_here(r)
                .map(|keys| w.targets.iter().any(|(t, k)| keys.recorded(&w.step, t) == Some(k.as_str())));
            if done.unwrap_or(true) {
                waiting.push(Waiting {
                    step: None,
                    what: what.clone(),
                    why: "another worker built part of it meanwhile (or the keys can't be read now); planning again"
                        .into(),
                });
                return false;
            }
        }
        // (A catalogue's start: the next waits build::CATALOGUE_EVERY_S from it, whether or not it goes out.)
        if spec.id.starts_with("catalog") {
            self.mem.catalogue_at = now_s();
            self.save();
        }
        // Nothing starts under a hold (docs/agent-parts.md §5.3): control's newest, read now, so one
        // taken up during this round holds this start too.
        if let Some(h) = self.hold().held.clone() {
            waiting.push(Waiting { step: step_of(&spec.id), what: what.clone(), why: h.why() });
            return false;
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

    /// The memory a job of `spec` is expected to take (MB): its targets' largest, as measured or
    /// offered (crate::coord::Coordinator::peak), else its step's first guess (`first_peak`).
    fn spec_peak(&self, spec: &JobSpec) -> u64 {
        let step = step_of(&spec.id).unwrap_or_default();
        match (&self.coord, spec.record.as_ref()) {
            (Some(c), Some(w)) => w
                .targets
                .iter()
                .map(|(t, _)| {
                    c.peak(&w.step, t)
                        .unwrap_or_else(|| first_peak(&w.step))
                        .max(c.floor(&w.step, t).map_or(0, |f| f.mb))
                })
                .max()
                .unwrap_or(0),
            _ => first_peak(&step),
        }
    }

    /// This Mac's whole capacity now (crate::coord::Capacity), for its asks and, leading, its own
    /// coordinator: the memory guard's limit; the build's memory (`build_room`: the amount the owner
    /// set with `scenic memory`, never more than the guard's limit; none set, the limit), and that
    /// less each running job's predicted need (the larger of
    /// what it holds and `spec_peak`: its targets' measured peak on the lead,
    /// the step's first guess on a member), the cores the load leaves, the
    /// disk's free space less the owner's room target and its jobs' reservations
    /// (`reserved`), the
    /// owner's use, power and the NAS as `c` says, and its jobs' leases.
    fn capacity(&self, c: &Conditions) -> crate::coord::Capacity {
        let total = match self.total_mb() {
            0 => self.mac_total_mb(),
            t => t,
        };
        let limit_mb = self.jobs_limit(total);
        let (mut held_mb, mut need, mut leases, mut waiting, mut idle, mut old_app) =
            (0u64, 0u64, Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (k, s) in self.slots.iter().enumerate() {
            if let Some(r) = &s.running {
                // (Taken over from an older app's agent, its programs that app's: task #688.)
                if let Some(app) = self.old_app_of(r) {
                    old_app.push(crate::coord::OldAppJob { job: r.spec.id.clone(), app });
                }
                let held = self.held_mb(k).unwrap_or(0);
                held_mb += held;
                need += held.max(self.spec_peak(&r.spec));
            }
            if let Some(l) = &s.lease {
                let n = match l {
                    Held::Own(n) | Held::Leased { lease: n, .. } | Held::Pooled { id: n, .. } => *n,
                };
                leases.push(n);
                // (Its job only waiting on a worker, sampled: the lead packs it at what it holds,
                // none of its cores, task #462.)
                if let Some(held_mb) = self.held_mb(k).filter(|_| self.waiting_on(k).is_some()) {
                    waiting.push(crate::coord::WaitingJob { lease: n, held_mb });
                } else if s.running.is_some()
                    && self.sampler.busy_at(k).is_some_and(|t| t.elapsed().as_secs() >= IDLE_CPU_S)
                {
                    // (Its cores idle a while: a job waiting on the network, task #665.)
                    idle.push(n);
                }
            }
        }
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get() as u32);
        let busy = self.mac.load1().map_or(0, |l| l.ceil() as u32);
        let reserved_mb = self.reserved(None) >> 20;
        crate::coord::Capacity {
            at: now_s(),
            limit_mb,
            held_mb,
            build_mb: self.build_room(),
            mem_mb: self.build_room().saturating_sub(need),
            cores,
            cores_free: cores.saturating_sub(busy),
            disk_mb: self.disk_room() >> 20,
            free_mb: self.disk_free() >> 20,
            target_mb: self.floor() >> 20,
            reserved_mb,
            in_use: c.user_active(),
            kept_cores: share::cores(self.in_use_share.as_ref(), cores),
            ac: c.ac,
            battery: c.battery,
            nas: c.nas,
            home: c.home,
            guard: self.guard_stops(),
            leases,
            waiting,
            idle,
            old_app,
            caches: caches_here(&self.o.home.join("cache")),
            nas_traffic: self.live.as_ref().and_then(|l| {
                let t = now_s();
                let v: Option<Vec<f64>> = live::NAS_TRAFFIC.iter().map(|n| l.latest(n, t, 5)).collect();
                v?.try_into().ok()
            }),
            nas_calls: self.live.as_ref().and_then(|l| l.calls_now(now_s())),
        }
    }

    /// The memory this Mac gives the build's jobs together (MB): the owner's amount
    /// (`scenic memory`, buildmem) within the memory guard's limit, else the limit
    /// (buildmem::room_mb). Its asks' memory (a member's, the lead's own) and the forecast's room.
    fn build_room(&self) -> u64 {
        let total = match self.total_mb() {
            0 => self.mac_total_mb(),
            t => t,
        };
        buildmem::room_mb(self.build_memory.as_ref(), self.jobs_limit(total))
    }

    /// This Mac's memory (MB): as a test sets it, else as read once this loop (`step`); 0 when it
    /// can't be read (the memory guard then guards nothing).
    fn total_mb(&self) -> u64 {
        self.mem_set.map_or(self.total, |m| m.0)
    }

    /// What this Mac's jobs may hold together, of its `total` MB (memguard::jobs_limit_mb): the
    /// guard's limit less this agent's own measured footprint.
    fn jobs_limit(&self, total: u64) -> u64 {
        memguard::jobs_limit_mb(total, self.own_mb)
    }

    /// This Mac's memory (MB) as the Mac says it now (0: unknown).
    fn mac_total_mb(&self) -> u64 {
        match self.mac {
            // (`SCENIC_TEST_MEM_MB`: the two-Mac test's Mac of that memory, its limit squeezed.)
            cond::Mac::Real => cond::test_mem_mb().unwrap_or_else(|| crate::sys::memsize().map_or(0, |b| b >> 20)),
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
    /// sampler saw: while the jobs here hold more together than this Mac's limit, another job than
    /// the largest stops at its next safe point when the largest fits alone (the newest, then the
    /// least progressed, one at a time: memguard::decide_by, `guard_ranks`; and nothing starts
    /// beside the largest until it ends), or, the Mac in trouble, stops at once, then the next while
    /// what's left still passes the limit; the largest, past the limit alone, stops at once while the Mac is in
    /// trouble (the sampler froze it, or the pressure or the swap says so now), else at its next
    /// safe point too; a job drained that reached no safe point in the time a pause gives one stops
    /// at once. One it stops is given back, not failed, kept from this Mac for an hour (a member's
    /// lease ended as failed, so its lead keeps it from this Mac as long), what it held its target's
    /// floor. With the switch off, or this Mac's memory unknown, nothing, and a job the sampler froze
    /// goes on. True when it stopped a job.
    fn guard(&mut self) -> bool {
        let limit = self.jobs_limit(self.total_mb());
        let on = self.guard_stops() && limit > 0;
        self.sampler.set(on, limit);
        if let Some(last) = self.sampler.take_froze().pop() {
            self.guard_last = Some(last);
        }
        if !on {
            for k in 0..self.slots.len() {
                if self.sampler.thaw(k) {
                    eprintln!("agent: memory guard: slot {k}'s job goes on (the guard is off)");
                }
                self.slots[k].pressure_frozen = None;
            }
            return false;
        }
        let held: Vec<Option<u64>> = (0..self.slots.len()).map(|k| self.held_mb(k)).collect();
        let total: u64 = held.iter().flatten().sum();
        // (A job started beside one waiting on a worker, that one gone on: stopped at once, not at
        // a safe point, once the two hold more than the limit or the Mac is short of memory; it was
        // placed by what the other held while it waited, so it did nothing wrong: `yield_stop`.)
        if let Some(k) = (0..self.slots.len()).find(|&k| self.yielding(k)) {
            let trouble = self.sampler.in_trouble();
            if total > limit || trouble {
                let gb = |mb: u64| mb as f64 / 1024.0;
                let why = if trouble {
                    "the Mac short of memory".to_string()
                } else {
                    format!(
                        "the jobs here holding {:.1} GB together, past this Mac's limit of {:.1} GB",
                        gb(total),
                        gb(limit)
                    )
                };
                // (The job it yields to: the one it started beside, in whichever slot.)
                let beside = self.slots[k].yields.clone().unwrap_or_default();
                return self.yield_stop(k, &format!("given back by the memory guard: started beside {beside} while it waited on a worker, it yields now that {beside} goes on, {why}"));
            }
        }
        // (A member's jobs far past what it spared them: one given back at once, `over_grant`.)
        if let Some(stopped) = self.over_grant(&held, limit) {
            return stopped;
        }
        // (The Mac short of memory: frozen, not drained: `pressure`.)
        if let Some(stopped) = self.pressure(&held, limit) {
            return stopped;
        }
        if total <= limit {
            return false;
        }
        let draining: Vec<bool> = self.slots.iter().map(|s| s.mem_drain.is_some()).collect();
        let late = (0..self.slots.len()).find(|&k| {
            draining[k]
                && !self.slots[k].mem_drain_alone
                && self.slots[k].drain_since.is_some_and(|t| t.elapsed() >= self.grace)
        });
        let gb = |mb: u64| mb as f64 / 1024.0;
        let id_of = |a: &Self, k: usize| a.slots[k].running.as_ref().map(|r| r.spec.id.clone()).unwrap_or_default();
        let yields: Vec<bool> = (0..self.slots.len()).map(|k| self.yielding(k)).collect();
        // (Of several, the newest drains first, then the least progressed: `guard_ranks`.)
        let ranks = self.guard_ranks();
        let stop = match (late, memguard::decide_by(&held, limit, &draining, &yields, &ranks)) {
            (Some(k), _) => Some((k, format!("stopped by the memory guard: it reached no safe point in {} min while the jobs here held {:.1} GB together, past this Mac's limit of {:.1} GB", DRAIN_GRACE.as_secs() / 60, gb(total), gb(limit)))),
            (None, memguard::Act::Nothing) => None,
            (None, memguard::Act::Drain(k)) => {
                // (The largest job: the one it makes room for.)
                let big = (0..self.slots.len())
                    .filter(|&b| b != k && held[b].is_some())
                    .max_by_key(|&b| (held[b], std::cmp::Reverse(b)))
                    .unwrap_or(0);
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

    /// A member's jobs far past the memory it spared them when it asked (`Slot::spare_mb`, the most
    /// of its jobs': `far_over`, within this Mac's `limit`), the Mac short of memory or not: the
    /// largest stopped at once (never another, beside it, that did nothing wrong). Past `far` by
    /// itself, or the member's only job, it's given back, not failed, what it held sent as an alone
    /// floor of its target under way, so the lead offers that target only to a Mac that spares that
    /// much; else (or its target under way unknown) it ends as failed (the lead keeps it from this
    /// Mac an hour), so it can't come back here at once. None when none is so far over; else whether it stopped one.
    fn over_grant(&mut self, held: &[Option<u64>], limit: u64) -> Option<bool> {
        let spare = self.slots.iter().filter(|s| s.running.is_some()).filter_map(|s| s.spare_mb).max()?;
        let far = far_over(spare).min(limit);
        let mine: Vec<Option<u64>> =
            held.iter().enumerate().map(|(k, h)| h.filter(|_| self.slots[k].spare_mb.is_some())).collect();
        let total: u64 = mine.iter().flatten().sum();
        if total <= far {
            return None;
        }
        // (The largest: past `far` alone it's what overran; else the jobs together did, and the
        // largest is the one most past its share.)
        let k = (0..mine.len()).filter(|&k| mine[k].is_some()).max_by_key(|&k| (mine[k], std::cmp::Reverse(k)))?;
        let mb = mine[k].unwrap_or(0);
        // (Its own measure only when it alone passed, or ran alone: else what it held says as much
        // of the jobs beside it.)
        let alone = mb > far || mine.iter().flatten().count() == 1;
        let key = self.sampler.current_of(k).filter(|_| alone);
        let gb = |mb: u64| mb as f64 / 1024.0;
        let what = match &key {
            Some(_) => {
                "what it held is kept as the least its target takes, learned alone, so it goes to a Mac with room"
            }
            None => {
                "not past it alone (or its target under way unknown): it ends as failed, kept from this Mac an hour"
            }
        };
        let why = format!("given back by the memory guard: this Mac's jobs held {:.1} GB together ({:.1} GB this one), far past the {:.1} GB it spared them (stopped past {:.1} GB); {what}", gb(total), gb(mb), gb(spare), gb(far));
        let fail = key.is_none();
        self.slots[k].over_floor = key.map(|c| (c, mb));
        Some(self.guard_stop_as(k, "memory guard", &why, fail))
    }

    /// The memory guard while the Mac is short of memory (`memguard::Sampler::in_trouble`), the
    /// jobs together past the limit (`held`, `limit`): rather than wait on a safe point, the
    /// top-ranked job (memguard::decide_by, those frozen already counted as gone) is frozen at once;
    /// it goes on once the Mac is no longer short or the largest job it made room for ended, and is
    /// stopped (given back, as the guard stops one) once frozen `PRESSURE_HOLD` with the Mac still
    /// short, or the time a pause gives a safe point; then the next, if what's left still passes.
    /// None: nothing for it to do (the Mac not short and nothing frozen, or the largest past the
    /// limit alone: the guard's own path, at once); else whether it stopped a job.
    fn pressure(&mut self, held: &[Option<u64>], limit: u64) -> Option<bool> {
        let trouble = self.sampler.in_trouble();
        let id_of = |a: &Self, k: usize| a.slots[k].running.as_ref().map(|r| r.spec.id.clone());
        for k in 0..self.slots.len() {
            let Some((at, mine, big)) = self.slots[k].pressure_frozen.clone() else { continue };
            if id_of(self, k).as_deref() != Some(mine.as_str()) {
                self.slots[k].pressure_frozen = None;
                continue;
            }
            let big_on = self.slots.iter().any(|s| s.running.as_ref().is_some_and(|r| r.spec.id == big));
            if !trouble || !big_on {
                // (Paused meanwhile, the build's pause or a drain past its grace: it stays so.)
                let paused = self.slots[k].running.as_ref().is_some_and(|r| r.paused.is_some());
                if paused {
                    self.slots[k].pressure_frozen = None;
                    continue;
                }
                self.sampler.thaw(k);
                self.slots[k].pressure_frozen = None;
                let why = if trouble { format!("{big} ended") } else { "the Mac no longer short of memory".into() };
                eprintln!("agent: memory guard: {mine} goes on: {why}");
                self.guard_last = Some((now_s(), format!("{mine} goes on: {why}")));
                continue;
            }
            if at.elapsed() >= PRESSURE_HOLD || at.elapsed() >= self.grace {
                let why = format!("stopped by the memory guard: frozen {} s for {big}, the Mac still short of memory with the jobs here past this Mac's limit of {:.1} GB", at.elapsed().as_secs(), limit as f64 / 1024.0);
                return Some(self.guard_stop(k, "memory guard", &why));
            }
        }
        if !trouble {
            return None;
        }
        // (A paused job, the build's pause or a drain past its grace, counts as frozen already: never
        // frozen for the pressure, so never let go by it.)
        let frozen: Vec<bool> = self
            .slots
            .iter()
            .map(|s| s.pressure_frozen.is_some() || s.running.as_ref().is_some_and(|r| r.paused.is_some()))
            .collect();
        let yields: Vec<bool> = (0..self.slots.len()).map(|k| self.yielding(k)).collect();
        let k = match memguard::decide_by(held, limit, &frozen, &yields, &self.guard_ranks()) {
            memguard::Act::Drain(k) => k,
            memguard::Act::Over(_) => return None,
            memguard::Act::Nothing => return Some(false),
        };
        let big = (0..self.slots.len())
            .filter(|&b| b != k && held[b].is_some())
            .max_by_key(|&b| (held[b], std::cmp::Reverse(b)))
            .and_then(|b| id_of(self, b))
            .unwrap_or_default();
        let Some(mine) = id_of(self, k) else { return Some(false) };
        if self.sampler.freeze(k) {
            let total: u64 = held.iter().flatten().sum();
            let why = format!("the Mac short of memory with the jobs here holding {:.1} GB together, past this Mac's limit of {:.1} GB: frozen for {big}, stopped in {} s if it stays short", total as f64 / 1024.0, limit as f64 / 1024.0, PRESSURE_HOLD.as_secs());
            eprintln!("agent: memory guard: {mine} {why}");
            self.guard_last = Some((now_s(), format!("{mine} {why}")));
            self.slots[k].pressure_frozen = Some((Instant::now(), mine, big));
        }
        Some(false)
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
        self.guard_stop_as(k, what, why, true)
    }

    /// `guard_stop`, `fail` false: given back as an interrupted job is (not failed, not kept from
    /// this Mac), what it took still learned as its targets' floors.
    fn guard_stop_as(&mut self, k: usize, what: &str, why: &str, fail: bool) -> bool {
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
        if fail {
            self.guard_backoff.insert(id, (now_s() + GUARD_BACKOFF_S, why.clone()));
        }
        self.slots[k].guard_stopped = fail;
        // (The lead's ramp's clean stretch starts again, `tend_ramp`, kept.)
        (self.mem.ramp.clean_since, self.mem.ramp.peak) = (now_s(), 0);
        self.save();
        self.stopped(k, &why);
        // (Its lease ended, given back: its scratch kept, its work with it, until room-making is
        // short of the room it needs, `room::free_given_back`.)
        if let Some(d) = &scratch {
            room::given_back_add(&self.o.home, d, now_s());
        }
        self.slots[k].running = None;
        self.slots[k].drain_since = None;
        self.slots[k].mem_drain = None;
        self.slots[k].mem_drain_alone = false;
        self.slots[k].guard_stopped = false;
        self.slots[k].over_floor = None;
        self.slots[k].pressure_frozen = None;
        self.slots[k].yields = None;
        std::fs::remove_file(self.record_path(k)).ok();
        true
    }

    /// The disk guard (docs/pool.md §7.7), once a loop, with the Mac shared by measure: a job the
    /// sampler froze, the disk short of the owner's room target (memguard::disk_short_of; no
    /// target: the disk full) and it growing fastest, goes on once the disk has the target free
    /// again (no target: a byte); else room is made from the caches no job holds (store::cachefile's
    /// exclusive locks: room::toward, on a thread of its own), and once that's done, the disk still
    /// short, the job is given back (`guard_stop`: not failed, its work kept by its hand-off, its
    /// disk floor learned). True when it stopped a job.
    fn disk_guard(&mut self, root: Option<&store::nasfs::NasRoot>) -> bool {
        // (The target alone the threshold, task #421: no fixed reserve, none hidden; with no target
        // a byte free, so a full disk.)
        let floor = self.floor().max(1);
        self.sampler.set_disk(true, self.floor(), &self.o.home);
        let frozen: Vec<usize> =
            (0..self.slots.len()).filter(|&k| self.slots[k].running.is_some() && self.sampler.disk_frozen(k)).collect();
        if frozen.is_empty() {
            self.disk_room = None;
            return false;
        }
        if self.disk_free() >= floor {
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
        let k = frozen
            .iter()
            .copied()
            .max_by_key(|&k| (self.sampler.disk_now(k).unwrap_or(0), std::cmp::Reverse(k)))
            .unwrap_or(frozen[0]);
        let ours = self.caches_task.as_ref().is_some_and(|t| t.disk);
        match self.disk_room {
            Some(_) if ours => false,
            // (Made, and still short: the scratch of jobs given back here, the oldest first; still
            // short, given back.)
            Some(_) if self.caches_task.is_none() => {
                if self.free_given_back(floor + room::margin(floor)) > 0 && self.disk_free() >= floor {
                    return false;
                }
                self.disk_room = None;
                let why = format!("given back by the disk guard: the disk had {} free, short of the disk room target's {}, with no more room to be made from the caches no job holds (what it took on the disk is kept as the least its target takes)", room::size(self.disk_free()), room::size(self.floor()));
                self.guard_stop(k, "disk guard", &why)
            }
            // (A freeing of the caches under way frees already: its end waited for.)
            _ if self.caches_task.is_some() => false,
            _ => {
                let Some(r) = root else { return false };
                let (cache, sources) = (self.o.home.join("cache"), r.long_retry().join("sources"));
                let goal = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(floor + room::margin(floor)));
                let hints = self.hints();
                match std::thread::Builder::new()
                    .name("caches".into())
                    .spawn(move || room::toward(&cache, &sources, goal, &hints))
                {
                    Ok(thread) => {
                        self.caches_task =
                            Some(CachesTask { ask: None, toward: None, disk: true, began: Instant::now(), thread });
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
        let busy: Vec<PathBuf> =
            self.slots.iter().filter_map(|s| s.running.as_ref().and_then(|r| scratch_of(&r.spec))).collect();
        room::free_given_back(&self.o.home, need, &busy, &|| self.disk_free())
    }

    /// What a job of `spec` is measured to take (docs/pool.md §7.7, crate::coord::Coordinator::
    /// measured): the most of its targets' memory and disk (MB), or for a job of no targets of its
    /// own, its id's; None for a cold start (a target of a step none of whose targets has run), or
    /// with no coordinator to ask.
    fn measured_job(&self, spec: &JobSpec) -> Option<crate::coord::Measured> {
        let c = self.coord.as_ref()?;
        match spec.record.as_ref() {
            Some(w) if !w.targets.is_empty() => {
                c.measured_batch(&w.step, &w.targets.iter().map(|t| t.0.clone()).collect::<Vec<_>>())
            }
            _ => {
                let key = own_cost_key(&spec.id);
                let step = memguard::step_of_key(&key).to_string();
                let target = key.strip_prefix(&format!("{step} ")).unwrap_or(&key).to_string();
                c.measured_batch(&step, &[target])
            }
        }
    }

    /// The disk slot `k` has reserved and not taken yet (bytes; docs/pool.md §7.1): while it holds a
    /// lease or runs a job, the job's measured disk less what it has written so far. On the lead,
    /// a running job's by its own measures (`disk_left`); a member's by what its lead granted it
    /// with (`Slot::reserve_mb`), a job granted and not yet started whole. Nothing for a slot
    /// holding nothing, or a cold start (no measure: the disk guard its backstop).
    fn reservation(&self, k: usize) -> u64 {
        let s = &self.slots[k];
        if s.running.is_none() && s.lease.is_none() {
            return 0;
        }
        let granted = s.reserve_mb.map_or(0, |mb| mb << 20);
        if s.running.is_none() {
            return granted;
        }
        let left = granted.saturating_sub(self.sampler.disk_now(k).unwrap_or(0));
        if self.coord.is_some() {
            left.max(self.disk_left(k))
        } else {
            left
        }
    }

    /// The disk this Mac's jobs have reserved and not taken yet (bytes): every slot's but
    /// `except`'s (`reservation`).
    fn reserved(&self, except: Option<usize>) -> u64 {
        (0..self.slots.len()).filter(|&k| Some(k) != except).map(|k| self.reservation(k)).sum()
    }

    /// The disk a job could have now (bytes): the free space less the owner's room target and the
    /// jobs' reservations (`reserved`): what an ask says (`disk_mb`).
    fn disk_room(&self) -> u64 {
        self.disk_free().saturating_sub(self.floor() + self.reserved(None))
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
        // (A halo step's targets share its cache, already counted in what it took so far.)
        let each = left.iter().filter_map(|t| c.measured(&step, t)).map(|m| m.disk_mb << 20);
        let targets = if crate::needs::HALO_STEPS.contains(&step.as_str()) {
            each.max().unwrap_or(0).saturating_sub(now)
        } else {
            each.sum()
        };
        targets.max((c.job_disk(&step) << 20).saturating_sub(now))
    }

    /// The job in slot `k` the memory guard drained the one beside, while it runs: nothing starts
    /// in the other slot until it ends.
    fn guard_held(&self, k: usize) -> Option<String> {
        let (slot, id) = self.guard_hold.clone()?;
        // (A slot dropped since, `trim_slots`: its job ended.)
        let running = self.slots.get(slot).and_then(|s| s.running.as_ref()).map(|r| &r.spec.id);
        (slot == k && running == Some(&id)).then_some(id)
    }

    /// The memory guard for the status.
    fn guard_view(&self) -> memguard::View {
        let limit = self.jobs_limit(self.total_mb());
        let why_off = match (self.guard_stops(), limit) {
            (false, _) => Some("switched off (state/pool/memory-guard)".to_string()),
            (true, 0) => Some("this Mac's memory can't be read: nothing is stopped".to_string()),
            _ => None,
        };
        memguard::View {
            on: why_off.is_none(),
            limit_mb: limit,
            held_mb: (0..self.slots.len()).filter_map(|k| self.held_mb(k)).sum(),
            why_off,
            last: self.guard_last.clone(),
        }
    }

    /// Whether every member of the pool reports the new app (`Status::all_new`): the lead's
    /// coordinator's word (`Coordinator::all_new`); a member's, its lead's heartbeat's as last
    /// read (once a minute: `lead_all_new`; an older lead's says no). No coordinator and no pool: true.
    fn all_new(&mut self, root: Option<&store::nasfs::NasRoot>) -> bool {
        if let Some(c) = self.coord.as_ref() {
            return c.all_new();
        }
        if self.leads() {
            return true;
        }
        let stale = self.lead_all_new.is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(60));
        if let (Some(root), true) = (root, stale) {
            let st: Option<Status> =
                nasread::read(&root.join("state/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
            self.lead_all_new = Some((Instant::now(), st.is_some_and(|s| s.all_new)));
        }
        self.lead_all_new.is_some_and(|(_, n)| n)
    }

    /// Slot `k`'s job for the status: its parts (the last it said), its progress, and from its pace
    /// the time it has left (kept for the forecast).
    fn job_view(&mut self, k: usize) -> Option<JobView> {
        let waiting = self.waiting_words(k);
        let live = self.job_live(k);
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
            // (Its pace measured again from a new z6 tile, a new part, or a step back.)
            let key = jobs::unit_key(&unit).to_string();
            let fresh = r.progress_base.as_ref().is_none_or(|b| b.2 != key || b.3 != part || frac < b.1);
            if fresh {
                r.progress_base =
                    Some(pace_base(r.adopted && r.progress_base.is_none(), r.started_at, frac, key, part));
            }
            if fresh || r.moved.is_none_or(|m| frac > m.0) {
                r.moved = Some((frac, now_s()));
            }
            let (t0, f0, _, _) = r.progress_base.as_ref().unwrap();
            let eta_s = (frac > *f0 && r.paused.is_none())
                .then(|| (t0.elapsed().as_secs_f64() * (1.0 - frac) / (frac - f0)) as u64);
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
            slot: Some(k),
            live,
        })
    }

    /// The proxy's tag for slot `k`'s job (crate::budget::net: its internet bytes).
    fn net_tag(k: usize) -> String {
        format!("slot{k}")
    }

    /// What slot `k`'s job is doing now (jobs::Live), while one runs.
    fn job_live(&self, k: usize) -> Option<jobs::Live> {
        let r = self.slots[k].running.as_ref()?;
        let targets = r.spec.record.as_ref().map_or(0, |w| w.targets.len());
        let net = self.proxy.as_ref().and_then(|p| p.net()).and_then(|n| n.job(&Self::net_tag(k)));
        Some(jobs::Live::read(&self.nas_counts_path(k), &self.slot_costs(k), targets, net))
    }

    /// This Mac's resources for the status (cond::Resources); its caches counted again on a thread
    /// of their own when ten minutes old.
    fn resources(&self, root: Option<&store::nasfs::NasRoot>) -> cond::Resources {
        // (A count's result taken in; not over a count made since, a clear's or a freeing's.)
        while let Ok((at, n)) = self.cache_sized.1.try_recv() {
            let mut c = self.cache_size.borrow_mut();
            if c.0 == Some(at) {
                c.1 = Some(n);
            }
        }
        let (counted, bytes) = self.cache_size.borrow().clone();
        if counted.is_none_or(|t| t.elapsed() >= Duration::from_secs(600)) {
            // (Marked counted now: one walk at a time.)
            let now = Instant::now();
            self.cache_size.borrow_mut().0 = Some(now);
            let (cache, helper, sized, nas) =
                (self.o.home.join("cache"), self.is_member(), self.cache_sized.0.clone(), root.cloned());
            std::thread::spawn(move || {
                sized.send((now, room::sizes(&cache, helper, nas.as_ref()))).ok();
            });
        }
        // (The NAS's latency as the live store last measured it, within half a minute: none
        // while no build runs.)
        let ms = self.live.as_ref().and_then(|l| l.latest(live::NAS_MS, now_s(), 30));
        self.mac.resources(
            &self.o.home,
            root,
            bytes.map(|b| (b.cheap as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0),
            ms.filter(|_| root.is_some()).map(|m| m.round() as u64),
        )
    }

    /// This Mac's caches (room::clear, room::toward), each on a thread of its own while the loop goes
    /// on beating (jobs run and start meanwhile, holding what they use: `caches_busy`): the owner's
    /// ask to clear them taken up at once, to its level (declined, why said, only for
    /// `caches_why_not`), else the freeing toward the owner's room target when the disk is short of
    /// it (a Mac with room keeps its caches). Why they can't be cleared now (None: they can), for
    /// the status.
    fn tend_caches(&mut self, root: Option<&store::nasfs::NasRoot>) -> Option<String> {
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
        self.retire(&cache, root);
        if let Some(ask) = room::take_clear(&self.o.home) {
            match (&why, sources) {
                (None, Some(s)) => {
                    let (level, answering) = (ask.level, self.answering());
                    self.caches_start(ask, move || room::clear(&cache, &s, level, answering))
                }
                _ => self.caches_record(room::Freed { why_not: why.clone(), ..Default::default() }, ask),
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
        }
        // (One loop at a time, `--once`: waited for, so its heartbeat says what it did.)
        if self.o.once {
            if let Some(t) = self.caches_task.take() {
                self.caches_done(t);
            }
        }
        self.caches_busy().or(why)
    }

    /// What no current code reads of this Mac's cache deleted (room::retire), at the agent's start
    /// and daily after, on a thread of its own, jobs running or not (what they use they hold:
    /// store::cachefile); not while a job an earlier agent left runs (its programs may not hold what
    /// they use), nor without the NAS (the manifest says which copies are current). What it freed
    /// logged.
    fn retire(&mut self, cache: &Path, root: Option<&store::nasfs::NasRoot>) {
        if let Some(t) = self.retire_task.take_if(|t| t.is_finished()) {
            match t.join() {
                Ok(f) if !f.is_empty() => {
                    let said: Vec<String> =
                        f.iter().map(|(k, b)| format!("{} {}", room::size(*b), room::words(k))).collect();
                    eprintln!("agent: deleted what no code reads of the cache: {}", said.join(", "));
                }
                Ok(_) => {}
                Err(_) => eprintln!("agent: the sweep of the cache's retired files failed midway"),
            }
        }
        self.orphans.retain(jobs::Group::is_the_jobs);
        let Some(root) = root else { return };
        if self.retire_task.is_some()
            || !self.orphans.is_empty()
            || self.retired_at.is_some_and(|t| t.elapsed() < Duration::from_secs(86400))
        {
            return;
        }
        self.retired_at = Some(Instant::now());
        let (cache, root) = (cache.to_path_buf(), root.long_retry());
        self.retire_task =
            std::thread::Builder::new().name("retire".into()).spawn(move || room::retire(&cache, Some(&root))).ok();
    }

    /// The lead's, once an agent, when no job runs here: the pass's answers its cache has, sent
    /// to the NAS where it hasn't their archive (crate::answers::seed: a cache from before the NAS
    /// kept them, or one a stopped job left, needn't wait for the steps' next run, perhaps the next
    /// pass's), on a thread of its own (a slow NAS doesn't hold the loop; no items or heritage job
    /// starts meanwhile). Done: what it did logged; failed: the steps' next start sends them.
    fn seed_answers(&mut self, root: Option<&store::nasfs::NasRoot>) {
        if let Some(t) = self.answers_seed.take_if(|t| t.is_finished()) {
            match t.join() {
                Ok(Ok(said)) => said.iter().for_each(|s| eprintln!("agent: {s}")),
                Ok(Err(e)) => eprintln!("agent: the pass's answers not sent to the NAS now ({e:#}); the items and heritage jobs' next start sends them"),
                Err(_) => eprintln!("agent: sending the pass's answers to the NAS failed midway"),
            }
        }
        self.orphans.retain(jobs::Group::is_the_jobs);
        let busy = self.slots.iter().any(|s| s.running.is_some()) || !self.orphans.is_empty();
        let Some(root) =
            root.filter(|_| !self.answers_seeded && !busy && self.leads() && self._lock.is_some() && !self.o.dry_run)
        else {
            return;
        };
        self.answers_seeded = true;
        let (root, cache, scratch) =
            (root.long_retry(), self.o.home.join("cache"), self.o.home.join("scratch/answers"));
        // (With the budgets on, alone among the steps keeping answers here by their lock, and each
        // archive sent under the answers' lease on the NAS: crate::answers::Local, Lease.)
        let leased = true;
        match std::thread::Builder::new()
            .name("answers".into())
            .spawn(move || crate::answers::seed(&root, &cache, &scratch, leased))
        {
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

    /// Starts a clear (`ask`) on a thread of its own; one that can't start is a failure.
    fn caches_start(&mut self, ask: room::ClearRequest, work: impl FnOnce() -> Result<room::Freed> + Send + 'static) {
        match std::thread::Builder::new().name("caches".into()).spawn(work) {
            Ok(thread) => {
                self.caches_task =
                    Some(CachesTask { ask: Some(ask), toward: None, disk: false, began: Instant::now(), thread })
            }
            Err(e) => self.caches_failed(anyhow::anyhow!("its thread didn't start: {e}"), ask),
        }
    }

    /// Starts a freeing toward the owner's disk room target (`target`; `goal`, the room it frees
    /// toward, bytes) on a thread of its own; one that can't start is tried again with the next
    /// (`toward_due`).
    fn caches_start_toward(
        &mut self,
        target: u64,
        goal: u64,
        work: impl FnOnce() -> Result<room::Freed> + Send + 'static,
    ) {
        match std::thread::Builder::new().name("caches".into()).spawn(work) {
            Ok(thread) => {
                self.caches_task = Some(CachesTask {
                    ask: None,
                    toward: Some((target, goal)),
                    disk: false,
                    began: Instant::now(),
                    thread,
                })
            }
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
        let fresh = self
            .toward_tried
            .is_some_and(|(t, at)| t >= target && now_s().saturating_sub(at) < 600 && self.mem.worked_at <= at);
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

    /// What's done to this Mac's caches now, while a clear or a freeing toward the target
    /// runs (jobs run and start meanwhile: what they use stays, store::cachefile).
    fn caches_busy(&self) -> Option<String> {
        let t = self.caches_task.as_ref()?;
        let doing = match (&t.ask, t.toward) {
            (Some(_), _) => "cleared".to_string(),
            (None, Some((t, g))) => format!("freed toward the disk room target ({}){}", room::size(t), room_past(t, g)),
            (None, None) => "freed for a job".to_string(),
        };
        Some(format!(
            "this Mac's caches are being {doing} ({} min so far); jobs run meanwhile, keeping what they use",
            t.began.elapsed().as_secs() / 60
        ))
    }

    /// A clear or a freeing done (waited for, when it isn't yet), its result recorded. Asked to stop
    /// midway: nothing kept, the freeing done again by the next agent, the ask left for it.
    fn caches_done(&mut self, t: CachesTask) {
        let r = t.thread.join().unwrap_or_else(|_| Err(anyhow::anyhow!("it failed midway")));
        if stopping() {
            return;
        }
        if t.disk {
            match r {
                Ok(f) => {
                    let note = format!(
                        "made room for the job the disk guard froze: {}; {} free",
                        f.say(),
                        room::size(self.disk_free())
                    );
                    eprintln!("agent: {note}");
                    if f.bytes() > 0 {
                        self.note(crate::coord::history::Event {
                            worker: Some(self.host.clone()),
                            note,
                            ..crate::coord::history::Event::new("caches")
                        });
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
                Err(e) => eprintln!(
                    "agent: freeing the caches toward the disk room target: {e:#}; trying again in ten minutes"
                ),
            }
            return;
        }
        // (Only a clear is left: the others returned above.)
        let Some(ask) = t.ask else { return };
        match r {
            Ok(f) => self.caches_record(f, ask),
            Err(e) => self.caches_failed(e, ask),
        }
    }

    /// A clear that failed: its ask answered with why.
    fn caches_failed(&mut self, e: anyhow::Error, ask: room::ClearRequest) {
        self.caches_record(room::Freed { why_not: Some(format!("{e:#}")), ..Default::default() }, ask)
    }

    /// What a clear did, or why it wasn't done: logged, kept for the status (a clear declined apart
    /// from the last one done), noted in the history, its ask answered.
    fn caches_record(&mut self, mut f: room::Freed, ask: room::ClearRequest) {
        f.at = now_s();
        (f.asked, f.by) = (Some(ask.at), Some(ask.by.clone()));
        f.level = Some(ask.level);
        let e = caches_event(&self.host, &f);
        eprintln!("agent: {}", e.note);
        self.note(e);
        let done = f.why_not.is_none();
        if done && ask.level == room::Level::Deep {
            self.mem.cleared_deep = Some(f);
        } else if done {
            self.mem.cleared = Some(f);
        } else {
            self.mem.declined = Some(f);
        }
        self.save();
        room::clear_answered(&self.o.home);
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
        let note = format!(
            "freed its caches toward the disk room target ({}){}: {}; {} free",
            room::size(target),
            room_past(target, goal),
            f.say(),
            room::size(free)
        );
        eprintln!("agent: {note}");
        if f.bytes() > 0 {
            self.note(crate::coord::history::Event {
                worker: Some(self.host.clone()),
                note,
                ..crate::coord::history::Event::new("caches")
            });
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
    /// NAS's speed here (measured: 60 MB/s on the LAN, 12 through Tailscale, task #484).
    fn caches_view(&self, why: Option<String>, home: bool, nas: bool) -> room::Caches {
        let sizes = self.cache_size.borrow().1.clone();
        room::Caches {
            clearable: sizes.as_ref().map(|s| s.clearable(room::Level::Caches)),
            // (Not the answers while a job keeping them runs here: a deep clear keeps them then.)
            clearable_deep: sizes.as_ref().map(|s| {
                let held = if self.answering() { s.clear.get("answers").copied().unwrap_or(0) } else { 0 };
                s.clearable(room::Level::Deep) - held
            }),
            each: sizes.map(|s| room::gone(&s.clear, if home { 60.0 } else { 12.0 })).unwrap_or_default(),
            why_not: why,
            cleared: self.mem.cleared.clone(),
            cleared_deep: self.mem.cleared_deep.clone(),
            declined: self.mem.declined.clone(),
            room: Some(room::RoomView {
                target: self.room_target.clone(),
                free: self.disk_free(),
                reserved_mb: self.reserved(None) >> 20,
                toward: self.mem.toward.clone(),
                short: self.room_short(nas),
            }),
        }
    }

    /// Why this Mac's caches can't be cleared now, if they can't: a job an earlier agent
    /// left runs here (`orphans`: its programs may not hold what they use), or the NAS isn't
    /// reachable (what goes must be kept there). Jobs of this agent's running, and work left, are
    /// no reason: what a job uses it holds (store::cachefile), and the build never ends.
    fn caches_why_not(&mut self, root: Option<&store::nasfs::NasRoot>) -> Option<String> {
        self.orphans.retain(jobs::Group::is_the_jobs);
        if let Some(g) = self.orphans.first() {
            return Some(format!("a job an earlier agent left still runs here ({})", g.id));
        }
        root.is_none().then(|| "the NAS isn't reachable".into())
    }

    /// Whether a job keeping the Wikidata and Wikipedia answers in this Mac's cache runs here (the
    /// items job, the heritage chain: crate::answers), so a deep clear keeps them (room::clear).
    fn answering(&self) -> bool {
        self.slots
            .iter()
            .filter_map(|s| s.running.as_ref())
            .any(|r| matches!(step_of(&r.spec.id).as_deref(), Some("items" | "heritage" | "heritage-sites")))
    }

    /// A job of `id` ran here: this Mac's caches may hold more, so a freeing toward the room target
    /// is tried again (not after the daily ones: they read no cache).
    fn worked(&mut self, id: &str) {
        if !matches!(step_of(id).as_deref(), Some("backup" | "gc")) {
            self.mem.worked_at = now_s();
        }
    }

    /// Counts this Mac's caches again now (after a clear or a freeing), for the status, and what a
    /// helper's can free before it next asks for work. (Not asking the NAS anything: on the loop, a
    /// NAS hanging then would hold it; links aside, the counting thread's checks are its.)
    fn count_caches(&mut self) {
        let n = room::sizes(&self.o.home.join("cache"), self.is_member(), None);
        *self.cache_size.borrow_mut() = (Some(Instant::now()), Some(n));
        self.cheap = None;
    }

    /// The lead's: the members' clears and declined asks noted in the history as their statuses
    /// show them, each once (those from before this agent started were noted by the one before).
    fn note_helpers_caches(&mut self, helpers: &[Status]) {
        if self.coord.is_none() {
            return;
        }
        for h in helpers {
            let Some(c) = &h.caches else { continue };
            let seen = *self.helper_caches.entry(h.host.clone()).or_insert(self.started);
            let mut new: Vec<&room::Freed> = [&c.cleared, &c.cleared_deep, &c.declined]
                .into_iter()
                .filter_map(|f| f.as_ref().filter(|f| f.at > seen))
                .collect();
            new.sort_by_key(|f| f.at);
            for f in new {
                self.note(caches_event(&h.host, f));
                self.helper_caches.insert(h.host.clone(), f.at);
            }
        }
    }

    /// The owner's room target as a job of `spec` is told it (room::TARGET_ENV, MB): the target
    /// it keeps (`floor_for`: none for the backup and GC).
    fn target_env(&self, spec: &JobSpec) -> (String, String) {
        let step = step_of(&spec.id).unwrap_or_default();
        (room::TARGET_ENV.into(), (self.floor_for(&step) >> 20).to_string())
    }

    fn start(&mut self, k: usize, mut spec: JobSpec, c: &Conditions) -> Result<()> {
        let cores = mac_cores() as usize;
        // The threads its grant says (crate::coord's `pack`: its measured cores within those its Mac
        // has left), the lead's own jobs' and a member's alike. A grant that says none (an older
        // lead's): all the cores (a helper, two fewer: its Mac has less memory).
        let granted = self.slots[k].granted.take();
        let wanted = match (granted, self.is_member()) {
            (Some(g), _) => g as usize,
            (None, true) => cores.saturating_sub(2).max(1),
            (None, false) => cores,
        };
        // Within the cores its Mac has less those the in-use share keeps while the owner uses it (one
        // at least). Those its other jobs were given aren't taken off: its lead packed it beside
        // them by the cores each keeps busy (crate::coord::sched's `need_cores`, task #688), so a
        // job waiting on its reads keeps its threads.
        let kept = if c.user_active() { share::cores(self.in_use_share.as_ref(), cores as u32) as usize } else { 0 };
        let threads = wanted.min(cores.saturating_sub(kept)).max(1);
        // A slot's scratch folder its own past the first (a step's work in it, wiped by its next run).
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
        eprintln!(
            "agent: starting {}{} ({threads} threads: {} of {cores} cores, {kept} kept for the owner)",
            spec.id,
            if k > 0 { format!(" in slot {}", k + 1) } else { String::new() },
            match granted {
                Some(g) => format!("granted {g}"),
                None => format!("{wanted} wanted"),
            },
        );
        // The lead's jobs hand their saves off as every job does (no job writes the records,
        // docs/pool.md §7.3), note what their z6 tiles cost, and may offer tasks to other workers
        // through the coordinator; a member's hand theirs off likewise (SCENIC_HANDOFF, in their
        // command).
        let mut env: Vec<(String, String)> = Vec::new();
        if self.leads() {
            if let Some(Held::Pooled { dir, .. }) = &self.slots[k].lease {
                env.push(("SCENIC_HANDOFF".into(), dir.to_string_lossy().into_owned()));
            }
            env.push(("SCENIC_COSTS".into(), self.costs_path(k).to_string_lossy().into_owned()));
            if let Some(c) = &self.coord {
                env.push(("SCENIC_COORD".into(), format!("http://127.0.0.1:{}", c.port())));
                env.push(("SCENIC_COORD_TOKEN".into(), c.job_token.clone()));
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
                    store::whole::write(&f, &serde_json::to_vec(&self.unmerged_for(&step)?)?)?;
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
        // Where its processes count their NAS traffic, afresh (store::nasio).
        let nas_counts = self.nas_counts_path(k);
        std::fs::remove_dir_all(&nas_counts).ok();
        std::fs::create_dir_all(&nas_counts).ok();
        env.push((store::nasio::ENV.into(), nas_counts.to_string_lossy().into_owned()));
        // The owner's room target it keeps free past what it writes (the OSM pass, pass-sets).
        env.push(self.target_env(&spec));
        // A paced job's threads (an input check's, tree cover's), kept as cores free up
        // (`tend_threads`; crate::inputs::pace).
        if is_paced(&spec) {
            let f = self.threads_path(k);
            std::fs::write(&f, threads.to_string()).with_context(|| format!("write {}", f.display()))?;
            env.push((crate::inputs::pace::THREADS_ENV.into(), f.to_string_lossy().into_owned()));
        }
        // Where it says it waits on a worker holding its z6 tile's tail, while it does (`waiting_on`),
        // afresh.
        let waiting = self.waiting_path(k);
        std::fs::remove_file(&waiting).ok();
        env.push((crate::offload::WAITING_ENV.into(), waiting.to_string_lossy().into_owned()));
        // Where it reports NAS calls stuck busy (crate::agent::heal).
        env.push((store::whole::STUCK_ENV.into(), self.stuck_log().to_string_lossy().into_owned()));
        // Its connections out through the proxy (Wikimedia's metered by the pool's budget), this
        // Mac's own direct, and no other way out (the sandbox).
        let sandbox = match &self.proxy {
            Some(p) => {
                env.extend(p.env_for(&Self::net_tag(k)));
                Some(crate::budget::SANDBOX)
            }
            None => None,
        };
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (step, targets) =
            spec.record.as_ref().map_or((spec.id.split(' ').next().map(str::to_string), Vec::new()), |w| {
                (Some(w.step.clone()), w.targets.iter().map(|t| t.0.clone()).collect())
            });
        let what = spec.what.clone();
        // (Its costs file afresh: a line an earlier job left, a target begun and never ended, would
        // name a target under way that isn't, crate::agent::memguard.)
        std::fs::remove_file(self.slot_costs(k)).ok();
        self.slots[k].running =
            Some(Running::start(spec, threads, &env, log, &self.record_path(k), Some(&done), sandbox)?);
        // (Known to control and **jobs**' thread at once: a hold drains it, a cancel stops it, its
        // end wakes the loop. Control sets a hold before it reads the jobs to drain, and this reads
        // the hold after publishing: a hold taken up since `try_start_said`'s look is either
        // control's to drain or seen here, so the job is told "drain" before its first safe point.)
        self.publish_jobs();
        if self.hold().held.is_some() {
            std::fs::write(&control, b"drain").ok();
        }
        self.slots[k].beaten = None;
        self.slots[k].hand_saved = None;
        // (A job of no targets of its own, the OSM pass, GC, a catalogue: measured by its id, so it's
        // placed by measure as any, docs/pool.md §7.7: `watch_job`.)
        if let Some(pgid) = self.slots[k].running.as_ref().map(|r| r.pgid) {
            self.watch_job(k, pgid);
        }
        self.note(crate::coord::history::Event {
            worker: Some(self.worker_of(k)),
            step,
            targets,
            what,
            ..crate::coord::history::Event::new("start")
        });
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
        let (id, what) = self.slots[k]
            .running
            .as_ref()
            .map_or((String::new(), String::new()), |r| (r.spec.id.clone(), r.spec.what.clone()));
        let step = if step.is_empty() { id.split(' ').next().unwrap_or("").to_string() } else { step.to_string() };
        self.note(crate::coord::history::Event {
            worker: Some(self.worker_of(k)),
            step: (!step.is_empty()).then_some(step),
            targets: done.iter().map(|t| t.0.clone()).collect(),
            secs: Some(secs as f64),
            ok: Some(ok),
            what,
            note: how.to_string(),
            ..crate::coord::history::Event::new("end")
        });
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
                said.push(if c.ac {
                    "on mains power".into()
                } else {
                    format!("on battery{}", c.battery.map(|b| format!(" ({b}%)")).unwrap_or_default())
                });
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
            self.note(crate::coord::history::Event {
                worker: Some(self.host.clone()),
                note: said.join("; "),
                ..crate::coord::history::Event::new("conditions")
            });
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
            // (A gate check or forget asked for, done: the plan's to forget.)
            if id.starts_with("inputs ") {
                self.plan.send(plan::Msg::Done(id.to_string()));
            }
            if let Some(n) = redo::ask_of(id) {
                if let Err(e) = redo::finished(&self.o.home, n) {
                    eprintln!("agent: redo {n} done, but its ask stays: {e:#}");
                }
            }
        } else {
            let n = self.mem.retry.get(id).map(|r| r.0).unwrap_or(0) + 1;
            // 10 min, 20, 40 … up to 6 h.
            let delay = (600u64 << (n - 1).min(6)).min(6 * 3600);
            self.mem.retry.insert(id.to_string(), (n, now_s() + delay));
        }
        self.mem
            .recent
            .insert(0, Done { id: id.into(), what: what.into(), ok, ended: now_s(), secs, note, cancelled: false });
        self.mem.recent.truncate(20);
        self.save();
    }

    /// Whether this agent keeps the lead's record of its catalogues (and notes planned map tiles in
    /// its history): the one that runs its jobs, not a dry run beside it (planning only: its record
    /// is its own, in memory).
    fn keeps_publishing(&self) -> bool {
        self._lock.is_some() && !self.o.dry_run
    }

    /// What a target of `step` took here lately (`Memory::step_secs`); None for a PER_WORK step
    /// (crate::needs: its jobs' times mix in targets that found nothing to do, task #830).
    fn step_secs_here(&self, step: &str) -> Option<f64> {
        step_secs_here(&self.mem, step)
    }

    /// Slot `k`'s job under way, as its lane's time left is worked out (forecast::SlotJob): its
    /// targets and which are done, or with none listed (the pass's, a worldwide one) its step's
    /// time here (`step_secs`, else a first guess); how long it has run; its own estimate.
    fn slot_job(&self, k: usize) -> Option<forecast::SlotJob> {
        let r = self.slots.get(k)?.running.as_ref()?;
        let step = r.spec.id.split(' ').next().unwrap_or("").to_string();
        let (targets, alone_s) = match r.spec.record.as_ref() {
            Some(w) => {
                let done = crate::control::read_done(&self.done_path(k), &w.step);
                (w.targets.iter().map(|(t, _)| (t.clone(), done.contains(t))).collect(), None)
            }
            None => (Vec::new(), Some(self.step_secs_here(&step).unwrap_or_else(|| first_secs(&step)))),
        };
        let step = r.spec.record.as_ref().map_or(step, |w| w.step.clone());
        Some(forecast::SlotJob {
            step,
            targets,
            alone_s,
            elapsed_s: r.elapsed().as_secs_f64(),
            eta: self.slots[k].job_eta.map(|e| e as f64),
        })
    }

    /// `keys` as every reader reads them (agent::rekey::as_read), for a job's `--expect-same`, by
    /// the coverage and terrain packs' indexes the last plan read (`plan::PlanView`); as they are
    /// when what that reads can't be told now (no plan yet, or the regions' version changed since).
    fn as_read(&self, root: &store::nasfs::NasRoot, mut keys: build::Keys) -> build::Keys {
        // (The plan's first: a member has none, and reads nothing for it.)
        let view = self.plan.view.get();
        let (Some((version, covs)), Some(tiles)) = (view.coverage.as_ref(), view.tiles.as_ref()) else { return keys };
        if self.latest_pass(root).is_none() {
            return keys;
        }
        let (recipes, unread) = load_regions(root);
        if recipes.is_empty() || !unread.is_empty() {
            return keys;
        }
        let Ok(manifest) = crate::out::read_manifest(root) else { return keys };
        let now = crate::inputs::regions::read::source(root, &manifest).map(|a| a.version);
        if now.ok().as_ref() != Some(version) {
            return keys;
        }
        let times = rekey::FileTimes::new(root);
        rekey::as_read(&mut keys, &covs.all, &manifest, &tiles, &times);
        keys
    }

    /// The build's pause as this agent knows it, brought up to date: this Mac's ask (its menu,
    /// `scenic pause`, the map: crate::control::Request) passed on, to this Mac's coordinator, or
    /// from a member to the lead's (and this Mac paused or going on meanwhile, while that can't
    /// be reached); on the lead, then, its coordinator's, mirrored to the NAS
    /// (`state/build/pause.json`, for whoever reads the build's state there). A helper hears the
    /// build's otherwise in its asks and beats.
    fn sync_pause(&mut self, root: Option<&store::nasfs::NasRoot>, waiting: &mut Vec<Waiting>) {
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
                } else if self.is_member() {
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
                            waiting.push(Waiting {
                                step: None,
                                what: "Pausing".into(),
                                why: format!("this Mac's ask isn't with the lead yet ({e:#}); it holds here meanwhile"),
                            });
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
                    Some(x) => serde_json::to_vec(x).map_err(anyhow::Error::from).and_then(|b| path.write_whole(b)),
                    None => path
                        .remove_file()
                        .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
                        .map_err(anyhow::Error::from),
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
            Some(x) => serde_json::to_vec(x).map_err(anyhow::Error::from).and_then(|b| store::whole::write(&path, &b)),
            None => std::fs::remove_file(&path)
                .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
                .map_err(anyhow::Error::from),
        };
        if let Err(e) = kept {
            eprintln!("agent: keeping the pause: {e:#}");
        }
        self.pause = p;
    }

    /// Where slot `k`'s paced job is told the threads it may use now (`tend_threads`).
    fn threads_path(&self, k: usize) -> PathBuf {
        self.o.home.join(format!("threads{}", slot_suffix(k)))
    }

    /// The memory free on this Mac (MB), as last read (at most every 10 s: reading it spawns a program).
    fn free_mem_mb(&self) -> Option<u64> {
        static LAST: std::sync::Mutex<Option<(Instant, Option<u64>)>> = std::sync::Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, v)) = *last {
            if t.elapsed() < Duration::from_secs(10) {
                return v;
            }
        }
        let v = self.mac.resources(&self.o.home, None, None, None).mem_free().map(|b| b >> 20);
        *last = Some((Instant::now(), v));
        v
    }

    /// Tells slot `k`'s paced job (`is_paced`) the threads it may use now (docs/inputs.md §4.3, Threads): this
    /// Mac's cores less those kept for its owner and those its other jobs were given, `MOST` at
    /// most. It takes more as they free up, and fewer when a job starts beside it.
    fn tend_threads(&self, k: usize, c: &Conditions) {
        let Some(r) = self.slots[k].running.as_ref().filter(|r| is_paced(&r.spec)) else {
            return;
        };
        let cores = mac_cores() as usize;
        let kept = if c.user_active() { share::cores(self.in_use_share.as_ref(), cores as u32) as usize } else { 0 };
        let beside: usize = self
            .slots
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .filter_map(|(j, s)| Some((j, s.running.as_ref()?)))
            // (A paced job beside it at what it's allowed now: so two don't widen into the same cores.)
            .map(|(j, b)| {
                let now = || std::fs::read_to_string(self.threads_path(j)).ok()?.trim().parse::<usize>().ok();
                if is_paced(&b.spec) {
                    now().unwrap_or(0).max(b.threads)
                } else {
                    b.threads
                }
            })
            .sum();
        // Past its grant only while the memory free covers its measured peak per thread for each
        // extra (the free memory read at most every 10 s: it asks the system). A regions check once
        // took 39 GB; with no peak measured yet it stays at its grant.
        // Each extra thread counted at the peak over the threads that peak was measured at (the whole
        // peak when unknown), after the growth the check has still to come (its peak less what it
        // holds now) is taken out of the free memory: `pace::allowed_now`.
        use crate::inputs::pace::{allowed_now, Memory};
        let peak = self.measured_job(&r.spec).map(|m| m.mem_mb);
        let width = r
            .spec
            .record
            .as_ref()
            .and_then(|w| Some((w.step.clone(), w.targets.first()?.0.clone())))
            .and_then(|(s, t)| self.coord.as_ref()?.peak_threads(&s, &t))
            .map(|t| t as usize);
        let held = self.held_mb(k).unwrap_or(0);
        let memory = |free_mb| peak.map(|peak_mb| Memory { peak_mb, width, held_mb: held, free_mb });
        // (Free memory read only when cores allow a widening at all.)
        let wide = allowed_now(cores, kept, beside, r.threads, memory(u64::MAX >> 20)) > r.threads;
        let free = if wide { self.free_mem_mb() } else { None };
        let now = allowed_now(cores, kept, beside, r.threads, free.and_then(memory)).to_string();
        let f = self.threads_path(k);
        if std::fs::read_to_string(&f).ok().as_deref() != Some(now.as_str()) {
            if let Err(e) = std::fs::write(&f, &now) {
                eprintln!("agent: telling {} its threads: {e}", r.spec.id);
            }
        }
    }

    /// Slot `k`'s job's channel (crate::control: "run" or "drain").
    fn control_path(&self, k: usize) -> PathBuf {
        self.o.home.join(format!("control{}", slot_suffix(k)))
    }

    /// Whether slot `k`'s job's channel says "drain" (told by control, crate::agent::asks, maybe before this
    /// loop timed it).
    fn told_drain(&self, k: usize) -> bool {
        std::fs::read(self.control_path(k)).is_ok_and(|b| b.starts_with(b"drain"))
    }

    /// Where slot `k`'s job notes the targets it finishes: a helper's leased job's in its lease's
    /// outbox folder (kept with its saves, should the agent stop), else in the agent's folder.
    fn done_path(&self, k: usize) -> PathBuf {
        match &self.slots[k].lease {
            Some(Held::Leased { dir, .. } | Held::Pooled { dir, .. }) => dir.join("done.txt"),
            _ => self.o.home.join(format!("done{}.txt", slot_suffix(k))),
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
        let step = self.slots[k]
            .running
            .as_ref()
            .and_then(|r| r.spec.record.as_ref().map(|w| w.step.clone()))
            .unwrap_or_default();
        let secs = self.slots[k].running.as_ref().map_or(0, |r| r.elapsed().as_secs());
        if let Some(id) = self.slots[k].running.as_ref().map(|r| r.spec.id.clone()) {
            self.worked(&id);
            self.save();
        }
        self.note_end(k, &step, &done, secs, false, why);
        self.end_lease(k, Outcome::Interrupted, &done, why);
    }

    /// The owner's cancels (crate::control::Cancel), each loop: the asks in this agent's folder taken
    /// up (`take_cancel`), the lead's word for this Mac's own leases (its coordinator's here, a
    /// member's in its beat's answer, `beat`), and each slot so told stopped (`cancel_slot`); the
    /// targets held for a newer app said in `waiting`. True when a job ended.
    fn tend_cancels(&mut self, root: Option<&store::nasfs::NasRoot>, waiting: &mut Vec<Waiting>) -> bool {
        let mut ended = self.take_cancels(root);
        if let Some(c) = &self.coord {
            for s in self.slots.iter_mut().filter(|s| s.running.is_some() && s.cancel.is_none()) {
                if let Some(Held::Own(id) | Held::Pooled { id, .. }) = &s.lease {
                    s.cancel = c.cancel_of(*id);
                }
            }
        }
        ended |= self.cancel_slots();
        if let Some(c) = &self.coord {
            for h in c.holds() {
                waiting.push(Waiting {
                    step: Some(h.step.clone()),
                    what: format!("{} {}", h.step, h.target),
                    why: format!("cancelled by {}: held until the lead runs an app newer than {}", h.by, h.newer_than),
                });
            }
        }
        ended
    }

    /// The owner's cancels **jobs**' thread took (their jobs already told to stop: crate::agent::slots),
    /// or, while it doesn't run, those waiting in the folder: each taken up (`take_cancel`) and its
    /// slots stopped as cancelled; whether one ended.
    fn take_cancels(&mut self, root: Option<&store::nasfs::NasRoot>) -> bool {
        if self._lock.is_none() || self.o.dry_run {
            return false;
        }
        let asks = self.jobs_part.cancels();
        if asks.is_empty() {
            return false;
        }
        for ask in asks {
            self.take_cancel(ask, root);
        }
        self.cancel_slots()
    }

    /// The slots marked cancelled stopped and given back (`cancel_slot`); whether one ended. A job
    /// ended already is left to `tend`, which records how it ended, unless it ended killed: the
    /// cancel's own stop (**jobs**' thread signals it the moment it takes the ask).
    fn cancel_slots(&mut self) -> bool {
        let mut ended = false;
        for k in 0..self.slots.len() {
            if let Some(c) = self.slots[k].cancel.take() {
                let exit = self.slots[k].running.as_mut().map(|r| r.poll().ok().flatten());
                let cancel = match exit {
                    Some(None) => true,
                    Some(Some(st)) => !st.success(),
                    None => false,
                };
                if cancel {
                    self.cancel_slot(k, &c);
                    ended = true;
                }
            }
        }
        ended
    }

    /// Why `spec` doesn't start: one of its targets the owner cancelled to rerun on a newer app
    /// (crate::coord::lease::Hold), while the lead runs the app it was cancelled on.
    fn held_why(&self, spec: &JobSpec) -> Option<String> {
        let w = spec.record.as_ref()?;
        let holds = self.coord.as_ref()?.holds();
        let h = holds.iter().find(|h| h.step == w.step && w.targets.iter().any(|t| t.0 == h.target))?;
        Some(format!(
            "{} was cancelled by {}: held until the lead runs an app newer than {}",
            h.target, h.by, h.newer_than
        ))
    }

    /// An owner's cancel taken up: the slots running its target marked (stopped by `tend_cancels`),
    /// the lead told (this Mac's coordinator, or the lead's by HTTP: its leases elsewhere told, the
    /// target held for a newer app when asked), and the answer written for `scenic job cancel`.
    fn take_cancel(&mut self, ask: crate::control::Cancel, root: Option<&store::nasfs::NasRoot>) {
        // (A grant from a plan no newer than this is dropped: `cancelled_since`.)
        let n = self.plan_n.max(self.plan.newest_n());
        self.cancels_at.insert(ask.target.clone(), n);
        let mut said: Vec<String> = Vec::new();
        let mut here: Vec<(String, u64)> = Vec::new();
        for k in 0..self.slots.len() {
            let Some(r) = self.slots[k].running.as_ref() else { continue };
            let hit = r.spec.record.as_ref().is_some_and(|w| {
                ask.step.as_ref().is_none_or(|s| *s == w.step) && w.targets.iter().any(|t| t.0 == ask.target)
            });
            if hit {
                let step = r.spec.record.as_ref().map(|w| w.step.clone()).unwrap_or_default();
                let lease = match &self.slots[k].lease {
                    Some(Held::Own(id) | Held::Pooled { id, .. }) => *id,
                    Some(Held::Leased { lease, .. }) => *lease,
                    None => 0,
                };
                said.push(format!("{} on this Mac", r.spec.id));
                here.push((step, lease));
                self.slots[k].cancel = Some(ask.clone());
            }
        }
        let elsewhere = |told: Vec<(u64, String, String)>, said: &mut Vec<String>| {
            for (id, w, what) in told {
                if !here.iter().any(|(_, l)| *l == id) {
                    said.push(format!("{what} on {w} (told in its next beat, within a minute)"));
                }
            }
        };
        if let Some(c) = &self.coord {
            elsewhere(c.cancel(&ask), &mut said);
            // (A job of no lease here: its target held all the same.)
            if ask.rerun_after_update {
                for (step, _) in here.iter().filter(|(_, l)| *l == 0) {
                    c.hold_for_update(step, &ask.target, &ask.by);
                }
            }
        } else if let Some(r) = root {
            match self.client(r, &mut Vec::new()).map(|c| c.cancel(&ask)) {
                Some(Ok(told)) => elsewhere(told, &mut said),
                Some(Err(e)) => said.push(format!("the lead couldn't be told ({e:#})")),
                None => said.push("the lead couldn't be reached".into()),
            }
        } else if ask.rerun_after_update {
            said.push("the lead couldn't be told without the NAS: it may rerun on this app".into());
        }
        let hit = !here.is_empty() || said.iter().any(|s| s.contains("told in its next beat"));
        let said = if hit {
            format!("cancelled {}: {}", said.join("; "), ask.said())
        } else if said.is_empty() {
            format!("nothing builds {} now", ask.target)
        } else {
            format!("nothing builds {} now ({})", ask.target, said.join("; "))
        };
        eprintln!("agent: {said}");
        crate::control::answer_cancel(&self.o.home, &crate::control::CancelAnswer { ask, said, hit });
    }

    /// Slot `k`'s job cancelled by the owner (`c`): stopped now and given back, not failed (no retry
    /// delay, nothing kept from this Mac), what it finished handed off, and its work under way
    /// dropped (its scratch for the target cleared); noted in the history and the status.
    fn cancel_slot(&mut self, k: usize, c: &crate::control::Cancel) {
        let Some(r) = self.slots[k].running.as_mut() else { return };
        eprintln!("agent: cancelling {} ({})", r.spec.id, c.said());
        r.stop(Duration::from_secs(30));
        let (id, what) = (r.spec.id.clone(), r.spec.what.clone());
        let scratch = r.spec.cmd.windows(2).find(|w| w[0] == "--scratch").map(|w| PathBuf::from(&w[1]));
        let secs = r.elapsed().as_secs();
        let step = r.spec.record.as_ref().map(|w| w.step.clone()).unwrap_or_default();
        let done = self.finished_targets(k, false);
        // (Not the guard's stop, nor a yield: a cancel keeps nothing from this Mac.)
        (self.slots[k].guard_stopped, self.slots[k].yield_stopped) = (false, false);
        self.worked(&id);
        self.note(crate::coord::history::Event {
            worker: Some(self.worker_of(k)),
            step: (!step.is_empty()).then(|| step.clone()),
            targets: vec![c.target.clone()],
            secs: Some(secs as f64),
            what: what.clone(),
            note: c.said(),
            ..crate::coord::history::Event::new("cancel")
        });
        let note = c.said();
        self.end_lease(k, Outcome::Interrupted, &done, &note);
        if let Some(dir) = scratch {
            for p in clear_target_scratch(&dir, &c.target) {
                eprintln!("agent: {id}: its work under way dropped ({})", p.display());
            }
        }
        self.mem.retry.remove(&id);
        self.mem.recent.insert(0, Done { id, what, ok: false, ended: now_s(), secs, note, cancelled: true });
        self.mem.recent.truncate(20);
        self.save();
        let s = &mut self.slots[k];
        (s.drain_since, s.mem_drain, s.mem_drain_alone, s.update_wait) = (None, None, false, None);
        s.running = None;
        s.yields = None;
        std::fs::remove_file(self.record_path(k)).ok();
    }

    /// Why this Mac isn't in the pool, for the status (`Status::out_of_pool`): None in the pool, or
    /// in a process that never joins (a dry run, another agent's lock held).
    fn out_of_pool(&self) -> Option<String> {
        if self.pool.is_some() || self._lock.is_none() || self.o.dry_run {
            return None;
        }
        Some(match &self.restart_for {
            Some(_) => "its member's lock is free: restarting into the pool now".into(),
            None => self.out_why.clone(),
        })
    }

    /// Why this process restarts now (`pool_restart`): once its first job's slot is free; out of
    /// the pool, its member's lock free now, or into another role, at once, its jobs left running
    /// for the next agent (`hand_over_jobs`), which takes them over as it starts: waiting for a
    /// job's end would leave this Mac out of the pool, or the pool without its lead, for as long as
    /// the job runs.
    fn restart_now(&self) -> Option<String> {
        let joining = self.pool.is_none() && self.restart_for.is_some();
        let at_once = joining || self.role_restart.is_some();
        self.pool_restart().filter(|_| at_once || self.slots[0].running.is_none())
    }

    /// Why this process restarts (the pool's switch changed, its role, or its term); None: it goes on.
    fn pool_restart(&self) -> Option<String> {
        self.restart_for
            .clone()
            .or_else(|| self.role_restart.clone())
            .or_else(|| self.pool_seen.as_ref()?.restart.clone())
    }

    /// The budgets' proxy: started once (taking from the pool's budget on the NAS's project folder
    /// `root`, as this agent); told the members this Mac knows (its share of the rate with the NAS
    /// away).
    fn tend_proxy(&mut self, root: &store::nasfs::NasRoot) {
        if self.proxy.is_none() {
            let me = self.member_id.clone().unwrap_or_else(|| self.me.clone());
            // (On the port the jobs taken over were told, while it can be bound: `proxy_port`.)
            use crate::budget::listen;
            let listener = self.proxy_port.and_then(|p| listen(p).ok()).map_or_else(|| listen(0), Ok);
            match listener.map_err(anyhow::Error::from).and_then(|l| crate::budget::start_proxy(root, &me, l)) {
                Ok(p) => {
                    eprintln!("agent: the budgets' proxy listens on {}", p.url());
                    self.proxy = Some(p);
                }
                Err(e) => eprintln!("agent: the budgets' proxy didn't start ({e:#}); no job starts until it has"),
            }
        }
        // (Not on the port the jobs taken over were told, it taken meanwhile: they can't reach out,
        // so they're stopped, interrupted, their targets given back.)
        if let (Some(p), Some(t)) = (&self.proxy, self.proxy_port) {
            self.proxy_port = None;
            if p.port() != t {
                self.stop_adopted("its proxy's port was taken: its connections can't go out");
            }
        }
        let n = if self.pool.is_some() { PoolView::heard_of(self.pool_seen.as_ref()) } else { 1 };
        if let Some(p) = &self.proxy {
            p.set_members(n);
        }
    }

    /// Resuming (crate::agent::resume, `state/pool/resume`) acts: switched on, in the pool. Off, or
    /// out of the pool, the agent is as it was.
    fn resume_on(&self) -> bool {
        self.resume && self.pool.is_some()
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
        let view = self.plan.view.get();
        let mut p = view.forecast.as_ref().map(placement::Placement::from_forecast).unwrap_or_default();
        if let Some(c) = &self.coord {
            let names: Vec<String> = c.workers().into_iter().map(|(n, _)| n).filter(|n| *n != self.host).collect();
            let events = c.for_forecast().2;
            p.speed = placement::measured_speeds(&events, &self.host, &names);
            let key = placement::step_speeds_key(&events, &self.host, &names);
            let mut kept = self.step_speeds.borrow_mut();
            if kept.as_ref().is_none_or(|(k, _)| *k != key) {
                *kept = Some((key, placement::step_speeds(&events, &self.host, &names)));
            }
            p.step_speed = kept.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
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
            // (The pool's run, the pool's part until #878.)
            if let Some(Some(mut run)) = self.pool.as_ref().map(pool::try_lock) {
                run.cx.broker = None;
                run.side.broker = None;
            }
            return;
        }
        let now = Instant::now();
        // A member's broker, for its own jobs' tasks (tried again five minutes after it failed; not
        // while a tick holds the run: the next loop).
        if self.coord.is_none()
            && self.broker.is_none()
            && self.is_member()
            && !self.o.dry_run
            && self._lock.is_some()
            && brokers::stale(self.broker_failed, now, brokers::HTTPS_EVERY)
        {
            self.stop_viewer();
            let tried = match self.pool.as_ref().map(pool::try_lock) {
                Some(Some(run)) => Some(make_coordinator_on(&self.o, Some(&run), &self.app, &self.host, broker_port())),
                _ => None,
            };
            if let Some(made) = tried {
                match made {
                    Some(b) => {
                        b.become_broker();
                        eprintln!(
                            "agent: this Mac's broker started at {}: its jobs' tasks are offered there",
                            b.contact.urls.join(", ")
                        );
                        self.broker = Some(std::sync::Arc::new(b));
                    }
                    None => self.broker_failed = Some(now),
                }
            }
        }
        let Some(c) = self.coord.as_ref().or(self.broker.as_ref()) else { return };
        if let Some(b) = &self.broker {
            for l in b.expire() {
                eprintln!(
                    "agent: {}'s lease of {} lapsed at this Mac's broker (no word for {} min); offered again",
                    l.worker,
                    l.what(),
                    crate::coord::TTL.as_secs() / 60
                );
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

    /// The live store (crate::agent::live), each loop: answered on this Mac's port (its coordinator,
    /// broker or viewer), and the NAS's latency sampled while a build runs here (a job of this
    /// Mac's, or its coordinator's leases out); the power and the owner's idleness as this loop
    /// read them (`c`), recorded.
    fn tend_live(&mut self, root: Option<&store::nasfs::NasRoot>, c: &Conditions) {
        let Some(l) = &self.live else { return };
        let t = now_s();
        l.record(live::AC, t, if c.ac { 1.0 } else { 0.0 });
        if let Some(b) = c.battery {
            l.record(live::BATTERY, t, f64::from(b));
        }
        if c.idle_s != u64::MAX {
            l.record(live::IDLE, t, c.idle_s as f64);
        }
        // (The cores the build may use now: all, less those kept for the owner while they use the Mac.)
        // (As the lead counts them for this Mac's jobs together: coord::sched::ask_cores.)
        let cores = mac_cores();
        let kept = if c.user_active() { share::cores(self.in_use_share.as_ref(), cores) } else { 0 };
        l.record_limits(t, cores.saturating_sub(kept).max(1), self.build_room(), self.floor(), self.reserved(None));
        if !l.has_net() {
            if let Some(n) = self.proxy.as_ref().and_then(|p| p.net()) {
                l.set_net(n);
            }
        }
        for c in [&self.coord, &self.broker, &self.viewer].into_iter().flatten() {
            if !c.has_live() {
                c.set_live(l.clone());
            }
        }
        let building = self.running_jobs() > 0 || self.coord.as_ref().is_some_and(|c| c.leases_out() > 0);
        l.set_nas(root.filter(|_| building));
        l.set_root(root);
    }

    /// The page on this Mac's port (crate::agent::pageview), each loop: a viewer alone started when
    /// neither a coordinator nor a broker answers there; each serving it from this Mac's read of the
    /// NAS; that read told the term, the members, this app and this Mac's lanes now. In the pool
    /// alone (out of it, the coordinator answers from its memory, as before).
    fn tend_pageview(&mut self, root: &store::nasfs::NasRoot) {
        if self.o.dry_run {
            return;
        }
        // (The Catalogues map, in the pool or out of it: the map's log followed, the lead's in
        // process, else its routes; asked each loop, so it misses no batch.)
        let lead = self.keeps_publishing();
        let source: std::sync::Arc<dyn cataloguesview::Source> = if lead {
            self.maplog.here(&self.o.home).feed.clone()
        } else {
            std::sync::Arc::new(cataloguesview::Lead::new(root, &self.host))
        };
        let keep = self.o.home.join(cataloguesview::KEPT);
        let cv = self
            .catalogues_view
            .get_or_insert_with(|| cataloguesview::CataloguesView::new(source.clone(), Some(keep)))
            .clone();
        if self.catalogues_lead != Some(lead) {
            cv.set_source(source);
            self.catalogues_lead = Some(lead);
        }
        cv.wake();
        for c in [&self.coord, &self.broker, &self.viewer].into_iter().flatten() {
            if !c.has_catalogues() {
                c.set_catalogues(cv.clone());
            }
        }
        // (The map's log, which the servers follow: the lead's coordinator answers it.)
        if lead {
            let here = self.maplog.here(&self.o.home);
            if let Some(c) = self.coord.as_ref().filter(|c| !c.has_maplog()) {
                c.set_maplog(here.feed.clone());
            }
            // (Each map server's adoption of a newer map, its cursor moved: in the history, for
            // publishing's times, crate::pubtimes.)
            for (host, s) in adopted(&here.feed.cursors()) {
                if self.adoptions.get(&host) != Some(&s.n) {
                    let e = crate::coord::history::Event {
                        t: s.at,
                        worker: Some(host.clone()),
                        n: Some(s.n),
                        ..crate::coord::history::Event::new(crate::pubtimes::ADOPT)
                    };
                    self.note(e);
                    self.adoptions.insert(host, s.n);
                }
            }
        }
        // (The map's finished tiles, the same: the build's keys on the NAS.)
        let bv = self.built_view.get_or_insert_with(|| builtview::BuiltView::new(root, Some(cv.clone()))).clone();
        for c in [&self.coord, &self.broker, &self.viewer].into_iter().flatten() {
            if !c.has_built() {
                c.set_built(bv.clone());
            }
        }
        if self._lock.is_none() {
            return;
        }
        let now = Instant::now();
        if self.coord.is_none()
            && self.broker.is_none()
            && self.viewer.is_none()
            && brokers::stale(self.viewer_failed, now, brokers::HTTPS_EVERY)
        {
            let made = self.pool.is_some().then(|| make_viewer(&self.app, &self.host));
            match made {
                Some(Some(v)) => {
                    eprintln!("agent: this Mac shows the build page at {}", v.contact.urls.join(", "));
                    self.viewer = Some(std::sync::Arc::new(v));
                }
                Some(None) => self.viewer_failed = Some(now),
                None => {}
            }
        }
        for c in [&self.coord, &self.broker, &self.viewer].into_iter().flatten() {
            if !c.has_pageview() {
                c.set_pageview(self.pageview.clone());
            }
        }
        // (A page's pause on a Mac not leading: this agent's to pass on, as its menu's.)
        let lead = self.coord.is_some();
        for c in [&self.broker, &self.viewer].into_iter().flatten() {
            c.set_forward((!lead).then(|| self.o.home.clone()));
        }
        if let Some(c) = &self.coord {
            c.set_forward(None);
        }
        // The pages' cancel and retry asks: to the pool's step (acted on leading, else passed on
        // to the lead: crate::pool::driver::Act); out of the pool, acted on here.
        // (While a tick holds the run: kept for the next loop.)
        let mut acts = std::mem::take(&mut self.page_acts);
        acts.extend([&self.coord, &self.broker, &self.viewer].into_iter().flatten().flat_map(|c| c.take_page_acts()));
        // (A retry: this Mac's own back-off for the target forgotten at once, a member's too; the
        // lead forgets its own and the coordinator's.)
        for a in &acts {
            if let crate::pool::driver::Act::Retry(r) = a {
                self.guard_backoff.retain(|id, _| !id.split(' ').any(|w| w == r.target));
            }
        }
        // (Why a member's retry can't be mailed to the lead now: said to its page at once.)
        if !lead {
            // (Not known now, a tick holding the run: the last word kept, never cleared.)
            let seen = match self.pool.as_ref().map(pool::try_lock) {
                Some(Some(run)) => Some((
                    run.side.driver().current().lead.as_ref().map(|t| t.app.clone()),
                    run.side.member().app.clone(),
                )),
                _ => None,
            };
            if let Some(blocked) = retry_blocked(seen.as_ref().map(|(l, me)| (l.as_deref(), me.as_str()))) {
                for c in [&self.broker, &self.viewer].into_iter().flatten() {
                    c.set_retry_blocked(blocked.clone());
                }
            }
        }
        if !acts.is_empty() {
            let alone = match self.pool.as_ref().map(pool::try_lock) {
                Some(Some(mut run)) => {
                    run.asks.extend(acts.drain(..).map(crate::pool::driver::Ask::Act));
                    false
                }
                Some(None) => false,
                None => true,
            };
            if alone {
                self.act_on(&acts);
            } else {
                self.page_acts = acts;
            }
        }
        // (The members' ids and the term, as the run holds them: no NAS read here. Their hosts are
        // the pageview's to read, as it reads, while a page asks.)
        let Some(p) = self.pool_seen.as_ref().filter(|_| self.pool.is_some()) else { return };
        let members: Vec<String> = p.members.clone();
        let term = p.gates.term;
        // (This Mac's lanes, only while a page asks: each slot's done targets are a read.)
        let own = if self.pageview.wanted(now_s()) {
            (0..self.slots.len())
                .map(|k| pageview::OwnLane { name: self.worker_of(k), job: self.slot_job(k) })
                .collect()
        } else {
            Vec::new()
        };
        self.pageview.set_inputs(pageview::Inputs {
            root: Some(root.clone()),
            term,
            members,
            app: self.app.clone(),
            own,
            keep: Some(self.o.home.join("pageview-waits.json")),
        });
        if !self.pageview_reading {
            pageview::spawn(self.pageview.clone());
            self.pageview_reading = true;
        }
    }

    /// The build pages' asks the lead acts on (crate::pool::driver::Act): a cancel, as the owner's
    /// `scenic job cancel` (a file in its folder: crate::control::request_cancel, taken up as its
    /// own); a retry, its target's failures forgotten (the coordinator's, and this agent's own
    /// back-off), so it's offered and run again now.
    fn act_on(&mut self, acts: &[crate::pool::driver::Act]) {
        for a in acts {
            match a {
                crate::pool::driver::Act::Cancel(c) => {
                    eprintln!("agent: {} asks to cancel {}", c.by, c.target);
                    if let Err(e) = crate::control::request_cancel(&self.o.home, c) {
                        eprintln!("agent: the page's cancel of {}: {e:#}", c.target);
                    }
                }
                crate::pool::driver::Act::Retry(r) => {
                    let n = self.coord.as_ref().map_or(0, |c| c.clear_failed(r.step.as_deref(), &r.target));
                    let before = self.guard_backoff.len();
                    self.guard_backoff.retain(|id, _| !id.split(' ').any(|w| w == r.target));
                    eprintln!(
                        "agent: {} asks to try {} again: {n} failure{} and {} back-off{} forgotten",
                        r.by,
                        r.target,
                        if n == 1 { "" } else { "s" },
                        before - self.guard_backoff.len(),
                        if before - self.guard_backoff.len() == 1 { "" } else { "s" }
                    );
                }
            }
        }
    }

    /// This Mac's viewer alone stopped (its port then free for a coordinator or a broker).
    fn stop_viewer(&mut self) {
        if let Some(v) = self.viewer.take() {
            v.stop();
        }
    }

    /// The coordinator brokering this Mac's tasks, for the run's steps (`Cx::broker`): the lead's,
    /// or a member's broker; None with tasks brokered by every member off.
    fn brokering(&self) -> Option<std::sync::Arc<crate::coord::Coordinator>> {
        self.brokers_on().then(|| self.coord.clone().or_else(|| self.broker.clone())).flatten()
    }

    /// The newest complete pass: by the merged records with resuming on (docs/pool.md §7.4), else
    /// by the NAS's `pass.*.json` files, as before.
    fn latest_pass(&self, root: &store::nasfs::NasRoot) -> Option<String> {
        crate::osmpass::latest_pass_by(root, self.resume_on())
    }

    /// This member's clock, as its pool's steps read it (unix seconds).
    fn resume_now(&self) -> u64 {
        // (The pool's run, the pool's part until #878.)
        match self.pool.as_ref().map(pool::try_lock) {
            Some(Some(p)) => p.side.now(),
            _ => now_s(),
        }
    }

    /// This member's progress file: as this process last wrote it, else read from the NAS (once,
    /// or again after a read failed). None out of the pool, or when it can't be read now.
    fn own_progress(&mut self, root: &store::nasfs::NasRoot) -> Option<resume::Progress> {
        if self.held_progress.is_none() {
            let me = self.member_id.clone()?;
            match resume::read(root, &me) {
                Ok(p) => {
                    self.held_progress = Some(p.unwrap_or(resume::Progress {
                        member: me,
                        worker: self.host.clone(),
                        jobs: BTreeMap::new(),
                    }))
                }
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
    fn note_progress(&mut self, root: &store::nasfs::NasRoot, job: &str, held: Option<resume::Held>) {
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
            Err(e) => eprintln!(
                "agent: this Mac's progress of {job} not noted on the NAS ({e:#}): the lead may run it elsewhere"
            ),
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
    fn tend_progress(&mut self, root: &store::nasfs::NasRoot) {
        if !self.resume_on() {
            return;
        }
        let Some(p) = self.own_progress(root) else { return };
        if p.jobs.is_empty() {
            return;
        }
        let now = self.resume_now();
        let running: BTreeSet<String> =
            self.slots.iter().filter_map(|s| s.running.as_ref()).map(|r| r.spec.id.clone()).collect();
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
            let complete =
                h.done && job.strip_prefix("osm-pass ").is_some_and(|d| latest.as_deref().is_some_and(|l| l >= d));
            let released = resume::released(root, &p.member, job, h.began());
            if complete || !resume::live(h, now, released) {
                let why = if complete {
                    "its pass is in the records"
                } else if released {
                    "the owner released it"
                } else {
                    "a day since its job last ran here"
                };
                eprintln!("agent: {job}'s progress here no longer held for this Mac ({why})");
                self.note_progress(root, job, None);
            }
        }
    }

    /// The OSM pass's command for the planet `planet` of `date`, in this Mac's scratch folder of it
    /// (where its stages' markers are: crate::osmpass), the lead's own and a member's resumed alike.
    fn osm_pass_cmd(&self, root: &store::nasfs::NasRoot, planet: &NasPath, date: &str) -> Vec<String> {
        osm_pass_cmd(&self.o, root, planet, date)
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
            anyhow::ensure!(
                since.elapsed() < Duration::from_secs(30),
                "the pool's step holds its state (its share slow to answer)"
            );
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
    fn pool_switch(&mut self, root: Option<&store::nasfs::NasRoot>) {
        let Some(r) = root.filter(|_| self._lock.is_some() && !self.o.dry_run) else { return };
        self.tend_proxy(r);
        if let Some(on) = resume::on(r) {
            if on != self.resume {
                eprintln!(
                    "agent: resuming on its Mac, and a pass complete by the records, is {}",
                    if on { "on" } else { "off" }
                );
            }
            self.resume = on;
        }
        self.tend_progress(r);
        if let Some(on) = placement::on(r) {
            if on != self.placement {
                eprintln!(
                    "agent: placement by how crucial a job is, and work offered ahead, is {}",
                    if on { "on" } else { "off" }
                );
            }
            self.placement = on;
        }
        if let Some(on) = brokers::on(r) {
            if on != self.brokers {
                eprintln!(
                    "agent: tasks brokered by every member are {}",
                    if on {
                        "on (this Mac's jobs' tasks its own broker's; pages from the members' addresses answered)"
                    } else {
                        "off"
                    }
                );
            }
            self.brokers = on;
        }
        self.tend_brokers();
        self.tend_pageview(r);
        // (The lead's coordinator told this Mac's capacity each loop.)
        // (Placement worked out once a loop: the history read once.)
        let placed = self.placement_now();
        if let Some(c) = &self.coord {
            c.set_capacity(self.last_cond.map(|cond| self.capacity(&cond)));
            c.set_placement(placed.clone());
        }
        // (A member's broker draws its pages' tasks the same way: by their paces at the kind as
        // that broker measured them, its own jobs' tasks against this Mac's runs. It has no
        // history and no forecast, so no member speeds nor slack: those only order the plan's
        // targets, which a broker doesn't offer.)
        if let Some(b) = &self.broker {
            b.set_placement(placed);
        }
        // Not in the pool (the NAS not reached as this process started, or its member's lock held
        // by another process): once the lock can be taken with the NAS answering, this process
        // restarts into the pool (its part, its coordinator).
        if self.pool.is_none() && self.restart_for.is_none() {
            let locks = self.o.home.parent().unwrap_or(&self.o.home).to_path_buf();
            if let Ok(id) = crate::pool::member_id(&self.o.home) {
                match crate::pool::MemberLock::take(&locks, &id) {
                    Ok(Some(_lock)) => {
                        let why = "the NAS answers and this Mac's member's lock is free: into the pool".to_string();
                        eprintln!("agent: {why}");
                        self.restart_for = Some(why);
                    }
                    Ok(None) => self.out_why = LOCK_HELD.into(),
                    Err(_) => {}
                }
            }
        }
    }

    /// The pool's step while it's on (docs/pool.md §12): the hand-offs from before the pool drained
    /// into the journal, the driver's step; then, leading, its
    /// coordinator told the term and whether it grants, and the files the lead keeps written (today's
    /// records for their readers, the raw tiles' archives named, the coordinator's state per term,
    /// its history). What it may do now; None when the pool isn't on. The pool's own thread
    /// (`pool::Ticker`) steps nothing from its start to its end, and steps the driver itself while
    /// this loop is busy elsewhere (`pool::Run::tick`); what it needs of the agent for that is set
    /// here (`pool::Cx`).
    fn pool_step(
        &mut self,
        root: Option<&store::nasfs::NasRoot>,
        c: &Conditions,
        waiting: &mut Vec<Waiting>,
    ) -> Option<pool::Gates> {
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
            run.entries.append(&mut self.held_entries);
            // (A sweep's start asked for its term re-asserted: `try_start_said`.)
            if std::mem::take(&mut self.reassert) {
                run.reassert = true;
            }
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
    fn pool_skipped(
        &mut self,
        root: Option<&store::nasfs::NasRoot>,
        since: Option<u64>,
        waiting: &mut Vec<Waiting>,
    ) -> pool::Gates {
        self.pool_skips += 1;
        // (By the awake clock: a sleep isn't a hang.)
        let held = since.map_or(0, |t| pool::Ticking::awake().saturating_sub(t));
        if held > crate::pool::driver::STALL_S {
            if self.pool_hung.is_none() {
                eprintln!("agent: the pool's thread's step has held its state {} min: taken for hung (the NAS not answering); its coordinator grants nothing", held / 60);
            }
            self.pool_hung = since;
            if let Some(co) = &self.coord {
                co.set_moving(Some(HUNG.into()));
                // (Its answers say no term; the next pool step that runs says it again.)
                co.set_member(None);
                // (On a thread of its own, not waited for: the NAS it reads and removes on hangs. One
                // at a time; and not once the contact was published again since it began.)
                let busy = self.unpublishing.as_ref().is_some_and(|t| !t.is_finished());
                if let (Some(r), true, false) = (root, self.published.is_some(), busy) {
                    let (c, r, gens) = (co.clone(), r.clone(), self.publish_gen.clone());
                    let at = gens.load(std::sync::atomic::Ordering::SeqCst);
                    self.unpublishing = Some(std::thread::spawn(move || {
                        c.unpublish_unless(&r, &|| gens.load(std::sync::atomic::Ordering::SeqCst) != at)
                    }));
                    self.published = None;
                }
            }
        }
        let why = if self.pool_hung.is_some() {
            format!("its step on the pool's thread has hung for {} min (the NAS isn't answering it): nothing is granted or started on its behalf until it ends; if it lasts, disconnect this Mac's SMB session in DSM (Control Panel → File Services → SMB, or Resource Monitor → Connections; docs/macs.md)", held / 60)
        } else if self.pool_skips >= HUNG_SKIPS {
            format!("its step on the pool's thread has been under way {held} s (its share slow to answer): nothing is granted or started on its behalf until it ends")
        } else {
            "its step is under way on the pool's thread (its share slow to answer)".to_string()
        };
        waiting.push(Waiting { step: None, what: "The pool".into(), why });
        let g = self.pool_seen.as_ref().map(|p| p.gates.clone()).unwrap_or_default();
        pool::Gates { term: g.term, leads: g.leads, ..Default::default() }
    }

    fn pool_step_in(
        &mut self,
        root: Option<&store::nasfs::NasRoot>,
        c: &Conditions,
        waiting: &mut Vec<Waiting>,
    ) -> Option<pool::Gates> {
        let lead = self.pooled()?.role == pool::Role::Lead;
        let Some(r) = root else {
            // (No NAS: no step; leading, nothing the driver hasn't said. Nor a tick.)
            let mut run = self.pooled()?;
            run.gates = pool::Gates { term: run.gates.term, leads: run.gates.leads, ..Default::default() };
            run.cx.root = None;
            if let Some(co) = &self.coord {
                co.set_moving(Some("the NAS isn't reachable".into()));
                // (Its answers say no term.)
                co.set_member(None);
            }
            return Some(run.gates.clone());
        };
        // No longer leading: the duties in flight cancelled (a catalogue killed: it writes once at
        // its end; a sweep stopped).
        let lost = lead && self.pooled().is_some_and(|p| p.gates.leads.is_none());
        if lost {
            for k in 0..self.slots.len() {
                if self.slots[k].running.as_ref().is_some_and(|j| is_duty(&j.spec.id)) {
                    let j = self.slots[k].running.as_mut().unwrap();
                    let why = "this Mac no longer leads";
                    eprintln!("agent: {} stopped: {why}", j.spec.id);
                    j.stop(Duration::from_secs(30));
                    self.stopped(k, &format!("stopped: {why}"));
                    self.slots[k].running = None;
                    std::fs::remove_file(self.record_path(k)).ok();
                }
            }
        }
        let home = self.o.home.clone();
        let answered = self.client.as_ref().and_then(|c| c.answered());
        let (lead_urls, lead_failed, lead_token) =
            self.client.as_ref().map(|c| (c.urls(), c.failed(), c.token())).unwrap_or_default();
        let mut run = self.pooled()?;
        // (Only an answer from the current term's lead's coordinator, as it said it is.)
        let cur = run.side.driver().current().lead.clone();
        let lead_answered = pool::lead_answered(answered.as_ref(), cur.as_ref());
        if let Some(co) = &self.coord {
            co.set_member(Some(&run.side.member().id));
        }
        run.cx = pool::Cx {
            root: Some(r.clone()),
            home: home.clone(),
            coord: self.coord.clone(),
            lead_answered,
            broker: self.brokering(),
            broker_https: self.broker_https.as_ref().and_then(|h| h.1.clone()),
            lead_urls,
            lead_failed,
            lead_token,
        };
        // (The jobs an earlier process left, once `run` stopped any still running; those this one
        // took over, `adopt_jobs`, hand off as they end.)
        let running: Vec<PathBuf> = self
            .slots
            .iter()
            .filter(|s| s.running.is_some())
            .filter_map(|s| match &s.lease {
                Some(Held::Pooled { dir, .. }) => Some(dir.clone()),
                _ => None,
            })
            .collect();
        run.take_left(&home, &running);
        if lead {
            run.drain(&home.join("coord/journal"));
        }
        run.drain_outbox(&home.join("outbox"));
        run.conds = Some(pool::Conds { home: c.home, ac: c.ac, battery: c.battery, held: self.hold().held.is_some() });
        run.tend_brokers();
        run.tend_reach();
        let out = run.step();
        // (When its checks ran: the coordinator's grants count from then, not from the tell.)
        let checked = crate::coord::wall_now();
        // (Its part changed, in this process: its coordinator started or stopped, its jobs going on;
        // not tried again once a restart is asked for, its coordinator failing to start.)
        let change = run.restart.is_none();
        drop(run);
        if change {
            self.declared_role();
            self.change_part(r, &out);
        }
        let keep: Vec<u64> = self
            .slots
            .iter()
            .filter_map(|s| match &s.lease {
                Some(Held::Pooled { id, .. } | Held::Own(id)) => Some(*id),
                _ => None,
            })
            .collect();
        let mut run = self.pooled()?;
        // (With the terms a tick took up, whose coordinator's part is this step's.)
        let mut took = std::mem::take(&mut run.pending_took_up);
        took.extend(out.events.iter().filter(|e| matches!(e, crate::pool::driver::Event::TookUp { .. })).cloned());
        pool::took_up(&mut run, self.coord.as_deref(), &took, out.leads, &self.host, &keep);
        lead::after(&mut run, &out, r, &home, self.coord.as_deref());
        run.cx.coord = self.coord.clone();
        run.cx.broker = self.brokering();
        for e in &out.events {
            if let crate::pool::driver::Event::Failed { what, why } | crate::pool::driver::Event::Waits { what, why } =
                e
            {
                waiting.push(Waiting {
                    step: None,
                    what: "The pool".into(),
                    why: format!("couldn't {what} yet: {why}"),
                });
            }
        }
        let g = run.gates.clone();
        if let Some(co) = &self.coord {
            co.set_moving_as_of(pool::moving(&g, false), checked);
        }
        drop(run);
        if lead && g.leads.is_some() {
            let saved = !out
                .events
                .iter()
                .any(|e| matches!(e, crate::pool::driver::Event::Failed { what: "save the records", .. }));
            // (The build pages' asks, its own page's and the members' passed on: first, so the
            // coordinator's state written now holds a retry's forgotten failures.)
            self.act_on(&out.acts);
            self.pool_lead_files(r, saved);
            return Some(self.pooled()?.gates.clone());
        }
        Some(g)
    }

    /// This process's term changed with its step `out` (docs/pool.md §7.6; its parts are its role's,
    /// docs/roles.md §1, never changed in process): the declared lead took up a term, so its
    /// coordinator (built as it started) leads it, the jobs it runs going on, their leases its own
    /// coordinator's from now (held again: `beat`); or it no longer leads one, so it restarts, its
    /// duties in flight stopped first. A lead whose coordinator didn't start restarts too.
    fn change_part(&mut self, root: &store::nasfs::NasRoot, out: &crate::pool::driver::Out) {
        let Some(role) = self.pooled().map(|r| r.role) else { return };
        if matches!((role, out.leads), (pool::Role::Member, Some(_)) | (pool::Role::Lead, None)) {
            // (Its part's heartbeat no longer: the beat writes no lead's `state/status.json` over
            // the new lead's, till this part composes its own.)
            self.beat.part_changed();
        }
        match (role, out.leads) {
            (pool::Role::Member, Some(term)) if self.coord.is_some() => {
                eprintln!("agent: this Mac leads term {term}: its coordinator leads it, its jobs going on");
                if let Some(mut run) = self.pooled() {
                    run.role = pool::Role::Lead;
                }
                // (Its running jobs' leases held in it at once, as a beat holds them, so it grants
                // none of their targets elsewhere: those taken over too.)
                for k in 0..self.slots.len() {
                    if self.slots[k].running.is_some() {
                        self.beat(k, Some(root));
                    }
                }
                self.published = None;
            }
            (pool::Role::Member, Some(term)) => self.restart_for_part(format!(
                "it leads term {term}, and no coordinator runs in this process (its role {:?} as it started)",
                self.role
            )),
            (pool::Role::Lead, None) if out.stop.is_none() => {
                eprintln!("agent: this Mac no longer leads: restarting, its duties stopped");
                if let Some(mut run) = self.pooled() {
                    run.role = pool::Role::Member;
                    // (Nothing of the coordinator's for a tick from here.)
                    run.cx.coord = None;
                }
                // (Its duties in flight stopped, a catalogue or a sweep: a lead's alone.)
                for k in 0..self.slots.len() {
                    if self.slots[k].running.as_ref().is_some_and(|j| is_duty(&j.spec.id)) {
                        let j = self.slots[k].running.as_mut().unwrap();
                        eprintln!("agent: {} stopped: this Mac no longer leads", j.spec.id);
                        j.stop(Duration::from_secs(30));
                        self.stopped(k, "stopped: this Mac no longer leads");
                        self.slots[k].running = None;
                        std::fs::remove_file(self.record_path(k)).ok();
                    }
                }
                self.lead_awake_off();
                self.restart_for_part("it no longer leads its term".into());
            }
            _ => {}
        }
    }

    /// A restart into this Mac's role asked for (`restart_now`: at once), the first reason kept.
    fn restart_for_part(&mut self, why: String) {
        if self.role_restart.is_none() {
            eprintln!("agent: {why}");
            self.role_restart = Some(why);
        }
    }

    /// The role as the declaration last read says (the driver's, each step) against the role this
    /// process started with: an unknown one learnt a member's is taken as it is (its parts are
    /// already a member's); any other change restarts the agent into the new role (docs/roles.md
    /// §1, decision 3). A declaration not read (none, or unreadable) changes nothing.
    fn declared_role(&mut self) {
        let Some(now) = self.pooled().map(|r| role_by(&r)) else { return };
        match (self.role, now) {
            (_, Role::Unknown) => {}
            (was, now) if was == now => {}
            (Role::Unknown, Role::Member) => {
                eprintln!("agent: the lead's declaration read: this Mac is a member");
                self.role = Role::Member;
                drop_lead_caches(&self.o.home);
            }
            (was, now) => self.restart_for_part(format!(
                "the lead's declaration changed this Mac's role ({} → {}): restarting into it",
                role_name(was),
                role_name(now)
            )),
        }
    }

    /// The idle-sleep assertion the lead holds while it leads with leases out (docs/pool.md §6.7), so
    /// no member stalls behind a lead that dozed off with no job of its own; let go otherwise.
    fn lead_awake(&mut self) {
        let leading = self.pool_seen.as_ref().is_some_and(|p| p.role == pool::Role::Lead && p.gates.leads.is_some());
        let leases = self.coord.as_ref().is_some_and(|c| c.leases_out() > 0);
        if !(leading && leases) {
            self.lead_awake_off();
            return;
        }
        if self.lead_awake.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(false))) {
            return;
        }
        self.lead_awake =
            store::spawn::detached("/usr/bin/caffeinate", &["-i", "-w", &std::process::id().to_string()]).ok();
    }

    fn lead_awake_off(&mut self) {
        if let Some(mut c) = self.lead_awake.take() {
            c.kill().ok();
        }
    }

    /// What the lead keeps written besides its records (docs/pool.md §12): today's files while a
    /// member's app still reads them (`pool::Run::keep_today`; `saved`: this step's records are on
    /// the NAS), the raw tiles' and pyramids' archives its records hold named in their stores'
    /// indexes, its coordinator's store's backup started (crate::pool::store::back_up), its history's
    /// new events.
    fn pool_lead_files(&mut self, root: &store::nasfs::NasRoot, saved: bool) {
        // (The forecast's files the plan handed over, written below.)
        let (fc_bytes, fc_entry) = std::mem::take(&mut self.fc_pending);
        let Some(mut guard) = self.pooled() else { return };
        let run = &mut *guard;
        run.keep_today(saved);
        if let Some(rec) = run.side.driver().records() {
            let new: Vec<(String, crate::rawpack::Pack)> =
                rec.raw.iter().filter(|(_, p)| !run.named_raw.contains(&p.name)).cloned().collect();
            if !new.is_empty() {
                if let Ok(Some(lock)) = crate::out::BuildLock::try_take(root) {
                    let (settled, n) = crate::rawpack::name_handed_places(root, &new, &lock);
                    run.named_raw.extend(settled);
                    if n > 0 {
                        eprintln!(
                            "agent: named {n} tiles' archive{} in their stores' indexes",
                            if n == 1 { "" } else { "s" }
                        );
                    }
                }
            }
        }
        let Some(co) = &self.coord else { return };
        let term = run.gates.leads.unwrap_or(run.gates.term);
        // (Its store backed up to the NAS by a thread of its own, crate::pool::store::back_up.)
        co.back_up(run.side.shared_nas());
        // The coordinator's view (what the worker page takes from its memory): when it changed, at
        // most each `pool::VIEW_EVERY`.
        let view = serde_json::to_vec(&co.view()).unwrap_or_default();
        let due = match &run.view_written {
            Some((t, b, at)) => *t != term || (*b != view && at.elapsed() >= pool::VIEW_EVERY),
            None => true,
        };
        if due {
            let mut rec = serde_json::json!({ "at": now_s() });
            rec["view"] = serde_json::from_slice(&view).unwrap_or_default();
            match run.side.nas().write_whole(&pool::view_path(term), &serde_json::to_vec(&rec).unwrap_or_default()) {
                Ok(()) => run.view_written = Some((term, view, std::time::Instant::now())),
                Err(e) => eprintln!("agent: the coordinator's view of term {term}: {e:#}"),
            }
        }
        // The build page's regions, each time they're made again (and in each new term).
        if let Some((key, v)) = self.plan.view.get().regions_view.as_ref() {
            if run.regions_written.as_ref() != Some(&(term, key.clone())) {
                match run
                    .side
                    .nas()
                    .write_whole(&pool::regions_path(term), &serde_json::to_vec(&**v).unwrap_or_default())
                {
                    Ok(()) => run.regions_written = Some((term, key.clone())),
                    Err(e) => eprintln!("agent: the build page's regions of term {term}: {e:#}"),
                }
            }
        }
        // The forecast's input, each time the forecast's made (at most each minute).
        if let Some(b) = fc_bytes {
            if let Err(e) = run.side.nas().write_whole(&pool::forecast_input_path(term), &b) {
                eprintln!("agent: the forecast's input of term {term}: {e:#}");
            }
        }
        // Into the forecasts' archive (fcarchive): handed to its thread, which writes in batches.
        if let Some(e) = fc_entry {
            let a = self.fc_archiver.get_or_init(|| {
                fcarchive::Archiver::start(root.clone())
                    .map_err(|e| eprintln!("agent: the forecasts' archive's thread: {e}"))
                    .ok()
            });
            if let Some(a) = a {
                a.send(e);
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
        std::fs::read_link(apps.join("current"))
            .ok()
            .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
            .filter(|cur| *cur != self.app)
    }

    /// Whether the loop restarts into a newer app now: one is installed and every slot is free, or
    /// its job, asked to stop at its next safe point for the update, has had its wait
    /// (`update_wait`) and reached none (a long job may have none for hours): left running then,
    /// for the new agent to take over (`hand_over_jobs`).
    fn update_due(&self) -> bool {
        let Some(app) = self.pending_app() else { return false };
        let why = update_why(&app);
        // (Every slot, each by its own drain: its jobs each reach their safe point, or have had
        // their time.)
        (0..self.slots.len()).all(|k| self.slot_update_due(k, &why))
    }

    /// Whether slot `k` lets the loop restart into a newer app (`update_due`, `why` the update's
    /// drain): no job there, or its job, asked to stop for it, reached no safe point in its wait.
    fn slot_update_due(&self, k: usize, why: &str) -> bool {
        match &self.slots[k].running {
            None => true,
            Some(r) => {
                if r.pausing.as_deref() != Some(why) {
                    return false;
                }
                let Some(since) = self.slots[k].drain_since else { return false };
                let wait = self.slots[k].update_wait.as_ref().and_then(|w| w.0).unwrap_or(self.grace);
                // (Past the time a pause gives, a job whose processes have done next to nothing
                // for UPDATE_STALL holds it no longer: it may be stuck. By its CPU, not its progress
                // lines, which a long step may not print.)
                let stalled = self.sampler.busy_at(k).is_some_and(|t| t.elapsed() >= UPDATE_STALL);
                since.elapsed() >= wait || (stalled && since.elapsed() >= self.grace)
            }
        }
    }

    /// How long an update's drain waits for slot `k`'s job to reach its safe point, and why: the
    /// time the target under way is expected to need, from its progress's pace (the status's time
    /// left, `job_view`) else its expected time (its own measured run, its step's fit, else the
    /// 90th percentile of its step's runs, crate::needs::Book::secs_p90: a long tail's long jobs
    /// aren't cut short by the mean jobs are sized by; a step of the lead's own, its time here
    /// lately) less what it has spent on it, times UPDATE_MARGIN, at most UPDATE_CAP; with no
    /// estimate, or one already spent, the time a pause gives (`self.grace`).
    fn update_wait(&self, k: usize) -> (Option<Duration>, String) {
        let base = self.grace;
        let Some(r) = self.slots[k].running.as_ref() else { return (None, String::new()) };
        let none = |why: &str| (None, format!("{why}, so the time a pause gives"));
        let (left, from) = if let Some(eta) = self.slots[k].job_eta {
            (eta as f64, "by its progress's pace")
        } else {
            let costs = self.coord.as_ref().map(|c| c.for_forecast().0).unwrap_or_default();
            let book = crate::needs::Book::new(&costs);
            let own = |step: &str| self.step_secs_here(step);
            let secs =
                |step: &str, t: &str| book.secs_p90(step, t, r.threads as u32).map(|s| s as f64).or_else(|| own(step));
            let expected = match r.spec.record.as_ref() {
                Some(w) => {
                    let done = crate::control::read_done(&self.done_path(k), &w.step);
                    // (The target under way timed from when the job last noted one done, its
                    // done file's change, else its start.)
                    let noted = std::fs::metadata(self.done_path(k))
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs());
                    let on_it = now_s().saturating_sub(if done.is_empty() { r.started } else { noted.max(r.started) });
                    w.targets
                        .iter()
                        .find(|t| !done.contains(&t.0))
                        .and_then(|t| secs(&w.step, &t.0))
                        .map(|s| s - on_it as f64)
                }
                None => own(r.spec.id.split(' ').next().unwrap_or("")).map(|s| s - r.elapsed().as_secs_f64()),
            };
            match expected {
                None => return none("its target has no estimate"),
                Some(s) if s <= 0.0 => return none("its target is past its expected time"),
                Some(s) => (s, "by its expected time less what it has spent"),
            }
        };
        let wait = Duration::from_secs_f64(left * UPDATE_MARGIN).min(UPDATE_CAP).max(base);
        (
            Some(wait),
            format!(
                "its target under way needs about {} min more {from} (×{UPDATE_MARGIN}, at most {} h)",
                (left / 60.0).ceil() as u64,
                UPDATE_CAP.as_secs() / 3600
            ),
        )
    }

    /// A newer app is installed locally (`../current` points elsewhere than the agent's folder).
    fn newer_app(&self) -> bool {
        newer_app(&self.o, &self.app)
    }
}

/// What a job of `step` is given after its targets (the pass's `date`, where this Mac's caches
/// and programs are): the lead's own jobs' and a member's alike.
fn step_args(o: &Options, step: &str, date: &str) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let cache = o.home.join("cache");
    let dem = || s(&o.bin.join("dem"));
    match step {
        "terrain-hi" | "terrain-deep" | "terrain-lo" | "terrain-root" => {
            vec!["--raw".into(), s(&cache.join("aws-terrarium"))]
        }
        "pois" | "marks" | "stations" | "overlays" => vec!["--pass".into(), date.to_string()],
        "ferries" | "rail-feeds" => vec!["--pass".into(), date.to_string()],
        // (bld-fetch's coverage, and bldtiles' countries, from the pass's outlines.)
        "bld-fetch" | "bldtiles" => vec!["--pass".into(), date.to_string()],
        "terrain-water" | "polar-root" => vec!["--pass".into(), date.to_string()],
        "items" => vec!["--pass".into(), date.to_string(), "--cache".into(), s(&cache)],
        "heritage-sites" | "heritage" => {
            vec!["--pass".into(), date.to_string(), "--dem".into(), dem(), "--cache".into(), s(&cache)]
        }
        "rail" => vec!["--pass".into(), date.to_string(), "--cache".into(), s(&cache)],
        "peaks" => {
            vec!["--pass".into(), date.to_string(), "--cache".into(), s(&cache), "--coarse-threads".into(), "6".into()]
        }
        "unit" => vec!["--pass".into(), date.to_string(), "--cache-dir".into(), s(&cache)],
        "trees" => {
            vec!["--pass".into(), date.to_string(), "--chm".into(), s(&cache.join("chm10"))]
        }
        "trees-lo" => vec!["--pass".into(), date.to_string()],
        // The server's mirror on this Mac (the agent's home is inside the app's) has the same
        // files: used instead of a second copy where it has them.
        "pack" | "lo" => {
            let mut v = vec!["--pass".into(), date.to_string(), "--cache".into(), s(&cache.join("base"))];
            if let Some(app) = o.home.parent() {
                v.extend(["--mirror".into(), s(&app.join("mirror"))]);
            }
            v
        }
        _ => Vec::new(),
    }
}

/// The OSM pass's command for the planet `planet` of `date`, in this Mac's scratch folder of it
/// (where its stages' markers are: crate::osmpass), the lead's own and a member's resumed alike.
fn osm_pass_cmd(o: &Options, root: &store::nasfs::NasRoot, planet: &NasPath, date: &str) -> Vec<String> {
    // (The planet and planetiler as the pass's arguments: tests/nas_access.rs.)
    let (planet, jar) = (planet.unchecked_path(), root.join("sources/basemap/planetiler.jar").unchecked_path());
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let scratch = o.home.join("scratch").join(format!("osm-{date}"));
    vec![
        s(&o.bin.join("scenic-build")),
        "osm-pass".into(),
        "--root".into(),
        jobs::root_arg(root),
        "--scratch".into(),
        s(&scratch),
        "--planet".into(),
        s(&planet),
        "--date".into(),
        date.to_string(),
        "--extract".into(),
        s(&o.bin.join("extract")),
        "--planetiler".into(),
        s(&jar),
        "--clear".into(),
        s(&o.home.join("cache").join("base")),
    ]
}

/// Where hand-offs wait to be drained into the journal: the coordinator's journal on this Mac.
fn handoff_bases(o: &Options) -> Vec<PathBuf> {
    vec![o.home.join("coord/journal")]
}

/// A newer app is installed locally (`../current` points elsewhere than the agent's folder).
fn newer_app(o: &Options, app: &str) -> bool {
    let Some(apps) = o.bin.parent() else { return false };
    if app == "development" {
        return false;
    }
    std::fs::read_link(apps.join("current"))
        .ok()
        .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
        .is_some_and(|cur| cur != app)
}

/// What a target of `step` took here lately (`Memory::step_secs`); None for a PER_WORK step
/// (crate::needs: its jobs' times mix in targets that found nothing to do, task #830).
fn step_secs_here(mem: &Memory, step: &str) -> Option<f64> {
    (!is_per_work(step)).then(|| mem.step_secs.get(&secs_key(step)).copied()).flatten()
}

/// The most jobs this Mac runs at once (`job_slots` of its cores).
fn lead_slots(helper: bool, ramp_held: bool, ramp: &Ramp) -> usize {
    let n = job_slots(mac_cores() as usize);
    // (Leading: no more than its ramp has reached, `tend_ramp`.)
    // (Held at its start until the NAS's limits are known, `ramp_held`.)
    if helper {
        n
    } else if ramp_held {
        n.min(RAMP_START)
    } else {
        n.min(ramp.slots.max(RAMP_START))
    }
}

/// The owner's disk room target in a waiting's words: ", the disk room target 100.0 GB kept
/// free past it"; none without one.
fn floor_words(floor: u64) -> String {
    match floor {
        0 => String::new(),
        f => format!(", the disk room target's {} kept free past it", room::size(f)),
    }
}

/// The keys to plan with: on the NAS, with the done records of the hand-offs waiting to be merged
/// on top (in `bases`: a worker's job built and handed back, not yet merged: not built again).
/// (Under nasread's big-read deadline, docs/agent-parts.md §4.3: a hung share fails it.)
fn planning_keys(root: &NasRoot, bases: Vec<PathBuf>) -> Result<build::Keys> {
    let r = root.clone();
    let keys = nasread::with_big(&root.join("state/build"), move || {
        build::Keys::load_with(&r, &bases).map_err(|e| std::io::Error::other(format!("{e:#}")))
    });
    keys.context("the build's keys")
}

/// A gate ask's acceptances applied (crate::inputs::apply_ask) against the unit's held report:
/// `shown` as the last plan read it, else as the records name it now; what it did logged.
fn apply_gate_ask(root: &NasRoot, a: &crate::inputs::Ask, shown: Option<crate::inputs::Report>, member: &str) {
    let held = shown.or_else(|| {
        let m = crate::out::read_manifest(root).ok()?;
        crate::inputs::read_report(root, m.get(&crate::inputs::held_logical(&a.unit))?).ok()
    });
    match crate::inputs::apply_ask(root, a, held.as_ref(), member) {
        Ok(said) => {
            for s in said {
                eprintln!("agent: {} ({}): {s}", a.unit, a.by);
            }
        }
        Err(e) => eprintln!("agent: {}'s ask of {}: {e:#}", a.by, a.unit),
    }
}

/// A cancelled job's work under way on `target` dropped (crate::control::Cancel): the entries at
/// the top of its scratch folder `dir` named for the target, as a job names them: `6/55/25` as
/// `6_55_25` or `6-55-25` (a target of no slash as itself), exactly or with an extension
/// (`6_55_25.tif`); the paths removed.
fn clear_target_scratch(dir: &Path, target: &str) -> Vec<PathBuf> {
    if target.is_empty() || target.contains("..") {
        return Vec::new();
    }
    let forms = [target.replace('/', "_"), target.replace('/', "-")];
    let names = |name: &str| {
        forms
            .iter()
            .any(|f| name == f || name.strip_prefix(f.as_str()).is_some_and(|r| r.starts_with('.') && r.len() > 1))
    };
    let mut gone = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if !names(&e.file_name().to_string_lossy()) {
            continue;
        }
        let p = e.path();
        let r = if e.file_type().is_ok_and(|t| t.is_dir()) {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        if r.is_ok() {
            gone.push(p);
        }
    }
    gone
}

/// What jobs read from inputs/ beside the manifest, by digest: which keys inputs/keys.env holds
/// ("keys", `key_names`), and the 3D buildings' downloaded sources ("bld-release", "bld-key", "bldprep 6/x/y",
/// "bldprep-rows 6/x/y": crate::bld::sources::digests).
/// `keys` as every reader reads them (agent::rekey::as_read), for `Agent::start_as_built`'s counts
/// off the loop.
fn as_built_keys(
    root: &store::nasfs::NasRoot,
    cov: &crate::coverage::Coverage,
    manifest: &BTreeMap<String, String>,
    keys: &mut build::Keys,
    tiles: &tiles::TerrainTiles,
) {
    let times = rekey::FileTimes::new(root);
    rekey::as_read(keys, cov, manifest, tiles, &times);
}

pub fn input_digests(root: &store::nasfs::NasRoot) -> BTreeMap<String, String> {
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
    // The regions as a catalogue records them; "?" when they can't be read now (build::catalogue_work
    // then waits).
    inputs.insert(
        "regions".into(),
        crate::inputs::regions::read::source_now(root).map(|a| a.version).unwrap_or_else(|_| "?".into()),
    );
    inputs.insert("keys".into(), key_names(&root.join("inputs/keys.env")).unwrap_or_else(|| "?".into()));
    // The 3D buildings' sources, of the declared releases (crate::inputs::sources): what each z6
    // tile's bldprep reads, and its rows, and what the keys name of the declarations ("bld-key");
    // none before they're accepted (no 3D buildings' work), "?" while they can't be read now.
    match crate::inputs::sources::open_now(root).and_then(|s| s.map(|s| crate::bld::sources::declared(&s)).transpose())
    {
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

/// The waiting line for the keys the timetable feeds' declarations fetch with that inputs/keys.env
/// doesn't hold (`inputs` "feed-keys" less "keys"): those feeds are left out until it does (not a
/// finding: keys.env isn't an input, and the declaration is right).
fn missing_feed_keys(inputs: &BTreeMap<String, String>) -> Option<String> {
    let held: Vec<&str> = inputs.get("keys")?.split(',').collect();
    let missing: Vec<&str> =
        inputs.get("feed-keys")?.split(',').filter(|k| !k.is_empty() && !held.contains(k)).collect();
    (!missing.is_empty()).then(|| {
        format!(
            "keys.env has no {}: the feeds declared with {} are left out until it does",
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" }
        )
    })
}

/// The names of the keys a `KEY=value` file holds a value for, sorted and comma-separated ("" when
/// there's no file; None when it can't be read). Never their values: the names go in job keys.
fn key_names(path: &NasPath) -> Option<String> {
    let text = match path.read_to_string() {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(String::new()),
        Err(_) => return None,
    };
    let mut names: Vec<&str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, v)| !k.trim().starts_with('#') && !v.trim().is_empty())
        .map(|(k, _)| k.trim())
        .collect();
    names.sort_unstable();
    names.dedup();
    Some(names.join(","))
}

/// How often an idle agent plans again while the budgets' proxy won't start.
const PROXY_REPLAN: Duration = Duration::from_secs(60);

/// The regions as the readers read them now (the accepted version: crate::inputs::regions::read),
/// sorted by id, and why none can be read when it can't (one line).

fn load_regions(root: &store::nasfs::NasRoot) -> (Vec<recipes::Recipe>, Vec<(String, String)>) {
    match crate::inputs::regions::read::source_now(root) {
        Ok(a) => match crate::inputs::regions::read::recipes(&a) {
            Ok(r) => (r, Vec::new()),
            Err(e) => (Vec::new(), vec![("regions".into(), format!("{e:#}"))]),
        },
        Err(e) => (Vec::new(), vec![("regions".into(), format!("{e:#}"))]),
    }
}

/// A clear's event in the history, as `worker` did it, in words.
fn caches_event(worker: &str, f: &room::Freed) -> crate::coord::history::Event {
    let asked = f.by.as_ref().map(|b| format!(", as {b} asked")).unwrap_or_default();
    let deeply = if f.level == Some(room::Level::Deep) { " deeply" } else { "" };
    let note = match &f.why_not {
        Some(why) => format!("didn't clear its build caches{deeply}{asked}: {why}"),
        None => format!("cleared its build caches{deeply}{asked}: {}", f.say()),
    };
    crate::coord::history::Event {
        worker: Some(worker.to_string()),
        note,
        ..crate::coord::history::Event::new("caches")
    }
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

/// A job's step: its id's first word ("unit 6/31/20": "unit", a leased task's its kind: "tail
/// 6/1/1": "tail"; `JobId`).
pub fn step_of(id: &str) -> Option<String> {
    Some(JobId::parse(id).step).filter(|s| !s.is_empty()).map(str::to_string)
}

/// What job `spec`'s own lease is held again for: its record, or for a job of none (buildings)
/// its step from its id and no targets (#666: it was killed when its lease lapsed).
fn held_work(spec: &JobSpec) -> build::Work {
    spec.record
        .clone()
        .unwrap_or_else(|| build::Work { step: step_of(&spec.id).unwrap_or_default(), targets: Vec::new() })
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
    let (kept, out): (Vec<JobSpec>, Vec<JobSpec>) =
        jobs.into_iter().partition(|j| !build::reads_sources(&step_of(&j.id).unwrap_or_default()));
    for j in out {
        waiting.push(Waiting {
            step: step_of(&j.id),
            what: j.what,
            why: "the source declarations' first accepted version (the input check takes inputs/sources/ in)".into(),
        });
    }
    kept
}

/// The jobs that read the pass's lake fill (crate::lakefill: the water, which draws it; the terrain's
/// water digests, whose pin names it; the catalogues, which name it to the server), taken out of
/// `jobs` while it `held`s them (the lakefill job has work and hasn't failed
/// build::LAKEFILL_HOLDS times in a row, or its source can't be read), each said in `waiting`.
/// Nothing reads a fill made from an older basemap meanwhile, and nothing is made twice.
fn hold_for_lakefill(jobs: Vec<JobSpec>, held: bool, waiting: &mut Vec<Waiting>) -> Vec<JobSpec> {
    if !held {
        return jobs;
    }
    let (kept, out): (Vec<JobSpec>, Vec<JobSpec>) =
        jobs.into_iter().partition(|j| !build::reads_lakefill(&step_of(&j.id).unwrap_or_default()));
    for j in out {
        waiting.push(Waiting {
            step: step_of(&j.id),
            what: j.what,
            why: "waits for Canada's missing lakes to be filled (the lakefill job)".into(),
        });
    }
    kept
}

/// The checklist as the status shows it. A step with work left that isn't this Mac's job now (`now`,
/// its step) says why: another Mac is on it, or its job waits (`waiting`: for the home network,
/// out a failure). Publishing, while a step above has work left, goes out as each z6 tile is built (a
/// catalogue at most each minute) and once they all are: it's done only then.
fn annotate(list: &mut [build::Line], now: &[String], helpers: &[Status], waiting: &[Waiting]) {
    // (A step whose job runs, here or on a helper, isn't done: build::Line::under_way.)
    let helper_steps = helpers.iter().flat_map(|h| h.jobs()).filter_map(|j| step_of(&j.id));
    let running: Vec<String> = now.iter().cloned().chain(helper_steps).collect();
    for s in list.iter_mut() {
        for r in &running {
            s.under_way(r);
        }
    }
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
        if let Some(h) = helpers.iter().find(|h| h.jobs().iter().any(|j| ours(&step_of(&j.id)))) {
            s.note = Some(format!("on {}", h.host));
        } else if let Some(w) = waiting.iter().find(|w| ours(&w.step)) {
            s.note = Some(w.why.clone());
        }
    }
    if let Some(p) = list.last_mut().filter(|p| p.steps.iter().any(|s| s == "catalog")) {
        if before_left && !busy(p) {
            p.left = Some(p.left.unwrap_or(0).max(1));
            // (Its own, a catalogue failing, says more.)
            p.note.get_or_insert_with(|| {
                "as each unit is built (a catalogue at most each minute), and once the steps above are".into()
            });
        }
    }
}

/// Where a job's pace is measured from: from now, at `frac`, as its progress is first seen or is
/// measured again (a fresh job's setup isn't folded in). But a job taken over from the agent before
/// this one (`taken_over`: adopted, its progress first seen here) is paced from its real start
/// (`started_at`) at nothing done, so its estimate doesn't start over: when it's on its first part
/// or has none. (On a later part it's paced from the take-over: when that part started isn't known.)
fn pace_base(
    taken_over: bool,
    started_at: Instant,
    frac: f64,
    key: String,
    part: Option<usize>,
) -> (Instant, f64, String, Option<usize>) {
    if taken_over && part.is_none_or(|p| p == 0) {
        (started_at, 0.0, key, part)
    } else {
        (Instant::now(), frac, key, part)
    }
}

/// Why a running job stops at its next safe point for a newer app installed.
fn update_why(app: &str) -> String {
    format!("restarting into the newly installed app ({app})")
}

/// Why a running job stops, and how, if it does: at once (frozen where it is) without what it can't
/// save without, the NAS, or away from home with a job that needs it; as the build's pause says;
/// at its next safe point when the battery runs low (CPU work), before the charge runs out.
fn stop_for(
    n: &Needs,
    c: &Conditions,
    pause: Option<&crate::control::Pause>,
) -> Option<(crate::control::Mode, String)> {
    use crate::control::Mode;
    match lapsed(n, c) {
        Some(why) => Some((Mode::Freeze, why)),
        None => pause.map(|p| (p.mode, p.why())),
    }
}

/// A member's hold (crate::control::HoldState) written whole to `p` on the NAS, on the writers
/// under a deadline (`nasread::write`).
fn write_hold(p: &NasPath, st: &crate::control::HoldState) -> Result<()> {
    let b = serde_json::to_vec(st)?;
    let owned = p.clone();
    nasread::write(p, move || {
        if let Some(d) = owned.parent() {
            d.create_dir_all()?;
        }
        owned.write_whole(&b)
    })
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
fn make_coordinator_on(
    o: &Options,
    run: Option<&pool::Run>,
    app: &str,
    host: &str,
    port: u16,
) -> Option<crate::coord::Coordinator> {
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
        Some(l) => crate::coord::Coordinator::start_on(&o.home.join("coord"), l, host, app),
        None => crate::coord::Coordinator::start(&o.home.join("coord"), port, host, app),
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

/// Why a member's page's retry can't be mailed to the lead now, from `seen` (the lead's app, None
/// with no lead; this Mac's app): Some(None) it can, Some(Some(why)) it can't; None, not known
/// now (the pool's run held by a tick, or out of the pool): the last word stands.
fn retry_blocked(seen: Option<(Option<&str>, &str)>) -> Option<Option<String>> {
    let (lead, me) = seen?;
    Some(match lead {
        None => Some("no lead now".to_string()),
        Some(app) if app != me => {
            Some(format!("the lead runs app {app}, this Mac {me}: try again on the lead's own page"))
        }
        Some(_) => None,
    })
}

/// A viewer alone on this Mac's coordinator port (crate::coord::Coordinator::viewer): the page and
/// its views, nothing kept; None when it can't listen.
fn make_viewer(app: &str, host: &str) -> Option<crate::coord::Coordinator> {
    let port = coord_port();
    #[cfg(test)]
    let held = crate::coord::take_test_listener(port);
    #[cfg(not(test))]
    let held: Option<std::net::TcpListener> = None;
    let l = match held {
        Some(l) => Ok(l),
        None => store::spawn::listen((crate::coord::LISTEN, port)),
    };
    match l.map_err(anyhow::Error::from).and_then(|l| crate::coord::Coordinator::viewer(l, host, app)) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("agent: no viewer on port {port} ({e:#})");
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
/// in a unit test (no Mac's own Tailscale decides it).
#[cfg(not(test))]
fn served_https(port: u16) -> Option<String> {
    crate::net::served_https(port)
}

#[cfg(test)]
fn served_https(_port: u16) -> Option<String> {
    None
}

/// The origins a proxy on this Mac serves its pages under (crate::net::names, "https://<name>"):
/// its broker's `pages`.
fn proxy_pages() -> Vec<String> {
    crate::net::names().iter().map(|n| format!("https://{n}")).collect()
}

/// The app versions the programs `cmds` start are in (`<apps>/<version>/<program>`), the apps'
/// folder that of an agent whose programs are in `bin`, other than its own `app` (Agent::job_apps).
/// Both sides compared resolved: a job's command names its agent's resolved folder (Agent::new_with),
/// and `current`, a link whose target changes, names no version.
fn job_apps_of<'a>(cmds: impl Iterator<Item = &'a str>, bin: &Path, app: &str) -> Vec<String> {
    let bin = bin.canonicalize().unwrap_or_else(|_| bin.to_path_buf());
    let Some(apps) = bin.parent() else { return Vec::new() };
    let mut out: Vec<String> = cmds
        .filter_map(|c| {
            let c = Path::new(c);
            // (Its folder resolved too, the version's own folder kept: not `current` followed.)
            let dir = c.parent()?;
            let dir = if dir.file_name().is_some_and(|n| n == "current") {
                dir.to_path_buf()
            } else {
                dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
            };
            Some(dir.strip_prefix(apps).ok()?.components().next()?.as_os_str().to_string_lossy().into_owned())
        })
        .filter(|v| v != app && v != "current")
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The app version of the programs in `bin` (`…/app/<version>/`), or "development".
pub fn app_version(bin: &Path) -> String {
    let canon = bin.canonicalize().unwrap_or_else(|_| bin.to_path_buf());
    match (canon.parent().and_then(|p| p.file_name()), canon.file_name()) {
        (Some(app), Some(v)) if app == "app" => v.to_string_lossy().into_owned(),
        _ => "development".into(),
    }
}

/// A read's bytes (Some(Some)), the file missing (Some(None)), or None when it can't be read now
/// (any other error, a timeout's too).
fn present_or_missing(r: std::io::Result<Vec<u8>>) -> Option<Option<Vec<u8>>> {
    match r {
        Ok(b) => Some(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
    }
}

/// Writes a small file that is replaced each time (status files), its folder made
/// (store::whole::write: this Mac's own temporary name, a busy rename falling back to an aside).
fn write_replace(p: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    store::whole::write(p, bytes)
}

/// The heartbeat `b` written to `p` on the NAS, on the writers under a deadline (nasread::write).
fn write_beat(p: &NasPath, b: Vec<u8>) -> Result<()> {
    let p2 = p.clone();
    nasread::write(p, move || write_replace_nas(&p2, &b))
}

/// `write_replace` on the NAS (its folder made once a process: store::nasfs).

fn write_replace_nas(p: &NasPath, bytes: &[u8]) -> Result<()> {
    if let Some(d) = p.parent() {
        d.create_dir_all()?;
    }
    p.write_whole(bytes)
}

/// The status a `scenic status` shows: the NAS's heartbeat, else this Mac's copy.
pub fn read_status(root: Option<&store::nasfs::NasRoot>, home: &Path) -> Option<Status> {
    root.and_then(|r| r.join("state/status.json").read_replaced().ok())
        .or_else(|| std::fs::read(home.join("status.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// The targets of `w` made again as they are, expected the same (scenic-build trees, terrain-hi and
/// slope-hi `--expect-same`): tree cover's, terrain-hi's and slope-hi's z6 tiles the records as read
/// (`done`: agent::rekey::as_read) have under the key they're built with, their mids made
/// (build::TreeWork::backfill, build::TerrainWork::backfill, and those an assembly needs). Any
/// other step's: none.
fn expect_same(w: &build::Work, done: &build::Keys) -> Vec<String> {
    if !matches!(w.step.as_str(), "trees" | "terrain-hi" | "slope-hi") {
        return Vec::new();
    }
    w.targets
        .iter()
        .filter(|(t, k)| t.starts_with("6/") && done.recorded(&w.step, t) == Some(k.as_str()))
        .map(|t| t.0.clone())
        .collect()
}

/// The helpers building now: `state/helpers/<host>.json` beaten within ten minutes, but `lead`'s
/// own. That one is from while it was a member and goes stale by itself for up to ten minutes after
/// it takes the lead (task #816: the forecast listed its slots twice, as the lead's and as a
/// member's, and `forecast::runs_of` numbered past the lanes).
fn helpers(root: &store::nasfs::NasRoot, lead: &str) -> Vec<Status> {
    let Ok(rd) = root.join("state/helpers").read_dir() else { return Vec::new() };
    let mut out: Vec<Status> = rd
        .into_iter()
        .filter(|e| e.path.extension() == Some("json"))
        .filter_map(|e| e.path.read().ok().and_then(|b| serde_json::from_slice::<Status>(&b).ok()))
        .filter(|h| h.host != lead && now_s().saturating_sub(h.beat) < 600)
        .map(|h| Status { regions: Vec::new(), checklist: Vec::new(), built: BTreeMap::new(), recent: Vec::new(), ..h })
        .collect();
    out.sort_by(|a, b| a.host.cmp(&b.host));
    out
}

/// The map's catalogue as the status says it (`CatalogueSeen`): its `seq` and its batch's time.
fn seen(c: &store::catalogue::Catalogue) -> CatalogueSeen {
    CatalogueSeen { n: c.n, at: cataloguesview::unix_of(&c.created).unwrap_or(0) }
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

/// Whether `step`'s time is the work its target finds (crate::needs::PER_WORK).
fn is_per_work(step: &str) -> bool {
    crate::needs::PER_WORK.iter().any(|p| p.0 == step)
}

/// About how long a target of `step` takes on the lead (seconds) until it's been timed there:
/// the forecast's first guess (crate::agent::forecast), from the jobs' logs of 2026-10 (a tree cover
/// z6 tile's from its z3 tiles' whole runs: 8,171 s for ~370 z6 tiles).
fn first_secs(step: &str) -> f64 {
    match step {
        "pass-sets" => 5000.0,
        "outlines" => 330.0,
        "trailends" => 15.0,
        // (The coastline set read twice, its rings assembled and cut by cell: measured by hand on
        // the M4, 2026-10-27.)
        "regions-land" => 600.0,
        "reach" => 1800.0,
        "buildings" | "items" => 3600.0,
        "summits" => 30.0,
        "labels" => 2200.0,
        "water" => 3600.0,
        // (The survey, 47 s, and the fill by bands, about 15 min on the M1, 2026-10-09.)
        "lakefill" => 1200.0,
        "heritage-sites" => 240.0,
        // (Reading the latest basemap's water under every z6 tile near the coverage.)
        "terrain-water" => 900.0,
        // (A z6 tile of GSI's five layers: 6/56/23's into a scratch store on the M4, 2026-10-27,
        // 4,950 s for 170,612 tiles, 1.97 GB; task #810.)
        "pyramids-fill" => 5000.0,
        "heritage" => 5400.0,
        // (Terrain's and slope's z6 tiles a z6 tile, their assemblies a z3 tile's zoomed-out levels:
        // the z3 tiles' last runs, 9,707 s and ~4,100 s over ~400 z6 tiles, 2026-10.)
        "terrain-hi" => 25.0,
        "terrain-lo" => 60.0,
        "slope-hi" => 10.0,
        // (A z10 tile of dense lidar to z16, task #639's measure on the M4: see docs/phase-times.md.)
        "terrain-deep" => 100.0,
        "slope-deep" => 30.0,
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
        "terrain-root" | "slope-root" | "depth-root" => 10.0,
        // (A block to z9 from GLO-30 on the NAS, terrain and shading together: 5.2 min a block on the
        // M4's 8 threads, 25 Oct; the root's z0–3, 41 tiles sampled.)
        "polar-terrain" => 240.0,
        "polar-slope" => 70.0,
        "polar-root" => 300.0,
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

/// An agent for a test: it reads the test's fixed Mac (cond::Mac::TEST), not the one running it.
#[cfg(test)]
fn test_agent(o: Options) -> Result<Agent> {
    // (Never the real NAS or the app's home: a test with no root would find the mounted NAS.)
    let root = o.root.as_ref().expect("a test agent needs a root of its own (a temp folder)");
    assert!(
        !store::nasfs::is_live_path(&root.unchecked_path()),
        "a test agent's root is the real NAS: {}",
        root.display()
    );
    assert!(!store::nasfs::is_live_path(&o.home), "a test agent's home is the app's real one: {}", o.home.display());
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

/// A forecast run off the agent's loop (`Agent::start_forecast`): it, its record's bytes, and how
/// long it took.
struct Forecasted {
    forecast: forecast::Forecast,
    bytes: anyhow::Result<Vec<u8>>,
    entry: fcarchive::Entry,
    tocome: forecast::ToCome,
    /// This Mac's tiles to come in it, as cache hints, with its time (`Agent::upcoming`).
    upcoming: Option<(u64, room::Hints)>,
    waits: Option<crate::coord::history::Waits>,
    took: std::time::Duration,
}

#[cfg(test)]
mod tests {

    /// Task #444: a NAS wait said again between plans (the plan's lines kept) is listed once, with
    /// its minutes now; gone once granted.
    #[test]
    fn nas_wait_lines_once_each() {
        let mut w = vec![Waiting { what: "other".into(), why: "x".into(), step: None }];
        nas_wait_lines(&mut w, vec![("depth-lo".into(), "3/1/3".into(), 45)], None);
        nas_wait_lines(&mut w, vec![("depth-lo".into(), "3/1/3".into(), 48)], None);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[1].why.starts_with("waiting 48 min") && w[1].what == "depth-lo 3/1/3", "{w:?}");
        // (Held by the NAS's own load: a line for the NAS, and each target's says why.)
        let held = Some("held by NAS load (91 % > target 85 %)".to_string());
        nas_wait_lines(&mut w, vec![("depth-lo".into(), "3/1/3".into(), 50)], held.clone());
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[1].what == "the NAS" && w[1].why.starts_with("held by NAS load (91 %"), "{w:?}");
        assert!(w[2].why.starts_with("waiting 50 min, held by NAS load"), "{w:?}");
        nas_wait_lines(&mut w, Vec::new(), None);
        assert_eq!(w.len(), 1, "granted: gone");
    }
    use super::*;

    /// #666: a job of no record (buildings) is held again by its step from its id, no targets,
    /// when its lease's beat fails; one with a record by its record.
    #[test]
    fn a_record_less_job_is_held_again_by_its_step() {
        let spec = |record| JobSpec {
            id: "buildings 2026-09-01".into(),
            what: String::new(),
            cmd: Vec::new(),
            needs: Default::default(),
            restart_after_sleep: false,
            record,
        };
        let w = held_work(&spec(None));
        assert_eq!((w.step.as_str(), w.targets.len()), ("buildings", 0));
        let r = build::Work { step: "unit".into(), targets: vec![("6/1/1".into(), "k".into())] };
        assert_eq!(held_work(&spec(Some(r.clone()))).targets, r.targets);
    }

    /// Task #392 review: what a capacity says this Mac holds of the shared caches is read off its
    /// disk: the whole local copies; emptied by whatever, nothing. (The base packs' folder holds
    /// the pack jobs' way indexes alone since task #396, and the old DEM cache's files are unused
    /// since task #595: no shared cache.)
    #[test]
    fn the_caches_here_are_read_off_the_disk() {
        let d = tempfile::tempdir().unwrap();
        let have = |n: &str| caches_here(d.path())[n].have_mb;
        std::fs::create_dir_all(d.path().join("base/base")).unwrap();
        std::fs::write(d.path().join("base/base/a.sect.idx"), vec![0u8; 3 << 20]).unwrap();
        std::fs::write(d.path().join("dem-cache.elev.f32"), vec![0u8; 2 << 20]).unwrap();
        std::fs::write(d.path().join("other"), vec![0u8; 5 << 20]).unwrap();
        assert_eq!(caches_here(d.path()).len(), 1, "the depth's copies alone");
        assert_eq!(have(crate::localread::CACHE), 0);
        std::fs::create_dir_all(d.path().join("blobs/layers/basemap")).unwrap();
        std::fs::write(d.path().join("blobs/layers/basemap/world-d.0123456789abcdef.pmtiles"), vec![0u8; 3 << 20])
            .unwrap();
        // (A unit's staging copy isn't of it.)
        std::fs::create_dir_all(d.path().join("blobs/layers/lo")).unwrap();
        std::fs::write(d.path().join("blobs/layers/lo/6-1-2.0123456789abcdef.pack"), vec![0u8; 1 << 20]).unwrap();
        assert_eq!(have(crate::localread::CACHE), 3);
        std::fs::remove_file(d.path().join("blobs/layers/basemap/world-d.0123456789abcdef.pmtiles")).unwrap();
        assert_eq!(have(crate::localread::CACHE), 0, "cleared: missing again");
    }

    /// A costs file's done lines read with their inputs' measures (task #235); a line before them
    /// has none, a `started` line is no cost, and the measures go on to the coordinator's record
    /// as Cost's `sizes` (absent there when empty, so an older reader sees what it knew).
    #[test]
    fn costs_lines_with_and_without_sizes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("costs.jsonl");
        std::fs::write(
            &p,
            concat!(
                "{\"unit\":\"terrain-hi 6/1/2\",\"started\":1}\n",
                "{\"unit\":\"terrain-hi 6/1/2\",\"peak_mb\":900,\"secs\":40,\"v\":3,\"disk_mb\":2,\"sizes\":{\"bytes\":5000,\"km2\":380000,\"threads\":8,\"tiffs\":12}}\n",
                "{\"unit\":\"8/1/2\",\"peak_mb\":3000,\"secs\":60,\"v\":1}\n",
            ),
        )
        .unwrap();
        let c = read_costs(&p);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].0, "terrain-hi 6/1/2");
        assert_eq!(
            c[0].1.sizes,
            [("bytes".to_string(), 5000), ("km2".into(), 380000), ("threads".into(), 8), ("tiffs".into(), 12)].into()
        );
        assert_eq!((c[0].1.peak_mb, c[0].1.secs, c[0].1.v), (900, 40, 3));
        assert!(c[1].1.sizes.is_empty());
        let j = serde_json::to_value(&c[1].1).unwrap();
        assert!(j.get("sizes").is_none());
        let back: crate::coord::Cost = serde_json::from_value(serde_json::to_value(&c[0].1).unwrap()).unwrap();
        assert_eq!(back, c[0].1);
        // (An older reader's Cost: no `sizes`, unknown fields ignored.)
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Old {
            peak_mb: u64,
            secs: u64,
        }
        let _: Old = serde_json::from_value(serde_json::to_value(&c[0].1).unwrap()).unwrap();
    }

    /// A job's failure wait and the memory guard's set when an idle agent plans again: the earliest
    /// end to come, none past.
    #[test]
    fn a_timed_wait_sets_when_to_plan_again() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        let job = |id: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec![],
            needs: Needs { nas: true },
            restart_after_sleep: true,
            record: None,
        };
        let plan = vec![job("backup"), job("gc"), job("pass-sets d")];
        assert_eq!(a.planner().replan_at(&plan, 1000), None);
        a.mem.retry.insert("backup".into(), (2, 1600));
        a.mem.retry.insert("gc".into(), (1, 900));
        a.guard_backoff.insert("pass-sets d".into(), (1300, "held".into()));
        a.mem.retry.insert("not planned".into(), (1, 1001));
        assert_eq!(a.planner().replan_at(&plan, 1000), Some(1300));
        assert_eq!(a.planner().replan_at(&plan, 1300), Some(1300));
        assert_eq!(a.planner().replan_at(&plan, 1301), Some(1600));
        assert_eq!(a.planner().replan_at(&plan, 1601), None);
        // The budgets' proxy not started: planned again each minute, not each loop.
        a.mem.retry.clear();
        a.guard_backoff.clear();
        a.o.dry_run = false;
        assert_eq!(a.planner().replan_at(&plan, 1000), Some(1000 + PROXY_REPLAN.as_secs()));
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        let mut last = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(last, 0);
        assert_eq!(a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root)), last);
        // (Each change a different length: the digest doesn't rest on the file times' resolution.)
        std::fs::write(root.join("state/build/manifest.json"), "{\"a\":\"b\"}").unwrap();
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        last = now;
        std::fs::write(root.join("state/build/jobs.json"), "{}").unwrap();
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        last = now;
        std::fs::create_dir_all(a.handoff_bases(&store::nasfs::NasRoot::local_for_test(&root))[0].join("other-mac"))
            .unwrap();
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        last = now;
        // Publishing's switches and the hold on catalogues.
        for f in [publish::DELTA_CATALOGUE, publish::NEAR_SLOPE, "inputs/hold-catalog"] {
            std::fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            std::fs::write(root.join(f), "on").unwrap();
            let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
            assert_ne!(now, last, "{f}");
            last = now;
        }
        // A held catalogue's folder made.
        std::fs::create_dir_all(root.join("catalog-held")).unwrap();
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        last = now;
        // The coordinator's leases: this Mac's own taken and let go (a start that didn't go
        // through) changes nothing; another worker's taken, then given back, does.
        let co = std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.coord = Some(co.clone());
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        last = now;
        let id = co.hold("gc", &[("t".into(), "k".into())]).unwrap();
        assert_eq!(a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root)), last);
        co.finish(id, &[]);
        assert_eq!(a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root)), last);
        let id = {
            let mut s = co.shared.lock().unwrap();
            let t = s.now();
            s.leases.grant(
                "other-mac",
                crate::coord::lease::Work::Job { step: "gc".into(), targets: vec![("t".into(), "k".into())] },
                t,
            )
        };
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        last = now;
        co.shared.lock().unwrap().leases.cancel(id).unwrap();
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        last = now;
        // The pool's switches the agent holds.
        a.placement = true;
        let now = a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root));
        assert_ne!(now, last);
        assert_eq!(a.planner().plan_seen(&store::nasfs::NasRoot::local_for_test(&root)), now);
    }

    /// Every scenic-build command line the agent makes, by its own code (step_args, the targets as
    /// named, the OSM pass's), passes its command line (crate::cli): none refused at its start.
    #[test]
    fn the_agents_commands_pass_the_steps_table() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        for st in steps::TABLE.iter() {
            let step = st.name.to_string();
            // (gc and backup: `scenic`'s, their commands checked as the plan makes them.)
            // (catalog-held: `catalogue --held --ready …`, as the plan makes it.)
            let scenics = crate::cli::command("scenic").unwrap().find_subcommand(&step).is_some();
            let takes = crate::cli::build::takes_targets(&step);
            if (scenics && takes.is_none()) || step == "catalog-held" {
                continue;
            }
            for target in [step.as_str(), "6/1/1", "3/1/1", "inputs/regions"] {
                if takes == Some(false) && target != step {
                    continue;
                }
                let mut cmd = vec![
                    "SCENIC_X=1".to_string(),
                    s(&a.o.bin.join("scenic-build")),
                    step.clone(),
                    "--root".into(),
                    jobs::root_arg(&store::nasfs::NasRoot::local_for_test(&root)),
                    "--scratch".into(),
                    s(&home.join("scratch").join(&step)),
                ];
                cmd.extend([target.to_string()].into_iter().filter(|t| every::names_target(t) && *t != step));
                cmd.extend(a.step_args(&step, "2026-10-01"));
                if matches!(step.as_str(), "trees" | "terrain-hi" | "slope-hi") {
                    cmd.extend(["--expect-same".to_string(), "6/1/1".into()]);
                }
                if let Some(Err(e)) = crate::cli::check_command(&cmd) {
                    panic!("{e}");
                }
            }
        }
        let pass = a.osm_pass_cmd(
            &store::nasfs::NasRoot::local_for_test(&root),
            &store::nasfs::NasPath::local_for_test(&root.join("planet.osm.pbf")),
            "2026-10-01",
        );
        assert_eq!(crate::cli::check_command(&pass), Some(Ok(())));
    }

    /// A z6 tile's tail as it runs its programs (scenic-metrics and the rest: crate::unit::tail),
    /// and the tasks' programs as their specs run them (trees, bldtile, terrainsub), pass their
    /// command lines (crate::cli): every one of them a program of this crate's.
    #[test]
    fn the_tasks_programs_pass_their_command_lines() {
        let u = crate::legacy::TileId::parse("6/1/1").unwrap();
        let mut runs: Vec<serde_json::Value> =
            crate::unit::tail(u, true, true).iter().map(|r| serde_json::to_value(r).unwrap()).collect();
        let none = BTreeMap::new();
        for spec in [
            // (A square south of the equator first: its negative top a value, not a flag.)
            crate::trees::task::spec(&[(1, 1), (2, 1)], &[(-10, -80), (40, 10)], "v", &none),
            crate::bld::task::spec(crate::legacy::TileId::parse("8/1/1").unwrap(), "v", &none),
            crate::terrain_task::spec(&[(1, 1), (1, 2)], "v", 1),
        ] {
            runs.extend(spec["runs"].as_array().unwrap().iter().cloned());
        }
        for r in &runs {
            let cmd: Vec<String> = std::iter::once(format!("/app/{}", r["prog"].as_str().unwrap()))
                .chain(r["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()))
                .collect();
            crate::cli::check_command(&cmd).unwrap_or_else(|| panic!("{cmd:?}: not a program of this crate")).unwrap();
        }
        assert!(runs.len() >= 8, "{}", runs.len());
    }

    /// Task #843: a member takes its own Mac's tiles to come from the lead's `/work/tocome` answer
    /// (as `ToCome::answer` writes it), at the forecast's time; none from a lead still `making`.
    #[test]
    fn a_member_takes_its_own_tiles_to_come_as_cache_hints() {
        let placed = [
            forecast::Placed { lane: 0, step: "unit".into(), target: "6/33/22".into(), from: 1_000_060, until: 0 },
            forecast::Placed { lane: 1, step: "unit".into(), target: "6/33/23".into(), from: 1_000_000, until: 0 },
        ];
        let tc = forecast::ToCome::of(&placed, &["m4".to_string(), "mini".to_string()], 1_000_000);
        let (at, h) = upcoming_of(200, &tc.answer(), "m4").unwrap();
        let c = Path::new("/c");
        assert_eq!(at, 1_000_000);
        assert_eq!(h.when(c, &c.join("blobs/base/6-33-22.0123456789abcdef.base")), Some(1_000_060));
        assert_eq!(h.when(c, &c.join("blobs/base/6-33-23.0123456789abcdef.base")), None);
        assert_eq!(upcoming_of(200, &tc.answer(), "m1"), Some((1_000_000, room::Hints::default())));
        assert!(upcoming_of(200, &serde_json::json!({ "making": "no forecast yet" }), "m4").is_none());
    }

    #[test]
    fn a_terrain_run_reads_its_own_shards_archives() {
        let packs = Path::new("/c/aws-terrarium/packs");
        for (name, ours) in [
            ("3-1-2.0123456789abcdef.tiles", true),
            ("6-8-16.0123456789abcdef.tiles", true),
            ("6-15-23.0123456789abcdef.tiles", true),
            ("6-16-16.0123456789abcdef.tiles", false),
            ("3-1-3.0123456789abcdef.tiles", false),
            ("root.0123456789abcdef.tiles", false),
        ] {
            assert_eq!(terrain_reads("terrain-hi 3/1/2", &packs.join(name)), ours, "{name}");
        }
        // Not a terrain run's, or not an archive copy.
        assert!(!terrain_reads("unit 6/8/16", &packs.join("6-8-16.0123456789abcdef.tiles")));
        assert!(!terrain_reads("terrain-root terrain-root", &packs.join("root.0123456789abcdef.tiles")));
        assert!(!terrain_reads("terrain-hi 3/1/2", Path::new("/c/aws-terrarium/12/2048/1500.png")));
        // A z6 tile its z6 tile's; an assembly its z3 tile's.
        assert!(
            terrain_reads("terrain-hi 6/8/16", &packs.join("6-8-16.0123456789abcdef.tiles"))
                && !terrain_reads("terrain-hi 6/8/16", &packs.join("6-8-17.0123456789abcdef.tiles"))
                && !terrain_reads("terrain-hi 6/8/16", &packs.join("3-1-2.0123456789abcdef.tiles"))
        );
        assert!(
            terrain_reads("terrain-lo 3/1/2", &packs.join("3-1-2.0123456789abcdef.tiles"))
                && !terrain_reads("terrain-lo 3/1/2", &packs.join("6-8-16.0123456789abcdef.tiles"))
        );
    }

    #[test]
    fn plans_with_the_hand_offs_waiting_in_the_journal() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("root"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let a = agent(&store::nasfs::NasRoot::local_for_test(&root), &home);
        let mut keys = build::Keys::default();
        keys.record("unit", &[("6/1/1".to_string(), "k1".to_string())]);
        keys.save(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        // Hand-offs the coordinator journaled here: they count as built until drained.
        let done = |u: &str, k: &str| crate::handoff::Handoff {
            done: Some(("unit".into(), vec![(u.into(), k.into())])),
            ..Default::default()
        };
        crate::handoff::write(&home.join("coord/journal/m1"), &done("6/1/2", "k2")).unwrap();
        crate::handoff::write(&home.join("coord/journal/m1"), &done("6/1/3", "k3")).unwrap();
        // The files they save count too (a tree cover z6 tile's mid: build::tree_work), but one a
        // later hand-off removes.
        let save = |l: &str, c: Option<&str>| crate::handoff::Handoff {
            changes: [(l.to_string(), c.map(str::to_string))].into(),
            ..Default::default()
        };
        crate::handoff::write(
            &home.join("coord/journal/m1"),
            &save("work/trees-mid/6-28-16", Some("work/trees-mid/6-28-16.1111111111111111.sect")),
        )
        .unwrap();
        crate::handoff::write(
            &home.join("coord/journal/m1"),
            &save("layers/trees-cover/hi/6-28-17", Some("layers/trees-cover/hi/6-28-17.2222222222222222.pack")),
        )
        .unwrap();
        crate::handoff::write(&home.join("coord/journal/m1"), &save("layers/trees-cover/hi/6-28-17", None)).unwrap();
        let k = a.planning_keys_here(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        assert_eq!(
            (k.recorded("unit", "6/1/1"), k.recorded("unit", "6/1/2"), k.recorded("unit", "6/1/3")),
            (Some("k1"), Some("k2"), Some("k3"))
        );
        assert_eq!(k.handed.iter().collect::<Vec<_>>(), ["work/trees-mid/6-28-16"]);
        assert!(
            build::Keys::load(&store::nasfs::NasRoot::local_for_test(&root)).handed.is_empty(),
            "never in the keys saved"
        );
    }

    fn agent(root: &store::nasfs::NasRoot, home: &Path) -> Agent {
        test_agent(Options {
            root: Some(root.clone()),
            home: home.to_path_buf(),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap()
    }

    /// Task #780: the owner's redo, as the lead's coordinator takes it (`/work/redo`), kept in its
    /// agent's folder (not by a dry run), planned as a job of the `redo` step whose done record
    /// names "<step> <target>", and dropped once that job succeeded; a failure keeps it.
    #[test]
    fn a_redo_ask_is_kept_run_as_a_job_and_dropped_when_done() {
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(&d.path().join("nas"));
        let home = d.path().join("home");
        let mut a = agent(&root, &home);
        let co = std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0);
        a.coord = Some(co.clone());
        let ask = redo::Ask {
            n: 7,
            step: "terrain-hi".into(),
            targets: vec!["6/18/22".into(), "6/18/23".into()],
            by: "t".into(),
            ..Default::default()
        };
        co.shared.lock().unwrap().redo_asks.push(ask.clone());
        a.planner().tend_redo();
        assert!(redo::pending(&home).unwrap().is_empty(), "a dry run's agent takes no ask");
        assert_eq!(co.take_redo_asks(), [ask.clone()], "left for the agent that runs");
        a.o.dry_run = false;
        co.shared.lock().unwrap().redo_asks.push(ask.clone());
        let seen = a.planner().plan_seen(&root);
        a.planner().tend_redo();
        assert_eq!(redo::pending(&home).unwrap(), [ask.clone()]);
        assert_ne!(a.planner().plan_seen(&root), seen, "planned again");
        let jobs = a.planner().redo_jobs(&root);
        assert_eq!(jobs.len(), 1);
        let j = &jobs[0];
        assert_eq!(j.id, "redo 7");
        assert_eq!(
            j.record,
            Some(build::Work {
                step: "redo".into(),
                targets: vec![
                    ("terrain-hi 6/18/22".into(), String::new()),
                    ("terrain-hi 6/18/23".into(), String::new())
                ]
            })
        );
        assert!(
            j.cmd.ends_with(&["terrain-hi".to_string(), "6/18/22".into(), "6/18/23".into()])
                && !j.cmd.contains(&"--run".to_string())
        );
        a.finished(&j.id, &j.what, false, 5, String::new());
        assert_eq!(redo::pending(&home).unwrap().len(), 1, "a failure keeps the ask");
        a.finished(&j.id, &j.what, true, 5, String::new());
        assert!(redo::pending(&home).unwrap().is_empty() && !home.join(redo::ASKS).exists());
        assert!(a.planner().redo_jobs(&root).is_empty());
        // Task #832: a restore's ask, its job's done record each target with its key, merged as a
        // record only over the key checked (`build::Keys::record`).
        let r = |t: &str, f: &str, to: &str| redo::Restore { target: t.into(), from: f.into(), to: to.into() };
        let restore = vec![
            r("6/18/22", "aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
            r("6/18/23", "cccccccccccccccc", "dddddddddddddddd"),
        ];
        let (step, restore) = redo::checked_restore("terrain-hi", &restore).unwrap();
        assert!(redo::checked_restore("terrain-hi", &[r("6/1/1", "x", "y")]).is_err(), "keys are hex");
        let ask =
            redo::Ask { n: 8, step, targets: vec!["6/18/22".into(), "6/18/23".into()], restore, ..Default::default() };
        co.shared.lock().unwrap().redo_asks.push(ask.clone());
        a.planner().tend_redo();
        let jobs = a.planner().redo_jobs(&root);
        let mut done = build::Keys::default();
        done.record(
            "terrain-hi",
            &[("6/18/22".into(), "aaaaaaaaaaaaaaaa".into()), ("6/18/23".into(), "eeeeeeeeeeeeeeee".into())],
        );
        let w = jobs[0].record.clone().unwrap();
        assert!(jobs[0].what.starts_with("Restoring 2 terrain-hi"), "{}", jobs[0].what);
        done.record(&w.step, &w.targets);
        assert_eq!(done.recorded("terrain-hi", "6/18/22"), Some("bbbbbbbbbbbbbbbb"));
        assert_eq!(done.recorded("terrain-hi", "6/18/23"), Some("eeeeeeeeeeeeeeee"), "built since: its own");
    }

    /// Task #679: only the agent that runs the jobs writes GC's depth targets; a planning-only
    /// agent beside it (no lock, a dry run) neither writes nor removes them.
    #[test]
    fn only_the_running_agent_writes_the_depth_targets() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (store::nasfs::NasRoot::local_for_test(&d.path().join("nas")), d.path().join("home"));
        let mut a = agent(&root, &home);
        a.o.dry_run = false;
        assert!(a._lock.is_some());
        let k = gc::DepthKeep { at: now_s(), lo: [(0, 4)].into(), hi: [(1, 33)].into() };
        *a.planner().depth_keep.borrow_mut() = Some(k.clone());
        let p = a.planner().keep_depth_targets();
        assert_eq!(gc::DepthKeep::read(&p, now_s()).unwrap(), Some(k));
        // Beside it, without the lock: its own targets (another app's) or none, the file untouched.
        let mut b = agent(&root, &home);
        assert!(b._lock.is_none() && b.o.dry_run);
        let before = std::fs::read(&p).unwrap();
        *b.planner().depth_keep.borrow_mut() = Some(gc::DepthKeep { at: now_s(), ..Default::default() });
        b.planner().keep_depth_targets();
        b.planner().depth_keep.borrow_mut().take();
        b.planner().keep_depth_targets();
        assert_eq!(std::fs::read(&p).unwrap(), before);
        // The running agent finding none: removed.
        a.planner().depth_keep.borrow_mut().take();
        a.planner().keep_depth_targets();
        assert!(!p.exists());
    }

    /// The build page's regions made off the loop, waited for and taken in.
    fn settle(ag: &mut Agent) {
        let p = ag.planner();
        while p.regions_making.borrow().as_ref().is_some_and(|h| !h.is_finished()) {
            std::thread::yield_now();
        }
        p.take_regions();
    }

    #[test]
    fn the_build_pages_regions_are_made_again_with_a_new_catalogue_and_given_to_the_page() {
        let (d, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let recipes = vec![recipes::Recipe { id: "a".into(), name: "Alpha".into(), outline: vec!["osm:5".into()] }];
        let mut m = BTreeMap::new();
        crate::inputs::regions::accept_fixture(
            &store::nasfs::NasRoot::local_for_test(root),
            root.into(),
            &recipes,
            &mut m,
        );
        let a = crate::inputs::open(&store::nasfs::NasRoot::local_for_test(root), &m, crate::inputs::regions::UNIT)
            .unwrap()
            .unwrap();
        let mut ag = agent(&store::nasfs::NasRoot::local_for_test(root), &home.path().join("home"));
        // No catalogue yet: its name and crumbs, no size.
        ag.planner().regions_view(&store::nasfs::NasRoot::local_for_test(root), &a, &recipes);
        settle(&mut ag);
        let v = ag.planner().regions_view.borrow().as_ref().unwrap().1.clone();
        assert_eq!(
            (v["catalog"].clone(), v["regions"][0]["crumbs"].clone()),
            (serde_json::Value::Null, serde_json::json!(["Alpha"]))
        );
        assert_eq!(v["regions"][0]["bytes"], serde_json::Value::Null);
        // A map recording it over z6 tile 6/40/31, whose base pack weighs 777 bytes.
        let mut c = store::catalogue::Catalogue::new(1);
        c.base.insert("6/40/31".into(), "base/6-40-31".into());
        let f = store::catalogue::FileRef {
            file: "base/6-40-31.1111111111111111.base".into(),
            size: 777,
            fmt: 0,
            extra: BTreeMap::new(),
        };
        c.files.insert("base/6-40-31".into(), f);
        c.coverage = serde_json::json!({ "regions": [{ "id": "a", "name": "Alpha", "outline": ["osm:5"],
            "shapes": { "osm:5": [[[[50.0, 0.5], [50.1, 0.5], [50.1, 0.6], [50.0, 0.6], [50.0, 0.5]]]] } }] });
        // (The map as a lead without its log reads it: the NAS's snapshot, docs/maplog.md §5.2.)
        let nas = store::nasfs::NasRoot::local_for_test(root);
        store::map::write_nas_snapshot(&nas, &store::map::Map::of(&c).unwrap()).unwrap();
        ag.planner().regions_view(&store::nasfs::NasRoot::local_for_test(root), &a, &recipes);
        settle(&mut ag);
        let v = ag.planner().regions_view.borrow().as_ref().unwrap().1.clone();
        assert_eq!((v["catalog"].as_u64(), v["regions"][0]["bytes"].as_u64()), (Some(1), Some(777)));
        // (Made once for the same version and catalogue: not again, even had it failed.)
        ag.planner().regions_view(&store::nasfs::NasRoot::local_for_test(root), &a, &recipes);
        assert!(ag.planner().regions_making.borrow().is_none());
    }

    #[test]
    fn a_helper_asks_only_for_what_its_disk_has_room_for() {
        let gb = |n: u64| n << 30;
        let all = |tasks: &[&'static str]| [&steps::SHARED[..], tasks].concat();
        // The shared steps need no room of their own (their grant's measured disk is reserved
        // for them, and no fixed reserve: task #421); the tasks theirs and their margin.
        assert_eq!(helper_steps(gb(20), gb(10), 0), all(&["tail", "terrainsub", "treeblock", "bldtile"]));
        assert_eq!(helper_steps(gb(10), gb(5), 0), all(&["tail", "terrainsub", "treeblock", "bldtile"]));
        // (Tree cover's rows and terrain's subtrees need 1 GB.)
        assert_eq!(helper_steps(gb(3), gb(2), 0), all(&["terrainsub", "treeblock"]));
        assert_eq!(helper_steps(gb(1), 0, 0), all(&[]));
        // The owner's disk room target stays free past them: 70 GB free with a 60 GB target leaves
        // room for every one; with a 69 GB target, the shared steps alone; past what's free, none.
        assert_eq!(helper_steps(gb(70), 0, gb(60)), all(&["tail", "terrainsub", "treeblock", "bldtile"]));
        assert_eq!(helper_steps(gb(70), 0, gb(69)), all(&[]));
        assert!(helper_steps(gb(70), 0, gb(71)).is_empty());
    }
    /// Beside a running job (#703): only the work whose need is free now past the room target and
    /// the jobs' reservations is asked for, so none is granted only to be given back. The M4, 35 GB
    /// free, a 20 GB target and 12 GB reserved by its bldtiles jobs: tails and bldtiles (5 GB) not
    /// asked for, treeblocks and terrainsubs (1 GB) and the shared steps (their own room granted) are.
    #[test]
    fn a_helper_asks_beside_its_jobs_only_for_what_starts() {
        let gb = |n: u64| n << 30;
        let all = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let can = all(&["bldtiles", "tail", "bldtile", "treeblock", "terrainsub"]);
        assert_eq!(room_beside(can.clone(), gb(35), gb(32)), all(&["bldtiles", "treeblock", "terrainsub"]));
        assert_eq!(room_beside(can.clone(), gb(35), gb(20)), can);
        assert_eq!(room_beside(can, gb(19), gb(20)), all(&[]));
    }

    /// A helper's task needs its kind's room, not a tail's whatever it is (#703: a terrainsub held
    /// for 5 GB though asked for with 1 GB, given back).
    #[test]
    fn a_task_needs_its_kinds_room() {
        // (A task's job named by its kind, as `take_grant` names it: "<kind> <unit>", task #905.)
        let spec = |kind: &str, unit: &str| JobSpec {
            id: JobId { step: kind, rest: Some(unit) }.to_string(),
            what: String::new(),
            cmd: Vec::new(),
            needs: Needs { nas: true },
            restart_after_sleep: false,
            record: None,
        };
        let want = [crate::terrain_task::KIND, crate::trees::task::KIND, crate::bld::task::KIND, "tail"];
        let kinds: Vec<_> = want
            .iter()
            .map(|k| {
                let s = spec(k, "6/1/2");
                assert_eq!(step_of(&s.id).as_deref(), Some(*k), "the Machines tab's step");
                (task_kind(&s).to_string(), helper_need(task_kind(&s)) >> 30)
            })
            .collect();
        assert_eq!(kinds, want.iter().zip([1, 1, 5, 5]).map(|(k, n)| (k.to_string(), n)).collect::<Vec<_>>());
        // A job adopted from an agent before #849, "task <lease>": a tail's room, the most.
        let old = JobSpec { id: "task 3".into(), ..spec("tail", "") };
        assert_eq!((task_kind(&old), helper_need(task_kind(&old)) >> 30), ("tail", 5));
    }

    /// The slow-plan fake (docs/agent-parts.md §7): a plan that sleeps first, as long as asked.
    #[test]
    fn the_slow_plan_fake_makes_a_plan_take_its_time() {
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(&d.path().join("nas"));
        let mut a = agent(&root, &d.path().join("home"));
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        a.planner().slow_plan = Some(Duration::from_millis(300));
        let t = Instant::now();
        a.planner().plan(&root, &c, &mut Vec::new());
        assert!(t.elapsed() >= Duration::from_millis(300));
    }

    impl Agent {
        /// The hold's asks taken up as a loop's round takes them: control's look (in line), then
        /// the NAS's part.
        pub(super) fn hold_asks(&mut self, root: Option<&store::nasfs::NasRoot>) {
            self.control.look();
            self.hold_on_nas(root);
        }
    }

    /// A lead's agent for the parts' tests (docs/agent-parts.md §7): its own NAS and home, its lock
    /// taken, a single loop's (its parts started by the test).
    fn parts_agent(d: &Path) -> (Agent, PathBuf) {
        let root = d.join("nas");
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: d.join("home"),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        (a, root)
    }

    /// The local status' beat as it changes while `busy` runs (on this thread) for `ran` at least:
    /// each change's time, sampled every 100 ms on a thread of the test's.
    fn beats_while(status: PathBuf, ran: Duration, mut busy: impl FnMut()) -> Vec<(Duration, u64)> {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let t0 = Instant::now();
        let sampler = std::thread::spawn(move || {
            let mut seen: Vec<(Duration, u64)> = Vec::new();
            while stopped.recv_timeout(Duration::from_millis(100)).is_err() {
                let beat =
                    std::fs::read(&status).ok().and_then(|b| serde_json::from_slice::<Status>(&b).ok()).map(|s| s.beat);
                if let Some(b) = beat.filter(|b| seen.last().is_none_or(|l| l.1 != *b)) {
                    seen.push((t0.elapsed(), b));
                }
            }
            seen
        });
        while t0.elapsed() < ran {
            busy();
        }
        stop.send(()).unwrap();
        let seen = sampler.join().unwrap();
        // (Within the beat's period and a second for the sampler and a loaded Mac, all along.)
        let most = beat::EVERY + Duration::from_millis(1500);
        let mut at = Duration::ZERO;
        for (t, _) in &seen {
            assert!(*t - at <= most, "a beat {:?} after the last: {seen:?}", *t - at);
            at = *t;
        }
        assert!(t0.elapsed() - at <= most && at >= ran - most, "{seen:?}");
        eprintln!("beats {:?}", seen.iter().map(|(t, _)| t.as_secs_f32()).collect::<Vec<_>>());
        seen
    }

    /// Task #871 step 2: the heartbeat advances every 10 s while every NAS call hangs (the slow-NAS
    /// fake), for 2 min, the loop going round on it meanwhile; the NAS's copy never written then.
    #[test]
    fn beat_steady_while_the_nas_hangs() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        let shared = root.join("state/status.json");
        let before = std::fs::read(&shared).unwrap();
        a.beat.spawn();
        let hung = nasread::fake::hang(&root);
        let seen = beats_while(a.o.home.join("status.json"), Duration::from_secs(120), || {
            a.round().ok();
        });
        assert!(seen.len() >= 12, "{seen:?}");
        assert_eq!(std::fs::read(&shared).unwrap(), before, "no beat reached the hung NAS");
        drop(hung);
    }

    /// Task #871 step 2: the heartbeat advances every 10 s while a plan runs 3 min (the slow-plan
    /// fake), the loop in it all along; the status says how old the loop's part is.
    #[test]
    fn beat_steady_while_planning() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        a.beat.spawn();
        a.planner().slow_plan = Some(Duration::from_secs(180));
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let r = store::nasfs::NasRoot::local_for_test(&root);
        let status = a.o.home.join("status.json");
        let seen = beats_while(status.clone(), Duration::from_secs(180), || {
            a.planner().plan(&r, &c, &mut Vec::new());
        });
        assert!(seen.len() >= 18, "{seen:?}");
        let s: Status = serde_json::from_slice(&std::fs::read(&status).unwrap()).unwrap();
        assert!(s.loop_age_s.is_some_and(|age| age >= 170), "{:?}", s.loop_age_s);
    }

    /// A pause asked (as the menu bar asks: the hold ask's file) on another thread, `after` from
    /// now, while `busy` runs here for `ran`: how long until it was answered, the local status
    /// said it (pausing, the job finishing named) and the job's channel said drain. The caller's
    /// control never sweeps (`asks::Handle::never_sweep`), so an answer at all was its watcher's
    /// event, whatever the loop was in: no wall bound a loaded Mac could miss (#910).
    fn pause_while(a: &mut Agent, after: Duration, ran: Duration, mut busy: impl FnMut(&mut Agent)) -> Duration {
        let home = a.o.home.clone();
        let chan = home.join("job-control");
        std::fs::write(&chan, b"run").unwrap();
        a.control.jobs.set(vec![slots::JobRef::on_chan(chan.clone())]);
        let asker = std::thread::spawn(move || {
            // (sleep ok: the ask comes once the loop is deep in its plan or its hung calls)
            std::thread::sleep(after);
            let t = Instant::now();
            let ask = crate::control::request_hold(&home, None, Some(crate::control::Hold::new("the menu bar on m1")))
                .unwrap();
            let said = || {
                std::fs::read(home.join("status.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Status>(&b).ok())
                    .and_then(|s| s.hold)
                    .is_some_and(|h| h.stage == crate::control::HoldStage::Pausing && h.finishing == ["unit 6/1/2"])
            };
            store::testkit::eventually("the pause answered and shown", || {
                crate::control::hold_answer(&home, &ask).is_some_and(|a| a.ok)
                    && said()
                    && std::fs::read(&chan).unwrap() == b"drain"
            });
            t.elapsed()
        });
        let t0 = Instant::now();
        while t0.elapsed() < ran {
            busy(a);
        }
        let took = asker.join().unwrap();
        eprintln!("the pause answered and shown in {took:?}");
        took
    }

    /// The loop's view as a job runs: what the beat says a pause finishes.
    fn a_job_running(a: &mut Agent) {
        let mut v = a.beat.view.get().as_ref().clone().unwrap();
        v.status.set_jobs(
            vec![JobView {
                id: "unit 6/1/2".into(),
                what: "z6 tile 6/1/2".into(),
                started: now_s(),
                paused: None,
                pausing: None,
                tail: String::new(),
                parts: Vec::new(),
                part: None,
                progress: None,
                mem_mb: None,
                threads: None,
                waiting: None,
                slot: Some(0),
                live: None,
            }],
            true,
        );
        a.beat.publish(v);
        a.control.jobs.set(vec![slots::JobRef::on_chan(a.o.home.join("job-control"))]);
    }

    /// Task #871 step 3: a pause on a hung NAS (every call hung, the loop going round on it):
    /// answered, said "pausing" in the status and the job told to drain by control's watcher and
    /// the beat it tells, never the loop.
    #[test]
    fn pause_shows_within_a_second_on_a_hung_nas() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        // (No sweep, no beat on its timer: the answer, the drain and the status' "pausing" were
        // control's event and its telling the beat, however long a loaded Mac took: #910.)
        a.control.never_sweep();
        a.beat.never_tick();
        a.control.spawn();
        a.beat.spawn();
        let hung = nasread::fake::hang(&root);
        a_job_running(&mut a);
        pause_while(&mut a, Duration::from_secs(5), Duration::from_secs(30), |a| {
            a.round().ok();
            // (The loop's own view has no job: the job said running again, as a long job's.)
            a_job_running(a);
        });
        drop(hung);
    }

    /// Task #871 step 4: the share's mount hung (a finder that never answers): the loop goes round
    /// in moments all along, NAS work waits ("the NAS is mounting (stuck since …)"), the beat and a
    /// pause go on; the root comes back once the mount answers.
    #[test]
    fn a_hung_mount_holds_only_nas_work() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        let (release, go) = std::sync::mpsc::channel::<()>();
        let go = std::sync::Mutex::new(go);
        let found = root.clone();
        a.nas = nas::Handle::new(
            std::sync::Arc::new(move |_| {
                go.lock().unwrap().recv().ok();
                Some(store::nasfs::NasRoot::local_for_test(found.clone()))
            }),
            false,
            |_| true,
        );
        a.nas.spawn();
        // (No sweep: the pause below answered at all is control's watcher's: #910.)
        a.control.never_sweep();
        a.control.spawn();
        a.beat.spawn();
        let status = a.o.home.join("status.json");
        // (The finder never answers until released: a round that waited on it would never end.)
        let t = Instant::now();
        for _ in 0..20 {
            a.round().unwrap();
        }
        eprintln!("20 rounds on a hung mount in {:?}", t.elapsed());
        let said = || std::fs::read_to_string(&status).unwrap_or_default();
        store::testkit::eventually("the stuck mount said", || said().contains("the NAS is mounting (stuck since"));
        assert!(a.root().is_none(), "no root while the mount hangs");
        // A pause goes on: answered by control while the mount still hangs.
        let ask =
            crate::control::request_hold(&a.o.home, None, Some(crate::control::Hold::new("the menu bar"))).unwrap();
        store::testkit::eventually("the pause answered", || {
            crate::control::hold_answer(&a.o.home, &ask).is_some_and(|x| x.ok)
        });
        assert!(a.root().is_none(), "answered while the mount hangs");
        drop(release);
        store::testkit::eventually("the root back", || a.root().is_some());
    }

    /// A job of `secs` seconds started in slot `k` (a unit job of target `target`), published.
    fn a_real_job(a: &mut Agent, k: usize, target: &str, secs: u64) -> i32 {
        let spec = JobSpec {
            id: format!("unit {target}"),
            what: format!("z6 tile {target}"),
            cmd: vec!["/bin/sh".into(), "-c".into(), format!("sleep {secs}")],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "unit".into(), targets: vec![(target.into(), "k".into())] }),
        };
        std::fs::create_dir_all(&a.o.home).unwrap();
        let log = a.o.home.join(format!("job-{k}.log"));
        let r = Running::start(spec, 1, &[], log, &a.record_path(k), None, None).unwrap();
        let pgid = r.pgid;
        a.slots[k].running = Some(r);
        a.publish_jobs();
        pgid
    }

    /// Task #871 step 5: the NAS hung (every call), a job's end wakes the loop at once (**jobs**'
    /// reaper), and the loop, going round all along, records it and frees its slot.
    #[test]
    fn a_job_is_reaped_while_the_nas_hangs() {
        let d = tempfile::tempdir().unwrap();
        // (No round first: it may start a job of its own in the slot.)
        let (mut a, root) = parts_agent(d.path());
        a.spawn_jobs_part();
        a.control.spawn();
        let hung = nasread::fake::hang(&root);
        a_real_job(&mut a, 0, "6/1/2", 2);
        let t = Instant::now();
        // (No round runs meanwhile: woken for the end at all is the reaper's telling: #910.)
        store::testkit::wait_for("the loop woken for the job's end", || {
            a.control.woken.try_recv().ok().filter(|m| *m == bus::ControlMsg::Ended)
        });
        let woke = t.elapsed();
        eprintln!("a 2-s job's end woke the loop after {woke:?}");
        assert!(woke >= Duration::from_secs(2), "{woke:?}");
        let r = Instant::now();
        while a.slots[0].running.is_some() {
            a.round().unwrap();
            // (A round on a hung NAS waits out each NAS call's deadline, 20 s: about 100 s.)
            assert!(r.elapsed() < Duration::from_secs(400), "the slot never freed");
        }
        eprintln!("recorded and its slot freed in {:?} on the hung NAS", r.elapsed());
        assert!(a.mem.recent.iter().any(|j| j.id == "unit 6/1/2"), "its end recorded");
        drop(hung);
    }

    /// Task #871 step 5: a cancel asked (as `scenic job cancel` asks: its file) while the loop is
    /// in a 30-s plan (the slow-plan fake): the job's group stopped by **jobs**' thread, no round
    /// running; the loop, its plan done, records it as cancelled (not failed) and answers.
    #[test]
    fn a_cancel_is_acted_on_within_a_second_while_planning() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        // (No round first: it may start a job of its own in the slot.)
        a.spawn_jobs_part();
        a.control.spawn();
        let pgid = a_real_job(&mut a, 0, "6/1/2", 600);
        a.planner().slow_plan = Some(Duration::from_secs(30));
        let home = a.o.home.clone();
        let asker = std::thread::spawn(move || {
            // (sleep ok: the ask comes once the loop is deep in its plan)
            std::thread::sleep(Duration::from_secs(5));
            let t = Instant::now();
            let ask = crate::control::Cancel {
                target: "6/1/2".into(),
                step: Some("unit".into()),
                rerun_after_update: false,
                by: "scenic job cancel on m1".into(),
                at: now_s(),
            };
            crate::control::request_cancel(&home, &ask).unwrap();
            store::testkit::eventually("the job stopped", || {
                !crate::sys::signal_group(pgid, crate::sys::Signal::Probe)
            });
            eprintln!("the cancel stopped the job in {:?}", t.elapsed());
        });
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let r = store::nasfs::NasRoot::local_for_test(&root);
        let t = Instant::now();
        a.planner().plan(&r, &c, &mut Vec::new());
        assert!(t.elapsed() >= Duration::from_secs(29), "the plan was slow all along");
        // (No round before this: the job stopped at all was **jobs**' thread's doing, never the
        // loop's, however long a loaded Mac took: #910.)
        asker.join().unwrap();
        a.round().unwrap();
        // (Its slot free of it: the round may start another job there.)
        assert!(a.slots[0].running.as_ref().is_none_or(|r| r.pgid != pgid));
        let j = a.mem.recent.iter().find(|j| j.id == "unit 6/1/2").expect("recorded");
        assert!(j.cancelled, "recorded as cancelled, not failed");
        assert!(crate::control::peek_cancels(&a.o.home).is_empty());
    }

    /// Task #871 step 5: nothing starts under a hold, even one taken up after the round began: a
    /// held Mac's granted job waits, said why.
    #[test]
    fn nothing_starts_under_a_hold() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, _root) = parts_agent(d.path());
        a.round().unwrap();
        assert!(a.set_hold(Some(crate::control::Hold::new("the menu bar on m1")), now_s()));
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut waiting = Vec::new();
        // (A free slot: the step's coordinator may have granted this Mac a duty in the first.)
        let k = (0..a.slots.len()).find(|&k| a.slots[k].running.is_none()).expect("a free slot");
        assert!(!a.try_start_said(k, waiting_job("unit 6/1/2"), &c, None, &mut waiting));
        assert!(a.slots[k].running.is_none(), "nothing started");
        assert!(waiting.iter().any(|w| w.why.contains("paused")), "{waiting:?}");
    }

    /// The slow-plan fake's run in `plan_running_long`: what a grant or a round that waited for the
    /// plan would take (#910: `ended_before` measures against it, not a "fast" bound).
    const PLAN_LONG: Duration = Duration::from_secs(180);

    /// The last plan, as a test sets it (`jobs`, its own: granted to this Mac alone), offered and
    /// published; then a 3-min plan under way on **plan**'s thread (the slow-plan fake), kicked as
    /// a job's end kicks it. The last plan's number.
    fn plan_running_long(a: &mut Agent, root: &Path, jobs: Vec<JobSpec>) -> u64 {
        // (This Mac's coordinator, its room known: what grants its own jobs.)
        let (co, _port) = crate::coord::start_for_test(&root.join("../coord"), "m4", "");
        a.coord = Some(std::sync::Arc::new(co));
        a.free_set = Some(100 << 30);
        a.mem_set = Some((16 << 10, 12 << 10));
        let p = a.planner();
        p.n += 1;
        p.jobs = jobs;
        p.offer_plan(&p.jobs.clone());
        p.publish();
        p.slow_plan = Some(PLAN_LONG);
        let n = p.n;
        a.plan.spawn();
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        a.tell_plan(Some(&store::nasfs::NasRoot::local_for_test(root)), &c, true);
        store::testkit::eventually("the 3-min plan under way", || a.plan.busy());
        n
    }

    /// A job of this Mac's own (no record: granted to it alone), `id`.
    fn own_job(id: &str) -> JobSpec {
        JobSpec { id: id.into(), what: id.into(), ..waiting_job(id) }
    }

    /// Task #871 step 6: a plan of 3 min (the slow-plan fake, on **plan**'s thread) delays no
    /// grant, no pause and no heartbeat: grants come from the last plan at once, each carrying its
    /// number, a lease never granted twice; a pause is answered and said within a second; the beat
    /// goes on every 10 s; each round takes moments; and the plan is still under way at the end.
    #[test]
    fn a_slow_plan_delays_no_grant_pause_or_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        let n = plan_running_long(&mut a, &root, vec![own_job("a-test one"), own_job("a-test two")]);
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let r = store::nasfs::NasRoot::local_for_test(&root);
        // Grants from the last plan, at once, each carrying its number; never the same lease twice.
        let view = a.plan.view.get();
        let mut w = Vec::new();
        let t = Instant::now();
        let first = a.lead_grants(&view.jobs, view.n, &c, &r, &mut w);
        store::testkit::ended_before(t, PLAN_LONG, "a grant while a 3-min plan runs");
        assert_eq!(first.iter().map(|g| g.2).collect::<Vec<_>>(), [n], "{w:?}");
        let t = Instant::now();
        let second = a.lead_grants(&view.jobs, view.n, &c, &r, &mut w);
        store::testkit::ended_before(t, PLAN_LONG, "a second grant while a 3-min plan runs");
        // (The other job waits beside the first's cold start; the first's lease is never granted again.)
        assert!(second.iter().all(|g| g.1.id != first[0].1.id), "a lease granted twice: {second:?}");
        for (k, _, _) in first.into_iter().chain(second) {
            a.end_lease(k, Outcome::Interrupted, &[], "the test");
        }
        // The rounds, granting and starting from the last plan, in moments.
        a.control.never_sweep();
        a.control.spawn();
        a.beat.spawn();
        for _ in 0..3 {
            let t = Instant::now();
            a.round().unwrap();
            store::testkit::ended_before(t, PLAN_LONG, "a round while a 3-min plan runs");
        }
        // A pause: answered and said within a second, the rounds going on.
        a_job_running(&mut a);
        pause_while(&mut a, Duration::from_secs(2), Duration::from_secs(8), |a| {
            a.round().ok();
            a_job_running(a);
        });
        // The beat: every 10 s.
        let seen = beats_while(a.o.home.join("status.json"), Duration::from_secs(30), || {
            a.round().ok();
        });
        assert!(seen.len() >= 3, "{seen:?}");
        // (Nothing waited for the plan: still under way, its last the one granted from.)
        assert!(a.plan.busy(), "the 3-min plan ended early");
        assert_eq!(a.plan.view.get().n, n);
        a.stop_jobs("the test");
        a.plan.stop(Duration::ZERO);
    }

    /// Task #871 step 6: a hung NAS (every call) holds only NAS work: the plan's thread in it, a
    /// grant from the last plan at once, a pause answered within a second, nothing started under
    /// the hold that pause sets.
    #[test]
    fn a_hung_nas_holds_only_nas_work_while_planning() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        let n = plan_running_long(&mut a, &root, vec![own_job("a-test one")]);
        let hung = nasread::fake::hang(&root);
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let r = store::nasfs::NasRoot::local_for_test(&root);
        let view = a.plan.view.get();
        let t = Instant::now();
        let mut w = Vec::new();
        let got = a.lead_grants(&view.jobs, view.n, &c, &r, &mut w);
        store::testkit::ended_before(t, nasread::DEADLINE, "a grant on a hung NAS");
        assert_eq!(got.iter().map(|g| g.2).collect::<Vec<_>>(), [n], "{w:?}");
        for (k, _, _) in got {
            a.end_lease(k, Outcome::Interrupted, &[], "the test");
        }
        a.control.never_sweep();
        a.control.spawn();
        a.beat.spawn();
        a_job_running(&mut a);
        pause_while(&mut a, Duration::from_secs(2), Duration::from_secs(6), |a| {
            a.round().ok();
            a_job_running(a);
        });
        // Under the hold: nothing granted, nothing started.
        let mut w = Vec::new();
        assert!(a.lead_grants(&view.jobs, view.n, &c, &r, &mut w).is_empty(), "{w:?}");
        assert!(!a.try_start_said(0, own_job("a-test one"), &c, None, &mut w));
        assert!(a.slots.iter().all(|s| s.running.is_none()), "nothing started under the hold");
        drop(hung);
        a.plan.stop(Duration::ZERO);
    }

    /// Task #871 step 6 (§5.4): a grant from a plan no newer than a cancel of its job or a target
    /// is dropped; one from a later plan, or an older lead's (no number), is not.
    #[test]
    fn a_grant_from_a_plan_before_a_cancel_is_dropped() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        let n = a.plan.view.get().n;
        assert!(n >= 1);
        a.take_cancel(
            crate::control::Cancel {
                target: "6/1/2".into(),
                step: Some("unit".into()),
                rerun_after_update: false,
                by: "a test".into(),
                at: now_s(),
            },
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
        );
        let spec = JobSpec {
            record: Some(build::Work { step: "unit".into(), targets: vec![("6/1/2".into(), "k".into())] }),
            ..waiting_job("unit 6/1/2")
        };
        assert!(a.cancelled_since(&spec, n).is_some_and(|w| w.contains("6/1/2")));
        assert_eq!(a.cancelled_since(&spec, n + 1), None);
        assert_eq!(a.cancelled_since(&spec, 0), None, "an older lead's grant: no number");
        assert_eq!(a.cancelled_since(&waiting_job("unit 6/1/3"), n), None);
        // (Plans are numbered past the time in ms: a lead started again numbers past every plan of
        // the one before, so a member's cancel noted at an old number drops none of its grants.)
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert_eq!(plan::next_n(0, t), 1_800_000_000_000);
        assert_eq!(plan::next_n(1_800_000_000_500, t), 1_800_000_000_501);
        assert!(n >= plan::next_n(0, std::time::SystemTime::now() - Duration::from_secs(60)));
    }

    /// Task #871 step 6, a mixed rollout: a grant without the plan's number (an older lead's) reads
    /// as 0; one with none (0) is written as an older lead writes it, for an older member.
    #[test]
    fn a_grants_plan_number_reads_across_app_versions() {
        let old = serde_json::json!({ "lease": 7, "ttl_s": 600, "kind": "job", "step": "unit", "targets": [["6/1/2", "k"]], "pass": "p" });
        let g: crate::coord::Grant = serde_json::from_value(old).unwrap();
        assert_eq!(g.plan, 0);
        assert!(serde_json::to_value(&g).unwrap().get("plan").is_none());
        let g = crate::coord::Grant { plan: 4, ..g };
        assert_eq!(serde_json::to_value(&g).unwrap()["plan"], 4);
    }

    /// Task #871 step 3: a pause during a plan (the slow-plan fake, 30 s): the same.
    #[test]
    fn pause_shows_within_a_second_while_planning() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, root) = parts_agent(d.path());
        a.round().unwrap();
        // (No sweep, no beat on its timer: the answer, the drain and the status' "pausing" were
        // control's event and its telling the beat, however long a loaded Mac took: #910.)
        a.control.never_sweep();
        a.beat.never_tick();
        a.control.spawn();
        a.beat.spawn();
        a_job_running(&mut a);
        a.planner().slow_plan = Some(Duration::from_secs(30));
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let r = store::nasfs::NasRoot::local_for_test(&root);
        pause_while(&mut a, Duration::from_secs(5), Duration::from_secs(30), |a| {
            a.planner().plan(&r, &c, &mut Vec::new());
        });
    }

    #[test]
    fn the_room_target_holds_jobs_and_frees_the_caches_toward_it() {
        room::TEST_FREE.with(|c| c.set(Some(40 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("sources")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: None,
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        // A 50 GB target, past what the disk has (40 GB free), which the (empty) caches can't make:
        // it waits, saying why; one keeping no target starts: the daily backup. (No fixed reserve
        // past the target, task #421.)
        room::set_target(&home, Some(50 << 30), "a test").unwrap();
        a.room_target = room::target(&home);
        let mut w = Vec::new();
        assert!(!a.try_start(
            0,
            job("terrain-hi 3/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut w
        ));
        assert_eq!(w.iter().map(|w| w.what.as_str()).collect::<Vec<_>>(), ["terrain-hi 3/1/1"], "{w:?}");
        assert!(w.iter().all(|w| w.why.contains("disk room target")), "{w:?}");
        a.slots[0].lease = None;
        assert!(a.try_start(
            0,
            job("backup daily"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut w
        ));
        assert_eq!(a.slots[0].running.as_ref().map(|r| r.spec.id.as_str()), Some("backup daily"));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots = Default::default();
        // Short of the target: the caches are due a freeing toward it; without the NAS, the status
        // says it can't be freed now.
        assert_eq!(a.toward_due(), Some(50 << 30));
        assert!(a.room_short(false).is_some_and(|s| s.contains("NAS isn't reachable")));
        // Tried again within ten minutes: held without making room again.
        let mut w = Vec::new();
        assert!(!a.try_start(0, job("unit 6/1/1"), &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut w));
        assert!(w[0].why.contains("disk room target"), "{w:?}");
        // (Its need past the target: past what the disk has.)
        assert!(a.disk_free() < a.need_of(0, &job("unit 6/1/1")) + a.floor());
        // A 5 GB target, which the disk has; it starts.
        room::set_target(&home, Some(5 << 30), "a test").unwrap();
        a.room_target = room::target(&home);
        a.floor_short = None;
        assert!(a.try_start(
            0,
            job("unit 6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
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
        std::fs::write(
            root.join("state/build/manifest.json"),
            br#"{"sources/osm/2026-09-28/filtered":"sources/osm/2026-09-28/filtered.0000000000000001.osm.pbf"}"#,
        )
        .unwrap();
        // (The freeing's thread sees a test's disk, 1 TB free unless it says: a target past it.)
        let big = (1u64 << 40) + (100 << 30);
        room::set_target(&home, Some(big), "a test").unwrap();
        a.room_target = room::target(&home);
        assert_eq!(a.toward_due(), Some(big));
        // (One loop at a time: the freeing waited for, its disk this test's.)
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(a.caches_task.is_none());
        let t = a.mem.toward.clone().unwrap();
        assert_eq!((t.target, t.bytes()), (Some(big), 1100), "{t:?}");
        assert!(
            !c.join("base/base/6-1-1.0000000000000001.base").exists()
                && !c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf").exists()
        );
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let (used, idle) = (
            home.join("cache/base/base/6-1-1.0000000000000001.base"),
            home.join("cache/base/base/6-1-2.0000000000000002.base"),
        );
        put(&used, &[0; 1000]);
        put(&idle, &[0; 500]);
        // A job runs (the daily backup: it keeps no target), holding one of the packs.
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(
            0,
            waiting_job("backup daily"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        store::cachefile::hold_existing(&used).unwrap();
        // A target past what's free (the freeing's thread sees this Mac's disk): freed toward now,
        // the job running, all but what it holds.
        let target = (1u64 << 40) + (100 << 30);
        room::set_target(&home, Some(target), "a test").unwrap();
        a.room_target = room::target(&home);
        assert!(!a.idle());
        assert_eq!(a.toward_due(), Some(target));
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: None,
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        std::fs::create_dir_all(home.join("cache/base/base")).unwrap();
        std::fs::write(home.join("cache/base/base/6-1-1.0000000000000001.base"), vec![0u8; 1000]).unwrap();
        // A target past what's free (the freeing's thread sees this Mac's disk), and a z6 tile held by
        // it: the caches are due a freeing toward the z6 tile's room past the target.
        let target = (1u64 << 40) + (100 << 30);
        let room = target;
        room::set_target(&home, Some(target), "a test").unwrap();
        a.room_target = room::target(&home);
        let mut w = Vec::new();
        assert!(!a.try_start(0, job("unit 6/1/1"), &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut w));
        a.slots[0].lease = None;
        assert!(a.idle() && w[0].why.contains("disk room target"), "{w:?}");
        assert_eq!(a.toward_due(), Some(room));
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        let f = a.mem.toward.clone().unwrap();
        assert_eq!((f.target, f.goal, f.bytes()), (Some(target), None, 1000), "{f:?}");
        // Done, and short of both: no freeing again within ten minutes, toward the z6 tile's room nor
        // toward the target, loop after loop (nor once the z6 tile's wait is over).
        for _ in 0..3 {
            a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(d.path().join("nas"))),
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
        let fail = |id| Tell::Fail { id, why: "x".into(), floors: Vec::new(), disks: Vec::new(), job_disk: 0 };
        a.tell_or_owe(fail(1));
        assert_eq!(a.owed.len(), 1, "no client: owed");
        a.client = Some(crate::coord::client::Client::at(vec![crate::coord::client::tests::dead()], "t".into(), "m1"));
        a.begin_round();
        a.tell_or_owe(fail(2));
        assert!(a.client.as_ref().unwrap().round_failed());
        a.tell_or_owe(Tell::GiveBack { id: 3, why: "x".into(), floors: Vec::new(), disks: Vec::new(), job_disk: 0 });
        a.tell_or_owe(Tell::Done(Box::new(crate::coord::Done { lease: 4, ..Default::default() })));
        assert_eq!(
            a.owed
                .iter()
                .map(|t| match t {
                    Tell::Fail { id, .. } | Tell::GiveBack { id, .. } => *id,
                    Tell::Done(d) => d.lease,
                })
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
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
        assert!(store::nasfs::is_live_path(Path::new("/Volumes/personal/projects/scenic-roads")));
        let d = tempfile::tempdir().unwrap();
        assert!(!store::nasfs::is_live_path(d.path()));
        let o = |root: Option<store::nasfs::NasRoot>| Options {
            root,
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: true,
            once: true,
            member: true,
        };
        assert!(std::panic::catch_unwind(|| test_agent(o(None))).is_err(), "no root");
        assert!(
            std::panic::catch_unwind(|| test_agent(o(Some(store::nasfs::NasRoot::local_for_test(
                "/Volumes/personal/projects/scenic-roads"
            )))))
            .is_err(),
            "the NAS"
        );
        if let Some(h) = std::env::var_os("HOME") {
            let live = Options {
                home: Path::new(&h).join("Library/Application Support/scenic"),
                ..o(Some(store::nasfs::NasRoot::local_for_test(d.path().join("nas"))))
            };
            assert!(std::panic::catch_unwind(|| test_agent(live)).is_err(), "the app's home");
        }
        assert!(
            std::panic::catch_unwind(|| store::nasfs::NasRoot::local_for_test("/Volumes/x/scenic-roads")).is_err(),
            "a share on the NAS"
        );
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(d.path().join("nas"))),
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
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
        c.offer(
            "2026-09-28",
            vec![crate::coord::Offer {
                step: "slope-hi".into(),
                targets: vec![("3/2/2".into(), "k".into(), 100)],
                job: None,
            }],
        );
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
        a.client = Some(crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{port}")],
            c.contact.token.clone(),
            "m1",
        ));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut w = Vec::new();
        let jobs = a.helper_job(&store::nasfs::NasRoot::local_for_test(&root), &cond, &mut w);
        assert_eq!(jobs.len(), 1, "{w:?}");
        let j = &jobs[0];
        // The lead's own command for slope, its saves handed off to its lease's outbox.
        assert_eq!(j.id, "slope-hi 3/2/2");
        assert!(j.cmd[1].starts_with("SCENIC_HANDOFF=") && j.cmd[2].starts_with("SCENIC_COSTS="));
        // (Its timings' record in its folder too, to go with its hand-off.)
        assert!(j.cmd[3].starts_with("SCENIC_TIMINGS=") && j.cmd[3].ends_with("/timings.json"));
        assert_eq!(&j.cmd[4..7], ["/app/scenic-build", "slope-hi", "--root"]);
        assert!(j.cmd.contains(&"3/2/2".to_string()));
        assert_eq!(j.record.as_ref().map(|r| r.step.as_str()), Some("slope-hi"));
        assert!(j.what.contains("for the lead"), "{}", j.what);
    }

    /// A Mac's slots, names and asks (task #240): a slot's files and worker by its number, the
    /// first's as before; it asks for its slots free.
    #[test]
    fn slots_are_named_by_number_and_an_agent_asks_for_its_free_ones() {
        assert_eq!((slot_suffix(0), slot_suffix(1), slot_suffix(4)), (String::new(), "-2".into(), "-5".into()));
        assert_eq!((slot_worker("m1", 0), slot_worker("m1", 2)), ("m1".into(), "m1 #3".into()));
        assert!(is_slot_of("m4 #2", "m4") && is_slot_of("m4 #7", "m4") && is_slot_of("m4", "m4"));
        assert!(!is_slot_of("m40", "m4") && !is_slot_of("m4 #1", "m4") && !is_slot_of("m1 #2", "m4"));
        assert_eq!(slot_workers("m4").len(), MAX_SLOTS);
        assert_eq!((job_slots(8), job_slots(2), job_slots(1000)), (6, 1, MAX_SLOTS));
        assert_eq!((want(6, 2), want(6, 7), want(6, 0)), (4, 0, 6));
    }

    /// Disk reservations on a member (task #242): a job granted with its measured disk reserves it
    /// whole until it starts, then less what it has written; the free space an ask says (and the
    /// owner's room-target goal) count them, and a slot holding nothing reserves nothing.
    #[test]
    fn a_members_granted_jobs_reserve_their_disk_until_their_lease_ends() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("nas")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(d.path().join("nas"))),
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
        let g: u64 = 1 << 30;
        a.free_set = Some(50 * g);
        assert_eq!((a.reserved(None), a.disk_room()), (0, 50 * g));
        // Granted, not started: 8 GB reserved whole; free 50 less 8 (no fixed reserve, task #421).
        a.slots[1].lease = Some(Held::Leased { lease: 7, dir: d.path().join("out") });
        a.slots[1].reserve_mb = Some(8 << 10);
        assert_eq!((a.reserved(None), a.reserved(Some(1)), a.disk_room()), (8 * g, 0, 42 * g));
        // A slot holding nothing reserves nothing, whatever it was last granted with.
        a.slots[0].reserve_mb = Some(100 << 10);
        assert_eq!(a.reserved(None), 8 * g);
        // With the owner's room target, room is made toward it and the reservations past it.
        a.room_target = Some(room::Target { bytes: 10 * g, by: String::new(), at: 0 });
        assert_eq!(a.goal(), 18 * g);
    }

    /// The owner's room target is the only floor (task #421): 49 GB free and a 20 GB target
    /// offer 29 GB for jobs, less the jobs' reservations; no target, all that's free. The
    /// capacity says no fixed reserve.
    #[test]
    fn the_room_target_alone_is_taken_off_the_disk_for_jobs() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("nas")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(d.path().join("nas"))),
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
        let g: u64 = 1 << 30;
        a.free_set = Some(49 * g);
        assert_eq!(a.disk_room(), 49 * g);
        a.room_target = Some(room::Target { bytes: 20 * g, by: String::new(), at: 0 });
        assert_eq!(a.disk_room(), 29 * g);
        a.slots[1].lease = Some(Held::Leased { lease: 7, dir: d.path().join("out") });
        a.slots[1].reserve_mb = Some(9 << 10);
        assert_eq!(a.disk_room(), 20 * g);
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let cap = a.capacity(&cond);
        assert_eq!((cap.free_mb, cap.target_mb, cap.reserved_mb, cap.disk_mb), (49 << 10, 20 << 10, 9 << 10, 20 << 10));
        assert_eq!(
            crate::coord::disk_words(&cap, cap.disk_mb),
            "49 GB free · 20 GB for jobs (20 GB room target, 9 GB reserved by its jobs)"
        );
    }

    /// Packing (task #240): a member asks for its slots free and runs every job granted, each in a
    /// slot of its own (its scratch, channel and costs its slot's); an update's drain waits for
    /// every slot; the memory guard, the jobs together past this Mac's limit (12 GB of 16: the M1),
    /// drains the newest, never the largest, one at a time and none more while what stays fits;
    /// one past the limit alone, the Mac short of memory, stopped; the slots shrink as jobs end.
    #[test]
    fn a_member_with_packing_runs_several_jobs_and_the_guard_drains_the_newest() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let apps = d.path().join("app");
        std::fs::create_dir_all(apps.join("v1")).unwrap();
        std::fs::create_dir_all(apps.join("v2")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        // The lead's coordinator offers six slope targets measured at 2 GB, 100 MB of disk and two
        // threads each.
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        let ts: Vec<String> = (1..=6).map(|i| format!("6/1/{i}")).collect();
        for t in &ts {
            let cost = crate::coord::Cost {
                peak_mb: 2000,
                secs: 600,
                v: crate::coord::cost_version("slope-hi"),
                sizes: [("threads".to_string(), 2)].into(),
                ..Default::default()
            };
            c.add_costs_by(&[(crate::coord::cost_key("slope-hi", t), cost)], "m1");
            c.add_disks(&[(crate::coord::cost_key("slope-hi", t), 100)]);
        }
        c.offer(
            "2026-09-28",
            vec![crate::coord::Offer {
                step: "slope-hi".into(),
                targets: ts.iter().map(|t| (t.clone(), format!("k {t}"), 0)).collect(),
                job: None,
            }],
        );
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: apps.join("v1"),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
        a.client = Some(crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{port}")],
            c.contact.token.clone(),
            "m1",
        ));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut w = Vec::new();
        assert!(a.takes_more());
        // (Its ask's memory the build's on its 16 GB (task #390): none set, the memory guard's
        // limit, 12 GB; set to 6 GB (`scenic memory 6`), within the limit, that: three of 2 GB fit.)
        assert_eq!(a.build_room(), memguard::limit_mb(16 << 10));
        buildmem::set(&home, Some(6 << 10), "a test").unwrap();
        a.build_memory = buildmem::read(&home);
        assert_eq!(a.build_room(), 6 << 10);
        assert_eq!(a.capacity(&cond).build_mb, 6 << 10);
        let got = a.helper_grants(&store::nasfs::NasRoot::local_for_test(&root), &cond, &mut w);
        assert_eq!(got.iter().map(|(k, _, _)| *k).collect::<Vec<_>>(), [0, 1, 2], "{w:?}");
        assert_eq!(a.slots.len(), 3);
        // Each started in its slot (a job that sleeps, its scratch argument kept).
        for (k, mut spec, _) in got {
            let scratch = scratch_of(&spec).unwrap();
            spec.cmd = vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into(), "x".into(), "--scratch".into()];
            spec.cmd.push(scratch.to_string_lossy().into_owned());
            assert!(
                a.try_start_said(k, spec, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut w),
                "{w:?}"
            );
        }
        let scratches: Vec<PathBuf> =
            a.slots.iter().map(|s| scratch_of(&s.running.as_ref().unwrap().spec).unwrap()).collect();
        assert_eq!(scratches[0], home.join("scratch/slope-hi"));
        assert_eq!(
            (scratches[1].clone(), scratches[2].clone()),
            (home.join("scratch-2/slope-hi"), home.join("scratch-3/slope-hi"))
        );
        assert_eq!(a.control_path(2), home.join("control-3"));
        assert!(matches!(a.slots[2].lease, Some(Held::Leased { .. })));
        assert_eq!(a.capacity(&cond).leases.len(), 3);
        let leases = a.capacity(&cond).leases;
        // (Task #665: a job under a core-minute for IDLE_CPU_S is named idle, its lease alone; one
        // busy within it, or not sampled yet, isn't.)
        assert!(a.capacity(&cond).idle.is_empty());
        // (A Mac up under IDLE_CPU_S + 1 min skips it: no Instant that far back.)
        let ago_i = |s: u64| Instant::now().checked_sub(Duration::from_secs(s));
        if let Some(old) = ago_i(IDLE_CPU_S + 60) {
            a.sampler.set_test_busy(Some(vec![Some(old), ago_i(IDLE_CPU_S - 60), None]));
            assert_eq!(a.capacity(&cond).idle, [leases[0]]);
            a.sampler.set_test_busy(None);
        }
        // (Its slots full by what the lead grants: asked again, nothing more fits.)
        assert!(a.helper_grants(&store::nasfs::NasRoot::local_for_test(&root), &cond, &mut w).is_empty());
        assert_eq!(a.slots.len(), 3, "no slot made for nothing");
        let tend_all = |a: &mut Agent| {
            for k in 0..a.slots.len() {
                a.tend(k, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
            }
        };
        // An update: every slot drains for it, and the loop restarts only once each has its safe
        // point or has had its time.
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v2", apps.join("current")).unwrap();
        tend_all(&mut a);
        assert!((0..3).all(|k| std::fs::read_to_string(a.control_path(k)).unwrap() == "drain"));
        assert!(!a.update_due() && !a.takes_more());
        let long_ago = Instant::now().checked_sub(DRAIN_GRACE + Duration::from_secs(60)).unwrap();
        a.slots[0].drain_since = Some(long_ago);
        a.slots[1].drain_since = Some(long_ago);
        assert!(!a.update_due(), "the third slot's job still has time");
        a.slots[2].drain_since = Some(long_ago);
        assert!(a.update_due());
        // (The update gone again: every job goes on.)
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        tend_all(&mut a);
        assert!((0..3).all(|k| std::fs::read_to_string(a.control_path(k)).unwrap() == "run"));
        assert!(a.slots.iter().all(|s| s.drain_since.is_none()));
        // The memory guard on a Mac of 16 GB (its limit 12 GB): slot 1's job the newest, slot 2's
        // next, slot 0's the oldest.
        a.mem_set = Some((16 << 10, 12 << 10));
        // (Each asked for sparing 6 GB: forgotten till the end, so the limit's guard is seen first;
        // what it spares is checked last.)
        assert!(a.slots.iter().all(|s| s.spare_mb == Some(6 << 10)));
        for s in a.slots.iter_mut() {
            s.spare_mb = None;
        }
        let now = now_s();
        for (k, ago) in [(0, 300), (1, 100), (2, 200)] {
            a.slots[k].running.as_mut().unwrap().started = now - ago;
        }
        let tick = |a: &mut Agent, held: Vec<Option<u64>>, trouble: bool| {
            a.sampler.set_test(Some(held), Some(trouble));
            a.sampler.tick();
            tend_all(a);
        };
        // Within the limit together: nothing.
        tick(&mut a, vec![Some(5 << 10), Some(3 << 10), Some(3 << 10)], false);
        assert!(!a.guard() && a.slots.iter().all(|s| s.mem_drain.is_none()));
        // Past it (13 GB), the largest fitting alone: the newest drains, not the largest, and the
        // member asks for nothing more meanwhile.
        tick(&mut a, vec![Some(5 << 10), Some(4 << 10), Some(4 << 10)], false);
        assert!(!a.guard());
        assert!(
            a.slots[1].mem_drain.is_some() && a.slots[0].mem_drain.is_none() && a.slots[2].mem_drain.is_none(),
            "{:?}",
            a.slots.iter().map(|s| s.mem_drain.clone()).collect::<Vec<_>>()
        );
        assert!(!a.takes_more());
        // (What stays once it stops, 9 GB, fits: none more drains.)
        tick(&mut a, vec![Some(5 << 10), Some(4 << 10), Some(4 << 10)], false);
        assert!(!a.guard() && a.slots[2].mem_drain.is_none());
        assert_eq!(std::fs::read_to_string(a.control_path(1)).unwrap(), "drain");
        // (The largest grown, what stays past the limit: the next newest drains too.)
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10), Some(4 << 10)], false);
        assert!(!a.guard() && a.slots[2].mem_drain.is_some() && a.slots[0].mem_drain.is_none());
        // The Mac short of memory, each job under the limit but together past it (17 GB): no safe
        // point waited for. The newest frozen at once; what stays (13 GB) still past it, the next
        // too; the largest, fitting alone, goes on. The Mac no longer short: both go on.
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10), Some(4 << 10)], true);
        assert!(!a.guard() && a.sampler.frozen(1) && !a.sampler.frozen(2) && !a.sampler.frozen(0));
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10), Some(4 << 10)], true);
        assert!(!a.guard() && a.sampler.frozen(1) && a.sampler.frozen(2));
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10), Some(4 << 10)], false);
        a.guard();
        assert!((0..3).all(|k| !a.sampler.frozen(k) && a.slots[k].running.is_some()));
        // A drain reaching no safe point in the time a pause gives (three slots): the first such
        // stopped at once, the others untouched.
        a.grace = Duration::ZERO;
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10), Some(4 << 10)], false);
        assert!(
            a.guard() && a.slots[1].running.is_none() && a.slots[0].running.is_some() && a.slots[2].running.is_some()
        );
        a.grace = DRAIN_GRACE;
        // The largest past the limit by itself, the Mac short of memory: stopped (the sampler froze
        // it), the job beside it untouched.
        tick(&mut a, vec![Some(13 << 10), None, Some(1 << 10)], true);
        assert!(a.sampler.frozen(0) && !a.sampler.frozen(2));
        assert!(a.guard() && a.slots[0].running.is_none() && a.slots[2].running.is_some());
        // The last far past the 6 GB this member spared it (10 GB and more), within the Mac's limit
        // and the Mac not short of memory: given back at once, not failed (interrupted, its lease's
        // result says, with its floors for the lead).
        a.slots[2].spare_mb = Some(6 << 10);
        let dir = match &a.slots[2].lease {
            Some(Held::Leased { dir, .. }) => dir.clone(),
            l => panic!("{l:?}"),
        };
        tick(&mut a, vec![None, None, Some(9 << 10)], false);
        assert!(!a.guard() && a.slots[2].running.is_some(), "9 GB: over, not far over");
        let target = a.slots[2].running.as_ref().unwrap().spec.record.as_ref().unwrap().targets[0].0.clone();
        tick(&mut a, vec![None, None, Some(11 << 10)], false);
        assert!(a.guard() && a.slots[2].running.is_none());
        let r: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("result.json")).unwrap()).unwrap();
        assert_eq!((r["interrupted"].as_bool(), r["failed"].as_bool()), (Some(true), Some(false)), "{r}");
        assert!(r["floors"].is_array() && r["error"].as_str().unwrap().contains("far past the 6.0 GB"), "{r}");
        assert!(!a.guard_backoff.keys().any(|id| id.ends_with(&target)) && a.slots[2].spare_mb.is_none());
        // What it held is its target's floor, learned alone (a pooled job's go with its give-back,
        // `Tell::GiveBack`; here, the pool off, as its result says); at the lead, this member
        // (sparing 6 GB) asking again isn't granted it.
        let floors: Vec<(String, memguard::Floor)> = serde_json::from_value(r["floors"].clone()).unwrap();
        c.add_floors(&floors);
        let f = c.floor("slope-hi", &target).unwrap();
        assert!(f.alone && f.mb >= 11 << 10, "{f:?}");
        // (Its three leases ended at the lead, as handing them back would: room for jobs again.)
        for l in &leases {
            a.client.as_ref().unwrap().give_back(*l, "the test").unwrap();
        }
        let again = a.helper_grants(&store::nasfs::NasRoot::local_for_test(&root), &cond, &mut w);
        assert!(!again.is_empty(), "{w:?}");
        for (k, spec, _) in &again {
            assert!(spec.record.as_ref().unwrap().targets.iter().all(|t| t.0 != target), "{target} granted again");
            a.slots[*k].lease = None;
            a.slots[*k].spare_mb = None;
        }
        // The last ends: the slots past the two kept dropped.
        a.trim_slots();
        assert_eq!(a.slots.len(), SLOTS);
        assert!(a.slots.iter().all(|s| s.running.is_none() && s.lease.is_none()));
    }

    #[test]
    fn a_3d_buildings_job_is_offered_by_the_rows_it_reads() {
        // Task #579's reads on the Mac mini: Paris's 6/32/22 (15.3 M rows kept of its row groups'
        // 16.5 M) peaked at 3.1 GB, Kantō's 6/56/25 (30.3 M) at 5.6 GB. B5's pilot raised New
        // York's 14.6 M records with the field in 7.9 GB, Toronto's 9.5 M in 3.8 GB, Kantō's 30.3 M
        // in 10.2 GB. Offered above each.
        assert_eq!((bld_peak("bldprep", 16_505_246), bld_peak("bldtiles", 16_505_246)), (4290, 8725));
        assert!(
            bld_peak("bldprep", 15_288_533) > 2965
                && bld_peak("bldprep", 30_321_333) > 5308
                && bld_peak("bldtiles", 14_611_533) > 7889
                && bld_peak("bldtiles", 9_496_532) > 3843
                && bld_peak("bldtiles", 30_320_309) > 10200
        );
        // A tile reading nothing (a GHSL tile alone): its fixed part.
        assert_eq!((bld_peak("bldprep", 0), bld_peak("bldtiles", 0)), (1300, 1800));
    }

    /// A job of the lead's own that fails ends its lease as failed (`finish_failed`): its target
    /// kept from the lead by its coordinator as a member's failed work is, so the next loop's ask
    /// isn't granted it again, the agent's own wait aside.
    #[test]
    fn a_failing_own_job_is_not_granted_again_on_the_next_loop() {
        room::TEST_FREE.with(|c| c.set(Some(100 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.free_set = Some(100 << 30);
        a.mem_set = Some((16 << 10, 12 << 10));
        let job = JobSpec {
            id: "rail rail".into(),
            what: "rail".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "exit 1".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "rail".into(), targets: vec![("rail".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let mut w = Vec::new();
        assert_eq!(
            lead_start(
                &mut a,
                std::slice::from_ref(&job),
                &cond,
                &store::nasfs::NasRoot::local_for_test(&root),
                &mut w
            ),
            [0],
            "{w:?}"
        );
        let t = Instant::now();
        while a.slots[0].running.is_some() {
            assert!(t.elapsed() < store::testkit::WATCHDOG, "it ends");
            a.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
        // (The agent's own wait set aside: the coordinator's backoff alone holds it.)
        a.mem.retry.clear();
        assert!(lead_start(
            &mut a,
            std::slice::from_ref(&job),
            &cond,
            &store::nasfs::NasRoot::local_for_test(&root),
            &mut w
        )
        .is_empty());
    }

    /// The lead's slots' ramp (`tend_ramp`): held at two while the NAS's limits aren't known, the
    /// stretch going on meanwhile (D1); one more after a clean stretch of `RAMP_CLEAN_S` in which
    /// it ran its slots at once (D2), kept in its state; a stall or a memory guard stop starts the
    /// stretch again, kept too (D4).
    #[test]
    fn the_leads_slots_ramp_up_after_a_clean_stretch() {
        room::TEST_FREE.with(|c| c.set(Some(100 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.free_set = Some(100 << 30);
        let most = job_slots(mac_cores() as usize);
        // (A time the agent took from the wall clock: within a second of now, a second boundary
        // between its reading and the test's.)
        let near_now = |t: u64| now_s().abs_diff(t) <= 1;
        let kept = |home: &Path| -> Ramp {
            serde_json::from_slice::<Memory>(&std::fs::read(home.join("state.json")).unwrap()).unwrap().ramp
        };
        // D1: no NAS limits yet: held at two whatever it reached, but the stretch goes on (a hold
        // isn't trouble: a restart's moment before the probe lands never starts it again), and it
        // doesn't grow while held (its slots can't be filled). Nothing rewritten meanwhile.
        let since = now_s() - RAMP_CLEAN_S - 100;
        a.mem.ramp = Ramp { slots: RAMP_START + 1, clean_since: since, peak: RAMP_START + 1 };
        for _ in 0..3 {
            a.tend_ramp();
        }
        assert_eq!(a.job_slots(), RAMP_START.min(most));
        assert_eq!(a.mem.ramp.slots, RAMP_START + 1, "held: no growth");
        assert_eq!(a.mem.ramp.clean_since, since, "the stretch goes on through the hold");
        assert!(!home.join("state.json").exists(), "a hold doesn't rewrite the state");
        assert!(a.ramp_trouble.is_none() && a.ramp_held);
        // The NAS probed: its slots as reached; the stretch, old enough and run at its slots
        // before the hold, grows them at once.
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        a.coord.as_ref().unwrap().set_nas(Some(limits));
        a.tend_ramp();
        assert!(!a.ramp_held);
        assert_eq!(a.mem.ramp.slots, RAMP_START + 2, "grown after the hold: the stretch was clean through it");
        assert_eq!(a.job_slots(), (RAMP_START + 2).min(most));
        // One probe failing (the limits known from before): neither a hold nor trouble, nothing
        // rewritten (a hiccup every few hours would keep the lead at two for good).
        let since = a.mem.ramp.clean_since;
        std::fs::remove_file(home.join("state.json")).unwrap();
        a.coord.as_ref().unwrap().set_nas_failures(1);
        a.tend_ramp();
        assert!(!a.ramp_held && a.ramp_trouble.is_none());
        assert_eq!(a.job_slots(), (RAMP_START + 2).min(most));
        assert_eq!(a.mem.ramp.clean_since, since);
        assert!(!home.join("state.json").exists());
        a.mem.ramp.clean_since = since - 100;
        // Two in a row: held, and trouble: the stretch starts again, kept; as a probe lands again,
        // the stretch starts then.
        a.coord.as_ref().unwrap().set_nas_failures(2);
        a.tend_ramp();
        assert!(a.ramp_held && a.ramp_trouble.as_deref().is_some_and(|w| w.contains("failing")));
        assert_eq!(a.job_slots(), RAMP_START.min(most));
        assert!(near_now(a.mem.ramp.clean_since) && near_now(kept(&home).clean_since));
        a.coord.as_ref().unwrap().set_nas_failures(0);
        a.tend_ramp();
        assert!(!a.ramp_held && a.ramp_trouble.is_none());
        assert_eq!(a.job_slots(), (RAMP_START + 2).min(most));
        a.mem.ramp = Ramp::default();
        a.tend_ramp();
        assert_eq!(a.job_slots(), RAMP_START.min(most));
        // D2: clean long enough but idle (never two jobs at once): no more.
        a.mem.ramp.clean_since = now_s() - RAMP_CLEAN_S;
        a.tend_ramp();
        assert_eq!(a.mem.ramp.slots, RAMP_START, "idle time doesn't count");
        // Having run two at once: one more, kept.
        a.mem.ramp.peak = RAMP_START;
        a.tend_ramp();
        assert_eq!(a.mem.ramp.slots, RAMP_START + 1);
        assert_eq!(a.job_slots(), (RAMP_START + 1).min(most));
        assert_eq!(kept(&home).slots, RAMP_START + 1);
        // A stall: the stretch starts again, the slots kept, saved.
        a.mem.ramp.clean_since = now_s() - RAMP_CLEAN_S;
        a.mem.ramp.peak = RAMP_START + 1;
        a.pool_hung = Some(now_s());
        a.tend_ramp();
        assert_eq!(a.mem.ramp.slots, RAMP_START + 1);
        assert!(near_now(a.mem.ramp.clean_since));
        assert!(near_now(kept(&home).clean_since));
        a.pool_hung = None;
        // D4: a memory guard stop starts it again, saved at once.
        a.mem.ramp.clean_since = 5;
        a.save();
        let job = JobSpec {
            id: "rail rail".into(),
            what: "rail".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "rail".into(), targets: vec![("rail".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(0, job, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        assert!(a.guard_stop_as(0, "memory guard", "past the limit (a test)", false));
        assert!(near_now(kept(&home).clean_since) && kept(&home).peak == 0, "{:?}", kept(&home));
        // A member's slots: by its cores alone.
        a.role = Role::Member;
        assert_eq!(a.job_slots(), most);
    }

    /// Every job of the lead's by grant (task #244, docs/pool.md §7.2): its plan offered through its
    /// own coordinator, its own steps (not offered to every member) granted to it alone, whole, and
    /// packed by their measures as a member's are: a step never measured alone, nothing beside it; a measured
    /// job beside another only when both fit this Mac's limit and its disk room; each started in a
    /// slot of its own (its scratch, and its worker in the history, its slot's). A member asking
    /// for the lead's steps gets none; nothing starts here without a grant.
    #[test]
    fn the_leads_jobs_are_granted_by_its_coordinator_and_packed_by_their_measures() {
        room::TEST_FREE.with(|c| c.set(Some(100 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let (co, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        a.coord = Some(std::sync::Arc::new(co));
        a.free_set = Some(100 << 30);
        // (A Mac of 16 GB: its limit 12 GB.)
        a.mem_set = Some((16 << 10, 12 << 10));
        // (Each a shell waiting, given its scratch folder as the plan's jobs are: the lead's own
        // steps, worldwide.)
        let job = |step: &str| JobSpec {
            id: format!("{step} {step}"),
            what: step.into(),
            cmd: vec![
                "/bin/sh".into(),
                "-c".into(),
                "sleep 3600".into(),
                "--scratch".into(),
                home.join("scratch").join(step).to_string_lossy().into_owned(),
            ],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: step.into(), targets: vec![(step.into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let stop = |a: &mut Agent| {
            for k in 0..a.slots.len() {
                if let Some(r) = a.slots[k].running.as_mut() {
                    r.stop(Duration::from_secs(5));
                    a.end_lease(k, Outcome::Interrupted, &[], "stopped");
                }
                a.slots[k].running = None;
            }
        };
        let running = |a: &Agent| {
            a.slots.iter().filter_map(|s| s.running.as_ref().map(|r| r.spec.id.clone())).collect::<Vec<_>>()
        };
        let mut w = Vec::new();
        // Nothing starts without a grant.
        assert!(!a.try_start_said(
            0,
            job("labels"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut w
        ));
        assert!(w.iter().any(|w| w.why.contains("not granted")), "{w:?}");
        // A cold start of a step never measured (labels): alone, nothing beside it.
        let plan = [job("labels"), job("items")];
        assert_eq!(
            lead_start(&mut a, &plan, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w),
            [0],
            "{w:?}"
        );
        assert!(lead_start(&mut a, &plan, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w).is_empty());
        assert_eq!(running(&a), ["labels labels"]);
        // A member asking for the lead's steps: none of them.
        let m1 = crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{port}")],
            a.coord.as_ref().unwrap().contact.token.clone(),
            "m1",
        );
        let ask = crate::coord::Ask {
            kind: "native".into(),
            can: vec!["labels".into(), "items".into()],
            mem_mb: 64_000,
            ..Default::default()
        };
        assert!(m1.ask(&ask).unwrap().is_none());
        stop(&mut a);
        // Measured: labels 6 GB, heritage 9 GB, items 3 GB and 25 GB of disk, two threads each.
        let c = a.coord.clone().unwrap();
        let cost = |step: &str, mb: u64| crate::coord::Cost {
            peak_mb: mb,
            secs: 60,
            worker: None,
            v: crate::coord::cost_version(step),
            sizes: [("threads".to_string(), 2)].into(),
            ..Default::default()
        };
        c.add_costs(&[
            (crate::coord::cost_key("labels", "labels"), cost("labels", 6144)),
            (crate::coord::cost_key("heritage", "heritage"), cost("heritage", 9216)),
            (crate::coord::cost_key("items", "items"), cost("items", 3072)),
        ]);
        c.add_disks(&[(crate::coord::cost_key("items", "items"), 25 << 10)]);
        // Heritage first (the plan's order); labels doesn't fit beside it, past this Mac's limit.
        let plan = [job("heritage"), job("labels")];
        assert_eq!(
            lead_start(&mut a, &plan, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w),
            [0],
            "{w:?}"
        );
        assert!(lead_start(&mut a, &plan, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w).is_empty());
        // Items fits its memory beside it, but not its disk (25 GB) while the disk has 20 GB.
        let plan = [job("heritage"), job("labels"), job("items")];
        a.free_set = Some(20 << 30);
        assert!(lead_start(&mut a, &plan, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w).is_empty());
        a.free_set = Some(100 << 30);
        assert_eq!(
            lead_start(&mut a, &plan, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w),
            [1],
            "{w:?}"
        );
        assert_eq!(running(&a), ["heritage heritage", "items items"]);
        // In a slot of its own: its scratch, and its worker in the history.
        let r = a.slots[1].running.as_ref().unwrap();
        assert_eq!(scratch_of(&r.spec), Some(home.join("scratch-2/items")));
        assert!(r.threads >= 1 && r.threads <= 2, "its measured threads: {}", r.threads);
        let (_, _, events, _) = c.for_forecast();
        assert!(events.iter().any(|e| e.kind == "start" && e.worker == Some(slot_worker(&a.host, 1))), "{events:?}");
        // Its leases are this Mac's own, in its coordinator.
        assert!(a.slots.iter().take(2).all(|s| matches!(s.lease, Some(Held::Own(_)))));
        stop(&mut a);
    }

    /// A `unit` job waiting on a worker holding its z6 tile's tail (its `WAITING_ENV` file) counts, beside
    /// it, as the memory it holds now, its cores idle: a second job starts in the room it frees; a
    /// stale file (its process not the job's) counts for nothing; when the first goes on and the two
    /// hold too much, the second yields to the memory guard, the larger or not.
    #[test]
    fn a_job_waiting_on_a_worker_frees_its_room_and_the_job_beside_it_yields() {
        room::TEST_FREE.with(|c| c.set(Some(100 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.free_set = Some(100 << 30);
        // (A Mac of 16 GB: its limit 12 GB.)
        a.mem_set = Some((16 << 10, 12 << 10));
        let job = |step: &str, t: &str| JobSpec {
            id: format!("{step} {t}"),
            what: step.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: step.into(), targets: vec![(t.into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let c = a.coord.as_ref().unwrap();
        let cost = |step: &str, mb: u64| crate::coord::Cost {
            peak_mb: mb,
            secs: 60,
            worker: None,
            v: crate::coord::cost_version(step),
            sizes: [("threads".to_string(), 2)].into(),
            ..Default::default()
        };
        c.add_costs(&[
            (crate::coord::cost_key("unit", "6/3/3"), cost("unit", 9216)),
            (crate::coord::cost_key("labels", "labels"), cost("labels", 6144)),
        ]);
        assert!(a.try_start(
            0,
            job("unit", "6/3/3"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let mut w = Vec::new();
        let tick = |a: &mut Agent, held: Vec<Option<u64>>| {
            a.sampler.set_test(Some(held), Some(false));
            a.sampler.tick();
            for k in 0..SLOTS {
                a.tend(k, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
            }
        };
        // Its tail out, holding 1 GB now, but not saying it waits: by its measure (9 GB), labels
        // (6 GB) isn't granted beside it.
        tick(&mut a, vec![Some(1 << 10), None]);
        let labels = [job("labels", "labels")];
        assert!(lead_start(&mut a, &labels, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w).is_empty());
        assert!(a.slots[1].running.is_none());
        // A stale file (a process not of the job's): the same.
        let note = |pid: u32| crate::offload::WaitNote { pid, worker: "ipad".into(), task: 7, since: now_s() };
        assert!(note(1).keep(&a.waiting_path(0)).map(std::mem::forget).is_some());
        assert!(a.waiting_on(0).is_none());
        assert!(lead_start(&mut a, &labels, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w).is_empty());
        // Its own (the job's process): labels is granted beside it (the z6 tile counted as it holds
        // now), with its measured threads, yielding.
        let pgid = a.slots[0].running.as_ref().unwrap().pgid as u32;
        assert!(note(pgid).keep(&a.waiting_path(0)).map(std::mem::forget).is_some());
        assert_eq!(a.waiting_on(0).map(|n| n.worker), Some("ipad".to_string()));
        assert!(a.job_view(0).and_then(|j| j.waiting).is_some_and(|w| w.contains("waiting on ipad")));
        assert_eq!(
            lead_start(&mut a, &labels, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w),
            [1],
            "{w:?}"
        );
        assert_eq!(a.slots[1].running.as_ref().map(|r| r.spec.id.clone()).as_deref(), Some("labels labels"));
        assert_eq!(a.slots[1].yields.as_deref(), Some("unit 6/3/3"));
        assert!(a.slots[1].running.as_ref().is_some_and(|r| r.threads >= 1));
        // The tail back, the z6 tile going on (its file gone): within the limit, the Mac not short of
        // memory, nothing.
        std::fs::remove_file(a.waiting_path(0)).unwrap();
        tick(&mut a, vec![Some(5 << 10), Some(6 << 10)]);
        assert!(!a.guard() && a.slots[1].running.is_some());
        // Past the limit together, the second the larger: it's stopped at once (no safe point
        // waited for), given back, not kept from this Mac, no floor learned of it; the z6 tile goes on.
        tick(&mut a, vec![Some(5 << 10), Some(8 << 10)]);
        assert!(a.guard());
        assert!(
            a.slots[1].running.is_none()
                && a.slots[1].yields.is_none()
                && a.slots[0].running.is_some()
                && a.slots[0].mem_drain.is_none()
        );
        assert!(a.guard_backoff.is_empty(), "{:?}", a.guard_backoff);
        assert!(a.coord.as_ref().unwrap().floor("labels", "labels").is_none());
        // Started beside it again, the Mac short of memory though within the limit: stopped at once.
        assert!(note(pgid).keep(&a.waiting_path(0)).map(std::mem::forget).is_some());
        tick(&mut a, vec![Some(1 << 10), None]);
        assert_eq!(
            lead_start(&mut a, &labels, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w),
            [1],
            "{w:?}"
        );
        assert!(a.slots[1].running.is_some());
        std::fs::remove_file(a.waiting_path(0)).unwrap();
        a.sampler.set_test(Some(vec![Some(3 << 10), Some(3 << 10)]), Some(true));
        a.sampler.tick();
        assert!(a.guard() && a.slots[1].running.is_none());
        // Its mark tied to the job it yields to: a job in the second slot marked for a job no
        // longer running (the first slot's ended, another started) doesn't yield.
        a.sampler.set_test(Some(vec![Some(1 << 10), None]), Some(false));
        a.sampler.tick();
        assert!(a.try_start(
            1,
            job("labels", "labels"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
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
    /// sandboxed (a connection around it denied); the Wikidata steps share the Mac; no job starts
    /// without the proxy.
    #[test]
    fn jobs_go_out_through_the_proxy_and_no_group_keeps_steps_apart() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/pool")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(100 << 30);
        a.mem_set = Some((48 << 10, 42 << 10));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let seen = d.path().join("seen");
        let job = |step: &str, t: &str| {
            let script = format!("env > {0}/{step}.env; /usr/bin/nc -v -z -G 5 192.0.2.1 9 2> {0}/{step}.out; echo done >> {0}/{step}.out", seen.display());
            JobSpec {
                id: format!("{step} {t}"),
                what: step.into(),
                cmd: vec!["/bin/sh".into(), "-c".into(), script],
                needs: Needs { nas: false },
                restart_after_sleep: false,
                record: Some(build::Work { step: step.into(), targets: vec![(t.into(), "k".into())] }),
            }
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
        a.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        let url = a.proxy.as_ref().map(|p| p.url()).expect("the proxy started");
        assert!(a.try_start(
            0,
            job("items", "items"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        assert!(a.try_start(
            1,
            job("heritage", "heritage"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        // (Each job ends by itself once it has looked: its exit waited for, bounded by nc's own
        // five seconds at most; no clock in the test.)
        for k in 0..SLOTS {
            assert!(a.slots[k].running.as_mut().unwrap().wait().unwrap().success());
        }
        for (k, step) in ["items", "heritage"].into_iter().enumerate() {
            let out = seen.join(format!("{step}.out"));
            assert!(std::fs::read_to_string(&out).unwrap().contains("done"));
            let env = std::fs::read_to_string(seen.join(format!("{step}.env"))).unwrap();
            assert!(
                env.contains(&format!("HTTPS_PROXY={}\n", url.replace("http://", &format!("http://slot{k}:x@"))))
                    && env.contains(&format!("{}={url}\n", crate::budget::PROXY_ENV))
                    && env.contains("NO_PROXY=localhost,127.0.0.1,::1\n"),
                "{env}"
            );
            assert!(std::fs::read_to_string(&out).unwrap().contains("Operation not permitted"), "{step}: sandboxed");
        }
        stop(&mut a);
        // No proxy: no job starts.
        let p = a.proxy.take();
        assert!(a.wait_reason(&job("items", "items"), &cond).is_some_and(|w| w.contains("proxy")));
        a.proxy = p;
        assert!(a.wait_reason(&job("items", "items"), &cond).is_none());
    }

    /// The disk guard (by measure): short of the owner's room target (here 30 GB), the job growing
    /// fastest frozen, room made
    /// from what no job holds, and it goes on once the disk has room; given back when none can be
    /// made, what it took on the disk learned as its target's disk floor.
    #[test]
    fn the_disk_guard_freezes_makes_room_and_lets_go_on_or_gives_back() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.mem_set = Some((16 << 10, 12 << 10));
        let g: u64 = 1 << 30;
        let job = |t: &str| JobSpec {
            id: format!("terrain {t}"),
            what: "terrain-hi".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "terrain-hi".into(), targets: vec![(t.into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        a.free_set = Some(100 * g);
        a.room_target = Some(room::Target { bytes: 30 * g, by: "a test".into(), at: now_s() });
        assert!(a.try_start(
            0,
            job("6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        a.sampler.set_test(Some(vec![None, None]), Some(false));
        a.sampler.set_disk(true, 30 * g, &home);
        // The disk falls short: frozen; room made (here none to make), then the disk has room again
        // (another program let go of its files): it goes on.
        a.free_set = Some(20 * g);
        a.sampler.set_test_disk(Some(20 * g), Some(vec![Some(5 * g), None]));
        a.sampler.tick();
        assert!(a.sampler.disk_frozen(0));
        assert!(
            !a.disk_guard(Some(&store::nasfs::NasRoot::local_for_test(&root))) && a.disk_room.is_some(),
            "room being made"
        );
        a.free_set = Some(40 * g);
        assert!(!a.disk_guard(Some(&store::nasfs::NasRoot::local_for_test(&root))));
        assert!(!a.sampler.disk_frozen(0) && a.slots[0].running.is_some(), "it goes on");
        // Short again, and no room to be made: given back, not failed, its disk floor learned.
        a.free_set = Some(20 * g);
        a.sampler.set_test_disk(Some(20 * g), Some(vec![Some(7 * g), None]));
        a.sampler.tick();
        assert!(a.sampler.disk_frozen(0));
        assert!(!a.disk_guard(Some(&store::nasfs::NasRoot::local_for_test(&root))));
        assert!(
            a.disk_guard(Some(&store::nasfs::NasRoot::local_for_test(&root))) && a.slots[0].running.is_none(),
            "given back"
        );
        assert!(!a.mem.retry.contains_key("terrain-hi 6/1/1"), "not held against it as a failure");
        assert!(
            a.coord
                .as_ref()
                .unwrap()
                .disks()
                .get(&crate::coord::cost_key("terrain-hi", "6/1/1"))
                .is_some_and(|mb| *mb >= 7 << 10),
            "{:?}",
            a.coord.as_ref().unwrap().disks()
        );
        assert!(
            a.guard_last.as_ref().is_some_and(|(_, w)| w.contains("given back by the disk guard")),
            "{:?}",
            a.guard_last
        );
        // Two frozen (the second filling the disk past half the target meanwhile), no room to make:
        // the one taking the most given back first, the other kept frozen until the next decision.
        a.free_set = Some(200 * g);
        assert!(a.try_start(
            0,
            job("6/2/2"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        assert!(a.try_start(
            1,
            job("6/3/3"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        a.sampler.set_test_disk(Some(20 * g), Some(vec![Some(2 * g), None]));
        a.sampler.tick();
        a.sampler.set_test_disk(Some(10 * g), Some(vec![Some(2 * g), Some(9 * g)]));
        a.sampler.tick();
        assert!(a.sampler.disk_frozen(0) && a.sampler.disk_frozen(1));
        a.free_set = Some(10 * g);
        assert!(!a.disk_guard(Some(&store::nasfs::NasRoot::local_for_test(&root))));
        assert!(
            a.disk_guard(Some(&store::nasfs::NasRoot::local_for_test(&root)))
                && a.slots[1].running.is_none()
                && a.slots[0].running.is_some(),
            "the larger given back"
        );
        assert!(a.sampler.disk_frozen(0));
        a.free_set = Some(40 * g);
        assert!(
            !a.disk_guard(Some(&store::nasfs::NasRoot::local_for_test(&root))) && !a.sampler.disk_frozen(0),
            "the other goes on once there's room"
        );
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.mem_set = Some((64 << 10, 56 << 10));
        let g: u64 = 1 << 30;
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        // 40 slope z6 tiles of 1 GB each on the disk, one of 3 GB; a z6 tile of 10 GB.
        let tiles: Vec<String> = (0..40).map(|i| format!("6/{i}/0")).collect();
        let c = a.coord.as_ref().unwrap();
        let cost = |step: &str, mb: u64| crate::coord::Cost {
            peak_mb: mb,
            secs: 60,
            worker: None,
            v: crate::coord::cost_version(step),
            ..Default::default()
        };
        c.add_costs(&[
            (crate::coord::cost_key("slope-hi", "6/0/0"), cost("slope-hi", 1000)),
            (crate::coord::cost_key("unit", "6/9/9"), cost("unit", 4000)),
        ]);
        let mut disks: Vec<(String, u64)> =
            tiles.iter().map(|t| (crate::coord::cost_key("slope-hi", t), 1024)).collect();
        disks[39].1 = 3072;
        disks.push((crate::coord::cost_key("unit", "6/9/9"), 10 << 10));
        c.add_disks(&disks);
        let batch = JobSpec {
            id: "slope-hi 6/0/0".into(),
            what: "slope pieces".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work {
                step: "slope-hi".into(),
                targets: tiles.iter().map(|t| (t.clone(), "k".into())).collect(),
            }),
        };
        assert_eq!(a.measured_job(&batch).map(|m| m.disk_mb), Some(39 * 1024 + 3072), "summed, not its largest");
        a.free_set = Some(100 * g);
        assert!(a.try_start(0, batch, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        // Five done, 5 GB taken so far: 34 of 1 GB and one of 3 GB still to come.
        std::fs::write(a.done_path(0), (0..5).map(|i| format!("slope-hi 6/{i}/0\n")).collect::<String>()).unwrap();
        a.sampler.set_test(Some(vec![None, None]), Some(false));
        a.sampler.set_test_disk(Some(60 * g), Some(vec![Some(5 * g), None]));
        a.sampler.tick();
        assert_eq!(a.disk_left(0), (34 * 1024 + 3072) << 20);
        // (Task #242: what it has still to take is its reservation, taken off the disk an ask and
        // the capacity say: 100 GB free, less the 37 reserved; no fixed reserve, task #421.)
        a.free_set = Some(100 * g);
        let cap = a.capacity(&cond);
        assert_eq!((cap.reserved_mb, cap.disk_mb), (34 * 1024 + 3072, 63 * 1024));
        let unit = JobSpec {
            id: "unit 6/9/9".into(),
            what: "a unit".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "unit".into(), targets: vec![("6/9/9".into(), "k".into())] }),
        };
        // 10 GB and 37 GB to come: past 45 GB free (its largest alone, 3 GB, would have let it be
        // granted beside, and the guard give it back an hour on, and again).
        a.free_set = Some(45 * g);
        assert!(a.disk_room() < 10 * g);
        a.free_set = Some(50 * g);
        assert!(a.disk_room() >= 10 * g);
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.slots[0].running = None;
        // A floor far past the disk (stale, or an outlier's): alone, the first slot starts it all the
        // same once room-making has done what it can.
        c_disks_huge(a.coord.as_ref().unwrap());
        a.free_set = Some(40 * g);
        assert!(
            a.try_start(0, unit.clone(), &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()),
            "started alone, the guard its backstop"
        );
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
        a.end_lease(0, Outcome::Interrupted, &[], "stopped");
        a.slots[0].running = None;
        // Short of the owner's room target itself: it waits, said why (started, the disk guard would
        // freeze it at once and give it back, and the next job likewise: the two-Mac run's loop).
        a.room_target = Some(room::Target { bytes: 20 * g, by: "a test".into(), at: now_s() });
        a.free_set = Some(19 * g);
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
        assert!(!a.try_start(0, scratched, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut w));
        assert!(
            a.slots[0].running.is_none() && w.iter().any(|x| x.why.contains("waits for the disk room target")),
            "{w:?}"
        );
        assert!(mine.exists() && !other.exists(), "its own scratch kept, the other's freed for room");
        a.floor_short = None;
        // Past the target: alone, the target is all it starts with, not its floor past it.
        a.free_set = Some(21 * g);
        let mut w = Vec::new();
        assert!(
            a.try_start(0, unit.clone(), &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut w),
            "started alone past a high floor, the target kept: {w:?}"
        );
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(5));
        }
    }

    fn c_disks_huge(c: &crate::coord::Coordinator) {
        c.add_disks(&[(crate::coord::cost_key("unit", "6/9/9"), 500 << 10)]);
    }

    /// The Mac short of memory (task #240), the lead's two jobs together past its limit, each under
    /// it: the one beside the largest frozen at once, not drained; it goes on once the largest
    /// ended; frozen again, it's stopped (given back) once the Mac stays short for a minute.
    #[test]
    fn under_pressure_the_job_beside_is_frozen_then_thawed_or_stopped() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.free_set = Some(40 << 30);
        a.mem_set = Some((16 << 10, 12 << 10));
        let job = |id: &str| {
            let (step, t) = id.split_once(' ').unwrap();
            JobSpec {
                id: id.into(),
                what: id.into(),
                cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
                needs: Needs { nas: false },
                restart_after_sleep: false,
                record: Some(build::Work { step: step.into(), targets: vec![(t.into(), "k".into())] }),
            }
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        let tick = |a: &mut Agent, held: Vec<Option<u64>>, trouble: bool| {
            a.sampler.set_test(Some(held), Some(trouble));
            a.sampler.tick();
            for k in 0..SLOTS {
                a.tend(k, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
            }
            a.guard()
        };
        assert!(a.try_start(
            0,
            job("terrain-hi 6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        assert!(a.try_start(
            1,
            job("bldtiles 6/2/2"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        // Frozen at once, not drained; the largest ending, it goes on.
        assert!(!tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], true));
        assert!(a.sampler.frozen(1) && a.slots[1].mem_drain.is_none() && a.slots[1].running.is_some());
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.stopped(0, "the test");
        a.slots[0].running = None;
        assert!(!tick(&mut a, vec![None, Some(4 << 10)], true));
        assert!(!a.sampler.frozen(1) && a.slots[1].pressure_frozen.is_none() && a.slots[1].running.is_some());
        // Frozen again beside a new largest; within the minute it's held, then stopped.
        assert!(a.try_start(
            0,
            job("terrain-hi 6/1/2"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        assert!(!tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], true));
        assert!(a.sampler.frozen(1));
        assert!(!tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], true) && a.slots[1].running.is_some());
        let back = Instant::now().checked_sub(PRESSURE_HOLD).unwrap();
        a.slots[1].pressure_frozen.as_mut().unwrap().0 = back;
        assert!(tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], true));
        assert!(a.slots[1].running.is_none() && a.slots[0].running.is_some());
        assert!(a.guard_view().last.is_some_and(|(_, w)| w.contains("frozen") && w.contains("still short")));
        // A job paused (the build's pause, a drain past its grace) counts as frozen already: the
        // pressure coming and going neither freezes it nor lets it go on.
        // This job reports its own state: a heartbeat line every 50 ms into a file. Stopped means
        // the file stops growing, going means it grows; the OS's own word (`ps`) is checked too.
        let hb = d.path().join("heartbeat");
        let mut beating = job("bldtiles 6/2/3");
        beating.cmd =
            vec!["/bin/sh".into(), "-c".into(), format!("while :; do echo x >> '{}'; sleep 0.05; done", hb.display())];
        assert!(a.try_start(1, beating, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        let size = || std::fs::metadata(&hb).map_or(0, |m| m.len());
        let until = |what: &str, f: &dyn Fn() -> bool| {
            let t = Instant::now();
            while !f() {
                assert!(t.elapsed() < store::testkit::WATCHDOG, "{what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        until("running: the heartbeat advances", &|| size() >= 6);
        a.slots[1].running.as_mut().unwrap().pause("the test");
        // A stop reaches the process a moment after it's sent (longer under load): the process is
        // stopped (any `T` state) and the heartbeat has stopped advancing.
        let stopped = |a: &Agent| {
            let pgid = a.slots[1].running.as_ref().unwrap().pgid;
            until("still stopped (ps)", &|| {
                let o = store::spawn::output(store::spawn::command("/bin/ps").args([
                    "-o",
                    "stat=",
                    "-p",
                    &pgid.to_string(),
                ]))
                .unwrap();
                String::from_utf8_lossy(&o.stdout).contains('T')
            });
            let before = size();
            std::thread::sleep(Duration::from_millis(300)); // (absence)
            assert_eq!(size(), before, "still stopped (heartbeat)");
        };
        for trouble in [true, true, false] {
            a.sampler.set_test(Some(vec![Some(9 << 10), Some(4 << 10)]), Some(trouble));
            a.sampler.tick();
            assert!(!a.guard());
            // The agent's own records first.
            assert!(a.slots[1].pressure_frozen.is_none() && a.slots[1].running.as_ref().unwrap().paused.is_some());
            stopped(&a);
        }
        a.slots[1].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
    }

    #[test]
    fn the_memory_guard_drains_the_job_beside_then_stops_the_one_past_the_limit_and_learns() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        a.free_set = Some(40 << 30);
        // (A Mac of 16 GB: its limit 12 GB.)
        a.mem_set = Some((16 << 10, 12 << 10));
        let job = |id: &str, targets: &[&str]| {
            let step = id.split(' ').next().unwrap();
            let record = Some(build::Work {
                step: step.into(),
                targets: targets.iter().map(|t| (t.to_string(), "k".to_string())).collect(),
            });
            JobSpec {
                id: id.into(),
                what: id.into(),
                cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
                needs: Needs { nas: false },
                restart_after_sleep: false,
                record,
            }
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        // (A costs file an earlier job left, naming a target begun and never ended: gone as the next
        // starts, so it can't be blamed.)
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(a.costs_path(0), "{\"unit\":\"water water\",\"started\":1}\n").unwrap();
        assert!(a.try_start(
            0,
            job("terrain-hi 6/1/1", &["6/1/1", "6/1/2"]),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        assert!(!a.costs_path(0).exists());
        assert!(a.try_start(
            1,
            job("bldtiles 6/2/2", &["6/2/2"]),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let tick = |a: &mut Agent, held: Vec<Option<u64>>, trouble: bool| {
            a.sampler.set_test(Some(held), Some(trouble));
            a.sampler.tick();
            for k in 0..SLOTS {
                a.tend(k, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
            }
        };
        // Together within the limit: nothing.
        tick(&mut a, vec![Some(7 << 10), Some(4 << 10)], false);
        assert!(!a.guard() && a.slots[1].mem_drain.is_none());
        // Together past it, the larger fitting alone: the one beside it stops at its next safe point
        // (asked through its channel), and nothing starts beside the larger meanwhile.
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert!(!a.guard());
        assert!(
            a.slots[1].mem_drain.as_deref().is_some_and(|w| w.contains("past this Mac's limit of 12.0 GB")),
            "{:?}",
            a.slots[1].mem_drain
        );
        tick(&mut a, vec![Some(9 << 10), Some(4 << 10)], false);
        assert_eq!(std::fs::read_to_string(a.control_path(1)).unwrap(), "drain");
        // (Nothing asked for meanwhile.)
        assert!(a.guard_held(0).is_some_and(|id| id == "terrain-hi 6/1/1") && !a.takes_more());
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
        let c = a.coord.clone().unwrap();
        assert_eq!(
            (c.floor("terrain-hi", "6/1/1").map(|f| (f.mb, f.alone)), c.floor("terrain-hi", "6/1/2")),
            (Some((13 << 10, false)), None)
        );
        assert!(!a.mem.retry.contains_key("terrain-hi 6/1/1"), "not held against it as a failure");
        assert!(a
            .wait_reason(&job("terrain-hi 6/1/1", &["6/1/1"]), &cond)
            .is_some_and(|w| w.contains("tried here again in 60 min")));
        let v = a.guard_view();
        assert!(
            v.on && v.limit_mb == 12 << 10
                && v.last
                    .as_ref()
                    .is_some_and(|(_, w)| w.contains("terrain-hi 6/1/1 stopped by the memory guard: it held 13.0 GB")),
            "{v:?}"
        );
        // A batch's floor past the limit: the target tried again in a job of its own, not held.
        let works = vec![build::Work {
            step: "terrain-hi".into(),
            targets: vec![("6/1/1".into(), "k".into()), ("6/1/2".into(), "k".into())],
        }];
        let h = a.planner().guard_holds(&works);
        assert!(h.here.is_empty() && h.alone.contains(&("terrain-hi".to_string(), "6/1/1".to_string())));
        // Learned alone (a job of that one target): held here, and everywhere while no Mac has room,
        // the status saying why; with a Mac of more room in the pool, left to it.
        a.guard_backoff.clear();
        assert!(a.try_start(
            0,
            job("terrain-hi 6/1/1", &["6/1/1"]),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        tick(&mut a, vec![Some(14 << 10), None], true);
        assert!(a.guard());
        let c = a.coord.clone().unwrap();
        assert_eq!(c.floor("terrain-hi", "6/1/1").map(|f| (f.mb, f.alone)), Some((14 << 10, true)));
        let h = a.planner().guard_holds(&works);
        assert!(
            h.here.contains(&("terrain-hi".to_string(), "6/1/1".to_string()))
                && h.here.len() == 1
                && h.all.len() == 1
                && h.alone.is_empty()
        );
        assert!(h.why[0].why.contains("more than any Mac in the pool has"), "{:?}", h.why);
        let w = crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{}", c.contact.urls[0].rsplit(':').next().unwrap())],
            c.contact.token.clone(),
            "big",
        );
        let _ = w.ask(&crate::coord::Ask {
            kind: "native".into(),
            label: Some("big (helper)".into()),
            can: vec!["terrain-hi".into()],
            mem_mb: 30 << 10,
            limit_mb: Some(42 << 10),
            ..Default::default()
        });
        let h = a.planner().guard_holds(&works);
        assert!(
            h.here.len() == 1 && h.all.is_empty() && h.why[0].why.contains("left to a Mac with room"),
            "{:?}",
            h.why
        );
        // A step only the lead runs (heritage), its floor past the lead's limit: judged against the
        // lead's limit, held, whatever a member's limit (the lead is fixed).
        let c = a.coord.clone().unwrap();
        let lead_only = vec![build::Work { step: "heritage".into(), targets: vec![("heritage".into(), "k".into())] }];
        let floor = |mb| {
            vec![(
                crate::coord::cost_key("heritage", "heritage"),
                memguard::Floor { mb, alone: true, v: crate::coord::cost_version("heritage") },
            )]
        };
        c.add_floors(&floor(14 << 10));
        let h = a.planner().guard_holds(&lead_only);
        assert!(h.all.len() == 1, "{:?}", h.why);
        let why = &h.why[0].why;
        assert!(
            why.contains("past the lead's limit of 12.0 GB; this step runs only on the lead")
                && why.contains("the lead's build memory")
                && !why.contains("(helper)")
                && !why.contains("more than any Mac"),
            "{why}"
        );
        c.add_floors(&floor(50 << 10));
        let why = &a.planner().guard_holds(&lead_only).why[0].why;
        assert!(why.contains("past the lead's limit of 12.0 GB") && !why.contains("lead give"), "{why}");
        c.floors(true, Some("heritage"), None);
        // The job beside the larger drained when the larger is the second slot's: nothing is asked
        // for until the larger ends.
        assert!(a.try_start(
            1,
            job("bldtiles 6/5/5", &["6/5/5"]),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        assert!(a.try_start(
            0,
            job("pack 6/4/4", &["6/4/4"]),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        tick(&mut a, vec![Some(4 << 10), Some(9 << 10)], false);
        assert!(!a.guard() && a.slots[0].mem_drain.is_some());
        assert!(a.guard_held(1).is_some() && !a.takes_more());
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
        a.guard_on = memguard::on(&store::nasfs::NasRoot::local_for_test(&root));
        tick(&mut a, vec![None, Some(20 << 10)], true);
        assert!(!a.guard() && a.slots[1].running.is_some());
        assert!(a.planner().guard_holds(&works).here.is_empty());
        for k in 0..SLOTS {
            if let Some(r) = a.slots[k].running.as_mut() {
                r.stop(Duration::from_secs(5));
            }
        }
    }

    #[test]
    fn a_catalogue_held_back_is_noted_until_one_is_made_and_a_dry_run_keeps_its_own() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        assert_eq!(*a.planner().catalogues.borrow(), build::LastCatalogue::default());
        let file = || -> build::LastCatalogue {
            serde_json::from_slice(&std::fs::read(home.join(PUBLISHED_FILE)).unwrap()).unwrap()
        };
        // A catalogue held back: when, kept in the file (a restart reads it), until the next is made.
        a.planner().note_held(Some(250));
        assert_eq!(file().held_at, Some(250));
        assert_eq!(read_published(&home).held_at, Some(250));
        a.planner().catalogue_made();
        assert_eq!(file(), build::LastCatalogue::default());
        // A dry run beside it (a second agent on this Mac, planning only): its record its own,
        // never the file.
        let mut dry = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        assert_eq!(*dry.planner().catalogues.borrow(), file());
        dry.planner().note_held(Some(300));
        assert_eq!(file(), build::LastCatalogue::default());
        // (A record that doesn't read: none.)
        std::fs::write(home.join(PUBLISHED_FILE), "?").unwrap();
        assert_eq!(read_published(&home), build::LastCatalogue::default());
    }

    /// The lead's plan `jobs` offered through its coordinator and asked for as its loop asks
    /// (`offer_plan`, `lead_grants`), each job granted started in its slot (`start_granted`); the
    /// slots granted, and why not in `w`.
    fn lead_start(
        a: &mut Agent,
        jobs: &[JobSpec],
        cond: &Conditions,
        root: &store::nasfs::NasRoot,
        w: &mut Vec<Waiting>,
    ) -> Vec<usize> {
        a.planner().offer_plan(jobs);
        let granted = a.lead_grants(jobs, 0, cond, root, w);
        let slots: Vec<usize> = granted.iter().map(|g| g.0).collect();
        a.start_granted(granted, cond, Some(root), w);
        for k in 0..a.slots.len() {
            if a.slots[k].running.is_none() && a.slots[k].lease.is_some() {
                a.end_lease(k, Outcome::Interrupted, &[], "it couldn't start here");
            }
        }
        slots
    }

    /// An agent that runs jobs (no dry run), the NAS at `root`: the lead's, or a member's.
    fn running_agent(root: &store::nasfs::NasRoot, home: &Path, helper: bool) -> Agent {
        test_agent(Options {
            root: Some(root.clone()),
            home: home.to_path_buf(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: helper,
        })
        .unwrap()
    }

    /// Each Mac's adopted `seq`: its map server's cursor in the log, adopted when first heard there
    /// (docs/maplog.md §6.1); other clients' cursors aren't servers. Worker names folded to their Macs.
    #[test]
    fn each_macs_adopted_catalogue_is_its_servers_cursor() {
        let feed = store::map::feed::Feed::new();
        let m = store::map::Map::new(812, "t".into(), [("u/6/0/0".to_string(), serde_json::json!(true))].into());
        feed.start(std::sync::Arc::new(m), 812, &[], Default::default());
        feed.changes(811, Some("server/m4"), 100);
        feed.changes(812, Some("server/m4"), 110);
        feed.changes(812, Some("server/m4"), 140);
        feed.changes(811, Some("server/m1"), 105);
        feed.changes(812, Some("mirror/m1"), 120);
        let a = adopted(&feed.cursors());
        assert_eq!(a.len(), 2);
        assert_eq!(a["m4"], CatalogueSeen { n: 812, at: 110 });
        assert_eq!(a["m1"], CatalogueSeen { n: 811, at: 105 });
        assert_eq!((host_of("m4 #3"), host_of("m4"), host_of("page #1")), ("m4", "m4", "page #1"));
    }

    /// A forecast made now: the build done (nothing left to build), or not.
    fn forecast_now(done: bool) -> forecast::Forecast {
        forecast::Forecast {
            at: now_s(),
            done_at: (!done).then(|| now_s() + 3600),
            why: done.then(|| forecast::NOTHING_LEFT.to_string()),
            ..Default::default()
        }
    }

    /// A job that waits half a minute, needing nothing.
    fn waiting_job(id: &str) -> JobSpec {
        JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: None,
        }
    }

    fn put(p: &Path, b: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }

    #[test]
    fn a_mac_keeps_its_caches_after_the_build() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let c = home.join("cache");
        let copy = c.join("blobs/layers/terrain/hi/6-1-1.0000000000000001.pack");
        put(&c.join("chm10/a.tif"), &crate::whole::testfiles::tiff(false));
        put(&copy, &[1; 1000]);
        let mut a = running_agent(&store::nasfs::NasRoot::local_for_test(&root), &home, false);
        // Work left: nothing stops a clear (task #814), and nothing is freed unasked.
        *a.planner().forecast.borrow_mut() = Some(forecast_now(false));
        assert_eq!(a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))), None);
        assert!(a.caches_task.is_none() && copy.exists());
        // Nothing left, with or without a job having run here since: nothing is freed, none
        // started (no room target: nothing to free toward), whatever is free.
        *a.planner().forecast.borrow_mut() = Some(forecast_now(true));
        a.worked("unit 6/1/1");
        *a.planner().forecast.borrow_mut() = Some(forecast_now(true));
        assert_eq!(a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))), None);
        assert!(a.caches_task.is_none() && copy.exists() && c.join("chm10/a.tif").exists());
        // A target the disk is short of: freed toward it, and only then.
        a.room_target = Some(room::Target { bytes: 1 << 50, by: String::new(), at: 0 });
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        // (One loop at a time: waited for.)
        assert!(!copy.exists() && a.mem.toward.as_ref().is_some_and(|f| f.bytes() >= 1000));
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
        let mut helper = running_agent(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("m1"), true);
        helper.seed_answers(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(!archive.exists());
        // The lead's: not while a job runs here, nor without the NAS; then once.
        let mut a = running_agent(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("m4"), false);
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(
            0,
            waiting_job("pack 6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        a.seed_answers(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(!archive.exists());
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        a.seed_answers(None);
        assert!(!archive.exists());
        a.seed_answers(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(archive.is_file());
        std::fs::remove_file(&archive).unwrap();
        a.seed_answers(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(!archive.exists(), "once an agent");
        // (While it runs, the steps that keep the answers wait on its hold of them here, their
        // own: crate::answers::Local.)
    }

    #[test]
    fn a_freeing_runs_on_a_thread_of_its_own_and_jobs_start_meanwhile() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // (Not one loop at a time: the freeing isn't waited for.)
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: false,
            member: false,
        })
        .unwrap();
        *a.planner().forecast.borrow_mut() = Some(forecast_now(true));
        // A freeing under way (one that waits to be let go): the loop isn't held, and jobs start
        // meanwhile (what they use they hold: store::cachefile).
        let (go, wait) = std::sync::mpsc::channel::<()>();
        a.caches_task = Some(CachesTask {
            ask: None,
            toward: Some((1 << 40, 1 << 40)),
            disk: false,
            began: Instant::now(),
            thread: std::thread::spawn(move || {
                wait.recv()
                    .map(|()| room::Freed {
                        freed: BTreeMap::from([("blobs".to_string(), 1000)]),
                        ..Default::default()
                    })
                    .map_err(anyhow::Error::from)
            }),
        });
        assert!(a
            .tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)))
            .is_some_and(|w| w.starts_with("this Mac's caches are being freed toward the disk room target")));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(
            0,
            waiting_job("pack 6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        // Done: what it freed kept.
        go.send(()).unwrap();
        while a.caches_task.as_ref().is_some_and(|t| !t.thread.is_finished()) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))), None);
        assert_eq!(a.mem.toward.as_ref().map(|f| f.bytes()), Some(1000));
        assert!(a.caches_task.is_none());
        assert!(a.try_start(
            0,
            waiting_job("pack 6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
    }

    #[test]
    fn a_job_an_earlier_agent_left_holds_the_caches_until_its_gone() {
        use std::os::unix::process::CommandExt;
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut a = running_agent(&store::nasfs::NasRoot::local_for_test(&root), &home, false);
        *a.planner().forecast.borrow_mut() = Some(forecast_now(true));
        // (A process group of its own, as a job's.)
        let mut left =
            store::spawn::spawn(store::spawn::command("/bin/sh").args(["-c", "sleep 3600"]).process_group(0)).unwrap();
        let pgid = left.id() as i32;
        let group =
            |leader_start: u64| jobs::Group { pgid, leader_start, started: now_s() - 5, id: "unit 6/1/1".into() };
        // Its group's id another program's now (another leader's start time): not the job's.
        a.orphans.push(group(1));
        assert_eq!(a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))), None);
        assert!(a.orphans.is_empty());
        // The job's: held while it runs, not once it's gone.
        a.orphans.push(group(crate::sys::process_start(pgid).unwrap()));
        assert_eq!(
            a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))).as_deref(),
            Some("a job an earlier agent left still runs here (unit 6/1/1)")
        );
        left.kill().unwrap();
        left.wait().unwrap();
        assert_eq!(a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))), None);
        assert!(a.orphans.is_empty());
    }

    #[test]
    fn an_ask_during_a_freeing_waits_for_it_then_clears() {
        // (The review's case.)
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let c = home.join("cache");
        put(&c.join("base/base/6-1-1.0000000000000001.base"), &[1; 500]);
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: false,
            member: false,
        })
        .unwrap();
        *a.planner().forecast.borrow_mut() = Some(forecast_now(true));
        let (go, wait) = std::sync::mpsc::channel::<()>();
        a.caches_task = Some(CachesTask {
            ask: None,
            toward: None,
            disk: false,
            began: Instant::now(),
            thread: std::thread::spawn(move || {
                wait.recv().map(|()| room::Freed::default()).map_err(anyhow::Error::from)
            }),
        });
        room::request_clear(&home, "scenic clean on m4", room::Level::Caches).unwrap();
        let settle = |a: &mut Agent| {
            while a.caches_task.as_ref().is_some_and(|t| !t.thread.is_finished()) {
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        // The ask waits for the freeing under way; once it's done, it's taken up, and cleared.
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(home.join(room::CLEAR_REQUEST).exists());
        go.send(()).unwrap();
        settle(&mut a);
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(a.caches_task.as_ref().is_some_and(|t| t.ask.is_some()));
        settle(&mut a);
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(a.mem.cleared.as_ref().is_some_and(|f| f.why_not.is_none() && f.freed.get("base") == Some(&500)));
        assert!(!c.join("base").exists());
    }

    #[test]
    fn a_member_clears_its_caches_whatever_the_build_has_left() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/writer"), "the-lead").unwrap();
        let c = home.join("cache");
        let tif = crate::whole::testfiles::tiff(false);
        put(&c.join("chm10/a.tif"), &tif);
        let mut a = running_agent(&store::nasfs::NasRoot::local_for_test(&root), &home, true);
        let beat = |at: u64, done: bool, job: Option<&str>| {
            let job = job.map(|id| JobView {
                id: id.into(),
                what: format!("Ranking the world's place labels ({id})"),
                started: at,
                paused: None,
                pausing: None,
                tail: String::new(),
                parts: Vec::new(),
                part: None,
                progress: None,
                mem_mb: None,
                threads: None,
                waiting: None,
                slot: Some(0),
                live: None,
            });
            let st = Status {
                host: "the-lead".into(),
                beat: at,
                job,
                forecast: Some(forecast::Forecast { at, ..forecast_now(done) }),
                ..Default::default()
            };
            std::fs::write(root.join("state/status.json"), serde_json::to_vec(&st).unwrap()).unwrap();
        };
        // No word from the lead: nothing stops a clear (the build never ends).
        assert_eq!(a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))), None);
        assert!(c.join("chm10/a.tif").exists() && a.caches_task.is_none(), "nothing cleared unasked");
        // The build with work left and a job of the lead's running: asked, cleared.
        beat(now_s(), false, Some("labels 2026-09-28"));
        let r = room::request_clear(&home, "scenic clean on m1", room::Level::Caches).unwrap();
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        while a.caches_task.as_ref().is_some_and(|t| !t.thread.is_finished()) {
            std::thread::sleep(Duration::from_millis(10));
        }
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        let f = a.mem.cleared.clone().unwrap();
        assert_eq!(
            (f.asked, f.why_not.as_deref(), f.freed.get("canopy")),
            (Some(r.at), None, Some(&(tif.len() as u64)))
        );
        assert!(!c.join("chm10/a.tif").exists() && a.mem.declined.is_none());
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
        let mut a = running_agent(&store::nasfs::NasRoot::local_for_test(&root), &home, false);
        *a.planner().forecast.borrow_mut() = Some(forecast_now(false));
        let asked = |home: &Path| home.join(room::CLEAR_REQUEST).exists() || home.join(room::CLEAR_TAKEN).exists();
        // Asked while a job an earlier agent left runs here (its programs may not hold what they
        // use): not cleared, and why said; the ask taken up all the same.
        use std::os::unix::process::CommandExt;
        let mut left =
            store::spawn::spawn(store::spawn::command("/bin/sh").args(["-c", "sleep 3600"]).process_group(0)).unwrap();
        let pgid = left.id() as i32;
        a.orphans.push(jobs::Group {
            pgid,
            leader_start: crate::sys::process_start(pgid).unwrap(),
            started: now_s() - 5,
            id: "backup".into(),
        });
        let r = room::request_clear(&home, "scenic clean on m4", room::Level::Caches).unwrap();
        a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        let f = a.mem.declined.clone().unwrap();
        assert_eq!(
            (f.asked, f.bytes(), f.why_not.as_deref()),
            (Some(r.at), 0, Some("a job an earlier agent left still runs here (backup)"))
        );
        assert!(!asked(&home) && c.join("chm10/a.tif").exists() && a.mem.cleared.is_none());
        left.kill().unwrap();
        left.wait().unwrap();
        // The build with work left, a job of this agent's running: cleared, the canopy squares too, and what
        // it freed said.
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(a.try_start(
            0,
            waiting_job("backup"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let r = room::request_clear(&home, "the menu bar on m4", room::Level::Caches).unwrap();
        assert_eq!(a.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root))), None);
        let f = a.mem.cleared.clone().unwrap();
        assert_eq!((f.asked, f.by.as_deref(), f.why_not.as_deref()), (Some(r.at), Some("the menu bar on m4"), None));
        assert_eq!(
            f.freed,
            BTreeMap::from([
                ("base".to_string(), 500),
                ("blobs".to_string(), 1000),
                ("canopy".to_string(), tif.len() as u64)
            ])
        );
        assert!(
            !c.join("chm10/a.tif").exists()
                && root.join("sources/canopy/a.tif").exists()
                && c.join("unit-stages.json").exists()
        );
        assert!(!asked(&home));
        assert_eq!(a.cache_size.borrow().1, Some(room::Sizes::default()), "counted again: nothing left to clear");
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        // Declined later (no NAS): said apart, the last clear done kept.
        room::request_clear(&home, "scenic clean on m4", room::Level::Caches).unwrap();
        a.tend_caches(None);
        assert_eq!(a.mem.declined.as_ref().and_then(|f| f.why_not.as_deref()), Some("the NAS isn't reachable"));
        assert_eq!(a.mem.cleared.as_ref().map(|f| f.asked), Some(Some(r.at)));
        // A dry run beside it (another agent runs the jobs) takes up no ask.
        room::request_clear(&home, "scenic clean on m4", room::Level::Caches).unwrap();
        let mut dry = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        *dry.planner().forecast.borrow_mut() = Some(forecast_now(true));
        dry.tend_caches(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(home.join(room::CLEAR_REQUEST).exists());
    }

    #[test]
    fn the_lead_notes_a_members_clears_in_the_history_once() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = running_agent(&store::nasfs::NasRoot::local_for_test(&root), &home, false);
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0));
        let helper = |at: u64, bytes: u64| Status {
            host: "m1".into(),
            caches: Some(room::Caches {
                cleared: Some(room::Freed {
                    at,
                    freed: BTreeMap::from([("canopy".to_string(), bytes)]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        // One from before this agent started (the one before noted it), then a new one, twice.
        a.note_helpers_caches(&[helper(a.started - 60, 3 << 30)]);
        a.note_helpers_caches(&[helper(a.started + 5, 3 << 30)]);
        a.note_helpers_caches(&[helper(a.started + 5, 3 << 30)]);
        let (_, _, events, _) = a.coord.as_ref().unwrap().for_forecast();
        let noted: Vec<(Option<&str>, &str)> =
            events.iter().filter(|e| e.kind == "caches").map(|e| (e.worker.as_deref(), e.note.as_str())).collect();
        assert_eq!(noted, [(Some("m1"), "cleared its build caches: 3.0 GB freed (canopy squares 3.0 GB)")]);
    }

    /// A root as the build has it: a pass whose reaches build Reykjavik's z6 tile (6/28/17) for
    /// region a, the heritage sites and the terrain made (the z3 tile's lo pack a real pack), and the
    /// z6 tile to build.
    fn rekey_root(root: &store::nasfs::NasRoot) {
        rekey_root_with(root, [-220_000_000, 640_000_000, -217_000_000, 641_600_000])
    }

    /// `rekey_root`, the z6 tile's ways where `owned` says (E7).
    fn rekey_root_with(root: &store::nasfs::NasRoot, owned: [i32; 4]) {
        let date = "2026-09-28";
        root.join("catalog").create_dir_all().unwrap();
        put(&root.unchecked_path().join(format!("sources/osm/{date}/pass.1111111111111111.json")), b"{}");
        let recipes =
            vec![recipes::Recipe { id: "a".into(), name: "A".into(), outline: vec!["place:-21.9,64.13,20".into()] }];
        let cov =
            crate::coverage::Coverage::fixture(&recipes, None, &root.unchecked_path().join("inputs/outlines")).unwrap();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        crate::inputs::regions::accept_fixture(root, (&root.join("inputs/outlines")).into(), &recipes, &mut m);
        let piece = format!("sources/osm/{date}/pieces/6-28-17");
        m.insert(piece.clone(), format!("{piece}.4444444444444444.osm.pbf"));
        let mut reaches = crate::reach::Reaches { fmt: 1, date: date.into(), ..Default::default() };
        reaches.units.insert("6/28/17".into(), crate::reach::Reach { cells: vec![], owned: Some(owned), long: vec![] });
        let rc = format!("{}.5555555555555555.json.zst", crate::reach::logical(date));
        put(&root.unchecked_path().join(&rc), &reaches.encode().unwrap());
        m.insert(crate::reach::logical(date), rc);
        // (What the z6 tiles wait for: the heritage sites' inputs and outputs, the buildings' index.)
        m.insert(
            crate::osmpass::set_name(date, "areas"),
            format!("sources/osm/{date}/sets/areas.1212121212121212.osm.pbf"),
        );
        crate::inputs::heritage::accept_fixture(root, &mut m);
        crate::inputs::elevation::accept_fixture(root, &mut m);
        m.insert(
            crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE),
            format!("{}.6767676767676767.json", crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE)),
        );
        m.insert(
            crate::heritage::base_logical(date, "heritage-sources"),
            format!("work/heritage/{date}/base/heritage-sources.5656565656565656.json"),
        );
        let lo = "layers/terrain/lo/3-3-2.1111111111111111.pack";
        root.join(lo).parent().unwrap().create_dir_all().unwrap();
        let mut w = store::pack::PackWriter::create(
            &root.unchecked_path().join(lo),
            serde_json::json!({"layer": "terrain"}),
            false,
        )
        .unwrap();
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
        for (l, c) in [
            ("layers/trees-cover/lo/3-3-2", "layers/trees-cover/lo/3-3-2.7777777777777777.pack"),
            ("layers/trees-cover/hi/6-28-17", "layers/trees-cover/hi/6-28-17.8888888888888888.pack"),
        ] {
            put(&root.unchecked_path().join(c), b"pack");
            m.insert(l.into(), c.into());
        }
        put(&root.unchecked_path().join("state/build/manifest.json"), &serde_json::to_vec(&m).unwrap());
        let mut keys = build::Keys::default();
        keys.lo.insert("reach".into(), build::reach_key(date, &m).unwrap());
        keys.record(
            "heritage-sites",
            &build::heritage_sites_work(&cov, date, &m, &keys, &input_digests(root)).unwrap().targets,
        );
        keys.record("trees", &rekey::v1::trees_targets(&cov, &m));
        let tiles = {
            let mut t = tiles::TerrainTiles::new(None);
            t.load(root, &m);
            t
        };
        // (The terrain made under its keys now.)
        let tt = build::terrain_slope_targets(&cov, &m, &tiles);
        keys.record("terrain-hi", &tt.terrain);
        keys.record("terrain-lo", &tt.terrain_lo);
        let plan = build::plan(
            &cov,
            date,
            &m,
            &keys,
            &input_digests(root),
            Some(&reaches),
            &tiles,
            build::Publish {
                each: &cov.by_region(),
                on_map: &BTreeMap::new(),
                served: &BTreeMap::new(),
                since_catalogue: None,
                held_for: None,
                held: false,
                way: Default::default(),
            },
        );
        assert!(plan.work.iter().any(|w| w.step == "unit" && w.targets[0].0 == "6/28/17"), "{:?}", plan.work);
        keys.save(root).unwrap();
    }

    /// Not a test: seeds the scratch folder `SCENIC_SCRATCH_PUBLISH` names (never the project's) with
    /// a region done that the map hasn't, so its lead publishes it (tools/check/pool-two-macs.sh
    /// `publish`): `rekey_root`'s state, the z6 tile's ways across four z6 tiles (map tiles to share), and
    /// the z6 tile built as the plan wants it now.
    #[test]
    #[ignore = "seeds a scratch folder for tools/check/pool-two-macs.sh publish"]
    fn seed_a_scratch_publish() {
        let Some(root) = std::env::var_os("SCENIC_SCRATCH_PUBLISH").map(PathBuf::from) else { return };
        assert!(!root.to_string_lossy().contains("projects/scenic-roads"), "not the project folder");
        let date = "2026-09-28";
        rekey_root_with(
            &store::nasfs::NasRoot::local_for_test(&root),
            [-220_000_000, 640_000_000, -10_000_000, 641_600_000],
        );
        let m: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(root.join("state/build/manifest.json")).unwrap()).unwrap();
        let cov = crate::coverage::Coverage::accepted(
            &store::nasfs::NasRoot::local_for_test(&root),
            &crate::inputs::regions::read::source(&store::nasfs::NasRoot::local_for_test(&root), &m).unwrap(),
            None,
        )
        .unwrap();
        let reaches = crate::reach::Reaches::load(&store::nasfs::NasRoot::local_for_test(&root), &m, date)
            .ok()
            .flatten()
            .expect("the reaches");
        let mut keys = build::Keys::load(&store::nasfs::NasRoot::local_for_test(&root));
        let tiles = {
            let mut t = tiles::TerrainTiles::new(None);
            t.load(&store::nasfs::NasRoot::local_for_test(&root), &m);
            t
        };
        let plan = build::plan(
            &cov,
            date,
            &m,
            &keys,
            &input_digests(&store::nasfs::NasRoot::local_for_test(&root)),
            Some(&reaches),
            &tiles,
            build::Publish {
                each: &cov.by_region(),
                on_map: &BTreeMap::new(),
                served: &BTreeMap::new(),
                since_catalogue: None,
                held_for: None,
                held: false,
                way: Default::default(),
            },
        );
        let unit = plan.work.iter().find(|w| w.step == "unit").expect("the unit to build").targets.clone();
        keys.record("unit", &unit);
        keys.save(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        eprintln!("seeded {}: unit {:?} built", root.display(), unit);
    }

    #[test]
    fn the_passs_outlines_come_first_and_hold_the_rest() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        rekey_root(&store::nasfs::NasRoot::local_for_test(&root));
        let date = "2026-09-28";
        let mf = root.join("state/build/manifest.json");
        let mut m: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(&mf).unwrap()).unwrap();
        m.insert(
            crate::osmpass::set_name(date, "outlines"),
            format!("sources/osm/{date}/sets/outlines.2323232323232323.osm.pbf"),
        );
        // (Outlines the pass made itself, before the step: no record of the step's.)
        m.insert(format!("sources/osm/{date}/outlines"), format!("sources/osm/{date}/outlines.2424242424242424.sect"));
        put(&mf, &serde_json::to_vec(&m).unwrap());
        let mut a = agent(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("home"));
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let ids = |plan: &[JobSpec]| plan.iter().map(|j| j.id.clone()).collect::<Vec<_>>();
        let mut w = Vec::new();
        let plan = a.planner().plan(&store::nasfs::NasRoot::local_for_test(&root), &cond, &mut w);
        let o = plan.iter().find(|j| j.id == format!("outlines {date}")).unwrap_or_else(|| panic!("{:?}", ids(&plan)));
        assert_eq!(o.record.as_ref().map(|r| r.step.as_str()), Some("outlines"));
        assert!(o.cmd[1] == "outlines" && o.cmd.windows(2).any(|p| p == ["--pass", date]), "{:?}", o.cmd);
        // Nothing else of the build meanwhile.
        assert!(
            !ids(&plan).iter().any(|i| i.starts_with("unit ") || i.starts_with("spoken") || i.starts_with("lakefill")),
            "{:?}",
            ids(&plan)
        );
        assert!(w.iter().any(|x| x.why.contains("outlines job")), "{w:?}");
        // Made under its key: the rest comes back, and no outlines job.
        let mut keys = build::Keys::load(&store::nasfs::NasRoot::local_for_test(&root));
        keys.record("outlines", &build::outlines_work(date, &m, &keys).unwrap().targets);
        keys.save(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        let plan = a.planner().plan(&store::nasfs::NasRoot::local_for_test(&root), &cond, &mut Vec::new());
        assert!(!ids(&plan).iter().any(|i| i.starts_with("outlines")) && !ids(&plan).is_empty(), "{:?}", ids(&plan));
    }

    #[test]
    fn the_checklist_says_which_steps_helpers_take() {
        let steps = build::fold(&build::checklist_to_come(), &BTreeMap::new(), &BTreeMap::new());
        let shared = |id: &str| steps.iter().find(|s| s.id == id).and_then(|s| s.shared.clone());
        assert_eq!(shared("terrain").as_deref(), Some("terrain-hi and terrain-deep"), "the pieces, not the assemblies");
        assert_eq!(shared("slope").as_deref(), Some("slope-deep and slope-hi"));
        assert_eq!(shared("roads").as_deref(), Some("all"));
        assert_eq!(shared("landmarks").as_deref(), Some("pois and peaks"));
        assert_eq!(shared("trees").as_deref(), Some("trees"), "the pieces, not the assemblies");
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
        put(
            &root.join("state/build/manifest.json"),
            &serde_json::to_vec(&BTreeMap::from([(sw.clone(), name)])).unwrap(),
        );
        let mut a = agent(&store::nasfs::NasRoot::local_for_test(&root), &home);
        let (mut w, mut held) = (Vec::new(), Holds::default());
        assert!(a
            .planner()
            .region_jobs(&store::nasfs::NasRoot::local_for_test(&root), None, None, &mut w, &mut held)
            .is_empty());
        assert!(
            w.iter().any(|x| x.why.contains(
                "older than the one the newer readers' switch (the regions', the elevation datasets') was recorded on"
            )),
            "{w:?}"
        );
        // Its file unreadable: unknown, so nothing either.
        put(
            &root.join("state/build/manifest.json"),
            &serde_json::to_vec(&BTreeMap::from([(sw, "gone.json".to_string())])).unwrap(),
        );
        let mut w = Vec::new();
        assert!(a
            .planner()
            .region_jobs(&store::nasfs::NasRoot::local_for_test(&root), None, None, &mut w, &mut held)
            .is_empty());
        assert!(w.iter().any(|x| x.why.contains("can't be read now")), "{w:?}");
    }

    #[test]
    fn plans_daily_jobs_and_writes_a_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let mut m = BTreeMap::new();
        crate::inputs::regions::accept_fixture(
            &store::nasfs::NasRoot::local_for_test(&root),
            (&root).into(),
            &[recipes::Recipe { id: "x".into(), name: "X".into(), outline: vec!["osm:1".into()] }],
            &mut m,
        );
        put(&root.join("state/build/manifest.json"), &serde_json::to_vec(&m).unwrap());
        let mut a = agent(&store::nasfs::NasRoot::local_for_test(&root), &home);
        a.round().unwrap();
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&root)), &home).unwrap();
        assert_eq!(st.regions.len(), 1);
        assert!(st.conditions.nas);
        // (The regions' check of their accepted version comes first, the gate's jobs first of all;
        // the backup after it.)
        assert!(
            st.waiting.iter().any(|w| w.what == "backup" || w.what.starts_with("Checking the regions inputs")),
            "{:?}",
            st.waiting
        );
        // Once backed up today, it isn't due.
        a.mem.last_ok.insert("backup".into(), now_s());
        let mut w = Vec::new();
        assert!(!a
            .planner()
            .plan(&store::nasfs::NasRoot::local_for_test(&root), &st.conditions, &mut w)
            .iter()
            .any(|j| j.id == "backup"));
    }

    /// The gate in the agent (docs/inputs.md §4.2, §4.3, §4.7): the inputs folders listed off the loop,
    /// a unit's check planned before every other job while its key isn't the one recorded (a full
    /// one daily), the status's entry; an acceptance asked of the agent written by it.
    #[test]
    fn the_gate_plans_a_check_first_and_shows_in_the_status() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let unit = crate::inputs::TEST_UNIT;
        let inputs_dir = root.join("inputs").join(unit);
        std::fs::create_dir_all(&inputs_dir).unwrap();
        let put = |text: &str| {
            let p = inputs_dir.join("a.jsonl");
            std::fs::write(&p, text).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(60))
                .unwrap();
        };
        put("{\"k\": \"x\", \"v\": -1}\n");
        let mut a = agent(&store::nasfs::NasRoot::local_for_test(&root), &home);
        // Off the gate (no flag): nothing planned.
        a.round().unwrap();
        let c = read_status(Some(&store::nasfs::NasRoot::local_for_test(&root)), &home).unwrap().conditions;
        let mut w = Vec::new();
        assert!(!a
            .planner()
            .plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w)
            .iter()
            .any(|j| j.id.starts_with("inputs ")));
        std::fs::create_dir_all(root.join("state/inputs")).unwrap();
        std::fs::write(root.join(crate::inputs::TEST_FLAG), b"").unwrap();
        let watch = |a: &mut Agent| a.planner().inputs.watch.as_ref().unwrap().1.get(unit);
        let ask = |a: &mut Agent| a.planner().inputs.watch.as_ref().unwrap().1.ask();
        ask(&mut a);
        let t = Instant::now();
        while watch(&mut a).is_none() {
            assert!(t.elapsed() < store::testkit::WATCHDOG, "listed");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The first check: a full one, before every other job.
        let plan = a.planner().plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w);
        assert_eq!(plan[0].id, format!("inputs {unit} full"));
        assert!(
            plan[0].cmd[1] == "inputs"
                && plan[0].cmd.ends_with(&["--full".to_string()])
                && plan[0].cmd.contains(&unit.to_string())
        );
        // (Given the listing and acceptances its key was made from: inputs::Planned.)
        let at = plan[0].cmd.iter().position(|a| a == "--listing").unwrap();
        let given: crate::inputs::Planned =
            serde_json::from_slice(&std::fs::read(&plan[0].cmd[at + 1]).unwrap()).unwrap();
        assert_eq!(given.listing.files.keys().cloned().collect::<Vec<_>>(), ["a.jsonl"]);
        let work = plan[0].record.clone().unwrap();
        assert_eq!(work.targets[0].0, format!("inputs/{unit}"));
        // Run (as its job would) and recorded: a normal check planned next (its key without the full
        // one's), then nothing while nothing changes.
        let mut out =
            crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s")).unwrap();
        crate::inputs::gate::run(&mut out, unit, true, None).unwrap();
        let mut keys = build::Keys::load_strict(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        keys.record("inputs", &work.targets);
        keys.save(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        // (The full check's time is the records', not this agent's: no daily full check due now,
        // for this agent or a new lead's, `b`.)
        let plan = a.planner().plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w);
        assert_eq!(plan[0].id, format!("inputs {unit}"));
        let work = plan[0].record.clone().unwrap();
        let mut keys = build::Keys::load_strict(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        keys.record("inputs", &work.targets);
        keys.save(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        assert!(!a
            .planner()
            .plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w)
            .iter()
            .any(|j| j.id.starts_with("inputs ")));
        // Held (the negative v): the status says so.
        a.round().unwrap();
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&root)), &home).unwrap();
        let v = st.inputs.iter().find(|v| v.unit == unit).unwrap();
        assert_eq!(v.state, crate::inputs::view::State::Held);
        assert_eq!(v.held, ["a.jsonl"]);
        // An acceptance asked of the agent: written by it (not by a dry run's), and the inputs folders
        // listed again, so its key changes and the unit's checked again.
        a.o.dry_run = false;
        crate::inputs::ask(
            &home,
            &crate::inputs::Ask { unit: unit.into(), all: true, by: "a test".into(), ..Default::default() },
        )
        .unwrap();
        a.planner().tend_gate(&store::nasfs::NasRoot::local_for_test(&root));
        a.o.dry_run = true;
        assert_eq!(crate::inputs::acceptances(&store::nasfs::NasRoot::local_for_test(&root), unit).unwrap().len(), 1);
        let t = Instant::now();
        while watch(&mut a).is_none_or(|l| l.accepted.is_empty()) {
            assert!(t.elapsed() < store::testkit::WATCHDOG, "listed again");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            a.planner().plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w)[0].id,
            format!("inputs {unit}")
        );
        drop(a);
        let mut b = agent(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("home-b"));
        b.round().unwrap();
        let t = Instant::now();
        while watch(&mut b).is_none() {
            assert!(t.elapsed() < store::testkit::WATCHDOG, "listed");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            b.planner()
                .plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w)
                .iter()
                .filter(|j| j.id.starts_with("inputs "))
                .all(|j| !j.id.ends_with(" full")),
            "a new lead doesn't repeat a full check less than a day old"
        );
        // Off the gate: no longer shown, nor checked; its records forgotten when asked, by a job of
        // the lead's.
        std::fs::remove_file(root.join(crate::inputs::TEST_FLAG)).unwrap();
        b.round().unwrap();
        assert!(read_status(Some(&store::nasfs::NasRoot::local_for_test(&root)), &d.path().join("home-b"))
            .unwrap()
            .inputs
            .is_empty());
        b.o.dry_run = false;
        crate::inputs::ask(
            &d.path().join("home-b"),
            &crate::inputs::Ask { unit: unit.into(), forget: true, ..Default::default() },
        )
        .unwrap();
        b.planner().tend_gate(&store::nasfs::NasRoot::local_for_test(&root));
        b.o.dry_run = true;
        let plan = b.planner().plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w);
        let f = plan.iter().find(|j| j.id == format!("inputs {unit} forget")).expect("a job forgetting its records");
        assert!(f.cmd.ends_with(&["--forget".to_string()]));
        let mut out =
            crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s")).unwrap();
        crate::inputs::gate::forget(&mut out, unit).unwrap();
        let m: BTreeMap<String, String> =
            crate::out::read_record(&store::nasfs::NasPath::local_for_test(&root.join("state/build/manifest.json")))
                .unwrap();
        assert!(!m.keys().any(|l| crate::inputs::unit_of(l) == Some(unit)));
        assert!(
            !b.planner()
                .plan(&store::nasfs::NasRoot::local_for_test(&root), &c, &mut w)
                .iter()
                .any(|j| j.id.starts_with("inputs ")),
            "nothing left to forget"
        );
    }

    #[test]
    fn the_agents_own_footprint_comes_off_what_its_jobs_may_hold() {
        // (#619: the Mac mini's agent held 9.5 GB beside jobs given 28 of its 32 GB.)
        let d = tempfile::tempdir().unwrap();
        let mut a = agent(&store::nasfs::NasRoot::local_for_test(&d.path().join("nas")), &d.path().join("home"));
        a.mem_set = Some((32 << 10, 20 << 10));
        a.own_mb = 9 << 10;
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert_eq!(a.guard_view().limit_mb, 19 << 10);
        assert_eq!(a.build_room(), 19 << 10);
        assert_eq!(a.capacity(&cond).limit_mb, 19 << 10, "the coordinator told the same");
        // Measured each loop where the Mac's memory is read, not set by a test.
        a.mem_set = None;
        a.total = a.mac_total_mb();
        a.own_mb = crate::sys::footprint().map_or(0, |b| b >> 20);
        assert!(a.own_mb > 0 && a.jobs_limit(a.total) == memguard::limit_mb(a.total).saturating_sub(a.own_mb));
    }

    #[test]
    fn a_restart_reads_no_geometry_off_the_nas() {
        // (#608: ~1000 small reads over SMB held a restarted agent's first round 20 minutes.)
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        let r = |p: &str| {
            vec![recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec![format!("place:{p},20")] }]
        };
        let rings = |c: &Coverages| c.all.shapes.iter().map(|s| s.rings.clone()).collect::<Vec<_>>();
        let mut m1 = BTreeMap::new();
        crate::inputs::regions::accept_fixture(
            &store::nasfs::NasRoot::local_for_test(&root),
            (&root).into(),
            &r("-21.9,64.13"),
            &mut m1,
        );
        let a = crate::inputs::regions::read::source(&store::nasfs::NasRoot::local_for_test(&root), &m1).unwrap();
        // A kept file of another format is made again in its place, not read past every time.
        let kept = home.join("cache/coverage").join(format!("{}.cov", store::naming::hash16(a.version.as_bytes())));
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, b"RDCOV001").unwrap();
        let first = rings(
            &agent(&store::nasfs::NasRoot::local_for_test(&root), &home)
                .planner()
                .coverage(&store::nasfs::NasRoot::local_for_test(&root), &m1, false)
                .unwrap(),
        );
        assert!(crate::coverage::from_kept(&mut &std::fs::read(&kept).unwrap()[..]).is_some());
        let geoms: Vec<PathBuf> = crate::inputs::regions::read::listed(&a)
            .unwrap()
            .iter()
            .map(|l| root.join(&l.stored.as_ref().unwrap().geometry))
            .collect();
        assert!(!geoms.is_empty());
        for g in &geoms {
            std::fs::remove_file(g).unwrap();
        }
        // The restart: a new agent, the same home, the files gone from the NAS: the cache answers.
        assert_eq!(
            rings(
                &agent(&store::nasfs::NasRoot::local_for_test(&root), &home)
                    .planner()
                    .coverage(&store::nasfs::NasRoot::local_for_test(&root), &m1, false)
                    .unwrap()
            ),
            first
        );
        // A new version isn't the cached one: read from its own files.
        let mut m2 = BTreeMap::new();
        crate::inputs::regions::accept_fixture(
            &store::nasfs::NasRoot::local_for_test(&root),
            (&root).into(),
            &r("-21.9,64.2"),
            &mut m2,
        );
        assert_ne!(
            rings(
                &agent(&store::nasfs::NasRoot::local_for_test(&root), &home)
                    .planner()
                    .coverage(&store::nasfs::NasRoot::local_for_test(&root), &m2, false)
                    .unwrap()
            ),
            first
        );
    }

    #[test]
    fn an_edit_to_the_regions_is_noticed() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        let r = |p: &str| {
            vec![recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec![format!("place:{p},20")] }]
        };
        let mut m1 = BTreeMap::new();
        crate::inputs::regions::accept_fixture(
            &store::nasfs::NasRoot::local_for_test(&root),
            (&root).into(),
            &r("-21.9,64.13"),
            &mut m1,
        );
        let mut m2 = BTreeMap::new();
        crate::inputs::regions::accept_fixture(
            &store::nasfs::NasRoot::local_for_test(&root),
            (&root).into(),
            &r("-21.9,64.2"),
            &mut m2,
        );
        // The agent: the first look isn't an edit; a new accepted version is (the plan's look, not
        // the heartbeat's).
        let mut a = agent(&store::nasfs::NasRoot::local_for_test(&root), &home);
        a.planner().coverage(&store::nasfs::NasRoot::local_for_test(&root), &m1, true).unwrap();
        a.planner().coverage(&store::nasfs::NasRoot::local_for_test(&root), &m1, true).unwrap();
        a.planner().coverage(&store::nasfs::NasRoot::local_for_test(&root), &m1, false).unwrap();
        assert!(a.planner().edited_at.get().is_none());
        a.planner().coverage(&store::nasfs::NasRoot::local_for_test(&root), &m2, true).unwrap();
        let (first, last) = a.planner().edited_at.get().unwrap();
        assert_eq!(edit_held(Some((first, last)), last), Some((Duration::ZERO, EDIT_HOLD)));
        // A run of edits holds the work an hour at most.
        let now = std::time::SystemTime::now();
        let ago = |m: u64| now - Duration::from_secs(m * 60);
        assert_eq!(
            edit_held(Some((ago(50), ago(1))), now),
            Some((Duration::from_secs(60), Duration::from_secs(10 * 60)))
        );
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: apps.join("v1"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        assert_eq!(a.app, "v1");
        a.round().unwrap();
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&root)), &home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("newly installed app")), "{:?}", st.waiting);
        assert!(!st.waiting.iter().any(|w| w.why.contains("would start")), "{:?}", st.waiting);
        // Pointing back at its own version: work starts again.
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        a.round().unwrap();
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&root)), &home).unwrap();
        assert!(st.waiting.iter().any(|w| w.why.contains("would start")), "{:?}", st.waiting);
    }

    /// An agent started by `app/current` starts its jobs from its own version's folder, so after
    /// `current` moves twice the agent then lists that version as a running job's (the map server's
    /// updater keeps it).
    /// Task #688: each Mac's jobs on an older app (`Capacity::old_app`, as the lead last heard it)
    /// are notices in the lead's status, by Mac, job and app.
    #[test]
    fn jobs_on_an_older_app_are_noticed() {
        let cap = |old: &[(&str, &str)]| crate::coord::Capacity {
            old_app: old
                .iter()
                .map(|(j, a)| crate::coord::OldAppJob { job: j.to_string(), app: a.to_string() })
                .collect(),
            ..Default::default()
        };
        let n = old_app_notices(&[
            ("m4".into(), cap(&[("terrain-hi 6/48/16", "20261025-1051-940d8881")])),
            ("mini".into(), cap(&[])),
        ]);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].what, "m4: terrain-hi 6/48/16");
        assert!(n[0].why.starts_with("runs app 20261025-1051-940d8881, older than its agent's"), "{}", n[0].why);
    }

    #[test]
    fn a_job_started_through_current_keeps_its_version_listed() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("catalog")).unwrap();
        let apps = d.path().canonicalize().unwrap().join("app");
        for v in ["v1", "v2", "v3"] {
            std::fs::create_dir_all(apps.join(v)).unwrap();
        }
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        let a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root)),
            home,
            bin: apps.join("current"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        assert_eq!((a.o.bin.clone(), a.app.clone()), (apps.join("v1"), "v1".to_string()));
        // Its job's command, as every job's is made.
        let cmd = a.o.bin.join("scenic-build").to_string_lossy().into_owned();
        for v in ["v2", "v3"] {
            std::fs::remove_file(apps.join("current")).unwrap();
            std::os::unix::fs::symlink(v, apps.join("current")).unwrap();
        }
        assert_eq!(job_apps_of([cmd.as_str()].into_iter(), &apps.join("v3"), "v3"), ["v1"]);
        // (A command an older agent made through `current` names no version: not followed.)
        let old = apps.join("current/scenic-build").to_string_lossy().into_owned();
        assert!(job_apps_of([old.as_str()].into_iter(), &apps.join("v3"), "v3").is_empty());
    }

    #[test]
    fn quitting_takes_no_new_work_and_ends_once_no_job_runs() {
        // (Task #194: Scenic quit, "Finish this job first" or "Stop now".)
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "bldtiles".into(), targets: vec![("6/1/1".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        assert!(a.quit_ask().is_none() && a.quit_due());
        assert!(a.try_start(
            0,
            job("bldtiles 6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let pgid = a.slots[0].running.as_ref().unwrap().pgid;
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let _kill = Kill(pgid);
        // "Finish this job first": the job goes on, nothing new starts (and the status says why),
        // the loop not ended while it runs.
        crate::control::request_quit(&home, crate::control::QuitMode::Finish, "Scenic on m1").unwrap();
        // (Taken up by control: in line here, as a single loop's round takes it.)
        a.control.look();
        let mut waiting = Vec::new();
        assert!(!a.try_start(
            1,
            job("bldtiles 6/2/2"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut waiting
        ));
        a.slots[1].lease = None;
        assert!(waiting.iter().any(|w| w.why.contains("Scenic quits once the job under way ends")), "{waiting:?}");
        a.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
        assert!(a.slots[0].running.as_ref().is_some_and(|r| r.pausing.is_none() && r.paused.is_none()));
        assert!(!a.quit_due());
        // Its job over ("Stop now" stops it so: given back, not a failure): the quit is due (this
        // Mac leads no pool), and the ask goes as the loop ends.
        a.stop_jobs("stopped: Scenic quit");
        assert!(a.slots.iter().all(|s| s.running.is_none()) && a.quit_due());
        a.quit_done = true;
        a.o.once = true;
        a.run().unwrap();
        assert!(a.quit_done() && crate::control::peek_quit(&home).is_none());
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: apps.join("v1"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str, t: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "bldtiles".into(), targets: vec![(t.into(), "k".into())] }),
        };
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
        assert!(a.try_start(
            0,
            job("bldtiles 6/1/1", "6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        assert!(a.try_start(
            1,
            job("bldtiles 6/2/2", "6/2/2"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let _kill = Kill(a.slots.iter().map(|s| s.running.as_ref().unwrap().pgid).collect());
        let tend = |a: &mut Agent| {
            for k in 0..SLOTS {
                a.tend(k, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
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
            assert_eq!(
                a.slots[k].running.as_ref().unwrap().pausing.as_deref(),
                Some("restarting into the newly installed app (v2)")
            );
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
        // Every slot free with a newer app: due at once (one free with another's job under way:
        // not yet).
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v2", apps.join("current")).unwrap();
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        assert!(!a.update_due());
        a.slots[1].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[1].running = None;
        assert!(a.update_due());
    }

    /// Task #393: a job outlives its agent's restart (a newer app, its launcher restarted): the
    /// agent ending leaves it running with its slot saved, the next agent takes it over, its lease
    /// held once (re-held under the restarted coordinator, never granted twice), and tends it to
    /// its end, its lease finished and its record gone; nothing of it left running.
    #[test]
    fn a_job_survives_its_agents_restart_taken_over_by_the_next() {
        let (old, new) = ("20261018-0100-aaaaaaa", "20261018-0300-bbbbbbb");
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let apps = d.path().join("app");
        std::fs::create_dir_all(apps.join(old)).unwrap();
        std::fs::create_dir_all(apps.join(new)).unwrap();
        std::os::unix::fs::symlink(old, apps.join("current")).unwrap();
        let opts = |bin: &str| Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: apps.join(bin),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut a = test_agent(opts(old)).unwrap();
        a.free_set = Some(40 << 30);
        let coord = d.path().join("coord");
        let co = std::sync::Arc::new(crate::coord::start_for_test(&coord, &a.me, old).0);
        a.coord = Some(co.clone());
        let go = d.path().join("go");
        let job = JobSpec {
            id: "terrain-hi 6/55/25".into(),
            what: "terrain-hi 6/55/25".into(),
            cmd: vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("while [ ! -e '{}' ]; do sleep 0.1; done; echo done", go.display()),
            ],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "terrain-hi".into(), targets: vec![("6/55/25".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        assert!(a.try_start(
            0,
            job.clone(),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let pgid = a.slots[0].running.as_ref().unwrap().pgid;
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let _kill = Kill(pgid);
        assert!(co.held("terrain-hi").contains("6/55/25"));
        // The agent ends (a newer app, a SIGTERM): the job left running, its record kept with its
        // slot, its lease not ended.
        a.hand_over_jobs();
        assert!(a.slots[0].running.is_none() && a.slots[0].lease.is_none());
        assert!(crate::sys::signal_group(pgid, crate::sys::Signal::Probe), "the job runs on");
        assert!(a.record_path(0).exists());
        assert!(co.held("terrain-hi").contains("6/55/25"), "its lease kept for the next agent");
        let me = a.me.clone();
        drop(a);
        // The coordinator restarts with it, the own leases of the agent before ended with it.
        co.stop();
        drop(co);
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink(new, apps.join("current")).unwrap();
        let co = std::sync::Arc::new(crate::coord::start_for_test(&coord, &me, new).0);
        // The next agent takes it over: its slot, its lease (held again here once its coordinator
        // is there: `beat`, as at any lease lost), the group the job's.
        let mut b = test_agent(opts(new)).unwrap();
        b.coord = Some(co.clone());
        let r = b.slots[0].running.as_ref().expect("taken over");
        assert!(r.adopted && r.pgid == pgid && r.spec == job);
        assert!(matches!(b.slots[0].lease, Some(Held::Own(_))));
        assert!(b.beat(0, Some(&store::nasfs::NasRoot::local_for_test(&root))));
        assert!(co.held("terrain-hi").contains("6/55/25"));
        assert_eq!(co.hold("terrain-hi", &job.record.as_ref().unwrap().targets), None, "never held twice");
        // Its app's folder kept for it while it runs (the map server's updater reads it).
        assert_eq!(b.job_apps(), Vec::<String>::new(), "its programs aren't in an app's folder here");
        // Tended to its end: its status read from its file (not this process's child), its lease
        // finished, its record gone, nothing of it left.
        assert_eq!(b.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap(), false);
        std::fs::write(&go, "").unwrap();
        let t = Instant::now();
        while !b.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap() {
            assert!(t.elapsed() < store::testkit::WATCHDOG, "the job ends");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(b.slots[0].running.is_none() && b.slots[0].lease.is_none());
        assert!(!co.held("terrain-hi").contains("6/55/25"));
        assert!(!b.record_path(0).exists());
        // (Its head, still this test's child as it isn't once its agent has ended, reaped here as
        // launchd would.)
        unsafe { libc::waitpid(pgid, std::ptr::null_mut(), 0) };
        assert!(!crate::sys::signal_group(pgid, crate::sys::Signal::Probe), "no process of it left");
    }

    /// Task #588: a fetching job taken over keeps its way out. The agent before stops its proxy as
    /// it lets go of its lock; the next starts its own on the port the job was told as it's made
    /// (before its loop), and the job's next request, sandboxed, goes through it, metered there.
    #[test]
    fn a_job_taken_over_reaches_out_through_the_next_agents_proxy() {
        use std::io::{Read, Write};
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        // (Every host the proxies open a connection to: this server, answering each request.)
        let up = store::spawn::listen("127.0.0.1:0").unwrap();
        let up_addr = up.local_addr().unwrap();
        up.set_nonblocking(false).unwrap();
        std::thread::spawn(move || {
            #[allow(clippy::disallowed_methods)]
            while let Ok((mut s, _)) = up.accept() {
                let mut b = [0u8; 4096];
                let _ = s.read(&mut b);
                let _ = write!(s, "HTTP/1.0 200 OK\r\n\r\nanswered\n");
            }
        });
        crate::budget::TEST_RESOLVE.with(|r| {
            *r.borrow_mut() = Some(std::sync::Arc::new(move |_: &str, _: u16| Some(up_addr)));
        });
        let opts = || Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: d.path().join("bin"),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut a = test_agent(opts()).unwrap();
        a.free_set = Some(40 << 30);
        let port = a.proxy.as_ref().expect("its proxy").addr.port();
        let (first, go) = (d.path().join("first"), d.path().join("go"));
        let get = format!("/usr/bin/curl -sS -m 20 http://en.{}/w/api.php", crate::budget::WIKIMEDIA[0]);
        let script = format!(
            "{get} > '{0}.tmp' && mv '{0}.tmp' '{0}'; while [ ! -e '{1}' ]; do sleep 0.05; done; {get}",
            first.display(),
            go.display()
        );
        let job = JobSpec {
            id: "items items".into(),
            what: "items".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), script],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "items".into(), targets: vec![("items".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        assert!(a.try_start(0, job, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        let pgid = a.slots[0].running.as_ref().unwrap().pgid;
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let _kill = Kill(pgid);
        let t = Instant::now();
        while !first.exists() {
            assert!(t.elapsed() < store::testkit::WATCHDOG, "its first request answered");
            std::thread::sleep(Duration::from_millis(20));
        }
        let wiki = |p: &crate::budget::Proxy| p.view().wiki;
        assert_eq!(wiki(a.proxy.as_ref().unwrap()), 1);
        // The agent ends, its job left running: its proxy stopped, then its lock let go (dropped
        // in that order).
        a.hand_over_jobs();
        drop(a);
        // (The next agent's proxy started as it's made, before its loop's first `tend_proxy`.)
        let mut b = Agent::new_with(opts(), |r| r.join("catalog").is_dir()).unwrap();
        b.mac = cond::Mac::TEST;
        assert!(b.slots[0].running.as_ref().is_some_and(|r| r.adopted && r.pgid == pgid));
        let p = b.proxy.as_ref().expect("the next agent's proxy, at once");
        assert_eq!(p.addr.port(), port, "on the port its job was told");
        assert_eq!(wiki(p), 0);
        // Its next request: answered through the next agent's proxy, metered there.
        std::fs::write(&go, "").unwrap();
        let r = b.slots[0].running.as_mut().unwrap();
        let st = r.wait().unwrap();
        assert!(st.success(), "{}", std::fs::read_to_string(&r.log).unwrap_or_default());
        assert_eq!(wiki(b.proxy.as_ref().unwrap()), 1, "metered by the next agent's proxy");
        b.slots[0].running = None;
        unsafe { libc::waitpid(pgid, std::ptr::null_mut(), 0) };
    }

    /// Task #393 review: a job taken over while this Mac's build is paused is told "drain" at once,
    /// never "run", so it passes no safe point.
    #[test]
    fn a_job_taken_over_while_paused_is_told_to_drain() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let opts = || Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: d.path().join("bin"),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut a = test_agent(opts()).unwrap();
        a.free_set = Some(40 << 30);
        let job = JobSpec {
            id: "terrain-hi 6/55/25".into(),
            what: "terrain-hi 6/55/25".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "terrain-hi".into(), targets: vec![("6/55/25".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        assert!(a.try_start(0, job, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        let pgid = a.slots[0].running.as_ref().unwrap().pgid;
        a.hand_over_jobs();
        drop(a);
        let p = crate::control::Pause::new(crate::control::Mode::Drain, "test");
        std::fs::write(home.join("pause.json"), serde_json::to_vec(&Some(p)).unwrap()).unwrap();
        let mut b = test_agent(opts()).unwrap();
        assert!(b.slots[0].running.as_ref().is_some_and(|r| r.adopted && r.pgid == pgid));
        assert_eq!(std::fs::read(b.control_path(0)).unwrap(), b"drain");
        b.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        unsafe { libc::waitpid(pgid, std::ptr::null_mut(), 0) };
    }

    /// Task #393: a record an older app wrote (no spec, no slot saved) isn't taken over: its job is
    /// stopped as before (jobs::stop_orphan), and so is a helper's leased job at the agent's end.
    #[test]
    fn a_job_with_no_saved_slot_is_stopped_not_taken_over() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let mut c = store::spawn::command("/bin/sleep");
        c.arg("3600");
        crate::sys::own_group(&mut c);
        let mut child = store::spawn::spawn(&mut c).unwrap();
        let pgid = child.id() as i32;
        let rec = serde_json::json!({
            "id": "unit 6/1/2", "pgid": pgid, "started": now_s(),
            "leader_start": crate::sys::process_start(pgid).unwrap(),
        });
        std::fs::write(home.join("job.json"), rec.to_string()).unwrap();
        let o = Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root)),
            home: home.clone(),
            bin: d.path().join("bin"),
            dry_run: false,
            once: true,
            member: false,
        };
        let b = test_agent(o).unwrap();
        assert!(b.slots[0].running.is_none(), "not taken over");
        assert!(home.join("job.json").exists(), "left to `run`'s stop of orphans");
        assert!(jobs::adopt(&home.join("job.json")).is_none());
        // (Reaped as it ends, as its own parent would: a zombie would hold the stop to its 30 s.)
        let reaper = std::thread::spawn(move || child.wait());
        jobs::stop_orphan(&home.join("job.json"));
        let _ = reaper.join();
        assert!(!crate::sys::signal_group(pgid, crate::sys::Signal::Probe));
    }

    #[test]
    fn a_cancelled_job_stops_cleanly_frees_the_update_and_reruns_on_the_newer_app() {
        // (Task #373: a long job on old code dropped so it reruns on the new app, in one command:
        // stopped and given back, not failed, its scratch for the target cleared, the target held
        // until the lead runs a newer app, and the update waiting on it free to restart at once.)
        let (old, new) = ("20261018-0100-aaaaaaa", "20261018-0300-bbbbbbb");
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let apps = d.path().join("app");
        std::fs::create_dir_all(apps.join(old)).unwrap();
        std::fs::create_dir_all(apps.join(new)).unwrap();
        std::os::unix::fs::symlink(old, apps.join("current")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: apps.join(old),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let coord = d.path().join("coord");
        let co = std::sync::Arc::new(crate::coord::start_for_test(&coord, &a.me, old).0);
        a.coord = Some(co.clone());
        let scratch = d.path().join("scratch");
        std::fs::create_dir_all(scratch.join("6_55_25")).unwrap();
        std::fs::create_dir_all(scratch.join("6_55_250")).unwrap();
        std::fs::write(scratch.join("6_55_25.tif"), "x").unwrap();
        std::fs::write(scratch.join("x-6_55_25"), "x").unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let job = JobSpec {
            id: "terrain-hi 6/55/25".into(),
            what: "terrain-hi 6/55/25".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into(), "sh".into(), "--scratch".into(), s(&scratch)],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "terrain-hi".into(), targets: vec![("6/55/25".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        assert!(a.try_start(
            0,
            job.clone(),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let pgid = a.slots[0].running.as_ref().unwrap().pgid;
        let _kill = Kill(pgid);
        // Its lease at the lead's coordinator (this Mac's own, a shared step's), taken as it started.
        assert!(matches!(a.slots[0].lease, Some(Held::Own(_))) && co.held("terrain-hi").contains("6/55/25"));
        // A newer app installed: the update waits on the long job (task #266).
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink(new, apps.join("current")).unwrap();
        a.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
        assert!(!a.update_due(), "the update waits for the job");
        // A cancel of something not running: said, nothing stopped.
        let miss = crate::control::Cancel::new("6/1/1", None, true, "scenic job cancel on m4");
        crate::control::request_cancel(&home, &miss).unwrap();
        assert!(!a.tend_cancels(Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        let ans = crate::control::cancel_answer(&home, &miss).unwrap();
        assert!(!ans.hit && ans.said.contains("nothing builds 6/1/1"), "{}", ans.said);
        // The owner's cancel, to rerun after the update.
        let ask = crate::control::Cancel::new("6/55/25", None, true, "scenic job cancel on m4");
        crate::control::request_cancel(&home, &ask).unwrap();
        let mut waiting = Vec::new();
        assert!(a.tend_cancels(Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut waiting));
        let ans = crate::control::cancel_answer(&home, &ask).unwrap();
        assert!(ans.hit && ans.said.contains("terrain-hi 6/55/25 on this Mac"), "{}", ans.said);
        assert!(crate::control::peek_cancels(&home).is_empty());
        // Stopped, its process group gone; given back cleanly: no retry delay, recorded as cancelled,
        // no failure in the history; its lease ended.
        assert!(a.slots[0].running.is_none() && a.slots[0].lease.is_none());
        assert_eq!(unsafe { libc::kill(-pgid, 0) }, -1, "its processes stopped");
        assert!(a.mem.retry.is_empty());
        let r = &a.mem.recent[0];
        assert!(r.cancelled && !r.ok && r.note.contains("scenic job cancel on m4"), "{:?}", r.note);
        let events = co.shared.lock().unwrap().history.since(0, 1000);
        assert!(events.iter().any(|e| e.kind == "cancel" && e.targets == ["6/55/25"]));
        assert!(!events.iter().any(|e| e.kind == "fail" || (e.kind == "end" && e.ok == Some(false))));
        // Its work under way dropped: the target's scratch, not another's.
        assert!(!scratch.join("6_55_25").exists() && !scratch.join("6_55_25.tif").exists());
        assert!(scratch.join("6_55_250").exists() && scratch.join("x-6_55_25").exists());
        // The update restarts at once.
        assert!(a.update_due());
        // Held on this app: it doesn't start here, said in the status.
        assert!(waiting.iter().any(|w| w.what == "terrain-hi 6/55/25" && w.why.contains(old)), "{waiting:?}");
        let mut w = Vec::new();
        assert!(!a.try_start(0, job.clone(), &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut w));
        assert!(w.iter().any(|w| w.why.contains("held until the lead runs an app newer")), "{w:?}");
        // The agent on the new app: its coordinator's app releases the hold, and the job reruns.
        co.stop();
        a.coord = None;
        drop(co);
        a.coord = Some(std::sync::Arc::new(crate::coord::start_for_test(&coord, &a.me, new).0));
        assert!(a.held_why(&job).is_none());
        let job = JobSpec { cmd: vec!["/bin/sh".into(), "-c".into(), "exit 0".into()], ..job };
        assert!(a.try_start(0, job, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        if let Some(r) = a.slots[0].running.as_mut() {
            r.stop(Duration::from_secs(1));
        }
    }

    #[test]
    fn an_update_waits_for_a_long_target_by_its_measured_time() {
        // (Task #266: a z3 tile's elevation step 81 min in was stopped 15 min into an update's drain,
        // the z3 tile lost. The drain waits for the target under way by its measured time; the clock
        // here moved by setting when the drain began and when the progress last moved.)
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let apps = d.path().join("app");
        std::fs::create_dir_all(apps.join("v1")).unwrap();
        std::fs::create_dir_all(apps.join("v2")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: apps.join("v1"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str, step: &str, t: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: step.into(), targets: vec![(t.into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let ago = |s: u64| Instant::now().checked_sub(Duration::from_secs(s)).unwrap();
        let update = |a: &mut Agent, to: &str| {
            std::fs::remove_file(apps.join("current")).unwrap();
            std::os::unix::fs::symlink(to, apps.join("current")).unwrap();
            a.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
        };
        assert!(a.try_start(
            0,
            job("elev 6/55/25", "elev", "6/55/25"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let _kill = Kill(a.slots[0].running.as_ref().unwrap().pgid);
        // Its progress's pace says an hour left: the update waits 90 min (×1.5).
        a.slots[0].job_eta = Some(3600);
        a.slots[0].running.as_mut().unwrap().moved = Some((0.5, now_s()));
        update(&mut a, "v2");
        let (wait, words) = a.slots[0].update_wait.clone().unwrap();
        assert_eq!(wait, Some(Duration::from_secs(5400)), "{words}");
        assert!(words.contains("60 min more by its progress's pace"), "{words}");
        // 20 min, then 80 in, still progressing: no restart (the old rule restarted at 15).
        a.slots[0].drain_since = Some(ago(20 * 60));
        assert!(!a.update_due());
        a.slots[0].drain_since = Some(ago(80 * 60));
        assert!(!a.update_due());
        // No progress line for an hour but its processes busy (as elev, which prints none): it
        // holds the update until its wait runs out.
        a.slots[0].running.as_mut().unwrap().moved = Some((0.5, now_s() - 60 * 60));
        a.sampler.set_test_busy(Some(vec![Some(Instant::now()), None]));
        assert!(!a.update_due());
        a.slots[0].drain_since = Some(ago(91 * 60));
        assert!(a.update_due());
        // Idle (less than a core-minute) for 31 min, 20 min into the wait: it may be stuck, and holds
        // the update no longer; idle for 20 min only, it still does.
        a.slots[0].drain_since = Some(ago(20 * 60));
        a.sampler.set_test_busy(Some(vec![Some(ago(20 * 60)), None]));
        assert!(!a.update_due());
        a.sampler.set_test_busy(Some(vec![Some(ago(31 * 60)), None]));
        assert!(a.update_due());
        // (Idle for 31 min but only 10 min into the drain: the time a pause gives first.)
        a.slots[0].drain_since = Some(ago(10 * 60));
        assert!(!a.update_due());
        a.sampler.set_test_busy(None);
        // The install undone: the wait forgotten; a new update works it out again, from the
        // target's expected time (no pace yet), capped at 3 h.
        update(&mut a, "v1");
        assert!(a.slots[0].update_wait.is_none() && !a.update_due());
        a.slots[0].job_eta = None;
        a.mem.step_secs.insert(secs_key("elev"), 5.0 * 3600.0);
        update(&mut a, "v2");
        let (wait, words) = a.slots[0].update_wait.clone().unwrap();
        assert_eq!(wait, Some(UPDATE_CAP), "{words}");
        assert!(words.contains("by its expected time"), "{words}");
        // Its step's runs, its own not measured: their 90th percentile (a long tail's long jobs),
        // not the mean the jobs are sized by: 17 runs of 100 s and 3 of 2 h wait 2 h × 1.5, capped.
        update(&mut a, "v1");
        a.mem.step_secs.remove(&secs_key("elev"));
        let co = crate::coord::start_for_test(&d.path().join("coord"), "m4", "").0;
        let runs: Vec<(String, crate::coord::Cost)> = (0..20u64)
            .map(|i| {
                let c = crate::coord::Cost {
                    peak_mb: 100,
                    secs: if i < 17 { 100 } else { 7200 },
                    worker: None,
                    v: 0,
                    sizes: Default::default(),
                };
                (crate::coord::cost_key("elev", &format!("6/1/{i}")), c)
            })
            .collect();
        co.add_costs(&runs);
        a.coord = Some(std::sync::Arc::new(co));
        update(&mut a, "v2");
        let (wait, words) = a.slots[0].update_wait.clone().unwrap();
        assert!(
            wait.is_some_and(|w| w >= Duration::from_secs(179 * 60)),
            "the 90th percentile, 2 h × 1.5 capped, not the mean's 19 min: {wait:?} {words}"
        );
        a.coord = None;
        // A batch with a target done just now: the next timed from then, not from the job's start
        // (its whole run counted against it would leave it nothing).
        update(&mut a, "v1");
        a.mem.step_secs.insert(secs_key("elev"), 3600.0);
        a.slots[0].running.as_mut().unwrap().spec.record.as_mut().unwrap().targets.push(("6/55/26".into(), "k".into()));
        a.slots[0].running.as_mut().unwrap().started = now_s() - 2 * 3600;
        std::fs::write(a.done_path(0), "elev 6/55/25\n").unwrap();
        assert_eq!(crate::control::read_done(&a.done_path(0), "elev"), vec!["6/55/25".to_string()]);
        update(&mut a, "v2");
        let (wait, words) = a.slots[0].update_wait.clone().unwrap();
        assert!(wait.is_some_and(|w| w > Duration::from_secs(80 * 60)), "{wait:?} {words}");
        std::fs::remove_file(a.done_path(0)).unwrap();
        a.slots[0].running.as_mut().unwrap().started = now_s();
        // A short target: never less than the time a pause gives.
        update(&mut a, "v1");
        a.mem.step_secs.insert(secs_key("elev"), 60.0);
        update(&mut a, "v2");
        let (wait, words) = a.slots[0].update_wait.clone().unwrap();
        assert_eq!(wait, Some(DRAIN_GRACE), "{words}");
        a.slots[0].drain_since = Some(ago(16 * 60));
        assert!(a.update_due());
        // No job: due at once.
        a.slots[0].running.as_mut().unwrap().stop(Duration::from_secs(5));
        a.slots[0].running = None;
        assert!(a.update_due());
    }

    #[test]
    fn an_update_keeps_the_pause_grace_for_a_target_with_no_estimate() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let apps = d.path().join("app");
        std::fs::create_dir_all(apps.join("v1")).unwrap();
        std::fs::create_dir_all(apps.join("v2")).unwrap();
        std::os::unix::fs::symlink("v1", apps.join("current")).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: apps.join("v1"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.free_set = Some(40 << 30);
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        let spec = JobSpec {
            id: "trees 6/1/1".into(),
            what: "x".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "trees".into(), targets: vec![("6/1/1".into(), "k".into())] }),
        };
        assert!(a.try_start(0, spec, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), &mut Vec::new()));
        let pgid = a.slots[0].running.as_ref().unwrap().pgid;
        std::fs::remove_file(apps.join("current")).unwrap();
        std::os::unix::fs::symlink("v2", apps.join("current")).unwrap();
        a.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
        let (wait, words) = a.slots[0].update_wait.clone().unwrap();
        assert_eq!(wait, None);
        assert!(words.contains("no estimate"), "{words}");
        a.slots[0].drain_since = Some(Instant::now().checked_sub(Duration::from_secs(14 * 60)).unwrap());
        assert!(!a.update_due());
        a.slots[0].drain_since = Some(Instant::now().checked_sub(DRAIN_GRACE).unwrap());
        assert!(a.update_due());
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }

    #[test]
    fn osm_pass_waits_for_space_or_runs() {
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(root.join("sources/osm/2026-09-28")).unwrap();
        std::fs::write(root.join("sources/osm/2026-09-28/planet.osm.pbf"), b"x").unwrap();
        std::fs::create_dir_all(root.join("sources/basemap")).unwrap();
        std::fs::write(root.join("sources/basemap/planetiler.jar"), b"x").unwrap();
        let mut a = agent(&store::nasfs::NasRoot::local_for_test(&root), &home);
        // (The disk as the test says, both ways: not this Mac's.)
        a.free_set = Some(PASS_SPACE);
        let mut w = Vec::new();
        let plan = a.planner().plan(
            &store::nasfs::NasRoot::local_for_test(&root),
            &Conditions { ac: true, nas: true, idle_s: 0, ..Default::default() },
            &mut w,
        );
        assert!(plan.iter().any(|j| j.id == "osm-pass 2026-09-28"));
        a.free_set = Some(PASS_SPACE / 4);
        let mut w = Vec::new();
        let plan = a.planner().plan(
            &store::nasfs::NasRoot::local_for_test(&root),
            &Conditions { ac: true, nas: true, idle_s: 0, ..Default::default() },
            &mut w,
        );
        assert!(
            !plan.iter().any(|j| j.id == "osm-pass 2026-09-28") && w.iter().any(|x| x.why.contains("GB free")),
            "{w:?}"
        );
        // With the owner's room target, the pass keeps it free too: PASS_SPACE alone waits; told
        // the target, as every job is (room::TARGET_ENV).
        a.free_set = Some(PASS_SPACE);
        a.room_target = Some(room::Target { bytes: 20 << 30, by: "a test".into(), at: 0 });
        let mut w = Vec::new();
        let plan = a.planner().plan(
            &store::nasfs::NasRoot::local_for_test(&root),
            &Conditions { ac: true, nas: true, idle_s: 0, ..Default::default() },
            &mut w,
        );
        assert!(!plan.iter().any(|j| j.id == "osm-pass 2026-09-28"), "{w:?}");
        assert!(w.iter().any(|x| x.why.contains("needs 100 GB free") && x.why.contains("disk room target")), "{w:?}");
        a.free_set = Some(PASS_SPACE + (20 << 30));
        let mut w = Vec::new();
        let plan = a.planner().plan(
            &store::nasfs::NasRoot::local_for_test(&root),
            &Conditions { ac: true, nas: true, idle_s: 0, ..Default::default() },
            &mut w,
        );
        let pass = plan.iter().find(|j| j.id == "osm-pass 2026-09-28").expect("the pass past the target");
        assert_eq!(a.target_env(pass), (room::TARGET_ENV.to_string(), "20480".to_string()));
        let backup = JobSpec { id: "backup daily".into(), ..pass.clone() };
        assert_eq!(a.target_env(&backup).1, "0", "the backup keeps none");
        a.room_target = None;
        assert_eq!(a.target_env(pass).1, "0");
        // A complete pass of that planet: nothing to do.
        std::fs::write(root.join("sources/osm/2026-09-28/pass.0123456789abcdef.json"), b"{}").unwrap();
        let mut w = Vec::new();
        assert!(!a
            .planner()
            .plan(&store::nasfs::NasRoot::local_for_test(&root), &Conditions::default(), &mut w)
            .iter()
            .any(|j| j.id.starts_with("osm-pass")));
    }

    /// A "none" terrain or slope z6 tile (the coverage gone from its z6 tile) is never expected the
    /// same, its z6 tile's earlier record or its own (a flap back) whatever: its job drops files.
    #[test]
    fn none_pieces_are_never_expected_the_same() {
        let mut done = build::Keys::default();
        done.terrain_hi.insert("6/40/20".into(), "piece".into());
        done.slope_hi.insert("6/40/20".into(), "piece".into());
        done.terrain_hi.insert("6/41/20".into(), "earlier".into());
        for step in ["terrain-hi", "slope-hi"] {
            let w = build::Work {
                step: step.into(),
                targets: vec![
                    ("6/40/20".into(), build::none_piece_key(step, "6/40/20")),
                    ("6/41/20".into(), build::none_piece_key(step, "6/41/20")),
                ],
            };
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
        let w = |step: &str, ts: &[(&str, &str)]| build::Work {
            step: step.into(),
            targets: ts.iter().map(|(t, k)| (t.to_string(), k.to_string())).collect(),
        };
        // One by one in a batch (a helper takes the far end of an offer, so a batch can mix them): a
        // z6 tile the records have under the key it's made with (its mid made), not one under another
        // key (stale) nor one never made.
        assert_eq!(
            expect_same(&w("trees", &[("6/28/16", "a"), ("6/28/17", "b2"), ("6/29/16", "e")]), &done),
            ["6/28/16"]
        );
        // A z3 tile's whole run, an assembly, another step: never.
        assert!(expect_same(&w("trees", &[("3/3/2", "c")]), &done).is_empty());
        assert!(expect_same(&w("trees-lo", &[("3/3/2", "d")]), &done).is_empty());
        assert!(expect_same(&w("unit", &[("6/28/16", "a")]), &done).is_empty());
        // The times kept here go by the way a step runs now: a z3 tile's tree cover isn't a z6 tile's.
        assert_eq!([secs_key("trees"), secs_key("trees-lo"), secs_key("unit")], ["trees v3", "trees-lo v1", "unit v3"]);
    }

    #[test]
    fn feed_keys_missing_said() {
        let m = |keys: &str, used: &str| -> BTreeMap<String, String> {
            [("keys".to_string(), keys.to_string()), ("feed-keys".to_string(), used.to_string())].into()
        };
        assert_eq!(missing_feed_keys(&m("LTA_ACCOUNT_KEY", "LTA_ACCOUNT_KEY")), None);
        assert_eq!(missing_feed_keys(&m("", "")), None);
        assert!(missing_feed_keys(&m("ODPT_KEY", "LTA_ACCOUNT_KEY")).is_some_and(|w| w.contains("no LTA_ACCOUNT_KEY:")));
    }

    #[test]
    fn key_names_only() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("keys.env");
        assert_eq!(key_names(&store::nasfs::NasPath::local_for_test(&p)).as_deref(), Some(""), "no file: no keys");
        std::fs::write(&p, "LTA_ACCOUNT_KEY=s3cr3t\nODPT_KEY=\n# OLD_KEY=x\n TDX_CLIENT_ID = abc \nnot a line\n")
            .unwrap();
        let names = key_names(&store::nasfs::NasPath::local_for_test(&p)).unwrap();
        assert_eq!(names, "LTA_ACCOUNT_KEY,TDX_CLIENT_ID", "the keys with values, by name");
        assert!(!names.contains("s3cr3t") && !names.contains("abc"));
    }

    /// While the pass's lake fill is being made, the jobs that read it wait, said in the checklist;
    /// once it's made, or after its failures, they're queued.
    #[test]
    fn the_jobs_reading_the_lake_fill_wait_for_it() {
        let spec = |id: &str| JobSpec {
            id: id.into(),
            what: format!("doing {id}"),
            cmd: Vec::new(),
            needs: Needs { nas: true },
            restart_after_sleep: true,
            record: None,
        };
        let jobs = || {
            [
                "lakefill 2026-09-28",
                "labels 2026-09-28",
                "water 2026-09-28",
                "terrain-water terrain-water",
                "terrain-hi 6/18/20",
                "catalog catalog",
                "catalog-held catalog-held",
                "unit 6/1/2",
            ]
            .map(spec)
            .to_vec()
        };
        let ids = |v: &[JobSpec]| v.iter().map(|j| j.id.clone()).collect::<Vec<_>>();
        let mut waiting = Vec::new();
        let kept = hold_for_lakefill(jobs(), true, &mut waiting);
        assert_eq!(ids(&kept), ["lakefill 2026-09-28", "labels 2026-09-28", "terrain-hi 6/18/20", "unit 6/1/2"]);
        assert_eq!(
            waiting.iter().map(|w| w.step.clone().unwrap()).collect::<Vec<_>>(),
            ["water", "terrain-water", "catalog", "catalog-held"]
        );
        assert!(waiting.iter().all(|w| w.why.contains("lakefill")));
        // Not held (the fill made, or failed LAKEFILL_HOLDS times): all queued, nothing said.
        let mut waiting = Vec::new();
        assert_eq!(ids(&hold_for_lakefill(jobs(), false, &mut waiting)), ids(&jobs()));
        assert!(waiting.is_empty());
        // What holds: the lakefill job's work, until its failures reach LAKEFILL_HOLDS.
        assert!(
            build::water_waits_for_lakefill(true, build::LAKEFILL_HOLDS - 1)
                && !build::water_waits_for_lakefill(true, build::LAKEFILL_HOLDS)
        );
    }

    /// Before the source declarations' first accepted version, the jobs that read them wait, said in
    /// the checklist; the rest go on.
    #[test]
    fn the_jobs_reading_the_declared_sources_wait_for_them() {
        let spec = |id: &str| JobSpec {
            id: id.into(),
            what: format!("doing {id}"),
            cmd: Vec::new(),
            needs: Needs { nas: true },
            restart_after_sleep: true,
            record: None,
        };
        let jobs = || {
            [
                "labels 2026-09-28",
                "unit 6/1/2",
                "terrain-hi 6/18/20",
                "trees 6/3/4",
                "items 2026-09-28",
                "heritage 2026-09-28",
                "heritage-sites 2026-09-28",
            ]
            .map(spec)
            .to_vec()
        };
        let ids = |v: &[JobSpec]| v.iter().map(|j| j.id.clone()).collect::<Vec<_>>();
        let mut waiting = Vec::new();
        assert_eq!(
            ids(&hold_for_sources(jobs(), true, &mut waiting)),
            ["labels 2026-09-28", "terrain-hi 6/18/20", "heritage-sites 2026-09-28"]
        );
        assert_eq!(
            waiting.iter().map(|w| w.step.clone().unwrap()).collect::<Vec<_>>(),
            ["unit", "trees", "items", "heritage"]
        );
        assert!(waiting.iter().all(|w| w.why.contains("inputs/sources/")));
        let mut waiting = Vec::new();
        assert_eq!(ids(&hold_for_sources(jobs(), false, &mut waiting)), ids(&jobs()));
        assert!(waiting.is_empty());
    }

    /// Task #506: a step whose job runs again (its key changed: new regions, say) isn't done though
    /// its records, read earlier, say so; the stage's count goes with it.
    #[test]
    fn a_step_running_again_is_not_done() {
        let item = |step: &str, done: usize, total: Option<usize>, left: usize| build::Item {
            step: step.into(),
            done,
            total,
            left: Some(left),
            ..Default::default()
        };
        let mut l = vec![build::Line {
            what: "landmarks".into(),
            steps: ["peaks", "heritage", "marks"].iter().map(|s| s.to_string()).collect(),
            done: 4,
            total: Some(4),
            left: Some(0),
            unit: "z6 tiles".into(),
            items: vec![item("peaks", 4, Some(4), 0), item("heritage", 0, None, 0), item("marks", 0, None, 0)],
            ..Default::default()
        }];
        assert!(l[0].finished());
        annotate(&mut l, &["heritage".to_string()], &[], &[]);
        assert!(!l[0].finished(), "its step runs");
        assert_eq!(l[0].left, Some(1));
        assert_eq!(l[0].items[1].left, Some(1), "heritage: 1 run left, not done");
        assert!(l[0].items[2].finished());
        // A sized step's: one target under way, the stage's count one fewer. And a helper's job
        // counts as this Mac's does.
        let mut helper = Status { host: "m1".into(), ..Default::default() };
        helper.jobs = vec![JobView {
            id: "peaks 6/1/2".into(),
            what: String::new(),
            started: 0,
            pausing: None,
            paused: None,
            tail: String::new(),
            progress: None,
            parts: Vec::new(),
            part: None,
            mem_mb: None,
            threads: None,
            waiting: None,
            slot: Some(0),
            live: None,
        }];
        annotate(&mut l, &[], &[helper], &[]);
        assert_eq!((l[0].items[0].done, l[0].items[0].left), (3, Some(1)));
        assert_eq!((l[0].done, l[0].left), (3, Some(2)));
    }

    /// Task #506: a job taken over from the agent before this one is paced from its real start,
    /// not from the take-over (where its next tick of progress made "about 346 min left" of 81%);
    /// a fresh job from its progress first seen.
    #[test]
    fn a_taken_over_jobs_pace_runs_from_its_start() {
        // (A Mac up less than 20 min has no such Instant: nothing to check there.)
        let Some(started) = Instant::now().checked_sub(Duration::from_secs(20 * 60)) else { return };
        let (t0, f0, _, _) = pace_base(true, started, 0.81, "steps".into(), None);
        assert_eq!((t0, f0), (started, 0.0));
        let eta = t0.elapsed().as_secs_f64() * (1.0 - 0.82) / (0.82 - f0);
        assert!(eta < 6.0 * 60.0, "about 4–5 min left, not hours: {eta}");
        // A fresh job (its setup not folded in), or a later part: from now.
        for (taken_over, part) in [(false, None), (true, Some(2))] {
            let (t0, f0, _, _) = pace_base(taken_over, started, 0.5, "steps".into(), part);
            assert!(t0 > started && f0 == 0.5);
        }
    }

    #[test]
    fn the_checklist_says_why_a_step_waits() {
        let step = |what: &str, steps: &[&str], left: usize| build::Line {
            id: what.to_lowercase(),
            what: what.into(),
            steps: steps.iter().map(|s| s.to_string()).collect(),
            left: Some(left),
            ..Default::default()
        };
        let list = || {
            vec![
                step("Pass", &["pass-sets", "reach"], 1),
                step("Roads", &["unit"], 3),
                step("Landmarks", &["pois", "peaks", "items", "heritage", "marks", "overlays"], 2),
                step("Publishing", &["catalog", "catalog-held"], 0),
            ]
        };
        let helper = Status {
            host: "m1".into(),
            job: Some(JobView {
                id: "unit 6/1/2".into(),
                what: String::new(),
                started: 0,
                pausing: None,
                paused: None,
                tail: String::new(),
                progress: None,
                parts: Vec::new(),
                part: None,
                mem_mb: None,
                threads: None,
                waiting: None,
                slot: Some(0),
                live: None,
            }),
            ..Default::default()
        };
        let waiting = [Waiting { step: Some("reach".into()), what: "How far…".into(), why: "away from home".into() }];
        let mut l = list();
        annotate(&mut l, &["heritage".to_string()], &[helper], &waiting);
        assert_eq!(l[0].note.as_deref(), Some("away from home"));
        assert_eq!(l[1].note.as_deref(), Some("on m1"));
        assert_eq!(l[2].note, None, "this Mac's job now");
        // Publishing waits for the steps above, though its catalogue is current.
        assert!(!l[3].finished() && l[3].note.as_deref().is_some_and(|n| n.starts_with("as each unit is built")));
        // All above done: done.
        let mut l: Vec<build::Line> = list()
            .into_iter()
            .map(|mut s| {
                s.left = Some(0);
                s
            })
            .collect();
        annotate(&mut l, &[], &[], &[]);
        assert!(l.iter().all(|s| s.finished() && s.note.is_none()));
        // A line not sized yet (trains a day before its sources) holds publishing up no more than a
        // done one; and publishing's own note (its catalogue failing) is kept.
        l[1].left = None;
        annotate(&mut l, &[], &[], &[]);
        assert!(l[3].finished() && l[3].note.is_none());
        let mut l = list();
        l[3].left = Some(1);
        let failing =
            [Waiting { step: Some("catalog".into()), what: String::new(), why: "failed 3 times in a row".into() }];
        annotate(&mut l, &[], &[], &failing);
        assert_eq!(l[3].note.as_deref(), Some("failed 3 times in a row"));
    }

    #[test]
    fn conditions_gate_jobs() {
        let n = Needs { nas: true };
        let at =
            |ac: bool, nas: bool, home: bool, battery: Option<u8>| Conditions { ac, nas, home, idle_s: 0, battery };
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
        let (drain, now) =
            (Pause::new(Mode::Drain, "the menu bar on m4"), Pause::new(Mode::Freeze, "scenic pause on m4"));
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
        std::fs::write(r.join("state/build/manifest.json"), r#"{"base/6-1-1": "base/6-1-1.1111111111111111.base"}"#)
            .unwrap();
        std::fs::write(r.join("state/build/jobs.json"), r#"{"unit": {"6/1/1": "k1"}}"#).unwrap();
        std::fs::write(r.join("state/build/pending.json"), "{}").unwrap();
        r
    }

    /// The owner's edit of the lead's declaration (crate::pool::term::LEAD): member `to` leads.
    fn move_lead(r: &Path, to: &str) {
        std::fs::write(r.join(crate::pool::term::LEAD), format!("{{\"member\": \"{to}\"}}")).unwrap();
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
            "#!/bin/sh\nstep=\"$1\"\nenv > \"{bin}/env-$step\"\nif [ -n \"$SCENIC_HANDOFF\" ]; then\n  mkdir -p \"$SCENIC_HANDOFF\"\n  case \"$step\" in\n    slope-hi) printf '{{\"changes\":{{\"layers/slope/lo/3-2-2\":\"layers/slope/lo/3-2-2.0123456789abcdef.pack\"}}}}' > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n    pack) printf '{{\"changes\":{{\"layers/roads/hi/6-1-1\":\"layers/roads/hi/6-1-1.0123456789abcdef.pack\"}}}}' > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n    *) printf '{{\"changes\":{{}},\"pending\":{{\"work/%s.0123456789abcdef.json\":\"aa\"}}}}' \"$step\" > \"$SCENIC_HANDOFF/00000000000000000001-1.json\" ;;\n  esac\nfi\nexit 0\n",
            bin = bin.display()
        );
        for p in ["scenic", "scenic-build"] {
            std::fs::write(bin.join(p), &script).unwrap();
            std::fs::set_permissions(bin.join(p), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        bin
    }

    /// Makes the stand-in `scenic-build` wait at `step` until `gate` exists. It also ends when its
    /// app folder (the test's temp folder) is gone or its parent is: a test that panics or is killed
    /// leaves no waiting shell behind (#654: one ran 16.5 h).
    fn gate_step(bin: &Path, step: &str, gate: &Path) {
        let script = std::fs::read_to_string(bin.join("scenic-build")).unwrap().replace(
            "step=\"$1\"\n",
            &format!(
                "step=\"$1\"\n[ \"$step\" = {step} ] && while [ ! -e '{}' ]; do [ -d '{}' ] && kill -0 $PPID 2>/dev/null || exit 1; sleep 0.05; done\n",
                gate.display(),
                bin.display()
            ),
        );
        std::fs::write(bin.join("scenic-build"), script).unwrap();
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
            a.round().unwrap();
            if done(a) {
                return;
            }
            assert!(
                Instant::now() < end,
                "not done in {WATCHDOG:?}: {:?}",
                read_status(None, &a.o.home).map(|s| s.waiting)
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    use store::testkit::WATCHDOG;

    /// A member's Mac with more room than its lead's test Mac (`cond::Mac::TEST`'s 16 GB), as the
    /// M4's 48 GB: a cold start (no job of its step measured) goes to it, whatever Mac runs the test.
    const MEMBER_MAC: cond::Mac =
        cond::Mac::Fixed { ac: true, battery: None, idle_s: 0, home: true, mem_gb: 48.0, mem_free_pct: 75 };

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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("app-folder/agent"),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        assert!(a.pool.is_some(), "this Mac's member in the pool, with no switch on the NAS");
        a.round().unwrap();
        assert!(r.join(crate::pool::term::DIR).join("1.json").exists(), "term 1 made");
        assert!(read_status(Some(&store::nasfs::NasRoot::local_for_test(&r)), &a.o.home).unwrap().pool.is_some());
        assert!(a.pool_restart().is_none());
    }

    #[test]
    fn a_tick_that_never_returns_stops_the_leads_grants_and_says_so() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("app-folder/agent"),
            bin,
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.mem.last_ok.insert("backup".into(), now_s());
        a.mem.last_ok.insert("gc".into(), now_s());
        a.round().unwrap();
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
        a.round().unwrap();
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
        a.round().unwrap();
        assert_eq!(a.pool_skips, 1, "passed over, not waited on");
        tx.send(()).unwrap();
        t.join().unwrap();
        a.round().unwrap();
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
        a.round().unwrap();
        assert!(a.pool_skips == 1 && a.pool_hung.is_none() && a.published.is_some(), "a minute: under way");
        ticking.start_at(pool::Ticking::awake() - crate::pool::driver::STALL_S - 1);
        a.round().unwrap();
        assert_eq!(moving(&a).as_deref(), Some(HUNG));
        assert!(a.published.is_none() && a.pool_hung.is_some(), "its contact off the NAS");
        let member = |a: &Agent| a.coord.as_ref().unwrap().shared.lock().unwrap().pool_member.clone();
        assert_eq!(member(&a), None, "its answers say no term: not late, not gone");
        a.round().unwrap();
        assert!(a.published.is_none(), "kept off while it hangs");
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&r)), &a.o.home).unwrap();
        assert!(
            st.waiting.iter().any(|w| w.why.contains("has hung") && w.why.contains("the NAS isn't answering")),
            "{:?}",
            st.waiting
        );
        // It returns: cleared, granting and published again.
        ticking.end();
        tx.send(()).unwrap();
        t.join().unwrap();
        a.round().unwrap();
        assert!(a.pool_hung.is_none() && a.pool_skips == 0 && a.published.is_some());
        assert!(member(&a).is_some(), "its term said again");
        assert_ne!(moving(&a).as_deref(), Some(HUNG));
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&r)), &a.o.home).unwrap();
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
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: home.clone(),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
        let (role, leads, me) =
            a.pooled().map(|run| (run.role, run.gates.leads, run.side.member().id.clone())).expect("the pool's part");
        assert_eq!((role, leads), (pool::Role::Lead, Some(1)));
        assert!(a.leads() && a.coord.is_some(), "it leads: its coordinator runs");
        // (Its writer's file changed after term 1: no one names the writer any more.)
        std::fs::write(r.join("state/build/writer"), "another-mac").unwrap();
        // Its member's lock is in the app's folder; its saved state in its own.
        assert!(d.path().join(format!("app-folder/pool-{me}.lock")).exists() && home.join("pool/saved.json").exists());
        // Its first job (the daily backup) runs under a lease of its term, its saves handed off; the
        // records it merges are today's files' too (term 1's saves write them).
        until(&mut a, |a| {
            crate::agent::build::Keys::load(a.o.root.as_ref().unwrap()).lo.is_empty()
                && a.pooled().is_some_and(|p| {
                    p.side
                        .driver()
                        .records()
                        .is_some_and(|r| r.pending.contains_key("work/backup.0123456789abcdef.json"))
                })
        });
        let env = std::fs::read_to_string(bin.join("env-backup")).unwrap();
        assert!(env.contains("SCENIC_HANDOFF="), "{env}");
        assert!(env.contains(&format!("{}", home.join("pool/jobs/1-").display())), "{env}");
        let rec = a.pooled().unwrap().side.driver().records().unwrap().clone();
        assert!(rec
            .reflected
            .iter()
            .any(|k| k.ends_with(&format!("/1-{}", rec.reflected.iter().next().unwrap().rsplit('-').next().unwrap()))));
        // (Its own, lease 1's: other jobs may run beside it, cold starts too since task #499.)
        assert!(
            std::fs::read_dir(home.join("pool/jobs"))
                .map_or(true, |d| !d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("1-"))),
            "its folder removed once its saved state held the entry"
        );
        // The writer isn't named any more (its file as it was), the coordinator's state of its term
        // and its history are on the NAS, its heartbeat names it.
        assert_eq!(std::fs::read_to_string(r.join("state/build/writer")).unwrap(), "another-mac");
        // (The coordinator's store's backup, by its own thread each `store::BACKUP_EVERY` (a test's
        // shorter), once the lead's step has started it.)
        until(&mut a, |a| {
            a.pooled().and_then(|p| p.gates.leads).is_some() && r.join(crate::pool::store::BACKUP_PATH).exists()
        });
        assert!(std::fs::read_dir(r.join("state/coord/history"))
            .unwrap()
            .flatten()
            .any(|day| day.path().join(format!("{me}.jsonl")).exists()));
        assert!(r.join(crate::pool::beat::path(&me)).exists());
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&r)), &home).unwrap();
        let pv = st.pool.expect("the pool in its status");
        assert_eq!((pv.role, pv.member.as_str()), (pool::Role::Lead, me.as_str()));
        // The daily sweep waits for a re-assertion, then runs on the step that made it, caught up:
        // term 2, whose records its readers read (today's files no more: its one member reads these).
        until(&mut a, |a| a.mem.last_ok.contains_key("gc"));
        assert!(a.pooled().unwrap().gates.term >= 2);
        assert!(r.join("state/build/terms/2.json").exists());
        let m = crate::out::records_now(&store::nasfs::NasRoot::local_for_test(&r)).unwrap().pending;
        assert!(m.contains_key("work/gc.0123456789abcdef.json"), "{m:?}");
        stop_jobs(&mut a);
    }

    /// Task #607: a raw archive handed off reaches the lead's records, is named in the raw store's
    /// index (pool_lead_files: one named already passed over unread, one not on the NAS passed over),
    /// then comes off the records, saved so on the NAS.
    #[test]
    fn the_leads_raw_archives_named_come_off_its_saved_records() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let store = r.join("sources/aws-terrarium");
        let named = crate::rawpack::Pack { name: "6-1-1.00000000000000aa.tiles".into(), bytes: 9 };
        let absent = crate::rawpack::Pack { name: "6-1-1.00000000000000bb.tiles".into(), bytes: 9 };
        let mut ix = crate::rawpack::Index::default();
        ix.shards.insert("6-1-1".into(), vec![named.clone()]);
        ix.save(&store::nasfs::NasPath::local_for_test(&store)).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("app-folder/agent"),
            bin,
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.mem.last_ok.insert("backup".into(), now_s());
        a.mem.last_ok.insert("gc".into(), now_s());
        let raw = serde_json::to_string(&vec![("6-1-1", &named), ("6-1-1", &absent)]).unwrap();
        let job = JobSpec {
            id: "backup".into(),
            what: "a hand-off of raw archives".into(),
            cmd: vec![
                "/bin/sh".into(),
                "-c".into(),
                format!(
                    "mkdir -p \"$SCENIC_HANDOFF\" && printf '%s' '{{\"changes\":{{}},\"pending\":{{\"work/backup.0123456789abcdef.json\":\"aa\"}},\"raw\":{raw}}}' > \"$SCENIC_HANDOFF/00000000000000000001-1.json\""
                ),
            ],
            needs: Needs { nas: true },
            restart_after_sleep: false,
            record: None,
        };
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 9999 };
        until(&mut a, |a| a.pooled().unwrap().gates.leads.is_some());
        assert!(a.try_start(0, job, &c, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()));
        until(&mut a, |a| {
            a.pooled()
                .unwrap()
                .side
                .driver()
                .records()
                .is_some_and(|r| r.pending.contains_key("work/backup.0123456789abcdef.json") && r.raw.is_empty())
        });
        assert!(a.pooled().unwrap().named_raw.is_empty());
        let term = a.pooled().unwrap().side.driver().records().unwrap().term;
        until(&mut a, |_| {
            let s = std::fs::read_to_string(r.join(crate::pool::records::path(term))).unwrap_or_default();
            s.contains("work/backup.0123456789abcdef.json") && !s.contains("00000000000000aa")
        });
        // (The index as it was: nothing read or named.)
        assert_eq!(crate::rawpack::Index::load(&store::nasfs::NasPath::local_for_test(&store)).unwrap(), ix);
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
        let dry = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap();
        assert!(dry.pool.is_none(), "a dry run takes no part");
        drop(dry);
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        m.mac = MEMBER_MAC;
        assert_eq!(m.pooled().unwrap().role, pool::Role::Member);
        assert!(m.is_member() && m.coord.is_none());
        let (il, im) = (lead.pooled().unwrap().side.member().id.clone(), m.pooled().unwrap().side.member().id.clone());
        assert_ne!(il, im);
        // The lead's coordinator offers slope; the member asks, and runs it under `1-<lease>`.
        lead.coord.as_ref().unwrap().offer(
            "2026-09-28",
            vec![crate::coord::Offer {
                step: "slope-hi".into(),
                targets: vec![("3/2/2".into(), "k".into(), 100)],
                job: None,
            }],
        );
        until(&mut m, |m| m.slots[0].running.is_some() || m.pooled().is_some_and(|p| !p.entries.is_empty()));
        let env = std::fs::read_to_string(bin.join("env-slope-hi")).unwrap_or_default();
        until(&mut m, |m| m.slots[0].running.is_none() && m.pooled().is_some_and(|p| p.entries.is_empty()));
        assert!(
            std::fs::read_to_string(bin.join("env-slope-hi"))
                .unwrap()
                .contains(&format!("{}", d.path().join("m/agent/pool/jobs/1-").display())),
            "{env}"
        );
        // Its entry in the journal, told to the lead by mail; the lead, knowing the member, merges it.
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        until(&mut lead, |l| {
            l.pooled()
                .unwrap()
                .side
                .driver()
                .records()
                .is_some_and(|r| r.keys.recorded("slope-hi", "3/2/2") == Some("k"))
        });
        let rec = lead.pooled().unwrap().side.driver().records().unwrap().clone();
        assert_eq!(
            rec.manifest.get("layers/slope/lo/3-2-2").map(String::as_str),
            Some("layers/slope/lo/3-2-2.0123456789abcdef.pack")
        );
        let key = rec.reflected.iter().find(|k| k.contains("/1-")).cloned().unwrap();
        assert!(r.join(crate::pool::journal::path(&key)).exists());
        // Acknowledged: the member tells it no more.
        until(&mut m, |m| m.pooled().unwrap().side.driver().mine().to_tell(1).is_empty());
        // The lead's coordinator ended the lease, its target kept out of offers until the plan shows
        // it built; its own jobs hold none of it meanwhile.
        assert!(lead.coord.as_ref().unwrap().held("slope-hi").is_empty());
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
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        // `pack` of 6/1/1 measured (a run of it said what it took), offered.
        let c = lead.coord.as_ref().unwrap();
        c.add_costs(&[(
            crate::coord::cost_key("pack", "6/1/1"),
            crate::coord::Cost {
                peak_mb: 100,
                secs: 60,
                worker: None,
                v: crate::coord::cost_version("pack"),
                ..Default::default()
            },
        )]);
        c.offer(
            "2026-09-28",
            vec![crate::coord::Offer {
                step: "pack".into(),
                targets: vec![("6/1/1".into(), "kp".into(), 100)],
                job: None,
            }],
        );
        let asked = |l: &Agent| {
            l.coord
                .as_ref()
                .unwrap()
                .workers()
                .into_iter()
                .find(|(_, w)| w.kind == "native")
                .map(|(_, w)| (w.can.clone(), w.ask.as_ref().and_then(|a| a.disk_mb)))
        };
        // The lead's coordinator told (with its own room), the member asking for every step.
        lead.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        m.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        until(&mut m, |m| m.slots[0].running.is_some() || bin.join("env-pack").exists());
        until(&mut m, |m| m.slots[0].running.is_none() && m.pooled().is_some_and(|p| p.entries.is_empty()));
        let (can, disk) = asked(&lead).unwrap();
        assert!(
            can.iter().any(|s| s == "pack")
                && can.iter().any(|s| s == "lo")
                && !can.iter().any(|s| s == "catalog")
                && disk.is_some(),
            "{can:?}"
        );
        // Its Mac's capacity carried by its ask (the test's Mac: 16 GB, on mains, in use, its cores
        // all free), and the lead's own set by its loop: both kept, the lead's first.
        let caps = lead.coord.as_ref().unwrap().capacities();
        assert_eq!(caps.len(), 2, "{caps:?}");
        let mc = &caps[1].1;
        // (Its limit less the test process's own footprint, measured each loop: #619.)
        let most = memguard::limit_mb(16 << 10);
        assert!(mc.limit_mb < most && mc.limit_mb + (4 << 10) > most, "{} of {most}", mc.limit_mb);
        assert!(
            mc.mem_mb <= mc.limit_mb
                && mc.cores_free == mc.cores
                && mc.ac
                && mc.in_use
                && mc.nas
                && mc.disk_mb > 0
                && mc.at > 0,
            "{mc:?}"
        );
        let env = std::fs::read_to_string(bin.join("env-pack")).unwrap();
        assert!(env.contains(&format!("{}", d.path().join("m/agent/pool/jobs/1-").display())), "{env}");
        // Its entry merged by the lead, within pack's write-set.
        until(&mut lead, |l| {
            l.pooled().unwrap().side.driver().records().is_some_and(|r| r.keys.recorded("pack", "6/1/1") == Some("kp"))
        });
        let rec = lead.pooled().unwrap().side.driver().records().unwrap().clone();
        assert_eq!(
            rec.manifest.get("layers/roads/hi/6-1-1").map(String::as_str),
            Some("layers/roads/hi/6-1-1.0123456789abcdef.pack")
        );
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
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        // Off: not told.
        lead.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        m.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(lead.coord.as_ref().unwrap().shared.lock().unwrap().placement.is_none());
        // On: told, the forecast's slack and speeds in it.
        switch_on(&r, placement::SWITCH);
        *lead.planner().forecast.borrow_mut() = Some(forecast::Forecast {
            done_at: Some(1000),
            steps: vec![forecast::StepFc { step: "slope-hi".into(), done_at: Some(1000), ..Default::default() }],
            ..Default::default()
        });
        lead.planner().publish();
        lead.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        m.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert_eq!(
            lead.coord.as_ref().unwrap().shared.lock().unwrap().placement.as_ref().map(|p| p.0.slack_of("slope-hi")),
            Some(0)
        );
        // `slope` of 3/2/2 measured, offered: the member alone asking, drawn to it.
        let c = lead.coord.as_ref().unwrap();
        c.add_costs(&[(
            crate::coord::cost_key("slope-hi", "3/2/2"),
            crate::coord::Cost {
                peak_mb: 100,
                secs: 60,
                worker: None,
                v: crate::coord::cost_version("slope-hi"),
                ..Default::default()
            },
        )]);
        c.offer(
            "2026-09-28",
            vec![crate::coord::Offer {
                step: "slope-hi".into(),
                targets: vec![("3/2/2".into(), "k".into(), 100)],
                job: None,
            }],
        );
        // (Read once its job has ended: the fake job's `env >` makes the file before it writes it,
        // so the file's being there says nothing of its lines. Only the member's job runs slope:
        // the lead isn't stepped after it's offered.)
        until(&mut m, |m| bin.join("env-slope-hi").exists() && m.slots[0].running.is_none());
        let env = std::fs::read_to_string(bin.join("env-slope-hi")).unwrap();
        assert!(env.lines().any(|l| l == format!("{}=1", placement::AHEAD_ENV)), "{env}");
        // Off again: as before.
        std::fs::write(r.join(placement::SWITCH), "off").unwrap();
        lead.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        m.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
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
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.free_set = Some(400 << 30);
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        // (The planet after the lead's first step: only the test's plans below see it.)
        std::fs::create_dir_all(r.join(format!("sources/osm/{date}"))).unwrap();
        std::fs::write(r.join(format!("sources/osm/{date}/planet.osm.pbf")), "planet").unwrap();
        std::fs::create_dir_all(r.join("sources/basemap")).unwrap();
        std::fs::write(r.join("sources/basemap/planetiler.jar"), "jar").unwrap();
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        let planned = |a: &mut Agent| {
            let mut w = Vec::new();
            let p = a.planner().plan(&store::nasfs::NasRoot::local_for_test(&r), &Conditions::default(), &mut w);
            (p.iter().any(|j| j.id == pass), w)
        };
        // The member's Mac holds the pass's progress (its filter done), noted an hour ago.
        let scratch = d.path().join(format!("m/agent/scratch/osm-{date}"));
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("filter.done"), "").unwrap();
        let held = |at: u64, done: bool| resume::Progress {
            member: im.clone(),
            worker: store::sys::host_name(),
            jobs: [(
                pass.clone(),
                resume::Held { at, done, scratch: scratch.to_string_lossy().into_owned(), since: 0 },
            )]
            .into(),
        };
        resume::write(&store::nasfs::NasRoot::local_for_test(&r), &held(now_s() - 3600, false)).unwrap();
        // Off: the lead plans it as before, whoever holds progress.
        lead.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        m.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(planned(&mut lead).0);
        assert!(lead.coord.as_ref().unwrap().shared.lock().unwrap().resume.is_none());
        // On: not here; offered to the member alone, which resumes it in its scratch folder.
        switch_on(&r, resume::SWITCH);
        lead.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        m.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        let (here, w) = planned(&mut lead);
        assert!(!here && w.iter().any(|w| w.why.contains("offered to it alone")), "{w:?}");
        // (Its earlier stage's entry, handed and not yet merged: laid over the snapshot for its job.)
        let earlier = crate::pool::journal::Entry {
            member: im.clone(),
            lease: LeaseId { term: 1, n: 99 },
            step: "osm-pass".into(),
            handoff: crate::handoff::Handoff {
                changes: [(
                    format!("sources/osm/{date}/filtered"),
                    Some(format!("sources/osm/{date}/filtered.2222222222222222.osm.pbf")),
                )]
                .into(),
                ..Default::default()
            },
            at: now_s(),
        };
        m.pooled().unwrap().entries.push((earlier, None));
        assert!(
            m.unmerged_for("osm-pass").unwrap().changes.contains_key(&format!("sources/osm/{date}/filtered"))
                && m.unmerged_for("unit").unwrap().changes.is_empty()
        );
        m.pooled().unwrap().entries.clear();
        until(&mut m, |m| {
            m.slots[0].running.as_ref().is_some_and(|j| j.spec.id == pass)
                || std::fs::read_to_string(bin.join("env-osm-pass")).is_ok()
        });
        until(&mut m, |m| m.slots[0].running.is_none());
        let env = std::fs::read_to_string(bin.join("env-osm-pass")).unwrap();
        assert!(
            env.contains("SCENIC_RESUME=1") && env.contains("SCENIC_UNMERGED=") && env.contains("SCENIC_HANDOFF="),
            "{env}"
        );
        assert!(env.contains(&format!("{}", d.path().join("m/agent/pool/jobs/1-").display())), "{env}");
        assert!(lead.coord.as_ref().unwrap().held("osm-pass").is_empty(), "its lease ended at the lead, not lapsing");
        // Finished there: held, unoffered, until merged.
        let p = resume::read(&store::nasfs::NasRoot::local_for_test(&r), &im).unwrap().unwrap();
        assert!(p.jobs[&pass].done, "{p:?}");
        let (here, w) = planned(&mut lead);
        assert!(!here && w.iter().any(|w| w.why.contains("waiting to be merged")), "{w:?}");
        // Released, or its day out, while the member runs it under a lease: not run here too.
        let lease = lead
            .coord
            .as_ref()
            .unwrap()
            .hold("osm-pass", &[(date.to_string(), format!("sources/osm/{date}/planet.osm.pbf"))])
            .unwrap();
        resume::write(&store::nasfs::NasRoot::local_for_test(&r), &held(now_s() - resume::HOLD_S - 1, false)).unwrap();
        let (here, w) = planned(&mut lead);
        assert!(!here && w.iter().any(|w| w.why.contains("under its lease")), "{w:?}");
        lead.coord.as_ref().unwrap().finish(lease, &[]);
        // Its day out or released, but noted lately and not finished: maybe running under a lease the
        // lead lost, so not run here too.
        let mut lately = held(now_s() - 60, false);
        lately.jobs.get_mut(&pass).unwrap().since = now_s() - resume::HOLD_S - 1;
        resume::write(&store::nasfs::NasRoot::local_for_test(&r), &lately).unwrap();
        let (here, w) = planned(&mut lead);
        assert!(!here && w.iter().any(|w| w.why.contains("may be running")), "{w:?}");
        // A day on with nothing more there: the lead's to run.
        resume::write(&store::nasfs::NasRoot::local_for_test(&r), &held(now_s() - resume::HOLD_S - 1, false)).unwrap();
        assert!(planned(&mut lead).0);
        // Released by the owner: the same. (Noted half an hour ago: not maybe running now.)
        let at = now_s() - 1800;
        resume::write(&store::nasfs::NasRoot::local_for_test(&r), &held(at, false)).unwrap();
        assert!(!planned(&mut lead).0);
        resume::release(&store::nasfs::NasRoot::local_for_test(&r), &im, &pass).unwrap();
        assert!(planned(&mut lead).0);
        // Switched off again: the offer taken back, the plan as before. (Progress begun a minute
        // before the released one: not that release's.)
        resume::write(&store::nasfs::NasRoot::local_for_test(&r), &held(at - 60, false)).unwrap();
        assert!(!planned(&mut lead).0 && lead.coord.as_ref().unwrap().shared.lock().unwrap().resume.is_some());
        std::fs::write(r.join(resume::SWITCH), "off").unwrap();
        lead.pool_switch(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(planned(&mut lead).0 && lead.coord.as_ref().unwrap().shared.lock().unwrap().resume.is_none());
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
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        m.mac = MEMBER_MAC;
        // The lead on a newer app than the member's (its updater ran first): the member gets
        // nothing, saying why, however long it asks.
        let theirs = m.app.clone();
        lead.coord.as_ref().unwrap().shared.lock().unwrap().app = "20261010-0900-aaaaaaa".into();
        lead.coord.as_ref().unwrap().offer(
            "2026-09-28",
            vec![crate::coord::Offer {
                step: "slope-hi".into(),
                targets: vec![("3/2/2".into(), "k".into(), 100)],
                job: None,
            }],
        );
        for _ in 0..3 {
            m.round().unwrap();
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

    /// Tasks brokered by every member (`state/pool/brokers`, crate::agent::brokers), two agents in
    /// one process: the member runs a broker and gives its job its address; its heartbeat says
    /// where it is, and it answers a page from the lead's address (CORS); a worker takes the
    /// member's task straight from the member's broker, and the lead's own task from the lead,
    /// each beating and handed back where it was taken. Off: the member's broker stops.
    #[test]
    fn brokers_a_members_task_is_taken_from_it() {
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let bport = free_port();
        TEST_BROKER_PORT.with(|p| p.set(Some(bport)));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        switch_on(&r, brokers::SWITCH);
        let bin = app(d.path());
        let gate = d.path().join("gate");
        gate_step(&bin, "slope-hi", &gate);
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let lport = lead.coord.as_ref().unwrap().port();
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        m.mac = MEMBER_MAC;
        m.round().unwrap();
        let (il, im) = (lead.pooled().unwrap().side.member().id.clone(), m.pooled().unwrap().side.member().id.clone());
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        m.pooled().unwrap().side.know(&il);
        // The member's broker, on a port of its own; the lead's coordinator brokers the lead's.
        assert!(m.coord.is_none() && m.broker.as_ref().is_some_and(|b| b.port() == bport) && lead.broker.is_none());
        // Its job given its broker's address (it waits at the gate meanwhile).
        // (Offered again until taken: the lead's own plan, made as it steps, offers whole and
        // would take a single offer back.)
        until(&mut m, |m| {
            lead.coord.as_ref().unwrap().offer(
                "2026-09-28",
                vec![crate::coord::Offer {
                    step: "slope-hi".into(),
                    targets: vec![("3/2/2".into(), "k".into(), 100)],
                    job: None,
                }],
            );
            m.slots[0].running.is_some()
        });
        // A task each: the member's job's at its broker, the lead's own at its coordinator.
        let offer = |port: u16, token: &str, unit: &str| -> u64 {
            let job =
                crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], token.to_string(), "job");
            let root = d.path().join(format!("task-{port}"));
            std::fs::create_dir_all(&root).unwrap();
            let o = crate::coord::task::Offer {
                owner: 42,
                kind: "tail".into(),
                spec: serde_json::json!({ "unit": unit }),
                root,
                inputs: Default::default(),
                mem_mb: 300,
                ahead: None,
            };
            job.post_json("/task/offer", &serde_json::to_value(&o).unwrap()).unwrap().1["id"].as_u64().unwrap()
        };
        // (The agents themselves, slower at a tail than the job's own run: neither takes these
        // tasks while the heartbeats are awaited below, however many steps that takes; the
        // worker below must.)
        for co in [m.broker.as_ref().unwrap(), lead.coord.as_ref().unwrap()] {
            let mut s = co.shared.lock().unwrap();
            for w in [&lead.host, &m.host] {
                s.tasks.paces.insert((w.clone(), "tail".into()), 10.0);
            }
        }
        let mt = offer(bport, &m.broker.as_ref().unwrap().job_token, "6/1/1");
        let lt = offer(lport, &lead.coord.as_ref().unwrap().job_token, "6/1/2");
        // The heartbeats say so, and each reads the other's (now: not waiting the half minute).
        let fresh = |a: &mut Agent| {
            a.pooled().unwrap().brokers_read = None;
            a.round().unwrap();
        };
        let told = |a: &Agent, who: &str| {
            a.coord.as_ref().or(a.broker.as_ref()).unwrap().shared.lock().unwrap().brokers.as_ref().is_some_and(|b| {
                b.peers.iter().any(|p| p.member == who && p.broker.as_ref().is_some_and(|b| !b.urls.is_empty()))
            })
        };
        let end = Instant::now() + WATCHDOG;
        while !(told(&lead, &im) && told(&m, &il)) {
            fresh(&mut lead);
            fresh(&mut m);
            assert!(Instant::now() < end, "the brokers' heartbeats not read");
            std::thread::sleep(Duration::from_millis(250));
        }
        // A page from the lead's address reads the member's broker, with CORS.
        let page_origin = crate::coord::brokers::origin(&lead.coord.as_ref().unwrap().contact.urls[0]).unwrap();
        let (code, _, acao) = http_post(bport, "/work/swarm", &[("Origin", &page_origin)], "{}");
        assert!(matches!(code, 200 | 503) && acao.as_deref() == Some(page_origin.as_str()), "{code} {acao:?}");
        // A worker (another Mac's agent, with the pool's key) takes the member's task from the
        // member's broker, and the lead's from the lead.
        let (mkey, lkey) =
            (m.broker.as_ref().unwrap().contact.token.clone(), lead.coord.as_ref().unwrap().contact.token.clone());
        let call = |port: u16, path: &str, body: serde_json::Value| -> (u16, serde_json::Value, Option<String>) {
            let key = format!("Bearer {}", if port == bport { &mkey } else { &lkey });
            http_post(port, path, &[("Authorization", &key), ("X-Worker", "w9")], &body.to_string())
        };
        let ask = |c: &crate::coord::Coordinator| {
            let app = c.shared.lock().unwrap().app.clone();
            serde_json::json!({ "worker": "w9", "kind": "native", "app": app, "can": ["tail"], "mem_mb": 1000, "cores": 4 })
        };
        let (code, g, _) = call(bport, "/work/ask", ask(m.broker.as_ref().unwrap()));
        assert_eq!((code, g["id"].as_u64()), (200, Some(mt)), "{g}");
        let ml = g["lease"].as_u64().unwrap();
        let (code, g, _) = call(lport, "/work/ask", ask(lead.coord.as_ref().unwrap()));
        assert_eq!((code, g["id"].as_u64()), (200, Some(lt)), "{g}");
        let ll = g["lease"].as_u64().unwrap();
        // Neither task dropped: each worker's lease beats and hands back where it was taken.
        for (port, lease, id) in [(bport, ml, mt), (lport, ll, lt)] {
            let (code, b, _) = call(port, "/work/beat", serde_json::json!({ "worker": "w9", "lease": lease }));
            assert_eq!((code, b["ok"].as_bool()), (200, Some(true)), "task {id}'s lease at {port}");
            let (code, _, _) = call(
                port,
                "/work/done",
                serde_json::json!({ "worker": "w9", "lease": lease, "outputs": [], "secs": 1.0, "peak_mb": 100 }),
            );
            assert_eq!(code, 200, "task {id} handed back at {port}");
        }
        assert_eq!(m.broker.as_ref().unwrap().leases_out(), 0);
        // The member's job had its broker's address, and ends through it, reaching the lead.
        std::fs::write(&gate, b"").unwrap();
        let end = Instant::now() + WATCHDOG;
        while !(m.slots[0].running.is_none()
            && lead
                .pooled()
                .unwrap()
                .side
                .driver()
                .records()
                .is_some_and(|r| r.keys.recorded("slope-hi", "3/2/2") == Some("k")))
        {
            lead.round().unwrap();
            m.round().unwrap();
            assert!(Instant::now() < end, "the job's end not merged");
            std::thread::sleep(Duration::from_millis(250));
        }
        let env = std::fs::read_to_string(bin.join("env-slope-hi")).unwrap();
        assert!(env.contains(&format!("SCENIC_COORD=http://127.0.0.1:{bport}")), "{env}");
        // Off: the member's broker stops, the lead's coordinator told none, and the heartbeat says
        // none from the run's next step.
        std::fs::write(r.join(brokers::SWITCH), "off").unwrap();
        lead.round().unwrap();
        m.round().unwrap();
        assert!(m.broker.is_none() && m.pooled().unwrap().side.broker.is_none());
        assert!(lead.coord.as_ref().unwrap().shared.lock().unwrap().brokers.is_none());
        // (Another Mac's page: this test's lead is on loopback, whose pages this Mac always answers.)
        assert_eq!(http_post(lport, "/work/swarm", &[("Origin", "http://m4.local:8090")], "{}").0, 403);
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
        TEST_BROKER_PORT.with(|p| p.set(None));
    }

    /// A POST of JSON to this process's coordinator on `port`, as a page sends it: its status, its
    /// JSON, and the origin its answer allows (CORS).
    fn http_post(
        port: u16,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, serde_json::Value, Option<String>) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let head: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        s.write_all(format!("POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
        let mut r = Vec::new();
        s.read_to_end(&mut r).unwrap();
        let text = String::from_utf8_lossy(&r).to_string();
        let (h, b) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let code = h.split(' ').nth(1).unwrap().parse().unwrap();
        let acao = h.lines().find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("access-control-allow-origin"))
                .map(|(_, v)| v.trim().to_string())
        });
        (code, serde_json::from_str(b.trim()).unwrap_or_default(), acao)
    }

    /// Task #882 step 1 (docs/roles.md, decision 3): a pool past term 1 with no lead declared: the
    /// role isn't known, so the agent starts a member's parts (no plan, no coordinator); the
    /// declaration then read naming another member, it is a member, with no restart.
    #[test]
    fn an_unknown_role_starts_the_members_parts() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let root = store::nasfs::NasRoot::local_for_test(r.clone());
        // (Term 2 by another Mac, the pool from before #881: none declared.)
        let id = String::from("m-00000000000000ff");
        let other = crate::pool::Member { id, host: "mini".into(), app: "x".into() };
        crate::pool::term::bootstrap(&root, &other, now_s() - 60).unwrap();
        let cur = crate::pool::term::current(&root).unwrap();
        crate::pool::term::claim(&root, &cur, &other, "a test", now_s() - 30).unwrap();
        let mut a = test_agent(Options {
            root: Some(root.clone()),
            home: d.path().join("m/agent"),
            bin: app(d.path()),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        assert!(a.pool.is_some() && a.role == Role::Unknown, "{:?}", a.role);
        assert!(a.is_member() && !a.plan.plans() && a.coord.is_none(), "a member's parts");
        a.round().unwrap();
        assert_eq!((a.role, a.pool_restart()), (Role::Unknown, None));
        // Declared: another Mac. A member now, in this process.
        move_lead(&r, &other.id);
        until(&mut a, |a| a.role == Role::Member);
        assert!(a.restart_now().is_none() && !a.plan.plans() && a.coord.is_none());
        stop_jobs(&mut a);
    }

    /// Task #882 step 1: the owner moves the lead's declaration from the lead to the member: each
    /// restarts at once (a job running or not: its jobs handed over), never changing its parts in
    /// its process; started again, each has the parts of its new role.
    #[test]
    fn a_changed_declaration_restarts_into_the_new_role() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let o = |who: &str| Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join(who).join("agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut lead = test_agent(o("l")).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let mut m = test_agent(o("m")).unwrap();
        m.round().unwrap();
        assert_eq!((lead.role, m.role), (Role::Lead, Role::Member));
        assert!(lead.coord.is_some() && lead.plan.plans() && m.coord.is_none() && !m.plan.plans());
        // A job of the lead's own runs in its first slot.
        let job = JobSpec {
            id: "x x".into(),
            what: "x".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: None,
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        lead.start(0, job, &cond).unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        move_lead(&r, &im);
        let end = Instant::now() + WATCHDOG;
        while lead.restart_now().is_none() || m.restart_now().is_none() {
            assert!(Instant::now() < end, "no restart in {WATCHDOG:?}");
            lead.round().unwrap();
            m.round().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        }
        assert!(lead.slots[0].running.is_some(), "at once, its job running");
        assert!(lead.restart_now().unwrap().contains("the lead → a member"), "{:?}", lead.restart_now());
        assert!(m.restart_now().unwrap().contains("a member → the lead"), "{:?}", m.restart_now());
        // Its parts unchanged in its process.
        assert!(lead.coord.is_some() && lead.leads() && m.coord.is_none() && m.is_member());
        // Restarted (the old processes' jobs handed over, their coordinators stopped).
        lead.hand_over_jobs();
        for a in [&mut lead, &mut m] {
            if let Some(c) = a.coord.take() {
                c.stop();
            }
            a.plan.stop(Duration::ZERO);
        }
        drop(lead);
        drop(m);
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let mut lead = test_agent(o("m")).unwrap();
        let mut m = test_agent(o("l")).unwrap();
        assert_eq!((lead.role, m.role), (Role::Lead, Role::Member));
        assert!(lead.coord.is_some() && lead.plan.plans(), "the new lead's parts");
        assert!(m.coord.is_none() && !m.plan.plans(), "the old lead's are a member's now");
        assert!(m.slots[0].running.is_some(), "its job taken over");
        stop_jobs(&mut m);
        stop_jobs(&mut lead);
    }

    /// Task #882 step 2 (docs/roles.md §5): two agents, the lead and a member. The member counts
    /// no region: it has no plan (so no `start_as_built`, no regions' states or steps, no
    /// checklist), opens no pack index, and the indexes a lead's process left in its folder go as it
    /// starts; its status carries none of the build's regions or checklist.
    #[test]
    fn a_member_never_counts_the_regions() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let o = |who: &str| Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join(who).join("agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut lead = test_agent(o("l")).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        // (Left by a process of this Mac's that led.)
        let idx = d.path().join("m/agent/pack-idx");
        std::fs::create_dir_all(&idx).unwrap();
        std::fs::write(idx.join("0123456789abcdef.idx"), b"x").unwrap();
        let mut m = test_agent(o("m")).unwrap();
        assert_eq!(m.role, Role::Member);
        assert!(!idx.exists(), "its pack indexes removed as it starts");
        for _ in 0..3 {
            lead.round().unwrap();
            m.round().unwrap();
        }
        assert!(lead.plan.plans() && !m.plan.plans(), "the member has no plan to count with");
        assert!(!idx.exists(), "no pack index opened");
        let view = m.plan.view.get();
        assert!(view.built.is_empty() && view.checklist.is_empty() && view.tiles.is_none());
        let st: Status = serde_json::from_slice(&std::fs::read(m.o.home.join("helper.json")).unwrap()).unwrap();
        assert!(st.regions.is_empty() && st.checklist.is_empty(), "{:?}", st.checklist);
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    /// Task #882 step 2, with #871's slow-NAS fake: the records' briefs hung, a member's rounds go
    /// on and never call for one (no checklist, no counts).
    #[test]
    fn a_members_round_never_waits_on_the_records_brief() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let o = |who: &str| Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join(who).join("agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut lead = test_agent(o("l")).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let mut m = test_agent(o("m")).unwrap();
        m.round().unwrap();
        let hung = nasread::fake::hang(&store::nasfs::NasRoot::local_for_test(&r).join(BRIEFS).unchecked_path());
        for _ in 0..5 {
            m.round().unwrap();
        }
        assert_eq!(hung.held(), 0, "the member's rounds asked for no brief");
        // (The fake sees a reader of the briefs, the plan's checklist's: held, it returns past
        // the deadline with none.)
        let read = records_deadlined(&store::nasfs::NasRoot::local_for_test(&r));
        assert!(hung.held() == 1 && read.keys.recorded("unit", "6/1/1").is_none());
        drop(hung);
        lead.plan.stop(Duration::ZERO);
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    /// A term taken up by a process with no coordinator (its role as it started a member's, or
    /// its coordinator didn't start): it restarts at once.
    #[test]
    fn a_term_without_a_coordinator_restarts_at_once() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: bin.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.round().unwrap();
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin,
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        m.round().unwrap();
        assert_eq!(m.pooled().unwrap().role, pool::Role::Member);
        let out = crate::pool::driver::Out { leads: Some(2), ..Default::default() };
        m.change_part(&store::nasfs::NasRoot::local_for_test(&r), &out);
        assert!(m.coord.is_none() && m.is_member());
        assert!(m.restart_now().is_some_and(|w| w.contains("no coordinator")), "{:?}", m.pool_restart());
        drop(lead);
    }

    #[test]
    fn an_own_jobs_timings_for_the_lead_are_the_slots_job_alone() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("agent"),
            bin: app(d.path()),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let rec = || Some(crate::timings::RunRec { kind: "unit".into(), ..Default::default() });
        // (Left from the slot's last job.) A job starting there carries none of them.
        a.slots[0].timings_out = rec();
        let spec = JobSpec {
            id: "x x".into(),
            what: "x".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: None,
        };
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

    /// The declared lead that loses its term: its duties stopped, and it restarts at once.
    #[test]
    fn a_lead_that_loses_its_term_stops_its_duties_and_restarts() {
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin,
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let catalogue = JobSpec {
            id: "catalog catalog".into(),
            what: "catalog".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "catalog".into(), targets: vec![("catalog".into(), "kc".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, catalogue, &cond, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()));
        // (Its step said it leads no term any more.)
        let out = crate::pool::driver::Out::default();
        lead.change_part(&store::nasfs::NasRoot::local_for_test(&r), &out);
        assert!(lead.slots[0].running.is_none(), "its catalogue stopped");
        assert_eq!(lead.pooled().unwrap().role, pool::Role::Member);
        assert!(lead.restart_now().is_some_and(|w| w.contains("no longer leads")), "{:?}", lead.pool_restart());
    }

    #[test]
    fn a_catalogue_the_lead_publishes_is_its_map_log_s_in_shadow() {
        // (docs/maplog.md §8, step 2: the catalog job's end, as the loop tends it.)
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let root = store::nasfs::NasRoot::local_for_test(r.clone());
        let mut lead = test_agent(Options {
            root: Some(root.clone()),
            home: d.path().join("l/agent"),
            bin: app(d.path()),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        // (What the job publishes, written here: the step's own work isn't the test's.)
        let mut c = store::catalogue::Catalogue::new(
            store::catalogue::list(r.join("catalog").as_path()).map_or(0, |ns| ns.into_iter().max().unwrap_or(0)) + 1,
        );
        c.units = vec!["6/1/1".into(), "6/1/2".into()];
        store::catalogue::write(&r.join("catalog"), &c).unwrap();
        let catalogue = JobSpec {
            id: "catalog catalog".into(),
            what: "catalog".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "true".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "catalog".into(), targets: vec![("catalog".into(), "kc".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, catalogue, &cond, Some(&root), &mut Vec::new()));
        store::testkit::eventually("the job ends", || lead.tend(0, &cond, Some(&root), 0).unwrap());
        // (On the shadow's own thread: its log seeded at that catalogue, and the NAS's snapshot.)
        let nas_snap = r.join(store::map::NAS_DIR).join(store::map::nas_name(c.n));
        store::testkit::eventually(&format!("the shadow writes the map at {}", c.n), || nas_snap.exists());
        let log = store::map::log::Log::open(&d.path().join("l/agent").join(publish::MAPLOG)).unwrap();
        assert_eq!(log.map().seq, c.n);
        log.map().check_is(&c).unwrap();
        // (Step 3: the lead's coordinator answers the servers from the shadow's feed.)
        lead.round().unwrap();
        assert!(lead.coord.as_ref().is_some_and(|c| c.has_maplog()), "the map's routes answer");
        assert_eq!(lead.maplog.here(&lead.o.home).feed.head(), Some(c.n));
        // (Step 4: and the plan reads the map from it, the catalogue checked equal to it.)
        assert_eq!(*lead.planner().map_now(&root).unwrap(), c);
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
    fn a_lead_whose_update_waits_on_its_job_keeps_the_lead() {
        // (Task #497) A newer app waits on the lead's own job, a member already on it: the lead
        // keeps the lead; it restarts at a safe point like any member.
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
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
        let mut lead = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("l/agent"),
            bin: old.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        // (A job of its own, holding its update.)
        let job = JobSpec {
            id: "pack 6/1/1".into(),
            what: "pack".into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "pack".into(), targets: vec![("6/1/1".into(), "k".into())] }),
        };
        lead.round().unwrap();
        assert_eq!(lead.pending_app().as_deref(), Some("20261010-0200-bbbbbbb"));
        let mut m = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("m/agent"),
            bin: new.clone(),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        assert!(lead.try_start(0, job, &cond, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()));
        for _ in 0..12 {
            lead.round().unwrap();
            m.round().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        }
        assert_eq!(m.pooled().unwrap().side.driver().leads(), None);
        assert!(lead.pooled().unwrap().side.driver().leads().is_some(), "it keeps the lead");
        assert!(lead.slots[0].running.is_some(), "its job runs on");
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    #[test]
    fn a_lead_quitting_quits_once_its_jobs_end_and_its_members_wait() {
        // (Task #194: Scenic quit on the lead's Mac. Since #881 the lead is fixed: it hands nothing
        // over; the build waits for it, the members finishing their jobs.)
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let made = app(d.path());
        let v = d.path().join("v/app/20261010-0100-aaaaaaa");
        std::fs::create_dir_all(&v).unwrap();
        for p in ["scenic", "scenic-build"] {
            std::fs::copy(made.join(p), v.join(p)).unwrap();
        }
        let opts = |h: &str| Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join(h),
            bin: v.clone(),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut lead = test_agent(opts("l/agent")).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let mut m = test_agent(opts("m/agent")).unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        for _ in 0..4 {
            m.round().unwrap();
            lead.round().unwrap();
        }
        crate::control::request_quit(&lead.o.home, crate::control::QuitMode::Now, "Scenic on l").unwrap();
        lead.round().unwrap();
        assert!(lead.quit_due(), "no job runs: it quits at once");
        for _ in 0..4 {
            m.round().unwrap();
        }
        assert_eq!(m.pooled().unwrap().side.driver().leads(), None, "the member takes nothing over");
        crate::control::clear_quit(&lead.o.home);
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    #[test]
    fn a_lead_paused_from_a_member_keeps_the_lead_and_resumes() {
        // (Task #224: `scenic pause <member>` from another Mac, the lead named. Task #497: its own
        // jobs stop; it keeps the lead and goes on granting the members' work; it goes on once
        // resumed.)
        room::TEST_FREE.with(|c| c.set(Some(400 << 30)));
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let made = app(d.path());
        let v = d.path().join("v/app/20261010-0100-aaaaaaa");
        std::fs::create_dir_all(&v).unwrap();
        for p in ["scenic", "scenic-build"] {
            std::fs::copy(made.join(p), v.join(p)).unwrap();
        }
        let opts = |h: &str| Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join(h),
            bin: v.clone(),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut lead = test_agent(opts("l/agent")).unwrap();
        lead.mem.last_ok.insert("backup".into(), now_s());
        lead.mem.last_ok.insert("gc".into(), now_s());
        lead.round().unwrap();
        let il = lead.pooled().unwrap().side.member().id.clone();
        let mut m = test_agent(opts("m/agent")).unwrap();
        let im = m.pooled().unwrap().side.member().id.clone();
        lead.pooled().unwrap().side.know(&im);
        lead.seen_pool();
        let end = Instant::now() + WATCHDOG;
        let seen = |a: &Agent| {
            a.pooled().unwrap().controls.view.as_ref().is_some_and(|v| v.members.iter().any(|x| !x.me && x.beat > 0))
        };
        while !seen(&lead) {
            m.round().unwrap();
            lead.round().unwrap();
            assert!(Instant::now() < end, "the member never seen: {:?}", lead.pooled().unwrap().controls.view);
            std::thread::sleep(Duration::from_millis(250));
        }
        // The lead's file on the NAS unreadable (damaged): refused, ask again, nothing written.
        let lead_file = r.join(crate::control::hold_path(&il));
        std::fs::create_dir_all(lead_file.parent().unwrap()).unwrap();
        std::fs::write(&lead_file, b"{").unwrap();
        let ask = crate::control::request_hold(&m.o.home, Some(&il), None).unwrap();
        m.round().unwrap();
        let a = crate::control::hold_answer(&m.o.home, &ask).unwrap();
        assert!(!a.ok && a.said.contains("can't be read now"), "{a:?}");
        assert_eq!(std::fs::read(&lead_file).unwrap(), b"{");
        std::fs::remove_file(&lead_file).unwrap();
        // The member's owner pauses the lead: written to the lead's file on the NAS, answered.
        let ask =
            crate::control::request_hold(&m.o.home, Some(&il), Some(crate::control::Hold::new("scenic pause on m")))
                .unwrap();
        m.round().unwrap();
        let a = crate::control::hold_answer(&m.o.home, &ask).unwrap();
        assert!(a.ok, "{a:?}");
        assert!(m.hold().held.is_none(), "the member itself goes on");
        for _ in 0..12 {
            lead.hold_read = None;
            lead.round().unwrap();
            m.round().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        }
        assert!(lead.hold().held.as_ref().is_some_and(|h| h.by == "scenic pause on m"));
        assert_eq!(m.pooled().unwrap().side.driver().leads(), None);
        assert!(lead.pooled().unwrap().side.driver().leads().is_some(), "it keeps the lead");
        assert_eq!(
            lead.coord.as_ref().unwrap().shared.lock().unwrap().moving,
            None,
            "its coordinator grants the members' work"
        );
        // Paused: its heartbeat says so.
        assert!(lead.pooled().unwrap().conds.unwrap().held);
        // Resumed from the member: taken up by the lead.
        let ask = crate::control::request_hold(&m.o.home, Some(&il), None).unwrap();
        m.round().unwrap();
        assert!(crate::control::hold_answer(&m.o.home, &ask).is_some_and(|a| a.ok));
        lead.hold_read = None;
        lead.round().unwrap();
        assert!(lead.hold().held.is_none() && crate::control::read_hold(&lead.o.home).held.is_none());
        stop_jobs(&mut lead);
        stop_jobs(&mut m);
    }

    #[test]
    fn a_retry_blocked_stays_so_while_the_run_cant_be_read() {
        // (Task #409 follow-up) a tick holding the pool's run says nothing new: the last word
        // stands, so the page's retry is still answered 409; once read, it's said again.
        assert_eq!(retry_blocked(Some((None, "b34"))), Some(Some("no lead now".into())));
        assert!(retry_blocked(Some((Some("b33"), "b34"))).is_some_and(|w| w.is_some_and(|w| w.contains("b33"))));
        assert_eq!(retry_blocked(Some((Some("b34"), "b34"))), Some(None));
        assert_eq!(retry_blocked(None), None, "not known: nothing set");
        let (c, _port) = crate::coord::start_for_test(&tempfile::tempdir().unwrap().path().join("coord"), "m1", "b34");
        for seen in [Some((None, "b34")), None] {
            if let Some(b) = retry_blocked(seen) {
                c.set_retry_blocked(b);
            }
            assert!(c.shared.lock().unwrap().retry_blocked.is_some(), "{seen:?}");
        }
        c.stop();
    }

    #[test]
    fn the_forecast_gives_a_member_paused_alone_no_lanes() {
        // (Task #224: its jobs are given back and it takes nothing new until resumed.)
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let mut a = agent_dry(&store::nasfs::NasRoot::local_for_test(&r), &d.path().join("home"));
        std::fs::create_dir_all(r.join("state/helpers")).unwrap();
        for (host, held) in [("busy-mac", false), ("held-mac", true)] {
            let st = Status {
                host: host.into(),
                beat: now_s(),
                held: held.then(|| crate::control::Hold::new("scenic pause on held-mac")),
                ..Default::default()
            };
            std::fs::write(r.join(format!("state/helpers/{host}.json")), serde_json::to_vec(&st).unwrap()).unwrap();
        }
        let work: Vec<build::Work> = (0..40)
            .map(|i| build::Work { step: "bldtiles".into(), targets: vec![(format!("6/{i}/1"), "k".into())] })
            .collect();
        let rec = a.planner().forecast_record(
            &store::nasfs::NasRoot::local_for_test(&r),
            &[],
            &[],
            &[],
            &work,
            Default::default(),
            None,
            &|_, _| 0,
        );
        let fc = rec.forecast();
        let named = |h: &str| fc.lanes.keys().chain(fc.next.keys()).any(|k| is_slot_of(k, h));
        assert!(
            named("busy-mac"),
            "{:?} {:?}",
            fc.lanes.keys().collect::<Vec<_>>(),
            fc.next.keys().collect::<Vec<_>>()
        );
        assert!(!named("held-mac"));
        // (Task #409) its input as written, read back and run, is the lead's forecast.
        let b = rec.bytes().unwrap();
        let back = forecast::Record::from_bytes(&b).unwrap();
        assert!(!back.input.machines.is_empty());
        assert_eq!(back.forecast(), fc);
    }

    #[test]
    fn a_new_lead_s_own_member_status_gives_it_no_second_lanes() {
        // (Task #816: just after a change of lead the lead's own state/helpers file, from while it was a
        // member, still beats fresh; read as a member's, its slots were listed twice under the
        // same names, the lanes kept one set, and forecast::runs_of numbered runs past them: the
        // forecast thread panicked "index out of bounds".)
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let mut a = agent_dry(&store::nasfs::NasRoot::local_for_test(&r), &d.path().join("home"));
        a.mem.ramp.slots = 3;
        std::fs::create_dir_all(r.join("state/helpers")).unwrap();
        for host in [a.host.clone(), "busy-mac".to_string()] {
            let st = Status { host: host.clone(), beat: now_s(), ..Default::default() };
            std::fs::write(r.join(format!("state/helpers/{host}.json")), serde_json::to_vec(&st).unwrap()).unwrap();
        }
        let work: Vec<build::Work> = (0..200)
            .map(|i| build::Work { step: "bldtiles".into(), targets: vec![(format!("6/{i}/1"), "k".into())] })
            .collect();
        let rec = a.planner().forecast_record(
            &store::nasfs::NasRoot::local_for_test(&r),
            &[],
            &[],
            &[],
            &work,
            Default::default(),
            None,
            &|_, _| 0,
        );
        let names: Vec<&str> = rec.input.machines.iter().map(|m| m.name.as_str()).collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "each lane once: {names:?}");
        assert!(rec.input.machines.iter().filter(|m| m.name == a.host).all(|m| m.lead_steps));
        let fc = rec.forecast();
        assert_eq!(fc.runs.targets.len(), fc.lanes.values().map(Vec::len).sum::<usize>());
    }

    #[test]
    fn the_forecast_gives_a_lead_paused_alone_no_lanes() {
        // (Task #628: a lead paused alone grants itself nothing new, so none of its lanes show work.)
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let mut a = agent_dry(&store::nasfs::NasRoot::local_for_test(&r), &d.path().join("home"));
        a.set_hold(Some(crate::control::Hold::new("scenic pause --here")), now_s());
        std::fs::create_dir_all(r.join("state/helpers")).unwrap();
        let st = Status { host: "busy-mac".into(), beat: now_s(), ..Default::default() };
        std::fs::write(r.join("state/helpers/busy-mac.json"), serde_json::to_vec(&st).unwrap()).unwrap();
        let work: Vec<build::Work> = (0..40)
            .map(|i| build::Work { step: "bldtiles".into(), targets: vec![(format!("6/{i}/1"), "k".into())] })
            .collect();
        let rec = a.planner().forecast_record(
            &store::nasfs::NasRoot::local_for_test(&r),
            &[],
            &[],
            &[],
            &work,
            Default::default(),
            None,
            &|_, _| 0,
        );
        assert!(rec.input.machines.iter().all(|m| m.mac.as_deref() != Some(a.host.as_str())));
        let fc = rec.forecast();
        let named = |h: &str| fc.lanes.keys().chain(fc.next.keys()).any(|k| is_slot_of(k, h));
        assert!(named("busy-mac") && !named(&a.host), "{:?}", fc.lanes.keys().collect::<Vec<_>>());
    }

    #[test]
    fn a_local_resume_wins_over_a_future_dated_pause_and_a_busy_pool_step_never_spins_the_loop() {
        // (Task #224 review.)
        TEST_PORT.with(|p| p.set(Some(free_port())));
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let bin = app(d.path());
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: d.path().join("app-folder/agent"),
            bin,
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        a.mem.last_ok.insert("backup".into(), now_s());
        a.mem.last_ok.insert("gc".into(), now_s());
        a.round().unwrap();
        let home = a.o.home.clone();
        // Paused from a Mac whose clock runs an hour ahead; resumed here: this Mac's ask wins.
        let me = a.pooled().unwrap().side.member().id.clone();
        let ahead = crate::control::HoldState {
            held: Some(crate::control::Hold::new("scenic pause on ahead")),
            at: now_s() + 3600,
        };
        write_hold(&store::nasfs::NasPath::local_for_test(&r.join(crate::control::hold_path(&me))), &ahead).unwrap();
        a.hold_read = None;
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(a.hold().held.is_some());
        let ask = crate::control::request_hold(&home, None, None).unwrap();
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(crate::control::hold_answer(&home, &ask).is_some_and(|x| x.ok));
        assert!(a.hold().held.is_none() && a.hold().at > ahead.at);
        // Written back over the NAS's older setting.
        a.hold_read = None;
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        let there: crate::control::HoldState =
            serde_json::from_slice(&std::fs::read(r.join(crate::control::hold_path(&me))).unwrap()).unwrap();
        assert_eq!(there, *a.hold());
        // The pool's step busy: an ask naming another Mac waits, and doesn't cut the loop's wait
        // again and again; this Mac's own is taken all the same.
        let shared = a.pool.clone().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            let _g = pool::lock(&shared);
            held_tx.send(()).unwrap();
            rx.recv().ok();
        });
        held_rx.recv().unwrap();
        while a.control.woken.try_recv().is_ok() {}
        crate::control::request_hold(&home, Some("other-mac"), Some(crate::control::Hold::new("x"))).unwrap();
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(a.control.woken.try_iter().any(|m| m == bus::ControlMsg::Ask));
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(home.join(crate::control::HOLD_REQUEST).exists() && a.control.woken.try_recv().is_err());
        let ask = crate::control::request_hold(&home, None, Some(crate::control::Hold::new("here"))).unwrap();
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&r)));
        assert!(crate::control::hold_answer(&home, &ask).is_some_and(|x| x.ok) && a.hold().held.is_some());
        tx.send(()).unwrap();
        t.join().unwrap();
    }

    #[test]
    fn pausing_this_mac_alone_drains_its_job_takes_nothing_new_and_survives_a_restart() {
        // (Task #224: `scenic pause --here`.)
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let opts = || Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: false,
            once: true,
            member: false,
        };
        let mut a = test_agent(opts()).unwrap();
        a.free_set = Some(40 << 30);
        let job = |id: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "sleep 3600".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "bldtiles".into(), targets: vec![("6/1/1".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 3600 };
        assert!(a.try_start(
            0,
            job("bldtiles 6/1/1"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut Vec::new()
        ));
        let pgid = a.slots[0].running.as_ref().unwrap().pgid;
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let _kill = Kill(pgid);
        // Paused here: the running job asked to stop at its next safe point, nothing new starts
        // (the status says why), the build itself not paused.
        let ask =
            crate::control::request_hold(&home, None, Some(crate::control::Hold::new("scenic pause on m1"))).unwrap();
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(crate::control::hold_answer(&home, &ask).is_some_and(|x| x.ok));
        assert!(a.hold().held.is_some() && a.pause.is_none());
        a.tend(0, &cond, Some(&store::nasfs::NasRoot::local_for_test(&root)), 0).unwrap();
        assert!(a.slots[0]
            .running
            .as_ref()
            .is_some_and(|r| r.pausing.as_deref().is_some_and(|w| w.contains("scenic pause on m1"))));
        assert_eq!(std::fs::read(a.control_path(0)).unwrap(), b"drain");
        let mut waiting = Vec::new();
        assert!(!a.try_start(
            1,
            job("bldtiles 6/2/2"),
            &cond,
            Some(&store::nasfs::NasRoot::local_for_test(&root)),
            &mut waiting
        ));
        a.slots[1].lease = None;
        assert!(waiting.iter().any(|w| w.why.contains("this Mac is paused")), "{waiting:?}");
        a.stop_jobs("stopped: the test");
        drop(a);
        // A restart keeps it.
        let mut a = test_agent(opts()).unwrap();
        assert!(a.hold().held.as_ref().is_some_and(|h| h.by == "scenic pause on m1"));
        assert!(a.hold_why().is_some());
        // Resumed: work starts again.
        crate::control::request_hold(&home, None, None).unwrap();
        a.hold_asks(Some(&store::nasfs::NasRoot::local_for_test(&root)));
        assert!(a.hold().held.is_none() && a.hold_why().is_none() && crate::control::read_hold(&home).held.is_none());
    }

    /// Task #630: a catalogue the pool's records hold back (not caught up) isn't asked of the lead's
    /// coordinator, so not granted, leased and logged again every loop only to wait; said waiting.
    #[test]
    fn a_catalogue_the_records_hold_back_is_not_asked_for() {
        room::TEST_FREE.with(|c| c.set(Some(100 << 30)));
        let d = tempfile::tempdir().unwrap();
        let (root, home) = (d.path().join("nas"), d.path().join("home"));
        std::fs::create_dir_all(&root).unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(root.clone())),
            home: home.clone(),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let (co, _port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        a.coord = Some(std::sync::Arc::new(co));
        a.free_set = Some(100 << 30);
        a.mem_set = Some((16 << 10, 12 << 10));
        let nas_: pool::SharedNas = std::sync::Arc::new(store::nasfs::NasRoot::local_for_test(&root));
        let side = pool::Side::open(&home, &home.join("pool"), d.path(), "development", nas_).unwrap().unwrap();
        a.pool = Some(std::sync::Arc::new(std::sync::Mutex::new(pool::Run::new(
            side,
            pool::Role::Lead,
            pool::Gates { term: 1, leads: Some(1), duties: true, ..Default::default() },
        ))));
        a.seen_pool();
        let job = JobSpec {
            id: "catalog catalog".into(),
            what: "catalog".into(),
            cmd: vec!["/usr/bin/true".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: Some(build::Work { step: "catalog".into(), targets: vec![("catalog".into(), "k".into())] }),
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 0 };
        let jobs = std::slice::from_ref(&job);
        a.planner().offer_plan(jobs);
        for _ in 0..2 {
            let mut w = Vec::new();
            assert!(
                a.lead_grants(jobs, 0, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w).is_empty(),
                "held: not granted"
            );
            assert!(w.iter().any(|w| w.why == UNPUBLISHED), "{w:?}");
        }
        a.pooled().unwrap().gates.caught_up = true;
        a.seen_pool();
        let mut w = Vec::new();
        assert_eq!(
            a.lead_grants(jobs, 0, &cond, &store::nasfs::NasRoot::local_for_test(&root), &mut w).len(),
            1,
            "caught up: granted {w:?}"
        );
    }

    #[test]
    fn the_leads_gates_hold_a_catalogue_and_a_sweep() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let home = d.path().join("home");
        let mut a = agent_dry(&store::nasfs::NasRoot::local_for_test(&r), &home);
        let nas_: pool::SharedNas = std::sync::Arc::new(store::nasfs::NasRoot::local_for_test(&r));
        let side = pool::Side::open(&home, &home.join("pool"), d.path(), "development", nas_).unwrap().unwrap();
        a.pool = Some(std::sync::Arc::new(std::sync::Mutex::new(pool::Run::new(
            side,
            pool::Role::Lead,
            pool::Gates { term: 1, leads: Some(1), duties: true, ..Default::default() },
        ))));
        a.seen_pool();
        let c = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 9999 };
        let job = |id: &str| JobSpec {
            id: id.into(),
            what: id.into(),
            cmd: vec!["/usr/bin/true".into()],
            needs: Needs { nas: true },
            restart_after_sleep: false,
            record: None,
        };
        // Not caught up: no catalogue; a sweep asks for a re-assertion first.
        let mut w = Vec::new();
        assert!(!a.try_start(0, job("catalog"), &c, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut w));
        assert!(w.iter().any(|w| w.why.contains("reflect the journal")), "{w:?}");
        assert!(!a.try_start(0, job("gc"), &c, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut w));
        assert!(a.reassert);
        // Caught up: the catalogue; the sweep only on a step that re-asserted.
        a.pooled().unwrap().gates.caught_up = true;
        a.seen_pool();
        assert!(
            a.try_start(0, job("catalog"), &c, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()),
            "would start (dry run)"
        );
        assert!(!a.try_start(0, job("gc"), &c, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()));
        a.pooled().unwrap().gates.fresh = true;
        a.seen_pool();
        assert!(a.try_start(0, job("gc"), &c, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()));
        // Other work as before.
        a.pooled().unwrap().gates = pool::Gates::default();
        a.seen_pool();
        assert!(a.try_start(0, job("backup"), &c, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()));
    }

    #[test]
    fn a_member_whose_lock_is_held_waits_for_it_then_restarts_into_the_pool() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let home = d.path().join("app-folder/agent");
        let id = crate::pool::member_id(&home).unwrap();
        let held = crate::pool::MemberLock::take(&d.path().join("app-folder"), &id).unwrap().unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: home.clone(),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        assert!(a.pool.is_none() && a.coord.is_none() && !a.o.dry_run, "waiting, not a dry run for good");
        a.round().unwrap();
        let st = read_status(Some(&store::nasfs::NasRoot::local_for_test(&r)), &home).unwrap();
        assert!(
            st.waiting.iter().any(|w| w.why.contains("not in the pool yet") && w.why.contains("member's lock")),
            "{:?}",
            st.waiting
        );
        assert!(a.pool_restart().is_none() && a.slots.iter().all(|s| s.running.is_none()));
        // (Task #423: the status says this Mac is out of the pool, and why.)
        assert!(st.out_of_pool.as_deref().is_some_and(|w| w.contains("lock is held")), "{:?}", st.out_of_pool);
        assert!(st.role().starts_with("not in the pool: waiting to join (this Mac's member's lock"), "{}", st.role());
        // A job running here (one it took over): the lock free, it restarts at once all the same,
        // the job left running for the next agent, which joins as it starts.
        let job = JobSpec {
            id: "backup".into(),
            what: "a backup (a sleep)".into(),
            cmd: vec!["/bin/sleep".into(), "30".into()],
            needs: Needs { nas: false },
            restart_after_sleep: false,
            record: None,
        };
        let cond = Conditions { ac: true, battery: None, nas: true, home: true, idle_s: 9999 };
        assert!(a.try_start(0, job, &cond, Some(&store::nasfs::NasRoot::local_for_test(&r)), &mut Vec::new()));
        assert!(a.slots[0].running.is_some() && a.restart_now().is_none());
        drop(held);
        a.round().unwrap();
        assert!(a.pool_restart().is_some_and(|w| w.contains("lock")), "{:?}", a.pool_restart());
        assert!(a.slots[0].running.is_some());
        assert!(a.restart_now().is_some(), "not waiting for its job's end");
        stop_jobs(&mut a);
    }

    /// Task #423: an agent started while the one before it holds this Mac's member's lock (still
    /// draining after an update) joins the pool as it starts, within `JOIN_WAIT` of the lock's
    /// release, not once a job it took over ends.
    #[test]
    fn an_agent_started_while_the_lock_is_held_joins_once_it_is_let_go() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let home = d.path().join("app-folder/agent");
        let id = crate::pool::member_id(&home).unwrap();
        let held = crate::pool::MemberLock::take(&d.path().join("app-folder"), &id).unwrap().unwrap();
        let let_go = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            drop(held);
        });
        let a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: home.clone(),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let_go.join().unwrap();
        // (In the pool at all means the retry loop got the lock before its own `JOIN_WAIT` ran
        // out; the test's wall time, which a loaded machine stretches, isn't what's checked.)
        assert!(a.pool.is_some(), "in the pool as it starts");
        assert!(a.out_of_pool().is_none() && a.restart_now().is_none());
    }

    /// docs/coordination.md §6.1 (task #878 step 2): a member's beat lists its slots' leases and
    /// those whose ends it owes; the lead hears it speaking for them, and releases one it leaves out.
    #[test]
    fn a_member_s_beat_lists_its_jobs_and_owed_ends() {
        let d = tempfile::tempdir().unwrap();
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(d.path().join("nas"))),
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("lead"), "lead", "");
        let (job, owed, lost) = {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            let mut g = |t: &str| {
                let w = crate::coord::lease::Work::Job { step: "unit".into(), targets: vec![(t.into(), "k".into())] };
                s.leases.grant(&a.host, w, now)
            };
            (g("6/1/1"), g("6/1/2"), g("6/1/3"))
        };
        a.slots[0].lease = Some(Held::Leased { lease: job, dir: d.path().join("out") });
        a.owed.push(Tell::Fail { id: owed, why: "x".into(), floors: Vec::new(), disks: Vec::new(), job_disk: 0 });
        assert_eq!(a.held_of_lead(), [job, owed].into());
        a.client = Some(crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{port}")],
            c.contact.token.clone(),
            &a.host,
        ));
        // (Granted long enough ago: past the grace.)
        {
            let clock = store::clock::Virtual::new();
            clock.advance(crate::coord::held::OMIT_GRACE * 2);
            c.shared.lock().unwrap().clock = clock;
        }
        a.beat_lead();
        let held = c.held("unit");
        assert!(held.contains("6/1/1") && held.contains("6/1/2") && !held.contains("6/1/3"), "{held:?}");
        assert!(c.shared.lock().unwrap().leases.of(lost).is_none());
        // (Once a `BEAT_EVERY`.)
        a.slots[0].lease = None;
        a.beat_lead();
        assert!(c.held("unit").contains("6/1/1"));
    }

    /// #423 follow-up: a passing error opening the member (its pool folder can't be made a moment)
    /// is tried again within `JOIN_WAIT`, not a failed start.
    #[test]
    fn a_passing_error_opening_the_member_is_tried_again() {
        let d = tempfile::tempdir().unwrap();
        let r = nas(d.path());
        let home = d.path().join("app-folder/agent");
        std::fs::create_dir_all(&home).unwrap();
        // (A file where its pool folder goes: `Side::open` fails until it's gone.)
        let blocker = home.join("pool");
        std::fs::write(&blocker, b"").unwrap();
        let gone = blocker.clone();
        let clear = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            std::fs::remove_file(&gone).unwrap();
        });
        let a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(r.clone())),
            home: home.clone(),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        clear.join().unwrap();
        assert!(a.pool.is_some(), "in the pool once the error passed");
    }

    /// Task #423: a member's job out of the pool (its agent waiting to join) keeps its lease with
    /// the lead: beaten by HTTP; and ending well, it's told done with its hand-off, not failed with
    /// no reason (the lead would build it again).
    #[test]
    fn a_job_out_of_the_pool_keeps_its_lease_and_ends_done() {
        let d = tempfile::tempdir().unwrap();
        // (A root of its own, no catalog/: the NAS away as it started, the pool off.)
        let mut a = test_agent(Options {
            root: Some(store::nasfs::NasRoot::local_for_test(d.path().join("nas"))),
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: false,
        })
        .unwrap();
        assert!(a.pool.is_none() && a.member_id.is_none());
        assert_eq!(a.out_of_pool().as_deref(), Some(NAS_AWAY));
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        c.offer(
            "2026-10-18",
            vec![crate::coord::Offer {
                step: "unit".into(),
                targets: vec![("6/12/25".into(), "k".into(), 100)],
                job: None,
            }],
        );
        let cl = crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{port}")],
            c.contact.token.clone(),
            &a.host,
        );
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec!["unit".into()], mem_mb: 64_000, ..Default::default() };
        let g = cl.ask(&ask).unwrap().expect("granted");
        a.client = Some(cl);
        let dir = d.path().join("home/pool/jobs/1-7");
        std::fs::create_dir_all(&dir).unwrap();
        let kept = pool::JobKept {
            step: "unit".into(),
            targets: vec![("6/12/25".into(), "k".into())],
            lease: crate::pool::journal::LeaseId { term: 1, n: 7 },
        };
        std::fs::write(dir.join("work.json"), serde_json::to_vec(&kept).unwrap()).unwrap();
        a.slots[0].lease = Some(Held::Pooled { id: g.lease, term: 1, dir: dir.clone(), own: false });
        assert!(a.beat(0, None), "its lease renewed with the lead");
        assert!(c.held("unit").contains("6/12/25"));
        // It ends well: told done for the lead to journal (nothing is in the pool's journal from
        // here), owed while the lead isn't reached, its folder kept meanwhile.
        let cl = a.client.take();
        a.end_lease(0, Outcome::Done, &[("6/12/25".into(), "k".into())], "");
        match a.owed.as_slice() {
            [Tell::Done(d)] => {
                assert!(!d.journaled, "the lead journals it");
                assert_eq!(
                    d.handoff.as_ref().and_then(|h| h.done.clone()),
                    Some(("unit".into(), vec![("6/12/25".into(), "k".into())]))
                );
            }
            o => panic!("not told done: {}", o.len()),
        }
        assert!(dir.join("work.json").is_file(), "kept until the lead answers");
        // The lead reached: it journals the hand-off and ends the lease; the folder goes, so the
        // next process doesn't hand it off again.
        a.client = cl;
        a.tell_owed(&store::nasfs::NasRoot::local_for_test(&d.path().join("nas")));
        assert!(a.owed.is_empty() && a.unjournaled.is_empty());
        assert_eq!(crate::handoff::waiting_in(&c.journal()).unwrap().len(), 1, "journaled by the lead");
        assert!(!c.held("unit").contains("6/12/25"));
        assert!(!dir.exists(), "its folder gone");
        // A failure that says nothing still goes with a reason.
        assert!(!fail_why(Outcome::Done, "", false).is_empty());
        assert!(!fail_why(Outcome::Failed, " ", true).trim().is_empty());
        assert_eq!(fail_why(Outcome::Failed, "boom", true), "boom");
    }

    /// Task #815: a member paused (or away) through a change of lead kept its way to the old lead's
    /// coordinator, kept on there as a broker, which answers its asks with nothing and never fails
    /// a request: resumed, it got no job until the lead came back to that Mac. The answer's member
    /// isn't the term's lead: the contact is read again, and the new lead grants it work.
    #[test]
    fn a_member_asking_the_old_lead_after_a_change_of_lead_asks_the_new_one() {
        let d = tempfile::tempdir().unwrap();
        let nas = d.path().join("nas");
        let root = store::nasfs::NasRoot::local_for_test(&nas);
        std::fs::create_dir_all(nas.join("state")).unwrap();
        let mut a = test_agent(Options {
            root: Some(root.clone()),
            home: d.path().join("home"),
            bin: PathBuf::from("/app"),
            dry_run: false,
            once: true,
            member: true,
        })
        .unwrap();
        let offer = |c: &crate::coord::Coordinator| {
            c.offer(
                "2026-10-28",
                vec![crate::coord::Offer {
                    step: "unit".into(),
                    targets: vec![("6/12/25".into(), "k".into(), 100)],
                    job: None,
                }],
            )
        };
        // Term 538's lead (the mini), which this member asked before it was paused.
        let (old, old_port) = crate::coord::start_for_test(&d.path().join("old"), "mini", "");
        (old.set_term(538), old.set_member(Some("m-mini")));
        offer(&old);
        a.client = Some(crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{old_port}")],
            old.contact.token.clone(),
            &a.host,
        ));
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec!["unit".into()], mem_mb: 64_000, ..Default::default() };
        // The change of lead, this member held meanwhile: the old lead a broker, term 539's lead (the M1)
        // publishing its contact.
        old.become_broker();
        let (new, new_port) = crate::coord::start_for_test(&d.path().join("new"), "m1", "");
        (new.set_term(539), new.set_member(Some("m-m1")));
        new.publish(&root).unwrap();
        offer(&new);
        a.pool_seen = Some(PoolView {
            member: "m-m4".into(),
            role: pool::Role::Member,
            gates: pool::Gates { term: 539, ..Default::default() },
            members: Vec::new(),
            heard: 3,
            unacked: 0,
            restart: None,
            lead: Some(lead::View {
                term: 539,
                lead: Some(lead::LeadOf { term: 539, member: "m-m1".into(), host: "m1".into(), ..Default::default() }),
                ..Default::default()
            }),
        });
        // Resumed: the old lead's broker answers with nothing (what the M4 saw for hours).
        let mut w = Vec::new();
        assert!(a.client(&root, &mut w).unwrap().ask(&ask).unwrap().is_none(), "the broker grants nothing");
        // Its answer named the mini: the contact read again, the M1 asked and granting.
        let c = a.client(&root, &mut w).expect("the new lead's contact");
        assert!(c.urls().iter().any(|u| u.ends_with(&format!(":{new_port}"))), "{:?}", c.urls());
        let g = c.ask(&ask).unwrap().expect("granted by term 539's lead");
        assert!(matches!(g.work, crate::coord::Granted::Job { ref step, .. } if step == "unit"));
        assert!(new.held("unit").contains("6/12/25"));
        // Kept while the lead answers as itself.
        let urls = a.client(&root, &mut w).unwrap().urls();
        assert_eq!(a.client(&root, &mut w).unwrap().urls(), urls);
        // A view lagging the term (still naming term 538's lead): the newer lead's kept.
        if let Some(v) = a.pool_seen.as_mut().and_then(|p| p.lead.as_mut()) {
            (v.term, v.lead) = (
                538,
                Some(lead::LeadOf { term: 538, member: "m-mini".into(), host: "mini".into(), ..Default::default() }),
            );
        }
        let c = a.client(&root, &mut w).unwrap();
        c.ask(&ask).unwrap();
        let ptr = c as *const _;
        assert_eq!(a.client(&root, &mut w).unwrap() as *const _, ptr, "kept, not read again");
    }

    #[test]
    fn a_hand_off_said_journaled_is_journaled_while_the_coordinators_pool_is_off() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        c.offer(
            "2026-09-28",
            vec![crate::coord::Offer {
                step: "slope-hi".into(),
                targets: vec![("3/2/2".into(), "k".into(), 100)],
                job: None,
            }],
        );
        let cl =
            crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        let ask = crate::coord::Ask {
            kind: "native".into(),
            can: vec!["slope-hi".into()],
            mem_mb: 64_000,
            ..Default::default()
        };
        let g = cl.ask(&ask).unwrap().expect("granted");
        let h = crate::handoff::Handoff {
            changes: [(
                "layers/slope/lo/3-2-2".to_string(),
                Some("layers/slope/lo/3-2-2.0123456789abcdef.pack".to_string()),
            )]
            .into(),
            done: Some(("slope-hi".into(), vec![("3/2/2".into(), "k".into())])),
            ..Default::default()
        };
        cl.done(&crate::coord::Done { lease: g.lease, handoff: Some(h), journaled: true, ..Default::default() })
            .unwrap();
        // (Switched off with the member's job under way: its hand-off kept here, to be merged.)
        assert_eq!(crate::handoff::waiting_in(&c.journal()).unwrap().len(), 1);
    }

    fn agent_dry(root: &store::nasfs::NasRoot, home: &Path) -> Agent {
        test_agent(Options {
            root: Some(root.clone()),
            home: home.to_path_buf(),
            bin: PathBuf::from("/nonexistent/bin"),
            dry_run: true,
            once: true,
            member: false,
        })
        .unwrap()
    }
}
