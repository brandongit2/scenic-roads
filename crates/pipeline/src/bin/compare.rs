//! The cutover's comparison (docs/plan.md §10, phase 6): two builds of the map in counts and
//! distributions, unit by unit and over the catalog, as a Markdown report of what differs.
//!
//!   compare --root <NAS project folder> [--old <catalog file>] [--new <catalog file> | --new build]
//!           [--units z/x/y,…] [--threads n] [--out report.md]
//!
//! - `--old`: the newest served catalog (`catalog/`) unless given.
//! - `--new`: the newest held catalog (`catalog-held/`) unless given; `build` takes the base packs
//!   and road values the build has recorded so far (`state/build/manifest.json`), so units can be
//!   compared as they're built.
//! - **Units:** those both sides have whose base pack or road values are other files (the same
//!   files hold the same data). Over them: ways and road and rail km (by class and way flag);
//!   elevations (road km by height, by grade, by DEM source); the scenic channels (road km by
//!   value); samples; and the chaining's road lengths. Then the units that differ most.
//! - **Hi data,** for the z6 tiles both sides have in other files: query parts and their roads'
//!   lengths (the drives' length filter), and climbs with their gains and lengths.
//! - **Catalogs** (when `--new` is one): units, layers' packs, global files, landmarks by kind,
//!   credits, coverage and the map's meta.
//!
//! Nothing is judged here: roads' lengths and climbs change by design under the new chaining, and
//! newer OSM data moves the counts. The report is read before the hold is released.

use anyhow::{bail, ensure, Context, Result};
use pipeline::basepack::{BasePack, Sect};
use pipeline::legacy::Unit;
use rayon::prelude::*;
use roadcore::packs::{Climb, Part};
use roadcore::scenic::ch;
use roadcore::{class, dist_m, DemSource, E7, NDEM};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

/// Bands of 10 m of elevation, from −500 m.
const ELEV_BANDS: usize = 700;
/// Lengths in quarter octaves from 10 m (10 m to ~650 km).
const LEN_BINS: usize = 64;
/// Climbs' gains in bands of 20 m.
const GAIN_BINS: usize = 40;
/// Road classes, ferries and the rail groups (roadcore::class).
const CLASSES: usize = class::NAMES.len();

fn len_bin(m: f64) -> usize {
    if m <= 10.0 {
        return 0;
    }
    (((m / 10.0).log2() * 4.0) as usize).min(LEN_BINS - 1)
}

/// The lower edge of length bin `b`, metres.
fn len_edge(b: usize) -> f64 {
    10.0 * 2f64.powf(b as f64 / 4.0)
}

/// One build of the map: its files by logical name.
struct Side {
    label: String,
    catalog: Option<store::catalog::Catalog>,
    files: BTreeMap<String, PathBuf>,
}

impl Side {
    fn catalog(root: &Path, path: &Path) -> Result<Side> {
        let c = store::catalog::read(path)?;
        let files = c.files.iter().map(|(l, f)| (l.clone(), root.join(&f.file))).collect();
        Ok(Side { label: format!("catalog {} of {} (`{}`)", c.n, c.created, path.display()), catalog: Some(c), files })
    }

    fn build(root: &Path) -> Result<Side> {
        let p = root.join("state/build/manifest.json");
        let m: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?)?;
        let files = m.into_iter().map(|(l, c)| (l, root.join(c))).collect();
        Ok(Side { label: format!("the build's records so far (`{}`)", p.display()), catalog: None, files })
    }

    /// The units it has base packs for.
    fn units(&self) -> BTreeSet<Unit> {
        self.files.keys().filter_map(|l| l.strip_prefix("base/")).filter_map(Unit::parse).collect()
    }

    /// A unit's base pack and road values.
    fn unit_files(&self, u: Unit) -> Option<(&PathBuf, &PathBuf)> {
        Some((self.files.get(&format!("base/{}", u.dash()))?, self.files.get(&format!("global/roads/{}", u.dash()))?))
    }

    /// The z6 tiles it has hi data for.
    fn hi_tiles(&self) -> BTreeSet<Unit> {
        self.files.keys().filter_map(|l| l.strip_prefix("hidata/")).filter_map(Unit::parse).collect()
    }
}

/// Units' counts and distributions, added up.
#[derive(Clone)]
struct Stats {
    units: u64,
    ways: u64,
    verts: u64,
    samples: u64,
    class_n: [u64; CLASSES],
    class_km: [f64; CLASSES],
    /// Road km with each way flag bit set (roadcore::flag).
    flag_km: [f64; 8],
    /// Rail km per service group (a track counts for every group on it).
    rail_km: [f64; 5],
    /// Road km by 10 m of elevation, by |grade| (0.5 % units), by DEM source and by each scenic
    /// channel's value, each vertex taking half of each segment it ends.
    elev: Vec<f64>,
    grade: Vec<f64>,
    src: [f64; NDEM],
    chan: Vec<[f64; 256]>,
    /// Base packs without scenic channels.
    no_scenic: u64,
    /// Ways of no known class, and road km of no known DEM source.
    odd_class: u64,
    odd_src: f64,
    /// Road km by the length of the road its way is part of (the chaining's), and the roads.
    road_len: [f64; LEN_BINS],
    roads: u64,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            units: 0,
            ways: 0,
            verts: 0,
            samples: 0,
            class_n: [0; CLASSES],
            class_km: [0.0; CLASSES],
            flag_km: [0.0; 8],
            rail_km: [0.0; 5],
            elev: vec![0.0; ELEV_BANDS],
            grade: vec![0.0; 256],
            src: [0.0; NDEM],
            chan: vec![[0.0; 256]; ch::N],
            no_scenic: 0,
            odd_class: 0,
            odd_src: 0.0,
            road_len: [0.0; LEN_BINS],
            roads: 0,
        }
    }
}

impl Stats {
    fn of(base: &Path, roads: &Path) -> Result<Stats> {
        let bp = BasePack::open(base, roads)?;
        let (ways, verts, elev, grade, src, rv) = (bp.ways()?, bp.verts()?, bp.elev()?, bp.grade()?, bp.src()?, bp.road_vals()?);
        let scenic = bp.scenic();
        let n = verts.len();
        ensure!(elev.len() == n && grade.len() == n && src.len() == n && scenic.is_none_or(|x| x.len() == n), "a per-vertex section isn't one record per vertex");
        ensure!(ways.iter().all(|w| (w.vstart + w.vcount as u64) as usize <= n), "a way's vertices run past the vertices");
        let mut s = Stats { units: 1, ways: ways.len() as u64, verts: verts.len() as u64, samples: bp.samples().map(|x| x.len() as u64).unwrap_or(0), no_scenic: scenic.is_none() as u64, ..Default::default() };
        let mut roads = BTreeSet::new();
        for (i, w) in ways.iter().enumerate() {
            let r = bp.range(w);
            let v = &verts[r.clone()];
            let seg: Vec<f64> = (1..v.len()).map(|k| dist_m(v[k - 1][0] as f64 * E7, v[k - 1][1] as f64 * E7, v[k][0] as f64 * E7, v[k][1] as f64 * E7) / 1000.0).collect();
            let km: f64 = seg.iter().sum();
            let Some(c) = ((w.class as usize) < CLASSES).then_some(w.class as usize) else {
                s.odd_class += 1;
                continue;
            };
            s.class_n[c] += 1;
            s.class_km[c] += km;
            if class::is_rail(w.class) {
                for (k, x) in s.rail_km.iter_mut().enumerate() {
                    if w.rail >> k & 1 == 1 {
                        *x += km;
                    }
                }
                continue;
            }
            if w.class == class::FERRY {
                continue;
            }
            for (b, x) in s.flag_km.iter_mut().enumerate() {
                if w.flags >> b & 1 == 1 {
                    *x += km;
                }
            }
            s.road_len[len_bin(rv[i].len as f64)] += km;
            roads.insert(rv[i].road);
            for (k, j) in r.enumerate() {
                let share = (if k > 0 { seg[k - 1] } else { 0.0 } + seg.get(k).copied().unwrap_or(0.0)) / 2.0;
                let m = elev.m(j) as f64;
                s.elev[(((m + 500.0) / 10.0).max(0.0) as usize).min(ELEV_BANDS - 1)] += share;
                s.grade[grade[j] as usize] += share;
                match s.src.get_mut(src[j] as usize) {
                    Some(x) => *x += share,
                    None => s.odd_src += share,
                }
                if let Some(sc) = scenic {
                    for (cv, x) in sc[j].iter().zip(s.chan.iter_mut()) {
                        x[*cv as usize] += share;
                    }
                }
            }
        }
        s.roads = roads.len() as u64;
        Ok(s)
    }

    fn add(&mut self, o: &Stats) {
        self.units += o.units;
        self.ways += o.ways;
        self.verts += o.verts;
        self.samples += o.samples;
        self.no_scenic += o.no_scenic;
        self.odd_class += o.odd_class;
        self.odd_src += o.odd_src;
        self.roads += o.roads;
        for (a, b) in self.class_n.iter_mut().zip(o.class_n) {
            *a += b;
        }
        add_all(&mut self.class_km, &o.class_km);
        add_all(&mut self.flag_km, &o.flag_km);
        add_all(&mut self.rail_km, &o.rail_km);
        add_all(&mut self.elev, &o.elev);
        add_all(&mut self.grade, &o.grade);
        add_all(&mut self.src, &o.src);
        add_all(&mut self.road_len, &o.road_len);
        for (a, b) in self.chan.iter_mut().zip(&o.chan) {
            add_all(a, b);
        }
    }

    /// Road km (not ferries, not rail).
    fn road_km(&self) -> f64 {
        self.class_km[..class::FERRY as usize].iter().sum()
    }

    fn elev_mean(&self) -> f64 {
        mean(&self.elev, |b| b as f64 * 10.0 - 495.0)
    }

    fn chan_mean(&self, c: usize) -> f64 {
        mean(&self.chan[c], |v| v as f64)
    }

    /// Each DEM source's share of the road km.
    fn src_shares(&self) -> Vec<f64> {
        let t: f64 = self.src.iter().sum();
        self.src.iter().map(|x| if t > 0.0 { x / t } else { 0.0 }).collect()
    }
}

fn add_all(a: &mut [f64], b: &[f64]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += y;
    }
}

/// The weighted mean of a histogram, bin `b` standing for `value(b)`.
fn mean(h: &[f64], value: impl Fn(usize) -> f64) -> f64 {
    let t: f64 = h.iter().sum();
    if t == 0.0 {
        return 0.0;
    }
    h.iter().enumerate().map(|(b, w)| w * value(b)).sum::<f64>() / t
}

/// The bin where a histogram's weight reaches the share `q` (None when it has none).
fn quantile(h: &[f64], q: f64) -> Option<usize> {
    let t: f64 = h.iter().sum();
    if t == 0.0 {
        return None;
    }
    let mut acc = 0.0;
    for (b, w) in h.iter().enumerate() {
        acc += w;
        if acc >= q * t {
            return Some(b);
        }
    }
    Some(h.len() - 1)
}

/// A table row of two quantiles turned into values, "–" for one that has none.
fn row_q(out: &mut String, what: &str, a: Option<f64>, b: Option<f64>, unit: &str, digits: usize) {
    let f = |v: Option<f64>| v.map(|v| format!("{v:.digits$}{unit}")).unwrap_or_else(|| "–".into());
    let d = match (a, b) {
        (Some(a), Some(b)) => format!("{:+.digits$}{unit}", b - a),
        _ => "".into(),
    };
    writeln!(out, "| {what} | {} | {} | {d} |", f(a), f(b)).unwrap();
}

/// The share of a histogram's weight in bin 0.
fn zero_share(h: &[f64]) -> f64 {
    let t: f64 = h.iter().sum();
    if t == 0.0 { 0.0 } else { h[0] / t }
}

/// Hi data's counts and distributions, added up over z6 tiles.
#[derive(Clone, Default)]
struct HiStats {
    tiles: u64,
    parts: u64,
    psamples: u64,
    /// Parts by their road's length (quarter octaves from 10 m).
    part_len: Vec<f64>,
    climbs: u64,
    climb_gain: Vec<f64>,
    climb_len: Vec<f64>,
    gain_m: f64,
    climb_m: f64,
}

impl HiStats {
    fn of(path: &Path) -> Result<HiStats> {
        let s = Sect::open(path)?;
        let mut h = HiStats { tiles: 1, part_len: vec![0.0; LEN_BINS], climb_gain: vec![0.0; GAIN_BINS], climb_len: vec![0.0; LEN_BINS], ..Default::default() };
        if s.has("parts") {
            let parts: &[Part] = s.slice("parts")?;
            h.parts = parts.len() as u64;
            for p in parts {
                h.psamples += p.count as u64;
                h.part_len[len_bin(p.road_len as f64)] += 1.0;
            }
        }
        if s.has("climbs") {
            let climbs: &[Climb] = s.slice("climbs")?;
            h.climbs = climbs.len() as u64;
            for c in climbs {
                h.climb_gain[((c.gain_m / 20.0).max(0.0) as usize).min(GAIN_BINS - 1)] += 1.0;
                h.climb_len[len_bin(c.length_m as f64)] += 1.0;
                h.gain_m += c.gain_m as f64;
                h.climb_m += c.length_m as f64;
            }
        }
        Ok(h)
    }

    fn add(&mut self, o: &HiStats) {
        if self.part_len.is_empty() {
            (self.part_len, self.climb_gain, self.climb_len) = (vec![0.0; LEN_BINS], vec![0.0; GAIN_BINS], vec![0.0; LEN_BINS]);
        }
        self.tiles += o.tiles;
        self.parts += o.parts;
        self.psamples += o.psamples;
        self.climbs += o.climbs;
        self.gain_m += o.gain_m;
        self.climb_m += o.climb_m;
        add_all(&mut self.part_len, &o.part_len);
        add_all(&mut self.climb_gain, &o.climb_gain);
        add_all(&mut self.climb_len, &o.climb_len);
    }
}

// ---- the report --------------------------------------------------------------------------------

fn num(x: f64) -> String {
    let neg = x < 0.0;
    let s = format!("{:.0}", x.abs());
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if neg { format!("−{out}") } else { out }
}

/// The change from `a` to `b`, in percent of `a`.
fn pct(a: f64, b: f64) -> String {
    if a == 0.0 {
        return if b == 0.0 { "".into() } else { "new".into() };
    }
    let p = (b - a) / a * 100.0;
    if p.abs() < 0.05 { "0 %".into() } else { format!("{p:+.1} %") }
}

fn row(out: &mut String, what: &str, a: f64, b: f64) {
    writeln!(out, "| {what} | {} | {} | {} |", num(a), num(b), pct(a, b)).unwrap();
}

fn row_f(out: &mut String, what: &str, a: f64, b: f64, unit: &str, digits: usize) {
    writeln!(out, "| {what} | {a:.digits$}{unit} | {b:.digits$}{unit} | {:+.digits$}{unit} |", b - a).unwrap();
}

const FLAG_NAMES: [&str; 8] = ["links", "bridges", "tunnels", "unpaved", "one-way", "toll", "designated scenic", "covered bridges"];
const RAIL_NAMES: [&str; 5] = ["tram", "metro", "commuter", "intercity", "heritage"];
const CHAN_NAMES: [&str; ch::N] = ["view", "water", "relief", "height above the land around (TPI)", "curviness", "enclosure", "built-up", "flags (any)", "vista", "open land", "forest cover", "tree height", "buildings lining"];
const SCENIC_FLAG_NAMES: [&str; 8] = ["scenic route", "park", "viewpoint", "waterfront", "heritage", "covered bridge", "special area", "indigenous"];

fn units_section(out: &mut String, a: &Stats, b: &Stats) {
    writeln!(out, "\n## Roads and rail\n\n| | old | new | change |\n|---|---:|---:|---:|").unwrap();
    row(out, "ways", a.ways as f64, b.ways as f64);
    row(out, "vertices", a.verts as f64, b.verts as f64);
    row(out, "samples (every ~100 m)", a.samples as f64, b.samples as f64);
    row(out, "road km", a.road_km(), b.road_km());
    for c in 0..class::FERRY as usize {
        row(out, &format!("&nbsp;&nbsp;{} km ({} ways → {})", class::NAMES[c], num(a.class_n[c] as f64), num(b.class_n[c] as f64)), a.class_km[c], b.class_km[c]);
    }
    row(out, "ferry km", a.class_km[class::FERRY as usize], b.class_km[class::FERRY as usize]);
    for (k, n) in RAIL_NAMES.iter().enumerate() {
        row(out, &format!("rail km, {n} services"), a.rail_km[k], b.rail_km[k]);
    }
    for (k, n) in FLAG_NAMES.iter().enumerate() {
        row(out, &format!("road km, {n}"), a.flag_km[k], b.flag_km[k]);
    }
    if a.no_scenic + b.no_scenic > 0 {
        writeln!(out, "\nBase packs without scenic channels: {} old, {} new.", a.no_scenic, b.no_scenic).unwrap();
    }
    if a.odd_class + b.odd_class > 0 || a.odd_src + b.odd_src > 0.0 {
        writeln!(out, "\nWays of no known class (left out): {} old, {} new. Road km of no known DEM source: {} old, {} new.", a.odd_class, b.odd_class, num(a.odd_src), num(b.odd_src)).unwrap();
    }

    writeln!(out, "\n## Elevations\n\n| | old | new | change |\n|---|---:|---:|---:|").unwrap();
    row_f(out, "mean road elevation", a.elev_mean(), b.elev_mean(), " m", 1);
    // (The top of the 10 m band the share reaches: at least that share of the road km is below it.)
    let elev_q = |s: &Stats, q: f64| quantile(&s.elev, q).map(|b| (b + 1) as f64 * 10.0 - 500.0);
    for q in [0.1, 0.5, 0.9, 0.99] {
        row_q(out, &format!("{:.0} % of road km below (10 m bands)", q * 100.0), elev_q(a, q), elev_q(b, q), " m", 0);
    }
    row_f(out, "mean |grade|", mean(&a.grade, |g| g as f64 * 0.5), mean(&b.grade, |g| g as f64 * 0.5), " %", 2);
    for q in [0.5, 0.9, 0.99] {
        let g = |s: &Stats| quantile(&s.grade, q).map(|b| b as f64 * 0.5);
        row_q(out, &format!("|grade| p{:.0}", q * 100.0), g(a), g(b), " %", 1);
    }
    let (sa, sb) = (a.src_shares(), b.src_shares());
    for d in 0..NDEM {
        if sa[d] + sb[d] > 0.0 {
            row_f(out, &format!("road km from {}", DemSource::label(d as u8)), sa[d] * 100.0, sb[d] * 100.0, " %", 2);
        }
    }

    writeln!(out, "\n## Scenic channels (road km by value, 0–255)\n\n| channel | old mean | new mean | change | old at 0 | new at 0 | old p90 | new p90 |\n|---|---:|---:|---:|---:|---:|---:|---:|").unwrap();
    for (c, n) in CHAN_NAMES.iter().enumerate() {
        if c == ch::FLAGS {
            continue;
        }
        writeln!(
            out,
            "| {n} | {:.1} | {:.1} | {:+.1} | {:.1} % | {:.1} % | {} | {} |",
            a.chan_mean(c),
            b.chan_mean(c),
            b.chan_mean(c) - a.chan_mean(c),
            zero_share(&a.chan[c]) * 100.0,
            zero_share(&b.chan[c]) * 100.0,
            quantile(&a.chan[c], 0.9).map(|v| v.to_string()).unwrap_or_else(|| "–".into()),
            quantile(&b.chan[c], 0.9).map(|v| v.to_string()).unwrap_or_else(|| "–".into())
        )
        .unwrap();
    }
    writeln!(out, "\n| scenic flag | old road km | new road km | change |\n|---|---:|---:|---:|").unwrap();
    let flag_km = |s: &Stats, bit: usize| s.chan[ch::FLAGS].iter().enumerate().filter(|(v, _)| v >> bit & 1 == 1).map(|(_, w)| w).sum::<f64>();
    for (bit, n) in SCENIC_FLAG_NAMES.iter().enumerate() {
        row(out, n, flag_km(a, bit), flag_km(b, bit));
    }

    writeln!(out, "\n## Roads as chained\n\nA way's road is its chain of ways (docs/formats.md, Road values); the length filter and the drives read the road's length.\n\n| | old | new | change |\n|---|---:|---:|---:|").unwrap();
    row(out, "roads (counted in each unit they cross)", a.roads as f64, b.roads as f64);
    for q in [0.1, 0.5, 0.9] {
        let l = |s: &Stats| quantile(&s.road_len, q).map(len_edge);
        row_q(out, &format!("road km on roads of at least (p{:.0})", q * 100.0), l(a), l(b), " m", 0);
    }
}

fn hi_section(out: &mut String, a: &HiStats, b: &HiStats) {
    writeln!(out, "\n## Hi data: query parts and climbs ({} z6 tiles)\n\n| | old | new | change |\n|---|---:|---:|---:|", a.tiles).unwrap();
    row(out, "query parts", a.parts as f64, b.parts as f64);
    row(out, "their samples", a.psamples as f64, b.psamples as f64);
    for q in [0.1, 0.5, 0.9] {
        let l = |h: &HiStats| quantile(&h.part_len, q).map(len_edge);
        row_q(out, &format!("parts on roads of at least (p{:.0})", q * 100.0), l(a), l(b), " m", 0);
    }
    row(out, "climbs", a.climbs as f64, b.climbs as f64);
    row(out, "climbs' gain, m", a.gain_m, b.gain_m);
    row(out, "climbs' length, m", a.climb_m, b.climb_m);
    for q in [0.5, 0.9] {
        let g = |h: &HiStats| quantile(&h.climb_gain, q).map(|b| b as f64 * 20.0);
        row_q(out, &format!("climbs gaining at least (p{:.0}, 20 m bands)", q * 100.0), g(a), g(b), " m", 0);
    }
}

/// Per unit, how far apart the two sides are, for ranking.
struct UnitDiff {
    unit: Unit,
    km: (f64, f64),
    ways: (f64, f64),
    elev: f64,
    chan: (f64, usize),
    src: f64,
}

impl UnitDiff {
    fn of(unit: Unit, a: &Stats, b: &Stats) -> UnitDiff {
        let chan = (0..ch::N).filter(|&c| c != ch::FLAGS).map(|c| ((b.chan_mean(c) - a.chan_mean(c)).abs(), c)).fold((0.0, 0), |m, x| if x.0 > m.0 { x } else { m });
        let src = a.src_shares().iter().zip(b.src_shares()).map(|(x, y)| (x - y).abs()).sum::<f64>() * 50.0;
        UnitDiff { unit, km: (a.road_km(), b.road_km()), ways: (a.ways as f64, b.ways as f64), elev: b.elev_mean() - a.elev_mean(), chan, src }
    }

    /// The largest difference, scaled: 10 % of km or ways, 10 m of mean elevation, 10 of a
    /// channel's mean or 10 % of the road km changing DEM source each count 1.
    fn score(&self) -> f64 {
        let rel = |(a, b): (f64, f64)| if a > 0.0 { ((b - a) / a).abs() * 10.0 } else if b > 0.0 { 10.0 } else { 0.0 };
        [rel(self.km), rel(self.ways), self.elev.abs() / 10.0, self.chan.0 / 10.0, self.src / 10.0].into_iter().fold(0.0, f64::max)
    }
}

fn per_unit_section(out: &mut String, diffs: &mut [UnitDiff]) {
    diffs.sort_by(|a, b| b.score().total_cmp(&a.score()));
    writeln!(out, "\n## The units that differ most\n\n| unit | road km | ways | mean elevation | largest channel change | DEM source moved |\n|---|---:|---:|---:|---|---:|").unwrap();
    for d in diffs.iter().take(30) {
        writeln!(
            out,
            "| {} | {} → {} ({}) | {} ({}) | {:+.1} m | {} {:+.1} | {:.1} % |",
            d.unit.slash(),
            num(d.km.0),
            num(d.km.1),
            pct(d.km.0, d.km.1),
            num(d.ways.1),
            pct(d.ways.0, d.ways.1),
            d.elev,
            CHAN_NAMES[d.chan.1],
            d.chan.0,
            d.src
        )
        .unwrap();
    }
}

fn catalog_section(out: &mut String, root: &Path, a: &store::catalog::Catalog, b: &store::catalog::Catalog) {
    writeln!(out, "\n## The catalogs\n\n| | old | new | change |\n|---|---:|---:|---:|").unwrap();
    row(out, "units", a.units.len() as f64, b.units.len() as f64);
    row(out, "files", a.files.len() as f64, b.files.len() as f64);
    row(out, "bytes", a.files.values().map(|f| f.size as f64).sum(), b.files.values().map(|f| f.size as f64).sum());
    row(out, "hi data tiles", a.hidata.len() as f64, b.hidata.len() as f64);
    row(out, "landmark tiles (markdata)", a.markdata.len() as f64, b.markdata.len() as f64);
    row(out, "overlay tiles (ovdata)", a.ovdata.len() as f64, b.ovdata.len() as f64);
    row(out, "basemap archives", a.basemap.len() as f64, b.basemap.len() as f64);
    let credits = |c: &store::catalog::Catalog| c.credits.as_array().map(|x| x.len()).unwrap_or(0) as f64;
    row(out, "credits", credits(a), credits(b));
    let regions = |c: &store::catalog::Catalog| c.coverage.get("regions").and_then(|x| x.as_array()).map(|x| x.len()).unwrap_or(0) as f64;
    row(out, "coverage regions", regions(a), regions(b));
    for k in ["ways", "vertices"] {
        row(out, &format!("meta: {k}"), a.meta.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0), b.meta.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0));
    }
    let (ua, ub): (BTreeSet<&String>, BTreeSet<&String>) = (a.units.iter().collect(), b.units.iter().collect());
    let only = |x: &BTreeSet<&String>, y: &BTreeSet<&String>| x.difference(y).map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    writeln!(out, "\nUnits only in the old: {}.  \nUnits only in the new: {}.", or_none(only(&ua, &ub)), or_none(only(&ub, &ua))).unwrap();

    writeln!(out, "\n| layer | old packs (root/lo/hi) | new packs | zooms old → new |\n|---|---:|---:|---|").unwrap();
    for l in a.layers.keys().chain(b.layers.keys()).collect::<BTreeSet<_>>() {
        let packs = |c: &store::catalog::Catalog| c.layers.get(l).map(|x| format!("{}/{}/{}", x.root.is_some() as u8, x.lo.len(), x.hi.len())).unwrap_or_else(|| "none".into());
        let zooms = |c: &store::catalog::Catalog| c.layers.get(l).map(|x| format!("{}–{}", x.minzoom, x.maxzoom)).unwrap_or_else(|| "–".into());
        writeln!(out, "| {l} | {} | {} | {} → {} |", packs(a), packs(b), zooms(a), zooms(b)).unwrap();
    }
    let (ga, gb): (BTreeSet<&String>, BTreeSet<&String>) = (a.global.keys().collect(), b.global.keys().collect());
    writeln!(out, "\nGlobal files only in the old: {}.  \nGlobal files only in the new: {}.", or_none(only(&ga, &gb)), or_none(only(&gb, &ga))).unwrap();

    // Landmarks by kind and tier, from each catalog's summary.
    let summary = |c: &store::catalog::Catalog| -> Option<serde_json::Value> {
        let f = c.files.get("global/marks/summary")?;
        serde_json::from_slice(&std::fs::read(root.join(&f.file)).ok()?).ok()
    };
    if let (Some(x), Some(y)) = (summary(a), summary(b)) {
        writeln!(out, "\n| landmarks | old | new | change |\n|---|---:|---:|---:|").unwrap();
        for group in ["kinds", "tiers"] {
            let (gx, gy) = (x.get(group).and_then(|v| v.as_object()), y.get(group).and_then(|v| v.as_object()));
            let keys: BTreeSet<&String> = gx.into_iter().flat_map(|o| o.keys()).chain(gy.into_iter().flat_map(|o| o.keys())).collect();
            for k in keys {
                let v = |g: Option<&serde_json::Map<String, serde_json::Value>>| g.and_then(|o| o.get(k)).and_then(|v| v.as_f64()).unwrap_or(0.0);
                row(out, &format!("{group}: {k}"), v(gx), v(gy));
            }
        }
    }
}

fn or_none(s: String) -> String {
    if s.is_empty() { "none".into() } else { s }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(opt(&args, "--root").context("--root <NAS project folder>")?);
    let newest = |dir: &str| -> Result<Option<PathBuf>> { Ok(store::catalog::list(&root.join(dir))?.first().map(|n| root.join(dir).join(store::catalog::file_name(*n)))) };
    let old_path = match opt(&args, "--old") {
        Some(p) => PathBuf::from(p),
        None => newest("catalog")?.context("no catalog in catalog/")?,
    };
    let old = Side::catalog(&root, &old_path)?;
    let new = match opt(&args, "--new").as_deref() {
        Some("build") => Side::build(&root)?,
        Some(p) => Side::catalog(&root, Path::new(p))?,
        None => match newest("catalog-held")? {
            Some(p) => Side::catalog(&root, &p)?,
            None => Side::build(&root)?,
        },
    };
    let threads: usize = opt(&args, "--threads").map(|t| t.parse()).transpose()?.unwrap_or(4);
    let only: Option<BTreeSet<Unit>> = opt(&args, "--units").map(|s| s.split(',').filter_map(Unit::parse).collect());

    // Units both have in other files.
    let (ou, nu) = (old.units(), new.units());
    let both: Vec<Unit> = ou.intersection(&nu).copied().filter(|u| only.as_ref().is_none_or(|o| o.contains(u))).collect();
    let mut same = 0;
    let mut todo = Vec::new();
    for u in &both {
        let (Some(a), Some(b)) = (old.unit_files(*u), new.unit_files(*u)) else { continue };
        if a == b {
            same += 1;
        } else {
            todo.push((*u, a, b));
        }
    }
    if todo.is_empty() && only.is_some() {
        bail!("none of those units differ");
    }
    eprintln!("compare: {} units in both, {same} the same files, {} to compare", both.len(), todo.len());
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let read: Vec<(Unit, Result<(Stats, Stats)>)> = pool.install(|| {
        todo.par_iter()
            .map(|(u, (ab, ar), (bb, br))| {
                let r = Stats::of(ab, ar).with_context(|| format!("old: {}", ab.display())).and_then(|a| Ok((a, Stats::of(bb, br).with_context(|| format!("new: {}", bb.display()))?)));
                let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                eprintln!("compare: {} ({k}/{})", u.slash(), todo.len());
                (*u, r)
            })
            .collect()
    });
    let (mut per, mut unread) = (Vec::new(), Vec::new());
    for (u, r) in read {
        match r {
            Ok((a, b)) => per.push((u, a, b)),
            Err(e) => unread.push((u, format!("{e:#}"))),
        }
    }
    let (mut ta, mut tb) = (Stats::default(), Stats::default());
    let mut diffs = Vec::new();
    for (u, a, b) in &per {
        ta.add(a);
        tb.add(b);
        diffs.push(UnitDiff::of(*u, a, b));
    }

    // Hi data in other files.
    let hi: Vec<(Unit, PathBuf, PathBuf)> = old
        .hi_tiles()
        .intersection(&new.hi_tiles())
        .filter(|t| only.as_ref().is_none_or(|o| o.contains(t)))
        .filter_map(|t| {
            let (a, b) = (old.files.get(&format!("hidata/{}", t.dash()))?, new.files.get(&format!("hidata/{}", t.dash()))?);
            (a != b).then(|| (*t, a.clone(), b.clone()))
        })
        .collect();
    let his: Vec<(HiStats, HiStats)> = pool.install(|| hi.par_iter().map(|(t, a, b)| Ok((HiStats::of(a).with_context(|| format!("old hidata {}", t.slash()))?, HiStats::of(b).with_context(|| format!("new hidata {}", t.slash()))?))).collect::<Result<Vec<_>>>())?;
    let (mut ha, mut hb) = (HiStats::default(), HiStats::default());
    for (a, b) in &his {
        ha.add(a);
        hb.add(b);
    }

    let mut out = String::new();
    writeln!(out, "# The map, old and new\n\n- Old: {}\n- New: {}\n- Units: {} in both; {same} in the same files; {} compared. {} only in the old, {} only in the new.", old.label, new.label, both.len(), per.len(), ou.difference(&nu).count(), nu.difference(&ou).count()).unwrap();
    writeln!(out, "\nRoad figures are owned ways' (a way belongs to the unit of its first vertex). Elevations, grades, sources and channels weigh each vertex by half the length of each segment it ends. Roads' lengths and climbs change by design under the new chaining; newer OSM data moves the counts.").unwrap();
    if !unread.is_empty() {
        writeln!(out, "\n**Units that couldn't be read** (left out of what follows):\n").unwrap();
        for (u, e) in &unread {
            writeln!(out, "- {}: {e}", u.slash()).unwrap();
        }
    }
    if !per.is_empty() {
        units_section(&mut out, &ta, &tb);
        per_unit_section(&mut out, &mut diffs);
    }
    if !his.is_empty() {
        hi_section(&mut out, &ha, &hb);
    }
    if let (Some(a), Some(b)) = (&old.catalog, &new.catalog) {
        if only.is_none() {
            catalog_section(&mut out, &root, a, b);
        }
    }
    match opt(&args, "--out") {
        Some(p) => std::fs::write(&p, out).with_context(|| format!("write {p}"))?,
        None => print!("{out}"),
    }
    Ok(())
}
