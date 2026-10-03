//! Compares two base packs of the same unit, way by way (OSM id): the pilot's against today's
//! converted data (docs/plan.md §10, phase 4).
//!   cargo run --release -p pipeline --example compare_base -- <a.sect> <a-roads.sect> <b.sect> <b-roads.sect> [w,s,e,n]
//! The optional box limits the comparison to ways starting inside it (the pilot's coverage).
use pipeline::basepack::BasePack;
use std::collections::HashMap;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pa = BasePack::open(&PathBuf::from(&a[1]), &PathBuf::from(&a[2]))?;
    let pb = BasePack::open(&PathBuf::from(&a[3]), &PathBuf::from(&a[4]))?;
    let bbox: Option<[i32; 4]> = a.get(5).map(|s| {
        let v: Vec<f64> = s.split(',').map(|x| x.parse().unwrap()).collect();
        [(v[0] * 1e7) as i32, (v[1] * 1e7) as i32, (v[2] * 1e7) as i32, (v[3] * 1e7) as i32]
    });
    let inside = |p: [i32; 2]| bbox.is_none_or(|b| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]);
    let (wa, va, ea, ga) = (pa.ways()?, pa.verts()?, pa.elev()?, pa.grade()?);
    let (wb, vb, eb, gb) = (pb.ways()?, pb.verts()?, pb.elev()?, pb.grade()?);
    let (sa, sb) = (pa.scenic(), pb.scenic());
    let (ra, rb) = (pa.road_vals()?, pb.road_vals()?);
    let ib: HashMap<i64, usize> = wb.iter().enumerate().map(|(i, w)| (w.id, i)).collect();
    let (mut n_a, mut matched, mut same_geom) = (0usize, 0usize, 0usize);
    let (mut verts, mut elev_eq, mut elev_close, mut grade_eq) = (0usize, 0usize, 0usize, 0usize);
    let mut elev_diff: Vec<f32> = Vec::new();
    let mut ch_diff = [0u64; 13];
    let mut ch_n = 0u64;
    // Per flag bit (roadcore::scenic::flag): set in A, set in B, A only, B only.
    let mut bits = [[0u64; 4]; 8];
    let mut road_len_ratio: Vec<f32> = Vec::new();
    for (i, w) in wa.iter().enumerate() {
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        if !inside(va[r.start]) {
            continue;
        }
        n_a += 1;
        let Some(&j) = ib.get(&w.id) else { continue };
        matched += 1;
        let x = &wb[j];
        let rj = x.vstart as usize..(x.vstart + x.vcount as u64) as usize;
        if va[r.clone()] != vb[rj.clone()] {
            continue;
        }
        same_geom += 1;
        for (k, kj) in r.clone().zip(rj.clone()) {
            verts += 1;
            let d = (ea[k] as f32 - eb[kj] as f32).abs() / 10.0;
            elev_eq += (d == 0.0) as usize;
            elev_close += (d <= 1.0) as usize;
            elev_diff.push(d);
            grade_eq += (ga[k] == gb[kj]) as usize;
            if let (Some(sa), Some(sb)) = (sa, sb) {
                for c in 0..13 {
                    ch_diff[c] += (sa[k][c] as i32 - sb[kj][c] as i32).unsigned_abs() as u64;
                }
                let (fa, fb) = (sa[k][7], sb[kj][7]);
                for (b, n) in bits.iter_mut().enumerate() {
                    let (x, y) = (fa >> b & 1 == 1, fb >> b & 1 == 1);
                    n[0] += x as u64;
                    n[1] += y as u64;
                    n[2] += (x && !y) as u64;
                    n[3] += (y && !x) as u64;
                }
                ch_n += 1;
            }
        }
        if ra[i].len > 0.0 {
            road_len_ratio.push(rb[j].len / ra[i].len);
        }
    }
    elev_diff.sort_by(f32::total_cmp);
    road_len_ratio.sort_by(f32::total_cmp);
    let q = |v: &[f32], p: f64| v.get(((v.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f32::NAN);
    println!("ways in A (in the box): {n_a}; in B too: {matched} ({:.2} %); same geometry: {same_geom}", 100.0 * matched as f64 / n_a.max(1) as f64);
    println!("B ways not in A: {}", wb.len().saturating_sub(matched));
    println!(
        "vertices compared: {verts}; elevation equal {:.2} %, within 1 m {:.2} %; |Δ| p50 {:.1} p99 {:.1} max {:.1} m; grade equal {:.2} %",
        100.0 * elev_eq as f64 / verts.max(1) as f64,
        100.0 * elev_close as f64 / verts.max(1) as f64,
        q(&elev_diff, 0.5),
        q(&elev_diff, 0.99),
        elev_diff.last().copied().unwrap_or(0.0),
        100.0 * grade_eq as f64 / verts.max(1) as f64
    );
    if ch_n > 0 {
        println!("scenic channels, mean |Δ| per vertex (0–255): {}", ch_diff.iter().map(|d| format!("{:.1}", *d as f64 / ch_n as f64)).collect::<Vec<_>>().join(" "));
        let names = ["scenic route", "park", "viewpoint", "waterfront", "heritage", "covered bridge", "special area", "indigenous"];
        for (b, n) in bits.iter().enumerate() {
            if n.iter().any(|&v| v > 0) {
                println!("  flag {:<14} A {:>6.2} %  B {:>6.2} %  A only {}  B only {}", names[b], 100.0 * n[0] as f64 / ch_n as f64, 100.0 * n[1] as f64 / ch_n as f64, n[2], n[3]);
            }
        }
    }
    println!("road length B/A: p10 {:.2} p50 {:.2} p90 {:.2}", q(&road_len_ratio, 0.1), q(&road_len_ratio, 0.5), q(&road_len_ratio, 0.9));
    Ok(())
}
