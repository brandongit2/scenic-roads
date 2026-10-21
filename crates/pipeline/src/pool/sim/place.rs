//! The placement simulator (docs/pool.md §13, Placement): N Macs, each with its memory, cores and
//! disk, working one plan through the lead's real coordinator (crate::coord), its measures read by
//! crate::needs as every placement path reads them, on a clock of the simulator's.
//!
//! - **The members** ask the coordinator over HTTP, as their agents do (`client::Client::ask`), with
//!   what their Mac spares (its memory guard's limit less what its jobs hold, its disk's free space
//!   now (past its room target) less its jobs' reservations, its capacity); what they're granted is
//!   `pack`'s, untouched. Each runs `Cfg::member_slots` jobs (0: the agent's, by its cores), asking
//!   while a slot is free for as many jobs as it has slots free, running every one granted; a job
//!   ends by giving its lease back, its costs (with its threads) and disk measured into the
//!   coordinator first.
//! - **The lead** asks its own coordinator in process as its agent does (`Coordinator::ask_own`,
//!   the agent's `lead_grants`): the plan offered in its order, the steps kept to it (not offered to
//!   every member) as its own planned jobs (`Offer::job`, a job a target), granted to it alone; its
//!   slots the agent's (`job_slots` of its cores); with no job running, for one job, its disk not
//!   bounded, else for its slots free with its disk room; each job run with the threads granted.
//! - **Each job** takes its step's true memory and cores from its start to its end, and its disk
//!   as it goes (written evenly over its run, as a real job fills its scratch): until it ends, the
//!   rest is its reservation (task #242), which its member's ask takes off its free space. A halo
//!   step's job (crate::needs::HALO_STEPS: `pack`, task #260; `lo`, task #381) takes one halo's targets (`Cfg::halo`
//!   a halo, offered a halo an offer as the agent offers them), its disk its halo's output once
//!   whatever its targets, its `bytes` its halo's base packs' files (not its NAS traffic), its NAS
//!   what it reads of them (`nas_read` over `nas_secs`) and what it writes. A job of a step reading a shared cache (crate::needs::CACHES: the DEM
//!   cache) on a Mac without it fills it (`Step::cache_mb`, measured with its targets and its
//!   job), the cache kept on that Mac's disk after (`Cfg::warm`: those every Mac has from the
//!   start), its capacity saying the caches it has (crate::coord::Capacity::caches); the
//!   coordinator reserves what a Mac lacks of a cache once (`Run::cache_twice` counts otherwise). A job
//!   that would pass its Mac's memory limit with the others there is stopped by the memory guard as
//!   it starts (its floor learned, its lease given back): allowed only of a cold start, the
//!   placement's one guess.
//!
//! Checked at every start, first where a job may go at all: no Mac past its slots; a halo step's job
//! within one halo; a step kept to the
//! lead (crate::agent::every::LEAD_ONLY, or not offered) only on the lead; one cold start of a step
//! in the pool at a time (beside other jobs by its provisional need, task #499). Then what it takes: past its Mac's memory limit only
//! a cold start, stopped by the guard; never past its disk (every job on it counted whole: no Mac's
//! disk over-committed, however little its jobs have written yet); its threads (those granted it,
//! crate::coord::Grant::threads, at least one) and those of the jobs beside it never past its Mac's
//! cores (less those a member's in-use share keeps, `Cfg::in_use`). And that the plan finishes.

use crate::agent::every;
use crate::agent::memguard::{self, Floor};
use crate::coord::{self, client, cost_key, cost_version, Ask, Capacity, Cost, Granted, Offer};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// A Mac of the simulation.
#[derive(Clone, Debug)]
pub struct Mac {
    pub name: &'static str,
    /// Its memory (MB): its jobs' limit is memguard::limit_mb of it.
    pub total_mb: u64,
    pub cores: u32,
    /// Its disk's free space with no job running (MB).
    pub disk_mb: u64,
}

/// A step of the plan: its targets and what each job of one target truly takes.
#[derive(Clone, Debug)]
pub struct Step {
    pub name: &'static str,
    pub targets: usize,
    pub mem_mb: u64,
    pub disk_mb: u64,
    pub secs: u64,
    pub threads: u32,
    /// What a job of one target reads and writes on the NAS (MB together, as crate::costin's `bytes`
    /// and `wrote`), and the files it makes there (`creates`).
    pub nas_mb: u64,
    pub creates: u64,
    /// What filling its shared cache (crate::needs::CACHES) on a Mac takes past its own disk (MB):
    /// a job of it on a Mac without the cache fills it, written with its own and kept there after;
    /// the jobs after it there read it.
    pub cache_mb: u64,
}

#[derive(Clone, Debug)]
pub struct Cfg {
    /// The Macs, the lead first.
    pub macs: Vec<Mac>,
    pub steps: Vec<Step>,
    /// The jobs a member runs at once; 0: as the agent has them, by its Mac's cores
    /// (crate::agent::job_slots).
    pub member_slots: usize,
    /// The NAS budget on (crate::budget::nas), with the NAS's limits as a probe would measure them.
    /// None: off.
    pub nas: Option<crate::budget::nas::Limits>,
    /// The members' owners using them, each member's in-use share keeping these cores free of its
    /// jobs (crate::agent::share). None: not in use.
    pub in_use: Option<u32>,
    /// A halo step's targets a halo (crate::needs::HALO_STEPS: a unit's map tiles, task #260). With
    /// a `unit` step in the plan, halo h is stale (offered) once unit target h is built, as a unit's
    /// map tiles are once it is.
    pub halo: usize,
    /// The steps measured before the run (a lead with history): one target of each, and one job
    /// of each that filled its shared cache (its disk and its cache's: `Step::cache_mb`).
    pub known: Vec<&'static str>,
    /// The shared caches every Mac has whole already (on its disk, counted out of its free space).
    pub warm: Vec<&'static str>,
    /// The parts a cache's fill comes in: a job copies one, the rest left for the jobs after it; 0
    /// or 1: the whole.
    pub fill_parts: u64,
    /// A Mac's cache emptied at a time, the pass the same (its index, simulated s, the cache), as
    /// room-making, the OSM pass's clear or a hand would.
    pub clear: Option<(usize, u64, &'static str)>,
}

/// What a run did.
#[derive(Debug, Default)]
pub struct Run {
    /// The most jobs each Mac ran at once.
    pub most: Vec<usize>,
    /// Jobs each Mac ran to the end.
    pub ran: Vec<usize>,
    /// Cold starts the memory guard stopped.
    pub guard_stops: usize,
    /// Simulated seconds to the plan's end.
    pub secs: u64,
    /// The most NAS weight the pool's jobs held at once (thousandths of the NAS, by `Cfg::nas`'s
    /// limits, or by a probe's of 100 MB/s and 40 creates/s with the budget off).
    pub most_nas: u32,
    /// Members' asks answered with nothing while a measured target of theirs was open (the NAS
    /// budget queuing them, among other reasons).
    pub turned_away: usize,
    /// The threads the members' jobs were granted, by step (the fewest and the most).
    pub granted: BTreeMap<String, (u32, u32)>,
    /// The steps the lead ran a job of, and the members.
    pub lead_steps: BTreeSet<String>,
    pub member_steps: BTreeSet<String>,
    /// The steps that ran beside each other on a Mac (its name, the two steps in order).
    pub beside: BTreeSet<(&'static str, String, String)>,
    /// The most disk a job of each step was granted with (MB, crate::coord::Grant::disk_mb).
    pub granted_disk: BTreeMap<String, u64>,
    /// The most jobs of each step a Mac ran at once (its name, the step).
    pub most_of: BTreeMap<(&'static str, String), usize>,
    /// Jobs granted their shared cache's reservation (more than their own disk) on a Mac where a
    /// job reading that cache ran already: the cache reserved twice.
    pub cache_twice: usize,
    /// Jobs granted on a Mac where none reading their cache ran, short of what it truly lacked of it.
    pub rest_unreserved: usize,
    /// Jobs granted a cache's reservation (more than their own disk), by Mac.
    pub fills_granted: BTreeMap<&'static str, usize>,
}

/// A loop of an agent (s).
const TICK: u64 = 20;
/// At most this long a run goes on (simulated s).
const MAX_S: u64 = 7 * 86_400;

struct Job {
    step: String,
    targets: Vec<String>,
    lease: u64,
    start: u64,
    end: u64,
    mem_mb: u64,
    disk_mb: u64,
    /// What of `disk_mb` fills its shared cache on its Mac (MB; 0: its Mac has it, or another job
    /// there fills it).
    fill: u64,
    threads: u32,
    /// Its NAS weight (its step's true rate by the limits).
    nas: u32,
    /// A cold start: none of its step measured when granted (at most one on a Mac).
    cold: bool,
}

fn key_of(t: &str) -> String {
    format!("k {t}")
}

fn target_name(step: &str, i: usize) -> String {
    format!("6/{}/{i}", step.len())
}

/// The halo of a halo step's target (`target_name`'s index over `Cfg::halo`).
fn halo_of(t: &str, halo: usize) -> usize {
    t.rsplit('/').next().and_then(|i| i.parse::<usize>().ok()).unwrap_or(0) / halo.max(1)
}

fn is_halo(step: &str) -> bool {
    crate::needs::HALO_STEPS.contains(&step)
}

/// What job `j` has written on its Mac's disk by `t` (MB): its disk spread evenly over its run.
fn written(j: &Job, t: u64) -> u64 {
    let run = j.end.saturating_sub(j.start).max(1);
    j.disk_mb * t.saturating_sub(j.start).min(run) / run
}

/// What a run of one target of `s` given `threads` measures, as crate::costin records it: its
/// memory and time, the threads it was given and the CPU time it took (its step's threads busy, at
/// most those given), and what it read and wrote on the NAS, half of `nas_mb` each (a halo step's
/// reads its `nas_read` over `nas_secs`, its run; its `bytes` its halo's base packs' files, as
/// crate::needs reads them).
fn measures(s: &Step, threads: u32) -> Cost {
    let nas = s.nas_mb << 20;
    let (read, wrote) = (nas / 2, nas - nas / 2);
    let (bytes, nas_read) = if is_halo(s.name) { (s.disk_mb << 20, read) } else { (read, 0) };
    Cost {
        peak_mb: s.mem_mb,
        secs: s.secs,
        worker: None,
        v: cost_version(s.name),
        sizes: [
            ("threads".to_string(), threads as u64),
            ("cpu_ms".to_string(), threads.min(s.threads) as u64 * s.secs * 1000),
            ("bytes".to_string(), bytes),
            ("nas_read".to_string(), nas_read),
            ("nas_secs".to_string(), s.secs),
            ("wrote".to_string(), wrote),
            ("creates".to_string(), s.creates),
        ]
        .into(),
        ..Default::default()
    }
}

/// Runs `cfg` to the plan's end; an error names the first invariant broken.
pub fn run(cfg: &Cfg) -> Result<Run, String> {
    run_with(cfg, true)
}

/// `run`, the members reserving their jobs' disk (`reserve`, as the agent does) or not (asking
/// with their free space now alone, as before task #242: a job's disk not written yet is counted
/// free).
fn run_with(cfg: &Cfg, reserve: bool) -> Result<Run, String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let lead_name = cfg.macs[0].name;
    let (c, port) = coord::start_for_test(&d.path().join("coord"), lead_name, "");
    let clock = store::clock::Virtual::new();
    c.shared.lock().unwrap().clock = clock.clone();
    c.set_nas(cfg.nas);
    // (The steps measured before (`Cfg::known`): one target of each, as a run of it left them.)
    for s in cfg.steps.iter().filter(|s| cfg.known.contains(&s.name)) {
        let tg = target_name(s.name, 0);
        c.add_costs(&[(cost_key(s.name, &tg), measures(s, s.threads))]);
        c.add_disks(&[(cost_key(s.name, &tg), s.disk_mb)]);
        c.add_job_run(s.name, std::slice::from_ref(&tg), s.disk_mb + s.cache_mb);
    }
    let limits = cfg.nas.unwrap_or(crate::budget::nas::Limits {
        bytes_s: 100e6,
        creates_s: 40.0,
        downloads: None,
        ..Default::default()
    });
    let weight = |s: &Step| {
        limits.weight(crate::budget::nas::Rate {
            bytes_s: (s.nas_mb << 20) as f64 / s.secs.max(1) as f64,
            creates_s: s.creates as f64 / s.secs.max(1) as f64,
            ..Default::default()
        })
    };
    let clients: Vec<Option<client::Client>> = cfg
        .macs
        .iter()
        .enumerate()
        .map(|(k, m)| {
            (k > 0)
                .then(|| client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), m.name))
        })
        .collect();
    let steps: BTreeMap<&str, &Step> = cfg.steps.iter().map(|s| (s.name, s)).collect();
    let slots = |k: usize| {
        if k == 0 || cfg.member_slots == 0 {
            // (As the agent has them: by its Mac's cores, crate::agent::job_slots.)
            crate::agent::job_slots(cfg.macs[k].cores as usize)
        } else {
            cfg.member_slots
        }
    };
    let offered_steps: Vec<String> =
        cfg.steps.iter().filter(|s| every::offered(s.name)).map(|s| s.name.to_string()).collect();
    let mut done: BTreeSet<(String, String)> = BTreeSet::new();
    let total: usize = cfg.steps.iter().map(|s| s.targets).sum();
    let mut running: Vec<Vec<Job>> = cfg.macs.iter().map(|_| Vec::new()).collect();
    // (What each Mac holds of each shared cache (MB): its whole fill where warm.)
    let whole = |c: &str| {
        cfg.steps.iter().filter(|s| crate::needs::cache_of(s.name) == Some(c)).map(|s| s.cache_mb).max().unwrap_or(0)
    };
    let mut have: Vec<BTreeMap<&'static str, u64>> =
        cfg.macs.iter().map(|_| cfg.warm.iter().map(|c| (*c, whole(c))).collect()).collect();
    let mut cleared = false;
    let mut out = Run { most: vec![0; cfg.macs.len()], ran: vec![0; cfg.macs.len()], ..Default::default() };
    let limit = |k: usize| memguard::limit_mb(cfg.macs[k].total_mb);
    let mut t = 0u64;
    while done.len() < total {
        if t > MAX_S {
            return Err(format!(
                "the plan didn't finish in {MAX_S} s: {} of {total} done, running {:?}",
                done.len(),
                running
                    .iter()
                    .map(|r| r.iter().map(|j| format!("{} {:?}", j.step, j.targets)).collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            ));
        }
        // Jobs ending: their measures in, then their leases ended (the lead's own finished, a
        // member's given back once its hand-off is merged: the plan no longer has its targets).
        for k in 0..cfg.macs.len() {
            let (ended, left): (Vec<Job>, Vec<Job>) =
                std::mem::take(&mut running[k]).into_iter().partition(|j| j.end <= t);
            running[k] = left;
            for j in ended {
                let s = steps[j.step.as_str()];
                let costs: Vec<(String, Cost)> =
                    j.targets.iter().map(|tg| (cost_key(&j.step, tg), measures(s, j.threads))).collect();
                c.add_costs_by(&costs, cfg.macs[k].name);
                // (A cold cache's fill measured with each target, as the accessors count it; kept on
                // the Mac after.)
                c.add_disks(
                    &j.targets.iter().map(|tg| (cost_key(&j.step, tg), s.disk_mb + j.fill)).collect::<Vec<_>>(),
                );
                c.add_job_run(&j.step, &j.targets, j.disk_mb);
                if let Some(cache) = crate::needs::cache_of(&j.step).filter(|_| j.fill > 0) {
                    *have[k].entry(cache).or_default() += j.fill;
                }
                done.extend(j.targets.iter().map(|tg| (j.step.clone(), tg.clone())));
                match &clients[k] {
                    None => c.finish(j.lease, &j.targets.iter().map(|tg| (tg.clone(), key_of(tg))).collect::<Vec<_>>()),
                    Some(w) => w.give_back(j.lease, "done").map_err(|e| format!("give back: {e:#}"))?,
                }
                out.ran[k] += 1;
            }
        }
        // The plan's work left, offered in its order: an offered step's targets, the lead's own steps
        // a job a target (its planned jobs, granted to it alone), as the agent's `offer_plan` offers
        // them.
        let units = cfg.steps.iter().any(|s| s.name == "unit");
        let open = |s: &Step| {
            (0..s.targets)
                .map(|i| target_name(s.name, i))
                .filter(|tg| !done.contains(&(s.name.to_string(), tg.clone())))
                // (A halo's map tiles stale once its unit is built.)
                .filter(|tg| {
                    !(units && is_halo(s.name))
                        || done.contains(&("unit".to_string(), target_name("unit", halo_of(tg, cfg.halo))))
                })
                .collect::<Vec<_>>()
        };
        c.offer(
            "p",
            cfg.steps
                .iter()
                .flat_map(|s| {
                    let targets = open(s).into_iter().map(|tg| (tg.clone(), key_of(&tg), 0));
                    if every::offered(s.name) && is_halo(s.name) {
                        // (A halo an offer, as the agent offers them: build::by_unit's groups.)
                        let mut by: BTreeMap<usize, Vec<(String, String, u64)>> = BTreeMap::new();
                        for t in targets {
                            by.entry(halo_of(&t.0, cfg.halo)).or_default().push(t);
                        }
                        by.into_values().map(|targets| Offer { step: s.name.into(), targets, job: None }).collect()
                    } else if every::offered(s.name) {
                        vec![Offer { step: s.name.into(), targets: targets.collect(), job: None }]
                    } else {
                        targets
                            .map(|t| Offer {
                                step: s.name.into(),
                                job: Some(format!("{} {}", s.name, t.0)),
                                targets: vec![t],
                            })
                            .collect()
                    }
                })
                .collect(),
        );
        let used = |r: &[Job]| {
            (
                r.iter().map(|j| j.mem_mb).sum::<u64>(),
                r.iter().map(|j| j.disk_mb).sum::<u64>(),
                r.iter().map(|j| j.threads).sum::<u32>(),
            )
        };
        let measured = |step: &str, ts: &[String]| c.measured_batch(step, ts);
        // (A cache emptied, `Cfg::clear`.)
        if let Some((k, at, c)) = cfg.clear.filter(|x| !cleared && t >= x.1) {
            let _ = at;
            have[k].remove(c);
            cleared = true;
        }
        // (A Mac's disk free with no job running: less the caches it holds.)
        let disk_of = |k: usize| cfg.macs[k].disk_mb.saturating_sub(have[k].values().sum());
        // (What a Mac holds of its caches, as its capacity says it: read off its disk.)
        let here = |k: usize| -> BTreeMap<String, coord::CacheHere> {
            crate::needs::CACHES
                .iter()
                .map(|(c, _)| (c.to_string(), coord::CacheHere { have_mb: have[k].get(c).copied().unwrap_or(0) }))
                .collect()
        };
        // (The lead's own targets the coordinator has held back for the NAS budget alone for
        // `OWN_AGE` or more, as they stood just before its ask: coord's own_nas_fits.)
        let aged: std::cell::RefCell<BTreeSet<(String, String)>> = Default::default();
        // A job starting on Mac `k`: the guard's stop of one past the limit, else the invariants.
        let start = |k: usize,
                     step: &str,
                     targets: Vec<String>,
                     lease: u64,
                     threads: u32,
                     granted_mb: u64,
                     running: &mut Vec<Vec<Job>>,
                     out: &mut Run|
         -> Result<bool, String> {
            let s = steps[step];
            let own = measured(step, &targets);
            let cold = own.is_none();
            // (Its shared cache: what its Mac lacks of it and no job there copies already, a part of
            // the fill at most, `Cfg::fill_parts`.)
            let cache = crate::needs::cache_of(step);
            let fill = match cache {
                Some(ch) => {
                    let copying: u64 =
                        running[k].iter().filter(|j| crate::needs::cache_of(&j.step) == Some(ch)).map(|j| j.fill).sum();
                    let lacks = s.cache_mb.saturating_sub(have[k].get(ch).copied().unwrap_or(0) + copying);
                    lacks.min(s.cache_mb.div_ceil(cfg.fill_parts.max(1)))
                }
                None => 0,
            };
            if let (Some(ch), Some(m)) = (cache, own) {
                let beside = running[k].iter().any(|j| crate::needs::cache_of(&j.step) == Some(ch));
                if granted_mb > m.disk_mb {
                    *out.fills_granted.entry(cfg.macs[k].name).or_default() += 1;
                    if beside {
                        out.cache_twice += 1;
                    }
                }
                let lacks = s.cache_mb.saturating_sub(have[k].get(ch).copied().unwrap_or(0));
                if !beside && granted_mb.saturating_sub(m.disk_mb) < lacks {
                    out.rest_unreserved += 1;
                }
            }
            let job = Job {
                step: step.into(),
                targets: targets.clone(),
                lease,
                start: t,
                end: t + s.secs * targets.len() as u64,
                mem_mb: s.mem_mb,
                // (A halo step's: its halo's cache once.)
                disk_mb: fill + if is_halo(step) { s.disk_mb } else { s.disk_mb * targets.len() as u64 },
                fill,
                threads,
                nas: weight(s),
                cold,
            };
            // (Where it may go at all, before whether it fits: a guard's stop excuses none of these.)
            if running[k].len() + 1 > slots(k) {
                return Err(format!("{} past its {} slots", cfg.macs[k].name, slots(k)));
            }
            if is_halo(step) && targets.iter().any(|t| halo_of(t, cfg.halo) != halo_of(&targets[0], cfg.halo)) {
                return Err(format!("{} placed {step} {targets:?}, past one halo", cfg.macs[k].name));
            }
            if k > 0 && (!every::offered(step) || every::LEAD_ONLY.contains(&step)) {
                return Err(format!("{} (a member) placed {step}, kept to the lead", cfg.macs[k].name));
            }
            if cold && running[k].iter().any(|j| j.cold) {
                return Err(format!("{} placed a cold start of {step} beside another cold start", cfg.macs[k].name));
            }
            if cold && running.iter().flatten().any(|j| j.step == step) {
                return Err(format!(
                    "{} placed a cold start of {step} while another of its jobs runs",
                    cfg.macs[k].name
                ));
            }
            let (mem, disk, threads) = used(&running[k]);
            if mem + job.mem_mb > limit(k) {
                if !cold {
                    return Err(format!(
                        "{} placed {step} {targets:?} (measured) past its memory limit: {} + {} > {} MB",
                        cfg.macs[k].name,
                        mem,
                        job.mem_mb,
                        limit(k)
                    ));
                }
                // (The memory guard stops it: its floor learned alone, its lease back.)
                out.guard_stops += 1;
                let floors: Vec<(String, Floor)> = targets
                    .iter()
                    .map(|tg| {
                        (
                            cost_key(step, tg),
                            Floor { mb: job.mem_mb, alone: running[k].is_empty(), v: cost_version(step) },
                        )
                    })
                    .collect();
                match &clients[k] {
                    None => {
                        c.add_floors(&floors);
                        c.finish(lease, &[]);
                    }
                    Some(w) => w
                        .end_with(lease, "the memory guard", true, &floors, &[], 0)
                        .map_err(|e| format!("end: {e:#}"))?,
                }
                return Ok(false);
            }
            if disk + job.disk_mb > disk_of(k) {
                return Err(format!(
                    "{} placed {step} {targets:?} past its disk (over-committed): {} + {} > {} MB",
                    cfg.macs[k].name,
                    disk,
                    job.disk_mb,
                    disk_of(k)
                ));
            }
            // (The threads granted it, within the cores a member's in-use share leaves.)
            let kept = if k > 0 { cfg.in_use.unwrap_or(0) } else { 0 };
            if job.threads == 0 || threads + job.threads + kept > cfg.macs[k].cores {
                return Err(format!(
                    "{} placed {step} past its cores: {threads} + {} threads + {kept} kept > {}",
                    cfg.macs[k].name, job.threads, cfg.macs[k].cores
                ));
            }
            // (The NAS budget on: a measured job only within the pool's room left; a member's also
            // with no other member's job weighing anything (the lead's own never hold it back), the
            // lead's own with nothing weighing anything.)
            // (Every job at its true weight, a cold start's too: the coordinator weighs a cold NAS
            // step's at the whole, task #499 review.)
            let w = |j: &Job| j.nas;
            let pool_nas: u32 = running.iter().flatten().map(w).sum();
            let members_nas: u32 = running.iter().skip(1).flatten().map(w).sum();
            let heaviest = running.iter().flatten().map(w).max().unwrap_or(0);
            let fits = if k == 0 {
                // (A planned job of the lead's own may go beside jobs none of which weighs the whole,
                // within the whole with the lead's own: coord's own_nas_fits, its wait as the
                // coordinator timed it.)
                let own_nas: u32 = running[0].iter().map(w).sum();
                let waited = targets.iter().any(|t| aged.borrow().contains(&(step.to_string(), t.clone())));
                crate::budget::nas::Limits::fits_own_job(pool_nas, own_nas, heaviest, job.nas, waited)
            } else {
                crate::budget::nas::Limits::fits(members_nas, pool_nas, job.nas)
            };
            if cfg.nas.is_some() && !cold && !fits {
                return Err(format!(
                    "{} placed {step} {targets:?} past the NAS budget: {pool_nas} + {} > {}",
                    cfg.macs[k].name,
                    job.nas,
                    crate::budget::nas::WHOLE
                ));
            }
            for j in &running[k] {
                let (a, b) = if j.step <= job.step { (&j.step, &job.step) } else { (&job.step, &j.step) };
                out.beside.insert((cfg.macs[k].name, a.clone(), b.clone()));
            }
            running[k].push(job);
            let n = running[k].iter().filter(|j| j.step == step).count();
            let e = out.most_of.entry((cfg.macs[k].name, step.to_string())).or_default();
            *e = (*e).max(n);
            out.most_nas = out.most_nas.max(running.iter().flatten().map(|j| j.nas).sum());
            out.most[k] = out.most[k].max(running[k].len());
            Ok(true)
        };

        // The lead: each slot free asks its own coordinator, as its agent does (`lead_grants`).
        while running[0].len() < slots(0) {
            let (mem, disk, threads) = used(&running[0]);
            let m = &cfg.macs[0];
            let alone = running[0].is_empty();
            let wrote: u64 = running[0].iter().map(|j| written(j, t)).sum();
            let reserved_mb = disk - wrote;
            let cap = Capacity {
                limit_mb: limit(0),
                held_mb: mem,
                mem_mb: limit(0).saturating_sub(mem),
                cores: m.cores,
                cores_free: m.cores - threads,
                disk_mb: disk_of(0).saturating_sub(wrote + reserved_mb),
                reserved_mb,
                ac: true,
                nas: true,
                home: true,
                guard: true,
                leases: running[0].iter().map(|j| j.lease).collect(),
                caches: here(0),
                ..Default::default()
            };
            let a = Ask {
                kind: "native".into(),
                can: cfg.steps.iter().map(|s| s.name.to_string()).collect(),
                mem_mb: limit(0),
                cores: m.cores,
                limit_mb: Some(limit(0)),
                // (As the agent asks: alone, for one job, its disk not bounded; else its disk room.)
                disk_mb: (!alone).then_some(cap.disk_mb),
                cap: Some(cap),
                want: if alone { 1 } else { crate::agent::want(slots(0), running[0].len()) },
                ..Default::default()
            };
            *aged.borrow_mut() = {
                let s = c.shared.lock().unwrap();
                let now = s.now();
                let g = s.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
                g.iter()
                    .filter(|(_, (since, _))| now.duration_since(*since) >= crate::budget::nas::OWN_AGE)
                    .map(|(k, _)| k.clone())
                    .collect()
            };
            let Some(mut g) = c.ask_own(&a, &BTreeSet::new()) else { break };
            let more = std::mem::take(&mut g.more);
            let mut all = true;
            for g in std::iter::once(g).chain(more) {
                let Granted::Job { step, targets, .. } = g.work else {
                    return Err("a task granted to the lead".into());
                };
                if g.job.is_some() == every::offered(&step) {
                    return Err(format!("the lead granted {step} as {:?}", g.job));
                }
                out.lead_steps.insert(step.clone());
                let e = out.granted_disk.entry(step.clone()).or_default();
                *e = (*e).max(g.disk_mb);
                let ts = targets.into_iter().map(|t| t.0).collect();
                all &= start(0, &step, ts, g.lease, g.threads, g.disk_mb, &mut running, &mut out)?;
            }
            if !all {
                break;
            }
        }

        // The members: each slot free asks.
        for k in 1..cfg.macs.len() {
            let w = clients[k].as_ref().unwrap();
            for j in &running[k] {
                if !w.beat(j.lease, None).map_err(|e| format!("beat: {e:#}"))? {
                    return Err(format!("{}'s lease {} lapsed", cfg.macs[k].name, j.lease));
                }
            }
            while running[k].len() < slots(k) {
                let (mem, disk, threads) = used(&running[k]);
                let m = &cfg.macs[k];
                let mem_mb = limit(k).saturating_sub(mem);
                // (As the agent says it: the free space now (its jobs' disk written so far gone),
                // less what its jobs have still to write, their reservations.)
                let wrote: u64 = running[k].iter().map(|j| written(j, t)).sum();
                let reserved_mb = if reserve { disk - wrote } else { 0 };
                let disk_mb = disk_of(k).saturating_sub(wrote + reserved_mb);
                let cap = Capacity {
                    limit_mb: limit(k),
                    held_mb: mem,
                    mem_mb,
                    cores: m.cores,
                    cores_free: m.cores - threads,
                    disk_mb,
                    reserved_mb,
                    in_use: cfg.in_use.is_some(),
                    kept_cores: cfg.in_use.unwrap_or(0),
                    ac: true,
                    nas: true,
                    home: true,
                    guard: true,
                    leases: running[k].iter().map(|j| j.lease).collect(),
                    caches: here(k),
                    ..Default::default()
                };
                let a = Ask {
                    worker: m.name.into(),
                    kind: "native".into(),
                    can: offered_steps.clone(),
                    // (As the agent asks: the build's memory on its Mac (`Agent::build_room`), all its
                    // cores, its free disk now; what its jobs leave free in `cap`.)
                    mem_mb: limit(k),
                    cores: m.cores,
                    limit_mb: Some(limit(k)),
                    disk_mb: Some(disk_mb),
                    cap: Some(cap),
                    // (As the agent asks: its slots free.)
                    want: crate::agent::want(slots(k), running[k].len()),
                    ..Default::default()
                };
                let Some(mut g) = w.ask(&a).map_err(|e| format!("ask: {e:#}"))? else {
                    // (Turned away with a measured target it could take open.)
                    let open_measured = cfg.steps.iter().filter(|s| every::offered(s.name)).any(|s| {
                        let held = c.held(s.name);
                        open(s)
                            .iter()
                            .any(|tg| !held.contains(tg) && measured(s.name, std::slice::from_ref(tg)).is_some())
                    });
                    out.turned_away += usize::from(open_measured);
                    break;
                };
                let more = std::mem::take(&mut g.more);
                let mut all = true;
                for g in std::iter::once(g).chain(more) {
                    let Granted::Job { step, targets, .. } = g.work else {
                        return Err("a task granted".into());
                    };
                    // (Run with the threads granted it, as the agent runs it; a measured job's noted:
                    // a cold start's may be the whole Mac, task #499 review.)
                    let ts: Vec<String> = targets.into_iter().map(|t| t.0).collect();
                    if measured(&step, &ts).is_some() {
                        let e = out.granted.entry(step.clone()).or_insert((g.threads, g.threads));
                        *e = (e.0.min(g.threads), e.1.max(g.threads));
                    }
                    out.member_steps.insert(step.clone());
                    let e = out.granted_disk.entry(step.clone()).or_default();
                    *e = (*e).max(g.disk_mb);
                    all &= start(k, &step, ts, g.lease, g.threads, g.disk_mb, &mut running, &mut out)?;
                }
                if !all {
                    break;
                }
            }
        }
        // (The lead's own leases kept alive.)
        for j in &running[0] {
            c.renew(j.lease, None);
        }
        clock.advance(Duration::from_secs(TICK));
        t += TICK;
    }
    out.secs = t;
    Ok(out)
}

/// The steps of a build's kinds: two kept to the lead, four offered, one too large for a 16 GB
/// Mac (terrain); `pack` the NAS-heavy one (about half the NAS a job, by a probe of 100 MB/s and 40
/// creates/s).
pub fn plan() -> Vec<Step> {
    vec![
        Step {
            cache_mb: 0,
            name: "bld-fetch",
            targets: 1,
            mem_mb: 800,
            disk_mb: 2_000,
            secs: 300,
            threads: 2,
            nas_mb: 0,
            creates: 0,
        },
        Step {
            cache_mb: 0,
            name: "terrain",
            targets: 6,
            mem_mb: 13_000,
            disk_mb: 3_000,
            secs: 600,
            threads: 4,
            nas_mb: 6000,
            creates: 50,
        },
        Step {
            cache_mb: 0,
            name: "slope",
            targets: 10,
            mem_mb: 3_000,
            disk_mb: 500,
            secs: 200,
            threads: 4,
            nas_mb: 2000,
            creates: 20,
        },
        Step {
            cache_mb: 0,
            name: "pack",
            targets: 24,
            mem_mb: 1_500,
            disk_mb: 200,
            secs: 120,
            threads: 4,
            nas_mb: 4800,
            creates: 300,
        },
        Step {
            cache_mb: 0,
            name: "lo",
            targets: 12,
            mem_mb: 1_000,
            disk_mb: 100,
            secs: 90,
            threads: 4,
            nas_mb: 500,
            creates: 30,
        },
        Step {
            cache_mb: 0,
            name: "catalog",
            targets: 1,
            mem_mb: 2_000,
            disk_mb: 100,
            secs: 60,
            threads: 2,
            nas_mb: 200,
            creates: 5,
        },
    ]
}

/// The M4 (the lead: 24 GB, 10 cores) and the M1 (16 GB, 8 cores).
pub fn m4_m1() -> Vec<Mac> {
    vec![
        Mac { name: "m4", total_mb: 24_576, cores: 10, disk_mb: 300_000 },
        Mac { name: "m1", total_mb: 16_384, cores: 8, disk_mb: 60_000 },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(macs: Vec<Mac>, member_slots: usize) -> Cfg {
        Cfg {
            macs,
            steps: plan(),
            member_slots,
            nas: None,
            in_use: None,
            halo: 6,
            known: Vec::new(),
            warm: Vec::new(),
            fill_parts: 1,
            clear: None,
        }
    }

    /// The lead's jobs by grant (task #244): it packs several at once by its measures like any
    /// member, its own steps (`bld-fetch`, `catalog`) on it alone; the member one at a time; nothing
    /// past a Mac's memory, cores or disk; one cold start of a step at a time; the plan done.
    #[test]
    fn the_lead_packs_its_jobs_by_grant_and_runs_its_own_steps() {
        let r = run(&cfg(m4_m1(), 1)).unwrap();
        assert!(r.most[0] >= 2 && r.most[1] == 1, "{r:?}");
        assert!(r.ran.iter().all(|&n| n > 0), "{r:?}");
        assert!(r.lead_steps.contains("bld-fetch") && r.lead_steps.contains("catalog"), "{r:?}");
        // The map tiles shared: the member takes `pack` jobs while the lead takes some too, and
        // neither takes the lead's own steps from it.
        assert!(r.lead_steps.contains("pack") && r.member_steps.contains("pack"), "{r:?}");
        assert!(!r.member_steps.contains("bld-fetch") && !r.member_steps.contains("catalog"), "{r:?}");
    }

    /// Four Macs (one with little disk) finish the same plan, no later than two (the lead packing
    /// several jobs, the plan's cold starts and lead-only steps bound it), the members sharing it.
    #[test]
    fn four_macs_finish() {
        let two = run(&cfg(m4_m1(), 1)).unwrap();
        let mut macs = m4_m1();
        macs.push(Mac { name: "mini", total_mb: 16_384, cores: 8, disk_mb: 40_000 });
        macs.push(Mac { name: "air", total_mb: 8_192, cores: 8, disk_mb: 35_000 });
        let r = run(&cfg(macs, 1)).unwrap();
        assert!(r.ran.iter().skip(1).filter(|&&n| n > 0).count() >= 2, "{r:?}");
        assert!(r.secs <= two.secs, "{} s with four, {} s with two", r.secs, two.secs);
    }

    /// Members with two slots: the M1 reaches two jobs at once and every invariant holds.
    #[test]
    fn members_with_two_slots_pack_two_jobs() {
        let r = run(&cfg(m4_m1(), 2)).unwrap();
        assert_eq!(r.most[1], 2, "{r:?}");
    }

    /// A 48 GB member with room for several holds several small jobs at once (its cores the bound:
    /// four 4-thread jobs on 16), every invariant holding, sooner than one a time.
    #[test]
    fn a_48_gb_member_holds_several_small_jobs_at_once() {
        let mut macs = m4_m1();
        macs[1] = Mac { name: "studio", total_mb: 49_152, cores: 16, disk_mb: 300_000 };
        let one = run(&cfg(macs.clone(), 1)).unwrap();
        let r = run(&cfg(macs, 8)).unwrap();
        assert!(r.most[1] >= 3, "{r:?}");
        assert!(r.most[1] <= 4, "past its cores: {r:?}");
        assert!(r.secs < one.secs, "{} s packed, {} s one a time", r.secs, one.secs);
    }

    /// The members' slots as the agent has them (task #240: by their cores, asking for those free):
    /// the M1 and a 48 GB member each hold several jobs at once, every invariant holding, the plan
    /// done sooner than one a time.
    #[test]
    fn members_with_the_agents_slots_pack_by_their_room() {
        let mut macs = m4_m1();
        macs.push(Mac { name: "studio", total_mb: 49_152, cores: 16, disk_mb: 300_000 });
        let one = run(&cfg(macs.clone(), 1)).unwrap();
        let r = run(&cfg(macs, 0)).unwrap();
        assert!(r.most[1] >= 2 && r.most[2] >= 3, "{r:?}");
        assert!(r.most[2] <= 4, "past its cores: {r:?}");
        assert!(r.secs < one.secs, "{} s packed, {} s one a time", r.secs, one.secs);
    }

    /// The NAS budget on (100 MB/s, 40 creates/s): a 48 GB member that packs `pack` jobs by its
    /// cores past twice the NAS without it is held to what the pool's NAS weights leave, the rest
    /// queued (asks answered with nothing while measured targets are open), every member start
    /// within the budget (checked at each), and the plan still finishes.
    #[test]
    fn the_nas_budget_queues_nas_heavy_jobs_past_its_cap() {
        let mut macs = m4_m1();
        macs[1] = Mac { name: "studio", total_mb: 49_152, cores: 16, disk_mb: 300_000 };
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        let off = run(&cfg(macs.clone(), 8)).unwrap();
        let on = run(&Cfg { nas: Some(limits), ..cfg(macs, 8) }).unwrap();
        assert!(off.most_nas > crate::budget::nas::WHOLE, "off: past the NAS: {off:?}");
        assert!(on.most_nas < off.most_nas, "on: less of the NAS at once: {on:?} against {off:?}");
        assert!(on.turned_away > off.turned_away, "on: asks queued: {on:?} against {off:?}");
    }

    /// Threads granted (task #241): a serial step (one thread busy) is measured to take 1–2 cores and
    /// granted those, its later jobs packing more beside each other; a parallel one at most the
    /// packed share (the M1's 8: 2), whatever its first, cold run was given; every Mac's threads
    /// never past its cores, checked at every start.
    #[test]
    fn jobs_are_granted_their_measured_threads_within_the_cores() {
        let mut steps = plan();
        steps.push(Step {
            cache_mb: 0,
            name: "pois",
            targets: 20,
            mem_mb: 500,
            disk_mb: 50,
            secs: 100,
            threads: 1,
            nas_mb: 100,
            creates: 5,
        });
        let r = run(&Cfg { steps, ..cfg(m4_m1(), 0) }).unwrap();
        let (lo, hi) = r.granted["pois"];
        assert!(lo >= 1 && hi <= 2, "{r:?}");
        assert!(r.granted.values().all(|&(lo, hi)| lo >= 1 && hi <= 2), "the M1's packed share: 2: {r:?}");
        assert!(r.most[1] >= 3, "serial jobs pack beside each other: {r:?}");
    }

    /// Disk reservations (task #242): an M1 with 8 GB past its room target, its jobs 5 GB each written
    /// as they go. Reserving, it's never granted a job its disk can't hold beside the others
    /// (checked at every start: every job on it counted whole) and the plan finishes; asking with
    /// its free space now alone, a second is granted beside the first before that one has written
    /// its disk, over-committing it.
    #[test]
    fn reservations_keep_a_tight_disk_from_being_over_committed() {
        let mut macs = m4_m1();
        macs[1].disk_mb = 8_000;
        let mut steps = plan();
        for s in steps.iter_mut().filter(|s| s.name == "slope") {
            (s.disk_mb, s.secs, s.targets) = (5_000, 1_200, 16);
        }
        let c = Cfg { steps, ..cfg(macs, 0) };
        let r = run(&c).unwrap();
        assert!(r.ran[1] > 0, "{r:?}");
        let e = run_with(&c, false).unwrap_err();
        assert!(e.contains("m1 placed") && e.contains("over-committed"), "{e}");
    }

    /// Map tiles under packing (task #260): each unit built makes its halo's tiles stale, offered
    /// before the units left (the plan's order) a halo an offer. On the M4 and the M1, with the NAS
    /// budget on and the steps measured before, a job of them takes one halo (checked at every
    /// start), granted its halo's disk once (task #396: the parts of the base packs it reads, no
    /// shared cache), its NAS what it writes; each Mac runs them beside units, every start within
    /// its memory, disk, cores and the NAS budget, and the plan finishes.
    #[test]
    fn map_tiles_pack_beside_units_on_both_macs() {
        let unit = Step {
            cache_mb: 0,
            name: "unit",
            targets: 24,
            mem_mb: 2_500,
            disk_mb: 3_000,
            secs: 900,
            threads: 4,
            nas_mb: 3_000,
            creates: 40,
        };
        let pack = Step {
            cache_mb: 0,
            name: "pack",
            targets: 12 * 20,
            mem_mb: 1_000,
            disk_mb: 1_300,
            secs: 60,
            threads: 2,
            nas_mb: 60,
            creates: 30,
        };
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        let c = Cfg {
            steps: vec![pack, unit],
            nas: Some(limits),
            halo: 20,
            known: vec!["unit", "pack"],
            ..cfg(m4_m1(), 0)
        };
        let r = run(&c).unwrap();
        for s in ["pack", "unit"] {
            assert!(r.lead_steps.contains(s) && r.member_steps.contains(s), "{s} on both: {r:?}");
        }
        for m in ["m4", "m1"] {
            assert!(r.beside.contains(&(m, "pack".to_string(), "unit".to_string())), "{m}: packs beside units: {r:?}");
        }
        assert_eq!(r.granted_disk["pack"], 1_300, "its halo's disk once: {r:?}");
        assert_eq!((r.cache_twice, r.rest_unreserved), (0, 0), "{r:?}");
        assert!(r.most_nas <= crate::budget::nas::WHOLE, "{r:?}");
    }

    /// The low zooms (task #381): a halo step too, a pass's z3 tiles one offer (one halo: the
    /// units owning them), a job of them granted its tiles' disk once, not their sum (task #396:
    /// the parts of the base packs it reads, no shared cache): two of them on the M4 at once; its
    /// NAS what it writes; with the NAS budget on, the plan finishes.
    #[test]
    fn lo_jobs_take_their_halos_disk_once() {
        let lo = Step {
            cache_mb: 0,
            name: "lo",
            targets: 16,
            mem_mb: 1_000,
            disk_mb: 500,
            secs: 100,
            threads: 2,
            nas_mb: 800,
            creates: 5,
        };
        let limits =
            crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        let c = Cfg { steps: vec![lo], nas: Some(limits), halo: 16, known: vec!["lo"], ..cfg(m4_m1(), 0) };
        let r = run(&c).unwrap();
        assert_eq!(r.granted_disk.get("lo"), Some(&500), "its halo's disk once: {r:?}");
        assert_eq!(r.most_of.get(&("m4", "lo".to_string())), Some(&2), "{r:?}");
        assert_eq!((r.cache_twice, r.rest_unreserved), (0, 0), "{r:?}");
        assert!(r.most_nas <= crate::budget::nas::WHOLE, "{r:?}");
    }

    /// Task #392, the M4 as measured on 18 Oct (48 GB, 14 cores, 85 GB free; its caches on its
    /// disk): units of 6.6 GB (the densest measured, coord::unit_peak)
    /// writing 6 GB each (their disks' 90th percentile, 5.9 GB), `pack` 0.3 GB and `lo` 0.5 GB
    /// each; the DEM cache's fill measured before, but there: none reserved.
    /// Where one job at a time ran before, bound by reservations of the most a job ever took,
    /// three or four units run at once within its memory and disk (checked at every start), the
    /// map tiles and low zooms beside them.
    #[test]
    fn the_m4_runs_several_units_at_once_by_todays_measures() {
        // (85 GB free with its DEM cache's 12.6 GB there.)
        let m4 = Mac { name: "m4", total_mb: 49_152, cores: 14, disk_mb: 85_000 + 12_600 };
        let unit = Step {
            cache_mb: 12_600,
            name: "unit",
            targets: 24,
            mem_mb: 6_600,
            disk_mb: 6_000,
            secs: 900,
            threads: 3,
            nas_mb: 3_000,
            creates: 40,
        };
        let pack = Step {
            cache_mb: 0,
            name: "pack",
            targets: 12 * 20,
            mem_mb: 1_000,
            disk_mb: 300,
            secs: 60,
            threads: 2,
            nas_mb: 60,
            creates: 30,
        };
        let lo = Step {
            cache_mb: 0,
            name: "lo",
            targets: 8,
            mem_mb: 1_000,
            disk_mb: 500,
            secs: 100,
            threads: 2,
            nas_mb: 80,
            creates: 5,
        };
        let c = Cfg {
            steps: vec![unit, pack, lo],
            halo: 20,
            known: vec!["unit", "pack", "lo"],
            warm: vec!["DEM"],
            ..cfg(vec![m4], 0)
        };
        let r = run(&c).unwrap();
        let units = r.most_of[&("m4", "unit".to_string())];
        assert!((3..=4).contains(&units), "units at once: {r:?}");
        assert!(r.beside.contains(&("m4", "lo".to_string(), "pack".to_string())), "{r:?}");
        assert_eq!((r.cache_twice, r.rest_unreserved), (0, 0), "{r:?}");
        assert_eq!(r.granted_disk["unit"], 6_000, "its output, its cache there: {r:?}");
    }

    /// Task #392 review: the M4 with its DEM cache gone (its fill free again) beside map tiles and
    /// low zooms. The cache's fill (measured before, a cold run's) is reserved whole with the first
    /// unit's job and never again beside it, so no start over-commits the disk (checked at each: no
    /// disk guard to thrash), the units still several at once, and the plan finishes.
    #[test]
    fn a_cold_fill_beside_units_doesnt_over_commit() {
        let m4 = Mac { name: "m4", total_mb: 49_152, cores: 14, disk_mb: 85_000 + 12_600 };
        let unit = Step {
            cache_mb: 12_600,
            name: "unit",
            targets: 24,
            mem_mb: 6_600,
            disk_mb: 6_000,
            secs: 900,
            threads: 3,
            nas_mb: 3_000,
            creates: 40,
        };
        let pack = Step {
            cache_mb: 0,
            name: "pack",
            targets: 12 * 20,
            mem_mb: 1_000,
            disk_mb: 300,
            secs: 60,
            threads: 2,
            nas_mb: 60,
            creates: 30,
        };
        let lo = Step {
            cache_mb: 0,
            name: "lo",
            targets: 8,
            mem_mb: 1_000,
            disk_mb: 500,
            secs: 100,
            threads: 2,
            nas_mb: 80,
            creates: 5,
        };
        let c = Cfg { steps: vec![unit, pack, lo], halo: 20, known: vec!["unit", "pack", "lo"], ..cfg(vec![m4], 0) };
        let r = run(&c).unwrap();
        assert!(r.granted_disk["unit"] >= 12_600 + 6_000, "the fill reserved: {r:?}");
        assert_eq!((r.cache_twice, r.rest_unreserved), (0, 0), "{r:?}");
        assert!(r.most_of[&("m4", "unit".to_string())] >= 3, "{r:?}");
    }

    /// Task #392 re-review: a member whose units each copy a part of the DEM cache, a quarter of a
    /// 40 GB fill: each job that's the first there reading it is still reserved what the Mac truly
    /// lacks (checked at each start), several such grants, no start over-committing its disk; and
    /// its cache emptied midway, the pass the same, the next job is reserved the fill again.
    #[test]
    fn a_cache_filled_in_parts_or_emptied_is_reserved_what_it_lacks() {
        let unit = Step {
            cache_mb: 40_000,
            name: "unit",
            targets: 200,
            mem_mb: 1_000,
            disk_mb: 300,
            secs: 120,
            threads: 2,
            nas_mb: 60,
            creates: 30,
        };
        let mut macs = m4_m1();
        // (The lead too small for a unit's memory: the member runs them.)
        (macs[0].total_mb, macs[1].disk_mb) = (1_024, 100_000);
        let base = Cfg { steps: vec![unit], known: vec!["unit"], fill_parts: 4, ..cfg(macs, 1) };
        let r = run(&base).unwrap();
        assert!(r.fills_granted.get("m1").is_some_and(|&n| n >= 2), "{r:?}");
        assert_eq!((r.cache_twice, r.rest_unreserved), (0, 0), "{r:?}");
        let r = run(&Cfg { fill_parts: 1, clear: Some((1, 1_200, "DEM")), ..base }).unwrap();
        assert!(r.fills_granted.get("m1").is_some_and(|&n| n >= 2), "filled, emptied, filled again: {r:?}");
        assert_eq!((r.cache_twice, r.rest_unreserved), (0, 0), "{r:?}");
    }

    /// The members' owners at them (task #241): each one's in-use share keeps its cores (here 4 of
    /// the M1's 8) free of its jobs, checked at every start, and the plan still finishes.
    #[test]
    fn the_in_use_share_keeps_its_cores_free() {
        let r = run(&Cfg { in_use: Some(4), ..cfg(m4_m1(), 0) }).unwrap();
        assert!(r.ran[1] > 0 && r.most[1] <= 2, "{r:?}");
    }
}
