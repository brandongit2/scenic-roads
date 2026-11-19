//! The pool's NAS and network limits (docs/pool.md §7.7, task #243), on unless the pool's setting
//! `nas-budget` (agentcore::coord::asks) says `off` (on while not set). Packing lets more jobs run
//! at once, and the share takes 20 to 55 small-file creates a second: the lead's coordinator keeps
//! the pool's running jobs' NAS weights together within what the NAS was measured to give
//! (agentcore::coord's `pack`, `pick_every` and `pick_own`: the lead's own jobs too), as the Wikimedia
//! budget keeps the pool's requests.
//!
//! - **A job's NAS weight** is the share of the NAS it takes, in thousandths: its targets' measured
//!   rate (agentcore::needs::Book::nas: bytes read and written a second, files made a second, from
//!   crate::costin's `bytes`, `wrote` and `creates` over its time), each over the NAS's (`Limits`),
//!   added (one combined weight; reads and writes split only if measures show they limit apart),
//!   with its metadata work beside them (task #451): the folder entries it listed, the files it
//!   statted and deleted a second (crate::costin's `listed`, `stats`, `deletes`), each over the
//!   NAS's rate for it. A clean-up's walk reads and writes next to nothing yet keeps the share busy.
//!   A job's targets run one after another: its weight is its heaviest target's.
//! - **The limits** are measured, never typed: `probe` makes small files and larger ones in a
//!   folder of its own, `STREAMS` streams at once (the NAS's capacity together, task #534), reads
//!   the larger back past the cache, lists a kept folder, stats its files, deletes the small ones
//!   one by one, and removes the rest. The coordinator probes every `EVERY_S`, whatever the setting
//!   says, and takes the best of the last `KEEP_S`'s probes (`Probes`): a probe run while the build
//!   loads the NAS measures what's left, so the best is the nearest to the whole. Probes taken
//!   under load are kept too (they only ever read low, and the best of the day passes over them;
//!   dropped, a lead started on a busy NAS would have no limits at all). The probes are kept in
//!   the lead's store (agentcore::coord::kept's `PROBES`, so in its backup too) and read once it's
//!   open: a restart, or a new lead on another Mac taking the backup up, keeps the day's best
//!   (task #603: a restart under load had cut the limits from 86 MB/s to 66.5).
//! - **The pacing** (`Pace`, task #1417): the jobs' weights together are held within a window, not
//!   the whole: every 10 s the coordinator reads every Mac's NAS answer time against its best and
//!   the NAS's own disks' busy shares (crate::budget::snmp) against the owner's target (`Target`),
//!   whatever loads the NAS, the build or not, and grows the window slowly while the NAS answers
//!   near its best, or cuts it to half what's in use once it slows past the knee. A job weighing
//!   less than a `STEP` isn't paced.
//! - **Downloads**: a job of a fetching step (pipeline::agent::steps::Kind::Fetch) counts one; the
//!   switch's `downloads <n>` caps them pool-wide (no cap without it).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the coordinator's probes are made, under the project folder (a folder per process).
pub const PROBE_DIR: &str = "state/pool/nas-probe";
/// How often the coordinator probes (s).
pub const EVERY_S: u64 = 30 * 60;
/// How long a probe counts toward the limits (s).
pub const KEEP_S: u64 = 24 * 3600;
/// The whole NAS, in weight (thousandths).
pub const WHOLE: u32 = 1000;
/// A probe's streams at once (task #534, 21 Oct: one stream measured 5 creates/s and 25 MB/s
/// where four together made 41 creates/s and eight 57 MB/s).
pub const STREAMS: usize = 4;
/// Each stream's small files (each `SMALL` bytes), and its larger file's size.
pub const SMALL_FILES: usize = 12;
pub const SMALL: usize = 4096;
pub const LARGE: usize = 16 << 20;

/// The target while none is set: 85 % (headroom kept: #451's stall came at a NAS held busy).
pub const DEFAULT_TARGET_PCT: u32 = 85;
/// The targets that may be set (%): never 100, never so low the pool stops.
pub const TARGET_RANGE: std::ops::RangeInclusive<u32> = 30..=95;

/// The target as set: its share (%), who set it and when (unix s).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Target {
    pub pct: u32,
    pub by: String,
    pub at: u64,
}

/// The target's share (%): the set one's, else `DEFAULT_TARGET_PCT`.
pub fn target_pct(t: Option<&Target>) -> u32 {
    t.map_or(DEFAULT_TARGET_PCT, |t| t.pct)
}

/// The pacing (task #1417): the weight the pool's jobs may hold together, `Limits::window`, found
/// as TCP finds its window, from the NAS's live latency as every Mac reads it and its disks' busy
/// share against the owner's target. The NAS is soft with a cliff (on 4 Oct its saturation left
/// processes on two Macs in uninterruptible waits): work is added slowly while it answers near its
/// best, and cut quickly once its answers slow, before the cliff. Every `READ_EVERY_S` the lead
/// reads:
///
/// - each Mac's NAS answer time (its agent's listing of the project's folder every 5 s while a
///   build runs, its mean over the last half minute, carried by its capacity) over that Mac's
///   **floor**, the least of its minutes' least answers over the last `FLOOR_S` (never under
///   store::aimd::NAS_LEAST); the worst Mac's ratio counts (a Mac away on a slow link has a slow
///   floor of its own, not a congested NAS);
/// - the NAS's busiest disk's busy share (SNMP, smoothed by `SMOOTH`) against the owner's target
///   (`Target`, `scenic nas-target`, by default `DEFAULT_TARGET_PCT`).
///
/// Then, as the NAS's own readers decide their reads in flight (store::aimd, whose `NEAR` and
/// `SLOW` it shares):
///
/// - **past the knee** (the worst ratio past `SLOW`, or the disks over the target): the window
///   drops to half the weight in use (`DECREASE`) at once, never under `STEP`: the jobs running go
///   on, and nothing more that's paced starts until enough of them end;
/// - **near the best** (the worst ratio within `NEAR`, the disks within the target): it grows,
///   only while it's what limits the pool (the weight in use and the least a target waiting for
///   the budget weighs past it; none waiting, the weight in use within `STEP` of it: a window no
///   job asks for isn't credit for a later burst, nor is room left by a job just ended): doubling a
///   minute until its first cut (TCP's slow start, as store::aimd's), then by `STEP` a minute;
/// - **between**, or nothing read: it holds.
///
/// It starts at `STEP`, or at what the jobs already leased weigh where that's more (a lead's
/// coordinator starting knows nothing of the NAS's load but what runs on it; a job beside nothing
/// always goes), and has no ceiling: the weights are estimates, and the latency, not they, says
/// when the NAS is full.
#[derive(Clone, Debug, PartialEq)]
pub struct Pace {
    window: f64,
    /// Whether it has been cut yet: till then it doubles.
    cut: bool,
    /// Each Mac's floor: its minutes' least answer times (ms), the last `FLOOR_S`'s, oldest first.
    floors: std::collections::BTreeMap<String, std::collections::VecDeque<(u64, f64)>>,
    /// The busiest disk's busy share, smoothed (%; None: not read).
    pub load: Option<f64>,
    /// The target it was last read against (%).
    pub target: u32,
    /// The last reading (None: none yet).
    pub last: Option<Reading>,
}

/// One reading of the pacing (`Pace::read`): when (unix s), the worst Mac's answer time against its
/// floor, the disks' smoothed load, the weight in use, and what it did.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Reading {
    pub at: u64,
    /// The Mac whose NAS answers were slowest against its floor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst: Option<Answers>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_pct: Option<f64>,
    pub in_use: u32,
    pub verdict: Verdict,
}

/// A Mac's NAS answer time and its floor (ms).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Answers {
    pub mac: String,
    pub ms: f64,
    pub floor_ms: f64,
}

impl Answers {
    /// Its answer time over its floor.
    pub fn ratio(&self) -> f64 {
        self.ms / self.floor_ms.max(f64::MIN_POSITIVE)
    }
}

/// What a reading did to the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Near the best, and the window what limits: grown.
    Grew,
    /// Past the knee: cut.
    Cut,
    /// Between, or the window not what limits.
    #[default]
    Held,
    /// Nothing read (no Mac's answer time, no disk reading): held.
    Unread,
}

/// How often the lead reads the pacing (s).
pub const READ_EVERY_S: u64 = 10;
/// The span a Mac's floor is its least minute over (s): long enough that a burst's slow minutes
/// don't become the floor, short enough to follow a Mac whose link has changed.
pub const FLOOR_S: u64 = 30 * 60;
/// The window's additive step, a minute, and its least (thousandths of the NAS). A job weighing
/// less isn't paced (`Limits::paced`): it's within what the pacing adds anyway.
pub const STEP: u32 = 50;
/// The window past the knee: this share of the weight in use.
pub const DECREASE: f64 = 0.5;

/// The smoothing's weight of a new disk reading (one each 10 s: about half a minute's memory, so
/// one busy second doesn't cut the window, nor one quiet one grow it).
pub const SMOOTH: f64 = 0.3;

impl Default for Pace {
    fn default() -> Self {
        Pace { window: STEP as f64, cut: false, floors: Default::default(), load: None, target: 0, last: None }
    }
}

impl Pace {
    /// The window now (thousandths of the NAS).
    pub fn window(&self) -> u32 {
        self.window.round() as u32
    }

    /// A reading at `t` (unix s): each Mac's NAS answer time now (ms), the disks' busy shares (%;
    /// None: not read) against the target `pct`, the weight the pool's leased jobs hold now, and
    /// the least a target waiting for the budget weighs (None: none waits; the window is what
    /// limits once it refuses that beside what's in use, or, none waiting, a `STEP`).
    pub fn read(
        &mut self,
        t: u64,
        answers: &[(String, f64)],
        busy: Option<&[f64]>,
        pct: u32,
        in_use: u32,
        wants: Option<u32>,
    ) -> Verdict {
        match busy.and_then(|b| b.iter().copied().filter(|b| b.is_finite()).reduce(f64::max)) {
            Some(now) => self.load = Some(self.load.map_or(now, |l| l + SMOOTH * (now - l))),
            None => self.load = None,
        }
        self.target = pct;
        let least = store::aimd::NAS_LEAST.as_secs_f64() * 1000.0;
        let mut worst: Option<Answers> = None;
        for (mac, ms) in answers.iter().filter(|(_, ms)| ms.is_finite() && *ms > 0.0) {
            let f = self.floors.entry(mac.clone()).or_default();
            let minute = t / 60;
            match f.back_mut() {
                Some((m, v)) if *m == minute => *v = v.min(*ms),
                _ => f.push_back((minute, *ms)),
            }
            let floor = f.iter().map(|x| x.1).fold(f64::INFINITY, f64::min).max(least);
            let a = Answers { mac: mac.clone(), ms: *ms, floor_ms: floor };
            if worst.as_ref().is_none_or(|w| a.ratio() > w.ratio()) {
                worst = Some(a);
            }
        }
        // (Minutes past `FLOOR_S` let go, and with them the Macs no longer heard from.)
        for f in self.floors.values_mut() {
            while f.front().is_some_and(|(m, _)| (m + 1) * 60 + FLOOR_S <= t) {
                f.pop_front();
            }
        }
        self.floors.retain(|_, f| !f.is_empty());
        let ratio = worst.as_ref().map(Answers::ratio);
        let over = self.load.is_some_and(|l| l > pct as f64);
        if self.last.is_none() {
            self.window = self.window.max(in_use as f64);
        }
        let dt = self.last.as_ref().map_or(READ_EVERY_S, |r| t.saturating_sub(r.at)).min(60) as f64;
        let verdict = if ratio.is_none() && self.load.is_none() {
            Verdict::Unread
        } else if over || ratio.is_some_and(|r| r > store::aimd::SLOW) {
            self.window = self.window.min(in_use as f64 * DECREASE).max(STEP as f64);
            self.cut = true;
            Verdict::Cut
        } else if ratio.is_none_or(|r| r <= store::aimd::NEAR)
            && in_use.saturating_add(wants.unwrap_or(STEP)) as f64 > self.window
        {
            if self.cut {
                self.window += STEP as f64 * dt / 60.0;
            } else {
                self.window *= det::Det::dexp2(dt / 60.0);
            }
            Verdict::Grew
        } else {
            Verdict::Held
        };
        self.last =
            Some(Reading { at: t, worst, load_pct: self.load.map(|l| (l * 10.0).round() / 10.0), in_use, verdict });
        verdict
    }

    /// Why the pacing holds new NAS work back, for the lead's waiting lines: the window under the
    /// weight in use (None: it isn't).
    pub fn why(&self) -> Option<String> {
        let r = self.last.as_ref()?;
        let w = self.window();
        (w < r.in_use).then(|| {
            let cause = match (&r.worst, r.load_pct) {
                (_, Some(l)) if l > self.target as f64 => {
                    format!("its disks {l:.0} % busy, over the target {} %", self.target)
                }
                (Some(a), _) if a.ratio() > store::aimd::SLOW => {
                    format!("answering {} in {:.0} ms, {:.1}× its best", a.mac, a.ms, a.ratio())
                }
                _ => "slowed lately".to_string(),
            };
            format!("NAS work paced to {} % of the NAS, {} % in use (the NAS {cause})", w / 10, r.in_use / 10)
        })
    }
}

impl Limits {
    /// The pacing (`Pace`) for a job of weight `w` beside leased jobs weighing `running`: a job
    /// weighing less than a `STEP`, one going beside nothing, or one within the window go; and
    /// while the window is the whole or more (not cut), whatever the `fits*` rules let through
    /// (their ways past the sum: the lead's own alone, an aged job). Only ever narrows them.
    pub fn paced(&self, running: u32, w: u32) -> bool {
        w < STEP || running == 0 || self.window >= WHOLE || running.saturating_add(w) <= self.window
    }
}

/// The switch as its file says: None, off; else on, with its download cap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switch {
    pub downloads: Option<usize>,
}

/// The switch's text: `off` alone, off; else on, with its `downloads <n>` (a line not understood
/// ignored).
pub fn parse(s: &str) -> Option<Switch> {
    if s.trim() == "off" {
        return None;
    }
    let mut w = Switch::default();
    for l in s.lines() {
        if let ["downloads", n] = l.split_whitespace().collect::<Vec<_>>().as_slice() {
            w.downloads = n.parse().ok().or(w.downloads);
        }
    }
    Some(w)
}

/// What one probe measured: bytes moved a second (written and read back, together), files made a
/// second, and the metadata operations a second: folder entries listed, files statted, files
/// deleted (0 in a probe from before them: not measured, so not weighed).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Probe {
    /// When (unix s).
    pub at: u64,
    pub bytes_s: f64,
    pub creates_s: f64,
    #[serde(default)]
    pub listed_s: f64,
    #[serde(default)]
    pub stats_s: f64,
    #[serde(default)]
    pub deletes_s: f64,
}

/// The pool's limits now: the NAS's (a probe's), the pacing's window, and the downloads' cap.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Limits {
    pub bytes_s: f64,
    pub creates_s: f64,
    /// The metadata operations a second (0: not probed, not weighed).
    pub listed_s: f64,
    pub stats_s: f64,
    pub deletes_s: f64,
    pub downloads: Option<usize>,
    /// The weight the pool's leased jobs may hold together (thousandths of the NAS: `Pace`'s
    /// window; `WHOLE` till it's read).
    pub window: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            bytes_s: 0.0,
            creates_s: 0.0,
            listed_s: 0.0,
            stats_s: 0.0,
            deletes_s: 0.0,
            downloads: None,
            window: WHOLE,
        }
    }
}

/// A target's or job's measured NAS rate (agentcore::needs::Book::nas): bytes and files made, and its
/// metadata work, a second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rate {
    pub bytes_s: f64,
    pub creates_s: f64,
    pub listed_s: f64,
    pub stats_s: f64,
    pub deletes_s: f64,
}

impl Limits {
    /// Whether `other` weighs every job as these do (the same NAS; the window and the downloads'
    /// cap aside): a weight kept for these stands for `other`.
    pub fn weighs_as(&self, other: &Limits) -> bool {
        let rates = |l: &Limits| [l.bytes_s, l.creates_s, l.listed_s, l.stats_s, l.deletes_s];
        rates(self) == rates(other)
    }

    /// The weight of a job at `r` (thousandths of the NAS; rounded up, so a measured job never
    /// weighs nothing; at most the whole, `WHOLE`). 0 while no probe measured the NAS.
    pub fn weight(&self, r: Rate) -> u32 {
        let part = |x: f64, of: f64| if of > 0.0 { x / of } else { 0.0 };
        let w = (part(r.bytes_s, self.bytes_s)
            + part(r.creates_s, self.creates_s)
            + part(r.listed_s, self.listed_s)
            + part(r.stats_s, self.stats_s)
            + part(r.deletes_s, self.deletes_s))
            * WHOLE as f64;
        if w > 0.0 {
            w.ceil().min(WHOLE as f64) as u32
        } else {
            0
        }
    }

    /// Whether a job of weight `w` may start beside leased jobs weighing `running` together, of
    /// which the members' (all but the lead's own) weigh `members`, in the window `win`: within
    /// it, weighing less than a `STEP` (not paced), or, when no member's job weighs anything,
    /// whatever the lead's own weigh (they're always about: a job that fits only beside nothing
    /// would never go).
    pub fn fits(members: u32, running: u32, w: u32, win: u32) -> bool {
        w < STEP || members == 0 || running.saturating_add(w) <= win
    }

    /// `fits` for a member's job, which would starve beside the members' other jobs: once it has
    /// waited `OWN_AGE` for the budget alone (`waited`) it goes too while no leased job weighs the
    /// whole (`heaviest`, the most one weighs), if it weighs the whole itself or more than the leased
    /// jobs together: it waits for heavier work to end, never for a stream of lighter (task #603: a
    /// 946 one aged beside a 103 one could never go, and the room held for it stopped the pool);
    /// once it's in, the leased jobs outweigh any other, so aged ones never pile up.
    pub fn fits_job(members: u32, running: u32, heaviest: u32, w: u32, waited: bool, win: u32) -> bool {
        Self::fits(members, running, w, win) || (waited && heaviest < WHOLE && (w >= WHOLE || running < w))
    }

    /// Whether a job of the lead's own of weight `w` may start beside leased jobs weighing
    /// `running` together, in the window `win`: within it, weighing less than a `STEP`, or beside
    /// nothing that weighs anything (a job past the window alone).
    fn fits_own(running: u32, w: u32, win: u32) -> bool {
        w < STEP || running == 0 || running.saturating_add(w) <= win
    }

    /// `fits_own` for a target of the lead's own every-step asks (agentcore::coord's `pick_every`),
    /// which members' small jobs would otherwise hold back for good (each lease's weight keeps
    /// `running` above zero): it goes too once it has waited `OWN_AGE` for the budget alone
    /// (`waited`), no leased job weighs the whole (`heaviest`, the most one weighs) and the lead's
    /// own leased jobs (`own`, their weight together) leave it room within the window: aged ones
    /// go past the members' jobs alone, one at a time, never piling up (task #603).
    pub fn fits_own_target(running: u32, own: u32, heaviest: u32, w: u32, waited: bool, win: u32) -> bool {
        Self::fits_own(running, w, win) || (waited && heaviest < WHOLE && own.saturating_add(w) <= win)
    }

    /// `fits_own_target` for the lead's own planned job (agentcore::coord's `own_nas_fits`): also, not
    /// waiting, once it weighs the whole itself while no leased job weighs the whole, so a whole
    /// one isn't held back by the lead's own stream of smaller jobs.
    pub fn fits_own_job(running: u32, own: u32, heaviest: u32, w: u32, waited: bool, win: u32) -> bool {
        Self::fits_own_target(running, own, heaviest, w, waited, win) || (heaviest < WHOLE && w >= WHOLE)
    }
}

/// The steps that wait for a quiet NAS (task #451): never urgent, they start only beside nothing
/// that weighs anything, or within the window by their own measured weight, never by an aged wait
/// (`fits_quiet`), never hold room for themselves, and yield at a safe point to a job held back
/// for the budget `OWN_AGE` that their going would let start (agentcore::coord's `nas_yield`).
/// Unmeasured the way they run now, they weigh the whole and go as the lead's own unmeasured NAS
/// steps do (agentcore::coord::sched's `own_nas_fits`), so their first run measures them.
pub const QUIET_STEPS: [&str; 1] = ["gc"];

/// How long a quiet step's job is held back for a quiet NAS before it goes as an aged job of the
/// lead's own (`fits_own_job`, its room held): a pool that stays busy never starves it for good
/// while its lead runs that long (the wait is kept in its coordinator's memory).
pub const QUIET_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

impl Limits {
    /// Whether a job of a quiet step (`QUIET_STEPS`) of measured weight `w` may start beside leased
    /// jobs weighing `running` together, in the window `win`: beside nothing that weighs anything,
    /// or, its weight more than nothing, within the window with them (a 0, the NAS's metadata
    /// rates not probed, alone).
    pub fn fits_quiet(running: u32, w: u32, win: u32) -> bool {
        running == 0 || (w > 0 && running.saturating_add(w) <= win)
    }
}

/// How long the lead's own planned job waits for the NAS budget alone before it goes beside
/// leased jobs none of which weighs the whole (`Limits::fits_own_job`).
pub const OWN_AGE: std::time::Duration = std::time::Duration::from_secs(600);

/// The probes kept (the last `KEEP_S`'s), and the limits they give: the best of each.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Probes(pub Vec<Probe>);

impl Probes {
    pub fn add(&mut self, p: Probe) {
        self.0.retain(|q| q.at + KEEP_S > p.at);
        self.0.push(p);
    }

    /// `kept`'s probes (another copy's: the store's, a backup's) added to these, those of `KEEP_S`
    /// before `now` let go, oldest first.
    pub fn merge(&mut self, kept: &Probes, now: u64) {
        for p in &kept.0 {
            if !self.0.contains(p) {
                self.0.push(*p);
            }
        }
        self.0.retain(|q| q.at + KEEP_S > now);
        self.0.sort_by_key(|p| p.at);
    }

    /// The limits (None: nothing probed yet).
    pub fn limits(&self, downloads: Option<usize>) -> Option<Limits> {
        let best = |f: fn(&Probe) -> f64| self.0.iter().map(f).fold(0.0, f64::max);
        (!self.0.is_empty()).then(|| Limits {
            bytes_s: best(|p| p.bytes_s),
            creates_s: best(|p| p.creates_s),
            listed_s: best(|p| p.listed_s),
            stats_s: best(|p| p.stats_s),
            deletes_s: best(|p| p.deletes_s),
            downloads,
            window: WHOLE,
        })
    }
}

fn host_part(me: &str) -> String {
    me.replace(['/', '.', '-'], "_")
}

/// A folder of a probe's own under `root` (`PROBE_DIR`), by `me` and this process:
/// `<me>-<pid>` (`me` with no `-` in it, so one host's folders are told from another's).
pub fn probe_dir(root: &store::nasfs::NasRoot, me: &str) -> PathBuf {
    // (Raw: the probe measures the share's own speed, its calls never counted as the build's NAS
    // work: tests/nas_access.rs.)
    root.join(PROBE_DIR).join(format!("{}-{}", host_part(me), std::process::id())).unchecked_path()
}

/// Removes `me`'s probe folders under `root` a stopped process left (a crash mid-probe): every
/// `<me>-<pid>` but this process's; other hosts' left alone. How many were removed.
pub fn clean_stale(root: &store::nasfs::NasRoot, me: &str) -> usize {
    let mine = format!("{}-{}", host_part(me), std::process::id());
    let prefix = format!("{}-", host_part(me));
    let Ok(rd) = root.join(PROBE_DIR).read_dir() else { return 0 };
    let mut n = 0;
    for e in rd {
        let pid = e.name.strip_prefix(&prefix);
        if pid.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            && e.name != mine
            && e.path.remove_dir_all().is_ok()
        {
            n += 1;
        }
    }
    n
}

/// Probes the NAS in `dir` (made, then removed with all it holds): in each of `STREAMS` streams at
/// once, `SMALL_FILES` files of `SMALL` bytes made (create-new, each written and closed: the
/// share's creates), then in each one of `LARGE` written and synced and read back past the cache;
/// then the kept folder listed (`meta_dir`) and its files statted, and the small files deleted one
/// by one. Short: a few seconds.
pub fn probe(dir: &Path) -> std::io::Result<Probe> {
    std::fs::create_dir_all(dir)?;
    let r = probe_in(dir);
    let gone = std::fs::remove_dir_all(dir);
    let p = r?;
    gone?;
    Ok(p)
}

fn probe_in(dir: &Path) -> std::io::Result<Probe> {
    // (Each part `STREAMS` streams at once: the NAS's capacity together, not one stream's.)
    let t = Instant::now();
    in_streams(|k| {
        let small = vec![7u8; SMALL];
        for i in 0..SMALL_FILES {
            let p = dir.join(format!("s{k}_{i}"));
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&p)?;
            f.write_all(&small)?;
            store::sys::close(f, &p)?;
        }
        Ok(())
    })?;
    let creates_s = (STREAMS * SMALL_FILES) as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let t = Instant::now();
    in_streams(|k| large_round(&dir.join(format!("large{k}"))))?;
    let bytes_s = (STREAMS * 2 * LARGE) as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let (listed_s, stats_s, deletes_s) = probe_meta(dir)?;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Ok(Probe { at, bytes_s, creates_s, listed_s, stats_s, deletes_s })
}

/// Runs `f(k)` for each stream `k` of `STREAMS` at once: the first error, if any.
fn in_streams(f: impl Fn(usize) -> std::io::Result<()> + Sync) -> std::io::Result<()> {
    std::thread::scope(|s| {
        let f = &f;
        let hs: Vec<_> = (0..STREAMS).map(|k| s.spawn(move || f(k))).collect();
        hs.into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(std::io::Error::other("a probe stream panicked"))))
            .collect()
    })
}

/// One stream's larger file at `p`: `LARGE` bytes written and synced, read back past the cache.
fn large_round(p: &Path) -> std::io::Result<()> {
    let large: Vec<u8> = (0..LARGE).map(|i| (i * 31 % 251) as u8).collect();
    {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(p)?;
        f.write_all(&large)?;
        f.sync_all()?;
        store::sys::close(f, p)?;
    }
    let mut f = std::fs::File::open(p)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: plain fcntl on a descriptor we own; failure only means the read may be cached.
        unsafe {
            libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1);
        }
    }
    let mut back = Vec::with_capacity(LARGE);
    f.read_to_end(&mut back)?;
    if back.len() != LARGE {
        return Err(std::io::Error::other(format!("read back {} of {LARGE} bytes", back.len())));
    }
    Ok(())
}

/// The metadata part of a probe in `dir` (its small files `s<k>_<i>` there): entries listed, files
/// statted and files deleted a second (one stream: a clean-up's walk is one).
fn probe_meta(dir: &Path) -> std::io::Result<(f64, f64, f64)> {
    let rate = |n: usize, t: Instant| n as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let (listed_s, stats_s) = probe_listing(&meta_dir(dir))?;
    let t = Instant::now();
    for k in 0..STREAMS {
        for i in 0..SMALL_FILES {
            std::fs::remove_file(dir.join(format!("s{k}_{i}")))?;
        }
    }
    Ok((listed_s, stats_s, rate(STREAMS * SMALL_FILES, t)))
}

/// The folder a probe in `dir` lists and stats (`<host>-meta` beside `<host>-<pid>`): kept between
/// probes, never written by the probe that measures it, so its listing and its files' attributes
/// come from the NAS, not this Mac's SMB cache of what it just wrote (task #451 review).
pub fn meta_dir(dir: &Path) -> PathBuf {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let host = name.rsplit_once('-').map_or(name.as_str(), |(h, _)| h);
    dir.with_file_name(format!("{host}-meta"))
}

/// Files in the kept folder (`meta_dir`), and how long since it was last written before it's
/// measured (the client's caches of it long gone).
pub const META_FILES: usize = 48;
pub const META_SETTLE: std::time::Duration = std::time::Duration::from_secs(300);

/// Entries listed and files statted a second in the kept folder `m`: (0, 0), not measured, when
/// it's missing (made now, for the next probe), incomplete (made again) or written within
/// `META_SETTLE`.
fn probe_listing(m: &Path) -> std::io::Result<(f64, f64)> {
    let names: Vec<String> = (0..META_FILES).map(|i| format!("m{i}")).collect();
    let settled = std::fs::metadata(m.join(&names[META_FILES - 1]))
        .and_then(|md| md.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|e| e >= META_SETTLE));
    if !settled {
        if !m.join(&names[META_FILES - 1]).exists() {
            std::fs::create_dir_all(m)?;
            for n in &names {
                std::fs::write(m.join(n), b"m")?;
            }
        }
        return Ok((0.0, 0.0));
    }
    let rate = |n: usize, t: Instant| n as f64 / t.elapsed().as_secs_f64().max(1e-6);
    let t = Instant::now();
    let mut n = 0;
    for e in std::fs::read_dir(m)? {
        e?;
        n += 1;
    }
    let listed_s = rate(n, t);
    let t = Instant::now();
    for f in &names {
        std::fs::metadata(m.join(f))?;
    }
    Ok((listed_s, rate(META_FILES, t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_says_on_off_and_its_download_cap() {
        assert_eq!(parse("off\n"), None);
        assert_eq!(parse(""), Some(Switch::default()));
        assert_eq!(parse("on\ndownloads 2\n"), Some(Switch { downloads: Some(2) }));
    }

    /// A weight is the job's share of the NAS's bytes and creates, added, in thousandths; the
    /// rules hold the jobs' weights together within the window.
    #[test]
    fn a_weight_is_a_share_of_the_nas() {
        let l = Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        assert_eq!(l.weight(Rate { bytes_s: 25e6, creates_s: 0.0, ..Default::default() }), 250);
        assert_eq!(l.weight(Rate { bytes_s: 25e6, creates_s: 4.0, ..Default::default() }), 350);
        assert_eq!(l.weight(Rate::default()), 0);
        assert_eq!(l.weight(Rate { bytes_s: 1.0, creates_s: 0.0, ..Default::default() }), 1, "measured: never nothing");
        assert_eq!(l.weight(Rate { bytes_s: 900e6, creates_s: 0.0, ..Default::default() }), WHOLE, "at most the whole");
        let w = WHOLE;
        assert!(Limits::fits(600, 600, 400, w) && !Limits::fits(601, 601, 400, w));
        assert!(Limits::fits(0, 900, 1000, w), "the lead's own alone: a member's job goes");
        assert!(!Limits::fits(1, 900, 1000, w));
        // The lead's own planned job beside members' small jobs (task #396 follow-up): a whole one
        // goes while none leased weighs the whole; a lesser one once it has waited; none beside a
        // whole one.
        assert!(!Limits::fits_own(30, 1000, w) && Limits::fits_own_job(30, 0, 20, 1000, false, w));
        assert!(!Limits::fits_own_job(30, 0, 1000, 1000, true, w), "beside a whole one: never");
        // (Task #603: an every-step target of the lead's goes past the members' jobs only once aged.)
        assert!(
            !Limits::fits_own_target(30, 0, 20, 1000, false, w) && Limits::fits_own_target(103, 0, 103, 946, true, w)
        );
        assert!(!Limits::fits_own_target(103, 0, 1000, 946, true, w), "beside a whole one: never");
        assert!(!Limits::fits_own_job(300, 0, 200, 800, false, w) && Limits::fits_own_job(300, 0, 200, 800, true, w));
        // (Beside an aged one of the lead's own already past the budget: within the window with it.)
        assert!(!Limits::fits_own_job(1100, 800, 800, 800, true, w), "aged ones don't pile up");
        assert!(Limits::fits_own_job(1100, 800, 800, 200, true, w));
        assert!(Limits::fits_own_job(1100, 300, 300, 1000, false, w), "a whole one beside the lead's own smaller ones");
        assert!(Limits::fits(999, 999, 0, w));
        // A quiet step (the clean-up): alone, or measured and within the window; never aged past it.
        assert!(Limits::fits_quiet(0, 0, w) && Limits::fits_quiet(0, 1000, w));
        assert!(!Limits::fits_quiet(1, 0, w), "weighing nothing (no metadata rates probed): alone only");
        assert!(Limits::fits_quiet(700, 300, w) && !Limits::fits_quiet(701, 300, w));
        assert!(Limits::fits_own(600, 400, w) && !Limits::fits_own(601, 400, w));
        assert!(
            Limits::fits_own(0, 1000, w) && !Limits::fits_own(1, 1000, w),
            "the lead's own: within the window, or alone"
        );
        // Within a window of 500: the sums held to it; a job under a step isn't paced.
        assert!(Limits::fits(300, 300, 200, 500) && !Limits::fits(301, 301, 200, 500));
        assert!(Limits::fits_quiet(300, 200, 500) && !Limits::fits_quiet(301, 200, 500));
        assert!(Limits::fits(950, 950, STEP - 1, 500) && !Limits::fits(950, 950, STEP, 500));
        // (Grown past the whole: more goes.)
        assert!(Limits::fits(900, 900, 300, 1200) && Limits::fits_own(900, 300, 1200));
    }

    /// Cut under the whole, the pacing lets through only what fits the window, a job weighing
    /// under a step, or one beside nothing: never what the whole window refuses, and none of the
    /// rules' ways past the sum (the lead's own alone, an aged job).
    #[test]
    fn a_cut_window_never_admits_what_the_whole_refuses() {
        let whole = Limits::default();
        let cut = Limits { window: 450, ..Default::default() };
        let ws = [0, 1, 49, 150, 450, 700, 999, 1000];
        for &running in &ws {
            for &members in &ws {
                for &w in &ws {
                    for &heaviest in &ws {
                        for waited in [false, true] {
                            let job = |l: &Limits| {
                                Limits::fits_job(members, running, heaviest, w, waited, l.window) && l.paced(running, w)
                            };
                            let own = |l: &Limits| {
                                Limits::fits_own_job(running, members, heaviest, w, waited, l.window)
                                    && l.paced(running, w)
                            };
                            assert!(!job(&cut) || job(&whole));
                            assert!(!own(&cut) || own(&whole));
                            if job(&cut) || own(&cut) {
                                assert!(w < STEP || running == 0 || running + w <= 450, "{running} {w}");
                            }
                        }
                    }
                }
            }
        }
        assert!(!cut.paced(450, 150) && cut.paced(300, 150) && cut.paced(0, 1000) && cut.paced(450, STEP - 1));
        assert!(whole.paced(950, 150), "not cut: the rules decide");
    }

    /// A reading of one Mac answering in `ms`, the disks unread.
    fn at(p: &mut Pace, t: u64, ms: f64, in_use: u32, wants: Option<u32>) -> Verdict {
        p.read(t, &[("m1".to_string(), ms)], None, DEFAULT_TARGET_PCT, in_use, wants)
    }

    /// Near the NAS's best the window doubles a minute while it's what limits, until its first
    /// cut; past the knee it drops to half the weight in use at once, and once more only as that
    /// falls; between, it holds; then it grows a step a minute.
    #[test]
    fn the_window_grows_slowly_and_is_cut_quickly() {
        let mut p = Pace::default();
        assert_eq!(p.window(), STEP);
        // (Not what limits: nothing waits, and the weight in use is far under it.)
        assert_eq!(at(&mut p, 0, 5.0, 0, None), Verdict::Held);
        let mut t = 0;
        for _ in 0..6 {
            t += 10;
            assert_eq!(at(&mut p, t, 5.0, 0, Some(150)), Verdict::Grew);
        }
        assert_eq!(p.window(), 2 * STEP, "doubled in a minute");
        for _ in 0..18 {
            t += 10;
            at(&mut p, t, 5.0, 700, Some(150));
        }
        assert_eq!(p.window(), 16 * STEP, "slow start: doubling");
        // (Between `NEAR` and `SLOW` of its floor: held.)
        t += 10;
        assert_eq!(at(&mut p, t, 10.0, 700, Some(150)), Verdict::Held);
        assert_eq!(p.window(), 800);
        // (Past the knee: half what's in use, at once, not again while that stands.)
        t += 10;
        assert_eq!(at(&mut p, t, 16.0, 700, Some(150)), Verdict::Cut);
        assert_eq!(p.window(), 350);
        let why = p.why().unwrap();
        assert_eq!(
            why,
            "NAS work paced to 35 % of the NAS, 70 % in use (the NAS answering m1 in 16 ms, 3.2× its best)"
        );
        t += 10;
        at(&mut p, t, 16.0, 700, Some(150));
        assert_eq!(p.window(), 350, "the same use: not cut again");
        t += 10;
        at(&mut p, t, 16.0, 300, Some(150));
        assert_eq!(p.window(), 150, "its jobs ending, the NAS still slow: half what's left");
        assert!(p.why().is_some());
        // (Calm again: a step a minute now, no more doubling.)
        for _ in 0..6 {
            t += 10;
            assert_eq!(at(&mut p, t, 5.0, 150, Some(150)), Verdict::Grew);
        }
        assert_eq!(p.window(), 150 + STEP);
        assert!(p.why().is_none(), "the window over what's in use");
        // (Cut never under a step: an outside load with nothing of the pool's running.)
        t += 10;
        at(&mut p, t, 40.0, 0, Some(150));
        assert_eq!(p.window(), STEP);
        // (A lead starting beside jobs already leased: from what they weigh, then doubling.)
        let mut q = Pace::default();
        assert_eq!(at(&mut q, 0, 5.0, 700, None), Verdict::Grew);
        assert_eq!(q.window(), 786, "700, then a sixth of a minute's doubling");
    }

    /// The disks over the owner's target cut the window as a slow answer does; nothing read holds
    /// it.
    #[test]
    fn the_disks_over_the_target_cut_it_and_nothing_read_holds_it() {
        let mut p = Pace::default();
        assert_eq!(p.read(0, &[], None, 85, 100, Some(150)), Verdict::Unread);
        assert_eq!(
            p.read(10, &[], Some(&[40.0, 60.0]), 85, 100, Some(150)),
            Verdict::Grew,
            "the busiest within the target"
        );
        assert_eq!(p.load, Some(60.0));
        assert_eq!(
            p.read(20, &[], Some(&[95.0]), 85, 100, Some(150)),
            Verdict::Grew,
            "smoothed: one busy reading isn't over"
        );
        for k in 3..8 {
            p.read(k * 10, &[], Some(&[95.0]), 85, 100, Some(150));
        }
        assert_eq!(p.last.as_ref().unwrap().verdict, Verdict::Cut);
        assert_eq!(p.window(), 50, "half the 100 in use");
        assert!(p.why().unwrap().contains("its disks 91 % busy, over the target 85 %"), "{:?}", p.why());
        let w = p.window();
        assert_eq!(p.read(100, &[], None, 85, 100, Some(150)), Verdict::Unread);
        assert_eq!(p.window(), w);
    }

    /// Each Mac is read against its own floor: one away on a slow link answering at its usual
    /// time isn't a loaded NAS; its floor follows it over `FLOOR_S`, and a Mac no longer heard
    /// from is let go.
    #[test]
    fn each_mac_is_read_against_its_own_floor() {
        let mut p = Pace::default();
        let read = |p: &mut Pace, t: u64, near: f64, away: f64| {
            p.read(t, &[("m1".into(), near), ("m6".into(), away)], None, 85, 0, None)
        };
        read(&mut p, 0, 5.0, 40.0);
        read(&mut p, 10, 5.5, 44.0);
        let r = p.last.clone().unwrap();
        assert_eq!(r.verdict, Verdict::Held);
        assert_eq!(r.worst.as_ref().map(|a| (a.mac.as_str(), a.ms, a.floor_ms)), Some(("m1", 5.5, 5.0)));
        read(&mut p, 20, 5.0, 160.0);
        assert_eq!(p.last.as_ref().unwrap().verdict, Verdict::Cut, "m6's answers four times its best");
        // (Its floor, a slower link for good: followed once its old minutes pass `FLOOR_S`.)
        let mut t = 30;
        while t < FLOOR_S + 120 {
            read(&mut p, t, 5.0, 160.0);
            t += 10;
        }
        let a = p.last.as_ref().unwrap().worst.clone().unwrap();
        assert!(a.ratio() < store::aimd::NEAR, "{a:?}");
        // (A floor never under the least a NAS answers in.)
        p.read(t, &[("m1".into(), 0.1)], None, 85, 0, None);
        assert_eq!(p.last.as_ref().unwrap().worst.as_ref().unwrap().floor_ms, 2.0);
        p.read(t + FLOOR_S + 60, &[("m1".into(), 5.0)], None, 85, 0, None);
        assert_eq!(p.floors.keys().collect::<Vec<_>>(), ["m1"], "m6 let go");
    }

    /// The simulated NAS: its answer time over its best as its true load grows (`rho`, a share of
    /// what it can take): flat at first, steep near the whole, and past it the cliff (on 4 Oct,
    /// processes on two Macs in uninterruptible waits): a hundred times.
    fn simulated(rho: f64) -> f64 {
        if rho >= 1.0 {
            100.0
        } else {
            1.0 / (1.0 - rho.powi(3))
        }
    }

    /// A burst of `n` NAS-heavy jobs asked at once, each weighed `w` but loading the NAS `real`
    /// (thousandths of what it can take: its weight a measure of other runs, a warm cache's or a
    /// quicker Mac's), each running `secs`, beside an outside load `outside(t)` (thousandths), on
    /// two Macs (one near the NAS, 5 ms its best; one away, 40 ms). Asked again every reading as
    /// the scheduler asks the rules (`fits_job`, `paced`), within the pacing's window (`pace`) or
    /// the whole (not paced). The answer times over the NAS's best each reading (t, ratio), and
    /// when the last job ended.
    fn burst(
        pace: bool,
        n: usize,
        w: u32,
        real: u32,
        secs: u64,
        outside: impl Fn(u64) -> u32,
    ) -> (Vec<(u64, f64)>, u64) {
        let mut p = Pace::default();
        let (mut queue, mut running, mut ratios) = (n, Vec::<u64>::new(), Vec::new());
        let mut t = 0;
        loop {
            running.retain(|&end| end > t);
            if queue == 0 && running.is_empty() {
                return (ratios, t);
            }
            assert!(t < 24 * 3600, "the burst never ended: {queue} left");
            let rho = (running.len() as u32 * real + outside(t)) as f64 / WHOLE as f64;
            let r = simulated(rho);
            ratios.push((t, r));
            let in_use = running.len() as u32 * w;
            let win = if pace {
                p.read(
                    t,
                    &[("m1".into(), 5.0 * r), ("m6".into(), 40.0 * r)],
                    None,
                    85,
                    in_use,
                    (queue > 0).then_some(w),
                );
                p.window()
            } else {
                WHOLE
            };
            let l = Limits { window: win, ..Default::default() };
            while queue > 0 {
                let in_use = running.len() as u32 * w;
                let heaviest = if running.is_empty() { 0 } else { w };
                if !(Limits::fits_job(in_use, in_use, heaviest, w, false, win) && l.paced(in_use, w)) {
                    break;
                }
                running.push(t + secs);
                queue -= 1;
            }
            t += READ_EVERY_S;
        }
    }

    /// Done when (task #1417): a burst of NAS-heavy jobs settles below the latency knee on a
    /// simulated NAS. Held to the whole by weights that read the NAS light, the same burst drives
    /// it over the cliff and keeps it there; paced, it climbs to the knee and stays under it, every
    /// job done; an outside load arriving mid-burst is met by cuts, and it settles under the knee
    /// again once the jobs that were out end.
    #[test]
    fn a_burst_of_nas_heavy_jobs_settles_below_the_knee() {
        let knee = store::aimd::SLOW;
        // (Weighed 150, loading the NAS 250: the whole lets six in, 1.5 times what it can take.)
        let (unpaced, _) = burst(false, 30, 150, 250, 300, |_| 0);
        assert!(unpaced.iter().filter(|x| x.1 >= 100.0).count() * 2 > unpaced.len(), "unpaced: at the cliff");
        let (paced, end) = burst(true, 30, 150, 250, 300, |_| 0);
        assert!(
            paced.iter().all(|x| x.1 <= knee),
            "paced: never past the knee: {:?}",
            paced.iter().find(|x| x.1 > knee)
        );
        let busy = paced.iter().filter(|x| x.1 > store::aimd::NEAR).count();
        assert!(busy * 2 > paced.len(), "not starved: the NAS used past near its best most of the burst");
        assert!(end <= 30 * 300 / 2 + 600, "the burst done at two or three at once: {end} s");
        // (Weighed 150, loading 400: two at once at most.)
        let (paced, _) = burst(true, 20, 150, 400, 300, |_| 0);
        assert!(paced.iter().all(|x| x.1 <= knee), "{:?}", paced.iter().find(|x| x.1 > knee));
        // (An outside load of half the NAS from 30 to 60 minutes in: past the knee until the jobs
        // out then end (two of their runs at most), under it after.)
        let outside = |t: u64| if (1800..3600).contains(&t) { 500 } else { 0 };
        let (paced, end) = burst(true, 40, 150, 250, 300, outside);
        let after = |from: u64, to: u64| paced.iter().filter(move |x| x.0 >= from && x.0 < to);
        assert!(after(0, 1800).all(|x| x.1 <= knee));
        assert!(after(1800 + 2 * 300 + 30, 3600).all(|x| x.1 <= knee), "settled under the outside load");
        assert!(after(3600, end).all(|x| x.1 <= knee));
        assert!(after(1800, 3600).any(|x| x.1 > knee), "the outside load seen past the knee");
    }

    /// The limits are the best of the last day's probes.
    #[test]
    fn the_limits_are_the_best_of_the_days_probes() {
        let mut p = Probes::default();
        assert!(p.limits(None).is_none());
        p.add(Probe { at: 1000, bytes_s: 80e6, creates_s: 30.0, ..Default::default() });
        p.add(Probe { at: 2000, bytes_s: 50e6, creates_s: 45.0, ..Default::default() });
        assert_eq!(
            p.limits(Some(2)),
            Some(Limits { bytes_s: 80e6, creates_s: 45.0, downloads: Some(2), ..Default::default() })
        );
        p.add(Probe { at: 1000 + KEEP_S, bytes_s: 10e6, creates_s: 5.0, ..Default::default() });
        assert_eq!(p.limits(None).unwrap().bytes_s, 50e6, "a day old: let go");
    }

    /// Task #603: the probes survive a restart (the store's copy merged in), the day-old let go.
    #[test]
    fn the_probes_survive_a_restart() {
        let mut kept = Probes::default();
        kept.add(Probe { at: 1000, bytes_s: 86e6, creates_s: 30.0, ..Default::default() });
        kept.add(Probe { at: 4000, bytes_s: 66.5e6, creates_s: 42.3, ..Default::default() });
        let mut p = Probes::default();
        p.add(Probe { at: 4500, bytes_s: 60e6, creates_s: 40.0, ..Default::default() });
        p.merge(&kept, 5000);
        assert_eq!(p.0.iter().map(|q| q.at).collect::<Vec<_>>(), [1000, 4000, 4500], "oldest first");
        assert_eq!(p.limits(None).unwrap().bytes_s, 86e6, "the day's best kept");
        p.merge(&kept, 5000);
        assert_eq!(p.0.len(), 3, "merged again: nothing doubled");
        p.merge(&Probes::default(), 1000 + KEEP_S);
        assert_eq!(p.0.iter().map(|q| q.at).collect::<Vec<_>>(), [4000, 4500], "a day old: let go");
    }

    /// A probe measures and leaves nothing behind.
    #[test]
    fn a_probe_measures_and_removes_its_files() {
        let d = tempfile::tempdir().unwrap();
        let dir = probe_dir(&store::nasfs::NasRoot::local_for_test(d.path()), "m1.local");
        let p = probe(&dir).unwrap();
        assert!(p.bytes_s > 0.0 && p.creates_s > 0.0, "{p:?}");
        assert!(!dir.exists());
        // Its listing and stats measured only in the kept folder, settled: none the first time
        // (the folder made), then once its files are old.
        assert_eq!((p.listed_s, p.stats_s), (0.0, 0.0), "{p:?}");
        assert!(p.deletes_s > 0.0);
        let m = meta_dir(&dir);
        assert!(m.join(format!("m{}", META_FILES - 1)).exists());
        let old = std::time::SystemTime::now() - META_SETTLE * 2;
        for i in 0..META_FILES {
            let f = std::fs::File::options().write(true).open(m.join(format!("m{i}"))).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
        }
        let p = probe(&dir).unwrap();
        assert!(p.listed_s > 0.0 && p.stats_s > 0.0, "{p:?}");
        assert!(!dir.exists() && m.exists());
        let probe_m = probe_dir(&store::nasfs::NasRoot::local_for_test(d.path()), "m1.local");
        assert_eq!(meta_dir(&probe_m).file_name().unwrap(), "m1_local-meta");
    }

    /// At start, this host's folders a crash left go; this process's and other hosts' stay.
    #[test]
    fn a_crashs_probe_folders_of_this_host_are_cleaned() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join(PROBE_DIR);
        for f in ["m4_local-1", "m4_local-22", "m1_local-3", "m4_local-x"] {
            std::fs::create_dir_all(base.join(f)).unwrap();
            std::fs::write(base.join(f).join("s0"), b"x").unwrap();
        }
        let own = probe_dir(&store::nasfs::NasRoot::local_for_test(d.path()), "m4.local");
        std::fs::create_dir_all(&own).unwrap();
        assert_eq!(clean_stale(&store::nasfs::NasRoot::local_for_test(d.path()), "m4.local"), 2);
        assert!(own.exists() && base.join("m1_local-3").exists() && base.join("m4_local-x").exists());
        assert!(!base.join("m4_local-1").exists() && !base.join("m4_local-22").exists());
    }
}
