//! The summaries' answers against the exact ones (docs/phase5.md "Accuracy and checks"), on
//! today's hidata (this Mac's mirror), with their summaries built by `roadcore::lsum::build` as
//! pack(T) writes them:
//!
//!     cargo test --release -p server approx_vs_exact -- --ignored --nocapture
//!
//! `SCENIC_HIDATA`: another folder of hidata files; `RAILFREQ`: global/railfreq, for trains a day.

use super::*;
use crate::views::{HiView, SectView, Src};
use roadcore::packs::RailInfo;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

/// Lines by their first way (older hidata have no `railinfo`): exact and summaries name a run's
/// line alike, since a run's first bin starts at its first sample.
struct TestRail {
    freqs: Vec<(u32, f32)>,
}

fn line_of(way: u64, class: u8) -> LineInfo {
    LineInfo { ident: way.to_string(), services: String::new(), colour: 0, rel: 0, way, rail: 0, class }
}

impl Rail for TestRail {
    fn freq(&self, way: u64) -> f32 {
        freq_in(&self.freqs, way)
    }
    fn line(&self, t: &QTile, p: &PSample) -> anyhow::Result<Option<LineInfo>> {
        let h = &t.here()[p.way as usize];
        Ok(Some(line_of(h.id, h.class)))
    }
    fn bin_line(&self, _: &LTile, b: &LBin) -> Option<LineInfo> {
        Some(line_of(b.way, b.class))
    }
}

struct Tiles {
    exact: BTreeMap<(u32, u32), QTile>,
    roads: BTreeMap<(u32, u32), LTile>,
    rail: BTreeMap<(u32, u32), LTile>,
    /// Bytes: the query sections, and the summaries (roads, rail).
    q_bytes: usize,
    l_bytes: (usize, usize),
}

fn load() -> Tiles {
    let dir = std::env::var("SCENIC_HIDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join("Library/Application Support/scenic/mirror/hidata"));
    // The newest file of each tile ("6-x-y.<hash>.sect").
    let mut newest: BTreeMap<(u32, u32), (std::time::SystemTime, PathBuf)> = BTreeMap::new();
    for e in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let parts: Vec<&str> = name.split(['-', '.']).collect();
        if parts.len() < 4 || parts[0] != "6" || !name.ends_with(".sect") {
            continue;
        }
        let (Ok(x), Ok(y)) = (parts[1].parse(), parts[2].parse()) else { continue };
        let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if newest.get(&(x, y)).is_none_or(|o| o.0 < t) {
            newest.insert((x, y), (t, e.path()));
        }
    }
    let mut out = Tiles { exact: BTreeMap::new(), roads: BTreeMap::new(), rail: BTreeMap::new(), q_bytes: 0, l_bytes: (0, 0) };
    for ((x, y), (_, path)) in newest {
        let f = std::fs::File::open(&path).unwrap();
        // SAFETY: the mirror's files aren't changed while mapped (content-named, replaced whole).
        let map = unsafe { memmap2::Mmap::map(&f) }.unwrap();
        let hv = Arc::new(HiView::new(SectView::open(Src::Local(Arc::new(map))).unwrap()).unwrap());
        let q = QTile::new(hv.clone()).unwrap();
        let ri = hv.railinfo.all().unwrap();
        let ri: &[RailInfo] = ri.cast();
        let ls = lsum::build(q.parts(), q.psamples(), q.pch(), q.here(), ri);
        let names = if ri.is_empty() { Vec::new() } else { hv.rail_names().unwrap().as_ref().clone() };
        out.q_bytes += std::mem::size_of_val(q.here()) + std::mem::size_of_val(q.parts()) + std::mem::size_of_val(q.psamples()) + std::mem::size_of_val(q.pch());
        out.l_bytes.0 += std::mem::size_of_val(&ls.lparts[..]) + std::mem::size_of_val(&ls.lbins[..]);
        out.l_bytes.1 += std::mem::size_of_val(&ls.lrparts[..]) + std::mem::size_of_val(&ls.lrbins[..]);
        out.roads.insert((x, y), LTile::from_records(&ls.lparts, &ls.lbins, &[], Vec::new()));
        out.rail.insert((x, y), LTile::from_records(&ls.lrparts, &ls.lrbins, ri, names));
        out.exact.insert((x, y), q);
    }
    out
}

fn pick<T: Clone>(m: &BTreeMap<(u32, u32), T>, region: &Region, margin_km: f64) -> Vec<T> {
    tiles_in(grown(region, margin_km)).iter().filter_map(|t| m.get(t).cloned()).collect()
}

fn region(b: [f64; 4]) -> Region {
    Region::parse(&format!("{},{},{},{}", b[0], b[1], b[2], b[3]), None).unwrap()
}

/// The top `k`'s overlap (the same road, windows meeting), and the largest and mean score
/// difference of the exact top `k` found in the summaries' list.
fn compare(e: &[(u64, f32, f32, f32)], a: &[(u64, f32, f32, f32)], k: usize) -> (f32, f32, f32) {
    let k = k.min(e.len()).min(a.len());
    if k == 0 {
        return (1.0, 0.0, 0.0);
    }
    let same = |x: &(u64, f32, f32, f32), y: &(u64, f32, f32, f32)| x.0 == y.0 && x.1 <= y.2 + 1.0 && y.1 <= x.2 + 1.0;
    let ov = e[..k].iter().filter(|x| a[..k].iter().any(|y| same(x, y))).count() as f32 / k as f32;
    let (mut mx, mut sum, mut n) = (0f32, 0f32, 0usize);
    for x in &e[..k] {
        if let Some(y) = a.iter().find(|y| same(x, y)) {
            let d = (y.3 - x.3).abs() * 100.0;
            mx = mx.max(d);
            sum += d;
            n += 1;
        }
    }
    (ov, mx, sum / n.max(1) as f32)
}

const VIEWS: [(&str, [f64; 4]); 9] = [
    ("W Europe", [-10.0, 36.0, 20.0, 56.0]),
    ("Eng+Wales", [-5.0, 50.0, 2.0, 56.0]),
    ("France", [-5.0, 42.0, 8.0, 51.0]),
    ("Iberia", [-10.0, 36.0, 4.0, 44.0]),
    ("S France", [-2.0, 42.0, 8.0, 46.0]),
    ("Scotland", [-8.0, 54.5, -1.0, 59.0]),
    ("BC+AB", [-130.0, 48.0, -114.0, 56.0]),
    ("NE America", [-84.0, 40.0, -66.0, 48.0]),
    ("Japan+Korea", [124.0, 30.0, 142.0, 42.0]),
];
const LENS: [f32; 4] = [2.0, 5.0, 10.0, 25.0];

fn drive_presets() -> Vec<[f32; NCOMP]> {
    vec![
        [2.0, 0.4, 2.0, 2.0, 2.0, 0.4, 0.2, -0.6, -1.5, -1.5, 0.3, 0.2],
        [1.0, 1.0, 0.5, 0.8, 0.3, 0.6, 0.6, 0.0, 0.0, -1.0, 0.5, 0.4],
        [0.5, 1.6, 0.4, 0.2, 0.0, 0.3, 0.6, 0.0, -0.2, -1.0, 0.3, 0.3],
    ]
}

fn ride_presets() -> Vec<[f32; RNCOMP]> {
    vec![
        [2.0, 0.6, 1.5, 1.0, 0.5, 0.4, 0.3, -1.0, 0.3, 0.3, 0.5],
        [2.0, 1.0, 2.0, 2.0, 1.0, 0.5, 0.5, -1.5, 0.5, 0.5, 0.0],
        [0.5, 0.2, 0.5, 0.2, 0.0, 0.0, 0.0, -0.5, 0.0, 0.0, 2.0],
    ]
}

/// Per length: the lowest top-20 overlap, the cases at 90 % or more, the largest score error
/// (top 20, top 30), the largest totals' difference.
#[derive(Default, Debug)]
struct Acc {
    cases: usize,
    min_ov: f32,
    at90: usize,
    max20: f32,
    max30: f32,
    tot: f32,
    ms_exact: f32,
    ms_approx: f32,
}

fn add(acc: &mut Acc, e: &[(u64, f32, f32, f32)], a: &[(u64, f32, f32, f32)], te: usize, ta: usize) {
    let (ov, m20, _) = compare(e, a, 20);
    let (_, m30, _) = compare(e, a, 30);
    if acc.cases == 0 {
        acc.min_ov = 1.0;
    }
    acc.cases += 1;
    acc.min_ov = acc.min_ov.min(ov);
    acc.at90 += (ov >= 0.9) as usize;
    acc.max20 = acc.max20.max(m20);
    acc.max30 = acc.max30.max(m30);
    if te > 0 {
        acc.tot = acc.tot.max((ta as f32 - te as f32).abs() / te as f32);
    }
}

#[test]
#[ignore]
fn approx_vs_exact() {
    let t0 = Instant::now();
    let tiles = load();
    eprintln!(
        "{} tiles in {:.1?}: query sections {:.0} MB, summaries {:.1} MB (rail {:.1} MB)",
        tiles.exact.len(),
        t0.elapsed(),
        tiles.q_bytes as f64 / 1e6,
        (tiles.l_bytes.0 + tiles.l_bytes.1) as f64 / 1e6,
        tiles.l_bytes.1 as f64 / 1e6
    );
    let freqs: Vec<(u32, f32)> = std::env::var("RAILFREQ")
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| b.chunks_exact(8).map(|c| (u32::from_le_bytes(c[..4].try_into().unwrap()), f32::from_le_bytes(c[4..].try_into().unwrap()).abs())).collect())
        .unwrap_or_default();
    let rail = TestRail { freqs };
    let cancel = Cancel::default();
    let mut drives: BTreeMap<u32, Acc> = BTreeMap::new();
    // Rides' trains a day: the same, matched, the largest ratio.
    let mut trains = (0usize, 0usize, 1f32);
    let mut rides: BTreeMap<u32, Acc> = BTreeMap::new();
    for (name, b) in VIEWS {
        let reg = region(b);
        for len_km in LENS {
            let len = len_km * 1000.0;
            let margin = len as f64 / 2000.0;
            let (qt, lt, lr) = (pick(&tiles.exact, &reg, margin), pick(&tiles.roads, &reg, margin), pick(&tiles.rail, &reg, margin));
            for w in drive_presets() {
                let p = DriveParams { len, w, wsum: w.iter().map(|x| x.max(0.0)).sum::<f32>().max(1e-6), classes: u32::MAX, surface: 3, toll: 3, unnamed: 0, lmin: None, lmax: None, limit: 30 };
                let t1 = Instant::now();
                let (te, e) = drives_exact(&qt, &reg, &p, &cancel).unwrap();
                let t2 = Instant::now();
                let (ta, a) = drives_approx(&lt, &reg, &p, &cancel).unwrap();
                let t3 = Instant::now();
                let key = |h: &DriveHit| (h.road, h.a, h.b, h.score);
                let (ek, ak): (Vec<_>, Vec<_>) = (e.iter().map(key).collect(), a.iter().map(key).collect());
                let acc = drives.entry(len_km as u32).or_default();
                add(acc, &ek, &ak, te, ta);
                acc.ms_exact += (t2 - t1).as_secs_f32() * 1000.0;
                acc.ms_approx += (t3 - t2).as_secs_f32() * 1000.0;
                // Every answer is well formed: real positions, the length asked or more.
                for h in &a {
                    assert!(h.geom.len() >= 2 && h.length_m >= len - 1.0, "{name} {len_km}: {} m, {} points", h.length_m, h.geom.len());
                }
            }
            for w in ride_presets() {
                let p = RideParams { len, w, groups: 0xff, limit: 30 };
                let t1 = Instant::now();
                let (te, e) = rides_exact(&qt, &reg, &p, &rail, &cancel).unwrap();
                let t2 = Instant::now();
                let (ta, a) = rides_approx(&lr, &reg, &p, &rail, &cancel).unwrap();
                let t3 = Instant::now();
                let key = |h: &RideHit| (h.road, h.a, h.b, h.score);
                let (ek, ak): (Vec<_>, Vec<_>) = (e.iter().map(key).collect(), a.iter().map(key).collect());
                let acc = rides.entry(len_km as u32).or_default();
                add(acc, &ek, &ak, te, ta);
                acc.ms_exact += (t2 - t1).as_secs_f32() * 1000.0;
                acc.ms_approx += (t3 - t2).as_secs_f32() * 1000.0;
                // Trains a day: the most on the window's ways (a window ending inside a busier way's
                // bin takes its trains: an edge effect).
                for x in &e[..e.len().min(20)] {
                    if let Some(y) = a.iter().find(|y| y.road == x.road && y.a <= x.b + 1.0 && x.a <= y.b + 1.0) {
                        trains.0 += (y.trains == x.trains) as usize;
                        trains.1 += 1;
                        let r = (y.trains.max(1.0) / x.trains.max(1.0)).max(x.trains.max(1.0) / y.trains.max(1.0));
                        trains.2 = trains.2.max(r);
                    }
                }
            }
        }
        // Rail lines: lengths and scores per line, the top 40's overlap.
        let (qt, lr) = (pick(&tiles.exact, &reg, 0.0), pick(&tiles.rail, &reg, 0.0));
        for w in ride_presets() {
            let (_, e) = lines_by_ident(lines_exact(&qt, &reg, &w, 0xff, &rail, &cancel).unwrap());
            let (_, a) = lines_by_ident(lines_approx(&lr, &reg, &w, 0xff, &rail, &cancel).unwrap());
            let (ke, ka): (f32, f32) = (e.iter().map(|x| x.len).sum(), a.iter().map(|x| x.len).sum());
            let by: HashMap<&str, &LineAcc> = a.iter().map(|x| (x.info.ident.as_str(), x)).collect();
            let (mut worst_len, mut worst_km, mut worst_name) = (0f32, 0f32, String::new());
            let mut worst_sc = 0f32;
            for x in &e {
                if let Some(y) = by.get(x.info.ident.as_str()) {
                    let d = (y.len - x.len).abs() / x.len;
                    if d > worst_len {
                        (worst_len, worst_km, worst_name) = (d, x.len / 1e3, x.info.ident.clone());
                    }
                    worst_sc = worst_sc.max((y.sc / y.len - x.sc / x.len).abs() * 100.0);
                }
            }
            let mut es: Vec<&LineAcc> = e.iter().collect();
            let mut as_: Vec<&LineAcc> = a.iter().collect();
            es.sort_by(|x, y| (y.sc / y.len).total_cmp(&(x.sc / x.len)));
            as_.sort_by(|x, y| (y.sc / y.len).total_cmp(&(x.sc / x.len)));
            let k = 40.min(es.len()).min(as_.len());
            let ov = if k == 0 { 1.0 } else { es[..k].iter().filter(|x| as_[..k].iter().any(|y| y.info.ident == x.info.ident)).count() as f32 / k as f32 };
            eprintln!("lines {name}: km {:.0} vs {:.0} ({:+.3} %), worst line length {:.2} % ({worst_name}, {:.1} km), worst score {:.2}, top-40 overlap {:.2}", ke / 1e3, ka / 1e3, (ka - ke) / ke.max(1.0) * 100.0, worst_len * 100.0, worst_km, worst_sc, ov);
            assert!((ka - ke).abs() <= ke * 0.005, "{name}: rail km in view");
            assert!(ov >= 0.9, "{name}: top-40 overlap {ov}");
        }
    }
    eprintln!("rides' trains a day: {} of {} matched the same, largest ratio {:.2}", trains.0, trains.1, trains.2);
    assert!(trains.0 as f32 >= trains.1 as f32 * 0.9, "trains a day");
    eprintln!("len  what    cases  min-ov20  at-90%  max|d|20  max|d|30  totals  exact-ms  approx-ms");
    for (what, m) in [("drives", &drives), ("rides", &rides)] {
        for (len, a) in m {
            eprintln!("{len:>3}  {what:<6}  {:>5}  {:>8.2}  {:>6}  {:>8.2}  {:>8.2}  {:>5.2}%  {:>8.0}  {:>9.0}", a.cases, a.min_ov, a.at90, a.max20, a.max30, a.tot * 100.0, a.ms_exact / a.cases as f32, a.ms_approx / a.cases as f32);
        }
    }
    // The design's figures (docs/phase5.md), with a little room.
    for (len, a) in &drives {
        let (ov, err) = match len {
            2 => (0.75, 4.5),
            5 => (0.9, 2.0),
            10 => (0.9, 1.0),
            _ => (0.95, 0.6),
        };
        assert!(a.min_ov >= ov && a.max20 <= err && a.tot <= 0.01, "drives {len} km: {a:?}");
    }
    for (len, a) in &rides {
        // (Near-ties: a case at 5 km overlaps 85 %, its scores within 0.6 points.)
        let (ov, err) = match len {
            2 => (0.75, 2.0),
            5 => (0.85, 1.0),
            10 => (0.9, 0.5),
            _ => (0.95, 0.3),
        };
        assert!(a.min_ov >= ov && a.max20 <= err && a.tot <= 0.01, "rides {len} km: {a:?}");
    }
}

