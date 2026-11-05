//! What a job of a step needs (task #236): one model per step, read by every placement path (the
//! coordinator's picks, its floors, the forecast). Nothing here is a figure kept by hand: a need is
//! measured (the target's own run, its floor), fitted over the step's measured runs, the step's
//! prior from its measured runs, or the caller's first guess (the steps table's, the size its plan
//! offered, a z6 tile's extract-size guess).
//!
//! Only what was measured the way the step runs now counts (crate::coord::cost_version): a run or a
//! floor of an older way of it (a z6 tile's before its v2, whose measures held malloc's cached freed
//! blocks) is neither a measure, nor a fit's run, nor part of a prior.
//!
//! - **Measured**: the target's own run, never below its floor (crate::agent::memguard::Floor); a
//!   floor alone counts as its measure.
//! - **Near** (memory, a tile target; task #931): the most its nearest measured targets of the step
//!   took (`Near`: `NEAR_RUNS` within `NEAR_RINGS` rings of tiles), × `MARGIN`, where that's judged
//!   good on the step's own measures (each predicted from the others) and erred less than its best
//!   good fit over a size the target has. What a target holds is its place's, whichever Mac runs it
//!   (terrain-deep's neighbouring tiles differ by a median 6–8 % run on two Macs, no Mac's the
//!   higher by more than 2 %, and 2 % run on one, measured on the lead's runs of 15 Oct – 4 Nov).
//! - **Fit**: base + k × size (+ c × threads when the runs differ in threads), fitted per step over
//!   its measured runs by least squares and raised to cover every one of them (an upper envelope),
//!   times `MARGIN`, capped at the most a run it was made from measured (a target past the sizes it
//!   saw isn't extrapolated past what's known: its floor, learned when it runs, holds it after), never
//!   below the target's floor. A size is one of the run's input measures (`sizes`, crate::costin) or
//!   the target tile's area (km², from its key). Each size is tried and judged on runs held out of
//!   its fit (`FOLDS`-fold): one that misses more than `MAX_MISS` of them, or is off by more than
//!   `MAX_ERR` on the median one, is poor and not used. The best of the good ones is used, when the
//!   target has that size (its own earlier run's, or its area).
//! - **Guess first** (z6 tiles and landmark candidates, `GUESS_FIRST`): with no good fit, the caller's
//!   guess scaled by the target's extract comes before the step's prior, so one dense z6 tile doesn't set
//!   every other's need.
//! - **Prior**: memory and disk, the step's targets' measures' 90th percentile (memory: their
//!   floors among them), so one outlier doesn't hold every other target to its figure, and a heavy
//!   target's own floor holds it; time, the mean of its targets' runs: the expected time, what a
//!   job's targets are counted to the batch length by and the forecast sums (task #433: by the 90th
//!   percentile a z6 tile was 1151 s against a mean of 363, so every `unit` job carried one z6 tile and
//!   paid the job's setup for it alone; depth 265 against 131).
//! - **Table**: the caller's guess, when nothing of the step was measured.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::agent::memguard::{step_of_key, Floor};
use crate::coord::{cost_key, cost_version, Cost};

/// The safety margin over a fit's envelope.
pub const MARGIN: f64 = 1.1;
/// The fewest measured runs a step's fit is made from.
pub const MIN_RUNS: usize = 10;
/// The held-out folds a fit is judged on.
pub const FOLDS: usize = 5;
/// The share of held-out runs a fit may predict under before it's poor.
pub const MAX_MISS: f64 = 0.05;
/// The median held-out error (|predicted − measured| / measured) past which a fit is poor.
pub const MAX_ERR: f64 = 0.5;
/// The steps whose caller's guess scales with the target (a z6 tile's extract; candidates read the
/// same extract) and so comes before the step's prior while no fit is good.
pub const GUESS_FIRST: [&str; 2] = ["unit", "pois"];

/// The steps whose job opens one halo of base packs for all its targets (task #260: `pack`, a
/// z6 tile's map tiles together; task #381: `lo`, a pass of the low zooms, its z3 tiles' owners'): the
/// parts of the base packs its targets need are read from the NAS once, into the job's scratch
/// (scenic-build's `open_for_tiles`, task #396), and every target reads them there. So a target's
/// disk measure is the job's reads, shared, not its own: a job of several takes the most of theirs,
/// not their sum (`batch_disk`); and a target's `bytes` (the halo's base-pack files' bytes, its
/// input size for the fits) aren't what it reads from the NAS: its NAS weight is what the job read
/// (`nas_read` over `nas_secs`, the job's reads and their time, on each of its targets) and what it writes (`wrote`, `creates`).
pub const HALO_STEPS: [&str; 2] = ["pack", "lo"];

/// The steps whose time is the work a target finds to do, not the target itself (task #830), with
/// the measure (crate::costin) that counts it: a pyramids-fill z6 tile whose tiles were all stored
/// already took 17–37 s, one fetching 170,000 of them 4,950 s, and five such no-op runs made the
/// step's time 30 s a z6 tile (a fill running 3 h shown as "of ≈ 28 s"). Its runs count in its step's
/// time weighted by that work (`time_weight`); a run that did none, or doesn't say, counts for
/// nothing there (its own target keeps its own time).
pub const PER_WORK: [(&str, &str); 1] = [("pyramids-fill", "fetched")];

/// The measure (crate::costin) a "none" target's run notes, the packs it dropped (crate::agent::build's
/// none keys: a z10 tile no deep target any more, its packs still named): such a run reads nothing
/// and takes a second or two where its step's builds take minutes and gigabytes. It counts for
/// nothing in its step's model (`runs`, `prior`, `time_weight`); its step's drops are modelled
/// apart (`Book::drop_need`): what a drop not yet run takes (crate::coord::sched, the forecast).
pub const DROPS: &str = "drops";

/// Whether run `c` was a drop (`DROPS`).
pub fn is_drop(c: &Cost) -> bool {
    c.sizes.contains_key(DROPS)
}

/// What run `c` of `step` weighs in its step's time (its prior, its fits, `secs_p90`, the agent's
/// per-target mean): 1, or a `PER_WORK` step's work done (None: none done, or not said).
pub fn time_weight(step: &str, c: &Cost) -> Option<f64> {
    if is_drop(c) {
        return None;
    }
    match PER_WORK.iter().find(|p| p.0 == step) {
        None => Some(1.0),
        Some((_, k)) => c.sizes.get(*k).filter(|&&n| n > 0).map(|&n| n as f64),
    }
}

/// The mean of times weighed (`time_weight`), rounded up; None with none.
pub fn weighted_mean(v: &[(u64, f64)]) -> Option<u64> {
    let w: f64 = v.iter().map(|p| p.1).sum();
    (w > 0.0).then(|| (v.iter().map(|&(s, w)| s as f64 * w).sum::<f64>() / w).ceil() as u64)
}

/// The disk (MB) a job of `step` takes over targets each measured to take `each`: a `HALO_STEPS`
/// step's the most of them (one cache, shared), another's their sum (each its own).
pub fn batch_disk(step: &str, each: impl IntoIterator<Item = u64>) -> u64 {
    if HALO_STEPS.contains(&step) {
        each.into_iter().max().unwrap_or(0)
    } else {
        each.into_iter().sum()
    }
}

/// The caches a job reads that every job of their steps on one Mac shares (task #392), each with
/// its steps. The run that fills one
/// measures the cache whole; the runs after it, warm, only their own output. So a target's disk
/// need is its own output (`Book::disk`: a measure past its step's 90th percentile taken for a
/// fill), and the cache is reserved once per Mac, not once per job (crate::coord's grants:
/// `cache_mb`). (`pack` and `lo` read only the parts of the base packs they need, into their own
/// scratch, since Task #396: their disk is their own.) The one shared cache: the whole local copies
/// of the basemap and the depth sources' packs the depth's and the water's steps read
/// (crate::localread, task #551), copied only where the Mac has room for it past its room target,
/// else read in place: reserved only on a Mac with that room (crate::coord's `missing`).
pub const CACHES: [(&str, &[&str]); 1] =
    [(crate::localread::CACHE, &["depth", "depth-lo", "terrain-water", "lakefill", "water"])];

/// The shared cache a job of `step` reads (`CACHES`), if any.
pub fn cache_of(step: &str) -> Option<&'static str> {
    CACHES.iter().find(|(_, steps)| steps.contains(&step)).map(|(c, _)| *c)
}

/// Where a need came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    Measured,
    /// Its nearest measured targets' (`Near`).
    Near,
    Fit,
    /// The caller's guess for a `GUESS_FIRST` step that has measures (so not a cold start).
    Guess,
    Prior,
    /// The caller's guess, nothing of the step measured: a cold start.
    Table,
}

/// What a job's input and output go over: the network (a fetch from outside), or the NAS (the
/// rest), by its step's kind in the steps table (crate::agent::steps::Kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Io {
    Net,
    Nas,
}

/// What a job of a step for a target needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Need {
    /// Peak memory (MB), and where it came from.
    pub mem_mb: u64,
    pub mem_from: From,
    /// Disk (MB; None: none of its step measured).
    pub disk_mb: Option<u64>,
    /// Wall time (s; None: none of its step measured).
    pub secs: Option<u64>,
    /// The cores it takes: the threads its job is given (0: as many as the Mac has). The steps
    /// table has no count of its own per step.
    pub cores: u32,
    pub io: Io,
}

/// What's measured and fitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Measure {
    Mem,
    Secs,
    Disk,
}

impl Measure {
    pub const ALL: [Measure; 3] = [Measure::Mem, Measure::Secs, Measure::Disk];

    pub fn name(self) -> &'static str {
        match self {
            Measure::Mem => "memory",
            Measure::Secs => "time",
            Measure::Disk => "disk",
        }
    }
}

/// A step's fit of one measure over one size.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub size: String,
    pub base: f64,
    pub k: f64,
    /// Per thread, when the runs differed in threads.
    pub per_thread: Option<f64>,
    /// The most a run it was made from measured: its cap.
    pub top: f64,
    /// The runs it's made from, and on those held out: the share predicted under, the median error.
    pub runs: usize,
    pub miss: f64,
    pub err: f64,
}

impl Fit {
    pub fn good(&self) -> bool {
        self.runs >= MIN_RUNS && self.miss <= MAX_MISS && self.err <= MAX_ERR
    }

    fn at(&self, x: f64, threads: Option<f64>) -> f64 {
        ((self.base + self.k * x + self.per_thread.zip(threads).map_or(0.0, |(c, t)| c * t)) * MARGIN)
            .clamp(0.0, self.top)
    }
}

/// A step's model of one measure, as made from its runs: its fits (best first), its prior, and
/// (memory alone) its measures by place.
#[derive(Clone, Debug, Default)]
struct Model {
    fits: Vec<Fit>,
    prior: Option<u64>,
    near: Option<std::sync::Arc<Near>>,
    /// (Time alone.) The step's runs by their source's time (`BySource`), and the Macs that have
    /// runs of it (`Book::secs_for`).
    by_source: Option<std::sync::Arc<BySource>>,
    macs: std::sync::Arc<std::collections::BTreeSet<String>>,
}

/// A step's times by what its target's source took (task #927): the source of a target is the same
/// target of a step it reads (the first of its steps table's reads naming it, `source_secs`: slope-deep's
/// terrain-deep, slope-hi's terrain-hi), whose run ended before its own could start. Its time there
/// says what the target holds: on the lead's runs to 5 Nov, a slope-deep target whose terrain-deep
/// took under 2 s took a median 0 s and a mean 1, one whose terrain-deep took 64-127 s a median 57
/// and a mean 79 (the log of one's time against the other's: correlation 0.92), where the step's
/// mean, 25 s, sized a batch of 30 empty targets as 12 minutes and one of 30 full ones as 12 too.
/// Its runs are grouped by their source's time at the lead's pace, in doubling bins (`bin`); each
/// bin's runs' times summed, all Macs' at the lead's pace (`all`) and each Mac's as they ran on it
/// (`own`).
#[derive(Debug, Default)]
pub struct BySource {
    all: BTreeMap<u32, (f64, usize)>,
    own: BTreeMap<(String, u32), (f64, usize)>,
}

/// The doubling bin a source's time `secs` falls in: 0 for under a second, 1 under 3 s, 2 under 7,
/// and so on.
fn bin(secs: f64) -> u32 {
    (1.0 + secs.max(0.0)).log2() as u32
}

/// The mean of a bin's runs where it has `MIN_RUNS` (a fit's fewest).
fn bin_mean(b: Option<&(f64, usize)>) -> Option<f64> {
    b.filter(|b| b.1 >= MIN_RUNS).map(|b| b.0 / b.1 as f64)
}

/// The nearest measured targets a target's memory is read from (`Near`): the rings of tiles
/// around it are taken outward until they hold this many, or `NEAR_RINGS` are taken.
pub const NEAR_RUNS: usize = 5;
/// The most rings of tiles around a target its nearest measured targets are looked for in.
/// (`NEAR_RUNS` and this were chosen on the lead's terrain-deep runs of 15 Oct – 4 Nov, each target
/// predicted from those done before its grant's hour (`backtest_mem_on_the_leads_measures` as it
/// first replayed, an hour at a time): 1 target
/// within 4 rings missed 9.3 % of the targets by any amount and 3.3 % by over a GB, 5 within 8 4.8
/// % and 1.9 %, 8 within 8 4.1 % and 1.5 % but reserved 1.28× their memory where 5 reserved 1.25×;
/// the step's prior then: 19.2 %, 5.8 % and 1.52×.)
pub const NEAR_RINGS: i64 = 8;

/// A step's memory by place (task #931): each tile target measured the way the step runs now (its
/// run's peak, or its floor, the larger) by its tile. A target not measured takes the most of its
/// nearest measured ones (`NEAR_RUNS` within `NEAR_RINGS` rings of tiles at its zoom) × `MARGIN`:
/// what a target holds is its place's (the sources it reads there), so its neighbours' runs say
/// it where the step's prior can't (terrain-deep's: 3 GB where no fine dataset is dense, past 9
/// GB over dense lidar; its prior, 7.8 GB, reserved twice a sparse target's memory and gave a dense
/// one back). Judged on its own measures as a fit is (each predicted from the others, leave one out):
/// used where good (`good`), and better than the step's best good fit (`Book::mem`).
#[derive(Debug, Default)]
pub struct Near {
    at: std::collections::HashMap<(u8, u32, u32), u64>,
    /// Its measures predicted, and of those: the share predicted under, the median error.
    pub runs: usize,
    pub miss: f64,
    pub err: f64,
    /// Each target's prediction as worked out (`of`): the picks weigh every target offered.
    memo: Mutex<std::collections::HashMap<(u8, u32, u32), Option<u64>>>,
}

impl Near {
    /// Of the measures `at` (tile → MB), judged.
    fn of_measures(at: std::collections::HashMap<(u8, u32, u32), u64>) -> Near {
        let mut n = Near { at, ..Default::default() };
        let (mut errs, mut miss) = (Vec::new(), 0usize);
        // (Each from the measured tiles before it, row by row (north first, then west), as a plan
        // reaches a place from one side: its neighbours all round would flatter it.)
        for (&t, &y) in &n.at {
            let Some(p) = n.nearest_by(t, |dx, dy| dy < 0 || (dy == 0 && dx < 0)) else { continue };
            if p < y as f64 {
                miss += 1;
            }
            errs.push((p - y as f64).abs() / (y as f64).max(1.0));
        }
        errs.sort_by(f64::total_cmp);
        n.runs = errs.len();
        n.miss = if errs.is_empty() { 0.0 } else { miss as f64 / errs.len() as f64 };
        n.err = errs.get(errs.len() / 2).copied().unwrap_or(f64::INFINITY);
        n
    }

    pub fn good(&self) -> bool {
        self.runs >= MIN_RUNS && self.miss <= MAX_MISS && self.err <= MAX_ERR
    }

    /// The most measured among tile `t`'s nearest measured tiles but itself, × `MARGIN` (MB);
    /// None with none within `NEAR_RINGS`. Columns wrap around the antimeridian, each tile counted
    /// once however few columns its zoom has.
    fn nearest(&self, t: (u8, u32, u32)) -> Option<f64> {
        self.nearest_by(t, |_, _| true)
    }

    /// `nearest`, of the tiles at the offsets `keep` keeps alone.
    fn nearest_by(&self, t: (u8, u32, u32), keep: impl Fn(i64, i64) -> bool) -> Option<f64> {
        let (z, x, y) = t;
        let n = 1i64 << z.min(30);
        // (A column offset counts in (-n/2, n/2] alone: at a low zoom a ring's far columns wrap
        // onto nearer ones, or the tile's own (task #931 review: at z3, ring 8's did, and judged a
        // measure by its own value).)
        let at = |dx: i64, dy: i64| {
            let (a, b) = ((x as i64 + dx).rem_euclid(n), y as i64 + dy);
            (-n < 2 * dx && 2 * dx <= n && (0..n).contains(&b))
                .then(|| self.at.get(&(z, a as u32, b as u32)).copied())
                .flatten()
        };
        let (mut got, mut most) = (0usize, 0u64);
        for r in 1..=NEAR_RINGS.min(n) {
            // (Ring r: its top and bottom rows, then its sides between.)
            let ring = (-r..=r).flat_map(|d| [(d, -r), (d, r)]).chain((-r + 1..r).flat_map(|d| [(-r, d), (r, d)]));
            for m in ring.filter(|&(dx, dy)| keep(dx, dy)).filter_map(|(dx, dy)| at(dx, dy)) {
                got += 1;
                most = most.max(m);
            }
            if got >= NEAR_RUNS {
                break;
            }
        }
        (got > 0).then(|| most as f64 * MARGIN)
    }

    /// `key`'s prediction (MB): its tile's `nearest`; None for a key that isn't a tile's, or with
    /// none near.
    fn of(&self, key: &str) -> Option<u64> {
        let t = tile(key)?;
        let mut g = self.memo.lock().unwrap_or_else(|e| e.into_inner());
        *g.entry(t).or_insert_with(|| self.nearest(t).map(|p| p.ceil() as u64))
    }
}

/// A job's cores as its runs measured them (`Book::cores`): the threads it is granted (`grant`),
/// and those it keeps busy (`busy_tenths`, in tenths of a core: its CPU time over the time it ran
/// (its wall time less the time it was stopped, `paused_ms`; task #697), to the nearest tenth, a
/// tenth at least, never past `grant`; None where no run said its CPU time: it counts for its
/// threads), what its
/// Mac's packing counts it for (crate::coord::sched, task #688: a job
/// waiting on its reads holds its threads but not its cores, so memory decides how many go
/// together).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cores {
    pub grant: u32,
    pub busy_tenths: Option<u32>,
}

/// The fewest runs at a thread count that make it a point of its step's curve (`Curve`).
pub const CURVE_RUNS: usize = 5;
/// One target in this many explores a neighbouring thread count its step's curve lacks (`Curve::pick`).
pub const EXPLORE_EVERY: u64 = 10;
/// The standard errors by which one point's wall must beat another's to count as faster.
pub const CURVE_SE: f64 = 2.0;

/// A thread count's runs on its step's curve: how many, their median wall time per unit of work
/// (`Curve::size`), that median's standard error (from their spread: 1.4826 × the median absolute
/// deviation over √runs), and the most cores one kept busy (tenths; None where none said its CPU
/// time).
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub threads: u32,
    pub runs: usize,
    pub wall: f64,
    pub se: f64,
    pub busy_tenths: Option<u32>,
}

/// A step's wall time by threads (task #904), from its runs the way it runs now (the costs book:
/// each target's last run, its threads and its wall time less the time stopped), each run's wall
/// over its target's size: what the step's good time fit predicts for it without its thread term
/// (`size` names that fit's size; no good fit, the MB its runs read, where each says some, else
/// None: the wall alone). Points ascend by threads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    pub size: Option<String>,
    pub points: Vec<Point>,
    /// Its waits dominate: the median of its runs' share of time waited (`waited_ms`) is half or
    /// more. Taken over the step's runs, not one target's last, so a single run waiting a little
    /// more or less doesn't switch a job between the curve and the climb (task #904 review).
    pub waits: bool,
}

impl Curve {
    /// The points made of `CURVE_RUNS` runs or more.
    fn known(&self) -> impl Iterator<Item = &Point> {
        self.points.iter().filter(|p| p.runs >= CURVE_RUNS)
    }

    fn at(&self, threads: u32) -> Option<&Point> {
        self.points.iter().find(|p| p.threads == threads)
    }

    /// The knee: the fewest threads that no more threads beat, beating meaning a wall lower by
    /// `CURVE_SE` standard errors of the difference (the runs' own spread, no set margin); None
    /// while under two thread counts are known.
    pub fn knee(&self) -> Option<u32> {
        let k: Vec<&Point> = self.known().collect();
        if k.len() < 2 {
            return None;
        }
        let beats = |q: &Point, p: &Point| p.wall - q.wall > CURVE_SE * (p.se * p.se + q.se * q.se).sqrt();
        k.iter().find(|p| !k.iter().any(|q| q.threads > p.threads && beats(q, p))).map(|p| p.threads)
    }

    /// The threads a job for `target` is given about `at` (the knee, else what its busy cores
    /// grant): one target in `EXPLORE_EVERY` (by a hash of its name, so its grant holds) runs a
    /// neighbour of `at` that has under `CURVE_RUNS` runs, the one above first, once `at` itself is
    /// known (`CURVE_RUNS` runs: a step new to the pool runs its grant first); so the curve is
    /// learned around its knee, and the exploring stops once both neighbours are known.
    pub fn pick(&self, at: u32, target: &str) -> u32 {
        // (FNV-1a: stable across runs and builds, unlike std's hasher.)
        let h = target.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        if h % EXPLORE_EVERY != 0 || self.at(at).is_none_or(|p| p.runs < CURVE_RUNS) {
            return at;
        }
        [at + 1, at.saturating_sub(1)]
            .into_iter()
            .find(|&n| n >= 1 && self.at(n).is_none_or(|p| p.runs < CURVE_RUNS))
            .unwrap_or(at)
    }

    /// The cores granted a job for `target` that doesn't climb, `c` as its busy cores grant it:
    /// the knee's threads (`c`'s grant while under two counts are known), explored by `pick`;
    /// counted for the most cores a run kept busy at the most threads known up to those, and every
    /// thread past them as a busy core (none measured them idle yet: no Mac is overcommitted); none
    /// known, `c`'s busy cores so.
    pub fn grant(&self, c: Cores, target: &str) -> Cores {
        let grant = self.pick(self.knee().unwrap_or(c.grant), target).max(1);
        if grant == c.grant {
            return c;
        }
        let below = self.known().filter(|p| p.threads <= grant && p.busy_tenths.is_some()).last();
        let busy = match below {
            Some(p) => p.busy_tenths.map(|b| b + (grant - p.threads) * 10),
            None => c.busy_tenths.map(|b| b + grant.saturating_sub(c.grant) * 10),
        };
        Cores { grant, busy_tenths: busy.map(|b| b.clamp(1, grant * 10)) }
    }
}

/// The models made, kept until the measures they're made from change: the change count they were
/// made at (the owner's: the coordinator bumps its count with each cost, floor or disk floor it
/// takes), by (step, measure); each step's most cores (`Book::cores`) and NAS prior
/// (`Book::nas_prior`) alike.
#[derive(Debug, Default)]
pub struct Cache(
    Mutex<(u64, BTreeMap<(String, Measure), Model>)>,
    Mutex<(u64, BTreeMap<String, Option<(Cores, bool)>>)>,
    Mutex<(u64, BTreeMap<String, Option<crate::budget::nas::Rate>>)>,
    /// Each target's `Book::nas` (step → target → it): the NAS budget weighs each target offered
    /// and leased for every ask.
    Mutex<(u64, Rates)>,
    /// Each step's wall time by threads (`Book::curve`).
    Mutex<(u64, BTreeMap<String, Curve>)>,
);

/// `Cache`'s targets' NAS rates: step → target → `Book::nas`.
type Rates = std::collections::HashMap<String, std::collections::HashMap<String, Option<crate::budget::nas::Rate>>>;

/// The measures needs are read from: what each target cost (crate::coord::Cost), and, where kept,
/// the floors (crate::agent::memguard) and disk floors (MB, by cost key); the models made from them
/// kept in `cache` as of change `rev` (a Book of its own, `new`, keeps its own).
pub struct Book<'a> {
    pub costs: &'a BTreeMap<String, Cost>,
    pub floors: Option<&'a BTreeMap<String, Floor>>,
    pub disks: Option<&'a BTreeMap<String, u64>>,
    /// Each Mac's pace at each step (crate::paces, task #926), where known: every run's time and
    /// NAS rate are put at the lead's pace by its worker's before a model is made of them, and a
    /// prediction for a worker (`secs_for`, `nas_for`) is the model at its pace. None: as measured.
    pub paces: Option<&'a crate::paces::Paces>,
    cache: CacheRef<'a>,
    rev: u64,
}

enum CacheRef<'a> {
    Own(Cache),
    Kept(&'a Cache),
}

/// A run a fit is made from: its sizes (km² always, for a tile target), threads, and the measure.
struct Run {
    sizes: BTreeMap<String, f64>,
    threads: Option<f64>,
    y: f64,
}

/// The z/x/y tile a target (or a cost key's) ends in.
fn tile(target: &str) -> Option<(u8, u32, u32)> {
    let t = target.rsplit(' ').next()?;
    let mut it = t.split('/');
    let z = it.next()?.parse().ok()?;
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    (it.next().is_none() && z <= 30).then_some((z, x, y))
}

/// The sizes known of a cost (its measured `sizes`; its tile's area when it has no `km2`).
fn sizes_of(key: &str, c: Option<&Cost>) -> BTreeMap<String, f64> {
    let mut m: BTreeMap<String, f64> = c
        .map(|c| {
            c.sizes
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "threads" | "cpu_ms" | "paused_ms" | "waited_ms"))
                .map(|(k, v)| (k.clone(), *v as f64))
                .collect()
        })
        .unwrap_or_default();
    if let (false, Some((z, x, y))) = (m.contains_key("km2"), tile(key)) {
        m.insert("km2".into(), crate::costin::tile_km2(z, x, y) as f64);
    }
    m
}

/// Least squares of y over [1, x] (and t when given), solved by its normal equations.
fn lsq(rows: &[(f64, Option<f64>, f64)], with_t: bool) -> Option<(f64, f64, Option<f64>)> {
    let n = if with_t { 3 } else { 2 };
    let mut a = [[0.0f64; 4]; 3];
    for &(x, t, y) in rows {
        let f = [1.0, x, t.unwrap_or(0.0)];
        for i in 0..n {
            for j in 0..n {
                a[i][j] += f[i] * f[j];
            }
            a[i][3] += f[i] * y;
        }
    }
    // (Gaussian elimination with partial pivoting.)
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-9 {
            return None;
        }
        a.swap(c, p);
        for r in 0..n {
            if r != c {
                let f = a[r][c] / a[c][c];
                for k in c..4 {
                    a[r][k] -= f * a[c][k];
                }
            }
        }
    }
    let s = |i: usize| a[i][3] / a[i][i];
    Some((s(0), s(1), with_t.then(|| s(2))))
}

/// The envelope fit of `runs` over size `size` (None: too few, or the sizes all one).
fn fit_on(runs: &[&Run], size: &str) -> Option<(f64, f64, Option<f64>)> {
    let rows: Vec<(f64, Option<f64>, f64)> = runs.iter().map(|r| (r.sizes[size], r.threads, r.y)).collect();
    let mut ts: Vec<f64> = rows.iter().filter_map(|r| r.1).collect();
    ts.dedup();
    let with_t = rows.iter().all(|r| r.1.is_some()) && ts.windows(2).any(|w| w[0] != w[1]);
    let (b, k, c) = lsq(&rows, with_t).or_else(|| if with_t { lsq(&rows, false) } else { None })?;
    // (Raised to cover every run.)
    let up = rows.iter().map(|&(x, t, y)| y - (b + k * x + c.zip(t).map_or(0.0, |(c, t)| c * t))).fold(0.0, f64::max);
    Some((b + up, k, c))
}

/// Fits `runs` over `size`, judged on folds held out.
fn judge(runs: &[Run], size: &str) -> Option<Fit> {
    let have: Vec<&Run> = runs.iter().filter(|r| r.sizes.contains_key(size)).collect();
    if have.len() < MIN_RUNS.min(runs.len()).max(2) {
        return None;
    }
    let top = |rs: &[&Run]| rs.iter().map(|r| r.y).fold(0.0, f64::max);
    let (base, k, per_thread) = fit_on(&have, size)?;
    let (mut errs, mut miss) = (Vec::new(), 0usize);
    for f in 0..FOLDS {
        let train: Vec<&Run> = have.iter().enumerate().filter(|(i, _)| i % FOLDS != f).map(|(_, r)| *r).collect();
        let Some((b, k, c)) = fit_on(&train, size) else { continue };
        let fit = Fit { size: size.into(), base: b, k, per_thread: c, top: top(&train), runs: 0, miss: 0.0, err: 0.0 };
        for (_, r) in have.iter().enumerate().filter(|(i, _)| i % FOLDS == f) {
            let p = fit.at(r.sizes[size], r.threads);
            if p < r.y {
                miss += 1;
            }
            errs.push((p - r.y).abs() / r.y.max(1.0));
        }
    }
    if errs.is_empty() {
        return None;
    }
    errs.sort_by(f64::total_cmp);
    Some(Fit {
        size: size.into(),
        base,
        k,
        per_thread,
        top: top(&have),
        runs: have.len(),
        miss: miss as f64 / errs.len() as f64,
        err: errs[errs.len() / 2],
    })
}

/// A run's NAS rate (`bytes` less what of them it read from this Mac's own disk, `local` (its whole
/// local copies', task #551), plus `wrote`, and `creates`, over its time; its metadata work,
/// `listed`, `stats` and `deletes`, likewise), when it was measured the
/// way its step runs now and says its `bytes`. A `HALO_STEPS` step's reads are its job's, not its
/// own: `nas_read` over `nas_secs` (the job's reads and the time they took, the same on each of its
/// targets), beside what it writes over the longer of its own time and `nas_secs`; a measure without `nas_secs` (task #396's
/// first, b38: a share of the job's reads over the target's own time, often 0 s, so every job
/// weighed the whole NAS) counts its writes alone. With the time (s) it is over: a job's rate is its
/// targets' moves over their times together (`Book::nas_load`).
fn nas_rate(step: &str, c: &Cost) -> Option<(crate::budget::nas::Rate, f64)> {
    let g = |k: &str| c.sizes.get(k).copied().unwrap_or(0) as f64;
    // (A halo step's writes are spread over its job's time too, not over its own write phase:
    // pack's `secs` is that phase's 0-1 s, which weighed a pack at the whole NAS, 25 Oct.)
    let halo = HALO_STEPS.contains(&step);
    let secs = if halo { c.secs.max(c.sizes.get("nas_secs").copied().unwrap_or(0)) } else { c.secs }.max(1) as f64;
    let read_s = if halo {
        c.sizes.get("nas_secs").filter(|&&s| s > 0).map_or(0.0, |&s| g("nas_read") / s as f64)
    } else {
        (g("bytes") - g("local")).max(0.0) / secs
    };
    (c.v >= cost_version(step) && c.sizes.contains_key("bytes")).then(|| {
        let r = crate::budget::nas::Rate {
            bytes_s: read_s + g("wrote") / secs,
            creates_s: g("creates") / secs,
            listed_s: g("listed") / secs,
            stats_s: g("stats") / secs,
            deletes_s: g("deletes") / secs,
        };
        (r, secs)
    })
}

/// Rate `r` of a run `pace` times as long (each of its counts over that time).
fn per_pace(r: crate::budget::nas::Rate, pace: f64) -> crate::budget::nas::Rate {
    let d = |x: f64| x / pace.max(1e-6);
    crate::budget::nas::Rate {
        bytes_s: d(r.bytes_s),
        creates_s: d(r.creates_s),
        listed_s: d(r.listed_s),
        stats_s: d(r.stats_s),
        deletes_s: d(r.deletes_s),
    }
}

/// The 90th percentile of values `v` each weighed by its time (nearest rank: the least value at or
/// under which 90 % of their time is); None with none, or no time.
fn p90_by_time(mut v: Vec<(f64, f64)>) -> Option<f64> {
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let all: f64 = v.iter().map(|x| x.1).sum();
    let mut at = 0.0;
    // (Within a millionth of the whole, so equal times give the nearest rank `p90` gives.)
    v.into_iter()
        .find(|x| {
            at += x.1;
            at >= 0.9 * all * (1.0 - 1e-6)
        })
        .filter(|_| all > 0.0)
        .map(|x| x.0)
}

/// The 90th percentile (nearest rank); None with none.
pub fn p90(mut v: Vec<u64>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[(v.len() * 9).div_ceil(10).saturating_sub(1)])
}

/// The mean, rounded up; None with none.
pub fn mean(v: &[u64]) -> Option<u64> {
    (!v.is_empty()).then(|| v.iter().sum::<u64>().div_ceil(v.len() as u64))
}

/// How many runs the way a step runs now its time prior takes alone (`Book::prior_of`); fewer, its
/// latest earlier way's mean stands in when larger.
pub const PRIOR_RUNS: usize = 20;

impl<'a> Book<'a> {
    /// A Book over `costs` alone, keeping its own models.
    pub fn new(costs: &'a BTreeMap<String, Cost>) -> Self {
        Book { costs, floors: None, disks: None, paces: None, cache: CacheRef::Own(Cache::default()), rev: 0 }
    }

    /// A Book over all three, keeping its own models.
    pub fn with(
        costs: &'a BTreeMap<String, Cost>,
        floors: Option<&'a BTreeMap<String, Floor>>,
        disks: Option<&'a BTreeMap<String, u64>>,
    ) -> Self {
        Book { costs, floors, disks, paces: None, cache: CacheRef::Own(Cache::default()), rev: 0 }
    }

    /// A Book whose models are kept in `cache` while its owner's change count is `rev`.
    pub fn kept(
        costs: &'a BTreeMap<String, Cost>,
        floors: &'a BTreeMap<String, Floor>,
        disks: &'a BTreeMap<String, u64>,
        cache: &'a Cache,
        rev: u64,
    ) -> Self {
        Book { costs, floors: Some(floors), disks: Some(disks), paces: None, cache: CacheRef::Kept(cache), rev }
    }

    /// Its times and NAS rates put at the lead's pace by `paces` (its owner's change count, `rev`,
    /// moves with them: a kept model is of the paces it was made with).
    pub fn paced(mut self, paces: &'a crate::paces::Paces) -> Self {
        self.paces = Some(paces);
        self
    }

    /// `worker`'s pace at `step` (1: not known, or no paces).
    fn pace(&self, worker: Option<&str>, step: &str) -> f64 {
        self.paces.zip(worker).map_or(1.0, |(p, w)| p.get(w, step))
    }

    /// Cost `c`'s time of `step` at the lead's pace (s, rounded).
    fn lead_secs(&self, step: &str, c: &Cost) -> u64 {
        (c.secs as f64 / self.pace(c.worker.as_deref(), step)).round() as u64
    }

    fn floor(&self, key: &str) -> Option<&Floor> {
        self.floors.and_then(|f| f.get(key))
    }

    /// The floor of `key` learned the way its step runs now (MB).
    fn current_floor(&self, step: &str, key: &str) -> Option<u64> {
        self.floor(key).filter(|f| f.v >= cost_version(step)).map(|f| f.mb)
    }

    /// The step's runs measured the way it runs now, of `m`.
    fn runs(&self, step: &str, m: Measure) -> Vec<Run> {
        let v = cost_version(step);
        let mut out = Vec::new();
        for (k, c) in self.costs.iter().filter(|(k, c)| step_of_key(k) == step && c.v >= v && !is_drop(c)) {
            let y = match m {
                Measure::Mem => c.peak_mb,
                Measure::Secs if time_weight(step, c).is_none() => continue,
                Measure::Secs => self.lead_secs(step, c),
                Measure::Disk => match self.disks.and_then(|d| d.get(k)) {
                    Some(&d) => d,
                    None => continue,
                },
            };
            if y == 0 {
                continue;
            }
            out.push(Run {
                sizes: sizes_of(k, Some(c)),
                threads: c.sizes.get("threads").map(|&t| t as f64),
                y: y as f64,
            });
        }
        out
    }

    /// The step's model of `m`: made, or as kept since the measures last changed.
    fn model(&self, step: &str, m: Measure) -> Model {
        let cache = match &self.cache {
            CacheRef::Own(c) => c,
            CacheRef::Kept(c) => *c,
        };
        let key = (step.to_string(), m);
        {
            let mut g = cache.0.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != self.rev {
                *g = (self.rev, BTreeMap::new());
            }
            if let Some(md) = g.1.get(&key) {
                return md.clone();
            }
        }
        let runs = self.runs(step, m);
        let mut names: Vec<&String> = runs.iter().flat_map(|r| r.sizes.keys()).collect();
        names.sort();
        names.dedup();
        let mut fits: Vec<Fit> = names.into_iter().filter_map(|s| judge(&runs, s)).collect();
        fits.sort_by(|a, b| b.good().cmp(&a.good()).then(a.err.total_cmp(&b.err)).then(a.size.cmp(&b.size)));
        // (Not a `GUESS_FIRST` step's: its guess scales with its piece, and no backtest has weighed
        // its place against that guess, which the history doesn't keep.)
        let near = (m == Measure::Mem && !GUESS_FIRST.contains(&step))
            .then(|| std::sync::Arc::new(Near::of_measures(self.places(step))));
        let (by_source, macs) = if m == Measure::Secs { self.by_source(step) } else { Default::default() };
        let md = Model { fits, prior: self.prior_of(step, m), near, by_source, macs };
        cache.0.lock().unwrap_or_else(|e| e.into_inner()).1.insert(key, md.clone());
        md
    }

    /// Step `step`'s runs by their source's time (`BySource`; None: none of them has a source run),
    /// and the Macs that have runs of it, the way it runs now.
    fn by_source(
        &self,
        step: &str,
    ) -> (Option<std::sync::Arc<BySource>>, std::sync::Arc<std::collections::BTreeSet<String>>) {
        let v = cost_version(step);
        let mut by = BySource::default();
        let mut macs = std::collections::BTreeSet::new();
        // (Each run's Mac, bin, time as it ran and at the lead's pace, for the judging below.)
        let mut runs: Vec<(String, u32, f64, f64)> = Vec::new();
        for (k, c) in self.costs.iter().filter(|(k, c)| step_of_key(k) == step && c.v >= v && !is_drop(c)) {
            let Some(mac) = c.worker.as_deref().map(crate::paces::mac_of) else { continue };
            macs.insert(mac.to_string());
            if time_weight(step, c).is_none() {
                continue;
            }
            let Some(b) = k.get(step.len() + 1..).and_then(|t| self.source_secs(step, t)).map(bin) else { continue };
            let y = self.lead_secs(step, c) as f64;
            runs.push((mac.to_string(), b, c.secs as f64, y));
            let e = by.all.entry(b).or_default();
            (e.0, e.1) = (e.0 + y, e.1 + 1);
            let e = by.own.entry((mac.to_string(), b)).or_default();
            (e.0, e.1) = (e.0 + c.secs as f64, e.1 + 1);
        }
        // (Used only where it predicts its own runs better than their mean does, each as `secs_on`
        // reads it (its Mac's bin, else every Mac's) with the run left out, by the median error a
        // fit is judged by (`judge`): a source that says nothing of a step's targets, on a step its
        // Macs run alike, doesn't replace its model.)
        let (n, sum) = (runs.len() as f64, runs.iter().map(|r| r.3).sum::<f64>());
        let err = |by_bin: bool| {
            let out = |a: &(f64, usize), y: f64| (a.1 > MIN_RUNS).then(|| (a.0 - y) / (a.1 - 1) as f64);
            let mut e: Vec<f64> = runs
                .iter()
                .map(|(mac, b, ran, y)| {
                    let own = by.own.get(&(mac.clone(), *b)).and_then(|a| out(a, *ran)).map(|p| (p, *ran));
                    let all = || by.all.get(b).and_then(|a| out(a, *y)).map(|p| (p, *y));
                    let (p, y) = own.or_else(all).filter(|_| by_bin).unwrap_or(((sum - y) / (n - 1.0).max(1.0), *y));
                    (p - y).abs() / y.max(1.0)
                })
                .collect();
            e.sort_by(f64::total_cmp);
            e.get(e.len() / 2).copied().unwrap_or(f64::INFINITY)
        };
        let good = runs.len() > MIN_RUNS && err(true) < err(false);
        (good.then(|| std::sync::Arc::new(by)), std::sync::Arc::new(macs))
    }

    /// The time target `target`'s source took (`BySource`), at the lead's pace (s): the same target's
    /// run, the way its step runs now, of the first step `step` reads that names it.
    fn source_secs(&self, step: &str, target: &str) -> Option<f64> {
        use crate::agent::steps::Read;
        crate::agent::steps::row(step)?.reads.iter().find_map(|r| match r {
            Read::Targets { step: src, of, .. } if of(target).iter().any(|t| t == target) => {
                let c = self.costs.get(&cost_key(src, target)).filter(|c| c.v >= cost_version(src) && !is_drop(c))?;
                Some(self.lead_secs(src, c) as f64)
            }
            _ => None,
        })
    }

    /// The step's fits of `m`, best first (the good ones, then the poor), one per size its runs have.
    pub fn fits(&self, step: &str, m: Measure) -> Vec<Fit> {
        self.model(step, m).fits
    }

    /// The step's prior of `m`: memory and disk, `p90` of its targets' measures the way it runs now
    /// (memory: their floors among them; disk: its disk floors); time, the `mean` of its runs.
    pub fn prior(&self, step: &str, m: Measure) -> Option<u64> {
        self.model(step, m).prior
    }

    /// The step's tile targets' memory measured the way it runs now, by tile (MB): each one's run,
    /// or its floor, the larger, as its prior counts them (a drop's measures say nothing of its builds).
    fn places(&self, step: &str) -> std::collections::HashMap<(u8, u32, u32), u64> {
        let v = cost_version(step);
        let mut at = std::collections::HashMap::new();
        let runs = self
            .costs
            .iter()
            .filter(|(k, c)| step_of_key(k) == step && c.v >= v && !is_drop(c))
            .map(|(k, c)| (k, c.peak_mb));
        let floors = self
            .floors
            .into_iter()
            .flatten()
            .filter(|(k, f)| step_of_key(k) == step && f.v >= v && !self.costs.get(*k).is_some_and(is_drop))
            .map(|(k, f)| (k, f.mb));
        for (k, mb) in runs.chain(floors).filter(|(_, mb)| *mb > 0) {
            if let Some(t) = tile(k) {
                let e = at.entry(t).or_insert(0);
                *e = (*e).max(mb);
            }
        }
        at
    }

    fn prior_of(&self, step: &str, m: Measure) -> Option<u64> {
        let v = cost_version(step);
        // (A drop's measures say nothing of its step's builds: `DROPS`.)
        let of = |k: &String| step_of_key(k) == step && !self.costs.get(k).is_some_and(is_drop);
        let vals: Vec<u64> = match m {
            Measure::Mem => {
                // (A target's own: its measure and floor, the larger.)
                let mut by: BTreeMap<&String, u64> =
                    self.costs.iter().filter(|(k, c)| of(k) && c.v >= v).map(|(k, c)| (k, c.peak_mb)).collect();
                for (k, f) in self.floors.into_iter().flatten().filter(|(k, f)| of(k) && f.v >= v) {
                    let e = by.entry(k).or_default();
                    *e = (*e).max(f.mb);
                }
                by.into_values().collect()
            }
            Measure::Secs => {
                // (Each weighed: a PER_WORK step's by the work it did, task #830.)
                let weighed = |c: &Cost| time_weight(step, c).map(|w| (self.lead_secs(step, c), w));
                let secs: Vec<(u64, f64)> =
                    self.costs.iter().filter(|(k, c)| of(k) && c.v >= v).filter_map(|(_, c)| weighed(c)).collect();
                // (While its runs the way it runs now are few, they are the quick ones: the first to
                // end. Its latest earlier way's mean stands in when larger, so its first jobs aren't sized by
                // them: depth's first 8 runs at version 1 took 14–34 s but one, and jobs of 64 tiles
                // ran 90 min against the quarter hour they were sized to.)
                if secs.len() < PRIOR_RUNS {
                    // (The latest earlier way's runs alone.)
                    let last = self.costs.iter().filter(|(k, c)| of(k) && c.v < v).map(|(_, c)| c.v).max();
                    let old: Vec<(u64, f64)> = self
                        .costs
                        .iter()
                        .filter(|(k, c)| of(k) && Some(c.v) == last)
                        .filter_map(|(_, c)| weighed(c))
                        .collect();
                    return match (weighted_mean(&secs), weighted_mean(&old)) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                }
                return weighted_mean(&secs);
            }
            Measure::Disk => self.disks.into_iter().flatten().filter(|(k, _)| of(k)).map(|(_, d)| *d).collect(),
        };
        p90(vals)
    }

    /// The fitted `m` of `target` of `step` (None: no good fit over a size it has).
    fn fitted(&self, md: &Model, key: &str, threads: u32) -> Option<u64> {
        self.fitted_by(md, key, threads).map(|(p, _)| p)
    }

    /// `fitted`, with the fit's held-out median error.
    fn fitted_by(&self, md: &Model, key: &str, threads: u32) -> Option<(u64, f64)> {
        if !md.fits.iter().any(Fit::good) {
            return None;
        }
        // (Its sizes: as its own earlier run measured them, else its area.)
        let sizes = sizes_of(key, self.costs.get(key));
        let t = (threads > 0).then_some(threads as f64);
        md.fits.iter().filter(|f| f.good()).find_map(|f| sizes.get(&f.size).map(|&x| (f.at(x, t).ceil() as u64, f.err)))
    }

    /// The memory (MB) a job of `step` for `target` needs, and where it came from: `need`'s, alone
    /// (what the picks read).
    pub fn mem(&self, step: &str, target: &str, threads: u32, guess: u64) -> (u64, From) {
        let key = cost_key(step, target);
        let floor = self.current_floor(step, &key);
        if let Some(c) = self.costs.get(&key).filter(|c| c.v >= cost_version(step)) {
            return (c.peak_mb.max(floor.unwrap_or(0)), From::Measured);
        }
        if let Some(f) = floor {
            return (f, From::Measured);
        }
        let md = self.model(step, Measure::Mem);
        // (By its place where that's judged good, unless a good fit over a size it has erred less
        // on the runs held out.)
        let near = md.near.as_ref().filter(|n| n.good()).and_then(|n| Some((n.of(&key)?, n.err)));
        let fit = self.fitted_by(&md, &key, threads);
        match (near, fit, md.prior) {
            (Some((p, e)), f, _) if f.is_none_or(|f| e <= f.1) => (p, From::Near),
            (_, Some((p, _)), _) => (p, From::Fit),
            (None, None, Some(_)) if guess > 0 && GUESS_FIRST.contains(&step) => (guess, From::Guess),
            (_, None, Some(p)) => (p, From::Prior),
            (_, None, None) => (guess, From::Table),
        }
    }

    /// The disk (MB) a job of `step` for `target` needs: its disk floor, else fitted, else its
    /// step's prior; None when none of its step was measured. A step reading a shared cache
    /// (`CACHES`): its own output alone, its disk floor at most its step's prior (a measure past
    /// it a cache's fill, reserved once per Mac apart), never fitted (its measures mix fills in).
    pub fn disk(&self, step: &str, target: &str, threads: u32) -> Option<u64> {
        let key = cost_key(step, target);
        let own = self.disks.and_then(|d| d.get(&key)).copied();
        if cache_of(step).is_some() {
            let prior = self.model(step, Measure::Disk).prior;
            return match (own, prior) {
                (Some(o), Some(p)) => Some(o.min(p)),
                (o, p) => o.or(p),
            };
        }
        if let Some(d) = own {
            return Some(d);
        }
        let md = self.model(step, Measure::Disk);
        self.fitted(&md, &key, threads).or(md.prior)
    }

    /// The wall time (s) a job of `step` for `target` takes: its own run's, else fitted, else its
    /// step's prior (at the lead's pace, with `paces`); None when none of its step ran the way it
    /// runs now.
    pub fn secs(&self, step: &str, target: &str, threads: u32) -> Option<u64> {
        self.secs_for(step, target, threads, None)
    }

    /// `secs` on `worker` (`secs_on`'s).
    pub fn secs_for(&self, step: &str, target: &str, threads: u32, worker: Option<&str>) -> Option<u64> {
        self.secs_on(step, target, threads, worker).map(|s| s.0)
    }

    /// What target `target` of `step` takes `worker` on `threads` (s; 0: as it ran), and whether by
    /// its Mac's own runs of the step (task #927, docs/pool.md §7.7, Batch length). None when none
    /// of its step's ran the way it runs now.
    /// - **Its Mac has runs of the step** (`by_source`'s `macs`): its own run, on its Mac as it ran,
    ///   from another Mac at the lead's pace by that one's and at this one's (crate::paces), and on
    ///   fewer threads than it kept busy, its CPU time over them, the longer; else, with a source
    ///   run (`BySource`, where it's judged better than the step's mean), the mean of its bin's runs
    ///   on this Mac where it has `MIN_RUNS` of them; else its step's good fit at this Mac's pace;
    ///   else its bin's runs on every Mac at this one's pace; else its step's prior at it.
    /// - **None** (or no worker): as before task #927, said by the second value: its own run as it
    ///   ran, twice that from another worker (no worker: as it ran); else the model at its pace.
    pub fn secs_on(&self, step: &str, target: &str, threads: u32, worker: Option<&str>) -> Option<(u64, bool)> {
        let key = cost_key(step, target);
        let md = self.model(step, Measure::Secs);
        let mac = worker.map(crate::paces::mac_of).filter(|m| md.macs.contains(*m));
        let own = self.costs.get(&key).filter(|c| c.v >= cost_version(step));
        let Some(mac) = mac else {
            if let Some(c) = own {
                let other = worker.is_some_and(|w| c.worker.as_deref() != Some(w));
                return Some((if other { c.secs * 2 } else { c.secs }, false));
            }
            let s = self.fitted(&md, &key, threads).or(md.prior)?;
            return Some(((s as f64 * self.pace(worker, step)).round() as u64, false));
        };
        let pace = self.pace(worker, step);
        if let Some(c) = own {
            let ran = c.worker.as_deref().map(crate::paces::mac_of);
            let at = if ran == Some(mac) { 1.0 } else { pace / self.pace(c.worker.as_deref(), step) };
            let cpu =
                c.sizes.get("cpu_ms").filter(|_| threads > 0).map_or(0.0, |&ms| ms as f64 / 1000.0 / threads as f64);
            return Some((((c.secs as f64).max(cpu) * at).round() as u64, true));
        }
        // (Its bin: this Mac's own runs in it, else, after a good fit, every Mac's.)
        let bin_of = md.by_source.as_ref().zip(self.source_secs(step, target)).map(|(by, src)| (by, bin(src)));
        let s = bin_of
            .and_then(|(by, b)| bin_mean(by.own.get(&(mac.to_string(), b))))
            // (Its fit as `secs` reads it, of no thread count: a fit's thread term is of runs
            // whose threads followed the load they ran in, not what more threads would do.)
            .or_else(|| self.fitted(&md, &key, 0).map(|s| s as f64 * pace))
            .or_else(|| bin_of.and_then(|(by, b)| bin_mean(by.all.get(&b))).map(|s| s * pace))
            .or_else(|| md.prior.map(|s| s as f64 * pace))?;
        Some((s.round() as u64, true))
    }

    /// What a drop of `step` (`DROPS`) takes, by its step's drops run the way it runs now: their
    /// mean time (s, rounded up) and the 90th percentile of their memory (MB). Before any of its
    /// step's has run, every step's drops' alike: a drop of any step reads the manifest alone and
    /// deletes names from it (crate::terrain_deep::drop_target, crate::terrain_pack::drop_piece,
    /// crate::slope_pack::drop_lo …), so one step's measure is the next's best first guess, never a
    /// build's minutes and gigabytes. None while no drop of any step ran.
    pub fn drop_need(&self, step: &str) -> Option<(u64, u64)> {
        let of = |only: Option<&str>| -> Vec<&Cost> {
            self.costs
                .iter()
                .filter(|(k, c)| {
                    let s = step_of_key(k);
                    only.is_none_or(|o| s == o) && c.v >= cost_version(s) && is_drop(c)
                })
                .map(|(_, c)| c)
                .collect()
        };
        let mut runs = of(Some(step));
        if runs.is_empty() {
            runs = of(None);
        }
        let secs = weighted_mean(&runs.iter().map(|c| (c.secs, 1.0)).collect::<Vec<_>>())?;
        Some((secs, p90(runs.iter().map(|c| c.peak_mb).collect()).unwrap_or(0)))
    }

    /// `secs_for` with its step's prior the 90th percentile of its runs the way it runs now instead
    /// of their mean: what an update's drain waits for a target under way (crate::agent's
    /// `update_wait`), where a long tail's long jobs must not be cut short.
    pub fn secs_p90(&self, step: &str, target: &str, threads: u32, worker: Option<&str>) -> Option<u64> {
        let key = cost_key(step, target);
        let v = cost_version(step);
        if let Some(c) = self.costs.get(&key).filter(|c| c.v >= v) {
            return Some(c.secs);
        }
        let md = self.model(step, Measure::Secs);
        let s = self.fitted(&md, &key, threads).or_else(|| {
            p90(self
                .costs
                .iter()
                .filter(|(k, c)| step_of_key(k) == step && c.v >= v && time_weight(step, c).is_some())
                .map(|(_, c)| self.lead_secs(step, c))
                .collect())
        })?;
        Some((s as f64 * self.pace(worker, step)).round() as u64)
    }

    /// The cores a job of `step` for `target` takes, as measured (crate::costin's `sizes`): its own
    /// run's, else the most of its step's runs the way it runs now (each of `Cores` its most); None
    /// when none of them says (what packs jobs beside each other, crate::coord's packing, then
    /// packs none beside it). (A run waiting on a crowded NAS reads less busy, which would pack more
    /// jobs, each reading less busy still; but a target not run the way it runs now, nearly every
    /// one a job is granted for, counts for its step's most, which no crowded run lowers: task #697.)
    /// One whose waits dominate (task #904: its runs waited half their time or more, `waited_ms`,
    /// and didn't climb) takes its threads from its step's wall time by threads instead
    /// (`Curve::grant`): a wait-bound step gets the threads that pay, and no more; a busy one climbs.
    pub fn cores(&self, step: &str, target: &str) -> Option<Cores> {
        let (c, no_climb) = self.cores_busy(step, target)?;
        Some(if no_climb && self.curve(step).waits { self.curve(step).grant(c, target) } else { c })
    }

    /// `cores` by its busy cores alone (task #896), and whether it doesn't climb (each run it's read
    /// from didn't compute on every thread): its own run's, else its step's most.
    fn cores_busy(&self, step: &str, target: &str) -> Option<(Cores, bool)> {
        let v = cost_version(step);
        let threads = |c: &Cost| c.sizes.get("threads").filter(|_| c.v >= v).map(|&t| t as u32);
        let of = |c: &Cost| {
            let t = threads(c)?.max(1);
            // (Its CPU time, `cpu_ms`, over the time it ran: its wall time less the time it was
            // stopped (`paused_ms`), a second at least: the cores it kept busy.)
            let ran = (c.secs * 1000).saturating_sub(c.sizes.get("paused_ms").copied().unwrap_or(0)).max(1000);
            let kept = c.sizes.get("cpu_ms").map(|&ms| ms as f64 / ran as f64);
            Some(match kept {
                None => (Cores { grant: t, busy_tenths: None }, false),
                Some(k) => {
                    // (Task #896: the cores it kept busy while it computed, its CPU time over the
                    // time it didn't wait (`waited_ms`, store::waits by the wall clock, task #870;
                    // none said: none), within its threads, to the nearest whole, and one thread
                    // more: to overlap its waits, or, where it computed on every thread, to climb
                    // by. So a job waiting on the NAS or the network, or held by its serial parts,
                    // settles where a run measures the same (2.4 busy of 6 or of 3: 3), and one its
                    // threads held climbs a thread a run, measured at each, to what it can use
                    // (crate::coord caps it at the Mac's cores). It counts for the thread it
                    // climbs by besides its busy cores, as nothing measured it idle yet, so a
                    // climb never overcommits its Mac (task #888 review).)
                    let busy = ((k * 10.0).round() as u32).max(1);
                    let waited = c.sizes.get("waited_ms").map_or(0, |&w| w.min(ran));
                    // (Task #896 review: waits covering its wall time, within a second, leave no
                    // time it computed in to measure: its busy cores, not every thread, so a run
                    // its threads all waited through settles at those and one more, never climbs.)
                    let computing = if ran - waited >= 1000 { k * ran as f64 / (ran - waited) as f64 } else { k };
                    let grant = (computing.min(f64::from(t)) + 0.5) as u32 + 1;
                    let busy = busy.min(t * 10) + grant.saturating_sub(t) * 10;
                    (Cores { grant, busy_tenths: Some(busy.min(grant * 10)) }, grant <= t)
                }
            })
        };
        if let Some(c) = self.costs.get(&cost_key(step, target)).and_then(of) {
            return Some(c);
        }
        // (The most of its runs at the most threads its step ran with: those measured a climb's
        // threads; a run at fewer, its doubling done, says nothing past them (task #888 review:
        // else a step's one busy run at 2 kept it granted 4 and counted for 4 after its runs at
        // 4 showed 2 busy).)
        let most = || {
            let of_step = || self.costs.iter().filter(|(k, _)| step_of_key(k) == step).map(|(_, c)| c);
            let top = of_step().filter_map(threads).max()?;
            of_step().filter(|c| threads(c) == Some(top)).filter_map(of).reduce(|(a, ca), (b, cb)| {
                let c = Cores {
                    grant: a.grant.max(b.grant),
                    busy_tenths: a.busy_tenths.zip(b.busy_tenths).map(|(a, b)| a.max(b)),
                };
                (c, ca && cb)
            })
        };
        // (Its step's most kept with the models while the measures stand: a placement asks it for
        // every job held.)
        let CacheRef::Kept(cache) = &self.cache else { return most() };
        let mut g = cache.1.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != self.rev {
            *g = (self.rev, BTreeMap::new());
        }
        *g.1.entry(step.to_string()).or_insert_with(most)
    }

    /// The step's wall time by threads (`Curve`), kept with the models while the measures stand.
    pub fn curve(&self, step: &str) -> Curve {
        let cache = match &self.cache {
            CacheRef::Own(c) => c,
            CacheRef::Kept(c) => *c,
        };
        {
            let mut g = cache.4.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != self.rev {
                *g = (self.rev, BTreeMap::new());
            }
            if let Some(c) = g.1.get(step) {
                return c.clone();
            }
        }
        let fit = self.fits(step, Measure::Secs).into_iter().find(Fit::good);
        let v = cost_version(step);
        let of_step = || self.costs.iter().filter(|(k, c)| step_of_key(k) == step && c.v >= v && !is_drop(c));
        // (No good fit: its runs' bytes read (MB), where every one says some: a wait-bound step's
        // work; else the wall alone.)
        let per_mb = fit.is_none() && of_step().all(|(_, c)| c.sizes.get("bytes").is_some_and(|&b| b > 0));
        let mut by: BTreeMap<u32, (Vec<f64>, Option<u32>)> = BTreeMap::new();
        let mut waited: Vec<f64> = Vec::new();
        for (k, c) in of_step() {
            let Some(&t) = c.sizes.get("threads") else { continue };
            let ran = (c.secs * 1000).saturating_sub(c.sizes.get("paused_ms").copied().unwrap_or(0));
            if ran == 0 || time_weight(step, c).is_none() {
                continue;
            }
            // (Its work: the fit's prediction for its size, without the fit's thread term.)
            let work = match &fit {
                Some(f) => match sizes_of(k, Some(c)).get(&f.size).map(|&x| f.base + f.k * x) {
                    Some(w) if w > 0.0 => w,
                    _ => continue,
                },
                None if per_mb => c.sizes["bytes"] as f64 / 1e6,
                None => 1.0,
            };
            if let Some(&w) = c.sizes.get("waited_ms") {
                waited.push(w as f64 / ran as f64);
            }
            let e = by.entry((t as u32).max(1)).or_default();
            e.0.push(ran as f64 / 1000.0 / work);
            if let Some(&ms) = c.sizes.get("cpu_ms") {
                let b = (ms as f64 / ran.max(1000) as f64 * 10.0).round() as u32;
                e.1 = Some(e.1.unwrap_or(0).max(b));
            }
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let n = v.len();
            if n % 2 == 1 {
                v[n / 2]
            } else {
                (v[n / 2 - 1] + v[n / 2]) / 2.0
            }
        };
        let points = by
            .into_iter()
            .map(|(threads, (mut w, busy_tenths))| {
                let m = median(&mut w);
                let mut dev: Vec<f64> = w.iter().map(|x| (x - m).abs()).collect();
                let mad = median(&mut dev);
                Point { threads, runs: w.len(), wall: m, se: 1.4826 * mad / (w.len() as f64).sqrt(), busy_tenths }
            })
            .collect();
        let waits = !waited.is_empty() && median(&mut waited) >= 0.5;
        let c = Curve { size: fit.map(|f| f.size).or(per_mb.then(|| "MB read".to_string())), points, waits };
        cache.4.lock().unwrap_or_else(|e| e.into_inner()).1.insert(step.to_string(), c.clone());
        c
    }

    /// The NAS rate a job of `step` for `target` runs at, as measured (crate::costin: `bytes` read
    /// plus `wrote`, and `creates`, each over its wall time): its own run's, else its step's 90th
    /// percentile of each over its runs the way it runs now; None when none of them was measured.
    /// What crate::budget::nas weighs it by.
    pub fn nas(&self, step: &str, target: &str) -> Option<crate::budget::nas::Rate> {
        let CacheRef::Kept(cache) = &self.cache else {
            return self.nas_own(step, target).or_else(|| self.nas_prior(step));
        };
        {
            let mut g = cache.3.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != self.rev {
                *g = (self.rev, Rates::new());
            }
            if let Some(&r) = g.1.get(step).and_then(|t| t.get(target)) {
                return r;
            }
        }
        // (Worked out unlocked: `nas_prior` takes its own lock.)
        let r = self.nas_own(step, target).or_else(|| self.nas_prior(step));
        cache
            .3
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .entry(step.to_string())
            .or_default()
            .insert(target.to_string(), r);
        r
    }

    /// `nas` on `worker`: its own run's as it ran, else its step's 90th percentile at the worker's
    /// pace (`paces`; a Mac twice as slow moves the same bytes over twice the time).
    pub fn nas_for(&self, step: &str, target: &str, worker: &str) -> Option<crate::budget::nas::Rate> {
        if let Some(r) = self.nas_own(step, target) {
            return Some(r);
        }
        self.nas_prior(step).map(|r| per_pace(r, self.pace(Some(worker), step)))
    }

    /// `nas`'s own run's rate alone.
    pub fn nas_own(&self, step: &str, target: &str) -> Option<crate::budget::nas::Rate> {
        self.costs.get(&cost_key(step, target)).and_then(|c| nas_rate(step, c)).map(|r| r.0)
    }

    /// `nas_for` on `worker` (None: `nas`, at the lead's pace), with the time it lasts (s): its own
    /// run's, as it ran, else its time as `secs_for` predicts it there (None: not known). A job runs
    /// its targets one after another, so it moves their bytes over their times together: what
    /// crate::coord::sched weighs a job of several by (task #939).
    pub fn nas_load(
        &self,
        step: &str,
        target: &str,
        worker: Option<&str>,
    ) -> Option<(crate::budget::nas::Rate, Option<f64>)> {
        if let Some((r, secs)) = self.costs.get(&cost_key(step, target)).and_then(|c| nas_rate(step, c)) {
            return Some((r, Some(secs)));
        }
        let r = match worker {
            Some(w) => self.nas_for(step, target, w)?,
            None => self.nas(step, target)?,
        };
        Some((r, self.secs_for(step, target, 0, worker).map(|s| s.max(1) as f64)))
    }

    /// `nas`'s step's 90th percentile alone: kept with the models while the measures stand (the
    /// NAS budget weighs every target offered and leased by it, task #544).
    pub fn nas_prior(&self, step: &str) -> Option<crate::budget::nas::Rate> {
        let CacheRef::Kept(cache) = &self.cache else { return self.nas_p90(step) };
        let mut g = cache.2.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != self.rev {
            *g = (self.rev, BTreeMap::new());
        }
        if let Some(&r) = g.1.get(step) {
            return r;
        }
        let r = self.nas_p90(step);
        g.1.insert(step.to_string(), r);
        r
    }

    /// `nas_prior` worked out: its step's runs' 90th percentile of each rate, each run weighed by
    /// its time: the rate its step's jobs keep the NAS under for 90 % of the time they run (task
    /// #939: of slope-deep's 4,254 runs on 4 Nov, 3,379 copied a 200 MB pack in a second or so,
    /// mostly from this Mac's cache, between runs of a minute or more at 10 MB/s; taken run by run,
    /// its 90th percentile was 248 MB/s, three times the NAS, where its runs moved 19 MB/s over
    /// their time together).
    fn nas_p90(&self, step: &str) -> Option<crate::budget::nas::Rate> {
        // (Each at the lead's pace: its time there shorter or longer by its worker's pace.)
        let runs: Vec<(crate::budget::nas::Rate, f64)> = self
            .costs
            .iter()
            .filter(|(k, _)| step_of_key(k) == step)
            .filter_map(|(_, c)| {
                let (r, secs) = nas_rate(step, c)?;
                let pace = self.pace(c.worker.as_deref(), step);
                Some((per_pace(r, 1.0 / pace), secs / pace))
            })
            .collect();
        let p = |f: fn(&crate::budget::nas::Rate) -> f64| p90_by_time(runs.iter().map(|(r, s)| (f(r), *s)).collect());
        Some(crate::budget::nas::Rate {
            bytes_s: p(|r| r.bytes_s)?,
            creates_s: p(|r| r.creates_s)?,
            listed_s: p(|r| r.listed_s)?,
            stats_s: p(|r| r.stats_s)?,
            deletes_s: p(|r| r.deletes_s)?,
        })
    }

    /// What a job of `step` for `target` needs, given `threads` (0: not said) and `guess`, the memory
    /// (MB) its caller would take without a measure (the steps table's first guess, what its plan
    /// offered it with, a z6 tile's extract-size guess).
    pub fn need(&self, step: &str, target: &str, threads: u32, guess: u64) -> Need {
        let (mem_mb, mem_from) = self.mem(step, target, threads, guess);
        let io = match crate::agent::steps::row(step).map(|r| r.kind) {
            Some(crate::agent::steps::Kind::Fetch) => Io::Net,
            _ => Io::Nas,
        };
        Need {
            mem_mb,
            mem_from,
            disk_mb: self.disk(step, target, threads),
            secs: self.secs(step, target, threads),
            cores: threads,
            io,
        }
    }
}

/// `scenic pool needs --fit`: per step and measure, its fits' held-out errors, and the steps that
/// don't fit (they take their prior, a z6 tile or candidates their extract's guess first).
pub fn fit_table(book: &Book) -> String {
    let mut steps: Vec<&str> = book.costs.keys().map(|k| step_of_key(k)).collect();
    steps.sort();
    steps.dedup();
    let mut out = format!(
        "{:<14} {:<7} {:>5}  {:<10} {:>7} {:>7} {:>8}  fit\n",
        "step", "measure", "runs", "size", "miss", "error", "prior err"
    );
    let mut poor: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for s in &steps {
        for m in Measure::ALL {
            let runs = book.runs(s, m);
            if runs.is_empty() {
                continue;
            }
            // (The step's prior as each run's prediction: what a poor fit falls back to; its
            // misses are what the targets' floors hold.)
            let prior = book.prior(s, m).unwrap_or(0) as f64;
            let mut e: Vec<f64> = runs.iter().map(|r| (prior - r.y).abs() / r.y.max(1.0)).collect();
            e.sort_by(f64::total_cmp);
            let max_err = format!("{:.1}%", e[e.len() / 2] * 100.0);
            let fits = book.fits(s, m);
            let n = runs.len();
            match fits.first() {
                Some(f) => {
                    let t = f.per_thread.map_or(String::new(), |c| format!(" + {c:.1}×threads"));
                    out += &format!(
                        "{:<14} {:<7} {:>5}  {:<10} {:>6.1}% {:>6.1}% {:>8}  {}{:.0} + {:.4}×{}{t}\n",
                        s,
                        m.name(),
                        n,
                        f.size,
                        f.miss * 100.0,
                        f.err * 100.0,
                        max_err,
                        if f.good() { "" } else { "poor: " },
                        f.base,
                        f.k,
                        f.size
                    );
                }
                None => {
                    out += &format!(
                        "{:<14} {:<7} {:>5}  {:<10} {:>7} {:>7} {:>8}  poor: no size to fit on\n",
                        s,
                        m.name(),
                        n,
                        "-",
                        "-",
                        "-",
                        max_err
                    )
                }
            }
            // (Memory by place, `Near`: each measure predicted from its nearest others.)
            if let Some(nr) = book.model(s, m).near.filter(|nr| nr.runs > 0) {
                out += &format!(
                    "{:<14} {:<7} {:>5}  {:<10} {:>6.1}% {:>6.1}% {:>8}  {}its {NEAR_RUNS} nearest within {NEAR_RINGS} rings × {MARGIN}\n",
                    s,
                    m.name(),
                    nr.runs,
                    "place",
                    nr.miss * 100.0,
                    nr.err * 100.0,
                    max_err,
                    if nr.good() { "" } else { "poor: " },
                );
            }
            if !fits.first().is_some_and(Fit::good) {
                poor.entry(s).or_default().push(m.name());
            }
        }
    }
    out += &format!("\nheld out: {FOLDS} folds; poor past {:.0}% missed or {:.0}% median error, or under {MIN_RUNS} runs; a fit's prediction is its envelope × {MARGIN}; capped at its runs' most; prior err: the step's prior (memory and disk its runs' 90th percentile, time their mean) taken as each run's (median)\n", MAX_MISS * 100.0, MAX_ERR * 100.0);
    out += "memory by place: a tile target not measured takes its nearest measured ones' most where that's good (and erred less than a good fit)\n";
    out += "steps that don't fit (they take their prior, or their memory by place where good; units and candidates their piece's guess before the prior):\n";
    for (s, ms) in &poor {
        out += &format!("  {s}: {}\n", ms.join(", "));
    }
    // (Task #904: each step's wall time by threads, its knee, and what a job not exploring is
    // granted: by its busy cores alone (task #896), and with its curve.)
    out += &format!(
        "\nthreads: wall per unit of work by threads (runs × median ± its standard error; a point from {CURVE_RUNS} runs), the knee (beaten by {CURVE_SE} standard errors or more), the grant by busy cores → by the curve (where waits dominate)\n"
    );
    for s in &steps {
        let c = book.curve(s);
        if c.points.is_empty() {
            continue;
        }
        let pts: Vec<String> =
            c.points.iter().map(|p| format!("{}: {}×{:.3}±{:.3}", p.threads, p.runs, p.wall, p.se)).collect();
        let grants = book.cores_busy(s, "").map_or("-".to_string(), |(b, no_climb)| {
            let waits = no_climb && c.waits;
            let after = if waits { c.knee().unwrap_or(b.grant) } else { b.grant };
            format!("{} → {}{}", b.grant, after, if waits { "" } else { " (not waits)" })
        });
        out += &format!(
            "  {s:<14} per {:<8} knee {:<3} grant {grants:<14} {}\n",
            c.size.as_deref().unwrap_or("run"),
            c.knee().map_or("-".to_string(), |k| k.to_string()),
            pts.join("  ")
        );
    }
    out
}

/// Reads costs from `costs.json` (`{<cost key>: Cost}`, the coordinator's) or a `costs.jsonl` (a
/// job's done lines: `{unit, peak_mb, secs, v, disk_mb, sizes}`; its disk taken as a disk measure).
pub fn read_costs(p: &std::path::Path) -> anyhow::Result<(BTreeMap<String, Cost>, BTreeMap<String, u64>)> {
    let s = std::fs::read_to_string(p)?;
    if let Ok(m) = serde_json::from_str::<BTreeMap<String, Cost>>(&s) {
        return Ok((m, BTreeMap::new()));
    }
    let (mut costs, mut disks) = (BTreeMap::new(), BTreeMap::new());
    for l in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let (Some(k), Some(peak)) = (v["unit"].as_str(), v["peak_mb"].as_u64()) else { continue };
        let sizes = v["sizes"]
            .as_object()
            .map(|m| m.iter().filter_map(|(k, n)| Some((k.clone(), n.as_u64()?))).collect())
            .unwrap_or_default();
        if let Some(d) = v["disk_mb"].as_u64().filter(|&d| d > 0) {
            disks.insert(k.to_string(), d);
        }
        costs.insert(
            k.to_string(),
            Cost {
                peak_mb: peak,
                secs: v["secs"].as_u64().unwrap_or(0),
                worker: None,
                v: v["v"].as_u64().unwrap_or(0) as u32,
                sizes,
            },
        );
    }
    Ok((costs, disks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(peak: u64, secs: u64, sizes: &[(&str, u64)]) -> Cost {
        Cost {
            peak_mb: peak,
            secs,
            worker: None,
            v: 0,
            sizes: sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    /// `runs` runs of slope a thread count, each target's wall `wall(t)` seconds give or take
    /// `noise` (a share, from a fixed sequence), keeping 0.3 cores busy and waiting the rest.
    fn curve_costs(threads: &[u32], runs: u32, noise: f64, wall: impl Fn(u32) -> f64) -> BTreeMap<String, Cost> {
        let mut costs = BTreeMap::new();
        let mut r = 12345u64;
        for &t in threads {
            for i in 0..runs {
                r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let u = (r >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
                let secs = (wall(t) * (1.0 + noise * u)).round() as u64;
                let sizes = [("threads", u64::from(t)), ("cpu_ms", secs * 300), ("waited_ms", secs * 700)];
                let c = Cost { v: cost_version("slope"), ..cost(100, secs, &sizes) };
                costs.insert(cost_key("slope", &format!("6/{t}/{i}")), c);
            }
        }
        costs
    }

    /// Task #904: the knee, the fewest threads no more threads beat by their runs' own spread, on
    /// flat, linear, saturating and noisy curves.
    #[test]
    fn the_knee_is_found_on_synthetic_curves() {
        let t6 = [1, 2, 3, 4, 5, 6];
        let knee = |costs: &BTreeMap<String, Cost>| Book::new(costs).curve("slope").knee();
        assert_eq!(knee(&curve_costs(&t6, 8, 0.0, |_| 100.0)), Some(1), "flat: one thread");
        assert_eq!(knee(&curve_costs(&t6, 8, 0.0, |t| 600.0 / f64::from(t))), Some(6), "linear: the most");
        // (#896's review on the M4: unit 6/46/16 at 121 s on 1 thread, 57 s on 3; 6 never beat 3.)
        let sat = |t: u32| [121.0, 80.0, 57.0, 57.0, 57.0, 57.0][t as usize - 1];
        assert_eq!(knee(&curve_costs(&t6, 8, 0.0, sat)), Some(3), "saturating: where it stops");
        assert_eq!(knee(&curve_costs(&t6, 30, 0.2, sat)), Some(3), "noisy, saturating");
        assert_eq!(knee(&curve_costs(&t6, 30, 0.3, |_| 100.0)), Some(1), "noisy, flat: no false knee");
        // (A gain inside the runs' spread isn't one: 2 % faster, 30 % noise, 8 runs.)
        assert_eq!(knee(&curve_costs(&[2, 3], 8, 0.3, |t| 100.0 - f64::from(t))), Some(2));
        assert_eq!(knee(&curve_costs(&[3], 30, 0.0, sat)), None, "one count known: no curve yet");
        assert_eq!(knee(&curve_costs(&[1, 3], 4, 0.0, sat)), None, "too few runs at each");
    }

    /// Task #904: a tenth of the targets explore a neighbour of the knee the curve lacks, and none
    /// once both are known; the threads explored past the measured count as busy cores.
    #[test]
    fn exploring_is_bounded() {
        let sat = |t: u32| [121.0, 80.0, 57.0, 57.0, 57.0, 57.0][t as usize - 1];
        let count = |costs: &BTreeMap<String, Cost>| {
            let b = Book::new(costs);
            let mut n = BTreeMap::new();
            for i in 0..10_000 {
                *n.entry(b.cores("slope", &format!("7/{i}/0")).unwrap().grant).or_insert(0u32) += 1;
            }
            n
        };
        // (Known at 3 alone: the busy cores' grant, 0.3 + 0.5 → 1 and a thread, 2, not known yet:
        // none explore; known at 2 too, its knee 3, a tenth explore 4.)
        let n = count(&curve_costs(&[3], 30, 0.0, sat));
        assert_eq!(n.keys().copied().collect::<Vec<_>>(), [2], "{n:?}");
        assert_eq!(count(&curve_costs(&[3], 4, 0.0, sat)).keys().copied().collect::<Vec<_>>(), [2]);
        // (Known at 2 and 3: the knee 3, a tenth exploring 4.)
        let n = count(&curve_costs(&[2, 3], 30, 0.0, sat));
        assert_eq!(n.keys().copied().collect::<Vec<_>>(), [3, 4], "{n:?}");
        assert!((700..1300).contains(&n[&4]), "{n:?}");
        // (Known at 2, 3, 4: nothing to explore.)
        assert_eq!(count(&curve_costs(&[2, 3, 4], 30, 0.0, sat)).keys().copied().collect::<Vec<_>>(), [3]);
        // (The fourth thread, unmeasured, counts as a busy core besides 3's 0.3.)
        let costs = curve_costs(&[2, 3], 30, 0.0, sat);
        let b = Book::new(&costs);
        let explorer = (0..).map(|i| format!("7/{i}/0")).find(|t| b.cores("slope", t).unwrap().grant == 4).unwrap();
        assert_eq!(b.cores("slope", &explorer), Some(Cores { grant: 4, busy_tenths: Some(13) }));
        // (A job whose runs computed on every thread climbs by its busy cores: the curve doesn't hold it.)
        let mut costs = BTreeMap::new();
        for i in 0..30 {
            let c = Cost {
                v: cost_version("slope"),
                ..cost(100, 100, &[("threads", 3), ("cpu_ms", 300_000), ("waited_ms", 0)])
            };
            costs.insert(cost_key("slope", &format!("6/3/{i}")), c);
            let c = Cost {
                v: cost_version("slope"),
                ..cost(100, 100, &[("threads", 2), ("cpu_ms", 200_000), ("waited_ms", 0)])
            };
            costs.insert(cost_key("slope", &format!("6/2/{i}")), c);
        }
        assert_eq!(Book::new(&costs).cores("slope", "7/0/0").map(|c| c.grant), Some(4));
    }

    /// A job's cores: its target's run's, else the most of its step's; None when no run says.
    #[test]
    fn cores_are_the_targets_run_else_the_most_of_its_steps() {
        let now = |step: &str, c: Cost| Cost { v: cost_version(step), ..c };
        let c = |grant, busy_tenths| Some(Cores { grant, busy_tenths });
        let mut costs = BTreeMap::new();
        costs.insert(cost_key("slope", "6/1/1"), now("slope", cost(100, 5, &[("threads", 4)])));
        costs.insert(cost_key("slope", "6/1/2"), now("slope", cost(100, 5, &[("threads", 2)])));
        costs.insert(cost_key("slope", "6/1/3"), cost(100, 5, &[("threads", 8)]));
        costs.insert(cost_key("lo", "3/1/1"), now("lo", cost(100, 5, &[])));
        let b = Book::new(&costs);
        assert_eq!(b.cores("slope", "6/1/2"), c(2, None), "no CPU time said: its busy cores not known");
        assert_eq!(b.cores("slope", "6/1/9"), c(4, None), "an older way's run counts for nothing");
        assert_eq!(b.cores("lo", "3/1/1"), None);
        // (Its CPU time over its wall time: a serial run given 8 threads that waited on nothing is
        // granted 2 and counted for 0.9 cores; a busy one, its threads full, climbs a thread from
        // its 8, counted for its 8 busy and the one it climbs by.)
        let w0 = ("waited_ms", 0);
        costs.insert(cost_key("pois", "6/1/1"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 9_000), w0])));
        costs.insert(cost_key("pois", "6/1/2"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 80_000)])));
        // (Task #896: 7.6 cores busy of 8 is 8 to the nearest whole: its threads held it.)
        costs.insert(cost_key("pois", "6/1/3"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 76_000), w0])));
        let b = Book::new(&costs);
        assert_eq!(
            (b.cores("pois", "6/1/1"), b.cores("pois", "6/1/2"), b.cores("pois", "6/1/3")),
            (c(2, Some(9)), c(9, Some(90)), c(9, Some(86))),
            "a busy run climbs"
        );
        // (Task #688: terrain-hi as measured on 26 Oct, 1.16 cores busy of 3 threads, no waits:
        // granted 2, counted for 1.2 (task #697: to the tenth, not rounded down to 1); its step's
        // most, each the most of its runs'.)
        let v = cost_version("terrain-hi");
        costs.insert(
            cost_key("terrain-hi", "6/48/16"),
            Cost { v, ..cost(3700, 1000, &[("threads", 3), ("cpu_ms", 1_160_000), w0]) },
        );
        costs.insert(
            cost_key("terrain-hi", "6/48/17"),
            Cost { v, ..cost(3700, 1000, &[("threads", 2), ("cpu_ms", 1_600_000), w0]) },
        );
        let b = Book::new(&costs);
        assert_eq!(b.cores("terrain-hi", "6/48/16"), c(2, Some(12)));
        assert_eq!(b.cores("terrain-hi", "6/48/17"), c(3, Some(26)), "1.6 of 2: climbs");
        assert_eq!(b.cores("terrain-hi", "6/48/18"), c(2, Some(12)), "its runs at the most threads, 3");
        // (Task #888 review: 2.4 cores busy, its grant settles at 3 whatever it ran with: 3 of 6,
        // of 4, of 3, never climbing.)
        for t in [6, 4, 3] {
            costs.insert(
                cost_key("terrain-hi", "6/48/19"),
                Cost { v, ..cost(3700, 1000, &[("threads", t), ("cpu_ms", 2_400_000), w0]) },
            );
            assert_eq!(Book::new(&costs).cores("terrain-hi", "6/48/19"), c(3, Some(24)), "{t}");
        }
        // (Task #896: 0.3 cores busy of 3 over 1000 s: waits not said, 1; waiting 500 s, 0.6
        // busy while it computed: 2; waiting 900 s, all 3 busy while it computed: it climbs one,
        // counted for its 0.3 and the thread.)
        // (Task #896 review: waiting its whole wall time, or all but half a second, on its 3
        // threads: its 0.3 busy and one more, 1, never a climb.)
        let all = [(Some(1_000_000), 1, 3), (Some(999_500), 1, 3)];
        for (waited, grant, busy) in
            [(None, 1, 3), (Some(500_000), 2, 3), (Some(900_000), 4, 13)].into_iter().chain(all)
        {
            let mut s = vec![("threads", 3), ("cpu_ms", 300_000)];
            s.extend(waited.map(|w| ("waited_ms", w)));
            costs.insert(cost_key("terrain-hi", "6/48/19"), Cost { v, ..cost(3700, 1000, &s) });
            assert_eq!(Book::new(&costs).cores("terrain-hi", "6/48/19"), c(grant, Some(busy)), "{waited:?}");
        }
        costs.remove(&cost_key("terrain-hi", "6/48/19"));
        // (Task #697: a run frozen by the memory guard for 420 of its 1000 s kept 2 cores busy over
        // the 580 s it ran, not 1.16.)
        costs.insert(
            cost_key("terrain-hi", "6/48/16"),
            Cost { v, ..cost(3700, 1000, &[("threads", 3), ("cpu_ms", 1_160_000), ("paused_ms", 420_000), w0]) },
        );
        assert_eq!(Book::new(&costs).cores("terrain-hi", "6/48/16"), c(3, Some(20)));
    }

    /// A target's NAS rate: its own run's bytes (read and written) and creates over its time, else
    /// its step's 90th percentile of each; None with no run measured (none says `bytes`).
    #[test]
    fn a_nas_rate_is_the_targets_run_else_its_steps_90th_percentile() {
        let now = |c: Cost| Cost { v: cost_version("slope"), ..c };
        let mut costs = BTreeMap::new();
        costs.insert(cost_key("slope", "6/1/1"), now(cost(100, 10, &[("bytes", 600), ("wrote", 400), ("creates", 5)])));
        for i in 2..12 {
            costs.insert(cost_key("slope", &format!("6/1/{i}")), now(cost(100, 10, &[("bytes", 100 * i)])));
        }
        costs.insert(cost_key("lo", "3/1/1"), cost(100, 5, &[("threads", 2)]));
        let b = Book::new(&costs);
        assert_eq!(
            b.nas("slope", "6/1/1"),
            Some(crate::budget::nas::Rate { bytes_s: 100.0, creates_s: 0.5, ..Default::default() })
        );
        // (Rates 20..110, and 100 the first's: the 90th percentile of 11 the 10th, 100.)
        assert_eq!(b.nas("slope", "6/9/9").map(|r| r.bytes_s), Some(100.0));
        assert_eq!(b.nas("lo", "3/1/1"), None);
        // (Task #551: what it read of its local copies, `local`, isn't the NAS's.)
        let mut costs = BTreeMap::new();
        let v = cost_version("depth");
        costs.insert(cost_key("depth", "6/1/1"), Cost { v, ..cost(100, 10, &[("bytes", 1000), ("local", 800)]) });
        assert_eq!(Book::new(&costs).nas("depth", "6/1/1").map(|r| r.bytes_s), Some(20.0));
    }

    /// Task #939: a step's 90th percentile weighs each run by its time, so many runs of a second
    /// at a high rate don't make the rate its jobs keep for minutes; a target's load is its rate
    /// with the time it lasts, its own run's, else its time as predicted.
    #[test]
    fn a_steps_nas_percentile_weighs_each_run_by_its_time() {
        let now = |c: Cost| Cost { v: cost_version("slope-deep"), ..c };
        let mut costs = BTreeMap::new();
        // (Eight runs of a second at 200 MB/s, two of 70 s at 10 MB/s: 148 s in all, 140 of them
        // at 10 MB/s.)
        for i in 0..8 {
            costs.insert(cost_key("slope-deep", &format!("10/0/{i}")), now(cost(100, 1, &[("bytes", 200_000_000)])));
        }
        for i in 8..10 {
            costs.insert(cost_key("slope-deep", &format!("10/0/{i}")), now(cost(100, 70, &[("bytes", 700_000_000)])));
        }
        let b = Book::new(&costs);
        assert_eq!(b.nas_prior("slope-deep").map(|r| r.bytes_s), Some(10e6), "run by run it was 200 MB/s");
        // (Its own run's rate and time; a target not run, the step's rate over its predicted time.)
        let own = b.nas_load("slope-deep", "10/0/9", None).unwrap();
        assert_eq!((own.0.bytes_s, own.1), (10e6, Some(70.0)));
        let other = b.nas_load("slope-deep", "10/9/9", None).unwrap();
        assert_eq!(other.0.bytes_s, 10e6);
        assert_eq!(other.1, b.secs_for("slope-deep", "10/9/9", 0, None).map(|s| s as f64));
        // (Equal times: the nearest rank `p90` gives.)
        assert_eq!(p90_by_time((1..=11).map(|i| (i as f64, 1.0)).collect()), Some(10.0));
        assert_eq!(p90_by_time(Vec::new()), None);
    }

    /// A halo step's NAS rate (task #260: `pack`): its job's reads over the time they took (task
    /// #396: `nas_read` over `nas_secs`, the same on each target, so a target of 0 s weighs the
    /// job's real rate, not its reads over one second) and what it writes and creates over the
    /// longer of its own time and the job's, its `bytes` (the halo's base packs' files) not; a measure with `nas_read` alone (b38's,
    /// a share over the target's own time) counts its writes alone. A halo step's batch's disk the
    /// most of its targets', another's their sum.
    #[test]
    fn a_halo_steps_nas_rate_is_its_jobs_reads_and_what_it_writes() {
        let mut costs = BTreeMap::new();
        let c = cost(100, 10, &[("bytes", 1300 << 20), ("wrote", 400), ("creates", 5)]);
        costs.insert(cost_key("pack", "6/1/1"), Cost { v: cost_version("pack"), ..c.clone() });
        // (Read 30 MB in 20 s; this target itself took 0 s.)
        let mut quick = cost(100, 0, &[("bytes", 1300 << 20), ("wrote", 400), ("creates", 5)]);
        quick.sizes.extend([("nas_read".to_string(), 30_000_000), ("nas_secs".to_string(), 20)]);
        costs.insert(cost_key("pack", "6/1/2"), Cost { v: cost_version("pack"), ..quick });
        let mut b38 = c;
        b38.sizes.insert("nas_read".into(), 959 << 20);
        costs.insert(cost_key("pack", "6/1/3"), Cost { v: cost_version("pack"), ..b38 });
        let b = Book::new(&costs);
        assert_eq!(
            b.nas("pack", "6/1/1"),
            Some(crate::budget::nas::Rate { bytes_s: 40.0, creates_s: 0.5, ..Default::default() })
        );
        assert_eq!(
            b.nas("pack", "6/1/2"),
            Some(crate::budget::nas::Rate { bytes_s: 1_500_020.0, creates_s: 0.25, ..Default::default() }),
            "its writes over the job's 20 s, not its own 0 s (pack 6/39/21 weighed the whole NAS)"
        );
        assert_eq!(
            b.nas("pack", "6/1/3"),
            Some(crate::budget::nas::Rate { bytes_s: 40.0, creates_s: 0.5, ..Default::default() }),
            "b38's: writes alone"
        );
        // (On a NAS of 100 MB/s and 40 creates/s: its 1.5 MB/s and 0.25 creates/s, not the whole.)
        let l = crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        assert_eq!(l.weight(b.nas("pack", "6/1/2").unwrap()), 22);
        assert_eq!(batch_disk("pack", [1300, 1300, 1200]), 1300);
        assert_eq!(batch_disk("slope", [1300, 1300, 1200]), 3800);
    }

    /// A step reading a shared cache (task #392: `CACHES`): a target's disk its own output, its
    /// floor at most its step's 90th percentile (the cold run that filled the cache measured it
    /// whole); another step's floor its own, whole.
    #[test]
    fn a_cache_steps_disk_is_its_output_not_the_fill() {
        let costs = BTreeMap::new();
        let mut disks: BTreeMap<String, u64> = (0..20).map(|i| (cost_key("depth", &format!("6/1/{i}")), 300)).collect();
        disks.insert(cost_key("depth", "6/16/20"), 67_890);
        disks.insert(cost_key("depth", "6/1/0"), 100);
        disks.insert(cost_key("peaks", "6/1/1"), 67_890);
        let b = Book::with(&costs, None, Some(&disks));
        assert_eq!(b.disk("depth", "6/16/20", 0), Some(300), "the fill reserved apart, once per Mac");
        assert_eq!(b.disk("depth", "6/1/0", 0), Some(100), "its own when under the step's");
        assert_eq!(b.disk("depth", "6/9/9", 0), Some(300), "never measured: the step's");
        assert_eq!(b.disk("peaks", "6/1/1", 0), Some(67_890), "no shared cache: its own whole");
        assert_eq!(
            (cache_of("pack"), cache_of("lo"), cache_of("depth"), cache_of("slope")),
            (None, None, Some(crate::localread::CACHE), None)
        );
    }

    /// Task #926: with each Mac's pace, a step's runs are read at the lead's pace (a slow Mac's
    /// times and NAS rates no longer drag the step's), and a prediction for a worker is at its
    /// Mac's pace; a target's own run as it ran on its Mac, at another Mac's pace by the two
    /// (task #927); a Mac with no pace at the lead's.
    #[test]
    fn a_steps_runs_are_read_at_the_lead_pace_and_predicted_at_each_macs() {
        let mut costs = BTreeMap::new();
        // (Ten runs on the lead at 100 s, ten on m1 at 300 s, each reading 300 MB.)
        for k in 0..20u32 {
            let (worker, secs) = if k < 10 { ("mini #2", 100) } else { ("m1", 300) };
            let mut c = cost(1000, secs, &[("bytes", 300_000_000)]);
            (c.worker, c.v) = (Some(worker.into()), cost_version("slope-hi"));
            costs.insert(format!("slope-hi 10/{k}/0"), c);
        }
        let raw = Book::new(&costs);
        assert_eq!(raw.secs("slope-hi", "10/99/0", 0), Some(200), "as measured: mixed");
        let paces = crate::paces::Paces {
            lead: "mini".into(),
            steps: [("m1".to_string(), [("slope-hi".to_string(), 3.0)].into())].into(),
            macs: [("m1".to_string(), 3.0)].into(),
            ..Default::default()
        };
        let b = Book::new(&costs).paced(&paces);
        assert_eq!(b.secs("slope-hi", "10/99/0", 0), Some(100), "at the lead's pace");
        assert_eq!(b.secs_for("slope-hi", "10/99/0", 0, Some("mini")), Some(100));
        assert_eq!(b.secs_for("slope-hi", "10/99/0", 0, Some("m1 #2")), Some(300), "at m1's");
        assert_eq!(b.secs_for("slope-hi", "10/99/0", 0, Some("m4")), Some(100), "not measured: the lead's");
        assert_eq!(b.secs_for("slope-hi", "10/12/0", 0, Some("m1")), Some(300), "its own run, on its Mac as it ran");
        assert_eq!(b.secs_for("slope-hi", "10/12/0", 0, Some("mini")), Some(100), "m1's run at the lead's pace");
        assert_eq!(b.secs_for("slope-hi", "10/12/0", 0, Some("m4")), Some(600), "no runs on m4: twice another's");
        assert_eq!(b.secs_p90("slope-hi", "10/99/0", 0, Some("m1")), Some(300));
        // (NAS rates: 3 MB/s on the lead, a third of that on m1.)
        let mb_s = |r: crate::budget::nas::Rate| (r.bytes_s / 1e6).round();
        assert_eq!(raw.nas("slope-hi", "10/99/0").map(mb_s), Some(3.0), "as measured: the lead's p90");
        assert_eq!(b.nas_for("slope-hi", "10/99/0", "mini").map(mb_s), Some(3.0));
        assert_eq!(b.nas_for("slope-hi", "10/99/0", "m1").map(mb_s), Some(1.0), "a third on m1");
        assert_eq!(b.nas_for("slope-hi", "10/15/0", "mini").map(mb_s), Some(1.0), "its own run, as it ran");
    }

    /// Memory that goes with a tile's area fits; a target never run is predicted by the fit over its
    /// own area, capped at the most a run measured.
    #[test]
    fn a_linear_step_fits_and_predicts_over_a_targets_size() {
        let mut costs = BTreeMap::new();
        // (z6 tiles from the equator northward: their areas fall.)
        for y in 0..20u32 {
            let t = format!("6/1/{}", 2 + y);
            let km2 = crate::costin::tile_km2(6, 1, 2 + y);
            costs.insert(format!("bldprep {t}"), cost(100 + km2 / 100, 60, &[]));
        }
        let b = Book::new(&costs);
        let f = &b.fits("bldprep", Measure::Mem)[0];
        assert_eq!(f.size, "km2");
        // (Held out past its training's sizes, a run is capped at their most: a miss or two.)
        assert!(f.good(), "{f:?}");
        let (mb, from) = b.mem("bldprep", "6/1/20", 0, 1);
        assert_eq!(from, From::Measured);
        assert_eq!(mb, 100 + crate::costin::tile_km2(6, 1, 20) / 100);
        // Between the sizes seen: the fit, within its margin.
        let want = (100 + crate::costin::tile_km2(6, 2, 10) / 100) as f64;
        let (mb, from) = b.mem("bldprep", "6/2/10", 0, 1);
        assert_eq!(from, From::Fit);
        assert!((mb as f64) >= want && (mb as f64) <= want * MARGIN + 2.0, "{mb} for {want}");
        // Past them (a tile at the equator's edge is larger than any seen): no more than the most.
        let most = costs.values().map(|c| c.peak_mb).max().unwrap();
        assert_eq!(b.mem("bldprep", "6/1/31", 0, 1), (most, From::Fit));
    }

    /// A step's time prior is the mean of its runs the way it runs now (the expected time: what a
    /// job's targets are counted to the batch length by), not their 90th percentile (task #433: by
    /// that, every `unit` job carried one z6 tile); its memory prior stays the 90th percentile.
    #[test]
    fn a_steps_time_prior_is_the_mean_of_its_runs() {
        let mut costs = BTreeMap::new();
        // (Nine quick extracts and one slow: a heavy tail, as the z6 tiles' times are; a step at cost
        // version 0, as the helper makes its costs.)
        for i in 1..=10u64 {
            costs.insert(format!("peaks 6/{i}/1"), cost(500, if i == 10 { 1200 } else { 100 }, &[]));
        }
        let b = Book::new(&costs);
        assert_eq!(b.prior("peaks", Measure::Secs), Some(210), "the mean: (9 × 100 + 1200) / 10");
        assert_eq!(b.secs("peaks", "6/99/1", 0), Some(210), "an unmeasured target takes it");
        assert_eq!(b.secs("peaks", "6/10/1", 0), Some(1200), "a measured one its own");
        assert_eq!(b.prior("peaks", Measure::Mem), Some(500));
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[1, 2]), Some(2), "rounded up");
        // An update's wait keeps the 90th percentile (a long tail's long jobs): 17 runs of 100 s
        // and 3 of 1200: the mean 265, the 90th percentile 1200; a measured target its own.
        let mut costs = BTreeMap::new();
        for i in 1..=20u64 {
            costs.insert(format!("peaks 6/{i}/1"), cost(500, if i > 17 { 1200 } else { 100 }, &[]));
        }
        let b = Book::new(&costs);
        assert_eq!(b.secs("peaks", "6/99/1", 0), Some(265));
        assert_eq!(b.secs_p90("peaks", "6/99/1", 0, None), Some(1200));
        assert_eq!(b.secs_p90("peaks", "6/1/1", 0, None), Some(100), "a measured one its own");
        assert_eq!(b.secs_p90("pois", "6/1/1", 0, None), None);
    }

    /// Task #445: while a step's runs the way it runs now are fewer than `PRIOR_RUNS` (the quick
    /// ones, first to end), its earlier way's mean stands in when larger; past them, theirs alone.
    #[test]
    fn a_new_ways_first_quick_runs_dont_shrink_the_time_prior() {
        let v = cost_version("depth");
        let mut costs = BTreeMap::new();
        for i in 1..=40u64 {
            costs.insert(format!("depth 6/{i}/1"), Cost { v: v - 1, ..cost(500, 216, &[]) });
        }
        for i in 1..=8u64 {
            costs.insert(format!("depth 6/{i}/2"), Cost { v, ..cost(500, 20, &[]) });
        }
        assert_eq!(Book::new(&costs).prior("depth", Measure::Secs), Some(216), "8 quick runs: the earlier way's");
        for i in 9..=PRIOR_RUNS as u64 {
            costs.insert(format!("depth 6/{i}/2"), Cost { v, ..cost(500, 20, &[]) });
        }
        assert_eq!(Book::new(&costs).prior("depth", Measure::Secs), Some(20), "enough runs: their own mean");
        // (The latest earlier way's alone: an older one's long runs don't count.)
        let mut two = BTreeMap::new();
        two.insert("unit 6/1/1".to_string(), Cost { v: 1, ..cost(500, 5000, &[]) });
        two.insert("unit 6/1/2".to_string(), Cost { v: 2, ..cost(500, 100, &[]) });
        two.insert("unit 6/1/3".to_string(), Cost { v: 3, ..cost(500, 50, &[]) });
        assert_eq!(cost_version("unit"), 3);
        assert_eq!(Book::new(&two).prior("unit", Measure::Secs), Some(100));
        // (A new way slower than the old: its own, the larger.)
        let mut slow = BTreeMap::new();
        slow.insert("depth 6/1/1".to_string(), Cost { v: v - 1, ..cost(500, 10, &[]) });
        slow.insert("depth 6/1/2".to_string(), Cost { v, ..cost(500, 300, &[]) });
        assert_eq!(Book::new(&slow).prior("depth", Measure::Secs), Some(300));
    }

    /// Task #830: the lead's five pyramids-fill z6 tiles with nothing to fetch (17–37 s, as its
    /// costs held on 29 Oct) made the step's time 30 s a z6 tile, and a fill running 3 h read "of ≈
    /// 28 s". A PER_WORK step's time counts its runs by the work each did: those that did none (or
    /// don't say: before the measure) count for nothing, so with only them it has no time of its
    /// own (the first guess stands); its fetching runs weighed by the tiles fetched; a no-op target
    /// keeps its own time.
    #[test]
    fn a_fill_with_nothing_to_fetch_doesnt_set_the_steps_time() {
        let mut costs = BTreeMap::new();
        for (t, s) in [("6/53/27", 37), ("6/54/25", 31), ("6/54/26", 17), ("6/54/27", 35), ("6/55/23", 29)] {
            costs.insert(format!("pyramids-fill {t}"), cost(2900, s, &[("km2", 300_000)]));
        }
        let b = Book::new(&costs);
        assert_eq!(b.prior("pyramids-fill", Measure::Secs), None, "not said: before the measure");
        assert_eq!(b.secs("pyramids-fill", "6/55/25", 2), None);
        assert_eq!(b.secs_p90("pyramids-fill", "6/55/25", 2, None), None);
        assert_eq!(b.secs("pyramids-fill", "6/54/26", 2), Some(17), "its own");
        for i in 1..=10u64 {
            costs.insert(format!("pyramids-fill 6/{i}/1"), cost(2900, 20, &[("fetched", 0), ("km2", 300_000)]));
        }
        assert_eq!(Book::new(&costs).prior("pyramids-fill", Measure::Secs), None, "nothing fetched");
        costs.insert("pyramids-fill 6/56/23".into(), cost(2900, 4950, &[("fetched", 170_612)]));
        costs.insert("pyramids-fill 6/57/23".into(), cost(2900, 60, &[("fetched", 1_000)]));
        let b = Book::new(&costs);
        // ((4950 × 170,612 + 60 × 1,000) / 171,612, rounded up.)
        assert_eq!(b.prior("pyramids-fill", Measure::Secs), Some(4922));
        assert_eq!(b.secs("pyramids-fill", "6/55/25", 2), Some(4922));
        assert_eq!(b.secs_p90("pyramids-fill", "6/55/25", 2, None), Some(4950));
        // (Every other step's runs count alike.)
        assert_eq!(time_weight("peaks", &cost(1, 1, &[])), Some(1.0));
        assert_eq!(weighted_mean(&[]), None);
    }

    /// The lead's queue on 29 Oct: 4,275 terrain-deep targets in Alaska, z10 tiles with a retired
    /// pack and no fine dataset, each a drop (its packs dropped, nothing read), forecast at the
    /// step's 76 s and 7 GB a build. A drop's run (`DROPS`) counts for nothing in its step's model;
    /// its step's drops are modelled apart (`Book::drop_need`).
    #[test]
    fn a_drop_is_no_build_of_its_step() {
        let v = cost_version("terrain-deep");
        let at = |c: Cost| Cost { v, ..c };
        let mut costs = BTreeMap::new();
        for (t, s) in [("10/256/352", 236), ("10/256/353", 200), ("10/257/352", 260)] {
            costs.insert(format!("terrain-deep {t}"), at(cost(7000, s, &[("km2", 900)])));
        }
        let b = Book::new(&costs);
        let (prior_s, prior_mb) = (b.prior("terrain-deep", Measure::Secs), b.prior("terrain-deep", Measure::Mem));
        assert_eq!(b.drop_need("terrain-deep"), None, "no drop ran yet");
        for (t, s) in [("10/100/252", 2), ("10/100/253", 1), ("10/101/228", 3)] {
            costs.insert(format!("terrain-deep {t}"), at(cost(150, s, &[("km2", 900), (DROPS, 1)])));
        }
        let b = Book::new(&costs);
        assert_eq!(b.prior("terrain-deep", Measure::Secs), prior_s, "the builds' time alone");
        assert_eq!(b.prior("terrain-deep", Measure::Mem), prior_mb, "the builds' memory alone");
        assert_eq!(b.secs("terrain-deep", "10/300/300", 0), prior_s);
        assert_eq!(b.drop_need("terrain-deep"), Some((2, 150)));
        // (Another step's first drop: every step's drops' measure, a drop being a drop whatever its
        // step; its own once one ran.)
        assert_eq!(b.drop_need("slope-hi"), Some((2, 150)));
        let sv = cost_version("slope-hi");
        let mut more = costs.clone();
        more.insert("slope-hi 6/10/20".into(), Cost { v: sv, ..cost(90, 5, &[(DROPS, 2)]) });
        assert_eq!(Book::new(&more).drop_need("slope-hi"), Some((5, 90)));
        // (A drop's own measure is its own.)
        assert_eq!(b.secs("terrain-deep", "10/100/252", 0), Some(2));
        assert!(
            is_drop(&costs["terrain-deep 10/100/252"])
                && time_weight("terrain-deep", &costs["terrain-deep 10/100/252"]).is_none()
        );
    }

    /// A poor fit takes the step's prior (its 90th percentile, not one outlier); one never measured
    /// takes the guess; a floor the way it runs now holds its target; an old floor is no measure.
    #[test]
    fn a_poor_fit_takes_the_prior_and_none_the_guess() {
        let mut costs = BTreeMap::new();
        for i in 1..=30u64 {
            costs.insert(format!("peaks 6/{i}/1"), cost(if i % 7 == 0 { 9000 } else { 300 + (i * 37) % 200 }, 5, &[]));
        }
        costs.insert("peaks 6/31/1".into(), cost(40_000, 5, &[]));
        let floors = BTreeMap::from([
            ("peaks 6/50/1".to_string(), Floor { mb: 12_000, alone: true, v: 0 }),
            ("terrain 6/50/1".to_string(), Floor { mb: 12_000, alone: true, v: 0 }),
        ]);
        let b = Book::with(&costs, Some(&floors), None);
        assert!(!b.fits("peaks", Measure::Mem).iter().any(Fit::good));
        assert_eq!(
            b.mem("peaks", "6/40/1", 0, 1),
            (9000, From::Prior),
            "the 90th percentile: neither the outlier nor the floor"
        );
        assert_eq!(b.mem("peaks", "6/50/1", 0, 1), (12_000, From::Measured));
        assert_eq!(b.mem("pois", "6/40/1", 0, 777), (777, From::Table));
        assert_eq!(b.need("pois", "6/40/1", 0, 777).secs, None);
        // (Terrain runs at v3: a v0 floor says nothing of it.)
        assert_eq!(b.mem("terrain", "6/50/1", 0, 1500), (1500, From::Table));
    }

    /// One dense z6 tile measured doesn't set the others': with no good fit, a z6 tile's extract-size guess
    /// comes before its step's prior.
    #[test]
    fn a_dense_unit_leaves_the_others_their_piece_guess() {
        let costs = BTreeMap::from([("6/1/1".to_string(), Cost { v: cost_version("unit"), ..cost(14_000, 600, &[]) })]);
        let b = Book::new(&costs);
        assert_eq!(b.mem("unit", "6/1/2", 0, 3700), (3700, From::Guess));
        assert_eq!(b.mem("peaks", "6/1/2", 0, 3700).1, From::Table);
        // With no guess (a caller that has none), the prior.
        assert_eq!(b.mem("unit", "6/1/2", 0, 0), (14_000, From::Prior));
    }

    /// Threads are an input when the runs differ in them.
    #[test]
    fn threads_are_fitted_when_they_differ() {
        let mut costs = BTreeMap::new();
        for i in 1..=40u64 {
            let t = 4 + (i % 3) * 4;
            costs.insert(
                format!("terrain-lo 3/{i}/1"),
                Cost {
                    v: cost_version("terrain-lo"),
                    ..cost(200 + i * 10 + t * 100, 5, &[("tiffs", i), ("threads", t)])
                },
            );
        }
        let b = Book::new(&costs);
        let f = b.fits("terrain-lo", Measure::Mem).into_iter().find(|f| f.size == "tiffs").unwrap();
        assert!(f.per_thread.is_some_and(|c| (c - 100.0).abs() < 1.0), "{f:?}");
        assert!(f.good());
    }

    /// A kept model is made again once its owner's change count moves, not before.
    #[test]
    fn a_kept_model_lasts_until_the_measures_change() {
        let cache = Cache::default();
        let (f, d) = (BTreeMap::new(), BTreeMap::new());
        let mut costs = BTreeMap::from([("peaks 6/1/1".to_string(), cost(1000, 5, &[]))]);
        assert_eq!(Book::kept(&costs, &f, &d, &cache, 1).prior("peaks", Measure::Mem), Some(1000));
        costs.insert("peaks 6/1/2".into(), cost(5000, 5, &[]));
        assert_eq!(Book::kept(&costs, &f, &d, &cache, 1).prior("peaks", Measure::Mem), Some(1000), "kept");
        assert_eq!(Book::kept(&costs, &f, &d, &cache, 2).prior("peaks", Measure::Mem), Some(5000));
    }

    /// Task #931: a terrain-deep target's memory is its place's (3 GB where its fine data is sparse,
    /// 9 over dense lidar). One not measured takes its nearest measured ones' most, × `MARGIN`, not
    /// the step's 90th percentile, which reserved twice a sparse one's memory and too little for a
    /// dense one; the nearest are the measured ones closest, rings out to `NEAR_RINGS`.
    #[test]
    fn a_target_not_measured_takes_its_places_memory() {
        let v = cost_version("terrain-deep");
        let mut costs = BTreeMap::new();
        // (A 20 × 20 block: dense east of column 310, sparse west of it.)
        for x in 300..320u32 {
            for y in 350..370u32 {
                let mb = if x >= 310 { 9000 + (x + y) as u64 % 7 * 100 } else { 3000 + (x * y) as u64 % 5 * 50 };
                costs.insert(format!("terrain-deep 10/{x}/{y}"), Cost { v, ..cost(mb, 300, &[("tiffs", 3)]) });
            }
        }
        // (Not measured: one deep in each half, one at the edge.)
        for t in ["10/303/360", "10/315/360", "10/310/360"] {
            costs.remove(&format!("terrain-deep {t}"));
        }
        let b = Book::new(&costs);
        let nr = b.model("terrain-deep", Measure::Mem).near.unwrap();
        assert!(nr.good() && nr.runs > 300, "{nr:?}");
        let prior = b.prior("terrain-deep", Measure::Mem).unwrap();
        let (sparse, from) = b.mem("terrain-deep", "10/303/360", 2, 0);
        assert_eq!(from, From::Near);
        assert!(sparse <= (3200.0 * MARGIN).ceil() as u64 && sparse >= 3000, "{sparse} (the prior {prior})");
        let (dense, _) = b.mem("terrain-deep", "10/315/360", 2, 0);
        assert!(dense >= 9000 && dense <= (9600.0 * MARGIN).ceil() as u64, "{dense}");
        // (At the edge its nearest include the dense side: the most, not the mean.)
        assert!(b.mem("terrain-deep", "10/310/360", 2, 0).0 >= 9000);
        // (Measured, its own.)
        let own = costs["terrain-deep 10/301/360"].peak_mb;
        assert_eq!(b.mem("terrain-deep", "10/301/360", 2, 0), (own, From::Measured));
        // (Far from any measured target: the prior.)
        assert_eq!(b.mem("terrain-deep", "10/100/100", 2, 0), (prior, From::Prior));
        // (A floor counts as its tile's measure: a sparse target's neighbour seen holding 12 GB.)
        let floors = BTreeMap::from([("terrain-deep 10/303/361".to_string(), Floor { mb: 12_000, alone: true, v })]);
        let b = Book::with(&costs, Some(&floors), None);
        assert!(b.mem("terrain-deep", "10/303/360", 2, 0).0 >= 12_000);
    }

    /// Where the memory isn't its place's (the same spread, the tiles shuffled), the nearest
    /// predict poorly, held out, and the step's prior stands.
    #[test]
    fn memory_by_place_is_used_only_where_it_predicts() {
        let v = cost_version("terrain-deep");
        let mut costs = BTreeMap::new();
        let mut r = 7u64;
        for x in 300..320u32 {
            for y in 350..370u32 {
                r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let mb = if (r >> 33) % 2 == 0 { 3000 } else { 9000 };
                costs.insert(format!("terrain-deep 10/{x}/{y}"), Cost { v, ..cost(mb, 300, &[]) });
            }
        }
        costs.remove("terrain-deep 10/305/360");
        let b = Book::new(&costs);
        let nr = b.model("terrain-deep", Measure::Mem).near.unwrap();
        assert!(!nr.good(), "{nr:?}");
        assert_eq!(b.mem("terrain-deep", "10/305/360", 2, 0).1, From::Prior);
    }

    /// The nearest wrap around the antimeridian, and stop at the poles.
    #[test]
    fn the_nearest_wrap_around_the_antimeridian() {
        let at = std::collections::HashMap::from([((10u8, 1023u32, 5u32), 4000u64), ((10, 512, 0), 2000)]);
        let n = Near { at, ..Default::default() };
        assert_eq!(n.nearest((10, 0, 5)), Some(4000.0 * MARGIN));
        assert_eq!(n.nearest((10, 510, 0)), Some(2000.0 * MARGIN));
        assert_eq!(n.nearest((10, 500, 500)), None);
        // (z3: eight columns. Its own tile isn't its neighbour, and the one beside it is counted
        // once, on ring 1, not again where ring 7's columns wrap onto it.)
        let at = std::collections::HashMap::from([((3u8, 2u32, 4u32), 9000u64), ((3, 3, 4), 1000)]);
        let n = Near { at, ..Default::default() };
        assert_eq!(n.nearest((3, 2, 4)), Some(1000.0 * MARGIN));
        assert_eq!(n.nearest_by((3, 2, 4), |dx, dy| dx != 1 || dy != 0), None, "its own tile isn't its neighbour");
        let n = Near::of_measures(n.at);
        assert_eq!(n.runs, 1, "{n:?}");
    }

    /// A `GUESS_FIRST` step keeps its piece's guess before its prior: its place is weighed by no
    /// backtest.
    #[test]
    fn a_guess_first_step_keeps_its_guess() {
        let v = cost_version("unit");
        let mut costs = BTreeMap::new();
        for x in 10..30u32 {
            for y in 10..30u32 {
                let mb = if (x * 7 + y * 13) % 3 == 0 { 9000 } else { 3000 };
                costs.insert(format!("6/{x}/{y}"), Cost { v, ..cost(mb, 300, &[]) });
            }
        }
        costs.remove("6/20/20");
        let b = Book::new(&costs);
        assert!(b.model("unit", Measure::Mem).near.is_none());
        assert_ne!(b.mem("unit", "6/20/20", 0, 5000).1, From::Near);
    }

    /// By hand (task #931): each tile target the lead measured, predicted as its coordinator would
    /// have at its grant, from the runs known before it (none of the floors: they carry no time):
    /// as `Book::mem` predicts now (by place where good), and by its fit or prior alone (before
    /// task #931). `SCENIC_MEM_BACKTEST=<folder>` holding `store/` (a copy of the lead's coordinator
    /// store, Application Support/scenic/agent/coord/store) and `history.jsonl` (its
    /// coord/history.jsonl). Prints per step how often each missed (predicted under what the target
    /// took, and by over a GB) and what it reserved against what the targets took; holds
    /// terrain-deep's misses and reservation under the fit's or prior's. Without the folder it
    /// prints that it skipped and checks nothing.
    #[test]
    fn backtest_mem_on_the_leads_measures() {
        use crate::coord::history::Event;
        let Ok(dir) = std::env::var("SCENIC_MEM_BACKTEST") else {
            eprintln!("memory backtest skipped: SCENIC_MEM_BACKTEST unset (no bound checked)");
            return;
        };
        let dir = std::path::Path::new(&dir);
        let store = crate::pool::store::Store::open(&dir.join("store")).unwrap();
        let loaded = crate::coord::kept::Loaded::of(&store.snapshot().docs);
        store.stop();
        let text = std::fs::read_to_string(dir.join("history.jsonl")).unwrap();
        let events: Vec<Event> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        // (Each target's grant: its last lease before its first run that ended well, known from
        // then: a member's done, or the end of the lead's own job its start named it in.)
        let mut leases: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        let mut known: BTreeMap<String, u64> = BTreeMap::new();
        let mut started: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
        for e in &events {
            let Some(step) = e.step.as_deref() else { continue };
            let keys = |ts: &[String]| ts.iter().map(|t| cost_key(step, t)).collect::<Vec<_>>();
            match e.kind.as_str() {
                "lease" => keys(&e.targets).into_iter().for_each(|k| leases.entry(k).or_default().push(e.t)),
                "start" => {
                    started.insert(e.worker.clone().unwrap_or_default(), (step.to_string(), e.targets.clone()));
                }
                "done" | "end" if e.ok == Some(true) => {
                    let ts = match (e.targets.is_empty(), started.get(e.worker.as_deref().unwrap_or(""))) {
                        (true, Some((s, ts))) if s == step => ts.clone(),
                        _ => e.targets.clone(),
                    };
                    for k in keys(&ts) {
                        known.entry(k).or_insert(e.t);
                    }
                }
                _ => {}
            }
        }
        // (Tile targets, measured the way their step runs now; not a GUESS_FIRST step's, whose
        // caller's guess (its extract's) the history doesn't keep.)
        let tiled = |k: &str| tile(k).is_some() && !GUESS_FIRST.contains(&step_of_key(k));
        let costs: Vec<(&String, &Cost)> = loaded
            .costs
            .iter()
            .filter(|(k, c)| tiled(k) && c.v >= cost_version(step_of_key(k)) && !is_drop(c) && known.contains_key(*k))
            .collect();
        let mut asked: Vec<(u64, &String)> = costs
            .iter()
            .filter_map(|(k, _)| leases.get(*k)?.iter().filter(|&&g| g <= known[*k]).max().map(|&g| (g, *k)))
            .collect();
        asked.sort();
        let mut by_known: Vec<(u64, &String, &Cost)> = costs.iter().map(|(k, c)| (known[*k], *k, *c)).collect();
        by_known.sort_by_key(|&(t, k, _)| (t, k));
        // (Per step: targets; missed and by over a GB, MB predicted: now's, then the fit's or
        // prior's; MB taken; where now's came from.)
        #[derive(Default, Debug)]
        struct Tally {
            n: usize,
            miss: [usize; 2],
            big: [usize; 2],
            pred: [f64; 2],
            took: f64,
            from: BTreeMap<String, usize>,
        }
        let mut tally: BTreeMap<String, Tally> = BTreeMap::new();
        let mut book: BTreeMap<String, Cost> = BTreeMap::new();
        let mut next = 0;
        // (A minute's grants at a time, from the runs known as it began: a coordinator knows a run
        // as it ends. Task #931 review: by the hour, a target granted late in it was predicted
        // without the neighbours that ended earlier in it, which a batch's next target has; so
        // replayed, terrain-hi missed 8.1 % by place against 5.0 % by its prior, by the minute 4.3
        // % against 4.9 %.)
        for hour in asked.chunk_by(|a, b| a.0 / 60 == b.0 / 60) {
            let from = hour[0].0 / 60 * 60;
            while next < by_known.len() && by_known[next].0 < from {
                book.insert(by_known[next].1.clone(), by_known[next].2.clone());
                next += 1;
            }
            let b = Book::new(&book);
            for (_, k) in hour {
                let step = step_of_key(k);
                let target = k.rsplit(' ').next().unwrap_or_default();
                let took = loaded.costs[*k].peak_mb as f64;
                let (now, from_now) = b.mem(step, target, 0, 0);
                let md = b.model(step, Measure::Mem);
                let Some(before) = b.fitted(&md, k, 0).or(md.prior) else { continue };
                let t = tally.entry(step.to_string()).or_default();
                t.n += 1;
                t.took += took;
                *t.from.entry(format!("{from_now:?}")).or_default() += 1;
                for (i, p) in [now as f64, before as f64].into_iter().enumerate() {
                    t.miss[i] += usize::from(p < took);
                    t.big[i] += usize::from(took - p > 1024.0);
                    t.pred[i] += p;
                }
            }
        }
        let pc = |a: usize, n: usize| a as f64 * 100.0 / n.max(1) as f64;
        for (s, t) in &tally {
            eprintln!(
                "{s:<14} {:>6} targets: missed {:>5.1} % (by over a GB {:>4.1} %), reserved {:.2}× | before: {:>5.1} % ({:>4.1} %), {:.2}× | {:?}",
                t.n,
                pc(t.miss[0], t.n),
                pc(t.big[0], t.n),
                t.pred[0] / t.took,
                pc(t.miss[1], t.n),
                pc(t.big[1], t.n),
                t.pred[1] / t.took,
                t.from
            );
        }
        if let Some(t) = tally.get("terrain-deep") {
            assert!(t.miss[0] < t.miss[1] && t.big[0] < t.big[1] && t.pred[0] < t.pred[1], "{t:?}");
        }
    }

    #[test]
    fn tiles_of_keys() {
        assert_eq!(tile("tail 6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("marks"), None);
    }
}
