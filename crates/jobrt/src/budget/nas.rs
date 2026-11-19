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
//!   less than a step (`Tuning::small`) isn't paced: such jobs are held to the whole together.
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
/// best, and cut quickly once its answers slow, before the cliff. Every `Tuning::read_s` the lead
/// reads (`Sample`):
///
/// - each Mac's NAS answer time over its **floor** (`Answer`: both the Mac's own, from its live
///   store, carried by each beat): its listing of the project's folder every 5 s while a build
///   runs, its mean over the last `Tuning::span_s`; a listing past its deadline, or while the last
///   is still stuck, counts as the deadline or the stuck call's age. A NAS's load adds
///   milliseconds to every Mac's answers; it doesn't multiply each floor: each Mac is ranked by
///   its **excess** (its answer time less its floor) over the least floor among those answering,
///   and its ratio is one more than that (alone, its own answer over its floor). With two Macs or
///   more read, the second-highest counts: one Mac's own link (the M1's Wi-Fi, a Mac away on
///   Tailscale) never paces the pool alone, while a NAS slowdown, the same milliseconds on a Mac
///   at home and one away, is read at the home Mac's scale. A Mac whose NAS calls miss their
///   deadline ranks past every knee, and one at home missing them is past the knee whatever its
///   rank. Growth is judged among the Macs whose floor is within `Tuning::slow` of the least (a
///   Mac on a slower link jitters by milliseconds of its own floor's scale, past the near band at
///   the least floor's: a lucky span would read a loaded NAS as near), the second-highest of them
///   near with three or more, every one with fewer;
/// - the NAS's busiest disk's busy share (SNMP, smoothed by `SMOOTH`) against the owner's target
///   (`Target`, `scenic nas-target`, by default `DEFAULT_TARGET_PCT`).
///
/// Then, as the NAS's own readers decide their reads in flight (store::aimd):
///
/// - **past the knee** (the ratio counted past `Tuning::slow`, or the disks over the target): the
///   window drops to `Tuning::decrease` of the weight in use at once, never under `Tuning::step`;
///   the slow start's threshold is set there. The jobs running go on, and nothing more that's
///   paced starts until enough of them end. No new cut until the weight in use has fallen to the
///   window and its answers had a `span_s` since to show it (store::aimd's once a window: a cut
///   compounding on readings of the jobs out before it takes the pool to nothing);
/// - **near the best** (within `Tuning::near`, the disks within the target, and a Mac's answers
///   read: the disks alone, or no Mac listing the NAS, never grow it): it grows, only while
///   it's what limits (the weight in use and the least a job waiting for the budget weighs past
///   it; none waiting, the weight in use within a step of it: a window no job asks for isn't credit
///   for a later burst, nor is room left by a job just ended), only on readings every Mac holding
///   leased jobs gave fresh (none older than its span and a beat: no growth on stale data), and
///   only once the jobs last started have run a `span_s` (their answers in the reading): doubling a
///   `Tuning::grow_s` up to the slow start's threshold (none before the first cut), then a step a
///   `grow_s`; and never more than one job past the load its readings tested: the weight in use
///   and the next job waiting (a step at least), or under the threshold twice the weight in use
///   (at least two steps) where that's more. A window grown on readings of a light load would
///   admit several heavy jobs in one jump; one job's is what the next reading past the knee cuts
///   back;
/// - **between**, or nothing read: it holds.
///
/// A reading that held says why (`Verdict::Held`'s `Hold`: the first of those gates it failed),
/// for the lead's status (`Reading::line`): a held reading without its reason can't be made.
///
/// A job beside nothing that the window doesn't take goes all the same, but not for
/// `Tuning::lone_s` from a reading past the knee (`Pace::lone`: beside an outside load near the
/// cliff, each lone job would take the NAS over it, and the hold is all that keeps it off), each
/// hold in a row twice as long, to `Tuning::lone_most_s`; a reading near the best ends it. Then
/// one goes, so nothing waits for good.
///
/// It starts at a step, or at what the jobs already leased weigh where that's more (a lead's
/// coordinator starting knows nothing of the NAS's load but what runs on it), unless the lead kept
/// its state (`State`) younger than `Tuning::restore_s`: a
/// restart then takes the window, its threshold and its cut up again (task #603's pattern: a
/// publish mid-burst must not undo a cut). It has no ceiling: the weights are estimates, and the
/// latency, not they, says when the NAS is full.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pace {
    st: State,
    /// Whether the state was taken up from a kept one (`restore`): the window isn't lifted to the
    /// weight in use at the first reading.
    restored: bool,
    /// The weight in use at the reading before, and when it last rose (M2: no growth until the jobs
    /// last started have run a span).
    prev_in_use: u32,
    rose_at: Option<u64>,
    /// The busiest disk's busy share, smoothed (%; None: not read).
    pub load: Option<f64>,
    /// The target it was last read against (%), and the tuning.
    pub target: u32,
    pub tuning: Tuning,
    /// The last reading (None: none yet).
    pub last: Option<Reading>,
}

/// The pacing's state the lead keeps in its store (agentcore::coord::kept's `PACE`), so a restart
/// takes it up (`Pace::restore`): the window, the slow start's threshold (None: never cut, it
/// doubles), when it was last cut, when the weight in use fell to the window since (the next cut's
/// hold-off), and since when a job beside nothing is held back (`Pace::lone`), all as of `at`
/// (unix s).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct State {
    pub at: u64,
    pub window: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lone_at: Option<u64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub lone_n: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Default for State {
    fn default() -> Self {
        State { at: 0, window: STEP as f64, threshold: None, cut_at: None, settled_at: None, lone_at: None, lone_n: 0 }
    }
}

/// One reading of the pacing (`Pace::read`): when (unix s), the Mac whose answers counted and its
/// ratio (one more than its excess over the least floor, at most `RATIO_MOST`), whether it was past
/// the knee, the disks' smoothed load, the weight in use and the room the next job waiting needs
/// (`Sample::wants`), what it did (and why, where it held), the slow start's threshold after it,
/// and until when a job beside nothing is held back (`Pace::lone`; None: it isn't).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Reading {
    pub at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counted: Option<Answer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
    pub past: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_pct: Option<f64>,
    pub in_use: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wants: Option<u32>,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lone_until: Option<u64>,
}

/// Why a reading held the window (`Verdict::Held`): the first of `Pace::read`'s gates it failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hold {
    /// Past the knee within the last cut's hold-off: the weight in use not yet down to the window,
    /// or no `Tuning::span_s` since it fell there.
    HoldOff,
    /// No Mac's answers read (none listing the NAS), the disks alone within the target.
    NoAnswer,
    /// Under the knee, not near the best.
    NotNear,
    /// A Mac holding leased jobs gave no fresh answers, or an answer read isn't fresh.
    NotFresh,
    /// The jobs last started haven't run a `Tuning::span_s`.
    Started,
    /// The window isn't what limits: the room the next job waiting needs fits beside what's in use
    /// (none waiting: the weight in use more than a step under it).
    NotLimiting,
}

impl Hold {
    /// What it says, for the lead's status.
    pub fn words(self) -> &'static str {
        match self {
            Hold::HoldOff => "past the knee, within the last cut's hold-off",
            Hold::NoAnswer => "no Mac's answers read",
            Hold::NotNear => "the answers counted not near their best",
            Hold::NotFresh => "not every Mac's answers fresh",
            Hold::Started => "the jobs last started not a span old",
            Hold::NotLimiting => "the window not what limits",
        }
    }
}

impl Reading {
    /// The pacing's line in the lead's status at `now` (unix s), its window `window`: the window,
    /// its threshold, the weight in use and the room the next job waiting needs (`Sample::wants`),
    /// the reading's ratio and disk load, its verdict and why it held, and the lone way's hold.
    pub fn line(&self, window: u32, now: u64) -> String {
        let th = self.threshold.map_or(String::new(), |t| format!(" (threshold {})", pct(t)));
        let wants = self.wants.map_or(", none held back for it".to_string(), |w| {
            format!(", the next job held back for it needs {}", pct(w))
        });
        let ratio = self.ratio.map_or(String::new(), |x| format!(", answers {x:.1}× their best"));
        let load = self.load_pct.map_or(String::new(), |l| format!(", disks {l:.0} % busy"));
        let verdict = match self.verdict {
            Verdict::Grew => "grew".to_string(),
            Verdict::Cut => "cut".to_string(),
            Verdict::Unread => "nothing read".to_string(),
            Verdict::Held(h) => format!("held: {}", h.words()),
        };
        let lone = self.lone_until.map_or(String::new(), |u| {
            format!("; a job beside nothing held back {} min more", u.saturating_sub(now).div_ceil(60))
        });
        format!(
            "NAS pacing: window {}{th}, {} in use{wants}; last reading {} s ago{ratio}{load}: {verdict}{lone}",
            pct(window),
            pct(self.in_use),
            now.saturating_sub(self.at)
        )
    }
}

/// A Mac's NAS answers as the lead reads them: its NAS answer time (ms, its mean over its span)
/// and its floor (ms, its own: the least such mean over `Tuning::floor_s` on its link), whether it
/// is at home (the floor's link: a Mac moved to Tailscale is another), whether its NAS calls
/// missed their deadline lately, and whether the reading is fresh (no older than its span and a
/// beat, its floor not new on a new link: it may cut, never grow).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Answer {
    pub mac: String,
    pub home: bool,
    pub ms: f64,
    pub floor_ms: f64,
    pub missed: bool,
    pub fresh: bool,
}

impl Answer {
    /// Its ratio against `least`, the least floor among the Macs answering: one more than its
    /// excess (answer time less its own floor) over it, at most `RATIO_MOST`; its calls missing
    /// their deadline, `RATIO_MOST`.
    pub fn ratio(&self, least: f64) -> f64 {
        if self.missed {
            RATIO_MOST
        } else {
            (1.0 + (self.ms - self.floor_ms).max(0.0) / least.max(f64::MIN_POSITIVE)).min(RATIO_MOST)
        }
    }
}

/// The most a reading's ratio is taken to be (a Mac missing its deadlines; the cliff's).
pub const RATIO_MOST: f64 = 100.0;

/// A weight (thousandths of the NAS) as the lead's lines say it: a whole percent, rounded.
pub fn pct(w: u32) -> String {
    format!("{:.0} %", f64::from(w) / 10.0)
}

/// What a reading did to the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Near the best, and the window what limits: grown.
    Grew,
    /// Past the knee: cut.
    Cut,
    /// Held, and why: between, the window not what limits, its readings not fresh, a cut's
    /// hold-off.
    Held(Hold),
    /// Nothing read (no Mac's answer time, no disk reading): held.
    #[default]
    Unread,
}

impl Verdict {
    /// Why it held (None: it didn't).
    pub fn hold(self) -> Option<Hold> {
        match self {
            Verdict::Held(h) => Some(h),
            _ => None,
        }
    }
}

/// What the lead reads each `Tuning::read_s` (`Pace::read`): at `t` (unix s), every Mac's answers
/// (`Answer`), whether a Mac holding leased jobs gave none fresh (`blind`), the disks' busy shares
/// (%; None: not read) against the target `pct`, the weight the pool's leased jobs hold, and the
/// room the next job waiting for the budget needs (agentcore's `Sched::nas_wants`: the least one
/// waiting weighs, or the room held for an aged one; None: none waits; the window is what limits once it
/// refuses that beside what's in use, or, none waiting, a step).
#[derive(Clone, Copy, Debug)]
pub struct Sample<'a> {
    pub t: u64,
    pub answers: &'a [Answer],
    pub blind: bool,
    pub busy: Option<&'a [f64]>,
    pub pct: u32,
    pub in_use: u32,
    pub wants: Option<u32>,
}

/// The pacing's bounds (`Pace`), each the pool's setting (`KNOBS`: agentcore::coord::asks, `scenic
/// pool set <name>`, the build page), read by the lead at each reading and by each Mac for its
/// answers (`span_s`, `floor_s`); not set, its default (`Tuning::default`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tuning {
    /// The window's additive step a `grow_s`, and its least (thousandths of the NAS).
    pub step: u32,
    /// A job weighing less (thousandths) isn't paced: held to the whole, not the window.
    pub small: u32,
    /// The window past the knee: this share of the weight in use.
    pub decrease: f64,
    /// The span a Mac's floor is its least answer over (s): long enough that a busy stretch, an
    /// hour-long job's, never becomes the floor; a link change starts it again.
    pub floor_s: u64,
    /// Past this many times its floor, a Mac's answers are past the knee.
    pub slow: f64,
    /// Within this many times its floor, near its best.
    pub near: f64,
    /// The growth's cadence (s): the window doubles, or grows a step, in this long.
    pub grow_s: u64,
    /// How often the lead reads (s).
    pub read_s: u64,
    /// The span a Mac's NAS answer time is its mean over (s): its listings every 5 s.
    pub span_s: u64,
    /// The longest a kept state is taken up by a restarted lead (s).
    pub restore_s: u64,
    /// How long a reading past the knee holds a lone job back (s; `Pace::lone`), and the longest
    /// its doubling makes a hold in a row (s; never under `lone_s`).
    pub lone_s: u64,
    pub lone_most_s: u64,
    /// How many spans of its link's history a Mac's floor needs before it counts (measured within
    /// `floor_s`): younger, its answers may cut, never grow (agentcore::agent::live).
    pub young_spans: u64,
}

/// How often the lead reads the pacing, by default (s).
pub const READ_EVERY_S: u64 = 10;
/// A Mac's floor's span, by default (s): the day, as the probes' best is (`KEEP_S`).
pub const FLOOR_S: u64 = 24 * 3600;
/// The window's additive step, its least, and the least a job is paced at, by default
/// (thousandths of the NAS).
pub const STEP: u32 = 50;
/// The window past the knee, by default: this share of the weight in use.
pub const DECREASE: f64 = 0.5;
/// The growth's cadence, by default (s).
pub const GROW_S: u64 = 60;
/// The span a Mac's NAS answer time is averaged over, by default (s): its listings every 5 s, six
/// of them, so one slow answer isn't the NAS loaded.
pub const SPAN_S: u64 = 30;
/// The longest a kept state is taken up, by default (s).
pub const RESTORE_S: u64 = 30 * 60;
/// How long a reading past the knee holds a lone job back, by default (s): as long as a job waits
/// for the budget before it goes as an aged one (`OWN_AGE`).
pub const LONE_S: u64 = OWN_AGE.as_secs();
/// The longest a hold in a row lasts, by default (s): 8 times `LONE_S`.
pub const LONE_MOST_S: u64 = 8 * LONE_S;
/// The spans of its link's history a Mac's floor needs, by default: ten minutes of listings at
/// the default span.
pub const YOUNG_SPANS: u64 = 20;

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            step: STEP,
            small: STEP,
            decrease: DECREASE,
            floor_s: FLOOR_S,
            slow: store::aimd::SLOW,
            near: store::aimd::NEAR,
            grow_s: GROW_S,
            read_s: READ_EVERY_S,
            span_s: SPAN_S,
            restore_s: RESTORE_S,
            lone_s: LONE_S,
            lone_most_s: LONE_MOST_S,
            young_spans: YOUNG_SPANS,
        }
    }
}

/// One of the pacing's bounds as a pool setting (crate::settings::Knob).
pub type Knob = crate::settings::Knob<Tuning>;

/// The pacing's bounds as the pool's settings (agentcore::coord::asks): each a number in its range,
/// null its default.
pub const KNOBS: [Knob; 13] = [
    Knob {
        name: "nas-step",
        what: "the window's step a growth cadence, and its least",
        unit: "‰",
        least: 10.0,
        most: 500.0,
        get: |k| k.step as f64,
        set: |k, v| k.step = v as u32,
    },
    Knob {
        name: "nas-small",
        what: "a job weighing less isn't paced (held to the whole)",
        unit: "‰",
        least: 0.0,
        most: 500.0,
        get: |k| k.small as f64,
        set: |k, v| k.small = v as u32,
    },
    Knob {
        name: "nas-decrease",
        what: "the window past the knee, a share of the weight in use",
        unit: "×",
        least: 0.1,
        most: 0.9,
        get: |k| k.decrease,
        set: |k, v| k.decrease = v,
    },
    Knob {
        name: "nas-floor-hours",
        what: "a Mac's best NAS answer: its least over this span",
        unit: "h",
        least: 1.0,
        most: 24.0,
        get: |k| k.floor_s as f64 / 3600.0,
        set: |k, v| k.floor_s = (v * 3600.0) as u64,
    },
    Knob {
        name: "nas-slow",
        what: "the knee: answers past this many times their best cut the window",
        unit: "×",
        least: 1.5,
        most: 10.0,
        get: |k| k.slow,
        set: |k, v| k.slow = v,
    },
    Knob {
        name: "nas-near",
        what: "near the best: answers within this many times it let the window grow",
        unit: "×",
        least: 1.05,
        most: 2.0,
        get: |k| k.near,
        set: |k, v| k.near = v,
    },
    Knob {
        name: "nas-grow-seconds",
        what: "the growth's cadence: the window doubles, or grows a step, in this long",
        unit: "s",
        least: 10.0,
        most: 600.0,
        get: |k| k.grow_s as f64,
        set: |k, v| k.grow_s = v as u64,
    },
    Knob {
        name: "nas-read-seconds",
        what: "how often the lead reads the NAS's answers and disks",
        unit: "s",
        least: 2.0,
        most: 60.0,
        get: |k| k.read_s as f64,
        set: |k, v| k.read_s = v as u64,
    },
    Knob {
        name: "nas-span-seconds",
        what: "a Mac's NAS answer time: its mean over this span",
        unit: "s",
        least: 10.0,
        most: 300.0,
        get: |k| k.span_s as f64,
        set: |k, v| k.span_s = v as u64,
    },
    Knob {
        name: "nas-restore-minutes",
        what: "a restarted lead takes its kept window up if this young",
        unit: "min",
        least: 0.0,
        most: 60.0,
        get: |k| k.restore_s as f64 / 60.0,
        set: |k, v| k.restore_s = (v * 60.0) as u64,
    },
    Knob {
        name: "nas-lone-minutes",
        what: "a reading past the knee holds a job beside nothing back this long",
        unit: "min",
        least: 1.0,
        most: 120.0,
        get: |k| k.lone_s as f64 / 60.0,
        set: |k, v| k.lone_s = (v * 60.0) as u64,
    },
    Knob {
        name: "nas-lone-most-minutes",
        what: "holds of a job beside nothing in a row double up to this long",
        unit: "min",
        least: 10.0,
        most: 240.0,
        get: |k| k.lone_most_s as f64 / 60.0,
        set: |k, v| k.lone_most_s = (v * 60.0) as u64,
    },
    Knob {
        name: "nas-young-spans",
        what: "a Mac's floor counts once its link has this many spans measured",
        unit: "spans",
        least: 5.0,
        most: 100.0,
        get: |k| k.young_spans as f64,
        set: |k, v| k.young_spans = v as u64,
    },
];

/// The knob named `name`.
pub fn knob(name: &str) -> Option<&'static Knob> {
    KNOBS.iter().find(|k| k.name == name)
}

impl Tuning {
    /// The tuning with each knob's value as `get` says it (None, or out of its range: its default);
    /// near never past the knee, the longest hold never under one.
    pub fn of(get: impl Fn(&str) -> Option<f64>) -> Tuning {
        let mut k = Tuning::default();
        for n in &KNOBS {
            if let Some(v) = get(n.name).filter(|v| (n.least..=n.most).contains(v)) {
                (n.set)(&mut k, v);
            }
        }
        k.near = k.near.min(k.slow);
        k.lone_most_s = k.lone_most_s.max(k.lone_s);
        k
    }
}

/// The smoothing's weight of a new disk reading (one each 10 s: about half a minute's memory, so
/// one busy second doesn't cut the window, nor one quiet one grow it).
pub const SMOOTH: f64 = 0.3;

impl Pace {
    /// The window now (thousandths of the NAS).
    pub fn window(&self) -> u32 {
        self.st.window.round() as u32
    }

    /// Its state to keep (`State`), as of `t`.
    pub fn state(&self, t: u64) -> State {
        State { at: t, ..self.st.clone() }
    }

    /// The kept state `s` taken up at `t` when it's younger than `restore_s`: whether it was.
    pub fn restore(&mut self, s: &State, t: u64, restore_s: u64) -> bool {
        let young = t.saturating_sub(s.at) < restore_s && s.window.is_finite() && s.window > 0.0;
        if young {
            self.st = s.clone();
            self.restored = true;
        }
        young
    }

    /// Whether a job beside nothing that the window doesn't take may go all the same
    /// (`Limits::alone`): not for `lone_for` from a reading past the knee (a lone job beside an
    /// outside load that holds the NAS near its cliff goes over it; the knee's reading may be its
    /// own); open again once the hold has lasted, or a reading near the best has ended it.
    pub fn lone(&self) -> bool {
        let at = self.last.as_ref().map_or(0, |r| r.at);
        self.st.lone_at.is_none_or(|s| at >= s + self.lone_for())
    }

    /// How long the lone way's hold lasts (s): `Tuning::lone_s`, doubled for each hold since the
    /// NAS last answered near its best, to `Tuning::lone_most_s` at most (an outside load that
    /// holds the NAS near its cliff for hours lets a lone job over it ever more rarely).
    fn lone_for(&self) -> u64 {
        let k = &self.tuning;
        let doubled = 1u64.checked_shl(self.st.lone_n.saturating_sub(1)).unwrap_or(u64::MAX);
        k.lone_s.saturating_mul(doubled).min(k.lone_most_s.max(k.lone_s))
    }

    /// A reading (`Sample`) by the bounds `k`: what it did.
    pub fn read(&mut self, s: &Sample, k: &Tuning) -> Verdict {
        match s.busy.and_then(|b| b.iter().copied().filter(|b| b.is_finite()).reduce(f64::max)) {
            Some(now) => self.load = Some(self.load.map_or(now, |l| l + SMOOTH * (now - l))),
            None => self.load = None,
        }
        (self.target, self.tuning) = (s.pct, *k);
        let t = s.t;
        let (in_use, step) = (s.in_use as f64, k.step as f64);
        // (Each Mac's answers by its link, its largest excess; ranked by excess over the least
        // floor answering; two or more read, the second counts.)
        let mut each: Vec<&Answer> = Vec::new();
        let excess = |a: &Answer| if a.missed { f64::INFINITY } else { a.ms - a.floor_ms };
        for a in s.answers.iter().filter(|a| a.missed || (a.ms.is_finite() && a.ms > 0.0 && a.floor_ms > 0.0)) {
            match each.iter_mut().find(|b| b.mac == a.mac && b.home == a.home) {
                Some(b) if excess(a) > excess(b) => *b = a,
                Some(_) => {}
                None => each.push(a),
            }
        }
        let least = each.iter().filter(|a| a.floor_ms > 0.0).map(|a| a.floor_ms).fold(f64::INFINITY, f64::min);
        each.sort_by(|a, b| b.ratio(least).total_cmp(&a.ratio(least)));
        // (A Mac at home missing its deadlines: past the knee whatever its rank.)
        let home_missed = each.iter().find(|a| a.home && a.missed).copied();
        let counted = home_missed.or(each.get(usize::from(each.len() >= 2)).copied()).cloned();
        let ratio = counted.as_ref().map(|a| a.ratio(least));
        let peers: Vec<&Answer> = each.iter().copied().filter(|a| a.floor_ms <= least * k.slow).collect();
        let near = peers.get(usize::from(peers.len() >= 3)).is_none_or(|a| a.ratio(least) <= k.near);
        let fresh = !s.blind && each.iter().all(|a| a.fresh);
        let over = self.load.is_some_and(|l| l > s.pct as f64);
        let past = over || ratio.is_some_and(|r| r > k.slow);
        // (The lone way's hold: begun at a reading past the knee; once it has lasted its time
        // (`lone_for`), let go by a reading under the knee or a job going beside nothing, a reading
        // past it then beginning another, twice as long; a reading near the best ends it whenever,
        // and its doubling.)
        let expired = self.st.lone_at.is_some_and(|at| t >= at + self.lone_for());
        if expired && (!past || (self.prev_in_use == 0 && s.in_use > 0)) {
            self.st.lone_at = None;
        }
        if near && !past && ratio.is_some() {
            (self.st.lone_at, self.st.lone_n) = (None, 0);
        }
        if past && self.st.lone_at.is_none() {
            self.st.lone_at = Some(t);
            self.st.lone_n = self.st.lone_n.saturating_add(1);
        }
        if self.last.is_none() && !self.restored {
            self.st.window = self.st.window.max(in_use);
        }
        if s.in_use > self.prev_in_use {
            self.rose_at = Some(t);
        }
        self.prev_in_use = s.in_use;
        let st = &mut self.st;
        if st.cut_at.is_some() && st.settled_at.is_none() && in_use <= st.window {
            st.settled_at = Some(t);
        }
        let dt = self.last.as_ref().map_or(k.read_s, |r| t.saturating_sub(r.at)).min(k.grow_s) as f64
            / k.grow_s.max(1) as f64;
        let unread = ratio.is_none() && self.load.is_none();
        // (The most past the load its readings tested it grows to: one job more, the next one
        // waiting (a step at least, none waiting); in the slow start, twice the weight in use too
        // (two steps at least). A jump of one job's weight, which the next reading past the knee
        // cuts back.)
        let additive = st.threshold.is_some_and(|th| st.window >= th);
        let one_more = in_use + s.wants.map_or(0.0, f64::from).max(step);
        let ceiling = if additive { one_more } else { one_more.max(2.0 * in_use.max(step)) };
        let hold = if unread {
            None
        } else if past {
            (st.cut_at.is_some() && st.settled_at.is_none_or(|at| t < at + k.span_s)).then_some(Hold::HoldOff)
        } else if ratio.is_none() {
            Some(Hold::NoAnswer)
        } else if !near {
            Some(Hold::NotNear)
        } else if !fresh {
            Some(Hold::NotFresh)
        } else if self.rose_at.is_some_and(|r| t < r + k.span_s) {
            Some(Hold::Started)
        } else if in_use + s.wants.map_or(step, f64::from) <= st.window {
            Some(Hold::NotLimiting)
        } else {
            None
        };
        let verdict = match hold {
            _ if unread => Verdict::Unread,
            Some(h) => Verdict::Held(h),
            None if past => {
                let to = (in_use * k.decrease).max(step);
                st.window = st.window.min(to).max(step);
                st.threshold = Some(to);
                st.cut_at = Some(t);
                st.settled_at = (in_use <= st.window).then_some(t);
                Verdict::Cut
            }
            None => {
                let grown = match st.threshold {
                    Some(_) if additive => st.window + step * dt,
                    th => (st.window * det::Det::dexp2(dt)).min(th.unwrap_or(f64::INFINITY)).max(st.window),
                };
                st.window = grown.min(ceiling);
                Verdict::Grew
            }
        };
        let threshold = st.threshold.map(|th| th.round() as u32);
        let load_pct = self.load.map(|l| (l * 10.0).round() / 10.0);
        let lone_until = self.st.lone_at.map(|at| at + self.lone_for()).filter(|&u| t < u);
        self.last = Some(Reading {
            at: t,
            counted,
            ratio,
            past,
            load_pct,
            in_use: s.in_use,
            wants: s.wants,
            verdict,
            threshold,
            lone_until,
        });
        verdict
    }

    /// Why the pacing holds new NAS work back, for the lead's waiting lines: the window under the
    /// weight in use (None: it isn't).
    pub fn why(&self) -> Option<String> {
        let r = self.last.as_ref()?;
        let w = self.window();
        (w < r.in_use).then(|| {
            let cause = match (&r.counted, r.load_pct, r.ratio) {
                (_, Some(l), _) if l > self.target as f64 => {
                    format!("its disks {l:.0} % busy, over the target {} %", self.target)
                }
                (Some(a), ..) if a.missed => format!("missing its deadlines from {}", a.mac),
                (Some(a), _, Some(x)) if x > self.tuning.slow => {
                    format!("answering {} in {:.0} ms, {:.1}× its best", a.mac, a.ms, x)
                }
                _ => match r.verdict.hold() {
                    Some(h) => format!("cut lately, not grown since: {}", h.words()),
                    None => "cut lately".to_string(),
                },
            };
            format!("NAS work paced to {} of the NAS, {} in use (the NAS {cause})", pct(w), pct(r.in_use))
        })
    }
}

/// The window the rules hold the jobs' weights to (`Limits::win`): the pacing's, and the weight
/// under which a job isn't paced (`Tuning::small`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Win {
    pub window: u32,
    pub small: u32,
}

impl Win {
    /// The whole NAS, jobs under a default step unpaced (a test's, the forecast's).
    pub const WHOLE: Win = Win { window: WHOLE, small: STEP };

    /// Whether a job of weight `w` beside leased jobs weighing `running` keeps within it: weighing
    /// nothing, within the window, or, weighing less than `small`, within the whole (a job under a
    /// step isn't paced, but small ones together never pass the whole).
    pub fn takes(self, running: u32, w: u32) -> bool {
        let with = running.saturating_add(w);
        w == 0 || with <= self.window || (w < self.small && with <= self.window.max(WHOLE))
    }
}

impl Limits {
    /// The window these hold the jobs to (`Win`).
    pub fn win(&self) -> Win {
        Win { window: self.window, small: self.small }
    }

    /// The pacing (`Pace`) for a job of weight `w` beside leased jobs weighing `running`: a job
    /// going beside nothing (while `alone`: `Pace::lone`), or one the window takes (`Win::takes`)
    /// goes; and while the window is the whole or more (not cut), whatever the `fits*` rules let
    /// through (their ways past the sum: the lead's own alone, an aged job). Only ever narrows them.
    pub fn paced(&self, running: u32, w: u32) -> bool {
        (running == 0 && self.alone) || self.window >= WHOLE || self.win().takes(running, w)
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
    /// window; `WHOLE` till it's read), and the weight under which a job isn't paced
    /// (`Tuning::small`); whether a job may go beside nothing whatever the window (`Pace::lone`).
    pub window: u32,
    pub small: u32,
    pub alone: bool,
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
            small: STEP,
            alone: true,
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
    /// which the members' (all but the lead's own) weigh `members`, in the window `win`: the window
    /// takes it (`Win::takes`), or, when no member's job weighs anything, whatever the lead's own
    /// weigh (they're always about: a job that fits only beside nothing would never go).
    pub fn fits(members: u32, running: u32, w: u32, win: Win) -> bool {
        members == 0 || win.takes(running, w)
    }

    /// `fits` for a member's job, which would starve beside the members' other jobs: once it has
    /// waited `OWN_AGE` for the budget alone (`waited`) it goes too while no leased job weighs the
    /// whole (`heaviest`, the most one weighs), if it weighs the whole itself or more than the leased
    /// jobs together: it waits for heavier work to end, never for a stream of lighter (task #603: a
    /// 946 one aged beside a 103 one could never go, and the room held for it stopped the pool);
    /// once it's in, the leased jobs outweigh any other, so aged ones never pile up.
    pub fn fits_job(members: u32, running: u32, heaviest: u32, w: u32, waited: bool, win: Win) -> bool {
        Self::fits(members, running, w, win) || (waited && heaviest < WHOLE && (w >= WHOLE || running < w))
    }

    /// Whether a job of the lead's own of weight `w` may start beside leased jobs weighing
    /// `running` together, in the window `win`: the window takes it (`Win::takes`), or beside
    /// nothing that weighs anything (a job past the window alone).
    fn fits_own(running: u32, w: u32, win: Win) -> bool {
        running == 0 || win.takes(running, w)
    }

    /// `fits_own` for a target of the lead's own every-step asks (agentcore::coord's `pick_every`),
    /// which members' small jobs would otherwise hold back for good (each lease's weight keeps
    /// `running` above zero): it goes too once it has waited `OWN_AGE` for the budget alone
    /// (`waited`), no leased job weighs the whole (`heaviest`, the most one weighs) and the lead's
    /// own leased jobs (`own`, their weight together) leave it room within the window: aged ones
    /// go past the members' jobs alone, one at a time, never piling up (task #603).
    pub fn fits_own_target(running: u32, own: u32, heaviest: u32, w: u32, waited: bool, win: Win) -> bool {
        Self::fits_own(running, w, win) || (waited && heaviest < WHOLE && own.saturating_add(w) <= win.window)
    }

    /// `fits_own_target` for the lead's own planned job (agentcore::coord's `own_nas_fits`): also, not
    /// waiting, once it weighs the whole itself while no leased job weighs the whole, so a whole
    /// one isn't held back by the lead's own stream of smaller jobs.
    pub fn fits_own_job(running: u32, own: u32, heaviest: u32, w: u32, waited: bool, win: Win) -> bool {
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
    pub fn fits_quiet(running: u32, w: u32, win: Win) -> bool {
        running == 0 || (w > 0 && running.saturating_add(w) <= win.window)
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
            ..Default::default()
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
        let w = Win::WHOLE;
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
        // Within a window of 500: the sums held to it; a job under a step isn't paced, but small
        // ones together are held to the whole (they never pass every sum).
        let cut = Win { window: 500, small: STEP };
        assert!(Limits::fits(300, 300, 200, cut) && !Limits::fits(301, 301, 200, cut));
        assert!(Limits::fits_quiet(300, 200, cut) && !Limits::fits_quiet(301, 200, cut));
        assert!(Limits::fits(950, 950, STEP - 1, cut) && !Limits::fits(950, 950, STEP, cut));
        assert!(!Limits::fits(960, 960, STEP - 1, cut) && !Limits::fits_own(960, STEP - 1, cut), "past the whole");
        assert!(Limits::fits(960, 960, 0, cut), "weighing nothing: always");
        assert!(!Limits::fits(950, 950, 40, Win { small: 30, ..cut }), "the small threshold as set");
        // (Grown past the whole: more goes.)
        let grown = Win { window: 1200, small: STEP };
        assert!(Limits::fits(900, 900, 300, grown) && Limits::fits_own(900, 300, grown));
    }

    /// Cut under the whole, the pacing lets through only what fits the window, a small job within
    /// the whole, or one beside nothing: never what the whole window refuses, and none of the
    /// rules' ways past the sum (the lead's own alone, an aged job).
    #[test]
    fn a_cut_window_never_admits_what_the_whole_refuses() {
        let whole = Limits::default();
        let cut = Limits { window: 450, ..Default::default() };
        let ws = [0, 1, 49, 150, 450, 700, 960, 999, 1000];
        for &running in &ws {
            for &members in &ws {
                for &w in &ws {
                    for &heaviest in &ws {
                        for waited in [false, true] {
                            let job = |l: &Limits| {
                                Limits::fits_job(members, running, heaviest, w, waited, l.win()) && l.paced(running, w)
                            };
                            let own = |l: &Limits| {
                                Limits::fits_own_job(running, members, heaviest, w, waited, l.win())
                                    && l.paced(running, w)
                            };
                            assert!(!job(&cut) || job(&whole));
                            assert!(!own(&cut) || own(&whole));
                            if job(&cut) || own(&cut) {
                                assert!(
                                    w == 0 || running == 0 || running + w <= 450 || (w < STEP && running + w <= WHOLE),
                                    "{running} {w}"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(!cut.paced(450, 150) && cut.paced(300, 150) && cut.paced(0, 1000) && cut.paced(450, STEP - 1));
        assert!(!cut.paced(960, STEP - 1), "small ones held to the whole together");
        assert!(whole.paced(950, 150), "not cut: the rules decide");
    }

    /// A Mac's answers: `ms` over its floor `floor`, at home, fresh.
    fn ans(mac: &str, ms: f64, floor: f64) -> Answer {
        Answer { mac: mac.into(), home: true, ms, floor_ms: floor, missed: false, fresh: true }
    }

    /// A reading of `answers`, the disks unread, by the default tuning.
    fn read(p: &mut Pace, t: u64, answers: &[Answer], in_use: u32, wants: Option<u32>) -> Verdict {
        let s = Sample { t, answers, blind: false, busy: None, pct: DEFAULT_TARGET_PCT, in_use, wants };
        p.read(&s, &Tuning::default())
    }

    /// A reading of one Mac answering in `ms` against a floor of 5 ms.
    fn at(p: &mut Pace, t: u64, ms: f64, in_use: u32, wants: Option<u32>) -> Verdict {
        read(p, t, &[ans("m1", ms, 5.0)], in_use, wants)
    }

    /// Near the NAS's best the window doubles a minute while it's what limits, until its first
    /// cut, and not until the jobs last started have run a span; past the knee it drops to half
    /// the weight in use at once, and once more only once that has fallen to it and a span passed;
    /// between, it holds; then it doubles back to the slow start's threshold and grows a step a
    /// minute.
    #[test]
    fn the_window_grows_slowly_and_is_cut_quickly() {
        let mut p = Pace::default();
        assert_eq!(p.window(), STEP);
        // (Not what limits: nothing waits, and the weight in use is far under it.)
        assert!(matches!(at(&mut p, 0, 5.0, 0, None), Verdict::Held(_)));
        let mut t = 0;
        for _ in 0..6 {
            t += 10;
            assert_eq!(at(&mut p, t, 5.0, 0, Some(150)), Verdict::Grew);
        }
        assert_eq!(p.window(), 2 * STEP, "doubled in a minute");
        // (Jobs started: no growth until they've run a span, their answers in the reading.)
        for _ in 0..3 {
            t += 10;
            assert!(matches!(at(&mut p, t, 5.0, 700, Some(150)), Verdict::Held(_)));
        }
        for _ in 0..12 {
            t += 10;
            assert_eq!(at(&mut p, t, 5.0, 700, Some(150)), Verdict::Grew);
        }
        assert_eq!(p.window(), 4 * 2 * STEP, "slow start: doubling a minute");
        // (Between `NEAR` and `SLOW` of its floor: held.)
        t += 10;
        assert!(matches!(at(&mut p, t, 10.0, 700, Some(150)), Verdict::Held(_)));
        assert_eq!(p.window(), 400);
        // (Past the knee: half what's in use, at once; not again while the jobs out before it run.)
        t += 10;
        assert_eq!(at(&mut p, t, 16.0, 800, Some(150)), Verdict::Cut);
        assert_eq!(p.window(), 400, "half the 800 in use: no more than it was");
        assert_eq!(
            p.why().unwrap(),
            "NAS work paced to 40 % of the NAS, 80 % in use (the NAS answering m1 in 16 ms, 3.2× its best)"
        );
        t += 10;
        assert!(matches!(at(&mut p, t, 16.0, 700, Some(150)), Verdict::Held(_)), "the jobs out before the cut");
        // (Fallen to the window: cut again once its answers had a span to show it.)
        t += 10;
        assert!(matches!(at(&mut p, t, 16.0, 300, Some(150)), Verdict::Held(_)));
        t += 20;
        assert!(matches!(at(&mut p, t, 16.0, 300, Some(150)), Verdict::Held(_)));
        t += 10;
        assert_eq!(at(&mut p, t, 16.0, 300, Some(150)), Verdict::Cut);
        assert_eq!(p.window(), 150, "the NAS still slow: half what's left");
        // (Calm again: at the threshold, a step a minute, no more doubling.)
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
        // (Under its threshold, it doubles back to it, then adds a step a minute.)
        let mut q = Pace::default();
        at(&mut q, 0, 5.0, 0, None);
        assert_eq!(at(&mut q, 10, 40.0, 400, None), Verdict::Cut);
        assert_eq!((q.window(), q.st.threshold), (STEP, Some(200.0)));
        assert!(
            matches!(at(&mut q, 20, 5.0, 400, None), Verdict::Held(_)),
            "the jobs started at the cut not run a span yet"
        );
        for t in [40, 100, 160] {
            assert_eq!(at(&mut q, t, 5.0, 400, None), Verdict::Grew);
        }
        assert_eq!(q.window(), 200, "doubled back to half what was in use at the cut, no further");
        at(&mut q, 220, 5.0, 400, None);
        assert_eq!(q.window(), 250, "then a step a minute");
        // (A lead starting beside jobs already leased: from what they weigh.)
        let mut q = Pace::default();
        assert!(matches!(at(&mut q, 0, 5.0, 700, None), Verdict::Held(_)));
        assert_eq!(q.window(), 700);
    }

    /// The disks over the owner's target cut the window as a slow answer does; nothing read holds
    /// it.
    #[test]
    fn the_disks_over_the_target_cut_it_and_nothing_read_holds_it() {
        let mut p = Pace::default();
        let k = Tuning::default();
        let disks = |p: &mut Pace, t: u64, busy: Option<&[f64]>| {
            p.read(&Sample { t, answers: &[], blind: false, busy, pct: 85, in_use: 100, wants: Some(150) }, &k)
        };
        assert_eq!(disks(&mut p, 0, None), Verdict::Unread);
        assert!(
            matches!(disks(&mut p, 30, Some(&[40.0, 60.0])), Verdict::Held(_)),
            "within the target, but no answers: held"
        );
        assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NoAnswer));
        assert_eq!(p.load, Some(60.0));
        assert!(matches!(disks(&mut p, 40, Some(&[95.0])), Verdict::Held(_)), "smoothed: one busy reading isn't over");
        let cut = (5..10).map(|k| disks(&mut p, k * 10, Some(&[95.0]))).filter(|v| *v == Verdict::Cut).count();
        assert_eq!(cut, 1, "cut once: its jobs out before the cut still run");
        assert_eq!(p.window(), 50, "half the 100 in use");
        assert!(p.why().unwrap().contains("% busy, over the target 85 %"), "{:?}", p.why());
        let w = p.window();
        assert_eq!(disks(&mut p, 100, None), Verdict::Unread);
        assert_eq!(p.window(), w);
    }

    /// The second by excess over the least floor counts once two or more are read: one Mac's own
    /// link (Wi-Fi, a move to Tailscale) never paces the pool alone; a NAS that's loaded adds its
    /// delay to all of them. A Mac whose calls miss their deadline is past the knee, one at home
    /// whatever its rank; stale answers, or a Mac holding jobs giving none, never grow the window.
    #[test]
    fn the_second_slowest_mac_counts_and_stale_answers_never_grow_it() {
        let mut p = Pace::default();
        // (m6 four times its best on its own link, m1 at its best: m1's counts.)
        assert_eq!(read(&mut p, 0, &[ans("m1", 5.0, 5.0), ans("m6", 160.0, 40.0)], 0, Some(150)), Verdict::Grew);
        let c = p.last.as_ref().unwrap().counted.clone().unwrap();
        assert_eq!((c.mac.as_str(), c.ms, c.floor_ms), ("m1", 5.0, 5.0));
        // (Alone, a Mac's own answers count.)
        assert_eq!(read(&mut p, 40, &[ans("m6", 160.0, 40.0)], 0, Some(150)), Verdict::Cut);
        // (Three: the second-slowest's.)
        let mut q = Pace::default();
        let three = [ans("m1", 20.0, 5.0), ans("m4", 17.5, 5.0), ans("m6", 40.0, 40.0)];
        assert_eq!(read(&mut q, 0, &three, 300, None), Verdict::Cut);
        assert_eq!(q.last.as_ref().unwrap().counted.as_ref().unwrap().mac, "m4");
        // (R2: the NAS adding 16 ms to m1 at home (5 ms) and m6 away (40 ms): 4.2 times the least
        // floor's best on both, past the knee, where m6's own ratio would be 1.4.)
        let mut q = Pace::default();
        let away = |ms, floor| Answer { home: false, ..ans("m6", ms, floor) };
        assert_eq!(read(&mut q, 0, &[ans("m1", 21.0, 5.0), away(56.0, 40.0)], 300, None), Verdict::Cut);
        assert!((q.last.as_ref().unwrap().ratio.unwrap() - 4.2).abs() < 1e-9);
        // (m1 at home missing its deadlines, m6 away at its best: past the knee, m1 counted.)
        let mut q = Pace::default();
        let m1 = Answer { missed: true, ..ans("m1", 5.0, 5.0) };
        assert_eq!(read(&mut q, 0, &[m1, away(40.0, 40.0)], 300, None), Verdict::Cut);
        assert_eq!(q.last.as_ref().unwrap().counted.as_ref().unwrap().mac, "m1");
        // (Its calls missing their deadline: past the knee whatever its mean, said so.)
        let mut q = Pace::default();
        let missed = |m: &str| Answer { missed: true, ..ans(m, 5.0, 5.0) };
        assert_eq!(read(&mut q, 0, &[missed("m1"), missed("m6")], 600, None), Verdict::Cut);
        assert_eq!(
            q.why().unwrap(),
            "NAS work paced to 30 % of the NAS, 60 % in use (the NAS missing its deadlines from m1)"
        );
        // (A stale answer, or a Mac holding jobs and giving none: held, never grown.)
        let mut q = Pace::default();
        let stale = Answer { fresh: false, ..ans("m6", 40.0, 40.0) };
        assert!(matches!(read(&mut q, 0, &[ans("m1", 5.0, 5.0), stale.clone()], 0, Some(150)), Verdict::Held(_)));
        assert!(matches!(read(&mut q, 30, &[ans("m1", 5.0, 5.0), stale], 0, Some(150)), Verdict::Held(_)));
        let s = Sample {
            t: 40,
            answers: &[ans("m1", 5.0, 5.0)],
            blind: true,
            busy: None,
            pct: 85,
            in_use: 0,
            wants: Some(150),
        };
        assert!(matches!(q.read(&s, &Tuning::default()), Verdict::Held(_)));
        assert_eq!(q.last.as_ref().unwrap().verdict.hold(), Some(Hold::NotFresh));
        // (Stale, but past the knee: cut all the same.)
        let s = Sample { t: 50, answers: &[Answer { fresh: false, ..ans("m1", 50.0, 5.0) }], ..s };
        assert_eq!(q.read(&s, &Tuning::default()), Verdict::Cut);
    }

    /// A restarted lead takes its kept state up when it's younger than `restore_s`: the window,
    /// its threshold and its cut's hold-off, never lifted to the jobs out; older, it starts again.
    #[test]
    fn a_restart_takes_the_kept_state_up() {
        let mut p = Pace::default();
        at(&mut p, 0, 5.0, 800, None);
        at(&mut p, 10, 40.0, 800, None);
        assert_eq!(p.window(), 400);
        let kept: State = serde_json::from_value(serde_json::to_value(p.state(20)).unwrap()).unwrap();
        assert_eq!(
            kept,
            State {
                at: 20,
                window: 400.0,
                threshold: Some(400.0),
                cut_at: Some(10),
                settled_at: None,
                lone_at: Some(10),
                lone_n: 1
            }
        );
        let mut q = Pace::default();
        assert!(q.restore(&kept, 20 + RESTORE_S - 1, RESTORE_S));
        assert!(matches!(at(&mut q, 20 + RESTORE_S - 1, 40.0, 800, None), Verdict::Held(_)), "its hold-off kept");
        assert_eq!(q.window(), 400, "not lifted to the 800 out");
        let mut q = Pace::default();
        assert!(!q.restore(&kept, 20 + RESTORE_S, RESTORE_S));
        at(&mut q, 20 + RESTORE_S, 5.0, 800, None);
        assert_eq!(q.window(), 800, "too old: afresh");
    }

    /// R3: a reading past the knee holds a job beside nothing back for `lone_s` (an outside load
    /// near the cliff, one job taking it over), then lets one go; a reading near the best ends the
    /// hold at once; holds in a row double, to `lone_most_s`.
    #[test]
    fn a_reading_past_the_knee_holds_a_lone_job_back_a_while() {
        let mut p = Pace::default();
        assert!(p.lone(), "nothing read: open");
        at(&mut p, 0, 5.0, 0, Some(150));
        assert!(p.lone());
        at(&mut p, 10, 20.0, 0, Some(150));
        assert!(!p.lone(), "past the knee with nothing out");
        let held = Limits { window: p.window(), alone: p.lone(), ..Default::default() };
        assert!(!held.paced(0, 150), "not beside nothing");
        assert!(Limits { alone: true, ..held }.paced(0, 150));
        at(&mut p, 10 + LONE_S - 10, 20.0, 0, Some(150));
        assert!(!p.lone(), "still held");
        at(&mut p, 10 + LONE_S, 20.0, 0, Some(150));
        assert!(p.lone(), "a `lone_s` on: one goes");
        at(&mut p, 20 + LONE_S, 25.0, 150, None);
        assert!(!p.lone(), "the job gone beside nothing, past the knee: held again");
        at(&mut p, 30 + LONE_S, 10.0, 150, None);
        assert!(!p.lone(), "between near and the knee within its `lone_s`: still held");
        at(&mut p, 40 + LONE_S, 5.0, 0, Some(150));
        assert!(p.lone(), "near the best: the hold ended at once");
        // (An ordinary cut with nothing running after it: the NAS recovered, a heavy job beside
        // nothing goes at the next reading near its best, not `lone_s` on.)
        let mut p = Pace::default();
        at(&mut p, 0, 20.0, 400, None);
        at(&mut p, 10, 20.0, 0, Some(900));
        assert!(!p.lone());
        at(&mut p, 20, 5.0, 0, Some(900));
        assert!(p.lone(), "recovered: open");
        assert!(Limits { window: p.window(), alone: p.lone(), ..Default::default() }.paced(0, 900));
        // (Holds in a row, the NAS never near its best between: each twice as long, to 8 times.)
        let mut p = Pace::default();
        let mut t = 0;
        at(&mut p, t, 30.0, 0, Some(150));
        for hold in [1, 2, 4, 8, 8] {
            assert!(!p.lone());
            at(&mut p, t + hold * LONE_S - 10, 30.0, 0, Some(150));
            assert!(!p.lone(), "held {hold} times `lone_s`");
            at(&mut p, t + hold * LONE_S, 30.0, 0, Some(150));
            assert!(p.lone());
            t += hold * LONE_S + 10;
            at(&mut p, t, 30.0, 150, None);
        }
        // (The longest hold as set: 20 minutes, holds of 10, 20, 20.)
        let k = Tuning { lone_most_s: 2 * LONE_S, ..Tuning::default() };
        let read = |p: &mut Pace, t: u64, in_use: u32| {
            let answers = [ans("m1", 30.0, 5.0)];
            let s = Sample { t, answers: &answers, blind: false, busy: None, pct: 85, in_use, wants: Some(150) };
            p.read(&s, &k);
            p.lone()
        };
        let (mut p, mut t) = (Pace::default(), 0);
        read(&mut p, t, 0);
        for hold in [1, 2, 2] {
            assert!(!read(&mut p, t + hold * LONE_S - 10, 0), "held {hold} times `lone_s`");
            assert!(read(&mut p, t + hold * LONE_S, 0));
            t += hold * LONE_S + 10;
            read(&mut p, t, 150);
        }
        let k = Tuning::of(|n| {
            (n == "nas-lone-minutes").then_some(120.0).or((n == "nas-lone-most-minutes").then_some(10.0))
        });
        assert_eq!((k.lone_s, k.lone_most_s), (7200, 7200), "the longest hold never under one");
    }

    /// Each reading says why it held the window (the first gate it failed), and the lead's status
    /// line says the window, its threshold, what's in use and waits, the verdict, why, and the lone
    /// way's hold.
    #[test]
    fn a_reading_says_why_it_held() {
        let hold = |p: &Pace| p.last.as_ref().unwrap().verdict.hold();
        let mut p = Pace::default();
        assert_eq!(read(&mut p, 0, &[], 0, Some(150)), Verdict::Unread);
        assert_eq!(hold(&p), None);
        assert!(matches!(at(&mut p, 10, 5.0, 0, None), Verdict::Held(_)));
        assert_eq!(hold(&p), Some(Hold::NotLimiting), "nothing waits, nothing in use");
        assert_eq!(at(&mut p, 20, 5.0, 0, Some(150)), Verdict::Grew);
        assert_eq!(hold(&p), None);
        assert!(matches!(at(&mut p, 30, 5.0, 100, Some(150)), Verdict::Held(_)));
        assert_eq!(hold(&p), Some(Hold::Started), "a job just started");
        assert!(matches!(at(&mut p, 70, 10.0, 100, Some(150)), Verdict::Held(_)));
        assert_eq!(hold(&p), Some(Hold::NotNear));
        let stale = Answer { fresh: false, ..ans("m1", 5.0, 5.0) };
        assert!(matches!(read(&mut p, 80, &[stale], 100, Some(150)), Verdict::Held(_)));
        assert_eq!(hold(&p), Some(Hold::NotFresh));
        assert_eq!(at(&mut p, 90, 40.0, 400, Some(150)), Verdict::Cut);
        assert_eq!(hold(&p), None);
        assert!(matches!(at(&mut p, 100, 40.0, 400, Some(150)), Verdict::Held(_)));
        assert_eq!(hold(&p), Some(Hold::HoldOff), "the jobs out before the cut still run");
        let r = p.last.clone().unwrap();
        assert_eq!((r.threshold, r.wants), (Some(200), Some(150)));
        assert_eq!(r.lone_until, Some(90 + LONE_S), "past the knee: a lone job held back");
        assert_eq!(
            r.line(p.window(), 160),
            "NAS pacing: window 6 % (threshold 20 %), 40 % in use, the next job held back for it needs 15 %; last reading 60 s \
             ago, answers 8.0× their best: held: past the knee, within the last cut's hold-off; a job beside \
             nothing held back 9 min more"
        );
        assert!(p.why().unwrap().ends_with("(the NAS answering m1 in 40 ms, 8.0× its best)"), "{:?}", p.why());
        // (Between near and the knee after the cut: why says so in place of a cause it can't see.)
        at(&mut p, 110, 10.0, 400, Some(150));
        assert_eq!(
            p.why().unwrap(),
            "NAS work paced to 6 % of the NAS, 40 % in use (the NAS cut lately, not grown since: the answers \
             counted not near their best)"
        );
    }

    /// The window never grows on readings with no Mac's answers (the disks alone, nothing listing
    /// the NAS), nor more than one job past the load its readings tested: the weight in use and the
    /// next job waiting, or under the threshold twice the weight in use where that's more.
    #[test]
    fn the_window_grows_only_on_answers_and_never_past_the_load_tested() {
        let k = Tuning::default();
        let disks_only = |p: &mut Pace, t: u64, in_use: u32| {
            let s = Sample { t, answers: &[], blind: false, busy: Some(&[2.0]), pct: 85, in_use, wants: Some(300) };
            p.read(&s, &k)
        };
        // (Idle, the disks quiet, a 300 job waiting: held for want of answers.)
        let mut p = Pace::default();
        for t in (0..600).step_by(10) {
            assert!(matches!(disks_only(&mut p, t, 0), Verdict::Held(_)));
            assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NoAnswer));
        }
        assert_eq!(p.window(), STEP);
        // (Answers near the best, nothing in use: doubling to that one job, no further.)
        for t in (600..1200).step_by(10) {
            at(&mut p, t, 5.0, 0, Some(300));
            assert!(p.window() <= 300);
        }
        assert_eq!(p.window(), 300);
        assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NotLimiting));
        // (Cut at 400 in use: at the threshold, a step a minute to one job past the weight in use,
        // never past it.)
        at(&mut p, 1200, 5.0, 400, Some(300));
        assert_eq!(at(&mut p, 1210, 40.0, 400, Some(300)), Verdict::Cut);
        assert_eq!((p.window(), p.st.threshold), (200, Some(200.0)));
        for t in (1220..3000).step_by(10) {
            at(&mut p, t, 5.0, 400, Some(300));
            assert!(p.window() <= 700);
        }
        assert_eq!(p.window(), 700, "400 in use and the 300 waiting");
        assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NotLimiting));
    }

    /// The re-review's probe: cut at 300 in use (window and threshold 150), one 150 job running
    /// and another waiting, the NAS near its best for two hours. Past the threshold the window
    /// grows a step a minute to one job past the weight in use, so the second goes.
    #[test]
    fn past_a_cut_the_window_grows_back_to_take_one_more_job() {
        let mut p = Pace::default();
        at(&mut p, 0, 5.0, 300, None);
        assert_eq!(at(&mut p, 10, 40.0, 300, Some(150)), Verdict::Cut);
        assert_eq!((p.window(), p.st.threshold), (150, Some(150.0)));
        let mut t = 10;
        while t < 2 * 3600 {
            t += 10;
            at(&mut p, t, 5.0, 150, Some(150));
        }
        assert_eq!(p.window(), 300, "one 150 running and one more");
        let l = Limits { window: p.window(), alone: p.lone(), ..Default::default() };
        assert!(l.paced(150, 150), "the second 150 job goes");
    }

    /// The pacing's bounds are the pool's settings: each within its range, else its default;
    /// near never past the knee.
    #[test]
    fn the_bounds_are_settings() {
        assert_eq!(Tuning::of(|_| None), Tuning::default());
        let set = [
            ("nas-step", 20.0),
            ("nas-slow", 2.0),
            ("nas-near", 1.8),
            ("nas-floor-hours", 6.0),
            ("nas-read-seconds", 1.0),
        ];
        let k = Tuning::of(|n| set.iter().find(|s| s.0 == n).map(|s| s.1));
        assert_eq!((k.step, k.slow, k.near, k.floor_s, k.read_s), (20, 2.0, 1.8, 6 * 3600, READ_EVERY_S));
        let k = Tuning::of(|n| (n == "nas-slow").then_some(1.5).or((n == "nas-near").then_some(2.0)));
        assert_eq!((k.slow, k.near), (1.5, 1.5));
        for n in &KNOBS {
            let d = (n.get)(&Tuning::default());
            assert!((n.least..=n.most).contains(&d), "{}'s default {d} in its range", n.name);
        }
    }

    /// The simulated NAS: its answer time over its best as its true load grows (`rho`, a share of
    /// what it can take): flat at first, steep near the whole, and past it the cliff (on 4 Oct,
    /// processes on two Macs in uninterruptible waits): a hundred times, its listings past their
    /// deadline.
    fn simulated(rho: f64) -> f64 {
        if rho >= 1.0 {
            CLIFF
        } else {
            1.0 / (1.0 - rho.powi(3))
        }
    }

    /// The cliff's ratio in the simulation.
    const CLIFF: f64 = 100.0;

    /// A Mac of the simulation: its name, its best NAS answer (ms), whether it's at home, and its
    /// own link's slowing at `t` (1: none).
    struct SimMac {
        name: &'static str,
        best: f64,
        home: bool,
        link: fn(u64) -> f64,
    }

    /// The NAS's own answer at its best in the simulation (ms): its load adds this times its ratio
    /// less one to every Mac's answers, on top of each Mac's own link.
    const NAS_BEST_MS: f64 = 5.0;

    /// A simulated burst: `n` jobs asked at once, each weighed `w` but loading the NAS `real`
    /// (thousandths of what it can take: its weight a measure of other runs, a warm cache's or a
    /// quicker Mac's), each running `secs`, beside an outside load `outside(t)` (thousandths), on
    /// `macs` (jobs dealt in turn); each Mac's answers its own mean over its span of listings
    /// every 5 s (its best, plus the NAS's added delay, `NAS_BEST_MS` times its ratio less one, all
    /// by its own link and a 20 % noise of its own; `scaled`: its best times the NAS's ratio, the
    /// first version's model), past the deadline (20 s, its calls missed) at the cliff, against its
    /// own floor over `floor_s`, sent with each beat (every 10 s), or with `asks_only` only with its
    /// asks (every 120 s while it holds jobs: an older agent's); the lead restarted at each reading
    /// `restart` says (its state kept).
    struct Burst {
        n: usize,
        w: u32,
        real: u32,
        secs: u64,
        outside: fn(u64) -> u32,
        macs: Vec<SimMac>,
        asks_only: bool,
        scaled: bool,
        restart: fn(u64) -> bool,
        k: Tuning,
    }

    /// What a burst came to: the seconds past the knee and at the cliff, when its last job ended,
    /// how often the window was cut, and the most jobs running at once since its last cut.
    #[derive(Debug)]
    struct Run {
        past_knee_s: u64,
        cliff_s: u64,
        end: u64,
        cuts: usize,
        most_since_cut: usize,
    }

    fn near_and_away() -> Vec<SimMac> {
        vec![
            SimMac { name: "m1", best: 5.0, home: true, link: |_| 1.0 },
            SimMac { name: "m6", best: 40.0, home: false, link: |_| 1.0 },
        ]
    }

    impl Burst {
        fn of(n: usize, w: u32, real: u32, secs: u64) -> Burst {
            Burst {
                n,
                w,
                real,
                secs,
                outside: |_| 0,
                macs: near_and_away(),
                asks_only: false,
                scaled: false,
                restart: |_| false,
                k: Tuning::default(),
            }
        }

        /// Run paced (`pace`) or held to the whole.
        fn run(&self, pace: bool) -> Run {
            const TICK: u64 = 5;
            let k = &self.k;
            let mut p = Pace::default();
            let mut queue = self.n;
            // (Each job's end and Mac; each Mac's listings (t, ms, missed), its means (t, ms) and
            // the answer the lead has of it, and when it was sent.)
            let mut running: Vec<(u64, usize)> = Vec::new();
            let mut probes: Vec<std::collections::VecDeque<(u64, f64, bool)>> =
                vec![Default::default(); self.macs.len()];
            let mut means: Vec<std::collections::VecDeque<(u64, f64)>> = vec![Default::default(); self.macs.len()];
            let mut said: Vec<Option<(u64, Answer)>> = vec![None; self.macs.len()];
            let mut run = Run { past_knee_s: 0, cliff_s: 0, end: 0, cuts: 0, most_since_cut: 0 };
            let (mut t, mut next, mut seed) = (0u64, 0usize, 0x2545_f491_u64);
            loop {
                running.retain(|&(end, _)| end > t);
                if queue == 0 && running.is_empty() {
                    run.end = t;
                    return run;
                }
                assert!(t < 48 * 3600, "the burst never ended: {queue} left");
                let rho = (running.len() as u32 * self.real + (self.outside)(t)) as f64 / WHOLE as f64;
                let r = simulated(rho);
                if r > k.slow {
                    run.past_knee_s += TICK;
                }
                if r >= CLIFF {
                    run.cliff_s += TICK;
                }
                for (i, m) in self.macs.iter().enumerate() {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let noise = 0.8 + 0.4 * ((seed >> 33) as f64 / (1u64 << 31) as f64);
                    let missed = r >= CLIFF;
                    let ms = match (missed, self.scaled) {
                        (true, _) => 20_000.0,
                        (false, true) => m.best * (m.link)(t) * r * noise,
                        (false, false) => (m.best + NAS_BEST_MS * (r - 1.0)) * (m.link)(t) * noise,
                    };
                    probes[i].push_back((t, ms, missed));
                    while probes[i].front().is_some_and(|x| x.0 + k.span_s <= t) {
                        probes[i].pop_front();
                    }
                }
                if t % 10 == 0 {
                    for (i, m) in self.macs.iter().enumerate() {
                        let pr = &probes[i];
                        let mean = pr.iter().map(|x| x.1).sum::<f64>() / pr.len() as f64;
                        means[i].push_back((t, mean));
                        while means[i].front().is_some_and(|x| x.0 + k.floor_s <= t) {
                            means[i].pop_front();
                        }
                        let floor = means[i].iter().map(|x| x.1).fold(f64::INFINITY, f64::min).max(2.0);
                        let holds = running.iter().any(|j| j.1 == i);
                        let sends = !self.asks_only || !holds || said[i].as_ref().is_none_or(|s| s.0 + 120 <= t);
                        if sends {
                            let missed = pr.iter().any(|x| x.2);
                            let a = Answer {
                                mac: m.name.into(),
                                home: m.home,
                                ms: mean,
                                floor_ms: floor,
                                missed,
                                fresh: true,
                            };
                            said[i] = Some((t, a));
                        }
                    }
                }
                if t % k.read_s == 0 {
                    if (self.restart)(t) {
                        let kept = p.state(t);
                        p = Pace::default();
                        p.restore(&kept, t, k.restore_s);
                    }
                    let in_use = running.len() as u32 * self.w;
                    let win = if pace {
                        let fresh_s = k.span_s + 10;
                        let answers: Vec<Answer> = said
                            .iter()
                            .flatten()
                            .map(|(at, a)| Answer { fresh: at + fresh_s >= t, ..a.clone() })
                            .collect();
                        let blind = (0..self.macs.len()).any(|i| {
                            running.iter().any(|j| j.1 == i) && said[i].as_ref().is_none_or(|s| s.0 + fresh_s < t)
                        });
                        let s = Sample {
                            t,
                            answers: &answers,
                            blind,
                            busy: None,
                            pct: 85,
                            in_use,
                            wants: (queue > 0).then_some(self.w),
                        };
                        if p.read(&s, k) == Verdict::Cut {
                            run.cuts += 1;
                            run.most_since_cut = 0;
                        }
                        p.window()
                    } else {
                        WHOLE
                    };
                    let l = Limits { window: win, small: k.small, alone: !pace || p.lone(), ..Default::default() };
                    while queue > 0 {
                        let in_use = running.len() as u32 * self.w;
                        let heaviest = if running.is_empty() { 0 } else { self.w };
                        if !(Limits::fits_job(in_use, in_use, heaviest, self.w, false, l.win())
                            && l.paced(in_use, self.w))
                        {
                            break;
                        }
                        running.push((t + self.secs, next % self.macs.len()));
                        next += 1;
                        queue -= 1;
                    }
                    if run.cuts > 0 {
                        run.most_since_cut = run.most_since_cut.max(running.len());
                    }
                }
                t += TICK;
            }
        }
    }

    /// Done when (task #1417): a burst of NAS-heavy jobs settles below the latency knee on a
    /// simulated NAS. Held to the whole by weights that read the NAS light, the same burst drives
    /// it over the cliff and keeps it there; paced, it climbs to the knee and stays under it, every
    /// job done. Then one case for each way the review of 19 Nov drove it over (H1-H5): an outside
    /// load pushing it over the cliff (its listings past their deadline); Macs whose answers come
    /// only with their asks; hour-long jobs (the floor's drift); a restart every 15 minutes; and one
    /// Mac slow on its own link. Each case's time past the knee and at the cliff is printed.
    #[test]
    fn a_burst_of_nas_heavy_jobs_settles_below_the_knee() {
        let say = |case: &str, r: &Run| eprintln!("{case}: {r:?}");
        // (Weighed 150, loading the NAS 250: the whole lets six in, 1.5 times what it can take.)
        let b = Burst::of(30, 150, 250, 300);
        let unpaced = b.run(false);
        say("unpaced", &unpaced);
        assert!(unpaced.cliff_s * 2 > unpaced.end, "unpaced: at the cliff most of the burst");
        let paced = b.run(true);
        say("paced", &paced);
        assert_eq!(paced.cliff_s, 0, "paced: never at the cliff");
        assert!(paced.past_knee_s <= 120, "paced: past the knee two minutes at most");
        assert!(paced.end <= 30 * 300 / 2 + 900, "not starved: two or three at once");
        // (Weighed 150, loading 400: two at once at most.)
        let heavy = Burst::of(20, 150, 400, 300).run(true);
        say("heavy", &heavy);
        assert_eq!(heavy.cliff_s, 0);
        // (H1: an outside load of 70 % from 30 to 60 minutes in, over the cliff with the jobs out:
        // its listings past their deadline cut the window at once; at the cliff no longer than the
        // jobs out then run.)
        let b = Burst { outside: |t| if (1800..3600).contains(&t) { 700 } else { 0 }, ..Burst::of(40, 150, 250, 300) };
        let r = b.run(true);
        say("H1 timeouts at the cliff", &r);
        assert!(r.cliff_s <= 300, "at the cliff only while the jobs out then run");
        assert!(r.cuts >= 1);
        // (Once the outside load is gone the window grows back past its cut, a job at a time: the
        // pool gets back to three heavy jobs at once, as many as the NAS takes under the knee.)
        assert!(r.most_since_cut >= 3, "back to several jobs after the cut: {r:?}");
        // (H2: answers only with each Mac's asks, every 120 s while it holds jobs: no growth on
        // them, never at the cliff.)
        let b = Burst { asks_only: true, ..Burst::of(30, 150, 250, 300) };
        let r = b.run(true);
        say("H2 stale answers from full Macs", &r);
        assert_eq!(r.cliff_s, 0);
        assert!(r.past_knee_s <= 120);
        // (H3: hour-long jobs: the floor, the day's least, doesn't drift up under them.)
        let b = Burst::of(12, 150, 250, 3600);
        let r = b.run(true);
        say("H3 hour-long jobs", &r);
        assert_eq!(r.cliff_s, 0);
        assert!(r.past_knee_s <= 120);
        let drift =
            Burst { k: Tuning { floor_s: 30 * 60, ..Tuning::default() }, ..Burst::of(12, 150, 250, 3600) }.run(true);
        say("H3 with a half-hour floor (before)", &drift);
        // (H4: a restart every 15 minutes through an outside load of half the NAS from 30 to 60
        // minutes in, its state taken up: its cuts stand.)
        let outside: fn(u64) -> u32 = |t| if (1800..3600).contains(&t) { 500 } else { 0 };
        let b = Burst { restart: |t| t > 0 && t % 900 == 0, outside, ..Burst::of(40, 150, 250, 300) };
        let r = b.run(true);
        say("H4 restarts mid-burst", &r);
        let unrestarted = Burst { outside, ..Burst::of(40, 150, 250, 300) }.run(true);
        say("H4 the same, never restarted", &unrestarted);
        assert!(r.cliff_s <= unrestarted.cliff_s + 60 && r.past_knee_s <= unrestarted.past_knee_s + 120);
        let k = Tuning { restore_s: 0, ..Tuning::default() };
        let afresh = Burst { restart: |t| t > 0 && t % 900 == 0, outside, k, ..Burst::of(40, 150, 250, 300) }.run(true);
        say("H4 restarts, nothing kept (before)", &afresh);
        // (H4 in the hold-off: H1's outside load, the lead restarted twice in the minutes after its
        // cut, the weight in use still over the window: kept, the hold-off and the cut stand; taken
        // up afresh, the window is lifted to the weight in use and cut again.)
        let h1: fn(u64) -> u32 = |t| if (1800..3600).contains(&t) { 700 } else { 0 };
        let during: fn(u64) -> bool = |t| t == 1840 || t == 1900;
        let kept = Burst { restart: during, outside: h1, ..Burst::of(40, 150, 250, 300) }.run(true);
        say("H4 restarts in the hold-off, kept", &kept);
        let lost = Burst { restart: during, outside: h1, k, ..Burst::of(40, 150, 250, 300) }.run(true);
        say("H4 restarts in the hold-off, nothing kept (before)", &lost);
        let still = Burst { outside: h1, ..Burst::of(40, 150, 250, 300) }.run(true);
        assert!(kept.cliff_s <= still.cliff_s && kept.past_knee_s <= still.past_knee_s + 60);
        assert!(kept.cuts <= lost.cuts);
        // (H5: m6's own link four times slower from 20 to 50 minutes in: the pool not paced by it.)
        let mut macs = near_and_away();
        macs[1].link = |t| if (1200..3000).contains(&t) { 4.0 } else { 1.0 };
        let b = Burst { macs, ..Burst::of(30, 150, 250, 300) };
        let r = b.run(true);
        say("H5 one Mac slow on its own link", &r);
        assert_eq!(r.cliff_s, 0);
        assert!(r.end <= paced.end + 600, "not throttled by one Mac's link: {} s against {} s", r.end, paced.end);
        // (R2: the NAS's load adds its delay to each Mac's answers, not times each one's floor: m1
        // at home at 5 ms and m6 away at 40 ms, an outside load rising to 70 % from 30 to 60
        // minutes in, held to 80 minutes; ranked by excess over the least floor, the slowdown
        // counts as it is, not an eighth of it, and the window is cut before the cliff. The
        // burst's own `paced` run is the same model without it.)
        let ramp: fn(u64) -> u32 = |t| match t {
            1800..3600 => ((t - 1800) * 700 / 1800) as u32,
            3600..4800 => 700,
            _ => 0,
        };
        let r = Burst { outside: ramp, ..Burst::of(40, 150, 250, 300) }.run(true);
        say("R2 added delay, one home and one away", &r);
        assert_eq!(r.cliff_s, 0);
        assert!(r.cuts >= 1);
        let scaled = Burst { scaled: true, ..Burst::of(30, 150, 250, 300) }.run(true);
        say("R2 floors scaled (the first model)", &scaled);
        assert_eq!(scaled.cliff_s, 0);
        // (R3: an outside load of 80 % from 30 to 90 minutes in, so a job beside nothing (its
        // 25 %) takes the NAS over the cliff: a reading past the knee holds the lone way for
        // `lone_s`, then one job goes, the next hold twice as long; at the cliff only with the jobs
        // out as it came and those lone ones, the burst done after.)
        let r3: fn(u64) -> u32 = |t| if (1800..5400).contains(&t) { 800 } else { 0 };
        let r = Burst { outside: r3, ..Burst::of(40, 150, 250, 300) }.run(true);
        say("R3 outside load 800", &r);
        let lone = Burst { outside: r3, k: Tuning { lone_s: 0, ..Tuning::default() }, ..Burst::of(40, 150, 250, 300) }
            .run(true);
        say("R3 outside load 800, lone jobs unheld (before)", &lone);
        // (Held 1, 2 and 4 times `lone_s` through its hour: three lone jobs, and those out as it came.)
        assert!(r.cliff_s <= 4 * 300 + 60, "at the cliff only as the lone jobs go");
        assert!(r.cliff_s * 4 <= lone.cliff_s);
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
