//! `scenic forecast-check` (docs/pool.md §7.9, task #621): the archived forecasts (crate::agent::fcarchive)
//! held to what happened (the history's jobs, crate::coord::history).
//!
//! A job done is a history's worker job (its "lease", then its "done" ok, by lease) or a lead's
//! ("end" ok: from its time less its `secs`): its Mac (the worker's name, a lead's slot's `#k`
//! cut), step, targets, start and end. A forecast
//! placed targets (step, target): each is held to the first job of that step and target that ended
//! after the forecast was made. The targets of one job are taken together: the forecast's start the
//! earliest of theirs, its end the latest, its Mac the first's lane's. So each (forecast, job) is a
//! sample: its end's error (forecast less actual: + late, − early) where the job ended after the
//! forecast; where it started after it, its start's (its start delay's error) and its run's (the
//! time from its start to its end, forecast less actual), the end's error the sum of the two; its
//! horizon the forecast's time until it (from when it was made; a run's, its end's). Errors are
//! said by step, by Mac (the Mac that ran it), by horizon and by the forecast's code version: n,
//! bias (the mean), the median and the 90th percentile of the absolute error.

use super::fcarchive::Entry;
use super::fcinputs::Input;
use super::forecast;
use crate::coord::history::Event;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;

/// The span a check reads by default (s back from now): the forecasts' archive and the forecast
/// inputs' (the days it keeps: crate::agent::fcinputs::KEEP_S).
pub const SPAN_S: u64 = 7 * 86_400;

/// A job done, as the history says it: its slot (the history's worker), its Mac.
#[derive(Clone, Debug, PartialEq)]
pub struct Actual {
    pub worker: String,
    pub mac: String,
    pub step: String,
    pub targets: Vec<String>,
    pub start: u64,
    pub end: u64,
}

/// The jobs done in `events`: workers' (lease to done) and the lead's (end, less its time), those
/// that ended well and name their targets; by end.
pub fn actuals(events: &[Event]) -> Vec<Actual> {
    let mut leased: BTreeMap<u64, u64> = BTreeMap::new();
    let mut out = Vec::new();
    for e in events {
        match e.kind.as_str() {
            "lease" => {
                if let Some(l) = e.lease {
                    leased.insert(l, e.t);
                }
            }
            "done" | "end" if e.ok == Some(true) && !e.targets.is_empty() => {
                let (Some(mac), Some(step)) = (&e.worker, &e.step) else { continue };
                let back = e.t.saturating_sub(e.secs.unwrap_or(0.0).max(0.0) as u64);
                let start = e.lease.and_then(|l| leased.get(&l).copied()).unwrap_or(back);
                // (A lead's slot past its first is named `<Mac> #k` in the history, as its lane.)
                let worker = mac.clone();
                let mac = mac.split(" #").next().unwrap_or(mac).to_string();
                out.push(Actual { worker, mac, step: step.clone(), targets: e.targets.clone(), start, end: e.t });
            }
            _ => {}
        }
    }
    out.sort_by_key(|a| a.end);
    out
}

/// What a sample's error is of: a job's end, its start, or its run (its start to its end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Of {
    End,
    Start,
    Run,
}

/// A forecast's error on a job's start, end or run.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub of: Of,
    pub step: String,
    pub mac: String,
    pub code: u32,
    /// How far ahead the forecast's time was (s from when it was made), and its error (s, forecast
    /// less actual).
    pub horizon_s: u64,
    pub err_s: f64,
}

/// n, the mean error, the median and 90th percentile of the absolute error (s).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Stats {
    pub n: usize,
    pub bias_s: f64,
    pub median_abs_s: f64,
    pub p90_abs_s: f64,
}

impl Stats {
    fn of(errs: &[f64]) -> Stats {
        if errs.is_empty() {
            return Stats::default();
        }
        let mut a: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
        a.sort_by(f64::total_cmp);
        let q = |p: f64| a[((a.len() as f64 * p).ceil() as usize).clamp(1, a.len()) - 1];
        Stats {
            n: errs.len(),
            bias_s: (errs.iter().sum::<f64>() / errs.len() as f64).round(),
            median_abs_s: q(0.5).round(),
            p90_abs_s: q(0.9).round(),
        }
    }
}

/// The start's or the end's errors, overall and by group.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Errors {
    pub all: Stats,
    pub by_horizon: Vec<(String, Stats)>,
    pub by_step: BTreeMap<String, Stats>,
    pub by_mac: BTreeMap<String, Stats>,
    pub by_code: BTreeMap<u32, Stats>,
}

/// The horizons errors are grouped by: up to (s), and their names.
pub const HORIZONS: [(u64, &str); 5] =
    [(900, "< 15 min"), (3600, "15 min-1 h"), (4 * 3600, "1-4 h"), (12 * 3600, "4-12 h"), (u64::MAX, "12 h +")];

impl Errors {
    fn of(s: &[&Sample]) -> Errors {
        let errs = |f: &dyn Fn(&Sample) -> bool| -> Vec<f64> { s.iter().filter(|x| f(x)).map(|x| x.err_s).collect() };
        let mut e = Errors { all: Stats::of(&errs(&|_| true)), ..Default::default() };
        let mut lo = 0;
        for (hi, name) in HORIZONS {
            let st = Stats::of(&errs(&|x| x.horizon_s >= lo && x.horizon_s < hi));
            if st.n > 0 {
                e.by_horizon.push((name.to_string(), st));
            }
            lo = hi;
        }
        let group = |key: &dyn Fn(&Sample) -> String| -> BTreeMap<String, Stats> {
            let mut m: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            for x in s {
                m.entry(key(x)).or_default().push(x.err_s);
            }
            m.into_iter().map(|(k, v)| (k, Stats::of(&v))).collect()
        };
        e.by_step = group(&|x| x.step.clone());
        e.by_mac = group(&|x| x.mac.clone());
        e.by_code = group(&|x| x.code.to_string()).into_iter().map(|(k, v)| (k.parse().unwrap_or(0), v)).collect();
        e
    }
}

/// How often the jobs ran in the order forecast: over the pairs of jobs a forecast placed that both
/// started after it was made (and not at the same time, nor forecast to), the share whose starts
/// came in the forecast's order; overall, and of the pairs both forecast to start within
/// `NEAR_S` of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Order {
    pub pairs: u64,
    pub in_order: f64,
    pub near_pairs: u64,
    pub near_in_order: f64,
}

/// The span `Order`'s near pairs are forecast to start within.
pub const NEAR_S: u64 = 4 * 3600;

/// The span `Report::work` holds the work placed to the work done over.
pub const WORK_S: u64 = 6 * 3600;

/// The check's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub from: u64,
    pub to: u64,
    /// Forecasts archived in the span, and jobs the history says were done in it.
    pub forecasts: usize,
    pub jobs_done: usize,
    /// Of those jobs, how many some forecast placed, and the share of the (forecast, job) pairs
    /// whose forecast had the Mac that ran it, and the slot.
    pub jobs_forecast: usize,
    pub same_mac: f64,
    pub same_slot: f64,
    pub order: Order,
    /// Targets forecast to end before the history's last event that no job did by then (a forecast
    /// each): work forecast that didn't happen as placed (dropped, failed, or under another target).
    pub not_done: usize,
    pub not_done_by_step: BTreeMap<String, usize>,
    /// The work placed against the work done (`WORK_S`): over the forecasts made `WORK_S` or more
    /// before the history's last event, the targets each forecast to end within `WORK_S` of it,
    /// and the targets the jobs that ended then did, by step (summed over the forecasts).
    pub work: BTreeMap<String, [u64; 2]>,
    /// The start's errors (its delay), the run's (its start to its end, by its end's horizon; a
    /// job started after the forecast) and the end's: of a job started after it, the end's error
    /// is its start's and its run's.
    pub start: Errors,
    pub run: Errors,
    pub end: Errors,
}

/// `entries` held to `jobs` (`actuals`): the samples, the targets forecast to end by `last` (unix s)
/// that no job did, by step, the jobs some forecast placed, the samples whose forecast had the Mac
/// and the slot that ran it, and the order's pairs (all, in order, near, near in order).
struct Matched {
    samples: Vec<Sample>,
    not_done: BTreeMap<String, usize>,
    placed: usize,
    same_mac: usize,
    same_slot: usize,
    pairs: [u64; 4],
    work: BTreeMap<String, [u64; 2]>,
}

fn matched(entries: &[Entry], jobs: &[Actual], last: u64) -> Matched {
    let mut by_target: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (k, a) in jobs.iter().enumerate() {
        for t in &a.targets {
            by_target.entry((&a.step, t)).or_default().push(k);
        }
    }
    let mut m = Matched {
        samples: Vec::new(),
        not_done: BTreeMap::new(),
        placed: 0,
        same_mac: 0,
        same_slot: 0,
        pairs: [0; 4],
        work: BTreeMap::new(),
    };
    let mut placed = vec![false; jobs.len()];
    for e in entries {
        // (The work placed in its first `WORK_S` against the work done then.)
        let till = e.at + WORK_S;
        if till <= last {
            for p in e.placed().iter().filter(|p| p.until <= till) {
                m.work.entry(p.step.clone()).or_default()[0] += 1;
            }
            for a in jobs.iter().filter(|a| a.end > e.at && a.end <= till) {
                m.work.entry(a.step.clone()).or_default()[1] += a.targets.len() as u64;
            }
        }
        // Job → (earliest start, latest end, its first target's lane), as this forecast placed it.
        let mut got: BTreeMap<usize, (u64, u64, usize)> = BTreeMap::new();
        for p in e.placed() {
            let (lane, from, until) = (&p.lane, &p.from, &p.until);
            let hit = by_target
                .get(&(p.step.as_str(), p.target.as_str()))
                .and_then(|ks| ks.iter().copied().find(|&k| jobs[k].end > e.at));
            match hit {
                Some(k) => {
                    let g = got.entry(k).or_insert((*from, *until, *lane));
                    g.0 = g.0.min(*from);
                    g.1 = g.1.max(*until);
                }
                None if *until < last => *m.not_done.entry(p.step.clone()).or_default() += 1,
                None => {}
            }
        }
        // (The jobs started after it: forecast start, actual start, for the order.)
        let mut starts: Vec<(u64, u64)> = Vec::new();
        for (k, (from, until, lane)) in got {
            let a = &jobs[k];
            placed[k] = true;
            let l = e.lanes.get(lane);
            m.same_mac += usize::from(l.is_some_and(|l| l.mac == a.mac));
            m.same_slot += usize::from(l.is_some_and(|l| l.name == a.worker));
            let s = |of: Of, horizon_s: u64, err_s: f64| Sample {
                of,
                step: a.step.clone(),
                mac: a.mac.clone(),
                code: e.code,
                horizon_s,
                err_s,
            };
            let horizon = until.saturating_sub(e.at);
            m.samples.push(s(Of::End, horizon, until as f64 - a.end as f64));
            if a.start > e.at {
                m.samples.push(s(Of::Start, from.saturating_sub(e.at), from as f64 - a.start as f64));
                let ran = a.end.saturating_sub(a.start) as f64;
                m.samples.push(s(Of::Run, horizon, until.saturating_sub(from) as f64 - ran));
                starts.push((from, a.start));
            }
        }
        for (i, x) in starts.iter().enumerate() {
            for y in &starts[i + 1..] {
                if x.0 == y.0 || x.1 == y.1 {
                    continue;
                }
                let ok = u64::from((x.0 < y.0) == (x.1 < y.1));
                m.pairs[0] += 1;
                m.pairs[1] += ok;
                if x.0.max(y.0) < e.at + NEAR_S {
                    m.pairs[2] += 1;
                    m.pairs[3] += ok;
                }
            }
        }
    }
    m.placed = placed.iter().filter(|p| **p).count();
    m
}

/// The check of the forecasts `entries` against `events`, over `from`..`to` (unix s): the jobs that
/// ended in it.
pub fn check(entries: &[Entry], events: &[Event], from: u64, to: u64) -> Report {
    let jobs: Vec<Actual> = actuals(events).into_iter().filter(|a| a.end >= from && a.end < to).collect();
    let last = events.iter().map(|e| e.t).max().unwrap_or(0).min(to);
    let m = matched(entries, &jobs, last);
    let of = |o: Of| -> Vec<&Sample> { m.samples.iter().filter(|x| x.of == o).collect() };
    let ends = of(Of::End);
    let share = |n: u64, of: u64| if of == 0 { 0.0 } else { (n as f64 / of as f64 * 100.0).round() / 100.0 };
    Report {
        from,
        to,
        forecasts: entries.len(),
        jobs_done: jobs.len(),
        jobs_forecast: m.placed,
        same_mac: share(m.same_mac as u64, ends.len() as u64),
        same_slot: share(m.same_slot as u64, ends.len() as u64),
        order: Order {
            pairs: m.pairs[0],
            in_order: share(m.pairs[1], m.pairs[0]),
            near_pairs: m.pairs[2],
            near_in_order: share(m.pairs[3], m.pairs[2]),
        },
        not_done: m.not_done.values().sum(),
        not_done_by_step: m.not_done,
        work: m.work,
        start: Errors::of(&of(Of::Start)),
        run: Errors::of(&of(Of::Run)),
        end: Errors::of(&ends),
    }
}

/// The history's events once each: a lead's file and a member's that led before both hold the
/// terms it led (as `backtest_against_the_history` reads them), in time order.
pub fn dedup(mut events: Vec<Event>) -> Vec<Event> {
    let key = |e: &Event| (e.t, e.kind.clone(), e.lease, e.worker.clone(), e.targets.clone());
    events.sort_by_key(key);
    events.dedup_by(|a, b| key(a) == key(b));
    events
}

/// The forecast inputs made from `from` to `to`, in time order: the archive's (crate::agent::fcinputs),
/// and the terms' (`state/build/term/<E>/forecast-input.json`, each term's last: docs/pool.md §10,
/// written before the archive while a member read the page's files on the NAS) at times it hasn't.
/// One that doesn't read is skipped, said.
pub fn inputs(root: &store::nasfs::NasRoot, from: u64, to: u64) -> Vec<Input> {
    let mut out = super::fcinputs::read(root, from, to);
    let archived: std::collections::BTreeSet<u64> = out.iter().map(|i| i.at).collect();
    for t in root.join("state/build/term").read_dir().unwrap_or_default() {
        let Some(term) = t.path.file_name().and_then(|n| n.parse().ok()) else { continue };
        let Ok(b) = t.path.join("forecast-input.json").read() else { continue };
        match Input::of_bytes(term, b) {
            Ok(i) if (from..to).contains(&i.at) && !archived.contains(&i.at) => out.push(i),
            Ok(_) => {}
            Err(e) => eprintln!("forecast-check: {}: {e}", t.path.file_name().unwrap_or("?")),
        }
    }
    out.sort_by_key(|i| i.at);
    out
}

/// `inputs` rerun (in time order, each read as it comes: one that doesn't read is skipped, said)
/// and archived as the lead archives its forecasts (`Entry::of`), as forecast code `code` made them
/// (`VERSION`, or the one before it, `Input::code`): each step's pace (`forecast::step_pace`) from the
/// history before it and the costs the records before it (and it) noted, each lane's job under way
/// held to its step's longest of late (`forecast::cap_busy`; a record holds its lanes' `busy_s` capped
/// already), the build's pause (`forecast::paused`) and each Mac's pace at each step (crate::paces: the
/// scheduler's, its costs and its lanes' times by it, `repace`) read from the history, each Mac's room
/// at its sustained best (`Rooms`, from the asks the inputs before it recorded) and each Mac's slots
/// its lanes (`unhold`: no hold of the day before). Code 7 also has the NAS's own gate open once the
/// jobs out then end; code 6, as recorded for the whole run.
pub fn replay(
    the_plan: &'static dyn crate::agent::seam::Plan,
    inputs: &[Input],
    events: &[Event],
    code: u32,
) -> Vec<Entry> {
    let mut noted = forecast::Noted::default();
    let mut rooms = Rooms::default();
    let mut out = Vec::new();
    for i in inputs {
        let mut r = match i.record() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("forecast-check: the input of term {} at {}: {e:#}", i.term, i.at);
                continue;
            }
        };
        let inp = &mut r.input;
        inp.code = code;
        inp.step_pace.clear();
        noted.note(the_plan, inp);
        rooms.note(the_plan, inp);
        rooms.set(inp);
        unhold(inp);
        inp.paused = forecast::paused(events, inp.now);
        repace(the_plan, inp, events);
        let paces = inp.sched.as_ref().map(|s| s.paces.clone()).unwrap_or_default();
        let speeds: BTreeMap<String, f64> = inp.machines.iter().map(|m| (m.name.clone(), m.speed)).collect();
        let speed = |w: &str, step: &str| {
            if paces.known() {
                1.0 / paces.get(w, step)
            } else {
                speeds.get(w).copied().unwrap_or(1.0)
            }
        };
        inp.step_pace = forecast::step_pace(events, inp.now, &noted, &speed);
        let longest = forecast::longest_jobs(events, inp.now);
        forecast::cap_busy(inp, &longest);
        let f = r.forecast(the_plan);
        let mut e = Entry::of(&r, &f, "replay");
        e.code = code;
        out.push(e);
    }
    out
}

/// `inp`'s lanes as code 5 makes them (task #951 review): a record before it has each Mac's lanes
/// cut to the jobs it ran at once the day before (#563's hold, gone), its idle ones dropped, so
/// replayed as it stands code 5 ran on 3 to 5 lanes where the live run has some 30. Each Mac whose
/// ask is recorded gets back the slots it may run (crate::agent's `forecast_record`): a member,
/// `job_slots` of its cores; the lead, its ask's slots free and its lanes running (`Ask::want`, its
/// ramp's slots less its jobs; one, when it runs none, leaves them unknown and its lanes as they
/// are). A Mac with no lane (paused alone) gets none; a record made by code 5 has them already.
fn unhold(inp: &mut forecast::Input) {
    let Some(st) = inp.sched.as_ref() else { return };
    let mut more = Vec::new();
    for w in &st.workers {
        let g = &w.ask.worker;
        let of: Vec<&forecast::Machine> =
            inp.machines.iter().filter(|m| m.mac.as_deref().unwrap_or(&m.name) == g.as_str()).collect();
        let Some(first) = of.first() else { continue };
        let running = of.iter().filter(|m| m.busy_s > 0.0 || m.running.is_some()).count();
        let slots = if *g == st.lead_name {
            if w.ask.want <= 1 && running == 0 {
                continue;
            }
            w.ask.want + running
        } else {
            match w.ask.cap.as_ref().filter(|c| c.cores > 0) {
                Some(c) => super::job_slots(c.cores as usize),
                None => continue,
            }
        };
        let names: std::collections::BTreeSet<&str> = of.iter().map(|m| m.name.as_str()).collect();
        let free = (0..super::MAX_SLOTS).map(|k| super::slot_worker(g, k)).filter(|n| !names.contains(n.as_str()));
        for name in free.take(slots.saturating_sub(of.len())) {
            more.push(forecast::Machine { name, busy_s: 0.0, running: None, from_s: 0.0, ..(*first).clone() });
        }
    }
    inp.machines.extend(more);
}

/// Each Mac's room as the inputs' recorded asks said it (`Rooms::note`), at each input's time: its
/// memory for jobs (what its jobs held and what was available, less its memory floor: its memory
/// less what the rest of the Mac used and the floor, as `live::Store::best` reads it) and its cores
/// less those busy outside the pool (`State::cores_left`), by Mac.
#[derive(Default)]
struct Rooms {
    seen: BTreeMap<String, Vec<(u64, f64, f64)>>,
}

impl Rooms {
    /// `inp`'s asks' rooms noted.
    fn note(&mut self, the_plan: &'static dyn crate::agent::seam::Plan, inp: &forecast::Input) {
        let Some(st) = inp.sched.as_ref() else { return };
        for w in &st.workers {
            let Some(c) = w.ask.cap.as_ref().filter(|c| c.avail_mb > 0) else { continue };
            let mem = (c.held_mb + c.avail_mb).saturating_sub(c.mem_floor_mb) as f64;
            let mine = self.seen.entry(w.ask.worker.clone()).or_default();
            mine.push((inp.now, mem, f64::from(st.cores_left(the_plan, &w.ask))));
        }
    }

    /// Each of `inp`'s asks' room at its sustained best (`Capacity::best`), when none was said: over
    /// the inputs noted to it (`forecast::sustained`), each standing until the next. An input is
    /// archived every `fcinputs::EVERY_S` at most (the first forecast after), so two more
    /// than twice that apart had one lost between them, and the run breaks there (the live one reads
    /// its Mac's minute samples).
    fn set(&self, inp: &mut forecast::Input) {
        let now = inp.now;
        let Some(st) = inp.sched.as_mut() else { return };
        for w in st.workers.iter_mut() {
            let Some(c) = w.ask.cap.as_mut() else { continue };
            let Some(seen) = self.seen.get(&w.ask.worker) else { continue };
            let pts =
                |k: usize| -> Vec<(u64, f64)> { seen.iter().map(|x| (x.0, if k == 0 { x.1 } else { x.2 })).collect() };
            let step = 2 * super::fcinputs::EVERY_S;
            let found = (|| {
                Some(forecast::MacRoom {
                    mem_mb: forecast::sustained(&pts(0), step, now)? as u64,
                    cores: (forecast::sustained(&pts(1), step, now)? as u32).max(1),
                })
            })();
            c.best = c.best.or(found);
        }
    }
}

/// Record input `inp` as code 4 makes it (task #926): each Mac's pace at each step from `events`
/// to its time (crate::paces, against its first lane's Mac, the lead), the scheduler's; and its
/// shared steps' costs made again by it (`recost`).
fn repace(the_plan: &'static dyn crate::agent::seam::Plan, inp: &mut forecast::Input, events: &[Event]) {
    let Some(lead) = inp.machines.first().map(|m| m.mac.clone().unwrap_or_else(|| m.name.clone())) else { return };
    let Some(st) = inp.sched.as_mut() else { return };
    st.paces = crate::paces::Paces::of(events, &lead, inp.now).with_starts(the_plan, events, &st.costs, inp.now);
    recost(the_plan, inp);
}

/// Record input `inp`'s shared steps' costs made as the lead makes them now (`forecast::target_cost`),
/// from its scheduler's measures, at the lead's pace by its paces (`forecast::at_lead`,
/// `step_sums`): a target's own run of the way its step runs now (crate::needs::Book::own, as the
/// coordinator sizes it), known; else its step's mean. A cost its record doesn't hold the measures
/// of (no scheduler, a step none of whose targets it holds, a step none of whose runs is of the
/// way it runs now: its time) stays as recorded, as does a Mac's whose pace isn't known (taken at
/// its lane's recorded speed, as the record made it). The replay of an archived input (`replay`;
/// forecast's `recorded_timing` with SCENIC_FORECAST_RECOST) so runs this build's cost table.
pub fn recost(the_plan: &'static dyn crate::agent::seam::Plan, inp: &mut forecast::Input) {
    let Some(st) = inp.sched.as_ref() else { return };
    let speeds: BTreeMap<String, f64> = inp.machines.iter().map(|m| (m.name.clone(), m.speed)).collect();
    let speed = |w: Option<&str>| w.and_then(|w| speeds.get(w)).copied().unwrap_or(1.0);
    let at = |c: &crate::coord::Cost, step: &str| forecast::at_lead(c, step, &st.paces, &speed);
    let sums = forecast::step_sums(the_plan, &st.costs, &at);
    let book = crate::needs::Book::new(the_plan, &st.costs);
    let held: std::collections::BTreeSet<&str> =
        st.costs.keys().map(|k| crate::agent::memguard::step_of_key(k)).collect();
    let put = |step: &str, target: &str, c: &mut forecast::Cost| {
        if held.contains(step) {
            let was = c.secs;
            let per = |s: &str| sums.get(s).filter(|x| x.1 > 0.0).map_or(was, |x| x.0 / x.1);
            *c = forecast::target_cost(&book, step, target, &at, &per, c.peak_mb);
        }
    };
    for (step, ts) in inp.costs.iter_mut() {
        for (t, c) in ts.iter_mut() {
            put(step, t, c);
        }
    }
    for (step, t, c) in inp.chains.iter_mut().chain(inp.after.iter_mut()).chain(inp.idle.iter_mut()) {
        if the_plan.shared().contains(&step.as_str()) {
            put(step, t, c);
        }
    }
}

/// The backtest's answer: the inputs before `split` (train) and from it (test), each replayed
/// old and new and checked against the history's jobs that ended from the record on.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Backtest {
    pub split: u64,
    pub train: [Report; 2],
    pub test: [Report; 2],
    /// The jobs' lengths against what the scheduler sized them to (`batches`), over the whole span.
    pub batches: Vec<Batches>,
}

/// A step's jobs on a Mac (`batches`): each one's time over the time the scheduler sizes it to
/// (`ratio`), all of them and those sized to half the batch length or more (`long`: the jobs the
/// length bound ended, or near it, where a short one ended for want of work offered).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Batches {
    pub step: String,
    pub mac: String,
    pub all: Ratio,
    pub long: Ratio,
}

/// Jobs' time over their sized time: their count, median and 90th percentile (nearest rank).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Ratio {
    pub n: usize,
    pub median: f64,
    pub p90: f64,
}

impl Ratio {
    fn of(mut v: Vec<f64>) -> Ratio {
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v.get(((v.len() as f64 * q).ceil() as usize).saturating_sub(1)).copied().unwrap_or(0.0);
        Ratio { n: v.len(), median: at(0.5), p90: at(0.9) }
    }
}

/// The history's jobs of the shared steps (`actuals`) that started in `from..to`, each against
/// the time the scheduler of the input recorded last before its start sizes it to take its worker
/// (`Sched::batch_secs` on that input's state at the threads it'd be granted alone, with each Mac's
/// pace and jobs' start worked out from `events` to then as the lead does, crate::paces): its time
/// (its end less its grant) over that, by step and Mac (task #927). A job of no time sized (a target
/// measured at none) isn't counted.
pub fn batches(
    the_plan: &'static dyn crate::agent::seam::Plan,
    inputs: &[Input],
    events: &[Event],
    from: u64,
    to: u64,
) -> Vec<Batches> {
    use crate::coord::sched::{Clock, Sched};
    let jobs: Vec<Actual> = actuals(events)
        .into_iter()
        .filter(|a| (from..to).contains(&a.start) && the_plan.shared().contains(&a.step.as_str()))
        .collect();
    let mut by: BTreeMap<(String, String), (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for (k, i) in inputs.iter().enumerate() {
        let until = inputs.get(k + 1).map_or(u64::MAX, |n| n.at);
        let ours: Vec<&Actual> = jobs.iter().filter(|a| (i.at..until).contains(&a.start)).collect();
        if ours.is_empty() {
            continue;
        }
        let Ok(r) = i.record() else { continue };
        let Some(mut st) = r.input.sched.clone() else { continue };
        let lead = if st.lead_name.is_empty() {
            r.input.machines.first().map(|m| m.mac.clone().unwrap_or_else(|| m.name.clone())).unwrap_or_default()
        } else {
            st.lead_name.clone()
        };
        st.paces = crate::paces::Paces::of(events, &lead, i.at).with_starts(the_plan, events, &st.costs, i.at);
        let s = Sched::from_state(
            the_plan,
            &st,
            Clock { now: std::time::Instant::now(), unix: i.at },
            std::time::Duration::from_secs(86_400),
        );
        for a in ours {
            // (At the threads it'd be granted alone, as `pick_every` sizes it.)
            let Some(first) = a.targets.first() else { continue };
            let total = s.workers.get(&a.mac).and_then(|w| w.ask.as_ref()).map_or(0, crate::coord::ask_cores);
            let threads = s.sizing_threads(&a.worker, &a.step, first, total.max(1));
            let Some((sized, _)) = s.batch_secs(&a.step, a.targets.iter().map(String::as_str), &a.worker, threads)
            else {
                continue;
            };
            let ratio = a.end.saturating_sub(a.start) as f64 / sized.max(1) as f64;
            let e = by.entry((a.step.clone(), a.mac.clone())).or_default();
            e.0.push(ratio);
            if sized * 2 >= s.batch_len() {
                e.1.push(ratio);
            }
        }
    }
    by.into_iter()
        .map(|((step, mac), (all, long))| Batches { step, mac, all: Ratio::of(all), long: Ratio::of(long) })
        .collect()
}

/// The backtest of `inputs` (`replay`, old and new) against `events`, over `from`..`to`.
pub fn backtest(
    the_plan: &'static dyn crate::agent::seam::Plan,
    inputs: &[Input],
    events: &[Event],
    from: u64,
    to: u64,
    split: u64,
) -> Backtest {
    // (One code after the other: side by side, the M1's replay of 60 inputs peaked at a 4.7 GB
    // footprint, task #988; each forecast's three runs go side by side already.)
    let (old, new) =
        (replay(the_plan, inputs, events, forecast::VERSION - 1), replay(the_plan, inputs, events, forecast::VERSION));
    let part =
        |es: &[Entry], test: bool| -> Vec<Entry> { es.iter().filter(|e| (e.at >= split) == test).cloned().collect() };
    let rep = |es: &[Entry], test: bool| check(&part(es, test), events, if test { split } else { from }, to);
    Backtest {
        split,
        train: [rep(&old, false), rep(&new, false)],
        test: [rep(&old, true), rep(&new, true)],
        batches: batches(the_plan, inputs, events, from, to),
    }
}

/// The backtest as text: for the test set, then the train set, the end's and the start's errors
/// old against new (minutes), and the order.
pub fn backtest_text(b: &Backtest) -> String {
    let mut o = String::new();
    let m = |s: f64| s / 60.0;
    let _ = writeln!(
        o,
        "Backtest: the terms' forecast inputs rerun, old (code {}) against new (code {}).",
        forecast::VERSION - 1,
        forecast::VERSION
    );
    let _ = writeln!(o, "Error = forecast - actual, minutes; median and p90 of the absolute error.");
    let d = super::backup::format_day(std::time::UNIX_EPOCH + std::time::Duration::from_secs(b.split));
    let split = format!("{d} {:02}:{:02} UTC", b.split % 86_400 / 3600, b.split % 3600 / 60);
    for (name, r) in [(format!("Test (from {split})"), &b.test), ("Train (before it)".to_string(), &b.train)] {
        let _ = writeln!(
            o,
            "\n{name}: {} inputs, {} jobs done, {} forecast.",
            r[0].forecasts, r[0].jobs_done, r[0].jobs_forecast
        );
        let _ = writeln!(
            o,
            "  targets forecast done by the history's end that weren't: {} -> {}",
            r[0].not_done, r[1].not_done
        );
        let work = |r: &Report| r.work.values().fold([0, 0], |a, w| [a[0] + w[0], a[1] + w[1]]);
        let (a, n) = (work(&r[0]), work(&r[1]));
        let _ = writeln!(
            o,
            "  targets placed in a forecast's first {} h: {} -> {}; done then {}",
            WORK_S / 3600,
            a[0],
            n[0],
            a[1]
        );
        for (k, w) in &r[0].work {
            let p = r[1].work.get(k).map_or(0, |w| w[0]);
            let _ = writeln!(o, "    {k:<20} placed {:>9} -> {:>9}; done {:>9}", w[0], p, w[1]);
        }
        let _ = writeln!(
            o,
            "  same Mac {:.0} % -> {:.0} %; same slot {:.0} % -> {:.0} %; in order {:.0} % -> {:.0} % ({} pairs), within {} h {:.0} % -> {:.0} %",
            r[0].same_mac * 100.0,
            r[1].same_mac * 100.0,
            r[0].same_slot * 100.0,
            r[1].same_slot * 100.0,
            r[0].order.in_order * 100.0,
            r[1].order.in_order * 100.0,
            r[0].order.pairs,
            NEAR_S / 3600,
            r[0].order.near_in_order * 100.0,
            r[1].order.near_in_order * 100.0,
        );
        for (what, old, new) in
            [("End", &r[0].end, &r[1].end), ("Start", &r[0].start, &r[1].start), ("Run", &r[0].run, &r[1].run)]
        {
            let _ = writeln!(
                o,
                "  {what:<22} {:>6} {:>8} {:>8} {:>8} | {:>6} {:>8} {:>8} {:>8}",
                "n", "bias", "median", "p90", "n", "bias", "median", "p90"
            );
            let row = |o: &mut String, k: &str, a: &Stats, b: &Stats| {
                let _ = writeln!(
                    o,
                    "    {k:<20} {:>6} {:>+8.1} {:>8.1} {:>8.1} | {:>6} {:>+8.1} {:>8.1} {:>8.1}",
                    a.n,
                    m(a.bias_s),
                    m(a.median_abs_s),
                    m(a.p90_abs_s),
                    b.n,
                    m(b.bias_s),
                    m(b.median_abs_s),
                    m(b.p90_abs_s)
                );
            };
            row(&mut o, "all", &old.all, &new.all);
            for (h, s) in &old.by_horizon {
                let n = new.by_horizon.iter().find(|x| &x.0 == h).map(|x| x.1.clone()).unwrap_or_default();
                row(&mut o, h, s, &n);
            }
            for (k, s) in &old.by_step {
                row(&mut o, k, s, &new.by_step.get(k).cloned().unwrap_or_default());
            }
        }
    }
    let _ = writeln!(
        o,
        "\nJobs' time over the time the scheduler sizes them to (all; sized to half the batch length or more):"
    );
    let _ = writeln!(
        o,
        "  {:<14} {:<24} {:>5} {:>7} {:>7} | {:>5} {:>7} {:>7}",
        "step", "Mac", "n", "median", "p90", "n", "median", "p90"
    );
    for b in &b.batches {
        let _ = writeln!(
            o,
            "  {:<14} {:<24} {:>5} {:>7.2} {:>7.2} | {:>5} {:>7.2} {:>7.2}",
            b.step, b.mac, b.all.n, b.all.median, b.all.p90, b.long.n, b.long.median, b.long.p90
        );
    }
    o
}

/// The report as text: a table each for the end, the start and the run, in minutes.
pub fn text(r: &Report) -> String {
    let day = |t: u64| {
        let d = super::backup::format_day(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t));
        format!("{d} {:02}:{:02}", t % 86_400 / 3600, t % 3600 / 60)
    };
    let mut o = String::new();
    let _ = writeln!(o, "Forecasts made {} to {} UTC: {} archived.", day(r.from), day(r.to), r.forecasts);
    let _ = writeln!(
        o,
        "Jobs done then: {}, {} of them forecast; the forecast had the Mac that ran it {:.0} % of the time; {} targets forecast to be done by the history's end weren't.",
        r.jobs_done,
        r.jobs_forecast,
        r.same_mac * 100.0,
        r.not_done
    );
    if !r.not_done_by_step.is_empty() {
        let by: Vec<String> = r.not_done_by_step.iter().map(|(k, n)| format!("{k} {n}")).collect();
        let _ = writeln!(o, "  (by step: {})", by.join(", "));
    }
    let _ = writeln!(
        o,
        "The slot that ran it {:.0} % of the time. Jobs started in the order forecast: {:.0} % of {} pairs; of those forecast to start within {} h, {:.0} % of {}.",
        r.same_slot * 100.0,
        r.order.in_order * 100.0,
        r.order.pairs,
        NEAR_S / 3600,
        r.order.near_in_order * 100.0,
        r.order.near_pairs
    );
    if !r.work.is_empty() {
        let by: Vec<String> = r.work.iter().map(|(k, w)| format!("{k} {}/{}", w[0], w[1])).collect();
        let _ = writeln!(
            o,
            "Targets placed in a forecast's first {} h / done then (summed over the forecasts): {}",
            WORK_S / 3600,
            by.join(", ")
        );
    }
    let _ = writeln!(
        o,
        "Error = forecast - actual, in minutes (+: the forecast was late; -: early); a sample a forecast and job."
    );
    let m = |s: f64| s / 60.0;
    let row = |o: &mut String, name: &str, s: &Stats| {
        let _ = writeln!(
            o,
            "  {name:<26} {:>7} {:>+8.1} {:>8.1} {:>8.1}",
            s.n,
            m(s.bias_s),
            m(s.median_abs_s),
            m(s.p90_abs_s)
        );
    };
    for (what, e) in [("End", &r.end), ("Start", &r.start), ("Run (start to end)", &r.run)] {
        let _ = writeln!(o, "\n{what:<28} {:>7} {:>8} {:>8} {:>8}", "n", "bias", "median", "p90");
        row(&mut o, "all", &e.all);
        let _ = writeln!(o, "  by horizon");
        for (h, s) in &e.by_horizon {
            row(&mut o, &format!("  {h}"), s);
        }
        let _ = writeln!(o, "  by step");
        for (k, s) in &e.by_step {
            row(&mut o, &format!("  {k}"), s);
        }
        let _ = writeln!(o, "  by Mac");
        for (k, s) in &e.by_mac {
            row(&mut o, &format!("  {k}"), s);
        }
        if e.by_code.len() > 1 {
            let _ = writeln!(o, "  by forecast code");
            for (k, s) in &e.by_code {
                row(&mut o, &format!("  v{k}"), s);
            }
        }
    }
    o
}

/// A time as the command takes it (UTC): unix seconds, YYYY-MM-DD, YYYY-MM-DDTHH:MM (or with a
/// space), or a span back from `now` (30m, 12h, 3d).
pub fn parse_time(s: &str, now: u64) -> Option<u64> {
    let s = s.trim();
    if let Ok(t) = s.parse::<u64>() {
        return Some(t);
    }
    if let Some((n, unit)) = s.char_indices().last().map(|(i, c)| (&s[..i], c)) {
        let by = match unit {
            'm' => Some(60),
            'h' => Some(3600),
            'd' => Some(86_400),
            _ => None,
        };
        if let (Some(by), Ok(n)) = (by, n.parse::<u64>()) {
            return Some(now.saturating_sub(n * by));
        }
    }
    let day = super::backup::parse_day(s.get(..10)?)?;
    match s.get(10..) {
        Some("") => Some(day),
        Some(rest) => {
            let hm = rest.strip_prefix(['T', ' '])?;
            let (h, m) = hm.split_once(':')?;
            Some(day + h.parse::<u64>().ok()? * 3600 + m.get(..2)?.parse::<u64>().ok()? * 60)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fcarchive::Lane;
    use super::*;

    fn ev(
        t: u64,
        kind: &str,
        worker: &str,
        lease: Option<u64>,
        step: &str,
        targets: &[&str],
        secs: Option<f64>,
    ) -> Event {
        Event {
            t,
            kind: kind.into(),
            worker: Some(worker.into()),
            lease,
            step: Some(step.into()),
            targets: targets.iter().map(|t| t.to_string()).collect(),
            secs,
            ok: (kind == "done" || kind == "end").then_some(true),
            ..Default::default()
        }
    }

    fn entry(at: u64, jobs: Vec<(usize, usize, &str, u64, u64)>) -> Entry {
        let steps = ["unit", "end-step"];
        let placed: Vec<super::super::forecast::Placed> = jobs
            .into_iter()
            .map(|j| super::super::forecast::Placed {
                lane: j.0,
                step: steps[j.1].into(),
                target: j.2.into(),
                from: at + j.3,
                until: at + j.4,
            })
            .collect();
        let mut e = Entry {
            at,
            code: 1,
            lanes: vec![
                Lane { name: "lead".into(), mac: "lead".into(), ..Default::default() },
                Lane { name: "m".into(), mac: "m".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        e.set_placed(&placed);
        e
    }

    #[test]
    fn jobs_are_the_leases_done_and_the_leads_ends() {
        let es = vec![
            ev(1000, "lease", "m", Some(7), "unit", &["a", "b"], None),
            ev(1600, "done", "m", Some(7), "unit", &["a", "b"], Some(590.0)),
            ev(2000, "end", "lead", None, "end-step", &["x"], Some(300.0)),
            ev(2100, "fail", "m", Some(8), "unit", &["c"], None),
        ];
        let a = actuals(&es);
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].start, a[0].end, a[0].mac.as_str()), (1000, 1600, "m"));
        assert_eq!((a[1].start, a[1].end), (1700, 2000));
        // A lead's slot past its first: its Mac.
        assert_eq!(actuals(&[ev(9, "end", "lead #3", None, "unit", &["z"], Some(1.0))])[0].mac, "lead");
    }

    #[test]
    fn a_jobs_targets_are_held_together_and_errors_signed_forecast_less_actual() {
        let es = vec![
            ev(1000, "lease", "m", Some(7), "unit", &["a", "b"], None),
            ev(1600, "done", "m", Some(7), "unit", &["a", "b"], None),
            ev(2000, "end", "lead", None, "end-step", &["x"], Some(300.0)),
            ev(5000, "agent", "lead", None, "", &[], None),
        ];
        // Made at 400: a and b on m from 700 (300 s on) to 1300 and 1900; x on the lead, 1500-2100;
        // y never done.
        let e = entry(
            400,
            vec![(1, 0, "a", 300, 900), (1, 0, "b", 900, 1500), (0, 1, "x", 1100, 1700), (0, 0, "y", 100, 200)],
        );
        let r = check(&[e], &es, 0, 10_000);
        assert_eq!((r.jobs_done, r.jobs_forecast, r.not_done, r.same_mac), (2, 2, 1, 1.0));
        assert_eq!(r.not_done_by_step, [("unit".to_string(), 1)].into());
        // Ends: a+b forecast 1900 against 1600 (+300); x 2100 against 2000 (+100).
        assert_eq!(r.end.all.n, 2);
        assert_eq!(r.end.all.bias_s, 200.0);
        assert_eq!(r.end.by_step["unit"].bias_s, 300.0);
        assert_eq!(r.end.by_mac["lead"].median_abs_s, 100.0);
        // Starts: a+b 700 against 1000 (-300); x 1500 against 1700 (-200).
        assert_eq!(r.start.all.bias_s, -250.0);
        assert_eq!(r.start.all.p90_abs_s, 300.0);
        // Runs: a+b 1200 s against 600 (+600), x 600 against 300 (+300): each end's error its
        // start's and its run's (+300 = -300 + 600; +100 = -200 + 300), by the end's horizon.
        assert_eq!((r.run.all.n, r.run.all.bias_s, r.run.by_step["unit"].bias_s), (2, 450.0, 600.0));
        assert_eq!(r.run.by_horizon[0].0, "15 min-1 h");
        assert!(text(&r).contains("Run (start to end)"), "{}", text(&r));
        // Horizons: the ends 1500 s and 1700 s out: 15 min-1 h.
        assert_eq!(r.end.by_horizon, [("15 min-1 h".to_string(), r.end.all.clone())]);
        let t = text(&r);
        assert!(t.contains("by horizon") && t.contains("unit"), "{t}");
    }

    #[test]
    fn a_forecast_made_once_a_job_ran_holds_to_its_end_alone_and_the_next_run_after_it() {
        let es = vec![
            ev(1000, "lease", "m", Some(7), "unit", &["a"], None),
            ev(1600, "done", "m", Some(7), "unit", &["a"], None),
            ev(3000, "lease", "m", Some(9), "unit", &["a"], None),
            ev(3600, "done", "m", Some(9), "unit", &["a"], None),
        ];
        let r = check(
            &[entry(1200, vec![(1, 0, "a", 0, 500)]), entry(2000, vec![(1, 0, "a", 1000, 1500)])],
            &es,
            0,
            10_000,
        );
        // The first: running, its end alone, 1700 against 1600 (+100); the second: the rerun, 3000-3500
        // against 3000-3600 (0, -100).
        assert_eq!(r.end.all.n, 2);
        assert_eq!(r.start.all.n, 1);
        assert_eq!(r.start.all.bias_s, 0.0);
        assert_eq!(r.end.all.bias_s, 0.0);
    }

    #[test]
    fn the_order_and_the_slot_are_held_to_the_history() {
        // a, b, c forecast to start in that order on m; they started b, a, c, b on m's second slot.
        let es = vec![
            ev(1000, "lease", "m", Some(1), "unit", &["a"], None),
            ev(1500, "done", "m", Some(1), "unit", &["a"], None),
            ev(900, "lease", "m #2", Some(2), "unit", &["b"], None),
            ev(1400, "done", "m #2", Some(2), "unit", &["b"], None),
            ev(2000, "lease", "m", Some(3), "unit", &["c"], None),
            ev(2500, "done", "m", Some(3), "unit", &["c"], None),
        ];
        let e = entry(500, vec![(1, 0, "a", 100, 600), (1, 0, "b", 600, 1100), (1, 0, "c", 20_000, 20_500)]);
        let r = check(&[e], &es, 0, 30_000);
        // Pairs: (a, b) out of order; (a, c), (b, c) in order; c past the near span.
        assert_eq!(r.order, Order { pairs: 3, in_order: 0.67, near_pairs: 1, near_in_order: 0.0 });
        assert_eq!((r.same_mac, r.same_slot), (1.0, 0.67));
        assert!(text(&r).contains("in the order forecast: 67 % of 3 pairs"), "{}", text(&r));
    }

    /// The replay: old runs each target at its cost; new at its step's pace from the jobs done
    /// since an earlier record noted their costs, and is archived as code `VERSION`.
    #[test]
    fn the_replay_runs_old_and_new_on_the_records() {
        use super::super::forecast::{Cadence, Cost, Input, Machine, Record};
        let rec = |now: u64, targets: std::ops::Range<u32>| {
            let c = Cost { secs: 600.0, known: true, peak_mb: 100 };
            Record {
                input: Input {
                    now,
                    before: Vec::new(),
                    drops: Vec::new(),
                    regions: std::borrow::Cow::Owned(Vec::new()),
                    costs: Default::default(),
                    cadence: Cadence::default(),
                    blind: None,
                    chains: targets.map(|i| ("depth".to_string(), format!("6/1/{i}"), c)).collect(),
                    after: Vec::new(),
                    idle: Vec::new(),
                    machines: vec![Machine {
                        name: "m4".into(),
                        speed: 1.0,
                        measured: true,
                        mem_mb: 8000,
                        ..Default::default()
                    }],
                    running: BTreeMap::new(),
                    macs: BTreeMap::new(),
                    sched: None,
                    pace: None,
                    step_pace: Default::default(),
                    unknown: Vec::new(),
                    paused: false,
                    along: Default::default(),
                    code: crate::agent::forecast::VERSION,
                },
                each_s: BTreeMap::new(),
                waiting: BTreeMap::new(),
            }
        };
        // Three depth targets noted at 600 s at 10,000, done by 12,000 in 300 s each; the next
        // record's left at 20,000.
        let es: Vec<Event> =
            (0..3).map(|i| ev(11_000 + i, "end", "m4", None, "depth", &[&format!("6/1/{i}")], Some(300.0))).collect();
        let records: Vec<super::super::fcinputs::Input> = [rec(10_000, 0..3), rec(20_000, 3..5)]
            .iter()
            .map(|r| super::super::fcinputs::Input::of_bytes(1, r.bytes().unwrap()).unwrap())
            .collect();
        let span = |es: &[Entry]| {
            let p = es[1].placed();
            (es[1].code, p[0].until - p[0].from)
        };
        let (old, new) = (forecast::VERSION - 1, forecast::VERSION);
        assert_eq!(span(&replay(crate::agent::seam::for_tests(), &records, &es, old)), (old, 300));
        assert_eq!(span(&replay(crate::agent::seam::for_tests(), &records, &es, new)), (new, 300));
        // Paused by then (the history's pause, for every code): nothing placed.
        let mut paused = es.clone();
        paused.push(Event { t: 15_000, kind: "pause".into(), ..Default::default() });
        let replayed = replay(crate::agent::seam::for_tests(), &records, &paused, new);
        assert!(replayed[1].placed().is_empty() && replayed[1].why.as_deref() == Some(forecast::PAUSED));
        assert_eq!(replayed[0].placed().len(), 3);
        // The backtest: the second record the test set's.
        let b = backtest(crate::agent::seam::for_tests(), &records, &es, 0, 30_000, 15_000);
        assert_eq!((b.train[0].forecasts, b.test[1].forecasts), (1, 1));
        assert!(backtest_text(&b).contains("Test (from 1970-01-01 04:10 UTC)"), "{}", backtest_text(&b));
    }

    /// Task #951 review: a record's lanes held to the day before's (#563, gone) get back each
    /// Mac's slots in code 5's replay: a member its cores' `job_slots`, the lead its ask's slots
    /// free and its lanes running; the lead alone (asking one) and a Mac with no lane, as they are.
    #[test]
    fn code_5_replays_each_mac_on_its_slots() {
        use super::super::forecast::{Input, Machine};
        use crate::coord::sched::{Asked, State};
        use crate::coord::{Ask, Capacity};
        let lane = |name: &str, mac: &str, busy_s: f64| Machine {
            name: name.into(),
            mac: Some(mac.into()),
            busy_s,
            ..Default::default()
        };
        let asked = |worker: &str, want: usize, cores: u32| Asked {
            ask: Ask {
                worker: worker.into(),
                want,
                cap: Some(Capacity { cores, ..Default::default() }),
                ..Default::default()
            },
            ..Default::default()
        };
        let input = |machines: Vec<Machine>, lead_want: usize| Input {
            now: 0,
            before: Vec::new(),
            drops: Vec::new(),
            regions: std::borrow::Cow::Owned(Vec::new()),
            costs: Default::default(),
            cadence: Default::default(),
            blind: None,
            chains: Vec::new(),
            after: Vec::new(),
            idle: Vec::new(),
            machines,
            running: BTreeMap::new(),
            macs: BTreeMap::new(),
            sched: Some(State {
                lead_name: "lead".into(),
                workers: vec![asked("lead", lead_want, 12), asked("m1", 7, 6), asked("off", 7, 6)],
                ..Default::default()
            }),
            pace: None,
            step_pace: Default::default(),
            unknown: Vec::new(),
            paused: false,
            along: Default::default(),
            code: crate::agent::forecast::VERSION,
        };
        let mut inp = input(vec![lane("lead", "lead", 300.0), lane("m1", "m1", 0.0)], 3);
        unhold(&mut inp);
        let names: Vec<&str> = inp.machines.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["lead", "m1", "lead #2", "lead #3", "lead #4", "m1 #2", "m1 #3", "m1 #4"]);
        assert!(inp.machines[2..].iter().all(|m| m.busy_s == 0.0 && m.running.is_none()));
        // Again: as they are.
        unhold(&mut inp);
        assert_eq!(inp.machines.len(), 8);
        // The lead alone, asking one: its slots unknown, its lanes kept.
        let mut alone = input(vec![lane("lead", "lead", 0.0)], 1);
        unhold(&mut alone);
        assert_eq!(alone.machines.len(), 1);
    }

    #[test]
    fn the_history_read_twice_is_kept_once() {
        let e = ev(1000, "done", "m", Some(1), "unit", &["a"], None);
        let d = dedup(vec![e.clone(), ev(900, "lease", "m", Some(1), "unit", &["a"], None), e]);
        assert_eq!(d.iter().map(|e| e.t).collect::<Vec<_>>(), [900, 1000]);
    }

    #[test]
    fn times_parse() {
        let now = 1_792_800_000;
        assert_eq!(parse_time("12h", now), Some(now - 43_200));
        assert_eq!(parse_time("2026-10-23", now), Some(1_792_713_600));
        assert_eq!(parse_time("2026-10-23T01:30", now), Some(1_792_713_600 + 5400));
        assert_eq!(parse_time("2026-10-23 01:30", now), Some(1_792_713_600 + 5400));
        assert_eq!(parse_time("17", now), Some(17));
        assert_eq!(parse_time("soon", now), None);
    }
}
