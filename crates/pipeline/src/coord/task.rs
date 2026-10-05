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
}

/// A task as a job offers it (`POST /task/offer`).
#[derive(Debug, Serialize, Deserialize)]
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
}

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
        Tasks { next: 1, by_id: BTreeMap::new(), dir }
    }

    /// Takes a job's offer; its id.
    pub fn offer(&mut self, o: Offer, now: Instant) -> std::io::Result<u64> {
        let id = self.next;
        self.next += 1;
        let out = self.dir.join(id.to_string());
        std::fs::create_dir_all(&out)?;
        self.by_id.insert(id, Task { id, owner: o.owner, kind: o.kind, spec: o.spec, root: o.root, inputs: o.inputs, mem_mb: o.mem_mb, state: State::Offered, out, failed_on: BTreeSet::new(), offered: now });
        Ok(id)
    }

    /// The task to give `worker` (who does `can`, and can spare `mem_mb`): the oldest offered that
    /// fits.
    pub fn pick(&self, worker: &str, can: &[String], mem_mb: u64) -> Option<u64> {
        self.by_id.values().filter(|t| matches!(t.state, State::Offered) && can.contains(&t.kind) && t.mem_mb <= mem_mb && !t.failed_on.contains(worker)).min_by_key(|t| (t.offered, t.id)).map(|t| t.id)
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
        match t.state {
            State::Leased { lease, .. } => Some(lease),
            _ => None,
        }
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
        // Who can spare 1 GB gets the small one; who can spare 4 GB the oldest.
        assert_eq!(ts.pick("phone", &can, 1000), Some(small));
        assert_eq!(ts.pick("ipad", &can, 4000), Some(big));
        assert_eq!(ts.pick("ipad", &["unit".to_string()], 4000), None);
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
}
