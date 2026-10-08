//! The pool's shadow run (docs/pool.md §12): the pool's driver beside today's coordination, on the
//! real NAS, deciding what the pool would (terms, take-ups, merges, gates) and acting on nothing.
//! It reads the build's records where today's agents keep them, and writes only under
//! `state/pool-shadow/` (crate::agent::pool::Overlay): its terms, its records per term, its
//! journal, its members' heartbeats and mail. Nothing the build reads.
//!
//! It shadows one agent, this Mac's (`Watch`), by what that agent leaves in its own folder, which it
//! reads and never writes:
//!
//! - on the build Mac, its coordinator's history (`coord/history.jsonl`): its own jobs as they start
//!   and end, the helpers' leases, the rounds' catalogs, its restarts. A job of its own that ended
//!   is a hand-off of this member's: its done record (its targets' keys as the agent recorded them)
//!   and the files of those targets it changed (crate::coord::saves; a step whose files that
//!   doesn't name hands off its done record alone);
//! - on a helper, its outbox (`outbox/<lease>/`): a leased job's work, saves and done marks while
//!   it runs; gone, it's handed off, and that hand-off is this member's.
//!
//! Each becomes a journal entry of the shadow's member, handed to its driver as the agent's jobs'
//! would be. The agent restarting is the shadow's driver made again from its saved state (a process
//! of its own would re-assert its term as the agent's restart will). A GC the agent begins has the
//! shadow's lead re-assert, as the pool's agent would before a sweep. What it decides is written to
//! its log (`shadow.jsonl` in its folder), a line each: the driver's events; its gates (`gate`:
//! may it grant, publish, sweep) when they change and whenever the agent made a catalog or began a
//! GC; and, after its lead merges, its records against the build's (`compare`).

use super::pool::{self, Give, Overlay, Side};
use super::{build, cond};
use crate::handoff::Handoff;
use crate::pool::driver::{Event, Out};
use crate::pool::journal::{Entry, LeaseId};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a helper's job's keys are waited for in the build's records before its hand-off is made
/// without them (its done record then the lease's).
const KEYS_WAIT: Duration = Duration::from_secs(900);
/// How often the shadow lead's records are compared whole with the build's.
const COMPARE_EVERY: Duration = Duration::from_secs(600);

/// What a shadow run is given.
#[derive(Clone, Debug)]
pub struct Options {
    /// The NAS project folder; None: the mounted share (never mounted by the shadow).
    pub root: Option<PathBuf>,
    /// Its own folder: its member id, saved state, mail marks and log (never the agent's).
    pub home: PathBuf,
    /// The folder of the agent it shadows (read, never written).
    pub live: PathBuf,
    /// The app it says it runs.
    pub app: String,
    /// One loop, then exit.
    pub once: bool,
    /// Stop after this long.
    pub stop_after: Option<Duration>,
}

/// A job of the shadowed agent's, as seen.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Seen {
    step: String,
    targets: Vec<(String, String)>,
    /// Its lease in the shadow: the term the shadow knew as it began, and its number (the history's
    /// event, or the helper's lease).
    term: u64,
    n: u64,
    /// The manifest's entries for its targets' files as it began (the build Mac's), and its saves
    /// and done marks so far (a helper's).
    #[serde(default)]
    before: BTreeMap<String, String>,
    #[serde(default)]
    handoff: Handoff,
    #[serde(default)]
    done_marks: Vec<String>,
    began: u64,
    /// Ended (unix seconds), and whether it went well; a helper's waits for its keys.
    #[serde(default)]
    ended: Option<u64>,
    #[serde(default)]
    ok: bool,
}

/// What the shadow keeps between its processes (`watch.json`): how far it read the agent's
/// history, the jobs under way, and the agent's start it last saw.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Watched {
    seq: u64,
    jobs: BTreeMap<String, Seen>,
    started: u64,
}

/// What the shadowed agent did since the last look.
#[derive(Debug, Default)]
struct Saw {
    entries: Vec<Entry>,
    restarted: bool,
    gc: bool,
    catalogs: Vec<String>,
    notes: Vec<serde_json::Value>,
}

/// A shadow run's state in this process.
pub struct Shadow {
    side: Side,
    root: PathBuf,
    live: PathBuf,
    dir: PathBuf,
    w: Watched,
    log: std::fs::File,
    /// Re-assert asked, a GC begun, until a step says it's fresh.
    reassert: bool,
    /// The gates as last logged.
    gates: Option<serde_json::Value>,
    compared: Option<Instant>,
}

impl Shadow {
    /// A shadow run of the agent whose folder is `live`, over the project folder `root`, its own
    /// folder `home`. None when another process is this member already.
    pub fn open(root: &Path, home: &Path, live: &Path, app: &str) -> Result<Option<Shadow>> {
        anyhow::ensure!(home != live, "the shadow's folder is the agent's own");
        let dir = home.join("pool-shadow");
        let locks = home.parent().unwrap_or(home).to_path_buf();
        let nas: pool::SharedNas = Arc::new(Overlay::new(root));
        let Some(side) = Side::open(home, &dir, &locks, app, nas, true)? else { return Ok(None) };
        let w: Watched = std::fs::read(dir.join("watch.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("shadow.jsonl")).with_context(|| format!("open {}", dir.join("shadow.jsonl").display()))?;
        let mut s = Shadow { side, root: root.to_path_buf(), live: live.to_path_buf(), dir, w, log, reassert: false, gates: None, compared: None };
        // (A first run reads the history from its end: what happened before isn't shadowed.)
        if s.w.seq == 0 {
            s.w.seq = history(&s.live).last().map_or(0, |e| e.seq);
            s.w.started = s.live_started().unwrap_or(0);
        }
        s.note("start", serde_json::json!({ "member": s.side.member().id, "host": s.side.member().host, "app": app, "live": live, "history_seq": s.w.seq }));
        Ok(Some(s))
    }

    /// Writes a line to its log.
    fn note(&mut self, kind: &str, mut v: serde_json::Value) {
        if let Some(o) = v.as_object_mut() {
            o.insert("t".into(), crate::agent::jobs::now_s().into());
            o.insert("kind".into(), kind.into());
        }
        eprintln!("shadow: {kind} {v}");
        writeln!(self.log, "{v}").ok();
    }

    /// The shadowed agent's start, as its status says (the build Mac's heartbeat, or a helper's).
    fn live_started(&self) -> Option<u64> {
        let b = std::fs::read(self.live.join("status.json")).or_else(|_| std::fs::read(self.live.join("helper.json"))).ok()?;
        serde_json::from_slice::<serde_json::Value>(&b).ok()?["started"].as_u64()
    }

    /// One loop: what the agent did, handed to the driver; its step; what it decided, logged.
    pub fn tick(&mut self) -> Out {
        let keys = build::Keys::load_strict(&self.root).ok();
        let saw = self.look(keys.as_ref());
        for n in saw.notes {
            self.note("observed", n);
        }
        if saw.restarted {
            match self.side.restart() {
                Ok(()) => self.note("restarted", serde_json::json!({ "why": "the agent it shadows restarted" })),
                Err(e) => self.note("restarted", serde_json::json!({ "error": format!("{e:#}") })),
            }
        }
        self.reassert |= saw.gc;
        let entries: Vec<(Entry, Option<PathBuf>)> = saw.entries.into_iter().map(|e| (e, None)).collect();
        for (e, _) in &entries {
            self.note("handed", serde_json::json!({ "key": e.key(), "step": e.step, "done": e.handoff.done.as_ref().map(|d| d.1.len()), "changes": e.handoff.changes.len() }));
        }
        let (ac, battery) = cond::power();
        let able = store::nas::at_home() && (ac || battery.is_none_or(|b| b >= cond::BATTERY_MIN));
        let out = self.side.step(Give { entries, able, reassert: self.reassert, ..Default::default() }, &pool::check);
        if out.fresh {
            self.reassert = false;
        }
        for e in &out.events {
            self.note("event", serde_json::json!({ "said": pool::said(e), "event": event_json(e) }));
        }
        if !out.send.is_empty() {
            let sent: Vec<serde_json::Value> = out.send.iter().map(|(to, m)| serde_json::json!({ "to": to, "msg": m })).collect();
            self.note("sent", serde_json::json!({ "msgs": sent }));
        }
        if let Some(l) = &out.list {
            self.note("list", serde_json::json!({ "n": l.n, "since": l.since }));
        }
        let gates = serde_json::json!({ "term": out.term, "leads": out.leads, "duties": out.duties, "settle": out.settle, "caught_up": out.caught_up, "fresh": out.fresh, "listed_at": out.listed_at });
        let mut changed = self.gates.as_ref() != Some(&gates);
        // (`fresh` is a step's: logged when it's so, not each time it goes.)
        if let (Some(was), false) = (&self.gates, out.fresh) {
            let mut a = was.clone();
            a["fresh"] = false.into();
            changed = a != gates;
        }
        if changed {
            self.note("gate", serde_json::json!({ "gates": gates }));
            self.gates = Some(gates.clone());
        }
        for c in saw.catalogs {
            self.note("gate", serde_json::json!({ "real": c, "would": out.duties && out.caught_up, "gates": gates }));
        }
        if saw.gc {
            self.note("gate", serde_json::json!({ "real": "gc began", "would": out.fresh && out.caught_up, "gates": gates }));
        }
        let merged = out.events.iter().any(|e| matches!(e, Event::Merged { .. } | Event::TookUp { .. }));
        if out.leads.is_some() && (merged || self.compared.is_none_or(|t| t.elapsed() >= COMPARE_EVERY)) {
            if let Some(k) = &keys {
                self.compare(k);
            }
            self.compared = Some(Instant::now());
        }
        self.keep();
        out
    }

    /// The shadow lead's records against the build's: the keys whole, and the manifest's entries
    /// its jobs' targets' files.
    fn compare(&mut self, real: &build::Keys) {
        let Some(r) = self.side.driver().records() else { return };
        let (ours, theirs) = (keys_flat(&r.keys), keys_flat(real));
        let mut only_ours = Vec::new();
        let mut only_theirs = Vec::new();
        let mut differ = Vec::new();
        for (k, v) in &ours {
            match theirs.get(k) {
                None => only_ours.push(k.clone()),
                Some(t) if t != v => differ.push(k.clone()),
                _ => {}
            }
        }
        for k in theirs.keys() {
            if !ours.contains_key(k) {
                only_theirs.push(k.clone());
            }
        }
        let manifest: BTreeMap<String, String> = crate::out::read_record(&self.root.join("state/build/manifest.json")).unwrap_or_default();
        let m_differ: Vec<&String> = r.manifest.iter().filter(|(l, c)| manifest.get(*l).is_some_and(|m| m != *c)).map(|(l, _)| l).take(20).collect();
        let m_counts = (r.manifest.keys().filter(|l| !manifest.contains_key(*l)).count(), manifest.keys().filter(|l| !r.manifest.contains_key(*l)).count(), r.manifest.iter().filter(|(l, c)| manifest.get(*l).is_some_and(|m| m != *c)).count());
        let v = serde_json::json!({
            "term": r.term, "seq": r.seq, "reflected": r.reflected.len(), "rejected": r.rejected.len(),
            "keys": { "same": ours.len() - only_ours.len() - differ.len(), "differ": differ.len(), "only_shadow": only_ours.len(), "only_build": only_theirs.len(), "differ_sample": differ.iter().take(20).collect::<Vec<_>>(), "only_build_sample": only_theirs.iter().take(20).collect::<Vec<_>>(), "only_shadow_sample": only_ours.iter().take(20).collect::<Vec<_>>() },
            "manifest": { "only_shadow": m_counts.0, "only_build": m_counts.1, "differ": m_counts.2, "differ_sample": m_differ },
        });
        self.note("compare", v);
    }

    fn keep(&self) {
        if let Ok(b) = serde_json::to_vec(&self.w) {
            crate::whole::write(&self.dir.join("watch.json"), &b).ok();
        }
    }

    /// What the agent did since the last look: its history's new events (the build Mac's), its
    /// outbox's jobs (a helper's), its restarts.
    fn look(&mut self, keys: Option<&build::Keys>) -> Saw {
        let mut saw = Saw::default();
        // (The term it knows: at a first step, the one its saved state knew.)
        let term = self.side.driver().current().term.max(self.side.driver().saved().term);
        // Restarted: its status says another start.
        if let Some(st) = self.live_started() {
            if self.w.started != 0 && st != self.w.started {
                saw.restarted = true;
                saw.notes.push(serde_json::json!({ "what": "the agent restarted", "started": st }));
            }
            self.w.started = st;
        }
        let host = cond::host_name();
        let second = super::second_worker(&host);
        let from = self.w.seq;
        for e in history(&self.live).into_iter().filter(|e| e.seq > from) {
            self.w.seq = e.seq;
            let ours = e.worker.as_deref().is_some_and(|w| w == host || w == second);
            match e.kind.as_str() {
                "start" if ours => {
                    let step = e.step.clone().unwrap_or_default();
                    let targets: Vec<(String, String)> = e.targets.iter().map(|t| (t.clone(), key_of(keys, &step, t).unwrap_or_default())).collect();
                    let before = self.files_of(&step, &targets);
                    let job = Seen { step, targets, term, n: e.seq, before, began: e.t, ..Default::default() };
                    self.w.jobs.insert(format!("{} {}", e.worker.clone().unwrap_or_default(), job.step), job);
                    saw.notes.push(serde_json::json!({ "what": "began", "event": e }));
                }
                "end" if ours => {
                    let id = format!("{} {}", e.worker.clone().unwrap_or_default(), e.step.clone().unwrap_or_default());
                    let job = self.w.jobs.remove(&id);
                    saw.notes.push(serde_json::json!({ "what": "ended", "event": e, "began_seen": job.is_some() }));
                    if let Some(job) = job {
                        if let Some(entry) = self.own_entry(job, keys, e.ok == Some(true), crate::agent::jobs::now_s()) {
                            saw.entries.push(entry);
                        }
                    }
                    if e.step.as_deref() == Some("catalog") && e.ok == Some(true) {
                        saw.catalogs.push("catalog made".into());
                    }
                }
                "start" if e.step.as_deref() == Some("gc") => saw.gc = true,
                "agent" if ours => {
                    // (Its status's start says so too: counted once.)
                    if !saw.restarted && self.w.started != 0 {
                        saw.notes.push(serde_json::json!({ "what": "the agent started", "event": e }));
                    }
                }
                "catalog" => saw.catalogs.push(format!("round's catalog: {}", e.note)),
                "lease" | "done" | "fail" | "lapse" | "round" | "pause" | "resume" => saw.notes.push(serde_json::json!({ "what": e.kind, "event": e })),
                _ => {}
            }
            if e.kind == "start" && e.step.as_deref() == Some("gc") {
                saw.gc = true;
            }
        }
        self.look_outbox(keys, term, &mut saw);
        saw
    }

    /// The manifest's entries for the files a job of `step` saves for `targets` (crate::coord::saves).
    fn files_of(&self, step: &str, targets: &[(String, String)]) -> BTreeMap<String, String> {
        if !crate::agent::claims::SHARED.contains(&step) {
            return BTreeMap::new();
        }
        let m: BTreeMap<String, String> = crate::out::read_record(&self.root.join("state/build/manifest.json")).unwrap_or_default();
        m.into_iter().filter(|(l, _)| targets.iter().any(|(t, _)| crate::coord::saves(step, t, l))).collect()
    }

    /// The hand-off of a job of the build Mac's own that ended: its targets the agent recorded (with
    /// the keys it recorded), and those targets' files that changed since it began.
    fn own_entry(&self, job: Seen, keys: Option<&build::Keys>, ok: bool, at: u64) -> Option<Entry> {
        let done: Vec<(String, String)> = job.targets.iter().filter_map(|(t, was)| key_of(keys, &job.step, t).filter(|k| ok || *k != *was).map(|k| (t.clone(), k))).collect();
        let after = self.files_of(&job.step, &job.targets);
        let mut changes: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (l, c) in &after {
            if job.before.get(l) != Some(c) && done.iter().any(|(t, _)| crate::coord::saves(&job.step, t, l)) {
                changes.insert(l.clone(), Some(c.clone()));
            }
        }
        for l in job.before.keys() {
            if !after.contains_key(l) && done.iter().any(|(t, _)| crate::coord::saves(&job.step, t, l)) {
                changes.insert(l.clone(), None);
            }
        }
        if done.is_empty() && changes.is_empty() {
            return None;
        }
        let handoff = Handoff { changes, done: (!done.is_empty()).then(|| (job.step.clone(), done)), ..Default::default() };
        Some(Entry { member: self.side.member().id.clone(), lease: LeaseId { term: job.term, n: job.n }, step: job.step, handoff, at })
    }

    /// A helper's outbox: each leased job's work and saves while it runs; gone, its hand-off (once
    /// its keys show in the build's records, or `KEYS_WAIT` on).
    fn look_outbox(&mut self, keys: Option<&build::Keys>, term: u64, saw: &mut Saw) {
        let outbox = self.live.join("outbox");
        let Ok(rd) = std::fs::read_dir(&outbox) else {
            // (No outbox: not a helper's folder, or none yet. Jobs that ended still wait.)
            self.helper_ended(keys, &BTreeSet::new(), saw);
            return;
        };
        let mut there = BTreeSet::new();
        for d in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let Some(lease) = d.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok()) else { continue };
            let id = format!("lease {lease}");
            there.insert(id.clone());
            let Some(w) = std::fs::read(d.join("work.json")).ok().and_then(|b| serde_json::from_slice::<build::Work>(&b).ok()) else { continue };
            let job = self.w.jobs.entry(id).or_insert_with(|| {
                saw.notes.push(serde_json::json!({ "what": "a helper's job began", "lease": lease, "step": w.step, "targets": w.targets.len() }));
                Seen { step: w.step.clone(), targets: w.targets.clone(), term, n: lease, began: crate::agent::jobs::now_s(), ..Default::default() }
            });
            if let Ok(Some(hs)) = crate::handoff::written_in(&d) {
                let mut h = Handoff::default();
                for x in hs {
                    h.absorb(x);
                }
                job.handoff = h;
            }
            let marks = crate::control::read_done(&d.join("done.txt"), &job.step);
            if !marks.is_empty() {
                job.done_marks = marks;
            }
            if let Some(r) = std::fs::read(d.join("result.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
                job.ok = r["ok"].as_bool() == Some(true);
            }
        }
        self.helper_ended(keys, &there, saw);
    }

    /// A helper's jobs whose outbox folders went (handed back): their hand-offs, once their keys show
    /// in the build's records (the build Mac merged them), or `KEYS_WAIT` on with the lease's.
    fn helper_ended(&mut self, keys: Option<&build::Keys>, there: &BTreeSet<String>, saw: &mut Saw) {
        let now = crate::agent::jobs::now_s();
        let gone: Vec<String> = self.w.jobs.keys().filter(|id| id.starts_with("lease ") && !there.contains(*id)).cloned().collect();
        for id in gone {
            let job = self.w.jobs.get_mut(&id).unwrap();
            let ended = *job.ended.get_or_insert(now);
            // Its targets as the build's records have them now: those the lease's keys show.
            let shown: Vec<(String, String)> = job.targets.iter().filter(|(t, k)| key_of(keys, &job.step, t).as_deref() == Some(k.as_str())).cloned().collect();
            let marked: Vec<(String, String)> = job.targets.iter().filter(|(t, _)| job.done_marks.contains(t)).cloned().collect();
            let waited = now.saturating_sub(ended) >= KEYS_WAIT.as_secs();
            if shown.len() < job.targets.len() && !waited {
                continue;
            }
            let job = self.w.jobs.remove(&id).unwrap();
            // (Done: what the records show; waited out, what its marks say it finished, or every
            // target when it said it went well.)
            let done = if !shown.is_empty() { shown } else if job.ok { job.targets.clone() } else { marked };
            saw.notes.push(serde_json::json!({ "what": "a helper's job handed back", "lease": job.n, "step": job.step, "targets": job.targets.len(), "shown_in_records": done.len(), "waited": waited }));
            let mut h = job.handoff.clone();
            if let Some((step, ds)) = Some((job.step.clone(), done.clone())).filter(|(_, d)| !d.is_empty()) {
                h.changes.retain(|l, _| ds.iter().any(|(t, _)| crate::coord::saves(&step, t, l)));
                h.done = Some((step, ds));
            } else {
                h.done = None;
                h.changes.clear();
            }
            if h.done.is_none() && h.changes.is_empty() {
                continue;
            }
            saw.entries.push(Entry { member: self.side.member().id.clone(), lease: LeaseId { term: job.term, n: job.n }, step: job.step, handoff: h, at: now });
        }
    }
}

/// The key the records hold for `target` of `step`: per-target steps', the steps kept with the lo
/// keys under their own name, a catalog's.
fn key_of(keys: Option<&build::Keys>, step: &str, target: &str) -> Option<String> {
    let k = keys?;
    if let Some(v) = k.recorded(step, target) {
        return Some(v.to_string());
    }
    match step {
        "catalog" => k.catalog.clone(),
        "catalog-held" => k.catalog_held.clone(),
        _ => k.lo.get(target).cloned(),
    }
}

/// Keys as `<map> <target>` → key, for comparing.
fn keys_flat(k: &build::Keys) -> BTreeMap<String, String> {
    let v = serde_json::to_value(k).unwrap_or_default();
    let mut out = BTreeMap::new();
    if let Some(o) = v.as_object() {
        for (map, m) in o {
            match m {
                serde_json::Value::Object(m) => {
                    for (t, key) in m {
                        out.insert(format!("{map} {t}"), key.as_str().unwrap_or_default().to_string());
                    }
                }
                serde_json::Value::String(s) => {
                    out.insert(map.clone(), s.clone());
                }
                _ => {}
            }
        }
    }
    out
}

/// An event as JSON, for the log.
fn event_json(e: &Event) -> serde_json::Value {
    match e {
        Event::Made { term, how } => serde_json::json!({ "made": term, "how": how }),
        Event::TookUp { term, how, handed } => serde_json::json!({ "took_up": term, "how": how, "handed": handed.is_some() }),
        Event::SteppedDown { term, why } => serde_json::json!({ "stepped_down": term, "why": why }),
        Event::Handover { term, to, what } => serde_json::json!({ "handover": term, "to": to, "what": what }),
        Event::Merged { applied, overtaken, refused, listed } => serde_json::json!({ "merged": applied, "overtaken": overtaken, "refused": refused, "listed": listed }),
        Event::Waits { what, why } => serde_json::json!({ "waits": what, "why": why }),
        Event::Failed { what, why } => serde_json::json!({ "failed": what, "why": why }),
    }
}

/// The agent's coordinator's history, as its file holds it now (none when it has none: a helper).
fn history(live: &Path) -> Vec<crate::coord::history::Event> {
    let Ok(text) = std::fs::read_to_string(live.join("coord/history.jsonl")) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Runs a shadow run until stopped (SIGTERM, SIGINT), `stop_after`, or one loop with `once`.
pub fn run(o: Options) -> Result<()> {
    crate::sys::on_terminate(super::on_signal);
    let began = Instant::now();
    let mut shadow: Option<Shadow> = None;
    loop {
        let root = o.root.clone().or_else(|| super::find_root(false)).filter(|r| super::answers(r));
        match root {
            Some(root) => match pool::mode(&root) {
                Some(pool::Mode::On) => anyhow::bail!("the pool is on ({}): nothing to shadow", root.join(pool::ENABLED).display()),
                None => eprintln!("shadow: the NAS doesn't say whether the pool is on; waiting"),
                Some(_) => {
                    if shadow.is_none() {
                        shadow = Some(Shadow::open(&root, &o.home, &o.live, &o.app)?.context("another process is this shadow's member")?);
                    }
                    shadow.as_mut().unwrap().tick();
                }
            },
            None => eprintln!("shadow: the NAS isn't reachable; waiting"),
        }
        if o.once || super::stopping() || o.stop_after.is_some_and(|d| began.elapsed() >= d) {
            return Ok(());
        }
        for _ in 0..20 {
            if super::stopping() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
