//! The jobs' spans on the lead (task #1104; docs/formats.md, Job spans): each job run's timeline
//! (jobrt::spans), its log and its timings' record, kept by its id in the lead's coordinator's folder,
//! compressed, and served whole by `GET /api/job/<id>` (`view`) for the job inspector.
//!
//! - **In:** a member's beat carries its jobs' spans and logs in chunks (`Chunk`, `held::Beat::spans`)
//!   from where the lead's last answer said it has them (`Ack`), while the lead says it takes them
//!   (`beats::SPANS`); the lead's own agent hands its jobs' to `take` the same way
//!   (crate::agent::spans). A chunk is taken from where the lead has the job to: what it has again is
//!   passed over, so a beat posted twice adds nothing; one from past it (a lead that took over
//!   mid-job, its file lost) isn't taken, and the answer saying where the lead has it to, its agent
//!   sends it again from there (it keeps a job's files until the lead has all). A log's middle left
//!   out on purpose (crate::agent::spans::LOG_HEAD) is kept with the gap said. What the answer says
//!   the lead has moves only once it's written. The job's timings' record comes in as it's kept
//!   (`record`, crate::coord::keep_timings).
//! - **Kept:** `jobs/<epoch>/<n / 10000>/<epoch>-<n>.zst`, zstd frames appended one after another, a
//!   frame a chunk (`Stored` lines: the job's own lines as it wrote them, the lead's beside them);
//!   once the job's last chunk is in, the file is compressed again as one frame. Never removed, as
//!   the history isn't (docs/pool.md §7.5), but on this Mac alone: a later lead lacks them (planned:
//!   task #1172); a month's size is docs/formats.md's.
//! - **Out:** `view`: the job's facts, its record, its history's events, its spans in the dev jobs'
//!   shape (`{id, parent, kind, name, t0, t1, status, attrs}`, tools/devtools' run view), its waits,
//!   events and log lines on the same axis (unix s). A job with no file is from before spans, or ran
//!   on an app before them: `legacy`.

use jobrt::jobid::JobId;
use jobrt::spans::Line;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The folder in the coordinator's (agent/coord/jobs).
pub const DIR: &str = "jobs";
/// zstd's level for a chunk's frame, and for the whole file once the job ended.
const LEVEL: i32 = 3;
const LEVEL_DONE: i32 = 9;
/// The largest chunk taken (bytes of spans and log together); a beat carries less
/// (crate::agent::spans::BEAT_MAX).
pub const CHUNK_MAX: usize = 4 << 20;

/// The job as its agent knows it (`Chunk::meta`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// Its name ("unit 6/32/24"), step and targets.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    /// The worker that ran it (its Mac, `<Mac> #2` for its next slots) and its Mac.
    pub worker: String,
    pub host: String,
    /// When its agent started it (unix s), and its log's length then: the log's bytes in chunks are
    /// from there (its spans' log lengths are the file's, `Line::Log`).
    pub start: u64,
    pub log_from: u64,
    /// The app its agent ran (`…/app/<version>`, jobrt::sys::app_version); empty from an agent
    /// before task #1105.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app: String,
}

/// What a beat carries of one job (`held::Beat::spans`): its spans file from byte `s_at` (`s`: whole
/// lines), its log (this run's, from `Meta::log_from`) from byte `l_at`, and `end` when this is the
/// rest of both, the job ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub job: JobId,
    /// The job's facts: until the lead has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    #[serde(default)]
    pub s_at: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub s: String,
    #[serde(default)]
    pub l_at: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub l: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub end: bool,
}

/// What the lead has of a job (`held::Answer::spans`): its spans and log to these bytes, its facts,
/// and all of it (`done`: its agent drops its files).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    pub job: JobId,
    pub s: u64,
    pub l: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub meta: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
}

/// A line of the lead's file: the lead's own, or the job's (`Job`: jobrt::spans::Line as written).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Own {
    /// The job's facts, as its first chunk carried them.
    #[serde(rename = "job")]
    Facts(Meta),
    /// Its log's bytes from `at` (from `Meta::log_from`).
    #[serde(rename = "log")]
    Log { at: u64, text: String },
    /// Bytes of the log left out between two chunks (`of` "log": its middle, past
    /// crate::agent::spans::LOG_HEAD).
    #[serde(rename = "gap")]
    Gap { of: String, from: u64, to: u64 },
    /// Where the lead has it to, after the frame's chunk.
    #[serde(rename = "at")]
    At { s: u64, l: u64 },
    /// Its last chunk in (unix ms).
    #[serde(rename = "done")]
    Done { at: u64 },
    /// Its timings' record (jobrt::timings::RunRec).
    #[serde(rename = "rec")]
    Rec { rec: Box<jobrt::timings::RunRec> },
}

/// A line of the file read back.
#[derive(Clone, Debug, PartialEq)]
pub enum Stored {
    Own(Own),
    Job(Line),
}

fn read_line(l: &str) -> Option<Stored> {
    if let Ok(o) = serde_json::from_str::<Own>(l) {
        return Some(Stored::Own(o));
    }
    serde_json::from_str::<Line>(l).ok().map(Stored::Job)
}

/// What the lead has of a job, in memory once read.
#[derive(Clone, Copy, Debug, Default)]
struct Have {
    s: u64,
    l: u64,
    meta: bool,
    done: bool,
}

/// The lead's store of the jobs' spans.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    have: Mutex<HashMap<JobId, Have>>,
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Store {
    /// The store in `dir` (the coordinator's `jobs/`).
    pub fn new(dir: PathBuf) -> Store {
        Store { dir, have: Mutex::new(HashMap::new()) }
    }

    /// Job `job`'s file.
    pub fn path(&self, job: JobId) -> PathBuf {
        self.dir.join(job.epoch.to_string()).join((job.n / 10_000).to_string()).join(format!("{job}.zst"))
    }

    /// Job `job`'s lines (None: no file).
    pub fn read(&self, job: JobId) -> Option<Vec<Stored>> {
        let b = std::fs::read(self.path(job)).ok()?;
        Some(decode(&b).lines().filter_map(read_line).collect())
    }

    /// What the lead has of `job`: in memory, else read from its file (its last `At`).
    fn have(&self, job: JobId) -> Have {
        if let Some(h) = self.have.lock().unwrap().get(&job) {
            return *h;
        }
        let mut h = Have::default();
        for l in self.read(job).unwrap_or_default() {
            match l {
                Stored::Own(Own::At { s, l }) => (h.s, h.l) = (s, l),
                Stored::Own(Own::Facts(_)) => h.meta = true,
                Stored::Own(Own::Done { .. }) => h.done = true,
                _ => {}
            }
        }
        h
    }

    /// Chunks taken (a member's beat's, the lead's own agent's): each from where the lead has its
    /// job to, appended as a frame; what the lead has of each after.
    pub fn take(&self, chunks: Vec<Chunk>) -> Vec<Ack> {
        let mut acks = Vec::new();
        for c in chunks {
            let job = c.job;
            // (One chunk of a job at a time: two beats of one member at once, the lead's own and a
            // member's of a job taken over.)
            let _g = lock_of(job);
            let mut h = self.have(job);
            if !h.done {
                // (What it has moves only once the frame is written: an answer never says the lead
                // has what it failed to keep, so its agent sends it again.)
                let mut next = h;
                match self.append(&c, &mut next) {
                    Ok(()) => h = next,
                    Err(e) => eprintln!("coordinator: keeping job {job}'s spans: {e:#}"),
                }
            }
            self.have.lock().unwrap().insert(job, h);
            if h.done {
                // (Its agent drops its files on this answer: none more come.)
                self.have.lock().unwrap().remove(&job);
            }
            acks.push(Ack { job, s: h.s, l: h.l, meta: h.meta, done: h.done });
        }
        acks
    }

    fn append(&self, c: &Chunk, h: &mut Have) -> anyhow::Result<()> {
        let mut lines: Vec<String> = Vec::new();
        let own = |o: &Own| serde_json::to_string(o).unwrap_or_default();
        if let Some(m) = c.meta.as_ref().filter(|_| !h.meta) {
            lines.push(own(&Own::Facts(m.clone())));
            h.meta = true;
        }
        // (The spans from where the lead has them: a chunk's lines start where the lead's end. One
        // from past it (a lead that took over mid-job, its file lost) isn't taken: the answer says
        // where the lead has them to, and its agent, which keeps them until the lead has all, sends
        // them from there.)
        let s_end = c.s_at + c.s.len() as u64;
        let s_ahead = c.s_at > h.s;
        if s_end > h.s && !c.s.is_empty() && !s_ahead {
            let from = (h.s - c.s_at) as usize;
            let text = c.s.get(from..).unwrap_or("");
            lines.extend(text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string));
            h.s = s_end;
        }
        // (The log likewise, but the gap its agent leaves on purpose, past the head it sent:
        // crate::agent::spans::LOG_HEAD. A lead with none of it is sent it from its start.)
        let l_end = c.l_at + c.l.len() as u64;
        let l_ahead = c.l_at > h.l && h.l == 0;
        if l_end > h.l && !c.l.is_empty() && !l_ahead {
            if c.l_at > h.l {
                lines.push(own(&Own::Gap { of: "log".into(), from: h.l, to: c.l_at }));
            }
            let from = h.l.saturating_sub(c.l_at) as usize;
            // (Cut on a char's boundary: a chunk's text is whole, and the lead's offsets are its.)
            let text = c.l.get(from..).unwrap_or("");
            lines.push(own(&Own::Log { at: c.l_at.max(h.l), text: text.to_string() }));
            h.l = l_end;
        }
        if c.end && !s_ahead && !l_ahead {
            lines.push(own(&Own::Done { at: unix_ms() }));
            h.done = true;
        }
        if lines.is_empty() {
            return Ok(());
        }
        lines.push(own(&Own::At { s: h.s, l: h.l }));
        self.frame(c.job, &lines)?;
        if h.done {
            self.compact(c.job)?;
        }
        Ok(())
    }

    /// `lines` appended to `job`'s file, a zstd frame.
    fn frame(&self, job: JobId, lines: &[String]) -> anyhow::Result<()> {
        let p = self.path(job);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut text = lines.join("\n");
        text.push('\n');
        let z = zstd::encode_all(text.as_bytes(), LEVEL)?;
        std::fs::OpenOptions::new().create(true).append(true).open(&p)?.write_all(&z)?;
        Ok(())
    }

    /// `job`'s file as one frame (its job ended: a better ratio than a frame a chunk).
    fn compact(&self, job: JobId) -> anyhow::Result<()> {
        let p = self.path(job);
        let text = decode(&std::fs::read(&p)?);
        let z = zstd::encode_all(text.as_bytes(), LEVEL_DONE)?;
        let tmp = p.with_extension("zst.tmp");
        std::fs::write(&tmp, &z)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    /// A job's timings' record kept beside its spans (none for a record of no job).
    pub fn record(&self, rec: &jobrt::timings::RunRec) {
        let Some(job) = rec.job else { return };
        let _g = lock_of(job);
        let line = serde_json::to_string(&Own::Rec { rec: Box::new(rec.clone()) }).unwrap_or_default();
        if let Err(e) = self.frame(job, &[line]) {
            eprintln!("coordinator: keeping job {job}'s record beside its spans: {e:#}");
        }
    }
}

/// A job's lock while a chunk of it is taken (a few jobs' at once at most: one per stripe).
fn lock_of(job: JobId) -> std::sync::MutexGuard<'static, ()> {
    static STRIPES: [Mutex<()>; 16] = [const { Mutex::new(()) }; 16];
    STRIPES[(job.n % 16) as usize].lock().unwrap_or_else(|e| e.into_inner())
}

/// A file's frames decoded, one after another (a frame that doesn't decode, a write cut short by a
/// crash, ends the text there).
fn decode(b: &[u8]) -> String {
    let mut out = Vec::new();
    if let Ok(mut d) = zstd::stream::read::Decoder::new(b) {
        let _ = std::io::copy(&mut d, &mut out);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Microseconds from a job's start as unix seconds.
fn at(start_us: u64, t: u64) -> f64 {
    ((start_us + t) / 1000) as f64 / 1000.0
}

/// A log line, its time (unix s) where its job's spans said the log's length about then.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LogLine {
    pub t: Option<f64>,
    pub text: String,
}

/// Job `job`'s view (`GET /api/job/<id>`, docs/formats.md, Job spans: the API): from its file, its
/// timings' record in it or the one `record` finds, its history's events `history`.
pub fn view(
    store: Option<&Store>,
    job: JobId,
    history: Vec<super::history::Event>,
    record: impl FnOnce() -> Option<jobrt::timings::RunRec>,
) -> serde_json::Value {
    let lines = store.and_then(|s| s.read(job));
    let Some(lines) = lines else {
        return serde_json::json!({
            "job": job, "legacy": true, "record": record(), "history": history,
            "now": unix_ms() as f64 / 1000.0,
        });
    };
    let mut meta: Option<Meta> = None;
    let mut rec: Option<jobrt::timings::RunRec> = None;
    let mut start_us: Option<u64> = None;
    let mut done_at: Option<u64> = None;
    let mut spans: Vec<serde_json::Value> = Vec::new();
    let mut open: HashMap<String, serde_json::Value> = HashMap::new();
    let mut closed: std::collections::HashSet<String> = Default::default();
    let mut waits = Vec::new();
    let mut events = Vec::new();
    let mut marks: Vec<(u64, u64)> = Vec::new();
    let mut log: Vec<(u64, String)> = Vec::new();
    let mut gaps = Vec::new();
    let mut last_t = 0u64;
    let mut kind = String::new();
    let (mut exit_ok, mut end_ok) = (None, None);
    // (The job's start first: every time is from it.)
    for l in &lines {
        if let Stored::Job(Line::Head { start_us: s, parent: None, kind: k, .. }) = l {
            if start_us.is_none() {
                start_us = Some(*s);
                kind = k.clone();
            }
        }
        if let Stored::Own(Own::Facts(m)) = l {
            meta.get_or_insert(m.clone());
        }
    }
    let start_us = start_us.or(meta.as_ref().map(|m| m.start * 1_000_000)).unwrap_or(0);
    let t = |x: u64| at(start_us, x);
    for l in lines {
        match l {
            Stored::Own(Own::Rec { rec: r }) => rec = Some(*r),
            Stored::Own(Own::Done { at }) => done_at = Some(at),
            Stored::Own(Own::Log { at, text }) => log.push((at, text)),
            Stored::Own(Own::Gap { of, from, to }) => {
                gaps.push(serde_json::json!({ "of": of, "from": from, "to": to }))
            }
            Stored::Own(_) => {}
            Stored::Job(Line::Open { id, parent, th, name, t0 }) => {
                last_t = last_t.max(t0);
                open.insert(
                    id.clone(),
                    serde_json::json!({ "id": id, "parent": parent.unwrap_or_else(|| "job".into()), "kind": lane_kind(&th), "name": name, "t0": t(t0), "t1": null, "status": "running", "attrs": { "th": th } }),
                );
            }
            Stored::Job(Line::Span { id, parent, th, name, t0, t1, n, busy, cpu, w, b, f, cut }) => {
                last_t = last_t.max(t1);
                let s = |us: u64| us as f64 / 1e6;
                let mut a = serde_json::Map::new();
                if !th.is_empty() {
                    a.insert("th".into(), th.clone().into());
                }
                if n > 1 {
                    a.insert("n".into(), n.into());
                    a.insert("busy_s".into(), s(busy.unwrap_or(t1 - t0)).into());
                }
                if let Some(c) = cpu {
                    a.insert("cpu_s".into(), s(c).into());
                }
                for (k, v) in ["nas_s", "net_s", "disk_s", "other_s"].iter().zip(w) {
                    if v > 0 {
                        a.insert((*k).into(), s(v).into());
                    }
                }
                if b > 0 {
                    a.insert("bytes".into(), b.into());
                }
                if f > 0 {
                    a.insert("files".into(), f.into());
                }
                open.remove(&id);
                closed.insert(id.clone());
                spans.push(serde_json::json!({ "id": id, "parent": parent.unwrap_or_else(|| "job".into()), "kind": lane_kind(&th), "name": name, "t0": t(t0), "t1": t(t1), "status": if cut { "cut" } else { "ok" }, "attrs": a }));
            }
            Stored::Job(Line::Wait { on, t0, t1, n, busy }) => {
                last_t = last_t.max(t1);
                waits.push(
                    serde_json::json!({ "on": on, "t0": t(t0), "t1": t(t1), "n": n, "busy_s": busy as f64 / 1e6 }),
                );
            }
            Stored::Job(Line::Event(e)) => {
                // (How it ended: its agent's word, else the last end said, the job's own after
                // its child programs'.)
                match (e.e.as_str(), e.ok) {
                    ("exit", Some(ok)) => exit_ok = Some(ok),
                    ("end", Some(ok)) => end_ok = Some(ok),
                    _ => {}
                }
                let when = e.t.map(t).or(e.at.map(|ms| ms as f64 / 1000.0));
                if let Some(x) = e.t {
                    last_t = last_t.max(e.t1.unwrap_or(x));
                }
                events.push(serde_json::json!({ "kind": e.e, "t": when, "t1": e.t1.map(t), "n": e.n, "ok": e.ok, "text": e.text }));
            }
            Stored::Job(Line::Log { t: x, at }) => {
                last_t = last_t.max(x);
                marks.push((at, x));
            }
            Stored::Job(_) => {}
        }
    }
    let running = done_at.is_none();
    // (A span said open and never closed: still running, or cut where the job's word ended.)
    for (id, mut v) in open {
        if closed.contains(&id) {
            continue;
        }
        if !running {
            v["status"] = "cut".into();
            v["t1"] = t(last_t).into();
        }
        spans.push(v);
    }
    spans.sort_by(|a, b| a["t0"].as_f64().partial_cmp(&b["t0"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    let end = (!running).then(|| t(last_t));
    let m = meta.clone().unwrap_or_default();
    let root = serde_json::json!({ "id": "job", "parent": null, "kind": "job", "name": if m.name.is_empty() { kind.clone() } else { m.name.clone() }, "t0": t(0), "t1": end, "status": match (running, exit_ok.or(end_ok)) { (true, _) => "running", (false, Some(false)) => "fail", _ => "ok" }, "attrs": {} });
    spans.insert(0, root);
    serde_json::json!({
        "job": job,
        "legacy": false,
        "running": running,
        "now": unix_ms() as f64 / 1000.0,
        "start": t(0),
        "end": end,
        "kind": kind,
        "name": m.name, "step": m.step, "targets": m.targets, "worker": m.worker, "host": m.host,
        "app": (!m.app.is_empty()).then_some(m.app),
        "record": rec.or_else(record),
        "history": history,
        "spans": spans,
        "waits": waits,
        "events": events,
        "log": log_lines(&log, m.log_from, &marks, start_us),
        "gaps": gaps,
    })
}

/// The most of a job's other runs `relate` names, and of the jobs beside it.
pub const RUNS_MAX: usize = 50;
pub const BESIDE_MAX: usize = 200;
/// The targets a run of `relate`'s names (`Run::n` counts them all).
const TARGETS_SHOWN: usize = 3;

/// Another job run as the history has it (`relate`): its id, step, targets (the first
/// `TARGETS_SHOWN`; `n` all of them), worker, from and until (unix s; `until` None: no end in the
/// history, running) and how it ended.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Run {
    pub job: JobId,
    pub step: String,
    pub targets: Vec<String>,
    pub n: usize,
    pub worker: String,
    pub from: f64,
    pub until: Option<f64>,
    pub ok: Option<bool>,
}

/// Every job run the history's events `all` name, by id, with all its targets: its step, targets
/// and worker (its start's, which names the slot, else its lease's), its start (an end's time less
/// its `secs`, else its start's, else its lease's) and its end (an `end`, `done`, `fail` or
/// `cancel`).
fn runs_of(all: &[super::history::Event]) -> HashMap<JobId, (Run, Vec<String>)> {
    struct Acc {
        run: Run,
        targets: Vec<String>,
        lease: Option<u64>,
        start: Option<u64>,
        timed: bool,
    }
    let mut out: HashMap<JobId, Acc> = HashMap::new();
    for e in all {
        let Some(j) = e.job else { continue };
        let end = matches!(e.kind.as_str(), "end" | "done" | "fail" | "cancel");
        if !end && !matches!(e.kind.as_str(), "lease" | "start") {
            continue;
        }
        let a = out.entry(j).or_insert_with(|| Acc {
            run: Run {
                job: j,
                step: String::new(),
                targets: Vec::new(),
                n: 0,
                worker: String::new(),
                from: e.t as f64,
                until: None,
                ok: None,
            },
            targets: Vec::new(),
            lease: None,
            start: None,
            timed: false,
        });
        if let Some(st) = &e.step {
            a.run.step.clone_from(st);
        }
        if !e.targets.is_empty() {
            a.targets.clone_from(&e.targets);
        }
        if let Some(w) = &e.worker {
            if a.run.worker.is_empty() || e.kind == "start" {
                a.run.worker.clone_from(w);
            }
        }
        match e.kind.as_str() {
            "lease" => a.lease = Some(a.lease.map_or(e.t, |t| t.min(e.t))),
            "start" => a.start = Some(a.start.map_or(e.t, |t| t.min(e.t))),
            _ => {
                a.run.until = Some(e.t as f64);
                a.run.ok = Some(e.ok.unwrap_or(!matches!(e.kind.as_str(), "fail" | "cancel")));
                if let Some(secs) = e.secs {
                    a.run.from = e.t as f64 - secs;
                    a.timed = true;
                }
            }
        }
    }
    out.into_iter()
        .map(|(j, mut a)| {
            if let (false, Some(t)) = (a.timed, a.start.or(a.lease)) {
                a.run.from = t as f64;
            }
            a.run.n = a.targets.len();
            a.run.targets = a.targets.iter().take(TARGETS_SHOWN).cloned().collect();
            (j, (a.run, a.targets))
        })
        .collect()
}

/// Job `job`'s view `v` (`view`'s) given the history's other runs around it, from its events `all`
/// (the history's seven days): `runs`, those of its step whose targets meet its own (a step of no
/// targets: all its runs), the newest `RUNS_MAX` (`runs_total` all of them); `beside`, those on its
/// Mac while it ran (from its start to its end, or `now`), the first `BESIDE_MAX` by start. Its own
/// step, targets, Mac and times are its view's, else its own events' (a legacy job's).
pub fn relate(v: &mut serde_json::Value, all: &[super::history::Event], job: JobId, now: f64) {
    let mut runs = runs_of(all);
    let mine = runs.remove(&job);
    let said = |k: &str| v[k].as_str().filter(|x| !x.is_empty()).map(str::to_string);
    let step = said("step").or(mine.as_ref().map(|m| m.0.step.clone())).unwrap_or_default();
    let targets: Vec<String> = match v["targets"].as_array().filter(|a| !a.is_empty()) {
        Some(a) => a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect(),
        None => mine.as_ref().map(|m| m.1.clone()).unwrap_or_default(),
    };
    let host =
        said("host").or(mine.as_ref().map(|m| crate::agent::host_of(&m.0.worker).to_string())).unwrap_or_default();
    let t0 = v["start"].as_f64().or(mine.as_ref().map(|m| m.0.from));
    let t1 = match v["legacy"].as_bool() {
        Some(false) => v["end"].as_f64().unwrap_or(now),
        _ => mine.as_ref().and_then(|m| m.0.until).unwrap_or(now),
    };
    let meets = |r: &Run, ts: &[String]| {
        !step.is_empty() && r.step == step && (targets.is_empty() || ts.iter().any(|t| targets.contains(t)))
    };
    let mut same: Vec<Run> = runs.values().filter(|(r, ts)| meets(r, ts)).map(|(r, _)| r.clone()).collect();
    same.sort_by(|a, b| b.from.total_cmp(&a.from));
    let total = same.len();
    same.truncate(RUNS_MAX);
    let mut beside: Vec<Run> = match t0 {
        Some(t0) if !host.is_empty() => runs
            .into_values()
            .map(|(r, _)| r)
            .filter(|r| crate::agent::host_of(&r.worker) == host && r.from < t1 && r.until.unwrap_or(now) > t0)
            .collect(),
        _ => Vec::new(),
    };
    beside.sort_by(|a, b| a.from.total_cmp(&b.from));
    beside.truncate(BESIDE_MAX);
    v["runs"] = serde_json::json!(same);
    v["runs_total"] = total.into();
    v["beside"] = serde_json::json!(beside);
}

/// A span's kind by its lane: the main thread's spans are phases, another thread's work beside it.
fn lane_kind(th: &str) -> &'static str {
    if th.is_empty() {
        "phase"
    } else {
        "beside"
    }
}

/// The log's lines (`log`: its pieces from `at`, from `log_from` in the file), each at the time the
/// log first reached its end by the job's marks (`marks`: the file's length, at µs from the job's
/// start), placed between the marks either side by its bytes.
fn log_lines(log: &[(u64, String)], log_from: u64, marks: &[(u64, u64)], start_us: u64) -> serde_json::Value {
    let mut marks = marks.to_vec();
    marks.sort();
    let mut out = Vec::new();
    let mut bytes = 0u64;
    for (from, text) in log {
        let mut pos = log_from + from;
        for line in text.split_inclusive('\n') {
            pos += line.len() as u64;
            bytes += line.len() as u64;
            // (The first mark at or past its end, and the one before: its time between theirs.)
            let i = marks.partition_point(|m| m.0 < pos);
            let tt = match (i.checked_sub(1).map(|j| marks[j]), marks.get(i)) {
                (Some(a), Some(b)) if b.0 > a.0 => {
                    Some(a.1 + ((b.1 - a.1) as f64 * (pos - a.0) as f64 / (b.0 - a.0) as f64) as u64)
                }
                (_, Some(b)) => Some(b.1),
                _ => None,
            };
            out.push(LogLine { t: tt.map(|x| at(start_us, x)), text: line.trim_end_matches('\n').to_string() });
        }
    }
    serde_json::json!({ "lines": out, "bytes": bytes })
}

/// `GET /api/job/<id>`'s route: `of` answers a job's view (None: no store here, 503).
pub type ViewOf = std::sync::Arc<dyn Fn(JobId) -> Option<serde_json::Value> + Send + Sync>;

/// The route, for a router of any state (the coordinator merges it behind its gate).
pub fn router<S: Clone + Send + Sync + 'static>(of: ViewOf) -> axum::Router<S> {
    axum::Router::new().route("/api/job/{id}", axum::routing::get(answer)).with_state(of)
}

async fn answer(
    axum::extract::State(of): axum::extract::State<ViewOf>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;
    let json = |code: StatusCode, v: serde_json::Value| {
        (code, [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], v.to_string())
            .into_response()
    };
    let Ok(job) = id.parse::<JobId>() else {
        return json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": format!("{id:?} isn't a job id: <epoch>-<n>") }),
        );
    };
    // (Its file read off the runtime's threads.)
    match tokio::task::spawn_blocking(move || of(job)).await {
        Ok(Some(v)) => json(StatusCode::OK, v),
        Ok(None) => {
            json(StatusCode::SERVICE_UNAVAILABLE, serde_json::json!({ "error": "no jobs' spans here: not the lead" }))
        }
        Err(e) => json(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "error": e.to_string() })),
    }
}

/// The record of `job` in a timings log (the coordinator's `timings.jsonl`), the last.
pub fn record_in(log: &Path, job: JobId) -> Option<jobrt::timings::RunRec> {
    jobrt::timings::read_log(log).into_iter().rev().find(|r| r.job == Some(job))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(job: JobId, s_at: u64, s: &str, l_at: u64, l: &str, end: bool) -> Chunk {
        Chunk { job, meta: Some(Meta::default()), s_at, s: s.into(), l_at, l: l.into(), end }
    }

    /// A chunk from past what the lead has (a lead taking over mid-job: its agent's last answer was
    /// the old lead's) isn't taken, its end neither: the answer says where the lead has it to, so
    /// its agent sends it from there, and the lead ends with the whole of it, no gap. A log's gap
    /// past a head the lead has is kept, said. A frame that can't be written moves nothing: the
    /// answer never says the lead has what it failed to keep.
    #[test]
    fn a_chunk_past_what_the_lead_has_is_sent_again_from_there_and_a_failed_write_acks_nothing() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path().join("jobs"));
        let job = JobId { epoch: 2, n: 5 };
        let (a, b) = ("{\"k\":\"l\",\"t\":0,\"at\":0}\n", "{\"k\":\"l\",\"t\":1,\"at\":9}\n");
        let ack = store.take(vec![chunk(job, a.len() as u64, b, 40, "tail\n", true)]);
        assert_eq!((ack[0].s, ack[0].l, ack[0].done), (0, 0, false), "{ack:?}");
        assert!(store.read(job).is_none_or(|l| !l.iter().any(|x| matches!(x, Stored::Job(_)))));
        let ack = store.take(vec![chunk(job, 0, &format!("{a}{b}"), 0, "head\n", false)]);
        assert_eq!((ack[0].s, ack[0].l), ((a.len() + b.len()) as u64, 5));
        let ack = store.take(vec![chunk(job, ack[0].s, "", 40, "tail\n", true)]);
        assert!(ack[0].done, "{ack:?}");
        let v = view(Some(&store), job, Vec::new(), || None);
        assert_eq!(v["gaps"], serde_json::json!([{ "of": "log", "from": 5, "to": 40 }]));
        assert_eq!(v["log"]["lines"].as_array().unwrap().len(), 2);
        // (Its folder a file: nothing can be written.)
        let bad = Store::new(d.path().join("blocked"));
        std::fs::write(d.path().join("blocked"), "").unwrap();
        let ack = bad.take(vec![chunk(job, 0, a, 0, "head\n", true)]);
        assert_eq!((ack[0].s, ack[0].l, ack[0].meta, ack[0].done), (0, 0, false, false));
    }

    /// Task #1105: a job's other runs around it, from the history: those of its step meeting its
    /// targets, newest first, timed by their end less its `secs` (else their start, else their
    /// lease); those on its Mac (any slot) while it ran, by start, one with no end yet running; not
    /// its own, nor another Mac's, nor one before or after it. A legacy job's from its own events.
    #[test]
    fn a_job_s_other_runs_and_the_jobs_beside_it_come_from_the_history() {
        use super::super::history::Event;
        let ev = |kind: &str, t: u64, job: &str, step: &str, worker: &str, targets: &[&str]| Event {
            kind: kind.into(),
            t,
            job: Some(job.parse().unwrap()),
            step: Some(step.into()),
            worker: Some(worker.into()),
            targets: targets.iter().map(|x| x.to_string()).collect(),
            ..Default::default()
        };
        let done = |t: u64, job: &str, step: &str, worker: &str, targets: &[&str], secs: f64, ok: bool| Event {
            secs: Some(secs),
            ok: Some(ok),
            ..ev("done", t, job, step, worker, targets)
        };
        let all = vec![
            done(100, "1-1", "unit", "m1", &["6/1/1"], 50.0, false),
            ev("lease", 1000, "1-2", "unit", "m1", &["6/1/1"]),
            ev("start", 1001, "1-2", "unit", "m1 #2", &["6/1/1"]),
            ev("lease", 1010, "1-3", "slope", "m1", &["10/1/1"]),
            done(1500, "1-3", "slope", "m1", &["10/1/1"], 480.0, true),
            ev("start", 1021, "1-4", "unit", "m1 #3", &["6/1/2", "6/1/1"]),
            ev("lease", 1030, "1-5", "slope", "m2", &["10/1/2"]),
            done(2500, "1-6", "slope", "m1", &["10/1/3"], 10.0, true),
            done(990, "1-7", "slope", "m1", &["10/1/4"], 20.0, true),
            done(2000, "1-2", "unit", "m1", &["6/1/1"], 1000.0, true),
        ];
        let job: JobId = "1-2".parse().unwrap();
        let mut v = serde_json::json!({ "legacy": false, "start": 1000.0, "end": 2000.0, "step": "unit", "targets": ["6/1/1"], "host": "m1" });
        relate(&mut v, &all, job, 3000.0);
        let ids = |k: &str| {
            v[k].as_array().unwrap().iter().map(|r| r["job"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        };
        assert_eq!(ids("runs"), ["1-4", "1-1"], "its step's runs meeting its targets, newest first");
        assert_eq!(v["runs_total"], 2);
        assert_eq!((v["runs"][1]["from"].as_f64(), v["runs"][1]["ok"].as_bool()), (Some(50.0), Some(false)));
        assert_eq!((v["runs"][0]["until"].clone(), v["runs"][0]["n"].as_u64()), (serde_json::Value::Null, Some(2)));
        assert_eq!(
            ids("beside"),
            ["1-3", "1-4"],
            "on its Mac while it ran; another Mac's, and before or after it, not"
        );
        assert_eq!(v["beside"][0]["from"].as_f64(), Some(1020.0));
        // (A legacy job: its step, targets, Mac and times its own events'.)
        let mut legacy = serde_json::json!({ "legacy": true });
        relate(&mut legacy, &all, job, 3000.0);
        assert_eq!((legacy["runs_total"].as_u64(), legacy["beside"].as_array().map(Vec::len)), (Some(2), Some(2)));
    }

    /// A benchmark, run by hand (`-- --ignored`): each job's spans file and log in
    /// `SCENIC_SPANS_MEASURE` (`<dir>/<tag>/spans.ndjson` and `<dir>/<tag>/log`, as a measuring run
    /// leaves them) sent as a member sends them, in beats' chunks, and kept as the lead keeps them:
    /// each one's size raw and on the lead.
    #[test]
    #[ignore]
    fn sizes_on_the_lead() {
        let Some(dir) = std::env::var_os("SCENIC_SPANS_MEASURE") else { return };
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path().join("jobs"));
        for (i, e) in std::fs::read_dir(&dir).unwrap().flatten().enumerate() {
            let (s, l) = (e.path().join("spans.ndjson"), e.path().join("log"));
            let Ok(spans) = std::fs::read_to_string(&s) else { continue };
            let log = std::fs::read_to_string(&l).unwrap_or_default();
            let job = JobId { epoch: 1, n: i as u64 + 1 };
            let max = crate::agent::spans::BEAT_MAX;
            let (mut sa, mut la) = (0usize, 0usize);
            while sa < spans.len() || la < log.len() {
                let s1 = (sa + max).min(spans.len());
                let s1 = if s1 < spans.len() { spans[..s1].rfind('\n').map_or(sa, |k| k + 1) } else { s1 };
                let l1 = (la + max).min(log.len());
                let l1 = if l1 < log.len() { log[..l1].rfind('\n').map_or(la, |k| k + 1) } else { l1 };
                let end = s1 == spans.len() && l1 == log.len();
                let meta = (sa == 0).then(Meta::default);
                store.take(vec![Chunk {
                    job,
                    meta,
                    s_at: sa as u64,
                    s: spans[sa..s1].to_string(),
                    l_at: la as u64,
                    l: log[la..l1].to_string(),
                    end,
                }]);
                (sa, la) = (s1, l1);
            }
            let kept = std::fs::metadata(store.path(job)).map_or(0, |m| m.len());
            eprintln!(
                "sizes: {}: {} lines, spans {} B, log {} B, on the lead {} B",
                e.file_name().to_string_lossy(),
                spans.lines().count(),
                spans.len(),
                log.len(),
                kept
            );
        }
    }
}
