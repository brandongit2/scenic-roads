//! The coordinator (docs/workers.md): the lead's agent hands work to any worker over HTTP and
//! takes the results back. One planner, many workers: the agent plans from the build's records,
//! which it alone writes; a worker only asks for work that fits it, does it, and hands it back.
//!
//! Two kinds of work, by what a worker can reach (§2):
//! - **Jobs** of the plan (crate::agent::steps::SHARED: z6 tiles, terrain, slope, tree cover,
//!   landmark candidates and peaks), for workers that mount the NAS (a member's agent, `--helper`). A
//!   worker's job saves through the coordinator: its hand-off (its manifest changes and done record,
//!   one per job) is journaled on this Mac's disk, written whole, then merged into the records by the
//!   agent (crate::handoff::merge_from), all of it or none.
//! - **Tasks** (`task`): pure work a running job offers, for any worker, a web page's included.
//!
//! Leases (`lease`) say who does what; this Mac's own jobs hold them too, so a job is never done
//! twice and a worker that goes quiet (or pauses for its conditions: it doesn't beat) gives its work
//! back; while the build is paused, every lease is held. A hand-off
//! for a lease that's gone is refused (410): its work was offered again, and a late save could put
//! an older build in the manifest. The token, the jobs' leases and what each z6 tile cost are kept on
//! this Mac's disk, so the agent restarting (a new app) is a pause to every worker, nothing more.
//!
//! Workers find it in `state/coordinator.json` on the NAS: its addresses (Tailscale's first, then
//! the LAN's) and the token the Macs' agents carry. The build page asks with no key: it reads the
//! build and takes the owner's asks (pausing, settings), never work (pages stopped helping, #875:
//! `NO_PAGE_HELPERS`). It answers this Mac, its LAN and the
//! tailnet only, also through a proxy on this Mac from those alone (crate::net::reached), a request
//! naming this Mac and from no web page elsewhere (crate::net::ours, from_the_page); a running job's
//! (`/task/…`) from this Mac itself, through no proxy (crate::net::own).

pub mod brokers;
pub mod client;
pub mod history;
pub mod inbox;
pub mod keep;
pub mod lease;
pub mod maproutes;
pub mod sched;
pub mod task;

use crate::agent::memguard::Floor;
use crate::handoff::Handoff;
use anyhow::Result;
use lease::{Lease, Leases, Work};
pub use sched::{ask_cores, outside_cores, JOB_S};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The coordinator's port.
pub const PORT: u16 = 8090;
/// The address a coordinator (or a member's broker, or viewer) listens on: every interface, for
/// the other Macs; loopback alone in a unit test, whose members are in the same process and reach
/// it at 127.0.0.1 (crate::net::urls): a freshly built test binary is no app a Mac's firewall lets
/// its Tailscale or LAN addresses reach (#596).
pub const LISTEN: &str = if cfg!(test) { "127.0.0.1" } else { "0.0.0.0" };
/// How long a lease lasts without a heartbeat (workers beat every minute).
pub const TTL: Duration = Duration::from_secs(600);
/// How long a worker counts as around after its last request.
const AROUND: Duration = Duration::from_secs(120);

/// Why lease `l` lapsed, in the lead's history (a "lapse" event's note): what it was for, and that
/// its worker said nothing for a whole `TTL` (its agent stopped or out of the pool, or this
/// coordinator out of its reach), its work offered again.
pub fn lapse_why(l: &Lease) -> String {
    format!(
        "{}: no word from {} for {} min (its agent stopped, or couldn't reach this coordinator); offered again",
        l.what(),
        l.worker,
        TTL.as_secs() / 60
    )
}
/// How long a worker counts as looking for work after its last ask (an agent's idle slot asks
/// every few seconds): a task's `takers`.
const ASKING: Duration = Duration::from_secs(30);
/// The most of what a worker says it's doing (its progress, a failure's why) kept: characters.
const WHAT_MAX: usize = 200;
const WHY_MAX: usize = 3000;
/// What a build page left open from before its helpers were removed (30 Oct, #875) is told when it
/// asks for work, beats for a task or hands one back: refused, never an error to retry on (its
/// next version, which its service worker loads, asks for none).
pub const NO_PAGE_HELPERS: &str = "pages no longer help with the build: reload this page";

/// How workers reach the coordinator: `state/coordinator.json` on the NAS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    /// Its addresses, in the order to try.
    pub urls: Vec<String>,
    pub token: String,
    /// The build page's address over HTTPS when `tailscale serve` proxies the coordinator (a Mac's
    /// menu bar opens it: its web view loads no plain HTTP but the LAN's). None otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

pub fn contact_path(root: &store::nasfs::NasRoot) -> store::nasfs::NasPath {
    root.join("state/coordinator.json")
}

/// What a job's target cost last time (a z6 tile's, another shared step's, a task's): its peak memory
/// (MB) and its wall time, the worker it was measured on (None: the lead, or not said), and
/// the way its step was run then (`cost_version`), and its inputs' measures then (crate::costin;
/// none from a job before them, and a reader before them ignores them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub peak_mb: u64,
    pub secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub v: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sizes: BTreeMap<String, u64>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_zero64(v: &u64) -> bool {
    *v == 0
}

/// The way a step runs now, as far as its memory and time go: a cost measured another way says
/// nothing of what a run takes now (terrain 2 and slope 2: each z6 tile's pack written as it's made,
/// where they held their whole z3 tile's; tree cover 2: a z6 tile a run (a z6 tile), where 1 was a z3
/// tile's whole run with the trees program, a band of a block's rows at a time on each thread, and
/// before that trees.py's workers each held a block's every zoom-12 value, 12 to 36 GB together;
/// its assemblies 1; terrain 3 and slope 3: a z6 tile a run (a z6 tile), their assemblies 1; terrain
/// 4: a z6 tile's tiles made a group of z8 subtrees at a time, where 3 held all its z12 tiles' at once,
/// 7 GB at its 90th percentile; 5: its deep levels too, z13–16 where a fine dataset is, a z8 tile's
/// blocks read ahead and kept, task #633; terrain-hi 6 and slope-hi 4: named so, their deep levels
/// their own steps', terrain-deep and slope-deep, task #639; terrain-hi 7 and terrain-lo 2: the
/// pool's base tiles sampled up to 4 chunks at once, crate::terrain_base::sampling_mb, where one at
/// a time took 0.2–0.7 GB less, task #695).
pub fn cost_version(step: &str) -> u32 {
    match crate::agent::steps::current(step) {
        "terrain-hi" => 7,
        "slope-hi" => 4,
        // (3: the canopy rows read where they lie and counted, its blocks paced, task #606: 2's
        // copied whole squares uncounted, so its NAS weight was its uploads alone.)
        "trees" => 3,
        // (A z6 tile's measure its job's processes together, as the memory guard measures a job; 0
        // was its programs' most, one at a time; 2: its processes started without malloc's large
        // cache, store::spawn::NO_LARGE_CACHE, where 1's measures counted 6–7 GB of freed blocks;
        // 3: its elevations sampled in batches, crate::dem::generic::BATCH, where 2's sampled a
        // z6 tile's vertices at once: 16 GB for the densest.)
        "unit" => 3,
        // (1: the World Heritage outlines clipped from the pass's `whs` set, not the filtered
        // planet; 0's floor, 21.9 GB, no run with its memory timed came near: 6.4 GB, the clip's 4 GB.)
        "heritage" => 1,
        // (1: the coverage asked by distance, never grown, task #493: 0's floors, 43 GB from
        // buffering the regions' outlines, no run since came near: 0.2 GB.)
        "bld-fetch" => 1,
        "terrain-lo" => 2,
        "trees-lo" | "slope-lo" => 1,
        // (2: composed only at a grid's own zoom, the coarser levels the means of the finer, and
        // the floor's shading's assembly made in depth-lo, task #804; 1: a grid lined up with no
        // overview sampled from one askew, task #433.)
        "depth" | "depth-lo" => 2,
        _ => 0,
    }
}

/// A z6 tile's predicted peak memory (MB): what it took last time, else about ten times its extract (the
/// densest measured: 6.6 GB for 668 MB), and never under 3.7 GB (the canopy step's).
pub fn unit_peak(costs: &BTreeMap<String, Cost>, unit: &str, piece: u64) -> u64 {
    costs
        .get(unit)
        .filter(|c| c.v >= cost_version("unit"))
        .map(|c| c.peak_mb)
        .unwrap_or_else(|| (piece >> 20).saturating_mul(10).max(3700))
}

/// What a task of `kind` for `unit` (its spec's) costs is kept under: "tail 6/x/y", "bldtile 8/x/y",
/// "treeblock 8/x/y" (a row's first block), "terrainsub 8/x/y" (a group's first subtree).
pub fn task_cost_key(kind: &str, unit: &str) -> String {
    format!("{kind} {unit}")
}

/// What a job of `step` for `target` costs is kept under: a z6 tile's by its target alone (as before
/// other steps were offered), another step's as "<step> <target>".
pub fn cost_key(step: &str, target: &str) -> String {
    if step == "unit" {
        target.to_string()
    } else {
        format!("{step} {target}")
    }
}

/// How long `target` of `step` is predicted to take `worker`: its last run's time (twice another
/// worker's: the members run at about half the lead's pace); None when it hasn't run the way
/// the step runs now.
pub fn job_secs(costs: &BTreeMap<String, Cost>, step: &str, target: &str, worker: &str) -> Option<u64> {
    let c = costs.get(&cost_key(step, target)).filter(|c| c.v >= cost_version(step))?;
    Some(if c.worker.as_deref() == Some(worker) { c.secs } else { c.secs * 2 })
}

/// The memory a job of `step` for `target` is predicted to take (MB), by its step's model
/// (crate::needs, its costs alone; `size` sizes its first guess, `first_guess`).
pub fn job_peak(costs: &BTreeMap<String, Cost>, step: &str, target: &str, size: u64) -> u64 {
    crate::needs::Book::new(costs).need(step, target, 0, first_guess(costs, step, target, size)).mem_mb
}

/// The memory a job of `step` for `target` is guessed to take (MB) when none of its step has been
/// measured (crate::needs' last resort): a z6 tile's by `unit_peak` (`size` its extract's bytes);
/// candidates' as the z6 tile's (they read the same extract, with one of the z6 tile's programs: `size` its
/// bytes too); another step's `size`, the estimate it was offered with.
pub fn first_guess(costs: &BTreeMap<String, Cost>, step: &str, target: &str, size: u64) -> u64 {
    match step {
        "unit" | "pois" => unit_peak(costs, target, size),
        _ => size,
    }
}

/// A step's work offered to the workers that mount the NAS: its targets in plan order (target, key,
/// size: a z6 tile's or candidates' extract bytes, another step's predicted peak memory in MB). A job
/// takes as many as fit `JOB_S` by their measured times (`pick_every`), but the lead's own planned
/// job (`job`: its id), granted whole and to the lead alone (crate::agent::every::grants).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub step: String,
    pub targets: Vec<(String, String, u64)>,
    pub job: Option<String>,
}

/// A worker, as its requests describe it.
#[derive(Clone, Debug, Serialize)]
pub struct Worker {
    /// "native": an agent (the only kind since pages stopped helping, #875).
    pub kind: String,
    pub label: String,
    /// The work it does (the shared steps it builds: crate::agent::steps::SHARED; "tail", "bldtile": tasks),
    /// the memory it spares (MB) and its cores.
    pub can: Vec<String>,
    pub mem_mb: u64,
    pub cores: u32,
    /// The app it runs (an agent's: crate::agent::app_version), when it says.
    pub app: Option<String>,
    /// Its Mac's capacity, as its last ask said (an older agent's: none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap: Option<Capacity>,
    /// Its last ask, for why it gets no work (`/work/swarm`).
    #[serde(skip)]
    pub ask: Option<Ask>,
    /// Its last ask said no capacity (`Ask::cap`): an agent on an older app (`old_member`). Never
    /// the lead's own agent, whose capacity is kept apart (`Coordinator::ask_own`).
    #[serde(skip)]
    pub old: bool,
    #[serde(skip)]
    pub seen: Instant,
    /// When it last asked for work.
    #[serde(skip)]
    pub asked: Option<Instant>,
    pub what: String,
    pub done: u32,
    pub failed: u32,
}

/// What the request handlers and the agent share.
#[derive(Debug)]
pub struct Shared {
    /// The scheduler's state (`sched::Sched`): what every grant is decided from, by its rules alone.
    pub sched: sched::Sched,
    /// The pass, and the plan's work a worker that mounts the NAS may do, a step at a time in plan
    /// order. A worker's come from the far end; this Mac's own jobs take the near end.
    pub pass: String,
    /// The number of the lead's plan the offers came from (crate::agent::plan::PlanView::n): each
    /// grant carries it (`Grant::plan`).
    pub plan: u64,
    /// A resumable job offered to the one worker whose Mac holds its progress (its name, and the
    /// job as an offer: its step, its target and key, `offer_resume`; crate::agent::resume), to
    /// no other, whatever pass is offered.
    pub resume: Option<(String, Offer)>,
    pub tasks: task::Tasks,
    /// The build's pause (crate::control), while it's paused: no work is given, a worker's beat is
    /// told (its job pauses too), and no lease lapses. Kept in `pause.json`, so a restart keeps it.
    pub paused: Option<crate::control::Pause>,
    /// When the pause last changed, as the ask said (unix seconds): an ask older than that (a Mac's
    /// held while it couldn't reach this one) is passed over.
    pub pause_at: u64,
    /// What happened, the last week's (`history`): the worker page's activity.
    pub history: history::History,
    /// The pool's lead (docs/pool.md §6.6): why it grants nothing now (its view not fresh, no longer
    /// leading), workers told to ask again in a moment.
    pub moving: Option<String>,
    /// When the pool's agent last said whether it grants (`set_moving`; this Mac's wall clock, unix
    /// s): None out of the pool. Not told for `pool::driver::STALL_S` (its process held up, or its
    /// Mac asleep and just woken, its step not run yet), it grants nothing, as if moving: its view
    /// may be old, and its next step re-asserts before it grants again.
    pub told: Option<u64>,
    /// The app the newer of the readers' switches was recorded on (the regions', the elevation
    /// datasets': crate::inputs::regions::read::switch_app), as the lead's agent last read it: no
    /// work for an older app, whatever else it matches.
    pub min_app: Option<String>,
    /// The pool's member this coordinator is (its agent's), while it leads: every answer says so,
    /// with its term (`POOL_MEMBER`, `POOL_TERM`), so a member tells its lead's coordinator from
    /// another's (its reach, crate::pool::reachable). None while its Mac has lost the NAS.
    pub pool_member: Option<String>,
    /// Where the members' messages to this Mac's member go (`/pool/mail`, `inbox`): its agent's
    /// (`Coordinator::set_inbox`); None, refused (the member sends by mail).
    pub inbox: Option<std::sync::Arc<inbox::Inbox>>,
    /// The build page's acceptances of the gate's warnings (`/work/inputs`), for the agent to write
    /// (crate::inputs::apply_ask).
    pub inputs_asks: Vec<crate::inputs::Ask>,
    /// The owner's asks to build targets again (`/work/redo`: `scenic redo`), for the agent to keep
    /// and run (crate::agent::redo).
    pub redo_asks: Vec<crate::agent::redo::Ask>,
    /// The lead's own Mac's capacity (`Coordinator::set_capacity`), as its members' asks carry theirs.
    pub own_cap: Option<Capacity>,
    /// The probes failed in a row, the last first (0: the last landed): at `NAS_FAILED_RUN` or more
    /// the lead's slots' ramp holds and its stretch starts again (crate::agent's `tend_ramp`); one
    /// alone (a hiccup every few hours, probes being half an hour apart) is neither.
    pub nas_failures: u32,
    /// The gate on the NAS's own disk load (crate::budget::nas::Loop): held, `nas` and `nas_own`
    /// say so (`Limits::held`); read by the lead's waiting lines (`Coordinator::nas_held`).
    pub nas_loop: crate::budget::nas::Loop,
    /// The probes the lead's prober keeps (`nas_prober`'s, as read from and written to
    /// crate::budget::nas::PROBES_FILE), and the disks' busy shares it read last (%): the status's.
    pub nas_probes: Vec<crate::budget::nas::Probe>,
    pub nas_disks: Vec<f64>,
    /// The NAS's mount as the prober last read it (`NasMount`), each `MOUNT_EVERY_S`.
    pub nas_mount: Option<NasMount>,
    /// Why each worker's last ask was granted fewer jobs than it wanted (`pack`: the packing's
    /// refusal, in words), and when (unix s); none once an ask got all it wanted. The status's
    /// (`Coordinator::refusals`): why nothing more fits a Mac.
    pub refused: BTreeMap<String, (u64, String)>,
    /// Tasks brokered by every member (docs/pool.md §8; crate::agent::brokers): the members and
    /// their brokers, as the agent last read their heartbeats. None: the switch off, the
    /// coordinator as it was (no page from elsewhere, a take-up's leases in place
    /// of its own).
    pub brokers: Option<brokers::Brokers>,
    /// Where the token, leases, costs and journal are kept.
    dir: PathBuf,
    /// The time its leases, workers' paces and their being around are reckoned in (a test's
    /// virtual one).
    pub clock: std::sync::Arc<dyn store::clock::Clock>,
    /// The page's data as this Mac reads it from the NAS (crate::agent::pageview): `/work/swarm`
    /// and `/work/history` answered from it once it has a snapshot.
    pub pageview: Option<Arc<crate::agent::pageview::PageView>>,
    /// This Mac's live store (crate::agent::live): `/work/live`. None: not given one (503).
    pub live: Option<Arc<crate::agent::live::Store>>,
    /// The build page's Catalogues map (crate::agent::cataloguesview): `/work/catalogs`. None: not
    /// given one (503).
    pub catalogues: Option<Arc<crate::agent::cataloguesview::CataloguesView>>,
    /// The map and its log's batches (store::map::feed, the lead's log writer's): `/map/changes`,
    /// `/map/snapshot` (maproutes). None: not the lead (503).
    pub maplog: Option<Arc<store::map::feed::Feed>>,
    /// The build page's map's finished tiles (crate::agent::builtview): `/work/built`. None: not
    /// given one (503).
    pub built: Option<Arc<crate::agent::builtview::BuiltView>>,
    /// The build page's regions as this Mac's agent last made them (crate::agent::regionsview):
    /// `/work/regions` out of the pool. None: not made yet (`making`).
    pub regions: Option<Arc<serde_json::Value>>,
    /// Each region's targets built of all by step (crate::agent::build::region_steps), as this
    /// Mac's agent last counted them (`{at, regions: {id: {step: [built, all]}}}`), and when a page
    /// last asked: the lead (or a Mac out of the pool) counts them only while asked
    /// (`REGION_STEPS_ASKING`). `/work/regions` with `steps`.
    pub region_steps: Option<Arc<serde_json::Value>>,
    pub region_steps_asked: Option<Instant>,
    /// Each Mac's tiles to come in the lead's last forecast (crate::agent::forecast::ToCome):
    /// `/work/tocome`. None: this Mac made none (a member's: the page asks the lead).
    pub tocome: Option<Arc<crate::agent::forecast::ToCome>>,
    /// A viewer alone (`Coordinator::viewer`): the page and its views, nothing else (but this
    /// Mac's own page's asks, passed to its agent).
    pub viewer: bool,
    /// Not the lead's (a member's broker, a viewer): a page's pause is this Mac's agent's to pass
    /// on (crate::control::request in its folder, `set_forward`), not this one's to set.
    pub forward: Option<PathBuf>,
    /// This Mac's agent's folder and the NAS's project folder (None: away), where the owner's
    /// settings lie (`/work/settings`, crate::agent::settings; `set_settings`). None: not said yet.
    pub settings: Option<(PathBuf, Option<store::nasfs::NasRoot>)>,
    /// The build pages' asks to cancel a job or try a target again (`/work/act`), for this Mac's
    /// agent to act on, leading, or to pass on to the lead (crate::pool::driver::Act).
    pub page_acts: Vec<crate::pool::driver::Act>,
    /// Not the lead's: why a page's retry can't be passed on to the lead now (none leads, or it runs
    /// another app: crate::pool::driver's `Act`), as its agent last saw; None: it can.
    pub retry_blocked: Option<String>,
    /// Its state's writer to the NAS, while it leads (`keep`, task #641): told of each change saved.
    pub keep: Arc<keep::Keep>,
}

/// How long a target waits on the NAS budget before the status says so (s).
pub const NAS_WAIT_SAY_S: u64 = 30 * 60;

impl Shared {
    /// Each member's fresh capacity (`Coordinator::capacities`): `me`'s own first.
    fn capacities(&self, me: &str) -> Vec<(String, Capacity)> {
        let now = self.now();
        // (A capacity is refreshed only by an ask: a busy member's, older than `CAP_FRESH_S`, left out.)
        let fresh = |c: &Capacity| unix_now().saturating_sub(c.at) < CAP_FRESH_S;
        let own = self.own_cap.clone().filter(fresh).map(|c| (me.to_string(), c));
        let others = self
            .workers
            .iter()
            .filter(|(_, w)| now.saturating_duration_since(w.seen) < AROUND)
            .filter_map(|(n, w)| Some((n.clone(), w.cap.clone().filter(fresh)?)));
        own.into_iter().chain(others).collect()
    }

    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    fn save_leases(&self) {
        self.keep.changed();
        if let Err(e) = self.leases.save(&self.dir.join("leases.json")) {
            eprintln!("coordinator: saving the leases: {e:#}");
        }
    }

    fn save_costs(&self) {
        self.keep.changed();
        let r = serde_json::to_vec(&self.costs)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.dir.join("costs.json"), &b));
        if let Err(e) = r {
            eprintln!("coordinator: saving the costs: {e:#}");
        }
        let r = serde_json::to_vec(&self.floors)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.dir.join("floors.json"), &b));
        if let Err(e) = r {
            eprintln!("coordinator: saving the floors: {e:#}");
        }
        let r = serde_json::to_vec(&self.disks)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.dir.join("disks.json"), &b));
        if let Err(e) = r {
            eprintln!("coordinator: saving the disk floors: {e:#}");
        }
        let r = serde_json::to_vec(&self.fills)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.dir.join(FILLS), &b));
        if let Err(e) = r {
            eprintln!("coordinator: saving the caches' fills: {e:#}");
        }
        let r = serde_json::to_vec(&self.job_disks)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&self.dir.join(JOB_DISKS), &b));
        if let Err(e) = r {
            eprintln!("coordinator: saving the jobs' disk: {e:#}");
        }
    }

    /// Disk floors seen (by cost key, MB): each kept where it's more than the one kept. One of the
    /// OSM pass (`osm-pass <date>`) puts the earlier passes' away. True when any changed.
    fn add_disks(&mut self, disks: &[(String, u64)]) -> bool {
        self.needs_rev += 1;
        let mut any = false;
        for (u, mb) in disks {
            // (A target measured at 0 MB keeps its 0: its own measure, not its step's prior.)
            if self.disks.get(u).is_some_and(|old| old >= mb) {
                continue;
            }
            if u.starts_with("osm-pass ") {
                self.disks.retain(|k, _| !k.starts_with("osm-pass ") || k == u);
            }
            self.disks.insert(u.clone(), *mb);
            any = true;
        }
        any
    }

    /// The disk a job of `step` took over its run (MB): one of its step's last `JOB_PEAKS`.
    fn add_job_disk(&mut self, step: &str, mb: u64) -> bool {
        if step.is_empty() || mb == 0 {
            return false;
        }
        let v = self.job_disks.entry(step.to_string()).or_default();
        v.push(mb);
        let n = v.len().saturating_sub(JOB_PEAKS);
        v.drain(..n);
        true
    }

    /// A job of `step` over `targets` took `mb` on the disk over its run: when that's past its
    /// targets' own output by `FILL_MIN` or more, and at least half the fill kept, it filled its
    /// shared cache (crate::needs::CACHES), the excess that cache's fill (a warm run's leaves it, a
    /// part refilled after an eviction too). True when it changed.
    fn note_fill(&mut self, step: &str, targets: &[String], mb: u64) -> bool {
        let Some(cache) = crate::needs::cache_of(step) else { return false };
        let own = targets.iter().map(|t| self.measured(step, t).map_or(0, |m| m.disk_mb));
        let extra = mb.saturating_sub(crate::needs::batch_disk(step, own));
        if extra < FILL_MIN || extra.saturating_mul(2) < self.fill_mb(cache) {
            return false;
        }
        self.fills.insert(cache.to_string(), extra);
        true
    }

    /// Costs measured (by `worker`): each in place of what was kept, and of its target's floor.
    fn add_costs(&mut self, costs: impl IntoIterator<Item = (String, Cost)>, worker: &str) {
        self.needs_rev += 1;
        for (u, c) in costs {
            self.floors.remove(&u);
            self.costs.insert(u, Cost { worker: Some(worker.to_string()), ..c });
        }
    }

    /// Floors seen (by cost key): each kept where its target has none, one of an older way of its
    /// step, or a lower one; or where it was learned alone and the one kept wasn't (a batch's floor
    /// may be its earlier targets' caches). A floor of the OSM pass (`osm-pass <date>`) puts the
    /// earlier passes' away. True when any changed.
    fn add_floors(&mut self, floors: &[(String, Floor)]) -> bool {
        self.needs_rev += 1;
        let mut any = false;
        for (u, f) in floors {
            let take = match self.floors.get(u) {
                None => true,
                Some(old) if old.v != f.v => f.v > old.v,
                Some(old) => f.mb > old.mb || (f.alone && !old.alone),
            };
            if take {
                if u.starts_with("osm-pass ") {
                    self.floors.retain(|k, _| !k.starts_with("osm-pass ") || k == u);
                }
                self.floors.insert(u.clone(), *f);
                any = true;
            }
        }
        any
    }

    /// The jobs `a`'s worker is granted: the scheduler's decision (`sched::Sched::pack`, on this
    /// coordinator's clock), then what a grant does outside it: the leases saved, each grant logged
    /// and in the history, the refusal kept for the status (`refused`). None: no job.
    fn pack(&mut self, a: &Ask, offers: &[Offer], lead: bool, now: Instant) -> Option<Grant> {
        let (picks, why) = self.sched.pack(a, offers, lead, sched::Clock { now, unix: unix_now() });
        match why {
            Some(w) => self.refused.insert(a.worker.clone(), (unix_now(), w)),
            None => self.refused.remove(&a.worker),
        };
        if !picks.is_empty() {
            self.save_leases();
        }
        let mut grants = picks.into_iter().map(|p| {
            eprintln!(
                "coordinator: {} took {} {} ({} threads)",
                a.worker,
                p.step,
                p.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" "),
                p.threads
            );
            self.history.add(history::Event {
                worker: Some(a.worker.clone()),
                lease: Some(p.lease),
                step: Some(p.step.clone()),
                targets: p.targets.iter().map(|t| t.0.clone()).collect(),
                ..history::Event::new("lease")
            });
            Grant {
                lease: p.lease,
                term: self.sched.leases.term,
                ttl_s: TTL.as_secs(),
                work: Granted::Job { step: p.step, targets: p.targets, pass: self.pass.clone() },
                more: Vec::new(),
                threads: p.threads,
                disk_mb: p.disk_mb,
                job: p.job,
                plan: self.plan,
            }
        });
        let mut first = grants.next()?;
        first.more = grants.collect();
        Some(first)
    }

    /// Placement on (crate::agent::placement): the task to give `a`'s worker, as `Tasks::pick` but
    /// a task of a kind it's measured slower at given while its head start covers it
    /// (`Tasks::covered`), and each task drawn among the workers that could take it now
    /// (`takers`), weighted by their measured speed at its kind (one over their pace; one not
    /// measured, this Mac's), passed by when drawn to another.
    fn pick_task_placed(&self, a: &Ask, now: Instant) -> Option<u64> {
        use crate::agent::placement;
        let (_, since) = self.placement.as_ref()?;
        let window = now.saturating_duration_since(*since).as_secs();
        let mut ids: Vec<&task::Task> =
            self.tasks.by_id.values().filter(|t| self.tasks.may_take(t, &a.worker, &a.can, a.mem_mb, now)).collect();
        ids.sort_by_key(|t| (t.offered, t.id));
        // (The others around and asking, once an ask: each task then by its kind and memory.)
        // (Each by its own last ask, as `placed_start`'s: an agent on another app than this one's
        // gets nothing.)
        let around: Vec<(&String, &Ask)> = self
            .workers
            .iter()
            .filter(|(n, w)| {
                *n != &a.worker
                    && now.duration_since(w.seen) < AROUND
                    && w.asked.is_some_and(|at| now.duration_since(at) < ASKING)
            })
            .filter_map(|(n, w)| {
                w.ask.as_ref().filter(|x| x.app.as_ref().is_none_or(|v| *v == self.app)).map(|x| (n, x))
            })
            .collect();
        ids.into_iter()
            .find(|t| {
                let mut who: Vec<(String, f64)> = around
                    .iter()
                    .filter(|(n, x)| self.tasks.may_take(t, n, &x.can, x.mem_mb, now))
                    .map(|(n, _)| {
                        let n = n.to_string();
                        let w = self.tasks.pace(&n, &t.kind).map_or(1.0, |p| 1.0 / p.max(1e-6));
                        (n, w)
                    })
                    .collect();
                who.push((a.worker.clone(), self.tasks.pace(&a.worker, &t.kind).map_or(1.0, |p| 1.0 / p.max(1e-6))));
                placement::draw(&placement::key(&format!("task {}", t.id), window), &who) == Some(a.worker.as_str())
            })
            .map(|t| t.id)
    }

    /// Why a worker gets the plan's work it does, or doesn't: for each step it does that's offered
    /// to it, the targets offered, then those held by another, done, kept from it after it failed
    /// them, too large for the memory it spares, and those left that it may take, counted.
    fn fit(&self, a: &Ask, now: Instant) -> Vec<serde_json::Value> {
        self.offers
            .iter()
            .filter(|o| a.can.contains(&o.step) && o.job.is_none())
            .map(|o| {
                let held = self.leases.held(&o.step, now);
                let (mut h, mut d, mut b, mut big, mut fits) = (0, 0, 0, 0, 0);
                for (t, k, size) in &o.targets {
                    let backoff = self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))).is_some_and(|(at, n)| now.duration_since(*at) < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5)));
                    match () {
                        _ if held.contains(t) => h += 1,
                        _ if self.done.get(&(o.step.clone(), t.clone())) == Some(k) => d += 1,
                        _ if backoff => b += 1,
                        _ if self.peak_of(&o.step, t, *size, a.cores) <= a.mem_mb => fits += 1,
                        _ => big += 1,
                    }
                }
                serde_json::json!({ "step": o.step, "offered": o.targets.len(), "held": h, "done": d, "kept_from": b, "too_big": big, "fits": fits })
            })
            .collect()
    }

    /// Marks `worker`'s request (what it said, and itself as `a` describes it). A worker first
    /// heard from: the history says so, and those not heard from for a day, holding nothing, go.
    /// How many workers could take task `t` now: around, asking for work lately (`ASKING`), doing
    /// its kind, sparing its memory, and not ones it failed on.
    /// Each with its pace at the kind (None: not measured) and whether a job may wait for it to take
    /// one while its pace isn't measured (once an hour: task::EXPLORE_EVERY).
    fn takers(&self, t: &task::Task, now: Instant) -> Vec<serde_json::Value> {
        let fits = |(n, w): &(&String, &Worker)| {
            now.duration_since(w.seen) < AROUND
                && w.asked.is_some_and(|a| now.duration_since(a) < ASKING)
                && w.can.contains(&t.kind)
                && w.mem_mb >= t.mem_mb
                && !t.failed_on.contains(*n)
        };
        self.workers
            .iter()
            .filter(fits)
            .map(|(n, _)| {
                let pace = self.tasks.pace(n, &t.kind);
                serde_json::json!({ "worker": n, "pace": pace, "explore": pace.is_none() && self.tasks.may_explore(n, now) })
            })
            .collect()
    }

    fn seen(&mut self, worker: &str, what: String, a: Option<&Ask>, now: Instant) {
        if !self.workers.contains_key(worker) {
            let note = a.and_then(|a| a.label.as_deref()).map(|l| l.chars().take(80).collect()).unwrap_or_default();
            self.history.add(history::Event {
                worker: Some(worker.to_string()),
                note,
                ..history::Event::new("worker")
            });
            let holding: BTreeSet<String> = self.leases.all(now).iter().map(|l| l.worker.clone()).collect();
            self.workers.retain(|n, w| now.duration_since(w.seen) < Duration::from_secs(86400) || holding.contains(n));
        }
        let w = self.workers.entry(worker.to_string()).or_insert_with(|| Worker {
            kind: String::new(),
            label: worker.to_string(),
            can: Vec::new(),
            mem_mb: 0,
            cores: 0,
            app: None,
            cap: None,
            ask: None,
            old: false,
            seen: now,
            asked: None,
            what: String::new(),
            done: 0,
            failed: 0,
        });
        w.seen = now;
        w.what = what.chars().take(WHAT_MAX).collect();
        if let Some(a) = a {
            (w.kind, w.can, w.mem_mb, w.cores, w.app) =
                (a.kind.clone(), a.can.clone(), a.mem_mb, a.cores, a.app.clone());
            w.cap = a.cap.clone();
            w.old = a.cap.is_none();
            w.ask = Some(a.clone());
            w.asked = Some(now);
            if let Some(l) = &a.label {
                w.label = l.chars().take(80).collect();
            }
        }
    }
}

// (The scheduler's state read and changed through the coordinator's as its own fields: the
// request handlers keep the state it's decided from up to date; `sched::Sched::pack` decides.)
impl std::ops::Deref for Shared {
    type Target = sched::Sched;
    fn deref(&self) -> &sched::Sched {
        &self.sched
    }
}

impl std::ops::DerefMut for Shared {
    fn deref_mut(&mut self) -> &mut sched::Sched {
        &mut self.sched
    }
}

/// The headers every answer of a pool's lead's coordinator carries: its term and its member.
pub const POOL_TERM: &str = "x-pool-term";
pub const POOL_MEMBER: &str = "x-pool-member";

pub struct Coordinator {
    pub shared: Arc<Mutex<Shared>>,
    pub contact: Contact,
    /// What this agent's jobs carry to offer tasks (`/task/…`), never published.
    pub job_token: String,
    port: u16,
    /// This Mac's name: its own jobs' leases are held under it.
    me: String,
    /// The NAS's project folder while the agent has it, for its NAS probe (`nas_prober`).
    root: Arc<Mutex<Option<store::nasfs::NasRoot>>>,
    /// Set when it stops (`stop`, or dropped): its listener woken (`wake`) and closed, its port free
    /// once `stop` returns (a member that took the lead in its process starts one; one that lost it
    /// stops it): `closed` hears from its thread as its listener is gone.
    stopped: Arc<std::sync::atomic::AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    closed: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// The requests' `route` calls under way (each holds it read) and whether it has stopped: `stop`
    /// takes it to write, so it returns only once none runs, and none starts after (`http::json`):
    /// no request writes its state (leases, costs, …) into its folder once `stop` has returned.
    routes: Arc<std::sync::RwLock<bool>>,
    /// The origins whose pages may call it from elsewhere (CORS: the members' addresses, while
    /// tasks are brokered by every member; None: none, as before).
    cors: Arc<std::sync::RwLock<Option<BTreeSet<String>>>>,
    /// Each step's phase shares over its last runs (crate::timings::PhaseShares), kept by their
    /// own thread (`phase_sharer`), apart from the pool's lock.
    phase_shares: Arc<Mutex<BTreeMap<String, crate::timings::PhaseShares>>>,
    /// Each step's time by phase over the page's chart ranges (crate::timings::PhaseTimes), kept
    /// by the same thread.
    phase_times: Arc<Mutex<Option<Arc<crate::timings::PhaseTimes>>>>,
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A request for work.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub worker: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The kinds of work it does: a shared step's jobs (crate::agent::steps::SHARED: it mounts
    /// the NAS), "tail", "bldtile", "treeblock" or "terrainsub" (tasks: a z6 tile's tail, a 3D
    /// buildings' z8 tile, a row of tree cover's z8 blocks, a terrain z6 tile's z8 subtrees).
    pub can: Vec<String>,
    /// The memory it spares now (MB).
    pub mem_mb: u64,
    #[serde(default)]
    pub cores: u32,
    /// The app it runs (an agent's, crate::agent::app_version).
    #[serde(default)]
    pub app: Option<String>,
    /// The most memory its Mac's jobs may hold together (MB: crate::agent::memguard::jobs_limit_mb), for
    /// the lead's reckoning of the most any Mac in the pool has (an older worker says none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_mb: Option<u64>,
    /// The disk it has room for a job on (MB: its free space and what its caches can free, less
    /// the owner's room target), measured as it asks: it's offered only targets
    /// whose measured disk fits (`pick_every`). An older worker says none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mb: Option<u64>,
    /// Its whole Mac's capacity now (Ask v2, `Capacity`); the fields above still filled for an
    /// older coordinator, which ignores this. None: an older agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<Capacity>,
    /// The jobs it would start now (its free slots). The lead grants up to that many at once
    /// (`Grant::more`); an older agent's ask (none), one.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub want: usize,
}

/// The memory limit `a`'s Mac says (MB): its capacity's, else the old field (`Ask::limit_mb`, an
/// older agent's, or a newer one's while the pool isn't all on the new app); None when neither says.
pub fn ask_limit(a: &Ask) -> Option<u64> {
    a.cap.as_ref().map(|c| c.limit_mb).filter(|&l| l > 0).or(a.limit_mb)
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

/// A Mac's disk in words (task #391): its free space, what's left of it for jobs (`for_jobs_mb`:
/// the free space less the owner's room target and its jobs' reservations), and what keeps the
/// rest: "38 GB free · 18 GB for jobs (20 GB room target)"; nothing kept, no parentheses.
pub fn disk_words(c: &Capacity, for_jobs_mb: u64) -> String {
    let gb = |mb: u64| format!("{} GB", (mb as f64 / 1024.0).round() as u64);
    let mut kept = Vec::new();
    if c.target_mb > 0 {
        kept.push(format!("{} room target", gb(c.target_mb)));
    }
    if c.reserved_mb > 0 {
        kept.push(format!("{} reserved by its jobs", gb(c.reserved_mb)));
    }
    let kept = if kept.is_empty() { String::new() } else { format!(" ({})", kept.join(", ")) };
    format!("{} free · {} for jobs{kept}", gb(c.free_mb), gb(for_jobs_mb))
}

/// How long a capacity stands (s): older, `Coordinator::capacities` leaves it out.
pub const CAP_FRESH_S: u64 = 120;

/// A Mac's capacity as its agent measured it (docs/pool.md §7.1): its whole Mac's, where an ask's
/// `mem_mb`, `cores` and `disk_mb` are one job's. Carried by each ask (a member's) and set each
/// loop by the lead for its own Mac (`Coordinator::set_capacity`); the coordinator keeps the last
/// per member (`capacities`). Every field defaults: an older or newer agent's capacity parses.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capacity {
    /// When it was measured (unix s).
    pub at: u64,
    /// The memory guard's limit: the most its jobs may hold together (MB: memguard::jobs_limit_mb, the agent's own footprint off).
    pub limit_mb: u64,
    /// What its running jobs hold now, as the guard's sampler last saw them (MB).
    pub held_mb: u64,
    /// The memory its owner gives the build's jobs together (MB: `scenic memory`, within the
    /// limit; crate::agent::buildmem::room_mb). 0: an older app's, which doesn't say.
    pub build_mb: u64,
    /// The build's memory (`build_mb`) less each running job's predicted need (the larger of
    /// what it holds and its predicted peak: its targets' measured peak on the lead, the
    /// step's first guess on a member, which has no coordinator's measures) (MB).
    pub mem_mb: u64,
    /// Its cores, and those free by the last minute's load average.
    pub cores: u32,
    pub cores_free: u32,
    /// The disk's free space less the owner's room target and the running jobs' reservations (MB).
    pub disk_mb: u64,
    /// The disk's free space (MB) and the owner's room target (0: none): what `disk_mb` is taken
    /// from (`disk_words`). 0 both: an older app's. (An older app's `reserve_mb`, a fixed reserve
    /// it took off too, is passed over as it parses: its `disk_mb` has it taken off already.)
    pub free_mb: u64,
    pub target_mb: u64,
    /// The disk its jobs have reserved and not taken yet (MB): each granted or running job's
    /// measured disk less what it has written so far (docs/pool.md §7.1).
    pub reserved_mb: u64,
    /// Its owner used it within the last five minutes (cond::AWAY_S).
    pub in_use: bool,
    /// The cores its in-use share keeps free of its jobs while its owner uses it
    /// (crate::agent::share::cores): left out of its grants' threads while `in_use`.
    pub kept_cores: u32,
    /// On mains power, and the battery's charge when it has one.
    pub ac: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery: Option<u8>,
    /// The NAS mounted and answering, and by its LAN name (at home).
    pub nas: bool,
    pub home: bool,
    /// Whether the memory guard stops jobs past the limit.
    pub guard: bool,
    /// The leases of its running jobs.
    pub leases: Vec<u64>,
    /// Those of its running jobs that only wait on a worker (crate::offload's WAITING_ENV), each
    /// with the memory it holds now: packed at that and none of their cores (task #462).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<WaitingJob>,
    /// The leases of its running jobs that left its cores idle (under a core-minute of CPU in the
    /// last `crate::agent::IDLE_CPU_S`: a job waiting on the network): packed at their measures but
    /// none of their cores, so CPU work goes beside them (task #665).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub idle: Vec<u64>,
    /// Its running jobs an agent of an older app started and this one took over
    /// (crate::agent::jobs::adopt), running that app's programs still: each one's job and app
    /// (task #688: terrain jobs on a release before #665's rolling fetch crawled over GSI's tiles
    /// for hours after the update). Said in the lead's status (`old_app_notices`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub old_app: Vec<OldAppJob>,
    /// What it holds of each shared cache (crate::needs::CACHES, by name; task #392), measured on
    /// its disk as it asks: the first job reading one is reserved its fill less that.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub caches: BTreeMap<String, CacheHere>,
    /// Its jobs' NAS traffic a second now (crate::agent::live::NAS_TRAFFIC's order and units, its
    /// live store's newest): the lead sums the members' (`Coordinator::nas_view`). None: not measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nas_traffic: Option<[f64; 7]>,
    /// Its NAS calls now (crate::agent::live::Store::calls_now: in flight, missed a second, its
    /// threads): the lead sums the members' into its live store (task #634). None: not measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nas_calls: Option<[f64; 3]>,
}

/// A job of a Mac's on an older app than its agent's (`Capacity::old_app`): its id and that app.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldAppJob {
    pub job: String,
    pub app: String,
}

/// A job of a Mac's that only waits on a worker (`Capacity::waiting`): its lease, and what it holds
/// now (MB, its memory guard's last sample).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingJob {
    pub lease: u64,
    pub held_mb: u64,
}

/// The NAS as the lead sees it, for the status (`Coordinator::nas_view`): the day's probes kept
/// (crate::budget::nas::PROBES_FILE's), the limits they give the lead's own jobs, the load gate's
/// reading (the busiest disk smoothed, its target, held) and the disks' last busy shares, and the
/// pool's NAS traffic, the fresh members' summed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NasView {
    pub probes: Vec<crate::budget::nas::Probe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<crate::budget::nas::Limits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_pct: Option<f64>,
    pub target_pct: u32,
    pub held: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disks_pct: Vec<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic: Option<[f64; 7]>,
    /// The NAS's mount (the project folder's filesystem), as the prober last read it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<NasMount>,
}

/// The filesystem holding the NAS's project folder (store::sys::mount_of): the server's name as
/// mounted (`//user@fishandchips.local/personal` gives `fishandchips`), its protocol (`SMB`, `NFS`,
/// `AFP`, else the filesystem's kind) and the bytes free on the share.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NasMount {
    pub name: String,
    pub proto: String,
    pub free: u64,
}

/// How often the prober reads the NAS's mount (`NasMount`): its free space moves slowly.
const MOUNT_EVERY_S: u64 = 60;

impl NasMount {
    /// From statfs's mounted-from name and filesystem kind.
    pub fn of(from: &str, kind: &str, free: u64) -> NasMount {
        let host = from.trim_start_matches('/');
        let host = host.split('/').next().unwrap_or(host);
        let host = host.rsplit('@').next().unwrap_or(host);
        // (`host:/path` for NFS; a port or `.local` dropped.)
        let host = host.split(':').next().unwrap_or(host);
        let name = host.strip_suffix(".local").unwrap_or(host).to_string();
        let proto = match kind {
            "smbfs" => "SMB",
            "nfs" => "NFS",
            "afpfs" => "AFP",
            k => k,
        }
        .to_string();
        NasMount { name, proto, free }
    }
}

/// What a Mac holds of a shared cache (`Capacity::caches`): its files' bytes on its disk (MB).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheHere {
    pub have_mb: u64,
}

/// The pool's NAS traffic (crate::agent::live::NAS_TRAFFIC's order): each member's with a
/// measure, summed. None while none has one.
pub fn pool_traffic(caps: &[(String, Capacity)]) -> Option<[f64; 7]> {
    caps.iter().filter_map(|(_, c)| c.nas_traffic).reduce(|a, b| std::array::from_fn(|i| a[i] + b[i]))
}

/// The pool's NAS calls (crate::agent::live::POOL_CALLS' order): each member's with a measure,
/// summed. None while none has one.
pub fn pool_calls(caps: &[(String, Capacity)]) -> Option<[f64; 3]> {
    caps.iter().filter_map(|(_, c)| c.nas_calls).reduce(|a, b| std::array::from_fn(|i| a[i] + b[i]))
}

/// The NAS probes failed in a row from which they count as failing (`Coordinator::nas_failing`).
pub const NAS_FAILED_RUN: u32 = 2;

/// Work granted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grant {
    pub lease: u64,
    /// The pool's term it's granted in (0: the pool off): its id there is `<term>-<lease>`.
    #[serde(default, skip_serializing_if = "is_zero64")]
    pub term: u64,
    pub ttl_s: u64,
    #[serde(flatten)]
    pub work: Granted,
    /// More jobs granted with this one, the ask wanting more than one (`Ask::want`): each its own
    /// lease, packed into the ask's room beside the others.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<Grant>,
    /// The worker threads its job runs with (RAYON_NUM_THREADS): its measured cores within those
    /// its Mac has left (`job_threads`). 0: not said (an older lead's), the agent's own rule.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub threads: u32,
    /// The disk its job is measured to take (MB: `Coordinator::measured_batch`'s), which its agent
    /// reserves until it hands the job off or gives it back (docs/pool.md §7.1). 0: a cold start,
    /// a task, or an older lead's.
    #[serde(default, skip_serializing_if = "is_zero64")]
    pub disk_mb: u64,
    /// The lead's own planned job granted (`Offer::job`: its id), to the lead's own agent alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    /// The number of the lead's plan its work was offered by (`Shared::plan`): a cancel taken after
    /// that plan drops it (docs/agent-parts.md §5.4). 0: an older lead's, or no plan.
    #[serde(default, skip_serializing_if = "is_zero64")]
    pub plan: u64,
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
    /// How far it is (0–1), when it says.
    #[serde(default)]
    pub frac: Option<f64>,
    /// What its job is doing now (crate::agent::jobs::Live), when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<crate::agent::jobs::Live>,
    /// The slot of its Mac running the job (crate::coord::lease::Lease::slot), when it says (an
    /// older app doesn't).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
}

/// Work done: a job's one hand-off (its saves and done record) and what its z6 tiles cost, or a task's
/// uploaded outputs, time and peak memory.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    /// What its targets took at least, as it saw them (crate::agent::memguard: cost key, floor).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floors: Vec<(String, Floor)>,
    /// The disk its targets took at least, as it saw them (cost key, MB), and the most it took over
    /// its whole run (MB).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<(String, u64)>,
    #[serde(default, skip_serializing_if = "is_zero64")]
    pub job_disk_mb: u64,
    #[serde(default)]
    pub outputs: Vec<task::Output>,
    /// Inputs a task's programs removed.
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub secs: f64,
    #[serde(default)]
    pub peak_mb: u64,
    /// A job's hand-off its member wrote to the pool's journal itself (docs/pool.md §7.3): its lease
    /// ends and its targets are kept out of offers, but nothing is journaled here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub journaled: bool,
    /// Its run's timings (crate::timings), kept in `timings.jsonl` (an older worker sends none).
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::timings::lenient")]
    pub timings: Option<crate::timings::RunRec>,
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
    /// What a job's targets took at least, as it saw them (crate::agent::memguard: cost key, floor):
    /// a job stopped past its Mac's limit says what it held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floors: Vec<(String, Floor)>,
    /// The disk its targets took at least, as it saw them (cost key, MB), and the most it took over
    /// its whole run (MB).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<(String, u64)>,
    #[serde(default, skip_serializing_if = "is_zero64")]
    pub job_disk_mb: u64,
}

impl Coordinator {
    /// A viewer alone, from birth, answering on `listener` (docs/pool.md §10): the page and its
    /// views (`/work/swarm`, `/work/history`, `/work/steps`, from a pageview once it's given one;
    /// `/work/live`, from the live store),
    /// every other call refused. No state of a coordinator's: nothing loaded nor kept (its token,
    /// the one thing it makes, in a scratch folder of its own, emptied as it starts), no history,
    /// no background loop, no page address written.
    pub fn viewer(listener: std::net::TcpListener, me: &str, app: &str) -> Result<Coordinator> {
        let dir = std::env::temp_dir().join(format!(
            "scenic-viewer-{}-{}",
            std::process::id(),
            listener.local_addr()?.port()
        ));
        std::fs::remove_dir_all(&dir).ok();
        // (Those of viewers whose process is gone: removed.)
        if let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let pid = name.strip_prefix("scenic-viewer-").and_then(|r| r.split('-').next()?.parse::<i32>().ok());
                // (Gone: no such process, as kill 0 tells; one of another user's is alive.)
                if pid.is_some_and(|p| unsafe { libc::kill(p, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)) {
                    std::fs::remove_dir_all(e.path()).ok();
                }
            }
        }
        Coordinator::start_with(&dir, listener, me, app, true)
    }

    /// Starts answering on `port`, keeping its state in `dir`; `app`: the app this Mac's agent
    /// runs, the one an agent asking for work must run too.
    pub fn start(dir: &Path, port: u16, me: &str, app: &str) -> Result<Coordinator> {
        // (Listening first: a port in use writes nothing of its state. A coordinator this process
        // stopped has let its port go as its `stop` returned; another process's is no reason to
        // wait, and isn't waited for.)
        let listener = anyhow::Context::with_context(store::spawn::listen((crate::coord::LISTEN, port)), || {
            format!("listen on port {port}")
        })?;
        Coordinator::start_on(dir, listener, me, app)
    }

    /// `start`, answering on `listener`, already listening (a test's, on a port it held from the
    /// start: no other can take it between).
    pub fn start_on(dir: &Path, listener: std::net::TcpListener, me: &str, app: &str) -> Result<Coordinator> {
        Coordinator::start_with(dir, listener, me, app, false)
    }

    /// `start_on`, or a viewer alone (`viewer`).
    fn start_with(
        dir: &Path,
        listener: std::net::TcpListener,
        me: &str,
        app: &str,
        viewer: bool,
    ) -> Result<Coordinator> {
        let port = listener.local_addr()?.port();
        if viewer {
            std::fs::create_dir_all(dir)?;
        } else {
            std::fs::create_dir_all(dir.join("journal"))?;
        }
        let token = http::token(dir)?;
        let now = Instant::now();
        // (A viewer's folder is its own scratch, emptied as it starts: no leases.)
        let mut leases = Leases::load(&dir.join("leases.json"), TTL, now);
        // (What releases a target held for a newer app: this one, `Hold`.)
        leases.app = app.to_string();
        leases.me = me.to_string();
        // This Mac's own: its jobs ended with the agent that ran them.
        for l in leases.drop_where(|l| l.worker == me) {
            eprintln!("coordinator: {}'s lease ended with the agent before this one", l.what());
        }
        let costs = std::fs::read(dir.join("costs.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let floors = std::fs::read(dir.join("floors.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut disks: BTreeMap<String, u64> = std::fs::read(dir.join("disks.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // (Measures of an old step shape dropped, `old_shape`: written without them at the next save.)
        let mut costs: BTreeMap<String, Cost> = costs;
        let mut floors: BTreeMap<String, Floor> = floors;
        costs.retain(|k, _| !old_shape(k));
        floors.retain(|k, _| !old_shape(k));
        disks.retain(|k, _| !old_shape(k));
        let (job_disks, fills) = job_disks_load(dir, &mut disks, !viewer);
        // The build's pause, as it was when the agent before this one stopped (`{pause, at}`; an older
        // agent's, the pause alone).
        let kept: serde_json::Value = std::fs::read(dir.join("pause.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let (paused, pause_at): (Option<crate::control::Pause>, u64) = match kept.get("at") {
            Some(at) if kept.get("pause").is_some() => {
                (serde_json::from_value(kept["pause"].clone()).ok().flatten(), at.as_u64().unwrap_or(0))
            }
            _ => {
                let p: Option<crate::control::Pause> = serde_json::from_value(kept).ok();
                let at = p.as_ref().map_or(0, |p| p.at);
                (p, at)
            }
        };
        // (No later than a minute from now: an ask's time past that is passed over, `AHEAD_S`.)
        let pause_at = pause_at.min(unix_now() + AHEAD_S);
        let mut history = history::History::load((!viewer).then(|| dir.join("history.jsonl")).as_deref());
        if !viewer {
            history.add(history::Event {
                worker: Some(me.to_string()),
                note: format!("app {app}"),
                ..history::Event::new("agent")
            });
        }
        // (The devices a page once had to be accepted as: no more, `devices.json` with them.)
        std::fs::remove_file(dir.join("devices.json")).ok();
        let shared = Shared {
            sched: sched::Sched {
                leases,
                offers: Vec::new(),
                offered: Default::default(),
                drops: Default::default(),
                done: BTreeMap::new(),
                failed: BTreeMap::new(),
                workers: BTreeMap::new(),
                costs,
                floors,
                disks,
                job_disks,
                fills,
                needs: Default::default(),
                needs_rev: 0,
                app: app.to_string(),
                placement: None,
                nas: None,
                nas_own: None,
                lead_name: me.to_string(),
                nas_waits: Default::default(),
                nas_reserved_at: Default::default(),
                quiet_since: Default::default(),
                last: BTreeMap::new(),
                measured_steps: Default::default(),
                measured_of: Default::default(),
                drop_needs: Default::default(),
            },
            pass: String::new(),
            plan: 0,
            resume: None,
            tasks: task::Tasks::new(dir.join("tasks")),
            paused,
            pause_at,
            history,
            moving: None,
            told: None,
            min_app: None,
            pool_member: None,
            inbox: None,
            inputs_asks: Vec::new(),
            redo_asks: Vec::new(),
            nas_failures: 0,
            nas_loop: Default::default(),
            nas_probes: Vec::new(),
            nas_disks: Vec::new(),
            nas_mount: None,
            refused: BTreeMap::new(),
            brokers: None,
            pageview: None,
            live: None,
            catalogues: None,
            maplog: None,
            built: None,
            regions: None,
            region_steps: None,
            region_steps_asked: None,
            tocome: None,
            viewer,
            forward: None,
            settings: None,
            page_acts: Vec::new(),
            retry_blocked: None,
            keep: Default::default(),
            own_cap: None,
            dir: dir.to_path_buf(),
            clock: store::clock::real(),
        };
        if !viewer {
            shared.save_leases();
        }
        let shared = Arc::new(Mutex::new(shared));
        // This agent's jobs' own token (they offer tasks): never published, and gone with them.
        let job_token = http::random()?;
        let root = Arc::new(Mutex::new(None));
        let stopped: Arc<std::sync::atomic::AtomicBool> = Default::default();
        let wake: Arc<tokio::sync::Notify> = Default::default();
        let (closed_tx, closed) = std::sync::mpsc::channel();
        let routes: Arc<std::sync::RwLock<bool>> = Default::default();
        let cors: Arc<std::sync::RwLock<Option<BTreeSet<String>>>> = Default::default();
        let ctx = http::Ctx {
            shared: shared.clone(),
            token: token.clone(),
            job_token: job_token.clone(),
            journal: dir.join("journal"),
            routes: routes.clone(),
            cors: cors.clone(),
        };
        let urls = http::serve(listener, port, ctx, stopped.clone(), wake.clone(), closed_tx)?;
        let phase_shares: Arc<Mutex<BTreeMap<String, crate::timings::PhaseShares>>> = Default::default();
        let phase_times: Arc<Mutex<Option<Arc<crate::timings::PhaseTimes>>>> = Default::default();
        if !viewer {
            nas_prober(shared.clone(), root.clone(), stopped.clone(), me.to_string());
            phase_sharer(dir.join("timings.jsonl"), phase_shares.clone(), phase_times.clone(), stopped.clone());
        }
        let c = Coordinator {
            shared,
            contact: Contact { urls, token, page: None },
            job_token,
            port,
            me: me.to_string(),
            root,
            stopped,
            wake,
            closed: Mutex::new(Some(closed)),
            routes,
            cors,
            phase_shares,
            phase_times,
        };
        if !viewer {
            c.write_page();
        }
        Ok(c)
    }

    /// How many leases are out now (jobs' and tasks').
    pub fn leases_out(&self) -> usize {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        s.leases.all(now).len()
    }

    /// Stops answering (this process no longer leads): its listener closed and its runtime shut down,
    /// its port free for the next coordinator once this returns (its thread says so, `closed`: no
    /// clock decides it), whatever a blocking task of it waits on. Its state stays on disk, as the
    /// requests under way left it: the JSON routes under way are let finish (local and short: the
    /// state's lock and its files), and none starts after, so no JSON route writes once this
    /// returns (an upload's rename already under way, `/work/out`, may still land).
    pub fn stop(&self) {
        self.stopped.store(true, std::sync::atomic::Ordering::Relaxed);
        self.shared.lock().unwrap().keep.stop();
        *self.routes.write().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.notify_one();
        if let Some(rx) = self.closed.lock().unwrap().take() {
            // (Its thread gone already, its listener with it: the sender dropped.)
            rx.recv().ok();
        }
        // (A viewer's scratch folder goes with it.)
        let s = self.shared.lock().unwrap();
        if s.viewer {
            std::fs::remove_dir_all(&s.dir).ok();
        }
    }

    /// Writes the build page's address for this Mac's status bar (tools/status): over HTTPS when
    /// `tailscale serve` proxies the coordinator (a secure context, where the page may keep the
    /// screen on), else its first address. (No key in it.)
    fn write_page(&self) {
        let Some(base) = http::served_https(self.port).or_else(|| self.contact.urls.first().map(|u| format!("{u}/")))
        else {
            return;
        };
        let page = self.shared.lock().unwrap().dir.join("page");
        let text = format!("{base}work/\n");
        if std::fs::read_to_string(&page).ok().as_deref() != Some(text.as_str())
            && store::whole::write(&page, text.as_bytes()).is_ok()
        {
            if let Ok(f) = std::fs::File::open(&page) {
                store::sys::set_mode(&f, 0o600).ok();
            }
        }
    }

    /// The NAS's project folder as the agent has it now (None: away), for its NAS probe
    /// (`nas_prober`).
    pub fn set_root(&self, root: Option<&store::nasfs::NasRoot>) {
        *self.root.lock().unwrap() = root.cloned();
    }

    /// Where the hand-offs it took are journaled (a folder per worker), for the agent to merge.
    pub fn journal(&self) -> PathBuf {
        self.shared.lock().unwrap().dir.join("journal")
    }

    /// Publishes how to reach it on the NAS, when what's there differs (written whole), and the
    /// worker page's address here (it moves to HTTPS once `tailscale serve` proxies the coordinator).
    pub fn publish(&self, root: &store::nasfs::NasRoot) -> Result<()> {
        self.write_page();
        // (With the page's HTTPS address as it is now: `tailscale serve` may come or go.)
        let mut contact = self.contact.clone();
        {
            contact.page = http::served_https(self.port).map(|b| format!("{b}work/"));
        }
        let there = contact_path(root).read().ok().and_then(|b| serde_json::from_slice::<Contact>(&b).ok());
        if there.as_ref() != Some(&contact) {
            contact_path(root).write_whole(serde_json::to_vec_pretty(&contact)?)?;
        }
        Ok(())
    }

    /// Takes it off the NAS (the agent stopping, or its Mac no longer leading), when it's this
    /// one's: its addresses, not only its token (the pool's, the same on every lead: a lead that
    /// lost its term while frozen would take the new lead's contact off).
    pub fn unpublish(&self, root: &store::nasfs::NasRoot) {
        self.unpublish_unless(root, &|| false);
    }

    /// `unpublish`, unless `published` says the contact was published again meanwhile (asked just
    /// before the remove: a removal that waited on a hung share doesn't take a fresh contact off).
    pub fn unpublish_unless(&self, root: &store::nasfs::NasRoot, published: &dyn Fn() -> bool) {
        let there = contact_path(root).read().ok().and_then(|b| serde_json::from_slice::<Contact>(&b).ok());
        if there.is_some_and(|c| c.token == self.contact.token && c.urls == self.contact.urls) && !published() {
            contact_path(root).remove_file().ok();
        }
    }

    /// A lease for this Mac's own job of `step` over `targets`, unless a worker holds one of them,
    /// or (a job of no targets, like buildings, either side) any job of the step; its id.
    pub fn hold(&self, step: &str, targets: &[(String, String)]) -> Option<u64> {
        let mut s = self.shared.lock().unwrap();
        let now = s.now();
        let held = s.leases.held(step, now);
        if targets.iter().any(|t| held.contains(&t.0))
            || s.leases.live(now).any(|l| {
                matches!(&l.work, Work::Job { step: st, targets: ts } if st == step && (ts.is_empty() || targets.is_empty()))
            })
        {
            return None;
        }
        // (In the pool, a target a member did under this key, its entry not merged yet: as held.)
        if s.leases.term > 0 && targets.iter().any(|(t, k)| s.done.get(&(step.to_string(), t.clone())) == Some(k)) {
            return None;
        }
        let id = s.leases.grant(&self.me, Work::Job { step: step.to_string(), targets: targets.to_vec() }, now);
        s.save_leases();
        Some(id)
    }

    /// The leases this Mac's slots run jobs under now (crate::agent: each loop): those never lapse,
    /// beats or not, while they're said; one whose job ended leaves and lapses as any (#666).
    /// Each with the slot running it (`Lease::slot`).
    pub fn keep_own(&self, ids: BTreeMap<u64, u32>) {
        let mut s = self.shared.lock().unwrap();
        for (&id, &k) in &ids {
            let me = self.me.clone();
            s.leases.at_slot(id, &me, k);
        }
        s.leases.mine = ids.into_keys().collect();
    }

    /// Keeps this Mac's job's lease alive; false when it lapsed (the job paused too long, or the
    /// agent was stuck) and another may be building its targets.
    pub fn renew(&self, id: u64, progress: Option<String>) -> bool {
        self.renew_live(id, progress, None)
    }

    /// `renew`, with what its job is doing now (crate::agent::jobs::Live).
    pub fn renew_live(&self, id: u64, progress: Option<String>, live: Option<crate::agent::jobs::Live>) -> bool {
        let mut s = self.shared.lock().unwrap();
        let now = s.now();
        s.leases.renew_frac(id, &self.me, progress, None, live, now)
    }

    /// Ends this Mac's job's lease; `done`: the targets it recorded (not offered again until the plan
    /// shows them; the rest are free again).
    pub fn finish(&self, id: u64, done: &[(String, String)]) {
        self.end_own(id, done, false);
    }

    /// `finish`, its job failed: its targets not `done` are kept from this Mac as a member's failed
    /// ones are from it (an hour, doubling each time).
    pub fn finish_failed(&self, id: u64, done: &[(String, String)]) {
        self.end_own(id, done, true);
    }

    fn end_own(&self, id: u64, done: &[(String, String)], failed: bool) {
        let mut s = self.shared.lock().unwrap();
        let now = s.now();
        if let Some(l) = s.leases.finish(id, &self.me, now) {
            if let Work::Job { step, targets } = &l.work {
                for (t, k) in done {
                    s.done.insert((step.clone(), t.clone()), k.clone());
                }
                if failed {
                    for (t, _) in targets.iter().filter(|t| !done.iter().any(|d| d.0 == t.0)) {
                        let e = s.failed.entry((self.me.clone(), cost_key(step, t))).or_insert((now, 0));
                        *e = (now, e.1 + 1);
                    }
                }
            }
            s.save_leases();
        }
    }

    /// This Mac's own agent asking for work as any member does (docs/pool.md §7.2): granted by the
    /// same packing (`pack`) and under this Mac's name, the plan's offers in plan order, the lead's
    /// own planned jobs among them (`Offer::job`, granted to it alone: crate::agent::every::grants)
    /// but those named in `skip` (they can't start now, for a reason of the agent's own). Nothing
    /// while the build is paused or the lead is moving. Its leases are renewed and ended as this
    /// Mac's own (`renew`, `finish`).
    pub fn ask_own(&self, a: &Ask, skip: &BTreeSet<String>) -> Option<Grant> {
        let mut s = self.shared.lock().unwrap();
        let now = s.now();
        if s.paused.is_some() || s.moving_now(wall_now()).is_some() {
            return None;
        }
        let mut a = a.clone();
        a.worker = self.me.clone();
        // (Its capacity kept as this Mac's own, `set_capacity`, not as a worker's: said once.)
        let seen = Ask { cap: None, ..a.clone() };
        s.seen(&self.me, format!("asked for {} steps", a.can.len()), Some(&seen), now);
        if let Some(w) = s.workers.get_mut(&self.me) {
            w.old = false;
        }
        let offers: Vec<Offer> = s
            .offers
            .iter()
            .filter(|o| a.can.contains(&o.step) && o.job.as_ref().is_none_or(|j| !skip.contains(j)))
            .cloned()
            .collect();
        s.pack(&a, &offers, true, now)
    }

    /// The scheduler's state now, as data (`sched::State`): what the forecast runs the scheduler on.
    /// This Mac's own ask with its capacity (`own_cap`), as a member's carries its.
    pub fn sched_state(&self) -> sched::State {
        let s = self.shared.lock().unwrap();
        let mut st = s.sched.state(s.now());
        for w in st.workers.iter_mut().filter(|w| w.ask.worker == self.me) {
            w.ask.cap = s.own_cap.clone();
        }
        st
    }

    /// The number of the plan the next offers come from (`Grant::plan`).
    pub fn set_plan(&self, n: u64) {
        self.shared.lock().unwrap().plan = n;
    }

    /// The plan's work, in plan order, and its pass: the steps offered to every member, and the
    /// lead's own planned jobs (`Offer::job`). The plan's keys include what's journaled, so work done
    /// under the key the plan has now is no longer kept out by hand.
    pub fn offer(&self, pass: &str, offers: Vec<Offer>) {
        let mut s = self.shared.lock().unwrap();
        let planned: BTreeMap<(&str, &str), &str> = offers
            .iter()
            .flat_map(|o| o.targets.iter().map(move |(t, k, _)| ((o.step.as_str(), t.as_str()), k.as_str())))
            .collect();
        let done = std::mem::take(&mut s.done);
        s.done = done
            .into_iter()
            .filter(|((st, t), k)| planned.get(&(st.as_str(), t.as_str())) == Some(&k.as_str()))
            .collect();
        s.pass = pass.to_string();
        s.set_offers(offers.into_iter().filter(|o| !o.targets.is_empty()).collect());
        let steps: BTreeSet<String> = s.offers.iter().map(|o| o.step.clone()).collect();
        s.last.retain(|(_, st), _| steps.contains(st));
    }

    /// The NAS budget's limits (crate::budget::nas; None: off). Its probes set them while the
    /// switch is on (`nas_prober`); the tests and the simulator set them here.
    pub fn set_nas(&self, limits: Option<crate::budget::nas::Limits>) {
        let mut s = self.shared.lock().unwrap();
        (s.nas, s.nas_own) = (limits, limits);
    }

    /// Whether the NAS's limits are known (`nas_own`: a probe landed, this day's best kept): the
    /// lead's slots' ramp holds at its start until they are (crate::agent's `tend_ramp`).
    pub fn nas_known(&self) -> bool {
        self.shared.lock().unwrap().nas_own.is_some()
    }

    /// Whether the NAS probes are failing: `NAS_FAILED_RUN` or more in a row (`Shared::nas_failures`):
    /// the ramp holds, and its stretch starts again. One failed alone isn't failing.
    pub fn nas_failing(&self) -> bool {
        self.shared.lock().unwrap().nas_failures >= NAS_FAILED_RUN
    }

    /// Tests: the probes failed `n` in a row (0: the last landed).
    pub fn set_nas_failures(&self, n: u32) {
        self.shared.lock().unwrap().nas_failures = n;
    }

    /// Whether the pool's leased jobs' NAS weights together pass the whole (crate::budget::nas::WHOLE)
    /// by the NAS's limits as probed (`nas_own`), as the budget's own rules let them (§7.7: the
    /// lead's own jobs alone, an aged job weighing the whole). False before the NAS is probed. (Not
    /// the lead's ramp's trouble since task #433: a grant by the rules says nothing about its slots.)
    pub fn nas_over(&self) -> bool {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        s.nas_room(now, true).is_some_and(|(_, running, _, _)| running > crate::budget::nas::WHOLE)
    }

    /// Whether this Mac's running job of a quiet step (crate::budget::nas::QUIET_STEPS: the clean-up)
    /// over `targets` should yield the NAS (task #451): a target held back for the budget alone for
    /// `OWN_AGE` (`Shared::nas_reserved`) that doesn't fit beside the leased jobs now but would
    /// without this one. Why, in words; None: it goes on.
    pub fn nas_yield(&self, step: &str, targets: &[String]) -> Option<String> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        let (nw, running, _, _) = s.nas_room(now, true)?;
        let (st, t, rw) = s.nas_reserved(&nw, now)?;
        if st == step {
            return None;
        }
        let (w, _) = sched::Sched::job_nas(&nw, step, targets.iter().map(String::as_str));
        let whole = crate::budget::nas::WHOLE;
        (w > 0 && running.saturating_add(rw) > whole && running.saturating_sub(w).saturating_add(rw) <= whole).then(
            || {
                format!(
                    "yields the NAS to {st} {t}, held back for it {} min or more",
                    crate::budget::nas::OWN_AGE.as_secs() / 60
                )
            },
        )
    }

    /// The targets waiting on the NAS budget alone for longer than `NAS_WAIT_SAY_S` (passed over in
    /// the last two minutes): (step, target, minutes), for the status's waiting lines.
    /// Why the NAS budget is held by the NAS's own load, if it is ("held by NAS load (91 % >
    /// target 85 %)", crate::budget::nas::Loop::why).
    pub fn nas_held(&self) -> Option<String> {
        self.shared.lock().unwrap().nas_loop.why()
    }

    pub fn nas_waits(&self) -> Vec<(String, String, u64)> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        let mut g = s.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|_, (_, last)| now.saturating_duration_since(*last) < Duration::from_secs(120));
        g.iter()
            .filter(|(_, (first, _))| now.saturating_duration_since(*first) >= Duration::from_secs(NAS_WAIT_SAY_S))
            .map(|((st, t), (first, _))| (st.clone(), t.clone(), now.saturating_duration_since(*first).as_secs() / 60))
            .collect()
    }

    /// The lead's own Mac's capacity, measured each loop (its jobs ask nothing).
    pub fn set_capacity(&self, cap: Option<Capacity>) {
        self.shared.lock().unwrap().own_cap = cap;
    }

    /// Whether every member around reports the new app (its asks carry a capacity): packing
    /// several jobs per member on, and the old wire's fields no longer written
    /// (crate::agent::Status::all_new). False while any is on an older app (`old_member`).
    pub fn all_new(&self) -> bool {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        s.old_member(now).is_none()
    }

    /// Each member's capacity as last said: this Mac's own first (`set_capacity`), then each worker
    /// around whose ask carried one.
    pub fn capacities(&self) -> Vec<(String, Capacity)> {
        self.shared.lock().unwrap().capacities(&self.me)
    }

    /// The NAS as the lead sees it (`NasView`), for the status; None before its prober knows the
    /// NAS (no probe kept, no disk read).
    pub fn nas_view(&self) -> Option<NasView> {
        let s = self.shared.lock().unwrap();
        let l = s.nas_loop;
        (!s.nas_probes.is_empty() || l.load.is_some()).then(|| NasView {
            probes: s.nas_probes.clone(),
            limits: s.nas_own,
            load_pct: l.load.filter(|v| v.is_finite()).map(|v| (v * 10.0).round() / 10.0),
            target_pct: l.target,
            held: l.held(),
            // (Finite alone: a NaN would be written null and the status read back not at all.)
            disks_pct: s.nas_disks.iter().copied().filter(|v| v.is_finite()).collect(),
            traffic: pool_traffic(&s.capacities(&self.me)),
            mount: s.nas_mount.clone(),
        })
    }

    /// Why nothing more fits each worker, as its last ask was packed (`Shared::refused`), the
    /// refusal itself younger than `CAP_FRESH_S` (older ones dropped here): the status's, per Mac.
    pub fn refusals(&self) -> BTreeMap<String, String> {
        let mut s = self.shared.lock().unwrap();
        // (Old ones let go: a worker gone asks no more.)
        s.refused.retain(|_, (at, _)| unix_now().saturating_sub(*at) < CAP_FRESH_S);
        s.refused
            .iter()
            .filter(|(_, (at, _))| unix_now().saturating_sub(*at) < CAP_FRESH_S)
            .map(|(w, (_, why))| (w.clone(), why.clone()))
            .collect()
    }

    /// Each step's phase shares over its last runs, as its thread last read the build's timings
    /// (`phase_sharer`; empty till then).
    pub fn phase_shares(&self) -> BTreeMap<String, crate::timings::PhaseShares> {
        self.phase_shares.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Each step's time by phase over the page's chart ranges, as its thread last summed them
    /// (`phase_sharer`, every minute; None till then).
    pub fn phase_times(&self) -> Option<Arc<crate::timings::PhaseTimes>> {
        self.phase_times.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Each worker's share of the NAS budget now: its leased jobs' NAS weights summed, in
    /// thousandths of the whole (crate::budget::nas::WHOLE). Empty before the NAS is probed.
    pub fn nas_shares(&self) -> BTreeMap<String, u32> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        let Some((nw, _, _, _)) = s.nas_room(now, true) else { return BTreeMap::new() };
        let mut out: BTreeMap<String, u32> = BTreeMap::new();
        for lease in s.leases.all(now) {
            if let Work::Job { step, targets } = &lease.work {
                let (w, _) = sched::Sched::job_nas(&nw, step, targets.iter().map(|t| t.0.as_str()));
                let e = out.entry(lease.worker.clone()).or_default();
                *e = e.saturating_add(w);
            }
        }
        out
    }

    /// What one job of each step leased now weighs of the NAS budget (its heaviest's, thousandths
    /// of the whole): the page's "a terrain job is allowed 12 %". Empty before the NAS is probed.
    pub fn nas_step_weights(&self) -> BTreeMap<String, u32> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        let Some((nw, _, _, _)) = s.nas_room(now, true) else { return BTreeMap::new() };
        let mut out: BTreeMap<String, u32> = BTreeMap::new();
        for lease in s.leases.all(now) {
            if let Work::Job { step, targets } = &lease.work {
                let (w, _) = sched::Sched::job_nas(&nw, step, targets.iter().map(|t| t.0.as_str()));
                let e = out.entry(step.clone()).or_default();
                *e = (*e).max(w);
            }
        }
        out
    }

    /// Placement by how crucial a job is on (`Some`: the forecast's slack and measured speeds) or
    /// off (docs/pool.md §12, phase 4's eleventh batch; crate::agent::placement). Its draws' windows
    /// run from when it was first turned on.
    pub fn set_placement(&self, p: Option<crate::agent::placement::Placement>) {
        let mut s = self.shared.lock().unwrap();
        let now = s.now();
        s.placement = match (p, s.placement.take()) {
            (Some(p), Some((_, since))) => Some((p, since)),
            (Some(p), None) => Some((p, now)),
            (None, _) => None,
        };
    }

    /// A resumable job offered to worker `worker` alone (crate::agent::resume: its Mac holds the
    /// job's progress), or none: granted whole, its targets as `o` has them, to that worker only,
    /// while no lease holds them.
    pub fn offer_resume(&self, to: Option<(String, Offer)>) {
        self.shared.lock().unwrap().resume = to.filter(|(_, o)| !o.targets.is_empty());
    }

    /// `offer` with z6 tiles alone (target, key, extract bytes).
    pub fn offer_units(&self, pass: &str, units: Vec<(String, String, u64)>) {
        self.offer(pass, vec![Offer { step: "unit".into(), targets: units, job: None }]);
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
        let s = self.shared.lock().unwrap();
        s.leases.held(step, s.now())
    }

    /// The owner's cancel (`scenic job cancel`, crate::control::Cancel), from this Mac's agent:
    /// every live job lease holding its target (of its step, when it names one) told to stop and
    /// give its work back (`cancel_of`, a member's beat), and with `rerun_after_update` the target
    /// held until this coordinator runs a newer app (`hold_for_update`). The leases told: (lease,
    /// worker, what).
    pub fn cancel(&self, c: &crate::control::Cancel) -> Vec<(u64, String, String)> {
        cancel_in(&mut self.shared.lock().unwrap(), c)
    }

    /// The owner's cancel of lease `id`, while it stands (`cancel`).
    pub fn cancel_of(&self, id: u64) -> Option<crate::control::Cancel> {
        let s = self.shared.lock().unwrap();
        s.leases.cancel_ask(id).cloned()
    }

    /// `target` of `step` held until this coordinator runs a newer app (lease::Hold): a job of it
    /// that held no lease here, cancelled to rerun after the update.
    pub fn hold_for_update(&self, step: &str, target: &str, by: &str) -> lease::Hold {
        let mut s = self.shared.lock().unwrap();
        let h = s.leases.hold_for_update(step, target, by, unix_now());
        s.save_leases();
        h
    }

    /// The targets held until a newer app (lease::Hold), for the status.
    pub fn holds(&self) -> Vec<lease::Hold> {
        self.shared.lock().unwrap().leases.holds()
    }

    /// Whether a cold start of `step` (none of its targets measured) is under way on another worker
    /// (a lease of it held by one): this Mac then starts none of its own, as none is granted to
    /// another meanwhile (`pick_every`), so one cold start of a step runs in the pool at a time.
    pub fn cold_under_way(&self, step: &str) -> bool {
        self.shared.lock().unwrap().measured(step, "").is_none() && !self.held_by_others(step).is_empty()
    }

    /// The targets of `step` leases of other workers than this Mac hold (its own jobs' aside).
    pub fn held_by_others(&self, step: &str) -> BTreeSet<String> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        s.leases
            .all(now)
            .iter()
            .filter(|l| l.worker != self.me && matches!(&l.work, Work::Job { step: st, .. } if st == step))
            .flat_map(|l| l.targets().iter().map(|t| t.0.clone()))
            .collect()
    }

    /// The leases' generation, this Mac's own jobs' left out (`Leases::generation`).
    pub fn lease_gen(&self) -> u64 {
        self.shared.lock().unwrap().leases.generation(&self.me)
    }

    /// Leases past their deadline dropped (their workers went quiet): a task's offered again.
    pub fn expire(&self) -> Vec<Lease> {
        let mut s = self.shared.lock().unwrap();
        // (None while the build is paused: every lease held, a paused or asleep worker's work not
        // given to another.)
        let now = s.now();
        if s.paused.is_some() {
            s.leases.hold_all(now);
            return Vec::new();
        }
        let gone = s.leases.expire(now);
        for l in &gone {
            let (step, targets) = match &l.work {
                Work::Job { step, targets } => (Some(step.clone()), targets.iter().map(|t| t.0.clone()).collect()),
                Work::Task { id } => (Some(s.tasks.kind_of(*id)), Vec::new()),
            };
            if let Work::Task { .. } = l.work {
                s.tasks.lapsed(l.id);
            }
            s.history.add(history::Event {
                worker: Some(l.worker.clone()),
                lease: Some(l.id),
                step,
                targets,
                note: lapse_why(l),
                ..history::Event::new("lapse")
            });
        }
        if gone.iter().any(|l| matches!(l.work, Work::Job { .. })) {
            s.save_leases();
        }
        // (Failures past their longest wait forgotten.)
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

    /// A job's timings (crate::timings), kept with the build's (`timings.jsonl`).
    pub fn add_timings(&self, rec: &crate::timings::RunRec) {
        let dir = self.shared.lock().unwrap().dir.clone();
        keep_timings(&dir, rec);
    }

    /// What this Mac's job's z6 tiles cost (measured here).
    pub fn add_costs(&self, costs: &[(String, Cost)]) {
        self.add_costs_by(costs, &self.me.clone());
    }

    /// `add_costs`, measured by `worker` (one of this Mac's slots: crate::agent::slot_worker).
    pub fn add_costs_by(&self, costs: &[(String, Cost)], worker: &str) {
        if costs.is_empty() {
            return;
        }
        let mut s = self.shared.lock().unwrap();
        s.add_costs(costs.iter().cloned(), worker);
        s.save_costs();
    }

    /// What this Mac's job saw its targets hold at least (crate::agent::memguard: cost key, floor).
    pub fn add_floors(&self, floors: &[(String, Floor)]) {
        let mut s = self.shared.lock().unwrap();
        if s.add_floors(floors) {
            s.save_costs();
        }
    }

    /// The floor of `target` of `step` (what it takes at least), when one was seen, of any way its
    /// step ran.
    pub fn floor(&self, step: &str, target: &str) -> Option<Floor> {
        self.shared.lock().unwrap().floors.get(&cost_key(step, target)).copied()
    }

    /// The floor of `target` of `step` that may hold it off a Mac: the way its step runs now.
    pub fn holding_floor(&self, step: &str, target: &str) -> Option<Floor> {
        self.shared.lock().unwrap().holding_floor(step, target)
    }

    /// The floors kept, after clearing those of `step` (all, with no step), or of its `target`
    /// (`scenic pool floors --clear`), when `clear`.
    pub fn floors(&self, clear: bool, step: Option<&str>, target: Option<&str>) -> BTreeMap<String, Floor> {
        floors_cleared(&mut self.shared.lock().unwrap(), clear, step, target)
    }

    /// What this Mac's job saw its targets take on the disk at least (cost key, MB).
    pub fn add_disks(&self, disks: &[(String, u64)]) {
        let mut s = self.shared.lock().unwrap();
        if s.add_disks(disks) {
            s.save_costs();
        }
    }

    /// The disk floors kept (cost key → MB).
    pub fn disks(&self) -> BTreeMap<String, u64> {
        self.shared.lock().unwrap().disks.clone()
    }

    /// What a job's target is measured to take (docs/pool.md §7.7): its memory (MB: its run's
    /// measure the way its step runs now, or its floor, the larger) and disk (MB: its disk floor),
    /// each its own when a run of it said, else the largest of its step's other targets' (a prior
    /// learned, not kept by hand: what's offered or guessed counts for nothing); None for a cold
    /// start, a target of a step none of whose targets has been measured.
    pub fn measured(&self, step: &str, target: &str) -> Option<Measured> {
        self.shared.lock().unwrap().measured(step, target)
    }

    /// What a job of `step` over `targets` is measured to take (docs/pool.md §7.7): the most of
    /// their memory, and on the disk the larger of their disks together (`needs::batch_disk`: summed,
    /// a batch's cache writes adding up; a halo step's the most of them, one cache shared) and the
    /// most a job of the step took over its run (`job_disks`); None when any is a cold start.
    pub fn measured_batch(&self, step: &str, targets: &[String]) -> Option<Measured> {
        let each: Vec<Measured> = targets.iter().map(|t| self.measured(step, t)).collect::<Option<_>>()?;
        let job = self.job_disk(step);
        Some(Measured {
            mem_mb: each.iter().map(|m| m.mem_mb).max().unwrap_or(0),
            disk_mb: crate::needs::batch_disk(step, each.iter().map(|m| m.disk_mb)).max(job),
            own: each.iter().all(|m| m.own),
        })
    }

    /// The disk a job of `step` takes at least, whatever its targets (MB; `Shared::job_floor`: its
    /// last jobs' 90th percentile, 0 for a step reading a shared cache).
    pub fn job_disk(&self, step: &str) -> u64 {
        self.shared.lock().unwrap().job_floor(step)
    }

    /// What filling shared cache `cache` on a Mac takes (MB, `Shared::fills`).
    pub fn fill_mb(&self, cache: &str) -> u64 {
        self.shared.lock().unwrap().fill_mb(cache)
    }

    /// What a job of `step` over `targets` took on the disk over its run (MB): its step's last
    /// runs, and its shared cache's fill when it filled it (`Shared::note_fill`).
    pub fn add_job_run(&self, step: &str, targets: &[String], mb: u64) {
        let mut s = self.shared.lock().unwrap();
        if s.add_job_disk(step, mb) | s.note_fill(step, targets, mb) {
            s.save_costs();
        }
    }

    /// What this Mac's job of `step` took on the disk over its run (MB).
    pub fn add_job_disk(&self, step: &str, mb: u64) {
        let mut s = self.shared.lock().unwrap();
        if s.add_job_disk(step, mb) {
            s.save_costs();
        }
    }

    /// The largest memory limit a worker around says its Mac has (crate::agent::memguard::limit_mb:
    /// an agent asking in the last ten minutes), when any says one.
    pub fn largest_limit(&self) -> Option<u64> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        s.workers
            .values()
            .filter(|w| now.duration_since(w.seen) < Duration::from_secs(600))
            .filter_map(|w| w.ask.as_ref().and_then(ask_limit))
            .max()
    }

    /// The memory a job of `step` for `target` is expected to take (MB), as `pick` sizes it: its
    /// step's model's (crate::needs), its first guess what the plan offered it with (a z6 tile's extract,
    /// another step's guess); None when nothing of its step was measured and it isn't offered.
    pub fn peak(&self, step: &str, target: &str) -> Option<u64> {
        let s = self.shared.lock().unwrap();
        match s.offered_size(step, target) {
            Some(size) => Some(s.peak_of(step, target, size, 0)),
            None => {
                let (mb, from) = s.book().mem(step, target, 0, 0);
                (from != crate::needs::From::Table).then_some(mb)
            }
        }
    }

    /// The threads the run that measured `step`'s `target` had (its cost record's `sizes.threads`),
    /// when it says: the width its peak was measured at.
    pub fn peak_threads(&self, step: &str, target: &str) -> Option<u32> {
        let s = self.shared.lock().unwrap();
        let book = s.book();
        let c = book.costs.get(&cost_key(step, target))?;
        if c.v < cost_version(step) {
            return None;
        }
        c.sizes.get("threads").map(|&t| t as u32)
    }

    /// Notes what happened (`history`): this Mac's jobs started and ended, catalogues, its conditions.
    pub fn note(&self, e: history::Event) {
        self.shared.lock().unwrap().history.add(e);
    }

    /// What the forecast reads (crate::agent::forecast): what each target cost, the jobs leased now
    /// (worker, step, targets), the history kept, and the memory each worker spares (MB).
    pub fn for_forecast(
        &self,
    ) -> (
        BTreeMap<String, Cost>,
        Vec<(String, String, Vec<String>, u64, bool)>,
        Vec<history::Event>,
        BTreeMap<String, u64>,
    ) {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        // (Each with how long ago it was granted; its worker the slot running it when said
        // (`Lease::slot`, crate::agent::slot_worker; then true), else its Mac.)
        let leased = s
            .leases
            .all(now)
            .iter()
            .filter_map(|l| match &l.work {
                Work::Job { step, targets } => Some((
                    l.slot.map_or_else(|| l.worker.clone(), |k| crate::agent::slot_worker(&l.worker, k as usize)),
                    step.clone(),
                    targets.iter().map(|t| t.0.clone()).collect(),
                    now.duration_since(l.granted).as_secs(),
                    l.slot.is_some(),
                )),
                Work::Task { .. } => None,
            })
            .collect();
        let mem = s.workers.iter().map(|(n, w)| (n.clone(), w.mem_mb)).collect();
        (s.costs.clone(), leased, s.history.since(0, usize::MAX), mem)
    }

    /// The targets of `steps` leased now, every Mac's.
    pub fn leased_targets(&self, steps: &[&str]) -> Vec<(String, String)> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        let mut out = Vec::new();
        for l in s.leases.all(now) {
            if let Work::Job { step, targets } = &l.work {
                if steps.contains(&step.as_str()) {
                    out.extend(targets.iter().map(|(t, _)| (step.clone(), t.clone())));
                }
            }
        }
        out
    }

    /// The targets of `step` leased now, by worker (a job's, every Mac's: the lead's own are by
    /// grant too).
    pub fn leased_of(&self, step: &str) -> BTreeMap<String, usize> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        let mut out: BTreeMap<String, usize> = BTreeMap::new();
        for l in s.leases.all(now) {
            if let Work::Job { step: st, targets } = &l.work {
                if st == step {
                    *out.entry(l.worker.clone()).or_default() += targets.len();
                }
            }
        }
        out
    }

    /// Whether a worker around (not found wrong) takes tails, spares what one takes typically (any,
    /// before one was offered) and is measured faster than this Mac at them (task::beats): this
    /// Mac's `unit` jobs then give it a moment to take theirs (crate::offload::LEASE_WAIT), the
    /// forecast's z6 tiles that much longer.
    pub fn tail_takers(&self) -> bool {
        let s = self.shared.lock().unwrap();
        let mb = s.tasks.typical_mb("tail").unwrap_or(0);
        let now = s.now();
        s.workers.iter().any(|(n, w)| {
            now.saturating_duration_since(w.seen) < AROUND
                && w.can.iter().any(|c| c == "tail")
                && w.mem_mb >= mb
                && s.tasks.pace(n, "tail").is_some_and(task::beats)
        })
    }

    /// The pool's member this coordinator answers as (`Shared::pool_member`); None while its Mac
    /// has lost the NAS: its answers then say no term (a member's reach counts no answer of it).
    pub fn set_member(&self, member: Option<&str>) {
        self.shared.lock().unwrap().pool_member = member.map(str::to_string);
    }

    /// Where the members' messages to its member go (`Shared::inbox`).
    pub fn set_inbox(&self, inbox: Option<std::sync::Arc<inbox::Inbox>>) {
        self.shared.lock().unwrap().inbox = inbox;
    }

    /// The pool's term the leases it grants now are in (docs/pool.md §7.5; 0: the pool off).
    pub fn set_term(&self, term: u64) {
        self.shared.lock().unwrap().leases.term = term;
    }

    /// The pool's term lease `id` was granted in (0: none, or the pool off).
    pub fn lease_term(&self, id: u64) -> u64 {
        self.shared.lock().unwrap().leases.of(id).map_or(0, |l| l.term)
    }

    /// Ends the leases of `workers` (this Mac's own, whose jobs ended with the process before);
    /// those ended.
    pub fn drop_workers(&self, workers: &[String], keep: &[u64]) -> Vec<Lease> {
        let mut s = self.shared.lock().unwrap();
        let gone = s.leases.drop_where(|l| workers.contains(&l.worker) && !keep.contains(&l.id));
        if !gone.is_empty() {
            s.save_leases();
        }
        gone
    }

    /// The pool's lead granting nothing now, and why (its view not fresh, no longer leading); None:
    /// granting again.
    pub fn set_moving(&self, why: Option<String>) {
        self.set_moving_as_of(why, wall_now());
    }

    /// `set_moving` as of `as_of` (this Mac's wall clock, unix s): when the pool's step that decided
    /// it ran, not when it's told (whatever ran between, a write on a hung share, doesn't stretch
    /// the time it grants for: `STALL_S` from the step's checks).
    pub fn set_moving_as_of(&self, why: Option<String>, as_of: u64) {
        let mut s = self.shared.lock().unwrap();
        s.moving = why;
        s.told = Some(as_of.min(wall_now()));
    }

    /// The build page's regions as this Mac's agent made them (`Shared::regions`).
    pub fn set_regions(&self, v: Arc<serde_json::Value>) {
        self.shared.lock().unwrap().regions = Some(v);
    }

    /// Each region's targets built by step as this Mac's agent counted them (`Shared::region_steps`).
    pub fn set_region_steps(&self, v: Arc<serde_json::Value>) {
        self.shared.lock().unwrap().region_steps = Some(v);
    }

    /// Whether a page asked for the regions' steps within `REGION_STEPS_ASKING`.
    pub fn region_steps_asked(&self) -> bool {
        self.shared.lock().unwrap().region_steps_asked.is_some_and(|t| t.elapsed() < REGION_STEPS_ASKING)
    }

    /// Each Mac's tiles to come in this Mac's last forecast (`Shared::tocome`).
    pub fn set_tocome(&self, t: Arc<crate::agent::forecast::ToCome>) {
        self.shared.lock().unwrap().tocome = Some(t);
    }

    /// The forecast's dependencies as the history's waits (history::WaitsBasis::next), worked out
    /// off the lock (a forecast's tens of thousands of items: tens of ms).
    pub fn waits_of(&self, g: history::Graph) -> history::Waits {
        let basis = self.shared.lock().unwrap().history.waits_basis();
        basis.next(g)
    }

    /// The waits (`waits_of`) the history's starts and leases name as waited on from now on
    /// (history::Event::after).
    pub fn set_waits(&self, w: history::Waits) {
        self.shared.lock().unwrap().history.set_waits(w);
    }

    /// The app the newer readers' switch was recorded on (`Shared::min_app`; None: none recorded).
    pub fn set_min_app(&self, app: Option<String>) {
        self.shared.lock().unwrap().min_app = app;
    }

    /// A target's failures forgotten (a page's retry, crate::pool::driver::Act::Retry): every
    /// worker's, of its step (any step: None); how many.
    pub fn clear_failed(&self, step: Option<&str>, target: &str) -> usize {
        let mut s = self.shared.lock().unwrap();
        let before = s.failed.len();
        s.failed.retain(|(_, k), _| match step {
            Some(st) => *k != cost_key(st, target),
            None => k != target && !k.ends_with(&format!(" {target}")),
        });
        before - s.failed.len()
    }

    /// The pages' asks to cancel or retry (`/work/act`), taken.
    pub fn take_page_acts(&self) -> Vec<crate::pool::driver::Act> {
        std::mem::take(&mut self.shared.lock().unwrap().page_acts)
    }

    /// Not the lead's: why a page's retry can't reach the lead now (`Shared::retry_blocked`).
    pub fn set_retry_blocked(&self, why: Option<String>) {
        self.shared.lock().unwrap().retry_blocked = why;
    }

    /// Not the lead's: a page's pause goes to this Mac's agent (its folder `home`) to pass on.
    pub fn set_forward(&self, home: Option<PathBuf>) {
        self.shared.lock().unwrap().forward = home;
    }

    /// Where the owner's settings lie (`Shared::settings`): this Mac's agent's folder `home`, the
    /// NAS's project folder `root`.
    pub fn set_settings(&self, home: &Path, root: Option<&store::nasfs::NasRoot>) {
        self.shared.lock().unwrap().settings = Some((home.to_path_buf(), root.cloned()));
    }

    /// The owner's asks to build targets again since the last call (`/work/redo`).
    pub fn take_redo_asks(&self) -> Vec<crate::agent::redo::Ask> {
        std::mem::take(&mut self.shared.lock().unwrap().redo_asks)
    }

    /// The build page's acceptances since the last call (`/work/inputs`).
    pub fn take_inputs_asks(&self) -> Vec<crate::inputs::Ask> {
        std::mem::take(&mut self.shared.lock().unwrap().inputs_asks)
    }

    /// What the worker page takes from the coordinator's memory (`/work/swarm`'s pause, leases,
    /// workers, tasks by state and `task_mb`), for the lead's `state/coord/term/<E>/view.json`
    /// (docs/pool.md §10): its times as unix seconds (a lease's `granted` and last `beat`, a
    /// worker's `seen`), so it's the same until something changes. Taken under the lock.
    pub fn view(&self) -> serde_json::Value {
        view_of(&self.shared.lock().unwrap())
    }

    /// The page's data served from `pv` (crate::agent::pageview) from here on.
    pub fn set_pageview(&self, pv: Arc<crate::agent::pageview::PageView>) {
        self.shared.lock().unwrap().pageview = Some(pv);
    }

    /// Whether it serves the page's data from a pageview.
    pub fn has_pageview(&self) -> bool {
        self.shared.lock().unwrap().pageview.is_some()
    }

    /// `/work/live` answered from `store` (crate::agent::live) from here on.
    pub fn set_live(&self, store: Arc<crate::agent::live::Store>) {
        self.shared.lock().unwrap().live = Some(store);
    }

    /// Whether it answers `/work/live`.
    pub fn has_live(&self) -> bool {
        self.shared.lock().unwrap().live.is_some()
    }

    /// `/work/catalogs` answered from `cv` (crate::agent::cataloguesview) from here on.
    pub fn set_catalogues(&self, cv: Arc<crate::agent::cataloguesview::CataloguesView>) {
        self.shared.lock().unwrap().catalogues = Some(cv);
    }

    /// `/map/changes` and `/map/snapshot` answered from `feed` (maproutes) from here on.
    pub fn set_maplog(&self, feed: Arc<store::map::feed::Feed>) {
        self.shared.lock().unwrap().maplog = Some(feed);
    }

    /// Whether it answers the map's routes.
    pub fn has_maplog(&self) -> bool {
        self.shared.lock().unwrap().maplog.is_some()
    }

    /// `/work/built` answered from `bv` (crate::agent::builtview) from here on.
    pub fn set_built(&self, bv: Arc<crate::agent::builtview::BuiltView>) {
        self.shared.lock().unwrap().built = Some(bv);
    }

    /// Whether it answers `/work/built`.
    pub fn has_built(&self) -> bool {
        self.shared.lock().unwrap().built.is_some()
    }

    /// Whether it answers `/work/catalogs`.
    pub fn has_catalogues(&self) -> bool {
        self.shared.lock().unwrap().catalogues.is_some()
    }

    /// Its state as the pool keeps it per term (docs/pool.md §6.2, §7.5): the jobs' leases (each
    /// with its term and when it was granted), what each target cost, the workers' failures (when,
    /// by this Mac's wall clock), the pause. Taken under the lock, written by the caller without it.
    pub fn pool_state(&self) -> PoolState {
        pool_state_of(&self.shared.lock().unwrap())
    }

    /// Writes its state of term `term` to the NAS from now on, off the caller's thread (`keep`):
    /// each lead's step names its term.
    pub fn keep_state(&self, nas: Arc<dyn crate::pool::nas::Nas + Send + Sync>, term: u64) {
        keep::keep(&self.shared, nas, term, keep::PACE);
    }

    /// Takes up `p`, the term before's state (its own before a restart): its leases over
    /// these, its costs and failures added, its pause if it's the later.
    pub fn load_pool_state(&self, p: &PoolState) -> Result<usize> {
        let mut s = self.shared.lock().unwrap();
        let (now, unix) = (s.now(), unix_now());
        // (A broker's own tasks' leases kept: its workers go on with them, docs/pool.md §8. One whose
        // id a restored lease has is lapsed, its task offered again.)
        let kept = if s.brokers.is_some() { s.leases.tasks() } else { Vec::new() };
        let n = s.leases.restore(&p.leases, now)?;
        for l in kept {
            let id = l.id;
            if !s.leases.put_back(l) {
                eprintln!("coordinator: task lease {id}'s id is a lease taken up: its task offered again");
                s.tasks.lapsed(id);
            }
        }
        // (Its measures, each in place of a floor kept here that the other lead had none of: its
        // measure took that floor's place there.)
        for k in p.costs.keys().filter(|k| !old_shape(k)) {
            if !p.floors.contains_key(k) {
                s.floors.remove(k);
            }
        }
        // (Nor an old step shape's measures, `old_shape`: a term's state kept them past the
        // folder's drop, and they came back with every take-up: the `trees 3/x/y` at 30–37 GB.)
        s.costs.extend(p.costs.iter().filter(|(k, _)| !old_shape(k)).map(|(k, v)| (k.clone(), v.clone())));
        s.needs_rev += 1;
        let floors: Vec<(String, Floor)> =
            p.floors.iter().filter(|(k, _)| !old_shape(k)).map(|(k, v)| (k.clone(), *v)).collect();
        s.add_floors(&floors);
        // (Nor, from a state of before `DISKS_V`, pack's and lo's targets' disks: each term's state
        // had kept those of before task #396, and brought them back with every take-up.)
        let (_, gone) = JOB_DISKS_V4;
        let stale = |k: &str| p.disks_v < DISKS_V && gone.contains(&crate::agent::memguard::step_of_key(k));
        let disks: Vec<(String, u64)> =
            p.disks.iter().filter(|(k, _)| !old_shape(k) && !stale(k)).map(|(k, v)| (k.clone(), *v)).collect();
        s.add_disks(&disks);
        // (Its jobs' disks: a step's runs where none are kept here; an older lead's single peak
        // only within reason of its targets' measures, `stale_peak`. Fills only of the caches there
        // are, as `job_disks_load` reads them.)
        for (c, mb) in p.fills.iter().filter(|(c, _)| crate::needs::CACHES.iter().any(|(n, _)| n == *c)) {
            s.fills.entry(c.clone()).or_insert(*mb);
        }
        for (step, v) in &p.job_peaks {
            if !s.job_disks.contains_key(step) {
                s.job_disks.insert(step.clone(), v.clone());
            }
        }
        for (step, mb) in &p.job_disks {
            if !s.job_disks.contains_key(step) && !stale_peak(&s.disks, step, *mb) {
                s.add_job_disk(step, *mb);
            }
        }
        for (w, k, at, n) in &p.failed {
            let at = now.checked_sub(Duration::from_secs(unix.saturating_sub(*at))).unwrap_or(now);
            s.failed.insert((w.clone(), k.clone()), (at, *n));
        }
        if p.pause_at >= s.pause_at {
            set_pause(&mut s, p.pause.clone(), p.pause_at);
        }
        s.save_leases();
        s.save_costs();
        Ok(n)
    }

    /// The history's events after number `seq` (the pool's history per writer: docs/pool.md §7.5).
    pub fn history_since(&self, seq: u64) -> Vec<history::Event> {
        self.shared.lock().unwrap().history.since(seq, usize::MAX)
    }

    /// Its port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Tasks brokered by every member (docs/pool.md §8, crate::agent::brokers): the members in
    /// touch and their brokers, as its agent read their heartbeats, and so the origins whose pages
    /// may call it (CORS). None: the switch off, the coordinator as it was.
    pub fn set_brokers(&self, b: Option<brokers::Brokers>) {
        *self.cors.write().unwrap_or_else(|e| e.into_inner()) = b.as_ref().map(|b| b.origins.clone());
        self.shared.lock().unwrap().brokers = b;
    }

    /// A coordinator kept on as its Mac's broker alone (docs/pool.md §8): a lead's that no longer
    /// leads (its tasks, and the workers holding them, going on), or a member's started for its own
    /// jobs' tasks. Nothing of the plan offered nor held: the jobs' leases are the lead's (handed
    /// over with the term), the asks its.
    pub fn become_broker(&self) {
        let mut s = self.shared.lock().unwrap();
        // (Its term's state no longer its to write: the new lead's, `keep`.)
        s.keep.off();
        s.pass.clear();
        s.set_offers(Vec::new());
        s.resume = None;
        s.moving = None;
        s.told = None;
        s.inputs_asks.clear();
        s.redo_asks.clear();
        s.done.clear();
        let gone = s.leases.drop_where(|l| matches!(l.work, Work::Job { .. }));
        if !gone.is_empty() {
            eprintln!(
                "coordinator: a broker now: {} job lease{} left to the lead",
                gone.len(),
                if gone.len() == 1 { "" } else { "s" }
            );
        }
        s.save_leases();
    }

    /// The workers around now (asked within two minutes), for the heartbeat: (name, worker).
    pub fn workers(&self) -> Vec<(String, Worker)> {
        let s = self.shared.lock().unwrap();
        let now = s.now();
        s.workers
            .iter()
            .filter(|(_, w)| now.saturating_duration_since(w.seen) < AROUND)
            .map(|(n, w)| (n.clone(), w.clone()))
            .collect()
    }
}

/// What a target is measured to take (`Coordinator::measured`): memory and disk (MB), and whether
/// its own run measured its memory (else its step's largest: a prior).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    pub mem_mb: u64,
    pub disk_mb: u64,
    pub own: bool,
}

/// `Coordinator::pool_state`, of its state as locked.
fn pool_state_of(s: &Shared) -> PoolState {
    let (now, unix) = (s.now(), unix_now());
    let failed = s
        .failed
        .iter()
        .map(|((w, k), (at, n))| (w.clone(), k.clone(), unix.saturating_sub(now.duration_since(*at).as_secs()), *n))
        .collect();
    PoolState {
        leases: s.leases.snapshot(),
        costs: s.costs.clone(),
        floors: s.floors.clone(),
        disks: s.disks.clone(),
        job_disks: s.job_disks.keys().map(|k| (k.clone(), s.job_p90(k))).collect(),
        job_peaks: s.job_disks.clone(),
        fills: s.fills.clone(),
        disks_v: DISKS_V,
        failed,
        pause: s.paused.clone(),
        pause_at: s.pause_at,
    }
}

/// The coordinator's state as the pool keeps it per term (`Coordinator::pool_state`):
/// `state/coord/term/<E>/state.json`, which the lead's next term loads (a restart's: its leases).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PoolState {
    #[serde(default)]
    pub leases: serde_json::Value,
    #[serde(default)]
    pub costs: BTreeMap<String, Cost>,
    /// The targets' floors (crate::agent::memguard: cost key → floor).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub floors: BTreeMap<String, Floor>,
    /// The targets' disk floors (cost key → MB).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub disks: BTreeMap<String, u64>,
    /// The 90th percentile of the disk each step's last jobs took (step → MB), for an older reader.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub job_disks: BTreeMap<String, u64>,
    /// The disk each step's last jobs took (step → MB, oldest first: `Shared::job_disks`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub job_peaks: BTreeMap<String, Vec<u64>>,
    /// What filling each shared cache takes (cache → MB: `Shared::fills`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fills: BTreeMap<String, u64>,
    /// The generation of its `disks` (`DISKS_V`; 0, of before it).
    #[serde(default)]
    pub disks_v: u32,
    /// (worker, cost key, when it last failed: unix seconds, how many times).
    #[serde(default)]
    pub failed: Vec<(String, String, u64, u32)>,
    #[serde(default)]
    pub pause: Option<crate::control::Pause>,
    #[serde(default)]
    pub pause_at: u64,
}

/// The owner's cancel (crate::control::Cancel) taken up in `s`: `Coordinator::cancel`, and the
/// `/work/cancel` of a member's agent.
fn cancel_in(s: &mut Shared, c: &crate::control::Cancel) -> Vec<(u64, String, String)> {
    let now = s.now();
    let hits: Vec<Lease> = s
        .leases
        .all(now)
        .into_iter()
        .filter(|l| match &l.work {
            Work::Job { step, targets } => {
                c.step.as_ref().is_none_or(|x| x == step) && targets.iter().any(|t| t.0 == c.target)
            }
            Work::Task { .. } => false,
        })
        .collect();
    let mut told = Vec::new();
    for l in hits {
        let Work::Job { step, .. } = &l.work else { continue };
        if c.rerun_after_update {
            s.leases.hold_for_update(step, &c.target, &c.by, c.at);
        }
        s.leases.ask_cancel(l.id, c.clone());
        s.history.add(history::Event {
            worker: Some(l.worker.clone()),
            lease: Some(l.id),
            step: Some(step.clone()),
            targets: vec![c.target.clone()],
            note: c.said(),
            ..history::Event::new("cancel")
        });
        eprintln!("coordinator: {} cancelled on {} ({})", l.what(), l.worker, c.said());
        told.push((l.id, l.worker.clone(), l.what()));
    }
    s.save_leases();
    told
}

/// Whether an agent on app `theirs` may build for a coordinator on `ours`: only the same. A target's
/// key is the coordinator's code's, its result the worker's: on another app, older or newer, the
/// two may differ (a step changed with its key, or without it), and the result would be recorded
/// under a key that says it's current. No list of what changed: any other app waits. `ours` empty
/// (tests): any.
fn app_ok(theirs: Option<&str>, ours: &str) -> bool {
    ours.is_empty() || theirs == Some(ours)
}

/// Whether app `a` was published after app `b` (both published: `20261005-1508-84142d3`, its UTC
/// publish time first); false when either isn't.
fn app_newer(a: &str, b: &str) -> bool {
    // (By bytes: a version is a worker's to say, and slicing a string mid-character would panic with
    // the coordinator's lock held.)
    let published = |v: &[u8]| {
        v.len() > 14 && v[8] == b'-' && v[13] == b'-' && v[..8].iter().chain(&v[9..13]).all(u8::is_ascii_digit)
    };
    published(a.as_bytes()) && published(b.as_bytes()) && a.as_bytes()[..13] > b.as_bytes()[..13]
}

/// An app version (`20261005-1508-84142d3`: its UTC publish time, then its commit) in words: "5 Oct
/// 15:08 UTC"; as it is when it isn't one.
fn app_when(v: &str) -> String {
    let p: Vec<&str> = v.split('-').collect();
    let month = |m: &str| {
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
            .get(m.parse::<usize>().ok()?.checked_sub(1)?)
            .copied()
    };
    match p.as_slice() {
        [d, t, _] if d.len() == 8 && t.len() == 4 && d.bytes().chain(t.bytes()).all(|b| b.is_ascii_digit()) => {
            match month(&d[4..6]) {
                Some(m) => format!("{} {m} {}:{} UTC", d[6..8].trim_start_matches('0'), &t[..2], &t[2..]),
                None => v.to_string(),
            }
        }
        _ => v.to_string(),
    }
}

/// The NAS budget's probes (crate::budget::nas), on a thread of their own while the coordinator
/// runs: while the NAS's project folder is known (`root`), each 10 s the switch is read; on, the NAS
/// is probed every `EVERY_S` (in a folder of this process's, removed after) and the limits set to
/// the best of the last day's probes, held shut while the NAS's own disk load read each round is
/// over the owner's target (nas::Loop); off, the limits are let go. A probe that fails leaves them.
/// The probes are kept on the NAS (`nas::PROBES_FILE`) and read once it's known (the day's best
/// outlive a restart and a change of lead).
fn nas_prober(
    shared: Arc<Mutex<Shared>>,
    root: Arc<Mutex<Option<store::nasfs::NasRoot>>>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
    me: String,
) {
    use crate::budget::nas;
    let run = move || {
        let mut probes = nas::Probes::default();
        // (The kept probes read: till then, read again each round and never saved over.)
        let mut loaded = false;
        let mut nas_loop = nas::Loop::default();
        // (Its config file removed when this thread ends: the lead stood down or stopped.)
        let mut session: Option<crate::budget::snmp::Session> = None;
        let mut told_unusable: Option<&'static str> = None;
        let mut last: Option<std::time::Instant> = None;
        let mut cleaned = false;
        let mut mounted: Option<std::time::Instant> = None;
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            let r = root.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(r) = r {
                // (Once the NAS is known: this host's folders a crashed probe left.)
                if !cleaned {
                    cleaned = true;
                    let n = nas::clean_stale(&r, &me);
                    if n > 0 {
                        eprintln!("coordinator: removed {n} probe folders of this host's a stopped process left");
                    }
                }
                if !loaded {
                    match nas::Probes::load(&r.join(nas::PROBES_FILE), unix_now()) {
                        Ok(kept) => {
                            loaded = true;
                            for p in kept.0 {
                                probes.add(p);
                            }
                            probes.0.sort_by_key(|p| p.at);
                        }
                        Err(e) => eprintln!("coordinator: the kept NAS probes can't be read now ({e}): trying again"),
                    }
                }
                // (Probed whatever the switch says: the lead's own jobs are always held to the
                // budget, `nas_own`; the members' only while it's on, `nas`.)
                match nas::read_switch(&r) {
                    None => eprintln!("coordinator: {} can't be read now: the NAS not probed this round", nas::SWITCH),
                    sw => {
                        let sw = sw.flatten();
                        let mut failed = None;
                        if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(nas::EVERY_S)) {
                            last = Some(std::time::Instant::now());
                            match nas::probe(&nas::probe_dir(&r, &me)) {
                                Ok(p) => {
                                    eprintln!(
                                        "coordinator: the NAS probed: {:.1} MB/s, {:.1} creates/s",
                                        p.bytes_s / 1e6,
                                        p.creates_s
                                    );
                                    probes.add(p);
                                    // (Not before the kept ones are read: never over them.)
                                    let saved = if loaded { probes.save(&r.join(nas::PROBES_FILE)) } else { Ok(()) };
                                    if let Err(e) = saved {
                                        eprintln!("coordinator: keeping the NAS probes: {e:#}");
                                    }
                                    failed = Some(false);
                                }
                                Err(e) => {
                                    eprintln!("coordinator: probing the NAS: {e}");
                                    failed = Some(true);
                                }
                            }
                        }
                        // (The NAS's own disk load against the owner's target, read outside the
                        // lock: up to snmp::TIMEOUT.)
                        let keys = crate::budget::snmp::read_keys(&r);
                        // (Keys that are there but unusable: said once, and again when that changes.)
                        let unusable = match keys {
                            Err(crate::budget::snmp::NoKeys::Unusable(k)) => Some(k),
                            _ => None,
                        };
                        if unusable != told_unusable {
                            if let Some(k) = unusable {
                                eprintln!(
                                    "coordinator: {k} has whitespace or '#', which net-snmp's conf can't carry: the NAS load gate is off"
                                );
                            }
                            told_unusable = unusable;
                        }
                        if keys.as_ref().map_or(true, |k| !session.as_ref().is_some_and(|s| s.is_for(k))) {
                            session = keys.ok().and_then(|k| {
                                crate::budget::snmp::Session::new(k)
                                    .inspect_err(|e| eprintln!("coordinator: no SNMP session: {e}"))
                                    .ok()
                            });
                        }
                        // (The mount, read outside the lock: statfs can wait on the NAS.)
                        let mount = if mounted.is_none_or(|t| t.elapsed() >= Duration::from_secs(MOUNT_EVERY_S)) {
                            mounted = Some(std::time::Instant::now());
                            // (The share's mount by its statfs: tests/nas_access.rs.)
                            Some(
                                store::sys::mount_of(&r.unchecked_path())
                                    .ok()
                                    .map(|(f, k, free)| NasMount::of(&f, &k, free)),
                            )
                        } else {
                            None
                        };
                        let mut disks = Vec::new();
                        match session.as_ref().map(|s| s.disk_busy()) {
                            Some(Ok(busy)) => {
                                nas_loop.read(&busy, nas::target_pct(nas::read_target(&r).as_ref()));
                                disks = busy;
                            }
                            Some(Err(e)) => {
                                if nas_loop.load.is_some() {
                                    eprintln!("coordinator: the NAS's disk load not read ({e}): the gate open");
                                }
                                nas_loop.lost();
                            }
                            None => nas_loop.lost(),
                        }
                        let held = nas_loop.held();
                        let gate = |l: nas::Limits| nas::Limits { held, ..l };
                        let own = probes.limits(sw.as_ref().and_then(|w| w.downloads)).map(gate);
                        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                        s.nas_loop = nas_loop;
                        s.nas_own = own;
                        s.nas_probes.clone_from(&probes.0);
                        // (The live store's NAS series, task #517: this reading's disks, and the
                        // pool's traffic.)
                        if let Some(l) = &s.live {
                            let t = unix_now();
                            l.record_disks(t, &disks);
                            let caps = s.capacities(&me);
                            if let Some(tr) = pool_traffic(&caps) {
                                l.record_pool(t, &tr);
                            }
                            if let Some(c) = pool_calls(&caps) {
                                l.record_pool_calls(t, &c);
                            }
                        }
                        s.nas_disks = disks;
                        if let Some(m) = mount {
                            s.nas_mount = m;
                        }
                        match failed {
                            Some(true) => s.nas_failures = s.nas_failures.saturating_add(1),
                            Some(false) => s.nas_failures = 0,
                            None => {}
                        }
                        s.nas = sw.and_then(|w| probes.limits(w.downloads)).map(gate);
                    }
                }
            }
            for _ in 0..10 {
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    if let Err(e) = std::thread::Builder::new().name("nas-probe".into()).spawn(run) {
        eprintln!("coordinator: no NAS probes: {e}");
    }
}

/// How often the lead reads what its timings log gained (`phase_sharer`).
const SHARES_EVERY_S: u64 = 60;

/// Keeps `into` to each step's phase shares over its last runs in the timings log at `log`, and
/// `times` to each step's time by phase over the page's chart ranges, read on its own thread every
/// `SHARES_EVERY_S` as it grows (crate::timings::LogTail: its size first, then only what was
/// added; the ranges summed again each time, as they move on), never under the pool's lock; till
/// the coordinator stops.
fn phase_sharer(
    log: PathBuf,
    into: Arc<Mutex<BTreeMap<String, crate::timings::PhaseShares>>>,
    times: Arc<Mutex<Option<Arc<crate::timings::PhaseTimes>>>>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
) {
    let run = move || {
        let mut tail = crate::timings::LogTail::default();
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            if tail.read(&log) {
                *into.lock().unwrap_or_else(|e| e.into_inner()) = tail.shares();
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            *times.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(tail.times(now)));
            for _ in 0..SHARES_EVERY_S {
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    if let Err(e) = std::thread::Builder::new().name("phase-shares".into()).spawn(run) {
        eprintln!("coordinator: no phase shares: {e}");
    }
}

/// Keeps `rec` in the coordinator's `dir`'s timings log.
fn keep_timings(dir: &Path, rec: &crate::timings::RunRec) {
    if let Err(e) = crate::timings::append(&dir.join("timings.jsonl"), rec) {
        eprintln!("coordinator: keeping a run's timings: {e}");
    }
}

/// A worker's name as a folder name.
pub fn folder(w: &str) -> String {
    w.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Whether `l` is a file a job of `step` saves for `target` (the steps table's:
/// crate::agent::steps::saves).
pub fn saves(step: &str, target: &str, l: &str) -> bool {
    crate::agent::steps::saves(step, target, l)
}

/// A job's hand-off, when it's its lease's: its done record is the lease's, every change is to one
/// of the files its step saves for one of the lease's targets (`saves`), each a content name of
/// that file, every upload it says it checked is one of its own, and each raw tiles' archive it put
/// on the NAS is named as packing names one for its shard. (Of any shard: a job packs every loose
/// tile in the helper's cache, an earlier job's left there too, a job stopped before it packed:
/// tiles of the NAS's own, whatever job fetched them.)
fn check_handoff(h: &Handoff, step: &str, targets: &[(String, String)]) -> Result<()> {
    // (Its lease's targets, or some of them: a job paused at a safe point hands off what it did.)
    match &h.done {
        Some((s, ts)) => anyhow::ensure!(
            s == step && !ts.is_empty() && ts.iter().all(|t| targets.contains(t)),
            "its done record isn't its lease's"
        ),
        None => anyhow::bail!("no done record"),
    }
    anyhow::ensure!(crate::agent::steps::SHARED.contains(&step), "a hand-off of {step} isn't work a worker does");
    for (shard, p) in &h.raw {
        anyhow::ensure!(crate::rawpack::is_shard(shard), "{shard} isn't a shard of raw tiles");
        anyhow::ensure!(crate::rawpack::named_for(&p.name, shard), "{} isn't an archive of {shard}", p.name);
    }
    // (Of the targets it says it did: one it didn't finish may have saved part of its files.)
    let did: Vec<&(String, String)> = h.done.as_ref().map(|d| d.1.iter().collect()).unwrap_or_default();
    for (l, v) in &h.changes {
        anyhow::ensure!(did.iter().any(|(t, _)| saves(step, t, l)), "{l} isn't one of the files of the targets it did");
        if let Some(c) = v {
            anyhow::ensure!(
                store::naming::parse_content_name(c).is_some_and(|n| n.logical == l),
                "{c} isn't a content name of {l}"
            );
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

/// A pooled job's end (its hand-off in the pool's journal, docs/pool.md §7.3, which the lead's
/// merge checks): what it says it did is its lease's step and some of its targets; any step, the
/// lead's own as a member's (a job running across a handover ends with the new lead); a job of no
/// targets of its own (the OSM pass, pass-sets) with no done record, as a resumed one granted its
/// pass as its target (crate::agent::resume).
fn check_pooled(h: &Handoff, step: &str, targets: &[(String, String)]) -> Result<()> {
    match &h.done {
        Some((s, ts)) => anyhow::ensure!(
            s == step && !ts.is_empty() && ts.iter().all(|t| targets.contains(t)),
            "its done record isn't its lease's"
        ),
        None => {
            anyhow::ensure!(targets.is_empty() || crate::agent::resume::RESUMABLE.contains(&step), "no done record")
        }
    }
    Ok(())
}

/// Where the disk each step's last jobs took is kept (`Shared::job_disks`): its v6, task #657.
const JOB_DISKS: &str = "job-disks-v6.json";

/// The file before `JOB_DISKS` (task #396's v5: its runs all of `pack` and `lo` as they run now),
/// kept whole; read once with `pack`'s and `lo`'s targets' disk measures (`JOB_DISKS_V4`'s gone
/// steps') dropped again: every take-up of a term's state (`load_pool_state`) had brought back
/// those of before task #396 (51–68 GB, the whole base packs' copy counted in), which no Mac's
/// room for jobs fits, and their z6 tiles were never granted (task #657: 9 of `pack` on 25 Oct).
const JOB_DISKS_V5: &str = "job-disks-v5.json";

/// The generation of the disk measures a term's state carries (`PoolState::disks_v`): 6 since
/// `JOB_DISKS`'s v6. A state of before it (0: none said) brings no `pack`'s nor `lo`'s targets'
/// disks to a take-up (`load_pool_state`).
pub const DISKS_V: u32 = 6;

/// The file before `JOB_DISKS` (task #392's v4: each step's last runs, step → MB), and the steps
/// whose runs in it and in the files before it (`JOB_DISKS_BEFORE`), and whose targets' disk
/// measures (`disks.json`), are no measure of a job now: `pack` and `lo`, whose jobs copied whole
/// base packs into a cache of their Mac's before task #396 (the copies counted in), and now read
/// only the parts they need into their own scratch.
const JOB_DISKS_V4: (&str, &[&str]) = ("job-disks-v4.json", &["pack", "lo"]);

/// The jobs' disks kept per step (`Shared::job_disks`).
pub const JOB_PEAKS: usize = 20;

/// Where the shared caches' fills are kept (`Shared::fills`).
const FILLS: &str = "cache-fills.json";

/// The least a run's disk past its targets' output counts as a fill of its cache (MB).
pub const FILL_MIN: u64 = 512;

/// The files before `JOB_DISKS` (each the most a job of each step took, step → MB), newest first,
/// each with the steps whose peaks in it are no measure of a job now: the halo steps'
/// (crate::needs::HALO_STEPS) from when a job of them spanned many halos (`pack` before task #260's
/// v2; `lo` before task #381's v3, its peak a whole pass's taken while its targets' disks were
/// summed); and `unit`'s in each, the fill of a per-Mac DEM cache task #595 deleted, no measure of
/// a job now.
const JOB_DISKS_BEFORE: [(&str, &[&str]); 3] = [
    ("job-disks-v3.json", &["unit"]),
    ("job-disks-v2.json", &["lo", "unit"]),
    ("job-disks.json", &["pack", "lo", "unit"]),
];

/// The 90th percentile of `step`'s targets' disk measures (MB; None: none).
fn targets_p90(disks: &BTreeMap<String, u64>, step: &str) -> Option<u64> {
    crate::needs::p90(
        disks.iter().filter(|(k, _)| crate::agent::memguard::step_of_key(k) == step).map(|(_, d)| *d).collect(),
    )
}

/// Whether a disk measure `mb` is no measure of a job whose targets measure `base` (MB): past
/// three times it and a GB (`stale_peak`, `remeasured`).
fn far_above(mb: u64, base: u64) -> bool {
    mb > base.saturating_mul(3) + 1024
}

/// Whether `mb`, a peak an older file kept of a job of `step` (one reading no shared cache: those
/// seed their cache's fill instead, `job_disks_load`), is far above what its targets measure (task
/// #392): past three times their 90th percentile and a GB. With few targets measured that's near
/// their most, so a peak within reason of the one job measured stays.
fn stale_peak(disks: &BTreeMap<String, u64>, step: &str, mb: u64) -> bool {
    targets_p90(disks, step).is_some_and(|p| far_above(mb, p))
}

/// The steps whose kept disk measures may hold a run that measured more than the job (task #398:
/// `depth-lo`, a 35 GB job peak against its targets' 65 MB, which kept its jobs on the lead's Mac).
const REMEASURED: [&str; 1] = ["depth-lo"];

/// `REMEASURED`'s job peaks and target measures far above what its targets measure (past three
/// times their median and a GB: an outlier or two can't lift a median as they lift a 90th
/// percentile) dropped, as each load reads them, so its jobs are sized by their measured use;
/// written without them at the next save. With no target measured, nothing dropped.
fn remeasured(job_disks: &mut BTreeMap<String, Vec<u64>>, disks: &mut BTreeMap<String, u64>) {
    for step in REMEASURED {
        let mut own: Vec<u64> =
            disks.iter().filter(|(k, _)| crate::agent::memguard::step_of_key(k) == step).map(|(_, d)| *d).collect();
        if own.is_empty() {
            continue;
        }
        own.sort_unstable();
        let far = |mb: u64| far_above(mb, own[own.len() / 2]);
        disks.retain(|k, mb| crate::agent::memguard::step_of_key(k) != step || !far(*mb));
        if let Some(runs) = job_disks.get_mut(step) {
            runs.retain(|mb| !far(*mb));
        }
    }
}

/// The disk each step's last jobs took, as kept in `dir`, and the shared caches' fills:
/// `JOB_DISKS` and `FILLS` (fills only of caches there are), else the file before them
/// (`JOB_DISKS_V5`) whole, `pack`'s and `lo`'s targets' measures dropped from `disks`, else
/// `JOB_DISKS_V4` without its gone steps' runs, their targets' measures dropped from `disks`
/// too, else the newest file before that (`JOB_DISKS_BEFORE`), each step's peak there one run,
/// without its stale and gone steps' and those far above their targets' measures (`stale_peak`,
/// by `disks`); a step reading a shared cache's peak there (the most a job took: a cold run's) its
/// cache's fill, less its targets' output (their 90th percentile), the most of its steps', and not
/// a run of it. Taken so once (`JOB_DISKS`, `FILLS` and `disks.json` written at once; with `keep`
/// alone: a viewer writes nothing).
fn job_disks_load(
    dir: &std::path::Path,
    disks: &mut BTreeMap<String, u64>,
    keep: bool,
) -> (BTreeMap<String, Vec<u64>>, BTreeMap<String, u64>) {
    // (Fills only of the caches there are: the base packs' went with task #396.)
    let fills: Option<BTreeMap<String, u64>> = std::fs::read(dir.join(FILLS))
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, u64>>(&b).ok())
        .map(|mut f| {
            f.retain(|c, _| crate::needs::CACHES.iter().any(|(n, _)| n == c));
            f
        });
    if let Some(mut m) = std::fs::read(dir.join(JOB_DISKS)).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        remeasured(&mut m, disks);
        return (m, fills.unwrap_or_default());
    }
    // (Before v6: pack's and lo's targets' disks dropped, written at once; before v5, their runs too.)
    let (v4, gone) = JOB_DISKS_V4;
    let before = disks.len();
    disks.retain(|k, _| !gone.contains(&crate::agent::memguard::step_of_key(k)));
    if keep && disks.len() < before {
        let _ = serde_json::to_vec(&*disks)
            .map_err(anyhow::Error::from)
            .and_then(|b| store::whole::write(&dir.join("disks.json"), &b));
    }
    let write = |m: &BTreeMap<String, Vec<u64>>, fills: &BTreeMap<String, u64>| {
        if !keep {
            return;
        }
        for (f, v) in [(JOB_DISKS, serde_json::to_vec(m)), (FILLS, serde_json::to_vec(fills))] {
            let _ = v.map_err(anyhow::Error::from).and_then(|b| store::whole::write(&dir.join(f), &b));
        }
    };
    if let Some(mut m) = std::fs::read(dir.join(JOB_DISKS_V5)).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        remeasured(&mut m, disks);
        let fills = fills.unwrap_or_default();
        write(&m, &fills);
        return (m, fills);
    }
    if let Some(mut m) =
        std::fs::read(dir.join(v4)).ok().and_then(|b| serde_json::from_slice::<BTreeMap<String, Vec<u64>>>(&b).ok())
    {
        m.retain(|step, _| !gone.contains(&step.as_str()));
        let fills = fills.unwrap_or_default();
        write(&m, &fills);
        return (m, fills);
    }
    for (f, stale) in JOB_DISKS_BEFORE {
        let stale: Vec<&str> = stale.iter().chain(gone).copied().collect();
        let Some(old) =
            std::fs::read(dir.join(f)).ok().and_then(|b| serde_json::from_slice::<BTreeMap<String, u64>>(&b).ok())
        else {
            continue;
        };
        let old: Vec<(String, u64)> = old.into_iter().filter(|(step, _)| !stale.contains(&step.as_str())).collect();
        let mut seeded: BTreeMap<String, u64> = BTreeMap::new();
        for (step, mb) in &old {
            if let Some(c) = crate::needs::cache_of(step) {
                let fill = mb.saturating_sub(targets_p90(disks, step).unwrap_or(0));
                let e = seeded.entry(c.to_string()).or_default();
                *e = (*e).max(fill);
            }
        }
        let fills = fills.unwrap_or(seeded);
        let m: BTreeMap<String, Vec<u64>> = old
            .into_iter()
            .filter(|(step, mb)| crate::needs::cache_of(step).is_none() && !stale_peak(disks, step, *mb))
            .map(|(step, mb)| (step, vec![mb]))
            .collect();
        write(&m, &fills);
        return (m, fills);
    }
    // (None before: v5 written now, so the drop above is done once.)
    let (m, fills) = (BTreeMap::new(), fills.unwrap_or_default());
    write(&m, &fills);
    (m, fills)
}

/// Whether cost key `k` is of a step's shape no plan makes now (task #392): a z3 tile's whole
/// `terrain-hi` (`terrain` before task #639) or `trees` run, before their z6 tiles; its measures
/// would hold the z6 tiles' priors.
fn old_shape(k: &str) -> bool {
    ["terrain 3/", "terrain-hi 3/", "trees 3/"].iter().any(|p| k.starts_with(p))
}

/// The floors `s` keeps, after clearing those of `step` (all, with no step), or of its `target`,
/// when `clear` (`scenic pool floors --clear`).
fn floors_cleared(s: &mut Shared, clear: bool, step: Option<&str>, target: Option<&str>) -> BTreeMap<String, Floor> {
    if clear {
        s.needs_rev += 1;
        let n = s.floors.len();
        let keep = |k: &String| match (step, target) {
            (None, _) => false,
            (Some(st), None) => crate::agent::memguard::step_of_key(k) != st,
            (Some(st), Some(t)) => *k != cost_key(st, t),
        };
        s.floors.retain(|k, _| keep(k));
        // (Their disk floors with them.)
        let d = s.disks.len();
        s.disks.retain(|k, _| keep(k));
        // (And the most its jobs took, a step's whole: cleared with its floors.)
        let j = s.job_disks.len();
        if target.is_none() {
            s.job_disks.retain(|st, _| step.is_some_and(|x| x != st));
        }
        if (s.disks.len() != d || s.job_disks.len() != j) && s.floors.len() == n {
            s.save_costs();
        }
        if s.floors.len() != n {
            eprintln!(
                "coordinator: {} floor{} cleared",
                n - s.floors.len(),
                if n - s.floors.len() == 1 { "" } else { "s" }
            );
            s.save_costs();
        }
    }
    s.floors.clone()
}

/// The build paused (how, by whom) or going on (`Coordinator::set_pause`, `/work/pause`), kept in
/// `pause.json` so the next agent starts with it.
fn set_pause(s: &mut Shared, pause: Option<crate::control::Pause>, at: u64) {
    if at < s.pause_at {
        eprintln!(
            "coordinator: an ask to {} from before the last change passed over",
            if pause.is_some() { "pause" } else { "go on" }
        );
        return;
    }
    s.pause_at = at;
    // (Every lease held a whole TTL again, as the build pauses or goes on: none lapses for the time it
    // was paused.)
    let now = s.now();
    s.leases.hold_all(now);
    if s.paused != pause {
        eprintln!("coordinator: {}", pause.as_ref().map_or("the build goes on".to_string(), |p| p.why()));
        let e = match &pause {
            Some(p) => history::Event {
                note: format!(
                    "{} ({})",
                    p.by,
                    if p.mode == crate::control::Mode::Freeze { "at once" } else { "at safe points" }
                ),
                ..history::Event::new("pause")
            },
            None => history::Event::new("resume"),
        };
        s.history.add(e);
    }
    let kept = serde_json::to_vec(&serde_json::json!({ "pause": pause, "at": at }))
        .map_err(anyhow::Error::from)
        .and_then(|b| store::whole::write(&s.dir.join("pause.json"), &b));
    if let Err(e) = kept {
        eprintln!("coordinator: keeping the pause: {e:#} (it holds until this agent stops)");
    }
    s.paused = pause;
}

/// The raw tiles' archives of a hand-off not taken, each well named for its shard, journaled on their
/// own (crate::handoff::RAW_AGAIN) for the next merge to name: they're on the NAS, and unnamed they'd
/// go two days later, their tiles fetched again.
fn raw_again(journal: &Path, h: Option<&Handoff>) {
    let raw: Vec<(String, crate::rawpack::Pack)> = h
        .into_iter()
        .flat_map(|h| h.raw.iter())
        .filter(|(a, p)| crate::rawpack::is_shard(a) && crate::rawpack::named_for(&p.name, a))
        .cloned()
        .collect();
    if raw.is_empty() {
        return;
    }
    if let Err(e) =
        crate::handoff::write(&journal.join(crate::handoff::RAW_AGAIN), &Handoff { raw, ..Default::default() })
    {
        eprintln!(
            "coordinator: a hand-off's raw tiles' archives not kept for naming ({e:#}): they go as unnamed ones do"
        );
    }
}

/// The page's views before this Mac's first read of the NAS (crate::agent::pageview): not ready.
/// How long a page's ask keeps the regions' steps counted (crate::agent's heartbeat counts them
/// again after a job ends, or each minute, while asked).
pub const REGION_STEPS_ASKING: Duration = Duration::from_secs(120);

fn not_ready() -> (u16, serde_json::Value) {
    (503, serde_json::json!({ "error": "this Mac hasn't read the build's state yet; ask again in a few seconds" }))
}

/// A page view this Mac's agent hasn't made yet (its loop busy, or just started): 200, not 503, so a
/// page's console shows no error while the agent is merely busy; `making` says why, for the page.
fn making(why: &str) -> (u16, serde_json::Value) {
    (200, serde_json::json!({ "making": why }))
}

/// What the page takes from a coordinator's memory (`Coordinator::view`): the pause, each lease and
/// worker, the tasks by state and `task_mb`, its times as unix seconds (`unix_of`).
fn view_of(s: &Shared) -> serde_json::Value {
    let now = s.now();
    let leases: Vec<serde_json::Value> = s
        .leases
        .all(now)
        .iter()
        .map(|l| {
            let (step, n) = match &l.work {
                Work::Job { step, targets } => (step.clone(), targets.len()),
                Work::Task { id } => (s.tasks.kind_of(*id), 1),
            };
            let lapses = unix_of(l.deadline());
            serde_json::json!({ "id": l.id, "worker": l.worker, "what": l.what(), "step": step, "n": n, "granted": unix_of(l.granted), "progress": l.progress, "frac": l.frac, "live": l.live, "beat": lapses.saturating_sub(TTL.as_secs()), "lapses": lapses })
        })
        .collect();
    let workers: Vec<serde_json::Value> = s
        .workers
        .iter()
        .map(|(n, w)| {
            let fit = w.ask.as_ref().map(|a| s.fit(a, now)).unwrap_or_default();
            // (`bad`, always false since results are no longer compared (task #876): the menu bar
            // app before it needs the field to read the workers at all. Remove it in the next
            // version.)
            serde_json::json!({ "name": n, "label": w.label, "kind": w.kind, "what": w.what, "mem_mb": w.mem_mb, "cores": w.cores, "done": w.done, "failed": w.failed, "bad": false, "app": w.app, "can": w.can, "cap": w.cap, "fit": fit, "seen": unix_of(w.seen) })
        })
        .collect();
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
    serde_json::json!({ "pause": s.paused, "leases": leases, "workers": workers, "tasks": tasks })
}

/// A view's leases and workers (`view_of`, or the lead's `view.json` read on another Mac) as the
/// page takes them at unix time `unix`: each time an age (a lease's `for_s`, `beat_s` and
/// `lapses_in_s`, a worker's `seen_s`) in place of its unix seconds.
pub fn ages(view: &serde_json::Value, unix: u64) -> (serde_json::Value, serde_json::Value) {
    let take = |o: &mut serde_json::Map<String, serde_json::Value>, k: &str| o.remove(k).and_then(|v| v.as_u64());
    let leases: Vec<serde_json::Value> = view["leases"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let mut o = l.as_object().cloned().unwrap_or_default();
            let (granted, beat, lapses) = (take(&mut o, "granted"), take(&mut o, "beat"), take(&mut o, "lapses"));
            o.insert("for_s".into(), unix.saturating_sub(granted.unwrap_or(unix)).into());
            o.insert("beat_s".into(), unix.saturating_sub(beat.unwrap_or(unix)).into());
            o.insert("lapses_in_s".into(), lapses.unwrap_or(unix).saturating_sub(unix).into());
            serde_json::Value::Object(o)
        })
        .collect();
    let workers: Vec<serde_json::Value> = view["workers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| {
            let mut o = w.as_object().cloned().unwrap_or_default();
            let seen = take(&mut o, "seen");
            o.insert("seen_s".into(), unix.saturating_sub(seen.unwrap_or(unix)).into());
            serde_json::Value::Object(o)
        })
        .collect();
    (leases.into(), workers.into())
}

/// This Mac's wall clock (unix s).
pub fn wall_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Shared {
    /// Why the pool's lead grants nothing now (`moving`), or its agent not heard from for
    /// `STALL_S` (`told`).
    fn moving_now(&self, now: u64) -> Option<String> {
        self.moving.clone().or_else(|| {
            self.told
                .filter(|t| now.saturating_sub(*t) > crate::pool::driver::STALL_S)
                .map(|t| format!("its pool not stepped for {} min: its view may be old", now.saturating_sub(t) / 60))
        })
    }
}

/// An `Instant`'s unix second, from one anchor (an `Instant` and the wall clock read together),
/// so a time already past says the same second each time it's asked; the anchor taken again only
/// when the wall clock has moved off it (a Mac asleep: its `Instant`s stop) by more than 2 s.
fn unix_of(t: Instant) -> u64 {
    static ANCHOR: Mutex<Option<(Instant, u64)>> = Mutex::new(None);
    let mut a = ANCHOR.lock().unwrap_or_else(|e| e.into_inner());
    let (i, u) = (Instant::now(), unix_now());
    let on = |a: (Instant, u64), t: Instant| match t.checked_duration_since(a.0) {
        Some(d) => a.1 + d.as_secs(),
        None => a.1.saturating_sub(a.0.duration_since(t).as_secs()),
    };
    if a.is_none_or(|a| on(a, i).abs_diff(u) > 2) {
        *a = Some((i, u));
    }
    on(a.unwrap(), t)
}

/// Seconds since the epoch.
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// How far ahead of this Mac's clock an agent's ask to pause may say it was made (the Macs' clocks
/// differ a little): later, it's taken as made a minute from now.
const AHEAD_S: u64 = 60;

/// Who's asking a request, as `http::gate` found: this Mac itself (`local`: from loopback, through
/// no proxy), with the build's own key (`owner`: the Macs' agents, this Mac's menu bar and
/// `scenic`), or a page (no key: the owner's asks alone); and from where
/// (crate::net::source).
#[derive(Clone, Debug, Default)]
struct Caller {
    local: bool,
    page: bool,
    from: String,
}

/// What a request says, as shown: no control or direction characters (a name that reads
/// backwards), `max` characters at most, trimmed.
fn plain(s: &str, max: usize) -> String {
    let bidi =
        |c: char| matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    s.chars().filter(|&c| !c.is_control() && !bidi(c)).take(max).collect::<String>().trim().to_string()
}

/// What a request asks of the shared state (JSON in, JSON out): (status, body).
fn route(
    path: &str,
    body: &[u8],
    shared: &Mutex<Shared>,
    journal: &Path,
    caller: &Caller,
) -> Result<(u16, serde_json::Value)> {
    let local = caller.local;
    let now = shared.lock().unwrap().now();
    // (While the build is paused, every lease is held, whatever went quiet meanwhile.)
    {
        let mut s = shared.lock().unwrap();
        if s.paused.is_some() {
            s.leases.hold_all(now);
        }
    }
    let ok = serde_json::json!({ "ok": true });
    match path {
        "/work/ask" => {
            let a: Ask = serde_json::from_slice(body)?;
            anyhow::ensure!(!a.worker.is_empty() && a.worker.len() <= 120, "a worker needs a name");
            let mut s = shared.lock().unwrap();
            // The pool's lead not fresh: nothing now (docs/pool.md §6.5).
            if let Some(why) = s.moving_now(wall_now()) {
                s.seen(&a.worker, format!("waiting: {why}"), None, now);
                return Ok((
                    409,
                    serde_json::json!({ "error": format!("the lead is moving ({why}); ask again in a moment") }),
                ));
            }
            // The build paused: nothing (an agent told why, and the pause).
            if let Some(p) = s.paused.clone() {
                s.seen(&a.worker, format!("waiting: {}", p.why()), None, now);
                return Ok((409, serde_json::json!({ "error": p.why(), "pause": p })));
            }
            // An agent on another app than this one's waits until the two run the same (its updater
            // run, or this Mac's: an agent switches between jobs, a lead re-asserting on its new app;
            // or the lead handed to a Mac on the newer): its work would be built with other code than
            // the keys it records say. (Before it's seen as a worker that does tasks: a `unit` job's
            // tails would wait for it.) Jobs it runs already go on.
            if !app_ok(a.app.as_deref(), &s.app) {
                let theirs = a.app.as_deref().map_or("an older one".to_string(), app_when);
                let why = if a.app.as_deref().is_some_and(|t| app_newer(t, &s.app)) {
                    format!("on the app of {theirs}, the lead on {}'s: it builds once the lead runs the same (its updater, or the lead handed to a Mac on the newer)", app_when(&s.app))
                } else {
                    format!(
                        "on the app of {theirs}, the lead on {}'s: it builds once it runs that one",
                        app_when(&s.app)
                    )
                };
                s.seen(&a.worker, why.clone(), None, now);
                return Ok((409, serde_json::json!({ "error": why })));
            }
            // Nor one older than the app a readers' switch was recorded on (its keys and code are
            // from before it: crate::inputs::regions::read::needed_app).
            if let Some(min) = s.min_app.clone() {
                let theirs = a.app.clone().unwrap_or_default();
                if !crate::pool::term::app_at_least(&theirs, &min) {
                    let why = format!("on the app of {}, older than the one the readers' switch was recorded on ({}): it builds once it runs that one or newer", if theirs.is_empty() { "an older one".to_string() } else { app_when(&theirs) }, app_when(&min));
                    s.seen(&a.worker, why.clone(), None, now);
                    return Ok((409, serde_json::json!({ "error": why })));
                }
            }
            s.seen(&a.worker, format!("asked for {}", a.can.join(" or ")), Some(&a), now);
            // The work only it can do first: a worker that mounts the NAS does a job of the plan (the
            // most work for what it fetches), the earliest step it can (what later steps wait on),
            // then a task; any other, a task.
            // A resumable job, to the worker whose Mac holds its progress alone, first: nothing
            // else would run there (crate::agent::resume).
            if let Some((_, o)) = s.resume.clone().filter(|(w, o)| *w == a.worker && a.can.contains(&o.step)) {
                let held = s.leases.held(&o.step, now);
                if !o.targets.iter().any(|(t, _, _)| held.contains(t)) {
                    let pick: Vec<(String, String)> =
                        o.targets.iter().map(|(t, k, _)| (t.clone(), k.clone())).collect();
                    let lease =
                        s.leases.grant(&a.worker, Work::Job { step: o.step.clone(), targets: pick.clone() }, now);
                    s.leases.set_threads(lease, ask_cores(&a));
                    s.save_leases();
                    eprintln!(
                        "coordinator: {} took {} {} (its progress is on its Mac)",
                        a.worker,
                        o.step,
                        pick.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" ")
                    );
                    s.history.add(history::Event {
                        worker: Some(a.worker.clone()),
                        lease: Some(lease),
                        step: Some(o.step.clone()),
                        targets: pick.iter().map(|t| t.0.clone()).collect(),
                        ..history::Event::new("lease")
                    });
                    let pass = pick.first().map(|t| t.0.clone()).unwrap_or_default();
                    let g = Grant {
                        lease,
                        term: s.leases.term,
                        ttl_s: TTL.as_secs(),
                        threads: ask_cores(&a),
                        disk_mb: s
                            .job_need(&o.step, &pick.iter().map(|t| t.0.clone()).collect::<Vec<_>>())
                            .map_or(0, |(m, _)| m.disk_mb),
                        work: Granted::Job { step: o.step.clone(), targets: pick, pass },
                        more: Vec::new(),
                        job: None,
                        plan: s.plan,
                    };
                    return Ok((200, serde_json::to_value(g)?));
                }
            }
            if !s.pass.is_empty() {
                // (The shared steps by their rank, then the others in plan order; the lead's own
                // planned jobs never: crate::agent::every::grants.)
                let mut offers: Vec<Offer> = s.offers.iter().filter(|o| a.can.contains(&o.step)).cloned().collect();
                offers.sort_by_key(|o| {
                    crate::agent::steps::SHARED.iter().position(|st| *st == o.step).unwrap_or(usize::MAX)
                });
                // (Placement on: the long poles' steps first, by their slack in the forecast; plan
                // order between equals.)
                if let Some((p, _)) = &s.placement {
                    offers.sort_by_key(|o| p.slack_of(&o.step));
                }
                // (As many jobs as the ask wants that fit its room together.)
                if let Some(g) = s.pack(&a, &offers, false, now) {
                    return Ok((200, serde_json::to_value(g)?));
                }
            }
            let picked = match &s.placement {
                Some(_) => s.pick_task_placed(&a, now),
                None => s.tasks.pick(&a.worker, &a.can, a.mem_mb),
            };
            if let Some(id) = picked {
                let lease = s.leases.grant(&a.worker, Work::Task { id }, now);
                // (Its id taken for good: kept, so no lease after a restart has it.)
                s.save_leases();
                let plan = s.plan;
                let t = s.tasks.by_id.get_mut(&id).unwrap();
                t.state = task::State::Leased { lease, worker: a.worker.clone() };
                t.leased_at = Some(now);
                let g = Grant {
                    lease,
                    term: 0,
                    ttl_s: TTL.as_secs(),
                    work: Granted::Task { id, task: t.spec.clone(), mem_mb: t.mem_mb },
                    more: Vec::new(),
                    threads: 0,
                    disk_mb: 0,
                    job: None,
                    plan,
                };
                eprintln!("coordinator: {} took task {id} ({})", a.worker, t.kind);
                return Ok((200, serde_json::to_value(g)?));
            }
            Ok((204, serde_json::Value::Null))
        }
        "/work/beat" => {
            let mut b: Beat = serde_json::from_slice(body)?;
            b.progress = b.progress.map(|p| p.chars().take(WHAT_MAX).collect());
            if let Some(l) = &mut b.live {
                for s in [&mut l.phase, &mut l.target].into_iter().flatten() {
                    *s = s.chars().take(WHAT_MAX).collect();
                }
            }
            let mut s = shared.lock().unwrap();
            let alive = s.leases.renew_frac(b.lease, &b.worker, b.progress.clone(), b.frac, b.live.take(), now);
            if let Some(k) = b.slot.filter(|_| alive) {
                s.leases.at_slot(b.lease, &b.worker, k);
            }
            s.seen(&b.worker, b.progress.unwrap_or_else(|| "working".into()), None, now);
            // (The build paused: its job pauses too, its lease kept meanwhile. The owner cancelled
            // it, `scenic job cancel`: it stops and gives the lease back.)
            let cancel = s.leases.cancel_ask(b.lease).filter(|_| alive).cloned();
            Ok((200, serde_json::json!({ "ok": alive, "pause": s.paused, "cancel": cancel })))
        }
        "/work/cancel" => {
            // A member's agent passing on its owner's `scenic job cancel` (crate::control::Cancel):
            // the leases told, by their worker.
            let mut c: crate::control::Cancel = serde_json::from_slice(body)?;
            (c.target, c.by) = (plain(&c.target, 200), plain(&c.by, WHAT_MAX));
            c.step = c.step.map(|st| plain(&st, 100));
            // (When it was asked, by the asker's clock: no later than AHEAD_S from now by this one's,
            // as a pause's.)
            c.at = c.at.min(unix_now() + AHEAD_S);
            let told = cancel_in(&mut shared.lock().unwrap(), &c);
            let told: Vec<serde_json::Value> = told
                .into_iter()
                .map(|(id, w, what)| serde_json::json!({ "lease": id, "worker": w, "what": what }))
                .collect();
            Ok((200, serde_json::json!({ "told": told })))
        }
        "/work/pause" => {
            // A Mac's ask (its menu, `scenic pause`, the map), passed on by its agent, or a page's
            // (its Pause): the build paused or going on, for every worker.
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let mut pause: Option<crate::control::Pause> = serde_json::from_value(b["pause"].clone())?;
            // (When it was asked: an agent says, by its Mac's clock, no later than AHEAD_S from now by
            // this one's; a page or an older helper doesn't, and it's now, by this Mac's: a browser's
            // clock isn't to be trusted with the order. A page's pause is by the build page.)
            let unix = unix_now();
            let at = match b["at"].as_u64().filter(|_| !caller.page) {
                Some(at) => at.min(unix + AHEAD_S),
                None => {
                    if let Some(p) = pause.as_mut() {
                        p.at = unix;
                    }
                    unix
                }
            };
            if let Some(p) = pause.as_mut() {
                p.at = p.at.min(unix + AHEAD_S);
                p.by = p.by.chars().take(WHAT_MAX).collect();
            }
            if let (Some(p), true) = (pause.as_mut(), caller.page) {
                p.by = format!("the build page ({})", caller.from);
            }
            // (Not the lead's: this Mac's agent passes it on, as its menu's.)
            let forward = shared.lock().unwrap().forward.clone();
            if let (Some(home), true) = (forward, caller.page) {
                crate::control::request(&home, pause)?;
                return Ok((200, ok));
            }
            set_pause(&mut shared.lock().unwrap(), pause, at);
            Ok((200, ok))
        }
        "/work/hold" => {
            // This Mac paused alone, or going on (docs/agent-parts.md §4.1, docs/pool.md §11):
            // `{"held": true | false}`, or another member's with `"member": <host or id>`. Asked of
            // this Mac's agent as `scenic pause --here` asks (its hold ask, the file an older CLI
            // writes too, which its control takes up the moment it lands), and answered as soon as
            // it is: `{"ok": …, "said": …}`, within a second for this Mac's own; past 2 s,
            // `{"asked": true}` (another member's waits for the agent's next round).
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let held = b["held"].as_bool().ok_or_else(|| anyhow::anyhow!("held: true or false"))?;
            let member = b["member"].as_str().map(|m| plain(m, 100));
            let Some((home, _)) = shared.lock().unwrap().settings.clone() else {
                return Ok((
                    503,
                    serde_json::json!({ "error": "this Mac's agent hasn't said where its folder is yet" }),
                ));
            };
            let by = if caller.page { format!("the build page ({})", caller.from) } else { caller.from.clone() };
            let ask =
                crate::control::request_hold(&home, member.as_deref(), held.then(|| crate::control::Hold::new(&by)))?;
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(2) {
                if let Some(a) = crate::control::hold_answer(&home, &ask) {
                    return Ok((200, serde_json::json!({ "ok": a.ok, "said": a.said })));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok((202, serde_json::json!({ "asked": true })))
        }
        "/work/quit" => {
            // The app quits (crate::control::Quit): `{"mode": "now" | "finish"}`, from this Mac only.
            // Asked of its agent as the app asks (the quit ask's file), taken up at once.
            if !caller.local {
                return Ok((403, serde_json::json!({ "error": "only this Mac quits its app" })));
            }
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let mode = match b["mode"].as_str() {
                Some("now") => crate::control::QuitMode::Now,
                Some("finish") => crate::control::QuitMode::Finish,
                _ => anyhow::bail!("mode: \"now\" or \"finish\""),
            };
            let Some((home, _)) = shared.lock().unwrap().settings.clone() else {
                return Ok((
                    503,
                    serde_json::json!({ "error": "this Mac's agent hasn't said where its folder is yet" }),
                ));
            };
            crate::control::request_quit(&home, mode, &caller.from)?;
            Ok((200, serde_json::json!({ "asked": true })))
        }
        "/work/settings" => {
            // The owner's settings from the build page (docs/workers.md §7): `{}` reads them;
            // `{"in_use": <cores> | "off"}`, `{"memory": <GB> | "off"}`, `{"room": <GB> | "off"}`,
            // set this Mac's, `{"nas_room": <GB> | "off"}` the pool's, each through the same function as
            // its `scenic` command (crate::agent::settings). Answered with them as they stand.
            use crate::agent::settings;
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let Some((home, root)) = shared.lock().unwrap().settings.clone() else {
                return Ok((
                    503,
                    serde_json::json!({ "error": "this Mac's agent hasn't said where its settings lie yet" }),
                ));
            };
            let by = format!("the build page ({})", caller.from);
            let said = |k: &str| -> Result<Option<String>> {
                Ok(match &b[k] {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(s) => Some(plain(s, 20)),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => anyhow::bail!("{k}: a number or \"off\""),
                })
            };
            if let Some(v) = said("in_use")? {
                let n = match v.as_str() {
                    "off" => None,
                    v => Some(v.parse::<u32>().map_err(|_| anyhow::anyhow!("in_use: cores or \"off\", not {v}"))?),
                };
                settings::set_in_use(&home, n, &by)?;
            }
            if let Some(v) = said("memory")? {
                settings::set_memory(&home, &v, &by)?;
            }
            if let Some(v) = said("room")? {
                settings::set_room(&home, &v, &by)?;
            }
            if let Some(v) = said("nas_room")? {
                let root =
                    root.as_ref().ok_or_else(|| anyhow::anyhow!("the NAS is away: its reserve can't be set now"))?;
                settings::set_nas_room(root, &v, &by)?;
            }
            let mut v = settings::view(&home, root.as_ref());
            v["mac"] = store::sys::host().into();
            Ok((200, v))
        }
        "/work/act" => {
            // A build page's ask (docs/pool.md §11): cancel a job
            // (`{"cancel": {"target": …, "step": …}}`) or try a failed target again now
            // (`{"retry": {…}}`), for its agent: acted on leading, else passed on to the lead.
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let by = format!("the build page ({})", caller.from);
            let one = |v: &serde_json::Value| -> Result<(String, Option<String>)> {
                let target =
                    v["target"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| anyhow::anyhow!("a target"))?;
                Ok((plain(target, 200), v["step"].as_str().map(|st| plain(st, 100))))
            };
            let act = match (b.get("cancel"), b.get("retry")) {
                (Some(c), None) => {
                    let (target, step) = one(c)?;
                    crate::pool::driver::Act::Cancel(crate::control::Cancel {
                        target,
                        step,
                        rerun_after_update: false,
                        by,
                        at: unix_now(),
                    })
                }
                (None, Some(r)) => {
                    let (target, step) = one(r)?;
                    crate::pool::driver::Act::Retry(crate::control::Retry { target, step, by, at: unix_now() })
                }
                _ => anyhow::bail!("{{\"cancel\": {{\"target\": …}}}} or {{\"retry\": {{\"target\": …}}}}"),
            };
            // Not the lead's: a cancel through this Mac's agent's own cancel asks (as `scenic job
            // cancel`: it tells the lead, whatever app the lead runs); a retry mailed to the lead,
            // refused now when it can't be (`retry_blocked`).
            let (forward, blocked) = {
                let s = shared.lock().unwrap();
                (s.forward.clone(), s.retry_blocked.clone())
            };
            match (&act, forward) {
                (crate::pool::driver::Act::Cancel(c), Some(home)) => {
                    crate::control::request_cancel(&home, c)?;
                    return Ok((200, ok));
                }
                (crate::pool::driver::Act::Retry(_), Some(_)) => {
                    if let Some(why) = blocked {
                        return Ok((409, serde_json::json!({ "error": format!("can't ask the lead: {why}") })));
                    }
                }
                _ => {}
            }
            shared.lock().unwrap().page_acts.push(act);
            Ok((200, ok))
        }
        "/pool/mail" => {
            // A member's messages to this Mac's member (docs/pool.md §9): taken only by the member
            // they're to, on its disk before the answer; else refused, sent by mail instead.
            let p: inbox::Post = serde_json::from_slice(body)?;
            let (me, inbox) = {
                let s = shared.lock().unwrap();
                (s.pool_member.clone(), s.inbox.clone())
            };
            match (me, inbox) {
                (Some(me), Some(i)) if me == p.to => {
                    anyhow::ensure!(p.msgs.len() <= inbox::KEPT, "at most {} messages a post", inbox::KEPT);
                    Ok((200, serde_json::json!({ "took": i.put(&p.from, p.msgs)? })))
                }
                _ => Ok((409, serde_json::json!({ "error": "not this member's mail" }))),
            }
        }
        "/work/inputs" => {
            // The build page's Accept and Accept All on a held unit's banner (docs/inputs.md §4.5):
            // `{"unit": <unit>, "accept": [<finding id>…]}` or `{"unit": <unit>, "all": true}`,
            // written by this Mac's agent (warnings alone: errors have no Accept).
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let unit = b["unit"]
                .as_str()
                .filter(|u| u.len() <= 64 && crate::inputs::valid_unit(u))
                .ok_or_else(|| anyhow::anyhow!("{{\"unit\": <unit>, \"accept\": [<id>…] | \"all\": true}}"))?;
            let accept: Vec<String> = b["accept"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|i| i.as_str().filter(|i| crate::inputs::valid_id(i)).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let all = b["all"].as_bool() == Some(true);
            // (And a member's agent's word that the inputs folders or acceptances changed, `check`, so
            // they're listed now; or its owner's ask to forget a z6 tile's records, `forget`: not a
            // page's.)
            let check = b["check"].as_bool() == Some(true);
            let full = check && b["full"].as_bool() == Some(true);
            let forget = b["forget"].as_bool() == Some(true) && !caller.page;
            anyhow::ensure!(all || !accept.is_empty() || check || forget, "nothing to accept");
            let by = if caller.page {
                format!("the build page ({})", caller.from)
            } else {
                format!("a member ({})", caller.from)
            };
            shared.lock().unwrap().inputs_asks.push(crate::inputs::Ask {
                unit: unit.to_string(),
                accept,
                all,
                check,
                full,
                forget,
                by,
                at: unix_now(),
                ..Default::default()
            });
            Ok((200, ok))
        }
        "/work/done" => {
            let mut d: Done = serde_json::from_slice(body)?;
            // Its timings kept, whatever became of its lease (by who ran it, as the lease says).
            if let Some(t) = d.timings.take() {
                let dir = shared.lock().unwrap().dir.clone();
                keep_timings(&dir, &crate::timings::RunRec { host: d.worker.clone(), ..t });
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
                Work::Job { step, targets } => {
                    let h = d.handoff.unwrap_or_default();
                    // (Its member wrote it to the pool's journal itself, which the lead's merge
                    // checks as any entry: here only that what it says it did is its lease's, its
                    // step any, a job of no targets of its own done with none.)
                    let pooled = d.journaled && s.leases.term > 0;
                    // Refused (422): the worker gives the lease back as failed, and drops the work
                    // (but for its raw tiles' archives, as above).
                    if let Err(e) =
                        if pooled { check_pooled(&h, step, targets) } else { check_handoff(&h, step, targets) }
                    {
                        // (From a worker on a newer app than this Mac's, whose step may save what
                        // this one's doesn't know of: its lease ends, not held against its targets.)
                        if s.workers
                            .get(&d.worker)
                            .is_some_and(|w| !s.app.is_empty() && w.app.as_deref() != Some(s.app.as_str()))
                        {
                            s.leases.finish(d.lease, &d.worker, now);
                            s.save_leases();
                        }
                        drop(s);
                        raw_again(journal, Some(&h));
                        return Ok((422, serde_json::json!({ "error": format!("{e:#}") })));
                    }
                    // Its lease ended and its z6 tiles kept out of offers now (those it did: a job paused
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
                    // (Its member wrote it to the pool's journal itself: nothing journaled here. Not
                    // while this coordinator's pool is off (switched off with the member's job
                    // under way): journaled, to be merged as any hand-off is.)
                    drop(s);
                    let wrote =
                        if pooled { Ok(()) } else { crate::handoff::write(&journal.join(folder(&d.worker)), &h) };
                    if let Err(e) = wrote {
                        let mut s = shared.lock().unwrap();
                        for (t, _) in &targets {
                            s.done.remove(&(step.clone(), t.clone()));
                        }
                        s.save_leases();
                        return Err(e.context("journal the hand-off"));
                    }
                    s = shared.lock().unwrap();
                    let by = d.worker.clone();
                    s.add_costs(
                        d.costs.into_iter().filter(|(u, _)| targets.iter().any(|t| *u == cost_key(step, &t.0))),
                        &by,
                    );
                    // (What the lease's targets took at least, those it didn't finish too.)
                    let floors: Vec<(String, Floor)> = d
                        .floors
                        .into_iter()
                        .filter(|(u, _)| lease_targets.iter().any(|t| *u == cost_key(step, &t.0)))
                        .collect();
                    s.add_floors(&floors);
                    let disks: Vec<(String, u64)> = d
                        .disks
                        .into_iter()
                        .filter(|(u, _)| {
                            lease_targets.iter().any(|t| *u == cost_key(step, &t.0))
                                || (lease_targets.is_empty() && crate::agent::memguard::step_of_key(u) == step)
                        })
                        .collect();
                    s.add_disks(&disks);
                    s.add_job_disk(step, d.job_disk_mb);
                    // (A fill only of a job that did all its targets: a paused one's run isn't its fill.)
                    if lease_targets.iter().all(|t| targets.contains(t)) {
                        let ts: Vec<String> = lease_targets.iter().map(|t| t.0.clone()).collect();
                        s.note_fill(step, &ts, d.job_disk_mb);
                    }
                    s.save_leases();
                    s.save_costs();
                    let secs = now.duration_since(l.granted).as_secs_f64();
                    s.history.add(history::Event {
                        worker: Some(d.worker.clone()),
                        lease: Some(d.lease),
                        step: Some(step.clone()),
                        targets: targets.iter().map(|t| t.0.clone()).collect(),
                        secs: Some(secs.round()),
                        ok: Some(!d.failed),
                        ..history::Event::new("done")
                    });
                }
                Work::Task { id } => {
                    let kind = s.tasks.kind_of(*id);
                    let unit = match s.tasks.done(d.lease, &d.worker, d.outputs, d.removed, d.secs, d.peak_mb, now) {
                        Ok(u) => u,
                        Err(e) => return Ok((422, serde_json::json!({ "error": format!("{e:#}") }))),
                    };
                    // One kept to measure its worker (the job ran it itself): its pace, and no more.
                    let measured = s.tasks.by_id.get(id).and_then(|t| t.measuring.zip(t.wall_s));
                    if let Some((here_s, wall_s)) = measured {
                        s.tasks.note_pace(&d.worker, &kind, wall_s, here_s);
                        s.tasks.close(*id);
                        eprintln!(
                            "coordinator: {} took {wall_s:.0} s for a {kind} this Mac took {here_s:.0} s for",
                            d.worker
                        );
                    }
                    // What its z6 tile's task takes, for the next time it's offered.
                    if let Some(u) = &unit {
                        s.costs.insert(
                            task_cost_key(&kind, u),
                            Cost {
                                peak_mb: d.peak_mb,
                                secs: d.secs as u64,
                                worker: Some(d.worker.clone()),
                                v: 0,
                                ..Default::default()
                            },
                        );
                        s.needs_rev += 1;
                        s.save_costs();
                    }
                    s.leases.finish(d.lease, &d.worker, now);
                    s.history.add(history::Event {
                        worker: Some(d.worker.clone()),
                        lease: Some(d.lease),
                        step: Some(kind),
                        targets: unit.into_iter().collect(),
                        secs: Some(d.secs.round()),
                        ok: Some(true),
                        ..history::Event::new("task")
                    });
                }
            }
            s.workers.get_mut(&d.worker).map(|w| w.done += 1);
            s.seen(&d.worker, format!("finished {}", l.what()), None, now);
            eprintln!("coordinator: {} finished {}", d.worker, l.what());
            Ok((200, ok))
        }
        "/work/fail" => {
            let mut f: Fail = serde_json::from_slice(body)?;
            f.error = f.error.chars().take(WHY_MAX).collect();
            let mut s = shared.lock().unwrap();
            let Some(l) = s.leases.finish(f.lease, &f.worker, now) else {
                return Ok((410, serde_json::json!({ "error": "that lease is gone" })));
            };
            let kind = match &l.work {
                Work::Task { id } => s.tasks.kind_of(*id),
                Work::Job { .. } => String::new(),
            };
            match &l.work {
                Work::Job { step, targets } => {
                    if !f.interrupted {
                        for (t, _) in targets {
                            let e = s.failed.entry((f.worker.clone(), cost_key(step, t))).or_insert((now, 0));
                            *e = (now, e.1 + 1);
                        }
                    }
                    // (What its targets took at least: a job stopped past its Mac's limit.)
                    let floors: Vec<(String, Floor)> = f
                        .floors
                        .iter()
                        .filter(|(u, _)| targets.iter().any(|t| *u == cost_key(step, &t.0)))
                        .cloned()
                        .collect();
                    let disks: Vec<(String, u64)> = f
                        .disks
                        .iter()
                        .filter(|(u, _)| {
                            targets.iter().any(|t| *u == cost_key(step, &t.0))
                                || (targets.is_empty() && crate::agent::memguard::step_of_key(u) == step)
                        })
                        .cloned()
                        .collect();
                    if s.add_floors(&floors) | s.add_disks(&disks) | s.add_job_disk(step, f.job_disk_mb) {
                        s.save_costs();
                    }
                    s.save_leases();
                }
                Work::Task { .. } => {
                    if let (Some(id), Some(peak)) = (
                        s.tasks.fail_how(f.lease, &f.worker, &f.error, f.oom_mb, f.interrupted),
                        f.oom_mb.filter(|_| !f.interrupted),
                    ) {
                        // Out of memory at `peak`: it takes more than that, next time too.
                        if let Some(u) =
                            s.tasks.by_id.get(&id).and_then(|t| t.spec["unit"].as_str().map(str::to_string))
                        {
                            let c = s.costs.entry(task_cost_key(&kind, &u)).or_default();
                            c.peak_mb = c.peak_mb.max(peak + peak / 4);
                            s.needs_rev += 1;
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
                Work::Job { step, targets } => history::Event {
                    step: Some(step.clone()),
                    targets: targets.iter().map(|t| t.0.clone()).collect(),
                    ..history::Event::new("fail")
                },
                // (A task given back, not failed: "task-back", #703.)
                Work::Task { .. } => history::Event {
                    step: Some(kind.clone()),
                    ..history::Event::new(if f.interrupted { "task-back" } else { "task-fail" })
                },
            };
            let note = if f.interrupted { format!("stopped: {why}") } else { why.clone() };
            let back = e.kind == "task-back";
            s.history.add(history::Event {
                worker: Some(f.worker.clone()),
                lease: Some(f.lease),
                secs: Some(now.duration_since(l.granted).as_secs_f64().round()),
                ok: (!back).then_some(false),
                note: if back { why.clone() } else { note },
                ..e
            });
            let how = if f.interrupted { "gave back" } else { "failed" };
            s.seen(&f.worker, format!("{how} {}: {why}", l.what()), None, now);
            eprintln!("coordinator: {} {how} {}: {why}", f.worker, l.what());
            Ok((200, ok))
        }
        "/work/status" => {
            let s = shared.lock().unwrap();
            let leases: Vec<serde_json::Value> = s
                .leases
                .all(now)
                .iter()
                .map(|l| {
                    let step = match &l.work {
                        Work::Job { step, .. } => step.clone(),
                        Work::Task { id } => s.tasks.kind_of(*id),
                    };
                    serde_json::json!({ "id": l.id, "worker": l.worker, "what": l.what(), "step": step, "for_s": now.duration_since(l.granted).as_secs(), "progress": l.progress })
                })
                .collect();
            let workers: BTreeMap<&String, serde_json::Value> = s
                .workers
                .iter()
                .map(|(n, w)| (n, serde_json::json!({ "worker": w, "seen_s": now.duration_since(w.seen).as_secs() })))
                .collect();
            let tasks: Vec<serde_json::Value> = s.tasks.by_id.values().map(|t| serde_json::json!({ "id": t.id, "kind": t.kind, "mem_mb": t.mem_mb, "state": format!("{:?}", t.state).split([' ', '{']).next().unwrap_or("") })).collect();
            let mut offered: BTreeMap<&str, usize> = BTreeMap::new();
            // (A step's offers summed: a halo step's are one a halo.)
            for o in &s.offers {
                *offered.entry(o.step.as_str()).or_default() += o.targets.len();
            }
            Ok((
                200,
                serde_json::json!({ "pass": s.pass, "paused": s.paused, "offered": offered, "done": s.done.len(), "leases": leases, "workers": workers, "tasks": tasks }),
            ))
        }
        "/work/swarm" => {
            // (In the pool, from this Mac's read of the NAS: the same on every Mac; none yet, not
            // ready. Never the memory's meanwhile: its numbering isn't the read's.)
            // (The pause, on the lead (nothing to forward: its pages' asks are set here), from its
            // memory: its NAS mirror waits for the agent's next loop, many seconds on a busy lead.)
            let (pv, own) = {
                let s = shared.lock().unwrap();
                (s.pageview.clone().filter(|p| p.active()), s.forward.is_none().then(|| s.paused.clone()))
            };
            if let Some(p) = pv {
                return Ok(p.swarm().map_or_else(not_ready, |mut a| {
                    if let Some(own) = own {
                        a["pause"] = serde_json::to_value(own).unwrap_or_default();
                    }
                    (200, a)
                }));
            }
            // The whole build at a glance, for the worker page: the lead's heartbeat (its
            // agent's status, written each loop beside this folder: its job, the checklist to the
            // end, the forecast, the regions, its helpers), the leases, the workers, the tasks and
            // the history's hours.
            let s = shared.lock().unwrap();
            let agent: serde_json::Value = s
                .dir
                .parent()
                .and_then(|h| std::fs::read(h.join("status.json")).ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(serde_json::Value::Null);
            // Each lease (whose, what, for how long, what it last said and how far, since its last
            // beat and until it lapses), each worker (and for one that does the shared steps, how
            // the work offered fits it), the tasks by state: `view_of`, its times as ages.
            let view = view_of(&s);
            let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            // The history's last number (the page asks `/work/history` for what's after the one it
            // has) and the last day by the hour.
            let (leases, workers) = ages(&view, unix);
            Ok((
                200,
                serde_json::json!({ "now": unix, "pause": s.paused, "agent": agent, "leases": leases, "workers": workers, "tasks": view["tasks"], "seq": s.history.seq(), "rates": s.history.rates(unix, 24), "app": s.app, "epoch": format!("coordinator {}", s.lead_name) }),
            ))
        }
        "/work/history" => {
            // What happened after event `since` (the oldest first, at most `max`, 500 by default).
            let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let since = b["since"].as_u64().unwrap_or(0);
            let max = b["max"].as_u64().map_or(500, |m| m.min(5000)) as usize;
            let pv = shared.lock().unwrap().pageview.clone().filter(|p| p.active());
            if let Some(p) = pv {
                return Ok(p.history(since, max).map_or_else(not_ready, |a| (200, a)));
            }
            let s = shared.lock().unwrap();
            Ok((200, serde_json::json!({ "seq": s.history.seq(), "events": s.history.since(since, max) })))
        }
        "/work/live" => {
            // This Mac's measures over time (crate::agent::live): a window, or the points since.
            let a: crate::agent::live::Ask = serde_json::from_slice(body).unwrap_or_default();
            let live = shared.lock().unwrap().live.clone();
            let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            Ok(live.map_or_else(not_ready, |l| (200, l.answer(&a, unix))))
        }
        "/work/catalogs" => {
            // The build page's Catalogues map (crate::agent::cataloguesview): the catalogues after
            // `after` as read so far (its reader started, the folder listed again, when due).
            let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let cv = shared.lock().unwrap().catalogues.clone();
            Ok(cv.map_or_else(not_ready, |cv| (200, cv.ask(b["after"].as_u64()))))
        }
        "/work/built" => {
            // The build page's map's finished tiles (crate::agent::builtview): every tile target the
            // build's keys record, by step, as last read.
            let bv = shared.lock().unwrap().built.clone();
            Ok(bv.map_or_else(not_ready, |bv| (200, bv.ask())))
        }
        "/work/tocome" => {
            // Every Mac's tiles to come in the lead's last forecast (crate::agent::forecast::ToCome):
            // the build page's map's queue, asked once a forecast.
            // None made (a member's, or the lead's before its first): 200 with why (`making`), not
            // an error: the page asks the lead, or waits.
            // With the history's `epoch`, the numbering its `after` is in: the page's history's,
            // a pageview's (its `after` renumbered there from this coordinator's, as this
            // member wrote them: crate::agent::pageview) when it serves the page.
            // `{"raw": true}` (a member's page asking the lead): `after` as this coordinator wrote
            // it, with `by`, the member name its numbers are under; the page gives them to its own
            // Mac's pageview as `{"renumber": {"by", "after"}}`, answered `{epoch, after}` in that
            // pageview's numbering (`epoch` null when it can't).
            let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let (tc, epoch, pv, member) = {
                let s = shared.lock().unwrap();
                let pv = s.pageview.clone().filter(|p| p.active());
                (s.tocome.clone(), format!("coordinator {}", s.lead_name), pv, s.pool_member.clone())
            };
            if let Some(r) = b.get("renumber") {
                let by = r["by"].as_str().unwrap_or("");
                let after: Vec<Vec<u64>> = serde_json::from_value(r["after"].clone()).unwrap_or_default();
                return Ok(match pv.and_then(|p| p.renumber(by, &after)) {
                    Some((e, after)) => (200, serde_json::json!({ "epoch": e, "after": after })),
                    None => (200, serde_json::json!({ "epoch": null })),
                });
            }
            let raw = b["raw"].as_bool() == Some(true);
            // (In the pool, this Mac's pageview's, of the forecast it runs for the swarm's answer
            // (task #836): its `at` the page's status's (`{"at"}`, the one the page read), on the
            // lead and every member alike, so no page needs another Mac's port.)
            if let Some(a) = pv.as_ref().filter(|_| !raw).and_then(|p| p.tocome(b["at"].as_u64())) {
                return Ok((200, a));
            }
            Ok(tc.map_or_else(
                || making("this Mac has made no forecast yet"),
                |t| {
                    let mut a = t.answer();
                    let (epoch, after) = match &pv {
                        _ if raw => {
                            a["by"] = member.into();
                            (Some(epoch), None)
                        }
                        None => (Some(epoch), None),
                        Some(p) => match member.as_deref().and_then(|m| p.renumber(m, &t.runs.after)) {
                            Some((e, after)) => (Some(e), Some(after)),
                            None => (None, Some(Vec::new())),
                        },
                    };
                    if let Some(after) = after {
                        a["after"] = serde_json::json!(after);
                    }
                    a["epoch"] = epoch.into();
                    (200, a)
                },
            ))
        }
        "/work/regions" => {
            // With `steps`: each region's targets built of all by step, as this Mac's agent last
            // counted them (the lead's, or out of the pool; asked again: counted while asked).
            let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            if b["steps"].as_bool() == Some(true) {
                let mut sh = shared.lock().unwrap();
                sh.region_steps_asked = Some(Instant::now());
                return Ok(sh.region_steps.clone().map_or_else(
                    || making("this Mac's agent is counting the regions' steps"),
                    |v| (200, (*v).clone()),
                ));
            }
            // The build page's regions (crate::agent::regionsview): in the pool, the lead's as this
            // Mac read them; out of it, this Mac's agent's.
            let pv = shared.lock().unwrap().pageview.clone().filter(|p| p.active());
            let v = match pv {
                Some(p) => p.regions(),
                None => shared.lock().unwrap().regions.clone(),
            };
            Ok(v.map_or_else(|| making("this Mac's agent is reading the regions"), |v| (200, (*v).clone())))
        }
        "/work/steps" => {
            // The steps table (crate::agent::steps, docs/pipeline.md): every step's and task's row
            // and the stages, for the page's words and colours (read once a load).
            Ok((
                200,
                serde_json::json!({ "steps": crate::agent::steps::table(), "stages": crate::agent::steps::STAGES }),
            ))
        }
        "/work/redo" => {
            // The owner's ask to build `step`'s finished `targets` again (`scenic redo`, with the
            // build's key; crate::agent::redo): kept by this Mac's agent, which runs it as a job of
            // its own. Its number answered, and the last lease granted before it (`after`: its job's
            // lease is later, so `scenic redo --run` knows its merge from an earlier job's). (Never a
            // page's: the gate lets only the key through.)
            let b: serde_json::Value = serde_json::from_slice(body)?;
            let targets: Vec<String> = b["targets"]
                .as_array()
                .map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            // (A restore's (`scenic redo --restore`, crate::agent::restore): each target with the key
            // recorded and the one to record in its place.)
            let restore: Vec<crate::agent::redo::Restore> =
                serde_json::from_value(b["restore"].clone()).unwrap_or_default();
            let (step, targets, restore) = if restore.is_empty() {
                let (s, t) = crate::agent::redo::checked(b["step"].as_str().unwrap_or(""), &targets)?;
                (s, t, restore)
            } else {
                let (s, r) = crate::agent::redo::checked_restore(b["step"].as_str().unwrap_or(""), &restore)?;
                (s, r.iter().map(|x| x.target.clone()).collect(), r)
            };
            let by = b["by"].as_str().unwrap_or("").chars().take(100).collect();
            let mut s = shared.lock().unwrap();
            // (Its number unique in this coordinator: ms, one more than the last.)
            let n = (unix_now() * 1000).max(s.redo_asks.iter().map(|a| a.n + 1).max().unwrap_or(0));
            s.redo_asks.push(crate::agent::redo::Ask { n, step, targets, by, restore });
            let after = crate::pool::journal::LeaseId { term: s.leases.term, n: s.leases.next() - 1 };
            Ok((200, serde_json::json!({ "n": n, "after": { "term": after.term, "n": after.n } })))
        }
        "/work/floors" => {
            // The targets' floors (crate::agent::memguard), cleared (`{"clear": true}`, of a `step`,
            // of its `target`) first when asked: `scenic pool floors`.
            let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let mut s = shared.lock().unwrap();
            let (n, d) = (s.floors.len(), s.disks.len());
            let floors =
                floors_cleared(&mut s, b["clear"].as_bool() == Some(true), b["step"].as_str(), b["target"].as_str());
            // (What the clear took out of this coordinator's memory, for the CLI to say.)
            let cleared = serde_json::json!({ "floors": n - floors.len(), "disks": d - s.disks.len() });
            Ok((200, serde_json::json!({ "floors": floors, "disks": s.disks, "cleared": cleared })))
        }
        "/work/timings" => {
            // The runs' timings kept (crate::timings), the last `last` of each kind (or of `kind`):
            // `scenic timings` on another Mac.
            let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let last = b["last"].as_u64().map_or(20, |m| m.min(500)) as usize;
            let dir = shared.lock().unwrap().dir.clone();
            Ok((
                200,
                serde_json::json!({ "runs": crate::timings::last_runs(&crate::timings::read_log(&dir.join("timings.jsonl")), b["kind"].as_str(), last) }),
            ))
        }
        p if p.starts_with("/task/") && !local => anyhow::bail!("{p} is for this Mac's jobs"),
        "/task/offer" => {
            let mut o: task::Offer = serde_json::from_slice(body)?;
            let mut s = shared.lock().unwrap();
            // What its z6 tile's task took last time (a worker's measure, with some room), in place of
            // the guess.
            if let Some(c) =
                o.spec["unit"].as_str().and_then(|u| s.costs.get(&task_cost_key(&o.kind, u))).filter(|c| c.peak_mb > 0)
            {
                o.mem_mb = c.peak_mb + c.peak_mb / 10;
            }
            let id = s.tasks.offer(o, now)?;
            Ok((200, serde_json::json!({ "id": id })))
        }
        "/task/workers" => {
            // Who's around to take tasks of a kind, and the most memory one spares.
            let kind: String = serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v["kind"].as_str().map(str::to_string))
                .unwrap_or_default();
            let s = shared.lock().unwrap();
            let around: Vec<&Worker> =
                s.workers.values().filter(|w| now.duration_since(w.seen) < AROUND && w.can.contains(&kind)).collect();
            Ok((
                200,
                serde_json::json!({ "workers": around.len(), "mem_mb": around.iter().map(|w| w.mem_mb).max().unwrap_or(0) }),
            ))
        }
        p if p.starts_with("/task/") => {
            // /task/<id>, /task/<id>/withdraw, /task/<id>/close
            let mut parts = p["/task/".len()..].split('/');
            let id: u64 = parts.next().unwrap_or("").parse()?;
            let mut s = shared.lock().unwrap();
            match parts.next() {
                None => {
                    let Some(t) = s.tasks.by_id.get(&id) else {
                        return Ok((404, serde_json::json!({ "error": "no such task" })));
                    };
                    let v = match &t.state {
                        // (With how many workers asking lately could take it now: a job about
                        // to take it back gives them a moment, crate::offload.)
                        task::State::Offered => serde_json::json!({ "state": "offered", "takers": s.takers(t, now) }),
                        // (With how long it's been held and how far it is, as its worker says: a
                        // job waits for it only while it'll be back sooner than its own run.)
                        task::State::Leased { worker, lease } => {
                            let l = s.leases.of(*lease);
                            serde_json::json!({ "state": "leased", "worker": worker, "age_s": l.map(|l| now.duration_since(l.granted).as_secs_f64()), "frac": l.and_then(|l| l.frac), "pace": s.tasks.pace(worker, &t.kind) })
                        }
                        task::State::Done { worker, outputs, removed, secs, peak_mb } => {
                            serde_json::json!({ "state": "done", "worker": worker, "out": t.out, "outputs": outputs, "removed": removed, "secs": secs, "peak_mb": peak_mb })
                        }
                        task::State::Failed { why } => serde_json::json!({ "state": "failed", "why": why }),
                    };
                    Ok((200, v))
                }
                Some("withdraw") => Ok((200, serde_json::json!({ "withdrawn": s.tasks.withdraw(id) }))),
                Some("explore") => {
                    // The job waits for the workers that could take it whose pace isn't measured:
                    // not again for an hour each.
                    let Some(t) = s.tasks.by_id.get(&id) else {
                        return Ok((404, serde_json::json!({ "error": "no such task" })));
                    };
                    let names: Vec<String> = s
                        .takers(t, now)
                        .iter()
                        .filter(|w| w["explore"] == true)
                        .filter_map(|w| w["worker"].as_str().map(str::to_string))
                        .collect();
                    for n in &names {
                        s.tasks.explore(n, now);
                    }
                    Ok((200, serde_json::json!({ "workers": names })))
                }
                Some("close") => {
                    // With the job's own run's time (or what it takes it to be): the worker's pace.
                    let b: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
                    let here_s = b["here_s"].as_f64();
                    if let (Some(here_s), Some(t)) = (here_s, s.tasks.by_id.get(&id)) {
                        let kind = t.kind.clone();
                        s.tasks.note_here(&kind, here_s);
                        let t = &s.tasks.by_id[&id];
                        if let (task::State::Done { worker, .. }, Some(wall_s)) = (t.state.clone(), t.wall_s) {
                            let kind = t.kind.clone();
                            s.tasks.note_pace(&worker, &kind, wall_s, here_s);
                        }
                    }
                    // Run by the job while a worker whose pace isn't measured holds it: kept for that
                    // worker to finish, to measure it.
                    if here_s.is_some_and(|h| s.tasks.measure(id, h)) {
                        return Ok((200, serde_json::json!({ "ok": true, "measuring": true })));
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

/// The HTTP side: axum, on a runtime of its own. Every request is bounded in
/// time and size, a slow or stalled one holds a task (not a thread), and files stream both ways.
mod http {
    use super::*;
    use axum::body::Body;
    use axum::extract::{ConnectInfo, Path as Url, Request, State};
    use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
    use axum::middleware::Next;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{any, get, put};
    use axum::{Json, Router};
    use std::net::SocketAddr;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};

    /// The build page (docs/workers.md §7), built in: (path under /work/, type, contents). An app to
    /// install (its manifest, service worker and icons: docs/workers.md §7, The page as an app).
    const PAGE: &[(&str, &str, &[u8])] = &[
        ("", "text/html; charset=utf-8", include_bytes!("../../../../web/work/index.html")),
        ("index.html", "text/html; charset=utf-8", include_bytes!("../../../../web/work/index.html")),
        // The build at a glance, both pages': its script and styles.
        ("dash.js", "text/javascript", include_bytes!("../../../../web/work/dash.js")),
        ("pool.js", "text/javascript", include_bytes!("../../../../web/work/pool.js")),
        ("sums.js", "text/javascript", include_bytes!("../../../../web/work/sums.js")),
        ("settings.js", "text/javascript", include_bytes!("../../../../web/work/settings.js")),
        // Live on each Mac (the Machines tab), its charts and their sums; the Activity tab's transfers.
        ("live.js", "text/javascript", include_bytes!("../../../../web/work/live.js")),
        ("nas.js", "text/javascript", include_bytes!("../../../../web/work/nas.js")),
        ("activity.js", "text/javascript", include_bytes!("../../../../web/work/activity.js")),
        ("chart.js", "text/javascript", include_bytes!("../../../../web/work/chart.js")),
        ("chartmath.js", "text/javascript", include_bytes!("../../../../web/work/chartmath.js")),
        // The Jobs tab's schedule: its view, gestures and arrows (task #787).
        ("timeline.js", "text/javascript", include_bytes!("../../../../web/work/timeline.js")),
        ("dash.css", "text/css; charset=utf-8", include_bytes!("../../../../web/work/dash.css")),
        // The overview's world map, and the world's countries' outlines it draws (Natural Earth
        // 1:50m, public domain): rings in Web Mercator, the world 0–1000 each way.
        ("worldmap.js", "text/javascript", include_bytes!("../../../../web/work/worldmap.js")),
        // Its Catalogues tab: the range's and the tiles' sums.
        // the served name keeps its old spelling (task #547: a page from an older lead asks for it)
        ("catalogs.js", "text/javascript", include_bytes!("../../../../web/work/catalogues.js")),
        ("countries.json", "application/json", include_bytes!("../../../../web/work/countries.json")),
        // The large lakes it draws as the sea (Natural Earth 1:10m lakes, scalerank ≤ 3, simplified,
        // the same rings; the NAS's sources/fetched/natural-earth-lakes-10m, task #831).
        ("lakes.json", "application/json", include_bytes!("../../../../web/work/lakes.json")),
        ("sw.js", "text/javascript", include_bytes!("../../../../web/work/sw.js")),
        (
            "manifest.webmanifest",
            "application/manifest+json",
            include_bytes!("../../../../web/work/manifest.webmanifest"),
        ),
        ("icons/icon-192.png", "image/png", include_bytes!("../../../../web/work/icons/icon-192.png")),
        ("icons/icon-512.png", "image/png", include_bytes!("../../../../web/work/icons/icon-512.png")),
        (
            "icons/icon-maskable-512.png",
            "image/png",
            include_bytes!("../../../../web/work/icons/icon-maskable-512.png"),
        ),
        ("icons/apple-touch-icon.png", "image/png", include_bytes!("../../../../web/work/icons/apple-touch-icon.png")),
    ];

    /// The page's version: its files' hash, in its service worker (sw.js's VERSION), so a new
    /// app's page is a new service worker, which takes over.
    pub(super) fn page_version() -> &'static str {
        static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        V.get_or_init(|| {
            store::naming::hash16(
                &PAGE.iter().flat_map(|(n, _, b)| n.as_bytes().iter().chain(b.iter()).copied()).collect::<Vec<u8>>(),
            )
        })
    }

    /// The page's files' addresses for its service worker (sw.js's PAGE, kept as it installs): all
    /// but the worker itself.
    pub(super) fn page_list() -> String {
        let v: Vec<String> =
            PAGE.iter().filter(|(n, _, _)| *n != "sw.js").map(|(n, _, _)| format!("/work/{n}")).collect();
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
        /// Held read by each request's `route`, true once the coordinator has stopped
        /// (Coordinator::stop): then none runs.
        pub routes: Arc<std::sync::RwLock<bool>>,
        /// The origins whose pages may call it from elsewhere (Coordinator::set_brokers).
        pub cors: Arc<std::sync::RwLock<Option<BTreeSet<String>>>>,
    }

    pub use crate::net::{allowed, random, served_https, urls};

    /// The workers' token: made once and kept (`workers-token`), so workers keep theirs across
    /// restarts. (`token`, the one pages carried long ago, in their address and their storage, is no
    /// key any more: gone. The Macs' agents read the new one from the NAS once theirs is refused.)
    pub fn token(dir: &Path) -> Result<String> {
        std::fs::remove_file(dir.join("token")).ok();
        crate::net::kept_token(&dir.join("workers-token"))
    }

    /// The answers worth gzipping: tower-http's defaults (not under 32 bytes, images, gRPC, event streams),
    /// and not raw bytes (octet-stream: packs, the NAS reads), tar, zip or a part of a file (a range).
    pub(super) fn compressible() -> impl tower_http::compression::predicate::Predicate {
        use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
        DefaultPredicate::new()
            .and(NotForContentType::const_new("application/octet-stream"))
            .and(NotForContentType::const_new("application/x-tar"))
            .and(NotForContentType::const_new("application/zip"))
            .and(NotForContentType::const_new("application/zstd"))
            .and(|_: StatusCode, _: axum::http::Version, h: &axum::http::HeaderMap, _: &axum::http::Extensions| {
                !h.contains_key(header::CONTENT_RANGE)
            })
    }

    /// Listens on `port`, on a runtime of its own; this Mac's addresses for workers.
    pub fn serve(
        listener: std::net::TcpListener,
        port: u16,
        ctx: Ctx,
        stopped: Arc<std::sync::atomic::AtomicBool>,
        wake: Arc<tokio::sync::Notify>,
        closed: std::sync::mpsc::Sender<()>,
    ) -> Result<Vec<String>> {
        listener.set_nonblocking(true)?;
        let routes = Router::new()
            // (Relative redirects: a proxy may serve the page under a path, tools/caddy's /build.)
            .route("/work", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "work/")]) }))
            .route("/work/", get(|| page(Url(String::new()))))
            // (The watching page's old address: the page is the dashboard now.)
            .route("/work/{*file}", get(page))
            .route("/work/ask", any(json))
            .route("/work/beat", any(json))
            .route("/work/pause", any(json))
            .route("/work/inputs", any(json))
            .route("/work/done", any(json))
            .route("/work/fail", any(json))
            .route("/work/cancel", any(json))
            .route("/work/act", any(json))
            .route("/work/settings", any(json))
            .route("/work/hold", any(json))
            .route("/work/quit", any(json))
            .route("/work/status", any(json))
            .route("/work/swarm", any(json))
            .route("/work/history", any(json))
            .route("/work/steps", any(json))
            .route("/work/live", any(json))
            .route("/work/regions", any(json))
            .route("/work/catalogs", any(json))
            .route("/work/tocome", any(json))
            .route("/work/built", any(json))
            .route("/work/timings", any(json))
            .route("/work/floors", any(json))
            .route("/work/redo", any(json))
            .route("/pool/mail", any(json))
            .route("/work/in/{lease}/{*path}", get(input))
            .route("/work/out/{lease}/{*path}", put(output))
            .route("/task/{*rest}", any(json));
        // (The map's, answered from memory: maproutes.)
        let shared = ctx.shared.clone();
        let routes = routes.merge(super::maproutes::router::<Ctx>(Arc::new(move || {
            shared.lock().unwrap_or_else(|e| e.into_inner()).maplog.clone()
        })));
        // (A test's handler that waits on a blocking task for good: a stop returns all the same.)
        #[cfg(test)]
        let routes = routes.route("/work/test-block", get(test_block));
        let app = routes
            .layer(axum::middleware::from_fn_with_state(ctx.clone(), gate))
            .layer(axum::middleware::from_fn_with_state(ctx.clone(), stamp))
            .layer(axum::middleware::from_fn_with_state(ctx.clone(), cors))
            .layer(tower_http::catch_panic::CatchPanicLayer::new())
            // (Gzip for a client that asks, outermost so it sees every header; see `compressible`.)
            .layer(
                tower_http::compression::CompressionLayer::new()
                    .no_br()
                    .no_deflate()
                    .no_zstd()
                    .compress_when(compressible()),
            )
            .with_state(ctx);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("coordinator")
            .enable_all()
            .build()?;
        std::thread::Builder::new().name("coordinator".into()).spawn(move || {
            rt.block_on(async move {
                match crate::net::Listener::new(listener) {
                    Ok(l) => accept(l, app, stopped, wake).await,
                    Err(e) => eprintln!("coordinator: its socket: {e}"),
                }
            });
            // (Its listener and every connection's socket closed by `accept` as it returned; its
            // runtime shut down without waiting for its blocking tasks (a stat of a NAS that
            // doesn't answer may never return), which hold no socket: nothing of it holds the port.)
            rt.shutdown_background();
            closed.send(()).ok();
        })?;
        Ok(urls(port))
    }

    /// The blocking task a test's request waits on (`/work/test-block`), told to `TEST_BLOCKED` as
    /// it begins: it never returns.
    #[cfg(test)]
    pub(super) static TEST_BLOCKED: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> = std::sync::Mutex::new(None);

    #[cfg(test)]
    async fn test_block() -> &'static str {
        let tx = TEST_BLOCKED.lock().unwrap().take();
        tokio::task::spawn_blocking(move || {
            if let Some(tx) = tx {
                tx.send(()).ok();
            }
            loop {
                std::thread::park();
            }
        })
        .await
        .ok();
        "unreachable"
    }

    /// The most connections at once, how long a request's headers may take to arrive (and an idle
    /// connection to stay open), and a connection's whole life.
    const CONNECTIONS: usize = 512;
    pub(super) const HEADERS_MAX: Duration = Duration::from_secs(20);
    const CONNECTION_MAX: Duration = Duration::from_secs(3 * 3600);

    /// Takes connections: one from elsewhere, or past the cap, is closed before a byte is read;
    /// each is kept alive by TCP (a device gone without a word is noticed), its headers bounded in
    /// time (which closes idle ones too), and its whole life bounded.
    async fn accept(
        listener: crate::net::Listener,
        app: Router,
        stopped: Arc<std::sync::atomic::AtomicBool>,
        wake: Arc<tokio::sync::Notify>,
    ) {
        use tower::ServiceExt;
        let room = Arc::new(tokio::sync::Semaphore::new(CONNECTIONS));
        // (Its connections: each ended, its socket closed, before a stop returns.)
        let mut conns = tokio::task::JoinSet::new();
        let (end, ending) = tokio::sync::watch::channel(false);
        loop {
            // (Stopped: every connection told to end, its socket closed at once (reset, not left
            // closing), and waited for, whatever its handler waits on; then the listener closed.
            // No socket of it is left holding the port once `stop` returns: a connection still
            // open would refuse the next coordinator's listen on it, EADDRINUSE.)
            if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                end.send(true).ok();
                while conns.join_next().await.is_some() {}
                return;
            }
            while conns.try_join_next().is_some() {}
            let got = tokio::select! {
                _ = wake.notified() => continue,
                got = listener.accept() => got,
            };
            let (stream, peer) = match got {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("coordinator: taking a connection: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            // (Refused ones closed while no child is being started too, store::spawn.)
            if !allowed(peer.ip()) {
                store::spawn::sockets(|| drop(stream));
                continue;
            }
            let Ok(permit) = room.clone().try_acquire_owned() else {
                store::spawn::sockets(|| drop(stream));
                continue;
            };
            let keepalive =
                socket2::TcpKeepalive::new().with_time(Duration::from_secs(60)).with_interval(Duration::from_secs(15));
            socket2::SockRef::from(&stream).set_tcp_keepalive(&keepalive).ok();
            let app = app.clone();
            let mut ending = ending.clone();
            conns.spawn(async move {
                use std::os::fd::{AsRawFd, BorrowedFd};
                let fd = stream.as_raw_fd();
                let svc = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                    req.extensions_mut().insert(ConnectInfo(peer));
                    app.clone().oneshot(req.map(Body::new))
                });
                let conn = hyper::server::conn::http1::Builder::new()
                    .timer(hyper_util::rt::TokioTimer::new())
                    .header_read_timeout(HEADERS_MAX)
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), svc);
                let mut conn = Box::pin(tokio::time::timeout(CONNECTION_MAX, conn));
                // (Told to end: no linger set while `conn` still owns the stream, so dropping it
                // resets the connection, the socket gone at once.)
                let end = async {
                    ending.wait_for(|e| *e).await.ok();
                    // SAFETY: `fd` is the stream `conn` owns, still open: `conn` is dropped only after
                    // the `select!`, this future completed or dropped.
                    socket2::SockRef::from(&unsafe { BorrowedFd::borrow_raw(fd) })
                        .set_linger(Some(Duration::ZERO))
                        .ok();
                };
                tokio::select! {
                    _ = conn.as_mut() => {}
                    _ = end => {}
                }
                // (Closed while no child is being started, store::spawn: one started by fork would
                // hold it until its exec.)
                store::spawn::sockets(|| drop(conn));
                drop(permit);
            });
        }
    }

    fn error(code: StatusCode, why: impl std::fmt::Display) -> Response {
        (code, [(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "error": why.to_string() })))
            .into_response()
    }

    /// A request's bearer credential (an empty one is none).
    fn bearer(h: &HeaderMap) -> Option<&str> {
        h.get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|k| !k.is_empty())
    }

    /// Who may ask what. This Mac, its LAN and the tailnet alone (and through the proxy on this Mac,
    /// from those alone: never the internet's, Tailscale Funnel's: crate::net::reached), and a
    /// request naming this Mac and from no web page elsewhere (crate::net::ours, from_the_page: DNS
    /// rebinding, a site's requests through a browser that reaches here). Then: the page and what
    /// its dashboard reads, anyone's; the Macs' agents' requests, with the build's key; a page's,
    /// with no key, for the owner's asks alone (pausing, settings; never work: `NO_PAGE_HELPERS`,
    /// `page_work`); and a job's
    /// (`/task/…`), with its own key, from this Mac itself (crate::net::own: `tailscale serve` hands
    /// the tailnet's requests over from loopback). A key that isn't one is refused (401), so an
    /// agent with an old one reads the new one. Who it is goes on to the request (`Caller`).
    /// Every answer says the pool's term and member this coordinator is, while it has them.
    async fn stamp(State(c): State<Ctx>, req: Request, next: Next) -> Response {
        let mut r = next.run(req).await;
        let (term, member) = {
            let s = c.shared.lock().unwrap();
            (s.leases.term, s.pool_member.clone())
        };
        if let (true, Some(m)) = (term > 0, member) {
            if let (Ok(t), Ok(m)) =
                (axum::http::HeaderValue::from_str(&term.to_string()), axum::http::HeaderValue::from_str(&m))
            {
                r.headers_mut().insert(super::POOL_TERM, t);
                r.headers_mut().insert(super::POOL_MEMBER, m);
            }
        }
        r
    }

    async fn gate(
        State(c): State<Ctx>,
        ConnectInfo(peer): ConnectInfo<SocketAddr>,
        mut req: Request,
        next: Next,
    ) -> Response {
        let ip = peer.ip();
        if !crate::net::reached(ip, req.headers()) {
            return error(StatusCode::FORBIDDEN, "not from here");
        }
        let from = crate::net::source(ip, req.headers());
        let local = crate::net::own(ip, req.headers());
        // (No Host at all: not a browser.)
        if req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).is_some_and(|h| !crate::net::ours(h)) {
            return error(StatusCode::FORBIDDEN, "not this lead's address");
        }
        if !crate::net::from_the_page(req.headers()) && member_page(&c, req.headers()).is_none() {
            return error(StatusCode::FORBIDDEN, "not from the build page");
        }
        let path = req.uri().path().to_string();
        let key = bearer(req.headers()).map(str::to_string);
        let owner = key.as_deref().is_some_and(|k| crate::net::same(k, &c.token));
        let job = key.as_deref().is_some_and(|k| crate::net::same(k, &c.job_token));
        req.extensions_mut().insert(Caller { local, page: key.is_none(), from });
        let is_page = req.method() == Method::GET
            && (path == "/work" || path.strip_prefix("/work/").is_some_and(|p| PAGE.iter().any(|(n, _, _)| *n == p)));
        let is_view = req.method() == Method::POST
            && matches!(
                path.as_str(),
                "/work/swarm"
                    | "/work/history"
                    | "/work/steps"
                    | "/work/live"
                    | "/work/regions"
                    | "/work/catalogs"
                    | "/work/tocome"
                    | "/work/built"
            );
        if is_page || is_view {
            return next.run(req).await;
        }
        // (The owner's asks from a build page, on any device that reaches it (the owner, 21 Oct:
        // no page of this Mac's own first); a viewer's and a member's broker's passed to its agent.)
        let viewer = c.shared.lock().unwrap().viewer;
        let owners_asks = matches!(
            path.as_str(),
            "/work/pause" | "/work/inputs" | "/work/act" | "/work/settings" | "/work/hold" | "/work/quit"
        );
        if viewer {
            return if key.is_none() && owners_asks && req.method() == Method::POST {
                next.run(req).await
            } else {
                error(StatusCode::FORBIDDEN, "this Mac shows the build; it coordinates nothing")
            };
        }
        // A job's: with its key, from this Mac itself.
        if path.starts_with("/task/") {
            return match () {
                _ if !job => error(StatusCode::UNAUTHORIZED, "not this Mac's job's"),
                _ if !local => error(StatusCode::FORBIDDEN, "that comes from the lead itself"),
                _ => next.run(req).await,
            };
        }
        if owner {
            return next.run(req).await;
        }
        if key.is_some() {
            return error(StatusCode::UNAUTHORIZED, "not the build's key");
        }
        // A page left open from before its helpers were removed, asking for work: told so (a beat,
        // its lease not held: the page stops that task), never an error it retries at once.
        if page_work(&path) {
            return if path == "/work/beat" {
                (
                    StatusCode::OK,
                    [(header::CACHE_CONTROL, "no-store")],
                    Json(serde_json::json!({ "ok": false, "error": super::NO_PAGE_HELPERS })),
                )
                    .into_response()
            } else {
                error(StatusCode::FORBIDDEN, super::NO_PAGE_HELPERS)
            };
        }
        // A page: the owner's asks.
        let theirs = req.method() == Method::POST && owners_asks;
        if !theirs {
            return error(StatusCode::FORBIDDEN, "a page reads the build and takes the owner's asks alone");
        }
        next.run(req).await
    }

    /// Whether `path` is one a page once worked through (its asks, beats, hand-backs and failures,
    /// its tasks' files and the programs' WebAssembly builds).
    pub(super) fn page_work(path: &str) -> bool {
        matches!(path, "/work/ask" | "/work/beat" | "/work/done" | "/work/fail")
            || ["/work/in/", "/work/net/", "/work/out/", "/work/prog/"].iter().any(|p| path.starts_with(p))
    }

    /// A request's page when it's one of the pool's members' (its `Origin` one of their addresses,
    /// `Coordinator::set_brokers`): tasks brokered by every member, docs/pool.md §8. None: no such
    /// page, or the switch off.
    fn member_page(c: &Ctx, h: &HeaderMap) -> Option<HeaderValue> {
        let o = h.get(header::ORIGIN)?;
        let s = o.to_str().ok()?.to_ascii_lowercase();
        c.cors.read().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|set| set.contains(&s)).then(|| o.clone())
    }

    /// CORS for the members' pages (docs/pool.md §8), around the gate: a preflight from one
    /// answered (the methods and headers a page sends: `Authorization`, `X-Worker`, `Range`, `PUT`;
    /// and a private network's ask, a browser's for a page calling a LAN or tailnet address), and
    /// every answer to one allowed to it. Any other request, as before: no CORS header, and a
    /// preflight unanswered.
    async fn cors(State(c): State<Ctx>, req: Request, next: Next) -> Response {
        let Some(origin) = member_page(&c, req.headers()) else { return next.run(req).await };
        let allow = |r: &mut Response| {
            let h = r.headers_mut();
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
            h.append(header::VARY, HeaderValue::from_static("Origin"));
        };
        if req.method() == Method::OPTIONS && req.headers().contains_key(header::ACCESS_CONTROL_REQUEST_METHOD) {
            let mut r = StatusCode::NO_CONTENT.into_response();
            allow(&mut r);
            let h = r.headers_mut();
            h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static(super::brokers::ALLOW_METHODS));
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static(super::brokers::ALLOW_HEADERS));
            h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from(super::brokers::MAX_AGE_S));
            if req.headers().contains_key("access-control-request-private-network") {
                h.insert("access-control-allow-private-network", HeaderValue::from_static("true"));
            }
            return r;
        }
        let mut r = next.run(req).await;
        allow(&mut r);
        r
    }

    async fn page(Url(file): Url<String>) -> Response {
        match PAGE.iter().find(|(n, _, _)| *n == file) {
            Some(("sw.js", t, body)) => (
                [(header::CONTENT_TYPE, *t), (header::CACHE_CONTROL, "no-store")],
                String::from_utf8_lossy(body).replace("__VERSION__", page_version()).replace("__PAGE__", &page_list()),
            )
                .into_response(),
            Some((_, t, body)) => {
                ([(header::CONTENT_TYPE, *t), (header::CACHE_CONTROL, "no-store")], *body).into_response()
            }
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
        let is_json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|t| t.split(';').next())
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
        if req.method() != Method::POST || !is_json {
            return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "a POST of JSON (application/json)");
        }
        let caller = req.extensions().get::<Caller>().cloned().unwrap_or_default();
        let body = match tokio::time::timeout(JSON_TIME, axum::body::to_bytes(req.into_body(), JSON_MAX)).await {
            Ok(Ok(b)) => b,
            Ok(Err(_)) => return error(StatusCode::PAYLOAD_TOO_LARGE, "too big, or cut short"),
            Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "its body didn't come"),
        };
        // (Not once the coordinator has stopped: what it would write is the next one's to keep.)
        let routed = tokio::task::spawn_blocking(move || {
            let stopped = c.routes.read().unwrap_or_else(|e| e.into_inner());
            if *stopped {
                return Ok((503, serde_json::json!({ "error": "this coordinator has stopped" })));
            }
            route(&path, &body, &c.shared, &c.journal, &caller)
        });
        match routed.await {
            Ok(Ok((204, _))) => (StatusCode::NO_CONTENT, [(header::CACHE_CONTROL, "no-store")]).into_response(),
            Ok(Ok((code, v))) => {
                (StatusCode::from_u16(code).unwrap_or(StatusCode::OK), [(header::CACHE_CONTROL, "no-store")], Json(v))
                    .into_response()
            }
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
        let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
            tokio::io::AsyncReadExt::take(f, n),
            1 << 20,
        ));
        let mut resp = (
            code,
            [(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "no-store"), (header::ACCEPT_RANGES, "bytes")],
            body,
        )
            .into_response();
        resp.headers_mut().insert(header::CONTENT_LENGTH, n.into());
        if code == StatusCode::PARTIAL_CONTENT {
            if let Ok(v) = format!("bytes {start}-{}/{len}", start + n - 1).parse() {
                resp.headers_mut().insert(header::CONTENT_RANGE, v);
            }
        }
        resp
    }

    /// A task's input, to the worker holding it.
    async fn input(State(c): State<Ctx>, Url((lease, path)): Url<(u64, String)>, h: HeaderMap) -> Response {
        let found = c.shared.lock().unwrap().tasks.input(lease, &worker(&h), &path);
        match found {
            Some((file, size)) => {
                send_file(
                    &file,
                    "application/octet-stream",
                    Some(size),
                    h.get(header::RANGE).and_then(|v| v.to_str().ok()),
                )
                .await
            }
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
    async fn output(
        State(c): State<Ctx>,
        Url((lease, path)): Url<(u64, String)>,
        h: HeaderMap,
        body: Body,
    ) -> Response {
        if h.get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > UPLOAD_MAX)
        {
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
                let next = tokio::time::timeout(CHUNK_TIME, futures_util::StreamExt::next(&mut stream))
                    .await
                    .map_err(|_| anyhow::anyhow!("no byte for {} s", CHUNK_TIME.as_secs()))?;
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

/// A port for a test's coordinator that no other test in this process has been given (the system's
/// pick, once): one a stopped coordinator let go is never handed to another test meanwhile, so a
/// test that checks its port is free finds it so.
#[cfg(test)]
pub(crate) fn test_port() -> u16 {
    let (l, port) = test_listener();
    HELD.lock().unwrap().insert(port, l);
    port
}

/// The ports `test_port` gave, each still held open (its listener), for the coordinator a test
/// starts on it to take (`take_test_listener`): no other process can take one between.
#[cfg(test)]
static HELD: std::sync::Mutex<BTreeMap<u16, std::net::TcpListener>> = std::sync::Mutex::new(BTreeMap::new());

/// The listener `test_port` holds for `port`, when it holds one (a coordinator starting on it takes
/// it, rather than listen on the port again).
#[cfg(test)]
pub(crate) fn take_test_listener(port: u16) -> Option<std::net::TcpListener> {
    HELD.lock().unwrap().remove(&port)
}

/// Whether nothing listens on `port`: listened on here, at once let go (std sets SO_REUSEADDR, so
/// a connection of an earlier listener left closing doesn't count; a listener still open does). Not
/// told by connecting: a connection takes a local port of its own, which a loaded Mac, its ports
/// held closing by the suites' many connections, may not have (EADDRNOTAVAIL), saying nothing of
/// the listener.
#[cfg(test)]
pub(crate) fn port_free(port: u16) -> std::io::Result<()> {
    store::spawn::listen((crate::coord::LISTEN, port)).map(|l| store::spawn::sockets(|| drop(l)))
}

/// A listener on a port no other test of this process has been given, and its port.
#[cfg(test)]
fn test_listener() -> (std::net::TcpListener, u16) {
    static GIVEN: std::sync::Mutex<BTreeSet<u16>> = std::sync::Mutex::new(BTreeSet::new());
    let mut g = GIVEN.lock().unwrap();
    // (Each turn a port given before: not time, but ports.)
    for _ in 0..1000 {
        let l = store::spawn::listen((crate::coord::LISTEN, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        if g.insert(port) {
            return (l, port);
        }
    }
    panic!("no port left that wasn't given before")
}

/// A test's coordinator (its state in `dir`), on a port the test held from the start (`test_port`'s
/// listener, handed over: no other process can take it between); and the port.
#[cfg(test)]
pub(crate) fn start_for_test(dir: &Path, me: &str, app: &str) -> (Coordinator, u16) {
    let (listener, port) = test_listener();
    (Coordinator::start_on(dir, listener, me, app).unwrap(), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{header, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use std::io::Read;
    use tower::ServiceExt;

    fn gzip_app() -> Router {
        let big = serde_json::to_vec(&(0..4000).map(|i| format!("6/{i}/7")).collect::<Vec<_>>()).unwrap();
        let bytes = |ctype: &'static str, status: StatusCode| {
            let b = vec![7u8; 5000];
            move || async move { (status, [(header::CONTENT_TYPE, ctype)], b) }
        };
        Router::new()
            .route("/json", get(move || async move { ([(header::CONTENT_TYPE, "application/json")], big) }))
            .route("/tiny", get(|| async { ([(header::CONTENT_TYPE, "application/json")], "{}") }))
            .route("/pack", get(bytes("application/octet-stream", StatusCode::OK)))
            .route("/webp", get(bytes("image/webp", StatusCode::OK)))
            .route("/tar", get(bytes("application/x-tar", StatusCode::OK)))
            .route(
                "/part",
                get(|| async {
                    (
                        StatusCode::PARTIAL_CONTENT,
                        [(header::CONTENT_TYPE, "text/plain"), (header::CONTENT_RANGE, "bytes 0-4999/9000")],
                        vec![7u8; 5000],
                    )
                }),
            )
            .layer(tower_http::compression::CompressionLayer::new().compress_when(http::compressible()))
    }

    async fn gzip_ask(path: &str, accept: Option<&str>) -> (Option<String>, Vec<u8>) {
        let mut req = Request::get(path);
        if let Some(a) = accept {
            req = req.header(header::ACCEPT_ENCODING, a);
        }
        let r = gzip_app().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let enc = r.headers().get(header::CONTENT_ENCODING).map(|v| v.to_str().unwrap().to_string());
        (enc, axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec())
    }

    /// Task #792: a client that asks gets a big JSON answer gzipped, the same body once decoded; one that
    /// doesn't, plain; raw bytes, images, tars and parts of a file are never touched; a tiny answer isn't.
    #[tokio::test]
    async fn gzip_for_big_answers_asked_for() {
        let (enc, plain) = gzip_ask("/json", None).await;
        assert_eq!(enc, None);
        let (enc, zipped) = gzip_ask("/json", Some("gzip, deflate, br")).await;
        assert_eq!(enc.as_deref(), Some("gzip"));
        assert!(zipped.len() * 3 < plain.len(), "{} of {}", zipped.len(), plain.len());
        let mut back = Vec::new();
        flate2::read::GzDecoder::new(&zipped[..]).read_to_end(&mut back).unwrap();
        assert_eq!(back, plain);
        for p in ["/tiny", "/pack", "/webp", "/tar", "/part"] {
            let (enc, body) = gzip_ask(p, Some("gzip")).await;
            assert_eq!((enc, body.len() > 5), (None, p != "/tiny"), "{p}");
        }
    }

    /// Task #423: a lease lapsed for its worker's silence says so in the history, never nothing.
    #[test]
    fn a_lapse_says_why() {
        let now = Instant::now();
        let mut l = Leases::new(Duration::from_millis(10));
        let id =
            l.grant("m1 #2", Work::Job { step: "unit".into(), targets: vec![("6/12/25".into(), "k".into())] }, now);
        let gone = l.expire(now + Duration::from_millis(20));
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].id, id);
        let why = lapse_why(&gone[0]);
        assert!(why.starts_with("unit 6/12/25: no word from m1 #2 for 10 min"), "{why}");
    }

    /// A stop returns whatever a blocking task of the coordinator waits on (a stat of a NAS that
    /// doesn't answer, here one that never returns), its listener closed: no clock decides it.
    #[test]
    fn a_stop_returns_while_a_blocking_task_never_does() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = start_for_test(&d.path().join("a"), "m4", "");
        let (tx, rx) = std::sync::mpsc::channel();
        *http::TEST_BLOCKED.lock().unwrap() = Some(tx);
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "a page");
        let asked = std::thread::spawn(move || w.get_bytes("/work/test-block").is_err());
        // (Its blocking task under way, for good; a request that never reached it fails here.)
        rx.recv_timeout(WATCHDOG).expect("the request reached its handler");
        c.stop();
        crate::coord::port_free(port).unwrap_or_else(|e| panic!("its listener closed: {e}"));
        // (The request's connection ended with the runtime: it fails, it doesn't hang.)
        assert!(asked.join().unwrap());
    }

    use store::testkit::WATCHDOG;

    /// The next coordinator, this process's (a member that takes the lead again), starts on the port
    /// a stopped one let go, a request answered by it before: `stop` returns once the port is free,
    /// no wait and no retry. It answers there, the state the first left loaded.
    #[test]
    fn the_next_coordinator_starts_on_the_port_a_stopped_one_let_go() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("a");
        let (c, port) = start_for_test(&dir, "m4", "");
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        w.ask(&ask(4096)).unwrap();
        c.stop();
        let next = Coordinator::start(&dir, port, "m4", "").unwrap();
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], next.contact.token.clone(), "m1");
        w.ask(&ask(4096)).unwrap();
    }

    /// A pool lead's coordinator whose agent hasn't said it may grant for `STALL_S` (held up, or
    /// just woken before its step: its view maybe old) grants nothing; told again, it does.
    #[test]
    fn a_lead_not_stepped_for_a_stall_grants_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = start_for_test(&d.path().join("a"), "m4", "");
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.set_moving(None);
        c.shared.lock().unwrap().told = Some(wall_now() - crate::pool::driver::STALL_S - 1);
        let e = w.ask(&ask(4096)).unwrap_err();
        assert!(format!("{e:#}").contains("not stepped"), "{e:#}");
        assert!(c.ask_own(&ask(4096), &BTreeSet::new()).is_none());
        c.set_moving(None);
        w.ask(&ask(4096)).unwrap();
        // A step's checks, then a hang (a write on the share) before the tell: told as of the
        // checks, it grants nothing once `STALL_S` has passed since them.
        c.set_moving_as_of(None, wall_now() - crate::pool::driver::STALL_S - 1);
        assert!(format!("{:#}", w.ask(&ask(4096)).unwrap_err()).contains("not stepped"));
        c.set_moving_as_of(None, wall_now() - 5);
        w.ask(&ask(4096)).unwrap();
    }

    #[test]
    fn unpublish_leaves_another_leads_contact_of_the_same_token() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state")).unwrap();
        let (c, _) = start_for_test(&d.path().join("a"), "m4", "");
        c.publish(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        // The next lead's contact, the pool's token the same, its addresses its own.
        let next = Contact {
            urls: vec!["http://MacBook-Pro-de-Brandon.local:18091".into()],
            token: c.contact.token.clone(),
            page: None,
        };
        contact_path(&store::nasfs::NasRoot::local_for_test(&root)).write(serde_json::to_vec(&next).unwrap()).unwrap();
        c.unpublish(&store::nasfs::NasRoot::local_for_test(&root));
        assert!(contact_path(&store::nasfs::NasRoot::local_for_test(&root)).exists(), "the next lead's contact stays");
        c.publish(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        c.unpublish(&store::nasfs::NasRoot::local_for_test(&root));
        assert!(!contact_path(&store::nasfs::NasRoot::local_for_test(&root)).exists(), "its own goes");
    }

    #[test]
    fn an_unpublish_overtaken_by_a_publish_leaves_the_contact() {
        // (An unpublish begun on a hung share, the contact published again before its remove: the
        // fresh contact stays. The agent counts its publications: `publish_gen`.)
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state")).unwrap();
        let (c, _) = start_for_test(&d.path().join("a"), "m4", "");
        let gens = std::sync::atomic::AtomicU64::new(1);
        c.publish(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        let at = gens.load(std::sync::atomic::Ordering::SeqCst);
        // Published again meanwhile.
        gens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        c.publish(&store::nasfs::NasRoot::local_for_test(&root)).unwrap();
        c.unpublish_unless(&store::nasfs::NasRoot::local_for_test(&root), &|| {
            gens.load(std::sync::atomic::Ordering::SeqCst) != at
        });
        assert!(contact_path(&store::nasfs::NasRoot::local_for_test(&root)).exists(), "the fresh contact stays");
        let at = gens.load(std::sync::atomic::Ordering::SeqCst);
        c.unpublish_unless(&store::nasfs::NasRoot::local_for_test(&root), &|| {
            gens.load(std::sync::atomic::Ordering::SeqCst) != at
        });
        assert!(!contact_path(&store::nasfs::NasRoot::local_for_test(&root)).exists(), "not published since: it goes");
    }

    /// #666 (25 Oct, the mini: its loop stuck ~10 min on the NAS lost a worldwide job at 56 %, and a
    /// second was granted): past the TTL with no beat the leases the lead's slots run (`keep_own`)
    /// stay, renewable and finishable; one of the lead's whose job ended unsaid lapses, as a member's
    /// does; a job of no targets blocks another of its step, and is held again once its holder's
    /// lapsed.
    #[test]
    fn a_stalled_loop_keeps_the_leads_running_leases() {
        let (_d, c, _w) = start();
        let clock = store::clock::Virtual::new();
        c.shared.lock().unwrap().clock = clock.clone();
        let z8 = c.hold("buildings", &[]).unwrap();
        let unit = c.hold("unit", &[("6/1/1".into(), "k".into())]).unwrap();
        let dead = c.hold("unit", &[("6/1/2".into(), "k".into())]).unwrap();
        let member = {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            s.leases.grant("m1", Work::Job { step: "gc".into(), targets: Vec::new() }, now)
        };
        c.keep_own([(z8, 0), (unit, 1)].into());
        // No duplicate of a job of no targets, the lead's or a member's, with targets or none.
        assert_eq!(c.hold("buildings", &[]), None);
        assert_eq!(c.hold("gc", &[]), None);
        assert_eq!(c.hold("gc", &[("x".into(), "k".into())]), None);
        clock.advance(TTL + Duration::from_secs(60));
        let gone: BTreeSet<u64> = c.expire().iter().map(|l| l.id).collect();
        assert_eq!(gone, [dead, member].into(), "a job ended unsaid lapses as a member's does");
        assert_eq!(c.hold("buildings", &[]), None, "the lead's job still holds its step");
        assert!(c.renew_live(z8, None, None));
        assert!(c.held("unit").contains("6/1/1") && !c.held("unit").contains("6/1/2"));
        // Its job ended, the loop still stuck: finished all the same, past its deadline.
        c.finish(unit, &[]);
        assert!(!c.held("unit").contains("6/1/1"));
        // Said no more (its job gone): z8's lapses too.
        c.keep_own(BTreeMap::new());
        clock.advance(TTL + Duration::from_secs(60));
        assert_eq!(c.expire().iter().map(|l| l.id).collect::<Vec<_>>(), [z8]);
        // The member's gone: its step is the lead's to hold.
        assert!(c.hold("gc", &[]).is_some());
    }

    fn start() -> (tempfile::TempDir, Coordinator, client::Client) {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = start_for_test(&d.path().join("coord"), "m4", "");
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        (d, c, w)
    }

    /// Z6 tiles 6/1/1 to 6/1/`n` measured: 3 GB, a minute, a thread each.
    fn measure_units(c: &Coordinator, n: usize) {
        for i in 1..=n {
            let sizes = [("threads".to_string(), 1)].into();
            let cost =
                Cost { peak_mb: 3000, secs: 60, worker: None, v: cost_version("unit"), sizes, ..Default::default() };
            c.add_costs(&[(format!("6/1/{i}"), cost)]);
        }
    }

    fn ask(mem_mb: u64) -> Ask {
        Ask {
            worker: "m1".into(),
            kind: "native".into(),
            can: vec!["unit".into()],
            mem_mb,
            cores: 8,
            ..Default::default()
        }
    }

    #[test]
    fn asks_parse_with_and_without_a_capacity() {
        // An older agent's ask: no capacity, the rest as before.
        let old = r#"{"worker":"m1","kind":"native","can":["unit"],"mem_mb":6144,"cores":8,"max":6,"disk_mb":90000}"#;
        let a: Ask = serde_json::from_str(old).unwrap();
        assert_eq!((a.mem_mb, a.disk_mb, a.cap.clone()), (6144, Some(90_000), None));
        assert!(!serde_json::to_string(&a).unwrap().contains("cap"), "none said: none written");
        // A newer one's: its capacity, a field this app doesn't know ignored, ones missing defaulted.
        let new = r#"{"worker":"m1","can":["unit"],"mem_mb":6144,"cap":{"at":5,"limit_mb":12288,"held_mb":2000,"mem_mb":8000,"cores":8,"cores_free":6,"disk_mb":50000,"in_use":true,"ac":false,"battery":54,"nas":true,"home":true,"guard":true,"leases":[3,4],"nas_share":0.5}}"#;
        let a: Ask = serde_json::from_str(new).unwrap();
        let cap = a.cap.clone().unwrap();
        assert_eq!(
            (cap.limit_mb, cap.mem_mb, cap.cores_free, cap.disk_mb, cap.reserved_mb, cap.battery, cap.leases.clone()),
            (12_288, 8000, 6, 50_000, 0, Some(54), vec![3, 4])
        );
        assert!(cap.in_use && !cap.ac && cap.nas && cap.guard);
        let partial: Capacity = serde_json::from_str(r#"{"limit_mb":1}"#).unwrap();
        assert_eq!(partial, Capacity { limit_mb: 1, ..Default::default() });
        // Round trip.
        assert_eq!(serde_json::from_str::<Ask>(&serde_json::to_string(&a).unwrap()).unwrap().cap, Some(cap));
    }

    #[test]
    fn the_coordinator_keeps_each_members_capacity() {
        let (_d, c, w) = start();
        c.offer_units("2026-09-28", vec![("6/1/1".into(), "k1".into(), 100 << 20)]);
        // The lead's own, then a member's from its ask; an older member's ask says none.
        c.set_capacity(Some(Capacity { at: unix_now(), limit_mb: 36_000, cores: 10, ac: true, ..Default::default() }));
        let cap = Capacity {
            at: unix_now(),
            limit_mb: 12_288,
            mem_mb: 9000,
            cores: 8,
            cores_free: 7,
            disk_mb: 40_000,
            nas: true,
            home: true,
            leases: vec![],
            ..Default::default()
        };
        w.ask(&Ask { cap: Some(cap.clone()), ..ask(4096) }).unwrap();
        let old = client::Client::at(w.urls(), c.contact.token.clone(), "old");
        old.ask(&ask(4096)).unwrap();
        let caps = c.capacities();
        assert_eq!(
            caps.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["m4", "m1"],
            "the lead's first; the older member's none"
        );
        assert_eq!(caps[0].1.limit_mb, 36_000);
        assert_eq!(caps[1].1, cap);
        // A capacity past CAP_FRESH_S (a busy member's last ask) left out, though its worker is around.
        c.shared.lock().unwrap().workers.get_mut("m1").unwrap().cap.as_mut().unwrap().at = unix_now() - CAP_FRESH_S;
        assert_eq!(c.capacities().len(), 1);
        // Shown by the swarm, for the build page.
        let s = c.shared.lock().unwrap();
        assert_eq!(s.workers["m1"].cap.as_ref().map(|c| c.mem_mb), Some(9000), "kept, though stale");
        assert!(s.workers["old"].cap.is_none());
    }

    /// Task #517: the NAS as the lead sees it: the probes kept, the limits, the gate's reading and
    /// the disks', and the pool's traffic, the members' with a measure summed.
    #[test]
    fn the_nas_mount_is_named_from_what_it_mounts() {
        let m = NasMount::of("//brandontsang@fishandchips.local/personal", "smbfs", 7);
        assert_eq!(m, NasMount { name: "fishandchips".into(), proto: "SMB".into(), free: 7 });
        assert_eq!(NasMount::of("nas.lan:/volume1/x", "nfs", 0).name, "nas.lan");
        assert_eq!(
            NasMount::of("/dev/disk3s1", "apfs", 0),
            NasMount { name: "dev".into(), proto: "apfs".into(), free: 0 }
        );
    }

    /// Task #634: the pool's NAS calls, the members' with a measure summed.
    #[test]
    fn the_pool_calls_sum_the_members_saying_them() {
        let cap = |c| ("m".to_string(), Capacity { nas_calls: c, ..Default::default() });
        assert_eq!(pool_calls(&[cap(None)]), None);
        assert_eq!(
            pool_calls(&[cap(Some([2.0, 0.5, 12.0])), cap(None), cap(Some([1.0, 0.0, 12.0]))]),
            Some([3.0, 0.5, 24.0])
        );
    }

    #[test]
    fn the_status_shows_the_nas_as_the_lead_sees_it() {
        let (_d, c, w) = start();
        assert_eq!(c.nas_view(), None, "nothing probed or read yet");
        let mut tr = [0.0; 7];
        tr[0] = 12.5;
        tr[3] = 4.0;
        c.set_capacity(Some(Capacity { at: unix_now(), nas_traffic: Some(tr), ..Default::default() }));
        w.ask(&Ask {
            cap: Some(Capacity { at: unix_now(), nas_traffic: Some(tr), ..Default::default() }),
            ..ask(4096)
        })
        .unwrap();
        let old = client::Client::at(w.urls(), c.contact.token.clone(), "old");
        old.ask(&Ask { cap: Some(Capacity { at: unix_now(), ..Default::default() }), ..ask(4096) }).unwrap();
        let p = crate::budget::nas::Probe { at: unix_now(), bytes_s: 80e6, creates_s: 40.0, ..Default::default() };
        {
            let mut s = c.shared.lock().unwrap();
            s.nas_probes = vec![p];
            s.nas_own = crate::budget::nas::Probes(vec![p]).limits(None);
            s.nas_loop.read(&[60.0, 91.0], 85);
            s.nas_disks = vec![60.0, 91.0];
        }
        let v = c.nas_view().unwrap();
        assert_eq!(v.probes, vec![p]);
        assert_eq!(v.limits.map(|l| l.bytes_s), Some(80e6));
        assert_eq!((v.load_pct, v.target_pct, v.held), (Some(91.0), 85, true));
        assert_eq!(v.disks_pct, vec![60.0, 91.0]);
        let t = v.traffic.unwrap();
        assert_eq!((t[0], t[3], t[2]), (25.0, 8.0, 0.0), "the lead's and m1's summed; the unmeasured left out");
        // A disk read as NaN: left out, and the status still reads back.
        c.shared.lock().unwrap().nas_disks = vec![f64::NAN, 50.0];
        let v = c.nas_view().unwrap();
        assert_eq!(v.disks_pct, vec![50.0]);
        let back: NasView = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back, v);
    }

    fn handoff(units: &[(&str, &str)]) -> Handoff {
        let changes = units
            .iter()
            .map(|(u, _)| {
                (
                    format!("base/{}", u.replace('/', "-")),
                    Some(format!("base/{}.0000000000000003.base", u.replace('/', "-"))),
                )
            })
            .collect();
        Handoff {
            changes,
            done: Some(("unit".into(), units.iter().map(|(u, k)| (u.to_string(), k.to_string())).collect())),
            ..Default::default()
        }
    }

    #[test]
    fn a_cost_measured_another_way_says_nothing_of_memory() {
        // Terrain measured holding its whole z3 tile (before it wrote a z6 tile at a time): its
        // estimate instead; measured since: what it took.
        let mut costs = BTreeMap::new();
        costs.insert(
            cost_key("terrain-hi", "3/0/2"),
            Cost { peak_mb: 32_900, secs: 3000, worker: None, v: 0, ..Default::default() },
        );
        assert_eq!(job_peak(&costs, "terrain-hi", "3/0/2", 2400), 2400);
        costs.insert(
            cost_key("terrain-hi", "3/0/2"),
            Cost { peak_mb: 2100, secs: 3000, worker: None, v: cost_version("terrain-hi"), ..Default::default() },
        );
        assert_eq!(job_peak(&costs, "terrain-hi", "3/0/2", 2400), 2100);
        // Slope likewise (written a z6 tile at a time too); other steps' measures stand.
        costs.insert(
            cost_key("slope-hi", "3/0/2"),
            Cost { peak_mb: 20_000, secs: 300, worker: None, v: 0, ..Default::default() },
        );
        assert_eq!(job_peak(&costs, "slope-hi", "3/0/2", 2000), 2000);
        costs.insert(
            cost_key("peaks", "6/1/1"),
            Cost { peak_mb: 5000, secs: 300, worker: None, v: 0, ..Default::default() },
        );
        assert_eq!(job_peak(&costs, "peaks", "6/1/1", 2500), 5000);
    }

    /// One dense z6 tile measured the way z6 tiles run now doesn't keep the others, not yet measured, off
    /// a 12 GB worker: they go by their extracts' guess (crate::needs::GUESS_FIRST), it by its own.
    #[test]
    fn a_dense_unit_leaves_the_small_ones_to_a_12_gb_worker() {
        let (_d, c, w) = start();
        let units: Vec<(String, String, u64)> =
            (1..=3).map(|i| (format!("6/1/{i}"), format!("k{i}"), 100 << 20)).collect();
        c.offer_units("2026-09-28", units);
        c.add_costs(&[(
            "6/1/1".into(),
            Cost { peak_mb: 14_000, secs: 600, worker: None, v: cost_version("unit"), ..Default::default() },
        )]);
        let g = w.ask(&Ask { ..ask(12 << 10) }).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g.work else { panic!("a job") };
        // (Its first: a job a target while none has a time measured.)
        assert_eq!(targets.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["6/1/2"]);
    }

    #[test]
    fn the_history_and_the_swarm_say_what_happened_and_why() {
        let (_d, c, w) = start();
        let units: Vec<(String, String, u64)> =
            (1..=3).map(|i| (format!("6/1/{i}"), format!("k{i}"), 100 << 20)).collect();
        c.offer_units("2026-09-28", units);
        // A worker that spares 4 GB takes two z3 tiles (the third too big), hands one back.
        // (Each measured: a z6 tile not measured would take its step's largest, crate::needs.)
        for (u, mb) in [("6/1/1", 9000), ("6/1/2", 3000), ("6/1/3", 3000)] {
            c.shared.lock().unwrap().costs.insert(
                u.into(),
                Cost { peak_mb: mb, secs: 60, worker: None, v: cost_version("unit"), ..Default::default() },
            );
        }
        let g = w.ask(&Ask { ..ask(4096) }).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g.work else { panic!("a job") };
        let one: Vec<(&str, &str)> = targets.iter().take(1).map(|(t, k)| (t.as_str(), k.as_str())).collect();
        w.done(&Done { worker: "m1".into(), lease: g.lease, handoff: Some(handoff(&one)), ..Default::default() })
            .unwrap();
        c.note(history::Event {
            worker: Some("m4".into()),
            targets: vec!["ohio".into()],
            ..history::Event::new("catalog")
        });
        // What happened, in order, after a number.
        let (code, h) = w.post_json("/work/history", &serde_json::json!({ "since": 0 })).unwrap();
        assert_eq!(code, 200);
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["agent", "worker", "lease", "done", "catalog"]);
        let last = h["seq"].as_u64().unwrap();
        assert!(w.post_json("/work/history", &serde_json::json!({ "since": last })).unwrap().1["events"]
            .as_array()
            .unwrap()
            .is_empty());
        // The swarm: the history's number and the hours; the worker's fit (the z3 tile it holds, the one
        // too big for it); the tasks by state.
        let (_, sw) = w.post_json("/work/swarm", &serde_json::json!({})).unwrap();
        assert_eq!(sw["seq"].as_u64(), Some(last));
        assert_eq!(sw["rates"]["rows"].as_array().unwrap().len(), 24);
        let m1 = sw["workers"].as_array().unwrap().iter().find(|x| x["name"] == "m1").unwrap();
        let fit = &m1["fit"][0];
        assert_eq!(
            (fit["offered"].as_u64(), fit["done"].as_u64(), fit["too_big"].as_u64(), fit["fits"].as_u64()),
            (Some(3), Some(1), Some(1), Some(1))
        );
        assert!(sw["tasks"].is_object() && sw["leases"].as_array().unwrap().is_empty());
        // (Task #409) the view the lead writes: the same workers, tasks and task_mb, its times as
        // unix seconds, and the same until something changes.
        let v = c.view();
        assert_eq!((&v["tasks"], &v["task_mb"], &v["pause"]), (&sw["tasks"], &sw["task_mb"], &sw["pause"]));
        let vm1 = v["workers"].as_array().unwrap().iter().find(|x| x["name"] == "m1").unwrap();
        assert_eq!((&vm1["fit"], &vm1["label"], &vm1["done"]), (&m1["fit"], &m1["label"], &m1["done"]));
        assert!(vm1["seen"].as_u64().unwrap() + 5 >= unix_now() && vm1.get("seen_s").is_none());
        assert_eq!(c.view(), v);
    }

    #[test]
    fn a_runs_timings_come_with_its_hand_off_and_are_read_from_another_mac() {
        use crate::timings::{PhaseRec, RunRec};
        let (d, c, w) = start();
        c.offer_units("2026-09-28", vec![("6/1/1".into(), "k1".into(), 100 << 20)]);
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let run = |kind: &str, host: &str| RunRec {
            v: 1,
            kind: kind.into(),
            host: host.into(),
            wall_s: 10.0,
            ok: true,
            phases: vec![PhaseRec { name: "reading records".into(), wall_s: 1.0, n: 1, ..Default::default() }],
            ..Default::default()
        };
        // A helper's, with its hand-off (its host as its lease says, whatever it said); this Mac's own.
        assert_eq!(
            w.done(&Done {
                lease: g.lease,
                handoff: Some(handoff(&[("6/1/1", "k1")])),
                timings: Some(run("unit", "someone")),
                ..Default::default()
            })
            .unwrap(),
            client::Handed::Taken
        );
        c.add_timings(&run("gc", "m4"));
        let kept = crate::timings::read_log(&d.path().join("coord/timings.jsonl"));
        assert_eq!(
            kept.iter().map(|r| (r.kind.as_str(), r.host.as_str())).collect::<Vec<_>>(),
            [("unit", "m1"), ("gc", "m4")]
        );
        // Asked from another Mac: a kind's last runs.
        let (code, v) = w.post_json("/work/timings", &serde_json::json!({ "kind": "unit", "last": 5 })).unwrap();
        assert_eq!(code, 200);
        let runs: Vec<RunRec> = serde_json::from_value(v["runs"].clone()).unwrap();
        assert_eq!(runs, kept[..1]);
        // An older worker's done, without them, or one whose don't read: nothing kept, nothing refused.
        let body = serde_json::json!({ "worker": "m1", "lease": 999 });
        assert!(serde_json::from_value::<Done>(body).unwrap().timings.is_none());
        let body = serde_json::json!({ "worker": "m1", "lease": 999, "timings": { "kind": 5 } });
        assert!(serde_json::from_value::<Done>(body).unwrap().timings.is_none());
        // One with no CPU time and its phases' classes, as an older record has them.
        let page = serde_json::json!({ "v": 1, "kind": "task tail", "id": "task 7", "start": 1, "wall_s": 9.5, "ok": true, "untimed_s": 0.5, "overhead_s": 0, "phases": [{ "name": "fetching its files from the coordinator", "class": "net", "wall_s": 2.0, "n": 1, "bytes": 1000, "files": 3 }, { "name": "sampling the elevations (elev)", "class": "mixed", "wall_s": 7.0, "n": 1, "bytes": 0, "files": 0 }] });
        let d: Done =
            serde_json::from_value(serde_json::json!({ "worker": "ipad", "lease": 999, "timings": page })).unwrap();
        let t = d.timings.unwrap();
        assert_eq!(
            (t.kind.as_str(), t.phases.len(), t.phases[1].class.as_str(), t.phases[0].cpu_s),
            ("task tail", 2, "mixed", None)
        );
    }

    /// (Task #863) a lease names the slot running its job once its worker says (a member's beat, the
    /// lead's loop's `keep_own`): the forecast's input names that slot's lane; one not said yet, or
    /// read back from an older app's leases, names its Mac.
    #[test]
    fn a_lease_names_the_slot_its_worker_says_runs_it() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = start_for_test(&d.path().join("coord"), "m4", "");
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.offer_units("2026-09-28", vec![("6/55/25".into(), "k".into(), 100 << 20)]);
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let own = c.hold("unit", &[("6/1/1".into(), "k".into())]).unwrap();
        let workers = |c: &Coordinator| {
            let mut v: Vec<(String, bool)> = c.for_forecast().1.into_iter().map(|l| (l.0, l.4)).collect();
            v.sort();
            v
        };
        assert_eq!(workers(&c), [("m1".to_string(), false), ("m4".to_string(), false)], "not said yet: its Mac");
        assert!(w.beat_told(g.lease, None, None, Some(2)).unwrap().0);
        c.keep_own([(own, 1)].into());
        assert_eq!(workers(&c), [("m1 #3".to_string(), true), ("m4 #2".to_string(), true)]);
        // (An older app's lease, no slot: read back as its Mac's.)
        let mut v = {
            let s = c.shared.lock().unwrap();
            serde_json::to_value(s.leases.get(own, "m4", s.now()).unwrap()).unwrap()
        };
        assert_eq!(v["slot"], 1);
        v.as_object_mut().unwrap().remove("slot");
        assert_eq!(serde_json::from_value::<Lease>(v).unwrap().slot, None);
    }

    /// The owner's cancel (task #373, `scenic job cancel`): a member's lease told in its beat's
    /// answer; given back, nothing kept from it (no failure, no backoff); with --rerun-after-update
    /// its target held from every worker, the lead's own jobs too, until the lead runs a newer app,
    /// the hold kept across a restart and carried in the lease snapshot; a member's agent's ask
    /// over HTTP the same.
    #[test]
    fn a_cancelled_job_is_given_back_and_held_until_a_newer_app() {
        use crate::control::Cancel;
        let (old, new) = ("20261018-0100-aaaaaaa", "20261018-0300-bbbbbbb");
        let d = tempfile::tempdir().unwrap();
        let (c, port) = start_for_test(&d.path().join("coord"), "m4", old);
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        let on = |app: &str| Ask { app: Some(app.into()), ..ask(4096) };
        c.offer_units("2026-09-28", vec![("6/55/25".into(), "k".into(), 100 << 20)]);
        let g = w.ask(&on(old)).unwrap().unwrap();
        // Nothing told before a cancel; a cancel of another target tells nothing.
        assert!(w.beat_told(g.lease, None, None, None).unwrap().2.is_none());
        // (Task #518: what its job is doing now, as its beat says, kept on the lease and shown.)
        let live = crate::agent::jobs::Live { phase: Some("z6 tiles".into()), targets: 1, ..Default::default() };
        assert!(w.beat_told(g.lease, None, Some(live), None).unwrap().0);
        let v = c.view();
        assert_eq!(v["leases"][0]["live"]["phase"], "z6 tiles", "{v}");
        let long = crate::agent::jobs::Live { target: Some("x".repeat(WHAT_MAX + 50)), ..Default::default() };
        assert!(w.beat_told(g.lease, None, Some(long), None).unwrap().0);
        let v = c.view();
        assert_eq!(v["leases"][0]["live"]["target"].as_str().unwrap().len(), WHAT_MAX, "capped at the lead");
        assert!(c.cancel(&Cancel::new("6/1/1", None, true, "scenic job cancel on m4")).is_empty());
        assert!(c.cancel(&Cancel::new("6/55/25", Some("pack"), true, "x")).is_empty(), "another step's");
        let told = c.cancel(&Cancel::new("6/55/25", None, true, "scenic job cancel on m4"));
        assert_eq!(told, vec![(g.lease, "m1".to_string(), "unit 6/55/25".to_string())]);
        let (alive, _, cancel) = w.beat_told(g.lease, None, None, None).unwrap();
        assert!(alive && cancel.is_some_and(|x| x.rerun_after_update && x.by == "scenic job cancel on m4"));
        // Given back, not failed: nothing kept from m1, no failure counted.
        w.end_with(g.lease, "cancelled by scenic job cancel on m4", true, &[], &[], 0).unwrap();
        {
            let s = c.shared.lock().unwrap();
            assert!(s.failed.is_empty() && s.workers.get("m1").is_none_or(|w| w.failed == 0));
            assert!(s.history.since(0, 1000).iter().any(|e| e.kind == "cancel" && e.lease == Some(g.lease)));
        }
        // Held on this app: no worker gets it, nor the lead's own job.
        assert!(c.held("unit").contains("6/55/25"));
        assert!(w.ask(&on(old)).unwrap().is_none());
        assert!(c.hold("unit", &[("6/55/25".into(), "k".into())]).is_none());
        assert_eq!(c.holds().len(), 1);
        assert!(c.pool_state().leases["holds"][0]["newer_than"] == old, "carried to the next lead");
        // Kept across a restart on the same app; released by a restart on a newer one, whose
        // workers (on the lead's app) get it again.
        c.stop();
        drop(c);
        let (c, _) = start_for_test(&d.path().join("coord"), "m4", old);
        assert!(c.held("unit").contains("6/55/25"));
        c.stop();
        drop(c);
        let (c, port) = start_for_test(&d.path().join("coord"), "m4", new);
        assert!(c.held("unit").is_empty() && c.holds().is_empty());
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.offer_units("2026-09-28", vec![("6/55/25".into(), "k".into(), 100 << 20)]);
        let g = w.ask(&on(new)).unwrap().unwrap();
        // A member's agent passes on its owner's cancel (no hold asked): told, nothing held.
        let m2 = client::Client::at(w.urls(), c.contact.token.clone(), "m2");
        let told = m2.cancel(&Cancel::new("6/55/25", None, false, "scenic job cancel on m2")).unwrap();
        assert_eq!(told, vec![(g.lease, "m1".to_string(), "unit 6/55/25".to_string())]);
        assert!(w.beat_told(g.lease, None, None, None).unwrap().2.is_some_and(|x| !x.rerun_after_update));
        w.end_with(g.lease, "cancelled", true, &[], &[], 0).unwrap();
        assert!(c.held("unit").is_empty());
        assert!(w.ask(&on(new)).unwrap().is_some(), "it reruns at once");
        // (A hold's release by app: another app not older than its own, or any in a test's "".)
        let h = lease::Hold { step: "u".into(), target: "t".into(), newer_than: new.into(), by: String::new(), at: 0 };
        assert!(!h.released(new) && !h.released(old) && h.released("20261019-0000-ccccccc") && h.released(""));
    }

    #[test]
    fn the_builds_pause_holds_for_every_worker() {
        use crate::control::{Mode, Pause};
        let (d, c, w) = start();
        let units: Vec<(String, String, u64)> =
            (1..=3).map(|i| (format!("6/1/{i}"), format!("k{i}"), 100 << 20)).collect();
        measure_units(&c, 3);
        c.offer_units("2026-09-28", units.clone());
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        // Paused (from a helper's menu, passed on by its agent): no work; an agent's told why and the
        // pause; a worker's beat hears it; no lease lapses meanwhile.
        let p = Pause::new(Mode::Drain, "the menu bar on m1");
        w.set_pause(Some(&p), p.at).unwrap();
        // (An ask from before it, held by a Mac that couldn't reach this one: passed over.)
        w.set_pause(None, p.at - 60).unwrap();
        assert_eq!(c.pause().map(|p| p.by), Some("the menu bar on m1".to_string()));
        let e = w.ask(&ask(4096)).unwrap_err();
        let r = e.downcast_ref::<client::Refused>().unwrap();
        assert!(r.why.contains("paused") && r.pause.as_ref().is_some_and(|p| p.mode == Mode::Drain));
        assert!(w.beat_paused(g.lease, None).unwrap().1.is_some());
        assert!(c.expire().is_empty());
        // A restart keeps it.
        drop(c);
        let (c, port2) = start_for_test(&d.path().join("coord"), "m4", "");
        assert!(c.pause().is_some());
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port2}")], c.contact.token.clone(), "m1");
        c.offer_units("2026-09-28", units.clone());
        // Going on: work again. A job paused at a safe point hands off what it did (those done, the
        // rest offered again); one stopped before any is given back, not held against its targets.
        c.set_pause(None, p.at + 1);
        assert_eq!(
            w.beat_paused(g.lease, None).unwrap(),
            (true, None),
            "its lease kept through the pause and the restart"
        );
        let Granted::Job { targets, .. } = &g.work else { panic!() };
        assert_eq!(targets.len(), 3);
        let part = handoff(&[("6/1/3", "k3")]);
        assert_eq!(
            w.done(&Done { lease: g.lease, handoff: Some(part), ..Default::default() }).unwrap(),
            client::Handed::Taken
        );
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g.work else { panic!() };
        assert_eq!(targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["6/1/1", "6/1/2"]);
        w.give_back(g.lease, "the build paused before it finished a z3 tile").unwrap();
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let Granted::Job { targets, .. } = &g.work else { panic!() };
        assert_eq!(targets.len(), 2, "given back, not failed: offered to it again at once");
    }

    #[test]
    fn candidates_are_expected_to_take_what_their_unit_did() {
        let cost = |mb: u64| Cost { peak_mb: mb, secs: 1, worker: None, v: cost_version("unit"), ..Default::default() };
        let mut costs = BTreeMap::new();
        // Before either ran: as a z6 tile on its extract would (ten times it, at least 3.7 GB).
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 500 << 20), 5000);
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 100 << 20), 3700);
        // The z6 tile's run, then its own.
        costs.insert("6/1/1".to_string(), cost(4200));
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 100 << 20), 4200);
        costs.insert("pois 6/1/1".to_string(), cost(1200));
        assert_eq!(job_peak(&costs, "pois", "6/1/1", 100 << 20), 1200);
        // Another step: its own, else its offer's guess.
        assert_eq!(job_peak(&costs, "slope-hi", "3/2/2", 3000), 3000);
    }

    #[test]
    fn an_agent_on_another_app_gets_no_work() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = start_for_test(&d.path().join("coord"), "m4", "20261005-1508-84142d3");
        let w = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.offer_units("2026-09-28", vec![("6/1/1".into(), "k1".into(), 100 << 20)]);
        // One from before agents said (an older helper's ask), or on another: refused, why given.
        let e = w.ask(&ask(4096)).unwrap_err();
        assert!(
            e.downcast_ref::<client::Refused>().is_some_and(|r| r.why.contains("an older one")
                && r.why.contains("5 Oct 15:08 UTC")
                && r.pause.is_none()),
            "{e:#}"
        );
        let e = w.ask(&Ask { app: Some("20261005-0819-7638722".into()), ..ask(4096) }).unwrap_err();
        assert!(e.to_string().contains("5 Oct 08:19 UTC"), "{e:#}");
        assert!(c.held("unit").is_empty());
        // On the same: the z6 tile.
        let g = w.ask(&Ask { app: Some("20261005-1508-84142d3".into()), ..ask(4096) }).unwrap().unwrap();
        assert!(matches!(g.work, Granted::Job { ref step, .. } if step == "unit"));
        // A page (its code is this coordinator's) never says.
        assert_eq!(app_when("development"), "development");
        // A newer one doesn't either (its results would go under the older code's keys), saying it
        // waits for the lead; development ones only with their like.
        let e = w.ask(&Ask { app: Some("20261006-0900-1234567".into()), ..ask(4096) }).unwrap_err();
        assert!(e.to_string().contains("once the lead runs the same"), "{e:#}");
        assert!(!app_ok(Some("20261005-1613-d05125b"), "20261005-1508-84142d3"));
        assert!(
            app_newer("20261005-1613-d05125b", "20261005-1508-84142d3")
                && !app_newer("20261005-1508-84142d3", "20261005-1613-d05125b")
                && !app_newer("development", "20261005-1508-84142d3")
        );
        assert!(
            !app_ok(Some("20261005-1508-aaaaaaa"), "20261005-1613-d05125b") && !app_ok(None, "20261005-1508-84142d3")
        );
        assert!(
            app_ok(Some("development"), "development")
                && !app_ok(Some("development"), "20261005-1508-84142d3")
                && !app_ok(Some("20261005-1613-d05125b"), "development")
        );
        assert!(app_ok(None, ""));
        // Once a readers' switch is recorded on a newer app than the lead's, nothing for any
        // (the lead's own the same as the worker's), a page included; on the switch's own, again.
        c.set_min_app(Some("20261005-1600-ffffffff".into()));
        let e = w.ask(&Ask { app: Some("20261005-1508-84142d3".into()), ..ask(4096) }).unwrap_err();
        assert!(e.to_string().contains("readers' switch was recorded on"), "{e:#}");
        c.set_min_app(Some("20261005-1508-84142d3".into()));
        assert!(w.ask(&Ask { app: Some("20261005-1508-84142d3".into()), ..ask(4096) }).is_ok());
        c.set_min_app(None);
        // Never a panic, whatever a worker says.
        assert!(
            !app_ok(Some("20261005-150é-x"), "20261005-1508-84142d3")
                && !app_ok(Some("é"), "20261005-1508-84142d3")
                && !app_ok(Some(""), "x")
        );
    }

    /// A target offered under its step's "none" key (crate::agent::build::is_none_key) is a drop:
    /// sized by its step's drops (crate::needs::Book::drop_need), never by its builds; until a drop
    /// has run, by its step's model as before.
    #[test]
    fn a_drop_is_sized_by_its_steps_drops() {
        let (_d, c, _w) = start();
        let none = crate::agent::build::none_deep_key("terrain-deep", "10/100/252");
        c.offer(
            "p",
            vec![Offer {
                step: "terrain-deep".into(),
                targets: vec![("10/100/252".into(), none, 7000), ("10/256/400".into(), "k".into(), 7000)],
                job: None,
            }],
        );
        let v = cost_version("terrain-deep");
        let put = |t: &str, mb: u64, secs: u64, drop: bool| {
            let sizes = if drop { [(crate::needs::DROPS.to_string(), 1)].into() } else { BTreeMap::new() };
            c.shared
                .lock()
                .unwrap()
                .costs
                .insert(format!("terrain-deep {t}"), Cost { peak_mb: mb, secs, v, sizes, ..Default::default() });
        };
        for (i, t) in ["10/256/352", "10/256/353", "10/257/352"].iter().enumerate() {
            put(t, 7000 + i as u64, 236, false);
        }
        let build = c.measured("terrain-deep", "10/256/400").unwrap().mem_mb;
        assert!(build >= 7000);
        assert_eq!(c.measured("terrain-deep", "10/100/252").unwrap().mem_mb, build, "no drop ran yet: as a build");
        put("10/100/250", 150, 2, true);
        assert_eq!(c.measured("terrain-deep", "10/100/252").unwrap().mem_mb, 150, "its step's drops'");
        assert_eq!(c.measured("terrain-deep", "10/256/400").unwrap().mem_mb, build, "a build as before");
        let s = c.shared.lock().unwrap();
        assert_eq!(s.secs_of("terrain-deep", "10/100/252", "m1"), Some(2));
        assert_eq!(s.secs_of("terrain-deep", "10/256/400", "m1"), Some(236));
    }

    #[test]
    fn a_disk_floor_is_learned_rises_and_moves_and_a_step_measured_once_has_a_prior() {
        let (_d, c, w) = start();
        c.offer(
            "p",
            vec![Offer { step: "peaks".into(), targets: vec![("6/1/1".into(), "k 6/1/1".into(), 2500)], job: None }],
        );
        let g = w.ask(&Ask { can: vec!["peaks".into()], ..ask(8192) }).unwrap().unwrap();
        // A cold start: no target of its step measured.
        assert_eq!(c.measured("peaks", "6/1/1"), None);
        // Given back with what it took on the disk (its lease's targets' alone).
        w.end_with(
            g.lease,
            "the build paused",
            true,
            &[],
            &[(cost_key("peaks", "6/1/1"), 7000), (cost_key("peaks", "6/9/9"), 1)],
            0,
        )
        .unwrap();
        assert_eq!(c.disks(), [(cost_key("peaks", "6/1/1"), 7000)].into());
        // Only rises; it moves with the pool's state.
        c.add_disks(&[(cost_key("peaks", "6/1/1"), 3000)]);
        assert_eq!(c.disks().get(&cost_key("peaks", "6/1/1")), Some(&7000));
        assert_eq!(c.pool_state().disks.get(&cost_key("peaks", "6/1/1")), Some(&7000));
        // Measured once: its own memory and disk; another target of its step, the step's largest
        // (a prior learned), until its own run says.
        c.add_costs(&[(
            cost_key("peaks", "6/1/1"),
            Cost { peak_mb: 3000, secs: 60, worker: None, v: 0, ..Default::default() },
        )]);
        assert_eq!(c.measured("peaks", "6/1/1"), Some(Measured { mem_mb: 3000, disk_mb: 7000, own: true }));
        assert_eq!(c.measured("peaks", "6/2/2"), Some(Measured { mem_mb: 3000, disk_mb: 7000, own: false }));
        assert_eq!(c.measured("pois", "6/2/2"), None);
        // A target measured at 0 keeps its 0, not its step's prior; one outlier doesn't make the
        // step's prior (its 90th percentile, not its most: crate::needs).
        c.add_disks(&[(cost_key("peaks", "6/3/3"), 0)]);
        assert_eq!(c.measured("peaks", "6/3/3").map(|m| m.disk_mb), Some(0));
        let many: Vec<(String, u64)> = (10..30).map(|i| (cost_key("peaks", &format!("6/{i}/1")), 1000)).collect();
        c.add_disks(&many);
        c.add_disks(&[(cost_key("peaks", "6/30/1"), 50_000)]);
        assert_eq!(c.measured("peaks", "6/99/1").map(|m| m.disk_mb), Some(1000));
        // A batch: its targets' disks summed, or its step's last jobs' 90th percentile, the larger.
        assert_eq!(c.measured_batch("peaks", &["6/1/1".into(), "6/10/1".into()]).map(|m| m.disk_mb), Some(8000));
        c.add_job_disk("peaks", 20_000);
        assert_eq!(c.measured_batch("peaks", &["6/1/1".into(), "6/10/1".into()]).map(|m| m.disk_mb), Some(20_000));
        assert_eq!(c.pool_state().job_disks.get("peaks"), Some(&20_000));
        // (Task #392: not the most ever, the last runs' 90th percentile: one big job is passed over
        // once ten more have run.)
        for _ in 0..10 {
            c.add_job_disk("peaks", 2_000);
        }
        assert_eq!(c.job_disk("peaks"), 2_000);
        assert_eq!(c.pool_state().job_peaks["peaks"].len(), 11);
        for _ in 0..JOB_PEAKS {
            c.add_job_disk("peaks", 3_000);
        }
        assert_eq!(c.pool_state().job_peaks["peaks"], vec![3_000; JOB_PEAKS], "the last JOB_PEAKS kept");
        // A halo step's batch (task #260: `pack`): the most of its targets', not their sum; its runs
        // a floor of each job, as any step's reading no shared cache (task #396: its reads its own).
        let halo: Vec<String> = (0..110).map(|i| format!("6/{i}/2")).collect();
        c.add_disks(&halo.iter().map(|t| (cost_key("pack", t), 1300)).collect::<Vec<_>>());
        c.add_costs(&[(
            cost_key("pack", &halo[0]),
            Cost { peak_mb: 1000, secs: 30, worker: None, v: 0, ..Default::default() },
        )]);
        assert_eq!(c.measured_batch("pack", &halo).map(|m| m.disk_mb), Some(1300));
        c.add_job_run("pack", &halo[..1], 1_500);
        assert_eq!((c.job_disk("pack"), c.fill_mb("base packs")), (1_500, 0));
        // A step reading a shared cache (task #392; `depth`, the local copies, task #551): a run past
        // its target's output that cache's fill (`fills`), not a floor of each job; a warm run's
        // leaves it.
        let (u, cache) = (["6/1/1".to_string()], crate::localread::CACHE);
        c.add_job_run("depth", &u, 18_584);
        assert_eq!((c.job_disk("depth"), c.fill_mb(cache)), (0, 18_584));
        for _ in 0..20 {
            c.add_job_run("depth", &u, 5_900);
        }
        assert_eq!(c.fill_mb(cache), 18_584, "warm runs leave the fill");
        c.add_job_run("depth", &u, 5_900 + 3_000);
        assert_eq!(c.fill_mb(cache), 18_584, "a part refilled after an eviction leaves it");
        c.floors(true, Some("pack"), None);
        c.add_disks(&halo.iter().map(|t| (cost_key("pack", t), 1300)).collect::<Vec<_>>());
        assert_eq!(c.measured_batch("pack", &halo).map(|m| m.disk_mb), Some(1300));
        // The files before v4: each peak one run, the stale steps' (pack's and lo's always, task
        // #396) and those far above their targets' measures (task #392) dropped.
        let fresh = tempfile::tempdir().unwrap();
        let mut d: BTreeMap<String, u64> = [(cost_key("pack", "6/1/1"), 300)].into();
        assert_eq!(job_disks_load(fresh.path(), &mut d, true), Default::default());
        assert!(fresh.path().join(JOB_DISKS).exists(), "a fresh folder's v6 written: the drop done once");
        let mut d: BTreeMap<String, u64> = [(cost_key("pack", "6/1/1"), 300)].into();
        job_disks_load(fresh.path(), &mut d, true);
        assert_eq!(d.len(), 1, "not dropped again");
        let old = tempfile::tempdir().unwrap();
        std::fs::write(old.path().join("job-disks.json"), r#"{"pack":146944,"lo":46000,"peaks":20000}"#).unwrap();
        assert_eq!(
            job_disks_load(old.path(), &mut BTreeMap::new(), true),
            ([("peaks".to_string(), vec![20_000])].into(), BTreeMap::new())
        );
        std::fs::remove_file(old.path().join(JOB_DISKS)).unwrap();
        std::fs::remove_file(old.path().join(FILLS)).unwrap();
        std::fs::write(
            old.path().join("job-disks-v3.json"),
            r#"{"pack":68279,"unit":18584,"depth":30584,"peaks":5037}"#,
        )
        .unwrap();
        let mut measured: BTreeMap<String, u64> =
            [(cost_key("pack", "6/1/1"), 300), (cost_key("depth", "6/1/1"), 584), (cost_key("peaks", "6/1/1"), 500)]
                .into();
        let fill = || -> BTreeMap<String, u64> { [(crate::localread::CACHE.to_string(), 30_000)].into() };
        assert_eq!(
            job_disks_load(old.path(), &mut measured, true),
            (BTreeMap::new(), fill()),
            "depth's 30.6 GB its cache's fill less its output, not a run; unit's (the deleted DEM cache's \
             fill) and pack's gone; 5 GB against 0.5 GB dropped"
        );
        assert!(!measured.contains_key(&cost_key("pack", "6/1/1")), "pack's targets' disks gone too");
        std::fs::write(old.path().join(JOB_DISKS), r#"{"peaks":[1300]}"#).unwrap();
        assert_eq!(
            job_disks_load(old.path(), &mut measured, true),
            ([("peaks".to_string(), vec![1300])].into(), fill())
        );
        // Cleared with the floors.
        c.floors(true, Some("peaks"), None);
        c.floors(true, Some("pack"), None);
        c.floors(true, Some("depth"), None);
        assert!(c.disks().is_empty());
    }

    /// Task #398: depth-lo's 35 GB job peak (its targets 65 MB) and a target's outlier are dropped
    /// as the coordinator loads them, so its jobs are sized by their measured use and fit any Mac.
    #[test]
    fn depth_lo_is_sized_by_its_measured_use() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("coord");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(JOB_DISKS), r#"{"depth-lo":[35000,70,65],"peaks":[35000]}"#).unwrap();
        std::fs::write(
            dir.join("disks.json"),
            serde_json::to_vec(&BTreeMap::from([
                ("depth-lo 3/1/3".to_string(), 65u64),
                ("depth-lo 3/2/2".into(), 60),
                ("depth-lo 3/2/3".into(), 36_000),
                ("peaks 6/1/1".into(), 10),
            ]))
            .unwrap(),
        )
        .unwrap();
        let (c, _port) = start_for_test(&dir, "m4", "");
        // (A shared cache's step since task #551: its jobs' runs that cache's fill, never a job floor.)
        assert_eq!(c.job_disk("depth-lo"), 0);
        assert!(!c.disks().contains_key("depth-lo 3/2/3"));
        assert_eq!(c.disks().get("depth-lo 3/1/3"), Some(&65));
        // Another step's measures stay as they were.
        assert_eq!(c.job_disk("peaks"), 35_000);
        assert!(super::super::agent::every::offered("depth-lo"));
    }

    /// Task #381: `lo` a halo step (a pass's z3 tiles their owners' base packs): a job of them
    /// reserves the most of its tiles' own, not the sum; task #392: the z3 `terrain` and `trees`
    /// runs' measures (an old step shape) gone; task #396: `pack` and `lo` no longer fill a cache of
    /// base packs, so the v4 file's runs of them and their targets' disks (the copies counted in)
    /// are dropped once as the coordinator starts, and the base packs' fill with them.
    #[test]
    fn lo_jobs_take_their_tiles_disk_and_stale_measures_go() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("coord");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("job-disks-v4.json"), r#"{"lo":[42971,500],"pack":[66000],"slope-hi":[65]}"#).unwrap();
        std::fs::write(dir.join(FILLS), r#"{"base packs":42471,"DEM":12684,"whole copies":900}"#).unwrap();
        std::fs::write(
            dir.join("disks.json"),
            // (A map, not one raw string across lines: the guards' scanners read a line at a time.)
            serde_json::to_vec(
                &[("lo 3/1/1".to_string(), 500), ("lo 3/0/1".into(), 43_008), ("lo 3/2/1".into(), 400)]
                    .into_iter()
                    .chain((3..=9).map(|i| (format!("lo 3/{i}/1"), 0)))
                    .chain([("terrain-hi 3/1/1".to_string(), 9000), ("terrain-hi 6/1/1".into(), 900)])
                    .collect::<BTreeMap<String, u64>>(),
            )
            .unwrap(),
        )
        .unwrap();
        let cost =
            |mb: u64| Cost { peak_mb: mb, secs: 60, worker: None, v: cost_version("terrain-hi"), ..Default::default() };
        std::fs::write(
            dir.join("costs.json"),
            serde_json::to_vec(&BTreeMap::from([
                ("terrain-hi 3/1/1".to_string(), cost(30_000)),
                ("trees 3/1/1".to_string(), cost(30_000)),
                ("terrain-hi 6/1/1".to_string(), cost(3_000)),
            ]))
            .unwrap(),
        )
        .unwrap();
        let (c, _port) = start_for_test(&dir, "m4", "");
        assert_eq!(
            (c.fill_mb("base packs"), c.fill_mb("DEM"), c.fill_mb(crate::localread::CACHE)),
            (0, 0, 900),
            "only the caches there are: the base packs' and the DEM cache's gone"
        );
        assert_eq!((c.job_disk("lo"), c.job_disk("pack"), c.job_disk("slope-hi")), (0, 0, 65));
        assert_eq!(c.measured("terrain-hi", "6/9/9").map(|m| (m.mem_mb, m.disk_mb)), Some((3_000, 900)));
        assert!(!c.disks().contains_key("terrain-hi 3/1/1"));
        assert!(!c.disks().keys().any(|k| k.starts_with("lo ")), "lo's targets' disks gone");
        // Each tile its own disk: the most of theirs, not their sum; a job's run a floor of the next.
        let tiles: Vec<String> = (0..=2).map(|i| format!("3/{i}/1")).collect();
        for (t, mb) in tiles.iter().zip([500, 2_000, 400]) {
            c.add_costs(&[(
                cost_key("lo", t),
                Cost { peak_mb: 800, secs: 90, worker: None, v: 0, ..Default::default() },
            )]);
            c.add_disks(&[(cost_key("lo", t), mb)]);
        }
        assert_eq!(c.measured_batch("lo", &tiles).map(|m| m.disk_mb), Some(2_000));
        c.add_job_run("lo", &tiles, 2_100);
        drop(c);
        // Started again: v6 read, nothing more dropped.
        let (c, _port) = start_for_test(&dir, "m4", "");
        assert_eq!((c.job_disk("lo"), c.fill_mb("base packs")), (2_100, 0));
        assert_eq!(c.disks().get("lo 3/1/1"), Some(&2_000));
    }

    #[test]
    fn a_floor_keeps_a_target_from_a_worker_too_small_and_a_measure_takes_its_place() {
        let (_d, c, w) = start();
        let o = |ts: &[(&str, u64)], _: usize| Offer {
            step: "peaks".into(),
            targets: ts.iter().map(|(t, m)| (t.to_string(), format!("k {t}"), *m)).collect(),
            job: None,
        };
        c.offer("p", vec![o(&[("6/1/1", 2500)], 1)]);
        let can = |mem: u64| Ask { can: vec!["peaks".into()], limit_mb: Some(12 << 10), ..ask(mem) };
        let f = |mb: u64, alone: bool| Floor { mb, alone, v: 0 };
        let g = w.ask(&can(4096)).unwrap().unwrap();
        assert_eq!(c.largest_limit(), Some(12 << 10), "its Mac's limit, as it says");
        // Stopped past its Mac's limit: given back with what it held, the floor of its target (not
        // another's, nor a target of no lease of it).
        w.give_back_with(
            g.lease,
            "stopped by the memory guard",
            &[(cost_key("peaks", "6/1/1"), f(9000, true)), (cost_key("peaks", "6/9/9"), f(1, true))],
        )
        .unwrap();
        assert_eq!((c.floor("peaks", "6/1/1"), c.floor("peaks", "6/9/9")), (Some(f(9000, true)), None));
        assert_eq!(c.peak("peaks", "6/1/1"), Some(9000), "never below its floor");
        // Not to a worker that spares less; to one that spares more.
        assert!(w.ask(&can(4096)).unwrap().is_none());
        let g = w.ask(&can(10 << 10)).unwrap().unwrap();
        // A floor only rises (a lower one seen later changes nothing); it moves with the pool's state.
        w.give_back_with(g.lease, "the build paused", &[(cost_key("peaks", "6/1/1"), f(5000, true))]).unwrap();
        assert_eq!(c.floor("peaks", "6/1/1").map(|f| f.mb), Some(9000));
        assert_eq!(c.pool_state().floors.get(&cost_key("peaks", "6/1/1")).map(|f| f.mb), Some(9000));
        // A run's measure takes its place.
        c.add_costs(&[(
            cost_key("peaks", "6/1/1"),
            Cost { peak_mb: 3000, secs: 60, worker: None, v: 0, ..Default::default() },
        )]);
        assert_eq!((c.floor("peaks", "6/1/1"), c.peak("peaks", "6/1/1")), (None, Some(3000)));
        let g = w.ask(&can(4096)).unwrap().unwrap();
        // A member's done carries its floors (of the lease's targets alone).
        let h = Handoff {
            changes: [("work/peaks/6-1-1".to_string(), Some("work/peaks/6-1-1.0000000000000003.sect".to_string()))]
                .into(),
            done: Some(("peaks".into(), vec![("6/1/1".into(), "k 6/1/1".into())])),
            ..Default::default()
        };
        w.done(&Done {
            lease: g.lease,
            handoff: Some(h),
            floors: vec![(cost_key("peaks", "6/1/1"), f(3500, true)), (cost_key("peaks", "6/7/7"), f(9, true))],
            ..Default::default()
        })
        .unwrap();
        assert_eq!((c.floor("peaks", "6/1/1").map(|f| f.mb), c.floor("peaks", "6/7/7")), (Some(3500), None));
        // A batch's floor gives way to one learned alone, even lower; a floor of an older way of its
        // step neither holds the target nor sizes it (crate::needs: a z6 tile's v1 floors held malloc's
        // cached blocks).
        c.add_floors(&[(cost_key("peaks", "6/2/2"), f(20_000, false))]);
        c.add_floors(&[(cost_key("peaks", "6/2/2"), f(4000, true))]);
        assert_eq!(c.floor("peaks", "6/2/2"), Some(f(4000, true)));
        c.add_floors(&[(cost_key("terrain-hi", "6/3/3"), Floor { mb: 20_000, alone: true, v: 1 })]);
        assert_eq!((c.holding_floor("terrain-hi", "6/3/3"), c.peak("terrain-hi", "6/3/3")), (None, None));
        // The OSM pass's floor puts an earlier pass's away.
        c.add_floors(&[(cost_key("osm-pass", "2026-08-01"), f(30_000, true))]);
        c.add_floors(&[(cost_key("osm-pass", "2026-09-28"), f(31_000, true))]);
        assert_eq!(
            (c.floor("osm-pass", "2026-08-01"), c.floor("osm-pass", "2026-09-28").map(|f| f.mb)),
            (None, Some(31_000))
        );
        // A target whose floor is a batch's, under what a worker spares, goes in a run as any.
        c.offer("p", vec![o(&[("6/4/1", 100), ("6/4/2", 100), ("6/4/3", 100)], 3)]);
        c.add_floors(&[(cost_key("peaks", "6/4/2"), f(2000, false))]);
        let g = w.ask(&can(8 << 10)).unwrap().unwrap();
        let Granted::Job { targets, .. } = g.work else { panic!() };
        assert_eq!(targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["6/4/1", "6/4/2", "6/4/3"]);
        // Kept across a restart of the coordinator; cleared as the owner asks (`scenic pool floors`).
        let kept: BTreeMap<String, Floor> =
            serde_json::from_slice(&std::fs::read(c.shared.lock().unwrap().dir.join("floors.json")).unwrap()).unwrap();
        assert_eq!(kept.get(&cost_key("peaks", "6/2/2")), Some(&f(4000, true)));
        let (code, v) = w
            .post_json("/work/floors", &serde_json::json!({ "clear": true, "step": "peaks", "target": "6/2/2" }))
            .unwrap();
        assert!(
            code == 200
                && v["floors"].get(cost_key("peaks", "6/2/2")).is_none()
                && v["floors"].get(cost_key("peaks", "6/1/1")).is_some(),
            "{v}"
        );
        assert!(c.floors(true, Some("peaks"), None).keys().all(|k| !k.starts_with("peaks ")));
        assert!(c.floors(true, None, None).is_empty());
    }

    #[test]
    fn a_clear_over_http_changes_what_the_running_coordinator_sizes_and_holds() {
        let (_d, c, w) = start();
        c.offer(
            "p",
            vec![Offer { step: "peaks".into(), targets: vec![("6/1/1".into(), "k".into(), 2500)], job: None }],
        );
        c.add_floors(&[(cost_key("peaks", "6/1/1"), Floor { mb: 43_000, alone: true, v: 0 })]);
        assert_eq!(c.peak("peaks", "6/1/1"), Some(43_000));
        assert!(c.holding_floor("peaks", "6/1/1").is_some());
        let (code, v) = w.post_json("/work/floors", &serde_json::json!({ "clear": true, "step": "peaks" })).unwrap();
        assert_eq!(code, 200);
        assert_eq!((v["cleared"]["floors"].as_u64(), v["cleared"]["disks"].as_u64()), (Some(1), Some(0)), "{v}");
        assert_eq!(c.holding_floor("peaks", "6/1/1"), None, "the floor held it off a Mac still");
        assert!(c.peak("peaks", "6/1/1").unwrap() < 43_000, "and sized it: {:?}", c.peak("peaks", "6/1/1"));
    }

    /// Task #780: the owner's redo (`scenic redo`, with the build's key) is taken for the agent, its
    /// step's current name and its targets once each, numbered; a step with no key per target, a
    /// bad target or a page's ask is refused.
    #[test]
    fn the_owners_redo_is_taken_for_the_agent() {
        let (_d, c, w) = start();
        let (code, v) = w
            .post_json(
                "/work/redo",
                &serde_json::json!({ "step": "terrain", "targets": ["6/18/22", "6/18/22", "6/18/23"], "by": "t" }),
            )
            .unwrap();
        assert_eq!(code, 200, "{v}");
        assert!(v["after"]["term"].is_u64() && v["after"]["n"].is_u64(), "{v}");
        let asks = c.take_redo_asks();
        assert_eq!(asks.len(), 1);
        assert_eq!(
            (asks[0].n, asks[0].step.as_str(), asks[0].targets.clone()),
            (v["n"].as_u64().unwrap(), "terrain-hi", vec!["6/18/22".to_string(), "6/18/23".into()])
        );
        for bad in [
            serde_json::json!({ "step": "catalog", "targets": ["x"] }),
            serde_json::json!({ "step": "terrain-hi", "targets": [] }),
            serde_json::json!({ "step": "terrain-hi", "targets": ["6/1/../2"] }),
        ] {
            assert_ne!(w.post_json("/work/redo", &bad).map(|r| r.0).unwrap_or(500), 200, "{bad}");
        }
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let page = send(
            &addr,
            "POST",
            "/work/redo",
            None,
            &[],
            &serde_json::json!({ "step": "terrain-hi", "targets": ["6/1/1"] }),
        )
        .0;
        assert_eq!(page, 403, "not a page's");
        assert!(c.take_redo_asks().is_empty());
        // Task #832: a restore (`scenic redo --restore`), its targets those it names.
        let k = |c: char| c.to_string().repeat(16);
        let restore = serde_json::json!([{ "target": "6/18/22", "from": k('a'), "to": k('b') }]);
        let (code, v) =
            w.post_json("/work/redo", &serde_json::json!({ "step": "terrain-hi", "restore": restore })).unwrap();
        assert_eq!(code, 200, "{v}");
        let asks = c.take_redo_asks();
        assert_eq!((asks[0].targets.clone(), asks[0].restore[0].to.clone()), (vec!["6/18/22".to_string()], k('b')));
        let bad =
            serde_json::json!({ "step": "terrain-hi", "restore": [{ "target": "6/1/1", "from": "x", "to": "y" }] });
        assert_ne!(w.post_json("/work/redo", &bad).map(|r| r.0).unwrap_or(500), 200);
    }

    #[test]
    fn a_floor_of_before_a_steps_fix_neither_holds_nor_sizes_it() {
        let (_d, c, _w) = start();
        assert_eq!(cost_version("bld-fetch"), 1);
        c.add_floors(&[(cost_key("bld-fetch", "6/1/1"), Floor { mb: 43_000, alone: true, v: 0 })]);
        assert_eq!(c.holding_floor("bld-fetch", "6/1/1"), None);
        assert!(c.peak("bld-fetch", "6/1/1").is_none_or(|mb| mb < 43_000));
    }

    #[test]
    fn a_take_up_holds_the_terms_leases_alone() {
        let (_d, c, _w) = start();
        // A lease this coordinator's folder kept from a term its Mac led before, ended since under
        // another lead; the term taken up holds another.
        let old = c.hold("trees", &[("6/33/22".into(), "k".into())]).unwrap();
        let p = PoolState {
            leases: serde_json::json!({ "next": old + 5, "leases": [{ "id": old + 3, "term": 3, "granted_at": 0, "worker": "m1", "work": { "Job": { "step": "terrain-hi", "targets": [["6/33/22", "k2"]] } }, "progress": null }] }),
            ..Default::default()
        };
        assert_eq!(c.load_pool_state(&p).unwrap(), 1);
        assert!(c.held("trees").is_empty(), "the old lease is gone");
        assert!(c.held("terrain-hi").contains("6/33/22"));
        // (Ids go on past both.)
        assert!(c.hold("slope-hi", &[("3/1/1".into(), "k".into())]).unwrap() >= old + 5);
    }

    /// Task #657: the scheduler's look-ups kept (`Sched::offered`, `measured_of`, the Book's NAS
    /// rates) grant as they'd grant worked out afresh. Two coordinators take one random run of
    /// offers, measures (costs, floors, disk floors), asks, finishes, failures and lapses; before
    /// each ask the second's kept look-ups are dropped (its offers indexed again, its memo and
    /// models emptied). Their grants are the same, every one; and the offer index, `failed_any` and
    /// `done_any` say what a scan of the whole says.
    #[test]
    fn kept_lookups_grant_as_fresh_ones() {
        let (_da, a, _wa) = start();
        let (_db, b, _wb) = start();
        let t0 = Instant::now();
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n.max(1)
        };
        let steps = ["pack", "peaks", "terrain-hi", "depth"];
        let tile = |i: u64| format!("6/{}/{}", i % 7, i / 7);
        // (Each job's lease in each: their ids are each coordinator's own.)
        let mut leases: Vec<(u64, u64)> = Vec::new();
        let (mut grants, mut offered) = (0usize, 0usize);
        for round in 0..600u64 {
            let now = t0 + Duration::from_secs(round * 7);
            let clock = sched::Clock { now, unix: 1_800_000_000 + round * 7 };
            match rnd(10) {
                0 => {
                    let offers: Vec<Offer> = (0..1 + rnd(3))
                        .map(|_| {
                            let st = steps[rnd(steps.len() as u64) as usize];
                            let n = 1 + rnd(12);
                            Offer {
                                step: st.into(),
                                targets: (0..n)
                                    .map(|_| {
                                        let t = tile(rnd(12));
                                        (t.clone(), format!("k {t}"), rnd(3) * 400)
                                    })
                                    .collect(),
                                job: None,
                            }
                        })
                        .collect();
                    offered += 1;
                    a.offer("p", offers.clone());
                    b.offer("p", offers);
                }
                1 => {
                    let (st, t) = (steps[rnd(4) as usize], tile(rnd(12)));
                    let c = Cost {
                        peak_mb: 100 + rnd(30) * 500,
                        secs: 10 + rnd(600),
                        worker: None,
                        v: if rnd(5) == 0 { 0 } else { cost_version(st) },
                        sizes: [("threads".to_string(), 1)].into(),
                        ..Default::default()
                    };
                    a.add_costs_by(&[(cost_key(st, &t), c.clone())], "m1");
                    b.add_costs_by(&[(cost_key(st, &t), c)], "m1");
                }
                2 | 3 => {
                    let (st, t, mb) = (steps[rnd(4) as usize], tile(rnd(12)), rnd(40) * 1000);
                    a.add_disks(&[(cost_key(st, &t), mb)]);
                    b.add_disks(&[(cost_key(st, &t), mb)]);
                }
                4 => {
                    let (st, t) = (steps[rnd(4) as usize], tile(rnd(12)));
                    let f = Floor { mb: 1000 + rnd(20) * 1000, alone: rnd(2) == 0, v: cost_version(st) };
                    a.add_floors(&[(cost_key(st, &t), f)]);
                    b.add_floors(&[(cost_key(st, &t), f)]);
                }
                5 if !leases.is_empty() => {
                    // A job ended: its targets done, or some failed, or its lease lapsed.
                    let ids = leases.remove(rnd(leases.len() as u64) as usize);
                    let how = rnd(3);
                    for (c, id) in [(&a, ids.0), (&b, ids.1)] {
                        let mut s = c.shared.lock().unwrap();
                        let Some(l) = s.leases.all(now).into_iter().find(|l| l.id == id) else { continue };
                        if let Work::Job { step, targets } = &l.work {
                            for (t, k) in targets {
                                match how {
                                    0 => {
                                        s.done.insert((step.clone(), t.clone()), k.clone());
                                    }
                                    1 => {
                                        s.failed.insert((l.worker.clone(), cost_key(step, t)), (now, 1));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        s.end(id);
                    }
                }
                _ => {
                    let lead = rnd(3) == 0;
                    let ask = Ask {
                        worker: if lead { "m4".into() } else { format!("w{}", rnd(3)) },
                        kind: "native".into(),
                        can: steps.iter().map(|s| s.to_string()).collect(),
                        mem_mb: 2_000 + rnd(30) * 1_000,
                        cores: 8,
                        disk_mb: Some(5_000 + rnd(40) * 1_000),
                        ..Default::default()
                    };
                    // (What each offered target and live job measures, as kept and afresh.)
                    let measures = |s: &Shared| {
                        let m: Vec<_> = s
                            .offers
                            .iter()
                            .flat_map(|o| o.targets.iter().map(|t| s.measured(&o.step, &t.0)).collect::<Vec<_>>())
                            .collect();
                        let l: Vec<u64> = s.leases.all(now).iter().map(|l| s.lease_mem(l)).collect();
                        (m, l)
                    };
                    let (ma, pa) = {
                        let mut s = a.shared.lock().unwrap();
                        let offers = s.offers.clone();
                        (measures(&s), s.sched.pack(&ask, &offers, lead, clock).0)
                    };
                    let pb = {
                        let mut s = b.shared.lock().unwrap();
                        let offers = s.offers.clone();
                        s.set_offers(offers.clone());
                        s.needs = Default::default();
                        // (The offer index and the shortcuts against a scan of the whole.)
                        for o in &offers {
                            for t in &o.targets {
                                let first = offers
                                    .iter()
                                    .filter(|x| x.step == o.step)
                                    .find_map(|x| x.targets.iter().find(|y| y.0 == t.0))
                                    .map(|y| y.2);
                                assert_eq!(s.offered_size(&o.step, &t.0), first);
                            }
                            assert_eq!(s.done_any(&o.step), s.done.keys().any(|(st, _)| *st == o.step));
                        }
                        assert_eq!(s.failed_any(&ask.worker), s.failed.keys().any(|(w, _)| *w == ask.worker));
                        assert_eq!(ma, measures(&s), "round {round}: the measures kept");
                        s.sched.pack(&ask, &offers, lead, clock).0
                    };
                    let same = |p: &[sched::Pick]| -> Vec<sched::Pick> {
                        p.iter().map(|x| sched::Pick { lease: 0, ..x.clone() }).collect()
                    };
                    assert_eq!(same(&pa), same(&pb), "round {round}: {ask:?}");
                    grants += pa.len();
                    leases.extend(pa.iter().zip(&pb).map(|(x, y)| (x.lease, y.lease)));
                }
            }
        }
        assert!(grants > 30 && offered > 30, "the run granted {grants} jobs over {offered} offers");
    }

    /// Task #657: a term's state of before `DISKS_V` brings back no `pack`'s nor `lo`'s targets'
    /// disks (those of before task #396, the whole base packs' copy counted in: 9 z6 tiles at
    /// 51–68 GB no Mac's room fitted, never granted) and no fill of a cache there isn't; one of
    /// `DISKS_V` brings its own; a lead's folder of v5 drops them once.
    #[test]
    fn a_take_up_brings_back_no_disks_of_before_the_halos_own() {
        let (_d, c, _w) = start();
        let mut p = PoolState {
            disks: [
                (cost_key("pack", "6/16/20"), 67_875),
                (cost_key("lo", "3/0/1"), 42_971),
                (cost_key("peaks", "6/1/1"), 900),
            ]
            .into(),
            fills: [("base packs".to_string(), 67_985), (crate::localread::CACHE.to_string(), 900)].into(),
            leases: serde_json::json!({ "next": 1, "leases": [] }),
            ..Default::default()
        };
        c.load_pool_state(&p).unwrap();
        let disks = c.disks();
        assert_eq!(
            (disks.get("pack 6/16/20"), disks.get("lo 3/0/1"), disks.get("peaks 6/1/1")),
            (None, None, Some(&900))
        );
        assert_eq!((c.fill_mb("base packs"), c.fill_mb(crate::localread::CACHE)), (0, 900));
        assert_eq!(c.pool_state().disks_v, DISKS_V, "a state written now says its generation");
        p.disks_v = DISKS_V;
        p.disks = [(cost_key("pack", "6/16/20"), 1_300)].into();
        c.load_pool_state(&p).unwrap();
        assert_eq!(c.disks().get("pack 6/16/20"), Some(&1_300), "a state of now brings its own");
        // A lead's folder of v5: its runs kept whole, pack's and lo's targets' disks dropped once.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(JOB_DISKS_V5), r#"{"pack":[1915,1892],"peaks":[1300]}"#).unwrap();
        let mut d: BTreeMap<String, u64> = [
            (cost_key("pack", "6/16/20"), 67_875),
            (cost_key("lo", "3/0/1"), 42_971),
            (cost_key("peaks", "6/1/1"), 900),
        ]
        .into();
        let (m, _) = job_disks_load(dir.path(), &mut d, true);
        assert_eq!(m, [("pack".to_string(), vec![1915, 1892]), ("peaks".to_string(), vec![1300])].into());
        assert_eq!(d, [(cost_key("peaks", "6/1/1"), 900)].into());
        assert!(dir.path().join(JOB_DISKS).exists());
        let mut d: BTreeMap<String, u64> = [(cost_key("pack", "6/1/1"), 300)].into();
        job_disks_load(dir.path(), &mut d, true);
        assert_eq!(d.len(), 1, "not dropped again");
    }

    #[test]
    fn a_take_up_keeps_the_newer_of_a_floor_and_a_measure() {
        let (_d, c, _w) = start();
        let f = |mb: u64| Floor { mb, alone: true, v: 0 };
        c.add_floors(&[(cost_key("peaks", "6/1/1"), f(9000)), (cost_key("peaks", "6/1/2"), f(9000))]);
        // The other lead measured 6/1/1 (its floor gone there), and saw a higher floor for 6/1/3.
        let p = PoolState {
            costs: [(
                cost_key("peaks", "6/1/1"),
                Cost { peak_mb: 3000, secs: 60, worker: None, v: 0, ..Default::default() },
            )]
            .into(),
            floors: [(cost_key("peaks", "6/1/3"), f(7000))].into(),
            leases: serde_json::json!({ "next": 1, "leases": [] }),
            ..Default::default()
        };
        c.load_pool_state(&p).unwrap();
        assert_eq!((c.floor("peaks", "6/1/1"), c.peak("peaks", "6/1/1")), (None, Some(3000)));
        // An old step shape's measures in a term's state aren't taken up (`old_shape`).
        let old =
            |mb: u64| Cost { peak_mb: mb, secs: 60, worker: None, v: cost_version("trees"), ..Default::default() };
        let p = PoolState {
            costs: [(cost_key("trees", "3/1/3"), old(37_450)), (cost_key("trees", "6/4/14"), old(1_360))].into(),
            floors: [(cost_key("trees", "3/2/3"), f(36_000))].into(),
            disks: [(cost_key("terrain-hi", "3/1/1"), 9_000)].into(),
            leases: serde_json::json!({ "next": 1, "leases": [] }),
            ..Default::default()
        };
        c.load_pool_state(&p).unwrap();
        let costs = c.shared.lock().unwrap().costs.clone();
        assert!(!costs.contains_key("trees 3/1/3") && costs.contains_key("trees 6/4/14"), "{costs:?}");
        assert_eq!(c.floor("trees", "3/2/3"), None);
        assert!(!c.disks().contains_key("terrain-hi 3/1/1"));
        assert_eq!(
            (c.floor("peaks", "6/1/2").map(|f| f.mb), c.floor("peaks", "6/1/3").map(|f| f.mb)),
            (Some(9000), Some(7000))
        );
    }

    /// A resumable job offered to the worker whose Mac holds its progress (crate::agent::resume):
    /// granted whole to it alone, whatever another asks with, once while its lease holds it, and
    /// before the plan's other work; none once it's taken off.
    #[test]
    fn a_resumable_job_goes_to_the_worker_holding_its_progress_alone() {
        let (_d, c, m1) = start();
        let port = m1.urls()[0].clone();
        let m2 = client::Client::at(vec![port], c.contact.token.clone(), "m2");
        c.offer(
            "2026-09-01",
            vec![Offer { step: "slope-hi".into(), targets: vec![("3/2/2".into(), "k".into(), 100)], job: None }],
        );
        c.offer_resume(Some((
            "m1".into(),
            Offer {
                step: "osm-pass".into(),
                targets: vec![("2026-10-01".into(), "sources/osm/2026-10-01/planet.osm.pbf".into(), 0)],
                job: None,
            },
        )));
        let can = |steps: &[&str]| Ask { can: steps.iter().map(|s| s.to_string()).collect(), ..ask(4096) };
        // Another worker asking for it: not its (the plan's work instead, or nothing).
        assert!(w_step(&m2, &can(&["osm-pass"])).is_none());
        // Its own, not asking for it (its file no longer holds it): the plan's work.
        assert_eq!(w_step(&m1, &can(&["slope-hi"])).as_deref(), Some("slope-hi 3/2/2"));
        // Asking for it: granted whole, its pass the job's.
        let g = m1.ask(&can(&["osm-pass", "slope-hi"])).unwrap().unwrap();
        let Granted::Job { step, targets, pass } = g.work else { panic!("a job") };
        assert_eq!((step.as_str(), pass.as_str()), ("osm-pass", "2026-10-01"));
        assert_eq!(targets, vec![("2026-10-01".to_string(), "sources/osm/2026-10-01/planet.osm.pbf".to_string())]);
        // Held by its lease: not granted twice.
        assert!(w_step(&m1, &can(&["osm-pass"])).is_none());
        c.finish(g.lease, &[]);
        c.offer_resume(None);
        assert!(w_step(&m1, &can(&["osm-pass"])).is_none());
    }

    /// This Mac's own agent asking lately with `own` room (memory, disk: MB), as the lead's agent
    /// asks (crate::agent's `lead_grants`).
    fn lead_asks(c: &Coordinator, own: (u64, u64)) {
        let mut s = c.shared.lock().unwrap();
        let now = s.now();
        let a = Ask {
            worker: "m4".into(),
            kind: "native".into(),
            can: crate::agent::steps::TABLE.iter().map(|r| r.name.to_string()).collect(),
            mem_mb: own.0,
            disk_mb: Some(own.1),
            ..Default::default()
        };
        s.seen("m4", "asked".into(), Some(&a), now);
    }

    /// Every step to every member (docs/pool.md §12, phase 4's ninth batch): a coordinator on a
    /// clock of the test's (none moves it but the test), this Mac's own agent asking with room
    /// `own` (none asking with (0, 0)); a step's offer of `ts` (target, its measures: memory, disk,
    /// seconds; None: never run).
    fn every(own: (u64, u64)) -> (tempfile::TempDir, Coordinator, client::Client, Arc<store::clock::Virtual>) {
        let (d, c, w) = start();
        let clock = store::clock::Virtual::new();
        c.shared.lock().unwrap().clock = clock.clone();
        if own != (0, 0) {
            lead_asks(&c, own);
        }
        (d, c, w, clock)
    }

    fn measured_offer(c: &Coordinator, step: &str, ts: &[(&str, Option<(u64, u64, u64)>)]) {
        for (t, m) in ts {
            if let Some((mem, disk, secs)) = m {
                // (Measured on m1: its own pace; another worker's run counts twice its time, job_secs.
                // Its run's threads said: a serial step's.)
                let sizes = [("threads".to_string(), 1)].into();
                c.add_costs_by(
                    &[(
                        cost_key(step, t),
                        Cost {
                            peak_mb: *mem,
                            secs: *secs,
                            worker: None,
                            v: cost_version(step),
                            sizes,
                            ..Default::default()
                        },
                    )],
                    "m1",
                );
                c.add_disks(&[(cost_key(step, t), *disk)]);
            }
        }
        c.offer(
            "p",
            vec![Offer {
                step: step.into(),
                targets: ts.iter().map(|(t, _)| (t.to_string(), format!("k {t}"), 0)).collect(),
                job: None,
            }],
        );
    }

    /// A new app's ask (it carries its Mac's capacity: `Ask::cap`).
    fn every_ask(worker: &str, can: &str, mem_mb: u64, disk_mb: u64) -> Ask {
        Ask {
            worker: worker.into(),
            kind: "native".into(),
            can: vec![can.into()],
            mem_mb,
            cores: 8,
            disk_mb: Some(disk_mb),
            cap: Some(Capacity { at: unix_now(), cores: 8, cores_free: 8, ..Default::default() }),
            ..Default::default()
        }
    }

    fn granted(w: &client::Client, a: &Ask) -> Vec<String> {
        match w.ask(a).unwrap().map(|g| g.work) {
            Some(Granted::Job { targets, .. }) => targets.into_iter().map(|t| t.0).collect(),
            _ => Vec::new(),
        }
    }

    /// A member offered the map tiles' `pack` (no shared step) by its measures: a run from the front
    /// of the plan, as many targets as fit a quarter of an hour by their measured times (the run's
    /// first whatever its time), and its next run just after its last.
    #[test]
    fn every_step_a_member_takes_pack_in_runs_of_a_quarter_hour_by_measured_time() {
        let (_d, c, w, _clock) = every((0, 0));
        let m = Some((500, 100, 300));
        measured_offer(
            &c,
            "pack",
            &[("6/1/1", m), ("6/1/2", m), ("6/1/3", m), ("6/1/4", m), ("6/1/5", Some((500, 100, 2000))), ("6/1/6", m)],
        );
        let a = every_ask("m1", "pack", 4096, 10_000);
        // Three of 300 s: 900 s, a quarter of an hour; a fourth would pass it.
        assert_eq!(granted(&w, &a), ["6/1/1", "6/1/2", "6/1/3"]);
        // Next, just after its last: 6/1/4, then 6/1/5's 2000 s past the quarter hour, so alone.
        assert_eq!(granted(&w, &a), ["6/1/4"]);
        assert_eq!(granted(&w, &a), ["6/1/5"], "a run's first whatever its time");
        assert_eq!(granted(&w, &a), ["6/1/6"]);
        assert!(granted(&w, &a).is_empty());
        // A target measured of a step with no time: one a job.
        let (_d, c, w, _clock) = every((0, 0));
        measured_offer(&c, "lo", &[("3/1/1", Some((500, 0, 0))), ("3/1/2", Some((500, 0, 0)))]);
        c.shared.lock().unwrap().costs.clear();
        c.add_floors(&[
            (cost_key("lo", "3/1/1"), Floor { mb: 500, alone: true, v: 0 }),
            (cost_key("lo", "3/1/2"), Floor { mb: 500, alone: true, v: 0 }),
        ]);
        assert_eq!(granted(&w, &every_ask("m1", "lo", 4096, 10_000)), ["3/1/1"]);
    }

    /// A member short of a target's measured memory or disk is offered none of it; its disk summed
    /// over a run ends the run within its room.
    #[test]
    fn every_step_a_member_short_of_a_targets_measures_is_offered_none_of_it() {
        let (_d, c, w, _clock) = every((0, 0));
        measured_offer(&c, "pack", &[("6/1/1", Some((9000, 100, 60))), ("6/1/2", Some((500, 50_000, 60)))]);
        assert!(granted(&w, &every_ask("m1", "pack", 4096, 10_000)).is_empty());
        assert_eq!(granted(&w, &every_ask("m1", "pack", 10_000, 10_000)), ["6/1/1"], "one with the memory");
        assert_eq!(granted(&w, &every_ask("m2", "pack", 10_000, 60_000)), ["6/1/2"], "one with the disk");
        // Each fits, two together don't: the run ends at the disk.
        let (_d, c, w, _clock) = every((0, 0));
        measured_offer(&c, "terrain-lo", &[("3/1/1", Some((500, 6000, 60))), ("3/1/2", Some((500, 6000, 60)))]);
        assert_eq!(granted(&w, &every_ask("m1", "terrain-lo", 4096, 10_000)), ["3/1/1"]);
        // A halo step's (`lo`, task #381): one cache, so both within the room.
        let (_d, c, w, _clock) = every((0, 0));
        measured_offer(&c, "lo", &[("3/1/1", Some((500, 6000, 60))), ("3/1/2", Some((500, 6000, 60)))]);
        assert_eq!(granted(&w, &every_ask("m1", "lo", 4096, 10_000)), ["3/1/1", "3/1/2"]);
    }

    /// A cold start (a step none of whose targets has run) goes, alone in its job, to a worker its
    /// provisional need fits (task #499: the step's declared memory times 3/2), one job of the step
    /// in the pool at a time; once one has run (measured), the hold lifts: the others are offered by
    /// its measures, batched.
    #[test]
    fn every_step_a_cold_start_goes_by_its_provisional_need_one_at_a_time() {
        let (_d, c, m1, _clock) = every((1000, 1000));
        let m2 = client::Client::at(m1.urls(), c.contact.token.clone(), "m2");
        measured_offer(&c, "lo", &[("3/1/1", None), ("3/1/2", None), ("3/1/3", None)]);
        let g = m2.ask(&every_ask("m2", "lo", 8192, 100_000)).unwrap().unwrap();
        assert!(
            matches!(&g.work, Granted::Job { targets, .. } if targets.len() == 1 && targets[0].0 == "3/1/1"),
            "one target, the first"
        );
        // While a job of the step is under way (a member's, or the lead's own: its lease), no other
        // cold start of it.
        assert!(granted(&m1, &every_ask("m1", "lo", 16_000, 100_000)).is_empty(), "one under way");
        m2.give_back(g.lease, "the test").unwrap();
        let own = c.hold("lo", &[("3/1/3".into(), "k 3/1/3".into())]).unwrap();
        assert!(granted(&m1, &every_ask("m1", "lo", 16_000, 100_000)).is_empty(), "the lead's own under way");
        c.finish(own, &[]);
        let g = m1.ask(&every_ask("m1", "lo", 16_000, 100_000)).unwrap().unwrap();
        assert!(matches!(&g.work, Granted::Job { targets, .. } if targets.len() == 1), "to the next asker");
        // Measured: the hold lifts, the others offered by its measures, batched, beside the first.
        c.add_costs(&[(
            cost_key("lo", "3/1/1"),
            Cost { peak_mb: 500, secs: 60, worker: None, v: cost_version("lo"), ..Default::default() },
        )]);
        assert_eq!(granted(&m2, &every_ask("m2", "lo", 4096, 100_000)), ["3/1/2", "3/1/3"]);
    }
    /// `n` targets of `step` measured (2 GB, 100 MB of disk, ten minutes: a job each), their runs'
    /// threads said when `threads` is.
    fn packable(c: &Coordinator, step: &str, n: usize, threads: Option<u64>) {
        let ts: Vec<String> = (1..=n).map(|i| format!("6/1/{i}")).collect();
        for t in &ts {
            let sizes = threads.map(|t| [("threads".to_string(), t)].into()).unwrap_or_default();
            let cost =
                Cost { peak_mb: 2000, secs: 600, worker: None, v: cost_version(step), sizes, ..Default::default() };
            c.add_costs_by(&[(cost_key(step, t), cost)], "m1");
            c.add_disks(&[(cost_key(step, t), 100)]);
        }
        c.offer(
            "p",
            vec![Offer {
                step: step.into(),
                targets: ts.iter().map(|t| (t.clone(), format!("k {t}"), 0)).collect(),
                job: None,
            }],
        );
    }

    fn all_granted(w: &client::Client, a: &Ask) -> Vec<Vec<String>> {
        let Some(mut g) = w.ask(a).unwrap() else { return Vec::new() };
        let more = std::mem::take(&mut g.more);
        std::iter::once(g)
            .chain(more)
            .map(|g| match g.work {
                Granted::Job { targets, .. } => targets.into_iter().map(|t| t.0).collect(),
                Granted::Task { .. } => Vec::new(),
            })
            .collect()
    }

    /// The NAS budget (crate::budget::nas): with packing, an ask is granted only the jobs whose NAS
    /// weights fit what every job leased in the pool leaves (the lead's own held ones among them);
    /// the rest wait until one ends. Off, as before. A fetching step's jobs only under the
    /// downloads' cap.
    #[test]
    fn the_nas_budget_holds_the_pools_jobs_to_the_nas() {
        // (Each target 50 MB/s of a 100 MB/s NAS: 500 thousandths.)
        let heavy = |c: &Coordinator, step: &str| {
            packable(c, step, 6, Some(1));
            for i in 1..=6 {
                let sizes = [
                    ("threads".to_string(), 1),
                    ("bytes".to_string(), 18_000_000_000),
                    ("wrote".to_string(), 12_000_000_000),
                ];
                let cost = Cost {
                    peak_mb: 2000,
                    secs: 600,
                    worker: None,
                    v: cost_version(step),
                    sizes: sizes.into(),
                    ..Default::default()
                };
                c.add_costs_by(&[(cost_key(step, &format!("6/1/{i}")), cost)], "m1");
            }
        };
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        let a = Ask { want: 6, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        let (_d, c, w, _clock) = every((0, 0));
        heavy(&c, "slope-hi");
        assert_eq!(all_granted(&w, &a).len(), 6, "off: by its room and cores alone (its measured 1 each)");
        let (_d, c, w, _clock) = every((0, 0));
        c.set_nas(Some(limits));
        heavy(&c, "slope-hi");
        assert_eq!(all_granted(&w, &a), [["6/1/1"], ["6/1/2"]], "on: two halves of the NAS");
        assert!(all_granted(&w, &a).is_empty(), "the NAS full: queued");
        let (_d, c, w, _clock) = every((0, 0));
        c.set_nas(Some(limits));
        heavy(&c, "slope-hi");
        assert!(c.hold("slope-hi", &[("6/1/6".into(), "k 6/1/6".into())]).is_some());
        assert_eq!(all_granted(&w, &a).len(), 1, "the lead's own job counts");
        // Downloads: a fetching step's jobs under the cap (the lead's own: no fetching step is
        // offered to every member).
        let (_d, c, _w, _clock) = every((0, 0));
        c.set_nas(Some(crate::budget::nas::Limits { downloads: Some(1), ..limits }));
        packable(&c, "rail-feeds", 1, Some(1));
        packable(&c, "bld-fetch", 1, Some(1));
        let own = |step: &str| Offer {
            step: step.into(),
            targets: vec![("6/1/1".into(), "k 6/1/1".into(), 0)],
            job: Some(format!("{step} 6/1/1")),
        };
        c.offer("p", vec![own("rail-feeds"), own("bld-fetch")]);
        let a = Ask {
            want: 3,
            can: vec!["rail-feeds".into(), "bld-fetch".into()],
            ..every_ask("m4", "rail-feeds", 64_000, 100_000)
        };
        let g = c.ask_own(&a, &BTreeSet::new()).unwrap();
        assert!(g.more.is_empty(), "one download at a time");
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none());
    }

    /// A job heavier than the whole (its weight capped at the whole) goes while the lead's own jobs
    /// alone hold weight, not only with nothing leased: the lead's jobs never starve it. One more
    /// waits; a target held back past half an hour is said (`nas_waits`), and let go once granted.
    #[test]
    fn a_job_past_the_whole_goes_beside_the_leads_own_and_a_long_wait_is_said() {
        let (_d, c, w, clock) = every((0, 0));
        // (Each target 200 MB/s of a 100 MB/s NAS: weighs the whole, capped.)
        packable(&c, "slope-hi", 4, Some(1));
        for i in 1..=4 {
            let sizes = [("threads".to_string(), 1), ("bytes".to_string(), 120_000_000_000)];
            let cost = Cost {
                peak_mb: 2000,
                secs: 600,
                worker: None,
                v: cost_version("slope-hi"),
                sizes: sizes.into(),
                ..Default::default()
            };
            c.add_costs_by(&[(cost_key("slope-hi", &format!("6/1/{i}")), cost)], "m1");
        }
        c.set_nas(Some(crate::budget::nas::Limits {
            bytes_s: 100e6,
            creates_s: 40.0,
            downloads: None,
            ..Default::default()
        }));
        let own = c.hold("slope-hi", &[("6/1/4".into(), "k 6/1/4".into())]).expect("the lead's own: the whole");
        let a = Ask { want: 3, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert_eq!(all_granted(&w, &a), [["6/1/1"]], "beside the lead's own alone");
        assert!(all_granted(&w, &a).is_empty(), "beside a member's: waits");
        assert!(c.nas_waits().is_empty(), "not yet half an hour");
        for _ in 0..31 {
            clock.advance(Duration::from_secs(60));
            // (Every lease kept alive.)
            assert!(c.renew(own, None), "the lead's own kept");
            let ids: Vec<u64> = {
                let g = c.shared.lock().unwrap();
                let now = g.now();
                g.leases.all(now).iter().filter(|l| l.worker == "m1").map(|l| l.id).collect()
            };
            assert_eq!(ids.len(), 1);
            for id in ids {
                assert!(w.beat(id, None).unwrap());
            }
            assert!(all_granted(&w, &a).is_empty(), "still waiting");
        }
        let waits = c.nas_waits();
        assert!(waits.iter().any(|(s, t, m)| s == "slope-hi" && t == "6/1/2" && *m >= 30), "{waits:?}");
    }

    /// Task #499: a cold job of the lead's own (bld-fetch, run before but not measured the way it
    /// runs now) goes beside the lead's jobs when its provisional need (its declared memory times
    /// 3/2) fits the room left; one that doesn't, or a step never measured (the catalogue), isn't
    /// starved: the lead's asks get nothing more until it can go. A member with more room, asking
    /// for the step it may never be granted, isn't its asker.
    #[test]
    fn a_cold_job_of_the_leads_own_goes_by_its_provisional_need() {
        let provisional = crate::agent::steps::mem_mb("bld-fetch") * 3 / 2;
        for (step, room, beside) in
            [("bld-fetch", 64_000, true), ("bld-fetch", 2000 + provisional - 1, false), ("catalog", 64_000, false)]
        {
            let (_d, c, w, _clock) = every((0, 0));
            packable(&c, "slope-hi", 4, Some(1));
            ran_before(&c, "bld-fetch");
            let mut offers = c.shared.lock().unwrap().offers.clone();
            offers.insert(
                0,
                Offer {
                    step: step.into(),
                    targets: vec![(step.into(), "k".into(), 0)],
                    job: Some(format!("{step} {step}")),
                },
            );
            c.offer("p", offers);
            // (A member with far more room asking for it: never granted it.)
            assert!(w
                .ask(&Ask { can: vec![step.into()], ..every_ask("m1", step, 1 << 20, 1 << 30) })
                .unwrap()
                .is_none());
            let a = Ask {
                want: 4,
                can: vec!["slope-hi".into(), step.into()],
                ..every_ask("m4", "slope-hi", room, 100_000)
            };
            let held = c.hold("slope-hi", &[("6/1/1".into(), "k 6/1/1".into())]).unwrap();
            let why = |c: &Coordinator| c.refusals().into_values().collect::<Vec<_>>().join("; ");
            let g = c.ask_own(&a, &BTreeSet::new());
            let job = format!("{step} {step}");
            if beside {
                let g = g.expect("beside the slope job");
                assert_eq!(g.job.as_deref(), Some(job.as_str()));
                assert!(!g.more.is_empty(), "more beside the cold start");
                continue;
            }
            assert!(g.is_none(), "drained for {step}");
            assert!(why(&c).contains(&format!("a cold job of {step} waits")), "{}", why(&c));
            c.finish(held, &[]);
            let g = c.ask_own(&a, &BTreeSet::new()).unwrap();
            assert_eq!(g.job.as_deref(), Some(job.as_str()));
        }
    }

    /// A job of the lead's own whose targets' floors the memory guard measured (an input check,
    /// which noted no cost, so says no cores: packed as the share) goes beside the lead's running
    /// jobs when it fits the room left; a floor past the room keeps it off.
    #[test]
    fn a_job_measured_by_its_floors_goes_beside() {
        let own = |unit: &str| Offer {
            step: "inputs".into(),
            targets: vec![(format!("inputs/{unit}"), "k".into(), 0)],
            job: Some(format!("inputs inputs/{unit}")),
        };
        let v = cost_version("inputs");
        for (mb, alone, beside) in [(340, true, true), (90_000, true, false), (340, false, true)] {
            let (_d, c, _w, _clock) = every((0, 0));
            packable(&c, "slope-hi", 4, Some(1));
            c.add_floors(&[(cost_key("inputs", "inputs/sources"), Floor { mb, alone, v })]);
            let mut offers = c.shared.lock().unwrap().offers.clone();
            offers.insert(0, own("sources"));
            c.offer("p", offers);
            let a = Ask {
                want: 4,
                can: vec!["slope-hi".into(), "inputs".into()],
                ..every_ask("m4", "slope-hi", 64_000, 100_000)
            };
            let _held = c.hold("slope-hi", &[("6/1/1".into(), "k 6/1/1".into())]).unwrap();
            let g = c.ask_own(&a, &BTreeSet::new());
            let jobs: Vec<String> = g
                .iter()
                .flat_map(|g| std::iter::once(g.job.clone()).chain(g.more.iter().map(|m| m.job.clone())))
                .flatten()
                .collect();
            let why = c.refusals().into_values().collect::<Vec<_>>().join("; ");
            assert_eq!(jobs.iter().any(|j| j == "inputs inputs/sources"), beside, "{mb} MB {alone}: {jobs:?} {why}");
        }
    }

    /// A failed job of the lead's own (`finish_failed`) is kept from it as a member's failed work is:
    /// not granted again on its next ask.
    #[test]
    fn a_failed_job_of_the_leads_own_is_not_granted_again_at_once() {
        let (_d, c, _w, _clock) = every((0, 0));
        packable(&c, "rail", 1, Some(1));
        let own = Offer {
            step: "rail".into(),
            targets: vec![("6/1/1".into(), "k 6/1/1".into(), 0)],
            job: Some("rail 6/1/1".into()),
        };
        c.offer("p", vec![own]);
        let a = Ask { can: vec!["rail".into()], ..every_ask("m4", "rail", 64_000, 100_000) };
        let g = c.ask_own(&a, &BTreeSet::new()).unwrap();
        c.finish_failed(g.lease, &[]);
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "kept from it an hour");
    }

    /// The NAS budget holds the lead's own jobs whatever its switch says (`nas_own`, probed
    /// always), the members' only while it's on; switched `off`, the lead's own by their own
    /// weights alone.
    #[test]
    fn the_nas_budget_always_holds_the_leads_own() {
        let (_d, c, w, _clock) = every((0, 0));
        heavy_slope(&c);
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (None, Some(limits));
        }
        // A member's two halves of the NAS, the switch `off`: granted past the whole.
        let m = Ask { want: 3, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert_eq!(all_granted(&w, &m).len(), 3, "the members unbudgeted");
        assert_eq!(c.nas_shares().get("m1"), Some(&1500), "its share shown past the whole");
        assert_eq!(c.nas_step_weights().get("slope-hi"), Some(&500), "one slope job's weight");
        // The lead's own: held within the whole by its own jobs alone (task #440: the members'
        // unheld jobs no longer keep it idle).
        assert!(!c.nas_over(), "the lead's ramp counts its own");
        let a = Ask { want: 3, ..every_ask("m4", "slope-hi", 64_000, 100_000) };
        assert_eq!(all_granted_own(&c, &a).len(), 2, "two halves of its own, no more");
    }

    fn all_granted_own(c: &Coordinator, a: &Ask) -> Vec<Vec<String>> {
        let Some(mut g) = c.ask_own(a, &BTreeSet::new()) else { return Vec::new() };
        let more = std::mem::take(&mut g.more);
        std::iter::once(g)
            .chain(more)
            .map(|g| match g.work {
                Granted::Job { targets, .. } => targets.into_iter().map(|t| t.0).collect(),
                Granted::Task { .. } => Vec::new(),
            })
            .collect()
    }

    /// Task #440 (b44): the switch missing, the members' jobs are held to the NAS budget (a member
    /// no longer reaches 248 % of it while the lead waits), and the lead's own then share it.
    #[test]
    fn a_missing_switch_holds_the_members_too() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            crate::budget::nas::read_switch(&store::nasfs::NasRoot::local_for_test(d.path())),
            Some(Some(Default::default())),
            "missing: on"
        );
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(crate::budget::nas::SWITCH), "off\n").unwrap();
        assert_eq!(
            crate::budget::nas::read_switch(&store::nasfs::NasRoot::local_for_test(d.path())),
            Some(None),
            "`off`: off"
        );
        let (_d, c, w, _clock) = every((0, 0));
        heavy_slope(&c);
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        let m = Ask { want: 6, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert_eq!(all_granted(&w, &m).len(), 2, "the member within the whole");
        assert!(c.nas_shares().get("m1").is_some_and(|&s| s <= 1000));
        let a = Ask { want: 3, ..every_ask("m4", "slope-hi", 64_000, 100_000) };
        assert!(all_granted_own(&c, &a).is_empty(), "the NAS full: the lead waits its turn");
    }

    /// Task #440 (b44: the M1's load ~280 from agents' builds outside the pool): a Mac whose load
    /// average keeps its cores busy is granted jobs in half its cores only, and the cores busy
    /// outside the pool are said; one idle is packed as before.
    #[test]
    fn a_mac_busy_outside_the_pool_gets_fewer_jobs() {
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        let ask = |free: u32| Ask {
            want: 4,
            cap: Some(Capacity { at: unix_now(), cores: 8, cores_free: free, ..Default::default() }),
            ..every_ask("m1", "slope-hi", 64_000, 100_000)
        };
        // (Swamped, half its cores at most kept for the load outside the pool (task #684): its
        // first job's two threads and one more job's.)
        assert_eq!(all_granted(&w, &ask(0)).len(), 2, "swamped: the half left only");
        let why = c.refusals().into_values().collect::<Vec<_>>().join("; ");
        assert!(why.contains("busy outside the pool"), "{why}");
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        assert_eq!(all_granted(&w, &ask(8)).len(), 4, "idle: by its cores");
    }

    /// Task #396 follow-up (b38's stall): a pack job whose target took 0 s weighs its job's real
    /// read rate (`nas_read` over `nas_secs`), not its reads over one second, so a member's slope
    /// still goes beside it; and the lead's own planned job weighing the whole goes while a member
    /// runs small jobs (none of them the whole), where before it waited for the pool to hold nothing.
    #[test]
    fn a_halo_job_weighs_its_real_rate_and_the_leads_whole_job_goes() {
        let (_d, c, w, _clock) = every((0, 0));
        heavy_slope(&c);
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        let pack = |c: &Coordinator, sizes: &[(&str, u64)]| {
            let sizes = sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect();
            let cost =
                Cost { peak_mb: 500, secs: 0, worker: None, v: cost_version("pack"), sizes, ..Default::default() };
            c.add_costs_by(&[(cost_key("pack", "6/2/2"), cost)], "m1");
            c.add_disks(&[(cost_key("pack", "6/2/2"), 100)]);
        };
        // (5.75 GB read in 120 s: 48 MB/s, about half the NAS.)
        pack(&c, &[("threads", 1), ("bytes", 6 << 30), ("nas_read", 5_750_000_000), ("nas_secs", 120)]);
        let mut offers = c.shared.lock().unwrap().offers.clone();
        offers.push(Offer { step: "pack".into(), targets: vec![("6/2/2".into(), "k 6/2/2".into(), 0)], job: None });
        c.offer("p", offers);
        let m = Ask { want: 1, can: vec!["pack".into()], ..every_ask("m1", "pack", 64_000, 100_000) };
        assert_eq!(all_granted(&w, &m), [["6/2/2"]]);
        let m = Ask { want: 3, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert_eq!(all_granted(&w, &m).len(), 1, "a slope (half the NAS) beside it, not two");
        // The lead's own planned job weighing the whole, beside a member's small job.
        let (_d, c, w, _clock) = every((0, 0));
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        packable(&c, "slope-hi", 1, Some(1));
        let small = [("threads".to_string(), 1), ("bytes".to_string(), 600_000_000)];
        let cost = |sizes: [(String, u64); 2]| Cost {
            peak_mb: 500,
            secs: 600,
            worker: None,
            v: cost_version("slope-hi"),
            sizes: sizes.into(),
            ..Default::default()
        };
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/1"), cost(small))], "m1");
        assert_eq!(all_granted(&w, &Ask { want: 1, ..every_ask("m1", "slope-hi", 64_000, 100_000) }).len(), 1);
        packable(&c, "rail", 1, Some(1));
        let whole = [("threads".to_string(), 1), ("bytes".to_string(), 600_000_000_000)];
        c.add_costs_by(&[(cost_key("rail", "6/1/1"), Cost { v: cost_version("rail"), ..cost(whole) })], "m4");
        c.offer(
            "p",
            vec![Offer {
                step: "rail".into(),
                targets: vec![("6/1/1".into(), "k 6/1/1".into(), 0)],
                job: Some("rail 6/1/1".into()),
            }],
        );
        let a = Ask { can: vec!["rail".into()], ..every_ask("m4", "rail", 64_000, 100_000) };
        assert!(c.ask_own(&a, &BTreeSet::new()).is_some(), "the whole one goes beside the member's small job");
    }

    /// The lead's own planned jobs aging past the NAS budget (task #396 follow-up): two of 800 of
    /// 1000 each, held back beside a member's small job, are noted waiting; once they've waited
    /// `OWN_AGE` one goes, its wait forgotten as it's granted, the other still held (the lead's own
    /// leave it no room: aged ones don't pile up); once the first ends, the second goes.
    #[test]
    fn the_leads_aged_own_jobs_go_one_at_a_time() {
        let (_d, c, w, clock) = every((0, 0));
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        let cost = |bytes: u64| Cost {
            peak_mb: 500,
            secs: 600,
            worker: None,
            v: cost_version("slope-hi"),
            sizes: [("threads".to_string(), 1), ("bytes".to_string(), bytes)].into(),
            ..Default::default()
        };
        packable(&c, "slope-hi", 1, Some(1));
        // (30 MB/s: 300 of the NAS, short of the whole.)
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/1"), cost(18_000_000_000))], "m1");
        assert_eq!(all_granted(&w, &Ask { want: 1, ..every_ask("m1", "slope-hi", 64_000, 100_000) }).len(), 1);
        packable(&c, "rail", 2, Some(1));
        for t in ["6/1/1", "6/1/2"] {
            // (80 MB/s: 800 of the NAS.)
            c.add_costs_by(&[(cost_key("rail", t), Cost { v: cost_version("rail"), ..cost(48_000_000_000) })], "m4");
        }
        let own = |t: &str| Offer {
            step: "rail".into(),
            targets: vec![(t.into(), format!("k {t}"), 0)],
            job: Some(format!("rail {t}")),
        };
        c.offer("p", vec![own("6/1/1"), own("6/1/2")]);
        let a = Ask { want: 2, can: vec!["rail".into()], ..every_ask("m4", "rail", 64_000, 100_000) };
        let waits = |c: &Coordinator| -> Vec<String> {
            let s = c.shared.lock().unwrap();
            let g = s.nas_waits.lock().unwrap();
            g.keys().map(|k| k.1.clone()).collect()
        };
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "beside the member's job, past the whole");
        assert_eq!(waits(&c), ["6/1/1", "6/1/2"], "both noted waiting");
        clock.advance(crate::budget::nas::OWN_AGE);
        let g = c.ask_own(&a, &BTreeSet::new()).expect("aged: one goes");
        assert!(g.more.is_empty(), "one at a time: {g:?}");
        assert_eq!(waits(&c), ["6/1/2"], "the granted one's wait forgotten");
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "the other, aged too, held: no pile-up");
        c.finish(g.lease, &[]);
        assert!(c.ask_own(&a, &BTreeSet::new()).is_some(), "the first ended: the second goes");
    }

    /// Task #552: placement's draw weighs a worker by its measured speed at the target's step
    /// (else over the shared steps, else the lead's), `CACHE_PREFER` times where its Mac holds the
    /// step's shared cache whole by its ask (its fill measured); short of it, or a step reading
    /// none, no more.
    #[test]
    fn the_draw_weighs_the_step_speed_and_the_cache_held() {
        use crate::agent::placement::{Placement, CACHE_PREFER};
        let cache = crate::localread::CACHE;
        let mut s = sched::Sched::default();
        s.fills.insert(cache.to_string(), 20_000);
        let p = Placement {
            speed: [("m1".to_string(), 0.5)].into(),
            step_speed: [("mini".to_string(), [("depth".to_string(), 3.0)].into())].into(),
            ..Default::default()
        };
        let holding = |mb: u64| Ask {
            cap: Some(Capacity {
                caches: [(cache.to_string(), CacheHere { have_mb: mb })].into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(s.draw_weight(&p, "depth", "mini", &holding(0)), 3.0, "its speed at the step");
        assert_eq!(s.draw_weight(&p, "slope-hi", "mini", &holding(0)), 1.0, "unmeasured: the lead's");
        assert_eq!(s.draw_weight(&p, "depth", "m1", &holding(20_000)), 0.5 * CACHE_PREFER, "holds it whole");
        assert_eq!(s.draw_weight(&p, "depth", "m1", &holding(19_000)), 0.5, "short of it");
        assert_eq!(s.draw_weight(&p, "slope-hi", "m1", &holding(20_000)), 0.5, "a step reading none");
        s.fills.clear();
        assert_eq!(s.draw_weight(&p, "depth", "m1", &holding(20_000)), 0.5, "no fill measured");
    }

    /// Task #552 review: a draw's winner is a member that would take the target now. One whose
    /// last ask wanted a job and was granted it since (its slots full) isn't drawn: the other
    /// takes every target, the z6 tiles of one z3 tile among them, so none sits untaken. A member
    /// passed by for one target takes the next drawn to it, down the offer.
    #[test]
    fn placement_draws_only_among_those_who_would_take_it_now() {
        let (_d, c, w1, _clock) = every((0, 0));
        let w2 = client::Client::at(w1.urls(), c.contact.token.clone(), "m2");
        // (Clustered: 64 z6 tiles, one z3 tile.)
        let ts: Vec<String> = (0..64).map(|i| format!("6/{}/{}", i % 8, i / 8)).collect();
        let cost = Cost { peak_mb: 500, secs: 900, worker: None, v: cost_version("pack"), ..Default::default() };
        c.add_costs_by(&ts.iter().map(|t| (cost_key("pack", t), cost.clone())).collect::<Vec<_>>(), "m1");
        c.add_disks(&ts.iter().map(|t| (cost_key("pack", t), 10)).collect::<Vec<_>>());
        let (a1, a2) =
            (every_ask("m1", "pack", 4096, 10_000), Ask { want: 1, ..every_ask("m2", "pack", 4096, 10_000) });
        assert!(w1.ask(&a1).unwrap().is_none() && w2.ask(&a2).unwrap().is_none());
        c.set_placement(Some(placement(&[], &[("m1", 1.0), ("m2", 1.0)])));
        let all =
            Offer { step: "pack".into(), targets: ts.iter().map(|t| (t.clone(), "k".into(), 0)).collect(), job: None };
        let one = |t: &str| Offer { step: "pack".into(), targets: vec![(t.to_string(), "k".into(), 0)], job: None };
        let drawn_to_m1 = |c: &Coordinator| {
            let s = c.shared.lock().unwrap();
            let now = s.now();
            ts.iter().filter(|t| !s.pick_every(&one(t), &a1, now).is_empty()).count()
        };
        // Both free: about half each, and m1 passed by for one goes on to the next drawn to it.
        let n = drawn_to_m1(&c);
        assert!(n > 16 && n < 48, "{n}");
        {
            let s = c.shared.lock().unwrap();
            assert!(!s.pick_every(&all, &a1, s.now()).is_empty(), "down the offer to one drawn to it");
        }
        // m2 granted the one job its ask wanted: full, it's drawn nothing; m1 takes every one.
        c.offer("p", vec![all.clone()]);
        assert!(w2.ask(&a2).unwrap().is_some());
        assert_eq!(drawn_to_m1(&c), ts.len() - 1);
    }

    /// Task #560: a member's room is its ask's free memory less the jobs granted it since (a
    /// second slot's job can't fit where the first's took the room), and a member passed over
    /// (`ASKING` since its ask) is drawn again once it asks (`Sched::asking`, the forecast's).
    #[test]
    fn placement_counts_the_memory_granted_since_the_ask_and_asking_again_draws() {
        let (_d, c, w1, clock) = every((0, 0));
        let w2 = client::Client::at(w1.urls(), c.contact.token.clone(), "m2");
        let cost = Cost { peak_mb: 600, secs: 900, worker: None, v: cost_version("pack"), ..Default::default() };
        c.add_costs_by(&[(cost_key("pack", "6/1/1"), cost.clone()), (cost_key("pack", "6/1/2"), cost)], "m1");
        let mut a2 = every_ask("m2", "pack", 4096, 10_000);
        a2.want = 2;
        a2.cap = Some(Capacity { limit_mb: 8000, mem_mb: 1000, ..a2.cap.clone().unwrap() });
        assert!(w2.ask(&a2).unwrap().is_none());
        let mut s = c.shared.lock().unwrap();
        let (now, asked) = (s.now(), s.workers["m2"].asked.unwrap());
        let m = s.measured("pack", "6/1/2").unwrap();
        assert!(s.would_take_now("m2", asked, &a2, &m, now), "1000 MB free, 600 needed");
        s.leases.grant_at(
            "m2",
            Work::Job { step: "pack".into(), targets: vec![("6/1/1".into(), "k".into())] },
            now,
            unix_now(),
        );
        assert!(!s.would_take_now("m2", asked, &a2, &m, now), "a slot left, but 400 MB");
        // Not asking for ASKING: out of the draws; asking again: back in.
        clock.advance(ASKING);
        let now = s.now();
        assert!(now.duration_since(s.workers["m2"].asked.unwrap()) >= ASKING);
        // A member with no slot free doesn't ask: it keeps its old ask and stays aged out.
        s.asking([("m2", 0, 1000)], now);
        assert!(now.duration_since(s.workers["m2"].asked.unwrap()) >= ASKING);
        s.asking([("m2", 2, 1000)], now);
        // (Asking again: the job granted before, 600 MB, is running: 400 free of the 1000.)
        let x = s.workers["m2"].ask.clone().unwrap();
        assert_eq!((x.want, x.cap.as_ref().unwrap().mem_mb), (2, 400));
        let asked = s.workers["m2"].asked.unwrap();
        assert!(!s.would_take_now("m2", asked, &x, &Measured { mem_mb: 500, ..m }, now), "500 > 400");
        assert!(s.would_take_now("m2", asked, &x, &Measured { mem_mb: 300, ..m }, now), "300 fits");
        // A job of a target never measured counts its estimate (the step's table or prior), not 0.
        let id = s.leases.grant_at(
            "m2",
            Work::Job { step: "pack".into(), targets: vec![("6/9/9".into(), "k".into())] },
            now,
            unix_now(),
        );
        let l = s.leases.of(id).unwrap().clone();
        assert_eq!(s.lease_mem(&l), s.book().mem("pack", "6/9/9", 0, 0).0);
        assert!(now.duration_since(s.workers["m2"].asked.unwrap()) < ASKING);
    }

    /// Task #552 (22 Oct, the lead: 52 units aged an hour, every other step "waiting for room in
    /// the NAS budget", the lead idle while two members ran units back to back): a step granted
    /// target by target doesn't keep the aged reservation for itself, each grant handing it to its
    /// next target. Its other waits restart with each of its grants, so the next oldest, the lead's
    /// own job, is held room for: a member's next unit waits, and the lead's job goes.
    #[test]
    fn a_step_granted_one_by_one_doesnt_keep_the_reservation() {
        let (_d, c, w, clock) = every((0, 0));
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        let cost = |step: &str, bytes: u64| Cost {
            peak_mb: 500,
            secs: 600,
            worker: None,
            v: cost_version(step),
            sizes: [("threads".to_string(), 1), ("bytes".to_string(), bytes)].into(),
            ..Default::default()
        };
        packable(&c, "slope-hi", 6, Some(1));
        for i in 1..=6 {
            // (40 MB/s: 400 of the NAS each.)
            c.add_costs_by(&[(cost_key("slope-hi", &format!("6/1/{i}")), cost("slope-hi", 24_000_000_000))], "m1");
        }
        let mut offers = c.shared.lock().unwrap().offers.clone();
        packable(&c, "rail", 1, Some(1));
        // (30 MB/s: 300.)
        c.add_costs_by(&[(cost_key("rail", "6/1/1"), cost("rail", 18_000_000_000))], "m4");
        offers.push(Offer {
            step: "rail".into(),
            targets: vec![("6/1/1".into(), "k 6/1/1".into(), 0)],
            job: Some("rail 6/1/1".into()),
        });
        c.offer("p", offers);
        let w2 = client::Client::at(w.urls(), c.contact.token.clone(), "m2");
        let by = |n: &str| if n == "m1" { &w } else { &w2 };
        let m = |n: &str| Ask { want: 1, ..every_ask(n, "slope-hi", 64_000, 100_000) };
        let one = |n: &str| all_granted(by(n), &m(n)).into_iter().flatten().collect::<Vec<_>>();
        let lease_of = |n: &str| {
            let s = c.shared.lock().unwrap();
            s.leases.all(s.now()).iter().filter(|l| l.worker == n).map(|l| l.id).max().unwrap()
        };
        assert_eq!(one("m1"), ["6/1/1"]);
        assert_eq!(one("m2"), ["6/1/2"]);
        // (800 leased: the slope's next held back, then the lead's rail.)
        assert!(one("m1").is_empty());
        clock.advance(Duration::from_secs(1));
        let lead = Ask { want: 1, can: vec!["rail".into()], ..every_ask("m4", "rail", 64_000, 100_000) };
        assert!(c.ask_own(&lead, &BTreeSet::new()).is_none(), "800 + 300: past the whole");
        // (Aged, the leases kept alive, the waits passed over again lately.)
        for _ in 0..20 {
            clock.advance(crate::budget::nas::OWN_AGE / 20);
            for n in ["m1", "m2"] {
                assert!(by(n).beat(lease_of(n), None).unwrap());
            }
        }
        assert!(one("m1").is_empty());
        assert!(c.ask_own(&lead, &BTreeSet::new()).is_none());
        // m1's ends: the slope's oldest wait goes (400 + 400).
        w.give_back(lease_of("m1"), "done").unwrap();
        assert_eq!(one("m1").len(), 1, "the oldest wait goes");
        // m2's ends: the slope just took room, so the rail's wait is the oldest now: room held for
        // it, m2's next slope waits (400 + 400 + 300 past the whole), and the lead's rail goes.
        w2.give_back(lease_of("m2"), "done").unwrap();
        assert!(one("m2").is_empty(), "room held for the lead's aged rail");
        assert!(c.ask_own(&lead, &BTreeSet::new()).is_some(), "the lead's aged rail goes");
    }

    /// Task #603 (23 Oct, the lead: pack 6/20/23 weighing 946 aged beside the M4's terrain of 103,
    /// 6,427 light targets held back 41-62 min, the pack never granted): the aged heavy target holds
    /// room and is granted beside the member's light job (one aged job past the whole), to the
    /// lead's ask and a member's alike; the light ones wait while it's past the whole. (A unit
    /// target stands in for the pack: a halo step weighs by what it wrote.)
    #[test]
    fn an_aged_heavy_target_beside_a_light_job_goes() {
        for asker in ["m1", "m4"] {
            let (_d, c, w, clock) = every((0, 0));
            let limits =
                crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
            {
                let mut s = c.shared.lock().unwrap();
                (s.nas, s.nas_own) = (Some(limits), Some(limits));
            }
            let cost = |step: &str, bytes: u64| Cost {
                peak_mb: 500,
                secs: 600,
                worker: None,
                v: cost_version(step),
                sizes: [("threads".to_string(), 1), ("bytes".to_string(), bytes)].into(),
                ..Default::default()
            };
            packable(&c, "terrain-hi", 1, Some(1));
            // (10.28 MB/s: 103 of the NAS.)
            c.add_costs_by(&[(cost_key("terrain-hi", "6/1/1"), cost("terrain-hi", 6_170_000_000))], "m1");
            let mut offers = c.shared.lock().unwrap().offers.clone();
            packable(&c, "unit", 1, Some(1));
            // (94.55 MB/s: 946.)
            c.add_costs_by(&[(cost_key("unit", "6/1/1"), cost("unit", 56_730_000_000))], "m1");
            offers.extend(c.shared.lock().unwrap().offers.clone());
            packable(&c, "slope-hi", 40, Some(1));
            for i in 1..=40 {
                // (5 MB/s: 50 each.)
                c.add_costs_by(&[(cost_key("slope-hi", &format!("6/1/{i}")), cost("slope-hi", 3_000_000_000))], "m1");
            }
            offers.extend(c.shared.lock().unwrap().offers.clone());
            c.offer("p", offers);
            let w2 = client::Client::at(w.urls(), c.contact.token.clone(), "m2");
            let by = client::Client::at(w.urls(), c.contact.token.clone(), asker);
            let ask = |n: &str, step: &str| Ask { want: 1, ..every_ask(n, step, 64_000, 100_000) };
            assert_eq!(granted(&w2, &ask("m2", "terrain-hi")), ["6/1/1"]);
            let terrain = {
                let s = c.shared.lock().unwrap();
                s.leases.all(s.now()).iter().map(|l| l.id).max().unwrap()
            };
            assert!(granted(&by, &ask(asker, "unit")).is_empty(), "{asker}: 103 + 946, past the whole");
            for _ in 0..19 {
                clock.advance(crate::budget::nas::OWN_AGE / 20);
                assert!(w2.beat(terrain, None).unwrap());
                assert!(granted(&by, &ask(asker, "unit")).is_empty(), "{asker}: not aged yet");
            }
            clock.advance(crate::budget::nas::OWN_AGE / 20);
            assert!(granted(&w2, &ask("m2", "slope-hi")).is_empty(), "{asker}: room held for the aged unit");
            assert_eq!(granted(&by, &ask(asker, "unit")), ["6/1/1"], "{asker}: the aged unit goes beside the terrain");
            assert!(granted(&w2, &ask("m2", "slope-hi")).is_empty(), "{asker}: 1049 leased, past the whole");
        }
    }

    /// Task #451 (20 Oct: GC beside two depth-lo jobs, the NAS at 34 % IO wait, all three stalled):
    /// the clean-up weighs its metadata work (folder entries listed, files statted and deleted over
    /// the NAS's probed rates for them), starts only on a quiet NAS (unmeasured, alone; measured,
    /// within the whole, never aged past it, its wait never noted), and yields to a job held back
    /// `OWN_AGE` that it alone keeps out.
    #[test]
    fn the_clean_up_waits_for_a_quiet_nas_and_yields_to_a_held_job() {
        let (_d, c, w, clock) = every((0, 0));
        let limits = crate::budget::nas::Limits {
            bytes_s: 100e6,
            creates_s: 40.0,
            listed_s: 1000.0,
            stats_s: 500.0,
            deletes_s: 40.0,
            downloads: None,
            held: false,
        };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        let cost = |step: &str, sizes: &[(&str, u64)]| Cost {
            peak_mb: 500,
            secs: 600,
            worker: None,
            v: cost_version(step),
            sizes: sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..Default::default()
        };
        packable(&c, "slope-hi", 3, Some(1));
        // (6/1/1: 30 MB/s, 300 of the NAS; 6/1/2: 50 MB/s, 500.)
        let slope = |b: u64| cost("slope-hi", &[("threads", 1), ("bytes", b)]);
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/1"), slope(18_000_000_000))], "m1");
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/2"), slope(30_000_000_000))], "m1");
        let mut offers = c.shared.lock().unwrap().offers.clone();
        offers.push(Offer {
            step: "gc".into(),
            targets: vec![("gc".into(), String::new(), 0)],
            job: Some("gc".into()),
        });
        c.offer("p", offers);
        let m = Ask { want: 1, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        let g1 = w.ask(&m).unwrap().expect("the member's 300");
        // (Time passes, the jobs' leases kept alive: the member's, and the clean-up's once held.)
        let own: std::cell::Cell<Option<u64>> = Default::default();
        let members = std::cell::RefCell::new(vec![g1.lease]);
        let age = |by: Duration| {
            for _ in 0..20 {
                clock.advance(by / 20);
                for &l in members.borrow().iter() {
                    assert!(w.beat(l, None).unwrap());
                }
                if let Some(n) = own.get() {
                    assert!(c.renew(n, None));
                }
            }
        };
        let a = Ask { want: 1, can: vec!["gc".into()], ..every_ask("m4", "gc", 64_000, 100_000) };
        let waits = |c: &Coordinator| -> Vec<String> {
            let s = c.shared.lock().unwrap();
            let g = s.nas_waits.lock().unwrap();
            g.keys().map(|k| k.0.clone()).collect()
        };
        // Unmeasured: alone only, its wait not noted, whatever the time.
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "cold: beside the member's job, held");
        age(crate::budget::nas::OWN_AGE);
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "never aged past it");
        assert!(waits(&c).is_empty(), "its wait never noted: it holds no room");
        // Measured: 200 entries listed and 8 files deleted a second (200 + 200 of the NAS), its
        // bytes next to nothing: 400 beside the member's 300.
        let gc = cost("gc", &[("threads", 1), ("bytes", 0), ("listed", 120_000), ("deletes", 4_800)]);
        c.add_costs_by(&[(cost_key("gc", "gc"), gc)], "m4");
        c.add_disks(&[(cost_key("gc", "gc"), 0)]);
        let g = c.ask_own(&a, &BTreeSet::new()).expect("within the whole beside the member's job");
        own.set(Some(g.lease));
        assert_eq!(c.nas_shares().get("m4"), Some(&400), "its metadata work weighed");
        assert_eq!(c.nas_yield("gc", &["gc".into()]), None, "nothing held back for it");
        // A member's 500 doesn't fit beside it (300 + 400); once held back `OWN_AGE`, the clean-up
        // yields (without it, 300 + 500 fit).
        let m2 = Ask { want: 1, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert!(all_granted(&w, &m2).is_empty(), "the member's 500 held back");
        assert_eq!(c.nas_yield("gc", &["gc".into()]), None, "not held long yet");
        age(crate::budget::nas::OWN_AGE);
        assert!(all_granted(&w, &m2).is_empty());
        let why = c.nas_yield("gc", &["gc".into()]).expect("yields to the held job");
        assert!(why.contains("slope-hi 6/1/2"), "{why}");
        c.finish(g.lease, &[]);
        own.set(None);
        let g2 = w.ask(&m2).unwrap().expect("the clean-up gone: it goes");
        assert!(matches!(&g2.work, Granted::Job { targets, .. } if targets[0].0 == "6/1/2"));
        members.borrow_mut().push(g2.lease);
        // Beside 800 it waits, however long.
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none());
        age(crate::budget::nas::OWN_AGE * 3);
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "never urgent");
    }

    /// Task #451 review: an unmeasured clean-up doesn't start beside a cold job (weight 0, its NAS
    /// load unknown), and one held back `QUIET_AGE` by a pool that stays busy goes as an aged job.
    #[test]
    fn the_clean_up_waits_for_no_cold_job_and_ages_in_after_a_day() {
        let (_d, c, w, clock) = every((0, 0));
        let limits = crate::budget::nas::Limits {
            bytes_s: 100e6,
            creates_s: 40.0,
            listed_s: 1000.0,
            stats_s: 500.0,
            deletes_s: 40.0,
            downloads: None,
            held: false,
        };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        // (A slope job measured for memory but not its NAS: cold for the budget, weight 0.)
        packable(&c, "slope-hi", 1, Some(1));
        let mut offers = c.shared.lock().unwrap().offers.clone();
        offers.push(Offer {
            step: "gc".into(),
            targets: vec![("gc".into(), String::new(), 0)],
            job: Some("gc".into()),
        });
        c.offer("p", offers);
        let g1 = w.ask(&Ask { want: 1, ..every_ask("m1", "slope-hi", 64_000, 100_000) }).unwrap().expect("slope-hi");
        assert_eq!(c.nas_shares().get("m1"), Some(&0));
        let a = Ask { want: 1, can: vec!["gc".into()], ..every_ask("m4", "gc", 64_000, 100_000) };
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "unmeasured beside a cold job: held");
        w.give_back(g1.lease, "test").unwrap();
        assert!(c.ask_own(&a, &BTreeSet::new()).is_some(), "alone: goes");

        // Measured at 400, beside a member's 700 that never ends: held, until a day has passed.
        let (_d, c, w, clock2) = every((0, 0));
        drop(clock);
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        packable(&c, "slope-hi", 1, Some(1));
        let cost = |sizes: &[(&str, u64)]| Cost {
            peak_mb: 500,
            secs: 600,
            worker: None,
            v: 0,
            sizes: sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..Default::default()
        };
        let slope = Cost { v: cost_version("slope-hi"), ..cost(&[("threads", 1), ("bytes", 42_000_000_000)]) };
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/1"), slope)], "m1");
        c.add_costs_by(
            &[(cost_key("gc", "gc"), cost(&[("threads", 1), ("bytes", 0), ("listed", 120_000), ("deletes", 4_800)]))],
            "m4",
        );
        c.add_disks(&[(cost_key("gc", "gc"), 0)]);
        let mut offers = c.shared.lock().unwrap().offers.clone();
        offers.push(Offer {
            step: "gc".into(),
            targets: vec![("gc".into(), String::new(), 0)],
            job: Some("gc".into()),
        });
        c.offer("p", offers);
        let g1 = w.ask(&Ask { want: 1, ..every_ask("m1", "slope-hi", 64_000, 100_000) }).unwrap().expect("slope-hi");
        assert_eq!(c.nas_shares().get("m1"), Some(&700));
        let age = |by: Duration| {
            for _ in 0..200 {
                clock2.advance(by / 200);
                assert!(w.beat(g1.lease, None).unwrap());
            }
        };
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "700 + 400: held");
        age(crate::budget::nas::QUIET_AGE / 2);
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "half a day: still held");
        age(crate::budget::nas::QUIET_AGE / 2);
        assert!(c.ask_own(&a, &BTreeSet::new()).is_some(), "a day held back: it goes as an aged job");
    }

    /// A whole-weight job of the lead's own goes beside a running job of the lead's own (and a
    /// member's small one), not held back until the lead holds nothing: the room the lead's own
    /// leave is asked of aged jobs alone.
    #[test]
    fn the_leads_whole_job_goes_beside_its_own_running_job() {
        let (_d, c, w, _clock) = every((0, 0));
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
        }
        let cost = |bytes: u64| Cost {
            peak_mb: 500,
            secs: 600,
            worker: None,
            v: cost_version("slope-hi"),
            sizes: [("threads".to_string(), 1), ("bytes".to_string(), bytes)].into(),
            ..Default::default()
        };
        packable(&c, "slope-hi", 1, Some(1));
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/1"), cost(6_000_000_000))], "m1");
        assert_eq!(all_granted(&w, &Ask { want: 1, ..every_ask("m1", "slope-hi", 64_000, 100_000) }).len(), 1);
        // (A rail job of 30 MB/s: 300 of the NAS; a peaks job the whole.)
        for (step, b) in [("rail", 18_000_000_000), ("peaks", 600_000_000_000)] {
            packable(&c, step, 1, Some(1));
            c.add_costs_by(&[(cost_key(step, "6/1/1"), Cost { v: cost_version(step), ..cost(b) })], "m4");
        }
        let own = |step: &str| Offer {
            step: step.into(),
            targets: vec![("6/1/1".into(), "k 6/1/1".into(), 0)],
            job: Some(format!("{step} 6/1/1")),
        };
        let a = |step: &str| Ask { want: 1, can: vec![step.into()], ..every_ask("m4", step, 64_000, 100_000) };
        c.offer("p", vec![own("rail")]);
        assert!(c.ask_own(&a("rail"), &BTreeSet::new()).is_some(), "the lead's 300 beside the member's 100");
        c.offer("p", vec![own("rail"), own("peaks")]);
        let g = c.ask_own(&a("peaks"), &BTreeSet::new()).expect("the whole one beside the lead's own running");
        assert!(matches!(&g.work, Granted::Job { step, .. } if step == "peaks"), "{g:?}");
    }

    /// Six slope targets each taking half a 100 MB/s NAS (50 MB/s), a thread each.
    fn heavy_slope(c: &Coordinator) {
        packable(c, "slope-hi", 6, Some(1));
        for i in 1..=6 {
            let sizes = [
                ("threads".to_string(), 1),
                ("bytes".to_string(), 18_000_000_000),
                ("wrote".to_string(), 12_000_000_000),
            ];
            let cost = Cost {
                peak_mb: 2000,
                secs: 600,
                worker: None,
                v: cost_version("slope-hi"),
                sizes: sizes.into(),
                ..Default::default()
            };
            c.add_costs_by(&[(cost_key("slope-hi", &format!("6/1/{i}")), cost)], "m1");
        }
    }

    /// The lead's own planned job (`pick_own`) is held by a cancel's hold as a member's work is:
    /// none granted while its target waits for a newer app.
    #[test]
    fn a_cancels_hold_keeps_the_leads_own_job_back() {
        let (_d, c, _w, _clock) = every((0, 0));
        packable(&c, "rail", 1, Some(1));
        let own = Offer {
            step: "rail".into(),
            targets: vec![("6/1/1".into(), "k 6/1/1".into(), 0)],
            job: Some("rail 6/1/1".into()),
        };
        c.offer("p", vec![own]);
        let a = Ask { can: vec!["rail".into()], ..every_ask("m4", "rail", 64_000, 100_000) };
        c.shared.lock().unwrap().leases.app = "20261005-1508-84142d3".into();
        c.shared.lock().unwrap().leases.hold_for_update("rail", "6/1/1", "scenic job cancel on m4", 1);
        assert!(c.ask_own(&a, &BTreeSet::new()).is_none(), "held for a newer app");
        // The lead on a newer app: released.
        c.shared.lock().unwrap().leases.app = "20261005-1613-d05125b".into();
        assert!(c.ask_own(&a, &BTreeSet::new()).is_some());
    }

    /// Packing (docs/pool.md §7.2): an ask gets as many jobs as it wants that fit its room together
    /// (memory, disk, cores), each its own lease, in plan order; its next ask what fits the room
    /// left; one wanted (or an older agent's ask, none), one.
    #[test]
    fn packing_grants_as_many_jobs_as_fit_the_asks_room() {
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        let a = Ask { want: 5, ..every_ask("m1", "slope-hi", 7000, 100_000) };
        assert_eq!(all_granted(&w, &a), [["6/1/1"], ["6/1/2"], ["6/1/3"]], "three fit its memory");
        assert!(all_granted(&w, &a).is_empty(), "its room taken");
        // By cores: 2 of 8 each, four at once whatever the memory.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        let a = Ask { want: 6, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert_eq!(all_granted(&w, &a).len(), 4);
        // One wanted, or none said: one.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        assert_eq!(all_granted(&w, &Ask { want: 1, ..a.clone() }).len(), 1);
        assert_eq!(all_granted(&w, &Ask { want: 0, ..a.clone() }).len(), 1);
        // Cores not said (no run said its threads): a thread each (task #896), all six.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, None);
        assert_eq!(all_granted(&w, &a).len(), 6);
    }

    /// A mixed-version pool (task #247): an older app's ask (no capacity, one slot) is granted one
    /// job; while it's around, every ask is granted one, the refusal saying why; once it asks with
    /// a capacity (the new app), packing is on again. The old field for the limit stands in for a
    /// capacity's.
    #[test]
    fn a_mixed_version_pool_grants_one_job_each_until_every_member_runs_the_new_app() {
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 8, Some(2));
        // (The lead's own asks (m4, in process), on the new app; the member m1 over HTTP.)
        let own = |c: &Coordinator, want: usize| -> usize {
            let a = Ask { want, ..every_ask("m4", "slope-hi", 64_000, 100_000) };
            c.ask_own(&a, &BTreeSet::new()).map_or(0, |g| 1 + g.more.len())
        };
        assert!(c.all_new(), "nobody old around");
        // An older app's ask: no capacity, no want; one job, and it works.
        let old = Ask { cap: None, want: 0, limit_mb: Some(12 << 10), ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert_eq!(all_granted(&w, &old).len(), 1, "an older app's ask: one job");
        assert!(!c.all_new(), "an older app around");
        assert_eq!(c.largest_limit(), Some(12 << 10), "its old field's limit read");
        // Even wanting more, an ask without a capacity is granted one.
        assert_eq!(all_granted(&w, &Ask { want: 3, ..old.clone() }).len(), 1);
        // The lead's own, while the old member is around: one, and why.
        assert_eq!(own(&c, 3), 1, "packing off while a member is on an older app");
        let why = c.refusals().get("m4").cloned().unwrap_or_default();
        assert!(why.contains("m1 runs an older app"), "{why}");
        assert!(!c.all_new(), "the lead's own never counts as old");
        // The member updated: its asks carry a capacity; packing on again.
        let updated = Ask {
            cap: Some(Capacity { at: unix_now(), cores: 8, cores_free: 8, ..Default::default() }),
            ..old.clone()
        };
        assert_eq!(all_granted(&w, &Ask { want: 2, ..updated.clone() }).len(), 2, "packing on for the member");
        assert!(c.all_new(), "every member on the new app");
        assert_eq!(own(&c, 2), 2, "packing on for the lead");
        // A capacity's limit read over the old field's.
        let capped = Ask {
            cap: Some(Capacity { at: unix_now(), cores: 8, cores_free: 8, limit_mb: 20 << 10, ..Default::default() }),
            limit_mb: None,
            ..updated
        };
        let _ = all_granted(&w, &capped);
        assert_eq!(c.largest_limit(), Some(20 << 10));
    }

    /// Threads granted (task #241): each job its measured cores, a serial one 1, one its threads
    /// held the cores free; a NAS-reading step its measured threads too (task #896: no share floors
    /// or caps a job). The cores an ask's jobs have together leave out the in-use share's while its
    /// owner uses its Mac.
    #[test]
    fn grants_carry_their_threads() {
        let threads = |w: &client::Client, a: &Ask| -> Vec<u32> {
            let Some(mut g) = w.ask(a).unwrap() else { return Vec::new() };
            let more = std::mem::take(&mut g.more);
            std::iter::once(g).chain(more).map(|g| g.threads).collect()
        };
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        let a = every_ask("m1", "slope-hi", 64_000, 100_000);
        assert_eq!(threads(&w, &a), [2], "its measured 2");
        let cap = Capacity { cores: 8, in_use: true, kept_cores: 2, ..Default::default() };
        let used = Ask { cap: Some(cap), ..a.clone() };
        assert_eq!(ask_cores(&used), 6, "the owner at it");
        // A serial step's 1 each.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(1));
        let a = Ask { want: 6, ..every_ask("m1", "slope-hi", 64_000, 100_000) };
        assert_eq!(threads(&w, &a), [1; 6]);
        // (Task #888, #896) a busy one (2.9 of its 3 threads: 3 to the nearest whole, its threads
        // held it) climbs a thread.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 1, Some(3));
        let busy = [("threads".to_string(), 3), ("cpu_ms".to_string(), 1_740_000)];
        let cost = Cost {
            peak_mb: 2000,
            secs: 600,
            worker: None,
            v: cost_version("slope-hi"),
            sizes: busy.into(),
            ..Default::default()
        };
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/1"), cost)], "m1");
        let one = every_ask("m1", "slope-hi", 64_000, 100_000);
        assert_eq!(threads(&w, &one), [4], "its measured demand, not a share");
        // A step reading the NAS (30 MB/s), measured at 1: its 1, no share floors it.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(1));
        for i in 1..=6 {
            let sizes = [("threads".to_string(), 1), ("bytes".to_string(), 18_000_000_000)];
            let cost = Cost {
                peak_mb: 2000,
                secs: 600,
                worker: None,
                v: cost_version("slope-hi"),
                sizes: sizes.into(),
                ..Default::default()
            };
            c.add_costs_by(&[(cost_key("slope-hi", &format!("6/1/{i}")), cost)], "m1");
        }
        assert_eq!(threads(&w, &a), [1; 6], "its measured 1 each");
    }

    /// A member holding jobs asks again as the agent does (the same share of its Mac each time, one
    /// job at a time): the jobs it holds are taken off it, so together they never
    /// pass its memory, nor its disk.
    #[test]
    fn packing_counts_the_jobs_a_member_holds_against_its_ask() {
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(1));
        let a = every_ask("m1", "slope-hi", 5000, 100_000);
        assert_eq!(all_granted(&w, &a).len(), 1);
        assert_eq!(all_granted(&w, &a).len(), 1);
        assert!(all_granted(&w, &a).is_empty(), "a third would pass its memory");
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(1));
        let a = every_ask("m1", "slope-hi", 64_000, 250);
        assert_eq!(all_granted(&w, &a).len(), 1);
        assert_eq!(all_granted(&w, &a).len(), 1);
        assert!(all_granted(&w, &a).is_empty(), "a third would pass its disk");
    }

    /// A member's job only waiting on a worker (task #462, `Capacity::waiting`): packed at the
    /// memory it holds now (never above its measure) and none of its cores, so a job goes beside
    /// it in its slot's room; only its own Mac's lease counts so (another member naming it frees
    /// nothing); its lease lapsed, it counts for nothing as ever. Two members, a virtual clock.
    #[test]
    fn a_members_job_waiting_on_a_worker_gives_its_room() {
        let (_d, c, w, clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(1));
        let ask = |who: &str, mem: u64, want: usize, leases: Vec<u64>, waiting: Vec<WaitingJob>| Ask {
            want,
            cap: Some(Capacity {
                at: unix_now(),
                cores: 8,
                cores_free: 8,
                disk_mb: 64_000,
                leases,
                waiting,
                ..Default::default()
            }),
            ..every_ask(who, "slope-hi", mem, 64_000)
        };
        let wait = |lease: u64, held_mb: u64| vec![WaitingJob { lease, held_mb }];
        // Memory: m1 spares 3000 MB, its job measured 2000: nothing beside it.
        let g1 = w.ask(&ask("m1", 3000, 1, vec![], vec![])).unwrap().unwrap();
        assert!(w.ask(&ask("m1", 3000, 1, vec![g1.lease], vec![])).unwrap().is_none());
        // Waiting, holding 300 MB: a second goes beside it.
        let g2 = w.ask(&ask("m1", 3000, 1, vec![g1.lease], wait(g1.lease, 300))).unwrap().unwrap();
        // Holding past its measure: counted at its measure (2000 + 2000 of 5000), no third.
        assert!(w.ask(&ask("m1", 5000, 1, vec![g1.lease, g2.lease], wait(g1.lease, 9000))).unwrap().is_none());
        // Another member naming m1's lease gains nothing by it: m2's 1500 MB fits no job.
        assert!(w.ask(&ask("m2", 1500, 1, vec![g1.lease], wait(g1.lease, 0))).unwrap().is_none());
        // m1 gone while it waited, its leases lapsed: its room whole again when it's back.
        clock.advance(TTL + Duration::from_secs(1));
        assert_eq!(all_granted(&w, &ask("m1", 4000, 2, vec![], vec![])).len(), 2);
        // Cores: 8, measured 2 each: four jobs; one of them waiting, a fifth.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        let gs = w.ask(&ask("m1", 64_000, 8, vec![], vec![])).unwrap().unwrap();
        let mut leases = vec![gs.lease];
        leases.extend(gs.more.iter().map(|g| g.lease));
        assert_eq!(leases.len(), 4);
        assert!(w.ask(&ask("m1", 64_000, 1, leases.clone(), vec![])).unwrap().is_none(), "no core left");
        assert!(w.ask(&ask("m1", 64_000, 1, leases.clone(), wait(leases[0], 300))).unwrap().is_some());
    }

    /// A member's job leaving its cores idle (task #665, `Capacity::idle`: a job waiting on the
    /// network): none of its cores counted, so a job goes beside it; its memory still counts at its
    /// measure (unlike a job waiting on a worker).
    #[test]
    fn a_members_job_leaving_its_cores_idle_gives_its_cores_alone() {
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        let ask = |who: &str, mem: u64, leases: Vec<u64>, idle: Vec<u64>| Ask {
            want: 8,
            cap: Some(Capacity {
                at: unix_now(),
                cores: 8,
                cores_free: 8,
                disk_mb: 64_000,
                leases,
                idle,
                ..Default::default()
            }),
            ..every_ask(who, "slope-hi", mem, 64_000)
        };
        // Cores: 8, measured 2 each: four jobs, then no core left.
        let gs = w.ask(&ask("m1", 64_000, vec![], vec![])).unwrap().unwrap();
        let mut leases = vec![gs.lease];
        leases.extend(gs.more.iter().map(|g| g.lease));
        assert_eq!(leases.len(), 4);
        assert!(w.ask(&ask("m1", 64_000, leases.clone(), vec![])).unwrap().is_none(), "no core left");
        // Two of m1's idle: their cores go to two more.
        assert_eq!(all_granted(&w, &ask("m1", 64_000, leases.clone(), leases[..2].to_vec())).len(), 2);
        // Memory still counts at the measure (2000 MB a job): idle, but no room for a fifth in 9000.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(2));
        let gs = w.ask(&ask("m1", 64_000, vec![], vec![])).unwrap().unwrap();
        let mut leases = vec![gs.lease];
        leases.extend(gs.more.iter().map(|g| g.lease));
        assert!(w.ask(&ask("m1", 9000, leases.clone(), leases.clone())).unwrap().is_none(), "its memory counts");
    }

    /// Task #688: a job counts for the cores it kept busy, not its threads, so jobs waiting on their
    /// reads go together until memory is full; a busy one counts for its threads as ever. The
    /// measures are terrain-hi's on 26 Oct (3 threads, 1.16 cores busy, 600 s).
    #[test]
    fn a_job_counts_for_the_cores_it_kept_busy() {
        let ask = |mem: u64, leases: Vec<u64>| Ask {
            want: 12,
            cap: Some(Capacity {
                at: unix_now(),
                cores: 14,
                cores_free: 14,
                disk_mb: 64_000,
                leases,
                ..Default::default()
            }),
            ..every_ask("m1", "slope-hi", mem, 64_000)
        };
        let packed = |cpu_ms: u64| {
            let (d, c, w, clock) = every((0, 0));
            let ts: Vec<String> = (1..=12).map(|i| format!("6/1/{i}")).collect();
            for t in &ts {
                let sizes =
                    [("threads".to_string(), 3), ("cpu_ms".to_string(), cpu_ms), ("waited_ms".to_string(), 0)].into();
                let cost = Cost {
                    peak_mb: 3700,
                    secs: 600,
                    worker: None,
                    v: cost_version("slope-hi"),
                    sizes,
                    ..Default::default()
                };
                c.add_costs_by(&[(cost_key("slope-hi", t), cost)], "m1");
                c.add_disks(&[(cost_key("slope-hi", t), 100)]);
            }
            c.offer(
                "p",
                vec![Offer {
                    step: "slope-hi".into(),
                    targets: ts.iter().map(|t| (t.clone(), format!("k {t}"), 0)).collect(),
                    job: None,
                }],
            );
            (d, c, w, clock)
        };
        // Not held by its threads (1.16 busy of 3): granted 2 (task #896: the cores it computed on,
        // to the nearest whole, and one more), counted for 1.2; the build's memory, 39 GB, holds ten
        // of 3.7 GB, where by threads 14 cores held seven.
        // (Task #688 review: past the seven their threads allow, one a minute, as the load shows
        // each.)
        let (_d, _c, w, clock) = packed(696_000);
        let gs = w.ask(&ask(39_000, vec![])).unwrap().unwrap();
        let mut leases = vec![gs.lease];
        leases.extend(gs.more.iter().map(|g| g.lease));
        assert_eq!(leases.len(), 8, "seven by their threads, one past them");
        assert!(std::iter::once(&gs).chain(&gs.more).all(|g| g.threads == 2), "each keeps its threads");
        assert!(w.ask(&ask(39_000, leases.clone())).unwrap().is_none(), "none before the load shows the last");
        // (Load past their busy cores, 8 × 1.2 = 10 (task #697: summed in tenths), by more than
        // one: the climb stops.)
        clock.advance(sched::SETTLE);
        let mut swamped = ask(39_000, leases.clone());
        swamped.cap.as_mut().unwrap().cores_free = 14 - 10 - 2;
        assert!(w.ask(&swamped).unwrap().is_none(), "busier than measured: no more");
        while leases.len() < 10 {
            let gs = w.ask(&ask(39_000, leases.clone())).unwrap().expect("one more each minute");
            assert!(gs.more.is_empty(), "one at a time");
            leases.push(gs.lease);
            clock.advance(sched::SETTLE);
        }
        assert!(w.ask(&ask(39_000, leases.clone())).unwrap().is_none(), "memory decides");
        // Busy on its threads (3 of 3): each climbs a thread (task #896), counted for its busy 3
        // and the one it climbs by, unmeasured (task #888 review: no overcommit): three of 4 in 14
        // cores, the fourth short of them.
        let (_d, _c, w, _clock) = packed(1_800_000);
        let gs = w.ask(&ask(64_000, vec![])).unwrap().unwrap();
        let threads: Vec<u32> = std::iter::once(&gs).chain(&gs.more).map(|g| g.threads).collect();
        assert_eq!(threads, [4, 4, 4], "cores decide");
    }

    /// Disk reservations (task #242): a grant carries its job's measured disk, which its agent
    /// reserves; the jobs its capacity names are taken off its disk by the agent already (not twice
    /// here), so it's granted what fits the disk its reservations leave, and nothing past it.
    #[test]
    fn grants_carry_their_disk_and_a_members_reservations_count_once() {
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(1));
        let ask = |disk: u64, leases: Vec<u64>| Ask {
            cap: Some(Capacity { cores: 8, cores_free: 8, disk_mb: disk, leases, ..Default::default() }),
            ..every_ask("m1", "slope-hi", 64_000, disk)
        };
        let g1 = w.ask(&ask(250, vec![])).unwrap().unwrap();
        assert_eq!(g1.disk_mb, 100);
        // 250 free past the target, 100 of it reserved by its job: 150 said, one more fits.
        let g2 = w.ask(&ask(150, vec![g1.lease])).unwrap().unwrap();
        assert_eq!(g2.disk_mb, 100);
        // Both reserved: 50 said, nothing fits.
        assert!(w.ask(&ask(50, vec![g1.lease, g2.lease])).unwrap().is_none());
        // A lease its agent doesn't name yet (granted since it asked) is taken off here.
        assert!(w.ask(&ask(150, vec![g1.lease])).unwrap().is_none());
    }

    /// The disk in words (task #391): the free space, what's left of it for jobs, and what keeps
    /// the rest (the owner's room target, the jobs' reservations), on the refusal too. No fixed
    /// reserve (task #421); an older app's `reserve_mb` parses and is passed over.
    #[test]
    fn the_disk_is_said_plainly() {
        let c = Capacity { free_mb: 39_014, target_mb: 20 << 10, ..Default::default() };
        assert_eq!(disk_words(&c, 18 << 10), "38 GB free · 18 GB for jobs (20 GB room target)");
        let c = Capacity { free_mb: 100 << 10, reserved_mb: 5 << 10, ..Default::default() };
        assert_eq!(disk_words(&c, 95 << 10), "100 GB free · 95 GB for jobs (5 GB reserved by its jobs)");
        let c = Capacity { free_mb: 100 << 10, ..Default::default() };
        assert_eq!(disk_words(&c, 100 << 10), "100 GB free · 100 GB for jobs");
        let old: Capacity =
            serde_json::from_str(r#"{"free_mb": 49152, "reserve_mb": 30720, "target_mb": 20480}"#).unwrap();
        assert_eq!((old.free_mb, old.target_mb), (48 << 10, 20 << 10));
        assert!(!serde_json::to_string(&old).unwrap().contains("reserve_mb"));
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 6, Some(1));
        let cap = Capacity { cores: 8, free_mb: 21 << 10, target_mb: 20 << 10, ..Default::default() };
        let a = Ask { cap: Some(cap), ..every_ask("m1", "slope-hi", 6144, 0) };
        assert!(w.ask(&a).unwrap().is_none());
        let why = c.refusals().get("m1").cloned().unwrap_or_default();
        assert!(why.contains("(6.0 GB memory; disk: 21 GB free · 0 GB for jobs (20 GB room target))"), "{why}");
    }

    /// A shared cache reserved once per Mac (task #392, crate::needs::CACHES): `depth`'s z6 tiles
    /// read one copy of the local copies (crate::localread), so of three jobs of them granted together only the first carries the cache (the fill
    /// measured) beside its own output; the others their output alone. A Mac whose disk can't hold
    /// the cache is granted none of them.
    #[test]
    fn a_shared_cache_is_reserved_once_per_mac() {
        let one = |i: usize| Offer {
            step: "depth".into(),
            targets: vec![(format!("6/1/{i}"), format!("k 6/1/{i}"), 0)],
            job: None,
        };
        let filled = |c: &Coordinator| {
            packable(c, "depth", 6, Some(1));
            c.add_job_run("depth", &["6/1/1".to_string()], 10_100);
            c.offer("p", (1..=3).map(one).collect());
        };
        let (_d, c, w, _clock) = every((0, 0));
        filled(&c);
        let ask = |disk: u64| Ask {
            want: 3,
            can: vec!["depth".into()],
            cap: Some(Capacity { cores: 8, cores_free: 8, disk_mb: disk, ..Default::default() }),
            ..every_ask("m1", "depth", 64_000, disk)
        };
        let mut g = w.ask(&ask(20_000)).unwrap().unwrap();
        let all: Vec<Grant> = std::iter::once(g.clone()).chain(std::mem::take(&mut g.more)).collect();
        let disks: Vec<u64> = all.iter().map(|g| g.disk_mb).collect();
        assert_eq!(disks.len(), 3, "{disks:?}");
        assert_eq!(disks[0], 10_100, "the first the cache and its output: {disks:?}");
        assert!(disks[1..].iter().all(|&d| d == 100), "the others their output: {disks:?}");
        let (_d, c, w, _clock) = every((0, 0));
        filled(&c);
        assert!(w.ask(&ask(10_050)).unwrap().is_none(), "no room for the cache");
        // What its Mac holds of the cache (task #392 review: `Capacity::caches`, read off its disk):
        // whole, nothing past its output; part of it, the rest; none, or not said, the whole fill.
        let here = |have: Option<u64>| Ask {
            cap: Some(Capacity {
                cores: 8,
                cores_free: 8,
                disk_mb: 20_000,
                caches: have
                    .map(|h| (crate::localread::CACHE.to_string(), CacheHere { have_mb: h }))
                    .into_iter()
                    .collect(),
                ..Default::default()
            }),
            want: 1,
            ..ask(20_000)
        };
        for (have, want) in [(Some(10_000), 100), (Some(6_000), 4_100), (Some(0), 10_100), (None, 10_100)] {
            let (_d, c, w, _clock) = every((0, 0));
            filled(&c);
            let g = w.ask(&here(have)).unwrap().unwrap();
            assert_eq!(g.disk_mb, want, "{have:?}");
        }
    }

    /// Task #551 review: the whole copies (crate::needs::CACHES) are reserved only on a Mac with
    /// room for them past its target; one short of it (a laptop) reads the NAS in place, so it's
    /// granted the job with its output alone, not held off.
    #[test]
    fn a_room_only_cache_is_reserved_only_where_there_is_room() {
        let one = |i: usize| Offer {
            step: "depth".into(),
            targets: vec![(format!("6/1/{i}"), format!("k 6/1/{i}"), 0)],
            job: None,
        };
        for (disk, want) in [(40_000, 30_100), (10_050, 100)] {
            let (_d, c, w, _clock) = every((0, 0));
            packable(&c, "depth", 6, Some(1));
            c.add_job_run("depth", &["6/1/1".to_string()], 30_100);
            c.offer("p", (1..=3).map(one).collect());
            assert_eq!(c.fill_mb(crate::localread::CACHE), 30_000);
            let a = Ask {
                want: 1,
                can: vec!["depth".into()],
                cap: Some(Capacity { cores: 8, cores_free: 8, disk_mb: disk, ..Default::default() }),
                ..every_ask("m1", "depth", 64_000, disk)
            };
            let g = w.ask(&a).unwrap().expect("granted");
            assert_eq!(g.disk_mb, want, "{disk} MB of room");
        }
    }

    /// A step that has run before (a disk floor noted), its memory not measured the way it runs
    /// now: a cold start, but not one never measured.
    fn ran_before(c: &Coordinator, step: &str) {
        c.add_disks(&[(cost_key(step, "6/63/63"), 10)]);
    }

    /// A cold `slope` target offered after `step`'s measured ones (2 GB, two threads).
    fn with_cold_slope(c: &Coordinator, step: &str, n: usize) {
        packable(c, step, n, Some(2));
        ran_before(c, "slope-hi");
        let mut offers = c.shared.lock().unwrap().offers.clone();
        offers.push(Offer { step: "slope-hi".into(), targets: vec![("6/2/2".into(), "k 6/2/2".into(), 0)], job: None });
        c.offer("p", offers);
    }

    /// Task #499: a cold start is packed by its provisional need (its step's declared memory times
    /// 3/2): beside measured jobs when that fits the room left, and measured jobs beside it; past the
    /// room left, not beside (alone once its worker holds nothing, whatever its size).
    #[test]
    fn packing_takes_a_cold_start_by_its_provisional_need() {
        let provisional = crate::agent::steps::mem_mb("slope-hi") * 3 / 2;
        let a = |mem: u64| Ask {
            want: 3,
            can: vec!["trees".into(), "slope-hi".into()],
            ..every_ask("m1", "trees", mem, 100_000)
        };
        // Room for both measured (2 GB each) and the cold one: all three.
        let (_d, c, w, _clock) = every((0, 0));
        with_cold_slope(&c, "trees", 2);
        // (Slope first: a shared step, offered to members by its rank.)
        assert_eq!(all_granted(&w, &a(4000 + provisional)), [["6/2/2"], ["6/1/1"], ["6/1/2"]], "beside");
        // A megabyte short: the measured ones only.
        let (_d, c, w, _clock) = every((0, 0));
        with_cold_slope(&c, "trees", 2);
        assert_eq!(all_granted(&w, &a(4000 + provisional - 1)).len(), 2, "past the room left: two of the three");
        // Nothing held: a cold start bigger than its whole room goes, alone.
        let (_d, c, w, _clock) = every((0, 0));
        ran_before(&c, "slope-hi");
        c.offer(
            "p",
            vec![Offer { step: "slope-hi".into(), targets: vec![("6/2/2".into(), "k 6/2/2".into(), 0)], job: None }],
        );
        assert_eq!(all_granted(&w, &a(provisional / 2)), [["6/2/2"]]);
        // Measured jobs beside a running cold start, by its provisional need.
        packable(&c, "trees", 2, Some(2));
        assert_eq!(all_granted(&w, &a(4000 + provisional)), [["6/1/1"], ["6/1/2"]], "beside a cold start");
    }

    /// Task #499 review: a step never measured in any way runs alone, nothing beside it; and at
    /// most one cold start on a Mac: two cold steps asked for by the same Mac, one granted.
    #[test]
    fn a_never_measured_step_runs_alone_and_one_cold_start_a_mac() {
        let lo = || Offer { step: "lo".into(), targets: vec![("3/1/1".into(), "k 3/1/1".into(), 0)], job: None };
        let a = Ask {
            want: 4,
            can: vec!["trees".into(), "slope-hi".into(), "terrain-hi".into(), "lo".into()],
            ..every_ask("m1", "trees", 64_000, 100_000)
        };
        // Never measured (lo): not beside measured jobs; alone on a Mac holding nothing, then
        // nothing beside it.
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "trees", 2, Some(2));
        let mut offers = c.shared.lock().unwrap().offers.clone();
        offers.push(lo());
        c.offer("p", offers.clone());
        assert_eq!(all_granted(&w, &a), [["6/1/1"], ["6/1/2"]]);
        let (_d, c, w, _clock) = every((0, 0));
        c.offer("p", vec![lo()]);
        assert_eq!(all_granted(&w, &a), [["3/1/1"]]);
        packable(&c, "trees", 2, Some(2));
        assert!(all_granted(&w, &a).is_empty(), "beside a step never measured: nothing");
        let why = c.refusals().into_values().collect::<Vec<_>>().join("; ");
        assert!(why.contains("lo was never measured"), "{why}");
        // Two cold steps (both run before), one Mac: one of them.
        let (_d, c, w, _clock) = every((0, 0));
        ran_before(&c, "slope-hi");
        ran_before(&c, "terrain-hi");
        let cold = |step: &str| Offer { step: step.into(), targets: vec![("6/2/2".into(), "k".into(), 0)], job: None };
        c.offer("p", vec![cold("slope-hi"), cold("terrain-hi")]);
        assert_eq!(all_granted(&w, &a).len(), 1, "one cold start a Mac");
        let why = c.refusals().into_values().collect::<Vec<_>>().join("; ");
        assert!(why.contains("one cold start on a Mac at a time"), "{why}");
    }

    /// Task #499 review: a cold start's cores are the whole Mac unless its step honours the threads
    /// it's granted (`THREADS_HONOURED`): a cold terrain job takes all 8, so nothing goes beside
    /// it, nor it beside a measured job; a cold slope job a thread (task #896).
    #[test]
    fn a_cold_start_counts_the_whole_mac_unless_its_step_honours_its_threads() {
        assert!(sched::THREADS_HONOURED.contains(&"slope-hi") && !sched::THREADS_HONOURED.contains(&"terrain-hi"));
        let a = Ask {
            want: 4,
            can: vec!["trees".into(), "terrain-hi".into()],
            ..every_ask("m1", "trees", 64_000, 100_000)
        };
        let cold = Offer { step: "terrain-hi".into(), targets: vec![("6/2/2".into(), "k".into(), 0)], job: None };
        // Beside a measured job: not granted (cores).
        let (_d, c, w, _clock) = every((0, 0));
        packable(&c, "trees", 1, Some(2));
        ran_before(&c, "terrain-hi");
        assert_eq!(all_granted(&w, &Ask { want: 1, can: vec!["trees".into()], ..a.clone() }), [["6/1/1"]]);
        let mut offers = c.shared.lock().unwrap().offers.clone();
        offers.push(cold.clone());
        c.offer("p", offers);
        assert!(all_granted(&w, &a).is_empty(), "the whole Mac's cores");
        let why = c.refusals().into_values().collect::<Vec<_>>().join("; ");
        assert!(why.contains("a job of terrain-hi takes 8"), "{why}");
        // Alone, all 8 threads; nothing beside it.
        let (_d, c, w, _clock) = every((0, 0));
        ran_before(&c, "terrain-hi");
        c.offer("p", vec![cold]);
        let g = w.ask(&a).unwrap().unwrap();
        assert_eq!(g.threads, 8);
        packable(&c, "trees", 2, Some(2));
        assert!(all_granted(&w, &a).is_empty(), "no core left beside it");
    }

    /// Task #823: the lead's own cold pyramids-fill job (a fetch waiting on GSI, its fetches its own
    /// threads) is granted one thread (task #896: a cold start's, measured from there), not the
    /// whole Mac (the mini's first held 6 of 12 cores), so a measured job goes beside it.
    #[test]
    fn a_cold_pyramids_fill_takes_a_thread() {
        assert!(sched::THREADS_HONOURED.contains(&"pyramids-fill"));
        let (_d, c, _w, _clock) = every((0, 0));
        ran_before(&c, "pyramids-fill");
        packable(&c, "trees", 1, Some(2));
        let mut offers = vec![Offer {
            step: "pyramids-fill".into(),
            targets: vec![("6/55/23".into(), "k 6/55/23".into(), 0)],
            job: Some("pyramids-fill 6/55/23".into()),
        }];
        offers.extend(c.shared.lock().unwrap().offers.clone());
        c.offer("p", offers);
        let a = Ask {
            want: 2,
            can: vec!["pyramids-fill".into(), "trees".into()],
            ..every_ask("m4", "pyramids-fill", 64_000, 100_000)
        };
        let g = c.ask_own(&a, &BTreeSet::new()).unwrap();
        assert!(matches!(&g.work, Granted::Job { step, .. } if step == "pyramids-fill"), "{:?}", g.work);
        assert_eq!(g.threads, 1);
        assert_eq!(g.more.len(), 1, "a measured job beside it");
    }

    /// Task #838: a redo (its jobs only change build records, `steps::records_only`) is never a
    /// cold start held for the lead's jobs: a cold one goes beside a long cold job holding the
    /// whole Mac, at one thread.
    #[test]
    fn a_cold_redo_goes_beside_a_long_lead_job() {
        let (_d, c, _w, _clock) = every((0, 0));
        ran_before(&c, "terrain-hi");
        let own = |step: &str, t: &str| Offer {
            step: step.into(),
            targets: vec![(t.into(), format!("k {t}"), 0)],
            job: Some(format!("{step} {t}")),
        };
        let a = Ask {
            want: 2,
            can: vec!["terrain-hi".into(), "redo".into()],
            ..every_ask("m4", "terrain-hi", 64_000, 100_000)
        };
        c.offer("p", vec![own("terrain-hi", "3/1/1")]);
        let g = c.ask_own(&a, &BTreeSet::new()).unwrap();
        assert!(matches!(&g.work, Granted::Job { step, .. } if step == "terrain-hi"), "{:?}", g.work);
        assert_eq!(g.threads, 8, "a cold terrain job: the whole Mac");
        c.offer("p", vec![own("redo", "redo 1")]);
        let g = c.ask_own(&a, &BTreeSet::new()).unwrap();
        assert!(matches!(&g.work, Granted::Job { step, .. } if step == "redo"), "{:?}", g.work);
        assert_eq!(g.threads, 1);
    }

    /// Task #499: the room left is never more than what its Mac's jobs hold now leaves (its memory
    /// guard's last sample, `Capacity::held_mb`): a cold start holding past its provisional need is
    /// counted at what it holds, so nothing goes beside an overrun (held past the whole room
    /// included); the guard stops one that grows past the limit (agent::memguard's tests). An
    /// older agent whose capacity says it holds nothing (0) is packed by the needs alone.
    #[test]
    fn packing_counts_what_a_cold_start_holds_now() {
        let a = |held_mb: u64| Ask {
            want: 3,
            can: vec!["trees".into(), "slope-hi".into()],
            cap: Some(Capacity { at: unix_now(), cores: 8, cores_free: 8, held_mb, ..Default::default() }),
            ..every_ask("m1", "trees", 20_000, 100_000)
        };
        let (_d, c, w, _clock) = every((0, 0));
        ran_before(&c, "slope-hi");
        c.offer(
            "p",
            vec![Offer { step: "slope-hi".into(), targets: vec![("6/2/2".into(), "k 6/2/2".into(), 0)], job: None }],
        );
        assert_eq!(all_granted(&w, &a(0)), [["6/2/2"]]);
        packable(&c, "trees", 4, Some(2));
        // Holding 17 GB of the 20: one 2 GB job fits beside it, not two.
        assert_eq!(all_granted(&w, &a(17_000)), [["6/1/1"]], "by what it holds");
        // Holding 19 GB, or past the whole room: nothing.
        assert!(all_granted(&w, &a(19_000)).is_empty(), "an overrun: nothing beside");
        assert!(all_granted(&w, &a(64_000)).is_empty(), "held past the room: nothing");
        // Held said 0 (an older agent's capacity): by the needs, 20 GB less the cold slope's 1.5 GB
        // and the 2 GB job held: two more.
        assert_eq!(all_granted(&w, &a(0)).len(), 2, "by the needs alone");
    }

    /// Today's stalls (task #499, 21 Oct): (1) bld-fetch, the lead's own, unmeasured the way it
    /// runs now (its floor cleared) on the M4: the other slots no longer sit idle beside it while
    /// it's within its provisional need; (2) slope unmeasured while the M4 runs a one-thread input
    /// check: it starts beside it instead of waiting for a Mac running nothing.
    #[test]
    fn todays_two_cold_starts_no_longer_stall() {
        // (1) bld-fetch held, cold: measured slope jobs go beside it.
        let (_d, c, _w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 4, Some(2));
        ran_before(&c, "bld-fetch");
        let a = Ask { want: 11, can: vec!["slope-hi".into()], ..every_ask("m4", "slope-hi", 40_000, 400_000) };
        let _f = c.hold("bld-fetch", &[("6/9/0".into(), "k 6/9/0".into())]).unwrap();
        let g = c.ask_own(&a, &BTreeSet::new()).expect("beside the cold bld-fetch");
        assert!(1 + g.more.len() >= 2, "{}", 1 + g.more.len());
        // (2) the M4 holding a measured input check; slope not measured the way it runs now:
        // granted beside it.
        let (_d, c, _w, _clock) = every((0, 0));
        packable(&c, "inputs", 1, Some(1));
        ran_before(&c, "slope-hi");
        let _i = c.hold("inputs", &[("6/1/1".into(), "k 6/1/1".into())]).unwrap();
        c.offer(
            "p",
            vec![Offer { step: "slope-hi".into(), targets: vec![("6/2/2".into(), "k 6/2/2".into(), 0)], job: None }],
        );
        let a = Ask { want: 11, can: vec!["slope-hi".into()], ..every_ask("m4", "slope-hi", 40_000, 400_000) };
        let g = c.ask_own(&a, &BTreeSet::new()).expect("slope beside the input check");
        assert!(matches!(&g.work, Granted::Job { step, .. } if step == "slope-hi"));
    }

    fn placement(slack: &[(&str, u64)], speed: &[(&str, f64)]) -> crate::agent::placement::Placement {
        crate::agent::placement::Placement {
            slack: slack.iter().map(|(s, v)| (s.to_string(), *v)).collect(),
            speed: speed.iter().map(|(w, v)| (w.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    /// Placement on (crate::agent::placement): a member's ask tried on the long poles' step first
    /// (no slack in the forecast), and within it from the target measured to take longest; off, the
    /// shared steps by their rank, from the plan's front.
    #[test]
    fn placement_the_long_poles_first() {
        let (_d, c, w, _clock) = every((0, 0));
        for (step, ts) in
            [("pack", vec![("6/1/1", 60)]), ("slope-hi", vec![("3/1/1", 600), ("3/1/2", 900), ("3/1/3", 300)])]
        {
            for (t, secs) in ts {
                let sizes = [("threads".to_string(), 1)].into();
                c.add_costs_by(
                    &[(
                        cost_key(step, t),
                        Cost { peak_mb: 500, secs, worker: None, v: cost_version(step), sizes, ..Default::default() },
                    )],
                    "m1",
                );
                c.add_disks(&[(cost_key(step, t), 10)]);
            }
        }
        let offers = || {
            vec![
                Offer { step: "pack".into(), targets: vec![("6/1/1".into(), "k".into(), 0)], job: None },
                Offer {
                    step: "slope-hi".into(),
                    targets: ["3/1/1", "3/1/2", "3/1/3"].iter().map(|t| (t.to_string(), format!("k {t}"), 0)).collect(),
                    job: None,
                },
            ]
        };
        let a = Ask { can: vec!["pack".into(), "slope-hi".into()], ..every_ask("m1", "pack", 4096, 10_000) };
        // Off: the shared step (slope) first, from its front.
        c.offer("p", offers());
        let g = w.ask(&a).unwrap().unwrap();
        assert!(matches!(&g.work, Granted::Job { step, targets, .. } if step == "slope-hi" && targets[0].0 == "3/1/1"));
        w.give_back(g.lease, "the test").unwrap();
        // On: slope has no slack (it ends with the build), pack ten minutes': slope first, from its
        // longest, then the next longest.
        c.set_placement(Some(placement(&[("slope-hi", 0), ("pack", 600)], &[])));
        c.offer("p", offers());
        assert_eq!(granted(&w, &a), ["3/1/2"]);
        assert_eq!(granted(&w, &a), ["3/1/1"]);
        assert_eq!(granted(&w, &a), ["3/1/3"]);
        assert_eq!(granted(&w, &a), ["6/1/1"]);
        // Off again: as before.
        c.set_placement(None);
        assert!(c.shared.lock().unwrap().placement.is_none());
    }

    /// Task #440 review: placement leaves a member out of a target's draw only while load from
    /// outside the pool fills its cores, not while its own jobs do. Member m2 holds one job: granted
    /// all 8 cores, all busy by its load, it stays in the draw by its weight; granted 4 with all 8
    /// busy (4 outside, the most counted: half), it's left out and m1 takes every target.
    #[test]
    fn placement_leaves_out_only_a_member_swamped_outside_the_pool() {
        let (_d, c, _w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 40, Some(1));
        c.set_placement(Some(placement(&[], &[("m1", 1.0), ("m2", 1.0)])));
        let offer = c.shared.lock().unwrap().offers[0].clone();
        let one = |i: usize| Offer { targets: vec![offer.targets[i].clone()], ..offer.clone() };
        let a1 = every_ask("m1", "slope-hi", 64_000, 100_000);
        // (Its slots past the job it holds free: it would take another, task #552.)
        let a2 = Ask {
            want: 2,
            cap: Some(Capacity { at: unix_now(), cores: 8, cores_free: 0, ..Default::default() }),
            ..every_ask("m2", "slope-hi", 64_000, 100_000)
        };
        let m1_takes = |threads: u32| -> usize {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            for l in s.leases.all(now) {
                s.leases.cancel(l.id);
            }
            let id = s.leases.grant(
                "m2",
                Work::Job {
                    step: "slope-hi".into(),
                    targets: vec![(offer.targets[39].0.clone(), offer.targets[39].1.clone())],
                },
                now,
            );
            s.leases.set_threads(id, threads);
            s.seen("m1", String::new(), Some(&a1), now);
            s.seen("m2", String::new(), Some(&a2), now);
            (0..39).filter(|&i| !s.pick_every(&one(i), &a1, now).is_empty()).count()
        };
        let in_draw = m1_takes(8);
        assert!(in_draw > 5 && in_draw < 34, "m2 by its weight: m1 took {in_draw} of 39");
        assert_eq!(m1_takes(4), 39, "m2 swamped outside the pool: left out");
    }

    /// Task #440 review, #896: the load average is rounded up, so an excess of one core counts no
    /// cores busy outside the pool; past it, all of the excess, at most half the Mac's cores.
    #[test]
    fn the_loads_rounding_isnt_load_outside_the_pool() {
        let ask = |free: u32| Ask {
            cap: Some(Capacity { at: unix_now(), cores: 8, cores_free: free, ..Default::default() }),
            ..every_ask("m1", "slope-hi", 64_000, 100_000)
        };
        assert_eq!(outside_cores(&ask(7), 0), 0, "one: the load's rounding");
        assert_eq!(outside_cores(&ask(6), 0), 2);
        assert_eq!(outside_cores(&ask(5), 2), 0, "its job's two and one more");
        assert_eq!(outside_cores(&ask(4), 2), 2);
        assert_eq!(outside_cores(&ask(0), 2), 4, "at most half its cores (task #684)");
    }

    /// Task #684: a load far past the Mac's cores (a niced test build) keeps half its cores at
    /// most: its own jobs still get the other half.
    #[test]
    fn load_outside_the_pool_keeps_half_the_cores_at_most() {
        let (_d, c, _w, _clock) = every((0, 0));
        packable(&c, "slope-hi", 40, Some(1));
        let offer = c.shared.lock().unwrap().offers[0].clone();
        let mut a = every_ask("m1", "slope-hi", 64_000, 100_000);
        a.want = 8;
        a.cap = Some(Capacity { at: unix_now(), cores: 12, cores_free: 0, ..Default::default() });
        assert_eq!(outside_cores(&a, 0), 6);
        // (The forecast packs it into the same: its cores less those (State::cores_left).)
        assert_eq!(sched::State::default().cores_left(&a), 6);
        let mut s = c.shared.lock().unwrap();
        let now = s.now();
        s.seen("m1", String::new(), Some(&a), now);
        assert!(!s.pick_every(&offer, &a, now).is_empty(), "granted beside the load");
    }

    /// Task #440 review: a member's job weighing the whole isn't starved by two members' jobs
    /// filling the budget: after `OWN_AGE` held back for it alone it goes beside them (none of them
    /// the whole).
    #[test]
    fn a_members_whole_job_goes_after_waiting() {
        let (_d, c, _w, clock) = every((0, 0));
        heavy_slope(&c);
        let whole = [("threads".to_string(), 1), ("bytes".to_string(), 120_000_000_000)];
        let cost = Cost {
            peak_mb: 2000,
            secs: 600,
            worker: None,
            v: cost_version("slope-hi"),
            sizes: whole.into(),
            ..Default::default()
        };
        c.add_costs_by(&[(cost_key("slope-hi", "6/1/6"), cost)], "m1");
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        let offer = {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
            s.offers[0].clone()
        };
        let six = Offer { targets: vec![offer.targets[5].clone()], ..offer.clone() };
        let a3 = every_ask("m3", "slope-hi", 64_000, 100_000);
        let held = |c: &Coordinator| {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            for (w, i) in [("m1", 0), ("m2", 1)] {
                let ts = vec![offer.targets[i].0.clone()].into_iter().map(|t| (t, String::new())).collect();
                s.leases.grant(w, Work::Job { step: "slope-hi".into(), targets: ts }, now);
            }
        };
        held(&c);
        let take = |c: &Coordinator| {
            let s = c.shared.lock().unwrap();
            let now = s.now();
            s.pick_every(&six, &a3, now)
        };
        assert!(take(&c).is_empty(), "the budget full: waits");
        clock.advance(crate::budget::nas::OWN_AGE - Duration::from_secs(1));
        {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            for l in s.leases.all(now) {
                s.leases.cancel(l.id);
            }
        }
        held(&c);
        assert!(take(&c).is_empty(), "not yet aged");
        clock.advance(Duration::from_secs(1));
        assert_eq!(take(&c).len(), 1, "aged: goes beside the members' halves");
    }

    /// Task #444: a member's job weighing less than the whole but more than the room the leased
    /// jobs leave (depth-lo's) isn't starved by a stream of smaller jobs refilling the budget: until
    /// `OWN_AGE` the smaller ones go; once aged, its room is held (no smaller job that would take it
    /// starts, a member's or the lead's), and it goes once it outweighs the leased jobs together.
    #[test]
    fn a_members_heavy_job_reserves_room_after_waiting() {
        let (_d, c, _w, clock) = every((0, 0));
        heavy_slope(&c);
        let weigh = |t: &str, bytes: u64| {
            let sizes = [("threads".to_string(), 1), ("bytes".to_string(), bytes)];
            let cost = Cost {
                peak_mb: 2000,
                secs: 600,
                worker: None,
                v: cost_version("slope-hi"),
                sizes: sizes.into(),
                ..Default::default()
            };
            c.add_costs_by(&[(cost_key("slope-hi", t), cost)], "m1");
        };
        // (6/1/1 500; 6/1/2 to 6/1/5 200 each; 6/1/6 600.)
        weigh("6/1/6", 36_000_000_000);
        for i in 2..=5 {
            weigh(&format!("6/1/{i}"), 12_000_000_000);
        }
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        let offer = {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
            s.offers[0].clone()
        };
        let one = |i: usize| Offer { targets: vec![offer.targets[i].clone()], ..offer.clone() };
        let lease = |c: &Coordinator, w: &str, i: usize| {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            let ts = vec![(offer.targets[i].0.clone(), String::new())];
            s.leases.grant(w, Work::Job { step: "slope-hi".into(), targets: ts }, now)
        };
        let end = |c: &Coordinator, id| {
            c.shared.lock().unwrap().leases.cancel(id);
        };
        let take = |c: &Coordinator, w: &str, i: usize| {
            let s = c.shared.lock().unwrap();
            let now = s.now();
            s.pick_every(&one(i), &every_ask(w, "slope-hi", 64_000, 100_000), now)
        };
        // (A third of `OWN_AGE` passing, the leases `ids` beating every 10 s, as their workers do.)
        let tick = |c: &Coordinator, ids: &[u64]| {
            for _ in 0..crate::budget::nas::OWN_AGE.as_secs() / 30 {
                clock.advance(Duration::from_secs(10));
                let mut s = c.shared.lock().unwrap();
                let now = s.now();
                for l in s.leases.all(now).into_iter().filter(|l| ids.contains(&l.id)) {
                    assert!(s.leases.renew(l.id, &l.worker, None, now));
                }
            }
        };
        let big = lease(&c, "m1", 0);
        let mut small = lease(&c, "m2", 1);
        assert!(take(&c, "m3", 5).is_empty(), "500 + 200 + 600: waits");
        // (A stream of smaller jobs: each one ending, the next goes in its place before it's aged.)
        for i in [2, 3] {
            tick(&c, &[big, small]);
            end(&c, small);
            assert!(take(&c, "m3", 5).is_empty(), "500 + 600: waits");
            assert_eq!(take(&c, "m4", i).len(), 1, "not aged: the smaller one goes");
            small = lease(&c, "m4", i);
        }
        tick(&c, &[big, small]);
        assert!(take(&c, "m3", 5).is_empty(), "aged, but 500 + 200 + 600 > 1000");
        assert!(take(&c, "m4", 4).is_empty(), "aged: its room held, no smaller job takes it");
        assert!(take(&c, "m2", 1).is_empty(), "nor another member's");
        let lead = c.shared.lock().unwrap().lead_name.clone();
        assert!(take(&c, &lead, 4).is_empty(), "nor the lead's");
        end(&c, small);
        // (Task #603: aged, heavier than the leased jobs together, it goes past the whole: it
        // waits for heavier work to end, never for lighter.)
        assert_eq!(take(&c, "m3", 5).len(), 1, "aged, 600 outweighs the 500 leased: it goes");
        let held = lease(&c, "m3", 5);
        assert!(take(&c, "m4", 4).is_empty(), "1100 leased: nothing else goes");
        end(&c, big);
        end(&c, held);
        assert_eq!(take(&c, "m4", 4).len(), 1, "they ended: the smaller one goes");
    }

    /// Task #444 review: a target over the budget that fits no asking member (its measured memory
    /// past theirs) is never noted waiting, so it never ages into holding the budget's room.
    #[test]
    fn a_target_no_member_can_take_never_reserves() {
        let (_d, c, _w, clock) = every((0, 0));
        heavy_slope(&c);
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        let offer = {
            let mut s = c.shared.lock().unwrap();
            (s.nas, s.nas_own) = (Some(limits), Some(limits));
            s.offers[0].clone()
        };
        let one = |i: usize| Offer { targets: vec![offer.targets[i].clone()], ..offer.clone() };
        {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            for (w, i) in [("m1", 0), ("m2", 1)] {
                let ts = vec![(offer.targets[i].0.clone(), String::new())];
                s.leases.grant(w, Work::Job { step: "slope-hi".into(), targets: ts }, now);
            }
        }
        let take = |c: &Coordinator, mem: u64| {
            let s = c.shared.lock().unwrap();
            let now = s.now();
            s.pick_every(&one(5), &every_ask("m3", "slope-hi", mem, 100_000), now)
        };
        // (Its 2000 MB past the 1000 the member spares: not noted, whatever the budget.)
        assert!(take(&c, 1000).is_empty());
        clock.advance(Duration::from_secs(60));
        assert!(take(&c, 1000).is_empty());
        assert!(c.shared.lock().unwrap().nas_waits.lock().unwrap().is_empty(), "never noted waiting");
        // (One that could take it but for the budget: noted.)
        assert!(take(&c, 64_000).is_empty());
        assert_eq!(c.shared.lock().unwrap().nas_waits.lock().unwrap().len(), 1);
        // (Never noted, it can't have aged into holding room past `OWN_AGE` either: `nas_reserved`
        // reads only the waits noted.)
    }

    /// Placement on: of two members asking whose room fits a target, it's drawn to one by their
    /// measured speeds, the other passing it by: exactly one of them gets each, the faster about
    /// three in four at three times the speed, the same draw in the same window, and a new window a
    /// new draw. Off: either may take any.
    #[test]
    fn placement_draws_each_target_to_one_member_by_measured_speed() {
        let (_d, c, w1, clock) = every((0, 0));
        let w2 = client::Client::at(w1.urls(), c.contact.token.clone(), "m2");
        let ts: Vec<String> = (0..400).map(|i| format!("6/{}/{}", i % 20, i / 20)).collect();
        // (In one call each: every call writes the whole costs file.)
        let cost = Cost { peak_mb: 500, secs: 900, worker: None, v: cost_version("pack"), ..Default::default() };
        c.add_costs_by(&ts.iter().map(|t| (cost_key("pack", t), cost.clone())).collect::<Vec<_>>(), "m1");
        c.add_disks(&ts.iter().map(|t| (cost_key("pack", t), 10)).collect::<Vec<_>>());
        // (Both asking lately: nothing offered yet.)
        let (a1, a2) = (every_ask("m1", "pack", 4096, 10_000), every_ask("m2", "pack", 4096, 10_000));
        assert!(w1.ask(&a1).unwrap().is_none() && w2.ask(&a2).unwrap().is_none());
        let one = |t: &str| Offer { step: "pack".into(), targets: vec![(t.to_string(), "k".into(), 0)], job: None };
        let picks = |c: &Coordinator| -> Vec<(bool, bool)> {
            let s = c.shared.lock().unwrap();
            let now = s.now();
            ts.iter()
                .map(|t| (!s.pick_every(&one(t), &a1, now).is_empty(), !s.pick_every(&one(t), &a2, now).is_empty()))
                .collect()
        };
        // Off: both may take every one.
        assert!(picks(&c).iter().all(|&(x, y)| x && y));
        c.set_placement(Some(placement(&[], &[("m1", 3.0), ("m2", 1.0)])));
        let first = picks(&c);
        assert!(first.iter().all(|&(x, y)| x != y), "each to exactly one");
        let share = first.iter().filter(|p| p.0).count() as f64 / ts.len() as f64;
        assert!((share - 0.75).abs() < 0.06, "{share}");
        // The same window: the same draws; the next: drawn again (a target passed by isn't held by
        // one member for good).
        // (Both still asking.)
        clock.advance(Duration::from_secs(crate::agent::placement::DRAW_S - 1));
        assert!(w1.ask(&a1).unwrap().is_none() && w2.ask(&a2).unwrap().is_none());
        assert_eq!(picks(&c), first);
        clock.advance(Duration::from_secs(1));
        assert!(w1.ask(&a1).unwrap().is_none() && w2.ask(&a2).unwrap().is_none());
        let next = picks(&c);
        assert!(next.iter().all(|&(x, y)| x != y) && next != first);
        // A target m2 couldn't take itself (it failed it lately, or it's past its room) isn't drawn
        // to it: m1 takes it.
        {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            for t in &ts[..200] {
                s.failed.insert(("m2".into(), cost_key("pack", t)), (now, 1));
            }
        }
        let after = picks(&c);
        assert!(after[..200].iter().all(|&(x, y)| x && !y), "failed on m2: m1's");
        assert_eq!(&after[200..], &next[200..]);
        let small = every_ask("m2", "pack", 100, 10_000);
        assert!(w2.ask(&small).unwrap().is_none());
        assert!(picks(&c).iter().all(|&(x, _)| x), "past m2's room: m1's");
        // The other no longer asking: the one asking takes any.
        clock.advance(ASKING);
        assert!(w1.ask(&a1).unwrap().is_none());
        let s = c.shared.lock().unwrap();
        let now = s.now();
        assert!(ts.iter().all(|t| !s.pick_every(&one(t), &a1, now).is_empty()));
    }

    /// Placement on: a task of a kind a worker is measured slower at than this Mac goes to it only
    /// while the head start it was offered with covers the worker's run (its pace, with the margin,
    /// times this Mac's measured time at the kind); one not offered ahead, never; off, as before
    /// (never).
    #[test]
    fn placement_a_slower_worker_gets_a_task_only_with_a_head_start_that_covers_it() {
        let (d, c, m1) = start();
        let clock = store::clock::Virtual::new();
        c.shared.lock().unwrap().clock = clock.clone();
        let ask = Ask {
            worker: "m1".into(),
            kind: "native".into(),
            can: vec!["terrainsub".into()],
            mem_mb: 4096,
            ..Default::default()
        };
        let root = d.path().join("job");
        std::fs::create_dir_all(&root).unwrap();
        let offer = |ahead: Option<f64>| {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            s.tasks
                .offer(
                    task::Offer {
                        owner: 42,
                        kind: "terrainsub".into(),
                        spec: serde_json::json!({}),
                        root: root.clone(),
                        inputs: Default::default(),
                        mem_mb: 100,
                        ahead,
                    },
                    now,
                )
                .unwrap()
        };
        {
            let mut s = c.shared.lock().unwrap();
            // Four times this Mac's time; this Mac's own run of one, 10 s.
            s.tasks.paces.insert(("m1".into(), "terrainsub".into()), 4.0);
            s.tasks.note_here("terrainsub", 10.0);
        }
        let took = |c: &Coordinator| match m1.ask(&ask).unwrap().map(|g| g.work) {
            Some(Granted::Task { id, .. }) => {
                c.shared.lock().unwrap().tasks.close(id);
                Some(id)
            }
            _ => None,
        };
        let plain = offer(None);
        let ahead = offer(Some(6.0));
        // Off: neither (it's slower).
        assert_eq!(took(&c), None);
        c.set_placement(Some(placement(&[], &[])));
        // On: 4 × 1.25 × 10 s = 50 s within the 60 s head start: the one offered ahead.
        assert_eq!(took(&c), Some(ahead));
        // 15 s on, 45 s left: not enough.
        let late = offer(Some(6.0));
        clock.advance(Duration::from_secs(15));
        assert_eq!(took(&c), None);
        assert!(
            c.shared.lock().unwrap().tasks.by_id.contains_key(&late)
                && c.shared.lock().unwrap().tasks.by_id.contains_key(&plain)
        );
        // Measured faster: any, as before.
        c.shared.lock().unwrap().tasks.paces.insert(("m1".into(), "terrainsub".into()), 0.5);
        assert_eq!(took(&c), Some(plain));
    }

    /// The lead's plan leaves out what other workers' leases hold, its own jobs' aside.
    #[test]
    fn the_targets_other_workers_hold_are_theirs_alone() {
        let (_d, c, w, _clock) = every((0, 0));
        measured_offer(&c, "pack", &[("6/1/1", Some((500, 10, 60)))]);
        let own = c.hold("pack", &[("6/1/2".into(), "k".into())]).unwrap();
        let g = w.ask(&every_ask("m1", "pack", 4096, 10_000)).unwrap().unwrap();
        assert_eq!(c.held("pack").len(), 2);
        assert_eq!(c.held_by_others("pack"), ["6/1/1".to_string()].into());
        c.finish(own, &[]);
        w.give_back(g.lease, "the test").unwrap();
        // A cold start of it under way on m1: none of the lead's own meanwhile; once measured, any.
        measured_offer(&c, "lo", &[("3/1/1", None), ("3/1/2", None)]);
        assert!(!c.cold_under_way("lo"), "none under way");
        let own = c.hold("lo", &[("3/1/2".into(), "k".into())]).unwrap();
        assert!(!c.cold_under_way("lo"), "the lead's own isn't another's");
        c.finish(own, &[]);
        let _ = w.ask(&every_ask("m1", "lo", 4096, 10_000)).unwrap().unwrap();
        assert!(c.cold_under_way("lo"));
        c.add_costs(&[(
            cost_key("lo", "3/1/9"),
            Cost { peak_mb: 500, secs: 60, worker: None, v: cost_version("lo"), ..Default::default() },
        )]);
        assert!(!c.cold_under_way("lo"), "measured: not a cold start");
    }

    /// Runs are contiguous in plan order: a target held (by the lead's own job, or another worker's)
    /// ends a run, and the next starts at the front of what's open.
    #[test]
    fn every_step_runs_are_contiguous() {
        let (_d, c, w, _clock) = every((0, 0));
        let m = Some((500, 10, 60));
        measured_offer(&c, "pack", &[("6/1/1", m), ("6/1/2", m), ("6/1/3", m), ("6/1/4", m)]);
        c.hold("pack", &[("6/1/2".into(), "k 6/1/2".into())]).unwrap();
        let a = every_ask("m1", "pack", 4096, 10_000);
        assert_eq!(granted(&w, &a), ["6/1/1"]);
        assert_eq!(granted(&w, &a), ["6/1/3", "6/1/4"]);
    }

    fn w_step(w: &client::Client, a: &Ask) -> Option<String> {
        match w.ask(a).unwrap().map(|g| g.work) {
            Some(Granted::Job { step, targets, .. }) => Some(format!("{step} {}", targets[0].0)),
            _ => None,
        }
    }

    #[test]
    fn a_hand_off_may_touch_only_its_steps_files() {
        let ts = |t: &str| vec![(t.to_string(), "k".to_string())];
        let h = |step: &str, t: &str, files: &[&str], raw: &[(&str, &str)]| {
            let changes = files.iter().map(|l| (l.to_string(), Some(format!("{l}.0000000000000003.pack")))).collect();
            let raw = raw
                .iter()
                .map(|(a, n)| (a.to_string(), crate::rawpack::Pack { name: n.to_string(), bytes: 1 }))
                .collect();
            Handoff { changes, done: Some((step.into(), ts(t))), raw, ..Default::default() }
        };
        // A z3 tile's lo pack and its z6 tiles' hi packs; not another z3 tile's.
        assert!(check_handoff(
            &h(
                "slope-hi",
                "3/2/2",
                &["layers/slope/lo/3-2-2", "layers/slope/hi/6-16-16", "layers/slope/hi/6-23-23"],
                &[]
            ),
            "slope-hi",
            &ts("3/2/2")
        )
        .is_ok());
        // Part of a lease's targets (a job paused at a safe point, or stopped): those it did, and
        // only their files.
        let two = vec![("3/2/2".to_string(), "k".to_string()), ("3/3/2".to_string(), "k".to_string())];
        assert!(check_handoff(&h("slope-hi", "3/2/2", &["layers/slope/lo/3-2-2"], &[]), "slope-hi", &two).is_ok());
        assert!(
            check_handoff(
                &h("slope-hi", "3/2/2", &["layers/slope/lo/3-2-2", "layers/slope/lo/3-3-2"], &[]),
                "slope-hi",
                &two
            )
            .is_err(),
            "a file of a target it didn't do"
        );
        assert!(check_handoff(&h("slope-hi", "3/2/2", &["layers/slope/hi/6-24-16"], &[]), "slope-hi", &ts("3/2/2"))
            .is_err());
        assert!(check_handoff(&h("slope-hi", "3/2/2", &["layers/terrain/hi/6-16-16"], &[]), "slope-hi", &ts("3/2/2"))
            .is_err());
        assert!(check_handoff(
            &h("trees", "3/2/2", &["layers/trees-leaf/hi/6-17-17", "layers/trees-cover/lo/3-2-2"], &[]),
            "trees",
            &ts("3/2/2")
        )
        .is_ok());
        // A tree cover z6 tile: its own hi packs and its mid, never its z3 tile's lo pack or another
        // z6 tile's; an assembly isn't a helper's.
        assert!(check_handoff(
            &h(
                "trees",
                "6/17/17",
                &["layers/trees-leaf/hi/6-17-17", "layers/trees-cover/hi/6-17-17", "work/trees-mid/6-17-17"],
                &[]
            ),
            "trees",
            &ts("6/17/17")
        )
        .is_ok());
        assert!(check_handoff(&h("trees", "6/17/17", &["layers/trees-cover/lo/3-2-2"], &[]), "trees", &ts("6/17/17"))
            .is_err());
        assert!(
            check_handoff(&h("trees", "6/17/17", &["work/trees-mid/6-17-18"], &[]), "trees", &ts("6/17/17")).is_err()
        );
        assert!(check_handoff(
            &h("trees-lo", "3/2/2", &["layers/trees-cover/lo/3-2-2"], &[]),
            "trees-lo",
            &ts("3/2/2")
        )
        .is_err());
        assert!(
            saves("trees-lo", "3/2/2", "layers/trees-leaf/lo/3-2-2")
                && !saves("trees-lo", "3/2/2", "layers/trees-leaf/hi/6-16-16")
        );
        // Terrain's and slope's z6 tiles: their hi pack and mid; their assemblies: their lo pack.
        assert!(
            saves("terrain-hi", "6/16/16", "layers/terrain/hi/6-16-16")
                && saves("terrain-hi", "6/16/16", "work/terrain-mid/6-16-16")
        );
        assert!(
            !saves("terrain-hi", "6/16/16", "layers/terrain/lo/3-2-2")
                && !saves("terrain-hi", "6/16/16", "layers/terrain/hi/6-16-17")
                && !saves("terrain-hi", "6/16/16", "work/slope-mid/6-16-16")
        );
        assert!(
            saves("slope-hi", "6/16/16", "layers/slope/hi/6-16-16")
                && saves("slope-hi", "6/16/16", "work/slope-mid/6-16-16")
                && !saves("slope-hi", "6/16/16", "layers/slope/lo/3-2-2")
        );
        assert!(
            saves("terrain-lo", "3/2/2", "layers/terrain/lo/3-2-2")
                && !saves("terrain-lo", "3/2/2", "layers/slope/lo/3-2-2")
                && !saves("terrain-lo", "3/2/2", "layers/terrain/hi/6-16-16")
        );
        assert!(
            saves("slope-lo", "3/2/2", "layers/slope/lo/3-2-2")
                && !saves("slope-lo", "3/2/2", "work/slope-mid/6-16-16")
        );
        // (A z3 tile's whole run, a lease of the scheme before: as it was.)
        assert!(
            saves("terrain-hi", "3/2/2", "layers/terrain/lo/3-2-2")
                && saves("terrain-hi", "3/2/2", "layers/terrain/hi/6-16-16")
        );
        assert!(check_handoff(&h("pois", "6/1/3", &["work/pois/6-1-3"], &[]), "pois", &ts("6/1/3")).is_ok());
        assert!(check_handoff(&h("pois", "6/1/3", &["work/peaks/6-1-3"], &[]), "pois", &ts("6/1/3")).is_err());
        // Raw tiles' archives: its own shards, named by their content.
        let terrain = |raw: &[(&str, &str)]| {
            check_handoff(&h("terrain-hi", "3/2/2", &["layers/terrain/lo/3-2-2"], raw), "terrain-hi", &ts("3/2/2"))
        };
        assert!(terrain(&[("6-20-21", "6-20-21.0123456789abcdef.tiles"), ("3-2-2", "3-2-2.0123456789abcdef.tiles")])
            .is_ok());
        // Another shard's (an earlier job's tiles, packed with this one's) too, and the root's.
        assert!(
            terrain(&[("6-30-21", "6-30-21.0123456789abcdef.tiles"), ("root", "root.0123456789abcdef.tiles")]).is_ok()
        );
        assert!(terrain(&[("6-64-21", "6-64-21.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("5-20-21", "5-20-21.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-020-21", "6-020-21.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-20-21", "6-20-22.0123456789abcdef.tiles")]).is_err());
        assert!(terrain(&[("6-20-21", "../6-20-21.0123456789abcdef.tiles")]).is_err());
        let peaks = |a: &str| {
            check_handoff(
                &h("peaks", "6/20/21", &["work/peaks/6-20-21"], &[(a, &format!("{a}.0123456789abcdef.tiles"))]),
                "peaks",
                &ts("6/20/21"),
            )
        };
        assert!(
            peaks("6-21-22").is_ok()
                && peaks("3-2-2").is_ok()
                && peaks("6-25-21").is_ok()
                && peaks("../6-25-21").is_err()
        );
        // A step no worker does.
        assert!(check_handoff(&h("catalog", "catalog", &[], &[]), "catalog", &ts("catalog")).is_err());
    }

    #[test]
    fn a_helper_asks_beats_and_hands_back_once() {
        let (d, c, w) = start();
        let units = vec![
            ("6/1/3".to_string(), "k3".to_string(), 20u64 << 20),
            ("6/1/1".into(), "k1".into(), 10 << 20),
            ("6/1/2".into(), "k2".into(), 500 << 20),
            ("6/1/4".into(), "k4".into(), 30 << 20),
        ];
        measure_units(&c, 4);
        c.add_costs(&[(
            "6/1/2".into(),
            Cost { peak_mb: 5000, secs: 60, worker: None, v: cost_version("unit"), ..Default::default() },
        )]);
        c.offer_units("2026-09-28", units.clone());
        // This Mac's own job holds 6/1/4.
        let own = c.hold("unit", &[("6/1/4".into(), "k4".into())]).unwrap();
        // A run from the front, not what this Mac builds, ended by one needing more than it spares
        // (6/1/2, 5 GB); the pass comes with it.
        let g = w.ask(&ask(4096)).unwrap().unwrap();
        let Granted::Job { targets, pass, .. } = &g.work else { panic!() };
        assert_eq!(targets, &vec![("6/1/3".to_string(), "k3".to_string()), ("6/1/1".into(), "k1".into())]);
        assert_eq!(pass, "2026-09-28");
        assert!(w.ask(&ask(4096)).unwrap().is_none(), "held: nothing left");
        assert!(w.beat(g.lease, Some("1/2 z3 tiles")).unwrap());
        // A hand-off naming another z6 tile's files is refused; its own, journaled whole.
        // (Its raw tiles' archives, on the NAS whatever became of it, kept for naming: the well
        // named alone.)
        let mut bad = handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")]);
        bad.changes.insert("base/6-9-9".into(), None);
        bad.raw = vec![
            ("6-1-1".into(), crate::rawpack::Pack { name: "6-1-1.0123456789abcdef.tiles".into(), bytes: 9 }),
            ("6-1-1".into(), crate::rawpack::Pack { name: "../x.tiles".into(), bytes: 9 }),
        ];
        assert!(matches!(
            w.done(&Done { lease: g.lease, handoff: Some(bad), ..Default::default() }).unwrap(),
            client::Handed::Refused(_)
        ));
        let kept = crate::handoff::waiting_in(&c.journal()).unwrap();
        assert_eq!(kept.len(), 1);
        assert!(
            kept[0].0.parent().unwrap().ends_with(crate::handoff::RAW_AGAIN)
                && kept[0].1.done.is_none()
                && kept[0].1.changes.is_empty()
        );
        assert_eq!(
            kept[0].1.raw.iter().map(|r| r.1.name.as_str()).collect::<Vec<_>>(),
            ["6-1-1.0123456789abcdef.tiles"]
        );
        std::fs::remove_file(&kept[0].0).unwrap();
        assert_eq!(
            w.done(&Done {
                lease: g.lease,
                handoff: Some(handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")])),
                costs: vec![(
                    "6/1/3".into(),
                    Cost { peak_mb: 2000, secs: 300, worker: None, v: 0, ..Default::default() }
                )],
                ..Default::default()
            })
            .unwrap(),
            client::Handed::Taken
        );
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
        assert_eq!(
            w.done(&Done {
                lease: g.lease,
                handoff: Some(handoff(&[("6/1/3", "k3"), ("6/1/1", "k1")])),
                ..Default::default()
            })
            .unwrap(),
            client::Handed::Gone
        );
        c.finish(own, &[("6/1/4".to_string(), "k4".to_string())]);
        let mut changed = units.clone();
        changed[1].1 = "k1b".into();
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
        // Another step's file of the same tile, another z6 tile's, a value that isn't its content name.
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
        assert!(
            manifest.starts_with("HTTP/1.1 200")
                && manifest.contains("application/manifest+json")
                && manifest.contains("\"start_url\": \"./\"")
        );
        let sw = String::from_utf8(get("/work/sw.js")).unwrap();
        assert!(
            sw.starts_with("HTTP/1.1 200")
                && !sw.contains("__VERSION__")
                && sw.contains(&format!("const VERSION = \"{}\"", http::page_version()))
        );
        // (Its list of the page's files, to keep as it installs: the page's own address among them,
        // not the worker's.)
        assert!(
            !sw.contains("__PAGE__")
                && sw.contains(r#"const PAGE = ["/work/","/work/index.html","/work/dash.js""#)
                && !sw.contains(r#""/work/sw.js""#)
        );
        let icon = get("/work/icons/icon-192.png");
        assert!(icon.starts_with(b"HTTP/1.1 200") && icon.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"));
        // (Task #792, the server's own layers: the page gzipped for a client that asks, a PNG never.)
        let get_gz = |path: &str| {
            let mut s = std::net::TcpStream::connect(&addr).unwrap();
            s.write_all(
                format!("GET {path} HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
            let mut b = Vec::new();
            s.read_to_end(&mut b).unwrap();
            String::from_utf8_lossy(&b[..b.windows(4).position(|x| x == b"\r\n\r\n").unwrap()]).to_ascii_lowercase()
        };
        let page = get_gz("/work/");
        assert!(page.contains("content-encoding: gzip") && page.contains("vary: accept-encoding"), "{page}");
        assert!(!get_gz("/work/icons/icon-192.png").contains("content-encoding"));
        // A connection whose headers don't come is closed by the server once their time is up, not
        // before. (Timed from before it's made: the server's clock can't start before this one,
        // however slowly this test's thread goes on. This end's own wait is a watchdog: a read that
        // runs it out means the server never closed it, which a busy Mac doesn't explain.)
        let t = Instant::now();
        let mut slow = std::net::TcpStream::connect(&addr).unwrap();
        slow.write_all(b"GET /work/ HTTP/1.1\r\nHost").unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(600))).unwrap();
        let mut b = [0u8; 64];
        match slow.read(&mut b) {
            Ok(n) => assert!(
                t.elapsed() >= http::HEADERS_MAX,
                "closed after {:?} with {n} bytes, before the headers' time was up",
                t.elapsed()
            ),
            Err(e) => panic!("the server never closed it: no answer in {:?} ({e})", t.elapsed()),
        }
        drop(held);
    }

    #[test]
    fn leases_and_the_token_outlive_a_restart() {
        let (d, c, w) = start();
        measure_units(&c, 2);
        c.offer_units("p", vec![("6/1/1".into(), "k1".into(), 1 << 20), ("6/1/2".into(), "k2".into(), 1 << 20)]);
        let own = c.hold("unit", &[("6/1/2".into(), "k2".into())]).unwrap();
        let g = w.ask(&Ask { ..ask(4096) }).unwrap().unwrap();
        let token = c.contact.token.clone();
        drop(c);
        // (A new port: the old listener's threads live on in this process.)
        let (c2, port) = start_for_test(&d.path().join("coord"), "m4", "");
        assert_eq!(c2.contact.token, token);
        let w2 = client::Client::at(vec![format!("http://127.0.0.1:{port}")], token, "m1");
        assert!(w2.beat(g.lease, None).unwrap(), "the helper's lease lives on");
        assert!(!c2.renew(own, None), "this Mac's own ended with its agent");
        assert_eq!(
            w2.done(&Done { lease: g.lease, handoff: Some(handoff(&[("6/1/1", "k1")])), ..Default::default() })
                .unwrap(),
            client::Handed::Taken
        );
    }

    /// A request as a browser, a page or a proxy sends it, straight to `addr`: `headers` over the
    /// usual ones (Host x, JSON); the answer's status, where a redirect leads, and its JSON.
    fn send(
        addr: &str,
        method: &str,
        path: &str,
        key: Option<&str>,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> (u16, String, serde_json::Value) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        let b = body.to_string();
        let mut h: Vec<(String, String)> =
            [("Host", "x"), ("Content-Type", "application/json"), ("X-Worker", "a page")]
                .iter()
                .filter(|(k, _)| !headers.iter().any(|(o, _)| o.eq_ignore_ascii_case(k)))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        h.extend(headers.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        if let Some(k) = key {
            h.push(("Authorization".into(), format!("Bearer {k}")));
        }
        let head: String = h.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        // (Written in one piece: a refusal answered once the head is in closes the connection, and
        // a body written after it, as on a loaded Mac, has the connection reset, the answer lost.)
        s.write_all(
            format!("{method} {path} HTTP/1.1\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len())
                .as_bytes(),
        )
        .unwrap();
        let mut r = Vec::new();
        s.read_to_end(&mut r).unwrap();
        let text = String::from_utf8_lossy(&r).to_string();
        let code = text.split(' ').nth(1).unwrap().parse().unwrap();
        let location = text
            .lines()
            .find_map(|l| l.strip_prefix("location: ").or_else(|| l.strip_prefix("Location: ")))
            .unwrap_or("")
            .trim()
            .to_string();
        let json = text.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str(b.trim()).ok()).unwrap_or_default();
        (code, location, json)
    }

    #[test]
    fn the_build_page_reads_and_helps_without_a_key() {
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
        let (code, _, steps) = send(&addr, "POST", "/work/steps", None, &[], &serde_json::json!({}));
        assert_eq!(code, 200);
        assert_eq!(steps["steps"][0]["id"], "osm-pass");
        assert_eq!(steps["stages"][0]["id"], "pass");
        // Its address without the slash leads to it, wherever a proxy serves it.
        assert_eq!(ask("GET", "/work", None), (302, "work/".to_string()));
        // Pausing: no key; the agents' own requests: the build's key.
        assert_eq!(ask("POST", "/work/pause", None).0, 200);
        assert_eq!(ask("POST", "/work/status", None).0, 403);
        assert_eq!(ask("POST", "/work/pause", Some(&c.contact.token)).0, 200);
        // (A wrong key is refused, so an agent with an old one reads the new one.)
        assert_eq!(ask("POST", "/work/pause", Some("0123456789abcdef0123456789abcdef")).0, 401);
    }

    #[test]
    fn a_members_broker_passes_the_owners_asks_from_any_device_to_its_agent() {
        // (Task #409 review; #521: from any device) a broker (not the lead's: its agent passes them
        // on): a page's pause goes to its agent, from this Mac or another device; a retry that
        // can't reach the lead is answered so.
        let (_d, c, w) = start();
        let home = tempfile::tempdir().unwrap();
        c.set_forward(Some(home.path().to_path_buf()));
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let far = [("X-Forwarded-For", "100.101.1.2")];
        let pause = serde_json::json!({ "pause": { "mode": "drain", "by": "x", "at": 0 } });
        for h in [&far[..], &[]] {
            assert_eq!(send(&addr, "POST", "/work/pause", None, h, &pause).0, 200);
            assert!(crate::control::take_request(home.path()).is_some() && c.pause().is_none(), "passed to its agent");
        }
        c.set_retry_blocked(Some("no lead now".into()));
        let (code, _, b) =
            send(&addr, "POST", "/work/act", None, &far, &serde_json::json!({ "retry": { "target": "6/1/1" } }));
        assert_eq!(code, 409);
        assert!(b["error"].as_str().is_some_and(|e| e.contains("no lead now")), "{b}");
        assert!(c.take_page_acts().is_empty());
    }

    /// Task #871 step 3: this Mac's pause from any device, through its agent's control (answered
    /// once it takes it up, within a second), and the app's quit from this Mac alone.
    #[test]
    fn a_hold_is_answered_by_the_agents_control_and_a_quit_is_this_macs() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let far = [("X-Forwarded-For", "100.101.1.2")];
        let held = serde_json::json!({ "held": true });
        assert_eq!(send(&addr, "POST", "/work/hold", None, &far, &held).0, 503, "no folder said yet");
        let home = tempfile::tempdir().unwrap();
        c.set_settings(home.path(), None);
        let hold = crate::agent::bus::Snap::new(crate::control::HoldState::default());
        let (beat, _beats) = std::sync::mpsc::channel();
        let mut control = crate::agent::asks::Handle::new(home.path().to_path_buf(), hold.clone(), beat);
        control.spawn();
        let t = Instant::now();
        let (code, _, b) = send(&addr, "POST", "/work/hold", None, &far, &held);
        assert_eq!((code, b["ok"].as_bool()), (200, Some(true)), "{b}");
        store::testkit::ended_before(t, Duration::from_secs(1), "answered within a second");
        assert!(hold.get().held.as_ref().is_some_and(|h| h.by == "the build page (100.101.1.2)"));
        assert!(crate::control::read_hold(home.path()).held.is_some());
        let (code, _, b) = send(&addr, "POST", "/work/hold", None, &far, &serde_json::json!({ "held": false }));
        assert_eq!((code, b["ok"].as_bool()), (200, Some(true)), "{b}");
        assert!(hold.get().held.is_none());
        assert_eq!(send(&addr, "POST", "/work/hold", None, &far, &serde_json::json!({})).0, 400);
        let quit = serde_json::json!({ "mode": "finish" });
        assert_eq!(send(&addr, "POST", "/work/quit", None, &far, &quit).0, 403);
        assert!(crate::control::peek_quit(home.path()).is_none());
        assert_eq!(send(&addr, "POST", "/work/quit", None, &[], &quit).0, 200);
        assert!(crate::control::peek_quit(home.path()).is_some_and(|q| q.mode == crate::control::QuitMode::Finish));
        store::testkit::eventually("control took the quit up", || control.quit.get().is_some());
    }

    #[test]
    fn the_build_pages_settings_go_through_the_commands_own_functions() {
        // (Task #521) `/work/settings` from any device: read, set and cleared in this Mac's agent's
        // folder and on the NAS, as `scenic in-use|memory|room|nas-room` would; a bad ask changes
        // nothing.
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let far = [("X-Forwarded-For", "100.101.1.2")];
        let post = |b: serde_json::Value| send(&addr, "POST", "/work/settings", None, &far, &b);
        assert_eq!(post(serde_json::json!({})).0, 503, "no folder said yet");
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        c.set_settings(home.path(), Some(&store::nasfs::NasRoot::local_for_test(nas.path())));
        let (code, _, v) = post(serde_json::json!({}));
        assert_eq!(code, 200, "{v}");
        assert_eq!(v["memory"]["set"], false);
        let (code, _, v) = post(serde_json::json!({ "in_use": 0, "memory": "2", "room": 1, "nas_room": "3 GB" }));
        assert_eq!(code, 200, "{v}");
        assert_eq!(crate::agent::share::read(home.path()).unwrap().cores, Some(0));
        let m = crate::agent::buildmem::read(home.path()).unwrap();
        assert!(m.mb == 2048 && m.by == "the build page (100.101.1.2)", "{m:?}");
        assert_eq!(crate::agent::room::target(home.path()).unwrap().bytes, 1 << 30);
        assert_eq!(
            crate::agent::room::nas_reserve(&store::nasfs::NasRoot::local_for_test(nas.path())).unwrap().bytes,
            3 << 30
        );
        assert_eq!(v["nas_room"]["bytes"], 3u64 << 30);
        assert_eq!(post(serde_json::json!({ "memory": "0.1" })).0, 400);
        assert_eq!(post(serde_json::json!({ "in_use": [1] })).0, 400);
        assert_eq!(crate::agent::buildmem::read(home.path()).unwrap().mb, 2048);
        let (code, _, v) =
            post(serde_json::json!({ "in_use": "off", "memory": "off", "room": "off", "nas_room": "off" }));
        assert_eq!(code, 200, "{v}");
        assert!(
            crate::agent::share::read(home.path()).is_none() && crate::agent::buildmem::read(home.path()).is_none()
        );
        assert!(
            crate::agent::room::target(home.path()).is_none()
                && crate::agent::room::nas_reserve(&store::nasfs::NasRoot::local_for_test(nas.path())).is_none()
        );
        c.set_settings(home.path(), None);
        assert_eq!(post(serde_json::json!({ "nas_room": "3" })).0, 400, "the NAS away");
        assert_eq!(post(serde_json::json!({ "room": "off" })).0, 200, "the NAS away: this Mac's still");
    }

    #[test]
    fn a_retry_forgets_a_targets_failures_by_every_worker() {
        // (Task #409 phase 3) a page's retry: no worker held off the target any more.
        let (_d, c, _w) = start();
        {
            let mut s = c.shared.lock().unwrap();
            let now = s.now();
            for (w, k) in [("m1", "6/1/2"), ("m2", "6/1/2"), ("m1", "slope-hi 6/1/2"), ("m1", "6/1/3")] {
                s.failed.insert((w.into(), k.into()), (now, 1));
            }
        }
        assert_eq!(c.clear_failed(Some("unit"), "6/1/2"), 2);
        assert_eq!(c.clear_failed(None, "6/1/2"), 1, "any step's");
        let left: Vec<String> = c.shared.lock().unwrap().failed.keys().map(|k| k.1.clone()).collect();
        assert_eq!(left, ["6/1/3"]);
        // (Kept so: the state the lead writes per term, read again by a restart's take-up.)
        let kept: Vec<String> = c.pool_state().failed.iter().map(|f| f.1.clone()).collect();
        assert_eq!(kept, ["6/1/3"]);
    }

    #[test]
    fn a_viewer_shows_the_page_does_nothing_else_and_keeps_nothing() {
        // (Task #409 review) a Mac with neither a coordinator nor a broker: the page and its views,
        // every other call refused, its key's or not; nothing written but its token, in its scratch.
        let (listener, port) = test_listener();
        let v = Coordinator::viewer(listener, "m1", "1.2.3").unwrap();
        let addr = format!("127.0.0.1:{port}");
        let ask = |path: &str, key: Option<&str>| send(&addr, "POST", path, key, &[], &serde_json::json!({})).0;
        assert_eq!(send(&addr, "GET", "/work/", None, &[], &serde_json::json!({})).0, 200);
        assert_eq!(ask("/work/steps", None), 200);
        let far = |path: &str| {
            send(
                &addr,
                "POST",
                path,
                None,
                &[("X-Forwarded-For", "100.101.1.2")],
                &serde_json::json!({ "cancel": { "target": "6/1/2" } }),
            )
            .0
        };
        for p in ["/work/pause", "/work/ask", "/work/beat", "/work/status", "/work/act", "/work/inputs"] {
            assert_eq!(ask(p, Some(&v.contact.token)), 403, "{p} with its key");
        }
        for p in ["/work/ask", "/work/beat", "/work/status"] {
            assert_eq!(far(p), 403, "{p} from another device");
        }
        // (#521) the owner's asks from any device: a cancel from another device queued for its agent.
        assert_eq!(far("/work/act"), 200);
        assert!(v.take_page_acts().len() == 1);
        for p in ["/work/ask", "/work/beat", "/work/status"] {
            assert_eq!(ask(p, None), 403, "{p}");
        }
        // (Phase 3) this Mac's own page: its pause passed to its agent, as its menu's; its cancel
        // and retry queued for its agent.
        let home = tempfile::tempdir().unwrap();
        v.set_forward(Some(home.path().to_path_buf()));
        let pause = serde_json::json!({ "pause": { "mode": "drain", "by": "x", "at": 0 } });
        assert_eq!(send(&addr, "POST", "/work/pause", None, &[], &pause).0, 200);
        assert!(crate::control::take_request(home.path()).is_some_and(|r| r.pause.is_some()));
        assert!(v.pause().is_none(), "not the viewer's to set");
        let act = |b: serde_json::Value| send(&addr, "POST", "/work/act", None, &[], &b).0;
        assert_eq!(act(serde_json::json!({ "cancel": { "target": "6/1/2", "step": "unit" } })), 200);
        assert_eq!(act(serde_json::json!({ "retry": { "target": "6/1/3" } })), 200);
        assert_eq!(act(serde_json::json!({ "both": 1 })), 400);
        // (Its cancel through its agent's own cancel asks, as `scenic job cancel`; its retry queued
        // for the agent to mail to the lead.)
        let cancels = crate::control::peek_cancels(home.path());
        assert!(
            matches!(&cancels[..], [c] if c.target == "6/1/2" && c.by.starts_with("the build page")),
            "{cancels:?}"
        );
        let acts = v.take_page_acts();
        assert!(matches!(&acts[..], [crate::pool::driver::Act::Retry(r)] if r.target == "6/1/3"), "{acts:?}");
        // In the pool (a pageview told a root), never the memory's views: not ready until it read.
        let pv = crate::agent::pageview::PageView::new();
        pv.set_inputs(crate::agent::pageview::Inputs {
            root: Some(store::nasfs::NasRoot::local_for_test("/nonexistent")),
            ..Default::default()
        });
        v.set_pageview(pv);
        assert_eq!(ask("/work/swarm", None), 503);
        assert_eq!(ask("/work/history", None), 503);
        assert_eq!(v.shared.lock().unwrap().history.seq(), 0, "no history of its own");
        let dir = v.shared.lock().unwrap().dir.clone();
        let mut kept: Vec<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        kept.sort();
        assert_eq!(kept, ["workers-token"]);
        assert!(dir.starts_with(std::env::temp_dir()));
        // Its scratch goes as it stops; a dead process's goes as the next starts.
        v.stop();
        assert!(!dir.exists());
        let stale = std::env::temp_dir().join(format!("scenic-viewer-{}-1", i32::MAX - 7));
        std::fs::create_dir_all(&stale).unwrap();
        let (listener, _) = test_listener();
        let next = Coordinator::viewer(listener, "m1", "1.2.3").unwrap();
        assert!(!stale.exists());
        next.stop();
    }

    #[test]
    fn the_leads_page_has_its_pause_from_memory_a_members_from_the_nas() {
        // (Task #673 review) the lead's NAS mirror waits for its agent's next loop: its own pages
        // see a press at once.
        let (d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let root = d.path().join("nas");
        std::fs::create_dir_all(&root).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let pv = crate::agent::pageview::PageView::new();
        pv.set_inputs(crate::agent::pageview::Inputs { root: Some(nas.clone()), ..Default::default() });
        pv.touch();
        assert!(pv.tick(&crate::agent::pageview::NasFiles(nas.clone()), unix_now()));
        c.set_pageview(pv);
        let pause = || send(&addr, "POST", "/work/swarm", None, &[], &serde_json::json!({})).2["pause"].clone();
        assert!(pause().is_null());
        let p = crate::control::Pause { mode: crate::control::Mode::Freeze, by: "x".into(), at: 1 };
        c.set_pause(Some(p), 1);
        assert_eq!(pause()["mode"], "freeze", "the lead's own, before its mirror");
        // A member's (forwarding its pages' asks): the NAS's alone, none there.
        c.set_forward(Some(d.path().into()));
        assert!(pause().is_null());
    }

    #[test]
    fn only_a_request_naming_this_mac_from_its_own_page_and_the_owners_and_jobs_from_this_mac_itself() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let post = |path: &str, key: Option<&str>, headers: &[(&str, &str)]| {
            send(&addr, "POST", path, key, headers, &serde_json::json!({ "kind": "tail" })).0
        };
        // A name a web page elsewhere could point here (DNS rebinding), a page elsewhere, or a JSON
        // request sent as a form is: refused, with the key or not.
        assert_eq!(post("/work/swarm", None, &[("Host", "evil.example")]), 403);
        assert_eq!(post("/work/swarm", None, &[("Origin", "https://evil.example")]), 403);
        assert_eq!(post("/work/pause", Some(&c.contact.token), &[("Origin", "http://192.168.1.66:8000")]), 403);
        assert_eq!(post("/work/swarm", None, &[("Content-Type", "text/plain")]), 415);
        // The build page's own, at the address it asked for: straight, or through tailscale serve.
        assert_eq!(
            post("/work/swarm", None, &[("Host", "100.70.85.80:8090"), ("Origin", "http://100.70.85.80:8090")]),
            200
        );
        assert_eq!(
            post(
                "/work/swarm",
                None,
                &[
                    ("Host", "127.0.0.1:8090"),
                    ("X-Forwarded-Host", "mac.tail1.ts.net"),
                    ("X-Forwarded-For", "100.101.1.2"),
                    ("Origin", "https://mac.tail1.ts.net")
                ]
            ),
            200
        );
        // A job's, from this Mac itself: not handed over by a proxy, whatever key.
        assert_eq!(post("/task/workers", Some(&c.job_token), &[]), 200);
        assert_eq!(post("/task/workers", Some(&c.job_token), &[("X-Forwarded-For", "100.101.1.2")]), 403);
        assert_eq!(post("/task/workers", Some(&c.contact.token), &[]), 401);
        // Through the proxy, from the internet (Tailscale Funnel), or an address that's none: never.
        assert_eq!(post("/work/swarm", None, &[("X-Forwarded-For", "8.8.8.8")]), 403);
        assert_eq!(
            post("/work/swarm", None, &[("X-Forwarded-For", "100.101.1.2"), ("Tailscale-Funnel-Request", "?1")]),
            403
        );
        assert_eq!(post("/work/swarm", None, &[("X-Forwarded-For", "100.101.1.2, nonsense")]), 403);
    }

    #[test]
    fn the_key_pages_carried_long_ago_is_no_key() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("coord");
        std::fs::create_dir_all(&dir).unwrap();
        let old = "0123456789abcdef0123456789abcdef";
        std::fs::write(dir.join("token"), old).unwrap();
        let (c, port) = start_for_test(&dir, "m4", "");
        assert!(c.contact.token != old && !dir.join("token").exists());
        let addr = format!("127.0.0.1:{port}");
        assert_eq!(send(&addr, "POST", "/work/pause", Some(old), &[], &serde_json::json!({})).0, 401);
        assert_eq!(send(&addr, "POST", "/work/pause", Some(&c.contact.token), &[], &serde_json::json!({})).0, 200);
    }

    /// The build page's Accept and Accept All on a held unit's banner (docs/inputs.md §4.5): asks
    /// for the agent to write, warnings' ids alone, never from a site elsewhere.
    #[test]
    fn a_page_accepts_the_gates_warnings() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let page = |body: serde_json::Value| send(&addr, "POST", "/work/inputs", None, &[], &body).0;
        assert_eq!(page(serde_json::json!({ "unit": "some-unit", "accept": ["gt-neg.0123456789abcdef"] })), 200);
        assert_eq!(page(serde_json::json!({ "unit": "some-unit", "all": true })), 200);
        assert_eq!(page(serde_json::json!({ "unit": "some-unit", "accept": ["not an id"] })) / 100, 4);
        assert_eq!(page(serde_json::json!({ "unit": "../state", "all": true })) / 100, 4);
        assert_eq!(page(serde_json::json!({ "unit": "some-unit" })) / 100, 4);
        assert_eq!(
            send(
                &addr,
                "POST",
                "/work/inputs",
                None,
                &[("Origin", "https://evil.example")],
                &serde_json::json!({ "unit": "some-unit", "all": true })
            )
            .0,
            403
        );
        let asks = c.take_inputs_asks();
        assert_eq!(
            asks.iter().map(|a| (a.accept.clone(), a.all)).collect::<Vec<_>>(),
            [(vec!["gt-neg.0123456789abcdef".to_string()], false), (vec![], true)]
        );
        assert!(asks.iter().all(|a| a.unit == "some-unit" && a.by.starts_with("the build page")));
        // A member's agent's word to list now (with the build's key), and its owner's ask to forget
        // a z6 tile's records: never a page's forget.
        assert_eq!(
            send(
                &addr,
                "POST",
                "/work/inputs",
                Some(&c.contact.token),
                &[],
                &serde_json::json!({ "unit": "a/b", "check": true, "full": true })
            )
            .0,
            200
        );
        assert_eq!(
            send(
                &addr,
                "POST",
                "/work/inputs",
                Some(&c.contact.token),
                &[],
                &serde_json::json!({ "unit": "a/b", "forget": true })
            )
            .0,
            200
        );
        assert_eq!(page(serde_json::json!({ "unit": "some-unit", "forget": true })) / 100, 4);
        let asks = c.take_inputs_asks();
        assert!(asks[0].check && asks[0].full && asks[1].forget && asks.len() == 2);
        assert!(c.take_inputs_asks().is_empty());
    }

    /// Task #876: pages no longer help with the build. A page left open from before (its asks,
    /// beats, hand-backs and failures, its tasks' files and the programs) is told so with no key,
    /// whatever it asks for and whatever name it gives: never a task (one offered stays offered, no
    /// worker is added), a beat answered `ok: false` (it stops that task) and the rest refused
    /// with `NO_PAGE_HELPERS` (never an error it retries at once); the owner's asks (pausing)
    /// still go through without a key, the agents' with theirs.
    #[test]
    fn the_coordinator_never_offers_a_page_a_task_and_tells_an_old_page_so() {
        let (d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let token = c.contact.token.clone();
        // A JSON request as a page makes it: the answer's status and body.
        let page = |path: &str, key: Option<&str>, body: serde_json::Value| -> (u16, serde_json::Value) {
            let (code, _, v) = send(&addr, "POST", path, key, &[], &body);
            (code, v)
        };
        let root = d.path().join("job");
        std::fs::create_dir_all(root.join("u")).unwrap();
        std::fs::write(root.join("u/in.bin"), b"input").unwrap();
        let job = client::Client::at(w.urls().to_vec(), c.job_token.clone(), "job");
        let offer = task::Offer {
            owner: 42,
            kind: "tail".into(),
            spec: serde_json::json!({ "unit": "6/1/1" }),
            root,
            inputs: [("u/in.bin".to_string(), 5)].into(),
            mem_mb: 100,
            ahead: None,
        };
        let id =
            job.post_json("/task/offer", &serde_json::to_value(&offer).unwrap()).unwrap().1["id"].as_u64().unwrap();
        // An old page's ask, as it made it (and an empty key, which is none): refused, said why.
        let old = serde_json::json!({ "worker": "Safari on iPad ab12cd", "kind": "web", "label": "Safari on iPad", "can": ["tail", "bldtile", "treeblock", "terrainsub"], "mem_mb": 3000, "cores": 4, "visible": true });
        for key in [None, Some("")] {
            let (code, b) = page("/work/ask", key, old.clone());
            assert_eq!((code, b["error"].as_str()), (403, Some(NO_PAGE_HELPERS)), "{b}");
        }
        // Whatever it says it is.
        let native = serde_json::json!({ "worker": "m4", "kind": "native", "can": ["unit", "tail"], "mem_mb": 3000, "cores": 4 });
        assert_eq!(page("/work/ask", None, native).1["error"], NO_PAGE_HELPERS);
        // A beat for a lease it held: not held (it stops that task), the lead's own untouched.
        let lease = c.hold("unit", &[("6/1/1".to_string(), "k".to_string())]).unwrap();
        let (code, b) =
            page("/work/beat", None, serde_json::json!({ "worker": "m4", "lease": lease, "progress": "x" }));
        assert_eq!((code, b["ok"].as_bool(), b["error"].as_str()), (200, Some(false), Some(NO_PAGE_HELPERS)));
        for path in ["/work/done", "/work/fail"] {
            let (code, b) = page(path, None, serde_json::json!({ "worker": "m4", "lease": lease, "error": "x" }));
            assert_eq!((code, b["error"].as_str()), (403, Some(NO_PAGE_HELPERS)), "{path}: {b}");
        }
        assert!(c.renew(lease, None), "the lead's own lease, untouched");
        // Its tasks' files and the programs' WebAssembly builds: none.
        for (method, path) in [
            ("GET", "/work/in/1/u/in.bin"),
            ("GET", "/work/net/1/nas/sources/x"),
            ("PUT", "/work/out/1/u/out.bin"),
            ("GET", "/work/prog/tile.wasm"),
        ] {
            let (code, _, b) = send(&addr, method, path, None, &[("X-Worker", "page m4")], &serde_json::json!({}));
            assert_eq!((code, b["error"].as_str()), (403, Some(NO_PAGE_HELPERS)), "{method} {path}: {b}");
        }
        // Nothing given, no one added: the task waits for an agent.
        assert_eq!(job.post_json(&format!("/task/{id}"), &serde_json::json!({})).unwrap().1["state"], "offered");
        assert!(c.shared.lock().unwrap().workers.keys().all(|n| !n.contains("Safari") && !n.starts_with("page ")));
        // An agent, with the build's key, takes it.
        let m1 = client::Client::at(w.urls().to_vec(), token.clone(), "m1");
        let g = m1
            .ask(&Ask {
                worker: "m1".into(),
                kind: "native".into(),
                can: vec!["tail".into()],
                mem_mb: 1000,
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert!(matches!(g.work, Granted::Task { id: t, .. } if t == id));
        // Not the agents' own requests, and not a job's.
        assert_eq!(page("/work/status", None, serde_json::json!({})).0, 403);
        assert_eq!(page("/task/workers", None, serde_json::json!({})).0, 401);
        // Its pause: made now by this Mac's clock, by the build page, whatever it says.
        let forever =
            serde_json::json!({ "pause": { "mode": "drain", "by": "the owner", "at": u64::MAX }, "at": u64::MAX });
        assert_eq!(page("/work/pause", None, forever.clone()).0, 200);
        let p = c.shared.lock().unwrap().paused.clone().unwrap();
        assert!(
            p.by.starts_with("the build page") && p.at <= unix_now() && c.shared.lock().unwrap().pause_at <= unix_now(),
            "{}",
            p.by
        );
        // (So the owner's resume goes through. An agent's time is its own, a minute ahead at most.)
        assert_eq!(page("/work/pause", Some(&token), serde_json::json!({ "pause": null })).0, 200);
        assert!(c.shared.lock().unwrap().paused.is_none());
        assert_eq!(page("/work/pause", Some(&token), forever).0, 200);
        assert!(c.shared.lock().unwrap().pause_at <= unix_now() + AHEAD_S);
        assert_eq!(
            page("/work/pause", Some(&token), serde_json::json!({ "pause": null, "at": unix_now() + AHEAD_S })).0,
            200
        );
        assert!(c.shared.lock().unwrap().paused.is_none());
    }

    /// A task given back (it couldn't start on the worker, #703) is "task-back" in the history, its
    /// why the note and no `ok: false`; one that failed stays "task-fail".
    #[test]
    fn a_task_given_back_is_not_a_failure() {
        let (d, c, w) = start();
        let root = d.path().join("job");
        std::fs::create_dir_all(&root).unwrap();
        let job = client::Client::at(w.urls().to_vec(), c.job_token.clone(), "job");
        let offer = task::Offer {
            owner: 42,
            kind: "tail".into(),
            spec: serde_json::json!({ "unit": "6/1/1" }),
            root,
            inputs: Default::default(),
            mem_mb: 800,
            ahead: None,
        };
        job.post_json("/task/offer", &serde_json::to_value(&offer).unwrap()).unwrap();
        let ask = Ask {
            worker: "m1".into(),
            kind: "native".into(),
            can: vec!["tail".into()],
            mem_mb: 4000,
            cores: 4,
            ..Default::default()
        };
        let g = w.ask(&ask).unwrap().unwrap();
        w.give_back(g.lease, "needs 25 GB free on the disk").unwrap();
        let g = w.ask(&ask).unwrap().unwrap();
        w.fail(g.lease, "exit status: 1", None).unwrap();
        let ends: Vec<_> = c
            .history_since(0)
            .into_iter()
            .filter(|e| e.kind.starts_with("task-"))
            .map(|e| (e.kind, e.ok, e.note))
            .collect();
        assert_eq!(
            ends,
            [
                ("task-back".to_string(), None, "needs 25 GB free on the disk".to_string()),
                ("task-fail".to_string(), Some(false), "exit status: 1".to_string()),
            ]
        );
    }

    #[test]
    fn a_task_goes_out_and_comes_back() {
        let (d, c, w) = start();
        let root = d.path().join("job");
        std::fs::create_dir_all(root.join("u")).unwrap();
        std::fs::write(root.join("u/in.bin"), b"input").unwrap();
        // The job's side, from this Mac.
        let job = client::Client::at(w.urls().to_vec(), c.job_token.clone(), "job");
        let offer = task::Offer {
            owner: 42,
            kind: "tail".into(),
            spec: serde_json::json!({ "unit": "6/1/1" }),
            root: root.clone(),
            inputs: [("u/in.bin".to_string(), 5)].into(),
            mem_mb: 800,
            ahead: None,
        };
        let id =
            job.post_json("/task/offer", &serde_json::to_value(&offer).unwrap()).unwrap().1["id"].as_u64().unwrap();
        // A worker that spares too little gets nothing; one that spares enough, the task.
        let native = |mem| Ask {
            worker: "ipad".into(),
            kind: "native".into(),
            can: vec!["tail".into()],
            mem_mb: mem,
            cores: 4,
            ..Default::default()
        };
        let ipad = client::Client::at(w.urls().to_vec(), c.contact.token.clone(), "ipad");
        assert!(ipad.ask(&native(500)).unwrap().is_none());
        let g = ipad.ask(&native(1000)).unwrap().unwrap();
        let Granted::Task { id: tid, .. } = g.work else { panic!() };
        assert_eq!(tid, id);
        assert_eq!(job.post_json(&format!("/task/{id}"), &serde_json::json!({})).unwrap().1["state"], "leased");
        // Its input, then its output.
        assert_eq!(ipad.get_bytes(&format!("/work/in/{}/u/in.bin", g.lease)).unwrap(), b"input");
        assert!(ipad.get_bytes(&format!("/work/in/{}/u/other.bin", g.lease)).is_err());
        ipad.put_bytes(&format!("/work/out/{}/u/out.bin", g.lease), b"output!").unwrap();
        assert_eq!(
            ipad.done(&Done {
                lease: g.lease,
                outputs: vec![task::Output { path: "u/out.bin".into(), size: 7 }],
                secs: 2.0,
                peak_mb: 300,
                ..Default::default()
            })
            .unwrap(),
            client::Handed::Taken
        );
        let st = job.post_json(&format!("/task/{id}"), &serde_json::json!({})).unwrap().1;
        assert_eq!(st["state"], "done");
        let out = PathBuf::from(st["out"].as_str().unwrap());
        assert_eq!(std::fs::read(out.join("u/out.bin")).unwrap(), b"output!");
        // The job took it: its folder goes.
        job.post_json(&format!("/task/{id}/close"), &serde_json::json!({})).unwrap();
        assert!(!out.exists());
        // What a worker measured for the z6 tile replaces the guess, more or less: 300 MB and a tenth.
        let id3 = job
            .post_json("/task/offer", &serde_json::to_value(&task::Offer { mem_mb: 5000, ..offer }).unwrap())
            .unwrap()
            .1["id"]
            .as_u64()
            .unwrap();
        let phone = client::Client::at(w.urls().to_vec(), c.contact.token.clone(), "phone");
        let g = phone.ask(&Ask { worker: "phone".into(), ..native(500) }).unwrap().unwrap();
        let Granted::Task { id: tid, mem_mb, .. } = g.work else { panic!() };
        assert_eq!((tid, mem_mb), (id3, 330));
    }

    /// A request as `send` makes it, its answer whole (status line, headers, body), with the
    /// usual headers but `X-Worker` (a preflight sends none).
    fn raw(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> String {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        let head: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: x\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        req.extend_from_slice(body);
        s.write_all(&req).unwrap();
        let mut r = Vec::new();
        s.read_to_end(&mut r).unwrap();
        String::from_utf8_lossy(&r).to_string()
    }

    fn status(r: &str) -> u16 {
        r.split(' ').nth(1).unwrap().parse().unwrap()
    }

    fn header_of<'a>(r: &'a str, name: &str) -> Option<&'a str> {
        r.split("\r\n\r\n")
            .next()
            .unwrap()
            .lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()))
    }

    /// `/work/live` (docs/pool.md §10, Live): 503 until the agent gives it a store; then the
    /// store's answer, allowed to a member's page (CORS) once the brokers are told, never elsewhere's.
    #[test]
    fn the_live_store_answers_the_page_and_a_members_with_cors() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let page = "https://m1.t.ts.net";
        let post = |origin: Option<&str>| {
            let mut h = vec![("Content-Type", "application/json")];
            h.extend(origin.map(|o| ("Origin", o)));
            raw(&addr, "POST", "/work/live", &h, br#"{"range": 60}"#)
        };
        // No store yet: not ready.
        assert_eq!(status(&post(None)), 503);
        let store = crate::agent::live::Store::new(None, "m4");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        store.record(crate::agent::live::LOAD, now, 2.5);
        c.set_live(store);
        let r = post(None);
        assert_eq!(status(&r), 200, "{r}");
        let a: serde_json::Value = serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!((a["host"].as_str(), a["step"].as_u64()), (Some("m4"), Some(1)));
        assert_eq!(a["series"]["load1"][0][2], 2.5);
        // Another Mac's page: allowed once it's a member's (its brokers told), never elsewhere's.
        assert_eq!(status(&post(Some(page))), 403);
        let own = brokers::Broker { urls: c.contact.urls.clone(), ..Default::default() };
        let m1 = brokers::Peer {
            member: "m-1".into(),
            host: "m1".into(),
            lead: false,
            broker: Some(brokers::Broker { https: Some(format!("{page}/")), ..Default::default() }),
        };
        c.set_brokers(Some(brokers::Brokers::new(vec![m1], &own)));
        let r = post(Some(page));
        assert_eq!((status(&r), header_of(&r, "access-control-allow-origin")), (200, Some(page)), "{r}");
        assert_eq!(status(&post(Some("https://evil.example"))), 403);
    }

    /// `/work/regions` (docs/pool.md §10, Regions) out of the pool, with no key: 200 with `making`
    /// (never an error) until this Mac's agent made them, then theirs.
    #[test]
    fn the_regions_answer_the_page_once_the_agent_made_them() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let post = || raw(&addr, "POST", "/work/regions", &[("Content-Type", "application/json")], b"{}");
        let making = |r: &str| -> Option<String> {
            assert_eq!(status(r), 200, "{r}");
            let a: serde_json::Value = serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            a["making"].as_str().map(str::to_string)
        };
        assert_eq!(making(&post()).as_deref(), Some("this Mac's agent is reading the regions"));
        c.set_regions(Arc::new(serde_json::json!({ "catalog": 4, "regions": [{ "id": "a" }] })));
        let r = post();
        assert_eq!(status(&r), 200, "{r}");
        let a: serde_json::Value = serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!((a["catalog"].as_u64(), a["regions"][0]["id"].as_str()), (Some(4), Some("a")));
        assert_eq!(making(&r), None);
        // The steps: asked (so counted from now), `making` until counted, then theirs alone.
        let steps =
            || raw(&addr, "POST", "/work/regions", &[("Content-Type", "application/json")], br#"{"steps":true}"#);
        assert!(!c.region_steps_asked());
        assert_eq!(making(&steps()).as_deref(), Some("this Mac's agent is counting the regions' steps"));
        assert!(c.region_steps_asked());
        c.set_region_steps(Arc::new(serde_json::json!({ "at": 5, "regions": { "a": { "unit": [1, 3] } } })));
        let r = steps();
        let a: serde_json::Value = serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(a["regions"]["a"]["unit"], serde_json::json!([1, 3]));
    }

    /// `/work/tocome` (docs/pool.md §10, To come), with no key: 200 with `making` until this Mac's
    /// forecast made them, then every Mac's tiles by step.
    #[test]
    fn the_tiles_to_come_answer_the_page_once_a_forecast_made_them() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let post = || raw(&addr, "POST", "/work/tocome", &[("Content-Type", "application/json")], b"{}");
        let r = post();
        assert_eq!(status(&r), 200, "{r}");
        assert!(r.contains(r#""making":"this Mac has made no forecast yet""#), "{r}");
        let placed = [crate::agent::forecast::Placed {
            lane: 0,
            step: "terrain-hi".into(),
            target: "6/33/20".into(),
            from: 160,
            until: 200,
        }];
        c.set_tocome(Arc::new(crate::agent::forecast::ToCome::of(&placed, &["m4".to_string()], 100)));
        let r = post();
        assert_eq!(status(&r), 200, "{r}");
        let a: serde_json::Value = serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(a["macs"]["m4"]["terrain-hi"], serde_json::json!([["6/33/20", 60]]));
        // The history's epoch its `after` is numbered in, the status's.
        assert!(a["epoch"].as_str().unwrap().starts_with("coordinator "), "{a}");
        assert_eq!(a["after"], serde_json::json!([]));
    }

    /// A member's page (docs/pool.md §10, To come): the lead answers `raw` with `after` as it wrote
    /// it and `by`, and the member's own pageview renumbers it in its numbering, with its epoch;
    /// a number it hasn't taken in is dropped, and with no history read, no epoch.
    #[test]
    fn a_members_page_has_the_leads_after_renumbered_by_its_own_pageview() {
        let (d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let post = |b: serde_json::Value| {
            let r =
                raw(&addr, "POST", "/work/tocome", &[("Content-Type", "application/json")], b.to_string().as_bytes());
            serde_json::from_str::<serde_json::Value>(r.split("\r\n\r\n").nth(1).unwrap()).unwrap()
        };
        // The lead's answer, raw: its numbers and the name they're under.
        c.set_member(Some("m-lead"));
        let mut tc = crate::agent::forecast::ToCome::default();
        tc.runs.after = vec![vec![40], vec![7]];
        c.set_tocome(Arc::new(tc));
        let a = post(serde_json::json!({ "raw": true }));
        assert_eq!(
            (a["by"].as_str(), a["epoch"].as_str().map(|e| e.starts_with("coordinator "))),
            (Some("m-lead"), Some(true))
        );
        assert_eq!(a["after"], serde_json::json!([[40], [7]]));
        // This Mac's pageview, before it has read anything: no numbering.
        let root = d.path().join("nas");
        std::fs::create_dir_all(&root).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let pv = crate::agent::pageview::PageView::new();
        pv.set_inputs(crate::agent::pageview::Inputs {
            root: Some(nas.clone()),
            term: 7,
            members: vec!["m-lead".into()],
            ..Default::default()
        });
        pv.touch();
        c.set_pageview(pv.clone());
        let ask = || post(serde_json::json!({ "renumber": { "by": "m-lead", "after": [[40], [7], []] } }));
        assert!(ask()["epoch"].is_null());
        // After it read the lead's history, where job 40 is numbered anew.
        let now = unix_now();
        let day = crate::pool::journal::day(now).unwrap();
        let put = |p: &str, b: &[u8]| {
            let p = nas.join(p);
            p.parent().unwrap().create_dir_all().unwrap();
            p.write(b).unwrap();
        };
        put("state/status.json", br#"{"host":"m4","beat":1,"forecast":null,"helpers":[]}"#);
        let done = crate::coord::history::Event {
            seq: 40,
            t: now - 15,
            step: Some("unit".into()),
            targets: vec!["6/1/1".into()],
            ..crate::coord::history::Event::new("done")
        };
        let ev = serde_json::to_string(&done).unwrap() + "\n";
        put(&format!("state/coord/history/{day}/m-lead.jsonl"), ev.as_bytes());
        assert!(pv.tick(&crate::agent::pageview::NasFiles(nas.clone()), now));
        let a = ask();
        let n = pv.history(0, 10).unwrap()["events"][0]["seq"].as_u64().unwrap();
        assert_ne!(n, 40);
        assert_eq!(a["after"], serde_json::json!([[n], [], []]));
        assert_eq!(a["epoch"], pv.history(0, 10).unwrap()["epoch"]);
    }

    /// (Task #836 review) In the pool, `/work/tocome` is this Mac's pageview's, of the run the
    /// swarm's answer carries: asked with that `at`, that run's tiles, in the pageview's epoch (not
    /// the coordinator's own forecast, set here too); `raw` (an older page asking the lead) still
    /// the coordinator's.
    #[test]
    fn in_the_pool_the_tiles_to_come_are_the_pageviews_run() {
        let (d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let post = |b: serde_json::Value| {
            let r =
                raw(&addr, "POST", "/work/tocome", &[("Content-Type", "application/json")], b.to_string().as_bytes());
            serde_json::from_str::<serde_json::Value>(r.split("\r\n\r\n").nth(1).unwrap()).unwrap()
        };
        c.set_tocome(Arc::new(crate::agent::forecast::ToCome { at: 5, ..Default::default() }));
        let root = d.path().join("nas");
        std::fs::create_dir_all(&root).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let now = unix_now();
        crate::agent::pageview::tests::fixture(&nas, now);
        let pv = crate::agent::pageview::PageView::new();
        pv.set_inputs(crate::agent::pageview::Inputs {
            root: Some(nas.clone()),
            term: 7,
            members: vec!["m-a".into(), "m-b".into()],
            ..Default::default()
        });
        pv.touch();
        c.set_pageview(pv.clone());
        // (Before the pageview's first run: the coordinator's.)
        assert_eq!(post(serde_json::json!({ "at": now }))["at"], 5);
        assert!(pv.tick(&crate::agent::pageview::NasFiles(nas.clone()), now));
        crate::agent::pageview::tests::settle(&pv);
        let sw = pv.swarm().unwrap();
        let at = sw["agent"]["forecast"]["at"].as_u64().unwrap();
        let a = post(serde_json::json!({ "at": at }));
        assert_eq!(a["at"].as_u64(), Some(at), "{a}");
        assert_eq!(a["epoch"], sw["epoch"]);
        assert!(a["macs"].as_object().is_some_and(|m| !m.is_empty()), "{a}");
        assert_eq!(post(serde_json::json!({ "raw": true }))["at"], 5);
    }

    /// The pool's members' pages (docs/pool.md §8): a page from another member's address reads this
    /// Mac with CORS, the preflight answered for `Content-Type` (and a private network's ask), every answer allowed to that page; one left open from before pages
    /// stopped helping (#876) is told so with CORS too, so it reads why. Switched off, the same page
    /// is refused as before, its preflight unanswered.
    #[test]
    fn a_members_page_reads_with_cors_is_told_pages_no_longer_help_and_off_is_refused() {
        let (_d, c, w) = start();
        let addr = w.urls()[0].trim_start_matches("http://").to_string();
        let page = "http://100.93.150.29:8090";
        let preflight = |method: &str, headers: &str| {
            raw(
                &addr,
                "OPTIONS",
                "/work/swarm",
                &[
                    ("Origin", page),
                    ("Access-Control-Request-Method", method),
                    ("Access-Control-Request-Headers", headers),
                    ("Access-Control-Request-Private-Network", "true"),
                ],
                b"",
            )
        };
        let ask = serde_json::json!({ "worker": "ipad", "kind": "web", "can": ["tail"], "mem_mb": 1000, "cores": 4 })
            .to_string();
        let post = |path: &str, body: &str| {
            raw(
                &addr,
                "POST",
                path,
                &[("Origin", page), ("Content-Type", "application/json"), ("X-Worker", "ipad")],
                body.as_bytes(),
            )
        };
        // Off: refused, the preflight not answered for it.
        assert_eq!(status(&post("/work/swarm", "{}")), 403);
        assert_eq!(header_of(&preflight("POST", "content-type"), "access-control-allow-origin"), None);
        // On: the members' addresses told (this Mac's own among them).
        let own = brokers::Broker { urls: c.contact.urls.clone(), ..Default::default() };
        let m1 = brokers::Peer {
            member: "m-1".into(),
            host: "m1".into(),
            lead: true,
            broker: Some(brokers::Broker { urls: vec![page.into()], ..Default::default() }),
        };
        c.set_brokers(Some(brokers::Brokers::new(vec![m1], &own)));
        for (method, headers) in [("POST", "content-type"), ("GET", "content-type")] {
            let r = preflight(method, headers);
            assert_eq!(status(&r), 204, "{r}");
            assert_eq!(header_of(&r, "access-control-allow-origin"), Some(page));
            assert_eq!(header_of(&r, "access-control-allow-methods"), Some(brokers::ALLOW_METHODS));
            assert_eq!(header_of(&r, "access-control-allow-headers"), Some(brokers::ALLOW_HEADERS));
            assert_eq!(header_of(&r, "access-control-allow-private-network"), Some("true"));
        }
        // Another page elsewhere: as before.
        let evil = raw(
            &addr,
            "OPTIONS",
            "/work/swarm",
            &[("Origin", "https://evil.example"), ("Access-Control-Request-Method", "POST")],
            b"",
        );
        assert_eq!(header_of(&evil, "access-control-allow-origin"), None);
        assert_eq!(
            status(&raw(
                &addr,
                "POST",
                "/work/swarm",
                &[("Origin", "https://evil.example"), ("Content-Type", "application/json")],
                b"{}"
            )),
            403
        );
        // The build at a glance: allowed to it.
        let r = post("/work/swarm", "{}");
        assert_eq!((status(&r), header_of(&r, "access-control-allow-origin")), (200, Some(page)), "{r}");
        // No one to find work at (pages stopped helping, #876): `/work/where` is gone.
        assert_eq!(status(&post("/work/where", "{}")), 403);
        // An old page asking for work: told why, and allowed to read it.
        let r = post("/work/ask", &ask);
        assert_eq!((status(&r), header_of(&r, "access-control-allow-origin")), (403, Some(page)), "{r}");
        assert!(r.contains(NO_PAGE_HELPERS), "{r}");
        // Off again: refused as before.
        c.set_brokers(None);
        assert_eq!(status(&post("/work/swarm", "{}")), 403);
    }

    /// A broker that takes the lead keeps its own tasks' leases as it takes up the term's jobs'
    /// (docs/pool.md §8: a handover drops no task); switched off, the term's leases replace them,
    /// as before. A lead's coordinator that becomes a broker leaves the jobs' leases and offers to
    /// the new lead.
    #[test]
    fn a_broker_taking_the_lead_keeps_its_tasks_leases_and_a_lead_becoming_one_leaves_its_jobs() {
        for on in [true, false] {
            let (d, c, w) = start();
            if on {
                c.set_brokers(Some(brokers::Brokers::default()));
            }
            let root = d.path().join("job");
            std::fs::create_dir_all(&root).unwrap();
            let job = client::Client::at(w.urls().to_vec(), c.job_token.clone(), "job");
            let offer = task::Offer {
                owner: 42,
                kind: "tail".into(),
                spec: serde_json::json!({ "unit": "6/1/1" }),
                root,
                inputs: Default::default(),
                mem_mb: 100,
                ahead: None,
            };
            job.post_json("/task/offer", &serde_json::to_value(&offer).unwrap()).unwrap();
            let page = client::Client::at(w.urls().to_vec(), c.contact.token.clone(), "ipad");
            let g = page
                .ask(&Ask {
                    worker: "ipad".into(),
                    kind: "native".into(),
                    can: vec!["tail".into()],
                    mem_mb: 1000,
                    cores: 4,
                    ..Default::default()
                })
                .unwrap()
                .unwrap();
            // Another lead's state: a job's lease.
            let other = {
                let mut l = lease::Leases::new(TTL);
                l.grant(
                    "m1",
                    Work::Job { step: "slope-hi".into(), targets: vec![("3/2/2".into(), "k".into())] },
                    Instant::now(),
                );
                PoolState { leases: l.snapshot(), ..Default::default() }
            };
            assert_eq!(c.load_pool_state(&other).unwrap(), 1);
            assert_eq!(page.beat(g.lease, None).unwrap(), on, "its task's lease kept only with the switch on");
            assert_eq!(c.held("slope-hi").len(), 1);
            // No longer leading: a broker, its job lease the new lead's.
            c.become_broker();
            assert!(c.held("slope-hi").is_empty());
            assert_eq!(c.leases_out(), usize::from(on));
        }
    }
}
