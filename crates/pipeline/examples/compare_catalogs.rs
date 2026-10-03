//! Compares the base packs of two catalogs unit by unit (plan §10, cutover): today's converted data
//! against the same regions built the new way. Ways are matched by OSM id; for those whose geometry
//! is unchanged (OSM edits between the two sources aside) every per-vertex value should be equal:
//! elevations, grades, the 13 scenic channels, each flag bit.
//!   cargo run --release -p pipeline --example compare_catalogs -- <nas root> <catalog a> <catalog b> [--units 6/31/20,…]
use pipeline::basepack::BasePack;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
struct Stats {
    ways_a: usize,
    ways_b: usize,
    matched: usize,
    same_geom: usize,
    verts: usize,
    elev_eq: usize,
    elev_1m: usize,
    elev_max: f32,
    grade_eq: usize,
    ch_diff: [u64; 13],
    ch_n: u64,
    /// Per flag bit: set in A, in B, A only, B only.
    bits: [[u64; 4]; 8],
    len_ratio: Vec<f32>,
}

impl Stats {
    fn add(&mut self, o: &Stats) {
        self.ways_a += o.ways_a;
        self.ways_b += o.ways_b;
        self.matched += o.matched;
        self.same_geom += o.same_geom;
        self.verts += o.verts;
        self.elev_eq += o.elev_eq;
        self.elev_1m += o.elev_1m;
        self.elev_max = self.elev_max.max(o.elev_max);
        self.grade_eq += o.grade_eq;
        for c in 0..13 {
            self.ch_diff[c] += o.ch_diff[c];
        }
        self.ch_n += o.ch_n;
        for b in 0..8 {
            for k in 0..4 {
                self.bits[b][k] += o.bits[b][k];
            }
        }
        self.len_ratio.extend_from_slice(&o.len_ratio);
    }
    fn pct(a: usize, n: usize) -> f64 {
        100.0 * a as f64 / n.max(1) as f64
    }
}

fn compare(pa: &BasePack, pb: &BasePack) -> anyhow::Result<Stats> {
    let (wa, va, ea, ga) = (pa.ways()?, pa.verts()?, pa.elev()?, pa.grade()?);
    let (wb, vb, eb, gb) = (pb.ways()?, pb.verts()?, pb.elev()?, pb.grade()?);
    let (sa, sb) = (pa.scenic(), pb.scenic());
    let (ra, rb) = (pa.road_vals()?, pb.road_vals()?);
    let ib: HashMap<i64, usize> = wb.iter().enumerate().map(|(i, w)| (w.id, i)).collect();
    let mut st = Stats { ways_a: wa.len(), ways_b: wb.len(), ..Default::default() };
    for (i, w) in wa.iter().enumerate() {
        let Some(&j) = ib.get(&w.id) else { continue };
        st.matched += 1;
        let x = &wb[j];
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        let rj = x.vstart as usize..(x.vstart + x.vcount as u64) as usize;
        if va[r.clone()] != vb[rj.clone()] {
            continue;
        }
        st.same_geom += 1;
        for (k, kj) in r.zip(rj) {
            st.verts += 1;
            let d = (ea.m(k) - eb.m(kj)).abs();
            st.elev_eq += (d == 0.0) as usize;
            st.elev_1m += (d <= 1.0) as usize;
            st.elev_max = st.elev_max.max(d);
            st.grade_eq += (ga[k] == gb[kj]) as usize;
            if let (Some(sa), Some(sb)) = (sa, sb) {
                for c in 0..13 {
                    st.ch_diff[c] += (sa[k][c] as i32 - sb[kj][c] as i32).unsigned_abs() as u64;
                }
                st.ch_n += 1;
                let (fa, fb) = (sa[k][7], sb[kj][7]);
                for (b, n) in st.bits.iter_mut().enumerate() {
                    let (p, q) = (fa >> b & 1 == 1, fb >> b & 1 == 1);
                    n[0] += p as u64;
                    n[1] += q as u64;
                    n[2] += (p && !q) as u64;
                    n[3] += (q && !p) as u64;
                }
            }
        }
        if ra[i].len > 0.0 {
            st.len_ratio.push(rb[j].len / ra[i].len);
        }
    }
    Ok(st)
}

fn open(root: &Path, cat: &store::catalog::Catalog, unit: &str) -> anyhow::Result<Option<BasePack>> {
    let (Some(b), Some(r)) = (cat.base.get(unit), cat.roads.get(unit)) else { return Ok(None) };
    let file = |l: &str| cat.files.get(l).map(|f| root.join(&f.file));
    let (Some(bp), Some(rp)) = (file(b), file(r)) else { return Ok(None) };
    Ok(Some(BasePack::open(&bp, &rp)?))
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(&a[1]);
    let read = |n: &str| store::catalog::read(&root.join("catalog").join(store::catalog::file_name(n.parse().expect("catalog number"))));
    let (ca, cb) = (read(&a[2])?, read(&a[3])?);
    let only: Option<Vec<String>> = a.windows(2).find(|w| w[0] == "--units").map(|w| w[1].split(',').map(str::to_string).collect());
    let units: Vec<String> = ca.base.keys().filter(|u| cb.base.contains_key(*u)).filter(|u| only.as_ref().is_none_or(|o| o.contains(u))).cloned().collect();
    eprintln!("{} units in both (a: {}, b: {})", units.len(), ca.base.len(), cb.base.len());
    let mut total = Stats::default();
    println!("{:<10} {:>9} {:>9} {:>7} {:>7} {:>8} {:>8} {:>8}", "unit", "ways a", "ways b", "match%", "geom%", "elev=%", "≤1m%", "grade=%");
    for u in &units {
        let (Some(pa), Some(pb)) = (open(&root, &ca, u)?, open(&root, &cb, u)?) else { continue };
        let st = compare(&pa, &pb)?;
        let flag = if Stats::pct(st.elev_1m, st.verts) < 99.0 || Stats::pct(st.grade_eq, st.verts) < 97.0 { "  <--" } else { "" };
        println!(
            "{:<10} {:>9} {:>9} {:>7.2} {:>7.2} {:>8.2} {:>8.2} {:>8.2}{flag}",
            u,
            st.ways_a,
            st.ways_b,
            Stats::pct(st.matched, st.ways_a),
            Stats::pct(st.same_geom, st.matched),
            Stats::pct(st.elev_eq, st.verts),
            Stats::pct(st.elev_1m, st.verts),
            Stats::pct(st.grade_eq, st.verts)
        );
        total.add(&st);
    }
    let t = &total;
    println!("\nall units: ways a {} b {}; in both {:.2} %; same geometry {:.2} % of those", t.ways_a, t.ways_b, Stats::pct(t.matched, t.ways_a), Stats::pct(t.same_geom, t.matched));
    println!("vertices compared {}: elevation equal {:.3} %, within 1 m {:.3} % (max |Δ| {:.1} m); grade equal {:.3} %", t.verts, Stats::pct(t.elev_eq, t.verts), Stats::pct(t.elev_1m, t.verts), t.elev_max, Stats::pct(t.grade_eq, t.verts));
    if t.ch_n > 0 {
        println!("scenic channels, mean |Δ| per vertex (0–255): {}", t.ch_diff.iter().map(|d| format!("{:.2}", *d as f64 / t.ch_n as f64)).collect::<Vec<_>>().join(" "));
        let names = ["scenic route", "park", "viewpoint", "waterfront", "heritage", "covered bridge", "special area", "indigenous"];
        for (b, n) in t.bits.iter().enumerate() {
            if n.iter().any(|&v| v > 0) {
                println!("  flag {:<14} a {:>6.2} %  b {:>6.2} %  a only {}  b only {}", names[b], 100.0 * n[0] as f64 / t.ch_n as f64, 100.0 * n[1] as f64 / t.ch_n as f64, n[2], n[3]);
            }
        }
    }
    let mut lr = total.len_ratio.clone();
    lr.sort_by(f32::total_cmp);
    let q = |p: f64| lr.get(((lr.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f32::NAN);
    println!("whole-road length b/a over matched ways: p10 {:.2} p50 {:.2} p90 {:.2}", q(0.1), q(0.5), q(0.9));
    Ok(())
}
