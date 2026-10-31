//! The coordinator's leases (docs/workers.md §5): which worker does which work. Timed on this
//! process's monotonic clock (another machine's clock never matters), renewed by heartbeats while
//! the work goes on (a job paused for its conditions doesn't beat), and lapsing when a worker goes
//! quiet: its work is offered again. While the build is paused every lease is held (`hold_all`),
//! beats or not, and each has a whole TTL again when it goes on. A job's leases are saved on this Mac's disk, so the agent restarting (a new app)
//! costs no worker its work; a task's die with the job that offered it.
//!
//! In the pool (docs/pool.md §7.5) a lease is also the term it was granted in (`Lease::term`: its
//! id there is `<term>-<id>`, crate::pool::journal::LeaseId), and when (`granted_at`: this Mac's
//! wall clock), so a lead after this one knows it as this one did (`snapshot`, `restore`).

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
    /// The pool's term it was granted in (0: none, the pool off).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub term: u64,
    /// When it was granted (unix seconds, this Mac's clock), kept across leads.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub granted_at: u64,
    pub worker: String,
    pub work: Work,
    /// (Restarted from the load when read back from disk.)
    #[serde(skip, default = "Instant::now")]
    pub granted: Instant,
    #[serde(skip, default = "Instant::now")]
    deadline: Instant,
    /// What the worker last said it was doing, and how far (0–1), when it says.
    pub progress: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frac: Option<f64>,
    /// What its job is doing now, as its worker's last beat said (crate::agent::jobs::Live; in
    /// memory alone).
    #[serde(skip)]
    pub live: Option<crate::agent::jobs::Live>,
    /// The threads its job was granted (0: not said): the cores the pool's own work takes on its
    /// Mac, apart from load outside the pool (crate::coord::outside_cores).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub threads: u32,
    /// The slot of its worker's Mac running its job (0: the first; crate::agent::slot_worker), as
    /// the worker last said (its beats, this Mac's loop: `Leases::at_slot`; task #863): a lease
    /// names its Mac, granted before a slot takes it. None until said (an older app's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
    /// Loaded (a restart's, a take-up's) and not yet listed by its member's beat
    /// (docs/coordination.md §4.4): its member's first beat that leaves it out releases it at once.
    #[serde(skip)]
    pub unconfirmed: bool,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

impl Lease {
    /// A job lease's targets (none for a task).
    pub fn targets(&self) -> &[(String, String)] {
        match &self.work {
            Work::Job { targets, .. } => targets,
            Work::Task { .. } => &[],
        }
    }

    /// When it lapses unless beaten again.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// How long until it lapses (its worker beating renews it).
    pub fn left(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }

    /// What it's for, in a few words.
    pub fn what(&self) -> String {
        match &self.work {
            Work::Job { step, targets } => {
                format!("{step} {}", targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" "))
            }
            Work::Task { id } => format!("task {id}"),
        }
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A target held out of every offer and start until the lead runs a newer app than the one it was
/// cancelled on (`scenic job cancel <target> --rerun-after-update`, crate::control::Cancel): its
/// step, its target, the lead's app then, who asked and when (unix seconds). A worker gets work only
/// on the lead's app (crate::coord's `app_ok`), so whichever Mac takes it then runs the newer one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub step: String,
    pub target: String,
    pub newer_than: String,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub at: u64,
}

impl Hold {
    /// Whether a lead on `app` runs the target again: an app other than the one it was held on and
    /// not an older published one ("" in tests: any).
    pub fn released(&self, app: &str) -> bool {
        app.is_empty() || (app != self.newer_than && !super::app_newer(&self.newer_than, app))
    }
}

/// A target the owner cancelled (`scenic job cancel`, crate::control::Cancel), told to the lead
/// while its plan numbered `at` or older ran or was offered (`at`: that number, and never below the
/// time in ms then, so it compares with any lead's plan numbers, crate::agent::plan's `next_n`):
/// no offer of a plan no newer has it (`Leases::unoffered`), so a plan started before the cancel
/// never grants it, whenever its offers come. Forgotten once a newer plan's offers are in
/// (`forget_cancels_before`); saved, and carried across leads, with the leases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancelled {
    pub target: String,
    /// Its step, when the cancel named one (None: every step's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    pub at: u64,
}

#[derive(Clone, Debug)]
pub struct Leases {
    next: u64,
    by_id: BTreeMap<u64, Lease>,
    /// How long a lease lasts without a heartbeat.
    pub ttl: Duration,
    /// The pool's term the leases granted now are in (0: the pool off).
    pub term: u64,
    /// Targets held until the lead runs a newer app (`Hold`): saved, and carried across leads, with
    /// the leases.
    holds: Vec<Hold>,
    /// The app this coordinator runs (crate::coord::Shared::app): what releases a hold.
    pub app: String,
    /// The Mac whose agent runs this coordinator ("" in tests: none). Its leases in `mine` never
    /// lapse: their jobs run in this process's slots, beats or not (a loop stuck on the NAS past
    /// `ttl` lost them, #666); a dead agent's are dropped as the next one starts.
    pub me: String,
    /// The leases this Mac's agent's slots run jobs under, as its loop last said
    /// (`Coordinator::keep_own`): one whose job ended, finished or not, leaves it and lapses as any.
    /// Not saved: a new agent says its own.
    pub mine: BTreeSet<u64>,
    /// The owner's cancels of running jobs (`scenic job cancel`, crate::control::Cancel), by lease:
    /// told to the lease's worker in its beat's answer (this Mac's own, `Coordinator::cancel_of`),
    /// which stops its job and gives the lease back, not failed. Each goes with its lease (finished,
    /// given back, withdrawn, dropped or lapsed); not saved.
    cancels: BTreeMap<u64, crate::control::Cancel>,
    /// The targets cancelled while a plan no newer than its `at` was the newest (`Cancelled`).
    cancelled: Vec<Cancelled>,
    /// The members (hosts, crate::agent::host_of) whose jobs' leases end only by their word
    /// (docs/coordination.md §6; crate::coord::held): never lapsing, released when their beat leaves
    /// them out, after 12 h unheard, or by the owner.
    pub by_word: BTreeSet<String>,
}

/// What's saved: the next id (ids never repeat across restarts), the jobs' leases and the holds.
#[derive(Serialize, Deserialize)]
struct Saved {
    next: u64,
    leases: Vec<Lease>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    holds: Vec<Hold>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cancelled: Vec<Cancelled>,
}

impl Default for Leases {
    fn default() -> Leases {
        Leases::new(super::TTL)
    }
}

impl Leases {
    pub fn new(ttl: Duration) -> Leases {
        Leases {
            next: 1,
            by_id: BTreeMap::new(),
            ttl,
            term: 0,
            holds: Vec::new(),
            app: String::new(),
            me: String::new(),
            mine: BTreeSet::new(),
            cancels: BTreeMap::new(),
            cancelled: Vec::new(),
            by_word: BTreeSet::new(),
        }
    }

    /// The leases saved at `path` (none when there's no file), each live for a whole `ttl` from
    /// `now`: their workers beat again once they reach this process. Ids go on from the last given,
    /// and never from below the time in milliseconds, so none is given twice even if the file is lost.
    pub fn load(path: &Path, ttl: Duration, now: Instant) -> Leases {
        let mut l = Leases::new(ttl);
        l.next = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
        if let Some(s) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Saved>(&b).ok()) {
            l.next = l.next.max(s.next);
            (l.holds, l.cancelled) = (s.holds, s.cancelled);
            for mut x in s.leases {
                (x.granted, x.deadline, x.unconfirmed) = (now, now + ttl, true);
                l.by_id.insert(x.id, x);
            }
        }
        l
    }

    /// The jobs' leases as the coordinator's store keeps them (crate::coord::kept): the next id, the
    /// holds and cancels, and each lease by its id.
    pub fn kept(&self) -> (serde_json::Value, Vec<(u64, serde_json::Value)>) {
        let Saved { next, leases, holds, cancelled } = self.saved();
        let meta = serde_json::to_value(Saved { next, leases: Vec::new(), holds, cancelled }).unwrap_or_default();
        (meta, leases.into_iter().filter_map(|l| Some((l.id, serde_json::to_value(&l).ok()?))).collect())
    }

    fn saved(&self) -> Saved {
        Saved {
            next: self.next,
            leases: self.by_id.values().filter(|l| matches!(l.work, Work::Job { .. })).cloned().collect(),
            holds: self.holds(),
            cancelled: self.cancelled.clone(),
        }
    }

    /// The jobs' leases as `save` writes them, for the pool's state per term (docs/pool.md §7.5):
    /// each with its term and when it was granted.
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::to_value(self.saved()).unwrap_or_default()
    }

    /// Takes the jobs' leases of `snapshot` (another lead's, or this one's own earlier term's), each
    /// live for a whole `ttl` from `now` (its worker beats again once it reaches this one), in
    /// place of those it had: the term's state is the leases out (one this coordinator's folder
    /// kept from a term it led before, ended since under another lead, would hold its targets out
    /// of offers until it lapsed); ids go on from the highest it knew or is given.
    pub fn restore(&mut self, v: &serde_json::Value, now: Instant) -> anyhow::Result<usize> {
        let s: Saved = serde_json::from_value(v.clone())?;
        self.by_id.clear();
        self.cancels.clear();
        self.next = self.next.max(s.next);
        (self.holds, self.cancelled) = (s.holds, s.cancelled);
        let n = s.leases.len();
        for mut x in s.leases {
            (x.granted, x.deadline, x.unconfirmed) = (now, now + self.ttl, true);
            self.next = self.next.max(x.id + 1);
            self.by_id.insert(x.id, x);
        }
        Ok(n)
    }

    /// The tasks' leases (a broker's own: kept across a take-up of another lead's jobs' leases,
    /// docs/pool.md §8).
    pub fn tasks(&self) -> Vec<Lease> {
        self.by_id.values().filter(|l| matches!(l.work, Work::Task { .. })).cloned().collect()
    }

    /// Puts a lease `tasks` gave back, as it was; false when its id is taken now (by a lease
    /// restored: it isn't put back).
    pub fn put_back(&mut self, l: Lease) -> bool {
        if self.by_id.contains_key(&l.id) {
            return false;
        }
        self.next = self.next.max(l.id + 1);
        self.by_id.insert(l.id, l);
        true
    }

    pub fn live(&self, now: Instant) -> impl Iterator<Item = &Lease> {
        self.by_id.values().filter(move |l| self.lasts(l, now))
    }

    /// Whether `l` is live at `now`: before its deadline, or one this process's slots run (`mine`).
    fn lasts(&self, l: &Lease, now: Instant) -> bool {
        l.deadline > now
            || (!self.me.is_empty() && l.worker == self.me && self.mine.contains(&l.id))
            || (matches!(l.work, Work::Job { .. }) && self.by_word.contains(crate::agent::host_of(&l.worker)))
    }

    /// `member`'s beat listing the leases it holds (docs/coordination.md §6.1; crate::coord::held):
    /// each listed confirmed; each of its jobs' leases it leaves out ended, but one granted within
    /// `grace` of `now` and not loaded since (its answer may still be on its way). Those ended, and
    /// those listed that aren't its leases here (ended meanwhile: its jobs stop). `now` is when the
    /// list was made: one granted after isn't in it, and stays.
    pub fn word(
        &mut self,
        member: &str,
        held: &BTreeSet<u64>,
        now: Instant,
        grace: Duration,
    ) -> (Vec<Lease>, Vec<u64>) {
        let of = |l: &Lease| matches!(l.work, Work::Job { .. }) && crate::agent::host_of(&l.worker) == member;
        let gone: Vec<u64> = self
            .by_id
            .values()
            .filter(|l| of(l) && !held.contains(&l.id) && l.granted <= now)
            .filter(|l| l.unconfirmed || now.saturating_duration_since(l.granted) >= grace)
            .map(|l| l.id)
            .collect();
        for id in held {
            if let Some(l) = self.by_id.get_mut(id).filter(|l| of(l)) {
                l.unconfirmed = false;
            }
        }
        // (A task's lease it lists is its too: live, not ended.)
        let its = |l: &Lease| crate::agent::host_of(&l.worker) == member;
        let unknown = held.iter().filter(|id| !self.by_id.get(id).is_some_and(its)).copied().collect();
        (self.drop_where(|l| gone.contains(&l.id)), unknown)
    }

    /// Every target of `step` held now, by anyone, or until a newer app (`Hold`).
    pub fn held(&self, step: &str, now: Instant) -> BTreeSet<String> {
        self.live(now)
            .filter(|l| matches!(&l.work, Work::Job { step: s, .. } if s == step))
            .flat_map(|l| l.targets().iter().map(|t| t.0.clone()))
            .chain(self.holds.iter().filter(|h| h.step == step && !h.released(&self.app)).map(|h| h.target.clone()))
            .collect()
    }

    /// `target` of `step` held until the lead runs a newer app than this one's (`Hold`), asked by
    /// `by` at `at` (unix seconds); the hold.
    pub fn hold_for_update(&mut self, step: &str, target: &str, by: &str, at: u64) -> Hold {
        self.holds.retain(|h| !(h.step == step && h.target == target));
        let h = Hold {
            step: step.to_string(),
            target: target.to_string(),
            newer_than: self.app.clone(),
            by: by.to_string(),
            at,
        };
        self.holds.push(h.clone());
        h
    }

    /// Lease `id`'s job cancelled by the owner (`cancels`), while the lease stands.
    pub fn ask_cancel(&mut self, id: u64, c: crate::control::Cancel) {
        if self.by_id.contains_key(&id) {
            self.cancels.insert(id, c);
        }
    }

    /// `target` (of `step`, None: of every step) cancelled while plan `at` was the newest
    /// (`Cancelled`).
    pub fn note_cancel(&mut self, target: &str, step: Option<String>, at: u64) {
        self.cancelled.retain(|c| !(c.target == target && c.step == step));
        self.cancelled.push(Cancelled { target: target.to_string(), step, at });
    }

    /// The cancels a plan numbered `plan` started after (`at` below it): forgotten, that plan's
    /// offers knowing of them. True when one was.
    pub fn forget_cancels_before(&mut self, plan: u64) -> bool {
        let n = self.cancelled.len();
        self.cancelled.retain(|c| c.at >= plan);
        self.cancelled.len() < n
    }

    /// The cancels standing (`Cancelled`).
    pub fn cancelled(&self) -> &[Cancelled] {
        &self.cancelled
    }

    /// Offer `o`, of plan `plan`, without what was cancelled after that plan began (`Cancelled`):
    /// a shared step's offer without those targets, the lead's own planned job (`Offer::job`, whole)
    /// not at all; None when nothing of it is left.
    pub fn unoffered(&self, plan: u64, o: &super::Offer) -> Option<super::Offer> {
        let cut = |t: &str| {
            self.cancelled.iter().any(|c| c.target == t && plan <= c.at && c.step.as_ref().is_none_or(|s| *s == o.step))
        };
        let mut o = o.clone();
        if o.job.is_some() && o.targets.iter().any(|t| cut(&t.0)) {
            return None;
        }
        o.targets.retain(|t| !cut(&t.0));
        (!o.targets.is_empty()).then_some(o)
    }

    /// The owner's cancel of lease `id`, if there's one.
    pub fn cancel_ask(&self, id: u64) -> Option<&crate::control::Cancel> {
        self.cancels.get(&id)
    }

    /// The holds standing now (those the app releases left out).
    pub fn holds(&self) -> Vec<Hold> {
        self.holds.iter().filter(|h| !h.released(&self.app)).cloned().collect()
    }

    /// Which leases workers other than `me` hold, as a number that changes whenever one is
    /// granted, finished, given back, dropped or lapses (ids never repeat): what an idle agent's
    /// plan digest folds in. `me`'s own (a start taking its lease, then letting it go) left out.
    pub fn generation(&self, me: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.by_id.values().filter(|l| l.worker != me).map(|l| l.id).collect::<Vec<_>>().hash(&mut h);
        h.finish()
    }

    /// A lease of `work` to `worker`; its id. (The caller makes sure no one else holds it.)
    /// The id the next grant takes: every lease granted so far has a smaller one.
    pub fn next(&self) -> u64 {
        self.next
    }

    pub fn grant(&mut self, worker: &str, work: Work, now: Instant) -> u64 {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.grant_at(worker, work, now, unix)
    }

    /// `grant`, its `granted_at` the unix second given (the scheduler's clock, crate::coord::sched).
    pub fn grant_at(&mut self, worker: &str, work: Work, now: Instant, granted_at: u64) -> u64 {
        let id = self.next;
        self.next += 1;
        self.by_id.insert(
            id,
            Lease {
                id,
                term: self.term,
                granted_at,
                worker: worker.to_string(),
                work,
                granted: now,
                deadline: now + self.ttl,
                progress: None,
                frac: None,
                live: None,
                slot: None,
                threads: 0,
                unconfirmed: false,
            },
        );
        id
    }

    /// Notes the threads lease `id`'s job was granted.
    pub fn set_threads(&mut self, id: u64, threads: u32) {
        if let Some(l) = self.by_id.get_mut(&id) {
            l.threads = threads;
        }
    }

    /// Lease `id`, live or not yet expired, whoever holds it.
    pub fn of(&self, id: u64) -> Option<&Lease> {
        self.by_id.get(&id)
    }

    /// `worker`'s live lease `id`, if it is one.
    pub fn get(&self, id: u64, worker: &str, now: Instant) -> Option<&Lease> {
        self.by_id.get(&id).filter(|l| l.worker == worker && self.lasts(l, now))
    }

    /// Renews `worker`'s lease `id`; false when it isn't that worker's live lease (lapsed, finished,
    /// or never granted: the worker should stop that work).
    pub fn renew(&mut self, id: u64, worker: &str, progress: Option<String>, now: Instant) -> bool {
        self.renew_frac(id, worker, progress, None, None, now)
    }

    /// `renew`, with how far the work is (0–1) and what its job is doing now, when the worker says.
    pub fn renew_frac(
        &mut self,
        id: u64,
        worker: &str,
        progress: Option<String>,
        frac: Option<f64>,
        live: Option<crate::agent::jobs::Live>,
        now: Instant,
    ) -> bool {
        let lasts = self.by_id.get(&id).is_some_and(|l| self.lasts(l, now));
        match self.by_id.get_mut(&id) {
            Some(l) if l.worker == worker && lasts => {
                l.deadline = now + self.ttl;
                if progress.is_some() {
                    l.progress = progress;
                }
                if let Some(f) = frac.filter(|f| f.is_finite()) {
                    l.frac = Some(f.clamp(0.0, 1.0));
                }
                if live.is_some() {
                    l.live = live;
                }
                true
            }
            _ => false,
        }
    }

    /// Lease `id` of `worker` runs on its Mac's slot `slot` (task #863).
    pub fn at_slot(&mut self, id: u64, worker: &str, slot: u32) {
        if let Some(l) = self.by_id.get_mut(&id).filter(|l| l.worker == worker) {
            l.slot = Some(slot);
        }
    }

    /// Ends `worker`'s live lease `id` (done or given back); the lease, if it was that.
    pub fn finish(&mut self, id: u64, worker: &str, now: Instant) -> Option<Lease> {
        match self.by_id.get(&id) {
            Some(l) if l.worker == worker && self.lasts(l, now) => {
                self.cancels.remove(&id);
                self.by_id.remove(&id)
            }
            _ => None,
        }
    }

    /// Ends lease `id` whoever holds it (its task withdrawn); the lease, if there was one.
    pub fn cancel(&mut self, id: u64) -> Option<Lease> {
        self.cancels.remove(&id);
        self.by_id.remove(&id)
    }

    /// Ends every lease `gone` picks (this Mac's own, when its agent starts: its jobs ended with the
    /// last one); those ended.
    pub fn drop_where(&mut self, gone: impl Fn(&Lease) -> bool) -> Vec<Lease> {
        let ids: Vec<u64> = self.by_id.values().filter(|l| gone(l)).map(|l| l.id).collect();
        for id in &ids {
            self.cancels.remove(id);
        }
        ids.into_iter().filter_map(|id| self.by_id.remove(&id)).collect()
    }

    /// Every lease held for a whole `ttl` from `now`, beats or not: while the build is paused (no work
    /// goes to another meanwhile), and once more as it goes on.
    pub fn hold_all(&mut self, now: Instant) {
        for l in self.by_id.values_mut() {
            l.deadline = l.deadline.max(now + self.ttl);
        }
    }

    /// Every lease past its deadline, removed (but those this process's slots run, `mine`): their work is
    /// offered again.
    pub fn expire(&mut self, now: Instant) -> Vec<Lease> {
        let gone: Vec<u64> = self.by_id.values().filter(|l| !self.lasts(l, now)).map(|l| l.id).collect();
        self.drop_where(|l| gone.contains(&l.id))
    }

    /// Every live lease, for the status.
    pub fn all(&self, now: Instant) -> Vec<Lease> {
        self.live(now).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_leases_outlast_their_beats() {
        // While the build is paused every lease is held, beats or not: none lapses, whatever time
        // passes between holds; one not held lapses as before.
        let now = Instant::now();
        let mut l = Leases::new(Duration::from_millis(60));
        let a = l.grant("m1", Work::Job { step: "unit".into(), targets: vec![("6/1/1".into(), "k".into())] }, now);
        for i in 1..=4 {
            l.hold_all(now + Duration::from_millis(50 * i));
        }
        let later = now + Duration::from_millis(220);
        assert!(l.get(a, "m1", later).is_some() && l.expire(later).is_empty());
        assert!(l.renew(a, "m1", None, later));
        assert_eq!(l.expire(later + Duration::from_millis(100)).len(), 1);
    }

    fn job(ts: &[(&str, &str)]) -> Work {
        Work::Job { step: "unit".into(), targets: ts.iter().map(|(t, k)| (t.to_string(), k.to_string())).collect() }
    }

    #[test]
    fn leases_hold_renew_lapse_and_survive_a_restart() {
        let t0 = Instant::now();
        let mut l = Leases::new(Duration::from_secs(600));
        let a = l.grant("m1", job(&[("6/1/1", "k1"), ("6/1/2", "k2")]), t0);
        let b = l.grant("m4", job(&[("6/2/2", "k3")]), t0);
        let t = l.grant("m2", Work::Task { id: 7 }, t0);
        assert_eq!(l.held("unit", t0), ["6/1/1", "6/1/2", "6/2/2"].map(String::from).into());
        assert!(l.held("pack", t0).is_empty());
        // Renewed by its own worker only.
        let t1 = t0 + Duration::from_secs(500);
        assert!(l.renew(a, "m1", Some("2/6 z3 tiles".into()), t1));
        assert!(!l.renew(a, "m4", None, t1));
        assert!(l.get(a, "m1", t1).is_some() && l.get(a, "m4", t1).is_none());
        // Past ttl without a beat: b and the task lapse, a (renewed) lives on.
        let t2 = t0 + Duration::from_secs(700);
        assert_eq!(l.held("unit", t2), ["6/1/1", "6/1/2"].map(String::from).into());
        assert!(!l.renew(b, "m4", None, t2), "a lapsed lease can't be renewed");
        assert!(l.finish(b, "m4", t2).is_none(), "nor finished");
        assert_eq!(l.expire(t2).iter().map(|x| x.id).collect::<Vec<_>>(), [b, t]);
        // Kept (the coordinator's store) and read back: the job's lease, live again for a whole ttl;
        // ids go on.
        l.grant("m2", Work::Task { id: 8 }, t2);
        let (mut kept, each) = l.kept();
        kept["leases"] = each.into_iter().map(|x| x.1).collect();
        let t3 = t2 + Duration::from_secs(5000);
        let mut back = Leases::new(Duration::from_secs(600));
        back.restore(&kept, t3).unwrap();
        assert_eq!(
            back.all(t3).iter().map(|x| (x.id, x.worker.as_str())).collect::<Vec<_>>(),
            [(a, "m1")],
            "tasks aren't saved"
        );
        assert_eq!(back.all(t3)[0].progress.as_deref(), Some("2/6 z3 tiles"));
        assert!(back.grant("m1", job(&[("6/3/3", "k4")]), t3) > t + 1);
        // Finished by its worker only; this Mac's own dropped at a restart.
        assert!(back.finish(a, "m4", t3).is_none());
        assert_eq!(back.drop_where(|x| x.worker == "m1").len(), 2);
        assert!(back.all(t3).is_empty());
    }

    /// docs/coordination.md §6.1: a member speaking for its leases keeps them beat or not; its beat
    /// leaving one out ends it (but one granted within the grace, its answer maybe on its way), a
    /// loaded one at once; one it lists that isn't its is said.
    #[test]
    fn leases_end_by_their_members_word() {
        let t0 = Instant::now();
        let mut l = Leases::new(Duration::from_secs(600));
        l.by_word.insert("m1".into());
        let a = l.grant("m1", job(&[("6/1/1", "k1")]), t0);
        let b = l.grant("m1 #2", job(&[("6/1/2", "k2")]), t0);
        let c = l.grant("m4", job(&[("6/1/3", "k3")]), t0);
        let far = t0 + Duration::from_secs(3600);
        assert_eq!(l.held("unit", far), ["6/1/1", "6/1/2"].map(String::from).into(), "m1's never lapse; m4's do");
        let grace = Duration::from_secs(30);
        let (gone, unknown) = l.word("m1", &[a, c].into(), t0 + Duration::from_secs(5), grace);
        assert!(gone.is_empty(), "b granted within the grace");
        assert_eq!(unknown, [c], "not m1's");
        // (A task's lease it lists is its, live: not said ended, nor ended by leaving it out.)
        let t = l.grant("m1 #3", Work::Task { id: 9 }, t0);
        let (gone, unknown) = l.word("m1", &[a, b, t].into(), t0 + Duration::from_secs(5), grace);
        assert!(gone.is_empty() && unknown.is_empty(), "{unknown:?}");
        let (gone, _) = l.word("m1", &[a, b].into(), far, grace);
        assert!(gone.is_empty() && l.of(t).is_some(), "a task's lease isn't the beat's to end");
        let (gone, _) = l.word("m1", &[a].into(), far, grace);
        assert_eq!(gone.iter().map(|x| x.id).collect::<Vec<_>>(), [b]);
        // Loaded: unconfirmed, left out at once.
        let (mut kept, each) = l.kept();
        kept["leases"] = each.into_iter().map(|x| x.1).collect();
        let mut back = Leases::new(Duration::from_secs(600));
        back.restore(&kept, far).unwrap();
        assert!(back.of(a).unwrap().unconfirmed);
        let (gone, _) = back.word("m1", &BTreeSet::new(), far, grace);
        assert_eq!(gone.iter().map(|x| x.id).collect::<Vec<_>>(), [a]);
    }

    #[test]
    fn a_lease_says_its_term_and_another_lead_takes_it_up() {
        // (docs/pool.md §7.5: leases `<term>-<n>`, wall-clock times, saved per term.)
        let t0 = Instant::now();
        let mut a = Leases::new(Duration::from_secs(600));
        a.term = 4;
        let id = a.grant("m1", job(&[("6/1/1", "k1")]), t0);
        a.grant("m2", Work::Task { id: 1 }, t0);
        let l = a.of(id).unwrap().clone();
        assert_eq!(l.term, 4);
        assert!(l.granted_at > 1_700_000_000);
        let snap = a.snapshot();
        assert!(snap.to_string().contains("\"term\":4"), "{snap}");
        // Another lead, in term 5: its jobs' leases (not the task's), each live a whole ttl; its own
        // ids go on past them.
        let t1 = t0 + Duration::from_secs(900);
        let mut b = Leases::new(Duration::from_secs(600));
        b.term = 5;
        assert_eq!(b.restore(&snap, t1).unwrap(), 1);
        assert!(b.get(id, "m1", t1).is_some() && b.held("unit", t1).contains("6/1/1"));
        assert_eq!((b.of(id).unwrap().term, b.of(id).unwrap().granted_at), (4, l.granted_at));
        let mine = b.grant("m4", job(&[("6/2/2", "k2")]), t1);
        assert!(mine > id && b.of(mine).unwrap().term == 5);
        assert!(b.restore(&serde_json::json!({"leases": 3}), t1).is_err());
    }
}
