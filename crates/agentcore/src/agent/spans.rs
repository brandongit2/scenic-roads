//! A job's spans and log sent to the lead (task #1104; docs/formats.md, Job spans): each job run of
//! this Mac's with an id writes its spans (jobrt::spans) to `spans/<id>.ndjson` in the agent's
//! folder, beside its facts (`<id>.json`: `Meta`, and its end once it ended) and what the lead has
//! of it (`<id>.sent`); `chunks` reads what the lead lacks of the spans and of the job's log, and
//! `acked` notes what the lead's answer says it has. A member's beat carries them (crate::agent::beat,
//! while the lead says it takes them: crate::coord::beats::SPANS); the lead's own jobs' go to its
//! coordinator's store every `EVERY` (`ship_local`). So a job's timeline is on the lead within a
//! beat or two of being written, and a Mac that goes down loses at most that.
//!
//! - **Bounded:** a chunk at most `BEAT_MAX` a beat, all jobs together; a run's log at most
//!   `LOG_HEAD` from its start and `LOG_TAIL` at its end (the middle of a longer one left out, the
//!   lead told of the gap).
//! - **Kept until the lead has it all** (its `Ack::done`): then the job's files go. A job whose agent
//!   lost it (no end said, not running here, its facts older than `LOST_AFTER`) is ended as lost;
//!   files older than `KEEP` (a lead that never took them: an app before spans) go.

use super::bus::Snap;
use crate::coord::jobspans::{Ack, Chunk, Meta};
use jobrt::jobid::JobId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The folder in the agent's.
pub const DIR: &str = "spans";
/// The most a beat carries, every job's spans and logs together (bytes); after a beat carrying
/// them failed, a quarter of what it carried, down to `BEAT_MIN`, doubled again with each beat that
/// gets through (`Shipper::beat_bound`).
pub const BEAT_MAX: usize = 512 << 10;
pub const BEAT_MIN: usize = 16 << 10;
/// Of a run's log, at most this much from its start and this much of its end.
pub const LOG_HEAD: u64 = 1 << 20;
pub const LOG_TAIL: u64 = 256 << 10;
/// How often the lead's own jobs' spans go to its store.
pub const EVERY: Duration = Duration::from_secs(10);
/// A job neither ended nor running here this long after it started was lost by its agent.
const LOST_AFTER: Duration = Duration::from_secs(600);
/// Files this old go whatever the lead has (s).
const KEEP: Duration = Duration::from_secs(14 * 86_400);

/// A job's facts on this Mac (`<id>.json`), its end once it ended.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    pub meta: Meta,
    /// The log its run writes to.
    pub log: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
}

/// What the lead has of a job (`<id>.sent`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Sent {
    s: u64,
    l: u64,
    meta: bool,
}

/// This Mac's jobs' spans to send (`<agent>/spans/`).
pub struct Shipper {
    dir: PathBuf,
    /// The jobs running here as the agent's loop last said (`running`), and when.
    running: Arc<Snap<(BTreeSet<JobId>, bool)>>,
    /// The lead's own store, while this Mac leads (`ship_local`).
    local: Arc<Snap<Option<Arc<crate::coord::jobspans::Store>>>>,
    /// The most the next beat carries (`beat_bound`).
    bound: std::sync::atomic::AtomicUsize,
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Shipper {
    /// The shipper of the agent in `home`.
    pub fn new(home: &Path) -> Arc<Shipper> {
        Arc::new(Shipper {
            dir: home.join(DIR),
            running: Snap::new(Default::default()),
            local: Snap::new(None),
            bound: BEAT_MAX.into(),
        })
    }

    /// The most the next beat carries (`BEAT_MAX`, less after beats that failed carrying spans).
    pub fn beat_bound(&self) -> usize {
        self.bound.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A beat carrying spans answered (`ok`) or not: the next's bound doubled, or quartered.
    pub fn beat_went(&self, ok: bool) {
        let b = self.beat_bound();
        let next = if ok { (b * 2).min(BEAT_MAX) } else { (b / 4).max(BEAT_MIN) };
        self.bound.store(next, std::sync::atomic::Ordering::Relaxed);
    }

    fn file(&self, job: JobId, ext: &str) -> PathBuf {
        self.dir.join(format!("{job}.{ext}"))
    }

    /// Job `job` starting (`meta`, its log `log`): its facts kept; where it writes its spans
    /// (jobrt::spans::SPANS_ENV). None: its folder can't be made (it runs without).
    pub fn begin(&self, job: JobId, meta: Meta, log: &Path) -> Option<PathBuf> {
        std::fs::create_dir_all(&self.dir).ok()?;
        let f = Facts { meta, log: log.to_path_buf(), ended: None };
        // (A run again under the same id, a job taken over: its spans go on in the same file.)
        if !self.file(job, "json").exists() {
            store::whole::write(&self.file(job, "json"), &serde_json::to_vec(&f).ok()?).ok()?;
        }
        Some(self.file(job, "ndjson"))
    }

    /// Job `job` ended (`ok`, `how`: its agent's word): said in its spans, its facts marked ended.
    pub fn end(&self, job: JobId, ok: bool, how: &str) {
        let p = self.file(job, "json");
        let Some(mut f) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Facts>(&b).ok()) else {
            return;
        };
        if f.ended.is_some() {
            return;
        }
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let e = jobrt::spans::Line::Event(jobrt::spans::Ev {
            at: Some(at),
            ok: Some(ok),
            ..jobrt::spans::event("exit", if ok { "ended well" } else { how })
        });
        if let Ok(mut line) = serde_json::to_vec(&e) {
            line.push(b'\n');
            use std::io::Write;
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.file(job, "ndjson"))
                .and_then(|mut o| o.write_all(&line));
        }
        f.ended = Some(now_s());
        if let Ok(b) = serde_json::to_vec(&f) {
            store::whole::write(&p, &b).ok();
        }
    }

    /// The jobs running here now (the agent's loop, each round): a job not among them, never
    /// ended, started `LOST_AFTER` ago, was lost by its agent (its Mac down, its agent crashed
    /// before it said): ended so.
    pub fn running(&self, jobs: BTreeSet<JobId>) {
        self.running.set((jobs, true));
    }

    /// This Mac's store to ship to while it leads (`ship_local`); None as it stops leading.
    pub fn lead_store(&self, s: Option<Arc<crate::coord::jobspans::Store>>) {
        self.local.set(s);
    }

    /// The jobs with files here, by id.
    fn jobs(&self) -> Vec<JobId> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<JobId> =
            rd.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".json")?.parse().ok()).collect();
        v.sort();
        v
    }

    fn facts(&self, job: JobId) -> Option<Facts> {
        serde_json::from_slice(&std::fs::read(self.file(job, "json")).ok()?).ok()
    }

    fn sent(&self, job: JobId) -> Sent {
        std::fs::read(self.file(job, "sent")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn forget(&self, job: JobId) {
        for ext in ["ndjson", "sent", "json"] {
            std::fs::remove_file(self.file(job, ext)).ok();
        }
    }

    /// What the lead lacks of each job, `max` bytes at most in all: a job's spans from where the
    /// lead has them, whole lines; its log likewise (whole lines while it runs, the head and tail of
    /// a long one); `end` once that's the rest of both and the job ended. Jobs lost or past `KEEP`
    /// dealt with first.
    pub fn chunks(&self, max: usize) -> Vec<Chunk> {
        let mut out = Vec::new();
        let mut left = max;
        let (running, said) = self.running.get().as_ref().clone();
        for job in self.jobs() {
            let Some(mut f) = self.facts(job) else { continue };
            let age = now_s().saturating_sub(f.meta.start);
            if age > KEEP.as_secs() {
                eprintln!(
                    "agent: job {job}'s spans and log dropped here, {} days old, the lead lacking some of them",
                    KEEP.as_secs() / 86_400
                );
                self.forget(job);
                continue;
            }
            if f.ended.is_none() && said && !running.contains(&job) && age > LOST_AFTER.as_secs() {
                self.end(job, false, "lost: its agent stopped tending it without saying how it ended");
                f.ended = Some(now_s());
            }
            if left == 0 {
                break;
            }
            let sent = self.sent(job);
            let ended = f.ended.is_some();
            let (s, s_rest) = read_lines(&self.file(job, "ndjson"), sent.s, left, ended);
            left -= s.len().min(left);
            let (l_at, l, l_rest) = read_log(&f, sent.l, left, ended);
            left -= l.len().min(left);
            let end = ended && !s_rest && !l_rest;
            if s.is_empty() && l.is_empty() && !end && sent.meta {
                continue;
            }
            out.push(Chunk { job, meta: (!sent.meta).then(|| f.meta.clone()), s_at: sent.s, s, l_at, l, end });
        }
        out
    }

    /// The lead's word on what it has (`Ack`s): noted, and a job it has all of forgotten here.
    pub fn acked(&self, acks: &[Ack]) {
        for a in acks {
            if a.done {
                self.forget(a.job);
                continue;
            }
            if !self.file(a.job, "json").exists() {
                continue;
            }
            let s = Sent { s: a.s, l: a.l, meta: a.meta };
            if let Ok(b) = serde_json::to_vec(&s) {
                store::whole::write(&self.file(a.job, "sent"), &b).ok();
            }
        }
    }

    /// The lead's own jobs' spans to its store (while this Mac leads, `lead_store`): what it lacks,
    /// taken and acknowledged at once.
    pub fn ship_local(&self) {
        let Some(store) = self.local.get().as_ref().clone() else { return };
        // (Its own disk: no beat's bound, but a chunk's.)
        let chunks = self.chunks(crate::coord::jobspans::CHUNK_MAX);
        if !chunks.is_empty() {
            let acks = store.take(chunks);
            self.acked(&acks);
        }
    }

    /// The thread shipping the lead's own jobs' every `EVERY` (`ship_local`), for as long as the
    /// process runs.
    pub fn spawn_local(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("spans-local".into())
            .spawn(move || loop {
                std::thread::sleep(EVERY);
                me.ship_local();
            })
            .ok();
    }
}

/// From byte `at` of `p`, at most `max` bytes of whole lines (all of what's there when `ended` and
/// it fits); and whether more is left after.
fn read_lines(p: &Path, at: u64, max: usize, ended: bool) -> (String, bool) {
    let Ok(mut f) = std::fs::File::open(p) else { return (String::new(), false) };
    let len = f.metadata().map_or(0, |m| m.len());
    if len <= at || f.seek(SeekFrom::Start(at)).is_err() {
        return (String::new(), false);
    }
    let want = ((len - at) as usize).min(max);
    let mut b = vec![0; want];
    if f.read_exact(&mut b).is_err() {
        return (String::new(), true);
    }
    // (Whole lines: a line being written waits; an ended job's last line too, unless it's all there.
    // A line longer than `max` goes in pieces.)
    let whole = b.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
    let take = match () {
        _ if ended && want as u64 == len - at && whole < want => want,
        _ if whole == 0 && want == max => want,
        _ => whole,
    };
    b.truncate(take);
    let more = at + (take as u64) < len;
    (text_of(&b), more)
}

/// `b` as text of the same length in bytes (the lead counts where it has a file to by them): a byte
/// that isn't UTF-8 (a program's binary output in its log, a char cut by a piece's end) as `?`.
fn text_of(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for c in b.utf8_chunks() {
        s.push_str(c.valid());
        s.extend(std::iter::repeat_n('?', c.invalid().len()));
    }
    s
}

/// The job's log from byte `at` of its run's (`Facts::meta`'s `log_from` on), at most `max` bytes:
/// its first `LOG_HEAD`, then (ended) its last `LOG_TAIL`, the middle left out; where it's from,
/// the text, and whether more is left. A log that shrank under it (cut by the agent's room-making)
/// gives nothing more.
fn read_log(f: &Facts, at: u64, max: usize, ended: bool) -> (u64, String, bool) {
    let from = f.meta.log_from;
    let len = std::fs::metadata(&f.log).map_or(0, |m| m.len()).saturating_sub(from);
    let long = len > LOG_HEAD + LOG_TAIL;
    let mut at = at;
    if long && at < LOG_HEAD {
        let (text, _) = read_lines(&f.log, from + at, max.min((LOG_HEAD - at) as usize), false);
        if !text.is_empty() || max == 0 {
            return (at, text, true);
        }
        // (Its head sent, to the last whole line in it.)
    }
    if long {
        // (Past the head: its tail, once it ended.)
        if !ended {
            return (at, String::new(), true);
        }
        at = at.max(len - LOG_TAIL);
    }
    if at >= len {
        return (at, String::new(), false);
    }
    let (text, more) = read_lines(&f.log, from + at, max, ended);
    let more = more || at + (text.len() as u64) < len;
    (at, text, more)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::jobspans::Store;

    fn meta(start: u64, log_from: u64) -> Meta {
        Meta {
            name: "unit 6/1/1".into(),
            step: "unit".into(),
            targets: vec!["6/1/1".into()],
            worker: "m1".into(),
            host: "m1".into(),
            start,
            log_from,
        }
    }

    /// A job's spans and log reach the lead's store chunk by chunk, each from where the lead has
    /// them, whole lines only; a chunk sent again adds nothing; its end and the lead's answer
    /// forget it here; the lead's view reads it back with its log's lines on the job's axis.
    #[test]
    fn spans_and_log_reach_the_lead_and_are_forgotten_once_it_has_them() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let sh = Shipper::new(&home);
        let store = Store::new(d.path().join("coord/jobs"));
        let job = JobId { epoch: 3, n: 1_794_000_000_007 };
        let log = home.join("logs/unit.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "an earlier run\n").unwrap();
        let from = 15;
        let start = now_s();
        let spans = sh.begin(job, meta(start, from), &log).unwrap();
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        let mut s = std::fs::OpenOptions::new().create(true).append(true).open(&spans).unwrap();
        let start_us = start * 1_000_000;
        writeln!(o, "=== run of unit 6/1/1 ===").unwrap();
        writeln!(s, r#"{{"k":"h","v":1,"start_us":{start_us},"kind":"unit","pid":1}}"#).unwrap();
        writeln!(s, r#"{{"k":"l","t":0,"at":{from}}}"#).unwrap();
        writeln!(s, r#"{{"k":"o","id":"1","name":"sampling","t0":1000}}"#).unwrap();
        // (A line half written waits.)
        write!(s, r#"{{"k":"s","id":"2""#).unwrap();
        s.flush().unwrap();
        let c = sh.chunks(BEAT_MAX);
        assert_eq!(c.len(), 1);
        assert!(c[0].meta.is_some() && !c[0].end && c[0].s.ends_with("\n") && !c[0].s.contains(r#""id":"2""#), "{c:?}");
        assert_eq!(c[0].l, "=== run of unit 6/1/1 ===\n");
        // Taken twice (a beat posted again): once.
        let a = store.take(c.clone());
        assert_eq!(store.take(c.clone()), a);
        sh.acked(&a);
        assert!(sh.chunks(BEAT_MAX).is_empty(), "nothing new: nothing sent");
        // The rest, the job's end, the log's last line.
        writeln!(s, r#","name":"x","t0":2000,"t1":3000}}"#).unwrap();
        writeln!(s, r#"{{"k":"s","id":"1","name":"sampling","t0":1000,"t1":2000000}}"#).unwrap();
        writeln!(o, "sampling done").unwrap();
        writeln!(s, r#"{{"k":"l","t":2000000,"at":{}}}"#, std::fs::metadata(&log).unwrap().len()).unwrap();
        sh.end(job, true, "");
        let c = sh.chunks(BEAT_MAX);
        assert!(c[0].end && c[0].meta.is_none() && c[0].s_at > 0, "{c:?}");
        let a = store.take(c);
        assert!(a[0].done);
        sh.acked(&a);
        assert!(sh.jobs().is_empty(), "the lead has it all: forgotten here");
        let v = view(&store, job);
        assert_eq!(v["legacy"], false);
        assert_eq!(v["running"], false);
        assert_eq!(v["spans"][0]["status"], "ok");
        let names: Vec<&str> = v["spans"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["unit 6/1/1", "sampling", "x"], "the job, then by their starts: {v}");
        let sampling = &v["spans"][1];
        assert_eq!((sampling["status"].as_str(), sampling["t1"].as_f64()), (Some("ok"), Some(start as f64 + 2.0)));
        let lines = v["log"]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["text"], "sampling done");
        let t = lines[1]["t"].as_f64().unwrap();
        assert!(t > start as f64 && t <= start as f64 + 2.0, "{t}");
        assert!(v["events"].as_array().unwrap().iter().any(|e| e["kind"] == "exit" && e["ok"] == true));
        // The file is compressed: a zstd frame.
        let b = std::fs::read(store.path(job)).unwrap();
        assert_eq!(&b[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
        // No file: legacy.
        assert_eq!(view(&store, JobId { epoch: 3, n: 1 })["legacy"], true);
    }

    /// A beat carrying spans that failed: the next carries a quarter, down to `BEAT_MIN`; each that
    /// gets through doubles it back to `BEAT_MAX`.
    #[test]
    fn a_failed_beat_carries_less_next() {
        let d = tempfile::tempdir().unwrap();
        let sh = Shipper::new(d.path());
        assert_eq!(sh.beat_bound(), BEAT_MAX);
        sh.beat_went(false);
        assert_eq!(sh.beat_bound(), BEAT_MAX / 4);
        for _ in 0..10 {
            sh.beat_went(false);
        }
        assert_eq!(sh.beat_bound(), BEAT_MIN);
        for _ in 0..10 {
            sh.beat_went(true);
        }
        assert_eq!(sh.beat_bound(), BEAT_MAX);
    }

    /// A log's bytes that aren't UTF-8 go as `?`, one each, so the lead's count of where it has the
    /// log to is the file's; a line longer than a beat's bound goes in pieces, never holding the
    /// rest back.
    #[test]
    fn a_log_s_bytes_are_counted_as_the_file_s() {
        assert_eq!(text_of(b"ok \xff\xfe\n\xe2\x82"), "ok ??\n??");
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("log");
        std::fs::write(&p, [b"x".repeat(300), b"\nnext\n".to_vec()].concat()).unwrap();
        let (a, more) = read_lines(&p, 0, 100, false);
        assert_eq!((a.len(), more), (100, true), "a piece of the long line");
        let (b, _) = read_lines(&p, 100, 1000, false);
        assert_eq!(b, "x".repeat(200) + "\nnext\n");
    }

    fn view(store: &Store, job: JobId) -> serde_json::Value {
        crate::coord::jobspans::view(Some(store), job, Vec::new(), || None)
    }

    /// A long log: its head and tail go, its middle is left out with the gap said; a job never
    /// ended and no longer running here is ended as lost; nothing is sent past the beat's bound.
    #[test]
    fn a_long_log_keeps_its_head_and_tail_and_a_lost_job_is_ended() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let sh = Shipper::new(&home);
        let store = Store::new(d.path().join("coord/jobs"));
        let job = JobId { epoch: 1, n: 9 };
        let log = home.join("j.log");
        std::fs::create_dir_all(&home).unwrap();
        let line = "x".repeat(99) + "\n";
        let n = ((LOG_HEAD + LOG_TAIL) / 100 + 2000) as usize;
        std::fs::write(&log, line.repeat(n)).unwrap();
        // (Started past `LOST_AFTER` ago, not running here.)
        sh.begin(job, meta(now_s() - LOST_AFTER.as_secs() - 5, 0), &log).unwrap();
        sh.running(BTreeSet::new());
        let mut sent = 0usize;
        for _ in 0..100 {
            let c = sh.chunks(BEAT_MAX);
            if c.is_empty() {
                break;
            }
            sent += c.iter().map(|c| c.s.len() + c.l.len()).sum::<usize>();
            assert!(c.iter().map(|c| c.s.len() + c.l.len()).sum::<usize>() <= BEAT_MAX);
            let a = store.take(c);
            sh.acked(&a);
        }
        assert!(sh.jobs().is_empty(), "ended as lost, and all of it sent");
        let total = (n * 100) as u64;
        assert!(sent as u64 <= LOG_HEAD + LOG_TAIL + 1000 && (sent as u64) < total, "{sent}");
        let v = view(&store, job);
        assert_eq!(v["gaps"].as_array().unwrap().len(), 1, "{}", v["gaps"]);
        assert_eq!(v["spans"][0]["status"], "fail", "lost: the run's own span failed");
        assert!(v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "exit" && e["text"].as_str().unwrap().starts_with("lost")));
    }
}
