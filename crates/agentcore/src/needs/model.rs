//! A step's needs learned from its runs (task #1416; docs/pool.md §7.7, What a step needs): what a
//! target needs apart from what its Mac supplied, by the size of its input, so a prediction is its
//! needs over a Mac's supply and crowding is an input, not noise in the history.
//!
//! A run (`Obs`) is a target's cost line (crate::coord::Cost) the way its step runs now, whose job's
//! record, where the lead's timings log still holds it, passed its plausibility checks and ended
//! well (jobrt::timings::JobFacts::clean, task #1419). Read from it, by each of its input sizes
//! (`PerUnit`, a unit of `size`; "target" one a target):
//!
//! - **work per unit**: the CPU it took, in the lead's CPU seconds (its instructions over the
//!   lead's speed where its record counted them, else its CPU on its Mac at the lead's speed by the
//!   two Macs' speeds, task #1418: crate::paces), over its size; a target's the `work_pct`
//!   percentile of its step's runs of about its size (`about`; the median of the fastest twice that
//!   share): crowding only adds time, and one freak can't set it. The rest of its time per unit
//!   (`Obs::rest`: its waits, and what its CPU doesn't count) likewise.
//! - **parallelism** (`Par`): a run that kept every thread busy while not waiting is a floor, at
//!   least its threads; one that didn't, what it kept busy; its waits left out.
//! - **memory**: the `high_pct` percentile of the peaks of its runs of about the target's size
//!   (half to twice it; fewer than `MIN_RUNS` so, the nearest larger: never a smaller one) and its
//!   threads or more (where enough ran so), plus its memory per thread past the threads they ran
//!   (least squares over size and threads): a small or fast run can't lower it. Disk alike, by size
//!   alone.
//! - **NAS** (`Moves`): the bytes it asked of the NAS and its NAS calls per unit, the `nas_pct`
//!   percentile.
//!
//! The size a target is predicted by is the first of the step's sizes, best first (`by`: the least
//! spread of its runs' work per unit), that the target has (its own earlier run's sizes, its tile's
//! area, the measure its plan offered it by, its source's time: crate::needs::SOURCE; "target"
//! always) and a run of about its size has (`PerUnit::ran_about`).
//!
//! **Prediction against what came** (`Off`): each run, in the order they came, against what the
//! runs before it predicted: off by more than `off` times on its work, or on its time past what its
//! supply explains (JobFacts::explain), it's flagged; `relearn` runs in a row off on one side of
//! their work and the step learns from the first of them on, as a cost version's bump does for code.

use std::collections::BTreeMap;

use jobrt::needs::Tuning;

/// The fewest runs a size, a check or an envelope is read from.
pub const MIN_RUNS: usize = super::MIN_RUNS;

/// The size every target has: one a target.
pub const TARGET: &str = "target";

/// What a run moved on the NAS (or a step's per unit of a size): the bytes it asked of it (read,
/// whether the NAS or this Mac's page cache answered, written and read back) and its calls.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moves {
    pub bytes: f64,
    pub creates: f64,
    pub listed: f64,
    pub stats: f64,
    pub deletes: f64,
}

impl Moves {
    fn scaled(self, k: f64) -> Moves {
        Moves {
            bytes: self.bytes * k,
            creates: self.creates * k,
            listed: self.listed * k,
            stats: self.stats * k,
            deletes: self.deletes * k,
        }
    }
}

/// A disk measure (crate::needs::Book's `disks`): its target's sizes, `TARGET` among them, and
/// what it took on the disk (MB).
pub type Disk = (BTreeMap<String, f64>, u64);

/// A run as the model reads it (crate::needs::Book makes them).
#[derive(Clone, Debug, Default)]
pub struct Obs {
    /// The order runs came in (its job's id's lease number: when it was granted); its cost key.
    pub order: u64,
    pub key: String,
    /// Its sizes (crate::needs' `sizes_of`), `TARGET` among them.
    pub sizes: BTreeMap<String, f64>,
    /// The threads it ran on (0: not said).
    pub threads: u32,
    /// Its CPU in the lead's CPU seconds (None: not said).
    pub work: Option<f64>,
    /// Its time waiting on the NAS and the network (s), and its wall time at the lead's pace (s).
    pub wait: f64,
    pub wall: f64,
    /// Its peak memory (MB).
    pub peak: u64,
    pub nas: Option<Moves>,
    /// How much slower its supply explains it ran (1 at least; jobrt::timings::JobFacts::explain).
    pub explain: f64,
    /// Whether it counts in the step's time (a `PER_WORK` step's run that did its work).
    pub timed: bool,
}

impl Obs {
    /// The cores it kept busy while not waiting.
    fn busy(&self) -> Option<f64> {
        let w = self.work?;
        let ran = (self.wall - self.wait).max(1.0);
        Some(w / ran)
    }

    /// Its time apart from its work over the cores it could use (its threads, at most `cores`, the
    /// step's parallelism): its waits, and what its CPU doesn't count (a serial stretch, a wait no
    /// phase timed); its waits alone where it doesn't say its CPU (its wall time per unit stands for
    /// the rest).
    fn rest(&self, cores: f64) -> f64 {
        let Some(w) = self.work else { return self.wait };
        let c = if self.threads > 0 { f64::from(self.threads).min(cores) } else { cores };
        (self.wall - w / c.max(1.0)).max(0.0)
    }
}

/// A step's parallelism: the most threads a run kept busy while not waiting (a floor: it used every
/// thread it had), and the median of what the runs that didn't keep busy (what it could use).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Par {
    pub floor: u32,
    pub busy: Option<f64>,
    /// The `high_pct` percentile of the cores its runs kept busy over their whole time, waits and
    /// all: what a job counts for in its Mac's packing.
    pub kept: Option<f64>,
    pub runs: usize,
}

impl Par {
    /// The cores a job of the step can use: its floor, or what its runs that had more threads kept
    /// busy, the more; one at least.
    pub fn cores(&self) -> f64 {
        self.busy.unwrap_or(0.0).max(f64::from(self.floor)).max(1.0)
    }
}

/// A step's needs per unit of one of its sizes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerUnit {
    pub size: String,
    /// Its runs having the size, and the spread of their work per unit (the standard deviation of
    /// its log: what ranks the sizes).
    pub runs: usize,
    pub spread: f64,
    /// Its times per unit over all its runs (`Times`), and each run's (`Sample`) by size: a
    /// target's are its runs' of about its size (`PerUnit::times`).
    pub whole: Times,
    time: Vec<Sample>,
    /// Its latest earlier way's times per unit, standing in where larger while the way it runs now
    /// has few runs (`Model::of`).
    stand_in: Option<Times>,
    pub nas: Option<Moves>,
    /// Its runs' (size, threads, peak MB) by size, and its memory per thread (MB).
    mem: Vec<(f64, u32, u64)>,
    pub mem_per_thread: f64,
    /// Its disk measures' (size, disk MB) by size.
    disk: Vec<(f64, u64)>,
    work_pct: f64,
    high_pct: f64,
}

/// A run's times per unit of a size: its size, its work (the lead's CPU s), the rest of its time
/// (s: `Obs::rest`) and, where it doesn't say its CPU, its wall time (s at the lead's pace).
type Sample = (f64, Option<f64>, f64, Option<f64>);

/// Times per unit (`Sample`'s) at `work_pct`, and at `high_pct` (an update's drain).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Times {
    pub work: Option<f64>,
    pub work_high: Option<f64>,
    pub rest: f64,
    pub rest_high: f64,
    pub wall: Option<f64>,
    pub wall_high: Option<f64>,
}

impl Times {
    fn of(v: &[Sample], work_pct: f64, high_pct: f64) -> Times {
        let work = sorted(v.iter().filter_map(|s| s.1).collect());
        let rest = sorted(v.iter().map(|s| s.2).collect());
        let wall = sorted(v.iter().filter_map(|s| s.3).collect());
        Times {
            work: pct(&work, work_pct),
            work_high: pct(&work, high_pct),
            rest: pct(&rest, work_pct).unwrap_or(0.0),
            rest_high: pct(&rest, high_pct).unwrap_or(0.0),
            wall: pct(&wall, work_pct),
            wall_high: pct(&wall, high_pct),
        }
    }

    /// Each the larger of the two's.
    fn up(self, o: Times) -> Times {
        let up = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        Times {
            work: up(self.work, o.work),
            work_high: up(self.work_high, o.work_high),
            rest: self.rest.max(o.rest),
            rest_high: self.rest_high.max(o.rest_high),
            wall: up(self.wall, o.wall),
            wall_high: up(self.wall_high, o.wall_high),
        }
    }
}

/// A run flagged off its prediction: its cost key, which of its needs, and what it took over its
/// prediction.
#[derive(Clone, Debug, PartialEq)]
pub struct Off {
    pub key: String,
    pub what: &'static str,
    pub ratio: f64,
}

/// A step's needs (`Model::of`).
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub runs: usize,
    /// Best first; `TARGET` among them while it has runs.
    pub by: Vec<PerUnit>,
    pub par: Par,
    /// The runs flagged off their predictions, oldest first.
    pub off: Vec<Off>,
    /// The run its learning starts from, where it relearned (`relearn` off in a row).
    pub since: Option<String>,
}

/// The `p` percentile of the sorted `v` (linear between ranks); None with none.
pub fn pct(v: &[f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let i = (v.len() - 1) as f64 * (p / 100.0).clamp(0.0, 1.0);
    let (lo, hi) = (i.floor() as usize, i.ceil() as usize);
    Some(v[lo] + (v[hi] - v[lo]) * (i - lo as f64))
}

/// `v` sorted, for `pct`.
pub fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v
}

/// The coefficient of t in the least squares of y over [1, x, t] (over [1, t] where the sizes x
/// are all one), solved by its normal equations (None: singular).
fn per_thread(rows: &[(f64, f64, f64)]) -> Option<f64> {
    let one_size = rows.iter().all(|r| r.0 == rows[0].0);
    let n = if one_size { 2 } else { 3 };
    let mut a = [[0.0f64; 4]; 3];
    for &(x, t, y) in rows {
        let f = if one_size { [1.0, t, 0.0] } else { [1.0, x, t] };
        for i in 0..n {
            for j in 0..n {
                a[i][j] += f[i] * f[j];
            }
            a[i][3] += f[i] * y;
        }
    }
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
    Some(a[n - 1][3] / a[n - 1][n - 1])
}

/// The runs of about `u` units among `v` (sorted by their size, `size`): those of half to twice
/// it, else (fewer than `MIN_RUNS`) the `MIN_RUNS` nearest of `u / 2` or more, else the largest
/// `MIN_RUNS`: never one under half of it while others are there.
fn about<T>(v: &[T], u: f64, size: impl Fn(&T) -> f64) -> &[T] {
    let from = v.partition_point(|m| size(m) < u / 2.0);
    let to = v.partition_point(|m| size(m) <= 2.0 * u);
    if to - from >= MIN_RUNS {
        return &v[from..to];
    }
    let from = from.min(v.len().saturating_sub(MIN_RUNS));
    &v[from..(from + MIN_RUNS).min(v.len())]
}

impl PerUnit {
    /// Of `runs` and the disk measures `disks` by `size`, at the tuning's percentiles, the step's
    /// parallelism `cores`.
    fn of(runs: &[&Obs], disks: &[Disk], size: &str, t: &Tuning, cores: f64) -> PerUnit {
        let have: Vec<(&Obs, f64)> =
            runs.iter().filter_map(|o| Some((*o, *o.sizes.get(size)?))).filter(|(_, u)| *u > 0.0).collect();
        let timed: Vec<&(&Obs, f64)> = have.iter().filter(|(o, _)| o.timed).collect();
        let mut time: Vec<Sample> = timed
            .iter()
            .map(|(o, u)| (*u, o.work.map(|w| w / u), o.rest(cores) / u, o.work.is_none().then(|| o.wall / u)))
            .collect();
        time.sort_by(|a, b| a.0.total_cmp(&b.0));
        // (The spread of its work per unit, or its wall time's where it doesn't say its CPU.)
        let logs: Vec<f64> = time.iter().filter_map(|s| s.1.or(s.3).filter(|&w| w > 0.0).map(det::Det::dln)).collect();
        let spread = if logs.len() < 2 {
            f64::INFINITY
        } else {
            let m = logs.iter().sum::<f64>() / logs.len() as f64;
            (logs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / logs.len() as f64).sqrt()
        };
        let nas_of: Vec<(Moves, f64)> = have.iter().filter_map(|(o, u)| Some((o.nas?, *u))).collect();
        let nas = (!nas_of.is_empty()).then(|| {
            let p = |f: fn(&Moves) -> f64| {
                pct(&sorted(nas_of.iter().map(|(m, u)| f(m) / u).collect()), t.nas_pct).unwrap_or(0.0)
            };
            Moves {
                bytes: p(|m| m.bytes),
                creates: p(|m| m.creates),
                listed: p(|m| m.listed),
                stats: p(|m| m.stats),
                deletes: p(|m| m.deletes),
            }
        });
        let mut mem: Vec<(f64, u32, u64)> =
            have.iter().filter(|(o, _)| o.peak > 0).map(|(o, u)| (*u, o.threads, o.peak)).collect();
        mem.sort_by(|a, b| a.0.total_cmp(&b.0));
        // (Its memory per thread: least squares over size and threads, where its runs' threads differ.)
        let differ = mem.iter().any(|m| m.1 != mem[0].1);
        let rows: Vec<(f64, f64, f64)> = mem.iter().map(|m| (m.0, f64::from(m.1), m.2 as f64)).collect();
        let mem_per_thread = if differ { per_thread(&rows).map_or(0.0, |c| c.max(0.0)) } else { 0.0 };
        let mut disk: Vec<(f64, u64)> =
            disks.iter().filter_map(|(s, d)| Some((*s.get(size)?, *d))).filter(|(u, _)| *u > 0.0).collect();
        disk.sort_by(|a, b| a.0.total_cmp(&b.0));
        PerUnit {
            size: size.to_string(),
            runs: have.len(),
            spread,
            whole: Times::of(&time, t.work_pct, t.high_pct),
            time,
            stand_in: None,
            nas,
            mem,
            mem_per_thread,
            disk,
            work_pct: t.work_pct,
            high_pct: t.high_pct,
        }
    }

    /// Whether a run of half to twice `u` units is among its runs (timed, or measured for memory).
    fn ran_about(&self, u: f64) -> bool {
        let t = &self.time;
        let m = &self.mem;
        t.partition_point(|s| s.0 < u / 2.0) < t.partition_point(|s| s.0 <= 2.0 * u)
            || m.partition_point(|r| r.0 < u / 2.0) < m.partition_point(|r| r.0 <= 2.0 * u)
    }

    /// A target of `u` units' times per unit: its runs' of about `u` units (`about`; work per unit
    /// isn't one figure for every size: a slope-deep target whose terrain-deep took a second is
    /// empty, one whose took a minute full), its earlier way's standing in where larger.
    pub fn times(&self, u: f64) -> Times {
        let local = Times::of(about(&self.time, u, |s| s.0), self.work_pct, self.high_pct);
        self.stand_in.map_or(local, |o| local.up(o))
    }

    /// The memory (MB) a target of `u` units needs on `threads` (0: not said): the high
    /// percentile of the peaks of the runs of about `u` units (`about`; of `threads` or more, where
    /// `MIN_RUNS` of them ran so), plus its memory per thread for each thread past the most they ran
    /// on. None with no run.
    pub fn mem(&self, u: f64, threads: u32) -> Option<u64> {
        let big = about(&self.mem, u, |m| m.0);
        let wide: Vec<&(f64, u32, u64)> = big.iter().filter(|m| m.1 >= threads).collect();
        let these: Vec<&(f64, u32, u64)> =
            if threads > 0 && wide.len() >= MIN_RUNS { wide } else { big.iter().collect() };
        let peaks = sorted(these.iter().map(|m| m.2 as f64).collect());
        let p = pct(&peaks, self.high_pct)?;
        let most = these.iter().map(|m| m.1).max().unwrap_or(0);
        let more = f64::from(threads.saturating_sub(most)) * self.mem_per_thread;
        Some((p + more).ceil() as u64)
    }

    /// The disk (MB) a target of `u` units needs: the high percentile of the measures' of about
    /// `u` units (`about`). None with none.
    pub fn disk(&self, u: f64) -> Option<u64> {
        let near = about(&self.disk, u, |d| d.0);
        pct(&sorted(near.iter().map(|d| d.1 as f64).collect()), self.high_pct).map(|d| d.ceil() as u64)
    }
}

impl Model {
    /// The model of `runs` (any order) and the disk measures `disks`, at `t`; `older`: the runs of
    /// the step's latest earlier way,
    /// whose work and rest per unit stand in where larger while the way it runs now has fewer than
    /// `PRIOR_RUNS` (its first runs to end are its quick ones).
    pub fn of(mut runs: Vec<Obs>, older: &[Obs], disks: &[Disk], t: &Tuning) -> Model {
        runs.sort_by(|a, b| a.order.cmp(&b.order).then(a.key.cmp(&b.key)));
        runs.drain(..runs.len().saturating_sub(t.runs.max(1) as usize));
        let (off, since) = check(&runs, t);
        let from = since.map_or(0, |i| i);
        let since = since.map(|i| runs[i].key.clone());
        let runs: Vec<&Obs> = runs[from..].iter().collect();
        let par = par(&runs, t);
        let mut by = rank(&runs, disks, t, par.cores());
        if runs.len() < super::PRIOR_RUNS && !older.is_empty() {
            let older: Vec<&Obs> = older.iter().collect();
            let cores = self::par(&older, t).cores();
            for p in &mut by {
                p.stand_in = Some(PerUnit::of(&older, &[], &p.size, t, cores).whole);
            }
            // (A size only the earlier way's runs rank: their work and rest, behind the way's own;
            // their memory, disk and NAS not: an earlier way's measures size nothing of those.)
            for mut o in rank(&older, &[], t, cores) {
                if !by.iter().any(|p| p.size == o.size) {
                    (o.mem, o.mem_per_thread, o.disk, o.nas) = (Vec::new(), 0.0, Vec::new(), None);
                    by.push(o);
                }
            }
        }
        Model { runs: runs.len(), par, by, off, since }
    }

    /// The first of its sizes (best first) that `sizes` has, with a run of about as many units of
    /// it (`PerUnit::ran_about`: past its runs, a size says nothing), and how many units of it.
    pub fn unit(&self, sizes: &BTreeMap<String, f64>) -> Option<(&PerUnit, f64)> {
        self.by.iter().find_map(|p| {
            let u = sizes.get(&p.size).copied().filter(|&u| u > 0.0 && p.ran_about(u))?;
            Some((p, u))
        })
    }

    /// The time (s) a target of `sizes` takes on `threads` (0: as many as it can use) on a Mac
    /// `ratio` times the lead's time: its work there over the cores it can use, plus the rest of its
    /// time (`Obs::rest`); at the high percentile when `high`. None: no run says its time.
    pub fn secs(&self, sizes: &BTreeMap<String, f64>, threads: u32, ratio: f64, high: bool) -> Option<f64> {
        let (p, u) = self.unit(sizes)?;
        let tm = p.times(u);
        let (work, wall, rest) =
            if high { (tm.work_high, tm.wall_high, tm.rest_high) } else { (tm.work, tm.wall, tm.rest) };
        let cores = if threads > 0 { f64::from(threads).min(self.par.cores()) } else { self.par.cores() };
        let compute = match (work, wall) {
            (Some(w), _) => w * u * ratio / cores,
            (None, Some(w)) => w * u * ratio,
            (None, None) => return None,
        };
        Some(compute + rest * u)
    }

    /// The CPU (the lead's CPU s) a target of `sizes` takes.
    pub fn work(&self, sizes: &BTreeMap<String, f64>) -> Option<f64> {
        let (p, u) = self.unit(sizes)?;
        Some(p.times(u).work? * u)
    }

    pub fn mem(&self, sizes: &BTreeMap<String, f64>, threads: u32) -> Option<u64> {
        let (p, u) = self.unit(sizes)?;
        p.mem(u, threads)
    }

    pub fn disk(&self, sizes: &BTreeMap<String, f64>) -> Option<u64> {
        let (p, u) = self.unit(sizes)?;
        p.disk(u)
    }

    /// What a target of `sizes` moves on the NAS.
    pub fn nas(&self, sizes: &BTreeMap<String, f64>) -> Option<Moves> {
        let (p, u) = self.unit(sizes)?;
        Some(p.nas?.scaled(u))
    }
}

/// The sizes of `runs` worth predicting by, best first: those `MIN_RUNS` of its timed runs have,
/// by the spread of their work per unit (`TARGET` whatever its spread).
fn rank(runs: &[&Obs], disks: &[Disk], t: &Tuning, cores: f64) -> Vec<PerUnit> {
    if runs.is_empty() {
        return Vec::new();
    }
    let mut names: Vec<&String> = runs.iter().flat_map(|r| r.sizes.keys()).collect();
    names.sort();
    names.dedup();
    let mut by: Vec<PerUnit> = names
        .into_iter()
        .map(|n| PerUnit::of(runs, disks, n, t, cores))
        .filter(|p| p.size == TARGET || (p.runs >= MIN_RUNS && p.spread.is_finite()))
        .collect();
    by.sort_by(|a, b| a.spread.total_cmp(&b.spread).then(a.size.cmp(&b.size)));
    by
}

/// `runs`' parallelism (`Par`): a run is a floor when the cores it kept busy while not waiting were
/// within `CHECK_SHARE` of its threads.
fn par(runs: &[&Obs], t: &Tuning) -> Par {
    let mut out = Par::default();
    let mut under = Vec::new();
    let mut kept = Vec::new();
    for r in runs.iter().filter(|r| r.timed && r.threads > 0) {
        let Some(b) = r.busy() else { continue };
        out.runs += 1;
        kept.push(r.work.unwrap_or(0.0) / r.wall.max(1.0));
        if b >= f64::from(r.threads) * (1.0 - jobrt::timings::CHECK_SHARE) {
            out.floor = out.floor.max(r.threads);
        } else {
            under.push(b);
        }
    }
    out.busy = pct(&sorted(under), 50.0);
    out.kept = pct(&sorted(kept), t.high_pct);
    out
}

/// The prediction-vs-actual check over `runs` (in the order they came): each against what the runs
/// before it (since the step last relearned) predict by `TARGET`'s and its best size's per-unit
/// work, once `MIN_RUNS` came; the runs flagged, and where its learning starts (`Model`).
fn check(runs: &[Obs], t: &Tuning) -> (Vec<Off>, Option<usize>) {
    let all: Vec<&Obs> = runs.iter().collect();
    let size = rank(&all, &[], t, 1.0).first().map_or(TARGET.to_string(), |p| p.size.clone());
    let (mut off, mut since) = (Vec::new(), None);
    // (Work per unit so far, sorted; cores so far; the flagged in a row, and on which side.)
    let mut per: Vec<f64> = Vec::new();
    let mut waits: Vec<f64> = Vec::new();
    let mut busy: Vec<f64> = Vec::new();
    let mut row: (usize, i8) = (0, 0);
    let mut start = 0usize;
    for (i, r) in runs.iter().enumerate() {
        let (Some(w), Some(&u)) = (r.work.filter(|_| r.timed), r.sizes.get(&size)) else { continue };
        if u <= 0.0 {
            continue;
        }
        let mut side = 0i8;
        if per.len() >= MIN_RUNS {
            let pw = pct(&per, t.work_pct).unwrap_or(0.0) * u;
            let ratio = w / pw.max(1e-3);
            if ratio > t.off || ratio < 1.0 / t.off {
                side = if ratio > 1.0 { 1 } else { -1 };
                off.push(Off { key: r.key.clone(), what: "work", ratio });
            } else {
                let cores = pct(&busy, 50.0).unwrap_or(1.0).max(1.0);
                let cores = if r.threads > 0 { cores.min(f64::from(r.threads)) } else { cores };
                let pt = pw / cores + pct(&waits, t.work_pct).unwrap_or(0.0) * u;
                let ratio = r.wall / pt.max(1.0);
                if ratio > t.off * r.explain.max(1.0) || ratio < 1.0 / t.off {
                    off.push(Off { key: r.key.clone(), what: "time", ratio });
                }
            }
        }
        row = if side != 0 && side == row.1 { (row.0 + 1, side) } else { (usize::from(side != 0), side) };
        if side != 0 && row.0 == 1 {
            start = i;
        }
        if row.0 >= t.relearn as usize {
            // (Relearned: from the first of the row on, as a cost version's bump.)
            since = Some(start);
            per.clear();
            waits.clear();
            busy.clear();
            for q in &runs[start..i] {
                if let (Some(w), Some(&u)) = (q.work.filter(|_| q.timed), q.sizes.get(&size)) {
                    if u > 0.0 {
                        per.push(w / u);
                        waits.push(q.wait / u);
                    }
                }
            }
            per.sort_by(f64::total_cmp);
            waits.sort_by(f64::total_cmp);
            row = (0, 0);
        }
        let at = per.partition_point(|x| *x < w / u);
        per.insert(at, w / u);
        let at = waits.partition_point(|x| *x < r.wait / u);
        waits.insert(at, r.wait / u);
        if let Some(b) = r.busy() {
            let at = busy.partition_point(|x| *x < b);
            busy.insert(at, b);
        }
    }
    (off, since)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(order: u64, units: f64, work: f64, threads: u32, peak: u64) -> Obs {
        Obs {
            order,
            key: format!("s 6/{order}/0"),
            sizes: [(TARGET.to_string(), 1.0), ("km2".to_string(), units)].into(),
            threads,
            work: Some(work),
            wait: 0.0,
            wall: work / f64::from(threads.max(1)),
            peak,
            explain: 1.0,
            timed: true,
            ..Default::default()
        }
    }

    /// Crowding only adds time: a step's work per unit is a low percentile of its runs, the median of
    /// the fastest, so its crowded runs don't move it, nor one freak fast run.
    #[test]
    fn work_per_unit_is_a_low_percentile_one_freak_cant_set() {
        let t = Tuning::default();
        // (20 runs of 10 units at 1 s a unit, 20 crowded at 3 s, one freak at 0.01 s.)
        let mut runs: Vec<Obs> = (0..20).map(|i| obs(i, 10.0, 10.0, 1, 100)).collect();
        runs.extend((20..40).map(|i| obs(i, 10.0, 30.0, 1, 100)));
        runs.push(obs(40, 10.0, 0.1, 1, 100));
        let m = Model::of(runs, &[], &[], &t);
        let sizes = [(TARGET.to_string(), 1.0), ("km2".to_string(), 5.0)].into();
        let w = m.work(&sizes).unwrap();
        assert!((w - 5.0).abs() < 1e-9, "{w}");
    }

    /// A run that kept every thread busy is a floor: at least its threads; one that didn't says what
    /// the step can use, its waits left out.
    #[test]
    fn parallelism_reads_saturated_runs_as_floors_and_leaves_waits_out() {
        let t = Tuning::default();
        let mut runs: Vec<Obs> = (0..10).map(|i| obs(i, 1.0, 40.0, 4, 100)).collect();
        // (At 8 threads, 6 cores busy while computing, half its time waiting.)
        for i in 10..20 {
            let mut o = obs(i, 1.0, 60.0, 8, 100);
            o.wall = 20.0;
            o.wait = 10.0;
            runs.push(o);
        }
        let m = Model::of(runs, &[], &[], &t);
        assert_eq!(m.par.floor, 4);
        assert_eq!(m.par.busy, Some(6.0));
        assert_eq!(m.par.cores(), 6.0);
        let sizes = [(TARGET.to_string(), 1.0)].into();
        // (On 2 threads: its 40 s of work over 2, plus its waits per target's percentile, 0.)
        assert!((m.secs(&sizes, 2, 1.0, false).unwrap() - 20.0).abs() < 1e-9);
        // (A Mac twice the lead's time: twice.)
        assert!((m.secs(&sizes, 2, 2.0, false).unwrap() - 40.0).abs() < 1e-9);
    }

    /// Memory is a high percentile of the runs at the target's size or more: small runs, however
    /// many, can't lower a large target's; more threads add the memory a thread costs.
    #[test]
    fn memory_is_a_high_percentile_small_runs_cant_lower() {
        let t = Tuning::default();
        let mut runs: Vec<Obs> = (0..20).map(|i| obs(i, 100.0, 10.0, 2, 4000 + i)).collect();
        let big = Model::of(runs.clone(), &[], &[], &t);
        runs.extend((20..200).map(|i| obs(i, 1.0, 0.1, 2, 50)));
        let crowded = Model::of(runs.clone(), &[], &[], &t);
        let at = |m: &Model, u: f64, th: u32| m.mem(&[("km2".to_string(), u)].into(), th).unwrap();
        assert_eq!(at(&big, 100.0, 2), at(&crowded, 100.0, 2));
        assert!(at(&crowded, 100.0, 2) >= 4017);
        assert!(at(&crowded, 1.0, 2) < 100);
        // (Memory per thread: runs at 4 threads holding 1 GB more.)
        runs.extend((200..220).map(|i| obs(i, 100.0, 10.0, 4, 5000 + i)));
        let m = Model::of(runs, &[], &[], &t);
        let per = m.by.iter().find(|p| p.size == "km2").unwrap().mem_per_thread;
        assert!(per > 300.0, "{per}");
        assert!(at(&m, 100.0, 8) > at(&m, 100.0, 4));
    }

    /// A run off its prediction by more than `off` is flagged; `relearn` in a row on one side and
    /// the step learns from the first of them on.
    #[test]
    fn runs_off_their_prediction_are_flagged_and_relearned_from() {
        let t = Tuning::default();
        let mut runs: Vec<Obs> = (0..30).map(|i| obs(i, 1.0, 10.0, 1, 100)).collect();
        // (One slow run, then the code changes: every run after takes ten times the work.)
        runs.push(obs(30, 1.0, 50.0, 1, 100));
        runs.push(obs(31, 1.0, 10.0, 1, 100));
        runs.extend((32..32 + t.relearn + 10).map(|i| obs(i, 1.0, 100.0, 1, 100)));
        let m = Model::of(runs, &[], &[], &t);
        assert_eq!(m.off[0].key, "s 6/30/0");
        assert_eq!(m.since.as_deref(), Some("s 6/32/0"));
        assert!((m.work(&[(TARGET.to_string(), 1.0)].into()).unwrap() - 100.0).abs() < 1e-9);
        // (A slow run its supply explains (its Mac's cores all taken) isn't flagged on its time.)
        let mut runs: Vec<Obs> = (0..30).map(|i| obs(i, 1.0, 10.0, 1, 100)).collect();
        let mut slow = obs(30, 1.0, 10.0, 1, 100);
        slow.wall = 50.0;
        slow.explain = 5.0;
        runs.push(slow.clone());
        assert!(Model::of(runs.clone(), &[], &[], &t).off.is_empty());
        slow.explain = 1.0;
        runs.pop();
        runs.push(slow);
        assert_eq!(Model::of(runs, &[], &[], &t).off.len(), 1);
    }

    /// A target's work per unit is its runs' of about its size: small targets here take twice
    /// a large one's work per unit (within what the check flags), and neither sets the other's; past every run's size, its size
    /// says nothing, and the target's own count does.
    #[test]
    fn work_per_unit_is_read_of_runs_of_about_its_size() {
        let t = Tuning::default();
        let mut runs: Vec<Obs> = (0..20).map(|i| obs(i, 1.0, 1.0, 1, 100)).collect();
        runs.extend((20..40).map(|i| obs(i, 100.0, 50.0, 1, 100)));
        let m = Model::of(runs, &[], &[], &t);
        assert_eq!(m.by[0].size, "km2");
        let at = |u: f64| m.work(&[(TARGET.to_string(), 1.0), ("km2".to_string(), u)].into()).unwrap();
        assert!((at(1.0) - 1.0).abs() < 1e-9 && (at(100.0) - 50.0).abs() < 1e-9, "{} {}", at(1.0), at(100.0));
        assert_eq!(m.unit(&[(TARGET.to_string(), 1.0), ("km2".to_string(), 1e6)].into()).unwrap().0.size, TARGET);
    }

    /// A target is predicted by the best of the step's sizes it has: its work spread least.
    #[test]
    fn the_size_spreading_work_least_predicts() {
        let t = Tuning::default();
        let runs: Vec<Obs> = (0..20)
            .map(|i| {
                let mut o = obs(i, 1.0 + i as f64, 2.0 * (1.0 + i as f64), 1, 100);
                o.sizes.insert("bytes".into(), 7.0);
                o
            })
            .collect();
        let m = Model::of(runs, &[], &[], &t);
        assert_eq!(m.by[0].size, "km2");
        let w = m.work(&[(TARGET.to_string(), 1.0), ("km2".to_string(), 3.0)].into()).unwrap();
        assert!((w - 6.0).abs() < 1e-9);
        // (Without its area, by the target.)
        assert!(m.work(&[(TARGET.to_string(), 1.0)].into()).is_some());
    }
}
