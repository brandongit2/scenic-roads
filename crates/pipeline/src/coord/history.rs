//! What happened in the build, in order (the worker page's activity: docs/workers.md, The page):
//! each job the lead started and ended, each lease a worker took, handed back, failed or let
//! lapse, each task done or failed, the catalogues written and the map tiles planned, the pauses,
//! the workers first heard from, the devices let help or not, the agents started, the
//! lead's conditions changing, and each Mac's build caches cleared or freed. Kept on the
//! lead's disk (`history.jsonl`, a line an event, the last week's, KEEP_MAX at most), served by
//! `/work/history` (the events after a number) and summed by the hour for the page (`rates`).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The days of history the page reads (today's and the six before), kept in the day folders
/// `state/coord/history/<day>/`; older days are packed one tar a day (crate::agent::histarchive).
pub const DAYS: u64 = 7;
/// How long events are kept (seconds), and the most kept (a week of a busy build is a few thousand).
pub const KEEP_S: u64 = DAYS * 86400;
pub const KEEP_MAX: usize = 50_000;

/// One thing that happened.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Its number: each event's is one more than the one before's, across restarts.
    #[serde(default)]
    pub seq: u64,
    /// When (unix seconds).
    #[serde(default)]
    pub t: u64,
    /// What: "start" and "end" (a lead's job), "lease", "done", "fail" and "lapse" (a worker's
    /// job), "task", "task-fail" and "task-back" (a task given back unfinished, not failed),
    /// "catalog" (a catalogue written: its `n`, its job's time and its crate::pubtimes::CatalogueStats),
    /// "merged" (journal entries merged: their keys), "planned" (map tiles first
    /// planned: crate::pubtimes::planned_event), "pause" and "resume", "worker" (first heard from), "device" (a device's ask to
    /// help accepted or declined, a device forgotten: its asks aren't), "agent" (one started),
    /// "conditions" (the lead's changed), "member" (a member coming into touch or going out of it,
    /// as the lead sees it: crate::agent::lead::membership), "caches" (a Mac's build caches cleared on its
    /// owner's ask, or freed toward its room target: what was freed, in the note).
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
    /// What the job does (its description), for a start and an end.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub what: String,
    /// In words: how it ended, why it failed, what changed, who paused.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The catalogue it's about (a "catalog": the one written).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    /// What else it says, for publishing's times (crate::pubtimes): a "catalog"'s
    /// crate::pubtimes::CatalogueStats, a "planned"'s units reaching each map tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// What a job waited on (a "start" or "lease", set by `History::add` from the forecast's
    /// dependencies: `Waits`): for each step its targets wait on, the job of that step that finished
    /// last among those they wait on, by its "end" or "done" event's number; sorted. Events before
    /// task #791 have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<u64>,
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
    /// Each target's last finishing job kept (step, target) → its "end" or "done" event's number.
    built: HashMap<(String, String), u64>,
    waits: Waits,
}

/// The forecast's dependencies (crate::agent::forecast's items): each item (step, target), the
/// items it waits on (`deps`, indices into `keys`) and the sets it waits on whole (`sets`, into
/// `groups`, each a list of items).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub keys: Vec<(String, String)>,
    pub deps: Vec<Vec<u32>>,
    pub sets: Vec<Vec<u32>>,
    pub groups: Vec<Vec<u32>>,
}

/// What the targets not yet started wait on: the last forecast's `Graph`, and for each of its items
/// what it waited on that finished before that forecast was made (an earlier forecast's
/// dependencies, done since: gone from the last one's), step → the last job's event number.
#[derive(Debug, Default)]
struct Waits {
    graph: Graph,
    index: HashMap<(String, String), u32>,
    resolved: HashMap<(String, String), BTreeMap<String, u64>>,
}

/// Into `out`, for each step, the last finishing job (`built`) of the items `of` names in `g`.
fn last_per_step(
    g: &Graph,
    of: impl Iterator<Item = u32>,
    built: &HashMap<(String, String), u64>,
    out: &mut BTreeMap<String, u64>,
) {
    for d in of {
        let Some(k) = g.keys.get(d as usize) else { continue };
        if let Some(&seq) = built.get(k) {
            let e = out.entry(k.0.clone()).or_default();
            *e = (*e).max(seq);
        }
    }
}

impl Waits {
    /// What item `i` of the graph waits on that's finished, step → the last job's number.
    fn of(
        &self,
        i: u32,
        built: &HashMap<(String, String), u64>,
        sets: &mut HashMap<u32, BTreeMap<String, u64>>,
    ) -> BTreeMap<String, u64> {
        let g = &self.graph;
        let mut out = self.resolved.get(&g.keys[i as usize]).cloned().unwrap_or_default();
        last_per_step(g, g.deps[i as usize].iter().copied(), built, &mut out);
        for &s in &g.sets[i as usize] {
            // (A set is shared by many items: worked out once.)
            let r = sets.entry(s).or_insert_with(|| {
                let mut r = BTreeMap::new();
                last_per_step(g, g.groups.get(s as usize).into_iter().flatten().copied(), built, &mut r);
                r
            });
            for (k, &v) in r.iter() {
                let e = out.entry(k.clone()).or_default();
                *e = (*e).max(v);
            }
        }
        out
    }
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
    /// By worker, by step: the targets it finished (a task: one of its kind's, "tail" or "bldtile").
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
        let mut h = History {
            path: path.map(Path::to_path_buf),
            events: VecDeque::new(),
            seq: 0,
            lines: 0,
            built: HashMap::new(),
            waits: Waits::default(),
        };
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
        while h.events.len() > KEEP_MAX {
            h.events.pop_front();
        }
        let ends: Vec<Event> = h.events.iter().filter(|e| finished(e)).cloned().collect();
        for e in &ends {
            h.note_built(e);
        }
        h
    }

    /// The forecast's dependencies (its items' last: `Graph`), what each start and lease from now
    /// on waited on is read from; what the last ones waited on that's finished since is kept.
    pub fn set_waits(&mut self, graph: Graph) {
        let old = std::mem::take(&mut self.waits);
        let mut sets = HashMap::new();
        let mut resolved = HashMap::new();
        for (k, &i) in &old.index {
            let r = old.of(i, &self.built, &mut sets);
            if !r.is_empty() {
                resolved.insert(k.clone(), r);
            }
        }
        let index: HashMap<(String, String), u32> =
            graph.keys.iter().enumerate().map(|(i, k)| (k.clone(), i as u32)).collect();
        resolved.retain(|k, _| index.contains_key(k));
        self.waits = Waits { graph, index, resolved };
    }

    /// A finished job's targets, each its last job (`built`); those of jobs out of the week's
    /// history let go.
    fn note_built(&mut self, e: &Event) {
        let step = e.step.clone().unwrap_or_default();
        for t in &e.targets {
            self.built.insert((step.clone(), t.clone()), e.seq);
        }
    }

    /// The oldest event's time.
    pub fn oldest(&self) -> Option<u64> {
        self.events.front().map(|e| e.t)
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
        if matches!(e.kind.as_str(), "start" | "lease") && e.after.is_empty() {
            e.after = self.after(e.step.as_deref().unwrap_or(""), &e.targets);
        }
        if finished(&e) {
            self.note_built(&e);
        }
        if let Some(p) = &self.path {
            let line = serde_json::to_string(&e).unwrap_or_default();
            let r = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
            if let Err(err) = r {
                eprintln!("coordinator: keeping the history: {err}");
            }
            self.lines += 1;
        }
        self.events.push_back(e);
        let from = now_s().saturating_sub(KEEP_S);
        while self.events.front().is_some_and(|e| e.t < from) || self.events.len() > KEEP_MAX {
            self.events.pop_front();
        }
        if self.lines > 2 * self.events.len() + 1000 {
            let first = self.events.front().map_or(0, |e| e.seq);
            self.built.retain(|_, &mut s| s >= first);
            self.rewrite();
        }
        self.seq
    }

    /// What a job of `step` on `targets` waits on, as `Event::after` says.
    fn after(&self, step: &str, targets: &[String]) -> Vec<u64> {
        let mut sets = HashMap::new();
        let mut out: Vec<u64> = targets
            .iter()
            .filter_map(|t| self.waits.index.get(&(step.to_string(), t.clone())))
            .flat_map(|&i| self.waits.of(i, &self.built, &mut sets).into_values())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The file rewritten with the events kept (whole: a crash leaves the old one).
    fn rewrite(&mut self) {
        let Some(p) = &self.path else { return };
        let text: String = self.events.iter().map(|e| serde_json::to_string(e).unwrap_or_default() + "\n").collect();
        match store::whole::write(p, text.as_bytes()) {
            Ok(()) => self.lines = self.events.len(),
            Err(e) => eprintln!("coordinator: rewriting the history: {e:#}"),
        }
    }

    /// The events after number `seq`, oldest first, at most `max` (the earliest of them).
    pub fn since(&self, seq: u64, max: usize) -> Vec<Event> {
        self.events.iter().filter(|e| e.seq > seq).take(max).cloned().collect()
    }

    /// The last `hours` hours to `now`, an hour a row (`Rates`): each worker's targets finished
    /// (the lead's jobs' at their end, a worker's at its hand-off, a task at its end) and the
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
                // (An agent started: its worker's jobs ended with the agent before, whenever that was
                // (its other slots' too, by any name they went by: crate::agent::is_slot_of); they
                // count to here at most.)
                "agent" => {
                    let mine: Vec<String> =
                        started.keys().filter(|k| crate::agent::is_slot_of(k, &w)).cloned().collect();
                    for k in mine {
                        if let Some(t0) = started.remove(&k) {
                            spans.push((k, t0, e.t));
                        }
                    }
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
                "task" | "task-fail" | "task-back" => {
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
                "end" | "done" if e.ok != Some(false) || !e.targets.is_empty() => {
                    Some((e.step.clone().unwrap_or_default(), e.targets.len() as u32))
                }
                "task" => Some((e.step.clone().unwrap_or_else(|| "tail".into()), 1)),
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

/// Whether `e` is a job's end with targets it finished (a lead's "end", a worker's "done"; one
/// that failed names those it finished first).
fn finished(e: &Event) -> bool {
    matches!(e.kind.as_str(), "end" | "done") && !e.targets.is_empty()
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
        // The lead's job over the hour's end, a worker's lease within the first, a page's task.
        h.add(Event { step: Some("unit".into()), ..ev("start", t + 1800, "m4") });
        h.add(Event {
            step: Some("unit".into()),
            targets: vec!["6/1/1".into(), "6/1/2".into()],
            ok: Some(true),
            ..ev("end", t + 4500, "m4")
        });
        h.add(Event { lease: Some(7), step: Some("terrain".into()), ..ev("lease", t + 600, "m1") });
        h.add(Event {
            lease: Some(7),
            step: Some("terrain".into()),
            targets: vec!["3/1/1".into()],
            ..ev("done", t + 1200, "m1")
        });
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

    #[test]
    fn a_start_names_what_it_waited_on() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("history.jsonl");
        let mut h = History::load(Some(&p));
        let k = |s: &str, t: &str| (s.to_string(), t.to_string());
        let done = |h: &mut History, step: &str, targets: &[&str]| {
            h.add(Event {
                step: Some(step.into()),
                targets: targets.iter().map(|t| t.to_string()).collect(),
                ..ev("done", 0, "m1")
            })
        };
        // A z6 tile waits on its slope-deep (its own) and on its region's terrain (a set).
        h.set_waits(Graph {
            keys: vec![
                k("terrain-hi", "6/1/1"),
                k("terrain-hi", "6/1/2"),
                k("slope-deep", "10/1/1"),
                k("unit", "6/1/1"),
            ],
            deps: vec![vec![], vec![], vec![], vec![2]],
            sets: vec![vec![], vec![], vec![], vec![0]],
            groups: vec![vec![0, 1]],
        });
        let t1 = done(&mut h, "terrain-hi", &["6/1/1"]);
        let t2 = done(&mut h, "terrain-hi", &["6/1/2"]);
        // A later forecast: the terrain done is in none of its sets, still waited on.
        h.set_waits(Graph {
            keys: vec![k("slope-deep", "10/1/1"), k("unit", "6/1/1")],
            deps: vec![vec![], vec![0]],
            sets: vec![vec![], vec![]],
            groups: vec![],
        });
        let s = done(&mut h, "slope-deep", &["10/1/1"]);
        // (The terrain step's last alone is named.)
        assert!(t1 < t2 && t2 < s);
        let start = h.add(Event { step: Some("unit".into()), targets: vec!["6/1/1".into()], ..ev("start", 0, "m4") });
        let e = h.since(start - 1, 1).remove(0);
        assert_eq!(e.after, [t2, s]);
        // Kept on disk, read back; a lease of a target nothing waits for names none.
        let h2 = History::load(Some(&p));
        assert_eq!(h2.since(start - 1, 1)[0].after, [t2, s]);
        let lease = h.add(Event { step: Some("trees".into()), targets: vec!["6/1/1".into()], ..ev("lease", 0, "m1") });
        assert!(h.since(lease - 1, 1)[0].after.is_empty());
        // An event of before #791 (no field) reads back with none, and none written.
        let old: Event = serde_json::from_str(r#"{"seq":1,"t":5,"kind":"start","step":"unit"}"#).unwrap();
        assert!(old.after.is_empty());
        assert!(!serde_json::to_string(&old).unwrap().contains("after"));
    }
}
