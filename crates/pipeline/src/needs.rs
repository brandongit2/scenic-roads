//! What a job of a step needs (task #236): one model per step, read by every placement path (the
//! coordinator's picks, its floors, the forecast). Nothing here is a figure kept by hand: a need is
//! measured (the target's own run, its floor), fitted over the step's measured runs, the step's
//! measured largest, or the caller's first guess (the steps table's, or the size its plan offered).
//!
//! - **Measured**: the target's own run the way its step runs now (crate::coord::cost_version), never
//!   below its floor (crate::agent::memguard::Floor); a floor learned the way the step runs now
//!   counts as its own measure.
//! - **Fit**: base + k × size (+ c × threads when the runs differ in threads), fitted per step over
//!   its measured runs by least squares and raised to cover every one of them (an upper envelope),
//!   times `MARGIN`, never below the target's floor. A size is one of the run's input measures
//!   (`sizes`, crate::costin) or the target tile's area (km², from its key). Each size is tried and
//!   judged on runs held out of its fit (`FOLDS`-fold): one that misses more than `MAX_MISS` of
//!   them, or is off by more than `MAX_ERR` on the median one, is poor and not used. The best of
//!   the good ones is used, when the target has that size (its own earlier run's, or its area).
//! - **Step max**: the most any of the step's targets measured (memory: their floors too).
//! - **Table**: the caller's guess.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::agent::memguard::{Floor, step_of_key};
use crate::coord::{Cost, cost_key, cost_version};

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

/// Where a need came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    Measured,
    Fit,
    StepMax,
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
        ((self.base + self.k * x + self.per_thread.zip(threads).map_or(0.0, |(c, t)| c * t)) * MARGIN).max(0.0)
    }
}

/// The measures needs are read from: what each target cost (crate::coord::Cost), and, where kept,
/// the floors (crate::agent::memguard) and disk floors (MB, by cost key).
#[derive(Clone, Copy)]
pub struct Book<'a> {
    pub costs: &'a BTreeMap<String, Cost>,
    pub floors: Option<&'a BTreeMap<String, Floor>>,
    pub disks: Option<&'a BTreeMap<String, u64>>,
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
    let mut m: BTreeMap<String, f64> = c.map(|c| c.sizes.iter().filter(|(k, _)| *k != "threads").map(|(k, v)| (k.clone(), *v as f64)).collect()).unwrap_or_default();
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
    let (base, k, per_thread) = fit_on(&have, size)?;
    let (mut errs, mut miss) = (Vec::new(), 0usize);
    for f in 0..FOLDS {
        let train: Vec<&Run> = have.iter().enumerate().filter(|(i, _)| i % FOLDS != f).map(|(_, r)| *r).collect();
        let Some((b, k, c)) = fit_on(&train, size) else { continue };
        let fit = Fit { size: size.into(), base: b, k, per_thread: c, runs: 0, miss: 0.0, err: 0.0 };
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
    Some(Fit { size: size.into(), base, k, per_thread, runs: have.len(), miss: miss as f64 / errs.len() as f64, err: errs[errs.len() / 2] })
}

/// The fits kept, by (step, measure), with what they were made from (its runs' count and sum).
type Kept = BTreeMap<(String, Measure), ((usize, u64), Vec<Fit>)>;
static KEPT: Mutex<Kept> = Mutex::new(BTreeMap::new());

impl<'a> Book<'a> {
    pub fn new(costs: &'a BTreeMap<String, Cost>) -> Self {
        Book { costs, floors: None, disks: None }
    }

    fn floor(&self, key: &str) -> Option<&Floor> {
        self.floors.and_then(|f| f.get(key))
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
            out.push(Run { sizes: sizes_of(k, Some(c)), threads: c.sizes.get("threads").map(|&t| t as f64), y: y as f64 });
        }
        out
    }

    /// The step's fits of `m`, best first (the good ones, then the poor), one per size its runs have.
    pub fn fits(&self, step: &str, m: Measure) -> Vec<Fit> {
        let runs = self.runs(step, m);
        let mark = (runs.len(), runs.iter().map(|r| r.y as u64 ^ r.sizes.values().map(|&x| x as u64).sum::<u64>().rotate_left(17)).fold(0u64, |a, h| a.wrapping_mul(31).wrapping_add(h)));
        let key = (step.to_string(), m);
        if let Some((_, f)) = KEPT.lock().unwrap_or_else(|e| e.into_inner()).get(&key).filter(|(at, _)| *at == mark) {
            return f.clone();
        }
        let mut names: Vec<&String> = runs.iter().flat_map(|r| r.sizes.keys()).collect();
        names.sort();
        names.dedup();
        let mut fits: Vec<Fit> = names.into_iter().filter_map(|s| judge(&runs, s)).collect();
        fits.sort_by(|a, b| b.good().cmp(&a.good()).then(a.err.total_cmp(&b.err)).then(a.size.cmp(&b.size)));
        KEPT.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (mark, fits.clone()));
        fits
    }

    /// The most the step's targets measured of `m` (memory: their floors the way it runs now too).
    pub fn step_max(&self, step: &str, m: Measure) -> Option<u64> {
        let v = cost_version(step);
        let floors = self.floors.into_iter().flatten().filter(|(k, f)| m == Measure::Mem && step_of_key(k) == step && f.v >= v).map(|(_, f)| f.mb);
        let disks = self.disks.into_iter().flatten().filter(|(k, _)| m == Measure::Disk && step_of_key(k) == step).map(|(_, d)| *d);
        let costs = self.costs.iter().filter(|(k, c)| m != Measure::Disk && step_of_key(k) == step && c.v >= v).map(|(_, c)| if m == Measure::Mem { c.peak_mb } else { c.secs });
        costs.chain(floors).chain(disks).max()
    }

    /// The fitted `m` of `target` of `step` (None: no good fit over a size it has).
    fn fitted(&self, step: &str, target: &str, m: Measure, threads: u32) -> Option<u64> {
        let key = cost_key(step, target);
        // (Its sizes: as its own earlier run measured them, else its area.)
        let sizes = sizes_of(&key, self.costs.get(&key));
        let t = (threads > 0).then_some(threads as f64);
        self.fits(step, m).into_iter().filter(Fit::good).find_map(|f| sizes.get(&f.size).map(|&x| f.at(x, t).ceil() as u64))
    }

    /// What a job of `step` for `target` needs, given `threads` (0: not said) and `guess`, the memory
    /// (MB) to take when nothing of its step was measured (the steps table's first guess, or what
    /// its plan offered it with).
    pub fn need(&self, step: &str, target: &str, threads: u32, guess: u64) -> Need {
        let key = cost_key(step, target);
        let v = cost_version(step);
        let floor_any = self.floor(&key).map_or(0, |f| f.mb);
        let own = self.costs.get(&key).filter(|c| c.v >= v);
        let own_floor = self.floor(&key).filter(|f| f.v >= v).map(|f| f.mb);
        let (mem, mem_from) = match (own.map(|c| c.peak_mb), own_floor) {
            (Some(p), _) => (p, From::Measured),
            (None, Some(f)) => (f, From::Measured),
            (None, None) => match (self.fitted(step, target, Measure::Mem, threads), self.step_max(step, Measure::Mem)) {
                (Some(p), _) => (p, From::Fit),
                (None, Some(p)) => (p, From::StepMax),
                (None, None) => (guess, From::Table),
            },
        };
        let disk = self.disks.and_then(|d| d.get(&key)).copied().or_else(|| self.fitted(step, target, Measure::Disk, threads)).or_else(|| self.step_max(step, Measure::Disk));
        let secs = own.map(|c| c.secs).or_else(|| self.fitted(step, target, Measure::Secs, threads)).or_else(|| self.step_max(step, Measure::Secs));
        let io = match crate::agent::steps::row(step).map(|r| r.kind) {
            Some(crate::agent::steps::Kind::Fetch) => Io::Net,
            _ => Io::Nas,
        };
        Need { mem_mb: mem.max(floor_any), mem_from, disk_mb: disk, secs, cores: threads, io }
    }
}

/// `scenic pool needs --fit`: per step and measure, its fits' held-out errors, and the steps that
/// don't fit (they take their measured largest).
pub fn fit_table(book: &Book) -> String {
    let mut steps: Vec<&str> = book.costs.keys().map(|k| step_of_key(k)).collect();
    steps.sort();
    steps.dedup();
    let mut out = format!("{:<14} {:<7} {:>5}  {:<10} {:>7} {:>7} {:>8}  fit\n", "step", "measure", "runs", "size", "miss", "error", "max err");
    let mut poor: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for s in &steps {
        for m in Measure::ALL {
            let runs = book.runs(s, m);
            if runs.is_empty() {
                continue;
            }
            // (The step's largest as each run's prediction: what a poor fit falls back to.)
            let top = runs.iter().map(|r| r.y).fold(0.0, f64::max);
            let mut e: Vec<f64> = runs.iter().map(|r| (top - r.y) / r.y.max(1.0)).collect();
            e.sort_by(f64::total_cmp);
            let max_err = format!("{:.1}%", e[e.len() / 2] * 100.0);
            let fits = book.fits(s, m);
            let n = runs.len();
            match fits.first() {
                Some(f) => {
                    let t = f.per_thread.map_or(String::new(), |c| format!(" + {c:.1}×threads"));
                    out += &format!("{:<14} {:<7} {:>5}  {:<10} {:>6.1}% {:>6.1}% {:>8}  {}{:.0} + {:.4}×{}{t}\n", s, m.name(), n, f.size, f.miss * 100.0, f.err * 100.0, max_err, if f.good() { "" } else { "poor: " }, f.base, f.k, f.size);
                }
                None => out += &format!("{:<14} {:<7} {:>5}  {:<10} {:>7} {:>7} {:>8}  poor: no size to fit on\n", s, m.name(), n, "-", "-", "-", max_err),
            }
            if !fits.first().is_some_and(Fit::good) {
                poor.entry(s).or_default().push(m.name());
            }
        }
    }
    out += &format!("\nheld out: {FOLDS} folds; poor past {:.0}% missed or {:.0}% median error, or under {MIN_RUNS} runs; a fit's prediction is its envelope × {MARGIN}; max err: the step's largest taken as each run's (median)\n", MAX_MISS * 100.0, MAX_ERR * 100.0);
    out += "steps that don't fit (they take their measured largest):\n";
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
        let sizes = v["sizes"].as_object().map(|m| m.iter().filter_map(|(k, n)| Some((k.clone(), n.as_u64()?))).collect()).unwrap_or_default();
        if let Some(d) = v["disk_mb"].as_u64().filter(|&d| d > 0) {
            disks.insert(k.to_string(), d);
        }
        costs.insert(k.to_string(), Cost { peak_mb: peak, secs: v["secs"].as_u64().unwrap_or(0), worker: None, v: v["v"].as_u64().unwrap_or(0) as u32, sizes });
    }
    Ok((costs, disks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(peak: u64, secs: u64, sizes: &[(&str, u64)]) -> Cost {
        Cost { peak_mb: peak, secs, worker: None, v: 0, sizes: sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    /// Memory that goes with a size fits, and a target never run is predicted over its own size.
    #[test]
    fn a_linear_step_fits_and_predicts_over_a_targets_size() {
        let mut costs = BTreeMap::new();
        for i in 1..=30u64 {
            costs.insert(format!("bldprep 6/{i}/1"), cost(500 + 2 * i * 100, 10 * i, &[("buildings", i * 100)]));
        }
        let b = Book::new(&costs);
        let fits = b.fits("bldprep", Measure::Mem);
        let f = &fits[0];
        assert_eq!(f.size, "buildings");
        assert!(f.good() && f.miss == 0.0, "{f:?}");
        // Measured: its own.
        assert_eq!(b.need("bldprep", "6/3/1", 0, 1).mem_from, From::Measured);
    }

    /// A step whose memory has nothing to do with its sizes is poor and takes its largest; one never
    /// measured takes the guess; a floor is never gone under.
    #[test]
    fn a_poor_fit_takes_the_step_max_and_none_the_guess() {
        let mut costs = BTreeMap::new();
        for i in 1..=30u64 {
            costs.insert(format!("peaks 6/{i}/1"), cost(if i % 3 == 0 { 9000 } else { 300 }, 5, &[("bytes", 1000)]));
        }
        let floors = BTreeMap::from([("peaks 6/50/1".to_string(), Floor { mb: 12_000, alone: true, v: 0 })]);
        let b = Book { costs: &costs, floors: Some(&floors), disks: None };
        assert!(!b.fits("peaks", Measure::Mem).iter().any(Fit::good));
        let n = b.need("peaks", "6/40/1", 0, 1);
        assert_eq!((n.mem_mb, n.mem_from), (12_000, From::StepMax), "a floor of the way it runs now is a measure too");
        let n = b.need("pois", "6/40/1", 0, 777);
        assert_eq!((n.mem_mb, n.mem_from, n.secs), (777, From::Table, None));
        assert_eq!(b.need("peaks", "6/50/1", 0, 1).mem_mb, 12_000);
    }

    /// Threads are an input when the runs differ in them.
    #[test]
    fn threads_are_fitted_when_they_differ() {
        let mut costs = BTreeMap::new();
        for i in 1..=40u64 {
            let t = 4 + (i % 3) * 4;
            costs.insert(format!("terrain-lo 3/{i}/1"), Cost { v: 1, ..cost(200 + i * 10 + t * 100, 5, &[("tiffs", i), ("threads", t)]) });
        }
        let b = Book::new(&costs);
        let f = b.fits("terrain-lo", Measure::Mem).into_iter().find(|f| f.size == "tiffs").unwrap();
        assert!(f.per_thread.is_some_and(|c| (c - 100.0).abs() < 1.0), "{f:?}");
        assert!(f.good());
    }

    #[test]
    fn tiles_of_keys() {
        assert_eq!(tile("tail 6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("6/3/4"), Some((6, 3, 4)));
        assert_eq!(tile("marks"), None);
    }
}
