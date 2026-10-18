//! What a job of a step needs (task #236): one model per step, read by every placement path (the
//! coordinator's picks, its floors, the forecast). Nothing here is a figure kept by hand: a need is
//! measured (the target's own run, its floor), fitted over the step's measured runs, the step's
//! prior from its measured runs, or the caller's first guess (the steps table's, the size its plan
//! offered, a unit's piece-size guess).
//!
//! Only what was measured the way the step runs now counts (crate::coord::cost_version): a run or a
//! floor of an older way of it (a unit's before its v2, whose measures held malloc's cached freed
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
//! - **Guess first** (units and landmark candidates, `GUESS_FIRST`): with no good fit, the caller's
//!   guess scaled by the target's piece comes before the step's prior, so one dense unit doesn't set
//!   every other's need.
//! - **Prior**: the step's targets' measures' 90th percentile (memory: their floors among them), so
//!   one outlier doesn't hold every other target to its figure; a heavy target's own floor holds it.
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
/// The steps whose caller's guess scales with the target (a unit's piece; candidates read the
/// same piece) and so comes before the step's prior while no fit is good.
pub const GUESS_FIRST: [&str; 2] = ["unit", "pois"];

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

/// The models made, kept until the measures they're made from change: the change count they were
/// made at (the owner's: the coordinator bumps its count with each cost, floor or disk floor it
/// takes), by (step, measure).
#[derive(Debug, Default)]
pub struct Cache(Mutex<(u64, BTreeMap<(String, Measure), Model>)>);

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
        .map(|c| c.sizes.iter().filter(|(k, _)| *k != "threads").map(|(k, v)| (k.clone(), *v as f64)).collect())
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

/// The 90th percentile (nearest rank); None with none.
pub fn p90(mut v: Vec<u64>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[(v.len() * 9).div_ceil(10).saturating_sub(1)])
}

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
        for (k, c) in self.costs.iter().filter(|(k, c)| step_of_key(k) == step && c.v >= v) {
            let y = match m {
                Measure::Mem => c.peak_mb,
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

    /// The step's prior of `m` (`p90` of its targets' measures the way it runs now; memory: their
    /// floors among them; disk: its disk floors).
    pub fn prior(&self, step: &str, m: Measure) -> Option<u64> {
        self.model(step, m).prior
    }

    fn prior_of(&self, step: &str, m: Measure) -> Option<u64> {
        let v = cost_version(step);
        let of = |k: &String| step_of_key(k) == step;
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
            Measure::Secs => self.costs.iter().filter(|(k, c)| of(k) && c.v >= v).map(|(_, c)| c.secs).collect(),
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
    /// step's prior; None when none of its step was measured.
    pub fn disk(&self, step: &str, target: &str, threads: u32) -> Option<u64> {
        let key = cost_key(step, target);
        if let Some(&d) = self.disks.and_then(|d| d.get(&key)) {
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

    /// The cores a job of `step` for `target` takes, as measured (the threads its run was given,
    /// `sizes`' `threads`, crate::costin): its own run's, else the most of its step's runs the way
    /// it runs now; None when none of them says (what packs jobs beside each other, crate::coord's
    /// packing, then packs none beside it).
    pub fn cores(&self, step: &str, target: &str) -> Option<u32> {
        let v = cost_version(step);
        let threads = |c: &Cost| c.sizes.get("threads").filter(|_| c.v >= v).map(|&t| t as u32);
        if let Some(t) = self.costs.get(&cost_key(step, target)).and_then(threads) {
            return Some(t);
        }
        self.costs.iter().filter(|(k, _)| step_of_key(k) == step).filter_map(|(_, c)| threads(c)).max()
    }

    /// What a job of `step` for `target` needs, given `threads` (0: not said) and `guess`, the memory
    /// (MB) its caller would take without a measure (the steps table's first guess, what its plan
    /// offered it with, a unit's piece-size guess).
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
/// don't fit (they take their prior, a unit or candidates their piece's guess first).
pub fn fit_table(book: &Book) -> String {
    let mut steps: Vec<&str> = book.costs.keys().map(|k| step_of_key(k)).collect();
    steps.sort();
    steps.dedup();
    let mut out = format!(
        "{:<14} {:<7} {:>5}  {:<10} {:>7} {:>7} {:>8}  fit\n",
        "step", "measure", "runs", "size", "miss", "error", "p90 err"
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
    out += &format!("\nheld out: {FOLDS} folds; poor past {:.0}% missed or {:.0}% median error, or under {MIN_RUNS} runs; a fit's prediction is its envelope × {MARGIN}; capped at its runs' most; p90 err: the step's prior (its runs' 90th percentile) taken as each run's (median)\n", MAX_MISS * 100.0, MAX_ERR * 100.0);
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

    /// A job's cores: its target's run's threads, else the most of its step's; None when no run says.
    #[test]
    fn cores_are_the_targets_run_else_the_most_of_its_steps() {
        let now = |step: &str, c: Cost| Cost { v: cost_version(step), ..c };
        let mut costs = BTreeMap::new();
        costs.insert(cost_key("slope", "6/1/1"), now("slope", cost(100, 5, &[("threads", 4)])));
        costs.insert(cost_key("slope", "6/1/2"), now("slope", cost(100, 5, &[("threads", 2)])));
        costs.insert(cost_key("slope", "6/1/3"), cost(100, 5, &[("threads", 8)]));
        costs.insert(cost_key("lo", "3/1/1"), now("lo", cost(100, 5, &[])));
        let b = Book::new(&costs);
        assert_eq!(b.cores("slope", "6/1/2"), Some(2));
        assert_eq!(b.cores("slope", "6/1/9"), Some(4), "an older way's run counts for nothing");
        assert_eq!(b.cores("lo", "3/1/1"), None);
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

    /// One dense unit measured doesn't set the others': with no good fit, a unit's piece-size guess
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
                Cost { v: 1, ..cost(200 + i * 10 + t * 100, 5, &[("tiffs", i), ("threads", t)]) },
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
