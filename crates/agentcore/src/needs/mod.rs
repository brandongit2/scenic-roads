//! What a job of a step needs (task #236): one model per step, read by every placement path (the
//! coordinator's picks, its floors, the forecast). Nothing here is a figure kept by hand: a need is
//! measured (the target's own run, its floor), its place's (its nearest measured targets), the
//! step's needs learned from its runs (`model`, task #1416), or the caller's first guess (the
//! steps table's, the size its plan offered, a z6 tile's extract-size guess).
//!
//! Only what was measured the way the step runs now counts (crate::coord::cost_version, its memory
//! and time): a run or a floor of an older way of it (a z6 tile's before its v2, whose measures held
//! malloc's cached freed blocks) is neither a measure nor one of the model's runs, and bounds no run
//! of the way it runs now (only a cold start's packing reads it, `Book::older_mem`, below). Disk
//! floors carry no version and always count.
//!
//! - **Measured**: the target's own run, never below its floor (crate::agent::memguard::Floor), its
//!   memory times its step's spread over its reruns about its peak (`Reruns::spread`); a floor alone counts as its
//!   measure. Its time on another Mac at that Mac's speed (crate::paces).
//! - **Near** (memory, a tile target; task #931): the most its nearest measured targets of the step
//!   took (`Near`: `NEAR_RUNS` within `NEAR_RINGS` rings of tiles), × `MARGIN`, where that's judged
//!   good on the step's own measures (each predicted from the others). What a target holds is its
//!   place's, whichever Mac runs it (terrain-deep's neighbouring tiles differ by a median 6–8 % run
//!   on two Macs, no Mac's the higher by more than 2 %, and 2 % run on one, measured on the lead's
//!   runs of 15 Oct – 4 Nov).
//! - **Model** (`model::Model`): the step's needs by the size of the target's input, learned from
//!   its runs whose job's record is clean: its work and the rest of its time per unit (a low
//!   percentile of its runs of about its size), its parallelism, its memory and disk (a high
//!   percentile of the runs its size or more), its NAS bytes per unit; its time on a Mac its work at
//!   that Mac's speed over the cores it can use, plus the rest.
//! - **Guess first** (z6 tiles and landmark candidates, `GUESS_FIRST`): while the model has fewer than
//!   `MIN_RUNS` runs, the caller's guess scaled by the target's extract comes before it, so one dense
//!   z6 tile doesn't set every other's need.
//! - **Table**: the caller's guess, when nothing of the step was measured (a cold start: the
//!   coordinator packs it by the step's runs of its latest earlier way, `Book::older_mem`, else
//!   its offered guess; with neither it runs alone, crate::coord::sched). That figure only packs
//!   a cold start beside other jobs: it never holds one off a Mac (a cold start is never too big).
//!
//! A step offered by an input measure (`OFFERED_BY`: the 3D buildings by their rows) has no memory
//! guess: what its caller passes as the guess is that measure of the target, a size the model reads
//! for a target not run yet.

pub mod model;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::agent::memguard::{step_of_key, Floor};
use crate::agent::seam::Plan;
use crate::coord::{cost_key, Cost};

/// The margin over the most a target's nearest measured targets took (`Near`).
pub const MARGIN: f64 = 1.1;
/// The fewest runs a step's model reads a size, an envelope or a check from, and memory by place
/// is judged on.
pub const MIN_RUNS: usize = 10;
/// The share of its measures memory by place may predict under before it's poor.
pub const MAX_MISS: f64 = 0.05;
/// The median error (|predicted − measured| / measured) past which memory by place is poor.
pub const MAX_ERR: f64 = 0.5;
/// The steps whose caller's guess scales with the target (a z6 tile's extract; candidates read the
/// same extract) and so comes before the step's prior while no fit is good.
pub const GUESS_FIRST: [&str; 2] = ["unit", "pois"];

/// The steps whose plan offers each target by an input measure of it rather than a memory guess,
/// with the measure (buildstore::costin): the 3D buildings by the rows their tile's sources hold
/// (bld::bld::sources::Sources::rows_for, the plan's "bldprep-rows" input). Their runs note the
/// rows they read (bldprep the sources' rows of its tile; bldtiles its normalized files', at most
/// those), so the step's fit over them predicts a target from its offer before it runs.
pub const OFFERED_BY: [(&str, &str); 2] = [("bldprep", "rows"), ("bldtiles", "rows")];

/// The measure `step`'s targets are offered by (`OFFERED_BY`), if any.
pub fn offered_by(step: &str) -> Option<&'static str> {
    OFFERED_BY.iter().find(|(s, _)| *s == step).map(|(_, m)| *m)
}

/// The steps whose job opens one halo of base packs for all its targets (task #260: `pack`, a
/// z6 tile's map tiles together; task #381: `lo`, a pass of the low zooms, its z3 tiles' owners'): the
/// parts of the base packs its targets need are read from the NAS once, into the job's scratch
/// (scenic-build's `open_for_tiles`, task #396), and every target reads them there. So a target's
/// disk measure is the job's reads, shared, not its own: a job of several takes the most of theirs,
/// not their sum (`batch_disk`); and a target's `bytes` (the halo's base-pack files' bytes, its
/// input size for the fits) aren't what it reads from the NAS: its NAS weight is what the job read
/// (`nas_read` over `nas_secs`, the job's reads and their time, on each of its targets) and what it writes (`wrote`, `creates`) and reads back (`read_back`).
pub const HALO_STEPS: [&str; 2] = ["pack", "lo"];

/// The steps whose time is the work a target finds to do, not the target itself (task #830), with
/// the measure (buildstore::costin) that counts it: a pyramids-fill z6 tile whose tiles were all stored
/// already took 17–37 s, one fetching 170,000 of them 4,950 s, and five such no-op runs made the
/// step's time 30 s a z6 tile (a fill running 3 h shown as "of ≈ 28 s"). Its runs count in its step's
/// time weighted by that work (`time_weight`); a run that did none, or doesn't say, counts for
/// nothing there (its own target keeps its own time).
pub const PER_WORK: [(&str, &str); 1] = [("pyramids-fill", "fetched")];

/// The measure (buildstore::costin) a "none" target's run notes, the packs it dropped (crate::agent::build's
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
/// need is its own output (`Book::disk`: a measure past its step's model's taken for a
/// fill), and the cache is reserved once per Mac, not once per job (crate::coord's grants:
/// `cache_mb`). (`pack` and `lo` read only the parts of the base packs they need, into their own
/// scratch, since Task #396: their disk is their own.) The one shared cache: the whole local copies
/// of the basemap and the depth sources' packs the depth's and the water's steps read
/// (buildstore::localread, task #551), copied only where the Mac has room for it past its room target,
/// else read in place: reserved only on a Mac with that room (crate::coord's `missing`). Its steps
/// are those whose jobs open the copies, held to the code by pipeline's tests/cache_steps.rs (task
/// #1173).
pub const CACHES: [(&str, &[&str]); 1] =
    [(buildstore::localread::CACHE, &["depth", "depth-lo", "terrain-water", "lakefill", "water"])];

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
    /// Its step's model (`model::Model`).
    Model,
    /// The caller's guess for a `GUESS_FIRST` step that has measures (so not a cold start).
    Guess,
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

/// A step's models as made from its runs: its needs (`model::Model`), its memory by place (`Near`,
/// not a `GUESS_FIRST` step's), and its latest earlier way's (`cost_version`) needs, made of that
/// way's runs alike (`Book::older_mem`).
#[derive(Debug, Default)]
pub struct Models {
    pub needs: model::Model,
    pub near: Option<Arc<Near>>,
    older: Option<(u32, model::Model)>,
    /// What its targets' reruns took against their runs before (`Reruns`): a target's own run's
    /// peak is booked times their spread about it.
    pub reruns: Reruns,
}

/// A step's reruns (task #1586): of each target's run the way its step runs now that reran one of
/// that way on as many threads (`Cost::was_mb`), that run's peak (MB) and its peak over it, by the
/// run before's peak; with the percentile its spread is read at (the pool's `needs-mem-pct`).
#[derive(Debug, Default)]
pub struct Reruns {
    by_was: Vec<(f64, f64)>,
    pct: f64,
}

impl Reruns {
    fn of(runs: impl Iterator<Item = (f64, f64)>, pct: f64) -> Reruns {
        let mut by_was: Vec<(f64, f64)> = runs.collect();
        by_was.sort_by(|a, b| a.0.total_cmp(&b.0));
        Reruns { by_was, pct }
    }

    /// What a target's next run takes against its run that peaked at `mb` (MB), at most: the
    /// percentile of the ratios of the step's reruns whose runs before peaked at about as much
    /// (half to twice it; fewer than `MIN_RUNS`, the nearest of those that peaked at half of it or
    /// more), never under 1 (1 with none). A target's peak moves run to run by its level: depth's
    /// reruns took up to 3.6 times their runs before (6/20/17: 564 MB, then 1,605); pack's of tens
    /// of MB up to 30 times (its base packs fetched or not), of 3 GB about as much again. So a
    /// lighter run's swing never books a heavier target: one spread for every level booked a 3.8 GB
    /// pack at 36 GB.
    pub fn spread(&self, mb: f64) -> f64 {
        let mb = mb.max(1.0);
        // (Sorted by their runs before's peaks: those from half of it up, the nearest first.)
        let above = &self.by_was[self.by_was.partition_point(|r| r.0 < mb / 2.0)..];
        let about = above.partition_point(|r| r.0 <= mb * 2.0);
        let near: Vec<f64> = above[..about.max(MIN_RUNS.min(above.len()))].iter().map(|r| r.1).collect();
        model::pct(&model::sorted(near), self.pct).map_or(1.0, |s| s.max(1.0))
    }
}

/// The models made, kept until the measures they're made from change: the change count they were
/// made at (the owner's: the coordinator bumps its count with each cost, floor or disk floor it
/// takes, and with each Mac's speeds and records it reads).
#[derive(Debug, Default)]
pub struct Cache {
    models: Mutex<(u64, roadcore::fx::FxMap<String, Arc<Models>>)>,
    /// Each target's `Book::nas` (step → target → it): the NAS budget weighs each target offered
    /// and leased for every ask.
    rates: Mutex<(u64, Rates)>,
    /// Each `Book::secs_on` asked (step → target → threads and worker): a placement sizes every
    /// target it weighs by it, and weighs every job leased at each ask by its targets' times.
    secs: Mutex<(u64, ByTarget<Vec<SecsAsked>>)>,
    /// Each target's own run's NAS rate and time (`Book::own_rate`, step → target): the NAS budget
    /// weighs every target offered and leased by it for each offer an ask weighs.
    own_rates: Mutex<(u64, ByTarget<Option<(jobrt::budget::nas::Rate, f64)>>)>,
    /// Each target's `Book::source` (step → target): every run read and every target predicted
    /// asks it, and its sources' names are worked out from the steps table's reads.
    sources: Mutex<(u64, ByTarget<Option<f64>>)>,
}

/// What `Cache` keeps of each target: step → target → it (looked up without making a key).
type ByTarget<T> = roadcore::fx::FxMap<String, roadcore::fx::FxMap<String, T>>;

/// A `Book::secs_on` kept (`Cache`): its threads and worker, and its answer.
type SecsAsked = ((u32, Option<String>), Option<(u64, bool)>);

/// `Cache`'s targets' NAS rates: step → target → `Book::nas`.
type Rates = ByTarget<Option<jobrt::budget::nas::Rate>>;

/// What the lead's timings records say of each job run (jobrt::timings::JobFacts), by its id: the
/// model learns from a run only where its job's record, if the lead's log still holds it, is clean.
pub type Records = BTreeMap<String, jobrt::timings::JobFacts>;

/// What a Book reads its costs by besides them, as the pool has them: each Mac's speed
/// (crate::paces), the jobs' records (`Records`) and the needs model's tuning (the pool's settings,
/// jobrt::needs::KNOBS). The scheduler holds them (crate::coord::sched::Sched, and its `State` for
/// the forecast); `Book::pool` and `Book::kept` take them.
#[derive(Clone, Copy)]
pub struct Inputs<'a> {
    pub paces: &'a crate::paces::Paces,
    pub records: &'a Records,
    pub tuning: jobrt::needs::Tuning,
}

/// The measures needs are read from: what each target cost (crate::coord::Cost), and, where kept,
/// the floors (crate::agent::memguard) and disk floors (MB, by cost key), each Mac's time against
/// the lead's (crate::paces, from the Macs' speeds), the jobs' records (`Records`) and the model's
/// percentiles and windows (jobrt::needs::Tuning, the pool's settings); the models made from them
/// kept in `cache` as of change `rev` (a Book of its own, `new`, keeps its own).
pub struct Book<'a> {
    pub the_plan: &'static dyn Plan,
    pub costs: &'a BTreeMap<String, Cost>,
    pub floors: Option<&'a BTreeMap<String, Floor>>,
    pub disks: Option<&'a BTreeMap<String, u64>>,
    /// Each Mac's time against the lead's, by their speeds (crate::paces, task #1418), where known:
    /// every run's CPU is read in the lead's CPU seconds by its worker's, and a prediction for a
    /// worker is at its Mac's. None: every Mac the lead's.
    pub paces: Option<&'a crate::paces::Paces>,
    pub records: Option<&'a Records>,
    pub tuning: jobrt::needs::Tuning,
    cache: CacheRef<'a>,
    rev: u64,
}

enum CacheRef<'a> {
    Own(Cache),
    Kept(&'a Cache),
}

/// Target `target` of `step`'s own run in `costs`, its measure for every reader (its memory, its
/// time, the coordinator's sizing, the forecast's cost table and run): its cost the way its step
/// runs now (crate::coord::cost_version). One of an older way says nothing of it (task #1165: the
/// forecast took terrain-deep runs of cost version 0, 0-8 s runs that found no tiffs, as known,
/// where the coordinator sized the same targets by the step's runs at minutes a target).
pub fn own<'c>(the_plan: &dyn Plan, costs: &'c BTreeMap<String, Cost>, step: &str, target: &str) -> Option<&'c Cost> {
    costs.get(&cost_key(step, target)).filter(|c| c.v >= the_plan.cost_version(step))
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

/// The size a target's source's time gives (`Book::source`, task #927): one plus the seconds, at
/// the lead's pace, the same target of the first step it reads that names it took (slope-deep's
/// terrain-deep, slope-hi's terrain-hi), the way that step runs now. It says what the target
/// holds before it runs: on the lead's runs to 5 Nov, a slope-deep target whose terrain-deep took
/// under 2 s took a median 0 s, one whose terrain-deep took 64-127 s a median 57 (the log of one's
/// time against the other's: correlation 0.92).
pub const SOURCE: &str = "source_s";

/// The sizes known of a cost (its measured `sizes`; its tile's area when it has no `km2`): not its
/// threads, nor the times its run took (`…_ms`: its CPU, its time stopped, its waits, whatever
/// their name), which measure the run, not its work.
fn sizes_of(key: &str, c: Option<&Cost>) -> BTreeMap<String, f64> {
    let mut m: BTreeMap<String, f64> = c
        .map(|c| {
            c.sizes
                .iter()
                .filter(|(k, _)| {
                    !matches!(k.as_str(), "threads" | "threads_least" | "threads_peak") && !k.ends_with("_ms")
                })
                .map(|(k, v)| (k.clone(), *v as f64))
                .collect()
        })
        .unwrap_or_default();
    if let (false, Some((z, x, y))) = (m.contains_key("km2"), tile(key)) {
        m.insert("km2".into(), buildstore::costin::tile_km2(z, x, y) as f64);
    }
    m
}

/// A run's NAS rate (what it asked of the NAS: `bytes` less what of them it read from this Mac's
/// own disk, `local` but its `held` (its whole local copies' reads, task #551, are no NAS reads; a
/// NAS read the page cache answered, task #1450, is asked of the NAS all the same: a warm cache
/// isn't a small need, task #1417), plus `wrote` and `read_back`, and `creates`, over its time; its metadata work,
/// `listed`, `stats` and `deletes`, likewise), when it was measured the
/// way its step runs now and says its `bytes`. A `HALO_STEPS` step's reads are its job's, not its
/// own: `nas_read` over `nas_secs` (the job's reads and the time they took, the same on each of its
/// targets), beside what it writes over the longer of its own time and `nas_secs`; a measure without `nas_secs` (task #396's
/// first, b38: a share of the job's reads over the target's own time, often 0 s, so every job
/// weighed the whole NAS) counts its writes alone. With the time (s) it is over: a job's rate is its
/// targets' moves over their times together (`Book::nas_load`).
fn nas_rate(the_plan: &dyn Plan, step: &str, c: &Cost) -> Option<(jobrt::budget::nas::Rate, f64)> {
    let (m, secs) = moves_of(step, c).filter(|_| c.v >= the_plan.cost_version(step))?;
    let r = jobrt::budget::nas::Rate {
        bytes_s: m.bytes / secs,
        creates_s: m.creates / secs,
        listed_s: m.listed / secs,
        stats_s: m.stats / secs,
        deletes_s: m.deletes / secs,
    };
    Some((r, secs))
}

/// What run `c` of `step` moved on the NAS (`model::Moves`), as `nas_rate` counts it, and the time
/// (s) it moved them over, so the one over the other is its rate: a `HALO_STEPS` step's reads its
/// job's (`nas_read`) at their own rate (over `nas_secs`), beside its writes, over the longer of
/// its own time and `nas_secs` (task #1510: its job's reads over its own second or two read a lo
/// pass at 53 GB/s). None where it doesn't say its `bytes`.
fn moves_of(step: &str, c: &Cost) -> Option<(model::Moves, f64)> {
    if !c.sizes.contains_key("bytes") {
        return None;
    }
    let g = |k: &str| c.sizes.get(k).copied().unwrap_or(0) as f64;
    // (A halo step's writes are spread over its job's time too, not over its own write phase:
    // pack's `secs` is that phase's 0-1 s, which weighed a pack at the whole NAS, 25 Oct.)
    let halo = HALO_STEPS.contains(&step);
    let secs = if halo { c.secs.max(c.sizes.get("nas_secs").copied().unwrap_or(0)) } else { c.secs }.max(1) as f64;
    let read = if halo {
        c.sizes.get("nas_secs").filter(|&&s| s > 0).map_or(0.0, |&s| g("nas_read") * secs / s as f64)
    } else {
        (g("bytes") - g("local") + g("held")).max(0.0)
    };
    // (What its uploads read back to check them: as measured, else, on a line from before it was
    // (task #1142), its `wrote`: each file costin counts made is a store::naming upload, read
    // back whole once it's written, as every one has been since the store began. So a step's old
    // measures weigh it as its new ones do, and no `cost_v` is bumped for it.)
    let back = c.sizes.get("read_back").map_or(g("wrote"), |&b| b as f64);
    let m = model::Moves {
        bytes: read + g("wrote") + back,
        creates: g("creates"),
        listed: g("listed"),
        stats: g("stats"),
        deletes: g("deletes"),
    };
    Some((m, secs))
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

/// How many runs the way a step runs now its model takes alone (`model::Model::of`); fewer, its
/// latest earlier way's work and waits per unit stand in where larger.
pub const PRIOR_RUNS: usize = 20;

impl<'a> Book<'a> {
    /// A Book over `costs` alone, at no Mac's speed, of no records, at the default tuning (a
    /// test's).
    #[cfg(test)]
    pub fn new(the_plan: &'static dyn Plan, costs: &'a BTreeMap<String, Cost>) -> Self {
        Book::with(the_plan, costs, None, None)
    }

    /// A Book over `costs` (and `disks`, the disk measures, where it has them) read as the pool
    /// reads them (`Inputs`: each Mac's speed, the jobs' records, the pool's tuning), keeping its
    /// own models: every Book outside the scheduler's (`kept`) is one, so the knobs take effect as
    /// set everywhere.
    pub fn pool(
        the_plan: &'static dyn Plan,
        costs: &'a BTreeMap<String, Cost>,
        disks: Option<&'a BTreeMap<String, u64>>,
        inputs: Inputs<'a>,
    ) -> Self {
        Book::with(the_plan, costs, None, disks).inputs(inputs)
    }

    /// A Book over all three, keeping its own models.
    fn with(
        the_plan: &'static dyn Plan,
        costs: &'a BTreeMap<String, Cost>,
        floors: Option<&'a BTreeMap<String, Floor>>,
        disks: Option<&'a BTreeMap<String, u64>>,
    ) -> Self {
        Book {
            the_plan,
            costs,
            floors,
            disks,
            paces: None,
            records: None,
            tuning: Default::default(),
            cache: CacheRef::Own(Cache::default()),
            rev: 0,
        }
    }

    /// The scheduler's Book (crate::coord::sched::Sched::book): its floors and disk floors too,
    /// read as the pool reads them (`Inputs`), its models kept in `cache` while its owner's change
    /// count is `rev` (which moves with each of them).
    pub fn kept(
        the_plan: &'static dyn Plan,
        costs: &'a BTreeMap<String, Cost>,
        floors: &'a BTreeMap<String, Floor>,
        disks: &'a BTreeMap<String, u64>,
        inputs: Inputs<'a>,
        cache: &'a Cache,
        rev: u64,
    ) -> Self {
        Book {
            cache: CacheRef::Kept(cache),
            rev,
            ..Book::with(the_plan, costs, Some(floors), Some(disks)).inputs(inputs)
        }
    }

    fn inputs(self, i: Inputs<'a>) -> Self {
        self.paced(i.paces).learned(i.records, i.tuning)
    }

    /// Its runs read, and its predictions made, at each Mac's time by `paces`.
    fn paced(mut self, paces: &'a crate::paces::Paces) -> Self {
        self.paces = Some(paces);
        self
    }

    /// Its runs learned from as their jobs' records say (`Records`), at the percentiles and windows
    /// `tuning` says.
    fn learned(mut self, records: &'a Records, tuning: jobrt::needs::Tuning) -> Self {
        self.records = Some(records);
        self.tuning = tuning;
        self
    }

    /// `worker`'s Mac's time against the lead's (1: not known, or no paces).
    fn ratio(&self, worker: Option<&str>) -> f64 {
        self.paces.zip(worker).map_or(1.0, |(p, w)| p.get(w))
    }

    fn cache(&self) -> &Cache {
        match &self.cache {
            CacheRef::Own(c) => c,
            CacheRef::Kept(c) => c,
        }
    }

    /// Target `target` of `step`'s own run in its costs (`own`).
    pub fn own(&self, step: &str, target: &str) -> Option<&'a Cost> {
        own(self.the_plan, self.costs, step, target)
    }

    fn floor(&self, key: &str) -> Option<&Floor> {
        self.floors.and_then(|f| f.get(key))
    }

    /// The floor of `key` learned the way its step runs now (MB).
    fn current_floor(&self, step: &str, key: &str) -> Option<u64> {
        self.floor(key).filter(|f| f.v >= self.the_plan.cost_version(step)).map(|f| f.mb)
    }

    /// The step's runs of way `v` (its costs of it, drops left out), as the model reads them
    /// (`model::Obs`): those whose job's record, where kept, isn't clean left out too.
    fn obs(&self, step: &str, v: u32) -> Vec<model::Obs> {
        let of_step: Vec<(&String, &Cost)> =
            self.costs.iter().filter(|(k, c)| step_of_key(k) == step && c.v == v && !is_drop(c)).collect();
        // (How many targets each job ran: its record's instructions are a lone target's alone.)
        let mut of_job: BTreeMap<jobrt::jobid::JobId, f64> = BTreeMap::new();
        for (_, c) in &of_step {
            if let Some(j) = c.job {
                *of_job.entry(j).or_default() += 1.0;
            }
        }
        let lead_speed = self.paces.and_then(|p| p.lead_ginsn_s());
        let mut out = Vec::new();
        for (k, c) in of_step {
            let fact = c.job.and_then(|j| self.records?.get(&j.to_string()));
            if fact.is_some_and(|f| !f.clean) {
                continue;
            }
            let g = |n: &str| c.sizes.get(n).copied().unwrap_or(0) as f64;
            let ratio = self.ratio(c.worker.as_deref());
            let cpu_s = c.sizes.get("cpu_ms").map(|&ms| ms as f64 / 1000.0);
            // (Its CPU in the lead's CPU seconds: its share of its job's instructions over the
            // lead's speed where its record counted them, else its CPU at the lead's speed.)
            let counted = fact.zip(lead_speed).filter(|(f, _)| f.ginsn > 0.0 && f.cpu_s > 0.0);
            let work = match (counted, cpu_s) {
                (Some((f, speed)), Some(cpu)) => Some(f.ginsn * (cpu / f.cpu_s).min(1.0) / speed),
                (Some((f, speed)), None) if c.job.and_then(|j| of_job.get(&j)).is_some_and(|&n| n <= 1.0) => {
                    Some(f.ginsn / speed)
                }
                (_, cpu) => cpu.map(|s| s / ratio),
            };
            let ran = (c.secs as f64 - g("paused_ms") / 1000.0).max(0.0);
            let wait = (g("nas_net_ms") / 1000.0).min(ran);
            let mut sizes = sizes_of(k, Some(c));
            let target = crate::coord::CostKey::parse(k).target;
            if let Some(s) = self.source(step, target) {
                sizes.insert(SOURCE.to_string(), s);
            }
            sizes.insert(model::TARGET.to_string(), 1.0);
            let wall = (ran - wait) / ratio + wait;
            // (Its NAS moves over their own time: its wall time, a halo step's the longer of it
            // and its job's reads' (`moves_of`), at the lead's pace.)
            let nas = moves_of(step, c).map(|(m, s)| (m, if HALO_STEPS.contains(&step) { s / ratio } else { wall }));
            out.push(model::Obs {
                order: c.job.map_or(0, |j| j.n),
                key: k.clone(),
                sizes,
                threads: peak_threads(c),
                work,
                wait,
                wall,
                peak: c.peak_mb.max(self.current_floor(step, k).unwrap_or(0)),
                nas,
                explain: fact.map_or(1.0, |f| f.explain.max(1.0)),
                timed: time_weight(step, c).is_some(),
            });
        }
        out
    }

    /// The step's models: made, or as kept since the measures last changed.
    pub fn model(&self, step: &str) -> Arc<Models> {
        let cache = self.cache();
        {
            let mut g = cache.models.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != self.rev {
                *g = (self.rev, Default::default());
            }
            if let Some(m) = g.1.get(step) {
                return m.clone();
            }
        }
        let v = self.the_plan.cost_version(step);
        // (Its latest earlier way's runs: their work stands in while the way it runs now has few.)
        let of = |k: &String, c: &Cost| step_of_key(k) == step && c.v < v && !is_drop(c);
        let way = self.costs.iter().filter(|(k, c)| of(k, c)).map(|(_, c)| c.v).max();
        let older_obs = way.map(|w| self.obs(step, w)).unwrap_or_default();
        let older = way.map(|w| (w, model::Model::of(older_obs.clone(), &[], &[], &self.tuning)));
        // (Not a `GUESS_FIRST` step's: its guess scales with its piece, and no backtest has weighed
        // its place against that guess, which the history doesn't keep.)
        let near = (!GUESS_FIRST.contains(&step)).then(|| Arc::new(Near::of_measures(self.places(step))));
        // (Its disk measures, each by its target's sizes: those of a target whose cost line says
        // an earlier way left out, as its memory is; a disk floor carries no way of its own, so one
        // with no cost line counts.)
        let disks: Vec<model::Disk> = self
            .disks
            .into_iter()
            .flatten()
            .filter(|(k, _)| step_of_key(k) == step && self.costs.get(*k).is_none_or(|c| c.v >= v))
            .map(|(k, &d)| {
                let mut sizes = sizes_of(k, self.costs.get(k));
                sizes.insert(model::TARGET.to_string(), 1.0);
                (sizes, d)
            })
            .collect();
        let needs = model::Model::of(self.obs(step, v), &older_obs, &disks, &self.tuning);
        let reruns = self.costs.iter().filter(|(k, c)| step_of_key(k) == step && c.v == v && !is_drop(c)).filter_map(
            |(_, c)| {
                let was = c.was_mb.filter(|&w| w > 0)? as f64;
                Some((was, c.peak_mb as f64 / was))
            },
        );
        let reruns = Reruns::of(reruns, self.tuning.mem_pct);
        let m = Arc::new(Models { needs, near, older, reruns });
        cache.models.lock().unwrap_or_else(|e| e.into_inner()).1.insert(step.to_string(), m.clone());
        m
    }

    /// The step's tile targets' memory measured the way it runs now, by tile (MB): each one's run,
    /// or its floor, the larger (a drop's measures say nothing of its builds).
    fn places(&self, step: &str) -> std::collections::HashMap<(u8, u32, u32), u64> {
        let v = self.the_plan.cost_version(step);
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

    /// What the latest earlier way of `step` (`cost_version`) measured `target` at (MB), for a cold
    /// start after its way changed (task #979): that way's needs' memory for the target (its runs
    /// at or above its size, as the way it runs now reads its own: `model::Model::mem`), or the
    /// target's own run of it, the larger; None when no run of an earlier way measured its memory.
    /// Floors of an earlier way stay out (one held malloc's cached freed blocks, task #499), and so
    /// do drops.
    pub fn older_mem(&self, step: &str, target: &str) -> Option<u64> {
        let md = self.model(step);
        let (way, older) = md.older.as_ref()?;
        let own = self.costs.get(&cost_key(step, target)).filter(|c| c.v == *way && !is_drop(c)).map(|c| c.peak_mb);
        let by_size = older.mem(&self.sizes_by(step, target, None, |v| v == *way), 0);
        by_size.into_iter().chain(own).max().filter(|&mb| mb > 0)
    }

    /// The sizes target `target` of `step` is predicted by: its own earlier run's (of any way:
    /// what it read, task #1507's lo 3/4/2; of an earlier way, not what it made, `MADE`), else its
    /// tile's area; the measure it was offered by (`offered`, `OFFERED_BY`) where its run doesn't
    /// say it; one target.
    fn sizes(&self, step: &str, target: &str, offered: Option<(&str, u64)>) -> BTreeMap<String, f64> {
        let v = self.the_plan.cost_version(step);
        self.sizes_by(step, target, offered, |w| w >= v)
    }

    /// `sizes` as a model of the ways `way` holds for reads them: the target's run of one of those
    /// gives every size it measured, a run of another what it read alone.
    fn sizes_by(
        &self,
        step: &str,
        target: &str,
        offered: Option<(&str, u64)>,
        way: impl Fn(u32) -> bool,
    ) -> BTreeMap<String, f64> {
        let key = cost_key(step, target);
        let ran = self.costs.get(&key);
        let mut sizes = sizes_of(&key, ran);
        if ran.is_some_and(|c| !way(c.v)) {
            sizes.retain(|k, _| !buildstore::costin::MADE.contains(&k.as_str()));
        }
        if let Some((m, x)) = offered.filter(|o| o.1 > 0) {
            sizes.entry(m.to_string()).or_insert(x as f64);
        }
        if let Some(s) = self.source(step, target) {
            sizes.insert(SOURCE.to_string(), s);
        }
        sizes.insert(model::TARGET.to_string(), 1.0);
        sizes
    }

    /// Target `target` of `step`'s `SOURCE` size; None where none of the steps it reads naming it
    /// ran it the way it runs now. Kept with the models while the measures stand.
    fn source(&self, step: &str, target: &str) -> Option<f64> {
        let cache = self.cache();
        {
            let mut g = cache.sources.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != self.rev {
                *g = (self.rev, Default::default());
            }
            if let Some(&s) = g.1.get(step).and_then(|t| t.get(target)) {
                return s;
            }
        }
        let s = self.the_plan.sources_naming(step, target).into_iter().find_map(|src| {
            let c = self
                .costs
                .get(&cost_key(src, target))
                .filter(|c| c.v >= self.the_plan.cost_version(src) && !is_drop(c))?;
            Some(1.0 + c.secs as f64 / self.ratio(c.worker.as_deref()))
        });
        let mut g = cache.sources.lock().unwrap_or_else(|e| e.into_inner());
        g.1.entry(step.to_string()).or_default().insert(target.to_string(), s);
        s
    }

    /// The memory (MB) a job of `step` for `target` needs on `threads` (0: as measured), and where
    /// it came from: `need`'s, alone (what the picks read). Its own run: its peak (never below its
    /// floor) times its step's spread over its reruns (`Models::spread`); on fewer threads than
    /// `threads` (`own_threads`), each thread more its `thread_mb`. A floor, or its place's figure
    /// (`near`), says no threads: each grown by its step's model's growth from its runs' threads
    /// to `threads` (`model::Model::mem` on them past it as measured). `guess`: the caller's
    /// first guess (MB); for a step offered by an input measure (`OFFERED_BY`), that measure of the
    /// target, its memory guess none.
    pub fn mem(&self, step: &str, target: &str, threads: u32, guess: u64) -> (u64, From) {
        let (guess, offered) = match offered_by(step) {
            Some(m) => (0, Some((m, guess))),
            None => (guess, None),
        };
        let key = cost_key(step, target);
        let floor = self.current_floor(step, &key);
        let md = self.model(step);
        if let Some(c) = self.own(step, target) {
            let peak = c.peak_mb.max(floor.unwrap_or(0)) as f64;
            let mb = (peak * md.reruns.spread(peak)).ceil() as u64;
            let more = match own_threads(c) {
                Some(ran) if threads > ran => u64::from(threads - ran) * self.own_thread_mb(step, target, c, ran),
                _ => 0,
            };
            return (mb + more, From::Measured);
        }
        let sizes = self.sizes(step, target, offered);
        // (A floor or a place's figure says no threads: grown by what the step's runs grow by from
        // their own threads to `threads`, its model's figure on them past its figure as measured.)
        let grown = |mb: u64| match (threads, md.needs.mem(&sizes, threads), md.needs.mem(&sizes, 0)) {
            (1.., Some(on), Some(ran)) => mb + on.saturating_sub(ran),
            _ => mb,
        };
        if let Some(f) = floor {
            return (grown(f), From::Measured);
        }
        if let Some(p) = md.near.as_ref().filter(|n| n.good()).and_then(|n| n.of(&key)) {
            return (grown(p), From::Near);
        }
        let few = md.needs.runs < MIN_RUNS;
        match md.needs.mem(&sizes, threads) {
            _ if few && guess > 0 && GUESS_FIRST.contains(&step) => (guess, From::Guess),
            Some(p) => (p, From::Model),
            None => (guess, From::Table),
        }
    }

    /// What each thread a job of `step` for `target` grows by past the threads its memory (`mem`)
    /// was read at adds to it (MB). A target whose own run said its threads: that run's peak
    /// (never below its floor) over its threads by its step's rule (`own_thread_mb`: the fitted
    /// memory per thread, a slope over threads that no size scales, at least `thread_share` of
    /// that peak over its threads; that whole where none is fitted). Its step's
    /// runs scaled to its size don't size it: past the target's own measure, they are the model's
    /// guess for a target of that size, and the largest of a step's targets is scaled up from runs
    /// far smaller (task #1528: inputs/regions, 4.6 GB on 1 thread, its siblings' 20-221 MB runs
    /// each scaled by its 2,012 files created, 207 GB a thread). Else its step's model's
    /// (`model::Model::thread_mb`); None where neither says (a cold start's: its job grows past no
    /// threads its grant doesn't cover, crate::agent::threads).
    pub fn thread_mb(&self, step: &str, target: &str) -> Option<u64> {
        match self.own(step, target).and_then(|c| Some((c, own_threads(c)?))) {
            Some((c, ran)) => Some(self.own_thread_mb(step, target, c, ran)),
            None => self.model(step).needs.thread_mb(&self.sizes(step, target, None)),
        }
    }

    /// The bar a move of a target's memory of `step` is weighed against, by its runs alone,
    /// whatever a target's size or own run (`model::Model::move_bar_mb`; crate::coord's
    /// `not_raised`: a move under it sizes nothing that matters in packing).
    pub fn move_bar_mb(&self, step: &str) -> Option<u64> {
        self.model(step).needs.move_bar_mb()
    }

    /// `thread_mb` of a target whose own run `c` ran on `ran` threads: its peak (never below its
    /// floor) over them, by its step's rule (`model::Model::per_thread_of`: the memory per thread
    /// fitted over its runs, at least `thread_share` of that; that itself where none is fitted).
    /// Task #1584: the regions check's runs held 4.4 GB on 4 threads and 4.8 GB on 2, so its
    /// whole peak a thread past its run's booked it at twice what it took on 2.
    fn own_thread_mb(&self, step: &str, target: &str, c: &Cost, ran: u32) -> u64 {
        let peak = c.peak_mb.max(self.current_floor(step, &cost_key(step, target)).unwrap_or(0));
        let linear = peak as f64 / f64::from(ran);
        self.model(step).needs.per_thread_of(&self.sizes(step, target, None), linear).ceil() as u64
    }

    /// The disk (MB) a job of `step` for `target` needs: its disk floor, else its step's model's;
    /// None when none of its step was measured. A step reading a shared cache (`CACHES`): its own
    /// output alone, its disk floor at most its step's model's for it (a measure past it a cache's
    /// fill, reserved once per Mac apart).
    pub fn disk(&self, step: &str, target: &str) -> Option<u64> {
        let key = cost_key(step, target);
        let own = self.disks.and_then(|d| d.get(&key)).copied();
        if own.is_some() && cache_of(step).is_none() {
            return own;
        }
        let modelled = self.model(step).needs.disk(&self.sizes(step, target, None));
        match (own, modelled) {
            (Some(o), Some(p)) => Some(o.min(p)),
            (o, p) => o.or(p),
        }
    }

    /// The wall time (s) a job of `step` for `target` takes on the lead (`secs_on`'s); None when
    /// none of its step ran the way it runs now.
    pub fn secs(&self, step: &str, target: &str, threads: u32) -> Option<u64> {
        self.secs_for(step, target, threads, None)
    }

    /// `secs` on `worker` (`secs_on`'s).
    pub fn secs_for(&self, step: &str, target: &str, threads: u32, worker: Option<&str>) -> Option<u64> {
        self.secs_on(step, target, threads, worker).map(|s| s.0)
    }

    /// What target `target` of `step` takes `worker` on `threads` (s; 0: as it ran, or as many as
    /// its step can use), and whether at its Mac's measured speed (crate::paces; its own run on its
    /// Mac, as measured; false: its Mac's speed not known, the lead's taken). Its own run: on its Mac as it ran, from another Mac at
    /// the two Macs' speeds (twice its time where either's isn't measured), and on fewer threads
    /// than its CPU kept busy, its CPU over them, where that's longer; else its step's model (`model::Model::secs`). None when none of its step's ran
    /// the way it runs now. Kept with the models while the measures stand.
    pub fn secs_on(&self, step: &str, target: &str, threads: u32, worker: Option<&str>) -> Option<(u64, bool)> {
        let cache = self.cache();
        let asked = |a: &SecsAsked| a.0 .0 == threads && a.0 .1.as_deref() == worker;
        {
            let mut g = cache.secs.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != self.rev {
                *g = (self.rev, Default::default());
            }
            if let Some(a) = g.1.get(step).and_then(|t| t.get(target)).and_then(|v| v.iter().find(|a| asked(a))) {
                return a.1;
            }
        }
        let s = self.secs_on_made(step, target, threads, worker);
        let mut g = cache.secs.lock().unwrap_or_else(|e| e.into_inner());
        let kept = g.1.entry(step.to_string()).or_default().entry(target.to_string()).or_default();
        kept.push(((threads, worker.map(str::to_string)), s));
        s
    }

    /// `secs_on` worked out.
    fn secs_on_made(&self, step: &str, target: &str, threads: u32, worker: Option<&str>) -> Option<(u64, bool)> {
        let known = worker.is_none_or(|w| self.paces.is_some_and(|p| p.knows(w)));
        let at = self.ratio(worker);
        if let Some(c) = self.own(step, target) {
            let ran = c.worker.as_deref().map(crate::paces::mac_of);
            // (On its own Mac, or for no worker, as it ran; on another, by the two Macs' speeds, or
            // twice its time where either's isn't measured: no reading says how they compare.)
            let here = worker.is_none() || (ran.is_some() && ran == worker.map(crate::paces::mac_of));
            let both = known && c.worker.as_deref().is_none_or(|w| self.paces.is_some_and(|p| p.knows(w)));
            let at = match (here, both) {
                (true, _) => 1.0,
                (false, true) => at / self.ratio(c.worker.as_deref()),
                (false, false) => 2.0,
            };
            let cpu =
                c.sizes.get("cpu_ms").filter(|_| threads > 0).map_or(0.0, |&ms| ms as f64 / 1000.0 / threads as f64);
            return Some((((c.secs as f64).max(cpu) * at).round() as u64, here || both));
        }
        let s = self.model(step).needs.secs(&self.sizes(step, target, None), threads, at, false)?;
        Some((s.round() as u64, known))
    }

    /// What a drop of `step` (`DROPS`) takes, by its step's drops run the way it runs now: their
    /// mean time (s, rounded up) and the 90th percentile of their memory (MB). Before any of its
    /// step's has run, every step's drops' alike: a drop of any step reads the manifest alone and
    /// deletes names from it (terrain::terrain_deep::drop_target, terrain::terrain_pack::drop_piece,
    /// terrain::slope_pack::drop_lo …), so one step's measure is the next's best first guess, never a
    /// build's minutes and gigabytes. None while no drop of any step ran.
    pub fn drop_need(&self, step: &str) -> Option<(u64, u64)> {
        let of = |only: Option<&str>| -> Vec<&Cost> {
            self.costs
                .iter()
                .filter(|(k, c)| {
                    let s = step_of_key(k);
                    only.is_none_or(|o| s == o) && c.v >= self.the_plan.cost_version(s) && is_drop(c)
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

    /// `secs_for` with its step's model at its high percentile (`Tuning::high_pct`) of work and
    /// waits: what an update's drain waits for a target under way (crate::agent's `update_wait`),
    /// where a long tail's long jobs must not be cut short; its own run's time where it has one.
    pub fn secs_p90(&self, step: &str, target: &str, threads: u32, worker: Option<&str>) -> Option<u64> {
        if let Some(c) = self.own(step, target) {
            return Some(c.secs);
        }
        let s = self.model(step).needs.secs(&self.sizes(step, target, None), threads, self.ratio(worker), true)?;
        Some(s.round() as u64)
    }

    /// The cores a job of `step` for `target` takes, as measured (buildstore::costin's `sizes`): its own
    /// run's, else its step's parallelism (`model::Par`): as many threads as it can use, counted
    /// for the cores its runs keep busy; None when none of them says (what packs jobs beside each
    /// other, crate::coord's packing, then packs none beside it). Its agent's live control grows
    /// or shrinks the threads from there (task #1420).
    pub fn cores(&self, step: &str, target: &str) -> Option<Cores> {
        let v = self.the_plan.cost_version(step);
        if let Some(c) = self.costs.get(&cost_key(step, target)).filter(|c| c.v >= v) {
            if let Some(&t) = c.sizes.get("threads") {
                return Some(own_cores(c, (t as u32).max(1)));
            }
        }
        let par = self.model(step).needs.par;
        (par.runs > 0).then(|| {
            let grant = par.cores().ceil() as u32;
            let busy = par.kept.map(|k| ((k * 10.0).round() as u32).clamp(1, grant * 10));
            Cores { grant, busy_tenths: busy }
        })
    }

    /// The NAS rate a job of `step` for `target` runs at: its own run's (`nas_rate`: `bytes` read
    /// plus `wrote` and `read_back`, and `creates`, each over its wall time), else its step's model's
    /// moves for it over its time on the lead; None when none of them was measured. What
    /// jobrt::budget::nas weighs it by.
    pub fn nas(&self, step: &str, target: &str) -> Option<jobrt::budget::nas::Rate> {
        let CacheRef::Kept(cache) = &self.cache else { return self.nas_made(step, target, None) };
        {
            let mut g = cache.rates.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != self.rev {
                *g = (self.rev, Rates::default());
            }
            if let Some(&r) = g.1.get(step).and_then(|t| t.get(target)) {
                return r;
            }
        }
        // (Worked out unlocked: the model takes its own lock.)
        let r = self.nas_made(step, target, None);
        cache
            .rates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .entry(step.to_string())
            .or_default()
            .insert(target.to_string(), r);
        r
    }

    /// `nas` on `worker`: its own run's as it ran, else its step's runs' of about its size, their
    /// moves over their time together (`model::PerUnit::rate`), on the worker's Mac (a Mac twice as
    /// slow moves the same bytes over twice the time).
    pub fn nas_for(&self, step: &str, target: &str, worker: &str) -> Option<jobrt::budget::nas::Rate> {
        self.nas_made(step, target, Some(worker))
    }

    fn nas_made(&self, step: &str, target: &str, worker: Option<&str>) -> Option<jobrt::budget::nas::Rate> {
        if let Some(r) = self.nas_own(step, target) {
            return Some(r);
        }
        // (Its step's runs of about its size: their moves over their time together, at the lead's
        // pace; a Mac `r` times the lead's time moves them over `r` times the time.)
        let m = self.model(step).needs.rate(&self.sizes(step, target, None))?;
        let r = self.ratio(worker).max(1e-9);
        Some(jobrt::budget::nas::Rate {
            bytes_s: m.bytes / r,
            creates_s: m.creates / r,
            listed_s: m.listed / r,
            stats_s: m.stats / r,
            deletes_s: m.deletes / r,
        })
    }

    /// `nas`'s own run's rate alone.
    pub fn nas_own(&self, step: &str, target: &str) -> Option<jobrt::budget::nas::Rate> {
        self.own_rate(step, target).map(|r| r.0)
    }

    /// Target `target`'s own run's NAS rate and time (`nas_rate`), kept with the models while the
    /// measures stand.
    fn own_rate(&self, step: &str, target: &str) -> Option<(jobrt::budget::nas::Rate, f64)> {
        let mut g = self.cache().own_rates.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != self.rev {
            *g = (self.rev, Default::default());
        }
        if let Some(&r) = g.1.get(step).and_then(|t| t.get(target)) {
            return r;
        }
        let r = self.costs.get(&cost_key(step, target)).and_then(|c| nas_rate(self.the_plan, step, c));
        g.1.entry(step.to_string()).or_default().insert(target.to_string(), r);
        r
    }

    /// `nas_for` on `worker` (None: `nas`, on the lead), with the time it lasts (s): its own run's,
    /// as it ran, else its time as `secs_for` predicts it there (None: not known). A job runs its
    /// targets one after another, so it moves their bytes over their times together: what
    /// crate::coord::sched weighs a job of several by (task #939).
    pub fn nas_load(
        &self,
        step: &str,
        target: &str,
        worker: Option<&str>,
    ) -> Option<(jobrt::budget::nas::Rate, Option<f64>)> {
        if let Some((r, secs)) = self.own_rate(step, target) {
            return Some((r, Some(secs)));
        }
        let r = match worker {
            Some(w) => self.nas_for(step, target, w)?,
            None => self.nas(step, target)?,
        };
        Some((r, self.secs_for(step, target, 0, worker).map(|s| s.max(1) as f64)))
    }

    /// What a job of `step` for `target` needs, given `threads` (0: not said) and `guess`, the memory
    /// (MB) its caller would take without a measure (the steps table's first guess, what its plan
    /// offered it with, a z6 tile's extract-size guess).
    pub fn need(&self, step: &str, target: &str, threads: u32, guess: u64) -> Need {
        let (mem_mb, mem_from) = self.mem(step, target, threads, guess);
        let io = if self.the_plan.fetches(step) { Io::Net } else { Io::Nas };
        Need {
            mem_mb,
            mem_from,
            disk_mb: self.disk(step, target),
            secs: self.secs(step, target, threads),
            cores: threads,
            io,
        }
    }
}

/// The threads a target's own run `c` measured its memory on: those it said (`threads`), or the
/// fewest a paced run or a live target ran on under way where fewer (`threads_least`, from the
/// fewest its file allowed: its peak may have come on them); None where not said.
pub fn own_threads(c: &Cost) -> Option<u32> {
    let least = c.sizes.get("threads_least").copied();
    c.sizes.get("threads").map(|&n| least.map_or(n, |l| l.min(n)) as u32).filter(|&n| n > 0)
}

/// The threads run `c`'s peak memory came on (0: not said), its step's memory a thread read at
/// them (its runs' `model::Obs::threads`): those in force as its job's peak was reached where a
/// live target's changed under it (`threads_peak`, task #1550 review: not the most it was grown
/// to, which a grow in a light phase would book its memory a thread too low at), else those it
/// said (`threads`), as a record from before it says.
fn peak_threads(c: &Cost) -> u32 {
    c.sizes.get("threads_peak").or_else(|| c.sizes.get("threads")).copied().unwrap_or(0) as u32
}

/// A target's own run's cores (`Book::cores`), from `c`, its run on `t` threads: its CPU time
/// (`cpu_ms`) over the time it ran (its wall time less the time it was stopped, `paused_ms`; a
/// second at least), the cores it kept busy, within its threads, to the nearest whole, and one
/// thread more: to overlap its waits, or, where it kept every thread busy, to climb by (task
/// #896). So a job waiting on the NAS or the network, or held by its serial parts, settles where a
/// run measures the same (2.4 busy of 6 or of 3: 3). It counts for the thread it climbs by besides
/// its busy cores, as nothing measured it idle yet, so a climb never overcommits its Mac (task #888
/// review). Its busy cores are its CPU over the time it ran, waits and all, as its agent's live
/// control judges them (crate::agent::threads, task #1435). No CPU time said: its threads, its
/// busy cores not known.
fn own_cores(c: &Cost, t: u32) -> Cores {
    let ran = (c.secs * 1000).saturating_sub(c.sizes.get("paused_ms").copied().unwrap_or(0)).max(1000);
    match c.sizes.get("cpu_ms").map(|&ms| ms as f64 / ran as f64) {
        None => Cores { grant: t, busy_tenths: None },
        Some(k) => {
            let busy = ((k * 10.0).round() as u32).max(1);
            let grant = (k.min(f64::from(t)) + 0.5) as u32 + 1;
            let busy = busy.min(t * 10) + grant.saturating_sub(t) * 10;
            Cores { grant, busy_tenths: Some(busy.min(grant * 10)) }
        }
    }
}

/// `x` with four significant figures, in scientific notation under a hundredth (a step's work
/// per byte is nanoseconds: shown to four places it read 0.0000, task #1510).
fn sig(x: f64) -> String {
    if x != 0.0 && x.abs() < 0.01 {
        format!("{x:.3e}")
    } else {
        format!("{x:.4}")
    }
}

/// `scenic pool needs`: per step, its runs learned from, its sizes best first with their
/// needs per unit, its parallelism, its memory by place, and the runs flagged off their
/// predictions; a step with costs but none of the way it runs now (its cost version) said so,
/// with how many of earlier ways it has.
pub fn model_table(book: &Book) -> String {
    let mut steps: Vec<&str> = book.costs.keys().map(|k| step_of_key(k)).collect();
    steps.sort();
    steps.dedup();
    let t = &book.tuning;
    let mut out = format!(
        "per unit of its sizes, best first (the least spread of work per unit): work in the lead's CPU s and the rest of its time in s at the {}th percentile of its runs; memory at the {}th of its runs that size or more; NAS bytes a second (at the lead's pace) of its smallest runs (of about one unit, or its ten smallest), their bytes over their time\n\n",
        t.work_pct, t.mem_pct
    );
    for s in &steps {
        let md = book.model(s);
        let n = &md.needs;
        if n.runs == 0 {
            let v = book.the_plan.cost_version(s);
            let older = book.costs.iter().filter(|(k, c)| step_of_key(k) == *s && c.v < v && !is_drop(c)).count();
            out += &format!("{s:<14} no runs the way it runs now (cost version {v}); {older} of earlier ways\n");
            continue;
        }
        let since = n.since.as_deref().map_or(String::new(), |k| format!(", relearned from {k}"));
        let busy = n.par.busy.map_or("-".to_string(), sig);
        out += &format!(
            "{s:<14} {} runs, its times from {}{since}; parallelism {:.1} (floor {}, busy {busy}, of {} runs)\n",
            n.runs,
            n.learned,
            n.par.cores(),
            n.par.floor,
            n.par.runs
        );
        for p in n.by.iter().take(3) {
            let f = |x: Option<f64>| x.map_or("-".to_string(), sig);
            let mid = p.mem(1.0, 0).map_or("-".to_string(), |m| format!("{m} MB"));
            let nas = p.rate(1.0).map_or("-".to_string(), |m| format!("{:.0} B/s", m.bytes));
            let per = match (p.mem_per_thread, p.thread_mb(1.0)) {
                (Some(f), _) => format!("{f:.0} MB a thread (fitted)"),
                (None, Some(t)) => format!("{t} MB a thread (its runs' peaks over their threads)"),
                (None, None) => "no threads said".to_string(),
            };
            out += &format!(
                "  per {:<10} {:>5} runs, spread {:.2}: work {} s, rest {} s, NAS {nas}, memory at one {mid}, {per}\n",
                p.size,
                p.runs,
                p.spread,
                f(p.whole.work),
                sig(p.whole.rest)
            );
        }
        if let Some(nr) = md.near.as_ref().filter(|nr| nr.runs > 0) {
            out += &format!(
                "  by place: {} measures, {:.1}% missed, median error {:.1}%{}\n",
                nr.runs,
                nr.miss * 100.0,
                nr.err * 100.0,
                if nr.good() { "" } else { " (poor: not used)" }
            );
        }
        if !n.off.is_empty() {
            let last: Vec<String> =
                n.off.iter().rev().take(5).map(|o| format!("{} {} ×{:.1}", o.key, o.what, o.ratio)).collect();
            out += &format!("  {} runs off their predictions, the last: {}\n", n.off.len(), last.join("; "));
        }
    }
    out
}

/// The costs a by-hand test reads (`costs.json`: `{<cost key>: Cost}`, the coordinator's, as its
/// store holds them).
#[cfg(test)]
fn read_costs(p: &std::path::Path) -> anyhow::Result<BTreeMap<String, Cost>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::cost_version;

    /// Task #1558, the lead's records of 20 Nov 14:32Z (its lake-depth costs, its jobs' facts from
    /// its timings log, its Macs' paces): the M1's one-tile jobs of targets last run before
    /// lake-depth's cost version 1 were held at 30,000-45,000 min (6/10/22 at 44,476), by the
    /// model's best size, what a run wrote, read off those runs' 120 MB grids where the way it runs
    /// now writes a few KB. Each such target is predicted, on each Mac, within its step's measured
    /// spread: its current way's runs' times at that Mac's pace.
    #[test]
    fn a_target_run_an_earlier_way_is_predicted_within_its_steps_spread() {
        let z = include_bytes!("testdata/lake-depth-20261120.json.zst");
        #[derive(serde::Deserialize)]
        struct Case {
            costs: BTreeMap<String, Cost>,
            facts: BTreeMap<String, jobrt::timings::JobFacts>,
            paces: crate::paces::Paces,
        }
        let c: Case = serde_json::from_slice(&zstd::decode_all(&z[..]).unwrap()).unwrap();
        let the_plan = crate::agent::seam::for_tests();
        let inputs = Inputs { paces: &c.paces, records: &c.facts, tuning: Default::default() };
        let b = Book::pool(the_plan, &c.costs, None, inputs);
        let v = the_plan.cost_version("lake-depth");
        let now: Vec<&Cost> = c.costs.values().filter(|c| c.v >= v && !is_drop(c)).collect();
        let older: Vec<&String> = c.costs.iter().filter(|(_, c)| c.v < v).map(|(k, _)| k).collect();
        assert!(now.len() > 100 && older.len() > 1000 && older.iter().any(|k| *k == "lake-depth 6/10/22"));
        for w in ["MacBook-Pro-de-Brandon", "Brandons-MacBook-Pro", "Mac-mini-de-Brandon"] {
            let ratio = |m: &str| c.paces.get(m);
            let at = |r: &Cost| r.secs as f64 * ratio(w) / ratio(r.worker.as_deref().unwrap_or(""));
            let (lo, hi) = now.iter().map(|r| at(r)).fold((f64::MAX, 0.0f64), |(l, h), s| (l.min(s), h.max(s)));
            for k in &older {
                let t = crate::coord::CostKey::parse(k).target;
                let (s, _) = b.secs_on("lake-depth", t, 0, Some(w)).unwrap();
                assert!((lo..=hi).contains(&(s as f64)), "{k} on {w}: {s} s, outside {lo:.0}-{hi:.0} s");
            }
            let sizes = b.sizes("lake-depth", "6/10/22", None);
            let m = b.model("lake-depth");
            let (by, _) = m.needs.unit(&sizes).unwrap();
            eprintln!(
                "lake-depth 6/10/22 on {w}: {:?} s by its {} (sizes {sizes:?}), the step's spread there {lo:.0}-{hi:.0} s",
                b.secs_on("lake-depth", "6/10/22", 0, Some(w)),
                by.size
            );
            assert!(!sizes.contains_key("wrote"), "nothing its run of an earlier way made");
        }
    }

    fn cost(peak: u64, secs: u64, sizes: &[(&str, u64)]) -> Cost {
        Cost {
            job: None,
            peak_mb: peak,
            secs,
            worker: None,
            v: 0,
            sizes: sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    /// Task #1586: a target's own run is booked at its peak times the spread of its step's reruns
    /// whose runs before peaked about as much (the memory percentile of each rerun's peak over its
    /// run before's), never under its peak; with no reruns, at its peak.
    #[test]
    fn an_own_run_is_booked_by_its_steps_rerun_spread() {
        let v = cost_version("depth");
        let at = |peak: u64, was: Option<u64>| Cost { v, was_mb: was, ..cost(peak, 60, &[("threads", 2)]) };
        let mut costs = BTreeMap::from([("depth 6/2/1".to_string(), at(800, None))]);
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(
            (b.model("depth").reruns.spread(800.0), b.mem("depth", "6/2/1", 2, 0)),
            (1.0, (800, From::Measured))
        );
        // (At 1 GB, nine reruns as their runs before, one twice: at the 98th percentile, 1.82. At
        // 50 GB, ten as their runs before.)
        for i in 0..10u64 {
            costs.insert(format!("depth 6/1/{i}"), at(if i == 9 { 2000 } else { 1000 }, Some(1000)));
            costs.insert(format!("depth 6/3/{i}"), at(50_000, Some(50_000)));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        let s = b.model("depth").reruns.spread(800.0);
        assert!((s - 1.82).abs() < 1e-9, "{s}");
        let (mb, from) = b.mem("depth", "6/2/1", 2, 0);
        assert!((1456..=1457).contains(&mb) && from == From::Measured, "{mb}");
        assert_eq!(b.model("depth").reruns.spread(40_000.0), 1.0, "a level's own reruns");
        // (Reruns that took less never book a target under its own peak.)
        for i in 0..10u64 {
            costs.insert(format!("depth 6/1/{i}"), at(500, Some(1000)));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.mem("depth", "6/2/1", 2, 0), (800, From::Measured));
    }

    /// A run of `step` the way it runs now: `secs` of wall, as much CPU on one thread, on `worker`.
    fn run(step: &str, worker: &str, peak: u64, secs: u64, sizes: &[(&str, u64)]) -> Cost {
        let mut c = cost(peak, secs, &[("threads", 1), ("cpu_ms", secs * 1000)]);
        c.sizes.extend(sizes.iter().map(|(k, v)| (k.to_string(), *v)));
        Cost { v: cost_version(step), worker: Some(worker.into()), ..c }
    }

    fn paces(lead: &str, macs: &[(&str, f64)]) -> crate::paces::Paces {
        crate::paces::Paces {
            lead: lead.into(),
            macs: macs.iter().map(|(m, r)| (m.to_string(), *r)).collect(),
            ..Default::default()
        }
    }

    /// Task #1550 review: a live target begun on 1 thread whose peak came on 3 (`threads_peak`) is
    /// booked in its step's memory a thread at those 3; one grown to 3 in a light phase and ending
    /// on them (`threads` 3), its peak on 1 (`threads_peak` 1), at that 1, not at the 3; a record
    /// without `threads_peak`, at the threads it says; its own memory still read on the fewest it
    /// ran on (`own_threads`).
    #[test]
    fn a_run_whose_threads_changed_is_booked_at_those_its_peak_came_on() {
        let book = |threads: u64, peak: Option<u64>| {
            let mut costs = BTreeMap::new();
            for i in 1..=4 {
                let mut c = run("terrain-deep", "m4", 3000, 60, &[]);
                c.sizes.insert("threads".into(), threads);
                if let Some(m) = peak {
                    c.sizes.insert("threads_peak".into(), m);
                }
                costs.insert(cost_key("terrain-deep", &format!("10/1/{i}")), c);
            }
            let b = Book::new(crate::agent::seam::for_tests(), &costs);
            // (A thread as its step books one for a target of none of its own: 10/2/1's.)
            (b.thread_mb("terrain-deep", "10/2/1"), own_threads(&costs[&cost_key("terrain-deep", "10/1/1")]))
        };
        assert_eq!(book(1, Some(3)), (Some(1000), Some(1)), "its peak on 3: booked at 3, its own on 1");
        assert_eq!(book(3, Some(1)), (Some(3000), Some(3)), "grown in a light phase, its peak on 1: at 1");
        assert_eq!(book(1, None), (Some(3000), Some(1)), "a record without it: at the threads it says");
    }

    /// A target's own run's cores (its CPU over the time it ran, task #896, #697, #1435), else its
    /// step's parallelism (`model::Par`); none where no run says its CPU.
    #[test]
    fn cores_are_the_targets_run_else_its_steps_parallelism() {
        let now = |step: &str, c: Cost| Cost { v: cost_version(step), ..c };
        let c = |grant, busy_tenths| Some(Cores { grant, busy_tenths });
        let mut costs = BTreeMap::new();
        costs.insert(cost_key("slope", "6/1/1"), now("slope", cost(100, 5, &[("threads", 4)])));
        costs.insert(cost_key("slope", "6/1/2"), now("slope", cost(100, 5, &[("threads", 2)])));
        costs.insert(cost_key("slope", "6/1/3"), cost(100, 5, &[("threads", 8)]));
        costs.insert(cost_key("lo", "3/1/1"), now("lo", cost(100, 5, &[])));
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.cores("slope", "6/1/2"), c(2, None), "no CPU time said: its busy cores not known");
        assert_eq!(b.cores("slope", "6/1/9"), None, "no run says its CPU");
        assert_eq!(b.cores("lo", "3/1/1"), None);
        // (Its CPU time over its wall time: a serial run given 8 threads that waited on nothing is
        // granted 2 and counted for 0.9 cores; a busy one, its threads full, climbs a thread from
        // its 8, counted for its 8 busy and the one it climbs by.)
        costs.insert(cost_key("pois", "6/1/1"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 9_000)])));
        costs.insert(cost_key("pois", "6/1/2"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 80_000)])));
        costs.insert(cost_key("pois", "6/1/3"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 76_000)])));
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(
            (b.cores("pois", "6/1/1"), b.cores("pois", "6/1/2"), b.cores("pois", "6/1/3")),
            (c(2, Some(9)), c(9, Some(90)), c(9, Some(86))),
            "a busy run climbs"
        );
        // (Not run: its step's parallelism: two runs kept every thread busy (a floor of 8), so it
        // can use 8; counted for its runs' high percentile of busy cores, 8.)
        assert_eq!(b.cores("pois", "6/9/9"), c(8, Some(80)));
        // (Task #688: terrain-hi as measured on 26 Oct, 1.16 cores busy of 3 threads, no waits:
        // granted 2, counted for 1.2; another 1.6 of 2. Not run: neither kept its threads busy,
        // the median of what they kept busy, 1.38: 2 threads, counted for their high percentile.)
        let v = cost_version("terrain-hi");
        costs.insert(
            cost_key("terrain-hi", "6/48/16"),
            Cost { v, ..cost(3700, 1000, &[("threads", 3), ("cpu_ms", 1_160_000)]) },
        );
        costs.insert(
            cost_key("terrain-hi", "6/48/17"),
            Cost { v, ..cost(3700, 1000, &[("threads", 2), ("cpu_ms", 1_600_000)]) },
        );
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.cores("terrain-hi", "6/48/16"), c(2, Some(12)));
        assert_eq!(b.cores("terrain-hi", "6/48/17"), c(3, Some(26)), "1.6 of 2: climbs");
        assert_eq!(b.cores("terrain-hi", "6/48/18"), c(2, Some(16)), "its step's parallelism");
        // (Task #888 review: 2.4 cores busy, its grant settles at 3 whatever it ran with.)
        for t in [6, 4, 3] {
            costs.insert(
                cost_key("terrain-hi", "6/48/19"),
                Cost { v, ..cost(3700, 1000, &[("threads", t), ("cpu_ms", 2_400_000)]) },
            );
            assert_eq!(
                Book::new(crate::agent::seam::for_tests(), &costs).cores("terrain-hi", "6/48/19"),
                c(3, Some(24)),
                "{t}"
            );
        }
        // (Task #1435: its busy cores are its CPU over the time it ran, waits and all.)
        for waited in [None, Some(("nas_net_ms", 500_000)), Some(("nas_net_ms", 900_000))] {
            let mut s = vec![("threads", 3), ("cpu_ms", 300_000)];
            s.extend(waited);
            costs.insert(cost_key("terrain-hi", "6/48/19"), Cost { v, ..cost(3700, 1000, &s) });
            assert_eq!(
                Book::new(crate::agent::seam::for_tests(), &costs).cores("terrain-hi", "6/48/19"),
                c(1, Some(3)),
                "{waited:?}"
            );
        }
        // (Task #697: a run frozen by the memory guard for 420 of its 1000 s kept 2 cores busy over
        // the 580 s it ran, not 1.16.)
        costs.insert(
            cost_key("terrain-hi", "6/48/16"),
            Cost { v, ..cost(3700, 1000, &[("threads", 3), ("cpu_ms", 1_160_000), ("paused_ms", 420_000)]) },
        );
        assert_eq!(Book::new(crate::agent::seam::for_tests(), &costs).cores("terrain-hi", "6/48/16"), c(3, Some(20)));
    }

    /// A target's NAS rate: its own run's bytes (read, written and read back) and creates over its
    /// time, else its step's runs' of about its size, their moves over their time together; None
    /// with no run measured (none says `bytes`). A line that doesn't say what it read back (from
    /// before task #1142) read back its `wrote`; what it read of its local copies isn't the NAS's
    /// (task #551), but the NAS reads its page cache held are (task #1417).
    #[test]
    fn a_nas_rate_is_the_targets_run_else_its_steps_bytes_per_unit() {
        let now = |c: Cost| Cost { v: cost_version("slope"), ..c };
        let mut costs = BTreeMap::new();
        let first = cost(100, 10, &[("bytes", 600), ("wrote", 200), ("read_back", 200), ("creates", 5)]);
        costs.insert(cost_key("slope", "6/1/1"), now(first));
        costs.insert(cost_key("slope", "6/2/1"), now(cost(100, 10, &[("bytes", 600), ("wrote", 200), ("creates", 5)])));
        for i in 2..12 {
            costs.insert(cost_key("slope", &format!("6/1/{i}")), now(cost(100, 10, &[("bytes", 100 * i)])));
        }
        costs.insert(cost_key("lo", "3/1/1"), cost(100, 5, &[("threads", 2)]));
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(
            b.nas("slope", "6/1/1"),
            Some(jobrt::budget::nas::Rate { bytes_s: 100.0, creates_s: 0.5, ..Default::default() })
        );
        assert_eq!(b.nas("slope", "6/2/1"), b.nas("slope", "6/1/1"), "its `wrote` read back");
        // (Bytes 200..1100 and 1000 the first two's, 8,500 in all, over their 120 s.)
        assert_eq!(b.nas("slope", "6/9/9").map(|r| r.bytes_s.round()), Some(71.0));
        assert_eq!(b.nas("lo", "3/1/1"), None);
        let mut costs = BTreeMap::new();
        let v = cost_version("depth");
        costs.insert(cost_key("depth", "6/1/1"), Cost { v, ..cost(100, 10, &[("bytes", 1000), ("local", 800)]) });
        assert_eq!(
            Book::new(crate::agent::seam::for_tests(), &costs).nas("depth", "6/1/1").map(|r| r.bytes_s),
            Some(20.0)
        );
        let held = cost(100, 10, &[("bytes", 1000), ("local", 800), ("held", 500)]);
        costs.insert(cost_key("depth", "6/1/1"), Cost { v, ..held });
        assert_eq!(
            Book::new(crate::agent::seam::for_tests(), &costs).nas("depth", "6/1/1").map(|r| r.bytes_s),
            Some(70.0)
        );
        // (A target's load: its own run's rate and time; one not run, its step's over its time.)
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.nas_load("depth", "6/1/1", None).map(|l| l.1), Some(Some(10.0)));
        let other = b.nas_load("depth", "6/9/9", None).unwrap();
        assert_eq!(other.1, b.secs_for("depth", "6/9/9", 0, None).map(|s| s as f64));
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
        let c = cost(100, 10, &[("bytes", 1300 << 20), ("wrote", 200), ("read_back", 200), ("creates", 5)]);
        costs.insert(cost_key("pack", "6/1/1"), Cost { v: cost_version("pack"), ..c.clone() });
        // (Read 30 MB in 20 s; this target itself took 0 s.)
        let mut quick = cost(100, 0, &[("bytes", 1300 << 20), ("wrote", 200), ("read_back", 200), ("creates", 5)]);
        quick.sizes.extend([("nas_read".to_string(), 30_000_000), ("nas_secs".to_string(), 20)]);
        costs.insert(cost_key("pack", "6/1/2"), Cost { v: cost_version("pack"), ..quick });
        let mut b38 = c;
        b38.sizes.insert("nas_read".into(), 959 << 20);
        costs.insert(cost_key("pack", "6/1/3"), Cost { v: cost_version("pack"), ..b38 });
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(
            b.nas("pack", "6/1/1"),
            Some(jobrt::budget::nas::Rate { bytes_s: 40.0, creates_s: 0.5, ..Default::default() })
        );
        assert_eq!(
            b.nas("pack", "6/1/2"),
            Some(jobrt::budget::nas::Rate { bytes_s: 1_500_020.0, creates_s: 0.25, ..Default::default() }),
            "its writes over the job's 20 s, not its own 0 s (pack 6/39/21 weighed the whole NAS)"
        );
        assert_eq!(
            b.nas("pack", "6/1/3"),
            Some(jobrt::budget::nas::Rate { bytes_s: 40.0, creates_s: 0.5, ..Default::default() }),
            "b38's: writes alone"
        );
        // (Task #1510: a target not run, its step's runs' moves over their own times together,
        // each as its own rate counts it: 6/1/2's reads over the job's 20 s, not over its own 0 s
        // (a lo pass read at 53 GB/s), (30 MB + 3 × 400 B) over 10 + 20 + 10 s.)
        let r = b.nas("pack", "6/9/9").unwrap();
        assert!((r.bytes_s - 750_030.0).abs() < 1e-6, "{r:?}");
        // (On a NAS of 100 MB/s and 40 creates/s: its 1.5 MB/s and 0.25 creates/s, not the whole.)
        let l = jobrt::budget::nas::Limits { bytes_s: 100e6, creates_s: 40.0, downloads: None, ..Default::default() };
        assert_eq!(l.weight(b.nas("pack", "6/1/2").unwrap()), 22);
        assert_eq!(batch_disk("pack", [1300, 1300, 1200]), 1300);
        assert_eq!(batch_disk("slope", [1300, 1300, 1200]), 3800);
    }

    /// A step reading a shared cache (task #392: `CACHES`): a target's disk its own output, its
    /// floor at most its step's model's (the cold run that filled the cache measured it whole);
    /// another step's floor its own, whole.
    #[test]
    fn a_cache_steps_disk_is_its_output_not_the_fill() {
        let mut disks: BTreeMap<String, u64> = (0..20).map(|i| (cost_key("depth", &format!("6/1/{i}")), 300)).collect();
        disks.insert(cost_key("depth", "6/16/20"), 67_890);
        disks.insert(cost_key("depth", "6/1/0"), 100);
        disks.insert(cost_key("peaks", "6/1/1"), 67_890);
        let costs: BTreeMap<String, Cost> =
            disks.keys().map(|k| (k.clone(), Cost { v: cost_version(step_of_key(k)), ..cost(100, 10, &[]) })).collect();
        let b = Book::with(crate::agent::seam::for_tests(), &costs, None, Some(&disks));
        assert_eq!(b.disk("depth", "6/16/20"), Some(300), "the fill reserved apart, once per Mac");
        assert_eq!(b.disk("depth", "6/1/0"), Some(100), "its own when under the step's");
        assert_eq!(b.disk("depth", "6/9/9"), Some(300), "never measured: the step's");
        assert_eq!(b.disk("peaks", "6/1/1"), Some(67_890), "no shared cache: its own whole");
        assert_eq!(
            (cache_of("pack"), cache_of("lo"), cache_of("depth"), cache_of("slope")),
            (None, None, Some(buildstore::localread::CACHE), None)
        );
    }

    /// Task #1416: a step's runs are read in the lead's CPU seconds by each Mac's time against the
    /// lead's (its speed, crate::paces), so a slow Mac's runs don't drag the step's; a prediction for
    /// a worker is at its Mac's time; a target's own run as it ran on its Mac, at another Mac's by
    /// the two; a Mac not measured at the lead's, said so.
    #[test]
    fn a_steps_runs_are_read_at_the_leads_speed_and_predicted_at_each_macs() {
        let mut costs = BTreeMap::new();
        // (Ten runs on the lead at 100 s, ten on m1 at 300 s, each reading 300 MB.)
        for k in 0..20u32 {
            let (worker, secs) = if k < 10 { ("mini #2", 100) } else { ("m1", 300) };
            costs.insert(format!("slope-hi 10/{k}/0"), run("slope-hi", worker, 1000, secs, &[("bytes", 300_000_000)]));
        }
        let raw = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(raw.secs("slope-hi", "10/99/0", 0), Some(100), "as measured: its fastest runs'");
        let p = paces("mini", &[("m1", 3.0)]);
        let b = Book::new(crate::agent::seam::for_tests(), &costs).paced(&p);
        assert_eq!(b.secs_on("slope-hi", "10/99/0", 0, Some("mini")), Some((100, true)));
        assert_eq!(b.secs_on("slope-hi", "10/99/0", 0, Some("m1 #2")), Some((300, true)), "at m1's");
        assert_eq!(b.secs_on("slope-hi", "10/99/0", 0, Some("m4")), Some((100, false)), "not measured: the lead's");
        assert_eq!(b.secs_for("slope-hi", "10/12/0", 0, Some("m1")), Some(300), "its own run, on its Mac as it ran");
        assert_eq!(b.secs_for("slope-hi", "10/12/0", 0, Some("mini")), Some(100), "m1's run at the lead's");
        assert_eq!(b.secs_p90("slope-hi", "10/99/0", 0, Some("m1")), Some(300));
        // (NAS rates: 3 MB/s on the lead, a third of that on m1.)
        let mb_s = |r: jobrt::budget::nas::Rate| (r.bytes_s / 1e6).round();
        assert_eq!(b.nas_for("slope-hi", "10/99/0", "mini").map(mb_s), Some(3.0));
        assert_eq!(b.nas_for("slope-hi", "10/99/0", "m1").map(mb_s), Some(1.0), "a third on m1");
        assert_eq!(b.nas_for("slope-hi", "10/15/0", "mini").map(mb_s), Some(1.0), "its own run, as it ran");
    }

    /// A run's job whose record failed its checks (or ended short) isn't learned from; one whose
    /// record counted its instructions is read by them at the lead's speed; one whose record the
    /// lead's log no longer holds counts by its CPU.
    #[test]
    fn a_run_is_learned_from_as_its_record_says() {
        let job = |n: u64| Some(jobrt::jobid::JobId { epoch: 1, n });
        let mut costs = BTreeMap::new();
        for k in 0..20u64 {
            let c = Cost { job: job(k), ..run("depth", "mini", 1000, 100, &[]) };
            costs.insert(format!("depth 6/{k}/0"), c);
        }
        let facts = |clean: bool, ginsn: f64| jobrt::timings::JobFacts {
            clean,
            ginsn,
            cpu_s: 100.0,
            explain: 1.0,
            ..Default::default()
        };
        // (Ten of them failed their checks, at a tenth of the time: none of those counts.)
        let mut records: Records = (0..10u64).map(|k| (format!("1-{k}"), facts(false, 0.0))).collect();
        for k in 0..10u64 {
            costs.get_mut(&format!("depth 6/{k}/0")).unwrap().sizes.insert("cpu_ms".into(), 10_000);
        }
        let p = crate::paces::Paces { lead: "mini".into(), lead_ginsn_s: 10.0, ..Default::default() };
        let b = Book::new(crate::agent::seam::for_tests(), &costs).inputs(Inputs {
            paces: &p,
            records: &records,
            tuning: Default::default(),
        });
        assert_eq!(b.model("depth").needs.runs, 10);
        assert_eq!(b.secs("depth", "6/99/0", 0), Some(100));
        // (The others' records counted 2000 G instructions at 10 G a second: 200 lead seconds.)
        records.extend((10..20u64).map(|k| (format!("1-{k}"), facts(true, 2000.0))));
        let b = Book::new(crate::agent::seam::for_tests(), &costs).inputs(Inputs {
            paces: &p,
            records: &records,
            tuning: Default::default(),
        });
        assert_eq!(b.secs("depth", "6/99/0", 0), Some(200));
    }

    /// While the way a step runs now has few runs, its first to end are its quick ones: its latest
    /// earlier way's work per unit stands in where larger, so its first jobs aren't sized by them.
    #[test]
    fn a_new_ways_first_quick_runs_dont_shrink_its_time() {
        let v = cost_version("depth");
        let mut costs = BTreeMap::new();
        for k in 0..30 {
            costs.insert(format!("depth 6/{k}/1"), Cost { v: v - 1, ..run("depth", "mini", 100, 300, &[]) });
        }
        for k in 0..5 {
            costs.insert(format!("depth 6/{k}/2"), run("depth", "mini", 100, 10, &[]));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.secs("depth", "6/50/50", 0), Some(300));
        for k in 5..PRIOR_RUNS {
            costs.insert(format!("depth 6/{k}/2"), run("depth", "mini", 100, 10, &[]));
        }
        assert_eq!(Book::new(crate::agent::seam::for_tests(), &costs).secs("depth", "6/50/50", 0), Some(10));
    }

    /// Task #830: a pyramids-fill z6 tile whose tiles were all stored took 17–37 s, one fetching
    /// 170,000 of them 4,950 s. A PER_WORK step's time counts its runs that did their work alone:
    /// with only no-op runs it has no time of its own; a no-op target keeps its own time.
    #[test]
    fn a_fill_with_nothing_to_fetch_doesnt_set_the_steps_time() {
        let mut costs = BTreeMap::new();
        for i in 1..=10u64 {
            costs.insert(format!("pyramids-fill 6/{i}/1"), run("pyramids-fill", "mini", 2900, 20, &[("fetched", 0)]));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.secs("pyramids-fill", "6/55/25", 2), None, "nothing fetched");
        assert_eq!(b.secs("pyramids-fill", "6/1/1", 2), Some(20), "its own");
        for (t, s, f) in [("6/56/23", 4950, 170_612), ("6/57/23", 4000, 150_000), ("6/58/23", 6000, 200_000)] {
            costs.insert(format!("pyramids-fill {t}"), run("pyramids-fill", "mini", 2900, s, &[("fetched", f)]));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert!(b.secs("pyramids-fill", "6/55/25", 2).is_some_and(|s| s >= 4000));
        assert_eq!(time_weight("peaks", &cost(1, 1, &[])), Some(1.0));
        assert_eq!(weighted_mean(&[]), None);
    }

    /// The lead's queue on 29 Oct: 4,275 terrain-deep targets in Alaska, each a drop (its packs
    /// dropped, nothing read), forecast at the step's 76 s and 7 GB a build. A drop's run (`DROPS`)
    /// counts for nothing in its step's model; its step's drops are modelled apart
    /// (`Book::drop_need`).
    #[test]
    fn a_drop_is_no_build_of_its_step() {
        let mut costs = BTreeMap::new();
        for (t, s) in [("10/256/352", 236), ("10/256/353", 200), ("10/257/352", 260)] {
            costs.insert(format!("terrain-deep {t}"), run("terrain-deep", "mini", 7000, s, &[("km2", 900)]));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        let (secs, mb) = (b.secs("terrain-deep", "10/300/300", 0), b.mem("terrain-deep", "10/300/300", 0, 0));
        assert_eq!(b.drop_need("terrain-deep"), None, "no drop ran yet");
        for (t, s) in [("10/100/252", 2), ("10/100/253", 1), ("10/101/228", 3)] {
            costs.insert(format!("terrain-deep {t}"), run("terrain-deep", "mini", 150, s, &[("km2", 900), (DROPS, 1)]));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.secs("terrain-deep", "10/300/300", 0), secs, "the builds' time alone");
        assert_eq!(b.mem("terrain-deep", "10/300/300", 0, 0), mb, "the builds' memory alone");
        assert_eq!(b.drop_need("terrain-deep"), Some((2, 150)));
        assert_eq!(b.drop_need("slope-hi"), Some((2, 150)));
        let mut more = costs.clone();
        more.insert("slope-hi 6/10/20".into(), Cost { v: cost_version("slope-hi"), ..cost(90, 5, &[(DROPS, 2)]) });
        assert_eq!(Book::new(crate::agent::seam::for_tests(), &more).drop_need("slope-hi"), Some((5, 90)));
        assert_eq!(b.secs("terrain-deep", "10/100/252", 0), Some(2));
        assert!(time_weight("terrain-deep", &costs["terrain-deep 10/100/252"]).is_none());
    }

    /// A target not measured takes its step's model's memory (its high percentile, not one
    /// outlier); one of a step never measured takes the guess; a floor the way it runs now holds
    /// its target; an old floor is no measure.
    #[test]
    fn memory_is_the_models_high_percentile_and_none_the_guess() {
        let mut costs = BTreeMap::new();
        for i in 1..=30u64 {
            costs.insert(format!("peaks 6/{i}/1"), cost(if i % 7 == 0 { 9000 } else { 300 + (i * 37) % 200 }, 5, &[]));
        }
        costs.insert("peaks 6/31/1".into(), cost(40_000, 5, &[]));
        let floors = BTreeMap::from([
            ("peaks 6/50/1".to_string(), Floor { mb: 12_000, alone: true, v: 0 }),
            ("terrain 6/50/1".to_string(), Floor { mb: 12_000, alone: true, v: 0 }),
        ]);
        let mut b = Book::with(crate::agent::seam::for_tests(), &costs, Some(&floors), None);
        // (At the 95th percentile, the one outlier of 31 runs left out.)
        b.tuning.mem_pct = 95.0;
        assert_eq!(b.mem("peaks", "6/40/1", 0, 1), (9000, From::Model), "the high percentile: not the outlier");
        assert_eq!(b.mem("peaks", "6/50/1", 0, 1), (12_000, From::Measured));
        assert_eq!(b.mem("slope", "6/40/1", 0, 777), (777, From::Table));
        assert_eq!(b.need("slope", "6/40/1", 0, 777).secs, None);
        assert_eq!(b.mem("terrain", "6/50/1", 0, 1500), (1500, From::Table));
    }

    /// Task #979: a step offered by its rows (`OFFERED_BY`) is predicted by them, a target not run
    /// yet by the rows it's offered with; its guess is no memory (none of the step measured: 0, a
    /// cold start with nothing of its own).
    #[test]
    fn a_target_offered_by_its_rows_is_predicted_by_them() {
        let mut costs = BTreeMap::new();
        // (300 MB and 100 B a row, a second of work a million rows.)
        for i in 1..=40u64 {
            let rows = i * 1_000_000;
            let c = run("bldprep", "mini", 300 + rows * 100 / 1_000_000, i, &[("rows", rows)]);
            costs.insert(cost_key("bldprep", &format!("6/{i}/0")), c);
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.model("bldprep").needs.by[0].size, "rows");
        let (small, from) = b.mem("bldprep", "6/63/63", 0, 2_000_000);
        let (big, _) = b.mem("bldprep", "6/63/63", 0, 30_000_000);
        assert_eq!(from, From::Model);
        // (Each by the runs at or above its rows: 2 million's the 10 of 2 to 11 million, 500 to
        // 1,400 MB; 30 million's those of 30 to 40.)
        assert!(small < big && (1300..=1400).contains(&small) && (3800..=4300).contains(&big), "{small} {big}");
        assert_eq!(b.mem("bldtiles", "6/63/63", 0, 30_000_000), (0, From::Table));
    }

    /// Task #979: a cold start after its step's way changed is sized by its runs of the latest
    /// earlier way (that way's model's memory for it, its runs at or above its size: here all one
    /// size, so their 98th percentile; or its own run of it, the larger), floors and older ways
    /// left out; none without such a run.
    #[test]
    fn a_new_ways_cold_start_is_sized_by_its_earlier_ways_runs() {
        let v = cost_version("terrain-hi");
        let mut costs = BTreeMap::new();
        for i in 1..=10u64 {
            costs.insert(cost_key("terrain-hi", &format!("6/{i}/1")), Cost { v: v - 1, ..cost(i * 100, 60, &[]) });
        }
        costs.insert(cost_key("terrain-hi", "6/20/1"), Cost { v: v - 2, ..cost(50_000, 60, &[]) });
        let floors = BTreeMap::from([(cost_key("terrain-hi", "6/30/1"), Floor { mb: 40_000, alone: true, v: v - 1 })]);
        let b = Book::with(crate::agent::seam::for_tests(), &costs, Some(&floors), None);
        assert_eq!(b.older_mem("terrain-hi", "6/40/1"), Some(982), "the 98th percentile");
        assert_eq!(b.older_mem("terrain-hi", "6/10/1"), Some(1000), "its own run");
        assert_eq!(b.older_mem("terrain-hi", "6/20/1"), Some(982), "an older way's: nothing");
        assert_eq!(b.mem("terrain-hi", "6/40/1", 0, 0).1, From::Table, "still a cold start");
        assert_eq!(Book::new(crate::agent::seam::for_tests(), &BTreeMap::new()).older_mem("terrain-hi", "6/1/1"), None);
    }

    /// Task #1507: a target whose own run and floor are of an older way of its step (lo 3/4/2's
    /// 17.5 GB) is sized by the runs of the way it runs now, its siblings', by its own earlier sizes
    /// (here scaled up from the largest: 90 MB a GiB of its base packs); with none of them, a cold
    /// start. Its disk floor carries no version (the cost version is its memory's and time's): its
    /// own, whatever the way.
    #[test]
    fn a_target_whose_history_predates_its_steps_way_is_sized_by_the_new_ways_runs() {
        let v = cost_version("lo");
        assert!(v > 0);
        let gib = 1u64 << 30;
        let old = Cost { v: v - 1, ..run("lo", "m1", 17_478, 380, &[("bytes", 23 * gib)]) };
        let mut costs = BTreeMap::from([(cost_key("lo", "3/4/2"), old)]);
        let disks = BTreeMap::from([(cost_key("lo", "3/4/2"), 35_803)]);
        let floors = BTreeMap::from([(cost_key("lo", "3/4/2"), Floor { mb: 18_000, alone: true, v: v - 1 })]);
        let b = Book::with(crate::agent::seam::for_tests(), &costs, Some(&floors), Some(&disks));
        assert_eq!(b.mem("lo", "3/4/2", 0, 0), (0, From::Table), "nothing of the way it runs now: a cold start");
        assert_eq!(b.disk("lo", "3/4/2"), Some(35_803), "its disk floor counts");
        for i in 1..=10u64 {
            let t = format!("3/{i}/5");
            costs.insert(cost_key("lo", &t), run("lo", "m1", 90 * i, 10 * i, &[("bytes", i * gib)]));
        }
        let b = Book::with(crate::agent::seam::for_tests(), &costs, Some(&floors), Some(&disks));
        assert_eq!(b.mem("lo", "3/4/2", 0, 0), (90 * 23, From::Model), "its siblings' runs, scaled to its size");
        assert_eq!(b.disk("lo", "3/4/2"), Some(35_803));
        assert_eq!(b.own("lo", "3/4/2"), None);
    }

    /// One dense z6 tile measured doesn't set the others': while its model has few runs, a z6
    /// tile's extract-size guess comes before it.
    #[test]
    fn a_dense_unit_leaves_the_others_their_piece_guess() {
        let costs = BTreeMap::from([("6/1/1".to_string(), Cost { v: cost_version("unit"), ..cost(14_000, 600, &[]) })]);
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert_eq!(b.mem("unit", "6/1/2", 0, 3700), (3700, From::Guess));
        assert_eq!(b.mem("peaks", "6/1/2", 0, 3700).1, From::Table);
        assert_eq!(b.mem("unit", "6/1/2", 0, 0), (14_000, From::Model));
    }

    /// Memory per thread is fitted when the runs differ in threads, and adds to a target run on
    /// more threads than its size's runs had.
    #[test]
    fn memory_per_thread_is_fitted_when_threads_differ() {
        let mut costs = BTreeMap::new();
        for i in 1..=40u64 {
            let t = 4 + (i % 3) * 4;
            let mut c = run("terrain-lo", "mini", 200 + i * 10 + t * 100, i, &[("tiffs", i)]);
            c.sizes.insert("threads".into(), t);
            c.sizes.insert("cpu_ms".into(), i * 1000);
            costs.insert(format!("terrain-lo 3/{i}/1"), c);
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        let md = b.model("terrain-lo");
        let p = md.needs.by.iter().find(|p| p.size == "tiffs").unwrap();
        let per = p.mem_per_thread.unwrap();
        assert!((per - 100.0).abs() < 1.0, "{per}");
        assert!(b.mem("terrain-lo", "3/90/1", 16, 0).0 > b.mem("terrain-lo", "3/90/1", 12, 0).0);
    }

    /// A kept model is made again once its owner's change count moves, not before.
    #[test]
    fn a_kept_model_lasts_until_the_measures_change() {
        let cache = Cache::default();
        let (f, d) = (BTreeMap::new(), BTreeMap::new());
        let (paces, records) = (crate::paces::Paces::default(), Records::new());
        let inputs = Inputs { paces: &paces, records: &records, tuning: Default::default() };
        let mut costs = BTreeMap::from([("peaks 6/1/1".to_string(), cost(1000, 5, &[]))]);
        let mem = |costs: &BTreeMap<String, Cost>, rev| {
            Book::kept(crate::agent::seam::for_tests(), costs, &f, &d, inputs, &cache, rev)
                .mem("peaks", "6/9/9", 0, 0)
                .0
        };
        assert_eq!(mem(&costs, 1), 1000);
        costs.insert("peaks 6/1/2".into(), cost(5000, 5, &[]));
        assert_eq!(mem(&costs, 1), 1000, "kept");
        assert!(mem(&costs, 2) > 1000);
    }

    /// Task #931: a terrain-deep target's memory is its place's (3 GB where its fine data is sparse,
    /// 9 over dense lidar). One not measured takes its nearest measured ones' most, × `MARGIN`, not
    /// the step's high percentile, which reserved twice a sparse one's memory and too little for a
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
        for t in ["10/303/360", "10/315/360", "10/310/360"] {
            costs.remove(&format!("terrain-deep {t}"));
        }
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        let nr = b.model("terrain-deep").near.clone().unwrap();
        assert!(nr.good() && nr.runs > 300, "{nr:?}");
        let (sparse, from) = b.mem("terrain-deep", "10/303/360", 2, 0);
        assert_eq!(from, From::Near);
        assert!(sparse <= (3200.0 * MARGIN).ceil() as u64 && sparse >= 3000, "{sparse}");
        let (dense, _) = b.mem("terrain-deep", "10/315/360", 2, 0);
        assert!(dense >= 9000 && dense <= (9600.0 * MARGIN).ceil() as u64, "{dense}");
        assert!(b.mem("terrain-deep", "10/310/360", 2, 0).0 >= 9000);
        let own = costs["terrain-deep 10/301/360"].peak_mb;
        assert_eq!(b.mem("terrain-deep", "10/301/360", 2, 0), (own, From::Measured));
        // (Far from any measured target: its step's model.)
        assert_eq!(b.mem("terrain-deep", "10/100/100", 2, 0).1, From::Model);
        let floors = BTreeMap::from([("terrain-deep 10/303/361".to_string(), Floor { mb: 12_000, alone: true, v })]);
        let b = Book::with(crate::agent::seam::for_tests(), &costs, Some(&floors), None);
        assert!(b.mem("terrain-deep", "10/303/360", 2, 0).0 >= 12_000);
    }

    /// Where the memory isn't its place's (the same spread, the tiles shuffled), the nearest
    /// predict poorly, held out, and the step's model stands.
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
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        let nr = b.model("terrain-deep").near.clone().unwrap();
        assert!(!nr.good(), "{nr:?}");
        assert_eq!(b.mem("terrain-deep", "10/305/360", 2, 0).1, From::Model);
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

    /// A `GUESS_FIRST` step keeps its piece's guess before its model: its place is weighed by no
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
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        assert!(b.model("unit").near.is_none());
        assert_ne!(b.mem("unit", "6/20/20", 0, 5000).1, From::Near);
    }

    /// By hand (task #1416): the held-out test of the needs model on the lead's records. For each
    /// day from `SCENIC_NEEDS_DAY0` (unix s), `SCENIC_NEEDS_DAYS` of them, a book of the runs that
    /// ended before it (its costs, its records' facts and each Mac's speed from the records that
    /// ended before it), predicting each target granted that day the way its step runs now. Reads
    /// `costs.json` (the lead's costs, `read_costs`), `history.jsonl` (its coord/history.jsonl) and
    /// `timings.jsonl` (its coord/timings.jsonl) in `SCENIC_NEEDS_BACKTEST`; writes `new.jsonl`,
    /// a line a target: its day, key, step, worker, threads and the time, memory, NAS bytes and
    /// busy cores predicted. `SCENIC_NEEDS_TUNINGS` (`<label>:<knob>=<value>,…;…`, the knobs
    /// jobrt::needs::KNOBS) writes `new-<label>.jsonl` for each tuning instead. Without the folder
    /// it prints that it skipped and checks nothing.
    #[test]
    fn backtest_needs_on_the_leads_records() {
        use std::io::Write;
        let Ok(dir) = std::env::var("SCENIC_NEEDS_BACKTEST") else {
            eprintln!("needs backtest skipped: SCENIC_NEEDS_BACKTEST unset (nothing checked)");
            return;
        };
        let dir = std::path::Path::new(&dir);
        let all = read_costs(&dir.join("costs.json")).unwrap();
        let text = std::fs::read_to_string(dir.join("history.jsonl")).unwrap();
        let events: Vec<crate::coord::history::Event> =
            text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let log = std::fs::read_to_string(dir.join("timings.jsonl")).unwrap();
        let recs: Vec<(f64, &str)> = log
            .lines()
            .filter_map(|l| {
                let r: jobrt::timings::RunRec = serde_json::from_str(l).ok()?;
                Some((r.start as f64 + r.wall_s, l))
            })
            .collect();
        let tunings: Vec<(String, jobrt::needs::Tuning)> = match std::env::var("SCENIC_NEEDS_TUNINGS") {
            Err(_) => vec![("new".into(), Default::default())],
            Ok(s) => s
                .split(';')
                .filter_map(|t| {
                    let (label, knobs) = t.split_once(':')?;
                    let set: BTreeMap<&str, f64> = knobs
                        .split(',')
                        .filter_map(|kv| kv.split_once('=').and_then(|(k, v)| Some((k.trim(), v.trim().parse().ok()?))))
                        .collect();
                    Some((format!("new-{}", label.trim()), jobrt::needs::Tuning::of(|n| set.get(n).copied())))
                })
                .collect(),
        };
        let the_plan = crate::agent::seam::for_tests();
        let lead = "Mac-mini-de-Brandon";
        let day0: u64 = std::env::var("SCENIC_NEEDS_DAY0").unwrap().parse().unwrap();
        let days: u64 = std::env::var("SCENIC_NEEDS_DAYS").unwrap().parse().unwrap();
        let mut outs: Vec<std::fs::File> =
            tunings.iter().map(|(l, _)| std::fs::File::create(dir.join(format!("{l}.jsonl"))).unwrap()).collect();
        let granted = |c: &Cost| c.job.map(|j| j.n / 1000);
        for d in 0..days {
            let t0 = day0 + d * 86_400;
            let book_costs: BTreeMap<String, Cost> = all
                .iter()
                .filter(|(_, c)| granted(c).is_none_or(|g| g + c.secs < t0))
                .map(|(k, c)| (k.clone(), c.clone()))
                .collect();
            let ev: Vec<crate::coord::history::Event> = events.iter().filter(|e| e.t < t0).cloned().collect();
            let tail_at = dir.join(format!("tail-{d}.jsonl"));
            let mut f = std::fs::File::create(&tail_at).unwrap();
            for (_, l) in recs.iter().filter(|(end, _)| *end < t0 as f64) {
                writeln!(f, "{l}").unwrap();
            }
            drop(f);
            let mut tail = jobrt::timings::LogTail::default();
            tail.read(&tail_at);
            std::fs::remove_file(&tail_at).unwrap();
            let speeds = tail.speeds(crate::paces::mac_of, |_| true);
            let paces = crate::paces::Paces::of(&speeds, lead).with_starts(the_plan, &ev, &book_costs, t0);
            let records = tail.facts().clone();
            for ((_, tuning), out) in tunings.iter().zip(&mut outs) {
                let book = Book::pool(
                    the_plan,
                    &book_costs,
                    None,
                    Inputs { paces: &paces, records: &records, tuning: *tuning },
                );
                if std::env::var("SCENIC_NEEDS_SHOW").is_ok_and(|s| s == d.to_string()) {
                    eprintln!("{}", model_table(&book));
                }
                for (k, c) in all.iter().filter(|(_, c)| granted(c).is_some_and(|g| (t0..t0 + 86_400).contains(&g))) {
                    let crate::coord::CostKey { step, target } = crate::coord::CostKey::parse(k);
                    if c.v < the_plan.cost_version(step) || is_drop(c) {
                        continue;
                    }
                    let threads = c.sizes.get("threads").copied().unwrap_or(0) as u32;
                    let w = c.worker.as_deref();
                    let offered = offered_by(step).and_then(|m| c.sizes.get(m).copied()).unwrap_or(0);
                    let (mem, from) = book.mem(step, target, threads, offered);
                    let secs = book.secs_on(step, target, threads, w).map(|s| s.0);
                    let nas = book.nas_load(step, target, w).map(|(r, s)| r.bytes_s * s.unwrap_or(0.0));
                    let busy = book.cores(step, target).and_then(|c| c.busy_tenths);
                    let line = serde_json::json!({
                        "day": d, "key": k, "step": step, "worker": w, "threads": threads,
                        "p_secs": secs, "p_mem": mem, "mem_from": format!("{from:?}"), "p_nas": nas, "p_busy": busy,
                    });
                    writeln!(out, "{line}").unwrap();
                }
            }
            eprintln!(
                "day {d}: book of {} costs, {} records, lead at {:?} G insn/s, {:?}",
                book_costs.len(),
                records.len(),
                paces.lead_ginsn_s(),
                paces.macs
            );
        }
    }

    /// By hand (task #1586): the lead's grants replayed. `SCENIC_GRANTS_REPLAY` names a folder of
    /// `book.json` (the costs before the window, as `read_costs` reads them) and `SCENIC_GRANTS_RUNS`
    /// a file in it of the window's job runs in lease order, a JSON line each (`job`, `step`,
    /// `targets`, `threads`, `worker`, `lease_t`, `end_t`, `secs`, `grant`, `whole`, `ok`, each
    /// target's `peaks` and `sizes`). Each run is booked as the scheduler books it, from the costs
    /// of the runs ended before its lease (each target's cost its run's peak, `was_mb` its run
    /// before's of the way its step runs now on as many threads, as the lead sets it): the most of
    /// its targets' memory on its threads (`Book::mem`, the run's grant standing in for its plan's
    /// guess), a cold start's (`From::Table`) its provisional need (crate::coord::sched's
    /// `provisional_mb` of its step's earlier way's, else the grant). Writes `<runs>.booked.jsonl`, a
    /// line a run: the run's line less its peaks and sizes, with `booked`, `from` and `unspread` (as
    /// `booked`, its targets' own runs at their peaks alone, without their step's spread). Without the
    /// folder it prints that it skipped and checks nothing.
    #[test]
    fn replay_grants_on_the_leads_records() {
        use std::io::Write;
        let Ok(dir) = std::env::var("SCENIC_GRANTS_REPLAY") else {
            eprintln!("grants replay skipped: SCENIC_GRANTS_REPLAY unset (nothing checked)");
            return;
        };
        let dir = std::path::Path::new(&dir);
        let runs_at = dir.join(std::env::var("SCENIC_GRANTS_RUNS").unwrap());
        let mut costs = read_costs(&dir.join("book.json")).unwrap();
        let runs: Vec<serde_json::Value> =
            std::fs::read_to_string(&runs_at).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let the_plan = crate::agent::seam::for_tests();
        let (paces, records, cache) = (crate::paces::Paces::default(), Records::default(), Cache::default());
        let inputs = Inputs { paces: &paces, records: &records, tuning: Default::default() };
        let (floors, disks) = (BTreeMap::new(), BTreeMap::new());
        let mut ended: Vec<&serde_json::Value> = runs.iter().filter(|r| r["ok"] == true).collect();
        ended.sort_by(|a, b| a["end_t"].as_f64().unwrap().total_cmp(&b["end_t"].as_f64().unwrap()));
        let (mut next, mut rev) = (0usize, 0u64);
        let mut out = std::fs::File::create(runs_at.with_extension("booked.jsonl")).unwrap();
        for r in &runs {
            let at = r["lease_t"].as_f64().unwrap();
            while next < ended.len() && ended[next]["end_t"].as_f64().unwrap() <= at {
                let e = ended[next];
                next += 1;
                rev += 1;
                let step = e["step"].as_str().unwrap();
                let v = the_plan.cost_version(step);
                for t in e["targets"].as_array().unwrap() {
                    let t = t.as_str().unwrap();
                    let key = cost_key(step, t);
                    let mut sizes: BTreeMap<String, u64> =
                        serde_json::from_value(e["sizes"][t].clone()).unwrap_or_default();
                    sizes.insert("threads".into(), e["threads"].as_u64().unwrap());
                    sizes.remove("threads_peak");
                    let mut c = Cost {
                        peak_mb: e["peaks"][t].as_u64().unwrap(),
                        secs: e["secs"].as_f64().unwrap() as u64,
                        worker: e["worker"].as_str().map(str::to_string),
                        job: e["job"].as_str().and_then(|j| j.parse().ok()),
                        v,
                        sizes,
                        was_mb: None,
                    };
                    let threads = |c: &Cost| c.sizes.get("threads").copied();
                    c.was_mb = costs
                        .get(&key)
                        .filter(|k| k.v == v && threads(k) == threads(&c) && !is_drop(k) && k.peak_mb > 0)
                        .map(|k| k.peak_mb);
                    costs.insert(key, c);
                }
            }
            let book = Book::kept(the_plan, &costs, &floors, &disks, inputs, &cache, rev);
            let step = r["step"].as_str().unwrap();
            let threads = r["threads"].as_u64().unwrap() as u32;
            let grant = r["grant"].as_u64().unwrap_or(0);
            let (mut booked, mut from, mut unspread) = (0u64, From::Measured, 0u64);
            for t in r["targets"].as_array().unwrap() {
                let t = t.as_str().unwrap();
                let guess = match offered_by(step) {
                    Some(m) => r["sizes"][t][m].as_u64().unwrap_or(0),
                    None => grant,
                };
                let (mb, f) = match book.mem(step, t, threads, guess) {
                    (_, From::Table) => {
                        (crate::coord::sched::provisional_mb(book.older_mem(step, t).unwrap_or(grant)), From::Table)
                    }
                    m => m,
                };
                // (Its own run's peak alone, as booked before the spread.)
                let spread = book.own(step, t).map_or(0, |c| {
                    (c.peak_mb as f64 * book.model(step).reruns.spread(c.peak_mb as f64)).ceil() as u64 - c.peak_mb
                });
                unspread = unspread.max(mb - spread);
                if mb >= booked {
                    (booked, from) = (mb, f);
                }
            }
            let mut line = r.clone();
            let o = line.as_object_mut().unwrap();
            o.remove("peaks");
            o.remove("sizes");
            o.insert("booked".into(), booked.into());
            o.insert("unspread".into(), unspread.into());
            o.insert("from".into(), format!("{from:?}").into());
            writeln!(out, "{line}").unwrap();
        }
        eprintln!("grants replay: {} runs booked", runs.len());
    }

    /// A target held out of its step's costs (`held_out`): its step and cost key, the memory
    /// it took, and its step's model of the rest's memory for it (`model::Model::mem`, on its
    /// threads) by the sizes its run measured (`post`) and by those known before it runs alone
    /// (`pre`: `Book::sizes`, its tile's area, the measure its plan offers it by, `OFFERED_BY`, and
    /// its source's time), as a never-run target is read.
    struct HeldOut {
        step: String,
        key: String,
        taken: u64,
        post: Option<u64>,
        pre: Option<u64>,
    }

    /// Of `all` (each read as the way its step runs now), each step's largest target by each of
    /// its sizes (each target once) held out (`held_out`).
    fn held_out_cases(all: &BTreeMap<String, Cost>) -> Vec<HeldOut> {
        let mut by_step: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
        for k in all.keys() {
            by_step.entry(crate::coord::CostKey::parse(k).step).or_default().push(k);
        }
        let mut cases = std::collections::BTreeSet::new();
        for (step, keys) in &by_step {
            let keys: Vec<&&String> = keys.iter().filter(|k| !is_drop(&all[**k]) && all[**k].peak_mb > 0).collect();
            if keys.len() <= MIN_RUNS {
                continue;
            }
            let mut names: Vec<String> = keys.iter().flat_map(|k| sizes_of(k, Some(&all[**k])).into_keys()).collect();
            names.sort();
            names.dedup();
            for n in names {
                let size = |k: &&&String| sizes_of(k, Some(&all[**k])).get(&n).copied().unwrap_or(0.0);
                if let Some(k) = keys.iter().filter(|k| size(k) > 0.0).max_by(|a, b| size(a).total_cmp(&size(b))) {
                    cases.insert((step.to_string(), (**k).clone()));
                }
            }
        }
        cases.into_iter().map(|(_, k)| held_out(all, &k)).collect()
    }

    /// Cost `key` of `all` held out of the rest (`HeldOut`).
    fn held_out(all: &BTreeMap<String, Cost>, key: &str) -> HeldOut {
        let mut costs = all.clone();
        let c = costs.remove(key).unwrap();
        let k = crate::coord::CostKey::parse(key);
        let threads = own_threads(&c).unwrap_or(0);
        let book = Book::new(crate::agent::seam::for_tests(), &costs);
        let model = book.model(k.step);
        let mut sizes = sizes_of(key, Some(&c));
        sizes.insert(model::TARGET.to_string(), 1.0);
        let offered = offered_by(k.step).and_then(|m| Some((m, *c.sizes.get(m)?)));
        let pre = model.needs.mem(&book.sizes(k.step, k.target, offered), threads);
        HeldOut {
            post: model.needs.mem(&sizes, threads),
            pre,
            step: k.step.to_string(),
            key: key.to_string(),
            taken: c.peak_mb,
        }
    }

    /// `cases`' predictions by `of` over what they took: their count, geometric mean, worst over
    /// (with its key) and those under (key, ratio), in a line.
    fn held_out_score(cases: &[HeldOut], of: impl Fn(&HeldOut) -> Option<u64>) -> String {
        let (mut logs, mut worst, mut under) = (Vec::new(), (0.0f64, String::new()), Vec::new());
        for c in cases {
            let Some(p) = of(c) else { continue };
            let r = p.max(1) as f64 / c.taken.max(1) as f64;
            logs.push(det::Det::dln(r));
            if r > worst.0 {
                worst = (r, c.key.clone());
            }
            if r < 1.0 {
                under.push(format!("{} {r:.3}x", c.key));
            }
        }
        let gm = det::Det::dexp(logs.iter().sum::<f64>() / logs.len().max(1) as f64);
        format!(
            "{} cases: geometric mean {gm:.3}, worst over {:.1}x ({}), under {}: {}",
            logs.len(),
            worst.0,
            worst.1,
            under.len(),
            under.join(", ")
        )
    }

    /// Task #1542's acceptance: bldprep 6/47/27 (84 M rows, past every other run by them; in range
    /// by its bytes), held out of the step's runs (the lead's of 20 Nov, testdata/bldprep-costs-
    /// 20261120.json.zst, cut from its store), is read at no less than it took and no more than the
    /// step's spread allows: the most memory a row its other runs of a million rows or more took,
    /// times its rows; by the sizes known before it runs (its area and the rows its plan offers it
    /// by) as by those its run measured. (Read by its bytes alone it had been 517 MB, 0.05 of the
    /// 10,770 it took.)
    #[test]
    fn bldprep_6_47_27_held_out_is_read_within_its_steps_spread() {
        let z = include_bytes!("testdata/bldprep-costs-20261120.json.zst");
        let mut all: BTreeMap<String, Cost> = serde_json::from_slice(&zstd::decode_all(&z[..]).unwrap()).unwrap();
        for c in all.values_mut() {
            c.v = cost_version("bldprep");
        }
        let key = "bldprep 6/47/27";
        let rows = |c: &Cost| c.sizes.get("rows").copied().unwrap_or(0) as f64;
        let per_row: Vec<f64> = all
            .iter()
            .filter(|(k, c)| k.as_str() != key && !is_drop(c) && c.peak_mb > 0 && rows(c) >= 1e6)
            .map(|(_, c)| c.peak_mb as f64 / rows(c))
            .collect();
        let hi = per_row.iter().fold(0.0f64, |a, &x| a.max(x)) * rows(&all[key]);
        let case = held_out(&all, key);
        let lo = case.taken as f64;
        eprintln!(
            "{key}: {} MB taken, {:?} by its run's sizes, {:?} by those known before, at most {hi:.0} MB",
            case.taken, case.post, case.pre
        );
        for (by, p) in [("its run's sizes", case.post), ("the sizes known before it runs", case.pre)] {
            let p = p.unwrap() as f64;
            assert!((lo..=hi).contains(&p), "by {by}: {p} MB, outside {lo:.0}-{hi:.0} MB");
        }
    }

    /// By hand (task #1542's review): every step's model built from the lead's costs (as
    /// `held_out_memory_past_the_runs_on_the_leads_costs` reads them), timed: the whole build, best
    /// of three, and its three slowest steps'.
    #[test]
    fn model_build_time_on_the_leads_costs() {
        let Ok(dir) = std::env::var("SCENIC_NEEDS_HELDOUT") else {
            eprintln!("needs build time skipped: SCENIC_NEEDS_HELDOUT unset (nothing checked)");
            return;
        };
        let the_plan = crate::agent::seam::for_tests();
        let mut all = read_costs(&std::path::Path::new(&dir).join("costs.json")).unwrap();
        for (k, c) in all.iter_mut() {
            c.v = the_plan.cost_version(crate::coord::CostKey::parse(k).step);
        }
        let steps: std::collections::BTreeSet<&str> =
            all.keys().map(|k| crate::coord::CostKey::parse(k).step).collect();
        let mut best = (f64::INFINITY, Vec::new());
        for _ in 0..3 {
            let book = Book::new(the_plan, &all);
            let t0 = std::time::Instant::now();
            let mut each: Vec<(f64, &str)> = steps
                .iter()
                .map(|s| {
                    let t = std::time::Instant::now();
                    book.model(s);
                    (t.elapsed().as_secs_f64(), *s)
                })
                .collect();
            let whole = t0.elapsed().as_secs_f64();
            each.sort_by(|a, b| b.0.total_cmp(&a.0));
            if whole < best.0 {
                best = (whole, each.into_iter().take(3).map(|(t, s)| format!("{s} {:.0} ms", t * 1e3)).collect());
            }
        }
        println!("{} steps' models built in {:.3} s; slowest: {}", steps.len(), best.0, best.1.join(", "));
    }

    /// By hand (tasks #1535, #1542): the needs model's memory held out past its runs, on the lead's
    /// costs (`costs.json` in `SCENIC_NEEDS_HELDOUT`, `read_costs`'s; each read as the way its step
    /// runs now): `held_out_cases`, a line each (step, target, taken, predicted by the sizes its
    /// run measured and by those known before it runs), then both scores (`held_out_score`).
    /// Without the folder it prints that it skipped and checks nothing.
    #[test]
    fn held_out_memory_past_the_runs_on_the_leads_costs() {
        let Ok(dir) = std::env::var("SCENIC_NEEDS_HELDOUT") else {
            eprintln!("needs held-out skipped: SCENIC_NEEDS_HELDOUT unset (nothing checked)");
            return;
        };
        let the_plan = crate::agent::seam::for_tests();
        let mut all = read_costs(&std::path::Path::new(&dir).join("costs.json")).unwrap();
        for (k, c) in all.iter_mut() {
            c.v = the_plan.cost_version(crate::coord::CostKey::parse(k).step);
        }
        let cases = held_out_cases(&all);
        for c in &cases {
            println!(
                "{} {}: {} MB taken, {:?} by its run's sizes, {:?} by those known before",
                c.step, c.key, c.taken, c.post, c.pre
            );
        }
        println!("by its run's sizes: {}", held_out_score(&cases, |c| c.post));
        println!("by the sizes known before it runs: {}", held_out_score(&cases, |c| c.pre));
    }

    /// By hand (task #1415): the admission replay on the lead's records (docs/pool.md §7.7, Memory
    /// admission). For each day from `SCENIC_NEEDS_DAY0` (unix s), `SCENIC_NEEDS_DAYS` of them, a
    /// book of the runs that ended before it (and the floors of targets granted before it), and
    /// each job granted that day by its targets' costs: its memory as crate::coord::sched admits
    /// it, on the threads it starts on (`at_start`), and as counted once grown to the threads it
    /// ran on (`at_ran`), with what it took (`peak`), its grant (`at`), its time and whether a
    /// cold start. Reads
    /// `costs.json` (`read_costs`), `floors.json` (the lead's floors by cost key), `history.jsonl`
    /// and `timings.jsonl` in `SCENIC_ADMIT_REPLAY`; writes `<SCENIC_ADMIT_LABEL>.jsonl` (default
    /// `admit`), at `SCENIC_ADMIT_MEM_PCT` (`needs-mem-pct`) and `SCENIC_ADMIT_THREAD_SHARE`
    /// (`needs-thread-share`; not set, their defaults); `replay_admission_packed_into_rooms` packs
    /// them. The "before" of the docs' table was this test's lines on the scheduler it replaced
    /// (b982e9797). Without the folder it prints that it skipped and checks nothing.
    #[test]
    fn replay_admission_on_the_leads_records() {
        use std::io::Write;
        let Ok(dir) = std::env::var("SCENIC_ADMIT_REPLAY") else {
            eprintln!("admission replay skipped: SCENIC_ADMIT_REPLAY unset (nothing checked)");
            return;
        };
        let label = std::env::var("SCENIC_ADMIT_LABEL").unwrap_or_else(|_| "admit".into());
        let dir = std::path::Path::new(&dir);
        let all = read_costs(&dir.join("costs.json")).unwrap();
        let floors_all: BTreeMap<String, Floor> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("floors.json")).unwrap()).unwrap();
        let text = std::fs::read_to_string(dir.join("history.jsonl")).unwrap();
        let events: Vec<crate::coord::history::Event> =
            text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let log = std::fs::read_to_string(dir.join("timings.jsonl")).unwrap();
        let recs: Vec<(f64, &str)> = log
            .lines()
            .filter_map(|l| {
                let r: jobrt::timings::RunRec = serde_json::from_str(l).ok()?;
                Some((r.start as f64 + r.wall_s, l))
            })
            .collect();
        let the_plan = crate::agent::seam::for_tests();
        let lead = "Mac-mini-de-Brandon";
        let day0: u64 = std::env::var("SCENIC_NEEDS_DAY0").unwrap().parse().unwrap();
        let days: u64 = std::env::var("SCENIC_NEEDS_DAYS").unwrap().parse().unwrap();
        let mut out = std::fs::File::create(dir.join(format!("{label}.jsonl"))).unwrap();
        let granted = |c: &Cost| c.job.map(|j| j.n / 1000);
        for d in 0..days {
            let t0 = day0 + d * 86_400;
            let book_costs: BTreeMap<String, Cost> = all
                .iter()
                .filter(|(_, c)| granted(c).is_none_or(|g| g + c.secs < t0))
                .map(|(k, c)| (k.clone(), c.clone()))
                .collect();
            // (A floor of a target granted that day or later is the day's own: left out.)
            let floors: BTreeMap<String, Floor> = floors_all
                .iter()
                .filter(|(k, _)| all.get(*k).and_then(granted).is_none_or(|g| g < t0))
                .map(|(k, f)| (k.clone(), *f))
                .collect();
            let no_disks = BTreeMap::new();
            let ev: Vec<crate::coord::history::Event> = events.iter().filter(|e| e.t < t0).cloned().collect();
            let tail_at = dir.join(format!("tail-{label}-{d}.jsonl"));
            let mut f = std::fs::File::create(&tail_at).unwrap();
            for (_, l) in recs.iter().filter(|(end, _)| *end < t0 as f64) {
                writeln!(f, "{l}").unwrap();
            }
            drop(f);
            let mut tail = jobrt::timings::LogTail::default();
            tail.read(&tail_at);
            std::fs::remove_file(&tail_at).unwrap();
            let speeds = tail.speeds(crate::paces::mac_of, |_| true);
            let paces = crate::paces::Paces::of(&speeds, lead).with_starts(the_plan, &ev, &book_costs, t0);
            let records = tail.facts().clone();
            let cache = Cache::default();
            let tuning = jobrt::needs::Tuning::of(|n| {
                let var = match n {
                    "needs-mem-pct" => "SCENIC_ADMIT_MEM_PCT",
                    "needs-thread-share" => "SCENIC_ADMIT_THREAD_SHARE",
                    _ => return None,
                };
                std::env::var(var).ok()?.parse().ok()
            });
            let book = Book::kept(
                the_plan,
                &book_costs,
                &floors,
                &no_disks,
                Inputs { paces: &paces, records: &records, tuning },
                &cache,
                0,
            );
            // (Each job granted that day: its targets' costs.)
            let mut jobs: BTreeMap<String, Vec<(&str, &str, &Cost)>> = BTreeMap::new();
            for (k, c) in all.iter().filter(|(_, c)| granted(c).is_some_and(|g| (t0..t0 + 86_400).contains(&g))) {
                let crate::coord::CostKey { step, target } = crate::coord::CostKey::parse(k);
                if c.v < the_plan.cost_version(step) || is_drop(c) {
                    continue;
                }
                jobs.entry(c.job.unwrap().to_string()).or_default().push((step, target, c));
            }
            for (job, ts) in &jobs {
                let step = ts[0].0;
                let ran = ts.iter().filter_map(|t| t.2.sizes.get("threads").copied()).max().unwrap_or(0) as u32;
                // (As crate::coord::sched admits a job: each target on the threads it starts on,
                // `Sched::start_threads`, the most of them; a grow past those its memory covers,
                // `Sched::thread_mb`, each thread its step's memory per thread.)
                let on = if the_plan.records_only(step) {
                    1
                } else {
                    ts.iter().filter_map(|t| book.cores(step, t.1).map(|c| c.grant)).max().unwrap_or(0)
                };
                let start = if on == 0 { ran } else { on.min(ran.max(1)) };
                let guess = |c: &Cost| offered_by(step).and_then(|m| c.sizes.get(m).copied()).unwrap_or(0);
                let mut cold = false;
                let mut at_start = 0u64;
                let mut per: Option<u64> = Some(0);
                let mut least: Option<u32> = Some(u32::MAX);
                for &(st, t, c) in ts {
                    let (mb, from) = book.mem(st, t, on, guess(c));
                    cold |= from == From::Table;
                    at_start = at_start.max(mb);
                    per = per.zip(book.thread_mb(st, t)).map(|(a, b)| a.max(b));
                    least = least.zip(book.own(st, t).and_then(own_threads)).map(|(a, b)| a.min(b));
                }
                let (per, covered) = match per {
                    Some(p) => (p, on.max(least.filter(|&n| n != u32::MAX).unwrap_or(0)).max(1)),
                    None => (at_start, 1),
                };
                let at_ran = at_start + per * u64::from(ran.saturating_sub(start.max(covered)));
                let peak = ts.iter().map(|t| t.2.peak_mb).max().unwrap_or(0);
                let secs: u64 = ts.iter().map(|t| t.2.secs).sum();
                let worker = ts[0].2.worker.clone();
                let line = serde_json::json!({
                    "day": d, "job": job, "step": step, "worker": worker, "targets": ts.len(), "at": ts.iter().filter_map(|t| granted(t.2)).min(),
                    "ran": ran, "start": start, "at_start": at_start, "at_ran": at_ran, "per": per,
                    "peak": peak, "secs": secs, "cold": cold,
                });
                writeln!(out, "{line}").unwrap();
            }
            eprintln!("day {d}: book of {} costs, {} jobs", book_costs.len(), jobs.len());
        }
    }

    /// By hand (task #1415): the admission replay's jobs packed into each Mac's room (docs/pool.md
    /// §7.7, Memory admission). For each label of `SCENIC_ADMIT_PACK` (comma-separated), its
    /// `<label>.jsonl` in `SCENIC_ADMIT_REPLAY` (`replay_admission_on_the_leads_records`'s, or the
    /// scheduler's it replaced): on each Mac (its slots' workers together, `paces::mac_of`), by
    /// `SCENIC_ADMIT_ROOMS` (`name=MB,...`: its memory less its floor), its jobs in the order they
    /// were granted, each started once its figure (`at_ran`: as counted on the threads it ran on)
    /// fits beside the figures of those started before it and still running (one past the room
    /// alone: once none runs), never before its grant nor before the job granted before it, and
    /// run its time. What was reserved and what the jobs took past it (GB h: each job's figure,
    /// and its peak past it, over its time), the jobs past it and by over a GB; the hours jobs
    /// waited past their grant for room (the work admitted: less wait, more admitted); what their
    /// peaks together took past the room (GB h), and the jobs started while they did. Each step's
    /// too. Writes `<label>-packed.json`.
    #[test]
    fn replay_admission_packed_into_rooms() {
        let (Ok(dir), Ok(labels), Ok(rooms)) = (
            std::env::var("SCENIC_ADMIT_REPLAY"),
            std::env::var("SCENIC_ADMIT_PACK"),
            std::env::var("SCENIC_ADMIT_ROOMS"),
        ) else {
            eprintln!("admission packing skipped: SCENIC_ADMIT_REPLAY, _PACK or _ROOMS unset (nothing checked)");
            return;
        };
        let rooms: BTreeMap<String, u64> = rooms
            .split(',')
            .filter_map(|r| r.split_once('=').map(|(n, mb)| (n.to_string(), mb.parse().unwrap())))
            .collect();
        #[derive(Default, Clone, Copy, serde::Serialize)]
        struct Sums {
            jobs: u64,
            work_h: f64,
            wait_h: f64,
            reserved_gbh: f64,
            past_gbh: f64,
            past: u64,
            past_gb: u64,
            over_room_gbh: f64,
            over_room_jobs: u64,
        }
        for label in labels.split(',') {
            let text = std::fs::read_to_string(std::path::Path::new(&dir).join(format!("{label}.jsonl"))).unwrap();
            let lines: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
            let mut by: BTreeMap<String, Sums> = BTreeMap::new();
            let mut all = Sums::default();
            // (Each Mac's jobs: granted at, time, figure, peak, step.)
            let mut macs: BTreeMap<String, Vec<(u64, u64, u64, u64, String)>> = BTreeMap::new();
            for l in &lines {
                let (Some(w), Some(at)) = (l["worker"].as_str().map(crate::paces::mac_of), l["at"].as_u64()) else {
                    continue;
                };
                if !rooms.contains_key(w) {
                    continue;
                }
                let n = |k: &str| l[k].as_u64().unwrap_or(0);
                let step = l["step"].as_str().unwrap_or("").to_string();
                macs.entry(w.to_string()).or_default().push((at, n("secs"), n("at_ran"), n("peak"), step));
            }
            let gbh = |mb: u64, secs: u64| mb as f64 * secs as f64 / 1024.0 / 3600.0;
            for (w, mut jobs) in macs {
                let room = rooms[&w];
                jobs.sort();
                // (Started: start, end, figure, peak, granted.)
                let mut started: Vec<(u64, u64, u64, u64, u64)> = Vec::new();
                let mut after = 0u64;
                for (at, secs, fig, peak, _) in &jobs {
                    let mut t = (*at).max(after);
                    loop {
                        let beside: Vec<&(u64, u64, u64, u64, u64)> =
                            started.iter().filter(|j| j.0 <= t && j.1 > t).collect();
                        if beside.is_empty() || beside.iter().map(|j| j.2).sum::<u64>() + fig <= room {
                            break;
                        }
                        t = beside.iter().map(|j| j.1).min().unwrap();
                    }
                    after = t;
                    started.push((t, t + secs, *fig, *peak, *at));
                }
                for ((t, end, fig, peak, at), (.., step)) in started.iter().zip(&jobs) {
                    let secs = end - t;
                    let at_once: u64 = started.iter().filter(|j| j.0 <= *t && j.1 > *t).map(|j| j.3).sum();
                    let s = by.entry(step.clone()).or_default();
                    for s in [&mut *s, &mut all] {
                        s.jobs += 1;
                        s.work_h += secs as f64 / 3600.0;
                        s.wait_h += (t - at) as f64 / 3600.0;
                        s.reserved_gbh += gbh(*fig, secs);
                        s.past_gbh += gbh(peak.saturating_sub(*fig), secs);
                        s.past += u64::from(peak > fig);
                        s.past_gb += u64::from(*peak > fig + 1024);
                        s.over_room_jobs += u64::from(at_once > room);
                    }
                }
                // (Their peaks together against the room.)
                let mut ev: Vec<(u64, i64)> =
                    started.iter().flat_map(|j| [(j.0, j.3 as i64), (j.1, -(j.3 as i64))]).collect();
                ev.sort_by_key(|e| (e.0, e.1));
                let (mut cur, mut last) = (0i64, 0u64);
                for (t, d) in ev {
                    all.over_room_gbh += gbh((cur - room as i64).max(0) as u64, t.saturating_sub(last));
                    cur += d;
                    last = t;
                }
            }
            let out = serde_json::json!({ "all": all, "steps": by });
            std::fs::write(std::path::Path::new(&dir).join(format!("{label}-packed.json")), out.to_string()).unwrap();
            eprintln!("{label}: {}", serde_json::to_string(&all).unwrap());
            for (step, s) in &by {
                eprintln!("{label} {step}: {}", serde_json::to_string(s).unwrap());
            }
        }
    }

    #[test]
    fn tiles_of_keys() {
        assert_eq!(tile("tail 6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("marks"), None);
    }

    /// Task #1528: a target's own run on fewer threads than it starts on grows by its own peak over
    /// its threads (task #1584: by its step's rule, the fitted memory per thread, at least
    /// `thread_share` of that), not by its siblings' runs scaled up to its size. The lead's inputs
    /// runs of 19 Nov: inputs/regions took 4,590 MB on 1
    /// thread, granted 2 by its own cores (0.7 busy and a thread more); its 9 siblings took
    /// 20-221 MB, each having created 1 file to its 2,012, so the model's figure for it, by files
    /// created, scaled each up 2,012 times: 207,116 MB a thread, 211,706 MB (206.7 GB) on 2,
    /// past every Mac, and the input gate's check waited.
    #[test]
    fn an_own_run_grows_by_its_own_peak_a_thread_not_its_siblings_scaled_to_its_size() {
        // (Target, peak MB, s, threads, CPU ms, bytes, files created, listed, written.)
        let runs: [(&str, u64, u64, u64, u64, u64, u64, u64, u64); 10] = [
            ("descriptions", 20, 1, 1, 8, 22_301, 1, 58, 4_335),
            ("elevation", 137, 4, 1, 1_543, 2_465_623_724, 1, 23, 1_599),
            ("extents", 25, 1, 1, 56, 461_104, 1, 971, 71_602),
            ("heritage", 221, 8, 1, 1_120, 183_363_733, 1, 81, 5_237),
            ("languages", 41, 1, 1, 37, 27_844_439, 1, 3, 248),
            ("regions", 4_590, 6_199, 1, 4_389_031, 1_239_655_417, 2_012, 3_393, 196_503_074),
            ("sources", 58, 2, 1, 73, 28_463_715, 1, 42, 2_855),
            ("timetables/feeds", 23, 6, 1, 44, 30_580_208, 1, 48, 3_464),
            ("timetables/gtfs", 20, 0, 2, 5, 8_629, 1, 27, 1_783),
            ("translations", 20, 1, 1, 11, 3_341, 1, 12, 805),
        ];
        let costs: BTreeMap<String, Cost> = runs
            .iter()
            .map(|&(t, peak, secs, threads, cpu_ms, bytes, creates, listed, wrote)| {
                let sizes = [
                    ("threads", threads),
                    ("cpu_ms", cpu_ms),
                    ("bytes", bytes),
                    ("creates", creates),
                    ("listed", listed),
                    ("stats", listed),
                    ("wrote", wrote),
                    ("read_back", wrote),
                ];
                (cost_key("inputs", &format!("inputs/{t}")), cost(peak, secs, &sizes))
            })
            .collect();
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        let t = "inputs/regions";
        assert_eq!(b.cores("inputs", t).map(|c| c.grant), Some(2), "its own run's 0.7 busy cores and a thread more");
        let (md, sizes) = (b.model("inputs"), b.sizes("inputs", t, None));
        let model = md.needs.thread_mb(&sizes).unwrap();
        assert!(model < 5_000, "its siblings grown by what a file more took (task #1535), not 207,116 MB: {model}");
        // (Task #1584: its step's runs, timetables/gtfs on 2 threads, fit nothing a thread, so a
        // thread past its own run's is its step's least share of its peak: 50 %, not its whole.)
        assert_eq!(md.needs.per_thread_of(&sizes, 4_590.0), 2_295.0);
        assert_eq!(b.thread_mb("inputs", t), Some(2_295), "half its own peak a thread, not 207,116 MB");
        assert_eq!(b.mem("inputs", t, 1, 0), (4_590, From::Measured));
        assert_eq!(b.mem("inputs", t, 2, 0), (6_885, From::Measured), "not 211,706 MB, nor 9,180");
        // (A target never run is still read by its step's runs grown to its size.)
        assert!(b.thread_mb("inputs", "inputs/new").is_some());
    }

    /// Task #1528 review: where its step's fitted memory per thread is past a target's own peak over
    /// its threads, each thread past them grows by the fitted figure: its step's runs say a thread
    /// holds about 1,000 MB whatever its target, and its own run on 2 threads took 300.
    #[test]
    fn an_own_run_grows_by_its_steps_fitted_slope_where_that_is_more() {
        let on = |peak: u64, threads: u64| Cost {
            v: cost_version("slope"),
            ..cost(peak, 60, &[("threads", threads), ("cpu_ms", 60_000 * threads)])
        };
        let mut costs: BTreeMap<String, Cost> =
            (1..=10).map(|t| (cost_key("slope", &format!("t{t}")), on(100 + 1_000 * t, t))).collect();
        costs.insert(cost_key("slope", "small"), on(300, 2));
        let b = Book::new(crate::agent::seam::for_tests(), &costs);
        let fitted = b.model("slope").needs.per_thread_of(&b.sizes("slope", "small", None), 150.0);
        assert!(fitted > 150.0, "past its own 300 MB over 2 threads: {fitted}");
        let per = fitted.ceil() as u64;
        assert_eq!(b.thread_mb("slope", "small"), Some(per));
        assert_eq!(b.mem("slope", "small", 4, 0), (300 + 2 * per, From::Measured));
    }
}
