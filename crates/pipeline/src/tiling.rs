//! Cutting road and rail lines into RT tiles (`roadcore::tile`): per zoom, a combined planar +
//! elevation Douglas-Peucker, then an exact clip at tile edges (round caps make the seams
//! invisible), then per tile the sub-pixel pieces merged into dots and the lines encoded.
//!
//! Used by `scenic-build pack`/`lo` (the ways of one area, gathered from base packs), which
//! describes each way with a `WayIn` and chooses which tiles to keep.

use det::Det;
use crate::count_bar;
use flate2::{write::GzEncoder, Compression};
use rayon::prelude::*;
use roadcore::elev::{self, Elevs};
use roadcore::scenic::ch;
use roadcore::tile::{encode, lflag, style, surface, TileLine, NCH};
use roadcore::{class, dist_m, flag, merc, WayRec, E7};
use std::collections::HashMap;
use std::io::Write;

/// Douglas-Peucker planar tolerance in screen pixels (256 px tiles).
const DP_TOL_PX: f64 = 0.35;

pub fn extent_log2(z: u8) -> u8 {
    if z >= 14 { 13 } else { 12 }
}

/// Elevation tolerance (m) for simplification at zoom z: 1 m at z14, doubling every 2 zooms out.
fn elev_tol(z: u8) -> f64 {
    2f64.dpowf((14.0 - z as f64) / 2.0).max(1.0)
}

pub fn cumdist(v: &[[i32; 2]]) -> Vec<f64> {
    let mut d = Vec::with_capacity(v.len());
    let mut acc = 0.0;
    d.push(0.0);
    for s in v.windows(2) {
        acc += dist_m(s[0][0] as f64 * E7, s[0][1] as f64 * E7, s[1][0] as f64 * E7, s[1][1] as f64 * E7);
        d.push(acc);
    }
    d
}

/// Douglas-Peucker on (x, y) with an additional elevation criterion. Returns kept indices.
fn simplify(xy: &[[f64; 2]], e: &[f32], tol: f64, etol: f64) -> Vec<usize> {
    let n = xy.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay) = (xy[a][0], xy[a][1]);
        let (dx, dy) = (xy[b][0] - ax, xy[b][1] - ay);
        let l2 = dx * dx + dy * dy;
        // Elevation is interpolated by position along the chord.
        let (mut best, mut bi) = (0f64, 0usize);
        for i in a + 1..b {
            let (px, py) = (xy[i][0] - ax, xy[i][1] - ay);
            let (dev, t) = if l2 > 0.0 {
                let t = ((px * dx + py * dy) / l2).clamp(0.0, 1.0);
                let (qx, qy) = (px - t * dx, py - t * dy);
                ((qx * qx + qy * qy).sqrt(), t)
            } else {
                ((px * px + py * py).sqrt(), (i - a) as f64 / (b - a) as f64)
            };
            let ei = e[a] as f64 + (e[b] as f64 - e[a] as f64) * t;
            let score = (dev / tol).max((e[i] as f64 - ei).abs() / etol);
            if score > best {
                best = score;
                bi = i;
            }
        }
        if best > 1.0 {
            keep[bi] = true;
            stack.push((a, bi));
            stack.push((bi, b));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

#[derive(Clone, Copy)]
struct P {
    x: f64,
    y: f64,
    e: f32,
    g: f32,
    /// Cumulative full-resolution distance along the way, metres.
    cd: f64,
    h: f32,
    sc: [f32; NCH],
}

fn lerp(a: &P, b: &P, t: f64) -> P {
    let tf = t as f32;
    P {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        e: a.e + (b.e - a.e) * tf,
        g: a.g + (b.g - a.g) * tf,
        cd: a.cd + (b.cd - a.cd) * t,
        h: a.h + (b.h - a.h) * tf,
        sc: {
            let mut o = [0f32; NCH];
            for (k, v) in o.iter_mut().enumerate() {
                *v = if k == ch::FLAGS { if t < 0.5 { a.sc[k] } else { b.sc[k] } } else { a.sc[k] + (b.sc[k] - a.sc[k]) * tf };
            }
            o
        },
    }
}

/// Split a polyline (world tile units) at tile boundaries; emits (tile x, tile y, points).
fn clip_to_grid(pts: &[P], ext: f64, mut emit: impl FnMut(u32, u32, Vec<P>)) {
    let tile_of = |p: &P| ((p.x / ext).floor() as i64, (p.y / ext).floor() as i64);
    let mid_tile = |a: &P, b: &P| (((a.x + b.x) * 0.5 / ext).floor() as i64, ((a.y + b.y) * 0.5 / ext).floor() as i64);
    let mut cur: Vec<P> = vec![pts[0]];
    let mut cur_tile = if pts.len() > 1 { mid_tile(&pts[0], &pts[1]) } else { tile_of(&pts[0]) };
    for s in pts.windows(2) {
        let (a, b) = (&s[0], &s[1]);
        // Parameters where the segment crosses vertical / horizontal grid lines.
        let mut ts: Vec<f64> = Vec::new();
        for (a0, b0) in [(a.x, b.x), (a.y, b.y)] {
            if a0 == b0 {
                continue;
            }
            let (lo, hi) = (a0.min(b0), a0.max(b0));
            let mut k = (lo / ext).floor() + 1.0;
            while k * ext < hi {
                ts.push((k * ext - a0) / (b0 - a0));
                k += 1.0;
            }
        }
        ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let mut prev_t = 0.0;
        let mut prev_p = *a;
        for &t in ts.iter().chain(std::iter::once(&1.0)) {
            if t - prev_t < 1e-12 {
                continue;
            }
            let p = if t >= 1.0 { *b } else { lerp(a, b, t) };
            let tile = mid_tile(&prev_p, &p);
            if tile != cur_tile {
                if cur.len() >= 2 && cur_tile.0 >= 0 && cur_tile.1 >= 0 {
                    emit(cur_tile.0 as u32, cur_tile.1 as u32, std::mem::take(&mut cur));
                }
                cur.clear();
                cur.push(prev_p);
                cur_tile = tile;
            }
            cur.push(p);
            prev_t = t;
            prev_p = p;
        }
    }
    if cur.len() >= 2 && cur_tile.0 >= 0 && cur_tile.1 >= 0 {
        emit(cur_tile.0 as u32, cur_tile.1 as u32, cur);
    } else if pts.len() == 1 {
        emit(cur_tile.0 as u32, cur_tile.1 as u32, vec![pts[0], pts[0]]);
    }
}

pub fn style_of(w: &WayRec) -> u8 {
    let mut s = w.class & 0x0f;
    if w.flags & flag::UNPAVED != 0 {
        s |= style::UNPAVED;
    }
    if w.flags & flag::BRIDGE != 0 {
        s |= style::BRIDGE;
    }
    if w.flags & flag::TUNNEL != 0 {
        s |= style::TUNNEL;
    }
    if w.flags & flag::LINK != 0 {
        s |= style::LINK;
    }
    s
}

/// Draw order: ferries & tunnels, then roads minor → major, then bridges minor → major.
pub fn draw_key(style: u8) -> u8 {
    let c = style & 0x0f;
    let group = if c == class::FERRY || style & style::TUNNEL != 0 {
        0
    } else if style & style::BRIDGE != 0 {
        2
    } else {
        1
    };
    group * 16 + c
}

/// One way as the tiler sees it: its record, the id written into the tiles' way column, and its
/// per-vertex data (all slices the same length as `verts`).
pub struct WayIn<'a> {
    pub rec: &'a WayRec,
    /// The way column: the OSM way id (RT v7).
    pub id: u32,
    pub verts: &'a [[i32; 2]],
    /// Processed elevation (`roadcore::elev`).
    pub elev_dm: Elevs<'a>,
    pub grade: &'a [u8],
    /// Drape height (m) for 3D, when the scenic analysis has run.
    pub drape: Option<&'a [i16]>,
    /// Scenic channels, when the scenic analysis has run.
    pub sc: Option<&'a [[u8; NCH]]>,
    pub surface: &'a str,
    /// Length of the whole road the way belongs to, metres.
    pub road_m: u32,
}

/// Simplify and clip every way at zoom `z`, keeping the pieces of tiles `keep(x, y)` accepts.
/// Returns (tile key, line) pieces sorted by tile, draw order, then way id.
pub fn cut(z: u8, ways: &[WayIn], keep: &(dyn Fn(u32, u32) -> bool + Sync), progress: bool) -> Vec<(u64, TileLine)> {
    let el2 = extent_log2(z);
    let ext = (1u64 << el2) as f64;
    let scale = (1u64 << z) as f64 * ext;
    let tol = DP_TOL_PX * ext / 256.0;
    let etol = elev_tol(z);
    let pb = progress.then(|| count_bar(ways.len() as u64, format!("z{z} simplify + clip")));
    // (Each piece with its place along its way: the pieces come from the threads in any order, and
    // two of one way in one tile may start at the same point.)
    let mut pieces: Vec<(u64, u32, TileLine)> = ways
        .par_iter()
        .fold(Vec::new, |mut acc, w| {
            let r = w.rec;
            let xy: Vec<[f64; 2]> = w
                .verts
                .iter()
                .map(|v| {
                    let (x, y) = merc(v[0] as f64 * E7, v[1] as f64 * E7);
                    [x * scale, y * scale]
                })
                .collect();
            let e: Vec<f32> = (0..w.elev_dm.len()).map(|i| w.elev_dm.m(i)).collect();
            let kept = simplify(&xy, &e, tol, etol);
            let cd = cumdist(w.verts);
            let pts: Vec<P> = kept
                .iter()
                .map(|&k| P {
                    x: xy[k][0],
                    y: xy[k][1],
                    e: e[k],
                    g: w.grade[k] as f32,
                    cd: cd[k],
                    h: w.drape.map_or(e[k], |d| d[k] as f32),
                    sc: w.sc.map_or_else(
                        || {
                            let mut z = [0f32; NCH];
                            z[ch::TPI] = 128.0;
                            z
                        },
                        |a| a[k].map(|v| v as f32),
                    ),
                })
                .collect();
            let st = style_of(r);
            let mut lf = if r.name == 0 && r.ref_ == 0 { lflag::UNNAMED } else { 0 };
            if class::is_rail(r.class) {
                lf |= r.rail << lflag::RAIL_SHIFT;
            } else {
                if r.flags & flag::ONEWAY != 0 {
                    lf |= lflag::ONEWAY;
                }
                if r.flags & flag::TOLL != 0 {
                    lf |= lflag::TOLL;
                }
            }
            let attr = [r.network, (r.maxspeed / 2).min(255) as u8, r.lanes, surface::code(w.surface)];
            let colour = if r.colour != 0 { (r.colour & 0xff_ffff) + 1 } else { 0 };
            let mut seq = 0u32;
            clip_to_grid(&pts, ext, |tx, ty, piece| {
                seq += 1;
                if !keep(tx, ty) {
                    return;
                }
                let (ox, oy) = (tx as f64 * ext, ty as f64 * ext);
                let true_len = piece.last().unwrap().cd - piece[0].cd;
                let mut tl = TileLine {
                    style: st,
                    flags: lf,
                    way: w.id,
                    true_len_dm: (true_len * 10.0).round().max(0.0) as u32,
                    road_m: w.road_m,
                    attr,
                    colour,
                    ..Default::default()
                };
                for q in &piece {
                    let pt = [(q.x - ox).round() as i32, (q.y - oy).round() as i32];
                    if tl.pts.last() == Some(&pt) {
                        continue;
                    }
                    tl.pts.push(pt);
                    tl.elev.push(((q.e * 10.0).round() as i32).clamp(elev::MIN_DM, elev::MAX_DM));
                    tl.grade.push(q.g.round().clamp(0.0, 255.0) as u8);
                    tl.drape.push(q.h.round().clamp(-500.0, 9000.0) as i16);
                    tl.sc.push(q.sc.map(|v| v.round().clamp(0.0, 255.0) as u8));
                }
                if tl.pts.len() == 1 {
                    // Sub-unit feature: keep as a dot (degenerate segment).
                    tl.pts.push(tl.pts[0]);
                    tl.elev.push(tl.elev[0]);
                    tl.grade.push(tl.grade[0]);
                    tl.drape.push(tl.drape[0]);
                    tl.sc.push(tl.sc[0]);
                }
                acc.push((roadcore::archive::tile_key(z, tx, ty), seq, tl));
            });
            if let Some(pb) = &pb {
                pb.inc(1);
            }
            acc
        })
        .reduce(Vec::new, |mut a, mut b| {
            if a.len() < b.len() {
                std::mem::swap(&mut a, &mut b);
            }
            a.append(&mut b);
            a
        });
    if let Some(pb) = pb {
        pb.finish_and_clear();
    }
    sort_pieces(&mut pieces);
    pieces.into_iter().map(|(k, _, t)| (k, t)).collect()
}

/// `cut`'s pieces (tile key, place along the way, line) in order: by tile, draw order, way, then
/// along the way by where each starts (two starting at the same point: by their place), whatever
/// order the threads gave them in.
fn sort_pieces(pieces: &mut [(u64, u32, TileLine)]) {
    pieces.par_sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then(draw_key(a.2.style).cmp(&draw_key(b.2.style)))
            .then(a.2.way.cmp(&b.2.way))
            // Pieces of one way in one tile: in order along it.
            .then(a.2.pts.first().cmp(&b.2.pts.first()))
            .then(a.1.cmp(&b.1))
    });
}

/// A sub-pixel feature merged into a dot.
struct Dot {
    len: f64,
    e: f64,
    g: f64,
    h: f64,
    ch: [f64; NCH],
    flags: u8,
    /// The longest piece: its way, length and per-line data.
    best: u32,
    best_len: u32,
    road_m: u32,
    attr: [u8; 4],
    colour: u32,
}

/// One encoded tile: key, gzip'd RT bytes, raw length, vertex count.
pub struct Encoded {
    pub key: u64,
    pub gz: Vec<u8>,
    pub raw_len: usize,
    pub nverts: usize,
}

/// Encode one zoom level's pieces (sorted by tile, draw order, way; see `cut`): per tile, the
/// sub-pixel pieces merged into dots (below the max zoom), then RT-encoded and gzip'd.
pub fn encode_zoom(z: u8, maxz: u8, pieces: &[(u64, TileLine)]) -> Vec<Encoded> {
    let el2 = extent_log2(z);
    let ext = (1u64 << el2) as f64;
    // Group by tile.
    let mut groups: Vec<(u64, usize, usize)> = Vec::new();
    let mut s = 0;
    for i in 1..=pieces.len() {
        if i == pieces.len() || pieces[i].0 != pieces[s].0 {
            groups.push((pieces[s].0, s, i));
            s = i;
        }
    }
    let encoded: Vec<Encoded> = groups
        .par_iter()
        .map(|&(key, s, e)| {
            // Features shorter than half a pixel are indistinguishable from dots. Below the max
            // zoom (which is overzoomed, so must keep true geometry) collapse them onto a
            // half-pixel grid, merged per style, keeping the summed true length and
            // length-weighted elevation / grade. Dots are kept apart by road length (one bin per
            // decade: finer bins cost ~1.3× more at z4–6), so the length filter roughly applies.
            let collapse = z < maxz;
            let half_px = ext / 512.0;
            let mut owned: Vec<TileLine> = Vec::with_capacity(e - s);
            // (cell, style, line flags, road length bin) -> merged dot
            let mut dots: HashMap<([i32; 2], u8, u8, u8), Dot> = HashMap::new();
            for p in &pieces[s..e] {
                let l = &p.1;
                let len: f64 = l
                    .pts
                    .windows(2)
                    .map(|w| (((w[1][0] - w[0][0]) as f64).powi(2) + ((w[1][1] - w[0][1]) as f64).powi(2)).sqrt())
                    .sum();
                if !(collapse && len < half_px) && len != 0.0 {
                    owned.push(l.clone());
                    continue;
                }
                let n = l.pts.len() as f64;
                let (mx, my) = l.pts.iter().fold((0.0, 0.0), |a, q| (a.0 + q[0] as f64 / n, a.1 + q[1] as f64 / n));
                let cell = if collapse {
                    [(mx / half_px).floor() as i32, (my / half_px).floor() as i32]
                } else {
                    [mx.round() as i32, my.round() as i32]
                };
                let em = l.elev.iter().map(|&v| v as f64).sum::<f64>() / n;
                let gm = l.grade.iter().map(|&v| v as f64).sum::<f64>() / n;
                let w = (l.true_len_dm.max(1)) as f64;
                let hm = l.drape.iter().map(|&v| v as f64).sum::<f64>() / n;
                let lbin = (l.road_m.max(1) as f64).dlog10() as u8;
                let d = dots.entry((cell, l.style, l.flags, lbin)).or_insert(Dot {
                    len: 0.0,
                    e: 0.0,
                    g: 0.0,
                    h: 0.0,
                    ch: [0.0; NCH],
                    flags: 0,
                    best: l.way,
                    best_len: 0,
                    road_m: l.road_m,
                    attr: l.attr,
                    colour: l.colour,
                });
                d.len += w;
                d.e += em * w;
                d.g += gm * w;
                d.h += hm * w;
                for c in 0..NCH {
                    d.ch[c] += l.sc.iter().map(|v| v[c] as f64).sum::<f64>() / n * w;
                }
                d.flags |= l.sc.iter().fold(0u8, |a, v| a | v[ch::FLAGS]);
                // The longest piece names the dot; ties go to the lower way id, so the result
                // doesn't depend on the order pieces arrive in.
                if l.true_len_dm > d.best_len || (l.true_len_dm == d.best_len && l.way < d.best) {
                    d.best = l.way;
                    d.best_len = l.true_len_dm;
                    d.road_m = l.road_m;
                    d.attr = l.attr;
                    d.colour = l.colour;
                }
            }
            // Dots in a fixed order (the map's iteration order is random).
            let mut dots: Vec<(([i32; 2], u8, u8, u8), Dot)> = dots.into_iter().collect();
            dots.sort_unstable_by_key(|(k, _)| *k);
            for ((cell, style, lflags, _), d) in dots {
                let pt = if collapse {
                    [((cell[0] as f64 + 0.5) * half_px).round() as i32, ((cell[1] as f64 + 0.5) * half_px).round() as i32]
                } else {
                    cell
                };
                let sw = d.len;
                let ev = (d.e / sw).round() as i32;
                let gv = (d.g / sw).round().clamp(0.0, 255.0) as u8;
                let hv = (d.h / sw).round().clamp(-500.0, 9000.0) as i16;
                let mut cv = [0u8; NCH];
                for c in 0..NCH {
                    cv[c] = if c == ch::FLAGS { d.flags } else { (d.ch[c] / sw).round().clamp(0.0, 255.0) as u8 };
                }
                owned.push(TileLine {
                    style,
                    flags: lflags,
                    way: d.best,
                    true_len_dm: sw.round().min(u32::MAX as f64) as u32,
                    road_m: d.road_m,
                    attr: d.attr,
                    colour: d.colour,
                    pts: vec![pt, pt],
                    elev: vec![ev, ev],
                    grade: vec![gv, gv],
                    drape: vec![hv, hv],
                    sc: vec![cv, cv],
                });
            }
            // Stable: pieces of one way keep their order along it.
            owned.sort_by_key(|l| (draw_key(l.style), l.way));
            let nverts: usize = owned.iter().map(|l| l.pts.len()).sum();
            let raw = encode(&owned, el2);
            let mut gz = GzEncoder::new(Vec::with_capacity(raw.len() / 2), Compression::new(6));
            gz.write_all(&raw).unwrap();
            Encoded { key, gz: gz.finish().unwrap(), raw_len: raw.len(), nverts }
        })
        .collect();
    encoded
}


#[cfg(test)]
mod order_tests {
    use super::*;

    /// Two pieces of a way in one tile starting at the same point (a road out across a tile's edge
    /// and back in at a unit's distance from where it started) keep their order along the way,
    /// whatever order the threads gave them in.
    #[test]
    fn pieces_starting_at_one_point_keep_their_order_along_the_way() {
        let line = |way: u32, pts: Vec<[i32; 2]>| TileLine { way, pts, ..Default::default() };
        let pieces = vec![
            (5u64, 1u32, line(9, vec![[4096, 10], [4096, 10]])),
            (5, 3, line(9, vec![[4096, 10], [3000, 900]])),
            (5, 2, line(9, vec![[4000, 4], [4096, 10]])),
            (5, 1, line(3, vec![[7, 7], [8, 8]])),
            (4, 1, line(9, vec![[1, 1], [2, 2]])),
        ];
        let mut want = None;
        for k in 0..pieces.len() {
            for rev in [false, true] {
                let mut v = pieces.clone();
                v.rotate_left(k);
                if rev {
                    v.reverse();
                }
                sort_pieces(&mut v);
                let got: Vec<(u64, u32, u32)> = v.iter().map(|p| (p.0, p.2.way, p.1)).collect();
                assert_eq!(got, vec![(4, 9, 1), (5, 3, 1), (5, 9, 2), (5, 9, 1), (5, 9, 3)]);
                want.get_or_insert(got);
            }
        }
    }
}
