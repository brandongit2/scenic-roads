//! The coordinator's leases (docs/workers.md §5): which worker does which work. Timed on this
//! process's monotonic clock (another machine's clock never matters), renewed by heartbeats while
//! the work goes on (a paused job doesn't beat), and lapsing when a worker goes quiet: its work is
//! offered again. A job's leases are saved on this Mac's disk, so the agent restarting (a new app)
//! costs no worker its work; a task's die with the job that offered it.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// What a lease is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Work {
    /// A job of the plan: its step, and its targets with their keys (build::Keys::record).
    Job { step: String, targets: Vec<(String, String)> },
    /// A task a running job offered (its id in coord::task::Tasks).
    Task { id: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lease {
    pub id: u64,
    pub worker: String,
    pub work: Work,
    /// (Restarted from the load when read back from disk.)
    #[serde(skip, default = "Instant::now")]
    pub granted: Instant,
    #[serde(skip, default = "Instant::now")]
    deadline: Instant,
    /// What the worker last said it was doing.
    pub progress: Option<String>,
}

impl Lease {
    /// A job lease's targets (none for a task).
    pub fn targets(&self) -> &[(String, String)] {
        match &self.work {
            Work::Job { targets, .. } => targets,
            Work::Task { .. } => &[],
        }
    }

    /// What it's for, in a few words.
    pub fn what(&self) -> String {
        match &self.work {
            Work::Job { step, targets } => format!("{step} {}", targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" ")),
            Work::Task { id } => format!("task {id}"),
        }
    }
}

#[derive(Debug)]
pub struct Leases {
    next: u64,
    by_id: BTreeMap<u64, Lease>,
    /// How long a lease lasts without a heartbeat.
    pub ttl: Duration,
}

/// What's saved: the next id (ids never repeat across restarts) and the jobs' leases.
#[derive(Serialize, Deserialize)]
struct Saved {
    next: u64,
    leases: Vec<Lease>,
}

impl Leases {
    pub fn new(ttl: Duration) -> Leases {
        Leases { next: 1, by_id: BTreeMap::new(), ttl }
    }

    /// The leases saved at `path` (none when there's no file), each live for a whole `ttl` from
    /// `now`: their workers beat again once they reach this process. Ids go on from the last given,
    /// and never from below the time in milliseconds, so none is given twice even if the file is lost.
    pub fn load(path: &Path, ttl: Duration, now: Instant) -> Leases {
        let mut l = Leases::new(ttl);
        l.next = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
        if let Some(s) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Saved>(&b).ok()) {
            l.next = l.next.max(s.next);
            for mut x in s.leases {
                (x.granted, x.deadline) = (now, now + ttl);
                l.by_id.insert(x.id, x);
            }
        }
        l
    }

    /// Saves the jobs' leases (whole: a crash never leaves half a file).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let leases: Vec<Lease> = self.by_id.values().filter(|l| matches!(l.work, Work::Job { .. })).cloned().collect();
        crate::whole::write(path, &serde_json::to_vec_pretty(&Saved { next: self.next, leases })?)
    }

    fn live(&self, now: Instant) -> impl Iterator<Item = &Lease> {
        self.by_id.values().filter(move |l| l.deadline > now)
    }

    /// Every target of `step` held now, by anyone.
    pub fn held(&self, step: &str, now: Instant) -> BTreeSet<String> {
        self.live(now).filter(|l| matches!(&l.work, Work::Job { step: s, .. } if s == step)).flat_map(|l| l.targets().iter().map(|t| t.0.clone())).collect()
    }

    /// A lease of `work` to `worker`; its id. (The caller makes sure no one else holds it.)
    pub fn grant(&mut self, worker: &str, work: Work, now: Instant) -> u64 {
        let id = self.next;
        self.next += 1;
        self.by_id.insert(id, Lease { id, worker: worker.to_string(), work, granted: now, deadline: now + self.ttl, progress: None });
        id
    }

    /// `worker`'s live lease `id`, if it is one.
    pub fn get(&self, id: u64, worker: &str, now: Instant) -> Option<&Lease> {
        self.by_id.get(&id).filter(|l| l.worker == worker && l.deadline > now)
    }

    /// Renews `worker`'s lease `id`; false when it isn't that worker's live lease (lapsed, finished,
    /// or never granted: the worker should stop that work).
    pub fn renew(&mut self, id: u64, worker: &str, progress: Option<String>, now: Instant) -> bool {
        match self.by_id.get_mut(&id) {
            Some(l) if l.worker == worker && l.deadline > now => {
                l.deadline = now + self.ttl;
                if progress.is_some() {
                    l.progress = progress;
                }
                true
            }
            _ => false,
        }
    }

    /// Ends `worker`'s live lease `id` (done or given back); the lease, if it was that.
    pub fn finish(&mut self, id: u64, worker: &str, now: Instant) -> Option<Lease> {
        match self.by_id.get(&id) {
            Some(l) if l.worker == worker && l.deadline > now => self.by_id.remove(&id),
            _ => None,
        }
    }

    /// Ends lease `id` whoever holds it (its task withdrawn); the lease, if there was one.
    pub fn cancel(&mut self, id: u64) -> Option<Lease> {
        self.by_id.remove(&id)
    }

    /// Ends every lease `gone` picks (this Mac's own, when its agent starts: its jobs ended with the
    /// last one); those ended.
    pub fn drop_where(&mut self, gone: impl Fn(&Lease) -> bool) -> Vec<Lease> {
        let ids: Vec<u64> = self.by_id.values().filter(|l| gone(l)).map(|l| l.id).collect();
        ids.into_iter().filter_map(|id| self.by_id.remove(&id)).collect()
    }

    /// Every lease past its deadline, removed: their work is offered again.
    pub fn expire(&mut self, now: Instant) -> Vec<Lease> {
        self.drop_where(|l| l.deadline <= now)
    }

    /// Every live lease, for the status.
    pub fn all(&self, now: Instant) -> Vec<Lease> {
        self.live(now).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(ts: &[(&str, &str)]) -> Work {
        Work::Job { step: "unit".into(), targets: ts.iter().map(|(t, k)| (t.to_string(), k.to_string())).collect() }
    }

    #[test]
    fn leases_hold_renew_lapse_and_survive_a_restart() {
        let t0 = Instant::now();
        let mut l = Leases::new(Duration::from_secs(600));
        let a = l.grant("m1", job(&[("6/1/1", "k1"), ("6/1/2", "k2")]), t0);
        let b = l.grant("m4", job(&[("6/2/2", "k3")]), t0);
        let t = l.grant("ipad", Work::Task { id: 7 }, t0);
        assert_eq!(l.held("unit", t0), ["6/1/1", "6/1/2", "6/2/2"].map(String::from).into());
        assert!(l.held("pack", t0).is_empty());
        // Renewed by its own worker only.
        let t1 = t0 + Duration::from_secs(500);
        assert!(l.renew(a, "m1", Some("2/6 areas".into()), t1));
        assert!(!l.renew(a, "m4", None, t1));
        assert!(l.get(a, "m1", t1).is_some() && l.get(a, "m4", t1).is_none());
        // Past ttl without a beat: b and the task lapse, a (renewed) lives on.
        let t2 = t0 + Duration::from_secs(700);
        assert_eq!(l.held("unit", t2), ["6/1/1", "6/1/2"].map(String::from).into());
        assert!(!l.renew(b, "m4", None, t2), "a lapsed lease can't be renewed");
        assert!(l.finish(b, "m4", t2).is_none(), "nor finished");
        assert_eq!(l.expire(t2).iter().map(|x| x.id).collect::<Vec<_>>(), [b, t]);
        // Saved and read back: the job's lease, live again for a whole ttl; ids go on.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("leases.json");
        l.grant("ipad", Work::Task { id: 8 }, t2);
        l.save(&p).unwrap();
        let t3 = t2 + Duration::from_secs(5000);
        let mut back = Leases::load(&p, Duration::from_secs(600), t3);
        assert_eq!(back.all(t3).iter().map(|x| (x.id, x.worker.as_str())).collect::<Vec<_>>(), [(a, "m1")], "tasks aren't saved");
        assert_eq!(back.all(t3)[0].progress.as_deref(), Some("2/6 areas"));
        assert!(back.grant("m1", job(&[("6/3/3", "k4")]), t3) > t + 1);
        // Finished by its worker only; this Mac's own dropped at a restart.
        assert!(back.finish(a, "m4", t3).is_none());
        assert_eq!(back.drop_where(|x| x.worker == "m1").len(), 2);
        assert!(back.all(t3).is_empty());
    }
}
