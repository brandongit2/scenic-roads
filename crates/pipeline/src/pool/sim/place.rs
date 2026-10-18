//! The placement simulator (docs/pool.md §13, Placement): N Macs, each with its memory, cores and
//! disk, working one plan through the lead's real coordinator (crate::coord), its measures read by
//! crate::needs as every placement path reads them, on a clock of the simulator's.
//!
//! - **The members** ask the coordinator over HTTP, as their agents do (`client::Client::ask`), with
//!   what their Mac spares (its memory guard's limit less what its jobs hold, its disk less the
//!   reserve and its jobs', its capacity); what they're granted is `pick_every`'s, untouched. Each
//!   runs `Cfg::member_slots` jobs (today's agent: one), asking again while a slot is free; a job
//!   ends by giving its lease back, its costs and disk measured into the coordinator first.
//! - **The lead** runs `agent::SLOTS` jobs (two), its own leases held (`Coordinator::hold`): its plan
//!   leaves out what others hold and a step whose cold start runs elsewhere (`cold_under_way`), as
//!   the agent's does; the first slot takes the plan's first unless it waits for the second job, the
//!   second the plan's first that can run beside the first, by the agent's own rules (`clash`,
//!   `beside_why`), nothing beside a cold start or with one next. Its room is the coordinator's
//!   `Every::own`, as the agent's `own_room` says it.
//! - **Each job** takes its step's true memory, disk and cores from its start to its end. A job
//!   that would pass its Mac's memory limit with the others there is stopped by the memory guard as
//!   it starts (its floor learned, its lease given back): allowed only of a cold start, the
//!   placement's one guess.
//!
//! Checked at every start: no Mac past its memory limit, its cores or its disk; a cold start alone
//! on its Mac and in the pool; a step kept to the lead (crate::agent::every::LEAD_ONLY, or not
//! offered) only on the lead; no Mac past its slots. And that the plan finishes.

use crate::agent::every;
use crate::agent::memguard::{self, Floor};
use crate::agent::room;
use crate::coord::{
    self, client, cost_key, cost_version, Ask, Capacity, Cost, Every, Granted, Offer,
};
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
}

#[derive(Clone, Debug)]
pub struct Cfg {
    /// The Macs, the lead first.
    pub macs: Vec<Mac>,
    pub steps: Vec<Step>,
    /// The jobs a member runs at once (today's agent: one). More is for multi-lease asks (task #239).
    pub member_slots: usize,
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
}

/// A loop of an agent (s).
const TICK: u64 = 20;
/// At most this long a run goes on (simulated s).
const MAX_S: u64 = 7 * 86_400;

struct Job {
    step: String,
    targets: Vec<String>,
    lease: u64,
    end: u64,
    mem_mb: u64,
    disk_mb: u64,
    threads: u32,
    cold: bool,
}

fn key_of(t: &str) -> String {
    format!("k {t}")
}

fn target_name(step: &str, i: usize) -> String {
    format!("6/{}/{i}", step.len())
}

/// Runs `cfg` to the plan's end; an error names the first invariant broken.
pub fn run(cfg: &Cfg) -> Result<Run, String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let lead_name = cfg.macs[0].name;
    let (c, port) = coord::start_for_test(&d.path().join("coord"), lead_name, "");
    let clock = store::clock::Virtual::new();
    c.shared.lock().unwrap().clock = clock.clone();
    c.set_map_update(Some(coord::AsOf {
        began: 1,
        units: BTreeMap::new(),
    }));
    let clients: Vec<Option<client::Client>> = cfg
        .macs
        .iter()
        .enumerate()
        .map(|(k, m)| {
            (k > 0).then(|| {
                client::Client::at(
                    vec![format!("http://127.0.0.1:{port}")],
                    c.contact.token.clone(),
                    m.name,
                )
            })
        })
        .collect();
    let steps: BTreeMap<&str, &Step> = cfg.steps.iter().map(|s| (s.name, s)).collect();
    let slots = |k: usize| {
        if k == 0 {
            crate::agent::SLOTS
        } else {
            cfg.member_slots
        }
    };
    let offered_steps: Vec<String> = cfg
        .steps
        .iter()
        .filter(|s| every::offered(s.name))
        .map(|s| s.name.to_string())
        .collect();
    let mut done: BTreeSet<(String, String)> = BTreeSet::new();
    let total: usize = cfg.steps.iter().map(|s| s.targets).sum();
    let mut running: Vec<Vec<Job>> = cfg.macs.iter().map(|_| Vec::new()).collect();
    let mut out = Run {
        most: vec![0; cfg.macs.len()],
        ran: vec![0; cfg.macs.len()],
        ..Default::default()
    };
    let limit = |k: usize| memguard::limit_mb(cfg.macs[k].total_mb);
    let reserve_mb = room::RESERVE >> 20;
    let mut t = 0u64;
    while done.len() < total {
        if t > MAX_S {
            return Err(format!(
                "the plan didn't finish in {MAX_S} s: {} of {total} done, running {:?}",
                done.len(),
                running
                    .iter()
                    .map(|r| r
                        .iter()
                        .map(|j| format!("{} {:?}", j.step, j.targets))
                        .collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            ));
        }
        // Jobs ending: their measures in, then their leases ended (the lead's own finished, a
        // member's given back once its hand-off is merged: the plan no longer has its targets).
        for k in 0..cfg.macs.len() {
            let (ended, left): (Vec<Job>, Vec<Job>) = std::mem::take(&mut running[k])
                .into_iter()
                .partition(|j| j.end <= t);
            running[k] = left;
            for j in ended {
                let s = steps[j.step.as_str()];
                let costs: Vec<(String, Cost)> = j
                    .targets
                    .iter()
                    .map(|tg| {
                        (
                            cost_key(&j.step, tg),
                            Cost {
                                peak_mb: s.mem_mb,
                                secs: s.secs,
                                worker: None,
                                v: cost_version(&j.step),
                                ..Default::default()
                            },
                        )
                    })
                    .collect();
                c.add_costs_by(&costs, cfg.macs[k].name);
                c.add_disks(
                    &j.targets
                        .iter()
                        .map(|tg| (cost_key(&j.step, tg), s.disk_mb))
                        .collect::<Vec<_>>(),
                );
                done.extend(j.targets.iter().map(|tg| (j.step.clone(), tg.clone())));
                match &clients[k] {
                    None => c.finish(
                        j.lease,
                        &j.targets
                            .iter()
                            .map(|tg| (tg.clone(), key_of(tg)))
                            .collect::<Vec<_>>(),
                    ),
                    Some(w) => w
                        .give_back(j.lease, "done")
                        .map_err(|e| format!("give back: {e:#}"))?,
                }
                out.ran[k] += 1;
            }
        }
        // The plan's work left, offered (every step to every member: the lead's own kept back).
        let open = |s: &Step| {
            (0..s.targets)
                .map(|i| target_name(s.name, i))
                .filter(|tg| !done.contains(&(s.name.to_string(), tg.clone())))
                .collect::<Vec<_>>()
        };
        c.offer(
            "p",
            cfg.steps
                .iter()
                .filter(|s| every::offered(s.name))
                .map(|s| Offer {
                    step: s.name.into(),
                    targets: open(s)
                        .into_iter()
                        .map(|tg| (tg.clone(), key_of(&tg), 0))
                        .collect(),
                    batch: 1,
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
        // A job starting on Mac `k`: the guard's stop of one past the limit, else the invariants.
        let start = |k: usize,
                     step: &str,
                     targets: Vec<String>,
                     lease: u64,
                     running: &mut Vec<Vec<Job>>,
                     out: &mut Run|
         -> Result<bool, String> {
            let s = steps[step];
            let cold = measured(step, &targets).is_none();
            let job = Job {
                step: step.into(),
                targets: targets.clone(),
                lease,
                end: t + s.secs * targets.len() as u64,
                mem_mb: s.mem_mb,
                disk_mb: s.disk_mb * targets.len() as u64,
                threads: s.threads,
                cold,
            };
            let (mem, disk, threads) = used(&running[k]);
            if mem + job.mem_mb > limit(k) {
                if !cold {
                    return Err(format!("{} placed {step} {targets:?} (measured) past its memory limit: {} + {} > {} MB", cfg.macs[k].name, mem, job.mem_mb, limit(k)));
                }
                // (The memory guard stops it: its floor learned alone, its lease back.)
                out.guard_stops += 1;
                let floors: Vec<(String, Floor)> = targets
                    .iter()
                    .map(|tg| {
                        (
                            cost_key(step, tg),
                            Floor {
                                mb: job.mem_mb,
                                alone: running[k].is_empty(),
                                v: cost_version(step),
                            },
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
            if disk + job.disk_mb + reserve_mb > cfg.macs[k].disk_mb {
                return Err(format!("{} placed {step} {targets:?} past its disk: {} + {} + reserve {reserve_mb} > {} MB", cfg.macs[k].name, disk, job.disk_mb, cfg.macs[k].disk_mb));
            }
            if threads + job.threads > cfg.macs[k].cores {
                return Err(format!("{} placed {step} past its cores", cfg.macs[k].name));
            }
            if running[k].len() + 1 > slots(k) {
                return Err(format!("{} past its {} slots", cfg.macs[k].name, slots(k)));
            }
            if k > 0 && (!every::offered(step) || every::LEAD_ONLY.contains(&step)) {
                return Err(format!(
                    "{} (a member) placed {step}, kept to the lead",
                    cfg.macs[k].name
                ));
            }
            if cold && !running[k].is_empty() {
                return Err(format!(
                    "{} placed a cold start of {step} beside {:?}",
                    cfg.macs[k].name,
                    running[k].iter().map(|j| &j.step).collect::<Vec<_>>()
                ));
            }
            if running[k].iter().any(|j| j.cold) {
                return Err(format!(
                    "{} placed {step} beside a cold start",
                    cfg.macs[k].name
                ));
            }
            if cold && running.iter().flatten().any(|j| j.step == step) {
                return Err(format!(
                    "{} placed a cold start of {step} while another of its jobs runs",
                    cfg.macs[k].name
                ));
            }
            running[k].push(job);
            out.most[k] = out.most[k].max(running[k].len());
            Ok(true)
        };

        // The lead: its plan (its own and offered steps alike, in order) less what others hold
        // and a step whose cold start runs elsewhere; a job a target.
        let plan: Vec<(String, String)> = cfg
            .steps
            .iter()
            .filter(|s| !(every::offered(s.name) && c.cold_under_way(s.name)))
            .flat_map(|s| {
                let held = c.held(s.name);
                open(s)
                    .into_iter()
                    .filter(move |tg| !held.contains(tg))
                    .map(move |tg| (s.name.to_string(), tg))
            })
            .collect();
        let lead_limit = limit(0);
        let lead_free = |r: &[Job]| (cfg.macs[0].disk_mb.saturating_sub(used(r).1)) << 20;
        let beside = |spec: &(String, String), other: &Job, r: &[Job]| -> Option<String> {
            if crate::agent::clash(&spec.0, &other.step) {
                return Some("they don't run together".into());
            }
            crate::agent::beside_why(
                measured(&spec.0, std::slice::from_ref(&spec.1)),
                measured(&other.step, &other.targets),
                false,
                Some(other.mem_mb),
                lead_limit,
                0,
                lead_free(r),
                &other.step,
            )
        };
        let mut tried: BTreeSet<(String, String)> = BTreeSet::new();
        // (Slot one free: the plan's first, unless it waits for the job beside it.)
        if running[0].is_empty() || (running[0].len() == 1 && slots(0) > 1 && !running[0][0].cold) {
            let first_free = running[0].is_empty();
            if first_free {
                if let Some(spec) = plan.first() {
                    tried.insert(spec.clone());
                    if let Some(l) = c.hold(&spec.0, &[(spec.1.clone(), key_of(&spec.1))]) {
                        start(0, &spec.0, vec![spec.1.clone()], l, &mut running, &mut out)?;
                    }
                }
            }
            // (The second: nothing beside a cold start, nor with one next; else the plan's first
            // that can run beside the first.)
            if running[0].len() == 1 && slots(0) > 1 && !running[0][0].cold {
                let head = plan.iter().find(|s| {
                    !running[0]
                        .iter()
                        .any(|j| j.step == s.0 && j.targets.contains(&s.1))
                });
                let head_cold =
                    head.is_some_and(|h| measured(&h.0, std::slice::from_ref(&h.1)).is_none());
                if !head_cold {
                    let pick = plan
                        .iter()
                        .filter(|s| {
                            !tried.contains(*s)
                                && !running[0]
                                    .iter()
                                    .any(|j| j.step == s.0 && j.targets.contains(&s.1))
                        })
                        .find(|s| beside(s, &running[0][0], &running[0]).is_none())
                        .cloned();
                    if let Some(spec) = pick {
                        if let Some(l) = c.hold(&spec.0, &[(spec.1.clone(), key_of(&spec.1))]) {
                            start(0, &spec.0, vec![spec.1.clone()], l, &mut running, &mut out)?;
                        }
                    }
                }
            }
        }
        // The lead's room for a cold start (the agent's `own_room`): none while it can start none.
        let own = if running[0].len() >= slots(0) || running[0].iter().any(|j| j.cold) {
            (0, 0)
        } else {
            (
                lead_limit.saturating_sub(used(&running[0]).0),
                cfg.macs[0]
                    .disk_mb
                    .saturating_sub(used(&running[0]).1 + reserve_mb),
            )
        };
        c.set_every(Some(Every { own: Some(own) }));

        // The members: each slot free asks.
        for k in 1..cfg.macs.len() {
            let w = clients[k].as_ref().unwrap();
            for j in &running[k] {
                let _ = w.beat(j.lease, None);
            }
            while running[k].len() < slots(k) {
                let (mem, disk, threads) = used(&running[k]);
                let m = &cfg.macs[k];
                let mem_mb = limit(k).saturating_sub(mem);
                let disk_mb = m.disk_mb.saturating_sub(disk + reserve_mb);
                let cap = Capacity {
                    limit_mb: limit(k),
                    held_mb: mem,
                    mem_mb,
                    cores: m.cores,
                    cores_free: m.cores - threads,
                    disk_mb,
                    ac: true,
                    nas: true,
                    home: true,
                    guard: true,
                    leases: running[k].iter().map(|j| j.lease).collect(),
                    ..Default::default()
                };
                let a = Ask {
                    worker: m.name.into(),
                    kind: "native".into(),
                    can: offered_steps.clone(),
                    mem_mb,
                    cores: m.cores - threads,
                    max: 6,
                    limit_mb: Some(limit(k)),
                    disk_mb: Some(disk_mb),
                    cap: Some(cap),
                    ..Default::default()
                };
                let Some(g) = w.ask(&a).map_err(|e| format!("ask: {e:#}"))? else {
                    break;
                };
                let Granted::Job { step, targets, .. } = g.work else {
                    return Err("a task granted".into());
                };
                if !start(
                    k,
                    &step,
                    targets.into_iter().map(|t| t.0).collect(),
                    g.lease,
                    &mut running,
                    &mut out,
                )? {
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

/// The steps of a map update's kind: two kept to the lead, four offered, one too large for a 16 GB
/// Mac (terrain).
pub fn plan() -> Vec<Step> {
    vec![
        Step {
            name: "bld-fetch",
            targets: 1,
            mem_mb: 800,
            disk_mb: 2_000,
            secs: 300,
            threads: 2,
        },
        Step {
            name: "terrain",
            targets: 6,
            mem_mb: 13_000,
            disk_mb: 3_000,
            secs: 600,
            threads: 4,
        },
        Step {
            name: "slope",
            targets: 10,
            mem_mb: 3_000,
            disk_mb: 500,
            secs: 200,
            threads: 4,
        },
        Step {
            name: "pack",
            targets: 24,
            mem_mb: 1_500,
            disk_mb: 200,
            secs: 120,
            threads: 4,
        },
        Step {
            name: "lo",
            targets: 12,
            mem_mb: 1_000,
            disk_mb: 100,
            secs: 90,
            threads: 4,
        },
        Step {
            name: "catalog",
            targets: 1,
            mem_mb: 2_000,
            disk_mb: 100,
            secs: 60,
            threads: 2,
        },
    ]
}

/// The M4 (the lead: 24 GB, 10 cores) and the M1 (16 GB, 8 cores).
pub fn m4_m1() -> Vec<Mac> {
    vec![
        Mac {
            name: "m4",
            total_mb: 24_576,
            cores: 10,
            disk_mb: 300_000,
        },
        Mac {
            name: "m1",
            total_mb: 16_384,
            cores: 8,
            disk_mb: 60_000,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Today's pool: the lead runs two jobs at once, the member one; nothing past a Mac's memory,
    /// cores or disk; cold starts alone; the lead's steps on the lead; the plan done.
    #[test]
    fn the_m4_and_the_m1_finish_two_slots_and_one() {
        let r = run(&Cfg {
            macs: m4_m1(),
            steps: plan(),
            member_slots: 1,
        })
        .unwrap();
        assert_eq!(r.most, [2, 1], "{r:?}");
        assert!(r.ran.iter().all(|&n| n > 0), "{r:?}");
    }

    /// Four Macs (one with little disk) finish the same plan, sooner than two.
    #[test]
    fn four_macs_finish() {
        let two = run(&Cfg {
            macs: m4_m1(),
            steps: plan(),
            member_slots: 1,
        })
        .unwrap();
        let mut macs = m4_m1();
        macs.push(Mac {
            name: "mini",
            total_mb: 16_384,
            cores: 8,
            disk_mb: 40_000,
        });
        macs.push(Mac {
            name: "air",
            total_mb: 8_192,
            cores: 8,
            disk_mb: 35_000,
        });
        let r = run(&Cfg {
            macs,
            steps: plan(),
            member_slots: 1,
        })
        .unwrap();
        assert!(
            r.ran.iter().skip(1).filter(|&&n| n > 0).count() >= 2,
            "{r:?}"
        );
        assert!(
            r.secs < two.secs,
            "{} s with four, {} s with two",
            r.secs,
            two.secs
        );
    }

    /// Members switched to two jobs each (task #239's multi-lease asks): today's coordinator gives a
    /// cold start to a member already running a job (`most_room` weighs room, not that the Mac is
    /// idle), which the simulator finds. #239 makes this run pass.
    #[test]
    fn members_with_two_slots_get_a_cold_start_beside_a_job_today() {
        let e = run(&Cfg {
            macs: m4_m1(),
            steps: plan(),
            member_slots: 2,
        })
        .unwrap_err();
        assert!(
            e.contains("placed a cold start of") && e.contains("beside"),
            "{e}"
        );
    }
}
