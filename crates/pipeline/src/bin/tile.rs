//! Post-process sampled elevations and cut the road tile pyramid.
//!
//! usage: tile <build_dir> [minzoom] [maxzoom]
//!        tile <build_dir> elev      processed elevations only (final.i16, grade.u8), which the
//!                                   scenic stage needs before the tiles can be cut
//!
//! Elevations are post-processed first (see `pipeline::elev`).
//!
//! Tiles: per zoom, combined planar + elevation Douglas-Peucker, clipped exactly at tile
//! edges (round caps make the seams invisible), encoded with `roadcore::tile`.

use anyhow::Result;
use flate2::{write::GzEncoder, Compression};
use pipeline::count_bar;
use rayon::prelude::*;
use roadcore::archive::ArchiveWriter;
use roadcore::tile::{encode, lflag, style, surface, TileLine, NCH};
use roadcore::scenic::ch;
use roadcore::{class, dist_m, flag, merc, Array, Ways, WayRec, E7};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

/// Douglas-Peucker planar tolerance in screen pixels (256 px tiles).
const DP_TOL_PX: f64 = 0.35;

fn extent_log2(z: u8) -> u8 {
    if z >= 14 { 13 } else { 12 }
}

/// Elevation tolerance (m) for simplification at zoom z: 1 m at z14, doubling every 2 zooms out.
fn elev_tol(z: u8) -> f64 {
    2f64.powf((14.0 - z as f64) / 2.0).max(1.0)
}

fn cumdist(v: &[[i32; 2]]) -> Vec<f64> {
    let mut d = Vec::with_capacity(v.len());
    let mut acc = 0.0;
    d.push(0.0);
    for s in v.windows(2) {
        acc += dist_m(s[0][0] as f64 * E7, s[0][1] as f64 * E7, s[1][0] as f64 * E7, s[1][1] as f64 * E7);
        d.push(acc);
    }
    d
}

// ---------------------------------------------------------------------------------------

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

fn style_of(w: &WayRec) -> u8 {
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
fn draw_key(style: u8) -> u8 {
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

/// Encode and store one zoom level's pieces (sorted by tile, draw order, way); returns the
/// zoom's stats for the metadata.
#[allow(clippy::too_many_arguments)]
fn write_zoom(z: u8, maxz: u8, el2: u8, ext: f64, pieces: &[(u64, TileLine)], aw: &mut ArchiveWriter, what: &str, t0: std::time::Instant) -> Result<String> {
        // Group by tile.
        let mut groups: Vec<(u64, usize, usize)> = Vec::new();
        let mut s = 0;
        for i in 1..=pieces.len() {
            if i == pieces.len() || pieces[i].0 != pieces[s].0 {
                groups.push((pieces[s].0, s, i));
                s = i;
            }
        }
        let pb = count_bar(groups.len() as u64, format!("z{z} {what} encode + gzip"));
        let encoded: Vec<(u64, Vec<u8>, usize, usize)> = groups
            .par_iter()
            .map(|&(key, s, e)| {
                // Features shorter than half a pixel are indistinguishable from dots. Below
                // the max zoom (which is overzoomed, so must keep true geometry) collapse them
                // onto a half-pixel grid, merged per style, keeping the summed true length and
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
                    if !(collapse && len < half_px) && !(len == 0.0) {
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
                    let lbin = (l.road_m.max(1) as f64).log10() as u8;
                    let d = dots.entry((cell, l.style, l.flags, lbin)).or_insert(Dot {
                        len: 0.0, e: 0.0, g: 0.0, h: 0.0, ch: [0.0; NCH], flags: 0,
                        best: l.way, best_len: 0, road_m: l.road_m, attr: l.attr, colour: l.colour,
                    });
                    d.len += w;
                    d.e += em * w;
                    d.g += gm * w;
                    d.h += hm * w;
                    for c in 0..NCH {
                        d.ch[c] += l.sc.iter().map(|v| v[c] as f64).sum::<f64>() / n * w;
                    }
                    d.flags |= l.sc.iter().fold(0u8, |a, v| a | v[ch::FLAGS]);
                    if l.true_len_dm >= d.best_len {
                        d.best = l.way;
                        d.best_len = l.true_len_dm;
                        d.road_m = l.road_m;
                        d.attr = l.attr;
                        d.colour = l.colour;
                    }
                }
                for ((cell, style, lflags, _), d) in dots {
                    let pt = if collapse {
                        [((cell[0] as f64 + 0.5) * half_px).round() as i32, ((cell[1] as f64 + 0.5) * half_px).round() as i32]
                    } else {
                        cell
                    };
                    let sw = d.len;
                    let ev = (d.e / sw).round() as i16;
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
                owned.sort_by_key(|l| (draw_key(l.style), l.way));
                let nverts: usize = owned.iter().map(|l| l.pts.len()).sum();
                let raw = encode(&owned, el2);
                let mut gz = GzEncoder::new(Vec::with_capacity(raw.len() / 2), Compression::new(6));
                gz.write_all(&raw).unwrap();
                pb.inc(1);
                (key, gz.finish().unwrap(), raw.len(), nverts)
            })
            .collect();
        pb.finish_and_clear();
        let (mut bytes, mut nv, mut maxb) = (0usize, 0usize, 0usize);
        for (key, gz, raw_len, nverts) in &encoded {
            let tz = (key >> 58) as u8;
            let tx = ((key >> 29) & ((1 << 29) - 1)) as u32;
            let ty = (key & ((1 << 29) - 1)) as u32;
            aw.add(tz, tx, ty, gz, *raw_len)?;
            bytes += gz.len();
            nv += nverts;
            maxb = maxb.max(gz.len());
        }
        eprintln!(
            "z{z:>2} {what}: {:>7} tiles, {:>11} verts, {:>8.1} MB (max tile {:>6.0} KB)  ({:.0?})",
            encoded.len(),
            nv,
            bytes as f64 / 1e6,
            maxb as f64 / 1e3,
            t0.elapsed()
        );
        Ok(format!(
            "\"{z}\":{{\"tiles\":{},\"vertices\":{},\"bytes\":{},\"max_tile\":{}}}",
            encoded.len(),
            nv,
            bytes,
            maxb
        ))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("data/build"));
    let elev_only = args.get(2).map(String::as_str) == Some("elev");
    let minz: u8 = if elev_only { 4 } else { args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(4) };
    let maxz: u8 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(14);
    let t0 = std::time::Instant::now();

    let wv = Ways::open(&dir)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let raw = Array::<f32>::open(&dir.join("elev.f32"))?;
    // Scenic inputs (optional: absent before the scenic stage has run).
    let drape_a = Array::<i16>::open(&dir.join("vterrain.i16")).ok();
    let sc_a = Array::<[u8; NCH]>::open(&dir.join("scenic.u8")).ok();
    let drape_all: Option<&[i16]> = drape_a.as_ref().map(|a| a.get()).filter(|a| a.len() == verts.len());
    let sc_all: Option<&[[u8; NCH]]> = sc_a.as_ref().map(|a| a.get()).filter(|a| a.len() == verts.len());
    eprintln!("drape heights: {}, scenic channels: {}", drape_all.is_some(), sc_all.is_some());
    let raw = raw.get();
    eprintln!("{} ways, {} vertices", ways.len(), verts.len());

    let strings = roadcore::read_strings(&dir)?;
    let net = pipeline::elev::Net::build(ways, verts);
    let p = pipeline::elev::process(&net, raw);
    if elev_only {
        let dm: Vec<i16> = p.elev.iter().map(|&e| (e * 10.0).round().clamp(-32000.0, 32000.0) as i16).collect();
        std::fs::write(roadcore::tmp(&dir, "final.i16"), bytemuck::cast_slice(&dm))?;
        std::fs::write(roadcore::tmp(&dir, "grade.u8"), &p.grade)?;
        roadcore::commit(&dir, &["final.i16", "grade.u8"])?;
        eprintln!("elevations processed ({:.0?})", t0.elapsed());
        return Ok(());
    }
    let cl = pipeline::climbs::find(&net, &p.elev);
    std::fs::write(roadcore::tmp(&dir, "climbs.bin"), bytemuck::cast_slice(&cl.recs))?;
    std::fs::write(roadcore::tmp(&dir, "climbs.geom"), bytemuck::cast_slice(&cl.geom))?;
    // Strokes (chained ways) for the server's scenic-drive ranking: offsets + items, where an
    // item is a way index with the top bit set when traversed in reverse.
    let mut offs: Vec<u32> = vec![0];
    let mut items: Vec<u32> = Vec::new();
    for st in &cl.strokes {
        items.extend(st.iter().map(|&(w, r)| w | if r { 1 << 31 } else { 0 }));
        offs.push(items.len() as u32);
    }
    std::fs::write(roadcore::tmp(&dir, "strokes.off"), bytemuck::cast_slice(&offs))?;
    std::fs::write(roadcore::tmp(&dir, "strokes.u32"), bytemuck::cast_slice(&items))?;
    drop(cl);
    // Whole-road lengths, for the length filter (per line in the tiles, per way for the server).
    let road_len = pipeline::roads::lengths(&net, &strings);
    std::fs::write(roadcore::tmp(&dir, "roadlen.f32"), bytemuck::cast_slice(&road_len))?;
    drop(net);
    let dm: Vec<i16> = p.elev.iter().map(|&e| (e * 10.0).round().clamp(-32000.0, 32000.0) as i16).collect();
    std::fs::write(roadcore::tmp(&dir, "final.i16"), bytemuck::cast_slice(&dm))?;
    std::fs::write(roadcore::tmp(&dir, "grade.u8"), &p.grade)?;
    eprintln!("elevations processed ({:.0?})", t0.elapsed());

    // Global stats for the metadata (length-weighted elevation histogram, 10 m bins).
    let mut hist = vec![0f64; 256];
    let (mut emin, mut emax) = (f32::MAX, f32::MIN);
    for w in ways.iter().filter(|w| !class::is_rail(w.class)) {
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        let v = &verts[r.clone()];
        let e = &p.elev[r];
        for k in 1..v.len() {
            let l = dist_m(v[k - 1][0] as f64 * E7, v[k - 1][1] as f64 * E7, v[k][0] as f64 * E7, v[k][1] as f64 * E7);
            let m = (e[k - 1] + e[k]) * 0.5;
            hist[((m / 10.0).max(0.0) as usize).min(255)] += l / 1000.0;
        }
        for &x in e {
            emin = emin.min(x);
            emax = emax.max(x);
        }
    }

    // Rail track length per service group (a track counts for every group using it).
    let mut rail_km = [0f64; 5];
    for w in ways.iter().filter(|w| class::is_rail(w.class)) {
        let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
        let l: f64 = verts[r].windows(2).map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7)).sum();
        for (k, km) in rail_km.iter_mut().enumerate() {
            if w.rail >> k & 1 == 1 {
                *km += l;
            }
        }
    }

    // ---- tiles ------------------------------------------------------------------------
    let mut aw = ArchiveWriter::create(&roadcore::tmp(&dir, "roads.tiles"), "{}")?;
    let mut rw = ArchiveWriter::create(&roadcore::tmp(&dir, "rails.tiles"), "{}")?;
    let mut zoom_stats = Vec::new();
    let mut rail_stats = Vec::new();
    // Pre-project all vertices to normalised Web Mercator once.
    let (mut bminx, mut bminy, mut bmaxx, mut bmaxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for v in verts.iter().step_by(97) {
        let (x, y) = (v[0] as f64 * E7, v[1] as f64 * E7);
        bminx = bminx.min(x);
        bmaxx = bmaxx.max(x);
        bminy = bminy.min(y);
        bmaxy = bmaxy.max(y);
    }

    for z in minz..=maxz {
        let el2 = extent_log2(z);
        let ext = (1u64 << el2) as f64;
        let scale = (1u64 << z) as f64 * ext;
        let tol = DP_TOL_PX * ext / 256.0;
        let etol = elev_tol(z);
        let pb = count_bar(ways.len() as u64, format!("z{z} simplify + clip"));
        let mut pieces: Vec<(u64, TileLine)> = ways
            .par_iter()
            .enumerate()
            .fold(Vec::new, |mut acc, (wi, w)| {
                let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
                // Projected per way (a whole-network array would be 6.6 GB for Europe + North America).
                let xy: Vec<[f64; 2]> = verts[r.clone()]
                    .iter()
                    .map(|v| {
                        let (x, y) = merc(v[0] as f64 * E7, v[1] as f64 * E7);
                        [x * scale, y * scale]
                    })
                    .collect();
                let e = &p.elev[r.clone()];
                let keep = simplify(&xy, e, tol, etol);
                let cd = cumdist(&verts[r.clone()]);
                let pts: Vec<P> = keep
                    .iter()
                    .map(|&k| P {
                        x: xy[k][0],
                        y: xy[k][1],
                        e: e[k],
                        g: p.grade[r.start + k] as f32,
                        cd: cd[k],
                        h: drape_all.map_or(e[k], |d| d[r.start + k] as f32),
                        sc: sc_all.map_or_else(
                            || {
                                let mut z = [0f32; NCH];
                                z[ch::TPI] = 128.0;
                                z
                            },
                            |a| a[r.start + k].map(|v| v as f32),
                        ),
                    })
                    .collect();
                let st = style_of(w);
                let mut lf = if w.name == 0 && w.ref_ == 0 { lflag::UNNAMED } else { 0 };
                if class::is_rail(w.class) {
                    lf |= w.rail << lflag::RAIL_SHIFT;
                } else {
                    if w.flags & flag::ONEWAY != 0 {
                        lf |= lflag::ONEWAY;
                    }
                    if w.flags & flag::TOLL != 0 {
                        lf |= lflag::TOLL;
                    }
                }
                let road_m = road_len[wi].round() as u32;
                let attr = [w.network, (w.maxspeed / 2).min(255) as u8, w.lanes, surface::code(strings[w.surface as usize].as_str())];
                let colour = if w.colour != 0 { (w.colour & 0xff_ffff) + 1 } else { 0 };
                clip_to_grid(&pts, ext, |tx, ty, piece| {
                    let (ox, oy) = (tx as f64 * ext, ty as f64 * ext);
                    let true_len = piece.last().unwrap().cd - piece[0].cd;
                    let mut tl = TileLine {
                        style: st,
                        flags: lf,
                        way: wi as u32,
                        true_len_dm: (true_len * 10.0).round().max(0.0) as u32,
                        road_m,
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
                        tl.elev.push((q.e * 10.0).round().clamp(-32000.0, 32000.0) as i16);
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
                    let key = roadcore::archive::tile_key(z, tx, ty);
                    acc.push((key, tl));
                });
                pb.inc(1);
                acc
            })
            .reduce(Vec::new, |mut a, mut b| {
                if a.len() < b.len() {
                    std::mem::swap(&mut a, &mut b);
                }
                a.append(&mut b);
                a
            });
        pb.finish_and_clear();
        pieces.par_sort_unstable_by(|a, b| {
            a.0.cmp(&b.0)
                .then(draw_key(a.1.style).cmp(&draw_key(b.1.style)))
                .then(a.1.way.cmp(&b.1.way))
        });
        // Roads and rail go to their own archives.
        let (rail_pieces, road_pieces): (Vec<(u64, TileLine)>, Vec<(u64, TileLine)>) =
            pieces.into_iter().partition(|p| class::is_rail(p.1.style & 0x0f));
        zoom_stats.push(write_zoom(z, maxz, el2, ext, &road_pieces, &mut aw, "roads", t0)?);
        rail_stats.push(write_zoom(z, maxz, el2, ext, &rail_pieces, &mut rw, "rails", t0)?);
    }
    aw.finish()?;
    rw.finish()?;

    let dem_stats = std::fs::read_to_string(dir.join("dem-stats.json")).unwrap_or_else(|_| "{}".into());
    let hist_s: Vec<String> = hist.iter().map(|v| format!("{:.1}", v)).collect();
    let meta = format!(
        "{{\"minzoom\":{minz},\"maxzoom\":{maxz},\
         \"bounds\":[{bminx:.4},{bminy:.4},{bmaxx:.4},{bmaxy:.4}],\"ways\":{},\"vertices\":{},\
         \"elev_min\":{emin:.1},\"elev_max\":{emax:.1},\"elev_hist_10m_km\":[{}],\
         \"classes\":{:?},\"zooms\":{{{}}},\"rail_zooms\":{{{}}},\"rail_km\":[{}],\"dem\":{},\"built\":{}}}",
        ways.len(),
        verts.len(),
        hist_s.join(","),
        class::NAMES,
        zoom_stats.join(","),
        rail_stats.join(","),
        rail_km.iter().map(|v| format!("{:.1}", v / 1000.0)).collect::<Vec<_>>().join(","),
        dem_stats.trim(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()
    );
    std::fs::write(roadcore::tmp(&dir, "roads.json"), meta)?;
    // final.i16 / grade.u8 are usually what `tile elev` wrote already: a rewrite would make the
    // scenic steps that read them look out of date.
    roadcore::commit_if_changed(&dir, &["final.i16", "grade.u8"])?;
    roadcore::commit(&dir, &["climbs.bin", "climbs.geom", "strokes.off", "strokes.u32", "roadlen.f32", "roads.tiles", "rails.tiles", "roads.json"])?;
    eprintln!("done in {:.0?}", t0.elapsed());
    Ok(())
}
