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
//!   whatever loads the NAS, the build or not, and grows the window slowly while work waits and
//!   the NAS has room (the disks within the target with the next job, the answers not rising with
//!   what the pool's jobs move on the NAS as measured), or cuts it to half what's in use once it's
//!   congested (the disks over the target, the answers past the knee and rising with that use, or
//!   missing their deadlines). A job weighing
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
/// processes on two Macs in uninterruptible waits): work is added slowly while it has room, and
/// cut quickly once its answers slow with the load, before the cliff. Every `Tuning::read_s` the lead
/// reads (`Sample`):
///
/// - each Mac's NAS answer time over its **floor** (`Answer`: both the Mac's own, from its live
///   store, carried by each beat), over the last `Tuning::span_s` while a build runs: its listing
///   of the project's folder every 5 s on its jobs' SMB session, or, with `Tuning::apart_judges`
///   on, its probe's while it answers, a 4 KB read past the NAS's cache on its own ssh every
///   `Tuning::apart_s` (agentcore::agent::apart: `Answer::apart_s`; off, in shadow, the probe's
///   figures beside the listings, `Answer::probe`); a probe or listing past its deadline, or
///   while the last is still stuck, counts as the deadline or the stuck call's age. A NAS's load adds
///   milliseconds to every Mac's answers; it doesn't multiply each floor: each Mac is ranked by
///   its **excess** (its answer time less its floor) over the least floor among those answering
///   the same way (listings or probes),
///   and its ratio is one more than that (alone, its own answer over its floor). With two Macs or
///   more read, the second-highest counts: one Mac's own link (the M1's Wi-Fi, a Mac away on
///   Tailscale) never paces the pool alone, while a NAS slowdown, the same milliseconds on a Mac
///   at home and one away, is read at the home Mac's scale. A Mac whose NAS calls miss their
///   deadline ranks past every knee, and one at home missing them is past the knee whatever its
///   rank;
/// - the NAS's busiest disk's busy share (SNMP, smoothed by `SMOOTH`) against the owner's target
///   (`Target`, `scenic nas-target`, by default `DEFAULT_TARGET_PCT`).
///
/// The answers over the floor say how slow the NAS is, not why: on 19 and 20 Nov (task #1506's
/// replay of the lead's readings, `nas_readings.csv`) the counted answers were past the knee in
/// 17 % of the readings with the pool's jobs weighing under 25 thousandths and 25 to 38 % at any
/// weight of 50 and more; by what the jobs moved on the NAS as measured, in 14 % under 25
/// thousandths and 54 % over 400. A weight is a job's heaviest target's rate, held for its
/// whole run; what its phases move comes and goes. So the answers are judged, as TCP Vegas judges
/// its round trips, against **the pool's NAS use as measured** (`Sample::moving`: each Mac's jobs'
/// traffic over the answers' own span, weighed as a job is, `Limits::weight`), the counted Mac's
/// own apart where its answers are its listings on the jobs' session: its own jobs' traffic
/// stalls those whatever the NAS's load (on the mini, 20 Nov, in the seconds its session moved 5
/// MB/s or more, a median of 73 ms, 1.7 ms in quiet ones), so they're judged against the others'
/// use (`Seen::used`); its answers taken apart, judged with all the pool moves, its own too
/// (`Seen::apart`). The ratios, each with the
/// uses it was read beside, are kept for `Tuning::track_s`; those of the last `Tuning::grow_s`
/// (the cadence) are judged against the rest at a step or more lighter and a step or more heavier
/// than any of the cadence's. While the counted Mac's own jobs move a step or more in the cadence
/// (`Seen::own`), its answers on the jobs' session can't tell the NAS from its link:
/// **undecidable**, neither rising nor
/// not the pool's: never halved on, never grown past the knee, and held while they rise against
/// those a step lighter in all the pool moves, its own too; risen past `Tuning::slow`, the window
/// goes back to the knee (`Judged::knee`: the heaviest weight in use read before the cadence with
/// answers under it), never under it, or halved where there's none. Otherwise they are **rising** while their lower quartile
/// is past the upper quartile of those a step lighter (the quicker quarter now slower than all but
/// the slowest quarter then: the day's noise, a stall now and then whatever the load, passes under
/// it); and **not the pool's** (never while the counted Mac's own jobs move a step or more, its
/// answers apart or not) while all the pool's jobs together move under a step, or the
/// cadence's median is within the upper quartile of those a step lighter, or those a step heavier
/// were no slower (their lower quartile within the cadence's upper). Between the two (the lower
/// quartile past the lighter's, the median not: a cadence is some three spans, so about three
/// readings each its own) neither. Once the cadence's readings span a `grow_s` it is decided:
/// congested if they're rising and their lower quartile is past `Tuning::slow`. A use held within
/// a step for a `track_s` leaves nothing to compare: neither.
///
/// Then, as the NAS's own readers decide their reads in flight (store::aimd):
///
/// - **congested** (the disks over the target, smoothed, or this reading's busiest at it or over;
///   the counted Mac missing its deadlines; or its
///   answers judged rising past `Tuning::slow`): the window drops to `Tuning::decrease` of the
///   weight in use at once, never under `Tuning::step`; the slow start's threshold is set there.
///   The jobs running go on, and nothing more that's paced starts until enough of them end. No new
///   cut until the weight in use has fallen to the window and its answers had a `span_s` since to
///   show it (store::aimd's once a window: a cut compounding on readings of the jobs out before it
///   takes the pool to nothing);
/// - **spare** (a Mac's answers read and not rising; under `Tuning::slow`, the knee (this reading's,
///   and the last `span_s`'s lower quartile), or past it, judged not the pool's, the disks read;
///   and the disks with room for the next job: their busy share smoothed and the room it needs, a
///   share of the NAS as probed, within the target; the disks not read, the answers alone say): it
///   grows,
///   only while it's what limits (the weight in use and the least a job waiting for the budget weighs past it;
///   none waiting, the weight in use within a step of it: a window no job asks for isn't credit
///   for a later burst, nor is room left by a job just ended), only on readings every Mac holding
///   leased jobs gave fresh (none older than its span and a beat: no growth on stale data), and
///   only once the jobs last started have run a `span_s` (their answers in the reading), and,
///   where they rose a step or more to the knee or past it (`Judged::knee`: the heaviest weight
///   in use with answers under `Tuning::slow` lately) or no knee is known, a `grow_s` more (a
///   growth cadence of their answers read): fast where the answers were good at that weight,
///   cautious beyond it:
///   doubling a `Tuning::grow_s` up to the slow start's threshold (none before the first cut),
///   then a step a `grow_s`; and never more than one job past the load its readings tested: the
///   weight in use and the next job waiting (a step at least), or under the threshold twice the
///   weight in use (at least two steps) where that's more. A window grown on readings of a light
///   load would admit several heavy jobs in one jump; one job's is what the next congested
///   reading cuts back;
/// - **neither**, or nothing read: it holds.
///
/// A reading that held says why (`Verdict::Held`'s `Hold`: the first of those gates it failed),
/// for the lead's status (`Reading::line`): a held reading without its reason can't be made.
///
/// A job beside nothing that the window doesn't take goes all the same, but not for
/// `Tuning::lone_s` from a congested reading (`Pace::lone`: beside an outside load near the cliff,
/// each lone job would take the NAS over it, and the hold is all that keeps it off), each hold in
/// a row twice as long, to `Tuning::lone_most_s`; a spare reading ends it. Then one goes, so
/// nothing waits for good.
///
/// It starts at a step, or at what the jobs already leased weigh where that's more (a lead's
/// coordinator starting knows nothing of the NAS's load but what runs on it), unless the lead kept
/// its state (`State`) younger than `Tuning::restore_s`: a
/// restart then takes the window, its threshold and its cut up again (task #603's pattern: a
/// publish mid-burst must not undo a cut). It is never more than the whole (`WHOLE`): the
/// weights are estimates, so the answers may say the NAS is full before them, but past the whole
/// they'd admit more than the probe measured it could take.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pace {
    st: State,
    /// Whether the state was taken up from a kept one (`restore`): the window isn't lifted to the
    /// weight in use at the first reading.
    restored: bool,
    /// The weight in use at the reading before, when it last rose, and what it rose from at the
    /// first rise of those since a span and a growth cadence (M2: no growth until the jobs last
    /// started have run a span, and a growth cadence more where they rose a step or more to the
    /// knee or past it).
    prev_in_use: u32,
    rose_at: Option<u64>,
    rose_from: u32,
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
/// hold-off), since when a job beside nothing is held back (`Pace::lone`), and the readings the
/// answers are judged on (`Seen`, oldest first, the last `Tuning::track_s`'s), all as of `at` (unix
/// s).
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub seen: Vec<Seen>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Default for State {
    fn default() -> Self {
        State {
            at: 0,
            window: STEP as f64,
            threshold: None,
            cut_at: None,
            settled_at: None,
            lone_at: None,
            lone_n: 0,
            seen: Vec::new(),
        }
    }
}

/// A reading with a Mac's answers (`Pace::read`): when (unix s), the pool's NAS use as measured
/// but the counted Mac's own (all of it where its answers were taken apart: `apart`), and the
/// counted Mac's own (`Sample::moving`, thousandths; None: none measured), the weight in use
/// (`Sample::in_use`), and the counted ratio (each over the least floor among those answering, so
/// Macs' compare, and the counted one changes with the noise).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Seen {
    pub t: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own: Option<u32>,
    #[serde(default)]
    pub held: u32,
    pub ratio: f64,
    /// The counted Mac's answers taken apart from its jobs' session (`Answer::apart_s`): `used`
    /// then holds its own jobs' traffic too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub apart: bool,
}

/// The cadence's answers judged against the pool's measured use (`Pace::read`, `judge`): the upper
/// quartile of those read a step lighter (None: none), whether they're rising past it, whether
/// they're judged not the pool's, whether the counted Mac's own jobs moved a step or more
/// (undecidable) and, so, whether they rise with all the pool moves (`swelling`), the heaviest
/// weight in use read before the cadence with answers under the knee (`knee`), the cadence's
/// lower quartile, and whether its readings span a growth cadence (decided).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Judged {
    lighter: Option<f64>,
    rising: bool,
    outside: bool,
    undecided: bool,
    swelling: bool,
    knee: Option<u32>,
    quick: Option<f64>,
    done: bool,
}

/// `seen` (oldest first) judged at `t` by `k` (`Pace`'s doc): the last `Tuning::grow_s`'s readings
/// against the rest at a step or more lighter and heavier than any of them; readings with no use
/// measured never judged.
fn judge(seen: &[Seen], t: u64, k: &Tuning) -> Judged {
    // (Judged against readings of the latest one's source alone, probe or listing: a listing's
    // history, its stalls among it, says nothing of a probe's cadence.)
    let src = seen.last().is_some_and(|x| x.apart);
    let same: Vec<Seen> = seen.iter().filter(|x| x.apart == src).copied().collect();
    let seen = &same[..];
    // (Answers taken apart: the counted Mac's own traffic is in `used` already, never undecidable.)
    let measured = || {
        seen.iter().filter_map(|x| Some((x.t, x.used?, if x.apart { 0 } else { x.own.unwrap_or(0) }, x.ratio, x.held)))
    };
    let now: Vec<(u64, u32, u32, f64, u32)> = measured().filter(|x| x.0 + k.grow_s > t).collect();
    let (Some(lo), Some(hi)) = (now.iter().map(|x| x.1).min(), now.iter().map(|x| x.1).max()) else {
        return Judged::default();
    };
    let all = now.iter().map(|x| x.1.saturating_add(x.2)).max().unwrap_or(0);
    let undecided = now.iter().any(|x| x.2 >= k.step);
    // (The counted Mac's own jobs moving a step or more, its answers apart or not: slow answers
    // then are never judged not the pool's.)
    let mover = seen.iter().any(|x| x.t + k.grow_s > t && x.used.is_some() && x.own.unwrap_or(0) >= k.step);
    // (Undecidable, the answers are judged against those a step lighter in all the pool moves, the
    // counted Mac's own too: rising with it, nothing grows. The knee: the heaviest weight in use
    // read before the cadence with answers under `Tuning::slow`.)
    let total = |x: &(u64, u32, u32, f64, u32)| x.1.saturating_add(x.2);
    let least = now.iter().map(total).min().unwrap_or(0);
    let lighter_all = || measured().filter(move |x| total(x).saturating_add(k.step) <= least);
    let knee = measured().filter(|x| x.3 <= k.slow && x.0 + k.grow_s <= t).map(|x| x.4).max();
    let lighter_all = quartile(lighter_all().map(|x| x.3), 3);
    let q = |n| quartile(now.iter().map(|x| x.3), n);
    let (quick, middle, slowest) = (q(1), q(2), q(3));
    let lighter = quartile(measured().filter(|x| x.1.saturating_add(k.step) <= lo).map(|x| x.3), 3);
    let heavier = quartile(measured().filter(|x| x.1 >= hi.saturating_add(k.step)).map(|x| x.3), 1);
    let past = |a: Option<f64>, b: Option<f64>| matches!((a, b), (Some(a), Some(b)) if a > b);
    let within = |a: Option<f64>, b: Option<f64>| matches!((a, b), (Some(a), Some(b)) if a <= b);
    let rising = !undecided && past(quick, lighter);
    let swelling = undecided && past(quick, lighter_all);
    let outside = !mover && !rising && (all < k.step || within(middle, lighter) || within(heavier, slowest));
    // (Decided once the readings span a growth cadence, each counting its `read_s`: a reading
    // missed meanwhile doesn't put it off.)
    let done = t + k.read_s.max(1) >= now[0].0 + k.grow_s;
    Judged { lighter, rising, outside, undecided, swelling, knee, quick, done }
}

/// The `q`-th quartile of `v` (the ⌈qn/4⌉-th least: 1 the lower, 3 the upper); None when it's
/// empty.
fn quartile(v: impl IntoIterator<Item = f64>, q: usize) -> Option<f64> {
    let mut v: Vec<f64> = v.into_iter().filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    (!v.is_empty()).then(|| v[(q * v.len()).div_ceil(4).max(1) - 1])
}

/// One reading of the pacing (`Pace::read`): when (unix s), the Mac whose answers counted and its
/// ratio (one more than its excess over the least floor, at most `RATIO_MOST`), the upper quartile
/// of the ratios read a step lighter in measured use (None: none), whether they're rising past it,
/// whether they're judged not the pool's (`outside`) or undecidable (`undecided`: the counted Mac's
/// own jobs moving a step or more; rising then in all the pool moves), whether it was congested
/// (`past`), the disks' smoothed load and this reading's busiest disk,
/// the weight in use, the pool's NAS use as measured but the counted Mac's (`Seen::used`) and the
/// counted Mac's own (`Seen::own`), the room the next job waiting needs (`Sample::wants`), what it
/// did
/// (and why, where it held), the slow start's threshold after it, until when a job beside
/// nothing is held back (`Pace::lone`; None: it isn't), and the counted Mac's probe apart's ratio,
/// judged or in shadow (`Answer::probe`, over the least probe floor; None: no probe answer).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Reading {
    pub at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counted: Option<Answer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rising: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub undecided: bool,
    pub past: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_pct: Option<f64>,
    pub in_use: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wants: Option<u32>,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lone_until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_ratio: Option<f64>,
}

/// Why a reading held the window (`Verdict::Held`): the first of `Pace::read`'s gates it failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hold {
    /// Congested within the last cut's hold-off: the weight in use not yet down to the window, or
    /// no `Tuning::span_s` since it fell there.
    HoldOff,
    /// No Mac's answers read (none listing the NAS), the disks alone within the target.
    NoAnswer,
    /// The answers rising with the pool's measured use: the cadence's lower quartile past the upper
    /// quartile of those a step lighter (undecidable: in all the pool moves, the counted Mac's own
    /// too).
    Rising,
    /// This reading's answers past the knee, or the last span's (their lower quartile), and not
    /// judged the pool's or not (undecidable among them).
    Slow,
    /// The disks' busy share (smoothed) and the room the next job needs past the target.
    Busy,
    /// A Mac holding leased jobs gave no fresh answers, or an answer read isn't fresh.
    NotFresh,
    /// The jobs last started haven't run a `Tuning::span_s` (their answers aren't in the reading),
    /// or, where they rose a step or more to the knee or past it (or no knee is known), a span and
    /// a `Tuning::grow_s` (a growth cadence of their answers not yet read).
    Started,
    /// The window isn't what limits: the room the next job waiting needs fits beside what's in use
    /// (none waiting: the weight in use more than a step under it).
    NotLimiting,
}

impl Hold {
    /// What it says, for the lead's status.
    pub fn words(self) -> &'static str {
        match self {
            Hold::HoldOff => "congested, within the last cut's hold-off",
            Hold::NoAnswer => "no Mac's answers read",
            Hold::Rising => "the answers rising with the pool's use",
            Hold::Slow => "the answers past the knee, not yet judged the pool's or not",
            Hold::Busy => "the disks too busy for the next job",
            Hold::NotFresh => "not every Mac's answers fresh",
            Hold::Started => "the jobs last started not yet judged in the answers",
            Hold::NotLimiting => "the window not what limits",
        }
    }
}

impl Reading {
    /// The pacing's line in the lead's status at `now` (unix s), its window `window`: the window,
    /// its threshold, the weight in use, the use measured (all the pool's jobs', and the counted
    /// Mac's share) and the room the next job waiting needs (`Sample::wants`), the reading's ratio
    /// (the counted Mac's, on its own ssh to the NAS and how often, or on its jobs' SMB session:
    /// `Answer::apart_s`; against
    /// those a step lighter, judged not the pool's, or undecidable) and disk load, its
    /// verdict and why it held, the lone way's hold, and the counted Mac's probe apart (judged or
    /// in shadow, `Answer::probe`, `Reading::probe_ratio`).
    pub fn line(&self, window: u32, now: u64) -> String {
        let th = self.threshold.map_or(String::new(), |t| format!(" (threshold {})", pct(t)));
        let mac = self.counted.as_ref().map_or("", |c| c.mac.as_str());
        let own = self.own.unwrap_or(0);
        let used = self.used.map_or(String::new(), |u| match own {
            0 => format!(", {} moving", pct(u.saturating_add(own))),
            _ => format!(", {} moving ({} of it {mac}'s)", pct(u.saturating_add(own)), pct(own)),
        });
        let wants = self.wants.map_or(", none held back for it".to_string(), |w| {
            format!(", the next job held back for it needs {}", pct(w))
        });
        let below = match (self.below, self.outside, self.undecided) {
            (_, _, true) => format!(" (undecidable: the answering Mac, {mac}, moves {})", pct(own)),
            (_, true, _) => " (not the pool's doing)".to_string(),
            (Some(b), ..) => format!(" ({b:.1}× a step lighter)"),
            (None, ..) => String::new(),
        };
        let via = self.counted.as_ref().map_or(String::new(), |c| match c.apart_s {
            0 => format!(" on {mac}'s jobs' SMB session"),
            s => format!(" on {mac}'s own ssh to the NAS (every {s} s)"),
        });
        let ratio = self.ratio.map_or(String::new(), |x| format!(", answers{via} {x:.1}× their best{below}"));
        // (The counted Mac's probe apart, judged or in shadow: its ratio and floor, its cadence,
        // connections and answers past their deadline, or why it's down.)
        let probe =
            self.counted.as_ref().and_then(|c| c.probe.as_ref().map(|p| (c, p))).map_or(String::new(), |(c, p)| {
                let role = if c.apart_s > 0 { "judged" } else { "in shadow" };
                let said = match (self.probe_ratio, p.floor_ms, &p.down) {
                    (_, _, Some(why)) => format!("down: {why}"),
                    (Some(x), Some(f), None) => format!("{x:.1}× its best of {f:.1} ms"),
                    _ => "no answer yet".to_string(),
                };
                format!(
                    "; probe apart ({role}, every {} s, {} connections, {} past their deadline): {said}",
                    p.every_s, p.connects, p.timeouts
                )
            });
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
            "NAS pacing: window {}{th}, {} in use{used}{wants}; last reading {} s ago{ratio}{load}: {verdict}{lone}{probe}",
            pct(window),
            pct(self.in_use),
            now.saturating_sub(self.at)
        )
    }
}

/// A Mac's NAS answers as the lead reads them: its NAS answer time (ms, `answer_of` its span)
/// and its floor (ms, its own: the least such mean over `Tuning::floor_s` on its link), whether it
/// is at home (the floor's link: a Mac moved to Tailscale is another), whether its NAS calls
/// missed their deadline lately, whether the reading is fresh (no older than its span and a
/// beat, its floor not new on a new link: it may cut, never grow), whether it was taken apart
/// from its jobs' SMB session (`apart_s`: the cadence of the Mac's probe on an ssh of its own to
/// the NAS, its floor that probe's, s, when `Tuning::apart_judges`; 0: its listings on the jobs'
/// session, which its own jobs' traffic stalls), and its probe apart as it stands, judged or in
/// shadow (`probe`; None: off, or a Mac before task #1552).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Answer {
    pub mac: String,
    pub home: bool,
    pub ms: f64,
    pub floor_ms: f64,
    pub missed: bool,
    pub fresh: bool,
    #[serde(skip_serializing_if = "is_zero64")]
    pub apart_s: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<Apart>,
}

fn is_zero64(n: &u64) -> bool {
    *n == 0
}

/// A Mac's probe apart as it stands (agentcore::agent::apart, task #1552): its answer time and
/// floor over the span as its listings' are read (ms; None: no answer in the span), its cadence
/// (s), the connections it opened and the answers past their deadline since its agent started,
/// and why it is down (None: answering).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Apart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor_ms: Option<f64>,
    pub every_s: u64,
    pub connects: u64,
    pub timeouts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub down: Option<String>,
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
    /// Spare, and the window what limits: grown.
    Grew,
    /// Congested: cut.
    Cut,
    /// Held, and why (`Hold`: the first gate it failed).
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
/// (%; None: not read) against the target `pct`, the weight the pool's leased jobs hold, what the
/// pool's jobs move on the NAS as measured (`moving`: thousandths of the NAS as probed, each Mac's
/// `Limits::weight` of its jobs' traffic over the answers' span, summed; None: none measured), and
/// the room the next job waiting for the budget needs (agentcore's
/// `Sched::nas_wants`: the least one waiting weighs, or the room held for an aged one; None: none
/// waits; the window is what limits once it refuses that beside what's in use, or, none waiting, a
/// step).
#[derive(Clone, Copy, Debug)]
pub struct Sample<'a> {
    pub t: u64,
    pub answers: &'a [Answer],
    pub blind: bool,
    pub busy: Option<&'a [f64]>,
    pub pct: u32,
    pub in_use: u32,
    pub moving: &'a [(String, u32)],
    pub wants: Option<u32>,
}

/// The pacing's bounds (`Pace`), each the pool's setting (`KNOBS`: agentcore::coord::asks, `scenic
/// pool set <name>`, the build page), read by the lead at each reading and by each Mac for its
/// answers (`span_s`, `floor_s`); not set, its default (`Tuning::default`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tuning {
    /// The window's additive step a `grow_s`, and its least (thousandths of the NAS); the answers
    /// are judged against those at least this much lighter or heavier in measured use, and a use
    /// under it isn't the pool's doing.
    pub step: u32,
    /// A job weighing less (thousandths) isn't paced: held to the whole, not the window.
    pub small: u32,
    /// The window once congested: this share of the weight in use.
    pub decrease: f64,
    /// The span a Mac's floor is its least answer over (s): long enough that a busy stretch, an
    /// hour-long job's, never becomes the floor; a link change starts it again.
    pub floor_s: u64,
    /// Past this many times its floor, a Mac's answers are past the knee: no growth while a span's
    /// are (their lower quartile) unless they're judged not the pool's, congested where they rise
    /// with its use.
    pub slow: f64,
    /// The growth's cadence (s): the window doubles, or grows a step, in this long; and the answers
    /// of this long are judged.
    pub grow_s: u64,
    /// How long the readings are kept to judge the answers against (s): those at a step lighter
    /// and heavier in measured use.
    pub track_s: u64,
    /// How often the lead reads (s).
    pub read_s: u64,
    /// The span a Mac's NAS answer time is read over (s, `answer_of`), and its jobs' traffic: its
    /// listings every 5 s; the knee read over the last; the jobs last started show in the answers
    /// this long after.
    pub span_s: u64,
    /// A listing quicker than this (ms) was answered by the Mac's SMB cache, not the NAS
    /// (`answer_of`); 0: every listing counts.
    pub cached_ms: f64,
    /// The longest a kept state is taken up by a restarted lead (s).
    pub restore_s: u64,
    /// How long a congested reading holds a lone job back (s; `Pace::lone`), and the longest
    /// its doubling makes a hold in a row (s; never under `lone_s`).
    pub lone_s: u64,
    pub lone_most_s: u64,
    /// How many spans of its link's history a Mac's floor needs before it counts (measured within
    /// `floor_s`): younger, its answers may cut, never grow (agentcore::agent::live).
    pub young_spans: u64,
    /// How often each Mac's probe on its own ssh to the NAS answers (s; agentcore::agent::apart);
    /// 0: no probe.
    pub apart_s: u64,
    /// Whether the window judges the probes' answers (`Answer::apart_s`); false, in shadow: they
    /// are recorded and shown (`Answer::probe`), the window judging the jobs' session's listings.
    pub apart_judges: bool,
}

/// How often the lead reads the pacing, by default (s).
pub const READ_EVERY_S: u64 = 10;
/// A Mac's floor's span, by default (s): the day, as the probes' best is (`KEEP_S`).
pub const FLOOR_S: u64 = 24 * 3600;
/// The window's additive step, its least, and the least a job is paced at, by default
/// (thousandths of the NAS).
pub const STEP: u32 = 50;
/// The window once congested, by default: this share of the weight in use.
pub const DECREASE: f64 = 0.5;
/// The growth's cadence, by default (s).
pub const GROW_S: u64 = 60;
/// How long the readings judged against are kept, by default (s): ten growth cadences (the
/// longer, the likelier the use moved a step within it, and the likelier an outside load came or
/// went between a reading and those it's judged against).
pub const TRACK_S: u64 = 10 * GROW_S;
/// The span a Mac's NAS answer time is read over, by default (s): its listings every 5 s, six of
/// them (`answer_of`).
pub const SPAN_S: u64 = 30;
/// A listing quicker than this was the SMB cache's, by default (ms): on 19 Nov the mini (wired)
/// and the M4 listed the project's folder from their caches in 0.05 to 0.2 ms, from the NAS in
/// 0.9 ms and more.
pub const CACHED_MS: f64 = 0.5;

/// A Mac's NAS answer time from its listings over a span (ms; `samples`): the lower quartile (the
/// ⌈n/4⌉-th least) of those not quicker than `cached_ms` (the SMB cache's); None with none left.
/// A listing queued behind the Mac's own jobs' reads on its SMB session is slow now and then (on
/// 19 Nov the mini beside its job: 1 to 2 ms, with 41 ms or 450 ms between), which the quartile
/// passes over as long as a quarter of the span's listings aren't; a NAS answering most of them
/// slowly raises it, however quick a few. A Mac's floor is the least of these over its floor's
/// span (agentcore::agent::live), the same reading of the same listings.
pub fn answer_of(samples: impl IntoIterator<Item = f64>, cached_ms: f64) -> Option<f64> {
    let mut v: Vec<f64> = samples.into_iter().filter(|x| x.is_finite() && *x >= cached_ms).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len().div_ceil(4) - 1])
}
/// The longest a kept state is taken up, by default (s).
pub const RESTORE_S: u64 = 30 * 60;
/// How long a congested reading holds a lone job back, by default (s): as long as a job waits
/// for the budget before it goes as an aged one (`OWN_AGE`).
pub const LONE_S: u64 = OWN_AGE.as_secs();
/// The longest a hold in a row lasts, by default (s): 8 times `LONE_S`.
pub const LONE_MOST_S: u64 = 8 * LONE_S;
/// The spans of its link's history a Mac's floor needs, by default: ten minutes of listings at
/// the default span.
pub const YOUNG_SPANS: u64 = 20;
/// How often a Mac's probe on its own connection answers, by default (s): as often as its
/// listings on the jobs' session.
pub const APART_S: u64 = 5;

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            step: STEP,
            small: STEP,
            decrease: DECREASE,
            floor_s: FLOOR_S,
            slow: store::aimd::SLOW,
            grow_s: GROW_S,
            track_s: TRACK_S,
            read_s: READ_EVERY_S,
            span_s: SPAN_S,
            cached_ms: CACHED_MS,
            restore_s: RESTORE_S,
            lone_s: LONE_S,
            lone_most_s: LONE_MOST_S,
            young_spans: YOUNG_SPANS,
            apart_s: APART_S,
            apart_judges: false,
        }
    }
}

/// One of the pacing's bounds as a pool setting (crate::settings::Knob).
pub type Knob = crate::settings::Knob<Tuning>;

/// The pacing's bounds as the pool's settings (agentcore::coord::asks): each a number in its range,
/// null its default.
pub const KNOBS: [Knob; 16] = [
    Knob {
        name: "nas-step",
        what: "the window's step a growth cadence, and its least; answers are judged against those this much lighter or heavier in measured use",
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
        what: "the window once congested, a share of the weight in use",
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
        what: "the knee: answers past this many times their best hold the window unless they're not the pool's doing, and cut it where they rise with its use",
        unit: "×",
        least: 1.5,
        most: 10.0,
        get: |k| k.slow,
        set: |k, v| k.slow = v,
    },
    Knob {
        name: "nas-grow-seconds",
        what: "the growth's cadence: the window doubles, or grows a step, in this long; the answers of this long are judged",
        unit: "s",
        least: 10.0,
        most: 600.0,
        get: |k| k.grow_s as f64,
        set: |k, v| k.grow_s = v as u64,
    },
    Knob {
        name: "nas-track-minutes",
        what: "the answers are judged against those of this long at a step lighter or heavier in the pool's measured NAS use",
        unit: "min",
        least: 2.0,
        most: 60.0,
        get: |k| k.track_s as f64 / 60.0,
        set: |k, v| k.track_s = (v * 60.0) as u64,
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
        what: "a Mac's NAS answer time: the lower quartile of its listings over this span, its jobs' traffic its mean; jobs just started show in the answers this long after",
        unit: "s",
        least: 10.0,
        most: 300.0,
        get: |k| k.span_s as f64,
        set: |k, v| k.span_s = v as u64,
    },
    Knob {
        name: "nas-cached-ms",
        what: "a listing quicker than this was the Mac's SMB cache's, not the NAS's (0: all count)",
        unit: "ms",
        least: 0.0,
        most: 10.0,
        get: |k| k.cached_ms,
        set: |k, v| k.cached_ms = v,
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
        what: "a congested reading holds a job beside nothing back this long",
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
    Knob {
        name: "nas-apart-seconds",
        what: "each Mac's probe on its own ssh to the NAS (a 4 KB read past its cache) answers this often (0: no probe)",
        unit: "s",
        least: 0.0,
        most: 60.0,
        get: |k| k.apart_s as f64,
        set: |k, v| k.apart_s = v as u64,
    },
    Knob {
        name: "nas-apart-judges",
        what: "1: the window judges the probes' answers apart from the jobs' SMB session; 0 (in shadow): they're recorded and shown, the window judges the jobs' session's listings",
        unit: "",
        least: 0.0,
        most: 1.0,
        get: |k| f64::from(u8::from(k.apart_judges)),
        set: |k, v| k.apart_judges = v >= 0.5,
    },
];

impl Tuning {
    /// The tuning with each knob's value as `get` says it (None, or out of its range: its default);
    /// the longest hold never under one, the readings kept never fewer than a growth cadence's.
    pub fn of(get: impl Fn(&str) -> Option<f64>) -> Tuning {
        let mut k = Tuning::default();
        for n in &KNOBS {
            if let Some(v) = get(n.name).filter(|v| (n.least..=n.most).contains(v)) {
                (n.set)(&mut k, v);
            }
        }
        k.lone_most_s = k.lone_most_s.max(k.lone_s);
        k.track_s = k.track_s.max(k.grow_s);
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
    /// outside load that holds the NAS near its cliff goes over it; the congested reading may be
    /// its own); open again once the hold has lasted, or a spare reading has ended it.
    pub fn lone(&self) -> bool {
        let at = self.last.as_ref().map_or(0, |r| r.at);
        self.st.lone_at.is_none_or(|s| at >= s + self.lone_for())
    }

    /// How long the lone way's hold lasts (s): `Tuning::lone_s`, doubled for each hold since the
    /// NAS last had room (a spare reading), to `Tuning::lone_most_s` at most (an outside load that
    /// holds the NAS near its cliff for hours lets a lone job over it ever more rarely).
    fn lone_for(&self) -> u64 {
        let k = &self.tuning;
        let doubled = 1u64.checked_shl(self.st.lone_n.saturating_sub(1)).unwrap_or(u64::MAX);
        k.lone_s.saturating_mul(doubled).min(k.lone_most_s.max(k.lone_s))
    }

    /// A reading (`Sample`) by the bounds `k`: what it did.
    pub fn read(&mut self, s: &Sample, k: &Tuning) -> Verdict {
        let busiest = s.busy.and_then(|b| b.iter().copied().filter(|b| b.is_finite()).reduce(f64::max));
        match busiest {
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
        // (Each over the least floor among the answers of its own source, probe or listing: a
        // listing's 2 ms never divides a probe's excess.)
        let least_of = |apart: bool| {
            each.iter()
                .filter(|a| a.floor_ms > 0.0 && (a.apart_s > 0) == apart)
                .map(|a| a.floor_ms)
                .fold(f64::INFINITY, f64::min)
        };
        let (least_listed, least_apart) = (least_of(false), least_of(true));
        let ratio_of = |a: &Answer| a.ratio(if a.apart_s > 0 { least_apart } else { least_listed });
        each.sort_by(|a, b| ratio_of(b).total_cmp(&ratio_of(a)));
        // (A Mac at home missing its deadlines: past the knee whatever its rank.)
        let home_missed = each.iter().find(|a| a.home && a.missed).copied();
        let counted = home_missed.or(each.get(usize::from(each.len() >= 2)).copied()).cloned();
        let ratio = counted.as_ref().map(ratio_of);
        // (The counted Mac's probe apart, judged or in shadow: over the least probe floor among
        // the Macs' probes.)
        let probe_least = s
            .answers
            .iter()
            .filter_map(|a| a.probe.as_ref()?.floor_ms.filter(|f| *f > 0.0))
            .fold(f64::INFINITY, f64::min);
        let probe_ratio = counted.as_ref().and_then(|c| {
            let p = c.probe.as_ref()?;
            Some((1.0 + (p.ms? - p.floor_ms?).max(0.0) / probe_least).min(RATIO_MOST))
        });
        let fresh = !s.blind && each.iter().all(|a| a.fresh);
        // (The pool's use the answers are judged beside: what the Macs' jobs move on the NAS as
        // measured, the counted Mac's own apart; judged with the rest where its answers were taken
        // apart from its jobs' session, as its jobs' traffic doesn't stall them.)
        let apart = counted.as_ref().is_some_and(|c| c.apart_s > 0);
        let is_own = |m: &str| counted.as_ref().is_some_and(|c| c.mac == m);
        let of = |own: bool| {
            (!s.moving.is_empty())
                .then(|| s.moving.iter().filter(|m| is_own(&m.0) == own).fold(0u32, |a, m| a.saturating_add(m.1)))
        };
        let (used, own) = (of(false), of(true));
        let prev = self.prev_in_use;
        if s.in_use > prev {
            if self.rose_at.is_none_or(|r| t >= r + k.span_s + k.grow_s) {
                self.rose_from = prev;
            }
            self.rose_at = Some(t);
        }
        self.prev_in_use = s.in_use;
        let seen = &mut self.st.seen;
        if let Some(r) = ratio {
            let judged = if apart { used.map(|u| u.saturating_add(own.unwrap_or(0))) } else { used };
            seen.push(Seen { t, used: judged, own, held: s.in_use, ratio: r, apart });
        }
        seen.retain(|x| x.t + k.track_s > t);
        let j = judge(seen, t, k);
        // (Under the knee, the answers known good there: a span. At it or past, or no knee known,
        // a growth cadence more where the weight rose a step or more.)
        let under = j.knee.is_some_and(|at| s.in_use < at);
        let stepped = !under && s.in_use >= self.rose_from.saturating_add(k.step);
        let started = self.rose_at.is_some_and(|r| t < r + k.span_s || (stepped && t < r + k.span_s + k.grow_s));
        let rising = j.rising;
        let slow = rising && j.done && j.quick.is_some_and(|q| q > k.slow);
        // (Undecidable answers swelling past the knee: the counted Mac's link or the NAS, which
        // can't be told, but the window goes back to the knee (`Judged::knee`: the heaviest weight
        // the answers weren't past it at), never under it: it doesn't halve on what may be its own
        // link; halved where there's none.)
        let to_knee = j.swelling
            && j.done
            && j.quick.is_some_and(|q| q > k.slow)
            && j.knee.is_none_or(|at| f64::from(at) < self.st.window);
        // (Over the target: the smoothed share past it, or this reading's busiest disk at it or
        // over: the smoothing's half minute lags a burst, and the cliff is a reading away.)
        let over = self.load.is_some_and(|l| l > s.pct as f64) || busiest.is_some_and(|b| b >= f64::from(s.pct));
        let missed = counted.as_ref().is_some_and(|a| a.missed);
        let past = over || missed || slow || to_knee;
        // (The knee: this reading's answers past it, or the last span's, their lower quartile;
        // unless a growth cadence of them is judged not the pool's doing, the disks read.)
        let outside = j.outside && j.done && self.load.is_some();
        let span = seen.iter().filter(|x| x.t + k.span_s > t && x.apart == apart).map(|x| x.ratio);
        let knee = !outside && (ratio.is_some_and(|r| r > k.slow) || quartile(span, 1).is_some_and(|q| q > k.slow));
        // (Room for a job of weight `w`: the disks' busy share smoothed and its weight, a share of
        // the NAS as probed, within the target (a reading at the target or over is congested); the
        // disks not read, the answers alone say. Spare: a step's room, the answers read, not rising,
        // not held at the knee.)
        let load = self.load;
        let room_for = |w: u32| load.is_none_or(|l| l + f64::from(w) / 10.0 <= f64::from(s.pct));
        let room = room_for(s.wants.unwrap_or(k.step));
        let spare = ratio.is_some() && !rising && !j.swelling && !knee && room_for(k.step);
        // (The lone way's hold: begun at a congested reading; once it has lasted its time
        // (`lone_for`), let go by a reading not congested or a job going beside nothing, a
        // congested reading then beginning another, twice as long; a spare reading ends it
        // whenever, and its doubling.)
        let expired = self.st.lone_at.is_some_and(|at| t >= at + self.lone_for());
        if expired && (!past || (prev == 0 && s.in_use > 0)) {
            self.st.lone_at = None;
        }
        if spare && !past {
            (self.st.lone_at, self.st.lone_n) = (None, 0);
        }
        if past && self.st.lone_at.is_none() {
            self.st.lone_at = Some(t);
            self.st.lone_n = self.st.lone_n.saturating_add(1);
        }
        if self.last.is_none() && !self.restored {
            self.st.window = self.st.window.max(in_use);
        }
        self.st.window = self.st.window.min(f64::from(WHOLE));
        let st = &mut self.st;
        if st.cut_at.is_some() && st.settled_at.is_none() && in_use <= st.window {
            st.settled_at = Some(t);
        }
        let dt = self.last.as_ref().map_or(k.read_s, |r| t.saturating_sub(r.at)).min(k.grow_s) as f64
            / k.grow_s.max(1) as f64;
        let unread = ratio.is_none() && self.load.is_none();
        // (The most past the load its readings tested it grows to: one job more, the next one
        // waiting (a step at least, none waiting); in the slow start, twice the weight in use too
        // (two steps at least). A jump of one job's weight, which the next congested reading cuts
        // back.)
        let additive = st.threshold.is_some_and(|th| st.window >= th);
        let one_more = in_use + s.wants.map_or(0.0, f64::from).max(step);
        let ceiling = if additive { one_more } else { one_more.max(2.0 * in_use.max(step)) };
        let hold = if unread {
            None
        } else if past {
            (st.cut_at.is_some() && st.settled_at.is_none_or(|at| t < at + k.span_s)).then_some(Hold::HoldOff)
        } else if ratio.is_none() {
            Some(Hold::NoAnswer)
        } else if rising || j.swelling {
            Some(Hold::Rising)
        } else if knee {
            Some(Hold::Slow)
        } else if !room {
            Some(Hold::Busy)
        } else if !fresh {
            Some(Hold::NotFresh)
        } else if started {
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
                let halved = (in_use * k.decrease).max(step);
                let to = match j.knee {
                    Some(at) if to_knee && !(over || missed || slow) => f64::from(at).max(step),
                    _ => halved,
                };
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
                st.window = grown.min(ceiling).min(f64::from(WHOLE));
                Verdict::Grew
            }
        };
        let threshold = st.threshold.map(|th| th.round() as u32);
        let load_pct = self.load.map(|l| (l * 10.0).round() / 10.0);
        let disk_pct = busiest.map(|l| (l * 10.0).round() / 10.0);
        let lone_until = self.st.lone_at.map(|at| at + self.lone_for()).filter(|&u| t < u);
        self.last = Some(Reading {
            at: t,
            counted,
            ratio,
            below: j.lighter,
            rising: rising || j.swelling,
            outside,
            undecided: j.undecided,
            past,
            load_pct,
            disk_pct,
            in_use: s.in_use,
            used,
            own,
            wants: s.wants,
            verdict,
            threshold,
            lone_until,
            probe_ratio,
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
                _ if r.disk_pct.is_some_and(|d| d >= self.target as f64) => format!(
                    "a disk {:.0} % busy in its last reading, at the target {} % or over",
                    r.disk_pct.unwrap_or(0.0),
                    self.target
                ),
                (Some(a), ..) if a.missed => format!("missing its deadlines from {}", a.mac),
                (Some(a), _, Some(x)) if r.past && r.rising => {
                    format!("answering {} in {:.0} ms, {:.1}× its best, rising with the pool's use", a.mac, a.ms, x)
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
        Answer { mac: mac.into(), home: true, ms, floor_ms: floor, missed: false, fresh: true, apart_s: 0, probe: None }
    }

    /// The disks quiet: 2 % busy.
    const QUIET: [f64; 1] = [2.0];

    /// The pool's jobs moving `used` thousandths on a Mac that never answers (`Sample::moving`).
    fn jobs(used: u32) -> [(String, u32); 1] {
        [("jobs".to_string(), used)]
    }

    /// A reading of `answers`, the disks at `busy`, the jobs moving `moving`, by the default
    /// tuning.
    fn sample(
        p: &mut Pace,
        t: u64,
        answers: &[Answer],
        busy: &[f64],
        in_use: u32,
        moving: &[(String, u32)],
        wants: Option<u32>,
    ) -> Verdict {
        let s = Sample { t, answers, blind: false, busy: Some(busy), pct: DEFAULT_TARGET_PCT, in_use, moving, wants };
        p.read(&s, &Tuning::default())
    }

    /// A reading of `answers`, the disks at `busy`, the jobs moving what they weigh, by the default
    /// tuning.
    fn disks(p: &mut Pace, t: u64, answers: &[Answer], busy: &[f64], in_use: u32, wants: Option<u32>) -> Verdict {
        sample(p, t, answers, busy, in_use, &jobs(in_use), wants)
    }

    /// A reading of `answers`, the disks quiet, by the default tuning.
    fn read(p: &mut Pace, t: u64, answers: &[Answer], in_use: u32, wants: Option<u32>) -> Verdict {
        disks(p, t, answers, &QUIET, in_use, wants)
    }

    /// A reading of one Mac answering in `ms` against a floor of 5 ms, the disks quiet.
    fn at(p: &mut Pace, t: u64, ms: f64, in_use: u32, wants: Option<u32>) -> Verdict {
        read(p, t, &[ans("m1", ms, 5.0)], in_use, wants)
    }

    /// A reading of one Mac missing its deadlines, the disks quiet.
    fn miss(p: &mut Pace, t: u64, in_use: u32, wants: Option<u32>) -> Verdict {
        read(p, t, &[Answer { missed: true, ..ans("m1", 5.0, 5.0) }], in_use, wants)
    }

    /// `n` readings of `answers` every `READ_EVERY_S` from `t` on (`t` left after the last), the
    /// disks quiet: their verdicts.
    fn readings(
        p: &mut Pace,
        t: &mut u64,
        n: usize,
        answers: &[Answer],
        in_use: u32,
        wants: Option<u32>,
    ) -> Vec<Verdict> {
        (0..n)
            .map(|_| {
                *t += READ_EVERY_S;
                read(p, *t, answers, in_use, wants)
            })
            .collect()
    }

    /// `readings` of one Mac answering in `ms` against a floor of 5 ms.
    fn run(p: &mut Pace, t: &mut u64, n: usize, ms: f64, in_use: u32, wants: Option<u32>) -> Vec<Verdict> {
        readings(p, t, n, &[ans("m1", ms, 5.0)], in_use, wants)
    }

    /// The readings from a rise to its first cut on answers rising past the knee: a growth
    /// cadence's at the use risen to.
    const JUDGED: usize = (GROW_S / READ_EVERY_S) as usize;

    /// The readings from a rise of a step or more to its first growth: a span's and a growth
    /// cadence's held for its jobs' answers to show and be read, the next grows.
    const STARTED: usize = ((SPAN_S + GROW_S) / READ_EVERY_S) as usize + 1;

    /// Whether `v` ends in a cut, none before.
    fn cut_last(v: &[Verdict]) -> bool {
        v.last() == Some(&Verdict::Cut) && !v[..v.len() - 1].contains(&Verdict::Cut)
    }

    /// Whether `v` holds one cut, and only the hold-off after it.
    fn cut_once(v: &[Verdict]) -> bool {
        let at = v.iter().position(|v| *v == Verdict::Cut);
        at.is_some_and(|i| v[i + 1..].iter().all(|v| *v == Verdict::Held(Hold::HoldOff)))
    }

    /// A pacing whose answers `best` were read at no load for a minute (a level the load rises
    /// from), and the time of its last reading.
    fn primed(best: &[Answer]) -> (Pace, u64) {
        let mut p = Pace::default();
        for t in (0..60).step_by(10) {
            read(&mut p, t, best, 0, None);
        }
        (p, 50)
    }

    /// With room the window doubles a minute while it's what limits, until its first cut, and not
    /// until the jobs last started show in the answers; answers rising with the pool's use hold it; past the
    /// knee and rising over a growth cadence's readings it drops to half the weight in use at once;
    /// then it doubles back to the slow start's threshold and grows a step a minute.
    #[test]
    fn the_window_grows_slowly_and_is_cut_quickly() {
        let mut p = Pace::default();
        assert_eq!(p.window(), STEP);
        // (Not what limits: nothing waits, and the weight in use is far under it.)
        assert!(matches!(at(&mut p, 0, 5.0, 0, None), Verdict::Held(_)));
        let mut t = 0;
        assert!(run(&mut p, &mut t, 6, 5.0, 0, Some(150)).iter().all(|v| *v == Verdict::Grew));
        assert_eq!(p.window(), 2 * STEP, "doubled in a minute");
        // (Jobs started: held a span, until their answers show.)
        let held = run(&mut p, &mut t, STARTED, 5.0, 700, Some(150));
        let (last, judging) = held.split_last().unwrap();
        assert!(judging.iter().all(|v| *v == Verdict::Held(Hold::Started)) && *last == Verdict::Grew, "{held:?}");
        assert!(run(&mut p, &mut t, 11, 5.0, 700, Some(150)).iter().all(|v| *v == Verdict::Grew));
        assert_eq!(p.window(), 4 * 2 * STEP, "slow start: doubling a minute");
        // (A step heavier, slower than the step lighter, under the knee: held while the jobs
        // started are judged, then rising.)
        let v = run(&mut p, &mut t, JUDGED, 10.0, 800, Some(150));
        assert_eq!(v.last(), Some(&Verdict::Held(Hold::Rising)), "{v:?}");
        assert!(!v.contains(&Verdict::Grew), "{v:?}");
        assert_eq!(p.last.as_ref().unwrap().below, Some(1.0), "the answers at 700 and at nothing");
        assert_eq!(p.window(), 400);
        // (A step heavier again, past the knee and rising: half what's in use, once a growth
        // cadence's readings show it.)
        let v = run(&mut p, &mut t, JUDGED, 16.0, 900, Some(150));
        assert!(cut_once(&v), "{v:?}");
        assert_eq!(p.window(), 400, "half the 900 in use: no more than it was");
        assert_eq!(
            p.why().unwrap(),
            format!(
                "NAS work paced to {} of the NAS, 90 % in use (the NAS answering m1 in 16 ms, 3.2× its best, \
                 rising with the pool's use)",
                pct(p.window())
            )
        );
        assert_eq!(
            at(&mut p, t + 10, 16.0, 900, Some(150)),
            Verdict::Held(Hold::HoldOff),
            "the jobs out before the cut"
        );
        // (Nothing of the pool's out: slow answers aren't its doing, but missed deadlines cut it,
        // never under a step.)
        t += 100;
        assert_eq!(at(&mut p, t, 40.0, 0, Some(150)), Verdict::Held(Hold::Slow), "not the pool's doing: held, not cut");
        t += SPAN_S;
        assert_eq!(miss(&mut p, t, 0, Some(150)), Verdict::Cut);
        assert_eq!(p.window(), STEP);
        // (Under its threshold, it doubles back to it, then adds a step a minute.)
        let mut q = Pace::default();
        at(&mut q, 0, 5.0, 0, None);
        let mut t = 0;
        assert!(cut_last(&run(&mut q, &mut t, JUDGED, 40.0, 400, None)));
        assert_eq!((q.window(), q.st.threshold), (STEP, Some(200.0)));
        // (The answers at their best again at the load cut from: no longer rising once a quarter
        // of the cadence's are, and the window grows past the hold-off once the jobs started are
        // judged.)
        let v = run(&mut q, &mut t, 15, 5.0, 400, None);
        assert_eq!(
            v[..3],
            [Verdict::Held(Hold::HoldOff), Verdict::Held(Hold::Started), Verdict::Held(Hold::Started)],
            "{v:?}"
        );
        assert!(v[3..].iter().all(|v| *v == Verdict::Grew), "grown past the hold-off: {v:?}");
        assert_eq!(q.window(), 200, "doubled back to half what was in use at the cut, no further");
        run(&mut q, &mut t, 6, 5.0, 400, None);
        assert_eq!(q.window(), 250, "then a step a minute");
        // (A lead starting beside jobs already leased: from what they weigh.)
        let mut q = Pace::default();
        assert!(matches!(at(&mut q, 0, 5.0, 700, None), Verdict::Held(_)));
        assert_eq!(q.window(), 700);
    }

    /// The answers are judged against those of the last `track_s` a step lighter and heavier in
    /// the pool's measured use: slow at a use under a step, or no slower than a step lighter, or
    /// as slow as those a step heavier, they're not the pool's doing, and the window grows past
    /// the knee; slower than a step lighter they're rising, and past the knee congested.
    #[test]
    fn the_answers_are_judged_against_the_pools_measured_use() {
        let mut p = Pace::default();
        // (A minute at no use, one slow answer in four: the quartile passes over it.)
        for (i, t) in (0..60).step_by(10).enumerate() {
            at(&mut p, t, if i % 4 == 3 { 100.0 } else { 5.0 }, 0, None);
        }
        let mut t = 50;
        // (40 in use, under a step: 20 times its best is not the pool's doing: never cut, and once
        // a growth cadence of them is judged the window grows past the knee.)
        let v = run(&mut p, &mut t, 12, 100.0, 40, Some(150));
        assert!(!v.contains(&Verdict::Cut), "{v:?}");
        assert_eq!(v.last(), Some(&Verdict::Grew), "{v:?}");
        assert!(p.last.as_ref().unwrap().outside);
        // (Up to 150 as slow: as slow a step lighter, so not its doing either.)
        let v = run(&mut p, &mut t, 12, 100.0, 150, Some(150));
        assert!(!v.contains(&Verdict::Cut), "{v:?}");
        let r = p.last.clone().unwrap();
        assert!(r.outside && !r.rising && r.below == Some(20.0), "{r:?}");
        // (Back to its best at 40 for most of a `track_s`, then up to 150 at 20 times its best:
        // slower than a step lighter, rising, past the knee: cut once a cadence of them is read.)
        run(&mut p, &mut t, 40, 5.0, 40, Some(150));
        let v = run(&mut p, &mut t, JUDGED, 100.0, 150, Some(150));
        assert!(cut_last(&v), "{v:?}");
        assert_eq!(p.last.as_ref().unwrap().below, Some(1.0));
        // (Fallen back to 40, as slow as a step heavier was: not its doing; grown from the
        // hold-off once a growth cadence of them is judged.)
        let v = run(&mut p, &mut t, 12, 100.0, 40, Some(150));
        assert!(!v.contains(&Verdict::Cut) && v.last() == Some(&Verdict::Grew), "{v:?}");
        assert!(p.last.as_ref().unwrap().outside);
        // (A noisy lighter use: answers within its upper quartile aren't rising, past it they are.)
        let (mut q, mut t) = (Pace::default(), 0);
        for ms in [5.0, 12.5, 5.0, 12.5, 5.0, 5.0] {
            run(&mut q, &mut t, 1, ms, 0, None);
        }
        let v = run(&mut q, &mut t, 12, 12.5, 300, Some(150));
        assert_eq!(q.last.as_ref().unwrap().below, Some(2.5), "two slow of six: the quartile among them");
        assert!(!v.contains(&Verdict::Cut) && v.last() == Some(&Verdict::Grew), "as slow as a step lighter: {v:?}");
        let v = run(&mut q, &mut t, JUDGED, 20.0, 450, Some(150));
        assert!(cut_once(&v), "slower: rising: {v:?}");
        // (Only what the jobs move counts, not what they weigh: the weight held while the use
        // rises a step, the answers slowing with it past the knee: cut.)
        let (mut q, mut t) = (Pace::default(), 0);
        let m1 = [ans("m1", 5.0, 5.0)];
        for _ in 0..12 {
            t += READ_EVERY_S;
            sample(&mut q, t, &m1, &QUIET, 300, &jobs(20), None);
        }
        let v: Vec<Verdict> = (0..JUDGED)
            .map(|_| {
                t += READ_EVERY_S;
                sample(&mut q, t, &[ans("m1", 40.0, 5.0)], &QUIET, 300, &jobs(200), None)
            })
            .collect();
        assert!(cut_last(&v), "{v:?}");
        // (Nothing measured: nothing judged; answers past the knee hold the window, never cut it.)
        let (mut q, mut t) = (Pace::default(), 0);
        let v: Vec<Verdict> = (0..30)
            .map(|i| {
                t += READ_EVERY_S;
                sample(&mut q, t, &[ans("m1", if i < 12 { 5.0 } else { 40.0 }, 5.0)], &QUIET, 300, &[], Some(150))
            })
            .collect();
        assert!(!v.contains(&Verdict::Cut) && v.last() == Some(&Verdict::Held(Hold::Slow)), "{v:?}");
    }

    /// Task #1506's case of 20 Nov 08:30Z: the NAS slow from outside the pool while the pool moves
    /// little on it. Five minutes at their best, the jobs weighing 30 and moving 10, nothing waiting
    /// (the window at its floor); then a job starts (63 in use) as an outside load takes the mini's
    /// answers to 41 ms on its 2-ms floor for ten minutes (20.5 times), the jobs moving 25, work
    /// waiting, the disks 39 % busy. Judged
    /// against the weight the slowing would read as the job's (it rose with it), the window cut to
    /// its floor and held there within the hold-off while the outside load lasts; against what the
    /// pool moves it isn't the pool's doing: never cut, and grown past its floor.
    #[test]
    fn outside_slowness_with_the_pool_moving_little_never_holds_the_window_at_its_floor() {
        let (mut p, mut t) = (Pace::default(), 0);
        let mini = |ms| [ans("m6", ms, 2.0)];
        let mut v = Vec::new();
        for i in 0..90 {
            t += READ_EVERY_S;
            let (ms, in_use, moving, wants) = if i < 30 { (2.0, 30, 10, None) } else { (41.0, 63, 25, Some(100)) };
            v.push(sample(&mut p, t, &mini(ms), &[39.0], in_use, &jobs(moving), wants));
        }
        assert_eq!(v[29], Verdict::Held(Hold::NotLimiting), "at its floor, nothing waiting");
        let slow = &v[30..];
        assert!(!v.contains(&Verdict::Cut), "{v:?}");
        assert!(slow.contains(&Verdict::Grew), "{slow:?}");
        assert!(p.window() > STEP, "the window {} at its floor", p.window());
        let r = p.last.as_ref().unwrap();
        assert!(r.outside && r.ratio == Some(20.5), "{r:?}");
    }

    /// The pool the NAS's load: the jobs leased weigh 300 throughout, moving 40 for five minutes,
    /// the answers at their best, then 250 as they reach their heaviest phases, the answers 20
    /// times their best with it, the disks 50 % busy: cut within a growth cadence of the rise in
    /// what they move. Judged against the weight, which never moved, the slowing would never cut.
    /// Once what they move falls back, the answers at their best again, nothing more is cut.
    #[test]
    fn the_pool_slowing_the_nas_by_what_it_moves_cuts_the_window() {
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..120 {
            t += READ_EVERY_S;
            let (ms, moving) = if (30..60).contains(&i) { (40.0, 250) } else { (2.0, 40) };
            v.push(sample(&mut p, t, &[ans("m6", ms, 2.0)], &[50.0], 300, &jobs(moving), Some(100)));
        }
        let cut = v.iter().position(|v| *v == Verdict::Cut);
        assert!(cut.is_some_and(|i| (30..30 + JUDGED).contains(&i)), "{v:?}");
        assert_eq!(v.iter().filter(|v| **v == Verdict::Cut).count(), 1, "{v:?}");
        assert_eq!(p.st.threshold, Some(150.0), "half the 300 in use");
    }

    /// Task #1506's review: the counted Mac the pool's only mover. The mini alone answering, its
    /// own jobs moving 30 for five minutes at its best, then 300 for ten, its answers 20 times
    /// their best with them, the weight in use 300 throughout, work waiting: its answers ride its
    /// own jobs' session, so they can't say whether the NAS or its link slowed (undecidable):
    /// never halved on them, but cut once back to the knee (the 300 its answers were at their best
    /// at), and never grown (held as rising with all the pool moves, or past the knee); the status
    /// line says so. The same with the disks not read; and the 08:30 case (the pool moving little,
    /// the answers slow from outside) with the disks not read holds at the knee too: answers alone
    /// never lift it.
    #[test]
    fn the_answering_macs_own_jobs_moving_leave_its_answers_undecidable() {
        let mini = |ms| [ans("m6", ms, 2.0)];
        let on_mini = |u: u32| [("m6".to_string(), u)];
        for busy in [Some(&[39.0][..]), None] {
            let (mut p, mut t) = (Pace::default(), 0);
            let mut v = Vec::new();
            for i in 0..90 {
                t += READ_EVERY_S;
                let (ms, moving) = if i < 30 { (2.0, 30) } else { (40.0, 300) };
                let moving = on_mini(moving);
                let s = Sample {
                    t,
                    answers: &mini(ms),
                    blind: false,
                    busy,
                    pct: 85,
                    in_use: 300,
                    moving: &moving,
                    wants: Some(100),
                };
                v.push(p.read(&s, &Tuning::default()));
            }
            let held = |v: &Verdict| matches!(v, Verdict::Held(Hold::Slow | Hold::Rising | Hold::HoldOff));
            let slow = &v[30 + STARTED..];
            assert!(!slow.contains(&Verdict::Grew), "disks {busy:?}: {slow:?}");
            assert!(v.iter().filter(|v| **v == Verdict::Cut).count() <= 1, "disks {busy:?}: {v:?}");
            assert!(slow.iter().all(|v| held(v) || *v == Verdict::Cut), "disks {busy:?}: {slow:?}");
            assert!(p.window() >= 300, "never under the knee: {}", p.window());
            let r = p.last.as_ref().unwrap();
            assert!(r.undecided && !r.outside && r.own == Some(300) && r.used == Some(0), "{r:?}");
            assert!(
                r.line(p.window(), t).contains("30 % moving (30 % of it m6's)")
                    && r.line(p.window(), t).contains("(undecidable: the answering Mac, m6, moves 30 %)"),
                "{}",
                r.line(p.window(), t)
            );
        }
        // (Its own a step, another Mac's under it: undecidable all the same.)
        let (mut p, mut t) = (Pace::default(), 0);
        for i in 0..30 {
            t += READ_EVERY_S;
            let moving = [("m6".to_string(), 60), ("m1".to_string(), if i < 15 { 0 } else { 40 })];
            sample(&mut p, t, &mini(if i < 15 { 2.0 } else { 40.0 }), &[39.0], 300, &moving, Some(100));
        }
        let r = p.last.as_ref().unwrap();
        assert!(r.undecided && matches!(r.verdict, Verdict::Held(Hold::Slow | Hold::Rising)), "{r:?}");
        // (The 08:30 case, the disks not read: held at the knee.)
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..90 {
            t += READ_EVERY_S;
            let (ms, in_use, moving, wants) = if i < 30 { (2.0, 30, 10, None) } else { (41.0, 63, 25, Some(100)) };
            let moving = jobs(moving);
            let s = Sample { t, answers: &mini(ms), blind: false, busy: None, pct: 85, in_use, moving: &moving, wants };
            v.push(p.read(&s, &Tuning::default()));
        }
        assert!(!v[30..].contains(&Verdict::Grew) && !v.contains(&Verdict::Cut), "{v:?}");
        assert_eq!(p.last.as_ref().unwrap().verdict, Verdict::Held(Hold::Slow));
    }

    /// Task #1552: the same mini, its answers taken apart from its jobs' session (its probe's own
    /// ssh, `Answer::apart_s`): its own jobs' traffic is the pool's like any Mac's, so answers
    /// rising with it are judged the pool's and halve the window (not back to the knee alone),
    /// never undecidable; the status line says which connection answered. Undone (the answers on
    /// the jobs' session), the case above. And slow answers apart while its own jobs move a step
    /// or more are never judged not the pool's: held at the knee, never grown past it.
    #[test]
    fn answers_taken_apart_judge_the_answering_macs_own_jobs() {
        let mini = |ms| [Answer { apart_s: 5, ..ans("m6", ms, 2.0) }];
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..90 {
            t += READ_EVERY_S;
            let (ms, moving) = if i < 30 { (2.0, 30) } else { (40.0, 300) };
            let moving = [("m6".to_string(), moving)];
            v.push(sample(&mut p, t, &mini(ms), &[39.0], 300, &moving, Some(100)));
        }
        let cut = v.iter().position(|v| *v == Verdict::Cut).expect("cut on its own jobs' load");
        assert!(cut >= 30, "{v:?}");
        assert!(p.window() <= 150, "halved, not back to the knee: {}", p.window());
        assert!(!v[30..].contains(&Verdict::Grew), "{v:?}");
        let r = p.last.as_ref().unwrap();
        assert!(!r.undecided && r.own == Some(300) && r.used == Some(0), "{r:?}");
        assert!(p.st.seen.last().is_some_and(|x| x.apart && x.used == Some(300)), "judged with the pool's");
        let line = r.line(p.window(), t);
        assert!(line.contains("30 % moving (30 % of it m6's)"), "{line}");
        assert!(line.contains("answers on m6's own ssh to the NAS (every 5 s)"), "{line}");
        assert!(!line.contains("undecidable"), "{line}");
        // (Slow from outside with nothing moving, then its own jobs moving 100 as slow: no
        // slower, but never judged not the pool's, so never grown past the knee.)
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..60 {
            t += READ_EVERY_S;
            let moving = [("m6".to_string(), if i < 30 { 0 } else { 100 })];
            v.push(sample(&mut p, t, &mini(40.0), &[39.0], 100, &moving, Some(100)));
        }
        assert!(!v[30..].contains(&Verdict::Grew), "{v:?}");
        let r = p.last.as_ref().unwrap();
        assert!(!r.outside && r.verdict == Verdict::Held(Hold::Slow), "{r:?}");
        // (On the jobs' session: undecidable, and said so.)
        let (mut p, mut t) = (Pace::default(), 0);
        for i in 0..40 {
            t += READ_EVERY_S;
            let moving = [("m6".to_string(), if i < 30 { 30 } else { 300 })];
            sample(&mut p, t, &[ans("m6", if i < 30 { 2.0 } else { 40.0 }, 2.0)], &[39.0], 300, &moving, Some(100));
        }
        let r = p.last.as_ref().unwrap();
        assert!(r.undecided, "{r:?}");
        assert!(r.line(p.window(), t).contains("answers on m6's jobs' SMB session"), "{}", r.line(p.window(), t));
    }

    /// Task #1552's review (B1): a probe's excess is over the least probe floor, never a listing's:
    /// the mini's probe at 10 ms (floor 6) is 1.67 times its best beside the M4's listings (floor 2
    /// ms), not 3.0; at 31.7 ms, 5.3, not 13.9.
    #[test]
    fn a_probes_ratio_is_over_the_least_probe_floor() {
        for (ms, want) in [(10.0, 1.0 + 4.0 / 6.0), (31.7, 1.0 + 25.7 / 6.0)] {
            let answers = [
                Answer { apart_s: 5, ..ans("m6", ms, 6.0) },
                ans("m4", 2.0, 2.0),
                // (The highest, so the mini's counts.)
                ans("m1", 80.0, 2.0),
            ];
            let mut p = Pace::default();
            sample(&mut p, 10, &answers, &QUIET, 50, &jobs(0), None);
            let r = p.last.as_ref().unwrap();
            assert_eq!(r.counted.as_ref().map(|c| c.mac.as_str()), Some("m6"));
            assert!((r.ratio.unwrap() - want).abs() < 1e-9, "{ms} ms: {:?} against {want}", r.ratio);
        }
    }

    /// Task #1552's review (B1): the answers are judged against readings of their own source. Five
    /// minutes of stalled listings (20 times their best at a use of 30), then the mini's probe at
    /// a use of 30 (1.2 times) and at 300 (10 times): judged against the probe's own lighter
    /// readings, rising, cut; against the listings' (all 20), it would hold as slow, never cut.
    #[test]
    fn the_answers_are_judged_against_their_own_sources_readings() {
        let k = Tuning { apart_judges: true, ..Tuning::default() };
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..72 {
            t += READ_EVERY_S;
            let (a, used) = match i {
                0..30 => (ans("m6", 40.0, 2.0), 30),
                30..48 => (Answer { apart_s: 5, ..ans("m6", 7.2, 6.0) }, 30),
                _ => (Answer { apart_s: 5, ..ans("m6", 60.0, 6.0) }, 300),
            };
            let moving = [("m6".to_string(), used)];
            let s = Sample {
                t,
                answers: &[a],
                blind: false,
                busy: Some(&QUIET),
                pct: 85,
                in_use: 300,
                moving: &moving,
                wants: Some(100),
            };
            v.push(p.read(&s, &k));
        }
        assert!(v[48..].contains(&Verdict::Cut), "{v:?}");
    }

    /// Task #1552: in shadow (`nas-apart-judges` off, the answers on the jobs' session, the probe's
    /// figures beside them), the window does as it does without a probe: the same verdicts and
    /// windows, reading by reading; the probe's ratio said.
    #[test]
    fn a_probe_in_shadow_changes_nothing_judged() {
        let run = |probe: bool| {
            let (mut p, mut t) = (Pace::default(), 0);
            let mut v = Vec::new();
            for i in 0..90 {
                t += READ_EVERY_S;
                let ms = if i < 30 { 2.0 } else { 40.0 };
                let shadow =
                    Apart { ms: Some(6.0 + f64::from(i)), floor_ms: Some(6.0), every_s: 5, ..Default::default() };
                let a = Answer { probe: probe.then_some(shadow), ..ans("m6", ms, 2.0) };
                let moving = [("m6".to_string(), if i < 30 { 30 } else { 300 })];
                v.push((sample(&mut p, t, &[a], &[39.0], 300, &moving, Some(100)), p.window()));
            }
            (v, p.last.unwrap())
        };
        let ((with, r), (without, _)) = (run(true), run(false));
        assert_eq!(with, without);
        assert_eq!(r.probe_ratio, Some(1.0 + 89.0 / 6.0));
        let line = r.line(300, r.at);
        assert!(
            line.contains(
                "; probe apart (in shadow, every 5 s, 0 connections, 0 past their deadline): 15.8× its best of 6.0 ms"
            ),
            "{line}"
        );
        assert!(line.contains("answers on m6's jobs' SMB session"), "{line}");
    }

    /// Task #1506's review: no dead band between rising and not the pool's left a cadence a hair
    /// either side of the lighter answers flipping between a cut and a growth past the knee. A
    /// cadence a step heavier whose lower quartile is past the lighter's upper but whose median
    /// isn't is neither: past the knee, held.
    #[test]
    fn a_cadence_between_rising_and_not_the_pools_is_held() {
        let (mut p, mut t) = (Pace::default(), 0);
        // (Ten minutes at a use of 0, the answers 10 or 20 times their best in turn.)
        for i in 0..60 {
            t += READ_EVERY_S;
            sample(
                &mut p,
                t,
                &[ans("m1", if i % 2 == 0 { 20.0 } else { 40.0 }, 2.0)],
                &[20.0],
                300,
                &jobs(0),
                Some(100),
            );
        }
        // (Then 300, the answers 15 and 30 times their best, one in three the quicker: the
        // cadence's lower quartile 15, within the lighter's upper, 20, its median 30, past it:
        // between.)
        let mut v = Vec::new();
        for i in 0..JUDGED * 2 {
            t += READ_EVERY_S;
            let ms = if i % 3 == 0 { 30.0 } else { 60.0 };
            v.push(sample(&mut p, t, &[ans("m1", ms, 2.0)], &[20.0], 300, &jobs(300), Some(100)));
        }
        let r = p.last.as_ref().unwrap();
        assert!(!r.rising && !r.outside && !r.undecided, "{r:?}");
        assert!(!v.contains(&Verdict::Cut) && !v.contains(&Verdict::Grew), "{v:?}");
        assert_eq!(r.verdict, Verdict::Held(Hold::Slow));
    }

    /// Task #1506's review: the window is never more than the whole. Grown with room for long, the
    /// weight in use near the whole and a heavy job waiting, it stops at the whole; a kept state
    /// past it (an older rule's) is taken up at the whole.
    #[test]
    fn the_window_is_never_more_than_the_whole() {
        let (mut p, mut t) = (Pace::default(), 0);
        let v = run(&mut p, &mut t, 120, 5.0, 900, Some(400));
        assert!(v.contains(&Verdict::Grew), "{v:?}");
        assert_eq!(p.window(), WHOLE);
        let kept = State { at: t, window: 1500.0, ..State::default() };
        let mut q = Pace::default();
        assert!(q.restore(&kept, t, RESTORE_S));
        read(&mut q, t + 10, &[ans("m1", 5.0, 5.0)], 900, Some(400));
        assert_eq!(q.window(), WHOLE);
    }

    /// Task #1506's review: a reading whose busiest disk is at the target or over it is congested,
    /// whatever the answers and the smoothed share say (the smoothing lags a burst by half a
    /// minute; the cliff is a reading away): never grown on, and cut. Quiet for a minute, 300 in
    /// use, the window grows; one reading at 85 % (the smoothed share still under half of it) cuts
    /// it to half what's in use; quiet again, past the hold-off, it grows.
    #[test]
    fn a_reading_with_the_disks_at_the_target_cuts_the_window() {
        let (mut p, mut t) = (Pace::default(), 0);
        let best = [ans("m1", 5.0, 5.0)];
        for _ in 0..STARTED + 3 {
            t += READ_EVERY_S;
            disks(&mut p, t, &best, &[10.0], 300, Some(150));
        }
        assert!(p.window() > 300, "grown: {}", p.window());
        t += READ_EVERY_S;
        assert_eq!(disks(&mut p, t, &best, &[85.0], 300, Some(150)), Verdict::Cut);
        assert!(p.load.is_some_and(|l| l + 15.0 <= 85.0), "the smoothed share alone had room: {:?}", p.load);
        assert_eq!(p.window(), 150);
        let why = p.why().unwrap();
        assert!(why.ends_with("(the NAS a disk 85 % busy in its last reading, at the target 85 % or over)"), "{why}");
        let v = run(&mut p, &mut t, 12, 5.0, 150, Some(150));
        assert!(v.contains(&Verdict::Grew) && !v.contains(&Verdict::Cut), "{v:?}");
    }

    /// Task #1506's re-review: the mini the only Mac answering and running the pool's jobs, which
    /// move what they weigh. Five minutes at 100 in use at its best, the disks quiet (the window
    /// growing), then `to` in use as its answers go to 20 times their best, the disks at `busy`,
    /// jobs of 100 waiting: undecidable
    /// (its own jobs moving), and swelling past the knee against the readings at 100, so the
    /// window goes back to the knee, 100, within the eight readings the weight's rule took to cut,
    /// neither halved nor under it; it doesn't collapse, and nothing more is cut.
    fn lone_mover(to: u32, busy: f64) -> (Vec<Verdict>, Pace) {
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..120 {
            t += READ_EVERY_S;
            let (ms, in_use, disks) = if i < 30 { (2.0, 100, 10.0) } else { (40.0, to, busy) };
            v.push(sample(
                &mut p,
                t,
                &[ans("m6", ms, 2.0)],
                &[disks],
                in_use,
                &[("m6".to_string(), in_use)],
                Some(100),
            ));
        }
        (v, p)
    }

    #[test]
    fn the_lone_movers_answers_swelling_past_the_knee_cut_the_window_back_to_it() {
        for busy in [60.0, 84.0] {
            for (to, knee) in [(200, 100), (300, 100)] {
                let (v, p) = lone_mover(to, busy);
                let cut = v.iter().position(|v| *v == Verdict::Cut);
                assert!(cut.is_some_and(|i| (30..38).contains(&i)), "{to} at {busy} %: {v:?}");
                assert_eq!(v.iter().filter(|v| **v == Verdict::Cut).count(), 1, "{v:?}");
                assert_eq!(p.window(), knee, "{to} at {busy} %: back to the knee, not half the {to} in use");
                assert!(p.last.as_ref().unwrap().undecided);
            }
        }
        // (Its knee more than half what's in use: back to it all the same, not to half.)
        let (mut p, mut t) = (Pace::default(), 0);
        for i in 0..60 {
            t += READ_EVERY_S;
            let (ms, in_use) = if i < 30 { (2.0, 200) } else { (40.0, 300) };
            sample(
                &mut p,
                t,
                &[ans("m6", ms, 2.0)],
                &[if i < 30 { 10.0 } else { 60.0 }],
                in_use,
                &[("m6".to_string(), in_use)],
                Some(100),
            );
        }
        assert_eq!(p.window(), 200);
    }

    /// Task #1506's re-review: the counted Mac moving its own jobs (a step or more) while the
    /// others' use varies by a step, its answers as slow at both: against the others' use alone
    /// they'd read as not the pool's (as slow a step lighter) and the knee would lift; undecidable,
    /// it holds.
    #[test]
    fn the_counted_macs_own_jobs_keep_the_others_use_from_judging_its_answers() {
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..90 {
            t += READ_EVERY_S;
            let others = if (i / 6) % 2 == 0 { 0 } else { 60 };
            let moving = [("m6".to_string(), 100), ("m1".to_string(), others)];
            v.push(sample(&mut p, t, &[ans("m6", 40.0, 2.0)], &[30.0], 300, &moving, Some(100)));
        }
        assert!(!v[STARTED..].contains(&Verdict::Grew) && !v.contains(&Verdict::Cut), "{v:?}");
        let r = p.last.as_ref().unwrap();
        assert!(r.undecided && !r.outside, "{r:?}");
    }

    /// Task #1506's re-review: the knee undefined (no reading of the last `track_s` before the
    /// cadence had answers under the knee) is no reason not to cut. The mini alone answering and
    /// running the pool's jobs: five minutes at 100 in use at its best, the disks at 60 %, the
    /// window grown to 200; twelve minutes at 4 times its best, the weight unchanged; then the jobs
    /// fill to 200 and its answers go to 20 times: halved, within the eight readings the weight's
    /// rule took. And a knee read at the cadence's own readings would be one fast reading at the new
    /// weight: the knee is the heaviest weight before them, so back to 100 all the same.
    #[test]
    fn undecidable_answers_with_no_knee_halve_the_window() {
        let on_mini = |u: u32| [("m6".to_string(), u)];
        let (mut p, mut t) = (Pace::default(), 0);
        let mut v = Vec::new();
        for i in 0..140 {
            t += READ_EVERY_S;
            let (ms, in_use) = match i {
                0..30 => (2.0, 100),
                30..102 => (8.0, 100),
                _ => (40.0, 200),
            };
            v.push(sample(&mut p, t, &[ans("m6", ms, 2.0)], &[60.0], in_use, &on_mini(in_use), Some(100)));
        }
        assert!(v[..30].contains(&Verdict::Grew));
        let cut = v.iter().position(|v| *v == Verdict::Cut);
        assert!(cut.is_some_and(|i| (102..110).contains(&i)), "{v:?}");
        assert_eq!(p.window(), 100, "half the 200 in use");
        // (One fast reading at the new weight, then slow.)
        let (mut p, mut t) = (Pace::default(), 0);
        for i in 0..60 {
            t += READ_EVERY_S;
            let (ms, in_use) = match i {
                0..30 => (2.0, 100),
                30 => (2.0, 300),
                _ => (40.0, 300),
            };
            sample(
                &mut p,
                t,
                &[ans("m6", ms, 2.0)],
                &[if i < 30 { 10.0 } else { 60.0 }],
                in_use,
                &on_mini(in_use),
                Some(100),
            );
        }
        assert_eq!(p.window(), 100, "back to the knee before the cadence");
    }

    /// Task #1506's review: the jobs last started are held a span (their answers in the reading)
    /// where the weight they rose to is under the knee (answers known good there lately), and a
    /// growth cadence more at the knee or past it. Past the knee: a minute at 100 in use at its
    /// best, then 300: still held four readings after the rise. Under it: two minutes at 400 at
    /// its best, then 100, then 300: grown on, no longer held, four readings after the rise. A
    /// rise of under a step is held a span alone; one of a step in two rises a cadence more.
    #[test]
    fn the_jobs_started_are_judged_longer_past_the_knee_than_under_it() {
        let best = [ans("m1", 5.0, 5.0)];
        let held_after = |phases: &[(usize, u32)]| {
            let (mut p, mut t) = (Pace::default(), 0);
            for &(n, in_use) in phases {
                for _ in 0..n {
                    t += READ_EVERY_S;
                    disks(&mut p, t, &best, &QUIET, in_use, Some(150));
                }
            }
            p.last.as_ref().unwrap().verdict.hold()
        };
        let span = (SPAN_S / READ_EVERY_S) as usize;
        assert_eq!(held_after(&[(12, 100), (span + 1, 300)]), Some(Hold::Started), "past the knee");
        assert_ne!(held_after(&[(12, 400), (12, 100), (span + 1, 300)]), Some(Hold::Started), "under the knee");
        assert_eq!(held_after(&[(12, 400), (12, 100), (1, 300)]), Some(Hold::Started), "a span all the same");
        // (Past the knee, a rise of under a step (100 to 120): a span alone. A step in two rises
        // within a cadence (100 to 130 to 160): judged from where it first rose, a cadence more.)
        assert_ne!(held_after(&[(12, 100), (span + 1, 120)]), Some(Hold::Started), "under a step");
        assert_eq!(held_after(&[(12, 100), (1, 130), (span + 1, 160)]), Some(Hold::Started), "a step in two rises");
    }

    /// Task #1506's two readings of 19 Nov as cases, the jobs moving what they weigh. 18:51: the
    /// disks 3 % busy, the pool holding 3 thousandths, the answers 1.4 times their best and every
    /// third reading 40 times (the day's noise with the disks idle), jobs of 100 waiting, each
    /// adding a tenth to the disks: the window grows to take them, a job at a time, until the disks
    /// leave no room for one more within the target, never cut. 19:12 (the readings as recorded):
    /// the use risen from 3 to 54 thousandths, the answers from 1.4 to 18 to 21 times their best
    /// with it, the disks 49 to 80 % busy: cut once a growth cadence of readings a step over 3
    /// shows it; the same answers at a use that hasn't moved aren't judged the pool's doing.
    #[test]
    fn an_idle_nas_grows_the_window_and_one_slowing_with_the_load_cuts_it() {
        let (mut p, mut t, mut jobs, w) = (Pace::default(), 0, 0u32, 100);
        // (Ending on a reading under the knee: one past it holds whatever the disks.)
        for i in 0..361 {
            t += READ_EVERY_S;
            let ms = if i % 3 == 2 { 80.0 } else { 2.8 };
            let busy = 3.0 + f64::from(jobs * w) / 10.0;
            let v = disks(&mut p, t, &[ans("m6", ms, 2.0)], &[busy], 3 + jobs * w, Some(w));
            assert_ne!(v, Verdict::Cut, "{t}");
            let l = Limits { window: p.window(), alone: p.lone(), ..Default::default() };
            while l.paced(3 + jobs * w, w) {
                jobs += 1;
            }
        }
        assert_eq!(jobs, 8, "the disks at 83 %: no room for a ninth within 85 %");
        assert_eq!(p.last.as_ref().unwrap().verdict, Verdict::Held(Hold::Busy));
        // (19:12, as the lead read it: 3 thousandths held at 1.4 times, then the load risen
        // to 23, 42 and 54 thousandths over a minute and a half, the answers 7 to 50 times their
        // best, at 54 18 to 21 times, the disks 34 to 80 % busy.)
        let at_1912: [(u32, f64, f64); 15] = [
            (23, 9.63, 38.0),
            (23, 6.61, 34.0),
            (42, 9.63, 49.0),
            (42, 49.53, 57.0),
            (42, 37.02, 47.0),
            (42, 20.7, 49.0),
            (54, 18.02, 52.0),
            (54, 20.7, 77.0),
            (54, 18.02, 80.0),
            (54, 20.7, 79.0),
            (54, 18.02, 74.0),
            (54, 20.7, 77.0),
            (54, 18.02, 52.0),
            (54, 20.7, 61.0),
            (54, 18.02, 64.0),
        ];
        let replay = |p: &mut Pace, t: &mut u64| -> Vec<Verdict> {
            at_1912
                .iter()
                .map(|&(in_use, x, busy)| {
                    *t += READ_EVERY_S;
                    disks(p, *t, &[ans("m6", 2.0 * x, 2.0)], &[busy], in_use, Some(w))
                })
                .collect()
        };
        let (mut p, mut t) = (Pace::default(), 0);
        for _ in 0..30 {
            t += READ_EVERY_S;
            disks(&mut p, t, &[ans("m6", 2.8, 2.0)], &[6.0], 3, Some(w));
        }
        let v = replay(&mut p, &mut t);
        assert!(v.contains(&Verdict::Cut), "{v:?}");
        let cut = v.iter().position(|v| *v == Verdict::Cut).unwrap();
        assert_eq!(cut, 6 + JUDGED - 1, "54 the first use a step over 3: a growth cadence of it: {v:?}");
        assert_eq!(p.window(), STEP, "half the 54 in use, never under a step");
        // (The same answers at a load the pool held from the start: no lighter one to rise from.)
        let (mut q, mut t) = (Pace::default(), 0);
        let v: Vec<Verdict> = (0..15)
            .map(|_| {
                t += READ_EVERY_S;
                disks(&mut q, t, &[ans("m6", 40.0, 2.0)], &[70.0], 54, Some(w))
            })
            .collect();
        assert!(!v.contains(&Verdict::Cut), "no lighter load to rise from: {v:?}");
    }

    /// The disks over the owner's target cut the window; nothing read holds it; with the answers
    /// read it grows only when the disks are read and leave the next job room within the target.
    #[test]
    fn the_disks_over_the_target_cut_it_and_nothing_read_holds_it() {
        let mut p = Pace::default();
        let k = Tuning::default();
        let busy = |p: &mut Pace, t: u64, busy: Option<&[f64]>| {
            p.read(
                &Sample { t, answers: &[], blind: false, busy, pct: 85, in_use: 100, moving: &[], wants: Some(150) },
                &k,
            )
        };
        assert_eq!(busy(&mut p, 0, None), Verdict::Unread);
        assert!(
            matches!(busy(&mut p, 30, Some(&[40.0, 60.0])), Verdict::Held(_)),
            "within the target, but no answers: held"
        );
        assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NoAnswer));
        assert_eq!(p.load, Some(60.0));
        assert_eq!(busy(&mut p, 40, Some(&[95.0])), Verdict::Cut, "a reading over the target: cut at once");
        assert!(p.load.is_some_and(|l| l < 85.0), "the smoothed share not yet over: {:?}", p.load);
        let cut = (5..10).map(|k| busy(&mut p, k * 10, Some(&[95.0]))).filter(|v| *v == Verdict::Cut).count();
        assert_eq!(cut, 0, "cut once: its jobs out before the cut still run");
        assert_eq!(p.window(), 50, "half the 100 in use");
        assert!(p.why().unwrap().contains("% busy, over the target 85 %"), "{:?}", p.why());
        let w = p.window();
        assert_eq!(busy(&mut p, 100, None), Verdict::Unread);
        assert_eq!(p.window(), w);
        // (The answers at their best: the disks unread, held; 75 % busy, no room for a 150 job
        // within 85 %; 60 %, room.)
        let mut p = Pace::default();
        let best = [ans("m1", 5.0, 5.0)];
        let s = Sample {
            t: 0,
            answers: &best,
            blind: false,
            busy: None,
            pct: 85,
            in_use: 0,
            moving: &[],
            wants: Some(150),
        };
        assert_eq!(p.read(&s, &k), Verdict::Grew, "the disks unread: the answers alone say");
        p.load = Some(75.0);
        assert_eq!(disks(&mut p, 10, &best, &[75.0], 0, Some(150)), Verdict::Held(Hold::Busy));
        assert_eq!(disks(&mut p, 20, &best, &[75.0], 0, Some(60)), Verdict::Grew, "room for a lighter one");
        p.load = Some(60.0);
        assert_eq!(disks(&mut p, 30, &best, &[60.0], 0, Some(150)), Verdict::Grew);
    }

    /// The second by excess over the least floor counts once two or more are read: one Mac's own
    /// link (Wi-Fi, a move to Tailscale) never paces the pool alone; a NAS that's loaded adds its
    /// delay to all of them. A Mac whose calls miss their deadline is congestion, one at home
    /// whatever its rank; stale answers, or a Mac holding jobs giving none, never grow the window.
    #[test]
    fn the_second_slowest_mac_counts_and_stale_answers_never_grow_it() {
        let mut p = Pace::default();
        // (m6 four times its best on its own link, m1 at its best: m1's counts.)
        assert_eq!(read(&mut p, 0, &[ans("m1", 5.0, 5.0), ans("m6", 160.0, 40.0)], 0, Some(150)), Verdict::Grew);
        let c = p.last.as_ref().unwrap().counted.clone().unwrap();
        assert_eq!((c.mac.as_str(), c.ms, c.floor_ms), ("m1", 5.0, 5.0));
        // (Alone, a Mac's own answers count.)
        read(&mut p, 40, &[ans("m6", 160.0, 40.0)], 0, Some(150));
        let r = p.last.as_ref().unwrap();
        assert_eq!((r.counted.as_ref().unwrap().mac.as_str(), r.ratio), ("m6", Some(4.0)));
        // (Three: the second-slowest's, past the knee and rising from their best at no load.)
        let (mut q, mut t) = primed(&[ans("m1", 5.0, 5.0), ans("m4", 5.0, 5.0), ans("m6", 40.0, 40.0)]);
        let three = [ans("m1", 20.0, 5.0), ans("m4", 17.5, 5.0), ans("m6", 40.0, 40.0)];
        assert!(cut_last(&readings(&mut q, &mut t, JUDGED, &three, 300, None)));
        assert_eq!(q.last.as_ref().unwrap().counted.as_ref().unwrap().mac, "m4");
        // (R2: the NAS adding 16 ms to m1 at home (5 ms) and m6 away (40 ms): 4.2 times the least
        // floor's best on both, past the knee, where m6's own ratio would be 1.4.)
        let away = |ms, floor| Answer { home: false, ..ans("m6", ms, floor) };
        let (mut q, mut t) = primed(&[ans("m1", 5.0, 5.0), away(40.0, 40.0)]);
        assert!(cut_last(&readings(&mut q, &mut t, JUDGED, &[ans("m1", 21.0, 5.0), away(56.0, 40.0)], 300, None)));
        assert!((q.last.as_ref().unwrap().ratio.unwrap() - 4.2).abs() < 1e-9);
        // (m1 at home missing its deadlines, m6 away at its best: congested at once, m1 counted.)
        let mut q = Pace::default();
        let m1 = Answer { missed: true, ..ans("m1", 5.0, 5.0) };
        assert_eq!(read(&mut q, 0, &[m1, away(40.0, 40.0)], 300, None), Verdict::Cut);
        assert_eq!(q.last.as_ref().unwrap().counted.as_ref().unwrap().mac, "m1");
        // (Its calls missing their deadline: congested whatever its mean, said so.)
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
            busy: Some(&QUIET),
            pct: 85,
            in_use: 0,
            moving: &[],
            wants: Some(150),
        };
        assert!(matches!(q.read(&s, &Tuning::default()), Verdict::Held(_)));
        assert_eq!(q.last.as_ref().unwrap().verdict.hold(), Some(Hold::NotFresh));
        // (Stale, but past the knee and rising: cut all the same.)
        let mut t = 40;
        let slow = [Answer { fresh: false, ..ans("m1", 50.0, 5.0) }];
        assert!(cut_last(&readings(&mut q, &mut t, JUDGED, &slow, 300, None)));
    }

    /// A restarted lead takes its kept state up when it's younger than `restore_s`: the window,
    /// its threshold, its cut's hold-off and the readings the answers are judged on, never lifted
    /// to the jobs out; older, it starts again.
    #[test]
    fn a_restart_takes_the_kept_state_up() {
        let mut p = Pace::default();
        at(&mut p, 0, 5.0, 800, None);
        at(&mut p, 10, 5.0, 0, None);
        let mut t = 10;
        assert!(cut_last(&run(&mut p, &mut t, JUDGED, 40.0, 800, None)));
        assert_eq!(p.window(), 400);
        let kept: State = serde_json::from_value(serde_json::to_value(p.state(t)).unwrap()).unwrap();
        let mut readings = vec![(800, 1.0), (0, 1.0)];
        readings.extend([(800, 8.0); JUDGED]);
        assert_eq!(
            kept,
            State {
                at: t,
                window: 400.0,
                threshold: Some(400.0),
                cut_at: Some(t),
                settled_at: None,
                lone_at: Some(t),
                lone_n: 1,
                seen: p.st.seen.clone(),
            }
        );
        assert_eq!(kept.seen.iter().map(|s| (s.used.unwrap(), s.ratio)).collect::<Vec<_>>(), readings);
        let mut q = Pace::default();
        let mut u = t + 100;
        assert!(q.restore(&kept, u, RESTORE_S));
        // (Slow at the 800 cut from, against the answers kept at nothing: rising, decided once a
        // growth cadence of them is read: congested within the hold-off kept.)
        let v = run(&mut q, &mut u, JUDGED, 40.0, 800, None);
        assert_eq!(v[..JUDGED - 1], [Verdict::Held(Hold::Rising); JUDGED - 1], "{v:?}");
        assert_eq!(v.last(), Some(&Verdict::Held(Hold::HoldOff)), "{v:?}");
        assert_eq!(q.last.as_ref().unwrap().below, Some(1.0), "the answers at nothing, kept");
        assert_eq!(q.window(), 400, "not lifted to the 800 out");
        // (Taken up past a `track_s`: nothing kept to judge against; held at the knee.)
        let mut q = Pace::default();
        let mut u = t + TRACK_S + 100;
        assert!(q.restore(&kept, u, RESTORE_S));
        assert_eq!(run(&mut q, &mut u, 3, 40.0, 800, None), [Verdict::Held(Hold::Slow); 3]);
        let mut q = Pace::default();
        assert!(!q.restore(&kept, t + RESTORE_S, RESTORE_S));
        at(&mut q, t + RESTORE_S, 5.0, 800, None);
        assert_eq!(q.window(), 800, "too old: afresh");
    }

    /// R3: a congested reading holds a job beside nothing back for `lone_s` (an outside load near
    /// the cliff, one job taking it over), then lets one go; a spare reading ends the hold at
    /// once; holds in a row double, to `lone_most_s`.
    #[test]
    fn a_congested_reading_holds_a_lone_job_back_a_while() {
        let mut p = Pace::default();
        assert!(p.lone(), "nothing read: open");
        at(&mut p, 0, 5.0, 0, Some(150));
        assert!(p.lone());
        miss(&mut p, 10, 0, Some(150));
        assert!(!p.lone(), "congested with nothing out");
        let held = Limits { window: p.window(), alone: p.lone(), ..Default::default() };
        assert!(!held.paced(0, 150), "not beside nothing");
        assert!(Limits { alone: true, ..held }.paced(0, 150));
        miss(&mut p, 10 + LONE_S - 10, 0, Some(150));
        assert!(!p.lone(), "still held");
        miss(&mut p, 10 + LONE_S, 0, Some(150));
        assert!(p.lone(), "a `lone_s` on: one goes");
        miss(&mut p, 20 + LONE_S, 150, None);
        assert!(!p.lone(), "the job gone beside nothing, congested: held again");
        p.load = Some(82.0);
        disks(&mut p, 30 + LONE_S, &[ans("m1", 5.0, 5.0)], &[82.0], 150, None);
        assert!(!p.lone(), "the disks with no room for a step within its `lone_s`: still held");
        p.load = Some(2.0);
        at(&mut p, 40 + LONE_S, 5.0, 0, Some(150));
        assert!(p.lone(), "spare: the hold ended at once");
        // (An ordinary cut with nothing running after it: the NAS recovered, a heavy job beside
        // nothing goes at the next spare reading, not `lone_s` on.)
        let mut p = Pace::default();
        miss(&mut p, 0, 400, None);
        miss(&mut p, 10, 0, Some(900));
        assert!(!p.lone());
        at(&mut p, 20, 5.0, 0, Some(900));
        assert!(p.lone(), "recovered: open");
        assert!(Limits { window: p.window(), alone: p.lone(), ..Default::default() }.paced(0, 900));
        // (Holds in a row, never spare between: each twice as long, to 8 times.)
        let mut p = Pace::default();
        let mut t = 0;
        miss(&mut p, t, 0, Some(150));
        for hold in [1, 2, 4, 8, 8] {
            assert!(!p.lone());
            miss(&mut p, t + hold * LONE_S - 10, 0, Some(150));
            assert!(!p.lone(), "held {hold} times `lone_s`");
            miss(&mut p, t + hold * LONE_S, 0, Some(150));
            assert!(p.lone());
            t += hold * LONE_S + 10;
            miss(&mut p, t, 150, None);
        }
        // (The longest hold as set: 20 minutes, holds of 10, 20, 20.)
        let k = Tuning { lone_most_s: 2 * LONE_S, ..Tuning::default() };
        let read = |p: &mut Pace, t: u64, in_use: u32| {
            let answers = [Answer { missed: true, ..ans("m1", 5.0, 5.0) }];
            let s = Sample {
                t,
                answers: &answers,
                blind: false,
                busy: Some(&QUIET),
                pct: 85,
                in_use,
                moving: &[],
                wants: Some(150),
            };
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
        let k =
            Tuning::of(|n| (n == "nas-grow-seconds").then_some(600.0).or((n == "nas-track-minutes").then_some(2.0)));
        assert_eq!((k.grow_s, k.track_s), (600, 600), "the readings kept never fewer than a cadence's");
    }

    /// Each reading says why it held the window (the first gate it failed), and the lead's status
    /// line says the window, its threshold, what's in use and waits, the answers a step lighter,
    /// the verdict, why, and the lone way's hold.
    #[test]
    fn a_reading_says_why_it_held() {
        let k = Tuning::default();
        let hold = |p: &Pace| p.last.as_ref().unwrap().verdict.hold();
        let unread =
            |t| Sample { t, answers: &[], blind: false, busy: None, pct: 85, in_use: 0, moving: &[], wants: Some(150) };
        let mut p = Pace::default();
        assert_eq!(p.read(&unread(0), &k), Verdict::Unread);
        assert_eq!(hold(&p), None);
        assert!(matches!(at(&mut p, 10, 5.0, 0, None), Verdict::Held(_)));
        assert_eq!(hold(&p), Some(Hold::NotLimiting), "nothing waits, nothing in use");
        assert_eq!(at(&mut p, 20, 5.0, 0, Some(150)), Verdict::Grew);
        assert_eq!(hold(&p), None);
        let best = [ans("m1", 5.0, 5.0)];
        let (mut q, mut u) = (Pace::default(), 0);
        let v = run(&mut q, &mut u, 3, 40.0, 0, Some(150));
        assert_eq!(v.last(), Some(&Verdict::Held(Hold::Slow)), "a span past the knee, whatever brought it");
        p.load = Some(80.0);
        assert_eq!(disks(&mut p, 24, &best, &[80.0], 0, Some(150)), Verdict::Held(Hold::Busy));
        p.load = Some(2.0);
        let stale = Answer { fresh: false, ..ans("m1", 5.0, 5.0) };
        assert_eq!(read(&mut p, 26, &[stale], 0, Some(150)), Verdict::Held(Hold::NotFresh));
        assert_eq!(at(&mut p, 30, 5.0, 100, Some(150)), Verdict::Held(Hold::Started), "a job just started");
        let mut t = 30;
        assert_eq!(run(&mut p, &mut t, JUDGED, 10.0, 100, Some(150)).last(), Some(&Verdict::Held(Hold::Rising)));
        // (Up to 400: past the knee and rising once five in six of a cadence's answers are.)
        assert!(cut_last(&run(&mut p, &mut t, JUDGED - 1, 40.0, 400, Some(150))));
        assert_eq!(hold(&p), None);
        assert_eq!(
            at(&mut p, t + 10, 40.0, 400, Some(150)),
            Verdict::Held(Hold::HoldOff),
            "the jobs out before the cut"
        );
        let r = p.last.clone().unwrap();
        assert_eq!((r.threshold, r.wants), (Some(200), Some(150)));
        assert_eq!(r.lone_until, Some(t + LONE_S), "congested: a lone job held back");
        assert_eq!(
            r.line(p.window(), t + 70),
            "NAS pacing: window 6 % (threshold 20 %), 40 % in use, 40 % moving, the next job held back for it needs 15 %; \
             last reading 60 s ago, answers on m1's jobs' SMB session 8.0× their best (2.0× a step lighter), disks 2 % busy: held: congested, within \
             the last cut's hold-off; a job beside nothing held back 9 min more"
        );
        assert!(
            p.why().unwrap().ends_with("(the NAS answering m1 in 40 ms, 8.0× its best, rising with the pool's use)"),
            "{:?}",
            p.why()
        );
        // (Fallen, but still a step over those at 100: still rising, the cause kept.)
        read(&mut p, t + 20, &[Answer { fresh: false, ..ans("m1", 40.0, 5.0) }], 300, Some(150));
        assert_eq!(
            p.why().unwrap(),
            "NAS work paced to 6 % of the NAS, 30 % in use (the NAS answering m1 in 40 ms, 8.0× its best, \
             rising with the pool's use)"
        );
        // (Judged not the pool's doing: said so.)
        let r = Reading { outside: true, below: Some(20.0), used: None, ..p.last.clone().unwrap() };
        assert!(r.line(p.window(), t + 20).contains(" in use, the next job"), "nothing measured, nothing said");
        assert!(r.line(p.window(), t + 20).contains("SMB session 8.0× their best (not the pool's doing)"));
        // (Answers at their best a span on, at the same load: no longer past, so why says what
        // held it in place of a cause it can't see.)
        let mut u = t + 20;
        run(&mut p, &mut u, 3, 5.0, 300, Some(150));
        read(&mut p, u, &[Answer { fresh: false, ..ans("m1", 5.0, 5.0) }], 300, Some(150));
        assert!(p.why().unwrap().contains("(the NAS cut lately, not grown since: "), "{:?}", p.why());
    }

    /// The window never grows on readings with no Mac's answers (the disks alone, nothing listing
    /// the NAS), nor more than one job past the load its readings tested: the weight in use and the
    /// next job waiting, or under the threshold twice the weight in use where that's more.
    #[test]
    fn the_window_grows_only_on_answers_and_never_past_the_load_tested() {
        let k = Tuning::default();
        let disks_only = |p: &mut Pace, t: u64, in_use: u32| {
            let s = Sample {
                t,
                answers: &[],
                blind: false,
                busy: Some(&QUIET),
                pct: 85,
                in_use,
                moving: &[],
                wants: Some(300),
            };
            p.read(&s, &k)
        };
        // (Idle, the disks quiet, a 300 job waiting: held for want of answers.)
        let mut p = Pace::default();
        for t in (0..600).step_by(10) {
            assert!(matches!(disks_only(&mut p, t, 0), Verdict::Held(_)));
            assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NoAnswer));
        }
        assert_eq!(p.window(), STEP);
        // (Answers at their best, nothing in use: doubling to that one job, no further.)
        for t in (600..1200).step_by(10) {
            at(&mut p, t, 5.0, 0, Some(300));
            assert!(p.window() <= 300);
        }
        assert_eq!(p.window(), 300);
        assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NotLimiting));
        // (Cut at 400 in use: at the threshold, a step a minute to one job past the weight in use,
        // never past it.)
        let mut t = 1190;
        assert!(cut_last(&run(&mut p, &mut t, JUDGED, 40.0, 400, Some(300))));
        assert_eq!((p.window(), p.st.threshold), (200, Some(200.0)));
        for _ in 0..180 {
            run(&mut p, &mut t, 1, 5.0, 400, Some(300));
            assert!(p.window() <= 700);
        }
        assert_eq!(p.window(), 700, "400 in use and the 300 waiting");
        assert_eq!(p.last.as_ref().unwrap().verdict.hold(), Some(Hold::NotLimiting));
    }

    /// The re-review's probe: cut at 300 in use (window and threshold 150), one 150 job running
    /// and another waiting, the NAS at its best for two hours. Past the threshold the window
    /// grows a step a minute to one job past the weight in use, so the second goes.
    #[test]
    fn past_a_cut_the_window_grows_back_to_take_one_more_job() {
        let mut p = Pace::default();
        at(&mut p, 0, 5.0, 300, None);
        at(&mut p, 10, 5.0, 0, None);
        let mut t = 10;
        assert!(cut_last(&run(&mut p, &mut t, JUDGED, 40.0, 300, Some(150))));
        assert_eq!((p.window(), p.st.threshold), (150, Some(150.0)));
        while t < 2 * 3600 {
            run(&mut p, &mut t, 1, 5.0, 150, Some(150));
        }
        assert_eq!(p.window(), 300, "one 150 running and one more");
        let l = Limits { window: p.window(), alone: p.lone(), ..Default::default() };
        assert!(l.paced(150, 150), "the second 150 job goes");
    }

    /// The pacing's bounds are the pool's settings: each within its range, else its default.
    #[test]
    fn the_bounds_are_settings() {
        assert_eq!(Tuning::of(|_| None), Tuning::default());
        let set = [("nas-step", 20.0), ("nas-slow", 2.0), ("nas-floor-hours", 6.0), ("nas-read-seconds", 1.0)];
        let k = Tuning::of(|n| set.iter().find(|s| s.0 == n).map(|s| s.1));
        assert_eq!((k.step, k.slow, k.floor_s, k.read_s), (20, 2.0, 6 * 3600, READ_EVERY_S));
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
    /// `macs` (jobs dealt in turn). Each Mac lists the NAS every 5 s: its best, plus the NAS's
    /// added delay (`NAS_BEST_MS` times its ratio less one; `scaled`: its best times the NAS's
    /// ratio, the first version's model), by its own link and a 10 % noise of its own; a share
    /// `slip` of them answered at its best whatever the NAS's load (a listing the NAS serves from
    /// its memory while the rest queue); past the deadline (20 s, its calls missed) at the cliff;
    /// and, of its own, the SMB cache's answer now and then (`CACHE_HIT`), and its listings queued
    /// behind its own jobs' reads (`STALL`, `SPIKE`: as the mini's on 19 Nov). Its answer is the
    /// production's (`answer_of` its span's listings), against its own floor (the least such over
    /// `floor_s`), sent with each beat (every 10 s), or with `asks_only` only with its asks (every
    /// 120 s while it holds jobs: an older agent's); the disks read as busy as `disks` times its
    /// true load's share (None: not read); the lead restarted at each reading `restart` says (its
    /// state kept).
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
        slip: f64,
        disks: Option<f64>,
        k: Tuning,
    }

    /// A Mac's listing answered by its SMB cache (0.125 ms): its share of them.
    const CACHE_HIT: f64 = 0.07;
    /// A Mac's listing queued behind its own jobs' reads while it holds one: its share, and the
    /// time added (ms); and a long one's (`SPIKE`). Idle, a twentieth as many.
    const STALL: (f64, f64) = (0.35, 41.0);
    const SPIKE: (f64, f64) = (0.02, 450.0);

    /// The simulation's next uniform draw in [0, 1) from `seed`.
    fn draw(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 33) as f64 / (1u64 << 31) as f64
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
                slip: 0.0,
                disks: Some(1.0),
                k: Tuning::default(),
            }
        }

        /// Run paced (`pace`) or held to the whole.
        fn run(&self, pace: bool) -> Run {
            const TICK: u64 = 5;
            let k = &self.k;
            let mut p = Pace::default();
            let mut queue = self.n;
            // (Each job's end and Mac; each Mac's listings (t, ms, missed), its answers (t, ms) and
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
                    let noise = 0.9 + 0.2 * draw(&mut seed);
                    let missed = r >= CLIFF;
                    let r = if draw(&mut seed) < self.slip { 1.0 } else { r };
                    let ms = match (missed, self.scaled) {
                        (true, _) => 20_000.0,
                        (false, true) => m.best * (m.link)(t) * r * noise,
                        (false, false) => (m.best + NAS_BEST_MS * (r - 1.0)) * (m.link)(t) * noise,
                    };
                    let busy = if running.iter().any(|j| j.1 == i) { 1.0 } else { 0.05 };
                    let (c, x) = (draw(&mut seed), draw(&mut seed));
                    let ms = match () {
                        _ if missed => ms,
                        _ if c < CACHE_HIT => 0.125,
                        _ if x < SPIKE.0 * busy => ms + SPIKE.1,
                        _ if x < (SPIKE.0 + STALL.0) * busy => ms + STALL.1,
                        _ => ms,
                    };
                    probes[i].push_back((t, ms, missed));
                    while probes[i].front().is_some_and(|x| x.0 + k.span_s <= t) {
                        probes[i].pop_front();
                    }
                }
                if t % 10 == 0 {
                    for (i, m) in self.macs.iter().enumerate() {
                        let pr = &probes[i];
                        let Some(ms) = answer_of(pr.iter().map(|x| x.1), k.cached_ms) else { continue };
                        means[i].push_back((t, ms));
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
                                ms,
                                floor_ms: floor,
                                missed,
                                fresh: true,
                                apart_s: 0,
                                probe: None,
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
                        let disk = [(rho * 100.0 * self.disks.unwrap_or(0.0)).min(100.0)];
                        // (What each Mac's jobs move: their true load, as measured.)
                        let moving: Vec<(String, u32)> = (self.macs.iter().enumerate())
                            .map(|(i, m)| {
                                (m.name.into(), running.iter().filter(|j| j.1 == i).count() as u32 * self.real)
                            })
                            .collect();
                        let s = Sample {
                            t,
                            answers: &answers,
                            blind,
                            busy: self.disks.map(|_| &disk[..]),
                            pct: 85,
                            in_use,
                            moving: &moving,
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
    /// Mac slow on its own link; and (Q1) the NAS queueing most listings, a quick one slipping
    /// through now and then. Each Mac's listings carry its own cache's answers and its own jobs'
    /// queueing throughout (`Burst`). Each case's time past the knee and at the cliff is printed.
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
        // (While m6's link counts it out, m1's answers alone count: its own jobs' queueing, read
        // past the knee in a span now and then, cuts the window a few times; a quarter of an hour
        // more at most.)
        assert!(r.end <= paced.end + 900, "not throttled by one Mac's link: {} s against {} s", r.end, paced.end);
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
        // (Q1: the NAS queueing most listings under R2's outside load, a quick one slipping
        // through now and then (one in ten, its root folder in its memory): its answers read most
        // of them, and the window is cut, at the cliff half a minute at most.)
        let q1 = Burst { outside: ramp, slip: 0.1, ..Burst::of(40, 150, 250, 300) }.run(true);
        say("Q1 most listings queued, a quick one now and then", &q1);
        assert!(q1.cuts >= 1 && q1.cliff_s <= 30, "{q1:?}");
        assert!(q1.past_knee_s <= 3 * r.past_knee_s.max(300), "{q1:?} against R2's {r:?}");
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

    /// Task #1506's re-review: the burst's cases with the disks read at half the NAS's true load
    /// (a busy share under what it bears), and with them not read, the time each spends at the
    /// cliff printed, to set beside the weight's rule's on the same cases
    /// (tools/dev/nas-replay-against.py): the burst, its heavy jobs, H1's outside load, R2's added
    /// delay, Q1's queued listings, and the burst and R2 with one Mac answering.
    #[test]
    fn bursts_with_the_disks_read_low_or_not_at_all() {
        let ramp: fn(u64) -> u32 = |t| match t {
            1800..3600 => ((t - 1800) * 700 / 1800) as u32,
            3600..4800 => 700,
            _ => 0,
        };
        let h1: fn(u64) -> u32 = |t| if (1800..3600).contains(&t) { 700 } else { 0 };
        let one = || near_and_away().into_iter().take(1).collect::<Vec<_>>();
        for disks in [Some(0.5), None] {
            let cases = [
                ("paced", Burst::of(30, 150, 250, 300)),
                ("heavy", Burst::of(20, 150, 400, 300)),
                ("H1", Burst { outside: h1, ..Burst::of(40, 150, 250, 300) }),
                ("R2", Burst { outside: ramp, ..Burst::of(40, 150, 250, 300) }),
                ("Q1", Burst { outside: ramp, slip: 0.1, ..Burst::of(40, 150, 250, 300) }),
                ("one Mac paced", Burst { macs: one(), ..Burst::of(30, 150, 250, 300) }),
                ("one Mac R2", Burst { macs: one(), outside: ramp, ..Burst::of(40, 150, 250, 300) }),
            ];
            for (name, b) in cases {
                let r = Burst { disks, ..b }.run(true);
                eprintln!(
                    "the disks read {disks:?}, {name}: at the cliff {} s, past the knee {} s, {} cuts, ended {} s",
                    r.cliff_s, r.past_knee_s, r.cuts, r.end
                );
                assert!(r.end > 0);
            }
        }
    }

    /// A reading of the lead's, 19 Nov 12:22Z to 20 Nov 08:29Z (`nas_readings.csv`, its header
    /// says how they were taken): its time, the busiest disk's share (%), the weight the pool's
    /// jobs held and the window as recorded (thousandths), the counted ratio the lead read, and
    /// each Mac's (`MACS`) answer (ms; None: none) and its jobs' NAS use as measured (thousandths;
    /// None: not measured).
    struct Row {
        t: u64,
        disk: f64,
        weight: u32,
        window: u32,
        x: f64,
        macs: [(Option<f64>, Option<u32>); 3],
    }

    /// The Macs of `nas_readings.csv`, in its columns' order.
    const MACS: [&str; 3] = ["m1", "m4", "m6"];

    fn recorded() -> Vec<Row> {
        include_str!("nas_readings.csv")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| {
                let f: Vec<&str> = l.split(',').collect();
                let mac = |i: usize| (f[5 + 2 * i].parse().ok(), f[6 + 2 * i].parse().ok());
                Row {
                    t: f[0].parse().unwrap(),
                    disk: f[1].parse().unwrap(),
                    weight: f[2].parse().unwrap(),
                    window: f[3].parse().unwrap(),
                    x: f[4].parse().unwrap(),
                    macs: [mac(0), mac(1), mac(2)],
                }
            })
            .collect()
    }

    /// What the pool's jobs moved at `r`, summed (None: no Mac's measured).
    fn moved(r: &Row) -> Option<u32> {
        r.macs.iter().filter_map(|m| m.1).reduce(|a, b| a + b)
    }

    /// terrain-deep's jobs' times of 19 Nov, in the order they ended (`nas_19nov_jobs.txt`, s).
    fn jobs_19nov() -> Vec<u64> {
        include_str!("nas_19nov_jobs.txt").lines().filter(|l| !l.starts_with('#')).map(|l| l.parse().unwrap()).collect()
    }

    /// The disks' busy share a thousandth of the pool's measured NAS use adds over the readings
    /// (%): their least-squares slope, the use against the busiest disk.
    fn disk_per_use(rows: &[Row]) -> f64 {
        let pts: Vec<(f64, f64)> = rows.iter().filter_map(|r| Some((f64::from(moved(r)?), r.disk))).collect();
        let n = pts.len() as f64;
        let (mu, md) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n));
        let cov: f64 = pts.iter().map(|p| (p.0 - mu) * (p.1 - md)).sum();
        let var: f64 = pts.iter().map(|p| (p.0 - mu).powi(2)).sum();
        cov / var
    }

    /// What a replay came to: the window at each reading, when it was cut and why, the readings
    /// it grew on whose own answers were past the knee (`past_knee`) and of those the ones not
    /// judged the pool's or not (`unjudged`) and the ones the jobs let go had slowed past the knee
    /// on their own (`on_the_pool`: their slowing over the recorded's past `Tuning::slow`), the
    /// jobs it started, the readings with the disks at the target or more (`hot`), at the model's
    /// cliff and the most of them in a row, those the jobs let go slowed past the knee, the readings
    /// at the window's floor by the last cut's cause, and whether undecidable then, and the
    /// readings undecidable (by the cadence's) and those whose counted Mac's own jobs moved a step
    /// or more (by the reading's own).
    #[derive(Debug, Default)]
    struct Replayed {
        windows: Vec<(u64, u32)>,
        cuts: Vec<(u64, &'static str)>,
        past_knee: Vec<u64>,
        unjudged: Vec<u64>,
        on_the_pool: Vec<u64>,
        started: usize,
        hot: usize,
        hot_longest: usize,
        cliff: usize,
        longest: usize,
        slowed: usize,
        floor_by: std::collections::BTreeMap<String, usize>,
        undecided: usize,
        own_step: usize,
    }

    /// A disk share's tenth (0 to 10).
    fn disk_bin(pct: f64) -> usize {
        ((pct / 10.0).floor().max(0.0) as usize).min(10)
    }

    /// The NAS's answer to each Mac (`MACS`' order) at each tenth of the disks' busy share, without
    /// its own session's stalls (task #1552): the median of its answers recorded there while its
    /// own jobs moved under a step (ms; None: none recorded), as a probe apart reads the NAS's load and not the
    /// Mac's own traffic (on the mini, 20 Nov: its reads on its own ssh followed the disks' load,
    /// a rank correlation of 0.70 over 29 half minutes, while its listings on the jobs' session
    /// took a median of 73 ms in the seconds its jobs moved 5 MB/s or more, 1.7 ms in quiet ones).
    fn calm_ms(rows: &[Row]) -> [[Option<f64>; 11]; 3] {
        let mut by: [[Vec<f64>; 11]; 3] = Default::default();
        for r in rows {
            for (i, m) in r.macs.iter().enumerate() {
                if let (Some(ms), Some(o)) = (m.0, m.1) {
                    if o < STEP {
                        by[i][disk_bin(r.disk)].push(ms);
                    }
                }
            }
        }
        by.map(|b| b.map(|v| quartile(v, 2)))
    }

    /// Unix s of 19 Nov 2026 at `h:m` UTC (`h` past 23: 20 Nov's).
    fn nov19(h: u64, m: u64) -> u64 {
        1_795_046_400 + h * 3600 + m * 60
    }

    /// `t` as d h:mm:ss.
    fn hms(t: u64) -> String {
        format!("{} {}:{:02}:{:02}", 19 + (t - nov19(0, 0)) / 86400, t % 86400 / 3600, t % 3600 / 60, t % 60)
    }

    /// The pacing over the lead's readings (task #1506), closed on the jobs it lets go: beside what
    /// the pool held then (as recorded), a queue of held-back jobs weighing `w` each and moving it
    /// (the targets that waited), each running as long as terrain-deep's jobs of 19 Nov did, in
    /// turn (`jobs_19nov`), on the Mac `on` (one of `MACS`, its own jobs' use; another, a Mac not
    /// answering), as the pacing lets them (`Limits::paced`) and within its window (`Win::takes`:
    /// past the whole, the window's the bound); `admit` false, none goes (the readings as they
    /// were, a job of `w` waiting throughout). The jobs let go add to the recorded disks what a
    /// thousandth of measured use added over the readings (`disk_per_use`), and slow each Mac's
    /// recorded answers
    /// by the simulated NAS's curve at the disks' share with them over its slowing at the share
    /// without (`simulated`), so the noise is the readings' and the slowing past it the jobs'.
    /// Each Mac answering as recorded, its floor store::aimd::NAS_LEAST (the least of each over
    /// the readings: the header's), at home and fresh, missing its deadlines at the cliff or where
    /// the lead read a Mac missing them; `apart`, its answers taken apart from its jobs' session
    /// (task #1552, `Answer::apart_s`), and where a Mac's own jobs moved a step or more, its
    /// answer not its recorded one (its own session's stalls among it, which its probe apart
    /// doesn't have) but the NAS's at that disk load (`calm_ms`).
    fn replay(w: u32, admit: bool, on: &str, apart: bool) -> Replayed {
        let k = Tuning::default();
        let rows = recorded();
        let (per, times) = (disk_per_use(&rows), jobs_19nov());
        let calm = calm_ms(&rows);
        let floor = store::aimd::NAS_LEAST.as_secs_f64() * 1000.0;
        let mut p = Pace::default();
        let mut running: Vec<u64> = Vec::new();
        let mut out = Replayed::default();
        let (mut in_row, mut hot_row) = (0, 0);
        let mut cut_by = "never cut";
        for r in &rows {
            let t = r.t;
            running.retain(|&end| end > t);
            let added = running.len() as u32 * w;
            let busy = r.disk + per * f64::from(added);
            let slowing = simulated(busy / 100.0) / simulated(r.disk / 100.0);
            let missed = r.x >= RATIO_MOST || busy >= 100.0;
            out.cliff += usize::from(busy >= 100.0);
            in_row = if busy >= 100.0 { in_row + 1 } else { 0 };
            out.longest = out.longest.max(in_row);
            out.slowed += usize::from(slowing > k.slow);
            out.hot += usize::from(busy >= 85.0);
            hot_row = if busy >= 85.0 { hot_row + 1 } else { 0 };
            out.hot_longest = out.hot_longest.max(hot_row);
            let answers: Vec<Answer> = MACS
                .iter()
                .zip(&r.macs)
                .zip(&calm)
                .filter_map(|((mac, m), calm)| {
                    let ms = match m.1 {
                        Some(o) if apart && o >= STEP => calm[disk_bin(r.disk)].unwrap_or(m.0?),
                        _ => m.0?,
                    };
                    let x = (1.0 + (ms - floor).max(0.0) / floor) * slowing;
                    Some(Answer {
                        missed,
                        apart_s: if apart { APART_S } else { 0 },
                        ..ans(mac, floor * x.min(RATIO_MOST), floor)
                    })
                })
                .collect();
            let mut moving: Vec<(String, u32)> =
                MACS.iter().zip(&r.macs).filter_map(|(mac, m)| Some((mac.to_string(), m.1?))).collect();
            match moving.iter_mut().find(|m| m.0 == on) {
                Some(m) => m.1 += added,
                None if added > 0 => moving.push((on.to_string(), added)),
                None => {}
            }
            let in_use = r.weight + added;
            let s = Sample {
                t,
                answers: &answers,
                blind: false,
                busy: Some(&[busy]),
                pct: 85,
                in_use,
                moving: &moving,
                wants: Some(w),
            };
            match p.read(&s, &k) {
                Verdict::Cut => {
                    let why = match () {
                        _ if p.load.is_some_and(|l| l > 85.0) => "disks smoothed",
                        _ if busy >= 85.0 => "a disk reading",
                        _ if missed => "missed",
                        _ => "answers",
                    };
                    out.cuts.push((t, why));
                    cut_by = why;
                }
                Verdict::Grew => {
                    let l = p.last.as_ref().unwrap();
                    if missed || l.ratio.is_some_and(|x| x > k.slow) {
                        out.past_knee.push(t);
                        if !l.outside {
                            out.unjudged.push(t);
                        }
                        if slowing > k.slow {
                            out.on_the_pool.push(t);
                        }
                    }
                }
                _ => {}
            }
            out.windows.push((t, p.window()));
            let l = p.last.as_ref();
            out.undecided += usize::from(l.is_some_and(|l| l.undecided));
            out.own_step += usize::from(l.is_some_and(|l| l.own.is_some_and(|o| o >= k.step)));
            if p.window() <= STEP {
                let held = p.last.as_ref().is_some_and(|l| l.undecided);
                *out.floor_by
                    .entry(format!("{cut_by}{}", if held { ", held undecidable" } else { "" }))
                    .or_default() += 1;
            }
            let l = Limits { window: p.window(), small: k.small, alone: p.lone(), ..Default::default() };
            let held = |n: usize| r.weight + n as u32 * w;
            while admit && l.paced(held(running.len()), w) && l.win().takes(held(running.len()), w) {
                running.push(t + times[out.started % times.len()]);
                out.started += 1;
            }
        }
        out
    }

    /// Task #1506: the rule over the lead's own readings of 19 Nov 12:22Z to 20 Nov 08:29Z
    /// (closed on the jobs it lets go: `replay`). What it must do, each checked: on the readings as
    /// they were, never grow on a reading whose answers are past the knee unless judged not the
    /// pool's; cut at 19 Nov 19:12 (the use risen from tens of thousandths to 400 to 685, the
    /// answers 18 to 74 times their best with it, the disks 47 to 80 % busy); let jobs go while
    /// the answers were slow from outside the pool (at 20 Nov 00:06, 41 times their best, the pool
    /// moving nothing, the disks 3 % busy: no cut); and with jobs of 100 and 250 waiting, on a Mac
    /// not answering or on the mini (whose answers counted in most of the readings, its own jobs'
    /// then), never grow past the knee where the jobs let go slowed the answers past it on their
    /// own, and never hold the NAS at the model's cliff longer than a span's readings (its
    /// answers missing their deadlines within it, which cuts). Task #1552: with every Mac's
    /// answers taken apart (`replay`'s `apart`), none undecidable, the readings as they were at
    /// the window's floor no more often than with them on the jobs' session, and the mini's cases
    /// held to the same bounds. Printed:
    /// the window every half hour, the cuts and why, the jobs started, the growths past the knee,
    /// the readings at the target and at the cliff, those the jobs alone slowed past the knee, as
    /// `replay` has them, and the windows as recorded (#1417's rule to 19 Nov 23:06Z,
    /// 20261119-2306's after).
    #[test]
    fn the_readings_of_19_and_20_nov_let_work_go_and_cut_at_the_pools_load() {
        let rows = recorded();
        eprintln!("the disks a thousandth of measured use adds: {:.3} %", disk_per_use(&rows));
        let say = |name: &str, r: &Replayed| {
            let at = |t: u64| r.windows.iter().rev().find(|x| x.0 <= t).map_or(0, |x| x.1);
            let line: Vec<String> = (nov19(12, 30) / 1800..=nov19(32, 0) / 1800)
                .map(|i| format!("{} {}", &hms(i * 1800)[..7], at(i * 1800)))
                .collect();
            let cuts: Vec<String> = r.cuts.iter().map(|(t, w)| format!("{} {w}", hms(*t))).collect();
            let mut v: Vec<u32> = r.windows.iter().map(|x| x.1).collect();
            v.sort_unstable();
            eprintln!(
                "{name}: started {}, median window {}, at its floor {} readings; grown past the knee {} \
                 ({} not judged, {} the jobs' doing); the disks at 85 % or more {} readings ({} in a row at most), \
                 at the cliff {} ({} in a row at most); the jobs alone past the knee {}",
                r.started,
                v[v.len() / 2],
                r.windows.iter().filter(|x| x.1 <= STEP).count(),
                r.past_knee.len(),
                r.unjudged.len(),
                r.on_the_pool.len(),
                r.hot,
                r.hot_longest,
                r.cliff,
                r.longest,
                r.slowed,
            );
            eprintln!("  at the floor by why: {:?}", r.floor_by);
            eprintln!(
                "  undecidable {} readings (a cadence's), the counted Mac's own jobs a step or more in {}",
                r.undecided, r.own_step
            );
            eprintln!("  window: {}", line.join(", "));
            eprintln!("  cuts ({}): {}", cuts.len(), cuts.join(", "));
        };
        let mut rec: Vec<u32> = rows.iter().map(|r| r.window).collect();
        rec.sort_unstable();
        eprintln!(
            "as recorded: median window {}, at its floor {} readings of {}",
            rec[rec.len() / 2],
            rows.iter().filter(|r| r.window <= STEP).count(),
            rows.len()
        );
        let r = replay(STEP, false, "jobs", false);
        say("the readings as they were", &r);
        let late: Vec<u32> = r.windows.iter().filter(|x| x.0 >= nov19(32, 15)).map(|x| x.1).collect();
        let cuts = r.cuts.iter().filter(|c| c.0 >= nov19(32, 15)).count();
        eprintln!(
            "  20 Nov 08:15-08:29 (task #1506's case): the window {} to {}, {cuts} cuts",
            late.iter().min().unwrap(),
            late.iter().max().unwrap()
        );
        assert_eq!(r.unjudged.iter().map(|t| hms(*t)).collect::<Vec<_>>(), Vec::<String>::new());
        assert!(
            r.cuts.iter().any(|c| (nov19(19, 10)..nov19(19, 15)).contains(&c.0)),
            "the pool's load at 19:12 cut: {:?}",
            r.cuts.iter().map(|c| hms(c.0)).collect::<Vec<_>>()
        );
        assert!(!r.cuts.iter().any(|c| (nov19(24, 5)..nov19(24, 8)).contains(&c.0)), "{:?}", r.cuts);
        // (Task #1552: the answers taken apart from the jobs' session.)
        let a = replay(STEP, false, "jobs", true);
        say("the readings as they were, the answers apart", &a);
        let floor = |r: &Replayed| r.windows.iter().filter(|x| x.1 <= STEP).count();
        assert_eq!(a.undecided, 0, "every Mac's answers apart: none undecidable");
        assert!(floor(&a) <= floor(&r), "at the floor {} readings, against {}", floor(&a), floor(&r));
        for (w, on, apart) in [
            (100, "jobs", false),
            (250, "jobs", false),
            (100, "m6", false),
            (250, "m6", false),
            (100, "m6", true),
            (250, "m6", true),
        ] {
            let r = replay(w, true, on, apart);
            let name = format!("jobs of {w} on {on}{}", if apart { ", the answers apart" } else { "" });
            say(&name, &r);
            assert!(r.started > 0);
            assert!(!apart || r.undecided == 0, "{name}: {} readings undecidable", r.undecided);
            assert_eq!(r.on_the_pool.iter().map(|t| hms(*t)).collect::<Vec<_>>(), Vec::<String>::new(), "{name}");
            let span = (SPAN_S / READ_EVERY_S) as usize;
            assert!(r.longest <= span, "{name}: {} readings in a row at the cliff", r.longest);
            // (At the cliff no more often than the weight's rule (6 readings in each row): with
            // the pool's swelling answers grown on, jobs of 250 on the mini reach it at 8.)
            assert!(r.cliff <= 6, "{name}: {} readings at the cliff", r.cliff);
        }
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
