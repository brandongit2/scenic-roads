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
use std::time::{Duration, Instant};

/// What a lease is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Work {
    /// A job of the plan: its step, and its targets with their keys (pool::records::Keys::record).
    Job { step: String, targets: Vec<(String, String)> },
    /// A task a running job offered (its id in coord::task::Tasks).
    Task { id: u64 },
}

/// How far back a leased job's recent NAS rate looks, at least (`Lease::window`).
pub const WINDOW: Duration = Duration::from_secs(60);

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
    /// When its job began, when that's before its grant or said (`began`; in memory alone): as its
    /// worker's last beat said it (crate::agent::jobs::Live::ran_s, its process's start carried
    /// through a take-over), else, for a lease taken up (read back at a lead's restart, a term's
    /// state, a record), its grant by the wall clock (`granted_at`). None: its grant.
    #[serde(skip)]
    began: Option<Instant>,
    #[serde(skip, default = "Instant::now")]
    deadline: Instant,
    /// What the worker last said it was doing (its job's progress line), when it says.
    pub progress: Option<String>,
    /// What its job is doing now, as its worker's last beat said (crate::agent::jobs::Live; in
    /// memory alone).
    #[serde(skip)]
    pub live: Option<crate::agent::jobs::Live>,
    /// When `live` was said, and earlier beats' views back to the newest one said `WINDOW` or more
    /// before it (its front, once one is): what its job moved lately is the difference
    /// (`Lease::lately`, crate::coord::sched's `job_weight`; in memory alone).
    #[serde(skip)]
    pub live_at: Option<Instant>,
    #[serde(skip)]
    pub window: std::collections::VecDeque<(Instant, crate::agent::jobs::Live)>,
    /// The first live view it heard that says all its job moved (`Live::nas_moved`) and its CPU
    /// time, and when: what its job's mean NAS rate and busy cores are reckoned from (`Lease::mean`,
    /// `Lease::cores_mean`; in memory alone). One saying less is replaced by the next that says
    /// more, so a mean is had whenever a recent rate is (`Lease::lately`, `Lease::cores_lately`).
    #[serde(skip)]
    pub first: Option<(Instant, crate::agent::jobs::Live)>,
    /// Its job's stillness as its worker's beats show it (docs/pool.md §10, Flags; in memory
    /// alone, a lead restarted counts from its load): the last beat heard, the last at which its
    /// work moved (None: none yet, counted from its grant: `Lease::still`), and the longest stretch
    /// between two such so far (s).
    #[serde(skip)]
    pub heard: Option<Instant>,
    #[serde(skip)]
    pub moved: Option<Instant>,
    #[serde(skip)]
    pub still_most: u64,
    /// Its job's progress as its worker's beats said it (`Pace`; in memory alone).
    #[serde(skip)]
    pub pace: Pace,
    /// The threads its job is counted for (0: not said): granted, then as re-granted (docs/pool.md
    /// §7.1, Threads re-granted: the most of what it runs on now, what its agent has told it and
    /// what it is still to be told), the cores the pool's own work takes on its Mac, apart from load
    /// outside the pool (crate::coord::outside_cores).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub threads: u32,
    /// The threads it was granted as it started (0: not said, a lease from before re-grants): the
    /// memory its grant was admitted with covers them; a re-grant past them adds its own
    /// (crate::coord::sched's `grown_mb`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub start_threads: u32,
    /// A re-grant its agent hasn't told its job yet (crate::coord::sched's `regrant`): the threads,
    /// and when they were decided (in memory alone: a lead restarted decides again).
    #[serde(skip)]
    pub told: Option<(u32, Instant)>,
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
    /// When its job began (`began`): its grant, unless its job is older or its worker said.
    pub fn began(&self) -> Instant {
        self.began.unwrap_or(self.granted)
    }

    /// How long its job has run to `now` (from `began`): its run, which its lease may be younger
    /// than (task #992: a job's "done" event put the time since its lease was read back at a
    /// lead's restart, 326 of the lead's 5,830 to 6 Nov, a median quarter of their runs).
    pub fn ran(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.began())
    }

    /// Its job began before its lease (taken up at a lead's restart or from a term's state, or held
    /// again for a job running at its agent's take-over): what its beats measure is of part of
    /// its run, the lead not having heard the rest.
    pub fn taken_up(&self) -> bool {
        self.began.is_some_and(|b| b < self.granted)
    }

    /// Its job's NAS rate over its last `WINDOW` or so of beats: None until a view said `WINDOW`
    /// before the newest, or when one doesn't say all it moved (crate::agent::jobs::Live::nas_rate).
    pub fn lately(&self) -> Option<crate::budget::nas::Rate> {
        self.rate_since(self.window.front())
    }

    /// Its job's mean NAS rate since the first view its lease heard (`first`): what it moved from
    /// that view to the newest over the time between them. A view's counts run from its job's
    /// start, which may be long before its lease (one taken up at a lead's restart, or held again
    /// at an agent's take-over), so they're never put over the lease's own time (task #991: a
    /// 5 MB/s download taken over five hours in weighed 56 MB/s, two thirds of the NAS). None
    /// until the two are `WINDOW` apart, or when one doesn't say all it moved.
    pub fn mean(&self) -> Option<crate::budget::nas::Rate> {
        self.rate_since(self.first.as_ref())
    }

    /// The cores its job kept busy over its last `WINDOW` or so of beats (its CPU time over that
    /// time, crate::agent::jobs::Live::cores_since): None as `lately`, or where a view doesn't say
    /// its CPU time.
    pub fn cores_lately(&self) -> Option<f64> {
        self.since(self.window.front(), crate::agent::jobs::Live::cores_since)
    }

    /// The cores its job kept busy since the first view its lease heard (`first`), as `mean`.
    pub fn cores_mean(&self) -> Option<f64> {
        self.since(self.first.as_ref(), crate::agent::jobs::Live::cores_since)
    }

    /// What its job moved from view `from` to the newest, a second, the two `WINDOW` or more apart.
    fn rate_since(&self, from: Option<&(Instant, crate::agent::jobs::Live)>) -> Option<crate::budget::nas::Rate> {
        self.since(from, crate::agent::jobs::Live::nas_rate)
    }

    /// `of` the newest view, view `from` and the seconds between them, the two `WINDOW` or more apart.
    fn since<T>(
        &self,
        from: Option<&(Instant, crate::agent::jobs::Live)>,
        of: impl Fn(&crate::agent::jobs::Live, &crate::agent::jobs::Live, f64) -> Option<T>,
    ) -> Option<T> {
        let (live, at) = (self.live.as_ref()?, self.live_at?);
        let (t, base) = from.filter(|(t, _)| at.saturating_duration_since(*t) >= WINDOW)?;
        of(live, base, at.saturating_duration_since(*t).as_secs_f64())
    }

    /// How long its job's work has not moved, by the beats heard: from the last beat at which it
    /// moved (its grant, before one) to the last heard; None before any beat (no word of it either
    /// way). A beat not heard says nothing: a worker out of touch is its own warning.
    pub fn still(&self) -> Option<Duration> {
        Some(self.heard?.saturating_duration_since(self.moved.unwrap_or(self.granted)))
    }

    /// The longest its job went with nothing moving, to `end` (its end: a move of its own), in
    /// whole seconds: what its step's runs are held to (crate::coord::history::History::still_p90s).
    pub fn still_most(&self, end: Instant) -> u64 {
        self.still_most.max(end.saturating_duration_since(self.moved.unwrap_or(self.granted)).as_secs())
    }

    /// `e` (its job's "done" or "end" event) with what its run measured to `end`: its longest still
    /// stretch (`still_most`), its rate in each unit its progress said, its longest wait for its
    /// progress to go on and the most threads it had meanwhile (`Pace`): what its step's runs are
    /// held to (crate::coord::history::History). None of them for a lease `taken_up`: they'd be of
    /// the part of its run since, its longest stretches cut short.
    pub fn noted(&self, e: crate::coord::history::Event, end: Instant) -> crate::coord::history::Event {
        if self.taken_up() {
            return e;
        }
        crate::coord::history::Event {
            still_s: Some(self.still_most(end)),
            gap_s: self.pace.gap_most(end),
            rates: self.pace.rates(),
            threads: self.pace.threads_most(),
            ..e
        }
    }

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

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// When a job began at unix second `at` (0: not said) began on this process's clock, `unix` being
/// `now`: None when not said or not before now (its grant then).
fn began_by_wall(at: u64, unix: u64, now: Instant) -> Option<Instant> {
    (at > 0 && at < unix).then(|| now.checked_sub(Duration::from_secs(unix - at))).flatten()
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
    /// The jobs' leases granted since the coordinator last took them (`take_granted`: worker, term,
    /// id), for GC's queue: each may be handed off under until its member says otherwise
    /// (crate::coord::held::tell_gc). Not saved: a lease out at a restart is among those loaded.
    granted: Vec<(String, u64, u64)>,
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
            granted: Vec::new(),
        }
    }

    /// The leases `saved` (a `leases.json` read; none without one), each live for a whole `ttl`
    /// from `now`: their workers beat again once they reach this process. Ids go on from the last
    /// given, and never from below the time in milliseconds, so none is given twice even if the
    /// file is lost.
    pub fn load(saved: Option<Vec<u8>>, ttl: Duration, now: Instant) -> Leases {
        let mut l = Leases::new(ttl);
        l.next = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
        if let Some(s) = saved.and_then(|b| serde_json::from_slice::<Saved>(&b).ok()) {
            l.next = l.next.max(s.next);
            (l.holds, l.cancelled) = (s.holds, s.cancelled);
            let unix = unix_now();
            for mut x in s.leases {
                (x.granted, x.deadline, x.unconfirmed) = (now, now + ttl, true);
                x.began = began_by_wall(x.granted_at, unix, now);
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
        let unix = unix_now();
        for mut x in s.leases {
            (x.granted, x.deadline, x.unconfirmed) = (now, now + self.ttl, true);
            x.began = began_by_wall(x.granted_at, unix, now);
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
        self.grant_at(worker, work, now, unix_now())
    }

    /// Lease `id`, granted at `now` for a job that began at `at` by the wall clock (unix s, `unix`
    /// now): one a record says was out (crate::coord::sched's `from_state`), its job running since.
    pub fn began_at(&mut self, id: u64, at: u64, unix: u64, now: Instant) {
        if let Some(l) = self.by_id.get_mut(&id) {
            l.began = began_by_wall(at, unix, now);
        }
    }

    /// `grant`, its `granted_at` the unix second given (the scheduler's clock, crate::coord::sched).
    pub fn grant_at(&mut self, worker: &str, work: Work, now: Instant, granted_at: u64) -> u64 {
        let id = self.next;
        self.next += 1;
        if matches!(work, Work::Job { .. }) {
            self.granted.push((worker.to_string(), self.term, id));
        }
        self.by_id.insert(
            id,
            Lease {
                id,
                term: self.term,
                granted_at,
                worker: worker.to_string(),
                work,
                granted: now,
                began: None,
                deadline: now + self.ttl,
                progress: None,
                live: None,
                live_at: None,
                window: Default::default(),
                first: None,
                heard: None,
                moved: None,
                still_most: 0,
                pace: Pace::default(),
                slot: None,
                threads: 0,
                start_threads: 0,
                told: None,
                unconfirmed: false,
            },
        );
        id
    }

    /// Notes the threads lease `id`'s job was granted.
    pub fn set_threads(&mut self, id: u64, threads: u32) {
        if let Some(l) = self.by_id.get_mut(&id) {
            (l.threads, l.start_threads) = (threads, threads);
        }
    }

    /// Lease `id`'s job, its threads re-decided (crate::coord::sched's `regrant`), as its Mac says it
    /// runs on `now_on` threads and its agent has told it `granted` (its threads file: what its next
    /// target takes; 0, not said: `now_on`): `told` (Some: threads decided at `at` to tell it), else
    /// the re-grant not told yet kept, until its agent has told it. It counts for the most of the
    /// three (a grow at once, a shrink once a target takes it up; and a re-grant an earlier lead told
    /// it, which it takes at its next target, though this lead never decided it). The re-grant still
    /// to tell, if any.
    pub fn regranted(&mut self, id: u64, now_on: u32, granted: u32, told: Option<(u32, Instant)>) -> Option<u32> {
        let l = self.by_id.get_mut(&id)?;
        let granted = if granted == 0 { now_on } else { granted };
        if l.start_threads == 0 {
            l.start_threads = l.threads.max(1);
        }
        if told.is_some() {
            l.told = told;
        }
        l.told = l.told.filter(|(t, _)| *t != granted);
        l.threads = now_on.max(granted).max(l.told.map_or(0, |(t, _)| t));
        l.told.map(|(t, _)| t)
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
        self.renew_live(id, worker, progress, None, now)
    }

    /// `renew`, with what its job is doing now, when the worker says.
    pub fn renew_live(
        &mut self,
        id: u64,
        worker: &str,
        progress: Option<String>,
        live: Option<crate::agent::jobs::Live>,
        now: Instant,
    ) -> bool {
        let lasts = self.by_id.get(&id).is_some_and(|l| self.lasts(l, now));
        let mut live = live;
        // (How long its job has run: its start, kept off its view, which moves whenever its job's
        // work does.)
        let ran = live.as_mut().and_then(|v| v.ran_s.take());
        match self.by_id.get_mut(&id) {
            Some(l) if l.worker == worker && lasts => {
                l.deadline = now + self.ttl;
                if let Some(r) = ran {
                    l.began = now.checked_sub(Duration::from_secs(r)).or(l.began);
                }
                // (Its work moved: its progress line, or anything its job's view says but its CPU
                // time, changed.)
                let moved = (progress.is_some() && progress != l.progress)
                    || live.as_ref().is_some_and(|v| l.live.as_ref().is_none_or(|w| v.moved_from(w)));
                if moved {
                    l.still_most = l.still_most(now);
                    l.moved = Some(now);
                }
                if let Some(line) = &progress {
                    let target = live.as_ref().or(l.live.as_ref()).and_then(|v| v.target.as_deref());
                    // (A lease taken up: its part under way began before it, by how much unheard,
                    // so it's counted from this beat on.)
                    let prev = l.heard.unwrap_or(if l.taken_up() { now } else { l.granted });
                    l.pace.beat(line, target, l.threads, prev, now, l.granted);
                }
                l.heard = Some(now);
                if progress.is_some() {
                    l.progress = progress;
                }
                if let Some(v) = live {
                    // (Views older than the newest one `WINDOW` back are let go.)
                    if let Some((at, prev)) = l.live_at.zip(l.live.take()) {
                        l.window.push_back((at, prev));
                    }
                    while l.window.get(1).is_some_and(|(t, _)| now.saturating_duration_since(*t) >= WINDOW) {
                        l.window.pop_front();
                    }
                    // (The first view saying what it moved and its CPU time: one saying less is
                    // replaced by the next that says more.)
                    let less = |f: &crate::agent::jobs::Live| {
                        (f.nas_moved().is_none() && v.nas_moved().is_some())
                            || (f.cpu_ms.is_none() && v.cpu_ms.is_some())
                    };
                    if l.first.as_ref().is_none_or(|(_, f)| less(f)) {
                        l.first = Some((now, v.clone()));
                    }
                    l.live = Some(v);
                    l.live_at = Some(now);
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

    /// The jobs' leases granted since the last take (worker, term, id), and every job's lease held
    /// now: what GC's queue keeps owed (crate::coord::held::tell_gc).
    pub fn take_granted(&mut self) -> Vec<(String, u64, u64)> {
        let mut v = std::mem::take(&mut self.granted);
        v.extend(
            self.by_id
                .values()
                .filter(|l| matches!(l.work, Work::Job { .. }))
                .map(|l| (l.worker.clone(), l.term, l.id)),
        );
        v
    }

    /// Every live lease, for the status.
    pub fn all(&self, now: Instant) -> Vec<Lease> {
        self.live(now).cloned().collect()
    }
}

/// A leased job's progress as its worker's beats said it (docs/pool.md §10, Flags: slow; task
/// #990): what the slow flag judges it by (`lately`) and what its "done" or "end" event notes
/// (`rates`, `gap_most`). Its progress line's numbers alone count, never its bytes: a retry sending
/// its bytes again moves no line, and a line gone back on the same target (work done again)
/// counts nothing until it passes the most it had said. A lead restarted counts from its first
/// beat after.
#[derive(Clone, Debug, Default)]
pub struct Pace {
    /// The part under way (`Part`).
    part: Option<Part>,
    /// Each unit's progress and seconds over the parts ended (units, s).
    ran: BTreeMap<String, (f64, f64)>,
    /// The last beat at which its progress went on (None: none yet), and the longest stretch from
    /// its grant or one such to the next so far (s).
    on: Option<Instant>,
    gap_most: u64,
    /// The most threads its job counted for at a beat that said its progress (0: none said).
    threads_most: u32,
}

/// One run of a job's progress line: on one target (the target under way, as its view says), in one
/// unit (jobrt::jobs::unit_key) and total. From the beat before the one that first said it
/// (`since`: it began after that one), the most it has said (`best`), each beat's most since, and
/// the fewest threads its job counted for at any of its beats (`threads_least`; 0: not said).
#[derive(Clone, Debug)]
struct Part {
    target: Option<String>,
    unit: String,
    total: f64,
    since: Instant,
    best: f64,
    beats: std::collections::VecDeque<(Instant, f64)>,
    threads_least: u32,
}

impl Pace {
    /// A beat at `now` saying progress line `line` ("<done>/<total> <unit>", as crate::agent's
    /// beat sends it) on `target`, its job counted for `threads` (0: not said), the beat before it
    /// heard at `prev` (its grant before one).
    /// Its progress went on when its line passed the most it had said, or a new part began (the
    /// last one's end).
    fn beat(&mut self, line: &str, target: Option<&str>, threads: u32, prev: Instant, now: Instant, granted: Instant) {
        let Some((done, total, unit)) = parse_line(line) else { return };
        let went_on = match self.part.as_mut() {
            Some(p) if p.target.as_deref() == target && p.unit == unit && p.total == total => {
                let on = done > p.best;
                p.best = p.best.max(done);
                on
            }
            _ => {
                self.end_part();
                let target = target.map(str::to_string);
                let unit = unit.to_string();
                let beats = Default::default();
                self.part = Some(Part { target, unit, total, since: prev, best: done, beats, threads_least: threads });
                true
            }
        };
        if went_on {
            self.gap_most = self.gap_most.max(now.saturating_duration_since(self.on.unwrap_or(granted)).as_secs());
            self.on = Some(now);
        }
        if let Some(p) = self.part.as_mut() {
            p.beats.push_back((now, p.best));
            p.threads_least = p.threads_least.min(threads);
        }
        self.threads_most = self.threads_most.max(threads);
    }

    /// The part under way taken into `ran`, to its last beat.
    fn end_part(&mut self) {
        if let Some(p) = self.part.take() {
            let end = p.beats.back().map_or(p.since, |b| b.0);
            let r = self.ran.entry(p.unit).or_default();
            r.0 += p.best;
            r.1 += end.saturating_duration_since(p.since).as_secs_f64();
        }
    }

    /// The longest its progress went without going on, from its grant to `end` (its end: s); None
    /// when it never said its progress.
    pub fn gap_most(&self, end: Instant) -> Option<u64> {
        Some(self.gap_most.max(end.saturating_duration_since(self.on?).as_secs()))
    }

    /// Its rate in each unit its progress said: what its parts in that unit reached over the time
    /// they took (units a second).
    pub fn rates(&self) -> BTreeMap<String, f64> {
        let mut ran = self.ran.clone();
        if let Some(p) = &self.part {
            let end = p.beats.back().map_or(p.since, |b| b.0);
            let r = ran.entry(p.unit.clone()).or_default();
            r.0 += p.best;
            r.1 += end.saturating_duration_since(p.since).as_secs_f64();
        }
        ran.into_iter().filter(|(_, (_, secs))| *secs > 0.0).map(|(u, (n, secs))| (u, n / secs)).collect()
    }

    /// Its rate over the last `window` of the part under way, to its last beat: its unit and what
    /// it went on by a second; None until the part has run that long.
    pub fn lately(&self, window: Duration) -> Option<(&str, f64)> {
        let p = self.part.as_ref()?;
        let &(last, best) = p.beats.back()?;
        let from = last.checked_sub(window).filter(|f| *f >= p.since && !window.is_zero())?;
        let base = p.beats.iter().rev().find(|b| b.0 <= from).map_or(0.0, |b| b.1);
        Some((&p.unit, (best - base) / window.as_secs_f64()))
    }

    /// The fewest threads its job counted for over its part under way (None: no part, or not said):
    /// what its step's usual is taken at (crate::coord::history::Paces), so a job isn't held to
    /// runs that had more threads than it.
    pub fn threads_least(&self) -> Option<u32> {
        self.part.as_ref().map(|p| p.threads_least).filter(|&t| t > 0)
    }

    /// The most threads its job counted for while it said its progress (None: not said): its
    /// "done" or "end" event's `threads`.
    pub fn threads_most(&self) -> Option<u32> {
        (self.threads_most > 0).then_some(self.threads_most)
    }
}

/// A beat's progress line ("<done>/<total> <unit>"): done, total (more than 0) and its unit
/// (jobrt::jobs::unit_key: a word in brackets after it left out).
fn parse_line(line: &str) -> Option<(f64, f64, &str)> {
    let (frac, unit) = line.trim().split_once(' ')?;
    let (d, t) = frac.split_once('/')?;
    let (d, t) = (d.parse::<f64>().ok()?, t.parse::<f64>().ok()?);
    (d.is_finite() && t.is_finite() && t > 0.0).then(|| (d.clamp(0.0, t), t, crate::agent::jobs::unit_key(unit.trim())))
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

    /// Task #925 review: a lease keeps its job's newest live view and one from one to two
    /// `WINDOW`s before it, so its recent NAS rate is their difference: a job quiet for minutes
    /// then reading hard shows its hard reads within two minutes, where its mean since its lease's
    /// first beat (`Lease::mean`) lags. A view not saying all it moved (an older agent's: bytes alone) gives no rate.
    #[test]
    fn a_lease_keeps_a_recent_window_of_what_its_job_moved() {
        let t0 = Instant::now();
        let mut l = Leases::new(Duration::from_secs(600));
        let a = l.grant("m1", job(&[("6/1/1", "k")]), t0);
        let moved = |b: u64| crate::agent::jobs::Live {
            nas_read: Some(b),
            nas_wrote: Some(0),
            nas_creates: Some(0),
            nas_listed: Some(0),
            nas_stats: Some(0),
            nas_deletes: Some(0),
            ..Default::default()
        };
        // (Ten minutes quiet, a beat every 20 s; then 100 MB/s.)
        let mut b = 0;
        for s in (20..=900).step_by(20) {
            if s > 600 {
                b += 2_000_000_000;
            }
            assert!(l.renew_live(a, "m1", None, Some(moved(b)), t0 + Duration::from_secs(s)));
        }
        let x = l.get(a, "m1", t0 + Duration::from_secs(900)).unwrap();
        let span = x.live_at.unwrap().duration_since(x.window.front().unwrap().0);
        assert!(span >= WINDOW && span < 2 * WINDOW, "{span:?}");
        let lately = x.lately().unwrap().bytes_s;
        // (Its mean since its first beat, at 20 s: 30 GB over 880 s.)
        let mean = x.mean().unwrap().bytes_s;
        assert!((lately - 100e6).abs() < 1.0 && (mean - 30e9 / 880.0).abs() < 1.0, "{lately} {mean}");
        let old = crate::agent::jobs::Live { nas_read: Some(b), nas_wrote: Some(0), ..Default::default() };
        assert!(old.nas_rate(&moved(0), 900.0).is_none() && moved(b).nas_rate(&old, 900.0).is_none());
        // (Task #991 review: a first view not saying all it moved isn't the mean's base; the next
        // that does is, so the mean is had once the recent rate is, never left at the targets'.)
        let c = l.grant("m1", job(&[("6/1/2", "k")]), t0);
        assert!(l.renew_live(c, "m1", None, Some(old.clone()), t0 + Duration::from_secs(20)));
        for s in (40..=120).step_by(20) {
            assert!(l.renew_live(c, "m1", None, Some(moved(s * 1_000_000)), t0 + Duration::from_secs(s)));
        }
        let y = l.get(c, "m1", t0 + Duration::from_secs(120)).unwrap();
        assert_eq!(y.first.as_ref().map(|f| f.0), Some(t0 + Duration::from_secs(40)));
        assert!((y.mean().unwrap().bytes_s - 1e6).abs() < 1.0 && y.lately().is_some());
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

    /// Task #992: a lease's job began when its beats say (`Live::ran_s`, kept off its view: two
    /// beats saying the same of its work but its run don't move it); one read back at a lead's
    /// restart began at its grant by the wall clock until its worker says. A lease younger than
    /// its job (`taken_up`) notes none of what its beats measured on its end (the lead heard part
    /// of its run), and its part under way counts from its first beat, not the lead's restart.
    #[test]
    fn a_lease_taken_up_is_timed_from_its_job_s_start() {
        let t0 = Instant::now();
        let s = |n: u64| Duration::from_secs(n);
        let mut l = Leases::new(s(600));
        let a = l.grant("m1", job(&[("6/1/1", "k")]), t0);
        let live = |ran: u64| crate::agent::jobs::Live { ran_s: Some(ran), targets: 1, ..Default::default() };
        // (Its job began 5 s after its grant, as its beat says.)
        assert!(l.renew_live(a, "m1", None, Some(live(5)), t0 + s(10)));
        assert!(l.renew_live(a, "m1", None, Some(live(45)), t0 + s(50)));
        let x = l.of(a).unwrap();
        assert!(!x.taken_up() && x.live.as_ref().unwrap().ran_s.is_none());
        assert_eq!((x.ran(t0 + s(65)), x.still()), (s(60), Some(s(40))), "not moved by its run alone");
        assert!(x.noted(crate::coord::history::Event::new("done"), t0 + s(65)).still_s.is_some());
        // Read back at a restart 100 s on, its grant 600 s before by the wall clock.
        let (mut kept, each) = l.kept();
        kept["leases"] = each.into_iter().map(|x| x.1).collect();
        kept["leases"][0]["granted_at"] = (x.granted_at - 600).into();
        let t1 = t0 + s(100);
        let mut back = Leases::new(s(600));
        back.restore(&kept, t1).unwrap();
        let y = back.of(a).unwrap();
        assert!(y.taken_up() && (s(600)..s(602)).contains(&y.ran(t1)), "{:?}", y.ran(t1));
        // Its beats then: its run as its job says, its part under way from its first beat.
        assert!(back.renew_live(a, "m1", Some("50/100 tiles".into()), Some(live(700)), t1 + s(20)));
        assert!(back.renew_live(a, "m1", Some("55/100 tiles".into()), Some(live(730)), t1 + s(50)));
        let y = back.of(a).unwrap();
        assert_eq!(y.ran(t1 + s(50)), s(730));
        assert_eq!(y.pace.lately(s(40)), None, "its part heard 30 s: not judged over 40");
        assert_eq!(y.pace.lately(s(30)), Some(("tiles", 5.0 / 30.0)));
        let e = y.noted(crate::coord::history::Event::new("done"), t1 + s(60));
        assert!(e.still_s.is_none() && e.gap_s.is_none() && e.rates.is_empty() && e.threads.is_none());
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

    /// A job's stillness by its beats (task #986): anything its view or progress line says
    /// changed is a move; the stretch from its last move to the last beat heard is how still it is
    /// now, and the longest between two moves (its end one) what its run is held to.
    #[test]
    fn a_jobs_stillness_is_the_time_since_its_beats_saw_it_move() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut ls = Leases::new(Duration::from_secs(600));
        let id = ls.grant("m1", Work::Job { step: "unit".into(), targets: vec![] }, t0);
        assert_eq!(ls.of(id).unwrap().still(), None, "no beat yet");
        let view = |bytes: u64| crate::agent::jobs::Live { nas_read: Some(bytes), ..Default::default() };
        assert!(ls.renew_live(id, "m1", None, Some(view(1)), at(60)));
        assert!(ls.renew_live(id, "m1", None, Some(view(1)), at(120)));
        assert!(ls.renew_live(id, "m1", None, Some(view(1)), at(180)));
        assert_eq!(ls.of(id).unwrap().still(), Some(Duration::from_secs(120)));
        // (Its bytes moved: still no more.)
        assert!(ls.renew_live(id, "m1", None, Some(view(2)), at(240)));
        assert_eq!(ls.of(id).unwrap().still(), Some(Duration::ZERO));
        // (Its progress line moved, its view the same.)
        assert!(ls.renew_live(id, "m1", Some("1/4 tiles".into()), Some(view(2)), at(270)));
        assert!(ls.renew_live(id, "m1", Some("1/4 tiles".into()), Some(view(2)), at(330)));
        assert_eq!(ls.of(id).unwrap().still(), Some(Duration::from_secs(60)));
        assert_eq!(ls.of(id).unwrap().still_most(at(400)), 180, "60 s to 240 s; its end 130 s after its last move");
        assert_eq!(ls.of(id).unwrap().still_most(at(500)), 230);
    }

    /// The cores a job keeps busy by its beats' CPU time (task #1091): since its first view and over
    /// its last minute or so, once two are `WINDOW` apart; its CPU time alone moving isn't a move
    /// (a job spinning on nothing would never be still).
    #[test]
    fn a_jobs_busy_cores_are_its_cpu_time_over_its_beats() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut ls = Leases::new(Duration::from_secs(600));
        let id = ls.grant("m1", Work::Job { step: "unit".into(), targets: vec![] }, t0);
        let view = |cpu_s: u64| crate::agent::jobs::Live { cpu_ms: Some(cpu_s * 1000), ..Default::default() };
        assert!(ls.renew_live(id, "m1", None, Some(view(0)), at(20)));
        assert!(ls.renew_live(id, "m1", None, Some(view(10)), at(50)));
        assert_eq!(
            (ls.of(id).unwrap().cores_mean(), ls.of(id).unwrap().cores_lately()),
            (None, None),
            "under a minute"
        );
        // (0.3 a core for 60 s, then 3 for 60.)
        assert!(ls.renew_live(id, "m1", None, Some(view(18)), at(80)));
        for s in [110, 140] {
            assert!(ls.renew_live(id, "m1", None, Some(view(18 + 3 * (s - 80))), at(s)));
        }
        let l = ls.of(id).unwrap();
        assert_eq!(l.cores_mean(), Some(198.0 / 120.0));
        assert_eq!(l.cores_lately(), Some(3.0));
        assert_eq!(l.still(), Some(Duration::from_secs(120)), "its CPU time alone moved");
    }

    /// A job's pace by its beats' progress lines (task #990): each target's run of a line is a part;
    /// only the most a line has said counts (a retry gone back, its bytes sent again, counts
    /// nothing until it passes it); its rate in each unit over its parts' time, its longest wait
    /// for progress, and its rate lately over a window of its part under way, once the part is
    /// that old.
    #[test]
    fn a_jobs_pace_is_its_progress_lines_most_over_time() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut ls = Leases::new(Duration::from_secs(6000));
        let id = ls.grant("m1", Work::Job { step: "terrain-hi".into(), targets: vec![] }, t0);
        let on = |target: &str, bytes: u64| crate::agent::jobs::Live {
            target: Some(format!("terrain-hi {target}")),
            nas_read: Some(bytes),
            ..Default::default()
        };
        let beat = |ls: &mut Leases, s: u64, line: &str, target: &str, bytes: u64| {
            assert!(ls.renew_live(id, "m1", Some(line.into()), Some(on(target, bytes)), at(s)));
        };
        beat(&mut ls, 60, "0/100 tiles", "6/1/1", 0);
        beat(&mut ls, 120, "0/100 tiles", "6/1/1", 10);
        beat(&mut ls, 180, "40/100 tiles", "6/1/1", 20);
        // (A retry: its bytes sent again, its line gone back.)
        beat(&mut ls, 240, "40/100 tiles", "6/1/1", 90);
        beat(&mut ls, 300, "20/100 tiles", "6/1/1", 160);
        beat(&mut ls, 360, "70/100 tiles", "6/1/1", 170);
        beat(&mut ls, 420, "100/100 tiles", "6/1/1", 180);
        // (The next target's part, of the same unit and total.)
        beat(&mut ls, 480, "10/100 tiles", "6/1/2", 190);
        let p = &ls.of(id).unwrap().pace;
        assert_eq!(p.lately(Duration::from_secs(120)), None, "its part is 60 s old");
        for (s, line) in [(540, "10/100 tiles"), (600, "10/100 tiles (blending)"), (660, "12/100 tiles")] {
            beat(&mut ls, s, line, "6/1/2", 200);
        }
        let l = ls.of(id).unwrap();
        assert_eq!(l.pace.rates(), BTreeMap::from([("tiles".to_string(), 112.0 / 660.0)]));
        assert_eq!(l.pace.lately(Duration::from_secs(240)), Some(("tiles", 12.0 / 240.0)), "from its part's start");
        assert_eq!(l.pace.lately(Duration::from_secs(120)), Some(("tiles", 2.0 / 120.0)));
        assert_eq!(l.pace.gap_most(at(700)), Some(180), "180 s to 360 s, and 480 s to 660 s");
        assert_eq!((l.pace.threads_least(), l.pace.threads_most()), (None, None), "its threads not said");
        let e = l.noted(crate::coord::history::Event::new("done"), at(900));
        assert_eq!((e.gap_s, e.still_s, e.rates.len()), (Some(240), Some(240), 1), "its end 240 s after its last");
        // (Its threads: the fewest over its part under way, the most over its run.)
        ls.set_threads(id, 4);
        beat(&mut ls, 720, "14/100 tiles", "6/1/2", 210);
        ls.regranted(id, 2, 2, None);
        beat(&mut ls, 780, "16/100 tiles", "6/1/2", 220);
        beat(&mut ls, 840, "1/9 packs", "6/1/2", 230);
        ls.set_threads(id, 3);
        beat(&mut ls, 900, "2/9 packs", "6/1/2", 240);
        let l = ls.of(id).unwrap();
        assert_eq!((l.pace.threads_least(), l.pace.threads_most()), (Some(2), Some(4)));
        assert_eq!(l.noted(crate::coord::history::Event::new("done"), at(900)).threads, Some(4));
        // (A job that never said its progress: no rate, no wait for it.)
        let quiet = ls.grant("m1", Work::Job { step: "pack".into(), targets: vec![] }, t0);
        assert!(ls.renew_live(quiet, "m1", None, None, at(60)));
        let e = ls.of(quiet).unwrap().noted(crate::coord::history::Event::new("done"), at(90));
        assert_eq!((e.gap_s, e.rates.is_empty()), (None, true));
    }
}
