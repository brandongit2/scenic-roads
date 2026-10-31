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
/// the forecast's and counts' threads), when nothing wakes it sooner.
pub(super) const LOOK: Duration = Duration::from_secs(5);
/// The plan made again at least this often, whatever changed: the fallback for what the digest
/// doesn't cover (time, disk room, planets newly on the NAS).
pub(super) const FALLBACK: Duration = Duration::from_secs(300);
/// The checklist and the regions' states counted again at least this often (a job's end sooner).
const COUNT_EVERY: Duration = Duration::from_secs(300);
/// The regions' steps counted again, while a page asks, at least this often.
const STEPS_EVERY: Duration = Duration::from_secs(60);

/// What **jobs** tells **plan**.
pub(super) enum Msg {
    /// Its side as of this round (the newest kept: a burst coalesces).
    Inputs(Box<Inputs>),
    /// A catalogue made at this time (build::LastCatalogue).
    CatalogueMade(u64),
    /// A job done: a gate check or forget it asked for done.
    Done(String),
    Stop,
}

/// **jobs**' side as the plan reads it: copied each round.
#[derive(Clone, Default)]
pub(super) struct Inputs {
    pub root: Option<NasRoot>,
    pub conditions: Conditions,
    pub mac: cond::Mac,
    pub mem: Memory,
    pub slots: Vec<SlotSeen>,
    pub free_set: Option<u64>,
    pub room_floor: u64,
    pub total_mb: u64,
    pub jobs_limit: u64,
    pub build_room: u64,
    pub guard_on: Option<bool>,
    pub guard_backoff: BTreeMap<String, (u64, String)>,
    pub floor_short: Option<(u64, u64, Instant)>,
    pub proxy_up: bool,
    pub pause: Option<crate::control::Pause>,
    pub pool_seen: Option<PoolView>,
    pub member_id: Option<String>,
    pub resume: bool,
    pub placement: bool,
    pub brokers: bool,
    pub pooled: bool,
    pub resume_now: u64,
    pub ramp_held: bool,
    pub ramp_trouble: Option<String>,
    pub broker: Option<std::sync::Arc<crate::coord::Coordinator>>,
    pub viewer: Option<std::sync::Arc<crate::coord::Coordinator>>,
    /// This Mac's coordinator as the round has it (made or stopped as its part changes).
    pub coord: Option<std::sync::Arc<crate::coord::Coordinator>>,
    /// Whether a job ended since the last round (the checklist and counts again).
    pub ended: bool,
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
    /// The jobs it planned, in its order: the lead's own granted from it (`Agent::lead_grants`).
    pub jobs: Vec<JobSpec>,
    /// Why its work waits, as it found it.
    pub waiting: Vec<Waiting>,
    pub regions: Vec<recipes::Recipe>,
    pub bad: Vec<(String, String)>,
    pub checklist: Vec<build::Line>,
    pub built: BTreeMap<String, build::RegionState>,
    pub forecast: Option<forecast::Forecast>,
    pub publishing: Option<build::PublishLine>,
    pub timetables: Option<crate::timetables::View>,
    pub catalogue: Option<CatalogueSeen>,
    pub published: Option<Published>,
    pub inputs: Vec<crate::inputs::view::InputView>,
    pub upcoming: Option<(u64, std::sync::Arc<room::Hints>)>,
    pub queued: Vec<build::Work>,
    pub ready: Vec<String>,
    pub regions_view: Option<(RegionsKey, std::sync::Arc<serde_json::Value>)>,
    /// The coverage and terrain packs' indexes it planned with: what a job's `--expect-same` reads
    /// its keys by (`Agent::as_read`).
    pub coverage: Option<(String, std::sync::Arc<Coverages>)>,
    pub tiles: Option<std::sync::Arc<tiles::TerrainTiles>>,
    /// Whether the forecast's thread runs (a NAS user: the self-heal waits for it).
    pub forecasting: bool,
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
            if !cfg!(test) {
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
    pub(super) hold: std::sync::Arc<bus::Snap<crate::control::HoldState>>,
    pub(super) view: std::sync::Arc<bus::Snap<PlanView>>,
    pub(super) out: mpsc::Sender<Out>,
    pub(super) out_rx: Option<mpsc::Receiver<Out>>,
    pub(super) phase: std::sync::Arc<bus::Snap<&'static str>>,
    /// Plan again now, and why (`why_plan`).
    pub(super) kicked: Option<&'static str>,
    /// The last plan's number (`next_n`), when it was made, its digest, its timed waits' end and
    /// why its work waits.
    pub(super) n: u64,
    /// The number of the plan running now, or the last (**jobs**' cancel reads it:
    /// `Handle::newest_n`).
    pub(super) running_n: std::sync::Arc<AtomicU64>,
    /// (When it ended, how long it took, its digest and its timed waits' end.)
    pub(super) last: Option<(Instant, Duration, u64, Option<u64>)>,
    pub(super) jobs: Vec<JobSpec>,
    pub(super) waiting: Vec<Waiting>,
    pub(super) regions: (Vec<recipes::Recipe>, Vec<(String, String)>),
    /// The checklist, and when it was worked out; how far each region is built, when that count was
    /// last started and the count under way; the same of the regions' steps for the page.
    pub(super) progress: Option<(Instant, Vec<build::Line>)>,
    pub(super) built: BTreeMap<String, build::RegionState>,
    pub(super) built_counted: Option<Instant>,
    pub(super) built_making: Option<std::thread::JoinHandle<(BTreeMap<String, build::RegionState>, Duration)>>,
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
    /// The pass's reaches as last read, by content name (large: read again only when they change).
    pub(super) reach: std::cell::RefCell<Option<(String, std::sync::Arc<crate::reach::Reaches>)>>,
    /// The terrain packs' indexes, which the z6 tiles' keys read (`tiles`, kept in `pack-idx/`): those
    /// the manifest names, each read once.
    pub(super) tiles: std::cell::RefCell<tiles::TerrainTiles>,
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
    pub(super) queued: std::cell::RefCell<Vec<build::Work>>,
    /// The last catalogue's folder and number, and its regions' outline entries by id (`on_map`).
    pub(super) last_catalogue: std::cell::RefCell<Option<((String, u64), BTreeMap<String, Vec<String>>)>>,
    /// The regions the last plan would publish as built (build::Plan::ready), for the checklist.
    pub(super) ready: std::cell::RefCell<Vec<String>>,
    /// The last plan's forecast (crate::agent::forecast), for the status.
    pub(super) forecast: std::cell::RefCell<Option<forecast::Forecast>>,
    /// When each (Mac, step) last had its grants' departure from the forecast logged
    /// (`forecast_now`): once an hour at most.
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
    /// Publishing as last planned (build::PublishLine), and the key of the last line logged (logged
    /// again only when it changes).
    pub(super) publish_line: std::cell::RefCell<Option<build::PublishLine>>,
    /// The timetables' report as last read (its content name, and what the status shows).
    pub(super) timetables: std::cell::RefCell<Option<(String, crate::timetables::View)>>,
    pub(super) publish_logged: std::cell::RefCell<String>,
    pub(super) catalogue_seen: std::cell::Cell<Option<CatalogueSeen>>,
    /// The catalogue each Mac's map server last adopted (`adopted`), and when it was read: read
    /// again when a newer catalogue is seen, else every five minutes.
    pub(super) adopted: std::cell::RefCell<(u64, Option<u64>, BTreeMap<String, CatalogueSeen>)>,
    /// The lead's record of its catalogues (build::LastCatalogue, its folder's `publishing.json`): when
    /// the last went out with the low zooms made.
    pub(super) catalogues: std::cell::RefCell<build::LastCatalogue>,
    /// Whether the catalogue last planned had the low zooms made (build::Plan::low, nothing left of
    /// the publishing chain): its catalogue, once made, is noted so (`LastCatalogue::low_at`).
    pub(super) catalogue_low: std::cell::Cell<bool>,
    /// Publishing's switches as last read (crate::agent::publish).
    pub(super) publishing: std::cell::Cell<publish::Switches>,
    /// The map tiles noted planned in the history (crate::pubtimes), since the last catalogue.
    pub(super) planned_tiles: std::cell::RefCell<crate::pubtimes::PlannedNotes>,
    /// The gate (crate::inputs): the inputs folders' listing thread, the status's entries as the last
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
        hold: std::sync::Arc<bus::Snap<crate::control::HoldState>>,
        catalogues: build::LastCatalogue,
    ) -> Planner {
        let (out, out_rx) = mpsc::channel();
        let tiles = tiles::TerrainTiles::new(Some(o.home.join("pack-idx")));
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
            running_n: std::sync::Arc::new(AtomicU64::new(0)),
            last: None,
            jobs: Vec::new(),
            waiting: Vec::new(),
            regions: Default::default(),
            progress: None,
            built: BTreeMap::new(),
            built_counted: None,
            built_making: None,
            steps_counted: None,
            steps_making: None,
            #[cfg(test)]
            slow_plan: None,
            offered: Default::default(),
            reach: Default::default(),
            tiles: std::cell::RefCell::new(tiles),
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
            adopted: Default::default(),
            catalogues: std::cell::RefCell::new(catalogues),
            catalogue_low: Default::default(),
            publishing: Default::default(),
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
                    self.progress = None;
                    // (Counted again once the count under way is in: the job's targets weren't in it.)
                    self.built_counted = None;
                    self.steps_counted = None;
                }
                self.coord = i.coord.clone();
                self.inp = *i;
            }
            Msg::CatalogueMade(at) => self.catalogue_made(at),
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
    /// asks taken up, a plan made when due (`why_plan`), the checklist and counts when due, and
    /// the view published.
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
        let now = now_s();
        let last = self.last.map(|(at, took, s, t)| (at.elapsed(), took, s, t));
        if let Some(why) = why_plan(self.kicked, last, seen, now) {
            self.kicked = None;
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
            self.n = n;
            self.offer_plan(&plan);
            self.regions = load_regions(&root);
            let replan_at = self.replan_at(&plan, now_s());
            self.last = Some((Instant::now(), t.elapsed(), seen, replan_at));
            if t.elapsed() >= Duration::from_secs(10) {
                eprintln!("agent: plan {} ({why}) took {} s", self.n, t.elapsed().as_secs());
            }
            self.jobs = plan;
            self.waiting = waiting;
            // (A new plan: the checklist with it.)
            self.progress = None;
        }
        self.phase("the checklist and counts");
        self.count(&root);
        self.publish();
        self.phase("");
    }

    /// The checklist (after a plan or a job's end, else every `COUNT_EVERY`), how far each region
    /// is built (as often, on a thread of its own), and each region's steps for the build page
    /// (while a page asks, every `STEPS_EVERY`).
    pub(super) fn count(&mut self, root: &NasRoot) {
        if self.progress.as_ref().is_none_or(|(t, _)| t.elapsed() >= COUNT_EVERY) {
            let regions = self.regions.0.clone();
            self.progress = Some((Instant::now(), self.checklist(root, &regions)));
        }
        if self.built_making.is_none() && self.built_counted.is_none_or(|t| t.elapsed() >= COUNT_EVERY) {
            self.built_counted = Some(Instant::now());
            self.built_making = self.start_as_built(root, "region-states", build::region_states);
        }
        // (In line, waited for: a single loop's status has them.)
        let wait = self.o.once;
        if self.built_making.as_ref().is_some_and(|h| wait || h.is_finished()) {
            match self.built_making.take().map(|h| h.join()) {
                Some(Ok((b, took))) => {
                    eprintln!("agent: the regions' states took {} ms", took.as_millis());
                    self.built = b;
                }
                _ => eprintln!("agent: counting the regions' states failed"),
            }
        }
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
            self.steps_making = self.start_as_built(root, "region-steps", build::region_steps);
        }
    }

    /// The view published whole (`PlanView`).
    pub(super) fn publish(&mut self) {
        let root = self.inp.root.clone();
        let published = Some(self.published(root.as_ref()));
        let view = PlanView {
            n: self.n,
            jobs: self.jobs.clone(),
            waiting: self.waiting.clone(),
            regions: self.regions.0.clone(),
            bad: self.regions.1.clone(),
            checklist: self.progress.as_ref().map(|(_, c)| c.clone()).unwrap_or_default(),
            built: self.built.clone(),
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
            coverage: self.coverage.borrow().clone(),
            tiles: Some(std::sync::Arc::new(self.tiles.borrow().clone())),
            forecasting: self.forecasting.borrow().as_ref().is_some_and(|h| !h.is_finished()),
        };
        self.view.set(view);
    }

    /// This Mac's hold, as control holds it now.
    pub(super) fn hold(&self) -> std::sync::Arc<crate::control::HoldState> {
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

    pub(super) fn total_mb(&self) -> u64 {
        self.inp.total_mb
    }

    pub(super) fn jobs_limit(&self, _total: u64) -> u64 {
        self.inp.jobs_limit
    }

    pub(super) fn build_room(&self) -> u64 {
        self.inp.build_room
    }

    pub(super) fn guard_stops(&self) -> bool {
        self.inp.guard_on.unwrap_or(memguard::DEFAULT)
    }

    pub(super) fn job_slots(&self) -> usize {
        lead_slots(self.o.helper, self.inp.ramp_held, &self.inp.mem.ramp)
    }

    pub(super) fn resume_on(&self) -> bool {
        self.inp.resume && self.inp.pooled
    }

    pub(super) fn latest_pass(&self, root: &NasRoot) -> Option<String> {
        crate::osmpass::latest_pass_by(root, self.resume_on())
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
        step_secs_here(&self.inp.mem, step)
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

    /// The records' manifest and the planning keys (`planning_keys`), from one read of the records.
    pub(super) fn planning_records(
        &self,
        root: &store::nasfs::NasRoot,
    ) -> Result<(BTreeMap<String, String>, build::Keys)> {
        let r = crate::out::brief_now(root)?;
        Ok((r.manifest, r.keys.with_waiting(&self.handoff_bases(root))?))
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
            match spec.record.as_ref().filter(|w| every::offered(&w.step)) {
                Some(w) => {
                    // (Every offer of its step: a halo step's are one a halo, crate::needs::HALO_STEPS.)
                    if seen.insert(w.step.clone()) {
                        offers.extend(offered.iter().filter(|o| o.step == w.step).cloned());
                    }
                }
                None => {
                    let step =
                        spec.record.as_ref().map(|w| w.step.clone()).or_else(|| step_of(&spec.id)).unwrap_or_default();
                    let mb = steps::mem_mb(&step);
                    offers.push(crate::coord::Offer {
                        targets: own_targets(spec).into_iter().map(|(t, k)| (t, k, mb)).collect(),
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
    /// learned alone the way its step runs now, passes this Mac's limit (`here`), and of them those
    /// past every Mac's in the pool, as the lead knows them (`all`: a step only this Mac runs, this
    /// Mac's limit alone), with why for the status; and those whose floor past it was a batch's
    /// (`alone`: tried again in a job of their own before they're held).
    pub(super) fn guard_holds(&self, works: &[build::Work]) -> GuardHolds {
        let mut h = GuardHolds::default();
        let limit = self.jobs_limit(self.total_mb());
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
                // A step only the lead runs: judged against the lead's limit alone; it waits (the
                // lead is fixed: the lead's memory or the step's is the owner's to see to).
                let (s, v) = if n_all == 1 { ("", "s") } else { ("s", "") };
                let remedy = "the lead's build memory (`scenic memory`) or the step's memory, for the owner to see to";
                h.why.push(Waiting { step: Some(w.step.clone()), what: steps::current(&w.step).into(), why: format!("{n_all} target{s} need{v} about {:.1} GB, past the lead's limit of {:.1} GB; this step runs only on the lead. {remedy} (`scenic pool floors --clear` once it's fixed)", gb(need), gb(limit)) });
            } else if n_all > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: steps::current(&w.step).into(), why: format!("{n_all} target{} need{} about {:.1} GB, more than any Mac in the pool has (the most: {:.1} GB): a step whose memory grows with its target, for the owner to see to (`scenic pool floors --clear` once it's fixed)", if n_all == 1 { "" } else { "s" }, if n_all == 1 { "s" } else { "" }, gb(need), gb(most)) });
            } else if n_here > 0 {
                h.why.push(Waiting { step: Some(w.step.clone()), what: steps::current(&w.step).into(), why: format!("{n_here} target{} need{} about {:.1} GB, past this Mac's limit of {:.1} GB: left to a Mac with room", if n_here == 1 { "" } else { "s" }, if n_here == 1 { "s" } else { "" }, gb(need), gb(limit)) });
            }
        }
        h
    }

    /// A catalogue made at `at`: noted in the lead's record (build::LastCatalogue), with the low zooms
    /// made if its plan had them (`catalogue_low`).
    pub(super) fn catalogue_made(&self, at: u64) {
        let mut r = self.catalogues.borrow_mut();
        r.at = at;
        r.held_at = None;
        if self.catalogue_low.get() {
            r.low_at = Some(at);
        }
        if self.keeps_publishing() {
            if let Err(e) = write_published(&self.o.home, &r) {
                eprintln!("agent: noting the catalogue made: {e:#}");
            }
        }
    }

    /// The steps gated by time whose recorded keys in `done` changed: noted run now in the lead's
    /// record (build::LastCatalogue::observe).
    pub(super) fn note_ran(&self, done: &build::Keys) {
        let mut r = self.catalogues.borrow_mut();
        if r.observe(done, now_s()) && self.keeps_publishing() {
            if let Err(e) = write_published(&self.o.home, &r) {
                eprintln!("agent: noting the steps run: {e:#}");
            }
        }
    }

    /// When a catalogue due was first held back (build::LastCatalogue::held_at), kept in the lead's
    /// record when it changes.
    pub(super) fn note_held(&self, held_at: Option<u64>) {
        let mut r = self.catalogues.borrow_mut();
        if r.held_at == held_at {
            return;
        }
        r.held_at = held_at;
        if self.keeps_publishing() {
            if let Err(e) = write_published(&self.o.home, &r) {
                eprintln!("agent: noting the catalogue held back: {e:#}");
            }
        }
    }

    /// Publishing as planned (build::PublishLine) kept for the status, the pause said, and logged
    /// when it or its reasons change.
    pub(super) fn record_publishing(&self, mut u: build::PublishLine) {
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
        let Some(c) = manifest.get(crate::timetables::REPORT) else { return };
        if self.timetables.borrow().as_ref().is_some_and(|t| &t.0 == c) {
            return;
        }
        if let Some(v) =
            nasread::read(&root.join(c)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        {
            *self.timetables.borrow_mut() = Some((c.clone(), crate::timetables::View::of(&v)));
        }
    }

    /// The work there is, in order (docs/building.md §2, Order).
    /// A digest of what an idle agent's plan depends on, read cheaply: the build's records (the
    /// manifest, the jobs' keys, the hand-offs waiting), the gate's view of the inputs, the pool
    /// (membership, role, gates), its mode and switch, and the pause. Equal digests: the plan would
    /// come out the same but for what `IDLE_REPLAN` covers (time, disk room, planets newly on the NAS).
    pub(super) fn plan_seen(&self, root: &store::nasfs::NasRoot) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let stat = |h: &mut std::collections::hash_map::DefaultHasher, p: store::nasfs::Loc<'_>| {
            let m = p.metadata().ok();
            m.as_ref().map(|m| (m.len(), m.modified().ok())).hash(h);
        };
        // (The records: the current term's snapshot, which its lead rewrites whole with each save;
        // today's files before the pool.)
        let term = self.inp.pool_seen.as_ref().map_or(0, |p| p.gates.term);
        if term > 0 {
            stat(&mut h, (&root.join(crate::pool::records::path(term))).into());
        }
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
        if let Some(p) = &self.inp.pool_seen {
            (&p.member, p.role, &p.members).hash(&mut h);
            serde_json::to_string(&p.gates).unwrap_or_default().hash(&mut h);
        }
        serde_json::to_string(&self.inp.pause).unwrap_or_default().hash(&mut h);
        // (Publishing's switches, the hold on catalogues, the catalogues' folders.)
        for f in [publish::DELTA_CATALOGUE, publish::NEAR_SLOPE, "inputs/hold-catalog", "catalog", "catalog-held"] {
            stat(&mut h, (&root.join(f)).into());
        }
        // (The owner's redo asks: one taken or done.)
        stat(&mut h, (&self.o.home.join(redo::ASKS)).into());
        // (The coordinator's leases: one lapsed, given back or taken frees or holds targets.)
        self.coord.as_ref().map(|c| c.lease_gen()).hash(&mut h);
        self.newer_app().hash(&mut h);
        // (The pool's switches as this agent holds them.)
        (self.inp.resume, self.inp.placement, self.inp.brokers, self.inp.guard_on).hash(&mut h);
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
        // (In the tests, every planned command checked against its program's table, crate::cli.)
        #[cfg(test)]
        for j in &jobs {
            if let Some(Err(e)) = crate::cli::check_command(&j.cmd) {
                panic!("{e}");
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

        // 1. The OSM pass, when the NAS holds a newer planet than the last complete pass.
        let have = self.latest_pass(root);
        let newer = crate::osmpass::newer_planet(root, have.as_deref()).ok().flatten();
        // (Resuming on: held for, and offered to, the member whose Mac has its progress.)
        let held = self.resume_hold(root, newer.as_ref());
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
        // Daily: the user's folders backed up, replaced files removed.
        if self.due("backup", Duration::from_secs(86400)) {
            out.push(JobSpec {
                id: "backup".into(),
                what: "backup".into(),
                cmd: vec![
                    s(&me),
                    "backup".into(),
                    "--root".into(),
                    jobs::root_arg(root),
                    "--local".into(),
                    s(&self.o.home.join("backups")),
                ],
                needs: Needs { nas: true },
                restart_after_sleep: true,
                record: None,
            });
        }
        // The owner's redos (`scenic redo`, crate::agent::redo), the lead's: a job each, which forgets
        // its targets' records.
        if self.coord.is_some() && !self.o.helper {
            out.extend(self.redo_jobs(root));
        }
        if self.due("gc", Duration::from_secs(86400)) {
            let keep = self.keep_depth_targets();
            out.push(JobSpec {
                id: "gc".into(),
                what: "gc".into(),
                cmd: vec![s(&me), "gc".into(), "--root".into(), jobs::root_arg(root), "--depth-keep".into(), s(&keep)],
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
                        "{} {} {} target{}' records",
                        if a.restore.is_empty() { "Forgetting" } else { "Restoring" },
                        a.targets.len(),
                        a.step,
                        if a.targets.len() == 1 { "" } else { "s" }
                    ),
                    cmd,
                    needs: Needs { nas: true },
                    restart_after_sleep: true,
                    record: Some(build::Work { step: redo::STEP.into(), targets: redo::done_of(&a) }),
                }
            })
            .collect()
    }

    /// The owner's redo asks this Mac's coordinator took (`/work/redo`), kept in its agent's folder
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
            !(held.sources && build::reads_sources(&o.step)) && !(held.lakefill && build::reads_lakefill(&o.step))
        });
        let jobs = hold_for_sources(jobs, held.sources, waiting);
        hold_for_lakefill(jobs, held.lakefill, waiting)
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
        // (The depth's targets for GC: none until this plan finds them.)
        self.depth_keep.borrow_mut().take();
        // (The records unreadable now: nothing planned until they are, rather than everything again.)
        let (manifest, keys): (BTreeMap<String, String>, build::Keys) = match self.planning_records(root) {
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
        let regions_app = crate::inputs::regions::read::switch_app(&manifest, &|n| {
            root.join(n).read().with_context(|| format!("read {n}"))
        })
        .map_err(|e| format!("{e:#}"));
        // (And the coordinator grants nothing to an app older than it, a page's included.)
        if let (Some(c), Ok(min)) = (&self.coord, &regions_app) {
            c.set_min_app(min.clone());
        }
        if let Err(why) = crate::inputs::regions::read::may_lead(&self.app, &regions_app) {
            waiting.push(Waiting {
                step: None,
                what: "All building".into(),
                why: format!("this Mac can't build now: {why}"),
            });
            return Vec::new();
        }
        // (The regions from the same records as the coverage.)
        let (recipes, unread): (Vec<recipes::Recipe>, Vec<(String, String)>) =
            match crate::inputs::regions::read::source(root, &manifest).and_then(|a| {
                let r = crate::inputs::regions::read::recipes(&a)?;
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
        held.sources = !crate::inputs::sources::accepted(&manifest);
        // (A job's name is its step's title, from the steps table: `step` is the scenic-build
        // command, the step by its id's.)
        let job = |id: String, step: &str, extra: Vec<String>, record: Option<build::Work>| {
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
            let what = steps::current(step_of(&id).as_deref().unwrap_or(step)).to_string();
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
            // the languages spoken, the lake fill, the rail feeds and the catalogue read them), so
            // nothing is made from the outlines they replace; unless the job has failed
            // `build::OUTLINES_HOLDS` times in a row (counted on this Mac) and the pass has
            // outlines: the rest then goes on with those.
            if let Some(w) = build::outlines_work(date, &manifest, &keys) {
                let failures = self.inp.mem.retry.get(&format!("outlines {date}")).map_or(0, |r| r.0);
                let made = manifest.contains_key(&format!("sources/osm/{date}/outlines"));
                jobs.push(job(format!("outlines {date}"), "outlines", p.clone(), Some(w)));
                if build::outlines_hold(made, failures) {
                    waiting.push(Waiting {
                        step: None,
                        what: "Building".into(),
                        why: "waits for the pass's outlines to be assembled (the outlines job)".into(),
                    });
                    return jobs;
                }
                waiting.push(Waiting {
                    step: Some("outlines".into()),
                    what: "outlines".into(),
                    why: format!(
                        "failed {failures} times in a row: the rest goes on with the outlines the pass has meanwhile"
                    ),
                });
            }
            if let Some(w) = build::trailends_work(date, &manifest, &keys) {
                jobs.push(job(format!("trailends {date}"), "trailends", p.clone(), Some(w)));
            }
            // The coastline's land, which the regions' check cuts a region with water back to.
            if let Some(w) = build::regions_land_work(date, &manifest, &keys) {
                jobs.push(job(format!("regions-land {date}"), "regions-land", p.clone(), Some(w)));
            }
            let reach_job = |w: build::Work| job(format!("reach {date}"), "reach", p.clone(), Some(w));
            if let Some(w) = build::reach_work(date, &manifest, &keys) {
                jobs.push(reach_job(w));
            } else if let Err(why) = self.current_reach(root, &manifest, &keys, date) {
                match why {
                    // Made, but it doesn't decode: made again.
                    crate::reach::LoadError::Bad(e) => {
                        waiting.push(Waiting {
                            step: None,
                            what: "roads".into(),
                            why: format!("the pass's reaches don't read ({e}); making them again"),
                        });
                        if let Some(k) = build::reach_key(date, &manifest) {
                            jobs.push(reach_job(build::Work {
                                step: "reach".into(),
                                targets: vec![("reach".into(), k)],
                            }));
                        }
                    }
                    crate::reach::LoadError::Io(e) => waiting.push(Waiting {
                        step: None,
                        what: "roads".into(),
                        why: format!("the pass's reaches can't be read now: {e}"),
                    }),
                }
            }
            // (Of the declared release: none before the source declarations are accepted.)
            let release = crate::buildtiles::release_of(root, &manifest).ok();
            if let Some(release) =
                release.as_deref().filter(|r| !manifest.contains_key(&crate::buildtiles::index_logical(r)))
            {
                jobs.push(job(format!("buildings {release}"), "buildings", Vec::new(), None));
            } else if let Ok(rd) = std::fs::read_dir(self.o.home.join("scratch/buildings")) {
                // Made: the scan's parts (~40 GB) go, even from a run stopped before it removed them.
                for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("parts-")) {
                    std::fs::remove_dir_all(e.path()).ok();
                }
            }
            if let Some(w) = build::labels_work(date, &manifest, &keys) {
                jobs.push(job(format!("labels {date}"), "labels", p.clone(), Some(w)));
            }
            let table = manifest.get(&crate::inputs::logical(crate::inputs::languages::UNIT));
            if let (Some(w), Some(table)) = (build::spoken_work(date, &manifest, &keys), table) {
                // (The languages' accepted version the key names.)
                jobs.push(job(
                    format!("spoken {date}"),
                    "spoken",
                    [p.clone(), vec!["--languages-index".to_string(), table.clone()]].concat(),
                    Some(w),
                ));
            } else if table.is_none() && manifest.contains_key(&format!("sources/osm/{date}/outlines")) {
                waiting.push(Waiting {
                    step: Some("spoken".into()),
                    what: "spoken".into(),
                    why: "the languages' first accepted version (the input check takes inputs/languages/ in)".into(),
                });
            }
            if !manifest.contains_key(&crate::inputs::logical(crate::inputs::heritage::UNIT)) {
                waiting.push(Waiting {
                    step: Some("heritage-sites".into()),
                    what: "heritage-sites".into(),
                    why: "the heritage registers' first accepted version (the input check takes inputs/heritage/ in)"
                        .into(),
                });
            }
            // (The lakes filled from Canada's own data first: while the lakefill job has work, unless
            // it has failed LAKEFILL_HOLDS times in a row (counted on this Mac, from nought again with
            // a new app), the jobs that read the fill wait (`hold_for_lakefill`: the water, the
            // terrain's water digests, the catalogue), so the pass's water, terrain and map all have
            // the new fill, or after that many failures all go without it. While the source's folder
            // can't be read, or while its source isn't declared, they wait too.)
            match crate::lakefill::run::planned_source(root, &manifest) {
                Err(e) => {
                    held.lakefill = true;
                    waiting.push(Waiting { step: Some("lakefill".into()), what: "lakefill".into(), why: e });
                }
                Ok(source) => {
                    let lakefill = build::lakefill_work(date, &manifest, &keys, source.as_deref());
                    let failures = self.inp.mem.retry.get(&format!("lakefill {date}")).map_or(0, |r| r.0);
                    held.lakefill = build::water_waits_for_lakefill(lakefill.is_some(), failures);
                    if let Some(w) = lakefill {
                        if failures >= build::LAKEFILL_HOLDS {
                            waiting.push(Waiting { step: Some("lakefill".into()), what: "lakefill".into(), why: format!("failed {failures} times in a row: the water, terrain and map go without this pass's fill meanwhile") });
                        }
                        jobs.push(job(format!("lakefill {date}"), "lakefill", p.clone(), Some(w)));
                    }
                }
            }
            if let Some(w) = build::water_work(date, &manifest, &keys) {
                jobs.push(job(format!("water {date}"), "water", p.clone(), Some(w)));
            }
        }
        // After each catalogue the map serves: the names to translate and descriptions to write.
        // (From the translations' and descriptions' accepted versions, which the agent names: the
        // job reads them through the accessor, never the inputs folders.)
        let served = served((&root.join("catalog")).into());
        // (The steps gated by time: when each last ran, as its recorded key changed.)
        self.note_ran(&keys);
        let (since, idle) = {
            let r = self.catalogues.borrow();
            (r.since("names-todo", now_s()), r.idle)
        };
        if let Some(w) = build::names_todo_work(served.as_ref().map(|s| s.key.as_str()), &manifest, &keys, since, idle)
        {
            let n = w.targets[0].1.clone();
            let mut args = Vec::new();
            for unit in [crate::inputs::translations::UNIT, crate::inputs::descriptions::UNIT] {
                args.extend([
                    format!("--{unit}-index"),
                    manifest.get(&crate::inputs::logical(unit)).cloned().unwrap_or_default(),
                ]);
            }
            jobs.push(job(format!("names-todo {n}"), "names-todo", args, Some(w)));
        } else if served.is_some()
            && [crate::inputs::translations::UNIT, crate::inputs::descriptions::UNIT]
                .iter()
                .any(|u| !manifest.contains_key(&crate::inputs::logical(u)))
        {
            waiting.push(Waiting {
                step: Some("names-todo".into()),
                what: "names-todo".into(),
                why: "the translations' or the descriptions' first accepted version (the input check takes them in)"
                    .into(),
            });
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
        // The timetables' to-do list and report, after each rail and ferries run (lead-only: it
        // writes into the static timetables' inputs folder's todo/, never read as an input).
        if let Some(w) = build::timetables_todo_work(cov, &manifest, &keys) {
            jobs.push(job(
                "timetables-todo".into(),
                "timetables-todo",
                vec!["--pass".to_string(), date.to_string()],
                Some(w),
            ));
        }
        self.note_timetables(root, &manifest);
        let mut done = keys;
        let mut inputs = input_digests(root);
        // (The inputs' descriptions' credits, for the catalogue's key: crate::inputs::credits. None
        // yet, no line, so the catalogue's key is as it was. Unreadable now: the last digest read
        // stands, or, with none read yet, the catalogue waits; never a key that flips on a failed
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
        // (The timetable feeds' declarations as rail-feeds' key names them: the key names they fetch
        // with and the digest of what of them it reads; "?" while their index can't be read, and
        // rail-feeds waits.)
        if let Some(ix) = manifest.get(&crate::inputs::logical(crate::inputs::feeds::UNIT)) {
            let d = crate::inputs::feeds::declared(root, ix);
            inputs.insert("feed-keys".into(), d.as_ref().map(|d| d.key_names.join(",")).unwrap_or_else(|_| "?".into()));
            inputs.insert("feeds".into(), d.map(|d| d.keyed).unwrap_or_else(|_| "?".into()));
        }
        // (The declared feeds the catalogue credits, for its key: "?" while they can't be read, and
        // the catalogue waits.)
        match crate::inputs::feeds::credited(root, &manifest) {
            Ok(Some(ids)) => {
                inputs.insert("feeds-credited".into(), ids.join(","));
            }
            Ok(None) => {}
            Err(_) => {
                inputs.insert("feeds-credited".into(), "?".into());
            }
        }
        // (The declared elevation datasets' spacing, which the rail and candidates keys name: "?"
        // while it can't be read, and they wait: crate::dem::spacing.)
        inputs.insert(
            crate::dem::spacing::INPUT.into(),
            crate::dem::spacing::digest_read(root, &manifest).unwrap_or_else(|_| "?".into()),
        );
        let held = root.join("inputs/hold-catalog").exists();
        // The base packs' extents (crate::extents), kept by the lead alone, against its manifest:
        // the headers of packs new since read once, timed. (A pack whose header can't be read is
        // left out and looked at again next plan, the rest kept; the file not written costs only
        // spills not yet seen: the jobs read what it lacks.)
        if self.inp.pool_seen.as_ref().is_none_or(|p| p.role == pool::Role::Lead) {
            let t = std::time::Instant::now();
            match crate::extents::keep(root, &manifest) {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!("extents: {n} base packs' headers read and kept in {:.1} s", t.elapsed().as_secs_f64())
                }
                Err(e) => eprintln!("extents: not kept now: {e:#}"),
            }
        }
        let reach = self.current_reach(root, &manifest, &done, date).ok().flatten();
        if !manifest.contains_key(&crate::inputs::logical(crate::inputs::feeds::UNIT)) {
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
        if manifest.contains_key(&crate::osmpass::set_name(date, "ferries"))
            && !manifest.contains_key(crate::rail::FERRY_FEEDS)
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
        if !manifest.contains_key(&crate::inputs::logical(crate::inputs::gtfs::UNIT)) {
            let why = "the static timetables' first accepted version (inputs/timetables/gtfs/: the input check takes them in)";
            if manifest.contains_key(crate::rail::FEEDS) {
                waiting.push(Waiting { step: Some("rail".into()), what: "trains".into(), why: why.into() });
            }
            if manifest.contains_key(&crate::osmpass::set_name(date, "ferries")) {
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
        // The regions the map's catalogue has (or the held one's, when catalogues are held for review)
        // and how long ago it went out: what the plan publishes regions by.
        // (The held ones, once there are any: the first goes by what's served.)
        let held_dir = root.join("catalog-held");
        let dir = if held && store::catalogue::list(&held_dir).is_ok_and(|ns| !ns.is_empty()) {
            held_dir
        } else {
            root.join("catalog")
        };
        let (on_map, since_publish) = self.on_map((&dir).into(), &recipes);
        // (The map's: the served catalogue's, held ones aside.)
        self.catalogue_seen.set(served_catalogue((&root.join("catalog")).into()));
        // (Publishing's switches: crate::agent::publish.)
        let sw = publish::Switches::read(root, self.publishing.get());
        if sw != self.publishing.get() {
            eprintln!("agent: publishing's switches: {sw:?}");
        }
        self.publishing.set(sw);
        let way = build::Publishing {
            since_low: self.catalogues.borrow().low_at.map(|t| now_s().saturating_sub(t)),
            near: sw.near,
            since_marks: self.catalogues.borrow().since("marks", now_s()),
            since_overlays: self.catalogues.borrow().since("overlays", now_s()),
        };
        // (The served catalogue's z6 tiles, read again only when a newer catalogue is there: the map tiles
        // to draw grouped by the z6 tile that changed.)
        let served = {
            let cat = crate::agent::served((&root.join("catalog")).into()).map(|s| s.cat);
            self.planned_tiles.borrow_mut().served(cat.as_deref())
        };
        let mut planned = {
            let tiles = self.terrain_tiles(root, &manifest);
            *self.depth_keep.borrow_mut() = build::depth_kept(cov, &manifest, &tiles, now_s());
            // (The records as re-keyed: what they are once the loop has re-keyed them, as a dry run,
            // or a loop that couldn't take the build lock, plans too.)
            let times = rekey::FileTimes::new(root);
            nasread::phase("the plan: the records re-keyed");
            rekey::as_read(&mut done, cov, &manifest, &tiles, &times);
            nasread::phase("the plan: build::plan");
            build::plan(
                &cov,
                date,
                &manifest,
                &done,
                &inputs,
                reach.as_deref(),
                &tiles,
                build::Publish {
                    each: &covs.each,
                    on_map: &on_map,
                    served: &served,
                    since_catalogue: since_publish,
                    held_for: self.catalogues.borrow().held_at.map(|t| now_s().saturating_sub(t)),
                    held,
                    way,
                },
            )
        };
        nasread::phase("the plan: its jobs");
        self.catalogues.borrow_mut().idle = planned.idle;
        // The steps held for other steps' targets: said, never a silent stall (the summits' own note,
        // below, is the same).
        for (step, why) in std::mem::take(&mut planned.held) {
            waiting.push(Waiting { what: step.clone(), why, step: Some(step) });
        }
        // The z6 tiles held until the heritage registers are first accepted: said, not a silent stall.
        if let Some(why) = build::heritage_awaits_acceptance(cov, date, &manifest, &inputs) {
            waiting.push(Waiting {
                step: Some("heritage-sites".into()),
                what: "sites".into(),
                why: format!("the regions' z6 tiles wait: {why}"),
            });
        }
        // The pass's summits (worldwide), once the terrain packs they read are built (as the
        // peaks after them): held, said.
        let summits = build::summits_work(date, &manifest, &done);
        // (Held, they're the landmarks chain's first step in the forecast, after the terrain.)
        let mut summits_held = None;
        let summits_waits = match steps::readiness("summits", "summits", &planned.left) {
            steps::Ready::Waits(w) => Some(steps::wait_note(&w)),
            _ => None,
        };
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
            let why = match self.tiles.borrow().elevation(&manifest) {
                Ok(None) => "the records don't say the build reads the declared elevation datasets yet (their switch, docs/inputs.md §6.8)".to_string(),
                Err(e) => e.0,
                Ok(Some(_)) => {
                    let why = self.tiles.borrow().unread().next().map(|(c, e)| format!(": {c}, {e}")).unwrap_or_default();
                    format!("the terrain's indexes can't be read now for {} of them{why}", planned.unknown.len())
                }
            };
            waiting.push(Waiting { step: Some("unit".into()), what: "roads".into(), why });
        }
        let edit_hold = edit_held(self.edited_at.get(), std::time::SystemTime::now());
        // (Hand-offs of a step a catalogue waits for (build::CATALOGUE_GATE: the map tiles, the road
        // index) not yet merged: counted as done, their files not yet in the manifest. A z6 tile's
        // waiting goes out with a later catalogue.)
        let unmerged = self.handoff_bases(root).iter().any(|b| {
            crate::handoff::waiting_in(b)
                .map(|w| {
                    w.iter()
                        .any(|(_, h)| h.done.as_ref().is_some_and(|d| build::CATALOGUE_GATE.contains(&d.0.as_str())))
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
            let at = self.catalogues.borrow().at;
            let b = if at > 0 { at } else { self.started };
            let mut notes = self.planned_tiles.borrow_mut();
            // (This coordinator's history read once a catalogue.)
            if notes.began() != b {
                let own = self.coord.as_ref().map(|c| c.history_since(0)).unwrap_or_default();
                if notes.begin(b, &own, now_s()) {
                    notes.seed_from_nas(root.clone());
                }
            }
            let tiles: Vec<(String, String)> =
                plan.iter().filter(|w| w.step == "pack").flat_map(|w| w.targets.iter().cloned()).collect();
            let new = notes.new_tiles(&tiles);
            if !new.is_empty() {
                let names: Vec<String> = new.iter().map(|t| t.0.clone()).collect();
                let by_unit = crate::pubtimes::restaled_by(&manifest, reach.as_deref(), &served, &names);
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
            planned.publishing.why = "the catalogue waits for the inputs' descriptions to be read".into();
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
        // (Held so since the gate first held past build::CATALOGUE_CAP_S: it goes, the regions those
        // hand-offs touch left out of `--ready`, build::plan's.)
        let held_for = self.catalogues.borrow().held_at.map(|t| now_s().saturating_sub(t));
        let mut held_back = planned.catalogue_held;
        if unmerged && !publish::unmerged_holds(unmerged, held_for) && plan.iter().any(|w| w.step == "catalog") {
            planned.publishing.why = format!(
                "a catalogue now with what's merged: held {} min by hand-offs waiting to be merged",
                build::CATALOGUE_CAP_S / 60
            );
        }
        if publish::unmerged_holds(unmerged, held_for) && plan.iter().any(|w| w.step == "catalog") {
            held_back = true;
            waiting.push(Waiting {
                step: Some("catalog".into()),
                what: "publish".into(),
                why: "waits for a helper's work to be merged".into(),
            });
            planned.publishing.why = "the catalogue waits for a helper's work to be merged".into();
            plan.retain(|w| w.step != "catalog");
        }
        // (When a catalogue was first held back, kept in the lead's record; none held, none to make
        // and none waiting out its minute: the count starts again.)
        let catalogue_planned = plan.iter().any(|w| w.step == "catalog");
        let was = self.catalogues.borrow().held_at;
        self.note_held(publish::held_at(was, held_back, catalogue_planned || planned.catalogue_due, now_s()));
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
        // A 3D buildings tile's rows read (crate::bld::sources::digests), for its jobs' memory.
        let bld_rows = |t: &str| inputs.get(&format!("bldprep-rows {t}")).and_then(|v| v.parse::<u64>().ok());
        let mut offers: Vec<crate::coord::Offer> = Vec::new();
        // (A terrain z3 tile's z6 tiles near the coverage, for its run's expected memory: only when
        // there's terrain to offer.)
        let z6: BTreeMap<String, usize> = if plan.iter().any(|w| w.step == "terrain-hi") {
            build::coverage_tiles(cov).into_iter().map(|(q, ts)| (format!("3/{}/{}", q.0, q.1), ts.len())).collect()
        } else {
            BTreeMap::new()
        };
        // (And whether it makes deep levels for its hi pack, north of 67°,
        // crate::terrain_deep::reaches_hi: while the fine datasets can't be read, taken as made.)
        let fine = self.tiles.borrow().fine(&manifest);
        let deep = |t: &str| match (&fine, crate::legacy::TileId::parse(t)) {
            (Some(f), Some(u)) => crate::terrain_deep::reaches_hi(f, u.z, u.x, u.y),
            _ => true,
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
        // A cold start of a step another worker runs now: none of the lead's own of that step (one
        // cold start of a step in the pool at a time).
        if let Some(c) = &self.coord {
            for w in plan.iter_mut().filter(|w| !w.targets.is_empty() && c.cold_under_way(&w.step)) {
                waiting.push(Waiting {
                    step: Some(w.step.clone()),
                    what: steps::current(&w.step).to_string(),
                    why: "its first job, never measured, runs on another member now: its others wait for its measure"
                        .into(),
                });
                w.targets.clear();
            }
        }
        // (Every step to every member: the plan's every step offered but the lead's own,
        // crate::agent::every.)
        // (What a terrain or slope job's process holds of the coverage it reads, `coverage_of`:
        // added to its step's bound, crate::coverage::Coverage::read_peak_mb.)
        let cov_read = cov.read_peak_mb();
        for w in plan.iter_mut().filter(|w| every::offered(&w.step)) {
            // What another worker builds now isn't planned here: what it leased from this Mac's
            // coordinator.
            let mut others = BTreeSet::new();
            if let Some(c) = &self.coord {
                others.extend(c.held(&w.step));
            }
            w.targets.retain(|t| !others.contains(&t.0));
            // The rest, offered to the workers that mount the NAS: z6 tiles with their extracts' sizes,
            // the others with the memory their jobs are expected to take (until one's run says);
            // each worker takes what fits its memory.
            let guess = |t: &str| match w.step.as_str() {
                "unit" | "pois" => size(t),
                "terrain-hi" => {
                    terrain_peak(if t.starts_with("6/") { 1 } else { z6.get(t).copied().unwrap_or(64) }, deep(t))
                        + cov_read
                }
                s @ ("terrain-deep" | "slope-deep" | "slope-hi") => first_peak(s) + cov_read,
                s @ ("bldprep" | "bldtiles") => bld_rows(t).map_or(first_peak(s), |n| bld_peak(s, n)),
                s => first_peak(s),
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
        // (Held here: left to a Mac with room. A target whose floor past this Mac's limit was a
        // batch's: a job of its own, so a floor learned alone holds it, or its measure frees it.)
        let mut alone: Vec<build::Work> = Vec::new();
        for w in plan.iter_mut() {
            w.targets.retain(|t| !holds.here.contains(&(w.step.clone(), t.0.clone())));
            let (one, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut w.targets)
                .into_iter()
                .partition(|t| holds.alone.contains(&(w.step.clone(), t.0.clone())));
            w.targets = rest;
            alone.extend(one.into_iter().map(|t| build::Work { step: w.step.clone(), targets: vec![t] }));
        }
        plan.extend(alone);
        // The forecast of the work left (crate::agent::forecast): the lead's, for the status (made
        // again at most each `forecast::EVERY`: idle, the plan's made each loop).
        // (Run on a thread of its own, `start_forecast`: the loop reads the last one done and never
        // waits on it, task #544.)
        self.take_forecast();
        let stale =
            self.forecast.borrow().as_ref().is_none_or(|f| now_s().saturating_sub(f.at) >= forecast::EVERY.as_secs());
        if !self.o.helper && stale && self.forecasting.borrow().is_none() {
            // (What can't be listed now: the regions' work after a new pass, or while the z6 tiles wait.)
            let blind = match newer {
                Some(d) => Some(format!("a new OpenStreetMap pass (the planet of {d}) comes first; the regions' work is known once it's done")),
                None if planned.regions.is_empty() => Some("the z6 tiles wait for the pass's heritage sites, reaches and roadside buildings; what the regions need is known once they're made".to_string()),
                None => None,
            };
            let z6: BTreeMap<String, usize> = build::coverage_tiles(cov)
                .into_iter()
                .map(|(q, ts)| (format!("3/{}/{}", q.0, q.1), ts.len()))
                .collect();
            let fine = self.tiles.borrow().fine(&manifest);
            let deep = |t: &str| match (&fine, crate::legacy::TileId::parse(t)) {
                (Some(f), Some(u)) => crate::terrain_deep::reaches(f, u.z, u.x, u.y),
                _ => true,
            };
            let peak = |step: &str, t: &str| match step {
                "unit" | "pois" => crate::coord::unit_peak(&BTreeMap::new(), t, size(t)),
                "terrain-hi" => {
                    terrain_peak(if t.starts_with("6/") { 1 } else { z6.get(t).copied().unwrap_or(64) }, deep(t))
                        + cov_read
                }
                s @ ("terrain-deep" | "slope-deep" | "slope-hi") => first_peak(s) + cov_read,
                s @ ("bldprep" | "bldtiles") => bld_rows(t).map_or(first_peak(s), |n| bld_peak(s, n)),
                s => first_peak(s),
            };
            // (And the elevation pyramids' fill, the lead's, its z6 tiles at its measured time each.)
            let mut before = before;
            before.extend(
                plan.iter()
                    .filter(|w| matches!(w.step.as_str(), "heritage-sites" | "pyramids-fill"))
                    .map(|w| job(format!("{} {date}", w.step), &w.step, Vec::new(), Some(w.clone()))),
            );
            let mut chains =
                build::chains_left(cov, date, &manifest, &done, &inputs, &self.terrain_tiles(root, &manifest));
            chains[1].splice(0..0, summits_held);
            let record = self.forecast_record(
                root,
                &before,
                &planned.regions,
                &planned.drops,
                &planned.backfill,
                chains,
                blind,
                &peak,
            );
            self.start_forecast(record);
        }
        plan.retain(|w| !w.targets.is_empty());
        // (A job a step's work in the plan: an offered step's is its place in the plan's order, its
        // targets granted as many as fit a quarter of an hour, `offer_plan`; the pyramids' fill a z6
        // tile, `build::job_of`.)
        for w in plan.into_iter().map(build::job_of) {
            // Held for review: the catalogue goes to catalog-held/ (no server reads it), once.
            if w.step == "catalog" && held {
                let k = w.targets.first().map(|t| t.1.clone()).unwrap_or_default();
                if done.catalogue_held.as_deref() == Some(k.as_str()) {
                    waiting.push(Waiting {
                        step: None,
                        what: "publish".into(),
                        why: "held for review (inputs/hold-catalog); its catalogue is in catalog-held/".into(),
                    });
                    continue;
                }
                let mut j = job(
                    "catalog-held".into(),
                    "catalog",
                    vec![
                        "--held".into(),
                        "--ready".into(),
                        ready_arg.clone(),
                        "--building".into(),
                        building_arg.clone(),
                    ],
                    Some(build::Work { step: "catalog-held".into(), targets: vec![("catalog-held".into(), k)] }),
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
                    manifest.get(&crate::inputs::logical(crate::inputs::feeds::UNIT)).cloned().unwrap_or_default(),
                ]);
            }
            // (The static timetables: the accepted version the trains and the ferries were planned
            // with.)
            if w.step == "rail" || w.step == "ferries" {
                extra.extend([
                    "--gtfs-index".to_string(),
                    manifest.get(&crate::inputs::logical(crate::inputs::gtfs::UNIT)).cloned().unwrap_or_default(),
                ]);
            }
            // (The heritage registers' and matches' accepted versions the plan read: the job reads
            // them, never the inputs folders.)
            if w.step == "heritage-sites" {
                for unit in [crate::inputs::heritage::UNIT, crate::inputs::heritage_matches::UNIT] {
                    if let Some(i) = manifest.get(&crate::inputs::logical(unit)) {
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
                // (Delta catalogues: crate::agent::publish; the step writes a whole one when due.)
                if self.publishing.get().delta {
                    extra.push("--delta".into());
                }
            }
            let n = w.targets.len();
            // "Terrain: 3 tiles" (the checklist names the steps alike: crate::agent::steps::said); the
            // z6 tiles made again for their mids say so.
            let mut what = steps::said(&w.step, n);
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

    /// Publishing as it stands (`Published`), the servers' log under `root` read again (within the
    /// NAS readers' deadline; past it, the last read kept) when a newer catalogue is seen or five
    /// minutes have passed.
    pub(super) fn published(&self, root: Option<&store::nasfs::NasRoot>) -> Published {
        let last = self.catalogue_seen.get();
        let c = self.catalogues.borrow();
        let mut drawing: BTreeMap<String, usize> = BTreeMap::new();
        for (w, k) in self.coord.as_ref().map(|c| c.leased_of("pack")).unwrap_or_default() {
            *drawing.entry(host_of(&w).to_string()).or_default() += k;
        }
        if let Some(root) = root {
            let mut a = self.adopted.borrow_mut();
            let n = last.map(|l| l.n);
            if a.1 != n || now_s().saturating_sub(a.0) >= 300 {
                // (Past the deadline: the last read kept, tried again five minutes on.)
                let read = adopted(root).unwrap_or_else(|_| a.2.clone());
                *a = (now_s(), n, read);
            }
        }
        // (The low zooms' flag of the same catalogue as `last`: the one this record noted, not a
        // newer one served since.)
        let low = c.at > 0 && c.low_at == Some(c.at) && last.is_some_and(|l| l.at <= c.at);
        Published { last, low, drawing, adopted: self.adopted.borrow().2.clone() }
    }

    /// The forecast (crate::agent::forecast) of the work left: `before`, the lead's jobs before
    /// the regions'; `regions`, the plan's; `backfill`, the z6 tiles whose mids it makes in
    /// idle time (build::Plan::backfill); `chains`, the trains', landmarks', 3D buildings' and depths' work to come
    /// (build::chains_left); `blind`, why the work can't all be listed now. Each target's time (at
    /// the lead's pace: a time measured on a member over its speed) and memory as last measured,
    /// else its step's mean or a first guess (`first_secs`; `peak` for its memory); the helpers at
    /// their measured speed; each machine free once its job under way is done. As data
    /// (forecast::Record), run off the loop (`start_forecast`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn forecast_record(
        &self,
        root: &store::nasfs::NasRoot,
        before: &[JobSpec],
        regions: &[build::RegionLeft],
        drops: &[(String, String)],
        backfill: &[build::Work],
        chains: [Vec<build::Work>; 5],
        blind: Option<String>,
        peak: &dyn Fn(&str, &str) -> u64,
    ) -> forecast::Record<'static> {
        use forecast::{Cost, Machine};
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
            // (Each Mac and step said at most once an hour.)
            let now = now_s();
            let mut said = self.departed.borrow_mut();
            said.retain(|_, at| now.saturating_sub(*at) < 3600);
            for (k, d) in forecast::departures(prev, &real, now) {
                // (Stamped when logged only: a departure recurring every plan is said again an hour on.)
                if let std::collections::btree_map::Entry::Vacant(e) = said.entry(k) {
                    e.insert(now);
                    eprintln!("agent: the grants departed from the forecast: {d}");
                }
            }
        }
        // Each worker's speed against the lead's, from the history (half until it's measured):
        // every one that measured a cost or did shared work, asleep or not (its costs are put at the
        // lead's pace by it); the members heard from lately are the machines.
        let helpers: Vec<Status> = helpers(root, &self.host);
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
        let mut speeds = forecast::speeds(&events, &self.host, &names, 0.5);
        speeds.extend(forecast::speeds(&events, &self.host, &own_names, 0.8));
        // (A time measured on a member, at the lead's pace; one measured here or by a page, as it is.)
        let pace = |worker: Option<&String>| worker.and_then(|w| speeds.get(w)).map_or(1.0, |s| s.0);
        // Each shared step's mean of the targets measured (a z6 tile's are kept by its target alone);
        // else what its jobs here took a target; else a first guess.
        // (Each measured the way its step runs now: crate::coord::cost_version. A z3 tile's tree
        // cover isn't a z6 tile's.)
        // (A step whose time is the work its target finds, crate::needs::PER_WORK, weighed by it: a
        // pyramids-fill z6 tile with nothing to fetch says nothing of one fetching for hours, task #830.)
        let mut sums: BTreeMap<String, (f64, f64)> = BTreeMap::new();
        for (k, c) in &costs {
            let step = memguard::step_of_key(k);
            if c.v < crate::coord::cost_version(step) {
                continue;
            }
            let Some(w) = crate::needs::time_weight(step, c) else { continue };
            let e = sums.entry(step.to_string()).or_default();
            (e.0, e.1) = (e.0 + c.secs as f64 * pace(c.worker.as_ref()) * w, e.1 + w);
        }
        let own_secs = |step: &str| self.step_secs_here(step).unwrap_or_else(|| first_secs(step));
        let per =
            |step: &str| -> f64 { sums.get(step).filter(|s| s.1 > 0.0).map_or_else(|| own_secs(step), |s| s.0 / s.1) };
        // (Its memory by its step's model, crate::needs: its own measure the way the step runs now,
        // else its step's fit or largest, else `peak`'s first guess.)
        let book = crate::needs::Book::new(&costs);
        let need_mb = |step: &str, t: &str| book.need(step, t, 0, peak(step, t)).mem_mb;
        let cost = |step: &str, t: &str| -> Cost {
            match costs.get(&crate::coord::cost_key(step, t)) {
                Some(c) => {
                    Cost { secs: c.secs as f64 * pace(c.worker.as_ref()), known: true, peak_mb: need_mb(step, t) }
                }
                None => Cost { secs: per(step), known: false, peak_mb: need_mb(step, t) },
            }
        };
        // (A drop: its own run's measure, else its step's drops', else any step's drops', else as a
        // target of its step until a drop has run: crate::needs::DROPS, Book::drop_need.)
        let drop_cost = |step: &str, t: &str| -> Cost {
            match costs.get(&crate::coord::cost_key(step, t)).filter(|c| crate::needs::is_drop(c)) {
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
                (step_of(&j.id), j.id.clone(), mine(&step_of(&j.id), j.record.as_ref().map_or(1, |w| w.targets.len())))
            })
            .collect();
        // (A region's map tiles each at its measured time, `cost`; the whole-map steps by their
        // cadence, as last run.)
        let [rail, landmarks, buildings, depth, polar] = chains;
        let cadence = {
            let c = self.catalogues.borrow();
            let now = now_s();
            forecast::Cadence {
                since_low: c.low_at.map(|t| now.saturating_sub(t)),
                since_marks: c.since("marks", now),
                since_overlays: c.since("overlays", now),
            }
        };
        // The trains' and the landmarks' chains from the start (their steps once what they read is
        // built: forecast::waits_of), but the overlays (they read the built z6 tiles) after everything
        // else; and a catalogue after them with what the chains made since.
        let (mut chain_jobs, mut after): (Vec<forecast::Job>, Vec<forecast::Job>) = (Vec::new(), Vec::new());
        for w in rail.iter().chain(landmarks.iter()).chain(buildings.iter()).chain(depth.iter()).chain(polar.iter()) {
            let jobs: Vec<forecast::Job> = if steps::SHARED.contains(&w.step.as_str()) {
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
        // take its tail, crate::offload.)
        let unit_extra_s = if self.coord.as_ref().is_some_and(|c| c.tail_takers()) {
            crate::offload::LEASE_WAIT.as_secs_f64()
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
        // (A Mac's room is the memory its owner gives the build's jobs together (`build_room`,
        // `scenic memory`), as `pack_why` takes an ask's memory: a member's `Ask::mem_mb` is it,
        // not a slot's; the coordinator's worker record keeps it (`mem`). This Mac's, its own.)
        rooms.insert(self.host.clone(), forecast::MacRoom { mem_mb: self.build_room(), cores: mac_cores() });
        let mut machines = vec![Machine {
            name: self.host.clone(),
            speed: 1.0,
            measured: true,
            equal: false,
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
                equal: true,
                mem_mb: mac_mb,
                busy_s: if k < self.inp.slots.len() { busy(k, speed) } else { 0.0 },
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
            // (A member paused alone (crate::control::Hold) has no lanes: its jobs are given back at
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
            // (Its room: the memory its owner gives the build's jobs together (`Ask::mem_mb`, as
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
                // (Its lease's targets at its pace, less the time since it took them, if longer
                // than its job's own estimate.)
                let lease_left = leased
                    .iter()
                    .filter(|l| l.0 == name)
                    .map(|(_, step, ts, age, _)| {
                        ts.iter().map(|t| cost(step, t).secs).sum::<f64>() / speed - *age as f64
                    })
                    .fold(0.0, f64::max);
                let job = jobs.iter().find(|j| j.slot == Some(k));
                // (No progress said: its step's measured time a target, at its pace.)
                let eta = job.map(|j| {
                    j.progress
                        .as_ref()
                        .and_then(|p| p.eta_s)
                        .map_or_else(|| per(&step_of(&j.id)) / speed.max(0.1), |e| e as f64)
                });
                let busy_s = eta.map_or(0.0, |e| forecast::busy_s(e, lease_left));
                let targets = leased.iter().find(|l| l.0 == name).map_or(1, |l| l.2.len().max(1));
                machines.push(Machine {
                    name,
                    speed,
                    measured,
                    equal: true,
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
        // (Each Mac held to what held it back over the last day: its lanes and their pace.)
        let held = forecast::held_back(&events, now_s());
        let machines = forecast::hold_back(machines, &held);
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
            regions: std::borrow::Cow::Owned(regions.to_vec()),
            costs: forecast::cost_table(regions, &cost),
            cadence,
            blind,
            chains: chain_jobs,
            after,
            idle,
            machines,
            running,
            macs: rooms,
            sched: self.coord.as_ref().map(|c| c.sched_state()),
            release: Vec::new(),
            held,
            pace: forecast::pace(&self.curves.borrow(), &events, now_s()),
            step_pace: BTreeMap::new(),
            paused: self.inp.pause.is_some(),
            took: BTreeMap::new(),
        };
        // (The time each Mac gave each step over the last day: what its lanes take, and first.)
        inp.took = forecast::took(&events, inp.now, &inp.macs_named());
        // (Each step's pace: its jobs of late against what the forecasts before them gave their
        // targets, at the lead's pace by their worker's speed.)
        let mut noted = self.noted.borrow_mut();
        noted.note(&inp);
        inp.step_pace = forecast::step_pace(&events, inp.now, &noted, &|w| pace(Some(&w.to_string())));
        drop(noted);
        // (Each lane's job under way held to its step's longest of late.)
        forecast::cap_busy(&mut inp, &forecast::longest_jobs(&events, now_s()));
        // (Each step's time a target, for the page to weigh the targets done: the headline.)
        let each_s = steps::TABLE.iter().map(|r| (r.name.to_string(), (per(r.name) * 10.0).round() / 10.0)).collect();
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
        let run = std::thread::Builder::new().name("forecast".into()).spawn(move || {
            forecast::low_priority();
            let took = std::time::Instant::now();
            let mut f = record.forecast();
            let entry = fcarchive::Entry::of(&record, &f, &app);
            // (Each Mac's tiles to come, for the page's map (`/work/tocome`), and the schedule's runs
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
            let upcoming = tocome.macs.get(&host).map(|m| (tocome.at, room::Hints::of_tocome(m, tocome.at)));
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
    /// with an item per step counted: the OSM pass (the planet, read when a newer one is on the
    /// NAS, and its worldwide steps), then the regions' steps (build::checklist).
    pub(super) fn checklist(&self, root: &store::nasfs::NasRoot, regions: &[recipes::Recipe]) -> Vec<build::Line> {
        let mut items = Vec::new();
        let mut next: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut notes: BTreeMap<&str, String> = BTreeMap::new();
        let have = self.latest_pass(root);
        // The planet: a newer one's under way (or waiting), else done.
        let newer = matches!(crate::osmpass::newer_planet(root, have.as_deref()), Ok(Some(_)));
        items.push(build::Item {
            step: "osm-pass".into(),
            left: Some((newer || have.is_none()) as usize),
            ..Default::default()
        });
        let Some(date) = have else {
            // Nothing to size the rest by until a pass is complete: its steps, to come.
            items.extend(WORLDWIDE_STEPS.iter().map(|s| build::Item { step: s.to_string(), ..Default::default() }));
            items.extend(build::checklist_to_come());
            return build::fold(&items, &next, &notes);
        };
        let crate::pool::records::Brief { manifest, keys, .. } = records_deadlined(root);
        // Countries the pass couldn't assemble, kept as the previous pass's outlines: said on the
        // pass's line until a pass has them whole (crate::outlines::assemble).
        if let Some(c) = manifest.get(&format!("sources/osm/{date}/outlines")) {
            if let Ok(stale) = crate::outlines::Outlines::stale_in(&root.join(c)) {
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
        // The pass's worldwide steps, each one run left or none.
        let left = [
            !crate::osmpass::SETS.iter().all(|st| manifest.contains_key(&crate::osmpass::set_name(&date, st.0))),
            build::outlines_work(&date, &manifest, &keys).is_some()
                || !manifest.contains_key(&format!("sources/osm/{date}/outlines")),
            build::trailends_work(&date, &manifest, &keys).is_some()
                || !manifest.contains_key(&crate::osmpass::set_name(&date, "hikes")),
            build::reach_work(&date, &manifest, &keys).is_some(),
            crate::buildtiles::release_of(root, &manifest)
                .map_or(true, |r| !manifest.contains_key(&crate::buildtiles::index_logical(&r))),
            build::summits_work(&date, &manifest, &keys).is_some()
                || !manifest.contains_key(&format!("work/summits/{date}")),
            build::labels_work(&date, &manifest, &keys).is_some(),
            // (A source folder that can't be read: held, left, as the jobs see it.)
            crate::lakefill::run::planned_source(root, &manifest)
                .map_or(true, |s| build::lakefill_work(&date, &manifest, &keys, s.as_deref()).is_some()),
            build::water_work(&date, &manifest, &keys).is_some(),
        ];
        items.extend(WORLDWIDE_STEPS.iter().zip(left).map(|(s, l)| build::Item {
            step: s.to_string(),
            left: Some(l as usize),
            ..Default::default()
        }));
        next.insert("pass", WORLDWIDE_STEPS.iter().zip(left).filter(|(_, l)| *l).map(|(s, _)| s.to_string()).collect());
        if regions.is_empty() {
            return build::fold(&items, &next, &notes);
        }
        let Ok(covs) = self.coverage(root, &manifest, false) else {
            return build::fold(&items, &next, &notes);
        };
        let cov = &covs.all;
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        let tiles = self.terrain_tiles(root, &manifest);
        let inputs = input_digests(root);
        // (As re-keyed: agent::rekey.)
        let mut keys = keys;
        let times = rekey::FileTimes::new(root);
        rekey::as_read(&mut keys, cov, &manifest, &tiles, &times);
        let (more, more_next) = build::checklist(
            cov,
            &date,
            &manifest,
            &keys,
            &inputs,
            root.join("inputs/hold-catalog").exists(),
            reach.as_deref(),
            &self.ready.borrow(),
            &tiles,
        );
        items.extend(more);
        next.extend(more_next);
        build::fold(&items, &next, &notes)
    }

    /// `f` (build::region_states, build::region_steps) over the coverage, the regions', the pass,
    /// the manifest, the records as every reader reads them (`as_built_keys`), the reaches and the
    /// terrain packs' indexes, on a thread of its own named `name` (the loop gives it what it holds
    /// already, and a copy of the indexes), with how long it took. None before the first pass or
    /// while the coverage can't be read.
    #[allow(clippy::type_complexity)]
    pub(super) fn start_as_built<T: Send + 'static>(
        &self,
        root: &store::nasfs::NasRoot,
        name: &str,
        f: fn(
            &crate::coverage::Coverage,
            &[(String, crate::coverage::Coverage)],
            &str,
            &BTreeMap<String, String>,
            &build::Keys,
            Option<&crate::reach::Reaches>,
            &tiles::TerrainTiles,
        ) -> T,
    ) -> Option<std::thread::JoinHandle<(T, Duration)>> {
        let (date, manifest, covs, keys, reach) = self.as_built_parts(root)?;
        let tiles = self.terrain_tiles(root, &manifest).clone();
        let root = root.clone();
        let run = move || {
            let t = Instant::now();
            let mut keys = keys;
            as_built_keys(&root, &covs.all, &manifest, &mut keys, &tiles);
            (f(&covs.all, &covs.each, &date, &manifest, &keys, reach.as_deref(), &tiles), t.elapsed())
        };
        std::thread::Builder::new()
            .name(name.into())
            .spawn(run)
            .inspect_err(|e| eprintln!("agent: the {name} thread: {e}"))
            .ok()
    }

    /// What `start_as_built` reads: the pass, the manifest, the coverages, the records as written and the
    /// reaches (the last two held: read again only when they change).
    #[allow(clippy::type_complexity)]
    pub(super) fn as_built_parts(
        &self,
        root: &store::nasfs::NasRoot,
    ) -> Option<(
        String,
        BTreeMap<String, String>,
        std::sync::Arc<Coverages>,
        build::Keys,
        Option<std::sync::Arc<crate::reach::Reaches>>,
    )> {
        let date = self.latest_pass(root)?;
        let crate::pool::records::Brief { manifest, keys, .. } = records_deadlined(root);
        let covs = self.coverage(root, &manifest, false).ok()?;
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        Some((date, manifest, covs, keys, reach))
    }

    /// The terrain packs' indexes `manifest` names, those not yet held read (from `pack-idx/`, else
    /// the NAS: half a minute for the whole build's, once).
    pub(super) fn terrain_tiles(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
    ) -> std::cell::Ref<'_, tiles::TerrainTiles> {
        let t = std::time::Instant::now();
        let n = self.tiles.borrow_mut().load(root, manifest);
        if n > 0 {
            eprintln!(
                "agent: read the indexes of {n} terrain pack{} in {:.1} s",
                if n == 1 { "" } else { "s" },
                t.elapsed().as_secs_f64()
            );
        }
        self.tiles.borrow()
    }

    /// The pass's reaches (crate::reach), once they're made for the current version (Ok(None)
    /// until then: no z6 tile is planned, and the reach job comes first among the pass's worldwide
    /// jobs), or why they can't be read.
    pub(super) fn current_reach(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
        keys: &build::Keys,
        date: &str,
    ) -> Result<Option<std::sync::Arc<crate::reach::Reaches>>, crate::reach::LoadError> {
        if build::reach_work(date, manifest, keys).is_some() {
            return Ok(None);
        }
        let Some(c) = manifest.get(&crate::reach::logical(date)) else { return Ok(None) };
        // (Read again when the base packs' extents kept with them change too: crate::extents.)
        let c = &format!("{c} {}", crate::extents::stamp(root));
        let mut cached = self.reach.borrow_mut();
        if let Some((have, r)) = cached.as_ref() {
            if have == c {
                return Ok(Some(r.clone()));
            }
        }
        let Some(r) = crate::reach::Reaches::load(root, manifest, date)? else { return Ok(None) };
        let r = std::sync::Arc::new(r);
        *cached = Some((c.clone(), r.clone()));
        Ok(Some(r))
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
        let a = crate::inputs::regions::read::source(root, manifest)
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
                let all = crate::coverage::Coverage::accepted(&root, &a, Some(&cache)).map_err(|e| format!("{e:#}"))?;
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
    /// version `a` or the served catalogue (the status's) changes, once each (a failed read isn't
    /// tried again until one of them changes), on a thread of its own (`regionsview::build`: the
    /// drawn files, the catalogue's sizes once per catalogue). Given to this Mac's coordinator (the
    /// page out of the pool) once made; the lead's pool step writes them for the others'
    /// (`pool_lead_files`).
    pub(super) fn regions_view(
        &self,
        root: &store::nasfs::NasRoot,
        a: &crate::inputs::Accepted,
        regions: &[recipes::Recipe],
    ) {
        self.take_regions();
        let cat = served((&root.join("catalog")).into()).map(|s| s.cat);
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

    /// The regions the last catalogue in `dir` has, by id: whether as its recipe is now (same outline
    /// entries); and how long ago it went out (seconds, by its file's time). Read once per catalogue;
    /// none (nothing on the map: every region new) when there's none, or it can't be read now.
    pub(super) fn on_map(
        &self,
        dir: store::nasfs::Loc<'_>,
        recipes: &[recipes::Recipe],
    ) -> (BTreeMap<String, bool>, Option<u64>) {
        let mut cached = self.last_catalogue.borrow_mut();
        // (The newest that reads, read once (`served`): a damaged one is passed over, as the map's
        // server does, not taken for none.)
        let mut at = None;
        if let Some(sv) = served(dir) {
            if cached.as_ref().is_none_or(|c| c.0 != (dir.display(), sv.seen.n)) {
                let regions: BTreeMap<String, Vec<String>> = sv
                    .cat
                    .coverage
                    .get("regions")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|r| {
                        Some((r["id"].as_str()?.to_string(), serde_json::from_value(r["outline"].clone()).ok()?))
                    })
                    .collect();
                *cached = Some(((dir.display(), sv.seen.n), regions));
            }
            at = Some(std::time::UNIX_EPOCH + Duration::from_secs(sv.seen.at));
        }
        // How long ago the last catalogue went out, or one last started (it may have failed): a
        // time ahead of this Mac's clock (the NAS's) counts as now.
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
        let had = &cached.as_ref().unwrap().1;
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
    pub(super) fn resume_hold(
        &self,
        root: &store::nasfs::NasRoot,
        newer: Option<&(NasPath, String)>,
    ) -> Option<Waiting> {
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
        // (The members as the loop's last pool step left them.)
        let mut members: BTreeSet<String> =
            self.inp.pool_seen.as_ref().map(|p| p.members.iter().cloned().collect()).unwrap_or_default();
        let id = format!("osm-pass {date}");
        let what = format!("Reading the OpenStreetMap planet of {date}");
        members.insert(me.clone());
        let mut files = Vec::new();
        for m in &members {
            match resume::read(root, m) {
                Ok(p) => files.extend(p),
                Err(e) => {
                    offer(None);
                    return Some(Waiting {
                        step: Some("osm-pass".into()),
                        what,
                        why: format!("a member's progress can't be read now ({e:#})"),
                    });
                }
            }
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
        match resume::decide(&id, &me, &files, now, &|m, at| resume::released(root, m, &id, at)) {
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
                Some((root.clone(), crate::inputs::watch::Watch::start(root.clone(), crate::inputs::watch::EVERY)));
        }
        // (Not a dry run's: the asks are the agent's that runs.)
        if self.o.dry_run {
            return;
        }
        let mut asks = crate::inputs::take_asks(&self.o.home);
        for c in [&self.coord, &self.inp.broker, &self.inp.viewer].into_iter().flatten() {
            asks.extend(c.take_inputs_asks());
        }
        if asks.is_empty() {
            return;
        }
        let member = crate::inputs::member_of(&self.o.home);
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
                    self.inputs.view.borrow().iter().find(|v| v.unit == a.unit).map(|v| crate::inputs::Report {
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
        keys: &build::Keys,
        waiting: &mut Vec<Waiting>,
    ) -> Vec<JobSpec> {
        let units = crate::inputs::units(root);
        let mut jobs = Vec::new();
        let mut checking: BTreeSet<String> = BTreeSet::new();
        // (Running now, on this Mac.)
        for r in self.inp.slots.iter().filter_map(|s| s.running.as_ref()) {
            if let Some(w) = r.spec.record.as_ref().filter(|w| w.step == "inputs") {
                checking.extend(w.targets.iter().filter_map(|t| t.0.strip_prefix("inputs/").map(str::to_string)));
            }
        }
        if let (Some((_, watch)), false) = (&self.inputs.watch, self.o.helper) {
            let s = |p: &Path| p.to_string_lossy().into_owned();
            for unit in &units {
                let Some(checks) = crate::inputs::checks(unit) else { continue };
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
                    && [crate::inputs::logical(unit), crate::inputs::held_logical(unit)]
                        .iter()
                        .all(|r| !manifest.contains_key(r))
                {
                    continue;
                }
                // (Daily by the records' last full check, whichever Mac made it: a new lead doesn't
                // repeat one made less than a day ago.)
                let full_at = manifest.get(&crate::inputs::listed_logical(unit)).and_then(|n| {
                    let mut seen = self.inputs.full_at.borrow_mut();
                    *seen
                        .entry(n.clone())
                        .or_insert_with(|| crate::inputs::read_listed_file(root, n).ok().and_then(|l| l.full_at))
                });
                let full = self.inputs.full.borrow().contains(*unit)
                    || full_at.is_none_or(|t| now_s().saturating_sub(t) >= 86400);
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
                // (Named by the z6 tile flat, `timetables+gtfs`: a nested z6 tile's name is one file's.)
                let flat = crate::inputs::flat(unit);
                let planned = self.o.home.join("inputs-listing").join(format!("{flat}.{key}.json"));
                let wrote =
                    std::fs::create_dir_all(planned.parent().unwrap()).map_err(anyhow::Error::from).and_then(|()| {
                        for e in std::fs::read_dir(planned.parent().unwrap())?.flatten() {
                            if e.file_name().to_string_lossy().starts_with(&format!("{flat}.")) && e.path() != planned {
                                std::fs::remove_file(e.path()).ok();
                            }
                        }
                        store::whole::write(
                            &planned,
                            &serde_json::to_vec(&crate::inputs::Planned {
                                listing: l.listing.clone(),
                                accepted: l.accepted.clone(),
                            })?,
                        )
                    });
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
                    record: Some(build::Work { step: "inputs".into(), targets: vec![(target, key)] }),
                });
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
                let named = [
                    crate::inputs::logical(unit),
                    crate::inputs::held_logical(unit),
                    crate::inputs::listed_logical(unit),
                ]
                .iter()
                .any(|l| manifest.contains_key(l));
                if units.contains(&unit.as_str()) || !named || !crate::inputs::valid_unit(unit) {
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
                    record: Some(build::Work {
                        step: "inputs".into(),
                        targets: vec![(format!("inputs/{unit}"), "forget".into())],
                    }),
                });
            }
        }
        let accepted = self.inputs.watch.as_ref().map(|(_, w)| w.accepted()).unwrap_or_default();
        *self.inputs.view.borrow_mut() =
            crate::inputs::view::of(root, manifest, &units, &checking, &accepted, &mut self.inputs.cache.borrow_mut());
        jobs
    }

    /// The regions' extract job (docs/inputs.md §6.1.6), when the regions' held report needs ids of
    /// the current pass and the job for them (and those the extract has) isn't done.
    pub(super) fn extract_job(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
        keys: &build::Keys,
    ) -> Option<JobSpec> {
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
            record: Some(build::Work { step: "regions-extract".into(), targets: vec![(target, key)] }),
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
}
