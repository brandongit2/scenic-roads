//! A simulated Mac's agent (crate::pool's `sim`; modelled): its loop around the pool's driver, its
//! role read from the declaration (docs/roles.md §1: a change restarts it into the new one), its
//! coordinator while it leads (crate::coord, in process: offered the plan's work, its own jobs
//! granted by `ask_own`, told what its driver's step came to as `agent::lead::after` and
//! `agent::pool`'s step tell it, its store backed up to the NAS), its member's beat to the lead
//! (crate::agent::beat::ToLead::post_on over the network) and its asks (`LeaseCalls::ask`), its
//! jobs (a few minutes each, a hand-off each as it ends; stopped when the lead says their lease
//! isn't its), and its restarts (an update, a crash, its disk replaced).

use super::*;
use crate::agent::beat::ToLead;
use crate::agent::brief::{Briefs, Fresh};
use crate::coord::beats::Beats;
use crate::coord::client::LeaseCalls;
use crate::coord::handoffs::Handoffs;
use crate::coord::{held, Ask, Capacity, Cost, Granted, Offer};
use crate::pool::driver::{Driver, Event, Heard, Out, Saved};
use crate::pool::Member;
use buildstore::handoff::Handoff;
use coordwire::client::Client;
use lead::Role;
use std::time::Duration;

/// Per loop while the faults last: that a free slot asks for work (a member's over the network,
/// the lead's own in process); that one of its jobs reads the current records.
const P_ASK: f64 = 0.5;
const P_READ: f64 = 0.2;
/// Per loop: that its map server asks its lead's feed (`/map/changes`); that the lead's `gc` job
/// takes a chunk (`/gc/chunk`); per job, that it finds its targets' files on the NAS as its brief
/// names them and takes them as they are (its hand-off reusing them, `Handoff::reused`).
const P_MAP: f64 = 0.5;
const P_GC: f64 = 0.2;
const P_REUSE: f64 = 0.3;
/// A `gc` job's chunk (crate::agent::gc::CHUNK_NAMES's, smaller: the simulator's names are few).
const GC_CHUNK: usize = 20;
/// A job's run (s).
const JOB_S: (u64, u64) = (60, 900);
/// The lead's store backed up this often (s: `pool::store::backup_every`).
const BACKUP_S: u64 = 600;
/// The memory, cores and disk each Mac asks with (MB, cores, MB): room for any job.
const MEM_MB: u64 = 16_000;
const CORES: u32 = 8;
const DISK_MB: u64 = 100_000;

/// A job running: its lease, targets, and when it ends (the run's time).
struct Job {
    epoch: u64,
    n: u64,
    targets: Vec<String>,
    /// Its targets' keys as its brief named them, their files found on the NAS and taken as they
    /// are (none: built anew).
    reused: Vec<(String, String)>,
    ends: u64,
}

pub(super) struct Mac {
    sim: Sim,
    k: usize,
    rng: Rng,
    me: Member,
    driver: Driver,
    /// Its driver's state as last saved (the agent saves it after each step, before acting on it):
    /// what a crash restarts it from.
    saved: Saved,
    jobs: u64,
    /// The epoch it leads in, as the world was told.
    leads: Option<u64>,
    /// Its role, decided as its process starts (docs/roles.md §1).
    role: Role,
    /// Its registry (a member's: its lead's answers; the lead's: its members' beats) and, leading,
    /// its coordinator's hand-offs.
    beats: Arc<Beats>,
    handoffs: Arc<Handoffs>,
    running: Vec<Job>,
    /// Its jobs' hand-offs handed to a step whose state isn't saved yet: in their jobs' folders
    /// still (crate::agent::pool::job_dir), handed again after a crash.
    unsaved: Vec<Entry>,
    /// Leading: its coordinator took another lead's state up once in this process (`Run::led`).
    took: bool,
    /// The Mac whose coordinator it reaches (the contact on the NAS, as it last read it).
    lead: Option<usize>,
    /// Its beat's last post failed.
    failing: bool,
    /// Leading: its store's backup's `seq` last written, and when (the run's time).
    backed: (u64, u64),
    /// Leading: GC's queue (opened on its map log's folder as its coordinator starts), and the
    /// records it last published the map of.
    gc: Option<Arc<store::map::gc::GcQueue>>,
    published: Option<(u64, u64)>,
    /// The brief it last had from its lead over the network (crate::agent::brief::Briefs).
    briefs: Briefs,
}

impl Mac {
    pub(super) fn new(sim: Sim, k: usize, seed: u64) -> Mac {
        let me = Member { id: id(k), host: host(k), app: app(0) };
        let rng = Rng(seed ^ (k as u64 + 1).wrapping_mul(0xA076_1D64_78BD_642F));
        let driver = Driver::unlocked(me.clone(), Saved::default());
        Mac {
            sim,
            k,
            rng,
            me,
            saved: driver.saved(),
            driver,
            jobs: 0,
            leads: None,
            role: Role::Unknown,
            beats: Arc::new(Beats::default()),
            handoffs: Arc::new(Handoffs::default()),
            running: Vec::new(),
            unsaved: Vec::new(),
            took: false,
            lead: None,
            failing: false,
            backed: (0, 0),
            gc: None,
            published: None,
            briefs: Briefs::default(),
        }
    }

    pub(super) fn run(mut self) {
        // (Its process crashed as it started: its next loop restarts it. Its thread had ended,
        // the turn left with it, and the run hung: the simulator's seed 1224 of every fault.)
        if let Err(e) = self.start() {
            if self.sim.over() {
                return;
            }
            self.sim.note(|| format!("mac{}: {e:#}", self.k));
        }
        loop {
            if let Err(e) = self.turn() {
                if self.sim.over() {
                    return;
                }
                self.sim.note(|| format!("mac{}: {e:#}", self.k));
            }
            if self.idle().is_err() {
                return;
            }
        }
    }

    /// Its process starts: its role read, its coordinator started when it's the lead.
    fn start(&mut self) -> Result<()> {
        self.role = lead::role(&self.sim, Some(&self.me.id));
        if self.role == Role::Lead {
            self.start_coord()?;
        }
        Ok(())
    }

    /// The lead's coordinator in this process (its store opened from its folder: its epoch one
    /// more), its clock its Mac's, granting nothing until its agent's step says it may (as
    /// `agent::make_coordinator_on` starts it); the plan's step measured on it.
    fn start_coord(&mut self) -> Result<()> {
        let (k, me, app) = (self.k, self.me.clone(), self.me.app.clone());
        let (beats, handoffs) = (self.beats.clone(), self.handoffs.clone());
        let mut gc = None;
        self.sim.alive(|w| -> Result<()> {
            let dir = w.macs[k].dir.join("coord");
            let clock = store::clock::Virtual::new_at(std::time::UNIX_EPOCH + Duration::from_secs(w.clock(k)));
            let c = Coordinator::in_process(crate::agent::seam::for_tests(), &dir, &me.host, &app, clock.clone())?;
            c.set_moving(Some(crate::agent::UNSTEPPED.into()));
            c.set_member(Some(&me.id));
            c.set_beats(Some(beats));
            c.set_handoffs(Some(handoffs));
            // (Its map log's feed and GC's queue, on its disk: the queue's file opened again.)
            let maplog = w.macs[k].dir.join("maplog");
            std::fs::create_dir_all(&maplog)?;
            let (gen, seq) = (w.macs[k].gen, w.macs[k].map_seq);
            let q = Arc::new(store::map::gc::GcQueue::default());
            q.open(&maplog, &map_of(seq, &w.maps.get(&(k, gen, seq)).cloned().unwrap_or_default()))?;
            c.set_maplog(w.macs[k].feed.clone(), q.clone());
            gc = Some(q);
            w.macs[k].coords += 1;
            let cost = Cost {
                peak_mb: 1_000,
                secs: 1_500,
                v: crate::coord::cost_version(STEP),
                sizes: [("threads".to_string(), 2), ("cpu_ms".to_string(), 2 * 1_500_000)].into(),
                ..Default::default()
            };
            let keys: Vec<String> = TARGETS.iter().map(|t| crate::coord::cost_key(STEP, t)).collect();
            c.add_costs(&keys.iter().map(|k| (k.clone(), cost.clone())).collect::<Vec<_>>());
            c.add_disks(&keys.iter().map(|k| (k.clone(), 1_000)).collect::<Vec<_>>());
            w.macs[k].epoch = c.shared.lock().unwrap().keep.store().map_or(0, |s| s.epoch());
            w.macs[k].coord = Some(Arc::new(c));
            w.macs[k].clock = Some(clock);
            w.known[k].clear();
            w.seen_coord(k);
            w.count("lead coordinators started");
            let e = w.macs[k].epoch;
            w.note(|| format!("mac{k}'s coordinator starts, its store's epoch {e}"));
            Ok(())
        })??;
        (self.gc, self.published) = (gc, None);
        self.backed = (0, self.sim.quiet(|w| w.t)?);
        Ok(())
    }

    /// Restarts its process: its coordinator stopped (`clean`: backed up first, as its stop
    /// writes it; or gone with a crash), its memory gone but its saved state, or that lost too
    /// (`Cfg::lose`, its disk replaced); its role read again.
    fn restart(&mut self, clean: bool) -> Result<()> {
        let k = self.k;
        if clean && self.role == Role::Lead {
            self.back_up(true);
        }
        let keeps: BTreeSet<String> =
            self.saved.mine.keys().cloned().chain(self.unsaved.iter().filter_map(Entry::key)).collect();
        let lost = self.sim.quiet(|w| {
            w.stop_coord(k, clean);
            w.running.retain(|_, r| r.mac != k);
            let lost = std::mem::take(&mut w.macs[k].lose) || (w.cfg.lose && w.rng.chance(0.3));
            if lost {
                w.state_lost(&keeps);
                w.macs[k].local = None;
                w.count("saved states lost");
            }
            w.macs[k].crashed = false;
            w.count("restarts");
            lost
        })?;
        let saved = if lost { Saved::default() } else { self.saved.clone() };
        if lost {
            // (Their jobs' folders gone with its disk or state.)
            self.unsaved.clear();
        }
        self.me.app = self.sim.quiet(|w| app(w.macs[k].app))?;
        self.driver = Driver::unlocked(self.me.clone(), saved);
        self.saved = self.driver.saved();
        let mine: BTreeSet<String> = self.saved.mine.keys().cloned().collect();
        self.sim.quiet(|w| w.macs[k].mine = mine)?;
        (self.beats, self.handoffs) = (Arc::new(Beats::default()), Arc::new(Handoffs::default()));
        self.running.clear();
        (self.took, self.lead, self.failing) = (false, None, false);
        (self.gc, self.published, self.briefs) = (None, None, Briefs::default());
        if self.leads.take().is_some() {
            self.sim.quiet(|w| w.leads(k, None))?;
        }
        self.start()
    }

    /// One loop of the agent.
    fn turn(&mut self) -> Result<()> {
        let k = self.k;
        // A process that died, or another app installed: a restart.
        let (crashed, app_now) = self.sim.quiet(|w| (w.macs[k].crashed, app(w.macs[k].app)))?;
        if crashed || app_now != self.me.app {
            self.restart(!crashed)?;
        }
        // Its role, read again: another (the owner moved the lead) restarts it into that one.
        let role = lead::role(&self.sim, Some(&self.me.id));
        if role != Role::Unknown && role != self.role && !self.sim.quiet(|w| w.macs[k].crashed)? {
            self.sim.note(|| format!("mac{k}'s role is now {role:?}: its agent restarts"));
            self.sim.count("restarts into a new role");
            self.restart(true)?;
        }
        let leading = self.role == Role::Lead;
        let faulting = self.sim.quiet(|w| w.faulting())?;
        let (t, now) = (self.sim.quiet(|w| w.t)?, Io::now(&self.sim));
        // Its jobs that ended: their hand-offs (and, leading, its own leases ended).
        let mut heard = Heard { entries: self.unsaved.clone(), ..Default::default() };
        let (ended, left): (Vec<Job>, Vec<Job>) =
            std::mem::take(&mut self.running).into_iter().partition(|j| j.ends <= t);
        self.running = left;
        for j in ended {
            let e = self.entry(&j, now);
            self.sim.alive(|w| {
                w.running.remove(&(j.epoch, j.n));
                w.handed_off(k, &e);
                if let (true, Some(c)) = (leading, w.macs[k].coord.clone()) {
                    w.tick(k);
                    c.finish(j.n, &[]);
                }
            })?;
            self.unsaved.push(e.clone());
            heard.entries.push(e);
        }
        // What it heard: leading, what its coordinator took; a member, its lead's word.
        let mut given: Vec<String> = heard.entries.iter().filter_map(Entry::key).collect();
        if leading {
            let h = self.handoffs.clone();
            heard.handed = self.sim.alive(|_| h.taken())?;
            given.extend(heard.handed.iter().filter_map(Entry::key));
            given.extend(self.driver.mine().keys().cloned());
        } else {
            heard.merged = self.beats.take_merged();
            let merged = heard.merged.clone();
            self.sim.alive(|w| w.told_merged(k, &merged))?;
            heard.lead_says = self.beats.lead_says(now);
        }
        // (The merge's checks: the simulator's, then GC's refusal, as crate::agent::pool's step
        // checks: last, since one it lets merge holds its names from GC's chunks.)
        let gc = self.gc.clone().filter(|_| leading);
        let checks = |e: &Entry, r: &Records| {
            check(e, r)?;
            match gc.as_deref().and_then(|q| crate::agent::gcqueue::refusal(q, e, now)) {
                Some(why) => Err(why),
                None => Ok(()),
            }
        };
        let out = self.driver.step(&self.sim, heard, &checks);
        // (When its checks ran, as the agent takes it right after its step: what its coordinator
        // grants by counts from then, a sleep after included, `Shared::told`.)
        let checked = Io::now(&self.sim);
        // (Its process died during the step: nothing of it saved or acted on.)
        if self.sim.quiet(|w| w.macs[k].crashed)? {
            return Ok(());
        }
        self.saved = self.driver.saved();
        self.unsaved.clear();
        self.after(out, faulting, &given, checked)
    }

    /// What its driver's step came to, its state saved: acted on.
    fn after(&mut self, out: Out, faulting: bool, given: &[String], checked: u64) -> Result<()> {
        let k = self.k;
        let mine: BTreeSet<String> = self.saved.mine.keys().cloned().collect();
        let caught = out.leads.is_some() && out.caught_up;
        self.sim.alive(|w| {
            w.macs[k].mine = mine;
            w.macs[k].caught = caught;
        })?;
        if let (Some(e), true) = (out.leads, out.caught_up) {
            self.sim.op(|w, me| w.caught_up(me, e, given, &out.held))?;
        }
        for ev in &out.events {
            self.event(ev)?;
        }
        if out.leads != self.leads {
            self.sim.op(|w, me| w.leads(me, out.leads))?;
            self.leads = out.leads;
        }
        if let (Some(e), false) = (out.leads, out.merged.is_empty()) {
            self.sim.op(|w, me| w.ack(me, e, &out.merged))?;
        }
        if self.role == Role::Lead {
            self.lead_after(&out, faulting, checked)?;
        } else {
            self.member_after(&out, faulting)?;
        }
        // A job reading the records: the one path's.
        if self.rng.chance(P_READ) {
            if let Ok(Some(r)) = records::load_one(&self.sim) {
                if let Err(why) = consistent(&r) {
                    self.sim.op(|w, me| {
                        w.wrong(format!("mac{me} read epoch {}'s records at {}, not consistent: {why}", r.epoch, r.seq))
                    })?;
                }
            }
        }
        Ok(())
    }

    /// The lead's agent after its step (crate::agent::pool's step, `took_up`, `lead::after`): its
    /// coordinator told what the step came to, the plan's work offered, its own slot's job asked
    /// for, its store backed up when due, its contact published.
    fn lead_after(&mut self, out: &Out, faulting: bool, checked: u64) -> Result<()> {
        let k = self.k;
        let (me, h) = (self.me.clone(), self.handoffs.clone());
        // (Another Mac's lead before it left its store's backup: taken up once a process.)
        let first =
            out.events.iter().any(|e| matches!(e, Event::TookUp { .. })) && !std::mem::replace(&mut self.took, true);
        let backup = if first {
            crate::pool::store::read_backup(&self.sim).ok().flatten().filter(|b| b.member != me.host)
        } else {
            None
        };
        let own: Vec<u64> = self.running.iter().map(|j| j.n).collect();
        let records = self.driver.records().cloned();
        let saved_records = self.driver.saved_records();
        let gates = crate::agent::pool::Gates {
            leads: out.leads,
            duties: out.duties,
            caught_up: out.caught_up,
            vouched: out.vouched,
            held: out.held.clone(),
        };
        let ask = faulting && self.running.is_empty() && self.rng.chance(P_ASK);
        let cap = self.capacity(checked);
        let q = self.gc.clone();
        let publish = saved_records
            .filter(|s| records.as_ref().is_some_and(|r| (r.epoch, r.seq) == *s) && self.published != Some(*s));
        let collect = out.duties && out.vouched && self.rng.chance(P_GC);
        let poll = self.rng.chance(P_MAP);
        let granted = self.sim.alive(|w| -> Result<Option<crate::coord::Grant>> {
            let Some(c) = w.macs[k].coord.clone() else { return Ok(None) };
            w.tick(k);
            h.merged(&out.merged);
            for e in &out.events {
                if let Event::TookUp { epoch, .. } = e {
                    c.set_epoch(*epoch);
                }
            }
            if let Some(b) = &backup {
                match crate::agent::pool::load_backup(&c, b) {
                    Ok(n) => {
                        w.count("lead states taken up from another's backup");
                        w.note(|| format!("mac{k}'s coordinator took up {n} leases from {}'s backup", b.member));
                    }
                    Err(e) => w.wrong(format!("mac{k} can't take up {}'s backup: {e:#}", b.member)),
                }
            }
            if first {
                c.drop_workers(&crate::agent::slot_workers(&me.host), &own);
            }
            if let Some(e) = out.leads {
                c.set_epoch(e);
            }
            c.set_led(out.leads.filter(|_| out.vouched).map(|e| (e, me.clone())));
            match &records {
                Some(r) if saved_records == Some((r.epoch, r.seq)) => c.set_brief(Some(r)),
                Some(_) => {}
                None => c.set_brief(None),
            }
            // (Its map published after each save that reached the NAS, GC's queue told of the
            // records; its `gc` job's chunk, removed, said gone; its own map server's ask.)
            if let (Some(r), Some(q), Some(_)) = (&records, &q, publish) {
                let now = w.clock(k);
                w.publish(k, r, q, now);
            }
            if collect {
                let ask = format!("{{\"max\": {GC_CHUNK}}}");
                let (code, v) = c.call(crate::agent::gc::CHUNK, ask.as_bytes(), &me.host)?;
                let names: Vec<String> = serde_json::from_value(v["names"].clone()).unwrap_or_default();
                if code == 200 && !names.is_empty() {
                    w.gc_removes(k, &names, records.as_ref());
                    let gone = serde_json::json!({ "gone": names });
                    let (code, v) = c.call(crate::agent::gc::GONE, &serde_json::to_vec(&gone)?, &me.host)?;
                    if code != 200 {
                        w.note(|| format!("mac{k}'s gc job: its gone: {code} {v}"));
                    }
                }
            }
            if poll {
                w.map_poll(k, k);
            }
            c.set_moving_as_of(crate::agent::pool::moving(&gates, false), checked);
            c.keep_own(own.iter().map(|&n| (n, 0)).collect());
            c.set_plan(1);
            let key = |t: &str| (t.to_string(), "k".to_string(), 0);
            c.offer(
                "sim",
                vec![Offer { step: STEP.into(), targets: TARGETS.iter().map(|t| key(t)).collect(), job: None }],
            );
            let g = if ask { c.ask_own(&Self::ask_of(&me.app, cap, own.len()), &BTreeSet::new(), 1) } else { None };
            w.seen_coord(k);
            w.one_granting();
            Ok(g)
        })??;
        if let Some(p) = publish {
            self.published = Some(p);
        }
        if let Some(g) = granted {
            self.start_job(g, k)?;
        }
        // Its store backed up each `BACKUP_S`, its contact published.
        if self.sim.quiet(|w| w.t)? >= self.backed.1 + BACKUP_S {
            self.back_up(false);
        }
        let contact = format!("{{\"mac\": {k}}}").into_bytes();
        if Nas::read(&self.sim, CONTACT).ok().flatten().as_deref() != Some(&contact[..]) {
            Nas::write_whole(&self.sim, CONTACT, &contact).ok();
        }
        Ok(())
    }

    /// Its store's snapshot to the NAS (crate::pool::store::back_up_once), as a stop writes it
    /// (`stopping`: whether it changed or not since).
    fn back_up(&mut self, stopping: bool) {
        let k = self.k;
        let Ok(Some(snap)) = self.sim.alive(|w| {
            let c = w.macs[k].coord.clone()?;
            let s = c.shared.lock().unwrap().keep.store().cloned()?;
            Some(crate::pool::store::Snapshot { member: host(k), at: w.clock(k), ..s.snapshot() })
        }) else {
            return;
        };
        let backed = if stopping { u64::MAX } else { self.backed.0 };
        match crate::pool::store::back_up_once(&snap, &self.sim, backed) {
            Ok(seq) => {
                self.backed = (seq, self.sim.quiet(|w| w.t).unwrap_or(0));
                self.sim.count("lead stores backed up");
            }
            Err(e) => self.sim.note(|| format!("mac{k}'s backup: {e:#}")),
        }
    }

    /// A member's agent after its step: its beat to its lead (the leases its jobs hold, the
    /// hand-offs it keeps: those the lead lacks posted), the jobs whose leases the answer says
    /// aren't its stopped, and its free slot's ask.
    fn member_after(&mut self, out: &Out, faulting: bool) -> Result<()> {
        let k = self.k;
        // (Its lead's coordinator: where the contact on the NAS says.)
        if let Ok(Some(b)) = Nas::read(&self.sim, CONTACT) {
            self.lead = serde_json::from_slice::<serde_json::Value>(&b)
                .ok()
                .and_then(|v| v["mac"].as_u64())
                .map(|m| m as usize);
        }
        let Some(to) = self.lead else { return Ok(()) };
        if self.rng.chance(P_MAP) {
            self.sim.op(|w, me| w.map_poll(me, to))?;
        }
        let now = Io::now(&self.sim);
        self.beats.set_own(crate::agent::pool::Heartbeat { pool: out.beat.clone(), ..Default::default() });
        self.beats.set_handoffs(self.driver.mine().entries().cloned().collect());
        let body = held::Beat {
            member: host(k),
            speaks: crate::agent::beat::speaks(),
            held: self.running.iter().map(|j| j.n).collect(),
            heartbeat: self.beats.own(),
            ..Default::default()
        };
        let to_lead = ToLead {
            urls: Vec::new(),
            token: String::new(),
            member: host(k),
            held: BTreeSet::new(),
            listed: std::time::Instant::now(),
            cap: None,
            beats: self.beats.clone(),
            home: self.sim.quiet(|w| w.macs[k].dir.clone())?,
        };
        let c = self.client(to);
        c.new_round();
        self.failing = to_lead.post_on(&c, &body, self.failing, Duration::ZERO, now);
        let released = self.beats.take_released();
        if !released.is_empty() {
            let stopped: Vec<Job> = {
                let (s, keep) = std::mem::take(&mut self.running).into_iter().partition(|j| released.contains(&j.n));
                self.running = keep;
                s
            };
            self.sim.alive(|w| {
                for j in &stopped {
                    w.running.remove(&(j.epoch, j.n));
                    w.count("jobs stopped, their leases not the lead's");
                    w.note(|| format!("mac{k} stops {}-{}: its lead says the lease isn't its", j.epoch, j.n));
                }
            })?;
        }
        if faulting && self.running.is_empty() && self.rng.chance(P_ASK) {
            let a = Self::ask_of(&self.me.app, self.capacity(now), 0);
            match c.ask(&a) {
                Ok(Some(g)) => self.start_job(g, to)?,
                Ok(None) => self.sim.count("asks given nothing"),
                Err(_) => self.sim.count("asks refused or unanswered"),
            }
        }
        Ok(())
    }

    /// Its client to Mac `to`'s coordinator, over the network.
    fn client(&self, to: usize) -> Client {
        let sim = self.sim.clone();
        Client::through(Arc::new(move |_method, path, body| sim.wire(to, path, body)), &host(self.k))
    }

    /// Its Mac's capacity now (`at`: its clock), its jobs' room.
    fn capacity(&self, at: u64) -> Capacity {
        Capacity {
            at,
            limit_mb: MEM_MB,
            mem_mb: MEM_MB,
            cores: CORES,
            cores_free: CORES,
            disk_mb: DISK_MB,
            ac: true,
            nas: true,
            home: true,
            leases: self.running.iter().map(|j| j.n).collect(),
            ..Default::default()
        }
    }

    /// Its ask for one job on app `app`, with `cap`.
    fn ask_of(app: &str, cap: Capacity, running: usize) -> Ask {
        Ask {
            kind: "native".into(),
            app: Some(app.to_string()),
            can: vec![STEP.into()],
            mem_mb: MEM_MB,
            cores: CORES,
            limit_mb: Some(MEM_MB),
            disk_mb: Some(DISK_MB),
            cap: Some(cap),
            want: usize::from(running == 0),
            ..Default::default()
        }
    }

    /// The job grant `g` of Mac `by`'s coordinator gives it starts, with the brief its lead has
    /// now (crate::agent::brief::Briefs::fresh: in process leading, else over the network; the NAS's
    /// when the lead serves none yet): the lead not answering, it doesn't start, its lease left out
    /// of its next beat (docs/coordination.md step 8d). Now and then its targets' files as that
    /// brief names them are found on the NAS and taken as they are.
    fn start_job(&mut self, g: crate::coord::Grant, by: usize) -> Result<()> {
        let Granted::Job { targets, .. } = g.work else { return Ok(()) };
        let targets: Vec<String> = targets.into_iter().map(|t| t.0).collect();
        let (k, lease) = (self.k, (g.epoch, g.lease));
        let worker = host(k);
        let here = self.sim.alive(|w| {
            let c = w.macs[by].coord.clone();
            let mark = c.as_ref().and_then(|c| c.brief()).map(|h| h.mark);
            w.grant_marks.entry(lease).or_insert((by, w.macs[by].coords, mark));
            c.filter(|_| by == k)
        })?;
        let fresh = match &here {
            Some(c) => {
                self.sim.alive(|w| w.served = Some((k, w.macs[k].coords)))?;
                self.briefs.fresh(crate::agent::brief::Lead::Here(c))
            }
            None if by == k => Fresh::Away("its coordinator is gone".into()),
            None => self.briefs.fresh(crate::agent::brief::Lead::At(&self.client(by))),
        };
        let brief = match fresh {
            Fresh::Got(h) => {
                self.sim.alive(|w| {
                    let from = w.served;
                    w.opens(k, lease, from, h.mark)
                })?;
                Some(h.brief)
            }
            Fresh::Nas(_) => {
                self.sim.count("jobs opening the NAS's brief (their lead serves none yet)");
                match Nas::read(&self.sim, records::ONE_BRIEF) {
                    Ok(Some(b)) => serde_json::from_slice::<records::Brief>(&b).ok().map(Arc::new),
                    _ => None,
                }
            }
            Fresh::Away(why) => {
                self.sim.count("jobs not started: the lead away");
                self.sim.note(|| format!("mac{k} doesn't start {}-{}: the lead's brief: {why}", lease.0, lease.1));
                return Ok(());
            }
        };
        let Some(brief) = brief else {
            self.sim.count("jobs not started: no brief");
            return Ok(());
        };
        let mut reused = Vec::new();
        if self.rng.chance(P_REUSE) {
            let keys: Option<Vec<(String, String)>> =
                targets.iter().map(|t| brief.keys.unit.get(t).map(|key| (t.clone(), key.clone()))).collect();
            if let Some(keys) = keys {
                let gone = self.sim.alive(|w| keys.iter().any(|(t, key)| w.removed.contains_key(&content(t, key))))?;
                if gone {
                    self.sim.count("jobs finding a file their brief names gone: built anew");
                } else {
                    reused = keys;
                }
            }
        }
        let d = self.rng.range(JOB_S.0, JOB_S.1);
        let ends = self.sim.alive(|w| {
            w.starts(k, by, g.epoch, g.lease, &worker, &targets);
            w.t + d
        })?;
        self.running.push(Job { epoch: g.epoch, n: g.lease, targets, reused, ends });
        Ok(())
    }

    fn event(&mut self, ev: &Event) -> Result<()> {
        let k = self.k;
        match ev {
            Event::TookUp { how, .. } => {
                let kind = match how.as_str() {
                    "its own records" => "take-ups from its own records",
                    h if h.starts_with("the one path's") => "take-ups from the one path's records",
                    _ => "take-ups from today's files",
                };
                self.sim.count(kind);
            }
            Event::Merged { applied, overtaken, .. } => {
                let (n, e) = (*overtaken as u64, self.driver.leads().unwrap_or(0));
                self.sim.alive(|w| {
                    *w.counts.entry("hand-offs passed over for a later lease's").or_default() += n;
                    w.applied(k, e, applied);
                })?;
            }
            Event::Waits { what: "take up the records", why } => {
                self.sim.count("take-ups tried again");
                // (Records older than the one path's brief: the Mac that led its epoch has them.)
                let led = why.split("the Mac that led epoch ").nth(1).and_then(|r| r.split(':').next()?.parse().ok());
                if let Some(e) = led {
                    self.sim.quiet(|w| w.waits_on(k, e))?;
                }
            }
            Event::Failed { what: "save the records", .. } => self.sim.count("saves tried again"),
            _ => {}
        }
        self.sim.note(|| format!("mac{k}: {ev:?}"));
        Ok(())
    }

    /// Its job `j`'s hand-off: its targets' files reused as its brief named them (refused when GC
    /// removed one under its lease); else its units built with a new key, now and then a prune, one that
    /// writes outside its step's outputs too (refused by every lead), or a flaky one (refused by
    /// the lead of an odd epoch).
    fn entry(&mut self, j: &Job, now: u64) -> Entry {
        self.jobs += 1;
        let (k, n) = (self.k, self.jobs);
        let ts = &j.targets;
        let built = |key: &str| Handoff {
            changes: ts.iter().map(|t| (logical(t), Some(content(t, key)))).collect(),
            pending: ts.iter().map(|t| (content(t, key), "sha".into())).collect(),
            done: Some((STEP.into(), ts.iter().map(|t| (t.clone(), key.to_string())).collect())),
            ..Default::default()
        };
        let h = match self.rng.below(20) {
            _ if !j.reused.is_empty() => Handoff {
                changes: j.reused.iter().map(|(t, key)| (logical(t), Some(content(t, key)))).collect(),
                reused: j.reused.iter().map(|(t, key)| content(t, key)).collect(),
                done: Some((STEP.into(), j.reused.clone())),
                ..Default::default()
            },
            0 => Handoff {
                changes: ts
                    .iter()
                    .flat_map(|t| {
                        [
                            (logical(t), Some(content(t, "bogus"))),
                            (format!("elsewhere/{t}"), Some(format!("elsewhere/{t}.bogus.x"))),
                        ]
                    })
                    .collect(),
                done: Some((STEP.into(), ts.iter().map(|t| (t.clone(), "bogus".into())).collect())),
                ..Default::default()
            },
            1 | 2 => Handoff {
                changes: ts.iter().map(|t| (logical(t), None)).collect(),
                done: Some(("prune".into(), ts.iter().map(|t| (format!("unit {t}"), String::new())).collect())),
                ..Default::default()
            },
            3 | 4 => built(&format!("f{k}x{n}")),
            _ => built(&format!("k{k}x{n}")),
        };
        Entry {
            member: self.me.id.clone(),
            lease: LeaseId { epoch: j.epoch, n: j.n },
            step: STEP.into(),
            handoff: h,
            at: now,
        }
    }

    /// Waits for its next loop. A member leaves for good now and then, between two of its loops:
    /// its lid closed for the week (never the declared lead: with it gone, nothing builds until
    /// it's back).
    fn idle(&self) -> Result<()> {
        let k = self.k;
        self.sim.quiet(|w| {
            let j = w.rng.below(5);
            w.macs[k].ready_at = w.t + LOOP_S + j;
            let leaves = w.cfg.leave
                && k != w.declared
                && !w.macs.iter().any(|m| m.gone)
                && w.t > w.cfg.faults / 3
                && w.faulting()
                && w.rng.chance(0.02);
            if leaves {
                w.macs[k].gone = true;
                w.macs[k].asleep_until = u64::MAX;
                w.count("Macs gone for good");
                w.note(|| format!("mac{k} leaves for good"));
            }
            w.after(k);
            if w.turn != k || w.done {
                self.sim.sh.cv.notify_all();
            }
        })
    }
}
