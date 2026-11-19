//! What a job of a step needs (task #236): one model per step, read by every placement path (the
//! coordinator's picks, its floors, the forecast). Nothing here is a figure kept by hand: a need is
//! measured (the target's own run, its floor), its place's (its nearest measured targets), the
//! step's needs learned from its runs (`model`, task #1416), or the caller's first guess (the
//! steps table's, the size its plan offered, a z6 tile's extract-size guess).
//!
//! Only what was measured the way the step runs now counts (crate::coord::cost_version): a run or a
//! floor of an older way of it (a z6 tile's before its v2, whose measures held malloc's cached freed
//! blocks) is neither a measure nor one of the model's runs.
//!
//! - **Measured**: the target's own run, never below its floor (crate::agent::memguard::Floor); a
//!   floor alone counts as its measure. Its time on another Mac at that Mac's speed (crate::paces).
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
//!   coordinator sizes it from the step's runs of its latest earlier way, `Book::older_mem`, else
//!   its offered guess; with neither it runs alone, crate::coord::sched).
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
/// else read in place: reserved only on a Mac with that room (crate::coord's `missing`).
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
/// not a `GUESS_FIRST` step's), and its latest earlier way's (`cost_version`) runs' high
/// percentile of memory (jobrt::needs::Tuning::high_pct; `Book::older_mem`).
#[derive(Debug, Default)]
pub struct Models {
    pub needs: model::Model,
    pub near: Option<Arc<Near>>,
    older: Option<(u32, u64)>,
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
                .filter(|(k, _)| !matches!(k.as_str(), "threads" | "threads_least") && !k.ends_with("_ms"))
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
    let g = |k: &str| c.sizes.get(k).copied().unwrap_or(0) as f64;
    // (A halo step's writes are spread over its job's time too, not over its own write phase:
    // pack's `secs` is that phase's 0-1 s, which weighed a pack at the whole NAS, 25 Oct.)
    let halo = HALO_STEPS.contains(&step);
    let secs = if halo { c.secs.max(c.sizes.get("nas_secs").copied().unwrap_or(0)) } else { c.secs }.max(1) as f64;
    let read_s = if halo {
        c.sizes.get("nas_secs").filter(|&&s| s > 0).map_or(0.0, |&s| g("nas_read") / s as f64)
    } else {
        (g("bytes") - g("local") + g("held")).max(0.0) / secs
    };
    // (What its uploads read back to check them: as measured, else, on a line from before it was
    // (task #1142), its `wrote`: each file costin counts made is a store::naming upload, read
    // back whole once it's written, as every one has been since the store began. So a step's old
    // measures weigh it as its new ones do, and no `cost_v` is bumped for it.)
    let back = c.sizes.get("read_back").map_or(g("wrote"), |&b| b as f64);
    (c.v >= the_plan.cost_version(step) && c.sizes.contains_key("bytes")).then(|| {
        let r = jobrt::budget::nas::Rate {
            bytes_s: read_s + (g("wrote") + back) / secs,
            creates_s: g("creates") / secs,
            listed_s: g("listed") / secs,
            stats_s: g("stats") / secs,
            deletes_s: g("deletes") / secs,
        };
        (r, secs)
    })
}

/// What run `c` of `step` moved on the NAS (`model::Moves`), as `nas_rate` counts it, its totals: a
/// `HALO_STEPS` step's reads its job's (`nas_read`), shared among the `shared` targets of its job
/// in the book. None where it doesn't say its `bytes`.
fn moves_of(step: &str, c: &Cost, shared: f64) -> Option<model::Moves> {
    if !c.sizes.contains_key("bytes") {
        return None;
    }
    let g = |k: &str| c.sizes.get(k).copied().unwrap_or(0) as f64;
    let read = if HALO_STEPS.contains(&step) {
        g("nas_read") / shared.max(1.0)
    } else {
        (g("bytes") - g("local") + g("held")).max(0.0)
    };
    let back = c.sizes.get("read_back").map_or(g("wrote"), |&b| b as f64);
    Some(model::Moves {
        bytes: read + g("wrote") + back,
        creates: g("creates"),
        listed: g("listed"),
        stats: g("stats"),
        deletes: g("deletes"),
    })
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
        // (How many targets each job ran: a halo step's job's reads are shared among them.)
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
            let shared = c.job.and_then(|j| of_job.get(&j)).copied().unwrap_or(1.0);
            out.push(model::Obs {
                order: c.job.map_or(0, |j| j.n),
                key: k.clone(),
                sizes,
                threads: c.sizes.get("threads").copied().unwrap_or(0) as u32,
                work,
                wait,
                wall: (ran - wait) / ratio + wait,
                peak: c.peak_mb.max(self.current_floor(step, k).unwrap_or(0)),
                nas: moves_of(step, c, shared),
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
        let older = way.and_then(|w| {
            let peaks =
                self.costs.iter().filter(|(k, c)| of(k, c) && c.v == w && c.peak_mb > 0).map(|(_, c)| c.peak_mb);
            let peaks = model::sorted(peaks.map(|p| p as f64).collect());
            model::pct(&peaks, self.tuning.high_pct).map(|p| (w, p.ceil() as u64))
        });
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
        let m = Arc::new(Models { needs, near, older });
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
    /// start after its way changed (task #979): that way's runs' high percentile, or the target's
    /// own run of it, the larger; None when no run of an earlier way is kept. Floors of an earlier
    /// way stay out (one held malloc's cached freed blocks, task #499), and so do drops.
    pub fn older_mem(&self, step: &str, target: &str) -> Option<u64> {
        let (way, p90) = self.model(step).older?;
        let own =
            self.costs.get(&cost_key(step, target)).filter(|c| c.v == way && !is_drop(c)).map_or(0, |c| c.peak_mb);
        Some(p90.max(own))
    }

    /// The sizes target `target` of `step` is predicted by: its own earlier run's (of any way),
    /// else its tile's area; the measure it was offered by (`offered`, `OFFERED_BY`) where its run
    /// doesn't say it; one target.
    fn sizes(&self, step: &str, target: &str, offered: Option<(&str, u64)>) -> BTreeMap<String, f64> {
        let key = cost_key(step, target);
        let mut sizes = sizes_of(&key, self.costs.get(&key));
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

    /// The memory (MB) a job of `step` for `target` needs, and where it came from: `need`'s, alone
    /// (what the picks read). `guess`: the caller's first guess (MB); for a step offered by an input
    /// measure (`OFFERED_BY`), that measure of the target, its memory guess none.
    pub fn mem(&self, step: &str, target: &str, threads: u32, guess: u64) -> (u64, From) {
        let (guess, offered) = match offered_by(step) {
            Some(m) => (0, Some((m, guess))),
            None => (guess, None),
        };
        let key = cost_key(step, target);
        let floor = self.current_floor(step, &key);
        if let Some(c) = self.own(step, target) {
            return (c.peak_mb.max(floor.unwrap_or(0)), From::Measured);
        }
        if let Some(f) = floor {
            return (f, From::Measured);
        }
        let md = self.model(step);
        if let Some(p) = md.near.as_ref().filter(|n| n.good()).and_then(|n| n.of(&key)) {
            return (p, From::Near);
        }
        let few = md.needs.runs < MIN_RUNS;
        match md.needs.mem(&self.sizes(step, target, offered), threads) {
            _ if few && guess > 0 && GUESS_FIRST.contains(&step) => (guess, From::Guess),
            Some(p) => (p, From::Model),
            None => (guess, From::Table),
        }
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

/// `scenic pool needs`: per step, its runs learned from, its sizes best first with their
/// needs per unit, its parallelism, its memory by place, and the runs flagged off their predictions.
pub fn model_table(book: &Book) -> String {
    let mut steps: Vec<&str> = book.costs.keys().map(|k| step_of_key(k)).collect();
    steps.sort();
    steps.dedup();
    let t = &book.tuning;
    let mut out = format!(
        "per unit of its sizes, best first (the least spread of work per unit): work in the lead's CPU s and the rest of its time in s at the {}th percentile of its runs; memory at the {}th of its runs that size or more; NAS bytes a second (at the lead's pace) of its runs of about one unit, their bytes over their time\n\n",
        t.work_pct, t.high_pct
    );
    for s in &steps {
        let md = book.model(s);
        let n = &md.needs;
        if n.runs == 0 {
            continue;
        }
        let since = n.since.as_deref().map_or(String::new(), |k| format!(", relearned from {k}"));
        let busy = n.par.busy.map_or("-".to_string(), |b| format!("{b:.1}"));
        out += &format!(
            "{s:<14} {} runs, its times from {}{since}; parallelism {:.1} (floor {}, busy {busy}, of {} runs)\n",
            n.runs,
            n.learned,
            n.par.cores(),
            n.par.floor,
            n.par.runs
        );
        for p in n.by.iter().take(3) {
            let f = |x: Option<f64>| x.map_or("-".to_string(), |x| format!("{x:.4}"));
            let mid = p.mem(1.0, 0).map_or("-".to_string(), |m| format!("{m} MB"));
            let nas = p.rate(1.0).map_or("-".to_string(), |m| format!("{:.0} B/s", m.bytes));
            out += &format!(
                "  per {:<10} {:>5} runs, spread {:.2}: work {} s, rest {:.4} s, NAS {nas}, memory at one {mid}, {:.0} MB a thread\n",
                p.size,
                p.runs,
                p.spread,
                f(p.whole.work),
                p.whole.rest,
                p.mem_per_thread
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
                job: None,
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
    use crate::coord::cost_version;

    fn cost(peak: u64, secs: u64, sizes: &[(&str, u64)]) -> Cost {
        Cost {
            job: None,
            peak_mb: peak,
            secs,
            worker: None,
            v: 0,
            sizes: sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
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
        let facts = |clean: bool, ginsn: f64| jobrt::timings::JobFacts { clean, ginsn, cpu_s: 100.0, explain: 1.0 };
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
        let b = Book::with(crate::agent::seam::for_tests(), &costs, Some(&floors), None);
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
    /// earlier way (their high percentile, or its own run of it, the larger), floors and older ways
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
        assert_eq!(b.older_mem("terrain-hi", "6/40/1"), Some(955), "the 95th percentile");
        assert_eq!(b.older_mem("terrain-hi", "6/10/1"), Some(1000), "its own run");
        assert_eq!(b.older_mem("terrain-hi", "6/20/1"), Some(955), "an older way's: nothing");
        assert_eq!(b.mem("terrain-hi", "6/40/1", 0, 0).1, From::Table, "still a cold start");
        assert_eq!(Book::new(crate::agent::seam::for_tests(), &BTreeMap::new()).older_mem("terrain-hi", "6/1/1"), None);
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
        assert!((p.mem_per_thread - 100.0).abs() < 1.0, "{}", p.mem_per_thread);
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
        let (all, _) = read_costs(&dir.join("costs.json")).unwrap();
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

    #[test]
    fn tiles_of_keys() {
        assert_eq!(tile("tail 6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("marks"), None);
    }
}
