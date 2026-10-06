//! Rule 3's 300 m stage scored on a held-out tenth of the measured heights, as B0 scored the fill
//! (docs/buildings3d.md §2.3): the buildings with a measured height (any source but Microsoft's
//! estimates) whose centroid hashes to 0 mod 10 are held out; every building with a height whose
//! centroid hashes so has it hidden from the neighbours' rule (one with floors keeps its floors'
//! height); each held-out building is estimated by stage 1, stage 2 and rules 4–5 as if
//! unmeasured, and scored under footprint bounds on stage 2. Also the moves: the tiles' own
//! buildings stage 2 fills that a bound sends to rules 4–5. By hand, over work files (bldprep's):
//! WORK=<dir> TILES=6/32/22,… OUTLINES=<outlines.sect> REGIONS=<dir> [BOUNDS=20,40] cargo test
//! --release -p pipeline --lib holdout -- --ignored --nocapture

use super::*;
use crate::agent::recipes;
use crate::outlines::Outlines;
use std::path::Path;

/// One held-out building's estimates (dm) and what it is.
#[derive(Clone, Copy)]
struct Q {
    area: f32,
    truth: u16,
    s1: Option<u16>,
    s2: Option<u16>,
    r45: u16,
    storey: f64,
}

/// A block's held-out buildings, its buildings stage 2 alone fills, and its buildings counted.
type Got = (Vec<Q>, Vec<M>, u64);

/// A building the map's rule 3 reaches only at its 300 m stage: its footprint and rules 4–5's `s`.
#[derive(Clone, Copy)]
struct M {
    area: f32,
    s45: u8,
}

fn held(c: [i32; 2]) -> bool {
    ((c[0] as i64).wrapping_mul(73_856_093) ^ (c[1] as i64).wrapping_mul(19_349_663)).rem_euclid(10) == 0
}

fn files_of(dir: &Path, t: Unit) -> Vec<Option<WorkFile>> {
    let mut out = Vec::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = (t.x as i64 + dx, t.y as i64 + dy);
            let pre = format!("6-{x}-{y}.");
            let f = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(&pre) && p.extension().is_some_and(|e| e == "sect"));
            out.push(f.map(|p| WorkFile::open(&p).unwrap()));
        }
    }
    out
}

// (A block's columns side by side, by record.)
#[allow(clippy::needless_range_loop)]
fn tile(files: &[Option<WorkFile>], cov: &Coverage, t: Unit, qs: &mut Vec<Q>, ms: &mut Vec<M>, own_n: &mut u64) {
    let own = files[4].as_ref().unwrap();
    let mut areas: Vec<(u32, u32)> = own.index.iter().map(|e| {
        let (_, x, y) = key_zxy(e.key);
        (x >> 6, y >> 6)
    }).collect();
    areas.sort_unstable();
    areas.dedup();
    let codes: Vec<Option<Codes>> = files.iter().map(|f| f.as_ref().map(|f| Codes::of(&f.meta))).collect();
    for &a in &areas {
        let (list, n_own) = area_blocks(files, t, a);
        let blocks: Vec<Block> = list.par_iter().map(|(fi, e)| files[*fi].as_ref().unwrap().block(e)).collect::<Result<_>>().unwrap();
        let shapes: Vec<Vec<Option<usize>>> = blocks[..n_own]
            .par_iter()
            .zip(&list[..n_own])
            .map(|(b, (_, e))| match cov.box_shape(box14(e.key)) {
                Some(s) => vec![Some(s); b.len()],
                None => (0..b.len()).map(|i| shape_of(cov, b, i)).collect(),
            })
            .collect();
        // Rules 0–2 for every building read: all of them (the map), and with the held-out heights
        // hidden (the hold-out).
        let mut pts_all = Vec::new();
        let mut pts_ho = Vec::new();
        for (bi, (b, (fi, _))) in blocks.iter().zip(&list).enumerate() {
            let est = codes[*fi].as_ref().unwrap().est;
            for i in 0..b.len() {
                if b.flags[i] & flag::PART != 0 {
                    continue;
                }
                let (h, f) = (b.h[i], b.f[i]);
                let is_est = est != 0 && b.hsrc[i] == est;
                let taken = (fill::H_MIN_DM..=fill::H_MAX_DM).contains(&h);
                let floors = (1..=fill::F_MAX).contains(&f);
                let fl = || {
                    let s = if bi < n_own { shapes[bi][i] } else { shape_of(cov, b, i) };
                    fill::floors_dm(f, fill::storey(country(cov, s)))
                };
                let first = if taken && !is_est { Some(h) } else if floors { Some(fl()) } else if taken { Some(h) } else { None };
                let (wx, wy) = world7(b.cen[i]);
                let p = |h| Point { x: wx * EQ, y: wy * EQ, area: b.area[i], h };
                if let Some(h) = first {
                    pts_all.push(p(h));
                }
                let hidden = taken && held(b.cen[i]);
                let first_ho = if hidden { floors.then(fl) } else { first };
                if let Some(h) = first_ho {
                    pts_ho.push(p(h));
                }
            }
        }
        let bb = tile_box_deg(8, a.0, a.1);
        let cos_min = (bb[1].abs().max(bb[3].abs()) + 0.05).min(85.0).to_radians().dcos();
        let near_all = Near::new(pts_all, cos_min);
        let near_ho = Near::new(pts_ho, cos_min);
        let got: Vec<Got> = blocks[..n_own]
            .par_iter()
            .zip(&list[..n_own])
            .enumerate()
            .map(|(bi, (b, (fi, _)))| {
                let cd = codes[*fi].as_ref().unwrap();
                let (mut s1, mut s2) = (Vec::new(), Vec::new());
                let (mut q, mut m, mut n) = (Vec::new(), Vec::new(), 0u64);
                for i in 0..b.len() {
                    if b.flags[i] & flag::PART != 0 {
                        continue;
                    }
                    let Some(sh) = shapes[bi][i] else { continue };
                    n += 1;
                    let cc = country(cov, Some(sh));
                    let (h, f) = (b.h[i], b.f[i]);
                    let is_est = cd.est != 0 && b.hsrc[i] == cd.est;
                    let taken = (fill::H_MIN_DM..=fill::H_MAX_DM).contains(&h);
                    let class = Meta::name(cd.classes, b.class[i]);
                    let subtype = Meta::name(cd.subtypes, b.subtype[i]);
                    let r45 = fill::last_rules(b.ghsl[i], b.area[i], fill::size_bin(class, subtype, b.area[i]), cc);
                    let (wx, wy) = world7(b.cen[i]);
                    let cos = (b.cen[i][1] as f64 * 1e-7).to_radians().dcos();
                    if taken && !is_est && held(b.cen[i]) {
                        let (a1, a2) = near_ho.stages(wx * EQ, wy * EQ, cos, b.area[i], &mut s1, &mut s2);
                        q.push(Q { area: b.area[i], truth: h, s1: a1, s2: a2, r45: r45.0, storey: fill::storey(cc).0 });
                    }
                    let first = taken || (1..=fill::F_MAX).contains(&f);
                    if !first {
                        let (a1, a2) = near_all.stages(wx * EQ, wy * EQ, cos, b.area[i], &mut s1, &mut s2);
                        if a1.is_none() && a2.is_some() {
                            m.push(M { area: b.area[i], s45: r45.1 });
                        }
                    }
                }
                (q, m, n)
            })
            .collect();
        for (q, m, n) in got {
            qs.extend(q);
            ms.extend(m);
            *own_n += n;
        }
    }
}

fn stats(errs: &mut [f64], over: usize) -> String {
    if errs.is_empty() {
        return "—".into();
    }
    errs.sort_by(f64::total_cmp);
    let q = |p: f64| errs[((errs.len() - 1) as f64 * p).round() as usize];
    format!("{:.2} / {:.2} m, {:.2} %, mean {:.3} ({})", q(0.5), q(0.9), 100.0 * over as f64 / errs.len() as f64, errs.iter().sum::<f64>() / errs.len() as f64, errs.len())
}

#[test]
#[ignore]
fn holdout() {
    let dir = std::env::var("WORK").unwrap();
    let o = Outlines::open(Path::new(&std::env::var("OUTLINES").unwrap())).unwrap();
    let (rs, bad) = recipes::load(Path::new(&std::env::var("REGIONS").unwrap()));
    assert!(bad.is_empty());
    let d = tempfile::tempdir().unwrap();
    let cov = Coverage::from_recipes(&rs, Some(&o), d.path()).unwrap();
    let (mut qs, mut ms, mut n) = (Vec::new(), Vec::new(), 0u64);
    for t in std::env::var("TILES").unwrap().split(',') {
        let t = Unit::parse(t).unwrap();
        let files = files_of(Path::new(&dir), t);
        let (q0, m0) = (qs.len(), ms.len());
        tile(&files, &cov, t, &mut qs, &mut ms, &mut n);
        println!("{}: {} held out, {} filled by stage 2 alone", t.slash(), qs.len() - q0, ms.len() - m0);
    }
    println!("buildings {n}, held out and measured {}, map buildings stage 2 fills {}", qs.len(), ms.len());
    // Each held-out building under a bound: stage 1, else stage 2 if its footprint is the bound or
    // more, else rules 4–5.
    let est = |q: &Q, bound: f32| q.s1.or(if q.area >= bound { q.s2 } else { None }).unwrap_or(q.r45);
    let classes: [(f32, f32, &str); 4] = [(0.0, 30.0, "under 30"), (30.0, 60.0, "30–60"), (60.0, 150.0, "60–150"), (150.0, f32::INFINITY, "150 and over")];
    let fine: [(f32, f32); 11] = [(0.0, 10.0), (10.0, 20.0), (20.0, 30.0), (30.0, 40.0), (40.0, 50.0), (50.0, 60.0), (60.0, 80.0), (80.0, 100.0), (100.0, 150.0), (150.0, 300.0), (300.0, f32::INFINITY)];
    let score = |sel: &dyn Fn(&Q) -> bool, f: &dyn Fn(&Q) -> u16| {
        let mut e = Vec::new();
        let mut over = 0;
        for q in qs.iter().filter(|q| sel(q)) {
            let d = (f(q) as f64 - q.truth as f64).abs() / 10.0;
            if d > 2.0 * q.storey {
                over += 1;
            }
            e.push(d);
        }
        stats(&mut e, over)
    };
    println!("\nWHERE STAGE 1 FAILS AND STAGE 2 ANSWERS (median / p90 |error|, share > 2 storeys, n):");
    println!("footprint m²\tstage 2 (300 m)\trules 4–5");
    for (lo, hi) in fine {
        let sel = |q: &Q| q.s1.is_none() && q.s2.is_some() && q.area >= lo && q.area < hi;
        println!("{lo}–{hi}\t{}\t{}", score(&sel, &|q: &Q| q.s2.unwrap()), score(&sel, &|q: &Q| q.r45));
    }
    let mut bounds = vec![0.0f32, 30.0, 60.0];
    if let Ok(more) = std::env::var("BOUNDS") {
        bounds.extend(more.split(',').map(|b| b.parse::<f32>().unwrap()));
    }
    println!("\nALL HELD-OUT BUILDINGS BY CLASS, THE WHOLE CHAIN (stage 1, stage 2 over the bound, rules 4–5):");
    print!("class");
    for b in &bounds {
        print!("\tbound {b}");
    }
    println!();
    for (lo, hi, name) in classes {
        print!("{name}");
        for &b in &bounds {
            print!("\t{}", score(&|q: &Q| q.area >= lo && q.area < hi, &|q: &Q| est(q, b)));
        }
        println!();
    }
    println!("\nMOVES: the map's buildings stage 2 fills ({}), sent to rules 4–5 by each bound:", ms.len());
    for &b in &bounds {
        let moved: Vec<&M> = ms.iter().filter(|m| m.area < b).collect();
        let to4 = moved.iter().filter(|m| m.s45 == fill::src::GHSL).count();
        println!("bound {b}: {} ({:.1} % of the {n} buildings): {} to rule 5, {} to rule 4", moved.len(), 100.0 * moved.len() as f64 / n as f64, moved.len() - to4, to4);
    }
}
