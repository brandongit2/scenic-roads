//! What happened in the build, in order (the worker page's activity: docs/workers.md, The page):
//! each job the build Mac started and ended, each lease a worker took, handed back, failed or let
//! lapse, each task done or failed, the catalogs, the pauses, the workers first heard from, the
//! agents started and the build Mac's conditions changing. Kept on the build Mac's disk
//! (`history.jsonl`, a line an event, the last week's), served by `/work/history` (the events after
//! a number) and summed by the hour for the page (`rates`).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

/// How long events are kept (seconds).
pub const KEEP_S: u64 = 7 * 86400;

/// One thing that happened.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Its number: each event's is one more than the one before's, across restarts.
    #[serde(default)]
    pub seq: u64,
    /// When (unix seconds).
    #[serde(default)]
    pub t: u64,
    /// What: "start" and "end" (a build Mac's job), "lease", "done", "fail" and "lapse" (a worker's
    /// job), "task" and "task-fail", "catalog", "pause" and "resume", "worker" (first heard from),
    /// "agent" (one started), "conditions" (the build Mac's changed).
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    /// The lease it's about (a worker's job, start to end).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    /// How long it took (an end, a done, a task).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// In words: what it was, why it failed, what changed, who paused.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Event {
    pub fn new(kind: &str) -> Event {
        Event { kind: kind.into(), ..Default::default() }
    }
}

/// The events kept, the last week's, in order.
#[derive(Debug)]
pub struct History {
    path: Option<PathBuf>,
    events: VecDeque<Event>,
    seq: u64,
    /// Lines in the file: rewritten with what's kept once it holds twice as many.
    lines: usize,
}

/// The events summed by the hour, for the page's activity: what each worker finished (its targets,
/// by step) and how long it was busy, and the build paused.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Rates {
    pub bucket_s: u64,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Row {
    /// The hour's start (unix seconds).
    pub t: u64,
    /// By worker, by step: the targets it finished (a task: one of "tail").
    pub done: BTreeMap<String, BTreeMap<String, u32>>,
    /// By worker: the seconds of the hour it was working.
    pub busy_s: BTreeMap<String, u64>,
    pub paused_s: u64,
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl History {
    /// What's kept at `path` (None: in memory alone), the last week's.
    pub fn load(path: Option<&Path>) -> History {
        let mut h = History { path: path.map(Path::to_path_buf), events: VecDeque::new(), seq: 0, lines: 0 };
        let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else { return h };
        let from = now_s().saturating_sub(KEEP_S);
        for l in text.lines() {
            h.lines += 1;
            // (A line cut short by a crash is passed over.)
            let Ok(e) = serde_json::from_str::<Event>(l) else { continue };
            h.seq = h.seq.max(e.seq);
            if e.t >= from {
                h.events.push_back(e);
            }
        }
        h
    }

    /// The last event's number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Adds `e` (now, unless it says when), kept on disk at once; its number.
    pub fn add(&mut self, mut e: Event) -> u64 {
        self.seq += 1;
        e.seq = self.seq;
        if e.t == 0 {
            e.t = now_s();
        }
        if let Some(p) = &self.path {
            let line = serde_json::to_string(&e).unwrap_or_default();
            let r = std::fs::OpenOptions::new().create(true).append(true).open(p).and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
            if let Err(err) = r {
                eprintln!("coordinator: keeping the history: {err}");
            }
            self.lines += 1;
        }
        self.events.push_back(e);
        let from = now_s().saturating_sub(KEEP_S);
        while self.events.front().is_some_and(|e| e.t < from) {
            self.events.pop_front();
        }
        if self.lines > 2 * self.events.len() + 1000 {
            self.rewrite();
        }
        self.seq
    }

    /// The file rewritten with the events kept (whole: a crash leaves the old one).
    fn rewrite(&mut self) {
        let Some(p) = &self.path else { return };
        let text: String = self.events.iter().map(|e| serde_json::to_string(e).unwrap_or_default() + "\n").collect();
        match crate::whole::write(p, text.as_bytes()) {
            Ok(()) => self.lines = self.events.len(),
            Err(e) => eprintln!("coordinator: rewriting the history: {e:#}"),
        }
    }

    /// The events after number `seq`, oldest first, at most `max` (the earliest of them).
    pub fn since(&self, seq: u64, max: usize) -> Vec<Event> {
        self.events.iter().filter(|e| e.seq > seq).take(max).cloned().collect()
    }

    /// The last `hours` hours to `now`, an hour a row (`Rates`): each worker's targets finished
    /// (the build Mac's jobs' at their end, a worker's at its hand-off, a task at its end) and the
    /// seconds it was busy (a job from its start to its end, a lease from its grant to its end, a
    /// task its time back from its end), and the seconds the build was paused.
    pub fn rates(&self, now: u64, hours: u64) -> Rates {
        const H: u64 = 3600;
        let first = (now / H).saturating_sub(hours.saturating_sub(1)) * H;
        let mut rows: Vec<Row> = (0..hours).map(|i| Row { t: first + i * H, ..Default::default() }).collect();
        let mut spans: Vec<(String, u64, u64)> = Vec::new();
        let mut paused: Vec<(u64, u64)> = Vec::new();
        // (Open spans: a job's start by its worker, a lease's grant by its id, the pause.)
        let mut started: BTreeMap<String, u64> = BTreeMap::new();
        let mut leased: BTreeMap<u64, (String, u64)> = BTreeMap::new();
        let mut pause_from: Option<u64> = None;
        for e in &self.events {
            let w = e.worker.clone().unwrap_or_default();
            match e.kind.as_str() {
                "start" => {
                    started.insert(w.clone(), e.t);
                }
                "end" => {
                    if let Some(t0) = started.remove(&w) {
                        spans.push((w.clone(), t0, e.t));
                    }
                }
                "lease" => {
                    if let Some(l) = e.lease {
                        leased.insert(l, (w.clone(), e.t));
                    }
                }
                "done" | "fail" | "lapse" => {
                    if let Some((w, t0)) = e.lease.and_then(|l| leased.remove(&l)) {
                        spans.push((w, t0, e.t));
                    }
                }
                "task" | "task-fail" => {
                    if let Some(s) = e.secs {
                        spans.push((w.clone(), e.t.saturating_sub(s as u64), e.t));
                    }
                }
                "pause" => {
                    pause_from.get_or_insert(e.t);
                }
                "resume" => {
                    if let Some(t0) = pause_from.take() {
                        paused.push((t0, e.t));
                    }
                }
                _ => {}
            }
            // What it finished.
            let finished = match e.kind.as_str() {
                "end" | "done" if e.ok != Some(false) || !e.targets.is_empty() => Some((e.step.clone().unwrap_or_default(), e.targets.len() as u32)),
                "task" => Some(("tail".to_string(), 1)),
                _ => None,
            };
            if let Some((step, n)) = finished.filter(|(_, n)| e.t >= first && *n > 0) {
                let row = &mut rows[((e.t - first) / H).min(hours - 1) as usize];
                *row.done.entry(w).or_default().entry(step).or_default() += n;
            }
        }
        // (What's still going counts to now.)
        spans.extend(started.into_iter().map(|(w, t0)| (w, t0, now)));
        spans.extend(leased.into_values().map(|(w, t0)| (w, t0, now)));
        paused.extend(pause_from.map(|t0| (t0, now)));
        let overlap = |a: u64, b: u64, r: u64| b.min(r + H).saturating_sub(a.max(r));
        for row in &mut rows {
            for (w, a, b) in &spans {
                let s = overlap(*a, *b, row.t);
                if s > 0 {
                    // (Two of a worker's at once, a page's slots: at most the hour.)
                    let e = row.busy_s.entry(w.clone()).or_default();
                    *e = (*e + s).min(H);
                }
            }
            row.paused_s = paused.iter().map(|(a, b)| overlap(*a, *b, row.t)).sum::<u64>().min(H);
        }
        Rates { bucket_s: H, rows }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, t: u64, worker: &str) -> Event {
        Event { kind: kind.into(), t, worker: Some(worker.into()), ..Default::default() }
    }

    #[test]
    fn events_are_kept_numbered_and_read_back() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("history.jsonl");
        let mut h = History::load(Some(&p));
        assert_eq!(h.add(Event::new("agent")), 1);
        assert_eq!(h.add(Event { step: Some("unit".into()), ..Event::new("start") }), 2);
        // A line cut short (a crash) is passed over; numbers go on.
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"seq\":3,\"t\":").unwrap();
        let mut h = History::load(Some(&p));
        assert_eq!(h.seq(), 2);
        assert_eq!(h.since(1, 10).iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), ["start"]);
        assert_eq!(h.add(Event::new("end")), 3);
        assert_eq!(h.since(0, 2).len(), 2);
        // A week old: gone.
        let mut old = History::load(None);
        old.add(Event { t: now_s() - KEEP_S - 10, ..Event::new("agent") });
        old.add(Event::new("agent"));
        assert_eq!(old.since(0, 10).len(), 1);
    }

    #[test]
    fn rates_by_the_hour() {
        let mut h = History::load(None);
        // (Hours of a day ago: kept, as the last week's are.)
        let t = (now_s() / 3600 - 24) * 3600;
        // The build Mac's job over the hour's end, a worker's lease within the first, a page's task.
        h.add(Event { step: Some("unit".into()), ..ev("start", t + 1800, "m4") });
        h.add(Event { step: Some("unit".into()), targets: vec!["6/1/1".into(), "6/1/2".into()], ok: Some(true), ..ev("end", t + 4500, "m4") });
        h.add(Event { lease: Some(7), step: Some("terrain".into()), ..ev("lease", t + 600, "m1") });
        h.add(Event { lease: Some(7), step: Some("terrain".into()), targets: vec!["3/1/1".into()], ..ev("done", t + 1200, "m1") });
        h.add(Event { secs: Some(60.0), ..ev("task", t + 4000, "ipad") });
        h.add(ev("pause", t + 5000, "m4"));
        let r = h.rates(t + 5400, 2);
        assert_eq!(r.rows.len(), 2);
        assert_eq!((r.rows[0].t, r.rows[1].t), (t, t + 3600));
        assert_eq!(r.rows[0].busy_s["m4"], 1800);
        assert_eq!(r.rows[1].busy_s["m4"], 900);
        assert_eq!(r.rows[0].busy_s["m1"], 600);
        assert_eq!(r.rows[0].done["m1"]["terrain"], 1);
        assert_eq!(r.rows[1].done["m4"]["unit"], 2);
        assert_eq!((r.rows[1].done["ipad"]["tail"], r.rows[1].busy_s["ipad"]), (1, 60));
        // Paused from 5000 s into the second hour, to now.
        assert_eq!(r.rows[1].paused_s, 400);
    }
}
