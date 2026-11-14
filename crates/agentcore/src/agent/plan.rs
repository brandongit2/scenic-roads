//! **plan**, the lead's (docs/agent-parts.md §4.4, task #871 step 6): the plan, its offers to this
//! Mac's coordinator, the gate, the redos, the checklist and the regions' counts, on a thread of
//! its own (`Handle::spawn`) owning all of it (`Planner`). It plans when something it depends on
//! changes, not each pass of a loop (`why_plan`): a job's end or a start held briefly (told by
//! **jobs**, `Msg::Kick`), the records' digest (`Planner::plan_seen`, looked at every `LOOK`), a
//! gate ask, a timed wait in the plan ending (`Planner::replan_at`), or `FALLBACK` at most. Each
//! plan is published whole (`PlanView`, numbered `PlanView::n`); **jobs** grants from the last one
//! at once (`Agent::lead_grants`), each grant carrying that number (`coord::Grant::plan`), so a
//! plan of minutes never holds a grant, a pause or the heartbeat. A plan running past
//! `beat::LOOP_STUCK` is logged and the process sampled once (`Stall`, decision 4 of the doc).
//!
//! What it reads of **jobs**' side comes with each round's `Inputs` (the slots, the memory, the
//! pause, the pool's view, the switches): a copy, never a lock. In line (a single loop's, a dry
//! run's, the tests'), the round runs it in its place, a plan as before.

use super::*;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;

/// How often the plan's thread looks at what it depends on (the records' digest, the gate's asks,
/// the forecast's and the regions' steps' threads), when nothing wakes it sooner.
pub(super) const LOOK: Duration = Duration::from_secs(5);
/// The plan made again at least this often, whatever changed: the fallback for what the digest
/// doesn't cover (time, disk room, a planet newly fetched).
pub(super) const FALLBACK: Duration = Duration::from_secs(300);
/// The regions' steps counted again, while a page asks, at least this often.
const STEPS_EVERY: Duration = Duration::from_secs(60);

/// What **jobs** tells **plan**.
pub(super) enum Msg {
    /// Its side as of this round (the newest kept: a burst coalesces).
    Inputs(Box<Inputs>),
    /// A catalogue made (publish::LastCatalogue::held_at).
    CatalogueMade,
    /// A job done: a gate check or forget it asked for done.
    Done(String),
    Stop,
}

/// **jobs**' side as the plan reads it: copied each round.
#[derive(Clone, Default)]
pub(super) struct Inputs {
    pub root: Option<NasRoot>,
    /// The members' beats this Mac hears by HTTP (crate::coord::beats): their statuses, the machines.
    pub beats: Option<std::sync::Arc<crate::coord::beats::Beats>>,
    pub conditions: Conditions,
    pub mac: cond::Mac,
    pub mem: Memory,
    pub slots: Vec<SlotSeen>,
    pub free_set: Option<u64>,
    pub room_floor: u64,
    /// The most this Mac's jobs could hold above its memory floor (MB: `Agent::jobs_ceiling`).
    pub jobs_ceiling: u64,
    pub build_room: u64,
    pub guard_backoff: BTreeMap<String, (u64, String)>,
    pub floor_short: Option<(u64, u64, Instant)>,
    pub proxy_up: bool,
    pub pause: Option<jobrt::control::Pause>,
    pub pool_seen: Option<PoolView>,
    pub member_id: Option<String>,
    pub placement: bool,
    /// Publishing's switches, as the pool's settings say (crate::agent::publish::Switches).
    pub switches: publish::Switches,
    pub pooled: bool,
    pub resume_now: u64,
    pub ramp_held: bool,
    pub ramp_trouble: Option<String>,
    /// This Mac's coordinator as the round has it (made or stopped as its part changes).
    pub coord: Option<std::sync::Arc<crate::coord::Coordinator>>,
    /// Whether a job ended since the last round (a plan again, the regions' counts with it).
    pub ended: bool,
    /// The lead's map, its log's (crate::agent::publish::Here): None until its thread starts.
    pub map: Option<std::sync::Arc<publish::Here>>,
    /// Their number, counted by **jobs** as it makes them (`Agent::told`): a plan made from them
    /// says so (`PlanView::from`).
    pub told: u64,
}

/// A slot as the plan reads it (`Inputs::slots`): its job, its lane's time left, what it waits on.
#[derive(Clone, Default)]
pub(super) struct SlotSeen {
    pub running: Option<RunSeen>,
    pub job: Option<forecast::SlotJob>,
    pub waiting: Option<String>,
}

/// A slot's running job as the plan reads it.
#[derive(Clone)]
pub(super) struct RunSeen {
    pub spec: JobSpec,
}

/// The last plan, and what the plan's thread keeps for the status: published whole after each
/// look (`Handle::view`).
#[derive(Clone, Default)]
pub(super) struct PlanView {
    /// The plan's number: each grant made from it carries it (0: none made yet).
    pub n: u64,
    /// The number of **jobs**' side it was made from (`Inputs::told`): a job of the lead's own that
    /// ended after those were made isn't granted from it (`Agent::own_ended`).
    pub from: u64,
    /// The jobs it planned, in its order: the lead's own granted from it (`Agent::lead_grants`).
    pub jobs: Vec<JobSpec>,
    /// Why its work waits, as it found it.
    pub waiting: Vec<Waiting>,
    pub regions: Vec<recipes::Recipe>,
    pub bad: Vec<(String, String)>,
    pub checklist: Vec<progress::Line>,
    pub built: BTreeMap<String, progress::RegionState>,
    pub forecast: Option<forecast::Forecast>,
    pub publishing: Option<publish::PublishLine>,
    pub timetables: Option<inputs::inputs::gtfs::View>,
    pub catalogue: Option<CatalogueSeen>,
    pub published: Option<Published>,
    pub inputs: Vec<inputs::inputs::view::InputView>,
    pub upcoming: Option<(u64, std::sync::Arc<room::Hints>)>,
    pub queued: Vec<jobs::Work>,
    pub ready: Vec<String>,
    pub regions_view: Option<(RegionsKey, std::sync::Arc<serde_json::Value>)>,
    /// Whether the forecast's thread runs (a NAS user: the self-heal waits for it).
    pub forecasting: bool,
    /// Whether the catalogue last planned has the low zooms made (seam::Planned::low): its batch says
    /// so (crate::agent::publish::LOW) once it's published.
    pub catalogue_low: bool,
}

/// What the plan hands **jobs** once each (the lead's pool step writes them to the NAS).
pub(super) enum Out {
    /// The forecast's input as data, and its archive entry.
    Forecast(Option<Vec<u8>>, fcarchive::Entry),
}

/// A plan running, as **jobs** watches it: since when (0: none), and the process sampled once
/// past `beat::LOOP_STUCK`.
#[derive(Default)]
pub(super) struct Stall {
    since: AtomicU64,
    said: AtomicU64,
}

/// **plan**'s handle, **jobs**'s side: its thread, or the planner in line.
pub(super) struct Handle {
    tx: Option<mpsc::Sender<Msg>>,
    pub view: std::sync::Arc<bus::Snap<PlanView>>,
    pub out: mpsc::Receiver<Out>,
    stall: std::sync::Arc<Stall>,
    phase: std::sync::Arc<bus::Snap<&'static str>>,
    running_n: std::sync::Arc<AtomicU64>,
    inline: Option<Box<Planner>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    /// The plan in line (`round` runs it in its place); `spawn` puts it on its thread.
    pub fn new(mut p: Planner) -> Handle {
        let (view, out, phase, running_n) = (p.view.clone(), p.out_rx.take(), p.phase.clone(), p.running_n.clone());
        let stall = std::sync::Arc::new(Stall::default());
        Handle {
            tx: None,
            view,
            out: out.expect("a planner's out"),
            stall,
            phase,
            running_n,
            inline: Some(Box::new(p)),
            thread: None,
        }
    }

    /// A handle with no planner (a member, which plans nothing): its view stays empty.
    pub fn none() -> Handle {
        let (_, out) = mpsc::channel();
        let phase = bus::Snap::new("");
        Handle {
            tx: None,
            view: bus::Snap::new(PlanView::default()),
            out,
            stall: std::sync::Arc::new(Stall::default()),
            phase,
            running_n: std::sync::Arc::new(AtomicU64::new(0)),
            inline: None,
            thread: None,
        }
    }

    /// Its thread started (the agent's real run): it plans on its own from then on.
    pub fn spawn(&mut self) {
        let Some(mut p) = self.inline.take() else { return };
        let (tx, rx) = mpsc::channel();
        let stall = self.stall.clone();
        let started = std::thread::Builder::new().name("plan".into()).spawn(move || p.run(rx, &stall));
        match started {
            Ok(h) => {
                self.tx = Some(tx);
                self.thread = Some(h);
            }
            Err(e) => eprintln!("agent: the plan's thread: {e}"),
        }
    }

    /// Whether it plans at all (the lead, a dry run; not a member).
    pub fn plans(&self) -> bool {
        self.inline.is_some() || self.tx.is_some()
    }

    /// The planner in line (None once its thread runs, or for a member).
    #[cfg(test)]
    pub fn inline(&mut self) -> Option<&mut Planner> {
        self.inline.as_deref_mut()
    }

    /// `m` told: to its thread, or taken up in line at once.
    pub fn send(&mut self, m: Msg) {
        if let Some(tx) = &self.tx {
            tx.send(m).ok();
        } else if let Some(p) = self.inline.as_deref_mut() {
            p.take(m);
        }
    }

    /// In line, the plan's look made now (a plan when due); with its thread, nothing (it looks on
    /// its own).
    pub fn look_inline(&mut self) {
        if let Some(p) = self.inline.as_deref_mut() {
            p.look();
        }
    }

    /// What the heartbeat reads of a plan running (crate::agent::beat::Beat::compose): since when
    /// (s since the epoch) and its phase, None while none runs.
    pub fn running(&self) -> beat::PlanRunning {
        let (stall, phase) = (self.stall.clone(), self.phase.clone());
        std::sync::Arc::new(move || {
            let since = stall.since.load(Ordering::SeqCst);
            (since > 0).then(|| (since, *phase.get()))
        })
    }

    /// The newest plan's number, the one running included (a cancel drops grants from plans no
    /// newer: a plan started before it may still offer what it cancelled).
    pub fn newest_n(&self) -> u64 {
        self.running_n.load(Ordering::SeqCst).max(self.view.get().n)
    }

    /// Whether a plan runs now (a NAS user: the self-heal waits for it).
    pub fn busy(&self) -> bool {
        self.stall.since.load(Ordering::SeqCst) > 0
    }

    /// A plan running past `beat::LOOP_STUCK`: logged with its phase and the process sampled,
    /// once a plan (decision 4); its words for the status while it lasts.
    pub fn stalled(&self, home: &Path) -> Option<String> {
        let since = self.stall.since.load(Ordering::SeqCst);
        let now = now_s();
        if since == 0 || now.saturating_sub(since) < beat::LOOP_STUCK.as_secs() {
            return None;
        }
        let phase = *self.phase.get();
        if self.stall.said.swap(since, Ordering::SeqCst) != since {
            eprintln!(
                "agent: {} the plan has run {} s, in \"{phase}\"; grants go on from the last plan",
                nasread::clock(now),
                now - since
            );
            if !buildstore::out::testing() {
                sample_self(Path::new("/usr/bin/sample"), home, now);
            }
        }
        Some(format!(
            "planning has taken {} min so far (in \"{phase}\"): grants go on from the last plan",
            (now - since) / 60
        ))
    }

    /// The thread told to stop, and waited for `wait` at most (a plan on a hung share may not end).
    pub fn stop(&mut self, wait: Duration) {
        if let Some(tx) = self.tx.take() {
            tx.send(Msg::Stop).ok();
        }
        if let Some(h) = self.thread.take() {
            let until = Instant::now() + wait;
            while !h.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
            if !h.is_finished() {
                eprintln!("agent: the plan hasn't ended in {} s; left", wait.as_secs());
            }
        }
    }
}

/// The next plan's number after `last`: past it, and at least the time in ms, so a lead started
/// again (or another Mac taking the lead) numbers past every plan before it, and a member's cancel
/// noted at an old lead's number never drops a new lead's grants (`Agent::cancelled_since`).
pub(crate) fn next_n(last: u64, now: std::time::SystemTime) -> u64 {
    let ms = now.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    (last + 1).max(ms)
}

/// Why a plan is made now, if one is: kicked (`kick`: a job ended, a start held briefly, a gate
/// ask), none yet, what it depends on changed (`seen` against the last plan's), its timed waits'
/// earliest end (`at`) came, or `FALLBACK` passed since the last (`age`).
///
/// But a change comes no sooner after the last plan ended than that plan took (`took`): on a busy
/// pool, jobs end faster than a plan is made (10-18 s on the Mac mini, 30 Oct), and planning on
/// each would keep a core of the lead's busy; so the plan takes half a core at most, the changes
/// meanwhile taken together (a kick is kept until then: `Planner::look`).
pub(super) fn why_plan(
    kick: Option<&'static str>,
    last: Option<(Duration, Duration, u64, Option<u64>)>,
    seen: u64,
    now: u64,
) -> Option<&'static str> {
    let Some((age, took, last_seen, at)) = last else { return Some("no plan yet") };
    if age >= FALLBACK {
        return Some("the fallback");
    }
    if age < took {
        return None;
    }
    if let Some(k) = kick {
        return Some(k);
    }
    if last_seen != seen {
        return Some("what it depends on changed");
    }
    at.is_some_and(|t| now >= t).then_some("a timed wait ended")
}

/// Whether the last plan's work is filtered again ("its jobs", not `build::plan`: docs/incremental-
/// planning.md step 2), when no plan is due (`why_plan`): the coordinator's leases changed (a grant,
/// a lapse, a lease given back: they change what is offered, not what is stale, so they are not in
/// the plan's digest) and a `build::plan` result is kept (`kept`); no sooner after the last plan, or
/// the last such filtering, ended (`last`, `refiltered`: their age and how long they took) than it
/// took, as `why_plan` holds plans to half a core.
pub(super) fn refilter_due(
    leases_changed: bool,
    kept: bool,
    last: Option<(Duration, Duration, u64, Option<u64>)>,
    refiltered: Option<(Duration, Duration)>,
) -> bool {
    leases_changed
        && kept
        && last.is_some_and(|(age, took, ..)| age >= took)
        && refiltered.is_none_or(|(age, took)| age >= took)
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

/// The plan's state, owned by its thread (or the round, in line).
pub(super) struct Planner {
    pub(super) o: Options,
    pub(super) host: String,
    pub(super) app: String,
    pub(super) started: u64,
    pub(super) coord: Option<std::sync::Arc<crate::coord::Coordinator>>,
    /// Whether this agent runs the jobs (holds the agent's lock): it alone writes what they read.
    pub(super) locked: bool,
    /// **jobs**' side as of its last round.
    pub(super) inp: Inputs,
    pub(super) hold: std::sync::Arc<bus::Snap<jobrt::control::HoldState>>,
    pub(super) view: std::sync::Arc<bus::Snap<PlanView>>,
    pub(super) out: mpsc::Sender<Out>,
    pub(super) out_rx: Option<mpsc::Receiver<Out>>,
    pub(super) phase: std::sync::Arc<bus::Snap<&'static str>>,
    /// Plan again now, and why (`why_plan`).
    pub(super) kicked: Option<&'static str>,
    /// The last plan's number (`next_n`), when it was made, its digest, its timed waits' end and
    /// why its work waits.
    pub(super) n: u64,
    /// The number of **jobs**' side the last plan was made from (`PlanView::from`).
    pub(super) from: u64,
    /// The number of the plan running now, or the last (**jobs**' cancel reads it:
    /// `Handle::newest_n`).
    pub(super) running_n: std::sync::Arc<AtomicU64>,
    /// (When it ended, how long it took, its digest and its timed waits' end.)
    pub(super) last: Option<(Instant, Duration, u64, Option<u64>)>,
    /// The coordinator's leases' generation as the last plan, or its jobs filtered again, read
    /// it; when its jobs were last filtered again and how long that took (`refilter_due`).
    pub(super) leases: Option<u64>,
    pub(super) refiltered: Option<(Instant, Duration)>,
    /// What the planner carries for the plan (seam::PlanHeld): the terrain packs' indexes, the
    /// pass's reaches, the last plan with its keys.
    pub(super) held_plan: Box<dyn seam::PlanHeld>,
    pub(super) refilter: std::cell::Cell<bool>,
    pub(super) jobs: Vec<JobSpec>,
    pub(super) waiting: Vec<Waiting>,
    pub(super) regions: (Vec<recipes::Recipe>, Vec<(String, String)>),
    /// The checklist, made with each plan (`checklist`); the regions' steps' part of it as the plan
    /// found them (seam::Planned::checklist: None while no plan got that far); how far each region
    /// is built, as the last plan that got that far counted (seam::Planned::built); the regions'
    /// steps for the page, when that count was last started and the count under way.
    pub(super) progress: Vec<progress::Line>,
    pub(super) regions_checklist: std::cell::RefCell<Option<progress::Checklist>>,
    pub(super) built: std::cell::RefCell<BTreeMap<String, progress::RegionState>>,
    pub(super) steps_counted: Option<Instant>,
    #[allow(clippy::type_complexity)]
    pub(super) steps_making:
        Option<std::thread::JoinHandle<(BTreeMap<String, BTreeMap<&'static str, [usize; 2]>>, Duration)>>,
    /// The slow-plan fake (docs/agent-parts.md §7): each plan sleeps this long first. Tests only.
    #[cfg(test)]
    pub(super) slow_plan: Option<Duration>,
    /// The offered steps' work as the regions' planning last offered it (`region_jobs`), and its
    /// pass: offered with the plan, in its order (`offer_plan`).
    pub(super) offered: std::cell::RefCell<(String, Vec<crate::coord::Offer>)>,
    /// The depth's targets as the last plan found them, for GC (`build::depth_kept`; None: not
    /// found then, and no depth pack is dropped).
    pub(super) depth_keep: std::cell::RefCell<Option<gc::DepthKeep>>,
    /// The coverage and each region's as last made, with what they were made from (`coverage`).
    pub(super) coverage: std::cell::RefCell<Option<(String, std::sync::Arc<Coverages>)>>,
    /// The coverage being read off the plan (`coverage`), by the accepted version it's of.
    pub(super) coverage_making:
        std::cell::RefCell<Option<(String, std::thread::JoinHandle<Result<Coverages, String>>)>>,
    /// The regions' accepted version as last seen, and when it changed while this agent ran
    /// (an edit: `EDIT_HOLD`): first in a run of edits, and last.
    pub(super) edits: std::cell::RefCell<Option<String>>,
    pub(super) edited_at: std::cell::Cell<Option<(std::time::SystemTime, std::time::SystemTime)>>,
    /// The OSM extracts' sizes by content name (the coordinator sizes z6 tiles by them; content-named
    /// files never change).
    pub(super) piece_sizes: std::cell::RefCell<std::collections::HashMap<String, u64>>,
    /// The work the last plan had left (its steps and targets): what room-making lets go last
    /// (`Agent::hints`).
    pub(super) queued: std::cell::RefCell<Vec<jobs::Work>>,
    /// The last map's folder and number (the map's, or the newest held: `on_map`), its regions'
    /// outline entries by id and its time (unix seconds).
    #[allow(clippy::type_complexity)]
    pub(super) last_catalogue: std::cell::RefCell<Option<((String, u64), (BTreeMap<String, Vec<String>>, u64))>>,
    /// The regions the last plan would publish as built (seam::Planned::ready), for the checklist.
    pub(super) ready: std::cell::RefCell<Vec<String>>,
    /// The last plan's forecast (crate::agent::forecast), for the status.
    pub(super) forecast: std::cell::RefCell<Option<forecast::Forecast>>,
    /// When each (Mac, step) last had its grants' departure from the forecast, or its many grants of
    /// a step the forecast doesn't model, logged (`forecast_record`): once an hour at most.
    pub(super) departed: std::cell::RefCell<BTreeMap<(String, String), u64>>,
    /// The forecasts' runs of the last `forecast::PACE_MAX_S`, one each `forecast::CURVE_S` at most
    /// (`Forecast::at`, `Forecast::curve`): what the next is held to (`forecast::pace`).
    pub(super) curves: std::cell::RefCell<Vec<(u64, Vec<u32>)>>,
    /// The costs the forecasts gave each target while it was work left (`forecast::Noted`): what
    /// each step's pace is held to (`forecast::step_pace`).
    pub(super) noted: std::cell::RefCell<forecast::Noted>,
    /// The build page's regions as last made (crate::agent::regionsview), by what they were made of;
    /// what they were last tried for (each once, failed or not); the last catalogue's sizes, by its
    /// number; and the making under way, off the plan.
    pub(super) regions_view: std::cell::RefCell<Option<(RegionsKey, std::sync::Arc<serde_json::Value>)>>,
    pub(super) regions_tried: std::cell::RefCell<Option<RegionsKey>>,
    pub(super) region_bytes: std::cell::RefCell<Option<(u64, BTreeMap<String, u64>)>>,
    pub(super) regions_making: std::cell::RefCell<Option<std::thread::JoinHandle<regionsview::Made>>>,
    /// The forecast being run off the plan (its own thread, `start_forecast`): its answer, its
    /// record's bytes and how long it took, taken in once it's done (`take_forecast`).
    pub(super) forecasting: std::cell::RefCell<Option<std::thread::JoinHandle<Forecasted>>>,
    /// This Mac's tiles to come in its last forecast, as cache hints (room::Hints::of_tocome,
    /// task #843), with that forecast's time.
    pub(super) upcoming: std::cell::RefCell<Option<(u64, std::sync::Arc<room::Hints>)>>,
    /// Publishing as last planned (publish::PublishLine), and the key of the last line logged (logged
    /// again only when it changes).
    pub(super) publish_line: std::cell::RefCell<Option<publish::PublishLine>>,
    /// The timetables' report as last read (its content name, and what the status shows).
    pub(super) timetables: std::cell::RefCell<Option<(String, inputs::inputs::gtfs::View)>>,
    pub(super) publish_logged: std::cell::RefCell<String>,
    /// The map's `seq` and when its batch went out (`map_now`), for the status.
    pub(super) catalogue_seen: std::cell::Cell<Option<CatalogueSeen>>,
    /// The lead's record of its catalogues (publish::LastCatalogue, its folder's `publishing.json`):
    /// when one was first held back, and the gated steps' runs.
    pub(super) catalogues: std::cell::RefCell<publish::LastCatalogue>,
    /// Whether the catalogue last planned had the low zooms made (seam::Planned::low, nothing left of
    /// the publishing chain): its batch, once made, says so (`PlanView::catalogue_low`).
    pub(super) catalogue_low: std::cell::Cell<bool>,
    /// Since when a catalogue has been due (publish::PublishLine::due_since).
    pub(super) catalogue_due_since: std::cell::Cell<Option<u64>>,
    /// The map tiles noted planned in the history (crate::pubtimes), since the last catalogue.
    pub(super) planned_tiles: std::cell::RefCell<crate::pubtimes::PlannedNotes>,
    /// The gate (inputs::inputs): the inputs folders' listing thread, the status's entries as the last
    /// plan made them, what they read, and the units a full check was asked of.
    pub(super) inputs: Gate,
}

impl Planner {
    /// The plan of an agent with `o`, its coordinator `coord`, this Mac's hold as control holds it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        o: Options,
        host: String,
        app: String,
        started: u64,
        coord: Option<std::sync::Arc<crate::coord::Coordinator>>,
        locked: bool,
        hold: std::sync::Arc<bus::Snap<jobrt::control::HoldState>>,
        catalogues: publish::LastCatalogue,
    ) -> Planner {
        let (out, out_rx) = mpsc::channel();
        let held_plan = o.the_plan.held(o.home.join("pack-idx"));
        Planner {
            o,
            host,
            app,
            started,
            coord,
            locked,
            inp: Inputs::default(),
            hold,
            view: bus::Snap::new(PlanView::default()),
            out,
            out_rx: Some(out_rx),
            phase: bus::Snap::new(""),
            kicked: None,
            n: 0,
            from: 0,
            running_n: std::sync::Arc::new(AtomicU64::new(0)),
            last: None,
            jobs: Vec::new(),
            waiting: Vec::new(),
            regions: Default::default(),
            progress: Vec::new(),
            regions_checklist: Default::default(),
            built: Default::default(),
            leases: None,
            refiltered: None,
            held_plan,
            refilter: Default::default(),
            steps_counted: None,
            steps_making: None,
            #[cfg(test)]
            slow_plan: None,
            offered: Default::default(),
            depth_keep: Default::default(),
            coverage: Default::default(),
            coverage_making: Default::default(),
            edits: Default::default(),
            edited_at: Default::default(),
            piece_sizes: Default::default(),
            queued: Default::default(),
            last_catalogue: Default::default(),
            ready: Default::default(),
            forecast: Default::default(),
            departed: Default::default(),
            curves: Default::default(),
            noted: Default::default(),
            regions_view: Default::default(),
            regions_tried: Default::default(),
            region_bytes: Default::default(),
            regions_making: Default::default(),
            forecasting: Default::default(),
            upcoming: Default::default(),
            publish_line: Default::default(),
            timetables: Default::default(),
            publish_logged: Default::default(),
            catalogue_seen: Default::default(),
            catalogues: std::cell::RefCell::new(catalogues),
            catalogue_low: Default::default(),
            catalogue_due_since: Default::default(),
            planned_tiles: Default::default(),
            inputs: Gate::default(),
        }
    }

    /// The plan's thread: each message taken up as it comes (a burst coalesced), a look made after
    /// it and every `LOOK` (`look`), until told to stop.
    pub(super) fn run(&mut self, rx: mpsc::Receiver<Msg>, stall: &Stall) {
        loop {
            match rx.recv_timeout(LOOK) {
                Ok(m) => {
                    if !self.take(m) {
                        return;
                    }
                    while let Ok(m) = rx.try_recv() {
                        if !self.take(m) {
                            return;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            stall.since.store(now_s(), Ordering::SeqCst);
            self.look();
            stall.since.store(0, Ordering::SeqCst);
        }
    }

    /// Message `m` taken up; false for `Stop`.
    pub(super) fn take(&mut self, m: Msg) -> bool {
        match m {
            Msg::Inputs(i) => {
                if i.ended {
                    self.kicked.get_or_insert("a job ended");
                    // (The regions' steps counted again once the count under way is in: the job's
                    // targets weren't in it.)
                    self.steps_counted = None;
                }
                self.coord = i.coord.clone();
                self.inp = *i;
            }
            Msg::CatalogueMade => self.catalogue_made(),
            Msg::Done(id) => {
                // (A full check asked for, done; a forget too.)
                if let Some(unit) = id.strip_prefix("inputs ").and_then(|r| r.strip_suffix(" full")) {
                    self.inputs.full.borrow_mut().remove(unit);
                }
                if let Some(unit) = id.strip_prefix("inputs ").and_then(|r| r.strip_suffix(" forget")) {
                    self.inputs.forget.borrow_mut().remove(unit);
                }
            }
            Msg::Stop => return false,
        }
        true
    }

    /// The phase it's in, for a stall's log.
    pub(super) fn phase(&self, name: &'static str) {
        self.phase.set(name);
    }

    /// A look: the forecast and the build page's regions taken in when done, the gate and the redo
    /// asks taken up, a plan made when due (`why_plan`) and the checklist and the regions' counts
    /// with it, the regions' steps when due, and the view published.
    pub(super) fn look(&mut self) {
        let Some(root) = self.inp.root.clone() else {
            // (The NAS back: planned at once, whatever the digest.)
            if let Some(l) = self.last.as_mut() {
                l.2 = 0;
            }
            self.publish();
            return;
        };
        self.phase("the forecast and regions taken in");
        self.take_forecast();
        self.take_regions();
        self.phase("the gate");
        self.tend_gate(&root);
        self.tend_redo();
        self.phase("the records' digest");
        let seen = self.plan_seen(&root);
        let leases = self.coord.as_ref().map(|c| c.lease_gen());
        let now = now_s();
        let last = self.last.map(|(at, took, s, t)| (at.elapsed(), took, s, t));
        let refiltered = self.refiltered.map(|(at, took)| (at.elapsed(), took));
        if let Some(why) = why_plan(self.kicked, last, seen, now) {
            self.kicked = None;
            self.leases = leases;
            self.phase("the plan");
            let t = Instant::now();
            let mut waiting = Vec::new();
            let c = self.inp.conditions;
            // (Numbered as it starts: a cancel taken meanwhile drops its grants too.)
            let n = next_n(self.n, std::time::SystemTime::now());
            self.running_n.store(n, Ordering::SeqCst);
            if let Some(c) = &self.coord {
                c.plan_started(n);
            }
            let plan = self.plan(&root, &c, &mut waiting);
            // (Its last phase ended with it, however it returned.)
            nasread::PLAN.enter("");
            (self.n, self.from) = (n, self.inp.told);
            self.offer_plan(&plan);
            self.regions = load_regions(&root);
            let replan_at = self.replan_at(&plan, now_s());
            self.last = Some((Instant::now(), t.elapsed(), seen, replan_at));
            if t.elapsed() >= Duration::from_secs(10) {
                eprintln!("agent: plan {} ({why}) took {} s", self.n, t.elapsed().as_secs());
            }
            self.jobs = plan;
            self.waiting = waiting;
            // (A new plan: the checklist with it, its regions' steps the plan's.)
            self.phase("the checklist");
            let regions = self.regions.0.clone();
            self.progress = self.checklist(&root, &regions);
        } else if refilter_due(leases != self.leases, self.held_plan.has_kept(), last, refiltered) {
            // (The leases changed alone: the last plan's work filtered again and offered, its digest
            // kept. Numbered as a plan is: its jobs are new specs, and a grant of one by its id must
            // find the spec of the offer it was granted from, never the last plan's of the same id
            // with other targets (task #923); a cancel taken meanwhile drops its grants too.)
            self.leases = leases;
            self.phase("its jobs again");
            let t = Instant::now();
            let mut waiting = Vec::new();
            let c = self.inp.conditions;
            let n = next_n(self.n, std::time::SystemTime::now());
            self.running_n.store(n, Ordering::SeqCst);
            if let Some(c) = &self.coord {
                c.plan_started(n);
            }
            self.refilter.set(true);
            let plan = self.plan(&root, &c, &mut waiting);
            self.refilter.set(false);
            nasread::PLAN.enter("");
            (self.n, self.from) = (n, self.inp.told);
            self.offer_plan(&plan);
            self.refiltered = Some((Instant::now(), t.elapsed()));
            eprintln!(
                "agent: plan {n}: the last plan's work filtered again (the leases changed) in {} ms",
                t.elapsed().as_millis()
            );
            self.jobs = plan;
            self.waiting = waiting;
        }
        self.phase("the regions' steps");
        self.count_steps(&root);
        self.publish();
        self.phase("");
    }

    /// Each region's steps for the build page (while a page asks, every `STEPS_EVERY`, on a thread of
    /// its own). (How far each region is built is the plan's: seam::Planned::built.)
    pub(super) fn count_steps(&mut self, root: &NasRoot) {
        let Some(c) = self.coord.clone() else { return };
        if self.steps_making.as_ref().is_some_and(|h| h.is_finished()) {
            match self.steps_making.take().map(|h| h.join()) {
                Some(Ok((s, took))) => {
                    eprintln!("agent: the regions' steps took {} ms", took.as_millis());
                    c.set_region_steps(std::sync::Arc::new(serde_json::json!({ "at": now_s(), "regions": s })));
                }
                _ => eprintln!("agent: counting the regions' steps failed"),
            }
        }
        if self.steps_making.is_none()
            && c.region_steps_asked()
            && self.steps_counted.is_none_or(|t| t.elapsed() >= STEPS_EVERY)
        {
            self.steps_counted = Some(Instant::now());
            self.steps_making = self.start_region_steps(root);
        }
    }

    /// The view published whole (`PlanView`).
    pub(super) fn publish(&mut self) {
        let published = Some(self.published());
        let view = PlanView {
            n: self.n,
            from: self.from,
            jobs: self.jobs.clone(),
            waiting: self.waiting.clone(),
            regions: self.regions.0.clone(),
            bad: self.regions.1.clone(),
            checklist: self.progress.clone(),
            built: self.built.borrow().clone(),
            forecast: self.forecast.borrow().clone(),
            publishing: self.publish_line.borrow().clone(),
            timetables: self.timetables.borrow().as_ref().map(|t| t.1.clone()),
            catalogue: self.catalogue_seen.get(),
            published,
            inputs: self.inputs.view.borrow().clone(),
            upcoming: self.upcoming.borrow().clone(),
            queued: self.queued.borrow().clone(),
            ready: self.ready.borrow().clone(),
            regions_view: self.regions_view.borrow().clone(),
            forecasting: self.forecasting.borrow().as_ref().is_some_and(|h| !h.is_finished()),
            catalogue_low: self.catalogue_low.get(),
        };
        self.view.set(view);
    }

    /// This Mac's hold, as control holds it now.
    pub(super) fn hold(&self) -> std::sync::Arc<jobrt::control::HoldState> {
        self.hold.get()
    }

    pub(super) fn worker_of(&self, k: usize) -> String {
        slot_worker(&self.host, k)
    }

    /// Slot `k`'s job under way, as its lane's time left is worked out, as **jobs** last said.
    pub(super) fn slot_job(&self, k: usize) -> Option<forecast::SlotJob> {
        self.inp.slots.get(k)?.job.clone()
    }

    /// What slot `k`'s job waits on, in words, as **jobs** last said.
    pub(super) fn waiting_words(&self, k: usize) -> Option<String> {
        self.inp.slots.get(k)?.waiting.clone()
    }

    pub(super) fn disk_free(&self) -> u64 {
        self.inp.free_set.unwrap_or_else(|| room::disk_free(&self.o.home).unwrap_or(0))
    }

    pub(super) fn floor(&self) -> u64 {
        self.inp.room_floor
    }

    pub(super) fn floor_words(&self) -> String {
        floor_words(self.floor())
    }

    pub(super) fn jobs_ceiling(&self) -> u64 {
        self.inp.jobs_ceiling
    }

    pub(super) fn build_room(&self) -> u64 {
        self.inp.build_room
    }

    pub(super) fn job_slots(&self) -> usize {
        lead_slots(false, self.inp.ramp_held, &self.inp.mem.ramp)
    }

    pub(super) fn resume_on(&self) -> bool {
        self.inp.pooled
    }

    pub(super) fn latest_pass(&self, root: &NasRoot) -> Option<String> {
        inputs::osmpass::latest_pass_by(root, self.resume_on())
    }

    /// The declared planet's date and next step (inputs::planet::next), the newest complete pass's
    /// date `have`: None when its pass is complete, or none is declared or can be read now (said in
    /// `waiting`, as a declared planet older than the last pass is).
    pub(super) fn planet_next(
        &self,
        root: &NasRoot,
        have: Option<&str>,
        waiting: &mut Vec<Waiting>,
    ) -> Option<(String, inputs::planet::Next)> {
        let wait =
            |why: String| Waiting { step: Some("osm-pass".into()), what: "the OpenStreetMap planet".into(), why };
        let declared = self.brief_now(root).and_then(|b| inputs::planet::declared(root, &b.manifest));
        let p = match declared {
            Ok(Some(p)) => p,
            Ok(None) => {
                waiting.push(wait(
                    "no planet is declared (inputs/sources/: the osm-planet role, docs/inputs.md §6.9)".into(),
                ));
                return None;
            }
            Err(e) => {
                waiting.push(wait(format!("the planet's declaration can't be read now: {e:#}")));
                return None;
            }
        };
        let next = inputs::planet::next(root, &p, have)?;
        if next == inputs::planet::Next::Behind {
            waiting.push(wait(format!(
                "the declared planet, of {}, is older than the last complete pass's ({}): passes only go forward",
                p.date,
                have.unwrap_or_default()
            )));
            return None;
        }
        Some((p.date, next))
    }

    pub(super) fn newer_app(&self) -> bool {
        newer_app(&self.o, &self.app)
    }

    pub(super) fn handoff_bases(&self, _root: &NasRoot) -> Vec<PathBuf> {
        handoff_bases(&self.o)
    }

    pub(super) fn osm_pass_cmd(&self, root: &NasRoot, planet: &NasPath, date: &str) -> Vec<String> {
        osm_pass_cmd(&self.o, root, planet, date)
    }

    pub(super) fn step_secs_here(&self, step: &str) -> Option<f64> {
        step_secs_here(self.o.the_plan, &self.inp.mem, step)
    }

    pub(super) fn due(&self, id: &str, every: Duration) -> bool {
        self.inp.mem.last_ok.get(id).is_none_or(|&t| now_s().saturating_sub(t) >= every.as_secs())
    }

    pub(super) fn note(&self, e: crate::coord::history::Event) {
        if let Some(c) = &self.coord {
            c.note(e);
        }
    }

    /// Whether this agent keeps the lead's record of its catalogues (and notes planned map tiles in
    /// its history): the one that runs its jobs, not a dry run beside it (planning only: its record
    /// is its own, in memory).
    pub(super) fn keeps_publishing(&self) -> bool {
        self.locked && !self.o.dry_run
    }

    /// The records' brief to plan by (crate::agent::brief): the one its coordinator serves, in
    /// process; the NAS's while it serves none (its agent's first save not made yet, or not
    /// leading).
    pub(super) fn brief_now(
        &self,
        root: &store::nasfs::NasRoot,
    ) -> Result<std::sync::Arc<crate::pool::records::Brief>> {
        match self.coord.as_deref().and_then(|c| c.brief()) {
            Some(h) => Ok(h.brief),
            None => brief::on_nas(root).map(std::sync::Arc::new),
        }
    }

    /// The records' manifest and the planning keys (`planning_keys`), from one brief.
    pub(super) fn planning_records(
        &self,
        root: &store::nasfs::NasRoot,
    ) -> Result<(BTreeMap<String, String>, crate::pool::records::Keys)> {
        let r = self.brief_now(root)?;
        Ok((r.manifest.clone(), r.keys.clone().with_waiting(&self.handoff_bases(root))?))
    }

    /// The plan offered through this Mac's coordinator (docs/pool.md §7.2), in its order: each
    /// offered step's targets where its work first comes (`offered_job`: their offer as the regions'
    /// planning made it, `region_jobs`), the lead's own planned jobs each whole (`Offer::job`: granted
    /// to this Mac alone, crate::agent::every::grants); an offered step whose work this Mac planned
    /// none of (past its memory, say) last.
    pub(super) fn offer_plan(&self, plan: &[JobSpec]) {
        let Some(c) = &self.coord else { return };
        c.set_plan(self.n);
        let (pass, offered) = self.offered.borrow().clone();
        let mut offers: Vec<crate::coord::Offer> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for spec in plan {
            match spec.record.as_ref().filter(|w| every::offered(self.o.the_plan, &w.step)) {
                Some(w) => {
                    // (Every offer of its step: a halo step's are one a halo, crate::needs::HALO_STEPS.)
                    if seen.insert(w.step.clone()) {
                        offers.extend(offered.iter().filter(|o| o.step == w.step).cloned());
                    }
                }
                None => {
                    // (No memory guessed: its measures size it, task #979.)
                    let step =
                        spec.record.as_ref().map(|w| w.step.clone()).or_else(|| step_of(&spec.id)).unwrap_or_default();
                    offers.push(crate::coord::Offer {
                        targets: own_targets(spec).into_iter().map(|(t, k)| (t, k, 0)).collect(),
                        step,
                        job: Some(spec.id.clone()),
                    });
                }
            }
        }
        offers.extend(offered.into_iter().filter(|o| !seen.contains(&o.step)));
        c.offer(&pass, offers);
    }

    /// The targets of the plan's `works` the memory guard holds (its switch on): those whose floor,
    /// learned alone the way its step runs now, passes the most this Mac's jobs could hold above its memory floor (its memory less the floor: `here`), and of them those
    /// past every Mac's in the pool, as the lead knows them (`all`: a step only this Mac runs, this
    /// Mac's limit alone), with why for the status; and those whose floor past it was a batch's
    /// (`alone`: tried again in a job of their own before they're held).
    pub(super) fn guard_holds(&self, works: &[jobs::Work]) -> GuardHolds {
        let mut h = GuardHolds::default();
        let limit = self.jobs_ceiling();
        let Some(c) = self.coord.as_ref().filter(|_| limit > 0) else { return h };
        let largest = c.largest_limit().unwrap_or(0).max(limit);
        let gb = |mb: u64| mb as f64 / 1024.0;
        for w in works {
            let most = if self.o.the_plan.shared().contains(&w.step.as_str()) { largest } else { limit };
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
            if n_all > 0 && !self.o.the_plan.shared().contains(&w.step.as_str()) {
                // A step only the lead runs: judged against the lead's limit alone; it waits (the
                // lead is fixed: the lead's memory or the step's is the owner's to see to).
                let (s, v) = if n_all == 1 { ("", "s") } else { ("s", "") };
                let remedy = "the lead's memory floor (`scenic memory`) or the step's memory, for the owner to see to";
                h.why.push(Waiting { step: Some(w.step.clone()), what: crate::pool::records::current(&w.step).into(), why: format!("{n_all} target{s} need{v} about {:.1} GB, past the {:.1} GB the lead's jobs could hold above its memory floor; this step runs only on the lead. {remedy} (`scenic pool floors --clear` once it's fixed)", gb(need), gb(limit)) });
            } else if n_all > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: crate::pool::records::current(&w.step).into(), why: format!("{n_all} target{} need{} about {:.1} GB, more than any Mac in the pool has (the most: {:.1} GB): a step whose memory grows with its target, for the owner to see to (`scenic pool floors --clear` once it's fixed)", if n_all == 1 { "" } else { "s" }, if n_all == 1 { "s" } else { "" }, gb(need), gb(most)) });
            } else if n_here > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: crate::pool::records::current(&w.step).into(), why: format!("{n_here} target{} need{} about {:.1} GB, past the {:.1} GB this Mac's jobs could hold above its memory floor: left to a Mac with room", if n_here == 1 { "" } else { "s" }, if n_here == 1 { "s" } else { "" }, gb(need), gb(limit)) });
            }
        }
        h
    }

    /// A catalogue made: the gate's count of it held back starts again (publish::LastCatalogue::held_at).
    /// (When it went out, and with the low zooms or not, is the map log's: `map_now`, `map_marks`.)
    pub(super) fn catalogue_made(&self) {
        let mut r = self.catalogues.borrow_mut();
        if r.held_at.take().is_none() {
            return;
        }
        if self.keeps_publishing() {
            if let Err(e) = write_published(&self.o.home, &r) {
                eprintln!("agent: noting the map update made: {e:#}");
            }
        }
    }

    /// The steps gated by time whose recorded keys in `done` changed: noted run now in the lead's
    /// record (publish::LastCatalogue::observe).
    pub(super) fn note_ran(&self, done: &crate::pool::records::Keys) {
        let mut r = self.catalogues.borrow_mut();
        if r.observe(done, now_s()) && self.keeps_publishing() {
            if let Err(e) = write_published(&self.o.home, &r) {
                eprintln!("agent: noting the steps run: {e:#}");
            }
        }
    }

    /// When a catalogue due was first held back (publish::LastCatalogue::held_at), kept in the lead's
    /// record when it changes.
    pub(super) fn note_held(&self, held_at: Option<u64>) {
        let mut r = self.catalogues.borrow_mut();
        if r.held_at == held_at {
            return;
        }
        r.held_at = held_at;
        if self.keeps_publishing() {
            if let Err(e) = write_published(&self.o.home, &r) {
                eprintln!("agent: noting the map update held back: {e:#}");
            }
        }
    }

    /// Publishing as planned (publish::PublishLine) kept for the status, the pause said, and logged
    /// when it or its reasons change.
    pub(super) fn record_publishing(&self, mut u: publish::PublishLine) {
        if let Some(p) = &self.inp.pause {
            u.why = format!("{} (but {})", u.why, p.why());
        }
        let key = u.key() + &self.inp.pause.is_some().to_string();
        if *self.publish_logged.borrow() != key {
            eprintln!("agent: {}", u.line());
            *self.publish_logged.borrow_mut() = key;
        }
        *self.publish_line.borrow_mut() = Some(u);
    }

    /// The timetables' report, read again when the records name another (a small file).
    pub(super) fn note_timetables(&self, root: &store::nasfs::NasRoot, manifest: &BTreeMap<String, String>) {
        let Some(c) = manifest.get(inputs::inputs::gtfs::REPORT) else { return };
        if self.timetables.borrow().as_ref().is_some_and(|t| &t.0 == c) {
            return;
        }
        if let Some(v) =
            nasread::read(&root.join(c)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        {
            *self.timetables.borrow_mut() = Some((c.clone(), inputs::inputs::gtfs::View::of(&v)));
        }
    }

    /// The work there is, in order (docs/building.md §2, Order).
    /// A digest of what an idle agent's plan depends on, read cheaply: the build's records (the
    /// manifest, the jobs' keys, the hand-offs waiting), the gate's view of the inputs and the units
    /// on it with their folders as last listed, the pool
    /// (membership, role, gates), its mode and switch, and the pause. Equal digests: the plan would
    /// come out the same but for what `IDLE_REPLAN` covers (time, disk room, a planet newly fetched)
    /// and the leases, whose change filters its work again (`refilter_due`).
    pub(super) fn plan_seen(&self, root: &store::nasfs::NasRoot) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let stat = |h: &mut std::collections::hash_map::DefaultHasher, p: store::nasfs::Loc<'_>| {
            let m = p.metadata().ok();
            m.as_ref().map(|m| (m.len(), m.modified().ok())).hash(h);
        };
        // (The records: the one path's snapshot, which the lead rewrites whole with each save
        // (docs/coordination.md steps 8b, 8c); today's files before the pool.)
        stat(&mut h, (&root.join(crate::pool::records::ONE)).into());
        // (And the brief its coordinator serves, the plan's (`brief_now`): a save seen at once, an
        // SMB stat's cache not waited out.)
        self.coord.as_deref().and_then(|c| c.brief_mark()).hash(&mut h);
        for f in ["state/build/manifest.json", "state/build/jobs.json"] {
            stat(&mut h, (&root.join(f)).into());
        }
        for b in self.handoff_bases(root) {
            stat(&mut h, (&b).into());
            let mut hosts: Vec<PathBuf> =
                std::fs::read_dir(&b).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            hosts.sort();
            for d in hosts {
                d.hash(&mut h);
                stat(&mut h, (&d).into());
            }
        }
        serde_json::to_string(&*self.inputs.view.borrow()).unwrap_or_default().hash(&mut h);
        // (And what the gate's part of the plan reads now: the units on the gate (the test unit's
        // flag) and each one's folder and acceptances as last listed. The view above is the last
        // plan's, so a unit taken off the gate, or a drop box listed anew, went unseen until the
        // fallback: #969 review, the gate's test failing on a loaded Mac.)
        let units = inputs::inputs::units(root);
        units.hash(&mut h);
        if let Some((_, w)) = &self.inputs.watch {
            for u in &units {
                w.get(u).map(|l| (serde_json::to_string(&l.listing).unwrap_or_default(), l.accepted)).hash(&mut h);
            }
        }
        if let Some(p) = &self.inp.pool_seen {
            (&p.member, p.role, &p.members).hash(&mut h);
            serde_json::to_string(&p.gates).unwrap_or_default().hash(&mut h);
        }
        serde_json::to_string(&self.inp.pause).unwrap_or_default().hash(&mut h);
        // (The held maps' folder on this Mac, and the map's head; publishing's switches are the
        // pool's, below.)
        stat(&mut h, (&publish::held_dir(&self.o.home)).into());
        self.inp.map.as_ref().and_then(|m| m.feed.head()).hash(&mut h);
        // (The owner's redo asks: one taken or done.)
        stat(&mut h, (&self.o.home.join(redo::ASKS)).into());
        // (Not the coordinator's leases: they filter the last plan's work again, `refilter_due`.)
        self.newer_app().hash(&mut h);
        // (The pool's switches as this agent holds them.)
        (self.inp.placement, self.inp.switches.held, self.inp.switches.near).hash(&mut h);
        // (Never 0: that's "not read".)
        h.finish() | 1
    }

    /// When the plan's timed waits end (epoch seconds, past `now`), the earliest: a job's failure or
    /// memory guard wait, room-making's ten minutes, the regions' edit hold, the next catalogue's
    /// minute. An idle agent plans again then (`plan_due`).
    pub(super) fn replan_at(&self, plan: &[JobSpec], now: u64) -> Option<u64> {
        let mut at: Vec<u64> = Vec::new();
        for spec in plan {
            at.extend(self.inp.guard_backoff.get(&spec.id).map(|(u, _)| *u));
            at.extend(self.inp.mem.retry.get(&spec.id).map(|(_, u)| *u));
        }
        if let Some((_, _, t)) = self.inp.floor_short {
            at.push(ends_in(now, Duration::from_secs(600).saturating_sub(t.elapsed())));
        }
        if let Some((_, left)) = edit_held(self.edited_at.get(), std::time::SystemTime::now()) {
            at.push(ends_in(now, left));
        }
        if let Some(u) = self.publish_line.borrow().as_ref().filter(|u| u.wait_s > 0) {
            at.push(now + u.wait_s);
        }
        // (The budgets' proxy not started: tried again each loop, `tend_proxy`; the plan again each
        // minute, not each loop, should it keep failing.)
        if !self.inp.proxy_up && !self.o.dry_run && !plan.is_empty() {
            at.push(now + PROXY_REPLAN.as_secs());
        }
        earliest_after(at, now)
    }

    pub(super) fn plan(
        &self,
        root: &store::nasfs::NasRoot,
        c: &Conditions,
        waiting: &mut Vec<Waiting>,
    ) -> Vec<JobSpec> {
        // (The slow-plan fake's, docs/agent-parts.md §7: a plan that takes minutes.)
        #[cfg(test)]
        if let Some(d) = self.slow_plan {
            std::thread::sleep(d);
        }
        let jobs = self.plan_unchecked(root, c, waiting);
        // (In a unit test, `buildstore::out::testing`, every planned command checked against its
        // program's table, crate::cli.)
        if buildstore::out::testing() {
            for j in &jobs {
                if let Some(Err(e)) = crate::cli::check_command(&j.cmd) {
                    panic!("{e}");
                }
            }
        }
        jobs
    }

    /// The depth's targets for GC's clean-up (`gc::DEPTH_KEEP_FILE` in the home), from this plan
    /// and the leases now: written as each plan finds GC due, so the job reads them minutes old;
    /// none found, none there. Only by the agent that runs the jobs: a dry run beside it, maybe
    /// another app's and with no leases, shares its home and never touches the file. Its path.
    pub(super) fn keep_depth_targets(&self) -> PathBuf {
        let keep = self.o.home.join(gc::DEPTH_KEEP_FILE);
        if self.locked && !self.o.dry_run {
            let leased = self.coord.as_ref().map(|c| c.leased_targets(&gc::DEPTH_STEPS)).unwrap_or_default();
            let found = self.depth_keep.borrow().clone().map(|k| k.with_leased(&leased));
            let written = found.map(|k| store::whole::write(&keep, &serde_json::to_vec(&k).unwrap_or_default()));
            if !matches!(written, Some(Ok(()))) {
                std::fs::remove_file(&keep).ok();
            }
        }
        keep
    }

    /// Whether the `gc` job has work (docs/storage.md, GC): a chunk GC's queue would hand
    /// it (one told and not gone, or a name due that a chunk takes: `gc::by_the_queue`, the older
    /// passes before `latest`), only the lead's agent running the jobs knowing; or a sweep's pass
    /// under way or due (`gc::Sweeps`, `sweeps` its file). In words, for the job's card.
    pub(super) fn gc_work(&self, sweeps: &Path, latest: Option<&str>) -> Option<String> {
        let now = now_s();
        let here = self.inp.map.as_ref().filter(|_| self.locked && !self.o.dry_run);
        if let (Some(here), Some(co)) = (here, &self.coord) {
            let told = here.gc.removing_now().len();
            let cursors = here.feed.cursors();
            let holders = super::gcqueue::Holders { cursors: &cursors, oldest_lease: co.lease_bounds().0, now };
            let due = told + here.gc.due_by(&holders, &|n| gc::by_the_queue(n, latest)).len();
            if due > 0 {
                return Some(format!("Removing {due} files nothing holds (GC's queue)"));
            }
        }
        let s = gc::Sweeps::read(sweeps);
        gc::SWEEPS.iter().find(|(n, _)| s.due(n, now)).map(|(n, _)| format!("Sweeping the NAS ({n})"))
    }

    pub(super) fn plan_unchecked(
        &self,
        root: &store::nasfs::NasRoot,
        _c: &Conditions,
        waiting: &mut Vec<Waiting>,
    ) -> Vec<JobSpec> {
        let mut out = Vec::new();
        // (Nothing offered but what this plan offers: `region_jobs`.)
        *self.offered.borrow_mut() = Default::default();
        let me = self.o.bin.join("scenic");
        let s = |p: &Path| p.to_string_lossy().into_owned();

        // 1. The OSM pass, when the declared planet is newer than the last complete pass: fetched
        // first when the NAS lacks it (inputs::planet).
        let have = self.latest_pass(root);
        let newer = match self.planet_next(root, have.as_deref(), waiting) {
            Some((date, inputs::planet::Next::Pass(planet))) => Some((planet, date)),
            Some((date, inputs::planet::Next::Fetch)) => {
                out.push(JobSpec {
                    id: format!("planet-fetch {date}"),
                    what: format!("Fetching the OpenStreetMap planet of {date}"),
                    cmd: vec![
                        s(&self.o.bin.join("scenic-build")),
                        "planet-fetch".into(),
                        "--root".into(),
                        jobs::root_arg(root),
                        "--date".into(),
                        date,
                    ],
                    needs: Needs { nas: true },
                    restart_after_sleep: true,
                    record: None,
                });
                None
            }
            _ => None,
        };
        // (Resuming on: held for, and offered to, the member whose Mac has its progress.)
        let held = self.resume_hold(newer.as_ref());
        if let Some((planet, date)) = newer.clone() {
            let what = format!("Reading the OpenStreetMap planet of {date}");
            let scratch = self.o.home.join("scratch").join(format!("osm-{date}"));
            let jar = root.join("sources/basemap/planetiler.jar");
            // The cheap caches' space counts as free (room::make_room frees it). (The pack cache the
            // pass clears holds the base packs' way indexes alone since task #396: a few kB each,
            // not counted.)
            let mut free = self.disk_free();
            // (The owner's room target kept free past it, as the pass keeps it: room::TARGET_ENV.)
            let need = PASS_SPACE + self.floor();
            if free < need {
                free += room::cheap_bytes(&self.o.home.join("cache"), Some(&root.join("sources")));
            }
            let started = scratch.exists();
            if let Some(w) = held {
                waiting.push(w);
            } else if !jar.exists() {
                waiting.push(Waiting {
                    step: Some("osm-pass".into()),
                    what,
                    why: "sources/basemap/planetiler.jar is missing on the NAS".into(),
                });
            } else if !started && free < need {
                waiting.push(Waiting {
                    step: Some("osm-pass".into()),
                    what,
                    why: format!(
                        "needs {} GB free on this Mac ({} GB free{})",
                        need >> 30,
                        free >> 30,
                        self.floor_words()
                    ),
                });
            } else {
                out.push(JobSpec {
                    id: format!("osm-pass {date}"),
                    what,
                    cmd: self.osm_pass_cmd(root, &planet, &date),
                    needs: Needs { nas: true },
                    restart_after_sleep: true,
                    record: None,
                });
            }
        }

        // The regions: terrain and slope near the coverage, base(U), pack(T), lo, a catalogue.
        out.extend(self.region_work(root, have.as_deref(), newer.as_ref().map(|n| n.1.as_str()), waiting));

        // The gate's checks before every other step (docs/inputs.md §4.3): a change is checked
        // before the plan builds with stale inputs.
        let (checks, rest): (Vec<JobSpec>, Vec<JobSpec>) =
            std::mem::take(&mut out).into_iter().partition(|j| j.record.as_ref().is_some_and(|w| w.step == "inputs"));
        out = checks;
        out.extend(rest);
        // Daily: the user's folders backed up, and the map server's profiles in the app's folder
        // (the agent's home is inside it), replaced files removed.
        if self.due("backup", Duration::from_secs(super::backup::EVERY_S)) {
            let mut cmd = vec![
                s(&me),
                "backup".into(),
                "--root".into(),
                jobs::root_arg(root),
                "--local".into(),
                s(&self.o.home.join("backups")),
            ];
            if let Some(app) = self.o.home.parent() {
                cmd.extend(["--app".into(), s(app)]);
            }
            out.push(JobSpec {
                id: "backup".into(),
                what: "backup".into(),
                cmd,
                needs: Needs { nas: true },
                restart_after_sleep: true,
                record: None,
            });
        }
        // The owner's redos (`scenic redo`, crate::agent::redo), the lead's: a job each, which forgets
        // its targets' records.
        if self.coord.is_some() {
            out.extend(self.redo_jobs(root));
        }
        // GC, whenever it has work (docs/storage.md, GC): the NAS budget grants it when quiet.
        let sweeps = self.o.home.join(super::publish::MAPLOG).join(gc::SWEEPS_FILE);
        if let Some(what) = self.gc_work(&sweeps, have.as_deref()) {
            let keep = self.keep_depth_targets();
            let cmd = vec![
                s(&me),
                "gc".into(),
                "--root".into(),
                jobs::root_arg(root),
                "--depth-keep".into(),
                s(&keep),
                "--sweeps".into(),
                s(&sweeps),
                "--history".into(),
                s(&self.o.home.join("coord/history.jsonl")),
            ];
            out.push(JobSpec {
                id: "gc".into(),
                what,
                cmd,
                needs: Needs { nas: true },
                restart_after_sleep: true,
                record: None,
            });
        }
        out
    }

    /// The jobs of the owner's redos kept in this Mac's agent's folder (crate::agent::redo): one per
    /// ask, `scenic redo` listing what it clears (it changes no file), its done record ("<step>
    /// <target>", no key) forgetting their keys as the lead merges it.
    pub(super) fn redo_jobs(&self, root: &store::nasfs::NasRoot) -> Vec<JobSpec> {
        let asks = match redo::pending(&self.o.home) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("agent: the redo asks: {e:#}");
                return Vec::new();
            }
        };
        let me = self.o.bin.join("scenic");
        asks.into_iter()
            .map(|a| {
                let mut cmd = vec![
                    me.to_string_lossy().into_owned(),
                    redo::STEP.into(),
                    "--root".into(),
                    jobs::root_arg(root),
                    a.step.clone(),
                ];
                cmd.extend(a.targets.iter().cloned());
                JobSpec {
                    id: redo::job_id(a.n),
                    what: format!(
                        "Forgetting {} {} target{}' records",
                        a.targets.len(),
                        a.step,
                        if a.targets.len() == 1 { "" } else { "s" }
                    ),
                    cmd,
                    needs: Needs { nas: true },
                    restart_after_sleep: true,
                    record: Some(jobs::Work { step: redo::STEP.into(), targets: redo::done_of(&a) }),
                }
            })
            .collect()
    }

    /// The owner's redo asks this Mac's coordinator took (`/api/redo`), kept in its agent's folder
    /// until their jobs are done: not a dry run's.
    pub(super) fn tend_redo(&self) {
        if self.o.dry_run {
            return;
        }
        let Some(c) = &self.coord else { return };
        let asks = c.take_redo_asks();
        if asks.is_empty() {
            return;
        }
        for a in &asks {
            eprintln!("agent: {} asked to redo {} {} target(s) (redo {})", a.by, a.targets.len(), a.step, a.n);
        }
        if let Err(e) = redo::add(&self.o.home, asks) {
            eprintln!("agent: the redo asks can't be kept: {e:#}");
        }
    }

    /// The build steps for the regions, as jobs (docs/building.md §2), in order: the newest pass's
    /// worldwide jobs, each gated by its own inputs, then what `build::plan` finds stale (its
    /// chains), each step's targets in one run of `scenic-build`. The agent runs the first not
    /// waiting out a failure, so one failing job doesn't hold up the others.
    pub(super) fn region_work(
        &self,
        root: &store::nasfs::NasRoot,
        pass: Option<&str>,
        newer: Option<&str>,
        waiting: &mut Vec<Waiting>,
    ) -> Vec<JobSpec> {
        let mut held = Holds::default();
        let jobs = self.region_jobs(root, pass, newer, waiting, &mut held);
        // (The offered steps' work held as their jobs are: not offered meanwhile.)
        self.offered.borrow_mut().1.retain(|o| {
            !(held.sources && self.o.the_plan.reads_sources(&o.step))
                && !(held.lakefill && self.o.the_plan.reads_lakefill(&o.step))
        });
        let jobs = hold_for_sources(self.o.the_plan, jobs, held.sources, waiting);
        hold_for_lakefill(self.o.the_plan, jobs, held.lakefill, waiting)
    }

    /// `region_work`'s jobs before the holds: `held.lakefill` set when the pass's lake fill holds
    /// the jobs that read it (`hold_for_lakefill`), `held.sources` while no version of the source
    /// declarations is accepted (`hold_for_sources`).
    pub(super) fn region_jobs(
        &self,
        root: &store::nasfs::NasRoot,
        pass: Option<&str>,
        newer: Option<&str>,
        waiting: &mut Vec<Waiting>,
        held: &mut Holds,
    ) -> Vec<JobSpec> {
        // (The depth's targets for GC, and the regions' checklist: none until this plan finds them.)
        self.depth_keep.borrow_mut().take();
        self.regions_checklist.borrow_mut().take();
        // (A plan's `build::plan` result made again: the last one let go before, not held beside it.
        // Filtered again, taken below.)
        if !self.refilter.get() {
            self.held_plan.let_go();
        }
        // (The records unreadable now: nothing planned until they are, rather than everything again.)
        let (manifest, keys): (BTreeMap<String, String>, crate::pool::records::Keys) = match self.planning_records(root)
        {
            Ok(r) => r,
            Err(e) => {
                waiting.push(Waiting {
                    step: None,
                    what: "Building".into(),
                    why: format!("the build's records can't be read now: {e:#}"),
                });
                return Vec::new();
            }
        };
        // (Once a readers' switch is recorded, an app older than the one it was recorded on reads
        // the wrong regions: it builds nothing, and says so.)
        let regions_app = inputs::inputs::regions::read::switch_app(&manifest, &|n| {
            root.join(n).read().with_context(|| format!("read {n}"))
        })
        .map_err(|e| format!("{e:#}"));
        // (And the coordinator grants nothing to an app older than it, a page's included.)
        if let (Some(c), Ok(min)) = (&self.coord, &regions_app) {
            c.set_min_app(min.clone());
        }
        if let Err(why) = inputs::inputs::regions::read::may_lead(&self.app, &regions_app) {
            waiting.push(Waiting {
                step: None,
                what: "All building".into(),
                why: format!("this Mac can't build now: {why}"),
            });
            return Vec::new();
        }
        // (The regions from the same records as the coverage.)
        let (recipes, unread): (Vec<recipes::Recipe>, Vec<(String, String)>) =
            match inputs::inputs::regions::read::source(root, &manifest).and_then(|a| {
                let r = inputs::inputs::regions::read::recipes(&a)?;
                self.regions_view(root, &a, &r);
                Ok(r)
            }) {
                Ok(r) => (r, Vec::new()),
                Err(e) => (Vec::new(), vec![("regions".into(), format!("{e:#}"))]),
            };
        if let Some((_, why)) = unread.iter().find(|(f, _)| f == "regions") {
            waiting.push(Waiting {
                step: None,
                what: "Building the regions".into(),
                why: format!("the regions can't be read now: {why}"),
            });
        }
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let build_bin = s(&self.o.bin.join("scenic-build"));
        let mut jobs: Vec<JobSpec> = self.gate_work(root, &manifest, &keys, waiting);
        held.sources = !inputs::inputs::sources::accepted(&manifest);
        // (A job's name is its step's title, from the steps table: `step` is the scenic-build
        // command, the step by its id's.)
        let job = |id: String, step: &str, extra: Vec<String>, record: Option<jobs::Work>| {
            let scratch = self.o.home.join("scratch").join(step);
            let mut cmd = vec![
                build_bin.clone(),
                step.to_string(),
                "--root".into(),
                jobs::root_arg(root),
                "--scratch".into(),
                s(&scratch),
            ];
            cmd.extend(extra);
            let what = crate::pool::records::current(step_of(&id).as_deref().unwrap_or(step)).to_string();
            // (Every job needs the NAS; none waits for power or home, the owner's choice: the pass's
            // whole-planet reads go over Tailscale when away, slowly.)
            JobSpec { id, what, cmd, needs: Needs { nas: true }, restart_after_sleep: true, record }
        };
        // Per pass, worldwide: the sets it lacks in their current filters (a set added or changed
        // since it ran), the outlines, the hiking routes' ends, the coastline's land, the reaches,
        // Overture's buildings (once per release), the labels, the languages spoken, the lake fill,
        // the water; then the names to translate and descriptions to write (build::pass_work). The
        // summits come after the regions' plan, below.
        let ask = seam::PassAsk {
            lakefill: pass.map_or(Ok(None), |_| inputs::lakefill::run::planned_source(root, &manifest)),
            failures: &|id| self.inp.mem.retry.get(id).map_or(0, |r| r.0),
        };
        // (Names-to-do's inputs' key: the last batch of the map's log touching what they read. The
        // steps gated by time: when each last ran, as its recorded key changed.)
        let names = || {
            let key = self
                .o
                .the_plan
                .names_todo_key(&self.map_marks(), self.inp.map.as_ref().and_then(|h| h.feed.map()).as_deref());
            self.note_ran(&keys);
            let r = self.catalogues.borrow();
            seam::NamesTodo { key, since: r.since("names-todo", now_s()), idle: r.idle }
        };
        let mut pw = self.held_plan.pass_work(root, pass, &manifest, &keys, ask, Some(&names));
        held.lakefill = pw.lakefill_held;
        waiting.append(&mut pw.waits);
        jobs.extend(std::mem::take(&mut pw.jobs).into_iter().map(|j| job(j.id, j.step, j.args, j.work)));
        if pw.outlines_hold {
            return jobs;
        }
        if pw.buildings_made {
            if let Ok(rd) = std::fs::read_dir(self.o.home.join("scratch/buildings")) {
                // Made: the scan's parts (~40 GB) go, even from a run stopped before it removed them.
                for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("parts-")) {
                    std::fs::remove_dir_all(e.path()).ok();
                }
            }
        }
        if recipes.is_empty() {
            return jobs;
        }
        let Some(date) = pass else {
            waiting.push(Waiting {
                step: None,
                what: "Building the regions".into(),
                why: "the first OpenStreetMap pass (it makes the outlines regions are drawn from)".into(),
            });
            return jobs;
        };
        // (The pass's worldwide jobs: the forecast's before the regions'.)
        let mut before = jobs.clone();
        let covs = match self.coverage(root, &manifest, unread.is_empty()) {
            Ok(c) => c,
            Err(why) => {
                waiting.push(Waiting { step: None, what: "Building the regions".into(), why });
                return jobs;
            }
        };
        let cov = &covs.all;
        self.note_timetables(root, &manifest);
        let done = keys;
        let mut inputs = self.o.the_plan.key_inputs(root, &manifest);
        // (The inputs' descriptions' credits, for the catalogue's key: inputs::inputs::credits. None
        // yet, no line, so the catalogue's key is as it was. Unreadable now: the last digest read
        // stands, or, with none read yet, the catalogue waits; never a key that flips on a failed
        // read.)
        let mut credits_unread = None;
        match inputs::inputs::credits::described(root, &manifest) {
            Ok(d) => *self.inputs.credits.borrow_mut() = Some(inputs::inputs::credits::digest(&d)),
            Err(e) if self.inputs.credits.borrow().is_none() => credits_unread = Some(format!("{e:#}")),
            Err(_) => {}
        }
        if let Some(Some(c)) = self.inputs.credits.borrow().clone() {
            inputs.insert("credits".into(), c);
        }
        // (The declared feeds the catalogue credits, for its key: "?" while they can't be read, and
        // the catalogue waits.)
        match inputs::inputs::feeds::credited(root, &manifest) {
            Ok(Some(ids)) => {
                inputs.insert("feeds-credited".into(), ids.join(","));
            }
            Ok(None) => {}
            Err(_) => {
                inputs.insert("feeds-credited".into(), "?".into());
            }
        }
        let held = self.inp.switches.held;
        // The base packs' extents (codecs::extents), kept by the lead alone, against its manifest:
        // the headers of packs new since read once, timed. (A pack whose header can't be read is
        // left out and looked at again next plan, the rest kept; the file not written costs only
        // spills not yet seen: the jobs read what it lacks.)
        if self.inp.pool_seen.as_ref().is_none_or(|p| p.role == poolside::Role::Lead) {
            let t = std::time::Instant::now();
            match codecs::extents::keep(root, &manifest) {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!("extents: {n} base packs' headers read and kept in {:.1} s", t.elapsed().as_secs_f64())
                }
                Err(e) => eprintln!("extents: not kept now: {e:#}"),
            }
        }
        self.held_plan.begin_round(root, &manifest, &done, date);
        if !manifest.contains_key(&inputs::inputs::logical(inputs::inputs::feeds::UNIT)) {
            waiting.push(Waiting { step: None, what: "trains".into(), why: "the timetable feeds' declarations' first accepted version (inputs/timetables/feeds/: the input check takes them in)".into() });
        } else if inputs.get("keys").map(String::as_str) == Some("?") {
            waiting.push(Waiting {
                step: None,
                what: "trains".into(),
                why: "inputs/keys.env can't be read now".into(),
            });
        } else if inputs.get("feed-keys").map(String::as_str) == Some("?") {
            waiting.push(Waiting {
                step: None,
                what: "trains".into(),
                why: "the timetable feeds' accepted declarations can't be read now".into(),
            });
        } else if let Some(why) = missing_feed_keys(&inputs) {
            waiting.push(Waiting { step: None, what: "trains".into(), why });
        }
        // (The ferries count the ferry feeds: without their list, rail-feeds' to make, they wait.)
        if manifest.contains_key(&inputs::osmpass::set_name(date, "ferries"))
            && !manifest.contains_key(inputs::rail::FERRY_FEEDS)
        {
            waiting.push(Waiting {
                step: Some("ferries".into()),
                what: "ferries".into(),
                why: "the ferry feeds' list (rail-feeds makes it once the timetable feeds' declarations are accepted)"
                    .into(),
            });
        }
        // (The trains and the ferries count the static files too: without their accepted version,
        // they wait, rather than be counted without them.)
        if !manifest.contains_key(&inputs::inputs::logical(inputs::inputs::gtfs::UNIT)) {
            let why = "the static timetables' first accepted version (inputs/timetables/gtfs/: the input check takes them in)";
            if manifest.contains_key(inputs::rail::FEEDS) {
                waiting.push(Waiting { step: Some("rail".into()), what: "trains".into(), why: why.into() });
            }
            if manifest.contains_key(&inputs::osmpass::set_name(date, "ferries")) {
                waiting.push(Waiting { step: Some("ferries".into()), what: "ferries".into(), why: why.into() });
            }
        }
        if inputs.get("bld-release").map(String::as_str) == Some("?") {
            waiting.push(Waiting {
                step: None,
                what: "buildings".into(),
                why: "the 3D buildings' sources' indexes on the NAS can't be read now".into(),
            });
        }
        // The regions the map has (or the held catalogue's, when catalogues are held for review) and
        // how long ago it went out: what the plan publishes regions by.
        let map = self.map_now();
        let (on_map, since_publish) = self.on_map(held, map.as_deref(), &recipes);
        // (The map's, held ones aside.)
        self.catalogue_seen.set(map.as_deref().map(seen));
        let way = publish::Publishing {
            since_low: self.since_low(now_s()),
            near: self.inp.switches.near,
            since_marks: self.catalogues.borrow().since("marks", now_s()),
            since_overlays: self.catalogues.borrow().since("overlays", now_s()),
        };
        // (The map's z6 tiles, read again only at a newer `seq`: the map tiles to draw grouped by the
        // z6 tile that changed.)
        let served = self.planned_tiles.borrow_mut().served(map.as_deref());
        drop(map);
        // (The plan, or on a lease change's filtering again the last result and its keys, `build::plan`
        // not made; its keys handed on to what derives keys of the same context below.)
        let round = seam::Round { root, cov, date, manifest: &manifest, inputs: &inputs, done: &done };
        let publish = seam::Publish {
            each: &covs.each,
            on_map: &on_map,
            served: &served,
            since_catalogue: since_publish,
            held_for: self.catalogues.borrow().held_at.map(|t| now_s().saturating_sub(t)),
            held,
            way,
        };
        let (mut planned, depth_keep) = self.held_plan.plan(&round, publish, self.refilter.get());
        *self.depth_keep.borrow_mut() = depth_keep;
        nasread::PLAN.enter("its jobs");
        *self.regions_checklist.borrow_mut() = Some(std::mem::take(&mut planned.checklist));
        *self.built.borrow_mut() = std::mem::take(&mut planned.built);
        self.catalogues.borrow_mut().idle = planned.idle;
        // The steps held for other steps' targets: said, never a silent stall (the summits' own note,
        // below, is the same).
        for (step, why) in std::mem::take(&mut planned.held) {
            waiting.push(Waiting { what: step.clone(), why, step: Some(step) });
        }
        // The z6 tiles held until the heritage registers are first accepted: said, not a silent stall.
        if let Some(why) = self.held_plan.heritage_waits(&round) {
            waiting.push(Waiting {
                step: Some("heritage-sites".into()),
                what: "sites".into(),
                why: format!("the regions' z6 tiles wait: {why}"),
            });
        }
        // The pass's summits (worldwide), once the terrain packs they read are built (as the
        // peaks after them): held, said.
        // The timetables' to-do list and report, after each rail and ferries run (lead-only: it
        // writes into the static timetables' inputs folder's todo/, never read as an input).
        if let Some(w) = self.held_plan.timetables_todo(&round) {
            jobs.push(job(
                "timetables-todo".into(),
                "timetables-todo",
                vec!["--pass".to_string(), date.to_string()],
                Some(w),
            ));
        }
        let summits = pw.summits.take();
        // (Held, they're the landmarks chain's first step in the forecast, after the terrain.)
        let mut summits_held = None;
        let summits_waits = self.o.the_plan.summits_waits(&planned.left);
        if let Some(why) =
            summits_waits.filter(|_| summits.is_some() || manifest.contains_key(&format!("work/summits/{date}")))
        {
            summits_held = summits;
            waiting.push(Waiting { step: Some("summits".into()), what: "summits".into(), why });
        } else if let Some(w) = summits {
            let j = job(
                format!("summits {date}"),
                "summits",
                vec!["--pass".into(), date.to_string(), "--cache".into(), s(&self.o.home.join("cache"))],
                Some(w),
            );
            jobs.insert(before.len(), j.clone());
            before.push(j);
        }
        // Z6 tiles whose terrain can't be worked out now (a pack's index unread): they wait for it.
        // Or the declared elevation datasets: before their switch (docs/inputs.md §6.8), or while
        // their accepted version can't be read.
        if !planned.unknown.is_empty() {
            let why = self.held_plan.unknown_why(&manifest, planned.unknown.len());
            waiting.push(Waiting { step: Some("unit".into()), what: "roads".into(), why });
        }
        let edit_hold = edit_held(self.edited_at.get(), std::time::SystemTime::now());
        // (Hand-offs of a step a catalogue waits for (build::CATALOGUE_GATE: the map tiles, the road
        // index) not yet merged: counted as done, their files not yet in the manifest. A z6 tile's
        // waiting goes out with a later catalogue.)
        let unmerged = self.handoff_bases(root).iter().any(|b| {
            buildstore::handoff::waiting_in(b)
                .map(|w| {
                    w.iter().any(|(_, h)| h.done.as_ref().is_some_and(|d| self.o.the_plan.catalogue_waits_for(&d.0)))
                })
                .unwrap_or(false)
        });
        let ready = planned.ready;
        *self.ready.borrow_mut() = ready.clone();
        // (As the catalogue's `--ready`: each with the outline it was built with.)
        let ready_arg: String = ready
            .iter()
            .map(|id| match recipes.iter().find(|r| &r.id == id) {
                Some(r) => format!("{id}={}", recipes::outline_digest(&r.outline)),
                None => id.clone(),
            })
            .collect::<Vec<_>>()
            .join(",");
        // (The catalogue's `--building`: the regions still building, each with its z6 tiles built of
        // those it builds.)
        let building_arg: String =
            planned.building.iter().map(|(id, b, t)| format!("{id}={b}/{t}")).collect::<Vec<_>>().join(",");
        let mut plan = planned.work;
        // (The map tiles as first planned at their keys, for publishing's times:
        // crate::pubtimes::PlannedNotes, since the last catalogue, what was noted known again after a
        // restart or a change of lead.)
        if self.keeps_publishing() {
            let at = self.catalogue_seen.get().map_or(0, |c| c.at);
            let b = if at > 0 { at } else { self.started };
            let mut notes = self.planned_tiles.borrow_mut();
            // (This coordinator's history read once a catalogue.)
            if notes.began() != b {
                let own = self.coord.as_ref().map(|c| c.history()).unwrap_or_default();
                if notes.begin(b, &own, now_s()) {
                    notes.seed_from_nas(root.clone());
                }
            }
            let tiles: Vec<(String, String)> =
                plan.iter().filter(|w| w.step == "pack").flat_map(|w| w.targets.iter().cloned()).collect();
            let new = notes.new_tiles(&tiles);
            if !new.is_empty() {
                let names: Vec<String> = new.iter().map(|t| t.0.clone()).collect();
                let by_unit = self.held_plan.restaled_by(&manifest, &served, &names);
                notes.note(&new);
                self.note(crate::pubtimes::planned_event(&new, by_unit));
            }
        }
        if let Some(why) = credits_unread.filter(|_| plan.iter().any(|w| w.step == "catalog")) {
            waiting.push(Waiting {
                step: Some("catalog".into()),
                what: "publish".into(),
                why: format!("the inputs' descriptions (their credits) can't be read now: {why}"),
            });
            planned.publishing.why = "the map update waits for the inputs' descriptions to be read".into();
            plan.retain(|w| w.step != "catalog");
        }
        // Just edited: the regions' work waits a while for more edits (each would build again what
        // the last started); what runs carries on.
        if let Some((age, left)) = edit_hold.filter(|_| !plan.is_empty()) {
            let left = left.as_secs().div_ceil(60);
            waiting.push(Waiting { step: None, what: "Building the regions".into(), why: format!("a region's recipe or outline changed {} min ago: their work starts in {left} min, after any more edits", age.as_secs() / 60) });
            plan.clear();
        }
        // A catalogue waits while a helper's hand-offs of its map tiles or road index wait to be
        // merged (counted as done, their files not yet in the manifest).
        // (Held so since the gate first held past publish::CATALOGUE_CAP_S: it goes, the regions those
        // hand-offs touch left out of `--ready`, build::plan's.)
        let held_for = self.catalogues.borrow().held_at.map(|t| now_s().saturating_sub(t));
        let mut held_back = planned.catalogue_held;
        if unmerged && !publish::unmerged_holds(unmerged, held_for) && plan.iter().any(|w| w.step == "catalog") {
            planned.publishing.why = format!(
                "a map update now with what's merged: held {} min by hand-offs waiting to be merged",
                publish::CATALOGUE_CAP_S / 60
            );
        }
        if publish::unmerged_holds(unmerged, held_for) && plan.iter().any(|w| w.step == "catalog") {
            held_back = true;
            waiting.push(Waiting {
                step: Some("catalog".into()),
                what: "publish".into(),
                why: "waits for a helper's work to be merged".into(),
            });
            planned.publishing.why = "the map update waits for a helper's work to be merged".into();
            plan.retain(|w| w.step != "catalog");
        }
        // (When a catalogue was first held back, kept in the lead's record; none held, none to make
        // and none waiting out its minute: the count starts again.)
        let catalogue_planned = plan.iter().any(|w| w.step == "catalog");
        let was = self.catalogues.borrow().held_at;
        self.note_held(publish::held_at(was, held_back, catalogue_planned || planned.catalogue_due, now_s()));
        // (Since when one has been due, for the stale flag: crate::agent::flags.)
        let due = held_back || catalogue_planned || planned.catalogue_due;
        let due_since = due.then(|| self.catalogue_due_since.get().unwrap_or_else(now_s));
        self.catalogue_due_since.set(due_since);
        planned.publishing.due_since = due_since;
        // (Its catalogue, once made, went out with the low zooms made, or not.)
        if plan.iter().any(|w| w.step == "catalog") {
            self.catalogue_low.set(planned.low);
        }
        self.record_publishing(std::mem::take(&mut planned.publishing));
        // (What room-making lets go last: `hints`.)
        *self.queued.borrow_mut() = plan.clone();
        // A z6 tile's extract's size (content-named files never change: each looked up once).
        let size = |u: &str| {
            let Some(c) = manifest.get(&format!("sources/osm/{date}/pieces/{}", u.replace('/', "-"))) else {
                return u64::MAX;
            };
            if let Some(&n) = self.piece_sizes.borrow().get(c) {
                return n;
            }
            let n = root.join(c).metadata().map_or(u64::MAX, |m| m.len());
            if n != u64::MAX {
                self.piece_sizes.borrow_mut().insert(c.clone(), n);
            }
            n
        };
        // A 3D buildings tile's rows read (bld::bld::sources::digests), for its jobs' memory.
        let bld_rows = |t: &str| inputs.get(&format!("bldprep-rows {t}")).and_then(|v| v.parse::<u64>().ok());
        let mut offers: Vec<crate::coord::Offer> = Vec::new();
        // (A terrain z3 tile's z6 tiles near the coverage, for its run's expected memory: only when
        // there's terrain to offer.)
        let z6: BTreeMap<String, usize> = if plan.iter().any(|w| w.step == "terrain-hi") {
            buildstore::coverage::coverage_tiles(cov)
                .into_iter()
                .map(|(q, ts)| (format!("3/{}/{}", q.0, q.1), ts.len()))
                .collect()
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
            for w in plan.iter_mut().filter(|w| !self.o.the_plan.shared().contains(&w.step.as_str())) {
                let others = c.held_by_others(&w.step);
                w.targets.retain(|t| !others.contains(&t.0));
            }
        }
        // A cold start of a step another worker runs now: none of the lead's own of that step (one
        // cold start of a step in the pool at a time).
        if let Some(c) = &self.coord {
            for w in plan.iter_mut().filter(|w| !w.targets.is_empty() && c.cold_under_way(&w.step)) {
                waiting.push(Waiting {
                    step: Some(w.step.clone()),
                    what: crate::pool::records::current(&w.step).to_string(),
                    why: "its first job, never measured, runs on another member now: its others wait for its measure"
                        .into(),
                });
                w.targets.clear();
            }
        }
        // (Every step to every member: the plan's every step offered but the lead's own,
        // crate::agent::every.)
        // (What a terrain-hi job's process holds of the coverage it reads, `coverage_of`: added to
        // its bound, buildstore::coverage::Coverage::read_peak_mb.)
        let cov_read = cov.read_peak_mb();
        for w in plan.iter_mut().filter(|w| every::offered(self.o.the_plan, &w.step)) {
            // What another worker builds now isn't planned here: what it leased from this Mac's
            // coordinator.
            let mut others = BTreeSet::new();
            if let Some(c) = &self.coord {
                others.extend(c.held(&w.step));
            }
            w.targets.retain(|t| !others.contains(&t.0));
            // The rest, offered to the workers that mount the NAS: z6 tiles with their extracts' sizes,
            // the 3D buildings by their rows (crate::needs::OFFERED_BY), terrain-hi with the memory
            // its code bounds its jobs at, the others with none (task #979: their measures size
            // them, a cold start its step's runs of an earlier way, else alone); each worker takes
            // what fits its memory.
            let guess = |t: &str| match w.step.as_str() {
                "unit" | "pois" => size(t),
                "terrain-hi" => {
                    self.o.the_plan.terrain_peak(if t.starts_with("6/") { 1 } else { z6.get(t).copied().unwrap_or(64) })
                        + cov_read
                }
                "bldprep" | "bldtiles" => bld_rows(t).unwrap_or(0),
                _ => 0,
            };
            offers.push(crate::coord::Offer {
                step: w.step.clone(),
                targets: w.targets.iter().map(|(t, k)| (t.clone(), k.clone(), guess(t))).collect(),
                job: None,
            });
        }
        // A step's targets offered together, in plan order (the plan lists a step's work by region:
        // an ask takes a run from its front). (A step with nothing left: none offered.)
        // (A halo step's work, one a halo (`build::by_unit`), offered so: a job of it takes one
        // halo's targets, its base packs opened once, crate::needs::HALO_STEPS.)
        let mut merged: Vec<crate::coord::Offer> = Vec::new();
        for o in offers {
            let halo = crate::needs::HALO_STEPS.contains(&o.step.as_str());
            match merged.iter_mut().find(|m| m.step == o.step && !halo) {
                Some(m) => m.targets.extend(o.targets),
                None => merged.push(o),
            }
        }
        // (Offered with the plan, in its order: `offer_plan`.)
        *self.offered.borrow_mut() = (date.to_string(), merged);
        // (Held here: left to a Mac with room. A target whose floor past this Mac's jobs' ceiling was a
        // batch's: a job of its own, so a floor learned alone holds it, or its measure frees it.)
        let mut alone: Vec<jobs::Work> = Vec::new();
        for w in plan.iter_mut() {
            w.targets.retain(|t| !holds.here.contains(&(w.step.clone(), t.0.clone())));
            let (one, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut w.targets)
                .into_iter()
                .partition(|t| holds.alone.contains(&(w.step.clone(), t.0.clone())));
            w.targets = rest;
            alone.extend(one.into_iter().map(|t| jobs::Work { step: w.step.clone(), targets: vec![t] }));
        }
        plan.extend(alone);
        // The forecast of the work left (crate::agent::forecast): the lead's, for the status (made
        // again at most each `forecast::EVERY`: idle, the plan's made each loop).
        // (Run on a thread of its own, `start_forecast`: the loop reads the last one done and never
        // waits on it, task #544.)
        self.take_forecast();
        let stale =
            self.forecast.borrow().as_ref().is_none_or(|f| now_s().saturating_sub(f.at) >= forecast::EVERY.as_secs());
        if stale && self.forecasting.borrow().is_none() {
            // (What can't be listed now: the regions' work after a new pass, or while the z6 tiles wait.)
            let blind = match newer {
                Some(d) => Some(format!("a new OpenStreetMap pass (the planet of {d}) comes first; the regions' work is known once it's done")),
                None if planned.regions.is_empty() => Some("the z6 tiles wait for the pass's heritage sites, reaches and roadside buildings; what the regions need is known once they're made".to_string()),
                None => None,
            };
            let z6: BTreeMap<String, usize> = buildstore::coverage::coverage_tiles(cov)
                .into_iter()
                .map(|(q, ts)| (format!("3/{}/{}", q.0, q.1), ts.len()))
                .collect();
            // (As the offers have it: the 3D buildings by their rows, crate::needs::OFFERED_BY.)
            let peak = |step: &str, t: &str| match step {
                "unit" | "pois" => {
                    crate::coord::unit_peak(self.o.the_plan.cost_version("unit"), &BTreeMap::new(), t, size(t))
                }
                "terrain-hi" => {
                    self.o.the_plan.terrain_peak(if t.starts_with("6/") { 1 } else { z6.get(t).copied().unwrap_or(64) })
                        + cov_read
                }
                "bldprep" | "bldtiles" => bld_rows(t).unwrap_or(0),
                _ => 0,
            };
            // (And the elevation pyramids' fill, the lead's, its z6 tiles at its measured time each.)
            let mut before = before;
            before.extend(
                plan.iter()
                    .filter(|w| matches!(w.step.as_str(), "heritage-sites" | "pyramids-fill"))
                    .map(|w| job(format!("{} {date}", w.step), &w.step, Vec::new(), Some(w.clone()))),
            );
            let mut chains = self.held_plan.chains_left(&round);

            chains[1].splice(0..0, summits_held);
            let record = self.forecast_record(
                &before,
                &planned.regions,
                &planned.drops,
                &planned.backfill,
                chains,
                &plan,
                blind,
                &planned.left,
                &peak,
            );
            self.start_forecast(record);
        }
        plan.retain(|w| !w.targets.is_empty());
        // (A job a step's work in the plan: an offered step's is its place in the plan's order, its
        // targets granted as many as fit a quarter of an hour, `offer_plan`; the pyramids' fill a z6
        // tile, `build::job_of`.)
        for w in plan.into_iter().map(|w| self.o.the_plan.job_of(w)) {
            // Held for review: the map goes to this agent's held maps (publish::held_dir: no server
            // follows them; the coordinator's `/map/held` serves the newest for review), once.
            if w.step == "catalog" && held {
                let k = w.targets.first().map(|t| t.1.clone()).unwrap_or_default();
                if done.catalogue_held.as_deref() == Some(k.as_str()) {
                    waiting.push(Waiting {
                        step: None,
                        what: "publish".into(),
                        why: "held for review (the pool's setting hold-catalog); its map is on the lead's disk (/map/held)"
                            .into(),
                    });
                    continue;
                }
                let mut j = job(
                    "catalog-held".into(),
                    "catalog",
                    vec![
                        "--held".into(),
                        publish::held_dir(&self.o.home).to_string_lossy().into_owned(),
                        "--ready".into(),
                        ready_arg.clone(),
                        "--building".into(),
                        building_arg.clone(),
                    ],
                    Some(jobs::Work { step: "catalog-held".into(), targets: vec![("catalog-held".into(), k)] }),
                );
                j.needs = Needs { nas: true };
                jobs.push(j);
                continue;
            }
            let mut extra: Vec<String> =
                w.targets.iter().map(|t| t.0.clone()).filter(|t| every::names_target(t) && *t != w.step).collect();
            extra.extend(super::step_args(&self.o, &w.step, date));
            // (The timetable feeds' declarations: the accepted version it was planned with.)
            if w.step == "rail-feeds" {
                extra.extend([
                    "--feeds-index".to_string(),
                    manifest.get(&inputs::inputs::logical(inputs::inputs::feeds::UNIT)).cloned().unwrap_or_default(),
                ]);
            }
            // (The static timetables: the accepted version the trains and the ferries were planned
            // with.)
            if w.step == "rail" || w.step == "ferries" {
                extra.extend([
                    "--gtfs-index".to_string(),
                    manifest.get(&inputs::inputs::logical(inputs::inputs::gtfs::UNIT)).cloned().unwrap_or_default(),
                ]);
            }
            // (The heritage registers' and matches' accepted versions the plan read: the job reads
            // them, never the inputs folders.)
            if w.step == "heritage-sites" {
                for unit in [inputs::inputs::heritage::UNIT, inputs::inputs::heritage_matches::UNIT] {
                    if let Some(i) = manifest.get(&inputs::inputs::logical(unit)) {
                        extra.extend([format!("--{unit}-index"), i.clone()]);
                    }
                }
            }
            // (Tree cover z6 tiles made again as they are, their mids made: expected the same.)
            let same = expect_same(&w, &done);
            if !same.is_empty() {
                extra.extend(["--expect-same".to_string(), same.join(",")]);
            }
            // (The regions a catalogue records as built: the plan's.)
            if w.step == "catalog" {
                extra.extend([
                    "--ready".to_string(),
                    ready_arg.clone(),
                    "--building".to_string(),
                    building_arg.clone(),
                ]);
                // (Its map posted to this agent: crate::agent::publish::POSTED.)
                extra.extend([
                    "--post".to_string(),
                    self.o
                        .home
                        .join(super::publish::MAPLOG)
                        .join(super::publish::POSTED)
                        .to_string_lossy()
                        .into_owned(),
                ]);
            }
            let n = w.targets.len();
            // "Terrain: 3 tiles" (the checklist names the steps alike: crate::agent::steps::said); the
            // z6 tiles made again for their mids say so.
            let mut what = self.o.the_plan.said(&w.step, n);
            if same.len() == n && matches!(w.step.as_str(), "trees" | "terrain-hi" | "slope-hi") {
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

    /// Publishing as it stands (`Published`): the map's last batch, whether it went out with the low
    /// zooms, the map tiles drawing, and each Mac's map server's adoption (its cursor in the log:
    /// docs/macs.md, The map's log).
    pub(super) fn published(&self) -> Published {
        let last = self.catalogue_seen.get();
        let mut drawing: BTreeMap<String, usize> = BTreeMap::new();
        for (w, k) in self.coord.as_ref().map(|c| c.leased_of("pack")).unwrap_or_default() {
            *drawing.entry(host_of(&w).to_string()).or_default() += k;
        }
        // (The low zooms' flag of the same batch as `last`: whether its `why` said so.)
        let low = last.is_some_and(|l| self.map_marks().get(publish::LOW_WATCH).is_some_and(|m| m.seq == l.n));
        let cursors = self.inp.map.as_ref().map(|h| h.feed.cursors()).unwrap_or_default();
        Published { last, low, drawing, adopted: adopted(&cursors) }
    }

    /// The map now as a catalogue (docs/building.md §2, 5): the lead's log's (`Inputs::map`); None
    /// without it (not the lead, a dry run, a log not started yet): the NAS keeps no copy.
    pub(super) fn map_now(&self) -> Option<std::sync::Arc<store::catalogue::Catalogue>> {
        self.inp.map.as_ref().and_then(|h| h.catalogue())
    }

    /// The map log's marks (store::map::log::Log::marks): none without the lead's log.
    pub(super) fn map_marks(&self) -> BTreeMap<String, store::map::Mark> {
        self.inp.map.as_ref().map(|h| h.feed.marks()).unwrap_or_default()
    }

    /// Seconds since a batch last went out with the low zooms made (its `why`: publish::LOW); None
    /// when the log has none.
    pub(super) fn since_low(&self, now: u64) -> Option<u64> {
        let m = self.map_marks().remove(publish::LOW_WATCH)?;
        Some(now.saturating_sub(super::cataloguesview::unix_of(&m.at)?))
    }

    /// The forecast (crate::agent::forecast) of the work left: `before`, the lead's jobs before
    /// the regions'; `regions`, the plan's; `backfill`, the z6 tiles whose mids it makes in
    /// idle time (seam::Planned::backfill); `chains`, the trains', landmarks', 3D buildings' and depths' work to come
    /// (build::chains_left); `work`, the plan's work at the keys it's planned at; `blind`, why the
    /// work can't all be listed now. None of it done whose record the plan didn't read (the
    /// coordinator's done set at that key: a plan made as its job ended, task #1375). Each target's time (at
    /// the lead's pace: a time measured on a member over its speed) and memory as last measured,
    /// else its step's mean or a first guess (`first_secs`; `peak` for its memory); the helpers at
    /// their measured speed; each machine free once its job under way is done. As data
    /// (forecast::Record), run off the loop (`start_forecast`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn forecast_record(
        &self,
        before: &[JobSpec],
        regions: &[seam::RegionLeft],
        drops: &[(String, String)],
        backfill: &[jobs::Work],
        chains: [Vec<jobs::Work>; 5],
        work: &[jobs::Work],
        blind: Option<String>,
        left: &seam::Left,
        peak: &dyn Fn(&str, &str) -> u64,
    ) -> forecast::Record<'static> {
        use forecast::{Cost, Machine};
        // (Work done whose record the plan didn't read yet, offered still by a plan made from
        // records read before its hand-off was merged: none of it is to come, task #1375.)
        let unread = match &self.coord {
            Some(c) => forecast::unread(
                &c.done_unread(),
                work.iter()
                    .chain(backfill)
                    .chain(chains.iter().flatten())
                    .chain(before.iter().filter_map(|j| j.record.as_ref())),
            ),
            None => BTreeSet::new(),
        };
        let left_of = |w: &jobs::Work| -> Vec<(String, String)> {
            // (No pair made a target while nothing is done so: the usual case.)
            w.targets
                .iter()
                .filter(|t| unread.is_empty() || !unread.contains(&(w.step.clone(), t.0.clone())))
                .cloned()
                .collect()
        };
        let backfill: Vec<jobs::Work> =
            backfill.iter().map(|w| jobs::Work { targets: left_of(w), ..w.clone() }).collect();
        let chains = chains.map(|ws| {
            ws.iter()
                .map(|w| jobs::Work { targets: left_of(w), ..w.clone() })
                .filter(|w| !w.targets.is_empty())
                .collect::<Vec<_>>()
        });
        // (A job before the regions' whose targets are all done so: none.)
        let before: Vec<&JobSpec> = before
            .iter()
            .filter(|j| j.record.as_ref().is_none_or(|w| w.targets.is_empty() || !left_of(w).is_empty()))
            .collect();
        let drops: Vec<&(String, String)> = drops.iter().filter(|d| !unread.contains(d)).collect();
        let mut regions = regions.to_vec();
        forecast::without_unread(&mut regions, &unread);
        let (costs, leased, events, mem) = match &self.coord {
            Some(c) => c.for_forecast(),
            None => Default::default(),
        };
        // (Where the real grants since the last forecast departed from its next steps: logged.)
        if let Some(prev) = self.forecast.borrow().as_ref() {
            let real: Vec<(u64, String, String)> = events
                .iter()
                .filter(|e| e.kind == "lease")
                .filter_map(|e| Some((e.t, e.worker.clone()?, e.step.clone()?)))
                .collect();
            // (Each Mac's asks answered since, which a line of fewer granted names: task #1163.)
            let answers = self.coord.as_ref().map(|c| c.answers(prev.at)).unwrap_or_default();
            // (Each Mac and step said at most once an hour.)
            let now = now_s();
            let mut said = self.departed.borrow_mut();
            said.retain(|_, at| now.saturating_sub(*at) < 3600);
            for (k, d) in forecast::departures(prev, &real, &answers, now) {
                // (Stamped when logged only: a departure recurring every plan is said again an hour on.)
                if let std::collections::btree_map::Entry::Vacant(e) = said.entry(k) {
                    e.insert(now);
                    eprintln!("agent: the grants departed from the forecast: {d}");
                }
            }
            // (A step the forecast doesn't model, granted many times an hour: said, the same once an
            // hour; its keys never a departure's.)
            for (k, d) in forecast::unforeseen_many(&real, now) {
                if let std::collections::btree_map::Entry::Vacant(e) = said.entry(k) {
                    e.insert(now);
                    eprintln!("agent: many grants of a step the forecast doesn't model: {d}");
                }
            }
        }
        // Each worker's speed against the lead's, from the history (half until it's measured):
        // every one that measured a cost or did shared work, asleep or not (its costs are put at the
        // lead's pace by it); the members heard from lately are the machines.
        let helpers: Vec<Status> = helpers(&self.host, self.inp.beats.as_deref());
        let mut names: BTreeSet<String> = helpers.iter().map(|h| h.host.clone()).collect();
        names.extend(costs.values().filter_map(|c| c.worker.clone()));
        names.extend(
            events.iter().filter(|e| matches!(e.kind.as_str(), "done" | "lease")).filter_map(|e| e.worker.clone()),
        );
        // (Its other slots, a lane each under its slot's name (`slot_worker`): their jobs beside the
        // first's, at about four fifths of its pace until measured.)
        names.retain(|n| !is_slot_of(n, &self.host));
        let others = self.worker_of(1);
        // (Never fewer lanes than its last running slot's number: slots may be sparse.)
        let running: Vec<usize> = (0..self.inp.slots.len()).filter(|&k| self.inp.slots[k].running.is_some()).collect();
        let own_lanes = forecast::slot_lanes(self.job_slots(), &running);
        let own_names: Vec<String> = (1..own_lanes.max(2)).map(|k| self.worker_of(k)).collect();
        let names: Vec<String> = names.into_iter().collect();
        let mut speeds = forecast::speeds(self.o.the_plan, &events, &self.host, &names, 0.5);
        speeds.extend(forecast::speeds(self.o.the_plan, &events, &self.host, &own_names, 0.8));
        // (A time measured on a member, at the lead's pace; one measured here or by a page, as it is.)
        let pace = |worker: Option<&str>| worker.and_then(|w| speeds.get(w)).map_or(1.0, |s| s.0);
        // (Each Mac's pace at each step, the coordinator's (crate::paces, task #926): a time put at
        // the lead's pace by its Mac's pace at its step; a Mac it doesn't know, by its speed.)
        let paces = self.coord.as_ref().map(|c| c.paces()).unwrap_or_default();
        let at_lead = |c: &crate::coord::Cost, step: &str| forecast::at_lead(c, step, &paces, &pace);
        // Each shared step's mean of the targets measured (a z6 tile's are kept by its target alone);
        // else what its jobs here took a target; else a first guess.
        // (Each measured the way its step runs now: crate::coord::cost_version. A z3 tile's tree
        // cover isn't a z6 tile's.)
        // (A step whose time is the work its target finds, crate::needs::PER_WORK, weighed by it: a
        // pyramids-fill z6 tile with nothing to fetch says nothing of one fetching for hours, task #830.)
        let sums = forecast::step_sums(self.o.the_plan, &costs, &at_lead);
        let own_secs = |step: &str| self.step_secs_here(step).unwrap_or_else(|| first_secs(step));
        let per =
            |step: &str| -> f64 { sums.get(step).filter(|s| s.1 > 0.0).map_or_else(|| own_secs(step), |s| s.0 / s.1) };
        // (Its memory by its step's model, crate::needs: its own measure the way the step runs now,
        // else its step's fit or largest, else `peak`'s first guess.)
        let book = crate::needs::Book::new(self.o.the_plan, &costs);
        let need_mb = |step: &str, t: &str| book.need(step, t, 0, peak(step, t)).mem_mb;
        let cost = |step: &str, t: &str| forecast::target_cost(&book, step, t, &at_lead, &per, need_mb(step, t));
        // (A drop: its own run's measure, else its step's drops', else any step's drops', else as a
        // target of its step until a drop has run: crate::needs::DROPS, Book::drop_need.)
        let drop_cost = |step: &str, t: &str| -> Cost {
            match book.own(step, t).filter(|c| crate::needs::is_drop(c)) {
                Some(c) => Cost { secs: c.secs as f64, known: true, peak_mb: c.peak_mb },
                None => match book.drop_need(step) {
                    Some((secs, mb)) => Cost { secs: secs as f64, known: true, peak_mb: mb },
                    None => cost(step, t),
                },
            }
        };
        let drops: Vec<forecast::Job> =
            drops.iter().map(|(step, t)| (step.clone(), t.clone(), drop_cost(step, t))).collect();
        // The lead's own steps: what a target took here lately (measured), else a first guess.
        // (A PER_WORK step's, its costs' weighed time, as its jobs' times here mix in those that
        // found nothing to do.)
        let mine = |step: &str, n: usize| -> Cost {
            let (each, known) = match sums.get(step).filter(|s| s.1 > 0.0 && is_per_work(step)) {
                Some(s) => (s.0 / s.1, true),
                None => self.step_secs_here(step).map_or((first_secs(step), false), |t| (t, true)),
            };
            Cost { secs: each * n.max(1) as f64, known, peak_mb: 0 }
        };
        let step_of = |id: &str| id.split(' ').next().unwrap_or("").to_string();
        // (Each job before the regions' is a step of its own; one running now is its slot's
        // running item below, its time left its slot's.)
        let before: Vec<forecast::Job> = before
            .iter()
            .map(|j| {
                (step_of(&j.id), j.id.clone(), mine(&step_of(&j.id), j.record.as_ref().map_or(1, |w| left_of(w).len())))
            })
            .collect();
        // (A region's map tiles each at its measured time, `cost`; the whole-map steps by their
        // cadence, as last run.)
        let [rail, landmarks, buildings, depth, polar] = chains;
        let cadence = {
            let c = self.catalogues.borrow();
            let now = now_s();
            forecast::Cadence {
                since_low: self.since_low(now),
                since_marks: c.since("marks", now),
                since_overlays: c.since("overlays", now),
            }
        };
        // The trains' and the landmarks' chains from the start (their steps once what they read is
        // built: forecast::waits_of), but the overlays (they read the built z6 tiles) after everything
        // else; and a catalogue after them with what the chains made since.
        let (mut chain_jobs, mut after): (Vec<forecast::Job>, Vec<forecast::Job>) = (Vec::new(), Vec::new());
        for w in rail.iter().chain(landmarks.iter()).chain(buildings.iter()).chain(depth.iter()).chain(polar.iter()) {
            let jobs: Vec<forecast::Job> = if self.o.the_plan.shared().contains(&w.step.as_str()) {
                w.targets.iter().map(|t| (w.step.clone(), t.0.clone(), cost(&w.step, &t.0))).collect()
            } else {
                vec![(
                    w.step.clone(),
                    w.targets.first().map(|t| t.0.clone()).unwrap_or_default(),
                    mine(&w.step, w.targets.len()),
                )]
            };
            if w.step == "overlays" {
                after.extend(jobs)
            } else {
                chain_jobs.extend(jobs)
            }
        }
        if !rail.is_empty() || !landmarks.is_empty() || !buildings.is_empty() || !depth.is_empty() || !polar.is_empty()
        {
            after.push(("catalog".into(), "after the chains".into(), mine("catalog", 1)));
        }
        // Each slot's job's time left: its pace says only its part's; its targets not yet done, as
        // they took last time, less what it spent on the one under way, if longer.
        let busy =
            |k: usize, speed: f64| self.slot_job(k).map_or(0.0, |j| j.busy_s(&|step, t| cost(step, t).secs, speed));
        // (A worker that takes tails around: each of this Mac's z6 tiles the moment its job gives it to
        // take its tail, ::offload.)
        let unit_extra_s = if self.coord.as_ref().is_some_and(|c| c.tail_takers()) {
            ::offload::LEASE_WAIT.as_secs_f64()
        } else {
            0.0
        };
        // (A slot's job no item of the work left is: its step and targets, on its lane first.)
        let running_of = |k: usize| {
            self.inp
                .slots
                .get(k)
                .and_then(|s| s.running.as_ref())
                .map(|r| (step_of(&r.spec.id), r.spec.record.as_ref().map_or(1, |w| w.targets.len().max(1))))
        };
        // Its other slots: a lane each, as many as it may run now (its ramp's: `forecast::slot_lanes`),
        // and those its ramp grows to over the forecast, one each clean stretch from when it next
        // grows (`RAMP_CLEAN_S`; within two days); each any work of the plan that fits its Mac's room
        // beside the others (`forecast::MacRoom`).
        let mac_mb = (self.inp.mac.resources(&self.o.home, None, None, None).mem_gb * 1024.0) as u64;
        let mut rooms: BTreeMap<String, forecast::MacRoom> = BTreeMap::new();
        // (A Mac's room is the memory its jobs may hold together above its floor now
        // (`build_room`, memguard::Reading::admit_mb), as `pack_why` takes an ask's memory: a member's `Ask::mem_mb` is it,
        // not a slot's; the coordinator's worker record keeps it (`mem`). This Mac's, its own.)
        rooms.insert(self.host.clone(), forecast::MacRoom { mem_mb: self.build_room(), cores: mac_cores() });
        let mut machines = vec![Machine {
            name: self.host.clone(),
            speed: 1.0,
            measured: true,
            mem_mb: u64::MAX,
            busy_s: busy(0, 1.0),
            unit_extra_s,
            lead_steps: true,
            running: running_of(0),
            mac: Some(self.host.clone()),
            from_s: 0.0,
        }];
        let grows = {
            let reached = self.inp.mem.ramp.slots.max(RAMP_START);
            let clean = self.inp.mem.ramp.clean_since;
            let most = job_slots(mac_cores() as usize);
            (!self.inp.ramp_held && self.inp.ramp_trouble.is_none() && clean > 0 && reached < most).then(|| {
                let first = (clean + RAMP_CLEAN_S).saturating_sub(now_s()) as f64;
                (0..most - own_lanes.min(most))
                    .map(|j| first + (j as u64 * RAMP_CLEAN_S) as f64)
                    .take_while(|&s| s < 2.0 * 86_400.0)
                    .collect::<Vec<f64>>()
            })
        }
        .unwrap_or_default();
        for k in 1..own_lanes + grows.len() {
            let name = self.worker_of(k);
            let (speed, measured) = speeds.get(&name).or_else(|| speeds.get(&others)).copied().unwrap_or((0.8, false));
            machines.push(Machine {
                name,
                speed,
                measured,
                mem_mb: mac_mb,
                // (With each Mac's pace, the lead's slots are all at the lead's: crate::paces.)
                busy_s: if k >= self.inp.slots.len() {
                    0.0
                } else if paces.known() {
                    busy(k, 1.0)
                } else {
                    busy(k, speed)
                },
                unit_extra_s,
                lead_steps: true,
                running: running_of(k),
                mac: Some(self.host.clone()),
                from_s: if k < own_lanes { 0.0 } else { grows[k - own_lanes] },
            });
        }
        // Each member's slots, a lane each, as many as it may run (crate::agent::job_slots of its
        // cores as the lead's coordinator last heard them, else one per job it runs), drawing on its
        // whole memory and cores together (`forecast::MacRoom`); each any work of the plan any Mac
        // may run that fits, as the coordinator packs its asks (task #383).
        let caps: BTreeMap<String, crate::coord::Capacity> =
            self.coord.as_ref().map(|c| c.capacities().into_iter().collect()).unwrap_or_default();
        for h in &helpers {
            // (A member paused alone (jobrt::control::Hold) has no lanes: its jobs are given back at
            // their next safe point, and it takes nothing new until it's resumed.)
            if h.held.is_some() {
                continue;
            }
            // (Each with its slot: an older agent's, its place in the list.)
            let jobs = h.jobs();
            let cap = caps.get(&h.host);
            // (Never fewer lanes than its last running slot's number: slots may be sparse.)
            let running: Vec<usize> = jobs.iter().filter_map(|j| j.slot).collect();
            let n = match cap {
                Some(c) => forecast::slot_lanes(job_slots(c.cores as usize), &running),
                None => running.iter().map(|k| k + 1).max().unwrap_or(1),
            };
            // (Its room: the memory its jobs may hold together above its floor (`Ask::mem_mb`, as
            // `pack_why` counts it), else its capacity's (`build_mb`; an older app's: its memory
            // guard's limit).)
            let mem_all = mem
                .get(&h.host)
                .copied()
                .filter(|&m| m > 0)
                .or(cap.map(|c| if c.build_mb > 0 { c.build_mb } else { c.limit_mb }).filter(|&m| m > 0));
            rooms.insert(
                h.host.clone(),
                forecast::MacRoom {
                    mem_mb: mem_all.unwrap_or(0),
                    cores: cap.map_or(h.resources.as_ref().map_or(8, |r| r.cores as u32), |c| c.cores),
                },
            );
            for k in 0..n {
                let name = slot_worker(&h.host, k);
                let (speed, measured) =
                    speeds.get(&name).or_else(|| speeds.get(&h.host)).copied().unwrap_or((0.5, false));
                // (A time at the lead's pace at its Mac's at the step (crate::paces: a Mac without
                // one the lead's), else at its speed.)
                let at_its = |step: &str, secs: f64| {
                    if paces.known() {
                        secs * paces.get(&name, step)
                    } else {
                        secs / speed.max(0.1)
                    }
                };
                // (Its lease's targets at its pace, less the time since it took them, if longer
                // than its job's own estimate.)
                let lease_left = leased
                    .iter()
                    .filter(|l| l.0 == name)
                    .map(|(_, step, ts, age, _)| {
                        at_its(step, ts.iter().map(|t| cost(step, t).secs).sum::<f64>()) - *age as f64
                    })
                    .fold(0.0, f64::max);
                let job = jobs.iter().find(|j| j.slot == Some(k));
                // (No progress said: its step's measured time a target, at its pace.)
                let eta = job.map(|j| {
                    j.progress
                        .as_ref()
                        .and_then(|p| p.eta_s)
                        .map_or_else(|| at_its(&step_of(&j.id), per(&step_of(&j.id))), |e| e as f64)
                });
                let busy_s = eta.map_or(0.0, |e| forecast::busy_s(e, lease_left));
                let targets = leased.iter().find(|l| l.0 == name).map_or(1, |l| l.2.len().max(1));
                machines.push(Machine {
                    name,
                    speed,
                    measured,
                    mem_mb: mem_all.unwrap_or(6144 * n as u64),
                    busy_s,
                    unit_extra_s: 0.0,
                    lead_steps: false,
                    running: job.map(|j| (step_of(&j.id), targets)),
                    mac: Some(h.host.clone()),
                    from_s: 0.0,
                });
            }
        }
        // (This Mac paused alone while it still leads (task #628): no lanes of its own either; it
        // grants itself nothing new until it's resumed, task #224.)
        if self.hold().held.is_some() {
            machines.retain(|m| m.mac.as_deref() != Some(self.host.as_str()));
        }
        // What's being built now, and by which.
        let mut running: BTreeMap<(String, String), usize> = BTreeMap::new();
        for k in 0..self.inp.slots.len() {
            let Some(m) = machines.iter().position(|m| m.name == self.worker_of(k)) else { continue };
            let Some(r) = self.inp.slots[k].running.as_ref() else { continue };
            if let Some(b) = before.iter().find(|b| b.0 == step_of(&r.spec.id)) {
                running.insert((b.0.clone(), b.1.clone()), m);
            }
            if let Some(w) = r.spec.record.as_ref() {
                running.extend(w.targets.iter().map(|t| ((w.step.clone(), t.0.clone()), m)));
            }
        }
        // (Each lease on its slot's lane, or one not said yet on a lane of its Mac running its step,
        // `forecast::lease_lanes`. A target a slot's own record put on its lane stays there.)
        let keys: Vec<(&str, &str, bool)> = leased.iter().map(|l| (l.0.as_str(), l.1.as_str(), l.4)).collect();
        for ((_, step, targets, _, _), m) in leased.iter().zip(forecast::lease_lanes(&machines, &keys)) {
            let Some(m) = m else { continue };
            for t in targets {
                running.entry((step.clone(), t.clone())).or_insert(m);
            }
        }
        // (The z6 tiles' mids made in idle time, last.)
        let idle: Vec<forecast::Job> = backfill
            .iter()
            .flat_map(|w| w.targets.iter().map(|(t, _)| (w.step.clone(), t.clone(), cost(&w.step, t))))
            .collect();
        let mut inp = forecast::Input {
            now: now_s(),
            before,
            drops,
            costs: forecast::cost_table(&regions, &cost),
            regions: std::borrow::Cow::Owned(regions),
            cadence,
            blind,
            chains: chain_jobs,
            after,
            idle,
            machines,
            running,
            macs: rooms,
            sched: self.coord.as_ref().map(|c| c.sched_state()),
            pace: forecast::pace(&self.curves.borrow(), &events, now_s()),
            step_pace: BTreeMap::new(),
            paused: self.inp.pause.is_some(),
            along: left.along.clone(),
            code: forecast::VERSION,
            // (The targets an input of which can't be worked out yet wait for what works it out,
            // as the plan holds them: the slope tiles reading the terrain's root while it's stale.)
            unknown: left
                .unknown_for
                .iter()
                .flat_map(|(name, u)| {
                    self.o.the_plan.input_readers(name).into_iter().map(move |step| forecast::Unknown {
                        step: step.to_string(),
                        targets: u.targets.iter().cloned().collect(),
                        until: (u.until.0.to_string(), u.until.1.clone(), mine(u.until.0, 1)),
                    })
                })
                .collect(),
        };
        // (Each step's pace: its jobs of late against what the forecasts before them gave their
        // targets, at the lead's pace by their worker's speed.)
        let mut noted = self.noted.borrow_mut();
        noted.note(self.o.the_plan, &inp);
        inp.step_pace = forecast::step_pace(&events, inp.now, &noted, &|w, step| {
            if paces.known() {
                1.0 / paces.get(w, step)
            } else {
                pace(Some(w))
            }
        });
        drop(noted);
        // (Each lane's job under way held to its step's longest of late.)
        forecast::cap_busy(&mut inp, &forecast::longest_jobs(&events, now_s()));
        // (Each step's time a target, for the page to weigh the targets done: the headline.)
        let each_s =
            self.o.the_plan.step_names().into_iter().map(|n| (n.to_string(), (per(n) * 10.0).round() / 10.0)).collect();
        // (A slot's job waiting on a worker holding its z6 tile's tail: said beside its machine.)
        let waiting =
            (0..self.inp.slots.len()).filter_map(|k| Some((self.worker_of(k), self.waiting_words(k)?))).collect();
        forecast::Record { input: inp, each_s, waiting }
    }

    /// Runs `record`'s forecast on a thread of its own (task #544: the scheduler's run at live
    /// scale took the loop for a quarter hour, 21 Oct): the loop reaps, starts jobs and writes the
    /// status meanwhile, and takes it in once it's done (`take_forecast`). Its record's bytes are
    /// made there too (as data, for the NAS: written by the lead's pool step, `pool_lead_files`).
    pub(super) fn start_forecast(&self, record: forecast::Record<'static>) {
        let app = self.app.clone();
        let coord = self.coord.clone();
        let host = self.host.clone();
        let the_plan = self.o.the_plan;
        let run = std::thread::Builder::new().name("forecast".into()).spawn(move || {
            forecast::low_priority();
            let took = std::time::Instant::now();
            let mut f = record.forecast(the_plan);
            let entry = fcarchive::Entry::of(&record, &f, &app);
            // (Each Mac's tiles to come, for the page's map (`/api/tocome`), and the schedule's runs
            // (their targets and dependencies); the rest of `placed` the archive's alone: neither
            // kept with the forecast the agent holds.)
            let (mut tocome, item_job, jobs) = forecast::ToCome::take(&mut f, &record.input.machines);
            // (What its items wait on as the history's, worked out here: off the loop; each run's
            // finished dependencies from it, for the page's arrows.)
            let waits = coord.map(|c| c.waits_of(std::mem::take(&mut f.graph)));
            if let Some(w) = &waits {
                tocome.runs.after = w.by_job(&item_job, jobs);
            }
            // (This Mac's own, as cache hints: `upcoming`.)
            let upcoming = tocome.macs.get(&host).map(|m| (tocome.at, room::Hints::of_tocome(the_plan, m, tocome.at)));
            Forecasted { forecast: f, bytes: record.bytes(), entry, tocome, upcoming, waits, took: took.elapsed() }
        });
        match run {
            Ok(h) => *self.forecasting.borrow_mut() = Some(h),
            Err(e) => eprintln!("agent: the forecast's thread: {e}"),
        }
    }

    /// The forecast run off the loop (`start_forecast`), taken in once it's done: the status's from
    /// then, its record's bytes for the NAS. A fault in it costs the status its forecast, never the
    /// agent.
    pub(super) fn take_forecast(&self) {
        if !self.forecasting.borrow().as_ref().is_some_and(|h| h.is_finished()) {
            return;
        }
        let Some(h) = self.forecasting.borrow_mut().take() else { return };
        match h.join() {
            Ok(done) => {
                // (Its time, in the log.)
                eprintln!(
                    "agent: the forecast took {} ms{}",
                    done.took.as_millis(),
                    if done.forecast.partial { " (cut at its time budget: partial)" } else { "" }
                );
                // (Its input and archive entry handed to **jobs**, whose pool step writes them.)
                let bytes = done.bytes.map_err(|e| eprintln!("agent: the forecast's input as data: {e:#}")).ok();
                self.out.send(Out::Forecast(bytes, done.entry)).ok();
                *self.upcoming.borrow_mut() = done.upcoming.map(|(at, h)| (at, std::sync::Arc::new(h)));
                if let Some(c) = &self.coord {
                    c.set_tocome(std::sync::Arc::new(done.tocome));
                    if let Some(w) = done.waits {
                        c.set_waits(w);
                    }
                }
                let f = &done.forecast;
                let mut curves = self.curves.borrow_mut();
                curves.retain(|c| c.0 + forecast::PACE_MAX_S > f.at);
                if !f.curve.is_empty() && curves.last().is_none_or(|c| c.0 + forecast::CURVE_S <= f.at) {
                    curves.push((f.at, f.curve.clone()));
                }
                drop(curves);
                *self.forecast.borrow_mut() = Some(done.forecast);
            }
            Err(_) => {
                eprintln!("agent: the forecast failed; the status goes without it");
                *self.forecast.borrow_mut() = None;
            }
        }
    }

    /// The build to the end (the status's checklist): a line per stage (crate::agent::steps::STAGES)
    /// with an item per step counted: the OSM pass (the planet, fetched and read when a newer one
    /// is declared, and its worldwide steps), then the regions' steps (build::checklist, the last plan's:
    /// `regions_checklist`).
    pub(super) fn checklist(&self, root: &store::nasfs::NasRoot, regions: &[recipes::Recipe]) -> Vec<progress::Line> {
        let mut items = Vec::new();
        let mut next: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut notes: BTreeMap<&str, String> = BTreeMap::new();
        let have = self.latest_pass(root);
        // The planet: a newer one declared is under way (fetched, then read; or waiting), else done.
        let newer = self.planet_next(root, have.as_deref(), &mut Vec::new()).is_some();
        items.push(progress::Item {
            step: "osm-pass".into(),
            left: Some((newer || have.is_none()) as usize),
            ..Default::default()
        });
        let Some(date) = have else {
            // Nothing to size the rest by until a pass is complete: its steps, to come.
            items.extend(
                self.o
                    .the_plan
                    .worldwide_steps()
                    .iter()
                    .map(|s| progress::Item { step: s.to_string(), ..Default::default() }),
            );
            items.extend(self.o.the_plan.checklist_to_come());
            return self.o.the_plan.fold(&items, &next, &notes);
        };
        let b = self.brief_now(root).unwrap_or_default();
        let (manifest, keys) = (&b.manifest, &b.keys);
        // Countries the pass couldn't assemble, kept as the previous pass's outlines: said on the
        // pass's line until a pass has them whole (buildstore::outlines::assemble).
        if let Some(c) = manifest.get(&format!("sources/osm/{date}/outlines")) {
            if let Ok(stale) = buildstore::outlines::Outlines::stale_in(&root.join(c)) {
                if !stale.is_empty() {
                    notes.insert(
                        "pass",
                        format!(
                            "Outlines kept from an earlier pass (broken in this planet's OpenStreetMap): {}",
                            stale
                                .iter()
                                .map(|(id, from)| format!("relation {id} (the {from} pass's)"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    );
                }
            }
        }
        // The pass's worldwide steps, each one run left or none (held or not: build::pass_work's).
        let ask = seam::PassAsk { lakefill: inputs::lakefill::run::planned_source(root, &manifest), failures: &|_| 0 };
        let left = self.held_plan.pass_work(root, Some(&date), &manifest, &keys, ask, None).left;
        items.extend(self.o.the_plan.worldwide_steps().iter().zip(left).map(|(s, l)| progress::Item {
            step: s.to_string(),
            left: Some(l as usize),
            ..Default::default()
        }));
        next.insert(
            "pass",
            self.o
                .the_plan
                .worldwide_steps()
                .iter()
                .zip(left)
                .filter(|(_, l)| *l)
                .map(|(s, _)| s.to_string())
                .collect(),
        );
        // The regions' steps as the plan found them, with its keys (none while no plan got that far:
        // the coverage unread, say).
        if let Some((more, more_next)) = self.regions_checklist.borrow().clone().filter(|_| !regions.is_empty()) {
            items.extend(more);
            next.extend(more_next);
        }
        self.o.the_plan.fold(&items, &next, &notes)
    }

    /// Each region's steps (build::region_steps) over the keying context (the coverage, the pass, the
    /// manifest, the planner's inputs, the reaches, the terrain packs' indexes), the regions and the
    /// records, on a thread of its own (the loop gives it what it holds already, and a copy of the
    /// indexes), with how long it took. None before the first pass or while the coverage can't be
    /// read.
    #[allow(clippy::type_complexity)]
    pub(super) fn start_region_steps(
        &self,
        root: &store::nasfs::NasRoot,
    ) -> Option<std::thread::JoinHandle<(BTreeMap<String, BTreeMap<&'static str, [usize; 2]>>, Duration)>> {
        let date = self.latest_pass(root)?;
        let b = self.brief_now(root).unwrap_or_default();
        let (manifest, keys) = (b.manifest.clone(), b.keys.clone());
        let covs = self.coverage(root, &manifest, false).ok()?;
        let count = self.held_plan.region_counts(root, date, manifest, covs, keys);
        let run = move || {
            let t = Instant::now();
            (count(), t.elapsed())
        };
        std::thread::Builder::new()
            .name("region-steps".into())
            .spawn(run)
            .inspect_err(|e| eprintln!("agent: the region-steps thread: {e}"))
            .ok()
    }

    /// The coverage of the regions' accepted version, and each region's, as last made: made again
    /// only for a new version (an edit is one), and then from this Mac's cache (by the version) when
    /// it holds it, so a restart reads none of the geometry files off the NAS (#608). Made on a
    /// thread of its own, waited for a second as it starts: a longer read (a new version's geometry off the NAS)
    /// goes on while the loop steps and beats, an error saying so until it's done (the plan's
    /// regions wait for it). `edits`: a change of version is the owner's edit (the plan's look; not
    /// the heartbeat's, whose read may have failed).
    pub(super) fn coverage(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
        edits: bool,
    ) -> Result<std::sync::Arc<Coverages>, String> {
        // (The accepted version: keyed by its index's name, an edit being a new version.)
        let a = inputs::inputs::regions::read::source(root, manifest)
            .map_err(|e| format!("the regions' accepted version: {e:#}"))?;
        if edits {
            let e = a.version.clone();
            if self.edits.borrow().as_ref().is_some_and(|last| *last != e) {
                let now = std::time::SystemTime::now();
                let first = self
                    .edited_at
                    .get()
                    .filter(|(_, last)| last.elapsed().is_ok_and(|a| a < EDIT_HOLD))
                    .map_or(now, |(first, _)| first);
                self.edited_at.set(Some((first, now)));
            }
            *self.edits.borrow_mut() = Some(e);
        }
        if let Some((k, c)) = self.coverage.borrow().as_ref() {
            if *k == a.version {
                return Ok(c.clone());
            }
        }
        // (Another version's is never given again: let go of it before the new one is read, not
        // held beside it, ~4 GB for the world's, #619.)
        self.coverage.borrow_mut().take();
        let mut making = self.coverage_making.borrow_mut();
        let fresh = if making.as_ref().is_none_or(|(v, h)| *v != a.version && h.is_finished()) {
            let (root, cache, a, v) = (root.long_retry(), self.o.home.join("cache"), a.clone(), a.version.clone());
            let run = std::thread::Builder::new().name("coverage".into()).spawn(move || {
                let all = inputs::inputs::regions::read::accepted_coverage(&root, &a, Some(&cache))
                    .map_err(|e| format!("{e:#}"))?;
                Ok(Coverages { each: all.by_region(), all })
            });
            *making = Some((v, run.map_err(|e| format!("the coverage's thread: {e}"))?));
            true
        } else {
            false
        };
        let Some((v, h)) = making.as_ref() else { unreachable!() };
        if *v != a.version {
            return Err(format!("reading the regions' coverage (of {v}, then {})", a.version));
        }
        // (Waited for a second when just started; after, only looked at.)
        let until = Instant::now() + Duration::from_secs(if fresh { 1 } else { 0 });
        while !h.is_finished() {
            if Instant::now() >= until {
                return Err(format!("reading the regions' coverage ({})", a.version));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let (_, h) = making.take().unwrap();
        let c = std::sync::Arc::new(h.join().map_err(|_| "reading the regions' coverage failed".to_string())??);
        *self.coverage.borrow_mut() = Some((a.version.clone(), c.clone()));
        Ok(c)
    }

    /// The build page's regions (crate::agent::regionsview), made again when the regions' accepted
    /// version `a` or the map's `seq` (the status's: `map_now`) changes, once each (a failed read isn't
    /// tried again until one of them changes), on a thread of its own (`regionsview::build`: the
    /// drawn files, the catalogue's sizes once per catalogue). Given to this Mac's coordinator (the
    /// page out of the pool) once made; the lead's pool step writes them for the others'
    /// (`pool_lead_files`).
    pub(super) fn regions_view(
        &self,
        root: &store::nasfs::NasRoot,
        a: &inputs::inputs::Accepted,
        regions: &[recipes::Recipe],
    ) {
        self.take_regions();
        let cat = self.map_now();
        let key: RegionsKey = (a.version.clone(), cat.as_ref().map(|c| c.n));
        if self.regions_making.borrow().is_some() || self.regions_tried.borrow().as_ref() == Some(&key) {
            return;
        }
        let (root, a, regions, had) =
            (root.long_retry(), a.clone(), regions.to_vec(), self.region_bytes.borrow().clone());
        let run = std::thread::Builder::new()
            .name("regions-view".into())
            .spawn(move || regionsview::build(&root, &a, &regions, cat, had));
        match run {
            Ok(h) => {
                *self.regions_making.borrow_mut() = Some(h);
                *self.regions_tried.borrow_mut() = Some(key);
            }
            Err(e) => eprintln!("agent: the build page's regions' thread: {e}"),
        }
    }

    /// The build page's regions made off the loop (`regions_view`), taken in once done.
    pub(super) fn take_regions(&self) {
        if !self.regions_making.borrow().as_ref().is_some_and(|h| h.is_finished()) {
            return;
        }
        let Some(h) = self.regions_making.borrow_mut().take() else { return };
        let Ok(made) = h.join() else {
            eprintln!("agent: making the build page's regions failed");
            return;
        };
        eprintln!("agent: the build page's regions took {} ms", made.took.as_millis());
        if made.bytes.is_some() {
            *self.region_bytes.borrow_mut() = made.bytes;
        }
        let Some(v) = made.view else { return };
        let v = std::sync::Arc::new(v);
        let key: RegionsKey = (v["version"].as_str().unwrap_or_default().to_string(), v["catalog"].as_u64());
        if let Some(c) = &self.coord {
            c.set_regions(v.clone());
        }
        *self.regions_view.borrow_mut() = Some((key, v));
    }

    /// The regions the map has (`map`, as `map_now` gives it), or the newest held map's while
    /// publishing is held for review (`held`, once this agent's held maps have one: publish::held_dir), by
    /// id: whether as its recipe is now (same outline entries); and how long ago it went out
    /// (seconds: its batch's time, the held one's). Its regions taken once per `seq`; none (nothing
    /// on the map: every region new) when there's none, or it can't be read now.
    pub(super) fn on_map(
        &self,
        held: bool,
        map: Option<&store::catalogue::Catalogue>,
        recipes: &[recipes::Recipe],
    ) -> (BTreeMap<String, bool>, Option<u64>) {
        // (Each region's id and outline entries, read from its text: an object's, both there.)
        #[derive(serde::Deserialize)]
        struct Head {
            id: String,
            outline: Vec<String>,
        }
        let regions_of = |c: &store::catalogue::Catalogue| -> BTreeMap<String, Vec<String>> {
            c.coverage
                .region_texts()
                .iter()
                .filter(|r| r.starts_with('{'))
                .filter_map(|r| serde_json::from_str::<Head>(r).ok())
                .map(|h| (h.id, h.outline))
                .collect()
        };
        let mut cached = self.last_catalogue.borrow_mut();
        let mut at = None;
        // (The held maps, once there are any: the newest read whole once.)
        let held_dir = publish::held_dir(&self.o.home);
        let newest_held =
            if held { store::map::seqs_in(&held_dir).ok().and_then(|ns| ns.first().copied()) } else { None };
        if let Some(n) = newest_held {
            let key = (publish::HELD.to_string(), n);
            if cached.as_ref().is_none_or(|c| c.0 != key) {
                let read = std::fs::read(held_dir.join(store::map::snapshot_name(n)))
                    .map_err(anyhow::Error::from)
                    .and_then(|b| store::map::Snapshot::decode(&b))
                    .and_then(|(m, _)| m.catalogue());
                match read {
                    Ok(c) => *cached = Some((key.clone(), (regions_of(&c), seen(&c).at))),
                    Err(e) => eprintln!("agent: held map {n}: {e:#}"),
                }
            }
            at = cached.as_ref().filter(|c| c.0 == key).map(|c| std::time::UNIX_EPOCH + Duration::from_secs(c.1 .1));
        } else if let Some(c) = map {
            let key = ("map".to_string(), c.n);
            if cached.as_ref().is_none_or(|k| k.0 != key) {
                *cached = Some((key, (regions_of(c), seen(c).at)));
            }
            at = Some(std::time::UNIX_EPOCH + Duration::from_secs(seen(c).at));
        }
        // How long ago the last catalogue went out, or one last started (it may have failed): a
        // time ahead of this Mac's clock counts as now.
        let now = std::time::SystemTime::now();
        let since = |t: std::time::SystemTime| now.duration_since(t).map_or(0, |d| d.as_secs());
        let started = (self.inp.mem.catalogue_at > 0).then(|| now_s().saturating_sub(self.inp.mem.catalogue_at));
        let since_publish = match (at.map(since), started) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if at.is_none() {
            return (BTreeMap::new(), since_publish);
        }
        let had = &cached.as_ref().unwrap().1 .0;
        let on_map = had
            .iter()
            .map(|(id, outline)| (id.clone(), recipes.iter().any(|r| &r.id == id && &r.outline == outline)))
            .collect();
        (on_map, since_publish)
    }

    /// What the lead's plan does with the OSM pass of `newer` (its planet and date), resuming on
    /// (crate::agent::resume::decide): None to plan it here as before; else why not, the pass
    /// offered meanwhile to the one member whose Mac holds its progress (the coordinator's
    /// `offer_resume`, none otherwise).
    pub(super) fn resume_hold(&self, newer: Option<&(NasPath, String)>) -> Option<Waiting> {
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
        let me = self.inp.member_id.clone()?;
        let id = format!("osm-pass {date}");
        let what = format!("Reading the OpenStreetMap planet of {date}");
        // (The members' progress as the lead keeps it from their beats, its own among them, with
        // the owner's releases.)
        let kept = self.coord.as_ref().map(|c| c.progress()).unwrap_or_default();
        let mut gone: BTreeSet<(String, u64)> = BTreeSet::new();
        let mut files = Vec::new();
        for k in kept {
            gone.extend(k.released.iter().filter(|(j, _)| *j == id).map(|(_, at)| (k.progress.member.clone(), *at)));
            files.push(k.progress);
        }
        // A member running it under a lease of this coordinator: not run here too, whatever its
        // file says now (released, its day out, or read stale meanwhile).
        if self.coord.as_ref().is_some_and(|c| c.held("osm-pass").contains(date)) {
            return Some(Waiting {
                step: Some("osm-pass".into()),
                what,
                why: "running on a member under its lease (its progress is on that Mac)".into(),
            });
        }
        let now = self.inp.resume_now;
        match resume::decide(&id, &me, &files, now, &|m, at| gone.contains(&(m.to_string(), at))) {
            resume::Decision::Plan => {
                offer(None);
                // (A member that noted it lately may be running it under a lease the restored
                // state lacks: not run here too until its note is old.)
                resume::running_elsewhere(&id, &me, &files, now).map(|p| Waiting {
                    step: Some("osm-pass".into()),
                    what,
                    why: format!(
                        "may be running on {} ({}): its progress noted {} min ago",
                        p.worker,
                        p.member,
                        now.saturating_sub(p.jobs[&id].at) / 60
                    ),
                })
            }
            resume::Decision::Offer { member, worker, until } => {
                let key = planet.rel().to_string();
                offer(Some((
                    worker.clone(),
                    crate::coord::Offer { step: "osm-pass".into(), targets: vec![(date.clone(), key, 0)], job: None },
                )));
                let h = until.saturating_sub(now).div_ceil(3600);
                Some(Waiting { step: Some("osm-pass".into()), what, why: format!("its progress is on {worker}'s disk ({member}): offered to it alone for {h} h more (a member asleep or away holds it so), or until the owner releases it (scenic pool release {member} \"{id}\")") })
            }
            resume::Decision::Wait(why) => {
                offer(None);
                Some(Waiting { step: Some("osm-pass".into()), what, why })
            }
        }
    }

    /// The gate's part of the plan (the lead's, a dry run's): the listing thread started, and the
    /// asks of it taken up: a listing now (and a full check), and acceptances, which this Mac
    /// writes itself (docs/inputs.md §4.5). A member's go to the lead (`Agent::tell_gate`).
    pub(super) fn tend_gate(&mut self, root: &store::nasfs::NasRoot) {
        if self.inputs.watch.as_ref().is_none_or(|(r, _)| r != root) {
            self.inputs.watch =
                Some((root.clone(), inputs::inputs::watch::Watch::start(root.clone(), inputs::inputs::watch::EVERY)));
        }
        // (Not a dry run's: the asks are the agent's that runs.)
        if self.o.dry_run {
            return;
        }
        let mut asks = inputs::inputs::take_asks(&self.o.home);
        if let Some(c) = &self.coord {
            asks.extend(c.take_inputs_asks());
        }
        if asks.is_empty() {
            return;
        }
        let member = inputs::inputs::member_of(&self.o.home);
        for a in asks {
            let by = if a.by.is_empty() { "someone" } else { &a.by };
            if a.forget {
                self.inputs.forget.borrow_mut().insert(a.unit.clone());
                eprintln!("agent: {by} asked to forget {}'s records", a.unit);
                continue;
            }
            if a.check || a.full {
                if a.full {
                    self.inputs.full.borrow_mut().insert(a.unit.clone());
                }
                eprintln!("agent: {by} asked to check {}{}", a.unit, if a.full { " in full" } else { "" });
            }
            if a.all || !a.accept.is_empty() || !a.unaccept.is_empty() {
                // The unit's held report as the last plan read it, else as the records name it now.
                let shown =
                    self.inputs.view.borrow().iter().find(|v| v.unit == a.unit).map(|v| inputs::inputs::Report {
                        findings: v.findings.iter().map(|s| s.finding.clone()).collect(),
                        ..Default::default()
                    });
                apply_gate_ask(root, &a, shown, &member);
            }
        }
        if let Some((_, w)) = &self.inputs.watch {
            w.ask();
        }
        // (An ask taken: planned again at once, its check among the jobs.)
        self.kicked.get_or_insert("a gate ask");
    }

    /// The gate's checks to run (docs/inputs.md §4.3): each unit on the gate whose check's key
    /// (its listing, acceptances, accepted index) isn't the one last recorded, a full check daily
    /// or when asked; and the status's entries, from the records it plans with.
    pub(super) fn gate_work(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
        keys: &crate::pool::records::Keys,
        waiting: &mut Vec<Waiting>,
    ) -> Vec<JobSpec> {
        let units = inputs::inputs::units(root);
        let mut jobs = Vec::new();
        let mut checking: BTreeSet<String> = BTreeSet::new();
        // (The listing files a check may still read, `drop_unnamed`: this plan's, the last plan's
        // (granted from until this one's offers replace its own), those of the jobs running here and
        // of every lease of the step, the coordinator's now (a grant made but not started yet).)
        let mut named: BTreeSet<String> = BTreeSet::new();
        let mut name = |w: &jobs::Work| {
            for (t, k) in w.targets.iter().filter(|_| w.step == "inputs") {
                named.extend(t.strip_prefix("inputs/").map(|u| listing_name(u, k)));
            }
        };
        for w in self.jobs.iter().chain(self.inp.slots.iter().filter_map(|s| s.running.as_ref().map(|r| &r.spec))) {
            if let Some(r) = &w.record {
                name(r);
            }
        }
        if let Some(c) = &self.coord {
            name(&jobs::Work { step: "inputs".into(), targets: c.leased_keyed("inputs") });
        }
        // (Running now, on this Mac.)
        for r in self.inp.slots.iter().filter_map(|s| s.running.as_ref()) {
            if let Some(w) = r.spec.record.as_ref().filter(|w| w.step == "inputs") {
                checking.extend(w.targets.iter().filter_map(|t| t.0.strip_prefix("inputs/").map(str::to_string)));
            }
        }
        if let Some((_, watch)) = &self.inputs.watch {
            let s = |p: &Path| p.to_string_lossy().into_owned();
            for unit in &units {
                let Some(checks) = inputs::inputs::checks(unit) else { continue };
                if let Some(why) = watch.failed(unit) {
                    waiting.push(Waiting {
                        step: Some("inputs".into()),
                        what: format!("Checking {unit}"),
                        why: format!("its inputs folder can't be listed now: {why}"),
                    });
                }
                let Some(l) = watch.get(unit) else { continue };
                // (Nothing added yet and nothing ever taken in or held: nothing to check.)
                let empty = l.listing.files.is_empty() && l.listing.strays.is_empty() && l.listing.misplaced.is_empty();
                if empty
                    && [inputs::inputs::logical(unit), inputs::inputs::held_logical(unit)]
                        .iter()
                        .all(|r| !manifest.contains_key(r))
                {
                    continue;
                }
                // (Daily by the records' last full check, whichever Mac made it: a new lead doesn't
                // repeat one made less than a day ago.)
                let full_at = manifest.get(&inputs::inputs::listed_logical(unit)).and_then(|n| {
                    let mut seen = self.inputs.full_at.borrow_mut();
                    *seen
                        .entry(n.clone())
                        .or_insert_with(|| inputs::inputs::read_listed_file(root, n).ok().and_then(|l| l.full_at))
                });
                let full = self.inputs.full.borrow().contains(*unit)
                    || full_at.is_none_or(|t| now_s().saturating_sub(t) >= 86400);
                let mut key = inputs::inputs::check_key(checks, &l.listing, &l.accepted, manifest);
                if full {
                    key = jobrt::jobs::h(&[&key, "full"]);
                }
                let target = format!("inputs/{unit}");
                if keys.lo.get(&target) == Some(&key) {
                    continue;
                }
                checking.insert(unit.to_string());
                // (The listing and acceptances the key is made from, for the job to check exactly
                // them: inputs::Planned. Written once: its name is its key's. It stays while a job
                // may read it, `named`: a folder changed meanwhile is planned again under another key.)
                let planned = self.o.home.join(LISTINGS).join(listing_name(unit, &key));
                named.insert(listing_name(unit, &key));
                let wrote = if planned.exists() {
                    Ok(())
                } else {
                    std::fs::create_dir_all(planned.parent().unwrap()).map_err(anyhow::Error::from).and_then(|()| {
                        store::whole::write(
                            &planned,
                            &serde_json::to_vec(&inputs::inputs::Planned {
                                listing: l.listing.clone(),
                                accepted: l.accepted.clone(),
                            })?,
                        )
                    })
                };
                if let Err(e) = wrote {
                    waiting.push(Waiting {
                        step: Some("inputs".into()),
                        what: format!("Checking {unit}"),
                        why: format!("its listing can't be kept for the check: {e:#}"),
                    });
                    continue;
                }
                let mut cmd = vec![
                    s(&self.o.bin.join("scenic-build")),
                    "inputs".into(),
                    "--root".into(),
                    jobs::root_arg(root),
                    "--scratch".into(),
                    s(&self.o.home.join("scratch").join("inputs")),
                    unit.to_string(),
                    "--listing".into(),
                    s(&planned),
                    "--dem".into(),
                    s(&self.o.bin.join("dem")),
                ];
                if full {
                    cmd.push("--full".into());
                }
                let id = if full { format!("inputs {unit} full") } else { format!("inputs {unit}") };
                jobs.push(JobSpec {
                    id,
                    what: format!("Checking the {unit} inputs{}", if full { ", every file" } else { "" }),
                    cmd,
                    needs: Needs { nas: true },
                    restart_after_sleep: true,
                    record: Some(jobs::Work { step: "inputs".into(), targets: vec![(target, key)] }),
                });
            }
        }
        // (Only by the agent that runs the jobs: a dry run beside it shares its home, not its leases.)
        if self.locked && !self.o.dry_run {
            drop_unnamed(&self.o.home.join(LISTINGS), &named);
        }
        // The regions' extract, when their check holds files waiting for ids it lacks (its held
        // report's `needs`, of the current pass): a job extracting them with those it has.
        if let Some(job) = self.extract_job(root, manifest, keys) {
            jobs.push(job);
        }
        // A unit off the gate whose records were asked to be forgotten (`scenic inputs test off
        // --forget`): a job that removes them from the records.
        let s = |p: &Path| p.to_string_lossy().into_owned();
        for unit in self.inputs.forget.borrow().iter() {
            let named = [
                inputs::inputs::logical(unit),
                inputs::inputs::held_logical(unit),
                inputs::inputs::listed_logical(unit),
            ]
            .iter()
            .any(|l| manifest.contains_key(l));
            if units.contains(&unit.as_str()) || !named || !inputs::inputs::valid_unit(unit) {
                continue;
            }
            let cmd = vec![
                s(&self.o.bin.join("scenic-build")),
                "inputs".into(),
                "--root".into(),
                jobs::root_arg(root),
                "--scratch".into(),
                s(&self.o.home.join("scratch").join("inputs")),
                unit.clone(),
                "--forget".into(),
            ];
            jobs.push(JobSpec {
                id: format!("inputs {unit} forget"),
                what: format!("Forgetting {unit}'s records"),
                cmd,
                needs: Needs { nas: true },
                restart_after_sleep: true,
                record: Some(jobs::Work {
                    step: "inputs".into(),
                    targets: vec![(format!("inputs/{unit}"), "forget".into())],
                }),
            });
        }
        let accepted = self.inputs.watch.as_ref().map(|(_, w)| w.accepted()).unwrap_or_default();
        *self.inputs.view.borrow_mut() =
            inputs::inputs::view::of(root, manifest, &units, &checking, &accepted, &mut self.inputs.cache.borrow_mut());
        jobs
    }

    /// The regions' extract job (docs/inputs.md §6.1.6), when the regions' held report needs ids of
    /// the current pass and the job for them (and those the extract has) isn't done.
    pub(super) fn extract_job(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
        keys: &crate::pool::records::Keys,
    ) -> Option<JobSpec> {
        use inputs::inputs::regions::extract;
        let held = manifest.get(&inputs::inputs::held_logical(inputs::inputs::regions::UNIT))?;
        let report = inputs::inputs::read_report(root, held).ok()?;
        let date = report.needs.first()?.strip_prefix("pass ")?.to_string();
        if inputs::osmpass::latest_pass_in(manifest).as_deref() != Some(date.as_str()) {
            return None;
        }
        let mut ids: BTreeSet<extract::Id> = report.needs.iter().skip(1).filter_map(|s| s.parse().ok()).collect();
        if ids.is_empty() {
            return None;
        }
        // (The extract's file unreadable now, not missing: no job, rather than one it may not need.)
        let had = match manifest.get(&extract::logical(&date)) {
            Some(n) => present_or_missing(nasread::read(&root.join(n)))?,
            None => None,
        };
        if let Some(e) = had.and_then(|b| extract::Extract::read(&b).ok()) {
            ids.extend(e.ids());
        }
        let key = extract::key(&date, &ids);
        let target = format!("regions-extract/{date}");
        if keys.lo.get(&target) == Some(&key) {
            return None;
        }
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let list: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        let cmd = vec![
            s(&self.o.bin.join("scenic-build")),
            "regions-extract".into(),
            "--root".into(),
            jobs::root_arg(root),
            "--scratch".into(),
            s(&self.o.home.join("scratch").join("regions-extract")),
            "--pass".into(),
            date.clone(),
            "--ids".into(),
            list.join(","),
        ];
        Some(JobSpec {
            id: format!("regions-extract {date}"),
            what: "Extracting the regions' OSM relations and ways".into(),
            cmd,
            needs: Needs { nas: true },
            restart_after_sleep: true,
            record: Some(jobs::Work { step: "regions-extract".into(), targets: vec![(target, key)] }),
        })
    }
}

/// The agent sampled (a plan stalled, decision 4) (`sample <pid> SAMPLE_SECS`) as a stall begins, in the agent's folder: one file
/// a stall, `sample_name`d by when it began; the last `SAMPLES_KEPT` stay (`prune_samples`).
pub const STALL_SAMPLE: &str = "plan-stall-sample";
/// How long the sampler samples the process (its symbolication, after, takes longer on a big, busy one).
const SAMPLE_SECS: u32 = 5;
/// How many stalls' samples stay in the agent's folder.
const SAMPLES_KEPT: usize = 5;

/// The sample file of a stall seen at `now` (s since the epoch): `plan-stall-sample-<UTC date>-<time>.txt`,
/// so names sort by time.
fn sample_name(now: u64) -> String {
    // (Days since the epoch to a civil date: Hinnant's algorithm.)
    let z = (now / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let d = now % 86400;
    format!("{STALL_SAMPLE}-{year:04}{month:02}{day:02}-{:02}{:02}{:02}Z.txt", d / 3600, d / 60 % 60, d % 60)
}

/// The agent's folder of the listings its checks read (`gate_work`).
const LISTINGS: &str = "inputs-listing";

/// The file in `LISTINGS` of unit `unit`'s listing keyed `key`, by the unit's flat name
/// (`timetables+gtfs`: a nested z6 tile's name is one file's).
fn listing_name(unit: &str, key: &str) -> String {
    format!("{}.{key}.json", inputs::inputs::flat(unit))
}

/// The files of `dir` not `named` removed (task #971: a job planned before its folder changed found
/// its listing gone, as each new key's file removed the unit's others). One read of the folder, on
/// the agent's own disk; what stays is what a job may read.
fn drop_unnamed(dir: &Path, named: &BTreeSet<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if !named.contains(&*e.file_name().to_string_lossy()) {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

/// The oldest samples in `home` removed past the newest `keep`, and the single file the sampler
/// wrote before samples were timestamped.
fn prune_samples(home: &Path, keep: usize) {
    std::fs::remove_file(home.join(format!("{STALL_SAMPLE}.txt"))).ok();
    let Ok(rd) = std::fs::read_dir(home) else { return };
    let prefix = format!("{STALL_SAMPLE}-");
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(".txt"))
        .collect();
    names.sort();
    for n in &names[..names.len().saturating_sub(keep)] {
        std::fs::remove_file(home.join(n)).ok();
    }
}

/// This process sampled by `sampler` for `SAMPLE_SECS` into a new file in `home` (`sample_name` of
/// `now`), off the watchdog's thread (waited for there, so it leaves no zombie). Never two at once:
/// while one runs (or hangs, until it's killed at `SAMPLE_LIMIT`), another stall's isn't started.
/// `sample` writes its file only once done, so a sampler killed at the limit leaves nothing (an
/// empty or missing file is removed). Its outcome and the file's path are logged.
fn sample_self(sampler: &Path, home: &Path, now: u64) {
    static SAMPLING: AtomicBool = AtomicBool::new(false);
    if SAMPLING.swap(true, Ordering::SeqCst) {
        eprintln!("agent: the stalled plan not sampled: the last stall's sampler still runs");
        return;
    }
    let out = home.join(sample_name(now));
    let child = store::spawn::spawn(
        store::spawn::command(sampler)
            .args([std::process::id().to_string(), SAMPLE_SECS.to_string(), "-mayDie".into(), "-file".into()])
            .arg(&out)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    );
    match child {
        Ok(mut c) => {
            let home = home.to_path_buf();
            std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let st = loop {
                    match c.try_wait() {
                        // (sleep ok: a poll of the sample's process until it ends or SAMPLE_LIMIT passes)
                        Ok(None) if t.elapsed() < SAMPLE_LIMIT => std::thread::sleep(Duration::from_millis(250)),
                        Ok(None) => {
                            c.kill().ok();
                            break c.wait().map(|_| None);
                        }
                        r => break r,
                    }
                };
                let kept = std::fs::metadata(&out).is_ok_and(|m| m.len() > 0);
                let file = format!("file {}: {}", out.display(), if kept { "kept" } else { "nothing written" });
                match st {
                    Ok(Some(st)) if st.success() => eprintln!("agent: the stalled plan sampled into {}", out.display()),
                    Ok(Some(st)) => eprintln!("agent: sampling the stalled plan: {st} ({file})"),
                    Ok(None) => eprintln!(
                        "agent: sampling the stalled plan: killed after {} s ({file})",
                        SAMPLE_LIMIT.as_secs()
                    ),
                    Err(e) => eprintln!("agent: sampling the stalled plan: {e}"),
                }
                if !kept {
                    std::fs::remove_file(&out).ok();
                }
                prune_samples(&home, SAMPLES_KEPT);
                SAMPLING.store(false, Ordering::SeqCst);
            });
        }
        Err(e) => {
            eprintln!("agent: sampling the stalled plan ({}): {e}", sampler.display());
            SAMPLING.store(false, Ordering::SeqCst);
        }
    }
}

/// A sampler still running this long after it started (it samples for `SAMPLE_SECS`, then
/// symbolicates: on the lead's idle agent that took 0.3 s past the 3 s sampled; a stalled, loaded
/// one needed more than the 60 s it was once given) is killed.
const SAMPLE_LIMIT: Duration = Duration::from_secs(300);

#[cfg(test)]
mod tests {
    use super::*;

    /// Task #1188: the jobs `plan_unchecked` makes besides the regions' work (`region_work`, what
    /// the forecast is made from) are of the steps the forecast declares it doesn't model
    /// (forecast::UNFORESEEN), and each step in that list is made there. Read from its source: each
    /// job put in its `out` is placed (a `JobSpec` written there, by the step its id starts with;
    /// the redos'; the regions' work; the regions' work put back after the input checks), and one
    /// put there any other way fails here, so a new duty can't pass unlisted.
    #[test]
    fn the_jobs_planned_outside_the_forecast_are_the_ones_it_says_it_doesnt_model() {
        let src = include_str!("plan.rs");
        let start = src.find("    pub(super) fn plan_unchecked(").expect("plan_unchecked");
        let body = &src[start..];
        let body = &body[..body[1..].find("\n    pub(super) fn ").expect("the next fn") + 1];
        let mut made: BTreeSet<String> = BTreeSet::new();
        let mut regions = 0;
        for (n, line) in body.lines().enumerate() {
            let Some(rest) = line.trim().strip_prefix("out").filter(|r| r.starts_with(['.', ' '])) else {
                continue;
            };
            let put = [".push(", ".extend(", " = "]
                .iter()
                .find_map(|p| rest.strip_prefix(p))
                .unwrap_or_else(|| panic!("the plan's jobs changed in a way the test can't place: {}", line.trim()));
            if put == "JobSpec {" {
                let id = body.lines().nth(n + 1).unwrap_or("").trim();
                let step = id
                    .strip_prefix("id: format!(\"")
                    .and_then(|r| r.split_once(' '))
                    .map(|(s, _)| s)
                    .or_else(|| id.strip_prefix("id: \"").and_then(|r| r.split_once('"')).map(|(s, _)| s));
                made.insert(step.unwrap_or_else(|| panic!("a job of a step not read here: {id}")).to_string());
            } else if put.starts_with("self.redo_jobs(") {
                made.insert(redo::STEP.to_string());
            } else if put.starts_with("self.region_work(") {
                regions += 1;
            } else if !matches!(put, "checks;" | "rest);") {
                panic!("a job put in the plan the test can't place: {}", line.trim());
            }
        }
        assert_eq!(regions, 1, "the regions' work: the forecast's");
        assert_eq!(made, forecast::UNFORESEEN.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>());
    }

    /// The plan is made again when something it depends on changed, it was kicked (a job's end, a
    /// gate ask), a timed wait ended, or its fallback is due; none made yet, at once. (Times passed
    /// in: no clock.)
    #[test]
    fn the_plan_is_made_on_changes() {
        let s = Duration::from_secs;
        let last = |age: u64, at: Option<u64>| Some((s(age), s(0), 7, at));
        assert_eq!(why_plan(None, None, 7, 1000), Some("no plan yet"));
        assert_eq!(why_plan(Some("a job ended"), last(0, None), 7, 1000), Some("a job ended"));
        // Nothing changed (paused with jobs left, say: the pause is in the digest): not at each
        // look; at the fallback.
        assert_eq!(why_plan(None, last(20, None), 7, 1000), None);
        assert_eq!(why_plan(None, last(FALLBACK.as_secs() - 1, None), 7, 1000), None);
        assert_eq!(why_plan(None, last(FALLBACK.as_secs(), None), 7, 1000), Some("the fallback"));
        // Something changed (the pause lifted): at once.
        assert_eq!(why_plan(None, last(0, None), 8, 1000), Some("what it depends on changed"));
        // A timed wait (a failure's, say) ending at 1100: not before, then.
        assert_eq!(why_plan(None, last(20, Some(1100)), 7, 1099), None);
        assert_eq!(why_plan(None, last(20, Some(1100)), 7, 1100), Some("a timed wait ended"));
        // A plan that took 15 s: no change taken up sooner than 15 s after it (half a core at most),
        // then at once; the fallback whatever.
        let took = |age: u64| Some((s(age), s(15), 7, None));
        assert_eq!(why_plan(Some("a job ended"), took(14), 8, 1000), None);
        assert_eq!(why_plan(Some("a job ended"), took(15), 8, 1000), Some("a job ended"));
        assert_eq!(why_plan(None, took(15), 8, 1000), Some("what it depends on changed"));
        assert_eq!(why_plan(None, Some((FALLBACK, FALLBACK * 2, 7, None)), 7, 1000), Some("the fallback"));
        assert_eq!(earliest_after([5, 3, 9], 4), Some(5));
        // A wait with under a second left: due the next second, never dropped; one due now kept.
        assert_eq!(ends_in(1000, Duration::from_millis(400)), 1001);
        assert_eq!(ends_in(1000, Duration::from_millis(1400)), 1002);
        assert_eq!(ends_in(1000, Duration::ZERO), 1000);
        assert_eq!(earliest_after([1000, 1200], 1000), Some(1000));
    }

    /// A lease change alone filters the last plan's work again (task #880 step 2), never a plan:
    /// only with a `build::plan` result kept, and no sooner after the last plan or filtering than
    /// it took. (Times passed in: no clock.)
    #[test]
    fn a_lease_change_filters_the_last_plans_work_again() {
        let s = Duration::from_secs;
        let last = |age: u64, took: u64| Some((s(age), s(took), 7, None));
        // (The leases aren't in the digest: no plan for them.)
        assert_eq!(why_plan(None, last(20, 0), 7, 1000), None);
        assert!(refilter_due(true, true, last(20, 0), None));
        assert!(!refilter_due(false, true, last(20, 0), None), "the leases as they were");
        assert!(!refilter_due(true, false, last(20, 0), None), "no plan's work kept");
        assert!(!refilter_due(true, true, None, None), "no plan yet");
        // A plan of 60 s: not filtered again sooner than 60 s after it; a filtering of 5 s, likewise.
        assert!(!refilter_due(true, true, last(59, 60), None));
        assert!(refilter_due(true, true, last(60, 60), None));
        assert!(!refilter_due(true, true, last(60, 60), Some((s(4), s(5)))));
        assert!(refilter_due(true, true, last(60, 60), Some((s(5), s(5)))));
    }

    fn samples(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with(STALL_SAMPLE))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn samples_are_named_by_time_and_only_the_newest_kept() {
        assert_eq!(sample_name(0), "plan-stall-sample-19700101-000000Z.txt");
        assert_eq!(sample_name(951_782_400 + 86399), "plan-stall-sample-20000229-235959Z.txt");
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("plan-stall-sample.txt"), "old").unwrap();
        for i in 0..7u64 {
            std::fs::write(d.path().join(sample_name(1_700_000_000 + i * 1000)), "s").unwrap();
        }
        prune_samples(d.path(), 3);
        let left = samples(d.path());
        assert_eq!(left.len(), 3, "{left:?}");
        assert_eq!(left[2], sample_name(1_700_006_000));
        assert_eq!(left[0], sample_name(1_700_004_000));
    }

    /// The links of a frozen root (`frozen_plan_on_a_private_root`): every entry of the NAS's
    /// project folder `nas` but `state/`, linked at `root` by its name. Made here, never by a shell
    /// loop: `ln -s <folder> <name>` run again, or with a name already a link to a folder, makes the
    /// link inside that folder, on the NAS (task #1004: `inputs/inputs` and 16 more, 7 Nov). A name
    /// already at `root` must be the link to its entry; anything else is refused.
    fn link_the_rest(root: &Path, nas: &Path) {
        for e in std::fs::read_dir(nas).unwrap() {
            let name = e.unwrap().file_name();
            if name == "state" || name.to_string_lossy().starts_with(['.', '@', '#']) {
                continue;
            }
            let (at, to) = (root.join(&name), nas.join(&name));
            match std::fs::read_link(&at) {
                Ok(l) => assert_eq!(l, to, "{} links elsewhere", at.display()),
                // (std's symlink fails where the name is taken: it never makes a link inside.)
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::os::unix::fs::symlink(&to, &at).unwrap(),
                Err(e) => panic!("{} is there and not a link: {e}", at.display()),
            }
        }
    }

    #[test]
    fn a_frozen_roots_links_are_made_once_and_never_inside_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let (root, nas) = (d.path().join("root"), d.path().join("nas"));
        for f in ["inputs/a.toml", "state/x", "map/m", "@eaDir/e"] {
            std::fs::create_dir_all(nas.join(f).parent().unwrap()).unwrap();
            std::fs::write(nas.join(f), b"x").unwrap();
        }
        std::fs::create_dir_all(root.join("state")).unwrap();
        link_the_rest(&root, &nas);
        link_the_rest(&root, &nas);
        let mut names: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        assert_eq!(names, ["inputs", "map", "state"]);
        assert_eq!(std::fs::read_link(root.join("inputs")).unwrap(), nas.join("inputs"));
        assert!(std::fs::symlink_metadata(nas.join("inputs/inputs")).is_err(), "no link inside the NAS's folder");
        assert!(std::fs::symlink_metadata(nas.join("map/map")).is_err());
        // A name taken by a folder of its own is refused, never linked into.
        std::fs::remove_file(root.join("map")).unwrap();
        std::fs::create_dir(root.join("map")).unwrap();
        assert!(std::panic::catch_unwind(|| link_the_rest(&root, &nas)).is_err());
        assert!(std::fs::symlink_metadata(root.join("map/map")).is_err());
    }

    /// The frozen root `root` (`SCENIC_FROZEN_ROOT`) for the ignored test `test` to plan on, in the
    /// sandboxed child; none in the parent, which checked the root (never one that is, or leads to,
    /// a live path; its `state/` a folder of its own), linked the rest of the NAS's project folder
    /// there and ran this test again in a child under `sandbox-exec`, whose profile denies every
    /// write under /Volumes and the app's own home, and found its `out` written.
    fn sandboxed(root: &str, test: &str, out: &str) -> Option<PathBuf> {
        let real = std::fs::canonicalize(root).unwrap();
        assert!(!store::nasfs::is_live_path(&real), "the root leads to a live path: {}", real.display());
        let state = std::fs::symlink_metadata(real.join("state")).unwrap();
        assert!(state.is_dir(), "the root's state/ must be a copy, not a link: {}", real.display());
        if std::env::var_os("SCENIC_FROZEN_SANDBOXED").is_some() {
            return Some(real);
        }
        let share = store::nas::find_mount(store::nas::HOST, store::nas::SHARE).expect("the NAS's share mounted");
        link_the_rest(&real, &share.point.join(store::nas::PROJECT));
        let app_home = std::env::var("HOME").unwrap() + "/Library/Application Support/scenic";
        let profile =
            format!("(version 1)(allow default)(deny file-write* (subpath \"/Volumes\") (subpath \"{app_home}\"))");
        let me = std::env::current_exe().unwrap();
        std::fs::remove_file(out).ok();
        let st = store::spawn::status(
            store::spawn::command("/usr/bin/sandbox-exec")
                .args(["-p", &profile])
                .arg(me)
                .args(["--exact", &format!("agent::plan::tests::{test}"), "--ignored", "--nocapture"])
                .env("SCENIC_FROZEN_SANDBOXED", "1"),
        )
        .unwrap();
        assert!(st.success(), "the sandboxed plan: {st}");
        assert!(std::fs::metadata(out).is_ok(), "the sandboxed plan wrote no {out}");
        None
    }

    /// A frozen root's planner's options: a dry run, once, its home `home`.
    fn frozen_options(root: &store::nasfs::NasRoot, home: &str) -> Options {
        Options {
            the_plan: crate::agent::seam::for_tests(),
            root: Some(root.clone()),
            home: PathBuf::from(home),
            bin: PathBuf::from("/app"),
            dry_run: true,
            once: true,
            member: false,
        }
    }

    /// The frozen-plan check (docs/crates.md §6.3, 6f2): a planner's `region_jobs` and `checklist`
    /// on a private copy of the lead's state (`SCENIC_FROZEN_ROOT`: `state/` copied there by hand,
    /// every other entry of the NAS's project folder linked there by the check itself,
    /// `link_the_rest`; never the NAS itself), its home
    /// `SCENIC_FROZEN_HOME` (the coverage's cache and the terrain indexes kept there between runs),
    /// with no coordinator, no gate watch and no record of past catalogues. What it planned (the
    /// jobs with their commands, targets and keys, before and after the holds; the waits; the
    /// holds; the offers; the checklist's lines), a line each, to `SCENIC_FROZEN_OUT`: two builds'
    /// plans of one state diffed. The forecast is left out (its thread parked, never run).
    ///
    /// It plans in a child of itself under `sandbox-exec`, whose profile denies every write under
    /// /Volumes (the NAS's mount, which the symlinks lead to) and the app's own home: the planner's
    /// writes through a symlink fail, never reach the NAS. It refuses a root that is, or leads to, a
    /// live path, and a `state/` that isn't a folder of its own there.
    ///
    /// Knobs, for states where the pass's holds and waits fire (the records edited in the copy do
    /// the rest): `SCENIC_FROZEN_PASS` the pass planned (`none`: before the first), else the
    /// root's latest; `SCENIC_FROZEN_FAILURES` failures in a row by job id (`outlines <date>=3;…`);
    /// `SCENIC_FROZEN_NAMES_SEQ` a map batch touching names-to-do's inputs (its key: none
    /// without, as no map log is read); `SCENIC_FROZEN_NAMES_AGO` names-to-do last ran that many
    /// seconds ago, at the key recorded (its gate shut under a day).
    #[test]
    #[ignore]
    fn frozen_plan_on_a_private_root() {
        let (Ok(root), Ok(home), Ok(out)) = (
            std::env::var("SCENIC_FROZEN_ROOT"),
            std::env::var("SCENIC_FROZEN_HOME"),
            std::env::var("SCENIC_FROZEN_OUT"),
        ) else {
            return;
        };
        let Some(real) = sandboxed(&root, "frozen_plan_on_a_private_root", &out) else { return };
        let env = |k: &str| std::env::var(k).ok();
        let root = store::nasfs::NasRoot::local_for_test(real);
        let o = frozen_options(&root, &home);
        let app = env("SCENIC_FROZEN_APP").unwrap_or_else(|| "development".into());
        let hold = bus::Snap::new(jobrt::control::HoldState::default());
        let mut last = publish::LastCatalogue::default();
        if let Some(ago) = env("SCENIC_FROZEN_NAMES_AGO") {
            let keys = crate::pool::records::Brief::now(&root).unwrap().keys;
            let key = keys.lo.get("names-todo").cloned().unwrap_or_default();
            last.ran.insert("names-todo".into(), publish::Ran { key, at: Some(now_s() - ago.parse::<u64>().unwrap()) });
        }
        let mut p = Planner::new(o, "frozen".into(), app, 0, None, false, hold, last);
        for (id, n) in
            env("SCENIC_FROZEN_FAILURES").iter().flat_map(|f| f.split(';')).filter_map(|f| f.rsplit_once('='))
        {
            p.inp.mem.retry.insert(id.to_string(), (n.parse().unwrap(), 0));
        }
        if let Some(seq) = env("SCENIC_FROZEN_NAMES_SEQ") {
            let here = publish::Here::default();
            let mark = store::map::Mark { seq: seq.parse().unwrap(), at: String::new() };
            here.feed.set_marks(BTreeMap::from([(landmarks::namestodo::INPUTS[0].to_string(), mark)]));
            p.inp.map = Some(std::sync::Arc::new(here));
        }
        // (No forecast: a thread that never ends holds its place.)
        *p.forecasting.borrow_mut() = Some(std::thread::spawn(|| -> Forecasted {
            loop {
                std::thread::park()
            }
        }));
        let pass = match env("SCENIC_FROZEN_PASS") {
            Some(d) if d == "none" => None,
            Some(d) => Some(d),
            None => p.latest_pass(&root),
        };
        let (manifest, _) = p.planning_records(&root).unwrap();
        // (The coverage read first, on its own thread: the plan's regions wait for it.)
        let t = Instant::now();
        while let Err(why) = p.coverage(&root, &manifest, false) {
            // (30 min, not `WATCHDOG`: a run by hand, ignored by the suites, over the live
            // records' coverage, which the NAS can take minutes to read.)
            assert!(t.elapsed() < Duration::from_secs(1800), "{why}");
            std::thread::sleep(Duration::from_millis(500));
        }
        eprintln!("frozen: the coverage in {:.1} s", t.elapsed().as_secs_f64());
        let t = Instant::now();
        let mut waiting = Vec::new();
        let mut held = Holds::default();
        let jobs = p.region_jobs(&root, pass.as_deref(), None, &mut waiting, &mut held);
        let mut lines = vec![format!("pass {pass:?}"), format!("held {held:?}")];
        lines.extend(jobs.iter().map(|j| format!("job {j:?}")));
        let jobs = hold_for_lakefill(
            crate::agent::seam::for_tests(),
            hold_for_sources(crate::agent::seam::for_tests(), jobs, held.sources, &mut waiting),
            held.lakefill,
            &mut waiting,
        );
        lines.extend(jobs.iter().map(|j| format!("held job {j:?}")));
        lines.extend(waiting.iter().map(|w| format!("waiting {w:?}")));
        let (date, offered) = p.offered.borrow().clone();
        lines.push(format!("offered {date}"));
        lines.extend(offered.iter().map(|o| format!("offer {o:?}")));
        eprintln!("frozen: the plan in {:.1} s", t.elapsed().as_secs_f64());
        let recipes = inputs::inputs::regions::read::source(&root, &manifest)
            .and_then(|a| inputs::inputs::regions::read::recipes(&a))
            .unwrap_or_default();
        lines.extend(p.checklist(&root, &recipes).iter().map(|l| format!("line {l:?}")));
        std::fs::write(&out, lines.join("\n") + "\n").unwrap();
        eprintln!(
            "frozen: {} jobs, {} waits, {} offers, {} lines to {out}",
            jobs.len(),
            waiting.len(),
            offered.len(),
            lines.len()
        );
    }

    /// The planner's memory over plans in a row (task #1251), on a frozen root as
    /// `frozen_plan_on_a_private_root`'s (`SCENIC_FROZEN_ROOT`, `SCENIC_FROZEN_HOME`, sandboxed
    /// alike): the coverage read, then `SCENIC_FROZEN_ROUNDS` plans (7 without), each with its
    /// checklist, its forecast when one is due and the view published, as the plan thread's look
    /// makes them. A line per plan to
    /// `SCENIC_FROZEN_OUT`: its time, the bytes it asked the allocator for, and the process's
    /// footprint, its peak, its resident size and its reusable pages after it
    /// (crate::agent::allocs). With `SCENIC_FROZEN_SITES=<n>`, the stacks of the plans after the
    /// first are sampled and the `n` functions and sites allocating the most bytes listed after
    /// (slower: the plans' times are then the sampling's too; with `SCENIC_FROZEN_SITES_LEAST=<bytes>`,
    /// of the blocks of that size or more alone). With `SCENIC_FROZEN_SIZES`, each
    /// plan's blocks by size after its line (`allocs::sizes`).
    #[test]
    #[ignore]
    fn frozen_plans_memory() {
        use crate::agent::allocs;
        let (Ok(root), Ok(home), Ok(out)) = (
            std::env::var("SCENIC_FROZEN_ROOT"),
            std::env::var("SCENIC_FROZEN_HOME"),
            std::env::var("SCENIC_FROZEN_OUT"),
        ) else {
            return;
        };
        let Some(real) = sandboxed(&root, "frozen_plans_memory", &out) else { return };
        allocs::counting();
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
        let root = store::nasfs::NasRoot::local_for_test(real);
        let app = std::env::var("SCENIC_FROZEN_APP").unwrap_or_else(|_| "development".into());
        let hold = bus::Snap::new(jobrt::control::HoldState::default());
        let mut p =
            Planner::new(frozen_options(&root, &home), "frozen".into(), app, 0, None, false, hold, Default::default());
        let made = p.out_rx.take().unwrap();
        let mb = |b: u64| b >> 20;
        let mut lines = Vec::new();
        let mut say = |l: String| {
            eprintln!("frozen: {l}");
            lines.push(l);
        };
        let memory = |what: &str, took: Duration, asked: (u64, u64)| {
            let m = allocs::Memory::now();
            format!(
                "{what}: {:.1} s, asked for {} MB in {} blocks; footprint {} MB (peak {}), resident {} MB, reusable {} MB",
                took.as_secs_f64(),
                mb(asked.0),
                asked.1,
                mb(m.footprint),
                mb(m.peak),
                mb(m.resident),
                mb(m.reusable)
            )
        };
        let since = |a: (u64, u64)| {
            let b = allocs::asked();
            (b.0 - a.0, b.1 - a.1)
        };
        let (t, a) = (Instant::now(), allocs::asked());
        let (manifest, _) = p.planning_records(&root).unwrap();
        while let Err(why) = p.coverage(&root, &manifest, false) {
            assert!(t.elapsed() < Duration::from_secs(1800), "{why}");
            std::thread::sleep(Duration::from_millis(500));
        }
        drop(manifest);
        say(memory("the coverage", t.elapsed(), since(a)));
        let rounds = num("SCENIC_FROZEN_ROUNDS").unwrap_or(7);
        let sites = num("SCENIC_FROZEN_SITES");
        let sizes = num("SCENIC_FROZEN_SIZES").is_some();
        for round in 1..=rounds {
            if round > 1 {
                allocs::on(sites.is_some(), num("SCENIC_FROZEN_SITES_LEAST").unwrap_or(0));
            }
            let (t, a) = (Instant::now(), allocs::asked());
            p.take_forecast();
            while made.try_recv().is_ok() {}
            let mut waiting = Vec::new();
            let c = p.inp.conditions;
            let plan = p.plan(&root, &c, &mut waiting);
            nasread::PLAN.enter("");
            p.regions = load_regions(&root);
            let (jobs, waits) = (plan.len(), waiting.len());
            if round == 1 {
                for w in &waiting {
                    say(format!("waiting: {} ({})", w.what, w.why));
                }
            }
            p.jobs = plan;
            p.waiting = waiting;
            let regions = p.regions.0.clone();
            p.progress = p.checklist(&root, &regions);
            p.publish();
            allocs::on(false, 0);
            say(memory(&format!("plan {round} ({jobs} jobs, {waits} waits)"), t.elapsed(), since(a)));
            if sizes {
                for l in allocs::sizes() {
                    say(l);
                }
            }
        }
        if let Some(n) = sites {
            for l in allocs::report(rounds.saturating_sub(1), n as usize) {
                say(l);
            }
        }
        std::fs::write(&out, lines.join("\n") + "\n").unwrap();
    }
}
