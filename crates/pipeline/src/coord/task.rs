//! Tasks (docs/workers.md §2): pure work a running job offers to any worker. A task is programs run
//! in order over a folder of files; the job says which programs and which files they read, the
//! coordinator serves exactly those files from this Mac's disk to the worker that leases it and
//! takes back the files it wrote, and the job checks and uses them. Nothing a task does needs the
//! NAS: what it reads was staged here once (the data plane, §4), so another worker costs the NAS
//! nothing.
//!
//! A task's predicted memory (its programs' peak and its files) decides who may take it: a worker
//! asks with what it can spare, whatever it is (§6). A worker that runs out of memory says so with
//! the peak it reached; the task is then offered only to workers that can spare more. One that
//! fails it otherwise, or goes quiet holding it, isn't offered it again; after two such workers the
//! job runs it itself (as it does any task no one takes).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Output {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub enum State {
    Offered,
    Leased { lease: u64, worker: String },
    Done { worker: String, outputs: Vec<Output>, removed: Vec<String>, secs: f64, peak_mb: u64 },
    Failed { why: String },
}

#[derive(Clone, Debug)]
pub struct Task {
    pub id: u64,
    /// The job that offered it (its process id): its tasks go when it ends.
    pub owner: u32,
    pub kind: String,
    /// What the worker is given: the programs to run, the files to fetch.
    pub spec: serde_json::Value,
    /// Where its files are on this Mac, and which (path relative to it → size): only these are served.
    pub root: PathBuf,
    pub inputs: BTreeMap<String, u64>,
    /// Its predicted peak memory on a worker (MB): its programs' and its files'.
    pub mem_mb: u64,
    pub state: State,
    /// Where a worker's uploads go.
    pub out: PathBuf,
    /// Workers it failed on or went quiet on: not offered to them again.
    pub failed_on: BTreeSet<String>,
    pub offered: Instant,
    /// When a worker last leased it, and how long that worker took (lease to done, seconds).
    pub leased_at: Option<Instant>,
    pub wall_s: Option<f64>,
    /// Kept for its worker to finish after the job ran it itself (the job's run took this long,
    /// seconds): the worker's pace measured, its result not used (`measure`).
    pub measuring: Option<f64>,
}

/// A task as a job offers it (`POST /task/offer`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub owner: u32,
    pub kind: String,
    pub spec: serde_json::Value,
    pub root: PathBuf,
    pub inputs: BTreeMap<String, u64>,
    pub mem_mb: u64,
}

#[derive(Debug)]
pub struct Tasks {
    next: u64,
    pub by_id: BTreeMap<u64, Task>,
    /// Where uploads go: a folder per task (on the same disk as the jobs' folders, so taking them is
    /// a rename).
    dir: PathBuf,
    /// The memory of the last tasks offered of each kind (MB, at most `RECENT`): what one takes,
    /// typically (`typical_mb`).
    recent_mb: BTreeMap<String, std::collections::VecDeque<u64>>,
    /// Each worker's pace at each kind of task: its time over the job's own for the same task
    /// (lease to done, against the job's run), weighed in as they come (`note_pace`).
    pub paces: BTreeMap<(String, String), f64>,
    /// When the jobs last waited for a worker whose pace isn't measured to take a task (`explore`).
    explored: BTreeMap<String, Instant>,
}

/// The margin a worker's pace is taken with: it's waited on only if its time, a quarter more,
/// comes before the job's own run would end.
pub const MARGIN: f64 = 1.25;

/// Whether a worker at `pace` is faster than the job's own run, with the margin.
pub fn beats(pace: f64) -> bool {
    pace * MARGIN < 1.0
}

/// How often the jobs wait for a worker whose pace isn't measured to take a task, at most: once an
/// hour a worker.
pub const EXPLORE_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// How many tasks' memory a kind's typical one is of.
const RECENT: usize = 20;

/// A path a worker names (an input it fetches, an output it uploads), when it stays inside the
/// folder: relative, no `..`, nothing odd.
pub fn safe(path: &str) -> Option<&Path> {
    let p = Path::new(path);
    let ok = !path.is_empty() && path.len() < 1024 && p.components().all(|c| matches!(c, Component::Normal(s) if !s.to_string_lossy().starts_with('.') || s.len() > 2));
    ok.then_some(p).filter(|p| !p.to_string_lossy().contains('\0'))
}

impl Tasks {
    pub fn new(dir: PathBuf) -> Tasks {
        // (Uploads left by a run before this one: their jobs are gone.)
        std::fs::remove_dir_all(&dir).ok();
        Tasks { next: 1, by_id: BTreeMap::new(), dir, recent_mb: BTreeMap::new(), paces: BTreeMap::new(), explored: BTreeMap::new() }
    }

    /// Takes a job's offer; its id.
    pub fn offer(&mut self, o: Offer, now: Instant) -> std::io::Result<u64> {
        let id = self.next;
        self.next += 1;
        let out = self.dir.join(id.to_string());
        std::fs::create_dir_all(&out)?;
        let recent = self.recent_mb.entry(o.kind.clone()).or_default();
        recent.push_back(o.mem_mb);
        if recent.len() > RECENT {
            recent.pop_front();
        }
        self.by_id.insert(id, Task { id, owner: o.owner, kind: o.kind, spec: o.spec, root: o.root, inputs: o.inputs, mem_mb: o.mem_mb, state: State::Offered, out, failed_on: BTreeSet::new(), offered: now, leased_at: None, wall_s: None, measuring: None });
        Ok(id)
    }

    /// What a task of `kind` takes, typically (MB): the median of the last offered (None: none
    /// was, since this coordinator started).
    pub fn typical_mb(&self, kind: &str) -> Option<u64> {
        let mut v: Vec<u64> = self.recent_mb.get(kind)?.iter().copied().collect();
        v.sort_unstable();
        v.get(v.len() / 2).copied()
    }

    /// The task to give `worker` (who does `can`, and can spare `mem_mb`): the oldest offered that
    /// fits. Never one of a kind it's measured slower at than the jobs' own runs: the job would run
    /// it at once anyway and end it, and the worker's work be thrown away (a page 8× slower at tails
    /// was given one, 8 Oct, and saw it "fail" as the job took it back). Measured again after the
    /// coordinator restarts (paces are kept in memory).
    pub fn pick(&self, worker: &str, can: &[String], mem_mb: u64) -> Option<u64> {
        let slow = |kind: &str| self.paces.get(&(worker.to_string(), kind.to_string())).is_some_and(|&p| !beats(p));
        self.by_id.values().filter(|t| matches!(t.state, State::Offered) && can.contains(&t.kind) && t.mem_mb <= mem_mb && !t.failed_on.contains(worker) && !slow(&t.kind)).min_by_key(|t| (t.offered, t.id)).map(|t| t.id)
    }

    /// Task `id`'s kind ("tail" when it's gone: the kind tasks had before there were others).
    pub fn kind_of(&self, id: u64) -> String {
        self.by_id.get(&id).map_or_else(|| "tail".to_string(), |t| t.kind.clone())
    }

    /// How many tasks of `kind` wait for a worker.
    pub fn waiting(&self, kind: &str) -> usize {
        self.by_id.values().filter(|t| t.kind == kind && matches!(t.state, State::Offered)).count()
    }

    /// The task leased as `lease`, if it's that worker's.
    pub fn by_lease(&mut self, lease: u64, worker: &str) -> Option<&mut Task> {
        self.by_id.values_mut().find(|t| matches!(&t.state, State::Leased { lease: l, worker: w } if *l == lease && w == worker))
    }

    /// An input of the task leased as `lease`: its file here and size, when the task reads it.
    pub fn input(&mut self, lease: u64, worker: &str, path: &str) -> Option<(PathBuf, u64)> {
        let t = self.by_lease(lease, worker)?;
        let size = *t.inputs.get(path)?;
        Some((t.root.join(safe(path)?), size))
    }

    /// Where an upload for the task leased as `lease` goes.
    pub fn upload(&mut self, lease: u64, worker: &str, path: &str) -> Option<PathBuf> {
        let t = self.by_lease(lease, worker)?;
        Some(t.out.join(safe(path)?))
    }

    /// The worker says it's done: every output it names is here, whole, or it isn't done. Its unit,
    /// if it has one.
    pub fn done(&mut self, lease: u64, worker: &str, outputs: Vec<Output>, removed: Vec<String>, secs: f64, peak_mb: u64) -> anyhow::Result<Option<String>> {
        let t = self.by_lease(lease, worker).ok_or_else(|| anyhow::anyhow!("no such task lease"))?;
        for o in &outputs {
            let p = t.out.join(safe(&o.path).ok_or_else(|| anyhow::anyhow!("bad path {}", o.path))?);
            let n = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(u64::MAX);
            anyhow::ensure!(n == o.size, "{}: {} bytes here, {} said", o.path, n, o.size);
        }
        anyhow::ensure!(removed.iter().all(|r| t.inputs.contains_key(r)), "it removed what it wasn't given");
        t.wall_s = t.leased_at.map(|at| at.elapsed().as_secs_f64());
        t.state = State::Done { worker: worker.to_string(), outputs, removed, secs, peak_mb };
        Ok(t.spec["unit"].as_str().map(str::to_string))
    }

    /// The worker failed it: out of memory at `oom_mb` (offered again to workers sparing more), or
    /// otherwise (not offered to it again; after two workers, failed: the job runs it).
    pub fn fail(&mut self, lease: u64, worker: &str, why: &str, oom_mb: Option<u64>) -> Option<u64> {
        self.fail_how(lease, worker, why, oom_mb, false)
    }

    /// `fail`; `interrupted`: given back, not failed (the page reloaded or closed, or killed in the
    /// background): offered again to any worker, this one too.
    pub fn fail_how(&mut self, lease: u64, worker: &str, why: &str, oom_mb: Option<u64>, interrupted: bool) -> Option<u64> {
        let t = self.by_lease(lease, worker)?;
        // (One kept to measure its worker: no more to it.)
        if t.measuring.is_some() {
            let id = t.id;
            self.close(id);
            return None;
        }
        match oom_mb {
            _ if interrupted => {}
            Some(peak) => t.mem_mb = t.mem_mb.max(peak + peak / 4),
            None => {
                t.failed_on.insert(worker.to_string());
            }
        }
        t.state = if t.failed_on.len() >= 2 { State::Failed { why: why.chars().take(2000).collect() } } else { State::Offered };
        Some(t.id)
    }

    /// The worker holding task lease `lease` went quiet: offered again, not to it.
    pub fn lapsed(&mut self, lease: u64) {
        if let Some(t) = self.by_id.values_mut().find(|t| matches!(&t.state, State::Leased { lease: l, .. } if *l == lease)) {
            if t.measuring.is_some() {
                let id = t.id;
                self.close(id);
                return;
            }
            if let State::Leased { worker, .. } = std::mem::replace(&mut t.state, State::Offered) {
                t.failed_on.insert(worker);
            }
            if t.failed_on.len() >= 2 {
                t.state = State::Failed { why: "two workers went quiet holding it".into() };
            }
        }
    }

    /// Withdraws task `id` if no one has leased it (the job runs it); whether it did.
    pub fn withdraw(&mut self, id: u64) -> bool {
        let gone = self.by_id.get(&id).is_some_and(|t| matches!(t.state, State::Offered | State::Failed { .. }));
        if gone {
            self.close(id);
        }
        gone
    }

    /// Ends task `id` (the job has what it needed from it): its uploads deleted; its lease, if any,
    /// for the coordinator to end.
    pub fn close(&mut self, id: u64) -> Option<u64> {
        let t = self.by_id.remove(&id)?;
        std::fs::remove_dir_all(&t.out).ok();
        // (One kept to measure its worker: its files moved here, `measure`.)
        if t.measuring.is_some() {
            std::fs::remove_dir_all(&t.root).ok();
        }
        match t.state {
            State::Leased { lease, .. } => Some(lease),
            _ => None,
        }
    }

    /// A worker's pace at a kind of task, when measured.
    pub fn pace(&self, worker: &str, kind: &str) -> Option<f64> {
        self.paces.get(&(worker.to_string(), kind.to_string())).copied()
    }

    /// `worker` took `wall_s` for a task of `kind` the job's own run took `here_s` for: weighed into
    /// its pace (half the last, half the new).
    pub fn note_pace(&mut self, worker: &str, kind: &str, wall_s: f64, here_s: f64) {
        if !(wall_s.is_finite() && here_s.is_finite() && wall_s > 0.0 && here_s > 0.0) {
            return;
        }
        let r = wall_s / here_s;
        let p = self.paces.entry((worker.to_string(), kind.to_string())).or_insert(r);
        *p = 0.5 * *p + 0.5 * r;
    }

    /// Whether the jobs may wait for `worker` (its pace not measured) to take a task now: not in the
    /// last hour (`EXPLORE_EVERY`).
    pub fn may_explore(&self, worker: &str, now: Instant) -> bool {
        self.explored.get(worker).is_none_or(|at| now.duration_since(*at) >= EXPLORE_EVERY)
    }

    /// A job waits for `worker` (its pace not measured) to take a task now.
    pub fn explore(&mut self, worker: &str, now: Instant) {
        self.explored.insert(worker.to_string(), now);
    }

    /// The job ran task `id` itself (in `here_s`) while its worker, whose pace at its kind isn't
    /// measured, still holds it: kept for that worker to finish, its files moved into this
    /// coordinator's folder (the job removes its own) and no longer the job's, so its time is
    /// measured. Whether it was kept.
    pub fn measure(&mut self, id: u64, here_s: f64) -> bool {
        let Some(t) = self.by_id.get_mut(&id) else { return false };
        let State::Leased { worker, .. } = &t.state else { return false };
        if self.paces.contains_key(&(worker.clone(), t.kind.clone())) || !(here_s > 0.0) {
            return false;
        }
        let root = self.dir.join(format!("{id}-in"));
        std::fs::remove_dir_all(&root).ok();
        if std::fs::rename(&t.root, &root).is_err() {
            return false;
        }
        (t.root, t.owner, t.measuring) = (root, 0, Some(here_s));
        true
    }

    /// Ends every task of job `owner` (it ended); their leases.
    pub fn close_owner(&mut self, owner: u32) -> Vec<u64> {
        let ids: Vec<u64> = self.by_id.values().filter(|t| t.owner == owner).map(|t| t.id).collect();
        ids.into_iter().filter_map(|id| self.close(id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(tasks: &mut Tasks, root: &Path, mem: u64, now: Instant) -> u64 {
        let o = Offer { owner: 9, kind: "tail".into(), spec: serde_json::json!({"unit": "6/1/1"}), root: root.to_path_buf(), inputs: [("u/a.bin".to_string(), 3)].into(), mem_mb: mem };
        tasks.offer(o, now).unwrap()
    }

    #[test]
    fn tasks_go_to_who_fits_and_come_back_whole() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("job");
        std::fs::create_dir_all(root.join("u")).unwrap();
        std::fs::write(root.join("u/a.bin"), b"abc").unwrap();
        let mut ts = Tasks::new(d.path().join("tasks"));
        let t0 = Instant::now();
        let big = offer(&mut ts, &root, 3000, t0);
        let small = offer(&mut ts, &root, 500, t0 + std::time::Duration::from_secs(1));
        let can = vec!["tail".to_string()];
        // What one takes typically: the median of those offered.
        assert_eq!((ts.typical_mb("tail"), ts.typical_mb("bldtile")), (Some(3000), None));
        // Who can spare 1 GB gets the small one; who can spare 4 GB the oldest.
        assert_eq!(ts.pick("phone", &can, 1000), Some(small));
        assert_eq!(ts.pick("ipad", &can, 4000), Some(big));
        assert_eq!(ts.pick("ipad", &["unit".to_string()], 4000), None);
        // Measured slower than the jobs at tails: given none (they'd be run here at once, its work
        // thrown away); measured faster, or not yet: as before.
        ts.paces.insert(("ipad".into(), "tail".into()), 7.8);
        assert_eq!(ts.pick("ipad", &can, 4000), None);
        ts.paces.insert(("ipad".into(), "tail".into()), 0.5);
        assert_eq!(ts.pick("ipad", &can, 4000), Some(big));
        ts.paces.remove(&("ipad".to_string(), "tail".to_string()));
        ts.by_id.get_mut(&small).unwrap().state = State::Leased { lease: 1, worker: "phone".into() };
        // Only its inputs, only to its worker, nothing outside its folder.
        assert_eq!(ts.input(1, "phone", "u/a.bin").map(|x| x.1), Some(3));
        assert!(ts.input(1, "ipad", "u/a.bin").is_none());
        assert!(ts.input(1, "phone", "u/b.bin").is_none());
        assert!(ts.upload(1, "phone", "../x").is_none() && ts.upload(1, "phone", "/etc/x").is_none());
        // Done only with every output whole.
        let up = ts.upload(1, "phone", "u/out.bin").unwrap();
        std::fs::create_dir_all(up.parent().unwrap()).unwrap();
        std::fs::write(&up, b"12345").unwrap();
        assert!(ts.done(1, "phone", vec![Output { path: "u/out.bin".into(), size: 9 }], vec![], 1.0, 400).is_err());
        assert!(ts.done(1, "phone", vec![Output { path: "u/out.bin".into(), size: 5 }], vec!["u/x".into()], 1.0, 400).is_err());
        assert_eq!(ts.done(1, "phone", vec![Output { path: "u/out.bin".into(), size: 5 }], vec!["u/a.bin".into()], 1.0, 400).unwrap().as_deref(), Some("6/1/1"));
        assert!(!ts.withdraw(small), "a done task isn't withdrawn");
        assert_eq!(ts.close(small), None);
        assert!(!up.exists());
        // Out of memory: offered again to who spares more; a failure: not to that worker again, and
        // after two workers, failed.
        ts.by_id.get_mut(&big).unwrap().state = State::Leased { lease: 2, worker: "ipad".into() };
        ts.fail(2, "ipad", "oom", Some(3600));
        assert_eq!(ts.by_id[&big].mem_mb, 4500);
        assert_eq!(ts.pick("ipad", &can, 4000), None);
        assert_eq!(ts.pick("mac", &can, 8000), Some(big));
        ts.by_id.get_mut(&big).unwrap().state = State::Leased { lease: 3, worker: "mac".into() };
        ts.lapsed(3);
        assert_eq!(ts.pick("mac", &can, 8000), None, "not to the worker that went quiet");
        ts.by_id.get_mut(&big).unwrap().state = State::Leased { lease: 4, worker: "mac2".into() };
        ts.fail(4, "mac2", "boom", None);
        assert!(matches!(ts.by_id[&big].state, State::Failed { .. }));
        assert!(ts.withdraw(big));
        // Given back (the page reloaded, or killed in the background): offered again, to it too, its
        // memory as it was.
        let back = offer(&mut ts, &root, 700, t0);
        ts.by_id.get_mut(&back).unwrap().state = State::Leased { lease: 6, worker: "ipad".into() };
        ts.fail_how(6, "ipad", "the page was reloaded or closed", None, true);
        assert_eq!((ts.pick("ipad", &can, 1000), ts.by_id[&back].mem_mb), (Some(back), 700));
        ts.by_id.remove(&back);
        // A job's tasks go with it.
        let a = offer(&mut ts, &root, 100, t0);
        ts.by_id.get_mut(&a).unwrap().state = State::Leased { lease: 5, worker: "phone".into() };
        offer(&mut ts, &root, 100, t0);
        assert_eq!(ts.close_owner(9), vec![5]);
        assert!(ts.by_id.is_empty());
    }

    #[test]
    fn a_worker_not_measured_finishes_a_task_the_job_ran_and_is_measured() {
        let d = tempfile::tempdir().unwrap();
        let mut ts = Tasks::new(d.path().join("tasks"));
        let t0 = Instant::now();
        let job = |n: &str| {
            let root = d.path().join(n);
            std::fs::create_dir_all(root.join("u")).unwrap();
            std::fs::write(root.join("u/a.bin"), b"abc").unwrap();
            root
        };
        // Leased by a worker not measured, run by the job: kept, its files the coordinator's, no
        // longer the job's.
        let id = offer(&mut ts, &job("j1"), 100, t0);
        ts.by_id.get_mut(&id).unwrap().state = State::Leased { lease: 1, worker: "ipad".into() };
        assert!(ts.measure(id, 40.0));
        assert!(!d.path().join("j1").exists());
        assert_eq!(ts.input(1, "ipad", "u/a.bin").map(|x| x.1), Some(3));
        assert!(ts.close_owner(9).is_empty());
        // It fails: closed, its files gone.
        assert_eq!(ts.fail(1, "ipad", "no", None), None);
        assert!(ts.by_id.is_empty() && !d.path().join("tasks").join(format!("{id}-in")).exists());
        // Measured: not kept; one that goes quiet: closed.
        ts.note_pace("ipad", "tail", 80.0, 40.0);
        ts.note_pace("ipad", "tail", 40.0, 40.0);
        assert_eq!(ts.pace("ipad", "tail"), Some(1.5));
        let id = offer(&mut ts, &job("j2"), 100, t0);
        ts.by_id.get_mut(&id).unwrap().state = State::Leased { lease: 2, worker: "ipad".into() };
        assert!(!ts.measure(id, 40.0));
        ts.by_id.get_mut(&id).unwrap().state = State::Leased { lease: 3, worker: "phone".into() };
        assert!(ts.measure(id, 40.0));
        ts.lapsed(3);
        assert!(ts.by_id.is_empty());
        // Waited for once an hour a worker.
        assert!(ts.may_explore("phone", t0));
        ts.explore("phone", t0);
        assert!(!ts.may_explore("phone", t0 + std::time::Duration::from_secs(3599)) && ts.may_explore("phone", t0 + EXPLORE_EVERY));
        assert!(beats(0.79) && !beats(0.8));
    }
}
