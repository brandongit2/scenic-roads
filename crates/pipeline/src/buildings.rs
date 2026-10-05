//! Roadside buildings: how much of a road's frontage is lined with buildings, per road sample
//! (→ samples.bld.u8, merged into `ch::BLDG` by `view::flags`).
//!
//! Buildings are Overture footprints' bounding boxes: every `.f32` in the folder given (a unit's:
//! the release's tiles near its roads, `buildtiles::stage`; today's build: data/buildings, from
//! `dem/buildings.py`), f32 [xmin, ymin, xmax, ymax] per building; heights are not used. For each sample, points every
//! 5 m along the road within ±50 m are checked on each side: a building whose extent along the
//! road covers the point counts fully if its near edge is within 30 m of the centreline, fading
//! to nothing at 80 m. The sample's value is the mean over points and both sides, so a village
//! street of joined houses scores near 1, a farm by the road a little, open country 0. Sheds (under
//! 15 m²) are ignored; rail and ferries get 0.

use det::Det;
use crate::count_bar;
use anyhow::{Context, Result};
use rayon::prelude::*;
use roadcore::scenic::{sflag, Sample};
use roadcore::{class, dist_m, Array, Ways, E7};
use std::path::Path;

const CELL: f64 = 0.0015; // degrees (~165 m north–south)
const IDX_BITS: u32 = 28;
const STEP_M: f64 = 5.0;
const HALF_M: f64 = 50.0;
const FULL_M: f64 = 30.0;
const ZERO_M: f64 = 80.0;
const MIN_AREA_M2: f64 = 15.0;

fn cell_key(ix: i64, iy: i64) -> u64 {
    let x = (ix + 131_072) as u64 & 0x3_ffff; // 18 bits: 240,000 cells around the world
    let y = (iy + 65_536) as u64 & 0x1_ffff; // 17 bits
    (x << 17 | y) << IDX_BITS
}

struct Index {
    boxes: Vec<[f32; 4]>,
    /// cell key | building index, sorted.
    keys: Vec<u64>,
}

impl Index {
    fn load(bdir: &Path, near_road: &(dyn Fn(i64, i64) -> bool + Sync)) -> Result<Index> {
        let mut boxes: Vec<[f32; 4]> = Vec::new();
        let mut files: Vec<_> = std::fs::read_dir(bdir).with_context(|| format!("{bdir:?}"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "f32"))
            .collect();
        files.sort();
        for f in &files {
            let a = Array::<[f32; 4]>::open(f)?;
            let before = boxes.len();
            boxes.par_extend(a.get().par_iter().filter(|b| {
                let lat = ((b[1] + b[3]) * 0.5) as f64;
                let w = (b[2] - b[0]) as f64 * 111_320.0 * lat.to_radians().dcos();
                let h = (b[3] - b[1]) as f64 * 110_540.0;
                if w * h < MIN_AREA_M2 || w > 1500.0 || h > 1500.0 {
                    return false;
                }
                // Kept when a cell its box meets is near a road sample (so a building counts the
                // same whichever other roads the unit's folder holds).
                let (x0, x1) = ((b[0] as f64 / CELL).floor() as i64, (b[2] as f64 / CELL).floor() as i64);
                let (y0, y1) = ((b[1] as f64 / CELL).floor() as i64, (b[3] as f64 / CELL).floor() as i64);
                (x0..=x1).any(|x| (y0..=y1).any(|y| near_road(x, y)))
            }));
            eprintln!("buildings: {} near roads from {}", boxes.len() - before, f.file_name().unwrap().to_string_lossy());
        }
        anyhow::ensure!(boxes.len() < 1 << IDX_BITS, "too many buildings for the index");
        // Each building in every cell its box overlaps.
        let mut keys: Vec<u64> = boxes
            .par_iter()
            .enumerate()
            .flat_map_iter(|(i, b)| {
                let (x0, x1) = ((b[0] as f64 / CELL).floor() as i64, (b[2] as f64 / CELL).floor() as i64);
                let (y0, y1) = ((b[1] as f64 / CELL).floor() as i64, (b[3] as f64 / CELL).floor() as i64);
                (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| cell_key(x, y) | i as u64))
            })
            .collect();
        keys.par_sort_unstable();
        Ok(Index { boxes, keys })
    }

    fn cell(&self, ix: i64, iy: i64, out: &mut Vec<u32>) {
        let k = cell_key(ix, iy);
        let a = self.keys.partition_point(|&v| v < k);
        let b = self.keys.partition_point(|&v| v < k + (1 << IDX_BITS));
        out.extend(self.keys[a..b].iter().map(|v| (v & ((1 << IDX_BITS) - 1)) as u32));
    }
}

/// How far from a sample a building can count: the stretch's half-length, the fade's end, and a
/// margin.
const REACH_M: f64 = HALF_M + ZERO_M + 5.0;

/// The cells (x0, x1, y0, y1) within `REACH_M` of a sample at (lon, lat): where its buildings are
/// looked up.
fn reach_cells(lon: f64, lat: f64) -> (i64, i64, i64, i64) {
    let (kx, ky) = (111_320.0 * lat.to_radians().dcos(), 110_540.0);
    (
        ((lon - REACH_M / kx) / CELL).floor() as i64,
        ((lon + REACH_M / kx) / CELL).floor() as i64,
        ((lat - REACH_M / ky) / CELL).floor() as i64,
        ((lat + REACH_M / ky) / CELL).floor() as i64,
    )
}

fn weight(d: f64) -> f64 {
    if d <= FULL_M {
        1.0
    } else if d >= ZERO_M {
        0.0
    } else {
        (ZERO_M - d) / (ZERO_M - FULL_M)
    }
}

pub fn run(dir: &Path, bdir: &Path) -> Result<()> {
    let samples_a = Array::<Sample>::open(&dir.join("samples.bin"))?;
    let samples = samples_a.get();
    let wv = Ways::open(dir)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let is_road = |w: u32| {
        let c = ways[w as usize].class;
        c < class::TRAM && c != class::FERRY
    };
    // The cells each road sample's buildings are looked up in (`reach_cells`), so a building in
    // none of them, which no sample could count, isn't kept.
    let mut near: Vec<u64> = samples
        .par_iter()
        .filter(|s| is_road(s.way))
        .flat_map_iter(|s| {
            let (x0, x1, y0, y1) = reach_cells(s.lon as f64 * E7, s.lat as f64 * E7);
            (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| cell_key(x, y)))
        })
        .collect();
    near.par_sort_unstable();
    near.dedup();
    eprintln!("buildings: {} road cells", near.len());
    let idx = Index::load(bdir, &|ix, iy| near.binary_search(&cell_key(ix, iy)).is_ok())?;
    drop(near);
    eprintln!("buildings: {} kept, {} cell entries", idx.boxes.len(), idx.keys.len());

    // Sample ranges per way (samples are sorted by way).
    let mut ranges: Vec<(u32, usize, usize)> = Vec::new();
    let mut k = 0;
    while k < samples.len() {
        let w = samples[k].way;
        let a = k;
        while k < samples.len() && samples[k].way == w {
            k += 1;
        }
        ranges.push((w, a, k));
    }
    let pb = count_bar(ranges.len() as u64, "roadside buildings");
    let parts: Vec<(usize, Vec<u8>)> = ranges
        .par_iter()
        .map(|&(w, a, b)| {
            pb.inc(1);
            if !is_road(w) {
                return (a, vec![0u8; b - a]);
            }
            let wr = &ways[w as usize];
            let v = &verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize];
            let ll: Vec<(f64, f64)> = v.iter().map(|p| (p[0] as f64 * E7, p[1] as f64 * E7)).collect();
            let mut cum = vec![0f64; ll.len()];
            for j in 1..ll.len() {
                cum[j] = cum[j - 1] + dist_m(ll[j - 1].0, ll[j - 1].1, ll[j].0, ll[j].1);
            }
            let total = *cum.last().unwrap_or(&0.0);
            let mut cand: Vec<u32> = Vec::new();
            let mut local: Vec<[f64; 4]> = Vec::new();
            let out = samples[a..b]
                .iter()
                .map(|s| {
                    if s.flags & sflag::TUNNEL != 0 || ll.len() < 2 {
                        return 0u8;
                    }
                    let (plon, plat) = (s.lon as f64 * E7, s.lat as f64 * E7);
                    let kx = 111_320.0 * plat.to_radians().dcos();
                    let ky = 110_540.0;
                    // Candidate buildings within reach of the stretch, in metres around the sample.
                    let reach = REACH_M;
                    let (x0, x1, y0, y1) = reach_cells(plon, plat);
                    cand.clear();
                    for x in x0..=x1 {
                        for y in y0..=y1 {
                            idx.cell(x, y, &mut cand);
                        }
                    }
                    cand.sort_unstable();
                    cand.dedup();
                    local.clear();
                    for &i in &cand {
                        let bx = idx.boxes[i as usize];
                        let r = [
                            (bx[0] as f64 - plon) * kx,
                            (bx[1] as f64 - plat) * ky,
                            (bx[2] as f64 - plon) * kx,
                            (bx[3] as f64 - plat) * ky,
                        ];
                        // Nearest point of the box to the sample.
                        let dx = r[0].max(0.0).max(-r[2]);
                        let dy = r[1].max(0.0).max(-r[3]);
                        if dx * dx + dy * dy <= reach * reach {
                            local.push(r);
                        }
                    }
                    if local.is_empty() {
                        return 0u8;
                    }
                    // Points along the road.
                    let mut acc = 0.0;
                    let mut n = 0usize;
                    let mut j = 0usize;
                    let mut o = -HALF_M;
                    while o <= HALF_M + 1e-6 {
                        let at = s.dist as f64 + o;
                        o += STEP_M;
                        if at < 0.0 || at > total {
                            continue;
                        }
                        while j + 2 < cum.len() && cum[j + 1] < at {
                            j += 1;
                        }
                        while j > 0 && cum[j] > at {
                            j -= 1;
                        }
                        let seg = (cum[j + 1] - cum[j]).max(1e-6);
                        let t = ((at - cum[j]) / seg).clamp(0.0, 1.0);
                        let (ax, ay) = ((ll[j].0 - plon) * kx, (ll[j].1 - plat) * ky);
                        let (bx, by) = ((ll[j + 1].0 - plon) * kx, (ll[j + 1].1 - plat) * ky);
                        let (qx, qy) = (ax + (bx - ax) * t, ay + (by - ay) * t);
                        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                        if len < 1e-3 {
                            continue;
                        }
                        let (tx, ty) = ((bx - ax) / len, (by - ay) / len);
                        let (nx, ny) = (-ty, tx);
                        let (mut left, mut right) = (0f64, 0f64);
                        for r in &local {
                            let (mut umin, mut umax, mut vmin, mut vmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
                            for (cx, cy) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
                                let (dx, dy) = (cx - qx, cy - qy);
                                let u = dx * tx + dy * ty;
                                let v = dx * nx + dy * ny;
                                umin = umin.min(u);
                                umax = umax.max(u);
                                vmin = vmin.min(v);
                                vmax = vmax.max(v);
                            }
                            if umax < -STEP_M * 0.5 || umin > STEP_M * 0.5 {
                                continue;
                            }
                            if vmin > 0.0 {
                                left = left.max(weight(vmin));
                            } else if vmax < 0.0 {
                                right = right.max(weight(-vmax));
                            } else if vmin + vmax > 0.0 {
                                left = 1.0;
                            } else {
                                right = 1.0;
                            }
                        }
                        acc += (left + right) * 0.5;
                        n += 1;
                    }
                    if n == 0 { 0 } else { ((acc / n as f64) * 255.0).round().clamp(0.0, 255.0) as u8 }
                })
                .collect();
            (a, out)
        })
        .collect();
    pb.finish_and_clear();
    let mut bld = vec![0u8; samples.len()];
    for (a, v) in parts {
        bld[a..a + v.len()].copy_from_slice(&v);
    }
    std::fs::write(roadcore::tmp(dir, "samples.bld.u8"), &bld)?;
    roadcore::commit(dir, &["samples.bld.u8"])?;
    let roads: Vec<u8> = samples.iter().zip(&bld).filter(|(s, _)| is_road(s.way)).map(|(_, &b)| b).collect();
    let share = |t: u8| roads.iter().filter(|&&b| b >= t).count() as f64 / roads.len().max(1) as f64 * 100.0;
    eprintln!(
        "buildings: road samples with any roadside buildings {:.1} %, a quarter lined {:.1} %, half {:.1} %, mostly {:.1} %",
        share(1),
        share(64),
        share(128),
        share(192)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_building_within_reach_is_kept_wherever_its_centre_is() {
        // A long building at 60° N: its west end 40 m east of a road sample, its centre 640 m east
        // (several cells away). It's in the sample's lookup cells, so kept.
        let d = tempfile::tempdir().unwrap();
        let (lon, lat) = (10.0, 60.0);
        let m = 111_320.0 * f64::to_radians(lat).dcos();
        let b = [(lon + 40.0 / m) as f32, (lat - 0.0002) as f32, (lon + 1240.0 / m) as f32, (lat + 0.0002) as f32];
        std::fs::write(d.path().join("a.f32"), bytemuck::cast_slice::<[f32; 4], u8>(&[b])).unwrap();
        let (x0, x1, y0, y1) = reach_cells(lon, lat);
        let mut near: Vec<u64> = (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| cell_key(x, y))).collect();
        near.sort_unstable();
        let idx = Index::load(d.path(), &|x, y| near.binary_search(&cell_key(x, y)).is_ok()).unwrap();
        assert_eq!(idx.boxes.len(), 1);
        let cx = (((b[0] + b[2]) * 0.5) as f64 / CELL).floor() as i64;
        assert!(cx - (lon / CELL).floor() as i64 > 1, "its centre is beyond the sample's neighbouring cells");
        // Beyond reach (its west end 200 m away): not kept.
        let far = [(lon + 200.0 / m) as f32, b[1], (lon + 1400.0 / m) as f32, b[3]];
        std::fs::write(d.path().join("a.f32"), bytemuck::cast_slice::<[f32; 4], u8>(&[far])).unwrap();
        assert_eq!(Index::load(d.path(), &|x, y| near.binary_search(&cell_key(x, y)).is_ok()).unwrap().boxes.len(), 0);
    }
}
