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
//!   job's targets are counted to a quarter hour by and the forecast sums (task #433: by the 90th
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

/// A step's model of one measure, as made from its runs: its fits (best first) and its prior.
#[derive(Clone, Debug, Default)]
struct Model {
    fits: Vec<Fit>,
    prior: Option<u64>,
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

/// The models made, kept until the measures they're made from change: the change count they were
/// made at (the owner's: the coordinator bumps its count with each cost, floor or disk floor it
/// takes), by (step, measure); each step's most cores (`Book::cores`) and NAS prior
/// (`Book::nas_prior`) alike.
#[derive(Debug, Default)]
pub struct Cache(
    Mutex<(u64, BTreeMap<(String, Measure), Model>)>,
    Mutex<(u64, BTreeMap<String, Option<Cores>>)>,
    Mutex<(u64, BTreeMap<String, Option<crate::budget::nas::Rate>>)>,
    /// Each target's `Book::nas` (step → target → it): the NAS budget weighs each target offered
    /// and leased for every ask.
    Mutex<(u64, Rates)>,
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
                .filter(|(k, _)| !matches!(k.as_str(), "threads" | "cpu_ms" | "paused_ms"))
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
/// weighed the whole NAS) counts its writes alone.
fn nas_rate(step: &str, c: &Cost) -> Option<crate::budget::nas::Rate> {
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
    (c.v >= cost_version(step) && c.sizes.contains_key("bytes")).then(|| crate::budget::nas::Rate {
        bytes_s: read_s + g("wrote") / secs,
        creates_s: g("creates") / secs,
        listed_s: g("listed") / secs,
        stats_s: g("stats") / secs,
        deletes_s: g("deletes") / secs,
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

/// How many runs the way a step runs now its time prior takes alone (`Book::prior_of`); fewer, its
/// latest earlier way's mean stands in when larger.
pub const PRIOR_RUNS: usize = 20;

impl<'a> Book<'a> {
    /// A Book over `costs` alone, keeping its own models.
    pub fn new(costs: &'a BTreeMap<String, Cost>) -> Self {
        Book { costs, floors: None, disks: None, cache: CacheRef::Own(Cache::default()), rev: 0 }
    }

    /// A Book over all three, keeping its own models.
    pub fn with(
        costs: &'a BTreeMap<String, Cost>,
        floors: Option<&'a BTreeMap<String, Floor>>,
        disks: Option<&'a BTreeMap<String, u64>>,
    ) -> Self {
        Book { costs, floors, disks, cache: CacheRef::Own(Cache::default()), rev: 0 }
    }

    /// A Book whose models are kept in `cache` while its owner's change count is `rev`.
    pub fn kept(
        costs: &'a BTreeMap<String, Cost>,
        floors: &'a BTreeMap<String, Floor>,
        disks: &'a BTreeMap<String, u64>,
        cache: &'a Cache,
        rev: u64,
    ) -> Self {
        Book { costs, floors: Some(floors), disks: Some(disks), cache: CacheRef::Kept(cache), rev }
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
                Measure::Secs => c.secs,
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
        let md = Model { fits, prior: self.prior_of(step, m) };
        cache.0.lock().unwrap_or_else(|e| e.into_inner()).1.insert(key, md.clone());
        md
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
                let weighed = |c: &Cost| time_weight(step, c).map(|w| (c.secs, w));
                let secs: Vec<(u64, f64)> =
                    self.costs.iter().filter(|(k, c)| of(k) && c.v >= v).filter_map(|(_, c)| weighed(c)).collect();
                // (While its runs the way it runs now are few, they are the quick ones: the first to
                // end. Its latest earlier way's mean stands in when larger, so its first jobs aren't sized by
                // them: depth's first 8 runs at version 1 took 14–34 s but one, and jobs of 64 tiles
                // ran 90 min against `JOB_S`'s quarter hour.)
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
        if !md.fits.iter().any(Fit::good) {
            return None;
        }
        // (Its sizes: as its own earlier run measured them, else its area.)
        let sizes = sizes_of(key, self.costs.get(key));
        let t = (threads > 0).then_some(threads as f64);
        md.fits.iter().filter(|f| f.good()).find_map(|f| sizes.get(&f.size).map(|&x| f.at(x, t).ceil() as u64))
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
        match (self.fitted(&md, &key, threads), md.prior) {
            (Some(p), _) => (p, From::Fit),
            (None, Some(_)) if guess > 0 && GUESS_FIRST.contains(&step) => (guess, From::Guess),
            (None, Some(p)) => (p, From::Prior),
            (None, None) => (guess, From::Table),
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
    /// step's prior; None when none of its step ran the way it runs now.
    pub fn secs(&self, step: &str, target: &str, threads: u32) -> Option<u64> {
        let key = cost_key(step, target);
        if let Some(c) = self.costs.get(&key).filter(|c| c.v >= cost_version(step)) {
            return Some(c.secs);
        }
        let md = self.model(step, Measure::Secs);
        self.fitted(&md, &key, threads).or(md.prior)
    }

    /// What a drop of `step` (`DROPS`) takes, by its step's drops run the way it runs now: their
    /// mean time (s, rounded up) and the 90th percentile of their memory (MB); None while none ran.
    pub fn drop_need(&self, step: &str) -> Option<(u64, u64)> {
        let v = cost_version(step);
        let runs: Vec<&Cost> = self
            .costs
            .iter()
            .filter(|(k, c)| step_of_key(k) == step && c.v >= v && is_drop(c))
            .map(|(_, c)| c)
            .collect();
        let secs = weighted_mean(&runs.iter().map(|c| (c.secs, 1.0)).collect::<Vec<_>>())?;
        Some((secs, p90(runs.iter().map(|c| c.peak_mb).collect()).unwrap_or(0)))
    }

    /// `secs` with its step's prior the 90th percentile of its runs the way it runs now instead of
    /// their mean: what an update's drain waits for a target under way (crate::agent's
    /// `update_wait`), where a long tail's long jobs must not be cut short.
    pub fn secs_p90(&self, step: &str, target: &str, threads: u32) -> Option<u64> {
        let key = cost_key(step, target);
        let v = cost_version(step);
        if let Some(c) = self.costs.get(&key).filter(|c| c.v >= v) {
            return Some(c.secs);
        }
        let md = self.model(step, Measure::Secs);
        self.fitted(&md, &key, threads).or_else(|| {
            p90(self
                .costs
                .iter()
                .filter(|(k, c)| step_of_key(k) == step && c.v >= v && time_weight(step, c).is_some())
                .map(|(_, c)| c.secs)
                .collect())
        })
    }

    /// The cores a job of `step` for `target` takes, as measured (crate::costin's `sizes`): its own
    /// run's, else the most of its step's runs the way it runs now (each of `Cores` its most); None
    /// when none of them says (what packs jobs beside each other, crate::coord's packing, then
    /// packs none beside it). (A run waiting on a crowded NAS reads less busy, which would pack more
    /// jobs, each reading less busy still; but a target not run the way it runs now, nearly every
    /// one a job is granted for, counts for its step's most, which no crowded run lowers: task #697.)
    pub fn cores(&self, step: &str, target: &str) -> Option<Cores> {
        let v = cost_version(step);
        let of = |c: &Cost| {
            let t = *c.sizes.get("threads").filter(|_| c.v >= v)? as u32;
            // (Its CPU time, `cpu_ms`, over the time it ran: its wall time less the time it was
            // stopped (`paused_ms`), a second at least: the cores it kept busy.)
            let ran = (c.secs * 1000).saturating_sub(c.sizes.get("paused_ms").copied().unwrap_or(0)).max(1000);
            let kept = c.sizes.get("cpu_ms").map(|&ms| ms as f64 / ran as f64);
            Some(match kept {
                None => Cores { grant: t.max(1), busy_tenths: None },
                Some(k) => {
                    let b = k.ceil() as u32;
                    // (Three quarters of them or more: it may have used more, twice them, so a
                    // grant can climb back (crate::coord caps it at the Mac's cores); else one
                    // more than it kept busy (a serial step's: 1–2).)
                    let grant = if b * 4 >= t * 3 { t * 2 } else { (b + 1).min(t) }.max(1);
                    Cores { grant, busy_tenths: Some(((k * 10.0).round() as u32).clamp(1, grant * 10)) }
                }
            })
        };
        if let Some(c) = self.costs.get(&cost_key(step, target)).and_then(of) {
            return Some(c);
        }
        let most = || {
            self.costs.iter().filter(|(k, _)| step_of_key(k) == step).filter_map(|(_, c)| of(c)).reduce(|a, b| Cores {
                grant: a.grant.max(b.grant),
                busy_tenths: a.busy_tenths.zip(b.busy_tenths).map(|(a, b)| a.max(b)),
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

    /// `nas`'s own run's rate alone.
    pub fn nas_own(&self, step: &str, target: &str) -> Option<crate::budget::nas::Rate> {
        self.costs.get(&cost_key(step, target)).and_then(|c| nas_rate(step, c))
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

    /// `nas_prior` worked out: its step's runs' 90th percentile of each rate.
    fn nas_p90(&self, step: &str) -> Option<crate::budget::nas::Rate> {
        let runs: Vec<crate::budget::nas::Rate> =
            self.costs.iter().filter(|(k, _)| step_of_key(k) == step).filter_map(|(_, c)| nas_rate(step, c)).collect();
        // (In thousandths of a byte or file a second, for the percentile's integers.)
        let p = |f: fn(&crate::budget::nas::Rate) -> f64| {
            p90(runs.iter().map(|r| (f(r) * 1000.0).ceil() as u64).collect()).map(|x| x as f64 / 1000.0)
        };
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
            if !fits.first().is_some_and(Fit::good) {
                poor.entry(s).or_default().push(m.name());
            }
        }
    }
    out += &format!("\nheld out: {FOLDS} folds; poor past {:.0}% missed or {:.0}% median error, or under {MIN_RUNS} runs; a fit's prediction is its envelope × {MARGIN}; capped at its runs' most; prior err: the step's prior (memory and disk its runs' 90th percentile, time their mean) taken as each run's (median)\n", MAX_MISS * 100.0, MAX_ERR * 100.0);
    out += "steps that don't fit (they take their prior; units and candidates their piece's guess first):\n";
    for (s, ms) in &poor {
        out += &format!("  {s}: {}\n", ms.join(", "));
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
        // (Its CPU time over its wall time: a serial run given 8 threads is granted 2 and counted
        // for 0.9 cores, a busy one granted twice its 8 and counted for 8.)
        costs.insert(cost_key("pois", "6/1/1"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 9_000)])));
        costs.insert(cost_key("pois", "6/1/2"), now("pois", cost(100, 10, &[("threads", 8), ("cpu_ms", 80_000)])));
        let b = Book::new(&costs);
        assert_eq!(
            (b.cores("pois", "6/1/1"), b.cores("pois", "6/1/2")),
            (c(2, Some(9)), c(16, Some(80))),
            "a busy run climbs back"
        );
        // (Task #688: terrain-hi as measured on 26 Oct, 1.16 cores busy of 3 threads: granted 3,
        // counted for 1.2 (task #697: to the tenth, not rounded down to 1); its step's most, each
        // the most of its runs'.)
        let v = cost_version("terrain-hi");
        costs.insert(
            cost_key("terrain-hi", "6/48/16"),
            Cost { v, ..cost(3700, 1000, &[("threads", 3), ("cpu_ms", 1_160_000)]) },
        );
        costs.insert(
            cost_key("terrain-hi", "6/48/17"),
            Cost { v, ..cost(3700, 1000, &[("threads", 2), ("cpu_ms", 1_600_000)]) },
        );
        let b = Book::new(&costs);
        assert_eq!(b.cores("terrain-hi", "6/48/16"), c(3, Some(12)));
        assert_eq!(b.cores("terrain-hi", "6/48/17"), c(4, Some(16)));
        assert_eq!(b.cores("terrain-hi", "6/48/18"), c(4, Some(16)));
        // (Task #697: a run frozen by the memory guard for 420 of its 1000 s kept 2 cores busy over
        // the 580 s it ran, not 1.16.)
        costs.insert(
            cost_key("terrain-hi", "6/48/16"),
            Cost { v, ..cost(3700, 1000, &[("threads", 3), ("cpu_ms", 1_160_000), ("paused_ms", 420_000)]) },
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
    /// job's targets are counted to a quarter hour by), not their 90th percentile (task #433: by
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
        assert_eq!(b.secs_p90("peaks", "6/99/1", 0), Some(1200));
        assert_eq!(b.secs_p90("peaks", "6/1/1", 0), Some(100), "a measured one its own");
        assert_eq!(b.secs_p90("pois", "6/1/1", 0), None);
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
        assert_eq!(b.secs_p90("pyramids-fill", "6/55/25", 2), None);
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
        assert_eq!(b.secs_p90("pyramids-fill", "6/55/25", 2), Some(4950));
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

    #[test]
    fn tiles_of_keys() {
        assert_eq!(tile("tail 6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("marks"), None);
    }
}
