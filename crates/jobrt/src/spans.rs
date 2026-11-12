//! A job's spans (task #1104; the format: docs/formats.md, Job spans): its run as a timeline, every
//! phase and sub-phase (crate::timings) a span with its start and end, the waits as intervals, and
//! what happened (its retries, safe points, a pause, its end), written as the job runs to the file
//! `SCENIC_SPANS` names, a JSON line each (`Line`). Its agent sends the file on to the lead with its
//! beats (agentcore::agent::spans), which keeps it by the job's id (agentcore::coord::jobspans).
//!
//! - **Spans:** each phase's span as it closes, on the thread it ran on (`th`: the main thread's
//!   none, another thread's its number in the order they first opened one, so work beside the main
//!   thread is a lane of its own), its parent the span it opened inside, with its CPU, waits, bytes
//!   and files. A span open a second or more is said open (`Line::Open`) at the next flush, so a
//!   job that dies leaves its long phases. A child program (crate::timings::child) writes its own to
//!   the same file, its ids under a prefix of its own (`3.`), its top-level spans inside the span
//!   that ran it, its times from the job's start.
//! - **Waits:** store::waits' intervals, each kind's (`Line::Wait`).
//! - **Events:** a retry (store::retry::log), safe points (crate::control::reached, merged into runs a
//!   second apart), the ask to pause (crate::control::draining), a pause or an error ending the job,
//!   its end (`Line::Event`); its log's length as it grows (`Line::Log`), so the lead can put the
//!   log's lines on the same axis.
//! - **Granularity, bounded** (measured: docs/formats.md, Job spans): spans shorter than a threshold
//!   (`MIN_OWN0_NS` at first) with no sub-spans, of one name under one parent on one thread, one
//!   after another within that threshold, are one span (`n` of them, `busy` their time together);
//!   waits of a kind within a gap (`WAIT_GAP0_NS`) likewise (store::waits). Each `STEP` lines written
//!   doubles both, and past `LINES_MAX` only the main thread's top-level spans and the events are
//!   written (an event says so): a job's file stays within a few MB whatever it does.
//! - **Cost:** a span's close formats nothing; the lines are made and written once a second, from
//!   the memory sampler's thread (crate::timings), in one append, outside the run's lock. Measured
//!   with the timings' own (`RunRec::overhead_s`), and on its own (`Rec::cost_ns`, said at the end).
//!   Recording never stops a job: a file that can't be written loses its lines, said once.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// Where a job writes its spans (a file it appends to; its agent sets it).
pub const SPANS_ENV: &str = "SCENIC_SPANS";
/// What a child program's spans are inside (set by crate::timings::child): `<parent span id>\t<its
/// ids' prefix>\t<the job's start, unix µs>\t<its main thread's lane>`.
pub const SPANS_IN_ENV: &str = "SCENIC_SPANS_IN";
/// The format's version (`Line::Head`).
pub const VERSION: u32 = 1;

/// Spans shorter than this (ns) with no sub-spans merge with their neighbours of the same name, at
/// first; doubled each `STEP` lines.
pub const MIN_OWN0_NS: u64 = 1_000_000;
/// Waits of a kind this close (ns) are one interval, at first; doubled each `STEP` lines.
pub const WAIT_GAP0_NS: u64 = 2_000_000;
/// Lines between two doublings of the thresholds.
pub const STEP: u64 = 2_000;
/// Past this many lines only the main thread's top-level spans and the events are written.
pub const LINES_MAX: u64 = 40_000;
/// The most spans waiting to merge at once (of different names or parents): past it, all written.
const PEND_MAX: usize = 1024;
/// Safe points this close (µs) are one event.
const SAFE_GAP_US: u64 = 1_000_000;
/// A span open this long is said open at the next flush (crate::timings).
pub const OPEN_SAID: std::time::Duration = std::time::Duration::from_secs(1);
/// The longest text an event keeps (chars).
pub const TEXT_MAX: usize = 500;

/// A line of a job's spans file, as written and as read (by its agent and the lead). Times are µs
/// from the job's start (`Head::start_us`, unix µs), but `Event::at` (unix ms, said by its agent). A
/// kind this reader doesn't know reads as `Other`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Line {
    /// The job's (a child's) start: the format's version, when (unix µs: the job's, a child's
    /// parent job's), its step (crate::timings::start's kind), its process, and the span it's
    /// inside (a child's).
    #[serde(rename = "h")]
    Head {
        v: u32,
        start_us: u64,
        kind: String,
        pid: u32,
        #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    /// A span open since `t0` (said once it's been open a second); its `Span` line replaces it.
    #[serde(rename = "o")]
    Open {
        id: String,
        #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        th: String,
        name: String,
        t0: u64,
    },
    /// A span (`n` merged: from the first's start to the last's end, `busy` their time together),
    /// its own CPU (µs), its waits while open (µs: NAS, network, disk, other work; the job's,
    /// crate::timings::Waits), bytes and files; `cut` when the job ended with it open.
    #[serde(rename = "s")]
    Span {
        id: String,
        #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        th: String,
        name: String,
        t0: u64,
        t1: u64,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        busy: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cpu: Option<u64>,
        #[serde(default, skip_serializing_if = "no_waits")]
        w: [u64; 4],
        #[serde(default, skip_serializing_if = "is_zero")]
        b: u64,
        #[serde(default, skip_serializing_if = "is_zero")]
        f: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cut: bool,
    },
    /// Waits on `on` ("nas", "net", "disk", "other"): `n` of them from `t0` to `t1`, `busy` µs of it.
    #[serde(rename = "w")]
    Wait { on: String, t0: u64, t1: u64, n: u32, busy: u64 },
    /// What happened: `e` "retry", "safe" (`n` safe points from `t` to `t1`), "pausing" (asked to
    /// stop at its next safe point), "paused", "error", "thinned", "end" (`ok`), "exit" (its agent's
    /// word on how it ended, `at` unix ms), with its text.
    #[serde(rename = "e")]
    Event(Ev),
    /// The job's log was `at` bytes long at `t` (the log as its agent opened it for this run).
    #[serde(rename = "l")]
    Log { t: u64, at: u64 },
    #[serde(other)]
    Other,
}

fn one() -> u64 {
    1
}
fn is_one(n: &u64) -> bool {
    *n == 1
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}
fn no_waits(w: &[u64; 4]) -> bool {
    w.iter().all(|&x| x == 0)
}

/// An event (`Line::Event`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ev {
    pub e: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t1: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

/// An event `e`, now-less (`t` set by the recorder), its text cut to `TEXT_MAX`.
pub fn event(e: &str, text: &str) -> Ev {
    Ev { e: e.to_string(), text: text.chars().take(TEXT_MAX).collect(), ..Default::default() }
}

/// A span closed, as crate::timings hands it over.
pub(crate) struct Closed<'a> {
    pub id: u64,
    pub parent: Option<u64>,
    pub th: u32,
    pub name: &'a str,
    pub t0: Instant,
    pub t1: Instant,
    pub cpu_us: Option<u64>,
    /// The waits while it was open (ns: NAS, network, disk, other).
    pub waits_ns: [u64; 4],
    pub bytes: u64,
    pub files: u64,
    /// It had sub-spans (never merged).
    pub kids: bool,
    pub cut: bool,
}

/// A span waiting to merge with the next of its name (`Rec::pend`).
struct Pend {
    id: u64,
    t0: u64,
    t1: u64,
    busy: u64,
    n: u64,
    cpu: Option<u64>,
    w: [u64; 4],
    b: u64,
    f: u64,
}

/// A job's (a child's) recorder, kept in crate::timings' run (its lock).
pub(crate) struct Rec {
    file: Option<std::sync::Arc<std::fs::File>>,
    /// The run's start: times are from it, plus `off_us` (a child's: its start less its job's).
    t0: Instant,
    off_us: u64,
    start_us: u64,
    /// Its ids' prefix (a child's), and the span its top-level spans are inside.
    prefix: String,
    parent: Option<String>,
    /// The lane its main thread's spans are on (a child's: its parent span's).
    main_th: String,
    /// The threads but the main one, in the order they first opened a span.
    lanes: Vec<std::thread::ThreadId>,
    next: u64,
    children: u64,
    /// Spans waiting to merge, by (parent, lane, name).
    pend: HashMap<(Option<u64>, u32, String), Pend>,
    /// The safe points' run being merged: its first, its last, how many.
    safe: Option<(u64, u64, u64)>,
    /// Lines made, not yet written.
    lines: Vec<Line>,
    /// Lines made so far, and the thresholds now (ns).
    made: u64,
    min_own: u64,
    wait_gap: u64,
    thinned: bool,
    /// The log's length as last said (the job's own, not a child's: its stderr).
    log_at: Option<u64>,
    lost: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The recorder's own time (ns): its closes, ticks and writes.
    pub cost_ns: u64,
}

fn unix_us() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64)
}

/// The length of the file on this process's stderr (its log), where it's a file.
fn log_len() -> Option<u64> {
    // SAFETY: fstat fills the struct it's given.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(2, &mut st) } == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG)
        .then_some(st.st_size as u64)
}

impl Rec {
    /// The recorder `SCENIC_SPANS` asks for (None: none asked, or its file can't be opened), for
    /// the run of `kind` started at `t0`.
    pub fn from_env(kind: &str, t0: Instant) -> Option<Rec> {
        let path = std::env::var_os(SPANS_ENV)?;
        Rec::to_file(Path::new(&path), kind, t0, std::env::var(SPANS_IN_ENV).ok().as_deref())
    }

    /// The recorder writing to `path`, within what `within` says (`SPANS_IN_ENV`: a child's).
    pub fn to_file(path: &Path, kind: &str, t0: Instant, within: Option<&str>) -> Option<Rec> {
        let file = match std::fs::OpenOptions::new().create(true).append(true).open(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("spans: not kept ({}: {e})", path.display());
                return None;
            }
        };
        let now_us = unix_us();
        let (parent, prefix, start_us, main_th) = match within {
            Some(v) => {
                let mut f = v.split('\t');
                let parent = f.next().filter(|p| !p.is_empty()).map(str::to_string);
                let prefix = f.next().unwrap_or("").to_string();
                let start = f.next().and_then(|s| s.parse().ok()).unwrap_or(now_us);
                (parent, prefix, start, f.next().unwrap_or("").to_string())
            }
            None => (None, String::new(), now_us, String::new()),
        };
        let child = parent.is_some() || !prefix.is_empty();
        let mut r = Rec {
            file: Some(std::sync::Arc::new(file)),
            t0,
            off_us: now_us.saturating_sub(start_us),
            start_us,
            prefix,
            parent: parent.clone(),
            main_th,
            lanes: Vec::new(),
            next: 0,
            children: 0,
            pend: HashMap::new(),
            safe: None,
            lines: Vec::new(),
            made: 0,
            min_own: MIN_OWN0_NS,
            wait_gap: WAIT_GAP0_NS,
            thinned: false,
            log_at: None,
            lost: Default::default(),
            cost_ns: 0,
        };
        r.push(Line::Head { v: VERSION, start_us, kind: kind.to_string(), pid: std::process::id(), parent });
        // (The job's own log; a child writes to the same.)
        if !child {
            r.log_at = log_len();
            if let Some(at) = r.log_at {
                r.push(Line::Log { t: 0, at });
            }
        }
        store::waits::intervals_from(t0, WAIT_GAP0_NS);
        Some(r)
    }

    /// µs from the job's start at `t`.
    fn us(&self, t: Instant) -> u64 {
        t.saturating_duration_since(self.t0).as_micros() as u64 + self.off_us
    }

    fn id(&self, n: u64) -> String {
        format!("{}{n}", self.prefix)
    }

    /// A new span's number.
    pub fn open_id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Thread `id`'s lane (0: the main thread's).
    pub fn lane(&mut self, id: std::thread::ThreadId, main: bool) -> u32 {
        if main {
            return 0;
        }
        match self.lanes.iter().position(|&t| t == id) {
            Some(i) => i as u32 + 1,
            None => {
                self.lanes.push(id);
                self.lanes.len() as u32
            }
        }
    }

    fn th(&self, lane: u32) -> String {
        match lane {
            0 => self.main_th.clone(),
            n => format!("{}{n}", self.prefix),
        }
    }

    /// Whether lines of this kind are still written (past `LINES_MAX`, only the main thread's
    /// top-level spans and the events).
    fn keeps(&mut self, top: bool) -> bool {
        if self.made < LINES_MAX || top {
            return true;
        }
        if !self.thinned {
            self.thinned = true;
            let t = self.us(Instant::now());
            self.push(Line::Event(Ev {
                t: Some(t),
                ..event("thinned", &format!("past {LINES_MAX} lines: only the main thread's top-level spans from here"))
            }));
        }
        false
    }

    /// A line made: counted, and to write.
    fn push(&mut self, l: Line) {
        self.lines.push(l);
        self.count();
    }

    /// A line counted (made, or a span pending that will be one), the thresholds doubled each
    /// `STEP`.
    fn count(&mut self) {
        self.made += 1;
        if self.made % STEP == 0 {
            self.min_own = self.min_own.saturating_mul(2);
            self.wait_gap = self.wait_gap.saturating_mul(2);
            store::waits::set_gap(self.wait_gap);
        }
    }

    /// The spans pending (counted as they began to) written, those `done` says.
    fn flush_pend(&mut self, done: impl Fn(&Pend) -> bool) {
        let keys: Vec<_> = self.pend.iter().filter(|(_, p)| done(p)).map(|(k, _)| k.clone()).collect();
        for k in keys {
            if let Some(p) = self.pend.remove(&k) {
                let l = self.pend_line(&k, p);
                self.lines.push(l);
            }
        }
    }

    fn pend_line(&self, key: &(Option<u64>, u32, String), p: Pend) -> Line {
        Line::Span {
            id: self.id(p.id),
            parent: key.0.map(|i| self.id(i)).or_else(|| self.parent.clone()),
            th: self.th(key.1),
            name: key.2.clone(),
            t0: p.t0,
            t1: p.t1,
            n: p.n,
            busy: (p.n > 1).then_some(p.busy),
            cpu: p.cpu,
            w: p.w,
            b: p.b,
            f: p.f,
            cut: false,
        }
    }

    /// A span closed: merged with the one before of its name when both are short and close, else
    /// that one written and this one waiting (short), or written (long, or with sub-spans).
    pub fn closed(&mut self, c: Closed) {
        let t_in = Instant::now();
        let (t0, t1) = (self.us(c.t0), self.us(c.t1));
        let us = |ns: u64| ns / 1000;
        let top = c.parent.is_none() && c.th == 0;
        if !self.keeps(top) {
            return;
        }
        let key = (c.parent, c.th, c.name.to_string());
        let short = !c.kids && !c.cut && c.t1.saturating_duration_since(c.t0).as_nanos() < self.min_own as u128;
        let gap = us(self.min_own);
        if short {
            if let Some(p) = self.pend.get_mut(&key).filter(|p| t0 <= p.t1 + gap) {
                p.t1 = t1;
                p.busy += t1 - t0;
                p.n += 1;
                p.cpu = match (p.cpu, c.cpu_us) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
                for (a, b) in p.w.iter_mut().zip(c.waits_ns) {
                    *a += us(b);
                }
                p.b += c.bytes;
                p.f += c.files;
                self.cost_ns += t_in.elapsed().as_nanos() as u64;
                return;
            }
        }
        if let Some(p) = self.pend.remove(&key) {
            let l = self.pend_line(&key, p);
            self.lines.push(l);
        }
        let p = Pend {
            id: c.id,
            t0,
            t1,
            busy: t1 - t0,
            n: 1,
            cpu: c.cpu_us,
            w: c.waits_ns.map(us),
            b: c.bytes,
            f: c.files,
        };
        if short {
            self.pend.insert(key, p);
            self.count();
            // (Many names pending at once, each under its own parent: written, so memory stays bounded.)
            if self.pend.len() > PEND_MAX {
                self.flush_pend(|_| true);
            }
        } else {
            let mut l = self.pend_line(&key, p);
            if let Line::Span { cut, .. } = &mut l {
                *cut = c.cut;
            }
            self.push(l);
        }
        self.cost_ns += t_in.elapsed().as_nanos() as u64;
    }

    /// A safe point at `at`: into the run being merged when within `SAFE_GAP_US` of its last.
    pub fn safe_point(&mut self, at: Instant) {
        let t = self.us(at);
        match &mut self.safe {
            Some((_, last, n)) if t <= *last + SAFE_GAP_US => {
                *last = (*last).max(t);
                *n += 1;
            }
            s => {
                if let Some((a, b, n)) = s.replace((t, t, 1)) {
                    self.safe_line(a, b, n);
                }
            }
        }
    }

    fn safe_line(&mut self, a: u64, b: u64, n: u64) {
        self.push(Line::Event(Ev { t: Some(a), t1: (b > a).then_some(b), n: Some(n), ..event("safe", "") }));
    }

    /// An event now.
    pub fn event(&mut self, mut e: Ev) {
        if e.t.is_none() && e.at.is_none() {
            e.t = Some(self.us(Instant::now()));
        }
        self.push(Line::Event(e));
    }

    /// A child program about to run inside span `parent` on lane `lane`: what it's told
    /// (`SPANS_IN_ENV`).
    pub fn child_env(&mut self, parent: Option<u64>, lane: u32) -> String {
        self.children += 1;
        let p = parent.map(|i| self.id(i)).or_else(|| self.parent.clone()).unwrap_or_default();
        format!("{p}\t{}{}.\t{}\t{}", self.prefix, self.children, self.start_us, self.th(lane))
    }

    /// The lines to write now, with the waits and notes store::waits has, the spans that can't
    /// merge any more, the open ones to say (`open`: id, parent, lane, name, start: those open
    /// `OPEN_SAID` not said yet) and the log's length; every one pending as well when `last`.
    /// Returns the bytes to append (`Sink::write`, outside the run's lock).
    pub fn tick(
        &mut self,
        open: &mut dyn Iterator<Item = (u64, Option<u64>, u32, &str, Instant)>,
        last: bool,
    ) -> Vec<u8> {
        let t_in = Instant::now();
        let now = self.us(t_in);
        let (ivs, notes) = store::waits::take(last);
        for n in notes {
            let t = n.0 / 1000 + self.off_us;
            self.push(Line::Event(Ev { t: Some(t), ..event(n.1, &n.2) }));
        }
        if self.made < LINES_MAX {
            for i in ivs {
                let on = ["nas", "net", "disk", "other"][i.on as usize];
                let o = self.off_us;
                self.push(Line::Wait {
                    on: on.into(),
                    t0: i.t0 / 1000 + o,
                    t1: i.t1 / 1000 + o,
                    n: i.n,
                    busy: i.busy / 1000,
                });
            }
        }
        let gap = self.min_own / 1000;
        self.flush_pend(|p| last || p.t1 + gap < now);
        if let Some((a, b, n)) = self.safe.filter(|s| last || s.1 + SAFE_GAP_US < now) {
            self.safe = None;
            self.safe_line(a, b, n);
        }
        if !last {
            for (id, parent, lane, name, t0) in open {
                let t0 = self.us(t0);
                if self.keeps(parent.is_none() && lane == 0) {
                    let l = Line::Open {
                        id: self.id(id),
                        parent: parent.map(|i| self.id(i)).or_else(|| self.parent.clone()),
                        th: self.th(lane),
                        name: name.to_string(),
                        t0,
                    };
                    self.push(l);
                }
            }
        }
        if self.log_at.is_some() {
            if let Some(at) = log_len().filter(|&a| Some(a) != self.log_at) {
                self.log_at = Some(at);
                self.push(Line::Log { t: now, at });
            }
        }
        let mut out = Vec::new();
        for l in self.lines.drain(..) {
            if serde_json::to_writer(&mut out, &l).is_ok() {
                out.push(b'\n');
            }
        }
        self.cost_ns += t_in.elapsed().as_nanos() as u64;
        out
    }

    /// Where `tick`'s bytes go, to write outside the run's lock.
    pub fn sink(&self) -> Option<Sink> {
        self.file.clone().map(|f| Sink(f, self.lost.clone()))
    }
}

/// A recorder's file (`Rec::sink`).
pub(crate) struct Sink(std::sync::Arc<std::fs::File>, std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Sink {
    /// Appends `b` (from `Rec::tick`): one write; a failure said once, the lines lost.
    pub fn write(&self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        if let Err(e) = (&*self.0).write_all(b) {
            if !self.1.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("spans: lines lost ({e})");
            }
        }
    }
}

/// A job's spans file read (lines that don't read skipped): what an agent or the lead reads.
pub fn parse(text: &str) -> Vec<Line> {
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_round_trip_and_an_unknown_kind_reads_as_other() {
        let s = Line::Span {
            id: "3.4".into(),
            parent: Some("3.1".into()),
            th: String::new(),
            name: "sampling \"x\"".into(),
            t0: 10,
            t1: 20,
            n: 1,
            busy: None,
            cpu: Some(5),
            w: [1, 0, 0, 0],
            b: 0,
            f: 0,
            cut: false,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(
            j,
            r#"{"k":"s","id":"3.4","in":"3.1","name":"sampling \"x\"","t0":10,"t1":20,"cpu":5,"w":[1,0,0,0]}"#
        );
        assert_eq!(serde_json::from_str::<Line>(&j).unwrap(), s);
        assert_eq!(parse("{\"k\":\"zz\",\"x\":1}\nnot json\n"), vec![Line::Other]);
    }

    /// A timed run keeping spans: its phases' spans nested (a sub-phase in its parent), a loop's
    /// short ones merged (500 in a few lines, every one counted), a thread's on a lane of its own, a
    /// NAS wait as an interval, a retry's note, two safe points as one event, a child program told
    /// what it's within, a phase open at the end cut there, and the end.
    #[test]
    fn a_timed_run_writes_its_spans_waits_and_events() {
        // Run again in a process of its own, this test alone in it (the run is the process's).
        const ALONE: &str = "SCENIC_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let d = tempfile::tempdir().unwrap();
            let f = d.path().join("spans.ndjson");
            let out = store::spawn::output(
                store::spawn::command(std::env::current_exe().unwrap())
                    .args(["--exact", "spans::tests::a_timed_run_writes_its_spans_waits_and_events"])
                    .env(ALONE, "1")
                    .env(SPANS_ENV, &f),
            )
            .unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && said.contains(" 1 passed"),
                "{said}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let lines = parse(&std::fs::read_to_string(&f).unwrap());
            assert!(matches!(&lines[0], Line::Head { kind, parent: None, .. } if kind == "test"), "{:?}", lines[0]);
            let spans: Vec<_> = lines
                .iter()
                .filter_map(|l| match l {
                    Line::Span { id, parent, th, name, t0, t1, n, cut, .. } => {
                        assert!(t1 >= t0, "{l:?}");
                        Some((id.clone(), parent.clone(), th.clone(), name.as_str(), *n, *cut))
                    }
                    _ => None,
                })
                .collect();
            let one =
                |name: &str| spans.iter().find(|s| s.3 == name).cloned().unwrap_or_else(|| panic!("{name}: {spans:?}"));
            let parent = one("parent");
            assert_eq!(one("child a").1, Some(parent.0.clone()), "a sub-phase within its parent");
            let tiny: Vec<_> = spans.iter().filter(|s| s.3 == "tiny").collect();
            assert_eq!(tiny.iter().map(|s| s.4).sum::<u64>(), 500, "every span counted");
            assert!(tiny.len() < 50, "short spans merged: {} lines", tiny.len());
            assert_eq!(one("beside").2, "1", "a thread's lane");
            assert!(one("still open").5, "open at the end: cut");
            assert!(!one("parent").5);
            let nas = lines.iter().find_map(|l| match l {
                Line::Wait { on, t0, t1, .. } if on == "nas" => Some(t1 - t0),
                _ => None,
            });
            assert!(nas.is_some_and(|d| d >= 20_000), "{nas:?}");
            let ev = |e: &str| {
                lines.iter().find_map(|l| match l {
                    Line::Event(x) if x.e == e => Some(x.clone()),
                    _ => None,
                })
            };
            assert!(ev("retry").is_some_and(|x| x.text.contains("EIO") && x.t.is_some()));
            assert_eq!(ev("safe").and_then(|x| x.n), Some(2));
            assert_eq!(ev("end").and_then(|x| x.ok), Some(true));
            return;
        }
        use crate::timings::{child, finish, phase, reset, start, sub};
        reset();
        start("test");
        {
            let _p = phase("parent");
            {
                let _s = sub("child a");
            }
            let mut c = store::spawn::command("true");
            child(&mut c);
            let within = c.get_envs().find(|(k, _)| *k == SPANS_IN_ENV).and_then(|(_, v)| v).unwrap();
            let f: Vec<String> = within.to_string_lossy().split('\t').map(str::to_string).collect();
            assert_eq!((f.len(), f[1].as_str()), (4, "1."), "{f:?}");
        }
        for _ in 0..500 {
            let _t = phase("tiny");
        }
        {
            let _p = phase("waits");
            let _w = store::waits::nas();
            std::thread::sleep(std::time::Duration::from_millis(20)); // (sleep ok: a NAS call's stand-in)
        }
        std::thread::spawn(|| {
            let _b = phase("beside");
        })
        .join()
        .unwrap();
        store::retry::log(format_args!("a NAS read: EIO; try 2 of 12 in 1s"));
        crate::control::reached();
        crate::control::reached();
        let _open = phase("still open");
        finish(true);
    }

    /// Past `LINES_MAX` lines, only the main thread's top-level spans are written, said once; each
    /// `STEP` lines doubles the threshold under which spans merge. A span open a while is said open.
    #[test]
    fn a_job_s_lines_are_bounded_and_open_spans_said() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.ndjson");
        let t0 = Instant::now();
        let mut r = Rec::to_file(&f, "test", t0, Some("7\t2.\t0\t1")).unwrap();
        let mut sink = Vec::new();
        let close = |r: &mut Rec, i: u64, parent: Option<u64>| {
            r.closed(Closed {
                id: i,
                parent,
                th: 0,
                name: &format!("p{i}"),
                t0,
                t1: t0 + std::time::Duration::from_secs(1),
                cpu_us: None,
                waits_ns: [0; 4],
                bytes: 0,
                files: 0,
                kids: false,
                cut: false,
            })
        };
        for i in 0..LINES_MAX + 100 {
            close(&mut r, i, Some(0));
            if i % 1000 == 0 {
                sink.extend(r.tick(&mut std::iter::empty(), false));
            }
        }
        assert_eq!(r.min_own, MIN_OWN0_NS << (LINES_MAX / STEP), "doubled each STEP");
        assert!(r.pend.len() <= PEND_MAX);
        close(&mut r, 1, None);
        sink.extend(r.tick(&mut [(9, None, 0, "open", t0)].into_iter(), false));
        sink.extend(r.tick(&mut std::iter::empty(), true));
        let lines = parse(&String::from_utf8(sink).unwrap());
        let spans = lines.iter().filter(|l| matches!(l, Line::Span { .. })).count() as u64;
        assert!(spans <= LINES_MAX && spans > LINES_MAX - 100, "{spans}");
        assert_eq!(lines.iter().filter(|l| matches!(l, Line::Event(e) if e.e == "thinned")).count(), 1);
        // (A child's: its ids under its prefix, its top-level spans within the span that ran it.)
        assert!(lines
            .iter()
            .any(|l| matches!(l, Line::Span { id, parent, .. } if id == "2.1" && parent.as_deref() == Some("7"))));
        assert!(
            lines.iter().any(|l| matches!(l, Line::Open { id, th, .. } if id == "2.9" && th == "1")),
            "said open, on its parent's lane"
        );
    }
}
